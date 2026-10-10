//! Port of Effect-TS/tsgo `internal/rules/stability_api_usage.go` at
//! `@effect/tsgo@0.51.1` (`47cb1ed7`): `experimentalApiUsage` and
//! `unstableApiUsage`, with the `allowedUnstableApis` and
//! `allowedExperimentalApis` allow lists.

use crate::effect::diag;
use crate::effect::etscore::*;
use crate::effect::rule::*;
use crate::effect::typeparser::*;
use crate::frontend::tspath::path::{get_base_file_name, normalize_path};
use crate::prelude::*;

pub static EXPERIMENTAL_API_USAGE: Rule = Rule {
    name: "experimentalApiUsage",
    group: "correctness",
    description: "Warns when using an API marked @stability experimental",
    default_severity: Severity::Warning,
    supported_effect: &["v4"],
    codes: &[377135],
    run: run_experimental_api_usage,
};

fn run_experimental_api_usage(ctx: &mut RuleContext<'_, '_>) -> Vec<Diagnostic> {
    run_stability_api_usage(ctx, "experimental")
}

pub static UNSTABLE_API_USAGE: Rule = Rule {
    name: "unstableApiUsage",
    group: "correctness",
    description: "Warns when using an API marked @stability unstable",
    default_severity: Severity::Warning,
    supported_effect: &["v4"],
    codes: &[377136],
    run: run_unstable_api_usage,
};

fn run_unstable_api_usage(ctx: &mut RuleContext<'_, '_>) -> Vec<Diagnostic> {
    run_stability_api_usage(ctx, "unstable")
}

/// Go `declarationStabilityInfo`.
#[derive(Clone, Copy, Default)]
struct DeclarationStabilityInfo {
    stability: &'static str,
    declaration: Node,
}

// Declared stability is cached per checker in TypeParser, so references to the
// same symbol or selected overload reuse one lookup across source files. The
// selected overload's own tag always wins over the symbol-level tag.
fn read_symbol(ctx: &mut RuleContext<'_, '_>, symbol: SymbolId) -> DeclarationStabilityInfo {
    if symbol.is_nil() {
        return DeclarationStabilityInfo::default();
    }
    let info = ctx.tp.declared_api_stability_of_symbol(symbol);
    DeclarationStabilityInfo {
        stability: api_stability_level_tag(info.level),
        declaration: info.declaration,
    }
}

fn read_signature(
    ctx: &mut RuleContext<'_, '_>,
    signature: SignatureId,
) -> DeclarationStabilityInfo {
    if signature.is_nil() {
        return DeclarationStabilityInfo::default();
    }
    let info = ctx.tp.declared_api_stability_of_signature(signature);
    DeclarationStabilityInfo {
        stability: api_stability_level_tag(info.level),
        declaration: info.declaration,
    }
}

// Go: rules/stability_api_usage.go runStabilityApiUsage
fn run_stability_api_usage(ctx: &mut RuleContext<'_, '_>, wanted: &'static str) -> Vec<Diagnostic> {
    let mut allow = StabilityApiAllowlist::new(ctx, wanted);
    let mut diagnostics = Vec::new();
    let sf = ctx.source_file;
    walk(ctx, &mut allow, &mut diagnostics, wanted, sf);
    diagnostics
}

