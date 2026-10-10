//! Port of Go `checker/checker.go` lines 24046-24956 (checker-27):
//! tuple, union, intersection, template, mapped, conditional, infer and
//! import type nodes, conditional type resolution, tuple targets, generic
//! object flags, and the basic type constructors (`newType` ...
//! `newLiteralType`).

use crate::jsnum::Number;
use crate::prelude::*;
use smallvec::SmallVec;

impl Checker {
    // Go: checker/checker.go:24609 isVariadicTupleElement
    pub fn is_variadic_tuple_element(&mut self, node: Node) -> bool {
        self.get_tuple_element_flags(node)
            .intersects(ElementFlags::VARIADIC)
    }

    // Go: checker/checker.go:24613 getArrayOrTupleTargetType
    pub fn get_array_or_tuple_target_type(&mut self, node: Node) -> TypeId {
        let readonly = self.is_readonly_type_operator(node.parent());
        let element_type = self.get_array_element_type_node(node);
        if element_type.is_some() {
            if readonly {
                return self.global_readonly_array_type;
            }
            return self.global_array_type;
        }
        let elements = node.elements();
        let mut element_infos: Vec<TupleElementInfo> = Vec::with_capacity(elements.len());
        for e in elements {
            element_infos.push(self.get_tuple_element_info(e));
        }
        self.get_tuple_target_type(&element_infos, readonly)
    }

    // Go: checker/checker.go:24625 isReadonlyTypeOperator
    pub fn is_readonly_type_operator(&self, node: Node) -> bool {
        is_type_operator_node(node) && node.operator() == SyntaxKind::ReadonlyKeyword
    }

    // Go: checker/checker.go:24629 getTypeFromNamedTupleTypeNode
    pub fn get_type_from_named_tuple_type_node(&mut self, node: Node) -> TypeId {
        if self.type_node_links.get(node).resolved_type.is_nil() {
            let resolved = if node.dot_dot_dot_token().is_some() {
                self.get_type_from_rest_type_node(node)
            } else {
                let t = self.get_type_from_type_node(node.type_());
                self.add_optionality_ex(
                    t,
                    true, /*isProperty*/
                    node.question_token().is_some(),
                )
            };
            self.type_node_links.get(node).resolved_type = resolved;
        }
        self.type_node_links.get(node).resolved_type
    }

    // Go: checker/checker.go:24641 getTypeFromRestTypeNode
    pub fn get_type_from_rest_type_node(&mut self, node: Node) -> TypeId {
        let mut type_node = node.type_();
        let element_type_node = self.get_array_element_type_node(type_node);
        if element_type_node.is_some() {
            type_node = element_type_node;
        }
        self.get_type_from_type_node(type_node)
    }

    // Go: checker/checker.go:24650 getArrayElementTypeNode
    pub fn get_array_element_type_node(&self, node: Node) -> Node {
        match node.kind() {
            SyntaxKind::ParenthesizedType => {
                return self.get_array_element_type_node(node.type_());
            }
            SyntaxKind::TupleType => {
                if node.elements().len() == 1 {
                    let node = node.elements().get(0);
                    if node.kind() == SyntaxKind::RestType {
                        return self.get_array_element_type_node(node.type_());
                    }
                    if node.kind() == SyntaxKind::NamedTupleMember
                        && node.dot_dot_dot_token().is_some()
                    {
                        return self.get_array_element_type_node(node.type_());
                    }
                }
            }
            SyntaxKind::ArrayType => {
                return node.element_type();
            }
            _ => {}
        }
        Node::NIL
    }

    // Go: checker/checker.go:24670 getTypeFromOptionalTypeNode
    pub fn get_type_from_optional_type_node(&mut self, node: Node) -> TypeId {
        let t = self.get_type_from_type_node(node.type_());
        self.add_optionality_ex(t, true /*isProperty*/, true /*isOptional*/)
    }

    // Go: checker/checker.go:24674 getTypeFromUnionTypeNode
    pub fn get_type_from_union_type_node(&mut self, node: Node) -> TypeId {
        if self.type_node_links.get(node).resolved_type.is_nil() {
            let alias = self.get_alias_for_type_node(node);
            let nodes = node.types().nodes();
            let mut types: Vec<TypeId> = Vec::with_capacity(nodes.len());
            for n in nodes {
                types.push(self.get_type_from_type_node(n));
            }
            let resolved = self.get_union_type_ex(
                &types,
                UnionReduction::LITERAL,
                alias,
                TypeId::NIL, /*origin*/
            );
            self.type_node_links.get(node).resolved_type = resolved;
        }
        self.type_node_links.get(node).resolved_type
    }

    // Go: checker/checker.go:24683 getTypeFromIntersectionTypeNode
    pub fn get_type_from_intersection_type_node(&mut self, node: Node) -> TypeId {
        if self.type_node_links.get(node).resolved_type.is_nil() {
            let alias = self.get_alias_for_type_node(node);
            let nodes = node.types().nodes();
            let mut types: Vec<TypeId> = Vec::with_capacity(nodes.len());
            for n in nodes {
                types.push(self.get_type_from_type_node(n));
            }
            // We perform no supertype reduction for X & {} or {} & X, where X is one of string, number, bigint,
            // or a pattern literal template type. This enables union types like "a" | "b" | string & {} or
            // "aa" | "ab" | `a${string}` which preserve the literal types for purposes of statement completion.
            let mut no_supertype_reduction = false;
            if types.len() == 2 {
                let empty_type_literal_type = self.empty_type_literal_type;
                if let Some(empty_index) = types.iter().position(|&t| t == empty_type_literal_type)
                {
                    let t = types[1 - empty_index];
                    let flags = self.ty(t).flags;
                    no_supertype_reduction = flags
                        .intersects(TypeFlags::STRING | TypeFlags::NUMBER | TypeFlags::BIG_INT)
                        || flags.intersects(TypeFlags::TEMPLATE_LITERAL)
                            && self.is_pattern_literal_type(t);
                }
            }
            let flags = if no_supertype_reduction {
                IntersectionFlags::NO_SUPERTYPE_REDUCTION
            } else {
                IntersectionFlags::NONE
            };
            let resolved = self.get_intersection_type_ex(&types, flags, alias);
            self.type_node_links.get(node).resolved_type = resolved;
        }
        self.type_node_links.get(node).resolved_type
    }

    // Go: checker/checker.go:24704 getTypeFromTemplateTypeNode
    pub fn get_type_from_template_type_node(&mut self, node: Node) -> TypeId {
        if self.type_node_links.get(node).resolved_type.is_nil() {
            let spans = node.template_spans().nodes().to_vec();
            let mut texts: Vec<String> = vec![String::new(); spans.len() + 1];
            let mut types: Vec<TypeId> = vec![TypeId::NIL; spans.len()];
            texts[0] = node.head().text().to_string();
            for (i, span) in spans.iter().copied().enumerate() {
                texts[i + 1] = span.literal().text().to_string();
                types[i] = self.get_type_from_type_node(span.type_());
            }
            let resolved = self.get_template_literal_type(&texts, &types);
            self.type_node_links.get(node).resolved_type = resolved;
        }
        self.type_node_links.get(node).resolved_type
    }

    // Go: checker/checker.go:24720 getTypeFromMappedTypeNode
    pub fn get_type_from_mapped_type_node(&mut self, node: Node) -> TypeId {
        if self.type_node_links.get(node).resolved_type.is_nil() {
            let t = self.new_object_type(ObjectFlags::MAPPED, node.symbol());
            self.ty_mut(t).as_mapped_type_mut().declaration = node;
            let alias = self.get_alias_for_type_node(node);
            self.ty_mut(t).alias = alias;
            self.type_node_links.get(node).resolved_type = t;
            // Eagerly resolve the constraint type which forces an error if the constraint type circularly
            // references itself through one or more type aliases.
            self.get_constraint_type_from_mapped_type(t);
        }
        self.type_node_links.get(node).resolved_type
    }

