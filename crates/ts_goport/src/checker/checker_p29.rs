use crate::prelude::*;

impl Checker {
    // Go: checker/checker.go:26489 intersectTypes
    pub fn intersect_types(&mut self, type1: TypeId, type2: TypeId) -> TypeId {
        if type1.is_nil() {
            return type2;
        }
        if type2.is_nil() {
            return type1;
        }
        self.get_intersection_type(&[type1, type2])
    }

    // We normalize combinations of intersection and union types based on the distributive property of the '&'
    // operator. Specifically, because X & (A | B) is equivalent to X & A | X & B, we can transform intersection
    // types with union type constituents into equivalent union types with intersection type constituents and
    // effectively ensure that union types are always at the top level in type representations.
    //
    // We do not perform structural deduplication on intersection types. Intersection types are created only by the &
    // type operator and we can't reduce those because we want to support recursive intersection types. For example,
    // a type alias of the form "type List<T> = T & { next: List<T> }" cannot be reduced during its declaration.
    // Also, unlike union types, the order of the constituent types is preserved in order that overload resolution
    // for intersections of types with signatures can be deterministic.
    // Go: checker/checker.go:26517 getIntersectionType
    pub fn get_intersection_type(&mut self, types: &[TypeId]) -> TypeId {
        self.get_intersection_type_ex(types, IntersectionFlags::NONE, None /*alias*/)
    }

    // Go: checker/checker.go:26521 getIntersectionTypeEx
    // PORT: Go `orderedSet[*Type]` is a stack `TypeSet` here. It keeps
    // insertion order and `contains`/`add` match Go (Go only adds values it
    // has not seen). Intersections are small, so a linear `contains` is
    // cheaper than a hash set. The reductions below change the set in place,
    // and `new_intersection_type` copies it only when it creates the type.
    pub fn get_intersection_type_ex(
        &mut self,
        types: &[TypeId],
        flags: IntersectionFlags,
        alias: Option<Rc<TypeAlias>>,
    ) -> TypeId {
        let mut type_set = TypeSet::with_capacity(types.len());
        let includes = self.add_types_to_intersection(&mut type_set, TypeFlags::NONE, types);
        let mut object_flags = ObjectFlags::NONE;
        // An intersection type is considered empty if it contains
        // the type never, or
        // more than one unit type or,
        // an object type and a nullable type (null or undefined), or
        // a string-like type and a type known to be non-string-like, or
        // a number-like type and a type known to be non-number-like, or
        // a symbol-like type and a type known to be non-symbol-like, or
        // a void-like type and a type known to be non-void-like, or
        // a non-primitive type and a type known to be primitive.
        if includes.intersects(TypeFlags::NEVER) {
            if type_set.contains(&self.silent_never_type) {
                return self.silent_never_type;
            }
            return self.never_type;
        }
        let dd = TypeFlags::DISJOINT_DOMAINS;
        if self.strict_null_checks
            && includes.intersects(TypeFlags::NULLABLE)
            && includes.intersects(
                TypeFlags::OBJECT | TypeFlags::NON_PRIMITIVE | TypeFlags::INCLUDES_EMPTY_OBJECT,
            )
            || includes.intersects(TypeFlags::NON_PRIMITIVE)
                && includes.intersects(dd.without(TypeFlags::NON_PRIMITIVE))
            || includes.intersects(TypeFlags::STRING_LIKE)
                && includes.intersects(dd.without(TypeFlags::STRING_LIKE))
            || includes.intersects(TypeFlags::NUMBER_LIKE)
                && includes.intersects(dd.without(TypeFlags::NUMBER_LIKE))
            || includes.intersects(TypeFlags::BIG_INT_LIKE)
                && includes.intersects(dd.without(TypeFlags::BIG_INT_LIKE))
            || includes.intersects(TypeFlags::ES_SYMBOL_LIKE)
                && includes.intersects(dd.without(TypeFlags::ES_SYMBOL_LIKE))
            || includes.intersects(TypeFlags::VOID_LIKE)
                && includes.intersects(dd.without(TypeFlags::VOID_LIKE))
        {
            return self.never_type;
        }
        if includes.intersects(TypeFlags::TEMPLATE_LITERAL | TypeFlags::STRING_MAPPING)
            && includes.intersects(TypeFlags::STRING_LITERAL)
        {
            let is_empty_set = self.extract_redundant_template_literals(&mut type_set);
            if is_empty_set {
                return self.never_type;
            }
        }
        if includes.intersects(TypeFlags::ANY) {
            if includes.intersects(TypeFlags::INCLUDES_WILDCARD) {
                return self.wildcard_type;
            }
            if includes.intersects(TypeFlags::INCLUDES_ERROR) {
                return self.error_type;
            }
            return self.any_type;
        }
        if !self.strict_null_checks && includes.intersects(TypeFlags::NULLABLE) {
            if includes.intersects(TypeFlags::INCLUDES_EMPTY_OBJECT) {
                return self.never_type;
            }
            if includes.intersects(TypeFlags::UNDEFINED) {
                return self.undefined_type;
            }
            return self.null_type;
        }
        if includes.intersects(TypeFlags::STRING)
            && includes.intersects(
                TypeFlags::STRING_LITERAL | TypeFlags::TEMPLATE_LITERAL | TypeFlags::STRING_MAPPING,
            )
            || includes.intersects(TypeFlags::NUMBER)
                && includes.intersects(TypeFlags::NUMBER_LITERAL)
            || includes.intersects(TypeFlags::BIG_INT)
                && includes.intersects(TypeFlags::BIG_INT_LITERAL)
            || includes.intersects(TypeFlags::ES_SYMBOL)
                && includes.intersects(TypeFlags::UNIQUE_ES_SYMBOL)
            || includes.intersects(TypeFlags::VOID) && includes.intersects(TypeFlags::UNDEFINED)
            || includes.intersects(TypeFlags::INCLUDES_EMPTY_OBJECT)
                && includes.intersects(TypeFlags::DEFINITELY_NON_NULLABLE)
        {
            if !flags.intersects(IntersectionFlags::NO_SUPERTYPE_REDUCTION) {
                self.remove_redundant_supertypes(&mut type_set, includes);
            }
        }
        if includes.intersects(TypeFlags::INCLUDES_MISSING_TYPE) {
            let undefined_type = self.undefined_type;
            // PORT: Go indexes with `slices.Index`, which panics on -1 the same way.
            let index = type_set
                .iter()
                .position(|&t| t == undefined_type)
                .expect("index out of range [-1]");
            type_set[index] = self.missing_type;
        }
        if type_set.is_empty() {
            return self.unknown_type;
        }
        if type_set.len() == 1 {
            return type_set[0];
        }
        if type_set.len() == 2 && !flags.intersects(IntersectionFlags::NO_CONSTRAINT_REDUCTION) {
            let mut type_var_index = 0;
            if !self
                .ty(type_set[0])
                .flags
                .intersects(TypeFlags::TYPE_VARIABLE)
            {
                type_var_index = 1;
            }
            let type_variable = type_set[type_var_index];
            let primitive_type = type_set[1 - type_var_index];
            if self
                .ty(type_variable)
                .flags
                .intersects(TypeFlags::TYPE_VARIABLE)
                && (self
                    .ty(primitive_type)
                    .flags
                    .intersects(TypeFlags::PRIMITIVE | TypeFlags::NON_PRIMITIVE)
                    && !self.is_generic_string_like_type(primitive_type)
                    || includes.intersects(TypeFlags::INCLUDES_EMPTY_OBJECT))
            {
                // We have an intersection T & P or P & T, where T is a type variable and P is a primitive type, the object type, or {}.
                let constraint = self.get_base_constraint_of_type(type_variable);
                // Check that T's constraint is similarly composed of primitive types, the object type, or {}.
                if constraint.is_some()
                    && self.every_type(constraint, &mut |c: &mut Checker, t: TypeId| {
                        c.is_primitive_or_object_or_empty_type(t)
                    })
                {
                    // If T's constraint is a subtype of P, simply return T. For example, given `T extends "a" | "b"`,
                    // the intersection `T & string` reduces to just T.
                    if self.is_type_strict_subtype_of(constraint, primitive_type) {
                        return type_variable;
                    }
                    if !(self.ty(constraint).flags.intersects(TypeFlags::UNION)
                        && self.some_type(constraint, &mut |c: &mut Checker, n: TypeId| {
                            c.is_type_strict_subtype_of(n, primitive_type)
                        }))
                    {
                        // No constituent of T's constraint is a subtype of P. If P is also not a subtype of T's constraint,
                        // then the constraint and P are unrelated, and the intersection reduces to never. For example, given
                        // `T extends "a" | "b"`, the intersection `T & number` reduces to never.
                        if !self.is_type_strict_subtype_of(primitive_type, constraint) {
                            return self.never_type;
                        }
                    }
                    // Some constituent of T's constraint is a subtype of P, or P is a subtype of T's constraint. Thus,
                    // the intersection further constrains the type variable. For example, given `T extends string | number`,
                    // the intersection `T & "a"` is marked as a constrained type variable. Likewise, given `T extends "a" | 1`,
                    // the intersection `T & number` is marked as a constrained type variable.
                    object_flags = ObjectFlags::IS_CONSTRAINED_TYPE_VARIABLE;
                }
            }
        }
        let key = self.get_intersection_key(&type_set, flags, alias.as_deref());
        let mut result = self
            .intersection_types
            .get(&key)
            .copied()
            .unwrap_or(TypeId::NIL);
        if result.is_nil() {
            if includes.intersects(TypeFlags::UNION) {
                let reduced = self.intersect_unions_of_primitive_types(&mut type_set);
                let every_union_with_undefined =
                    type_set.iter().all(|&t| self.is_union_with_undefined(t));
                if reduced {
                    // When the intersection creates a reduced set (which might mean that *all* union types have
                    // disappeared), we restart the operation to get a new set of combined flags. Once we have
                    // reduced we'll never reduce again, so this occurs at most once.
                    result = self.get_intersection_type_ex(&type_set, flags, alias);
                } else if every_union_with_undefined {
                    let mut contained_undefined_type = self.undefined_type;
                    let contains_missing_type = self.contains_missing_type.clone();
                    let mut some_contains_missing = false;
                    for &t in &type_set {
                        if contains_missing_type(self, t) {
                            some_contains_missing = true;
                            break;
                        }
                    }
                    if some_contains_missing {
                        contained_undefined_type = self.missing_type;
                    }
                    self.filter_types(&mut type_set, &mut |c: &mut Checker, t: TypeId| {
                        c.is_not_undefined_type(t)
                    });
                    let inner =
                        self.get_intersection_type_ex(&type_set, flags, None /*alias*/);
                    result = self.get_union_type_ex(
                        &[inner, contained_undefined_type],
                        UnionReduction::LITERAL,
                        alias,
                        TypeId::NIL, /*origin*/
                    );
                } else if type_set.iter().all(|&t| self.is_union_with_null(t)) {
                    self.filter_types(&mut type_set, &mut |c: &mut Checker, t: TypeId| {
                        c.is_not_null_type(t)
                    });
                    let inner =
                        self.get_intersection_type_ex(&type_set, flags, None /*alias*/);
                    let null_type = self.null_type;
                    result = self.get_union_type_ex(
                        &[inner, null_type],
                        UnionReduction::LITERAL,
                        alias,
                        TypeId::NIL, /*origin*/
                    );
                } else if type_set.len() >= 3 && types.len() > 2 {
                    // When we have three or more constituents, more than two inputs (to head off infinite reexpansion), some of which are unions, we employ a "divide and conquer" strategy
                    // where A & B & C & D is processed as (A & B) & (C & D). Since intersections of unions often produce far smaller
                    // unions of intersections than the full cartesian product (due to some intersections becoming `never`), this can
                    // dramatically reduce the overall work.
                    let middle = type_set.len() / 2;
                    let left = self.get_intersection_type_ex(
                        &type_set[..middle],
                        flags,
                        None, /*alias*/
                    );
                    let right = self.get_intersection_type_ex(
                        &type_set[middle..],
                        flags,
                        None, /*alias*/
                    );
                    result = self.get_intersection_type_ex(&[left, right], flags, alias);
                } else {
                    // We are attempting to construct a type of the form X & (A | B) & (C | D). Transform this into a type of
                    // the form X & A & C | X & A & D | X & B & C | X & B & D. If the estimated size of the resulting union type
                    // exceeds 100000 constituents, report an error.
                    if !self.check_cross_product_union(&type_set) {
                        return self.error_type;
                    }
                    let constituents = self.get_cross_product_intersections(&type_set, flags);
                    // We attach a denormalized origin type when at least one constituent of the cross-product union is an
                    // intersection (i.e. when the intersection didn't just reduce one or more unions to smaller unions) and
                    // the denormalized origin has fewer constituents than the union itself.
                    let mut origin = TypeId::NIL;
                    if constituents.iter().any(|&t| self.is_intersection_type(t))
                        && self.get_constituent_count_of_types(&constituents)
                            > self.get_constituent_count_of_types(&type_set)
                    {
                        origin = self.new_intersection_type(ObjectFlags::NONE, &type_set);
                    }
                    result = self.get_union_type_ex(
                        &constituents,
                        UnionReduction::LITERAL,
                        alias,
                        origin,
                    );
                }
            } else {
                let propagating = self.get_propagating_flags_of_types(
                    types,
                    TypeFlags::NULLABLE, /*excludeKinds*/
                );
                result = self.new_intersection_type(object_flags | propagating, &type_set);
                self.ty_mut(result).alias = alias;
            }
            self.intersection_types.insert(key, result);
        }
        result
    }

