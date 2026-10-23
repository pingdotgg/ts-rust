//! Port of checker/nodebuilder_hover.go: declaration nodes for expandable hover.
//!
//! Every NodeBuilderImpl method uses the Relater pattern: an `impl Checker`
//! method that takes `b: &Rc<RefCell<NodeBuilderImpl>>`. No `RefCell` borrow is
//! held across a Checker call.

use crate::prelude::*;

// Go: checker/nodebuilder_hover.go:16 isExpanding
pub fn is_expanding(ctx: &NodeBuilderContext) -> bool {
    ctx.max_expansion_depth != -1
}

// PORT: Go repeats `b.f.NewModifierList(ast.CreateModifiersFromModifierFlags(flags, b.f.NewModifier))`
// inline. This helper is that expression.
fn modifier_list_from_flags(f: &NodeFactory, flags: ModifierFlags) -> ModifierList {
    let modifiers = create_modifiers_from_modifier_flags(flags, &mut |kind| f.new_modifier(kind));
    f.new_modifier_list(&modifiers)
}

// PORT: Go reads `b.ctx` directly. The context is its own RefCell here, so
// clone the handle out and drop the builder borrow before any Checker call.
fn hv_ctx(b: &Rc<RefCell<NodeBuilderImpl>>) -> Rc<RefCell<NodeBuilderContext>> {
    b.borrow().ctx.clone()
}

impl Checker {
    // Go: checker/nodebuilder_hover.go:25 expandSymbolForHover
    pub fn expand_symbol_for_hover(
        &mut self,
        b: &Rc<RefCell<NodeBuilderImpl>>,
        symbol: SymbolId,
    ) -> Vec<Node> {
        let mut results = Vec::new();
        let flags = self.sym_flags(symbol);
        if flags.intersects(SymbolFlags::ENUM) {
            let node = self.expand_enum_decl(b, symbol);
            if node.is_some() {
                results.push(node);
            }
        }
        if flags.intersects(SymbolFlags::CLASS) {
            let node = self.expand_class_decl(b, symbol);
            if node.is_some() {
                results.push(node);
            }
        }
        // Module/namespace before interface (matching Strada ordering for merged declarations)
        if flags.intersects(SymbolFlags::VALUE_MODULE | SymbolFlags::NAMESPACE_MODULE) {
            let node = self.expand_module_decl(b, symbol);
            if node.is_some() {
                results.push(node);
            }
        }
        if flags.intersects(SymbolFlags::INTERFACE) && !flags.intersects(SymbolFlags::CLASS) {
            let node = self.expand_interface_decl(b, symbol);
            if node.is_some() {
                results.push(node);
            }
        }
        results
    }

    // Go: checker/nodebuilder_hover.go:52 expandEnumDecl
    pub(crate) fn expand_enum_decl(
        &mut self,
        b: &Rc<RefCell<NodeBuilderImpl>>,
        symbol: SymbolId,
    ) -> Node {
        let name = symbol_name(&self.symbols, symbol);
        hv_ctx(b).borrow_mut().approximate_length += 9 + go_len(&name) as i32;
        let symbol_type = self.get_type_of_symbol(symbol);
        let member_props: Vec<SymbolId> = self
            .get_properties_of_type(symbol_type)
            .into_iter()
            .filter(|&p| self.sym_flags(p).intersects(SymbolFlags::ENUM_MEMBER))
            .collect();
        let mut members: Vec<Node> = Vec::new();
        for (i, &p) in member_props.iter().enumerate() {
            if self.check_truncation_length_if_expanding(b) && i + 3 < member_props.len() - 1 {
                hv_ctx(b).borrow_mut().expansion_truncated = true;
                let text = format!(" ... {} more ... ", member_props.len() - i - 1);
                let last = member_props[member_props.len() - 1];
                let last_initializer = self.enum_member_initializer(b, last);
                let last_name = self.sym(last).name.clone();
                let bb = b.borrow();
                members.push(
                    bb.f().new_enum_member(
                        bb.f().new_string_literal(text, TokenFlags::NONE),
                        Node::NIL,
                    ),
                );
                members.push(
                    bb.f()
                        .new_enum_member(bb.f().new_identifier(last_name), last_initializer),
                );
                break;
            }
            let member_decl = self
                .sym(p)
                .declarations
                .iter()
                .copied()
                .find(|&d| is_enum_member(d))
                .unwrap_or(Node::NIL);
            let initializer = if member_decl.is_some() && member_decl.initializer().is_some() {
                b.borrow().f().deep_clone_node(member_decl.initializer())
            } else {
                self.enum_member_initializer(b, p)
            };
            let p_name = self.sym(p).name.clone();
            let bb = b.borrow();
            {
                let mut c = bb.ctx.borrow_mut();
                c.approximate_length += 4 + go_len(&p_name) as i32;
                if initializer.is_some() {
                    c.approximate_length += 5; // " = " + value estimate
                }
            }
            let member = bb
                .f()
                .new_enum_member(bb.f().new_identifier(p_name), initializer);
            members.push(member);
        }

        let mut const_modifier = ModifierFlags::NONE;
        if self.is_const_enum_symbol(symbol) {
            const_modifier = ModifierFlags::CONST;
        }
        let bb = b.borrow();
        let mut mods = ModifierList::NIL;
        if const_modifier != ModifierFlags::NONE {
            mods = modifier_list_from_flags(bb.f(), const_modifier);
        }
        bb.f().new_enum_declaration(
            mods,
            bb.f().new_identifier(name),
            bb.f().new_node_list(&members),
        )
    }

