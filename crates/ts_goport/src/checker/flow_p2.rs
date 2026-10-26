//! Port of typescript-go `internal/checker/flow.go` lines 953-1854
//! (`getInstanceType` through `getReferenceCandidate`).
//!
//! PORT: Go `*FlowState` is `&mut FlowState` (see `FlowState`), so flow
//! functions take `f: &mut FlowState`.

use crate::prelude::*;
use std::borrow::Cow;
use std::sync::LazyLock;

impl Checker {
    // Go: checker/flow.go:966 getInstanceType
    pub fn get_instance_type(&mut self, constructor_type: TypeId) -> TypeId {
        let prototype_property_type =
            self.get_type_of_property_of_type(constructor_type, "prototype");
        if prototype_property_type.is_some() && !self.is_type_any(prototype_property_type) {
            return prototype_property_type;
        }
        let construct_signatures =
            self.get_signatures_of_type(constructor_type, SignatureKind::CONSTRUCT);
        if !construct_signatures.is_empty() {
            let mut types = Vec::with_capacity(construct_signatures.len());
            for signature in construct_signatures {
                let erased = self.get_erased_signature(signature);
                types.push(self.get_return_type_of_signature(erased));
            }
            return self.get_union_type(&types);
        }
        // We use the empty object type to indicate we don't know the type of objects created by
        // this constructor function.
        self.empty_object_type
    }

    // Go: checker/flow.go:982 narrowTypeByPrivateIdentifierInInExpression
    // PORT: Go `*ast.BinaryExpression` is the binary expression `Node`.
    pub fn narrow_type_by_private_identifier_in_in_expression(
        &mut self,
        f: &mut FlowState,
        t: TypeId,
        expr: Node,
        assume_true: bool,
    ) -> TypeId {
        let target = self.get_reference_candidate(expr.right());
        let reference = f.reference;
        if !self.is_matching_flow_reference(f, target) {
            return t;
        }
        let symbol = self.get_symbol_for_private_identifier_expression(expr.left());
        if symbol.is_nil() {
            return t;
        }
        let class_symbol = self.sym(symbol).parent;
        let value_declaration = self.sym(symbol).value_declaration;
        let target_type = if has_static_modifier(value_declaration) {
            self.get_type_of_symbol(class_symbol)
        } else {
            self.get_declared_type_of_symbol(class_symbol)
        };
        self.get_narrowed_type(t, target_type, assume_true, true /*checkDerived*/)
    }

    // Go: checker/flow.go:1001 narrowTypeByInKeyword
    pub fn narrow_type_by_in_keyword(
        &mut self,
        f: &mut FlowState,
        t: TypeId,
        name_type: TypeId,
        assume_true: bool,
    ) -> TypeId {
        // PERF: namekeys1. The interned name, so the lookup in each
        // constituent (`is_type_presence_possible`) goes by id.
        let name = self.get_property_name_from_type_as_name(name_type);
        let is_known_property = self.some_type(t, &mut |c: &mut Checker, t: TypeId| {
            c.is_type_presence_possible(t, &name, true /*assumeTrue*/)
        });
        if is_known_property {
            // If the check is for a known property (i.e. a property declared in some constituent of
            // the target type), we filter the target type by presence of absence of the property.
            return self.filter_type(t, &mut |c: &mut Checker, t: TypeId| {
                c.is_type_presence_possible(t, &name, assume_true)
            });
        }
        if assume_true {
            // If the check is for an unknown property, we intersect the target type with `Record<X, unknown>`,
            // where X is the name of the property.
            let get_global_record_symbol = self.get_global_record_symbol.clone();
            let record_symbol = get_global_record_symbol(self);
            if record_symbol.is_some() {
                let unknown_type = self.unknown_type;
                let record = self.get_type_alias_instantiation(
                    record_symbol,
                    &[name_type, unknown_type],
                    None,
                );
                return self.get_intersection_type(&[t, record]);
            }
        }
        t
    }

    // Go: checker/flow.go:1024 isTypePresencePossible
    pub fn is_type_presence_possible(
        &mut self,
        t: TypeId,
        prop_name: &Name,
        assume_true: bool,
    ) -> bool {
        let prop = self.get_property_of_type(t, prop_name);
        if prop.is_some() {
            return self.sym(prop).flags.intersects(SymbolFlags::OPTIONAL)
                || self.sym(prop).check_flags.intersects(CheckFlags::PARTIAL)
                || assume_true;
        }
        self.get_applicable_index_info_for_name(t, prop_name)
            .is_some()
            || !assume_true
    }

    // Go: checker/flow.go:1032 narrowTypeByOptionalChainContainment
    pub fn narrow_type_by_optional_chain_containment(
        &mut self,
        f: &mut FlowState,
        t: TypeId,
        operator: SyntaxKind,
        value: Node,
        assume_true: bool,
    ) -> TypeId {
        // We are in a branch of obj?.foo === value (or any one of the other equality operators). We narrow obj as follows:
        // When operator is === and type of value excludes undefined, null and undefined is removed from type of obj in true branch.
        // When operator is !== and type of value excludes undefined, null and undefined is removed from type of obj in false branch.
        // When operator is == and type of value excludes null and undefined, null and undefined is removed from type of obj in true branch.
        // When operator is != and type of value excludes null and undefined, null and undefined is removed from type of obj in false branch.
        // When operator is === and type of value is undefined, null and undefined is removed from type of obj in false branch.
        // When operator is !== and type of value is undefined, null and undefined is removed from type of obj in true branch.
        // When operator is == and type of value is null or undefined, null and undefined is removed from type of obj in false branch.
        // When operator is != and type of value is null or undefined, null and undefined is removed from type of obj in true branch.
        let equals_operator = operator == SyntaxKind::EqualsEqualsToken
            || operator == SyntaxKind::EqualsEqualsEqualsToken;
        let nullable_flags = if operator == SyntaxKind::EqualsEqualsToken
            || operator == SyntaxKind::ExclamationEqualsToken
        {
            TypeFlags::NULLABLE
        } else {
            TypeFlags::UNDEFINED
        };
        let value_type = self.get_type_of_expression(value);
        // Note that we include any and unknown in the exclusion test because their domain includes null and undefined.
        let remove_nullable = equals_operator != assume_true
            && self.every_type(value_type, &mut |c: &mut Checker, t: TypeId| {
                c.ty(t).flags.intersects(nullable_flags)
            })
            || equals_operator == assume_true
                && self.every_type(value_type, &mut |c: &mut Checker, t: TypeId| {
                    !c.ty(t)
                        .flags
                        .intersects(TypeFlags::ANY_OR_UNKNOWN | nullable_flags)
                });
        if remove_nullable {
            return self.get_adjusted_type_with_facts(t, TypeFacts::NE_UNDEFINED_OR_NULL);
        }
        t
    }

    // Go: checker/flow.go:1059 getTypeAtSwitchClause
    // PERF: takes the flow node that the caller has read (`flow.get_flow()`).
    pub fn get_type_at_switch_clause(
        &mut self,
        f: &mut FlowState,
        flow_node: &FlowNode,
    ) -> FlowType {
        let data = flow_node.as_flow_switch_clause_data();
        let expr = skip_parentheses(data.switch_statement.expression());
        let flow_type = self.get_type_at_flow_node(f, flow_node.antecedent);
        let mut t = flow_type.t;
        let reference = f.reference;
        if self.is_matching_flow_reference(f, expr) {
            t = self.narrow_type_by_switch_on_discriminant(t, &data);
        } else if expr.kind() == SyntaxKind::TypeOfExpression
            && self.is_matching_flow_reference(f, expr.expression())
        {
            t = self.narrow_type_by_switch_on_type_of(t, &data);
        } else if expr.kind() == SyntaxKind::TrueKeyword {
            t = self.narrow_type_by_switch_on_true(f, t, &data);
        } else {
            if self.strict_null_checks {
                if self.optional_chain_contains_reference(expr, reference) {
                    t = self.narrow_type_by_switch_optional_chain_containment(
                        t,
                        &data,
                        &mut |c: &mut Checker, t: TypeId| {
                            !c.ty(t)
                                .flags
                                .intersects(TypeFlags::UNDEFINED | TypeFlags::NEVER)
                        },
                    );
                } else if is_type_of_expression(expr)
                    && self.optional_chain_contains_reference(expr.expression(), reference)
                {
                    t = self.narrow_type_by_switch_optional_chain_containment(
                        t,
                        &data,
                        &mut |c: &mut Checker, t: TypeId| {
                            !(c.ty(t).flags.intersects(TypeFlags::NEVER)
                                || c.ty(t).flags.intersects(TypeFlags::STRING_LITERAL)
                                    && c.get_string_literal_value_ref(t) == "undefined")
                        },
                    );
                }
            }
            let access = self.get_discriminant_property_access(f, expr, t);
            if access.is_some() {
                t = self.narrow_type_by_switch_on_discriminant_property(t, access, &data);
            }
        }
        self.new_flow_type(t, flow_type.incomplete)
    }

