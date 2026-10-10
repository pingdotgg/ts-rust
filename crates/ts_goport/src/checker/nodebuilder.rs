//! Port of Go `checker/nodebuilder.go`: the `NodeBuilder` entry points that
//! push a context, call into `NodeBuilderImpl` and pop the context.
//!
//! PORT: Go `NodeBuilder` methods reach the checker through `b.impl.ch`. Here
//! the entry points are `impl Checker` methods named `node_builder_<name>`
//! that take `nb: &Rc<RefCell<NodeBuilder>>`. Methods that do not need the
//! checker stay on `NodeBuilder`.

use crate::prelude::*;
use crate::printer::{EmitContext, new_emit_context};

// Go: checker/nodebuilder.go:10 NodeBuilder
// PORT: Go `VerbosityContext` (line 21) is defined in printer_impl.rs, with
// shared cells for the two output fields.
pub struct NodeBuilder {
    pub ctx_stack: Vec<Rc<RefCell<NodeBuilderContext>>>,
    pub host: &'static GoProgram,
    pub impl_: Rc<RefCell<NodeBuilderImpl>>,
    pub verbosity: Option<VerbosityContext>, // nil for non-hover callers
}

impl NodeBuilder {
    // Go: checker/nodebuilder.go:29 NodeBuilder.EmitContext
    // EmitContext implements NodeBuilderInterface.
    pub fn emit_context(&self) -> Rc<EmitContext> {
        self.impl_.borrow().e.clone()
    }

    // Go: checker/nodebuilder.go:33 NodeBuilder.enterContext
    pub fn enter_context(
        &mut self,
        enclosing_declaration: Node,
        flags: NodeBuilderFlags,
        internal_flags: InternalNodeBuilderFlags,
        tracker: Option<Rc<dyn SymbolTracker>>,
    ) {
        let mut verbosity_level = -1;
        let mut max_truncation_length = 0;
        if let Some(verbosity) = &self.verbosity {
            verbosity_level = verbosity.level;
            max_truncation_length = verbosity.max_truncation_length;
        }
        let old_ctx = self.impl_.borrow().ctx.clone();
        self.ctx_stack.push(old_ctx);
        let mut ctx = NodeBuilderContext::new(self.host);
        ctx.flags = flags;
        ctx.internal_flags = internal_flags;
        ctx.max_expansion_depth = verbosity_level;
        ctx.max_truncation_length = max_truncation_length;
        ctx.enclosing_declaration = enclosing_declaration;
        // PORT: Go `ast.GetSourceFileOfNode(nil)` returns nil. The Rust
        // function panics on nil, so the nil case is checked here.
        ctx.enclosing_file = if enclosing_declaration.is_some() {
            get_source_file_of_node(enclosing_declaration)
        } else {
            Node::NIL
        };
        let ctx = Rc::new(RefCell::new(ctx));
        self.impl_.borrow_mut().ctx = ctx.clone();
        let tracker: Rc<dyn SymbolTracker> = new_symbol_tracker_impl(ctx.clone(), tracker);
        ctx.borrow_mut().tracker = tracker;
    }

    // Go: checker/nodebuilder.go:62 NodeBuilder.propagateVerbosityOut
    // propagateVerbosityOut copies expansion signals from the context to the VerbosityContext output.
    pub fn propagate_verbosity_out(&self) {
        if let Some(verbosity) = &self.verbosity {
            let ctx = self.impl_.borrow().ctx.clone();
            let c = ctx.borrow();
            // Only set to true, never clear, multiple calls share the same VerbosityContext
            if c.can_increase_expansion_depth {
                verbosity.can_increase_verbosity.set(true);
            }
            if c.expansion_truncated {
                verbosity.truncated.set(true);
            }
        }
    }

    // Go: checker/nodebuilder.go:74 NodeBuilder.popContext
    pub fn pop_context(&mut self) {
        if let Some(ctx) = self.ctx_stack.pop() {
            self.impl_.borrow_mut().ctx = ctx;
        } else {
            // PORT: Go sets `ctx` to nil. See `nil_context`.
            self.impl_.borrow_mut().ctx = nil_context(self.host);
        }
    }
}