    // Go: checker/checker.go:26690 isUnionWithUndefined
    pub fn is_union_with_undefined(&self, t: TypeId) -> bool {
        self.ty(t).flags.intersects(TypeFlags::UNION)
            && self
                .ty(self.ty(t).types()[0])
                .flags
                .intersects(TypeFlags::UNDEFINED)
    }

    // Go: checker/checker.go:26694 isUnionWithNull
    pub fn is_union_with_null(&self, t: TypeId) -> bool {
        self.ty(t).flags.intersects(TypeFlags::UNION)
            && (self
                .ty(self.ty(t).types()[0])
                .flags
                .intersects(TypeFlags::NULL)
                || self
                    .ty(self.ty(t).types()[1])
                    .flags
                    .intersects(TypeFlags::NULL))
    }

    // Go: checker/checker.go:26698 isIntersectionType
    pub fn is_intersection_type(&self, t: TypeId) -> bool {
        self.ty(t).flags.intersects(TypeFlags::INTERSECTION)
    }

    // Go: checker/checker.go:26702 isPrimitiveUnion
    pub fn is_primitive_union(&self, t: TypeId) -> bool {
        self.ty(t)
            .object_flags
            .intersects(ObjectFlags::PRIMITIVE_UNION)
    }

    // Go: checker/checker.go:26706 isNotUndefinedType
    pub fn is_not_undefined_type(&self, t: TypeId) -> bool {
        !self.ty(t).flags.intersects(TypeFlags::UNDEFINED)
    }

    // Go: checker/checker.go:26710 isNotNullType
    pub fn is_not_null_type(&self, t: TypeId) -> bool {
        !self.ty(t).flags.intersects(TypeFlags::NULL)
    }

    // Add the given types to the given type set. Order is preserved, freshness is removed from literal
    // types, duplicates are removed, and nested types of the given kind are flattened into the set.
    // Go: checker/checker.go:26716 addTypesToIntersection
    pub fn add_types_to_intersection(
        &mut self,
        type_set: &mut TypeSet,
        includes: TypeFlags,
        types: &[TypeId],
    ) -> TypeFlags {
        let mut includes = includes;
        for &t in types {
            let regular = self.get_regular_type_of_literal_type(t);
            includes = self.add_type_to_intersection(type_set, includes, regular);
        }
        includes
    }