    // Go: checker/flow.go:1091 narrowTypeBySwitchOnDiscriminant
    pub fn narrow_type_by_switch_on_discriminant(
        &mut self,
        t: TypeId,
        data: &FlowSwitchClauseData,
    ) -> TypeId {
        // We only narrow if all case expressions specify
        // values with unit types, except for the case where
        // `type` is unknown. In this instance we map object
        // types to the nonPrimitive type and narrow with that.
        let switch_types = self.get_switch_clause_types(data.switch_statement);
        if switch_types.is_empty() {
            return t;
        }
        let clause_types: Vec<TypeId> =
            switch_types[data.clause_start as usize..data.clause_end as usize].to_vec();
        let never_type = self.never_type;
        let has_default_clause =
            data.clause_start == data.clause_end || clause_types.contains(&never_type);
        if self.ty(t).flags.intersects(TypeFlags::UNKNOWN) && !has_default_clause {
            let mut ground_clause_types: Option<Vec<TypeId>> = None;
            for (i, &s) in clause_types.iter().enumerate() {
                let s_flags = self.ty(s).flags;
                if s_flags.intersects(TypeFlags::PRIMITIVE | TypeFlags::NON_PRIMITIVE) {
                    if let Some(ground) = ground_clause_types.as_mut() {
                        ground.push(s);
                    }
                } else if s_flags.intersects(TypeFlags::OBJECT) {
                    if ground_clause_types.is_none() {
                        ground_clause_types = Some(clause_types[..i].to_vec());
                    }
                    let non_primitive_type = self.non_primitive_type;
                    ground_clause_types
                        .as_mut()
                        .unwrap()
                        .push(non_primitive_type);
                } else {
                    return t;
                }
            }
            return match ground_clause_types {
                None => self.get_union_type(&clause_types),
                Some(ground) => self.get_union_type(&ground),
            };
        }
        let discriminant_type = self.get_union_type(&clause_types);
        let mut case_type = TypeId::NIL;
        if self
            .ty(discriminant_type)
            .flags
            .intersects(TypeFlags::NEVER)
        {
            case_type = self.never_type;
        } else {
            if self
                .ty(discriminant_type)
                .flags
                .intersects(TypeFlags::PRIMITIVE)
                && self.is_uniform_union_type(t)
            {
                let regular_type = self.get_regular_type_of_literal_type(discriminant_type);
                if self.union_contains_type(t, regular_type, false /*matchSymbol*/) {
                    case_type = regular_type;
                }
            }
            if case_type.is_nil() {
                let filtered = self.filter_type(t, &mut |c: &mut Checker, t: TypeId| {
                    c.are_types_comparable(discriminant_type, t)
                });
                case_type = self.replace_primitives_with_literals(filtered, discriminant_type);
            }
        }
        if !has_default_clause {
            return case_type;
        }
        let default_type = self.filter_type(t, &mut |c: &mut Checker, t: TypeId| {
            if !c.is_unit_like_type(t) {
                return true;
            }
            let mut u = c.undefined_type;
            if !c.ty(t).flags.intersects(TypeFlags::UNDEFINED) {
                let unit = c.extract_unit_type(t);
                u = c.get_regular_type_of_literal_type(unit);
            }
            !switch_types
                .iter()
                .any(|&st| c.is_unit_type(st) && c.are_types_comparable(st, u))
        });
        if self.ty(case_type).flags.intersects(TypeFlags::NEVER) {
            return default_type;
        }
        self.get_union_type(&[case_type, default_type])
    }

    // Go: checker/flow.go:1157 narrowTypeBySwitchOnTypeOf
    pub fn narrow_type_by_switch_on_type_of(
        &mut self,
        t: TypeId,
        data: &FlowSwitchClauseData,
    ) -> TypeId {
        let witnesses = self.get_switch_clause_type_of_witnesses(data.switch_statement);
        // PORT: Go distinguishes a nil witness slice (a non-string case exists) from an
        // empty one (no clauses). `SwitchStatementLinks.witnesses` is a `Vec`, so both
        // read as empty. With no clauses Go takes the default-clause path with no
        // facts, which keeps every constituent and so also yields `t`.
        if witnesses.is_empty() {
            return t;
        }
        let clauses = data
            .switch_statement
            .case_block()
            .clauses()
            .nodes()
            .to_vec();
        // Equal start and end denotes implicit fallthrough; undefined marks explicit default clause.
        let default_index = clauses
            .iter()
            .position(|clause| clause.kind() == SyntaxKind::DefaultClause)
            .map_or(-1, |i| i as i32);
        let clause_start = data.clause_start;
        let clause_end = data.clause_end;
        let has_default_clause = clause_start == clause_end
            || (default_index >= clause_start && default_index < clause_end);
        if has_default_clause {
            // In the default clause we filter constituents down to those that are not-equal to all handled cases.
            let not_equal_facts =
                self.get_not_equal_facts_from_typeof_switch(clause_start, clause_end, &witnesses);
            return self.filter_type(t, &mut |c: &mut Checker, t: TypeId| {
                c.get_type_facts(t, not_equal_facts) == not_equal_facts
            });
        }
        // In the non-default cause we create a union of the type narrowed by each of the listed cases.
        let clause_witnesses: Vec<String> =
            witnesses[clause_start as usize..clause_end as usize].to_vec();
        let mut types = Vec::with_capacity(clause_witnesses.len());
        for text in &clause_witnesses {
            if !text.is_empty() {
                types.push(self.narrow_type_by_type_name(t, text));
            } else {
                types.push(self.never_type);
            }
        }
        self.get_union_type(&types)
    }

    // Go: checker/flow.go:1187 narrowTypeBySwitchOnTrue
    pub fn narrow_type_by_switch_on_true(
        &mut self,
        f: &mut FlowState,
        mut t: TypeId,
        data: &FlowSwitchClauseData,
    ) -> TypeId {
        let clauses = data
            .switch_statement
            .case_block()
            .clauses()
            .nodes()
            .to_vec();
        let default_index = clauses
            .iter()
            .position(|clause| clause.kind() == SyntaxKind::DefaultClause)
            .map_or(-1, |i| i as i32);
        let clause_start = data.clause_start;
        let clause_end = data.clause_end;
        let has_default_clause = clause_start == clause_end
            || (default_index >= clause_start && default_index < clause_end);
        // First, narrow away all of the cases that preceded this set of cases.
        for i in 0..clause_start as usize {
            let clause = clauses[i];
            if clause.kind() == SyntaxKind::CaseClause {
                t = self.narrow_type(f, t, clause.expression(), false /*assumeTrue*/);
            }
        }
        // If our current set has a default, then none the other cases were hit either.
        // There's no point in narrowing by the other cases in the set, since we can
        // get here through other paths.
        if has_default_clause {
            for i in clause_end as usize..clauses.len() {
                let clause = clauses[i];
                if clause.kind() == SyntaxKind::CaseClause {
                    t = self.narrow_type(f, t, clause.expression(), false /*assumeTrue*/);
                }
            }
            return t;
        }
        // Now, narrow based on the cases in this set.
        let mut types = Vec::with_capacity((clause_end - clause_start) as usize);
        for &clause in &clauses[clause_start as usize..clause_end as usize] {
            if clause.kind() == SyntaxKind::CaseClause {
                types.push(self.narrow_type(f, t, clause.expression(), true /*assumeTrue*/));
            } else {
                types.push(self.never_type);
            }
        }
        self.get_union_type(&types)
    }

    // Go: checker/flow.go:1223 narrowTypeBySwitchOptionalChainContainment
    pub fn narrow_type_by_switch_optional_chain_containment(
        &mut self,
        t: TypeId,
        data: &FlowSwitchClauseData,
        clause_check: &mut dyn FnMut(&mut Checker, TypeId) -> bool,
    ) -> TypeId {
        let every_clause_checks = data.clause_start != data.clause_end && {
            let switch_types = self.get_switch_clause_types(data.switch_statement);
            let mut every = true;
            for &s in &switch_types[data.clause_start as usize..data.clause_end as usize] {
                if !clause_check(self, s) {
                    every = false;
                    break;
                }
            }
            every
        };
        if every_clause_checks {
            return self.get_type_with_facts(t, TypeFacts::NE_UNDEFINED_OR_NULL);
        }
        t
    }

    // Go: checker/flow.go:1231 narrowTypeBySwitchOnDiscriminantProperty
    pub fn narrow_type_by_switch_on_discriminant_property(
        &mut self,
        t: TypeId,
        access: Node,
        data: &FlowSwitchClauseData,
    ) -> TypeId {
        if data.clause_start < data.clause_end && self.ty(t).flags.intersects(TypeFlags::UNION) {
            let (accessed_name, _) = self.get_accessed_property_name(access);
            if !accessed_name.is_empty() && self.get_key_property_name(t) == accessed_name {
                let clause_types: Vec<TypeId> = self.get_switch_clause_types(data.switch_statement)
                    [data.clause_start as usize..data.clause_end as usize]
                    .to_vec();
                let mut types = Vec::with_capacity(clause_types.len());
                for s in clause_types {
                    let result = self.get_constituent_type_for_key_type(t, s);
                    if result.is_some() {
                        types.push(result);
                    } else {
                        types.push(self.unknown_type);
                    }
                }
                let candidate = self.get_union_type(&types);
                if candidate != self.unknown_type {
                    return candidate;
                }
            }
        }
        let data = *data;
        self.narrow_type_by_discriminant(t, access, &mut |c: &mut Checker, t: TypeId| {
            c.narrow_type_by_switch_on_discriminant(t, &data)
        })
    }

