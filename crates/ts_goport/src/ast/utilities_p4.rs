//! Port of typescript-go `internal/ast/utilities.go` lines 2729-3631.

use crate::prelude::*;
use std::borrow::Cow;

// Go: ast/utilities.go:2797 IsRequireCall
pub fn is_require_call(node: Node, require_string_literal_like_argument: bool) -> bool {
    if !is_call_expression(node) {
        return false;
    }
    let call = node;
    if !is_identifier(call.expression()) || call.expression().text() != "require" {
        return false;
    }
    if call.arguments().len() != 1 {
        return false;
    }
    !require_string_literal_like_argument || is_string_literal_like(call.arguments().get(0))
}

// Go: ast/utilities.go:2811 IsRequireVariableStatement
pub fn is_require_variable_statement(node: Node) -> bool {
    if is_variable_statement(node) {
        let declarations = node.declaration_list().declarations().nodes();
        if declarations.len() > 0 {
            return declarations
                .iter()
                .all(|d| is_variable_declaration_initialized_to_require(d));
        }
    }
    false
}

// Go: ast/utilities.go:2820 GetJSXImplicitImportBase
pub fn get_jsx_implicit_import_base(compiler_options: &CompilerOptions, file: Node) -> String {
    let jsx_import_source_pragma = get_pragma_from_source_file(file, "jsximportsource");
    let jsx_runtime_pragma = get_pragma_from_source_file(file, "jsxruntime");
    if get_pragma_argument(jsx_runtime_pragma.as_deref(), "factory") == "classic" {
        return String::new();
    }
    if compiler_options.jsx == JsxEmit::REACT_JSX
        || compiler_options.jsx == JsxEmit::REACT_JSX_DEV
        || !compiler_options.jsx_import_source.is_empty()
        || jsx_import_source_pragma.is_some()
        || get_pragma_argument(jsx_runtime_pragma.as_deref(), "factory") == "automatic"
    {
        let mut result = get_pragma_argument(jsx_import_source_pragma.as_deref(), "factory");
        if result.is_empty() {
            result = compiler_options.jsx_import_source.clone();
        }
        if result.is_empty() {
            result = "react".to_string();
        }
        return result;
    }
    String::new()
}

// Go: ast/utilities.go:2843 GetJSXRuntimeImport
pub fn get_jsx_runtime_import(base: &str, options: &CompilerOptions) -> String {
    if base.is_empty() {
        return base.to_string();
    }
    format!(
        "{}/{}",
        base,
        if options.jsx == JsxEmit::REACT_JSX_DEV {
            "jsx-dev-runtime"
        } else {
            "jsx-runtime"
        }
    )
}

// Go: ast/utilities.go:2850 GetPragmaFromSourceFile
// PORT: Go `*Pragma` (nil when absent) -> `Option<FileRef<Pragma>>`, a guard
// on an entry of `source_file_info(file).pragmas`. Pass it to
// `get_pragma_argument` with `as_deref()`.
pub fn get_pragma_from_source_file(file: Node, name: &str) -> Option<FileRef<Pragma>> {
    if file.is_nil() {
        return None;
    }
    let info = source_file_info(file);
    // Last one wins.
    let index = info
        .pragmas
        .iter()
        .rposition(|pragma| pragma.name == name)?;
    Some(match info {
        FileRef::Static(info) => FileRef::Static(&info.pragmas[index]),
        FileRef::Pinned { version, .. } => FileRef::Pinned {
            version,
            key: index,
            get: |version, index| &version.go_file().info.pragmas[index],
        },
    })
}

// Go: ast/utilities.go:2862 GetPragmaArgument
pub fn get_pragma_argument(pragma: Option<&Pragma>, name: &str) -> String {
    if let Some(pragma) = pragma {
        if let Some(arg) = pragma.args.get(name) {
            return arg.value.clone();
        }
    }
    String::new()
}

// Go: ast/utilities.go:2874 IsVariableDeclarationInitializedToRequire
// Of the form: `const x = require("x")` or `const { x } = require("x")` or with `var` or `let`
// The variable must not be exported and must not have a type annotation, even a jsdoc one.
// The initializer must be a call to `require` with a string literal or a string literal-like argument.
pub fn is_variable_declaration_initialized_to_require(node: Node) -> bool {
    let mut node = node;
    if node.kind() == SyntaxKind::BindingElement {
        node = node.parent().parent();
    }
    is_variable_declaration_initialized_with_require_helper(
        node, false, /*allowAccessedRequire*/
    )
}

// Go: ast/utilities.go:2881 IsVariableDeclarationInitializedToBareOrAccessedRequire
pub fn is_variable_declaration_initialized_to_bare_or_accessed_require(node: Node) -> bool {
    is_variable_declaration_initialized_with_require_helper(
        node, true, /*allowAccessedRequire*/
    )
}

// Go: ast/utilities.go:2885 isVariableDeclarationInitializedWithRequireHelper
pub fn is_variable_declaration_initialized_with_require_helper(
    node: Node,
    allow_accessed_require: bool,
) -> bool {
    if !is_in_js_file(node) {
        return false;
    }
    if node.kind() != SyntaxKind::VariableDeclaration {
        return false;
    }
    let mut initializer = node.initializer();
    if initializer.is_nil() {
        return false;
    }
    if allow_accessed_require {
        initializer = get_leftmost_access_expression(initializer);
    }

    !node
        .parent()
        .parent()
        .modifier_flags()
        .intersects(ModifierFlags::EXPORT)
        && node.type_().is_nil()
        && is_require_call(initializer, true /*requireStringLiteralLikeArgument*/)
}

