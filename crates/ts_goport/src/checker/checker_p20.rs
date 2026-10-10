//! Port of `checker/checker.go` lines 17614-18535: binding element and
//! binding pattern types, assignment declaration types, implicit any
//! reporting, type widening, and accessor and alias symbol types.

use crate::prelude::*;

impl Checker {
    // Go: checker/checker.go:18043 getBindingElementTypeFromParentType
    pub fn get_binding_element_type_from_parent_type(
        &mut self,
        declaration: Node,
        parent_type: TypeId,
        no_tuple_bounds_check: bool,
    ) -> TypeId {
        let mut parent_type = parent_type;
        // If an any type was inferred for parent, infer that for the binding element
        if self.is_type_any(parent_type) {
            return parent_type;
        }
        let pattern = declaration.parent();
        // Relax null check on ambient destructuring parameters, since the parameters have no implementation and are just documentation
        if self.strict_null_checks
            && declaration.flags().intersects(NodeFlags::AMBIENT)
            && is_part_of_parameter_declaration(declaration)
        {
            parent_type = self.get_non_nullable_type(parent_type);
        } else if self.strict_null_checks && pattern.parent().initializer().is_some() && {
            let init_type = self.get_type_of_initializer(pattern.parent().initializer());
            !self.has_type_facts(init_type, TypeFacts::EQ_UNDEFINED)
        } {
            parent_type = self.get_type_with_facts(parent_type, TypeFacts::NE_UNDEFINED);
        }
        let access_flags = AccessFlags::EXPRESSION_POSITION
            | if no_tuple_bounds_check || self.has_default_value(declaration) {
                AccessFlags::ALLOW_MISSING
            } else {
                AccessFlags::NONE
            };
        let t: TypeId;
        match pattern.kind() {
            SyntaxKind::ObjectBindingPattern => {
                if has_dot_dot_dot_token(declaration) {
                    parent_type = self.get_reduced_type(parent_type);
                    if self.ty(parent_type).flags.intersects(TypeFlags::UNKNOWN)
                        || !self.is_valid_spread_type(parent_type)
                    {
                        self.error(
                            declaration,
                            diag::Rest_types_may_only_be_created_from_object_types,
                            args![],
                        );
                        return self.error_type;
                    }
                    let elements = pattern.elements();
                    let mut literal_members: Vec<Node> = Vec::with_capacity(elements.len());
                    for element in elements {
                        if !has_dot_dot_dot_token(element) {
                            let name = element.property_name_or_name();
                            literal_members.push(name);
                        }
                    }
                    t = self.get_rest_type(parent_type, &literal_members, declaration.symbol());
                } else {
                    // Use explicitly specified property name ({ p: xxx } form), or otherwise the implied name ({ p } form)
                    let name = declaration.property_name_or_name();
                    let index_type = self.get_literal_type_from_property_name(name);
                    let declared_type = self.get_indexed_access_type_ex(
                        parent_type,
                        index_type,
                        access_flags,
                        name,
                        None,
                    );
                    t = self.get_flow_type_of_destructuring(declaration, declared_type);
                }
            }
            SyntaxKind::ArrayBindingPattern => {
                // This elementType will be used if the specific property corresponding to this index is not
                // present (aka the tuple element property). This call also checks that the parentType is in
                // fact an iterable or array (depending on target language).
                let use_ = IterationUse::DESTRUCTURING
                    | if has_dot_dot_dot_token(declaration) {
                        IterationUse(0)
                    } else {
                        IterationUse::POSSIBLY_OUT_OF_BOUNDS
                    };
                let undefined_type = self.undefined_type;
                let element_type = self.check_iterated_type_or_element_type(
                    use_,
                    parent_type,
                    undefined_type,
                    pattern,
                );
                let index: i32 = pattern
                    .elements()
                    .to_vec()
                    .iter()
                    .position(|&e| e == declaration)
                    .map_or(-1, |i| i as i32);
                if has_dot_dot_dot_token(declaration) {
                    // If the parent is a tuple type, the rest element has a tuple type of the
                    // remaining tuple element types. Otherwise, the rest element has an array type with same
                    // element type as the parent type.
                    let base_constraint =
                        self.map_type(parent_type, &mut |c: &mut Checker, t: TypeId| {
                            if c.ty(t)
                                .flags
                                .intersects(TypeFlags::INSTANTIABLE_NON_PRIMITIVE)
                            {
                                return c.get_base_constraint_or_type(t);
                            }
                            t
                        });
                    if self.every_type(base_constraint, &mut |c: &mut Checker, t: TypeId| {
                        c.is_tuple_type(t)
                    }) {
                        t = self.map_type(base_constraint, &mut |c: &mut Checker, t: TypeId| {
                            c.slice_tuple_type(t, index, 0)
                        });
                    } else {
                        t = self.create_array_type(element_type);
                    }
                } else if self.is_array_like_type(parent_type) {
                    let index_type =
                        self.get_number_literal_type(crate::jsnum::Number(f64::from(index)));
                    let mut declared_type = self.get_indexed_access_type_or_undefined(
                        parent_type,
                        index_type,
                        access_flags,
                        declaration.name(),
                        None,
                    );
                    if declared_type.is_nil() {
                        declared_type = self.error_type;
                    }
                    t = self.get_flow_type_of_destructuring(declaration, declared_type);
                } else {
                    t = element_type;
                }
            }
            _ => panic!("Unhandled case in getBindingElementTypeFromParentType"),
        }
        if declaration.initializer().is_nil() {
            return t;
        }
        if walk_up_binding_elements_and_patterns(declaration)
            .type_()
            .is_some()
        {
            // In strict null checking mode, if a default value of a non-undefined type is specified, remove
            // undefined from the final type.
            if self.strict_null_checks {
                let init_type =
                    self.check_declaration_initializer(declaration, CheckMode::NORMAL, TypeId::NIL);
                if !self.has_type_facts(init_type, TypeFacts::IS_UNDEFINED) {
                    return self.get_non_undefined_type(t);
                }
            }
            return t;
        }
        let non_undefined = self.get_non_undefined_type(t);
        let init_type =
            self.check_declaration_initializer(declaration, CheckMode::NORMAL, TypeId::NIL);
        let union = self.get_union_type_ex(
            &[non_undefined, init_type],
            UnionReduction::SUBTYPE,
            None,
            TypeId::NIL,
        );
        self.widen_type_inferred_from_initializer(declaration, union)
    }

