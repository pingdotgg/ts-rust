//! Port of typescript-go `checker/inference.go` lines 947-1627.
//!
//! PORT: Go `*InferenceInfo` is an index into
//! `inference_contexts[ctx].inferences` (see `PORTING.md`). Go functions that
//! take a standalone `*InferenceInfo` take `(ctx: InferenceContextId,
//! index: usize)`. Go functions that take `[]*InferenceInfo` take the
//! `InferenceContextId` that owns the list.

use crate::prelude::*;
use smallvec::SmallVec;

/// PERF (identmatch1, not in Go): the slot of one member in
/// `infer_from_identical_types` (`normalize_for_matching`). `normalized` is
/// the kept normalized type, NIL until one is kept. `resolves` is the
/// `Checker::empty_anonymous_resolves` count when it was kept: the type is
/// used only while that count is the same.
#[derive(Clone, Copy)]
pub(crate) struct NormalizedSlot {
    normalized: TypeId,
    resolves: u32,
}

impl NormalizedSlot {
    pub(crate) const EMPTY: NormalizedSlot = NormalizedSlot {
        normalized: TypeId::NIL,
        resolves: 0,
    };
}

impl Checker {
    // Infer a suitable input type for a homomorphic mapped type { [P in keyof T]: X }. We construct
    // an object type with the same set of properties as the source type, where the type of each
    // property is computed by inferring from the source property type to X for the type
    // variable T[P] (i.e. we treat the type T[P] as the type variable we're inferring for).
    // Go: checker/inference.go:1015 inferTypeForHomomorphicMappedType
    pub fn infer_type_for_homomorphic_mapped_type(
        &mut self,
        source: TypeId,
        target: TypeId,
        constraint: TypeId,
    ) -> TypeId {
        let key = ReverseMappedTypeKey {
            source_id: source,
            target_id: target,
            constraint_id: constraint,
        };
        if let Some(&cached) = self.reverse_homomorphic_mapped_cache.get(&key) {
            if cached.is_some() {
                return cached;
            }
        }
        let t = self.create_reverse_mapped_type(source, target, constraint);
        self.reverse_homomorphic_mapped_cache.insert(key, t);
        t
    }

    // Go: checker/inference.go:1025 createReverseMappedType
    pub fn create_reverse_mapped_type(
        &mut self,
        source: TypeId,
        target: TypeId,
        constraint: TypeId,
    ) -> TypeId {
        // We consider a source type reverse mappable if it has a string index signature or if
        // it has one or more properties and is of a partially inferable type.
        let string_type = self.string_type;
        if !(self.get_index_info_of_type(source, string_type).is_some()
            || self.get_properties_of_type_count(source) != 0
                && self.is_partially_inferable_type(source))
        {
            return TypeId::NIL;
        }
        // For arrays and tuples we infer new arrays and tuples where the reverse mapping has been
        // applied to the element type(s).
        if self.is_array_type(source) {
            let element = self.type_arguments_of(source)[0];
            let element_type = self.infer_reverse_mapped_type(element, target, constraint);
            if element_type.is_nil() {
                return TypeId::NIL;
            }
            let readonly = self.is_readonly_array_type(source);
            return self.create_array_type_ex(element_type, readonly);
        }
        if self.is_tuple_type(source) {
            let source_element_types = self.get_element_types(source);
            let mut element_types = Vec::with_capacity(source_element_types.len());
            for t in source_element_types {
                element_types.push(self.infer_reverse_mapped_type(t, target, constraint));
            }
            if !element_types.iter().all(|t| t.is_some()) {
                return TypeId::NIL;
            }
            let mut element_infos = self.target_tuple_type(source).element_infos.clone();
            if self
                .get_mapped_type_modifiers(target)
                .intersects(MappedTypeModifiers::INCLUDE_OPTIONAL)
            {
                element_infos = element_infos
                    .into_iter()
                    .map(|info| {
                        if info.flags.intersects(ElementFlags::OPTIONAL) {
                            return TupleElementInfo {
                                flags: ElementFlags::REQUIRED,
                                labeled_declaration: info.labeled_declaration,
                            };
                        }
                        info
                    })
                    .collect();
            }
            let readonly = self.target_tuple_type(source).readonly;
            return self.create_tuple_type_ex(&element_types, &element_infos, readonly);
        }
        // For all other object types we infer a new object type where the reverse mapping has been
        // applied to the type of each property.
        let reversed = self.new_object_type(
            ObjectFlags::REVERSE_MAPPED | ObjectFlags::ANONYMOUS,
            SymbolId::NIL, /*symbol*/
        );
        {
            let r = self.ty_mut(reversed).as_reverse_mapped_type_mut();
            r.source = source;
            r.mapped_type = target;
            r.constraint_type = constraint;
        }
        reversed
    }

    // We consider a type to be partially inferable if it isn't marked non-inferable or if it is
    // an object literal type with at least one property of an inferable type. For example, an object
    // literal { a: 123, b: x => true } is marked non-inferable because it contains a context sensitive
    // arrow function, but is considered partially inferable because property 'a' has an inferable type.
    // Go: checker/inference.go:1071 isPartiallyInferableType
    pub fn is_partially_inferable_type(&mut self, t: TypeId) -> bool {
        if !self
            .ty(t)
            .object_flags
            .intersects(ObjectFlags::NON_INFERRABLE_TYPE)
        {
            return true;
        }
        if self.is_object_literal_type(t) {
            for prop in self.get_properties_of_type(t) {
                let prop_type = self.get_type_of_symbol(prop);
                if self.is_partially_inferable_type(prop_type) {
                    return true;
                }
            }
        }
        if self.is_tuple_type(t) {
            for e in self.get_element_types(t) {
                if self.is_partially_inferable_type(e) {
                    return true;
                }
            }
        }
        false
    }

    // Go: checker/inference.go:1077 inferReverseMappedType
    pub fn infer_reverse_mapped_type(
        &mut self,
        source: TypeId,
        target: TypeId,
        constraint: TypeId,
    ) -> TypeId {
        if source.is_nil() || target.is_nil() || constraint.is_nil() {
            // Go reads `source.id`, `target.id` and `constraint.id`: a
            // reverse mapped symbol of another checker has no links here.
            // Go loads `id` (offset 8) of each: addr=0x8 (inference.go:1078).
            crate::core::go_nil_dereference_at(0x8);
        }
        let key = ReverseMappedTypeKey {
            source_id: source,
            target_id: target,
            constraint_id: constraint,
        };
        if let Some(&cached) = self.reverse_mapped_cache.get(&key) {
            return if cached.is_some() {
                cached
            } else {
                self.unknown_type
            };
        }
        self.reverse_mapped_source_stack.push(source);
        self.reverse_mapped_target_stack.push(target);
        let save_expanding_flags = self.reverse_expanding_flags;
        let source_stack = self.reverse_mapped_source_stack.clone();
        if self.is_deeply_nested_type(source, &source_stack, 2) {
            self.reverse_expanding_flags |= ExpandingFlags::SOURCE;
        }
        let target_stack = self.reverse_mapped_target_stack.clone();
        if self.is_deeply_nested_type(target, &target_stack, 2) {
            self.reverse_expanding_flags |= ExpandingFlags::TARGET;
        }
        let mut t = TypeId::NIL;
        if self.reverse_expanding_flags != ExpandingFlags::BOTH {
            t = self.infer_reverse_mapped_type_worker(source, target, constraint);
        }
        self.reverse_mapped_source_stack.pop();
        self.reverse_mapped_target_stack.pop();
        self.reverse_expanding_flags = save_expanding_flags;
        self.reverse_mapped_cache.insert(key, t);
        t
    }

    // Go: checker/inference.go:1102 inferReverseMappedTypeWorker
    pub fn infer_reverse_mapped_type_worker(
        &mut self,
        source: TypeId,
        target: TypeId,
        constraint: TypeId,
    ) -> TypeId {
        let constraint_target = self.ty(constraint).as_index_type().target;
        let type_parameter_from_mapped = self.get_type_parameter_from_mapped_type(target);
        let type_parameter =
            self.get_indexed_access_type(constraint_target, type_parameter_from_mapped);
        let template_type = self.get_template_type_from_mapped_type(target);
        let inference = new_inference_info(type_parameter);
        // PORT: Go passes the one-element `[]*InferenceInfo{inference}` to
        // `inferTypes`. Rust `infer_types` infers into the inference list of
        // an inference context, so the lone inference lives in a scratch
        // context. `inferTypes` reads only the inference list.
        let inferences = InferenceContextId(self.inference_contexts.len() as u32);
        self.inference_contexts.push(InferenceContext {
            inferences: Box::new([inference]),
            ..InferenceContext::default()
        });
        self.infer_types(
            inferences,
            source,
            template_type,
            InferencePriority::NONE,
            false,
        );
        let inferred = self.get_type_from_inference(inferences, 0);
        let t = if inferred.is_some() {
            inferred
        } else {
            self.unknown_type
        };
        self.get_widened_type(t)
    }

