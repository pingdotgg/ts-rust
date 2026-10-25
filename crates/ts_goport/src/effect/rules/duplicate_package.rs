//! Port of Effect-TS/tsgo `internal/rules/duplicate_package.go`.

use crate::effect::diag;
use crate::effect::etscore::*;
use crate::effect::rule::*;
use crate::effect::typeparser::*;
use crate::prelude::*;

/// duplicatePackageDiag holds pre-computed diagnostic info for a single duplicated package name.
struct DuplicatePackageDiag {
    package_name: String,
    /// e.g. "1.0.0 @ /path/a, 2.0.0 @ /path/b"
    details: String,
    config_value: String,
}

/// DuplicatePackage warns when multiple versions of the same Effect-related package
/// are loaded into the program.
pub static DUPLICATE_PACKAGE: Rule = Rule {
    name: "duplicatePackage",
    group: "correctness",
    description: "Warns when multiple versions of an Effect-related package are detected in the program",
    default_severity: Severity::Warning,
    supported_effect: &["v3", "v4"],
    codes: &[377051],
    run: run_duplicate_package,
};

fn run_duplicate_package(ctx: &mut RuleContext<'_, '_>) -> Vec<Diagnostic> {
    let entries = compute_duplicate_package_diags(ctx.tp, Some(ctx.options));
    if entries.is_empty() {
        return Vec::new();
    }

    // Attach one diagnostic per duplicated package to the first statement (or the source file node).
    let target;
    let stmts = ctx.source_file.statements();
    if !stmts.is_empty() {
        target = stmts.get(0);
    } else {
        target = ctx.source_file;
    }
    let loc = ctx.get_error_range(target);

    let mut diags = Vec::with_capacity(entries.len());
    for e in entries {
        diags.push(ctx.new_diagnostic(
            ctx.source_file,
            loc,
            diag::Multiple_versions_of_package_0_were_detected_Colon_1_Package_duplication_can_change_runtime_identity_and_type_equality_across_Effect_modules_If_this_is_intentional_set_the_LSP_config_allowedDuplicatedPackages_to_2_effect_duplicatePackage,
            Vec::new(),
            vec![e.package_name, e.details, e.config_value],
        ));
    }
    diags
}

/// computeDuplicatePackageDiags scans all packages and finds names with multiple distinct versions.
fn compute_duplicate_package_diags(
    tp: &mut TypeParser<'_>,
    effect_config: Option<&ResolvedEffectPluginOptions>,
) -> Vec<DuplicatePackageDiag> {
    let packages = tp.discover_packages();

    // Filter to Effect-related packages.
    struct VersionEntry {
        version: String,
        dir: String,
    }
    let mut by_name: FxHashMap<String, Vec<VersionEntry>> = FxHashMap::default();
    for pkg in &packages {
        if pkg.name != "effect" && !pkg.depends_on_effect {
            continue;
        }
        let mut ver = String::new();
        if let Some(version) = &pkg.version {
            ver = version.clone();
        }
        by_name
            .entry(pkg.name.clone())
            .or_default()
            .push(VersionEntry {
                version: ver,
                dir: pkg.package_directory.clone(),
            });
    }

    let mut allowed: Vec<String> = Vec::new();
    if let Some(effect_config) = effect_config {
        allowed = effect_config.get_allowed_duplicated_packages().to_vec();
    }

    let mut diags = Vec::new();

    // Sort names for deterministic ordering.
    let mut names: Vec<String> = by_name.keys().cloned().collect();
    names.sort();

    for name in names {
        let entries = &by_name[&name];
        if entries.len() <= 1 {
            continue;
        }
        if allowed.contains(&name) {
            continue;
        }

        // Build details string: "1.0.0 @ /path/a, 2.0.0 @ /path/b"
        let mut parts: Vec<String> = Vec::with_capacity(entries.len());
        for e in entries {
            if !e.version.is_empty() {
                parts.push(format!("{} @ {}", e.version, e.dir));
            } else {
                parts.push(format!("(unknown) @ {}", e.dir));
            }
        }
        let mut config_list = allowed.clone();
        config_list.push(name.clone());
        // PORT: Go `json.Marshal` of a `[]string` cannot fail, so Go's "[]"
        // fallback is unreachable.
        let config_value = encoding_json_marshal_strings(&config_list);
        diags.push(DuplicatePackageDiag {
            package_name: name,
            details: parts.join(", "),
            config_value,
        });
    }

    diags
}

/// Go `encoding/json` (v1) `Marshal` of a `[]string`: a JSON array with Go's
/// default string escaping (HTML-safe: `<`, `>` and `&` become `\u003c`,
/// `\u003e` and `\u0026`; U+2028 and U+2029 are escaped).
// PORT: no Go function; it inlines `encodeState.string` with escapeHTML.
fn encoding_json_marshal_strings(values: &[String]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::from("[");
    for (i, value) in values.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push('"');
        for ch in value.chars() {
            match ch {
                '\\' => out.push_str("\\\\"),
                '"' => out.push_str("\\\""),
                '\u{08}' => out.push_str("\\b"),
                '\u{0C}' => out.push_str("\\f"),
                '\n' => out.push_str("\\n"),
                '\r' => out.push_str("\\r"),
                '\t' => out.push_str("\\t"),
                '<' | '>' | '&' => {
                    let b = ch as u8;
                    out.push_str("\\u00");
                    out.push(HEX[(b >> 4) as usize] as char);
                    out.push(HEX[(b & 0xF) as usize] as char);
                }
                '\u{2028}' => out.push_str("\\u2028"),
                '\u{2029}' => out.push_str("\\u2029"),
                c if (c as u32) < 0x20 => {
                    let b = c as u8;
                    out.push_str("\\u00");
                    out.push(HEX[(b >> 4) as usize] as char);
                    out.push(HEX[(b & 0xF) as usize] as char);
                }
                c => out.push(c),
            }
        }
        out.push('"');
    }
    out.push(']');
    out
}
