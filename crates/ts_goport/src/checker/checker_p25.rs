use crate::prelude::*;
use smallvec::SmallVec;

// PORT: cross-file decisions for this range.
// - Go `*TypeAlias` parameters and returns that store or create an alias are
//   `Option<Rc<TypeAlias>>` (the `Type::alias` field type).
// - The cache key functions `getTypeInstantiationKey` and
//   `getConditionalTypeKey` only read the alias, so they take
//   `Option<&TypeAlias>` (pass `alias.as_deref()`). They are free functions
//   because they only hash ids.
// - Go `*ConditionalRoot` is `Rc<RefCell<ConditionalRoot>>` (see types.rs), so
//   `get_conditional_type` takes it by value (a cloned `Rc`).

impl Checker {
    /// Go `t.AsObjectType().instantiations` (nil is `None`). An interface or
    /// tuple keeps the map in `InterfaceType`, any other object type in
    /// `object_type_instantiations` (see `ObjectType`).
    pub fn object_instantiations(&self, t: TypeId) -> Option<&InstantiationMap> {
        let data = &self.ty(t).data;
        if let Some(d) = data.as_interface_type() {
            return d.instantiations.as_ref();
        }
        match data.as_object_type()?.instantiations {
            InstantiationMapId::NIL => None,
            id => Some(&self.object_type_instantiations[id.0 as usize]),
        }
    }

    /// Go `t.AsObjectType().instantiations`, made first when it is nil.
    pub fn object_instantiations_mut(&mut self, t: TypeId) -> &mut InstantiationMap {
        if self.ty(t).data.as_interface_type().is_some() {
            return self
                .ty_mut(t)
                .as_interface_type_mut()
                .instantiations
                .get_or_insert_with(InstantiationMap::default);
        }
        let mut id = self.ty(t).as_object_type().instantiations;
        if id == InstantiationMapId::NIL {
            id = InstantiationMapId(
                u32::try_from(self.object_type_instantiations.len())
                    .expect("instantiation map overflow"),
            );
            self.object_type_instantiations
                .push(InstantiationMap::default());
            self.ty_mut(t).as_object_type_mut().instantiations = id;
        }
        &mut self.object_type_instantiations[id.0 as usize]
    }

    // Go: checker/checker.go:22724 getObjectTypeInstantiation
    pub fn get_object_type_instantiation(
        &mut self,
        t: TypeId,
        m: MapperId,
        alias: Option<Rc<TypeAlias>>,
    ) -> TypeId {
        let declaration: Node;
        let target: TypeId;
        let t_object_flags = self.ty(t).object_flags;
        if t_object_flags.intersects(ObjectFlags::REFERENCE) {
            // Deferred type reference
            declaration = self.ty(t).as_type_reference().node;
        } else if t_object_flags.intersects(ObjectFlags::INSTANTIATION_EXPRESSION_TYPE) {
            declaration = self.ty(t).as_instantiation_expression_type().node;
        } else {
            let t_symbol = self.ty(t).symbol;
            declaration = self.sym(t_symbol).declarations[0];
        }
        // PORT: both link fields are read with one lookup. The type parameter
        // list is a `SharedList`, so the clone copies no elements.
        let links = self.type_node_links.get(declaration);
        let resolved_type = links.resolved_type;
        let cached_outer_type_parameters = links.outer_type_parameters.clone();
        if t_object_flags.intersects(ObjectFlags::REFERENCE) {
            // Deferred type reference
            target = resolved_type;
        } else if t_object_flags.intersects(ObjectFlags::INSTANTIATED) {
            target = self.ty(t).target();
        } else {
            target = t;
        }
        // PORT: Go tells a nil `outerTypeParameters` (not computed) from an
        // empty one (computed, none in scope). `None` is Go nil, so an empty
        // list is also cached and the scope and reference walk runs once per
        // declaration, as in Go.
        let outer_type_parameters = if let Some(list) = cached_outer_type_parameters {
            list
        } else {
            // The first time an anonymous type is instantiated we compute and store a list of the type
            // parameters that are in scope (and therefore potentially referenced). For type literals that
            // aren't the right hand side of a generic type alias declaration we optimize by reducing the
            // set of type parameters to those that are possibly referenced in the literal.
            let mut type_parameters =
                self.get_outer_type_parameters(declaration, true /*includeThisTypes*/);
            if self.ty(target).alias.type_arguments().is_empty() {
                if t_object_flags
                    .intersects(ObjectFlags::REFERENCE | ObjectFlags::INSTANTIATION_EXPRESSION_TYPE)
                {
                    type_parameters = type_parameters
                        .into_iter()
                        .filter(|&tp| self.is_type_parameter_possibly_referenced(tp, declaration))
                        .collect();
                } else {
                    let target_symbol = self.ty(target).symbol;
                    if self
                        .sym(target_symbol)
                        .flags
                        .intersects(SymbolFlags::METHOD | SymbolFlags::TYPE_LITERAL)
                    {
                        let t_symbol = self.ty(t).symbol;
                        let declarations = self.sym(t_symbol).declarations.clone();
                        type_parameters = type_parameters
                            .into_iter()
                            .filter(|&tp| {
                                declarations
                                    .iter()
                                    .any(|&d| self.is_type_parameter_possibly_referenced(tp, d))
                            })
                            .collect();
                    }
                }
            }
            let list: SharedList<TypeId> = type_parameters.into();
            self.type_node_links.get(declaration).outer_type_parameters = Some(list.clone());
            list
        };
        if outer_type_parameters.is_empty() {
            return t;
        }
        // We are instantiating an anonymous type that has one or more type parameters in scope. Apply the
        // mapper to the type parameters to produce the effective list of type arguments, and compute the
        // instantiation cache key from the type IDs of the type arguments.
        // PORT: the type arguments stay on the stack, because a cache hit only
        // hashes them.
        let t_mapper = self.ty(t).mapper();
        let mut type_arguments: SmallVec<[TypeId; 8]> =
            SmallVec::with_capacity(outer_type_parameters.len());
        for &tp in outer_type_parameters.iter() {
            type_arguments.push(self.map_type_with_composite_mapper(tp, t_mapper, m));
        }
        // PORT: Go `c.instantiateTypeAlias(t.alias, m)` when `alias` is nil.
        // PERF: the instantiated alias type arguments go into a stack list,
        // and the key hashes the alias symbol and that list. The owned `Vec`
        // and the `Rc<TypeAlias>` are made only on a cache miss, because a
        // hit never uses them. `instantiate_types_into` makes the same
        // `instantiate_type` calls in the same order as `instantiate_types`.
        let mut instantiated_alias_type_arguments: SmallVec<[TypeId; 8]> = SmallVec::new();
        let instantiated_alias_symbol: Option<SymbolId> = if alias.is_none() {
            self.ty(t).alias.clone().map(|t_alias| {
                self.instantiate_types_into(
                    &t_alias.type_arguments,
                    m,
                    &mut instantiated_alias_type_arguments,
                );
                t_alias.symbol
            })
        } else {
            None
        };
        let key_alias = match &alias {
            Some(a) => Some((a.symbol, a.type_arguments.as_slice())),
            None => instantiated_alias_symbol
                .map(|symbol| (symbol, instantiated_alias_type_arguments.as_slice())),
        };
        let key = type_instantiation_key_parts(
            &self.symbols,
            &type_arguments,
            key_alias,
            t_object_flags.intersects(ObjectFlags::SINGLE_SIGNATURE_TYPE),
        );
        let mut result = match self.object_instantiations(target) {
            Some(instantiations) => instantiations.get(&key).copied().unwrap_or_default(),
            None => {
                let target_alias = self.ty(target).alias.clone();
                let target_key = get_type_instantiation_key(
                    &self.symbols,
                    &outer_type_parameters,
                    target_alias.as_deref(),
                    false,
                );
                let instantiations = self.object_instantiations_mut(target);
                instantiations.insert(target_key, target);
                instantiations.get(&key).copied().unwrap_or_default()
            }
        };
        if result.is_nil() {
            let new_alias = alias.or_else(|| {
                instantiated_alias_symbol.map(|symbol| {
                    Rc::new(TypeAlias {
                        symbol,
                        type_arguments: instantiated_alias_type_arguments.into_vec(),
                    })
                })
            });
            let type_arguments = SharedList::from(&type_arguments[..]);
            let mut new_mapper =
                self.new_type_mapper_shared(outer_type_parameters, type_arguments.clone());
            let target_object_flags = self.ty(target).object_flags;
            if target_object_flags.intersects(ObjectFlags::SINGLE_SIGNATURE_TYPE) && m.is_some() {
                new_mapper = self.combine_type_mappers(new_mapper, m);
            }
            if target_object_flags.intersects(ObjectFlags::REFERENCE) {
                let t_target = self.ty(t).target();
                let t_node = self.ty(t).as_type_reference().node;
                result =
                    self.create_deferred_type_reference(t_target, t_node, new_mapper, new_alias);
            } else if target_object_flags.intersects(ObjectFlags::MAPPED) {
                result = self.instantiate_mapped_type(target, new_mapper, new_alias);
            } else {
                result = self.instantiate_anonymous_type(target, new_mapper, new_alias);
            }
            self.object_instantiations_mut(target).insert(key, result);
            if self
                .ty(result)
                .flags
                .intersects(TypeFlags::OBJECT_FLAGS_TYPE)
                && !self
                    .ty(result)
                    .object_flags
                    .intersects(ObjectFlags::COULD_CONTAIN_TYPE_VARIABLES_COMPUTED)
            {
                // if `result` is one of the object types we tried to make (it may not be, due to how `instantiateMappedType` works), we can carry forward the type variable containment check from the input type arguments
                let result_could_contain_object_flags = type_arguments
                    .iter()
                    .any(|&a| self.could_contain_type_variables(a));
                if !self
                    .ty(result)
                    .object_flags
                    .intersects(ObjectFlags::COULD_CONTAIN_TYPE_VARIABLES_COMPUTED)
                {
                    if self.ty(result).object_flags.intersects(
                        ObjectFlags::MAPPED | ObjectFlags::ANONYMOUS | ObjectFlags::REFERENCE,
                    ) {
                        let extra = if result_could_contain_object_flags {
                            ObjectFlags::COULD_CONTAIN_TYPE_VARIABLES
                        } else {
                            ObjectFlags::NONE
                        };
                        self.ty_mut(result).object_flags |=
                            ObjectFlags::COULD_CONTAIN_TYPE_VARIABLES_COMPUTED | extra;
                    } else {
                        // If none of the type arguments for the outer type parameters contain type variables, it follows
                        // that the instantiated type doesn't reference type variables.
                        // Intrinsics have `CouldContainTypeVariablesComputed` pre-set, so this should only cover unions and intersections resulting from `instantiateMappedType`
                        let extra = if !result_could_contain_object_flags {
                            ObjectFlags::COULD_CONTAIN_TYPE_VARIABLES_COMPUTED
                        } else {
                            ObjectFlags::NONE
                        };
                        self.ty_mut(result).object_flags |= extra;
                    }
                }
            }
        }
        result
    }

