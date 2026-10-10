//! Port of Effect-TS/tsgo `internal/rules/api_stability_leak.go` at
//! `@effect/tsgo@0.51.1` (`47cb1ed7`): `apiStabilityLeak`, an opt-in rule in
//! the `maintainers` group.
//!
//! PORT: Go's walker holds `ctx`; here the walker holds only its own state and
//! its methods take `ctx`, because the session and the rule context both use
//! the type parser.

use super::deterministic_keys::get_package_json_directory;
use super::stability_api_usage::{resolve_stability_alias, stability_api_module_name};
use crate::effect::diag;
use crate::effect::etscore::*;
use crate::effect::rule::*;
use crate::effect::typeparser::*;
use crate::prelude::*;

/// ApiStabilityLeak reports exported APIs whose public surface exposes a type
/// that is marked with a lower stability than the export itself. A stable export
/// may not expose unstable or experimental types, and an unstable export may not
/// expose experimental types. Experimental exports impose no restriction.
pub static API_STABILITY_LEAK: Rule = Rule {
    name: "apiStabilityLeak",
    group: "maintainers",
    description: "Reports exported APIs whose public surface exposes a less stable type",
    default_severity: Severity::Off,
    supported_effect: &["v4"],
    codes: &[377137, 377138],
    run: check_api_stability_leaks,
};

// Go: rules/api_stability_leak.go checkApiStabilityLeaks
/// checkApiStabilityLeaks compares each export's declared stability ceiling with
/// the actual stability of its public surface. Every export is inspected in its
/// own ApiStabilitySession, so the export's type and signatures share one
/// traversal and unpublishable results stay local to it. Declared stability
/// lookups persist per checker, and a complete, settled, context-free concrete
/// type or signature surface is reused by later exports, files and TypeParsers
/// over the same checker. The ceiling and the diagnostic location are per
/// export.
fn check_api_stability_leaks(ctx: &mut RuleContext<'_, '_>) -> Vec<Diagnostic> {
    let module_symbol = ctx.tp.checker.get_symbol_of_declaration(ctx.source_file);
    if module_symbol.is_nil() {
        return Vec::new();
    }

    let mut diagnostics = Vec::new();
    for export_symbol in sorted_api_stability_exports(ctx, module_symbol) {
        if export_symbol.is_nil() {
            continue;
        }
        if api_stability_export_is_internal(ctx, module_symbol, export_symbol) {
            continue;
        }
        let ceiling = api_stability_ceiling(ctx, export_symbol);
        if ceiling >= ApiStabilityLevel::Experimental {
            continue;
        }
        let target = resolve_stability_alias(ctx.tp.checker, export_symbol);
        if target.is_nil() {
            continue;
        }
        let location = api_stability_export_location(ctx, export_symbol);
        if location.is_nil() {
            continue;
        }

        let session = ctx.tp.new_api_stability_session();
        let mut walker = ApiStabilityWalker::new(session, export_symbol, target, location, ceiling);
        walker.inspect_export(ctx);
        diagnostics.append(&mut walker.diagnostics);
    }
    diagnostics
}

// Go: rules/api_stability_leak.go sortedApiStabilityExports
/// sortedApiStabilityExports returns a module's exports in a deterministic
/// order. The checker returns them in Go map order; each export is analyzed
/// independently from its own session, so visit order only decides the order in
/// which bounded per-export analyses consume their own budgets. Sorting by name
/// (the unique export-table key) makes the reported findings reproducible across
/// runs and processes. Diagnostics are sorted later, so this only removes
/// analysis-order nondeterminism.
// PORT: Go compares names with `<` (Go bytes): `compare_go_strings`. The port
// gets the exports in table order, so equal names keep that order.
fn sorted_api_stability_exports(ctx: &mut RuleContext<'_, '_>, module: SymbolId) -> Vec<SymbolId> {
    let mut exports = ctx.tp.checker.get_exports_of_module_exported(module);
    let c = &*ctx.tp.checker;
    exports.sort_by(|&left, &right| {
        if left.is_nil() || right.is_nil() {
            if left == right {
                return std::cmp::Ordering::Equal;
            }
            if left.is_nil() {
                return std::cmp::Ordering::Greater;
            }
            return std::cmp::Ordering::Less;
        }
        crate::scanner_util::compare_go_strings(
            c.sym(left).name.as_str(),
            c.sym(right).name.as_str(),
        )
    });
    exports
}

// Go: rules/api_stability_leak.go apiStabilityCeiling
/// apiStabilityCeiling returns the stability ceiling that governs an export. A
/// forwarding tag written on a re-export declaration in the selected file takes
/// precedence over the forwarded symbol's own stability, including when it is
/// stricter; presence is therefore returned separately from the declared level.
/// The tag is source-specific, so it is read here rather than through the
/// per-checker symbol cache that only sees the forwarded symbol's declarations.
fn api_stability_ceiling(
    ctx: &mut RuleContext<'_, '_>,
    export_symbol: SymbolId,
) -> ApiStabilityLevel {
    let mut ceiling = ctx.tp.declared_api_stability(export_symbol);
    let (forwarded, present) = api_stability_forwarding_stability(ctx, export_symbol);
    if present {
        ceiling = forwarded;
    }
    ceiling
}

