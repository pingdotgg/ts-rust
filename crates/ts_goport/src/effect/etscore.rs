//! Port of Effect-TS/tsgo `etscore` at `@effect/tsgo@0.51.1`: the
//! `@effect/language-service` plugin options, severities and the command
//! line mode flag.

use crate::frontend::tsoptions::command_line_option::CompilerOptionsValue;
use indexmap::IndexMap;
use std::sync::atomic::{AtomicBool, Ordering};

// Go: etscore/consts.go
pub const EFFECT_PLUGIN_NAME: &str = "@effect/language-service";

// Go: etscore/version_generated.go EffectVersion
/// The `@effect/tsgo` release this port follows. Update it with the port.
/// Build info written with the rules on records it (`build_info_version`).
pub const EFFECT_VERSION: &str = "0.51.1";

// Go: etscore/severity.go
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Severity {
    #[default]
    Off,
    Warning,
    Error,
    Suggestion,
    Message,
    /// Used in directives only, not in tsconfig.
    SkipFile,
}

impl Severity {
    // Go: etscore/severity.go ParseSeverity
    #[must_use]
    pub fn parse(s: &str) -> Severity {
        match s.trim().to_lowercase().as_str() {
            "off" => Severity::Off,
            "warning" | "warn" => Severity::Warning,
            "error" => Severity::Error,
            "suggestion" => Severity::Suggestion,
            "message" => Severity::Message,
            "skip-file" => Severity::SkipFile,
            // default to error for unknown values
            _ => Severity::Error,
        }
    }

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Off => "off",
            Severity::Warning => "warning",
            Severity::Error => "error",
            Severity::Suggestion => "suggestion",
            Severity::Message => "message",
            Severity::SkipFile => "skip-file",
        }
    }

    #[must_use]
    pub fn is_off(self) -> bool {
        self == Severity::Off || self == Severity::SkipFile
    }

    fn visibility_rank(self) -> i32 {
        match self {
            Severity::Error => 4,
            Severity::Warning => 3,
            Severity::Suggestion => 2,
            Severity::Message => 1,
            _ => 0,
        }
    }

    #[must_use]
    pub fn at_least_as_visible_as(self, min: Severity) -> bool {
        self.visibility_rank() >= min.visibility_rank()
    }
}

// Go: etscore/climode.go
static COMMAND_LINE_MODE: AtomicBool = AtomicBool::new(false);

/// Go `EnterCommandLineMode`: sets the mode and returns a guard that restores
/// the previous value when dropped.
#[must_use]
pub fn enter_command_line_mode() -> CommandLineModeGuard {
    CommandLineModeGuard {
        prev: COMMAND_LINE_MODE.swap(true, Ordering::SeqCst),
    }
}

pub struct CommandLineModeGuard {
    prev: bool,
}

impl Drop for CommandLineModeGuard {
    fn drop(&mut self) {
        COMMAND_LINE_MODE.store(self.prev, Ordering::SeqCst);
    }
}

#[must_use]
pub fn is_command_line_mode() -> bool {
    COMMAND_LINE_MODE.load(Ordering::SeqCst)
}

// Go: etscore/options.go
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct KeyPattern {
    pub target: String,
    pub pattern: String,
    pub skip_leading_path: Vec<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TopLevelNamedReexportsMode {
    /// Go `""`, the zero value.
    #[default]
    Unset,
    Ignore,
    Follow,
}

/// Go `EffectPluginOptions`. Go `map[string]Severity` is an `IndexMap` (only
/// lookups read it). Go nil slices and maps are `None` where Go tells nil
/// from empty (`DiagnosticSeverity`; the 3 strict allow lists, which build
/// info writes as `[]` when the config sets `[]`), else empty.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EffectPluginOptions {
    pub refactors: bool,
    pub diagnostics: bool,
    pub include_suggestions_in_tsc: bool,
    pub quickinfo: bool,
    pub completions: bool,
    pub debug: bool,
    pub goto: bool,
    pub renames: bool,
    pub ignore_effect_suggestions_in_tsc_exit_code: bool,
    pub ignore_effect_warnings_in_tsc_exit_code: bool,
    pub ignore_effect_errors_in_tsc_exit_code: bool,
    pub skip_disabled_optimization: bool,
    pub mermaid_provider: String,
    pub no_external: bool,
    pub layer_graph_follow_depth: i64,
    pub inlays: bool,
    pub namespace_import_packages: Vec<String>,
    pub barrel_import_packages: Vec<String>,
    pub import_aliases: IndexMap<String, String>,
    pub top_level_named_reexports: TopLevelNamedReexportsMode,
    pub key_patterns: Vec<KeyPattern>,
    pub extended_key_detection: bool,
    pub pipeable_min_arg_count: i64,
    pub allowed_unstable_apis: Option<Vec<String>>,
    pub allowed_experimental_apis: Option<Vec<String>>,
    pub allowed_duplicated_packages: Option<Vec<String>>,
    pub effect_fn: Vec<String>,
    pub diagnostic_severity: Option<IndexMap<String, Severity>>,
    pub overrides: Vec<Override>,
}

