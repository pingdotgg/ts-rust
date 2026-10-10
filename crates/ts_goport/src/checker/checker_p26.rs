//! Port of typescript-go `internal/checker/checker.go` lines 23118-24045
//! (type arguments from nodes, tuple normalization, tuple/array predicates,
//! type alias references, declared types of symbols, enum member values,
//! and array/tuple type nodes).

use crate::jsnum::Number;
use crate::prelude::*;

impl Checker {
    // Go: checker/checker.go:23672 getTypeArgumentsFromNode
    pub fn get_type_arguments_from_node(&mut self, node: Node) -> Vec<TypeId> {
        let mut result = Vec::new();
        for n in node.type_arguments() {
            result.push(self.get_type_from_type_node(n));
        }
        result
    }

    // Go: checker/checker.go:23676 checkNoTypeArguments
    pub fn check_no_type_arguments(&mut self, node: Node, symbol: SymbolId) -> bool {
        if node.type_arguments().len() != 0 {
            let type_name = if symbol.is_some() {
                self.symbol_to_string(symbol)
            } else {
                declaration_name_to_string(node.type_name())
            };
            self.error(node, diag::Type_0_is_not_generic, args![type_name]);
            return false;
        }
        true
    }

    // Return true if the given type reference node is directly aliased or if it needs to be deferred
    // because it is possibly contained in a circular chain of eagerly resolved types.
    // Go: checker/checker.go:23692 isDeferredTypeReferenceNode
    pub fn is_deferred_type_reference_node(
        &mut self,
        node: Node,
        has_default_type_arguments: bool,
    ) -> bool {
        if self.get_alias_symbol_for_type_node(node).is_some() {
            return true;
        }
        if self.is_resolved_by_type_alias(node) {
            match node.kind() {
                SyntaxKind::ArrayType => {
                    return self.may_resolve_type_alias(node.element_type());
                }
                SyntaxKind::TupleType => {
                    for e in node.elements() {
                        if self.may_resolve_type_alias(e) {
                            return true;
                        }
                    }
                    return false;
                }
                SyntaxKind::TypeReference => {
                    if has_default_type_arguments {
                        return true;
                    }
                    for a in node.type_arguments() {
                        if self.may_resolve_type_alias(a) {
                            return true;
                        }
                    }
                    return false;
                }
                _ => {}
            }
            panic!("Unhandled case in isDeferredTypeReferenceNode");
        }
        false
    }

    // Return true when the given node is transitively contained in type constructs that eagerly
    // resolve their constituent types. We include SyntaxKind.TypeReference because type arguments
    // of type aliases are eagerly resolved.
    // Go: checker/checker.go:23713 isResolvedByTypeAlias
    pub fn is_resolved_by_type_alias(&self, node: Node) -> bool {
        let parent = node.parent();
        match parent.kind() {
            SyntaxKind::ParenthesizedType
            | SyntaxKind::NamedTupleMember
            | SyntaxKind::TypeReference
            | SyntaxKind::UnionType
            | SyntaxKind::IntersectionType
            | SyntaxKind::IndexedAccessType
            | SyntaxKind::ConditionalType
            | SyntaxKind::TypeOperator
            | SyntaxKind::ArrayType
            | SyntaxKind::TupleType => self.is_resolved_by_type_alias(parent),
            SyntaxKind::TypeAliasDeclaration | SyntaxKind::JsTypeAliasDeclaration => true,
            _ => false,
        }
    }

    // Return true if resolving the given node (i.e. getTypeFromTypeNode) possibly causes resolution
    // of a type alias.
    // Go: checker/checker.go:23727 mayResolveTypeAlias
    pub fn may_resolve_type_alias(&mut self, node: Node) -> bool {
        match node.kind() {
            SyntaxKind::TypeReference => {
                let s = self.resolve_type_reference_name(node, SymbolFlags::TYPE, false);
                self.sym(s).flags.intersects(SymbolFlags::TYPE_ALIAS)
            }
            SyntaxKind::TypeQuery => true,
            SyntaxKind::TypeOperator => {
                node.operator() != SyntaxKind::UniqueKeyword
                    && self.may_resolve_type_alias(node.type_())
            }
            SyntaxKind::ParenthesizedType
            | SyntaxKind::OptionalType
            | SyntaxKind::NamedTupleMember => self.may_resolve_type_alias(node.type_()),
            SyntaxKind::RestType => {
                node.type_().kind() != SyntaxKind::ArrayType
                    || self.may_resolve_type_alias(node.type_().element_type())
            }
            SyntaxKind::UnionType | SyntaxKind::IntersectionType => {
                for t in node.types().nodes() {
                    if self.may_resolve_type_alias(t) {
                        return true;
                    }
                }
                false
            }
            SyntaxKind::IndexedAccessType => {
                self.may_resolve_type_alias(node.object_type())
                    || self.may_resolve_type_alias(node.index_type())
            }
            SyntaxKind::ConditionalType => {
                self.may_resolve_type_alias(node.check_type())
                    || self.may_resolve_type_alias(node.extends_type())
                    || self.may_resolve_type_alias(node.true_type())
                    || self.may_resolve_type_alias(node.false_type())
            }
            _ => false,
        }
    }

    // Go: checker/checker.go:23752 createNormalizedTypeReference
    pub fn create_normalized_type_reference(
        &mut self,
        target: TypeId,
        type_arguments: &[TypeId],
    ) -> TypeId {
        if self.ty(target).object_flags.intersects(ObjectFlags::TUPLE) {
            return self.create_normalized_tuple_type(target, type_arguments);
        }
        self.create_type_reference(target, type_arguments)
    }

    // Go: checker/checker.go:23759 createNormalizedTupleTypeEx
    pub fn create_normalized_tuple_type_ex(
        &mut self,
        target: TypeId,
        element_types: &[TypeId],
        object_flags: ObjectFlags,
    ) -> TypeId {
        let (combined_flags, element_infos, readonly) = {
            let d = self.ty(target).as_tuple_type();
            (d.combined_flags, d.element_infos.clone(), d.readonly)
        };
        if !combined_flags.intersects(ElementFlags::NON_REQUIRED) {
            // No need to normalize when we only have regular required elements
            return self.create_type_reference_ex(target, element_types, object_flags);
        }
        if combined_flags.intersects(ElementFlags::VARIADIC) {
            for (i, &e) in element_types.iter().enumerate() {
                if i < element_infos.len()
                    && element_infos[i].flags.intersects(ElementFlags::VARIADIC)
                    && self
                        .ty(e)
                        .flags
                        .intersects(TypeFlags::NEVER | TypeFlags::UNION)
                {
                    // Transform [A, ...(X | Y | Z)] into [A, ...X] | [A, ...Y] | [A, ...Z]
                    let check_types: Vec<TypeId> = element_types
                        .iter()
                        .enumerate()
                        .map(|(i, &t)| {
                            if i < element_infos.len()
                                && element_infos[i].flags.intersects(ElementFlags::VARIADIC)
                            {
                                t
                            } else {
                                self.unknown_type
                            }
                        })
                        .collect();
                    if self.check_cross_product_union(&check_types) {
                        let element_types_copy = element_types.to_vec();
                        return self.map_type(e, &mut |c: &mut Checker, t: TypeId| {
                            // Go: core.ReplaceElement(elementTypes, i, t)
                            let mut replaced = element_types_copy.clone();
                            replaced[i] = t;
                            c.create_normalized_tuple_type_ex(target, &replaced, object_flags)
                        });
                    }
                }
            }
        }
        // We have optional, rest, or variadic elements that may need normalizing. Normalization ensures that all variadic
        // elements are generic and that the tuple type has one of the following layouts, disregarding variadic elements:
        // (1) Zero or more required elements, followed by zero or more optional elements, followed by zero or one rest element.
        // (2) Zero or more required elements, followed by a rest element, followed by zero or more required elements.
        // In either layout, zero or more generic variadic elements may be present at any location.
        // Note that the element types may contain an extra 'this' type argument that we want to ignore during normalization
        // and then just append to the normalized element types.
        let mut n = TupleNormalizer::default();
        if !n.normalize(self, &element_types[..element_infos.len()], &element_infos) {
            return self.error_type;
        }
        if element_types.len() > element_infos.len() {
            n.types.push(element_types[element_infos.len()]);
        }
        let tuple_target = self.get_tuple_target_type(&n.infos, readonly);
        if tuple_target == self.empty_generic_type {
            return self.empty_object_type;
        } else if n.types.len() != 0 {
            return self.create_type_reference_ex(tuple_target, &n.types, object_flags);
        }
        tuple_target
    }