    // Go: checker/checker.go:18128 getRestType
    pub fn get_rest_type(
        &mut self,
        source: TypeId,
        properties: &[Node],
        symbol: SymbolId,
    ) -> TypeId {
        let source = self.filter_type(source, &mut |c: &mut Checker, t: TypeId| {
            !c.ty(t).flags.intersects(TypeFlags::NULLABLE)
        });
        if self.ty(source).flags.intersects(TypeFlags::NEVER) {
            return self.empty_object_type;
        }
        if self.ty(source).flags.intersects(TypeFlags::UNION) {
            return self.map_type(source, &mut |c: &mut Checker, t: TypeId| {
                c.get_rest_type(t, properties, symbol)
            });
        }
        let mut literal_types: Vec<TypeId> = Vec::with_capacity(properties.len());
        for &p in properties {
            literal_types.push(self.get_literal_type_from_property_name(p));
        }
        let mut omit_key_type = self.get_union_type(&literal_types);
        let mut spreadable_properties: Vec<SymbolId> = Vec::new();
        let mut unspreadable_to_rest_keys: Vec<TypeId> = Vec::new();
        for prop in self.get_properties_of_type(source) {
            let literal_type_from_property = self.get_literal_type_from_property(
                prop,
                TypeFlags::STRING_OR_NUMBER_LITERAL_OR_UNIQUE,
                false,
            );
            if !self.is_type_assignable_to(literal_type_from_property, omit_key_type)
                && !self
                    .get_declaration_modifier_flags_from_symbol(prop)
                    .intersects(ModifierFlags::PRIVATE | ModifierFlags::PROTECTED)
                && self.is_spreadable_property(prop)
            {
                spreadable_properties.push(prop);
            } else {
                unspreadable_to_rest_keys.push(literal_type_from_property);
            }
        }
        if self.is_generic_object_type(source) || self.is_generic_index_type(omit_key_type) {
            if !unspreadable_to_rest_keys.is_empty() {
                // If the type we're spreading from has properties that cannot
                // be spread into the rest type (e.g. getters, methods), ensure
                // they are explicitly omitted, as they would in the non-generic case.
                let mut types = vec![omit_key_type];
                types.extend(unspreadable_to_rest_keys.iter().copied());
                omit_key_type = self.get_union_type(&types);
            }
            if self.ty(omit_key_type).flags.intersects(TypeFlags::NEVER) {
                return source;
            }
            let omit_type_alias = (self.get_global_omit_symbol.clone())(self);
            if omit_type_alias.is_nil() {
                return self.error_type;
            }
            return self.get_type_alias_instantiation(
                omit_type_alias,
                &[source, omit_key_type],
                None,
            );
        }
        let members = self.symbols.new_table();
        for prop in spreadable_properties {
            let spread = self.get_spread_symbol(prop, false /*readonly*/);
            let name = self.sym(prop).name.clone();
            self.symbols.set(members, name, spread);
        }
        let index_infos = self.get_index_infos_of_type(source);
        let result = self.new_anonymous_type(symbol, members, &[], &[], &index_infos);
        self.ty_mut(result).object_flags |= ObjectFlags::OBJECT_REST_TYPE;
        result
    }

    // Determine the control flow type associated with a destructuring declaration or assignment. The following
    // forms of destructuring are possible:
    //
    //	let { x } = obj;  // BindingElement
    //	let [ x ] = obj;  // BindingElement
    //	{ x } = obj;      // ShorthandPropertyAssignment
    //	{ x: v } = obj;   // PropertyAssignment
    //	[ x ] = obj;      // Expression
    //
    // We construct a synthetic element access expression corresponding to 'obj.x' such that the control
    // flow analyzer doesn't have to handle all the different syntactic forms.
    // Go: checker/checker.go:18185 getFlowTypeOfDestructuring
    pub fn get_flow_type_of_destructuring(&mut self, node: Node, declared_type: TypeId) -> TypeId {
        let reference = self.get_synthetic_element_access(node);
        if reference.is_some() {
            return self.get_flow_type_of_reference(reference, declared_type);
        }
        declared_type
    }

    // Go: checker/checker.go:18193 getSyntheticElementAccess
    pub fn get_synthetic_element_access(&mut self, node: Node) -> Node {
        let parent_access = self.get_parent_element_access(node);
        if parent_access.is_some() && get_flow_node_of_node(parent_access).is_some() {
            let (prop_name, ok) = self.get_destructuring_property_name(node);
            if ok {
                let literal = self.factory.new_string_literal(prop_name, TokenFlags::NONE);
                set_node_loc(literal, node.loc());
                let mut lhs_expr = parent_access;
                if !is_left_hand_side_expression(parent_access) {
                    lhs_expr = self.factory.new_parenthesized_expression(parent_access);
                    set_node_loc(lhs_expr, node.loc());
                }
                let result = self.factory.new_element_access_expression(
                    lhs_expr,
                    Node::NIL,
                    literal,
                    NodeFlags::NONE,
                );
                set_node_loc(result, node.loc());
                set_node_parent(literal, result);
                set_node_parent(result, node);
                if lhs_expr != parent_access {
                    set_node_parent(lhs_expr, result);
                }
                set_node_flow_node(result, get_flow_node_of_node(parent_access));
                return result;
            }
        }
        Node::NIL
    }

    // Go: checker/checker.go:18218 getParentElementAccess
    pub fn get_parent_element_access(&mut self, node: Node) -> Node {
        let ancestor = node.parent().parent();
        match ancestor.kind() {
            SyntaxKind::BindingElement | SyntaxKind::PropertyAssignment => {
                self.get_synthetic_element_access(ancestor)
            }
            SyntaxKind::ArrayLiteralExpression => self.get_synthetic_element_access(node.parent()),
            SyntaxKind::VariableDeclaration => ancestor.initializer(),
            SyntaxKind::BinaryExpression => ancestor.right(),
            _ => Node::NIL,
        }
    }

    // Return the type implied by a binding pattern. This is the type implied purely by the binding pattern itself
    // and without regard to its context (i.e. without regard any type annotation or initializer associated with the
    // declaration in which the binding pattern is contained). For example, the implied type of [x, y] is [any, any]
    // and the implied type of { x, y: z = 1 } is { x: any; y: number; }. The type implied by a binding pattern is
    // used as the contextual type of an initializer associated with the binding pattern. Also, for a destructuring
    // parameter with no type annotation or initializer, the type implied by the binding pattern becomes the type of
    // the parameter.
    // Go: checker/checker.go:18240 getTypeFromBindingPattern
    pub fn get_type_from_binding_pattern(
        &mut self,
        pattern: Node,
        include_pattern_in_type: bool,
        report_errors: bool,
    ) -> TypeId {
        if include_pattern_in_type {
            self.contextual_binding_patterns.push(pattern);
        }
        let result = if is_object_binding_pattern(pattern) {
            self.get_type_from_object_binding_pattern(
                pattern,
                include_pattern_in_type,
                report_errors,
            )
        } else {
            self.get_type_from_array_binding_pattern(
                pattern,
                include_pattern_in_type,
                report_errors,
            )
        };
        if include_pattern_in_type {
            self.contextual_binding_patterns.pop();
        }
        result
    }

