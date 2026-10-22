//! Port of typescript-go `internal/ast/utilities.go` lines 906-1805.

use crate::astdata::NodeData;
use crate::prelude::*;

// Go: ast/utilities.go:906 SetImportsOfSourceFile
// PORT: skipped. Go documents it as "should never be called outside the
// parser". Parser data (`imports`) is fixed in `SourceFileInfo` when the
// program is installed, so there is nothing to set here.

// Go: ast/utilities.go:911 FindAncestor
/// Walks up the parents of a node to find the ancestor that matches the callback.
pub fn find_ancestor(mut node: Node, mut callback: impl FnMut(Node) -> bool) -> Node {
    while node.is_some() {
        if callback(node) {
            return node;
        }
        node = node.parent();
    }
    Node::NIL
}

/// `find_ancestor` whose callback gets the view of each node (`NodeRef`):
/// a parent in the same store, and the kind and parent reads of the
/// callback, cost no registry lookup. Same nodes, same order.
// PERF: checkid1. For the hot walks of the identifier and property access
// checks, which go to the root or near it.
#[inline]
pub fn find_ancestor_ref(node: Node, mut callback: impl FnMut(NodeRef) -> bool) -> Node {
    let mut node = NodeRef::new(node);
    while node.is_some() {
        if callback(node) {
            return node.node();
        }
        node = node.parent();
    }
    Node::NIL
}

// Go: ast/utilities.go:921 FindManyAncestors
/// Walks up the parents of `node` once. Slot `i` of the result is the nearest
/// ancestor that `callbacks[i]` matches, or nil. A node fills at most one
/// slot: the first callback that matches it and whose slot is still empty.
#[must_use]
pub fn find_many_ancestors(mut node: Node, callbacks: &[fn(Node) -> bool]) -> Vec<Node> {
    let mut ancestors = vec![Node::NIL; callbacks.len()];
    let mut found = 0;
    while node.is_some() {
        for (i, callback) in callbacks.iter().enumerate() {
            if ancestors[i].is_nil() && callback(node) {
                ancestors[i] = node;
                found += 1;
                if found == callbacks.len() {
                    return ancestors;
                }
                break;
            }
        }
        node = node.parent();
    }
    ancestors
}

/// `find_ancestor(node, |n| callback(n, n.kind()))`: the callback also gets
/// the node's Go `Kind`.
// PERF: U4 (CH7). The steps inside a published store read its records,
// found once (`frozen_find_ancestor`), not through the registry lookup of
// every `kind()` and `parent()` read. Same nodes, same order.
pub fn find_ancestor_with_kind(
    mut node: Node,
    mut callback: impl FnMut(Node, SyntaxKind) -> bool,
) -> Node {
    loop {
        match frozen_find_ancestor(node, &mut callback) {
            Some(AncestorWalk::Found(found)) => return found,
            Some(AncestorWalk::Next(next)) => node = next,
            None => {
                if node.is_nil() {
                    return Node::NIL;
                }
                if callback(node, node.kind()) {
                    return node;
                }
                node = node.parent();
            }
        }
    }
}

// Go: ast/utilities.go:941 FindAncestorKind
/// Walks up the parents of a node to find the ancestor that matches the kind.
// PERF: noderef1 G2. Through views (`NodeRef`): no registry lookup for a
// parent in the same store.
pub fn find_ancestor_kind(node: Node, kind: SyntaxKind) -> Node {
    let mut node = NodeRef::new(node);
    while node.is_some() {
        if node.kind() == kind {
            return node.node();
        }
        node = node.parent();
    }
    Node::NIL
}

// Go: ast/utilities.go:959 ToFindAncestorResult
pub fn to_find_ancestor_result(b: bool) -> FindAncestorResult {
    if b {
        return FindAncestorResult::FIND_ANCESTOR_TRUE;
    }
    FindAncestorResult::FIND_ANCESTOR_FALSE
}

// Go: ast/utilities.go:967 FindAncestorOrQuit
/// Walks up the parents of a node to find the ancestor that matches the callback.
pub fn find_ancestor_or_quit(
    mut node: Node,
    mut callback: impl FnMut(Node) -> FindAncestorResult,
) -> Node {
    while node.is_some() {
        let result = callback(node);
        if result == FindAncestorResult::FIND_ANCESTOR_QUIT {
            return Node::NIL;
        } else if result == FindAncestorResult::FIND_ANCESTOR_TRUE {
            return node;
        }
        node = node.parent();
    }
    Node::NIL
}

/// `find_ancestor_or_quit` whose callback gets the view of each node
/// (`NodeRef`), as `find_ancestor_ref`.
#[inline]
pub fn find_ancestor_or_quit_ref(
    node: Node,
    mut callback: impl FnMut(NodeRef) -> FindAncestorResult,
) -> Node {
    let mut node = NodeRef::new(node);
    while node.is_some() {
        let result = callback(node);
        if result == FindAncestorResult::FIND_ANCESTOR_QUIT {
            return Node::NIL;
        } else if result == FindAncestorResult::FIND_ANCESTOR_TRUE {
            return node.node();
        }
        node = node.parent();
    }
    Node::NIL
}

// Go: ast/utilities.go:980 IsNodeDescendantOf
// PERF: checkid1. Parent steps through views, as `find_ancestor_ref`.
pub fn is_node_descendant_of(node: Node, ancestor: Node) -> bool {
    let mut node = NodeRef::new(node);
    while node.is_some() {
        if node.node() == ancestor {
            return true;
        }
        node = node.parent();
    }
    false
}

// Go: ast/utilities.go:990 ModifierToFlag
pub fn modifier_to_flag(token: SyntaxKind) -> ModifierFlags {
    match token {
        SyntaxKind::StaticKeyword => ModifierFlags::STATIC,
        SyntaxKind::PublicKeyword => ModifierFlags::PUBLIC,
        SyntaxKind::ProtectedKeyword => ModifierFlags::PROTECTED,
        SyntaxKind::PrivateKeyword => ModifierFlags::PRIVATE,
        SyntaxKind::AbstractKeyword => ModifierFlags::ABSTRACT,
        SyntaxKind::AccessorKeyword => ModifierFlags::ACCESSOR,
        SyntaxKind::ExportKeyword => ModifierFlags::EXPORT,
        SyntaxKind::DeclareKeyword => ModifierFlags::AMBIENT,
        SyntaxKind::ConstKeyword => ModifierFlags::CONST,
        SyntaxKind::DefaultKeyword => ModifierFlags::DEFAULT,
        SyntaxKind::AsyncKeyword => ModifierFlags::ASYNC,
        SyntaxKind::ReadonlyKeyword => ModifierFlags::READONLY,
        SyntaxKind::OverrideKeyword => ModifierFlags::OVERRIDE,
        SyntaxKind::InKeyword => ModifierFlags::IN,
        SyntaxKind::OutKeyword => ModifierFlags::OUT,
        SyntaxKind::Decorator => ModifierFlags::DECORATOR,
        _ => ModifierFlags::NONE,
    }
}

// Go: ast/utilities.go:1028 ModifiersToFlags
pub fn modifiers_to_flags(modifiers: &[Node]) -> ModifierFlags {
    let mut flags = ModifierFlags::NONE;
    for modifier in modifiers {
        flags |= modifier_to_flag(modifier.kind());
    }
    flags
}

// Go: ast/utilities.go:1036 HasSyntacticModifier
pub fn has_syntactic_modifier(node: Node, flags: ModifierFlags) -> bool {
    node.modifier_flags().intersects(flags)
}

