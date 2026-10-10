//! Port of Go `checker/checker.go` lines 4794-5714 (index constraint checks,
//! property initialization, interface/enum/module declarations, imports and
//! exports).

use crate::diagnostics::Message;
use crate::prelude::*;

// PORT: Go `tspath.IsExternalModuleNameRelative`, `tspath.FileExtensionIsOneOf`
// and their helpers. tspath has no port in this crate, so these private
// copies follow the Go code exactly.

// Go: tspath/path.go:954 PathIsRelative
fn path_is_relative_p06(path: &str) -> bool {
    // True if path is ".", "..", or starts with "./", "../", ".\\", or "..\\".
    if path == "." || path == ".." {
        return true;
    }
    let b = path.as_bytes();
    if b.len() >= 2 && b[0] == b'.' && (b[1] == b'/' || b[1] == b'\\') {
        return true;
    }
    if b.len() >= 3 && b[0] == b'.' && b[1] == b'.' && (b[2] == b'/' || b[2] == b'\\') {
        return true;
    }
    false
}

// Go: tspath/path.go IsVolumeCharacter
fn is_volume_character_p06(ch: u8) -> bool {
    (b'a'..=b'z').contains(&ch) || (b'A'..=b'Z').contains(&ch)
}

// Go: tspath/path.go:152 getFileUrlVolumeSeparatorEnd
fn get_file_url_volume_separator_end_p06(url: &[u8], start: usize) -> i32 {
    if url.len() <= start {
        return -1;
    }
    let ch0 = url[start];
    if ch0 == b':' {
        return (start + 1) as i32;
    }
    if ch0 == b'%' && url.len() > start + 2 && url[start + 1] == b'3' {
        let ch2 = url[start + 2];
        if ch2 == b'a' || ch2 == b'A' {
            return (start + 3) as i32;
        }
    }
    -1
}

// Go: tspath/path.go:169 GetEncodedRootLength
fn get_encoded_root_length_p06(path: &str) -> i32 {
    let b = path.as_bytes();
    let ln = b.len();
    if ln == 0 {
        return 0;
    }
    let ch0 = b[0];

    // POSIX or UNC
    if ch0 == b'/' || ch0 == b'\\' {
        if ln == 1 || b[1] != ch0 {
            return 1; // POSIX: "/" (or non-normalized "\")
        }
        let offset = 2;
        return match b[offset..].iter().position(|&c| c == ch0) {
            None => ln as i32,                    // UNC: "//server" or "\\server"
            Some(p1) => (p1 + offset + 1) as i32, // UNC: "//server/" or "\\server\"
        };
    }

    // DOS
    if is_volume_character_p06(ch0) && ln > 1 && b[1] == b':' {
        if ln == 2 {
            return 2; // DOS: "c:" (but not "c:d")
        }
        let ch2 = b[2];
        if ch2 == b'/' || ch2 == b'\\' {
            return 3; // DOS: "c:/" or "c:\"
        }
    }

    // Untitled paths (e.g., "^/untitled/ts-nul-authority/Untitled-1")
    if ch0 == b'^' && ln > 1 && b[1] == b'/' {
        return 2; // Untitled: "^/"
    }

    // URL
    const URL_SCHEME_SEPARATOR: &str = "://";
    if let Some(scheme_end) = path.find(URL_SCHEME_SEPARATOR) {
        let authority_start = scheme_end + URL_SCHEME_SEPARATOR.len();
        if let Some(authority_length) = path[authority_start..].find('/') {
            // URL: "file:///", "file://server/", "file://server/path"
            let authority_end = authority_start + authority_length;

            // For local "file" URLs, include the leading DOS volume (if present).
            let scheme = &path[..scheme_end];
            let authority = &path[authority_start..authority_end];
            if scheme == "file"
                && (authority.is_empty() || authority == "localhost")
                && (ln > authority_end + 2)
                && is_volume_character_p06(b[authority_end + 1])
            {
                let volume_separator_end =
                    get_file_url_volume_separator_end_p06(b, authority_end + 2);
                if volume_separator_end != -1 {
                    if volume_separator_end as usize == ln {
                        return !volume_separator_end;
                    }
                    if b[volume_separator_end as usize] == b'/' {
                        return !(volume_separator_end + 1);
                    }
                }
            }
            return !((authority_end + 1) as i32); // URL: "file://server/", "http://server/"
        }
        return !(ln as i32); // URL: "file://server", "http://server"
    }

    // relative
    0
}

// Go: tspath/path.go:981 IsExternalModuleNameRelative
fn is_external_module_name_relative_p06(module_name: &str) -> bool {
    // TypeScript 1.0 spec (April 2014): 11.2.1
    // An external module name is "relative" if the first term is "." or "..".
    // Update: We also consider a path like `C:\foo.ts` "relative" because we do not search for it in `node_modules` or treat it as an ambient module.
    path_is_relative_p06(module_name) || get_encoded_root_length_p06(module_name) > 0
}

// Go: tspath/extension.go:79 FileExtensionIsOneOf
fn file_extension_is_one_of_p06(path: &str, extensions: &[&str]) -> bool {
    extensions
        .iter()
        .any(|ext| path.len() > ext.len() && path.ends_with(ext))
}

/// The elements of Go `node.AsImportAttributes().Attributes.Nodes`.
// PORT: Go `Node.Attributes()` (JSX only) wins the name in node.rs and
// fields.rs does not generate the clashing `Attributes` field accessor. The
// `Attributes` list is the only child list of an `ImportAttributes` node, so
// its `ImportAttribute` children are exactly the list nodes, in order.
fn import_attributes_list_p06(node: Node) -> Vec<Node> {
    let mut result = Vec::new();
    node.for_each_child(&mut |child: Node| {
        if child.kind() == SyntaxKind::ImportAttribute {
            result.push(child);
        }
        false
    });
    result
}

// Go: checker/checker.go:5010 (struct) InheritanceInfo
#[derive(Clone, Copy, Debug, Default)]
pub struct InheritanceInfo {
    pub prop: SymbolId,
    pub containing_type: TypeId,
}

impl Checker {
    // Go: checker/checker.go:4904 checkIndexConstraintForProperty
    pub fn check_index_constraint_for_property(
        &mut self,
        t: TypeId,
        prop: SymbolId,
        prop_name_type: TypeId,
        prop_type: TypeId,
    ) {
        let declaration = self.sym(prop).value_declaration;
        let name = get_name_of_declaration(declaration);
        if name.is_some() && is_private_identifier(name) {
            return;
        }
        let index_infos = self.get_applicable_index_infos(t, prop_name_type);
        if index_infos.is_empty() {
            return;
        }
        let t_symbol = self.ty(t).symbol;
        let mut interface_declaration = Node::NIL;
        if self.ty(t).object_flags.intersects(ObjectFlags::INTERFACE) {
            interface_declaration =
                get_declaration_of_kind(&self.symbols, t_symbol, SyntaxKind::InterfaceDeclaration);
        }
        let mut prop_declaration = Node::NIL;
        if declaration.is_some() && is_binary_expression(declaration)
            || name.is_some() && is_computed_property_name(name)
        {
            prop_declaration = declaration;
        }
        let mut local_prop_declaration = Node::NIL;
        if self.get_parent_of_symbol(prop) == t_symbol {
            local_prop_declaration = declaration;
        }
        for info in index_infos {
            let info_declaration = self.index_info(info).declaration;
            let info_key_type = self.index_info(info).key_type;
            let info_value_type = self.index_info(info).value_type;
            let mut local_index_declaration = Node::NIL;
            if info_declaration.is_some() {
                let decl_symbol = self.get_symbol_of_declaration(info_declaration);
                if self.get_parent_of_symbol(decl_symbol) == t_symbol {
                    local_index_declaration = info_declaration;
                }
            }
            // We check only when (a) the property is declared in the containing type, or (b) the applicable index signature is declared
            // in the containing type, or (c) the containing type is an interface and no base interface contains both the property and
            // the index signature (i.e. property and index signature are declared in separate inherited interfaces).
            let mut error_node = if local_prop_declaration.is_some() {
                local_prop_declaration
            } else {
                local_index_declaration
            };
            if error_node.is_nil() && interface_declaration.is_some() {
                let prop_name = self.sym(prop).name.clone();
                let mut some = false;
                for base in self.get_base_types(t) {
                    if self.get_property_of_object_type(base, &prop_name).is_some()
                        && self.get_index_type_of_type(base, info_key_type).is_some()
                    {
                        some = true;
                        break;
                    }
                }
                if !some {
                    error_node = interface_declaration;
                }
            }
            if error_node.is_some() && !self.is_type_assignable_to(prop_type, info_value_type) {
                let mut diagnostic = new_diagnostic_for_node(
                    error_node,
                    diag::Property_0_of_type_1_is_not_assignable_to_2_index_type_3,
                    args![
                        self.symbol_to_string(prop),
                        self.type_to_string(prop_type),
                        self.type_to_string(info_key_type),
                        self.type_to_string(info_value_type)
                    ],
                );
                if prop_declaration.is_some() && error_node != prop_declaration {
                    diagnostic.add_related_info(Some(new_diagnostic_for_node(
                        prop_declaration,
                        diag::X_0_is_declared_here,
                        args![self.symbol_to_string(prop)],
                    )));
                }
                self.add_diagnostic(diagnostic);
            }
        }
    }

