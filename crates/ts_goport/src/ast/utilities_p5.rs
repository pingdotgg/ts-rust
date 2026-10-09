//! Port of `ast/utilities.go` lines 3632-4564.

use crate::prelude::*;

// Go: ast/utilities.go:3693 IsNewExpressionTarget
pub fn is_new_expression_target(
    node: Node,
    include_element_access: bool,
    skip_past_outer_expressions: bool,
) -> bool {
    is_callee_worker(
        node,
        is_new_expression,
        select_expression_of_call_or_new_expression_or_decorator,
        include_element_access,
        skip_past_outer_expressions,
    )
}

// Go: ast/utilities.go:3697 IsCallOrNewExpressionTarget
pub fn is_call_or_new_expression_target(
    node: Node,
    include_element_access: bool,
    skip_past_outer_expressions: bool,
) -> bool {
    is_callee_worker(
        node,
        is_call_or_new_expression,
        select_expression_of_call_or_new_expression_or_decorator,
        include_element_access,
        skip_past_outer_expressions,
    )
}

// Go: ast/utilities.go:3701 IsTaggedTemplateTag
pub fn is_tagged_template_tag(
    node: Node,
    include_element_access: bool,
    skip_past_outer_expressions: bool,
) -> bool {
    is_callee_worker(
        node,
        is_tagged_template_expression,
        select_tag_of_tagged_template_expression,
        include_element_access,
        skip_past_outer_expressions,
    )
}

// Go: ast/utilities.go:3705 IsDecoratorTarget
pub fn is_decorator_target(
    node: Node,
    include_element_access: bool,
    skip_past_outer_expressions: bool,
) -> bool {
    is_callee_worker(
        node,
        is_decorator,
        select_expression_of_call_or_new_expression_or_decorator,
        include_element_access,
        skip_past_outer_expressions,
    )
}

// Go: ast/utilities.go:3709 IsJsxOpeningLikeElementTagName
pub fn is_jsx_opening_like_element_tag_name(
    node: Node,
    include_element_access: bool,
    skip_past_outer_expressions: bool,
) -> bool {
    is_callee_worker(
        node,
        is_jsx_opening_like_element,
        select_tag_name_of_jsx_opening_like_element,
        include_element_access,
        skip_past_outer_expressions,
    )
}

// Go: ast/utilities.go:3713 isCalleeWorker
// PORT: Go func params become plain `fn` pointers; all callers pass package functions.
pub fn is_callee_worker(
    node: Node,
    pred: fn(Node) -> bool,
    callee_selector: fn(Node) -> Node,
    include_element_access: bool,
    skip_past_outer_expressions: bool,
) -> bool {
    let mut target = if include_element_access {
        climb_past_property_or_element_access(node)
    } else {
        climb_past_property_access(node)
    };
    if skip_past_outer_expressions {
        // Only skip outer expressions if the target is actually an expression node
        if is_expression(target) {
            // PORT: Go OEKAll = OEKParentheses | OEKAssertions | OEKPartiallyEmittedExpressions |
            // OEKExpressionsWithTypeArguments, with OEKAssertions = TypeAssertions | NonNullAssertions |
            // Satisfies. flags.rs has no OEK_ALL, so it is built here.
            let oek_all = OuterExpressionKinds::OEK_PARENTHESES
                | OuterExpressionKinds::OEK_TYPE_ASSERTIONS
                | OuterExpressionKinds::OEK_NON_NULL_ASSERTIONS
                | OuterExpressionKinds::OEK_SATISFIES
                | OuterExpressionKinds::OEK_PARTIALLY_EMITTED_EXPRESSIONS
                | OuterExpressionKinds::OEK_EXPRESSIONS_WITH_TYPE_ARGUMENTS;
            target = skip_outer_expressions(target, oek_all);
        }
    }
    target.is_some()
        && target.parent().is_some()
        && pred(target.parent())
        && callee_selector(target.parent()) == target
}

// Go: ast/utilities.go:3735 IsRightSideOfQualifiedNameOrPropertyAccess
pub fn is_right_side_of_qualified_name_or_property_access(node: Node) -> bool {
    let parent = node.parent();
    match parent.kind() {
        SyntaxKind::QualifiedName => return parent.right() == node,
        SyntaxKind::PropertyAccessExpression => return parent.name() == node,
        SyntaxKind::MetaProperty => return parent.name() == node,
        _ => {}
    }
    false
}

// Go: ast/utilities.go:3748 ShouldTransformImportCall
pub fn should_transform_import_call(
    file_name: &str,
    options: &CompilerOptions,
    implied_node_format_for_emit: ModuleKind,
) -> bool {
    let module_kind = options.get_emit_module_kind();
    if ModuleKind::NODE16 <= module_kind && module_kind <= ModuleKind::NODE_NEXT
        || module_kind == ModuleKind::PRESERVE
    {
        return false;
    }
    implied_node_format_for_emit < ModuleKind::ES2015
}

// Go: ast/utilities.go:3756 HasQuestionToken
pub fn has_question_token(node: Node) -> bool {
    is_question_token(node.question_token())
}

// Go: ast/utilities.go:3760 IsJsxOpeningLikeElement
pub fn is_jsx_opening_like_element(node: Node) -> bool {
    is_jsx_opening_element(node) || is_jsx_self_closing_element(node)
}

// Go: ast/utilities.go:3764 GetInvokedExpression
pub fn get_invoked_expression(node: Node) -> Node {
    match node.kind() {
        SyntaxKind::TaggedTemplateExpression => node.tag(),
        SyntaxKind::JsxOpeningElement | SyntaxKind::JsxSelfClosingElement => node.tag_name(),
        SyntaxKind::BinaryExpression => node.right(),
        SyntaxKind::JsxOpeningFragment => node,
        _ => node.expression(),
    }
}