    // Go: checker/checker.go:23807 createNormalizedTupleType
    pub fn create_normalized_tuple_type(
        &mut self,
        target: TypeId,
        element_types: &[TypeId],
    ) -> TypeId {
        self.create_normalized_tuple_type_ex(target, element_types, ObjectFlags::NONE)
    }
}

// PORT: Go `TupleNormalizer` stores the checker in field `c`. Here the
// checker is passed to `normalize` and `add` instead, because the normalizer
// lives on the stack while the checker is mutably borrowed.
// Go: checker/checker.go:23811 TupleNormalizer
#[derive(Clone, Debug, Default)]
pub struct TupleNormalizer {
    pub types: Vec<TypeId>,
    pub infos: Vec<TupleElementInfo>,
    pub last_required_index: i32,
    pub first_rest_index: i32,
    pub last_optional_or_rest_index: i32,
}

impl TupleNormalizer {
    // Go: checker/checker.go:23820 TupleNormalizer.normalize
    pub fn normalize(
        &mut self,
        c: &mut Checker,
        element_types: &[TypeId],
        element_infos: &[TupleElementInfo],
    ) -> bool {
        self.last_required_index = -1;
        self.first_rest_index = -1;
        self.last_optional_or_rest_index = -1;
        for (i, &t) in element_types.iter().enumerate() {
            let info = element_infos[i];
            if info.flags.intersects(ElementFlags::VARIADIC) {
                if c.ty(t).flags.intersects(TypeFlags::ANY) {
                    self.add(
                        c,
                        t,
                        TupleElementInfo {
                            flags: ElementFlags::REST,
                            labeled_declaration: info.labeled_declaration,
                        },
                    );
                } else if c
                    .ty(t)
                    .flags
                    .intersects(TypeFlags::INSTANTIABLE_NON_PRIMITIVE)
                    || c.is_generic_mapped_type(t)
                {
                    // Generic variadic elements stay as they are.
                    self.add(c, t, info);
                } else if c.is_tuple_type(t) {
                    let spread_types = c.get_element_types(t);
                    if spread_types.len() + self.types.len() >= 10_000 {
                        let message = if is_part_of_type_node(c.current_node) {
                            diag::Type_produces_a_tuple_type_that_is_too_large_to_represent
                        } else {
                            diag::Expression_produces_a_tuple_type_that_is_too_large_to_represent
                        };
                        let current_node = c.current_node;
                        c.error(current_node, message, vec![]);
                        return false;
                    }
                    // Spread variadic elements with tuple types into the resulting tuple.
                    let spread_infos = c.target_tuple_type(t).element_infos.clone();
                    for (j, &s) in spread_types.iter().enumerate() {
                        self.add(c, s, spread_infos[j]);
                    }
                } else {
                    // Treat everything else as an array type and create a rest element.
                    let mut s = TypeId::NIL;
                    if c.is_array_like_type(t) {
                        let number_type = c.number_type;
                        s = c.get_index_type_of_type(t, number_type);
                    }
                    if s.is_nil() {
                        s = c.error_type;
                    }
                    self.add(
                        c,
                        s,
                        TupleElementInfo {
                            flags: ElementFlags::REST,
                            labeled_declaration: info.labeled_declaration,
                        },
                    );
                }
            } else {
                // Copy other element kinds with no change.
                self.add(c, t, info);
            }
        }
        // Turn optional elements preceding the last required element into required elements
        for i in 0..self.last_required_index.max(0) as usize {
            if self.infos[i].flags.intersects(ElementFlags::OPTIONAL) {
                self.infos[i].flags = ElementFlags::REQUIRED;
            }
        }
        if self.first_rest_index >= 0 && self.first_rest_index < self.last_optional_or_rest_index {
            // Turn elements between first rest and last optional/rest into a single rest element
            let mut types = Vec::new();
            let first = self.first_rest_index as usize;
            let last = self.last_optional_or_rest_index as usize;
            for i in first..=last {
                let mut t = self.types[i];
                if self.infos[i].flags.intersects(ElementFlags::VARIADIC) {
                    let number_type = c.number_type;
                    t = c.get_indexed_access_type(t, number_type);
                }
                types.push(t);
            }
            self.types[first] = c.get_union_type(&types);
            self.types.drain(first + 1..last + 1);
            self.infos.drain(first + 1..last + 1);
        }
        true
    }

    // Go: checker/checker.go:23886 TupleNormalizer.add
    pub fn add(&mut self, c: &mut Checker, t: TypeId, info: TupleElementInfo) {
        if info.flags.intersects(ElementFlags::REQUIRED) {
            self.last_required_index = self.types.len() as i32;
        }
        if info.flags.intersects(ElementFlags::REST) && self.first_rest_index < 0 {
            self.first_rest_index = self.types.len() as i32;
        }
        if info
            .flags
            .intersects(ElementFlags::OPTIONAL | ElementFlags::REST)
        {
            self.last_optional_or_rest_index = self.types.len() as i32;
        }
        let t = c.add_optionality_ex(
            t,
            true, /*isProperty*/
            info.flags.intersects(ElementFlags::OPTIONAL),
        );
        self.types.push(t);
        self.infos.push(info);
    }
}

// Return count of starting consecutive tuple elements of the given kind(s)
// Go: checker/checker.go:23901 getStartElementCount
pub fn get_start_element_count(t: &TupleType, flags: ElementFlags) -> i32 {
    for (i, info) in t.element_infos.iter().enumerate() {
        if !info.flags.intersects(flags) {
            return i as i32;
        }
    }
    t.element_infos.len() as i32
}

// Return count of ending consecutive tuple elements of the given kind(s)
// Go: checker/checker.go:23911 getEndElementCount
pub fn get_end_element_count(t: &TupleType, flags: ElementFlags) -> i32 {
    let mut i = t.element_infos.len();
    while i > 0 {
        if !t.element_infos[i - 1].flags.intersects(flags) {
            return (t.element_infos.len() - i) as i32;
        }
        i -= 1;
    }
    t.element_infos.len() as i32
}

// Go: checker/checker.go:23920 getTotalFixedElementCount
pub fn get_total_fixed_element_count(t: &TupleType) -> i32 {
    t.fixed_length + get_end_element_count(t, ElementFlags::FIXED)
}