/// Go `ResolvedEffectPluginOptions` (alias `EffectPluginDiagnosticsOptions`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ResolvedEffectPluginOptions {
    pub namespace_import_packages: Vec<String>,
    pub barrel_import_packages: Vec<String>,
    pub import_aliases: IndexMap<String, String>,
    pub top_level_named_reexports: TopLevelNamedReexportsMode,
    pub key_patterns: Vec<KeyPattern>,
    pub extended_key_detection: bool,
    pub pipeable_min_arg_count: i64,
    pub allowed_duplicated_packages: Vec<String>,
    pub allowed_unstable_apis: Vec<String>,
    pub allowed_experimental_apis: Vec<String>,
    pub effect_fn: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Override {
    pub include: Vec<String>,
    pub exclude: Vec<String>,
    pub options: OverrideOptions,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OverrideOptions {
    pub diagnostic_severity: Option<IndexMap<String, Severity>>,
    pub pipeable_min_arg_count: Option<i64>,
    pub key_patterns: Option<Vec<KeyPattern>>,
    pub extended_key_detection: Option<bool>,
    pub allowed_unstable_apis: Option<Vec<String>>,
    pub allowed_experimental_apis: Option<Vec<String>>,
    pub allowed_duplicated_packages: Option<Vec<String>>,
    pub effect_fn: Option<Vec<String>>,
}

pub const EFFECT_FN_SPAN: &str = "span";
pub const EFFECT_FN_UNTRACED: &str = "untraced";
pub const EFFECT_FN_NO_SPAN: &str = "no-span";
pub const EFFECT_FN_INFERRED_SPAN: &str = "inferred-span";
pub const EFFECT_FN_SUGGESTED_SPAN: &str = "suggested-span";

const DEFAULT_EFFECT_FN: &[&str] = &[EFFECT_FN_SPAN];

// Go: etscore/options.go DiagnosticsEnabled
#[must_use]
pub fn diagnostics_enabled(config: Option<&EffectPluginOptions>) -> bool {
    config.is_some_and(|c| c.diagnostics && c.diagnostic_severity.is_some())
}

impl EffectPluginOptions {
    // Go: GetIncludeSuggestionsInTsc (nil receiver is true; callers pass Some)
    #[must_use]
    pub fn get_include_suggestions_in_tsc(&self) -> bool {
        self.include_suggestions_in_tsc
    }

    #[must_use]
    pub fn get_layer_graph_follow_depth(&self) -> i64 {
        self.layer_graph_follow_depth.max(0)
    }

    #[must_use]
    pub fn get_namespace_import_packages(&self) -> Vec<String> {
        normalize_package_list(&self.namespace_import_packages)
    }

    #[must_use]
    pub fn get_barrel_import_packages(&self) -> Vec<String> {
        normalize_package_list(&self.barrel_import_packages)
    }
}

impl ResolvedEffectPluginOptions {
    #[must_use]
    pub fn get_effect_fn(&self) -> Vec<String> {
        if self.effect_fn.is_empty() {
            return DEFAULT_EFFECT_FN.iter().map(|s| (*s).to_string()).collect();
        }
        self.effect_fn.clone()
    }

    #[must_use]
    pub fn effect_fn_includes(&self, variant: &str) -> bool {
        self.get_effect_fn().iter().any(|v| v == variant)
    }

    #[must_use]
    pub fn get_pipeable_min_arg_count(&self) -> i64 {
        if self.pipeable_min_arg_count > 0 {
            return self.pipeable_min_arg_count;
        }
        2
    }

    #[must_use]
    pub fn get_key_patterns(&self) -> Vec<KeyPattern> {
        if self.key_patterns.is_empty() {
            return default_key_patterns();
        }
        self.key_patterns.clone()
    }

    #[must_use]
    pub fn get_allowed_duplicated_packages(&self) -> &[String] {
        &self.allowed_duplicated_packages
    }

    #[must_use]
    pub fn get_namespace_import_packages(&self) -> Vec<String> {
        normalize_package_list(&self.namespace_import_packages)
    }
}

// Go: etscore/options.go DefaultKeyPatterns
#[must_use]
pub fn default_key_patterns() -> Vec<KeyPattern> {
    vec![
        KeyPattern {
            target: "service".into(),
            pattern: "default".into(),
            skip_leading_path: vec!["src/".into()],
        },
        KeyPattern {
            target: "custom".into(),
            pattern: "default".into(),
            skip_leading_path: vec!["src/".into()],
        },
    ]
}

// Go: etscore/options.go normalizePackageName
#[must_use]
pub fn normalize_package_name(name: &str) -> String {
    name.trim().to_lowercase()
}

// Go: etscore/options.go normalizePackageList
#[must_use]
pub fn normalize_package_list(packages: &[String]) -> Vec<String> {
    let mut result = Vec::with_capacity(packages.len());
    for p in packages {
        let normalized = normalize_package_name(p);
        if !normalized.is_empty() {
            result.push(normalized);
        }
    }
    result
}

// Go: etscore/options_parser.go

type Getter<'a> = &'a IndexMap<String, CompilerOptionsValue>;

fn as_map(value: &CompilerOptionsValue) -> Option<Getter<'_>> {
    match value {
        CompilerOptionsValue::Map(m) => Some(m),
        _ => None,
    }
}