    // Go: checker/checker.go:22823 isTypeParameterPossiblyReferenced
    pub fn is_type_parameter_possibly_referenced(&mut self, tp: TypeId, node: Node) -> bool {
        // If the type parameter doesn't have exactly one declaration, if there are intervening statement blocks
        // between the node and the type parameter declaration, if the node contains actual references to the
        // type parameter, or if the node contains type queries that we can't prove couldn't contain references to the type parameter,
        // we consider the type parameter possibly referenced.
        let tp_symbol = self.ty(tp).symbol;
        if tp_symbol.is_some() && self.sym(tp_symbol).declarations.len() == 1 {
            let container = self.sym(tp_symbol).declarations[0].parent();
            let mut n = node;
            while n != container {
                if n.is_nil()
                    || is_block(n)
                    || is_conditional_type_node(n)
                        && self.contains_reference_p25(tp, n.extends_type())
                {
                    return true;
                }
                n = n.parent();
            }
            return self.contains_reference_p25(tp, node);
        }
        true
    }

    // PORT: Go `containsReference`, the recursive closure inside
    // `isTypeParameterPossiblyReferenced`. It captures `tp`.
    fn contains_reference_p25(&mut self, tp: TypeId, node: Node) -> bool {
        match node.kind() {
            SyntaxKind::ThisType => {
                return self.ty(tp).as_type_parameter().is_this_type;
            }
            SyntaxKind::TypeReference => {
                // use worker because we're looking for === equality
                if !self.ty(tp).as_type_parameter().is_this_type
                    && node.type_arguments().is_empty()
                    && self.get_symbol_from_type_reference(node) == self.ty(tp).symbol
                {
                    return true;
                }
            }
            SyntaxKind::TypeQuery => {
                let entity_name = node.expr_name();
                let first_identifier = get_first_identifier(entity_name);
                if !is_this_identifier(first_identifier) {
                    let first_identifier_symbol = self.get_resolved_symbol(first_identifier);
                    let tp_symbol = self.ty(tp).symbol;
                    let tp_declaration = self.sym(tp_symbol).declarations[0]; // There is exactly one declaration, otherwise `containsReference` is not called
                    let tp_scope = if is_type_parameter_declaration(tp_declaration) {
                        tp_declaration.parent() // Type parameter is a regular type parameter, e.g. foo<T>
                    } else if self.ty(tp).as_type_parameter().is_this_type {
                        tp_declaration // Type parameter is the this type, and its declaration is the class declaration.
                    } else {
                        Node::NIL
                    };
                    if tp_scope.is_some() {
                        let declarations = self.sym(first_identifier_symbol).declarations.clone();
                        return declarations
                            .iter()
                            .any(|&d| is_node_descendant_of(d, tp_scope))
                            || node
                                .type_arguments()
                                .iter()
                                .any(|a| self.contains_reference_p25(tp, a));
                    }
                }
                return true;
            }
            SyntaxKind::MethodDeclaration | SyntaxKind::MethodSignature => {
                let return_type = node.type_();
                return return_type.is_nil() && node.body().is_some()
                    || node
                        .type_parameters()
                        .iter()
                        .any(|p| self.contains_reference_p25(tp, p))
                    || node
                        .parameters()
                        .iter()
                        .any(|p| self.contains_reference_p25(tp, p))
                    || return_type.is_some() && self.contains_reference_p25(tp, return_type);
            }
            _ => {}
        }
        node.for_each_child(&mut |child: Node| self.contains_reference_p25(tp, child))
    }

    // Go: checker/checker.go:22878 instantiateAnonymousType
    pub fn instantiate_anonymous_type(
        &mut self,
        t: TypeId,
        m: MapperId,
        alias: Option<Rc<TypeAlias>>,
    ) -> TypeId {
        let mut m = m;
        let mut alias = alias;
        let t_object_flags = self.ty(t).object_flags;
        let t_symbol = self.ty(t).symbol;
        let result = self.new_object_type(
            t_object_flags.without(
                ObjectFlags::COULD_CONTAIN_TYPE_VARIABLES_COMPUTED
                    | ObjectFlags::COULD_CONTAIN_TYPE_VARIABLES,
            ) | ObjectFlags::INSTANTIATED,
            t_symbol,
        );
        if t_object_flags.intersects(ObjectFlags::MAPPED) {
            let declaration = self.ty(t).as_mapped_type().declaration;
            self.ty_mut(result).as_mapped_type_mut().declaration = declaration;
            // C.f. instantiateSignature
            let orig_type_parameter = self.get_type_parameter_from_mapped_type(t);
            let fresh_type_parameter = self.clone_type_parameter(orig_type_parameter);
            self.ty_mut(result).as_mapped_type_mut().type_parameter = fresh_type_parameter;
            let simple = self.new_simple_type_mapper(orig_type_parameter, fresh_type_parameter);
            m = self.combine_type_mappers(simple, m);
            self.ty_mut(fresh_type_parameter)
                .as_type_parameter_mut()
                .mapper = m;
        } else if t_object_flags.intersects(ObjectFlags::INSTANTIATION_EXPRESSION_TYPE) {
            let node = self.ty(t).as_instantiation_expression_type().node;
            self.ty_mut(result)
                .as_instantiation_expression_type_mut()
                .node = node;
        }
        if alias.is_none() {
            let t_alias = self.ty(t).alias.clone();
            alias = self.instantiate_type_alias(t_alias, m);
        }
        self.ty_mut(result).alias = alias.clone();
        if let Some(a) = &alias {
            if !a.type_arguments.is_empty() {
                let flags = self.get_propagating_flags_of_types(&a.type_arguments, TypeFlags::NONE);
                self.ty_mut(result).object_flags |= flags;
            }
        }
        let d = self.ty_mut(result).as_object_type_mut();
        d.target = t;
        d.mapper = m;
        result
    }