fn walk(
    ctx: &mut RuleContext<'_, '_>,
    allow: &mut StabilityApiAllowlist,
    diagnostics: &mut Vec<Diagnostic>,
    wanted: &'static str,
    node: Node,
) -> bool {
    if node.is_nil() {
        return false;
    }
    if node.kind() == SyntaxKind::Identifier && !is_declaration_name_or_import_property_name(node) {
        // The selected overload is authoritative for calls. A tagged overload
        // may differ from other declarations of the same symbol.
        let mut stability = DeclarationStabilityInfo::default();
        let mut selected_declaration = Node::NIL;
        let mut callee = node;
        let parent = node.parent();
        if parent.is_some()
            && parent.kind() == SyntaxKind::PropertyAccessExpression
            && parent.name() == node
        {
            callee = parent;
        }
        let parent = callee.parent();
        if parent.is_some()
            && matches!(
                parent.kind(),
                SyntaxKind::CallExpression | SyntaxKind::NewExpression
            )
            && parent.expression() == callee
        {
            let signature = ctx.tp.checker.get_resolved_signature_exported(parent);
            if signature.is_some() && ctx.tp.checker.sig(signature).declaration().is_some() {
                selected_declaration = ctx.tp.checker.sig(signature).declaration();
                stability = read_signature(ctx, signature);
                if stability.stability.is_empty() {
                    // A signature with no own tag still carries the selected
                    // declaration so the symbol fallback can be suppressed for it.
                    stability = DeclarationStabilityInfo {
                        stability: "",
                        declaration: selected_declaration,
                    };
                }
            }
        }
        let symbol = ctx.tp.checker.get_symbol_at_location_exported(node);
        let resolved_symbol = ctx.tp.reference_symbol_at_node(node);
        let use_symbol = selected_declaration.is_nil()
            || !symbol_has_declaration(ctx.tp.checker, symbol, selected_declaration)
                && !symbol_has_declaration(ctx.tp.checker, resolved_symbol, selected_declaration);
        if stability.stability.is_empty() && use_symbol {
            stability = read_symbol(ctx, symbol);
        }
        if stability.stability.is_empty() && use_symbol {
            stability = read_symbol(ctx, resolved_symbol);
        }
        if stability.stability == wanted {
            let mut name = node.text().to_string();
            let (allowed, api_name) = allow.decide(ctx, stability.declaration);
            if allowed {
                return false;
            }
            if !api_name.is_empty() {
                name = api_name;
            }
            let mut message = diag::X_0_is_an_unstable_API_Breaking_changes_may_happen_between_versions_effect_unstableApiUsage;
            if wanted == "experimental" {
                message = diag::X_0_is_an_experimental_API_effect_experimentalApiUsage;
            }
            diagnostics.push(ctx.new_diagnostic(
                ctx.source_file,
                ctx.get_error_range(node),
                message,
                Vec::new(),
                vec![name],
            ));
        }
    }
    node.for_each_child(|child| walk(ctx, allow, diagnostics, wanted, child));
    false
}

// Go: rules/stability_api_usage.go symbolHasDeclaration
fn symbol_has_declaration(c: &Checker, symbol: SymbolId, declaration: Node) -> bool {
    if symbol.is_nil() || declaration.is_nil() {
        return false;
    }
    c.sym(symbol).declarations.contains(&declaration)
}

// Go: rules/stability_api_usage.go stabilitySymbolIsModuleExport
/// stabilitySymbolIsModuleExport reports whether symbol is exported from the
/// module under its own name.
pub fn stability_symbol_is_module_export(
    c: &mut Checker,
    symbol: SymbolId,
    module_symbol: SymbolId,
) -> bool {
    if symbol.is_nil() || module_symbol.is_nil() {
        return false;
    }
    let name = c.sym(symbol).name.clone();
    c.try_get_member_in_module_exports_and_properties(&name, module_symbol) == symbol
}

// Go: rules/stability_api_usage.go stabilityOwningExportSymbol
/// stabilityOwningExportSymbol walks out from a selected declaration to the
/// nearest enclosing declaration that is exported from the module. A call or
/// construct signature of a dual export has its own `__call` symbol, so the
/// export it belongs to can only be found by climbing to the annotated
/// declaration (for example the variable the object type is assigned to).
pub fn stability_owning_export_symbol(
    c: &mut Checker,
    declaration: Node,
    module_symbol: SymbolId,
) -> SymbolId {
    let mut node = declaration.parent();
    while node.is_some() {
        let mut symbol = c.get_symbol_of_declaration(node);
        if symbol.is_some() {
            if c.sym(symbol).export_symbol.is_some() {
                symbol = c.sym(symbol).export_symbol;
            }
            if stability_symbol_is_module_export(c, symbol, module_symbol) {
                return symbol;
            }
        }
        node = node.parent();
    }
    SymbolId::NIL
}