    // Go: checker/checker.go:24734 getTypeFromConditionalTypeNode
    pub fn get_type_from_conditional_type_node(&mut self, node: Node) -> TypeId {
        if self.type_node_links.get(node).resolved_type.is_nil() {
            let check_type = self.get_type_from_type_node(node.check_type());
            let alias = self.get_alias_for_type_node(node);
            let all_outer_type_parameters =
                self.get_outer_type_parameters(node, true /*includeThisTypes*/);
            let outer_type_parameters: SharedList<TypeId> =
                if alias.is_some() && !alias.type_arguments().is_empty() {
                    all_outer_type_parameters.into()
                } else {
                    let mut filtered = Vec::with_capacity(all_outer_type_parameters.len());
                    for tp in all_outer_type_parameters {
                        if self.is_type_parameter_possibly_referenced(tp, node) {
                            filtered.push(tp);
                        }
                    }
                    filtered.into()
                };
            let extends_type = self.get_type_from_type_node(node.extends_type());
            let is_distributive = self
                .ty(check_type)
                .flags
                .intersects(TypeFlags::TYPE_PARAMETER);
            let infer_type_parameters = self.get_infer_type_parameters(node);
            let root = Rc::new(RefCell::new(ConditionalRoot {
                node,
                check_type,
                extends_type,
                is_distributive,
                infer_type_parameters,
                outer_type_parameters: outer_type_parameters.clone(),
                instantiations: None,
                alias,
                distribution_dependent: None,
            }));
            let resolved = self.get_conditional_type(
                root.clone(),
                MapperId::NIL, /*mapper*/
                false,         /*forConstraint*/
                None,
            );
            self.type_node_links.get(node).resolved_type = resolved;
            // PORT: Go tests `outerTypeParameters != nil`. A non-nil empty list only
            // creates a map that is never read (instantiations are only used when
            // there are outer type parameters), so testing for non-empty is equivalent.
            if !outer_type_parameters.is_empty() {
                let mut instantiations = InstantiationMap::default();
                let key = self.get_conditional_type_key(
                    &outer_type_parameters,
                    None,  /*alias*/
                    false, /*forConstraint*/
                );
                instantiations.insert(key, resolved);
                root.borrow_mut().instantiations = Some(instantiations);
            }
        }
        self.type_node_links.get(node).resolved_type
    }

    // Go: checker/checker.go:24765 getConditionalType
    pub fn get_conditional_type(
        &mut self,
        root: Rc<RefCell<ConditionalRoot>>,
        mapper: MapperId,
        for_constraint: bool,
        alias: Option<Rc<TypeAlias>>,
    ) -> TypeId {
        let mut root = root;
        let mut mapper = mapper;
        let mut alias = alias;
        let mut result: TypeId;
        let mut extra_types: Vec<TypeId> = Vec::new();
        let mut tail_count = 0;
        // We loop here for an immediately nested conditional type in the false position, effectively treating
        // types of the form 'A extends B ? X : C extends D ? Y : E extends F ? Z : ...' as a single construct for
        // purposes of resolution. We also loop here when resolution of a conditional type ends in resolution of
        // another (or, through recursion, possibly the same) conditional type. In the potentially tail-recursive
        // cases we increment the tail recursion counter and stop after 1000 iterations.
        loop {
            if tail_count == 1000 {
                let current_node = self.current_node;
                self.error(
                    current_node,
                    diag::Type_instantiation_is_excessively_deep_and_possibly_infinite,
                    args![],
                );
                return self.error_type;
            }
            let (root_node, root_check_type, root_extends_type, root_infer_type_parameters) = {
                let r = root.borrow();
                (
                    r.node,
                    r.check_type,
                    r.extends_type,
                    r.infer_type_parameters.clone(),
                )
            };
            let actual_check_type = self.get_actual_type_variable(root_check_type);
            let check_type = self.instantiate_type(actual_check_type, mapper);
            let extends_type = self.instantiate_type(root_extends_type, mapper);
            if check_type == self.error_type || extends_type == self.error_type {
                return self.error_type;
            }
            if check_type == self.wildcard_type || extends_type == self.wildcard_type {
                return self.wildcard_type;
            }
            let check_type_node = skip_type_parentheses(root_node.check_type());
            let extends_type_node = skip_type_parentheses(root_node.extends_type());
            // When the check and extends types are simple tuple types of the same arity, we defer resolution of the
            // conditional type when any tuple elements are generic. This is such that non-distributable conditional
            // types can be written `[X] extends [Y] ? ...` and be deferred similarly to `X extends Y ? ...`.
            let check_tuples = self.is_simple_tuple_type(check_type_node)
                && self.is_simple_tuple_type(extends_type_node)
                && check_type_node.elements().len() == extends_type_node.elements().len();
            let check_type_deferred = self.is_deferred_type(check_type, check_tuples);
            let mut combined_mapper = MapperId::NIL;
            if !root_infer_type_parameters.is_empty() {
                // When we're looking at making an inference for an infer type, when we get its constraint, it'll automagically be
                // instantiated with the context, so it doesn't need the mapper for the inference context - however the constraint
                // may refer to another _root_, _uncloned_ `infer` type parameter [1], or to something mapped by `mapper` [2].
                // [1] Eg, if we have `Foo<T, U extends T>` and `Foo<number, infer B>` - `B` is constrained to `T`, which, in turn, has been instantiated
                // as `number`
                // Conversely, if we have `Foo<infer A, infer B>`, `B` is still constrained to `T` and `T` is instantiated as `A`
                // [2] Eg, if we have `Foo<T, U extends T>` and `Foo<Q, infer B>` where `Q` is mapped by `mapper` into `number` - `B` is constrained to `T`
                // which is in turn instantiated as `Q`, which is in turn instantiated as `number`.
                // So we need to:
                //    * combine `context.nonFixingMapper` with `mapper` so their constraints can be instantiated in the context of `mapper` (otherwise they'd only get inference context information)
                //    * incorporate all of the component mappers into the combined mapper for the true and false members
                // This means we have two mappers that need applying:
                //    * The original `mapper` used to create this conditional
                //    * The mapper that maps the infer type parameter to its inference result (`context.mapper`)
                let context = self.new_inference_context(
                    &root_infer_type_parameters,
                    SignatureId::NIL, /*signature*/
                    InferenceFlags::NONE,
                    None,
                );
                if mapper.is_some() {
                    let non_fixing_mapper = self.inference_context(context).non_fixing_mapper;
                    let combined = self.combine_type_mappers(non_fixing_mapper, mapper);
                    self.inference_context_mut(context).non_fixing_mapper = combined;
                }
                if !check_type_deferred {
                    // We don't want inferences from constraints as they may cause us to eagerly resolve the
                    // conditional type instead of deferring resolution. Also, we always want strict function
                    // types rules (i.e. proper contravariance) for inferences.
                    self.infer_types(
                        context,
                        check_type,
                        extends_type,
                        InferencePriority::NO_CONSTRAINTS | InferencePriority::ALWAYS_STRICT,
                        false,
                    );
                }
                // It's possible for 'infer T' type parameters to be given uninstantiated constraints when the
                // those type parameters are used in type references (see getInferredTypeParameterConstraint). For
                // that reason we need context.mapper to be first in the combined mapper. See #42636 for examples.
                let context_mapper = self.inference_context(context).mapper;
                if mapper.is_some() {
                    combined_mapper = self.combine_type_mappers(context_mapper, mapper);
                } else {
                    combined_mapper = context_mapper;
                }
            }
            // Instantiate the extends type including inferences for 'infer T' type parameters
            let inferred_extends_type = if combined_mapper.is_some() {
                self.instantiate_type(root_extends_type, combined_mapper)
            } else {
                extends_type
            };
            // We attempt to resolve the conditional type only when the check and extends types are non-generic
            if !check_type_deferred && !self.is_deferred_type(inferred_extends_type, check_tuples) {
                // Return falseType for a definitely false extends check. We check an instantiations of the two
                // types with type parameters mapped to the wildcard type, the most permissive instantiations
                // possible (the wildcard type is assignable to and from all types). If those are not related,
                // then no instantiations will be and we can just return the false branch type.
                let definitely_false = !self
                    .ty(inferred_extends_type)
                    .flags
                    .intersects(TypeFlags::ANY_OR_UNKNOWN)
                    && (self.ty(check_type).flags.intersects(TypeFlags::ANY) || {
                        let permissive_check = self.get_permissive_instantiation(check_type);
                        let permissive_extends =
                            self.get_permissive_instantiation(inferred_extends_type);
                        !self.is_type_assignable_to(permissive_check, permissive_extends)
                    });
                if definitely_false {
                    // Return union of trueType and falseType for 'any' since it matches anything. Furthermore, for a
                    // distributive conditional type applied to the constraint of a type variable, include trueType if
                    // there are possible values of the check type that are also possible values of the extends type.
                    // We use a reverse assignability check as it is less expensive than the comparable relationship
                    // and avoids false positives of a non-empty intersection check.
                    let include_true = self.ty(check_type).flags.intersects(TypeFlags::ANY)
                        || for_constraint
                            && !self
                                .ty(inferred_extends_type)
                                .flags
                                .intersects(TypeFlags::NEVER)
                            && {
                                let permissive_extends =
                                    self.get_permissive_instantiation(inferred_extends_type);
                                self.some_type(
                                    permissive_extends,
                                    &mut |c: &mut Checker, t: TypeId| {
                                        let permissive_check =
                                            c.get_permissive_instantiation(check_type);
                                        c.is_type_assignable_to(t, permissive_check)
                                    },
                                )
                            };
                    if include_true {
                        let true_type = self.get_type_from_type_node(root_node.true_type());
                        let true_mapper = if combined_mapper.is_some() {
                            combined_mapper
                        } else {
                            mapper
                        };
                        let instantiated = self.instantiate_type(true_type, true_mapper);
                        extra_types.push(instantiated);
                    }
                    // If falseType is an immediately nested conditional type that isn't distributive or has an
                    // identical checkType, switch to that type and loop.
                    let false_type = self.get_type_from_type_node(root_node.false_type());
                    if self.ty(false_type).flags.intersects(TypeFlags::CONDITIONAL) {
                        let new_root = self.ty(false_type).as_conditional_type().root.clone();
                        let (new_root_node, new_root_is_distributive, new_root_check_type) = {
                            let r = new_root.borrow();
                            (r.node, r.is_distributive, r.check_type)
                        };
                        if new_root_node.parent() == root_node
                            && (!new_root_is_distributive || new_root_check_type == root_check_type)
                        {
                            root = new_root;
                            continue;
                        }
                        let (new_root, new_root_mapper) =
                            self.get_tail_recursion_root(false_type, mapper);
                        if let Some(new_root) = new_root {
                            let has_alias = new_root.borrow().alias.is_some();
                            root = new_root;
                            mapper = new_root_mapper;
                            alias = None;
                            if has_alias {
                                tail_count += 1;
                            }
                            continue;
                        }
                    }
                    result = self.instantiate_type(false_type, mapper);
                    break;
                }
                // Return trueType for a definitely true extends check. We check instantiations of the two
                // types with type parameters mapped to their restrictive form, i.e. a form of the type parameter
                // that has no constraint. This ensures that, for example, the type
                //   type Foo<T extends { x: any }> = T extends { x: string } ? string : number
                // doesn't immediately resolve to 'string' instead of being deferred.
                let definitely_true = self
                    .ty(inferred_extends_type)
                    .flags
                    .intersects(TypeFlags::ANY_OR_UNKNOWN)
                    || {
                        let restrictive_check = self.get_restrictive_instantiation(check_type);
                        let restrictive_extends =
                            self.get_restrictive_instantiation(inferred_extends_type);
                        self.is_type_assignable_to(restrictive_check, restrictive_extends)
                    };
                if definitely_true {
                    let true_type = self.get_type_from_type_node(root_node.true_type());
                    let true_mapper = if combined_mapper.is_some() {
                        combined_mapper
                    } else {
                        mapper
                    };
                    let (new_root, new_root_mapper) =
                        self.get_tail_recursion_root(true_type, true_mapper);
                    if let Some(new_root) = new_root {
                        let has_alias = new_root.borrow().alias.is_some();
                        root = new_root;
                        mapper = new_root_mapper;
                        alias = None;
                        if has_alias {
                            tail_count += 1;
                        }
                        continue;
                    }
                    result = self.instantiate_type(true_type, true_mapper);
                    break;
                }
            }
            // Return a deferred type for a check that is neither definitely true nor definitely false
            result = self.new_conditional_type(root.clone(), mapper, combined_mapper);
            if alias.is_some() {
                self.ty_mut(result).alias = alias;
            } else {
                let root_alias = root.borrow().alias.clone();
                let instantiated_alias = self.instantiate_type_alias(root_alias, mapper);
                self.ty_mut(result).alias = instantiated_alias;
            }
            break;
        }
        if !extra_types.is_empty() {
            extra_types.push(result);
            return self.get_union_type(&extra_types);
        }
        result
    }

