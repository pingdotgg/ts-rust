use crate::prelude::*;

use crate::diagnostics::Message;

// PORT: Go methods on `*Relater` are `impl Checker` methods with the Go snake
// name that take the relater handle `r: &Rc<RefCell<Relater>>` right after
// `self`. The relater is shared (`Checker::free_relater` pools it) and Go
// passes `r.reportError` and `r.isRelatedTo` as func values into
// `compareSignaturesRelated` (a 'static `TypeComparer`), so each borrow of the
// relater is kept short and never held across a checker call. Go `r.c` is
// `self`. Go `r.relation == r.c.xxxRelation` compares the relater's
// `RelationKind`.
// PORT: Go `*ErrorChain` is `Option<Rc<ErrorChain>>` (the chain is shared by
// saved error states). `ErrorChain { next, message, args: Vec<String> }`.

// PORT: Go compares `*diagnostics.Message` pointers. `diag::X` are statics, so
// pointer equality is exact.
fn msg_eq(m: Option<&'static Message>, target: &'static Message) -> bool {
    m.is_some_and(|m| std::ptr::eq(m, target))
}

impl Checker {
    // Go: checker/relater.go:3935 typeArgumentsRelatedTo
    pub fn type_arguments_related_to(
        &mut self,
        r: &Rc<RefCell<Relater>>,
        sources: &[TypeId],
        targets: &[TypeId],
        variances: &[VarianceFlags],
        report_errors: bool,
        intersection_state: IntersectionState,
    ) -> Ternary {
        let relation = r.borrow().kind;
        if sources.len() != targets.len() && relation == RelationKind::Identity {
            return Ternary::FALSE;
        }
        let length = sources.len().min(targets.len());
        let mut result = Ternary::TRUE;
        for i in 0..length {
            // When variance information isn't available we default to covariance. This happens
            // in the process of computing variance information for recursive types and when
            // comparing 'this' type arguments.
            let mut variance_flags = VarianceFlags::COVARIANT;
            if i < variances.len() {
                variance_flags = variances[i];
            }
            let variance = variance_flags & VarianceFlags::VARIANCE_MASK;
            // We ignore arguments for independent type parameters (because they're never witnessed).
            if variance != VarianceFlags::INDEPENDENT {
                let s = sources[i];
                let t = targets[i];
                let mut related: Ternary;
                if variance_flags.intersects(VarianceFlags::UNMEASURABLE) {
                    // Even an `Unmeasurable` variance works out without a structural check if the source and target are _identical_.
                    // We can't simply assume invariance, because `Unmeasurable` marks nonlinear relations, for example, a relation tainted by
                    // the `-?` modifier in a mapped type (where, no matter how the inputs are related, the outputs still might not be)
                    if relation == RelationKind::Identity {
                        related = self.is_related_to(
                            r,
                            s,
                            t,
                            RecursionFlags::BOTH,
                            false, /*reportErrors*/
                        );
                    } else {
                        related = self.compare_types_identical(s, t);
                    }
                } else {
                    // Propagate unreliable variance flag in variance computations
                    if !self.variance_stack.is_empty()
                        && variance_flags.intersects(VarianceFlags::UNRELIABLE)
                    {
                        let m = self.report_unreliable_mapper;
                        self.instantiate_type(s, m);
                    }
                    if variance == VarianceFlags::COVARIANT {
                        related = self.is_related_to_ex(
                            r,
                            s,
                            t,
                            RecursionFlags::BOTH,
                            report_errors,
                            None, /*headMessage*/
                            intersection_state,
                        );
                    } else if variance == VarianceFlags::CONTRAVARIANT {
                        related = self.is_related_to_ex(
                            r,
                            t,
                            s,
                            RecursionFlags::BOTH,
                            report_errors,
                            None, /*headMessage*/
                            intersection_state,
                        );
                    } else if variance == VarianceFlags::BIVARIANT {
                        // In the bivariant case we first compare contravariantly without reporting
                        // errors. Then, if that doesn't succeed, we compare covariantly with error
                        // reporting. Thus, error elaboration will be based on the covariant check,
                        // which is generally easier to reason about.
                        related = self.is_related_to(
                            r,
                            t,
                            s,
                            RecursionFlags::BOTH,
                            false, /*reportErrors*/
                        );
                        if related == Ternary::FALSE {
                            related = self.is_related_to_ex(
                                r,
                                s,
                                t,
                                RecursionFlags::BOTH,
                                report_errors,
                                None, /*headMessage*/
                                intersection_state,
                            );
                        }
                    } else {
                        // In the invariant case we first compare covariantly, and only when that
                        // succeeds do we proceed to compare contravariantly. Thus, error elaboration
                        // will typically be based on the covariant check.
                        related = self.is_related_to_ex(
                            r,
                            s,
                            t,
                            RecursionFlags::BOTH,
                            report_errors,
                            None, /*headMessage*/
                            intersection_state,
                        );
                        if related != Ternary::FALSE {
                            related &= self.is_related_to_ex(
                                r,
                                t,
                                s,
                                RecursionFlags::BOTH,
                                report_errors,
                                None, /*headMessage*/
                                intersection_state,
                            );
                        }
                    }
                }
                if related == Ternary::FALSE {
                    return Ternary::FALSE;
                }
                result &= related;
            }
        }
        result
    }

    // A type [P in S]: X is related to a type [Q in T]: Y if T is related to S and X' is
    // related to Y, where X' is an instantiation of X in which P is replaced with Q. Notice
    // that S and T are contra-variant whereas X and Y are co-variant.
    // Go: checker/relater.go:4004 mappedTypeRelatedTo
    pub fn mapped_type_related_to(
        &mut self,
        r: &Rc<RefCell<Relater>>,
        source: TypeId,
        target: TypeId,
        report_errors: bool,
    ) -> Ternary {
        let relation = r.borrow().kind;
        let modifiers_related = relation == RelationKind::Comparable
            || relation == RelationKind::Identity
                && self.get_mapped_type_modifiers(source) == self.get_mapped_type_modifiers(target)
            || relation != RelationKind::Identity
                && self.get_combined_mapped_type_optionality(source)
                    <= self.get_combined_mapped_type_optionality(target);
        if modifiers_related {
            let target_constraint = self.get_constraint_type_from_mapped_type(target);
            let source_constraint_type = self.get_constraint_type_from_mapped_type(source);
            let marker_mapper = if self.get_combined_mapped_type_optionality(source) < 0 {
                self.report_unmeasurable_mapper
            } else {
                self.report_unreliable_mapper
            };
            let source_constraint = self.instantiate_type(source_constraint_type, marker_mapper);
            let result = self.is_related_to(
                r,
                target_constraint,
                source_constraint,
                RecursionFlags::BOTH,
                report_errors,
            );
            if result != Ternary::FALSE {
                let source_tp = self.get_type_parameter_from_mapped_type(source);
                let target_tp = self.get_type_parameter_from_mapped_type(target);
                let mapper = self.new_simple_type_mapper(source_tp, target_tp);
                let source_name_type = self.get_name_type_from_mapped_type(source);
                let source_name = self.instantiate_type(source_name_type, mapper);
                let target_name_type = self.get_name_type_from_mapped_type(target);
                let target_name = self.instantiate_type(target_name_type, mapper);
                if source_name == target_name {
                    let source_template = self.get_template_type_from_mapped_type(source);
                    let instantiated = self.instantiate_type(source_template, mapper);
                    let target_template = self.get_template_type_from_mapped_type(target);
                    return result
                        & self.is_related_to(
                            r,
                            instantiated,
                            target_template,
                            RecursionFlags::BOTH,
                            report_errors,
                        );
                }
            }
        }
        Ternary::FALSE
    }

