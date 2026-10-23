//! Port of Go `checker/symboltracker.go`.

use crate::prelude::*;
use std::cell::Cell;
use std::rc::Weak;

// Go: checker/symboltracker.go:8 SymbolTrackerImpl
// PORT: Go `context` is a plain pointer. The context also holds this tracker,
// so here it is a `Weak` to break the `Rc` cycle. The context outlives every
// call on its tracker, so the upgrade does not fail.
pub struct SymbolTrackerImpl {
    pub context: Weak<RefCell<NodeBuilderContext>>,
    pub inner: Option<Rc<dyn SymbolTracker>>,
    // PORT: a `Cell`, because tracker methods take `&self`.
    pub disable_track_symbol: Cell<bool>,
}

// Go: checker/symboltracker.go:14 NewSymbolTrackerImpl
// PORT: returns an `Rc`, the shape that `NodeBuilderContext.tracker` holds.
pub fn new_symbol_tracker_impl(
    context: Rc<RefCell<NodeBuilderContext>>,
    mut tracker: Option<Rc<dyn SymbolTracker>>,
) -> Rc<SymbolTrackerImpl> {
    if tracker.is_some() {
        loop {
            let Some(t) = tracker.clone() else { break };
            let inner = match t.as_symbol_tracker_impl() {
                Some(t) => t.inner.clone(),
                None => break,
            };
            tracker = inner;
        }
    }

    Rc::new(SymbolTrackerImpl {
        context: Rc::downgrade(&context),
        inner: tracker,
        disable_track_symbol: Cell::new(false),
    })
}

impl SymbolTrackerImpl {
    fn context(&self) -> Rc<RefCell<NodeBuilderContext>> {
        self.context
            .upgrade()
            .expect("SymbolTrackerImpl context is gone")
    }

    // Go: checker/symboltracker.go:106 SymbolTrackerImpl.onDiagnosticReported
    fn on_diagnostic_reported(&self) {
        self.context().borrow_mut().reported_diagnostic = true;
    }
}

impl SymbolTracker for SymbolTrackerImpl {
    // Go: checker/symboltracker.go:28 SymbolTrackerImpl.TrackSymbol
    fn track_symbol(
        &self,
        c: &mut Checker,
        symbol: SymbolId,
        enclosing_declaration: Node,
        meaning: SymbolFlags,
    ) -> bool {
        if !self.disable_track_symbol.get() {
            if let Some(inner) = &self.inner {
                if inner.track_symbol(c, symbol, enclosing_declaration, meaning) {
                    self.on_diagnostic_reported();
                    return true;
                }
            }
            // Skip recording type parameters as they dont contribute to late painted statements
            if !c.sym_flags(symbol).intersects(SymbolFlags::TYPE_PARAMETER) {
                self.context()
                    .borrow_mut()
                    .tracked_symbols
                    .push(TrackedSymbolArgs {
                        symbol,
                        enclosing_declaration,
                        meaning,
                    });
            }
        }
        false
    }

    // Go: checker/symboltracker.go:42 SymbolTrackerImpl.ReportInaccessibleThisError
    fn report_inaccessible_this_error(&self, c: &mut Checker) {
        self.on_diagnostic_reported();
        let Some(inner) = &self.inner else { return };
        inner.report_inaccessible_this_error(c);
    }

    // Go: checker/symboltracker.go:50 SymbolTrackerImpl.ReportPrivateInBaseOfClassExpression
    fn report_private_in_base_of_class_expression(&self, c: &mut Checker, property_name: &str) {
        self.on_diagnostic_reported();
        let Some(inner) = &self.inner else { return };
        inner.report_private_in_base_of_class_expression(c, property_name);
    }

    // Go: checker/symboltracker.go:58 SymbolTrackerImpl.ReportInaccessibleUniqueSymbolError
    fn report_inaccessible_unique_symbol_error(&self, c: &mut Checker) {
        self.on_diagnostic_reported();
        let Some(inner) = &self.inner else { return };
        inner.report_inaccessible_unique_symbol_error(c);
    }

    // Go: checker/symboltracker.go:66 SymbolTrackerImpl.ReportCyclicStructureError
    fn report_cyclic_structure_error(&self, c: &mut Checker) {
        self.on_diagnostic_reported();
        let Some(inner) = &self.inner else { return };
        inner.report_cyclic_structure_error(c);
    }

    // Go: checker/symboltracker.go:74 SymbolTrackerImpl.ReportLikelyUnsafeImportRequiredError
    fn report_likely_unsafe_import_required_error(
        &self,
        c: &mut Checker,
        specifier: &str,
        symbol_name: &str,
    ) {
        self.on_diagnostic_reported();
        let Some(inner) = &self.inner else { return };
        inner.report_likely_unsafe_import_required_error(c, specifier, symbol_name);
    }

    // Go: checker/symboltracker.go:82 SymbolTrackerImpl.ReportTruncationError
    fn report_truncation_error(&self, c: &mut Checker) {
        self.on_diagnostic_reported();
        let Some(inner) = &self.inner else { return };
        inner.report_truncation_error(c);
    }

    // Go: checker/symboltracker.go:90 SymbolTrackerImpl.ReportNonlocalAugmentation
    fn report_nonlocal_augmentation(
        &self,
        c: &mut Checker,
        containing_file: Node,
        parent_symbol: SymbolId,
        augmenting_symbol: SymbolId,
    ) {
        self.on_diagnostic_reported();
        let Some(inner) = &self.inner else { return };
        inner.report_nonlocal_augmentation(c, containing_file, parent_symbol, augmenting_symbol);
    }

    // Go: checker/symboltracker.go:98 SymbolTrackerImpl.ReportNonSerializableProperty
    fn report_non_serializable_property(&self, c: &mut Checker, property_name: &str) {
        self.on_diagnostic_reported();
        let Some(inner) = &self.inner else { return };
        inner.report_non_serializable_property(c, property_name);
    }

    // Go: checker/symboltracker.go:110 SymbolTrackerImpl.ReportInferenceFallback
    fn report_inference_fallback(&self, c: &mut Checker, node: Node) {
        let Some(inner) = &self.inner else { return };
        inner.report_inference_fallback(c, node);
    }

    // Go: checker/symboltracker.go:117 SymbolTrackerImpl.PushErrorFallbackNode
    fn push_error_fallback_node(&self, c: &mut Checker, node: Node) {
        let Some(inner) = &self.inner else { return };
        inner.push_error_fallback_node(c, node);
    }

    // Go: checker/symboltracker.go:124 SymbolTrackerImpl.PopErrorFallbackNode
    fn pop_error_fallback_node(&self, c: &mut Checker) {
        let Some(inner) = &self.inner else { return };
        inner.pop_error_fallback_node(c);
    }

    fn as_symbol_tracker_impl(&self) -> Option<&SymbolTrackerImpl> {
        Some(self)
    }
}