// Go: checker/nodebuilder.go:188 simplifyClassDeclaration
fn simplify_class_declaration(
    c: &Checker,
    f: &crate::ast::NodeFactory,
    mut class_decl: Node,
    symbol: SymbolId,
) -> Node {
    let original_class_decl = c
        .sym(symbol)
        .declarations
        .iter()
        .copied()
        .find(|&d| is_class_like(d))
        .unwrap_or(class_decl);
    let modifiers =
        original_class_decl.modifier_flags() & !(ModifierFlags::EXPORT | ModifierFlags::AMBIENT);
    let is_anonymous = is_class_expression(original_class_decl);
    if is_anonymous {
        class_decl = f.update_class_declaration(
            class_decl,
            class_decl.modifiers(),
            Node::NIL,
            class_decl.type_parameter_list(),
            class_decl.heritage_clauses(),
            class_decl.member_list(),
        );
    }
    replace_modifiers(
        f,
        class_decl,
        f.new_modifier_list(&create_modifiers_from_modifier_flags(modifiers, &mut |k| {
            f.new_modifier(k)
        })),
    )
}

// Go: checker/nodebuilder.go:212 simplifyModifiers
fn simplify_modifiers(
    c: &Checker,
    f: &crate::ast::NodeFactory,
    new_decl: Node,
    is_decl_kind: fn(Node) -> bool,
    symbol: SymbolId,
) -> Node {
    let decl_with_modifiers = c
        .sym(symbol)
        .declarations
        .iter()
        .copied()
        .find(|&d| is_decl_kind(d))
        .unwrap_or(new_decl);
    let modifiers =
        decl_with_modifiers.modifier_flags() & !(ModifierFlags::EXPORT | ModifierFlags::AMBIENT);
    replace_modifiers(
        f,
        new_decl,
        f.new_modifier_list(&create_modifiers_from_modifier_flags(modifiers, &mut |k| {
            f.new_modifier(k)
        })),
    )
}

/// Go `b.impl`.
fn nb_impl(nb: &Rc<RefCell<NodeBuilder>>) -> Rc<RefCell<NodeBuilderImpl>> {
    nb.borrow().impl_.clone()
}

impl Checker {
    // Go: checker/nodebuilder.go:84 NodeBuilder.exitContext
    // PORT: a Checker method, because the truncation check calls the tracker.
    fn nb_exit_context(&mut self, nb: &Rc<RefCell<NodeBuilder>>, result: Node) -> Node {
        nb.borrow().propagate_verbosity_out();
        self.nb_exit_context_check(nb);
        let encountered_error = nb_impl(nb).borrow().ctx.borrow().encountered_error;
        nb.borrow_mut().pop_context();
        if encountered_error {
            return Node::NIL;
        }
        result
    }

    // Go: checker/nodebuilder.go:94 NodeBuilder.exitContextSlice
    // PORT: Go returns a nil slice on error. Here it is an empty `Vec`.
    fn nb_exit_context_slice(
        &mut self,
        nb: &Rc<RefCell<NodeBuilder>>,
        result: Vec<Node>,
    ) -> Vec<Node> {
        nb.borrow().propagate_verbosity_out();
        self.nb_exit_context_check(nb);
        let encountered_error = nb_impl(nb).borrow().ctx.borrow().encountered_error;
        nb.borrow_mut().pop_context();
        if encountered_error {
            return Vec::new();
        }
        result
    }

    // Go: checker/nodebuilder.go:104 NodeBuilder.exitContextCheck
    fn nb_exit_context_check(&mut self, nb: &Rc<RefCell<NodeBuilder>>) {
        let ctx = nb_impl(nb).borrow().ctx.clone();
        let tracker = {
            let c = ctx.borrow();
            if c.truncating && c.flags.intersects(NodeBuilderFlags::NO_TRUNCATION) {
                Some(c.tracker.clone())
            } else {
                None
            }
        };
        if let Some(tracker) = tracker {
            tracker.report_truncation_error(self);
        }
    }

    // Go: checker/nodebuilder.go:111 NodeBuilder.IndexInfoToIndexSignatureDeclaration
    // IndexInfoToIndexSignatureDeclaration implements NodeBuilderInterface.
    pub fn node_builder_index_info_to_index_signature_declaration(
        &mut self,
        nb: &Rc<RefCell<NodeBuilder>>,
        info: IndexInfoId,
        enclosing_declaration: Node,
        flags: NodeBuilderFlags,
        internal_flags: InternalNodeBuilderFlags,
        tracker: Option<Rc<dyn SymbolTracker>>,
    ) -> Node {
        nb.borrow_mut()
            .enter_context(enclosing_declaration, flags, internal_flags, tracker);
        let b = nb_impl(nb);
        let result = self.index_info_to_index_signature_declaration_helper(&b, info, Node::NIL);
        self.nb_exit_context(nb, result)
    }