// Go: ast/utilities.go:2905 GetModuleSpecifierOfBareOrAccessedRequire
pub fn get_module_specifier_of_bare_or_accessed_require(node: Node) -> Node {
    if is_variable_declaration_initialized_with_require_helper(
        node, false, /*allowAccessedRequire*/
    ) {
        return node.initializer().arguments().get(0);
    }
    if is_variable_declaration_initialized_with_require_helper(
        node, true, /*allowAccessedRequire*/
    ) {
        let leftmost = get_leftmost_access_expression(node.initializer());
        if is_require_call(leftmost, true /*requireStringLiteralLikeArgument*/) {
            return leftmost.arguments().get(0);
        }
    }
    Node::NIL
}

// Go: ast/utilities.go:2918 IsModuleExportsAccessExpression
pub fn is_module_exports_access_expression(node: Node) -> bool {
    if is_access_expression(node) && is_module_identifier(node.expression()) {
        let name = get_element_or_property_access_name(node);
        if name.is_some() {
            return name.text() == "exports";
        }
    }
    false
}

// Go: ast/utilities.go:2927 IsModuleExportsQualifiedName
pub fn is_module_exports_qualified_name(node: Node) -> bool {
    is_qualified_name(node) && is_module_identifier(node.left()) && node.right().text() == "exports"
}

// Go: ast/utilities.go:2931 IsCheckJSEnabledForFile
pub fn is_check_js_enabled_for_file(source_file: Node, compiler_options: &CompilerOptions) -> bool {
    if let Some(enabled) = with_source_file_info(source_file, |info| {
        info.check_js_directive
            .as_ref()
            .map(|directive| directive.enabled)
    }) {
        return enabled;
    }
    compiler_options.check_js == Tristate::True
}

// Go: ast/utilities.go:2938 IsPlainJSFile
pub fn is_plain_js_file(file: Node, check_js: Tristate) -> bool {
    file.is_some()
        && with_source_file_info(file, |info| {
            (info.script_kind == ScriptKind::JS || info.script_kind == ScriptKind::JSX)
                && info.check_js_directive.is_none()
        })
        && check_js == Tristate::Unknown
}

// Go: ast/utilities.go:2942 GetLeftmostAccessExpression
pub fn get_leftmost_access_expression(expr: Node) -> Node {
    let mut expr = expr;
    while is_access_expression(expr) {
        expr = expr.expression();
    }
    expr
}

// Go: ast/utilities.go:2949 IsTypeOnlyImportDeclaration
pub fn is_type_only_import_declaration(node: Node) -> bool {
    match node.kind() {
        SyntaxKind::ImportSpecifier => node.is_type_only() || node.parent().parent().is_type_only(),
        SyntaxKind::NamespaceImport => node.parent().is_type_only(),
        SyntaxKind::ImportClause | SyntaxKind::ImportEqualsDeclaration => node.is_type_only(),
        _ => false,
    }
}

// Go: ast/utilities.go:2961 isTypeOnlyExportDeclaration
pub fn is_type_only_export_declaration(node: Node) -> bool {
    match node.kind() {
        SyntaxKind::ExportSpecifier => node.is_type_only() || node.parent().parent().is_type_only(),
        SyntaxKind::ExportDeclaration => {
            let d = node;
            d.is_type_only() && d.module_specifier().is_some() && d.export_clause().is_nil()
        }
        SyntaxKind::NamespaceExport => node.parent().is_type_only(),
        _ => false,
    }
}

// Go: ast/utilities.go:2974 IsTypeOnlyImportOrExportDeclaration
pub fn is_type_only_import_or_export_declaration(node: Node) -> bool {
    is_type_only_import_declaration(node) || is_type_only_export_declaration(node)
}

// Go: ast/utilities.go:2978 IsExclusivelyTypeOnlyImportOrExport
pub fn is_exclusively_type_only_import_or_export(node: Node) -> bool {
    match node.kind() {
        SyntaxKind::ExportDeclaration => {
            return node.is_type_only();
        }
        SyntaxKind::ImportDeclaration | SyntaxKind::JsImportDeclaration => {
            let import_clause = node.import_clause();
            if import_clause.is_some() {
                return import_clause.is_type_only();
            }
        }
        SyntaxKind::JsDocImportTag => {
            let import_clause = node.import_clause();
            if import_clause.is_some() {
                return import_clause.is_type_only();
            }
        }
        _ => {}
    }
    false
}

// Go: ast/utilities.go:2994 GetClassLikeDeclarationOfSymbol
pub fn get_class_like_declaration_of_symbol(symbols: &SymbolArena, symbol: SymbolId) -> Node {
    symbols
        .sym(symbol)
        .declarations
        .iter()
        .copied()
        .find(|&d| is_class_like(d))
        .unwrap_or(Node::NIL)
}

// Go: ast/utilities.go:2998 IsCallLikeExpression
pub fn is_call_like_expression(node: Node) -> bool {
    match node.kind() {
        SyntaxKind::JsxOpeningElement
        | SyntaxKind::JsxSelfClosingElement
        | SyntaxKind::JsxOpeningFragment
        | SyntaxKind::CallExpression
        | SyntaxKind::NewExpression
        | SyntaxKind::TaggedTemplateExpression
        | SyntaxKind::Decorator => true,
        SyntaxKind::BinaryExpression => {
            node.operator_token().kind() == SyntaxKind::InstanceOfKeyword
        }
        _ => false,
    }
}