    // Go: checker/inference.go:1110 resolveReverseMappedTypeMembers
    pub fn resolve_reverse_mapped_type_members(&mut self, t: TypeId) {
        let (r_source, r_mapped_type, r_constraint_type) = {
            let r = self.ty(t).as_reverse_mapped_type();
            (r.source, r.mapped_type, r.constraint_type)
        };
        let string_type = self.string_type;
        let index_info = self.get_index_info_of_type(r_source, string_type);
        let modifiers = self.get_mapped_type_modifiers(r_mapped_type);
        let readonly_mask = !modifiers.intersects(MappedTypeModifiers::INCLUDE_READONLY);
        let optional_mask = if modifiers.intersects(MappedTypeModifiers::INCLUDE_OPTIONAL) {
            SymbolFlags::NONE
        } else {
            SymbolFlags::OPTIONAL
        };
        let mut index_infos: Vec<IndexInfoId> = Vec::new();
        if index_info.is_some() {
            let (value_type, is_readonly) = {
                let info = self.index_info(index_info);
                (info.value_type, info.is_readonly)
            };
            let inferred =
                self.infer_reverse_mapped_type(value_type, r_mapped_type, r_constraint_type);
            let inferred = if inferred.is_some() {
                inferred
            } else {
                self.unknown_type
            };
            index_infos = vec![self.new_index_info(
                string_type,
                inferred,
                readonly_mask && is_readonly,
                Node::NIL,
                &[],
            )];
        }
        let members = self.symbols.new_table();
        let limited_constraint = self.get_limited_constraint(t);
        for prop in self.get_properties_of_type(r_source) {
            // In case of a reverse mapped type with an intersection constraint, if we were able to
            // extract the filtering type literals we skip those properties that are not assignable to them,
            // because the extra properties wouldn't get through the application of the mapped type anyway
            if limited_constraint.is_some() {
                let property_name_type = self.get_literal_type_from_property(
                    prop,
                    TypeFlags::STRING_OR_NUMBER_LITERAL_OR_UNIQUE,
                    false,
                );
                if !self.is_type_assignable_to(property_name_type, limited_constraint) {
                    continue;
                }
            }
            let check_flags = CheckFlags::REVERSE_MAPPED
                | if readonly_mask && self.is_readonly_symbol(prop) {
                    CheckFlags::READONLY
                } else {
                    CheckFlags::NONE
                };
            let (prop_flags, prop_name, prop_declarations) = {
                let p = self.sym(prop);
                (p.flags, p.name.clone(), p.declarations.clone())
            };
            let inferred_prop = self.new_symbol_ex(
                SymbolFlags::PROPERTY | (prop_flags & optional_mask),
                &prop_name,
                check_flags,
            );
            self.sym_mut(inferred_prop).declarations = prop_declarations;
            // Go reads the links of `inferredProp` (the left side) first.
            self.value_symbol_links
                .get_by_id(&self.symbols, inferred_prop);
            let name_type = self
                .value_symbol_links
                .get_by_id(&self.symbols, prop)
                .name_type;
            self.value_symbol_links
                .get_by_id(&self.symbols, inferred_prop)
                .name_type = name_type;
            let property_type = self.get_type_of_symbol(prop);
            self.reverse_mapped_symbol_links
                .get(inferred_prop)
                .property_type = property_type;
            let constraint_target = self.ty(r_constraint_type).as_index_type().target;
            let is_simplifiable = self
                .ty(constraint_target)
                .flags
                .intersects(TypeFlags::INDEXED_ACCESS)
                && {
                    let ia = self.ty(constraint_target).as_indexed_access_type();
                    let (object_type, index_type) = (ia.object_type, ia.index_type);
                    self.ty(object_type)
                        .flags
                        .intersects(TypeFlags::TYPE_PARAMETER)
                        && self
                            .ty(index_type)
                            .flags
                            .intersects(TypeFlags::TYPE_PARAMETER)
                };
            if is_simplifiable {
                // A reverse mapping of `{[K in keyof T[K_1]]: T[K_1]}` is the same as that of `{[K in keyof T]: T}`, since all we care about is
                // inferring to the "type parameter" (or indexed access) shared by the constraint and template. So, to reduce the number of
                // type identities produced, we simplify such indexed access occurrences
                let new_type_param = self
                    .ty(constraint_target)
                    .as_indexed_access_type()
                    .object_type;
                let new_mapped_type =
                    self.replace_indexed_access(r_mapped_type, constraint_target, new_type_param);
                self.reverse_mapped_symbol_links
                    .get(inferred_prop)
                    .mapped_type = new_mapped_type;
                let constraint_type = self.get_index_type(new_type_param);
                self.reverse_mapped_symbol_links
                    .get(inferred_prop)
                    .constraint_type = constraint_type;
            } else {
                let links = self.reverse_mapped_symbol_links.get(inferred_prop);
                links.mapped_type = r_mapped_type;
                links.constraint_type = r_constraint_type;
            }
            self.symbols.set(members, prop_name, inferred_prop);
        }
        self.set_structured_type_members(t, members, &[], &[], &index_infos);
    }

    // Go: checker/inference.go:1156 getTypeOfReverseMappedSymbol
    pub fn get_type_of_reverse_mapped_symbol(&mut self, symbol: SymbolId) -> TypeId {
        if self
            .value_symbol_links
            .get_by_id(&self.symbols, symbol)
            .resolved_type
            .is_nil()
        {
            let (property_type, mapped_type, constraint_type) = {
                let reverse_links = self.reverse_mapped_symbol_links.get(symbol);
                (
                    reverse_links.property_type,
                    reverse_links.mapped_type,
                    reverse_links.constraint_type,
                )
            };
            let inferred =
                self.infer_reverse_mapped_type(property_type, mapped_type, constraint_type);
            let resolved = if inferred.is_some() {
                inferred
            } else {
                self.unknown_type
            };
            self.value_symbol_links
                .get_by_id(&self.symbols, symbol)
                .resolved_type = resolved;
        }
        self.value_symbol_links
            .get_by_id(&self.symbols, symbol)
            .resolved_type
    }

    // If the original mapped type had an intersection constraint we extract its components,
    // and we make an attempt to do so even if the intersection has been reduced to a union.
    // This entire process allows us to possibly retrieve the filtering type literals.
    // e.g. { [K in keyof U & ("a" | "b") ] } -> "a" | "b"
    // Go: checker/inference.go:1169 getLimitedConstraint
    pub fn get_limited_constraint(&mut self, t: TypeId) -> TypeId {
        let mapped_type = self.ty(t).as_reverse_mapped_type().mapped_type;
        let constraint = self.get_constraint_type_from_mapped_type(mapped_type);
        let constraint_flags = self.ty(constraint).flags;
        if !(constraint_flags.intersects(TypeFlags::UNION)
            || constraint_flags.intersects(TypeFlags::INTERSECTION))
        {
            return TypeId::NIL;
        }
        let mut origin = constraint;
        if constraint_flags.intersects(TypeFlags::UNION) {
            origin = self.ty(constraint).as_union_type().origin;
        }
        if origin.is_nil() || !self.ty(origin).flags.intersects(TypeFlags::INTERSECTION) {
            return TypeId::NIL;
        }
        let constraint_type = self.ty(t).as_reverse_mapped_type().constraint_type;
        let filtered: Vec<TypeId> = self
            .ty(origin)
            .types()
            .iter()
            .copied()
            .filter(|&t| t != constraint_type)
            .collect();
        let limited_constraint = self.get_intersection_type(&filtered);
        if limited_constraint != self.never_type {
            return limited_constraint;
        }
        TypeId::NIL
    }