    // Go: checker/checker.go:4950 checkIndexConstraintForIndexSignature
    pub fn check_index_constraint_for_index_signature(
        &mut self,
        t: TypeId,
        check_info: IndexInfoId,
    ) {
        let declaration = self.index_info(check_info).declaration;
        let check_key_type = self.index_info(check_info).key_type;
        let check_value_type = self.index_info(check_info).value_type;
        let index_infos = self.get_applicable_index_infos(t, check_key_type);
        if index_infos.is_empty() {
            return;
        }
        let t_symbol = self.ty(t).symbol;
        let mut interface_declaration = Node::NIL;
        if self.ty(t).object_flags.intersects(ObjectFlags::INTERFACE) {
            interface_declaration =
                get_declaration_of_kind(&self.symbols, t_symbol, SyntaxKind::InterfaceDeclaration);
        }
        let mut local_check_declaration = Node::NIL;
        if declaration.is_some() {
            let decl_symbol = self.get_symbol_of_declaration(declaration);
            if self.get_parent_of_symbol(decl_symbol) == t_symbol {
                local_check_declaration = declaration;
            }
        }
        for info in index_infos {
            if info == check_info {
                continue;
            }
            let info_declaration = self.index_info(info).declaration;
            let info_key_type = self.index_info(info).key_type;
            let info_value_type = self.index_info(info).value_type;
            let mut local_index_declaration = Node::NIL;
            if info_declaration.is_some() {
                let decl_symbol = self.get_symbol_of_declaration(info_declaration);
                if self.get_parent_of_symbol(decl_symbol) == t_symbol {
                    local_index_declaration = info_declaration;
                }
            }
            // We check only when (a) the check index signature is declared in the containing type, or (b) the applicable index
            // signature is declared in the containing type, or (c) the containing type is an interface and no base interface contains
            // both index signatures (i.e. the index signatures are declared in separate inherited interfaces).
            let mut error_node = if local_check_declaration.is_some() {
                local_check_declaration
            } else {
                local_index_declaration
            };
            if error_node.is_nil() && interface_declaration.is_some() {
                let mut some = false;
                for base in self.get_base_types(t) {
                    if self.get_index_info_of_type(base, check_key_type).is_some()
                        && self.get_index_type_of_type(base, info_key_type).is_some()
                    {
                        some = true;
                        break;
                    }
                }
                if !some {
                    error_node = interface_declaration;
                }
            }
            if error_node.is_some()
                && !self.is_type_assignable_to(check_value_type, info_value_type)
            {
                let a0 = self.type_to_string(check_key_type);
                let a1 = self.type_to_string(check_value_type);
                let a2 = self.type_to_string(info_key_type);
                let a3 = self.type_to_string(info_value_type);
                self.error(
                    error_node,
                    diag::X_0_index_type_1_is_not_assignable_to_2_index_type_3,
                    args![a0, a1, a2, a3],
                );
            }
        }
    }

    // Go: checker/checker.go:4987 checkClassOrInterfaceForDuplicateIndexSignatures
    pub fn check_class_or_interface_for_duplicate_index_signatures(&mut self, node: Node) {
        // Only check the type once
        let symbol = self.get_symbol_of_declaration(node);
        if !self
            .declared_type_links
            .get(symbol)
            .index_signatures_checked
        {
            self.declared_type_links
                .get(symbol)
                .index_signatures_checked = true;
            self.check_type_for_duplicate_index_signatures(node);
        }
    }

    // Go: checker/checker.go:4995 checkTypeForDuplicateIndexSignatures
    pub fn check_type_for_duplicate_index_signatures(&mut self, node: Node) {
        // TypeScript 1.0 spec (April 2014)
        // 3.7.4: An object type can contain at most one string index signature and one numeric index signature.
        // 8.5: A class declaration can have at most one string index member declaration and one numeric index member declaration
        let node_symbol = self.get_symbol_of_declaration(node);
        let index_symbol = self.get_index_symbol(node_symbol);
        if index_symbol.is_nil() || self.sym(index_symbol).declarations.len() <= 1 {
            return;
        }
        // PORT: Go map iteration order is random; IndexMap keeps first-seen order.
        let mut index_signature_map: IndexMap<TypeId, Vec<Node>> = IndexMap::new();
        for declaration in self.sym(index_symbol).declarations.clone() {
            if is_index_signature_declaration(declaration) {
                let parameters = declaration.parameters();
                if parameters.len() == 1 && parameters.get(0).type_().is_some() {
                    let param_type = self.get_type_from_type_node(parameters.get(0).type_());
                    let distributed = self.ty(param_type).distributed().to_vec();
                    for t in distributed {
                        index_signature_map.entry(t).or_default().push(declaration);
                    }
                }
            }
            // Do nothing for late-bound index signatures: allow these to duplicate one another and explicit indexes
        }
        for (t, declarations) in index_signature_map {
            if declarations.len() > 1 {
                for declaration in declarations {
                    let s = self.type_to_string(t);
                    self.error(
                        declaration,
                        diag::Duplicate_index_signature_for_type_0,
                        args![s],
                    );
                }
            }
        }
    }

    // Go: checker/checker.go:5024 checkPropertyInitialization
    pub fn check_property_initialization(&mut self, node: Node) {
        if !self.strict_null_checks
            || !self.strict_property_initialization
            || node.flags().intersects(NodeFlags::AMBIENT)
        {
            return;
        }
        let constructor = find_constructor_declaration(node);
        for member in node.members() {
            if member.modifier_flags().intersects(ModifierFlags::AMBIENT) {
                continue;
            }
            if !is_static(member) && self.is_property_without_initializer(member) {
                let prop_name = member.name();
                if is_identifier(prop_name)
                    || is_private_identifier(prop_name)
                    || is_computed_property_name(prop_name)
                {
                    let member_symbol = self.get_symbol_of_declaration(member);
                    let t = self.get_type_of_symbol(member_symbol);
                    if !(self.ty(t).flags.intersects(TypeFlags::ANY_OR_UNKNOWN)
                        || self.contains_undefined_type(t))
                    {
                        if constructor.is_nil()
                            || !self.is_property_initialized_in_constructor(
                                prop_name,
                                t,
                                constructor,
                            )
                        {
                            self.error(
                                member.name(),
                                diag::Property_0_has_no_initializer_and_is_not_definitely_assigned_in_the_constructor,
                                args![declaration_name_to_string(prop_name)],
                            );
                        }
                    }
                }
            }
        }
    }

    // Go: checker/checker.go:5047 isPropertyWithoutInitializer
    pub fn is_property_without_initializer(&self, node: Node) -> bool {
        is_property_declaration(node)
            && !has_abstract_modifier(node)
            && !is_exclamation_token(node.postfix_token())
            && node.initializer().is_nil()
    }

    // Go: checker/checker.go:5051 isPropertyInitializedInStaticBlocks
    pub fn is_property_initialized_in_static_blocks(
        &mut self,
        prop_name: Node,
        prop_type: TypeId,
        static_blocks: &[Node],
        start_pos: i32,
        end_pos: i32,
    ) -> bool {
        for &static_block in static_blocks {
            // static block must be within the provided range as they are evaluated in document order (unlike constructors)
            if static_block.pos() >= start_pos && static_block.pos() <= end_pos {
                let this_keyword = self.factory.new_keyword_expression(SyntaxKind::ThisKeyword);
                let reference = self.factory.new_property_access_expression(
                    this_keyword,
                    Node::NIL,
                    prop_name,
                    NodeFlags::NONE,
                );
                set_node_parent(reference.expression(), reference);
                set_node_parent(reference, static_block);
                set_node_flow_node(reference, static_block.return_flow_node());
                let optional_type = self.get_optional_type(prop_type, false);
                let flow_type = self.get_flow_type_of_reference_ex(
                    reference,
                    prop_type,
                    optional_type,
                    Node::NIL,
                    FlowNodeId::NIL,
                );
                if !self.contains_undefined_type(flow_type) {
                    return true;
                }
            }
        }
        false
    }

    // Go: checker/checker.go:5068 isPropertyInitializedInConstructor
    pub fn is_property_initialized_in_constructor(
        &mut self,
        prop_name: Node,
        prop_type: TypeId,
        constructor: Node,
    ) -> bool {
        let reference = if is_computed_property_name(prop_name) {
            let this_keyword = self.factory.new_keyword_expression(SyntaxKind::ThisKeyword);
            self.factory.new_element_access_expression(
                this_keyword,
                Node::NIL,
                prop_name.expression(),
                NodeFlags::NONE,
            )
        } else {
            let this_keyword = self.factory.new_keyword_expression(SyntaxKind::ThisKeyword);
            self.factory.new_property_access_expression(
                this_keyword,
                Node::NIL,
                prop_name,
                NodeFlags::NONE,
            )
        };
        set_node_parent(reference.expression(), reference);
        set_node_parent(reference, constructor);
        set_node_flow_node(reference, constructor.return_flow_node());
        let optional_type = self.get_optional_type(prop_type, false);
        let flow_type = self.get_flow_type_of_reference_ex(
            reference,
            prop_type,
            optional_type,
            Node::NIL,
            FlowNodeId::NIL,
        );
        !self.contains_undefined_type(flow_type)
    }

