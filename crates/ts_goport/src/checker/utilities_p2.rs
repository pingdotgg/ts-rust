//! Go `internal/checker/utilities.go` lines 904-1844.

use crate::jsnum::PseudoBigInt;
use crate::prelude::*;
use std::cell::Cell;

// Go: checker/utilities.go:1049 isValidNumberString
pub fn is_valid_number_string(s: &str, round_trip_only: bool) -> bool {
    if s.is_empty() {
        return false;
    }
    let n = crate::jsnum::from_string(s);
    !n.is_nan() && !n.is_infinite() && (!round_trip_only || n.to_string() == s)
}

// Go: checker/utilities.go:1057 isValidBigIntString
pub fn is_valid_big_int_string(s: &str, round_trip_only: bool) -> bool {
    if s.is_empty() {
        return false;
    }
    let mut scanner = crate::frontend::scanner::new_scanner();
    scanner.set_skip_trivia(false);
    let success = Rc::new(Cell::new(true));
    let success_in_callback = success.clone();
    scanner.set_on_error(Some(Box::new(
        move |_diagnostic: &'static crate::diagnostics::Message,
              _start: i32,
              _length: i32,
              _args: Vec<String>| {
            success_in_callback.set(false);
        },
    )));
    let text = format!("{s}n");
    scanner.set_text(&text);
    let mut result = scanner.scan();
    let negative = result == SyntaxKind::MinusToken;
    if negative {
        result = scanner.scan();
    }
    let flags = scanner.token_flags();
    // validate that
    // * scanning proceeded without error
    // * a bigint can be scanned, and that when it is scanned, it is
    // * the full length of the input string (so the scanner is one character beyond the augmented input length)
    // * it does not contain a numeric separator (the `BigInt` constructor does not accept a numeric separator in its input)
    success.get()
        && result == SyntaxKind::BigIntLiteral
        && scanner.token_end() == s.len() as i32 + 1
        && !flags.intersects(TokenFlags::CONTAINS_SEPARATOR)
        && (!round_trip_only
            || s == pseudo_big_int_to_string(&PseudoBigInt::new(
                &crate::jsnum::parse_pseudo_big_int(scanner.token_value()),
                negative,
            )))
}

// Go: checker/utilities.go:1083 isValidESSymbolDeclaration
pub fn is_valid_es_symbol_declaration(node: Node) -> bool {
    if is_variable_declaration(node) {
        return is_var_const(node)
            && is_identifier(node.name())
            && is_variable_declaration_in_variable_statement(node);
    }
    if is_property_declaration(node) {
        return has_readonly_modifier(node) && has_static_modifier(node);
    }
    is_property_signature_declaration(node) && has_readonly_modifier(node)
}

// Go: checker/utilities.go:1093 isVariableDeclarationInVariableStatement
pub fn is_variable_declaration_in_variable_statement(node: Node) -> bool {
    is_variable_declaration_list(node.parent()) && is_variable_statement(node.parent().parent())
}

impl Checker {
    // Go: checker/utilities.go:1097 IsKnownSymbol
    // PERF: intern1 fix 2. A name that is not internal is not late bound,
    // with no text read.
    pub fn is_known_symbol(&self, symbol: SymbolId) -> bool {
        let name = &self.sym(symbol).name;
        name.is_internal() && is_late_bound_name(name)
    }

    // Go: checker/utilities.go:1101 IsPrivateIdentifierSymbol
    pub fn is_private_identifier_symbol(&self, symbol: SymbolId) -> bool {
        if symbol.is_nil() {
            return false;
        }
        self.sym(symbol)
            .name
            .starts_with(&format!("{INTERNAL_SYMBOL_NAME_PREFIX}#"))
    }
}

// Go: checker/utilities.go:1108 isLateBoundName
// PORT: Go checks `name[0] == '\xfe' && name[1] == '@'`. The byte 0xFE is
// INTERNAL_SYMBOL_NAME_PREFIX in the port form (see `ast::misc`), so this
// checks for that prefix followed by the byte '@'.
pub fn is_late_bound_name(name: &str) -> bool {
    match name.strip_prefix(INTERNAL_SYMBOL_NAME_PREFIX) {
        Some(rest) => !rest.is_empty() && rest.as_bytes()[0] == b'@',
        None => false,
    }
}

impl Checker {
    // Go: checker/utilities.go:1112 isObjectOrArrayLiteralType
    pub fn is_object_or_array_literal_type(&self, t: TypeId) -> bool {
        self.ty(t)
            .object_flags
            .intersects(ObjectFlags::OBJECT_LITERAL | ObjectFlags::ARRAY_LITERAL)
    }
}

// Go: checker/utilities.go:1116 getContainingClassExcludingClassDecorators
pub fn get_containing_class_excluding_class_decorators(node: Node) -> Node {
    let decorator = find_ancestor_or_quit(node.parent(), |n: Node| {
        if is_class_like(n) {
            return FindAncestorResult::FIND_ANCESTOR_QUIT;
        }
        if is_decorator(n) {
            return FindAncestorResult::FIND_ANCESTOR_TRUE;
        }
        FindAncestorResult::FIND_ANCESTOR_FALSE
    });
    if decorator.is_some() && is_class_like(decorator.parent()) {
        return get_containing_class(decorator.parent());
    }
    if decorator.is_some() {
        return get_containing_class(decorator);
    }
    get_containing_class(node)
}

impl Checker {
    // Go: checker/utilities.go:1135 isThisTypeParameter
    pub fn is_this_type_parameter(&self, t: TypeId) -> bool {
        let ty = self.ty(t);
        ty.flags.intersects(TypeFlags::TYPE_PARAMETER) && ty.as_type_parameter().is_this_type
    }
}

// Go: checker/utilities.go:1139 isClassInstanceProperty
pub fn is_class_instance_property(node: Node) -> bool {
    if is_in_js_file(node) && is_expando_property_declaration(node) {
        let left = node.left();
        return (!is_bindable_static_access_expression(left, false /*excludeThisKeyword*/)
            || !is_prototype_access(left.expression()))
            && !is_bindable_static_name_expression(left, true /*excludeThisKeyword*/);
    }
    node.parent().is_some()
        && is_class_like(node.parent())
        && is_property_declaration(node)
        && !has_accessor_modifier(node)
}

// Go: checker/utilities.go:1148 isThisInitializedObjectBindingExpression
pub fn is_this_initialized_object_binding_expression(node: Node) -> bool {
    node.is_some()
        && (is_shorthand_property_assignment(node) || is_property_assignment(node))
        && is_binary_expression(node.parent().parent())
        && node.parent().parent().operator_token().kind() == SyntaxKind::EqualsToken
        && node.parent().parent().right().kind() == SyntaxKind::ThisKeyword
}

// Go: checker/utilities.go:1154 isThisInitializedDeclaration
pub fn is_this_initialized_declaration(node: Node) -> bool {
    node.is_some()
        && is_variable_declaration(node)
        && node.initializer().is_some()
        && node.initializer().kind() == SyntaxKind::ThisKeyword
}

// Go: checker/utilities.go:1158 isInfinityOrNaNString
pub fn is_infinity_or_nan_string(name: &str) -> bool {
    name == "Infinity" || name == "-Infinity" || name == "NaN"
}

impl Checker {
    // Go: checker/utilities.go:1162 isConstantVariable
    pub fn is_constant_variable(&mut self, symbol: SymbolId) -> bool {
        self.sym_flags(symbol).intersects(SymbolFlags::VARIABLE)
            && self
                .get_declaration_node_flags_from_symbol(symbol)
                .intersects(NodeFlags::CONSTANT)
    }

