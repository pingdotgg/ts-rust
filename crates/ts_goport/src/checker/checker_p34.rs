use crate::prelude::*;

use crate::diagnostics::Message;

/// The flags of the arms of Go `getTypeFactsWorker` up to its intersection
/// arm. A type takes that arm when these flags give `INTERSECTION`.
const INTERSECTION_ARM: TypeFlags = TypeFlags::STRING
    .union(TypeFlags::STRING_MAPPING)
    .union(TypeFlags::STRING_LITERAL)
    .union(TypeFlags::TEMPLATE_LITERAL)
    .union(TypeFlags::NUMBER)
    .union(TypeFlags::ENUM)
    .union(TypeFlags::NUMBER_LITERAL)
    .union(TypeFlags::BIG_INT)
    .union(TypeFlags::BIG_INT_LITERAL)
    .union(TypeFlags::BOOLEAN)
    .union(TypeFlags::BOOLEAN_LIKE)
    .union(TypeFlags::OBJECT)
    .union(TypeFlags::VOID)
    .union(TypeFlags::UNDEFINED)
    .union(TypeFlags::NULL)
    .union(TypeFlags::ES_SYMBOL_LIKE)
    .union(TypeFlags::NON_PRIMITIVE)
    .union(TypeFlags::NEVER)
    .union(TypeFlags::UNION)
    .union(TypeFlags::INTERSECTION);

impl Checker {
    // Go: checker/checker.go:31339 instantiateInstantiableTypes
    pub fn instantiate_instantiable_types(&mut self, t: TypeId, mapper: MapperId) -> TypeId {
        let flags = self.ty(t).flags;
        if flags.intersects(TypeFlags::INSTANTIABLE) {
            return self.instantiate_type(t, mapper);
        }
        if flags.intersects(TypeFlags::UNION) {
            let mapped =
                self.map_constituents(t, &mut |c, u| c.instantiate_instantiable_types(u, mapper));
            return self.get_union_type_ex(&mapped, UnionReduction::NONE, None, TypeId::NIL);
        }
        if flags.intersects(TypeFlags::INTERSECTION) {
            let mapped =
                self.map_constituents(t, &mut |c, u| c.instantiate_instantiable_types(u, mapper));
            return self.get_intersection_type(&mapped);
        }
        t
    }

    // Go: checker/checker.go:31356 pushCachedContextualType
    pub fn push_cached_contextual_type(&mut self, node: Node) {
        let t = self.get_contextual_type(node, ContextFlags::NONE);
        self.push_contextual_type(node, t, true /*isCache*/);
    }

    // Go: checker/checker.go:31360 pushContextualType
    pub fn push_contextual_type(&mut self, node: Node, t: TypeId, is_cache: bool) {
        self.contextual_infos
            .push(ContextualInfo { node, t, is_cache });
    }

    // Go: checker/checker.go:31364 popContextualType
    pub fn pop_contextual_type(&mut self) {
        self.contextual_infos.pop();
    }

    // Go: checker/checker.go:31370 findContextualNode
    pub fn find_contextual_node(&self, node: Node, include_caches: bool) -> i32 {
        for (i, info) in self.contextual_infos.iter().enumerate() {
            if node == info.node && (include_caches || !info.is_cache) {
                return i as i32;
            }
        }
        -1
    }

    // Go: checker/checker.go:31381 isContextSensitive
    // Returns true if the given expression contains (at any level of nesting) a function or arrow expression
    // that is subject to contextual typing.
    pub fn is_context_sensitive(&mut self, node: Node) -> bool {
        match node.kind() {
            SyntaxKind::FunctionExpression
            | SyntaxKind::ArrowFunction
            | SyntaxKind::MethodDeclaration
            | SyntaxKind::FunctionDeclaration => {
                return self.is_context_sensitive_function_like_declaration(node);
            }
            SyntaxKind::ObjectLiteralExpression => {
                return node
                    .properties()
                    .iter()
                    .any(|p| self.is_context_sensitive(p));
            }
            SyntaxKind::ArrayLiteralExpression => {
                return node.elements().iter().any(|e| self.is_context_sensitive(e));
            }
            SyntaxKind::ConditionalExpression => {
                return self.is_context_sensitive(node.when_true())
                    || self.is_context_sensitive(node.when_false());
            }
            SyntaxKind::BinaryExpression => {
                return node_kind_is(
                    node.operator_token(),
                    &[SyntaxKind::BarBarToken, SyntaxKind::QuestionQuestionToken],
                ) && (self.is_context_sensitive(node.left())
                    || self.is_context_sensitive(node.right()));
            }
            SyntaxKind::PropertyAssignment => {
                return self.is_context_sensitive(node.initializer());
            }
            SyntaxKind::ParenthesizedExpression => {
                return self.is_context_sensitive(node.expression());
            }
            SyntaxKind::JsxAttributes => {
                return node
                    .properties()
                    .iter()
                    .any(|p| self.is_context_sensitive(p))
                    || is_jsx_opening_element(node.parent())
                        && node
                            .parent()
                            .parent()
                            .children()
                            .nodes()
                            .iter()
                            .any(|ch| self.is_context_sensitive(ch));
            }
            SyntaxKind::JsxAttribute => {
                // If there is no initializer, JSX attribute has a boolean value of true which is not context sensitive.
                let initializer = node.initializer();
                return initializer.is_some() && self.is_context_sensitive(initializer);
            }
            SyntaxKind::JsxExpression => {
                // It is possible to that node.expression is undefined (e.g <div x={} />)
                let expression = node.expression();
                return expression.is_some() && self.is_context_sensitive(expression);
            }
            SyntaxKind::YieldExpression => {
                let expression = node.expression();
                return expression.is_some() && self.is_context_sensitive(expression);
            }
            _ => {}
        }
        false
    }

    // Go: checker/checker.go:31415 isContextSensitiveFunctionLikeDeclaration
    pub fn is_context_sensitive_function_like_declaration(&mut self, node: Node) -> bool {
        has_context_sensitive_parameters(node)
            || self.has_context_sensitive_return_expression(node)
            || self.has_context_sensitive_yield_expression(node)
    }

    // Go: checker/checker.go:31419 hasContextSensitiveReturnExpression
    pub fn has_context_sensitive_return_expression(&mut self, node: Node) -> bool {
        if !node.type_parameters().is_empty() || node.type_().is_some() {
            // PORT: Go `node.TypeParameters() != nil`; NodeSlice is empty when nil.
            return false;
        }
        let body = node.body();
        if body.is_nil() {
            return false;
        }
        if !is_block(body) {
            return self.is_context_sensitive(body);
        }
        for_each_return_statement(body, |statement: Node| -> bool {
            statement.expression().is_some() && self.is_context_sensitive(statement.expression())
        })
    }

    // Go: checker/checker.go:31435 hasContextSensitiveYieldExpression
    pub fn has_context_sensitive_yield_expression(&mut self, node: Node) -> bool {
        get_function_flags(node).intersects(FunctionFlags::GENERATOR)
            && node.body().is_some()
            && for_each_yield_expression(node.body(), &mut |e: Node| -> bool {
                self.is_context_sensitive(e)
            })
    }

    // Go: checker/checker.go:31439 pushInferenceContext
    pub fn push_inference_context(&mut self, node: Node, context: InferenceContextId) {
        self.inference_context_infos
            .push(InferenceContextInfo { node, context });
    }

    // Go: checker/checker.go:31443 popInferenceContext
    pub fn pop_inference_context(&mut self) {
        self.inference_context_infos.pop();
    }

    // Go: checker/checker.go:31449 getInferenceContext
    pub fn get_inference_context(&self, node: Node) -> InferenceContextId {
        // Go: `slices.Backward` (ts#63902).
        for v in self.inference_context_infos.iter().rev() {
            if is_node_descendant_of(node, v.node) {
                return v.context;
            }
        }
        InferenceContextId::NIL
    }

    // Go: checker/checker.go:31458 getTypeFacts
    pub fn get_type_facts(&mut self, t: TypeId, mask: TypeFacts) -> TypeFacts {
        self.get_type_facts_worker(t, mask) & mask
    }

    // Go: checker/checker.go:31462 hasTypeFacts
    pub fn has_type_facts(&mut self, t: TypeId, mask: TypeFacts) -> bool {
        self.get_type_facts(t, mask).0 != 0
    }

    // Go: checker/checker.go:31466 getTypeFactsWorker
    // PERF: unionfn1. Go's switch is split in two, in Go's order. The arms
    // that read only the type (flags, literal values, the false types, an
    // object whose facts the caller does not need) are
    // `type_facts_of_flags`, which inlines into the constituent loops, so
    // most constituents need no call. The other arms (an object whose facts
    // the caller needs, a union, an intersection) are `type_facts_slow`.
    pub fn get_type_facts_worker(&mut self, t: TypeId, caller_only_needs: TypeFacts) -> TypeFacts {
        let mut t = t;
        let mut flags = self.ty(t).flags;
        if flags.intersects(TypeFlags::INTERSECTION | TypeFlags::INSTANTIABLE) {
            t = self.get_base_constraint_of_type(t);
            if t.is_nil() {
                t = self.unknown_type;
            }
            flags = self.ty(t).flags;
            // PERF: chkfacts1. An intersection (the base constraint of a
            // plain intersection is the intersection itself) goes to its arm
            // at once when it has none of the flags of an earlier arm.
            if flags & INTERSECTION_ARM == TypeFlags::INTERSECTION {
                return self.get_intersection_type_facts(t, caller_only_needs);
            }
        }
        match self.type_facts_of_flags(t, flags, caller_only_needs) {
            Some(facts) => facts,
            None => self.type_facts_slow(t, flags, caller_only_needs),
        }
    }