    // Return the type implied by an object binding pattern
    // Go: checker/checker.go:18257 getTypeFromObjectBindingPattern
    pub fn get_type_from_object_binding_pattern(
        &mut self,
        pattern: Node,
        include_pattern_in_type: bool,
        report_errors: bool,
    ) -> TypeId {
        let members = self.symbols.new_table();
        let mut string_index_info = IndexInfoId::NIL;
        let mut object_flags =
            ObjectFlags::OBJECT_LITERAL | ObjectFlags::CONTAINS_OBJECT_OR_ARRAY_LITERAL;
        for e in pattern.elements() {
            let name = e.property_name_or_name();
            if has_dot_dot_dot_token(e) {
                let (string_type, any_type) = (self.string_type, self.any_type);
                string_index_info = self.new_index_info(
                    string_type,
                    any_type,
                    false, /*isReadonly*/
                    Node::NIL,
                    &[],
                );
                continue;
            }
            let expr_type = self.get_literal_type_from_property_name(name);
            if !self.is_type_usable_as_property_name(expr_type) {
                // do not include computed properties in the implied type
                object_flags |= ObjectFlags::OBJECT_LITERAL_PATTERN_WITH_COMPUTED_PROPERTIES;
                continue;
            }
            let text = self.get_property_name_from_type(expr_type);
            let flags = SymbolFlags::PROPERTY
                | if e.initializer().is_some() {
                    SymbolFlags::OPTIONAL
                } else {
                    SymbolFlags::NONE
                };
            let symbol = self.new_symbol(flags, &text);
            // Go reads the links (and gives the id) before the right side.
            self.value_symbol_links.get_by_id(&self.symbols, symbol);
            let resolved_type =
                self.get_type_from_binding_element(e, include_pattern_in_type, report_errors);
            self.value_symbol_links
                .get_by_id(&self.symbols, symbol)
                .resolved_type = resolved_type;
            let symbol_name = self.sym(symbol).name.clone();
            self.symbols.set(members, symbol_name, symbol);
        }
        let index_infos: Vec<IndexInfoId> = if string_index_info.is_some() {
            vec![string_index_info]
        } else {
            Vec::new()
        };
        let result = self.new_anonymous_type(SymbolId::NIL, members, &[], &[], &index_infos);
        self.ty_mut(result).object_flags |= object_flags;
        if include_pattern_in_type {
            self.pattern_for_type.insert(result, pattern);
            self.ty_mut(result).object_flags |= ObjectFlags::CONTAINS_OBJECT_OR_ARRAY_LITERAL;
        }
        result
    }

    // Return the type implied by an array binding pattern
    // Go: checker/checker.go:18293 getTypeFromArrayBindingPattern
    pub fn get_type_from_array_binding_pattern(
        &mut self,
        pattern: Node,
        include_pattern_in_type: bool,
        report_errors: bool,
    ) -> TypeId {
        let elements = pattern.elements().to_vec();
        let last_element = elements.last().copied().unwrap_or(Node::NIL);
        let mut rest_element = Node::NIL;
        if last_element.is_some()
            && is_binding_element(last_element)
            && has_dot_dot_dot_token(last_element)
        {
            rest_element = last_element;
        }
        if elements.is_empty() || elements.len() == 1 && rest_element.is_some() {
            // TODO: remove ScriptTargetES2015
            if self.language_version >= ScriptTarget::ES2015 {
                let any_type = self.any_type;
                return self.create_iterable_type(any_type);
            }
            return self.any_array_type;
        }
        // Go: core.FindLastIndex(...) + 1
        let mut min_length: usize = 0;
        for i in (0..elements.len()).rev() {
            let e = elements[i];
            if !(e == rest_element || e.name().is_nil() || self.has_default_value(e)) {
                min_length = i + 1;
                break;
            }
        }
        let mut element_types: Vec<TypeId> = vec![TypeId::NIL; elements.len()];
        let mut element_infos: Vec<TupleElementInfo> =
            vec![TupleElementInfo::default(); elements.len()];
        for (i, &e) in elements.iter().enumerate() {
            let t = if e.name().is_nil() {
                self.any_type
            } else {
                self.get_type_from_binding_element(e, include_pattern_in_type, report_errors)
            };
            let flags = if e == rest_element {
                ElementFlags::REST
            } else if i >= min_length {
                ElementFlags::OPTIONAL
            } else {
                ElementFlags::REQUIRED
            };
            element_types[i] = t;
            element_infos[i] = TupleElementInfo {
                flags,
                ..Default::default()
            };
        }
        let mut result = self.create_tuple_type_ex(&element_types, &element_infos, false);
        if include_pattern_in_type {
            result = self.clone_type_reference(result);
            self.pattern_for_type.insert(result, pattern);
            self.ty_mut(result).object_flags |= ObjectFlags::CONTAINS_OBJECT_OR_ARRAY_LITERAL;
        }
        result
    }

    // Return the type implied by a binding pattern element. This is the type of the initializer of the element if
    // one is present. Otherwise, if the element is itself a binding pattern, it is the type implied by the binding
    // pattern. Otherwise, it is the type any.
    // Go: checker/checker.go:18342 getTypeFromBindingElement
    pub fn get_type_from_binding_element(
        &mut self,
        element: Node,
        include_pattern_in_type: bool,
        report_errors: bool,
    ) -> TypeId {
        if element.initializer().is_some() {
            // The type implied by a binding pattern is independent of context, so we check the initializer with no
            // contextual type or, if the element itself is a binding pattern, with the type implied by that binding
            // pattern.
            let mut contextual_type = self.unknown_type;
            if is_binding_pattern(element.name()) {
                contextual_type = self.get_type_from_binding_pattern(
                    element.name(),
                    true,  /*includePatternInType*/
                    false, /*reportErrors*/
                );
            }
            let init_type =
                self.check_declaration_initializer(element, CheckMode::NORMAL, contextual_type);
            let widened = self.get_widened_literal_type_for_initializer(element, init_type);
            return self.add_optionality(widened);
        }
        if is_binding_pattern(element.name()) {
            return self.get_type_from_binding_pattern(
                element.name(),
                include_pattern_in_type,
                report_errors,
            );
        }
        if report_errors && !self.declaration_belongs_to_private_ambient_member(element) {
            let any_type = self.any_type;
            self.report_implicit_any(element, any_type, WideningKind::NORMAL);
        }
        // When we're including the pattern in the type (an indication we're obtaining a contextual type), we
        // use a non-inferrable any type. Inference will never directly infer this type, but it is possible
        // to infer a type that contains it, e.g. for a binding pattern like [foo] or { foo }. In such cases,
        // widening of the binding pattern type substitutes a regular any for the non-inferrable any.
        if include_pattern_in_type {
            return self.non_inferrable_any_type;
        }
        self.any_type
    }

    // Go: checker/checker.go:18369 declarationBelongsToPrivateAmbientMember
    pub fn declaration_belongs_to_private_ambient_member(&self, declaration: Node) -> bool {
        let mut member_declaration = get_root_declaration(declaration);
        if is_parameter_declaration(member_declaration) {
            member_declaration = member_declaration.parent();
        }
        is_private_within_ambient(member_declaration)
    }

    // Go: checker/checker.go:18377 getTypeOfPrototypeProperty
    pub fn get_type_of_prototype_property(&mut self, prototype: SymbolId) -> TypeId {
        // TypeScript 1.0 spec (April 2014): 8.4
        // Every class automatically contains a static property member named 'prototype',
        // the type of which is an instantiation of the class type with type Any supplied as a type argument for each type parameter.
        // It is an error to explicitly declare a static property member with the name 'prototype'.
        let parent = self.get_parent_of_symbol(prototype);
        let class_type = self.get_declared_type_of_symbol(parent);
        let type_parameters = self
            .ty(class_type)
            .as_interface_type()
            .type_parameters()
            .to_vec();
        if !type_parameters.is_empty() {
            let any_type = self.any_type;
            let type_arguments: Vec<TypeId> = type_parameters.iter().map(|_| any_type).collect();
            return self.create_type_reference(class_type, &type_arguments);
        }
        class_type
    }

    // PORT: Go `thisAssignmentDeclarationKind` consts (checker.go:17961) are
    // generated in `crate::flags` as `ThisAssignmentDeclarationKind`.