    // Go: checker/nodebuilder_hover.go:92 enumMemberInitializer
    pub(crate) fn enum_member_initializer(
        &mut self,
        b: &Rc<RefCell<NodeBuilderImpl>>,
        p: SymbolId,
    ) -> Node {
        let member_decl = self
            .sym(p)
            .declarations
            .iter()
            .copied()
            .find(|&d| is_enum_member(d))
            .unwrap_or(Node::NIL);
        if member_decl.is_nil() {
            return Node::NIL;
        }
        // PORT: Go `any` result is `Option<LiteralValue>`; `None` is Go nil.
        let val = self.get_constant_value(member_decl);
        let bb = b.borrow();
        match val {
            None => Node::NIL,
            Some(LiteralValue::String(v)) => bb.f().new_string_literal(v, TokenFlags::NONE),
            Some(LiteralValue::Number(v)) => {
                bb.f().new_numeric_literal(v.to_string(), TokenFlags::NONE)
            }
            Some(_) => Node::NIL,
        }
    }

    // Go: checker/nodebuilder_hover.go:111 expandClassDecl
    pub(crate) fn expand_class_decl(
        &mut self,
        b: &Rc<RefCell<NodeBuilderImpl>>,
        symbol: SymbolId,
    ) -> Node {
        let name = symbol_name(&self.symbols, symbol);
        hv_ctx(b).borrow_mut().approximate_length += 9 + go_len(&name) as i32;

        let class_like_declarations: Vec<Node> = self
            .sym(symbol)
            .declarations
            .iter()
            .copied()
            .filter(|&d| is_class_like(d))
            .collect();
        let original_decl = class_like_declarations
            .first()
            .copied()
            .unwrap_or(Node::NIL);
        let old_enclosing = hv_ctx(b).borrow().enclosing_declaration;
        if original_decl.is_some() {
            hv_ctx(b).borrow_mut().enclosing_declaration = original_decl;
        }
        // PORT: the Go deferred restore of enclosingDeclaration runs before the
        // single return below.

        let local_params =
            self.get_local_type_parameters_of_class_or_interface_or_type_alias(symbol);
        let type_param_decls: Vec<Node> = local_params
            .iter()
            .map(|&p| self.type_parameter_to_declaration(b, p))
            .collect();

        let declared_type = self.get_declared_type_of_class_or_interface(symbol);
        let class_type = self.get_type_with_this_argument(declared_type, TypeId::NIL, false);
        let target_type = self.get_target_type(class_type);
        let base_types = self.get_base_types(target_type);
        let static_type = self.get_type_of_symbol(symbol);
        let static_symbol = self.ty(static_type).symbol;
        let is_class = static_symbol.is_some()
            && self.sym(static_symbol).value_declaration.is_some()
            && is_class_like(self.sym(static_symbol).value_declaration);
        let static_base_type = if is_class {
            self.get_base_constructor_type_of_class(declared_type)
        } else {
            self.any_type
        };

        // Heritage clauses
        let heritage_clauses = self.hover_heritage_clauses(b, &class_like_declarations);

        // Instance members via addPropertyToElementList (reusing existing serialization),
        // then convert TypeElements to ClassElements and add class-specific modifiers
        let all_props = self.get_properties_of_type(class_type);
        let symbol_props =
            self.filter_inherited_properties(class_type, &base_types, all_props.to_vec());
        let public_props: Vec<SymbolId> = symbol_props
            .iter()
            .copied()
            .filter(|&s| !is_hash_private(&self.symbols, s))
            .collect();
        let has_private = symbol_props
            .iter()
            .any(|&s| is_hash_private(&self.symbols, s));

        let mut instance_members =
            self.serialize_properties_with_truncation(b, public_props, Vec::new());
        instance_members = type_elements_to_class_elements(b.borrow().f(), instance_members);
        instance_members = self.add_class_modifiers(b, instance_members, false);

        // Static members
        let static_props: Vec<SymbolId> = self
            .get_properties_of_type(static_type)
            .into_iter()
            .filter(|&p| {
                !self.sym_flags(p).intersects(SymbolFlags::PROTOTYPE)
                    && self.sym(p).name != "prototype"
                    && !self.is_namespace_member(p)
            })
            .collect();
        let mut static_members =
            self.serialize_properties_with_truncation(b, static_props, Vec::new());
        static_members = type_elements_to_class_elements(b.borrow().f(), static_members);
        static_members = self.add_class_modifiers(b, static_members, true);

        // Hash-private members
        let mut private_members = Vec::new();
        if has_private {
            let private_props: Vec<SymbolId> = symbol_props
                .iter()
                .copied()
                .filter(|&s| is_hash_private(&self.symbols, s))
                .collect();
            private_members =
                self.serialize_properties_with_truncation(b, private_props, private_members);
            private_members = type_elements_to_class_elements(b.borrow().f(), private_members);
        }

        // Constructors
        let constructors =
            self.serialize_constructors(b, static_type, static_base_type, is_class, symbol);

        // Index signatures
        let first_base = base_types.first().copied().unwrap_or(TypeId::NIL);
        let index_sigs = self.serialize_index_signatures_of_type(b, class_type, first_base);

        let mut all_members: Vec<Node> = Vec::with_capacity(
            index_sigs.len()
                + static_members.len()
                + constructors.len()
                + instance_members.len()
                + private_members.len(),
        );
        all_members.extend(index_sigs);
        all_members.extend(static_members);
        all_members.extend(constructors);
        all_members.extend(instance_members);
        all_members.extend(private_members);

        let bb = b.borrow();
        bb.ctx.borrow_mut().enclosing_declaration = old_enclosing;
        bb.f().new_class_declaration(
            ModifierList::NIL,
            bb.f().new_identifier(name),
            bb.f().new_node_list(&type_param_decls),
            bb.f().new_node_list(&heritage_clauses),
            bb.f().new_node_list(&all_members),
        )
    }

