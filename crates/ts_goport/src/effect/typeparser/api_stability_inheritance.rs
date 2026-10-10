//! Port of Effect-TS/tsgo `internal/typeparser/api_stability_inheritance.go` at
//! `@effect/tsgo@0.51.1` (`47cb1ed7`).

use crate::effect::typeparser::*;
use crate::prelude::*;

impl ApiStabilityAnalysis {
    // Go: typeparser/api_stability_inheritance.go apiStabilityAnalysis.optionalTaggedBase
    /// optionalTaggedBase reads declaration metadata only. Every public member,
    /// including inherited and merged members, must be an optional property or
    /// method with its own recognized stability tag. Internal members are ignored.
    /// Call, construct and index signatures cannot be optional. Unknown bases fail
    /// closed; member types are
    /// never evaluated merely to decide whether to exempt an inherited base.
    // PORT: Go makes `optionalTaggedSymbols` on the first write; the port's map
    // always exists. Only `true` is cached, as in Go.
    pub fn optional_tagged_base(&mut self, tp: &mut TypeParser<'_>, symbol: SymbolId) -> bool {
        if self
            .optional_tagged_symbols
            .get(&symbol)
            .copied()
            .unwrap_or(false)
        {
            return true;
        }
        let mut pending = vec![symbol];
        let mut seen: FxHashSet<SymbolId> = FxHashSet::default();
        while let Some(current) = pending.pop() {
            if current.is_nil()
                || !tp
                    .checker
                    .sym(current)
                    .flags
                    .intersects(SymbolFlags::INTERFACE | SymbolFlags::CLASS)
                || tp.checker.sym(current).declarations.is_empty()
                || !self.consume_work()
            {
                return false;
            }
            if !seen.insert(current) {
                continue;
            }
            let is_class_with_heritage =
                tp.checker.sym(current).flags.intersects(SymbolFlags::CLASS)
                    && api_stability_symbol_declares_heritage(tp.checker, current);
            if is_class_with_heritage {
                let declared =
                    checker_integration::get_resolved_declared_type_of_symbol_if_materialized(
                        tp.checker, current,
                    );
                if declared.is_nil() {
                    return false;
                }
                let (_, resolved) =
                    checker_integration::get_resolved_base_types_of_type_if_materialized(
                        tp.checker, declared,
                    );
                if !resolved {
                    return false;
                }
            }
            let declarations = tp.checker.sym(current).declarations.to_vec();
            for &declaration in &declarations {
                if !matches!(
                    declaration.kind(),
                    SyntaxKind::InterfaceDeclaration
                        | SyntaxKind::ClassDeclaration
                        | SyntaxKind::ClassExpression
                ) {
                    return false;
                }
                for member in declaration.members().iter() {
                    let member_symbol = tp.checker.get_symbol_of_declaration(member);
                    if api_stability_symbol_has_only_internal_declarations(
                        tp.checker,
                        member_symbol,
                    ) {
                        continue;
                    }
                    if !matches!(
                        member.kind(),
                        SyntaxKind::PropertySignature
                            | SyntaxKind::MethodSignature
                            | SyntaxKind::PropertyDeclaration
                            | SyntaxKind::MethodDeclaration
                    ) || !has_question_token(member)
                        || stability_of_declaration_tag(member).is_empty()
                    {
                        return false;
                    }
                }
            }
            for &declaration in &declarations {
                if declaration.kind() != SyntaxKind::InterfaceDeclaration {
                    continue;
                }
                for node in get_extends_heritage_clause_elements(declaration) {
                    let name = match node.kind() {
                        SyntaxKind::TypeReference => node.type_name(),
                        SyntaxKind::ExpressionWithTypeArguments => node.expression(),
                        _ => return false,
                    };
                    let mut base = self.symbol_at_type_name_node(tp, name);
                    let mut aliases: FxHashSet<SymbolId> = FxHashSet::default();
                    while base.is_some()
                        && tp.checker.sym(base).flags.intersects(SymbolFlags::ALIAS)
                        && aliases.insert(base)
                    {
                        base = api_stability_immediate_aliased_symbol(tp.checker, base);
                    }
                    pending.push(base);
                }
            }
            if is_class_with_heritage {
                let declared =
                    checker_integration::get_resolved_declared_type_of_symbol_if_materialized(
                        tp.checker, current,
                    );
                let (bases, _) =
                    checker_integration::get_resolved_base_types_of_type_if_materialized(
                        tp.checker, declared,
                    );
                for base in bases {
                    if base.is_nil() || tp.checker.ty(base).symbol().is_nil() {
                        return false;
                    }
                    pending.push(tp.checker.ty(base).symbol());
                }
            }
        }
        self.optional_tagged_symbols.insert(symbol, true);
        true
    }

    // Go: typeparser/api_stability_inheritance.go apiStabilityAnalysis.optionalTaggedInheritedMember
    /// optionalTaggedInheritedMember applies the base exception to properties the
    /// compiler flattened into a derived member table, including superclass factory
    /// intersection types. Own members are always inspected independently.
    pub fn optional_tagged_inherited_member(
        &mut self,
        tp: &mut TypeParser<'_>,
        member: SymbolId,
        owner: SymbolId,
    ) -> bool {
        let declaring = self.member_declaring_symbol(tp, member);
        declaring.is_some()
            && owner.is_some()
            && declaring != owner
            && self.optional_tagged_base(tp, declaring)
    }
}
