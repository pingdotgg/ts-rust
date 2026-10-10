//! Port of Effect-TS/tsgo `internal/pluginoptions`: the per-file plugin
//! options after the `overrides` whose globs match the file.

use crate::effect::etscore::{
    EffectPluginOptions, Override, ResolvedEffectPluginOptions, Severity,
};
use crate::frontend::tspath::path::get_directory_path;
use crate::frontend::vfs::vfsmatch::{Usage, new_spec_matcher};
use indexmap::IndexMap;

// Go: pluginoptions ResolveEffectPluginOptionsForSourceFile
#[must_use]
pub fn resolve_effect_plugin_options_for_source_file(
    config: &EffectPluginOptions,
    file_name: &str,
    config_file_path: &str,
    use_case_sensitive_file_names: bool,
) -> ResolvedEffectPluginOptions {
    let mut effective = clone_options(config);
    if config.overrides.is_empty() {
        return effective;
    }
    let base_path = get_directory_path(config_file_path);
    for o in &config.overrides {
        if matches_override(o, file_name, &base_path, use_case_sensitive_file_names) {
            apply_override(&mut effective, o);
        }
    }
    effective
}

// Go: pluginoptions ResolveDiagnosticSeverityForFile
/// The rule severities for `file_name`. `None` is Go nil (no
/// `diagnosticSeverity` and no overrides).
#[must_use]
pub fn resolve_diagnostic_severity_for_file(
    config: &EffectPluginOptions,
    file_name: &str,
    config_file_path: &str,
    use_case_sensitive_file_names: bool,
) -> Option<IndexMap<String, Severity>> {
    if config.overrides.is_empty() {
        return config.diagnostic_severity.clone();
    }
    let base_path = get_directory_path(config_file_path);
    let mut resolved = config.diagnostic_severity.clone().unwrap_or_default();
    for o in &config.overrides {
        if !matches_override(o, file_name, &base_path, use_case_sensitive_file_names) {
            continue;
        }
        let Some(severity) = &o.options.diagnostic_severity else {
            continue;
        };
        for (k, v) in severity {
            resolved.insert(k.clone(), *v);
        }
    }
    Some(resolved)
}

fn matches_override(
    o: &Override,
    file_name: &str,
    base_path: &str,
    use_case_sensitive_file_names: bool,
) -> bool {
    if !o.include.is_empty() {
        let matcher = new_spec_matcher(
            &o.include,
            base_path,
            Usage::Files,
            use_case_sensitive_file_names,
        );
        if !matcher.is_some_and(|m| m.match_string(file_name)) {
            return false;
        }
    }
    if !o.exclude.is_empty() {
        let matcher = new_spec_matcher(
            &o.exclude,
            base_path,
            Usage::Exclude,
            use_case_sensitive_file_names,
        );
        if matcher.is_some_and(|m| m.match_string(file_name)) {
            return false;
        }
    }
    true
}

fn apply_override(target: &mut ResolvedEffectPluginOptions, o: &Override) {
    if let Some(n) = o.options.pipeable_min_arg_count {
        target.pipeable_min_arg_count = n;
    }
    if let Some(patterns) = &o.options.key_patterns {
        target.key_patterns = patterns.clone();
    }
    if let Some(b) = o.options.extended_key_detection {
        target.extended_key_detection = b;
    }
    if let Some(apis) = &o.options.allowed_unstable_apis {
        target.allowed_unstable_apis = apis.clone();
    }
    if let Some(apis) = &o.options.allowed_experimental_apis {
        target.allowed_experimental_apis = apis.clone();
    }
    if let Some(pkgs) = &o.options.allowed_duplicated_packages {
        target.allowed_duplicated_packages = pkgs.clone();
    }
    if let Some(variants) = &o.options.effect_fn {
        target.effect_fn = variants.clone();
    }
}

fn clone_options(config: &EffectPluginOptions) -> ResolvedEffectPluginOptions {
    ResolvedEffectPluginOptions {
        namespace_import_packages: config.namespace_import_packages.clone(),
        barrel_import_packages: config.barrel_import_packages.clone(),
        import_aliases: config.import_aliases.clone(),
        top_level_named_reexports: config.top_level_named_reexports,
        key_patterns: config.key_patterns.clone(),
        extended_key_detection: config.extended_key_detection,
        pipeable_min_arg_count: config.pipeable_min_arg_count,
        allowed_duplicated_packages: config
            .allowed_duplicated_packages
            .clone()
            .unwrap_or_default(),
        allowed_unstable_apis: config.allowed_unstable_apis.clone().unwrap_or_default(),
        allowed_experimental_apis: config.allowed_experimental_apis.clone().unwrap_or_default(),
        effect_fn: config.effect_fn.clone(),
    }
}
