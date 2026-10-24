use crate::prelude::*;

// Port of typescript-go `internal/checker/relater.go` lines 2796-3902.
//
// PORT: Go `*Relater` methods hold `r.c *Checker`. A Rust `Relater` cannot
// hold `&mut Checker`, and Go passes `r.isRelatedToWorker` as a stored
// `TypeComparer` (for example to `newInferenceContext`) that is called again
// while this relater is in use. So every `(r *Relater)` method is ported as
// an `impl Checker` method that takes the relater handle
// `r: &Rc<RefCell<Relater>>` as its first parameter (same snake name; Go has
// no Checker method with any of these names). `r.c.X` becomes `self.X`.
// Relater fields are read and written through short `r.borrow()` /
// `r.borrow_mut()` calls that never live across another call, so re-entrant
// use of the same relater is safe.

/// `is_type_subset_of_union` keeps the answer for a union source when its
/// searches make about this many comparisons or more. Shorter calls cost
/// about as much as a lookup, and the table stays small.
const UNION_SUBSET_MEMO_MIN: usize = 64;

/// Go `r.relation == rel` (pointer equality of `*Relation`).
fn relation_is(r: &Rc<RefCell<Relater>>, kind: RelationKind) -> bool {
    r.borrow().kind == kind
}

/// Go `a == b` on `*ErrorChain` values (pointer equality, nil == nil).
fn error_chain_eq(a: &Option<Rc<ErrorChain>>, b: &Option<Rc<ErrorChain>>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => Rc::ptr_eq(a, b),
        _ => false,
    }
}

/// Go closure `relateVariances` inside `structuredTypeRelatedToWorker`. It
/// assigns the enclosing `result`, `varianceCheckFailed` and
/// `originalErrorChain` locals, so those are passed by `&mut`.
#[allow(clippy::too_many_arguments)]
fn relate_variances(
    c: &mut Checker,
    r: &Rc<RefCell<Relater>>,
    source_type_arguments: &[TypeId],
    target_type_arguments: &[TypeId],
    variances: &[VarianceFlags],
    intersection_state: IntersectionState,
    report_errors: bool,
    save_error_state: &ErrorState,
    result: &mut Ternary,
    variance_check_failed: &mut bool,
    original_error_chain: &mut Option<Rc<ErrorChain>>,
) -> (Ternary, bool) {
    *result = c.type_arguments_related_to(
        r,
        source_type_arguments,
        target_type_arguments,
        variances,
        report_errors,
        intersection_state,
    );
    if *result != Ternary::FALSE {
        return (*result, true);
    }
    if variances
        .iter()
        .any(|v| v.intersects(VarianceFlags::ALLOWS_STRUCTURAL_FALLBACK))
    {
        // If some type parameter was `Unmeasurable` or `Unreliable`, and we couldn't pass by assuming it was identical, then we
        // have to allow a structural fallback check
        // We elide the variance-based error elaborations, since those might not be too helpful, since we'll potentially
        // be assuming identity of the type parameter.
        *original_error_chain = None;
        c.restore_error_state(r, save_error_state.clone());
        return (Ternary::FALSE, false);
    }
    let allow_structural_fallback = c.has_covariant_void_argument(target_type_arguments, variances);
    *variance_check_failed = !allow_structural_fallback;
    // The type arguments did not relate appropriately, but it may be because we have no variance
    // information (in which case typeArgumentsRelatedTo defaulted to covariance for all type
    // arguments). It might also be the case that the target type has a 'void' type argument for
    // a covariant type parameter that is only used in return positions within the generic type
    // (in which case any type argument is permitted on the source side). In those cases we proceed
    // with a structural comparison. Otherwise, we know for certain the instantiations aren't
    // related and we can return here.
    if !variances.is_empty() && !allow_structural_fallback {
        // In some cases generic types that are covariant in regular type checking mode become
        // invariant in --strictFunctionTypes mode because one or more type parameters are used in
        // both co- and contravariant positions. In order to make it easier to diagnose *why* such
        // types are invariant, if any of the type parameters are invariant we reset the reported
        // errors and instead force a structural comparison (which will include elaborations that
        // reveal the reason).
        // We can switch on `reportErrors` here, since varianceCheckFailed guarantees we return `False`,
        // we can return `False` early here to skip calculating the structural error message we don't need.
        if *variance_check_failed
            && !(report_errors
                && variances
                    .iter()
                    .any(|v| (*v & VarianceFlags::VARIANCE_MASK) == VarianceFlags::INVARIANT))
        {
            return (Ternary::FALSE, true);
        }
        // We remember the original error information so we can restore it in case the structural
        // comparison unexpectedly succeeds. This can happen when the structural comparison result
        // is a Ternary.Maybe for example caused by the recursion depth limiter.
        *original_error_chain = r.borrow().error_chain.clone();
        c.restore_error_state(r, save_error_state.clone());
    }
    (Ternary::FALSE, false)
}

impl Checker {
    // Go: checker/relater.go:2830 getTypeOfPropertyInTypes
    // PERF (jsxname1): `name` is the property symbol's `Name`, so the
    // lookups compare name ids.
    pub fn get_type_of_property_in_types(&mut self, types: &[TypeId], name: &Name) -> TypeId {
        let mut prop_types: Vec<TypeId> = Vec::new();
        for &t in types {
            let prop_type = self.get_type_of_property_in_type(t, name);
            prop_types.push(prop_type);
        }
        self.get_union_type(&prop_types)
    }

    // Go: checker/relater.go:2838 getTypeOfPropertyInType
    pub fn get_type_of_property_in_type(&mut self, t: TypeId, name: &Name) -> TypeId {
        let t = self.get_apparent_type(t);
        let prop = if self
            .ty(t)
            .flags
            .intersects(TypeFlags::UNION_OR_INTERSECTION)
        {
            self.get_property_of_union_or_intersection_type_key(t, TableKey::Name(name), false)
        } else {
            self.get_property_of_object_type_key(t, TableKey::Name(name))
        };
        if prop.is_some() {
            return self.get_type_of_symbol(prop);
        }
        let index_info = self.get_applicable_index_info_for_name(t, name);
        if index_info.is_some() {
            return self.index_info(index_info).value_type;
        }
        self.undefined_type
    }

    // Go: checker/relater.go:2856 shouldCheckAsExcessProperty
    pub fn should_check_as_excess_property(&self, prop: SymbolId, container: SymbolId) -> bool {
        let prop_decl = self.sym(prop).value_declaration;
        let container_decl = self.sym(container).value_declaration;
        prop_decl.is_some() && container_decl.is_some() && prop_decl.parent() == container_decl
    }

    // Go: checker/relater.go:2860 isIgnoredJsxProperty
    pub fn is_ignored_jsx_property(&self, source: TypeId, source_prop: SymbolId) -> bool {
        self.ty(source)
            .object_flags
            .intersects(ObjectFlags::JSX_ATTRIBUTES)
            && is_hyphenated_jsx_name(&self.sym(source_prop).name)
    }

    // Go: checker/relater.go:2864 isTypeSubsetOf
    pub fn is_type_subset_of(&mut self, source: TypeId, target: TypeId) -> bool {
        source == target
            || self.ty(source).flags.intersects(TypeFlags::NEVER)
            || self.ty(target).flags.intersects(TypeFlags::UNION)
                && self.is_type_subset_of_union(source, target)
    }

    // Go: checker/relater.go:2868 isTypeSubsetOfUnion
    // PERF (unionsub1): not in Go. When the searches of a union source are
    // long, the answer is kept in `union_subset_answers` and a repeat call
    // returns it. eslint-plugin-svelte makes 10,724 calls on 1,039 distinct
    // pairs, and the repeats made 16.2 M of the 19.7 M comparisons (sources
    // of 43,000 types searched in unions of 55,844). A repeat has Go's
    // answer and no effect: the member lists and the fields that
    // `compare_types` reads do not change after a type is made, so Go's
    // repeat makes the same comparisons with the same results, and the one
    // effect of a comparison (a lazy symbol id) came with the first call.
    // A long first call searches on entries (`contains_types_by_entries`).
    // Go's only caller is `isTypeSubsetOf` (from flow.go and relater.go:2754);
    // `removeSubtypes` (checker.go:26534) does not call it.
    pub fn is_type_subset_of_union(&mut self, source: TypeId, target: TypeId) -> bool {
        if self.ty(source).flags.intersects(TypeFlags::UNION) {
            let n = self.ty(source).types().len();
            let m = self.ty(target).types().len();
            // About the number of comparisons (n searches of log2(m) steps).
            let work = n * (usize::BITS - m.leading_zeros()) as usize;
            let keep = work >= UNION_SUBSET_MEMO_MIN;
            if keep && let Some(&answer) = self.union_subset_answers.get(&(source, target)) {
                return answer;
            }
            let (sources, targets) = (self.ty(source).types(), self.ty(target).types());
            // The entries cost about one comparison per type.
            let answer = if work >= 4 * (n + m) {
                self.contains_types_by_entries(targets, sources)
            } else {
                sources.iter().all(|&t| self.contains_type(targets, t))
            };
            if keep {
                self.union_subset_answers.insert((source, target), answer);
            }
            return answer;
        }
        if self.ty(source).flags.intersects(TypeFlags::ENUM_LIKE)
            && self.get_base_type_of_enum_like_type(source) == target
        {
            return true;
        }
        self.contains_type(self.ty(target).types(), source)
    }