    // Go: checker/relater.go:4021 typeRelatedToDiscriminatedType
    pub fn type_related_to_discriminated_type(
        &mut self,
        r: &Rc<RefCell<Relater>>,
        source: TypeId,
        target: TypeId,
    ) -> Ternary {
        // 1. Generate the combinations of discriminant properties & types 'source' can satisfy.
        //    a. If the number of combinations is above a set limit, the comparison is too complex.
        // 2. Filter 'target' to the subset of types whose discriminants exist in the matrix.
        //    a. If 'target' does not satisfy all discriminants in the matrix, 'source' is not related.
        // 3. For each type in the filtered 'target', determine if all non-discriminant properties of
        //    'target' are related to a property in 'source'.
        //
        // NOTE: See ~/tests/cases/conformance/types/typeRelationships/assignmentCompatibility/assignmentCompatWithDiscriminatedUnion.ts
        //       for examples.
        let relation = r.borrow().kind;
        let source_properties = self.get_properties_of_type(source);
        let source_properties_filtered =
            self.find_discriminant_properties(&source_properties, target);
        if source_properties_filtered.is_empty() {
            return Ternary::FALSE;
        }
        // Though we could compute the number of combinations as we generate
        // the matrix, this would incur additional memory overhead due to
        // array allocations. To reduce this overhead, we first compute
        // the number of combinations to ensure we will not surpass our
        // fixed limit before incurring the cost of any allocations:
        let mut num_combinations: i32 = 1;
        for &source_property in &source_properties_filtered {
            let t = self.get_non_missing_type_of_symbol(source_property);
            num_combinations *= self.count_types(t);
            if num_combinations > 25 {
                if let Some(tr) = self.tracer {
                    tr.instant(
                        crate::tracing::Phase::CheckTypes,
                        "typeRelatedToDiscriminatedType_DepthLimit",
                        vec![
                            ("sourceId", source.into()),
                            ("targetId", target.into()),
                            ("numCombinations", num_combinations.into()),
                        ],
                    );
                }
                return Ternary::FALSE;
            }
            if num_combinations == 0 {
                return Ternary::FALSE;
            }
        }
        // Compute the set of types for each discriminant property.
        let mut source_discriminant_types: Vec<Vec<TypeId>> =
            vec![Vec::new(); source_properties_filtered.len()];
        let mut excluded_properties: FxHashSet<String> = FxHashSet::default();
        for (i, &source_property) in source_properties_filtered.iter().enumerate() {
            let source_property_type = self.get_non_missing_type_of_symbol(source_property);
            source_discriminant_types[i] = self.ty(source_property_type).distributed();
            excluded_properties.insert(self.sym(source_property).name.to_string());
        }
        // Build the cartesian product
        let mut discriminant_combinations: Vec<Vec<TypeId>> =
            vec![Vec::new(); num_combinations as usize];
        for i in 0..num_combinations as usize {
            let mut combination: Vec<TypeId> = vec![TypeId::NIL; source_discriminant_types.len()];
            let mut n = i;
            for j in (0..source_discriminant_types.len()).rev() {
                let source_types = &source_discriminant_types[j];
                let length = source_types.len();
                combination[j] = source_types[n % length];
                n /= length;
            }
            discriminant_combinations[i] = combination;
        }
        // Match each combination of the cartesian product of discriminant properties to one or more
        // constituents of 'target'. If any combination does not have a match then 'source' is not relatable.
        let mut matching_types: Vec<TypeId> = Vec::new();
        let target_types = self.ty(target).types_list();
        let skip_optional = self.strict_null_checks || relation == RelationKind::Comparable;
        for combination in &discriminant_combinations {
            let mut has_match = false;
            'outer: for &t in &target_types {
                for i in 0..source_properties_filtered.len() {
                    let source_property = source_properties_filtered[i];
                    let name = self.sym(source_property).name.clone();
                    let target_property = self.get_property_of_type_name(t, &name);
                    if target_property.is_nil() {
                        continue 'outer;
                    }
                    if source_property == target_property {
                        continue;
                    }
                    // We compare the source property to the target in the context of a single discriminant type.
                    let combination_type = combination[i];
                    let related = self.property_related_to(
                        r,
                        source,
                        target,
                        source_property,
                        target_property,
                        &mut move |_: &mut Checker, _: SymbolId| combination_type,
                        false, /*reportErrors*/
                        IntersectionState::NONE,
                        skip_optional, /*skipOptional*/
                    );
                    // If the target property could not be found, or if the properties were not related,
                    // then this constituent is not a match.
                    if related == Ternary::FALSE {
                        continue 'outer;
                    }
                }
                if !matching_types.contains(&t) {
                    matching_types.push(t);
                }
                has_match = true;
            }
            if !has_match {
                // We failed to match any type for this combination.
                return Ternary::FALSE;
            }
        }
        // Compare the remaining non-discriminant properties of each match.
        let mut result = Ternary::TRUE;
        for &t in &matching_types {
            result &= self.properties_related_to(
                r,
                source,
                t,
                false, /*reportErrors*/
                &excluded_properties,
                false, /*optionalsOnly*/
                IntersectionState::NONE,
            );
            if result != Ternary::FALSE {
                result &= self.signatures_related_to(
                    r,
                    source,
                    t,
                    SignatureKind::CALL,
                    false, /*reportErrors*/
                    IntersectionState::NONE,
                );
                if result != Ternary::FALSE {
                    result &= self.signatures_related_to(
                        r,
                        source,
                        t,
                        SignatureKind::CONSTRUCT,
                        false, /*reportErrors*/
                        IntersectionState::NONE,
                    );
                    if result != Ternary::FALSE
                        && !(self.is_tuple_type(source) && self.is_tuple_type(t))
                    {
                        // Comparing numeric index types when both `source` and `type` are tuples is unnecessary as the
                        // element types should be sufficiently covered by `propertiesRelatedTo`. It also causes problems
                        // with index type assignability as the types for the excluded discriminants are still included
                        // in the index type.
                        result &= self.index_signatures_related_to(
                            r,
                            source,
                            t,
                            false, /*sourceIsPrimitive*/
                            false, /*reportErrors*/
                            IntersectionState::NONE,
                        );
                    }
                }
            }
            if result == Ternary::FALSE {
                return result;
            }
        }
        result
    }

    // Go: checker/relater.go:4132 propertiesRelatedTo
    pub fn properties_related_to(
        &mut self,
        r: &Rc<RefCell<Relater>>,
        source: TypeId,
        target: TypeId,
        report_errors: bool,
        excluded_properties: &FxHashSet<String>,
        optionals_only: bool,
        intersection_state: IntersectionState,
    ) -> Ternary {
        let relation = r.borrow().kind;
        if relation == RelationKind::Identity {
            return self.properties_identical_to(r, source, target, excluded_properties);
        }
        let mut result = Ternary::TRUE;
        if self.is_tuple_type(target) {
            if self.is_array_or_tuple_type(source) {
                if !self.target_tuple_type(target).readonly
                    && (self.is_readonly_array_type(source)
                        || self.is_tuple_type(source) && self.target_tuple_type(source).readonly)
                {
                    return Ternary::FALSE;
                }
                let source_arity = self.get_type_reference_arity(source);
                let target_arity = self.get_type_reference_arity(target);
                let source_rest: bool;
                if self.is_tuple_type(source) {
                    source_rest = self
                        .target_tuple_type(source)
                        .combined_flags
                        .intersects(ElementFlags::REST);
                } else {
                    source_rest = true;
                }
                let target_has_rest_element = self
                    .target_tuple_type(target)
                    .combined_flags
                    .intersects(ElementFlags::REST);
                let target_has_variable_element = self
                    .target_tuple_type(target)
                    .combined_flags
                    .intersects(ElementFlags::VARIABLE);
                let source_min_length: i32;
                if self.is_tuple_type(source) {
                    source_min_length = self.target_tuple_type(source).min_length;
                } else {
                    source_min_length = 0;
                }
                let target_min_length = self.target_tuple_type(target).min_length;
                if !source_rest && source_arity < target_min_length {
                    if report_errors {
                        self.report_error(
                            r,
                            diag::Source_has_0_element_s_but_target_requires_1,
                            args![source_arity, target_min_length],
                        );
                    }
                    return Ternary::FALSE;
                }
                if !target_has_variable_element && target_arity < source_min_length {
                    if report_errors {
                        self.report_error(
                            r,
                            diag::Source_has_0_element_s_but_target_allows_only_1,
                            args![source_min_length, target_arity],
                        );
                    }
                    return Ternary::FALSE;
                }
                if !target_has_variable_element && (source_rest || target_arity < source_arity) {
                    if report_errors {
                        if source_min_length < target_min_length {
                            self.report_error(
                                r,
                                diag::Target_requires_0_element_s_but_source_may_have_fewer,
                                args![target_min_length],
                            );
                        } else {
                            self.report_error(
                                r,
                                diag::Target_allows_only_0_element_s_but_source_may_have_more,
                                args![target_arity],
                            );
                        }
                    }
                    return Ternary::FALSE;
                }
                let source_type_arguments = self.get_type_arguments(source);
                let target_type_arguments = self.get_type_arguments(target);
                let target_start_count =
                    get_start_element_count(self.target_tuple_type(target), ElementFlags::NON_REST);
                let target_end_count =
                    get_end_element_count(self.target_tuple_type(target), ElementFlags::NON_REST);
                let mut can_exclude_discriminants = !excluded_properties.is_empty();
                for source_position in 0..source_arity {
                    let source_flags: ElementFlags;
                    if self.is_tuple_type(source) {
                        source_flags = self.target_tuple_type(source).element_infos
                            [source_position as usize]
                            .flags;
                    } else {
                        source_flags = ElementFlags::REST;
                    }
                    let source_position_from_end = source_arity - 1 - source_position;
                    let target_position: i32;
                    if target_has_rest_element && source_position >= target_start_count {
                        target_position =
                            target_arity - 1 - source_position_from_end.min(target_end_count);
                    } else {
                        if source_position >= target_arity {
                            if report_errors {
                                self.report_error(
                                    r,
                                    diag::Target_allows_only_0_element_s_but_source_may_have_more,
                                    args![target_arity],
                                );
                            }
                            return Ternary::FALSE;
                        }
                        target_position = source_position;
                    }
                    let mut target_flags = ElementFlags::NONE;
                    if target_position >= 0 {
                        target_flags = self.target_tuple_type(target).element_infos
                            [target_position as usize]
                            .flags;
                    }
                    if target_flags.intersects(ElementFlags::VARIADIC)
                        && !source_flags.intersects(ElementFlags::VARIADIC)
                    {
                        if report_errors {
                            self.report_error(r, diag::Source_provides_no_match_for_variadic_element_at_position_0_in_target, args![target_position]);
                        }
                        return Ternary::FALSE;
                    }
                    if source_flags.intersects(ElementFlags::VARIADIC)
                        && !target_flags.intersects(ElementFlags::VARIABLE)
                    {
                        if report_errors {
                            self.report_error(
                                r,
                                diag::Variadic_element_at_position_0_in_source_does_not_match_element_at_position_1_in_target,
                                args![source_position, target_position],
                            );
                        }
                        return Ternary::FALSE;
                    }
                    if target_flags.intersects(ElementFlags::REQUIRED)
                        && !source_flags.intersects(ElementFlags::REQUIRED)
                    {
                        if report_errors {
                            self.report_error(r, diag::Source_provides_no_match_for_required_element_at_position_0_in_target, args![target_position]);
                        }
                        return Ternary::FALSE;
                    }
                    // We can only exclude discriminant properties if we have not yet encountered a variable-length element.
                    if can_exclude_discriminants {
                        if source_flags.intersects(ElementFlags::VARIABLE)
                            || target_flags.intersects(ElementFlags::VARIABLE)
                        {
                            can_exclude_discriminants = false;
                        }
                        if can_exclude_discriminants
                            && excluded_properties.contains(&source_position.to_string())
                        {
                            continue;
                        }
                    }
                    let source_type = self.remove_missing_type(
                        source_type_arguments[source_position as usize],
                        (source_flags & target_flags).intersects(ElementFlags::OPTIONAL),
                    );
                    let target_type = target_type_arguments[target_position as usize];
                    let target_check_type: TypeId;
                    if source_flags.intersects(ElementFlags::VARIADIC)
                        && target_flags.intersects(ElementFlags::REST)
                    {
                        target_check_type = self.create_array_type(target_type);
                    } else {
                        target_check_type = self.remove_missing_type(
                            target_type,
                            target_flags.intersects(ElementFlags::OPTIONAL),
                        );
                    }
                    let related = self.is_related_to_ex(
                        r,
                        source_type,
                        target_check_type,
                        RecursionFlags::BOTH,
                        report_errors,
                        None, /*headMessage*/
                        intersection_state,
                    );
                    if related == Ternary::FALSE {
                        if report_errors && (target_arity > 1 || source_arity > 1) {
                            if target_has_rest_element
                                && source_position >= target_start_count
                                && source_position_from_end >= target_end_count
                                && target_start_count != source_arity - target_end_count - 1
                            {
                                self.report_error(
                                    r,
                                    diag::Type_at_positions_0_through_1_in_source_is_not_compatible_with_type_at_position_2_in_target,
                                    args![target_start_count, source_arity - target_end_count - 1, target_position],
                                );
                            } else {
                                self.report_error(
                                    r,
                                    diag::Type_at_position_0_in_source_is_not_compatible_with_type_at_position_1_in_target,
                                    args![source_position, target_position],
                                );
                            }
                        }
                        return Ternary::FALSE;
                    }
                    result &= related;
                }
                return result;
            }
            if self
                .target_tuple_type(target)
                .combined_flags
                .intersects(ElementFlags::VARIABLE)
            {
                return Ternary::FALSE;
            }
        }
        let require_optional_properties = (relation == RelationKind::Subtype
            || relation == RelationKind::StrictSubtype)
            && !self.is_object_literal_type(source)
            && !self.is_empty_array_literal_type(source)
            && !self.is_tuple_type(source);
        let unmatched_property = self.get_unmatched_property(
            source,
            target,
            require_optional_properties,
            false, /*matchDiscriminantProperties*/
        );
        if unmatched_property.is_some() {
            if report_errors && self.should_report_unmatched_property_error(source, target) {
                self.report_unmatched_property(
                    r,
                    source,
                    target,
                    unmatched_property,
                    require_optional_properties,
                );
            }
            return Ternary::FALSE;
        }
        if self.is_object_literal_type(target) {
            let source_props = self.get_properties_of_type(source);
            for source_prop in self.exclude_properties(source_props, excluded_properties) {
                let name = self.sym(source_prop).name.clone();
                if self
                    .get_property_of_object_type_key(target, TableKey::Name(&name))
                    .is_nil()
                {
                    if report_errors {
                        let prop_str = self.symbol_to_string(source_prop);
                        let target_str = self.type_to_string_exported(target);
                        self.report_error(
                            r,
                            diag::Property_0_does_not_exist_on_type_1,
                            args![prop_str, target_str],
                        );
                    }
                    return Ternary::FALSE;
                }
            }
        }
        // We only call this for union target types when we're attempting to do excess property checking - in those cases, we want to get _all possible props_
        // from the target union, across all members
        let properties = self.get_properties_of_type(target);
        let numeric_names_only = self.is_tuple_type(source) && self.is_tuple_type(target);
        for target_prop in self.exclude_properties(properties, excluded_properties) {
            let name = self.sym(target_prop).name.clone();
            let target_prop_flags = self.sym(target_prop).flags;
            if !target_prop_flags.intersects(SymbolFlags::PROTOTYPE)
                && (!numeric_names_only || is_numeric_literal_name(&name) || name == "length")
                && (!optionals_only || target_prop_flags.intersects(SymbolFlags::OPTIONAL))
            {
                let source_prop = self.get_property_of_type_name(source, &name);
                if source_prop.is_some() && source_prop != target_prop {
                    let skip_optional = relation == RelationKind::Comparable;
                    let related = self.property_related_to(
                        r,
                        source,
                        target,
                        source_prop,
                        target_prop,
                        &mut |c: &mut Checker, s: SymbolId| c.get_non_missing_type_of_symbol(s),
                        report_errors,
                        intersection_state,
                        skip_optional,
                    );
                    if related == Ternary::FALSE {
                        return Ternary::FALSE;
                    }
                    result &= related;
                }
            }
        }
        result
    }

    // Go: checker/relater.go:4302 propertyRelatedTo
    pub fn property_related_to(
        &mut self,
        r: &Rc<RefCell<Relater>>,
        source: TypeId,
        target: TypeId,
        source_prop: SymbolId,
        target_prop: SymbolId,
        get_type_of_source_property: &mut dyn FnMut(&mut Checker, SymbolId) -> TypeId,
        report_errors: bool,
        intersection_state: IntersectionState,
        skip_optional: bool,
    ) -> Ternary {
        let relation = r.borrow().kind;
        let source_prop_flags = self.get_declaration_modifier_flags_from_symbol(source_prop);
        let target_prop_flags = self.get_declaration_modifier_flags_from_symbol(target_prop);
        if source_prop_flags.intersects(ModifierFlags::PRIVATE)
            || target_prop_flags.intersects(ModifierFlags::PRIVATE)
        {
            if self.sym(source_prop).value_declaration != self.sym(target_prop).value_declaration {
                if report_errors {
                    if source_prop_flags.intersects(ModifierFlags::PRIVATE)
                        && target_prop_flags.intersects(ModifierFlags::PRIVATE)
                    {
                        let prop_str = self.symbol_to_string(target_prop);
                        self.report_error(
                            r,
                            diag::Types_have_separate_declarations_of_a_private_property_0,
                            args![prop_str],
                        );
                    } else {
                        let prop_str = self.symbol_to_string(target_prop);
                        let source_is_private =
                            source_prop_flags.intersects(ModifierFlags::PRIVATE);
                        let first = self.type_to_string_exported(if source_is_private {
                            source
                        } else {
                            target
                        });
                        let second = self.type_to_string_exported(if source_is_private {
                            target
                        } else {
                            source
                        });
                        self.report_error(
                            r,
                            diag::Property_0_is_private_in_type_1_but_not_in_type_2,
                            args![prop_str, first, second],
                        );
                    }
                }
                return Ternary::FALSE;
            }
        } else if target_prop_flags.intersects(ModifierFlags::PROTECTED) {
            if !self.is_valid_override_of(source_prop, target_prop) {
                if report_errors {
                    let mut source_type = self.get_declaring_class(source_prop);
                    if source_type.is_nil() {
                        source_type = source;
                    }
                    let mut target_type = self.get_declaring_class(target_prop);
                    if target_type.is_nil() {
                        target_type = target;
                    }
                    let prop_str = self.symbol_to_string(target_prop);
                    let source_str = self.type_to_string_exported(source_type);
                    let target_str = self.type_to_string_exported(target_type);
                    self.report_error(
                        r,
                        diag::Property_0_is_protected_but_type_1_is_not_a_class_derived_from_2,
                        args![prop_str, source_str, target_str],
                    );
                }
                return Ternary::FALSE;
            }
        } else if source_prop_flags.intersects(ModifierFlags::PROTECTED) {
            if report_errors {
                let prop_str = self.symbol_to_string(target_prop);
                let source_str = self.type_to_string_exported(source);
                let target_str = self.type_to_string_exported(target);
                self.report_error(
                    r,
                    diag::Property_0_is_protected_in_type_1_but_public_in_type_2,
                    args![prop_str, source_str, target_str],
                );
            }
            return Ternary::FALSE;
        }
        // Ensure {readonly a: whatever} is not a subtype of {a: whatever},
        // while {a: whatever} is a subtype of {readonly a: whatever}.
        // This ensures the subtype relationship is ordered, and preventing declaration order
        // from deciding which type "wins" in union subtype reduction.
        // They're still assignable to one another, since `readonly` doesn't affect assignability.
        // This is only applied during the strictSubtypeRelation -- currently used in subtype reduction
        if relation == RelationKind::StrictSubtype
            && self.is_readonly_symbol(source_prop)
            && !self.is_readonly_symbol(target_prop)
        {
            return Ternary::FALSE;
        }
        // If the target comes from a partial union prop, allow `undefined` in the target type
        let related = self.is_property_symbol_type_related(
            r,
            source_prop,
            target_prop,
            get_type_of_source_property,
            report_errors,
            intersection_state,
        );
        if related == Ternary::FALSE {
            if report_errors {
                let prop_str = self.symbol_to_string(target_prop);
                self.report_error(
                    r,
                    diag::Types_of_property_0_are_incompatible,
                    args![prop_str],
                );
            }
            return Ternary::FALSE;
        }
        // When checking for comparability, be more lenient with optional properties.
        let source_flags = self.sym(source_prop).flags;
        let target_flags = self.sym(target_prop).flags;
        if !skip_optional
            && source_flags.intersects(SymbolFlags::OPTIONAL)
            && target_flags.intersects(SymbolFlags::CLASS_MEMBER)
            && !target_flags.intersects(SymbolFlags::OPTIONAL)
        {
            // TypeScript 1.0 spec (April 2014): 3.8.3
            // S is a subtype of a type T, and T is a supertype of S if ...
            // S' and T are object types and, for each member M in T..
            // M is a property and S' contains a property N where
            // if M is a required property, N is also a required property
            // (M - property in T)
            // (N - property in S)
            if report_errors {
                let prop_str = self.symbol_to_string(target_prop);
                let source_str = self.type_to_string_exported(source);
                let target_str = self.type_to_string_exported(target);
                self.report_error(
                    r,
                    diag::Property_0_is_optional_in_type_1_but_required_in_type_2,
                    args![prop_str, source_str, target_str],
                );
            }
            return Ternary::FALSE;
        }
        related
    }

    // Go: checker/relater.go:4366 isPropertySymbolTypeRelated
    pub fn is_property_symbol_type_related(
        &mut self,
        r: &Rc<RefCell<Relater>>,
        source_prop: SymbolId,
        target_prop: SymbolId,
        get_type_of_source_property: &mut dyn FnMut(&mut Checker, SymbolId) -> TypeId,
        report_errors: bool,
        intersection_state: IntersectionState,
    ) -> Ternary {
        let relation = r.borrow().kind;
        let target_is_optional = self.strict_null_checks
            && self
                .sym(target_prop)
                .check_flags
                .intersects(CheckFlags::PARTIAL);
        let target_type = self.get_non_missing_type_of_symbol(target_prop);
        let effective_target =
            self.add_optionality_ex(target_type, false /*isProperty*/, target_is_optional);
        // source could resolve to `any` and that's not related to `unknown` target under strict subtype relation
        let mask = if relation == RelationKind::StrictSubtype {
            TypeFlags::ANY
        } else {
            TypeFlags::ANY_OR_UNKNOWN
        };
        if self.ty(effective_target).flags.intersects(mask) {
            return Ternary::TRUE;
        }
        let effective_source = get_type_of_source_property(self, source_prop);
        self.is_related_to_ex(
            r,
            effective_source,
            effective_target,
            RecursionFlags::BOTH,
            report_errors,
            None, /*headMessage*/
            intersection_state,
        )
    }

    // Go: checker/relater.go:4377 reportUnmatchedProperty
    pub fn report_unmatched_property(
        &mut self,
        r: &Rc<RefCell<Relater>>,
        source: TypeId,
        target: TypeId,
        unmatched_property: SymbolId,
        require_optional_properties: bool,
    ) {
        // give specific error in case where private names have the same description
        let value_declaration = self.sym(unmatched_property).value_declaration;
        let source_symbol = self.ty(source).symbol;
        if value_declaration.is_some()
            && value_declaration.name().is_some()
            && is_private_identifier(value_declaration.name())
            && source_symbol.is_some()
            && self.sym(source_symbol).flags.intersects(SymbolFlags::CLASS)
        {
            let private_identifier_description = value_declaration.name().text().to_string();
            let symbol_table_key =
                self.private_identifier_symbol_name(source_symbol, &private_identifier_description);
            if self
                .get_property_of_type(source, &symbol_table_key)
                .is_some()
            {
                let source_str = self.symbol_to_string_exported(source_symbol);
                let target_symbol = self.ty(target).symbol;
                let target_str = self.symbol_to_string_exported(target_symbol);
                self.report_error(
                    r,
                    diag::Property_0_in_type_1_refers_to_a_different_member_that_cannot_be_accessed_from_within_type_2,
                    args![private_identifier_description, source_str, target_str],
                );
                return;
            }
        }
        let props = self.get_unmatched_properties(
            source,
            target,
            require_optional_properties,
            false, /*matchDiscriminantProperties*/
        );
        if props.len() == 1 {
            let (source_type, target_type) = self.get_type_names_for_error_display(source, target);
            let prop_name = self.symbol_to_string(unmatched_property);
            self.report_error(
                r,
                diag::Property_0_is_missing_in_type_1_but_required_in_type_2,
                args![prop_name, source_type, target_type],
            );
            let declarations = self.sym(unmatched_property).declarations.clone();
            if !declarations.is_empty() {
                let d = create_diagnostic_for_node(
                    declarations[0],
                    diag::X_0_is_declared_here,
                    args![prop_name],
                );
                r.borrow_mut().related_info.push(d);
            }
        } else if self
            .try_elaborate_array_like_errors(r, source, target, false /*reportErrors*/)
        {
            let (source_type, target_type) = self.get_type_names_for_error_display(source, target);
            if props.len() > 5 {
                let names: Vec<String> = props[..4]
                    .iter()
                    .map(|&p| self.symbol_to_string(p))
                    .collect();
                let prop_names = names.join(", ");
                self.report_error(
                    r,
                    diag::Type_0_is_missing_the_following_properties_from_type_1_Colon_2_and_3_more,
                    args![source_type, target_type, prop_names, props.len() - 4],
                );
            } else {
                let names: Vec<String> = props.iter().map(|&p| self.symbol_to_string(p)).collect();
                let prop_names = names.join(", ");
                self.report_error(
                    r,
                    diag::Type_0_is_missing_the_following_properties_from_type_1_Colon_2,
                    args![source_type, target_type, prop_names],
                );
            }
        }
    }

    // Go: checker/relater.go:4411 tryElaborateArrayLikeErrors
    pub fn try_elaborate_array_like_errors(
        &mut self,
        r: &Rc<RefCell<Relater>>,
        source: TypeId,
        target: TypeId,
        report_errors: bool,
    ) -> bool {
        /*
         * The spec for elaboration is:
         * - If the source is a readonly tuple and the target is a mutable array or tuple, elaborate on mutability and skip property elaborations.
         * - If the source is a tuple then skip property elaborations if the target is an array or tuple.
         * - If the source is a readonly array and the target is a mutable array or tuple, elaborate on mutability and skip property elaborations.
         * - If the source an array then skip property elaborations if the target is a tuple.
         */
        if self.is_tuple_type(source) {
            if self.target_tuple_type(source).readonly && self.is_mutable_array_or_tuple(target) {
                if report_errors {
                    let source_str = self.type_to_string_exported(source);
                    let target_str = self.type_to_string_exported(target);
                    self.report_error(
                        r,
                        diag::The_type_0_is_readonly_and_cannot_be_assigned_to_the_mutable_type_1,
                        args![source_str, target_str],
                    );
                }
                return false;
            }
            return self.is_array_or_tuple_type(target);
        }
        if self.is_readonly_array_type(source) && self.is_mutable_array_or_tuple(target) {
            if report_errors {
                let source_str = self.type_to_string_exported(source);
                let target_str = self.type_to_string_exported(target);
                self.report_error(
                    r,
                    diag::The_type_0_is_readonly_and_cannot_be_assigned_to_the_mutable_type_1,
                    args![source_str, target_str],
                );
            }
            return false;
        }
        if self.is_tuple_type(target) {
            return self.is_array_type(source);
        }
        true
    }

    // Go: checker/relater.go:4440 tryElaborateErrorsForPrimitivesAndObjects
    pub fn try_elaborate_errors_for_primitives_and_objects(
        &mut self,
        r: &Rc<RefCell<Relater>>,
        source: TypeId,
        target: TypeId,
    ) {
        let matches = (source == self.global_string_type && target == self.string_type)
            || (source == self.global_number_type && target == self.number_type)
            || (source == self.global_boolean_type && target == self.boolean_type)
            || {
                let get_global_es_symbol_type = self.get_global_es_symbol_type.clone();
                source == get_global_es_symbol_type(self) && target == self.es_symbol_type
            };
        if matches {
            let target_str = self.type_to_string_exported(target);
            let source_str = self.type_to_string_exported(source);
            self.report_error(
                r,
                diag::X_0_is_a_primitive_but_1_is_a_wrapper_object_Prefer_using_0_when_possible,
                args![target_str, source_str],
            );
        }
    }

    // Go: checker/relater.go:4449 propertiesIdenticalTo
    pub fn properties_identical_to(
        &mut self,
        r: &Rc<RefCell<Relater>>,
        source: TypeId,
        target: TypeId,
        excluded_properties: &FxHashSet<String>,
    ) -> Ternary {
        if !self.ty(source).flags.intersects(TypeFlags::OBJECT)
            || !self.ty(target).flags.intersects(TypeFlags::OBJECT)
        {
            return Ternary::FALSE;
        }
        let source_props = self.get_properties_of_object_type(source);
        let source_properties = self.exclude_properties(source_props, excluded_properties);
        let target_props = self.get_properties_of_object_type(target);
        let target_properties = self.exclude_properties(target_props, excluded_properties);
        if source_properties.len() != target_properties.len() {
            return Ternary::FALSE;
        }
        let mut result = Ternary::TRUE;
        for &source_prop in &source_properties {
            let name = self.sym(source_prop).name.clone();
            let target_prop = self.get_property_of_object_type_key(target, TableKey::Name(&name));
            if target_prop.is_nil() {
                return Ternary::FALSE;
            }
            let related = self.compare_properties(
                source_prop,
                target_prop,
                &mut |c: &mut Checker, s: TypeId, t: TypeId| c.is_related_to_simple(r, s, t),
            );
            if related == Ternary::FALSE {
                return Ternary::FALSE;
            }
            result &= related;
        }
        result
    }

    // Go: checker/relater.go:4473 signaturesRelatedTo
    pub fn signatures_related_to(
        &mut self,
        r: &Rc<RefCell<Relater>>,
        source: TypeId,
        target: TypeId,
        kind: SignatureKind,
        report_errors: bool,
        intersection_state: IntersectionState,
    ) -> Ternary {
        let relation = r.borrow().kind;
        if relation == RelationKind::Identity {
            return self.signatures_identical_to(r, source, target, kind);
        }
        // With respect to signatures, the anyFunctionType wildcard is a subtype of every other function type.
        if source == self.any_function_type {
            return Ternary::TRUE;
        }
        if target == self.any_function_type {
            return Ternary::FALSE;
        }
        let source_signatures = self.get_signatures_of_type(source, kind);
        let target_signatures = self.get_signatures_of_type(target, kind);
        if kind == SignatureKind::CONSTRUCT
            && !source_signatures.is_empty()
            && !target_signatures.is_empty()
        {
            let source_is_abstract = self
                .sig(source_signatures[0])
                .flags
                .intersects(SignatureFlags::ABSTRACT);
            let target_is_abstract = self
                .sig(target_signatures[0])
                .flags
                .intersects(SignatureFlags::ABSTRACT);
            if source_is_abstract && !target_is_abstract {
                // An abstract constructor type is not assignable to a non-abstract constructor type
                // as it would otherwise be possible to new an abstract class. Note that the assignability
                // check we perform for an extends clause excludes construct signatures from the target,
                // so this check never proceeds.
                if report_errors {
                    self.report_error(r, diag::Cannot_assign_an_abstract_constructor_type_to_a_non_abstract_constructor_type, args![]);
                }
                return Ternary::FALSE;
            }
            if !self.constructor_visibilities_are_compatible(
                r,
                source_signatures[0],
                target_signatures[0],
                report_errors,
            ) {
                return Ternary::FALSE;
            }
        }
        let mut result = Ternary::TRUE;
        let (source_object_flags, source_symbol) =
            (self.ty(source).object_flags, self.ty(source).symbol);
        let (target_object_flags, target_symbol) =
            (self.ty(target).object_flags, self.ty(target).symbol);
        if source_object_flags.intersects(ObjectFlags::INSTANTIATED)
            && target_object_flags.intersects(ObjectFlags::INSTANTIATED)
            && source_symbol == target_symbol
            || source_object_flags.intersects(ObjectFlags::REFERENCE)
                && target_object_flags.intersects(ObjectFlags::REFERENCE)
                && self.ty(source).target() == self.ty(target).target()
        {
            // We have instantiations of the same anonymous type (which typically will be the type of a
            // method). Simply do a pairwise comparison of the signatures in the two signature lists instead
            // of the much more expensive N * M comparison matrix we explore below. We erase type parameters
            // as they are known to always be the same.
            for i in 0..target_signatures.len() {
                let related = self.signature_related_to(
                    r,
                    source_signatures[i],
                    target_signatures[i],
                    true, /*erase*/
                    report_errors,
                    intersection_state,
                );
                if related == Ternary::FALSE {
                    return Ternary::FALSE;
                }
                result &= related;
            }
        } else if source_signatures.len() == 1 && target_signatures.len() == 1 {
            // For simple functions (functions with a single signature) we only erase type parameters for
            // the comparable relation. Otherwise, if the source signature is generic, we instantiate it
            // in the context of the target signature before checking the relationship. Ideally we'd do
            // this regardless of the number of signatures, but the potential costs are prohibitive due
            // to the quadratic nature of the logic below.
            let erase_generics = relation == RelationKind::Comparable;
            result = self.signature_related_to(
                r,
                source_signatures[0],
                target_signatures[0],
                erase_generics,
                report_errors,
                intersection_state,
            );
        } else {
            'outer: for &t in &target_signatures {
                let save_error_state = self.get_error_state(r);
                // Only elaborate errors from the first failure
                let mut should_elaborate_errors = report_errors;
                for &s in &source_signatures {
                    let related = self.signature_related_to(
                        r,
                        s,
                        t,
                        true, /*erase*/
                        should_elaborate_errors,
                        intersection_state,
                    );
                    if related != Ternary::FALSE {
                        result &= related;
                        self.restore_error_state(r, save_error_state);
                        continue 'outer;
                    }
                    should_elaborate_errors = false;
                }
                if should_elaborate_errors {
                    let source_str = self.type_to_string_exported(source);
                    let sig_str = self.signature_to_string(t);
                    self.report_error(
                        r,
                        diag::Type_0_provides_no_match_for_the_signature_1,
                        args![source_str, sig_str],
                    );
                }
                return Ternary::FALSE;
            }
        }
        result
    }

    // Go: checker/relater.go:4550 constructorVisibilitiesAreCompatible
    pub fn constructor_visibilities_are_compatible(
        &mut self,
        r: &Rc<RefCell<Relater>>,
        source_signature: SignatureId,
        target_signature: SignatureId,
        report_errors: bool,
    ) -> bool {
        let source_declaration = self.sig(source_signature).declaration;
        let target_declaration = self.sig(target_signature).declaration;
        if source_declaration.is_nil() || target_declaration.is_nil() {
            return true;
        }
        let source_accessibility =
            source_declaration.modifier_flags() & ModifierFlags::NON_PUBLIC_ACCESSIBILITY_MODIFIER;
        let target_accessibility =
            target_declaration.modifier_flags() & ModifierFlags::NON_PUBLIC_ACCESSIBILITY_MODIFIER;
        // A public, protected and private signature is assignable to a private signature.
        if target_accessibility == ModifierFlags::PRIVATE {
            return true;
        }
        // A public and protected signature is assignable to a protected signature.
        if target_accessibility == ModifierFlags::PROTECTED
            && source_accessibility != ModifierFlags::PRIVATE
        {
            return true;
        }
        // Only a public signature is assignable to public signature.
        if target_accessibility != ModifierFlags::PROTECTED && source_accessibility.is_empty() {
            return true;
        }
        if report_errors {
            self.report_error(
                r,
                diag::Cannot_assign_a_0_constructor_type_to_a_1_constructor_type,
                args![
                    visibility_to_string(source_accessibility),
                    visibility_to_string(target_accessibility)
                ],
            );
        }
        false
    }

    // See signatureAssignableTo, compareSignaturesIdentical
    // Go: checker/relater.go:4575 signatureRelatedTo
    pub fn signature_related_to(
        &mut self,
        r: &Rc<RefCell<Relater>>,
        mut source: SignatureId,
        mut target: SignatureId,
        erase: bool,
        report_errors: bool,
        intersection_state: IntersectionState,
    ) -> Ternary {
        let relation = r.borrow().kind;
        let mut check_mode = SignatureCheckMode::NONE;
        if relation == RelationKind::Subtype {
            check_mode = SignatureCheckMode::STRICT_TOP_SIGNATURE;
        } else if relation == RelationKind::StrictSubtype {
            check_mode =
                SignatureCheckMode::STRICT_TOP_SIGNATURE | SignatureCheckMode::STRICT_ARITY;
        }
        if erase {
            source = self.get_erased_signature(source);
            target = self.get_erased_signature(target);
        }
        // PORT: the Go closure `isRelatedToWorker` is a `TypeComparer`
        // (`Rc<dyn Fn>`), so it holds its own clone of the relater handle.
        let rr = r.clone();
        let is_related_to_worker: TypeComparer = Rc::new(
            move |c: &mut Checker,
                  source: TypeId,
                  target: TypeId,
                  report_errors: bool|
                  -> Ternary {
                c.is_related_to_ex(
                    &rr,
                    source,
                    target,
                    RecursionFlags::BOTH,
                    report_errors,
                    None, /*headMessage*/
                    intersection_state,
                )
            },
        );
        let report_unreliable_mapper = self.report_unreliable_mapper;
        self.compare_signatures_related(
            source,
            target,
            check_mode,
            report_errors,
            Some(
                &mut |c: &mut Checker, message: &'static Message, args: Vec<String>| {
                    c.report_error(r, message, args)
                },
            ),
            is_related_to_worker,
            report_unreliable_mapper,
        )
    }

    // Go: checker/relater.go:4593 signaturesIdenticalTo
    pub fn signatures_identical_to(
        &mut self,
        r: &Rc<RefCell<Relater>>,
        source: TypeId,
        target: TypeId,
        kind: SignatureKind,
    ) -> Ternary {
        let source_signatures = self.get_signatures_of_type(source, kind);
        let target_signatures = self.get_signatures_of_type(target, kind);
        if source_signatures.len() != target_signatures.len() {
            return Ternary::FALSE;
        }
        let mut result = Ternary::TRUE;
        for i in 0..source_signatures.len() {
            let related = self.compare_signatures_identical(
                source_signatures[i],
                target_signatures[i],
                false, /*partialMatch*/
                false, /*ignoreThisTypes*/
                false, /*ignoreReturnTypes*/
                &mut |c: &mut Checker, s: TypeId, t: TypeId| c.is_related_to_simple(r, s, t),
            );
            if related == Ternary::FALSE {
                return Ternary::FALSE;
            }
            result &= related;
        }
        result
    }

    // Go: checker/relater.go:4610 indexSignaturesRelatedTo
    pub fn index_signatures_related_to(
        &mut self,
        r: &Rc<RefCell<Relater>>,
        source: TypeId,
        target: TypeId,
        source_is_primitive: bool,
        report_errors: bool,
        intersection_state: IntersectionState,
    ) -> Ternary {
        let relation = r.borrow().kind;
        if relation == RelationKind::Identity {
            return self.index_signatures_identical_to(r, source, target);
        }
        let index_infos = self.get_index_infos_of_type(target);
        let target_has_string_index = index_infos
            .iter()
            .any(|&info| self.index_info(info).key_type == self.string_type);
        let mut result = Ternary::TRUE;
        for &target_info in &index_infos {
            let related: Ternary;
            let target_value_type = self.index_info(target_info).value_type;
            if relation != RelationKind::StrictSubtype
                && !source_is_primitive
                && target_has_string_index
                && self.ty(target_value_type).flags.intersects(TypeFlags::ANY)
            {
                related = Ternary::TRUE;
            } else if self.is_generic_mapped_type(source) && target_has_string_index {
                let template = self.get_template_type_from_mapped_type(source);
                related = self.is_related_to(
                    r,
                    template,
                    target_value_type,
                    RecursionFlags::BOTH,
                    report_errors,
                );
            } else {
                related = self.type_related_to_index_info(
                    r,
                    source,
                    target_info,
                    report_errors,
                    intersection_state,
                );
            }
            if related == Ternary::FALSE {
                return Ternary::FALSE;
            }
            result &= related;
        }
        result
    }

    // Go: checker/relater.go:4635 typeRelatedToIndexInfo
    pub fn type_related_to_index_info(
        &mut self,
        r: &Rc<RefCell<Relater>>,
        source: TypeId,
        target_info: IndexInfoId,
        report_errors: bool,
        intersection_state: IntersectionState,
    ) -> Ternary {
        let relation = r.borrow().kind;
        let target_key_type = self.index_info(target_info).key_type;
        let source_info = self.get_applicable_index_info(source, target_key_type);
        if source_info.is_some() {
            return self.index_info_related_to(
                r,
                source_info,
                target_info,
                report_errors,
                intersection_state,
            );
        }
        // Intersection constituents are never considered to have an inferred index signature. Also, in the strict subtype relation,
        // only fresh object literals are considered to have inferred index signatures. This ensures { [x: string]: xxx } <: {} but
        // not vice-versa. Without this rule, those types would be mutual strict subtypes.
        if !intersection_state.intersects(IntersectionState::SOURCE)
            && (relation != RelationKind::StrictSubtype
                || self
                    .ty(source)
                    .object_flags
                    .intersects(ObjectFlags::FRESH_LITERAL))
            && self.is_object_type_with_inferable_index(source)
        {
            return self.members_related_to_index_info(
                r,
                source,
                target_info,
                report_errors,
                intersection_state,
            );
        }
        if report_errors {
            let key_str = self.type_to_string_exported(target_key_type);
            let source_str = self.type_to_string_exported(source);
            self.report_error(
                r,
                diag::Index_signature_for_type_0_is_missing_in_type_1,
                args![key_str, source_str],
            );
        }
        Ternary::FALSE
    }

    // Return true if the type was inferred from
    //   - an object literal, object type literal, enum type, or a value module and has no call or construct signatures, or
    //   - a JS expando object literal or a rest type, or
    //   - a reverse mapped type with a source for which one of the above is true.
    // Go: checker/relater.go:4656 isObjectTypeWithInferableIndex
    pub fn is_object_type_with_inferable_index(&mut self, t: TypeId) -> bool {
        if self.ty(t).flags.intersects(TypeFlags::INTERSECTION) {
            return (0..self.ty(t).types().len())
                .all(|i| self.is_object_type_with_inferable_index(self.type_at(t, i)));
        }
        let symbol = self.ty(t).symbol;
        let object_flags = self.ty(t).object_flags;
        symbol.is_some()
            && self.sym(symbol).flags.intersects(
                SymbolFlags::OBJECT_LITERAL
                    | SymbolFlags::TYPE_LITERAL
                    | SymbolFlags::ENUM
                    | SymbolFlags::VALUE_MODULE,
            )
            && !self.sym(symbol).flags.intersects(SymbolFlags::CLASS)
            && !self.type_has_call_or_construct_signatures(t)
            || object_flags.intersects(ObjectFlags::JS_LITERAL | ObjectFlags::OBJECT_REST_TYPE)
            || object_flags.intersects(ObjectFlags::REVERSE_MAPPED) && {
                let reverse_source = self.ty(t).as_reverse_mapped_type().source;
                self.is_object_type_with_inferable_index(reverse_source)
            }
    }

    // Go: checker/relater.go:4666 membersRelatedToIndexInfo
    pub fn members_related_to_index_info(
        &mut self,
        r: &Rc<RefCell<Relater>>,
        source: TypeId,
        target_info: IndexInfoId,
        report_errors: bool,
        intersection_state: IntersectionState,
    ) -> Ternary {
        let mut result = Ternary::TRUE;
        let key_type = self.index_info(target_info).key_type;
        let target_value_type = self.index_info(target_info).value_type;
        let props: SharedList<SymbolId>;
        if self.ty(source).flags.intersects(TypeFlags::INTERSECTION) {
            props = self.get_properties_of_union_or_intersection_type(source);
        } else {
            props = self.get_properties_of_object_type(source);
        }
        for &prop in &props {
            // Skip over ignored JSX and symbol-named members
            if self.is_ignored_jsx_property(source, prop) {
                continue;
            }
            let literal = self.get_literal_type_from_property(
                prop,
                TypeFlags::STRING_OR_NUMBER_LITERAL_OR_UNIQUE,
                false,
            );
            if self.is_applicable_index_type(literal, key_type) {
                let prop_type = self.get_non_missing_type_of_symbol(prop);
                let t: TypeId;
                if self.exact_optional_property_types
                    || self.ty(prop_type).flags.intersects(TypeFlags::UNDEFINED)
                    || key_type == self.number_type
                    || !self.sym(prop).flags.intersects(SymbolFlags::OPTIONAL)
                {
                    t = prop_type;
                } else {
                    t = self.get_type_with_facts(prop_type, TypeFacts::NE_UNDEFINED);
                }
                let related = self.is_related_to_ex(
                    r,
                    t,
                    target_value_type,
                    RecursionFlags::BOTH,
                    report_errors,
                    None, /*headMessage*/
                    intersection_state,
                );
                if related == Ternary::FALSE {
                    if report_errors {
                        let prop_str = self.symbol_to_string(prop);
                        self.report_error(
                            r,
                            diag::Property_0_is_incompatible_with_index_signature,
                            args![prop_str],
                        );
                    }
                    return Ternary::FALSE;
                }
                result &= related;
            }
        }
        for info in self.get_index_infos_of_type(source) {
            let info_key_type = self.index_info(info).key_type;
            if self.is_applicable_index_type(info_key_type, key_type) {
                let related = self.index_info_related_to(
                    r,
                    info,
                    target_info,
                    report_errors,
                    intersection_state,
                );
                if !(related != Ternary::FALSE) {
                    return Ternary::FALSE;
                }
                result &= related;
            }
        }
        result
    }

    // Go: checker/relater.go:4710 indexInfoRelatedTo
    pub fn index_info_related_to(
        &mut self,
        r: &Rc<RefCell<Relater>>,
        source_info: IndexInfoId,
        target_info: IndexInfoId,
        report_errors: bool,
        intersection_state: IntersectionState,
    ) -> Ternary {
        let source_value_type = self.index_info(source_info).value_type;
        let target_value_type = self.index_info(target_info).value_type;
        let related = self.is_related_to_ex(
            r,
            source_value_type,
            target_value_type,
            RecursionFlags::BOTH,
            report_errors,
            None, /*headMessage*/
            intersection_state,
        );
        if related == Ternary::FALSE && report_errors {
            let source_key_type = self.index_info(source_info).key_type;
            let target_key_type = self.index_info(target_info).key_type;
            if source_key_type == target_key_type {
                let key_str = self.type_to_string_exported(source_key_type);
                self.report_error(
                    r,
                    diag::X_0_index_signatures_are_incompatible,
                    args![key_str],
                );
            } else {
                let source_str = self.type_to_string_exported(source_key_type);
                let target_str = self.type_to_string_exported(target_key_type);
                self.report_error(
                    r,
                    diag::X_0_and_1_index_signatures_are_incompatible,
                    args![source_str, target_str],
                );
            }
        }
        related
    }

    // Go: checker/relater.go:4722 indexSignaturesIdenticalTo
    pub fn index_signatures_identical_to(
        &mut self,
        r: &Rc<RefCell<Relater>>,
        source: TypeId,
        target: TypeId,
    ) -> Ternary {
        let source_infos = self.get_index_infos_of_type(source);
        let target_infos = self.get_index_infos_of_type(target);
        if source_infos.len() != target_infos.len() {
            return Ternary::FALSE;
        }
        for &target_info in &target_infos {
            let target_key_type = self.index_info(target_info).key_type;
            let source_info = self.get_index_info_of_type(source, target_key_type);
            if !(source_info.is_some() && {
                let source_value_type = self.index_info(source_info).value_type;
                let target_value_type = self.index_info(target_info).value_type;
                self.is_related_to(
                    r,
                    source_value_type,
                    target_value_type,
                    RecursionFlags::BOTH,
                    false,
                ) != Ternary::FALSE
                    && self.index_info(source_info).is_readonly
                        == self.index_info(target_info).is_readonly
            }) {
                return Ternary::FALSE;
            }
        }
        Ternary::TRUE
    }

    // Go: checker/relater.go:4737 reportErrorResults
    pub fn report_error_results(
        &mut self,
        r: &Rc<RefCell<Relater>>,
        original_source: TypeId,
        original_target: TypeId,
        mut source: TypeId,
        mut target: TypeId,
        head_message: Option<&'static Message>,
    ) {
        let source_has_base = self
            .get_single_base_for_non_augmenting_subtype(original_source)
            .is_some();
        let target_has_base = self
            .get_single_base_for_non_augmenting_subtype(original_target)
            .is_some();
        if self.ty(original_source).alias.is_some() || source_has_base {
            source = original_source;
        }
        if self.ty(original_target).alias.is_some() || target_has_base {
            target = original_target;
        }
        if self.ty(source).flags.intersects(TypeFlags::OBJECT)
            && self.ty(target).flags.intersects(TypeFlags::OBJECT)
        {
            self.try_elaborate_array_like_errors(r, source, target, true /*reportErrors*/);
        }
        let source_flags = self.ty(source).flags;
        let target_flags = self.ty(target).flags;
        if source_flags.intersects(TypeFlags::OBJECT)
            && target_flags.intersects(TypeFlags::PRIMITIVE)
        {
            self.try_elaborate_errors_for_primitives_and_objects(r, source, target);
        } else if self.ty(source).symbol.is_some()
            && source_flags.intersects(TypeFlags::OBJECT)
            && self.global_object_type == source
        {
            self.report_error(r, diag::The_Object_type_is_assignable_to_very_few_other_types_Did_you_mean_to_use_the_any_type_instead, args![]);
        } else if self
            .ty(source)
            .object_flags
            .intersects(ObjectFlags::JSX_ATTRIBUTES)
            && target_flags.intersects(TypeFlags::INTERSECTION)
        {
            let target_types = self.ty(target).types_list();
            let error_node = r.borrow().error_node;
            // PORT: Go `JsxNames.IntrinsicAttributes` and
            // `JsxNames.IntrinsicClassAttributes` are these string constants.
            let intrinsic_attributes = self.get_jsx_type("IntrinsicAttributes", error_node);
            let intrinsic_class_attributes =
                self.get_jsx_type("IntrinsicClassAttributes", error_node);
            if !self.is_error_type(intrinsic_attributes)
                && !self.is_error_type(intrinsic_class_attributes)
                && (target_types.contains(&intrinsic_attributes)
                    || target_types.contains(&intrinsic_class_attributes))
            {
                return;
            }
        } else if self
            .ty(original_target)
            .flags
            .intersects(TypeFlags::INTERSECTION)
            && self
                .ty(original_target)
                .object_flags
                .intersects(ObjectFlags::IS_NEVER_INTERSECTION)
        {
            let mut message = diag::The_intersection_0_was_reduced_to_never_because_property_1_has_conflicting_types_in_some_constituents;
            let props = self.get_properties_of_union_or_intersection_type(original_target);
            let mut prop = SymbolId::NIL;
            for &p in &props {
                if self.is_discriminant_with_never_type(p) {
                    prop = p;
                    break;
                }
            }
            if prop.is_nil() {
                message = diag::The_intersection_0_was_reduced_to_never_because_property_1_exists_in_multiple_constituents_and_is_private_in_some;
                let props = self.get_properties_of_union_or_intersection_type(original_target);
                for &p in &props {
                    if self.is_conflicting_private_property(p) {
                        prop = p;
                        break;
                    }
                }
            }
            if prop.is_some() {
                let target_str = self.type_to_string_ex(
                    original_target,
                    Node::NIL, /*enclosingDeclaration*/
                    TypeFormatFlags::NO_TYPE_REDUCTION,
                    None,
                );
                let prop_str = self.symbol_to_string(prop);
                self.report_error(r, message, args![target_str, prop_str]);
            }
        }
        self.report_relation_error(r, head_message, source, target);
        let source_symbol = self.ty(source).symbol;
        if self.ty(source).flags.intersects(TypeFlags::TYPE_PARAMETER)
            && source_symbol.is_some()
            && !self.sym(source_symbol).declarations.is_empty()
            && self.get_constraint_of_type(source).is_nil()
        {
            let synthetic_param = self.clone_type_parameter(source);
            let mapper = self.new_simple_type_mapper(source, synthetic_param);
            let constraint = self.instantiate_type(target, mapper);
            self.ty_mut(synthetic_param)
                .as_type_parameter_mut()
                .constraint = constraint;
            if self.has_non_circular_base_constraint(synthetic_param) {
                let target_constraint_string = self.type_to_string_exported(target);
                let declaration = self.sym(source_symbol).declarations[0];
                let d = new_diagnostic_for_node(
                    declaration,
                    diag::This_type_parameter_might_need_an_extends_0_constraint,
                    args![target_constraint_string],
                );
                r.borrow_mut().related_info.push(d);
            }
        }
    }

    // Go: checker/relater.go:4783 reportRelationError
    pub fn report_relation_error(
        &mut self,
        r: &Rc<RefCell<Relater>>,
        mut message: Option<&'static Message>,
        source: TypeId,
        target: TypeId,
    ) {
        // Effect-TS/tsgo patch 004: collect the relation error for Effect diagnostics.
        if self.compiler_options.effect.is_some() {
            let error_node = r.borrow().error_node;
            if error_node.is_some() {
                let sf = get_source_file_of_node(error_node);
                if sf.is_some() {
                    self.effect_relation_errors.entry(sf).or_default().push(
                        crate::effect::RelationError {
                            source,
                            target,
                            error_node,
                        },
                    );
                }
            }
        }
        let (source_type, target_type) = self.get_type_names_for_error_display(source, target);
        let mut generalized_source = source;
        let mut generalized_source_type = source_type.clone();
        // Don't generalize on 'never' - we really want the original type
        // to be displayed for use-cases like 'assertNever'.
        if !self.ty(target).flags.intersects(TypeFlags::NEVER)
            && self.is_literal_type(source)
            && !self.type_could_have_top_level_singleton_types(target)
        {
            generalized_source = self.get_base_type_of_literal_type(source);
            generalized_source_type = self.get_type_name_for_error_display(generalized_source);
        }
        // If `target` is of indexed access type (and `source` it is not), we use the object type of `target` for better error reporting
        let target_flags: TypeFlags;
        if self.ty(target).flags.intersects(TypeFlags::INDEXED_ACCESS)
            && !self.ty(source).flags.intersects(TypeFlags::INDEXED_ACCESS)
        {
            let object_type = self.ty(target).as_indexed_access_type().object_type;
            target_flags = self.ty(object_type).flags;
        } else {
            target_flags = self.ty(target).flags;
        }
        if target_flags.intersects(TypeFlags::TYPE_PARAMETER)
            && target != self.marker_super_type_for_check
            && target != self.marker_sub_type_for_check
        {
            let constraint = self.get_base_constraint_of_type(target);
            if self.is_distributed_type_parameter(target) && {
                let target_constraint = self.ty(target).as_type_parameter().constraint;
                self.is_type_assignable_to(generalized_source, target_constraint)
            } {
                self.report_error(
                    r,
                    diag::X_0_is_only_assignable_to_the_non_distributed_1_but_1_has_been_distributed_here,
                    args![generalized_source_type, target_type],
                );
            } else if constraint.is_some()
                && self.is_type_assignable_to(generalized_source, constraint)
            {
                let constraint_str = self.type_to_string_exported(constraint);
                self.report_error(
                    r,
                    diag::X_0_is_assignable_to_the_constraint_of_type_1_but_1_could_be_instantiated_with_a_different_subtype_of_constraint_2,
                    args![generalized_source_type, target_type, constraint_str],
                );
            } else if constraint.is_some() && self.is_type_assignable_to(source, constraint) {
                let constraint_str = self.type_to_string_exported(constraint);
                self.report_error(
                    r,
                    diag::X_0_is_assignable_to_the_constraint_of_type_1_but_1_could_be_instantiated_with_a_different_subtype_of_constraint_2,
                    args![source_type, target_type, constraint_str],
                );
            } else {
                r.borrow_mut().error_chain = None; // Only report this error once
                self.report_error(
                    r,
                    diag::X_0_could_be_instantiated_with_an_arbitrary_type_which_could_be_unrelated_to_1,
                    args![target_type, generalized_source_type],
                );
            }
        }
        let relation = r.borrow().kind;
        if message.is_none() {
            if relation == RelationKind::Comparable {
                message = Some(diag::Type_0_is_not_comparable_to_type_1);
            } else if source_type == target_type {
                message = Some(diag::Type_0_is_not_assignable_to_type_1_Two_different_types_with_this_name_exist_but_they_are_unrelated);
            } else if self.exact_optional_property_types
                && !self
                    .get_exact_optional_unassignable_properties(source, target)
                    .is_empty()
            {
                message = Some(diag::Type_0_is_not_assignable_to_type_1_with_exactOptionalPropertyTypes_Colon_true_Consider_adding_undefined_to_the_types_of_the_target_s_properties);
            } else {
                if self.ty(source).flags.intersects(TypeFlags::STRING_LITERAL)
                    && self.ty(target).flags.intersects(TypeFlags::UNION)
                {
                    let suggested_type =
                        self.get_suggested_type_for_nonexistent_string_literal_type(source, target);
                    if suggested_type.is_some() {
                        let suggested_str = self.type_to_string_exported(suggested_type);
                        self.report_error(
                            r,
                            diag::Type_0_is_not_assignable_to_type_1_Did_you_mean_2,
                            args![generalized_source_type, target_type, suggested_str],
                        );
                        return;
                    }
                }
                message = Some(diag::Type_0_is_not_assignable_to_type_1);
            }
        } else if msg_eq(
            message,
            diag::Argument_of_type_0_is_not_assignable_to_parameter_of_type_1,
        ) && self.exact_optional_property_types
            && !self
                .get_exact_optional_unassignable_properties(source, target)
                .is_empty()
        {
            message = Some(diag::Argument_of_type_0_is_not_assignable_to_parameter_of_type_1_with_exactOptionalPropertyTypes_Colon_true_Consider_adding_undefined_to_the_types_of_the_target_s_properties);
        }
        let message = message.expect("message is set above");
        let chain_message = self.get_chain_message(r, 0);
        if msg_eq(chain_message, diag::Object_literal_may_only_specify_known_properties_and_0_does_not_exist_in_type_1)
            || msg_eq(chain_message, diag::Object_literal_may_only_specify_known_properties_but_0_does_not_exist_in_type_1_Did_you_mean_to_write_2)
        {
            // Suppress if next message is an excess property error
            return;
        } else if msg_eq(chain_message, diag::Excessive_complexity_comparing_types_0_and_1)
            || msg_eq(chain_message, diag::The_type_0_is_readonly_and_cannot_be_assigned_to_the_mutable_type_1)
        {
            // Suppress if next message is an excessive complexity/stack depth message for source and target or a readonly
            // vs. mutable error for source and target
            if self.chain_args_match(r, &[Some(generalized_source_type.as_str()), Some(target_type.as_str())]) {
                return;
            }
        } else if msg_eq(chain_message, diag::Property_0_is_missing_in_type_1_but_required_in_type_2) {
            // Suppress if next message is a missing property message for source and target and we're not
            // reporting on conversion or interface implementation
            if !is_conversion_or_interface_implementation_message(message)
                && self.chain_args_match(r, &[None, Some(generalized_source_type.as_str()), Some(target_type.as_str())])
            {
                return;
            }
        } else if msg_eq(chain_message, diag::Type_0_is_missing_the_following_properties_from_type_1_Colon_2_and_3_more)
            || msg_eq(chain_message, diag::Type_0_is_missing_the_following_properties_from_type_1_Colon_2)
        {
            // Suppress if next message is a missing property message for source and target and we're not
            // reporting on conversion or interface implementation
            if !is_conversion_or_interface_implementation_message(message)
                && self.chain_args_match(r, &[Some(generalized_source_type.as_str()), Some(target_type.as_str())])
            {
                return;
            }
        }
        self.report_error(r, message, args![generalized_source_type, target_type]);
    }

    // Go: checker/relater.go:4864 reportError
    pub fn report_error(
        &mut self,
        r: &Rc<RefCell<Relater>>,
        mut message: &'static Message,
        mut args: Vec<String>,
    ) {
        if std::ptr::eq(message, diag::Types_of_property_0_are_incompatible) {
            // Suppress if next message is an excess property error
            let chain_message = self.get_chain_message(r, 0);
            if msg_eq(chain_message, diag::Object_literal_may_only_specify_known_properties_and_0_does_not_exist_in_type_1)
                || msg_eq(chain_message, diag::Object_literal_may_only_specify_known_properties_but_0_does_not_exist_in_type_1_Did_you_mean_to_write_2)
            {
                return;
            }
            // Transform a property incompatibility message for property 'x' followed by some elaboration message
            // followed by a signature return type incompatibility message into a single return type incompatibility
            // message for 'x()' or 'x(...)'
            let mut arg = String::new();
            let chain_message = self.get_chain_message(r, 1);
            if msg_eq(
                chain_message,
                diag::Call_signatures_with_no_arguments_have_incompatible_return_types_0_and_1,
            ) {
                arg = get_property_name_arg(&args[0]) + "()";
            } else if msg_eq(
                chain_message,
                diag::Construct_signatures_with_no_arguments_have_incompatible_return_types_0_and_1,
            ) {
                arg = "new ".to_string() + &get_property_name_arg(&args[0]) + "()";
            } else if msg_eq(
                chain_message,
                diag::Call_signature_return_types_0_and_1_are_incompatible,
            ) {
                arg = get_property_name_arg(&args[0]) + "(...)";
            } else if msg_eq(
                chain_message,
                diag::Construct_signature_return_types_0_and_1_are_incompatible,
            ) {
                arg = "new ".to_string() + &get_property_name_arg(&args[0]) + "(...)";
            }
            if !arg.is_empty() {
                message = diag::The_types_returned_by_0_are_incompatible_between_these_types;
                args[0] = arg;
                let mut rb = r.borrow_mut();
                let next_next = rb
                    .error_chain
                    .as_ref()
                    .unwrap()
                    .next
                    .as_ref()
                    .unwrap()
                    .next
                    .clone();
                rb.error_chain = next_next;
            }
            // Transform a property incompatibility message for property 'x' followed by some elaboration message
            // followed by a property incompatibility message for property 'y' into a single property incompatibility
            // message for 'x.y'
            let chain_message = self.get_chain_message(r, 1);
            if msg_eq(chain_message, diag::Types_of_property_0_are_incompatible)
                || msg_eq(
                    chain_message,
                    diag::The_types_of_0_are_incompatible_between_these_types,
                )
                || msg_eq(
                    chain_message,
                    diag::The_types_returned_by_0_are_incompatible_between_these_types,
                )
            {
                let head = get_property_name_arg(&args[0]);
                let arg: String;
                {
                    let mut rb = r.borrow_mut();
                    let next = rb.error_chain.as_ref().unwrap().next.clone().unwrap();
                    let tail = get_property_name_arg(&next.args[0]);
                    arg = add_to_dotted_name(&head, &tail);
                    rb.error_chain = next.next.clone();
                }
                if std::ptr::eq(message, diag::Types_of_property_0_are_incompatible) {
                    message = diag::The_types_of_0_are_incompatible_between_these_types;
                }
                self.report_error(r, message, args![arg]);
                return;
            }
        }
        let mut rb = r.borrow_mut();
        let next = rb.error_chain.take();
        rb.error_chain = Some(Rc::new(ErrorChain {
            next,
            message,
            args,
        }));
    }
}