    // Go: checker/inference.go:1189 replaceIndexedAccess
    pub fn replace_indexed_access(
        &mut self,
        instantiable: TypeId,
        t: TypeId,
        replacement: TypeId,
    ) -> TypeId {
        // map type.indexType to 0
        // map type.objectType to `[TReplacement]`
        // thus making the indexed access `[TReplacement][0]` or `TReplacement`
        let (index_type, object_type) = {
            let ia = self.ty(t).as_indexed_access_type();
            (ia.index_type, ia.object_type)
        };
        let zero = self.get_number_literal_type(crate::jsnum::Number::new(0.0));
        let tuple = self.create_tuple_type(&[replacement]);
        let mapper = self.new_type_mapper(&[index_type, object_type], &[zero, tuple]);
        self.instantiate_type(instantiable, mapper)
    }

    // Go: checker/inference.go:1196 typesDefinitelyUnrelated
    pub fn types_definitely_unrelated(&mut self, source: TypeId, target: TypeId) -> bool {
        // Two tuple types with incompatible arities are definitely unrelated.
        // Two object types that each have a property that is unmatched in the other are definitely unrelated.
        if self.is_tuple_type(source) && self.is_tuple_type(target) {
            return self.tuple_types_definitely_unrelated(source, target);
        }
        self.get_unmatched_property(
            source, target, false, /*requireOptionalProperties*/
            true,  /*matchDiscriminantProperties*/
        )
        .is_some()
            && self
                .get_unmatched_property(
                    target, source, false, /*requireOptionalProperties*/
                    false, /*matchDiscriminantProperties*/
                )
                .is_some()
    }

    // PORT: Go package function; reads type data, so it is a `Checker` method.
    // Go: checker/inference.go:1206 tupleTypesDefinitelyUnrelated
    pub fn tuple_types_definitely_unrelated(&self, source: TypeId, target: TypeId) -> bool {
        let s = self.target_tuple_type(source);
        let t = self.target_tuple_type(target);
        !t.combined_flags.intersects(ElementFlags::VARIADIC) && t.min_length > s.min_length
            || !t.combined_flags.intersects(ElementFlags::VARIABLE)
                && (s.combined_flags.intersects(ElementFlags::VARIABLE)
                    || t.fixed_length < s.fixed_length)
    }

    // Go: checker/inference.go:1213 isTupleTypeStructureMatching
    pub fn is_tuple_type_structure_matching(&self, t1: TypeId, t2: TypeId) -> bool {
        if self.get_type_reference_arity(t1) != self.get_type_reference_arity(t2) {
            return false;
        }
        let infos1 = &self.target_tuple_type(t1).element_infos;
        let infos2 = &self.target_tuple_type(t2).element_infos;
        for (i, e) in infos1.iter().enumerate() {
            if (e.flags & ElementFlags::VARIABLE) != (infos2[i].flags & ElementFlags::VARIABLE) {
                return false;
            }
        }
        true
    }

    // Go: checker/inference.go:1225 isTypeOrBaseIdenticalTo
    // PORT: perf (identmatch1). Called only by `infer_from_identical_types`, with the slots of the pair (see
    // `is_type_identical_in_matching`).
    pub(crate) fn is_type_or_base_identical_to(
        &mut self,
        s: TypeId,
        t: TypeId,
        slots: (&mut NormalizedSlot, &mut NormalizedSlot),
    ) -> bool {
        if t == self.missing_type {
            return s == t;
        }
        self.is_type_identical_in_matching(s, t, slots)
            || self.ty(t).flags.intersects(TypeFlags::STRING)
                && self.ty(s).flags.intersects(TypeFlags::STRING_LITERAL)
            || self.ty(t).flags.intersects(TypeFlags::NUMBER)
                && self.ty(s).flags.intersects(TypeFlags::NUMBER_LITERAL)
    }

    // Go: checker/relater.go:117 isTypeIdenticalTo for one pair of `inferFromMatchingTypes` (inference.go:381).
    // `slots` are the pair's source and target slots of `infer_from_identical_types`.
    // PORT: perf (identmatch1). This is Go's `isTypeRelatedTo` (relater.go:169) with identityRelation. A pair that
    // Go sends to the normalization of isRelatedToEx (relater.go:2654 getNormalizedType) runs that part in
    // `is_normalized_type_identical_in_matching`, without the relater, and with the slots in place of a repeat
    // normalization.
    // Second copy: this repeats Go's `isTypeRelatedTo` (relater.go:169), whose port is `is_type_related_to_kind`
    // (relater_p1.rs). A pin bump that changes isTypeRelatedTo changes both (UPSTREAM.md, "Pin bump checks").
    pub(crate) fn is_type_identical_in_matching(
        &mut self,
        source: TypeId,
        target: TypeId,
        slots: (&mut NormalizedSlot, &mut NormalizedSlot),
    ) -> bool {
        let mut source = source;
        let mut target = target;
        if self.is_fresh_literal_type(source) {
            source = self.ty(source).as_literal_type().regular_type;
        }
        if self.is_fresh_literal_type(target) {
            target = self.ty(target).as_literal_type().regular_type;
        }
        if source == target {
            return true;
        }
        const MAY_SIMPLIFY: TypeFlags = TypeFlags::UNION_OR_INTERSECTION
            .union(TypeFlags::INDEXED_ACCESS)
            .union(TypeFlags::CONDITIONAL)
            .union(TypeFlags::SUBSTITUTION);
        let source_flags = self.ty(source).flags;
        let target_flags = self.ty(target).flags;
        if (source_flags | target_flags).intersects(MAY_SIMPLIFY) {
            // One side has a MAY_SIMPLIFY flag. An object type has none of them, so the relation cache read below
            // does not apply, and that side is structured or instantiable. So Go calls checkTypeRelatedTo, and
            // isRelatedToEx normalizes both types unless its object to primitive test (relater.go:2640) applies.
            if !(source_flags.intersects(TypeFlags::OBJECT)
                && target_flags.intersects(TypeFlags::PRIMITIVE))
            {
                return self.is_normalized_type_identical_in_matching(source, target, slots);
            }
        } else {
            // We have excluded types that may simplify to other forms, so types must have identical flags
            if source_flags != target_flags {
                return false;
            }
            if source_flags.intersects(TypeFlags::SINGLETON) {
                return true;
            }
        }
        if source_flags.intersects(TypeFlags::OBJECT) && target_flags.intersects(TypeFlags::OBJECT)
        {
            let (id, _) = self.get_relation_key(
                source,
                target,
                IntersectionState::NONE,
                true,  /*isIdentity*/
                false, /*ignoreConstraints*/
            );
            let related = self.identity_relation.borrow().get(id);
            if related != RelationComparisonResult::NONE {
                return related.intersects(RelationComparisonResult::SUCCEEDED);
            }
        }
        if (source_flags | target_flags).intersects(TypeFlags::STRUCTURED_OR_INSTANTIABLE) {
            let relation = self.identity_relation.clone();
            return self.check_type_related_to(
                source,
                target,
                &relation,
                Node::NIL, /*errorNode*/
            );
        }
        false
    }

    // Go: checker/relater.go:358 checkTypeRelatedToEx and :2634 isRelatedToEx with identityRelation and no error
    // node, for a pair of `is_type_identical_in_matching` that Go normalizes. `source` and `target` are the pair
    // after the fresh literal step.
    // PORT: perf (identmatch1). Not inlined, so the entry above stays small for the pairs that end before it.
    // Second copy: this repeats the start of Go's `isRelatedToEx` (relater.go:2634-2665, the object to primitive
    // test is in the entry above), whose port is `is_related_to_ex` (relater_p3.rs). A pin bump that changes that
    // part changes both (UPSTREAM.md, "Pin bump checks").
    #[inline(never)]
    fn is_normalized_type_identical_in_matching(
        &mut self,
        source: TypeId,
        target: TypeId,
        (source_slot, target_slot): (&mut NormalizedSlot, &mut NormalizedSlot),
    ) -> bool {
        // Go: relater.go:369 checkTypeRelatedToEx: it reads the relation size before isRelatedToEx normalizes.
        let relation_size = self.identity_relation.borrow().size();
        // Go: relater.go:2654-2655 getNormalizedType, the source before the target.
        let normalized_source =
            self.normalize_for_matching(source, false /*writing*/, source_slot);
        let normalized_target =
            self.normalize_for_matching(target, true /*writing*/, target_slot);
        // Go: relater.go:2656-2665 isRelatedToEx: these ends set no overflow and no error chain, so Go's
        // checkTypeRelatedToEx only takes and puts back a relater.
        if normalized_source == normalized_target {
            return true;
        }
        let flags = self.ty(normalized_source).flags;
        if flags != self.ty(normalized_target).flags {
            return false;
        }
        if flags.intersects(TypeFlags::SINGLETON) {
            return true;
        }
        self.check_normalized_type_identical_to(
            source,
            target,
            normalized_source,
            normalized_target,
            relation_size,
        )
    }