/// `has_syntactic_modifier` on `d`, the data of `node` that the caller
/// already loaded with `parsed_node_data` (query Q7-3, see `Node::modifiers_in`).
// PERF: U4 (bind A). A published store node reads the U1 (b) modifier column
// (`frozen_store_modifier_flags`), as `Node::modifier_flags` does, not its
// modifier list.
pub fn has_syntactic_modifier_in(node: Node, d: LoadedData, flags: ModifierFlags) -> bool {
    if let Some(modifier_flags) = frozen_store_modifier_flags(node) {
        debug_assert_eq!(modifier_flags, node.modifiers_in(d).modifier_flags());
        return modifier_flags.intersects(flags);
    }
    node.modifiers_in(d).modifier_flags().intersects(flags)
}

// Go: ast/utilities.go:1040 HasAccessorModifier
pub fn has_accessor_modifier(node: Node) -> bool {
    has_syntactic_modifier(node, ModifierFlags::ACCESSOR)
}

// Go: ast/utilities.go:1044 HasStaticModifier
pub fn has_static_modifier(node: Node) -> bool {
    has_syntactic_modifier(node, ModifierFlags::STATIC)
}

// Go: ast/utilities.go:1048 IsStatic
pub fn is_static(node: Node) -> bool {
    // https://tc39.es/ecma262/#sec-static-semantics-isstatic
    is_class_element(node) && has_static_modifier(node) || is_class_static_block_declaration(node)
}

// Go: ast/utilities.go:1053 CanHaveSymbol
pub fn can_have_symbol(node: Node) -> bool {
    matches!(
        node.kind(),
        SyntaxKind::ArrowFunction
            | SyntaxKind::BinaryExpression
            | SyntaxKind::BindingElement
            | SyntaxKind::CallExpression
            | SyntaxKind::CallSignature
            | SyntaxKind::ClassDeclaration
            | SyntaxKind::ClassExpression
            | SyntaxKind::ClassStaticBlockDeclaration
            | SyntaxKind::Constructor
            | SyntaxKind::ConstructorType
            | SyntaxKind::ConstructSignature
            | SyntaxKind::ElementAccessExpression
            | SyntaxKind::EnumDeclaration
            | SyntaxKind::EnumMember
            | SyntaxKind::ExportAssignment
            | SyntaxKind::ExportDeclaration
            | SyntaxKind::ExportSpecifier
            | SyntaxKind::FunctionDeclaration
            | SyntaxKind::FunctionExpression
            | SyntaxKind::FunctionType
            | SyntaxKind::GetAccessor
            | SyntaxKind::ImportClause
            | SyntaxKind::ImportEqualsDeclaration
            | SyntaxKind::ImportSpecifier
            | SyntaxKind::IndexSignature
            | SyntaxKind::InterfaceDeclaration
            | SyntaxKind::JsTypeAliasDeclaration
            | SyntaxKind::JsxAttribute
            | SyntaxKind::JsxAttributes
            | SyntaxKind::JsxSpreadAttribute
            | SyntaxKind::MappedType
            | SyntaxKind::MethodDeclaration
            | SyntaxKind::MethodSignature
            | SyntaxKind::ModuleDeclaration
            | SyntaxKind::NamedTupleMember
            | SyntaxKind::NamespaceExport
            | SyntaxKind::NamespaceExportDeclaration
            | SyntaxKind::NamespaceImport
            | SyntaxKind::NewExpression
            | SyntaxKind::NoSubstitutionTemplateLiteral
            | SyntaxKind::NumericLiteral
            | SyntaxKind::ObjectLiteralExpression
            | SyntaxKind::Parameter
            | SyntaxKind::PropertyAccessExpression
            | SyntaxKind::PropertyAssignment
            | SyntaxKind::PropertyDeclaration
            | SyntaxKind::PropertySignature
            | SyntaxKind::SetAccessor
            | SyntaxKind::ShorthandPropertyAssignment
            | SyntaxKind::SourceFile
            | SyntaxKind::SpreadAssignment
            | SyntaxKind::StringLiteral
            | SyntaxKind::TypeAliasDeclaration
            | SyntaxKind::TypeLiteral
            | SyntaxKind::TypeParameter
            | SyntaxKind::VariableDeclaration
    )
}

// Go: ast/utilities.go:1072 CanHaveIllegalDecorators
pub fn can_have_illegal_decorators(node: Node) -> bool {
    matches!(
        node.kind(),
        SyntaxKind::PropertyAssignment
            | SyntaxKind::ShorthandPropertyAssignment
            | SyntaxKind::FunctionDeclaration
            | SyntaxKind::Constructor
            | SyntaxKind::IndexSignature
            | SyntaxKind::ClassStaticBlockDeclaration
            | SyntaxKind::MissingDeclaration
            | SyntaxKind::VariableStatement
            | SyntaxKind::InterfaceDeclaration
            | SyntaxKind::TypeAliasDeclaration
            | SyntaxKind::EnumDeclaration
            | SyntaxKind::ModuleDeclaration
            | SyntaxKind::ImportEqualsDeclaration
            | SyntaxKind::ImportDeclaration
            | SyntaxKind::JsImportDeclaration
            | SyntaxKind::NamespaceExportDeclaration
            | SyntaxKind::ExportDeclaration
            | SyntaxKind::ExportAssignment
    )
}

// Go: ast/utilities.go:1088 CanHaveIllegalModifiers
pub fn can_have_illegal_modifiers(node: Node) -> bool {
    matches!(
        node.kind(),
        SyntaxKind::ClassStaticBlockDeclaration
            | SyntaxKind::PropertyAssignment
            | SyntaxKind::ShorthandPropertyAssignment
            | SyntaxKind::MissingDeclaration
            | SyntaxKind::NamespaceExportDeclaration
    )
}

// Go: ast/utilities.go:1100 CanHaveModifiers
pub fn can_have_modifiers(node: Node) -> bool {
    matches!(
        node.kind(),
        SyntaxKind::TypeParameter
            | SyntaxKind::Parameter
            | SyntaxKind::PropertySignature
            | SyntaxKind::PropertyDeclaration
            | SyntaxKind::MethodSignature
            | SyntaxKind::MethodDeclaration
            | SyntaxKind::Constructor
            | SyntaxKind::GetAccessor
            | SyntaxKind::SetAccessor
            | SyntaxKind::IndexSignature
            | SyntaxKind::ConstructorType
            | SyntaxKind::FunctionExpression
            | SyntaxKind::ArrowFunction
            | SyntaxKind::ClassExpression
            | SyntaxKind::VariableStatement
            | SyntaxKind::FunctionDeclaration
            | SyntaxKind::ClassDeclaration
            | SyntaxKind::InterfaceDeclaration
            | SyntaxKind::TypeAliasDeclaration
            | SyntaxKind::EnumDeclaration
            | SyntaxKind::ModuleDeclaration
            | SyntaxKind::ImportEqualsDeclaration
            | SyntaxKind::ImportDeclaration
            | SyntaxKind::JsImportDeclaration
            | SyntaxKind::ExportAssignment
            | SyntaxKind::ExportDeclaration
    )
}