    // Go: checker/relater.go:2883 unionOrIntersectionRelatedTo
    pub fn union_or_intersection_related_to(
        &mut self,
        r: &Rc<RefCell<Relater>>,
        source: TypeId,
        target: TypeId,
        report_errors: bool,
        intersection_state: IntersectionState,
    ) -> Ternary {
        let mut source = source;
        // Note that these checks are specifically ordered to produce correct results. In particular,
        // we need to deconstruct unions before intersections (because unions are always at the top),
        // and we need to handle "each" relations before "some" relations for the same kind of type.
        if self.ty(source).flags.intersects(TypeFlags::UNION) {
            if self.ty(target).flags.intersects(TypeFlags::UNION) {
                // Intersections of union types are normalized into unions of intersection types, and such normalized
                // unions can get very large and expensive to relate. The following fast path checks if the source union
                // originated in an intersection. If so, and if that intersection contains the target type, then we know
                // the result to be true (for any two types A and B, A & B is related to both A and B).
                let source_origin = self.ty(source).as_union_type().origin;
                if source_origin.is_some()
                    && self
                        .ty(source_origin)
                        .flags
                        .intersects(TypeFlags::INTERSECTION)
                    && self.ty(target).alias.is_some()
                    && self.ty(source_origin).types().contains(&target)
                {
                    return Ternary::TRUE;
                }
                // Similarly, in unions of unions the we preserve the original list of unions. This original list is often
                // much shorter than the normalized result, so we scan it in the following fast path.
                let target_origin = self.ty(target).as_union_type().origin;
                if target_origin.is_some()
                    && self.ty(target_origin).flags.intersects(TypeFlags::UNION)
                    && self.ty(source).alias.is_some()
                    && self.ty(target_origin).types().contains(&source)
                {
                    return Ternary::TRUE;
                }
            }
            let source_is_primitive = self.ty(source).flags.intersects(TypeFlags::PRIMITIVE);
            if relation_is(r, RelationKind::Comparable) {
                return self.some_type_related_to_type(
                    r,
                    source,
                    target,
                    report_errors && !source_is_primitive,
                    intersection_state,
                );
            }
            return self.each_type_related_to_type(
                r,
                source,
                target,
                report_errors && !source_is_primitive,
                intersection_state,
            );
        }
        if self.ty(target).flags.intersects(TypeFlags::UNION) {
            let regular_source = self.get_regular_type_of_object_literal(source);
            let report = report_errors
                && !self.ty(source).flags.intersects(TypeFlags::PRIMITIVE)
                && !self.ty(target).flags.intersects(TypeFlags::PRIMITIVE);
            return self.type_related_to_some_type(
                r,
                regular_source,
                target,
                report,
                intersection_state,
            );
        }
        if self.ty(target).flags.intersects(TypeFlags::INTERSECTION) {
            return self.type_related_to_each_type(
                r,
                source,
                target,
                report_errors,
                IntersectionState::TARGET,
            );
        }
        // Source is an intersection. For the comparable relation, if the target is a primitive type we hoist the
        // constraints of all non-primitive types in the source into a new intersection. We do this because the
        // intersection may further constrain the constraints of the non-primitive types. For example, given a type
        // parameter 'T extends 1 | 2', the intersection 'T & 1' should be reduced to '1' such that it doesn't
        // appear to be comparable to '2'.
        if relation_is(r, RelationKind::Comparable)
            && self.ty(target).flags.intersects(TypeFlags::PRIMITIVE)
        {
            // PORT: Go `core.SameMap` returns the original slice when no element
            // changes, and `core.Same` then compares slice identity. Element-wise
            // equality with the original list gives the same answer.
            let source_types = self.ty(source).types_list();
            let mut constraints: Vec<TypeId> = Vec::with_capacity(source_types.len());
            for &t in &source_types {
                if self.ty(t).flags.intersects(TypeFlags::INSTANTIABLE) {
                    let constraint = self.get_base_constraint_of_type(t);
                    if constraint.is_some() {
                        constraints.push(constraint);
                    } else {
                        constraints.push(self.unknown_type);
                    }
                } else {
                    constraints.push(t);
                }
            }
            if constraints[..] != source_types[..] {
                source = self.get_intersection_type(&constraints);
                if self.ty(source).flags.intersects(TypeFlags::NEVER) {
                    return Ternary::FALSE;
                }
                if !self.ty(source).flags.intersects(TypeFlags::INTERSECTION) {
                    let result = self.is_related_to(
                        r,
                        source,
                        target,
                        RecursionFlags::SOURCE,
                        false, /*reportErrors*/
                    );
                    if result != Ternary::FALSE {
                        return result;
                    }
                    return self.is_related_to(
                        r,
                        target,
                        source,
                        RecursionFlags::SOURCE,
                        false, /*reportErrors*/
                    );
                }
            }
        }
        // Check to see if any constituents of the intersection are immediately related to the target.
        // Don't report errors though. Elaborating on whether a source constituent is related to the target is
        // not actually useful and leads to some confusing error messages. Instead, we rely on the caller
        // checking whether the full intersection viewed as an object is related to the target.
        self.some_type_related_to_type(
            r,
            source,
            target,
            false, /*reportErrors*/
            IntersectionState::SOURCE,
        )
    }

    // Go: checker/relater.go:2952 someTypeRelatedToType
    pub fn some_type_related_to_type(
        &mut self,
        r: &Rc<RefCell<Relater>>,
        source: TypeId,
        target: TypeId,
        report_errors: bool,
        intersection_state: IntersectionState,
    ) -> Ternary {
        if self.ty(source).flags.intersects(TypeFlags::UNION)
            && self.contains_type(self.ty(source).types(), target)
        {
            return Ternary::TRUE;
        }
        let n = self.ty(source).types().len();
        for i in 0..n {
            let t = self.type_at(source, i);
            let related = self.is_related_to_ex(
                r,
                t,
                target,
                RecursionFlags::SOURCE,
                report_errors && i == n - 1,
                None, /*headMessage*/
                intersection_state,
            );
            if related != Ternary::FALSE {
                return related;
            }
        }
        Ternary::FALSE
    }

    // Go: checker/relater.go:2966 eachTypeRelatedToType
    pub fn each_type_related_to_type(
        &mut self,
        r: &Rc<RefCell<Relater>>,
        source: TypeId,
        target: TypeId,
        report_errors: bool,
        intersection_state: IntersectionState,
    ) -> Ternary {
        let mut result = Ternary::TRUE;
        // PORT: both constituent lists are read in place (`type_at`); they
        // never change once the types exist.
        let source_count = self.ty(source).types().len();
        // We strip `undefined` from the target if the `source` trivially doesn't contain it for our correspondence-checking fastpath
        // since `undefined` is frequently added by optionality and would otherwise spoil a potentially useful correspondence
        let stripped_target = self.get_undefined_stripped_target_if_needed(r, source, target);
        let stripped_is_union = self.ty(stripped_target).flags.intersects(TypeFlags::UNION);
        let stripped_count = if stripped_is_union {
            self.ty(stripped_target).types().len()
        } else {
            0
        };
        for i in 0..source_count {
            let source_type = self.type_at(source, i);
            if stripped_is_union
                && source_count >= stripped_count
                && source_count % stripped_count == 0
            {
                // many unions are mappings of one another; in such cases, simply comparing members at the same index can shortcut the comparison
                // such unions will have identical lengths, and their corresponding elements will match up. Another common scenario is where a large
                // union has a union of objects intersected with it. In such cases, if the input was, eg `("a" | "b" | "c") & (string | boolean | {} | {whatever})`,
                // the result will have the structure `"a" | "b" | "c" | "a" & {} | "b" & {} | "c" & {} | "a" & {whatever} | "b" & {whatever} | "c" & {whatever}`
                // - the resulting union has a length which is a multiple of the original union, and the elements correspond modulo the length of the original union
                let related = self.is_related_to_ex(
                    r,
                    source_type,
                    self.type_at(stripped_target, i % stripped_count),
                    RecursionFlags::BOTH,
                    false, /*reportErrors*/
                    None,  /*headMessage*/
                    intersection_state,
                );
                if related != Ternary::FALSE {
                    result &= related;
                    continue;
                }
            }
            let related = self.is_related_to_ex(
                r,
                source_type,
                target,
                RecursionFlags::SOURCE,
                report_errors,
                None, /*headMessage*/
                intersection_state,
            );
            if related == Ternary::FALSE {
                return Ternary::FALSE;
            }
            result &= related;
        }
        result
    }

    // Go: checker/relater.go:2998 getUndefinedStrippedTargetIfNeeded
    pub fn get_undefined_stripped_target_if_needed(
        &mut self,
        r: &Rc<RefCell<Relater>>,
        source: TypeId,
        target: TypeId,
    ) -> TypeId {
        if self.ty(source).flags.intersects(TypeFlags::UNION)
            && self.ty(target).flags.intersects(TypeFlags::UNION)
            && !self
                .ty(self.ty(source).types()[0])
                .flags
                .intersects(TypeFlags::UNDEFINED)
            && self
                .ty(self.ty(target).types()[0])
                .flags
                .intersects(TypeFlags::UNDEFINED)
        {
            return self.extract_types_of_kind(target, !TypeFlags::UNDEFINED);
        }
        target
    }

    // Go: checker/relater.go:3005 typeRelatedToSomeType
    pub fn type_related_to_some_type(
        &mut self,
        r: &Rc<RefCell<Relater>>,
        source: TypeId,
        target: TypeId,
        report_errors: bool,
        intersection_state: IntersectionState,
    ) -> Ternary {
        if self.ty(target).flags.intersects(TypeFlags::UNION) {
            if self.contains_type(self.ty(target).types(), source) {
                return Ternary::TRUE;
            }
            let source_flags = self.ty(source).flags;
            if !relation_is(r, RelationKind::Comparable)
                && self
                    .ty(target)
                    .object_flags
                    .intersects(ObjectFlags::PRIMITIVE_UNION)
                && !source_flags.intersects(TypeFlags::ENUM_LITERAL)
                && (source_flags.intersects(
                    TypeFlags::STRING_LITERAL
                        | TypeFlags::BOOLEAN_LITERAL
                        | TypeFlags::BIG_INT_LITERAL,
                ) || (relation_is(r, RelationKind::Subtype)
                    || relation_is(r, RelationKind::StrictSubtype))
                    && source_flags.intersects(TypeFlags::NUMBER_LITERAL))
            {
                // When relating a literal type to a union of primitive types, we know the relation is false unless
                // the union contains the base primitive type or the literal type in one of its fresh/regular forms.
                // We exclude numeric literals for non-subtype relations because numeric literals are assignable to
                // numeric enum literals with the same value. Similarly, we exclude enum literal types because
                // identically named enum types are related (see isEnumTypeRelatedTo). We exclude the comparable
                // relation in entirety because it needs to be checked in both directions.
                let alternate_form = {
                    let literal = self.ty(source).as_literal_type();
                    if source == literal.regular_type {
                        literal.fresh_type
                    } else {
                        literal.regular_type
                    }
                };
                let primitive = if source_flags.intersects(TypeFlags::STRING_LITERAL) {
                    self.string_type
                } else if source_flags.intersects(TypeFlags::NUMBER_LITERAL) {
                    self.number_type
                } else if source_flags.intersects(TypeFlags::BIG_INT_LITERAL) {
                    self.bigint_type
                } else {
                    TypeId::NIL
                };
                let target_types = self.ty(target).types();
                if primitive.is_some() && self.contains_type(target_types, primitive)
                    || alternate_form.is_some() && self.contains_type(target_types, alternate_form)
                {
                    return Ternary::TRUE;
                }
                return Ternary::FALSE;
            }
            let match_ = self.get_matching_union_constituent_for_type(target, source);
            if match_.is_some() {
                let related = self.is_related_to_ex(
                    r,
                    source,
                    match_,
                    RecursionFlags::TARGET,
                    false, /*reportErrors*/
                    None,  /*headMessage*/
                    intersection_state,
                );
                if related != Ternary::FALSE {
                    return related;
                }
            }
        }
        for i in 0..self.ty(target).types().len() {
            let t = self.type_at(target, i);
            let related = self.is_related_to_ex(
                r,
                source,
                t,
                RecursionFlags::TARGET,
                false, /*reportErrors*/
                None,  /*headMessage*/
                intersection_state,
            );
            if related != Ternary::FALSE {
                return related;
            }
        }
        if report_errors {
            // Elaborate only if we can find a best matching type in the target union
            let best_matching_type = self.get_best_matching_type(
                source,
                target,
                &mut |c: &mut Checker, s: TypeId, t: TypeId| c.is_related_to_simple(r, s, t),
            );
            if best_matching_type.is_some() {
                self.is_related_to_ex(
                    r,
                    source,
                    best_matching_type,
                    RecursionFlags::TARGET,
                    true, /*reportErrors*/
                    None, /*headMessage*/
                    intersection_state,
                );
            }
        }
        Ternary::FALSE
    }

