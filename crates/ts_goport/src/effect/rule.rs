//! Port of Effect-TS/tsgo `internal/rule`: the rule type and the context a
//! rule runs with.

use crate::diagnostics::Message;
use crate::effect::directives::to_category;
use crate::effect::etscore::{ResolvedEffectPluginOptions, Severity};
use crate::effect::typeparser::TypeParser;
use crate::gostd::Context;
use crate::prelude::*;

/// Go `rule.Rule`.
pub struct Rule {
    pub name: &'static str,
    pub group: &'static str,
    pub description: &'static str,
    pub default_severity: Severity,
    pub supported_effect: &'static [&'static str],
    /// The codes of the messages the rule reports.
    pub codes: &'static [i32],
    pub run: fn(&mut RuleContext<'_, '_>) -> Vec<Diagnostic>,
}

pub const UNUSED_DIRECTIVE_NAME: &str = "unusedDirective";
pub const UNKNOWN_RULE_NAME_NAME: &str = "unknownRuleName";

// Go: rule.ByName
#[must_use]
pub fn by_name(rules: &[&'static Rule], name: &str) -> Option<&'static Rule> {
    rules.iter().copied().find(|r| r.name == name)
}

// Go: rule.CodeToRuleName
#[must_use]
pub fn code_to_rule_name(rules: &[&'static Rule], code: i32) -> &'static str {
    rules
        .iter()
        .find(|r| r.codes.contains(&code))
        .map_or("", |r| r.name)
}

/// Go `rule.Context`. Go's `ctx.Checker` is `ctx.tp.checker` (or
/// `ctx.checker()`), and Go's `ctx.TypeParser` is `ctx.tp`.
pub struct RuleContext<'a, 'c> {
    pub context: &'a Context,
    pub program: &'static GoProgram,
    pub tp: &'a mut TypeParser<'c>,
    pub source_file: Node,
    pub options: &'a ResolvedEffectPluginOptions,
    pub default_severity: Severity,
}

impl RuleContext<'_, '_> {
    /// Go `ctx.Checker`.
    pub fn checker(&mut self) -> &mut Checker {
        self.tp.checker
    }

    // Go: rule.Context.GetErrorRange
    #[must_use]
    pub fn get_error_range(&self, node: Node) -> TextRange {
        get_error_range_for_node(self.source_file, node)
    }

    // Go: rule.Context.NewDiagnostic
    #[must_use]
    pub fn new_diagnostic(
        &self,
        sf: Node,
        loc: TextRange,
        message: &'static Message,
        related_information: Vec<Diagnostic>,
        args: Vec<String>,
    ) -> Diagnostic {
        new_diagnostic_from_serialized(
            sf,
            loc,
            i32::try_from(message.code()).unwrap_or(i32::MAX),
            to_category(self.default_severity),
            message.key(),
            args,
            Vec::new(),
            related_information,
            false,
            false,
            false,
        )
    }
}
