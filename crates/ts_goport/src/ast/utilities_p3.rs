//! Go `internal/ast/utilities.go` lines 1806-2728.

use crate::prelude::*;

// Go: ast/utilities.go:1864 GetSuperContainer
pub fn get_super_container(node: Node, stop_on_functions: bool) -> Node {
    let mut node = node.parent();
    while node.is_some() {
        match node.kind() {
            SyntaxKind::ComputedPropertyName => {
                node = node.parent();
            }
            SyntaxKind::FunctionDeclaration
            | SyntaxKind::FunctionExpression
            | SyntaxKind::ArrowFunction => {
                if stop_on_functions {
                    return node;
                }
                // Go `continue`: falls through to the loop post statement.
            }
            SyntaxKind::PropertyDeclaration
            | SyntaxKind::PropertySignature
            | SyntaxKind::MethodDeclaration
            | SyntaxKind::MethodSignature
            | SyntaxKind::Constructor
            | SyntaxKind::GetAccessor
            | SyntaxKind::SetAccessor
            | SyntaxKind::ClassStaticBlockDeclaration => {
                return node;
            }
            SyntaxKind::Decorator => {
                // Decorators are always applied outside of the body of a class or method.
                if node.parent().kind() == SyntaxKind::Parameter
                    && is_class_element(node.parent().parent())
                {
                    // If the decorator's parent is a ParameterDeclaration, we resolve the this container from
                    // the grandparent class declaration.
                    node = node.parent().parent();
                } else if is_class_element(node.parent()) {
                    // If the decorator's parent is a class element, we resolve the 'this' container
                    // from the parent class declaration.
                    node = node.parent();
                }
            }
            _ => {}
        }
        node = node.parent();
    }
    Node::NIL
}

// Go: ast/utilities.go:1892 GetImmediatelyInvokedFunctionExpression
pub fn get_immediately_invoked_function_expression(fn_: Node) -> Node {
    if is_function_expression_or_arrow_function(fn_) {
        let mut prev = fn_;
        let mut parent = fn_.parent();
        while is_parenthesized_expression(parent) {
            prev = parent;
            parent = parent.parent();
        }
        if is_call_expression(parent) && parent.expression() == prev {
            return parent;
        }
    }
    Node::NIL
}

// Go: ast/utilities.go:1907 IsEnumConst
pub fn is_enum_const(node: Node) -> bool {
    get_combined_modifier_flags(node).intersects(ModifierFlags::CONST)
}

// Go: ast/utilities.go:1911 ExpressionIsAlias
pub fn expression_is_alias(node: Node) -> bool {
    is_entity_name_expression(node) || is_class_expression(node)
}

// Go: ast/utilities.go:1915 IsInstanceOfExpression
pub fn is_instance_of_expression(node: Node) -> bool {
    is_binary_expression(node) && node.operator_token().kind() == SyntaxKind::InstanceOfKeyword
}

// Go: ast/utilities.go:1919 IsAnyImportOrReExport
pub fn is_any_import_or_re_export(node: Node) -> bool {
    is_import_node(node) || is_export_declaration(node)
}

// Go: ast/utilities.go:1923 IsImportNode
pub fn is_import_node(node: Node) -> bool {
    is_any_import_syntax(node) || node_kind_is(node, &[SyntaxKind::JsImportDeclaration])
}

// Checks if the node is a genuine import declation. In particular the re-parsed KindJSImportDeclaration
// is explicitly excluded because the callers of this function are typically not prepared to handle it properly.
// For more permissive check, use IsImportNode.
// Go: ast/utilities.go:1930 IsAnyImportSyntax
pub fn is_any_import_syntax(node: Node) -> bool {
    node_kind_is(
        node,
        &[
            SyntaxKind::ImportDeclaration,
            SyntaxKind::ImportEqualsDeclaration,
        ],
    )
}

// Go: ast/utilities.go:1934 IsJsonSourceFile
pub fn is_json_source_file(file: Node) -> bool {
    with_source_file_info(file, |info| info.script_kind == ScriptKind::JSON)
}

// Go: ast/utilities.go:1938 IsInJsonFile
pub fn is_in_json_file(node: Node) -> bool {
    node.flags().intersects(NodeFlags::JSON_FILE)
}

// Go: ast/utilities.go:1942 GetExternalModuleName
pub fn get_external_module_name(node: Node) -> Node {
    match node.kind() {
        SyntaxKind::ImportDeclaration
        | SyntaxKind::JsImportDeclaration
        | SyntaxKind::ExportDeclaration => {
            return node.module_specifier();
        }
        SyntaxKind::ImportEqualsDeclaration => {
            if node.module_reference().kind() == SyntaxKind::ExternalModuleReference {
                return node.module_reference().expression();
            }
            return Node::NIL;
        }
        SyntaxKind::ImportType => {
            return get_import_type_node_literal(node);
        }
        SyntaxKind::CallExpression => {
            // Go core.FirstOrNil(node.Arguments())
            let args = node.arguments();
            if args.is_empty() {
                return Node::NIL;
            }
            return args.get(0);
        }
        SyntaxKind::ModuleDeclaration => {
            if is_string_literal(node.name()) {
                return node.name();
            }
            return Node::NIL;
        }
        _ => {}
    }
    panic!("Unhandled case in getExternalModuleName")
}

// Go: ast/utilities.go:1964 HasImportAttributes (ts#63931)
pub fn has_import_attributes(node: Node) -> bool {
    matches!(
        node.kind(),
        SyntaxKind::ImportDeclaration
            | SyntaxKind::JsImportDeclaration
            | SyntaxKind::ExportDeclaration
            | SyntaxKind::ImportType
    )
}

// Go: ast/utilities.go:1972 GetImportAttributes
pub fn get_import_attributes(node: Node) -> Node {
    match node.kind() {
        SyntaxKind::ImportDeclaration | SyntaxKind::JsImportDeclaration => {
            return node.attributes();
        }
        SyntaxKind::ExportDeclaration => {
            return node.attributes();
        }
        SyntaxKind::ImportType => {
            return node.attributes();
        }
        _ => {}
    }
    panic!("Unhandled case in getImportAttributes: {:?}", node.kind())
}

// Go: ast/utilities.go:1984 getImportTypeNodeLiteral
pub fn get_import_type_node_literal(node: Node) -> Node {
    if is_import_type_node(node) {
        let argument = node.argument();
        if is_literal_type_node(argument) {
            let literal = argument.literal();
            if is_string_literal(literal) {
                return literal;
            }
        }
    }
    Node::NIL
}

