//! Port of Effect-TS/tsgo `internal/rules/global_fetch.go`.

use crate::effect::diag;
use crate::effect::etscore::*;
use crate::effect::rule::*;
use crate::effect::typeparser::*;
use crate::prelude::*;

pub static GLOBAL_FETCH: Rule = Rule {
    name: "globalFetch",
    group: "effectNative",
    description: "Warns when using the global fetch function outside Effect generators instead of the Effect HTTP client",
    default_severity: Severity::Off,
    supported_effect: &["v3", "v4"],
    codes: &[377061],
    run: run_global_fetch_rule,
};

fn run_global_fetch_rule(ctx: &mut RuleContext<'_, '_>) -> Vec<Diagnostic> {
    run_global_fetch(ctx, false)
}

pub static GLOBAL_FETCH_IN_EFFECT: Rule = Rule {
    name: "globalFetchInEffect",
    group: "effectNative",
    description: "Warns when using the global fetch function inside Effect generators instead of the Effect HTTP client",
    default_severity: Severity::Off,
    supported_effect: &["v3", "v4"],
    codes: &[377063],
    run: run_global_fetch_in_effect,
};

fn run_global_fetch_in_effect(ctx: &mut RuleContext<'_, '_>) -> Vec<Diagnostic> {
    run_global_fetch(ctx, true)
}

fn run_global_fetch(ctx: &mut RuleContext<'_, '_>, check_in_effect: bool) -> Vec<Diagnostic> {
    let fetch_symbol =
        ctx.tp
            .checker
            .resolve_name_exported("fetch", Node::NIL, SymbolFlags::VALUE, false);
    if fetch_symbol.is_nil() {
        return Vec::new();
    }

    let mut package_name = "effect/http";
    if ctx.tp.supported_effect_version() == EffectMajorVersion::V3 {
        package_name = "@effect/platform";
    }

    let mut message = diag::This_code_uses_the_global_fetch_function_HTTP_requests_are_represented_through_HttpClient_from_0_effect_globalFetch;
    if check_in_effect {
        message = diag::This_Effect_code_calls_the_global_fetch_function_HTTP_requests_in_Effect_code_are_represented_through_HttpClient_from_0_effect_globalFetchInEffect;
    }

    #[allow(clippy::too_many_arguments)]
    fn walk(
        ctx: &mut RuleContext<'_, '_>,
        diags: &mut Vec<Diagnostic>,
        check_in_effect: bool,
        fetch_symbol: SymbolId,
        message: &'static crate::diagnostics::Message,
        package_name: &str,
        node: Node,
    ) -> bool {
        if node.is_nil() {
            return false;
        }
        if node.kind() == SyntaxKind::CallExpression {
            let in_effect = ctx
                .tp
                .get_effect_context_flags(node)
                .intersects(EffectContextFlags::IN_EFFECT);
            if in_effect == check_in_effect {
                let call = node;
                let sym = ctx.tp.get_symbol_at_location(call.expression());
                if ctx.tp.resolve_to_global_symbol(sym) == fetch_symbol {
                    diags.push(ctx.new_diagnostic(
                        ctx.source_file,
                        get_error_range_for_node(ctx.source_file, call.expression()),
                        message,
                        Vec::new(),
                        vec![package_name.to_string()],
                    ));
                }
            }
        }

        node.for_each_child(|child| {
            walk(
                ctx,
                diags,
                check_in_effect,
                fetch_symbol,
                message,
                package_name,
                child,
            )
        });
        false
    }

    let mut diags = Vec::new();
    let sf = ctx.source_file;
    walk(
        ctx,
        &mut diags,
        check_in_effect,
        fetch_symbol,
        message,
        package_name,
        sf,
    );

    diags
}