// Go: ast/utilities.go:3779 IsCallOrNewExpression
pub fn is_call_or_new_expression(node: Node) -> bool {
    is_call_expression(node) || is_new_expression(node)
}

// Go: ast/utilities.go:3783 IndexOfNode
pub fn index_of_node(nodes: NodeSlice, node: Node) -> i32 {
    // PORT: Go slices.BinarySearchFunc returns the first position whose element
    // compares >= target. This lower-bound search keeps that exact position
    // without copying the list.
    let (mut lo, mut hi) = (0, nodes.len());
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        if compare_node_positions(nodes.get(mid), node) < 0 {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    if lo < nodes.len() && compare_node_positions(nodes.get(lo), node) == 0 {
        return lo as i32;
    }
    -1
}

// Go: ast/utilities.go:3791 CompareNodePositions
pub fn compare_node_positions(n1: Node, n2: Node) -> i32 {
    // PORT: inlined Go core.CompareTextRanges(n1.Loc, n2.Loc).
    let (r1, r2) = (n1.loc(), n2.loc());
    let c = r1.pos() - r2.pos();
    if c != 0 {
        return c;
    }
    r1.end() - r2.end()
}

// Go: ast/utilities.go:3795 IsUnterminatedLiteral
pub fn is_unterminated_literal(node: Node) -> bool {
    is_literal_kind(node.kind()) && node.token_flags().intersects(TokenFlags::UNTERMINATED)
        || is_template_literal_kind(node.kind())
            && node.template_flags().intersects(TokenFlags::UNTERMINATED)
}

// Gets a value indicating whether a class element is either a static or an instance property declaration with an initializer.
// Go: ast/utilities.go:3801 IsInitializedProperty
pub fn is_initialized_property(member: Node) -> bool {
    member.kind() == SyntaxKind::PropertyDeclaration && member.initializer().is_some()
}

// Go: ast/utilities.go:3806 IsTrivia
pub fn is_trivia(token: SyntaxKind) -> bool {
    (SyntaxKind::FIRST_TRIVIA_TOKEN as u16) <= (token as u16)
        && (token as u16) <= (SyntaxKind::LAST_TRIVIA_TOKEN as u16)
}

// Go: ast/utilities.go:3810 HasDecorators
pub fn has_decorators(node: Node) -> bool {
    has_syntactic_modifier(node, ModifierFlags::DECORATOR)
}

/// Go `hasFileNameImpl` (implements the Go `HasFileName` interface).
/// PORT: `tspath.Path` is a Go string type, ported as `String`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HasFileNameImpl {
    pub file_name: String,
    pub path: String,
}

// Go: ast/utilities.go:3850 NewHasFileName
pub fn new_has_file_name(file_name: &str, path: &str) -> HasFileNameImpl {
    HasFileNameImpl {
        file_name: file_name.to_string(),
        path: path.to_string(),
    }
}

impl HasFileNameImpl {
    // Go: ast/utilities.go:3857 (*hasFileNameImpl).FileName
    pub fn file_name(&self) -> String {
        self.file_name.clone()
    }

    // Go: ast/utilities.go:3830 (*hasFileNameImpl).Path (at 673a5f17d713; ts#64159 renames it
    // PathKey, ast/utilities.go:3861)
    pub fn path(&self) -> String {
        self.path.clone()
    }
}

// Go: ast/utilities.go:3834 GetSemanticJsxChildren
pub fn get_semantic_jsx_children(children: &[Node]) -> Vec<Node> {
    children
        .iter()
        .copied()
        .filter(|i| match i.kind() {
            SyntaxKind::JsxExpression => i.expression().is_some(),
            SyntaxKind::JsxText => !i.contains_only_trivia_white_spaces(),
            _ => true,
        })
        .collect()
}

// Returns true if the node kind has a comment property.
// Go: ast/utilities.go:3848 hasComment
pub fn has_comment(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        SyntaxKind::JsDoc
            | SyntaxKind::JsDocUnknownTag
            | SyntaxKind::JsDocAugmentsTag
            | SyntaxKind::JsDocImplementsTag
            | SyntaxKind::JsDocDeprecatedTag
            | SyntaxKind::JsDocPublicTag
            | SyntaxKind::JsDocPrivateTag
            | SyntaxKind::JsDocProtectedTag
            | SyntaxKind::JsDocReadonlyTag
            | SyntaxKind::JsDocOverrideTag
            | SyntaxKind::JsDocCallbackTag
            | SyntaxKind::JsDocOverloadTag
            | SyntaxKind::JsDocParameterTag
            | SyntaxKind::JsDocPropertyTag
            | SyntaxKind::JsDocReturnTag
            | SyntaxKind::JsDocThisTag
            | SyntaxKind::JsDocTypeTag
            | SyntaxKind::JsDocTemplateTag
            | SyntaxKind::JsDocTypedefTag
            | SyntaxKind::JsDocSeeTag
            | SyntaxKind::JsDocThrowsTag
            | SyntaxKind::JsDocSatisfiesTag
            | SyntaxKind::JsDocImportTag
    )
}

// Go: ast/utilities.go:3862 IsAssignmentPattern
pub fn is_assignment_pattern(node: Node) -> bool {
    node.kind() == SyntaxKind::ArrayLiteralExpression
        || node.kind() == SyntaxKind::ObjectLiteralExpression
}

// Go: ast/utilities.go:3866 GetElementsOfBindingOrAssignmentPattern
pub fn get_elements_of_binding_or_assignment_pattern(name: Node) -> Vec<Node> {
    match name.kind() {
        SyntaxKind::ObjectBindingPattern
        | SyntaxKind::ArrayBindingPattern
        | SyntaxKind::ArrayLiteralExpression => {
            // `a` in `{a}`
            // `a` in `[a]`
            name.elements().to_vec()
        }
        SyntaxKind::ObjectLiteralExpression => {
            // `a` in `{a}`
            name.properties().to_vec()
        }
        _ => Vec::new(),
    }
}