// Go: ast/utilities.go:1997 IsExpressionNode
pub fn is_expression_node(node: Node) -> bool {
    match node.kind() {
        SyntaxKind::SuperKeyword
        | SyntaxKind::NullKeyword
        | SyntaxKind::TrueKeyword
        | SyntaxKind::FalseKeyword
        | SyntaxKind::RegularExpressionLiteral
        | SyntaxKind::ArrayLiteralExpression
        | SyntaxKind::ObjectLiteralExpression
        | SyntaxKind::PropertyAccessExpression
        | SyntaxKind::ElementAccessExpression
        | SyntaxKind::CallExpression
        | SyntaxKind::NewExpression
        | SyntaxKind::TaggedTemplateExpression
        | SyntaxKind::AsExpression
        | SyntaxKind::TypeAssertionExpression
        | SyntaxKind::SatisfiesExpression
        | SyntaxKind::NonNullExpression
        | SyntaxKind::ParenthesizedExpression
        | SyntaxKind::FunctionExpression
        | SyntaxKind::ClassExpression
        | SyntaxKind::ArrowFunction
        | SyntaxKind::VoidExpression
        | SyntaxKind::DeleteExpression
        | SyntaxKind::TypeOfExpression
        | SyntaxKind::PrefixUnaryExpression
        | SyntaxKind::PostfixUnaryExpression
        | SyntaxKind::BinaryExpression
        | SyntaxKind::ConditionalExpression
        | SyntaxKind::SpreadElement
        | SyntaxKind::TemplateExpression
        | SyntaxKind::OmittedExpression
        | SyntaxKind::JsxElement
        | SyntaxKind::JsxSelfClosingElement
        | SyntaxKind::JsxFragment
        | SyntaxKind::YieldExpression
        | SyntaxKind::AwaitExpression => true,
        SyntaxKind::MetaProperty => {
            // `import.<phase>` in `import.<phase>(...)` is not an expression
            !is_import_call(node.parent()) || node.parent().expression() != node
        }
        SyntaxKind::ExpressionWithTypeArguments => !is_heritage_clause(node.parent()),
        SyntaxKind::QualifiedName => {
            let mut node = node;
            while node.parent().kind() == SyntaxKind::QualifiedName {
                node = node.parent();
            }
            is_type_query_node(node.parent())
                || is_js_doc_link_like(node.parent())
                || is_js_doc_name_reference(node.parent())
                || is_jsx_tag_name(node)
        }
        SyntaxKind::PrivateIdentifier => {
            is_binary_expression(node.parent())
                && node.parent().left() == node
                && node.parent().operator_token().kind() == SyntaxKind::InKeyword
        }
        SyntaxKind::Identifier => {
            if is_type_query_node(node.parent())
                || is_js_doc_link_like(node.parent())
                || is_js_doc_name_reference(node.parent())
                || is_jsx_tag_name(node)
            {
                return true;
            }
            // Go `fallthrough` into the literal case.
            is_in_expression_context(node)
        }
        SyntaxKind::NumericLiteral
        | SyntaxKind::BigIntLiteral
        | SyntaxKind::StringLiteral
        | SyntaxKind::NoSubstitutionTemplateLiteral
        | SyntaxKind::ThisKeyword => is_in_expression_context(node),
        _ => false,
    }
}

// Go: ast/utilities.go:2032 IsInExpressionContext
pub fn is_in_expression_context(node: Node) -> bool {
    let parent = node.parent();
    match parent.kind() {
        SyntaxKind::VariableDeclaration
        | SyntaxKind::Parameter
        | SyntaxKind::PropertyDeclaration
        | SyntaxKind::PropertySignature
        | SyntaxKind::EnumMember
        | SyntaxKind::PropertyAssignment
        | SyntaxKind::BindingElement => parent.initializer() == node,
        SyntaxKind::ExpressionStatement
        | SyntaxKind::IfStatement
        | SyntaxKind::DoStatement
        | SyntaxKind::WhileStatement
        | SyntaxKind::ReturnStatement
        | SyntaxKind::WithStatement
        | SyntaxKind::SwitchStatement
        | SyntaxKind::CaseClause
        | SyntaxKind::DefaultClause
        | SyntaxKind::ThrowStatement
        | SyntaxKind::TypeAssertionExpression
        | SyntaxKind::AsExpression
        | SyntaxKind::TemplateSpan
        | SyntaxKind::ComputedPropertyName
        | SyntaxKind::SatisfiesExpression => parent.expression() == node,
        SyntaxKind::ForStatement => {
            let initializer = parent.initializer();
            initializer == node && initializer.kind() != SyntaxKind::VariableDeclarationList
                || parent.condition() == node
                || parent.incrementor() == node
        }
        SyntaxKind::ForInStatement | SyntaxKind::ForOfStatement => {
            let initializer = parent.initializer();
            initializer == node && initializer.kind() != SyntaxKind::VariableDeclarationList
                || parent.expression() == node
        }
        SyntaxKind::Decorator
        | SyntaxKind::JsxExpression
        | SyntaxKind::JsxSpreadAttribute
        | SyntaxKind::SpreadAssignment => true,
        SyntaxKind::ExpressionWithTypeArguments => {
            parent.expression() == node && !is_part_of_type_node(parent)
        }
        SyntaxKind::ShorthandPropertyAssignment => parent.object_assignment_initializer() == node,
        SyntaxKind::FunctionExpression | SyntaxKind::ClassExpression => {
            // The name of a function or class expression is a declaration name, not an expression.
            parent.name() != node
        }
        _ => is_expression_node(parent),
    }
}

// Go: ast/utilities.go:2058 IsPartOfTypeNode
pub fn is_part_of_type_node(node: Node) -> bool {
    let kind = node.kind();
    if (kind as u16) >= (SyntaxKind::FIRST_TYPE_NODE as u16)
        && (kind as u16) <= (SyntaxKind::LAST_TYPE_NODE as u16)
    {
        return true;
    }
    match node.kind() {
        SyntaxKind::AnyKeyword
        | SyntaxKind::UnknownKeyword
        | SyntaxKind::NumberKeyword
        | SyntaxKind::BigIntKeyword
        | SyntaxKind::StringKeyword
        | SyntaxKind::BooleanKeyword
        | SyntaxKind::SymbolKeyword
        | SyntaxKind::ObjectKeyword
        | SyntaxKind::UndefinedKeyword
        | SyntaxKind::NullKeyword
        | SyntaxKind::NeverKeyword => {
            return true;
        }
        SyntaxKind::VoidKeyword => {
            return node.parent().kind() != SyntaxKind::VoidExpression;
        }
        SyntaxKind::ExpressionWithTypeArguments => {
            return is_part_of_type_expression_with_type_arguments(node);
        }
        SyntaxKind::TypeParameter => {
            return node.parent().kind() == SyntaxKind::MappedType
                || node.parent().kind() == SyntaxKind::InferType;
        }
        SyntaxKind::Identifier => {
            let parent = node.parent();
            if is_qualified_name(parent) && parent.right() == node {
                return is_part_of_type_node_in_parent(parent);
            }
            if is_property_access_expression(parent) && parent.name() == node {
                return is_part_of_type_node_in_parent(parent);
            }
            return is_part_of_type_node_in_parent(node);
        }
        SyntaxKind::QualifiedName
        | SyntaxKind::PropertyAccessExpression
        | SyntaxKind::ThisKeyword => {
            return is_part_of_type_node_in_parent(node);
        }
        _ => {}
    }
    false
}

// Go: ast/utilities.go:2089 isPartOfTypeNodeInParent
pub fn is_part_of_type_node_in_parent(node: Node) -> bool {
    let parent = node.parent();
    if parent.kind() == SyntaxKind::TypeQuery {
        return false;
    }
    if parent.kind() == SyntaxKind::ImportType {
        return !parent.is_type_of();
    }

    // Do not recursively call isPartOfTypeNode on the parent. In the example:
    //
    //     let a: A.B.C;
    //
    // Calling isPartOfTypeNode would consider the qualified name A.B a type node.
    // Only C and A.B.C are type nodes.
    if (parent.kind() as u16) >= (SyntaxKind::FIRST_TYPE_NODE as u16)
        && (parent.kind() as u16) <= (SyntaxKind::LAST_TYPE_NODE as u16)
    {
        return true;
    }
    match parent.kind() {
        SyntaxKind::ExpressionWithTypeArguments => {
            return is_part_of_type_expression_with_type_arguments(parent);
        }
        SyntaxKind::TypeParameter => {
            return node == parent.constraint();
        }
        SyntaxKind::VariableDeclaration
        | SyntaxKind::Parameter
        | SyntaxKind::PropertyDeclaration
        | SyntaxKind::PropertySignature
        | SyntaxKind::FunctionDeclaration
        | SyntaxKind::FunctionExpression
        | SyntaxKind::ArrowFunction
        | SyntaxKind::Constructor
        | SyntaxKind::MethodDeclaration
        | SyntaxKind::MethodSignature
        | SyntaxKind::GetAccessor
        | SyntaxKind::SetAccessor
        | SyntaxKind::CallSignature
        | SyntaxKind::ConstructSignature
        | SyntaxKind::IndexSignature
        | SyntaxKind::TypeAssertionExpression => {
            return node == parent.type_();
        }
        SyntaxKind::CallExpression
        | SyntaxKind::NewExpression
        | SyntaxKind::TaggedTemplateExpression => {
            return parent.type_arguments().to_vec().contains(&node);
        }
        _ => {}
    }
    false
}