// Go: checker/relater.go:4912 addToDottedName
pub fn add_to_dotted_name(head: &str, tail: &str) -> String {
    let mut head = head.to_string();
    if head.starts_with("new ") {
        head = "(".to_string() + &head + ")";
    }
    let mut pos = 0;
    loop {
        if tail[pos..].starts_with('(') {
            pos += 1;
        } else if tail[pos..].starts_with("new ") {
            pos += 4;
        } else {
            break;
        }
    }
    let prefix = &tail[..pos];
    let suffix = &tail[pos..];
    if suffix.starts_with('[') {
        return prefix.to_string() + &head + suffix;
    }
    prefix.to_string() + &head + "." + suffix
}

impl Checker {
    // Go: checker/relater.go:4934 getChainMessage
    pub fn get_chain_message(
        &self,
        r: &Rc<RefCell<Relater>>,
        mut index: i32,
    ) -> Option<&'static Message> {
        let mut e = r.borrow().error_chain.clone();
        loop {
            let Some(chain) = e else {
                return None;
            };
            if index == 0 {
                return Some(chain.message);
            }
            e = chain.next.clone();
            index -= 1;
        }
    }

    // Return true if the arguments of the first entry on the error chain match the
    // given arguments (where nil acts as a wildcard).
    // PORT: Go `args ...any` holds strings or nil; a nil wildcard is `None`.
    // Go panics on a nil chain or a short args list; so does this port.
    // Go: checker/relater.go:4950 chainArgsMatch
    pub fn chain_args_match(&self, r: &Rc<RefCell<Relater>>, args: &[Option<&str>]) -> bool {
        let rb = r.borrow();
        let chain = rb.error_chain.as_ref().unwrap();
        for (i, a) in args.iter().enumerate() {
            if let Some(a) = a {
                if *a != chain.args[i] {
                    return false;
                }
            }
        }
        true
    }
}