    // Go: checker/relater.go:2654 getNormalizedType (checker.go:28470 getNormalizedType) of one member of an
    // identity pair of inferFromMatchingTypes, at Go's place in Go's order. `slot` is the member's slot in the
    // current call.
    // PORT: perf (identmatch1). Not in Go. Go normalizes a member again at each pair. A repeat gives the type of
    // the first normalization when each read in it gives the value that it gave the first time. A read of a
    // cached or computed-once value (the reduced type caches, the IsGenericType, CouldContainTypeVariables and
    // IsNeverIntersection flags, cachedTypes, the relation caches, the interned types) does: the first
    // normalization computed that value or read it, and a value is computed again only by a computation that
    // was in progress before this call started, which ends after the call. One read does not:
    // `IsEmptyAnonymousObjectType` (checker.go:27077; for example in getIntersectionType and
    // shouldNormalizeIntersection) reads MembersResolved and does not resolve. So its answer for an anonymous type
    // goes from false to true when the type's members are first resolved and are empty, in a normalization or
    // between two pairs (identity relation, inferFromTypes). `empty_anonymous_resolves` counts these resolves.
    // A resolve can also give a different answer for a short time, in a window that opens and closes in its own
    // frame. In resolveAnonymousTypeMembers: the placeholder members (checker.go:21089,21099 setStructuredTypeMembers),
    // and the module and class path from :21120 setStructuredTypeMembers until it sets the signatures. In
    // getResolvedMembersOrExportsOfSymbol: the early table (:16349 getResolvedMembersOrExportsOfSymbol: read by the
    // TypeLiteral test) before the late-bound members are added. A matching call cannot see a window open at one
    // read and closed at a later read. When the call is nested in the resolve, the window is outer to the call and
    // is open for the whole call. When the resolve starts in the call, the window is nested in one normalization or
    // in the work between two pairs, and is closed before the next read. After it, the answer is the answer before
    // the resolve, or true when the members are empty, and then the count moves.
    // Audit: target/continuation-r97-goport/int97/r230repair/identmatch1r3/audit.md. So:
    // - a result is kept only when its normalization changed no instantiation, type, symbol, signature,
    //   diagnostic or `empty_anonymous_resolves` count, and started under the instantiation limits
    //   (checker.go:22608 instantiateTypeWithAlias: 100 levels, 5,000,000 instantiations). A repeat can still
    //   instantiate (the Index branch: isGenericMappedType and getIndexTypeForMappedType on a mapped type with an
    //   `as` clause). An empty resolve in the first normalization changes the next ones (`X & T` before the resolve
    //   of an empty `X`, `T` after it).
    // - a kept type is used only while `empty_anonymous_resolves` has the count it was kept with, and under the
    //   count limit. The stack depth is the same at each pair of one call.
    // With `--generateTrace` on, nothing is kept: a repeat can emit a tracer event and change no count
    // (checker.go:27256 checkCrossProductUnion: the checkCrossProductUnion_DepthLimit event, after the intern miss
    // in getIntersectionTypeEx, which does not store the error type), and Go emits it at each repeat.
    fn normalize_for_matching(
        &mut self,
        t: TypeId,
        writing: bool,
        slot: &mut NormalizedSlot,
    ) -> TypeId {
        let under_count_limit = self.instantiation_count < 5_000_000;
        if slot.normalized.is_some()
            && slot.resolves == self.empty_anonymous_resolves
            && under_count_limit
        {
            return slot.normalized;
        }
        let counts = |c: &Checker| {
            (
                c.total_instantiation_count,
                c.type_count,
                c.symbol_count,
                c.signature_count,
                c.diagnostics.count,
                c.empty_anonymous_resolves,
            )
        };
        let can_keep =
            under_count_limit && self.instantiation_stack.len() < 100 && self.tracer.is_none();
        let before = counts(self);
        let normalized = self.get_normalized_type(t, writing);
        if can_keep && counts(self) == before {
            *slot = NormalizedSlot {
                normalized,
                resolves: self.empty_anonymous_resolves,
            };
        }
        normalized
    }

    // Go: checker/relater.go:358 checkTypeRelatedToEx with identityRelation and no error node, from the end of
    // isRelatedToEx's identity branch (relater.go:2666 traceUnionsOrIntersectionsTooLarge) on: `source` and
    // `target` are the pair after the fresh literal step, `normalized_*` its normalized types, `relation_size` the
    // size read before they were made.
    // PORT: perf (identmatch1). Not in Go as a function. It saves the second normalization of the pair.
    // Second copy: this repeats Go's `checkTypeRelatedToEx` (relater.go:358, whose port is
    // `check_type_related_to_ex` in relater_p1.rs) and the identity branch end of isRelatedToEx
    // (from relater.go:2666 traceUnionsOrIntersectionsTooLarge; its port is `is_related_to_ex`). A pin bump that
    // changes either changes both (UPSTREAM.md, "Pin bump checks").
    fn check_normalized_type_identical_to(
        &mut self,
        source: TypeId,
        target: TypeId,
        normalized_source: TypeId,
        normalized_target: TypeId,
        relation_size: i32,
    ) -> bool {
        let r = self.get_relater();
        {
            let mut rb = r.borrow_mut();
            rb.kind = RelationKind::Identity;
            rb.error_node = Node::NIL;
            rb.relation_count = (16_000_000 - relation_size) / 8;
        }
        self.trace_unions_or_intersections_too_large(&r, normalized_source, normalized_target);
        let result = self.recursive_type_related_to(
            &r,
            normalized_source,
            normalized_target,
            false, /*reportErrors*/
            IntersectionState::NONE,
            RecursionFlags::BOTH,
        );
        let (overflow, has_error_chain) = {
            let rb = r.borrow();
            (rb.overflow, rb.error_chain.is_some())
        };
        if overflow {
            // Record this relation as having failed such that we don't attempt the overflowing operation again.
            let (id, _) = self.get_relation_key(
                source,
                target,
                IntersectionState::NONE,
                true,  /*isIdentity*/
                false, /*ignoreConstraints*/
            );
            self.identity_relation.borrow_mut().set(
                id,
                RelationComparisonResult::FAILED | RelationComparisonResult::COMPLEXITY_OVERFLOW,
            );
            if let Some(tr) = self.tracer {
                let (depth, target_depth) = {
                    let rb = r.borrow();
                    (rb.source_stack.len(), rb.target_stack.len())
                };
                tr.instant(
                    crate::tracing::Phase::CheckTypes,
                    "checkTypeRelatedTo_DepthLimit",
                    vec![
                        ("sourceId", source.into()),
                        ("targetId", target.into()),
                        ("depth", depth.into()),
                        ("targetDepth", target_depth.into()),
                    ],
                );
            }
            let source_string = self.type_to_string(source);
            let target_string = self.type_to_string(target);
            let diagnostic = new_diagnostic_for_node(
                self.current_node,
                diag::Excessive_complexity_comparing_types_0_and_1,
                args![source_string, target_string],
            );
            self.report_diagnostic(diagnostic, None);
        } else if has_error_chain {
            // Go's import retry needs a head message and an error node; there are none here.
            let diagnostic = {
                let rb = r.borrow();
                create_diagnostic_chain_from_error_chain(
                    rb.error_chain.as_deref(),
                    rb.error_node,
                    rb.related_info.as_slice(),
                )
            };
            if let Some(diagnostic) = diagnostic {
                self.report_diagnostic(diagnostic, None);
            }
        }
        self.put_relater(r);
        result != Ternary::FALSE
    }

    // Go: checker/inference.go:1234 isTypeCloselyMatchedBy
    pub fn is_type_closely_matched_by(&self, s: TypeId, t: TypeId) -> bool {
        let st = self.ty(s);
        let tt = self.ty(t);
        st.flags.intersects(TypeFlags::OBJECT)
            && tt.flags.intersects(TypeFlags::OBJECT)
            && st.symbol.is_some()
            && st.symbol == tt.symbol
            || match (&st.alias, &tt.alias) {
                (Some(sa), Some(ta)) => !sa.type_arguments.is_empty() && sa.symbol == ta.symbol,
                _ => false,
            }
    }

