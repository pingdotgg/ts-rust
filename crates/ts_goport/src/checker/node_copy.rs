//! Port of checker/nodecopy.go: reuse of existing type nodes inside the node
//! builder, with a recovery boundary that discards the copy on error.
//!
//! Relater pattern (printer plan contract item 8): NodeBuilderImpl methods are
//! `impl Checker` methods that take `b: &Rc<RefCell<NodeBuilderImpl>>`. No
//! `RefCell` borrow is held across a Checker call.
//!
//! PORT: Go builds the existing-node visitor from closures over `b`, `bound`
//! and `visitor`. Here the captured state is `ExistingNodeTreeState`, the
//! `ctx` of an `ast::NodeVisitor` (see ast/visitor.rs). Go's closures become
//! methods on that visitor type (`ExistingNodeTreeVisitor`), and the visit
//! callback and hooks call them.

use crate::prelude::*;
use crate::printer::{EmitContext, EmitFlags, SymbolAccessibility};

// PORT: private accessors for NodeBuilderImpl fields (owned by U11). Clone
// the handle out, then drop the borrow before any Checker call.
fn nb_ctx(b: &Rc<RefCell<NodeBuilderImpl>>) -> Rc<RefCell<NodeBuilderContext>> {
    b.borrow().ctx.clone()
}

fn nb_e(b: &Rc<RefCell<NodeBuilderImpl>>) -> Rc<EmitContext> {
    b.borrow().e.clone()
}

fn nb_tracker(b: &Rc<RefCell<NodeBuilderImpl>>) -> Rc<dyn SymbolTracker> {
    let ctx = nb_ctx(b);
    let tracker = ctx.borrow().tracker.clone();
    tracker
}

impl Checker {
    // Go: checker/nodecopy.go:12 reuseNode
    pub fn reuse_node(&mut self, b: &Rc<RefCell<NodeBuilderImpl>>, node: Node) -> Node {
        if node.is_nil() {
            return node;
        }

        self.try_reuse_existing_node_helper(b, node)
    }

    // Go: checker/nodecopy.go:20 tryJSTypeNodeToTypeNode
    pub fn try_js_type_node_to_type_node(
        &mut self,
        b: &Rc<RefCell<NodeBuilderImpl>>,
        node: Node,
    ) -> Node {
        self.reuse_node(b, node)
    }

    // Go: checker/nodecopy.go:24 reuseName
    pub fn reuse_name(
        &mut self,
        b: &Rc<RefCell<NodeBuilderImpl>>,
        node: Node,
        is_method: bool,
    ) -> Node {
        let res = self.reuse_node(b, node);
        if res.is_nil() {
            return res;
        }

        let (text, ok) = try_get_text_of_property_name(res);
        if !ok {
            return res;
        }

        let kind = classify_property_name(&text, is_string_literal(res), is_method);
        if is_identifier(res) && kind == PropertyNameNodeKind::IDENTIFIER {
            return res;
        }
        if is_string_literal(res) && kind == PropertyNameNodeKind::STRING_LITERAL {
            return res;
        }

        let renamed = match kind {
            PropertyNameNodeKind::IDENTIFIER => self.nb_new_identifier(b, &text, SymbolId::NIL),
            PropertyNameNodeKind::STRING_LITERAL => {
                nb_e(b).factory().new_string_literal(text, TokenFlags::NONE)
            }
            _ => return res,
        };
        nb_e(b).set_original(renamed, res);
        self.set_text_range(b, renamed, res)
    }

    // Go: checker/nodecopy.go:56 reuseTypeNode
    pub fn reuse_type_node(&mut self, b: &Rc<RefCell<NodeBuilderImpl>>, node: Node) -> Node {
        if node.is_nil() {
            return node;
        }
        let r = self.reuse_node(b, node);
        if r.is_some() {
            // After successful reuse during hover, probe the reused AST for expandable
            // type references so canIncreaseExpansionDepth is set even though
            // typeToTypeNode (and shouldExpandType) were never called.
            let probe = {
                let ctx = nb_ctx(b);
                let c = ctx.borrow();
                c.max_expansion_depth >= 0 && !c.can_increase_expansion_depth
            };
            if probe {
                self.walk_node_for_expandability(b, node);
            }
            return r;
        }
        nb_tracker(b).report_inference_fallback(self, node);
        let t = self.nb_get_type_from_type_node(b, node, false);
        self.type_to_type_node(b, t)
    }

    // Go: checker/nodecopy.go:78 walkNodeForExpandability
    //
    // walkNodeForExpandability walks a reused AST node tree, calling checkTypeExpandability
    // on each type reference, type predicate, or import type node.
    // Short-circuits once canIncreaseExpansionDepth is set.
    pub fn walk_node_for_expandability(&mut self, b: &Rc<RefCell<NodeBuilderImpl>>, node: Node) {
        let ctx = nb_ctx(b);
        if ctx.borrow().can_increase_expansion_depth || node.is_nil() {
            return;
        }
        // Check these explicitly so we look into type arguments wehther or not they are in the tree or not.
        if is_type_reference_node(node)
            || is_expression_with_type_arguments(node)
            || is_type_predicate_node(node)
            || is_import_type_node(node)
        {
            let t = self.nb_get_type_from_type_node(b, node, false);
            if t.is_some() {
                self.check_type_expandability(b, t);
                if ctx.borrow().can_increase_expansion_depth {
                    return;
                }
            }
        }
        node.for_each_child(|child| {
            self.walk_node_for_expandability(b, child);
            ctx.borrow().can_increase_expansion_depth
        });
    }
}

// Go: checker/nodecopy.go:98 recoveryBoundary
pub struct RecoveryBoundary {
    pub ctx: Rc<RefCell<NodeBuilderContext>>,
    pub had_error: bool,
    // PORT: Go `[]func()`. The deferred reports call the wrapped tracker,
    // which takes the checker.
    pub deferred_reports: Vec<Box<dyn FnOnce(&mut Checker)>>,
    pub old_tracker: Rc<dyn SymbolTracker>,
    pub old_tracked_symbols: Vec<TrackedSymbolArgs>,
    pub tracked_symbols: Vec<TrackedSymbolArgs>,
    pub old_encountered_error: bool,
    pub old_approximate_length: i32,
}

impl RecoveryBoundary {
    // Go: checker/nodecopy.go:109 recoveryBoundary.markError
    pub fn mark_error(&mut self, f: Option<Box<dyn FnOnce(&mut Checker)>>) {
        self.had_error = true;
        if let Some(f) = f {
            self.deferred_reports.push(f);
        }
    }

    // Go: checker/nodecopy.go:122 recoveryBoundary.startRecoveryScope
    pub fn start_recovery_scope(&self) -> OriginalRecoveryScopeState {
        let tracked_symbols_top = self.ctx.borrow().tracked_symbols.len();
        let unreported_errors_top = self.deferred_reports.len();
        OriginalRecoveryScopeState {
            tracked_symbols_top,
            unreported_errors_top,
            had_error: self.had_error,
        }
    }

    // Go: checker/nodecopy.go:128 recoveryBoundary.endRecoveryScope
    pub fn end_recovery_scope(&mut self, state: OriginalRecoveryScopeState) {
        self.had_error = state.had_error;
        self.ctx
            .borrow_mut()
            .tracked_symbols
            .truncate(state.tracked_symbols_top);
        self.deferred_reports.truncate(state.unreported_errors_top);
    }
}