/// Go `apiStabilityWalker`.
/// apiStabilityWalker walks one export's public surface through the export's
/// analysis-local ApiStabilitySession and reports every dependency whose
/// declared stability exceeds the export's ceiling. Namespace re-exports are
/// enumerated through their module symbol so type-only exports are not lost. The
/// walker holds no type-graph state: cycle safety and memoization belong to the
/// session; when the export is inspected, its unpublishable results are
/// discarded and its complete, context-free surfaces are reused through the
/// per-checker stores.
struct ApiStabilityWalker {
    session: ApiStabilitySession,
    ceiling: ApiStabilityLevel,
    export_symbol: SymbolId,
    export_target: SymbolId,
    location: Node,

    // inspectedModules breaks namespace re-export cycles by resolved module
    // identity. offenders is keyed by offender declaration, symbol or signature,
    // so the same type is reported once per export regardless of how many paths
    // reach it.
    inspected_modules: FxHashMap<SymbolId, bool>,
    offenders: FxHashMap<ApiStabilityOffenderKey, bool>,

    diagnostics: Vec<Diagnostic>,
}

/// Go `apiStabilityOffenderKey`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct ApiStabilityOffenderKey {
    symbol: SymbolId,
    signature: SignatureId,
    declaration: Node,
}

impl ApiStabilityWalker {
    // Go: rules/api_stability_leak.go newApiStabilityWalker
    fn new(
        session: ApiStabilitySession,
        export_symbol: SymbolId,
        export_target: SymbolId,
        location: Node,
        ceiling: ApiStabilityLevel,
    ) -> Self {
        ApiStabilityWalker {
            session,
            ceiling,
            export_symbol,
            export_target,
            location,
            inspected_modules: FxHashMap::default(),
            offenders: FxHashMap::default(),
            diagnostics: Vec::new(),
        }
    }

    // Go: rules/api_stability_leak.go apiStabilityWalker.inspectExport
    /// inspectExport seeds the walk. Namespace re-exports expose their members'
    /// surfaces, so modules are enumerated explicitly.
    fn inspect_export(&mut self, ctx: &mut RuleContext<'_, '_>) {
        if self.export_target.is_nil() {
            return;
        }
        self.inspect_symbol(ctx, self.export_target);
    }

    // Go: rules/api_stability_leak.go apiStabilityWalker.inspectSymbol
    fn inspect_symbol(&mut self, ctx: &mut RuleContext<'_, '_>, symbol: SymbolId) {
        if symbol.is_nil() {
            return;
        }
        if ctx
            .tp
            .checker
            .sym(symbol)
            .flags
            .intersects(SymbolFlags::MODULE)
        {
            self.inspect_module(ctx, symbol);
            return;
        }
        let used = self.session.used_by_symbol(ctx.tp, symbol);
        for dependency in used.dependencies {
            self.record_dependency(ctx, dependency);
        }
    }

    // Go: rules/api_stability_leak.go apiStabilityWalker.inspectModule
    /// inspectModule enumerates a namespace module's exports and inspects each
    /// member. Modules are guarded by resolved identity so namespace re-export cycles
    /// terminate. Type-only exports (interfaces and aliases) are not value properties
    /// of the namespace, so they are enumerated through the module symbol.
    fn inspect_module(&mut self, ctx: &mut RuleContext<'_, '_>, module: SymbolId) {
        if module.is_nil()
            || self
                .inspected_modules
                .get(&module)
                .copied()
                .unwrap_or(false)
        {
            return;
        }
        self.inspected_modules.insert(module, true);
        for member in sorted_api_stability_exports(ctx, module) {
            if member.is_nil() {
                continue;
            }
            // An `@internal` member is not part of the namespace's public API, so
            // neither its declared stability nor its surface is inspected.
            if api_stability_export_is_internal(ctx, module, member) {
                continue;
            }
            // A namespace exposes each member's own declared stability as part of the
            // surface, so a tagged member is reported even when it is not reachable
            // through another member's dependencies.
            self.record_declared(ctx, member);
            let resolved = resolve_stability_alias(ctx.tp.checker, member);
            self.inspect_symbol(ctx, resolved);
        }
    }

    // Go: rules/api_stability_leak.go apiStabilityWalker.recordDeclared
    /// recordDeclared reports a namespace member whose own declared stability is
    /// above the namespace owner's ceiling. The member's surface is inspected
    /// separately for dependencies that themselves exceed the ceiling.
    fn record_declared(&mut self, ctx: &mut RuleContext<'_, '_>, symbol: SymbolId) {
        if symbol.is_nil() {
            return;
        }
        let declared = ctx.tp.declared_api_stability_of_symbol(symbol);
        self.report(
            ctx,
            ApiStabilityDependency {
                symbol,
                declaration: declared.declaration,
                level: declared.level,
                ..Default::default()
            },
        );
    }

    // Go: rules/api_stability_leak.go apiStabilityWalker.recordDependency
    /// recordDependency reports a dependency whose declared stability is above the
    /// export's ceiling. The owner's ceiling governs the entire surface; a member's
    /// own `@stability` tag never loosens it, and no separate stricter-member
    /// requirement is imposed.
    fn record_dependency(
        &mut self,
        ctx: &mut RuleContext<'_, '_>,
        dependency: ApiStabilityDependency,
    ) {
        self.report(ctx, dependency);
    }