    // Go: checker/flow.go:1253 getTypeAtFlowBranchLabel
    // PORT: Go `*ast.FlowList` is the antecedent slice in list order. Go's
    // `flow` parameter is not read, so it is left out.
    pub fn get_type_at_flow_branch_label(
        &mut self,
        f: &mut FlowState,
        antecedents: &[FlowNodeId],
    ) -> FlowType {
        let antecedent_start = self.antecedent_types.len();
        let mut subtype_reduction = false;
        let mut seen_incomplete = false;
        let mut bypass_flow = FlowNodeId::NIL;
        for &antecedent in antecedents {
            // PERF: lsshells M3b. No guard for a static file (`get_flow_in`).
            // flowcost1: the walk's cached flow nodes (`FlowState::flow_data`).
            let mut antecedent_node_guard = None;
            let antecedent_node = f.flow_data(antecedent, &mut antecedent_node_guard);
            if bypass_flow.is_nil()
                && antecedent_node.flags.intersects(FlowFlags::SWITCH_CLAUSE)
                && antecedent_node.as_flow_switch_clause_data().is_empty()
            {
                // The antecedent is the bypass branch of a potentially exhaustive switch statement.
                bypass_flow = antecedent;
                continue;
            }
            let flow_type = self.get_type_at_flow_node(f, antecedent);
            let (declared_type, initial_type) = (f.declared_type, f.initial_type);
            // If the type at a particular antecedent path is the declared type and the
            // reference is known to always be assigned (i.e. when declared and initial types
            // are the same), there is no reason to process more antecedents since the only
            // possible outcome is subtypes that will be removed in the final union type anyway.
            if flow_type.t == declared_type && declared_type == initial_type {
                self.antecedent_types.truncate(antecedent_start);
                return FlowType {
                    t: flow_type.t,
                    incomplete: false,
                };
            }
            if !self.antecedent_types[antecedent_start..].contains(&flow_type.t) {
                self.antecedent_types.push(flow_type.t);
            }
            // If an antecedent type is not a subset of the declared type, we need to perform
            // subtype reduction. This happens when a "foreign" type is injected into the control
            // flow using the instanceof operator or a user defined type predicate.
            if !self.is_type_subset_of(flow_type.t, initial_type) {
                subtype_reduction = true;
            }
            if flow_type.incomplete {
                seen_incomplete = true;
            }
        }
        if bypass_flow.is_some() {
            let flow_type = self.get_type_at_flow_node(f, bypass_flow);
            // If the bypass flow contributes a type we haven't seen yet and the switch statement
            // isn't exhaustive, process the bypass flow type. Since exhaustiveness checks increase
            // the risk of circularities, we only want to perform them when they make a difference.
            if !self.ty(flow_type.t).flags.intersects(TypeFlags::NEVER)
                && !self.antecedent_types[antecedent_start..].contains(&flow_type.t)
                && !self.is_exhaustive_switch_statement(
                    bypass_flow
                        .get_flow()
                        .as_flow_switch_clause_data()
                        .switch_statement,
                )
            {
                let (declared_type, initial_type) = (f.declared_type, f.initial_type);
                if flow_type.t == declared_type && declared_type == initial_type {
                    self.antecedent_types.truncate(antecedent_start);
                    return FlowType {
                        t: flow_type.t,
                        incomplete: false,
                    };
                }
                self.antecedent_types.push(flow_type.t);
                if !self.is_type_subset_of(flow_type.t, initial_type) {
                    subtype_reduction = true;
                }
                if flow_type.incomplete {
                    seen_incomplete = true;
                }
            }
        }
        let types: Vec<TypeId> = self.antecedent_types[antecedent_start..].to_vec();
        let union_or_evolving = self.get_union_or_evolving_array_type(
            f,
            &types,
            if subtype_reduction {
                UnionReduction::SUBTYPE
            } else {
                UnionReduction::LITERAL
            },
        );
        let result = self.new_flow_type(union_or_evolving, seen_incomplete);
        self.antecedent_types.truncate(antecedent_start);
        result
    }

    // At flow control branch or loop junctions, if the type along every antecedent code path
    // is an evolving array type, we construct a combined evolving array type. Otherwise we
    // finalize all evolving array types.
    // Go: checker/flow.go:1314 getUnionOrEvolvingArrayType
    pub fn get_union_or_evolving_array_type(
        &mut self,
        f: &mut FlowState,
        types: &[TypeId],
        subtype_reduction: UnionReduction,
    ) -> TypeId {
        if self.is_evolving_array_type_list(types) {
            let mut element_types = Vec::with_capacity(types.len());
            for &t in types {
                element_types.push(self.get_element_type_of_evolving_array_type(t));
            }
            let element_type = self.get_union_type(&element_types);
            return self.get_evolving_array_type(element_type);
        }
        let mut finalized = Vec::with_capacity(types.len());
        for &t in types {
            finalized.push(self.finalize_evolving_array_type(t));
        }
        let union = self.get_union_type_ex(&finalized, subtype_reduction, None, TypeId::NIL);
        let result = self.recombine_unknown_type(union);
        let declared_type = f.declared_type;
        if result != declared_type
            && (self.ty(result).flags & self.ty(declared_type).flags).intersects(TypeFlags::UNION)
            && self.ty(result).types() == self.ty(declared_type).types()
        {
            return declared_type;
        }
        result
    }

    // Go: checker/flow.go:1325 getTypeAtFlowLoopLabel
    // PERF: `flow_data` is `flow.get_flow()`, which the caller has read.
    pub fn get_type_at_flow_loop_label(
        &mut self,
        f: &mut FlowState,
        flow: FlowNodeId,
        flow_data: &FlowNode,
    ) -> FlowType {
        if f.ref_key.is_zero() {
            let ref_key = self.get_flow_reference_key(f);
            f.ref_key = ref_key;
        }
        let ref_key = f.ref_key;
        if ref_key == *NON_DOTTED_NAME_CACHE_KEY {
            // No cache key is generated when binding patterns are in unnarrowable situations
            return FlowType {
                t: f.declared_type,
                incomplete: false,
            };
        }
        let key = FlowLoopKey {
            flow_node: flow,
            ref_key,
        };
        // If we have previously computed the control flow type for the reference at
        // this flow loop junction, return the cached type.
        if let Some(&cached) = self.flow_loop_cache.get(&key) {
            if cached.is_some() {
                return FlowType {
                    t: cached,
                    incomplete: false,
                };
            }
        }
        // If this flow loop junction and reference are already being processed, return
        // the union of the types computed for each branch so far, marked as incomplete.
        // It is possible to see an empty array in cases where loops are nested and the
        // back edge of the outer loop reaches an inner loop that is already being analyzed.
        // In such cases we restart the analysis of the inner loop, which will then see
        // a non-empty in-process array for the outer loop and eventually terminate because
        // the first antecedent of a loop junction is always the non-looping control flow
        // path that leads to the top.
        let in_process = self
            .flow_loop_stack
            .iter()
            .find(|loop_info| loop_info.key == key && !loop_info.types.is_empty())
            .map(|loop_info| loop_info.types.clone());
        if let Some(loop_types) = in_process {
            let union =
                self.get_union_or_evolving_array_type(f, &loop_types, UnionReduction::LITERAL);
            return self.new_flow_type(union, true /*incomplete*/);
        }
        // Add the flow loop junction and reference to the in-process stack and analyze
        // each antecedent code path.
        let mut antecedent_types: Vec<TypeId> = Vec::with_capacity(4);
        let mut subtype_reduction = false;
        let mut first_antecedent_type = FlowType {
            t: TypeId::NIL,
            incomplete: false,
        };
        for &antecedent in &flow_data.antecedents {
            let flow_type;
            if first_antecedent_type.t.is_nil() {
                // The first antecedent of a loop junction is always the non-looping control
                // flow path that leads to the top.
                first_antecedent_type = self.get_type_at_flow_node(f, antecedent);
                flow_type = FlowType {
                    t: first_antecedent_type.t,
                    incomplete: first_antecedent_type.incomplete,
                };
            } else {
                // All but the first antecedent are the looping control flow paths that lead
                // back to the loop junction. We track these on the flow loop stack.
                // PORT: Go appends a slice header that shares `antecedentTypes`; this
                // frame does not change `antecedentTypes` while the entry is on the
                // stack, so a copy is equivalent.
                self.flow_loop_stack.push(FlowLoopInfo {
                    key,
                    types: antecedent_types.clone(),
                });
                // PORT: Go sets `c.flowTypeCache = nil`; the map field is taken and
                // restored instead.
                let save_flow_type_cache = std::mem::take(&mut self.flow_type_cache);
                flow_type = self.get_type_at_flow_node(f, antecedent);
                self.flow_type_cache = save_flow_type_cache;
                self.flow_loop_stack.pop();
                // If we see a value appear in the cache it is a sign that control flow analysis
                // was restarted and completed by checkExpressionCached. We can simply pick up
                // the resulting type and bail out.
                if let Some(&cached) = self.flow_loop_cache.get(&key) {
                    if cached.is_some() {
                        return FlowType {
                            t: cached,
                            incomplete: false,
                        };
                    }
                }
            }
            if !antecedent_types.contains(&flow_type.t) {
                antecedent_types.push(flow_type.t);
            }
            // If an antecedent type is not a subset of the declared type, we need to perform
            // subtype reduction. This happens when a "foreign" type is injected into the control
            // flow using the instanceof operator or a user defined type predicate.
            let (declared_type, initial_type) = (f.declared_type, f.initial_type);
            if !self.is_type_subset_of(flow_type.t, initial_type) {
                subtype_reduction = true;
            }
            // If the type at a particular antecedent path is the declared type there is no
            // reason to process more antecedents since the only possible outcome is subtypes
            // that will be removed in the final union type anyway.
            if flow_type.t == declared_type {
                break;
            }
        }
        // The result is incomplete if the first antecedent (the non-looping control flow path)
        // is incomplete.
        let result = self.get_union_or_evolving_array_type(
            f,
            &antecedent_types,
            if subtype_reduction {
                UnionReduction::SUBTYPE
            } else {
                UnionReduction::LITERAL
            },
        );
        if first_antecedent_type.incomplete {
            return self.new_flow_type(result, true /*incomplete*/);
        }
        self.flow_loop_cache.insert(key, result);
        FlowType {
            t: result,
            incomplete: false,
        }
    }