    // We tail-recurse for generic conditional types that (a) have not already been evaluated and cached, and
    // (b) are non distributive, have a check type that is unaffected by instantiation, or have a non-union check
    // type. Note that recursion is possible only through aliased conditional types, so we only increment the tail
    // recursion counter for those.
    // PORT: Go returns `(nil, nil)` for no root; here `(None, MapperId::NIL)`.
    // Go: checker/checker.go:24915 getTailRecursionRoot
    pub fn get_tail_recursion_root(
        &mut self,
        new_type: TypeId,
        new_mapper: MapperId,
    ) -> (Option<Rc<RefCell<ConditionalRoot>>>, MapperId) {
        if self.ty(new_type).flags.intersects(TypeFlags::CONDITIONAL) && new_mapper.is_some() {
            let new_root = self.ty(new_type).as_conditional_type().root.clone();
            // PORT: the shared list clone copies no elements, and the new
            // mapper keeps it as its sources.
            let (outer_type_parameters, is_distributive, root_check_type) = {
                let r = new_root.borrow();
                (
                    r.outer_type_parameters.clone(),
                    r.is_distributive,
                    r.check_type,
                )
            };
            if !outer_type_parameters.is_empty() {
                let conditional_mapper = self.ty(new_type).as_conditional_type().mapper;
                let type_param_mapper = self.combine_type_mappers(conditional_mapper, new_mapper);
                let mut type_arguments: Vec<TypeId> =
                    Vec::with_capacity(outer_type_parameters.len());
                for &t in &outer_type_parameters {
                    type_arguments.push(self.mapper_map(type_param_mapper, t));
                }
                let new_root_mapper =
                    self.new_type_mapper_shared(outer_type_parameters, type_arguments.into());
                let mut new_check_type = TypeId::NIL;
                if is_distributive {
                    new_check_type = self.get_mapped_type(root_check_type, new_root_mapper);
                }
                if new_check_type.is_nil()
                    || new_check_type == root_check_type
                    || !self
                        .ty(new_check_type)
                        .flags
                        .intersects(TypeFlags::UNION | TypeFlags::NEVER)
                {
                    return (Some(new_root), new_root_mapper);
                }
            }
        }
        (None, MapperId::NIL)
    }

    // Go: checker/checker.go:24934 isSimpleTupleType
    pub fn is_simple_tuple_type(&self, node: Node) -> bool {
        is_tuple_type_node(node)
            && !node.elements().is_empty()
            && !node.elements().iter().any(|e| {
                is_optional_type_node(e)
                    || is_rest_type_node(e)
                    || is_named_tuple_member(e)
                        && (e.question_token().is_some() || e.dot_dot_dot_token().is_some())
            })
    }

    // Go: checker/checker.go:24940 isDeferredType
    pub fn is_deferred_type(&mut self, t: TypeId, check_tuples: bool) -> bool {
        if self.is_generic_type(t) {
            return true;
        }
        if check_tuples && self.is_tuple_type(t) {
            let element_types: Vec<TypeId> = self.get_element_types(t).to_vec();
            for e in element_types {
                if self.is_generic_type(e) {
                    return true;
                }
            }
        }
        false
    }

    // Go: checker/checker.go:24944 getPermissiveInstantiation
    pub fn get_permissive_instantiation(&mut self, t: TypeId) -> TypeId {
        if self
            .ty(t)
            .flags
            .intersects(TypeFlags::PRIMITIVE | TypeFlags::ANY_OR_UNKNOWN | TypeFlags::NEVER)
        {
            return t;
        }
        let key = CachedTypeKey {
            kind: CachedTypeKind::PERMISSIVE_INSTANTIATION,
            type_id: t,
        };
        let cached = self.cached_types.get(&key).copied().unwrap_or_default();
        if cached.is_some() {
            return cached;
        }
        let permissive_mapper = self.permissive_mapper;
        let result = self.instantiate_type(t, permissive_mapper);
        self.cached_types.insert(key, result);
        result
    }