// Go: checker/nodecopy.go:116 originalRecoveryScopeState
#[derive(Clone, Copy, Debug)]
pub struct OriginalRecoveryScopeState {
    pub tracked_symbols_top: usize,
    pub unreported_errors_top: usize,
    pub had_error: bool,
}

// Go: checker/nodecopy.go:134 wrappingTracker
pub struct WrappingTracker {
    pub wrapped: Rc<dyn SymbolTracker>,
    pub bound: Rc<RefCell<RecoveryBoundary>>,
}

impl SymbolTracker for WrappingTracker {
    // Go: checker/nodecopy.go:139 wrappingTracker.PopErrorFallbackNode
    fn pop_error_fallback_node(&self, c: &mut Checker) {
        self.wrapped.pop_error_fallback_node(c);
    }

    // Go: checker/nodecopy.go:143 wrappingTracker.PushErrorFallbackNode
    fn push_error_fallback_node(&self, c: &mut Checker, node: Node) {
        self.wrapped.push_error_fallback_node(c, node);
    }

    // Go: checker/nodecopy.go:147 wrappingTracker.ReportCyclicStructureError
    fn report_cyclic_structure_error(&self, _c: &mut Checker) {
        let w = self.wrapped.clone();
        self.bound
            .borrow_mut()
            .mark_error(Some(Box::new(move |c: &mut Checker| {
                w.report_cyclic_structure_error(c)
            })));
    }

    // Go: checker/nodecopy.go:151 wrappingTracker.ReportInaccessibleThisError
    fn report_inaccessible_this_error(&self, _c: &mut Checker) {
        let w = self.wrapped.clone();
        self.bound
            .borrow_mut()
            .mark_error(Some(Box::new(move |c: &mut Checker| {
                w.report_inaccessible_this_error(c)
            })));
    }

    // Go: checker/nodecopy.go:155 wrappingTracker.ReportInaccessibleUniqueSymbolError
    fn report_inaccessible_unique_symbol_error(&self, _c: &mut Checker) {
        let w = self.wrapped.clone();
        self.bound
            .borrow_mut()
            .mark_error(Some(Box::new(move |c: &mut Checker| {
                w.report_inaccessible_unique_symbol_error(c)
            })));
    }

    // Go: checker/nodecopy.go:159 wrappingTracker.ReportInferenceFallback
    fn report_inference_fallback(&self, c: &mut Checker, node: Node) {
        self.wrapped.report_inference_fallback(c, node); // Should this also be deferred?
    }

    // Go: checker/nodecopy.go:163 wrappingTracker.ReportLikelyUnsafeImportRequiredError
    fn report_likely_unsafe_import_required_error(
        &self,
        _c: &mut Checker,
        specifier: &str,
        symbol_name: &str,
    ) {
        let w = self.wrapped.clone();
        let specifier = specifier.to_string();
        let symbol_name = symbol_name.to_string();
        self.bound
            .borrow_mut()
            .mark_error(Some(Box::new(move |c: &mut Checker| {
                w.report_likely_unsafe_import_required_error(c, &specifier, &symbol_name)
            })));
    }

    // Go: checker/nodecopy.go:167 wrappingTracker.ReportNonSerializableProperty
    fn report_non_serializable_property(&self, _c: &mut Checker, property_name: &str) {
        let w = self.wrapped.clone();
        let property_name = property_name.to_string();
        self.bound
            .borrow_mut()
            .mark_error(Some(Box::new(move |c: &mut Checker| {
                w.report_non_serializable_property(c, &property_name)
            })));
    }

    // Go: checker/nodecopy.go:171 wrappingTracker.ReportNonlocalAugmentation
    fn report_nonlocal_augmentation(
        &self,
        c: &mut Checker,
        containing_file: Node,
        parent_symbol: SymbolId,
        augmenting_symbol: SymbolId,
    ) {
        self.wrapped.report_nonlocal_augmentation(
            c,
            containing_file,
            parent_symbol,
            augmenting_symbol,
        ); // Should this also be deferred?
    }

    // Go: checker/nodecopy.go:175 wrappingTracker.ReportPrivateInBaseOfClassExpression
    fn report_private_in_base_of_class_expression(&self, _c: &mut Checker, property_name: &str) {
        let w = self.wrapped.clone();
        let property_name = property_name.to_string();
        self.bound
            .borrow_mut()
            .mark_error(Some(Box::new(move |c: &mut Checker| {
                w.report_private_in_base_of_class_expression(c, &property_name)
            })));
    }

    // Go: checker/nodecopy.go:179 wrappingTracker.ReportTruncationError
    fn report_truncation_error(&self, c: &mut Checker) {
        self.wrapped.report_truncation_error(c); // Should this also be deferred?
    }

    // Go: checker/nodecopy.go:183 wrappingTracker.TrackSymbol
    fn track_symbol(
        &self,
        _c: &mut Checker,
        symbol: SymbolId,
        enclosing_declaration: Node,
        meaning: SymbolFlags,
    ) -> bool {
        self.bound
            .borrow_mut()
            .tracked_symbols
            .push(TrackedSymbolArgs {
                symbol,
                enclosing_declaration,
                meaning,
            });
        false
    }
}

// Go: checker/nodecopy.go:188 newWrappingTracker
pub fn new_wrapping_tracker(
    inner: Rc<dyn SymbolTracker>,
    bound: Rc<RefCell<RecoveryBoundary>>,
) -> Rc<WrappingTracker> {
    Rc::new(WrappingTracker {
        wrapped: inner,
        bound,
    })
}

impl Checker {
    // Go: checker/nodecopy.go:195 createRecoveryBoundary
    pub fn create_recovery_boundary(
        &mut self,
        b: &Rc<RefCell<NodeBuilderImpl>>,
    ) -> Rc<RefCell<RecoveryBoundary>> {
        self.check_not_canceled();
        let ctx = nb_ctx(b);
        let bound = {
            let c = ctx.borrow();
            Rc::new(RefCell::new(RecoveryBoundary {
                ctx: ctx.clone(),
                had_error: false,
                deferred_reports: Vec::new(),
                old_tracker: c.tracker.clone(),
                old_tracked_symbols: c.tracked_symbols.clone(),
                tracked_symbols: Vec::new(),
                old_encountered_error: c.encountered_error,
                old_approximate_length: c.approximate_length,
            }))
        };
        let old_tracker = ctx.borrow().tracker.clone();
        let wrapping: Rc<dyn SymbolTracker> = new_wrapping_tracker(old_tracker, bound.clone());
        let new_tracker: Rc<dyn SymbolTracker> =
            new_symbol_tracker_impl(ctx.clone(), Some(wrapping));
        {
            let mut c = ctx.borrow_mut();
            c.tracker = new_tracker;
            // PORT: Go sets nil; the saved copy is in old_tracked_symbols.
            c.tracked_symbols = Vec::new();
        }
        bound
    }

