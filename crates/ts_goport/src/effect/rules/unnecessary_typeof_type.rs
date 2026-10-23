// Go: internal/rules/unnecessary_typeof_type.go

use crate::effect::diag;
use crate::effect::etscore::*;
use crate::effect::rule::*;
use crate::effect::typeparser::*;
use crate::prelude::*;

pub static UNNECESSARY_TYPEOF_TYPE: Rule = Rule {
    name: "unnecessaryTypeofType",
    group: "style",
    description: "Suggests replacing typeof Schema.Type style annotations with the matching named type when available",
    default_severity: Severity::Suggestion,
    supported_effect: &["v3", "v4"],
    codes: &[377090],
    run: run_unnecessary_typeof_type,
};

fn run_unnecessary_typeof_type(ctx: &mut RuleContext<'_, '_>) -> Vec<Diagnostic> {
    let matches = analyze_unnecessary_typeof_type(ctx.tp, ctx.source_file);
    let mut diags = Vec::with_capacity(matches.len());
    for m in &matches {
        diags.push(ctx.new_diagnostic(
            m.source_file,
            m.location,
            diag::This_typeof_Type_query_can_be_replaced_with_0_effect_unnecessaryTypeofType,
            Vec::new(),
            vec![m.replacement_text.clone()],
        ));
    }
    diags
}

#[derive(Clone, Debug)]
pub struct UnnecessaryTypeofTypeMatch {
    pub source_file: Node,
    pub location: TextRange,
    pub query_node: Node,
    pub inner_entity_name: Node,
    pub replacement_text: String,
}

// Go: rules/unnecessary_typeof_type.go AnalyzeUnnecessaryTypeofType
pub fn analyze_unnecessary_typeof_type(
    tp: &mut TypeParser<'_>,
    sf: Node,
) -> Vec<UnnecessaryTypeofTypeMatch> {
    fn visit(
        tp: &mut TypeParser<'_>,
        sf: Node,
        matches: &mut Vec<UnnecessaryTypeofTypeMatch>,
        node: Node,
    ) {
        if node.kind() == SyntaxKind::TypeQuery {
            if let Some(m) = analyze_unnecessary_typeof_type_node(tp, sf, node) {
                matches.push(m);
            }
        }
    }

    let mut matches = Vec::new();
    // PERF: Go walks the tree; the node order has the same nodes in the same
    // order (effperf4).
    for &n in tp.node_order(sf).nodes() {
        visit(tp, sf, &mut matches, n);
    }
    matches
}

// Go: rules/unnecessary_typeof_type.go analyzeUnnecessaryTypeofTypeNode
fn analyze_unnecessary_typeof_type_node(
    tp: &mut TypeParser<'_>,
    sf: Node,
    node: Node,
) -> Option<UnnecessaryTypeofTypeMatch> {
    let query = node;
    if query.is_nil() || query.expr_name().is_nil() {
        return None;
    }
    if query.type_argument_list().is_some() && !query.type_arguments().is_empty() {
        return None;
    }

    let expr_name = query.expr_name();
    if expr_name.is_nil() || expr_name.kind() != SyntaxKind::QualifiedName {
        return None;
    }

    let qualified_name = expr_name;
    if qualified_name.is_nil()
        || qualified_name.right().is_nil()
        || qualified_name.right().text() != "Type"
        || qualified_name.left().is_nil()
    {
        return None;
    }

    let inner_entity_name = qualified_name.left();
    if inner_entity_name.is_nil() {
        return None;
    }

    let query_type = tp.get_type_at_location(node);
    if query_type.is_nil() {
        return None;
    }

    let c = &mut *tp.checker;
    let (inner_symbol, inner_type) = resolve_entity_name_as_type(c, node, qualified_name.left());
    if inner_type.is_nil() {
        return None;
    }
    if is_self_referential_type_alias_reference(c, inner_symbol, node) {
        return None;
    }

    if !c.is_type_assignable_to(query_type, inner_type)
        || !c.is_type_assignable_to(inner_type, query_type)
    {
        return None;
    }

    Some(UnnecessaryTypeofTypeMatch {
        source_file: sf,
        location: get_error_range_for_node(sf, node),
        query_node: node,
        inner_entity_name,
        replacement_text: get_text_of_node(inner_entity_name),
    })
}