    // Go: checker/flow.go:1404 getTypeAtFlowArrayMutation
    // PERF: takes the flow node that the caller has read (`flow.get_flow()`).
    pub fn get_type_at_flow_array_mutation(
        &mut self,
        f: &mut FlowState,
        flow_node: &FlowNode,
    ) -> FlowType {
        let declared_type = f.declared_type;
        if declared_type == self.auto_type || declared_type == self.auto_array_type {
            let node = flow_node.node;
            let expr = if is_call_expression(node) {
                node.expression().expression()
            } else {
                node.left().expression()
            };
            let reference = f.reference;
            let candidate = self.get_reference_candidate(expr);
            if self.is_matching_flow_reference(f, candidate) {
                let flow_type = self.get_type_at_flow_node(f, flow_node.antecedent);
                if self
                    .ty(flow_type.t)
                    .object_flags
                    .intersects(ObjectFlags::EVOLVING_ARRAY)
                {
                    let mut evolved_type = flow_type.t;
                    if is_call_expression(node) {
                        for arg in node.arguments() {
                            evolved_type = self.add_evolving_array_element_type(evolved_type, arg);
                        }
                    } else {
                        // We must get the context free expression type so as to not recur in an uncached fashion on the LHS (which causes exponential blowup in compile time)
                        let index_type = self
                            .get_context_free_type_of_expression(node.left().argument_expression());
                        if self.is_type_assignable_to_kind(index_type, TypeFlags::NUMBER_LIKE) {
                            evolved_type =
                                self.add_evolving_array_element_type(evolved_type, node.right());
                        }
                    }
                    return self.new_flow_type(evolved_type, flow_type.incomplete);
                }
                return flow_type;
            }
        }
        FlowType {
            t: TypeId::NIL,
            incomplete: false,
        }
    }

    // Go: checker/flow.go:1436 getDiscriminantPropertyAccess
    pub fn get_discriminant_property_access(
        &mut self,
        f: &mut FlowState,
        expr: Node,
        computed_type: TypeId,
    ) -> Node {
        // As long as the computed type is a subset of the declared type, we use the full declared type to detect
        // a discriminant property. In cases where the computed type isn't a subset, e.g because of a preceding type
        // predicate narrowing, we use the actual computed type.
        let declared_type = f.declared_type;
        if self.ty(declared_type).flags.intersects(TypeFlags::UNION)
            || self.ty(computed_type).flags.intersects(TypeFlags::UNION)
        {
            let access = self.get_candidate_discriminant_property_access(f, expr);
            if access.is_some() {
                let (name, ok) = self.get_accessed_property_name(access);
                if ok {
                    let mut t = computed_type;
                    if self.ty(declared_type).flags.intersects(TypeFlags::UNION)
                        && self.is_type_subset_of(computed_type, declared_type)
                    {
                        t = declared_type;
                    }
                    if self.is_discriminant_property(t, &name) {
                        return access;
                    }
                }
            }
        }
        Node::NIL
    }

    // Go: checker/flow.go:1457 getCandidateDiscriminantPropertyAccess
    pub fn get_candidate_discriminant_property_access(
        &mut self,
        f: &mut FlowState,
        expr: Node,
    ) -> Node {
        let reference = f.reference;
        // PERF: flowcost1. Go `IsBindingPattern`, `IsFunctionExpressionOrArrowFunction`
        // and `IsObjectLiteralMethod` of the reference on the kind that the
        // walk has read (`FlowState::reference_kind`).
        if matches!(
            f.reference_kind,
            SyntaxKind::ObjectBindingPattern
                | SyntaxKind::ArrayBindingPattern
                | SyntaxKind::FunctionExpression
                | SyntaxKind::ArrowFunction
        ) || f.reference_kind == SyntaxKind::MethodDeclaration
            && reference.parent().kind() == SyntaxKind::ObjectLiteralExpression
        {
            // When the reference is a binding pattern or function or arrow expression, we are narrowing a pseudo-reference in
            // getNarrowedTypeOfSymbol. An identifier for a destructuring variable declared in the same binding pattern or
            // parameter declared in the same parameter list is a candidate.
            if is_identifier(expr) {
                let symbol = self.get_resolved_symbol(expr);
                let export_symbol = self.get_export_symbol_of_value_symbol_if_exported(symbol);
                let declaration = self.sym(export_symbol).value_declaration;
                if declaration.is_some()
                    && (is_binding_element(declaration) || is_parameter_declaration(declaration))
                    && reference == declaration.parent()
                    && declaration.initializer().is_nil()
                    && !has_dot_dot_dot_token(declaration)
                {
                    return declaration;
                }
            }
        } else if is_access_expression(expr) {
            // An access expression is a candidate if the reference matches the left hand expression.
            if self.is_matching_flow_reference(f, expr.expression()) {
                return expr;
            }
        } else if is_identifier(expr) {
            let symbol = self.get_resolved_symbol(expr);
            if self.is_constant_variable(symbol) {
                let declaration = self.sym(symbol).value_declaration;
                let mut initializer = get_candidate_variable_declaration_initializer(declaration);
                // Given 'const x = obj.kind', allow 'x' as an alias for 'obj.kind'
                if initializer.is_some()
                    && is_access_expression(initializer)
                    && self.is_matching_flow_reference(f, initializer.expression())
                {
                    return initializer;
                }
                // Given 'const { kind: x } = obj', allow 'x' as an alias for 'obj.kind'
                if is_binding_element(declaration) && declaration.initializer().is_nil() {
                    initializer = get_candidate_variable_declaration_initializer(
                        declaration.parent().parent(),
                    );
                    if initializer.is_some()
                        && (is_identifier(initializer) || is_access_expression(initializer))
                        && self.is_matching_flow_reference(f, initializer)
                    {
                        return declaration;
                    }
                }
            }
        }
        Node::NIL
    }
}

// Go: checker/flow.go:1496 getCandidateVariableDeclarationInitializer
pub fn get_candidate_variable_declaration_initializer(node: Node) -> Node {
    if is_variable_declaration(node) && node.type_().is_nil() {
        let initializer = node.initializer();
        if initializer.is_some() {
            return skip_parentheses(initializer);
        }
    }
    Node::NIL
}

impl Checker {
    // An evolving array type tracks the element types that have so far been seen in an
    // 'x.push(value)' or 'x[n] = value' operation along the control flow graph. Evolving
    // array types are ultimately converted into manifest array types (using getFinalArrayType)
    // and never escape the getFlowTypeOfReference function.
    // Go: checker/flow.go:1509 getEvolvingArrayType
    pub fn get_evolving_array_type(&mut self, element_type: TypeId) -> TypeId {
        let key = CachedTypeKey {
            kind: CachedTypeKind::EVOLVING_ARRAY_TYPE,
            type_id: self.ty(element_type).id,
        };
        let mut result = self.cached_types.get(&key).copied().unwrap_or_default();
        if result.is_nil() {
            result = self.new_object_type(ObjectFlags::EVOLVING_ARRAY, SymbolId::NIL);
            self.ty_mut(result)
                .as_evolving_array_type_mut()
                .element_type = element_type;
            self.cached_types.insert(key, result);
        }
        result
    }

    // Go: checker/flow.go:1520 getElementTypeOfEvolvingArrayType
    pub fn get_element_type_of_evolving_array_type(&mut self, t: TypeId) -> TypeId {
        if self
            .ty(t)
            .object_flags
            .intersects(ObjectFlags::EVOLVING_ARRAY)
        {
            return self.ty(t).as_evolving_array_type().element_type;
        }
        self.never_type
    }

    // Go: checker/flow.go:1527 isEvolvingArrayTypeList
    // PORT: Go package function that reads type data; a `Checker` method here.
    pub fn is_evolving_array_type_list(&self, types: &[TypeId]) -> bool {
        let mut has_evolving_array_type = false;
        for &t in types {
            if !self.ty(t).flags.intersects(TypeFlags::NEVER) {
                if !self
                    .ty(t)
                    .object_flags
                    .intersects(ObjectFlags::EVOLVING_ARRAY)
                {
                    return false;
                }
                has_evolving_array_type = true;
            }
        }
        has_evolving_array_type
    }