// Go: ast/utilities.go:1133 CanHaveDecorators
pub fn can_have_decorators(node: Node) -> bool {
    matches!(
        node.kind(),
        SyntaxKind::Parameter
            | SyntaxKind::PropertyDeclaration
            | SyntaxKind::MethodDeclaration
            | SyntaxKind::GetAccessor
            | SyntaxKind::SetAccessor
            | SyntaxKind::ClassExpression
            | SyntaxKind::ClassDeclaration
    )
}

// Go: ast/utilities.go:1147 IsFunctionOrModuleBlock
pub fn is_function_or_module_block(node: Node) -> bool {
    is_source_file(node)
        || is_module_block(node)
        || is_block(node) && is_function_like(node.parent())
}

// Go: ast/utilities.go:1151 IsFunctionExpressionOrArrowFunction
pub fn is_function_expression_or_arrow_function(node: Node) -> bool {
    is_function_expression(node) || is_arrow_function(node)
}

// Go: ast/utilities.go:1157 ForEachReturnStatement
/// Warning: This has the same semantics as the forEach family of functions in
/// that traversal terminates in the event that 'visitor' returns true.
pub fn for_each_return_statement(body: Node, mut visitor: impl FnMut(Node) -> bool) -> bool {
    fn traverse(node: Node, visitor: &mut dyn FnMut(Node) -> bool) -> bool {
        match node.kind() {
            SyntaxKind::ReturnStatement => visitor(node),
            SyntaxKind::CaseBlock
            | SyntaxKind::Block
            | SyntaxKind::IfStatement
            | SyntaxKind::DoStatement
            | SyntaxKind::WhileStatement
            | SyntaxKind::ForStatement
            | SyntaxKind::ForInStatement
            | SyntaxKind::ForOfStatement
            | SyntaxKind::WithStatement
            | SyntaxKind::SwitchStatement
            | SyntaxKind::CaseClause
            | SyntaxKind::DefaultClause
            | SyntaxKind::LabeledStatement
            | SyntaxKind::TryStatement
            | SyntaxKind::CatchClause => {
                node.for_each_child(&mut |child: Node| traverse(child, visitor))
            }
            _ => false,
        }
    }
    traverse(body, &mut visitor)
}

// Go: ast/utilities.go:1173 GetRootDeclaration
pub fn get_root_declaration(mut node: Node) -> Node {
    while node.kind() == SyntaxKind::BindingElement {
        node = node.parent().parent();
    }
    node
}

// Go: ast/utilities.go:1180 GetCombinedModifierFlags
// PERF: `get_root_declaration` is inlined here and each node kind is read
// once (it was read in `get_root_declaration` and again for the
// VariableDeclaration test, each read with the store bounds checks). The
// walk and the flag reads are the same as Go. A nil node fails the Go
// `node != nil` tests, so it returns early. The same holds for
// `get_combined_node_flags` and `get_combined_parser_flags`.
// PERF: checkid1. The nodes, kinds, parents and modifier flags are read
// through views (`NodeRef`): a parent in the same store costs no registry
// lookup. The same holds for `get_combined_parser_flags`.
pub fn get_combined_modifier_flags(node: Node) -> ModifierFlags {
    let mut node = NodeRef::new(node);
    let mut kind = node.kind();
    while kind == SyntaxKind::BindingElement {
        node = node.parent().parent();
        kind = node.kind();
    }
    let mut flags = node.modifier_flags();
    if kind == SyntaxKind::VariableDeclaration {
        node = node.parent();
        if node.is_nil() {
            return flags;
        }
        kind = node.kind();
    }
    if kind == SyntaxKind::VariableDeclarationList {
        flags |= node.modifier_flags();
        node = node.parent();
        if node.is_nil() {
            return flags;
        }
        kind = node.kind();
    }
    if kind == SyntaxKind::VariableStatement {
        flags |= node.modifier_flags();
    }
    flags
}

// Go: ast/utilities.go:1196 GetCombinedNodeFlags
pub fn get_combined_node_flags(mut node: Node) -> NodeFlags {
    let mut kind = node.kind();
    while kind == SyntaxKind::BindingElement {
        node = node.parent().parent();
        kind = node.kind();
    }
    let mut flags = node.flags();
    if kind == SyntaxKind::VariableDeclaration {
        node = node.parent();
        if node.is_nil() {
            return flags;
        }
        kind = node.kind();
    }
    if kind == SyntaxKind::VariableDeclarationList {
        flags |= node.flags();
        node = node.parent();
        if node.is_nil() {
            return flags;
        }
        kind = node.kind();
    }
    if kind == SyntaxKind::VariableStatement {
        flags |= node.flags();
    }
    flags
}

/// Go `GetCombinedNodeFlags(node) & mask` for a `mask` without a binder bit
/// (see `Node::parser_flags`).
// PERF: U4 (CH7). The same walk as `get_combined_node_flags`, without the
// binder data of each node. `(a | b | c) & mask` is
// `(a & mask) | (b & mask) | (c & mask)`.
pub fn get_combined_parser_flags(node: Node, mask: NodeFlags) -> NodeFlags {
    let mut node = NodeRef::new(node);
    let mut kind = node.kind();
    while kind == SyntaxKind::BindingElement {
        node = node.parent().parent();
        kind = node.kind();
    }
    let mut flags = node.parser_flags(mask);
    if kind == SyntaxKind::VariableDeclaration {
        node = node.parent();
        if node.is_nil() {
            return flags;
        }
        kind = node.kind();
    }
    if kind == SyntaxKind::VariableDeclarationList {
        flags |= node.parser_flags(mask);
        node = node.parent();
        if node.is_nil() {
            return flags;
        }
        kind = node.kind();
    }
    if kind == SyntaxKind::VariableStatement {
        flags |= node.parser_flags(mask);
    }
    flags
}

// Go: ast/utilities.go:1213 IsVarAwaitUsing
/// Gets whether a bound `VariableDeclaration` or `VariableDeclarationList` is part of an `await using` declaration.
pub fn is_var_await_using(node: Node) -> bool {
    get_combined_parser_flags(node, NodeFlags::BLOCK_SCOPED) == NodeFlags::AWAIT_USING
}

// Go: ast/utilities.go:1218 IsVarUsing
/// Gets whether a bound `VariableDeclaration` or `VariableDeclarationList` is part of a `using` declaration.
pub fn is_var_using(node: Node) -> bool {
    get_combined_parser_flags(node, NodeFlags::BLOCK_SCOPED) == NodeFlags::USING
}

// Go: ast/utilities.go:1223 GetJSDocDeprecatedTag
/// Returns the first @deprecated JSDoc tag for the given node, or nil if none exists.
pub fn get_js_doc_deprecated_tag(node: Node) -> Node {
    for jsdoc in node.js_doc(Node::NIL).to_vec() {
        let tags = jsdoc.tags();
        if !tags.is_nil() {
            for tag in tags.nodes().to_vec() {
                if is_js_doc_deprecated_tag(tag) {
                    return tag;
                }
            }
        }
    }
    Node::NIL
}

// Go: ast/utilities.go:1240 IsDeprecatedDeclaration
/// Reports whether the given declaration is marked as @deprecated.
/// It checks NodeFlagsPossiblyContainsDeprecatedTag on combined node flags, then confirms
/// by walking up to find the node with the flag and performing a JSDoc lookup.
// PERF: U4 (CH7). A node of a store without the flag is not deprecated
// (`frozen_store_lacks_deprecated_tag`). Otherwise only the flag bit of the
// combined flags is read, which is a parser bit.
pub fn is_deprecated_declaration(declaration: Node) -> bool {
    if frozen_store_lacks_deprecated_tag(declaration) {
        debug_assert!(
            !get_combined_node_flags(declaration)
                .intersects(NodeFlags::POSSIBLY_CONTAINS_DEPRECATED_TAG),
            "assertion failed: !get_combined_node_flags(declaration).intersects(NodeFlags::POSSIBLY_CONTAINS_DEPRECATED_TAG)"
        );
        return false;
    }
    let combined =
        get_combined_parser_flags(declaration, NodeFlags::POSSIBLY_CONTAINS_DEPRECATED_TAG);
    is_deprecated_declaration_with_cached_flags(declaration, combined)
}