    // Go: checker/checker.go:24957 getRestrictiveInstantiation
    pub fn get_restrictive_instantiation(&mut self, t: TypeId) -> TypeId {
        if self
            .ty(t)
            .flags
            .intersects(TypeFlags::PRIMITIVE | TypeFlags::ANY_OR_UNKNOWN | TypeFlags::NEVER)
        {
            return t;
        }
        let key = CachedTypeKey {
            kind: CachedTypeKind::RESTRICTIVE_INSTANTIATION,
            type_id: t,
        };
        let cached = self.cached_types.get(&key).copied().unwrap_or_default();
        if cached.is_some() {
            return cached;
        }
        let restrictive_mapper = self.restrictive_mapper;
        let result = self.instantiate_type(t, restrictive_mapper);
        self.cached_types.insert(key, result);
        // We set the following so we don't attempt to set the restrictive instance of a restrictive instance
        // which is redundant - we'll produce new type identities, but all type params have already been mapped.
        // This also gives us a way to detect restrictive instances upon comparisons and _disable_ the "distributeive constraint"
        // assignability check for them, which is distinctly unsafe, as once you have a restrctive instance, all the type parameters
        // are constrained to `unknown` and produce tons of false positives/negatives!
        let result_id = result;
        self.cached_types.insert(
            CachedTypeKey {
                kind: CachedTypeKind::RESTRICTIVE_INSTANTIATION,
                type_id: result_id,
            },
            result,
        );
        result
    }

    // Go: checker/checker.go:24976 getRestrictiveTypeParameter
    pub fn get_restrictive_type_parameter(&mut self, t: TypeId) -> TypeId {
        let constraint = self.ty(t).as_type_parameter().constraint;
        if constraint.is_nil() && self.get_constraint_declaration(t).is_nil()
            || constraint == self.no_constraint_type
        {
            return t;
        }
        let key = CachedTypeKey {
            kind: CachedTypeKind::RESTRICTIVE_TYPE_PARAMETER,
            type_id: t,
        };
        let cached = self.cached_types.get(&key).copied().unwrap_or_default();
        if cached.is_some() {
            return cached;
        }
        let symbol = self.ty(t).symbol;
        let result = self.new_type_parameter(symbol);
        let no_constraint_type = self.no_constraint_type;
        self.ty_mut(result).as_type_parameter_mut().constraint = no_constraint_type;
        self.cached_types.insert(key, result);
        result
    }

    // Go: checker/checker.go:24990 restrictiveMapperWorker
    pub fn restrictive_mapper_worker(&mut self, t: TypeId) -> TypeId {
        if self.ty(t).flags.intersects(TypeFlags::TYPE_PARAMETER) {
            return self.get_restrictive_type_parameter(t);
        }
        t
    }

    // Go: checker/checker.go:24997 permissiveMapperWorker
    pub fn permissive_mapper_worker(&mut self, t: TypeId) -> TypeId {
        if self.ty(t).flags.intersects(TypeFlags::TYPE_PARAMETER) {
            return self.wildcard_type;
        }
        t
    }

    // Go: checker/checker.go:25004 getTrueTypeFromConditionalType
    pub fn get_true_type_from_conditional_type(&mut self, t: TypeId) -> TypeId {
        if self.ty(t).as_conditional_type().resolved_true_type.is_nil() {
            let (true_type_node, mapper) = {
                let d = self.ty(t).as_conditional_type();
                (d.root.borrow().node.true_type(), d.mapper)
            };
            let true_type = self.get_type_from_type_node(true_type_node);
            let resolved = self.instantiate_type(true_type, mapper);
            self.ty_mut(t).as_conditional_type_mut().resolved_true_type = resolved;
        }
        self.ty(t).as_conditional_type().resolved_true_type
    }

    // Go: checker/checker.go:25012 getFalseTypeFromConditionalType
    pub fn get_false_type_from_conditional_type(&mut self, t: TypeId) -> TypeId {
        if self
            .ty(t)
            .as_conditional_type()
            .resolved_false_type
            .is_nil()
        {
            let (false_type_node, mapper) = {
                let d = self.ty(t).as_conditional_type();
                (d.root.borrow().node.false_type(), d.mapper)
            };
            let false_type = self.get_type_from_type_node(false_type_node);
            let resolved = self.instantiate_type(false_type, mapper);
            self.ty_mut(t).as_conditional_type_mut().resolved_false_type = resolved;
        }
        self.ty(t).as_conditional_type().resolved_false_type
    }

    // Go: checker/checker.go:25020 getInferredTrueTypeFromConditionalType
    pub fn get_inferred_true_type_from_conditional_type(&mut self, t: TypeId) -> TypeId {
        if self
            .ty(t)
            .as_conditional_type()
            .resolved_inferred_true_type
            .is_nil()
        {
            let (true_type_node, combined_mapper) = {
                let d = self.ty(t).as_conditional_type();
                (d.root.borrow().node.true_type(), d.combined_mapper)
            };
            let resolved = if combined_mapper.is_some() {
                let true_type = self.get_type_from_type_node(true_type_node);
                self.instantiate_type(true_type, combined_mapper)
            } else {
                self.get_true_type_from_conditional_type(t)
            };
            self.ty_mut(t)
                .as_conditional_type_mut()
                .resolved_inferred_true_type = resolved;
        }
        self.ty(t).as_conditional_type().resolved_inferred_true_type
    }

    // Go: checker/checker.go:25032 getTypeFromInferTypeNode
    pub fn get_type_from_infer_type_node(&mut self, node: Node) -> TypeId {
        if self.type_node_links.get(node).resolved_type.is_nil() {
            let symbol = self.get_symbol_of_declaration(node.type_parameter());
            let resolved = self.get_declared_type_of_type_parameter(symbol);
            self.type_node_links.get(node).resolved_type = resolved;
        }
        self.type_node_links.get(node).resolved_type
    }

    // Go: checker/checker.go:25040 getTypeFromImportTypeNode
    pub fn get_type_from_import_type_node(&mut self, node: Node) -> TypeId {
        if self.type_node_links.get(node).resolved_type.is_nil() {
            let n = node;
            if !is_literal_import_type_node(node) {
                self.error(n.argument(), diag::String_literal_expected, args![]);
                let unknown_symbol = self.unknown_symbol;
                self.symbol_node_links.get(node).resolved_symbol = unknown_symbol;
                let error_type = self.error_type;
                self.type_node_links.get(node).resolved_type = error_type;
                return error_type;
            }
            let target_meaning = if n.is_type_of() {
                SymbolFlags::VALUE
            } else {
                SymbolFlags::TYPE
            };
            // Go comment: Future work: support unions/generics/whatever via a deferred import-type
            let import_attributes_type =
                self.get_type_from_import_attributes(get_import_attributes(node));
            let inner_module_symbol = self.resolve_external_module_name(
                node,
                n.argument().literal(),
                false, /*ignoreErrors*/
                import_attributes_type,
            );
            if inner_module_symbol.is_nil() {
                let unknown_symbol = self.unknown_symbol;
                self.symbol_node_links.get(node).resolved_symbol = unknown_symbol;
                let error_type = self.error_type;
                self.type_node_links.get(node).resolved_type = error_type;
                return error_type;
            }
            let module_symbol = self.resolve_external_module_symbol(
                inner_module_symbol,
                false, /*dontResolveAlias*/
            );
            if !node_is_missing(n.qualifier()) {
                let name_chain = self.get_identifier_chain(n.qualifier());
                let mut current_namespace = module_symbol;
                let chain_len = name_chain.len();
                for (i, current) in name_chain.into_iter().enumerate() {
                    let mut meaning = SymbolFlags::NAMESPACE;
                    if i == chain_len - 1 {
                        meaning = target_meaning;
                    }
                    // typeof a.b.c is normally resolved using `checkExpression` which in turn defers to `checkQualifiedName`
                    // That, in turn, ultimately uses `getPropertyOfType` on the type of the symbol, which differs slightly from
                    // the `exports` lookup process that only looks up namespace members which is used for most type references
                    let resolved_namespace = self.resolve_symbol(current_namespace);
                    let merged_resolved_symbol = self.get_merged_symbol(resolved_namespace);
                    let mut symbol_from_variable = SymbolId::NIL;
                    let mut symbol_from_module = SymbolId::NIL;
                    if n.is_type_of() {
                        let t = self.get_type_of_symbol(merged_resolved_symbol);
                        symbol_from_variable = self.get_property_of_type_ex(
                            t,
                            current.text(),
                            false, /*skipObjectFunctionPropertyAugment*/
                            true,  /*includeTypeOnlyMembers*/
                        );
                    } else {
                        let exports = self.get_exports_of_symbol(merged_resolved_symbol);
                        symbol_from_module = self.get_symbol(exports, current.text(), meaning);
                        if symbol_from_module.is_nil() {
                            // a CommonJS module might have typedefs exported alongside an export=
                            // !!!
                            let immediate_module_symbol = self.resolve_external_module_symbol(
                                inner_module_symbol,
                                true, /*dontResolveAlias*/
                            );
                            if immediate_module_symbol.is_some()
                                && self
                                    .sym(immediate_module_symbol)
                                    .declarations
                                    .iter()
                                    .any(|&d| {
                                        get_assignment_declaration_kind(d)
                                            == JSDeclarationKind::MODULE_EXPORTS
                                    })
                            {
                                let parent = self.sym(immediate_module_symbol).parent;
                                let parent_exports = self.get_exports_of_symbol(parent);
                                symbol_from_module =
                                    self.get_symbol(parent_exports, current.text(), meaning);
                            }
                        }
                    }
                    let next = if symbol_from_module.is_some() {
                        symbol_from_module
                    } else {
                        symbol_from_variable
                    };
                    if next.is_nil() {
                        let namespace_name =
                            self.get_fully_qualified_name(current_namespace, Node::NIL);
                        self.error(
                            current,
                            diag::Namespace_0_has_no_exported_member_1,
                            args![namespace_name, declaration_name_to_string(current)],
                        );
                        let error_type = self.error_type;
                        self.type_node_links.get(node).resolved_type = error_type;
                        return error_type;
                    }
                    self.symbol_node_links.get(current).resolved_symbol = next;
                    self.symbol_node_links.get(current.parent()).resolved_symbol = next;
                    current_namespace = next;
                }
                let resolved =
                    self.resolve_import_symbol_type(node, current_namespace, target_meaning);
                self.type_node_links.get(node).resolved_type = resolved;
            } else if self
                .get_symbol_flags(module_symbol)
                .intersects(target_meaning)
            {
                let resolved = self.resolve_import_symbol_type(node, module_symbol, target_meaning);
                self.type_node_links.get(node).resolved_type = resolved;
            } else {
                let message = if target_meaning == SymbolFlags::VALUE {
                    diag::Module_0_does_not_refer_to_a_value_but_is_used_as_a_value_here
                } else {
                    diag::Module_0_does_not_refer_to_a_type_but_is_used_as_a_type_here_Did_you_mean_typeof_import_0
                };
                self.error(node, message, args![n.argument().literal().text()]);
                let unknown_symbol = self.unknown_symbol;
                self.symbol_node_links.get(node).resolved_symbol = unknown_symbol;
                let error_type = self.error_type;
                self.type_node_links.get(node).resolved_type = error_type;
            }
        }
        self.type_node_links.get(node).resolved_type
    }

