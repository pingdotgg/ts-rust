//! Port of Effect-TS/tsgo `internal/effectconfigraw`: merging the Effect
//! plugin options across `extends`. A child config replaces only the plugin
//! keys it sets. `diagnosticSeverity` merges by rule, and `overrides`
//! appends. Override globs are rewritten relative to the config that
//! declared them.

use crate::effect::etscore::{EFFECT_PLUGIN_NAME, EffectPluginOptions};
use crate::frontend::tsoptions::command_line_option::CompilerOptionsValue;
use crate::frontend::tspath::path::{
    ComparePathsOptions, combine_paths, convert_to_relative_path, get_directory_path,
    is_rooted_disk_path,
};
use crate::options::CompilerOptions;
use indexmap::IndexMap;
use std::sync::Arc;

// Go: effectconfigraw MergeEffectCompilerOptions
/// Patch 013 `MergeCompilerOptionsCallback`: runs after the standard merge
/// of `source_options` into `target_options`.
pub fn merge_effect_compiler_options(
    target_options: &mut CompilerOptions,
    source_options: &CompilerOptions,
    raw_source: Option<&IndexMap<String, CompilerOptionsValue>>,
    source_config_path: &str,
    base_path: &str,
) {
    let Some(source) = source_options.effect.as_deref() else {
        return;
    };
    let mut source_effect = clone_effect_options(source);
    rewrite_effect_options_overrides(&mut source_effect, source_config_path, base_path);
    let Some(source_plugin_raw) = get_effect_plugin_raw(raw_source) else {
        target_options.effect = Some(Arc::new(source_effect));
        return;
    };
    let Some(target) = target_options.effect.as_deref() else {
        target_options.effect = Some(Arc::new(source_effect));
        return;
    };
    target_options.effect = Some(Arc::new(merge_effect_options(
        target,
        &source_effect,
        source_plugin_raw,
    )));
}

fn merge_effect_options(
    target: &EffectPluginOptions,
    source: &EffectPluginOptions,
    raw: &IndexMap<String, CompilerOptionsValue>,
) -> EffectPluginOptions {
    let mut merged = clone_effect_options(target);
    let has = |key: &str| raw.contains_key(key);
    macro_rules! take {
        ($($key:literal => $field:ident,)*) => {
            $(if has($key) {
                merged.$field = source.$field.clone();
            })*
        };
    }
    take! {
        "refactors" => refactors,
        "diagnostics" => diagnostics,
        "includeSuggestionsInTsc" => include_suggestions_in_tsc,
        "quickinfo" => quickinfo,
        "completions" => completions,
        "goto" => goto,
        "renames" => renames,
        "ignoreEffectSuggestionsInTscExitCode" => ignore_effect_suggestions_in_tsc_exit_code,
        "ignoreEffectWarningsInTscExitCode" => ignore_effect_warnings_in_tsc_exit_code,
        "ignoreEffectErrorsInTscExitCode" => ignore_effect_errors_in_tsc_exit_code,
        "skipDisabledOptimization" => skip_disabled_optimization,
        "mermaidProvider" => mermaid_provider,
        "noExternal" => no_external,
        "layerGraphFollowDepth" => layer_graph_follow_depth,
        "inlays" => inlays,
        "namespaceImportPackages" => namespace_import_packages,
        "barrelImportPackages" => barrel_import_packages,
        "importAliases" => import_aliases,
        "topLevelNamedReexports" => top_level_named_reexports,
        "keyPatterns" => key_patterns,
        "extendedKeyDetection" => extended_key_detection,
        "pipeableMinArgCount" => pipeable_min_arg_count,
        "allowedUnstableApis" => allowed_unstable_apis,
        "allowedExperimentalApis" => allowed_experimental_apis,
        "allowedDuplicatedPackages" => allowed_duplicated_packages,
        "effectFn" => effect_fn,
    }
    if has("diagnosticSeverity") {
        merged.diagnostic_severity = merge_severity_maps(
            merged.diagnostic_severity.as_ref(),
            source.diagnostic_severity.as_ref(),
        );
    }
    if has("overrides") {
        merged.overrides.extend(source.overrides.iter().cloned());
    }
    merged
}

// Go: effectconfigraw cloneEffectOptions
/// A copy of the options. Go copies the root lists with
/// `append([]string(nil), ...)`, which turns `[]` into nil: through
/// `extends`, a root `[]` is not written to build info. The other fields keep
/// the plain clone (their empty and nil forms already write the same).
fn clone_effect_options(source: &EffectPluginOptions) -> EffectPluginOptions {
    let mut cloned = source.clone();
    for list in [
        &mut cloned.allowed_unstable_apis,
        &mut cloned.allowed_experimental_apis,
        &mut cloned.allowed_duplicated_packages,
    ] {
        if list.as_ref().is_some_and(Vec::is_empty) {
            *list = None;
        }
    }
    cloned
}

// Go: mergeSeverityMaps. Go nil stays None only when both are nil.
fn merge_severity_maps<V: Clone>(
    target: Option<&IndexMap<String, V>>,
    source: Option<&IndexMap<String, V>>,
) -> Option<IndexMap<String, V>> {
    let Some(source) = source else {
        return target.cloned();
    };
    let mut merged = target.cloned().unwrap_or_default();
    for (k, v) in source {
        merged.insert(k.clone(), v.clone());
    }
    Some(merged)
}

fn rewrite_effect_options_overrides(
    effect: &mut EffectPluginOptions,
    source_config_path: &str,
    base_path: &str,
) {
    if effect.overrides.is_empty() || source_config_path.is_empty() {
        return;
    }
    let options = ComparePathsOptions {
        use_case_sensitive_file_names: true,
        current_directory: base_path.to_string(),
    };
    let relative_difference =
        convert_to_relative_path(&get_directory_path(source_config_path), &options);
    if relative_difference.is_empty() {
        return;
    }
    for o in &mut effect.overrides {
        o.include = rewrite_specs(&o.include, &relative_difference);
        o.exclude = rewrite_specs(&o.exclude, &relative_difference);
    }
}

fn rewrite_specs(specs: &[String], relative_difference: &str) -> Vec<String> {
    specs
        .iter()
        .map(|spec| {
            if starts_with_config_dir_template(spec) || is_rooted_disk_path(spec) {
                spec.clone()
            } else {
                combine_paths(relative_difference, &[spec])
            }
        })
        .collect()
}

fn get_effect_plugin_raw(
    raw: Option<&IndexMap<String, CompilerOptionsValue>>,
) -> Option<&IndexMap<String, CompilerOptionsValue>> {
    let CompilerOptionsValue::Map(compiler_options) = raw?.get("compilerOptions")? else {
        return None;
    };
    let CompilerOptionsValue::List(plugins) = compiler_options.get("plugins")? else {
        return None;
    };
    plugins.iter().find_map(|plugin| match plugin {
        CompilerOptionsValue::Map(plugin)
            if matches!(plugin.get("name"), Some(CompilerOptionsValue::String(name)) if name == EFFECT_PLUGIN_NAME) =>
        {
            Some(plugin)
        }
        _ => None,
    })
}

const CONFIG_DIR_TEMPLATE: &str = "${configDir}";

fn starts_with_config_dir_template(value: &str) -> bool {
    value
        .to_lowercase()
        .starts_with(&CONFIG_DIR_TEMPLATE.to_lowercase())
}