    // Go: checker/nodecopy.go:204 finalizeBoundary
    pub fn finalize_boundary(
        &mut self,
        b: &Rc<RefCell<NodeBuilderImpl>>,
        bound: &Rc<RefCell<RecoveryBoundary>>,
    ) -> bool {
        let ctx = nb_ctx(b);
        let (deferred_reports, had_error, tracked_symbols) = {
            let mut bd = bound.borrow_mut();
            let mut c = ctx.borrow_mut();
            c.tracker = bd.old_tracker.clone();
            c.tracked_symbols = bd.old_tracked_symbols.clone();
            c.encountered_error = bd.old_encountered_error;
            c.approximate_length = bd.old_approximate_length;
            // PORT: the reports are FnOnce, so they move out of the boundary.
            (
                std::mem::take(&mut bd.deferred_reports),
                bd.had_error,
                bd.tracked_symbols.clone(),
            )
        };

        for f in deferred_reports {
            f(self);
        }
        if had_error {
            return false;
        }
        for a in &tracked_symbols {
            let tracker = nb_tracker(b);
            tracker.track_symbol(self, a.symbol, a.enclosing_declaration, a.meaning);
        }
        true
    }

    // Go: checker/nodecopy.go:222 tryReuseExistingNodeHelper
    pub fn try_reuse_existing_node_helper(
        &mut self,
        b: &Rc<RefCell<NodeBuilderImpl>>,
        existing: Node,
    ) -> Node {
        let bound = self.create_recovery_boundary(b);
        // !!! TODO: Cache visitor and just reset bound+host builder? We try this for a *lot* of nodes.
        // PORT: the visitor borrows the emit context for its factory (Go `b.f`).
        let e = nb_e(b);
        let transformed = {
            let mut v = get_existing_node_tree_visitor(self, b, &e, bound.clone());
            v.visit_node(existing)
        };
        if !self.finalize_boundary(b, &bound) {
            return Node::NIL;
        }
        nb_ctx(b).borrow_mut().approximate_length += go_node_text_len(existing);
        transformed
    }

    // Go: checker/nodecopy.go:234 getModuleSpecifierOverride
    pub fn get_module_specifier_override(
        &mut self,
        b: &Rc<RefCell<NodeBuilderImpl>>,
        parent: Node,
        lit: Node,
    ) -> String {
        let ctx = nb_ctx(b);
        let (enclosing_file, enclosing_declaration) = {
            let c = ctx.borrow();
            (c.enclosing_file, c.enclosing_declaration)
        };
        if enclosing_file != get_source_file_of_node(lit) {
            let mut mode = ResolutionMode::NONE;
            if parent.attributes().is_some() {
                (mode, _) = parent.attributes().get_resolution_mode_override(None);
            }
            let mut name = lit.text().to_string();
            let original_name = name.clone();
            let node_symbol = self.try_get_resolved_symbol_from_type_node(b, parent);
            let mut meaning = SymbolFlags::TYPE;
            if parent.is_type_of() {
                meaning = SymbolFlags::VALUE;
            }
            let mut parent_symbol = SymbolId::NIL;
            if node_symbol.is_some()
                && self
                    .is_symbol_accessible(node_symbol, enclosing_declaration, meaning, false)
                    .accessibility
                    == SymbolAccessibility::ACCESSIBLE
            {
                parent_symbol = self.lookup_symbol_chain(b, node_symbol, meaning, true)[0];
            }
            if parent_symbol.is_some() && self.is_external_module_symbol(parent_symbol) {
                name = self
                    .get_specifier_for_module_symbol(b, parent_symbol, mode)
                    .specifier;
            } else {
                let target_file = self.get_external_module_file_from_declaration(parent);
                if target_file.is_some() {
                    name = self
                        .get_specifier_for_module_symbol(b, target_file.symbol(), mode)
                        .specifier;
                }
            }
            if !name.is_empty() && name.contains("/node_modules/") {
                ctx.borrow_mut().encountered_error = true;
                nb_tracker(b).report_likely_unsafe_import_required_error(self, &name, "");
            }
            if name != original_name {
                return name;
            }
        }
        String::new()
    }

    // Go: checker/nodecopy.go:270 rewriteModuleSpecifier
    pub fn rewrite_module_specifier(
        &mut self,
        b: &Rc<RefCell<NodeBuilderImpl>>,
        parent: Node,
        lit: Node,
    ) -> Node {
        let new_name = self.get_module_specifier_override(b, parent, lit);
        if new_name.is_empty() {
            return lit;
        }
        let e = nb_e(b);
        let res = e.factory().new_string_literal(new_name, TokenFlags::NONE);
        e.set_original(res, lit);
        res
    }

    // Go: checker/nodecopy.go:280 getEnclosingDeclarationIgnoringFakeScope
    pub fn get_enclosing_declaration_ignoring_fake_scope(
        &mut self,
        b: &Rc<RefCell<NodeBuilderImpl>>,
    ) -> Node {
        let mut enc = nb_ctx(b).borrow().enclosing_declaration;
        while enc.is_some()
            && b.borrow_mut()
                .links
                .get(enc)
                .fake_scope_for_signature_declaration
                .is_some()
        {
            enc = enc.parent();
        }
        enc
    }
}

// Go: checker/emitresolver.go:311 getMeaningOfEntityNameReference
// PORT: no unit owns emitresolver.go yet, so the free function is ported here
// as a private helper. Move it when emitresolver.go lands.
fn get_meaning_of_entity_name_reference(entity_name: Node) -> SymbolFlags {
    let parent = entity_name.parent();
    // get symbol of the first identifier of the entityName
    if parent.kind() == SyntaxKind::TypeQuery
        || parent.kind() == SyntaxKind::ExpressionWithTypeArguments && !is_part_of_type_node(parent)
        || parent.kind() == SyntaxKind::ComputedPropertyName
        || parent.kind() == SyntaxKind::TypePredicate && parent.parameter_name() == entity_name
        || parent.kind() == SyntaxKind::BinaryExpression
    {
        // Typeof value
        return SymbolFlags::VALUE | SymbolFlags::EXPORT_VALUE;
    }
    if entity_name.kind() == SyntaxKind::QualifiedName
        || entity_name.kind() == SyntaxKind::PropertyAccessExpression
        || parent.kind() == SyntaxKind::ImportEqualsDeclaration
        || (parent.kind() == SyntaxKind::QualifiedName && parent.left() == entity_name)
        || (parent.kind() == SyntaxKind::PropertyAccessExpression
            && parent.expression() == entity_name)
        || (parent.kind() == SyntaxKind::ElementAccessExpression
            && parent.expression() == entity_name)
    {
        // Left identifier from type reference or TypeAlias
        // Entity name of the import declaration
        return SymbolFlags::NAMESPACE;
    }
    // Type Reference or TypeAlias entity = Identifier
    SymbolFlags::TYPE
}