    // Go: checker/relater.go:3064 typeRelatedToEachType
    pub fn type_related_to_each_type(
        &mut self,
        r: &Rc<RefCell<Relater>>,
        source: TypeId,
        target: TypeId,
        report_errors: bool,
        intersection_state: IntersectionState,
    ) -> Ternary {
        let mut result = Ternary::TRUE;
        for i in 0..self.ty(target).types().len() {
            let target_type = self.type_at(target, i);
            let related = self.is_related_to_ex(
                r,
                source,
                target_type,
                RecursionFlags::TARGET,
                report_errors,
                None, /*headMessage*/
                intersection_state,
            );
            if related == Ternary::FALSE {
                return Ternary::FALSE;
            }
            result &= related;
        }
        result
    }

    // Go: checker/relater.go:3077 eachTypeRelatedToSomeType
    pub fn each_type_related_to_some_type(
        &mut self,
        r: &Rc<RefCell<Relater>>,
        source: TypeId,
        target: TypeId,
    ) -> Ternary {
        let mut result = Ternary::TRUE;
        for i in 0..self.ty(source).types().len() {
            let source_type = self.type_at(source, i);
            let related = self.type_related_to_some_type(
                r,
                source_type,
                target,
                false, /*reportErrors*/
                IntersectionState::NONE,
            );
            if related == Ternary::FALSE {
                return Ternary::FALSE;
            }
            result &= related;
        }
        result
    }

    // Determine if possibly recursive types are related. First, check if the result is already available in the global cache.
    // Second, check if we have already started a comparison of the given two types in which case we assume the result to be true.
    // Third, check if both types are part of deeply nested chains of generic type instantiations and if so assume the types are
    // equal and infinitely expanding. Fourth, if we have reached a depth of 100 nested comparisons, assume we have runaway recursion
    // and issue an error. Otherwise, actually compare the structure of the two types.
    // Go: checker/relater.go:3095 recursiveTypeRelatedTo
    pub fn recursive_type_related_to(
        &mut self,
        r: &Rc<RefCell<Relater>>,
        source: TypeId,
        target: TypeId,
        report_errors: bool,
        intersection_state: IntersectionState,
        recursion_flags: RecursionFlags,
    ) -> Ternary {
        // PORT: perf. One borrow of `r` for `overflow` and the relation.
        let (overflow, kind) = {
            let rb = r.borrow();
            (rb.overflow, rb.kind)
        };
        if overflow {
            // Note that stack depth overflows can cause _any_ relation involving structured types to become false, so it is
            // important to have well-defined behavior even in cases that shouldn't normally occur.
            return Ternary::FALSE;
        }
        let is_identity = kind == RelationKind::Identity;
        let (id, constrained) = self.get_relation_key(
            source,
            target,
            intersection_state,
            is_identity,
            false, /*ignoreConstraints*/
        );
        let entry = self.relation_of(kind).borrow().get(id);
        if entry != RelationComparisonResult::NONE {
            if report_errors
                && entry.intersects(RelationComparisonResult::FAILED)
                && !entry.intersects(RelationComparisonResult::OVERFLOW)
            {
                // We are elaborating errors and the cached result is a failure not due to a comparison overflow,
                // so we will do the comparison again to generate an error message.
            } else {
                self.reliability_flags |= entry
                    & (RelationComparisonResult::REPORTS_UNMEASURABLE
                        | RelationComparisonResult::REPORTS_UNRELIABLE);
                if report_errors && entry.intersects(RelationComparisonResult::OVERFLOW) {
                    let source_string = self.type_to_string_exported(source);
                    let target_string = self.type_to_string_exported(target);
                    self.report_error(
                        r,
                        diag::Excessive_complexity_comparing_types_0_and_1,
                        args![source_string, target_string],
                    );
                }
                if entry.intersects(RelationComparisonResult::SUCCEEDED) {
                    return Ternary::TRUE;
                }
                return Ternary::FALSE;
            }
        }
        {
            // PORT: perf. One borrow of `r` for both tests.
            let mut rb = r.borrow_mut();
            if rb.relation_count <= 0 {
                rb.overflow = true;
                return Ternary::FALSE;
            }
            // If source and target are already being compared, consider them related with assumptions
            // PORT: perf. A linear scan while the stack is small (see
            // `Relater::maybe_keys_contain`).
            if rb.maybe_keys_contain(&id) {
                return Ternary::MAYBE;
            }
        }
        // A constrained key indicates that we have type references that reference constrained
        // type parameters. For such keys we also check against the key we would have gotten if all type parameters
        // were unconstrained.
        if constrained {
            let (broadest_equivalent_id, _) = self.get_relation_key(
                source,
                target,
                intersection_state,
                is_identity,
                true, /*ignoreConstraints*/
            );
            if r.borrow().maybe_keys_contain(&broadest_equivalent_id) {
                return Ternary::MAYBE;
            }
        }
        // PORT: one borrow for the stack pushes. The borrow is held across
        // isDeeplyNestedType, which cannot reach `r` (it is not in the
        // relater pool while in use).
        let (maybe_start, save_expanding_flags, expanding_flags) = {
            let mut guard = r.borrow_mut();
            // Reborrow so the stack and its ids can be borrowed apart.
            let rb = &mut *guard;
            if rb.source_stack.len() == 100 || rb.target_stack.len() == 100 {
                // We stop relating if we reach 100 levels of nesting. This is a backstop to catch infinite recursion
                // that wasn't caught by isDeeplyNestedType. It will also stop relating types that truly are over 100
                // levels deep, but those are exceedingly rare.
                return Ternary::MAYBE;
            }
            let maybe_start = rb.maybe_keys.len() as i32;
            rb.push_maybe_key(id);
            let save_expanding_flags = rb.expanding_flags;
            if recursion_flags.intersects(RecursionFlags::SOURCE) {
                rb.source_stack.push(source);
                crate::checker::relater_p1::bunrcount::site(1); // PROBE bunrcount1
                if !rb.expanding_flags.intersects(ExpandingFlags::SOURCE)
                    && self.is_deeply_nested_relater_type(
                        source,
                        &rb.source_stack,
                        &mut rb.source_ids,
                        3,
                    )
                {
                    rb.expanding_flags |= ExpandingFlags::SOURCE;
                }
            }
            if recursion_flags.intersects(RecursionFlags::TARGET) {
                rb.target_stack.push(target);
                crate::checker::relater_p1::bunrcount::site(2); // PROBE bunrcount1
                if !rb.expanding_flags.intersects(ExpandingFlags::TARGET)
                    && self.is_deeply_nested_relater_type(
                        target,
                        &rb.target_stack,
                        &mut rb.target_ids,
                        3,
                    )
                {
                    rb.expanding_flags |= ExpandingFlags::TARGET;
                }
            }
            (maybe_start, save_expanding_flags, rb.expanding_flags)
        };
        let save_reliability_flags = self.reliability_flags;
        self.reliability_flags = RelationComparisonResult::NONE;
        // Go `defer tr.Push(...)()` in the else branch: the event ends when
        // the function returns.
        let mut _trace: Option<crate::tracing::Pop> = None;
        let result = if expanding_flags == ExpandingFlags::BOTH {
            if let Some(tr) = self.tracer {
                let (depth, target_depth) = {
                    let rb = r.borrow();
                    (rb.source_stack.len(), rb.target_stack.len())
                };
                tr.instant(
                    crate::tracing::Phase::CheckTypes,
                    "recursiveTypeRelatedTo_DepthLimit",
                    vec![
                        ("sourceId", source.into()),
                        ("targetId", target.into()),
                        ("depth", depth.into()),
                        ("targetDepth", target_depth.into()),
                    ],
                );
            }
            Ternary::MAYBE
        } else {
            _trace = self.tracer.map(|tr| {
                tr.push(
                    crate::tracing::Phase::CheckTypes,
                    "structuredTypeRelatedTo",
                    vec![("sourceId", source.into()), ("targetId", target.into())],
                    false,
                )
            });
            self.structured_type_related_to(r, source, target, report_errors, intersection_state)
        };
        let propagating_variance_flags = self.reliability_flags;
        self.reliability_flags |= save_reliability_flags;
        let at_depth_zero = {
            let mut guard = r.borrow_mut();
            let rb = &mut *guard;
            if recursion_flags.intersects(RecursionFlags::SOURCE) {
                rb.source_stack.pop();
                rb.source_ids.truncate(rb.source_stack.len());
            }
            if recursion_flags.intersects(RecursionFlags::TARGET) {
                rb.target_stack.pop();
                rb.target_ids.truncate(rb.target_stack.len());
            }
            rb.expanding_flags = save_expanding_flags;
            rb.source_stack.is_empty() && rb.target_stack.is_empty()
        };
        if result != Ternary::FALSE {
            if result == Ternary::TRUE || at_depth_zero {
                if result == Ternary::TRUE || result == Ternary::MAYBE {
                    // If result is definitely true, record all maybe keys as having succeeded. Also, record Ternary.Maybe
                    // results as having succeeded once we reach depth 0, but never record Ternary.Unknown results.
                    self.reset_maybe_stack(r, maybe_start, propagating_variance_flags, true);
                } else {
                    self.reset_maybe_stack(r, maybe_start, propagating_variance_flags, false);
                }
            }
            // Note: it's intentional that we don't reset in the else case;
            // we leave them on the stack such that when we hit depth zero
            // above, we can report all of them as successful.
        } else {
            // A false result goes straight into global cache (when something is false under
            // assumptions it will also be false without assumptions)
            let mut rb = r.borrow_mut();
            self.relation_of(rb.kind).borrow_mut().set(
                id,
                RelationComparisonResult::FAILED | propagating_variance_flags,
            );
            rb.relation_count -= 1;
            drop(rb);
            self.reset_maybe_stack(r, maybe_start, propagating_variance_flags, false);
        }
        result
    }

    // Go: checker/relater.go:3202 resetMaybeStack
    pub fn reset_maybe_stack(
        &mut self,
        r: &Rc<RefCell<Relater>>,
        maybe_start: i32,
        propagating_variance_flags: RelationComparisonResult,
        mark_all_as_succeeded: bool,
    ) {
        let mut guard = r.borrow_mut();
        // Reborrow so the fields can be borrowed apart.
        let rb = &mut *guard;
        let maybe_start = maybe_start as usize;
        if mark_all_as_succeeded {
            let mut relation = self.relation_of(rb.kind).borrow_mut();
            for &key in &rb.maybe_keys[maybe_start..] {
                relation.set(
                    key,
                    RelationComparisonResult::SUCCEEDED | propagating_variance_flags,
                );
                rb.relation_count -= 1;
            }
        }
        // PORT: perf. The set deletes happen here, in one step when the
        // stack becomes small (see `Relater::truncate_maybe_keys`).
        rb.truncate_maybe_keys(maybe_start);
    }

    // Go: checker/relater.go:3213 getErrorState
    // PERF: both fields are shared (`RelatedInfo`), so the state and each
    // copy of it copy two pointers, as Go copies a chain pointer and a slice
    // header. Before, each state copied and dropped a `Vec<Diagnostic>`.
    #[inline]
    pub fn get_error_state(&self, r: &Rc<RefCell<Relater>>) -> ErrorState {
        let rb = r.borrow();
        ErrorState {
            error_chain: rb.error_chain.clone(),
            related_info: rb.related_info.clone(),
        }
    }