// Go: ast/utilities.go:3009 IsJsxCallLike
pub fn is_jsx_call_like(node: Node) -> bool {
    matches!(
        node.kind(),
        SyntaxKind::JsxOpeningElement
            | SyntaxKind::JsxSelfClosingElement
            | SyntaxKind::JsxOpeningFragment
    )
}

// Go: ast/utilities.go:3017 IsCallLikeOrFunctionLikeExpression
pub fn is_call_like_or_function_like_expression(node: Node) -> bool {
    is_call_like_expression(node) || is_function_expression_or_arrow_function(node)
}

// Go: ast/utilities.go:3021 NodeHasKind
pub fn node_has_kind(node: Node, kind: SyntaxKind) -> bool {
    if node.is_nil() {
        return false;
    }
    node.kind() == kind
}

// Go: ast/utilities.go:3028 IsContextualKeyword
pub fn is_contextual_keyword(token: SyntaxKind) -> bool {
    // PORT: compare discriminants; crate::astdata::SyntaxKind does not derive Ord.
    SyntaxKind::FIRST_CONTEXTUAL_KEYWORD as u16 <= token as u16
        && token as u16 <= SyntaxKind::LAST_CONTEXTUAL_KEYWORD as u16
}

// Go: ast/utilities.go:3032 IsThisInTypeQuery
pub fn is_this_in_type_query(node: Node) -> bool {
    if !is_this_identifier(node) {
        return false;
    }
    let mut node = node;
    while is_qualified_name(node.parent()) && node.parent().left() == node {
        node = node.parent();
    }
    node.parent().kind() == SyntaxKind::TypeQuery
}

// Go: ast/utilities.go:3043 IsLet
// Gets whether a bound `VariableDeclaration` or `VariableDeclarationList` is part of a `let` declaration.
pub fn is_let(node: Node) -> bool {
    (get_combined_node_flags(node) & NodeFlags::BLOCK_SCOPED) == NodeFlags::LET
}

// Go: ast/utilities.go:3047 IsClassMemberModifier
pub fn is_class_member_modifier(token: SyntaxKind) -> bool {
    is_parameter_property_modifier(token)
        || token == SyntaxKind::StaticKeyword
        || token == SyntaxKind::OverrideKeyword
        || token == SyntaxKind::AccessorKeyword
}

// Go: ast/utilities.go:3052 IsParameterPropertyModifier
pub fn is_parameter_property_modifier(kind: SyntaxKind) -> bool {
    modifier_to_flag(kind).intersects(ModifierFlags::PARAMETER_PROPERTY_MODIFIER)
}

// Go: ast/utilities.go:3056 ForEachChildAndJSDoc
// PORT: Go `Visitor` (`func(*Node) bool`) -> `&mut dyn FnMut(Node) -> bool`.
// `visitNodes` is inlined as a loop over `node.js_doc(file)`.
pub fn for_each_child_and_js_doc(
    node: Node,
    source_file: Node,
    v: &mut dyn FnMut(Node) -> bool,
) -> bool {
    for js_doc in node.js_doc(source_file).iter() {
        if v(js_doc) {
            return true;
        }
    }
    node.for_each_child(v)
}

// Go: ast/utilities.go:3063 HasTypeArguments
pub fn has_type_arguments(node: Node) -> bool {
    matches!(
        node.kind(),
        SyntaxKind::CallExpression
            | SyntaxKind::NewExpression
            | SyntaxKind::TaggedTemplateExpression
            | SyntaxKind::TypeReference
            | SyntaxKind::ExpressionWithTypeArguments
            | SyntaxKind::ImportType
            | SyntaxKind::TypeQuery
            | SyntaxKind::JsxOpeningElement
            | SyntaxKind::JsxSelfClosingElement
    )
}

// Go: ast/utilities.go:3073 IsTypeReferenceType
pub fn is_type_reference_type(node: Node) -> bool {
    node.kind() == SyntaxKind::TypeReference
        || node.kind() == SyntaxKind::ExpressionWithTypeArguments
}

// Go: ast/utilities.go:3077 IsVariableLike
pub fn is_variable_like(node: Node) -> bool {
    matches!(
        node.kind(),
        SyntaxKind::BindingElement
            | SyntaxKind::EnumMember
            | SyntaxKind::Parameter
            | SyntaxKind::PropertyAssignment
            | SyntaxKind::PropertyDeclaration
            | SyntaxKind::PropertySignature
            | SyntaxKind::ShorthandPropertyAssignment
            | SyntaxKind::VariableDeclaration
    )
}

// Go: ast/utilities.go:3086 HasInitializer
pub fn has_initializer(node: Node) -> bool {
    match node.kind() {
        SyntaxKind::VariableDeclaration
        | SyntaxKind::Parameter
        | SyntaxKind::BindingElement
        | SyntaxKind::PropertyDeclaration
        | SyntaxKind::PropertyAssignment
        | SyntaxKind::EnumMember
        | SyntaxKind::ForStatement
        | SyntaxKind::ForInStatement
        | SyntaxKind::ForOfStatement
        | SyntaxKind::JsxAttribute => node.initializer().is_some(),
        _ => false,
    }
}