    // Go: checker/nodebuilder_hover.go:187 addClassModifiers
    pub(crate) fn add_class_modifiers(
        &mut self,
        b: &Rc<RefCell<NodeBuilderImpl>>,
        mut members: Vec<Node>,
        is_static: bool,
    ) -> Vec<Node> {
        for i in 0..members.len() {
            let m = members[i];
            // Find the symbol for this member by matching the property name
            let mut member_symbol = SymbolId::NIL;
            let member_name = m.name();
            if member_name.is_some() {
                if let Some(&sym) = b.borrow().id_to_symbol.get(&member_name) {
                    member_symbol = sym;
                }
            }
            if member_symbol.is_nil() {
                continue;
            }
            let mut mod_flags = self
                .get_declaration_modifier_flags_from_symbol(member_symbol)
                .without(ModifierFlags::ASYNC);
            if is_static {
                mod_flags = mod_flags | ModifierFlags::STATIC;
            }
            if mod_flags != ModifierFlags::NONE && can_have_modifiers(m) {
                let existing = m.modifier_flags();
                if mod_flags != existing {
                    let bb = b.borrow();
                    let mods = modifier_list_from_flags(bb.f(), mod_flags | existing);
                    members[i] = replace_modifiers(bb.f(), m, mods);
                }
            }
        }
        members
    }

    // Go: checker/nodebuilder_hover.go:234 expandInterfaceDecl
    pub(crate) fn expand_interface_decl(
        &mut self,
        b: &Rc<RefCell<NodeBuilderImpl>>,
        symbol: SymbolId,
    ) -> Node {
        let name = symbol_name(&self.symbols, symbol);
        hv_ctx(b).borrow_mut().approximate_length += 14 + go_len(&name) as i32;

        let interface_type = self.get_declared_type_of_class_or_interface(symbol);
        let interface_declarations: Vec<Node> = self
            .sym(symbol)
            .declarations
            .iter()
            .copied()
            .filter(|&d| is_interface_declaration(d))
            .collect();
        let local_params =
            self.get_local_type_parameters_of_class_or_interface_or_type_alias(symbol);
        let type_param_decls: Vec<Node> = local_params
            .iter()
            .map(|&p| self.type_parameter_to_declaration(b, p))
            .collect();
        let base_types = self.get_base_types(interface_type);
        let mut base_type = TypeId::NIL;
        if !base_types.is_empty() {
            base_type = self.get_intersection_type(&base_types);
        }

        // Members: reuse existing serialization functions
        let resolved = self.resolve_structured_type_members(interface_type);
        let construct_signatures = resolved.construct_signatures().to_vec();
        let call_signatures = resolved.call_signatures().to_vec();
        let resolved_properties = resolved.properties.clone();
        let mut members: Vec<Node> = Vec::new();

        // Index signatures, filtering those identical to base
        members.extend(self.serialize_index_signatures_of_type(b, interface_type, base_type));
        // Construct signatures (skip abstract)
        for sig in construct_signatures {
            if self.sig(sig).flags.intersects(SignatureFlags::ABSTRACT) {
                continue;
            }
            members.push(self.signature_to_signature_declaration_helper(
                b,
                sig,
                SyntaxKind::ConstructSignature,
                None,
            ));
        }
        // Call signatures
        for sig in call_signatures {
            members.push(self.signature_to_signature_declaration_helper(
                b,
                sig,
                SyntaxKind::CallSignature,
                None,
            ));
        }
        // Properties, filtering inherited
        let filtered_props = self.filter_inherited_properties(
            interface_type,
            &base_types,
            resolved_properties.to_vec(),
        );
        members = self.serialize_properties_with_truncation(b, filtered_props, members);

        // Heritage clauses
        let heritage_clauses = self.hover_heritage_clauses(b, &interface_declarations);

        let bb = b.borrow();
        bb.f().new_interface_declaration(
            ModifierList::NIL,
            bb.f().new_identifier(name),
            bb.f().new_node_list(&type_param_decls),
            bb.f().new_node_list(&heritage_clauses),
            bb.f().new_node_list(&members),
        )
    }