    // Go: checker/checker.go:26723 addTypeToIntersection
    pub fn add_type_to_intersection(
        &mut self,
        type_set: &mut TypeSet,
        includes: TypeFlags,
        t: TypeId,
    ) -> TypeFlags {
        let mut includes = includes;
        let mut t = t;
        let flags = self.ty(t).flags;
        if flags.intersects(TypeFlags::INTERSECTION) {
            // Go `addTypesToIntersection(typeSet, includes, t.Types())`.
            for i in 0..self.ty(t).types().len() {
                let regular = self.get_regular_type_of_literal_type(self.type_at(t, i));
                includes = self.add_type_to_intersection(type_set, includes, regular);
            }
            return includes;
        }
        if self.is_empty_anonymous_object_type(t) {
            if !includes.intersects(TypeFlags::INCLUDES_EMPTY_OBJECT) {
                includes |= TypeFlags::INCLUDES_EMPTY_OBJECT;
                // Go `orderedSet.Add` adds only values it has not seen.
                if !type_set.contains(&t) {
                    type_set.push(t);
                }
            }
        } else {
            if flags.intersects(TypeFlags::ANY_OR_UNKNOWN) {
                if t == self.wildcard_type {
                    includes |= TypeFlags::INCLUDES_WILDCARD;
                }
                if self.is_error_type(t) {
                    includes |= TypeFlags::INCLUDES_ERROR;
                }
            } else if self.strict_null_checks || !flags.intersects(TypeFlags::NULLABLE) {
                if t == self.missing_type {
                    includes |= TypeFlags::INCLUDES_MISSING_TYPE;
                    t = self.undefined_type;
                }
                if !type_set.contains(&t) {
                    if self.ty(t).flags.intersects(TypeFlags::UNIT)
                        && includes.intersects(TypeFlags::UNIT)
                    {
                        // We have seen two distinct unit types which means we should reduce to an
                        // empty intersection. Adding TypeFlags.NonPrimitive causes that to happen.
                        includes |= TypeFlags::NON_PRIMITIVE;
                    }
                    type_set.push(t);
                }
            }
            includes |= flags & TypeFlags::INCLUDES_MASK;
        }
        includes
    }

    // Go: checker/checker.go:26760 removeRedundantSupertypes
    // PORT: Go returns the filtered slice; Rust filters `types` in place.
    pub fn remove_redundant_supertypes(&mut self, types: &mut TypeSet, includes: TypeFlags) {
        let mut i = types.len();
        while i > 0 {
            i -= 1;
            let t = types[i];
            let tf = self.ty(t).flags;
            let remove = tf.intersects(TypeFlags::STRING)
                && includes.intersects(
                    TypeFlags::STRING_LITERAL
                        | TypeFlags::TEMPLATE_LITERAL
                        | TypeFlags::STRING_MAPPING,
                )
                || tf.intersects(TypeFlags::NUMBER)
                    && includes.intersects(TypeFlags::NUMBER_LITERAL)
                || tf.intersects(TypeFlags::BIG_INT)
                    && includes.intersects(TypeFlags::BIG_INT_LITERAL)
                || tf.intersects(TypeFlags::ES_SYMBOL)
                    && includes.intersects(TypeFlags::UNIQUE_ES_SYMBOL)
                || tf.intersects(TypeFlags::VOID) && includes.intersects(TypeFlags::UNDEFINED)
                || self.is_empty_anonymous_object_type(t)
                    && includes.intersects(TypeFlags::DEFINITELY_NON_NULLABLE);
            if remove {
                types.remove(i);
            }
        }
    }

    /**
     * Returns true if the intersection of the template literals and string literals is the empty set,
     * for example `get${string}` & "setX", and should reduce to never.
     */
    // Go: checker/checker.go:26782 extractRedundantTemplateLiterals
    // PORT: Go returns the filtered slice and the flag; Rust filters `types`
    // in place and returns the flag.
    pub fn extract_redundant_template_literals(&mut self, types: &mut TypeSet) -> bool {
        let literals: Vec<TypeId> = types
            .iter()
            .copied()
            .filter(|&t| self.ty(t).flags.intersects(TypeFlags::STRING_LITERAL))
            .collect();
        let mut i = types.len();
        while i > 0 {
            i -= 1;
            let t = types[i];
            if !self
                .ty(t)
                .flags
                .intersects(TypeFlags::TEMPLATE_LITERAL | TypeFlags::STRING_MAPPING)
            {
                continue;
            }
            for &t2 in &literals {
                if self.is_type_subtype_of(t2, t) {
                    // For example, `get${T}` & "getX" is just "getX", and Lowercase<string> & "foo" is just "foo"
                    types.remove(i);
                    break;
                }
                if self.is_pattern_literal_type(t) {
                    return true;
                }
            }
        }
        false
    }

    // If the given list of types contains more than one union of primitive types, replace the
    // first with a union containing an intersection of those primitive types, then remove the
    // other unions and return true. Otherwise, do nothing and return false.
    // Go: checker/checker.go:26808 intersectUnionsOfPrimitiveTypes
    // PORT: Go returns the new slice and the flag; Rust changes `types` in
    // place and returns the flag.
    pub fn intersect_unions_of_primitive_types(&mut self, types: &mut TypeSet) -> bool {
        let index = match types.iter().position(|&t| self.is_primitive_union(t)) {
            Some(index) => index,
            None => return false,
        };
        // Remove all but the first union of primitive types and collect them in
        // the unionTypes array.
        let mut i = index + 1;
        let mut union_types: Vec<TypeId> = vec![types[index]];
        while i < types.len() {
            let t = types[i];
            if self
                .ty(t)
                .object_flags
                .intersects(ObjectFlags::PRIMITIVE_UNION)
            {
                union_types.push(t);
                types.remove(i);
            } else {
                i += 1;
            }
        }
        // Return false if there was only one union of primitive types
        if union_types.len() == 1 {
            return false;
        }
        // We have more than one union of primitive types, now intersect them. For each
        // type in each union we check if the type is matched in every union and if so
        // we include it in the result.
        let mut checked: Vec<TypeId> = Vec::new();
        let mut result: Vec<TypeId> = Vec::new();
        for &u in &union_types {
            for i in 0..self.ty(u).types().len() {
                let t = self.type_at(u, i);
                let (new_checked, inserted) = self.insert_type(&checked, t);
                checked = new_checked;
                if inserted {
                    if self.each_union_contains(&union_types, t) {
                        // undefinedType/missingType are always sorted first so we leverage that here
                        if t == self.undefined_type
                            && !result.is_empty()
                            && result[0] == self.missing_type
                        {
                            continue;
                        }
                        if t == self.missing_type
                            && !result.is_empty()
                            && result[0] == self.undefined_type
                        {
                            result[0] = self.missing_type;
                            continue;
                        }
                        result = self.insert_type(&result, t).0;
                    }
                }
            }
        }
        // Finally replace the first union with the result
        types[index] = self.get_union_type_from_sorted_list(
            &result,
            ObjectFlags::PRIMITIVE_UNION,
            None,        /*alias*/
            TypeId::NIL, /*origin*/
        );
        true
    }

    // Check that the given type has a match in every union. A given type is matched by
    // an identical type, and a literal type is additionally matched by its corresponding
    // primitive type, and missingType is matched by undefinedType (and vice versa).
    // Go: checker/checker.go:26861 eachUnionContains
    pub fn each_union_contains(&self, union_types: &[TypeId], t: TypeId) -> bool {
        for &u in union_types {
            if !self.union_contains_type(u, t, true /*matchSymbol*/) {
                return false;
            }
        }
        true
    }