    // Go: rules/api_stability_leak.go apiStabilityWalker.report
    fn report(&mut self, ctx: &mut RuleContext<'_, '_>, dependency: ApiStabilityDependency) {
        if dependency.level <= self.ceiling {
            return;
        }
        if dependency.symbol.is_nil()
            && dependency.signature.is_nil()
            && dependency.declaration.is_nil()
        {
            return;
        }
        if self.same_as_export(ctx, &dependency) {
            return;
        }
        let key = ApiStabilityOffenderKey {
            symbol: dependency.symbol,
            signature: dependency.signature,
            declaration: dependency.declaration,
        };
        if self.offenders.get(&key).copied().unwrap_or(false) {
            return;
        }
        self.offenders.insert(key, true);
        let mut related_information = Vec::new();
        let declaration = dependency.declaration;
        if declaration.is_some() {
            let source_file = get_source_file_of_node(declaration);
            if source_file.is_some() {
                related_information = vec![ctx.new_diagnostic(
                    source_file,
                    get_error_range_for_node(source_file, declaration),
                    diag::X_0_is_declared_1_here_effect_apiStabilityLeak,
                    Vec::new(),
                    vec![
                        api_stability_dependency_name(ctx.tp.checker, &dependency),
                        api_stability_level_name(dependency.level).to_string(),
                    ],
                )];
            }
        }
        let export_name = api_stability_export_display_name(ctx, self.export_symbol);
        let dependency_name = api_stability_dependency_name(ctx.tp.checker, &dependency);
        self.diagnostics.push(ctx.new_diagnostic(
            ctx.source_file,
            ctx.get_error_range(self.location),
            diag::X_0_exposes_1_an_2_API_effect_apiStabilityLeak,
            related_information,
            vec![
                export_name,
                dependency_name,
                api_stability_level_name(dependency.level).to_string(),
            ],
        ));
    }

    // Go: rules/api_stability_leak.go apiStabilityWalker.sameAsExport
    /// sameAsExport filters the export's own symbol from its dependency list. A
    /// signature or distinct declaration finding is never the export itself, so it
    /// is still reported.
    fn same_as_export(
        &self,
        ctx: &mut RuleContext<'_, '_>,
        dependency: &ApiStabilityDependency,
    ) -> bool {
        if dependency.symbol.is_nil() || dependency.signature.is_some() {
            return false;
        }
        let c = &mut *ctx.tp.checker;
        c.get_symbol_if_same_reference(dependency.symbol, self.export_symbol)
            .is_some()
            || c.get_symbol_if_same_reference(dependency.symbol, self.export_target)
                .is_some()
    }
}

// Go: rules/api_stability_leak.go apiStabilityLevelName
fn api_stability_level_name(level: ApiStabilityLevel) -> &'static str {
    if level == ApiStabilityLevel::Experimental {
        return "experimental";
    }
    "unstable"
}

// Go: rules/api_stability_leak.go apiStabilityExportDisplayName
fn api_stability_export_display_name(
    ctx: &mut RuleContext<'_, '_>,
    export_symbol: SymbolId,
) -> String {
    let name = ctx.tp.checker.sym(export_symbol).name.to_string();
    let Some(pkg) = ctx.tp.package_json_for_source_file(ctx.source_file) else {
        return name;
    };
    let (package_name, ok) = pkg.fields.name.get_value();
    if !ok || package_name.is_empty() {
        return name;
    }
    let directory = get_package_json_directory(ctx.program, ctx.source_file);
    let module_name = stability_api_module_name(
        &package_name,
        &directory,
        source_file_file_name(ctx.source_file),
    );
    if !module_name.is_empty() {
        return format!("{module_name}#{name}");
    }
    name
}

// Go: rules/api_stability_leak.go apiStabilityExportLocation
/// apiStabilityExportLocation returns a node in the selected file that names the
/// export, preferring a local declaration and falling back to the `export * from`
/// declaration that forwards it.
fn api_stability_export_location(ctx: &mut RuleContext<'_, '_>, export_symbol: SymbolId) -> Node {
    let declarations: Vec<Node> = ctx.tp.checker.sym(export_symbol).declarations.to_vec();
    for declaration in declarations {
        if declaration.is_nil() || get_source_file_of_node(declaration) != ctx.source_file {
            continue;
        }
        let name = get_name_of_declaration(declaration);
        if name.is_some() {
            return name;
        }
        return declaration;
    }
    api_stability_reexport_location(ctx, export_symbol)
}

// Go: rules/api_stability_leak.go apiStabilityForwardingStability
/// apiStabilityForwardingStability returns the stability declared by a re-export
/// declaration in the selected file that forwards this export: a star, a named
/// specifier or a namespace export. The boolean reports whether any forwarding
/// declaration carries a tag, which lets the caller give an explicit forwarding
/// tag precedence even when it tightens the inherited ceiling.
fn api_stability_forwarding_stability(
    ctx: &mut RuleContext<'_, '_>,
    export_symbol: SymbolId,
) -> (ApiStabilityLevel, bool) {
    if export_symbol.is_nil() {
        return (ApiStabilityLevel::Stable, false);
    }
    let mut best = ApiStabilityLevel::Stable;
    let mut present = false;
    let statements: Vec<Node> = ctx.source_file.statements().iter().collect();
    for statement in statements {
        if statement.is_nil() || statement.kind() != SyntaxKind::ExportDeclaration {
            continue;
        }
        let stability = stability_tag_of_declaration(statement);
        if stability.is_empty() {
            continue;
        }
        if !api_stability_export_declaration_forwards(ctx, statement, export_symbol) {
            continue;
        }
        present = true;
        let level = api_stability_level_from_tag(&stability);
        if level > best {
            best = level;
        }
    }
    (best, present)
}