    // Go: checker/checker.go:25115 getIdentifierChain
    pub fn get_identifier_chain(&self, node: Node) -> Vec<Node> {
        if is_identifier(node) {
            return vec![node];
        }
        let mut chain = self.get_identifier_chain(node.left());
        chain.push(node.right());
        chain
    }

    // Go: checker/checker.go:25122 resolveImportSymbolType
    pub fn resolve_import_symbol_type(
        &mut self,
        node: Node,
        symbol: SymbolId,
        meaning: SymbolFlags,
    ) -> TypeId {
        let resolved_symbol = self.resolve_symbol(symbol);
        self.symbol_node_links.get(node).resolved_symbol = resolved_symbol;
        if meaning == SymbolFlags::VALUE {
            // intentionally doesn't use resolved symbol so type is cached as expected on the alias
            let t = self.get_type_of_symbol(symbol);
            return self.get_instantiation_expression_type(t, node);
        }
        // getTypeReferenceType doesn't handle aliases - it must get the resolved symbol
        self.get_type_reference_type(node, resolved_symbol)
    }

    // Go: checker/checker.go:25133 createTypeFromGenericGlobalType
    pub fn create_type_from_generic_global_type(
        &mut self,
        generic_global_type: TypeId,
        type_arguments: &[TypeId],
    ) -> TypeId {
        if generic_global_type != self.empty_generic_type {
            return self.create_type_reference(generic_global_type, type_arguments);
        }
        self.empty_object_type
    }

    // Go: checker/checker.go:25140 getGlobalStrictFunctionType
    pub fn get_global_strict_function_type(&mut self, name: &str) -> TypeId {
        if self.strict_bind_call_apply {
            return self.get_global_type(name, 0 /*arity*/, true /*reportErrors*/);
        }
        self.global_function_type
    }

    // Go: checker/checker.go:25147 getGlobalImportMetaExpressionType
    pub fn get_global_import_meta_expression_type(&mut self) -> TypeId {
        if self.deferred_global_import_meta_expression_type.is_nil() {
            // Create a synthetic type `ImportMetaExpression { meta: MetaProperty }`
            let symbol = self.new_symbol(SymbolFlags::NONE, "ImportMetaExpression");
            let import_meta_type = (self.get_global_import_meta_type.clone())(self);
            let meta_property_symbol =
                self.new_symbol_ex(SymbolFlags::PROPERTY, "meta", CheckFlags::READONLY);
            self.sym_mut(meta_property_symbol).parent = symbol;
            self.value_symbol_links
                .get_by_id(&self.symbols, meta_property_symbol)
                .resolved_type = import_meta_type;
            let members = self.create_symbol_table(&[meta_property_symbol]);
            self.sym_mut(symbol).members = members;
            let t = self.new_anonymous_type(symbol, members, &[], &[], &[]);
            self.deferred_global_import_meta_expression_type = t;
        }
        self.deferred_global_import_meta_expression_type
    }

    // Go: checker/checker.go:25162 createIterableType
    pub fn create_iterable_type(&mut self, iterated_type: TypeId) -> TypeId {
        let iterable = (self.get_global_iterable_type_checked.clone())(self);
        let void_type = self.void_type;
        let undefined_type = self.undefined_type;
        self.create_type_from_generic_global_type(
            iterable,
            &[iterated_type, void_type, undefined_type],
        )
    }

    // Go: checker/checker.go:25166 createArrayType
    pub fn create_array_type(&mut self, element_type: TypeId) -> TypeId {
        self.create_array_type_ex(element_type, false /*readonly*/)
    }

    // Go: checker/checker.go:25170 createArrayTypeEx
    pub fn create_array_type_ex(&mut self, element_type: TypeId, readonly: bool) -> TypeId {
        let target = if readonly {
            self.global_readonly_array_type
        } else {
            self.global_array_type
        };
        self.create_type_from_generic_global_type(target, &[element_type])
    }

    // Go: checker/checker.go:25174 getTupleElementFlags
    pub fn get_tuple_element_flags(&self, node: Node) -> ElementFlags {
        match node.kind() {
            SyntaxKind::OptionalType => {
                return ElementFlags::OPTIONAL;
            }
            SyntaxKind::RestType => {
                return if self.get_array_element_type_node(node.type_()).is_some() {
                    ElementFlags::REST
                } else {
                    ElementFlags::VARIADIC
                };
            }
            SyntaxKind::NamedTupleMember => {
                if node.question_token().is_some() {
                    return ElementFlags::OPTIONAL;
                } else if node.dot_dot_dot_token().is_some() {
                    return if self.get_array_element_type_node(node.type_()).is_some() {
                        ElementFlags::REST
                    } else {
                        ElementFlags::VARIADIC
                    };
                }
                return ElementFlags::REQUIRED;
            }
            _ => {}
        }
        ElementFlags::REQUIRED
    }

    // Go: checker/checker.go:25193 getTupleElementInfo
    pub fn get_tuple_element_info(&self, node: Node) -> TupleElementInfo {
        TupleElementInfo {
            flags: self.get_tuple_element_flags(node),
            labeled_declaration: if is_named_tuple_member(node) || is_parameter_declaration(node) {
                node
            } else {
                Node::NIL
            },
        }
    }

    // Go: checker/checker.go:25200 createTupleType
    pub fn create_tuple_type(&mut self, element_types: &[TypeId]) -> TypeId {
        let element_infos: Vec<TupleElementInfo> = element_types
            .iter()
            .map(|_| TupleElementInfo {
                flags: ElementFlags::REQUIRED,
                labeled_declaration: Node::NIL,
            })
            .collect();
        self.create_tuple_type_ex(element_types, &element_infos, false /*readonly*/)
    }

