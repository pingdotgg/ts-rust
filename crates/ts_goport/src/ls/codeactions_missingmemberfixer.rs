use crate::ls::prelude::*;

// Port of Go `ls/codeactions_missingmemberfixer.go`.
//
// PORT (whole file):
// - Go `missingMemberFixer` holds pointers to the change tracker, the
//   checker and the import adder, and its caller keeps using all three. The
//   Rust fixer borrows them (`&'a mut`); the caller reads them back through
//   the pub fields while the fixer lives.
// - Go `autoimport.ImportAdder` is an interface value that can be nil. The
//   caller owns an `Option<Box<dyn autoimport::ImportAdder>>` (w3 shape) and
//   the fixer borrows it as `Option<&mut dyn autoimport::ImportAdder>`.
// - Go `NewNodeBuilderEx` shares its `idToSymbol` map with the caller. The
//   Rust builder owns the map; the fixer reads it back from
//   `nb.impl_.borrow().id_to_symbol` after each build (PORTING "Programs and
//   checkers").
// - Go reads `*ast.Symbol` fields directly. Here they are read from the
//   checker's symbol arena (`type_checker.sym(s)`).

use crate::flags_macros::go_flags;

// Go: ls/codeactions_missingmemberfixer.go:18 preserveOptionalFlags
go_flags!(PreserveOptionalFlags, i32 {
    METHOD = 1 << 0; // preserveOptionalFlagsMethod
    PROPERTY = 1 << 1; // preserveOptionalFlagsProperty
    ALL = (1 << 0) | (1 << 1); // preserveOptionalFlagsAll
});

// Go: ls/codeactions_missingmemberfixer.go:26 missingMemberFixer
pub struct MissingMemberFixer<'a> {
    pub change_tracker: &'a mut change::Tracker,
    pub type_checker: &'a mut Checker,
    pub program: &'a compiler::NewProgram,
    pub preferences: lsutil::UserPreferences,
    pub import_adder: Option<&'a mut (dyn autoimport::ImportAdder + 'static)>,
    pub locale: locale::Locale,
}

// Go: ls/codeactions_missingmemberfixer.go:35 newMissingMemberFixer
pub fn new_missing_member_fixer<'a>(
    change_tracker: &'a mut change::Tracker,
    program: &'a compiler::NewProgram,
    type_checker: &'a mut Checker,
    preferences: lsutil::UserPreferences,
    import_adder: Option<&'a mut (dyn autoimport::ImportAdder + 'static)>,
    locale: locale::Locale,
) -> MissingMemberFixer<'a> {
    MissingMemberFixer {
        change_tracker,
        type_checker,
        program,
        preferences,
        import_adder,
        locale,
    }
}

/// The `idToSymbol` map that a node builder filled so far.
// PORT: Go shares the map between the builder and the caller. The Rust
// builder owns it, so this copies it out of `nb.impl_` (PORTING "Programs and
// checkers"). Not a Go function.
fn read_id_to_symbol(node_builder: &Rc<RefCell<NodeBuilder>>) -> FxHashMap<Node, SymbolId> {
    node_builder.borrow().impl_.borrow().id_to_symbol.clone()
}

impl<'a> MissingMemberFixer<'a> {
    // Go: ls/codeactions_missingmemberfixer.go:46 createNodeBuilder
    // PORT: Go returns `(nodeBuilder, idToSymbol)`. The builder owns the
    // map here; callers read it with `read_id_to_symbol` after each build.
    fn create_node_builder(&self) -> Rc<RefCell<NodeBuilder>> {
        let id_to_symbol: FxHashMap<Node, SymbolId> = FxHashMap::default();
        Rc::new(RefCell::new(new_node_builder_ex(
            &*self.type_checker,
            Rc::clone(&self.change_tracker.emit_context),
            Some(id_to_symbol),
        )))
    }