// Go: checker/nodecopy.go:288 getExistingNodeTreeVisitor
// TODO: wrap all these closures into methods on an object so we can guarantee we reuse the same memory on each invocation by reusing/resetting the object
// instead of re-closing-over all of these each time we need a visitor. In theory the compiler could handle this, but in practice closure inlining hasn't been reliable
// PORT: Go's closures are the methods of `ExistingNodeTreeVisitor` below.
// Their captured state is the visitor's `ctx`, and `nonLocalNode` starts true.
// `e` is Go `b.e`; the caller keeps it alive so the visitor can borrow its
// factory (Go `b.f`).
pub fn get_existing_node_tree_visitor<'a>(
    c: &'a mut Checker,
    b: &'a Rc<RefCell<NodeBuilderImpl>>,
    e: &'a EmitContext,
    bound: Rc<RefCell<RecoveryBoundary>>,
) -> ExistingNodeTreeVisitor<'a> {
    let hooks = NodeVisitorHooks {
        visit_nodes: Some(Rc::new(
            |nodes: NodeList, v: &mut ExistingNodeTreeVisitor<'a>| v.hook_visit_nodes(nodes),
        )),
        visit_node: Some(Rc::new(
            |node: Node, v: &mut ExistingNodeTreeVisitor<'a>| v.hook_visit_node(node),
        )),
        ..NodeVisitorHooks::default()
    };
    new_node_visitor(
        |node: Node, v: &mut ExistingNodeTreeVisitor<'a>| v.existing_node_tree_visit(node),
        Some(e.factory().as_node_factory()),
        hooks,
        ExistingNodeTreeState {
            c,
            b,
            bound,
            e,
            non_local_node: true,
        },
    )
}

/// The state that Go's getExistingNodeTreeVisitor closures capture.
pub struct ExistingNodeTreeState<'a> {
    c: &'a mut Checker,
    b: &'a Rc<RefCell<NodeBuilderImpl>>,
    bound: Rc<RefCell<RecoveryBoundary>>,
    e: &'a EmitContext,
    non_local_node: bool,
}

/// Go's `visitor` inside getExistingNodeTreeVisitor.
pub type ExistingNodeTreeVisitor<'a> = NodeVisitor<'a, ExistingNodeTreeState<'a>>;

/// The state that Go `attachSymbolToLeftmostIdentifier`'s inner visitor captures.
struct AttachSymbolState<'a> {
    c: &'a mut Checker,
    b: &'a Rc<RefCell<NodeBuilderImpl>>,
    e: &'a EmitContext,
    leftmost: Node,
    sym: SymbolId,
}

/// Go `vis` inside attachSymbolToLeftmostIdentifier (no hooks).
type AttachSymbolVisitor<'a> = NodeVisitor<'a, AttachSymbolState<'a>>;

// Go: checker/nodecopy.go:295 visitorFunc (inside attachSymbolToLeftmostIdentifier)
fn attach_symbol_visitor_func(node: Node, v: &mut AttachSymbolVisitor<'_>) -> Node {
    let st = &mut v.ctx;
    if node == st.leftmost {
        let mut name = Node::NIL;
        if st.sym.is_some() {
            let type_ = st.c.get_declared_type_of_symbol(st.sym);
            if st
                .c
                .sym(st.sym)
                .flags
                .intersects(SymbolFlags::TYPE_PARAMETER)
            {
                name = st.c.type_parameter_to_name(st.b, type_);
            }
        }
        if name.is_nil() {
            name = st.c.nb_new_identifier(st.b, node.text(), st.sym);
        }
        name = st.c.set_text_range(st.b, name, node);
        st.e.add_emit_flags(name, EmitFlags::NO_ASCII_ESCAPING);
        return name;
    }
    let visited = node.visit_each_child(v);
    let st = &mut v.ctx;
    st.c.set_text_range(st.b, visited, node)
}

// Go: checker/nodecopy.go:293 attachSymbolToLeftmostIdentifier
// PORT: builds Go `vis` over a reborrow of the checker. See the method below.
fn attach_symbol_to_leftmost_identifier_with<'x>(
    c: &'x mut Checker,
    b: &'x Rc<RefCell<NodeBuilderImpl>>,
    e: &'x EmitContext,
    leftmost: Node,
    node: Node,
    sym: SymbolId,
) -> Node {
    let mut vis = new_node_visitor(
        |node: Node, v: &mut AttachSymbolVisitor<'x>| attach_symbol_visitor_func(node, v),
        Some(e.factory().as_node_factory()),
        NodeVisitorHooks::default(),
        AttachSymbolState {
            c,
            b,
            e,
            leftmost,
            sym,
        },
    );
    attach_symbol_visitor_func(node, &mut vis)
}

impl<'a> ExistingNodeTreeVisitor<'a> {
    // Go: checker/nodecopy.go:293 attachSymbolToLeftmostIdentifier
    // note: also handles renaming type parameters renamed within the current context
    fn attach_symbol_to_leftmost_identifier(
        &mut self,
        leftmost: Node,
        node: Node,
        sym: SymbolId,
    ) -> Node {
        let (b, e) = (self.ctx.b, self.ctx.e);
        attach_symbol_to_leftmost_identifier_with(&mut *self.ctx.c, b, e, leftmost, node, sym)
    }