    // Go: checker/checker.go:5082 checkInterfaceDeclaration
    pub fn check_interface_declaration(&mut self, node: Node) {
        if !self.check_grammar_modifiers(node) {
            self.check_grammar_interface_declaration(node);
        }
        if !self.container_allows_block_scoped_variable(node.parent()) {
            self.grammar_error_on_node(
                node,
                diag::X_0_declarations_can_only_be_declared_inside_a_block,
                args!["interface"],
            );
        }
        self.check_type_parameters(node.type_parameters());
        self.check_type_name_is_reserved(node.name(), diag::Interface_name_cannot_be_0);
        self.check_exports_on_merged_declarations(node);
        let symbol = self.get_symbol_of_declaration(node);
        self.check_type_parameter_lists_identical(symbol);
        // Check once per checker, but report on the first interface declaration,
        // independently of which declaration is checked first.
        // (ts#64566, Go N' checker.go:5110)
        if !self.declared_type_links.get(symbol).interface_checked {
            self.declared_type_links.get(symbol).interface_checked = true;
            let first_interface_declaration =
                get_declaration_of_kind(&self.symbols, symbol, SyntaxKind::InterfaceDeclaration);
            let t = self.get_declared_type_of_symbol(symbol);
            let type_with_this = self.get_type_with_this_argument(t, TypeId::NIL, false);
            // run subsequent checks only if first set succeeded
            if self.check_inherited_properties_are_identical(t, first_interface_declaration.name())
            {
                for base_type in self.get_base_types(t) {
                    let this_type = self.ty(t).as_interface_type().this_type;
                    let base_with_this =
                        self.get_type_with_this_argument(base_type, this_type, false);
                    self.check_type_assignable_to(
                        type_with_this,
                        base_with_this,
                        first_interface_declaration.name(),
                        Some(diag::Interface_0_incorrectly_extends_interface_1),
                    );
                }
                self.check_index_constraints(t, symbol, false /*isStaticIndex*/);
            }
        }
        self.check_object_type_for_duplicate_declarations(node, false /*checkPrivateNames*/);
        for heritage_element in get_extends_heritage_clause_elements(node) {
            if is_expression_with_type_arguments(heritage_element) {
                let expr = heritage_element.expression();
                if !is_entity_name_expression(expr) || is_optional_chain(expr) {
                    self.error(expr, diag::An_interface_can_only_extend_an_identifier_Slashqualified_name_with_optional_type_arguments, args![]);
                }
            }
            self.check_type_reference_node(heritage_element);
        }
        self.check_source_elements(node.members());
        self.check_class_or_interface_for_duplicate_index_signatures(node);
        self.register_for_unused_identifiers_check(node);
    }

    // Go: checker/checker.go:5127 checkInheritedPropertiesAreIdentical
    pub fn check_inherited_properties_are_identical(&mut self, t: TypeId, type_node: Node) -> bool {
        let base_types = self.get_base_types(t);
        if base_types.len() < 2 {
            return true;
        }
        let mut seen: FxHashMap<String, InheritanceInfo> = FxHashMap::default();
        // PORT: Go `c.resolveDeclaredMembers(t)` returns `t.AsInterfaceType()`;
        // the declared members are read back from `t` after the call.
        self.resolve_declared_members(t);
        let declared_members = self.ty(t).as_interface_type().declared_members;
        for (id, p) in self.symbols.entries(declared_members) {
            if self.is_named_member(p, &id) {
                let name = self.sym(p).name.to_string();
                seen.insert(
                    name,
                    InheritanceInfo {
                        prop: p,
                        containing_type: t,
                    },
                );
            }
        }
        let mut identical = true;
        for base in base_types {
            let this_type = self.ty(t).as_interface_type().this_type;
            let base_with_this = self.get_type_with_this_argument(base, this_type, false);
            let properties = self.get_properties_of_type(base_with_this);
            for prop in properties {
                let prop_name = self.sym(prop).name.to_string();
                match seen.get(&prop_name).copied() {
                    None => {
                        seen.insert(
                            prop_name,
                            InheritanceInfo {
                                prop,
                                containing_type: base,
                            },
                        );
                    }
                    Some(existing) => {
                        let is_inherited_property = existing.containing_type != t;
                        if is_inherited_property
                            && !self.is_property_identical_to(existing.prop, prop)
                        {
                            identical = false;
                            let type_name1 = self.type_to_string(existing.containing_type);
                            let type_name2 = self.type_to_string(base);
                            let error_info = new_diagnostic_for_node(
                                type_node,
                                diag::Named_property_0_of_types_1_and_2_are_not_identical,
                                args![self.symbol_to_string(prop), type_name1, type_name2],
                            );
                            let t_name = self.type_to_string(t);
                            self.add_diagnostic(new_diagnostic_chain(
                                Some(error_info),
                                diag::Interface_0_cannot_simultaneously_extend_types_1_and_2,
                                args![t_name, type_name1, type_name2],
                            ));
                        }
                    }
                }
            }
        }
        identical
    }

    // Go: checker/checker.go:5159 isPropertyIdenticalTo
    pub fn is_property_identical_to(
        &mut self,
        source_prop: SymbolId,
        target_prop: SymbolId,
    ) -> bool {
        self.compare_properties(
            source_prop,
            target_prop,
            &mut |c: &mut Checker, s: TypeId, t: TypeId| c.compare_types_identical(s, t),
        ) != Ternary::FALSE
    }

    // Go: checker/checker.go:5163 checkEnumDeclaration
    pub fn check_enum_declaration(&mut self, node: Node) {
        self.check_grammar_modifiers(node);
        self.check_collisions_for_declaration_name(node, node.name());
        self.check_exports_on_merged_declarations(node);
        self.check_source_elements(node.members());

        if self.should_check_erasable_syntax(node) && !node.flags().intersects(NodeFlags::AMBIENT) {
            self.error(
                node,
                diag::This_syntax_is_not_allowed_when_erasableSyntaxOnly_is_enabled,
                args![],
            );
        }

        self.compute_enum_member_values(node);
        // Spec 2014 - Section 9.3:
        // It isn't possible for one enum declaration to continue the automatic numbering sequence of another,
        // and when an enum type has multiple declarations, only one declaration is permitted to omit a value
        // for the first member.
        //
        // Only perform this check once per symbol
        let enum_symbol = self.get_symbol_of_declaration(node);
        if !self.declared_type_links.get(enum_symbol).enum_checked {
            self.declared_type_links.get(enum_symbol).enum_checked = true;
            let declarations = self.sym(enum_symbol).declarations.clone();
            if declarations.len() > 1 {
                // ts#64566 (Go N' checker.go:5202): the const test of the first enum declaration.
                let first_enum_declaration = get_declaration_of_kind(
                    &self.symbols,
                    enum_symbol,
                    SyntaxKind::EnumDeclaration,
                );
                let enum_is_const = is_enum_const(first_enum_declaration);
                // check that const is placed\omitted on all enum declarations
                for &decl in &declarations {
                    if is_enum_declaration(decl) && is_enum_const(decl) != enum_is_const {
                        self.error(
                            get_name_of_declaration(decl),
                            diag::Enum_declarations_must_all_be_const_or_non_const,
                            args![],
                        );
                    }
                }
            }
            let mut seen_enum_missing_initial_initializer = false;
            for &declaration in &declarations {
                // return true if we hit a violation of the rule, false otherwise
                if declaration.kind() != SyntaxKind::EnumDeclaration {
                    continue;
                }
                let members = declaration.members();
                if members.is_empty() {
                    continue;
                }
                let first_enum_member = members.get(0);
                if first_enum_member.initializer().is_nil() {
                    if seen_enum_missing_initial_initializer {
                        self.error(
                            first_enum_member.name(),
                            diag::In_an_enum_with_multiple_declarations_only_one_declaration_can_omit_an_initializer_for_its_first_enum_element,
                            args![],
                        );
                    } else {
                        seen_enum_missing_initial_initializer = true;
                    }
                }
            }
        }
    }

    // Go: checker/checker.go:5214 checkEnumMember
    pub fn check_enum_member(&mut self, node: Node) {
        if is_private_identifier(node.name()) {
            self.error(
                node,
                diag::An_enum_member_cannot_be_named_with_a_private_identifier,
                args![],
            );
        }
        // ts#64674, Go N' checker.go:5237: a computed name is checked even
        // though it is a grammar error.
        if is_computed_property_name(node.name()) {
            self.check_expression(node.name().expression());
        }
        if node.initializer().is_some() {
            self.check_expression(node.initializer());
        }
    }