// Go: ast/utilities.go:1246 IsDeprecatedDeclarationWithCachedFlags
/// The core logic for IsDeprecatedDeclaration, parameterized on pre-computed
/// combined flags so the checker can supply cached flags.
pub fn is_deprecated_declaration_with_cached_flags(
    declaration: Node,
    combined_flags: NodeFlags,
) -> bool {
    if !combined_flags.intersects(NodeFlags::POSSIBLY_CONTAINS_DEPRECATED_TAG) {
        return false;
    }
    // Walk up to find the node that directly has the flag, since JSDoc is
    // attached to that node (e.g. VariableStatement, not VariableDeclaration).
    let mut n = declaration;
    while n.is_some() {
        if !n
            .parser_flags(NodeFlags::POSSIBLY_CONTAINS_DEPRECATED_TAG)
            .is_empty()
        {
            return get_js_doc_deprecated_tag(n).is_some();
        }
        n = n.parent();
    }
    false
}

// Go: ast/utilities.go:1261 IsVarConst
/// Gets whether a bound `VariableDeclaration` or `VariableDeclarationList` is part of a `const` declaration.
pub fn is_var_const(node: Node) -> bool {
    get_combined_parser_flags(node, NodeFlags::BLOCK_SCOPED) == NodeFlags::CONST
}

// Go: ast/utilities.go:1266 IsVarConstLike
/// Gets whether a bound `VariableDeclaration` or `VariableDeclarationList` is part of a `const`, `using` or `await using` declaration.
pub fn is_var_const_like(node: Node) -> bool {
    let flags = get_combined_parser_flags(node, NodeFlags::BLOCK_SCOPED);
    flags == NodeFlags::CONST || flags == NodeFlags::USING || flags == NodeFlags::AWAIT_USING
}

// Go: ast/utilities.go:1275 IsVarLet
/// Gets whether a bound `VariableDeclaration` or `VariableDeclarationList` is part of a `let` declaration.
pub fn is_var_let(node: Node) -> bool {
    get_combined_parser_flags(node, NodeFlags::BLOCK_SCOPED) == NodeFlags::LET
}

// Go: ast/utilities.go:1279 IsImportMeta
pub fn is_import_meta(node: Node) -> bool {
    if node.kind() == SyntaxKind::MetaProperty {
        return node.keyword_token() == SyntaxKind::ImportKeyword && node.name().text() == "meta";
    }
    false
}

// Go: ast/utilities.go:1286 WalkUpBindingElementsAndPatterns
pub fn walk_up_binding_elements_and_patterns(binding: Node) -> Node {
    let mut node = binding.parent();
    while is_binding_element(node.parent()) {
        node = node.parent().parent();
    }
    node.parent()
}

// Go: ast/utilities.go:1294 IsSourceFileJS
pub fn is_source_file_js(file: Node) -> bool {
    let script_kind = with_source_file_info(file, |info| info.script_kind);
    script_kind == ScriptKind::JS || script_kind == ScriptKind::JSX
}

// Go: ast/utilities.go:1298 IsInJSFile
// PERF: U4 (CH7). `JAVA_SCRIPT_FILE` is a parser bit (`Node::parser_flags`).
pub fn is_in_js_file(node: Node) -> bool {
    node.is_some() && !node.parser_flags(NodeFlags::JAVA_SCRIPT_FILE).is_empty()
}

// Go: ast/utilities.go:1302 IsDeclaration
pub fn is_declaration(node: Node) -> bool {
    is_declaration_ref(NodeRef::new(node))
}

/// `is_declaration` on the view of the node.
// PERF: noderef1 G1. The predicates of the declaration-name family read
// the kind, the parent and the data of a node through one view
// (`NodeRef`), with one registry lookup per node.
#[inline]
pub fn is_declaration_ref(node: NodeRef) -> bool {
    if node.kind() == SyntaxKind::TypeParameter {
        return node.parent().is_some();
    }
    is_declaration_node_ref(node)
}

// Go: ast/utilities.go:1310 IsDeclarationName
/// True if `name` is the name of a declaration node.
pub fn is_declaration_name(name: Node) -> bool {
    is_declaration_name_ref(NodeRef::new(name))
}

/// `is_declaration_name` on the view of the name.
pub fn is_declaration_name_ref(name: NodeRef) -> bool {
    !matches!(
        name.kind(),
        SyntaxKind::SourceFile | SyntaxKind::ObjectBindingPattern | SyntaxKind::ArrayBindingPattern
    ) && {
        let parent = name.parent();
        is_declaration_ref(parent) && parent.name() == name.node()
    }
}

// Go: ast/utilities.go:1315 IsDeclarationNameOrImportPropertyName
/// Like 'isDeclarationName', but returns true for LHS of `import { x as y }` or `export { x as y }`.
pub fn is_declaration_name_or_import_property_name(name: Node) -> bool {
    is_declaration_name_or_import_property_name_ref(NodeRef::new(name))
}

/// `is_declaration_name_or_import_property_name` on the view of the name.
pub fn is_declaration_name_or_import_property_name_ref(name: NodeRef) -> bool {
    match name.parent().kind() {
        SyntaxKind::ImportSpecifier | SyntaxKind::ExportSpecifier => {
            matches!(
                name.kind(),
                SyntaxKind::Identifier | SyntaxKind::StringLiteral
            )
        }
        _ => is_declaration_name_ref(name),
    }
}

// Go: ast/utilities.go:1324 IsLiteralComputedPropertyDeclarationName
pub fn is_literal_computed_property_declaration_name(node: Node) -> bool {
    is_literal_computed_property_declaration_name_ref(NodeRef::new(node))
}

/// `is_literal_computed_property_declaration_name` on the view of the
/// node.
pub fn is_literal_computed_property_declaration_name_ref(node: NodeRef) -> bool {
    matches!(
        node.kind(),
        SyntaxKind::StringLiteral
            | SyntaxKind::NoSubstitutionTemplateLiteral
            | SyntaxKind::NumericLiteral
    ) && {
        let parent = node.parent();
        parent.kind() == SyntaxKind::ComputedPropertyName && is_declaration_ref(parent.parent())
    }
}

// Go: ast/utilities.go:1330 IsExternalModuleImportEqualsDeclaration
pub fn is_external_module_import_equals_declaration(node: Node) -> bool {
    node.kind() == SyntaxKind::ImportEqualsDeclaration
        && node.module_reference().kind() == SyntaxKind::ExternalModuleReference
}

// Go: ast/utilities.go:1334 IsModuleOrEnumDeclaration
pub fn is_module_or_enum_declaration(node: Node) -> bool {
    node.kind() == SyntaxKind::ModuleDeclaration || node.kind() == SyntaxKind::EnumDeclaration
}

// Go: ast/utilities.go:1338 IsLiteralImportTypeNode
pub fn is_literal_import_type_node(node: Node) -> bool {
    is_import_type_node(node)
        && is_literal_type_node(node.argument())
        && is_string_literal(node.argument().literal())
}