impl Checker {
    // Go: checker/checker.go:23924 getElementTypes
    pub fn get_element_types(&mut self, t: TypeId) -> SharedList<TypeId> {
        let type_arguments = self.get_type_arguments(t);
        let arity = self.get_type_reference_arity(t) as usize;
        if type_arguments.len() == arity {
            return type_arguments;
        }
        type_arguments.slice(0..arity)
    }

    // Go: checker/checker.go:23933 getTypeReferenceArity
    pub fn get_type_reference_arity(&self, t: TypeId) -> i32 {
        self.target_interface_type(t).type_parameters().len() as i32
    }

    // Go: checker/checker.go:23937 isArrayType
    pub fn is_array_type(&self, t: TypeId) -> bool {
        let ty = self.ty(t);
        ty.object_flags.intersects(ObjectFlags::REFERENCE)
            && (ty.target() == self.global_array_type
                || ty.target() == self.global_readonly_array_type)
    }

    // Go: checker/checker.go:23941 isReadonlyArrayType
    pub fn is_readonly_array_type(&self, t: TypeId) -> bool {
        let ty = self.ty(t);
        ty.object_flags.intersects(ObjectFlags::REFERENCE)
            && ty.target() == self.global_readonly_array_type
    }

    // PORT: Go package function `isTupleType` reads type data, so it is a
    // `Checker` method (see the PORT note in types.rs).
    // Go: checker/checker.go:23945 isTupleType
    // PERF: most types with REFERENCE are `TypeReference` data (instances
    // such as `T[]` and `[A, B]`), so the target is read from that variant
    // directly. `Type::target` (a jump table over the object kinds) takes
    // the rest: generic interface and tuple targets. The value is the same:
    // TypeReference data always has `TypeFlags::OBJECT`.
    #[inline]
    pub fn is_tuple_type(&self, t: TypeId) -> bool {
        let ty = self.ty(t);
        if !ty.object_flags.intersects(ObjectFlags::REFERENCE) {
            return false;
        }
        let target = match &ty.data {
            TypeData::TypeReference(r) => r.object.target,
            _ => ty.target(),
        };
        self.ty(target).object_flags.intersects(ObjectFlags::TUPLE)
    }

    // Go: checker/checker.go:23949 isMutableTupleType
    pub fn is_mutable_tuple_type(&self, t: TypeId) -> bool {
        self.is_tuple_type(t) && !self.target_tuple_type(t).readonly
    }

    // Go: checker/checker.go:23957 isSingleElementGenericTupleType
    pub fn is_single_element_generic_tuple_type(&self, t: TypeId) -> bool {
        self.is_generic_tuple_type(t) && self.target_tuple_type(t).element_infos.len() == 1
    }

    // Go: checker/checker.go:23961 isArrayOrTupleType
    pub fn is_array_or_tuple_type(&self, t: TypeId) -> bool {
        self.is_array_type(t) || self.is_tuple_type(t)
    }

    // Go: checker/checker.go:23965 isMutableArrayOrTuple
    pub fn is_mutable_array_or_tuple(&self, t: TypeId) -> bool {
        self.is_array_type(t) && !self.is_readonly_array_type(t)
            || self.is_tuple_type(t) && !self.target_tuple_type(t).readonly
    }

    // Go: checker/checker.go:23969 getElementTypeOfArrayType
    pub fn get_element_type_of_array_type(&mut self, t: TypeId) -> TypeId {
        if self.is_array_type(t) {
            return self.type_arguments_of(t)[0];
        }
        TypeId::NIL
    }

    // Go: checker/checker.go:23976 isArrayLikeType
    pub fn is_array_like_type(&mut self, t: TypeId) -> bool {
        // A type is array-like if it is a reference to the global Array or global ReadonlyArray type,
        // or if it is not the undefined or null type and if it is assignable to ReadonlyArray<any>
        if self.is_array_type(t) {
            return true;
        }
        if self.ty(t).flags.intersects(TypeFlags::NULLABLE) {
            return false;
        }
        let any_readonly_array_type = self.any_readonly_array_type;
        self.is_type_assignable_to(t, any_readonly_array_type)
    }

    // Go: checker/checker.go:23982 isMutableArrayLikeType
    pub fn is_mutable_array_like_type(&mut self, t: TypeId) -> bool {
        // A type is mutable-array-like if it is a reference to the global Array type, or if it is not the
        // any, undefined, null or never type and if it is assignable to Array<any>
        if self.is_mutable_array_or_tuple(t) {
            return true;
        }
        if self
            .ty(t)
            .flags
            .intersects(TypeFlags::ANY | TypeFlags::NULLABLE | TypeFlags::NEVER)
        {
            return false;
        }
        let any_array_type = self.any_array_type;
        self.is_type_assignable_to(t, any_array_type)
    }

    // Go: checker/checker.go:23988 isEmptyArrayLiteralType
    pub fn is_empty_array_literal_type(&mut self, t: TypeId) -> bool {
        let element_type = self.get_element_type_of_array_type(t);
        element_type.is_some() && self.is_empty_literal_type(element_type)
    }

    // Go: checker/checker.go:23993 isEmptyLiteralType
    pub fn is_empty_literal_type(&self, t: TypeId) -> bool {
        if self.strict_null_checks {
            return t == self.implicit_never_type;
        }
        t == self.undefined_widening_type
    }

    // Go: checker/checker.go:24000 isTupleLikeType
    pub fn is_tuple_like_type(&mut self, t: TypeId) -> bool {
        if self.is_tuple_type(t) || self.get_property_of_type(t, "0").is_some() {
            return true;
        }
        if self.is_array_like_type(t) {
            let length_type = self.get_type_of_property_of_type(t, "length");
            if length_type.is_some() {
                return self.every_type(length_type, &mut |c: &mut Checker, t: TypeId| {
                    c.ty(t).flags.intersects(TypeFlags::NUMBER_LITERAL)
                });
            }
        }
        false
    }

    // Go: checker/checker.go:24012 isArrayOrTupleLikeType
    pub fn is_array_or_tuple_like_type(&mut self, t: TypeId) -> bool {
        self.is_array_like_type(t) || self.is_tuple_like_type(t)
    }

    // Go: checker/checker.go:24016 isArrayOrTupleOrIntersection
    pub fn is_array_or_tuple_or_intersection(&self, t: TypeId) -> bool {
        let ty = self.ty(t);
        ty.flags.intersects(TypeFlags::INTERSECTION)
            && ty.types().iter().all(|&t| self.is_array_or_tuple_type(t))
    }

    // Go: checker/checker.go:24020 getTupleElementType
    pub fn get_tuple_element_type(&mut self, t: TypeId, index: i32) -> TypeId {
        let prop_type = self.get_type_of_property_of_type(t, &index.to_string());
        if prop_type.is_some() {
            return prop_type;
        }
        if self.every_type(t, &mut |c: &mut Checker, t: TypeId| c.is_tuple_type(t)) {
            let undefined_like_type =
                if self.compiler_options.no_unchecked_indexed_access == Tristate::True {
                    self.undefined_type
                } else {
                    TypeId::NIL
                };
            return self.get_tuple_element_type_out_of_start_count(
                t,
                Number::new(index as f64),
                undefined_like_type,
            );
        }
        TypeId::NIL
    }

