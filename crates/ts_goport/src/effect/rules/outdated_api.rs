//! Port of Effect-TS/tsgo `internal/rules/outdated_api.go`.

use crate::effect::diag;
use crate::effect::etscore::*;
use crate::effect::rule::*;
use crate::effect::typeparser::*;
use crate::prelude::*;

use super::outdated_api_db::{EFFECT_MODULE_MIGRATION_DB, Migration, MigrationTag};

/// Go `OutdatedApi`: detects usage of Effect v3 APIs in projects targeting
/// Effect v4.
pub static OUTDATED_API: Rule = Rule {
    name: "outdatedApi",
    group: "correctness",
    description: "Detects usage of APIs that have been removed or renamed in Effect v4",
    default_severity: Severity::Warning,
    supported_effect: &["v4"],
    codes: &[377052, 377053],
    run: run_outdated_api,
};

// Go: OutdatedApi.Run
fn run_outdated_api(ctx: &mut RuleContext<'_, '_>) -> Vec<Diagnostic> {
    let source_file = ctx.source_file;
    let matches = analyze_outdated_api(ctx.tp, source_file);
    if matches.is_empty() {
        return Vec::new();
    }

    let mut diags = Vec::with_capacity(matches.len() + 1);
    for m in &matches {
        diags.push(ctx.new_diagnostic(
            m.source_file,
            m.location,
            diag::This_project_targets_Effect_v4_but_this_code_uses_the_Effect_v3_API_0_The_referenced_API_belongs_to_the_v3_surface_rather_than_the_configured_v4_surface_1_effect_outdatedApi,
            Vec::new(),
            vec![m.property_name.clone(), m.migration_hint.clone()],
        ));
    }

    // Append global summary diagnostic at position 0:0
    diags.push(ctx.new_diagnostic(
        source_file,
        TextRange::new(0, 0),
        diag::This_project_targets_Effect_v4_but_this_code_uses_Effect_v3_APIs_The_referenced_API_belongs_to_the_v3_surface_rather_than_the_configured_v4_surface_effect_outdatedApi,
        Vec::new(),
        Vec::new(),
    ));

    diags
}

/// Go `OutdatedApiMatch`: holds the analysis results for one outdated API
/// usage.
#[derive(Clone, Debug)]
pub struct OutdatedApiMatch {
    pub source_file: Node,
    pub location: TextRange,
    pub property_name: String,
    pub migration_hint: String,
    pub migration: Migration,
}

// Go: AnalyzeOutdatedApi
/// Finds all usages of Effect v3 APIs in an Effect v4 project.
pub fn analyze_outdated_api(tp: &mut TypeParser<'_>, sf: Node) -> Vec<OutdatedApiMatch> {
    if tp.supported_effect_version() != EffectMajorVersion::V4 {
        return Vec::new();
    }

    let mut matches = Vec::new();

    fn visit(tp: &mut TypeParser<'_>, sf: Node, matches: &mut Vec<OutdatedApiMatch>, n: Node) {
        if n.kind() == SyntaxKind::PropertyAccessExpression {
            if let Some(m) = check_outdated_api_property_access(tp, sf, n) {
                matches.push(m);
            }
        }
    }

    // PERF: Go walks the tree; the node order has the same nodes in the same
    // order (effperf4).
    for &n in tp.node_order(sf).nodes() {
        visit(tp, sf, &mut matches, n);
    }

    matches
}

// Go: checkOutdatedApiPropertyAccess
fn check_outdated_api_property_access(
    tp: &mut TypeParser<'_>,
    sf: Node,
    n: Node,
) -> Option<OutdatedApiMatch> {
    // Go: prop := n.AsPropertyAccessExpression()
    let prop = n;
    if prop.is_nil() {
        return None;
    }

    let name_node = prop.name();
    if name_node.is_nil() {
        return None;
    }

    let identifier_name = name_node.text();
    if identifier_name.is_empty() {
        return None;
    }

    let migration = *EFFECT_MODULE_MIGRATION_DB.get(identifier_name)?;

    // Skip unchanged entries — they exist in both v3 and v4
    if migration.tag == MigrationTag::Unchanged {
        return None;
    }

    // Get the type of the expression (left side of the property access)
    let expr = prop.expression();
    if expr.is_nil() {
        return None;
    }

    let target_type = tp.get_type_at_location(expr);
    if target_type.is_nil() {
        return None;
    }

    // Only report if the property does NOT exist on the target type
    // (confirming it's a v3-only API)
    if tp
        .checker
        .get_property_of_type_exported(target_type, identifier_name)
        .is_some()
    {
        return None;
    }

    // Verify the expression references the Effect module
    if !tp.is_expression_effect_module(expr) {
        return None;
    }

    Some(OutdatedApiMatch {
        source_file: sf,
        location: get_error_range_for_node(sf, name_node),
        property_name: identifier_name.to_string(),
        migration_hint: migration_hint_text(&migration),
        migration,
    })
}

// Go: migrationHintText
fn migration_hint_text(m: &Migration) -> String {
    match m.tag {
        MigrationTag::Removed => m.alternative_pattern.to_string(),
        MigrationTag::RenamedSameBehaviour => format!("Renamed to {}.", m.new_name),
        MigrationTag::RenamedAndNeedsOptions => {
            format!("Renamed to {}. {}", m.new_name, m.options_instructions)
        }
        _ => String::new(),
    }
}
