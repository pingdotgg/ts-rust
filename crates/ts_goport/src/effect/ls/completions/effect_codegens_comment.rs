//! Port of Effect-TS/tsgo `internal/completions/effect_codegens_comment.go`
//! (effect-tsgo 0.51.1).

use super::effect_diagnostics_comment::find_comment_at;
use super::effect_schema_self_in_classes::{byte_span_to_range, make_completion_item};
use crate::effect::codegens;
use crate::effect::ls::completion::{Completion, CompletionContext};
use crate::ls::prelude::*;

// Go: completions/effect_codegens_comment.go:15 effectCodegensComment
/// effectCodegensComment provides completions for @effect-codegens directives
/// when typing @ inside a comment.
pub static EFFECT_CODEGENS_COMMENT: Completion = Completion {
    name: "effectCodegensComment",
    description: "Provides @effect-codegens completion inside comments",
    run: run_effect_codegens_comment,
};

// Go: completions/effect_codegens_comment.go:21 runEffectCodegensComment
fn run_effect_codegens_comment(
    ctx: &mut CompletionContext<'_, '_>,
) -> Vec<lsproto::CompletionItem> {
    let Some(at_start) = find_comment_at(ctx) else {
        return Vec::new();
    };

    // loc[4] and loc[5] are the start/end byte offsets of the `@` capture group
    let span_length = (ctx.position - at_start).max(0);

    let replacement_range = byte_span_to_range(ctx, at_start, span_length);

    // Collect and sort all codegen names
    let mut names: Vec<&str> = codegens::ALL.iter().map(|c| c.name).collect();
    names.sort_unstable();

    let all_codegens = names.join(",");
    let enable_snippet = format!("${{1|{all_codegens}|}} $0");

    let sort_text = "11";

    vec![make_completion_item(
        "@effect-codegens".to_string(),
        format!("@effect-codegens {enable_snippet}"),
        sort_text,
        replacement_range,
        lsproto::CompletionItemKind::CONSTANT,
    )]
}