    /// `get_type_facts_worker` of constituent `t`, whose flags are `flags`.
    #[inline(always)]
    fn constituent_type_facts(
        &mut self,
        t: TypeId,
        flags: TypeFlags,
        caller_only_needs: TypeFacts,
    ) -> TypeFacts {
        if flags.intersects(TypeFlags::INTERSECTION | TypeFlags::INSTANTIABLE) {
            return self.get_type_facts_worker(t, caller_only_needs);
        }
        match self.type_facts_of_flags(t, flags, caller_only_needs) {
            Some(facts) => facts,
            None => self.type_facts_slow(t, flags, caller_only_needs),
        }
    }

    /// The arms of Go `getTypeFactsWorker` that read only `t` (after its base
    /// constraint step), in Go's order. `None` for the arms that
    /// `type_facts_slow` takes: an object whose facts the caller needs, a
    /// union and an intersection.
    #[inline(always)]
    fn type_facts_of_flags(
        &self,
        t: TypeId,
        flags: TypeFlags,
        caller_only_needs: TypeFacts,
    ) -> Option<TypeFacts> {
        let strict = self.strict_null_checks;
        let facts = if flags.intersects(TypeFlags::STRING | TypeFlags::STRING_MAPPING) {
            if strict {
                TypeFacts::STRING_STRICT_FACTS
            } else {
                TypeFacts::STRING_FACTS
            }
        } else if flags.intersects(TypeFlags::STRING_LITERAL | TypeFlags::TEMPLATE_LITERAL) {
            let is_empty = flags.intersects(TypeFlags::STRING_LITERAL)
                && self.get_string_literal_value_ref(t).is_empty();
            match (strict, is_empty) {
                (true, true) => TypeFacts::EMPTY_STRING_STRICT_FACTS,
                (true, false) => TypeFacts::NON_EMPTY_STRING_STRICT_FACTS,
                (false, true) => TypeFacts::EMPTY_STRING_FACTS,
                (false, false) => TypeFacts::NON_EMPTY_STRING_FACTS,
            }
        } else if flags.intersects(TypeFlags::NUMBER | TypeFlags::ENUM) {
            if strict {
                TypeFacts::NUMBER_STRICT_FACTS
            } else {
                TypeFacts::NUMBER_FACTS
            }
        } else if flags.intersects(TypeFlags::NUMBER_LITERAL) {
            let is_zero = self.get_number_literal_value(t).0 == 0.0;
            match (strict, is_zero) {
                (true, true) => TypeFacts::ZERO_NUMBER_STRICT_FACTS,
                (true, false) => TypeFacts::NON_ZERO_NUMBER_STRICT_FACTS,
                (false, true) => TypeFacts::ZERO_NUMBER_FACTS,
                (false, false) => TypeFacts::NON_ZERO_NUMBER_FACTS,
            }
        } else if flags.intersects(TypeFlags::BIG_INT) {
            if strict {
                TypeFacts::BIG_INT_STRICT_FACTS
            } else {
                TypeFacts::BIG_INT_FACTS
            }
        } else if flags.intersects(TypeFlags::BIG_INT_LITERAL) {
            let is_zero = self.is_zero_big_int(t);
            match (strict, is_zero) {
                (true, true) => TypeFacts::ZERO_BIG_INT_STRICT_FACTS,
                (true, false) => TypeFacts::NON_ZERO_BIG_INT_STRICT_FACTS,
                (false, true) => TypeFacts::ZERO_BIG_INT_FACTS,
                (false, false) => TypeFacts::NON_ZERO_BIG_INT_FACTS,
            }
        } else if flags.intersects(TypeFlags::BOOLEAN) {
            if strict {
                TypeFacts::BOOLEAN_STRICT_FACTS
            } else {
                TypeFacts::BOOLEAN_FACTS
            }
        } else if flags.intersects(TypeFlags::BOOLEAN_LIKE) {
            let is_false = t == self.false_type || t == self.regular_false_type;
            match (strict, is_false) {
                (true, true) => TypeFacts::FALSE_STRICT_FACTS,
                (true, false) => TypeFacts::TRUE_STRICT_FACTS,
                (false, true) => TypeFacts::FALSE_FACTS,
                (false, false) => TypeFacts::TRUE_FACTS,
            }
        } else if flags.intersects(TypeFlags::OBJECT) {
            let possible_facts = if strict {
                TypeFacts::EMPTY_OBJECT_STRICT_FACTS
                    | TypeFacts::FUNCTION_STRICT_FACTS
                    | TypeFacts::OBJECT_STRICT_FACTS
            } else {
                TypeFacts::EMPTY_OBJECT_FACTS | TypeFacts::FUNCTION_FACTS | TypeFacts::OBJECT_FACTS
            };
            if caller_only_needs.intersects(possible_facts) {
                return None;
            }
            // If the caller doesn't care about any of the facts that we could possibly produce,
            // return zero so we can skip resolving members.
            TypeFacts::NONE
        } else if flags.intersects(TypeFlags::VOID) {
            TypeFacts::VOID_FACTS
        } else if flags.intersects(TypeFlags::UNDEFINED) {
            TypeFacts::UNDEFINED_FACTS
        } else if flags.intersects(TypeFlags::NULL) {
            TypeFacts::NULL_FACTS
        } else if flags.intersects(TypeFlags::ES_SYMBOL_LIKE) {
            if strict {
                TypeFacts::SYMBOL_STRICT_FACTS
            } else {
                TypeFacts::SYMBOL_FACTS
            }
        } else if flags.intersects(TypeFlags::NON_PRIMITIVE) {
            if strict {
                TypeFacts::OBJECT_STRICT_FACTS
            } else {
                TypeFacts::OBJECT_FACTS
            }
        } else if flags.intersects(TypeFlags::NEVER) {
            TypeFacts::NONE
        } else if flags.intersects(TypeFlags::UNION | TypeFlags::INTERSECTION) {
            return None;
        } else {
            TypeFacts::UNKNOWN_FACTS
        };
        Some(facts)
    }

    /// The arms of Go `getTypeFactsWorker` that `type_facts_of_flags` leaves:
    /// an object whose facts the caller needs, a union, an intersection.
    #[inline(never)]
    fn type_facts_slow(
        &mut self,
        t: TypeId,
        flags: TypeFlags,
        caller_only_needs: TypeFacts,
    ) -> TypeFacts {
        let strict = self.strict_null_checks;
        if flags.intersects(TypeFlags::OBJECT) {
            let object_flags = self.ty(t).object_flags;
            // PERF: chkfacts1. Go `isEmptyObjectType` of an object that is
            // not a mapped type is its members and `isEmptyResolvedType`
            // (`isGenericMappedType` is false), without the two calls.
            if object_flags.intersects(ObjectFlags::ANONYMOUS)
                && if object_flags.intersects(ObjectFlags::MAPPED) {
                    self.is_empty_object_type(t)
                } else {
                    self.resolve_structured_type_members(t);
                    self.is_empty_resolved_type(t)
                }
            {
                if strict {
                    return TypeFacts::EMPTY_OBJECT_STRICT_FACTS;
                }
                return TypeFacts::EMPTY_OBJECT_FACTS;
            } else if self.is_function_object_type(t) {
                if strict {
                    return TypeFacts::FUNCTION_STRICT_FACTS;
                }
                return TypeFacts::FUNCTION_FACTS;
            } else if strict {
                return TypeFacts::OBJECT_STRICT_FACTS;
            }
            return TypeFacts::OBJECT_FACTS;
        }
        if flags.intersects(TypeFlags::UNION) {
            // The list never changes after the type is made. PERF:
            // chkfacts1. The loop reads it in place (`detach`), not through
            // a handle copied on the stack.
            let mut buf: SharedListBuf<TypeId> = Default::default();
            let list = &self.ty(t).as_union_or_intersection_type().types;
            let Some(types) = list.detach(&mut buf) else {
                let types = list.clone();
                return self.union_type_facts(&types, caller_only_needs);
            };
            return self.union_type_facts(types, caller_only_needs);
        }
        self.get_intersection_type_facts(t, caller_only_needs)
    }

    /// The union arm of Go `getTypeFactsWorker` over the constituents
    /// `types`.
    #[inline(always)]
    fn union_type_facts(&mut self, types: &[TypeId], caller_only_needs: TypeFacts) -> TypeFacts {
        let mut facts = TypeFacts::NONE;
        for &m in types {
            let member_flags = self.ty(m).flags;
            facts |= self.constituent_type_facts(m, member_flags, caller_only_needs);
        }
        facts
    }