    // Go: checker/checker.go:22905 getConditionalTypeInstantiation
    pub fn get_conditional_type_instantiation(
        &mut self,
        t: TypeId,
        mapper: MapperId,
        for_constraint: bool,
        alias: Option<Rc<TypeAlias>>,
    ) -> TypeId {
        self.get_conditional_type_instantiation_combined(
            t,
            MapperId::NIL,
            mapper,
            for_constraint,
            alias,
        )
    }

    /// Go `getConditionalTypeInstantiation(t, c.combineTypeMappers(m1, m2), ...)`.
    /// PORT: the mapper is only used to map the outer type parameters, so
    /// the combined mapper is applied without adding it to the arena.
    pub fn get_conditional_type_instantiation_combined(
        &mut self,
        t: TypeId,
        m1: MapperId,
        m2: MapperId,
        for_constraint: bool,
        alias: Option<Rc<TypeAlias>>,
    ) -> TypeId {
        let root = self.ty(t).as_conditional_type().root.clone();
        // PORT: the root's outer type parameters never change after it is
        // created. The shared list clone copies no elements, and the mapping
        // can reenter this root, so no borrow is held across it.
        let outer_type_parameters = root.borrow().outer_type_parameters.clone();
        if !outer_type_parameters.is_empty() {
            // We are instantiating a conditional type that has one or more type parameters in scope. Apply the
            // mapper to the type parameters to produce the effective list of type arguments, and compute the
            // instantiation cache key from the type IDs of the type arguments.
            // PORT: the type arguments stay on the stack, because a cache hit
            // only hashes them.
            let mut type_arguments: SmallVec<[TypeId; 8]> =
                SmallVec::with_capacity(outer_type_parameters.len());
            for &tp in outer_type_parameters.iter() {
                type_arguments.push(self.map_type_with_composite_mapper(tp, m1, m2));
            }
            let key = get_conditional_type_key(
                &self.symbols,
                &type_arguments,
                alias.as_deref(),
                for_constraint,
            );
            let mut result = root
                .borrow()
                .instantiations
                .as_ref()
                .and_then(|instantiations| instantiations.get(&key).copied())
                .unwrap_or_default();
            if result.is_nil() {
                // PORT: the mapper keeps the root's list, so a miss copies
                // only the type arguments.
                let new_mapper = self.new_type_mapper_shared(
                    outer_type_parameters,
                    SharedList::from(&type_arguments[..]),
                );
                let check_type = root.borrow().check_type;
                let is_distributive = root.borrow().is_distributive;
                let mut distribution_type = TypeId::NIL;
                if is_distributive {
                    let mapped = self.get_mapped_type(check_type, new_mapper);
                    distribution_type = self.get_reduced_type(mapped);
                }
                // Distributive conditional types are distributed over union types. For example, when the
                // distributive conditional type T extends U ? X : Y is instantiated with A | B for T, the
                // result is (A extends U ? X : Y) | (B extends U ? X : Y).
                if distribution_type.is_some()
                    && check_type != distribution_type
                    && self
                        .ty(distribution_type)
                        .flags
                        .intersects(TypeFlags::UNION | TypeFlags::NEVER)
                {
                    result = self.map_type_with_alias(
                        distribution_type,
                        &mut |c: &mut Checker, t: TypeId| -> TypeId {
                            let prepended = c.prepend_type_mapping(check_type, t, new_mapper);
                            c.get_conditional_type(root.clone(), prepended, for_constraint, None)
                        },
                        alias,
                    );
                } else {
                    result =
                        self.get_conditional_type(root.clone(), new_mapper, for_constraint, alias);
                }
                root.borrow_mut()
                    .instantiations
                    .get_or_insert_with(InstantiationMap::default)
                    .insert(key, result);
            }
            return result;
        }
        t
    }

    // Go: checker/checker.go:22938 cloneTypeParameter
    pub fn clone_type_parameter(&mut self, tp: TypeId) -> TypeId {
        let symbol = self.ty(tp).symbol;
        let result = self.new_type_parameter(symbol);
        self.ty_mut(result).as_type_parameter_mut().target = tp;
        result
    }

    // Go: checker/checker.go:22944 getHomomorphicTypeVariable
    pub fn get_homomorphic_type_variable(&mut self, t: TypeId) -> TypeId {
        let constraint_type = self.get_constraint_type_from_mapped_type(t);
        if self.ty(constraint_type).flags.intersects(TypeFlags::INDEX) {
            let index_target = self.ty(constraint_type).as_index_type().target;
            let type_variable = self.get_actual_type_variable(index_target);
            if self
                .ty(type_variable)
                .flags
                .intersects(TypeFlags::TYPE_PARAMETER)
            {
                return type_variable;
            }
        }
        TypeId::NIL
    }

    // Go: checker/checker.go:22955 instantiateMappedType
    pub fn instantiate_mapped_type(
        &mut self,
        t: TypeId,
        m: MapperId,
        alias: Option<Rc<TypeAlias>>,
    ) -> TypeId {
        // For a homomorphic mapped type { [P in keyof T]: X }, where T is some type variable, the mapping
        // operation depends on T as follows:
        // * If T is a primitive type no mapping is performed and the result is simply T.
        // * If T is a union type we distribute the mapped type over the union.
        // * If T is an array we map to an array where the element type has been transformed.
        // * If T is a tuple we map to a tuple where the element types have been transformed.
        // * If T is an intersection of array or tuple types we map to an intersection of transformed array or tuple types.
        // * Otherwise we map to an object type where the type of each property has been transformed.
        // For example, when T is instantiated to a union type A | B, we produce { [P in keyof A]: X } |
        // { [P in keyof B]: X }, and when when T is instantiated to a union type A | undefined, we produce
        // { [P in keyof A]: X } | undefined.
        let type_variable = self.get_homomorphic_type_variable(t);
        if type_variable.is_some() {
            let mapped_type_variable = self.instantiate_type(type_variable, m);
            if type_variable != mapped_type_variable {
                let reduced = self.get_reduced_type(mapped_type_variable);
                return self.map_type_with_alias(
                    reduced,
                    &mut |c: &mut Checker, s: TypeId| -> TypeId {
                        c.instantiate_mapped_type_constituent_p25(s, t, type_variable, m)
                    },
                    alias,
                );
            }
        }
        // If the constraint type of the instantiation is the wildcard type, return the wildcard type.
        let constraint_type = self.get_constraint_type_from_mapped_type(t);
        if self.instantiate_type(constraint_type, m) == self.wildcard_type {
            return self.wildcard_type;
        }
        self.instantiate_anonymous_type(t, m, alias)
    }

    // PORT: Go `instantiateConstituent`, the recursive closure inside
    // `instantiateMappedType`. It captures `t` (with `d := t.AsMappedType()`),
    // `typeVariable` and `m`.
    fn instantiate_mapped_type_constituent_p25(
        &mut self,
        s: TypeId,
        t: TypeId,
        type_variable: TypeId,
        m: MapperId,
    ) -> TypeId {
        let s_flags = self.ty(s).flags;
        if !s_flags.intersects(
            TypeFlags::ANY_OR_UNKNOWN
                | TypeFlags::INSTANTIABLE_NON_PRIMITIVE
                | TypeFlags::OBJECT
                | TypeFlags::INTERSECTION,
        ) || s == self.wildcard_type
            || self.is_error_type(s)
        {
            return s;
        }
        let declaration = self.ty(t).as_mapped_type().declaration;
        if declaration.name_type().is_nil() {
            if self.is_array_type(s)
                || s_flags.intersects(TypeFlags::ANY)
                    && self.find_resolution_cycle_start_index(
                        TypeSystemEntity::Type(type_variable),
                        TypeSystemPropertyName::RESOLVED_BASE_CONSTRAINT,
                    ) < 0
                    && self.has_array_or_type_type_constraint(type_variable)
            {
                let prepended = self.prepend_type_mapping(type_variable, s, m);
                return self.instantiate_mapped_array_type(s, t, prepended);
            }
            if self.is_tuple_type(s) {
                return self.instantiate_mapped_tuple_type(s, t, type_variable, m);
            }
            if self.is_array_or_tuple_or_intersection(s) {
                let mapped = self.map_constituents(s, &mut |c, constituent| {
                    c.instantiate_mapped_type_constituent_p25(constituent, t, type_variable, m)
                });
                return self.get_intersection_type(&mapped);
            }
        }
        let prepended = self.prepend_type_mapping(type_variable, s, m);
        self.instantiate_anonymous_type(t, prepended, None)
    }