    // Go: checker/checker.go:18399 getWidenedTypeForAssignmentDeclaration
    pub fn get_widened_type_for_assignment_declaration(&mut self, symbol: SymbolId) -> TypeId {
        let mut t = TypeId::NIL;
        let (kind, location) = self.is_constructor_declared_this_property(symbol);
        match kind {
            ThisAssignmentDeclarationKind::THIS_ASSIGNMENT_DECLARATION_TYPED => {
                if location.is_nil() {
                    panic!("location should not be nil when this assignment has a type.");
                }
                t = self.get_type_from_type_node(location);
            }
            ThisAssignmentDeclarationKind::THIS_ASSIGNMENT_DECLARATION_CONSTRUCTOR => {
                if location.is_nil() {
                    panic!(
                        "constructor should not be nil when this assignment is in a constructor."
                    );
                }
                t = self.get_flow_type_in_constructor(symbol, location);
            }
            ThisAssignmentDeclarationKind::THIS_ASSIGNMENT_DECLARATION_METHOD => {
                t = self.get_type_of_property_in_base_class(symbol);
            }
            _ => {}
        }
        if t.is_nil() {
            let mut types: Vec<TypeId> = Vec::new();
            let declarations = self.sym(symbol).declarations.clone();
            for (i, &declaration) in declarations.iter().enumerate() {
                if is_binary_expression(declaration) && declaration.type_().is_some() {
                    t = self.get_type_from_type_node(declaration.type_());
                    break;
                }
                let assigned_type = self.get_assignment_declaration_initializer_type(declaration);
                if assigned_type.is_some() {
                    // We ignore initial assignments of undefined to CommonJS exports when there are multiple assignment declarations
                    if get_assignment_declaration_kind(declaration)
                        != JSDeclarationKind::EXPORTS_PROPERTY
                        || i != 0
                        || declarations.len() == 1
                        || !self
                            .ty(assigned_type)
                            .flags
                            .intersects(TypeFlags::UNDEFINED)
                    {
                        if !types.contains(&assigned_type) {
                            types.push(assigned_type);
                        }
                    }
                }
            }
            if kind == ThisAssignmentDeclarationKind::THIS_ASSIGNMENT_DECLARATION_METHOD
                && !types.is_empty()
            {
                if self.strict_null_checks {
                    let undefined_or_missing_type = self.undefined_or_missing_type;
                    if !types.contains(&undefined_or_missing_type) {
                        types.push(undefined_or_missing_type);
                    }
                }
            }
            if t.is_nil() {
                t = self.any_type;
                if !types.is_empty() {
                    t = self.get_union_type(&types);
                }
            }
        }
        t = self.get_widened_type(t);
        // report an all-nullable or empty union as an implicit any in JS files
        let value_declaration = self.sym(symbol).value_declaration;
        if value_declaration.is_some() && is_in_js_file(value_declaration) && {
            let filtered = self.filter_type(t, &mut |c: &mut Checker, t: TypeId| {
                !c.ty(t).flags.without(TypeFlags::NULLABLE).is_empty()
            });
            filtered == self.never_type
        } {
            let any_type = self.any_type;
            self.report_implicit_any(value_declaration, any_type, WideningKind::NORMAL);
            return self.any_type;
        }
        t
    }

    // Go: checker/checker.go:18452 getAssignmentDeclarationInitializerType
    pub fn get_assignment_declaration_initializer_type(&mut self, node: Node) -> TypeId {
        if is_binary_expression(node) {
            let t: TypeId;
            match get_assignment_declaration_kind(node) {
                JSDeclarationKind::MODULE_EXPORTS | JSDeclarationKind::EXPORTS_PROPERTY => {
                    let expr_type =
                        self.check_expression_cached(get_right_most_assigned_expression(node));
                    t = self.get_regular_type_of_literal_type(expr_type);
                }
                kind => {
                    if kind == JSDeclarationKind::THIS_PROPERTY
                        && self.contains_same_named_this_property(node.left(), node.right())
                    {
                        return TypeId::NIL;
                    }
                    // fallthrough
                    t = self.check_expression_for_mutable_location(node.right(), CheckMode::NORMAL);
                }
            }
            if self.is_empty_array_literal_type(t)
                && !self.has_parent_with_type_annotation(node.symbol())
            {
                let any_array_type = self.any_array_type;
                self.report_implicit_any(node, any_array_type, WideningKind::NORMAL);
                return self.any_array_type;
            }
            return t;
        }
        if is_call_expression(node) {
            return self.get_type_from_property_descriptor(node.arguments().get(2));
        }
        TypeId::NIL
    }

    // Return true if the parent symbol of the given assignment declaration symbol has declaration with a type
    // annotation. For example, returns true for the symbol associated with `f.a` below:
    //
    //	const f: { (): void, a: string[] } = () => {};
    //	f.a = [];
    // Go: checker/checker.go:18483 hasParentWithTypeAnnotation
    pub fn has_parent_with_type_annotation(&mut self, symbol: SymbolId) -> bool {
        let parent = self.sym(symbol).parent;
        if parent.is_some() {
            let parent_value_declaration = self.sym(parent).value_declaration;
            if parent_value_declaration.is_some()
                && is_function_expression_or_arrow_function(parent_value_declaration)
            {
                let possibly_annotated_symbol =
                    self.get_symbol_of_node(parent_value_declaration.parent());
                if possibly_annotated_symbol.is_some() {
                    let value_declaration = self.sym(possibly_annotated_symbol).value_declaration;
                    if value_declaration.is_some() {
                        return value_declaration.type_().is_some();
                    }
                }
            }
        }
        false
    }

    // Go: checker/checker.go:18492 containsSameNamedThisProperty
    pub fn contains_same_named_this_property(
        &mut self,
        this_property: Node,
        expression: Node,
    ) -> bool {
        fn visit(c: &mut Checker, this_property: Node, node: Node) -> bool {
            if c.is_matching_reference(this_property, node) {
                return true;
            }
            if is_function_like(node) {
                return false;
            }
            node.for_each_child(&mut |child: Node| visit(c, this_property, child))
        }
        visit(self, this_property, expression)
    }

    // Go: checker/checker.go:18506 getTypeFromPropertyDescriptor
    pub fn get_type_from_property_descriptor(&mut self, node: Node) -> TypeId {
        let object_literal_type = self.check_expression_cached(node);
        let value_type = self.get_type_of_property_of_type(object_literal_type, "value");
        if value_type.is_some() {
            return value_type;
        }
        let get_func = self.get_type_of_property_of_type(object_literal_type, "get");
        if get_func.is_some() {
            let get_sig = self.get_single_call_signature(get_func);
            if get_sig.is_some() {
                return self.get_return_type_of_signature(get_sig);
            }
        }
        let set_func = self.get_type_of_property_of_type(object_literal_type, "set");
        if set_func.is_some() {
            let set_sig = self.get_single_call_signature(set_func);
            if set_sig.is_some() {
                return self.get_type_of_first_parameter_of_signature(set_sig);
            }
        }
        self.any_type
    }