    // Go: ls/codeactions_missingmemberfixer.go:52 createMemberFromSymbol
    pub fn create_member_from_symbol(
        &mut self,
        symbol: SymbolId,
        enclosing_declaration: Node,
        source_file: Node,
        body: Node,
        preserve_optional: PreserveOptionalFlags,
        abstract_: bool,
    ) -> Vec<Node> {
        let declarations: Vec<Node> = self.type_checker.sym(symbol).declarations.to_vec();
        let declaration = declarations.first().copied().unwrap_or(Node::NIL);

        let quote_preference = lsutil::get_quote_preference(source_file, &self.preferences);
        let ambient = enclosing_declaration.flags().intersects(NodeFlags::AMBIENT);
        let signature_only = ambient || abstract_;
        let optional = self
            .type_checker
            .sym_flags(symbol)
            .intersects(SymbolFlags::OPTIONAL);
        let mut kind = SyntaxKind::PropertySignature;
        if declaration.is_some() {
            kind = declaration.kind();
        }
        let declaration_name = create_declaration_name(
            self.change_tracker.node_factory(),
            self.type_checker,
            symbol,
            declaration,
        );
        let modifiers = self.create_modifiers(symbol, declaration);

        let mut flags = NodeBuilderFlags::NO_TRUNCATION;
        if quote_preference == lsutil::QuotePreference::SINGLE {
            flags |= NodeBuilderFlags::USE_SINGLE_QUOTES_FOR_STRING_LITERAL_TYPE;
        }

        let type_of_symbol = self
            .type_checker
            .get_type_of_symbol_at_location(symbol, enclosing_declaration);
        let t = self.type_checker.get_widened_type_exported(type_of_symbol);
        let mut nodes: Vec<Node> = Vec::new();

        match kind {
            SyntaxKind::PropertySignature | SyntaxKind::PropertyDeclaration => {
                let node_builder = self.create_node_builder();
                let type_node =
                    self.create_type_node(t, enclosing_declaration, flags, &node_builder);
                let mut question_token = Node::NIL;
                if optional && preserve_optional.intersects(PreserveOptionalFlags::PROPERTY) {
                    question_token = self
                        .change_tracker
                        .node_factory()
                        .new_token(SyntaxKind::QuestionToken);
                }
                let factory = self.change_tracker.node_factory();
                nodes.push(factory.new_property_declaration(
                    modifiers,
                    create_property_name(factory, declaration_name, quote_preference),
                    question_token,
                    type_node,
                    Node::NIL, /*initializer*/
                ));
                nodes
            }

            SyntaxKind::GetAccessor | SyntaxKind::SetAccessor => {
                let node_builder = self.create_node_builder();
                let accessors = get_all_accessor_declarations(&declarations, declaration);
                let mut ordered_accessors: Vec<Node> = Vec::new();
                if accessors.second_accessor.is_nil() {
                    ordered_accessors.push(accessors.first_accessor);
                } else {
                    ordered_accessors.push(accessors.first_accessor);
                    ordered_accessors.push(accessors.second_accessor);
                }

                for accessor in ordered_accessors {
                    if is_get_accessor_declaration(accessor) {
                        // PORT: Go evaluates the call arguments left to
                        // right; the locals keep that order.
                        let name = create_property_name(
                            self.change_tracker.node_factory(),
                            declaration_name,
                            quote_preference,
                        );
                        let type_node =
                            self.create_type_node(t, enclosing_declaration, flags, &node_builder);
                        let accessor_body =
                            self.create_body(body, quote_preference, signature_only);
                        nodes.push(
                            self.change_tracker
                                .node_factory()
                                .new_get_accessor_declaration(
                                    modifiers,
                                    name,
                                    NodeList::NIL, /*typeParameters*/
                                    NodeList::NIL, /*parameters*/
                                    type_node,
                                    Node::NIL, /*fullSignature*/
                                    accessor_body,
                                ),
                        );
                    }

                    if is_set_accessor_declaration(accessor) {
                        let parameter = get_set_accessor_value_parameter(accessor);
                        if parameter.is_nil() {
                            crate::core::go_panic(
                                "Expected set accessor to have a parameter.".to_string(),
                            );
                        }

                        let name = create_property_name(
                            self.change_tracker.node_factory(),
                            declaration_name,
                            quote_preference,
                        );
                        let names = vec![parameter.name().text().to_string()];
                        let types = vec![self.create_type_node(
                            t,
                            enclosing_declaration,
                            flags,
                            &node_builder,
                        )];
                        let parameters = create_dummy_parameters(
                            self.change_tracker.node_factory(),
                            1,
                            &names,
                            &types,
                            1,
                            is_in_js_file(enclosing_declaration),
                        );
                        let accessor_body =
                            self.create_body(body, quote_preference, signature_only);
                        nodes.push(
                            self.change_tracker
                                .node_factory()
                                .new_set_accessor_declaration(
                                    modifiers,
                                    name,
                                    NodeList::NIL, /*typeParameters*/
                                    parameters,
                                    Node::NIL, /*type*/
                                    Node::NIL, /*fullSignature*/
                                    accessor_body,
                                ),
                        );
                    }
                }
                nodes
            }

            SyntaxKind::MethodSignature | SyntaxKind::MethodDeclaration => {
                let signatures = self.get_call_signatures(t);
                let preserve_optional =
                    optional && preserve_optional.intersects(PreserveOptionalFlags::METHOD);
                if signatures.is_empty() {
                    return Vec::new();
                }

                if declarations.len() == 1 {
                    let method_body = self.create_body(body, quote_preference, signature_only);
                    let method = self.create_signature_declaration_from_signature(
                        signatures.first().copied().unwrap_or(SignatureId::NIL),
                        SyntaxKind::MethodDeclaration,
                        source_file,
                        enclosing_declaration,
                        method_body,
                        modifiers,
                        declaration_name,
                        preserve_optional,
                    );
                    if method.is_some() {
                        nodes.push(method);
                    }
                    return nodes;
                }

                for &signature in &signatures {
                    let signature_declaration = self.type_checker.sig(signature).declaration();
                    if signature_declaration.is_some()
                        && signature_declaration.flags().intersects(NodeFlags::AMBIENT)
                    {
                        continue;
                    }

                    let method = self.create_signature_declaration_from_signature(
                        signature,
                        SyntaxKind::MethodDeclaration,
                        source_file,
                        enclosing_declaration,
                        Node::NIL, /*body*/
                        modifiers,
                        declaration_name,
                        preserve_optional,
                    );
                    if method.is_some() {
                        nodes.push(method);
                    }
                }

                if signature_only {
                    return nodes;
                }

                if declarations.len() > signatures.len() {
                    let signature = self.type_checker.get_signature_from_declaration_exported(
                        declarations.last().copied().unwrap_or(Node::NIL),
                    );
                    let method_body =
                        self.create_body(body, quote_preference, false /*signatureOnly*/);
                    let method = self.create_signature_declaration_from_signature(
                        signature,
                        SyntaxKind::MethodDeclaration,
                        source_file,
                        enclosing_declaration,
                        method_body,
                        modifiers,
                        declaration_name,
                        preserve_optional,
                    );
                    if method.is_some() {
                        nodes.push(method);
                    }
                } else {
                    let method = self.create_signature_declaration_from_signatures(
                        &signatures,
                        declaration_name,
                        preserve_optional,
                        modifiers,
                        quote_preference,
                        body,
                        enclosing_declaration,
                    );
                    if method.is_some() {
                        nodes.push(method);
                    }
                }

                nodes
            }
            _ => Vec::new(),
        }
    }