    // Go: checker/utilities.go:1166 isParameterOrMutableLocalVariable
    pub fn is_parameter_or_mutable_local_variable(&self, symbol: SymbolId) -> bool {
        // Return true if symbol is a parameter, a catch clause variable, or a mutable local variable
        let value_declaration = self.sym(symbol).value_declaration;
        if value_declaration.is_some() {
            let declaration = get_root_declaration(value_declaration);
            return declaration.is_some()
                && (is_parameter_declaration(declaration)
                    || is_variable_declaration(declaration)
                        && (is_catch_clause(declaration.parent())
                            || self.is_mutable_local_variable_declaration(declaration)));
        }
        false
    }

    // Go: checker/utilities.go:1175 isMutableLocalVariableDeclaration
    pub fn is_mutable_local_variable_declaration(&self, declaration: Node) -> bool {
        // Return true if symbol is a non-exported and non-global `let` variable
        !declaration.parent().parser_flags(NodeFlags::LET).is_empty()
            && !(get_combined_modifier_flags(declaration).intersects(ModifierFlags::EXPORT)
                || declaration.parent().parent().kind() == SyntaxKind::VariableStatement
                    && is_global_source_file(declaration.parent().parent().parent()))
    }
}

// Go: checker/utilities.go:1180 isInAmbientOrTypeNode
// PERF: U4 (CH7), as `Checker::is_in_ambient_or_type_node`.
pub fn is_in_ambient_or_type_node(node: Node) -> bool {
    !node.parser_flags(NodeFlags::AMBIENT).is_empty()
        || find_ancestor_with_kind(node, |_, kind| {
            matches!(
                kind,
                SyntaxKind::InterfaceDeclaration
                    | SyntaxKind::TypeAliasDeclaration
                    | SyntaxKind::JsTypeAliasDeclaration
                    | SyntaxKind::TypeLiteral
            )
        })
        .is_some()
}

// Go: checker/utilities.go:1186 isLiteralExpressionOfObject
pub fn is_literal_expression_of_object(node: Node) -> bool {
    matches!(
        node.kind(),
        SyntaxKind::ObjectLiteralExpression
            | SyntaxKind::ArrayLiteralExpression
            | SyntaxKind::RegularExpressionLiteral
            | SyntaxKind::FunctionExpression
            | SyntaxKind::ClassExpression
    )
}

// Go: checker/utilities.go:1195 canHaveFlowNode
// PORT: Go `node.FlowNodeData() != nil`. That is non-nil exactly for node
// kinds whose Go data struct embeds `FlowNodeBase` (directly or through
// StatementBase, IterationStatementBase or AccessorDeclarationBase, see
// ast/ast_generated.go), so this lists those kinds.
pub fn can_have_flow_node(node: Node) -> bool {
    matches!(
        node.kind(),
        // IterationStatementBase
        SyntaxKind::DoStatement
            | SyntaxKind::WhileStatement
            | SyntaxKind::ForStatement
            // StatementBase
            | SyntaxKind::ForInStatement
            | SyntaxKind::ForOfStatement
            | SyntaxKind::EmptyStatement
            | SyntaxKind::IfStatement
            | SyntaxKind::BreakStatement
            | SyntaxKind::ContinueStatement
            | SyntaxKind::ReturnStatement
            | SyntaxKind::WithStatement
            | SyntaxKind::SwitchStatement
            | SyntaxKind::ThrowStatement
            | SyntaxKind::TryStatement
            | SyntaxKind::DebuggerStatement
            | SyntaxKind::LabeledStatement
            | SyntaxKind::ExpressionStatement
            | SyntaxKind::Block
            | SyntaxKind::VariableStatement
            | SyntaxKind::MissingDeclaration
            | SyntaxKind::FunctionDeclaration
            | SyntaxKind::ClassDeclaration
            | SyntaxKind::InterfaceDeclaration
            | SyntaxKind::TypeAliasDeclaration
            | SyntaxKind::JsTypeAliasDeclaration
            | SyntaxKind::EnumDeclaration
            | SyntaxKind::ModuleBlock
            | SyntaxKind::NotEmittedStatement
            | SyntaxKind::ImportDeclaration
            | SyntaxKind::JsImportDeclaration
            | SyntaxKind::ExportAssignment
            | SyntaxKind::NamespaceExportDeclaration
            | SyntaxKind::ModuleDeclaration
            | SyntaxKind::ImportEqualsDeclaration
            | SyntaxKind::ExportDeclaration
            // AccessorDeclarationBase
            | SyntaxKind::GetAccessor
            | SyntaxKind::SetAccessor
            // Other FlowNodeBase embedders
            | SyntaxKind::Identifier
            | SyntaxKind::QualifiedName
            | SyntaxKind::BindingElement
            | SyntaxKind::MethodDeclaration
            | SyntaxKind::ArrowFunction
            | SyntaxKind::FunctionExpression
            | SyntaxKind::PropertyAccessExpression
            | SyntaxKind::ElementAccessExpression
            | SyntaxKind::MetaProperty
            // KeywordExpression
            | SyntaxKind::NullKeyword
            | SyntaxKind::TrueKeyword
            | SyntaxKind::FalseKeyword
            | SyntaxKind::ThisKeyword
            | SyntaxKind::SuperKeyword
            | SyntaxKind::ImportKeyword
    )
}

// Go: checker/utilities.go:1199 isNonNullAccess
pub fn is_non_null_access(node: Node) -> bool {
    is_access_expression(node) && is_non_null_expression(node.expression())
}

// Go: checker/utilities.go:1203 getBindingElementPropertyName
pub fn get_binding_element_property_name(node: Node) -> Node {
    node.property_name_or_name()
}

// Go: checker/utilities.go:1207 isCallChain
pub fn is_call_chain(node: Node) -> bool {
    is_call_expression(node) && !node.parser_flags(NodeFlags::OPTIONAL_CHAIN).is_empty()
}

impl Checker {
    // Go: checker/utilities.go:1211 callLikeExpressionMayHaveTypeArguments
    pub fn call_like_expression_may_have_type_arguments(&self, node: Node) -> bool {
        is_call_or_new_expression(node)
            || is_tagged_template_expression(node)
            || is_jsx_opening_like_element(node)
    }
}

// Go: checker/utilities.go:1215 isSuperCall
// PORT: identical to `ast.IsSuperCall`, which is already the public
// `is_super_call` in `crate::ast`. A second public item with the same name
// would make the prelude glob ambiguous, so this copy stays private and
// checker callers resolve to the identical ast function.
#[allow(dead_code)]
fn is_super_call(n: Node) -> bool {
    is_call_expression(n) && n.expression().kind() == SyntaxKind::SuperKeyword
}

// Go: checker/utilities.go:1219 getMembersOfDeclaration
pub fn get_members_of_declaration(node: Node) -> Vec<Node> {
    match node.kind() {
        SyntaxKind::InterfaceDeclaration
        | SyntaxKind::ClassDeclaration
        | SyntaxKind::ClassExpression
        | SyntaxKind::TypeLiteral => node.members().to_vec(),
        SyntaxKind::ObjectLiteralExpression => node.properties().to_vec(),
        _ => Vec::new(),
    }
}