// Go: rules/unnecessary_typeof_type.go resolveEntityNameAsType
fn resolve_entity_name_as_type(
    c: &mut Checker,
    location: Node,
    entity_name: Node,
) -> (SymbolId, TypeId) {
    if location.is_nil() || entity_name.is_nil() {
        return (SymbolId::NIL, TypeId::NIL);
    }

    let entity_node = entity_name;
    if entity_node.is_nil() {
        return (SymbolId::NIL, TypeId::NIL);
    }

    match entity_node.kind() {
        SyntaxKind::Identifier => {
            let mut symbol = c.resolve_name_exported(
                entity_node.text(),
                location,
                SymbolFlags::TYPE | SymbolFlags::NAMESPACE,
                false,
            );
            symbol = resolve_alias_symbol(c, symbol);
            let t = resolved_symbol_type(c, symbol, location);
            (symbol, t)
        }
        SyntaxKind::QualifiedName => {
            let qualified_name = entity_node;
            if qualified_name.is_nil()
                || qualified_name.left().is_nil()
                || qualified_name.right().is_nil()
            {
                return (SymbolId::NIL, TypeId::NIL);
            }

            let (left_symbol, left_type) =
                resolve_entity_name_as_type(c, location, qualified_name.left());
            let mut member_symbol = resolve_qualified_type_member(
                c,
                location,
                left_symbol,
                left_type,
                qualified_name.right().text(),
            );
            member_symbol = resolve_alias_symbol(c, member_symbol);
            let t = resolved_symbol_type(c, member_symbol, location);
            (member_symbol, t)
        }
        _ => (SymbolId::NIL, TypeId::NIL),
    }
}

// Go: rules/unnecessary_typeof_type.go resolveQualifiedTypeMember
fn resolve_qualified_type_member(
    c: &mut Checker,
    location: Node,
    left_symbol: SymbolId,
    left_type: TypeId,
    member_name: &str,
) -> SymbolId {
    if location.is_nil() || member_name.is_empty() {
        return SymbolId::NIL;
    }

    if left_symbol.is_some() {
        let member = c.try_get_member_in_module_exports_and_properties(member_name, left_symbol);
        if member.is_some() {
            return member;
        }

        let declared_type = c.get_declared_type_of_symbol_exported(left_symbol);
        if declared_type.is_some() {
            let member = c.get_property_of_type_exported(declared_type, member_name);
            if member.is_some() {
                return member;
            }
        }
    }

    if left_type.is_some() {
        let member = c.get_property_of_type_exported(left_type, member_name);
        if member.is_some() {
            return member;
        }
    }

    SymbolId::NIL
}

// Go: rules/unnecessary_typeof_type.go resolvedSymbolType
fn resolved_symbol_type(c: &mut Checker, symbol: SymbolId, location: Node) -> TypeId {
    if symbol.is_nil() || location.is_nil() {
        return TypeId::NIL;
    }

    if c.sym_flags(symbol).intersects(SymbolFlags::TYPE) {
        let declared_type = c.get_declared_type_of_symbol_exported(symbol);
        if declared_type.is_some() {
            return declared_type;
        }
    }

    c.get_type_of_symbol_at_location(symbol, location)
}

// Go: rules/unnecessary_typeof_type.go resolveAliasSymbol
fn resolve_alias_symbol(c: &mut Checker, mut symbol: SymbolId) -> SymbolId {
    while symbol.is_some() && c.sym_flags(symbol).intersects(SymbolFlags::ALIAS) {
        symbol = c.get_aliased_symbol(symbol);
    }
    symbol
}

// Go: rules/unnecessary_typeof_type.go isSelfReferentialTypeAliasReference
// PORT: takes the checker to read `symbol.Declarations`.
fn is_self_referential_type_alias_reference(c: &Checker, symbol: SymbolId, location: Node) -> bool {
    if symbol.is_nil() || location.is_nil() {
        return false;
    }

    let mut current = location.parent();
    while current.is_some() {
        if current.kind() == SyntaxKind::TypeAliasDeclaration
            && c.sym(symbol).declarations.contains(&current)
        {
            return true;
        }
        current = current.parent();
    }

    false
}