// PORT: Go takes `arg any` and asserts a string; diagnostic args are
// `String` in this port.
// Go: checker/relater.go:4959 getPropertyNameArg
pub fn get_property_name_arg(arg: &str) -> String {
    let s = arg.as_bytes();
    if !s.is_empty() && (s[0] == b'"' || s[0] == b'\'' || s[0] == b'`') {
        return "[".to_string() + arg + "]";
    }
    arg.to_string()
}

// Go: checker/relater.go:4967 isConversionOrInterfaceImplementationMessage
pub fn is_conversion_or_interface_implementation_message(message: &'static Message) -> bool {
    std::ptr::eq(message, diag::Class_0_incorrectly_implements_interface_1)
        || std::ptr::eq(message, diag::Class_0_incorrectly_implements_class_1_Did_you_mean_to_extend_1_and_inherit_its_members_as_a_subclass)
        || std::ptr::eq(message, diag::Conversion_of_type_0_to_type_1_may_be_a_mistake_because_neither_type_sufficiently_overlaps_with_the_other_If_this_was_intentional_convert_the_expression_to_unknown_first)
        || std::ptr::eq(message, diag::Its_instance_type_0_is_not_a_valid_JSX_element)
        || std::ptr::eq(message, diag::Its_return_type_0_is_not_a_valid_JSX_element)
        || std::ptr::eq(message, diag::Its_element_type_0_is_not_a_valid_JSX_element)
}

