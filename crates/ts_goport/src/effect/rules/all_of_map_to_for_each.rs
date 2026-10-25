//! Port of Effect-TS/tsgo `internal/rules/all_of_map_to_for_each.go`.

use crate::effect::diag;
use crate::effect::etscore::*;
use crate::effect::rule::*;
use crate::effect::typeparser::*;
use crate::prelude::*;

// Go: rules.AllOfMapToForEach
/// AllOfMapToForEach suggests using Effect.forEach instead of constructing an
/// intermediate array of effects with Array#map and passing it to Effect.all.
pub static ALL_OF_MAP_TO_FOR_EACH: Rule = Rule {
    name: "allOfMapToForEach",
    group: "style",
    description: "Suggests using Effect.forEach instead of Effect.all over an effectful Array#map",
    default_severity: Severity::Suggestion,
    supported_effect: &["v3", "v4"],
    codes: &[377113],
    run: run_all_of_map_to_for_each,
};

fn run_all_of_map_to_for_each(ctx: &mut RuleContext<'_, '_>) -> Vec<Diagnostic> {
    let matches = analyze_all_of_map_to_for_each(ctx.tp, ctx.source_file);
    let mut diagnostics = Vec::with_capacity(matches.len());
    for m in &matches {
        diagnostics.push(ctx.new_diagnostic(
            m.source_file,
            m.location,
            diag::Effect_forEach_expresses_this_effectful_array_mapping_more_directly_than_Effect_all_over_Array_map_effect_allOfMapToForEach,
            Vec::new(),
            Vec::new(),
        ));
    }
    diagnostics
}

// Go: rules.AllOfMapToForEachMatch
/// AllOfMapToForEachMatch holds the nodes needed by the diagnostic and its fix.
#[derive(Clone)]
pub struct AllOfMapToForEachMatch {
    pub source_file: Node,
    pub location: TextRange,
    pub call_node: Node,
    pub effect_module: Node,
    pub receiver: Node,
    pub callback: Node,
    pub options: Node,
    pub has_type_arguments: bool,
    /// CanFix reports whether the match is a standalone Effect.all call the
    /// quick fix can rewrite. Data-last Effect.all references inside piping
    /// flows are diagnostic-only, since the fix would have to restructure the
    /// surrounding pipe.
    pub can_fix: bool,
}

// Go: rules.AnalyzeAllOfMapToForEach
/// AnalyzeAllOfMapToForEach finds Effect.all over an effectful Array#map whose
/// receiver is array-like, in both the direct data-first form and the data-last
/// form expressed through piping flows (e.g. pipe(xs.map(f), Effect.all)).
pub fn analyze_all_of_map_to_for_each(
    tp: &mut TypeParser<'_>,
    sf: Node,
) -> Vec<AllOfMapToForEachMatch> {
    let mut matches = Vec::new();

    // Direct data-first calls: Effect.all(xs.map(f), options?)
    // PERF: Go walks the tree; the node order has the same nodes in the same
    // order (effperf4).
    for &n in tp.node_order(sf).nodes() {
        visit(tp, sf, &mut matches, n);
    }

    // Data-last Effect.all references inside piping flows. A bare reference is
    // invisible to the call walk above because it never appears as a call.
    let flows = tp.piping_flows(sf, true);
    for flow in flows.iter() {
        let mut any_transformation = |_: &mut TypeParser<'_>, _: &PipingFlowTransformation| true;
        let mut all_reference =
            |tp: &mut TypeParser<'_>, transformation: &PipingFlowTransformation| {
                transformation.args.is_empty()
                    && (transformation.kind == TransformationKind::Pipe
                        || transformation.kind == TransformationKind::Pipeable)
                    && transformation.callee.is_some()
                    && tp.is_node_reference_to_effect_module_api(
                        transformation.callee,
                        member!("all"),
                    )
            };
        let mut predicates: [&mut PipingFlowTransformationPredicate<'_>; 2] =
            [&mut any_transformation, &mut all_reference];
        let sequences = flow.find_transformation_sequences(tp, &mut predicates);
        for sequence in &sequences {
            let start = sequence.start as usize;
            let transformation = &flow.transformations[start + 1];
            let map_node = transformation_application_node(&flow.transformations[start]);
            if let Some(m) = analyze_all_of_map_to_for_each_receiver(
                tp,
                sf,
                transformation.callee,
                map_node,
                Node::NIL,
                false,
            ) {
                matches.push(m);
            }
        }
    }

    // PORT: Go uses sort.Slice (not stable); this sort is stable.
    matches.sort_by(|a, b| a.location.pos().cmp(&b.location.pos()));
    matches
}

