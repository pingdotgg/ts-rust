//! Port of Effect-TS/tsgo `internal/fixables/missing_effect_error_catch.go`.

use crate::effect::ls::fixable::{Fixable, FixableContext};
use crate::effect::rules::missing_effect_error::analyze_missing_effect_error;
use crate::effect::typeparser::{EffectMajorVersion, TypeParser};
use crate::gostd::strconv;
use crate::ls::CodeAction;
use crate::prelude::*;

// Go: fixables/missing_effect_error_catch.go:19 MissingEffectErrorCatchFix
pub static MISSING_EFFECT_ERROR_CATCH_FIX: Fixable = Fixable {
    name: "missingEffectErrorCatch",
    description: "Catch all errors with Effect.catch/catchAll",
    // Go: tsdiag.Missing_errors_0_in_the_expected_Effect_type_effect_missingEffectError
    error_codes: &[377003],
    fix_ids: &[
        "missingEffectError_catch",
        "missingEffectError_catchAll",
        "missingEffectError_tagged",
    ],
    run: run_missing_effect_error_catch_fix,
};

// Go: fixables/missing_effect_error_catch.go:32 fixCandidate
struct FixCandidate {
    start: i32,
    end: i32,
    tags: Vec<String>,
}

impl FixCandidate {
    /// Go: `candidate == nil || start < candidate.start || (start ==
    /// candidate.start && end < candidate.end)`.
    fn is_replaced_by(candidate: &Option<FixCandidate>, start: i32, end: i32) -> bool {
        match candidate {
            None => true,
            Some(c) => start < c.start || (start == c.start && end < c.end),
        }
    }
}

// Go: fixables/missing_effect_error_catch.go:27 runMissingEffectErrorCatchFix
fn run_missing_effect_error_catch_fix(ctx: &mut FixableContext<'_, '_>) -> Vec<CodeAction> {
    let sf = ctx.source_file;

    let mut catch_candidate: Option<FixCandidate> = None;
    let mut tagged_candidate: Option<FixCandidate> = None;

    let matches = analyze_missing_effect_error(ctx.tp, sf);
    for m in &matches {
        if !m.location.intersects(ctx.span) && !ctx.span.contained_by(m.location) {
            continue;
        }

        let error_expr = expression_node_for_missing_effect_error(m.error_node);
        if error_expr.is_nil() {
            continue;
        }

        let node_start_pos = get_token_pos_of_node(error_expr, sf, false);
        let node_end_pos = error_expr.end();

        // Offer catch/catchAll only when expected error type is `never`.
        if ctx
            .tp
            .checker
            .ty(m.expected_error_type)
            .flags()
            .intersects(TypeFlags::NEVER)
            && FixCandidate::is_replaced_by(&catch_candidate, node_start_pos, node_end_pos)
        {
            catch_candidate = Some(FixCandidate {
                start: node_start_pos,
                end: node_end_pos,
                tags: Vec::new(),
            });
        }

        // Offer catchTags only when all missing error members have a literal _tag.
        if let Some(tags) = collect_literal_missing_error_tags(ctx.tp, &m.unhandled_errors)
            && FixCandidate::is_replaced_by(&tagged_candidate, node_start_pos, node_end_pos)
        {
            tagged_candidate = Some(FixCandidate {
                start: node_start_pos,
                end: node_end_pos,
                tags,
            });
        }
    }

    let mut actions = Vec::new();

    if let Some(candidate) = &catch_candidate {
        let method_name = if ctx.tp.supported_effect_version() == EffectMajorVersion::V3 {
            "catchAll"
        } else {
            "catch"
        };

        let description = format!("Catch all errors with Effect.{method_name}");
        let todo_message = format!("TODO: {method_name} not implemented");

        let start = ctx.byte_pos_to_lsp_position(candidate.start);
        let end = ctx.byte_pos_to_lsp_position(candidate.end);
        if let Some(action) = ctx.new_fix_action(description, |tracker| {
            tracker.insert_text(sf, start, &format!("Effect.{method_name}("));
            tracker.insert_text(
                sf,
                end,
                &format!(
                    ", () => Effect.dieMessage({}))",
                    strconv::quote(&todo_message)
                ),
            );
        }) {
            actions.push(action);
        }
    }

    if let Some(candidate) = &tagged_candidate {
        let start = ctx.byte_pos_to_lsp_position(candidate.start);
        let end = ctx.byte_pos_to_lsp_position(candidate.end);
        if let Some(action) = ctx.new_fix_action(
            "Catch unexpected errors with Effect.catchTag".to_string(),
            |tracker| {
                tracker.insert_text(sf, start, "Effect.catchTags(");
                tracker.insert_text(
                    sf,
                    end,
                    &format!(", {})", build_catch_tags_handlers_object(&candidate.tags)),
                );
            },
        ) {
            actions.push(action);
        }
    }

    actions
}