    // Go: ls/codeactions_missingmemberfixer.go:170 getCallSignatures
    fn get_call_signatures(&mut self, t: TypeId) -> Vec<SignatureId> {
        if self.type_checker.ty(t).is_union() {
            // PORT: Go `core.FlatMap` over `t.Types()`.
            let types = self.type_checker.ty(t).types().to_vec();
            let mut result: Vec<SignatureId> = Vec::new();
            for member in types {
                result.extend(self.type_checker.get_call_signatures(member));
            }
            return result;
        }
        self.type_checker.get_call_signatures(t)
    }

    // Go: ls/codeactions_missingmemberfixer.go:177 createTypeNode
    // PORT: Go also takes `idToSymbol`; it is read back from the builder
    // after the build.
    fn create_type_node(
        &mut self,
        t: TypeId,
        enclosing_declaration: Node,
        flags: NodeBuilderFlags,
        node_builder: &Rc<RefCell<NodeBuilder>>,
    ) -> Node {
        let type_node = self.type_checker.node_builder_type_to_type_node(
            node_builder,
            t,
            enclosing_declaration,
            flags,
            InternalNodeBuilderFlags::NONE,
            None, /*tracker*/
        );
        let id_to_symbol = read_id_to_symbol(node_builder);
        self.import_type_node(type_node, &id_to_symbol)
    }