    // Go: checker/checker.go:26870 unionContainsType
    pub fn union_contains_type(&self, union: TypeId, t: TypeId, match_symbol: bool) -> bool {
        let types = self.ty(union).types();
        if self.contains_type(types, t) {
            return true;
        }
        if t == self.missing_type {
            return self.contains_type(types, self.undefined_type);
        }
        if t == self.undefined_type {
            return self.contains_type(types, self.missing_type);
        }
        let mut primitive = TypeId::NIL;
        let tf = self.ty(t).flags;
        if tf.intersects(TypeFlags::STRING_LITERAL) {
            primitive = self.string_type;
        } else if tf.intersects(TypeFlags::ENUM | TypeFlags::NUMBER_LITERAL) {
            primitive = self.number_type;
        } else if tf.intersects(TypeFlags::BIG_INT_LITERAL) {
            primitive = self.bigint_type;
        } else if tf.intersects(TypeFlags::UNIQUE_ES_SYMBOL) && match_symbol {
            primitive = self.es_symbol_type;
        }
        primitive.is_some() && self.contains_type(types, primitive)
    }

    // Go: checker/checker.go:26895 getCrossProductIntersections
    pub fn get_cross_product_intersections(
        &mut self,
        types: &[TypeId],
        flags: IntersectionFlags,
    ) -> Vec<TypeId> {
        let count = self.get_cross_product_union_size(types);
        let mut intersections: Vec<TypeId> = Vec::new();
        for i in 0..count {
            let mut constituents = types.to_vec();
            let mut n = i;
            let mut j = types.len() as i32 - 1;
            while j >= 0 {
                let ju = j as usize;
                if self.ty(types[ju]).flags.intersects(TypeFlags::UNION) {
                    let source_types = self.ty(types[ju]).types();
                    let length = source_types.len() as i32;
                    constituents[ju] = source_types[(n % length) as usize];
                    n /= length;
                }
                j -= 1;
            }
            let t = self.get_intersection_type_ex(&constituents, flags, None /*alias*/);
            if !self.ty(t).flags.intersects(TypeFlags::NEVER) {
                intersections.push(t);
            }
        }
        intersections
    }

    // Go: checker/checker.go:26917 getConstituentCount
    pub fn get_constituent_count(&self, t: TypeId) -> i32 {
        let ty = self.ty(t);
        if !ty.flags.intersects(TypeFlags::UNION_OR_INTERSECTION) || ty.alias.is_some() {
            return 1;
        }
        if ty.flags.intersects(TypeFlags::UNION) && ty.as_union_type().origin.is_some() {
            return self.get_constituent_count(ty.as_union_type().origin);
        }
        self.get_constituent_count_of_types(ty.types())
    }

    // Go: checker/checker.go:26927 getConstituentCountOfTypes
    pub fn get_constituent_count_of_types(&self, types: &[TypeId]) -> i32 {
        let mut n = 0;
        for &t in types {
            n += self.get_constituent_count(t);
        }
        n
    }

    // Go: checker/checker.go:26935 filterTypes
    // PORT: Go writes into the caller's slice, so the parameter is `&mut [TypeId]`.
    pub fn filter_types(
        &mut self,
        types: &mut [TypeId],
        predicate: &mut dyn FnMut(&mut Checker, TypeId) -> bool,
    ) {
        for i in 0..types.len() {
            types[i] = self.filter_type(types[i], &mut *predicate);
        }
    }

    // Go: checker/checker.go:26941 IsEmptyAnonymousObjectType
    pub fn is_empty_anonymous_object_type(&mut self, t: TypeId) -> bool {
        let object_flags = self.ty(t).object_flags;
        let symbol = self.ty(t).symbol;
        object_flags.intersects(ObjectFlags::ANONYMOUS)
            && (object_flags.intersects(ObjectFlags::MEMBERS_RESOLVED)
                && self.is_empty_resolved_type(t)
                || symbol.is_some()
                    && self.sym(symbol).flags.intersects(SymbolFlags::TYPE_LITERAL)
                    && {
                        let members = self.get_members_of_symbol(symbol);
                        self.symbols.len(members) == 0
                    })
    }

    // Go: checker/checker.go:26946 isEmptyResolvedType
    // PORT: Go takes the `*StructuredType` of a type. Every Go caller passes
    // `t.AsStructuredType()` or `resolveStructuredTypeMembers(t)` (which is
    // `t.AsStructuredType()` after resolving), so this takes the type `t` and
    // reads its `StructuredType`. Callers resolve members first as Go does.
    pub fn is_empty_resolved_type(&self, t: TypeId) -> bool {
        let s = self.ty(t).as_structured_type();
        t != self.any_function_type
            && s.properties.is_empty()
            && s.signatures().is_empty()
            && s.index_infos().is_empty()
    }

    // Go: checker/checker.go:26950 isEmptyObjectType
    pub fn is_empty_object_type(&mut self, t: TypeId) -> bool {
        let flags = self.ty(t).flags;
        if flags.intersects(TypeFlags::OBJECT) {
            if self.is_generic_mapped_type(t) {
                return false;
            }
            self.resolve_structured_type_members(t);
            return self.is_empty_resolved_type(t);
        }
        if flags.intersects(TypeFlags::NON_PRIMITIVE) {
            return true;
        }
        if flags.intersects(TypeFlags::UNION) {
            let types = self.ty(t).types_list();
            for u in types {
                if self.is_empty_object_type(u) {
                    return true;
                }
            }
            return false;
        }
        if flags.intersects(TypeFlags::INTERSECTION) {
            for i in 0..self.ty(t).types().len() {
                let u = self.type_at(t, i);
                if !self.is_empty_object_type(u) {
                    return false;
                }
            }
            return true;
        }
        false
    }

    // Go: checker/checker.go:26964 isPatternLiteralPlaceholderType
    pub fn is_pattern_literal_placeholder_type(&self, t: TypeId) -> bool {
        if self.ty(t).flags.intersects(TypeFlags::INTERSECTION) {
            // Return true if the intersection consists of one or more placeholders and zero or
            // more object type tags.
            let mut seen_placeholder = false;
            for &s in self.ty(t).types() {
                if self
                    .ty(s)
                    .flags
                    .intersects(TypeFlags::LITERAL | TypeFlags::NULLABLE)
                    || self.is_pattern_literal_placeholder_type(s)
                {
                    seen_placeholder = true;
                } else if !self.ty(s).flags.intersects(TypeFlags::OBJECT) {
                    return false;
                }
            }
            return seen_placeholder;
        }
        self.ty(t)
            .flags
            .intersects(TypeFlags::ANY | TypeFlags::STRING | TypeFlags::NUMBER | TypeFlags::BIG_INT)
            || self.is_pattern_literal_type(t)
    }

    // Go: checker/checker.go:26981 isPatternLiteralType
    pub fn is_pattern_literal_type(&self, t: TypeId) -> bool {
        // A pattern literal type is a template literal or a string mapping type that contains only
        // non-generic pattern literal placeholders.
        let flags = self.ty(t).flags;
        flags.intersects(TypeFlags::TEMPLATE_LITERAL)
            && self
                .ty(t)
                .as_template_literal_type()
                .types
                .iter()
                .all(|&u| self.is_pattern_literal_placeholder_type(u))
            || flags.intersects(TypeFlags::STRING_MAPPING)
                && self.is_pattern_literal_placeholder_type(self.ty(t).target())
    }

    // Go: checker/checker.go:26988 isGenericStringLikeType
    pub fn is_generic_string_like_type(&self, t: TypeId) -> bool {
        self.ty(t)
            .flags
            .intersects(TypeFlags::TEMPLATE_LITERAL | TypeFlags::STRING_MAPPING)
            && !self.is_pattern_literal_type(t)
    }

    // Go: checker/checker.go:26992 forEachType
    pub fn for_each_type(&mut self, t: TypeId, f: &mut dyn FnMut(&mut Checker, TypeId)) {
        if self.ty(t).flags.intersects(TypeFlags::UNION) {
            for i in 0..self.ty(t).types().len() {
                let u = self.type_at(t, i);
                f(self, u);
            }
        } else {
            f(self, t);
        }
    }