    // Go: checker/nodecopy.go:317 trackExistingEntityName
    fn track_existing_entity_name(
        &mut self,
        node: Node,
        override_enclosing: Node,
    ) -> (bool, Node, SymbolId) {
        let b = self.ctx.b;
        let ctx = nb_ctx(b);
        let e = self.ctx.e;
        let f: &NodeFactory = e.factory();
        let ctx_enclosing = ctx.borrow().enclosing_declaration;
        let mut enclosing_declaration = ctx_enclosing;
        if override_enclosing.is_some() {
            enclosing_declaration = override_enclosing;
        }
        let mut introduces_error = false;
        let leftmost = get_first_identifier(node);
        if is_in_js_file(node)
            && (is_exports_identifier(leftmost)
                || is_module_exports_access_expression(leftmost.parent())
                || (is_qualified_name(leftmost.parent())
                    && is_module_identifier(leftmost.parent().left())
                    && is_exports_identifier(leftmost.parent().right())))
        {
            introduces_error = true;
            let cloned = f.deep_clone_node(node);
            return (
                introduces_error,
                self.ctx.c.set_text_range(b, cloned, node),
                SymbolId::NIL,
            );
        }
        let meaning = get_meaning_of_entity_name_reference(node);
        let mut sym;
        if is_this_identifier(leftmost) {
            // `this` isn't a bindable identifier - skip resolution, find a relevant `this` symbol directly and avoid exhaustive scope traversal
            let container = self.ctx.c.get_this_container(leftmost, false, false);
            sym = self.ctx.c.get_symbol_of_declaration(container);
            if self
                .ctx
                .c
                .is_symbol_accessible(sym, leftmost, meaning, false)
                .accessibility
                != SymbolAccessibility::ACCESSIBLE
            {
                introduces_error = true;
                nb_tracker(b).report_inaccessible_this_error(self.ctx.c);
            }
            return (
                introduces_error,
                self.attach_symbol_to_leftmost_identifier(leftmost, node, sym),
                SymbolId::NIL,
            );
        }
        sym = self
            .ctx
            .c
            .resolve_entity_name(leftmost, meaning, true, true, Node::NIL);
        if ctx_enclosing.is_some()
            && !(sym.is_some()
                && self
                    .ctx
                    .c
                    .sym(sym)
                    .flags
                    .intersects(SymbolFlags::TYPE_PARAMETER))
        {
            sym = self
                .ctx
                .c
                .get_export_symbol_of_value_symbol_if_exported(sym);
            // Some declarations may be transplanted to a new location.
            // When this happens we need to make sure that the name has the same meaning at both locations
            // We also check for the unknownSymbol because when we create a fake scope some parameters may actually not be usable
            // either because they are the expanded rest parameter,
            // or because they are the newly added parameters from the tuple, which might have different meanings in the original context
            let sym_at_location =
                self.ctx
                    .c
                    .resolve_entity_name(leftmost, meaning, true, true, ctx_enclosing);
            let unknown_symbol = self.ctx.c.unknown_symbol;
            let mismatch = if sym_at_location.is_some() && sym.is_some() {
                let exported = self
                    .ctx
                    .c
                    .get_export_symbol_of_value_symbol_if_exported(sym_at_location);
                self.ctx
                    .c
                    .get_symbol_if_same_reference(exported, sym)
                    .is_nil()
            } else {
                false
            };
            if
            // Check for unusable parameters symbols
            sym_at_location == unknown_symbol
                // If the symbol is not found, but was not found in the original scope either we probably have an error, don't reuse the node
                || (sym_at_location.is_nil() && sym.is_some())
                // If the symbol is found both in declaration scope and in current scope then it should point to the same reference
                || mismatch
            {
                // In isolated declaration we will not do rest parameter expansion so there is no need to report on these.
                if sym_at_location != unknown_symbol {
                    nb_tracker(b).report_inference_fallback(self.ctx.c, node);
                }
                introduces_error = true;
                let cloned = f.deep_clone_node(node);
                return (
                    introduces_error,
                    self.ctx.c.set_text_range(b, cloned, node),
                    sym,
                );
            } else {
                sym = sym_at_location;
            }
        }

        if sym.is_some() {
            // If a parameter is resolvable in the current context it is also visible, so no need to go to symbol accesibility
            let (sym_flags, value_declaration) = {
                let s = self.ctx.c.sym(sym);
                (s.flags, s.value_declaration)
            };
            if sym_flags.intersects(SymbolFlags::FUNCTION_SCOPED_VARIABLE)
                && value_declaration.is_some()
            {
                if is_part_of_parameter_declaration(value_declaration)
                    || is_js_doc_parameter_tag(value_declaration)
                {
                    return (
                        introduces_error,
                        self.attach_symbol_to_leftmost_identifier(leftmost, node, sym),
                        SymbolId::NIL,
                    );
                }
            }
            if !sym_flags.intersects(SymbolFlags::TYPE_PARAMETER) /* Type parameters are visible in the current context if they are are resolvable */
                && !is_declaration_name(node)
                && self.ctx.c.is_symbol_accessible(sym, enclosing_declaration, meaning, false).accessibility
                    != SymbolAccessibility::ACCESSIBLE
            {
                nb_tracker(b).report_inference_fallback(self.ctx.c, node);
                introduces_error = true;
            } else {
                nb_tracker(b).track_symbol(self.ctx.c, sym, enclosing_declaration, meaning);
            }
            return (
                introduces_error,
                self.attach_symbol_to_leftmost_identifier(leftmost, node, sym),
                SymbolId::NIL,
            );
        }
        let cloned = f.deep_clone_node(node);
        (
            introduces_error,
            self.ctx.c.set_text_range(b, cloned, node),
            SymbolId::NIL,
        )
    }

    // Go: checker/nodecopy.go:385 tryVisitIndexedAccess
    fn try_visit_indexed_access(&mut self, node: Node) -> Node {
        let result_object_type = self.try_visit_simple_type_node(node.object_type());
        if result_object_type.is_nil() {
            return Node::NIL;
        }
        let e = self.ctx.e;
        let index_type = self.visit_node(node.index_type());
        let updated =
            e.factory()
                .update_indexed_access_type_node(node, result_object_type, index_type);
        self.ctx.c.set_text_range(self.ctx.b, updated, node)
    }

    // Go: checker/nodecopy.go:392 tryVisitKeyOf
    fn try_visit_key_of(&mut self, node: Node) -> Node {
        let t = self.try_visit_simple_type_node(node.type_());
        if t.is_nil() {
            return Node::NIL;
        }
        let e = self.ctx.e;
        let updated = e
            .factory()
            .update_type_operator_node(node, node.operator(), t);
        self.ctx.c.set_text_range(self.ctx.b, updated, node)
    }

    // Go: checker/nodecopy.go:400 tryVisitTypeQuery
    fn try_visit_type_query(&mut self, node: Node) -> Node {
        let (introduces_error, expr_name, _) =
            self.track_existing_entity_name(node.expr_name(), Node::NIL);
        if !introduces_error {
            let e = self.ctx.e;
            let type_arguments = self.visit_nodes(node.type_argument_list());
            let updated = e
                .factory()
                .update_type_query_node(node, expr_name, type_arguments);
            return self.ctx.c.set_text_range(self.ctx.b, updated, node);
        }

        let type_arguments = self.visit_nodes(node.type_argument_list());
        let serialized_name =
            self.ctx
                .c
                .serialize_type_name(self.ctx.b, node.expr_name(), true, type_arguments);
        if serialized_name.is_some() {
            return self
                .ctx
                .c
                .set_text_range(self.ctx.b, serialized_name, node.expr_name());
        }
        Node::NIL
    }

    // Go: checker/nodecopy.go:416 tryVisitTypeReference
    fn try_visit_type_reference(&mut self, node: Node) -> Node {
        if is_const_type_reference(node) {
            return Node::NIL;
        }
        let s = self
            .ctx
            .c
            .try_get_resolved_symbol_from_type_node(self.ctx.b, node);
        if s.is_nil() {
            return Node::NIL; // ???
        }
        if self
            .ctx
            .c
            .sym(s)
            .flags
            .intersects(SymbolFlags::TYPE_PARAMETER)
        {
            let declared_type = self.ctx.c.get_declared_type_of_symbol(s);
            let mapper = nb_ctx(self.ctx.b).borrow().mapper;
            if mapper.is_some()
                && self.ctx.c.get_mapped_type(declared_type, mapper) != declared_type
            {
                return Node::NIL; // refers to type parameter remapped by context (TODO improvement: just return the remapped param name?)
            }
        }
        let b = self.ctx.b;
        let node_type = self.ctx.c.nb_get_type_from_type_node(b, node, false);
        if !self
            .ctx
            .c
            .can_reuse_existing_js_type_node(b, node, node_type)
        {
            // fallback to serialization for jsdoc types that have insufficient or incomplete type args, or are remapped by the checker in only jsdoc contexts
            // TODO: remappings like `promise` -> `Promise<any>` are static, we *could* statically remap the nodes, too. But that only matters for `isolatedDeclarations`
            // in JS, should we enable that.
            return Node::NIL;
        }
        let (introduces_error, new_name, _) =
            self.track_existing_entity_name(node.type_name(), Node::NIL);
        if !introduces_error {
            let e = self.ctx.e;
            let type_arguments = self.visit_nodes(node.type_argument_list());
            let updated = e
                .factory()
                .update_type_reference_node(node, new_name, type_arguments);
            self.ctx.c.set_text_range(self.ctx.b, updated, node)
        } else {
            let type_arguments = self.visit_nodes(node.type_argument_list());
            let serialized_name =
                self.ctx
                    .c
                    .serialize_type_name(self.ctx.b, node.type_name(), false, type_arguments);
            if serialized_name.is_some() {
                return self
                    .ctx
                    .c
                    .set_text_range(self.ctx.b, serialized_name, node.type_name());
            }
            Node::NIL
        }
    }

