//! Port of Effect-TS/tsgo `internal/rules/prefer_unsafe_constructor.go`.

use crate::effect::diag;
use crate::effect::etscore::*;
use crate::effect::rule::*;
use crate::effect::typeparser::*;
use crate::prelude::*;

/// PreferUnsafeConstructor detects Effect.runSync applied directly to an effect-package
/// constructor call whose module also exports a `<name>Unsafe` sibling producing the same
/// value synchronously (e.g. Effect.runSync(Scope.make()) -> Scope.makeUnsafe()).
// Go: rules/prefer_unsafe_constructor.go PreferUnsafeConstructor
pub static PREFER_UNSAFE_CONSTRUCTOR: Rule = Rule {
    name: "preferUnsafeConstructor",
    group: "antipattern",
    description: "Suggests replacing Effect.runSync of a pure effect constructor with the synchronous *Unsafe variant exported by the same module",
    default_severity: Severity::Suggestion,
    supported_effect: &["v4"],
    codes: &[377109],
    run: run_prefer_unsafe_constructor,
};

fn run_prefer_unsafe_constructor(ctx: &mut RuleContext<'_, '_>) -> Vec<Diagnostic> {
    let sf = ctx.source_file;
    let matches = analyze_prefer_unsafe_constructor(ctx.tp, sf);
    let mut diags = Vec::with_capacity(matches.len());
    for m in &matches {
        let original_text =
            get_source_text_of_node_from_source_file(m.source_file, m.outer_callee, false)
                + "("
                + &get_source_text_of_node_from_source_file(m.source_file, m.inner_callee, false)
                + "(...))";
        let unsafe_text = m.unsafe_callee_text.clone() + "(...)";
        diags.push(ctx.new_diagnostic(
            m.source_file,
            m.location,
            diag::X_0_starts_a_fiber_to_run_a_pure_constructor_Use_the_synchronous_variant_1_instead_effect_preferUnsafeConstructor,
            Vec::new(),
            vec![original_text, unsafe_text],
        ));
    }
    diags
}

/// PreferUnsafeConstructorMatch holds the nodes needed by the diagnostic and quick fix.
// Go: rules/prefer_unsafe_constructor.go PreferUnsafeConstructorMatch
#[derive(Clone, Debug)]
pub struct PreferUnsafeConstructorMatch {
    /// The source file where this match was found
    pub source_file: Node,
    /// Error range on the outer Effect.runSync call
    pub location: TextRange,
    /// The Effect.runSync(...) call expression
    pub outer_call: Node,
    /// The Effect.runSync callee expression
    pub outer_callee: Node,
    /// The constructor call expression (e.g. Scope.make())
    pub inner_call: Node,
    /// The constructor callee expression (e.g. Scope.make)
    pub inner_callee: Node,
    /// The identifier holding the constructor name (e.g. make)
    pub inner_callee_name: Node,
    /// The sibling export name (e.g. makeUnsafe)
    pub unsafe_name: String,
    /// The callee text with the name replaced (e.g. Scope.makeUnsafe)
    pub unsafe_callee_text: String,
}

/// AnalyzePreferUnsafeConstructor finds Effect.runSync calls whose single argument is a
/// direct call to an effect-package constructor with a matching `<name>Unsafe` sibling export.
// Go: rules/prefer_unsafe_constructor.go AnalyzePreferUnsafeConstructor
pub fn analyze_prefer_unsafe_constructor(
    tp: &mut TypeParser<'_>,
    sf: Node,
) -> Vec<PreferUnsafeConstructorMatch> {
    let mut matches = Vec::new();

    fn visit(
        tp: &mut TypeParser<'_>,
        sf: Node,
        matches: &mut Vec<PreferUnsafeConstructorMatch>,
        n: Node,
    ) {
        if n.kind() == SyntaxKind::CallExpression
            && let Some(m) = analyze_prefer_unsafe_constructor_node(tp, sf, n)
        {
            matches.push(m);
        }
    }

    // PERF: Go walks the tree; the node order has the same nodes in the same
    // order (effperf4).
    for &n in tp.node_order(sf).nodes() {
        visit(tp, sf, &mut matches, n);
    }
    matches
}