// Go: rules/api_stability_leak.go apiStabilityExportDeclarationForwards
/// apiStabilityExportDeclarationForwards reports whether an export declaration in
/// the selected file forwards the given export symbol.
// PORT: Go takes the `*ast.ExportDeclaration`; the port takes its node.
fn api_stability_export_declaration_forwards(
    ctx: &mut RuleContext<'_, '_>,
    declaration: Node,
    export_symbol: SymbolId,
) -> bool {
    let export_clause = declaration.export_clause();
    let c = &mut *ctx.tp.checker;
    if export_clause.is_nil() {
        let module_specifier = declaration.module_specifier();
        if module_specifier.is_nil() {
            return false;
        }
        let mut module_symbol = c.get_symbol_at_location_exported(module_specifier);
        if module_symbol.is_some() && c.sym(module_symbol).flags.intersects(SymbolFlags::ALIAS) {
            module_symbol = c.get_aliased_symbol(module_symbol);
        }
        if module_symbol.is_nil() {
            return false;
        }
        let name = c.sym(export_symbol).name.clone();
        let exported = c.try_get_member_in_module_exports_and_properties(&name, module_symbol);
        return exported.is_some()
            && c.get_symbol_if_same_reference(exported, export_symbol)
                .is_some();
    }
    let export_name = c.sym(export_symbol).name.clone();
    match export_clause.kind() {
        SyntaxKind::NamespaceExport => {
            let name = export_clause.name();
            name.is_some() && name.text() == export_name.as_str()
        }
        SyntaxKind::NamedExports => {
            for element in export_clause.elements() {
                if element.is_nil() || element.kind() != SyntaxKind::ExportSpecifier {
                    continue;
                }
                let name = element.name();
                if name.is_some() && name.text() == export_name.as_str() {
                    return true;
                }
            }
            false
        }
        _ => false,
    }
}

/// Go `apiStabilityInternalVisit`.
/// apiStabilityInternalVisit identifies one module-scoped visibility question:
/// whether a symbol is internal as exported from a module. Recursion cycles are
/// broken by this identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct ApiStabilityInternalVisit {
    module: SymbolId,
    symbol: SymbolId,
}

type InternalVisiting = FxHashMap<ApiStabilityInternalVisit, bool>;

// Go: rules/api_stability_leak.go apiStabilityExportIsInternal
/// apiStabilityExportIsInternal reports whether an export is marked `@internal`
/// and must not be checked. Native TypeScript recognizes the same tag for
/// `stripInternal` declaration emit: a tagged declaration is omitted from the
/// emitted declarations, so it is not part of the public API.
///
/// An export is internal only when every path that exposes it from the module
/// is internal: a declaration without the tag is public, a named specifier,
/// star or namespace forwarding without the tag is public, and an untagged
/// forwarding inherits the visibility of the target it forwards. A merged
/// symbol with at least one non-internal declaration therefore stays public, so
/// a public overload is still checked when only the implementation is tagged,
/// while an untagged re-export of an internal target is skipped because the
/// forwarded target is not public API. The check reads metadata only: parsed
/// JSDoc tags, declarations and re-export statements, never types, so it never
/// touches the dependency caches.
fn api_stability_export_is_internal(
    ctx: &mut RuleContext<'_, '_>,
    module: SymbolId,
    export_symbol: SymbolId,
) -> bool {
    if module.is_nil() || export_symbol.is_nil() {
        return false;
    }
    api_stability_symbol_is_internal(ctx, module, export_symbol, &mut InternalVisiting::default())
}

// Go: rules/api_stability_leak.go apiStabilitySymbolIsInternal
/// apiStabilitySymbolIsInternal reports whether a symbol is internal as exported
/// from one module. Declarations of the symbol that live in the module are its
/// paths; a star re-export is inspected separately because it is not a
/// declaration of the forwarded symbol. A path that revisits an in-progress
/// visibility question reports no new public declaration: public declarations
/// are always collected when their module is entered, before that module's
/// forwards are followed, so a cycle can only be reached after every reachable
/// declaration path has already decided its visibility.
///
/// When the module's export table selects the symbol for its own name and the
/// symbol has a declaration in the module, that explicit declaration is the
/// compiler's export winner: a redundant `export *` forward of the same name
/// must not defeat a tag written on the explicit declaration. Star paths are
/// still followed when the symbol is only reachable through them, or when the
/// selected export resolves to a different symbol.
fn api_stability_symbol_is_internal(
    ctx: &mut RuleContext<'_, '_>,
    module: SymbolId,
    symbol: SymbolId,
    visiting: &mut InternalVisiting,
) -> bool {
    if module.is_nil() || symbol.is_nil() {
        return false;
    }
    let visit = ApiStabilityInternalVisit { module, symbol };
    if visiting.get(&visit).copied().unwrap_or(false) {
        return true;
    }
    visiting.insert(visit, true);
    let result = api_stability_symbol_is_internal_body(ctx, module, symbol, visiting);
    visiting.remove(&visit);
    result
}