    // Go: ls/codeactions_missingmemberfixer.go:181 createModifiers
    fn create_modifiers(&self, symbol: SymbolId, declaration: Node) -> ModifierList {
        let mut modifier_flags = ModifierFlags::NONE;
        if declaration.is_some() {
            let effective = self
                .type_checker
                .get_declaration_modifier_flags_from_symbol_exported(symbol);
            modifier_flags = effective & ModifierFlags::STATIC;
            if effective.intersects(ModifierFlags::PUBLIC) {
                modifier_flags |= ModifierFlags::PUBLIC;
            } else if effective.intersects(ModifierFlags::PROTECTED) {
                modifier_flags |= ModifierFlags::PROTECTED;
            }
            if is_auto_accessor_property_declaration(declaration) {
                modifier_flags |= ModifierFlags::ACCESSOR;
            }
        }
        if self.should_add_override_keyword(declaration) {
            modifier_flags |= ModifierFlags::OVERRIDE;
        }
        if modifier_flags == ModifierFlags::NONE {
            return ModifierList::NIL;
        }
        let factory = self.change_tracker.node_factory();
        let modifiers =
            create_modifiers_from_modifier_flags(modifier_flags, &mut |kind: SyntaxKind| {
                factory.new_modifier(kind)
            });
        factory.new_modifier_list(&modifiers)
    }

    // Go: ls/codeactions_missingmemberfixer.go:204 shouldAddOverrideKeyword
    fn should_add_override_keyword(&self, declaration: Node) -> bool {
        declaration.is_some()
            && self.program.options().no_implicit_override.is_true()
            && has_abstract_modifier(declaration)
    }