    // Go: checker/checker.go:25205 createTupleTypeEx
    pub fn create_tuple_type_ex(
        &mut self,
        element_types: &[TypeId],
        element_infos: &[TupleElementInfo],
        readonly: bool,
    ) -> TypeId {
        let tuple_target = self.get_tuple_target_type(element_infos, readonly);
        if tuple_target == self.empty_generic_type {
            return self.empty_object_type;
        } else if !element_types.is_empty() {
            return self.create_normalized_type_reference(tuple_target, element_types);
        }
        tuple_target
    }

    // Go: checker/checker.go:25216 getTupleTargetType
    pub fn get_tuple_target_type(
        &mut self,
        element_infos: &[TupleElementInfo],
        readonly: bool,
    ) -> TypeId {
        if element_infos.len() == 1 && element_infos[0].flags.intersects(ElementFlags::REST) {
            // [...X[]] is equivalent to just X[]
            if readonly {
                return self.global_readonly_array_type;
            }
            return self.global_array_type;
        }
        let key = get_tuple_key(element_infos, readonly);
        let mut t = self.tuple_types.get(&key).copied().unwrap_or_default();
        if t.is_nil() {
            t = self.create_tuple_target_type(element_infos, readonly);
            self.tuple_types.insert(key, t);
        }
        t
    }

    // We represent tuple types as type references to synthesized generic interface types created by
    // this function. The types are of the form:
    //
    //	interface Tuple<T0, T1, T2, ...> extends Array<T0 | T1 | T2 | ...> { 0: T0, 1: T1, 2: T2, ... }
    //
    // Note that the generic type created by this function has no symbol associated with it. The same
    // is true for each of the synthesized type parameters.
    // Go: checker/checker.go:25240 createTupleTargetType
    pub fn create_tuple_target_type(
        &mut self,
        element_infos: &[TupleElementInfo],
        readonly: bool,
    ) -> TypeId {
        let arity = element_infos.len();
        let min_length = element_infos
            .iter()
            .filter(|e| {
                e.flags
                    .intersects(ElementFlags::REQUIRED | ElementFlags::VARIADIC)
            })
            .count() as i32;
        let mut type_parameters: Vec<TypeId> = Vec::new();
        let members = self.symbols.new_table();
        let mut combined_flags = ElementFlags::NONE;
        if arity != 0 {
            type_parameters = Vec::with_capacity(arity);
            for i in 0..arity {
                let type_parameter = self.new_type_parameter(SymbolId::NIL);
                type_parameters.push(type_parameter);
                let flags = element_infos[i].flags;
                combined_flags |= flags;
                if !combined_flags.intersects(ElementFlags::VARIABLE) {
                    let symbol_flags = SymbolFlags::PROPERTY
                        | if flags.intersects(ElementFlags::OPTIONAL) {
                            SymbolFlags::OPTIONAL
                        } else {
                            SymbolFlags::NONE
                        };
                    let check_flags = if readonly {
                        CheckFlags::READONLY
                    } else {
                        CheckFlags::NONE
                    };
                    let property = self.new_symbol_ex(symbol_flags, &i.to_string(), check_flags);
                    self.value_symbol_links
                        .get_by_id(&self.symbols, property)
                        .resolved_type = type_parameter;
                    // c.valueSymbolLinks.get(property).tupleLabelDeclaration = elementInfos[i].labeledDeclaration
                    let name = self.sym(property).name.clone();
                    self.symbols.set(members, name, property);
                }
            }
        }
        let fixed_length = self.symbols.len(members) as i32;
        let length_symbol = self.new_symbol_ex(
            SymbolFlags::PROPERTY,
            "length",
            if readonly {
                CheckFlags::READONLY
            } else {
                CheckFlags::NONE
            },
        );
        if combined_flags.intersects(ElementFlags::VARIABLE) {
            let number_type = self.number_type;
            self.value_symbol_links
                .get_by_id(&self.symbols, length_symbol)
                .resolved_type = number_type;
        } else {
            let mut literal_types: Vec<TypeId> = Vec::new();
            for i in min_length..=(arity as i32) {
                literal_types.push(self.get_number_literal_type(Number(i as f64)));
            }
            let union = self.get_union_type(&literal_types);
            self.value_symbol_links
                .get_by_id(&self.symbols, length_symbol)
                .resolved_type = union;
        }
        let length_name = self.sym(length_symbol).name.clone();
        self.symbols.set(members, length_name, length_symbol);
        let t = self.new_object_type(ObjectFlags::TUPLE | ObjectFlags::REFERENCE, SymbolId::NIL);
        let this_type = self.new_type_parameter(SymbolId::NIL);
        {
            let tp = self.ty_mut(this_type).as_type_parameter_mut();
            tp.is_this_type = true;
            tp.constraint = t;
        }
        let mut all_type_parameters = type_parameters;
        all_type_parameters.push(this_type);
        // TypeParameters() is allTypeParameters without the trailing this type.
        let tps: Vec<TypeId> = all_type_parameters[..all_type_parameters.len() - 1].to_vec();
        let key = get_type_list_key(&tps);
        let d = self.ty_mut(t).as_tuple_type_mut();
        d.interface.this_type = this_type;
        d.interface.all_type_parameters = all_type_parameters;
        let mut instantiations = InstantiationMap::default();
        instantiations.insert(key, t);
        d.interface.instantiations = Some(instantiations);
        d.interface.reference.object.target = t;
        d.interface.reference.resolved_type_arguments = tps.into();
        d.interface.declared_members_resolved = true;
        d.interface.declared_members = members;
        d.element_infos = element_infos.to_vec();
        d.min_length = min_length;
        d.fixed_length = fixed_length;
        d.combined_flags = combined_flags;
        d.readonly = readonly;
        t
    }

    // PORT: Go returns nil when `index >= length`; here `TypeId::NIL`.
    // Go: checker/checker.go:25295 getElementTypeOfSliceOfTupleType
    pub fn get_element_type_of_slice_of_tuple_type(
        &mut self,
        t: TypeId,
        index: i32,
        end_skip_count: i32,
        writing: bool,
        no_reductions: bool,
    ) -> TypeId {
        let length = self.get_type_reference_arity(t) - end_skip_count;
        let element_infos: Vec<TupleElementInfo> = self.target_tuple_type(t).element_infos.clone();
        if index < length {
            let type_arguments = self.get_type_arguments(t);
            let mut element_types: Vec<TypeId> = Vec::new();
            for i in index..length {
                let mut e = type_arguments[i as usize];
                if element_infos[i as usize]
                    .flags
                    .intersects(ElementFlags::VARIADIC)
                {
                    let number_type = self.number_type;
                    e = self.get_indexed_access_type(e, number_type);
                }
                element_types.push(e);
            }
            if writing {
                return self.get_intersection_type(&element_types);
            }
            let reduction = if no_reductions {
                UnionReduction::NONE
            } else {
                UnionReduction::LITERAL
            };
            return self.get_union_type_ex(&element_types, reduction, None, TypeId::NIL);
        }
        TypeId::NIL
    }

    // Go: checker/checker.go:25316 getRestTypeOfTupleType
    pub fn get_rest_type_of_tuple_type(&mut self, t: TypeId) -> TypeId {
        let fixed_length = self.target_tuple_type(t).fixed_length;
        self.get_element_type_of_slice_of_tuple_type(t, fixed_length, 0, false, false)
    }

    // Go: checker/checker.go:25320 getTupleElementTypeOutOfStartCount
    pub fn get_tuple_element_type_out_of_start_count(
        &mut self,
        t: TypeId,
        index: Number,
        undefined_like_type: TypeId,
    ) -> TypeId {
        self.map_type(t, &mut |c: &mut Checker, t: TypeId| {
            let rest_type = c.get_rest_type_of_tuple_type(t);
            if rest_type.is_nil() {
                return c.undefined_type;
            }
            if undefined_like_type.is_some()
                && index >= Number(get_total_fixed_element_count(c.target_tuple_type(t)) as f64)
            {
                return c.get_union_type(&[rest_type, undefined_like_type]);
            }
            rest_type
        })
    }

    // Go: checker/checker.go:25333 isGenericType
    pub fn is_generic_type(&mut self, t: TypeId) -> bool {
        !self.get_generic_object_flags(t).is_empty()
    }

    // Go: checker/checker.go:25337 isGenericObjectType
    pub fn is_generic_object_type(&mut self, t: TypeId) -> bool {
        self.get_generic_object_flags(t)
            .intersects(ObjectFlags::IS_GENERIC_OBJECT_TYPE)
    }