// Go: checker/relater.go:4976 chainDepth
pub fn chain_depth(chain: &Option<Rc<ErrorChain>>) -> i32 {
    let mut depth = 0;
    let mut chain = chain.clone();
    while let Some(c) = chain {
        depth += 1;
        chain = c.next.clone();
    }
    depth
}

impl Checker {
    // An object type S is considered to be derived from an object type T if
    // S is a union type and every constituent of S is derived from T,
    // T is a union type and S is derived from at least one constituent of T, or
    // S is an intersection type and some constituent of S is derived from T, or
    // S is a type variable with a base constraint that is derived from T, or
    // T is {} and S is an object-like type (ensuring {} is less derived than Object), or
    // T is one of the global types Object and Function and S is a subtype of T, or
    // T occurs directly or indirectly in an 'extends' clause of S.
    // Note that this check ignores type parameters and only considers the
    // inheritance hierarchy.
    // Go: checker/relater.go:4995 isTypeDerivedFrom
    pub fn is_type_derived_from(&mut self, source: TypeId, target: TypeId) -> bool {
        let source_flags = self.ty(source).flags;
        let target_flags = self.ty(target).flags;
        if source_flags.intersects(TypeFlags::UNION) {
            (0..self.ty(source).types().len())
                .all(|i| self.is_type_derived_from(self.type_at(source, i), target))
        } else if target_flags.intersects(TypeFlags::UNION) {
            (0..self.ty(target).types().len())
                .any(|i| self.is_type_derived_from(source, self.type_at(target, i)))
        } else if source_flags.intersects(TypeFlags::INTERSECTION) {
            (0..self.ty(source).types().len())
                .any(|i| self.is_type_derived_from(self.type_at(source, i), target))
        } else if source_flags.intersects(TypeFlags::INSTANTIABLE_NON_PRIMITIVE) {
            let mut constraint = self.get_base_constraint_of_type(source);
            if constraint.is_nil() {
                constraint = self.unknown_type;
            }
            self.is_type_derived_from(constraint, target)
        } else if self.is_empty_anonymous_object_type(target) {
            source_flags.intersects(TypeFlags::OBJECT | TypeFlags::NON_PRIMITIVE)
        } else if target == self.global_object_type {
            source_flags.intersects(TypeFlags::OBJECT | TypeFlags::NON_PRIMITIVE)
                && !self.is_empty_anonymous_object_type(source)
        } else if target == self.global_function_type {
            source_flags.intersects(TypeFlags::OBJECT) && self.is_function_object_type(source)
        } else {
            let target_type = self.get_target_type(target);
            self.has_base_type(source, target_type)
                || (self.is_array_type(target) && !self.is_readonly_array_type(target) && {
                    let global_readonly_array_type = self.global_readonly_array_type;
                    self.is_type_derived_from(source, global_readonly_array_type)
                })
        }
    }