fn as_bool(value: &CompilerOptionsValue) -> Option<bool> {
    match value {
        CompilerOptionsValue::Bool(b) => Some(*b),
        _ => None,
    }
}

fn as_str(value: &CompilerOptionsValue) -> Option<&str> {
    match value {
        CompilerOptionsValue::String(s) => Some(s),
        _ => None,
    }
}

/// Go `val.(float64)` then `int(f)`.
fn as_int(value: &CompilerOptionsValue) -> Option<i64> {
    match value {
        #[allow(clippy::cast_possible_truncation)]
        CompilerOptionsValue::Number(f) => Some(*f as i64),
        CompilerOptionsValue::Int(i) => Some(*i),
        _ => None,
    }
}

/// Go `val.([]any)`. A Go `[]any(nil)` is a list with no elements.
fn as_list(value: &CompilerOptionsValue) -> Option<&[CompilerOptionsValue]> {
    match value {
        CompilerOptionsValue::List(l) => Some(l),
        CompilerOptionsValue::NilList => Some(&[]),
        _ => None,
    }
}

fn string_items(list: &[CompilerOptionsValue]) -> Vec<String> {
    list.iter().filter_map(as_str).map(str::to_string).collect()
}

// Go: etscore/options_parser.go ParseFromPlugins
#[must_use]
pub fn parse_from_plugins(value: &CompilerOptionsValue) -> Option<EffectPluginOptions> {
    let plugins = as_list(value)?;
    for p in plugins {
        let Some(plugin) = as_map(p) else {
            continue;
        };
        if plugin.get("name").and_then(as_str) != Some(EFFECT_PLUGIN_NAME) {
            continue;
        }
        return parse_plugin(plugin);
    }
    None
}