// Go: ast/utilities.go:2123 isPartOfTypeExpressionWithTypeArguments
pub fn is_part_of_type_expression_with_type_arguments(node: Node) -> bool {
    let parent = node.parent();
    is_heritage_clause(parent)
        && (!is_class_like(parent.parent()) || parent.token() == SyntaxKind::ImplementsKeyword)
        || is_js_doc_implements_tag(parent)
        || is_js_doc_augments_tag(parent)
}

// Go: ast/utilities.go:2130 IsJSDocLinkLike
pub fn is_js_doc_link_like(node: Node) -> bool {
    node_kind_is(
        node,
        &[
            SyntaxKind::JsDocLink,
            SyntaxKind::JsDocLinkCode,
            SyntaxKind::JsDocLinkPlain,
        ],
    )
}

// Go: ast/utilities.go:2134 IsJSDocTag
pub fn is_js_doc_tag(node: Node) -> bool {
    (node.kind() as u16) >= (SyntaxKind::FIRST_JS_DOC_TAG_NODE as u16)
        && (node.kind() as u16) <= (SyntaxKind::LAST_JS_DOC_TAG_NODE as u16)
}

// Go: ast/utilities.go:2138 IsSuperCall
pub fn is_super_call(node: Node) -> bool {
    is_call_expression(node) && node.expression().kind() == SyntaxKind::SuperKeyword
}

// Go: ast/utilities.go:2145 IsImportCall
pub fn is_import_call(node: Node) -> bool {
    if !is_call_expression(node) {
        return false;
    }
    let e = node.expression();
    e.kind() == SyntaxKind::ImportKeyword || is_import_phase_meta_property(e)
}

// Go: ast/utilities.go:2153 IsImportPhaseMetaProperty (ts#63915)
pub fn is_import_phase_meta_property(node: Node) -> bool {
    is_import_defer_meta_property(node) || is_import_source_meta_property(node)
}

// Go: ast/utilities.go:2157 IsImportDeferMetaProperty (ts#63915)
pub fn is_import_defer_meta_property(node: Node) -> bool {
    is_import_meta_property(node, "defer")
}

// Go: ast/utilities.go:2161 IsImportSourceMetaProperty (ts#63915)
pub fn is_import_source_meta_property(node: Node) -> bool {
    is_import_meta_property(node, "source")
}

// Go: ast/utilities.go:2165 isImportMetaProperty (ts#63915)
fn is_import_meta_property(node: Node, name: &str) -> bool {
    is_meta_property(node)
        && node.keyword_token() == SyntaxKind::ImportKeyword
        && node.name().text() == name
}

// Go: ast/utilities.go:2169 IsSourcePhaseImport (ts#63915)
pub fn is_source_phase_import(node: Node) -> bool {
    if is_import_declaration(node) {
        let clause = node.import_clause();
        return clause.is_some() && clause.phase_modifier() == SyntaxKind::SourceKeyword;
    }
    is_source_phase_import_call(node)
}

// Go: ast/utilities.go:2177 IsSourcePhaseImportCall (ts#63915)
pub fn is_source_phase_import_call(node: Node) -> bool {
    is_call_expression(node) && is_import_source_meta_property(node.expression())
}

// Go: ast/utilities.go:2150 IsComputedNonLiteralName
pub fn is_computed_non_literal_name(name: Node) -> bool {
    is_computed_property_name(name) && !is_string_or_numeric_literal_like(name.expression())
}

// Go: ast/utilities.go:2154 IsQuestionToken
pub fn is_question_token(node: Node) -> bool {
    node.is_some() && node.kind() == SyntaxKind::QuestionToken
}

// PORT: Go `getTextOfNode func(*Node) string` may be nil, so it is an Option.
// Go: ast/utilities.go:2158 EntityNameToString
pub fn entity_name_to_string(
    name: Node,
    get_text_of_node: Option<&dyn Fn(Node) -> String>,
) -> String {
    match name.kind() {
        SyntaxKind::ThisKeyword => {
            return "this".to_string();
        }
        SyntaxKind::Identifier | SyntaxKind::PrivateIdentifier => match get_text_of_node {
            Some(f) if !node_is_synthesized(name) => return f(name),
            _ => return name.text().to_string(),
        },
        SyntaxKind::QualifiedName => {
            return entity_name_to_string(name.left(), get_text_of_node)
                + "."
                + &entity_name_to_string(name.right(), get_text_of_node);
        }
        SyntaxKind::PropertyAccessExpression => {
            return entity_name_to_string(name.expression(), get_text_of_node)
                + "."
                + &entity_name_to_string(name.name(), get_text_of_node);
        }
        SyntaxKind::JsxNamespacedName => {
            return entity_name_to_string(name.namespace(), get_text_of_node)
                + ":"
                + &entity_name_to_string(name.name(), get_text_of_node);
        }
        _ => {}
    }
    panic!("Unhandled case in EntityNameToString")
}

// Go: ast/utilities.go:2177 GetTextOfPropertyName
pub fn get_text_of_property_name(name: Node) -> String {
    let (text, _) = try_get_text_of_property_name(name);
    text
}

// Go: ast/utilities.go:2182 TryGetTextOfPropertyName
pub fn try_get_text_of_property_name(name: Node) -> (String, bool) {
    match name.kind() {
        SyntaxKind::Identifier
        | SyntaxKind::PrivateIdentifier
        | SyntaxKind::StringLiteral
        | SyntaxKind::NumericLiteral
        | SyntaxKind::BigIntLiteral
        | SyntaxKind::NoSubstitutionTemplateLiteral => {
            return (name.text().to_string(), true);
        }
        SyntaxKind::ComputedPropertyName => {
            if is_string_or_numeric_literal_like(name.expression()) {
                return (name.expression().text().to_string(), true);
            }
        }
        SyntaxKind::JsxNamespacedName => {
            return (
                format!("{}:{}", name.namespace().text(), name.name().text()),
                true,
            );
        }
        _ => {}
    }
    (String::new(), false)
}

// Go: ast/utilities.go:2197 IsJSDocNode
pub fn is_js_doc_node(node: Node) -> bool {
    (node.kind() as u16) >= (SyntaxKind::FIRST_JS_DOC_NODE as u16)
        && (node.kind() as u16) <= (SyntaxKind::LAST_JS_DOC_NODE as u16)
}

// Go: ast/utilities.go:2201 IsNonWhitespaceToken
pub fn is_non_whitespace_token(node: Node) -> bool {
    is_token_kind(node.kind()) && !is_whitespace_only_jsx_text(node)
}

// Go: ast/utilities.go:2205 IsWhitespaceOnlyJsxText
pub fn is_whitespace_only_jsx_text(node: Node) -> bool {
    node.kind() == SyntaxKind::JsxText && node.contains_only_trivia_white_spaces()
}

