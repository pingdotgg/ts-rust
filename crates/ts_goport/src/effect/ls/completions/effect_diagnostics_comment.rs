//! Port of Effect-TS/tsgo `internal/completions/effect_diagnostics_comment.go`
//! (effect-tsgo 0.51.1), with the comment `@` match that the codegens and
//! JSDoc comment completions use too.

use super::effect_schema_self_in_classes::{byte_span_to_range, make_completion_item};
use crate::effect::ls::completion::{Completion, CompletionContext};
use crate::effect::rules;
use crate::ls::prelude::*;

// Go: completions/effect_diagnostics_comment.go:16 effectDiagnosticsComment
/// effectDiagnosticsComment provides completions for @effect-diagnostics and
/// @effect-diagnostics-next-line directives when typing @ inside a comment.
pub static EFFECT_DIAGNOSTICS_COMMENT: Completion = Completion {
    name: "effectDiagnosticsComment",
    description: "Provides @effect-diagnostics completion inside comments",
    run: run_effect_diagnostics_comment,
};

// Go: completions/effect_diagnostics_comment.go:23 commentAtRegex
/// commentAtRegex matches `//`, `/*`, or `/**` followed by optional whitespace and `@` at end of string.
/// Returns the byte offset of the `@` (Go `loc[4]` of
/// `FindStringSubmatchIndex`), or None when there is no match.
// PORT: Go `regexp.MustCompile(`(//|/\*\*?)\s*(@)\s*$`)`. The Go regexp port
// (`gostd::regexp`) has no capture positions, so this matches by hand. RE2
// `\s` is `[\t\n\f\r ]` and `$` (no `m` flag) is the end of the text. The
// `@` group cannot hold `\s`, and the final `\s*$` takes only the trailing
// whitespace, so a match exists only when the last byte that is not `\s` is
// `@` and the text before it, without its trailing `\s`, ends with the
// opener (`//`, `/*` or `/**`). The `@` group then starts at that byte. The
// match runs on bytes, as Go's `sourceText[:ctx.Position]` does.
pub fn comment_at_regex_find(text_before_cursor: &[u8]) -> Option<usize> {
    let is_space = |b: &u8| matches!(b, b'\t' | b'\n' | b'\x0c' | b'\r' | b' ');
    let at = text_before_cursor.iter().rposition(|b| !is_space(b))?;
    if text_before_cursor[at] != b'@' {
        return None;
    }
    let opener_end = text_before_cursor[..at]
        .iter()
        .rposition(|b| !is_space(b))
        .map_or(0, |i| i + 1);
    let before = &text_before_cursor[..opener_end];
    if before.ends_with(b"//") || before.ends_with(b"/*") || before.ends_with(b"/**") {
        return Some(at);
    }
    None
}

/// Go `sourceText[:ctx.Position]` as bytes, then `commentAtRegex.FindStringSubmatchIndex`:
/// the byte offset of the `@` group start, or None.
// PORT: Go's slice panics on a position past the text; the LSP converter
// never gives one, and the port clamps instead of panicking.
pub(super) fn find_comment_at(ctx: &CompletionContext<'_, '_>) -> Option<i32> {
    let source_text = source_file_text(ctx.source_file);
    let bytes = source_text.as_bytes();
    let end = usize::try_from(ctx.position).unwrap_or(0).min(bytes.len());
    comment_at_regex_find(&bytes[..end]).map(|at| i32::try_from(at).unwrap_or(i32::MAX))
}

// Go: completions/effect_diagnostics_comment.go:25 runEffectDiagnosticsComment
fn run_effect_diagnostics_comment(
    ctx: &mut CompletionContext<'_, '_>,
) -> Vec<lsproto::CompletionItem> {
    let Some(at_start) = find_comment_at(ctx) else {
        return Vec::new();
    };

    // loc[4] and loc[5] are the start/end byte offsets of the `@` capture group
    let span_length = (ctx.position - at_start).max(0);

    let replacement_range = byte_span_to_range(ctx, at_start, span_length);

    // Collect and sort all diagnostic rule names
    let mut names: Vec<&str> = rules::ALL.iter().map(|r| r.name).collect();
    names.sort_unstable();

    let all_diagnostics = names.join(",");
    let disable_snippet =
        format!("${{1|{all_diagnostics}|}}:${{2|off,warning,error,message,suggestion|}}$0");

    let sort_text = "11";

    vec![
        make_completion_item(
            "@effect-diagnostics".to_string(),
            format!("@effect-diagnostics {disable_snippet}"),
            sort_text,
            replacement_range,
            lsproto::CompletionItemKind::CONSTANT,
        ),
        make_completion_item(
            "@effect-diagnostics-next-line".to_string(),
            format!("@effect-diagnostics-next-line {disable_snippet}"),
            sort_text,
            replacement_range,
            lsproto::CompletionItemKind::CONSTANT,
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::comment_at_regex_find;

    // The hand match against Go's `(//|/\*\*?)\s*(@)\s*$` on the texts of the
    // Go comment tests and a few edge cases (Go `loc[4]`, or no match).
    #[test]
    fn comment_at_matches_the_go_pattern() {
        let cases: &[(&str, Option<usize>)] = &[
            ("// @", Some(3)),
            ("/* @", Some(3)),
            ("/** @", Some(4)),
            ("//  @  ", Some(4)),
            ("const x = 1\n// @", Some(15)),
            ("//@", Some(2)),
            ("/*@\n\t ", Some(2)),
            ("// some comment without at", None),
            ("const x = @", None),
            ("@", None),
            ("", None),
            ("/ @", None),
            ("*/ @", None),
            ("x** @", None),
            ("// @ x", None),
            ("// @@", None),
            ("// @ // @", Some(8)),
            ("// \u{b}@", None),
        ];
        for &(text, want) in cases {
            assert_eq!(
                comment_at_regex_find(text.as_bytes()),
                want,
                "text {text:?}"
            );
        }
    }
}