    // Go: checker/checker.go:27002 someType
    pub fn some_type(
        &mut self,
        t: TypeId,
        f: &mut dyn FnMut(&mut Checker, TypeId) -> bool,
    ) -> bool {
        if self.ty(t).flags.intersects(TypeFlags::UNION) {
            for i in 0..self.ty(t).types().len() {
                let u = self.type_at(t, i);
                if f(self, u) {
                    return true;
                }
            }
            return false;
        }
        f(self, t)
    }

    // Go: checker/checker.go:27009 everyType
    pub fn every_type(
        &mut self,
        t: TypeId,
        f: &mut dyn FnMut(&mut Checker, TypeId) -> bool,
    ) -> bool {
        if self.ty(t).flags.intersects(TypeFlags::UNION) {
            for i in 0..self.ty(t).types().len() {
                let u = self.type_at(t, i);
                if !f(self, u) {
                    return false;
                }
            }
            return true;
        }
        f(self, t)
    }

    // Go: checker/checker.go:27016 everyContainedType
    pub fn every_contained_type(
        &mut self,
        t: TypeId,
        f: &mut dyn FnMut(&mut Checker, TypeId) -> bool,
    ) -> bool {
        if self
            .ty(t)
            .flags
            .intersects(TypeFlags::UNION_OR_INTERSECTION)
        {
            for i in 0..self.ty(t).types().len() {
                let u = self.type_at(t, i);
                if !f(self, u) {
                    return false;
                }
            }
            return true;
        }
        f(self, t)
    }

    // Go: checker/checker.go:27023 filterType
    pub fn filter_type(
        &mut self,
        t: TypeId,
        f: &mut dyn FnMut(&mut Checker, TypeId) -> bool,
    ) -> TypeId {
        if self.ty(t).flags.intersects(TypeFlags::UNION) {
            // PORT: Go `core.Same(types, core.Filter(types, f))` is true exactly
            // when nothing was filtered out. The filtered list is built only
            // once the first type is dropped, so a type that keeps every
            // constituent costs no copy.
            let types_len = self.ty(t).types().len();
            let mut filtered: Option<Vec<TypeId>> = None;
            for i in 0..types_len {
                let u = self.type_at(t, i);
                let keep = f(self, u);
                if let Some(list) = &mut filtered {
                    if keep {
                        list.push(u);
                    }
                } else if !keep {
                    let mut list = Vec::with_capacity(types_len - 1);
                    list.extend_from_slice(&self.ty(t).types()[..i]);
                    filtered = Some(list);
                }
            }
            let Some(filtered) = filtered else {
                return t;
            };
            let origin = self.ty(t).as_union_type().origin;
            let mut new_origin = TypeId::NIL;
            if origin.is_some() && self.ty(origin).flags.intersects(TypeFlags::UNION) {
                // If the origin type is a (denormalized) union type, filter its non-union constituents. If that ends
                // up removing a smaller number of types than in the normalized constituent set (meaning some of the
                // filtered types are within nested unions in the origin), then we can't construct a new origin type.
                // Otherwise, if we have exactly one type left in the origin set, return that as the filtered type.
                // Otherwise, construct a new filtered origin type.
                let origin_types = self.ty(origin).types_list();
                let mut origin_filtered: Vec<TypeId> = Vec::with_capacity(origin_types.len());
                for &u in &origin_types {
                    if self.ty(u).flags.intersects(TypeFlags::UNION) || f(self, u) {
                        origin_filtered.push(u);
                    }
                }
                if origin_types.len() - origin_filtered.len() == types_len - filtered.len() {
                    if origin_filtered.len() == 1 {
                        return origin_filtered[0];
                    }
                    new_origin = self.new_union_type(ObjectFlags::NONE, &origin_filtered);
                }
            }
            // filtering could remove intersections so `ContainsIntersections` might be forwarded "incorrectly"
            // it is purely an optimization hint so there is no harm in accidentally forwarding it
            let object_flags = self.ty(t).object_flags
                & (ObjectFlags::PRIMITIVE_UNION | ObjectFlags::CONTAINS_INTERSECTIONS);
            return self.get_union_type_from_sorted_list(
                &filtered,
                object_flags,
                None, /*alias*/
                new_origin,
            );
        }
        if self.ty(t).flags.intersects(TypeFlags::NEVER) || f(self, t) {
            return t;
        }
        self.never_type
    }

    // Go: checker/checker.go:27059 removeType
    pub fn remove_type(&mut self, t: TypeId, target_type: TypeId) -> TypeId {
        if !self.ty(t).flags.intersects(TypeFlags::UNION) {
            if t == target_type {
                return self.never_type;
            }
            return t;
        }
        let origin = self.ty(t).as_union_type().origin;
        if origin.is_some()
            && self.ty(origin).flags.intersects(TypeFlags::UNION)
            && self.contains_type(self.ty(origin).types(), target_type)
        {
            return self.filter_type(t, &mut |_c: &mut Checker, t: TypeId| t != target_type);
        }
        let types = self.ty(t).types();
        if let (i, true) = self.search_union_types(types, target_type) {
            if types.len() == 2 {
                return types[1 - i];
            }
            // Remove the target type from the slice.
            let mut filtered = Vec::with_capacity(types.len() - 1);
            filtered.extend_from_slice(&types[..i]);
            filtered.extend_from_slice(&types[i + 1..]);
            let object_flags = self.ty(t).object_flags
                & (ObjectFlags::PRIMITIVE_UNION | ObjectFlags::CONTAINS_INTERSECTIONS);
            return self.get_union_type_from_sorted_list(
                &filtered,
                object_flags,
                None,        /*alias*/
                TypeId::NIL, /*origin*/
            );
        }
        t
    }

    // Go: checker/checker.go:27081 containsType
    pub fn contains_type(&self, types: &[TypeId], t: TypeId) -> bool {
        self.search_union_types(types, t).1
    }

    // Go: checker/checker.go:27086 insertType
    // PORT: Go returns the (possibly grown) slice; Rust returns a new Vec.
    pub fn insert_type(&self, types: &[TypeId], t: TypeId) -> (Vec<TypeId>, bool) {
        match self.search_union_types(types, t) {
            (i, false) => {
                let mut result = types.to_vec();
                result.insert(i, t);
                (result, true)
            }
            (_, true) => (types.to_vec(), false),
        }
    }

    // Go: checker/checker.go:27093 countTypes
    pub fn count_types(&self, t: TypeId) -> i32 {
        let ty = self.ty(t);
        if ty.flags.intersects(TypeFlags::UNION) {
            return ty.types().len() as i32;
        }
        if ty.flags.intersects(TypeFlags::NEVER) {
            return 0;
        }
        1
    }

    // Go: checker/checker.go:27103 isErrorType
    pub fn is_error_type(&self, t: TypeId) -> bool {
        // The only 'any' types that have alias symbols are those manufactured by getTypeFromTypeAliasReference for
        // a reference to an unresolved symbol. We want those to behave like the errorType.
        t == self.error_type
            || self.ty(t).flags.intersects(TypeFlags::ANY) && self.ty(t).alias.is_some()
    }
}

// Go: checker/checker.go:27109 compareTypeIds
pub fn compare_type_ids(t1: TypeId, t2: TypeId) -> i32 {
    t1.index() as i32 - t2.index() as i32
}

impl Checker {
    // Go: checker/checker.go:27113 checkCrossProductUnion
    pub fn check_cross_product_union(&mut self, types: &[TypeId]) -> bool {
        let size = self.get_cross_product_union_size(types);
        if size >= 100_000 {
            if let Some(tr) = self.tracer {
                tr.instant(
                    crate::tracing::Phase::CheckTypes,
                    "checkCrossProductUnion_DepthLimit",
                    vec![("size", size.into())],
                );
            }
            let current_node = self.current_node;
            self.error(
                current_node,
                diag::Expression_produces_a_union_type_that_is_too_complex_to_represent,
                args![],
            );
            return false;
        }
        true
    }