    /**
     * Get type from reference to type alias. When a type alias is generic, the declared type of the type alias may include
     * references to the type parameters of the alias. We replace those with the actual type arguments by instantiating the
     * declared type. Instantiations are cached using the type identities of the type arguments as the key.
     */
    // Go: checker/checker.go:24036 getTypeFromTypeAliasReference
    pub fn get_type_from_type_alias_reference(&mut self, node: Node, symbol: SymbolId) -> TypeId {
        let type_arguments = node.type_arguments();
        if self
            .sym(symbol)
            .check_flags
            .intersects(CheckFlags::UNRESOLVED)
        {
            let mut alias_type_arguments = Vec::new();
            for a in type_arguments {
                alias_type_arguments.push(self.get_type_from_type_node(a));
            }
            let alias = Rc::new(TypeAlias {
                symbol,
                type_arguments: alias_type_arguments,
            });
            let key = get_alias_key(&self.symbols, Some(&*alias));
            let mut error_type = self.error_types.get(&key).copied().unwrap_or_default();
            if error_type.is_nil() {
                error_type = self.new_intrinsic_type(TypeFlags::ANY, "error");
                self.ty_mut(error_type).alias = Some(alias);
                self.error_types.insert(key, error_type);
            }
            return error_type;
        }
        let t = self.get_declared_type_of_symbol(symbol);
        let type_parameters = self.type_alias_links.get(symbol).type_parameters.clone();
        if type_parameters.len() != 0 {
            let num_type_arguments = type_arguments.len() as i32;
            let min_type_argument_count = self.get_min_type_argument_count(&type_parameters);
            if num_type_arguments < min_type_argument_count
                || num_type_arguments > type_parameters.len() as i32
            {
                let message = if min_type_argument_count == type_parameters.len() as i32 {
                    diag::Generic_type_0_requires_1_type_argument_s
                } else {
                    diag::Generic_type_0_requires_between_1_and_2_type_arguments
                };
                let symbol_string = self.symbol_to_string(symbol);
                self.error(
                    node,
                    message,
                    args![
                        symbol_string,
                        min_type_argument_count,
                        type_parameters.len()
                    ],
                );
                return self.error_type;
            }
            // We refrain from associating a local type alias with an instantiation of a top-level type alias
            // because the local alias may end up being referenced in an inferred return type where it is not
            // accessible--which in turn may lead to a large structural expansion of the type when generating
            // a .d.ts file. See #43622 for an example.
            let alias_symbol = self.get_alias_symbol_for_type_node(node);
            let mut new_alias_symbol = SymbolId::NIL;
            if alias_symbol.is_some()
                && (self.is_local_type_alias(symbol) || !self.is_local_type_alias(alias_symbol))
            {
                new_alias_symbol = alias_symbol;
            }
            let mut alias_type_arguments = Vec::new();
            if new_alias_symbol.is_some() {
                alias_type_arguments = self.get_type_arguments_for_alias_symbol(new_alias_symbol);
            } else if is_type_reference_type(node) {
                let alias_symbol = self.resolve_type_reference_name(
                    node,
                    SymbolFlags::ALIAS,
                    true, /*ignoreErrors*/
                );
                // refers to an alias import/export/reexport - by making sure we use the target as an aliasSymbol,
                // we ensure the exported symbol is used to refer to the type when it is reserialized later
                if alias_symbol.is_some() && alias_symbol != self.unknown_symbol {
                    let resolved = self.resolve_alias(alias_symbol);
                    if resolved.is_some()
                        && self.sym(resolved).flags.intersects(SymbolFlags::TYPE_ALIAS)
                    {
                        new_alias_symbol = resolved;
                        alias_type_arguments = self.get_type_arguments_from_node(node);
                    }
                }
            }
            let mut new_alias = None;
            if new_alias_symbol.is_some() {
                new_alias = Some(Rc::new(TypeAlias {
                    symbol: new_alias_symbol,
                    type_arguments: alias_type_arguments,
                }));
            }
            let args_from_node = self.get_type_arguments_from_node(node);
            return self.get_type_alias_instantiation(symbol, &args_from_node, new_alias);
        }
        if self.check_no_type_arguments(node, symbol) {
            return t;
        }
        self.error_type
    }

    // Go: checker/checker.go:24097 getTypeAliasInstantiation
    pub fn get_type_alias_instantiation(
        &mut self,
        symbol: SymbolId,
        type_arguments: &[TypeId],
        alias: Option<Rc<TypeAlias>>,
    ) -> TypeId {
        let t = self.get_declared_type_of_symbol(symbol);
        if t == self.intrinsic_marker_type {
            let name = self.sym(symbol).name.clone();
            if let Some(&type_kind) = INTRINSIC_TYPE_KINDS.get(name.as_str()) {
                if type_arguments.len() == 1 {
                    match type_kind {
                        IntrinsicTypeKind::NO_INFER => {
                            return self.get_no_infer_type(type_arguments[0]);
                        }
                        _ => {
                            return self.get_string_mapping_type(symbol, type_arguments[0]);
                        }
                    }
                }
            }
        }
        let type_parameters = self.type_alias_links.get(symbol).type_parameters.clone();
        let key = get_type_alias_instantiation_key(&self.symbols, type_arguments, alias.as_deref());
        let mut instantiation = self
            .type_alias_links
            .get(symbol)
            .instantiations
            .as_ref()
            .and_then(|m| m.get(&key).copied())
            .unwrap_or_default();
        if instantiation.is_nil() {
            let min_type_argument_count = self.get_min_type_argument_count(&type_parameters);
            let value_declaration = self.sym(symbol).value_declaration;
            let filled = self.fill_missing_type_arguments(
                type_arguments,
                &type_parameters,
                min_type_argument_count,
                is_in_js_file(value_declaration),
            );
            let mapper = self.new_type_mapper(&type_parameters, &filled);
            instantiation = self.instantiate_type_with_alias(t, mapper, alias);
            // PORT: Go writes into the links map; a nil map (non-generic alias) panics like Go.
            self.type_alias_links
                .get(symbol)
                .instantiations
                .as_mut()
                .expect("assignment to entry in nil map")
                .insert(key, instantiation);
        }
        instantiation
    }

    // PORT: Go package function `isLocalTypeAlias` reads symbol data, so it
    // is a `Checker` method.
    // Go: checker/checker.go:24121 isLocalTypeAlias
    pub fn is_local_type_alias(&self, symbol: SymbolId) -> bool {
        let declaration = self
            .sym(symbol)
            .declarations
            .iter()
            .copied()
            .find(|&d| is_type_alias(d))
            .unwrap_or_default();
        declaration.is_some() && get_containing_function(declaration).is_some()
    }

    // Go: checker/checker.go:24126 getDeclaredTypeOfSymbol
    pub fn get_declared_type_of_symbol(&mut self, symbol: SymbolId) -> TypeId {
        let mut result = self.try_get_declared_type_of_symbol(symbol);
        if result.is_nil() {
            result = self.error_type;
        }
        result
    }