    // Return true if the given node is 'x' in an 'x.length', x.push(value)', 'x.unshift(value)' or
    // 'x[n] = value' operation, where 'n' is an expression of type any, undefined, or a number-like type.
    // Go: checker/flow.go:1542 isEvolvingArrayOperationTarget
    pub fn is_evolving_array_operation_target(&mut self, node: Node) -> bool {
        let root = self.get_reference_root(node);
        let parent = root.parent();
        let is_length_push_or_unshift = is_property_access_expression(parent)
            && (parent.name().text() == "length"
                || is_call_expression(parent.parent())
                    && is_identifier(parent.name())
                    && is_push_or_unshift_identifier(parent.name()));
        let is_element_assignment = self.is_evolving_array_element_assignment(root, parent);
        is_length_push_or_unshift || is_element_assignment
    }

    /// The Go `isElementAssignment` part of `isEvolvingArrayOperationTarget`
    /// for `root` (the reference root) and its `parent`. Unlike the length,
    /// push and unshift part, it can make types (`get_type_of_expression`),
    /// so a caller that does not need the answer still calls it at the same
    /// point (`check_identifier`).
    pub fn is_evolving_array_element_assignment(&mut self, root: Node, parent: Node) -> bool {
        is_element_access_expression(parent)
            && parent.expression() == root
            && is_binary_expression(parent.parent())
            && parent.parent().operator_token().kind() == SyntaxKind::EqualsToken
            && parent.parent().left() == parent
            && !is_assignment_target(parent.parent())
            && {
                let argument_type = self.get_type_of_expression(parent.argument_expression());
                self.is_type_assignable_to_kind(argument_type, TypeFlags::NUMBER_LIKE)
            }
    }

    // When adding evolving array element types we do not perform subtype reduction. Instead,
    // we defer subtype reduction until the evolving array type is finalized into a manifest
    // array type.
    // Go: checker/flow.go:1557 addEvolvingArrayElementType
    pub fn add_evolving_array_element_type(
        &mut self,
        evolving_array_type: TypeId,
        node: Node,
    ) -> TypeId {
        let context_free = self.get_context_free_type_of_expression(node);
        let base = self.get_base_type_of_literal_type(context_free);
        let new_element_type = self.get_regular_type_of_object_literal(base);
        let element_type = self
            .ty(evolving_array_type)
            .as_evolving_array_type()
            .element_type;
        if self.is_type_subset_of(new_element_type, element_type) {
            return evolving_array_type;
        }
        let union = self.get_union_type(&[element_type, new_element_type]);
        self.get_evolving_array_type(union)
    }

    // Go: checker/flow.go:1566 finalizeEvolvingArrayType
    pub fn finalize_evolving_array_type(&mut self, t: TypeId) -> TypeId {
        if self
            .ty(t)
            .object_flags
            .intersects(ObjectFlags::EVOLVING_ARRAY)
        {
            return self.get_final_array_type(t);
        }
        t
    }

    // Go: checker/flow.go:1573 getFinalArrayType
    // PORT: Go takes `*EvolvingArrayType`; this takes the evolving array `TypeId`.
    pub fn get_final_array_type(&mut self, t: TypeId) -> TypeId {
        if self
            .ty(t)
            .as_evolving_array_type()
            .final_array_type
            .is_nil()
        {
            let element_type = self.ty(t).as_evolving_array_type().element_type;
            let final_array_type = self.create_final_array_type(element_type);
            self.ty_mut(t).as_evolving_array_type_mut().final_array_type = final_array_type;
        }
        self.ty(t).as_evolving_array_type().final_array_type
    }

    // Go: checker/flow.go:1580 createFinalArrayType
    pub fn create_final_array_type(&mut self, element_type: TypeId) -> TypeId {
        let flags = self.ty(element_type).flags;
        if flags.intersects(TypeFlags::NEVER) {
            return self.auto_array_type;
        }
        if flags.intersects(TypeFlags::UNION) {
            let types = self.ty(element_type).types_list();
            let union = self.get_union_type_ex(&types, UnionReduction::SUBTYPE, None, TypeId::NIL);
            return self.create_array_type(union);
        }
        self.create_array_type(element_type)
    }

    // Go: checker/flow.go:1590 reportFlowControlError
    pub fn report_flow_control_error(&mut self, node: Node) {
        let block = find_ancestor(node, is_function_or_module_block);
        let source_file = get_source_file_of_node(node);
        let span = get_range_of_token_at_position(source_file, block.statement_list().pos());
        self.add_diagnostic(new_diagnostic(
            source_file,
            span,
            diag::The_containing_function_or_module_body_is_too_large_for_control_flow_analysis,
            vec![],
        ));
    }

    /// Go `c.getResolvedSymbol(source)`, from the memo when it holds
    /// `source`.
    fn memo_resolved_symbol(&mut self, source: Node) -> SymbolId {
        let memo = &self.matching_reference_memo;
        if memo.node == source && memo.resolved.is_some() {
            return memo.resolved;
        }
        let resolved = self.get_resolved_symbol(source);
        // The first resolution can check other code, which can put another
        // identifier in the memo. Then the answer is not stored.
        let memo = &mut self.matching_reference_memo;
        if memo.node == source {
            memo.resolved = resolved;
        }
        resolved
    }

    /// Go `c.getExportSymbolOfValueSymbolIfExported(c.getResolvedSymbol(source))`,
    /// from the memo when it holds `source` and no merge came after it.
    fn memo_export_symbol(&mut self, source: Node) -> SymbolId {
        let memo = &self.matching_reference_memo;
        if memo.node == source
            && memo.export.is_some()
            && memo.export_merge_version == self.merge_version
        {
            return memo.export;
        }
        let resolved = self.memo_resolved_symbol(source);
        let export = self.get_export_symbol_of_value_symbol_if_exported(resolved);
        let merge_version = self.merge_version;
        let memo = &mut self.matching_reference_memo;
        if memo.node == source {
            memo.export = export;
            memo.export_merge_version = merge_version;
        }
        export
    }

    // Go: checker/flow.go:1597 isMatchingReference
    // PERF: the kind of `target` is read once (`target_kind`). This runs
    // about 2.2M times on effect, mostly with an identifier `source`.
    pub fn is_matching_reference(&mut self, source: Node, target: Node) -> bool {
        self.is_matching_reference_kind(source, target, target.kind())
    }

    /// True when the memo already shows that `is_matching_reference(source,
    /// target)` is false for a variable or binding declaration `target` (see
    /// the declaration test in `is_matching_reference_kind`): the memo holds
    /// `source` with its export symbol, and `target` is not one of that
    /// symbol's declarations. It only reads; any other case is false, and the
    /// caller makes the full call.
    // PERF: chkA. `get_type_at_flow_assignment` tests each assignment of a
    // walk against one reference, so after the first call the memo holds it.
    #[inline]
    pub fn matching_reference_memo_says_no(
        &self,
        source: Node,
        target: Node,
        target_kind: SyntaxKind,
    ) -> bool {
        let memo = &self.matching_reference_memo;
        let says_no = matches!(
            target_kind,
            SyntaxKind::VariableDeclaration | SyntaxKind::BindingElement
        ) && memo.node == source
            && source.is_some()
            && !memo.this_in_type_query
            && memo.export.is_some()
            && memo.export_merge_version == self.merge_version
            && !self.sym(memo.export).declarations.contains(&target);
        debug_assert!(!says_no || self.merge_rule_holds(memo.export, target));
        says_no
    }

    /// True when the access memo already shows that `is_matching_reference(
    /// source, target)` is false: `source` is the synthetic property access in
    /// the memo (`SyntheticAccessMemo`), `target` is a property access, and the
    /// two names differ. It only reads; any other case is false, and the
    /// caller makes the full call.
    // PERF: this8000. The same answer as the two-property-access arm of
    // `is_matching_reference_kinds` (Go checker/flow.go:1621-1625: a
    // PropertyAccessExpression name is `Name().Text()`, flow.go:1728-1729,
    // and different names give false before the expressions are compared),
    // without the call. A constructor walk compares its one synthetic
    // `this.<name>` with each assignment. Whether PGO inlines that call
    // changed with unrelated code (int103: +16.6% st instructions on
    // js-this-8000 and ts-this-8000).
    #[inline(always)]
    pub fn synthetic_access_memo_says_no(
        &self,
        source: Node,
        source_kind: SyntaxKind,
        target: Node,
        target_kind: SyntaxKind,
    ) -> bool {
        let memo = &self.matching_reference_memo.access;
        let says_no = source_kind == SyntaxKind::PropertyAccessExpression
            && target_kind == SyntaxKind::PropertyAccessExpression
            && memo.node == source
            && memo.name != target.name().text_name();
        debug_assert!(!says_no || is_synthetic_node(source));
        debug_assert!(!says_no || memo.name == source.name().text_name());
        says_no
    }

    /// The merge rule that the chkA early returns depend on (the declaration
    /// test in `is_matching_reference_kind` and
    /// `matching_reference_memo_says_no`): when `export_symbol` is the merged
    /// symbol of `target` (Go `getMergedSymbol(target.Symbol())`), `target`
    /// is one of the declarations of `export_symbol`. Only `debug_assert!`
    /// calls it.
    fn merge_rule_holds(&self, export_symbol: SymbolId, target: Node) -> bool {
        let symbol = target.symbol();
        symbol.is_nil()
            || self.get_merged_symbol(symbol) != export_symbol
            || self.sym(export_symbol).declarations.contains(&target)
    }