// Go: ast/utilities.go:3879 IsDeclarationBindingElement
pub fn is_declaration_binding_element(binding_element: Node) -> bool {
    matches!(
        binding_element.kind(),
        SyntaxKind::VariableDeclaration | SyntaxKind::Parameter | SyntaxKind::BindingElement
    )
}

/// Gets the name of an BindingOrAssignmentElement.
// Go: ast/utilities.go:3891 GetTargetOfBindingOrAssignmentElement
pub fn get_target_of_binding_or_assignment_element(binding_element: Node) -> Node {
    if is_declaration_binding_element(binding_element) {
        // `a` in `let { a } = ...`
        // `a` in `let { a = 1 } = ...`
        // `b` in `let { a: b } = ...`
        // `b` in `let { a: b = 1 } = ...`
        // `a` in `let { ...a } = ...`
        // `{b}` in `let { a: {b} } = ...`
        // `{b}` in `let { a: {b} = 1 } = ...`
        // `[b]` in `let { a: [b] } = ...`
        // `[b]` in `let { a: [b] = 1 } = ...`
        // `a` in `let [a] = ...`
        // `a` in `let [a = 1] = ...`
        // `a` in `let [...a] = ...`
        // `{a}` in `let [{a}] = ...`
        // `{a}` in `let [{a} = 1] = ...`
        // `[a]` in `let [[a]] = ...`
        // `[a]` in `let [[a] = 1] = ...`
        return binding_element.name();
    }

    if is_object_literal_element(binding_element) {
        match binding_element.kind() {
            SyntaxKind::PropertyAssignment => {
                // `b` in `({ a: b } = ...)`
                // `b` in `({ a: b = 1 } = ...)`
                // `{b}` in `({ a: {b} } = ...)`
                // `{b}` in `({ a: {b} = 1 } = ...)`
                // `[b]` in `({ a: [b] } = ...)`
                // `[b]` in `({ a: [b] = 1 } = ...)`
                // `b.c` in `({ a: b.c } = ...)`
                // `b.c` in `({ a: b.c = 1 } = ...)`
                // `b[0]` in `({ a: b[0] } = ...)`
                // `b[0]` in `({ a: b[0] = 1 } = ...)`
                return get_target_of_binding_or_assignment_element(binding_element.initializer());
            }
            SyntaxKind::ShorthandPropertyAssignment => {
                // `a` in `({ a } = ...)`
                // `a` in `({ a = 1 } = ...)`
                return binding_element.name();
            }
            SyntaxKind::SpreadAssignment => {
                // `a` in `({ ...a } = ...)`
                return get_target_of_binding_or_assignment_element(binding_element.expression());
            }
            _ => {}
        }

        // no target
        return Node::NIL;
    }

    if is_assignment_expression(binding_element, true /*excludeCompoundAssignment*/) {
        // `a` in `[a = 1] = ...`
        // `{a}` in `[{a} = 1] = ...`
        // `[a]` in `[[a] = 1] = ...`
        // `a.b` in `[a.b = 1] = ...`
        // `a[0]` in `[a[0] = 1] = ...`
        return get_target_of_binding_or_assignment_element(binding_element.left());
    }

    if is_spread_element(binding_element) {
        // `a` in `[...a] = ...`
        return get_target_of_binding_or_assignment_element(binding_element.expression());
    }

    // `a` in `[a] = ...`
    // `{a}` in `[{a}] = ...`
    // `[a]` in `[[a]] = ...`
    // `a.b` in `[a.b] = ...`
    // `a[0]` in `[a[0]] = ...`
    binding_element
}

// Go: ast/utilities.go:3961 TryGetPropertyNameOfBindingOrAssignmentElement
pub fn try_get_property_name_of_binding_or_assignment_element(binding_element: Node) -> Node {
    match binding_element.kind() {
        SyntaxKind::BindingElement => {
            // `a` in `let { a: b } = ...`
            // `[a]` in `let { [a]: b } = ...`
            // `"a"` in `let { "a": b } = ...`
            // `1` in `let { 1: b } = ...`
            if binding_element.property_name().is_some() {
                let property_name = binding_element.property_name();
                if is_computed_property_name(property_name)
                    && is_string_or_numeric_literal_like(property_name.expression())
                {
                    return property_name.expression();
                }
                return property_name;
            }
        }
        SyntaxKind::PropertyAssignment => {
            // `a` in `({ a: b } = ...)`
            // `[a]` in `({ [a]: b } = ...)`
            // `"a"` in `({ "a": b } = ...)`
            // `1` in `({ 1: b } = ...)`
            if binding_element.name().is_some() {
                let property_name = binding_element.name();
                if is_computed_property_name(property_name)
                    && is_string_or_numeric_literal_like(property_name.expression())
                {
                    return property_name.expression();
                }
                return property_name;
            }
        }
        SyntaxKind::SpreadAssignment => {
            // `a` in `({ ...a } = ...)`
            return binding_element.name();
        }
        _ => {}
    }

    let target = get_target_of_binding_or_assignment_element(binding_element);
    if target.is_some() && is_property_name(target) {
        return target;
    }
    Node::NIL
}

