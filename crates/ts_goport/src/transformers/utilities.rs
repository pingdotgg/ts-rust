//! Port of Go `transformers/utilities.go`.

use crate::prelude::*;

// Go: transformers/utilities.go:12 IsGeneratedIdentifier
pub fn is_generated_identifier(emit_context: &EmitContext, name: Node) -> bool {
    emit_context.has_auto_generate_info(name)
}

// Go: transformers/utilities.go:16 IsHelperName
pub fn is_helper_name(emit_context: &EmitContext, name: Node) -> bool {
    emit_context
        .emit_flags(name)
        .intersects(EmitFlags::HELPER_NAME)
}

// Go: transformers/utilities.go:20 IsLocalName
pub fn is_local_name(emit_context: &EmitContext, name: Node) -> bool {
    emit_context
        .emit_flags(name)
        .intersects(EmitFlags::LOCAL_NAME)
}

// Go: transformers/utilities.go:24 IsExportName
pub fn is_export_name(emit_context: &EmitContext, name: Node) -> bool {
    emit_context
        .emit_flags(name)
        .intersects(EmitFlags::EXPORT_NAME)
}

// Go: transformers/utilities.go:28 IsIdentifierReference
pub fn is_identifier_reference(name: Node, parent: Node) -> bool {
    match parent.kind() {
        SyntaxKind::BinaryExpression
        | SyntaxKind::PrefixUnaryExpression
        | SyntaxKind::PostfixUnaryExpression
        | SyntaxKind::YieldExpression
        | SyntaxKind::AsExpression
        | SyntaxKind::SatisfiesExpression
        | SyntaxKind::ElementAccessExpression
        | SyntaxKind::NonNullExpression
        | SyntaxKind::SpreadElement
        | SyntaxKind::SpreadAssignment
        | SyntaxKind::ParenthesizedExpression
        | SyntaxKind::ArrayLiteralExpression
        | SyntaxKind::DeleteExpression
        | SyntaxKind::TypeOfExpression
        | SyntaxKind::VoidExpression
        | SyntaxKind::AwaitExpression
        | SyntaxKind::TypeAssertionExpression
        | SyntaxKind::ExpressionWithTypeArguments
        | SyntaxKind::JsxSelfClosingElement
        | SyntaxKind::JsxSpreadAttribute
        | SyntaxKind::JsxExpression
        | SyntaxKind::PartiallyEmittedExpression => {
            // all immediate children that can be `Identifier` would be instances of `IdentifierReference`
            true
        }
        SyntaxKind::ComputedPropertyName
        | SyntaxKind::Decorator
        | SyntaxKind::IfStatement
        | SyntaxKind::DoStatement
        | SyntaxKind::WhileStatement
        | SyntaxKind::WithStatement
        | SyntaxKind::ReturnStatement
        | SyntaxKind::SwitchStatement
        | SyntaxKind::CaseClause
        | SyntaxKind::ThrowStatement
        | SyntaxKind::ExpressionStatement
        | SyntaxKind::ExportAssignment
        | SyntaxKind::PropertyAccessExpression
        | SyntaxKind::TemplateSpan => {
            // only an `Expression()` child that can be `Identifier` would be an instance of `IdentifierReference`
            parent.expression() == name
        }
        SyntaxKind::VariableDeclaration
        | SyntaxKind::Parameter
        | SyntaxKind::BindingElement
        | SyntaxKind::PropertyDeclaration
        | SyntaxKind::PropertySignature
        | SyntaxKind::PropertyAssignment
        | SyntaxKind::EnumMember
        | SyntaxKind::JsxAttribute => {
            // only an `Initializer()` child that can be `Identifier` would be an instance of `IdentifierReference`
            parent.initializer() == name
        }
        SyntaxKind::ShorthandPropertyAssignment => parent.object_assignment_initializer() == name,
        SyntaxKind::ForStatement => {
            parent.initializer() == name
                || parent.condition() == name
                || parent.incrementor() == name
        }
        SyntaxKind::ForInStatement | SyntaxKind::ForOfStatement => {
            parent.initializer() == name || parent.expression() == name
        }
        SyntaxKind::ImportEqualsDeclaration => parent.module_reference() == name,
        SyntaxKind::ArrowFunction => parent.body() == name,
        SyntaxKind::ConditionalExpression => {
            parent.condition() == name || parent.when_true() == name || parent.when_false() == name
        }
        SyntaxKind::CallExpression | SyntaxKind::NewExpression => {
            parent.expression() == name || parent.arguments().iter().any(|a| a == name)
        }
        SyntaxKind::TaggedTemplateExpression => parent.tag() == name,
        SyntaxKind::ImportAttribute => parent.value() == name,
        SyntaxKind::JsxOpeningElement | SyntaxKind::JsxClosingElement => parent.tag_name() == name,
        _ => false,
    }
}