    // Go: checker/checker.go:24134 tryGetDeclaredTypeOfSymbol
    pub fn try_get_declared_type_of_symbol(&mut self, symbol: SymbolId) -> TypeId {
        if symbol.is_nil() {
            // Go reads `symbol.Flags` of a nil symbol: `getTypeOfNode` on a
            // type parameter that the binder skips (a JSDoc `@template` in
            // a TS file) passes nil here.
            go_nil_dereference();
        }
        let flags = self.sym(symbol).flags;
        if flags.intersects(SymbolFlags::CLASS | SymbolFlags::INTERFACE) {
            return self.get_declared_type_of_class_or_interface(symbol);
        } else if flags.intersects(SymbolFlags::TYPE_PARAMETER) {
            return self.get_declared_type_of_type_parameter(symbol);
        } else if flags.intersects(SymbolFlags::TYPE_ALIAS) {
            return self.get_declared_type_of_type_alias(symbol);
        } else if flags.intersects(SymbolFlags::ENUM) {
            return self.get_declared_type_of_enum(symbol);
        } else if flags.intersects(SymbolFlags::ENUM_MEMBER) {
            return self.get_declared_type_of_enum_member(symbol);
        } else if flags.intersects(SymbolFlags::ALIAS) {
            return self.get_declared_type_of_alias(symbol);
        }
        TypeId::NIL
    }
}

// Go: checker/checker.go:24152 getTypeReferenceName
pub fn get_type_reference_name(node: Node) -> Node {
    match node.kind() {
        SyntaxKind::TypeReference => {
            return node.type_name();
        }
        SyntaxKind::ExpressionWithTypeArguments => {
            // We only support expressions that are simple qualified names. For other
            // expressions this produces nil
            let expr = node.expression();
            if is_entity_name_expression(expr) {
                return expr;
            }
        }
        _ => {}
    }
    Node::NIL
}

impl Checker {
    // Go: checker/checker.go:24167 getAliasForTypeNode
    pub fn get_alias_for_type_node(&mut self, node: Node) -> Option<Rc<TypeAlias>> {
        let symbol = self.get_alias_symbol_for_type_node(node);
        if symbol.is_some() {
            let type_arguments = self.get_type_arguments_for_alias_symbol(symbol);
            return Some(Rc::new(TypeAlias {
                symbol,
                type_arguments,
            }));
        }
        None
    }

    // Go: checker/checker.go:24175 getAliasSymbolForTypeNode
    pub fn get_alias_symbol_for_type_node(&mut self, node: Node) -> SymbolId {
        let mut host = node.parent();
        while is_parenthesized_type_node(host)
            || is_type_operator_node(host) && host.operator() == SyntaxKind::ReadonlyKeyword
        {
            host = host.parent();
        }
        if is_type_alias(host) {
            return self.get_symbol_of_declaration(host);
        }
        SymbolId::NIL
    }

    // Go: checker/checker.go:24186 getTypeArgumentsForAliasSymbol
    pub fn get_type_arguments_for_alias_symbol(&mut self, symbol: SymbolId) -> Vec<TypeId> {
        if symbol.is_some() {
            return self.get_local_type_parameters_of_class_or_interface_or_type_alias(symbol);
        }
        Vec::new()
    }

    // Go: checker/checker.go:24193 getOuterTypeParametersOfClassOrInterface
    pub fn get_outer_type_parameters_of_class_or_interface(
        &mut self,
        symbol: SymbolId,
    ) -> Vec<TypeId> {
        let declaration = self.get_class_or_interface_like_declaration(symbol);
        debug_assert!(
            declaration.is_some(),
            "Class was missing valueDeclaration -OR- non-class had no interface declarations"
        );
        self.get_outer_type_parameters(declaration, false /*includeThisTypes*/)
    }

    // Go: checker/checker.go:24200 getClassOrInterfaceLikeDeclaration
    // Returns the declaration used to obtain a class, interface, or function symbol's outer type parameters.
    pub fn get_class_or_interface_like_declaration(&self, symbol: SymbolId) -> Node {
        if self
            .sym(symbol)
            .flags
            .intersects(SymbolFlags::CLASS | SymbolFlags::FUNCTION)
        {
            return self.sym(symbol).value_declaration;
        }
        self.sym(symbol)
            .declarations
            .iter()
            .copied()
            .find(|&d| {
                if is_interface_declaration(d) {
                    return true;
                }
                if !is_variable_declaration(d) {
                    return false;
                }
                let initializer = d.initializer();
                initializer.is_some() && is_function_expression_or_arrow_function(initializer)
            })
            .unwrap_or_default()
    }

    // Go: checker/checker.go:24216 canGetTypeParametersOfClassOrInterface
    pub fn can_get_type_parameters_of_class_or_interface(&self, symbol: SymbolId) -> bool {
        self.get_class_or_interface_like_declaration(symbol)
            .is_some()
    }

    // Return the outer type parameters of a node or undefined if the node has no outer type parameters.
    // Go: checker/checker.go:24221 getOuterTypeParameters
    pub fn get_outer_type_parameters(
        &mut self,
        node: Node,
        include_this_types: bool,
    ) -> Vec<TypeId> {
        let mut node = node;
        loop {
            node = node.parent();
            if node.is_nil() {
                return Vec::new();
            }
            let kind = node.kind();
            match kind {
                SyntaxKind::ClassDeclaration
                | SyntaxKind::ClassExpression
                | SyntaxKind::InterfaceDeclaration
                | SyntaxKind::CallSignature
                | SyntaxKind::ConstructSignature
                | SyntaxKind::MethodSignature
                | SyntaxKind::FunctionType
                | SyntaxKind::ConstructorType
                | SyntaxKind::FunctionDeclaration
                | SyntaxKind::MethodDeclaration
                | SyntaxKind::FunctionExpression
                | SyntaxKind::ArrowFunction
                | SyntaxKind::TypeAliasDeclaration
                | SyntaxKind::JsTypeAliasDeclaration
                | SyntaxKind::MappedType
                | SyntaxKind::ConditionalType => {
                    let mut outer_type_parameters =
                        self.get_outer_type_parameters(node, include_this_types);
                    if (kind == SyntaxKind::FunctionExpression
                        || kind == SyntaxKind::ArrowFunction
                        || is_object_literal_method(node))
                        && self.is_context_sensitive(node)
                    {
                        let symbol = self.get_symbol_of_declaration(node);
                        let type_of_symbol = self.get_type_of_symbol(symbol);
                        let signature = self
                            .get_signatures_of_type(type_of_symbol, SignatureKind::CALL)
                            .first()
                            .copied()
                            .unwrap_or_default();
                        if signature.is_some() && self.sig(signature).type_parameters.len() != 0 {
                            outer_type_parameters
                                .extend(self.sig(signature).type_parameters.iter().copied());
                            return outer_type_parameters;
                        }
                    }
                    if kind == SyntaxKind::MappedType {
                        let symbol = self.get_symbol_of_declaration(node.type_parameter());
                        let tp = self.get_declared_type_of_type_parameter(symbol);
                        outer_type_parameters.push(tp);
                        return outer_type_parameters;
                    }
                    if kind == SyntaxKind::ConditionalType {
                        let infer_type_parameters = self.get_infer_type_parameters(node);
                        outer_type_parameters.extend(infer_type_parameters);
                        return outer_type_parameters;
                    }
                    let mut outer_and_own_type_parameters =
                        self.append_type_parameters(outer_type_parameters, node.type_parameters());
                    let mut this_type = TypeId::NIL;
                    if include_this_types
                        && (kind == SyntaxKind::ClassDeclaration
                            || kind == SyntaxKind::ClassExpression
                            || kind == SyntaxKind::InterfaceDeclaration)
                    {
                        let symbol = self.get_symbol_of_declaration(node);
                        let declared = self.get_declared_type_of_class_or_interface(symbol);
                        this_type = self.ty(declared).as_interface_type().this_type;
                    }
                    if this_type.is_some() {
                        outer_and_own_type_parameters.push(this_type);
                        return outer_and_own_type_parameters;
                    }
                    return outer_and_own_type_parameters;
                }
                _ => {}
            }
        }
    }