/// analyzePreferUnsafeConstructorNode checks a single call expression for the
/// Effect.runSync(<effect constructor>()) pattern with a matching *Unsafe sibling.
/// Go returns `(match, bool)`; `None` is Go's `false`.
// Go: rules/prefer_unsafe_constructor.go analyzePreferUnsafeConstructorNode
fn analyze_prefer_unsafe_constructor_node(
    tp: &mut TypeParser<'_>,
    sf: Node,
    node: Node,
) -> Option<PreferUnsafeConstructorMatch> {
    let call = node;
    if call.argument_list().is_nil() || call.arguments().len() != 1 {
        return None;
    }
    if !tp.is_node_reference_to_effect_module_api(call.expression(), member!("runSync")) {
        return None;
    }

    // The argument must itself be a direct constructor call, not a variable or composed effect.
    let inner = skip_parentheses(call.arguments().get(0));
    if inner.is_nil() || inner.kind() != SyntaxKind::CallExpression {
        return None;
    }
    let inner_call = inner;
    let inner_callee = inner_call.expression();

    let name_node = match inner_callee.kind() {
        SyntaxKind::Identifier => inner_callee,
        SyntaxKind::PropertyAccessExpression => inner_callee.name(),
        _ => return None,
    };
    if name_node.is_nil() || name_node.kind() != SyntaxKind::Identifier {
        return None;
    }

    // Spread arguments make the argument list impossible to validate statically.
    if inner_call.argument_list().is_some() {
        for arg in inner_call.arguments().iter() {
            if arg.kind() == SyntaxKind::SpreadElement {
                return None;
            }
        }
    }

    // The constructor call must produce an `Effect<A, never, never>`: a fallible or
    // service-requiring effect has no behavior-preserving synchronous replacement.
    let inner_type = tp.get_type_at_location(inner);
    let eff = tp.effect_type(inner_type)?;
    if eff.a.is_nil() {
        return None;
    }
    if eff.e.is_nil() || !tp.checker.ty(eff.e).flags.intersects(TypeFlags::NEVER) {
        return None;
    }
    if eff.r.is_nil() || !tp.checker.ty(eff.r).flags.intersects(TypeFlags::NEVER) {
        return None;
    }

    let sym = tp.reference_symbol_at_node(inner_callee);
    if sym.is_nil() {
        return None;
    }
    // Use the resolved export name: a named import may alias the local identifier
    // (e.g. `import { make as makeScope }`), and the sibling lives under the export name.
    let name = tp.checker.sym(sym).name.to_string();
    if name.is_empty() || name.ends_with("Unsafe") {
        return None;
    }

    let unsafe_name = name.clone() + "Unsafe";
    let declarations = tp.checker.sym(sym).declarations.clone();
    for decl in declarations.iter().copied() {
        if decl.is_nil() {
            continue;
        }
        let decl_sf = get_source_file_of_node(decl);
        if decl_sf.is_nil()
            || !source_file_info(decl_sf).is_declaration_file
            || !tp.is_source_file_in_package(decl_sf, "effect")
        {
            continue;
        }
        let module_sym = tp.checker.get_symbol_of_declaration(decl_sf);
        if module_sym.is_nil() {
            continue;
        }
        // The callee must be the module-level export of that name, not a nested member
        // (e.g. a method) that happens to be declared in an effect declaration file.
        let export_member = tp
            .checker
            .try_get_member_in_module_exports_and_properties(&name, module_sym);
        let export_sym = resolve_aliased_symbol(tp.checker, export_member);
        if tp
            .checker
            .get_symbol_if_same_reference(export_sym, sym)
            .is_nil()
        {
            continue;
        }
        let sibling_member = tp
            .checker
            .try_get_member_in_module_exports_and_properties(&unsafe_name, module_sym);
        let sibling = resolve_aliased_symbol(tp.checker, sibling_member);
        if sibling.is_nil() {
            continue;
        }
        if !unsafe_sibling_matches_call(tp, inner_call, sibling, eff.a) {
            continue;
        }

        // For a property access like `Scope.make` the sibling stays reachable through the
        // same object, so `Scope.makeUnsafe` both renders in the message and drives the fix.
        // A bare identifier callee has no local binding for the sibling, so only the plain
        // export name is shown and no rename-based fix is possible.
        let mut unsafe_callee_text = unsafe_name.clone();
        if inner_callee.kind() == SyntaxKind::PropertyAccessExpression {
            let callee_text = get_source_text_of_node_from_source_file(sf, inner_callee, false);
            let name_text = name_node.text();
            unsafe_callee_text = callee_text
                .strip_suffix(name_text)
                .unwrap_or(&callee_text)
                .to_string()
                + &unsafe_name;
        }
        return Some(PreferUnsafeConstructorMatch {
            source_file: sf,
            location: get_error_range_for_node(sf, node),
            outer_call: node,
            outer_callee: call.expression(),
            inner_call: inner,
            inner_callee,
            inner_callee_name: name_node,
            unsafe_name,
            unsafe_callee_text,
        });
    }

    None
}