fn parse_plugin(plugin: Getter<'_>) -> Option<EffectPluginOptions> {
    let mut result = EffectPluginOptions {
        refactors: true,
        diagnostics: true,
        include_suggestions_in_tsc: true,
        quickinfo: true,
        completions: true,
        goto: true,
        renames: true,
        ignore_effect_suggestions_in_tsc_exit_code: true,
        ignore_effect_warnings_in_tsc_exit_code: false,
        diagnostic_severity: Some(IndexMap::new()),
        ..Default::default()
    };
    let get = |key: &str| plugin.get(key);
    let set_bool = |key: &str, field: &mut bool| {
        if let Some(b) = get(key).and_then(as_bool) {
            *field = b;
        }
    };

    set_bool("refactors", &mut result.refactors);
    set_bool("diagnostics", &mut result.diagnostics);
    if let Some(diag) = get("diagnosticSeverity") {
        if diag.is_nil() {
            return None;
        }
        result.diagnostic_severity = Some(parse_diagnostic_severity_map(diag));
    }
    if let Some(val) = get("overrides") {
        if let Some(overrides) = parse_overrides(val) {
            result.overrides = overrides;
        }
    }
    set_bool(
        "includeSuggestionsInTsc",
        &mut result.include_suggestions_in_tsc,
    );
    set_bool("quickinfo", &mut result.quickinfo);
    set_bool("completions", &mut result.completions);
    set_bool("debug", &mut result.debug);
    set_bool("goto", &mut result.goto);
    set_bool("renames", &mut result.renames);
    set_bool(
        "ignoreEffectSuggestionsInTscExitCode",
        &mut result.ignore_effect_suggestions_in_tsc_exit_code,
    );
    set_bool(
        "ignoreEffectWarningsInTscExitCode",
        &mut result.ignore_effect_warnings_in_tsc_exit_code,
    );
    set_bool(
        "ignoreEffectErrorsInTscExitCode",
        &mut result.ignore_effect_errors_in_tsc_exit_code,
    );
    set_bool(
        "skipDisabledOptimization",
        &mut result.skip_disabled_optimization,
    );
    if let Some(arr) = get("keyPatterns").and_then(as_list) {
        result.key_patterns = parse_key_patterns(arr);
    }
    set_bool("extendedKeyDetection", &mut result.extended_key_detection);
    if let Some(n) = get("pipeableMinArgCount").and_then(as_int) {
        result.pipeable_min_arg_count = n;
    }
    if let Some(s) = get("mermaidProvider").and_then(as_str) {
        result.mermaid_provider = s.to_string();
    }
    set_bool("noExternal", &mut result.no_external);
    if let Some(n) = get("layerGraphFollowDepth").and_then(as_int) {
        result.layer_graph_follow_depth = n;
    }
    set_bool("inlays", &mut result.inlays);
    if let Some(arr) = get("effectFn").and_then(as_list) {
        result.effect_fn = string_items(arr);
    }
    if let Some(apis) = get("allowedUnstableApis").and_then(parse_string_array_strict) {
        result.allowed_unstable_apis = Some(apis);
    }
    if let Some(apis) = get("allowedExperimentalApis").and_then(parse_string_array_strict) {
        result.allowed_experimental_apis = Some(apis);
    }
    // Go: lenient (a non-string item is skipped), and `[]` is not nil.
    if let Some(arr) = get("allowedDuplicatedPackages").and_then(as_list) {
        result.allowed_duplicated_packages = Some(string_items(arr));
    }
    if let Some(pkgs) =
        get("namespaceImportPackages").and_then(parse_normalized_string_array_strict)
    {
        result.namespace_import_packages = pkgs;
    }
    if let Some(pkgs) = get("barrelImportPackages").and_then(parse_normalized_string_array_strict) {
        result.barrel_import_packages = pkgs;
    }
    if let Some(aliases) = get("importAliases").and_then(parse_normalized_string_map_strict) {
        result.import_aliases = aliases;
    }
    if let Some(s) = get("topLevelNamedReexports").and_then(as_str) {
        match s.to_lowercase().as_str() {
            "ignore" => result.top_level_named_reexports = TopLevelNamedReexportsMode::Ignore,
            "follow" => result.top_level_named_reexports = TopLevelNamedReexportsMode::Follow,
            _ => {}
        }
    }
    Some(result)
}

fn parse_normalized_string_array_strict(value: &CompilerOptionsValue) -> Option<Vec<String>> {
    let arr = as_list(value)?;
    let mut result = Vec::with_capacity(arr.len());
    for item in arr {
        let s = as_str(item)?;
        let normalized = normalize_package_name(s);
        if !normalized.is_empty() {
            result.push(normalized);
        }
    }
    Some(result)
}

fn parse_normalized_string_map_strict(
    value: &CompilerOptionsValue,
) -> Option<IndexMap<String, String>> {
    let map = as_map(value)?;
    let mut result = IndexMap::new();
    for (k, v) in map {
        let s = as_str(v)?;
        let normalized = normalize_package_name(k);
        if !normalized.is_empty() {
            result.insert(normalized, s.to_string());
        }
    }
    Some(result)
}

fn parse_overrides(value: &CompilerOptionsValue) -> Option<Vec<Override>> {
    let arr = as_list(value)?;
    let mut result = Vec::with_capacity(arr.len());
    for item in arr {
        let Some(scope) = as_map(item) else {
            continue;
        };
        let mut o = Override::default();
        if let Some(include) = scope.get("include") {
            o.include = parse_string_array_lossy(include);
        }
        if let Some(exclude) = scope.get("exclude") {
            o.exclude = parse_string_array_lossy(exclude);
        }
        if let Some(options) = scope.get("options") {
            o.options = parse_override_options(options);
        }
        result.push(o);
    }
    Some(result)
}

fn parse_override_options(value: &CompilerOptionsValue) -> OverrideOptions {
    let mut result = OverrideOptions::default();
    let Some(options) = as_map(value) else {
        return result;
    };
    if let Some(v) = options.get("diagnosticSeverity") {
        result.diagnostic_severity = Some(parse_diagnostic_severity_map(v));
    }
    if let Some(n) = options.get("pipeableMinArgCount").and_then(as_int) {
        result.pipeable_min_arg_count = Some(n);
    }
    if let Some(arr) = options.get("keyPatterns").and_then(as_list) {
        result.key_patterns = Some(parse_key_patterns(arr));
    }
    if let Some(b) = options.get("extendedKeyDetection").and_then(as_bool) {
        result.extended_key_detection = Some(b);
    }
    if let Some(apis) = options
        .get("allowedUnstableApis")
        .and_then(parse_string_array_strict)
    {
        result.allowed_unstable_apis = Some(apis);
    }
    if let Some(apis) = options
        .get("allowedExperimentalApis")
        .and_then(parse_string_array_strict)
    {
        result.allowed_experimental_apis = Some(apis);
    }
    if let Some(pkgs) = options
        .get("allowedDuplicatedPackages")
        .and_then(parse_string_array_strict)
    {
        result.allowed_duplicated_packages = Some(pkgs);
    }
    if let Some(variants) = options.get("effectFn").and_then(parse_string_array_strict) {
        result.effect_fn = Some(variants);
    }
    result
}