    // Go: checker/relater.go:3220 restoreErrorState
    // PORT: Go passes `errorState` by value. Callers that reuse a saved state
    // pass a clone.
    pub fn restore_error_state(&mut self, r: &Rc<RefCell<Relater>>, e: ErrorState) {
        let mut rb = r.borrow_mut();
        rb.error_chain = e.error_chain;
        rb.related_info = e.related_info;
    }

    // Go: checker/relater.go:3225 structuredTypeRelatedTo
    pub fn structured_type_related_to(
        &mut self,
        r: &Rc<RefCell<Relater>>,
        source: TypeId,
        target: TypeId,
        report_errors: bool,
        intersection_state: IntersectionState,
    ) -> Ternary {
        let save_error_state = self.get_error_state(r);
        let mut result = self.structured_type_related_to_worker(
            r,
            source,
            target,
            report_errors,
            intersection_state,
        );
        if !relation_is(r, RelationKind::Identity) {
            // The combined constraint of an intersection type is the intersection of the constraints of
            // the constituents. When an intersection type contains instantiable types with union type
            // constraints, there are situations where we need to examine the combined constraint. One is
            // when the target is a union type. Another is when the intersection contains types belonging
            // to one of the disjoint domains. For example, given type variables T and U, each with the
            // constraint 'string | number', the combined constraint of 'T & U' is 'string | number' and
            // we need to check this constraint against a union on the target side. Also, given a type
            // variable V constrained to 'string | number', 'V & number' has a combined constraint of
            // 'string & number | number & number' which reduces to just 'number'.
            // This also handles type parameters, as a type parameter with a union constraint compared against a union
            // needs to have its constraint hoisted into an intersection with said type parameter, this way
            // the type param can be compared with itself in the target (with the influence of its constraint to match other parts)
            // For example, if `T extends 1 | 2` and `U extends 2 | 3` and we compare `T & U` to `T & U & (1 | 2 | 3)`
            let source_flags = self.ty(source).flags;
            let target_flags = self.ty(target).flags;
            if result == Ternary::FALSE
                && (source_flags.intersects(TypeFlags::INTERSECTION)
                    || source_flags.intersects(TypeFlags::TYPE_PARAMETER)
                        && target_flags.intersects(TypeFlags::UNION))
            {
                let source_types: SharedList<TypeId> =
                    if source_flags.intersects(TypeFlags::INTERSECTION) {
                        self.ty(source).types_list()
                    } else {
                        vec![source].into()
                    };
                let constraint = self.get_effective_constraint_of_intersection(
                    &source_types,
                    target_flags.intersects(TypeFlags::UNION),
                );
                if constraint.is_some()
                    && self.every_type(constraint, &mut |_c: &mut Checker, c: TypeId| c != source)
                {
                    // TODO: Stack errors so we get a pyramid for the "normal" comparison above, _and_ a second for this
                    result = self.is_related_to_ex(
                        r,
                        constraint,
                        target,
                        RecursionFlags::SOURCE,
                        false, /*reportErrors*/
                        None,  /*headMessage*/
                        intersection_state,
                    );
                }
            }
            // When the target is an intersection we need an extra property check in order to detect nested excess
            // properties and nested weak types. The following are motivating examples that all should be errors, but
            // aren't without this extra property check:
            //
            //   let obj: { a: { x: string } } & { c: number } = { a: { x: 'hello', y: 2 }, c: 5 };  // Nested excess property
            //
            //   declare let wrong: { a: { y: string } };
            //   let weak: { a?: { x?: number } } & { c?: string } = wrong;  // Nested weak object type
            //
            if result != Ternary::FALSE
                && !intersection_state.intersects(IntersectionState::TARGET)
                && target_flags.intersects(TypeFlags::INTERSECTION)
                && !self.is_generic_object_type(target)
                && source_flags.intersects(TypeFlags::OBJECT | TypeFlags::INTERSECTION)
            {
                result &= self.properties_related_to(
                    r,
                    source,
                    target,
                    report_errors,
                    &FxHashSet::default(), /*excludedProperties*/
                    false,                 /*optionalsOnly*/
                    IntersectionState::NONE,
                );
                if result != Ternary::FALSE
                    && self.is_object_literal_type(source)
                    && self
                        .ty(source)
                        .object_flags
                        .intersects(ObjectFlags::FRESH_LITERAL)
                {
                    result &= self.index_signatures_related_to(
                        r,
                        source,
                        target,
                        false, /*sourceIsPrimitive*/
                        report_errors,
                        IntersectionState::NONE,
                    );
                }
            // When the source is an intersection we need an extra check of any optional properties in the target to
            // detect possible mismatched property types. For example:
            //
            //   function foo<T extends object>(x: { a?: string }, y: T & { a: boolean }) {
            //     x = y;  // Mismatched property in source intersection
            //   }
            //
            } else if result != Ternary::FALSE
                && self.is_non_generic_object_type(target)
                && !self.is_array_or_tuple_type(target)
                && self.is_source_intersection_needing_extra_check(r, source, target)
            {
                result &= self.properties_related_to(
                    r,
                    source,
                    target,
                    report_errors,
                    &FxHashSet::default(), /*excludedProperties*/
                    true,                  /*optionalsOnly*/
                    intersection_state,
                );
            }
        }
        if result != Ternary::FALSE {
            self.restore_error_state(r, save_error_state);
        }
        result
    }

    // Go: checker/relater.go:3287 isSourceIntersectionNeedingExtraCheck
    pub fn is_source_intersection_needing_extra_check(
        &mut self,
        r: &Rc<RefCell<Relater>>,
        source: TypeId,
        target: TypeId,
    ) -> bool {
        if !self.ty(source).flags.intersects(TypeFlags::INTERSECTION) {
            return false;
        }
        let apparent = self.get_apparent_type(source);
        self.ty(apparent)
            .flags
            .intersects(TypeFlags::STRUCTURED_TYPE)
            && !self.ty(source).types().iter().any(|&t| {
                t == target
                    || self
                        .ty(t)
                        .object_flags
                        .intersects(ObjectFlags::NON_INFERRABLE_TYPE)
            })
    }