/// unsafeSiblingMatchesCall reports whether some call signature of the sibling symbol is
/// applicable to the constructor call's exact argument list and produces a value usable
/// where the runSync result flows, so the rewrite stays well typed.
// Go: rules/prefer_unsafe_constructor.go unsafeSiblingMatchesCall
fn unsafe_sibling_matches_call(
    tp: &mut TypeParser<'_>,
    inner_call: Node,
    sibling: SymbolId,
    success_type: TypeId,
) -> bool {
    let sibling_type = tp.checker.get_type_of_symbol(sibling);
    if sibling_type.is_nil() {
        return false;
    }

    let resolved = tp.checker.get_resolved_signature_exported(inner_call);
    if resolved.is_nil() {
        return false;
    }

    let mut args: Vec<Node> = Vec::new();
    if inner_call.argument_list().is_some() {
        args = inner_call.arguments().to_vec();
    }

    for sig in tp
        .checker
        .get_signatures_of_type_exported(sibling_type, SignatureKind::CALL)
    {
        if sig.is_nil() || !signature_accepts_arguments(tp, sig, &args) {
            continue;
        }
        // Relate the applicable signature alone: calling the sibling where
        // `(constructor params) => <success type>` is expected. This instantiates
        // generic siblings against the concrete argument and result types.
        let sig_type_parameters = tp.checker.sig(sig).type_parameters().to_vec();
        let sig_this_parameter = tp.checker.sig(sig).this_parameter();
        let sig_parameters = tp.checker.sig(sig).parameters().to_vec();
        let sig_return_type = tp.checker.get_return_type_of_signature_exported(sig);
        let sig_fn_type = tp.checker.new_function_type(
            &sig_type_parameters,
            sig_this_parameter,
            &sig_parameters,
            sig_return_type,
        );
        let resolved_parameters = tp.checker.sig(resolved).parameters().to_vec();
        let expected =
            tp.checker
                .new_function_type(&[], SymbolId::NIL, &resolved_parameters, success_type);
        if !tp.checker.is_type_assignable_to(sig_fn_type, expected) {
            continue;
        }
        // Concrete returns must also be assignable in the other direction so the
        // rewrite cannot change the expression's inferred type. Generic siblings are
        // accepted on the relation above alone: every *Unsafe sibling in the pinned
        // effect package mirrors its constructor's instantiation, and the real
        // exceptions are all rejected by the argument or relation checks.
        if tp.checker.sig(sig).type_parameters().is_empty() {
            let ret = tp.checker.get_return_type_of_signature_exported(sig);
            if !returns_mutually_assignable(tp.checker, ret, success_type) {
                continue;
            }
        }
        return true;
    }
    false
}

/// signatureAcceptsArguments reports whether the signature is applicable to the exact
/// argument list of the constructor call: compatible arity and every argument type
/// accepted by its parameter (falling back to the constraint for type parameters).
// Go: rules/prefer_unsafe_constructor.go signatureAcceptsArguments
fn signature_accepts_arguments(tp: &mut TypeParser<'_>, sig: SignatureId, args: &[Node]) -> bool {
    let params = tp.checker.sig(sig).parameters().to_vec();
    let min_argument_count = tp.checker.sig(sig).min_argument_count();
    let has_rest_parameter = tp.checker.sig(sig).has_rest_parameter();
    if (args.len() as i32) < min_argument_count {
        return false;
    }
    if args.len() > params.len() && !has_rest_parameter {
        return false;
    }
    for (i, &arg) in args.iter().enumerate() {
        if has_rest_parameter && i as i64 >= params.len() as i64 - 1 {
            // Rest arguments are constrained by the whole-signature relation instead.
            break;
        }
        if i >= params.len() {
            return false;
        }
        let param_type = tp.checker.get_type_of_symbol(params[i]);
        let arg_type = tp.get_type_at_location(arg);
        if param_type.is_nil()
            || arg_type.is_nil()
            || !type_accepts_argument_value(tp.checker, arg_type, param_type)
        {
            return false;
        }
    }
    true
}

/// typeAcceptsArgumentValue reports whether a value of argType can be passed where
/// paramType is expected, treating unconstrained type parameters as accepting anything.
// Go: rules/prefer_unsafe_constructor.go typeAcceptsArgumentValue
fn type_accepts_argument_value(c: &mut Checker, arg_type: TypeId, param_type: TypeId) -> bool {
    if c.ty(param_type).flags.intersects(TypeFlags::TYPE_PARAMETER) {
        let constraint = c.get_constraint_of_type_parameter_exported(param_type);
        return constraint.is_nil() || type_accepts_argument_value(c, arg_type, constraint);
    }
    c.is_type_assignable_to(arg_type, param_type)
}

/// returnsMutuallyAssignable reports whether a concrete sibling return type and the
/// Effect success type denote the same value type: assignable in both directions, so
/// the rewrite cannot narrow or widen the expression's inferred type.
// Go: rules/prefer_unsafe_constructor.go returnsMutuallyAssignable
fn returns_mutually_assignable(c: &mut Checker, ret: TypeId, success_type: TypeId) -> bool {
    if ret.is_nil() {
        return false;
    }
    c.is_type_assignable_to(ret, success_type) && c.is_type_assignable_to(success_type, ret)
}

/// resolveAliasedSymbol follows import/export aliases to the original symbol.
// Go: rules/prefer_unsafe_constructor.go resolveAliasedSymbol
fn resolve_aliased_symbol(c: &mut Checker, mut sym: SymbolId) -> SymbolId {
    while sym.is_some() && c.sym_flags(sym).intersects(SymbolFlags::ALIAS) {
        sym = c.get_aliased_symbol(sym);
    }
    sym
}