    // Go: checker/checker.go:24259 getInferTypeParameters
    pub fn get_infer_type_parameters(&mut self, node: Node) -> Vec<TypeId> {
        let mut result = Vec::new();
        // PORT: Go ranges over the locals map (random order). We use the
        // symbol table insertion order; the sort below gives Go's order.
        for symbol in self.symbols.values(node.locals()) {
            if self
                .sym(symbol)
                .flags
                .intersects(SymbolFlags::TYPE_PARAMETER)
            {
                let t = self.get_declared_type_of_symbol(symbol);
                result.push(t);
            }
        }
        // ts#64621, Go N' checker.go:24334: a stable order for the type mapper.
        result.sort_by(|a, b| self.compare_types(*a, *b).cmp(&0));
        result
    }

    // The local type parameters are the combined set of type parameters from all declarations of the class,
    // interface, or type alias.
    // Go: checker/checker.go:24271 getLocalTypeParametersOfClassOrInterfaceOrTypeAlias
    pub fn get_local_type_parameters_of_class_or_interface_or_type_alias(
        &mut self,
        symbol: SymbolId,
    ) -> Vec<TypeId> {
        self.append_local_type_parameters_of_class_or_interface_or_type_alias(Vec::new(), symbol)
    }

    // Go: checker/checker.go:24275 appendLocalTypeParametersOfClassOrInterfaceOrTypeAlias
    pub fn append_local_type_parameters_of_class_or_interface_or_type_alias(
        &mut self,
        types: Vec<TypeId>,
        symbol: SymbolId,
    ) -> Vec<TypeId> {
        let mut types = types;
        for &node in self.sym(symbol).declarations.clone().iter() {
            if node_kind_is(
                node,
                &[
                    SyntaxKind::InterfaceDeclaration,
                    SyntaxKind::ClassDeclaration,
                    SyntaxKind::ClassExpression,
                ],
            ) || is_type_alias(node)
            {
                types = self.append_type_parameters(types, node.type_parameters());
            }
        }
        types
    }

    // Appends the type parameters given by a list of declarations to a set of type parameters and returns the resulting set.
    // The function allocates a new array if the input type parameter set is undefined, but otherwise it modifies the set
    // in-place and returns the same array.
    // Go: checker/checker.go:24287 appendTypeParameters
    pub fn append_type_parameters(
        &mut self,
        type_parameters: Vec<TypeId>,
        declarations: NodeSlice,
    ) -> Vec<TypeId> {
        let mut type_parameters = type_parameters;
        for declaration in declarations {
            let symbol = self.get_symbol_of_declaration(declaration);
            let tp = self.get_declared_type_of_type_parameter(symbol);
            // Go: core.AppendIfUnique
            if !type_parameters.contains(&tp) {
                type_parameters.push(tp);
            }
        }
        type_parameters
    }

    // Go: checker/checker.go:24294 getDeclaredTypeOfTypeParameter
    pub fn get_declared_type_of_type_parameter(&mut self, symbol: SymbolId) -> TypeId {
        // One link lookup on the cached hit. The miss returns the type it
        // just stored, as Go returns links.declaredType.
        let cached = self.declared_type_links.get(symbol).declared_type;
        if cached.is_some() {
            return cached;
        }
        let t = self.new_type_parameter(symbol);
        self.declared_type_links.get(symbol).declared_type = t;
        t
    }

    // Go: checker/checker.go:24302 getDeclaredTypeOfTypeAlias
    pub fn get_declared_type_of_type_alias(&mut self, symbol: SymbolId) -> TypeId {
        if self.type_alias_links.get(symbol).declared_type.is_nil() {
            // Note that we use the links object as the target here because the symbol object is used as the unique
            // identity for resolution of the 'type' property in SymbolLinks.
            if !self.push_type_resolution(
                TypeSystemEntity::Symbol(symbol),
                TypeSystemPropertyName::DECLARED_TYPE,
            ) {
                return self.error_type;
            }
            let declaration = self
                .sym(symbol)
                .declarations
                .iter()
                .copied()
                .find(|&d| is_type_or_js_type_alias_declaration(d))
                .unwrap_or_default();
            let type_node = declaration.type_();
            let mut t = self.get_type_from_type_node(type_node);
            if self.pop_type_resolution() {
                let type_parameters =
                    self.get_local_type_parameters_of_class_or_interface_or_type_alias(symbol);
                if type_parameters.len() != 0 {
                    // Initialize the instantiation cache for generic type aliases. The declared type corresponds to
                    // an instantiation of the type alias with the type parameters supplied as type arguments.
                    let key = get_type_list_key(&type_parameters);
                    let links = self.type_alias_links.get(symbol);
                    links.type_parameters = type_parameters;
                    let mut instantiations = InstantiationMap::default();
                    instantiations.insert(key, t);
                    links.instantiations = Some(instantiations);
                }
                if t == self.intrinsic_marker_type
                    && self.sym(symbol).name == "BuiltinIteratorReturn"
                {
                    t = self.get_builtin_iterator_return_type();
                }
            } else {
                let mut error_node = declaration.name();
                if error_node.is_nil() {
                    error_node = declaration;
                }
                let symbol_string = self.symbol_to_string(symbol);
                self.error(
                    error_node,
                    diag::Type_alias_0_circularly_references_itself,
                    args![symbol_string],
                );
                t = self.error_type;
            }
            let links = self.type_alias_links.get(symbol);
            if links.declared_type.is_nil() {
                links.declared_type = t;
            }
        }
        self.type_alias_links.get(symbol).declared_type
    }

    // Go: checker/checker.go:24340 getDeclaredTypeOfEnum
    pub fn get_declared_type_of_enum(&mut self, symbol: SymbolId) -> TypeId {
        if !(self.declared_type_links.get(symbol).declared_type.is_some()) {
            let mut member_type_list = Vec::new();
            for &declaration in self.sym(symbol).declarations.clone().iter() {
                if declaration.kind() == SyntaxKind::EnumDeclaration {
                    for member in declaration.members() {
                        if !has_dynamic_name(member) {
                            let member_symbol = self.get_symbol_of_declaration(member);
                            let value = self.get_enum_member_value(member).value;
                            let member_type = if let Some(value) = value {
                                self.get_enum_literal_type(value, symbol, member_symbol)
                            } else {
                                self.create_computed_enum_type(member_symbol)
                            };
                            let fresh = self.get_fresh_type_of_literal_type(member_type);
                            self.declared_type_links.get(member_symbol).declared_type = fresh;
                            member_type_list.push(member_type);
                        }
                    }
                }
            }
            let enum_type = if member_type_list.len() != 0 {
                self.get_union_type_ex(
                    &member_type_list,
                    UnionReduction::LITERAL,
                    Some(Rc::new(TypeAlias {
                        symbol,
                        type_arguments: Vec::new(),
                    })),
                    TypeId::NIL, /*origin*/
                )
            } else {
                self.create_computed_enum_type(symbol)
            };
            if self.ty(enum_type).flags.intersects(TypeFlags::UNION) {
                let ty = self.ty_mut(enum_type);
                ty.flags |= TypeFlags::ENUM_LITERAL;
                ty.symbol = symbol;
            }
            self.declared_type_links.get(symbol).declared_type = enum_type;
        }
        self.declared_type_links.get(symbol).declared_type
    }

