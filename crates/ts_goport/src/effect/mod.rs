//! Native Effect diagnostics: a port of Effect-TS/tsgo at
//! `@effect/tsgo@0.51.1` (`47cb1ed7704aff44cacaa0f4d2ef24de990cba0e`).
//!
//! The reference patches typescript-go in a few places (`_patches/typescript`).
//! Each patch site here names its patch. The rules run after the checker has
//! checked a source file, with the same checker, and add their diagnostics
//! to the checker's diagnostics (codes 377000 to 377999). Nothing runs unless
//! the tsconfig has an `@effect/language-service` plugin entry with
//! diagnostics enabled. A standalone API process (`tsgo --api`) runs nothing
//! unless `TSGO_EFFECT_API=1` (`rulerunner::set_api_process`).
//!
//! Read `crates/ts_goport/src/effect/PORTING.md` before editing.

use crate::gostd::Context;
use crate::prelude::*;

pub mod configcheck;
pub mod configraw;
pub mod diag;
pub mod directives;
pub mod etscore;
pub mod graph;
pub mod keybuilder;
pub mod layergraph;
pub mod pluginoptions;
pub mod rule;
pub mod rulerunner;
pub mod rules;
pub mod typeparser;

/// Effect-TS/tsgo patch 005 `RelationError`: a type relation error that the
/// relater reported, kept for Effect rules.
#[derive(Clone, Copy, Debug)]
pub struct RelationError {
    pub source: TypeId,
    pub target: TypeId,
    pub error_node: Node,
}

/// Go `rule.IsEffectCode`.
#[must_use]
pub fn is_effect_code(code: i32) -> bool {
    (377_000..378_000).contains(&code)
}

// Go: etscheckerhooks/init.go afterCheckSourceFile
/// Runs the Effect rules for `source_file` and adds their diagnostics to
/// the checker. `check_source_file` calls it once per file, after the type
/// check and the unused check (the reference calls it before the unused
/// check; see the comment there).
pub fn after_check_source_file(ctx: &Context, c: &mut Checker, source_file: Node) {
    let Some(effect_config) = c.compiler_options.effect.clone() else {
        return;
    };
    let min = rulerunner::min_visible_severity(Some(&effect_config));
    let program = c.program;
    let Ok(diagnostics) = rulerunner::run(
        ctx,
        program,
        c,
        source_file,
        Some(&effect_config),
        None,
        min,
    ) else {
        return;
    };
    for diag in diagnostics {
        c.diagnostics.add(diag);
    }
}

// Go: checker.go (patch 002) GetRelationErrors
impl Checker {
    /// The relation errors collected while checking `sf`. Checks the file
    /// first when it is not checked yet.
    pub fn get_relation_errors(&mut self, sf: Node) -> Vec<RelationError> {
        if !self.source_file_links.get(sf).type_checked {
            // Go passes `c.ctx`, which can be nil; the file is checked either way.
            let ctx = self
                .ctx
                .clone()
                .unwrap_or_else(crate::gostd::context::background);
            self.get_diagnostics(&ctx, sf, false);
        }
        self.effect_relation_errors
            .get(&sf)
            .cloned()
            .unwrap_or_default()
    }
}

// Go: etsexecutehooks/init.go FilterDiagnosticsForExitCode
/// The diagnostics that decide the tsc exit code: Effect diagnostics whose
/// category the plugin options ignore are left out. Non-Effect diagnostics
/// always count.
/// Without Effect options it borrows `diags`, so projects without the plugin
/// pay nothing.
#[must_use]
pub fn filter_diagnostics_for_exit_code<'a>(
    opts: Option<&etscore::EffectPluginOptions>,
    diags: &'a [Diagnostic],
) -> std::borrow::Cow<'a, [Diagnostic]> {
    let Some(opts) = opts else {
        return std::borrow::Cow::Borrowed(diags);
    };
    diags
        .iter()
        .filter(|d| !(is_effect_code(d.code) && should_ignore_for_exit_code(opts, d.category)))
        .cloned()
        .collect::<Vec<_>>()
        .into()
}

fn should_ignore_for_exit_code(
    opts: &etscore::EffectPluginOptions,
    category: crate::diagnostics::Category,
) -> bool {
    use crate::diagnostics::Category;
    match category {
        Category::Suggestion | Category::Message => opts.ignore_effect_suggestions_in_tsc_exit_code,
        Category::Warning => opts.ignore_effect_warnings_in_tsc_exit_code,
        Category::Error => opts.ignore_effect_errors_in_tsc_exit_code,
        _ => false,
    }
}