    // Go: checker/checker.go:5223 checkModuleDeclaration
    pub fn check_module_declaration(&mut self, node: Node) {
        let body = node.body();
        if body.is_some() {
            self.check_source_element(body);
            if !is_global_scope_augmentation(node) {
                self.register_for_unused_identifiers_check(node);
            }
        }
        let is_global_augmentation = is_global_scope_augmentation(node);
        let in_ambient_context = node.flags().intersects(NodeFlags::AMBIENT);
        if is_global_augmentation && !in_ambient_context {
            self.error(
                node.name(),
                diag::Augmentations_for_the_global_scope_should_have_declare_modifier_unless_they_appear_in_already_ambient_context,
                args![],
            );
        }
        let attributes = node.attributes();
        if attributes.is_some() {
            self.check_import_attributes_type(attributes);
        }
        let is_ambient_external_module = is_ambient_module(node);
        let context_error_message = if is_ambient_external_module {
            diag::An_ambient_module_declaration_is_only_allowed_at_the_top_level_in_a_file
        } else {
            diag::A_namespace_declaration_is_only_allowed_at_the_top_level_of_a_namespace_or_module
        };
        if self.check_grammar_module_element_context(node, context_error_message) {
            // If we hit a module declaration in an illegal context, just bail out to avoid cascading errors.
            return;
        }
        if !self.check_grammar_modifiers(node) {
            if !in_ambient_context && is_string_literal(node.name()) {
                self.grammar_error_on_node(
                    node.name(),
                    diag::Only_ambient_modules_can_use_quoted_names,
                    args![],
                );
            }
        }
        if is_identifier(node.name()) {
            self.check_collisions_for_declaration_name(node, node.name());
            if node.keyword() == SyntaxKind::ModuleKeyword {
                self.error(
                    node.name(),
                    diag::A_namespace_declaration_should_not_be_declared_using_the_module_keyword_Please_use_the_namespace_keyword_instead,
                    args![],
                );
            }
        }
        self.check_exports_on_merged_declarations(node);
        let symbol = self.get_symbol_of_declaration(node);
        // The following checks only apply on a non-ambient instantiated module declaration.
        if self.sym(symbol).flags.intersects(SymbolFlags::VALUE_MODULE)
            && !in_ambient_context
            && is_instantiated_module(node, self.compiler_options.should_preserve_const_enums())
        {
            if self.should_check_erasable_syntax(node) {
                self.error(
                    node,
                    diag::This_syntax_is_not_allowed_when_erasableSyntaxOnly_is_enabled,
                    args![],
                );
            }
            if self.compiler_options.get_isolated_modules()
                && with_source_file_info(get_source_file_of_node(node), |info| {
                    info.external_module_indicator
                })
                .is_nil()
            {
                // This could be loosened a little if needed. The only problem we are trying to avoid is unqualified
                // references to namespace members declared in other files. But use of namespaces is discouraged anyway,
                // so for now we will just not allow them in scripts, which is the only place they can merge cross-file.
                let flag_name = self.get_isolated_modules_like_flag_name();
                self.error(
                    node.name(),
                    diag::Namespaces_are_not_allowed_in_global_script_files_when_0_is_enabled_If_this_file_is_not_intended_to_be_a_global_script_set_moduleDetection_to_force_or_add_an_empty_export_statement,
                    args![flag_name],
                );
            }
            if self.sym(symbol).declarations.len() > 1 {
                let first_non_ambient_class_or_func =
                    self.get_first_non_ambient_class_or_function_declaration(symbol);
                if first_non_ambient_class_or_func.is_some() {
                    if get_source_file_of_node(node)
                        != get_source_file_of_node(first_non_ambient_class_or_func)
                    {
                        self.error(
                            node.name(),
                            diag::A_namespace_declaration_cannot_be_in_a_different_file_from_a_class_or_function_with_which_it_is_merged,
                            args![],
                        );
                    } else if node.pos() < first_non_ambient_class_or_func.pos() {
                        self.error(
                            node.name(),
                            diag::A_namespace_declaration_cannot_be_located_prior_to_a_class_or_function_with_which_it_is_merged,
                            args![],
                        );
                    }
                }
            }
            if self.compiler_options.verbatim_module_syntax.is_true()
                && is_source_file(node.parent())
                && node.modifier_flags().intersects(ModifierFlags::EXPORT)
                && get_emit_module_format_of_file(node.parent()) == ModuleKind::COMMON_JS
            {
                let export_modifier = node
                    .modifier_nodes()
                    .iter()
                    .find(|m| m.kind() == SyntaxKind::ExportKeyword)
                    .unwrap_or(Node::NIL);
                self.error(
                    export_modifier,
                    diag::A_top_level_export_modifier_cannot_be_used_on_value_declarations_in_a_CommonJS_module_when_verbatimModuleSyntax_is_enabled,
                    args![],
                );
            }
        }
        if is_ambient_external_module {
            if is_external_module_augmentation(node) {
                if attributes.is_some() {
                    self.error(
                        attributes,
                        diag::Import_attributes_are_not_allowed_on_a_module_augmentation,
                        args![],
                    );
                }
                // body of the augmentation should be checked for consistency only if augmentation was applied to its target (either global scope or module)
                // otherwise we'll be swamped in cascading errors.
                // We can detect if augmentation was applied using following rules:
                // - augmentation for a global scope is always applied
                // - augmentation for some external module is applied if symbol for augmentation is merged (it was combined with target module).
                let check_body = is_global_augmentation || {
                    let s = self.get_symbol_of_declaration(node);
                    self.sym(s).flags.intersects(SymbolFlags::TRANSIENT)
                };
                if check_body && node.body().is_some() {
                    for statement in node.body().statements() {
                        self.check_module_augmentation_element(statement);
                    }
                }
            } else if is_global_source_file(node.parent()) {
                if is_global_augmentation {
                    self.error(
                        node.name(),
                        diag::Augmentations_for_the_global_scope_can_only_be_directly_nested_in_external_modules_or_ambient_module_declarations,
                        args![],
                    );
                } else if is_external_module_name_relative_p06(node.name().text()) {
                    self.error(
                        node.name(),
                        diag::Ambient_module_declaration_cannot_specify_relative_module_name,
                        args![],
                    );
                }
            } else {
                if is_global_augmentation {
                    self.error(
                        node.name(),
                        diag::Augmentations_for_the_global_scope_can_only_be_directly_nested_in_external_modules_or_ambient_module_declarations,
                        args![],
                    );
                } else {
                    // Node is not an augmentation and is not located on the script level.
                    // This means that this is declaration of ambient module that is located in other module or namespace which is prohibited.
                    self.error(
                        node.name(),
                        diag::Ambient_modules_cannot_be_nested_in_other_modules_or_namespaces,
                        args![],
                    );
                }
            }
        }
    }

    // Go: checker/checker.go:5320 checkImportAttributesType
    pub fn check_import_attributes_type(&mut self, attributes: Node) {
        self.check_grammar_import_attributes_type(attributes);
        self.check_source_element(attributes);
        let import_attributes_type = self.get_global_import_attributes_type_checked();
        let module_attributes_type =
            self.get_type_of_module_declaration_import_attributes(attributes);
        if import_attributes_type != self.empty_object_type {
            self.check_type_assignable_to(
                module_attributes_type,
                import_attributes_type,
                attributes,
                None,
            );
        }
    }

    // Go: checker/checker.go:5330 getTypeOfModuleDeclarationImportAttributes
    pub fn get_type_of_module_declaration_import_attributes(&mut self, attributes: Node) -> TypeId {
        if attributes.is_nil() {
            return self.empty_object_type;
        }
        self.get_type_from_type_node(attributes)
    }

    // Go: checker/checker.go:5337 getTypeOfModuleImportAttributes
    pub fn get_type_of_module_import_attributes(&mut self, symbol: SymbolId) -> TypeId {
        if let Some(&t) = self.module_import_attributes_types.get(&symbol) {
            return t;
        }
        let result;
        let module_decl = self
            .sym(symbol)
            .declarations
            .iter()
            .copied()
            .find(|&d| is_module_with_string_literal_name(d))
            .unwrap_or(Node::NIL);
        if module_decl.is_nil() {
            result = self.empty_object_type;
        } else {
            result =
                self.get_type_of_module_declaration_import_attributes(module_decl.attributes());
        }
        self.module_import_attributes_types.insert(symbol, result);
        result
    }
}

impl Checker {
    // Go: checker/checker.go:5357 getFirstNonAmbientClassOrFunctionDeclaration
    // PORT: package-level Go function that reads symbol data, so it is a
    // `Checker` method per the contract.
    pub fn get_first_non_ambient_class_or_function_declaration(&self, symbol: SymbolId) -> Node {
        for &declaration in &self.sym(symbol).declarations {
            if (is_class_declaration(declaration)
                || is_function_declaration(declaration) && node_is_present(declaration.body()))
                && !declaration.flags().intersects(NodeFlags::AMBIENT)
            {
                return declaration;
            }
        }
        Node::NIL
    }

    // Go: checker/checker.go:5366 getIsolatedModulesLikeFlagName
    pub fn get_isolated_modules_like_flag_name(&self) -> String {
        if self.compiler_options.verbatim_module_syntax.is_true() {
            "verbatimModuleSyntax".to_string()
        } else {
            "isolatedModules".to_string()
        }
    }