    // Go: checker/relater.go:3294 structuredTypeRelatedToWorker
    pub fn structured_type_related_to_worker(
        &mut self,
        r: &Rc<RefCell<Relater>>,
        source: TypeId,
        target: TypeId,
        report_errors: bool,
        intersection_state: IntersectionState,
    ) -> Ternary {
        let mut source = source;
        let mut result = Ternary::FALSE;
        let mut variance_check_failed = false;
        let mut original_error_chain: Option<Rc<ErrorChain>> = None;
        let save_error_state = self.get_error_state(r);
        // PORT: the Go closure `relateVariances` is the free fn `relate_variances`.
        let is_identity = relation_is(r, RelationKind::Identity);
        let source_flags = self.ty(source).flags;
        let target_flags = self.ty(target).flags;
        if is_identity {
            // We've already checked that source.flags and target.flags are identical
            if source_flags.intersects(TypeFlags::UNION_OR_INTERSECTION) {
                let mut result = self.each_type_related_to_some_type(r, source, target);
                if result != Ternary::FALSE {
                    result &= self.each_type_related_to_some_type(r, target, source);
                }
                return result;
            } else if source_flags.intersects(TypeFlags::INDEX) {
                let s = self.ty(source).target();
                let t = self.ty(target).target();
                return self.is_related_to(
                    r,
                    s,
                    t,
                    RecursionFlags::BOTH,
                    false, /*reportErrors*/
                );
            } else if source_flags.intersects(TypeFlags::INDEXED_ACCESS) {
                let (s_obj, s_idx) = {
                    let d = self.ty(source).as_indexed_access_type();
                    (d.object_type, d.index_type)
                };
                let (t_obj, t_idx) = {
                    let d = self.ty(target).as_indexed_access_type();
                    (d.object_type, d.index_type)
                };
                result = self.is_related_to(
                    r,
                    s_obj,
                    t_obj,
                    RecursionFlags::BOTH,
                    false, /*reportErrors*/
                );
                if result != Ternary::FALSE {
                    result &= self.is_related_to(
                        r,
                        s_idx,
                        t_idx,
                        RecursionFlags::BOTH,
                        false, /*reportErrors*/
                    );
                    if result != Ternary::FALSE {
                        return result;
                    }
                }
            } else if source_flags.intersects(TypeFlags::CONDITIONAL) {
                let (s_dist, s_check, s_extends) = {
                    let d = self.ty(source).as_conditional_type();
                    (
                        d.root.borrow().is_distributive,
                        d.check_type,
                        d.extends_type,
                    )
                };
                let (t_dist, t_check, t_extends) = {
                    let d = self.ty(target).as_conditional_type();
                    (
                        d.root.borrow().is_distributive,
                        d.check_type,
                        d.extends_type,
                    )
                };
                if s_dist == t_dist {
                    result = self.is_related_to(
                        r,
                        s_check,
                        t_check,
                        RecursionFlags::BOTH,
                        false, /*reportErrors*/
                    );
                    if result != Ternary::FALSE {
                        result &= self.is_related_to(
                            r,
                            s_extends,
                            t_extends,
                            RecursionFlags::BOTH,
                            false, /*reportErrors*/
                        );
                        if result != Ternary::FALSE {
                            let s_true = self.get_true_type_from_conditional_type(source);
                            let t_true = self.get_true_type_from_conditional_type(target);
                            result &= self.is_related_to(
                                r,
                                s_true,
                                t_true,
                                RecursionFlags::BOTH,
                                false, /*reportErrors*/
                            );
                            if result != Ternary::FALSE {
                                let s_false = self.get_false_type_from_conditional_type(source);
                                let t_false = self.get_false_type_from_conditional_type(target);
                                result &= self.is_related_to(
                                    r,
                                    s_false,
                                    t_false,
                                    RecursionFlags::BOTH,
                                    false, /*reportErrors*/
                                );
                                if result != Ternary::FALSE {
                                    return result;
                                }
                            }
                        }
                    }
                }
            } else if source_flags.intersects(TypeFlags::SUBSTITUTION) {
                let (s_base, s_constraint) = {
                    let d = self.ty(source).as_substitution_type();
                    (d.base_type, d.constraint)
                };
                let (t_base, t_constraint) = {
                    let d = self.ty(target).as_substitution_type();
                    (d.base_type, d.constraint)
                };
                result = self.is_related_to(
                    r,
                    s_base,
                    t_base,
                    RecursionFlags::BOTH,
                    false, /*reportErrors*/
                );
                if result != Ternary::FALSE {
                    result &= self.is_related_to(
                        r,
                        s_constraint,
                        t_constraint,
                        RecursionFlags::BOTH,
                        false, /*reportErrors*/
                    );
                    if result != Ternary::FALSE {
                        return result;
                    }
                }
            } else if source_flags.intersects(TypeFlags::TEMPLATE_LITERAL) {
                let (s_texts, s_types) = {
                    let d = self.ty(source).as_template_literal_type();
                    (d.texts.clone(), d.types.clone())
                };
                let (t_texts, t_types) = {
                    let d = self.ty(target).as_template_literal_type();
                    (d.texts.clone(), d.types.clone())
                };
                if s_texts == t_texts {
                    result = Ternary::TRUE;
                    for (i, &source_type) in s_types.iter().enumerate() {
                        let target_type = t_types[i];
                        result &= self.is_related_to(
                            r,
                            source_type,
                            target_type,
                            RecursionFlags::BOTH,
                            false, /*reportErrors*/
                        );
                        if result == Ternary::FALSE {
                            return result;
                        }
                    }
                    return result;
                }
            } else if source_flags.intersects(TypeFlags::STRING_MAPPING) {
                if self.ty(source).symbol == self.ty(target).symbol {
                    let s = self.ty(source).as_string_mapping_type().target;
                    let t = self.ty(target).as_string_mapping_type().target;
                    return self.is_related_to(
                        r,
                        s,
                        t,
                        RecursionFlags::BOTH,
                        false, /*reportErrors*/
                    );
                }
            }
            if !source_flags.intersects(TypeFlags::OBJECT) {
                return Ternary::FALSE;
            }
        } else if source_flags.intersects(TypeFlags::UNION_OR_INTERSECTION)
            || target_flags.intersects(TypeFlags::UNION_OR_INTERSECTION)
        {
            result = self.union_or_intersection_related_to(
                r,
                source,
                target,
                report_errors,
                intersection_state,
            );
            if result != Ternary::FALSE {
                return result;
            }
            // The ordered decomposition above doesn't handle all cases. Specifically, we also need to handle:
            // Source is instantiable (e.g. source has union or intersection constraint).
            // Source is an object, target is a union (e.g. { a, b: boolean } <=> { a, b: true } | { a, b: false }).
            // Source is an intersection, target is an object (e.g. { a } & { b } <=> { a, b }).
            // Source is an intersection, target is a union (e.g. { a } & { b: boolean } <=> { a, b: true } | { a, b: false }).
            // Source is an intersection, target instantiable (e.g. string & { tag } <=> T["a"] constrained to string & { tag }).
            if !(source_flags.intersects(TypeFlags::INSTANTIABLE)
                || source_flags.intersects(TypeFlags::OBJECT)
                    && target_flags.intersects(TypeFlags::UNION)
                || source_flags.intersects(TypeFlags::INTERSECTION)
                    && target_flags
                        .intersects(TypeFlags::OBJECT | TypeFlags::UNION | TypeFlags::INSTANTIABLE))
            {
                return Ternary::FALSE;
            }
        }
        // We limit alias variance probing to only object and conditional types since their alias behavior
        // is more predictable than other, interned types, which may or may not have an alias depending on
        // the order in which things were checked.
        let source_alias = self.ty(source).alias.clone();
        let target_alias = self.ty(target).alias.clone();
        if let (Some(sa), Some(ta)) = (&source_alias, &target_alias) {
            if source_flags.intersects(TypeFlags::OBJECT | TypeFlags::CONDITIONAL)
                && !sa.type_arguments.is_empty()
                && sa.symbol == ta.symbol
                && !(self.is_marker_type(source) || self.is_marker_type(target))
            {
                let variances = self.get_alias_variances(sa.symbol);
                if variances.is_empty() {
                    return Ternary::UNKNOWN;
                }
                let params = self.type_alias_links.get(sa.symbol).type_parameters.clone();
                let min_params = self.get_min_type_argument_count(&params);
                let node_is_in_js_file = is_in_js_file(self.sym(sa.symbol).value_declaration);
                let source_types = self.fill_missing_type_arguments(
                    &sa.type_arguments,
                    &params,
                    min_params,
                    node_is_in_js_file,
                );
                let target_types = self.fill_missing_type_arguments(
                    &ta.type_arguments,
                    &params,
                    min_params,
                    node_is_in_js_file,
                );
                let (variance_result, ok) = relate_variances(
                    self,
                    r,
                    &source_types,
                    &target_types,
                    &variances,
                    intersection_state,
                    report_errors,
                    &save_error_state,
                    &mut result,
                    &mut variance_check_failed,
                    &mut original_error_chain,
                );
                if ok {
                    return variance_result;
                }
            }
        }
        // For a generic type T and a type U that is assignable to T, [...U] is assignable to T, U is assignable to readonly [...T],
        // and U is assignable to [...T] when U is constrained to a mutable array or tuple type.
        if self.is_single_element_generic_tuple_type(source)
            && !self.target_tuple_type(source).readonly
        {
            let first = self.type_arguments_of(source)[0];
            result = self.is_related_to(
                r,
                first,
                target,
                RecursionFlags::SOURCE,
                false, /*reportErrors*/
            );
            if result != Ternary::FALSE {
                return result;
            }
        }
        if self.is_single_element_generic_tuple_type(target) && {
            let readonly = self.target_tuple_type(target).readonly;
            readonly || {
                let base = self.get_base_constraint_or_type(source);
                self.is_mutable_array_or_tuple(base)
            }
        } {
            let first = self.type_arguments_of(target)[0];
            result = self.is_related_to(
                r,
                source,
                first,
                RecursionFlags::TARGET,
                false, /*reportErrors*/
            );
            if result != Ternary::FALSE {
                return result;
            }
        }
        if target_flags.intersects(TypeFlags::TYPE_PARAMETER) {
            // A source type { [P in Q]: X } is related to a target type T if keyof T is related to Q and X is related to T[Q].
            if self.ty(source).object_flags.intersects(ObjectFlags::MAPPED)
                && self
                    .ty(source)
                    .as_mapped_type()
                    .declaration()
                    .name_type()
                    .is_nil()
                && {
                    let index_type = self.get_index_type(target);
                    let constraint_type = self.get_constraint_type_from_mapped_type(source);
                    self.is_related_to(r, index_type, constraint_type, RecursionFlags::BOTH, false)
                        != Ternary::FALSE
                }
            {
                if !self
                    .get_mapped_type_modifiers(source)
                    .intersects(MappedTypeModifiers::INCLUDE_OPTIONAL)
                {
                    let template_type = self.get_template_type_from_mapped_type(source);
                    let type_parameter = self.get_type_parameter_from_mapped_type(source);
                    let indexed_access_type = self.get_indexed_access_type(target, type_parameter);
                    result = self.is_related_to(
                        r,
                        template_type,
                        indexed_access_type,
                        RecursionFlags::BOTH,
                        report_errors,
                    );
                    if result != Ternary::FALSE {
                        return result;
                    }
                }
            }
            if relation_is(r, RelationKind::Comparable)
                && source_flags.intersects(TypeFlags::TYPE_PARAMETER)
            {
                // This is a carve-out in comparability to essentially forbid comparing a type parameter with another type parameter
                // unless one extends the other. (Remember: comparability is mostly bidirectional!)
                let constraint = self.get_constraint_of_type_parameter(source);
                if constraint.is_some()
                    && self.some_type(constraint, &mut |c: &mut Checker, t: TypeId| {
                        c.ty(t).flags.intersects(TypeFlags::TYPE_PARAMETER)
                    })
                {
                    return self.is_related_to(
                        r,
                        constraint,
                        target,
                        RecursionFlags::SOURCE,
                        false, /*reportErrors*/
                    );
                }
                return Ternary::FALSE;
            }
        } else if target_flags.intersects(TypeFlags::INDEXED_ACCESS) {
            if source_flags.intersects(TypeFlags::INDEXED_ACCESS) {
                // Relate components directly before falling back to constraint relationships
                // A type S[K] is related to a type T[J] if S is related to T and K is related to J.
                let (s_obj, s_idx) = {
                    let d = self.ty(source).as_indexed_access_type();
                    (d.object_type, d.index_type)
                };
                let (t_obj, t_idx) = {
                    let d = self.ty(target).as_indexed_access_type();
                    (d.object_type, d.index_type)
                };
                result = self.is_related_to(r, s_obj, t_obj, RecursionFlags::BOTH, report_errors);
                if result != Ternary::FALSE {
                    result &=
                        self.is_related_to(r, s_idx, t_idx, RecursionFlags::BOTH, report_errors);
                }
                if result != Ternary::FALSE {
                    return result;
                }
                if report_errors {
                    original_error_chain = r.borrow().error_chain.clone();
                }
            }
            // A type S is related to a type T[K] if S is related to C, where C is the base
            // constraint of T[K] for writing.
            if relation_is(r, RelationKind::Assignable) || relation_is(r, RelationKind::Comparable)
            {
                let (object_type, index_type) = {
                    let d = self.ty(target).as_indexed_access_type();
                    (d.object_type, d.index_type)
                };
                let base_object_type = self.get_base_constraint_or_type(object_type);
                let base_index_type = self.get_base_constraint_or_type(index_type);
                if !self.is_generic_object_type(base_object_type)
                    && !self.is_generic_index_type(base_index_type)
                {
                    let access_flags = AccessFlags::WRITING
                        | if base_object_type != object_type {
                            AccessFlags::NO_INDEX_SIGNATURES
                        } else {
                            AccessFlags::NONE
                        };
                    let constraint = self.get_indexed_access_type_or_undefined(
                        base_object_type,
                        base_index_type,
                        access_flags,
                        Node::NIL,
                        None,
                    );
                    if constraint.is_some() {
                        if report_errors && original_error_chain.is_some() {
                            // create a new chain for the constraint error
                            self.restore_error_state(r, save_error_state.clone());
                        }
                        result = self.is_related_to_ex(
                            r,
                            source,
                            constraint,
                            RecursionFlags::TARGET,
                            report_errors,
                            None, /*headMessage*/
                            intersection_state,
                        );
                        if result != Ternary::FALSE {
                            return result;
                        }
                        // prefer the shorter chain of the constraint comparison chain, and the direct comparison chain
                        let current_chain = r.borrow().error_chain.clone();
                        if report_errors
                            && original_error_chain.is_some()
                            && current_chain.is_some()
                        {
                            if chain_depth(&original_error_chain) <= chain_depth(&current_chain) {
                                r.borrow_mut().error_chain = original_error_chain.clone();
                            }
                        }
                    }
                }
            }
            if report_errors {
                original_error_chain = None;
            }
        } else if target_flags.intersects(TypeFlags::INDEX) {
            let (target_type, target_index_flags) = {
                let d = self.ty(target).as_index_type();
                (d.target, d.index_flags)
            };
            // A keyof S is related to a keyof T if T is related to S.
            if source_flags.intersects(TypeFlags::INDEX) {
                let s_target = self.ty(source).as_index_type().target;
                result = self.is_related_to(
                    r,
                    target_type,
                    s_target,
                    RecursionFlags::BOTH,
                    false, /*reportErrors*/
                );
                if result != Ternary::FALSE {
                    return result;
                }
            }
            if self.is_tuple_type(target_type) {
                // An index type can have a tuple type target when the tuple type contains variadic elements.
                // Check if the source is related to the known keys of the tuple type.
                let known_keys = self.get_known_keys_of_tuple_type(target_type);
                result = self.is_related_to(
                    r,
                    source,
                    known_keys,
                    RecursionFlags::TARGET,
                    report_errors,
                );
                if result != Ternary::FALSE {
                    return result;
                }
            } else {
                // A type S is assignable to keyof T if S is assignable to keyof C, where C is the
                // simplified form of T or, if T doesn't simplify, the constraint of T.
                let constraint = self.get_simplified_type_or_constraint(target_type);
                if constraint.is_some() {
                    // We require Ternary.True here such that circular constraints don't cause
                    // false positives. For example, given 'T extends { [K in keyof T]: string }',
                    // 'keyof T' has itself as its constraint and produces a Ternary.Maybe when
                    // related to other types.
                    let index_type = self.get_index_type_ex(
                        constraint,
                        target_index_flags | IndexFlags::NO_REDUCIBLE_CHECK,
                    );
                    if self.is_related_to(
                        r,
                        source,
                        index_type,
                        RecursionFlags::TARGET,
                        report_errors,
                    ) == Ternary::TRUE
                    {
                        return Ternary::TRUE;
                    }
                } else if self.is_generic_mapped_type(target_type) {
                    // generic mapped types that don't simplify or have a constraint still have a very simple set of keys we can compare against
                    // - their nameType or constraintType.
                    // In many ways, this comparison is a deferred version of what `getIndexTypeForMappedType` does to actually resolve the keys for _non_-generic types
                    let name_type = self.get_name_type_from_mapped_type(target_type);
                    let constraint_type = self.get_constraint_type_from_mapped_type(target_type);
                    let target_keys;
                    if name_type.is_some()
                        && self.is_mapped_type_with_keyof_constraint_declaration(target_type)
                    {
                        // we need to get the apparent mappings and union them with the generic mappings, since some properties may be
                        // missing from the `constraintType` which will otherwise be mapped in the object
                        let mapped_keys =
                            self.get_apparent_mapped_type_keys(name_type, target_type);
                        // We still need to include the non-apparent (and thus still generic) keys in the target side of the comparison (in case they're in the source side)
                        target_keys = self.get_union_type(&[mapped_keys, name_type]);
                    } else if name_type.is_some() {
                        target_keys = name_type;
                    } else {
                        target_keys = constraint_type;
                    }
                    if self.is_related_to(
                        r,
                        source,
                        target_keys,
                        RecursionFlags::TARGET,
                        report_errors,
                    ) == Ternary::TRUE
                    {
                        return Ternary::TRUE;
                    }
                }
            }
        } else if target_flags.intersects(TypeFlags::CONDITIONAL) {
            // If we reach 10 levels of nesting for the same conditional type, assume it is an infinitely expanding recursive
            // conditional type and bail out with a Ternary.Maybe result.
            // PORT: read under a borrow of `r`; nothing reached from
            // isDeeplyNestedType can use `r`. The borrow is mutable because
            // the check can fill the stack's recursion ids.
            let nested = {
                let mut guard = r.borrow_mut();
                let rb = &mut *guard;
                crate::checker::relater_p1::bunrcount::site(3); // PROBE bunrcount1
                self.is_deeply_nested_relater_type(target, &rb.target_stack, &mut rb.target_ids, 10)
            };
            if nested {
                return Ternary::MAYBE;
            }
            let (c_root, c_check_type, c_extends_type) = {
                let c = self.ty(target).as_conditional_type();
                (c.root.clone(), c.check_type, c.extends_type)
            };
            // We check for a relationship to a conditional type target only when the conditional type has no
            // 'infer' positions, is not distributive or is distributive but doesn't reference the check type
            // parameter in either of the result types, and the source isn't an instantiation of the same
            // conditional type (as happens when computing variance).
            // PORT: Go `inferTypeParameters == nil`; an empty Vec stands for nil.
            let no_infer = c_root.borrow().infer_type_parameters.is_empty();
            if no_infer
                && !self.is_distribution_dependent(&c_root)
                && !(source_flags.intersects(TypeFlags::CONDITIONAL)
                    && Rc::ptr_eq(&self.ty(source).as_conditional_type().root, &c_root))
            {
                // Check if the conditional is always true or always false but still deferred for distribution purposes.
                let permissive_check = self.get_permissive_instantiation(c_check_type);
                let permissive_extends = self.get_permissive_instantiation(c_extends_type);
                let skip_true = !self.is_type_assignable_to(permissive_check, permissive_extends);
                let skip_false = !skip_true && {
                    let restrictive_check = self.get_restrictive_instantiation(c_check_type);
                    let restrictive_extends = self.get_restrictive_instantiation(c_extends_type);
                    self.is_type_assignable_to(restrictive_check, restrictive_extends)
                };
                // TODO: Find a nice way to include potential conditional type breakdowns in error output, if they seem good (they usually don't)
                if skip_true {
                    result = Ternary::TRUE;
                } else {
                    let true_type = self.get_true_type_from_conditional_type(target);
                    result = self.is_related_to_ex(
                        r,
                        source,
                        true_type,
                        RecursionFlags::TARGET,
                        false, /*reportErrors*/
                        None,  /*headMessage*/
                        intersection_state,
                    );
                }
                if result != Ternary::FALSE {
                    if skip_false {
                        result &= Ternary::TRUE;
                    } else {
                        let false_type = self.get_false_type_from_conditional_type(target);
                        result &= self.is_related_to_ex(
                            r,
                            source,
                            false_type,
                            RecursionFlags::TARGET,
                            false, /*reportErrors*/
                            None,  /*headMessage*/
                            intersection_state,
                        );
                    }
                    if result != Ternary::FALSE {
                        return result;
                    }
                }
            }
        } else if target_flags.intersects(TypeFlags::TEMPLATE_LITERAL) {
            if source_flags.intersects(TypeFlags::TEMPLATE_LITERAL) {
                if relation_is(r, RelationKind::Comparable) {
                    let unrelated = self.template_literal_types_definitely_unrelated(
                        self.ty(source).as_template_literal_type(),
                        self.ty(target).as_template_literal_type(),
                    );
                    if unrelated {
                        return Ternary::FALSE;
                    }
                    return Ternary::TRUE;
                }
                // Report unreliable variance for type variables referenced in template literal type placeholders.
                // For example, `foo-${number}` is related to `foo-${string}` even though number isn't related to string.
                let report_unreliable_mapper = self.report_unreliable_mapper;
                self.instantiate_type(source, report_unreliable_mapper);
            }
            // PORT: the target data is cloned so the checker is not borrowed during the call.
            // PERF (perffu1): most pairs fail Go's first test, which makes no
            // type, so the clone comes after it.
            if !self.template_literal_match_fails_early(source, target) {
                let target_template = self.ty(target).as_template_literal_type().clone();
                let rr = r.clone();
                if self.is_type_matched_by_template_literal_type(
                    source,
                    &target_template,
                    &mut |c: &mut Checker, s: TypeId, t: TypeId, report_errors: bool| {
                        c.is_related_to_worker(&rr, s, t, report_errors)
                    },
                ) {
                    return Ternary::TRUE;
                }
            }
        } else if target_flags.intersects(TypeFlags::STRING_MAPPING) {
            if !source_flags.intersects(TypeFlags::STRING_MAPPING) {
                if self.is_member_of_string_mapping(source, target) {
                    return Ternary::TRUE;
                }
            }
        } else if self.is_generic_mapped_type(target) && !relation_is(r, RelationKind::Identity) {
            // Check if source type `S` is related to target type `{ [P in Q]: T }` or `{ [P in Q as R]: T}`.
            let keys_remapped = self
                .ty(target)
                .as_mapped_type()
                .declaration()
                .name_type()
                .is_some();
            let template_type = self.get_template_type_from_mapped_type(target);
            let modifiers = self.get_mapped_type_modifiers(target);
            if !modifiers.intersects(MappedTypeModifiers::EXCLUDE_OPTIONAL) {
                // If the mapped type has shape `{ [P in Q]: T[P] }`,
                // source `S` is related to target if `T` = `S`, i.e. `S` is related to `{ [P in Q]: S[P] }`.
                if !keys_remapped
                    && self
                        .ty(template_type)
                        .flags
                        .intersects(TypeFlags::INDEXED_ACCESS)
                {
                    let (tt_obj, tt_idx) = {
                        let d = self.ty(template_type).as_indexed_access_type();
                        (d.object_type, d.index_type)
                    };
                    if tt_obj == source
                        && tt_idx == self.get_type_parameter_from_mapped_type(target)
                    {
                        return Ternary::TRUE;
                    }
                }
                if !self.is_generic_mapped_type(source) {
                    // If target has shape `{ [P in Q as R]: T}`, then its keys have type `R`.
                    // If target has shape `{ [P in Q]: T }`, then its keys have type `Q`.
                    let target_keys = if keys_remapped {
                        self.get_name_type_from_mapped_type(target)
                    } else {
                        self.get_constraint_type_from_mapped_type(target)
                    };
                    // Type of the keys of source type `S`, i.e. `keyof S`.
                    let source_keys =
                        self.get_index_type_ex(source, IndexFlags::NO_INDEX_SIGNATURES);
                    let include_optional =
                        modifiers.intersects(MappedTypeModifiers::INCLUDE_OPTIONAL);
                    let filtered_by_applicability = if include_optional {
                        self.intersect_types(target_keys, source_keys)
                    } else {
                        TypeId::NIL
                    };
                    // A source type `S` is related to a target type `{ [P in Q]: T }` if `Q` is related to `keyof S` and `S[Q]` is related to `T`.
                    // A source type `S` is related to a target type `{ [P in Q as R]: T }` if `R` is related to `keyof S` and `S[R]` is related to `T.
                    // A source type `S` is related to a target type `{ [P in Q]?: T }` if some constituent `Q'` of `Q` is related to `keyof S` and `S[Q']` is related to `T`.
                    // A source type `S` is related to a target type `{ [P in Q as R]?: T }` if some constituent `R'` of `R` is related to `keyof S` and `S[R']` is related to `T`.
                    if include_optional
                        && !self
                            .ty(filtered_by_applicability)
                            .flags
                            .intersects(TypeFlags::NEVER)
                        || !include_optional
                            && self.is_related_to(
                                r,
                                target_keys,
                                source_keys,
                                RecursionFlags::BOTH,
                                false,
                            ) != Ternary::FALSE
                    {
                        let template_type = self.get_template_type_from_mapped_type(target);
                        let type_parameter = self.get_type_parameter_from_mapped_type(target);
                        // Fastpath: When the template type has the form `Obj[P]` where `P` is the mapped type parameter, directly compare source `S` with `Obj`
                        // to avoid creating the (potentially very large) number of new intermediate types made by manufacturing `S[P]`.
                        let non_null_component =
                            self.extract_types_of_kind(template_type, !TypeFlags::NULLABLE);
                        let non_null_indexed = {
                            let nn = self.ty(non_null_component);
                            if nn.flags.intersects(TypeFlags::INDEXED_ACCESS) {
                                let d = nn.as_indexed_access_type();
                                Some((d.object_type, d.index_type))
                            } else {
                                None
                            }
                        };
                        let fast_path_object = match non_null_indexed {
                            Some((nn_obj, nn_idx))
                                if !keys_remapped && nn_idx == type_parameter =>
                            {
                                nn_obj
                            }
                            _ => TypeId::NIL,
                        };
                        if fast_path_object.is_some() {
                            let nn_obj = fast_path_object;
                            result = self.is_related_to(
                                r,
                                source,
                                nn_obj,
                                RecursionFlags::TARGET,
                                report_errors,
                            );
                            if result != Ternary::FALSE {
                                return result;
                            }
                        } else {
                            // We need to compare the type of a property on the source type `S` to the type of the same property on the target type,
                            // so we need to construct an indexing type representing a property, and then use indexing type to index the source type for comparison.
                            // If the target type has shape `{ [P in Q]: T }`, then a property of the target has type `P`.
                            // If the target type has shape `{ [P in Q]?: T }`, then a property of the target has type `P`,
                            // but the property is optional, so we only want to compare properties `P` that are common between `keyof S` and `Q`.
                            // If the target type has shape `{ [P in Q as R]: T }`, then a property of the target has type `R`.
                            // If the target type has shape `{ [P in Q as R]?: T }`, then a property of the target has type `R`,
                            // but the property is optional, so we only want to compare properties `R` that are common between `keyof S` and `R`.
                            let mut indexing_type = type_parameter;
                            if keys_remapped {
                                indexing_type = if filtered_by_applicability.is_some() {
                                    filtered_by_applicability
                                } else {
                                    target_keys
                                };
                            } else if filtered_by_applicability.is_some() {
                                indexing_type = self.get_intersection_type(&[
                                    filtered_by_applicability,
                                    type_parameter,
                                ]);
                            }
                            let indexed_access_type =
                                self.get_indexed_access_type(source, indexing_type);
                            // Compare `S[indexingType]` to `T`, where `T` is the type of a property of the target type.
                            result = self.is_related_to(
                                r,
                                indexed_access_type,
                                template_type,
                                RecursionFlags::BOTH,
                                report_errors,
                            );
                            if result != Ternary::FALSE {
                                return result;
                            }
                        }
                    }
                    original_error_chain = r.borrow().error_chain.clone();
                    self.restore_error_state(r, save_error_state.clone());
                }
            }
        }
        if source_flags.intersects(TypeFlags::TYPE_VARIABLE) {
            // IndexedAccess comparisons are handled above in the `target.flags&TypeFlagsIndexedAccess` branch
            if !source_flags.intersects(TypeFlags::INDEXED_ACCESS)
                || !target_flags.intersects(TypeFlags::INDEXED_ACCESS)
            {
                let mut constraint = self.get_constraint_of_type(source);
                if constraint.is_nil() {
                    constraint = self.unknown_type;
                }
                // hi-speed no-this-instantiation check (less accurate, but avoids costly `this`-instantiation when the constraint will suffice), see #28231 for report on why this is needed
                result = self.is_related_to_ex(
                    r,
                    constraint,
                    target,
                    RecursionFlags::SOURCE,
                    false, /*reportErrors*/
                    None,  /*headMessage*/
                    intersection_state,
                );
                if result != Ternary::FALSE {
                    return result;
                }
                let constraint_with_this = self.get_type_with_this_argument(
                    constraint, source, false, /*needApparentType*/
                );
                result = self.is_related_to_ex(
                    r,
                    constraint_with_this,
                    target,
                    RecursionFlags::SOURCE,
                    report_errors
                        && constraint != self.unknown_type
                        && !(target_flags & source_flags).intersects(TypeFlags::TYPE_PARAMETER),
                    None, /*headMessage*/
                    intersection_state,
                );
                if result != Ternary::FALSE {
                    return result;
                }
                if self.is_mapped_type_generic_indexed_access(source) {
                    // For an indexed access type { [P in K]: E}[X], above we have already explored an instantiation of E with X
                    // substituted for P. We also want to explore type { [P in K]: E }[C], where C is the constraint of X.
                    let (s_obj, s_idx) = {
                        let d = self.ty(source).as_indexed_access_type();
                        (d.object_type, d.index_type)
                    };
                    let index_constraint = self.get_constraint_of_type(s_idx);
                    if index_constraint.is_some() {
                        let indexed = self.get_indexed_access_type(s_obj, index_constraint);
                        result = self.is_related_to(
                            r,
                            indexed,
                            target,
                            RecursionFlags::SOURCE,
                            report_errors,
                        );
                        if result != Ternary::FALSE {
                            return result;
                        }
                    }
                }
            }
        } else if source_flags.intersects(TypeFlags::INDEX) {
            let (s_target, s_index_flags) = {
                let d = self.ty(source).as_index_type();
                (d.target, d.index_flags)
            };
            let is_deferred_mapped_index = self.should_defer_index_type(s_target, s_index_flags)
                && self
                    .ty(s_target)
                    .object_flags
                    .intersects(ObjectFlags::MAPPED);
            let string_number_symbol_type = self.string_number_symbol_type;
            result = self.is_related_to(
                r,
                string_number_symbol_type,
                target,
                RecursionFlags::SOURCE,
                report_errors && !is_deferred_mapped_index,
            );
            if result != Ternary::FALSE {
                return result;
            }
            if is_deferred_mapped_index {
                let mapped_type = s_target;
                let name_type = self.get_name_type_from_mapped_type(mapped_type);
                // Unlike on the target side, on the source side we do *not* include the generic part of the `nameType`, since that comes from a
                // (potentially anonymous) mapped type local type parameter, so that'd never assign outside the mapped type body, but we still want to
                // allow assignments of index types of identical (or similar enough) mapped types.
                // eg, `keyof {[X in keyof A]: Obj[X]}` should be assignable to `keyof {[Y in keyof A]: Tup[Y]}` because both map over the same set of keys (`keyof A`).
                // Without this source-side breakdown, a `keyof {[X in keyof A]: Obj[X]}` style type won't be assignable to anything except itself, which is much too strict.
                let source_mapped_keys = if name_type.is_some()
                    && self.is_mapped_type_with_keyof_constraint_declaration(mapped_type)
                {
                    self.get_apparent_mapped_type_keys(name_type, mapped_type)
                } else if name_type.is_some() {
                    name_type
                } else {
                    self.get_constraint_type_from_mapped_type(mapped_type)
                };
                result = self.is_related_to(
                    r,
                    source_mapped_keys,
                    target,
                    RecursionFlags::SOURCE,
                    report_errors,
                );
                if result != Ternary::FALSE {
                    return result;
                }
            }
        } else if source_flags.intersects(TypeFlags::CONDITIONAL) {
            // If we reach 10 levels of nesting for the same conditional type, assume it is an infinitely expanding recursive
            // conditional type and bail out with a Ternary.Maybe result.
            // PORT: read under a borrow of `r`; nothing reached from
            // isDeeplyNestedType can use `r`. The borrow is mutable because
            // the check can fill the stack's recursion ids.
            let nested = {
                let mut guard = r.borrow_mut();
                let rb = &mut *guard;
                crate::checker::relater_p1::bunrcount::site(4); // PROBE bunrcount1
                self.is_deeply_nested_relater_type(source, &rb.source_stack, &mut rb.source_ids, 10)
            };
            if nested {
                return Ternary::MAYBE;
            }
            if target_flags.intersects(TypeFlags::CONDITIONAL) {
                // Two conditional types 'T1 extends U1 ? X1 : Y1' and 'T2 extends U2 ? X2 : Y2' are related if
                // one of T1 and T2 is related to the other, U1 and U2 are identical types, X1 is related to X2,
                // and Y1 is related to Y2.
                let (source_params, mut source_extends, s_check) = {
                    let d = self.ty(source).as_conditional_type();
                    (
                        d.root.borrow().infer_type_parameters.clone(),
                        d.extends_type,
                        d.check_type,
                    )
                };
                let (t_extends, t_check) = {
                    let d = self.ty(target).as_conditional_type();
                    (d.extends_type, d.check_type)
                };
                let mut mapper = MapperId::NIL;
                if !source_params.is_empty() {
                    // If the source has infer type parameters, we instantiate them in the context of the target
                    let rr = r.clone();
                    let compare_types: TypeComparer = Rc::new(
                        move |c: &mut Checker,
                              s: TypeId,
                              t: TypeId,
                              report_errors: bool|
                              -> Ternary {
                            c.is_related_to_worker(&rr, s, t, report_errors)
                        },
                    );
                    let ctx = self.new_inference_context(
                        &source_params,
                        SignatureId::NIL, /*signature*/
                        InferenceFlags::NONE,
                        Some(compare_types),
                    );
                    self.infer_types(
                        ctx,
                        t_extends,
                        source_extends,
                        InferencePriority::NO_CONSTRAINTS | InferencePriority::ALWAYS_STRICT,
                        false,
                    );
                    let ctx_mapper = self.inference_context(ctx).mapper;
                    source_extends = self.instantiate_type(source_extends, ctx_mapper);
                    mapper = ctx_mapper;
                }
                if self.is_type_identical_to(source_extends, t_extends)
                    && (self.is_related_to(r, s_check, t_check, RecursionFlags::BOTH, false)
                        != Ternary::FALSE
                        || self.is_related_to(r, t_check, s_check, RecursionFlags::BOTH, false)
                            != Ternary::FALSE)
                {
                    let s_true = self.get_true_type_from_conditional_type(source);
                    let s_true = self.instantiate_type(s_true, mapper);
                    let t_true = self.get_true_type_from_conditional_type(target);
                    result =
                        self.is_related_to(r, s_true, t_true, RecursionFlags::BOTH, report_errors);
                    if result != Ternary::FALSE {
                        let s_false = self.get_false_type_from_conditional_type(source);
                        let t_false = self.get_false_type_from_conditional_type(target);
                        result &= self.is_related_to(
                            r,
                            s_false,
                            t_false,
                            RecursionFlags::BOTH,
                            report_errors,
                        );
                    }
                    if result != Ternary::FALSE {
                        return result;
                    }
                }
            }
            // conditionals can be related to one another via normal constraint, as, eg, `A extends B ? O : never` should be assignable to `O`
            // when `O` is a conditional (`never` is trivially assignable to `O`, as is `O`!).
            let default_constraint = self.get_default_constraint_of_conditional_type(source);
            if default_constraint.is_some() {
                result = self.is_related_to(
                    r,
                    default_constraint,
                    target,
                    RecursionFlags::SOURCE,
                    report_errors,
                );
                if result != Ternary::FALSE {
                    return result;
                }
            }
            // conditionals aren't related to one another via distributive constraint as it is much too inaccurate and allows way
            // more assignments than are desirable (since it maps the source check type to its constraint, it loses information).
            if !target_flags.intersects(TypeFlags::CONDITIONAL)
                && self.has_non_circular_base_constraint(source)
            {
                let distributive_constraint =
                    self.get_constraint_of_distributive_conditional_type(source);
                if distributive_constraint.is_some() {
                    self.restore_error_state(r, save_error_state.clone());
                    result = self.is_related_to(
                        r,
                        distributive_constraint,
                        target,
                        RecursionFlags::SOURCE,
                        report_errors,
                    );
                    if result != Ternary::FALSE {
                        return result;
                    }
                }
            }
        } else if source_flags.intersects(TypeFlags::TEMPLATE_LITERAL)
            && !target_flags.intersects(TypeFlags::OBJECT)
        {
            if !target_flags.intersects(TypeFlags::TEMPLATE_LITERAL) {
                let constraint = self.get_base_constraint_of_type(source);
                if constraint.is_some() && constraint != source {
                    result = self.is_related_to(
                        r,
                        constraint,
                        target,
                        RecursionFlags::SOURCE,
                        report_errors,
                    );
                    if result != Ternary::FALSE {
                        return result;
                    }
                }
            }
        } else if source_flags.intersects(TypeFlags::STRING_MAPPING) {
            if target_flags.intersects(TypeFlags::STRING_MAPPING) {
                if self.ty(source).symbol != self.ty(target).symbol {
                    return Ternary::FALSE;
                }
                let s = self.ty(source).as_string_mapping_type().target;
                let t = self.ty(target).as_string_mapping_type().target;
                result = self.is_related_to(r, s, t, RecursionFlags::BOTH, report_errors);
                if result != Ternary::FALSE {
                    return result;
                }
            } else {
                let constraint = self.get_base_constraint_of_type(source);
                if constraint.is_some() {
                    result = self.is_related_to(
                        r,
                        constraint,
                        target,
                        RecursionFlags::SOURCE,
                        report_errors,
                    );
                    if result != Ternary::FALSE {
                        return result;
                    }
                }
            }
        } else {
            // An empty object type is related to any mapped type that includes a '?' modifier.
            if !relation_is(r, RelationKind::Subtype)
                && !relation_is(r, RelationKind::StrictSubtype)
                && self.is_partial_mapped_type(target)
                && self.is_empty_object_type(source)
            {
                return Ternary::TRUE;
            }
            if self.is_generic_mapped_type(target) {
                if self.is_generic_mapped_type(source) {
                    result = self.mapped_type_related_to(r, source, target, report_errors);
                    if result != Ternary::FALSE {
                        return result;
                    }
                }
                return Ternary::FALSE;
            }
            let source_is_primitive = source_flags.intersects(TypeFlags::PRIMITIVE);
            if !relation_is(r, RelationKind::Identity) {
                source = self.get_apparent_type(source);
            } else if self.is_generic_mapped_type(source) {
                return Ternary::FALSE;
            }
            // PORT: `source` may have changed above, so its flags are read again.
            let source_flags = self.ty(source).flags;
            let source_object_flags = self.ty(source).object_flags;
            let target_object_flags = self.ty(target).object_flags;
            if source_object_flags.intersects(ObjectFlags::REFERENCE)
                && target_object_flags.intersects(ObjectFlags::REFERENCE)
                && self.ty(source).target() == self.ty(target).target()
                && !self.is_tuple_type(source)
                && !self.is_marker_type(source)
                && !self.is_marker_type(target)
            {
                // When strictNullChecks is disabled, the element type of the empty array literal is undefinedWideningType,
                // and an empty array literal wouldn't be assignable to a `never[]` without this check.
                if self.is_empty_array_literal_type(source) {
                    return Ternary::TRUE;
                }
                // We have type references to the same generic type, and the type references are not marker
                // type references (which are intended by be compared structurally). Obtain the variance
                // information for the type parameters and relate the type arguments accordingly.
                let source_target = self.ty(source).target();
                let variances = self.get_variances(source_target);
                // We return Ternary.Maybe for a recursive invocation of getVariances (signaled by emptyArray). This
                // effectively means we measure variance only from type parameter occurrences that aren't nested in
                // recursive instantiations of the generic type.
                if variances.is_empty() {
                    return Ternary::UNKNOWN;
                }
                let source_type_arguments = self.get_type_arguments(source);
                let target_type_arguments = self.get_type_arguments(target);
                let (variance_result, ok) = relate_variances(
                    self,
                    r,
                    &source_type_arguments,
                    &target_type_arguments,
                    &variances,
                    intersection_state,
                    report_errors,
                    &save_error_state,
                    &mut result,
                    &mut variance_check_failed,
                    &mut original_error_chain,
                );
                if ok {
                    return variance_result;
                }
            } else if self.is_array_type(target)
                && (self.is_readonly_array_type(target)
                    && self.every_type(source, &mut |c: &mut Checker, t: TypeId| {
                        c.is_array_or_tuple_type(t)
                    })
                    || self.every_type(source, &mut |c: &mut Checker, t: TypeId| {
                        c.is_mutable_tuple_type(t)
                    }))
            {
                if !relation_is(r, RelationKind::Identity) {
                    let number_type = self.number_type;
                    let any_type = self.any_type;
                    let s = self.get_index_type_of_type_ex(source, number_type, any_type);
                    let t = self.get_index_type_of_type_ex(target, number_type, any_type);
                    return self.is_related_to(r, s, t, RecursionFlags::BOTH, report_errors);
                }
                // By flags alone, we know that the `target` is a readonly array while the source is a normal array or tuple
                // or `target` is an array and source is a tuple - in both cases the types cannot be identical, by construction
                return Ternary::FALSE;
            } else if self.is_generic_tuple_type(source)
                && self.is_tuple_type(target)
                && !self.is_generic_tuple_type(target)
            {
                let constraint = self.get_base_constraint_or_type(source);
                if constraint != source {
                    return self.is_related_to(
                        r,
                        constraint,
                        target,
                        RecursionFlags::SOURCE,
                        report_errors,
                    );
                }
            } else if (relation_is(r, RelationKind::Subtype)
                || relation_is(r, RelationKind::StrictSubtype))
                && self.is_empty_object_type(target)
                && target_object_flags.intersects(ObjectFlags::FRESH_LITERAL)
                && !self.is_empty_object_type(source)
            {
                return Ternary::FALSE;
            }
            // Even if relationship doesn't hold for unions, intersections, or generic type references,
            // it may hold in a structural comparison.
            // In a check of the form X = A & B, we will have previously checked if A relates to X or B relates
            // to X. Failing both of those we want to check if the aggregation of A and B's members structurally
            // relates to X. Thus, we include intersection types on the source side here.
            if source_flags.intersects(TypeFlags::OBJECT | TypeFlags::INTERSECTION)
                && target_flags.intersects(TypeFlags::OBJECT)
            {
                // Report structural errors only if we haven't reported any errors yet
                let report_structural_errors = report_errors
                    && error_chain_eq(&r.borrow().error_chain, &save_error_state.error_chain)
                    && !source_is_primitive;
                result = self.properties_related_to(
                    r,
                    source,
                    target,
                    report_structural_errors,
                    &FxHashSet::default(), /*excludedProperties*/
                    false,                 /*optionalsOnly*/
                    intersection_state,
                );
                if result != Ternary::FALSE {
                    result &= self.signatures_related_to(
                        r,
                        source,
                        target,
                        SignatureKind::CALL,
                        report_structural_errors,
                        intersection_state,
                    );
                    if result != Ternary::FALSE {
                        result &= self.signatures_related_to(
                            r,
                            source,
                            target,
                            SignatureKind::CONSTRUCT,
                            report_structural_errors,
                            intersection_state,
                        );
                        if result != Ternary::FALSE {
                            result &= self.index_signatures_related_to(
                                r,
                                source,
                                target,
                                source_is_primitive,
                                report_structural_errors,
                                intersection_state,
                            );
                        }
                    }
                }
                if result != Ternary::FALSE {
                    if !variance_check_failed {
                        return result;
                    }
                    if original_error_chain.is_some() {
                        r.borrow_mut().error_chain = original_error_chain.clone();
                    } else if r.borrow().error_chain.is_none() {
                        r.borrow_mut().error_chain = save_error_state.error_chain.clone();
                    }
                    // Use variance error (there is no structural one) and return false
                }
            }
            // If S is an object type and T is a discriminated union, S may be related to T if
            // there exists a constituent of T for every combination of the discriminants of S
            // with respect to T. We do not report errors here, as we will use the existing
            // error result from checking each constituent of the union.
            if source_flags.intersects(TypeFlags::OBJECT | TypeFlags::INTERSECTION)
                && target_flags.intersects(TypeFlags::UNION)
            {
                let object_only_target = self.extract_types_of_kind(
                    target,
                    TypeFlags::OBJECT | TypeFlags::INTERSECTION | TypeFlags::SUBSTITUTION,
                );
                if self
                    .ty(object_only_target)
                    .flags
                    .intersects(TypeFlags::UNION)
                {
                    let result =
                        self.type_related_to_discriminated_type(r, source, object_only_target);
                    if result != Ternary::FALSE {
                        return result;
                    }
                }
            }
        }
        Ternary::FALSE
    }
}