/// PORT: the body of Go `apiStabilitySymbolIsInternal` after the visiting
/// insert (Go `defer delete(visiting, visit)`).
fn api_stability_symbol_is_internal_body(
    ctx: &mut RuleContext<'_, '_>,
    module: SymbolId,
    symbol: SymbolId,
    visiting: &mut InternalVisiting,
) -> bool {
    let files = api_stability_module_source_files(ctx.tp.checker, module);
    if files.is_empty() {
        return false;
    }
    let mut found = false;
    let declarations: Vec<Node> = ctx.tp.checker.sym(symbol).declarations.to_vec();
    for declaration in declarations {
        if declaration.is_nil() || !api_stability_declaration_in_files(declaration, &files) {
            continue;
        }
        found = true;
        if !api_stability_declaration_path_is_internal(ctx, module, symbol, declaration, visiting) {
            return false;
        }
    }
    if found && api_stability_symbol_is_module_export(ctx, module, symbol) {
        return true;
    }
    for statement in api_stability_module_export_statements(ctx.tp.checker, module) {
        if statement.is_nil() || statement.kind() != SyntaxKind::ExportDeclaration {
            continue;
        }
        // Named and namespace forwards are declarations of the forwarded
        // symbol; only a star forward is not, so it is inspected here.
        if statement.export_clause().is_some() {
            continue;
        }
        if !api_stability_export_declaration_forwards(ctx, statement, symbol) {
            continue;
        }
        found = true;
        if !api_stability_star_path_is_internal(ctx, symbol, statement, visiting) {
            return false;
        }
    }
    found
}

// Go: rules/api_stability_leak.go apiStabilitySymbolIsModuleExport
/// apiStabilitySymbolIsModuleExport reports whether a module's own export table
/// selects exactly this symbol for its name. It is the metadata-only identity
/// test that lets an explicit export declaration take precedence over redundant
/// star forwards; it resolves aliases but never reads a type.
fn api_stability_symbol_is_module_export(
    ctx: &mut RuleContext<'_, '_>,
    module: SymbolId,
    symbol: SymbolId,
) -> bool {
    if module.is_nil() || symbol.is_nil() {
        return false;
    }
    let c = &mut *ctx.tp.checker;
    let name = c.sym(symbol).name.clone();
    let exported = c.try_get_member_in_module_exports_and_properties(&name, module);
    c.get_symbol_if_same_reference(exported, symbol).is_some()
}

// Go: rules/api_stability_leak.go apiStabilityDeclarationPathIsInternal
/// apiStabilityDeclarationPathIsInternal reports whether one declaration of an
/// exported symbol is an internal path. A tag on the declaration or, for a named
/// or namespace forward, on its enclosing export declaration marks the path
/// internal; an untagged named forward stays internal only when the target it
/// forwards is itself internal. An import alias follows the imported symbol to
/// the module it was exported from, so `export { X }` after `import { X }`
/// inherits the original declaration's tag; an `export import` alias follows the
/// target of its module reference the same way. A namespace forward without a tag
/// is public: its container is public, and its members are filtered individually.
fn api_stability_declaration_path_is_internal(
    ctx: &mut RuleContext<'_, '_>,
    module: SymbolId,
    symbol: SymbolId,
    declaration: Node,
    visiting: &mut InternalVisiting,
) -> bool {
    if internal_tag_of_declaration(declaration) {
        return true;
    }
    let statement = api_stability_forwarding_statement_of(declaration);
    if statement.is_some() && internal_tag_of_declaration(statement) {
        return true;
    }
    match declaration.kind() {
        SyntaxKind::ExportSpecifier => {
            let mut target_module = module;
            if statement.is_some() {
                let module_specifier = statement.module_specifier();
                if module_specifier.is_some() {
                    target_module = api_stability_module_of_specifier(ctx, module_specifier);
                }
            }
            api_stability_aliased_target_is_internal(ctx, target_module, symbol, visiting)
        }
        SyntaxKind::ExportAssignment => {
            api_stability_aliased_target_is_internal(ctx, module, symbol, visiting)
        }
        SyntaxKind::ImportSpecifier | SyntaxKind::ImportClause => {
            api_stability_import_target_is_internal(ctx, declaration, symbol, visiting)
        }
        SyntaxKind::ImportEqualsDeclaration => {
            api_stability_import_equals_target_is_internal(ctx, symbol, declaration, visiting)
        }
        _ => false,
    }
}

/// Go `slices.ContainsFunc(symbol.Declarations, ast.IsAliasSymbolDeclaration)`.
fn api_stability_has_alias_symbol_declaration(c: &Checker, symbol: SymbolId) -> bool {
    c.sym(symbol)
        .declarations
        .iter()
        .any(|&declaration| is_alias_symbol_declaration(declaration))
}

