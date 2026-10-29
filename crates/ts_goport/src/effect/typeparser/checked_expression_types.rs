// Go: internal/typeparser/checked_expression_types.go
// PORT: Effect-TS/tsgo#860 (first released in @effect/tsgo@0.53.2), ported
// ahead of the 0.51.1 pin. See effect/PORTING.md.

use crate::effect::typeparser::checker_integration::{
    get_resolved_type_from_type_node, is_checking_source_file, is_source_file_type_checked,
};
use crate::effect::typeparser::*;
use crate::prelude::*;

/// RecordCheckedExpressionType stores the type the checker computed for an
/// expression while checking a source file. Only final evaluations count: plain
/// Normal mode, or Contextual alone when a call argument is checked against its
/// parameter. Inference passes and type-only queries are tentative. The checker
/// may evaluate a node more than once, and the last final evaluation is the one
/// its diagnostics are based on, so later writes win. Rule runs pause recording,
/// and evaluations outside a source-file check are ignored, so only the check
/// itself writes the store.
pub fn record_checked_expression_type(
    c: &mut Checker,
    node: Node,
    check_mode: CheckMode,
    t: TypeId,
) {
    if !check_mode.without(CheckMode::CONTEXTUAL).is_empty() || !is_checking_source_file(c) {
        return;
    }
    let links = effect_links_of(c);
    if links.checked_expression_types_paused {
        return;
    }
    *links.checked_expression_type.get(node) = CachedId::new(t.0);
}

/// PauseCheckedExpressionTypes stops recording for the checker until the
/// returned guard is dropped (Go returns the resume function, which the caller
/// defers).
pub fn pause_checked_expression_types(c: &mut Checker) -> CheckedExpressionTypesPause {
    effect_links_of(c).checked_expression_types_paused = true;
    CheckedExpressionTypesPause { _private: () }
}

/// The resume function of `pause_checked_expression_types`. PORT: the guard
/// holds no borrow of the checker, so the caller resumes with `resume`.
#[must_use]
pub struct CheckedExpressionTypesPause {
    _private: (),
}

impl CheckedExpressionTypesPause {
    pub fn resume(self, c: &mut Checker) {
        effect_links_of(c).checked_expression_types_paused = false;
    }
}

impl TypeParser<'_> {
    /// checkedExpressionType returns the type the file's check computed for an
    /// expression, without evaluating anything, or nil when the check never
    /// evaluated it.
    pub fn checked_expression_type(&mut self, mut node: Node) -> TypeId {
        if !is_expression_node(node) || is_part_of_type_node(node) {
            return TypeId::NIL;
        }
        if is_right_side_of_qualified_name_or_property_access(node) {
            node = node.parent();
        }
        if !is_source_file_type_checked(self.checker, get_source_file_of_node(node)) {
            return TypeId::NIL;
        }
        let mut t = TypeId::NIL;
        if let Some(recorded) = self.links().checked_expression_type.try_get(node) {
            t = TypeId(recorded.get());
        }
        if t.is_nil() {
            // Initializers and return expressions first evaluated while recording
            // was paused are only in the checker's own cache.
            t = get_resolved_type_from_type_node(self.checker, node);
        }
        if t.is_nil() {
            return TypeId::NIL;
        }
        self.checker.get_regular_type_of_literal_type(t)
    }
}

// Go: internal/typeparser/type_parser.go effectLinksOf
pub fn effect_links_of(c: &mut Checker) -> &mut EffectLinks {
    c.effect_links.get_or_insert_with(Box::default)
}
