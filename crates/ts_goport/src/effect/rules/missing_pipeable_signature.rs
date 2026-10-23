//! Port of Effect-TS/tsgo `internal/rules/missing_pipeable_signature.go`.

use crate::effect::diag;
use crate::effect::etscore::*;
use crate::effect::rule::*;
use crate::effect::typeparser::*;
use crate::prelude::*;

// Go: rules/missing_pipeable_signature.go MissingPipeableSignature
pub static MISSING_PIPEABLE_SIGNATURE: Rule = Rule {
    name: "missingPipeableSignature",
    group: "style",
    description: "Reports exported fixed-arity functions whose call signatures have no corresponding pipeable overload",
    default_severity: Severity::Off,
    supported_effect: &["v3", "v4"],
    codes: &[377101],
    run: check_missing_pipeable_signatures,
};

// Go: rules/missing_pipeable_signature.go checkMissingPipeableSignatures
fn check_missing_pipeable_signatures(ctx: &mut RuleContext<'_, '_>) -> Vec<Diagnostic> {
    let module_symbol = ctx.tp.checker.get_symbol_of_declaration(ctx.source_file);
    if module_symbol.is_nil() {
        return Vec::new();
    }

    let mut diagnostics = Vec::new();
    let exports = ctx.tp.checker.get_exports_of_module_exported(module_symbol);
    for export_symbol in exports {
        let (target, location) = local_export_target(ctx, export_symbol);
        if target.is_nil() || location.is_nil() {
            continue;
        }

        let export_type = ctx
            .tp
            .checker
            .get_type_of_symbol_at_location(target, location);
        if export_type.is_nil() {
            continue;
        }
        let signatures = ctx
            .tp
            .checker
            .get_signatures_of_type_exported(export_type, SignatureKind::CALL);
        if signatures.is_empty() {
            continue;
        }

        let mut pipeable_targets: FxHashMap<SignatureId, bool> = FxHashMap::default();
        let mut has_pipeable_signature: FxHashMap<SignatureId, bool> = FxHashMap::default();
        for &data_first in signatures.iter() {
            if !is_eligible_data_first_signature(ctx.tp.checker, data_first) {
                continue;
            }
            let params_len = ctx.tp.checker.sig(data_first).parameters().len() as i32;
            for subject_index in [0, params_len - 1] {
                for &candidate in signatures.iter() {
                    if candidate == data_first {
                        continue;
                    }
                    if matches_pipeable_signature(
                        ctx.tp.checker,
                        data_first,
                        candidate,
                        subject_index,
                        None,
                    ) {
                        has_pipeable_signature.insert(data_first, true);
                        pipeable_targets.insert(candidate, true);
                    }
                }
            }
        }

        for &signature in signatures.iter() {
            if !is_eligible_data_first_signature(ctx.tp.checker, signature)
                || pipeable_targets.get(&signature).copied().unwrap_or(false)
                || has_pipeable_signature
                    .get(&signature)
                    .copied()
                    .unwrap_or(false)
            {
                continue;
            }
            let export_name = ctx.tp.checker.sym(export_symbol).name.as_str().to_string();
            let signature_text = ctx.tp.checker.signature_to_string_ex(
                signature,
                location,
                TypeFormatFlags::WRITE_ARROW_STYLE_SIGNATURE,
                None,
            );
            diagnostics.push(ctx.new_diagnostic(
                ctx.source_file,
                ctx.get_error_range(location),
                diag::Exported_function_0_has_no_pipeable_overload_corresponding_to_its_signature_1_effect_missingPipeableSignature,
                Vec::new(),
                args![export_name, signature_text],
            ));
        }
    }

    diagnostics
}

// Go: rules/missing_pipeable_signature.go localExportTarget
fn local_export_target(ctx: &mut RuleContext<'_, '_>, export_symbol: SymbolId) -> (SymbolId, Node) {
    if export_symbol.is_nil() {
        return (SymbolId::NIL, Node::NIL);
    }

    let mut target = export_symbol;
    if ctx
        .tp
        .checker
        .sym_flags(target)
        .intersects(SymbolFlags::ALIAS)
    {
        target = ctx.tp.checker.get_aliased_symbol(target);
    }
    if target.is_nil() {
        return (SymbolId::NIL, Node::NIL);
    }

    let mut target_declaration = Node::NIL;
    for &declaration in ctx.tp.checker.sym(target).declarations.iter() {
        if declaration.is_some() && get_source_file_of_node(declaration) == ctx.source_file {
            target_declaration = declaration;
            break;
        }
    }
    if target_declaration.is_nil() {
        return (SymbolId::NIL, Node::NIL);
    }

    let mut location = get_name_of_declaration(ctx.tp.checker.sym(target).value_declaration);
    if location.is_nil() {
        location = get_name_of_declaration(target_declaration);
    }
    if location.is_nil() {
        location = target_declaration;
    }
    (target, location)
}

// Go: rules/missing_pipeable_signature.go isEligibleDataFirstSignature
fn is_eligible_data_first_signature(c: &Checker, signature: SignatureId) -> bool {
    signature.is_some()
        && !c.sig(signature).has_rest_parameter()
        && c.sig(signature).parameters().len() >= 2
}