// Go: rules/api_stability_leak.go apiStabilityImportTargetIsInternal
/// apiStabilityImportTargetIsInternal follows an untagged import alias to the
/// symbol it imports and reports whether that symbol is internal as exported
/// from the module it was imported from. An `export { X }` or
/// `export default X` of an imported identifier re-exposes the same declaration,
/// so it is internal exactly when the imported declaration is. Only the alias
/// target named by the import declaration is followed; no other identifier is
/// inferred and no type is read.
fn api_stability_import_target_is_internal(
    ctx: &mut RuleContext<'_, '_>,
    declaration: Node,
    symbol: SymbolId,
    visiting: &mut InternalVisiting,
) -> bool {
    if symbol.is_nil()
        || !ctx
            .tp
            .checker
            .sym(symbol)
            .flags
            .intersects(SymbolFlags::ALIAS)
    {
        return false;
    }
    if !api_stability_has_alias_symbol_declaration(ctx.tp.checker, symbol) {
        return false;
    }
    let target_module = api_stability_import_target_module(ctx, declaration);
    if target_module.is_nil() {
        return false;
    }
    let target = api_stability_immediate_aliased_symbol(ctx.tp.checker, symbol);
    if target.is_nil() || target == symbol {
        return false;
    }
    api_stability_symbol_is_internal(ctx, target_module, target, visiting)
}

// Go: rules/api_stability_leak.go apiStabilityImportTargetModule
/// apiStabilityImportTargetModule resolves the module an import alias targets
/// through the import declaration that contains the alias. An unresolvable
/// specifier reports no module, which keeps the import path public.
fn api_stability_import_target_module(
    ctx: &mut RuleContext<'_, '_>,
    declaration: Node,
) -> SymbolId {
    let mut node = declaration;
    while node.is_some() {
        match node.kind() {
            SyntaxKind::ImportDeclaration => {
                return api_stability_module_of_specifier(ctx, node.module_specifier());
            }
            SyntaxKind::SourceFile | SyntaxKind::ModuleBlock => return SymbolId::NIL,
            _ => {}
        }
        node = node.parent();
    }
    SymbolId::NIL
}

// Go: rules/api_stability_leak.go apiStabilityImportEqualsTargetIsInternal
/// apiStabilityImportEqualsTargetIsInternal follows an untagged `import X = ...`
/// alias, including the `export import` form, to the target it names and reports
/// whether that target is internal as exported from its own module. The checker's
/// native immediate alias target is preferred; when the checker left it
/// unresolved (a namespace import qualifier, a chained alias or an unknown
/// require), the target is read from the qualifier module's export table or from
/// the referenced module itself, which is the same declaration metadata the
/// checker's own resolution uses. Only the written module reference is followed
/// and no type is read, so a tag on the alias declaration or on the target's
/// declaration keeps exactly the precedence the named and default import paths
/// give it.
fn api_stability_import_equals_target_is_internal(
    ctx: &mut RuleContext<'_, '_>,
    symbol: SymbolId,
    declaration: Node,
    visiting: &mut InternalVisiting,
) -> bool {
    if symbol.is_nil()
        || !ctx
            .tp
            .checker
            .sym(symbol)
            .flags
            .intersects(SymbolFlags::ALIAS)
    {
        return false;
    }
    if !api_stability_has_alias_symbol_declaration(ctx.tp.checker, symbol) {
        return false;
    }
    if declaration.kind() != SyntaxKind::ImportEqualsDeclaration {
        return false;
    }
    let reference = declaration.module_reference();
    if reference.is_nil() {
        return false;
    }
    let mut target = api_stability_immediate_aliased_symbol(ctx.tp.checker, symbol);
    if target.is_nil() {
        let name = api_stability_import_equals_target_name(reference);
        if !name.is_empty() {
            let qualifier = api_stability_module_of_entity(
                ctx,
                api_stability_import_equals_qualifier(reference),
            );
            if qualifier.is_some() {
                target = ctx
                    .tp
                    .checker
                    .try_get_member_in_module_exports_and_properties(&name, qualifier);
            }
        }
    }
    if target.is_nil() && reference.kind() == SyntaxKind::ExternalModuleReference {
        // A native `require` alias stands for the module itself; the module
        // reference is that target when the checker has not resolved it.
        target =
            api_stability_module_of_entity(ctx, api_stability_import_equals_qualifier(reference));
    }
    if target.is_nil() || target == symbol {
        return false;
    }
    let modules = api_stability_declaration_modules(ctx, target);
    if modules.is_empty() {
        return false;
    }
    // The alias forwards one symbol; it is internal only when every module
    // that declares that symbol exposes it internally, like any merged path.
    for module in modules {
        if !api_stability_symbol_is_internal(ctx, module, target, visiting) {
            return false;
        }
    }
    true
}

// Go: rules/api_stability_leak.go apiStabilityImportEqualsQualifier
/// apiStabilityImportEqualsQualifier returns the entity name that qualifies the
/// target of an import-equals reference: the left side of a qualified name (the
/// namespace the named member belongs to), the require argument of an external
/// module reference, or nil for a bare identifier target.
fn api_stability_import_equals_qualifier(reference: Node) -> Node {
    if reference.is_nil() {
        return Node::NIL;
    }
    match reference.kind() {
        SyntaxKind::QualifiedName => reference.left(),
        SyntaxKind::ExternalModuleReference => reference.expression(),
        _ => Node::NIL,
    }
}