    // Go: checker/checker.go:24377 getEnumMemberValue
    pub fn get_enum_member_value(&mut self, node: Node) -> EvaluatorResult {
        self.compute_enum_member_values(node.parent());
        self.enum_member_links.get(node).value.clone()
    }

    // Go: checker/checker.go:24382 createComputedEnumType
    pub fn create_computed_enum_type(&mut self, symbol: SymbolId) -> TypeId {
        let regular_type = self.new_literal_type(TypeFlags::ENUM, None, TypeId::NIL);
        self.ty_mut(regular_type).symbol = symbol;
        let fresh_type = self.new_literal_type(TypeFlags::ENUM, None, regular_type);
        self.ty_mut(fresh_type).symbol = symbol;
        self.ty_mut(regular_type).as_literal_type_mut().fresh_type = fresh_type;
        self.ty_mut(fresh_type).as_literal_type_mut().fresh_type = fresh_type;
        regular_type
    }

    // Go: checker/checker.go:24392 getDeclaredTypeOfEnumMember
    pub fn get_declared_type_of_enum_member(&mut self, symbol: SymbolId) -> TypeId {
        if !(self.declared_type_links.get(symbol).declared_type.is_some()) {
            let parent = self.get_parent_of_symbol(symbol);
            let enum_type = self.get_declared_type_of_enum(parent);
            let links = self.declared_type_links.get(symbol);
            if links.declared_type.is_nil() {
                links.declared_type = enum_type;
            }
        }
        self.declared_type_links.get(symbol).declared_type
    }

    // Go: checker/checker.go:24403 computeEnumMemberValues
    pub fn compute_enum_member_values(&mut self, node: Node) {
        let node_links = self.node_links.get(node);
        if !(node_links
            .flags
            .intersects(NodeCheckFlags::ENUM_VALUES_COMPUTED))
        {
            node_links.flags |= NodeCheckFlags::ENUM_VALUES_COMPUTED;
            let mut auto_value: Option<Number> = Some(Number::new(0.0));
            let mut previous = Node::NIL;
            for member in node.members() {
                let result = self.compute_enum_member_value(member, auto_value, previous);
                let number_value = match &result.value {
                    Some(LiteralValue::Number(value)) => Some(*value),
                    _ => None,
                };
                self.enum_member_links.get(member).value = result;
                if let Some(value) = number_value {
                    let next_value = value + Number::new(1.0);
                    auto_value = Some(next_value);
                } else {
                    auto_value = None;
                }
                previous = member;
            }
        }
    }

    // Go: checker/checker.go:24423 computeEnumMemberValue
    pub fn compute_enum_member_value(
        &mut self,
        member: Node,
        auto_value: Option<Number>,
        previous: Node,
    ) -> EvaluatorResult {
        if is_computed_non_literal_name(member.name()) {
            self.error(
                member.name(),
                diag::Computed_property_names_are_not_allowed_in_enums,
                vec![],
            );
        } else if is_big_int_literal(member.name()) {
            self.error(
                member.name(),
                diag::An_enum_member_cannot_have_a_numeric_name,
                vec![],
            );
        } else {
            let text = get_text_of_property_name(member.name());
            if is_numeric_literal_name(&text) && !is_infinity_or_nan_string(&text) {
                self.error(
                    member.name(),
                    diag::An_enum_member_cannot_have_a_numeric_name,
                    vec![],
                );
            }
        }
        if member.initializer().is_some() {
            return self.compute_constant_enum_member_value(member);
        }
        // In ambient non-const numeric enum declarations, enum members without initializers are
        // considered computed members (as opposed to having auto-incremented values).
        if member.parent().flags().intersects(NodeFlags::AMBIENT) && !is_enum_const(member.parent())
        {
            return new_result(None, false, false, false);
        }
        // If the member declaration specifies no value, the member is considered a constant enum member.
        // If the member is the first member in the enum declaration, it is assigned the value zero.
        // Otherwise, it is assigned the value of the immediately preceding member plus one, and an error
        // occurs if the immediately preceding member is not a constant enum member.
        let Some(auto_value) = auto_value else {
            self.error(
                member.name(),
                diag::Enum_member_must_have_initializer,
                vec![],
            );
            return new_result(None, false, false, false);
        };
        if self.compiler_options.get_isolated_modules()
            && previous.is_some()
            && previous.initializer().is_some()
        {
            let prev_value = self.get_enum_member_value(previous);
            let prev_is_num = matches!(prev_value.value, Some(LiteralValue::Number(_)));
            if !prev_is_num || prev_value.resolved_other_files {
                self.error(
                    member.name(),
                    diag::Enum_member_following_a_non_literal_numeric_member_must_have_an_initializer_when_isolatedModules_is_enabled,
                    vec![],
                );
            }
        }
        new_result(Some(LiteralValue::Number(auto_value)), false, false, false)
    }

    // Go: checker/checker.go:24460 computeConstantEnumMemberValue
    pub fn compute_constant_enum_member_value(&mut self, member: Node) -> EvaluatorResult {
        let is_const_enum = is_enum_const(member.parent());
        let initializer = member.initializer();
        let ev = self.evaluate.clone();
        let result = ev(self, initializer, member);
        if result.value.is_some() {
            if is_const_enum {
                if let Some(LiteralValue::Number(num_value)) = &result.value {
                    let num_value = *num_value;
                    if num_value.is_infinite() || num_value.is_nan() {
                        self.error(
                            initializer,
                            if num_value.is_nan() {
                                diag::X_const_enum_member_initializer_was_evaluated_to_disallowed_value_NaN
                            } else {
                                diag::X_const_enum_member_initializer_was_evaluated_to_a_non_finite_value
                            },
                            vec![],
                        );
                    }
                }
            }
            if self.compiler_options.get_isolated_modules() {
                if matches!(result.value, Some(LiteralValue::String(_)))
                    && !result.is_syntactically_string
                {
                    let member_name =
                        format!("{}.{}", member.parent().name().text(), member.name().text());
                    self.error(
                        initializer,
                        diag::X_0_has_a_string_type_but_must_have_syntactically_recognizable_string_syntax_when_isolatedModules_is_enabled,
                        args![member_name],
                    );
                }
            }
        } else if is_const_enum {
            self.error(
                initializer,
                diag::X_const_enum_member_initializers_must_be_constant_expressions,
                vec![],
            );
        } else if member.parent().flags().intersects(NodeFlags::AMBIENT) {
            self.error(
                initializer,
                diag::In_ambient_enum_declarations_member_initializer_must_be_constant_expression,
                vec![],
            );
        } else {
            let source = self.check_expression(initializer);
            let number_type = self.number_type;
            self.check_type_assignable_to(
                source,
                number_type,
                initializer,
                Some(diag::Type_0_is_not_assignable_to_type_1_as_required_for_computed_enum_member_values),
            );
        }
        result
    }