#[cfg(test)]
mod union_subset_tests {
    use super::*;
    use crate::checker::utilities_p1::union_sort_tests::with_alias_types;

    /// `is_type_subset_of_union` gives Go's answer (each source member
    /// searched in the target) on first and repeat calls, and keeps only
    /// the answers of long searches. The first calls from `Huge` or `Most`
    /// (300 and 260 types) to a target of 20 types or more search on
    /// entries (`contains_types_by_entries`).
    #[test]
    fn kept_subset_answers_are_go_answers() {
        let literals = |r: std::ops::Range<usize>| {
            r.map(|i| format!("\"a{i}\""))
                .collect::<Vec<_>>()
                .join(" | ")
        };
        let numbers =
            |r: std::ops::Range<usize>| r.map(|i| i.to_string()).collect::<Vec<_>>().join(" | ");
        let source = format!(
            "type Big = {};\ntype Half = {};\ntype Other = {} | \"zz\";\ntype Few = \"a1\" | \"a2\";\n\
             type Obj = {{ x: 1 }} | {{ y: 2 }} | {};\ntype Huge = {} | {};\ntype Most = {} | {};\n",
            literals(0..40),
            literals(0..20),
            literals(5..25),
            literals(0..12),
            literals(0..240),
            numbers(0..60),
            literals(0..200),
            numbers(0..60),
        );
        with_alias_types(&source, |c, types| {
            let unions: Vec<TypeId> = types
                .iter()
                .copied()
                .filter(|&t| c.ty(t).flags.intersects(TypeFlags::UNION))
                .collect();
            let [big, half, other, few, obj, huge, most] = unions[..] else {
                panic!("{} unions", unions.len());
            };
            for round in 0..2 {
                for &s in &unions {
                    for &t in &unions {
                        let targets = c.ty(t).types();
                        let go = c
                            .ty(s)
                            .types()
                            .iter()
                            .all(|&m| c.search_union_types(targets, m).1);
                        assert_eq!(c.is_type_subset_of_union(s, t), go, "round {round}");
                    }
                }
            }
            let kept = |s, t| c.union_subset_answers.get(&(s, t)).copied();
            assert_eq!(kept(half, big), Some(true));
            assert_eq!(kept(other, big), Some(false));
            assert_eq!(kept(big, obj), Some(false));
            assert_eq!(kept(obj, big), Some(false));
            assert_eq!(kept(most, huge), Some(true));
            assert_eq!(kept(huge, most), Some(false));
            assert_eq!(kept(most, half), Some(false));
            // 2 searches of 6 steps: not kept.
            assert_eq!(kept(few, big), None);
            assert!(c.is_type_subset_of_union(few, big));
        });
    }
}