    // Go: checker/nodecopy.go:452 tryVisitSimpleTypeNode
    fn try_visit_simple_type_node(&mut self, node: Node) -> Node {
        let inner_node = skip_parentheses(node);
        match inner_node.kind() {
            SyntaxKind::TypeReference => return self.try_visit_type_reference(inner_node),
            SyntaxKind::TypeQuery => return self.try_visit_type_query(inner_node),
            SyntaxKind::IndexedAccessType => return self.try_visit_indexed_access(inner_node),
            SyntaxKind::TypeOperator => {
                if inner_node.operator() == SyntaxKind::KeyOfKeyword {
                    return self.try_visit_key_of(inner_node);
                }
            }
            _ => {}
        }
        self.visit_node(node)
    }

    // Go: checker/nodecopy.go:468 visitExistingNodeTreeSymbolsWorker
    fn visit_existing_node_tree_symbols_worker(&mut self, mut node: Node) -> Node {
        let b = self.ctx.b;
        let e = self.ctx.e;
        let factory: &NodeFactory = e.factory();
        // !!! TODO: the reparser *should* make all the jsdoc remapping logic here redundant,
        // assuming we only ever try to preserve reparsed nodes and never walk back to the jsdoc "originals"
        // accidentally.
        // Still, what can be ported of the logic is here, just in case.
        // Begin JSDoc handling
        if node.kind() == SyntaxKind::JsDocTypeExpression {
            // Unwrap JSDocTypeExpressions
            return self.visit_node(node.type_());
        }
        // !!! TODO: We don't _actually_ support jsdoc namepath types, emit `any` instead; verify we handle as gracefully as strada
        if node.kind() == SyntaxKind::JsDocAllType
        /* || node.Kind == ast.JSDocNamepathType */
        {
            return factory.new_keyword_type_node(SyntaxKind::AnyKeyword);
        }
        // !!! TODO: verify JSDocUnknwonType is hopefully just parsed into `unknown` upfront; the kind no longer exists
        if node.kind() == SyntaxKind::JsDocNullableType {
            let union_members = [
                self.visit_node(node.type_()),
                factory
                    .new_literal_type_node(factory.new_keyword_expression(SyntaxKind::NullKeyword)),
            ];
            return factory.new_union_type_node(factory.new_node_list(&union_members));
        }
        if node.kind() == SyntaxKind::JsDocOptionalType {
            let union_members = [
                self.visit_node(node.type_()),
                factory.new_keyword_type_node(SyntaxKind::UndefinedKeyword),
            ];
            return factory.new_union_type_node(factory.new_node_list(&union_members));
        }
        if node.kind() == SyntaxKind::JsDocNonNullableType {
            // Unwrap
            return self.visit_node(node.type_());
        }
        if node.kind() == SyntaxKind::JsDocVariadicType {
            // !!! TODO: verify this matches how jsdoc variadics are actually handled now?
            let element = self.visit_node(node.type_());
            return factory.new_array_type_node(element);
        }
        if node.kind() == SyntaxKind::JsDocTypeLiteral {
            let mut members = Vec::new();
            for t in node.js_doc_property_tags() {
                if t.kind() != SyntaxKind::JsDocPropertyTag
                    && t.kind() != SyntaxKind::JsDocParameterTag
                {
                    continue;
                }
                let n = t.name();
                let target_name = if is_identifier(n) {
                    n
                } else {
                    n.right() // !!! TODO: without typesystem backup, doing this cast unguarded seems really suspect, even though it is what strada does
                };
                let name = self.visit_node(target_name);
                let should_be_optional = t.is_bracketed()
                    || (t.type_expression().is_some()
                        && t.type_expression().kind() == SyntaxKind::JsDocOptionalType);
                let mut question = Node::NIL;
                if should_be_optional {
                    question = factory.new_token(SyntaxKind::QuestionToken);
                }
                let ty = self.visit_node(t.type_expression()); // !!! TODO: alternate lookup locations for the type? serialize on demand if it doesn't serialze? strada does something funky here.

                members.push(factory.new_property_signature_declaration(
                    ModifierList::NIL,
                    name,
                    question,
                    ty,
                    Node::NIL,
                ));
            }
            return factory.new_type_literal_node(factory.new_node_list(&members));
        }
        // (Go keeps commented-out JSDocIndexSignature and JSDocFunctionType handling here.)
        // End JSDoc handling

        if is_type_reference_node(node)
            && is_identifier(node.type_name())
            && node.type_name().text().is_empty()
        {
            let replacement = factory.new_keyword_type_node(SyntaxKind::AnyKeyword);
            e.set_original(replacement, node);
            return replacement;
        }
        if is_this_type_node(node) {
            // TODO: strada never marks `this` type nodes as an error - it calls `canReuseTypeNode` on it, but that function always returns `true` for `this`
            // type nodes, which in turn fails to verify that the `this` context is the same between the source and target locations. The conservative thing is to
            // _never_ copy a `this`. We could improve this, but strada is *definitely* wrong and overbroad here. (note that we're inling uses of `canReuseTypeNode`
            // in corsa because of the unfurled host structure meaning we don't need to defer to a host object for functionality it needs)
            // bound.markError(nil) // conservative approach
            return node;
        }
        if is_type_parameter_declaration(node) {
            let (_, new_name, _) = self.track_existing_entity_name(node.name(), Node::NIL);
            let modifiers = self.visit_modifiers(node.modifiers());
            let constraint = self.visit_node(node.constraint());
            let expression = self.visit_node(node.expression());
            let default_type = self.visit_node(node.default_type());
            return factory.update_type_parameter_declaration(
                node,
                modifiers,
                new_name,
                constraint,
                expression,
                default_type,
            );
        }
        if is_indexed_access_type_node(node) {
            let result = self.try_visit_indexed_access(node);
            if result.is_some() {
                return result;
            }
            self.ctx.bound.borrow_mut().mark_error(None);
            return node;
        }
        if is_type_reference_node(node) {
            let result = self.try_visit_type_reference(node);
            if result.is_some() {
                return result;
            }
            self.ctx.bound.borrow_mut().mark_error(None);
            return node;
        }
        if is_type_query_node(node) {
            let result = self.try_visit_type_query(node);
            if result.is_some() {
                return result;
            }
            self.ctx.bound.borrow_mut().mark_error(None);
            return node;
        }
        if is_type_operator_node(node) {
            if node.operator() == SyntaxKind::UniqueKeyword
                && node.type_().kind() == SyntaxKind::SymbolKeyword
            {
                let non_fake_enclosing =
                    self.ctx.c.get_enclosing_declaration_ignoring_fake_scope(b);
                let same_scope = find_ancestor(node, |a| a == non_fake_enclosing);
                if same_scope.is_nil() {
                    self.ctx.bound.borrow_mut().mark_error(None);
                    return node;
                }
            } else if node.operator() == SyntaxKind::KeyOfKeyword {
                let result = self.try_visit_key_of(node);
                if result.is_some() {
                    return result;
                }
                self.ctx.bound.borrow_mut().mark_error(None);
                return node;
            }
        }
        if is_literal_import_type_node(node) {
            // assert keyword in imported attributes is deprecated, so we don't reuse types that contain it
            // Ex: import("pkg", { assert: {} }
            if node.attributes().is_some() && node.attributes().token() == SyntaxKind::AssertKeyword
            {
                self.ctx.bound.borrow_mut().mark_error(None);
                return node;
            }
            let t = self.ctx.c.nb_get_type_from_type_node(b, node, true);
            if t.is_nil() {
                self.ctx.bound.borrow_mut().mark_error(None);
                return node;
            }
            if is_in_js_file(node) {
                // !!! TODO: invalidate node reuse if js fallback logic used in type param list/typeof lookup (but isn't this logic gone?)
                // s := b.ch.symbolNodeLinks.Get(node).resolvedSymbol
            }
            let original_spec = node.argument().literal();
            let mut specifier = self.ctx.c.rewrite_module_specifier(b, node, original_spec);
            if original_spec == specifier {
                specifier = self.visit_node(specifier); // visit node if not replaced
            }
            let mut arg = node.argument();
            if specifier != original_spec {
                arg = factory.new_literal_type_node(specifier);
            }
            let attributes = self.visit_node(node.attributes());
            let qualifier = self.visit_node(node.qualifier());
            let type_arguments = self.visit_nodes(node.type_argument_list());
            return factory.update_import_type_node(
                node,
                node.is_type_of(),
                arg,
                attributes,
                qualifier,
                type_arguments,
            );
        }
        if node.name().is_some()
            && node.name().kind() == SyntaxKind::ComputedPropertyName
            && !self.ctx.c.has_late_bindable_name(node)
        {
            if !has_dynamic_name(node) {
                // !!! TODO: This matches strada, but rather than recursing, this should probably fall down to later cases.
                // Take a `["field"]` property declaration - it still needs a `: any` appended to it
                return self.visit_each_child(node);
            }
            // !!! TODO: this condition matches strada, but it just seems wrong? Or at the very least extraordinarily approximate, and doesn't flag a builder error...
            let allow_unresolved_names = nb_ctx(b)
                .borrow()
                .internal_flags
                .intersects(InternalNodeBuilderFlags::ALLOW_UNRESOLVED_NAMES);
            let should_remove_declaration = !(allow_unresolved_names
                && is_entity_name_expression(node.name().expression())
                && {
                    // PORT: split into two statements to satisfy the borrow checker.
                    let t = self.ctx.c.check_computed_property_name(node.name());
                    self.ctx.c.ty(t).flags.intersects(TypeFlags::ANY)
                });
            if should_remove_declaration {
                return Node::NIL;
            }
        }
        if (is_function_like(node) && node.type_().is_nil())
            || (is_property_declaration(node)
                && node.type_().is_nil()
                && node.initializer().is_nil())
            || (is_property_signature_declaration(node)
                && node.type_().is_nil()
                && node.initializer().is_nil())
            || (is_parameter_declaration(node)
                && node.type_().is_nil()
                && node.initializer().is_nil())
        {
            let mut visited = self.visit_each_child(node);
            if visited == node {
                let cloned = factory.clone_node(node);
                visited = self.ctx.c.set_text_range(b, cloned, node);
            }
            node = visited;
            let new_type = factory.new_keyword_type_node(SyntaxKind::AnyKeyword);
            match node.kind() {
                SyntaxKind::PropertyDeclaration => {
                    return factory.update_property_declaration(
                        node,
                        node.modifiers(),
                        node.name(),
                        node.postfix_token(),
                        new_type,
                        Node::NIL,
                    );
                }
                SyntaxKind::PropertySignature => {
                    return factory.update_property_signature_declaration(
                        node,
                        node.modifiers(),
                        node.name(),
                        node.postfix_token(),
                        new_type,
                        Node::NIL,
                    );
                }
                SyntaxKind::Parameter => {
                    return factory.update_parameter_declaration(
                        node,
                        ModifierList::NIL,
                        node.dot_dot_dot_token(),
                        node.name(),
                        node.question_token(),
                        new_type,
                        Node::NIL,
                    );
                }
                SyntaxKind::MethodSignature => {
                    return factory.update_method_signature_declaration(
                        node,
                        node.modifiers(),
                        node.name(),
                        node.postfix_token(),
                        node.type_parameter_list(),
                        node.parameter_list(),
                        new_type,
                    );
                }
                SyntaxKind::CallSignature => {
                    return factory.update_call_signature_declaration(
                        node,
                        node.type_parameter_list(),
                        node.parameter_list(),
                        new_type,
                    );
                }
                SyntaxKind::JsDocSignature => {
                    return factory.update_js_doc_signature(
                        node,
                        node.type_parameter_list(),
                        node.parameter_list(),
                        new_type,
                    );
                }
                SyntaxKind::ConstructSignature => {
                    return factory.update_construct_signature_declaration(
                        node,
                        node.type_parameter_list(),
                        node.parameter_list(),
                        new_type,
                    );
                }
                SyntaxKind::IndexSignature => {
                    return factory.update_index_signature_declaration(
                        node,
                        node.modifiers(),
                        node.parameter_list(),
                        new_type,
                    );
                }
                SyntaxKind::FunctionType => {
                    return factory.update_function_type_node(
                        node,
                        node.type_parameter_list(),
                        node.parameter_list(),
                        new_type,
                    );
                }
                SyntaxKind::ConstructorType => {
                    return factory.update_constructor_type_node(
                        node,
                        node.modifiers(),
                        node.type_parameter_list(),
                        node.parameter_list(),
                        new_type,
                    );
                }
                _ => {}
            }
        }
        if is_computed_property_name(node) && is_entity_name_expression(node.expression()) {
            let (introduces_error, result, _) =
                self.track_existing_entity_name(node.expression(), Node::NIL);
            if !introduces_error {
                return factory.update_computed_property_name(node, result);
            } else {
                // !!! TODO: rewriting computed names based on evaluator/typecheck results?
                // strada's behavior seems hard to justify vs marking an error and moving on
                self.ctx.bound.borrow_mut().mark_error(None);
                return self.visit_each_child(node);
            }
        }
        if is_type_predicate_node(node) {
            let parameter_name = if is_identifier(node.parameter_name()) {
                let (introduces_error, result, _) =
                    self.track_existing_entity_name(node.parameter_name(), Node::NIL);
                // Should not usually happen the only case is when a type predicate comes from a JSDoc type annotation with it's own parameter symbol definition.
                // /** @type {(v: unknown) => v is undefined} */
                // const isUndef = v => v === undefined;
                if introduces_error {
                    self.ctx.bound.borrow_mut().mark_error(None);
                }
                result
            } else {
                factory.clone_node(node.parameter_name())
            };
            let asserts_modifier = self.visit_node(node.asserts_modifier());
            let type_ = self.visit_node(node.type_());
            return factory.update_type_predicate_node(
                node,
                asserts_modifier,
                parameter_name,
                type_,
            );
        }
        if is_conditional_type_node(node) {
            let check_type = self.visit_node(node.check_type());
            let infer_type_parameters = self.ctx.c.get_infer_type_parameters(node);
            // PORT: Go passes nil slices and a nil mapper.
            let dispose = self.ctx.c.enter_new_scope(
                b,
                node,
                &[],
                &infer_type_parameters,
                &[],
                MapperId::NIL,
            );
            let extends_type = self.visit_node(node.extends_type());
            let true_type = self.visit_node(node.true_type());
            dispose(&mut *self.ctx.c);
            let false_type = self.visit_node(node.false_type());
            return factory.update_conditional_type_node(
                node,
                check_type,
                extends_type,
                true_type,
                false_type,
            );
        }

        // style applications
        let multiline_object_literals = nb_ctx(b)
            .borrow()
            .flags
            .intersects(NodeBuilderFlags::MULTILINE_OBJECT_LITERALS);
        if is_tuple_type_node(node)
            || (!multiline_object_literals && is_type_literal_node(node))
            || is_mapped_type_node(node)
        {
            // make tuples/types/mappedtypes single line
            let mut res = self.visit_each_child(node);
            if res == node {
                res = factory.clone_node(res);
                res = self.ctx.c.set_text_range(b, res, node);
            }
            e.add_emit_flags(res, EmitFlags::SINGLE_LINE);
            return res;
        }

        if is_string_literal_like(node) {
            // Preserve the original characters of the literal (e.g. emojis) in declaration emit
            // rather than escaping them as ASCII Unicode escapes. Mirrors TypeScript's behavior
            // for synthesized string literal types in the node builder (checker.ts:6853).
            let mut c = factory.clone_node(node);
            let use_single_quotes = nb_ctx(b)
                .borrow()
                .flags
                .intersects(NodeBuilderFlags::USE_SINGLE_QUOTES_FOR_STRING_LITERAL_TYPE);
            if is_string_literal(node)
                && use_single_quotes
                && !node.token_flags().intersects(TokenFlags::SINGLE_QUOTE)
            {
                // set single quote on string literals
                // PORT: Go flips TokenFlags on the clone in place. Synthetic
                // nodes are immutable, so build the clone again with the flipped
                // flags and copy its loc and node flags. The ast clone has no
                // emit-context entries yet, so nothing else is lost.
                let flipped = factory.new_string_literal(
                    c.text(),
                    TokenFlags(c.token_flags().0 ^ TokenFlags::SINGLE_QUOTE.0),
                );
                set_node_loc(flipped, c.loc());
                set_node_flags(flipped, c.flags());
                c = flipped;
            }
            e.add_emit_flags(c, EmitFlags::NO_ASCII_ESCAPING);
            return c;
        }

        self.visit_each_child(node)
    }
}