// Go: ast/utilities.go:1342 IsJsxTagName
pub fn is_jsx_tag_name(node: Node) -> bool {
    is_jsx_tag_name_of(node, NodeRef::new(node).parent())
}

/// `is_jsx_tag_name(node)`, where `parent` is the view of the parent of
/// `node`.
#[inline]
pub fn is_jsx_tag_name_of(node: Node, parent: NodeRef) -> bool {
    match parent.kind() {
        SyntaxKind::JsxOpeningElement
        | SyntaxKind::JsxClosingElement
        | SyntaxKind::JsxSelfClosingElement => parent.node().tag_name() == node,
        _ => false,
    }
}

// Go: ast/utilities.go:1351 IsImportOrExportSpecifier
pub fn is_import_or_export_specifier(node: Node) -> bool {
    is_import_specifier(node) || is_export_specifier(node)
}

// Go: ast/utilities.go:1355 IsVoidZero
pub fn is_void_zero(node: Node) -> bool {
    is_void_expression(node)
        && is_numeric_literal(node.expression())
        && node.expression().text() == "0"
}

/// `exports` and `module`, interned once for `is_exports_identifier` and
/// `is_module_identifier`.
static EXPORTS_NAME: std::sync::LazyLock<Name> = std::sync::LazyLock::new(|| Name::from("exports"));
static MODULE_NAME: std::sync::LazyLock<Name> = std::sync::LazyLock::new(|| Name::from("module"));

// Go: ast/utilities.go:1359 IsExportsIdentifier
// PERF: compares name ids (`Node::text_is`), with no text load.
pub fn is_exports_identifier(node: Node) -> bool {
    is_identifier(node) && node.text_is(&EXPORTS_NAME)
}

// Go: ast/utilities.go:1363 IsModuleIdentifier
// PERF: compares name ids (`Node::text_is`), with no text load.
// `is_module_exports_access_expression` calls this first, also for TS
// files (`check_testing_known_truthy_types`).
pub fn is_module_identifier(node: Node) -> bool {
    is_identifier(node) && node.text_is(&MODULE_NAME)
}

/// `this`, interned once for `is_this_identifier`.
static THIS_NAME: std::sync::LazyLock<Name> = std::sync::LazyLock::new(|| Name::from("this"));

// Go: ast/utilities.go:1367 IsThisIdentifier
// PERF: U1 (a). Compares name ids (`Node::text_is`), with no text load.
// `is_this_in_type_query` calls this first.
pub fn is_this_identifier(node: Node) -> bool {
    is_identifier(node) && node.text_is(&THIS_NAME)
}

// Go: ast/utilities.go:1371 IsThisParameter
pub fn is_this_parameter(node: Node) -> bool {
    is_parameter_declaration(node) && node.name().is_some() && is_this_identifier(node.name())
}

// Go: ast/utilities.go:1375 IsBindableStaticAccessExpression
pub fn is_bindable_static_access_expression(node: Node, exclude_this_keyword: bool) -> bool {
    is_property_access_expression(node)
        && (!exclude_this_keyword && node.expression().kind() == SyntaxKind::ThisKeyword
            || is_identifier(node.name())
                && is_bindable_static_name_expression(
                    node.expression(),
                    true, /*excludeThisKeyword*/
                ))
        || is_bindable_static_element_access_expression(node, exclude_this_keyword)
}

// Go: ast/utilities.go:1381 IsBindableStaticElementAccessExpression
pub fn is_bindable_static_element_access_expression(
    node: Node,
    exclude_this_keyword: bool,
) -> bool {
    is_literal_like_element_access(node)
        && ((!exclude_this_keyword && node.expression().kind() == SyntaxKind::ThisKeyword)
            || is_entity_name_expression(node.expression())
            || is_bindable_static_access_expression(
                node.expression(),
                true, /*excludeThisKeyword*/
            ))
}

// Go: ast/utilities.go:1388 IsPrototypeAccess
pub fn is_prototype_access(node: Node) -> bool {
    if is_bindable_static_access_expression(node, false /*excludeThisKeyword*/) {
        let name = get_element_or_property_access_name(node);
        if name.is_some() {
            return name.text() == "prototype";
        }
    }
    false
}

// Go: ast/utilities.go:1397 IsLiteralLikeElementAccess
pub fn is_literal_like_element_access(node: Node) -> bool {
    is_element_access_expression(node)
        && is_string_or_numeric_literal_like(node.argument_expression())
}

// Go: ast/utilities.go:1401 IsBindableStaticNameExpression
pub fn is_bindable_static_name_expression(node: Node, exclude_this_keyword: bool) -> bool {
    is_entity_name_expression(node)
        || is_bindable_static_access_expression(node, exclude_this_keyword)
}

// Go: ast/utilities.go:1407 GetElementOrPropertyAccessName
/// Does not handle signed numeric names like `a[+0]` - handling those would require handling prefix unary expressions
/// throughout late binding handling as well, which is awkward (but ultimately probably doable if there is demand)
pub fn get_element_or_property_access_name(node: Node) -> Node {
    match node.kind() {
        SyntaxKind::PropertyAccessExpression => {
            if is_identifier(node.name()) {
                return node.name();
            }
            return Node::NIL;
        }
        SyntaxKind::ElementAccessExpression => {
            let arg = skip_parentheses(node.argument_expression());
            if is_string_or_numeric_literal_like(arg) {
                return arg;
            }
            return Node::NIL;
        }
        _ => {}
    }
    panic!("Unhandled case in GetElementOrPropertyAccessName")
}

// Go: ast/utilities.go:1423 GetInitializerOfBinaryExpression
/// PORT: Go takes `*BinaryExpression`; this takes the BinaryExpression node.
pub fn get_initializer_of_binary_expression(expr: Node) -> Node {
    let mut expr = expr;
    while is_binary_expression(expr.right()) {
        expr = expr.right();
    }
    expr.right().expression()
}

// Go: ast/utilities.go:1430 IsExpressionWithTypeArgumentsInClassExtendsClause
pub fn is_expression_with_type_arguments_in_class_extends_clause(node: Node) -> bool {
    try_get_class_extending_expression_with_type_arguments(node).is_some()
}

// Go: ast/utilities.go:1434 TryGetClassExtendingExpressionWithTypeArguments
pub fn try_get_class_extending_expression_with_type_arguments(node: Node) -> Node {
    if !is_expression_with_type_arguments(node) {
        return Node::NIL;
    }
    let (cls, is_implements) =
        try_get_class_implementing_or_extending_heritage_clause_element(node);
    if cls.is_some() && !is_implements {
        return cls;
    }
    Node::NIL
}

// Go: ast/utilities.go:1445 TryGetClassImplementingOrExtendingHeritageClauseElement
/// Returns `(class, isImplements)`.
pub fn try_get_class_implementing_or_extending_heritage_clause_element(node: Node) -> (Node, bool) {
    if (is_expression_with_type_arguments(node) || is_type_reference_node(node))
        && is_heritage_clause(node.parent())
        && is_class_like(node.parent().parent())
    {
        return (
            node.parent().parent(),
            node.parent().token() == SyntaxKind::ImplementsKeyword,
        );
    }
    (Node::NIL, false)
}

// Go: ast/utilities.go:1453 GetNameOfDeclaration
pub fn get_name_of_declaration(declaration: Node) -> Node {
    if declaration.is_nil() {
        return Node::NIL;
    }
    name_of_declaration(declaration, || declaration.name())
}