    // Go: checker/checker.go:23000 hasArrayOrTypeTypeConstraint
    pub fn has_array_or_type_type_constraint(&mut self, type_variable: TypeId) -> bool {
        let constraint = self.get_constraint_of_type_parameter(type_variable);
        constraint.is_some()
            && self.every_type(constraint, &mut |c: &mut Checker, t: TypeId| {
                c.is_array_or_tuple_type(t)
            })
    }

    // Go: checker/checker.go:23005 instantiateMappedArrayType
    pub fn instantiate_mapped_array_type(
        &mut self,
        array_type: TypeId,
        mapped_type: TypeId,
        m: MapperId,
    ) -> TypeId {
        let number_type = self.number_type;
        let element_type = self.instantiate_mapped_type_template(
            mapped_type,
            number_type,
            true, /*isOptional*/
            m,
        );
        if self.is_error_type(element_type) {
            return self.error_type;
        }
        let is_readonly = self.is_readonly_array_type(array_type);
        let modifiers = self.get_mapped_type_modifiers(mapped_type);
        self.create_array_type_ex(
            element_type,
            get_modified_readonly_state(is_readonly, modifiers),
        )
    }

    // Go: checker/checker.go:23013 instantiateMappedTupleType
    pub fn instantiate_mapped_tuple_type(
        &mut self,
        tuple_type: TypeId,
        mapped_type: TypeId,
        type_variable: TypeId,
        m: MapperId,
    ) -> TypeId {
        // We apply the mapped type's template type to each of the fixed part elements. For variadic elements, we
        // apply the mapped type itself to the variadic element type. For other elements in the variable part of the
        // tuple, we surround the element type with an array type and apply the mapped type to that. This ensures
        // that we get sequential property key types for the fixed part of the tuple, and property key type number
        // for the remaining elements. For example
        //
        //   type Keys<T> = { [K in keyof T]: K };
        //   type Foo<T extends any[]> = Keys<[string, string, ...T, string]>; // ["0", "1", ...Keys<T>, number]
        //
        let element_infos = self.target_tuple_type(tuple_type).element_infos.clone();
        let fixed_length = self.target_tuple_type(tuple_type).fixed_length;
        let mut fixed_mapper = m;
        if fixed_length != 0 {
            fixed_mapper = self.prepend_type_mapping(type_variable, tuple_type, m);
        }
        let modifiers = self.get_mapped_type_modifiers(mapped_type);
        let element_types = self.get_element_types(tuple_type);
        let mut new_element_types: Vec<TypeId> = vec![TypeId::NIL; element_types.len()];
        let mut new_element_infos = element_infos.clone();
        for (i, &e) in element_types.iter().enumerate() {
            let flags = element_infos[i].flags;
            let mapped: TypeId;
            if (i as i32) < fixed_length {
                let key = self.get_string_literal_type(&i.to_string());
                mapped = self.instantiate_mapped_type_template(
                    mapped_type,
                    key,
                    flags.intersects(ElementFlags::OPTIONAL),
                    fixed_mapper,
                );
            } else if flags.intersects(ElementFlags::VARIADIC) {
                let prepended = self.prepend_type_mapping(type_variable, e, m);
                mapped = self.instantiate_type(mapped_type, prepended);
            } else {
                let array_type = self.create_array_type(e);
                let prepended = self.prepend_type_mapping(type_variable, array_type, m);
                let instantiated = self.instantiate_type(mapped_type, prepended);
                let element = self.get_element_type_of_array_type(instantiated);
                mapped = if element.is_nil() {
                    self.unknown_type
                } else {
                    element
                };
            }
            if modifiers.intersects(MappedTypeModifiers::INCLUDE_OPTIONAL) {
                if flags.intersects(ElementFlags::REQUIRED) {
                    new_element_infos[i].flags = ElementFlags::OPTIONAL;
                }
            } else if modifiers.intersects(MappedTypeModifiers::EXCLUDE_OPTIONAL) {
                if flags.intersects(ElementFlags::OPTIONAL) {
                    new_element_infos[i].flags = ElementFlags::REQUIRED;
                }
            }
            new_element_types[i] = mapped;
        }
        let tuple_readonly = self.target_tuple_type(tuple_type).readonly;
        let mapped_modifiers = self.get_mapped_type_modifiers(mapped_type);
        let new_readonly = get_modified_readonly_state(tuple_readonly, mapped_modifiers);
        if new_element_types.contains(&self.error_type) {
            return self.error_type;
        }
        self.create_tuple_type_ex(&new_element_types, &new_element_infos, new_readonly)
    }

    // Go: checker/checker.go:23066 instantiateMappedTypeTemplate
    pub fn instantiate_mapped_type_template(
        &mut self,
        t: TypeId,
        key: TypeId,
        is_optional: bool,
        m: MapperId,
    ) -> TypeId {
        let type_parameter = self.get_type_parameter_from_mapped_type(t);
        let template_mapper = self.append_type_mapping(m, type_parameter, key);
        let mapped_target = self.ty(t).as_mapped_type().object.target;
        let template_source = if mapped_target.is_some() {
            mapped_target
        } else {
            t
        };
        let template_type = self.get_template_type_from_mapped_type(template_source);
        let prop_type = self.instantiate_type(template_type, template_mapper);
        let modifiers = self.get_mapped_type_modifiers(t);
        if self.strict_null_checks
            && modifiers.intersects(MappedTypeModifiers::INCLUDE_OPTIONAL)
            && !self.maybe_type_of_kind(prop_type, TypeFlags::UNDEFINED | TypeFlags::VOID)
        {
            self.get_optional_type(prop_type, true /*isProperty*/)
        } else if self.strict_null_checks
            && modifiers.intersects(MappedTypeModifiers::EXCLUDE_OPTIONAL)
            && is_optional
        {
            self.remove_missing_or_undefined_type(prop_type)
        } else {
            prop_type
        }
    }
}

// Go: checker/checker.go:23080 getModifiedReadonlyState
pub fn get_modified_readonly_state(state: bool, modifiers: MappedTypeModifiers) -> bool {
    if modifiers.intersects(MappedTypeModifiers::INCLUDE_READONLY) {
        return true;
    } else if modifiers.intersects(MappedTypeModifiers::EXCLUDE_READONLY) {
        return false;
    }
    state
}

impl Checker {
    // Go: checker/checker.go:23090 getTypeParameterFromMappedType
    pub fn get_type_parameter_from_mapped_type(&mut self, t: TypeId) -> TypeId {
        if self.ty(t).as_mapped_type().type_parameter.is_nil() {
            let declaration = self.ty(t).as_mapped_type().declaration;
            let symbol = self.get_symbol_of_declaration(declaration.type_parameter());
            let type_parameter = self.get_declared_type_of_type_parameter(symbol);
            self.ty_mut(t).as_mapped_type_mut().type_parameter = type_parameter;
        }
        self.ty(t).as_mapped_type().type_parameter
    }

    // Go: checker/checker.go:23098 getConstraintTypeFromMappedType
    pub fn get_constraint_type_from_mapped_type(&mut self, t: TypeId) -> TypeId {
        if self.ty(t).as_mapped_type().constraint_type.is_nil() {
            let type_parameter = self.get_type_parameter_from_mapped_type(t);
            let constraint = self.get_constraint_of_type_parameter(type_parameter);
            let constraint_type = if constraint.is_some() {
                constraint
            } else {
                self.error_type
            };
            self.ty_mut(t).as_mapped_type_mut().constraint_type = constraint_type;
        }
        self.ty(t).as_mapped_type().constraint_type
    }