fn parse_string_array_strict(value: &CompilerOptionsValue) -> Option<Vec<String>> {
    let arr = as_list(value)?;
    arr.iter()
        .map(|item| as_str(item).map(str::to_string))
        .collect()
}

fn parse_string_array_lossy(value: &CompilerOptionsValue) -> Vec<String> {
    as_list(value).map(string_items).unwrap_or_default()
}

fn parse_diagnostic_severity_map(value: &CompilerOptionsValue) -> IndexMap<String, Severity> {
    let mut result = IndexMap::new();
    if let Some(m) = as_map(value) {
        for (k, v) in m {
            if let Some(s) = as_str(v) {
                result.insert(k.clone(), Severity::parse(s));
            }
        }
    }
    result
}

fn parse_key_patterns(arr: &[CompilerOptionsValue]) -> Vec<KeyPattern> {
    let mut patterns = Vec::new();
    for item in arr {
        let Some(m) = as_map(item) else {
            continue;
        };
        let mut kp = KeyPattern {
            target: "service".into(),
            pattern: "default".into(),
            skip_leading_path: vec!["src/".into()],
        };
        if let Some(s) = m.get("target").and_then(as_str) {
            kp.target = s.to_string();
        }
        if let Some(s) = m.get("pattern").and_then(as_str) {
            kp.pattern = s.to_string();
        }
        if let Some(arr) = m.get("skipLeadingPath").and_then(as_list) {
            kp.skip_leading_path = string_items(arr);
        }
        patterns.push(kp);
    }
    patterns
}