    // Go: checker/relater.go:5026 isDistributionDependent
    // PERF: the root keeps the answer of the first walk
    // (`ConditionalRoot::distribution_dependent`), and a repeat call returns
    // it. Go walks the result type nodes on every call (vue-macros: 590,519
    // calls on 93 roots). The walk reads the AST and the symbols that
    // `get_symbol_from_type_reference` and `get_resolved_symbol` resolve; the
    // first walk resolves and caches them (Go's effects, as Go makes them),
    // and the cached links never change, so a repeat walk has no effect and
    // gives the same answer. That needs Go to keep one writer of
    // `resolvedSymbol` on TypeReference nodes: a pin bump checks it
    // (UPSTREAM.md "Pin bump checks").
    pub fn is_distribution_dependent(&mut self, root: &Rc<RefCell<ConditionalRoot>>) -> bool {
        let (is_distributive, check_type, node, known) = {
            let rb = root.borrow();
            (
                rb.is_distributive,
                rb.check_type,
                rb.node,
                rb.distribution_dependent,
            )
        };
        if !is_distributive {
            return false;
        }
        if let Some(known) = known {
            return known;
        }
        let result = self.is_type_parameter_possibly_referenced(check_type, node.true_type())
            || self.is_type_parameter_possibly_referenced(check_type, node.false_type());
        root.borrow_mut().distribution_dependent = Some(result);
        result
    }