    // Go: checker/nodebuilder_hover.go:275 hoverHeritageClauses
    // PORT: no Checker call is needed, so `&self` is enough.
    pub(crate) fn hover_heritage_clauses(
        &self,
        b: &Rc<RefCell<NodeBuilderImpl>>,
        declarations: &[Node],
    ) -> Vec<Node> {
        let bb = b.borrow();
        let mut extends_types: Vec<Node> = Vec::new();
        let mut implements_types: Vec<Node> = Vec::new();
        for &declaration in declarations {
            for heritage_element in get_extends_heritage_clause_elements(declaration) {
                extends_types.push(bb.f().deep_clone_node(heritage_element));
            }
            for heritage_element in get_implements_heritage_clause_elements(declaration) {
                implements_types.push(bb.f().deep_clone_node(heritage_element));
            }
        }

        let mut heritage_clauses = Vec::new();
        if !extends_types.is_empty() {
            heritage_clauses.push(bb.f().new_heritage_clause(
                SyntaxKind::ExtendsKeyword,
                bb.f().new_node_list(&extends_types),
            ));
        }
        if !implements_types.is_empty() {
            heritage_clauses.push(bb.f().new_heritage_clause(
                SyntaxKind::ImplementsKeyword,
                bb.f().new_node_list(&implements_types),
            ));
        }
        heritage_clauses
    }

    // Go: checker/nodebuilder_hover.go:299 serializePropertiesWithTruncation
    pub(crate) fn serialize_properties_with_truncation(
        &mut self,
        b: &Rc<RefCell<NodeBuilderImpl>>,
        properties: Vec<SymbolId>,
        mut elements: Vec<Node>,
    ) -> Vec<Node> {
        let properties: Vec<SymbolId> = properties
            .into_iter()
            .filter(|&p| !self.sym_flags(p).intersects(SymbolFlags::PROTOTYPE))
            .collect();
        for (i, &p) in properties.iter().enumerate() {
            if self.check_truncation_length_if_expanding(b) && i + 3 < properties.len() - 1 {
                hv_ctx(b).borrow_mut().expansion_truncated = true;
                let text = format!("... {} more ...", properties.len() - i - 1);
                let element = {
                    let bb = b.borrow();
                    bb.f().new_property_signature_declaration(
                        ModifierList::NIL,
                        bb.f().new_identifier(text),
                        Node::NIL,
                        Node::NIL,
                        Node::NIL,
                    )
                };
                elements.push(element);
                elements = self.add_property_to_element_list(
                    b,
                    properties[properties.len() - 1],
                    elements,
                );
                break;
            }
            elements = self.add_property_to_element_list(b, p, elements);
        }
        elements
    }