// Go: ast/utilities.go:3097 IsVariableParameterOrProperty
pub fn is_variable_parameter_or_property(node: Node) -> bool {
    matches!(
        node.kind(),
        SyntaxKind::VariableDeclaration
            | SyntaxKind::Parameter
            | SyntaxKind::PropertySignature
            | SyntaxKind::PropertyDeclaration
    )
}

// PORT: Go `node.FunctionLikeData() != nil`. These are exactly the node kinds
// whose Go data embeds `FunctionLikeBase` (directly, through
// `FunctionLikeWithBodyBase`, `AccessorDeclarationBase` or
// `FunctionOrConstructorTypeNodeBase`). For them Go `node.Type()` returns
// `FunctionLikeData().Type`.
fn has_function_like_data_p4(node: Node) -> bool {
    matches!(
        node.kind(),
        SyntaxKind::FunctionDeclaration
            | SyntaxKind::CallSignature
            | SyntaxKind::ConstructSignature
            | SyntaxKind::Constructor
            | SyntaxKind::IndexSignature
            | SyntaxKind::MethodSignature
            | SyntaxKind::MethodDeclaration
            | SyntaxKind::GetAccessor
            | SyntaxKind::SetAccessor
            | SyntaxKind::FunctionType
            | SyntaxKind::ConstructorType
            | SyntaxKind::ArrowFunction
            | SyntaxKind::FunctionExpression
            | SyntaxKind::JsDocSignature
    )
}

// Go: ast/utilities.go:3137 GetTypeAnnotationNode
pub fn get_type_annotation_node(node: Node) -> Node {
    match node.kind() {
        SyntaxKind::VariableDeclaration
        | SyntaxKind::Parameter
        | SyntaxKind::PropertySignature
        | SyntaxKind::PropertyDeclaration
        | SyntaxKind::TypePredicate
        | SyntaxKind::ParenthesizedType
        | SyntaxKind::TypeOperator
        | SyntaxKind::MappedType
        | SyntaxKind::TypeAssertionExpression
        | SyntaxKind::AsExpression
        | SyntaxKind::SatisfiesExpression
        | SyntaxKind::TypeAliasDeclaration
        | SyntaxKind::JsTypeAliasDeclaration
        | SyntaxKind::NamedTupleMember
        | SyntaxKind::OptionalType
        | SyntaxKind::RestType
        | SyntaxKind::TemplateLiteralTypeSpan
        | SyntaxKind::JsDocTypeExpression
        | SyntaxKind::JsDocParameterTag
        | SyntaxKind::JsDocPropertyTag
        | SyntaxKind::JsDocNullableType
        | SyntaxKind::JsDocNonNullableType
        | SyntaxKind::JsDocOptionalType => node.type_(),
        _ => {
            if has_function_like_data_p4(node) {
                // Go: funcLike.Type
                return node.type_();
            }
            Node::NIL
        }
    }
}

// Go: ast/utilities.go:3123 IsObjectTypeDeclaration
pub fn is_object_type_declaration(node: Node) -> bool {
    is_class_like(node) || is_interface_declaration(node) || is_type_literal_node(node)
}

// Go: ast/utilities.go:3127 IsClassOrTypeElement
pub fn is_class_or_type_element(node: Node) -> bool {
    is_class_element(node) || is_type_element(node)
}

// Go: ast/utilities.go:3131 GetClassExtendsHeritageElement
pub fn get_class_extends_heritage_element(node: Node) -> Node {
    let heritage_elements = get_heritage_elements(node, SyntaxKind::ExtendsKeyword);
    if heritage_elements.len() > 0 {
        return heritage_elements[0];
    }
    Node::NIL
}

// Go: ast/utilities.go:3139 IsTypeKeywordToken
pub fn is_type_keyword_token(node: Node) -> bool {
    node.kind() == SyntaxKind::TypeKeyword
}

// Go: ast/utilities.go:3144 IsJSDocSingleCommentNodeList
// See `IsJSDocSingleCommentNode`.
pub fn is_js_doc_single_comment_node_list(node_list: NodeList) -> bool {
    if node_list.is_nil() || node_list.nodes().len() == 0 {
        return false;
    }
    let parent = node_list.nodes().get(0).parent();
    if parent.is_nil() {
        return false;
    }
    // PORT: Go compares `*NodeList` pointers (`nodeList == parent.CommentList()`).
    // A node belongs to exactly one list, so the lists are the same list iff
    // both are non-nil and share the same first node handle.
    is_js_doc_single_comment_node(parent) && {
        let comment_list = parent.comment_list();
        !comment_list.is_nil() && comment_list.nodes().get(0) == node_list.nodes().get(0)
    }
}

// Go: ast/utilities.go:3156 IsJSDocSingleCommentNodeComment
// See `IsJSDocSingleCommentNode`.
pub fn is_js_doc_single_comment_node_comment(node: Node) -> bool {
    if node.is_nil() || node.parent().is_nil() {
        return false;
    }
    is_js_doc_single_comment_node(node.parent())
        && node == node.parent().comment_list().nodes().get(0)
}

// Go: ast/utilities.go:3165 IsJSDocSingleCommentNode
// In Strada, if a JSDoc node has a single comment, that comment is represented as a string property
// as a simplification, and therefore that comment is not visited by `forEachChild`.
pub fn is_js_doc_single_comment_node(node: Node) -> bool {
    has_comment(node.kind())
        && !node.comment_list().is_nil()
        && node.comment_list().nodes().len() == 1
}