// Go: the JSON form of `EffectPluginOptions` (its `json:",omitzero"` tags),
// which patch 028 writes to `.tsbuildinfo` as `effect`.
impl EffectPluginOptions {
    /// The options as a JSON value. Zero fields are left out, as Go
    /// `omitzero` does.
    #[must_use]
    pub fn to_value(&self) -> CompilerOptionsValue {
        let mut m = IndexMap::new();
        let mut b = |k: &str, v: bool| {
            if v {
                m.insert(k.to_string(), CompilerOptionsValue::Bool(true));
            }
        };
        b("refactors", self.refactors);
        b("diagnostics", self.diagnostics);
        b("includeSuggestionsInTsc", self.include_suggestions_in_tsc);
        b("quickinfo", self.quickinfo);
        b("completions", self.completions);
        b("debug", self.debug);
        b("goto", self.goto);
        b("renames", self.renames);
        b(
            "ignoreEffectSuggestionsInTscExitCode",
            self.ignore_effect_suggestions_in_tsc_exit_code,
        );
        b(
            "ignoreEffectWarningsInTscExitCode",
            self.ignore_effect_warnings_in_tsc_exit_code,
        );
        b(
            "ignoreEffectErrorsInTscExitCode",
            self.ignore_effect_errors_in_tsc_exit_code,
        );
        b("skipDisabledOptimization", self.skip_disabled_optimization);
        if !self.mermaid_provider.is_empty() {
            m.insert(
                "mermaidProvider".into(),
                CompilerOptionsValue::String(self.mermaid_provider.clone()),
            );
        }
        if self.no_external {
            m.insert("noExternal".into(), CompilerOptionsValue::Bool(true));
        }
        if self.layer_graph_follow_depth != 0 {
            m.insert(
                "layerGraphFollowDepth".into(),
                CompilerOptionsValue::Int(self.layer_graph_follow_depth),
            );
        }
        if self.inlays {
            m.insert("inlays".into(), CompilerOptionsValue::Bool(true));
        }
        let list = |v: &[String]| {
            CompilerOptionsValue::List(
                v.iter()
                    .cloned()
                    .map(CompilerOptionsValue::String)
                    .collect(),
            )
        };
        if !self.namespace_import_packages.is_empty() {
            m.insert(
                "namespaceImportPackages".into(),
                list(&self.namespace_import_packages),
            );
        }
        if !self.barrel_import_packages.is_empty() {
            m.insert(
                "barrelImportPackages".into(),
                list(&self.barrel_import_packages),
            );
        }
        if !self.import_aliases.is_empty() {
            m.insert(
                "importAliases".into(),
                CompilerOptionsValue::Map(
                    self.import_aliases
                        .iter()
                        .map(|(k, v)| (k.clone(), CompilerOptionsValue::String(v.clone())))
                        .collect(),
                ),
            );
        }
        match self.top_level_named_reexports {
            TopLevelNamedReexportsMode::Unset => {}
            TopLevelNamedReexportsMode::Ignore => {
                m.insert(
                    "topLevelNamedReexports".into(),
                    CompilerOptionsValue::String("ignore".into()),
                );
            }
            TopLevelNamedReexportsMode::Follow => {
                m.insert(
                    "topLevelNamedReexports".into(),
                    CompilerOptionsValue::String("follow".into()),
                );
            }
        }
        if !self.key_patterns.is_empty() {
            m.insert("keyPatterns".into(), key_patterns_value(&self.key_patterns));
        }
        if self.extended_key_detection {
            m.insert(
                "extendedKeyDetection".into(),
                CompilerOptionsValue::Bool(true),
            );
        }
        if self.pipeable_min_arg_count != 0 {
            m.insert(
                "pipeableMinArgCount".into(),
                CompilerOptionsValue::Int(self.pipeable_min_arg_count),
            );
        }
        // Go `omitzero` leaves out only a nil slice, so `[]` is written.
        if let Some(apis) = &self.allowed_unstable_apis {
            m.insert("allowedUnstableApis".into(), list(apis));
        }
        if let Some(apis) = &self.allowed_experimental_apis {
            m.insert("allowedExperimentalApis".into(), list(apis));
        }
        if let Some(pkgs) = &self.allowed_duplicated_packages {
            m.insert("allowedDuplicatedPackages".into(), list(pkgs));
        }
        if !self.effect_fn.is_empty() {
            m.insert("effectFn".into(), list(&self.effect_fn));
        }
        if let Some(severity) = &self.diagnostic_severity {
            m.insert("diagnosticSeverity".into(), severity_value(severity));
        }
        if !self.overrides.is_empty() {
            let overrides = self
                .overrides
                .iter()
                .map(|o| {
                    let mut om = IndexMap::new();
                    if !o.include.is_empty() {
                        om.insert("include".to_string(), list(&o.include));
                    }
                    if !o.exclude.is_empty() {
                        om.insert("exclude".to_string(), list(&o.exclude));
                    }
                    let mut opts = IndexMap::new();
                    if let Some(sev) = &o.options.diagnostic_severity {
                        opts.insert("diagnosticSeverity".to_string(), severity_value(sev));
                    }
                    if let Some(n) = o.options.pipeable_min_arg_count {
                        opts.insert(
                            "pipeableMinArgCount".to_string(),
                            CompilerOptionsValue::Int(n),
                        );
                    }
                    if let Some(kp) = &o.options.key_patterns {
                        opts.insert("keyPatterns".to_string(), key_patterns_value(kp));
                    }
                    if let Some(b) = o.options.extended_key_detection {
                        opts.insert(
                            "extendedKeyDetection".to_string(),
                            CompilerOptionsValue::Bool(b),
                        );
                    }
                    if let Some(p) = &o.options.allowed_unstable_apis {
                        opts.insert("allowedUnstableApis".to_string(), list(p));
                    }
                    if let Some(p) = &o.options.allowed_experimental_apis {
                        opts.insert("allowedExperimentalApis".to_string(), list(p));
                    }
                    if let Some(p) = &o.options.allowed_duplicated_packages {
                        opts.insert("allowedDuplicatedPackages".to_string(), list(p));
                    }
                    if let Some(v) = &o.options.effect_fn {
                        opts.insert("effectFn".to_string(), list(v));
                    }
                    if !opts.is_empty() {
                        om.insert("options".to_string(), CompilerOptionsValue::Map(opts));
                    }
                    CompilerOptionsValue::Map(om)
                })
                .collect();
            m.insert("overrides".into(), CompilerOptionsValue::List(overrides));
        }
        CompilerOptionsValue::Map(m)
    }