    // Go: checker/checker.go:23106 getNameTypeFromMappedType
    pub fn get_name_type_from_mapped_type(&mut self, t: TypeId) -> TypeId {
        let declaration = self.ty(t).as_mapped_type().declaration;
        if declaration.name_type().is_nil() {
            return TypeId::NIL;
        }
        if self.ty(t).as_mapped_type().name_type.is_nil() {
            let type_from_node = self.get_type_from_type_node(declaration.name_type());
            let mapper = self.ty(t).as_mapped_type().object.mapper;
            let name_type = self.instantiate_type(type_from_node, mapper);
            self.ty_mut(t).as_mapped_type_mut().name_type = name_type;
        }
        self.ty(t).as_mapped_type().name_type
    }

    // Go: checker/checker.go:23117 getTemplateTypeFromMappedType
    pub fn get_template_type_from_mapped_type(&mut self, t: TypeId) -> TypeId {
        if self.ty(t).as_mapped_type().template_type.is_nil() {
            let declaration = self.ty(t).as_mapped_type().declaration;
            let template_type = if declaration.type_().is_some() {
                let type_from_node = self.get_type_from_type_node(declaration.type_());
                let is_optional = self
                    .get_mapped_type_modifiers(t)
                    .intersects(MappedTypeModifiers::INCLUDE_OPTIONAL);
                let with_optionality =
                    self.add_optionality_ex(type_from_node /*isProperty*/, true, is_optional);
                let mapper = self.ty(t).as_mapped_type().object.mapper;
                self.instantiate_type(with_optionality, mapper)
            } else {
                self.error_type
            };
            self.ty_mut(t).as_mapped_type_mut().template_type = template_type;
        }
        self.ty(t).as_mapped_type().template_type
    }

    // Go: checker/checker.go:23129 isMappedTypeWithKeyofConstraintDeclaration
    pub fn is_mapped_type_with_keyof_constraint_declaration(&self, t: TypeId) -> bool {
        let constraint_declaration = self.get_constraint_declaration_for_mapped_type(t);
        is_type_operator_node(constraint_declaration)
            && constraint_declaration.operator() == SyntaxKind::KeyOfKeyword
    }

    // Go: checker/checker.go:23134 getConstraintDeclarationForMappedType
    pub fn get_constraint_declaration_for_mapped_type(&self, t: TypeId) -> Node {
        self.ty(t)
            .as_mapped_type()
            .declaration
            .type_parameter()
            .constraint()
    }

    // Go: checker/checker.go:23138 getApparentMappedTypeKeys
    pub fn get_apparent_mapped_type_keys(
        &mut self,
        name_type: TypeId,
        target_type: TypeId,
    ) -> TypeId {
        let modifiers_type_of_target = self.get_modifiers_type_from_mapped_type(target_type);
        let modifiers_type = self.get_apparent_type(modifiers_type_of_target);
        let mut mapped_keys: Vec<TypeId> = Vec::new();
        self.for_each_mapped_type_property_key_type_and_index_signature_key_type(
            modifiers_type,
            TypeFlags::STRING_OR_NUMBER_LITERAL_OR_UNIQUE,
            false,
            &mut |c: &mut Checker, t: TypeId| {
                let target_mapper = c.ty(target_type).mapper();
                let type_parameter = c.get_type_parameter_from_mapped_type(target_type);
                let mapper = c.append_type_mapping(target_mapper, type_parameter, t);
                mapped_keys.push(c.instantiate_type(name_type, mapper));
            },
        );
        self.get_union_type(&mapped_keys)
    }

    // Go: checker/checker.go:23147 forEachMappedTypePropertyKeyTypeAndIndexSignatureKeyType
    pub fn for_each_mapped_type_property_key_type_and_index_signature_key_type(
        &mut self,
        t: TypeId,
        include: TypeFlags,
        strings_only: bool,
        cb: &mut dyn FnMut(&mut Checker, TypeId),
    ) {
        for prop in self.get_properties_of_type(t) {
            let key_type = self.get_literal_type_from_property(prop, include, false);
            cb(self, key_type);
        }
        if self.ty(t).flags.intersects(TypeFlags::ANY) {
            let string_type = self.string_type;
            cb(self, string_type);
        } else {
            for info in self.get_index_infos_of_type(t) {
                let key_type = self.index_info(info).key_type;
                if !strings_only
                    || self
                        .ty(key_type)
                        .flags
                        .intersects(TypeFlags::STRING | TypeFlags::TEMPLATE_LITERAL)
                {
                    cb(self, key_type);
                }
            }
        }
    }

    // Go: checker/checker.go:23162 instantiateReverseMappedType
    pub fn instantiate_reverse_mapped_type(&mut self, t: TypeId, m: MapperId) -> TypeId {
        let r_mapped_type = self.ty(t).as_reverse_mapped_type().mapped_type;
        let r_constraint_type = self.ty(t).as_reverse_mapped_type().constraint_type;
        let r_source = self.ty(t).as_reverse_mapped_type().source;
        let inner_mapped_type = self.instantiate_type(r_mapped_type, m);
        if !self
            .ty(inner_mapped_type)
            .object_flags
            .intersects(ObjectFlags::MAPPED)
        {
            return t;
        }
        let inner_index_type = self.instantiate_type(r_constraint_type, m);
        if !self.ty(inner_index_type).flags.intersects(TypeFlags::INDEX) {
            return t;
        }
        let source = self.instantiate_type(r_source, m);
        let instantiated = self.infer_type_for_homomorphic_mapped_type(
            source,
            inner_mapped_type,
            inner_index_type,
        );
        if instantiated.is_some() {
            return instantiated;
        }
        t
        // Nested invocation of `inferTypeForHomomorphicMappedType` or the `source` instantiated into something unmappable
    }

    // Go: checker/checker.go:23180 instantiateTypeAlias
    pub fn instantiate_type_alias(
        &mut self,
        alias: Option<Rc<TypeAlias>>,
        m: MapperId,
    ) -> Option<Rc<TypeAlias>> {
        let alias = alias?;
        let type_arguments = self.instantiate_types(&alias.type_arguments, m);
        Some(Rc::new(TypeAlias {
            symbol: alias.symbol,
            type_arguments,
        }))
    }

    // Go: checker/checker.go:23187 instantiateTypes
    pub fn instantiate_types(&mut self, types: &[TypeId], m: MapperId) -> Vec<TypeId> {
        self.instantiate_list(types, m, Checker::instantiate_type)
    }

    /// Go `instantiateTypes` into a scratch list: `out` is cleared, then gets
    /// `instantiate_type(t, m)` for each `t` of `types`. Same calls in the
    /// same order as `instantiate_types`, with no heap list for up to 8.
    pub fn instantiate_types_into(
        &mut self,
        types: &[TypeId],
        m: MapperId,
        out: &mut SmallVec<[TypeId; 8]>,
    ) {
        out.clear();
        out.reserve(types.len());
        for &t in types {
            let instantiated = self.instantiate_type(t, m);
            out.push(instantiated);
        }
    }

    // Go: checker/checker.go:23191 instantiateSymbols
    pub fn instantiate_symbols(&mut self, symbols: &[SymbolId], m: MapperId) -> Vec<SymbolId> {
        self.instantiate_list(symbols, m, Checker::instantiate_symbol)
    }

    // Go: checker/checker.go:23195 instantiateSignatures
    pub fn instantiate_signatures(
        &mut self,
        signatures: &[SignatureId],
        m: MapperId,
    ) -> Vec<SignatureId> {
        self.instantiate_list(signatures, m, Checker::instantiate_signature)
    }

    // Go: checker/checker.go:23199 instantiateIndexInfos
    pub fn instantiate_index_infos(
        &mut self,
        index_infos: &[IndexInfoId],
        m: MapperId,
    ) -> Vec<IndexInfoId> {
        self.instantiate_list(index_infos, m, Checker::instantiate_index_info)
    }

    // Go: checker/checker.go:23203 instantiateList
    // PORT: Go returns the input slice itself when nothing changes. Rust
    // returns an owned copy in that case.
    pub fn instantiate_list<T: Copy + PartialEq>(
        &mut self,
        values: &[T],
        m: MapperId,
        instantiator: fn(&mut Checker, T, MapperId) -> T,
    ) -> Vec<T> {
        for (i, &value) in values.iter().enumerate() {
            let mapped = instantiator(self, value, m);
            if mapped != value {
                let mut result: Vec<T> = Vec::with_capacity(values.len());
                result.extend_from_slice(&values[..i]);
                result.push(mapped);
                for j in i + 1..values.len() {
                    result.push(instantiator(self, values[j], m));
                }
                return result;
            }
        }
        values.to_vec()
    }