// Go: ast/utilities.go:3169 IsValidTypeOnlyAliasUseSite
pub fn is_valid_type_only_alias_use_site(use_site: Node) -> bool {
    use_site
        .flags()
        .intersects(NodeFlags::AMBIENT | NodeFlags::JS_DOC)
        || is_part_of_type_query(use_site)
        || is_identifier_in_non_emitting_heritage_clause(use_site)
        || is_part_of_possibly_valid_type_or_abstract_computed_property_name(use_site)
        || !(is_expression_node(use_site) || is_shorthand_property_name_use_site(use_site))
}

// Go: ast/utilities.go:3177 isIdentifierInNonEmittingHeritageClause
pub fn is_identifier_in_non_emitting_heritage_clause(node: Node) -> bool {
    if !is_identifier(node) {
        return false;
    }
    let mut parent = node.parent();
    while is_property_access_expression(parent) || is_expression_with_type_arguments(parent) {
        parent = parent.parent();
    }
    is_heritage_clause(parent)
        && (parent.token() == SyntaxKind::ImplementsKeyword
            || is_interface_declaration(parent.parent()))
}

// Go: ast/utilities.go:3188 isPartOfPossiblyValidTypeOrAbstractComputedPropertyName
pub fn is_part_of_possibly_valid_type_or_abstract_computed_property_name(node: Node) -> bool {
    let mut node = node;
    while node_kind_is(
        node,
        &[SyntaxKind::Identifier, SyntaxKind::PropertyAccessExpression],
    ) {
        node = node.parent();
    }
    if node.kind() != SyntaxKind::ComputedPropertyName {
        return false;
    }
    if has_syntactic_modifier(node.parent(), ModifierFlags::ABSTRACT) {
        return true;
    }
    node_kind_is(
        node.parent().parent(),
        &[SyntaxKind::InterfaceDeclaration, SyntaxKind::TypeLiteral],
    )
}

// Go: ast/utilities.go:3201 isShorthandPropertyNameUseSite
pub fn is_shorthand_property_name_use_site(use_site: Node) -> bool {
    is_identifier(use_site)
        && is_shorthand_property_assignment(use_site.parent())
        && use_site.parent().name() == use_site
}

// Go: ast/utilities.go:3205 GetPropertyNameForPropertyNameNode
pub fn get_property_name_for_property_name_node(name: Node) -> String {
    property_name_text(name).into_owned()
}

// Go: ast/utilities.go:3205 GetPropertyNameForPropertyNameNode
// Same logic, but it does not allocate. Node texts are `&'static str`, so
// only the minus-signed numeric name (`[-1]`) needs a new String. Hot
// callers (getLiteralTypeFromPropertyName) use this to skip a String per call.
pub fn property_name_text(name: Node) -> Cow<'static, str> {
    match name.kind() {
        SyntaxKind::Identifier
        | SyntaxKind::PrivateIdentifier
        | SyntaxKind::StringLiteral
        | SyntaxKind::NoSubstitutionTemplateLiteral
        | SyntaxKind::NumericLiteral
        | SyntaxKind::BigIntLiteral
        | SyntaxKind::JsxNamespacedName => {
            return Cow::Borrowed(name.text());
        }
        SyntaxKind::ComputedPropertyName => {
            let name_expression = name.expression();
            if is_string_or_numeric_literal_like(name_expression) {
                return Cow::Borrowed(name_expression.text());
            }
            if is_signed_numeric_literal(name_expression) {
                let text = name_expression.operand().text();
                if name_expression.operator() == SyntaxKind::MinusToken {
                    return Cow::Owned(format!("-{text}"));
                }
                return Cow::Borrowed(text);
            }
            return Cow::Borrowed(INTERNAL_SYMBOL_NAME_MISSING);
        }
        _ => {}
    }
    panic!("Unhandled case in getPropertyNameForPropertyNameNode")
}

// Go: ast/utilities.go:3227 IsPartOfTypeOnlyImportOrExportDeclaration
pub fn is_part_of_type_only_import_or_export_declaration(node: Node) -> bool {
    find_ancestor(node, &mut is_type_only_import_or_export_declaration).is_some()
}

// Go: ast/utilities.go:3231 IsEmittableImport
pub fn is_emittable_import(node: Node) -> bool {
    match node.kind() {
        SyntaxKind::ImportDeclaration => {
            node.import_clause().is_some() && !node.import_clause().is_type_only()
        }
        SyntaxKind::ExportDeclaration | SyntaxKind::ImportEqualsDeclaration => !node.is_type_only(),
        SyntaxKind::CallExpression => is_import_call(node),
        _ => false,
    }
}

// Go: ast/utilities.go:3243 IsResolutionModeOverrideHost
pub fn is_resolution_mode_override_host(node: Node) -> bool {
    if node.is_nil() {
        return false;
    }
    matches!(
        node.kind(),
        SyntaxKind::ImportType
            | SyntaxKind::ExportDeclaration
            | SyntaxKind::ImportDeclaration
            | SyntaxKind::JsImportDeclaration
    )
}

// PORT: Go reads the `Attributes` field of ImportTypeNode, ImportDeclaration
// and ExportDeclaration. In Rust `node.attributes()` is the Go `Node.Attributes()`
// method (JSX only), and fields.rs does not generate a clashing field accessor.
// The `Attributes` field is the only child of kind `ImportAttributes` on these
// kinds, so we find it with `for_each_child`.
fn import_attributes_field_p4(node: Node) -> Node {
    let mut attributes = Node::NIL;
    node.for_each_child(&mut |child: Node| {
        if child.kind() == SyntaxKind::ImportAttributes {
            attributes = child;
            return true;
        }
        false
    });
    attributes
}