    // Go: checker/checker.go:25341 isGenericIndexType
    pub fn is_generic_index_type(&mut self, t: TypeId) -> bool {
        self.get_generic_object_flags(t)
            .intersects(ObjectFlags::IS_GENERIC_INDEX_TYPE)
    }

    // Go: checker/checker.go:25345 getGenericObjectFlags
    pub fn get_generic_object_flags(&mut self, t: TypeId) -> ObjectFlags {
        let mut combined_flags = ObjectFlags::NONE;
        let flags = self.ty(t).flags;
        if flags.intersects(TypeFlags::UNION_OR_INTERSECTION | TypeFlags::SUBSTITUTION) {
            if !self
                .ty(t)
                .object_flags
                .intersects(ObjectFlags::IS_GENERIC_TYPE_COMPUTED)
            {
                if flags.intersects(TypeFlags::UNION_OR_INTERSECTION) {
                    for i in 0..self.ty(t).types().len() {
                        let u = self.type_at(t, i);
                        combined_flags |= self.get_generic_object_flags(u);
                    }
                } else {
                    let (base_type, constraint) = {
                        let d = self.ty(t).as_substitution_type();
                        (d.base_type, d.constraint)
                    };
                    combined_flags = self.get_generic_object_flags(base_type)
                        | self.get_generic_object_flags(constraint);
                }
                self.ty_mut(t).object_flags |=
                    ObjectFlags::IS_GENERIC_TYPE_COMPUTED | combined_flags;
            }
            return self.ty(t).object_flags & ObjectFlags::IS_GENERIC_TYPE;
        }
        if flags.intersects(TypeFlags::INSTANTIABLE_NON_PRIMITIVE)
            || self.is_generic_mapped_type(t)
            || self.is_generic_tuple_type(t)
        {
            combined_flags |= ObjectFlags::IS_GENERIC_OBJECT_TYPE;
        }
        if flags.intersects(TypeFlags::INSTANTIABLE_NON_PRIMITIVE | TypeFlags::INDEX)
            || self.is_generic_string_like_type(t)
        {
            combined_flags |= ObjectFlags::IS_GENERIC_INDEX_TYPE;
        }
        combined_flags
    }

    // Go: checker/checker.go:25369 isGenericTupleType
    pub fn is_generic_tuple_type(&self, t: TypeId) -> bool {
        self.is_tuple_type(t)
            && self
                .target_tuple_type(t)
                .combined_flags
                .intersects(ElementFlags::VARIADIC)
    }

    // Go: checker/checker.go:25373 isGenericMappedType
    pub fn is_generic_mapped_type(&mut self, t: TypeId) -> bool {
        if self.ty(t).object_flags.intersects(ObjectFlags::MAPPED) {
            let constraint = self.get_constraint_type_from_mapped_type(t);
            if self.is_generic_index_type(constraint) {
                return true;
            }
            // A mapped type is generic if the 'as' clause references generic types other than the iteration type.
            // To determine this, we substitute the constraint type (that we now know isn't generic) for the iteration
            // type and check whether the resulting type is generic.
            let name_type = self.get_name_type_from_mapped_type(t);
            if name_type.is_some() {
                let type_parameter = self.get_type_parameter_from_mapped_type(t);
                let mapper = self.new_simple_type_mapper(type_parameter, constraint);
                let instantiated = self.instantiate_type(name_type, mapper);
                if self.is_generic_index_type(instantiated) {
                    return true;
                }
            }
        }
        false
    }

    // A union type which is reducible upon instantiation (meaning some members are removed under certain instantiations)
    // must be kept generic, as that instantiation information needs to flow through the type system. By replacing all
    // type parameters in the union with a special never type that is treated as a literal in `getReducedType`, we can cause
    // the `getReducedType` logic to reduce the resulting type if possible (since only intersections with conflicting
    // literal-typed properties are reducible).
    // Go: checker/checker.go:25397 isGenericReducibleType
    pub fn is_generic_reducible_type(&mut self, t: TypeId) -> bool {
        let flags = self.ty(t).flags;
        if flags.intersects(TypeFlags::UNION)
            && self
                .ty(t)
                .object_flags
                .intersects(ObjectFlags::CONTAINS_INTERSECTIONS)
        {
            for i in 0..self.ty(t).types().len() {
                let u = self.type_at(t, i);
                if self.is_generic_reducible_type(u) {
                    return true;
                }
            }
        }
        flags.intersects(TypeFlags::INTERSECTION) && self.is_reducible_intersection(t)
    }

    // Go: checker/checker.go:25402 isReducibleIntersection
    pub fn is_reducible_intersection(&mut self, t: TypeId) -> bool {
        if self
            .ty(t)
            .as_intersection_type()
            .unique_literal_filled_instantiation
            .is_nil()
        {
            let unique_literal_mapper = self.unique_literal_mapper;
            let instantiated = self.instantiate_type(t, unique_literal_mapper);
            self.ty_mut(t)
                .as_intersection_type_mut()
                .unique_literal_filled_instantiation = instantiated;
        }
        let filled = self
            .ty(t)
            .as_intersection_type()
            .unique_literal_filled_instantiation;
        self.get_reduced_type(filled) != filled
    }

    // Go: checker/checker.go:25410 getUniqueLiteralTypeForTypeParameter
    pub fn get_unique_literal_type_for_type_parameter(&mut self, t: TypeId) -> TypeId {
        if self.ty(t).flags.intersects(TypeFlags::TYPE_PARAMETER) {
            return self.unique_literal_type;
        }
        t
    }

    // Go: checker/checker.go:25417 getConditionalFlowTypeOfType
    pub fn get_conditional_flow_type_of_type(&mut self, t: TypeId, node: Node) -> TypeId {
        // PORT: from the SourceFile or its EndOfFile token, Go's walk reaches
        // the nil parent of the SourceFile, and `IsParameterDeclaration(nil)`
        // dereferences it. The shortcuts below skip that walk.
        if matches!(node.kind(), SyntaxKind::SourceFile | SyntaxKind::EndOfFile) {
            crate::core::go_nil_dereference();
        }
        if let Some(facts) = frozen_node_store_facts(node) {
            // PERF: hono P7-C7. The loop adds a constraint only under a
            // ConditionalType or MappedType parent, and its other tests have no
            // side effects. From a node of an alias-free store with local
            // parents, the parent walk stays in that store, so a store with
            // neither kind gives `t`.
            if facts.alias_free && facts.parents_local && !facts.has_flow_constraint_kind {
                return t;
            }
            // PERF: the walk reads only the parents and child fields of
            // frozen nodes, and with local parents it stays in this frozen
            // store, so its steps are the same on every call. A node with
            // type node links keeps them (it never gets links here); each
            // call then runs only the tests on `t` and the same calls in the
            // same order.
            if facts.parents_local
                && let Some(links) = self.type_node_links.try_get(node)
            {
                let steps = if links.flow_steps_known {
                    links.flow_steps.clone()
                } else {
                    let mut found: SmallVec<[FlowStep; 4]> = SmallVec::new();
                    for_each_conditional_flow_step(node, |step| found.push(step));
                    let steps: Option<Rc<[FlowStep]>> =
                        (!found.is_empty()).then(|| Rc::from(&found[..]));
                    let links = self.type_node_links.get(node);
                    links.flow_steps_known = true;
                    links.flow_steps = steps.clone();
                    steps
                };
                let Some(steps) = steps else {
                    return t;
                };
                let mut constraints: SmallVec<[TypeId; 4]> = SmallVec::new();
                for &step in steps.iter() {
                    self.add_conditional_flow_constraint(t, step, &mut constraints);
                }
                return self.conditional_flow_type(t, &constraints);
            }
        }
        let mut constraints: SmallVec<[TypeId; 4]> = SmallVec::new();
        for_each_conditional_flow_step(node, |step| {
            self.add_conditional_flow_constraint(t, step, &mut constraints);
        });
        self.conditional_flow_type(t, &constraints)
    }