// Go: ast/utilities.go:2209 GetNewTargetContainer
pub fn get_new_target_container(node: Node) -> Node {
    let container = get_this_container(
        node, false, /*includeArrowFunctions*/
        false, /*includeClassComputedPropertyName*/
    );
    if container.is_some() {
        match container.kind() {
            SyntaxKind::Constructor
            | SyntaxKind::FunctionDeclaration
            | SyntaxKind::FunctionExpression => {
                return container;
            }
            _ => {}
        }
    }
    Node::NIL
}

// Go: ast/utilities.go:2220 GetEnclosingBlockScopeContainer
pub fn get_enclosing_block_scope_container(node: Node) -> Node {
    find_ancestor(node.parent(), &mut |current: Node| {
        is_block_scope(current, current.parent())
    })
}

// Go: ast/utilities.go:2226 IsBlockScope
pub fn is_block_scope(node: Node, parent_node: Node) -> bool {
    match node.kind() {
        SyntaxKind::SourceFile
        | SyntaxKind::CaseBlock
        | SyntaxKind::CatchClause
        | SyntaxKind::ModuleDeclaration
        | SyntaxKind::ForStatement
        | SyntaxKind::ForInStatement
        | SyntaxKind::ForOfStatement
        | SyntaxKind::Constructor
        | SyntaxKind::MethodDeclaration
        | SyntaxKind::GetAccessor
        | SyntaxKind::SetAccessor
        | SyntaxKind::FunctionDeclaration
        | SyntaxKind::FunctionExpression
        | SyntaxKind::ArrowFunction
        | SyntaxKind::PropertyDeclaration
        | SyntaxKind::ClassStaticBlockDeclaration => {
            return true;
        }
        SyntaxKind::Block => {
            // function block is not considered block-scope container
            // see comment in binder.ts: bind(...), case for SyntaxKind.Block
            return !is_function_like_or_class_static_block_declaration(parent_node);
        }
        _ => {}
    }
    false
}

// Go type SemanticMeaning (ast/utilities.go:2172) lives in crate::flags.

// Go: ast/utilities.go:2250 GetMeaningFromDeclaration
pub fn get_meaning_from_declaration(node: Node) -> SemanticMeaning {
    match node.kind() {
        SyntaxKind::VariableDeclaration => {
            return SemanticMeaning::VALUE;
        }
        SyntaxKind::Parameter
        | SyntaxKind::BindingElement
        | SyntaxKind::PropertyDeclaration
        | SyntaxKind::PropertySignature
        | SyntaxKind::PropertyAssignment
        | SyntaxKind::ShorthandPropertyAssignment
        | SyntaxKind::MethodDeclaration
        | SyntaxKind::MethodSignature
        | SyntaxKind::Constructor
        | SyntaxKind::GetAccessor
        | SyntaxKind::SetAccessor
        | SyntaxKind::FunctionDeclaration
        | SyntaxKind::FunctionExpression
        | SyntaxKind::ArrowFunction
        | SyntaxKind::CatchClause
        | SyntaxKind::JsxAttribute => {
            return SemanticMeaning::VALUE;
        }

        SyntaxKind::TypeParameter
        | SyntaxKind::InterfaceDeclaration
        | SyntaxKind::TypeAliasDeclaration
        | SyntaxKind::JsTypeAliasDeclaration
        | SyntaxKind::TypeLiteral => {
            return SemanticMeaning::TYPE;
        }
        SyntaxKind::EnumMember | SyntaxKind::ClassDeclaration => {
            return SemanticMeaning::VALUE | SemanticMeaning::TYPE;
        }

        SyntaxKind::ModuleDeclaration => {
            if is_ambient_module(node) {
                return SemanticMeaning::NAMESPACE | SemanticMeaning::VALUE;
            } else if get_module_instance_state(node) == ModuleInstanceState::INSTANTIATED {
                return SemanticMeaning::NAMESPACE | SemanticMeaning::VALUE;
            } else {
                return SemanticMeaning::NAMESPACE;
            }
        }

        SyntaxKind::EnumDeclaration
        | SyntaxKind::NamedImports
        | SyntaxKind::ImportSpecifier
        | SyntaxKind::ImportEqualsDeclaration
        | SyntaxKind::ImportDeclaration
        | SyntaxKind::JsImportDeclaration
        | SyntaxKind::ExportAssignment
        | SyntaxKind::ExportDeclaration => {
            return SemanticMeaning::ALL;
        }

        // An external module can be a Value
        SyntaxKind::SourceFile => {
            return SemanticMeaning::NAMESPACE | SemanticMeaning::VALUE;
        }
        _ => {}
    }

    SemanticMeaning::ALL
}

// Go: ast/utilities.go:2308 IsPropertyAccessOrQualifiedName
pub fn is_property_access_or_qualified_name(node: Node) -> bool {
    node.kind() == SyntaxKind::PropertyAccessExpression || node.kind() == SyntaxKind::QualifiedName
}

// Go: ast/utilities.go:2312 IsLabelName
pub fn is_label_name(node: Node) -> bool {
    is_label_of_labeled_statement(node) || is_jump_statement_target(node)
}

// Go: ast/utilities.go:2316 IsLabelOfLabeledStatement
pub fn is_label_of_labeled_statement(node: Node) -> bool {
    if !is_identifier(node) {
        return false;
    }
    if !is_labeled_statement(node.parent()) {
        return false;
    }
    node == node.parent().label()
}

// Go: ast/utilities.go:2326 IsJumpStatementTarget
pub fn is_jump_statement_target(node: Node) -> bool {
    if !is_identifier(node) {
        return false;
    }
    if !is_break_or_continue_statement(node.parent()) {
        return false;
    }
    node == node.parent().label()
}

// Go: ast/utilities.go:2336 IsBreakOrContinueStatement
pub fn is_break_or_continue_statement(node: Node) -> bool {
    node_kind_is(
        node,
        &[SyntaxKind::BreakStatement, SyntaxKind::ContinueStatement],
    )
}

// GetModuleInstanceState is used during binding as well as in transformations and tests, and therefore may be invoked
// with a node that does not yet have its `Parent` pointer set. In this case, an `ancestors` represents a stack of
// virtual `Parent` pointers that can be used to walk up the tree. Since `getModuleInstanceStateForAliasTarget` may
// potentially walk up out of the provided `Node`, merely setting the parent pointers for a given `ModuleDeclaration`
// prior to invoking `GetModuleInstanceState` is not sufficient. It is, however, necessary that the `Parent` pointers
// for all ancestors of the `Node` provided to `GetModuleInstanceState` have been set.

// Push a virtual parent pointer onto `ancestors` and return it.
// PORT: Go `append` on a slice value; we return a new Vec so callers never share a backing array.
// Go: ast/utilities.go:2348 pushAncestor
pub fn push_ancestor(ancestors: &[Node], parent: Node) -> Vec<Node> {
    let mut result = ancestors.to_vec();
    result.push(parent);
    result
}

// If a virtual `Parent` exists on the stack, returns the previous stack entry and the virtual `Parent`.
// Otherwise, we return `nil` and the value of `node.Parent`.
// Go: ast/utilities.go:2354 popAncestor
pub fn pop_ancestor(ancestors: &[Node], node: Node) -> (&[Node], Node) {
    if ancestors.is_empty() {
        return (&[], node.parent());
    }
    let n = ancestors.len() - 1;
    (&ancestors[..n], ancestors[n])
}

// Go type ModuleInstanceState (ast/utilities.go:2294) lives in crate::flags.

// Go: ast/utilities.go:2371 GetModuleInstanceState
pub fn get_module_instance_state(node: Node) -> ModuleInstanceState {
    // PORT: Go passes a nil `visited` map and getModuleInstanceStateCached allocates it on first use.
    // Every later call shares that map, so allocating it here is equivalent.
    let mut visited: FxHashMap<Node, ModuleInstanceState> = FxHashMap::default();
    get_module_instance_state_unexported(node, &[], &mut visited)
}