// Go: rules/api_stability_leak.go apiStabilityImportEqualsTargetName
/// apiStabilityImportEqualsTargetName returns the member name an import-equals
/// reference names inside its qualifier module. A bare identifier names itself
/// and an external `require` names the module, so both report no member name.
fn api_stability_import_equals_target_name(reference: Node) -> String {
    if reference.is_nil() || reference.kind() != SyntaxKind::QualifiedName {
        return String::new();
    }
    let right = reference.right();
    if right.is_nil() {
        return String::new();
    }
    right.text().to_string()
}

// Go: rules/api_stability_leak.go apiStabilityDeclarationModule
/// apiStabilityDeclarationModule returns the module symbol that directly exports
/// a declaration: the nearest enclosing declared namespace, or the source file's
/// module for a top-level declaration. It is the metadata-only fallback for an
/// alias target whose module is not named by a module reference, such as a bare
/// identifier target or a chained alias.
fn api_stability_declaration_module(ctx: &mut RuleContext<'_, '_>, declaration: Node) -> SymbolId {
    let mut node = declaration;
    while node.is_some() {
        if matches!(
            node.kind(),
            SyntaxKind::ModuleDeclaration | SyntaxKind::SourceFile
        ) {
            return ctx.tp.checker.get_symbol_of_declaration(node);
        }
        node = node.parent();
    }
    SymbolId::NIL
}

// Go: rules/api_stability_leak.go apiStabilityDeclarationModules
/// apiStabilityDeclarationModules returns the distinct modules that declare a
/// symbol, in declaration order. A symbol merged from several modules has one
/// visibility path per module.
fn api_stability_declaration_modules(
    ctx: &mut RuleContext<'_, '_>,
    symbol: SymbolId,
) -> Vec<SymbolId> {
    if symbol.is_nil() {
        return Vec::new();
    }
    let mut modules: Vec<SymbolId> = Vec::new();
    let declarations: Vec<Node> = ctx.tp.checker.sym(symbol).declarations.to_vec();
    for declaration in declarations {
        if declaration.is_nil() {
            continue;
        }
        let module = api_stability_declaration_module(ctx, declaration);
        if module.is_nil() || modules.contains(&module) {
            continue;
        }
        modules.push(module);
    }
    modules
}

// Go: rules/api_stability_leak.go apiStabilityStarPathIsInternal
/// apiStabilityStarPathIsInternal reports whether one star re-export path is
/// internal. The tag on the star declaration marks the whole path internal;
/// otherwise the path inherits the visibility of the symbol in the module it is
/// forwarded from.
fn api_stability_star_path_is_internal(
    ctx: &mut RuleContext<'_, '_>,
    symbol: SymbolId,
    statement: Node,
    visiting: &mut InternalVisiting,
) -> bool {
    if internal_tag_of_declaration(statement) {
        return true;
    }
    let module = api_stability_module_of_specifier(ctx, statement.module_specifier());
    api_stability_symbol_is_internal(ctx, module, symbol, visiting)
}

// Go: rules/api_stability_leak.go apiStabilityAliasedTargetIsInternal
/// apiStabilityAliasedTargetIsInternal follows an untagged alias declaration to
/// its immediate target and reports whether the target is internal. The checker
/// synthesizes declaration-less aliases for `export =` and JSON modules and
/// panics when asked for their immediate target, so those stay public.
fn api_stability_aliased_target_is_internal(
    ctx: &mut RuleContext<'_, '_>,
    module: SymbolId,
    symbol: SymbolId,
    visiting: &mut InternalVisiting,
) -> bool {
    if symbol.is_nil()
        || !ctx
            .tp
            .checker
            .sym(symbol)
            .flags
            .intersects(SymbolFlags::ALIAS)
    {
        return false;
    }
    if !api_stability_has_alias_symbol_declaration(ctx.tp.checker, symbol) {
        return false;
    }
    let target = api_stability_immediate_aliased_symbol(ctx.tp.checker, symbol);
    if target.is_nil() || target == symbol {
        return false;
    }
    api_stability_symbol_is_internal(ctx, module, target, visiting)
}

// Go: rules/api_stability_leak.go apiStabilityForwardingStatementOf
/// apiStabilityForwardingStatementOf returns the export declaration statement a
/// named specifier or namespace export belongs to. A forwarding tag is written
/// above the whole statement, so the statement carries the tag for every
/// specifier inside it.
fn api_stability_forwarding_statement_of(declaration: Node) -> Node {
    if declaration.is_nil() || declaration.parent().is_nil() {
        return Node::NIL;
    }
    let parent = declaration.parent();
    if parent.kind() == SyntaxKind::ExportDeclaration {
        return parent;
    }
    let grandparent = parent.parent();
    if grandparent.is_some() && grandparent.kind() == SyntaxKind::ExportDeclaration {
        return grandparent;
    }
    Node::NIL
}

// Go: rules/api_stability_leak.go apiStabilityModuleSourceFiles
/// apiStabilityModuleSourceFiles returns the source files a module symbol is
/// declared in. A source file module is declared by the file itself; a declared
/// namespace or ambient module is declared by its module declaration, and its
/// members live in the file containing that declaration. Merged namespace
/// declarations therefore contribute every file they appear in.
fn api_stability_module_source_files(c: &Checker, module: SymbolId) -> Vec<Node> {
    if module.is_nil() {
        return Vec::new();
    }
    let mut files: Vec<Node> = Vec::new();
    for &declaration in &c.sym(module).declarations {
        if declaration.is_nil() {
            continue;
        }
        let file = match declaration.kind() {
            SyntaxKind::SourceFile => declaration,
            SyntaxKind::ModuleDeclaration => get_source_file_of_node(declaration),
            _ => Node::NIL,
        };
        if file.is_nil() || files.contains(&file) {
            continue;
        }
        files.push(file);
    }
    files
}