    // Go: checker/checker.go:5370 checkModuleAugmentationElement
    pub fn check_module_augmentation_element(&mut self, node: Node) {
        match node.kind() {
            SyntaxKind::VariableStatement => {
                // error each individual name in variable statement instead of marking the entire variable statement
                for decl in node.declaration_list().declarations().nodes() {
                    self.check_module_augmentation_element(decl);
                }
            }
            SyntaxKind::ExportAssignment | SyntaxKind::ExportDeclaration => {
                self.grammar_error_on_first_token(
                    node,
                    diag::Exports_and_export_assignments_are_not_permitted_in_module_augmentations,
                    args![],
                );
            }
            SyntaxKind::ImportEqualsDeclaration
            | SyntaxKind::ImportDeclaration
            | SyntaxKind::JsImportDeclaration => {
                // import a = e.x; in module augmentation is ok, but not import a = require('fs)
                if node.kind() == SyntaxKind::ImportEqualsDeclaration
                    && is_internal_module_import_equals_declaration(node)
                {
                    return;
                }
                // Go: fallthrough from KindImportEqualsDeclaration
                self.grammar_error_on_first_token(
                    node,
                    diag::Imports_are_not_permitted_in_module_augmentations_Consider_moving_them_to_the_enclosing_external_module,
                    args![],
                );
            }
            SyntaxKind::BindingElement | SyntaxKind::VariableDeclaration => {
                let name = node.name();
                if is_binding_pattern(name) {
                    for el in name.elements() {
                        // mark individual names in binding pattern
                        self.check_module_augmentation_element(el);
                    }
                }
            }
            _ => {}
        }
    }

    // Go: checker/checker.go:5398 checkImportDeclaration
    pub fn check_import_declaration(&mut self, node: Node) {
        // Grammar checking
        let diagnostic = if is_in_js_file(node) {
            diag::An_import_declaration_can_only_be_used_at_the_top_level_of_a_module
        } else {
            diag::An_import_declaration_can_only_be_used_at_the_top_level_of_a_namespace_or_module
        };
        if self.check_grammar_module_element_context(node, diagnostic) {
            // If we hit an import declaration in an illegal context, just bail out to avoid cascading errors.
            self.check_external_module_name_in_global_scope(node);
            return;
        }
        if !self.check_grammar_modifiers(node) && !node.modifiers().is_nil() {
            self.grammar_error_on_first_token(
                node,
                diag::An_import_declaration_cannot_have_modifiers,
                args![],
            );
        }
        if self.check_external_import_or_export_declaration(node) {
            let attributes = get_import_attributes(node);
            let mut resolved_module = SymbolId::NIL;
            let import_clause = node.import_clause();
            let module_specifier = node.module_specifier();
            if import_clause.is_some() && !self.check_grammar_import_clause(import_clause) {
                if import_clause.name().is_some() {
                    self.check_import_binding(import_clause);
                }
                let mut needs_import_star = false;
                let named_bindings = import_clause.named_bindings();
                if named_bindings.is_some() {
                    if is_namespace_import(named_bindings) {
                        self.check_import_binding(named_bindings);
                        if get_emit_module_format_of_file(get_source_file_of_node(node))
                            == ModuleKind::COMMON_JS
                        {
                            // import * as ns from "foo";
                            needs_import_star = true;
                            self.check_external_emit_helpers(
                                node,
                                ExternalEmitHelpers::IMPORT_STAR,
                            );
                        }
                    } else {
                        let import_attributes_type =
                            self.get_type_from_import_attributes(attributes);
                        resolved_module = self.resolve_external_module_name(
                            node,
                            node.module_specifier(),
                            false,
                            import_attributes_type,
                        );
                        if resolved_module.is_some() {
                            for binding in named_bindings.elements() {
                                self.check_import_binding(binding);
                            }
                        }
                    }
                }
                if import_clause.name().is_some()
                    && !needs_import_star
                    && get_emit_module_format_of_file(get_source_file_of_node(node))
                        == ModuleKind::COMMON_JS
                {
                    // import d from "foo";
                    self.check_external_emit_helpers(node, ExternalEmitHelpers::IMPORT_DEFAULT);
                }

                if !import_clause.is_type_only()
                    && ModuleKind::NODE18 <= self.module_kind
                    && self.module_kind <= ModuleKind::NODE_NEXT
                    && {
                        let import_attributes_type =
                            self.get_type_from_import_attributes(attributes);
                        self.is_only_importable_as_default(
                            module_specifier,
                            resolved_module,
                            import_attributes_type,
                        )
                    }
                    && !has_type_json_import_attribute(node)
                {
                    let module_kind = self.module_kind.string();
                    self.error(
                        module_specifier,
                        diag::Importing_a_JSON_file_into_an_ECMAScript_module_requires_a_type_Colon_json_import_attribute_when_module_is_set_to_0,
                        args![module_kind],
                    );
                }
            } else if self
                .compiler_options
                .no_unchecked_side_effect_imports
                .is_true_or_unknown()
                && import_clause.is_nil()
            {
                let ignore_errors = self.compiler_options.no_check.is_true();
                let mut error_message: Option<&'static Message> = None;
                if !ignore_errors {
                    error_message = Some(
                        diag::Cannot_find_module_or_type_declarations_for_side_effect_import_of_0,
                    );
                }
                let import_attributes_type = self.get_type_from_import_attributes(attributes);
                self.resolve_external_module_name_worker(
                    node,
                    module_specifier,
                    error_message,
                    ignore_errors,
                    false, /*isForAugmentation*/
                    import_attributes_type,
                );
            }
        }
        self.check_import_attributes(node);
    }

    // Go: checker/checker.go:5467 checkExternalImportOrExportDeclaration
    pub fn check_external_import_or_export_declaration(&mut self, node: Node) -> bool {
        let module_name = get_external_module_name(node);
        if module_name.is_nil() || node_is_missing(module_name) {
            // Should be a parse error.
            return false;
        }
        if !is_string_literal(module_name) {
            self.error(module_name, diag::String_literal_expected, args![]);
            return false;
        }
        let in_ambient_external_module =
            is_module_block(node.parent()) && is_ambient_module(node.parent().parent());
        if !is_source_file(node.parent()) && !in_ambient_external_module {
            self.error(
                module_name,
                if is_export_declaration(node) {
                    diag::Export_declarations_are_not_permitted_in_a_namespace
                } else {
                    diag::Import_declarations_in_a_namespace_cannot_reference_a_module
                },
                args![],
            );
            return false;
        }
        if in_ambient_external_module && is_external_module_name_relative_p06(module_name.text()) {
            // we have already reported errors on top level imports/exports in external module augmentations in checkModuleDeclaration
            // no need to do this again.
            if !is_top_level_in_external_module_augmentation(node) {
                // TypeScript 1.0 spec (April 2013): 12.1.6
                // An ExternalImportDeclaration in an AmbientExternalModuleDeclaration may reference
                // other external modules only through top - level external module names.
                // Relative external module names are not permitted.
                self.error(
                    node,
                    diag::Import_or_export_declaration_in_an_ambient_module_declaration_cannot_reference_module_through_relative_module_name,
                    args![],
                );
                return false;
            }
        }
        if !is_import_equals_declaration(node) {
            let attributes = get_import_attributes(node);
            if attributes.is_some() {
                if self.check_grammar_import_attribute_values(attributes) {
                    return false;
                }
            }
        }
        true
    }

    // Go: checker/checker.go:5505 checkImportBinding
    pub fn check_import_binding(&mut self, node: Node) {
        self.check_collisions_for_declaration_name(node, node.name());
        self.check_alias_symbol(node);
        if is_import_specifier(node) {
            self.check_module_export_name(node.property_name(), true /*allowStringLiteral*/);
            if module_export_name_is_default(node.property_name_or_name())
                && get_emit_module_format_of_file(get_source_file_of_node(node))
                    == ModuleKind::COMMON_JS
            {
                self.check_external_emit_helpers(node, ExternalEmitHelpers::IMPORT_DEFAULT);
            }
        }
    }

    // Go: checker/checker.go:5517 checkModuleExportName
    pub fn check_module_export_name(&mut self, name: Node, allow_string_literal: bool) {
        if name.is_nil() || name.kind() != SyntaxKind::StringLiteral {
            return;
        }
        if !allow_string_literal {
            self.grammar_error_on_node(name, diag::Identifier_expected, args![]);
        } else if self.module_kind == ModuleKind::ES2015 || self.module_kind == ModuleKind::ES2020 {
            if !with_source_file_info(get_source_file_of_node(name), |info| {
                info.is_declaration_file
            }) {
                self.grammar_error_on_node(
                    name,
                    diag::String_literal_import_and_export_names_are_not_supported_when_the_module_flag_is_set_to_es2015_or_es2020,
                    args![],
                );
            }
        }
    }
}