// PORT: Go `getModuleInstanceState` has the same snake name as the exported
// `GetModuleInstanceState`. The exported one keeps the plain name because
// other packages call it; this private one gets the `_unexported` suffix.
// `visited` is keyed by the Node handle instead of Go `NodeId` (same identity).
// Go: ast/utilities.go:2375 getModuleInstanceState
fn get_module_instance_state_unexported(
    node: Node,
    ancestors: &[Node],
    visited: &mut FxHashMap<Node, ModuleInstanceState>,
) -> ModuleInstanceState {
    let body = node.body();
    if body.is_some() {
        get_module_instance_state_cached(body, &push_ancestor(ancestors, node), visited)
    } else {
        ModuleInstanceState::INSTANTIATED
    }
}

// Go: ast/utilities.go:2384 getModuleInstanceStateCached
fn get_module_instance_state_cached(
    node: Node,
    ancestors: &[Node],
    visited: &mut FxHashMap<Node, ModuleInstanceState>,
) -> ModuleInstanceState {
    if let Some(&cached) = visited.get(&node) {
        if cached != ModuleInstanceState::UNKNOWN {
            return cached;
        }
        return ModuleInstanceState::NON_INSTANTIATED;
    }
    visited.insert(node, ModuleInstanceState::UNKNOWN);
    let result = get_module_instance_state_worker(node, ancestors, visited);
    visited.insert(node, result);
    result
}

// Go: ast/utilities.go:2401 getModuleInstanceStateWorker
fn get_module_instance_state_worker(
    node: Node,
    ancestors: &[Node],
    visited: &mut FxHashMap<Node, ModuleInstanceState>,
) -> ModuleInstanceState {
    // A module is uninstantiated if it contains only
    match node.kind() {
        SyntaxKind::InterfaceDeclaration
        | SyntaxKind::TypeAliasDeclaration
        | SyntaxKind::JsTypeAliasDeclaration => {
            return ModuleInstanceState::NON_INSTANTIATED;
        }
        SyntaxKind::EnumDeclaration => {
            if is_enum_const(node) {
                return ModuleInstanceState::CONST_ENUM_ONLY;
            }
        }
        SyntaxKind::ImportDeclaration
        | SyntaxKind::JsImportDeclaration
        | SyntaxKind::ImportEqualsDeclaration => {
            if !has_syntactic_modifier(node, ModifierFlags::EXPORT) {
                return ModuleInstanceState::NON_INSTANTIATED;
            }
        }
        SyntaxKind::ExportDeclaration => {
            let export_clause = node.export_clause();
            if node.module_specifier().is_nil()
                && export_clause.is_some()
                && export_clause.kind() == SyntaxKind::NamedExports
            {
                let mut state = ModuleInstanceState::NON_INSTANTIATED;
                let ancestors = push_ancestor(ancestors, node);
                let ancestors = push_ancestor(&ancestors, export_clause);
                for specifier in export_clause.elements().to_vec() {
                    let specifier_state =
                        get_module_instance_state_for_alias_target(specifier, &ancestors, visited);
                    if specifier_state > state {
                        state = specifier_state;
                    }
                    if state == ModuleInstanceState::INSTANTIATED {
                        return state;
                    }
                }
                return state;
            }
        }
        SyntaxKind::ModuleBlock => {
            let mut state = ModuleInstanceState::NON_INSTANTIATED;
            let ancestors = push_ancestor(ancestors, node);
            node.for_each_child(&mut |n: Node| -> bool {
                let child_state = get_module_instance_state_cached(n, &ancestors, visited);
                match child_state {
                    ModuleInstanceState::NON_INSTANTIATED => {
                        return false;
                    }
                    ModuleInstanceState::CONST_ENUM_ONLY => {
                        state = ModuleInstanceState::CONST_ENUM_ONLY;
                        return false;
                    }
                    ModuleInstanceState::INSTANTIATED => {
                        state = ModuleInstanceState::INSTANTIATED;
                        return true;
                    }
                    _ => {}
                }
                panic!("Unhandled case in getModuleInstanceStateWorker")
            });
            return state;
        }
        SyntaxKind::ModuleDeclaration => {
            return get_module_instance_state_unexported(node, ancestors, visited);
        }
        _ => {}
    }
    ModuleInstanceState::INSTANTIATED
}

// Go: ast/utilities.go:2455 getModuleInstanceStateForAliasTarget
fn get_module_instance_state_for_alias_target(
    node: Node,
    ancestors: &[Node],
    visited: &mut FxHashMap<Node, ModuleInstanceState>,
) -> ModuleInstanceState {
    let name = node.property_name_or_name();
    if name.kind() != SyntaxKind::Identifier {
        // Skip for invalid syntax like this: export { "x" }
        return ModuleInstanceState::INSTANTIATED;
    }
    let (mut ancestors, mut p) = pop_ancestor(ancestors, node);
    while p.is_some() {
        if is_block(p) || is_module_block(p) || is_source_file(p) {
            let mut found = ModuleInstanceState::UNKNOWN;
            let statements_ancestors = push_ancestor(ancestors, p);
            for statement in p.statements().to_vec() {
                if node_has_name(statement, name) {
                    let state =
                        get_module_instance_state_cached(statement, &statements_ancestors, visited);
                    if found == ModuleInstanceState::UNKNOWN || state > found {
                        found = state;
                    }
                    if found == ModuleInstanceState::INSTANTIATED {
                        return found;
                    }
                    if statement.kind() == SyntaxKind::ImportEqualsDeclaration {
                        // Treat re-exports of import aliases as instantiated since they're ambiguous. This is consistent
                        // with `export import x = mod.x` being treated as instantiated:
                        //   import x = mod.x;
                        //   export { x };
                        found = ModuleInstanceState::INSTANTIATED;
                    }
                }
            }
            if found != ModuleInstanceState::UNKNOWN {
                return found;
            }
        }
        let (next_ancestors, next_p) = pop_ancestor(ancestors, p);
        ancestors = next_ancestors;
        p = next_p;
    }
    // Couldn't locate, assume could refer to a value
    ModuleInstanceState::INSTANTIATED
}

// Go: ast/utilities.go:2492 IsInstantiatedModule
pub fn is_instantiated_module(node: Node, preserve_const_enums: bool) -> bool {
    let module_state = get_module_instance_state(node);
    module_state == ModuleInstanceState::INSTANTIATED
        || (preserve_const_enums && module_state == ModuleInstanceState::CONST_ENUM_ONLY)
}

// Go: ast/utilities.go:2498 NodeHasName
pub fn node_has_name(statement: Node, id: Node) -> bool {
    let name = statement.name();
    if name.is_some() {
        return is_identifier(name) && name.text() == id.text();
    }
    if is_variable_statement(statement) {
        let declarations = statement.declaration_list().declarations().nodes().to_vec();
        return declarations.iter().any(|&d| node_has_name(d, id));
    }
    false
}

// Go: ast/utilities.go:2510 IsInternalModuleImportEqualsDeclaration
pub fn is_internal_module_import_equals_declaration(node: Node) -> bool {
    is_import_equals_declaration(node)
        && node.module_reference().kind() != SyntaxKind::ExternalModuleReference
}

// Go: ast/utilities.go:2514 IsConstAssertion
pub fn is_const_assertion(node: Node) -> bool {
    match node.kind() {
        SyntaxKind::AsExpression | SyntaxKind::TypeAssertionExpression => {
            return is_const_type_reference(node.type_());
        }
        _ => {}
    }
    false
}

/// `const`, interned once for `is_const_type_reference`.
static CONST_NAME: std::sync::LazyLock<Name> = std::sync::LazyLock::new(|| Name::from("const"));