// Go: ast/utilities.go:3254 HasResolutionModeOverride
pub fn has_resolution_mode_override(node: Node) -> bool {
    if node.is_nil() {
        return false;
    }
    let mut attributes = Node::NIL;
    match node.kind() {
        SyntaxKind::ImportType => {
            attributes = import_attributes_field_p4(node);
        }
        SyntaxKind::ImportDeclaration | SyntaxKind::JsImportDeclaration => {
            attributes = import_attributes_field_p4(node);
        }
        SyntaxKind::ExportDeclaration => {
            attributes = import_attributes_field_p4(node);
        }
        _ => {}
    }
    if attributes.is_some() {
        let (_, ok) = attributes.get_resolution_mode_override(None);
        return ok;
    }
    false
}

// Go: ast/utilities.go:3274 IsStringTextContainingNode
pub fn is_string_text_containing_node(node: Node) -> bool {
    node.kind() == SyntaxKind::StringLiteral || is_template_literal_kind(node.kind())
}

// Go: ast/utilities.go:3278 IsTemplateLiteralKind
pub fn is_template_literal_kind(kind: SyntaxKind) -> bool {
    // PORT: compare discriminants; crate::astdata::SyntaxKind does not derive Ord.
    SyntaxKind::FIRST_TEMPLATE_TOKEN as u16 <= kind as u16
        && kind as u16 <= SyntaxKind::LAST_TEMPLATE_TOKEN as u16
}

// Go: ast/utilities.go:3282 IsTemplateLiteralToken
pub fn is_template_literal_token(node: Node) -> bool {
    is_template_literal_kind(node.kind())
}

// Go: ast/utilities.go:3286 GetExternalModuleImportEqualsDeclarationExpression
pub fn get_external_module_import_equals_declaration_expression(node: Node) -> Node {
    debug_assert!(is_external_module_import_equals_declaration(node));
    node.module_reference().expression()
}

// Go: ast/utilities.go:3291 CreateModifiersFromModifierFlags
pub fn create_modifiers_from_modifier_flags(
    flags: ModifierFlags,
    create_modifier: &mut dyn FnMut(SyntaxKind) -> Node,
) -> Vec<Node> {
    let mut result: Vec<Node> = Vec::new();
    if flags.intersects(ModifierFlags::EXPORT) {
        result.push(create_modifier(SyntaxKind::ExportKeyword));
    }
    if flags.intersects(ModifierFlags::AMBIENT) {
        result.push(create_modifier(SyntaxKind::DeclareKeyword));
    }
    if flags.intersects(ModifierFlags::DEFAULT) {
        result.push(create_modifier(SyntaxKind::DefaultKeyword));
    }
    if flags.intersects(ModifierFlags::CONST) {
        result.push(create_modifier(SyntaxKind::ConstKeyword));
    }
    if flags.intersects(ModifierFlags::PUBLIC) {
        result.push(create_modifier(SyntaxKind::PublicKeyword));
    }
    if flags.intersects(ModifierFlags::PRIVATE) {
        result.push(create_modifier(SyntaxKind::PrivateKeyword));
    }
    if flags.intersects(ModifierFlags::PROTECTED) {
        result.push(create_modifier(SyntaxKind::ProtectedKeyword));
    }
    if flags.intersects(ModifierFlags::ABSTRACT) {
        result.push(create_modifier(SyntaxKind::AbstractKeyword));
    }
    if flags.intersects(ModifierFlags::STATIC) {
        result.push(create_modifier(SyntaxKind::StaticKeyword));
    }
    if flags.intersects(ModifierFlags::OVERRIDE) {
        result.push(create_modifier(SyntaxKind::OverrideKeyword));
    }
    if flags.intersects(ModifierFlags::READONLY) {
        result.push(create_modifier(SyntaxKind::ReadonlyKeyword));
    }
    if flags.intersects(ModifierFlags::ACCESSOR) {
        result.push(create_modifier(SyntaxKind::AccessorKeyword));
    }
    if flags.intersects(ModifierFlags::ASYNC) {
        result.push(create_modifier(SyntaxKind::AsyncKeyword));
    }
    if flags.intersects(ModifierFlags::IN) {
        result.push(create_modifier(SyntaxKind::InKeyword));
    }
    if flags.intersects(ModifierFlags::OUT) {
        result.push(create_modifier(SyntaxKind::OutKeyword));
    }
    result
}

// Go: ast/utilities.go:3341 GetThisParameter
pub fn get_this_parameter(signature: Node) -> Node {
    // callback tags do not currently support this parameters
    if signature.parameters().len() != 0 {
        let this_parameter = signature.parameters().get(0);
        if is_this_parameter(this_parameter) {
            return this_parameter;
        }
    }
    Node::NIL
}