    /// The part of the `getConditionalFlowTypeOfType` loop body that reads
    /// `t`, at one step of the walk (`for_each_conditional_flow_step`).
    fn add_conditional_flow_constraint(
        &mut self,
        t: TypeId,
        step: FlowStep,
        constraints: &mut SmallVec<[TypeId; 4]>,
    ) {
        match step {
            FlowStep::Conditional { parent, covariant } => {
                // Always substitute on type parameters, regardless of variance, since even
                // in contravariant positions, they may rely on substituted constraints to be valid
                if covariant || self.ty(t).flags.intersects(TypeFlags::TYPE_VARIABLE) {
                    let constraint =
                        self.get_implied_constraint(t, parent.check_type(), parent.extends_type());
                    if constraint.is_some() {
                        constraints.push(constraint);
                    }
                }
            }
            FlowStep::Mapped { parent } => {
                if !self.ty(t).flags.intersects(TypeFlags::TYPE_PARAMETER) {
                    return;
                }
                let mapped_type = self.get_type_from_type_node(parent);
                let mapped_type_parameter = self.get_type_parameter_from_mapped_type(mapped_type);
                if mapped_type_parameter == self.get_actual_type_variable(t) {
                    let type_parameter = self.get_homomorphic_type_variable(mapped_type);
                    if type_parameter.is_some() {
                        let constraint = self.get_constraint_of_type_parameter(type_parameter);
                        if constraint.is_some()
                            && self.every_type(constraint, &mut |c: &mut Checker, t: TypeId| {
                                c.is_array_or_tuple_type(t)
                            })
                        {
                            let number_type = self.number_type;
                            let numeric_string_type = self.numeric_string_type;
                            constraints
                                .push(self.get_union_type(&[number_type, numeric_string_type]));
                        }
                    }
                }
            }
        }
    }

    /// The end of `getConditionalFlowTypeOfType`: `t`, or its substitution
    /// type when the walk found constraints.
    fn conditional_flow_type(&mut self, t: TypeId, constraints: &[TypeId]) -> TypeId {
        if !constraints.is_empty() {
            let intersection = self.get_intersection_type(constraints);
            return self.get_substitution_type(t, intersection);
        }
        t
    }

    // PORT: Go returns nil for no implied constraint; here `TypeId::NIL`.
    // Go: checker/checker.go:25454 getImpliedConstraint
    pub fn get_implied_constraint(
        &mut self,
        t: TypeId,
        check_node: Node,
        extends_node: Node,
    ) -> TypeId {
        if is_unary_tuple_type_node(check_node) && is_unary_tuple_type_node(extends_node) {
            return self.get_implied_constraint(
                t,
                check_node.elements().get(0),
                extends_node.elements().get(0),
            );
        }
        let check_type = self.get_type_from_type_node(check_node);
        if self.get_actual_type_variable(check_type) == self.get_actual_type_variable(t) {
            return self.get_type_from_type_node(extends_node);
        }
        TypeId::NIL
    }
}

// Go: checker/checker.go:25464 isUnaryTupleTypeNode
pub fn is_unary_tuple_type_node(node: Node) -> bool {
    is_tuple_type_node(node) && node.elements().len() == 1
}

/// The parent walk of Go `getConditionalFlowTypeOfType`, without the tests
/// on the type: calls `f` with each step where a branch of the loop body
/// can fire, in walk order. It reads only the AST. The loop body at other
/// steps adds nothing for any type, and a conditional step never takes the
/// mapped branch (its parent is not a MappedType).
fn for_each_conditional_flow_step(node: Node, mut f: impl FnMut(FlowStep)) {
    let mut covariant = true;
    let mut node = node;
    while node.is_some() && !is_statement(node) && node.kind() != SyntaxKind::JsDoc {
        let parent = node.parent();
        // only consider variance flipped by parameter locations - `keyof` types would usually be considered variance inverting, but
        // often get used in indexed accesses where they behave sortof invariantly, but our checking is lax
        if is_parameter_declaration(parent) {
            covariant = !covariant;
        }
        if is_conditional_type_node(parent) && node == parent.true_type() {
            f(FlowStep::Conditional { parent, covariant });
        } else if is_mapped_type_node(parent)
            && parent.name_type().is_nil()
            && node == parent.type_()
        {
            f(FlowStep::Mapped { parent });
        }
        node = parent;
    }
}

impl Checker {
    // PERF: always inlined. A `data` built before the call is still copied
    // into the slot with a 168-byte `memcpy`, so the hot constructors call
    // `new_type_with` and build their data in its closure.
    // Go: checker/checker.go:25468 newType
    #[inline(always)]
    pub fn new_type(
        &mut self,
        flags: TypeFlags,
        object_flags: ObjectFlags,
        data: TypeData,
    ) -> TypeId {
        self.new_type_with(flags, object_flags, move || data)
    }

    /// `new_type` with the data that `make_data` returns. `make_data` runs
    /// after the arena slot is ready (`ChunkedArena::push_with`), so a caller
    /// that builds a large `TypeData` in it has no stack copy.
    ///
    /// PERF: `make_data` must not call out of line (no allocation). A call
    /// keeps the grow path of `Vec::push`, and the whole `Type` is then built
    /// on the stack and copied with `memcpy`. Callers allocate boxed or
    /// arena data first and only move it in the closure.
    // PORT: Go sets `t.checker = c` and records the type with the tracer; the
    // checker back pointer and tracer are out of scope. Go `t.id = TypeId(c.TypeCount)`
    // equals the arena index, so the new entry is pushed at that index.
    #[inline(always)]
    pub fn new_type_with(
        &mut self,
        flags: TypeFlags,
        object_flags: ObjectFlags,
        make_data: impl FnOnce() -> TypeData,
    ) -> TypeId {
        self.type_count += 1;
        let id = TypeId(self.types.len() as u32);
        debug_assert_eq!(id.0, self.type_count);
        let object_flags = object_flags.without(
            ObjectFlags::COULD_CONTAIN_TYPE_VARIABLES_COMPUTED
                | ObjectFlags::COULD_CONTAIN_TYPE_VARIABLES
                | ObjectFlags::MEMBERS_RESOLVED,
        );
        self.types.push_with(move || Type {
            flags,
            object_flags,
            id,
            symbol: SymbolId::NIL,
            alias: None,
            data: make_data(),
        });
        if let Some(tracer) = self.tracer {
            tracer.record_type(id);
        }
        id
    }

    // Go: checker/checker.go:25482 newIntrinsicType
    pub fn new_intrinsic_type(&mut self, flags: TypeFlags, intrinsic_name: &str) -> TypeId {
        self.new_intrinsic_type_ex(flags, intrinsic_name, ObjectFlags::NONE)
    }

    // Go: checker/checker.go:25486 newIntrinsicTypeEx
    pub fn new_intrinsic_type_ex(
        &mut self,
        flags: TypeFlags,
        intrinsic_name: &str,
        object_flags: ObjectFlags,
    ) -> TypeId {
        let intrinsic_name = intrinsic_name.to_string();
        self.new_type_with(flags, object_flags, move || {
            TypeData::Intrinsic(IntrinsicType { intrinsic_name })
        })
    }

    // Go: checker/checker.go:25492 createWideningType
    pub fn create_widening_type(&mut self, non_widening_type: TypeId) -> TypeId {
        if self.strict_null_checks {
            return non_widening_type;
        }
        let flags = self.ty(non_widening_type).flags;
        let intrinsic_name = self
            .ty(non_widening_type)
            .as_intrinsic_type()
            .intrinsic_name
            .clone();
        let t = self.new_intrinsic_type(flags, &intrinsic_name);
        self.ty_mut(t).object_flags |= ObjectFlags::CONTAINS_WIDENING_TYPE;
        t
    }

    // Go: checker/checker.go:25501 createUnknownUnionType
    pub fn create_unknown_union_type(&mut self) -> TypeId {
        if self.strict_null_checks {
            let undefined_type = self.undefined_type;
            let null_type = self.null_type;
            let unknown_empty_object_type = self.unknown_empty_object_type;
            return self.get_union_type(&[undefined_type, null_type, unknown_empty_object_type]);
        }
        self.unknown_type
    }

    // Go: checker/checker.go:25508 newLiteralType
    pub fn new_literal_type(
        &mut self,
        flags: TypeFlags,
        value: Option<LiteralValue>,
        regular_type: TypeId,
    ) -> TypeId {
        let t = self.new_type_with(flags, ObjectFlags::NONE, move || {
            TypeData::Literal(LiteralType {
                value,
                fresh_type: TypeId::NIL,
                regular_type: TypeId::NIL,
                property_name: std::cell::OnceCell::new(),
                go_plain: std::cell::OnceCell::new(),
            })
        });
        let regular = if regular_type.is_some() {
            regular_type
        } else {
            t
        };
        self.ty_mut(t).as_literal_type_mut().regular_type = regular;
        t
    }
}