/// Walk an AssignmentPattern to determine if it contains object rest (`...`) syntax. We cannot rely on
/// propagation of `TransformFlags.ContainsObjectRestOrSpread` since it isn't propagated by default in
/// ObjectLiteralExpression and ArrayLiteralExpression since we do not know whether they belong to an
/// AssignmentPattern at the time the nodes are parsed.
// Go: ast/utilities.go:4014 ContainsObjectRestOrSpread
pub fn contains_object_rest_or_spread(node: Node) -> bool {
    if node
        .subtree_facts()
        .intersects(SubtreeFacts::SUBTREE_CONTAINS_OBJECT_REST_OR_SPREAD)
    {
        return true;
    }
    if node
        .subtree_facts()
        .intersects(SubtreeFacts::SUBTREE_CONTAINS_ES_OBJECT_REST_OR_SPREAD)
    {
        // check for nested spread assignments, otherwise '{ x: { a, ...b } = foo } = c'
        // will not be correctly interpreted by the rest/spread transformer
        for element in get_elements_of_binding_or_assignment_pattern(node) {
            let target = get_target_of_binding_or_assignment_element(element);
            if target.is_some() && is_assignment_pattern(target) {
                if target
                    .subtree_facts()
                    .intersects(SubtreeFacts::SUBTREE_CONTAINS_OBJECT_REST_OR_SPREAD)
                {
                    return true;
                }
                if target
                    .subtree_facts()
                    .intersects(SubtreeFacts::SUBTREE_CONTAINS_ES_OBJECT_REST_OR_SPREAD)
                {
                    if contains_object_rest_or_spread(target) {
                        return true;
                    }
                }
            }
        }
    }
    false
}

// Go: ast/utilities.go:4038 IsEmptyObjectLiteral
pub fn is_empty_object_literal(expression: Node) -> bool {
    is_object_literal_expression(expression) && expression.properties().len() == 0
}

// Go: ast/utilities.go:4042 IsEmptyArrayLiteral
pub fn is_empty_array_literal(expression: Node) -> bool {
    is_array_literal_expression(expression) && expression.elements().len() == 0
}

// Go: ast/utilities.go:4046 GetRestIndicatorOfBindingOrAssignmentElement
pub fn get_rest_indicator_of_binding_or_assignment_element(binding_element: Node) -> Node {
    match binding_element.kind() {
        SyntaxKind::Parameter => binding_element.dot_dot_dot_token(),
        SyntaxKind::BindingElement => binding_element.dot_dot_dot_token(),
        SyntaxKind::SpreadElement | SyntaxKind::SpreadAssignment => binding_element,
        _ => Node::NIL,
    }
}

// Go: ast/utilities.go:4058 IsJSDocNameReferenceContext
pub fn is_js_doc_name_reference_context(node: Node) -> bool {
    node.flags().intersects(NodeFlags::JS_DOC)
        && find_ancestor(node, &|node: Node| {
            is_js_doc_name_reference(node) || is_js_doc_link_like(node)
        })
        .is_some()
}

// GetJSDocRoot returns the containing JSDoc node for a node inside a JSDoc comment.
// Go: ast/utilities.go:4065 GetJSDocRoot
pub fn get_js_doc_root(node: Node) -> Node {
    find_ancestor(node.parent(), &|n: Node| n.kind() == SyntaxKind::JsDoc)
}

// GetJSDocHost returns the declaration that the JSDoc comment containing the given node is attached to.
// Go: ast/utilities.go:4072 GetJSDocHost
pub fn get_js_doc_host(node: Node) -> Node {
    let js_doc = get_js_doc_root(node);
    if js_doc.is_nil() {
        return Node::NIL;
    }
    js_doc.parent()
}

// GetHostSignatureFromJSDoc returns the function-like declaration that hosts the JSDoc comment
// containing the given node. This is used to resolve @link references to parameters.
// Go: ast/utilities.go:4082 GetHostSignatureFromJSDoc
pub fn get_host_signature_from_js_doc(node: Node) -> Node {
    let host = get_js_doc_host(node);
    if host.is_nil() {
        return Node::NIL;
    }
    // !!! Strada's getEffectiveJSDocHost applies JS assignment pattern transforms (getSourceOfAssignment, getSourceOfDefaultedAssignment, etc.) not yet ported
    if is_property_signature_declaration(host)
        && host.type_().is_some()
        && is_function_like(host.type_())
    {
        return host.type_();
    }
    if is_function_like(host) {
        return host;
    }
    Node::NIL
}

// Finds the declaration that owns the JSDoc for a function-like node.
// Keep these hosts aligned with JSDoc parameter reparsing so unmatched @param diagnostics use the same attachment rules.
// Keep in sync with getNextJSDocCommentLocation in the API's src/ast/jsdoc.ts
// Go: ast/utilities.go:4100 GetNextJSDocCommentLocation
pub fn get_next_js_doc_comment_location(node: Node) -> Node {
    let parent = node.parent();
    if parent.is_some() {
        match parent.kind() {
            SyntaxKind::PropertyAssignment
            | SyntaxKind::ExportAssignment
            | SyntaxKind::PropertyDeclaration
            | SyntaxKind::VariableDeclaration
            | SyntaxKind::SatisfiesExpression
            | SyntaxKind::ReturnStatement
            | SyntaxKind::VariableStatement
            | SyntaxKind::ExpressionStatement => return parent,
            SyntaxKind::VariableDeclarationList => {
                if parent.declarations().nodes().get(0) == node {
                    return parent;
                }
            }
            _ => {}
        }
    }
    Node::NIL
}

// Go: ast/utilities.go:4115 IsImportOrImportEqualsDeclaration
pub fn is_import_or_import_equals_declaration(node: Node) -> bool {
    is_import_declaration(node) || is_import_equals_declaration(node)
}

// Go: ast/utilities.go:4119 IsPrimitiveLiteralValue
pub fn is_primitive_literal_value(node: Node, include_big_int: bool) -> bool {
    match node.kind() {
        SyntaxKind::TrueKeyword
        | SyntaxKind::FalseKeyword
        | SyntaxKind::NumericLiteral
        | SyntaxKind::StringLiteral
        | SyntaxKind::NoSubstitutionTemplateLiteral => true,
        SyntaxKind::BigIntLiteral => include_big_int,
        SyntaxKind::PrefixUnaryExpression => {
            if node.operator() == SyntaxKind::MinusToken {
                return is_numeric_literal(node.operand())
                    || (include_big_int && is_big_int_literal(node.operand()));
            }
            if node.operator() == SyntaxKind::PlusToken {
                return is_numeric_literal(node.operand());
            }
            false
        }
        _ => false,
    }
}