/// Go `newStabilityApiAllowlist` (the returned closure and its maps).
/// Decisions belong to one rule invocation: per-file overrides can change the
/// allow-list, and the tagged declaration distinguishes overloads and reexports.
struct StabilityApiAllowlist {
    decisions: FxHashMap<Node, (bool, String)>,
    modules: FxHashMap<Node, String>,
    entries: Vec<String>,
}

impl StabilityApiAllowlist {
    // Go: rules/stability_api_usage.go newStabilityApiAllowlist
    fn new(ctx: &RuleContext<'_, '_>, wanted: &str) -> Self {
        let entries = if wanted == "experimental" {
            ctx.options.allowed_experimental_apis.clone()
        } else {
            ctx.options.allowed_unstable_apis.clone()
        };
        StabilityApiAllowlist {
            decisions: FxHashMap::default(),
            modules: FxHashMap::default(),
            entries,
        }
    }

    /// The Go closure: whether `declaration` is allowed, and the API name the
    /// diagnostic uses ("" for the identifier text).
    fn decide(&mut self, ctx: &mut RuleContext<'_, '_>, declaration: Node) -> (bool, String) {
        if let Some(cached) = self.decisions.get(&declaration) {
            return cached.clone();
        }
        let result = self.decide_uncached(ctx, declaration);
        self.decisions.insert(declaration, result.clone());
        result
    }

    fn decide_uncached(
        &mut self,
        ctx: &mut RuleContext<'_, '_>,
        declaration: Node,
    ) -> (bool, String) {
        if declaration.is_nil() {
            return (false, String::new());
        }
        let sf = get_source_file_of_node(declaration);
        if sf.is_nil() {
            return (false, String::new());
        }
        let module_name = if let Some(name) = self.modules.get(&sf) {
            name.clone()
        } else {
            let mut module_name = String::new();
            if let Some(pkg) = ctx.tp.package_json_for_source_file(sf) {
                let (package_name, ok) = pkg.fields.name.get_value();
                if ok && !package_name.is_empty() {
                    let directory =
                        super::deterministic_keys::get_package_json_directory(ctx.program, sf);
                    module_name = stability_api_module_name(
                        &package_name,
                        &directory,
                        source_file_file_name(sf),
                    );
                }
            }
            self.modules.insert(sf, module_name.clone());
            module_name
        };
        if module_name.is_empty() {
            return (false, String::new());
        }
        let c = &mut *ctx.tp.checker;
        let module_symbol = c.get_symbol_of_declaration(sf);
        let mut symbol = c.get_symbol_of_declaration(declaration);
        if symbol.is_some() && c.sym(symbol).export_symbol.is_some() {
            symbol = c.sym(symbol).export_symbol;
        }
        // A selected overload (for example a call signature of a dual export)
        // carries its own `__call` symbol rather than the export's, so it is not
        // a module export. Resolve the enclosing exported declaration instead so
        // the name and the per-export allow-list match stay export-scoped.
        if module_symbol.is_some() && !stability_symbol_is_module_export(c, symbol, module_symbol) {
            let owner = stability_owning_export_symbol(c, declaration, module_symbol);
            if owner.is_some() {
                symbol = owner;
            }
        }
        let mut allowed = false;
        let mut name = module_name.clone();
        if module_symbol.is_some() && symbol.is_some() {
            let symbol_name = c.sym(symbol).name.clone();
            let exported =
                c.try_get_member_in_module_exports_and_properties(&symbol_name, module_symbol);
            if exported == symbol {
                name.push('#');
                name.push_str(&symbol_name);
            }
        }
        for entry in &self.entries {
            let (module, member, has_member) = match entry.split_once('#') {
                Some((module, member)) => (module, member, true),
                None => (entry.as_str(), "", false),
            };
            if !has_member {
                if !module.is_empty()
                    && (module_name == module
                        || module_name
                            .strip_prefix(module)
                            .is_some_and(|rest| rest.starts_with('/')))
                {
                    allowed = true;
                    break;
                }
            } else if module == module_name
                && !member.is_empty()
                && module_symbol.is_some()
                && symbol.is_some()
            {
                let exported =
                    c.try_get_member_in_module_exports_and_properties(member, module_symbol);
                if exported.is_some() {
                    if exported == symbol {
                        allowed = true;
                        break;
                    }
                    let left = resolve_stability_alias(c, exported);
                    let right = resolve_stability_alias(c, symbol);
                    if c.get_symbol_if_same_reference(left, right).is_some() {
                        allowed = true;
                        break;
                    }
                }
            }
        }
        (allowed, name)
    }
}

