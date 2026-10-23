//! Port of Effect-TS/tsgo `internal/rules/extends_native_error.go`.

use crate::effect::diag;
use crate::effect::etscore::*;
use crate::effect::rule::*;
use crate::effect::typeparser::*;
use crate::prelude::*;

// Go: rules/extends_native_error.go ExtendsNativeError
pub static EXTENDS_NATIVE_ERROR: Rule = Rule {
    name: "extendsNativeError",
    group: "effectNative",
    description: "Warns when a class directly extends the native Error class",
    default_severity: Severity::Off,
    supported_effect: &["v3", "v4"],
    codes: &[377055],
    run: run_extends_native_error,
};

fn run_extends_native_error(ctx: &mut RuleContext<'_, '_>) -> Vec<Diagnostic> {
    let matches = analyze_extends_native_error(ctx.tp, ctx.source_file);
    let mut diags = Vec::with_capacity(matches.len());
    for m in &matches {
        diags.push(ctx.new_diagnostic(
            m.source_file,
            m.location,
            diag::This_class_extends_the_native_Error_type_directly_Untagged_native_errors_lose_distinction_in_the_Effect_failure_channel_effect_extendsNativeError,
            Vec::new(),
            Vec::new(),
        ));
    }
    diags
}

// Go: rules/extends_native_error.go ExtendsNativeErrorMatch
#[derive(Clone, Copy)]
pub struct ExtendsNativeErrorMatch {
    pub source_file: Node,
    pub location: TextRange,
}

// Go: rules/extends_native_error.go AnalyzeExtendsNativeError
pub fn analyze_extends_native_error(
    tp: &mut TypeParser<'_>,
    sf: Node,
) -> Vec<ExtendsNativeErrorMatch> {
    let error_symbol =
        tp.checker
            .resolve_name_exported("Error", Node::NIL, SymbolFlags::TYPE, false);
    if error_symbol.is_nil() {
        return Vec::new();
    }

    let mut matches = Vec::new();

    // PERF: Go walks the tree with a stack of nodes; this stack holds node
    // order indexes and pushes and pops at the same points (effperf4).
    let order = tp.node_order(sf);
    let mut node_to_visit: Vec<u32> = order.children(0).collect();

    while let Some(i) = node_to_visit.pop() {
        let node = order.node(i);
        if node.kind() == SyntaxKind::ClassDeclaration
            && let Some(m) = check_extends_native_error(tp, sf, node, error_symbol)
        {
            matches.push(m);
        }

        node_to_visit.extend(order.children(i));
    }

    matches
}

// Go: rules/extends_native_error.go checkExtendsNativeError
fn check_extends_native_error(
    tp: &mut TypeParser<'_>,
    sf: Node,
    node: Node,
    error_symbol: SymbolId,
) -> Option<ExtendsNativeErrorMatch> {
    let extends_elements = get_extends_heritage_clause_elements(node);
    if extends_elements.is_empty() {
        return None;
    }

    for elem in extends_elements {
        if elem.kind() != SyntaxKind::ExpressionWithTypeArguments {
            continue;
        }
        let expr = elem.expression();

        let expr_symbol = tp.get_symbol_at_location(expr);
        let mut resolved_symbol = expr_symbol;
        if resolved_symbol.is_some()
            && tp
                .checker
                .sym_flags(resolved_symbol)
                .intersects(SymbolFlags::ALIAS)
        {
            resolved_symbol = tp.checker.get_aliased_symbol(resolved_symbol);
        }

        let mut is_native_error = resolved_symbol == error_symbol;
        if !is_native_error && resolved_symbol.is_some() && resolved_symbol != error_symbol {
            let expr_type = tp.get_type_at_location(expr);
            if expr_type.is_some() {
                let construct_signatures = tp
                    .checker
                    .get_signatures_of_type_exported(expr_type, SignatureKind::CONSTRUCT);
                if !construct_signatures.is_empty() {
                    let instance_type = tp
                        .checker
                        .get_return_type_of_signature_exported(construct_signatures[0]);
                    if instance_type.is_some()
                        && tp.checker.ty(instance_type).symbol == error_symbol
                    {
                        is_native_error = true;
                    }
                }
            }
        }

        if is_native_error {
            let mut location_node = node.name();
            if location_node.is_nil() {
                location_node = expr;
            }
            return Some(ExtendsNativeErrorMatch {
                source_file: sf,
                location: get_error_range_for_node(sf, location_node),
            });
        }
    }

    None
}