// Go: ast/utilities.go:4142 HasInferredType
pub fn has_inferred_type(node: Node) -> bool {
    matches!(
        node.kind(),
        SyntaxKind::Parameter
            | SyntaxKind::PropertySignature
            | SyntaxKind::PropertyDeclaration
            | SyntaxKind::BindingElement
            | SyntaxKind::PropertyAccessExpression
            | SyntaxKind::ElementAccessExpression
            | SyntaxKind::BinaryExpression
            | SyntaxKind::CallExpression
            | SyntaxKind::VariableDeclaration
            | SyntaxKind::ExportAssignment
            | SyntaxKind::PropertyAssignment
            | SyntaxKind::ShorthandPropertyAssignment
            | SyntaxKind::JsDocParameterTag
            | SyntaxKind::JsDocPropertyTag
    )
}

// Go: ast/utilities.go:4166 IsKeyword
pub fn is_keyword(token: SyntaxKind) -> bool {
    (SyntaxKind::FIRST_KEYWORD as u16) <= (token as u16)
        && (token as u16) <= (SyntaxKind::LAST_KEYWORD as u16)
}

// Go: ast/utilities.go:4170 IsNonContextualKeyword
pub fn is_non_contextual_keyword(token: SyntaxKind) -> bool {
    is_keyword(token) && !is_contextual_keyword(token)
}

// Go: ast/utilities.go:4174 HasModifier
pub fn has_modifier(node: Node, flags: ModifierFlags) -> bool {
    node.modifier_flags().intersects(flags)
}

// Go: ast/utilities.go:4178 IsExpandoInitializer
pub fn is_expando_initializer(declaration: Node, initializer: Node) -> bool {
    if initializer.is_nil() {
        return false;
    }
    if is_function_expression_or_arrow_function(initializer) {
        return true;
    }
    if is_in_js_file(initializer) {
        return is_class_expression(initializer)
            || (is_object_literal_expression(initializer)
                && initializer.properties().len() == 0
                && declaration.type_().is_nil());
    }
    false
}

// Go: ast/utilities.go:4191 GetContainingFunction
pub fn get_containing_function(node: Node) -> Node {
    find_ancestor(node.parent(), &is_function_like)
}

// Go: ast/utilities.go:4195 ImportFromModuleSpecifier
pub fn import_from_module_specifier(node: Node) -> Node {
    let result = try_get_import_from_module_specifier(node);
    if result.is_some() {
        return result;
    }
    crate::gostd::debug::fail_bad_syntax_kind(node.parent().kind(), None)
}

// Go: ast/utilities.go:4203 TryGetImportFromModuleSpecifier
pub fn try_get_import_from_module_specifier(node: Node) -> Node {
    match node.parent().kind() {
        SyntaxKind::ImportDeclaration
        | SyntaxKind::JsImportDeclaration
        | SyntaxKind::ExportDeclaration => node.parent(),
        SyntaxKind::ExternalModuleReference => node.parent().parent(),
        SyntaxKind::CallExpression => {
            if is_import_call(node.parent())
                || is_require_call(
                    node.parent(),
                    false, /*requireStringLiteralLikeArgument*/
                )
            {
                return node.parent();
            }
            Node::NIL
        }
        SyntaxKind::LiteralType => {
            if !is_string_literal(node) {
                return Node::NIL;
            }
            if is_import_type_node(node.parent().parent()) {
                return node.parent().parent();
            }
            Node::NIL
        }
        _ => Node::NIL,
    }
}

// Go: ast/utilities.go:4226 IsImplicitlyExportedJSDocDeclaration
pub fn is_implicitly_exported_js_doc_declaration(node: Node) -> bool {
    if !is_source_file(node.parent()) || !is_external_or_common_js_module(node.parent()) {
        return false;
    }
    if is_js_type_alias_declaration(node) {
        return true;
    }
    // A reparsed ModuleDeclaration synthesized from a JSDoc @typedef/@callback
    // dotted name should also be treated as implicitly exported in modules.
    is_module_declaration(node) && node.flags().intersects(NodeFlags::REPARSED)
}

// Go: ast/utilities.go:4238 HasContextSensitiveParameters
pub fn has_context_sensitive_parameters(node: Node) -> bool {
    // Functions with type parameters are not context sensitive.
    // PORT: Go `node.TypeParameters() == nil`. The Go parser stores a nil Nodes
    // slice for an empty list (Arena.Clone of an empty slice is nil), and type
    // parameter lists are never "missing" lists, so nil == empty here.
    if node.type_parameters().is_empty() {
        // Functions with any parameters that lack type annotations are context sensitive.
        if node.parameters().iter().any(|p| p.type_().is_nil()) {
            return true;
        }
        if !is_arrow_function(node) {
            // If the first parameter is not an explicit 'this' parameter, then the function has
            // an implicit 'this' parameter which is subject to contextual typing.
            // Go: core.FirstOrNil(node.Parameters())
            let parameters = node.parameters();
            let parameter = if parameters.is_empty() {
                Node::NIL
            } else {
                parameters.get(0)
            };
            if parameter.is_nil() || !is_this_parameter(parameter) {
                return node.flags().intersects(NodeFlags::CONTAINS_THIS);
            }
        }
    }
    false
}

// Go: ast/utilities.go:4257 IsInfinityOrNaNString
pub fn is_infinity_or_na_n_string(name: &str) -> bool {
    name == "Infinity" || name == "-Infinity" || name == "NaN"
}

// Go: ast/utilities.go:4261 GetFirstConstructorWithBody
pub fn get_first_constructor_with_body(node: Node) -> Node {
    for member in node.members().iter() {
        if is_constructor_declaration(member) && node_is_present(member.body()) {
            return member;
        }
    }
    Node::NIL
}