// Go: checker/utilities.go:1229 isInRightSideOfImportOrExportAssignment
// PERF: noderef1 G1. The parents are read through views (`NodeRef`): one
// registry lookup for the node, none for a parent in its store.
pub fn is_in_right_side_of_import_or_export_assignment(node: Node) -> bool {
    is_in_right_side_of_import_or_export_assignment_ref(NodeRef::new(node))
}

/// `is_in_right_side_of_import_or_export_assignment` on the view of the
/// node.
pub fn is_in_right_side_of_import_or_export_assignment_ref(mut node: NodeRef) -> bool {
    let mut parent = node.parent();
    while parent.kind() == SyntaxKind::QualifiedName {
        node = parent;
        parent = node.parent();
    }

    match parent.kind() {
        SyntaxKind::ImportEqualsDeclaration => parent.node().module_reference() == node.node(),
        SyntaxKind::ExportAssignment => parent.expression() == node.node(),
        _ => false,
    }
}

// Go: checker/utilities.go:1238 isJsxIntrinsicTagName
pub fn is_jsx_intrinsic_tag_name(tag_name: Node) -> bool {
    is_identifier(tag_name) && is_intrinsic_jsx_name(&tag_name.text())
        || is_jsx_namespaced_name(tag_name)
}

// Go: checker/utilities.go:1242 getContainingObjectLiteral
pub fn get_containing_object_literal(f: Node) -> Node {
    if (f.kind() == SyntaxKind::MethodDeclaration
        || f.kind() == SyntaxKind::GetAccessor
        || f.kind() == SyntaxKind::SetAccessor)
        && f.parent().kind() == SyntaxKind::ObjectLiteralExpression
    {
        return f.parent();
    } else if f.kind() == SyntaxKind::FunctionExpression
        && f.parent().kind() == SyntaxKind::PropertyAssignment
    {
        return f.parent().parent();
    }
    Node::NIL
}

// Go: checker/utilities.go:1253 isImportTypeQualifierPart
pub fn is_import_type_qualifier_part(node: Node) -> Node {
    is_import_type_qualifier_part_ref(NodeRef::new(node))
}

/// `is_import_type_qualifier_part` on the view of the node.
pub fn is_import_type_qualifier_part_ref(mut node: NodeRef) -> Node {
    let mut parent = node.parent();
    while parent.kind() == SyntaxKind::QualifiedName {
        node = parent;
        parent = parent.parent();
    }

    if parent.is_some()
        && parent.kind() == SyntaxKind::ImportType
        && parent.node().qualifier() == node.node()
    {
        return parent.node();
    }

    Node::NIL
}

// Go: checker/utilities.go:1267 isInNameOfExpressionWithTypeArgumentsOrHeritageTypeReference
pub fn is_in_name_of_expression_with_type_arguments_or_heritage_type_reference(node: Node) -> bool {
    is_in_name_of_expression_with_type_arguments_or_heritage_type_reference_ref(NodeRef::new(node))
}

/// `is_in_name_of_expression_with_type_arguments_or_heritage_type_reference`
/// on the view of the node.
pub fn is_in_name_of_expression_with_type_arguments_or_heritage_type_reference_ref(
    mut node: NodeRef,
) -> bool {
    let mut parent = node.parent();
    while matches!(
        parent.kind(),
        SyntaxKind::PropertyAccessExpression | SyntaxKind::QualifiedName
    ) {
        node = parent;
        parent = node.parent();
    }

    parent.kind() == SyntaxKind::ExpressionWithTypeArguments
        || is_name_of_heritage_clause_type_reference_ref(node)
}

impl Checker {
    // Go: checker/utilities.go:1276 getIndexSymbolFromSymbolTable
    pub fn get_index_symbol_from_symbol_table(&self, symbol_table: SymbolTable) -> SymbolId {
        self.symbols.get(symbol_table, INTERNAL_SYMBOL_NAME_INDEX)
    }
}

// Go: checker/utilities.go:1282 expressionResultIsUnused
// Indicates whether the result of an `Expression` will be unused.
// NOTE: This requires a node with a valid `parent` pointer.
pub fn expression_result_is_unused(mut node: Node) -> bool {
    loop {
        let parent = node.parent();
        // walk up parenthesized expressions, but keep a pointer to the top-most parenthesized expression
        if is_parenthesized_expression(parent) {
            node = parent;
            continue;
        }
        // result is unused in an expression statement, `void` expression, or the initializer or incrementer of a `for` loop
        if is_expression_statement(parent)
            || is_void_expression(parent)
            || is_for_statement(parent)
                && (parent.initializer() == node || parent.incrementor() == node)
        {
            return true;
        }
        if is_binary_expression(parent) && parent.operator_token().kind() == SyntaxKind::CommaToken
        {
            // left side of comma is always unused
            if node == parent.left() {
                return true;
            }
            // right side of comma is unused if parent is unused
            node = parent;
            continue;
        }
        return false;
    }
}

// Go: checker/utilities.go:1307 pseudoBigIntToString
pub fn pseudo_big_int_to_string(value: &PseudoBigInt) -> String {
    value.to_string()
}