    // Go: checker/checker.go:24489 evaluateEntity
    pub fn evaluate_entity(&mut self, expr: Node, location: Node) -> EvaluatorResult {
        match expr.kind() {
            SyntaxKind::Identifier | SyntaxKind::PropertyAccessExpression => {
                let symbol = self.resolve_entity_name(
                    expr,
                    SymbolFlags::VALUE,
                    true, /*ignoreErrors*/
                    false,
                    Node::NIL,
                );
                if symbol.is_nil() {
                    return new_result(None, false, false, false);
                }
                if expr.kind() == SyntaxKind::Identifier {
                    if is_infinity_or_nan_string(expr.text())
                        && (symbol
                            == self.get_global_symbol(
                                expr.text(),
                                SymbolFlags::VALUE,
                                None, /*diagnostic*/
                            ))
                    {
                        // Technically we resolved a global lib file here, but the decision to treat this as numeric
                        // is more predicated on the fact that the single-file resolution *didn't* resolve to a
                        // different meaning of `Infinity` or `NaN`. Transpilers handle this no problem.
                        return new_result(
                            Some(LiteralValue::Number(crate::jsnum::from_string(expr.text()))),
                            false,
                            false,
                            false,
                        );
                    }
                }
                if self.sym(symbol).flags.intersects(SymbolFlags::ENUM_MEMBER) {
                    if location.is_some() {
                        return self.evaluate_enum_member(expr, symbol, location);
                    }
                    let value_declaration = self.sym(symbol).value_declaration;
                    return self.get_enum_member_value(value_declaration);
                }
                if self.is_constant_variable(symbol) {
                    let declaration = self.sym(symbol).value_declaration;
                    if declaration.is_some()
                        && is_variable_declaration(declaration)
                        && declaration.type_().is_nil()
                        && declaration.initializer().is_some()
                        && (location.is_nil()
                            || declaration != location
                                && self.is_block_scoped_name_declared_before_use(
                                    declaration,
                                    location,
                                ))
                    {
                        let ev = self.evaluate.clone();
                        let result = ev(self, declaration.initializer(), declaration);
                        if location.is_some()
                            && get_source_file_of_node(location)
                                != get_source_file_of_node(declaration)
                        {
                            return new_result(result.value, false, true, true);
                        }
                        return new_result(
                            result.value,
                            result.is_syntactically_string,
                            result.resolved_other_files,
                            true, /*hasExternalReferences*/
                        );
                    }
                }
                new_result(None, false, false, false)
            }
            SyntaxKind::ElementAccessExpression => {
                let root = expr.expression();
                if is_entity_name_expression(root)
                    && is_string_literal_like(expr.argument_expression())
                {
                    let root_symbol = self.resolve_entity_name(
                        root,
                        SymbolFlags::VALUE,
                        true, /*ignoreErrors*/
                        false,
                        Node::NIL,
                    );
                    if root_symbol.is_some()
                        && self.sym(root_symbol).flags.intersects(SymbolFlags::ENUM)
                    {
                        let name = expr.argument_expression().text();
                        let exports = self.sym(root_symbol).exports;
                        let member = self.symbols.get(exports, name);
                        if member.is_some() {
                            if location.is_some() {
                                return self.evaluate_enum_member(expr, member, location);
                            }
                            let value_declaration = self.sym(member).value_declaration;
                            return self.get_enum_member_value(value_declaration);
                        }
                    }
                }
                new_result(None, false, false, false)
            }
            _ => panic!("Unhandled case in evaluateEntity"),
        }
    }

    // Go: checker/checker.go:24542 evaluateEnumMember
    pub fn evaluate_enum_member(
        &mut self,
        expr: Node,
        symbol: SymbolId,
        location: Node,
    ) -> EvaluatorResult {
        let declaration = self.sym(symbol).value_declaration;
        if declaration.is_nil() || declaration == location {
            let symbol_string = self.symbol_to_string(symbol);
            self.error(
                expr,
                diag::Property_0_is_used_before_being_assigned,
                args![symbol_string],
            );
            return new_result(None, false, false, false);
        }
        if !self.is_block_scoped_name_declared_before_use(declaration, location) {
            self.error(
                expr,
                diag::A_member_initializer_in_a_enum_declaration_cannot_reference_members_declared_after_it_including_members_defined_in_other_enums,
                vec![],
            );
            return new_result(
                Some(LiteralValue::Number(Number::new(0.0))),
                false,
                false,
                false,
            );
        }
        let value = self.get_enum_member_value(declaration);
        if location.parent() != declaration.parent() {
            return new_result(
                value.value,
                value.is_syntactically_string,
                value.resolved_other_files,
                true, /*hasExternalReferences*/
            );
        }
        value
    }

    // Go: checker/checker.go:24559 getDeclaredTypeOfAlias
    pub fn get_declared_type_of_alias(&mut self, symbol: SymbolId) -> TypeId {
        if self.declared_type_links.get(symbol).declared_type.is_nil() {
            let resolved = self.resolve_alias(symbol);
            let t = self.get_declared_type_of_symbol(resolved);
            self.declared_type_links.get(symbol).declared_type = t;
        }
        self.declared_type_links.get(symbol).declared_type
    }

    // Go: checker/checker.go:24567 getTypeFromTypeQueryNode
    pub fn get_type_from_type_query_node(&mut self, node: Node) -> TypeId {
        if self.type_node_links.get(node).resolved_type.is_nil() {
            // TypeScript 1.0 spec (April 2014): 3.6.3
            // The expression is processed as an identifier expression (section 4.3)
            // or property access expression(section 4.10),
            // the widened type(section 3.9) of which becomes the result.
            let t = self.check_expression_with_type_arguments(node);
            let widened = self.get_widened_type(t);
            let resolved = self.get_regular_type_of_literal_type(widened);
            self.type_node_links.get(node).resolved_type = resolved;
        }
        self.type_node_links.get(node).resolved_type
    }

    // Go: checker/checker.go:24580 getTypeFromArrayOrTupleTypeNode
    pub fn get_type_from_array_or_tuple_type_node(&mut self, node: Node) -> TypeId {
        if self.type_node_links.get(node).resolved_type.is_nil() {
            let target = self.get_array_or_tuple_target_type(node);
            let resolved_type;
            if target == self.empty_generic_type {
                resolved_type = self.empty_object_type;
            } else if !(node.kind() == SyntaxKind::TupleType && {
                let mut some = false;
                for e in node.elements() {
                    if self.is_variadic_tuple_element(e) {
                        some = true;
                        break;
                    }
                }
                some
            }) && self.is_deferred_type_reference_node(node, false)
            {
                if node.kind() == SyntaxKind::TupleType && node.elements().len() == 0 {
                    resolved_type = target;
                } else {
                    resolved_type = self.create_deferred_type_reference(
                        target,
                        node,
                        MapperId::NIL, /*mapper*/
                        None,          /*alias*/
                    );
                }
            } else {
                let element_types = if node.kind() == SyntaxKind::ArrayType {
                    vec![self.get_type_from_type_node(node.element_type())]
                } else {
                    let mut types = Vec::new();
                    for e in node.elements() {
                        types.push(self.get_type_from_type_node(e));
                    }
                    types
                };
                if self.ty(target).object_flags.intersects(ObjectFlags::TUPLE) {
                    resolved_type = self.create_normalized_tuple_type_ex(
                        target,
                        &element_types,
                        ObjectFlags::FROM_TYPE_NODE,
                    );
                } else {
                    resolved_type = self.create_type_reference_ex(
                        target,
                        &element_types,
                        ObjectFlags::FROM_TYPE_NODE,
                    );
                }
            }
            self.type_node_links.get(node).resolved_type = resolved_type;
        }
        self.type_node_links.get(node).resolved_type
    }
}