// Go: transformers/utilities.go:112 convertBindingElementToArrayAssignmentElement
fn convert_binding_element_to_array_assignment_element(
    emit_context: &EmitContext,
    element: Node,
) -> Node {
    let f = emit_context.factory();
    if element.name().is_nil() {
        let elision = f.new_omitted_expression();
        emit_context.set_original(elision, element);
        emit_context.assign_comment_and_source_map_ranges(elision, element);
        return elision;
    }
    let expression =
        convert_binding_name_to_assignment_element_target(emit_context, element.name());
    if element.dot_dot_dot_token().is_some() {
        let spread = f.new_spread_element(expression);
        emit_context.set_original(spread, element);
        emit_context.assign_comment_and_source_map_ranges(spread, element);
        return spread;
    }
    if element.initializer().is_some() {
        let assignment = f.new_assignment_expression(expression, element.initializer());
        emit_context.set_original(assignment, element);
        emit_context.assign_comment_and_source_map_ranges(assignment, element);
        return assignment;
    }
    expression
}

// Go: transformers/utilities.go:135 convertBindingElementToObjectAssignmentElement
fn convert_binding_element_to_object_assignment_element(
    emit_context: &EmitContext,
    element: Node,
) -> Node {
    let f = emit_context.factory();
    if element.dot_dot_dot_token().is_some() {
        let spread = f.new_spread_assignment(element.name());
        emit_context.set_original(spread, element);
        emit_context.assign_comment_and_source_map_ranges(spread, element);
        return spread;
    }
    if element.property_name().is_some() {
        let mut expression =
            convert_binding_name_to_assignment_element_target(emit_context, element.name());
        if element.initializer().is_some() {
            expression = f.new_assignment_expression(expression, element.initializer());
        }
        let assignment = f.new_property_assignment(
            ModifierList::NIL, /*modifiers*/
            element.property_name(),
            Node::NIL, /*postfixToken*/
            Node::NIL, /*typeNode*/
            expression,
        );
        emit_context.set_original(assignment, element);
        emit_context.assign_comment_and_source_map_ranges(assignment, element);
        return assignment;
    }
    let mut equals_token = Node::NIL;
    if element.initializer().is_some() {
        equals_token = f.new_token(SyntaxKind::EqualsToken);
    }
    let assignment = f.new_shorthand_property_assignment(
        ModifierList::NIL, /*modifiers*/
        element.name(),
        Node::NIL, /*postfixToken*/
        Node::NIL, /*typeNode*/
        equals_token,
        element.initializer(),
    );
    emit_context.set_original(assignment, element);
    emit_context.assign_comment_and_source_map_ranges(assignment, element);
    assignment
}

// Go: transformers/utilities.go:169 ConvertBindingPatternToAssignmentPattern
pub fn convert_binding_pattern_to_assignment_pattern(
    emit_context: &EmitContext,
    element: Node,
) -> Node {
    match element.kind() {
        SyntaxKind::ArrayBindingPattern => {
            convert_binding_element_to_array_assignment_pattern(emit_context, element)
        }
        SyntaxKind::ObjectBindingPattern => {
            convert_binding_element_to_object_assignment_pattern(emit_context, element)
        }
        _ => panic!("Unknown binding pattern"),
    }
}

// Go: transformers/utilities.go:180 convertBindingElementToObjectAssignmentPattern
fn convert_binding_element_to_object_assignment_pattern(
    emit_context: &EmitContext,
    element: Node,
) -> Node {
    let properties: Vec<Node> = element
        .elements()
        .iter()
        .map(|e| convert_binding_element_to_object_assignment_element(emit_context, e))
        .collect();
    let f = emit_context.factory();
    let property_list = f.new_node_list_with_loc(&properties, element.element_list().loc());
    let object = f.new_object_literal_expression(property_list, false /*multiLine*/);
    emit_context.set_original(object, element);
    emit_context.assign_comment_and_source_map_ranges(object, element);
    object
}