// Go: ast/utilities.go:2522 IsConstTypeReference
// PERF: U1 (a). Compares name ids (`Node::text_is`), with no text load.
pub fn is_const_type_reference(node: Node) -> bool {
    is_type_reference_node(node)
        && node.type_arguments().len() == 0
        && is_identifier(node.type_name())
        && node.type_name().text_is(&CONST_NAME)
}

// Go: ast/utilities.go:2526 IsGlobalSourceFile
pub fn is_global_source_file(node: Node) -> bool {
    node.kind() == SyntaxKind::SourceFile && !is_external_or_common_js_module(node)
}

// Go: ast/utilities.go:2530 IsParameterLike
pub fn is_parameter_like(node: Node) -> bool {
    match node.kind() {
        SyntaxKind::Parameter | SyntaxKind::TypeParameter => {
            return true;
        }
        _ => {}
    }
    false
}

// Go: ast/utilities.go:2538 GetDeclarationOfKind
pub fn get_declaration_of_kind(symbols: &SymbolArena, symbol: SymbolId, kind: SyntaxKind) -> Node {
    for &declaration in &symbols.sym(symbol).declarations {
        if declaration.kind() == kind {
            return declaration;
        }
    }
    Node::NIL
}

// Go: ast/utilities.go:2547 FindConstructorDeclaration
pub fn find_constructor_declaration(node: Node) -> Node {
    for member in node.members().to_vec() {
        if is_constructor_declaration(member) && node_is_present(member.body()) {
            return member;
        }
    }
    Node::NIL
}

// Go: ast/utilities.go:2556 GetFirstIdentifier
pub fn get_first_identifier(node: Node) -> Node {
    match node.kind() {
        SyntaxKind::Identifier => {
            return node;
        }
        SyntaxKind::QualifiedName => {
            return get_first_identifier(node.left());
        }
        SyntaxKind::PropertyAccessExpression => {
            return get_first_identifier(node.expression());
        }
        _ => {}
    }
    panic!("Unhandled case in GetFirstIdentifier")
}

// Go: ast/utilities.go:2568 GetNamespaceDeclarationNode
pub fn get_namespace_declaration_node(node: Node) -> Node {
    match node.kind() {
        SyntaxKind::ImportDeclaration | SyntaxKind::JsImportDeclaration => {
            let import_clause = node.import_clause();
            if import_clause.is_some()
                && import_clause.named_bindings().is_some()
                && is_namespace_import(import_clause.named_bindings())
            {
                return import_clause.named_bindings();
            }
        }
        SyntaxKind::ImportEqualsDeclaration => {
            return node;
        }
        SyntaxKind::ExportDeclaration => {
            let export_clause = node.export_clause();
            if export_clause.is_some() && is_namespace_export(export_clause) {
                return export_clause;
            }
        }
        _ => {
            panic!("Unhandled case in getNamespaceDeclarationNode");
        }
    }
    Node::NIL
}

// Go: ast/utilities.go:2588 ModuleExportNameIsDefault
pub fn module_export_name_is_default(node: Node) -> bool {
    // PORT: Go `InternalSymbolNameDefault` is the string "default".
    node.text() == "default"
}

// Go: ast/utilities.go:2592 IsDefaultImport
pub fn is_default_import(
    node: Node, /*ImportDeclaration | ImportEqualsDeclaration | ExportDeclaration*/
) -> bool {
    match node.kind() {
        SyntaxKind::ImportDeclaration | SyntaxKind::JsImportDeclaration => {
            let import_clause = node.import_clause();
            // PORT: Go reads the unexported ImportClause `name` field; `name()` returns it.
            return import_clause.is_some() && import_clause.name().is_some();
        }
        _ => {}
    }
    false
}

// PORT: Go `tspath.FileExtensionIsOneOf`; tspath has no port in this crate, so
// this private copy follows `tspath.FileExtensionIs` exactly.
fn file_extension_is_one_of_p3(path: &str, extensions: &[&str]) -> bool {
    extensions
        .iter()
        .any(|ext| path.len() > ext.len() && path.ends_with(ext))
}

// Go: ast/utilities.go:2601 GetImpliedNodeFormatForFile
pub fn get_implied_node_format_for_file(path: &str, package_json_type: &str) -> ModuleKind {
    // Go core.ResolutionModeNone / ESM / CommonJS are ModuleKind None / ESNext / CommonJS.
    let mut implied_node_format = ModuleKind::NONE;
    if file_extension_is_one_of_p3(path, &[".d.mts", ".mts", ".mjs"]) {
        implied_node_format = ModuleKind::ES_NEXT;
    } else if file_extension_is_one_of_p3(path, &[".d.cts", ".cts", ".cjs"]) {
        implied_node_format = ModuleKind::COMMON_JS;
    } else if file_extension_is_one_of_p3(path, &[".d.ts", ".ts", ".tsx", ".js", ".jsx"]) {
        implied_node_format = if package_json_type == "module" {
            ModuleKind::ES_NEXT
        } else {
            ModuleKind::COMMON_JS
        };
    }

    implied_node_format
}

// Go: ast/utilities.go:2614 GetEmitModuleFormatOfFileWorker
pub fn get_emit_module_format_of_file_worker(
    file_name: &str,
    options: &CompilerOptions,
    source_file_meta_data: &SourceFileMetaData,
) -> ModuleKind {
    let result = get_implied_node_format_for_emit_worker(
        file_name,
        options.get_emit_module_kind(),
        source_file_meta_data,
    );
    if result != ModuleKind::NONE {
        return result;
    }
    options.get_emit_module_kind()
}

// Go: ast/utilities.go:2622 GetImpliedNodeFormatForEmitWorker
pub fn get_implied_node_format_for_emit_worker(
    file_name: &str,
    emit_module_kind: ModuleKind,
    source_file_meta_data: &SourceFileMetaData,
) -> ModuleKind {
    if ModuleKind::NODE16 <= emit_module_kind && emit_module_kind <= ModuleKind::NODE_NEXT {
        return source_file_meta_data.implied_node_format;
    }
    if source_file_meta_data.implied_node_format == ModuleKind::COMMON_JS
        && (source_file_meta_data.package_json_type == "commonjs"
            || file_extension_is_one_of_p3(file_name, &[".cjs", ".cts"]))
    {
        return ModuleKind::COMMON_JS;
    }
    if source_file_meta_data.implied_node_format == ModuleKind::ES_NEXT
        && (source_file_meta_data.package_json_type == "module"
            || file_extension_is_one_of_p3(file_name, &[".mjs", ".mts"]))
    {
        return ModuleKind::ES_NEXT;
    }
    ModuleKind::NONE
}

// Go: ast/utilities.go:2639 GetDeclarationContainer
pub fn get_declaration_container(node: Node) -> Node {
    find_ancestor(
        get_root_declaration(node),
        &mut |node: Node| match node.kind() {
            SyntaxKind::VariableDeclaration
            | SyntaxKind::VariableDeclarationList
            | SyntaxKind::ImportSpecifier
            | SyntaxKind::NamedImports
            | SyntaxKind::NamespaceImport
            | SyntaxKind::ImportClause => false,
            _ => true,
        },
    )
    .parent()
}

// Indicates that a symbol is an alias that does not merge with a local declaration.
// OR Is a JSContainer which may merge an alias with a local declaration
// Go: ast/utilities.go:2657 IsNonLocalAlias
pub fn is_non_local_alias(symbols: &SymbolArena, symbol: SymbolId, excludes: SymbolFlags) -> bool {
    if symbol.is_nil() {
        return false;
    }
    let flags = symbols.sym(symbol).flags;
    flags & (SymbolFlags::ALIAS | excludes) == SymbolFlags::ALIAS
        || flags.intersects(SymbolFlags::ALIAS) && flags.intersects(SymbolFlags::ASSIGNMENT)
}

