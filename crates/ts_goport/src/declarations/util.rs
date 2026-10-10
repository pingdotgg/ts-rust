//! Port of `transformers/declarations/util.go`.
//!
//! These helpers are used by the declaration transformer body. Most of that
//! body is not ported yet, so some are not called yet.
#![allow(dead_code)]

use crate::prelude::*;
use crate::printer::{EmitContext, EmitResolver};

// Go: transformers/declarations/util.go:9 needsScopeMarker
pub(crate) fn needs_scope_marker(result: Node) -> bool {
    !is_any_import_or_re_export(result)
        && !is_export_assignment(result)
        && !has_syntactic_modifier(result, ModifierFlags::EXPORT)
        && !is_ambient_module(result)
}

// Go: transformers/declarations/util.go:13 canHaveLiteralInitializer
// ts#64649: takes the emit resolver, not the host.
pub(crate) fn can_have_literal_initializer(resolver: &dyn EmitResolver, node: Node) -> bool {
    match node.kind() {
        SyntaxKind::PropertyDeclaration | SyntaxKind::PropertySignature => resolver
            .get_effective_declaration_flags(node, ModifierFlags::PRIVATE)
            .is_empty(),
        SyntaxKind::Parameter | SyntaxKind::VariableDeclaration => true,
        _ => false,
    }
}

// Go: transformers/declarations/util.go:25 canProduceDiagnostics
// PERF: Go chains 23 `ast.Is*` tests. Each one is a pure kind compare, so one
// kind read and one match give the same result. The arms keep the Go order.
pub(crate) fn can_produce_diagnostics(node: Node) -> bool {
    can_produce_diagnostics_kind(node.kind())
}

/// `can_produce_diagnostics` of a node of kind `kind`.
pub(crate) fn can_produce_diagnostics_kind(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        SyntaxKind::VariableDeclaration
            | SyntaxKind::PropertyDeclaration
            | SyntaxKind::PropertySignature
            | SyntaxKind::BindingElement
            | SyntaxKind::SetAccessor
            | SyntaxKind::GetAccessor
            | SyntaxKind::ConstructSignature
            | SyntaxKind::CallSignature
            | SyntaxKind::MethodDeclaration
            | SyntaxKind::MethodSignature
            | SyntaxKind::FunctionDeclaration
            | SyntaxKind::Parameter
            | SyntaxKind::TypeParameter
            | SyntaxKind::ExpressionWithTypeArguments
            | SyntaxKind::ImportEqualsDeclaration
            | SyntaxKind::TypeAliasDeclaration
            | SyntaxKind::JsTypeAliasDeclaration
            | SyntaxKind::Constructor
            | SyntaxKind::IndexSignature
            | SyntaxKind::PropertyAccessExpression
            | SyntaxKind::ElementAccessExpression
            | SyntaxKind::BinaryExpression
            | SyntaxKind::CallExpression // !!! TODO: JSDoc support (ast.IsJSDocTypeAlias)
    )
}

// Go: transformers/declarations/util.go:52 canReuseModifierNodes
pub(crate) fn can_reuse_modifier_nodes(nodes: &[Node]) -> bool {
    for &node in nodes {
        if is_modifier(node) && node.flags().intersects(NodeFlags::REPARSED) {
            return false;
        }
    }
    true
}

// Go: transformers/declarations/util.go:61 isDeclarationAndNotVisible
pub(crate) fn is_declaration_and_not_visible(
    emit_context: &EmitContext,
    resolver: &dyn EmitResolver,
    node: Node,
) -> bool {
    let node = emit_context.parse_node(node);
    match node.kind() {
        SyntaxKind::FunctionDeclaration
        | SyntaxKind::ModuleDeclaration
        | SyntaxKind::InterfaceDeclaration
        | SyntaxKind::ClassDeclaration
        | SyntaxKind::TypeAliasDeclaration
        | SyntaxKind::JsTypeAliasDeclaration
        | SyntaxKind::EnumDeclaration => !resolver.is_declaration_visible(node),
        // The following should be doing their own visibility checks based on filtering their members
        SyntaxKind::VariableDeclaration => !get_binding_name_visible(resolver, node),
        SyntaxKind::ImportEqualsDeclaration
        | SyntaxKind::ImportDeclaration
        | SyntaxKind::JsImportDeclaration
        | SyntaxKind::ExportDeclaration
        | SyntaxKind::ExportAssignment => false,
        SyntaxKind::ClassStaticBlockDeclaration => true,
        _ => false,
    }
}

// Go: transformers/declarations/util.go:87 getBindingNameVisible
pub(crate) fn get_binding_name_visible(resolver: &dyn EmitResolver, elem: Node) -> bool {
    if is_omitted_expression(elem) {
        return false;
    }
    // TODO: parseArrayBindingElement _never_ parses out an OmittedExpression anymore, instead producing a nameless binding element
    // Audit if OmittedExpression should be removed
    if elem.name().is_nil() {
        return false;
    }
    if is_binding_pattern(elem.name()) {
        // If any child binding pattern element has been marked visible (usually by collect linked aliases), then this is visible
        for elem in elem.name().elements().iter() {
            if get_binding_name_visible(resolver, elem) {
                return true;
            }
        }
        false
    } else {
        resolver.is_declaration_visible(elem)
    }
}