    // Go: checker/nodebuilder_hover.go:317 serializeConstructors
    pub(crate) fn serialize_constructors(
        &mut self,
        b: &Rc<RefCell<NodeBuilderImpl>>,
        static_type: TypeId,
        static_base_type: TypeId,
        is_class: bool,
        symbol: SymbolId,
    ) -> Vec<Node> {
        let value_declaration = self.sym(symbol).value_declaration;
        let is_non_constructable = !is_class
            && value_declaration.is_some()
            && is_in_js_file(value_declaration)
            && self
                .get_signatures_of_type(static_type, SignatureKind::CONSTRUCT)
                .is_empty();
        if is_non_constructable {
            let bb = b.borrow();
            bb.ctx.borrow_mut().approximate_length += 21;
            let modifiers = modifier_list_from_flags(bb.f(), ModifierFlags::PRIVATE);
            return vec![bb.f().new_constructor_declaration(
                modifiers,
                NodeList::NIL,
                bb.f().new_node_list(&[]),
                Node::NIL,
                Node::NIL,
                Node::NIL,
            )];
        }
        let signatures = self.get_signatures_of_type(static_type, SignatureKind::CONSTRUCT);
        if static_base_type.is_some() {
            let base_sigs = self.get_signatures_of_type(static_base_type, SignatureKind::CONSTRUCT);
            if base_sigs.is_empty()
                && signatures
                    .iter()
                    .all(|&sig| self.sig(sig).parameters.is_empty())
            {
                return Vec::new();
            }
            if base_sigs.len() == signatures.len() {
                let mut all_match = true;
                for i in 0..base_sigs.len() {
                    if self.compare_signatures_identical(
                        signatures[i],
                        base_sigs[i],
                        false,
                        false,
                        true,
                        &mut |c: &mut Checker, s, t| c.compare_types_identical(s, t),
                    ) != Ternary::TRUE
                    {
                        all_match = false;
                        break;
                    }
                }
                if all_match {
                    return Vec::new();
                }
            }
            let mut private_protected = ModifierFlags::NONE;
            for &sig in &signatures {
                let declaration = self.sig(sig).declaration;
                if declaration.is_some() {
                    private_protected = private_protected
                        | (declaration.modifier_flags()
                            & (ModifierFlags::PRIVATE | ModifierFlags::PROTECTED));
                }
            }
            if private_protected != ModifierFlags::NONE {
                let bb = b.borrow();
                return vec![bb.f().new_constructor_declaration(
                    modifier_list_from_flags(bb.f(), private_protected),
                    NodeList::NIL,
                    bb.f().new_node_list(&[]),
                    Node::NIL,
                    Node::NIL,
                    Node::NIL,
                )];
            }
        } else if signatures
            .iter()
            .all(|&sig| self.sig(sig).parameters.is_empty())
        {
            return Vec::new();
        }
        let mut result = Vec::new();
        for sig in signatures {
            hv_ctx(b).borrow_mut().approximate_length += 1;
            result.push(self.signature_to_signature_declaration_helper(
                b,
                sig,
                SyntaxKind::Constructor,
                None,
            ));
        }
        result
    }

    // Go: checker/nodebuilder_hover.go:369 serializeIndexSignaturesOfType
    pub(crate) fn serialize_index_signatures_of_type(
        &mut self,
        b: &Rc<RefCell<NodeBuilderImpl>>,
        input: TypeId,
        base_type: TypeId,
    ) -> Vec<Node> {
        let mut result = Vec::new();
        for info in self.get_index_infos_of_type(input) {
            if base_type.is_some() {
                let key_type = self.index_info(info).key_type;
                let base_info = self.get_index_info_of_type(base_type, key_type);
                if base_info.is_some() {
                    let value_type = self.index_info(info).value_type;
                    let base_value_type = self.index_info(base_info).value_type;
                    if self.is_type_identical_to(value_type, base_value_type) {
                        continue;
                    }
                }
            }
            result.push(self.index_info_to_index_signature_declaration_helper(b, info, Node::NIL));
        }
        result
    }

    // Go: checker/nodebuilder_hover.go:385 serializeNamespaceMember
    pub(crate) fn serialize_namespace_member(
        &mut self,
        b: &Rc<RefCell<NodeBuilderImpl>>,
        resolved: SymbolId,
        name: &str,
    ) -> Node {
        let flags = self.sym_flags(resolved);
        if flags.intersects(SymbolFlags::TYPE_ALIAS) {
            self.serialize_type_alias_for_namespace(b, resolved, name)
        } else if flags.intersects(SymbolFlags::ENUM) {
            self.expand_enum_decl(b, resolved)
        } else if flags.intersects(SymbolFlags::CLASS) {
            self.expand_class_decl(b, resolved)
        } else if flags.intersects(SymbolFlags::INTERFACE) {
            self.expand_interface_decl(b, resolved)
        } else if flags.intersects(SymbolFlags::VALUE_MODULE | SymbolFlags::NAMESPACE_MODULE) {
            self.expand_module_decl(b, resolved)
        } else {
            let symbol_type = self.get_type_of_symbol(resolved);
            let t = self.get_widened_type(symbol_type);
            hv_ctx(b).borrow_mut().approximate_length += go_len(&name) as i32 + 5;
            let type_node = self.serialize_type_for_declaration(b, Node::NIL, t, resolved, true);
            let bb = b.borrow();
            let declaration = bb.f().new_variable_declaration(
                bb.f().new_identifier(name),
                Node::NIL,
                type_node,
                Node::NIL,
            );
            bb.f().new_variable_statement(
                ModifierList::NIL,
                bb.f().new_variable_declaration_list(
                    bb.f().new_node_list(&[declaration]),
                    NodeFlags::LET,
                ),
            )
        }
    }

