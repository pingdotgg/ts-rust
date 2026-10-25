//! Port of Effect-TS/tsgo `internal/rules/prefer_typed_schema_decoder.go`.

use crate::effect::diag;
use crate::effect::etscore::*;
use crate::effect::rule::*;
use crate::effect::typeparser::*;
use crate::prelude::*;

// PORT: Go keeps a `map[string]string` and ranges over it in random order.
// A callee references at most one of these exports, and the match returns
// on the first hit, so the order does not change the output.
// Go: rules/prefer_typed_schema_decoder.go typedSchemaDecoders
static TYPED_SCHEMA_DECODERS: [(ExportMember, &str); 6] = [
    (ExportMember::new("decodeUnknownEffect"), "decodeEffect"),
    (ExportMember::new("decodeUnknownSync"), "decodeSync"),
    (ExportMember::new("decodeUnknownExit"), "decodeExit"),
    (ExportMember::new("decodeUnknownOption"), "decodeOption"),
    (ExportMember::new("decodeUnknownResult"), "decodeResult"),
    (ExportMember::new("decodeUnknownPromise"), "decodePromise"),
];

/// PreferTypedSchemaDecoder suggests typed Schema decoders when the input is
/// statically assignable to the schema's Encoded type.
// Go: rules/prefer_typed_schema_decoder.go PreferTypedSchemaDecoder
pub static PREFER_TYPED_SCHEMA_DECODER: Rule = Rule {
    name: "preferTypedSchemaDecoder",
    group: "style",
    description: "Suggests typed Schema decoders when the input is assignable to the schema's Encoded type",
    default_severity: Severity::Suggestion,
    supported_effect: &["v4"],
    codes: &[377112],
    run: run_prefer_typed_schema_decoder,
};

fn run_prefer_typed_schema_decoder(ctx: &mut RuleContext<'_, '_>) -> Vec<Diagnostic> {
    let matches = analyze_prefer_typed_schema_decoder(ctx.tp, ctx.source_file);
    let mut diags = Vec::with_capacity(matches.len());
    for m in &matches {
        diags.push(ctx.new_diagnostic(
            m.source_file,
            m.location,
            diag::This_input_is_already_assignable_to_the_schema_s_Encoded_type_Use_0_to_preserve_compile_time_type_checking_instead_of_discarding_the_input_type_through_1_effect_preferTypedSchemaDecoder,
            Vec::new(),
            vec![m.typed_name.clone(), m.unknown_name.clone()],
        ));
    }
    diags
}

// Go: rules/prefer_typed_schema_decoder.go PreferTypedSchemaDecoderMatch
#[derive(Clone, Debug)]
pub struct PreferTypedSchemaDecoderMatch {
    pub source_file: Node,
    pub location: TextRange,
    pub decoder_name: Node,
    pub unknown_name: String,
    pub typed_name: String,
}

/// AnalyzePreferTypedSchemaDecoder finds unknown-input decoder applications whose
/// input is statically accepted by the schema's Encoded type.
// Go: rules/prefer_typed_schema_decoder.go AnalyzePreferTypedSchemaDecoder
pub fn analyze_prefer_typed_schema_decoder(
    tp: &mut TypeParser<'_>,
    sf: Node,
) -> Vec<PreferTypedSchemaDecoderMatch> {
    if tp.supported_effect_version() != EffectMajorVersion::V4 {
        return Vec::new();
    }

    let mut matches: Vec<PreferTypedSchemaDecoderMatch> = Vec::new();
    let mut handled_decoder_names: FxHashSet<Node> = FxHashSet::default();

    let flows = tp.piping_flows(sf, false);
    for flow in flows.iter() {
        let mut input_node = flow.subject.node;
        let mut input_type = flow.subject.out_type;
        for transformation in &flow.transformations {
            let (callee, schema) = schema_decoder_transformation(transformation);
            if callee.is_some() && schema.is_some() {
                if let Some(m) = analyze_typed_schema_decoder_application(
                    tp, sf, callee, schema, input_node, input_type,
                ) {
                    handled_decoder_names.insert(m.decoder_name);
                    matches.push(m);
                }
            }
            input_node = Node::NIL;
            input_type = transformation.out_type;
        }
    }

    fn visit(
        tp: &mut TypeParser<'_>,
        sf: Node,
        matches: &mut Vec<PreferTypedSchemaDecoderMatch>,
        handled_decoder_names: &mut FxHashSet<Node>,
        node: Node,
    ) {
        if node.kind() == SyntaxKind::CallExpression {
            let call = node;
            if call.argument_list().is_some()
                && !call.arguments().is_empty()
                && call.expression().is_some()
                && call.expression().kind() == SyntaxKind::CallExpression
            {
                let factory = call.expression();
                if factory.argument_list().is_some() && !factory.arguments().is_empty() {
                    if let Some(m) = analyze_typed_schema_decoder_application(
                        tp,
                        sf,
                        factory.expression(),
                        factory.arguments().get(0),
                        call.arguments().get(0),
                        TypeId::NIL,
                    ) {
                        if !handled_decoder_names.contains(&m.decoder_name) {
                            handled_decoder_names.insert(m.decoder_name);
                            matches.push(m);
                        }
                    }
                }
            }
        }
    }
    // PERF: Go walks the tree; the node order has the same nodes in the same
    // order (effperf4).
    for &n in tp.node_order(sf).nodes() {
        visit(tp, sf, &mut matches, &mut handled_decoder_names, n);
    }
    // PORT: Go `sort.Slice` is not stable; the port sorts stably. Matches
    // with the same start come from one decoder name node.
    matches.sort_by(|a, b| a.location.pos().cmp(&b.location.pos()));

    matches
}