    // Go: checker/nodebuilder.go:117 NodeBuilder.SerializeReturnTypeForSignature
    // SerializeReturnTypeForSignature implements NodeBuilderInterface.
    pub fn node_builder_serialize_return_type_for_signature(
        &mut self,
        nb: &Rc<RefCell<NodeBuilder>>,
        signature_declaration: Node,
        enclosing_declaration: Node,
        flags: NodeBuilderFlags,
        internal_flags: InternalNodeBuilderFlags,
        tracker: Option<Rc<dyn SymbolTracker>>,
    ) -> Node {
        nb.borrow_mut()
            .enter_context(enclosing_declaration, flags, internal_flags, tracker);
        let b = nb_impl(nb);
        let signature = self.get_signature_from_declaration(signature_declaration);
        let (_, cleanup) = self.enter_signature_scope(&b, signature);
        let result = self.serialize_return_type_for_signature(&b, signature, true);
        cleanup(self);
        self.nb_exit_context(nb, result)
    }

    // Go: checker/nodebuilder.go:126 NodeBuilder.SerializeTypeParametersForSignature
    pub fn node_builder_serialize_type_parameters_for_signature(
        &mut self,
        nb: &Rc<RefCell<NodeBuilder>>,
        signature_declaration: Node,
        enclosing_declaration: Node,
        flags: NodeBuilderFlags,
        internal_flags: InternalNodeBuilderFlags,
        tracker: Option<Rc<dyn SymbolTracker>>,
    ) -> Vec<Node> {
        nb.borrow_mut().enter_context(
            enclosing_declaration,
            flags,
            internal_flags,
            tracker.clone(),
        );
        let symbol = self.get_symbol_of_declaration(signature_declaration);
        let type_params = self.node_builder_symbol_to_type_parameter_declarations(
            nb,
            symbol,
            enclosing_declaration,
            flags,
            internal_flags,
            tracker,
        );
        self.nb_exit_context_slice(nb, type_params)
    }

    // Go: checker/nodebuilder.go:134 NodeBuilder.SerializeTypeForDeclaration
    // SerializeTypeForDeclaration implements NodeBuilderInterface.
    pub fn node_builder_serialize_type_for_declaration(
        &mut self,
        nb: &Rc<RefCell<NodeBuilder>>,
        declaration: Node,
        symbol: SymbolId,
        enclosing_declaration: Node,
        flags: NodeBuilderFlags,
        internal_flags: InternalNodeBuilderFlags,
        tracker: Option<Rc<dyn SymbolTracker>>,
    ) -> Node {
        nb.borrow_mut()
            .enter_context(enclosing_declaration, flags, internal_flags, tracker);
        let b = nb_impl(nb);
        let result =
            self.serialize_type_for_declaration(&b, declaration, TypeId::NIL, symbol, true);
        self.nb_exit_context(nb, result)
    }

    // Go: checker/nodebuilder.go:140 NodeBuilder.SerializeTypeForExpression
    // SerializeTypeForExpression implements NodeBuilderInterface.
    pub fn node_builder_serialize_type_for_expression(
        &mut self,
        nb: &Rc<RefCell<NodeBuilder>>,
        expr: Node,
        enclosing_declaration: Node,
        flags: NodeBuilderFlags,
        internal_flags: InternalNodeBuilderFlags,
        tracker: Option<Rc<dyn SymbolTracker>>,
    ) -> Node {
        nb.borrow_mut()
            .enter_context(enclosing_declaration, flags, internal_flags, tracker);
        let b = nb_impl(nb);
        let result = self.serialize_type_for_expression(&b, expr);
        self.nb_exit_context(nb, result)
    }