    // Go: checker/nodebuilder_hover.go:413 expandModuleDecl
    pub(crate) fn expand_module_decl(
        &mut self,
        b: &Rc<RefCell<NodeBuilderImpl>>,
        symbol: SymbolId,
    ) -> Node {
        let exports = self.get_exports_of_symbol(symbol);
        let mut members: Vec<SymbolId> = Vec::new();
        for sym in self.symbols.values(exports) {
            // Filter to namespace-relevant members
            if !self.is_namespace_member(sym) {
                continue;
            }
            if !is_identifier_text(&self.sym(sym).name, LanguageVariant::STANDARD) {
                continue;
            }
            members.push(sym);
        }
        self.sort_symbols(&mut members);
        hv_ctx(b).borrow_mut().approximate_length += 14;

        // Use the same name as symbol display.
        let old_flags = hv_ctx(b).borrow().flags;
        // PORT: Go converts the SymbolFormatFlags value to nodebuilder.Flags by
        // number, so the same numeric conversion is kept here.
        hv_ctx(b).borrow_mut().flags = old_flags
            | NodeBuilderFlags::WRITE_TYPE_PARAMETERS_IN_QUALIFIED_NAME
            | NodeBuilderFlags(SymbolFormatFlags::USE_ONLY_EXTERNAL_ALIASING.0);
        let local_name = self.symbol_to_node(b, symbol, SymbolFlags::ALL);
        // PORT: the Go deferred restore and the explicit restore set the same
        // value, so one restore is enough.
        hv_ctx(b).borrow_mut().flags = old_flags;

        struct HoverStatement {
            node: Node,
            is_local: bool, // local declarations (e.g. alias targets) should not get export modifier
        }
        let mut body_stmts: Vec<HoverStatement> = Vec::new();
        let mut emitted_locals: FxHashSet<SymbolId> = FxHashSet::default();
        // PORT: Go uses `for i := 0; i < len; i++` and sets `i = len - 2` to
        // skip. `next` holds the index of the following iteration.
        let mut next = 0usize;
        while next < members.len() {
            let i = next;
            next += 1;
            let m = members[i];
            let m_name = self.sym(m).name.clone();
            if self.check_truncation_length_if_expanding(b) && i + 3 < members.len() - 1 {
                hv_ctx(b).borrow_mut().expansion_truncated = true;
                let text = format!("... ({} more) ...", members.len() - i - 1);
                let node = {
                    let bb = b.borrow();
                    bb.f().new_expression_statement(bb.f().new_identifier(text))
                };
                body_stmts.push(HoverStatement {
                    node,
                    is_local: false,
                });
                next = members.len() - 1; // skip to last member
                continue;
            }

            // Handle alias/re-export symbols
            if self.sym_flags(m).intersects(SymbolFlags::ALIAS) {
                let alias_decl = self.get_declaration_of_alias_symbol(m);
                let alias_target = self.get_target_of_alias_declaration(alias_decl);
                let target = self.get_merged_symbol(alias_target);
                if target.is_some() {
                    // If the alias target is a local symbol (not itself an export), emit its declaration first
                    if self.sym_flags(target).intersects(
                        SymbolFlags::BLOCK_SCOPED_VARIABLE
                            | SymbolFlags::FUNCTION_SCOPED_VARIABLE
                            | SymbolFlags::PROPERTY,
                    ) && emitted_locals.insert(target)
                    {
                        let target_type = self.get_type_of_symbol(target);
                        let local_type = self.get_widened_type(target_type);
                        let target_name = self.sym(target).name.clone();
                        hv_ctx(b).borrow_mut().approximate_length +=
                            go_len(&target_name) as i32 + 5;
                        let type_node = self.serialize_type_for_declaration(
                            b,
                            Node::NIL,
                            local_type,
                            target,
                            true,
                        );
                        let bb = b.borrow();
                        let declaration = bb.f().new_variable_declaration(
                            bb.f().new_identifier(target_name),
                            Node::NIL,
                            type_node,
                            Node::NIL,
                        );
                        let local_stmt = bb.f().new_variable_statement(
                            ModifierList::NIL,
                            bb.f().new_variable_declaration_list(
                                bb.f().new_node_list(&[declaration]),
                                NodeFlags::LET,
                            ),
                        );
                        body_stmts.push(HoverStatement {
                            node: local_stmt,
                            is_local: true,
                        });
                    }
                    let target_name = self.sym(target).name.clone();
                    let bb = b.borrow();
                    bb.ctx.borrow_mut().approximate_length += 16 + go_len(&m_name) as i32;
                    let mut property_name = Node::NIL;
                    if m_name != target_name {
                        property_name = bb.f().new_identifier(target_name);
                    }
                    let specifier = bb.f().new_export_specifier(
                        false,
                        property_name,
                        bb.f().new_identifier(m_name),
                    );
                    let stmt = bb.f().new_export_declaration(
                        ModifierList::NIL,
                        false,
                        bb.f().new_named_exports(bb.f().new_node_list(&[specifier])),
                        Node::NIL,
                        Node::NIL,
                    );
                    body_stmts.push(HoverStatement {
                        node: stmt,
                        is_local: false,
                    });
                    continue;
                }
            }

            let resolved = self.resolve_symbol(m);

            // Handle functions as function declarations
            if self
                .sym_flags(resolved)
                .intersects(SymbolFlags::FUNCTION | SymbolFlags::METHOD)
            {
                let t = self.get_type_of_symbol(resolved);
                let sigs = self.get_signatures_of_type(t, SignatureKind::CALL);
                for sig in sigs {
                    hv_ctx(b).borrow_mut().approximate_length += 1;
                    let name = b.borrow().f().new_identifier(m_name.clone());
                    let decl = self.signature_to_signature_declaration_helper(
                        b,
                        sig,
                        SyntaxKind::FunctionDeclaration,
                        Some(&SignatureToSignatureDeclarationOptions {
                            name,
                            ..Default::default()
                        }),
                    );
                    body_stmts.push(HoverStatement {
                        node: decl,
                        is_local: false,
                    });
                }
                // If the function also has namespace characteristics, emit an empty namespace.
                let merged = self.get_merged_symbol(resolved);
                let merged_exports = self.sym(merged).exports;
                let has_module_exports = self
                    .sym_flags(merged)
                    .intersects(SymbolFlags::VALUE_MODULE | SymbolFlags::NAMESPACE_MODULE)
                    && merged_exports.is_some()
                    && self.symbols.len(merged_exports) != 0;
                if !has_module_exports {
                    let bb = b.borrow();
                    let node = bb.f().new_module_declaration(
                        ModifierList::NIL,
                        SyntaxKind::NamespaceKeyword,
                        bb.f().new_identifier(m_name),
                        Node::NIL, /*attributes*/
                        bb.f().new_module_block(bb.f().new_node_list(&[])),
                    );
                    body_stmts.push(HoverStatement {
                        node,
                        is_local: false,
                    });
                }
                continue;
            }

            // Handle remaining member kinds (type alias, enum, class, interface, namespace, variable)
            let node = self.serialize_namespace_member(b, resolved, &m_name);
            if node.is_some() {
                body_stmts.push(HoverStatement {
                    node,
                    is_local: false,
                });
            }
        }

        let bb = b.borrow();
        // Add export modifier to exported statements (skip local declarations and ExportDeclarations).
        for s in body_stmts.iter_mut() {
            if s.is_local || is_export_declaration(s.node) {
                continue;
            }
            if can_have_modifiers(s.node) {
                let mf = s.node.modifier_flags() | ModifierFlags::EXPORT;
                s.node = replace_modifiers(bb.f(), s.node, modifier_list_from_flags(bb.f(), mf));
            }
        }

        // Collect nodes, stripping export if all statements are exported.
        let mut body_statements: Vec<Node> = body_stmts.iter().map(|s| s.node).collect();
        let all_exported = !body_statements.is_empty()
            && body_statements
                .iter()
                .all(|&d| has_syntactic_modifier(d, ModifierFlags::EXPORT));
        if all_exported {
            for stmt in body_statements.iter_mut() {
                if can_have_modifiers(*stmt) {
                    let mf = stmt.modifier_flags().without(ModifierFlags::EXPORT);
                    *stmt = replace_modifiers(bb.f(), *stmt, modifier_list_from_flags(bb.f(), mf));
                }
            }
        }

        let mut keyword = SyntaxKind::NamespaceKeyword;
        if !is_identifier(local_name) {
            keyword = SyntaxKind::ModuleKeyword;
        }
        let mut attributes = Node::NIL;
        let declaration = self
            .sym(symbol)
            .declarations
            .iter()
            .copied()
            .find(|&declaration| {
                is_module_declaration(declaration) && declaration.attributes().is_some()
            })
            .unwrap_or(Node::NIL);
        if declaration.is_some() {
            attributes = bb.f().deep_clone_node(declaration.attributes());
            bb.e.set_emit_flags(attributes, EmitFlags::SINGLE_LINE);
        }
        bb.f().new_module_declaration(
            ModifierList::NIL,
            keyword,
            local_name,
            attributes,
            bb.f()
                .new_module_block(bb.f().new_node_list(&body_statements)),
        )
    }