    // Go: checker/checker.go:27125 getCrossProductUnionSize
    // PORT: Go `int` is `i32` (PORTING.md), so the overflow cap is `i32::MAX`
    // in place of `math.MaxInt`. Both are far above the 100_000 limit.
    pub fn get_cross_product_union_size(&self, types: &[TypeId]) -> i32 {
        let mut size: i32 = 1;
        for &t in types {
            let ty = self.ty(t);
            if ty.flags.intersects(TypeFlags::UNION) {
                let n = ty.types().len() as i32;
                // Cap the result to avoid integer overflow when computing the cross product of many large unions.
                // In TypeScript, number overflow produces Infinity which naturally exceeds the limit check;
                // in Go, we must guard against int wrapping to zero or negative.
                if n > 0 && size > i32::MAX / n {
                    return i32::MAX;
                }
                size *= n;
            } else if ty.flags.intersects(TypeFlags::NEVER) {
                return 0;
            }
        }
        size
    }

    // Go: checker/checker.go:27145 getIndexType
    pub fn get_index_type(&mut self, t: TypeId) -> TypeId {
        self.get_index_type_ex(t, IndexFlags::NONE)
    }

    // Go: checker/checker.go:27149 getIndexTypeEx
    pub fn get_index_type_ex(&mut self, t: TypeId, index_flags: IndexFlags) -> TypeId {
        let t = self.get_reduced_type(t);
        if self.is_no_infer_type(t) {
            let base_type = self.ty(t).as_substitution_type().base_type;
            let index_type = self.get_index_type_ex(base_type, index_flags);
            return self.get_no_infer_type(index_type);
        }
        if self.should_defer_index_type(t, index_flags) {
            return self.get_index_type_for_generic_type(t, index_flags);
        }
        if self.ty(t).flags.intersects(TypeFlags::UNION) {
            let types = self.ty(t).types_list();
            let mapped: Vec<TypeId> = types
                .into_iter()
                .map(|u| self.get_index_type_ex(u, index_flags))
                .collect();
            return self.get_intersection_type(&mapped);
        }
        if self.ty(t).flags.intersects(TypeFlags::INTERSECTION) {
            let types = self.ty(t).types_list();
            let mapped: Vec<TypeId> = types
                .into_iter()
                .map(|u| self.get_index_type_ex(u, index_flags))
                .collect();
            return self.get_union_type(&mapped);
        }
        if self.ty(t).object_flags.intersects(ObjectFlags::MAPPED) {
            return self.get_index_type_for_mapped_type(t, index_flags);
        }
        if t == self.wildcard_type {
            return self.wildcard_type;
        }
        if self.ty(t).flags.intersects(TypeFlags::UNKNOWN) {
            return self.never_type;
        }
        if self
            .ty(t)
            .flags
            .intersects(TypeFlags::ANY | TypeFlags::NEVER)
        {
            return self.string_number_symbol_type;
        }
        let include = (if index_flags.intersects(IndexFlags::NO_INDEX_SIGNATURES) {
            TypeFlags::STRING_LITERAL
        } else {
            TypeFlags::STRING_LIKE
        }) | (if index_flags.intersects(IndexFlags::STRINGS_ONLY) {
            TypeFlags::NONE
        } else {
            TypeFlags::NUMBER_LIKE | TypeFlags::ES_SYMBOL_LIKE
        });
        self.get_literal_type_from_properties(t, include, index_flags == IndexFlags::NONE)
    }

    // Go: checker/checker.go:27174 getExtractStringType
    pub fn get_extract_string_type(&mut self, t: TypeId) -> TypeId {
        let get_global_extract_symbol = self.get_global_extract_symbol.clone();
        let extract_type_alias = get_global_extract_symbol(self);
        if extract_type_alias.is_some() {
            let string_type = self.string_type;
            return self.get_type_alias_instantiation(extract_type_alias, &[t, string_type], None);
        }
        self.string_type
    }

    // Go: checker/checker.go:27182 getLiteralTypeFromProperties
    pub fn get_literal_type_from_properties(
        &mut self,
        t: TypeId,
        include: TypeFlags,
        include_origin: bool,
    ) -> TypeId {
        let key = PropertiesTypesKey {
            type_id: t,
            include,
            include_origin,
        };
        if let Some(&cached) = self.properties_types.get(&key) {
            return cached;
        }
        let mut origin = TypeId::NIL;
        if include_origin
            && self
                .ty(t)
                .object_flags
                .intersects(ObjectFlags::CLASS_OR_INTERFACE | ObjectFlags::REFERENCE)
            || self.ty(t).alias.is_some()
        {
            origin = self.new_index_type(t, IndexFlags::NONE);
        }
        let props = self.get_properties_of_type(t);
        let index_infos = self.get_index_infos_of_type(t);
        let mut types: Vec<TypeId> = Vec::with_capacity(props.len() + index_infos.len());
        for prop in props {
            let prop_type = self.get_literal_type_from_property(prop, include, false);
            types.push(prop_type);
        }
        for info in index_infos {
            let key_type = self.index_info(info).key_type;
            if info != self.enum_number_index_info && self.is_key_type_included(key_type, include) {
                if key_type == self.string_type && include.intersects(TypeFlags::NUMBER) {
                    types.push(self.string_or_number_type);
                } else {
                    types.push(key_type);
                }
            }
        }
        let result = self.get_union_type_ex(&types, UnionReduction::LITERAL, None, origin);
        self.properties_types.insert(key, result);
        result
    }

    // Go: checker/checker.go:27211 getLiteralTypeFromProperty
    pub fn get_literal_type_from_property(
        &mut self,
        prop: SymbolId,
        include: TypeFlags,
        include_non_public: bool,
    ) -> TypeId {
        if include_non_public
            || !self
                .get_declaration_modifier_flags_from_symbol(prop)
                .intersects(ModifierFlags::NON_PUBLIC_ACCESSIBILITY_MODIFIER)
        {
            let late_bound = self.get_late_bound_symbol(prop);
            let mut t = self
                .value_symbol_links
                .get_by_id(&self.symbols, late_bound)
                .name_type;
            if t.is_nil() {
                if self.sym(prop).name == INTERNAL_SYMBOL_NAME_DEFAULT {
                    t = self.get_string_literal_type("default");
                } else {
                    let name = get_name_of_declaration(self.sym(prop).value_declaration);
                    if name.is_some() {
                        t = self.get_literal_type_from_property_name(name);
                    }
                    if t.is_nil() && !self.is_known_symbol(prop) {
                        let prop_name = symbol_name(&self.symbols, prop);
                        t = self.get_string_literal_type(&prop_name);
                    }
                }
            }
            if t.is_some() && self.ty(t).flags.intersects(include) {
                return t;
            }
        }
        self.never_type
    }

    // Go: checker/checker.go:27234 getLiteralTypeFromPropertyName
    pub fn get_literal_type_from_property_name(&mut self, name: Node) -> TypeId {
        if is_private_identifier(name) {
            return self.never_type;
        }
        if is_numeric_literal(name) {
            let t = self.check_expression(name);
            return self.get_regular_type_of_literal_type(t);
        }
        if is_computed_property_name(name) {
            let t = self.check_computed_property_name(name);
            return self.get_regular_type_of_literal_type(t);
        }
        // Borrowed text: get_string_literal_type makes the String key only
        // when the literal type is new.
        let property_name = property_name_text(name);
        if property_name != INTERNAL_SYMBOL_NAME_MISSING {
            return self.get_string_literal_type(&property_name);
        }
        if is_expression(name) {
            let t = self.check_expression(name);
            return self.get_regular_type_of_literal_type(t);
        }
        self.never_type
    }