    // Go: checker/checker.go:31602 getIntersectionTypeFacts
    // PERF: chkfacts1. Go walks the list twice: `maybeTypeOfKind(t,
    // Primitive)`, then `getTypeFactsWorker` of each member. Here the first
    // walk does the kind test and also takes the facts of each member that
    // `type_facts_of_flags` gives: they read only the member and have no
    // effects. A second walk, only when a member needs more (a constraint, an
    // object whose facts the caller needs, a union), takes those members in
    // Go's order, so their effects come in Go's order. `|` and `&` give the
    // same facts in any order. The list is read in place (`detach`).
    pub fn get_intersection_type_facts(
        &mut self,
        t: TypeId,
        caller_only_needs: TypeFacts,
    ) -> TypeFacts {
        let mut buf: SharedListBuf<TypeId> = Default::default();
        let list = &self.ty(t).as_union_or_intersection_type().types;
        let Some(types) = list.detach(&mut buf) else {
            let types = list.clone();
            return self.intersection_type_facts(&types, caller_only_needs);
        };
        self.intersection_type_facts(types, caller_only_needs)
    }

    /// `get_intersection_type_facts` over the constituents `types`.
    #[inline(always)]
    fn intersection_type_facts(
        &mut self,
        types: &[TypeId],
        caller_only_needs: TypeFacts,
    ) -> TypeFacts {
        // When an intersection contains a primitive type we ignore object type constituents as they are
        // presumably type tags. For example, in string & { __kind__: "name" } we ignore the object type.
        let mut ignore_objects = false;
        // When computing the type facts of an intersection type, certain type facts are computed as `and`
        // and others are computed as `or`.
        let mut ored_facts = TypeFacts::NONE;
        let mut anded_facts = TypeFacts::ALL;
        // The flags-only facts of object members, which count only when
        // `ignore_objects` stays false.
        let mut ored_object_facts = TypeFacts::NONE;
        let mut anded_object_facts = TypeFacts::ALL;
        let mut second_walk = false;
        for &m in types {
            let member_flags = self.ty(m).flags;
            // Go `maybeTypeOfKind` of a member, as `maybe_type_of_kind`
            // tests it.
            if member_flags.intersects(TypeFlags::PRIMITIVE)
                || (member_flags.intersects(TypeFlags::UNION_OR_INTERSECTION)
                    && self.maybe_type_of_kind(m, TypeFlags::PRIMITIVE))
            {
                ignore_objects = true;
            }
            if member_flags.intersects(TypeFlags::INTERSECTION | TypeFlags::INSTANTIABLE) {
                second_walk = true;
                continue;
            }
            match self.type_facts_of_flags(m, member_flags, caller_only_needs) {
                Some(f) if member_flags.intersects(TypeFlags::OBJECT) => {
                    ored_object_facts |= f;
                    anded_object_facts &= f;
                }
                Some(f) => {
                    ored_facts |= f;
                    anded_facts &= f;
                }
                None => second_walk = true,
            }
        }
        if !ignore_objects {
            ored_facts |= ored_object_facts;
            anded_facts &= anded_object_facts;
        }
        if second_walk {
            for &m in types {
                let member_flags = self.ty(m).flags;
                if ignore_objects && member_flags.intersects(TypeFlags::OBJECT) {
                    continue;
                }
                let f =
                    if member_flags.intersects(TypeFlags::INTERSECTION | TypeFlags::INSTANTIABLE) {
                        self.get_type_facts_worker(m, caller_only_needs)
                    } else if self
                        .type_facts_of_flags(m, member_flags, caller_only_needs)
                        .is_none()
                    {
                        self.type_facts_slow(m, member_flags, caller_only_needs)
                    } else {
                        // Taken in the first walk.
                        continue;
                    };
                ored_facts |= f;
                anded_facts &= f;
            }
        }
        (ored_facts & TypeFacts::OR_FACTS_MASK) | (anded_facts & TypeFacts::AND_FACTS_MASK)
    }

    // Go: checker/checker.go:31620 isZeroBigInt
    pub fn is_zero_big_int(&self, t: TypeId) -> bool {
        // PORT: Go compares with `jsnum.PseudoBigInt{}` (the zero value).
        let v = self.get_big_int_literal_value(t);
        !v.negative && v.base10_value.is_empty()
    }

    // Go: checker/checker.go:31624 isFunctionObjectType
    pub fn is_function_object_type(&mut self, t: TypeId) -> bool {
        if self
            .ty(t)
            .object_flags
            .intersects(ObjectFlags::EVOLVING_ARRAY)
        {
            return false;
        }
        // lazymem1: with the switch on, the Go body of #64475
        // (lazy_members.rs). PERF: a resolved type does not test it.
        if !self
            .ty(t)
            .object_flags
            .intersects(ObjectFlags::MEMBERS_RESOLVED)
            && self.lazy_members
        {
            let lazy = crate::checker::lazy_members::opaque(
                Self::is_function_object_type_lazy as fn(&mut Self, TypeId) -> bool,
            );
            return lazy(self, t);
        }
        // We do a quick check for a "bind" property before performing the more expensive subtype
        // check. This gives us a quicker out in the common case where an object type is not a function.
        let resolved = self.resolve_structured_type_members(t);
        let has_signatures = !resolved.signatures().is_empty();
        let members = resolved.members;
        if has_signatures {
            return true;
        }
        let global_function_type = self.global_function_type;
        // PERF: unionfn1. Looked up by its interned name: no text hash or
        // text compare per call.
        static BIND: std::sync::LazyLock<Name> = std::sync::LazyLock::new(|| Name::from("bind"));
        self.symbols.get_name(members, &BIND).is_some()
            && self.is_type_subtype_of(t, global_function_type)
    }

    // Go: checker/checker.go:31634 getTypeWithFacts
    pub fn get_type_with_facts(&mut self, t: TypeId, include: TypeFacts) -> TypeId {
        self.filter_type(t, &mut |c: &mut Checker, t: TypeId| {
            c.has_type_facts(t, include)
        })
    }

    // Go: checker/checker.go:31643 getAdjustedTypeWithFacts
    // This function is similar to getTypeWithFacts, except that in strictNullChecks mode it replaces type
    // unknown with the union {} | null | undefined (and reduces that accordingly), and it intersects remaining
    // instantiable types with {}, {} | null, or {} | undefined in order to remove null and/or undefined.
    pub fn get_adjusted_type_with_facts(&mut self, t: TypeId, facts: TypeFacts) -> TypeId {
        let source = if self.strict_null_checks && self.ty(t).flags.intersects(TypeFlags::UNKNOWN) {
            self.unknown_union_type
        } else {
            t
        };
        let with_facts = self.get_type_with_facts(source, facts);
        let reduced = self.recombine_unknown_type(with_facts);
        if self.strict_null_checks {
            if facts == TypeFacts::NE_UNDEFINED {
                let null_type = self.null_type;
                return self.remove_nullable_by_intersection(
                    reduced,
                    TypeFacts::EQ_UNDEFINED,
                    TypeFacts::EQ_NULL,
                    TypeFacts::IS_NULL,
                    null_type,
                );
            } else if facts == TypeFacts::NE_NULL {
                let undefined_type = self.undefined_type;
                return self.remove_nullable_by_intersection(
                    reduced,
                    TypeFacts::EQ_NULL,
                    TypeFacts::EQ_UNDEFINED,
                    TypeFacts::IS_UNDEFINED,
                    undefined_type,
                );
            } else if facts == TypeFacts::NE_UNDEFINED_OR_NULL || facts == TypeFacts::TRUTHY {
                return self.map_type(reduced, &mut |c: &mut Checker, t: TypeId| {
                    if c.has_type_facts(t, TypeFacts::EQ_UNDEFINED_OR_NULL) {
                        return c.get_global_non_nullable_type_instantiation(t);
                    }
                    t
                });
            }
        }
        reduced
    }

    // Go: checker/checker.go:31663 removeNullableByIntersection
    pub fn remove_nullable_by_intersection(
        &mut self,
        t: TypeId,
        target_facts: TypeFacts,
        other_facts: TypeFacts,
        other_includes_facts: TypeFacts,
        other_type: TypeId,
    ) -> TypeId {
        let facts = self.get_type_facts(
            t,
            TypeFacts::EQ_UNDEFINED
                | TypeFacts::EQ_NULL
                | TypeFacts::IS_UNDEFINED
                | TypeFacts::IS_NULL,
        );
        // Simply return the type if it never compares equal to the target nullable.
        if !facts.intersects(target_facts) {
            return t;
        }
        // By default we intersect with a union of {} and the opposite nullable.
        let empty_object_type = self.empty_object_type;
        let empty_and_other_union = self.get_union_type(&[empty_object_type, other_type]);
        // For each constituent type that can compare equal to the target nullable, intersect with the above union
        // if the type doesn't already include the opposite nullable and the constituent can compare equal to the
        // opposite nullable; otherwise, just intersect with {}.
        self.map_type(t, &mut |c: &mut Checker, t: TypeId| {
            if c.has_type_facts(t, target_facts) {
                if !facts.intersects(other_includes_facts) && c.has_type_facts(t, other_facts) {
                    return c.get_intersection_type(&[t, empty_and_other_union]);
                }
                let empty_object_type = c.empty_object_type;
                return c.get_intersection_type(&[t, empty_object_type]);
            }
            t
        })
    }