impl<'a> ExistingNodeTreeVisitor<'a> {
    // Go: checker/nodecopy.go:827 visitor Visit func (inside getExistingNodeTreeVisitor)
    fn existing_node_tree_visit(&mut self, node: Node) -> Node {
        // If there was an error in a sibling node bail early, the result will be discarded anyway
        if self.ctx.bound.borrow().had_error {
            return node;
        }
        let b = self.ctx.b;
        let e = self.ctx.e;
        let recover = self.ctx.bound.borrow().start_recovery_scope();
        let introduces_new_scope = is_function_like(node) || is_mapped_type_node(node);
        let mut exit = None;
        if introduces_new_scope {
            let mut params: Vec<SymbolId> = Vec::new();
            let mut type_params: Vec<TypeId> = Vec::new();
            if is_function_like(node) {
                let sig = self.ctx.c.get_signature_from_declaration(node);
                params = self.ctx.c.sig(sig).parameters.clone();
                type_params = self.ctx.c.sig(sig).type_parameters.clone();
            } else if is_conditional_type_node(node) {
                // !!! TODO: impossible in combination with the scope start check???
                type_params = self.ctx.c.get_infer_type_parameters(node);
            } else if is_mapped_type_node(node) {
                let tp_symbol = self.ctx.c.get_symbol_of_declaration(node.type_parameter());
                type_params = vec![self.ctx.c.get_declared_type_of_type_parameter(tp_symbol)];
            }
            // PORT: Go passes nil for originalParameters and mapper.
            exit = Some(self.ctx.c.enter_new_scope(
                b,
                node,
                &params,
                &type_params,
                &[],
                MapperId::NIL,
            ));
        }
        let mut result = self.visit_existing_node_tree_symbols_worker(node);
        if let Some(exit) = exit {
            exit(&mut *self.ctx.c);
        }

        if result == node && !node_is_synthesized(node) {
            result = e.factory().deep_clone_node(node); // always clone a new node
        }

        // We want to clone the subtree, so when we mark it up with __pos and __end in quickfixes,
        //  we don't get odd behavior because of reused nodes. We also need to clone to _remove_
        //  the position information if the node comes from a different file than the one the node builder
        //  is set to build for (even though we are reusing the node structure, the position information
        //  would make the printer print invalid spans for literals and identifiers, and the formatter would
        //  choke on the mismatched positonal spans between a parent and an injected child from another file).
        result = self.ctx.c.set_text_range(b, result, node);

        if self.ctx.bound.borrow().had_error {
            if is_type_node(node) && !is_type_predicate_node(node) {
                self.ctx.bound.borrow_mut().end_recovery_scope(recover);
                // TODO: this fallback matches strada behavior, but it lacks any verification that the type from `node` actually matches
                // the type we'd expect at this traversal position within the parent type.
                let t = self.ctx.c.nb_get_type_from_type_node(b, node, false);
                return self.ctx.c.type_to_type_node(b, t);
            }
            let cloned = e.factory().clone_node(node);
            return self.ctx.c.set_text_range(b, cloned, node);
        }

        result
    }