    // Go: checker/checker.go:27254 isKeyTypeIncluded
    pub fn is_key_type_included(&self, key_type: TypeId, include: TypeFlags) -> bool {
        let ty = self.ty(key_type);
        ty.flags.intersects(include)
            || ty.flags.intersects(TypeFlags::INTERSECTION)
                && ty
                    .types()
                    .iter()
                    .any(|&t| self.is_key_type_included(t, include))
    }
}

// Go: checker/checker.go:27261 isInvalidComputedPropertyName
pub fn is_invalid_computed_property_name(node: Node) -> bool {
    let grandparent = node.parent().parent();
    (is_type_literal_node(grandparent)
        || is_class_like(grandparent)
        || is_interface_declaration(grandparent))
        && is_binary_expression(node.expression())
        && node.expression().operator_token().kind() == SyntaxKind::InKeyword
        && !is_accessor(node.parent())
}

impl Checker {
    // Go: checker/checker.go:27267 checkComputedPropertyName
    // PORT: Go holds a pointer to the links; here each access reads
    // `self.type_node_links.get(node)` again.
    pub fn check_computed_property_name(&mut self, node: Node) -> TypeId {
        let expression = node.expression();
        if self.type_node_links.get(node).resolved_type.is_nil() {
            let circular_constraint_type = self.circular_constraint_type;
            self.type_node_links.get(node).resolved_type = circular_constraint_type;
            if is_invalid_computed_property_name(node) {
                let error_type = self.error_type;
                self.type_node_links.get(node).resolved_type = error_type;
                return error_type;
            }
            let resolved_type = self.check_expression(expression);
            self.type_node_links.get(node).resolved_type = resolved_type;
            // This will allow types number, string, symbol or any. It will also allow enums, the unknown
            // type, and any union of these types (like string | number).
            if self.ty(resolved_type).flags.intersects(TypeFlags::NULLABLE)
                || !self.is_type_assignable_to_kind(
                    resolved_type,
                    TypeFlags::STRING_LIKE | TypeFlags::NUMBER_LIKE | TypeFlags::ES_SYMBOL_LIKE,
                ) && {
                    let string_number_symbol_type = self.string_number_symbol_type;
                    !self.is_type_assignable_to(resolved_type, string_number_symbol_type)
                }
            {
                self.error(
                    node,
                    diag::A_computed_property_name_must_be_of_type_string_number_symbol_or_any,
                    args![],
                );
            }
        }
        self.type_node_links.get(node).resolved_type
    }

    // Go: checker/checker.go:27287 isNoInferType
    pub fn is_no_infer_type(&self, t: TypeId) -> bool {
        // A NoInfer<T> type is represented as a substitution type with a TypeFlags.Unknown constraint.
        self.ty(t).flags.intersects(TypeFlags::SUBSTITUTION)
            && self
                .ty(self.ty(t).as_substitution_type().constraint)
                .flags
                .intersects(TypeFlags::UNKNOWN)
    }

    // Go: checker/checker.go:27292 getSubstitutionIntersection
    pub fn get_substitution_intersection(&mut self, t: TypeId) -> TypeId {
        if self.is_no_infer_type(t) {
            return self.ty(t).as_substitution_type().base_type;
        }
        let constraint = self.ty(t).as_substitution_type().constraint;
        let base_type = self.ty(t).as_substitution_type().base_type;
        self.get_intersection_type(&[constraint, base_type])
    }

    // Go: checker/checker.go:27299 shouldDeferIndexType
    pub fn should_defer_index_type(&mut self, t: TypeId, index_flags: IndexFlags) -> bool {
        let flags = self.ty(t).flags;
        flags.intersects(TypeFlags::INSTANTIABLE_NON_PRIMITIVE)
            || self.is_generic_tuple_type(t)
            || self.is_generic_mapped_type(t) && self.get_name_type_from_mapped_type(t).is_some()
            || flags.intersects(TypeFlags::UNION)
                && !index_flags.intersects(IndexFlags::NO_REDUCIBLE_CHECK)
                && self.is_generic_reducible_type(t)
            || flags.intersects(TypeFlags::INTERSECTION)
                && self.maybe_type_of_kind(t, TypeFlags::INSTANTIABLE)
                && {
                    (0..self.ty(t).types().len())
                        .any(|i| self.is_empty_anonymous_object_type(self.type_at(t, i)))
                }
    }

    // Go: checker/checker.go:27307 getMappedTypeNameTypeKind
    pub fn get_mapped_type_name_type_kind(&mut self, t: TypeId) -> MappedTypeNameTypeKind {
        let name_type = self.get_name_type_from_mapped_type(t);
        if name_type.is_nil() {
            return MappedTypeNameTypeKind::NONE;
        }
        let type_parameter = self.get_type_parameter_from_mapped_type(t);
        if self.is_type_assignable_to(name_type, type_parameter) {
            return MappedTypeNameTypeKind::FILTERING;
        }
        MappedTypeNameTypeKind::REMAPPING
    }

    // Go: checker/checker.go:27318 getIndexTypeForGenericType
    pub fn get_index_type_for_generic_type(
        &mut self,
        t: TypeId,
        index_flags: IndexFlags,
    ) -> TypeId {
        let key = CachedTypeKey {
            kind: if index_flags.intersects(IndexFlags::STRINGS_ONLY) {
                CachedTypeKind::STRING_INDEX_TYPE
            } else {
                CachedTypeKind::INDEX_TYPE
            },
            type_id: t,
        };
        if let Some(&index_type) = self.cached_types.get(&key) {
            if index_type.is_some() {
                return index_type;
            }
        }
        let index_type = self.new_index_type(t, index_flags & IndexFlags::STRINGS_ONLY);
        self.cached_types.insert(key, index_type);
        index_type
    }

    // This roughly mirrors `resolveMappedTypeMembers` in the nongeneric case, except only reports a union of the keys calculated,
    // rather than manufacturing the properties. We can't just fetch the `constraintType` since that would ignore mappings
    // and mapping the `constraintType` directly ignores how mapped types map _properties_ and not keys (thus ignoring subtype
    // reduction in the constraintType) when possible.
    // @param noIndexSignatures Indicates if _string_ index signatures should be elided. (other index signatures are always reported)
    // Go: checker/checker.go:27336 getIndexTypeForMappedType
    pub fn get_index_type_for_mapped_type(&mut self, t: TypeId, index_flags: IndexFlags) -> TypeId {
        let type_parameter = self.get_type_parameter_from_mapped_type(t);
        let constraint_type = self.get_constraint_type_from_mapped_type(t);
        let mapped_target = self.ty(t).as_mapped_type().object.target;
        let name_type = self.get_name_type_from_mapped_type(if mapped_target.is_some() {
            mapped_target
        } else {
            t
        });
        if name_type.is_nil() && !index_flags.intersects(IndexFlags::NO_INDEX_SIGNATURES) {
            // no mapping and no filtering required, just quickly bail to returning the constraint in the common case
            return constraint_type;
        }
        let mut key_types: Vec<TypeId> = Vec::new();
        let mut add_member_for_key_type = |c: &mut Checker, key_type: TypeId| {
            let mut prop_name_type = key_type;
            if name_type.is_some() {
                let mapper = c.ty(t).as_mapped_type().object.mapper;
                let mapper = c.append_type_mapping(mapper, type_parameter, key_type);
                prop_name_type = c.instantiate_type(name_type, mapper);
            }
            // `keyof` currently always returns `string | number` for concrete `string` index signatures - the below ternary keeps that behavior for mapped types
            // See `getLiteralTypeFromProperties` where there's a similar ternary to cause the same behavior.
            key_types.push(if prop_name_type == c.string_type {
                c.string_or_number_type
            } else {
                prop_name_type
            });
        };
        // Calling getApparentType on the `T` of a `keyof T` in the constraint type of a generic mapped type can
        // trigger a circularity. For example, `T extends { [P in keyof T & string as Captitalize<P>]: any }` is
        // a circular definition. For this reason, we only eagerly manifest the keys if the constraint is non-generic.
        if self.is_generic_index_type(constraint_type) {
            if self.is_mapped_type_with_keyof_constraint_declaration(t) {
                // We have a generic index and a homomorphic mapping and a key remapping - we need to defer
                // the whole `keyof whatever` for later since it's not safe to resolve the shape of modifier type.
                return self.get_index_type_for_generic_type(t, index_flags);
            }
            // Include the generic component in the resulting type.
            self.for_each_type(constraint_type, &mut add_member_for_key_type);
        } else if self.is_mapped_type_with_keyof_constraint_declaration(t) {
            let modifiers_type = self.get_modifiers_type_from_mapped_type(t);
            let modifiers_type = self.get_apparent_type(modifiers_type);
            // The 'T' in 'keyof T'
            self.for_each_mapped_type_property_key_type_and_index_signature_key_type(
                modifiers_type,
                TypeFlags::STRING_OR_NUMBER_LITERAL_OR_UNIQUE,
                index_flags.intersects(IndexFlags::STRINGS_ONLY),
                &mut add_member_for_key_type,
            );
        } else {
            let lower_bound = self.get_lower_bound_of_key_type(constraint_type);
            self.for_each_type(lower_bound, &mut add_member_for_key_type);
        }
        // We had to pick apart the constraintType to potentially map/filter it - compare the final resulting list with the
        // original constraintType, so we can return the union that preserves aliases/origin data if possible.
        let result;
        if index_flags.intersects(IndexFlags::NO_INDEX_SIGNATURES) {
            let union = self.get_union_type(&key_types);
            result = self.filter_type(union, &mut |c: &mut Checker, t: TypeId| {
                !c.ty(t).flags.intersects(TypeFlags::ANY | TypeFlags::STRING)
            });
        } else {
            result = self.get_union_type(&key_types);
        }
        if self.ty(result).flags.intersects(TypeFlags::UNION)
            && self.ty(constraint_type).flags.intersects(TypeFlags::UNION)
            && get_type_list_key(self.ty(result).types())
                == get_type_list_key(self.ty(constraint_type).types())
        {
            return constraint_type;
        }
        result
    }