// Go: ast/utilities.go:3352 ReplaceModifiers
pub fn replace_modifiers(factory: &NodeFactory, node: Node, modifier_array: ModifierList) -> Node {
    match node.kind() {
        SyntaxKind::TypeParameter => {
            return factory.update_type_parameter_declaration(
                node,
                modifier_array,
                node.name(),
                node.constraint(),
                node.expression(),
                node.default_type(),
            );
        }
        SyntaxKind::Parameter => {
            return factory.update_parameter_declaration(
                node,
                modifier_array,
                node.dot_dot_dot_token(),
                node.name(),
                node.question_token(),
                node.type_(),
                node.initializer(),
            );
        }
        SyntaxKind::ConstructorType => {
            return factory.update_constructor_type_node(
                node,
                modifier_array,
                node.type_parameter_list(),
                node.parameter_list(),
                node.type_(),
            );
        }
        SyntaxKind::PropertySignature => {
            return factory.update_property_signature_declaration(
                node,
                modifier_array,
                node.name(),
                node.postfix_token(),
                node.type_(),
                node.initializer(),
            );
        }
        SyntaxKind::PropertyDeclaration => {
            return factory.update_property_declaration(
                node,
                modifier_array,
                node.name(),
                node.postfix_token(),
                node.type_(),
                node.initializer(),
            );
        }
        SyntaxKind::MethodSignature => {
            return factory.update_method_signature_declaration(
                node,
                modifier_array,
                node.name(),
                node.postfix_token(),
                node.type_parameter_list(),
                node.parameter_list(),
                node.type_(),
            );
        }
        SyntaxKind::MethodDeclaration => {
            return factory.update_method_declaration(
                node,
                modifier_array,
                node.asterisk_token(),
                node.name(),
                node.postfix_token(),
                node.type_parameter_list(),
                node.parameter_list(),
                node.type_(),
                node.full_signature(),
                node.body(),
            );
        }
        SyntaxKind::Constructor => {
            return factory.update_constructor_declaration(
                node,
                modifier_array,
                node.type_parameter_list(),
                node.parameter_list(),
                node.type_(),
                node.full_signature(),
                node.body(),
            );
        }
        SyntaxKind::GetAccessor => {
            return factory.update_get_accessor_declaration(
                node,
                modifier_array,
                node.name(),
                node.type_parameter_list(),
                node.parameter_list(),
                node.type_(),
                node.full_signature(),
                node.body(),
            );
        }
        SyntaxKind::SetAccessor => {
            return factory.update_set_accessor_declaration(
                node,
                modifier_array,
                node.name(),
                node.type_parameter_list(),
                node.parameter_list(),
                node.type_(),
                node.full_signature(),
                node.body(),
            );
        }
        SyntaxKind::IndexSignature => {
            return factory.update_index_signature_declaration(
                node,
                modifier_array,
                node.parameter_list(),
                node.type_(),
            );
        }
        SyntaxKind::FunctionExpression => {
            return factory.update_function_expression(
                node,
                modifier_array,
                node.asterisk_token(),
                node.name(),
                node.type_parameter_list(),
                node.parameter_list(),
                node.type_(),
                node.full_signature(),
                node.body(),
            );
        }
        SyntaxKind::ArrowFunction => {
            return factory.update_arrow_function(
                node,
                modifier_array,
                node.type_parameter_list(),
                node.parameter_list(),
                node.type_(),
                node.full_signature(),
                node.equals_greater_than_token(),
                node.body(),
            );
        }
        SyntaxKind::ClassExpression => {
            return factory.update_class_expression(
                node,
                modifier_array,
                node.name(),
                node.type_parameter_list(),
                node.heritage_clauses(),
                node.member_list(),
            );
        }
        SyntaxKind::VariableStatement => {
            return factory.update_variable_statement(
                node,
                modifier_array,
                node.declaration_list(),
            );
        }
        SyntaxKind::FunctionDeclaration => {
            return factory.update_function_declaration(
                node,
                modifier_array,
                node.asterisk_token(),
                node.name(),
                node.type_parameter_list(),
                node.parameter_list(),
                node.type_(),
                node.full_signature(),
                node.body(),
            );
        }
        SyntaxKind::ClassDeclaration => {
            return factory.update_class_declaration(
                node,
                modifier_array,
                node.name(),
                node.type_parameter_list(),
                node.heritage_clauses(),
                node.member_list(),
            );
        }
        SyntaxKind::InterfaceDeclaration => {
            return factory.update_interface_declaration(
                node,
                modifier_array,
                node.name(),
                node.type_parameter_list(),
                node.heritage_clauses(),
                node.member_list(),
            );
        }
        SyntaxKind::TypeAliasDeclaration => {
            return factory.update_type_alias_declaration(
                node,
                modifier_array,
                node.name(),
                node.type_parameter_list(),
                node.type_(),
            );
        }
        SyntaxKind::EnumDeclaration => {
            return factory.update_enum_declaration(
                node,
                modifier_array,
                node.name(),
                node.member_list(),
            );
        }
        SyntaxKind::ModuleDeclaration => {
            return factory.update_module_declaration(
                node,
                modifier_array,
                node.keyword(),
                node.name(),
                node.attributes(),
                node.body(),
            );
        }
        SyntaxKind::ImportEqualsDeclaration => {
            return factory.update_import_equals_declaration(
                node,
                modifier_array,
                node.is_type_only(),
                node.name(),
                node.module_reference(),
            );
        }
        SyntaxKind::ImportDeclaration => {
            return factory.update_import_declaration(
                node,
                modifier_array,
                node.import_clause(),
                node.module_specifier(),
                node.attributes(),
            );
        }
        SyntaxKind::ExportAssignment => {
            return factory.update_export_assignment(
                node,
                modifier_array,
                node.is_export_equals(),
                node.type_(),
                node.expression(),
            );
        }
        SyntaxKind::ExportDeclaration => {
            return factory.update_export_declaration(
                node,
                modifier_array,
                node.is_type_only(),
                node.export_clause(),
                node.module_specifier(),
                node.attributes(),
            );
        }
        _ => {}
    }
    panic!(
        "Node that does not have modifiers tried to have modifier replaced: {}",
        node.kind() as i32
    )
}