    // Create an object with properties named in the string literal type. Every property has type `any`.
    // Go: checker/inference.go:1240 createEmptyObjectTypeFromStringLiteral
    pub fn create_empty_object_type_from_string_literal(&mut self, t: TypeId) -> TypeId {
        let members = self.symbols.new_table();
        let distributed = self.ty(t).distributed();
        for t in distributed {
            if !self.ty(t).flags.intersects(TypeFlags::STRING_LITERAL) {
                continue;
            }
            let name = self.get_string_literal_value(t);
            let literal_prop = self.new_symbol(SymbolFlags::PROPERTY, &name);
            let any_type = self.any_type;
            self.value_symbol_links
                .get_by_id(&self.symbols, literal_prop)
                .resolved_type = any_type;
            let t_symbol = self.ty(t).symbol;
            if t_symbol.is_some() {
                let (declarations, value_declaration) = {
                    let sym = self.sym(t_symbol);
                    (sym.declarations.clone(), sym.value_declaration)
                };
                let lp = self.sym_mut(literal_prop);
                lp.declarations = declarations;
                lp.value_declaration = value_declaration;
            }
            self.symbols.set(members, name, literal_prop);
        }
        let mut index_infos: Vec<IndexInfoId> = Vec::new();
        if self.ty(t).flags.intersects(TypeFlags::STRING) {
            let (string_type, empty_object_type) = (self.string_type, self.empty_object_type);
            index_infos = vec![self.new_index_info(
                string_type,
                empty_object_type,
                false, /*isReadonly*/
                Node::NIL,
                &[],
            )];
        }
        self.new_anonymous_type(SymbolId::NIL, members, &[], &[], &index_infos)
    }

    // Go: checker/inference.go:1262 newInferenceContext
    pub fn new_inference_context(
        &mut self,
        type_parameters: &[TypeId],
        signature: SignatureId,
        flags: InferenceFlags,
        compare_types: Option<TypeComparer>,
    ) -> InferenceContextId {
        let compare_types = match compare_types {
            Some(compare_types) => compare_types,
            None => self.compare_types_assignable.clone(),
        };
        let inferences: Vec<InferenceInfo> = type_parameters
            .iter()
            .map(|&tp| new_inference_info(tp))
            .collect();
        self.new_inference_context_worker(inferences, signature, flags, compare_types)
    }

    /// Go `c.newInferenceContext(signature.TypeParameters(), signature, flags, nil)`.
    /// PERF: the inference list is made from the signature's type parameters
    /// in place, without a copy of the list.
    pub fn new_inference_context_of_signature(
        &mut self,
        signature: SignatureId,
        flags: InferenceFlags,
    ) -> InferenceContextId {
        let compare_types = self.compare_types_assignable.clone();
        let inferences: Vec<InferenceInfo> = self
            .sig(signature)
            .type_parameters
            .iter()
            .map(|&tp| new_inference_info(tp))
            .collect();
        self.new_inference_context_worker(inferences, signature, flags, compare_types)
    }

    // Go: checker/inference.go:1269 cloneInferenceContext
    pub fn clone_inference_context(
        &mut self,
        n: InferenceContextId,
        extra_flags: InferenceFlags,
    ) -> InferenceContextId {
        if n.is_nil() {
            return InferenceContextId::NIL;
        }
        let (inferences, signature, flags, compare_types) = {
            let ctx = self.inference_context(n);
            (
                ctx.inferences
                    .iter()
                    .map(clone_inference_info)
                    .collect::<Vec<_>>(),
                ctx.signature,
                ctx.flags | extra_flags,
                ctx.compare_types.clone(),
            )
        };
        self.new_inference_context_worker(inferences, signature, flags, compare_types)
    }

    // Go: checker/inference.go:1276 cloneInferredPartOfContext
    pub fn clone_inferred_part_of_context(&mut self, n: InferenceContextId) -> InferenceContextId {
        let count = self.inference_context(n).inferences.len();
        // PERF: Go filters the info pointers, then clones each kept info.
        // The port clones each kept info once (`clone_inference_info` is a
        // field-by-field clone), with no second copy of the list.
        let mut inferences: Vec<InferenceInfo> = Vec::new();
        for i in 0..count {
            if self.has_inference_candidates(n, i) {
                inferences.push(clone_inference_info(
                    &self.inference_context(n).inferences[i],
                ));
            }
        }
        if inferences.is_empty() {
            return InferenceContextId::NIL;
        }
        let (signature, flags, compare_types) = {
            let ctx = self.inference_context(n);
            (ctx.signature, ctx.flags, ctx.compare_types.clone())
        };
        self.new_inference_context_worker(inferences, signature, flags, compare_types)
    }

    // Go: checker/inference.go:1284 newInferenceContextWorker
    pub fn new_inference_context_worker(
        &mut self,
        inferences: Vec<InferenceInfo>,
        signature: SignatureId,
        flags: InferenceFlags,
        compare_types: TypeComparer,
    ) -> InferenceContextId {
        let n = InferenceContextId(self.inference_contexts.len() as u32);
        // PERF: every field is set here. `..InferenceContext::default()`
        // would allocate the nil comparer (an `Rc`) and drop it again.
        self.inference_contexts.push(InferenceContext {
            inferences: inferences.into_boxed_slice(),
            signature,
            flags,
            compare_types,
            mapper: MapperId::NIL,
            non_fixing_mapper: MapperId::NIL,
            rare: None,
        });
        let mapper = self.new_inference_type_mapper(n, true /*fixing*/);
        self.inference_context_mut(n).mapper = mapper;
        let non_fixing_mapper = self.new_inference_type_mapper(n, false /*fixing*/);
        self.inference_context_mut(n).non_fixing_mapper = non_fixing_mapper;
        n
    }

    // Go: checker/inference.go:1296 addIntraExpressionInferenceSite
    pub fn add_intra_expression_inference_site(
        &mut self,
        n: InferenceContextId,
        node: Node,
        t: TypeId,
    ) {
        self.inference_context_mut(n)
            .rare_mut()
            .intra_expression_inference_sites
            .push(IntraExpressionInferenceSite { node, t });
    }

    // We collect intra-expression inference sites within object and array literals to handle cases where
    // inferred types flow between context sensitive element expressions. For example:
    //
    //	declare function foo<T>(arg: [(n: number) => T, (x: T) => void]): void;
    //	foo([_a => 0, n => n.toFixed()]);
    //
    // Above, both arrow functions in the tuple argument are context sensitive, thus both are omitted from the
    // pass that collects inferences from the non-context sensitive parts of the arguments. In the subsequent
    // pass where nothing is omitted, we need to commit to an inference for T in order to contextually type the
    // parameter in the second arrow function, but we want to first infer from the return type of the first
    // arrow function. This happens automatically when the arrow functions are discrete arguments (because we
    // infer from each argument before processing the next), but when the arrow functions are elements of an
    // object or array literal, we need to perform intra-expression inferences early.
    // Go: checker/inference.go:1313 inferFromIntraExpressionSites
    pub fn infer_from_intra_expression_sites(&mut self, n: InferenceContextId) {
        // PORT: Go ranges over the slice header taken at loop start; the clone
        // keeps that behavior when inference appends new sites.
        let sites = self
            .inference_context(n)
            .intra_expression_inference_sites()
            .to_vec();
        for site in sites {
            let contextual_type = if is_method_declaration(site.node) {
                self.get_contextual_type_for_object_literal_method(
                    site.node,
                    ContextFlags::NO_CONSTRAINTS,
                )
            } else {
                self.get_contextual_type(site.node, ContextFlags::NO_CONSTRAINTS)
            };
            if contextual_type.is_some() {
                self.infer_types(n, site.t, contextual_type, InferencePriority::NONE, false);
            }
        }
        if let Some(rare) = &mut self.inference_context_mut(n).rare {
            rare.intra_expression_inference_sites = Vec::new();
        }
    }