// Go: checker/checker.go:5530 hasTypeJsonImportAttribute
pub fn has_type_json_import_attribute(node: Node) -> bool {
    // PORT: Go reads `node.AsImportDeclaration().Attributes`; for the
    // ImportDeclaration kinds this is exactly `ast.GetImportAttributes(node)`.
    let attributes = get_import_attributes(node);
    attributes.is_some()
        && import_attributes_list_p06(attributes)
            .into_iter()
            .any(|attr| {
                attr.name().text() == "type"
                    && is_string_literal_like(attr.value())
                    && attr.value().text() == "json"
            })
}

impl Checker {
    // Go: checker/checker.go:5537 checkImportAttributes
    pub fn check_import_attributes(&mut self, declaration: Node) {
        let node = get_import_attributes(declaration);
        if node.is_nil() {
            return;
        }
        let import_attributes_type = self.get_global_import_attributes_type_checked();
        if import_attributes_type != self.empty_object_type {
            let source = self.get_type_from_import_attributes(node);
            let target = self.get_nullable_type(import_attributes_type, TypeFlags::UNDEFINED);
            self.check_type_assignable_to(source, target, node, None);
        }
        let is_type_only = is_exclusively_type_only_import_or_export(declaration)
            || is_import_type_node(declaration);
        let override_ = self.get_resolution_mode_override(node, is_type_only);
        if is_type_only {
            return; // Other grammar checks do not apply to type-only imports with import attributes
        }

        if !self.module_kind.supports_import_attributes() {
            self.grammar_error_on_node(
                node,
                diag::Import_attributes_are_only_supported_when_the_module_option_is_set_to_esnext_node18_node20_nodenext_or_preserve,
                args![],
            );
            return;
        }

        let module_specifier = get_external_module_name(declaration);
        if module_specifier.is_some() {
            if self.get_emit_syntax_for_module_specifier_expression(module_specifier)
                == ModuleKind::COMMON_JS
            {
                self.grammar_error_on_node(
                    node,
                    diag::Import_attributes_are_not_allowed_on_statements_that_compile_to_CommonJS_require_calls,
                    args![],
                );
                return;
            }
        }

        if override_ != RESOLUTION_MODE_NONE {
            self.grammar_error_on_node(
                node,
                diag::X_resolution_mode_can_only_be_set_for_type_only_imports,
                args![],
            );
        }
    }

    // Go: checker/checker.go:5569 getTypeFromImportAttributes
    pub fn get_type_from_import_attributes(&mut self, node: Node) -> TypeId {
        if node.is_nil() {
            return TypeId::NIL;
        }
        if is_import_attributes(node) {
            return self.check_import_attributes_expression(node);
        }
        self.check_expression_cached(node)
    }

    // Go: checker/checker.go:5579 checkImportAttributesExpression
    pub fn check_import_attributes_expression(&mut self, node: Node) -> TypeId {
        if self.type_node_links.get(node).resolved_type.is_nil() {
            let symbol = self.new_symbol(
                SymbolFlags::OBJECT_LITERAL,
                INTERNAL_SYMBOL_NAME_IMPORT_ATTRIBUTES,
            );
            let members = self.symbols.new_table();
            for attribute in import_attributes_list_p06(node) {
                let member = self.new_symbol(SymbolFlags::PROPERTY, attribute.name().text());
                // Go reads the links (and gives the id) before the right side.
                self.value_symbol_links.get_by_id(&self.symbols, member);
                let value_type = self.check_expression_cached(attribute.value());
                let resolved_type = self.get_regular_type_of_literal_type(value_type);
                self.value_symbol_links
                    .get_by_id(&self.symbols, member)
                    .resolved_type = resolved_type;
                let member_name = self.sym(member).name.clone();
                self.symbols.set(members, member_name, member);
            }
            let t = self.new_anonymous_type(symbol, members, &[], &[], &[]);
            self.ty_mut(t).object_flags |=
                ObjectFlags::OBJECT_LITERAL | ObjectFlags::NON_INFERRABLE_TYPE;
            self.type_node_links.get(node).resolved_type = t;
        }
        self.type_node_links.get(node).resolved_type
    }

    // Go: checker/checker.go:5596 getImportAttributesTypeForModuleSpecifier
    pub fn get_import_attributes_type_for_module_specifier(
        &mut self,
        module_specifier: Node,
    ) -> TypeId {
        let parent = module_specifier.parent();
        if is_import_declaration_or_js_import_declaration(parent) || is_export_declaration(parent) {
            return self.get_type_from_import_attributes(get_import_attributes(parent));
        }
        if is_literal_type_node(parent) && is_literal_import_type_node(parent.parent()) {
            return self.get_type_from_import_attributes(get_import_attributes(parent.parent()));
        }
        if is_import_call(parent) && parent.arguments().len() > 1 {
            let options = parent.arguments().get(1);
            let options_type = self.check_expression_cached(options);
            return self.get_type_of_property_of_type(options_type, "with");
        }
        TypeId::NIL
    }

    // Go: checker/checker.go:5610 checkImportEqualsDeclaration
    pub fn check_import_equals_declaration(&mut self, node: Node) {
        let diagnostic = if is_in_js_file(node) {
            diag::An_import_declaration_can_only_be_used_at_the_top_level_of_a_module
        } else {
            diag::An_import_declaration_can_only_be_used_at_the_top_level_of_a_namespace_or_module
        };
        if self.check_grammar_module_element_context(node, diagnostic) {
            self.check_external_module_name_in_global_scope(node);
            return; // If we hit an import declaration in an illegal context, just bail out to avoid cascading errors.
        }
        self.check_grammar_modifiers(node);
        if self.should_check_erasable_syntax(node) && !node.flags().intersects(NodeFlags::AMBIENT) {
            self.error(
                node,
                diag::This_syntax_is_not_allowed_when_erasableSyntaxOnly_is_enabled,
                args![],
            );
        }
        if is_internal_module_import_equals_declaration(node)
            || self.check_external_import_or_export_declaration(node)
        {
            self.check_import_binding(node);
            self.mark_linked_references(
                node,
                ReferenceHint::EXPORT_IMPORT_EQUALS,
                SymbolId::NIL,
                TypeId::NIL,
            );
            let module_reference = node.module_reference();
            if !is_external_module_reference(module_reference) {
                let node_symbol = self.get_symbol_of_declaration(node);
                let target = self.resolve_alias(node_symbol);
                if target != self.unknown_symbol {
                    let target_flags = self.get_symbol_flags(target);
                    if target_flags.intersects(SymbolFlags::VALUE) {
                        // Target is a value symbol, check that it is not hidden by a local declaration with the same name
                        let module_name = get_first_identifier(module_reference);
                        let resolved = self.resolve_entity_name(
                            module_name,
                            SymbolFlags::VALUE | SymbolFlags::NAMESPACE,
                            false,
                            false,
                            Node::NIL,
                        );
                        if !self.sym(resolved).flags.intersects(SymbolFlags::NAMESPACE) {
                            self.error(
                                module_name,
                                diag::Module_0_is_hidden_by_a_local_declaration_with_the_same_name,
                                args![declaration_name_to_string(module_name)],
                            );
                        }
                    }
                    if target_flags.intersects(SymbolFlags::TYPE) {
                        self.check_type_name_is_reserved(
                            node.name(),
                            diag::Import_name_cannot_be_0,
                        );
                    }
                }
                if node.is_type_only() {
                    self.grammar_error_on_node(
                        node,
                        diag::An_import_alias_cannot_use_import_type,
                        args![],
                    );
                }
            } else {
                if ModuleKind::ES2015 <= self.module_kind
                    && self.module_kind <= ModuleKind::ES_NEXT
                    && !node.is_type_only()
                    && !node.flags().intersects(NodeFlags::AMBIENT)
                {
                    // Import equals declaration cannot be emitted as ESM
                    self.grammar_error_on_node(
                        node,
                        diag::Import_assignment_cannot_be_used_when_targeting_ECMAScript_modules_Consider_using_import_Asterisk_as_ns_from_mod_import_a_from_mod_import_d_from_mod_or_another_module_format_instead,
                        args![],
                    );
                }
            }
        }
    }