    // Go: checker/nodebuilder.go:146 NodeBuilder.SignatureToSignatureDeclaration
    // SignatureToSignatureDeclaration implements NodeBuilderInterface.
    pub fn node_builder_signature_to_signature_declaration(
        &mut self,
        nb: &Rc<RefCell<NodeBuilder>>,
        signature: SignatureId,
        kind: SyntaxKind,
        enclosing_declaration: Node,
        flags: NodeBuilderFlags,
        internal_flags: InternalNodeBuilderFlags,
        tracker: Option<Rc<dyn SymbolTracker>>,
    ) -> Node {
        nb.borrow_mut()
            .enter_context(enclosing_declaration, flags, internal_flags, tracker);
        let b = nb_impl(nb);
        let result = self.signature_to_signature_declaration_helper(&b, signature, kind, None);
        self.nb_exit_context(nb, result)
    }

    // Go: checker/nodebuilder.go:152 NodeBuilder.ExpandSymbolForHover
    // ExpandSymbolForHover produces declaration nodes for a symbol with verbosity level support.
    pub fn node_builder_expand_symbol_for_hover(
        &mut self,
        nb: &Rc<RefCell<NodeBuilder>>,
        symbol: SymbolId,
        meaning: SymbolFlags,
    ) -> Vec<Node> {
        nb.borrow_mut().enter_context(
            Node::NIL,
            NodeBuilderFlags::IGNORE_ERRORS
                | NodeBuilderFlags::MULTILINE_OBJECT_LITERALS
                | NodeBuilderFlags::USE_ALIAS_DEFINED_OUTSIDE_CURRENT_SCOPE,
            InternalNodeBuilderFlags::NONE,
            None,
        );
        let b = nb_impl(nb);
        let ctx = b.borrow().ctx.clone();

        // Push the declared type onto the type stack to prevent re-expansion.
        // We push a nil sentinel after the real type so that isTypeOnStack
        // (which skips the last element) still checks declaredType.
        let declared_type = self.get_declared_type_of_symbol(symbol);
        ctx.borrow_mut().type_stack.push(declared_type);
        ctx.borrow_mut().type_stack.push(TypeId::NIL);

        let nodes = self.expand_symbol_for_hover(&b, symbol);

        {
            let mut c = ctx.borrow_mut();
            let n = c.type_stack.len() - 2;
            c.type_stack.truncate(n);
        }

        nb.borrow().propagate_verbosity_out();

        // Simplify declarations by applying original modifiers
        let e = b.borrow().e.clone();
        let f = e.factory().as_node_factory();
        let mut result: Vec<Node> = Vec::with_capacity(nodes.len());
        for node in nodes {
            match node.kind() {
                SyntaxKind::ClassDeclaration => {
                    result.push(simplify_class_declaration(self, f, node, symbol))
                }
                SyntaxKind::EnumDeclaration => result.push(simplify_modifiers(
                    self,
                    f,
                    node,
                    is_enum_declaration,
                    symbol,
                )),
                SyntaxKind::InterfaceDeclaration => {
                    if meaning.intersects(SymbolFlags::INTERFACE) {
                        result.push(simplify_modifiers(
                            self,
                            f,
                            node,
                            is_interface_declaration,
                            symbol,
                        ));
                    }
                }
                SyntaxKind::ModuleDeclaration => result.push(simplify_modifiers(
                    self,
                    f,
                    node,
                    is_module_declaration,
                    symbol,
                )),
                _ => {}
            }
        }

        self.nb_exit_context_slice(nb, result)
    }

    // Go: checker/nodebuilder.go:225 NodeBuilder.SymbolToEntityName
    // SymbolToEntityName implements NodeBuilderInterface.
    pub fn node_builder_symbol_to_entity_name(
        &mut self,
        nb: &Rc<RefCell<NodeBuilder>>,
        symbol: SymbolId,
        meaning: SymbolFlags,
        enclosing_declaration: Node,
        flags: NodeBuilderFlags,
        internal_flags: InternalNodeBuilderFlags,
        tracker: Option<Rc<dyn SymbolTracker>>,
    ) -> Node {
        nb.borrow_mut()
            .enter_context(enclosing_declaration, flags, internal_flags, tracker);
        let b = nb_impl(nb);
        let result = self.symbol_to_name(&b, symbol, meaning, false);
        self.nb_exit_context(nb, result)
    }