    // Go: checker/relater.go:5030 traceUnionsOrIntersectionsTooLarge
    pub fn trace_unions_or_intersections_too_large(
        &mut self,
        _r: &Rc<RefCell<Relater>>,
        source: TypeId,
        target: TypeId,
    ) {
        let Some(tr) = self.tracer else {
            return;
        };
        if self
            .ty(source)
            .flags
            .intersects(TypeFlags::UNION_OR_INTERSECTION)
            && self
                .ty(target)
                .flags
                .intersects(TypeFlags::UNION_OR_INTERSECTION)
        {
            if (self.ty(source).object_flags & self.ty(target).object_flags)
                .intersects(ObjectFlags::PRIMITIVE_UNION)
            {
                // There's a fast path for comparing primitive unions
                return;
            }
            let source_size = self.ty(source).types().len();
            let target_size = self.ty(target).types().len();
            if source_size * target_size > 1_000_000 {
                tr.instant(
                    crate::tracing::Phase::CheckTypes,
                    "traceUnionsOrIntersectionsTooLarge_DepthLimit",
                    vec![
                        ("sourceId", source.into()),
                        ("sourceSize", source_size.into()),
                        ("targetId", target.into()),
                        ("targetSize", target_size.into()),
                    ],
                );
            }
        }
    }
}