    /// The options from their JSON value (`to_value`). Missing fields are
    /// zero, as Go `json.Unmarshal` leaves them.
    #[must_use]
    pub fn from_value(value: &CompilerOptionsValue) -> Option<EffectPluginOptions> {
        let m = as_map(value)?;
        let b = |k: &str| m.get(k).and_then(as_bool).unwrap_or(false);
        let strings = |k: &str| {
            m.get(k)
                .and_then(as_list)
                .map(string_items)
                .unwrap_or_default()
        };
        let opt_strings = |k: &str| m.get(k).and_then(as_list).map(string_items);
        let mut result = EffectPluginOptions {
            refactors: b("refactors"),
            diagnostics: b("diagnostics"),
            include_suggestions_in_tsc: b("includeSuggestionsInTsc"),
            quickinfo: b("quickinfo"),
            completions: b("completions"),
            debug: b("debug"),
            goto: b("goto"),
            renames: b("renames"),
            ignore_effect_suggestions_in_tsc_exit_code: b("ignoreEffectSuggestionsInTscExitCode"),
            ignore_effect_warnings_in_tsc_exit_code: b("ignoreEffectWarningsInTscExitCode"),
            ignore_effect_errors_in_tsc_exit_code: b("ignoreEffectErrorsInTscExitCode"),
            skip_disabled_optimization: b("skipDisabledOptimization"),
            mermaid_provider: m
                .get("mermaidProvider")
                .and_then(as_str)
                .unwrap_or("")
                .to_string(),
            no_external: b("noExternal"),
            layer_graph_follow_depth: m.get("layerGraphFollowDepth").and_then(as_int).unwrap_or(0),
            inlays: b("inlays"),
            namespace_import_packages: strings("namespaceImportPackages"),
            barrel_import_packages: strings("barrelImportPackages"),
            import_aliases: m
                .get("importAliases")
                .and_then(as_map)
                .map(|a| {
                    a.iter()
                        .filter_map(|(k, v)| Some((k.clone(), as_str(v)?.to_string())))
                        .collect()
                })
                .unwrap_or_default(),
            top_level_named_reexports: match m.get("topLevelNamedReexports").and_then(as_str) {
                Some("ignore") => TopLevelNamedReexportsMode::Ignore,
                Some("follow") => TopLevelNamedReexportsMode::Follow,
                _ => TopLevelNamedReexportsMode::Unset,
            },
            key_patterns: m
                .get("keyPatterns")
                .and_then(as_list)
                .map(parse_key_patterns)
                .unwrap_or_default(),
            extended_key_detection: b("extendedKeyDetection"),
            pipeable_min_arg_count: m.get("pipeableMinArgCount").and_then(as_int).unwrap_or(0),
            allowed_unstable_apis: opt_strings("allowedUnstableApis"),
            allowed_experimental_apis: opt_strings("allowedExperimentalApis"),
            allowed_duplicated_packages: opt_strings("allowedDuplicatedPackages"),
            effect_fn: strings("effectFn"),
            diagnostic_severity: m
                .get("diagnosticSeverity")
                .map(parse_diagnostic_severity_map),
            overrides: Vec::new(),
        };
        if let Some(overrides) = m.get("overrides") {
            result.overrides = parse_overrides(overrides).unwrap_or_default();
        }
        Some(result)
    }
}

fn severity_value(severity: &IndexMap<String, Severity>) -> CompilerOptionsValue {
    CompilerOptionsValue::Map(
        severity
            .iter()
            .map(|(k, v)| {
                (
                    k.clone(),
                    CompilerOptionsValue::String(v.as_str().to_string()),
                )
            })
            .collect(),
    )
}