    // Go: checker/nodebuilder.go:231 NodeBuilder.SymbolToExpression
    // SymbolToExpression implements NodeBuilderInterface.
    pub fn node_builder_symbol_to_expression(
        &mut self,
        nb: &Rc<RefCell<NodeBuilder>>,
        symbol: SymbolId,
        meaning: SymbolFlags,
        enclosing_declaration: Node,
        flags: NodeBuilderFlags,
        internal_flags: InternalNodeBuilderFlags,
        tracker: Option<Rc<dyn SymbolTracker>>,
    ) -> Node {
        nb.borrow_mut()
            .enter_context(enclosing_declaration, flags, internal_flags, tracker);
        let b = nb_impl(nb);
        let result = self.symbol_to_expression(&b, symbol, meaning);
        self.nb_exit_context(nb, result)
    }

    // Go: checker/nodebuilder.go:237 NodeBuilder.SymbolToNode
    // SymbolToNode implements NodeBuilderInterface.
    pub fn node_builder_symbol_to_node(
        &mut self,
        nb: &Rc<RefCell<NodeBuilder>>,
        symbol: SymbolId,
        meaning: SymbolFlags,
        enclosing_declaration: Node,
        flags: NodeBuilderFlags,
        internal_flags: InternalNodeBuilderFlags,
        tracker: Option<Rc<dyn SymbolTracker>>,
    ) -> Node {
        nb.borrow_mut()
            .enter_context(enclosing_declaration, flags, internal_flags, tracker);
        let b = nb_impl(nb);
        let result = self.symbol_to_node(&b, symbol, meaning);
        self.nb_exit_context(nb, result)
    }

    // Go: checker/nodebuilder.go:243 NodeBuilder.SymbolToParameterDeclaration
    // SymbolToParameterDeclaration implements NodeBuilderInterface.
    // PORT: Go uses a value receiver, so the push and pop change a copy of
    // `ctxStack`. The impl is shared through a pointer, so `impl.ctx` ends up
    // the same as with a pointer receiver. This port uses the shared builder.
    pub fn node_builder_symbol_to_parameter_declaration(
        &mut self,
        nb: &Rc<RefCell<NodeBuilder>>,
        symbol: SymbolId,
        enclosing_declaration: Node,
        flags: NodeBuilderFlags,
        internal_flags: InternalNodeBuilderFlags,
        tracker: Option<Rc<dyn SymbolTracker>>,
    ) -> Node {
        nb.borrow_mut()
            .enter_context(enclosing_declaration, flags, internal_flags, tracker);
        let b = nb_impl(nb);
        let result = self.symbol_to_parameter_declaration(&b, symbol, false);
        self.nb_exit_context(nb, result)
    }

    // Go: checker/nodebuilder.go:249 NodeBuilder.SymbolToTypeParameterDeclarations
    // SymbolToTypeParameterDeclarations implements NodeBuilderInterface.
    pub fn node_builder_symbol_to_type_parameter_declarations(
        &mut self,
        nb: &Rc<RefCell<NodeBuilder>>,
        symbol: SymbolId,
        enclosing_declaration: Node,
        flags: NodeBuilderFlags,
        internal_flags: InternalNodeBuilderFlags,
        tracker: Option<Rc<dyn SymbolTracker>>,
    ) -> Vec<Node> {
        nb.borrow_mut()
            .enter_context(enclosing_declaration, flags, internal_flags, tracker);
        let b = nb_impl(nb);
        let result = self.symbol_to_type_parameter_declarations(&b, symbol);
        self.nb_exit_context_slice(nb, result)
    }

    // Go: checker/nodebuilder.go:255 NodeBuilder.TypeParameterToDeclaration
    // TypeParameterToDeclaration implements NodeBuilderInterface.
    pub fn node_builder_type_parameter_to_declaration(
        &mut self,
        nb: &Rc<RefCell<NodeBuilder>>,
        parameter: TypeId,
        enclosing_declaration: Node,
        flags: NodeBuilderFlags,
        internal_flags: InternalNodeBuilderFlags,
        tracker: Option<Rc<dyn SymbolTracker>>,
    ) -> Node {
        nb.borrow_mut()
            .enter_context(enclosing_declaration, flags, internal_flags, tracker);
        let b = nb_impl(nb);
        let result = self.type_parameter_to_declaration(&b, parameter);
        self.nb_exit_context(nb, result)
    }