    /// `is_matching_reference` for a caller that has read `target_kind`
    /// (`target.kind()`).
    #[inline]
    pub fn is_matching_reference_kind(
        &mut self,
        source: Node,
        target: Node,
        target_kind: SyntaxKind,
    ) -> bool {
        self.is_matching_reference_kinds(source, source.kind(), target, target_kind)
    }

    /// Go `c.isMatchingReference(f.reference, target)` with the reference
    /// kind that the walk has read (`FlowState::reference_kind`).
    #[inline]
    pub fn is_matching_flow_reference(&mut self, f: &FlowState, target: Node) -> bool {
        self.is_matching_reference_kinds(f.reference, f.reference_kind, target, target.kind())
    }

    /// `is_matching_reference` for a caller that has read `source_kind`
    /// (`source.kind()`) and `target_kind` (`target.kind()`).
    // PERF: flowcost1. A flow walk compares its one reference with each
    // flow node on its path; the walk reads the reference kind once.
    pub fn is_matching_reference_kinds(
        &mut self,
        source: Node,
        source_kind: SyntaxKind,
        target: Node,
        target_kind: SyntaxKind,
    ) -> bool {
        debug_assert_eq!(source_kind, source.kind());
        match target_kind {
            SyntaxKind::ParenthesizedExpression | SyntaxKind::NonNullExpression => {
                return self.is_matching_reference(source, target.expression());
            }
            SyntaxKind::BinaryExpression => {
                return is_assignment_expression(target, false)
                    && self.is_matching_reference(source, target.left())
                    || is_binary_expression(target)
                        && target.operator_token().kind() == SyntaxKind::CommaToken
                        && self.is_matching_reference(source, target.right());
            }
            _ => {}
        }
        match source_kind {
            SyntaxKind::MetaProperty => {
                return is_meta_property(target)
                    && source.keyword_token() == target.keyword_token()
                    && source.name().text() == target.name().text();
            }
            SyntaxKind::Identifier | SyntaxKind::PrivateIdentifier => {
                // PERF: every answer below is false for a target of another
                // kind, and `is_this_in_type_query` only reads the tree, so
                // such a target returns before it.
                if !matches!(
                    target_kind,
                    SyntaxKind::ThisKeyword
                        | SyntaxKind::Identifier
                        | SyntaxKind::VariableDeclaration
                        | SyntaxKind::BindingElement
                ) {
                    return false;
                }
                // PERF: the reads of `source` come from the memo (see
                // `MatchingReferenceMemo`). Each one is made at the same
                // point as in Go the first time.
                if self.matching_reference_memo.node != source {
                    let memo = &mut self.matching_reference_memo;
                    memo.node = source;
                    memo.this_in_type_query = is_this_in_type_query(source);
                    memo.resolved = SymbolId::NIL;
                    memo.export = SymbolId::NIL;
                    memo.export_merge_version = 0;
                }
                if self.matching_reference_memo.this_in_type_query {
                    return target_kind == SyntaxKind::ThisKeyword;
                }
                if target_kind == SyntaxKind::Identifier {
                    let source_symbol = self.memo_resolved_symbol(source);
                    if source_symbol == self.get_resolved_symbol(target) {
                        return true;
                    }
                }
                if target_kind == SyntaxKind::VariableDeclaration
                    || target_kind == SyntaxKind::BindingElement
                {
                    let export_symbol = self.memo_export_symbol(source);
                    // PERF: chkA. Go compares `export_symbol` with
                    // `getMergedSymbol(target.Symbol())`. The binder puts
                    // `target` in the declarations of its symbol, and only a
                    // merge (`merge_symbol`, `clone_symbol`) gives a symbol
                    // another merged symbol. Both copy the declarations of
                    // their source into the merged symbol, which only grows.
                    // So the two are equal only when `target` is one of the
                    // declarations of `export_symbol`. Any other target gives
                    // false with no read of its symbol and no merge lookup.
                    // Debug builds check the rule (`merge_rule_holds`).
                    if export_symbol.is_some()
                        && !self.sym(export_symbol).declarations.contains(&target)
                    {
                        debug_assert!(self.merge_rule_holds(export_symbol, target));
                        return false;
                    }
                    // PERF: Go `getSymbolOfDeclaration(target)` without its
                    // `getLateBoundSymbol` step, which returns its argument
                    // here: the binder symbol of a variable or binding
                    // element has an identifier name, never the internal
                    // computed name. So the symbol is not read.
                    let symbol = target.symbol();
                    debug_assert_eq!(
                        symbol,
                        if symbol.is_some() {
                            self.get_late_bound_symbol(symbol)
                        } else {
                            symbol
                        }
                    );
                    let declared = if symbol.is_some() {
                        self.get_merged_symbol(symbol)
                    } else {
                        SymbolId::NIL
                    };
                    return export_symbol == declared;
                }
                return false;
            }
            SyntaxKind::ThisKeyword => {
                return target_kind == SyntaxKind::ThisKeyword;
            }
            SyntaxKind::SuperKeyword => {
                return target_kind == SyntaxKind::SuperKeyword;
            }
            SyntaxKind::NonNullExpression
            | SyntaxKind::ParenthesizedExpression
            | SyntaxKind::SatisfiesExpression => {
                return self.is_matching_reference(source.expression(), target);
            }
            // PERF: for a property access `source` and a target that is not
            // an access expression, every Go test below is false, and the
            // Go `getAccessedPropertyName(source)` only reads the name. An
            // element access `source` keeps the Go path: its name lookup can
            // resolve symbols and types.
            SyntaxKind::PropertyAccessExpression
                if !matches!(
                    target_kind,
                    SyntaxKind::PropertyAccessExpression | SyntaxKind::ElementAccessExpression
                ) =>
            {
                return false;
            }
            // PERF: flowcost1. Two property accesses: Go compares the two
            // `getAccessedPropertyName` texts (`Name().Text()` of each), and
            // equal texts are equal interned names, so the name ids are
            // compared (`Node::text_name`) without reading either text.
            SyntaxKind::PropertyAccessExpression
                if target_kind == SyntaxKind::PropertyAccessExpression =>
            {
                debug_assert_eq!(
                    source.name().text_name() == target.name().text_name(),
                    source.name().text() == target.name().text()
                );
                // PERF: synthref1. A synthetic `source` (Go
                // `getFlowTypeInConstructor` and
                // `isPropertyInitializedInConstructor`) reads its name and
                // expression from the memo (`SyntheticAccessMemo`).
                if is_synthetic_node(source) {
                    self.fill_synthetic_access_memo(source);
                    let memo = &self.matching_reference_memo.access;
                    if memo.name != target.name().text_name() {
                        return false;
                    }
                    let (expression, expression_kind) = (memo.expression, memo.expression_kind);
                    let target_expression = target.expression();
                    return self.is_matching_reference_kinds(
                        expression,
                        expression_kind,
                        target_expression,
                        target_expression.kind(),
                    );
                }
                return source.name().text_name() == target.name().text_name()
                    && self.is_matching_reference(source.expression(), target.expression());
            }
            SyntaxKind::PropertyAccessExpression | SyntaxKind::ElementAccessExpression => {
                let (source_property_name, ok) = self.get_accessed_property_name(source);
                if ok {
                    if is_access_expression(target) {
                        let (target_property_name, ok) = self.get_accessed_property_name(target);
                        if ok {
                            return target_property_name == source_property_name
                                && self.is_matching_reference(
                                    source.expression(),
                                    target.expression(),
                                );
                        }
                    }
                }
                if is_element_access_expression(source) && is_element_access_expression(target) {
                    let source_arg = source.argument_expression();
                    let target_arg = target.argument_expression();
                    if is_identifier(source_arg) && is_identifier(target_arg) {
                        let symbol = self.get_resolved_symbol(source_arg);
                        if symbol == self.get_resolved_symbol(target_arg)
                            && (self.is_constant_variable(symbol)
                                || self.is_parameter_or_mutable_local_variable(symbol)
                                    && !self.is_symbol_assigned(symbol))
                        {
                            return self
                                .is_matching_reference(source.expression(), target.expression());
                        }
                    }
                }
            }
            SyntaxKind::QualifiedName => {
                if is_access_expression(target) {
                    let (target_property_name, ok) = self.get_accessed_property_name(target);
                    if ok {
                        return source.right().text_is(&target_property_name)
                            && self.is_matching_reference(source.left(), target.expression());
                    }
                }
            }
            SyntaxKind::BinaryExpression => {
                return is_binary_expression(source)
                    && source.operator_token().kind() == SyntaxKind::CommaToken
                    && self.is_matching_reference(source.right(), target);
            }
            _ => {}
        }
        false
    }
}