// Returns true for nodes that are considered executable for the purposes of unreachable code detection.
// Go: ast/utilities.go:4271 IsPotentiallyExecutableNode
pub fn is_potentially_executable_node(node: Node) -> bool {
    if (SyntaxKind::FIRST_STATEMENT as u16) <= (node.kind() as u16)
        && (node.kind() as u16) <= (SyntaxKind::LAST_STATEMENT as u16)
    {
        if is_variable_statement(node) {
            let declaration_list = node.declaration_list();
            if get_combined_node_flags(declaration_list).intersects(NodeFlags::BLOCK_SCOPED) {
                return true;
            }
            let declarations = declaration_list.declarations().nodes();
            return declarations.iter().any(|d| d.initializer().is_some());
        }
        return true;
    }
    is_class_declaration(node) || is_enum_declaration(node) || is_module_declaration(node)
}

// Go: ast/utilities.go:4288 HasAbstractModifier
pub fn has_abstract_modifier(node: Node) -> bool {
    has_syntactic_modifier(node, ModifierFlags::ABSTRACT)
}

// Go: ast/utilities.go:4292 HasAmbientModifier
pub fn has_ambient_modifier(node: Node) -> bool {
    has_syntactic_modifier(node, ModifierFlags::AMBIENT)
}

// Go: ast/utilities.go:4296 NodeCanBeDecorated
pub fn node_can_be_decorated(
    use_legacy_decorators: bool,
    node: Node,
    parent: Node,
    grandparent: Node,
) -> bool {
    // private names cannot be used with decorators yet
    if use_legacy_decorators && node.name().is_some() && is_private_identifier(node.name()) {
        return false;
    }
    match node.kind() {
        SyntaxKind::ClassDeclaration => {
            // class declarations are valid targets
            return true;
        }
        SyntaxKind::ClassExpression => {
            // class expressions are valid targets for native decorators
            return !use_legacy_decorators;
        }
        SyntaxKind::PropertyDeclaration => {
            // property declarations are valid if their parent is a class declaration.
            return parent.is_some()
                && (use_legacy_decorators && is_class_declaration(parent)
                    || !use_legacy_decorators
                        && is_class_like(parent)
                        && !has_abstract_modifier(node)
                        && !has_ambient_modifier(node));
        }
        SyntaxKind::GetAccessor | SyntaxKind::SetAccessor | SyntaxKind::MethodDeclaration => {
            // if this method has a body and its parent is a class declaration, this is a valid target.
            return parent.is_some()
                && node.body().is_some()
                && (use_legacy_decorators && is_class_declaration(parent)
                    || !use_legacy_decorators && is_class_like(parent));
        }
        SyntaxKind::Parameter => {
            // TODO(rbuckton): ParameterDeclaration decorator support for ES decorators must wait until it is standardized
            if !use_legacy_decorators {
                return false;
            }
            // if the parameter's parent has a body and its grandparent is a class declaration, this is a valid target.
            return parent.is_some()
                && parent.body().is_some()
                && (parent.kind() == SyntaxKind::Constructor
                    || parent.kind() == SyntaxKind::MethodDeclaration
                    || parent.kind() == SyntaxKind::SetAccessor)
                && get_this_parameter(parent) != node
                && grandparent.is_some()
                && grandparent.kind() == SyntaxKind::ClassDeclaration;
        }
        _ => {}
    }

    false
}

// Go: ast/utilities.go:4330 ClassOrConstructorParameterIsDecorated
pub fn class_or_constructor_parameter_is_decorated(
    use_legacy_decorators: bool,
    node: Node,
) -> bool {
    if node_is_decorated(use_legacy_decorators, node, Node::NIL, Node::NIL) {
        return true;
    }
    let constructor = get_first_constructor_with_body(node);
    constructor.is_some() && child_is_decorated(use_legacy_decorators, constructor, node)
}

// Go: ast/utilities.go:4338 ClassElementOrClassElementParameterIsDecorated
pub fn class_element_or_class_element_parameter_is_decorated(
    use_legacy_decorators: bool,
    node: Node,
    parent: Node,
) -> bool {
    let mut parameters = NodeList::NIL;
    if is_accessor(node) {
        let decls = get_all_accessor_declarations(&parent.members().to_vec(), node);
        let mut first_accessor_with_decorators = Node::NIL;
        if has_decorators(decls.first_accessor) {
            first_accessor_with_decorators = decls.first_accessor;
        } else if decls.second_accessor.is_some() && has_decorators(decls.second_accessor) {
            first_accessor_with_decorators = decls.second_accessor;
        }
        if first_accessor_with_decorators.is_nil() || node != first_accessor_with_decorators {
            return false;
        }
        if decls.set_accessor.is_some() {
            parameters = decls.set_accessor.parameter_list();
        }
    } else if is_method_declaration(node) {
        parameters = node.parameter_list();
    }
    if node_is_decorated(use_legacy_decorators, node, parent, Node::NIL) {
        return true;
    }
    if !parameters.is_nil() && parameters.nodes().len() > 0 {
        for parameter in parameters.nodes().iter() {
            if is_this_parameter(parameter) {
                continue;
            }
            if node_is_decorated(use_legacy_decorators, parameter, node, parent) {
                return true;
            }
        }
    }
    false
}

// Go: ast/utilities.go:4373 NodeIsDecorated
pub fn node_is_decorated(
    use_legacy_decorators: bool,
    node: Node,
    parent: Node,
    grandparent: Node,
) -> bool {
    has_decorators(node) && node_can_be_decorated(use_legacy_decorators, node, parent, grandparent)
}

// Go: ast/utilities.go:4377 NodeOrChildIsDecorated
pub fn node_or_child_is_decorated(
    use_legacy_decorators: bool,
    node: Node,
    parent: Node,
    grandparent: Node,
) -> bool {
    node_is_decorated(use_legacy_decorators, node, parent, grandparent)
        || child_is_decorated(use_legacy_decorators, node, parent)
}