    // Go: ls/codeactions_missingmemberfixer.go:208 createSignatureDeclarationFromSignature
    fn create_signature_declaration_from_signature(
        &mut self,
        signature: SignatureId,
        kind: SyntaxKind,
        source_file: Node,
        enclosing_declaration: Node,
        body: Node,
        modifiers: ModifierList,
        name: Node,
        optional: bool,
    ) -> Node {
        let quote_preference = lsutil::get_quote_preference(source_file, &self.preferences);
        let mut flags = NodeBuilderFlags::NO_TRUNCATION
            | NodeBuilderFlags::SUPPRESS_ANY_RETURN_TYPE
            | NodeBuilderFlags::ALLOW_EMPTY_TUPLE;
        if quote_preference == lsutil::QuotePreference::SINGLE {
            flags |= NodeBuilderFlags::USE_SINGLE_QUOTES_FOR_STRING_LITERAL_TYPE;
        }

        let node_builder = self.create_node_builder();
        let signature_declaration = self
            .type_checker
            .node_builder_signature_to_signature_declaration(
                &node_builder,
                signature,
                kind,
                enclosing_declaration,
                flags,
                InternalNodeBuilderFlags::ALLOW_UNRESOLVED_NAMES,
                None, /*tracker*/
            );
        if signature_declaration.is_nil() {
            return Node::NIL;
        }
        // PORT: Go reads the shared `idToSymbol` map below; the builder is not
        // used again, so one read-back sees the same entries.
        let id_to_symbol = read_id_to_symbol(&node_builder);

        let is_js = is_in_js_file(enclosing_declaration);
        let mut parameters = signature_declaration.parameter_list();
        let mut type_parameters = if is_js {
            NodeList::NIL
        } else {
            signature_declaration.type_parameter_list()
        };
        let mut type_node = if is_js {
            Node::NIL
        } else {
            signature_declaration.type_()
        };

        if type_parameters.is_some() && type_parameters.nodes().len() > 0 {
            let mut nodes: Vec<Node> = Vec::with_capacity(type_parameters.nodes().len());
            for tp in type_parameters.nodes().iter() {
                if tp.is_nil() {
                    continue;
                }

                if is_type_parameter_declaration(tp) {
                    let type_parameter = tp;

                    let mut constraint = type_parameter.constraint();
                    if constraint.is_some() {
                        constraint = self.import_type_node(constraint, &id_to_symbol);
                    }

                    let mut default_type = type_parameter.default_type();
                    if default_type.is_some() {
                        default_type = self.import_type_node(default_type, &id_to_symbol);
                    }

                    nodes.push(
                        self.change_tracker
                            .node_factory()
                            .update_type_parameter_declaration(
                                type_parameter,
                                type_parameter.modifiers(),
                                type_parameter.name(),
                                constraint,
                                type_parameter.expression(),
                                default_type,
                            ),
                    );
                } else {
                    nodes.push(tp);
                }
            }
            type_parameters = self.change_tracker.node_factory().new_node_list(&nodes);
        }

        if parameters.is_some() {
            let mut nodes: Vec<Node> = Vec::with_capacity(parameters.nodes().len());
            for p in parameters.nodes().iter() {
                if p.is_nil() {
                    continue;
                }

                let parameter = p;
                let mut parameter_type_node = parameter.type_();
                if parameter_type_node.is_some() {
                    parameter_type_node = self.import_type_node(parameter_type_node, &id_to_symbol);
                }

                nodes.push(
                    self.change_tracker
                        .node_factory()
                        .update_parameter_declaration(
                            parameter,
                            parameter.modifiers(),
                            parameter.dot_dot_dot_token(),
                            parameter.name(),
                            if is_js {
                                Node::NIL
                            } else {
                                parameter.question_token()
                            },
                            parameter_type_node,
                            parameter.initializer(),
                        ),
                );
            }
            parameters = self.change_tracker.node_factory().new_node_list(&nodes);
        }

        if type_node.is_some() {
            type_node = self.import_type_node(type_node, &id_to_symbol);
        }

        let mut question_token = Node::NIL;
        if optional {
            question_token = self
                .change_tracker
                .node_factory()
                .new_token(SyntaxKind::QuestionToken);
        }

        let factory = self.change_tracker.node_factory();
        match kind {
            SyntaxKind::FunctionExpression => {
                let fn_ = signature_declaration;
                factory.update_function_expression(
                    fn_,
                    modifiers,
                    fn_.asterisk_token(),
                    if name.is_some() && is_identifier(name) {
                        name
                    } else {
                        Node::NIL
                    },
                    type_parameters,
                    parameters,
                    type_node,
                    fn_.full_signature(),
                    if body.is_some() { body } else { fn_.body() },
                )
            }

            SyntaxKind::ArrowFunction => {
                let fn_ = signature_declaration;
                factory.update_arrow_function(
                    fn_,
                    modifiers,
                    type_parameters,
                    parameters,
                    type_node,
                    fn_.full_signature(),
                    fn_.equals_greater_than_token(),
                    if body.is_some() { body } else { fn_.body() },
                )
            }

            SyntaxKind::MethodDeclaration => {
                let method = signature_declaration;
                // PORT: Go `core.IfElse` evaluates both arguments, so the
                // empty identifier is always made and `createPropertyName`
                // always runs.
                let empty_name = factory.new_identifier("");
                let property_name = create_property_name(factory, name, quote_preference);
                let method_name = if name.is_nil() {
                    empty_name
                } else {
                    property_name
                };
                factory.update_method_declaration(
                    method,
                    modifiers,
                    method.asterisk_token(),
                    method_name,
                    question_token,
                    type_parameters,
                    parameters,
                    type_node,
                    method.full_signature(),
                    body,
                )
            }

            SyntaxKind::FunctionDeclaration => {
                let fn_ = signature_declaration;
                factory.update_function_declaration(
                    fn_,
                    modifiers,
                    fn_.asterisk_token(),
                    if name.is_some() && is_identifier(name) {
                        name
                    } else {
                        Node::NIL
                    },
                    type_parameters,
                    parameters,
                    type_node,
                    fn_.full_signature(),
                    if body.is_some() { body } else { fn_.body() },
                )
            }

            _ => Node::NIL,
        }
    }