    // Go: checker/inference.go:1328 getInferredType
    pub fn get_inferred_type(&mut self, n: InferenceContextId, index: i32) -> TypeId {
        let index = index as usize;
        if self.inference_context(n).inferences[index]
            .inferred_type
            .is_nil()
        {
            let type_parameter = self.inference_context(n).inferences[index].type_parameter;
            if type_parameter == self.error_type {
                return type_parameter;
            }
            let mut inferred_type = TypeId::NIL;
            let mut fallback_type = TypeId::NIL;
            let signature = self.inference_context(n).signature;
            let n_flags = self.inference_context(n).flags;
            if signature.is_some() {
                let mut inferred_covariant_type = TypeId::NIL;
                if !self.inference_context(n).inferences[index]
                    .candidates()
                    .is_empty()
                {
                    inferred_covariant_type = self.get_covariant_inference(n, index, signature);
                }
                let mut inferred_contravariant_type = TypeId::NIL;
                if !self.inference_context(n).inferences[index]
                    .contra_candidates()
                    .is_empty()
                {
                    inferred_contravariant_type = self.get_contravariant_inference(n, index);
                }
                if inferred_covariant_type.is_some() || inferred_contravariant_type.is_some() {
                    // If we have both co- and contra-variant inferences, we prefer the co-variant inference if it is not 'never',
                    // all co-variant inferences are assignable to it (i.e. it isn't one of a conflicting set of candidates), it is
                    // assignable to some contra-variant inference, and no other type parameter is constrained to this type parameter
                    // and has inferences that would conflict. Otherwise, we prefer the contra-variant inference.
                    // Similarly ignore co-variant `any` inference when both are available as almost everything is assignable to it
                    // and it would spoil the overall inference.
                    let prefer_covariant_type = inferred_covariant_type.is_some()
                        && (inferred_contravariant_type.is_nil()
                            || !self
                                .ty(inferred_covariant_type)
                                .flags
                                .intersects(TypeFlags::NEVER | TypeFlags::ANY)
                                && {
                                    // PORT: perf. The snapshot (Go ranges over
                                    // the slice) lives on the stack up to 8.
                                    let contra_candidates: SmallVec<[TypeId; 8]> =
                                        SmallVec::from_slice(
                                            self.inference_context(n).inferences[index]
                                                .contra_candidates(),
                                        );
                                    let mut some = false;
                                    for t in contra_candidates {
                                        if self.is_type_assignable_to(inferred_covariant_type, t) {
                                            some = true;
                                            break;
                                        }
                                    }
                                    some
                                }
                                && {
                                    let count = self.inference_context(n).inferences.len();
                                    let mut every = true;
                                    for j in 0..count {
                                        let other_type_parameter =
                                            self.inference_context(n).inferences[j].type_parameter;
                                        let ok = j != index
                                            && self.get_constraint_of_type_parameter(
                                                other_type_parameter,
                                            ) != type_parameter
                                            || {
                                                let other_candidates: SmallVec<[TypeId; 8]> =
                                                    SmallVec::from_slice(
                                                        self.inference_context(n).inferences[j]
                                                            .candidates(),
                                                    );
                                                let mut all = true;
                                                for t in other_candidates {
                                                    if !self.is_type_assignable_to(
                                                        t,
                                                        inferred_covariant_type,
                                                    ) {
                                                        all = false;
                                                        break;
                                                    }
                                                }
                                                all
                                            };
                                        if !ok {
                                            every = false;
                                            break;
                                        }
                                    }
                                    every
                                });
                    if prefer_covariant_type {
                        inferred_type = inferred_covariant_type;
                        fallback_type = inferred_contravariant_type;
                    } else {
                        inferred_type = inferred_contravariant_type;
                        fallback_type = inferred_covariant_type;
                    }
                } else if n_flags.intersects(InferenceFlags::NO_DEFAULT) {
                    // We use silentNeverType as the wildcard that signals no inferences.
                    inferred_type = self.silent_never_type;
                } else {
                    // Infer either the default or the empty object type when no inferences were
                    // made. It is important to remember that in this case, inference still
                    // succeeds, meaning there is no error for not having inference candidates. An
                    // inference error only occurs when there are *conflicting* candidates, i.e.
                    // candidates with no common supertype.
                    let default_type = self.get_default_from_type_parameter(type_parameter);
                    if default_type.is_some() {
                        // Instantiate the default type. Any forward reference to a type
                        // parameter should be instantiated to the empty object type.
                        let backreference_mapper = self.new_backreference_mapper(n, index as i32);
                        let non_fixing_mapper = self.inference_context(n).non_fixing_mapper;
                        let mapper =
                            self.merge_type_mappers(backreference_mapper, non_fixing_mapper);
                        inferred_type = self.instantiate_type(default_type, mapper);
                    }
                }
            } else {
                inferred_type = self.get_type_from_inference(n, index);
            }
            self.inference_context_mut(n).inferences[index].inferred_type = inferred_type;
            if self.inference_context(n).inferences[index]
                .inferred_type
                .is_nil()
            {
                let t = if n_flags.intersects(InferenceFlags::ANY_DEFAULT) {
                    self.any_type
                } else {
                    self.unknown_type
                };
                self.inference_context_mut(n).inferences[index].inferred_type = t;
            }
            let constraint = self.get_constraint_of_type_parameter(type_parameter);
            if constraint.is_some() {
                let non_fixing_mapper = self.inference_context(n).non_fixing_mapper;
                let instantiated_constraint = self.instantiate_type(constraint, non_fixing_mapper);
                let compare_types = self.inference_context(n).compare_types.clone();
                // A pure return type inference is still filtered in a recursive call resolution, whose result can become the type of the enclosing declaration.
                // (ts#64530, Go N' inference.go:1393)
                if inferred_type.is_some()
                    && (!self
                        .inference_context(n)
                        .flags
                        .intersects(InferenceFlags::NO_CONSTRAINT_CHECKS)
                        || self.inference_context(n).inferences[index].priority
                            == InferencePriority::RETURN_TYPE)
                {
                    let constraint_with_this = self.get_type_with_this_argument(
                        instantiated_constraint,
                        inferred_type,
                        false,
                    );
                    if (*compare_types)(self, inferred_type, constraint_with_this, false)
                        == Ternary::FALSE
                    {
                        let mut filtered_by_constraint = TypeId::NIL;
                        if self.inference_context(n).inferences[index].priority
                            == InferencePriority::RETURN_TYPE
                        {
                            // If we have a pure return type inference, we may succeed by removing constituents of the inferred type
                            // that aren't assignable to the constraint type (pure return type inferences are speculation anyway).
                            let cmp = compare_types.clone();
                            filtered_by_constraint = self.map_type(
                                inferred_type,
                                &mut |c: &mut Checker, t: TypeId| -> TypeId {
                                    if (*cmp)(c, t, constraint_with_this, false) != Ternary::FALSE {
                                        t
                                    } else {
                                        c.never_type
                                    }
                                },
                            );
                        }
                        inferred_type = if filtered_by_constraint.is_some()
                            && !self
                                .ty(filtered_by_constraint)
                                .flags
                                .intersects(TypeFlags::NEVER)
                        {
                            filtered_by_constraint
                        } else {
                            TypeId::NIL
                        };
                    }
                }
                if inferred_type.is_nil() {
                    // If the fallback type satisfies the constraint, we pick it. Otherwise, we pick the constraint.
                    let fallback_satisfies = fallback_type.is_some() && {
                        let fallback_with_this = self.get_type_with_this_argument(
                            instantiated_constraint,
                            fallback_type,
                            false,
                        );
                        (*compare_types)(self, fallback_type, fallback_with_this, false)
                            != Ternary::FALSE
                    };
                    inferred_type = if fallback_satisfies {
                        fallback_type
                    } else {
                        instantiated_constraint
                    };
                }
                self.inference_context_mut(n).inferences[index].inferred_type = inferred_type;
            }
            self.clear_active_mapper_caches();
        }
        self.inference_context(n).inferences[index].inferred_type
    }

    // Go: checker/inference.go:1418 getInferredTypes
    pub fn get_inferred_types(&mut self, n: InferenceContextId) -> Vec<TypeId> {
        let count = self.inference_context(n).inferences.len();
        let mut result = vec![TypeId::NIL; count];
        for i in 0..count {
            result[i] = self.get_inferred_type(n, i as i32);
        }
        result
    }

    /// `get_inferred_types` in a list that lives on the stack up to 4 types.
    /// PERF: for callers that only read the list.
    pub fn get_inferred_type_list(&mut self, n: InferenceContextId) -> SmallVec<[TypeId; 4]> {
        let count = self.inference_context(n).inferences.len();
        let mut result: SmallVec<[TypeId; 4]> = SmallVec::with_capacity(count);
        for i in 0..count {
            result.push(self.get_inferred_type(n, i as i32));
        }
        result
    }

    // Go: checker/inference.go:1426 getMapperFromContext
    pub fn get_mapper_from_context(&self, n: InferenceContextId) -> MapperId {
        if n.is_nil() {
            return MapperId::NIL;
        }
        self.inference_context(n).mapper
    }