// Go: ast/utilities.go:3590 IsLateVisibilityPaintedStatement
pub fn is_late_visibility_painted_statement(node: Node) -> bool {
    matches!(
        node.kind(),
        SyntaxKind::ImportDeclaration
            | SyntaxKind::JsImportDeclaration
            | SyntaxKind::ImportEqualsDeclaration
            | SyntaxKind::VariableStatement
            | SyntaxKind::ClassDeclaration
            | SyntaxKind::FunctionDeclaration
            | SyntaxKind::ModuleDeclaration
            | SyntaxKind::TypeAliasDeclaration
            | SyntaxKind::JsTypeAliasDeclaration
            | SyntaxKind::InterfaceDeclaration
            | SyntaxKind::EnumDeclaration
    )
}

// Go: ast/utilities.go:3609 IsExternalModuleAugmentation
pub fn is_external_module_augmentation(node: Node) -> bool {
    is_ambient_module(node) && is_module_augmentation_external(node)
}

// Go: ast/utilities.go:3613 GetSourceFileOfModule
pub fn get_source_file_of_module(symbols: &SymbolArena, module: SymbolId) -> Node {
    let mut declaration = symbols.sym(module).value_declaration;
    if declaration.is_nil() {
        declaration = get_non_augmentation_declaration(symbols, module);
    }
    get_source_file_of_node(declaration)
}

// Go: ast/utilities.go:3621 GetNonAugmentationDeclaration
pub fn get_non_augmentation_declaration(symbols: &SymbolArena, symbol: SymbolId) -> Node {
    symbols
        .sym(symbol)
        .declarations
        .iter()
        .copied()
        .find(|&d| !is_external_module_augmentation(d) && !is_global_scope_augmentation(d))
        .unwrap_or(Node::NIL)
}

// Go: ast/utilities.go:3658 IsTypeDeclaration
pub fn is_type_declaration(node: Node) -> bool {
    match node.kind() {
        SyntaxKind::TypeParameter
        | SyntaxKind::ClassDeclaration
        | SyntaxKind::InterfaceDeclaration
        | SyntaxKind::TypeAliasDeclaration
        | SyntaxKind::JsTypeAliasDeclaration
        | SyntaxKind::EnumDeclaration => true,
        SyntaxKind::ImportClause => node.is_type_only() && node.name().is_some(),
        SyntaxKind::ImportSpecifier | SyntaxKind::ExportSpecifier => {
            node.parent().parent().is_type_only()
        }
        _ => false,
    }
}

// Go: ast/utilities.go:3640 IsTypeDeclarationName
pub fn is_type_declaration_name(name: Node) -> bool {
    name.kind() == SyntaxKind::Identifier
        && is_type_declaration(name.parent())
        && get_name_of_declaration(name.parent()) == name
}

// Go: ast/utilities.go:3646 IsRightSideOfPropertyAccess
pub fn is_right_side_of_property_access(node: Node) -> bool {
    node.parent().kind() == SyntaxKind::PropertyAccessExpression && node.parent().name() == node
}

// Go: ast/utilities.go:3650 IsArgumentExpressionOfElementAccess
pub fn is_argument_expression_of_element_access(node: Node) -> bool {
    node.parent().is_some()
        && node.parent().kind() == SyntaxKind::ElementAccessExpression
        && node.parent().argument_expression() == node
}

// Go: ast/utilities.go:3654 ClimbPastPropertyAccess
pub fn climb_past_property_access(node: Node) -> Node {
    if is_right_side_of_property_access(node) {
        return node.parent();
    }
    node
}

// Go: ast/utilities.go:3661 climbPastPropertyOrElementAccess
pub fn climb_past_property_or_element_access(node: Node) -> Node {
    if is_right_side_of_property_access(node) || is_argument_expression_of_element_access(node) {
        return node.parent();
    }
    node
}

// Go: ast/utilities.go:3668 selectExpressionOfCallOrNewExpressionOrDecorator
pub fn select_expression_of_call_or_new_expression_or_decorator(node: Node) -> Node {
    if is_call_expression(node) || is_new_expression(node) || is_decorator(node) {
        return node.expression();
    }
    Node::NIL
}

// Go: ast/utilities.go:3675 selectTagOfTaggedTemplateExpression
pub fn select_tag_of_tagged_template_expression(node: Node) -> Node {
    if is_tagged_template_expression(node) {
        return node.tag();
    }
    Node::NIL
}

// Go: ast/utilities.go:3682 selectTagNameOfJsxOpeningLikeElement
pub fn select_tag_name_of_jsx_opening_like_element(node: Node) -> Node {
    if is_jsx_opening_element(node) || is_jsx_self_closing_element(node) {
        return node.tag_name();
    }
    Node::NIL
}

// Go: ast/utilities.go:3689 IsCallExpressionTarget
pub fn is_call_expression_target(
    node: Node,
    include_element_access: bool,
    skip_past_outer_expressions: bool,
) -> bool {
    is_callee_worker(
        node,
        is_call_expression,
        select_expression_of_call_or_new_expression_or_decorator,
        include_element_access,
        skip_past_outer_expressions,
    )
}
