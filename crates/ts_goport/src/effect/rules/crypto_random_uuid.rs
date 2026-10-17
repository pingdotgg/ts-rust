//! Port of Effect-TS/tsgo `internal/rules/crypto_random_uuid.go`.

use crate::diagnostics::Message;
use crate::effect::diag;
use crate::effect::etscore::*;
use crate::effect::rule::*;
use crate::effect::typeparser::*;
use crate::prelude::*;

// Go: rules/crypto_random_uuid.go CryptoRandomUUID
pub static CRYPTO_RANDOM_UUID: Rule = Rule {
    name: "cryptoRandomUUID",
    group: "effectNative",
    description: "Warns when using crypto.randomUUID() outside Effect generators instead of the Effect Crypto module",
    default_severity: Severity::Off,
    supported_effect: &["v4"],
    codes: &[377078],
    run: run_crypto_random_uuid_rule,
};

fn run_crypto_random_uuid_rule(ctx: &mut RuleContext<'_, '_>) -> Vec<Diagnostic> {
    run_crypto_random_uuid(ctx, false)
}

// Go: rules/crypto_random_uuid.go CryptoRandomUUIDInEffect
pub static CRYPTO_RANDOM_UUID_IN_EFFECT: Rule = Rule {
    name: "cryptoRandomUUIDInEffect",
    group: "effectNative",
    description: "Warns when using crypto.randomUUID() inside Effect generators instead of the Effect Crypto module",
    default_severity: Severity::Off,
    supported_effect: &["v4"],
    codes: &[377079],
    run: run_crypto_random_uuid_in_effect,
};

fn run_crypto_random_uuid_in_effect(ctx: &mut RuleContext<'_, '_>) -> Vec<Diagnostic> {
    run_crypto_random_uuid(ctx, true)
}

// Go: rules/crypto_random_uuid.go runCryptoRandomUUID
fn run_crypto_random_uuid(ctx: &mut RuleContext<'_, '_>, check_in_effect: bool) -> Vec<Diagnostic> {
    let mut message = diag::This_code_uses_crypto_randomUUID_prefer_the_Effect_Crypto_module_instead_effect_cryptoRandomUUID;
    if check_in_effect {
        message = diag::This_Effect_code_uses_crypto_randomUUID_prefer_the_Effect_Crypto_module_instead_effect_cryptoRandomUUIDInEffect;
    }

    let mut diags = Vec::new();

    // PORT: the body of Go's recursive `walk` closure, for one node.
    fn visit(
        ctx: &mut RuleContext<'_, '_>,
        diags: &mut Vec<Diagnostic>,
        message: &'static Message,
        check_in_effect: bool,
        node: Node,
    ) {
        if node.kind() == SyntaxKind::CallExpression {
            let call = node;
            let in_effect = ctx
                .tp
                .get_effect_context_flags(node)
                .intersects(EffectContextFlags::IN_EFFECT);
            if in_effect == check_in_effect
                && ctx.tp.is_node_reference_to_global_member(
                    call.expression(),
                    "crypto",
                    "randomUUID",
                )
            {
                diags.push(ctx.new_diagnostic(
                    ctx.source_file,
                    get_error_range_for_node(ctx.source_file, node),
                    message,
                    Vec::new(),
                    Vec::new(),
                ));
            }
        }
    }

    let sf = ctx.source_file;
    // PERF: Go walks the tree; the node order has the same nodes in the same
    // order (effperf4).
    for &n in ctx.tp.node_order(sf).nodes() {
        visit(ctx, &mut diags, message, check_in_effect, n);
    }

    diags
}