    // Return a type mapper that combines the context's return mapper with a mapper that erases any additional type parameters
    // to their inferences at the time of creation.
    // Go: checker/inference.go:1435 createOuterReturnMapper
    pub fn create_outer_return_mapper(&mut self, context: InferenceContextId) -> MapperId {
        if self
            .inference_context(context)
            .outer_return_mapper()
            .is_nil()
        {
            let cloned = self.clone_inference_context(context, InferenceFlags::NONE);
            let mut mapper = self.inference_context(cloned).mapper;
            let return_mapper = self.inference_context(context).return_mapper();
            if return_mapper.is_some() {
                mapper = self.new_merged_type_mapper(return_mapper, mapper);
            }
            self.inference_context_mut(context)
                .rare_mut()
                .outer_return_mapper = mapper;
        }
        self.inference_context(context).outer_return_mapper()
    }

    // Go: checker/inference.go:1446 getCovariantInference
    pub fn get_covariant_inference(
        &mut self,
        n: InferenceContextId,
        inference: usize,
        signature: SignatureId,
    ) -> TypeId {
        // PORT: perf. The candidate snapshot lives on the stack up to 8.
        let (inference_candidates, type_parameter, top_level, is_fixed, priority) = {
            let info = &self.inference_context(n).inferences[inference];
            (
                SmallVec::<[TypeId; 8]>::from_slice(info.candidates()),
                info.type_parameter,
                info.top_level,
                info.is_fixed,
                info.priority,
            )
        };
        // Extract all object and array literal types and replace them with a single widened and normalized type.
        let candidates = self.union_object_and_array_literal_candidates(&inference_candidates);
        // We widen inferred literal types if
        // all inferences were made to top-level occurrences of the type parameter, and
        // the type parameter has no constraint or its constraint includes no primitive or literal types, and
        // the type parameter was fixed during inference or does not occur at top-level in the return type.
        let primitive_constraint = self.has_primitive_constraint(type_parameter)
            || self.is_const_type_variable(type_parameter, 0);
        let widen_literal_types = !primitive_constraint
            && top_level
            && (is_fixed
                || !self.is_type_parameter_at_top_level_in_return_type(signature, type_parameter));
        // PERF: the candidate lists are temporaries, so they live on the
        // stack up to 8 types.
        let base_candidates: SmallVec<[TypeId; 8]> = if primitive_constraint {
            candidates
                .iter()
                .map(|&t| self.get_regular_type_of_literal_type(t))
                .collect()
        } else if widen_literal_types {
            candidates
                .iter()
                .map(|&t| self.get_widened_literal_type(t))
                .collect()
        } else {
            candidates
        };
        // If all inferences were made from a position that implies a combined result, infer a union type.
        // Otherwise, infer a common supertype.
        let unwidened_type = if priority.intersects(InferencePriority::PRIORITY_IMPLIES_COMBINATION)
        {
            self.get_union_type_ex(&base_candidates, UnionReduction::SUBTYPE, None, TypeId::NIL)
        } else {
            self.get_common_supertype(&base_candidates)
        };
        self.get_widened_type(unwidened_type)
    }

    // Go: checker/inference.go:1475 getContravariantInference
    pub fn get_contravariant_inference(
        &mut self,
        n: InferenceContextId,
        inference: usize,
    ) -> TypeId {
        // PORT: perf. The candidate snapshot lives on the stack up to 8.
        let (priority, contra_candidates) = {
            let info = &self.inference_context(n).inferences[inference];
            (
                info.priority,
                SmallVec::<[TypeId; 8]>::from_slice(info.contra_candidates()),
            )
        };
        if priority.intersects(InferencePriority::PRIORITY_IMPLIES_COMBINATION) {
            return self.get_intersection_type(&contra_candidates);
        }
        self.get_common_subtype(&contra_candidates)
    }

    // Go: checker/inference.go:1482 unionObjectAndArrayLiteralCandidates
    pub fn union_object_and_array_literal_candidates(
        &mut self,
        candidates: &[TypeId],
    ) -> SmallVec<[TypeId; 8]> {
        if candidates.len() > 1 {
            let object_literals: SmallVec<[TypeId; 8]> = candidates
                .iter()
                .copied()
                .filter(|&t| self.is_object_or_array_literal_type(t))
                .collect();
            if !object_literals.is_empty() {
                let literals_type = self.get_union_type_ex(
                    &object_literals,
                    UnionReduction::SUBTYPE,
                    None,
                    TypeId::NIL,
                );
                let mut non_literal_types: SmallVec<[TypeId; 8]> = candidates
                    .iter()
                    .copied()
                    .filter(|&t| !self.is_object_or_array_literal_type(t))
                    .collect();
                non_literal_types.push(literals_type);
                return non_literal_types;
            }
        }
        SmallVec::from_slice(candidates)
    }

    // Go: checker/inference.go:1494 hasPrimitiveConstraint
    pub fn has_primitive_constraint(&mut self, t: TypeId) -> bool {
        let mut constraint = self.get_constraint_of_type_parameter(t);
        if constraint.is_some() {
            if self.ty(constraint).flags.intersects(TypeFlags::CONDITIONAL) {
                constraint = self.get_default_constraint_of_conditional_type(constraint);
            }
            return self.maybe_type_of_kind(
                constraint,
                TypeFlags::PRIMITIVE
                    | TypeFlags::INDEX
                    | TypeFlags::TEMPLATE_LITERAL
                    | TypeFlags::STRING_MAPPING,
            );
        }
        false
    }

    // Go: checker/inference.go:1505 isTypeParameterAtTopLevel
    pub fn is_type_parameter_at_top_level(&mut self, t: TypeId, tp: TypeId, depth: i32) -> bool {
        if t == tp {
            return true;
        }
        if self
            .ty(t)
            .flags
            .intersects(TypeFlags::UNION_OR_INTERSECTION)
        {
            for i in 0..self.ty(t).types().len() {
                let u = self.type_at(t, i);
                if self.is_type_parameter_at_top_level(u, tp, depth) {
                    return true;
                }
            }
        }
        if depth < 3 && self.ty(t).flags.intersects(TypeFlags::CONDITIONAL) {
            let true_type = self.get_true_type_from_conditional_type(t);
            if self.is_type_parameter_at_top_level(true_type, tp, depth + 1) {
                return true;
            }
            let false_type = self.get_false_type_from_conditional_type(t);
            if self.is_type_parameter_at_top_level(false_type, tp, depth + 1) {
                return true;
            }
        }
        false
    }

    // Go: checker/inference.go:1513 isTypeParameterAtTopLevelInReturnType
    pub fn is_type_parameter_at_top_level_in_return_type(
        &mut self,
        signature: SignatureId,
        type_parameter: TypeId,
    ) -> bool {
        let type_predicate = self.get_type_predicate_of_signature(signature);
        if type_predicate.is_some() {
            let pt = self.pred(type_predicate).t;
            return pt.is_some() && self.is_type_parameter_at_top_level(pt, type_parameter, 0);
        }
        let return_type = self.get_return_type_of_signature(signature);
        self.is_type_parameter_at_top_level(return_type, type_parameter, 0)
    }

    // Go: checker/inference.go:1521 getTypeFromInference
    pub fn get_type_from_inference(&mut self, n: InferenceContextId, inference: usize) -> TypeId {
        // PORT: perf. The candidate snapshots live on the stack up to 8.
        let (candidates, contra_candidates) = {
            let info = &self.inference_context(n).inferences[inference];
            (
                SmallVec::<[TypeId; 8]>::from_slice(info.candidates()),
                SmallVec::<[TypeId; 8]>::from_slice(info.contra_candidates()),
            )
        };
        if !candidates.is_empty() {
            return self.get_union_type_ex(&candidates, UnionReduction::SUBTYPE, None, TypeId::NIL);
        }
        if !contra_candidates.is_empty() {
            return self.get_intersection_type(&contra_candidates);
        }
        TypeId::NIL
    }

    // PORT: Go package function that returns a nil-able `*InferenceInfo`. It
    // reads inference data, so it is a `Checker` method returning the index
    // into `inference_contexts[n.inferences].inferences`, or `None` for nil.
    // `InferenceState::inferences` is the `InferenceContextId` whose list Go
    // passes to `inferTypes`.
    // Go: checker/inference.go:1531 getInferenceInfoForType
    pub fn get_inference_info_for_type(&self, n: &InferenceState, t: TypeId) -> Option<usize> {
        if self.ty(t).flags.intersects(TypeFlags::TYPE_VARIABLE) {
            let t = self.get_non_distributed_type_parameter(t);
            for (i, inference) in self
                .inference_context(n.inferences)
                .inferences
                .iter()
                .enumerate()
            {
                if t == inference.type_parameter {
                    return Some(i);
                }
            }
        }
        None
    }