// Go: ast/utilities.go:4381 ChildIsDecorated
pub fn child_is_decorated(use_legacy_decorators: bool, node: Node, parent: Node) -> bool {
    match node.kind() {
        SyntaxKind::ClassDeclaration | SyntaxKind::ClassExpression => node
            .members()
            .iter()
            .any(|m| node_or_child_is_decorated(use_legacy_decorators, m, node, parent)),
        SyntaxKind::MethodDeclaration | SyntaxKind::SetAccessor | SyntaxKind::Constructor => node
            .parameters()
            .iter()
            .any(|p| node_is_decorated(use_legacy_decorators, p, node, parent)),
        _ => false,
    }
}

/// Go `ast.AllAccessorDeclarations`. Typed Go pointers (`*AccessorDeclaration`,
/// `*SetAccessorDeclaration`, `*GetAccessorDeclaration`) are all `Node`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AllAccessorDeclarations {
    pub first_accessor: Node,
    pub second_accessor: Node,
    pub set_accessor: Node,
    pub get_accessor: Node,
}

// Go: ast/utilities.go:4405 GetAllAccessorDeclarationsForDeclaration
pub fn get_all_accessor_declarations_for_declaration(
    accessor: Node,
    declarations_of_symbol: &[Node],
) -> AllAccessorDeclarations {
    let other_kind = if accessor.kind() == SyntaxKind::SetAccessor {
        SyntaxKind::GetAccessor
    } else if accessor.kind() == SyntaxKind::GetAccessor {
        SyntaxKind::SetAccessor
    } else {
        panic!("Unexpected node kind {:?}", accessor.kind());
    };
    // otherAccessor := GetDeclarationOfKind(c.getSymbolOfDeclaration(accessor), otherKind)
    let mut other_accessor = Node::NIL;
    for &d in declarations_of_symbol {
        if d.kind() == other_kind {
            other_accessor = d;
            break;
        }
    }

    let first_accessor;
    let second_accessor;
    if other_accessor.is_some() && (other_accessor.pos() < accessor.pos()) {
        first_accessor = other_accessor;
        second_accessor = accessor;
    } else {
        first_accessor = accessor;
        second_accessor = other_accessor;
    }

    let mut set_accessor = Node::NIL;
    let mut get_accessor = Node::NIL;
    if accessor.kind() == SyntaxKind::SetAccessor {
        set_accessor = accessor;
        if other_accessor.is_some() {
            get_accessor = other_accessor;
        }
    } else {
        get_accessor = accessor;
        if other_accessor.is_some() {
            set_accessor = other_accessor;
        }
    }

    AllAccessorDeclarations {
        first_accessor,
        second_accessor,
        set_accessor,
        get_accessor,
    }
}

// Go: ast/utilities.go:4455 GetAllAccessorDeclarations
pub fn get_all_accessor_declarations(
    parent_declarations: &[Node],
    accessor: Node,
) -> AllAccessorDeclarations {
    if has_dynamic_name(accessor) {
        // dynamic names can only be match up via checker symbol lookup, just return an object with just this accessor
        return get_all_accessor_declarations_for_declaration(accessor, &[accessor]);
    }

    let accessor_name = get_property_name_for_property_name_node(accessor.name());
    let accessor_static = is_static(accessor);
    let mut matches: Vec<Node> = Vec::new();
    for &member in parent_declarations {
        if !is_accessor(member) || is_static(member) != accessor_static {
            continue;
        }
        let member_name = get_property_name_for_property_name_node(member.name());
        if member_name == accessor_name {
            matches.push(member);
        }
    }
    get_all_accessor_declarations_for_declaration(accessor, &matches)
}

// Go: ast/utilities.go:4476 IsAsyncFunction
pub fn is_async_function(node: Node) -> bool {
    match node.kind() {
        SyntaxKind::FunctionDeclaration
        | SyntaxKind::FunctionExpression
        | SyntaxKind::ArrowFunction
        | SyntaxKind::MethodDeclaration => {
            // PORT: Go `node.BodyData()` fields Body and AsteriskToken.
            node.body().is_some()
                && node.asterisk_token().is_nil()
                && has_syntactic_modifier(node, ModifierFlags::ASYNC)
        }
        _ => false,
    }
}

/// Gets the most likely element type for a TypeNode. This is not an exhaustive test
/// as it assumes a rest argument can only be an array type (either T[], or Array<T>).
///
/// @param node The type node.
// Go: ast/utilities.go:4493 GetRestParameterElementType
pub fn get_rest_parameter_element_type(node: Node) -> Node {
    if node.is_nil() {
        return node;
    }
    if node.kind() == SyntaxKind::ArrayType {
        return node.element_type();
    }
    if node.kind() == SyntaxKind::TypeReference && !node.type_argument_list().is_nil() {
        // Go: core.FirstOrNil(TypeArguments.Nodes)
        let type_arguments = node.type_argument_list().nodes();
        return if type_arguments.is_empty() {
            Node::NIL
        } else {
            type_arguments.get(0)
        };
    }
    Node::NIL
}

// Go: ast/utilities.go:4506 TagNamesAreEquivalent
pub fn tag_names_are_equivalent(lhs: Node, rhs: Node) -> bool {
    if lhs.kind() != rhs.kind() {
        return false;
    }
    match lhs.kind() {
        SyntaxKind::Identifier => return lhs.text() == rhs.text(),
        SyntaxKind::ThisKeyword => return true,
        SyntaxKind::JsxNamespacedName => {
            return lhs.namespace().text() == rhs.namespace().text()
                && lhs.name().text() == rhs.name().text();
        }
        SyntaxKind::PropertyAccessExpression => {
            return lhs.name().text() == rhs.name().text()
                && tag_names_are_equivalent(lhs.expression(), rhs.expression());
        }
        _ => {}
    }
    panic!("Unhandled case in TagNamesAreEquivalent");
}