    // Go: checker/nodebuilder_hover.go:558 serializeTypeAliasForNamespace
    pub(crate) fn serialize_type_alias_for_namespace(
        &mut self,
        b: &Rc<RefCell<NodeBuilderImpl>>,
        symbol: SymbolId,
        name: &str,
    ) -> Node {
        let alias_type = self.get_declared_type_of_type_alias(symbol);
        let type_params =
            self.get_local_type_parameters_of_class_or_interface_or_type_alias(symbol);
        let type_param_decls: Vec<Node> = type_params
            .iter()
            .map(|&p| self.type_parameter_to_declaration(b, p))
            .collect();
        // PORT: Go saveRestoreFlags returns a closure. Its saved fields are
        // copied here and restored after typeToTypeNode.
        let (saved_flags, saved_internal_flags, saved_depth) = {
            let ctx = hv_ctx(b);
            let c = ctx.borrow();
            (c.flags, c.internal_flags, c.depth)
        };
        hv_ctx(b).borrow_mut().flags = saved_flags | NodeBuilderFlags::IN_TYPE_ALIAS;
        let type_node = self.type_to_type_node(b, alias_type);
        {
            let ctx = hv_ctx(b);
            let mut c = ctx.borrow_mut();
            c.flags = saved_flags;
            c.internal_flags = saved_internal_flags;
            c.depth = saved_depth;
            c.approximate_length += 8 + go_len(&name) as i32;
        }
        let bb = b.borrow();
        bb.f().new_type_alias_declaration(
            ModifierList::NIL,
            bb.f().new_identifier(name),
            bb.f().new_node_list(&type_param_decls),
            type_node,
        )
    }

