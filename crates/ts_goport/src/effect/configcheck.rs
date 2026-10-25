//! Port of Effect-TS/tsgo `internal/effectconfigcheck`: reports rule names
//! in `diagnosticSeverity` that no rule has (`unknownRuleName`).

use crate::effect::diag;
use crate::effect::directives::to_category;
use crate::effect::etscore::{EFFECT_PLUGIN_NAME, Severity, diagnostics_enabled};
use crate::effect::rule::{UNKNOWN_RULE_NAME_NAME, UNUSED_DIRECTIVE_NAME, by_name};
use crate::effect::rules;
use crate::frontend::tsoptions::errors::create_diagnostic_for_node_in_source_file_or_compiler_diagnostic;
use crate::frontend::tsoptions::tsconfig_p2::for_each_tsconfig_prop_array;
use crate::prelude::*;

// Go: effectconfigcheck.validate
/// Patch 030 `ValidateCompilerOptionsCallback`: runs on the final options
/// after `extends` is merged. `source_file` is the tsconfig file node (or nil).
#[must_use]
pub fn validate(options: &CompilerOptions, source_file: Node) -> Vec<Diagnostic> {
    // PORT: a standalone API process reports no Effect diagnostics
    // (`rulerunner::enabled_options`).
    let Some(config) = crate::effect::rulerunner::enabled_options(options) else {
        return Vec::new();
    };
    if !diagnostics_enabled(Some(config)) {
        return Vec::new();
    }
    let severity = config
        .diagnostic_severity
        .as_ref()
        .and_then(|m| m.get(UNKNOWN_RULE_NAME_NAME).copied())
        .unwrap_or(Severity::Warning);
    if severity.is_off() {
        return Vec::new();
    }

    let plugin = effect_plugin_syntax(source_file);
    let mut result = Vec::new();
    let mut check = |severities: Option<&IndexMap<String, Severity>>, syntax: Node| {
        let Some(severities) = severities else {
            return;
        };
        let mut unknown: Vec<&String> = severities
            .keys()
            .filter(|name| {
                name.as_str() != UNUSED_DIRECTIVE_NAME
                    && name.as_str() != UNKNOWN_RULE_NAME_NAME
                    && by_name(rules::ALL, name).is_none()
            })
            .collect();
        unknown.sort();
        for name in unknown {
            let property = find_property(syntax, name);
            let node = if property.is_some() {
                property.name()
            } else {
                Node::NIL
            };
            let mut diagnostic = create_diagnostic_for_node_in_source_file_or_compiler_diagnostic(
                source_file,
                node,
                diag::Unknown_Effect_diagnostic_rule_0_in_diagnosticSeverity_effect_unknownRuleName,
                vec![name.clone()],
            );
            diagnostic.category = to_category(severity);
            result.push(diagnostic);
        }
    };
    check(
        config.diagnostic_severity.as_ref(),
        property_value(plugin, "diagnosticSeverity"),
    );

    let local_overrides: Vec<Node> = array_elements(property_value(plugin, "overrides"))
        .into_iter()
        .filter(|n| is_object_literal_expression(*n))
        .collect();
    let local_start = config.overrides.len() as i64 - local_overrides.len() as i64;
    for (i, o) in config.overrides.iter().enumerate() {
        let mut syntax = Node::NIL;
        if local_start >= 0 && i as i64 >= local_start {
            let local = local_overrides[(i as i64 - local_start) as usize];
            syntax = property_value(property_value(local, "options"), "diagnosticSeverity");
        }
        check(o.options.diagnostic_severity.as_ref(), syntax);
    }
    result
}

fn effect_plugin_syntax(source_file: Node) -> Node {
    let compiler_options =
        for_each_tsconfig_prop_array(source_file, "compilerOptions", Some).unwrap_or(Node::NIL);
    if compiler_options.is_nil() {
        return Node::NIL;
    }
    for plugin in array_elements(property_value(compiler_options.initializer(), "plugins")) {
        let name = property_value(plugin, "name");
        if name.is_some() && is_string_literal_like(name) && name.text() == EFFECT_PLUGIN_NAME {
            return plugin;
        }
    }
    Node::NIL
}

fn find_property(node: Node, name: &str) -> Node {
    if node.is_nil() || !is_object_literal_expression(node) {
        return Node::NIL;
    }
    let mut result = Node::NIL;
    for property in node.properties().iter() {
        if is_property_assignment(property) && get_text_of_property_name(property.name()) == name {
            result = property;
        }
    }
    result
}

fn property_value(node: Node, name: &str) -> Node {
    let property = find_property(node, name);
    if property.is_some() {
        return property.initializer();
    }
    Node::NIL
}

fn array_elements(node: Node) -> Vec<Node> {
    if node.is_some() && is_array_literal_expression(node) {
        return node.elements().to_vec();
    }
    Vec::new()
}