    // Go: checker/checker.go:31685 recombineUnknownType
    pub fn recombine_unknown_type(&self, t: TypeId) -> TypeId {
        if t == self.unknown_union_type {
            return self.unknown_type;
        }
        t
    }

    // Go: checker/checker.go:31692 getGlobalNonNullableTypeInstantiation
    pub fn get_global_non_nullable_type_instantiation(&mut self, t: TypeId) -> TypeId {
        let resolver = self.get_global_non_nullable_type_alias_or_nil.clone();
        let alias = resolver(self);
        if alias.is_some() {
            return self.get_type_alias_instantiation(alias, &[t], None);
        }
        let empty_object_type = self.empty_object_type;
        self.get_intersection_type(&[t, empty_object_type])
    }

    // Go: checker/checker.go:31700 convertAutoToAny
    pub fn convert_auto_to_any(&self, t: TypeId) -> TypeId {
        if t == self.auto_type {
            return self.any_type;
        } else if t == self.auto_array_type {
            return self.any_array_type;
        }
        t
    }

    // Go: checker/checker.go:31716 checkAwaitedType
    // Gets the "awaited type" of a type.
    // @param type The type to await.
    // @param withAlias When `true`, wraps the "awaited type" in `Awaited<T>` if needed.
    // @remarks The "awaited type" of an expression is its "promised type" if the expression is a
    // Promise-like type; otherwise, it is the type of the expression. This is used to reflect
    // The runtime behavior of the `await` keyword.
    pub fn check_awaited_type(
        &mut self,
        t: TypeId,
        with_alias: bool,
        error_node: Node,
        diagnostic_message: &'static Message,
    ) -> TypeId {
        let awaited_type = if with_alias {
            self.get_awaited_type_ex(t, error_node, Some(diagnostic_message), Vec::new())
        } else {
            self.get_awaited_type_no_alias_ex(t, error_node, Some(diagnostic_message), Vec::new())
        };
        if awaited_type.is_some() {
            return awaited_type;
        }
        self.error_type
    }

    // Go: checker/checker.go:31737 getAwaitedType
    // Gets the "awaited type" of a type.
    //
    // The "awaited type" of an expression is its "promised type" if the expression is a
    // Promise-like type; otherwise, it is the type of the expression. If the "promised
    // type" is itself a Promise-like, the "promised type" is recursively unwrapped until a
    // non-promise type is found.
    //
    // This is used to reflect the runtime behavior of the `await` keyword.
    pub fn get_awaited_type(&mut self, t: TypeId) -> TypeId {
        self.get_awaited_type_ex(t, Node::NIL, None, Vec::new())
    }

    // Go: checker/checker.go:31741 getAwaitedTypeEx
    pub fn get_awaited_type_ex(
        &mut self,
        t: TypeId,
        error_node: Node,
        diagnostic_message: Option<&'static Message>,
        args: Vec<String>,
    ) -> TypeId {
        let awaited_type =
            self.get_awaited_type_no_alias_ex(t, error_node, diagnostic_message, args);
        if awaited_type.is_some() {
            return self.create_awaited_type_if_needed(awaited_type);
        }
        TypeId::NIL
    }

    // Go: checker/checker.go:31750 getAwaitedTypeNoAlias
    // Gets the "awaited type" of a type without introducing an `Awaited<T>` wrapper.
    pub fn get_awaited_type_no_alias(&mut self, t: TypeId) -> TypeId {
        self.get_awaited_type_no_alias_ex(t, Node::NIL, None, Vec::new())
    }

    // Go: checker/checker.go:31754 getAwaitedTypeNoAliasEx
    pub fn get_awaited_type_no_alias_ex(
        &mut self,
        t: TypeId,
        error_node: Node,
        diagnostic_message: Option<&'static Message>,
        args: Vec<String>,
    ) -> TypeId {
        if self.is_type_any(t) {
            return t;
        }
        // If this is already an `Awaited<T>`, just return it. This avoids `Awaited<Awaited<T>>` in higher-order
        if self.is_awaited_type_instantiation(t) {
            return t;
        }
        // If we've already cached an awaited type, return a possible `Awaited<T>` for it.
        let key = CachedTypeKey {
            kind: CachedTypeKind::AWAITED_TYPE,
            type_id: t,
        };
        if let Some(&awaited_type) = self.cached_types.get(&key) {
            if awaited_type.is_some() {
                return awaited_type;
            }
        }
        // For a union, get a union of the awaited types of each constituent.
        if self.ty(t).flags.intersects(TypeFlags::UNION) {
            if self.awaited_type_stack.contains(&t) {
                if error_node.is_some() {
                    self.error(
                        error_node,
                        diag::Type_is_referenced_directly_or_indirectly_in_the_fulfillment_callback_of_its_own_then_method,
                        args![],
                    );
                }
                return TypeId::NIL;
            }
            self.awaited_type_stack.push(t);
            let mapped = self.map_type(t, &mut |c: &mut Checker, t: TypeId| {
                c.get_awaited_type_no_alias_ex(t, error_node, diagnostic_message, args.clone())
            });
            self.awaited_type_stack.pop();
            self.cached_types.insert(key, mapped);
            return mapped;
        }
        // If `type` is generic and should be wrapped in `Awaited<T>`, return it.
        if self.is_awaited_type_needed(t) {
            self.cached_types.insert(key, t);
            return t;
        }
        let mut this_type_for_error = TypeId::NIL;
        let promised_type = self.get_promised_type_of_promise_ex(
            t,
            Node::NIL, /*errorNode*/
            Some(&mut this_type_for_error),
        );
        if promised_type.is_some() {
            if t == promised_type || self.awaited_type_stack.contains(&promised_type) {
                // Verify that we don't have a bad actor in the form of a promise whose
                // promised type is the same as the promise type, or a mutually recursive
                // promise. If so, we return undefined as we cannot guess the shape. If this
                // were the actual case in the JavaScript, this Promise would never resolve.
                //
                // An example of a bad actor with a singly-recursive promise type might
                // be:
                //
                //  interface BadPromise {
                //      then(
                //          onfulfilled: (value: BadPromise) => any,
                //          onrejected: (error: any) => any): BadPromise;
                //  }
                //
                // The above interface will pass the PromiseLike check, and return a
                // promised type of `BadPromise`. Since this is a self reference, we
                // don't want to keep recursing ad infinitum.
                //
                // An example of a bad actor in the form of a mutually-recursive
                // promise type might be:
                //
                //  interface BadPromiseA {
                //      then(
                //          onfulfilled: (value: BadPromiseB) => any,
                //          onrejected: (error: any) => any): BadPromiseB;
                //  }
                //
                //  interface BadPromiseB {
                //      then(
                //          onfulfilled: (value: BadPromiseA) => any,
                //          onrejected: (error: any) => any): BadPromiseA;
                //  }
                //
                if error_node.is_some() {
                    self.error(
                        error_node,
                        diag::Type_is_referenced_directly_or_indirectly_in_the_fulfillment_callback_of_its_own_then_method,
                        args![],
                    );
                }
                return TypeId::NIL;
            }
            // Keep track of the type we're about to unwrap to avoid bad recursive promise types.
            // See the comments above for more information.
            self.awaited_type_stack.push(t);
            let awaited_type = self.get_awaited_type_no_alias_ex(
                promised_type,
                error_node,
                diagnostic_message,
                args,
            );
            self.awaited_type_stack.pop();
            if awaited_type.is_nil() {
                return TypeId::NIL;
            }
            self.cached_types.insert(key, awaited_type);
            return awaited_type;
        }
        // The type was not a promise, so it could not be unwrapped any further.
        // As long as the type does not have a callable "then" property, it is
        // safe to return the type; otherwise, an error is reported and we return
        // undefined.
        //
        // An example of a non-promise "thenable" might be:
        //
        //  await { then(): void {} }
        //
        // The "thenable" does not match the minimal definition for a promise. When
        // a Promise/A+-compatible or ES6 promise tries to adopt this value, the promise
        // will never settle. We treat this as an error to help flag an early indicator
        // of a runtime problem. If the user wants to return this value from an async
        // function, they would need to wrap it in some other value. If they want it to
        // be treated as a promise, they can cast to <any>.
        if self.is_thenable_type(t) {
            if error_node.is_some() {
                let mut diagnostic: Option<Diagnostic> = None;
                if this_type_for_error.is_some() {
                    let type_string = self.type_to_string_exported(t);
                    let this_string = self.type_to_string_exported(this_type_for_error);
                    diagnostic = Some(new_diagnostic_for_node(
                        error_node,
                        diag::The_this_context_of_type_0_is_not_assignable_to_method_s_this_of_type_1,
                        args![type_string, this_string],
                    ));
                }
                // PORT: Go passes a possibly nil message; callers that pass an error node always pass one.
                let message =
                    diagnostic_message.expect("diagnostic message required with error node");
                self.add_diagnostic(new_diagnostic_chain_for_node(
                    diagnostic, error_node, message, args,
                ));
            }
            return TypeId::NIL;
        }
        self.cached_types.insert(key, t);
        t
    }