// Go: transformers/utilities.go:193 convertBindingElementToArrayAssignmentPattern
fn convert_binding_element_to_array_assignment_pattern(
    emit_context: &EmitContext,
    element: Node,
) -> Node {
    let elements: Vec<Node> = element
        .elements()
        .iter()
        .map(|e| convert_binding_element_to_array_assignment_element(emit_context, e))
        .collect();
    let f = emit_context.factory();
    let element_list = f.new_node_list_with_loc(&elements, element.element_list().loc());
    let object = f.new_array_literal_expression(element_list, false /*multiLine*/);
    emit_context.set_original(object, element);
    emit_context.assign_comment_and_source_map_ranges(object, element);
    object
}

// Go: transformers/utilities.go:206 convertBindingNameToAssignmentElementTarget
fn convert_binding_name_to_assignment_element_target(
    emit_context: &EmitContext,
    element: Node,
) -> Node {
    if is_binding_pattern(element) {
        return convert_binding_pattern_to_assignment_pattern(emit_context, element);
    }
    element
}

// Go: transformers/utilities.go:213 ConvertVariableDeclarationToAssignmentExpression
pub fn convert_variable_declaration_to_assignment_expression(
    emit_context: &EmitContext,
    element: Node,
) -> Node {
    if element.initializer().is_nil() {
        return Node::NIL;
    }
    let expression =
        convert_binding_name_to_assignment_element_target(emit_context, element.name());
    let assignment = emit_context
        .factory()
        .new_assignment_expression(expression, element.initializer());
    emit_context.set_original(assignment, element);
    emit_context.assign_comment_and_source_map_ranges(assignment, element);
    assignment
}

// Go: transformers/utilities.go:224 SingleOrMany
// PORT: Go distinguishes a nil slice (result nil) from an empty one (an
// empty SyntaxList). `None` is the nil slice.
pub fn single_or_many(nodes: Option<&[Node]>, factory: &NodeFactory) -> Node {
    let Some(nodes) = nodes else {
        return Node::NIL;
    };
    if nodes.len() == 1 {
        return nodes[0];
    }
    factory.new_syntax_list(nodes)
}

// Go: transformers/utilities.go:241 IsSimpleCopiableExpression
// Used in the module transformer to check if an expression is reasonably without sideeffect,
//
//	and thus better to copy into multiple places rather than to cache in a temporary variable
//	- this is mostly subjective beyond the requirement that the expression not be sideeffecting
//
// Also used by the logical assignment downleveling transform to skip temp variables when they're
// not needed.
pub fn is_simple_copiable_expression(expression: Node) -> bool {
    is_string_literal_like(expression)
        || is_numeric_literal(expression)
        || is_keyword_kind(expression.kind())
        || is_identifier(expression)
}

// Go: transformers/utilities.go:248 IsOriginalNodeSingleLine
pub fn is_original_node_single_line(emit_context: &EmitContext, node: Node) -> bool {
    if node.is_nil() {
        return false;
    }
    let original = emit_context.most_original(node);
    if original.is_nil() {
        return false;
    }
    let source = get_source_file_of_node(original);
    if source.is_nil() {
        return false;
    }
    let start_line = get_ecma_line_of_position(source, original.loc().pos());
    let end_line = get_ecma_line_of_position(source, original.loc().end());
    start_line == end_line
}

// Go: transformers/utilities.go:270 IsSimpleInlineableExpression
/// A simple inlinable expression is an expression which can be copied into multiple locations
/// without risk of repeating any sideeffects and whose value could not possibly change between
/// any such locations
pub fn is_simple_inlineable_expression(expression: Node) -> bool {
    !is_identifier(expression) && is_simple_copiable_expression(expression)
}

// Go: transformers/utilities.go:275 FindSuperStatementIndexPath
// FindSuperStatementIndexPath finds a path of indices to a statement containing a `super()` call.
// PORT: Go returns a nil slice when there is no `super()` call; that is an
// empty `Vec` here (Go callers only check `len`).
pub fn find_super_statement_index_path(statements: &[Node], start: usize) -> Vec<usize> {
    let mut indices =
        find_super_statement_index_path_worker(statements, start, Vec::new()).unwrap_or_default();
    indices.reverse();
    indices
}