    // Go: checker/nodecopy.go:873 NodeVisitorHooks.VisitNodes (inside getExistingNodeTreeVisitor)
    // PORT: Go gets the visitor as `v`; here it is `self`.
    fn hook_visit_nodes(&mut self, nodes: NodeList) -> NodeList {
        let mut res = self.visit_nodes(nodes);
        if self.ctx.non_local_node && res.is_some() {
            // Remove position data from node lists originating in other files
            // PORT: Go clones the list when unchanged and then sets Loc in
            // place. NodeList Loc is fixed at creation, so always build a new
            // list with the (-1, -1) range.
            res = self
                .ctx
                .e
                .factory()
                .new_synthetic_node_list(&res.nodes().to_vec(), TextRange::new(-1, -1));
        }
        res
    }

    // Go: checker/nodecopy.go:890 NodeVisitorHooks.VisitNode (inside getExistingNodeTreeVisitor)
    fn hook_visit_node(&mut self, node: Node) -> Node {
        // Capture if the current node is in the current file so node lists knoww if they can keep positions or not
        let old_non_local_node = self.ctx.non_local_node;
        let enclosing_file = nb_ctx(self.ctx.b).borrow().enclosing_file;
        self.ctx.non_local_node = enclosing_file.is_nil()
            || enclosing_file != get_source_file_of_node(self.ctx.e.most_original(node));
        let res = self.visit_node(node);
        self.ctx.non_local_node = old_non_local_node;
        res
    }
}

/// Go `node.End() - node.Pos()`: the number of Go bytes of the node's text.
// PORT: source text is a port form (see `scanner_util::GO_STRING_MARKER`),
// whose offsets differ from Go byte offsets after a unit. Node positions are
// unit boundaries, so the Go length of the text between them is the Go
// difference.
fn go_node_text_len(node: Node) -> i32 {
    let loc = node.loc();
    let port_len = loc.end() - loc.pos();
    if loc.pos() < 0 {
        return port_len;
    }
    let file = get_source_file_of_node(node);
    if file.is_nil() {
        return port_len;
    }
    match source_file_text(file).get(loc.pos() as usize..loc.end() as usize) {
        Some(text) => go_len(text) as i32,
        None => port_len,
    }
}