/// `get_name_of_declaration` on `d`, the data of the non-nil `declaration`
/// that the caller already loaded with `parsed_node_data` (query Q7-3, see
/// `Node::name_in`).
pub fn get_name_of_declaration_in(declaration: Node, d: LoadedData) -> Node {
    name_of_declaration(declaration, || declaration.name_in(d))
}

/// The body of `get_name_of_declaration` for a non-nil declaration.
/// `read_name` reads `declaration.name()`, so the `_in` variant can read it
/// from loaded data.
fn name_of_declaration(declaration: Node, read_name: impl FnOnce() -> Node) -> Node {
    let non_assigned_name = non_assigned_name_of_declaration(declaration, read_name);
    if non_assigned_name.is_some() {
        return non_assigned_name;
    }
    if is_function_expression(declaration)
        || is_arrow_function(declaration)
        || is_class_expression(declaration)
    {
        return get_assigned_name(declaration);
    }
    Node::NIL
}

// Go: ast/utilities.go:1467 GetNonAssignedNameOfDeclaration
pub fn get_non_assigned_name_of_declaration(declaration: Node) -> Node {
    non_assigned_name_of_declaration(declaration, || declaration.name())
}

/// The body of `get_non_assigned_name_of_declaration`. `read_name` reads
/// `declaration.name()` (see `name_of_declaration`).
fn non_assigned_name_of_declaration(declaration: Node, read_name: impl FnOnce() -> Node) -> Node {
    // !!!
    match declaration.kind() {
        SyntaxKind::BinaryExpression | SyntaxKind::CallExpression => {
            let kind = get_assignment_declaration_kind(declaration);
            if kind == JSDeclarationKind::PROPERTY
                || kind == JSDeclarationKind::THIS_PROPERTY
                || kind == JSDeclarationKind::EXPORTS_PROPERTY
            {
                let left = declaration.left();
                let name = get_element_or_property_access_name(left);
                if name.is_some() {
                    return name;
                }
                return left;
            } else if kind == JSDeclarationKind::OBJECT_DEFINE_PROPERTY_VALUE
                || kind == JSDeclarationKind::OBJECT_DEFINE_PROPERTY_EXPORTS
            {
                return declaration.arguments().get(1);
            }
            return Node::NIL;
        }
        SyntaxKind::ExportAssignment => {
            let expr = declaration.expression();
            if is_identifier(expr) {
                return expr;
            }
            return Node::NIL;
        }
        _ => {}
    }
    read_name()
}

// Go: ast/utilities.go:1492 GetAssignedName
pub fn get_assigned_name(node: Node) -> Node {
    let parent = node.parent();
    if parent.is_some() {
        match parent.kind() {
            SyntaxKind::PropertyAssignment => {
                return parent.name();
            }
            SyntaxKind::BindingElement => {
                return parent.name();
            }
            SyntaxKind::BinaryExpression => {
                if node == parent.right() {
                    let left = parent.left();
                    match left.kind() {
                        SyntaxKind::Identifier => {
                            return left;
                        }
                        SyntaxKind::PropertyAccessExpression => {
                            return left.name();
                        }
                        SyntaxKind::ElementAccessExpression => {
                            let arg = skip_parentheses(left.argument_expression());
                            if is_string_or_numeric_literal_like(arg) {
                                return arg;
                            }
                        }
                        _ => {}
                    }
                }
            }
            SyntaxKind::VariableDeclaration => {
                let name = parent.name();
                if is_identifier(name) {
                    return name;
                }
            }
            _ => {}
        }
    }
    Node::NIL
}

// Go: ast/utilities.go:1547 GetAssignmentDeclarationKind
pub fn get_assignment_declaration_kind(node: Node) -> JSDeclarationKind {
    match node.kind() {
        SyntaxKind::BinaryExpression => {
            return get_binary_assignment_declaration_kind(&BinaryView::new(node));
        }
        SyntaxKind::CallExpression => {
            if is_in_js_file(node) && is_bindable_object_define_property_call(node) {
                let entity_name = node.arguments().get(0);
                if is_exports_identifier(entity_name)
                    || is_module_exports_access_expression(entity_name)
                {
                    return JSDeclarationKind::OBJECT_DEFINE_PROPERTY_EXPORTS;
                }
                return JSDeclarationKind::OBJECT_DEFINE_PROPERTY_VALUE;
            }
        }
        _ => {}
    }
    JSDeclarationKind::NONE
}

/// `get_assignment_declaration_kind` of a BinaryExpression, on its fields
/// that the caller already read (`BinaryView`): Go's `KindBinaryExpression`
/// case.
// PERF: binderview1. Every test is pure, so the kind, the expression and
// the JS file bit of `bin.Left` are read once each, and the expression's
// kind once for the identifier and access tests on it.
pub fn get_binary_assignment_declaration_kind(bin: &BinaryView) -> JSDeclarationKind {
    if bin.operator != SyntaxKind::EqualsToken {
        return JSDeclarationKind::NONE;
    }
    let left = bin.left;
    let left_kind = left.kind();
    // Go `IsAccessExpression(bin.Left)`.
    if !matches!(
        left_kind,
        SyntaxKind::PropertyAccessExpression | SyntaxKind::ElementAccessExpression
    ) {
        return JSDeclarationKind::NONE;
    }
    let in_js = is_in_js_file(left);
    let expression = left.expression();
    let expression_kind = expression.kind();
    let expression_is_identifier = expression_kind == SyntaxKind::Identifier;
    if in_js {
        // Go `IsModuleExportsAccessExpression(bin.Left)` on an access
        // expression: its expression is the `module` identifier and its
        // name is `exports`.
        if expression_is_identifier
            && expression.text_is(&MODULE_NAME)
            && is_exports_access_name(left)
            && !is_exports_identifier(bin.right)
        {
            return JSDeclarationKind::MODULE_EXPORTS;
        }
        // Go `IsModuleExportsAccessExpression(e) || IsExportsIdentifier(e)`
        // of `e = bin.Left.Expression()`.
        let module_exports_or_exports = if expression_is_identifier {
            expression.text_is(&EXPORTS_NAME)
        } else {
            matches!(
                expression_kind,
                SyntaxKind::PropertyAccessExpression | SyntaxKind::ElementAccessExpression
            ) && is_module_exports_access_expression(expression)
        };
        if module_exports_or_exports && get_element_or_property_access_name(left).is_some() {
            return JSDeclarationKind::EXPORTS_PROPERTY;
        }
        if expression_kind == SyntaxKind::ThisKeyword {
            return JSDeclarationKind::THIS_PROPERTY;
        }
    }
    // Go `IsEntityNameExpressionEx(bin.Left.Expression(), IsInJSFile(bin.Left))`,
    // whose first test is `IsIdentifier`.
    let entity_name = expression_is_identifier || is_entity_name_expression_ex(expression, in_js);
    if left_kind == SyntaxKind::PropertyAccessExpression
        && entity_name
        && is_identifier(left.name())
        || left_kind == SyntaxKind::ElementAccessExpression && entity_name
    {
        return JSDeclarationKind::PROPERTY;
    }
    JSDeclarationKind::NONE
}

/// The tail of Go `IsModuleExportsAccessExpression(node)` for an access
/// expression `node`: its access name is `exports`.
fn is_exports_access_name(node: Node) -> bool {
    let name = get_element_or_property_access_name(node);
    name.is_some() && name.text() == "exports"
}