    // A property is considered a constructor declared property when all declaration sites are this.xxx assignments,
    // when no declaration sites have JSDoc type annotations, and when at least one declaration site is in the body of
    // a class constructor.
    // Go: checker/checker.go:18527 isConstructorDeclaredThisProperty
    pub fn is_constructor_declared_this_property(
        &mut self,
        symbol: SymbolId,
    ) -> (ThisAssignmentDeclarationKind, Node) {
        let value_declaration = self.sym(symbol).value_declaration;
        if value_declaration.is_nil() || !is_binary_expression(value_declaration) {
            return (
                ThisAssignmentDeclarationKind::THIS_ASSIGNMENT_DECLARATION_NONE,
                Node::NIL,
            );
        }
        if let Some(&kind) = self.this_expando_kinds.get(&symbol) {
            let Some(&location) = self.this_expando_locations.get(&symbol) else {
                panic!("location should be cached whenever this expando symbol is cached");
            };
            return (kind, location);
        }
        let mut all_this = true;
        let mut type_annotation = Node::NIL;
        let declarations = self.sym(symbol).declarations.clone();
        for &declaration in declarations.iter() {
            if !is_binary_expression(declaration) {
                all_this = false;
                break;
            }
            let left = declaration.left();
            if get_assignment_declaration_kind(declaration) == JSDeclarationKind::THIS_PROPERTY
                && (left.kind() != SyntaxKind::ElementAccessExpression
                    || is_string_or_numeric_literal_like(left.argument_expression()))
            {
                if declaration.type_().is_some() {
                    type_annotation = declaration.type_();
                }
            } else {
                all_this = false;
                break;
            }
        }
        let mut location = Node::NIL;
        let mut kind = ThisAssignmentDeclarationKind::THIS_ASSIGNMENT_DECLARATION_NONE;
        if all_this {
            if type_annotation.is_some() {
                location = type_annotation;
                kind = ThisAssignmentDeclarationKind::THIS_ASSIGNMENT_DECLARATION_TYPED;
            } else {
                location = self.get_declaring_constructor(symbol);
                kind = if location.is_nil() {
                    ThisAssignmentDeclarationKind::THIS_ASSIGNMENT_DECLARATION_METHOD
                } else {
                    ThisAssignmentDeclarationKind::THIS_ASSIGNMENT_DECLARATION_CONSTRUCTOR
                };
            }
        }
        self.this_expando_kinds.insert(symbol, kind);
        self.this_expando_locations.insert(symbol, location);
        (kind, location)
    }

    // Go: checker/checker.go:18572 isGlobalSymbolConstructor
    pub fn is_global_symbol_constructor(&mut self, node: Node) -> bool {
        let symbol = self.get_symbol_of_node(node);
        let global_symbol = (self
            .get_global_es_symbol_constructor_type_symbol_or_nil
            .clone())(self);
        global_symbol.is_some() && symbol == global_symbol
    }

    // Go: checker/checker.go:18578 widenTypeForVariableLikeDeclaration
    pub fn widen_type_for_variable_like_declaration(
        &mut self,
        t: TypeId,
        declaration: Node,
        report_errors: bool,
    ) -> TypeId {
        let mut t = t;
        if t.is_some() {
            // This special case is required for backwards compatibility with libraries that merge a `symbol` property into `SymbolConstructor`.
            // See https://github.com/microsoft/typescript-go/issues/1212
            if self.ty(t).flags.intersects(TypeFlags::ES_SYMBOL)
                && self.is_global_symbol_constructor(declaration.parent())
            {
                t = self.get_es_symbol_like_type_for_node(declaration);
            }

            if report_errors {
                self.report_errors_from_widening(declaration, t, WideningKind::NORMAL);
            }

            // always widen a 'unique symbol' type if the type was created for a different declaration.
            if self.ty(t).flags.intersects(TypeFlags::UNIQUE_ES_SYMBOL)
                && (is_binding_element(declaration) || declaration.type_().is_nil())
                && {
                    let declaration_symbol = self.get_symbol_of_declaration(declaration);
                    self.ty(t).symbol != declaration_symbol
                }
            {
                t = self.es_symbol_type;
            }
            return self.get_widened_type(t);
        }
        // Rest parameters default to type any[], other parameters default to type any
        if is_parameter_declaration(declaration) && declaration.dot_dot_dot_token().is_some() {
            t = self.any_array_type;
        } else {
            t = self.any_type;
        }
        // Report implicit any errors unless this is a private property within an ambient declaration
        if report_errors {
            if !declaration_belongs_to_private_ambient_member(declaration) {
                self.report_implicit_any(declaration, t, WideningKind::NORMAL);
            }
        }
        t
    }

    // Go: checker/checker.go:18611 reportImplicitAny
    pub fn report_implicit_any(
        &mut self,
        declaration: Node,
        t: TypeId,
        widening_kind: WideningKind,
    ) {
        if is_in_js_file(declaration)
            && !is_check_js_enabled_for_file(
                get_source_file_of_node(declaration),
                self.compiler_options,
            )
        {
            // Only report implicit any errors/suggestions in TS and ts-check JS files
            return;
        }
        let widened = self.get_widened_type(t);
        let type_as_string = self.type_to_string_exported(widened);
        let diagnostic: &'static crate::diagnostics::Message;
        match declaration.kind() {
            SyntaxKind::BinaryExpression
            | SyntaxKind::PropertyDeclaration
            | SyntaxKind::PropertySignature => {
                diagnostic = if self.no_implicit_any {
                    diag::Member_0_implicitly_has_an_1_type
                } else {
                    diag::Member_0_implicitly_has_an_1_type_but_a_better_type_may_be_inferred_from_usage
                };
            }
            SyntaxKind::Parameter => {
                let param = declaration;
                if is_identifier(param.name()) {
                    let name = param.name();
                    let original_keyword_kind = identifier_to_keyword_kind(name);
                    let parent = declaration.parent();
                    if (is_call_signature_declaration(parent)
                        || is_method_signature_declaration(parent)
                        || is_function_type_node(parent))
                        && parent.parameters().to_vec().contains(&declaration)
                        && (is_type_node_kind(original_keyword_kind)
                            || self
                                .resolve_name(
                                    declaration,
                                    name.text(),
                                    SymbolFlags::TYPE,
                                    None,  /*nameNotFoundMessage*/
                                    true,  /*isUse*/
                                    false, /*excludeGlobals*/
                                )
                                .is_some())
                    {
                        let index = parent
                            .parameters()
                            .to_vec()
                            .iter()
                            .position(|&p| p == declaration)
                            .map_or(-1, |i| i as i32);
                        let new_name = format!("arg{index}");
                        let type_name = declaration_name_to_string(param.name())
                            + if param.dot_dot_dot_token().is_some() {
                                "[]"
                            } else {
                                ""
                            };
                        let no_implicit_any = self.no_implicit_any;
                        self.error_or_suggestion(
                            no_implicit_any,
                            declaration,
                            diag::Parameter_has_a_name_but_no_type_Did_you_mean_0_Colon_1,
                            args![new_name, type_name],
                        );
                        return;
                    }
                }
                if param.dot_dot_dot_token().is_some() {
                    if self.no_implicit_any {
                        diagnostic = diag::Rest_parameter_0_implicitly_has_an_any_type;
                    } else {
                        diagnostic =
                            diag::Rest_parameter_0_implicitly_has_an_any_type_but_a_better_type_may_be_inferred_from_usage;
                    }
                } else if self.no_implicit_any {
                    diagnostic = diag::Parameter_0_implicitly_has_an_1_type;
                } else {
                    diagnostic = diag::Parameter_0_implicitly_has_an_1_type_but_a_better_type_may_be_inferred_from_usage;
                }
            }
            SyntaxKind::BindingElement => {
                diagnostic = diag::Binding_element_0_implicitly_has_an_1_type;
                if !self.no_implicit_any {
                    // Don't issue a suggestion for binding elements since the codefix doesn't yet support them.
                    return;
                }
            }
            SyntaxKind::FunctionDeclaration
            | SyntaxKind::MethodDeclaration
            | SyntaxKind::MethodSignature
            | SyntaxKind::GetAccessor
            | SyntaxKind::SetAccessor
            | SyntaxKind::FunctionExpression
            | SyntaxKind::ArrowFunction => {
                if self.no_implicit_any && declaration.name().is_nil() {
                    if widening_kind == WideningKind::GENERATOR_YIELD {
                        self.error(
                            declaration,
                            diag::Generator_implicitly_has_yield_type_0_Consider_supplying_a_return_type_annotation,
                            args![type_as_string],
                        );
                    } else {
                        self.error(
                            declaration,
                            diag::Function_expression_which_lacks_return_type_annotation_implicitly_has_an_0_return_type,
                            args![type_as_string],
                        );
                    }
                    return;
                }
                if !self.no_implicit_any {
                    diagnostic = diag::X_0_implicitly_has_an_1_return_type_but_a_better_type_may_be_inferred_from_usage;
                } else if declaration.flags().intersects(NodeFlags::REPARSED) {
                    let name = declaration_name_to_string(get_name_of_declaration(declaration));
                    if !name.is_empty() {
                        self.error(
                            declaration,
                            diag::X_0_which_lacks_return_type_annotation_implicitly_has_an_1_return_type,
                            args![name, type_as_string],
                        );
                    } else {
                        self.error(
                            declaration,
                            diag::This_overload_implicitly_returns_the_type_0_because_it_lacks_a_return_type_annotation,
                            args![type_as_string],
                        );
                    }
                    return;
                } else if widening_kind == WideningKind::GENERATOR_YIELD {
                    diagnostic =
                        diag::X_0_which_lacks_return_type_annotation_implicitly_has_an_1_yield_type;
                } else {
                    diagnostic = diag::X_0_which_lacks_return_type_annotation_implicitly_has_an_1_return_type;
                }
            }
            SyntaxKind::MappedType => {
                if self.no_implicit_any {
                    self.error(
                        declaration,
                        diag::Mapped_object_type_implicitly_has_an_any_template_type,
                        args![],
                    );
                }
                return;
            }
            _ => {
                if self.no_implicit_any {
                    diagnostic = diag::Variable_0_implicitly_has_an_1_type;
                } else {
                    diagnostic = diag::Variable_0_implicitly_has_an_1_type_but_a_better_type_may_be_inferred_from_usage;
                }
            }
        }
        let no_implicit_any = self.no_implicit_any;
        let name_string = declaration_name_to_string(get_name_of_declaration(declaration));
        self.error_or_suggestion(
            no_implicit_any,
            declaration,
            diagnostic,
            args![name_string, type_as_string],
        );
    }