#[cfg(test)]
mod distribution_dependent_tests {
    use super::*;
    use crate::checker::utilities_p1::union_sort_tests::with_alias_types;

    /// `is_distribution_dependent` gives the answer of Go's walk
    /// (relater.go:5026, checker.go:22823) on the first call and on a repeat,
    /// and the root keeps it after the first call. A root that is not
    /// distributive keeps nothing. The roots: `T` in the true type, `T` in
    /// the false type, `T` in neither, a `typeof` of a value outside the
    /// alias (resolved by the first walk), and `[T]`, not distributive.
    #[test]
    fn the_root_keeps_the_distribution_dependence() {
        const SOURCE: &str = r#"
declare const v: string;
type A<T> = T extends string ? T[] : never;
type B<T> = T extends string ? number : { x: T };
type C<T> = T extends string ? number : boolean;
type D<T> = T extends string ? typeof v : 0;
type E<T> = [T] extends [string] ? T : never;
"#;
        let got = with_alias_types(SOURCE, |c, types| {
            types
                .iter()
                .map(|&t| {
                    let root = c.ty(t).as_conditional_type().root.clone();
                    let before = root.borrow().distribution_dependent;
                    let first = c.is_distribution_dependent(&root);
                    let kept = root.borrow().distribution_dependent;
                    let (check_type, node) = {
                        let rb = root.borrow();
                        (rb.check_type, rb.node)
                    };
                    let walk = c
                        .is_type_parameter_possibly_referenced(check_type, node.true_type())
                        || c.is_type_parameter_possibly_referenced(check_type, node.false_type());
                    let repeat = c.is_distribution_dependent(&root);
                    (before, first, kept, walk, repeat)
                })
                .collect::<Vec<_>>()
        });
        assert_eq!(
            got,
            vec![
                (None, true, Some(true), true, true),
                (None, true, Some(true), true, true),
                (None, false, Some(false), false, false),
                (None, false, Some(false), false, false),
                (None, false, None, true, false),
            ]
        );
    }
}