    // Go: ls/codeactions_missingmemberfixer.go:305 createSignatureDeclarationFromSignatures
    fn create_signature_declaration_from_signatures(
        &mut self,
        signatures: &[SignatureId],
        name: Node,
        optional: bool,
        modifiers: ModifierList,
        quote_preference: lsutil::QuotePreference,
        body: Node,
        enclosing_declaration: Node,
    ) -> Node {
        if signatures.is_empty() {
            return Node::NIL;
        }

        let node_builder = self.create_node_builder();
        let mut max_args_signature = signatures[0];
        let mut min_argument_count = self.type_checker.sig(signatures[0]).min_argument_count();

        let mut has_rest_parameter = false;
        for &signature in signatures {
            let sig = self.type_checker.sig(signature);
            let max_args = self.type_checker.sig(max_args_signature);
            min_argument_count = min_argument_count.min(sig.min_argument_count());
            if sig.has_rest_parameter() {
                has_rest_parameter = true;
            }
            if sig.parameters().len() >= max_args.parameters().len()
                && (!sig.has_rest_parameter() || max_args.has_rest_parameter())
            {
                max_args_signature = signature;
            }
        }

        let max_args = self.type_checker.sig(max_args_signature);
        let max_non_rest_args =
            max_args.parameters().len() as i32 - if max_args.has_rest_parameter() { 1 } else { 0 };
        let parameter_symbols: Vec<SymbolId> = max_args.parameters().to_vec();
        let mut parameter_names: Vec<String> = Vec::with_capacity(parameter_symbols.len());
        for symbol in parameter_symbols {
            parameter_names.push(self.type_checker.sym(symbol).name.as_str().to_string());
        }
        let mut parameters = create_dummy_parameters(
            self.change_tracker.node_factory(),
            max_non_rest_args,
            &parameter_names,
            &[], /*types*/
            min_argument_count,
            is_in_js_file(enclosing_declaration),
        );

        if has_rest_parameter {
            let mut rest_parameter_name = "rest".to_string();
            if (max_non_rest_args as usize) < parameter_names.len()
                && !parameter_names[max_non_rest_args as usize].is_empty()
            {
                rest_parameter_name = parameter_names[max_non_rest_args as usize].clone();
            }

            let factory = self.change_tracker.node_factory();
            let mut question_token = Node::NIL;
            if max_non_rest_args >= min_argument_count {
                question_token = factory.new_token(SyntaxKind::QuestionToken);
            }

            // PORT: Go appends to `parameters.Nodes` in place. A `NodeList`
            // handle is fixed once made, so a new list with the same nodes
            // replaces it. The list is not attached to any node yet.
            let mut parameter_nodes = parameters.nodes().to_vec();
            parameter_nodes.push(
                factory.new_parameter_declaration(
                    ModifierList::NIL, /*modifiers*/
                    factory.new_token(SyntaxKind::DotDotDotToken),
                    factory.new_identifier(rest_parameter_name),
                    question_token,
                    factory.new_array_type_node(
                        factory.new_keyword_type_node(SyntaxKind::UnknownKeyword),
                    ),
                    Node::NIL, /*initializer*/
                ),
            );
            parameters = factory.new_node_list(&parameter_nodes);
        }

        // PORT: Go `core.IfElse` evaluates both arguments, so the empty
        // identifier is always made and `createPropertyName` always runs.
        let empty_name = self.change_tracker.node_factory().new_identifier("");
        let property_name =
            create_property_name(self.change_tracker.node_factory(), name, quote_preference);
        let method_name = if name.is_nil() {
            empty_name
        } else {
            property_name
        };

        // PORT: Go `core.IfElse(optional, NewToken(..), nil)` makes the token
        // even when `optional` is false.
        let optional_token = self
            .change_tracker
            .node_factory()
            .new_token(SyntaxKind::QuestionToken);
        let question_token = if optional { optional_token } else { Node::NIL };
        let return_type =
            self.get_return_type_from_signatures(signatures, enclosing_declaration, &node_builder);
        let method_body = self.create_body(body, quote_preference, false /*signatureOnly*/);

        self.change_tracker.node_factory().new_method_declaration(
            modifiers,
            Node::NIL, /*asteriskToken*/
            method_name,
            question_token,
            NodeList::NIL, /*typeParameters*/
            parameters,
            return_type,
            Node::NIL, /*fullSignature*/
            method_body,
        )
    }