    // Go: checker/inference.go:1543 getCommonSupertype
    pub fn get_common_supertype(&mut self, types: &[TypeId]) -> TypeId {
        if types.len() == 1 {
            return types[0];
        }
        // Remove nullable types from each of the candidates.
        let mut primary_types = types.to_vec();
        if self.strict_null_checks {
            primary_types = types
                .iter()
                .map(|&t| {
                    self.filter_type(t, &mut |c: &mut Checker, u: TypeId| {
                        !c.ty(u).flags.intersects(TypeFlags::NULLABLE)
                    })
                })
                .collect();
        }
        // When the candidate types are all literal types with the same base type, return a union
        // of those literal types. Otherwise, return the leftmost type for which no type to the
        // right is a supertype.
        let supertype = if self.literal_types_with_same_base_type(&primary_types) {
            self.get_union_type(&primary_types)
        } else {
            self.get_single_common_supertype(&primary_types)
        };
        // Add any nullable types that occurred in the candidates back to the result.
        // PORT: Go `core.Same(primaryTypes, types)` is true exactly when
        // `core.SameMap` changed no element, so compare elements.
        if primary_types.as_slice() == types {
            return supertype;
        }
        let flags = self.get_combined_type_flags(types) & TypeFlags::NULLABLE;
        self.get_nullable_type(supertype, flags)
    }

    // Go: checker/inference.go:1570 getSingleCommonSupertype
    pub fn get_single_common_supertype(&mut self, types: &[TypeId]) -> TypeId {
        // First, find the leftmost type for which no type to the right is a strict supertype, and if that
        // type is a strict supertype of all other candidates, return it. Otherwise, return the leftmost type
        // for which no type to the right is a (regular) supertype.
        let candidate = self.find_leftmost_type(types, Checker::is_type_strict_subtype_of);
        let mut every = true;
        for &t in types {
            if !(t == candidate || self.is_type_strict_subtype_of(t, candidate)) {
                every = false;
                break;
            }
        }
        if every {
            return candidate;
        }
        self.find_leftmost_type(types, Checker::is_type_subtype_of)
    }

    // Go: checker/inference.go:1581 findLeftmostType
    pub fn find_leftmost_type(
        &mut self,
        types: &[TypeId],
        f: fn(&mut Checker, TypeId, TypeId) -> bool,
    ) -> TypeId {
        let mut candidate = TypeId::NIL;
        for &t in types {
            if candidate.is_nil() || f(self, candidate, t) {
                candidate = t;
            }
        }
        candidate
    }

    // Return the leftmost type for which no type to the right is a subtype.
    // Go: checker/inference.go:1592 getCommonSubtype
    pub fn get_common_subtype(&mut self, types: &[TypeId]) -> TypeId {
        let mut subtype = TypeId::NIL;
        for &t in types {
            if subtype.is_nil() || self.is_type_subtype_of(t, subtype) {
                subtype = t;
            }
        }
        subtype
    }

    // Go: checker/inference.go:1602 getCombinedTypeFlags
    pub fn get_combined_type_flags(&self, types: &[TypeId]) -> TypeFlags {
        let mut flags = TypeFlags::NONE;
        for &t in types {
            if self.ty(t).flags.intersects(TypeFlags::UNION) {
                flags |= self.get_combined_type_flags(self.ty(t).types());
            } else {
                flags |= self.ty(t).flags;
            }
        }
        flags
    }

    // Go: checker/inference.go:1614 literalTypesWithSameBaseType
    pub fn literal_types_with_same_base_type(&mut self, types: &[TypeId]) -> bool {
        let mut common_base_type = TypeId::NIL;
        for &t in types {
            if !self.ty(t).flags.intersects(TypeFlags::NEVER) {
                let base_type = self.get_base_type_of_literal_type(t);
                if common_base_type.is_nil() {
                    common_base_type = base_type;
                }
                if base_type == t || base_type != common_base_type {
                    return false;
                }
            }
        }
        true
    }

    // Go: checker/inference.go:1630 isFromInferenceBlockedSource
    pub fn is_from_inference_blocked_source(&self, t: TypeId) -> bool {
        // PERF: the skip set is empty outside the language service, and then
        // no declaration is in it. The reads below have no side effects.
        if self.skip_direct_inference_nodes.is_empty() {
            return false;
        }
        let symbol = self.ty(t).symbol;
        symbol.is_some()
            && self
                .sym(symbol)
                .declarations
                .iter()
                .any(|&d| self.is_skip_direct_inference_node(d))
    }

    // Go: checker/inference.go:1634 isSkipDirectInferenceNode
    pub fn is_skip_direct_inference_node(&self, node: Node) -> bool {
        self.skip_direct_inference_nodes.contains(&node)
    }
}

// Go: checker/inference.go:1638 newInferenceInfo
pub fn new_inference_info(type_parameter: TypeId) -> InferenceInfo {
    InferenceInfo {
        type_parameter,
        priority: InferencePriority::MAX_VALUE,
        top_level: true,
        implied_arity: -1,
        ..InferenceInfo::default()
    }
}

// Go: checker/inference.go:1642 cloneInferenceInfo
pub fn clone_inference_info(info: &InferenceInfo) -> InferenceInfo {
    InferenceInfo {
        type_parameter: info.type_parameter,
        candidate_lists: info.candidate_lists.clone(),
        first_candidate: info.first_candidate,
        first_is_contra: info.first_is_contra,
        inferred_type: info.inferred_type,
        priority: info.priority,
        top_level: info.top_level,
        is_fixed: info.is_fixed,
        implied_arity: info.implied_arity,
    }
}

impl Checker {
    // PORT: Go package function over `[]*InferenceInfo`; takes the context
    // that owns the list.
    // Go: checker/inference.go:1655 clearCachedInferences
    pub fn clear_cached_inferences(&mut self, inferences: InferenceContextId) {
        for inference in &mut self.inference_context_mut(inferences).inferences {
            if !inference.is_fixed {
                inference.inferred_type = TypeId::NIL;
            }
        }
    }

    // PORT: Go package function over one `*InferenceInfo`; takes
    // (context, index).
    // Go: checker/inference.go:1663 hasInferenceCandidates
    pub fn has_inference_candidates(&self, n: InferenceContextId, info: usize) -> bool {
        let info = &self.inference_context(n).inferences[info];
        !info.candidates().is_empty() || !info.contra_candidates().is_empty()
    }

    // Go: checker/inference.go:1667 hasInferenceCandidatesOrDefault
    pub fn has_inference_candidates_or_default(&self, n: InferenceContextId, info: usize) -> bool {
        self.has_inference_candidates(n, info)
            || has_type_parameter_default(
                self,
                self.inference_context(n).inferences[info].type_parameter,
            )
    }
}

// PORT: Go package function `hasTypeParameterDefault` in inference.go has the
// same name as the method `(*Checker).hasTypeParameterDefault`
// (checker.go:21847, `Checker::has_type_parameter_default`). It is a free
// function that takes the checker for type and symbol data, so the two
// names do not collide.
// Go: checker/inference.go:1671 hasTypeParameterDefault
pub fn has_type_parameter_default(c: &Checker, tp: TypeId) -> bool {
    let symbol = c.ty(tp).symbol;
    if symbol.is_some() {
        for &d in &c.sym(symbol).declarations {
            if is_type_parameter_declaration(d) && d.default_type().is_some() {
                return true;
            }
        }
    }
    false
}

impl Checker {
    // PORT: Go package function over two `[]*InferenceInfo`; takes the
    // contexts that own the lists.
    // Go: checker/inference.go:1682 hasOverlappingInferences
    pub fn has_overlapping_inferences(&self, a: InferenceContextId, b: InferenceContextId) -> bool {
        for i in 0..self.inference_context(a).inferences.len() {
            if self.has_inference_candidates(a, i) && self.has_inference_candidates(b, i) {
                return true;
            }
        }
        false
    }

    // PORT: Go stores the same `*InferenceInfo` in both lists. Rust copies the
    // value. Go callers drop the source list right after the merge, so no
    // later write can observe the difference.
    // Go: checker/inference.go:1691 mergeInferences
    pub fn merge_inferences(&mut self, target: InferenceContextId, source: InferenceContextId) {
        for i in 0..self.inference_context(target).inferences.len() {
            if !self.has_inference_candidates(target, i) && self.has_inference_candidates(source, i)
            {
                let info = self.inference_context(source).inferences[i].clone();
                self.inference_context_mut(target).inferences[i] = info;
            }
        }
    }
}