    // Go: checker/nodebuilder_hover.go:571 filterInheritedProperties
    // PORT: the builder is not used, so no `b` parameter.
    pub(crate) fn filter_inherited_properties(
        &mut self,
        t: TypeId,
        base_types: &[TypeId],
        properties: Vec<SymbolId>,
    ) -> Vec<SymbolId> {
        if base_types.is_empty() {
            return properties;
        }
        // Build a lookup from property name to symbol for parent-identity comparison.
        let mut props_by_name: FxHashMap<String, SymbolId> = FxHashMap::default();
        for &p in &properties {
            props_by_name.insert(self.sym(p).name.to_string(), p);
        }
        // Collect names of properties inherited unchanged from base types.
        let mut inherited: FxHashSet<String> = FxHashSet::default();
        for &base in base_types {
            let target = self.get_target_type(t);
            let this_type = self.ty(target).as_interface_type().this_type;
            let base_with_this = self.get_type_with_this_argument(base, this_type, false);
            for prop in self.get_properties_of_type(base_with_this) {
                let prop_name = &self.sym(prop).name;
                if let Some(&existing) = props_by_name.get(prop_name.as_str()) {
                    if self.sym(prop).parent == self.sym(existing).parent {
                        inherited.insert(prop_name.to_string());
                    }
                }
            }
        }
        if inherited.is_empty() {
            return properties;
        }
        properties
            .into_iter()
            .filter(|&p| !inherited.contains(self.sym(p).name.as_str()))
            .collect()
    }

    // Go: checker/nodebuilder_hover.go:598 isNamespaceMember
    // PORT: the builder is not used, so no `b` parameter.
    pub(crate) fn is_namespace_member(&self, p: SymbolId) -> bool {
        let s = self.sym(p);
        s.flags
            .intersects(SymbolFlags::TYPE | SymbolFlags::NAMESPACE | SymbolFlags::ALIAS)
            || !(s.flags.intersects(SymbolFlags::PROTOTYPE)
                || s.name == "prototype"
                || (s.value_declaration.is_some()
                    && has_static_modifier(s.value_declaration)
                    && is_class_like(s.value_declaration.parent())))
    }
}

// Go: checker/nodebuilder_hover.go:217 typeElementsToClassElements
pub fn type_elements_to_class_elements(f: &NodeFactory, mut members: Vec<Node>) -> Vec<Node> {
    for m in members.iter_mut() {
        let node = *m;
        match node.kind() {
            SyntaxKind::PropertySignature => {
                *m = f.new_property_declaration(
                    node.modifiers(),
                    node.name(),
                    node.question_token(),
                    node.type_(),
                    Node::NIL,
                );
            }
            SyntaxKind::MethodSignature => {
                *m = f.new_method_declaration(
                    node.modifiers(),
                    Node::NIL,
                    node.name(),
                    node.question_token(),
                    node.type_parameter_list(),
                    node.parameter_list(),
                    node.type_(),
                    Node::NIL,
                    Node::NIL,
                );
            }
            _ => {}
        }
    }
    members
}

// Go: checker/nodebuilder_hover.go:603 isHashPrivate
pub fn is_hash_private(symbols: &SymbolArena, s: SymbolId) -> bool {
    let value_declaration = symbols.sym(s).value_declaration;
    value_declaration.is_some()
        && value_declaration.name().is_some()
        && is_private_identifier(value_declaration.name())
}