    // Go: checker/checker.go:31868 isAwaitedTypeInstantiation
    pub fn is_awaited_type_instantiation(&mut self, t: TypeId) -> bool {
        if self.ty(t).flags.intersects(TypeFlags::CONDITIONAL) {
            let resolver = self.get_global_awaited_symbol_or_nil.clone();
            let awaited_symbol = resolver(self);
            if awaited_symbol.is_nil() {
                return false;
            }
            return match &self.ty(t).alias {
                Some(alias) => alias.symbol == awaited_symbol && alias.type_arguments.len() == 1,
                None => false,
            };
        }
        false
    }

    // Go: checker/checker.go:31876 isAwaitedTypeNeeded
    pub fn is_awaited_type_needed(&mut self, t: TypeId) -> bool {
        // If this is already an `Awaited<T>`, we shouldn't wrap it. This helps to avoid `Awaited<Awaited<T>>` in higher-order.
        if self.is_type_any(t) || self.is_awaited_type_instantiation(t) {
            return false;
        }
        // We only need `Awaited<T>` if `T` contains possibly non-primitive types.
        if self.is_generic_object_type(t) {
            let base_constraint = self.get_base_constraint_of_type(t);
            // We only need `Awaited<T>` if `T` is a type variable that has no base constraint, or the base constraint of `T` is `any`, `unknown`, `{}`, `object`,
            // or is promise-like.
            if base_constraint.is_some() {
                return self
                    .ty(base_constraint)
                    .flags
                    .intersects(TypeFlags::ANY_OR_UNKNOWN)
                    || self.is_empty_object_type(base_constraint)
                    || self.some_type(base_constraint, &mut |c: &mut Checker, t: TypeId| {
                        c.is_thenable_type(t)
                    });
            }
            return self.maybe_type_of_kind(t, TypeFlags::TYPE_VARIABLE);
        }
        false
    }

    // Go: checker/checker.go:31894 createAwaitedTypeIfNeeded
    pub fn create_awaited_type_if_needed(&mut self, t: TypeId) -> TypeId {
        // We wrap type `T` in `Awaited<T>` based on the following conditions:
        // - `T` is not already an `Awaited<U>`, and
        // - `T` is generic, and
        // - One of the following applies:
        //   - `T` has no base constraint, or
        //   - The base constraint of `T` is `any`, `unknown`, `object`, or `{}`, or
        //   - The base constraint of `T` is an object type with a callable `then` method.
        if self.is_awaited_type_needed(t) {
            let awaited_type = self.try_create_awaited_type(t);
            if awaited_type.is_some() {
                return awaited_type;
            }
        }
        t
    }

    // Go: checker/checker.go:31911 tryCreateAwaitedType
    pub fn try_create_awaited_type(&mut self, t: TypeId) -> TypeId {
        // Nothing to do if `Awaited<T>` doesn't exist
        let resolver = self.get_global_awaited_symbol.clone();
        let awaited_symbol = resolver(self);
        if awaited_symbol.is_some() {
            // Unwrap unions that may contain `Awaited<T>`, otherwise its possible to manufacture an `Awaited<Awaited<T> | U>` where
            // an `Awaited<T | U>` would suffice.
            let unwrapped = self.unwrap_awaited_type(t);
            return self.get_type_alias_instantiation(awaited_symbol, &[unwrapped], None);
        }
        TypeId::NIL
    }

    // Go: checker/checker.go:31923 unwrapAwaitedType
    // For a generic `Awaited<T>`, gets `T`.
    pub fn unwrap_awaited_type(&mut self, t: TypeId) -> TypeId {
        if self.ty(t).flags.intersects(TypeFlags::UNION) {
            return self.map_type(t, &mut |c: &mut Checker, t: TypeId| {
                c.unwrap_awaited_type(t)
            });
        } else if self.is_awaited_type_instantiation(t) {
            return self
                .ty(t)
                .alias
                .as_ref()
                .expect("awaited alias")
                .type_arguments[0];
        }
        t
    }

    // Go: checker/checker.go:31933 isThenableType
    pub fn is_thenable_type(&mut self, t: TypeId) -> bool {
        let constraint = self.get_base_constraint_or_type(t);
        if self.all_types_assignable_to_kind(constraint, TypeFlags::PRIMITIVE | TypeFlags::NEVER) {
            // primitive types cannot be considered "thenable" since they are not objects.
            return false;
        }
        let then_function = self.get_type_of_property_of_type(t, "then");
        if then_function.is_nil() {
            return false;
        }
        let then_type = self.get_type_with_facts(then_function, TypeFacts::NE_UNDEFINED_OR_NULL);
        self.get_signatures_of_type(then_type, SignatureKind::CALL)
            .len()
            != 0
    }

    // Go: checker/checker.go:31942 getAwaitedTypeOfPromise
    pub fn get_awaited_type_of_promise(&mut self, t: TypeId) -> TypeId {
        self.get_awaited_type_of_promise_ex(t, Node::NIL, None, Vec::new())
    }

    // Go: checker/checker.go:31946 getAwaitedTypeOfPromiseEx
    pub fn get_awaited_type_of_promise_ex(
        &mut self,
        t: TypeId,
        error_node: Node,
        diagnostic_message: Option<&'static Message>,
        args: Vec<String>,
    ) -> TypeId {
        let promised_type = self.get_promised_type_of_promise_ex(t, error_node, None);
        if promised_type.is_some() {
            return self.get_awaited_type_ex(promised_type, error_node, diagnostic_message, args);
        }
        TypeId::NIL
    }

    // Go: checker/checker.go:31955 isSomeSymbolAssigned
    // Check if a parameter or catch variable (or their bindings elements) is assigned anywhere
    pub fn is_some_symbol_assigned(&mut self, root_declaration: Node) -> bool {
        self.is_some_symbol_assigned_worker(root_declaration.name())
    }

    // Go: checker/checker.go:31959 isSomeSymbolAssignedWorker
    pub fn is_some_symbol_assigned_worker(&mut self, node: Node) -> bool {
        if node.kind() == SyntaxKind::Identifier {
            let symbol = self.get_symbol_of_declaration(node.parent());
            return self.is_symbol_assigned(symbol);
        }
        node.elements()
            .iter()
            .any(|e| e.name().is_some() && self.is_some_symbol_assigned_worker(e.name()))
    }

    // Go: checker/checker.go:31968 getTargetType
    // PORT: Go has both a package func `getTargetType` (checker.go:19465, ported in
    // checker_p22 as `Checker::get_target_type`) and this method with identical logic.
    // Both would be `get_target_type` on `Checker`, so this one gets a `_method` suffix.
    // Callers of either Go form can use `get_target_type`.
    pub fn get_target_type_method(&self, t: TypeId) -> TypeId {
        if self.ty(t).object_flags.intersects(ObjectFlags::REFERENCE) {
            return self.ty(t).as_type_reference().object.target;
        }
        t
    }

    // Go: checker/checker.go:31975 getNarrowableTypeForReference
    pub fn get_narrowable_type_for_reference(
        &mut self,
        t: TypeId,
        reference: Node,
        check_mode: CheckMode,
    ) -> TypeId {
        let mut t = t;
        if self.is_no_infer_type(t) {
            t = self.ty(t).as_substitution_type().base_type;
        }
        // When the type of a reference is or contains an instantiable type with a union type constraint, and
        // when the reference is in a constraint position (where it is known we'll obtain the apparent type) or
        // has a contextual type containing no top-level instantiables (meaning constraints will determine
        // assignability), we substitute constraints for all instantiables in the type of the reference to give
        // control flow analysis an opportunity to narrow it further. For example, for a reference of a type
        // parameter type 'T extends string | undefined' with a contextual type 'string', we substitute
        // 'string | undefined' to give control flow analysis the opportunity to narrow to type 'string'.
        let substitute_constraints = !check_mode.intersects(CheckMode::INFERENTIAL)
            && self.some_type(t, &mut |c: &mut Checker, t: TypeId| {
                c.is_generic_type_with_union_constraint(t)
            })
            && (self.is_constraint_position(t, reference)
                || self.has_contextual_type_with_no_generic_types(reference, check_mode));
        if substitute_constraints {
            return self.map_type(t, &mut |c: &mut Checker, t: TypeId| {
                c.get_base_constraint_or_type(t)
            });
        }
        t
    }

    // Go: checker/checker.go:31993 isConstraintPosition
    pub fn is_constraint_position(&mut self, t: TypeId, node: Node) -> bool {
        let parent = node.parent();
        // In an element access obj[x], we consider obj to be in a constraint position, except when obj is of
        // a generic type without a nullable constraint and x is a generic type. This is because when both obj
        // and x are of generic types T and K, we want the resulting type to be T[K].
        if is_property_access_expression(parent) || is_qualified_name(parent) {
            return true;
        }
        if (is_call_expression(parent) || is_new_expression(parent)) && parent.expression() == node
        {
            return true;
        }
        is_element_access_expression(parent)
            && parent.expression() == node
            && !(self.some_type(t, &mut |c: &mut Checker, t: TypeId| {
                c.is_generic_type_without_nullable_constraint(t)
            }) && {
                let index_type = self.get_type_of_expression(parent.argument_expression());
                self.is_generic_index_type(index_type)
            })
    }