// Go: ast/utilities.go:4525 IsTagName
pub fn is_tag_name(node: Node) -> bool {
    node.parent().is_some() && is_js_doc_tag(node.parent()) && node.parent().tag_name() == node
}

// We want to store any numbers/strings if they were a name that could be
// related to a declaration.  So, if we have 'import x = require("something")'
// then we want 'something' to be in the name table.  Similarly, if we have
// "a['propname']" then we want to store "propname" in the name table.
// Go: ast/utilities.go:4533 literalIsName
pub fn literal_is_name(node: Node) -> bool {
    is_declaration_name(node)
        || node.parent().kind() == SyntaxKind::ExternalModuleReference
        || is_argument_of_element_access_expression(node)
        || is_literal_computed_property_declaration_name(node)
}

// Go: ast/utilities.go:4540 isArgumentOfElementAccessExpression
pub fn is_argument_of_element_access_expression(node: Node) -> bool {
    node.is_some()
        && node.parent().is_some()
        && node.parent().kind() == SyntaxKind::ElementAccessExpression
        && node.parent().argument_expression() == node
}

// If the given node is part of a subtree of JSDoc nodes that have been cloned into a reparsed construct,
// return the corresponding reparsed clone in the subtree. Otherwise, just return the node.
// Go: ast/utilities.go:4548 GetReparsedNodeForNode
pub fn get_reparsed_node_for_node(node: Node) -> Node {
    if node.is_some()
        && node.flags().intersects(NodeFlags::JS_DOC)
        && !node.flags().intersects(NodeFlags::REPARSED)
    {
        let file = get_source_file_of_node(node);
        let info = file.is_some().then(|| source_file_info(file));
        if let Some(info) = &info
            && !info.reparsed_clones.is_empty()
        {
            let reparsed_clones: &[Node] = &info.reparsed_clones;
            // PORT: Go slices.BinarySearchFunc: first position whose element compares >= node.
            let mut pos = reparsed_clones.partition_point(|c| compare_node_positions(*c, node) < 0);
            let found = pos < reparsed_clones.len()
                && compare_node_positions(reparsed_clones[pos], node) == 0;
            if !found && pos > 0 {
                pos -= 1;
            }
            let candidate = reparsed_clones[pos];
            if text_range_contained_by(node.loc(), candidate.loc()) {
                let reparsed = find_clone_in_node(candidate, node);
                if reparsed.is_some() {
                    return reparsed;
                }
            }
        }
    }
    node
}

// PORT: Go `core.TextRange.ContainedBy` (`t2.pos <= t.pos && t2.end >= t.end`), inlined
// here so this file does not depend on a TextRange method name.
fn text_range_contained_by(t: TextRange, t2: TextRange) -> bool {
    t2.pos() <= t.pos() && t2.end() >= t.end()
}

// Go: ast/utilities.go:4566 findCloneInNode
fn find_clone_in_node(mut node: Node, original: Node) -> Node {
    loop {
        if node.kind() == original.kind()
            && node.pos() == original.pos()
            && node.end() == original.end()
        {
            return node;
        }
        let current = node;
        let mut next = Node::NIL;
        let found_containing_child = current.for_each_child(&mut |n: Node| -> bool {
            if text_range_contained_by(original.loc(), n.loc()) {
                next = n;
                return true;
            }
            false
        });
        if !found_containing_child {
            return Node::NIL;
        }
        node = next;
    }
}

// Go: ast/utilities.go:4584 IsExpandoPropertyDeclaration
pub fn is_expando_property_declaration(node: Node) -> bool {
    node.is_some() && is_binary_expression(node)
}

// IsSuperProperty checks if a node is super.x or super[x].
// Go: ast/utilities.go:4589 IsSuperProperty
pub fn is_super_property(node: Node) -> bool {
    (is_property_access_expression(node) || is_element_access_expression(node))
        && node.expression().kind() == SyntaxKind::SuperKeyword
}

// Indicates whether a node is a potential source of an assigned name for a class, function, or arrow function.
// Go: ast/utilities.go:4595 IsNamedEvaluationSource
pub fn is_named_evaluation_source(node: Node) -> bool {
    match node.kind() {
        SyntaxKind::PropertyAssignment => return !is_proto_setter(node.name()),
        SyntaxKind::ShorthandPropertyAssignment => {
            return node.object_assignment_initializer().is_some();
        }
        SyntaxKind::VariableDeclaration => {
            return is_identifier(node.name()) && node.initializer().is_some();
        }
        SyntaxKind::Parameter => {
            return is_identifier(node.name())
                && node.initializer().is_some()
                && node.dot_dot_dot_token().is_nil();
        }
        SyntaxKind::BindingElement => {
            return is_identifier(node.name())
                && node.initializer().is_some()
                && node.dot_dot_dot_token().is_nil();
        }
        SyntaxKind::PropertyDeclaration => return node.initializer().is_some(),
        SyntaxKind::BinaryExpression => match node.operator_token().kind() {
            SyntaxKind::EqualsToken
            | SyntaxKind::AmpersandAmpersandEqualsToken
            | SyntaxKind::BarBarEqualsToken
            | SyntaxKind::QuestionQuestionEqualsToken => return is_identifier(node.left()),
            _ => {}
        },
        SyntaxKind::ExportAssignment => return true,
        _ => {}
    }
    false
}

// Indicates whether a property name is the special `__proto__` property.
// Per the ECMA-262 spec, this only matters for property assignments whose name is
// the Identifier `__proto__`, or the string literal `"__proto__"`, but not for
// computed property names.
// Go: ast/utilities.go:4624 IsProtoSetter
pub fn is_proto_setter(node: Node) -> bool {
    (is_identifier(node) || is_string_literal(node)) && node.text() == "__proto__"
}

// Go: ast/utilities.go:4628 IsStringLiteralLikeType (ts#63931)
pub fn is_string_literal_like_type(node: Node) -> bool {
    node.kind() == SyntaxKind::LiteralType && is_string_literal_like(node.literal())
}
