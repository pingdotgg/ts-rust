// Go: internal/typeparser/get_type_at_location.go

use crate::effect::typeparser::*;
use crate::prelude::*;

impl TypeParser<'_> {
    /// GetTypeAtLocation wraps checker.GetTypeAtLocation with node-kind and JSX safety guards.
    /// It returns nil when the node is nil, not an expression/type-node/declaration,
    /// an import clause, a JSX tag name, or a JSX attribute name. It also recovers
    /// from checker panics (e.g. nil symbol dereferences on certain declaration
    /// nodes) and returns nil.
    pub fn get_type_at_location(&mut self, node: Node) -> TypeId {
        if node.is_nil() {
            return TypeId::NIL;
        }

        // Go `Cached(&tp.links.TypeAtLocation, node, ..)`.
        if let Some(cached) = self.links().type_at_location.try_get(node) {
            return TypeId(cached.get());
        }
        let t = self.get_type_at_location_uncached(node);
        *self.links().type_at_location.get(node) = CachedId::new(t.0);
        t
    }

    pub fn get_type_at_location_uncached(&mut self, node: Node) -> TypeId {
        if node.is_nil() {
            return TypeId::NIL;
        }

        if node.parent().is_some() {
            if is_jsx_tag_name(node) {
                return TypeId::NIL;
            }

            if is_jsx_attribute(node.parent()) && node.parent().name() == node {
                return TypeId::NIL;
            }
        }

        if !is_expression(node) && !is_type_node(node) && !is_declaration(node) {
            return TypeId::NIL;
        }

        // ImportClause passes the IsDeclaration check above, but a clause without a
        // default binding (import { A } from "x", import * as ns from "x") declares
        // no symbol itself, and checker.GetTypeAtLocation panics dereferencing the
        // nil symbol. The clause type is never useful to rules: its bindings are
        // visited as separate nodes and carry the actual types.
        if node.kind() == SyntaxKind::ImportClause {
            return TypeId::NIL;
        }

        // Tagged templates pass interpolation values directly to the tag function;
        // they do not stringify them. Asking the checker for the type of the inner
        // TemplateExpression forces a normally unreachable checking path that can
        // emit TS2731 for symbol-typed interpolations as a side effect.
        if node.kind() == SyntaxKind::TemplateExpression
            && node.parent().is_some()
            && is_tagged_template_expression(node.parent())
        {
            return TypeId::NIL;
        }

        // A meta property used as a call callee (import.defer(...)) has no type of
        // its own and the checker debug-asserts when asked (checkMetaProperty); the
        // enclosing call expression carries the meaningful type.
        if node.kind() == SyntaxKind::MetaProperty
            && node.parent().is_some()
            && is_call_expression(node.parent())
            && node.parent().expression() == node
        {
            return TypeId::NIL;
        }

        if is_inside_type_only_heritage_expression(node) {
            return TypeId::NIL;
        }

        let c = &mut *self.checker;
        // Go: defer func() { if r := recover(); r != nil { result = nil } }()
        go_recover(|| c.get_type_at_location(node)).unwrap_or(TypeId::NIL)
    }
}

/// isInsideTypeOnlyHeritageExpression reports whether node is an
/// ExpressionWithTypeArguments or one of its identifier/property-access
/// sub-expressions inside a type-only heritage clause. The checker can
/// mis-resolve these as value expressions and emit bogus diagnostics.
pub fn is_inside_type_only_heritage_expression(node: Node) -> bool {
    if node.kind() == SyntaxKind::ExpressionWithTypeArguments {
        return is_type_only_heritage_clause(node.parent());
    }

    if node.kind() != SyntaxKind::Identifier && node.kind() != SyntaxKind::PropertyAccessExpression
    {
        return false;
    }

    let mut n = node.parent();
    while n.is_some() {
        match n.kind() {
            SyntaxKind::PropertyAccessExpression => {
                n = n.parent();
                continue;
            }
            SyntaxKind::ExpressionWithTypeArguments => {
                return is_type_only_heritage_clause(n.parent());
            }
            _ => return false,
        }
    }

    false
}

pub fn is_type_only_heritage_clause(node: Node) -> bool {
    if node.is_nil() || !is_heritage_clause(node) {
        return false;
    }

    let heritage_clause = node;
    let container = node.parent();
    if container.is_nil() {
        return false;
    }

    if container.kind() == SyntaxKind::InterfaceDeclaration {
        return true;
    }

    is_class_like(container) && heritage_clause.token() == SyntaxKind::ImplementsKeyword
}