// Go: rules/prefer_typed_schema_decoder.go schemaDecoderTransformation
fn schema_decoder_transformation(transformation: &PipingFlowTransformation) -> (Node, Node) {
    if transformation.callee.is_nil() || transformation.args.is_empty() {
        return (Node::NIL, Node::NIL);
    }
    (transformation.callee, transformation.args[0])
}

// Go: rules/prefer_typed_schema_decoder.go analyzeTypedSchemaDecoderApplication
fn analyze_typed_schema_decoder_application(
    tp: &mut TypeParser<'_>,
    sf: Node,
    callee: Node,
    schema: Node,
    input_node: Node,
    input_type: TypeId,
) -> Option<PreferTypedSchemaDecoderMatch> {
    let mut input_type = input_type;
    let (unknown_name, typed_name) = match_unknown_schema_decoder(tp, callee);
    if unknown_name.is_empty() {
        return None;
    }

    let schema_at = tp.get_type_at_location(schema);
    let schema_type = tp.effect_schema_types(schema_at);
    let Some(schema_type) = schema_type else {
        return None;
    };
    if schema_type.e.is_nil() {
        return None;
    }
    if input_type.is_nil() && input_node.is_some() {
        input_type = tp.get_type_at_location(input_node);
    }
    if input_type.is_nil()
        || tp
            .checker
            .ty(input_type)
            .flags()
            .intersects(TypeFlags::ANY | TypeFlags::UNKNOWN)
        || contains_unresolved_type_variable(tp.checker, input_type, &mut FxHashMap::default())
    {
        return None;
    }

    let mut assignable_type = input_type;
    if input_node.is_some() {
        let literal = skip_parentheses(input_node);
        if literal.is_some()
            && (literal.kind() == SyntaxKind::ObjectLiteralExpression
                || literal.kind() == SyntaxKind::ArrayLiteralExpression)
        {
            assignable_type = tp.checker.check_expression_with_contextual_type(
                literal,
                schema_type.e,
                InferenceContextId::NIL,
                CheckMode::TYPE_ONLY,
            );
        }
    }
    if assignable_type.is_nil()
        || !tp
            .checker
            .is_type_assignable_to(assignable_type, schema_type.e)
    {
        return None;
    }

    let name_node = schema_decoder_name_node(callee);
    if name_node.is_nil() {
        return None;
    }
    Some(PreferTypedSchemaDecoderMatch {
        source_file: sf,
        location: get_error_range_for_node(sf, name_node),
        decoder_name: name_node,
        unknown_name,
        typed_name,
    })
}

// Go: rules/prefer_typed_schema_decoder.go containsUnresolvedTypeVariable
fn contains_unresolved_type_variable(
    c: &mut Checker,
    t: TypeId,
    seen: &mut FxHashMap<TypeId, bool>,
) -> bool {
    if t.is_nil() || seen.get(&t).copied().unwrap_or(false) {
        return false;
    }
    seen.insert(t, true);
    let flags = c.ty(t).flags();
    if flags.intersects(TypeFlags::TYPE_VARIABLE) {
        return true;
    }
    if flags.intersects(TypeFlags::UNION_OR_INTERSECTION) {
        let parts = c.ty(t).types().to_vec();
        for part in parts {
            if contains_unresolved_type_variable(c, part, seen) {
                return true;
            }
        }
    }
    if c.ty(t).object_flags().intersects(ObjectFlags::REFERENCE) {
        for arg in c.get_type_arguments(t) {
            if contains_unresolved_type_variable(c, arg, seen) {
                return true;
            }
        }
    }
    for property in c.get_properties_of_type_exported(t) {
        let property_type = c.get_type_of_symbol_exported(property);
        if contains_unresolved_type_variable(c, property_type, seen) {
            return true;
        }
    }
    for index_info in c.get_index_infos_of_type(t) {
        let value_type = c.index_info(index_info).value_type;
        if contains_unresolved_type_variable(c, value_type, seen) {
            return true;
        }
    }
    false
}

// Go: rules/prefer_typed_schema_decoder.go matchUnknownSchemaDecoder
fn match_unknown_schema_decoder(tp: &mut TypeParser<'_>, callee: Node) -> (String, String) {
    for (unknown_name, typed_name) in &TYPED_SCHEMA_DECODERS {
        if tp.is_node_reference_to_effect_schema_module_api(callee, unknown_name)
            || tp.is_node_reference_to_effect_schema_parser_module_api(callee, unknown_name)
        {
            return (unknown_name.name.to_string(), typed_name.to_string());
        }
    }
    (String::new(), String::new())
}

// Go: rules/prefer_typed_schema_decoder.go schemaDecoderNameNode
fn schema_decoder_name_node(callee: Node) -> Node {
    if callee.is_nil() {
        return Node::NIL;
    }
    if callee.kind() == SyntaxKind::PropertyAccessExpression {
        return callee.name();
    }
    if callee.kind() == SyntaxKind::Identifier {
        return callee;
    }
    Node::NIL
}