// Go: ast/utilities.go:1581 IsBindableObjectDefinePropertyCall
pub fn is_bindable_object_define_property_call(node: Node) -> bool {
    let args = node.arguments();
    if args.len() == 3 {
        let expr = node.expression();
        if is_property_access_expression(expr)
            && is_identifier(expr.expression())
            && expr.expression().text() == "Object"
            && expr.name().text() == "defineProperty"
            && is_string_or_numeric_literal_like(args.get(1))
            && is_bindable_static_name_expression(args.get(0), true /*excludeThisKeyword*/)
        {
            return true;
        }
    }
    false
}

// Go: ast/utilities.go:1602 HasDynamicName
/// A declaration has a dynamic name if all of the following are true:
///   1. The declaration has a computed property name.
///   2. The computed name is *not* expressed as a StringLiteral.
///   3. The computed name is *not* expressed as a NumericLiteral.
///   4. The computed name is *not* expressed as a PlusToken or MinusToken
///      immediately followed by a NumericLiteral.
pub fn has_dynamic_name(declaration: Node) -> bool {
    let name = get_name_of_declaration(declaration);
    name.is_some() && is_dynamic_name(name)
}

/// `has_dynamic_name` on `d`, the data of the non-nil `declaration` that
/// the caller already loaded with `parsed_node_data` (query Q7-3).
pub fn has_dynamic_name_in(declaration: Node, d: LoadedData) -> bool {
    let name = get_name_of_declaration_in(declaration, d);
    name.is_some() && is_dynamic_name(name)
}

// Go: ast/utilities.go:1607 IsDynamicName
pub fn is_dynamic_name(name: Node) -> bool {
    let expr = match name.kind() {
        SyntaxKind::ComputedPropertyName => name.expression(),
        SyntaxKind::ElementAccessExpression => skip_parentheses(name.argument_expression()),
        _ => return false,
    };
    !is_string_or_numeric_literal_like(expr) && !is_signed_numeric_literal(expr)
}

// Go: ast/utilities.go:1620 IsEntityNameExpression
pub fn is_entity_name_expression(node: Node) -> bool {
    is_entity_name_expression_ex(node, false /*allowJS*/)
}

// Go: ast/utilities.go:1624 IsEntityNameExpressionEx
pub fn is_entity_name_expression_ex(node: Node, allow_js: bool) -> bool {
    is_identifier(node)
        || is_property_access_entity_name_expression(node, allow_js)
        || allow_js
            && (node.kind() == SyntaxKind::ThisKeyword
                || is_element_access_entity_name_expression(node, allow_js))
}

// Go: ast/utilities.go:1630 IsPropertyAccessEntityNameExpression
pub fn is_property_access_entity_name_expression(node: Node, allow_js: bool) -> bool {
    is_property_access_expression(node)
        && is_identifier(node.name())
        && is_entity_name_expression_ex(node.expression(), allow_js)
}

// Go: ast/utilities.go:1634 isElementAccessEntityNameExpression
pub fn is_element_access_entity_name_expression(node: Node, allow_js: bool) -> bool {
    is_element_access_expression(node)
        && is_string_or_numeric_literal_like(node.argument_expression())
        && is_entity_name_expression_ex(node.expression(), allow_js)
}

// Go: ast/utilities.go:1638 IsDottedName
pub fn is_dotted_name(node: Node) -> bool {
    match node.kind() {
        SyntaxKind::Identifier
        | SyntaxKind::ThisKeyword
        | SyntaxKind::SuperKeyword
        | SyntaxKind::MetaProperty => true,
        SyntaxKind::PropertyAccessExpression | SyntaxKind::ParenthesizedExpression => {
            is_dotted_name(node.expression())
        }
        _ => false,
    }
}

// Go: ast/utilities.go:1648 HasSamePropertyAccessName
pub fn has_same_property_access_name(node1: Node, node2: Node) -> bool {
    if node1.kind() == SyntaxKind::Identifier && node2.kind() == SyntaxKind::Identifier {
        return node1.text() == node2.text();
    } else if node1.kind() == SyntaxKind::PropertyAccessExpression
        && node2.kind() == SyntaxKind::PropertyAccessExpression
    {
        return node1.name().text() == node2.name().text()
            && has_same_property_access_name(node1.expression(), node2.expression());
    }
    false
}

// Go: ast/utilities.go:1658 IsAmbientModule
pub fn is_ambient_module(node: Node) -> bool {
    is_module_declaration(node)
        && (node.name().kind() == SyntaxKind::StringLiteral || is_global_scope_augmentation(node))
}

// Go: ast/utilities.go:1662 IsAmbientModuleSymbolName
pub fn is_ambient_module_symbol_name(s: &str) -> bool {
    try_get_ambient_module_name_from_symbol_name(s).is_some()
}

// Go: ast/utilities.go:1669 TryGetAmbientModuleNameFromSymbolName (ts#63931)
// Ambient module symbols are either of the form `"modulename"` or `InternalSymbolNamePrefix + "\"modulename\"pattern@nodeId"`;
// see `getDeclarationName`.
// PORT: `(string, bool)` returns `Option<&str>`. `s` is the port form of the
// Go name (see `INTERNAL_SYMBOL_NAME_PREFIX`); the marker index is only
// compared with 1, and the part before it is empty in both forms together.
pub fn try_get_ambient_module_name_from_symbol_name(s: &str) -> Option<&str> {
    if s.starts_with('"') && s.ends_with('"') {
        return Some(&s[1..s.len() - 1]);
    }

    // patternPrefix := InternalSymbolNamePrefix + "\""
    let rest = s
        .strip_prefix(INTERNAL_SYMBOL_NAME_PREFIX)?
        .strip_prefix('"')?;
    let marker_index = rest.rfind("\"pattern@")?;
    if marker_index < 1 {
        return None;
    }
    Some(&rest[..marker_index])
}

// Go: ast/utilities.go:1686 IsExternalModule
/// PORT: Go takes `*SourceFile`; this takes the SourceFile node.
pub fn is_external_module(file: Node) -> bool {
    with_source_file_info(file, |info| info.external_module_indicator.is_some())
}

// Go: ast/utilities.go:1690 IsExternalOrCommonJSModule
pub fn is_external_or_common_js_module(file: Node) -> bool {
    with_source_file_info(file, |info| {
        info.external_module_indicator.is_some() || info.common_js_module_indicator.is_some()
    })
}

// Go: ast/utilities.go:1695 IsEffectiveExternalModule
// TODO(Go): Should we deprecate `IsExternalOrCommonJSModule` in favor of this function?
pub fn is_effective_external_module(node: Node, compiler_options: &CompilerOptions) -> bool {
    is_external_module(node)
        || (is_common_js_containing_module_kind(compiler_options.get_emit_module_kind())
            && with_source_file_info(node, |info| info.common_js_module_indicator.is_some()))
}

// Go: ast/utilities.go:1699 isCommonJSContainingModuleKind
pub fn is_common_js_containing_module_kind(kind: ModuleKind) -> bool {
    kind == ModuleKind::COMMON_JS || ModuleKind::NODE16 <= kind && kind <= ModuleKind::NODE_NEXT
}

// Go: ast/utilities.go:1703 IsExternalModuleIndicator
pub fn is_external_module_indicator(node: Node) -> bool {
    // Exported top-level member indicates moduleness
    is_any_import_or_re_export(node)
        || is_export_assignment(node)
        || has_syntactic_modifier(node, ModifierFlags::EXPORT)
}