    // Go: checker/nodebuilder.go:261 NodeBuilder.TypePredicateToTypePredicateNode
    // TypePredicateToTypePredicateNode implements NodeBuilderInterface.
    pub fn node_builder_type_predicate_to_type_predicate_node(
        &mut self,
        nb: &Rc<RefCell<NodeBuilder>>,
        predicate: TypePredicateId,
        enclosing_declaration: Node,
        flags: NodeBuilderFlags,
        internal_flags: InternalNodeBuilderFlags,
        tracker: Option<Rc<dyn SymbolTracker>>,
    ) -> Node {
        nb.borrow_mut()
            .enter_context(enclosing_declaration, flags, internal_flags, tracker);
        let b = nb_impl(nb);
        let result = self.type_predicate_to_type_predicate_node(&b, predicate);
        self.nb_exit_context(nb, result)
    }

    // Go: checker/nodebuilder.go:267 NodeBuilder.TypeToTypeNode
    // TypeToTypeNode implements NodeBuilderInterface.
    pub fn node_builder_type_to_type_node(
        &mut self,
        nb: &Rc<RefCell<NodeBuilder>>,
        typ: TypeId,
        enclosing_declaration: Node,
        flags: NodeBuilderFlags,
        internal_flags: InternalNodeBuilderFlags,
        tracker: Option<Rc<dyn SymbolTracker>>,
    ) -> Node {
        nb.borrow_mut()
            .enter_context(enclosing_declaration, flags, internal_flags, tracker);
        let b = nb_impl(nb);
        let result = self.type_to_type_node(&b, typ);
        self.nb_exit_context(nb, result)
    }

    // Go: checker/nodebuilder.go:272 NodeBuilder.TryJSTypeNodeToTypeNode
    pub fn node_builder_try_js_type_node_to_type_node(
        &mut self,
        nb: &Rc<RefCell<NodeBuilder>>,
        node: Node,
        enclosing_declaration: Node,
        flags: NodeBuilderFlags,
        internal_flags: InternalNodeBuilderFlags,
        tracker: Option<Rc<dyn SymbolTracker>>,
    ) -> Node {
        nb.borrow_mut()
            .enter_context(enclosing_declaration, flags, internal_flags, tracker);
        let b = nb_impl(nb);
        let result = self.try_js_type_node_to_type_node(&b, node);
        self.nb_exit_context(nb, result)
    }

    // Go: checker/nodebuilder.go:288 Checker.getNodeBuilder
    // PORT: Go also returns a release func (`Factory.ReleaseArenas`) that the
    // to-string entries defer. Here those entries open a print scope
    // (`PrintScope` in printer_impl.rs), and this builder's factory puts its
    // nodes in the print owner while one is open, so the end of the call
    // frees them.
    pub fn get_node_builder(&mut self) -> Rc<RefCell<NodeBuilder>> {
        if let Some(nb) = &self.type_to_string_nodebuilder {
            return nb.clone();
        }
        let nb = self.get_node_builder_ex(None /*idToSymbol*/);
        nb.borrow()
            .emit_context()
            .factory()
            .as_node_factory()
            .set_print_owned();
        self.type_to_string_nodebuilder = Some(nb.clone());
        nb
    }

    // Go: checker/nodebuilder.go:299 Checker.getNodeBuilderEx
    pub fn get_node_builder_ex(
        &mut self,
        id_to_symbol: Option<FxHashMap<Node, SymbolId>>,
    ) -> Rc<RefCell<NodeBuilder>> {
        Rc::new(RefCell::new(new_node_builder_ex(
            self,
            new_emit_context(),
            id_to_symbol,
        )))
    }
}

// Go: checker/nodebuilder.go:279 NewNodeBuilder
pub fn new_node_builder(ch: &Checker, e: Rc<EmitContext>) -> NodeBuilder {
    new_node_builder_ex(ch, e, None /*idToSymbol*/)
}

// Go: checker/nodebuilder.go:283 NewNodeBuilderEx
pub fn new_node_builder_ex(
    ch: &Checker,
    e: Rc<EmitContext>,
    id_to_symbol: Option<FxHashMap<Node, SymbolId>>,
) -> NodeBuilder {
    let impl_ = new_node_builder_impl(ch, e, id_to_symbol);
    NodeBuilder {
        impl_: Rc::new(RefCell::new(impl_)),
        ctx_stack: Vec::with_capacity(1),
        host: ch.program,
        verbosity: None,
    }
}