    /// Go `instantiateList` for callers that only test `core.Same` on the
    /// result: `None` when no element changes (Go returns the input slice).
    pub fn instantiate_list_if_changed<T: Copy + PartialEq>(
        &mut self,
        values: &[T],
        m: MapperId,
        instantiator: fn(&mut Checker, T, MapperId) -> T,
    ) -> Option<Vec<T>> {
        for (i, &value) in values.iter().enumerate() {
            let mapped = instantiator(self, value, m);
            if mapped != value {
                let mut result: Vec<T> = Vec::with_capacity(values.len());
                result.extend_from_slice(&values[..i]);
                result.push(mapped);
                for &rest in &values[i + 1..] {
                    result.push(instantiator(self, rest, m));
                }
                return Some(result);
            }
        }
        None
    }

    // Go: checker/checker.go:23219 tryGetTypeFromTypeNode
    pub fn try_get_type_from_type_node(&mut self, node: Node) -> TypeId {
        let type_node = node.type_();
        if type_node.is_some() {
            return self.get_type_from_type_node(type_node);
        }
        TypeId::NIL
    }

    // Go: checker/checker.go:23227 getTypeFromTypeNode
    pub fn get_type_from_type_node(&mut self, node: Node) -> TypeId {
        let t = self.get_type_from_type_node_worker(node);
        self.get_conditional_flow_type_of_type(t, node)
    }

    // Go: checker/checker.go:23231 getTypeFromTypeNodeWorker
    pub fn get_type_from_type_node_worker(&mut self, node: Node) -> TypeId {
        match node.kind() {
            SyntaxKind::AnyKeyword | SyntaxKind::JsDocAllType => self.any_type,
            SyntaxKind::JsDocNonNullableType => self.get_type_from_type_node(node.type_()),
            SyntaxKind::JsDocNullableType => {
                let t = self.get_type_from_type_node(node.type_());
                if self.strict_null_checks {
                    self.get_nullable_type(t, TypeFlags::NULL)
                } else {
                    t
                }
            }
            SyntaxKind::JsDocVariadicType => {
                let t = self.get_type_from_type_node(node.type_());
                self.create_array_type(t)
            }
            SyntaxKind::JsDocOptionalType => {
                let t = self.get_type_from_type_node(node.type_());
                self.add_optionality(t)
            }
            SyntaxKind::UnknownKeyword => self.unknown_type,
            SyntaxKind::StringKeyword => self.string_type,
            SyntaxKind::NumberKeyword => self.number_type,
            SyntaxKind::BigIntKeyword => self.bigint_type,
            SyntaxKind::BooleanKeyword => self.boolean_type,
            SyntaxKind::SymbolKeyword => self.es_symbol_type,
            SyntaxKind::VoidKeyword => self.void_type,
            SyntaxKind::UndefinedKeyword => self.undefined_type,
            SyntaxKind::NullKeyword => self.null_type,
            SyntaxKind::NeverKeyword => self.never_type,
            SyntaxKind::ObjectKeyword => self.non_primitive_type,
            SyntaxKind::IntrinsicKeyword => self.intrinsic_marker_type,
            SyntaxKind::ThisType | SyntaxKind::ThisKeyword => {
                self.get_type_from_this_type_node(node)
            }
            SyntaxKind::LiteralType => self.get_type_from_literal_type_node(node),
            SyntaxKind::TypeReference | SyntaxKind::ExpressionWithTypeArguments => {
                self.get_type_from_type_reference(node)
            }
            SyntaxKind::TypePredicate => {
                if node.asserts_modifier().is_some() {
                    return self.void_type;
                }
                self.boolean_type
            }
            SyntaxKind::TypeQuery => self.get_type_from_type_query_node(node),
            SyntaxKind::ArrayType | SyntaxKind::TupleType => {
                self.get_type_from_array_or_tuple_type_node(node)
            }
            SyntaxKind::OptionalType => self.get_type_from_optional_type_node(node),
            SyntaxKind::UnionType => self.get_type_from_union_type_node(node),
            SyntaxKind::IntersectionType => self.get_type_from_intersection_type_node(node),
            SyntaxKind::NamedTupleMember => self.get_type_from_named_tuple_type_node(node),
            SyntaxKind::ParenthesizedType => self.get_type_from_type_node(node.type_()),
            SyntaxKind::RestType => self.get_type_from_rest_type_node(node),
            SyntaxKind::FunctionType | SyntaxKind::ConstructorType | SyntaxKind::TypeLiteral => {
                self.get_type_from_type_literal_or_function_or_constructor_type_node(node)
            }
            SyntaxKind::TypeOperator => self.get_type_from_type_operator_node(node),
            SyntaxKind::IndexedAccessType => self.get_type_from_indexed_access_type_node(node),
            SyntaxKind::TemplateLiteralType => self.get_type_from_template_type_node(node),
            SyntaxKind::MappedType => self.get_type_from_mapped_type_node(node),
            SyntaxKind::ConditionalType => self.get_type_from_conditional_type_node(node),
            SyntaxKind::InferType => self.get_type_from_infer_type_node(node),
            SyntaxKind::ImportType => self.get_type_from_import_type_node(node),
            _ => self.error_type,
        }
    }

    // Go: checker/checker.go:23320 getTypeFromThisTypeNode
    pub fn get_type_from_this_type_node(&mut self, node: Node) -> TypeId {
        if self.type_node_links.get(node).resolved_type.is_nil() {
            let t = self.get_this_type(node);
            self.type_node_links.get(node).resolved_type = t;
        }
        self.type_node_links.get(node).resolved_type
    }

    // Go: checker/checker.go:23328 getThisType
    pub fn get_this_type(&mut self, node: Node) -> TypeId {
        let container = get_this_container(
            node,  /*includeArrowFunctions*/
            false, /*includeClassComputedPropertyName*/
            false,
        );
        if container.is_some() {
            let parent = container.parent();
            if parent.is_some() && (is_class_like(parent) || is_interface_declaration(parent)) {
                if !is_static(container)
                    && (!is_constructor_declaration(container)
                        || is_node_descendant_of(node, container.body()))
                {
                    let symbol = self.get_symbol_of_declaration(parent);
                    let declared = self.get_declared_type_of_class_or_interface(symbol);
                    let this_type = self.ty(declared).as_interface_type().this_type;
                    return if this_type.is_some() {
                        this_type
                    } else {
                        self.error_type
                    };
                }
            }
        }
        self.error(
            node,
            diag::A_this_type_is_available_only_in_a_non_static_member_of_a_class_or_interface,
            args![],
        );
        self.error_type
    }

    // Go: checker/checker.go:23342 getTypeFromLiteralTypeNode
    pub fn get_type_from_literal_type_node(&mut self, node: Node) -> TypeId {
        if node.literal().kind() == SyntaxKind::NullKeyword {
            return self.null_type;
        }
        if self.type_node_links.get(node).resolved_type.is_nil() {
            let t = self.check_expression(node.literal());
            let resolved = self.get_regular_type_of_literal_type(t);
            self.type_node_links.get(node).resolved_type = resolved;
        }
        self.type_node_links.get(node).resolved_type
    }

    // Go: checker/checker.go:23353 getTypeFromTypeLiteralOrFunctionOrConstructorTypeNode
    pub fn get_type_from_type_literal_or_function_or_constructor_type_node(
        &mut self,
        node: Node,
    ) -> TypeId {
        if self.type_node_links.get(node).resolved_type.is_nil() {
            // Deferred resolution of members is handled by resolveObjectTypeMembers
            let alias = self.get_alias_for_type_node(node);
            let sym = node.symbol();
            let resolved = if sym.is_nil() || {
                let members = self.get_members_of_symbol(sym);
                self.symbols.len(members) == 0 && alias.is_none()
            } {
                self.empty_type_literal_type
            } else {
                let t = self.new_object_type(ObjectFlags::ANONYMOUS, node.symbol());
                self.ty_mut(t).alias = alias;
                t
            };
            self.type_node_links.get(node).resolved_type = resolved;
        }
        self.type_node_links.get(node).resolved_type
    }