// Go: rules/api_stability_leak.go apiStabilityModuleExportStatements
/// apiStabilityModuleExportStatements returns the statements that can forward
/// exports out of a module with `export *`: the top-level statements of a source
/// file, or the statements of the module blocks of its declared namespaces and
/// ambient modules. Named and namespace forwards are declarations of the
/// forwarded symbol itself and are inspected through symbol.Declarations; only
/// star forwards need this separate enumeration. A dotted namespace declaration
/// nests module declarations, so the body chain is followed to its module block.
fn api_stability_module_export_statements(c: &Checker, module: SymbolId) -> Vec<Node> {
    if module.is_nil() {
        return Vec::new();
    }
    let mut statements: Vec<Node> = Vec::new();
    for &declaration in &c.sym(module).declarations {
        if declaration.is_nil() {
            continue;
        }
        match declaration.kind() {
            SyntaxKind::SourceFile => statements.extend(declaration.statements().iter()),
            SyntaxKind::ModuleDeclaration => {
                let block = api_stability_module_block_of(declaration);
                if block.is_some() {
                    statements.extend(block.statements().iter());
                }
            }
            _ => {}
        }
    }
    statements
}

// Go: rules/api_stability_leak.go apiStabilityModuleBlockOf
/// apiStabilityModuleBlockOf returns the module block at the end of a module
/// declaration's body chain. A namespace declared without a body (an ambient
/// declaration) has no block and forwards nothing.
fn api_stability_module_block_of(declaration: Node) -> Node {
    let mut node = declaration;
    while node.is_some() && node.kind() == SyntaxKind::ModuleDeclaration {
        let body = node.body();
        if body.is_nil() {
            return Node::NIL;
        }
        if body.kind() == SyntaxKind::ModuleBlock {
            return body;
        }
        node = body;
    }
    Node::NIL
}

// Go: rules/api_stability_leak.go apiStabilityDeclarationInFiles
fn api_stability_declaration_in_files(declaration: Node, files: &[Node]) -> bool {
    let file = get_source_file_of_node(declaration);
    file.is_some() && files.contains(&file)
}

// Go: rules/api_stability_leak.go apiStabilityModuleOfSpecifier
/// apiStabilityModuleOfSpecifier resolves the module symbol a module specifier
/// refers to. An unresolved specifier reports no module, which keeps the
/// forwarding path public.
fn api_stability_module_of_specifier(
    ctx: &mut RuleContext<'_, '_>,
    module_specifier: Node,
) -> SymbolId {
    api_stability_module_of_entity(ctx, module_specifier)
}

// Go: rules/api_stability_leak.go apiStabilityModuleOfEntity
/// apiStabilityModuleOfEntity resolves the module symbol an entity name or module
/// specifier refers to, following an import alias to the module it names. An
/// unresolved entity or a symbol that declares no module reports no module, which
/// keeps the forwarding path public.
fn api_stability_module_of_entity(ctx: &mut RuleContext<'_, '_>, entity: Node) -> SymbolId {
    if entity.is_nil() {
        return SymbolId::NIL;
    }
    let c = &mut *ctx.tp.checker;
    let mut module = c.get_symbol_at_location_exported(entity);
    if module.is_nil() {
        return SymbolId::NIL;
    }
    if c.sym(module).flags.intersects(SymbolFlags::ALIAS) {
        module = c.get_aliased_symbol(module);
    }
    if module.is_nil() || api_stability_module_source_files(c, module).is_empty() {
        return SymbolId::NIL;
    }
    module
}

// Go: rules/api_stability_leak.go apiStabilityReexportLocation
fn api_stability_reexport_location(ctx: &mut RuleContext<'_, '_>, export_symbol: SymbolId) -> Node {
    let mut fallback = Node::NIL;
    let statements: Vec<Node> = ctx.source_file.statements().iter().collect();
    let c = &mut *ctx.tp.checker;
    for statement in statements {
        if statement.is_nil() || statement.kind() != SyntaxKind::ExportDeclaration {
            continue;
        }
        let module_specifier = statement.module_specifier();
        if module_specifier.is_nil() || statement.export_clause().is_some() {
            continue;
        }
        if fallback.is_nil() {
            fallback = statement;
        }
        let mut module_symbol = c.get_symbol_at_location_exported(module_specifier);
        if module_symbol.is_nil() {
            continue;
        }
        if c.sym(module_symbol).flags.intersects(SymbolFlags::ALIAS) {
            module_symbol = c.get_aliased_symbol(module_symbol);
        }
        if module_symbol.is_nil() {
            continue;
        }
        let name = c.sym(export_symbol).name.clone();
        let exported = c.try_get_member_in_module_exports_and_properties(&name, module_symbol);
        if exported.is_some()
            && c.get_symbol_if_same_reference(exported, export_symbol)
                .is_some()
        {
            return statement;
        }
    }
    fallback
}