    // Go: checker/checker.go:18695 getWidenedType
    pub fn get_widened_type(&mut self, t: TypeId) -> TypeId {
        self.get_widened_type_with_context(t, None /*context*/)
    }

    // Go: checker/checker.go:18699 getWidenedTypeWithContext
    // PORT: Go `*WideningContext` is `Option<Rc<RefCell<WideningContext>>>`
    // (nil is `None`).
    pub fn get_widened_type_with_context(
        &mut self,
        t: TypeId,
        context: Option<Rc<RefCell<WideningContext>>>,
    ) -> TypeId {
        if self
            .ty(t)
            .object_flags
            .intersects(ObjectFlags::REQUIRES_WIDENING)
        {
            let key = CachedTypeKey {
                kind: CachedTypeKind::WIDENED,
                type_id: t,
            };
            if context.is_none() {
                if let Some(&cached) = self.cached_types.get(&key) {
                    if cached.is_some() {
                        return cached;
                    }
                }
            }
            let mut result = TypeId::NIL;
            let flags = self.ty(t).flags;
            if flags.intersects(TypeFlags::ANY | TypeFlags::NULLABLE) {
                result = self.any_type;
            } else if self.is_object_literal_type(t) {
                result = self.get_widened_type_of_object_literal(t, context.clone());
            } else if flags.intersects(TypeFlags::UNION) {
                let types = self.ty(t).types_list();
                let union_context = match &context {
                    Some(ctx) => ctx.clone(),
                    None => Rc::new(RefCell::new(WideningContext {
                        siblings: types.to_vec(),
                        ..Default::default()
                    })),
                };
                let mut widened_types: Vec<TypeId> = Vec::with_capacity(types.len());
                for &member in &types {
                    if self.ty(member).flags.intersects(TypeFlags::NULLABLE) {
                        widened_types.push(member);
                    } else {
                        widened_types.push(
                            self.get_widened_type_with_context(member, Some(union_context.clone())),
                        );
                    }
                }
                // Widening an empty object literal transitions from a highly restrictive type to
                // a highly inclusive one. For that reason we perform subtype reduction here if the
                // union includes empty object types (e.g. reducing {} | string to just {}).
                let mut some_empty = false;
                for &w in &widened_types {
                    if self.is_empty_object_type(w) {
                        some_empty = true;
                        break;
                    }
                }
                let reduction = if some_empty {
                    UnionReduction::SUBTYPE
                } else {
                    UnionReduction::LITERAL
                };
                result = self.get_union_type_ex(&widened_types, reduction, None, TypeId::NIL);
            } else if flags.intersects(TypeFlags::INTERSECTION) {
                let types = self.ty(t).types_list();
                let mut widened_types: Vec<TypeId> = Vec::with_capacity(types.len());
                for member in types {
                    widened_types.push(self.get_widened_type(member));
                }
                result = self.get_intersection_type(&widened_types);
            } else if self.is_array_or_tuple_type(t) {
                let target = self.ty(t).target();
                let type_arguments = self.get_type_arguments(t);
                let mut widened_arguments: Vec<TypeId> = Vec::with_capacity(type_arguments.len());
                for arg in type_arguments {
                    widened_arguments.push(self.get_widened_type(arg));
                }
                result = self.create_type_reference(target, &widened_arguments);
            }
            if result.is_some() && context.is_none() {
                self.cached_types.insert(key, result);
            }
            return if result.is_some() { result } else { t };
        }
        t
    }

    // Go: checker/checker.go:18740 getWidenedTypeOfObjectLiteral
    pub fn get_widened_type_of_object_literal(
        &mut self,
        t: TypeId,
        context: Option<Rc<RefCell<WideningContext>>>,
    ) -> TypeId {
        if let Some(ctx) = &context {
            if let Some(&cached) = ctx.borrow().widened_types.get(&t) {
                if cached.is_some() {
                    return cached;
                }
            }
        }
        let members = self.symbols.new_table();
        for prop in self.get_properties_of_object_type(t) {
            let widened = self.get_widened_property(prop, context.clone());
            let name = self.sym(prop).name.clone();
            self.symbols.set(members, name, widened);
        }
        if let Some(ctx) = &context {
            for prop in self.get_properties_of_context(ctx) {
                let name = self.sym(prop).name.clone();
                if self.symbols.get_name(members, &name).is_nil() {
                    let undefined_property = self.get_undefined_property(prop);
                    self.symbols.set(members, name, undefined_property);
                }
            }
        }
        let symbol = self.ty(t).symbol;
        let infos = self.get_index_infos_of_type(t);
        let mut widened_infos: Vec<IndexInfoId> = Vec::with_capacity(infos.len());
        for info in infos {
            let (key_type, value_type, is_readonly, declaration, components) = {
                let i = self.index_info(info);
                (
                    i.key_type,
                    i.value_type,
                    i.is_readonly,
                    i.declaration,
                    i.components.clone(),
                )
            };
            let widened_value_type = self.get_widened_type(value_type);
            widened_infos.push(self.new_index_info(
                key_type,
                widened_value_type,
                is_readonly,
                declaration,
                &components,
            ));
        }
        let result = self.new_anonymous_type(symbol, members, &[], &[], &widened_infos);
        // Retain js literal flag through widening
        let retained =
            self.ty(t).object_flags & (ObjectFlags::JS_LITERAL | ObjectFlags::NON_INFERRABLE_TYPE);
        self.ty_mut(result).object_flags |= retained;
        // Only cache in child contexts since the root context never widens a particular object literal type more than once
        if let Some(ctx) = &context {
            if ctx.borrow().parent.is_some() {
                ctx.borrow_mut().widened_types.insert(t, result);
            }
        }
        result
    }