    // Go: checker/checker.go:23369 getTypeFromIndexedAccessTypeNode
    pub fn get_type_from_indexed_access_type_node(&mut self, node: Node) -> TypeId {
        if self.type_node_links.get(node).resolved_type.is_nil() {
            let object_type = self.get_type_from_type_node(node.object_type());
            let index_type = self.get_type_from_type_node(node.index_type());
            let potential_alias = self.get_alias_for_type_node(node);
            let resolved = self.get_indexed_access_type_ex(
                object_type,
                index_type,
                AccessFlags::NONE,
                node,
                potential_alias,
            );
            self.type_node_links.get(node).resolved_type = resolved;
        }
        self.type_node_links.get(node).resolved_type
    }

    // Go: checker/checker.go:23380 getTypeFromTypeOperatorNode
    pub fn get_type_from_type_operator_node(&mut self, node: Node) -> TypeId {
        if self.type_node_links.get(node).resolved_type.is_nil() {
            let arg_type = node.type_();
            let resolved = match node.operator() {
                SyntaxKind::KeyOfKeyword => {
                    let t = self.get_type_from_type_node(arg_type);
                    self.get_index_type(t)
                }
                SyntaxKind::UniqueKeyword => {
                    if arg_type.kind() == SyntaxKind::SymbolKeyword {
                        self.get_es_symbol_like_type_for_node(walk_up_parenthesized_types(
                            node.parent(),
                        ))
                    } else {
                        self.error_type
                    }
                }
                SyntaxKind::ReadonlyKeyword => self.get_type_from_type_node(arg_type),
                _ => panic!("Unhandled case in getTypeFromTypeOperatorNode"),
            };
            self.type_node_links.get(node).resolved_type = resolved;
        }
        self.type_node_links.get(node).resolved_type
    }

    // Go: checker/checker.go:23402 getESSymbolLikeTypeForNode
    pub fn get_es_symbol_like_type_for_node(&mut self, node: Node) -> TypeId {
        if is_valid_es_symbol_declaration(node) {
            let symbol = self.get_symbol_of_node(node);
            if symbol.is_some() {
                let mut unique_type = self
                    .unique_es_symbol_types
                    .get(&symbol)
                    .copied()
                    .unwrap_or_default();
                if unique_type.is_nil() {
                    let mut b = String::new();
                    b.push_str(INTERNAL_SYMBOL_NAME_PREFIX);
                    b.push('@');
                    b.push_str(&self.sym(symbol).name);
                    b.push('@');
                    b.push_str(&get_symbol_id(&self.symbols, symbol).to_string());
                    unique_type = self.new_unique_es_symbol_type(symbol, &b);
                    self.unique_es_symbol_types.insert(symbol, unique_type);
                }
                return unique_type;
            }
        }
        self.es_symbol_type
    }

    // Go: checker/checker.go:23423 getTypeFromTypeReference
    pub fn get_type_from_type_reference(&mut self, node: Node) -> TypeId {
        // PORT: a cache hit reads the links once. The miss path stores the
        // result and returns it, which is what Go reads back from the links.
        let cached = self.type_node_links.get(node).resolved_type;
        if cached.is_nil() {
            // Cache both the resolved symbol and the resolved type. The resolved symbol is needed when we check the
            // type reference in checkTypeReferenceNode.
            // handle LS queries on the `const` in `x as const` by resolving to the type of `x`
            let resolved =
                if is_const_type_reference(node) && is_assertion_expression(node.parent()) {
                    self.check_expression_cached(node.parent().expression())
                } else {
                    let t = self.get_intended_type_from_js_doc_type_reference(node);
                    if t.is_some() {
                        t
                    } else {
                        let symbol = self.get_symbol_from_type_reference(node);
                        let t = self.get_type_reference_type(node, symbol);
                        self.get_distributed_type_parameter(node, t)
                    }
                };
            self.type_node_links.get(node).resolved_type = resolved;
            return resolved;
        }
        cached
    }

    // Go: checker/checker.go:23440 getDistributedTypeParameter
    pub fn get_distributed_type_parameter(&mut self, node: Node, t: TypeId) -> TypeId {
        if self.ty(t).flags.intersects(TypeFlags::TYPE_PARAMETER)
            && !self.ty(t).as_type_parameter().is_distributed
        {
            let mut n = node.parent();
            while n.is_some() && !is_statement(n) {
                if is_conditional_type_node(n) {
                    let check_type_node = n.check_type();
                    if is_simple_identifier_type_reference(check_type_node)
                        && self.get_symbol_from_type_reference(check_type_node) == self.ty(t).symbol
                    {
                        // If node is contained in a distributive conditional type for the given type parameter,
                        // return the distributed form of the type parameter.
                        return self.get_distributed_type_from_type_parameter(t);
                    }
                }
                n = n.parent();
            }
        }
        t
    }

    // Go: checker/checker.go:23455 getDistributedTypeFromTypeParameter
    pub fn get_distributed_type_from_type_parameter(&mut self, t: TypeId) -> TypeId {
        if self.ty(t).as_type_parameter().distributed_type.is_nil() {
            let symbol = self.ty(t).symbol;
            let distributed_type = self.new_type_parameter(symbol);
            self.ty_mut(t).as_type_parameter_mut().distributed_type = distributed_type;
            let tp = self.ty_mut(distributed_type).as_type_parameter_mut();
            tp.is_distributed = true;
            tp.constraint = t;
        }
        self.ty(t).as_type_parameter().distributed_type
    }

    // Go: checker/checker.go:23465 getNonDistributedTypeParameter
    // PORT: a Go package function; a `Checker` method here because it reads
    // the type arena.
    pub fn get_non_distributed_type_parameter(&self, t: TypeId) -> TypeId {
        let ty = self.ty(t);
        if ty.flags.intersects(TypeFlags::TYPE_PARAMETER) && ty.as_type_parameter().is_distributed {
            return ty.as_type_parameter().constraint;
        }
        t
    }

    // Go: checker/checker.go:23476 getIntendedTypeFromJSDocTypeReference
    pub fn get_intended_type_from_js_doc_type_reference(&mut self, node: Node) -> TypeId {
        if node.flags().intersects(NodeFlags::JS_DOC) && is_type_reference_node(node) {
            let type_name = node.type_name();
            if is_identifier(type_name) {
                let type_args = node.type_arguments();
                match type_name.text() {
                    "String" => {
                        self.check_no_type_arguments(node, SymbolId::NIL);
                        return self.string_type;
                    }
                    "Number" => {
                        self.check_no_type_arguments(node, SymbolId::NIL);
                        return self.number_type;
                    }
                    "BigInt" => {
                        self.check_no_type_arguments(node, SymbolId::NIL);
                        return self.bigint_type;
                    }
                    "Boolean" => {
                        self.check_no_type_arguments(node, SymbolId::NIL);
                        return self.boolean_type;
                    }
                    "Void" => {
                        self.check_no_type_arguments(node, SymbolId::NIL);
                        return self.void_type;
                    }
                    "Undefined" => {
                        self.check_no_type_arguments(node, SymbolId::NIL);
                        return self.undefined_type;
                    }
                    "Null" => {
                        self.check_no_type_arguments(node, SymbolId::NIL);
                        return self.null_type;
                    }
                    "Function" | "function" => {
                        self.check_no_type_arguments(node, SymbolId::NIL);
                        return self.global_function_type;
                    }
                    "array" => {
                        if type_args.is_empty() && !self.no_implicit_any {
                            return self.any_array_type;
                        }
                    }
                    "promise" => {
                        if type_args.is_empty() && !self.no_implicit_any {
                            let any_type = self.any_type;
                            return self.create_promise_type(any_type);
                        }
                    }
                    "Object" => {
                        if type_args.len() == 2 {
                            let record_symbol = self.get_global_record_symbol();
                            if record_symbol.is_some() {
                                let index_type = self.get_type_from_type_node(type_args.get(0));
                                if self.is_valid_index_key_type(index_type) {
                                    let value_type = self.get_type_from_type_node(type_args.get(1));
                                    return self.get_type_alias_instantiation(
                                        record_symbol,
                                        &[index_type, value_type],
                                        None,
                                    );
                                }
                            }
                            return self.any_type;
                        }
                        if !self.no_implicit_any {
                            self.check_no_type_arguments(node, SymbolId::NIL);
                            return self.any_type;
                        }
                    }
                    _ => {}
                }
            }
        }
        TypeId::NIL
    }