// Go: ast/utilities.go:1708 IsExportNamespaceAsDefaultDeclaration
pub fn is_export_namespace_as_default_declaration(node: Node) -> bool {
    if is_export_declaration(node) {
        let export_clause = node.export_clause();
        return is_namespace_export(export_clause)
            && module_export_name_is_default(export_clause.name());
    }
    false
}

// Go: ast/utilities.go:1716 IsGlobalScopeAugmentation
pub fn is_global_scope_augmentation(node: Node) -> bool {
    is_module_declaration(node) && node.keyword() == SyntaxKind::GlobalKeyword
}

// Go: ast/utilities.go:1720 IsModuleAugmentationExternal
pub fn is_module_augmentation_external(node: Node) -> bool {
    // external module augmentation is a ambient module declaration that is either:
    // - defined in the top level scope and source file is an external module
    // - defined inside ambient module declaration located in the top level scope and source file not an external module
    match node.parent().kind() {
        SyntaxKind::SourceFile => is_external_module(node.parent()),
        SyntaxKind::ModuleBlock => {
            let grand_parent = node.parent().parent();
            is_ambient_module(grand_parent)
                && is_source_file(grand_parent.parent())
                && !is_external_module(grand_parent.parent())
        }
        _ => false,
    }
}

// Go: ast/utilities.go:1734 IsModuleWithStringLiteralName
pub fn is_module_with_string_literal_name(node: Node) -> bool {
    is_module_declaration(node) && node.name().kind() == SyntaxKind::StringLiteral
}

// Go: ast/utilities.go:1738 GetContainingClass
pub fn get_containing_class(node: Node) -> Node {
    find_ancestor(node.parent(), is_class_like)
}

// Go: ast/utilities.go:1742 GetExtendsHeritageClauseElements
pub fn get_extends_heritage_clause_elements(node: Node) -> Vec<Node> {
    get_heritage_elements(node, SyntaxKind::ExtendsKeyword)
}

// Go: ast/utilities.go:1746 GetImplementsHeritageClauseElements
pub fn get_implements_heritage_clause_elements(node: Node) -> Vec<Node> {
    get_heritage_elements(node, SyntaxKind::ImplementsKeyword)
}

// Go: ast/utilities.go:1750 GetHeritageElements
/// Go returns `[]*HeritageClauseElement`: ExpressionWithTypeArguments or
/// TypeReference nodes (tsgo#4797).
pub fn get_heritage_elements(node: Node, kind: SyntaxKind) -> Vec<Node> {
    let clause = get_heritage_clause(node, kind);
    if clause.is_some() {
        return clause.types().nodes().to_vec();
    }
    Vec::new()
}

// Go: ast/utilities.go:1759 GetHeritageClauseElementName
/// GetHeritageClauseElementName returns the expression or type name of a heritage clause element.
pub fn get_heritage_clause_element_name(node: Node) -> Node {
    if is_type_reference_node(node) {
        return node.type_name();
    }
    node.expression()
}

// Go: ast/utilities.go:1766 IsNameOfHeritageClauseTypeReference
pub fn is_name_of_heritage_clause_type_reference(node: Node) -> bool {
    is_name_of_heritage_clause_type_reference_ref(NodeRef::new(node))
}

/// `is_name_of_heritage_clause_type_reference` on the view of the node.
pub fn is_name_of_heritage_clause_type_reference_ref(mut node: NodeRef) -> bool {
    let mut parent = node.parent();
    while parent.kind() == SyntaxKind::QualifiedName {
        node = parent;
        parent = node.parent();
    }
    parent.kind() == SyntaxKind::TypeReference
        && parent.type_name() == node.node()
        && parent.parent().kind() == SyntaxKind::HeritageClause
}

// Go: ast/utilities.go:1775 GetHeritageClause
pub fn get_heritage_clause(node: Node, kind: SyntaxKind) -> Node {
    let clauses = get_heritage_clauses(node);
    if !clauses.is_nil() {
        for clause in clauses.nodes().to_vec() {
            if clause.token() == kind {
                return clause;
            }
        }
    }
    Node::NIL
}

// Go: ast/utilities.go:1787 getHeritageClauses
pub fn get_heritage_clauses(node: Node) -> NodeList {
    match node.kind() {
        SyntaxKind::ClassDeclaration
        | SyntaxKind::ClassExpression
        | SyntaxKind::InterfaceDeclaration => node.heritage_clauses(),
        _ => NodeList::NIL,
    }
}

// Go: ast/utilities.go:1799 IsPartOfTypeQuery
pub fn is_part_of_type_query(mut node: Node) -> bool {
    while node.kind() == SyntaxKind::QualifiedName || node.kind() == SyntaxKind::Identifier {
        node = node.parent();
    }
    node.kind() == SyntaxKind::TypeQuery
}

// Go: ast/utilities.go:1813 IsPartOfParameterDeclaration
/// This function returns true if the this node's root declaration is a parameter.
/// For example, passing a `ParameterDeclaration` will return true, as will passing a
/// binding element that is a child of a `ParameterDeclaration`.
///
/// If you are looking to test that a `Node` is a `ParameterDeclaration`, use `isParameter`.
pub fn is_part_of_parameter_declaration(node: Node) -> bool {
    get_root_declaration(node).kind() == SyntaxKind::Parameter
}

// Go: ast/utilities.go:1817 IsInTopLevelContext
pub fn is_in_top_level_context(mut node: Node) -> bool {
    // The name of a class or function declaration is a BindingIdentifier in its surrounding scope.
    if is_identifier(node) {
        let parent = node.parent();
        if (is_class_declaration(parent) || is_function_declaration(parent))
            && parent.name() == node
        {
            node = parent;
        }
    }
    let container = get_this_container(
        node, true,  /*includeArrowFunctions*/
        false, /*includeClassComputedPropertyName*/
    );
    is_source_file(container)
}

// Go: ast/utilities.go:1829 GetThisContainer
pub fn get_this_container(
    mut node: Node,
    include_arrow_functions: bool,
    include_class_computed_property_name: bool,
) -> Node {
    loop {
        node = node.parent();
        if node.is_nil() {
            panic!("nil parent in getThisContainer");
        }
        match node.kind() {
            SyntaxKind::ComputedPropertyName => {
                if include_class_computed_property_name && is_class_like(node.parent().parent()) {
                    return node;
                }
                node = node.parent().parent();
            }
            SyntaxKind::Decorator => {
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
            SyntaxKind::ArrowFunction => {
                if include_arrow_functions {
                    return node;
                }
            }
            SyntaxKind::FunctionDeclaration
            | SyntaxKind::FunctionExpression
            | SyntaxKind::ModuleDeclaration
            | SyntaxKind::ClassStaticBlockDeclaration
            | SyntaxKind::PropertyDeclaration
            | SyntaxKind::PropertySignature
            | SyntaxKind::MethodDeclaration
            | SyntaxKind::MethodSignature
            | SyntaxKind::Constructor
            | SyntaxKind::GetAccessor
            | SyntaxKind::SetAccessor
            | SyntaxKind::CallSignature
            | SyntaxKind::ConstructSignature
            | SyntaxKind::IndexSignature
            | SyntaxKind::EnumDeclaration
            | SyntaxKind::SourceFile => {
                return node;
            }
            _ => {}
        }
    }
}