    // Go: ls/codeactions_missingmemberfixer.go:359 getReturnTypeFromSignatures
    // PORT: Go also takes `idToSymbol`; it is read back from the builder
    // after the build.
    fn get_return_type_from_signatures(
        &mut self,
        signatures: &[SignatureId],
        enclosing_declaration: Node,
        node_builder: &Rc<RefCell<NodeBuilder>>,
    ) -> Node {
        if signatures.is_empty() {
            return Node::NIL;
        }

        let mut return_types: Vec<TypeId> = Vec::with_capacity(signatures.len());
        for &signature in signatures {
            return_types.push(
                self.type_checker
                    .get_return_type_of_signature_exported(signature),
            );
        }

        let union_type = self.type_checker.get_union_type_exported(&return_types);
        let type_node = self.type_checker.node_builder_type_to_type_node(
            node_builder,
            union_type,
            enclosing_declaration,
            NodeBuilderFlags::NO_TRUNCATION,
            InternalNodeBuilderFlags::ALLOW_UNRESOLVED_NAMES,
            None, /*typeArguments*/
        );
        let id_to_symbol = read_id_to_symbol(node_builder);
        self.import_type_node(type_node, &id_to_symbol)
    }

    // Go: ls/codeactions_missingmemberfixer.go:373 importTypeNode
    fn import_type_node(
        &mut self,
        type_node: Node,
        id_to_symbol: &FxHashMap<Node, SymbolId>,
    ) -> Node {
        if type_node.is_nil() || self.import_adder.is_none() {
            return type_node;
        }
        let Some(import_adder) = self.import_adder.as_deref_mut() else {
            return type_node;
        };

        let (imported_type_node, symbols) =
            autoimport::try_get_auto_importable_reference_from_type_node(
                &self.type_checker.symbols,
                type_node,
                id_to_symbol,
            );
        if imported_type_node.is_some() {
            for symbol in symbols {
                let export_symbol = Self::get_exported_symbol(self.type_checker, symbol);
                if export_symbol.is_nil() {
                    continue;
                }
                import_adder.add_import_from_exported_symbol(
                    self.type_checker,
                    export_symbol,
                    true, /*isValidTypeOnlyUseSite*/
                );
            }
            return imported_type_node;
        }

        let mut seen: FxHashSet<SymbolId> = FxHashSet::default();
        // PORT: Go map order is random. This walks the builder's
        // `FxHashMap` in its own order; the import adder sorts its edits.
        for &symbol in id_to_symbol.values() {
            if symbol.is_nil() || seen.contains(&symbol) {
                continue;
            }
            seen.insert(symbol);
            let export_symbol = Self::get_exported_symbol(self.type_checker, symbol);
            if export_symbol.is_nil() {
                continue;
            }
            import_adder.add_import_from_exported_symbol(
                self.type_checker,
                export_symbol,
                true, /*isValidTypeOnlyUseSite*/
            );
        }
        type_node
    }

    // Go: ls/codeactions_missingmemberfixer.go:405 getExportedSymbol
    // PORT: a Go method on the fixer that reads only `f.typeChecker`. It
    // takes the checker, so `importTypeNode` can call it while it holds the
    // import adder.
    fn get_exported_symbol(type_checker: &Checker, symbol: SymbolId) -> SymbolId {
        let symbol = type_checker.get_export_symbol_of_symbol(symbol);
        if symbol.is_nil() || type_checker.sym(symbol).parent.is_nil() {
            return SymbolId::NIL;
        }
        symbol
    }

    // Go: ls/codeactions_missingmemberfixer.go:413 createIndexSignatureDeclarationFromType
    pub fn create_index_signature_declaration_from_type(
        &mut self,
        class_declaration: Node,
        implemented_type: TypeId,
        key_type: TypeId,
    ) -> Node {
        let index_info = self
            .type_checker
            .get_index_info_of_type_exported(implemented_type, key_type);
        if index_info.is_nil() {
            return Node::NIL;
        }

        let builder = Rc::new(RefCell::new(new_node_builder(
            &*self.type_checker,
            Rc::clone(&self.change_tracker.emit_context),
        )));
        self.type_checker
            .node_builder_index_info_to_index_signature_declaration(
                &builder,
                index_info,
                class_declaration,
                NodeBuilderFlags::NONE,
                InternalNodeBuilderFlags::NONE,
                None,
            )
    }

    // Go: ls/codeactions_missingmemberfixer.go:423 createBody
    fn create_body(
        &self,
        body: Node,
        quote_preference: lsutil::QuotePreference,
        signature_only: bool,
    ) -> Node {
        if signature_only {
            return Node::NIL;
        }
        let body = self.change_tracker.node_factory().deep_clone_node(body);
        if body.is_nil() {
            return self.create_stubbed_method_body(quote_preference);
        }
        body
    }