    // Go: checker/checker.go:18772 getWidenedProperty
    pub fn get_widened_property(
        &mut self,
        prop: SymbolId,
        context: Option<Rc<RefCell<WideningContext>>>,
    ) -> SymbolId {
        if !self.sym(prop).flags.intersects(SymbolFlags::PROPERTY) {
            // Since get accessors already widen their return value there is no need to
            // widen accessor based properties here.
            return prop;
        }
        let original = self.get_type_of_symbol(prop);
        let mut prop_context: Option<Rc<RefCell<WideningContext>>> = None;
        if let Some(ctx) = &context {
            let name = self.sym(prop).name.clone();
            prop_context = Some(WideningContext::get_child_context(ctx, &name));
        }
        let widened = self.get_widened_type_with_context(original, prop_context);
        if widened == original {
            return prop;
        }
        self.create_symbol_with_type(prop, widened)
    }
}

impl WideningContext {
    // Go: checker/checker.go:18790 WideningContext.getChildContext
    // PORT: the child keeps a `Weak` to its parent, so this takes the
    // parent's `Rc` instead of `&self`. Call as
    // `WideningContext::get_child_context(&ctx, name)`.
    pub fn get_child_context(
        this: &Rc<RefCell<WideningContext>>,
        property_name: &str,
    ) -> Rc<RefCell<WideningContext>> {
        if let Some(cached) = this.borrow().child_contexts.get(property_name) {
            return cached.clone();
        }
        let result = Rc::new(RefCell::new(WideningContext {
            parent: Some(Rc::downgrade(this)),
            property_name: property_name.to_string(),
            ..Default::default()
        }));
        this.borrow_mut()
            .child_contexts
            .insert(property_name.to_string(), result.clone());
        result
    }
}

impl Checker {
    // Go: checker/checker.go:18802 getPropertiesOfContext
    // PORT: Go tests `resolvedProperties == nil`. The Rust field is a `Vec`,
    // so an empty result is computed again on the next call. The inputs are
    // cached, so the result is the same.
    pub fn get_properties_of_context(
        &mut self,
        context: &Rc<RefCell<WideningContext>>,
    ) -> Vec<SymbolId> {
        if context.borrow().resolved_properties.is_empty() {
            let mut names: IndexMap<String, SymbolId> = IndexMap::new();
            for t in self.get_siblings_of_context(context) {
                if self.is_object_literal_type(t)
                    && !self
                        .ty(t)
                        .object_flags
                        .intersects(ObjectFlags::CONTAINS_SPREAD)
                {
                    for prop in self.get_properties_of_type(t) {
                        let name = self.sym(prop).name.to_string();
                        names.insert(name, prop);
                    }
                }
            }
            context.borrow_mut().resolved_properties = names.values().copied().collect();
        }
        context.borrow().resolved_properties.clone()
    }

    // Go: checker/checker.go:18817 getSiblingsOfContext
    // PORT: Go tests `siblings == nil`. The Rust field is a `Vec`, so an
    // empty result is computed again on the next call. The inputs are
    // cached, so the result is the same.
    pub fn get_siblings_of_context(
        &mut self,
        context: &Rc<RefCell<WideningContext>>,
    ) -> Vec<TypeId> {
        if context.borrow().siblings.is_empty() {
            let mut siblings: Vec<TypeId> = Vec::new();
            let (parent, property_name) = {
                let ctx = context.borrow();
                (
                    ctx.parent
                        .as_ref()
                        .and_then(std::rc::Weak::upgrade)
                        .expect("child widening context has a live parent"),
                    ctx.property_name.clone(),
                )
            };
            for t in self.get_siblings_of_context(&parent) {
                if self.is_object_literal_type(t) {
                    let prop = self.get_property_of_object_type(t, &property_name);
                    if prop.is_some() {
                        let prop_type = self.get_type_of_symbol(prop);
                        siblings.extend(self.ty(prop_type).distributed());
                    }
                }
            }
            context.borrow_mut().siblings = siblings;
        }
        context.borrow().siblings.clone()
    }

    // Go: checker/checker.go:18833 getUndefinedProperty
    pub fn get_undefined_property(&mut self, prop: SymbolId) -> SymbolId {
        let name = self.sym(prop).name.to_string();
        if let Some(&cached) = self.undefined_properties.get(&name) {
            if cached.is_some() {
                return cached;
            }
        }
        let undefined_or_missing_type = self.undefined_or_missing_type;
        let result = self.create_symbol_with_type(prop, undefined_or_missing_type);
        self.sym_mut(result).flags |= SymbolFlags::OPTIONAL;
        self.undefined_properties.insert(name, result);
        result
    }

    // Go: checker/checker.go:18843 getTypeOfEnumMember
    pub fn get_type_of_enum_member(&mut self, symbol: SymbolId) -> TypeId {
        if self
            .value_symbol_links
            .get_by_id(&self.symbols, symbol)
            .resolved_type
            .is_nil()
        {
            let t = self.get_declared_type_of_enum_member(symbol);
            self.value_symbol_links
                .get_by_id(&self.symbols, symbol)
                .resolved_type = t;
        }
        self.value_symbol_links
            .get_by_id(&self.symbols, symbol)
            .resolved_type
    }