// Go: transformers/utilities.go:281 findSuperStatementIndexPathWorker
fn find_super_statement_index_path_worker(
    statements: &[Node],
    start: usize,
    mut indices: Vec<usize>,
) -> Option<Vec<usize>> {
    for (i, &statement) in statements.iter().enumerate().skip(start) {
        if get_super_call_from_statement(statement).is_some() {
            indices.push(i);
            return Some(indices);
        } else if is_try_statement(statement) {
            if let Some(mut result) = find_super_statement_index_path_worker(
                &statement.try_block().statements().to_vec(),
                0,
                indices.clone(),
            ) {
                result.push(i);
                return Some(result);
            }
        }
    }
    None
}

// Go: transformers/utilities.go:296 GetSuperCallFromStatement
// GetSuperCallFromStatement extracts the super() call expression from an expression statement, if any.
pub fn get_super_call_from_statement(statement: Node) -> Node {
    if !is_expression_statement(statement) {
        return Node::NIL;
    }
    let expression = skip_parentheses(statement.expression());
    if is_super_call(expression) {
        return expression;
    }
    Node::NIL
}

// Go: transformers/utilities.go:308 MoveRangePastModifiers
// MoveRangePastModifiers returns a text range that starts past any modifiers on the node.
pub fn move_range_past_modifiers(node: Node) -> TextRange {
    if is_property_declaration(node) || is_method_declaration(node) {
        return TextRange::new(node.name().pos(), node.end());
    }

    let mut last_modifier = Node::NIL;
    if can_have_modifiers(node) {
        last_modifier = node.modifier_nodes().last().unwrap_or(Node::NIL);
    }

    if last_modifier.is_some() && !position_is_synthesized(last_modifier.end()) {
        return TextRange::new(last_modifier.end(), node.end());
    }
    move_range_past_decorators(node)
}

// Go: transformers/utilities.go:325 MoveRangePastDecorators
// MoveRangePastDecorators returns a text range that starts past any decorators on the node.
pub fn move_range_past_decorators(node: Node) -> TextRange {
    let mut last_decorator = Node::NIL;
    if can_have_modifiers(node) {
        last_decorator = node
            .modifier_nodes()
            .iter()
            .filter(|&n| is_decorator(n))
            .last()
            .unwrap_or(Node::NIL);
    }

    if last_decorator.is_some() && !position_is_synthesized(last_decorator.end()) {
        return TextRange::new(last_decorator.end(), node.end());
    }
    node.loc()
}

// Go: transformers/utilities.go:341 GetNonAssignmentOperatorForCompoundAssignment
// GetNonAssignmentOperatorForCompoundAssignment returns the non-assignment operator for a compound assignment.
pub fn get_non_assignment_operator_for_compound_assignment(kind: SyntaxKind) -> SyntaxKind {
    match kind {
        SyntaxKind::PlusEqualsToken => SyntaxKind::PlusToken,
        SyntaxKind::MinusEqualsToken => SyntaxKind::MinusToken,
        SyntaxKind::AsteriskEqualsToken => SyntaxKind::AsteriskToken,
        SyntaxKind::AsteriskAsteriskEqualsToken => SyntaxKind::AsteriskAsteriskToken,
        SyntaxKind::SlashEqualsToken => SyntaxKind::SlashToken,
        SyntaxKind::PercentEqualsToken => SyntaxKind::PercentToken,
        SyntaxKind::LessThanLessThanEqualsToken => SyntaxKind::LessThanLessThanToken,
        SyntaxKind::GreaterThanGreaterThanEqualsToken => SyntaxKind::GreaterThanGreaterThanToken,
        SyntaxKind::GreaterThanGreaterThanGreaterThanEqualsToken => {
            SyntaxKind::GreaterThanGreaterThanGreaterThanToken
        }
        SyntaxKind::AmpersandEqualsToken => SyntaxKind::AmpersandToken,
        SyntaxKind::BarEqualsToken => SyntaxKind::BarToken,
        SyntaxKind::CaretEqualsToken => SyntaxKind::CaretToken,
        SyntaxKind::BarBarEqualsToken => SyntaxKind::BarBarToken,
        SyntaxKind::AmpersandAmpersandEqualsToken => SyntaxKind::AmpersandAmpersandToken,
        SyntaxKind::QuestionQuestionEqualsToken => SyntaxKind::QuestionQuestionToken,
        _ => kind,
    }
}