    // Go: ls/codeactions_missingmemberfixer.go:434 createStubbedMethodBody
    fn create_stubbed_method_body(&self, quote_preference: lsutil::QuotePreference) -> Node {
        let mut token_flags = TokenFlags::NONE;
        if quote_preference == lsutil::QuotePreference::SINGLE {
            token_flags = TokenFlags::SINGLE_QUOTE;
        }

        let factory = self.change_tracker.node_factory();
        factory.new_block(
            factory.new_node_list(&[factory.new_throw_statement(factory.new_new_expression(
                factory.new_identifier("Error"),
                NodeList::NIL, /*typeArguments*/
                factory.new_node_list(&[factory.new_string_literal(
                    crate::diagnostics_loc::message_localize(
                        diag::Method_not_implemented,
                        &self.locale,
                        &args![],
                    ),
                    token_flags,
                )]),
            ))]),
            true, /*multiLine*/
        )
    }
}

// Go: ls/codeactions_missingmemberfixer.go:451 createDummyParameters
fn create_dummy_parameters(
    factory: &NodeFactory,
    arg_count: i32,
    names: &[String],
    types: &[Node],
    min_argument_count: i32,
    in_js: bool,
) -> NodeList {
    let mut parameters: Vec<Node> = Vec::new();
    let mut parameter_name_counts: FxHashMap<String, i32> = FxHashMap::default();

    for i in 0..arg_count {
        let mut parameter_name: String;
        if (i as usize) < names.len() && !names[i as usize].is_empty() {
            parameter_name = names[i as usize].clone();
        } else {
            parameter_name = format!("arg{i}");
        }

        let count = parameter_name_counts
            .get(&parameter_name)
            .copied()
            .unwrap_or(0);
        parameter_name_counts.insert(parameter_name.clone(), count + 1);

        if count > 0 {
            parameter_name += &count.to_string();
        }

        let mut question_token = Node::NIL;
        if i >= min_argument_count {
            question_token = factory.new_token(SyntaxKind::QuestionToken);
        }

        let type_node: Node;
        if in_js {
            type_node = Node::NIL;
        } else if (i as usize) < types.len() && types[i as usize].is_some() {
            type_node = types[i as usize];
        } else {
            type_node = factory.new_keyword_type_node(SyntaxKind::UnknownKeyword);
        }
        parameters.push(factory.new_parameter_declaration(
            ModifierList::NIL, /*modifiers*/
            Node::NIL,         /*dotDotDotToken*/
            factory.new_identifier(parameter_name),
            question_token,
            type_node,
            Node::NIL, /*initializer*/
        ));
    }
    factory.new_node_list(&parameters)
}

// Go: ls/codeactions_missingmemberfixer.go:489 createDeclarationName
fn create_declaration_name(
    factory: &NodeFactory,
    type_checker: &mut Checker,
    symbol: SymbolId,
    declaration: Node,
) -> Node {
    if symbol.is_some()
        && type_checker
            .sym_check_flags(symbol)
            .intersects(CheckFlags::MAPPED)
    {
        let name_type = type_checker.get_name_type_of_symbol(symbol);
        if name_type.is_some() && type_checker.is_type_usable_as_property_name_exported(name_type) {
            return factory
                .new_identifier(type_checker.get_property_name_from_type_exported(name_type));
        }
    }
    if declaration.is_some() && declaration.name().is_some() {
        return factory.clone_node(declaration.name());
    }
    if symbol.is_some() {
        return factory.new_identifier(type_checker.sym(symbol).name.as_str());
    }
    Node::NIL
}

// Go: ls/codeactions_missingmemberfixer.go:505 createPropertyName
fn create_property_name(
    factory: &NodeFactory,
    node: Node,
    quote_preference: lsutil::QuotePreference,
) -> Node {
    if is_identifier(node) && node.text() == "constructor" {
        let mut token_flags = TokenFlags::NONE;
        if quote_preference == lsutil::QuotePreference::SINGLE {
            token_flags = TokenFlags::SINGLE_QUOTE;
        }
        return factory
            .new_computed_property_name(factory.new_string_literal(node.text(), token_flags));
    }
    factory.deep_clone_node(node)
}