// Go: rules/stability_api_usage.go stabilityApiModuleName
/// This is a declaration path, not a reconstructed public import specifier.
pub fn stability_api_module_name(package_name: &str, directory: &str, file_name: &str) -> String {
    if directory.is_empty() {
        return String::new();
    }
    let directory = format!("{}/", trim_suffix(&normalize_path(directory), "/"));
    let file_name = normalize_path(file_name);
    let Some(mut relative) = file_name.strip_prefix(directory.as_str()) else {
        return String::new();
    };
    for prefix in ["dist/dts/", "dist/esm/", "dist/cjs/", "src/", "dist/"] {
        if let Some(rest) = relative.strip_prefix(prefix) {
            relative = rest;
            break;
        }
    }
    for extension in [
        ".d.ts", ".d.mts", ".d.cts", ".ts", ".mts", ".cts", ".tsx", ".js", ".mjs", ".cjs", ".jsx",
    ] {
        if let Some(rest) = relative.strip_suffix(extension) {
            relative = rest;
            break;
        }
    }
    if get_base_file_name(relative) == "index" {
        relative = trim_suffix(trim_suffix(relative, "index"), "/");
    }
    if relative.is_empty() {
        return package_name.to_string();
    }
    format!("{package_name}/{relative}")
}

/// Go `strings.TrimSuffix`: removes one copy of the suffix.
fn trim_suffix<'a>(s: &'a str, suffix: &str) -> &'a str {
    s.strip_suffix(suffix).unwrap_or(s)
}

// Go: rules/stability_api_usage.go resolveStabilityAlias
/// Synthetic default aliases have no alias declaration and cannot be resolved.
pub fn resolve_stability_alias(c: &mut Checker, symbol: SymbolId) -> SymbolId {
    let mut symbol = symbol;
    let mut seen: FxHashSet<SymbolId> = FxHashSet::default();
    while symbol.is_some() && c.sym(symbol).flags.intersects(SymbolFlags::ALIAS) {
        if !seen.insert(symbol) {
            return SymbolId::NIL;
        }
        if !c
            .sym(symbol)
            .declarations
            .iter()
            .any(|&declaration| is_alias_symbol_declaration(declaration))
        {
            return SymbolId::NIL;
        }
        let next = api_stability_immediate_aliased_symbol(c, symbol);
        if next == symbol {
            break;
        }
        symbol = next;
    }
    symbol
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Go: rules/stability_api_usage_test.go TestStabilityApiModuleName.
    #[test]
    fn stability_api_module_name_matches_go() {
        for (file, want) in [
            ("/pkg/src/http/HttpClient.ts", "effect/http/HttpClient"),
            ("/pkg/dist/http/HttpClient.d.ts", "effect/http/HttpClient"),
            (
                "/pkg/dist/dts/http/HttpClient.d.mts",
                "effect/http/HttpClient",
            ),
            (
                "/pkg/dist/cjs/http/HttpClient.d.cts",
                "effect/http/HttpClient",
            ),
            ("/pkg/dist/http/index.d.ts", "effect/http"),
            ("/pkg/index.ts", "effect"),
            ("/pkg/types/client.d.ts", "effect/types/client"),
            ("/pkg-other/client.ts", ""),
        ] {
            assert_eq!(
                stability_api_module_name("effect", "/pkg", file),
                want,
                "{file}"
            );
        }
    }
}