fn key_patterns_value(patterns: &[KeyPattern]) -> CompilerOptionsValue {
    CompilerOptionsValue::List(
        patterns
            .iter()
            .map(|kp| {
                let mut m = IndexMap::new();
                m.insert(
                    "target".to_string(),
                    CompilerOptionsValue::String(kp.target.clone()),
                );
                m.insert(
                    "pattern".to_string(),
                    CompilerOptionsValue::String(kp.pattern.clone()),
                );
                m.insert(
                    "skipLeadingPath".to_string(),
                    CompilerOptionsValue::List(
                        kp.skip_leading_path
                            .iter()
                            .cloned()
                            .map(CompilerOptionsValue::String)
                            .collect(),
                    ),
                );
                CompilerOptionsValue::Map(m)
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_severity_matches_go() {
        for (input, want) in [
            ("off", Severity::Off),
            ("WARN", Severity::Warning),
            ("warning", Severity::Warning),
            ("Error", Severity::Error),
            ("suggestion", Severity::Suggestion),
            ("message", Severity::Message),
            ("skip-file", Severity::SkipFile),
            ("unknown", Severity::Error),
            ("", Severity::Error),
        ] {
            assert_eq!(Severity::parse(input), want, "{input}");
        }
    }

    #[test]
    fn build_info_value_round_trips() {
        let plugin = CompilerOptionsValue::List(vec![CompilerOptionsValue::Map(
            [
                (
                    "name",
                    CompilerOptionsValue::String(EFFECT_PLUGIN_NAME.into()),
                ),
                (
                    "namespaceImportPackages",
                    CompilerOptionsValue::List(vec![CompilerOptionsValue::String("effect".into())]),
                ),
                (
                    "diagnosticSeverity",
                    CompilerOptionsValue::Map(
                        [(
                            "globalDate".to_string(),
                            CompilerOptionsValue::String("error".into()),
                        )]
                        .into_iter()
                        .collect(),
                    ),
                ),
                (
                    "overrides",
                    CompilerOptionsValue::List(vec![CompilerOptionsValue::Map(
                        [
                            (
                                "include".to_string(),
                                CompilerOptionsValue::List(vec![CompilerOptionsValue::String(
                                    "**/*.test.ts".into(),
                                )]),
                            ),
                            (
                                "options".to_string(),
                                CompilerOptionsValue::Map(
                                    [(
                                        "diagnosticSeverity".to_string(),
                                        CompilerOptionsValue::Map(
                                            [(
                                                "preferSchemaOverJson".to_string(),
                                                CompilerOptionsValue::String("off".into()),
                                            )]
                                            .into_iter()
                                            .collect(),
                                        ),
                                    )]
                                    .into_iter()
                                    .collect(),
                                ),
                            ),
                        ]
                        .into_iter()
                        .collect(),
                    )]),
                ),
            ]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect(),
        )]);
        let options = parse_from_plugins(&plugin).expect("plugin");
        assert!(options.include_suggestions_in_tsc);
        assert_eq!(
            EffectPluginOptions::from_value(&options.to_value()),
            Some(options)
        );
    }

    /// The allow lists of effect-tsgo 0.51.1: a strict parse at the root and in
    /// an override, `[]` kept apart from absent, and the build info order
    /// (`pipeableMinArgCount`, the 2 API lists, `allowedDuplicatedPackages`).
    #[test]
    fn allow_lists_parse_and_round_trip() {
        let strings = |items: &[&str]| {
            CompilerOptionsValue::List(
                items
                    .iter()
                    .map(|s| CompilerOptionsValue::String((*s).into()))
                    .collect(),
            )
        };
        let map = |pairs: Vec<(&str, CompilerOptionsValue)>| {
            CompilerOptionsValue::Map(pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
        };
        let plugin = CompilerOptionsValue::List(vec![map(vec![
            (
                "name",
                CompilerOptionsValue::String(EFFECT_PLUGIN_NAME.into()),
            ),
            ("allowedUnstableApis", strings(&["effect/cli/Command#make"])),
            ("allowedExperimentalApis", strings(&[])),
            (
                "allowedDuplicatedPackages",
                CompilerOptionsValue::List(vec![
                    CompilerOptionsValue::String("x".into()),
                    CompilerOptionsValue::Int(1),
                ]),
            ),
            ("pipeableMinArgCount", CompilerOptionsValue::Int(3)),
            (
                "overrides",
                CompilerOptionsValue::List(vec![map(vec![
                    ("include", strings(&["src/a.ts"])),
                    (
                        "options",
                        map(vec![
                            ("allowedUnstableApis", strings(&[])),
                            (
                                "allowedExperimentalApis",
                                CompilerOptionsValue::List(vec![
                                    CompilerOptionsValue::String("q".into()),
                                    CompilerOptionsValue::Int(1),
                                ]),
                            ),
                        ]),
                    ),
                ])]),
            ),
        ])]);
        let options = parse_from_plugins(&plugin).expect("plugin");
        assert_eq!(
            options.allowed_unstable_apis,
            Some(vec!["effect/cli/Command#make".to_string()])
        );
        assert_eq!(options.allowed_experimental_apis, Some(Vec::new()));
        // The root `allowedDuplicatedPackages` parse skips a non-string item.
        assert_eq!(
            options.allowed_duplicated_packages,
            Some(vec!["x".to_string()])
        );
        let o = &options.overrides[0].options;
        assert_eq!(o.allowed_unstable_apis, Some(Vec::new()));
        // A strict parse drops the whole list for a non-string item.
        assert_eq!(o.allowed_experimental_apis, None);

        let value = options.to_value();
        let CompilerOptionsValue::Map(m) = &value else {
            panic!("map")
        };
        let keys: Vec<&str> = m.keys().map(String::as_str).collect();
        let at = |k: &str| keys.iter().position(|key| *key == k).expect(k);
        assert!(at("pipeableMinArgCount") < at("allowedUnstableApis"));
        assert!(at("allowedUnstableApis") < at("allowedExperimentalApis"));
        assert!(at("allowedExperimentalApis") < at("allowedDuplicatedPackages"));
        assert!(at("allowedDuplicatedPackages") < at("diagnosticSeverity"));
        assert_eq!(EffectPluginOptions::from_value(&value), Some(options));
    }
}