// Go: fixables/missing_effect_error_catch.go:114 collectLiteralMissingErrorTags
// PORT: Go returns `(nil, false)` for "not all literal tags"; Rust returns
// None. Go's checker argument is `tp.checker`.
fn collect_literal_missing_error_tags(
    tp: &mut TypeParser<'_>,
    missing: &[TypeId],
) -> Option<Vec<String>> {
    if missing.is_empty() {
        return None;
    }

    let mut seen = FxHashSet::default();
    let mut tags = Vec::with_capacity(missing.len());
    for &missing_type in missing {
        let mut tag_type = tp
            .checker
            .get_type_of_property_of_type_exported(missing_type, "_tag");
        if tag_type.is_nil() {
            tag_type = tp.get_type_of_property_by_name(missing_type, "_tag");
        }
        if tag_type.is_nil()
            || !tp
                .checker
                .ty(tag_type)
                .flags()
                .intersects(TypeFlags::STRING_LITERAL)
        {
            return None;
        }
        let Some(LiteralValue::String(tag_value)) =
            tp.checker.ty(tag_type).as_literal_type().value()
        else {
            return None;
        };
        if seen.insert(tag_value.clone()) {
            tags.push(tag_value.clone());
        }
    }

    tags.sort();
    Some(tags)
}

// Go: fixables/missing_effect_error_catch.go:146 buildCatchTagsHandlersObject
fn build_catch_tags_handlers_object(tags: &[String]) -> String {
    let mut b = String::from("{ ");
    for (i, tag) in tags.iter().enumerate() {
        if i > 0 {
            b.push_str(", ");
        }
        b.push_str(&strconv::quote(tag));
        b.push_str(": () => Effect.dieMessage(");
        b.push_str(&strconv::quote(&format!(
            "TODO: catchTags() not implemented for {tag}"
        )));
        b.push(')');
    }
    b.push_str(" }");
    b
}

// Go: fixables/missing_effect_error_catch.go:162 expressionNodeForMissingEffectError
// A bare `return` panics in Go `ast.IsExpression(stmt.Expression)` (nil
// dereference), and `is_expression(Node::NIL)` panics the same way. No
// match reaches it: the checker's error node for a bare return has the
// source type `undefined` (checkReturnExpression), and
// AnalyzeMissingEffectError skips a source that is not an Effect.
fn expression_node_for_missing_effect_error(node: Node) -> Node {
    if node.is_nil() {
        return Node::NIL;
    }
    // Handle SatisfiesExpression before the general IsExpression check:
    // we want to wrap just the LHS expression, not the entire `expr satisfies Type`.
    if node.kind() == SyntaxKind::SatisfiesExpression {
        return node.expression();
    }
    if is_expression(node) {
        return node;
    }
    match node.kind() {
        SyntaxKind::VariableDeclaration => {
            let initializer = node.initializer();
            if initializer.is_some() && is_expression(initializer) {
                return initializer;
            }
        }
        SyntaxKind::ReturnStatement => {
            let expression = node.expression();
            if is_expression(expression) {
                return expression;
            }
        }
        SyntaxKind::ArrowFunction => {
            let body = node.body();
            if is_expression(body) {
                return body;
            }
        }
        _ => {}
    }
    Node::NIL
}
