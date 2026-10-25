//! Port of Effect-TS/tsgo `internal/rules/run_of_exit_to_run_exit.go`.

use crate::effect::diag;
use crate::effect::etscore::*;
use crate::effect::rule::*;
use crate::effect::typeparser::*;
use crate::prelude::*;

/// RunOfExitToRunExit suggests using the dedicated promise Exit runner instead
/// of running an Effect that has first been transformed with Effect.exit.
// Go: rules/run_of_exit_to_run_exit.go RunOfExitToRunExit
pub static RUN_OF_EXIT_TO_RUN_EXIT: Rule = Rule {
    name: "runOfExitToRunExit",
    group: "style",
    description: "Suggests using Effect.runPromiseExit instead of passing Effect.exit to Effect.runPromise",
    default_severity: Severity::Suggestion,
    supported_effect: &["v3", "v4"],
    codes: &[377123],
    run: run_run_of_exit_to_run_exit,
};

fn run_run_of_exit_to_run_exit(ctx: &mut RuleContext<'_, '_>) -> Vec<Diagnostic> {
    let sf = ctx.source_file;
    let matches = analyze_run_of_exit_to_run_exit(ctx.tp, sf);
    let mut diagnostics = Vec::with_capacity(matches.len());
    for m in &matches {
        diagnostics.push(ctx.new_diagnostic(
            m.source_file,
            m.location,
            diag::Effect_0_of_Effect_exit_re_implements_Effect_1_Use_the_dedicated_Exit_runner_directly_effect_runOfExitToRunExit,
            Vec::new(),
            vec![m.runner_name.clone(), m.replacement_name.clone()],
        ));
    }
    diagnostics
}

/// RunOfExitToRunExitMatch holds the transformations needed by the diagnostic
/// and quick fix.
// PORT: Go `ExitTransformation` is a pointer into the flow; the port keeps a copy.
// Go: rules/run_of_exit_to_run_exit.go RunOfExitToRunExitMatch
#[derive(Clone, Debug)]
pub struct RunOfExitToRunExitMatch {
    pub source_file: Node,
    pub location: TextRange,
    pub exit_transformation: PipingFlowTransformation,
    pub runner_callee: Node,
    pub runner_name_node: Node,
    pub runner_name: String,
    pub replacement_name: String,
}