    // Go: checker/checker.go:27388 getIndexedAccessType
    pub fn get_indexed_access_type(&mut self, object_type: TypeId, index_type: TypeId) -> TypeId {
        self.get_indexed_access_type_ex(object_type, index_type, AccessFlags::NONE, Node::NIL, None)
    }

    // Go: checker/checker.go:27392 getIndexedAccessTypeEx
    pub fn get_indexed_access_type_ex(
        &mut self,
        object_type: TypeId,
        index_type: TypeId,
        access_flags: AccessFlags,
        access_node: Node,
        alias: Option<Rc<TypeAlias>>,
    ) -> TypeId {
        let mut result = self.get_indexed_access_type_or_undefined(
            object_type,
            index_type,
            access_flags,
            access_node,
            alias,
        );
        if result.is_nil() {
            result = if access_node.is_some() {
                self.error_type
            } else {
                self.unknown_type
            };
        }
        result
    }

    // Go: checker/checker.go:27400 getIndexedAccessTypeOrUndefined
    // Returns `TypeId::NIL` where Go returns nil.
    pub fn get_indexed_access_type_or_undefined(
        &mut self,
        object_type: TypeId,
        index_type: TypeId,
        access_flags: AccessFlags,
        access_node: Node,
        alias: Option<Rc<TypeAlias>>,
    ) -> TypeId {
        let mut index_type = index_type;
        let mut access_flags = access_flags;
        if object_type == self.wildcard_type || index_type == self.wildcard_type {
            return self.wildcard_type;
        }
        let object_type = self.get_reduced_type(object_type);
        // If the object type has a string index signature and no other members we know that the result will
        // always be the type of that index signature and we can simplify accordingly.
        let is_string_index_signature_only_type = self.is_string_index_signature_only_type.clone();
        if is_string_index_signature_only_type(self, object_type)
            && !self.ty(index_type).flags.intersects(TypeFlags::NULLABLE)
            && self.is_type_assignable_to_kind(index_type, TypeFlags::STRING | TypeFlags::NUMBER)
        {
            index_type = self.string_type;
        }
        // In noUncheckedIndexedAccess mode, indexed access operations that occur in an expression in a read position and resolve to
        // an index signature have 'undefined' included in their type.
        if self.compiler_options.no_unchecked_indexed_access == Tristate::True
            && access_flags.intersects(AccessFlags::EXPRESSION_POSITION)
        {
            access_flags |= AccessFlags::INCLUDE_UNDEFINED;
        }
        // If the index type is generic, or if the object type is generic and doesn't originate in an expression and
        // the operation isn't exclusively indexing the fixed (non-variadic) portion of a tuple type, we are performing
        // a higher-order index access where we cannot meaningfully access the properties of the object type. Note that
        // for a generic T and a non-generic K, we eagerly resolve T[K] if it originates in an expression. This is to
        // preserve backwards compatibility. For example, an element access 'this["foo"]' has always been resolved
        // eagerly using the constraint type of 'this' at the given location.
        if self.should_defer_indexed_access_type(object_type, index_type, access_node) {
            if self
                .ty(object_type)
                .flags
                .intersects(TypeFlags::ANY_OR_UNKNOWN)
            {
                return object_type;
            }
            // Defer the operation by creating an indexed access type.
            let persistent_access_flags = access_flags & AccessFlags::PERSISTENT;
            let key = self.get_indexed_access_key(
                object_type,
                index_type,
                access_flags,
                alias.as_deref(),
            );
            let mut t = self
                .indexed_access_types
                .get(&key)
                .copied()
                .unwrap_or(TypeId::NIL);
            if t.is_nil() {
                t = self.new_indexed_access_type(object_type, index_type, persistent_access_flags);
                self.ty_mut(t).alias = alias;
                self.indexed_access_types.insert(key, t);
            }
            return t;
        }
        // In the following we resolve T[K] to the type of the property in T selected by K.
        // We treat boolean as different from other unions to improve errors;
        // skipping straight to getPropertyTypeForIndexType gives errors with 'boolean' instead of 'true'.
        let apparent_object_type = self.get_reduced_apparent_type(object_type);
        if self.ty(index_type).flags.intersects(TypeFlags::UNION)
            && !self.ty(index_type).flags.intersects(TypeFlags::BOOLEAN)
        {
            let mut prop_types: Vec<TypeId> = Vec::new();
            let mut was_missing_prop = false;
            for i in 0..self.ty(index_type).types().len() {
                let t = self.type_at(index_type, i);
                let extra = if was_missing_prop {
                    AccessFlags::SUPPRESS_NO_IMPLICIT_ANY_ERROR
                } else {
                    AccessFlags::NONE
                };
                let prop_type = self.get_property_type_for_index_type(
                    object_type,
                    apparent_object_type,
                    t,
                    index_type,
                    access_node,
                    access_flags | extra,
                );
                if prop_type.is_some() {
                    prop_types.push(prop_type);
                } else if access_node.is_nil() {
                    // If there's no error node, we can immediately stop, since error reporting is off
                    return TypeId::NIL;
                } else {
                    // Otherwise we set a flag and return at the end of the loop so we still mark all errors
                    was_missing_prop = true;
                }
            }
            if was_missing_prop {
                return TypeId::NIL;
            }
            if access_flags.intersects(AccessFlags::WRITING) {
                return self.get_intersection_type_ex(&prop_types, IntersectionFlags::NONE, alias);
            }
            return self.get_union_type_ex(
                &prop_types,
                UnionReduction::LITERAL,
                alias,
                TypeId::NIL,
            );
        }
        self.get_property_type_for_index_type(
            object_type,
            apparent_object_type,
            index_type,
            index_type,
            access_node,
            access_flags | AccessFlags::CACHE_SYMBOL | AccessFlags::REPORT_DEPRECATED,
        )
    }
}