fn visit(tp: &mut TypeParser<'_>, sf: Node, matches: &mut Vec<AllOfMapToForEachMatch>, node: Node) {
    if let Some(m) = analyze_all_of_map_to_for_each_call(tp, sf, node) {
        matches.push(m);
    }
}

// Go: rules.transformationApplicationNode
/// transformationApplicationNode returns the call expression that applies the
/// transformation's callee, when that application is represented in the tree.
/// Bare pipeable references have no application of their own and return nil.
fn transformation_application_node(transformation: &PipingFlowTransformation) -> Node {
    if transformation.callee.is_nil()
        || transformation.callee.parent().is_nil()
        || !is_call_expression(transformation.callee.parent())
    {
        return Node::NIL;
    }
    let call = transformation.callee.parent();
    if call.expression() != transformation.callee {
        return Node::NIL;
    }
    call
}

// Go: rules.analyzeAllOfMapToForEachCall
fn analyze_all_of_map_to_for_each_call(
    tp: &mut TypeParser<'_>,
    sf: Node,
    node: Node,
) -> Option<AllOfMapToForEachMatch> {
    if node.kind() != SyntaxKind::CallExpression {
        return None;
    }
    let all_call = node;
    let all_args = all_call.arguments().to_vec();
    if all_call.expression().is_nil() || all_args.is_empty() || all_args.len() > 2 {
        return None;
    }
    if !tp.is_node_reference_to_effect_module_api(all_call.expression(), member!("all"))
        || contains_spread_element(&all_args)
    {
        return None;
    }

    let mut options = Node::NIL;
    if all_args.len() == 2 {
        options = all_args[1];
    }
    let mut m = analyze_all_of_map_to_for_each_receiver(
        tp,
        sf,
        all_call.expression(),
        all_args[0],
        options,
        has_call_type_arguments(all_call),
    )?;
    m.call_node = node;
    m.can_fix = true;
    Some(m)
}

// Go: rules.analyzeAllOfMapToForEachReceiver
/// analyzeAllOfMapToForEachReceiver validates the xs.map(f) shape feeding an
/// Effect.all node and assembles the match.
fn analyze_all_of_map_to_for_each_receiver(
    tp: &mut TypeParser<'_>,
    sf: Node,
    all_node: Node,
    map_node: Node,
    options: Node,
    all_type_arguments: bool,
) -> Option<AllOfMapToForEachMatch> {
    let map_node = skip_parentheses(map_node);
    if map_node.is_nil() || map_node.kind() != SyntaxKind::CallExpression {
        return None;
    }
    let map_call = map_node;
    let map_args = map_call.arguments().to_vec();
    if map_call.expression().is_nil()
        || map_call.expression().kind() != SyntaxKind::PropertyAccessExpression
        || map_args.len() != 1
        || contains_spread_element(&map_args)
    {
        return None;
    }
    let map_access = map_call.expression();
    if map_access.name().is_nil()
        || map_access.name().text() != "map"
        || map_access.expression().is_nil()
    {
        return None;
    }

    let receiver_type = tp.get_type_at_location(map_access.expression());
    let is_array_receiver = receiver_type.is_some()
        && (tp.checker.is_array_type(receiver_type)
            || tp.checker.is_readonly_array_type(receiver_type));
    if !is_array_receiver {
        return None;
    }

    let callback = map_args[0];
    let map_result_type = tp.get_type_at_location(map_node);
    if map_result_type.is_nil() {
        return None;
    }
    let number_index_type = tp.checker.get_number_index_type(map_result_type);
    if tp.effect_type(number_index_type).is_none() {
        return None;
    }

    if options.is_some() {
        let options_type = tp.get_type_at_location(options);
        if options_type.is_some()
            && tp
                .checker
                .get_property_of_type_exported(options_type, "mode")
                .is_some()
        {
            return None;
        }
    }

    let mut effect_module = Node::NIL;
    if all_node.kind() == SyntaxKind::PropertyAccessExpression {
        effect_module = all_node.expression();
    }

    Some(AllOfMapToForEachMatch {
        source_file: sf,
        location: get_error_range_for_node(sf, all_node),
        call_node: Node::NIL,
        effect_module,
        receiver: map_access.expression(),
        callback,
        options,
        has_type_arguments: all_type_arguments || has_call_type_arguments(map_call),
        can_fix: false,
    })
}

// Go: rules.containsSpreadElement
pub fn contains_spread_element(nodes: &[Node]) -> bool {
    for &node in nodes {
        if node.is_nil() || node.kind() == SyntaxKind::SpreadElement {
            return true;
        }
    }
    false
}

// Go: rules.hasCallTypeArguments
pub fn has_call_type_arguments(call: Node) -> bool {
    call.is_some() && !call.type_arguments().is_empty()
}