/// AnalyzeRunOfExitToRunExit finds Effect.exit immediately followed by a
/// non-Exit runner in a normalized piping flow. Adjacency is important: an
/// Effect.exit followed by another transformation is not the runner's input.
// Go: rules/run_of_exit_to_run_exit.go AnalyzeRunOfExitToRunExit
pub fn analyze_run_of_exit_to_run_exit(
    tp: &mut TypeParser<'_>,
    sf: Node,
) -> Vec<RunOfExitToRunExitMatch> {
    if sf.is_nil() {
        return Vec::new();
    }

    let mut matches: Vec<RunOfExitToRunExitMatch> = Vec::new();
    let flows = tp.piping_flows(sf, false);
    for flow in flows.iter() {
        let mut is_exit = |tp: &mut TypeParser<'_>, transformation: &PipingFlowTransformation| {
            transformation.callee.is_some()
                && transformation.args.is_empty()
                && tp.is_node_reference_to_effect_module_api(transformation.callee, member!("exit"))
        };
        let mut is_runner = |tp: &mut TypeParser<'_>, transformation: &PipingFlowTransformation| {
            let (_, _, runner_name, _) = run_of_exit_runner(tp, transformation.callee);
            !runner_name.is_empty()
        };
        let mut predicates: [&mut PipingFlowTransformationPredicate<'_>; 2] =
            [&mut is_exit, &mut is_runner];
        let sequences = flow.find_transformation_sequences(tp, &mut predicates);
        for sequence in &sequences {
            let start = sequence.start as usize;
            let exit_transformation = &flow.transformations[start];
            let runner_transformation = &flow.transformations[start + 1];
            let (runner_callee, runner_name_node, runner_name, replacement_name) =
                run_of_exit_runner(tp, runner_transformation.callee);

            append_run_of_exit_match(
                &mut matches,
                RunOfExitToRunExitMatch {
                    source_file: sf,
                    location: run_of_exit_location(
                        sf,
                        runner_transformation.callee,
                        runner_name_node,
                    ),
                    exit_transformation: exit_transformation.clone(),
                    runner_callee,
                    runner_name_node,
                    runner_name: runner_name.to_string(),
                    replacement_name: replacement_name.to_string(),
                },
            );
        }
    }

    // Non-dual runners with RunOptions are not piping-flow transformations,
    // because their effect parameter is one of multiple arguments. Inspect the
    // runner call itself, then still use a piping flow for its effect argument so
    // direct Effect.exit calls and pipe tails share the same matching logic.
    fn visit(
        tp: &mut TypeParser<'_>,
        sf: Node,
        matches: &mut Vec<RunOfExitToRunExitMatch>,
        node: Node,
    ) {
        if node.kind() == SyntaxKind::CallExpression {
            let call = node;
            if call.is_some()
                && call.expression().is_some()
                && call.argument_list().is_some()
                && !call.arguments().is_empty()
            {
                let (runner_callee, runner_name_node, runner_name, replacement_name) =
                    run_of_exit_runner(tp, call.expression());
                if !runner_name.is_empty() {
                    let argument_flow = tp.longest_piping_flow_at(call.arguments().get(0), false);
                    if let Some(argument_flow) = argument_flow
                        && !argument_flow.transformations.is_empty()
                    {
                        let exit_transformation =
                            &argument_flow.transformations[argument_flow.transformations.len() - 1];
                        if exit_transformation.callee.is_some()
                            && exit_transformation.args.is_empty()
                            && tp.is_node_reference_to_effect_module_api(
                                exit_transformation.callee,
                                member!("exit"),
                            )
                        {
                            append_run_of_exit_match(
                                matches,
                                RunOfExitToRunExitMatch {
                                    source_file: sf,
                                    location: run_of_exit_location(
                                        sf,
                                        call.expression(),
                                        runner_name_node,
                                    ),
                                    exit_transformation: exit_transformation.clone(),
                                    runner_callee,
                                    runner_name_node,
                                    runner_name: runner_name.to_string(),
                                    replacement_name: replacement_name.to_string(),
                                },
                            );
                        }
                    }
                }
            }
        }
    }
    // PERF: Go walks the tree; the node order has the same nodes in the same
    // order (effperf4).
    for &n in tp.node_order(sf).nodes() {
        visit(tp, sf, &mut matches, n);
    }

    // PORT: Go `sort.Slice` is not stable; matches with an equal start come
    // from distinct ranges, and the port keeps them in walk order.
    matches.sort_by(|a, b| a.location.pos().cmp(&b.location.pos()));
    matches
}

// Go: rules/run_of_exit_to_run_exit.go appendRunOfExitMatch
fn append_run_of_exit_match(
    matches: &mut Vec<RunOfExitToRunExitMatch>,
    candidate: RunOfExitToRunExitMatch,
) {
    for m in matches.iter() {
        if m.location.pos() == candidate.location.pos()
            && m.location.end() == candidate.location.end()
        {
            return;
        }
    }
    matches.push(candidate);
}

// Go: rules/run_of_exit_to_run_exit.go runOfExitLocation
fn run_of_exit_location(sf: Node, callee: Node, name_node: Node) -> TextRange {
    let mut location_node = callee;
    if name_node.is_some() && name_node.parent().is_some() {
        location_node = name_node.parent();
    }
    get_error_range_for_node(sf, location_node)
}

/// Go returns `(targetNode, nameNode, runnerName, replacementName)`.
// Go: rules/run_of_exit_to_run_exit.go runOfExitRunner
fn run_of_exit_runner(
    tp: &mut TypeParser<'_>,
    callee: Node,
) -> (Node, Node, &'static str, &'static str) {
    if callee.is_nil() {
        return (Node::NIL, Node::NIL, "", "");
    }

    let mut target = callee;
    if callee.kind() == SyntaxKind::CallExpression {
        let call = callee;
        if call.is_nil() || call.expression().is_nil() {
            return (Node::NIL, Node::NIL, "", "");
        }
        target = call.expression();
    }

    for (runner, replacement) in [
        (member!("runPromise"), "runPromiseExit"),
        (member!("runPromiseWith"), "runPromiseExitWith"),
    ] {
        if !tp.is_node_reference_to_effect_module_api(target, runner) {
            continue;
        }
        let mut name_node = Node::NIL;
        if target.kind() == SyntaxKind::PropertyAccessExpression {
            name_node = target.name();
        }
        return (target, name_node, runner.name, replacement);
    }

    (Node::NIL, Node::NIL, "", "")
}