    // Go: checker/checker.go:32003 isGenericTypeWithUnionConstraint
    pub fn is_generic_type_with_union_constraint(&mut self, t: TypeId) -> bool {
        if self.ty(t).flags.intersects(TypeFlags::INTERSECTION) {
            return (0..self.ty(t).types().len())
                .any(|i| self.is_generic_type_with_union_constraint(self.type_at(t, i)));
        }
        if !self.ty(t).flags.intersects(TypeFlags::INSTANTIABLE) {
            return false;
        }
        let constraint = self.get_base_constraint_or_type(t);
        self.ty(constraint)
            .flags
            .intersects(TypeFlags::NULLABLE | TypeFlags::UNION)
    }

    // Go: checker/checker.go:32010 isGenericTypeWithoutNullableConstraint
    pub fn is_generic_type_without_nullable_constraint(&mut self, t: TypeId) -> bool {
        if self.ty(t).flags.intersects(TypeFlags::INTERSECTION) {
            return (0..self.ty(t).types().len())
                .any(|i| self.is_generic_type_without_nullable_constraint(self.type_at(t, i)));
        }
        if !self.ty(t).flags.intersects(TypeFlags::INSTANTIABLE) {
            return false;
        }
        let constraint = self.get_base_constraint_or_type(t);
        !self.maybe_type_of_kind(constraint, TypeFlags::NULLABLE)
    }

    // Go: checker/checker.go:32017 hasContextualTypeWithNoGenericTypes
    pub fn has_contextual_type_with_no_generic_types(
        &mut self,
        node: Node,
        check_mode: CheckMode,
    ) -> bool {
        // Computing the contextual type for a child of a JSX element involves resolving the type of the
        // element's tag name, so we exclude that here to avoid circularities.
        // If check mode has `CheckMode.RestBindingElement`, we skip binding pattern contextual types,
        // as we want the type of a rest element to be generic when possible.
        if (is_identifier(node)
            || is_property_access_expression(node)
            || is_element_access_expression(node))
            && !((is_jsx_opening_element(node.parent())
                || is_jsx_self_closing_element(node.parent()))
                && node.parent().tag_name() == node)
        {
            let context_flags = if check_mode.intersects(CheckMode::REST_BINDING_ELEMENT) {
                ContextFlags::SKIP_BINDING_PATTERNS
            } else {
                ContextFlags::NONE
            };
            let contextual_type = self.get_contextual_type(node, context_flags);
            if contextual_type.is_some() {
                return !self.is_generic_type(contextual_type);
            }
        }
        false
    }

    // Go: checker/checker.go:32032 getNonUndefinedType
    pub fn get_non_undefined_type(&mut self, t: TypeId) -> TypeId {
        let mut type_or_constraint = t;
        if self.some_type(t, &mut |c: &mut Checker, t: TypeId| {
            c.is_generic_type_with_undefined_constraint(t)
        }) {
            type_or_constraint = self.map_type(t, &mut |c: &mut Checker, t: TypeId| {
                if c.ty(t).flags.intersects(TypeFlags::INSTANTIABLE) {
                    return c.get_base_constraint_or_type(t);
                }
                t
            });
        }
        self.get_type_with_facts(type_or_constraint, TypeFacts::NE_UNDEFINED)
    }

    // Go: checker/checker.go:32045 isGenericTypeWithUndefinedConstraint
    pub fn is_generic_type_with_undefined_constraint(&mut self, t: TypeId) -> bool {
        if self.ty(t).flags.intersects(TypeFlags::INSTANTIABLE) {
            let constraint = self.get_base_constraint_of_type(t);
            if constraint.is_some() {
                return self.maybe_type_of_kind(constraint, TypeFlags::UNDEFINED);
            }
        }
        false
    }

    // Go: checker/checker.go:32055 getActualTypeVariable
    pub fn get_actual_type_variable(&mut self, t: TypeId) -> TypeId {
        let flags = self.ty(t).flags;
        if flags.intersects(TypeFlags::SUBSTITUTION) {
            let base_type = self.ty(t).as_substitution_type().base_type;
            return self.get_actual_type_variable(base_type);
        }
        if flags.intersects(TypeFlags::INDEXED_ACCESS) {
            let t_object_type = self.ty(t).as_indexed_access_type().object_type;
            let t_index_type = self.ty(t).as_indexed_access_type().index_type;
            let object_type = self.get_actual_type_variable(t_object_type);
            let index_type = self.get_actual_type_variable(t_index_type);
            if object_type != t_object_type || index_type != t_index_type {
                return self.get_indexed_access_type(object_type, index_type);
            }
        }
        self.get_non_distributed_type_parameter(t)
    }

    // Go: checker/checker.go:32069 GetSymbolAtLocation
    pub fn get_symbol_at_location_exported(&mut self, node: Node) -> SymbolId {
        // !!!
        // const node = getParseTreeNode(nodeIn);

        // set ignoreErrors: true because any lookups invoked by the API shouldn't cause any new errors
        self.get_symbol_at_location(get_reparsed_node_for_node(node), true /*ignoreErrors*/)
    }