    // Go: checker/checker.go:23533 getSymbolFromTypeReference
    pub fn get_symbol_from_type_reference(&mut self, node: Node) -> SymbolId {
        if self.symbol_node_links.get(node).resolved_symbol.is_nil() {
            // The `const` in a `const` assertion resolves to nothing; resolveName knows not to
            // report an error for it, so no special-casing is needed here.
            let resolved = self.resolve_type_reference_name(
                node,
                SymbolFlags::TYPE,
                false, /*ignoreErrors*/
            );
            self.symbol_node_links.get(node).resolved_symbol = resolved;
        }
        self.symbol_node_links.get(node).resolved_symbol
    }

    // Go: checker/checker.go:23543 resolveTypeReferenceName
    pub fn resolve_type_reference_name(
        &mut self,
        type_reference: Node,
        meaning: SymbolFlags,
        ignore_errors: bool,
    ) -> SymbolId {
        let name = get_type_reference_name(type_reference);
        if name.is_nil() {
            return self.unknown_symbol;
        }
        let symbol = self.resolve_entity_name(
            name,
            meaning,
            ignore_errors,
            false,     /*dontResolveAlias*/
            Node::NIL, /*location*/
        );
        if symbol.is_some() && symbol != self.unknown_symbol {
            return symbol;
        }
        if ignore_errors {
            return self.unknown_symbol;
        }
        self.get_unresolved_symbol_for_entity_name(name)
    }

    // Go: checker/checker.go:23558 getUnresolvedSymbolForEntityName
    pub fn get_unresolved_symbol_for_entity_name(&mut self, name: Node) -> SymbolId {
        let identifier = match name.kind() {
            SyntaxKind::QualifiedName => name.right(),
            SyntaxKind::PropertyAccessExpression => name.name(),
            _ => name,
        };
        let text = identifier.text();
        if !text.is_empty() {
            let parent_symbol = match name.kind() {
                SyntaxKind::QualifiedName => {
                    self.get_unresolved_symbol_for_entity_name(name.left())
                }
                SyntaxKind::PropertyAccessExpression => {
                    self.get_unresolved_symbol_for_entity_name(name.expression())
                }
                _ => SymbolId::NIL,
            };
            let path = if parent_symbol.is_some() {
                format!("{}.{}", self.get_symbol_path(parent_symbol), text)
            } else {
                text.to_string()
            };
            let mut result = self
                .unresolved_symbols
                .get(&path)
                .copied()
                .unwrap_or_default();
            if result.is_nil() {
                result = self.new_symbol_ex(SymbolFlags::TYPE_ALIAS, text, CheckFlags::UNRESOLVED);
                self.unresolved_symbols.insert(path, result);
                self.sym_mut(result).parent = parent_symbol;
                let unresolved_type = self.unresolved_type;
                self.type_alias_links.get(result).declared_type = unresolved_type;
            }
            return result;
        }
        self.unknown_symbol
    }

    // Go: checker/checker.go:23595 getSymbolPath
    pub fn get_symbol_path(&self, symbol: SymbolId) -> String {
        let parent = self.sym(symbol).parent;
        if parent.is_some() {
            return format!("{}.{}", self.get_symbol_path(parent), self.sym(symbol).name);
        }
        self.sym(symbol).name.to_string()
    }

    // Go: checker/checker.go:23602 getTypeReferenceType
    pub fn get_type_reference_type(&mut self, node: Node, symbol: SymbolId) -> TypeId {
        if symbol == self.unknown_symbol {
            return self.error_type;
        }
        let flags = self.sym(symbol).flags;
        if flags.intersects(SymbolFlags::CLASS | SymbolFlags::INTERFACE) {
            return self.get_type_from_class_or_interface_reference(node, symbol);
        }
        if flags.intersects(SymbolFlags::TYPE_ALIAS) {
            return self.get_type_from_type_alias_reference(node, symbol);
        }
        // Get type from reference to named type that cannot be generic (enum or type parameter)
        let res = self.try_get_declared_type_of_symbol(symbol);
        if res.is_some() && self.check_no_type_arguments(node, symbol) {
            return self.get_regular_type_of_literal_type(res);
        }

        // !!! Resolving values as types for JS
        self.error_type
    }

    /**
     * Get type from type-reference that reference to class or interface
     */
    // Go: checker/checker.go:23625 getTypeFromClassOrInterfaceReference
    pub fn get_type_from_class_or_interface_reference(
        &mut self,
        node: Node,
        symbol: SymbolId,
    ) -> TypeId {
        let merged = self.get_merged_symbol(symbol);
        let t = self.get_declared_type_of_class_or_interface(merged);
        let type_parameters = self
            .ty(t)
            .as_interface_type()
            .local_type_parameters()
            .to_vec();
        if !type_parameters.is_empty() {
            let num_type_arguments = node.type_arguments().len() as i32;
            let min_type_argument_count = self.get_min_type_argument_count(&type_parameters);
            let is_js = is_in_js_file(node);
            let is_js_implicit_any = !self.no_implicit_any && is_js;
            if !is_js_implicit_any
                && (num_type_arguments < min_type_argument_count
                    || num_type_arguments > type_parameters.len() as i32)
            {
                let message: &'static crate::diagnostics::Message;

                let missing_augments_tag = is_js
                    && is_expression_with_type_arguments(node)
                    && !is_js_doc_augments_tag(node.parent());
                if missing_augments_tag {
                    message = if (min_type_argument_count as usize) < type_parameters.len() {
                        diag::Expected_0_1_type_arguments_provide_these_with_an_extends_tag
                    } else {
                        diag::Expected_0_type_arguments_provide_these_with_an_extends_tag
                    };
                } else {
                    message = if (min_type_argument_count as usize) < type_parameters.len() {
                        diag::Generic_type_0_requires_between_1_and_2_type_arguments
                    } else {
                        diag::Generic_type_0_requires_1_type_argument_s
                    };
                }
                let type_str = self.type_to_string_ex(
                    t,
                    Node::NIL, /*enclosingDeclaration*/
                    TypeFormatFlags::WRITE_ARRAY_AS_GENERIC_TYPE,
                    None,
                );
                self.error(
                    node,
                    message,
                    args![type_str, min_type_argument_count, type_parameters.len()],
                );
                if !is_js {
                    // TODO: Adopt same permissive behavior in TS as in JS to reduce follow-on editing experience failures (requires editing fillMissingTypeArguments)
                    return self.error_type;
                }
            }
            if node.kind() == SyntaxKind::TypeReference
                && self.is_deferred_type_reference_node(
                    node,
                    num_type_arguments != type_parameters.len() as i32,
                )
            {
                return self.create_deferred_type_reference(
                    t,
                    node,
                    MapperId::NIL, /*mapper*/
                    None,          /*alias*/
                );
            }
            // In a type reference, the outer type parameters of the referenced class or interface are automatically
            // supplied as type arguments and the type reference only specifies arguments for the local type parameters
            // of the class or interface.
            let node_type_arguments = self.get_type_arguments_from_node(node);
            let local_type_arguments = self.fill_missing_type_arguments(
                &node_type_arguments,
                &type_parameters,
                min_type_argument_count,
                is_js,
            );
            // PORT: the arguments stay on the stack; `create_type_reference_ex`
            // copies them only on a cache miss.
            // PERF: copy loops, not `from_slice` (memmove and memcpy calls).
            let outer_type_parameters = self.ty(t).as_interface_type().outer_type_parameters();
            let mut type_arguments: SmallVec<[TypeId; 8]> =
                SmallVec::with_capacity(outer_type_parameters.len() + local_type_arguments.len());
            type_arguments.extend(outer_type_parameters.iter().copied());
            type_arguments.extend(local_type_arguments);
            return self.create_type_reference_ex(t, &type_arguments, ObjectFlags::FROM_TYPE_NODE);
        }
        if self.check_no_type_arguments(node, symbol) {
            return t;
        }
        self.error_type
    }
}

// Go: checker/checker.go:23472 isSimpleIdentifierTypeReference
pub fn is_simple_identifier_type_reference(node: Node) -> bool {
    is_type_reference_node(node)
        && is_identifier(node.type_name())
        && node.type_argument_list().is_nil()
}