    // Go: checker/checker.go:5653 checkExportDeclaration
    pub fn check_export_declaration(&mut self, node: Node) {
        let diagnostic = if is_in_js_file(node) {
            diag::An_export_declaration_can_only_be_used_at_the_top_level_of_a_module
        } else {
            diag::An_export_declaration_can_only_be_used_at_the_top_level_of_a_namespace_or_module
        };
        if self.check_grammar_module_element_context(node, diagnostic) {
            self.check_external_module_name_in_global_scope(node);
            return; // If we hit an export in an illegal context, just bail out to avoid cascading errors.
        }
        if !self.check_grammar_modifiers(node) && !node.modifiers().is_nil() {
            self.grammar_error_on_first_token(
                node,
                diag::An_export_declaration_cannot_have_modifiers,
                args![],
            );
        }
        self.check_grammar_export_declaration(node);
        let module_specifier = node.module_specifier();
        let export_clause = node.export_clause();
        if module_specifier.is_nil() || self.check_external_import_or_export_declaration(node) {
            if export_clause.is_some() && !is_namespace_export(export_clause) {
                // export { x, y }
                // export { x, y } from "foo"
                for binding in export_clause.elements() {
                    self.check_export_specifier(binding);
                }
                let in_ambient_external_module =
                    is_module_block(node.parent()) && is_ambient_module(node.parent().parent());
                let in_ambient_namespace_declaration = !in_ambient_external_module
                    && is_module_block(node.parent())
                    && module_specifier.is_nil()
                    && node.flags().intersects(NodeFlags::AMBIENT);
                if !is_source_file(node.parent())
                    && !in_ambient_external_module
                    && !in_ambient_namespace_declaration
                {
                    self.error(
                        node,
                        diag::Export_declarations_are_not_permitted_in_a_namespace,
                        args![],
                    );
                }
            } else {
                // export * from "foo"
                // export * as ns from "foo";
                let import_attributes_type =
                    self.get_type_from_import_attributes(get_import_attributes(node));
                let module_symbol = self.resolve_external_module_name(
                    node,
                    module_specifier,
                    false,
                    import_attributes_type,
                );
                if module_symbol.is_some() && self.has_export_assignment_symbol(module_symbol) {
                    let s = self.symbol_to_string(module_symbol);
                    self.error(
                        module_specifier,
                        diag::Module_0_uses_export_and_cannot_be_used_with_export_Asterisk,
                        args![s],
                    );
                } else if export_clause.is_some() {
                    self.check_alias_symbol(export_clause);
                    self.check_module_export_name(
                        export_clause.name(),
                        true, /*allowStringLiteral*/
                    );
                }
                if get_emit_module_format_of_file(get_source_file_of_node(node))
                    == ModuleKind::COMMON_JS
                {
                    if node.export_clause().is_some() {
                        // export * as ns from "foo";
                        self.check_external_emit_helpers(node, ExternalEmitHelpers::IMPORT_STAR);
                    } else {
                        // export * from "foo"
                        self.check_external_emit_helpers(node, ExternalEmitHelpers::EXPORT_STAR);
                    }
                }
            }
        }
        self.check_import_attributes(node);
    }

    // Go: checker/checker.go:5702 checkExternalModuleNameInGlobalScope
    pub fn check_external_module_name_in_global_scope(&mut self, node: Node) {
        if get_enclosing_container(node).kind() != SyntaxKind::SourceFile
            || (is_import_declaration_or_js_import_declaration(node)
                && node.import_clause().is_nil())
        {
            return;
        }
        let module_name = get_external_module_name(node);
        if module_name.is_some() {
            let mut attributes = Node::NIL;
            if has_import_attributes(node) {
                attributes = get_import_attributes(node);
            }
            let import_attributes_type = self.get_type_from_import_attributes(attributes);
            self.resolve_external_module_name(node, module_name, false, import_attributes_type);
        }
    }

    // Go: checker/checker.go:5716 checkExportSpecifier
    pub fn check_export_specifier(&mut self, node: Node) {
        self.check_alias_symbol(node);
        let has_module_specifier = node.parent().parent().module_specifier().is_some();
        self.check_module_export_name(node.property_name(), has_module_specifier);
        self.check_module_export_name(node.name(), true /*allowStringLiteral*/);

        if !has_module_specifier {
            let exported_name = node.property_name_or_name();
            if exported_name.kind() == SyntaxKind::StringLiteral {
                return; // Skip for invalid syntax like this: export { "x" }
            }
            // find immediate value referenced by exported name (SymbolFlags.Alias is set so we don't chase down aliases)
            let symbol = self.resolve_name(
                exported_name,
                exported_name.text(),
                SymbolFlags::VALUE
                    | SymbolFlags::TYPE
                    | SymbolFlags::NAMESPACE
                    | SymbolFlags::ALIAS,
                None, /*nameNotFoundMessage*/
                true, /*isUse*/
                false,
            );
            if symbol.is_some()
                && (symbol == self.undefined_symbol
                    || symbol == self.global_this_symbol
                    || !self.sym(symbol).declarations.is_empty()
                        && is_global_source_file(get_declaration_container(
                            self.sym(symbol).declarations[0],
                        )))
            {
                self.error(
                    exported_name,
                    diag::Cannot_export_0_Only_local_declarations_can_be_exported_from_a_module,
                    args![exported_name.text()],
                );
            } else {
                self.mark_linked_references(
                    node,
                    ReferenceHint::EXPORT_SPECIFIER,
                    SymbolId::NIL, /*propSymbol*/
                    TypeId::NIL,   /*parentType*/
                );
            }
        } else if get_emit_module_format_of_file(get_source_file_of_node(node))
            == ModuleKind::COMMON_JS
            && module_export_name_is_default(node.property_name_or_name())
        {
            self.check_external_emit_helpers(node, ExternalEmitHelpers::IMPORT_DEFAULT);
        }
    }
}

// Go: checker/checker.go:5740 isContainedByNamespace
pub fn is_contained_by_namespace(node: Node) -> bool {
    let mut container = node.parent();
    if !is_source_file(container) {
        container = container.parent();
    }
    is_module_declaration(container) && !is_ambient_module(container)
}