// An alias symbol is created by one of the following declarations:
//
//	import <symbol> = ...
//	const <symbol> = ... (JS only)
//	const { <symbol>, ... } = ... (JS only)
//	import <symbol> from ...
//	import * as <symbol> from ...
//	import { x as <symbol> } from ...
//	export { x as <symbol> } from ...
//	export * as ns <symbol> from ...
//	export = <EntityNameExpression>
//	export default <EntityNameExpression>
//	module.exports = <EntityNameExpression> (JS only)
//	module.exports.<symbol> = <EntityNameExpression> (JS only)
//	exports.<symbol> = <EntityNameExpression> (JS only)
// Go: ast/utilities.go:2680 IsAliasSymbolDeclaration
pub fn is_alias_symbol_declaration(node: Node) -> bool {
    match node.kind() {
        SyntaxKind::ImportEqualsDeclaration
        | SyntaxKind::NamespaceExportDeclaration
        | SyntaxKind::NamespaceImport
        | SyntaxKind::NamespaceExport
        | SyntaxKind::ImportSpecifier
        | SyntaxKind::ExportSpecifier => {
            return true;
        }
        SyntaxKind::ImportClause => {
            return node.name().is_some();
        }
        SyntaxKind::ExportAssignment => {
            return expression_is_alias(node.expression());
        }
        SyntaxKind::VariableDeclaration | SyntaxKind::BindingElement => {
            return is_variable_declaration_initialized_to_require(node);
        }
        SyntaxKind::BinaryExpression => match get_assignment_declaration_kind(node) {
            JSDeclarationKind::MODULE_EXPORTS | JSDeclarationKind::EXPORTS_PROPERTY => {
                return expression_is_alias(node.right());
            }
            _ => {}
        },
        _ => {}
    }
    false
}

// Go: ast/utilities.go:2700 IsParseTreeNode
pub fn is_parse_tree_node(node: Node) -> bool {
    !node.flags().intersects(NodeFlags::SYNTHESIZED)
}

// Returns a token if position is in [start-of-leading-trivia, end), includes JSDoc only if requested
// Go: ast/utilities.go:2705 GetNodeAtPosition
pub fn get_node_at_position(file: Node, position: i32, include_js_doc: bool) -> Node {
    let mut current = file;
    loop {
        let mut child = Node::NIL;
        if include_js_doc {
            for jsdoc in current.js_doc(file).to_vec() {
                if node_contains_position(jsdoc, position) {
                    child = jsdoc;
                    break;
                }
            }
        }
        if child.is_nil() {
            current.for_each_child(&mut |node: Node| -> bool {
                if node_contains_position(node, position) {
                    child = node;
                    return true;
                }
                false
            });
        }
        if child.is_nil() || is_meta_property(child) {
            return current;
        }
        current = child;
    }
}

// Go: ast/utilities.go:2733 nodeContainsPosition
pub fn node_contains_position(node: Node, position: i32) -> bool {
    (node.kind() as u16) >= (SyntaxKind::FIRST_NODE as u16)
        && node.pos() <= position
        && (position < node.end() || position == node.end() && node.kind() == SyntaxKind::EndOfFile)
}

/// `get_node_at_position` for a series of positions in one tree that does
/// not change between the calls. It gives the node that
/// `get_node_at_position` gives for each position. It is fast when the
/// positions never go down; a smaller position descends from the root.
// PERF: loadcrit1 step 0. `for_each_dynamic_import_or_require_call` looks up
// each `import`/`require` word. Go descends from the root for each word and
// checks the children of each level in order (ast/utilities.go:2705). A
// `.d.ts` file with a wide type literal (@redis/client
// `commands/index.d.ts`: 743 members, 10,783 words) makes that
// O(words x members). In the port each check also reads a store record that
// the parse has not published yet (`loc_slow`, `kind_slow`), so this one
// parse held the umami and directus loads for about 80 ms.
//
// The cursor also descends from the root for each position, and gives the
// same first child at each level as Go's scan. It keeps the levels of the
// last descent: each node with its children in `for_each_child` order and
// `skip`, the count of leading children that contain no position from the
// last one on. A later position only grows, so the scan of a level starts
// at `skip`, and the children it passes cannot contain the position. This
// holds for any tree shape (overlapping or unsorted children, JSDoc), so
// the JS files use the cursor too.
#[derive(Default)]
struct NodeAtPositionCursor {
    /// The levels of the last descent, root first. A level past the
    /// current depth is the state of a node that the descent left; it is
    /// used again only for the same node.
    levels: Vec<CursorLevel>,
    last_position: i32,
}

struct CursorLevel {
    node: Node,
    children: Vec<Node>,
    /// No child before this index contains a position from
    /// `NodeAtPositionCursor::last_position` on.
    skip: usize,
}

impl CursorLevel {
    fn new(node: Node) -> CursorLevel {
        let mut level = CursorLevel {
            node,
            children: Vec::new(),
            skip: 0,
        };
        level.collect_children();
        level
    }

    fn reset(&mut self, node: Node) {
        self.node = node;
        self.children.clear();
        self.skip = 0;
        self.collect_children();
    }

    fn collect_children(&mut self) {
        let children = &mut self.children;
        self.node.for_each_child(|child| {
            children.push(child);
            false
        });
    }

    /// The first child that contains `position` (Go's `ForEachChild` scan
    /// in `GetNodeAtPosition`), or nil. Moves `skip` over the leading
    /// children that contain no position from `position` on.
    fn first_child_containing(&mut self, position: i32) -> Node {
        let mut passed_only_dead = true;
        for index in self.skip..self.children.len() {
            let child = self.children[index];
            let kind = child.kind();
            let dead = if (kind as u16) < (SyntaxKind::FIRST_NODE as u16) {
                true
            } else {
                let loc = child.loc();
                // Go: ast/utilities.go:2733 nodeContainsPosition
                if loc.pos() <= position
                    && (position < loc.end()
                        || position == loc.end() && kind == SyntaxKind::EndOfFile)
                {
                    return child;
                }
                // No later position is in it: the range left from
                // `max(pos, position)` on is empty.
                let from = loc.pos().max(position);
                loc.end() < from || loc.end() == from && kind != SyntaxKind::EndOfFile
            };
            passed_only_dead &= dead;
            if passed_only_dead {
                self.skip = index + 1;
            }
        }
        Node::NIL
    }
}

impl NodeAtPositionCursor {
    // Go: ast/utilities.go:2705 GetNodeAtPosition
    fn node_at(&mut self, file: Node, position: i32, include_js_doc: bool) -> Node {
        if position < self.last_position {
            // The skips may pass the node of a smaller position. Go has no
            // cursor and descends from the root for each position, so do
            // that. The levels stay valid for `last_position` on.
            return get_node_at_position(file, position, include_js_doc);
        }
        self.last_position = position;
        if self.levels.is_empty() {
            self.levels.push(CursorLevel::new(file));
        }
        let mut depth = 0;
        loop {
            let current = self.levels[depth].node;
            let mut child = Node::NIL;
            if include_js_doc {
                for jsdoc in current.js_doc(file).to_vec() {
                    if node_contains_position(jsdoc, position) {
                        child = jsdoc;
                        break;
                    }
                }
            }
            if child.is_nil() {
                child = self.levels[depth].first_child_containing(position);
            }
            if child.is_nil() || is_meta_property(child) {
                return current;
            }
            depth += 1;
            match self.levels.get_mut(depth) {
                None => self.levels.push(CursorLevel::new(child)),
                Some(level) if level.node != child => level.reset(child),
                Some(_) => {}
            }
        }
    }
}