/// PERF: not in Go. What `is_matching_reference` read about its last
/// identifier `source`, and (in `access`) about its last synthetic property
/// access `source`. Flow analysis compares one reference with each flow
/// node on its path, so the same `source` comes again and again.
///
/// Each value is stable once read: `is_this_in_type_query` only reads the
/// tree, and `get_resolved_symbol` stores its answer in the node links, which
/// nothing overwrites later. The export symbol depends on the merged symbol
/// table and on the flags a merge can add, so it is read again after any
/// merge (`Checker::merge_version`). A nested `is_matching_reference` call
/// (during the first resolution of `node`) can replace the memo, so a value
/// is stored only while the memo still holds its node.
#[derive(Clone, Debug, Default)]
pub struct MatchingReferenceMemo {
    /// The identifier; nil when the memo is empty.
    pub node: Node,
    /// Go `ast.IsThisInTypeQuery(node)`.
    pub this_in_type_query: bool,
    /// Go `c.getResolvedSymbol(node)`; nil until read.
    pub resolved: SymbolId,
    /// Go `c.getExportSymbolOfValueSymbolIfExported(resolved)`; nil until read.
    pub export: SymbolId,
    /// `Checker::merge_version` when `export` was read.
    pub export_merge_version: u64,
    /// The last synthetic property access `source`. A new identifier
    /// `source` keeps it.
    pub access: SyntheticAccessMemo,
}

/// PERF: synthref1, not in Go. What `is_matching_reference` and
/// `contains_matching_reference` read about their last synthetic property
/// access `source`: the `this.<name>` reference of a constructor or static
/// block walk (Go `getFlowTypeInConstructor`, `getFlowTypeInStaticBlocks`,
/// `isPropertyInitializedInConstructor` and
/// `isPropertyInitializedInStaticBlocks`). The walk compares it with each
/// assignment of the constructor or the static blocks, and each read of a
/// synthetic node goes through the synthetic arena (its name text is also
/// interned again), where Go reads a field.
///
/// The values are tree reads, so they are stable: the checker does not
/// change the data of its own reference nodes, and a synthetic handle never
/// names another node while the checker lives (`ast::synthetic`: a freed
/// chunk leaves a hole that is never used again). That last rule holds
/// because of two rules that no code checks: a checker never moves to
/// another thread (the arena is thread-local), and no call replaces the
/// thread's arena (`install_synthetic_seed`, `install_twin_synthetic_arena`,
/// `forget_synthetic_nodes`, `free_synthetic_nodes`) while a checker lives
/// on the thread. All 4 creators pass a `this` expression, so `expression`
/// is never nil (with a nil expression Go's walk returns false, and
/// `fill_synthetic_access_memo` would panic on the kind read).
#[derive(Clone, Debug)]
pub struct SyntheticAccessMemo {
    /// The synthetic property access (so its kind is
    /// `PropertyAccessExpression`); nil when the memo is empty.
    pub node: Node,
    /// Go `node.Name().Text()`, interned.
    pub name: Name,
    /// Go `node.Expression()`.
    pub expression: Node,
    /// The kind of `expression`.
    pub expression_kind: SyntaxKind,
}

impl Default for SyntheticAccessMemo {
    fn default() -> Self {
        SyntheticAccessMemo {
            node: Node::NIL,
            name: Name::default(),
            expression: Node::NIL,
            expression_kind: SyntaxKind::Unknown,
        }
    }
}

// Go: checker/flow.go:1651 nonDottedNameCacheKey
// PORT: Go package var `CacheHashKey(xxh3.HashString128("?"))`, computed through
// `KeyBuilder` like `SIGNATURE_KEY_*` in checker_p01. Read with `*NON_DOTTED_NAME_CACHE_KEY`.
pub static NON_DOTTED_NAME_CACHE_KEY: LazyLock<CacheHashKey> = LazyLock::new(|| {
    let mut b = KeyBuilder::default();
    b.write_string("?");
    b.hash()
});

impl Checker {
    // Return the flow cache key for a "dotted name" (i.e. a sequence of identifiers
    // separated by dots). The key consists of the id of the symbol referenced by the
    // leftmost identifier followed by zero or more property names separated by dots.
    // The result is nonDottedNameCacheKey if the reference isn't a dotted name.
    // Go: checker/flow.go:1657 getFlowReferenceKey
    pub fn get_flow_reference_key(&mut self, f: &mut FlowState) -> CacheHashKey {
        let (reference, declared_type, initial_type, flow_container) = (
            f.reference,
            f.declared_type,
            f.initial_type,
            f.flow_container,
        );
        let mut b = KeyBuilder::default();
        if self.write_flow_cache_key(
            &mut b,
            reference,
            declared_type,
            initial_type,
            flow_container,
        ) {
            return b.hash();
        }
        *NON_DOTTED_NAME_CACHE_KEY // Reference isn't a dotted name
    }

    // Go: checker/flow.go:1665 writeFlowCacheKey
    pub fn write_flow_cache_key(
        &mut self,
        b: &mut KeyBuilder,
        node: Node,
        declared_type: TypeId,
        initial_type: TypeId,
        flow_container: Node,
    ) -> bool {
        match node.kind() {
            SyntaxKind::Identifier | SyntaxKind::ThisKeyword => {
                // PORT: Go `case KindIdentifier: ... fallthrough` into `case KindThisKeyword`.
                if node.kind() == SyntaxKind::Identifier && !is_this_in_type_query(node) {
                    let symbol = self.get_resolved_symbol(node);
                    if symbol == self.unknown_symbol {
                        return false;
                    }
                    b.write_symbol(&self.symbols, symbol);
                }
                b.write_byte(b':');
                b.write_type(declared_type);
                if initial_type != declared_type {
                    b.write_byte(b'=');
                    b.write_type(initial_type);
                }
                if flow_container.is_some() {
                    b.write_byte(b'@');
                    b.write_node(flow_container);
                }
                return true;
            }
            SyntaxKind::NonNullExpression | SyntaxKind::ParenthesizedExpression => {
                return self.write_flow_cache_key(
                    b,
                    node.expression(),
                    declared_type,
                    initial_type,
                    flow_container,
                );
            }
            SyntaxKind::QualifiedName => {
                if !self.write_flow_cache_key(
                    b,
                    node.left(),
                    declared_type,
                    initial_type,
                    flow_container,
                ) {
                    return false;
                }
                b.write_byte(b'.');
                b.write_string(node.right().text());
                return true;
            }
            SyntaxKind::PropertyAccessExpression | SyntaxKind::ElementAccessExpression => {
                let (prop_name, ok) = self.get_accessed_property_name(node);
                if ok {
                    if !self.write_flow_cache_key(
                        b,
                        node.expression(),
                        declared_type,
                        initial_type,
                        flow_container,
                    ) {
                        return false;
                    }
                    b.write_byte(b'.');
                    b.write_string(&prop_name);
                    return true;
                }
                if is_element_access_expression(node) && is_identifier(node.argument_expression()) {
                    let symbol = self.get_resolved_symbol(node.argument_expression());
                    if self.is_constant_variable(symbol)
                        || self.is_parameter_or_mutable_local_variable(symbol)
                            && !self.is_symbol_assigned(symbol)
                    {
                        if !self.write_flow_cache_key(
                            b,
                            node.expression(),
                            declared_type,
                            initial_type,
                            flow_container,
                        ) {
                            return false;
                        }
                        b.write_string(".@");
                        b.write_symbol(&self.symbols, symbol);
                        return true;
                    }
                }
            }
            SyntaxKind::ObjectBindingPattern
            | SyntaxKind::ArrayBindingPattern
            | SyntaxKind::FunctionDeclaration
            | SyntaxKind::FunctionExpression
            | SyntaxKind::ArrowFunction
            | SyntaxKind::MethodDeclaration => {
                b.write_node(node);
                b.write_byte(b'#');
                b.write_type(declared_type);
                return true;
            }
            _ => {}
        }
        false
    }

    // Go: checker/flow.go:1727 getAccessedPropertyName
    // PERF: namekeys1. Returns the interned `Name` of Go's string, so the
    // callers look properties up by id (`TableKey::Name`) and compare names
    // by id. A property access reads the name that its store node keeps
    // (`text_name`); other names are interned here.
    pub fn get_accessed_property_name(&mut self, access: Node) -> (Name, bool) {
        if is_property_access_expression(access) {
            return (access.name().text_name(), true);
        }
        if is_element_access_expression(access) {
            let (name, ok) = self.try_get_element_access_expression_name(access);
            return (Name::from(name.as_ref()), ok);
        }
        if is_binding_element(access) {
            let (name, ok) = self.get_destructuring_property_name(access);
            return (Name::from(name.as_ref()), ok);
        }
        if is_parameter_declaration(access) {
            let index = access
                .parent()
                .parameters()
                .to_vec()
                .iter()
                .position(|&p| p == access)
                .map_or(-1, |i| i as i32);
            return (Name::from(index.to_string()), true);
        }
        (Name::default(), false)
    }

    // Go: checker/flow.go:1743 tryGetElementAccessExpressionName
    // PORT: Go `*ast.ElementAccessExpression` is the element access `Node`.
    pub fn try_get_element_access_expression_name(
        &mut self,
        node: Node,
    ) -> (Cow<'static, str>, bool) {
        let argument_expression = node.argument_expression();
        if is_string_or_numeric_literal_like(argument_expression) {
            return (Cow::Borrowed(argument_expression.text()), true);
        }
        if is_entity_name_expression(argument_expression) {
            return self.try_get_name_from_entity_name_expression(argument_expression);
        }
        (Cow::Borrowed(""), false)
    }