// Go: checker/utilities.go:1311 getSuperContainer
// PORT: identical to `ast.GetSuperContainer`, which is already the public
// `get_super_container` in `crate::ast`. This copy stays private to keep the
// prelude glob unambiguous; checker callers resolve to the identical ast
// function.
#[allow(dead_code)]
fn get_super_container(mut node: Node, stop_on_functions: bool) -> Node {
    loop {
        node = node.parent();
        if node.is_nil() {
            return Node::NIL;
        }
        match node.kind() {
            SyntaxKind::ComputedPropertyName => {
                node = node.parent();
            }
            SyntaxKind::FunctionDeclaration
            | SyntaxKind::FunctionExpression
            | SyntaxKind::ArrowFunction => {
                if !stop_on_functions {
                    continue;
                }
                // Go `fallthrough` into the return case.
                return node;
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
                if is_parameter_declaration(node.parent())
                    && is_class_element(node.parent().parent())
                {
                    // If the decorator's parent is a Parameter, we resolve the this container from
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
    }
}

// Go: checker/utilities.go:1343 forEachYieldExpression
// PERF: deferbody1. Go reads `node.Kind` as a field in the switch, in
// `IsFunctionLike` and in `IsPartOfTypeNode`. Here each read was a registry
// lookup, so the walk reads the node's view and kind once and passes them on
// (`is_part_of_type_node_of`). The walk and its order are Go's.
pub fn for_each_yield_expression(body: Node, visitor: &mut dyn FnMut(Node) -> bool) -> bool {
    fn traverse(node: Node, visitor: &mut dyn FnMut(Node) -> bool) -> bool {
        let view = NodeRef::new(node);
        let kind = view.kind();
        match kind {
            SyntaxKind::YieldExpression => {
                if visitor(node) {
                    return true;
                }
                let operand = node.expression();
                if operand.is_nil() {
                    return false;
                }
                return traverse(operand, visitor);
            }
            SyntaxKind::EnumDeclaration
            | SyntaxKind::InterfaceDeclaration
            | SyntaxKind::ModuleDeclaration
            | SyntaxKind::TypeAliasDeclaration => {
                // These are not allowed inside a generator now, but eventually they may be allowed
                // as local types. Regardless, skip them to avoid the work.
            }
            // PERF: deferbody1. A token (identifiers, literals, keywords and
            // punctuation) has no children: Go `ForEachChild` has no case for
            // its kind (ast_generated.go:8693) and returns false, and a token
            // is not function-like. Go gets false through `IsPartOfTypeNode`,
            // which reads the parent of an identifier, and then the empty
            // child walk; here the walk returns false at once. Those reads
            // have no effect.
            _ if kind.is_token() => {}
            _ => {
                // PORT: Go `IsFunctionLike` also tests `node != nil`; the kind
                // read above has already failed for nil, as Go's switch does.
                if is_function_like_kind(kind) {
                    if node.name().is_some() && is_computed_property_name(node.name()) {
                        // Note that we will not include methods/accessors of a class because they would require
                        // first descending into the class. This is by design.
                        return traverse(node.name().expression(), visitor);
                    }
                } else if !is_part_of_type_node_of(view, kind) {
                    // This is the general case, which should include mostly expressions and statements.
                    // Also includes NodeArrays.
                    return node.for_each_child(&mut |child: Node| traverse(child, visitor));
                }
            }
        }
        false
    }
    traverse(body, visitor)
}

// Go: checker/utilities.go:1377 getEnclosingContainer
pub fn get_enclosing_container(node: Node) -> Node {
    find_ancestor(node.parent(), |n: Node| {
        get_container_flags(n).intersects(ContainerFlags::IS_CONTAINER)
    })
}

impl Checker {
    // Go: checker/utilities.go:1383 getDeclarationsOfKind
    pub fn get_declarations_of_kind(&self, symbol: SymbolId, kind: SyntaxKind) -> Vec<Node> {
        self.sym(symbol)
            .declarations
            .iter()
            .copied()
            .filter(|d| d.kind() == kind)
            .collect()
    }
}

// Go: checker/utilities.go:1387 hasType
pub fn has_type(node: Node) -> bool {
    node.type_().is_some()
}

impl Checker {
    // Go: checker/utilities.go:1391 getNonRestParameterCount
    pub fn get_non_rest_parameter_count(&self, sig: SignatureId) -> i32 {
        self.sig(sig).parameters.len() as i32
            - if self.signature_has_rest_parameter(sig) {
                1
            } else {
                0
            }
    }
}

// Go: checker/utilities.go:1395 minAndMax
pub fn min_and_max<T: Copy>(slice: &[T], mut get_value: impl FnMut(T) -> i32) -> (i32, i32) {
    let mut min_value = 0;
    let mut max_value = 0;
    for (i, &element) in slice.iter().enumerate() {
        let value = get_value(element);
        if i == 0 {
            min_value = value;
            max_value = value;
        } else {
            min_value = min_value.min(value);
            max_value = max_value.max(value);
        }
    }
    (min_value, max_value)
}

// Go: checker/utilities.go:1410 FeatureMapEntry
#[derive(Clone, Debug)]
pub struct FeatureMapEntry {
    pub lib: &'static str,
    pub props: Vec<&'static str>,
}

// Go: checker/utilities.go:1415 getFeatureMap
// PORT: Go `sync.OnceValue` becomes a `OnceLock`. The map keys and entry
// order are copied from Go.
pub fn get_feature_map() -> &'static FxHashMap<&'static str, Vec<FeatureMapEntry>> {
    static FEATURE_MAP: std::sync::OnceLock<FxHashMap<&'static str, Vec<FeatureMapEntry>>> =
        std::sync::OnceLock::new();
    FEATURE_MAP.get_or_init(|| {
        fn entry(lib: &'static str, props: &[&'static str]) -> FeatureMapEntry {
            FeatureMapEntry {
                lib,
                props: props.to_vec(),
            }
        }
        let entries: Vec<(&'static str, Vec<FeatureMapEntry>)> = vec![
            (
                "Array",
                vec![
                    entry(
                        "es2015",
                        &[
                            "find",
                            "findIndex",
                            "fill",
                            "copyWithin",
                            "entries",
                            "keys",
                            "values",
                        ],
                    ),
                    entry("es2016", &["includes"]),
                    entry("es2019", &["flat", "flatMap"]),
                    entry("es2022", &["at"]),
                    entry(
                        "es2023",
                        &[
                            "findLastIndex",
                            "findLast",
                            "toReversed",
                            "toSorted",
                            "toSpliced",
                            "with",
                        ],
                    ),
                ],
            ),
            ("Iterator", vec![entry("es2015", &[])]),
            ("IteratorConstructor", vec![entry("es2026", &["concat"])]),
            ("RawJSON", vec![entry("es2026", &[])]),
            ("JSON", vec![entry("es2026", &["isRawJSON", "rawJSON"])]),
            ("AsyncIterator", vec![entry("es2015", &[])]),
            (
                "ArrayBuffer",
                vec![entry(
                    "es2024",
                    &[
                        "maxByteLength",
                        "resizable",
                        "resize",
                        "detached",
                        "transfer",
                        "transferToFixedLength",
                    ],
                )],
            ),
            (
                "Atomics",
                vec![
                    entry(
                        "es2017",
                        &[
                            "add",
                            "and",
                            "compareExchange",
                            "exchange",
                            "isLockFree",
                            "load",
                            "or",
                            "store",
                            "sub",
                            "wait",
                            "notify",
                            "xor",
                        ],
                    ),
                    entry("es2024", &["waitAsync"]),
                ],
            ),
            (
                "SharedArrayBuffer",
                vec![
                    entry("es2017", &["byteLength", "slice"]),
                    entry("es2024", &["growable", "maxByteLength", "grow"]),
                ],
            ),
            ("AsyncIterable", vec![entry("es2018", &[])]),
            ("AsyncIterableIterator", vec![entry("es2018", &[])]),
            ("AsyncGenerator", vec![entry("es2018", &[])]),
            ("AsyncGeneratorFunction", vec![entry("es2018", &[])]),
            (
                "RegExp",
                vec![
                    entry("es2015", &["flags", "sticky", "unicode"]),
                    entry("es2018", &["dotAll"]),
                    entry("es2024", &["unicodeSets"]),
                ],
            ),
            ("RegExpConstructor", vec![entry("es2025", &["escape"])]),
            (
                "Reflect",
                vec![entry(
                    "es2015",
                    &[
                        "apply",
                        "construct",
                        "defineProperty",
                        "deleteProperty",
                        "get",
                        "getOwnPropertyDescriptor",
                        "getPrototypeOf",
                        "has",
                        "isExtensible",
                        "ownKeys",
                        "preventExtensions",
                        "set",
                        "setPrototypeOf",
                    ],
                )],
            ),
            (
                "ArrayConstructor",
                vec![
                    entry("es2015", &["from", "of"]),
                    entry("es2026", &["fromAsync"]),
                ],
            ),
            (
                "ObjectConstructor",
                vec![
                    entry(
                        "es2015",
                        &[
                            "assign",
                            "getOwnPropertySymbols",
                            "keys",
                            "is",
                            "setPrototypeOf",
                        ],
                    ),
                    entry(
                        "es2017",
                        &["values", "entries", "getOwnPropertyDescriptors"],
                    ),
                    entry("es2019", &["fromEntries"]),
                    entry("es2022", &["hasOwn"]),
                    entry("es2024", &["groupBy"]),
                ],
            ),
            (
                "NumberConstructor",
                vec![entry(
                    "es2015",
                    &[
                        "isFinite",
                        "isInteger",
                        "isNaN",
                        "isSafeInteger",
                        "parseFloat",
                        "parseInt",
                    ],
                )],
            ),
            (
                "Math",
                vec![
                    entry(
                        "es2015",
                        &[
                            "clz32", "imul", "sign", "log10", "log2", "log1p", "expm1", "cosh",
                            "sinh", "tanh", "acosh", "asinh", "atanh", "hypot", "trunc", "fround",
                            "cbrt",
                        ],
                    ),
                    entry("es2025", &["f16round"]),
                    entry("es2026", &["sumPrecise"]),
                ],
            ),
            (
                "Map",
                vec![
                    entry("es2015", &["entries", "keys", "values"]),
                    entry("es2026", &["getOrInsert", "getOrInsertComputed"]),
                ],
            ),
            ("MapConstructor", vec![entry("es2024", &["groupBy"])]),
            (
                "Set",
                vec![
                    entry("es2015", &["entries", "keys", "values"]),
                    entry(
                        "es2025",
                        &[
                            "union",
                            "intersection",
                            "difference",
                            "symmetricDifference",
                            "isSubsetOf",
                            "isSupersetOf",
                            "isDisjointFrom",
                        ],
                    ),
                ],
            ),
            (
                "PromiseConstructor",
                vec![
                    entry("es2015", &["all", "race", "reject", "resolve"]),
                    entry("es2020", &["allSettled"]),
                    entry("es2021", &["any"]),
                    entry("es2024", &["withResolvers"]),
                    entry("es2025", &["try"]),
                ],
            ),
            (
                "Symbol",
                vec![
                    entry("es2015", &["for", "keyFor"]),
                    entry("es2019", &["description"]),
                ],
            ),
            (
                "WeakMap",
                vec![
                    entry("es2015", &[]),
                    entry("es2026", &["getOrInsert", "getOrInsertComputed"]),
                ],
            ),
            ("WeakSet", vec![entry("es2015", &[])]),
            (
                "String",
                vec![
                    entry(
                        "es2015",
                        &[
                            "codePointAt",
                            "includes",
                            "endsWith",
                            "normalize",
                            "repeat",
                            "startsWith",
                            "anchor",
                            "big",
                            "blink",
                            "bold",
                            "fixed",
                            "fontcolor",
                            "fontsize",
                            "italics",
                            "link",
                            "small",
                            "strike",
                            "sub",
                            "sup",
                        ],
                    ),
                    entry("es2017", &["padStart", "padEnd"]),
                    entry("es2019", &["trimStart", "trimEnd", "trimLeft", "trimRight"]),
                    entry("es2020", &["matchAll"]),
                    entry("es2021", &["replaceAll"]),
                    entry("es2022", &["at"]),
                    entry("es2024", &["isWellFormed", "toWellFormed"]),
                ],
            ),
            (
                "StringConstructor",
                vec![entry("es2015", &["fromCodePoint", "raw"])],
            ),
            ("DateTimeFormat", vec![entry("es2017", &["formatToParts"])]),
            (
                "Promise",
                vec![entry("es2015", &[]), entry("es2018", &["finally"])],
            ),
            ("RegExpMatchArray", vec![entry("es2018", &["groups"])]),
            ("RegExpExecArray", vec![entry("es2018", &["groups"])]),
            (
                "Intl",
                vec![
                    entry("es2018", &["PluralRules"]),
                    entry("es2020", &["RelativeTimeFormat", "Locale", "DisplayNames"]),
                    entry("es2021", &["ListFormat", "DateTimeFormat"]),
                    entry("es2022", &["Segmenter"]),
                    entry("es2025", &["DurationFormat"]),
                ],
            ),
            ("NumberFormat", vec![entry("es2018", &["formatToParts"])]),
            (
                "SymbolConstructor",
                vec![
                    entry("es2020", &["matchAll"]),
                    entry("esnext", &["metadata", "dispose", "asyncDispose"]),
                ],
            ),
            (
                "DataView",
                vec![
                    entry(
                        "es2020",
                        &["setBigInt64", "setBigUint64", "getBigInt64", "getBigUint64"],
                    ),
                    entry("es2025", &["setFloat16", "getFloat16"]),
                ],
            ),
            ("BigInt", vec![entry("es2020", &[])]),
            (
                "RelativeTimeFormat",
                vec![entry(
                    "es2020",
                    &["format", "formatToParts", "resolvedOptions"],
                )],
            ),
            (
                "Int8Array",
                vec![
                    entry("es2022", &["at"]),
                    entry(
                        "es2023",
                        &[
                            "findLastIndex",
                            "findLast",
                            "toReversed",
                            "toSorted",
                            "toSpliced",
                            "with",
                        ],
                    ),
                ],
            ),
            (
                "Uint8Array",
                vec![
                    entry("es2022", &["at"]),
                    entry(
                        "es2023",
                        &[
                            "findLastIndex",
                            "findLast",
                            "toReversed",
                            "toSorted",
                            "toSpliced",
                            "with",
                        ],
                    ),
                    entry(
                        "es2026",
                        &["toBase64", "setFromBase64", "toHex", "setFromHex"],
                    ),
                ],
            ),
            (
                "Uint8ClampedArray",
                vec![
                    entry("es2022", &["at"]),
                    entry(
                        "es2023",
                        &[
                            "findLastIndex",
                            "findLast",
                            "toReversed",
                            "toSorted",
                            "toSpliced",
                            "with",
                        ],
                    ),
                ],
            ),
            (
                "Int16Array",
                vec![
                    entry("es2022", &["at"]),
                    entry(
                        "es2023",
                        &[
                            "findLastIndex",
                            "findLast",
                            "toReversed",
                            "toSorted",
                            "toSpliced",
                            "with",
                        ],
                    ),
                ],
            ),
            (
                "Uint16Array",
                vec![
                    entry("es2022", &["at"]),
                    entry(
                        "es2023",
                        &[
                            "findLastIndex",
                            "findLast",
                            "toReversed",
                            "toSorted",
                            "toSpliced",
                            "with",
                        ],
                    ),
                ],
            ),
            (
                "Int32Array",
                vec![
                    entry("es2022", &["at"]),
                    entry(
                        "es2023",
                        &[
                            "findLastIndex",
                            "findLast",
                            "toReversed",
                            "toSorted",
                            "toSpliced",
                            "with",
                        ],
                    ),
                ],
            ),
            (
                "Uint32Array",
                vec![
                    entry("es2022", &["at"]),
                    entry(
                        "es2023",
                        &[
                            "findLastIndex",
                            "findLast",
                            "toReversed",
                            "toSorted",
                            "toSpliced",
                            "with",
                        ],
                    ),
                ],
            ),
            ("Float16Array", vec![entry("es2025", &[])]),
            (
                "Float32Array",
                vec![
                    entry("es2022", &["at"]),
                    entry(
                        "es2023",
                        &[
                            "findLastIndex",
                            "findLast",
                            "toReversed",
                            "toSorted",
                            "toSpliced",
                            "with",
                        ],
                    ),
                ],
            ),
            (
                "Float64Array",
                vec![
                    entry("es2022", &["at"]),
                    entry(
                        "es2023",
                        &[
                            "findLastIndex",
                            "findLast",
                            "toReversed",
                            "toSorted",
                            "toSpliced",
                            "with",
                        ],
                    ),
                ],
            ),
            (
                "BigInt64Array",
                vec![
                    entry("es2020", &[]),
                    entry("es2022", &["at"]),
                    entry(
                        "es2023",
                        &[
                            "findLastIndex",
                            "findLast",
                            "toReversed",
                            "toSorted",
                            "toSpliced",
                            "with",
                        ],
                    ),
                ],
            ),
            (
                "BigUint64Array",
                vec![
                    entry("es2020", &[]),
                    entry("es2022", &["at"]),
                    entry(
                        "es2023",
                        &[
                            "findLastIndex",
                            "findLast",
                            "toReversed",
                            "toSorted",
                            "toSpliced",
                            "with",
                        ],
                    ),
                ],
            ),
            ("Error", vec![entry("es2022", &["cause"])]),
            ("ErrorConstructor", vec![entry("es2026", &["isError"])]),
            (
                "Uint8ArrayConstructor",
                vec![entry("es2026", &["fromBase64", "fromHex"])],
            ),
            ("DisposableStack", vec![entry("esnext", &[])]),
            ("AsyncDisposableStack", vec![entry("esnext", &[])]),
            ("Date", vec![entry("esnext", &["toTemporalInstant"])]),
        ];
        entries.into_iter().collect()
    })
}

// Go: checker/utilities.go:1688 rangeOfTypeParameters
pub fn range_of_type_parameters(source_file: Node, type_parameters: NodeList) -> TextRange {
    let text = source_file_text(source_file);
    TextRange::new(
        type_parameters.pos() - 1,
        (text.len() as i32).min(skip_trivia(&text, type_parameters.end()) + 1),
    )
}

// Go: checker/utilities.go:1692 tryGetPropertyAccessOrIdentifierToString
// PORT: Go `entityNameToString` here is the checker package function
// (utilities.go), which is `ast.EntityNameToString(name, scanner.GetTextOfNode)`.
// The ast version is called directly with the same text getter.
pub fn try_get_property_access_or_identifier_to_string(expr: Node) -> String {
    if is_property_access_expression(expr) {
        let base_str = try_get_property_access_or_identifier_to_string(expr.expression());
        if !base_str.is_empty() {
            return base_str
                + "."
                + &crate::ast::entity_name_to_string(expr.name(), Some(&get_text_of_node));
        }
    } else if is_element_access_expression(expr) {
        let base_str = try_get_property_access_or_identifier_to_string(expr.expression());
        if !base_str.is_empty() && is_property_name(expr.argument_expression()) {
            return base_str
                + "."
                + &get_property_name_for_property_name_node(expr.argument_expression());
        }
    } else if is_identifier(expr) {
        return expr.text().to_string();
    } else if is_jsx_namespaced_name(expr) {
        return crate::ast::entity_name_to_string(expr, Some(&get_text_of_node));
    }
    String::new()
}

impl Checker {
    // Go: checker/utilities.go:1712 allDeclarationsInSameSourceFile
    pub fn all_declarations_in_same_source_file(&self, symbol: SymbolId) -> bool {
        let declarations = &self.sym(symbol).declarations;
        if declarations.len() > 1 {
            let mut source_file = Node::NIL;
            for (i, &d) in declarations.iter().enumerate() {
                if i == 0 {
                    source_file = get_source_file_of_node(d);
                } else if get_source_file_of_node(d) != source_file {
                    return false;
                }
            }
        }
        true
    }

    // Go: checker/utilities.go:1726 containsNonMissingUndefinedType
    pub fn contains_non_missing_undefined_type(&self, t: TypeId) -> bool {
        let candidate = if self.ty(t).flags.intersects(TypeFlags::UNION) {
            self.ty(t).types()[0]
        } else {
            t
        };
        self.ty(candidate).flags.intersects(TypeFlags::UNDEFINED) && candidate != self.missing_type
    }
}

// Go: checker/utilities.go:1736 getAnyImportSyntax
pub fn get_any_import_syntax(node: Node) -> Node {
    let import_node = match node.kind() {
        SyntaxKind::ImportEqualsDeclaration => node,
        SyntaxKind::ImportClause => node.parent(),
        SyntaxKind::NamespaceImport => node.parent().parent(),
        SyntaxKind::ImportSpecifier => node.parent().parent().parent(),
        _ => return Node::NIL,
    };
    import_node
}

// Go: checker/utilities.go:1756 isReservedMemberName
// A reserved member name consists of the byte 0xFE (which is an invalid UTF-8 encoding) followed by one or more
// characters where the first character is not '@' or '#'. The '@' character indicates that the name is denoted by
// a well known ES Symbol instance and the '#' character indicates that the name is a PrivateIdentifier.
// PORT: the byte 0xFE is INTERNAL_SYMBOL_NAME_PREFIX in the port form (see
// `ast::misc`), so "name[0]" is that prefix and "name[1]" is the first byte
// after it.
pub fn is_reserved_member_name(name: &str) -> bool {
    match name.strip_prefix(INTERNAL_SYMBOL_NAME_PREFIX) {
        Some(rest) => !rest.is_empty() && rest.as_bytes()[0] != b'@' && rest.as_bytes()[0] != b'#',
        None => false,
    }
}

// Go: checker/utilities.go:1760 introducesArgumentsExoticObject
pub fn introduces_arguments_exotic_object(node: Node) -> bool {
    matches!(
        node.kind(),
        SyntaxKind::MethodDeclaration
            | SyntaxKind::MethodSignature
            | SyntaxKind::Constructor
            | SyntaxKind::GetAccessor
            | SyntaxKind::SetAccessor
            | SyntaxKind::FunctionDeclaration
            | SyntaxKind::FunctionExpression
    )
}

impl Checker {
    // Go: checker/utilities.go:1769 symbolsToArray
    // PORT: Go ranges over a map (random order). The port returns the
    // symbols in table insertion order.
    pub fn symbols_to_array(&self, symbols: SymbolTable) -> Vec<SymbolId> {
        let mut result = Vec::new();
        for (id, symbol) in self.symbols.entries(symbols) {
            if !is_reserved_member_name(&id) {
                result.push(symbol);
            }
        }
        result
    }

    // Go: checker/utilities.go:1779 SkipAlias
    pub fn skip_alias(&mut self, symbol: SymbolId) -> SymbolId {
        if self.sym_flags(symbol).intersects(SymbolFlags::ALIAS) {
            return self.get_aliased_symbol(symbol);
        }
        symbol
    }

    // Go: checker/utilities.go:1787 IsExternalModuleSymbol
    // True if the symbol is for an external module, as opposed to a namespace.
    pub fn is_external_module_symbol(&self, module_symbol: SymbolId) -> bool {
        self.sym(module_symbol).is_external_module()
    }

    // Go: checker/utilities.go:1791 isCanceled
    pub fn is_canceled(&self) -> bool {
        self.ctx.as_ref().is_some_and(|ctx| ctx.err().is_some())
    }

    // Go: checker/utilities.go:1795 checkNotCanceled
    pub fn check_not_canceled(&self) {
        if self.was_canceled {
            panic!("Checker was previously cancelled");
        }
    }

    // Go: checker/utilities.go:1801 getPackagesMap
    // PORT: the Go field is a nil-able map. The Rust field is a plain
    // `FxHashMap`, so an empty map means "not computed yet". An empty computed
    // map is recomputed to the same empty result, so behavior is the same.
    // Go builds the map from the program's resolutions with the same body as
    // `Program.GetPackagesMap`; here it is a copy of the program's map.
    pub fn get_packages_map(&mut self) -> &FxHashMap<String, bool> {
        if self.packages_map.is_empty() {
            self.packages_map = FxHashMap::clone(&crate::program::get_packages_map());
        }
        &self.packages_map
    }

    // Go: checker/utilities.go:1816 typesPackageExists
    pub fn types_package_exists(&mut self, package_name: &str) -> bool {
        let packages_map = self.get_packages_map();
        packages_map.contains_key(&tspath_up2::get_types_package_name(package_name))
    }

    // Go: checker/utilities.go:1822 packageBundlesTypes
    pub fn package_bundles_types(&mut self, package_name: &str) -> bool {
        let packages_map = self.get_packages_map();
        packages_map.get(package_name).copied().unwrap_or(false)
    }
}

// Go: checker/utilities.go:1828 ValueToString
// PORT: Go takes `any` and panics on other types. `LiteralValue` is closed
// over the four handled types, so the panic case cannot happen.
pub fn value_to_string(value: &LiteralValue) -> String {
    match value {
        LiteralValue::String(value) => {
            format!("\"{}\"", escape_string(value, QuoteChar::DOUBLE_QUOTE))
        }
        LiteralValue::Number(value) => value.to_string(),
        LiteralValue::Bool(value) => if *value { "true" } else { "false" }.to_string(),
        LiteralValue::PseudoBigInt(value) => format!("{value}n"),
    }
}

// Go: checker/utilities.go:1842 nodeStartsNewLexicalEnvironment
pub fn node_starts_new_lexical_environment(node: Node) -> bool {
    matches!(
        node.kind(),
        SyntaxKind::Constructor
            | SyntaxKind::FunctionExpression
            | SyntaxKind::FunctionDeclaration
            | SyntaxKind::ArrowFunction
            | SyntaxKind::MethodDeclaration
            | SyntaxKind::GetAccessor
            | SyntaxKind::SetAccessor
            | SyntaxKind::ModuleDeclaration
            | SyntaxKind::SourceFile
    )
}

impl Checker {
    // Go: checker/utilities.go:1856 isUncheckedJSSuggestion
    // Determines whether a did-you-mean error should be a suggestion in an unchecked JS file.
    // Only applies to unchecked JS files without checkJS, // @ts-check or // @ts-nocheck
    // It does not suggest when the suggestion:
    // - Is from a global file that is different from the reference file, or
    // - (optionally) Is a class, or is a this.x property access expression
    pub fn is_unchecked_js_suggestion(
        &self,
        node: Node,
        suggestion: SymbolId,
        exclude_classes: bool,
    ) -> bool {
        let file = get_source_file_of_node(node);
        if file.is_some() {
            let info = source_file_info(file);
            if self.compiler_options.check_js.is_unknown()
                && info.check_js_directive.is_none()
                && (info.script_kind == ScriptKind::JS || info.script_kind == ScriptKind::JSX)
            {
                let mut declaration_file = Node::NIL;
                if suggestion.is_some() {
                    if let Some(&first_declaration) = self.sym(suggestion).declarations.first() {
                        if first_declaration.is_some() {
                            declaration_file = get_source_file_of_node(first_declaration);
                        }
                    }
                }
                let suggestion_has_no_extends_or_decorators = suggestion.is_nil() || {
                    let value_declaration = self.sym(suggestion).value_declaration;
                    value_declaration.is_nil()
                        || !is_class_like(value_declaration)
                        || !get_extends_heritage_clause_elements(value_declaration).is_empty()
                        || class_or_constructor_parameter_is_decorated(false, value_declaration)
                };
                return !(file != declaration_file
                    && declaration_file.is_some()
                    && is_global_source_file(declaration_file))
                    && !(exclude_classes
                        && suggestion.is_some()
                        && self.sym_flags(suggestion).intersects(SymbolFlags::CLASS)
                        && suggestion_has_no_extends_or_decorators)
                    && !(node.is_some()
                        && exclude_classes
                        && is_property_access_expression(node)
                        && node.expression().kind() == SyntaxKind::ThisKeyword
                        && suggestion_has_no_extends_or_decorators);
            }
        }
        false
    }

    // Go: checker/utilities.go:1886 isJSLiteralType
    // Returns if a type is or consists of a JSLiteral object type
    // In addition to objects which are directly literals,
    // * unions where every element is a jsliteral
    // * intersections where at least one element is a jsliteral
    // * and instantiable types constrained to a jsliteral
    // Should all count as literals and not print errors on access or assignment of possibly existing properties.
    // This mirrors the behavior of the index signature propagation, to which this behaves similarly (but doesn't affect assignability or inference).
    pub fn is_js_literal_type(&mut self, t: TypeId) -> bool {
        if self.no_implicit_any {
            return false;
            // Flag is meaningless under `noImplicitAny` mode
        }
        if self.ty(t).object_flags.intersects(ObjectFlags::JS_LITERAL) {
            return true;
        }
        if self.ty(t).flags.intersects(TypeFlags::UNION) {
            let types = self.ty(t).types_list();
            return types.into_iter().all(|u| self.is_js_literal_type(u));
        }
        if self.ty(t).flags.intersects(TypeFlags::INTERSECTION) {
            let types = self.ty(t).types_list();
            return types.into_iter().any(|u| self.is_js_literal_type(u));
        }
        if self.ty(t).flags.intersects(TypeFlags::INSTANTIABLE) {
            let constraint = self.get_resolved_base_constraint(t, &[]);
            return constraint != t && self.is_js_literal_type(constraint);
        }
        false
    }
}

// Go: checker/utilities.go:1909 DiagnosticDetails
// DiagnosticDetails holds a resolved diagnostic message and its arguments,
// used for sharing diagnostic chain computation between the checker and incremental builder.
// PORT: Go `Args []any` holds only strings here, so the port uses
// `Vec<String>`, the argument type of `new_diagnostic_for_node`.
#[derive(Clone, Debug)]
pub struct DiagnosticDetails {
    pub message: &'static crate::diagnostics::Message,
    pub args: Vec<String>,
}

// Go: checker/utilities.go:1918 CreateModuleNotFoundChain
// CreateModuleNotFoundChain computes the diagnostic message and arguments for a module-not-found
// error chain entry. This is shared between the checker (initial diagnostic creation) and the
// incremental builder (repopulation of cached diagnostics).
// Mirrors createModuleNotFoundChain in the TypeScript compiler's utilities.ts.
// PORT: Go `program Program` is the installed `&'static GoProgram`. Its
// methods are the program.rs free functions, so the parameter is unused.
// `program.GetPackagesMap()` is `program::get_packages_map()`.
pub fn create_module_not_found_chain(
    program: &GoProgram,
    file: Node,
    module_reference: &str,
    mode: ResolutionMode,
    package_name: &str,
) -> DiagnosticDetails {
    create_module_not_found_chain_with(
        program,
        file,
        module_reference,
        mode,
        package_name,
        crate::program::get_packages_map,
    )
}

/// `create_module_not_found_chain` that reads the packages map from
/// `packages_map`, called only when the map is needed.
// PERF: chkport1 item 3. The checker passes its own memo
// (`Checker::get_packages_map`, Go `c.packagesMap`). Others pass the
// program's map (`program::get_packages_map`, made once per program).
pub fn create_module_not_found_chain_with<M: std::borrow::Borrow<FxHashMap<String, bool>>>(
    _program: &GoProgram,
    file: Node,
    module_reference: &str,
    mode: ResolutionMode,
    package_name: &str,
    packages_map: impl FnOnce() -> M,
) -> DiagnosticDetails {
    let mut package_name = package_name.to_string();
    let resolved_module = get_resolved_module(file, module_reference, mode);

    if let Some(resolved_module) = resolved_module
        .as_deref()
        .filter(|m| !m.alternate_result.is_empty())
    {
        if resolved_module
            .alternate_result
            .contains("/node_modules/@types/")
        {
            package_name = format!(
                "@types/{}",
                tspath_up2::mangle_scoped_package_name(&package_name)
            );
        }
        return DiagnosticDetails {
            message: diag::There_are_types_at_0_but_this_result_could_not_be_resolved_when_respecting_package_json_exports_The_1_library_may_need_to_update_its_package_json_or_typings,
            args: vec![resolved_module.alternate_result.to_string(), package_name],
        };
    }

    let packages_map = packages_map();
    let packages_map = packages_map.borrow();
    if packages_map.contains_key(&tspath_up2::get_types_package_name(&package_name)) {
        let mangled = tspath_up2::mangle_scoped_package_name(&package_name);
        return DiagnosticDetails {
            message: diag::If_the_0_package_actually_exposes_this_module_consider_sending_a_pull_request_to_amend_https_Colon_Slash_Slashgithub_com_SlashDefinitelyTyped_SlashDefinitelyTyped_Slashtree_Slashmaster_Slashtypes_Slash_1,
            args: vec![package_name, mangled],
        };
    }
    if packages_map.get(&package_name).copied().unwrap_or(false) {
        return DiagnosticDetails {
            message: diag::If_the_0_package_actually_exposes_this_module_try_adding_a_new_declaration_d_ts_file_containing_declare_module_1,
            args: vec![package_name, module_reference.to_string()],
        };
    }
    DiagnosticDetails {
        message: diag::Try_npm_i_save_dev_types_Slash_1_if_it_exists_or_add_a_new_declaration_d_ts_file_containing_declare_module_0,
        args: vec![module_reference.to_string(), tspath_up2::mangle_scoped_package_name(&package_name)],
    }
}

// Go: checker/utilities.go:1954 CreateModeMismatchDetails
// CreateModeMismatchDetails computes the diagnostic message and arguments for a mode-mismatch
// error chain entry. This is shared between the checker (initial diagnostic creation) and the
// incremental builder (repopulation of cached diagnostics).
// Mirrors createModeMismatchDetails in the TypeScript compiler's utilities.ts.
// PORT: `program` is unused for the same reason as in
// `create_module_not_found_chain`.
pub fn create_mode_mismatch_details(_program: &GoProgram, file: Node) -> DiagnosticDetails {
    let ext = tspath_up2::try_get_extension_from_path(source_file_file_name(file));
    let target_ext = if ext == tspath_up2::EXTENSION_TS {
        tspath_up2::EXTENSION_MTS
    } else if ext == tspath_up2::EXTENSION_JS {
        tspath_up2::EXTENSION_MJS
    } else {
        ""
    };
    let meta = get_source_file_meta_data(&source_file_info(file).path);
    let package_json_type = &meta.package_json_type;
    let package_json_directory = &meta.package_json_directory;

    if !package_json_directory.is_empty() && package_json_type.is_empty() {
        if !target_ext.is_empty() {
            return DiagnosticDetails {
                message: diag::To_convert_this_file_to_an_ECMAScript_module_change_its_file_extension_to_0_or_add_the_field_type_Colon_module_to_1,
                args: vec![target_ext.to_string(), crate::frontend::tspath::combine_paths(package_json_directory, &["package.json"])],
            };
        }
        return DiagnosticDetails {
            message: diag::To_convert_this_file_to_an_ECMAScript_module_add_the_field_type_Colon_module_to_0,
            args: vec![crate::frontend::tspath::combine_paths(package_json_directory, &["package.json"])],
        };
    }
    if !target_ext.is_empty() {
        return DiagnosticDetails {
            message: diag::To_convert_this_file_to_an_ECMAScript_module_change_its_file_extension_to_0_or_create_a_local_package_json_file_with_type_Colon_module,
            args: vec![target_ext.to_string()],
        };
    }
    DiagnosticDetails {
        message: diag::To_convert_this_file_to_an_ECMAScript_module_create_a_local_package_json_file_with_type_Colon_module,
        args: Vec::new(),
    }
}

// Go: checker/utilities.go:1985 walkUpOuterExpressions
pub fn walk_up_outer_expressions(node: Node) -> Node {
    let mut parent = node.parent();
    while parent.is_some() && is_outer_expression(parent, OuterExpressionKinds::OEK_ALL) {
        parent = parent.parent();
    }
    parent
}

// Go: checker/utilities.go:1993 GetSetAccessorValueParameter
pub fn get_set_accessor_value_parameter(accessor: Node) -> Node {
    let parameters = accessor.parameters();
    if !parameters.is_empty() {
        let has_this = parameters.len() == 2 && is_this_parameter(parameters.get(0));
        return parameters.get(if has_this { 1 } else { 0 });
    }
    Node::NIL
}

// PORT: private ports of the Go `tspath` extension helpers and the
// `module.MangleScopedPackageName` / `module.GetTypesPackageName` helpers
// used above. No shared Rust port of them exists in this crate.
mod tspath_up2 {
    // Go: tspath/extension.go:9
    pub const EXTENSION_TS: &str = ".ts";
    pub const EXTENSION_TSX: &str = ".tsx";
    pub const EXTENSION_DTS: &str = ".d.ts";
    pub const EXTENSION_JS: &str = ".js";
    pub const EXTENSION_JSX: &str = ".jsx";
    pub const EXTENSION_JSON: &str = ".json";
    pub const EXTENSION_MJS: &str = ".mjs";
    pub const EXTENSION_MTS: &str = ".mts";
    pub const EXTENSION_DMTS: &str = ".d.mts";
    pub const EXTENSION_CJS: &str = ".cjs";
    pub const EXTENSION_CTS: &str = ".cts";
    pub const EXTENSION_DCTS: &str = ".d.cts";

    // Go: tspath/extension.go:43 extensionsToRemove
    const EXTENSIONS_TO_REMOVE: [&str; 12] = [
        EXTENSION_DTS,
        EXTENSION_DMTS,
        EXTENSION_DCTS,
        EXTENSION_MJS,
        EXTENSION_MTS,
        EXTENSION_CJS,
        EXTENSION_CTS,
        EXTENSION_TS,
        EXTENSION_JS,
        EXTENSION_TSX,
        EXTENSION_JSX,
        EXTENSION_JSON,
    ];

    // Go: tspath/path.go:1142 FileExtensionIs
    fn file_extension_is(path: &str, extension: &str) -> bool {
        path.len() > extension.len() && path.ends_with(extension)
    }

    // Go: tspath/extension.go:66 TryGetExtensionFromPath
    pub fn try_get_extension_from_path(p: &str) -> &'static str {
        for ext in EXTENSIONS_TO_REMOVE {
            if file_extension_is(p, ext) {
                return ext;
            }
        }
        ""
    }

    // Go: module/util.go:65 MangleScopedPackageName
    pub fn mangle_scoped_package_name(package_name: &str) -> String {
        if package_name.as_bytes().first() == Some(&b'@') {
            let Some(idx) = package_name.find('/') else {
                return package_name.to_string();
            };
            return format!("{}__{}", &package_name[1..idx], &package_name[idx + 1..]);
        }
        package_name.to_string()
    }

    // Go: module/util.go:84 GetTypesPackageName
    pub fn get_types_package_name(package_name: &str) -> String {
        format!("@types/{}", mangle_scoped_package_name(package_name))
    }
}