    // Go: checker/checker.go:32082 getSymbolAtLocation
    // Returns the symbol associated with a given AST node. Do *not* use this function in the checker itself! It should
    // be used only by the language service and external tools. The semantics of the function are deliberately "fuzzy"
    // and aim to just return *some* symbol for the node. To obtain the symbol associated with a node for type checking
    // purposes, use appropriate function for the context, e.g. `getResolvedSymbol` for an expression identifier,
    // `getSymbolOfDeclaration` for a declaration, etc.
    pub fn get_symbol_at_location(&mut self, node: Node, ignore_errors: bool) -> SymbolId {
        if is_source_file(node) {
            if is_external_or_common_js_module(node) {
                return self.get_merged_symbol(node.symbol());
            }
            return SymbolId::NIL;
        }
        let parent = node.parent();
        let grand_parent = parent.parent();

        if node.flags().intersects(NodeFlags::IN_WITH_STATEMENT) {
            // We cannot answer semantic questions within a with block, do not proceed any further
            return SymbolId::NIL;
        }

        if is_declaration_name_or_import_property_name(node) {
            // This is a declaration, call getSymbolOfNode
            let parent_symbol = self.get_symbol_of_declaration(parent);
            if is_import_or_export_specifier(parent) && parent.property_name() == node {
                return self.get_immediate_aliased_symbol(parent_symbol);
            }
            return parent_symbol;
        } else if is_literal_computed_property_declaration_name(node) {
            return self.get_symbol_of_declaration(grand_parent);
        }

        if is_identifier(node) {
            if is_in_right_side_of_import_or_export_assignment(node) {
                return self.get_symbol_of_name_or_property_access_expression(node);
            } else if is_binding_element(parent)
                && is_object_binding_pattern(grand_parent)
                && node == parent.property_name()
            {
                let type_of_pattern = self.get_type_of_node(grand_parent);
                let property_declaration = self.get_property_of_type(type_of_pattern, node.text());
                if property_declaration.is_some() {
                    return property_declaration;
                }
            } else if is_meta_property(parent) && parent.name() == node {
                let keyword_token = parent.keyword_token();
                if keyword_token == SyntaxKind::NewKeyword && node.text() == "target" {
                    // `target` in `new.target`
                    let t = self.check_new_target_meta_property(parent);
                    return self.ty(t).symbol;
                }
                // The `meta` in `import.meta` could be given `getTypeOfNode(parent).symbol` (the `ImportMeta` interface symbol), but
                // we have a fake expression type made for other reasons already, whose transient `meta`
                // member should more exactly be the kind of (declarationless) symbol we want.
                // (See #44364 and #45031 for relevant implementation PRs)
                if keyword_token == SyntaxKind::ImportKeyword && node.text() == "meta" {
                    let t = self.get_global_import_meta_expression_type();
                    let members = self.ty(t).as_object_type().structured.members;
                    return self.symbols.get(members, "meta");
                }
                // no other meta properties are valid syntax, thus no others should have symbols
                return SymbolId::NIL;
            } else if is_js_doc_parameter_tag(parent) && parent.name() == node {
                let fn_ = get_node_at_position(get_source_file_of_node(node), node.pos(), false);
                if fn_.is_some() && is_function_like(fn_) {
                    for param in fn_.parameters() {
                        if is_identifier(param.name()) && param.name().text() == node.text() {
                            return self.get_symbol_of_node(param);
                        }
                    }
                }
            }
        }

        let kind = node.kind();
        match kind {
            SyntaxKind::Identifier
            | SyntaxKind::PrivateIdentifier
            | SyntaxKind::PropertyAccessExpression
            | SyntaxKind::QualifiedName
            | SyntaxKind::ThisKeyword
            | SyntaxKind::ThisType => {
                // PORT: Go `fallthrough` from the name kinds into ThisKeyword, then into ThisType.
                if matches!(
                    kind,
                    SyntaxKind::Identifier
                        | SyntaxKind::PrivateIdentifier
                        | SyntaxKind::PropertyAccessExpression
                        | SyntaxKind::QualifiedName
                ) && !is_this_in_type_query(node)
                {
                    return self.get_symbol_of_name_or_property_access_expression(node);
                }
                if kind != SyntaxKind::ThisType {
                    let container = self.get_this_container(
                        node, false, /*includeArrowFunctions*/
                        false, /*includeClassComputedPropertyName*/
                    );
                    if is_function_like(container) {
                        let sig = self.get_signature_from_declaration(container);
                        let this_parameter = self.sig(sig).this_parameter;
                        if this_parameter.is_some() {
                            return this_parameter;
                        }
                    }
                    if is_in_expression_context(node) {
                        let t = self.check_expression(node);
                        return self.ty(t).symbol;
                    }
                }
                let t = self.get_type_from_this_type_node(node);
                self.ty(t).symbol
            }
            SyntaxKind::SuperKeyword => {
                let t = self.check_expression(node);
                self.ty(t).symbol
            }
            SyntaxKind::ConstructorKeyword => {
                // constructor keyword for an overload, should take us to the definition if it exist
                let constructor_declaration = parent;
                if constructor_declaration.is_some()
                    && constructor_declaration.kind() == SyntaxKind::Constructor
                {
                    return constructor_declaration.parent().symbol();
                }
                SymbolId::NIL
            }
            SyntaxKind::StringLiteral
            | SyntaxKind::NoSubstitutionTemplateLiteral
            | SyntaxKind::NumericLiteral => {
                // PORT: Go `fallthrough` from the string literal kinds into NumericLiteral.
                if kind != SyntaxKind::NumericLiteral {
                    // 1). import x = require("./mo/*gotToDefinitionHere*/d")
                    // 2). External module name in an import declaration
                    // 3). Require in Javascript
                    // 4). type A = import("./f/*gotToDefinitionHere*/oo")
                    if (is_external_module_import_equals_declaration(grand_parent)
                        && get_external_module_import_equals_declaration_expression(grand_parent)
                            == node)
                        || ((parent.kind() == SyntaxKind::ImportDeclaration
                            || parent.kind() == SyntaxKind::JsImportDeclaration
                            || parent.kind() == SyntaxKind::ExportDeclaration)
                            && get_external_module_name(parent) == node)
                        || is_variable_declaration_initialized_to_require(grand_parent)
                        || is_import_call(parent)
                        || (is_literal_type_node(parent)
                            && is_literal_import_type_node(grand_parent)
                            && grand_parent.argument() == parent)
                    {
                        let import_attributes_type =
                            self.get_import_attributes_type_for_module_specifier(node);
                        return self.resolve_external_module_name(
                            node,
                            node,
                            ignore_errors,
                            import_attributes_type,
                        );
                    }
                    if is_call_expression(parent)
                        && is_bindable_object_define_property_call(parent)
                        && parent.arguments().get(1) == node
                    {
                        return self.get_symbol_of_declaration(parent);
                    }
                }
                // index access
                let mut object_type = TypeId::NIL;
                if is_element_access_expression(parent) {
                    if parent.argument_expression() == node {
                        object_type = self.get_type_of_expression(parent.expression());
                    }
                } else if is_literal_type_node(parent) && is_indexed_access_type_node(grand_parent)
                {
                    object_type = self.get_type_from_type_node(grand_parent.object_type());
                }

                if object_type.is_some() {
                    return self.get_property_of_type(object_type, node.text());
                }
                SymbolId::NIL
            }
            SyntaxKind::DefaultKeyword
            | SyntaxKind::FunctionKeyword
            | SyntaxKind::EqualsGreaterThanToken
            | SyntaxKind::ClassKeyword => self.get_symbol_of_node(node.parent()),
            SyntaxKind::ImportType => {
                if is_literal_import_type_node(node) {
                    return self.get_symbol_at_location(node.argument().literal(), ignore_errors);
                }
                SymbolId::NIL
            }
            SyntaxKind::ExportKeyword => {
                if is_export_assignment(parent) {
                    if parent.symbol().is_nil() {
                        panic!("Symbol should be defined");
                    }
                    return parent.symbol();
                }
                SymbolId::NIL
            }
            SyntaxKind::ImportKeyword | SyntaxKind::NewKeyword => {
                // PORT: Go `fallthrough` from ImportKeyword into NewKeyword.
                if kind == SyntaxKind::ImportKeyword
                    && is_meta_property(node.parent())
                    && node.parent().text() == "defer"
                {
                    return SymbolId::NIL;
                }
                if is_meta_property(parent) {
                    let t = self.check_meta_property_keyword(parent);
                    return self.ty(t).symbol;
                }
                SymbolId::NIL
            }
            SyntaxKind::InstanceOfKeyword => {
                if is_binary_expression(parent) {
                    let t = self.get_type_of_expression(parent.right());
                    let has_instance_method_type =
                        self.get_symbol_has_instance_method_of_object_type(t);
                    if has_instance_method_type.is_some()
                        && self.ty(has_instance_method_type).symbol.is_some()
                    {
                        return self.ty(has_instance_method_type).symbol;
                    }
                    return self.ty(t).symbol;
                }
                SymbolId::NIL
            }
            SyntaxKind::MetaProperty => {
                let t = self.check_expression(node);
                self.ty(t).symbol
            }
            SyntaxKind::JsxNamespacedName => {
                // PORT: Go `fallthrough` into default, which returns nil.
                if is_jsx_tag_name(node) && is_jsx_intrinsic_tag_name(node) {
                    let symbol = self.get_intrinsic_tag_symbol(node.parent());
                    if symbol == self.unknown_symbol {
                        return SymbolId::NIL;
                    }
                    return symbol;
                }
                SymbolId::NIL
            }
            _ => SymbolId::NIL,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Writes `aliases` as `type T<i> = <alias>;` lines of `a.ts` in a new
    /// project (`type T<i><alias>;` for an alias that starts with `<`, so it
    /// can have type parameters), with `strictNullChecks` set to `strict`,
    /// and runs `f` on the checker of `a.ts` with the type of each alias, in
    /// order.
    fn with_alias_types<R: Send + 'static>(
        label: &str,
        aliases: &[&str],
        strict: bool,
        f: impl FnOnce(&mut Checker, &[TypeId]) -> R + Send + 'static,
    ) -> R {
        let dir =
            std::env::temp_dir().join(format!("ts_goport_{label}_{strict}_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let source: String = aliases
            .iter()
            .enumerate()
            .map(|(i, alias)| {
                if alias.starts_with('<') {
                    format!("type T{i}{alias};\n")
                } else {
                    format!("type T{i} = {alias};\n")
                }
            })
            .collect();
        std::fs::write(dir.join("a.ts"), source).unwrap();
        std::fs::write(
            dir.join("tsconfig.json"),
            format!(
                r#"{{ "compilerOptions": {{ "strictNullChecks": {strict}, "target": "es2020", "types": [] }}, "files": ["a.ts"] }}"#
            ),
        )
        .unwrap();
        let config = dir.join("tsconfig.json");
        let program = crate::program::try_load_version(&config.to_string_lossy(), |_| {})
            .unwrap_or_else(|e| panic!("cannot load {}: {e}", config.display()));
        let _ = std::fs::remove_dir_all(&dir);
        let scope = crate::core::enter_program(Some(program));
        let file = program
            .source_files()
            .find(|file| file.info.file_name.ends_with("/a.ts"))
            .expect("a.ts is not in the program")
            .root;
        let result = crate::program::with_type_checker_for_file(file, move |checker| {
            let types: Vec<TypeId> = file
                .statements()
                .iter()
                .map(|alias| checker.get_type_from_type_node(alias.type_()))
                .collect();
            f(checker, &types)
        });
        drop(scope);
        crate::program::release_program(program);
        result
    }

    /// Go `getStringLiteralType` (checker.go:25770) keys its map on the
    /// value. The port keys `string_literal_types` on the value's FxHash and
    /// compares each type's value in the bucket (`checker_p28.rs`
    /// `get_string_literal_type_cow`, tmpl1). The test puts the type of "b"
    /// in the buckets of "a" and "c", as a hash collision would: "a" still
    /// gives its own type, and "c" makes a new type once.
    #[test]
    fn string_literal_types_compare_values_in_a_hash_bucket() {
        use std::hash::BuildHasher;
        let aliases = [r#""a""#, r#""b""#];
        let got = with_alias_types("strlit_bucket", &aliases, true, |checker, types| {
            let (a, b) = (types[0], types[1]);
            assert_eq!(checker.get_string_literal_type("a"), a);
            assert_eq!(checker.get_string_literal_type("b"), b);
            for value in ["a", "c"] {
                let hash = rustc_hash::FxBuildHasher.hash_one(value);
                checker
                    .string_literal_types
                    .entry(hash)
                    .or_default()
                    .insert(0, b);
            }
            let c = checker.get_string_literal_type("c");
            (
                checker.get_string_literal_type("a") == a,
                c != b && c != a,
                checker.get_string_literal_value_ref(c) == "c",
                checker.get_string_literal_type("c") == c,
                checker.get_string_literal_type("b") == b,
            )
        });
        assert_eq!(got, (true, true, true, true, true));
    }

    /// Go `maybeTypeOfKind` recurses into a union or intersection
    /// constituent (checker.go:28071). unionfn1 tests the flags of other
    /// constituents in its loop.
    #[test]
    fn maybe_type_of_kind_looks_into_nested_constituents() {
        let aliases = [
            "(string & { a: 1 }) | { b: 1 }",
            "{ a: 1 } | { b: 1 }",
            "number | { b: 1 }",
        ];
        let got = with_alias_types("maybe_kind", &aliases, true, |checker, types| {
            types
                .iter()
                .map(|&t| {
                    (
                        checker.maybe_type_of_kind(t, TypeFlags::STRING),
                        checker.maybe_type_of_kind(t, TypeFlags::NUMBER),
                    )
                })
                .collect::<Vec<_>>()
        });
        assert_eq!(got, [(true, false), (false, false), (false, true)]);
    }

    /// Go `getTypeFactsWorker` (checker.go:31466) and
    /// `getIntersectionTypeFacts` (:31602) for each arm, with and without
    /// `strictNullChecks`. unionfn1 split the switch in two and reads
    /// union and intersection constituents in its own loops.
    #[test]
    fn type_facts_match_go_arms() {
        type F = TypeFacts;
        let object = |strict: bool| {
            if strict {
                F::OBJECT_STRICT_FACTS
            } else {
                F::OBJECT_FACTS
            }
        };
        let function = |strict: bool| {
            if strict {
                F::FUNCTION_STRICT_FACTS
            } else {
                F::FUNCTION_FACTS
            }
        };
        // (alias, facts with strictNullChecks, facts without).
        let cases = [
            ("string", F::STRING_STRICT_FACTS, F::STRING_FACTS),
            (
                r#""" | "a""#,
                F::EMPTY_STRING_STRICT_FACTS | F::NON_EMPTY_STRING_STRICT_FACTS,
                F::EMPTY_STRING_FACTS | F::NON_EMPTY_STRING_FACTS,
            ),
            (
                "`a${string}`",
                F::NON_EMPTY_STRING_STRICT_FACTS,
                F::NON_EMPTY_STRING_FACTS,
            ),
            ("number", F::NUMBER_STRICT_FACTS, F::NUMBER_FACTS),
            (
                "0 | 1",
                F::ZERO_NUMBER_STRICT_FACTS | F::NON_ZERO_NUMBER_STRICT_FACTS,
                F::ZERO_NUMBER_FACTS | F::NON_ZERO_NUMBER_FACTS,
            ),
            ("bigint", F::BIG_INT_STRICT_FACTS, F::BIG_INT_FACTS),
            (
                "0n | 1n",
                F::ZERO_BIG_INT_STRICT_FACTS | F::NON_ZERO_BIG_INT_STRICT_FACTS,
                F::ZERO_BIG_INT_FACTS | F::NON_ZERO_BIG_INT_FACTS,
            ),
            ("boolean", F::BOOLEAN_STRICT_FACTS, F::BOOLEAN_FACTS),
            ("false", F::FALSE_STRICT_FACTS, F::FALSE_FACTS),
            ("true", F::TRUE_STRICT_FACTS, F::TRUE_FACTS),
            (
                r#""" | 0 | 0n | false | "a""#,
                F::EMPTY_STRING_STRICT_FACTS
                    | F::ZERO_NUMBER_STRICT_FACTS
                    | F::ZERO_BIG_INT_STRICT_FACTS
                    | F::FALSE_STRICT_FACTS
                    | F::NON_EMPTY_STRING_STRICT_FACTS,
                F::EMPTY_STRING_FACTS
                    | F::ZERO_NUMBER_FACTS
                    | F::ZERO_BIG_INT_FACTS
                    | F::FALSE_FACTS
                    | F::NON_EMPTY_STRING_FACTS,
            ),
            ("void", F::VOID_FACTS, F::VOID_FACTS),
            ("undefined", F::UNDEFINED_FACTS, F::UNDEFINED_FACTS),
            ("null", F::NULL_FACTS, F::NULL_FACTS),
            ("symbol", F::SYMBOL_STRICT_FACTS, F::SYMBOL_FACTS),
            ("object", object(true), object(false)),
            ("never", F::NONE, F::NONE),
            ("unknown", F::UNKNOWN_FACTS, F::UNKNOWN_FACTS),
            ("{}", F::EMPTY_OBJECT_STRICT_FACTS, F::EMPTY_OBJECT_FACTS),
            ("{ a: 1 }", object(true), object(false)),
            ("() => void", function(true), function(false)),
            // A primitive in an intersection: the object constituent is ignored.
            (
                r#"string & { __kind: "name" }"#,
                F::STRING_STRICT_FACTS,
                F::STRING_FACTS,
            ),
            // No primitive: facts are or-ed and and-ed by the two masks.
            (
                "{ a: 1 } & (() => void)",
                ((object(true) | function(true)) & F::OR_FACTS_MASK)
                    | (object(true) & function(true) & F::AND_FACTS_MASK),
                ((object(false) | function(false)) & F::OR_FACTS_MASK)
                    | (object(false) & function(false) & F::AND_FACTS_MASK),
            ),
            // The base constraint step: a constraint, and none (unknown).
            (
                r#"<X extends "" | 0> = X"#,
                F::EMPTY_STRING_STRICT_FACTS | F::ZERO_NUMBER_STRICT_FACTS,
                F::EMPTY_STRING_FACTS | F::ZERO_NUMBER_FACTS,
            ),
            ("<X> = X", F::UNKNOWN_FACTS, F::UNKNOWN_FACTS),
            // An intersection constituent of a union.
            (
                "(string & { a: 1 }) | number",
                F::STRING_STRICT_FACTS | F::NUMBER_STRICT_FACTS,
                F::STRING_FACTS | F::NUMBER_FACTS,
            ),
        ];
        let aliases: Vec<&str> = cases.iter().map(|case| case.0).collect();
        for strict in [true, false] {
            let got = with_alias_types("type_facts", &aliases, strict, |checker, types| {
                types
                    .iter()
                    .map(|&t| checker.get_type_facts(t, TypeFacts::ALL))
                    .collect::<Vec<_>>()
            });
            assert_eq!(got.len(), cases.len());
            for (case, got) in cases.iter().zip(got) {
                let want = if strict { case.1 } else { case.2 };
                assert_eq!(
                    got.0, want.0,
                    "facts of {} (strictNullChecks {strict})",
                    case.0
                );
            }
        }
    }

    /// Go `getIntersectionTypeFacts` (checker.go:31602) where chkfacts1's
    /// first walk takes some members and its second walk the others: a
    /// primitive after an object, a member that needs its constraint, and a
    /// caller that needs no object facts.
    #[test]
    fn intersection_type_facts_match_go() {
        type F = TypeFacts;
        let aliases = [
            r#"{ __kind: "name" } & string"#,
            "<X extends string> = { a: 1 } & X",
            "{ a: 1 } & { b: 1 }",
        ];
        for strict in [true, false] {
            let (object, string) = if strict {
                (F::OBJECT_STRICT_FACTS, F::STRING_STRICT_FACTS)
            } else {
                (F::OBJECT_FACTS, F::STRING_FACTS)
            };
            let both =
                ((object | string) & F::OR_FACTS_MASK) | (object & string & F::AND_FACTS_MASK);
            // Go's object arm gives none when the caller needs none of the
            // object facts.
            let possible = if strict {
                F::EMPTY_OBJECT_STRICT_FACTS | F::FUNCTION_STRICT_FACTS | F::OBJECT_STRICT_FACTS
            } else {
                F::EMPTY_OBJECT_FACTS | F::FUNCTION_FACTS | F::OBJECT_FACTS
            };
            let needs = F::EQ_UNDEFINED;
            let object_for_needs = if needs.intersects(possible) {
                object
            } else {
                F::NONE
            };
            let objects = ((object_for_needs & F::OR_FACTS_MASK)
                | (object_for_needs & F::AND_FACTS_MASK))
                & needs;
            let got = with_alias_types("isect_facts", &aliases, strict, move |checker, types| {
                vec![
                    checker.get_type_facts(types[0], F::ALL),
                    checker.get_intersection_type_facts(types[1], F::ALL),
                    // The base constraint of the intersection is
                    // `{ a: 1 } & string`.
                    checker.get_type_facts(types[1], F::ALL),
                    checker.get_type_facts(types[2], F::ALL),
                    checker.get_type_facts(types[2], needs),
                ]
            });
            assert_eq!(
                got,
                [string, both, string, object, objects],
                "strictNullChecks {strict}"
            );
        }
    }

    /// Go `getBaseConstraintOfType` (checker.go:27901) gives the same
    /// constraint before and after it is resolved (chkfacts1 reads a
    /// resolved one inline): a constraint, none, a circular one and an
    /// intersection (its own constraint).
    #[test]
    fn base_constraint_of_type_when_resolved() {
        let aliases = [
            "<X extends string> = X",
            "<X> = X",
            "<X extends Y, Y extends X> = X",
            "{ a: 1 } & { b: 2 }",
        ];
        let got = with_alias_types("base_constraint", &aliases, true, |checker, types| {
            let string_type = checker.string_type;
            types
                .iter()
                .map(|&t| {
                    let first = checker.get_base_constraint_of_type(t);
                    let second = checker.get_base_constraint_of_type(t);
                    assert_eq!(first, second);
                    if first.is_nil() {
                        "none"
                    } else if first == string_type {
                        "string"
                    } else if first == t {
                        "itself"
                    } else {
                        "other"
                    }
                })
                .collect::<Vec<_>>()
        });
        assert_eq!(got, ["string", "none", "none", "itself"]);
    }
}