    // Go: checker/flow.go:1753 tryGetNameFromEntityNameExpression
    pub fn try_get_name_from_entity_name_expression(
        &mut self,
        node: Node,
    ) -> (Cow<'static, str>, bool) {
        // flowskip1 verify: an effect site (flow_skip.rs).
        self.flow_skip.effects += 1;
        let symbol = self.resolve_entity_name(
            node,
            SymbolFlags::VALUE,
            true, /*ignoreErrors*/
            false,
            Node::NIL,
        );
        if symbol.is_nil()
            || !(self.is_constant_variable(symbol)
                || self.sym(symbol).flags.intersects(SymbolFlags::ENUM_MEMBER))
        {
            return (Cow::Borrowed(""), false);
        }
        let declaration = self.sym(symbol).value_declaration;
        if declaration.is_nil() {
            return (Cow::Borrowed(""), false);
        }
        let t = self.try_get_type_from_type_node(declaration);
        if t.is_some() {
            let (name, ok) = try_get_name_from_type(self, t);
            if ok {
                return (Cow::Owned(name), true);
            }
        }
        // We exclude binding elements because their initializers don't solely determine their types and resolving
        // full types can cause circularities (see https://github.com/microsoft/TypeScript/issues/63192).
        if has_only_expression_initializer(declaration)
            && !is_binding_element(declaration)
            && self.is_block_scoped_name_declared_before_use(declaration, node)
        {
            let initializer = declaration.initializer();
            if initializer.is_some() {
                let initializer_type = self.get_type_of_expression(initializer);
                if initializer_type.is_some() {
                    let (name, ok) = try_get_name_from_type(self, initializer_type);
                    return (Cow::Owned(name), ok);
                }
            } else if is_enum_member(declaration) {
                let (name, ok) = try_get_text_of_property_name(declaration.name());
                return (Cow::Owned(name), ok);
            }
        }
        (Cow::Borrowed(""), false)
    }
}

// Go: checker/flow.go:1782 tryGetNameFromType
// PORT: Go package function that reads type data. The contract makes such
// functions `Checker` methods, but `Checker::try_get_name_from_type` already
// exists (Go method `(*Checker).tryGetNameFromType`, checker.go:19158, which
// behaves differently). This stays a free function that takes the checker.
pub fn try_get_name_from_type(c: &Checker, t: TypeId) -> (String, bool) {
    let flags = c.ty(t).flags;
    if flags.intersects(TypeFlags::UNIQUE_ES_SYMBOL) {
        return (c.ty(t).as_unique_es_symbol_type().name.clone(), true);
    }
    if flags.intersects(TypeFlags::STRING_OR_NUMBER_LITERAL) {
        // PORT: Go passes the `any` value; a nil value panics in `AnyToString`.
        let value = c
            .ty(t)
            .as_literal_type()
            .value
            .as_ref()
            .expect("Unhandled case in AnyToString");
        return (any_to_string(value), true);
    }
    (String::new(), false)
}

impl Checker {
    // Go: checker/flow.go:1792 getDestructuringPropertyName
    pub fn get_destructuring_property_name(&mut self, node: Node) -> (Cow<'static, str>, bool) {
        let parent = node.parent();
        if is_binding_element(node) && is_object_binding_pattern(parent) {
            return self.get_literal_property_name_text(get_binding_element_property_name(node));
        }
        if is_property_assignment(node) || is_shorthand_property_assignment(node) {
            return self.get_literal_property_name_text(node.name());
        }
        if is_array_literal_expression(parent) || is_array_binding_pattern(parent) {
            let index = parent
                .elements()
                .to_vec()
                .iter()
                .position(|&e| e == node)
                .map_or(-1, |i| i as i32);
            return (Cow::Owned(index.to_string()), true);
        }
        (Cow::Borrowed(""), false)
    }

    // Go: checker/flow.go:1806 getLiteralPropertyNameText
    pub fn get_literal_property_name_text(&mut self, name: Node) -> (Cow<'static, str>, bool) {
        let t = self.get_literal_type_from_property_name(name);
        if self
            .ty(t)
            .flags
            .intersects(TypeFlags::STRING_LITERAL | TypeFlags::NUMBER_LITERAL)
        {
            let value = self
                .ty(t)
                .as_literal_type()
                .value
                .as_ref()
                .expect("Unhandled case in AnyToString");
            // PERF: an identifier or string literal name gives a string literal
            // type of its own text. When the value is that text, borrow the node
            // text (it lives for the program) instead of copying the value.
            if let LiteralValue::String(value) = value
                && matches!(
                    name.kind(),
                    SyntaxKind::Identifier
                        | SyntaxKind::StringLiteral
                        | SyntaxKind::NoSubstitutionTemplateLiteral
                )
                && name.text() == value.as_str()
            {
                return (Cow::Borrowed(name.text()), true);
            }
            return (Cow::Owned(any_to_string(value)), true);
        }
        (Cow::Borrowed(""), false)
    }

    // Go: checker/flow.go:1814 isConstantReference
    pub fn is_constant_reference(&mut self, node: Node) -> bool {
        match node.kind() {
            SyntaxKind::ThisKeyword => {
                return true;
            }
            SyntaxKind::Identifier => {
                if !is_this_in_type_query(node) {
                    let symbol = self.get_resolved_symbol(node);
                    return self.is_constant_variable(symbol)
                        || self.is_parameter_or_mutable_local_variable(symbol)
                            && !self.is_symbol_assigned(symbol)
                        || {
                            let value_declaration = self.sym(symbol).value_declaration;
                            value_declaration.is_some() && is_function_expression(value_declaration)
                        };
                }
            }
            SyntaxKind::PropertyAccessExpression | SyntaxKind::ElementAccessExpression => {
                // The resolvedSymbol property is initialized by checkPropertyAccess or checkElementAccess before we get here.
                if self.is_constant_reference(node.expression()) {
                    let symbol = self.get_resolved_symbol_or_nil(node);
                    if symbol.is_some() {
                        return self.is_readonly_symbol(symbol);
                    }
                }
            }
            SyntaxKind::ObjectBindingPattern | SyntaxKind::ArrayBindingPattern => {
                let root_declaration = get_root_declaration(node.parent());
                if is_parameter_declaration(root_declaration)
                    || is_variable_declaration(root_declaration)
                        && is_catch_clause(root_declaration.parent())
                {
                    return !self.is_some_symbol_assigned(root_declaration);
                }
                return is_variable_declaration(root_declaration)
                    && self.is_var_const_like(root_declaration);
            }
            _ => {}
        }
        false
    }

    // Go: checker/flow.go:1841 containsMatchingReference
    // PERF: synthref1. The first step of a synthetic property access
    // `source` reads its kind and expression from the memo
    // (`SyntheticAccessMemo`), and the kind of each `source` is read once.
    pub fn contains_matching_reference(&mut self, mut source: Node, target: Node) -> bool {
        let mut source_kind =
            if is_synthetic_node(source) && self.matching_reference_memo.access.node == source {
                debug_assert_eq!(source.kind(), SyntaxKind::PropertyAccessExpression);
                SyntaxKind::PropertyAccessExpression
            } else {
                source.kind()
            };
        while matches!(
            source_kind,
            SyntaxKind::PropertyAccessExpression | SyntaxKind::ElementAccessExpression
        ) {
            if source_kind == SyntaxKind::PropertyAccessExpression && is_synthetic_node(source) {
                self.fill_synthetic_access_memo(source);
                let memo = &self.matching_reference_memo.access;
                (source, source_kind) = (memo.expression, memo.expression_kind);
            } else {
                source = source.expression();
                source_kind = source.kind();
            }
            if self.is_matching_reference_kinds(source, source_kind, target, target.kind()) {
                return true;
            }
        }
        false
    }

    /// Puts the synthetic property access `source` in the access memo
    /// (`SyntheticAccessMemo`) unless it is there.
    fn fill_synthetic_access_memo(&mut self, source: Node) {
        debug_assert!(is_synthetic_node(source));
        debug_assert_eq!(source.kind(), SyntaxKind::PropertyAccessExpression);
        if self.matching_reference_memo.access.node == source {
            debug_assert_eq!(
                self.matching_reference_memo.access.name,
                source.name().text_name()
            );
            debug_assert_eq!(
                self.matching_reference_memo.access.expression,
                source.expression()
            );
            return;
        }
        let expression = source.expression();
        self.matching_reference_memo.access = SyntheticAccessMemo {
            node: source,
            name: source.name().text_name(),
            expression,
            expression_kind: expression.kind(),
        };
    }

    // Go: checker/flow.go:1851 optionalChainContainsReference
    pub fn optional_chain_contains_reference(&mut self, mut source: Node, target: Node) -> bool {
        while is_optional_chain(source) {
            source = source.expression();
            if self.is_matching_reference(source, target) {
                return true;
            }
        }
        false
    }

    // Go: checker/flow.go:1861 getReferenceCandidate
    pub fn get_reference_candidate(&mut self, node: Node) -> Node {
        match node.kind() {
            SyntaxKind::ParenthesizedExpression => {
                return self.get_reference_candidate(node.expression());
            }
            SyntaxKind::BinaryExpression => match node.operator_token().kind() {
                SyntaxKind::EqualsToken
                | SyntaxKind::BarBarEqualsToken
                | SyntaxKind::AmpersandAmpersandEqualsToken
                | SyntaxKind::QuestionQuestionEqualsToken => {
                    return self.get_reference_candidate(node.left());
                }
                SyntaxKind::CommaToken => {
                    return self.get_reference_candidate(node.right());
                }
                _ => {}
            },
            _ => {}
        }
        node
    }
}