// Go: transformers/declarations/util.go:109 isEnclosingDeclaration
pub(crate) fn is_enclosing_declaration(node: Node) -> bool {
    is_enclosing_declaration_kind(node.kind())
}

/// `is_enclosing_declaration` of a node of kind `kind`.
// PERF: emitast2. Each test of Go `isEnclosingDeclaration` is a kind test, so
// the kind is read once.
pub(crate) fn is_enclosing_declaration_kind(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        SyntaxKind::SourceFile
            | SyntaxKind::TypeAliasDeclaration
            | SyntaxKind::JsTypeAliasDeclaration
            | SyntaxKind::ModuleDeclaration
            | SyntaxKind::ClassDeclaration
            | SyntaxKind::InterfaceDeclaration
            | SyntaxKind::IndexSignature
            | SyntaxKind::MappedType
            | SyntaxKind::VariableDeclaration
    ) || is_function_like_kind(kind)
}

// Go: transformers/declarations/util.go:122 isAlwaysType
pub(crate) fn is_always_type(node: Node) -> bool {
    node.kind() == SyntaxKind::InterfaceDeclaration
}

// Go: transformers/declarations/util.go:129 maskModifierFlags
pub(crate) fn mask_modifier_flags(
    node: Node,
    modifier_mask: ModifierFlags,
    modifier_additions: ModifierFlags,
) -> ModifierFlags {
    let mut flags = (get_combined_modifier_flags(node) & modifier_mask) | modifier_additions;
    if flags.intersects(ModifierFlags::DEFAULT) && !flags.intersects(ModifierFlags::EXPORT) {
        // A non-exported default is a nonsequitor - we usually try to remove all export modifiers
        // from statements in ambient declarations; but a default export must retain its export modifier to be syntactically valid
        // PORT: Go `flags ^= Export`; Export is clear here, so XOR sets it.
        flags |= ModifierFlags::EXPORT;
    }
    if flags.intersects(ModifierFlags::DEFAULT) && flags.intersects(ModifierFlags::AMBIENT) {
        // PORT: Go `flags ^= Ambient`; Ambient is set here, so XOR clears it.
        flags = flags.without(ModifierFlags::AMBIENT); // `declare` is never required alongside `default` (and would be an error if printed)
    }
    flags
}

// Go: transformers/declarations/util.go:142 unwrapParenthesizedExpression
pub(crate) fn unwrap_parenthesized_expression(mut o: Node) -> Node {
    while o.kind() == SyntaxKind::ParenthesizedExpression {
        o = o.expression();
    }
    o
}

// Go: transformers/declarations/util.go:149 isPrivateMethodTypeParameter
// ts#64649: takes the emit resolver, not the host.
pub(crate) fn is_private_method_type_parameter(resolver: &dyn EmitResolver, node: Node) -> bool {
    node.parent().kind() == SyntaxKind::MethodDeclaration
        && !resolver
            .get_effective_declaration_flags(node.parent(), ModifierFlags::PRIVATE)
            .is_empty()
}

// Go: transformers/declarations/util.go:155 shouldEmitFunctionProperties
// Returns true if expando properties should be emitted for this function.
// Properties are emitted if any overload in the symbol has a body (implementation).
// PORT: `input.Symbol.Declarations` is the binder symbol, read from the
// program's binder symbols (`program::bound_symbols`).
pub(crate) fn should_emit_function_properties(input: Node) -> bool {
    if input.body().is_some() {
        return true;
    }
    !super::diagnostics::bound_symbol_declarations(input.symbol())
        .iter()
        .all(|&decl| !is_function_declaration(decl) || decl.body().is_nil())
}

// Go: transformers/declarations/util.go:164 getEffectiveBaseTypeNode
pub(crate) fn get_effective_base_type_node(node: Node) -> Node {
    // !!! TODO: JSDoc support
    // if (baseType && isInJSFile(node)) {
    //     // Prefer an @augments tag because it may have type parameters.
    //     const tag = getJSDocAugmentsTag(node);
    //     if (tag) {
    //         return tag.class;
    //     }
    // }
    get_class_extends_heritage_element(node)
}

// Go: transformers/declarations/util.go:177 isScopeMarker
pub(crate) fn is_scope_marker(node: Node) -> bool {
    is_export_assignment(node) || is_export_declaration(node)
}

// Go: transformers/declarations/util.go:181 hasScopeMarker
// PORT: Go `*ast.StatementList` (nil allowed) is `Option<&[Node]>`.
pub(crate) fn has_scope_marker(statements: Option<&[Node]>) -> bool {
    match statements {
        None => false,
        Some(nodes) => nodes.iter().any(|&n| is_scope_marker(n)),
    }
}