    // Go: checker/checker.go:18851 getTypeOfAccessors
    pub fn get_type_of_accessors(&mut self, symbol: SymbolId) -> TypeId {
        if self
            .value_symbol_links
            .get_by_id(&self.symbols, symbol)
            .resolved_type
            .is_nil()
        {
            if !self.push_type_resolution(
                TypeSystemEntity::Symbol(symbol),
                TypeSystemPropertyName::TYPE,
            ) {
                return self.error_type;
            }
            let getter = get_declaration_of_kind(&self.symbols, symbol, SyntaxKind::GetAccessor);
            let setter = get_declaration_of_kind(&self.symbols, symbol, SyntaxKind::SetAccessor);
            let accessor = self
                .sym(symbol)
                .declarations
                .iter()
                .copied()
                .find(|&d| is_auto_accessor_property_declaration(d))
                .unwrap_or(Node::NIL);
            // We try to resolve a getter type annotation, a setter type annotation, or a getter function
            // body return type inference, in that order.
            let mut t = self.get_annotated_accessor_type(getter);
            if t.is_nil() {
                t = self.get_annotated_accessor_type(setter);
            }
            if t.is_nil() {
                t = self.get_annotated_accessor_type(accessor);
            }
            if t.is_nil() && getter.is_some() {
                let body = getter.body();
                if body.is_some() {
                    t = self.get_return_type_from_body(getter, CheckMode::NORMAL);
                }
            }
            if t.is_nil() && accessor.is_some() {
                t = self.get_widened_type_for_variable_like_declaration(
                    accessor, true, /*reportErrors*/
                );
            }
            if t.is_nil() {
                let no_implicit_any = self.no_implicit_any;
                if setter.is_some() && !is_private_within_ambient(setter) {
                    let symbol_string = self.symbol_to_string(symbol);
                    self.error_or_suggestion(
                        no_implicit_any,
                        setter,
                        diag::Property_0_implicitly_has_type_any_because_its_set_accessor_lacks_a_parameter_type_annotation,
                        args![symbol_string],
                    );
                } else if getter.is_some() && !is_private_within_ambient(getter) {
                    let symbol_string = self.symbol_to_string(symbol);
                    self.error_or_suggestion(
                        no_implicit_any,
                        getter,
                        diag::Property_0_implicitly_has_type_any_because_its_get_accessor_lacks_a_return_type_annotation,
                        args![symbol_string],
                    );
                } else if accessor.is_some() && !is_private_within_ambient(accessor) {
                    let symbol_string = self.symbol_to_string(symbol);
                    self.error_or_suggestion(
                        no_implicit_any,
                        accessor,
                        diag::Member_0_implicitly_has_an_1_type,
                        args![symbol_string, "any"],
                    );
                }
                t = self.any_type;
            }
            if !self.pop_type_resolution() {
                if self.get_annotated_accessor_type_node(getter).is_some() {
                    let symbol_string = self.symbol_to_string(symbol);
                    self.error(
                        getter,
                        diag::X_0_is_referenced_directly_or_indirectly_in_its_own_type_annotation,
                        args![symbol_string],
                    );
                } else if self.get_annotated_accessor_type_node(setter).is_some() {
                    let symbol_string = self.symbol_to_string(symbol);
                    self.error(
                        setter,
                        diag::X_0_is_referenced_directly_or_indirectly_in_its_own_type_annotation,
                        args![symbol_string],
                    );
                } else if self.get_annotated_accessor_type_node(accessor).is_some() {
                    // PORT: Go reports on `setter` here, not `accessor`. Kept as in Go.
                    let symbol_string = self.symbol_to_string(symbol);
                    self.error(
                        setter,
                        diag::X_0_is_referenced_directly_or_indirectly_in_its_own_type_annotation,
                        args![symbol_string],
                    );
                } else if getter.is_some() && self.no_implicit_any {
                    let symbol_string = self.symbol_to_string(symbol);
                    self.error(
                        getter,
                        diag::X_0_implicitly_has_return_type_any_because_it_does_not_have_a_return_type_annotation_and_is_referenced_directly_or_indirectly_in_one_of_its_return_expressions,
                        args![symbol_string],
                    );
                }
                t = self.any_type;
            }
            let links = self.value_symbol_links.get_by_id(&self.symbols, symbol);
            if links.resolved_type.is_nil() {
                links.resolved_type = t;
            }
        }
        self.value_symbol_links
            .get_by_id(&self.symbols, symbol)
            .resolved_type
    }

    // Go: checker/checker.go:18906 getWriteTypeOfAccessors
    pub fn get_write_type_of_accessors(&mut self, symbol: SymbolId) -> TypeId {
        if self
            .value_symbol_links
            .get_by_id(&self.symbols, symbol)
            .write_type
            .is_nil()
        {
            if !self.push_type_resolution(
                TypeSystemEntity::Symbol(symbol),
                TypeSystemPropertyName::WRITE_TYPE,
            ) {
                return self.error_type;
            }
            let mut setter =
                get_declaration_of_kind(&self.symbols, symbol, SyntaxKind::SetAccessor);
            if setter.is_nil() {
                let prop_declaration =
                    get_declaration_of_kind(&self.symbols, symbol, SyntaxKind::PropertyDeclaration);
                if prop_declaration.is_some()
                    && is_auto_accessor_property_declaration(prop_declaration)
                {
                    setter = prop_declaration;
                }
            }
            let mut write_type = self.get_annotated_accessor_type(setter);
            if !self.pop_type_resolution() {
                if self.get_annotated_accessor_type_node(setter).is_some() {
                    let symbol_string = self.symbol_to_string(symbol);
                    self.error(
                        setter,
                        diag::X_0_is_referenced_directly_or_indirectly_in_its_own_type_annotation,
                        args![symbol_string],
                    );
                }
                write_type = self.any_type;
            }
            // Absent an explicit setter type annotation we use the read type of the accessor.
            if self
                .value_symbol_links
                .get_by_id(&self.symbols, symbol)
                .write_type
                .is_nil()
            {
                if write_type.is_some() {
                    self.value_symbol_links
                        .get_by_id(&self.symbols, symbol)
                        .write_type = write_type;
                } else {
                    let read_type = self.get_type_of_accessors(symbol);
                    self.value_symbol_links
                        .get_by_id(&self.symbols, symbol)
                        .write_type = read_type;
                }
            }
        }
        self.value_symbol_links
            .get_by_id(&self.symbols, symbol)
            .write_type
    }

    // Go: checker/checker.go:18938 getTypeOfAlias
    pub fn get_type_of_alias(&mut self, symbol: SymbolId) -> TypeId {
        if self
            .value_symbol_links
            .get_by_id(&self.symbols, symbol)
            .resolved_type
            .is_nil()
        {
            if !self.push_type_resolution(
                TypeSystemEntity::Symbol(symbol),
                TypeSystemPropertyName::TYPE,
            ) {
                return self.error_type;
            }
            let target_symbol = self.resolve_alias(symbol);
            let alias_declaration = self.get_declaration_of_alias_symbol(symbol);
            let export_symbol = self.get_target_of_alias_declaration(alias_declaration);
            // It only makes sense to get the type of a value symbol. If the result of resolving
            // the alias is not a value, then it has no type. To get the type associated with a
            // type symbol, call getDeclaredTypeOfSymbol.
            // This check is important because without it, a call to getTypeOfSymbol could end
            // up recursively calling getTypeOfAlias, causing a stack overflow.
            if self
                .value_symbol_links
                .get_by_id(&self.symbols, symbol)
                .resolved_type
                .is_nil()
            {
                if self
                    .get_symbol_flags(target_symbol)
                    .intersects(SymbolFlags::VALUE)
                {
                    let t = self.get_type_of_symbol(target_symbol);
                    self.value_symbol_links
                        .get_by_id(&self.symbols, symbol)
                        .resolved_type = t;
                } else {
                    let error_type = self.error_type;
                    self.value_symbol_links
                        .get_by_id(&self.symbols, symbol)
                        .resolved_type = error_type;
                }
            }
            if !self.pop_type_resolution() {
                self.report_circularity_error(if export_symbol.is_some() {
                    export_symbol
                } else {
                    symbol
                });
                if self
                    .value_symbol_links
                    .get_by_id(&self.symbols, symbol)
                    .resolved_type
                    .is_nil()
                {
                    let error_type = self.error_type;
                    self.value_symbol_links
                        .get_by_id(&self.symbols, symbol)
                        .resolved_type = error_type;
                }
                return self
                    .value_symbol_links
                    .get_by_id(&self.symbols, symbol)
                    .resolved_type;
            }
        }
        self.value_symbol_links
            .get_by_id(&self.symbols, symbol)
            .resolved_type
    }
}