impl Checker {
    // Go: checker/checker.go:5748 checkExportAssignment
    pub fn check_export_assignment(&mut self, node: Node) {
        let is_export_equals = node.is_export_equals();
        // Always check the exported expression so its identifiers are resolved even when the
        // export assignment is misplaced (grammar error), keeping diagnostics stable
        // regardless of traversal order.
        let expr_type = self.check_expression_cached(node.expression());
        let illegal_context_message = if is_export_equals {
            diag::An_export_assignment_must_be_at_the_top_level_of_a_file_or_module_declaration
        } else {
            diag::A_default_export_must_be_at_the_top_level_of_a_file_or_module_declaration
        };
        if self.check_grammar_module_element_context(node, illegal_context_message) {
            return; // If we hit an export assignment in an illegal context, just bail out to avoid cascading errors.
        }
        if self.should_check_erasable_syntax(node)
            && node.is_export_equals()
            && !node.flags().intersects(NodeFlags::AMBIENT)
        {
            self.error(
                node,
                diag::This_syntax_is_not_allowed_when_erasableSyntaxOnly_is_enabled,
                args![],
            );
        }
        if is_contained_by_namespace(node) {
            // Go upstream note (danielr): should these be grammar errors?
            if is_export_equals {
                self.error(
                    node,
                    diag::An_export_assignment_cannot_be_used_in_a_namespace,
                    args![],
                );
            } else {
                self.error(
                    node,
                    diag::A_default_export_can_only_be_used_in_an_ECMAScript_style_module,
                    args![],
                );
            }
            return;
        }
        if !self.check_grammar_modifiers(node)
            && is_export_assignment(node)
            && !node.modifiers().is_nil()
        {
            self.grammar_error_on_first_token(
                node,
                diag::An_export_assignment_cannot_have_modifiers,
                args![],
            );
        }
        let is_illegal_export_default_in_cjs = !is_export_equals
            && !node.flags().intersects(NodeFlags::AMBIENT)
            && self.compiler_options.verbatim_module_syntax.is_true()
            && get_emit_module_format_of_file(get_source_file_of_node(node))
                == ModuleKind::COMMON_JS;
        if is_identifier(node.expression()) {
            let id = node.expression();
            let resolved = self.resolve_entity_name(
                id,
                SymbolFlags::ALL,
                true, /*ignoreErrors*/
                true, /*dontResolveAlias*/
                node,
            );
            let sym = self.get_export_symbol_of_value_symbol_if_exported(resolved);
            if sym.is_some() {
                self.mark_linked_references(
                    node,
                    ReferenceHint::EXPORT_ASSIGNMENT,
                    SymbolId::NIL,
                    TypeId::NIL,
                );
                let type_only_declaration =
                    self.get_type_only_alias_declaration_ex(sym, SymbolFlags::VALUE);
                // If not a value, we're interpreting the identifier as a type export, along the lines of (`export { Id as default }`)
                if self.get_symbol_flags(sym).intersects(SymbolFlags::VALUE) {
                    // However if it is a value, we need to check it's being used correctly
                    if !is_illegal_export_default_in_cjs
                        && !node.flags().intersects(NodeFlags::AMBIENT)
                        && self.compiler_options.verbatim_module_syntax.is_true()
                        && type_only_declaration.is_some()
                    {
                        let message = if is_export_equals {
                            diag::An_export_declaration_must_reference_a_real_value_when_verbatimModuleSyntax_is_enabled_but_0_resolves_to_a_type_only_declaration
                        } else {
                            diag::An_export_default_must_reference_a_real_value_when_verbatimModuleSyntax_is_enabled_but_0_resolves_to_a_type_only_declaration
                        };
                        self.error(id, message, args![id.text()]);
                    }
                } else if !is_illegal_export_default_in_cjs
                    && !node.flags().intersects(NodeFlags::AMBIENT)
                    && self.compiler_options.verbatim_module_syntax.is_true()
                {
                    let message = if is_export_equals {
                        diag::An_export_declaration_must_reference_a_value_when_verbatimModuleSyntax_is_enabled_but_0_only_refers_to_a_type
                    } else {
                        diag::An_export_default_must_reference_a_value_when_verbatimModuleSyntax_is_enabled_but_0_only_refers_to_a_type
                    };
                    self.error(id, message, args![id.text()]);
                }
                if !is_illegal_export_default_in_cjs
                    && !node.flags().intersects(NodeFlags::AMBIENT)
                    && self.compiler_options.get_isolated_modules()
                    && !self.sym(sym).flags.intersects(SymbolFlags::VALUE)
                {
                    let non_local_meanings = self.get_symbol_flags_ex(
                        sym, false, /*excludeTypeOnlyMeanings*/
                        true,  /*excludeLocalMeanings*/
                    );
                    if self.sym(sym).flags.intersects(SymbolFlags::ALIAS)
                        && non_local_meanings.intersects(SymbolFlags::TYPE)
                        && !non_local_meanings.intersects(SymbolFlags::VALUE)
                        && (type_only_declaration.is_nil()
                            || get_source_file_of_node(type_only_declaration)
                                != get_source_file_of_node(node))
                    {
                        // import { SomeType } from "./someModule";
                        // export default SomeType; OR
                        // export = SomeType;
                        let message = if is_export_equals {
                            diag::X_0_resolves_to_a_type_and_must_be_marked_type_only_in_this_file_before_re_exporting_when_1_is_enabled_Consider_using_import_type_where_0_is_imported
                        } else {
                            diag::X_0_resolves_to_a_type_and_must_be_marked_type_only_in_this_file_before_re_exporting_when_1_is_enabled_Consider_using_export_type_0_as_default
                        };
                        let flag_name = self.get_isolated_modules_like_flag_name();
                        self.error(id, message, args![id.text(), flag_name]);
                    } else if type_only_declaration.is_some()
                        && get_source_file_of_node(type_only_declaration)
                            != get_source_file_of_node(node)
                    {
                        // import { SomeTypeOnlyValue } from "./someModule";
                        // export default SomeTypeOnlyValue; OR
                        // export = SomeTypeOnlyValue;
                        let message = if is_export_equals {
                            diag::X_0_resolves_to_a_type_only_declaration_and_must_be_marked_type_only_in_this_file_before_re_exporting_when_1_is_enabled_Consider_using_import_type_where_0_is_imported
                        } else {
                            diag::X_0_resolves_to_a_type_only_declaration_and_must_be_marked_type_only_in_this_file_before_re_exporting_when_1_is_enabled_Consider_using_export_type_0_as_default
                        };
                        let flag_name = self.get_isolated_modules_like_flag_name();
                        // PORT: Go adds related info to the diagnostic returned by
                        // `c.error`. Build, annotate, then add (same steps as `c.error`).
                        let diagnostic =
                            new_diagnostic_for_node(id, message, args![id.text(), flag_name]);
                        let diagnostic = self.add_type_only_declaration_related_info(
                            diagnostic,
                            type_only_declaration,
                            id.text(),
                        );
                        self.add_diagnostic(diagnostic);
                    }
                }
            }
        }
        if is_illegal_export_default_in_cjs {
            self.error(
                node,
                get_verbatim_module_syntax_error_message(node),
                args![],
            );
        }
        let mut container = node.parent();
        if !is_source_file(container) {
            container = container.parent();
        }
        self.check_external_module_exports(container);
        let type_node = node.type_();
        if type_node.is_some() && node.kind() == SyntaxKind::ExportAssignment {
            let t = self.get_type_from_type_node(type_node);
            self.check_type_assignable_to_and_optionally_elaborate(
                expr_type,
                t,
                node.expression(),
                node.expression(),
                None, /*headMessage*/
                None,
            );
        }
        if node.flags().intersects(NodeFlags::AMBIENT)
            && !is_entity_name_expression(node.expression())
        {
            self.grammar_error_on_node(
                node.expression(),
                diag::The_expression_of_an_export_assignment_must_be_an_identifier_or_qualified_name_in_an_ambient_context,
                args![],
            );
        }
        if is_export_equals {
            // Forbid export= in esm implementation files, and esm mode declaration files
            if self.module_kind >= ModuleKind::ES2015
                && self.module_kind != ModuleKind::PRESERVE
                && ((node.flags().intersects(NodeFlags::AMBIENT)
                    && get_implied_node_format_for_emit(get_source_file_of_node(node))
                        == ModuleKind::ES_NEXT)
                    || (!node.flags().intersects(NodeFlags::AMBIENT)
                        && get_implied_node_format_for_emit(get_source_file_of_node(node))
                            != ModuleKind::COMMON_JS))
            {
                // export assignment is not supported in es6 modules
                self.grammar_error_on_node(
                    node,
                    diag::Export_assignment_cannot_be_used_when_targeting_ECMAScript_modules_Consider_using_export_default_or_another_module_format_instead,
                    args![],
                );
            } else if self.module_kind == ModuleKind::SYSTEM
                && !node.flags().intersects(NodeFlags::AMBIENT)
            {
                // system modules does not support export assignment
                self.grammar_error_on_node(
                    node,
                    diag::Export_assignment_is_not_supported_when_module_flag_is_system,
                    args![],
                );
            }
        }
    }
}

// Go: checker/checker.go:5846 getVerbatimModuleSyntaxErrorMessage
pub fn get_verbatim_module_syntax_error_message(node: Node) -> &'static Message {
    let source_file = get_source_file_of_node(node);
    let file_name = source_file_file_name(source_file);

    // Check if the file is .cts or .cjs (CommonJS-specific extensions)
    if file_extension_is_one_of_p06(file_name, &[".cts", ".cjs"]) {
        return diag::ECMAScript_imports_and_exports_cannot_be_written_in_a_CommonJS_file_under_verbatimModuleSyntax;
    }
    // For .ts, .tsx, .js, etc.
    diag::ECMAScript_imports_and_exports_cannot_be_written_in_a_CommonJS_file_under_verbatimModuleSyntax_Adjust_the_type_field_in_the_nearest_package_json_to_make_this_file_an_ECMAScript_module_or_adjust_your_verbatimModuleSyntax_module_and_moduleResolution_settings_in_TypeScript
}

impl Checker {
    // Go: checker/checker.go:5858 checkExternalModuleExports
    pub fn check_external_module_exports(&mut self, node: Node) {
        let module_symbol = self.get_symbol_of_declaration(node);
        if !self.module_symbol_links.get(module_symbol).exports_checked {
            let module_exports = self.sym(module_symbol).exports;
            let export_equals_symbol = self
                .symbols
                .get(module_exports, INTERNAL_SYMBOL_NAME_EXPORT_EQUALS);
            // An export assignment is in error if (a) the module exports value members or (b) if the module exports type or
            // namespace members and the exported entity also exports type or namespace members.
            if export_equals_symbol.is_some()
                && (self.has_exported_members_of_kind(module_symbol, SymbolFlags::VALUE)
                    || self.has_shadowed_namespace(export_equals_symbol))
            {
                let alias_declaration = self.get_declaration_of_alias_symbol(export_equals_symbol);
                let declaration = if alias_declaration.is_some() {
                    alias_declaration
                } else {
                    self.sym(export_equals_symbol).value_declaration
                };
                if declaration.is_some()
                    && !is_top_level_in_external_module_augmentation(declaration)
                {
                    self.error(declaration, diag::An_export_assignment_cannot_be_used_in_a_module_with_other_exported_elements, args![]);
                }
            }
            // Checks for export * conflicts
            let exports = self.get_exports_of_module(module_symbol);
            // PORT: Go map iteration order is random; this uses table insertion order.
            for (id, symbol) in self.symbols.entries(exports) {
                if id == INTERNAL_SYMBOL_NAME_EXPORT_STAR {
                    continue;
                }
                // ECMA262: 15.2.1.1 It is a Syntax Error if the ExportedNames of ModuleItemList contains any duplicate entries.
                // (TS Exceptions: namespaces, function overloads, enums, and interfaces)
                if self
                    .sym(symbol)
                    .flags
                    .intersects(SymbolFlags::NAMESPACE | SymbolFlags::ENUM)
                {
                    continue;
                }
                let declarations = self.sym(symbol).declarations.clone();
                let exported_declarations_count = declarations
                    .iter()
                    .filter(|&&d| {
                        is_not_overload(d) && !is_accessor(d) && !is_interface_declaration(d)
                    })
                    .count();
                if self.sym(symbol).flags.intersects(SymbolFlags::TYPE_ALIAS)
                    && exported_declarations_count <= 2
                {
                    // it is legal to merge type alias with other values
                    // so count should be either 1 (just type alias) or 2 (type alias + merged value)
                    continue;
                }
                if exported_declarations_count > 1
                    && !declarations.iter().all(|&node| {
                        get_assignment_declaration_kind(node) == JSDeclarationKind::EXPORTS_PROPERTY
                    })
                {
                    for &declaration in &declarations {
                        if is_not_overload(declaration) {
                            self.error(
                                declaration,
                                diag::Cannot_redeclare_exported_variable_0,
                                args![id],
                            );
                        }
                    }
                }
            }
            self.module_symbol_links.get(module_symbol).exports_checked = true;
        }
    }
}