// Go: ast/utilities.go:2737 findImportOrRequire
pub fn find_import_or_require(text: &str, start: i32) -> (i32, i32) {
    let bytes = text.as_bytes();
    let mut index = start.max(0) as usize;
    let n = bytes.len();
    let mut size: usize;
    while index < n {
        // Go strings.IndexAny(text[index:], "ir"); both are ASCII so a byte scan matches.
        let Some(next) = memchr::memchr2(b'i', b'r', &bytes[index..]) else {
            break;
        };
        index += next;

        let expected: &[u8];
        if bytes[index] == b'i' {
            size = 6;
            expected = b"import";
        } else {
            size = 7;
            expected = b"require";
        }
        if index + size <= n && &bytes[index..index + size] == expected {
            return (index as i32, size as i32);
        }
        index += 1;
    }

    (-1, 0)
}

// Go: ast/utilities.go:2764 ForEachDynamicImportOrRequireCall
pub fn for_each_dynamic_import_or_require_call(
    file: Node,
    include_type_space_imports: bool,
    require_string_literal_like_argument: bool,
    cb: &mut dyn FnMut(Node, Node) -> bool,
) -> bool {
    let is_java_script_file = is_in_js_file(file);
    let text = source_file_text(file);
    let include_js_doc = is_java_script_file && include_type_space_imports;
    // PERF: loadcrit1 step 0. Go calls `GetNodeAtPosition` from the root for
    // each word. The cursor gives the same node for each word (see
    // `NodeAtPositionCursor`).
    let mut cursor = NodeAtPositionCursor::default();
    let (mut last_index, mut size) = find_import_or_require(&text, 0);
    while last_index >= 0 {
        let node = cursor.node_at(file, last_index, include_js_doc);
        debug_assert_eq!(
            node,
            get_node_at_position(file, last_index, include_js_doc),
            "cursor node at {last_index}"
        );
        if is_java_script_file && is_require_call(node, require_string_literal_like_argument) {
            if cb(node, node.arguments().get(0)) {
                return true;
            }
        } else if is_import_call(node)
            && node.arguments().len() > 0
            && (!require_string_literal_like_argument
                || is_string_literal_like(node.arguments().get(0)))
        {
            if cb(node, node.arguments().get(0)) {
                return true;
            }
        } else if include_type_space_imports && is_literal_import_type_node(node) {
            if cb(node, node.argument().literal()) {
                return true;
            }
        }
        // skip past import/require
        last_index += size;
        (last_index, size) = find_import_or_require(&text, last_index);
    }
    false
}

#[cfg(test)]
mod node_at_position_cursor_tests {
    use super::*;
    use crate::frontend::parser::{SourceFileParseOptions, parse_source_file};
    use crate::frontend::tspath::Path;

    fn parse(name: &str, text: &'static str, kind: ScriptKind) -> Node {
        parse_source_file(
            &SourceFileParseOptions {
                file_name: name.to_string(),
                path: Path(name.to_string()),
                ..Default::default()
            },
            text,
            kind,
        )
        .root
    }

    /// The cursor gives the node of `get_node_at_position` at every
    /// position in order, and at every `import`/`require` word (the
    /// positions of `for_each_dynamic_import_or_require_call`, also in
    /// comments, strings and names), with and without JSDoc.
    fn check(file: Node) {
        let text = source_file_text(file);
        let mut words = Vec::new();
        let (mut index, mut size) = find_import_or_require(&text, 0);
        while index >= 0 {
            words.push(index);
            (index, size) = find_import_or_require(&text, index + size);
        }
        assert!(!words.is_empty());
        let every = (0..=text.len() as i32).collect::<Vec<_>>();
        for include_js_doc in [false, true] {
            for positions in [&every, &words] {
                let mut cursor = NodeAtPositionCursor::default();
                for &position in positions {
                    assert_eq!(
                        cursor.node_at(file, position, include_js_doc),
                        get_node_at_position(file, position, include_js_doc),
                        "position {position}, JSDoc {include_js_doc}"
                    );
                }
            }
        }
    }

    #[test]
    fn the_cursor_finds_the_node_of_a_root_descent() {
        // A wide type literal, as @redis/client `commands/index.d.ts`.
        let mut wide = String::from("// import require\ndeclare const _default: {\n");
        for i in 0..300 {
            wide += &format!(
                "    /** import {i} */ m{i}: typeof import(\"./m{i}\"); importantRequire{i}: import(\"./t{i}\").T<\"require\">;\n"
            );
        }
        wide += "    // import before the brace\n};\nexport default _default; // require\n";
        check(parse("/wide.d.ts", wide.leak(), ScriptKind::TS));

        check(parse(
            "/a.ts",
            "import x from \"x\";\nexport * from \"./require\";\n\
             const m = import.meta; const n = import.meta.url;\n\
             async function f() { await import(\"a\"); return import(`b`).then(() => import(c)); }\n\
             type T = typeof import(\"t\") extends import(\"u\").U ? 1 : 2;\n\
             class C { @dec(import(\"d\")) static [require(\"k\")]: import(\"v\").V; }\n\
             declare module \"require\" { export type R = import(\"r\").R; }\n\
             // import(\"in a comment\") require(\"x\")\n\
             const s = \"import('a string') require\"; /* import */",
            ScriptKind::TS,
        ));

        check(parse(
            "/a.js",
            "const a = require(\"a\");\n\
             /** @type {import(\"b\").B} */\n\
             const b = require(`b`).b;\n\
             /**\n * @typedef {import('c').C} C\n * @param {import(\"d\").D} d\n * @returns {Promise<import(\"e\")>}\n */\n\
             function f(d) { return import(\"e\"); }\n\
             module.exports = { requireIt: () => require(\"f\") };\n\
             /** @import { G } from \"g\" */\n",
            ScriptKind::JS,
        ));

        // Error recovery: missing and zero-width nodes, unclosed lists.
        check(parse(
            "/broken.ts",
            "const a = import(; type T = import(\nclass { import( }\n\
             function require(x, { import\nlet [b, require(\"c\")] = import(\"d\"\n\
             export default async function* ( require ) => import\n\
             <div>{import(\"e\")} require</div>\n\
             type U = { [K in import(\"f\")]: require",
            ScriptKind::TS,
        ));

        check(parse(
            "/a.tsx",
            "const x = <A b={import(\"b\")}>import require {require(\"c\")}</A>;\n\
             export const y = () => <></>; import(\"z\");",
            ScriptKind::TSX,
        ));
    }

    /// After the cursor has passed a position, a smaller one still gets the
    /// node of a root descent (loadcrit1 follow-up: Go has no cursor). The
    /// positions go up, down and up again, so a fallback that moves
    /// `last_position` down fails (the followups33 skeptic's mutant CURS).
    #[test]
    fn a_smaller_position_gets_the_node_of_a_root_descent() {
        let mut wide = String::from("declare const _default: {\n");
        for i in 0..20 {
            wide += &format!("    m{i}: typeof import(\"./m{i}\");\n");
        }
        wide += "};\nexport default _default;\n";
        let file = parse("/wide.d.ts", wide.leak(), ScriptKind::TS);
        let end = source_file_text(file).len() as i32;
        for include_js_doc in [false, true] {
            let mut cursor = NodeAtPositionCursor::default();
            for position in (0..=end).chain((0..=end).rev()).chain(0..=end) {
                assert_eq!(
                    cursor.node_at(file, position, include_js_doc),
                    get_node_at_position(file, position, include_js_doc),
                    "position {position}, JSDoc {include_js_doc}"
                );
            }
        }
    }
}
