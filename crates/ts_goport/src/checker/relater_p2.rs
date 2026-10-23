//! Port of Go `checker/relater.go` lines 916-1826: best-match helpers for
//! error elaboration, discriminants, variances and signature relations.

use crate::prelude::*;

impl Checker {
    // Go: checker/relater.go:909 findBestTypeForInvokable
    pub fn find_best_type_for_invokable(
        &mut self,
        source: TypeId,
        union_target: TypeId,
        kind: SignatureKind,
    ) -> TypeId {
        if !self.get_signatures_of_type(source, kind).is_empty() {
            for i in 0..self.ty(union_target).types().len() {
                let t = self.type_at(union_target, i);
                if !self.get_signatures_of_type(t, kind).is_empty() {
                    return t;
                }
            }
            return TypeId::NIL;
        }
        TypeId::NIL
    }

    // Go: checker/relater.go:916 findMostOverlappyType
    pub fn find_most_overlappy_type(&mut self, source: TypeId, union_target: TypeId) -> TypeId {
        let mut best_match = TypeId::NIL;
        if !self
            .ty(source)
            .flags
            .intersects(TypeFlags::PRIMITIVE | TypeFlags::INSTANTIABLE_PRIMITIVE)
        {
            let mut matching_count: i32 = 0;
            let types = self.ty(union_target).types_list();
            for target in types {
                if !self
                    .ty(target)
                    .flags
                    .intersects(TypeFlags::PRIMITIVE | TypeFlags::INSTANTIABLE_PRIMITIVE)
                {
                    let source_index = self.get_index_type(source);
                    let target_index = self.get_index_type(target);
                    let overlap = self.get_intersection_type(&[source_index, target_index]);
                    if self.ty(overlap).flags.intersects(TypeFlags::INDEX) {
                        // perfect overlap of keys
                        return target;
                    } else if self.is_unit_type(overlap)
                        || self.ty(overlap).flags.intersects(TypeFlags::UNION)
                    {
                        // We only want to account for literal types otherwise.
                        // If we have a union of index types, it seems likely that we
                        // needed to elaborate between two generic mapped types anyway.
                        let mut length: i32 = 1;
                        if self.ty(overlap).flags.intersects(TypeFlags::UNION) {
                            length = self
                                .ty(overlap)
                                .types()
                                .iter()
                                .filter(|&&t| self.is_unit_type(t))
                                .count() as i32;
                        }
                        if length >= matching_count {
                            best_match = target;
                            matching_count = length;
                        }
                    }
                }
            }
        }
        best_match
    }

    // Go: checker/relater.go:945 findBestTypeForObjectLiteral
    pub fn find_best_type_for_object_literal(
        &mut self,
        source: TypeId,
        union_target: TypeId,
    ) -> TypeId {
        if self
            .ty(source)
            .object_flags
            .intersects(ObjectFlags::OBJECT_LITERAL)
            && self.some_type(union_target, &mut |c: &mut Checker, t: TypeId| {
                c.is_array_like_type(t)
            })
        {
            for i in 0..self.ty(union_target).types().len() {
                let t = self.type_at(union_target, i);
                if !self.is_array_like_type(t) {
                    return t;
                }
            }
            return TypeId::NIL;
        }
        TypeId::NIL
    }

    // Go: checker/relater.go:952 shouldReportUnmatchedPropertyError
    pub fn should_report_unmatched_property_error(
        &mut self,
        source: TypeId,
        target: TypeId,
    ) -> bool {
        let type_call_signatures =
            self.get_signatures_of_structured_type(source, SignatureKind::CALL);
        let type_construct_signatures =
            self.get_signatures_of_structured_type(source, SignatureKind::CONSTRUCT);
        let type_properties = self.get_properties_of_object_type(source);
        if (!type_call_signatures.is_empty() || !type_construct_signatures.is_empty())
            && type_properties.is_empty()
        {
            if (!self
                .get_signatures_of_type(target, SignatureKind::CALL)
                .is_empty()
                && !type_call_signatures.is_empty())
                || !self
                    .get_signatures_of_type(target, SignatureKind::CONSTRUCT)
                    .is_empty()
                    && !type_construct_signatures.is_empty()
            {
                // target has similar signature kinds to source, still focus on the unmatched property
                return true;
            }
            return false;
        }
        true
    }

    // Go: checker/relater.go:967 getUnmatchedProperty
    pub fn get_unmatched_property(
        &mut self,
        source: TypeId,
        target: TypeId,
        require_optional_properties: bool,
        match_discriminant_properties: bool,
    ) -> SymbolId {
        self.get_unmatched_properties_worker(
            source,
            target,
            require_optional_properties,
            match_discriminant_properties,
            None,
        )
    }

    // Go: checker/relater.go:971 getUnmatchedProperties
    pub fn get_unmatched_properties(
        &mut self,
        source: TypeId,
        target: TypeId,
        require_optional_properties: bool,
        match_discriminant_properties: bool,
    ) -> Vec<SymbolId> {
        let mut props: Vec<SymbolId> = Vec::new();
        self.get_unmatched_properties_worker(
            source,
            target,
            require_optional_properties,
            match_discriminant_properties,
            Some(&mut props),
        );
        props
    }

    // PORT: Go `propsOut *[]*ast.Symbol` (nil or not) is `Option<&mut Vec<SymbolId>>`.
    // Go: checker/relater.go:977 getUnmatchedPropertiesWorker
    pub fn get_unmatched_properties_worker(
        &mut self,
        source: TypeId,
        target: TypeId,
        require_optional_properties: bool,
        match_discriminant_properties: bool,
        mut props_out: Option<&mut Vec<SymbolId>>,
    ) -> SymbolId {
        let properties = self.get_properties_of_type(target);
        for target_prop in properties {
            // TODO: remove this when we support static private identifier fields and find other solutions to get privateNamesAndStaticFields test to pass
            if self.is_static_private_identifier_property(target_prop) {
                continue;
            }
            let (target_prop_flags, target_prop_check_flags, target_prop_name) = {
                let s = self.sym(target_prop);
                (s.flags, s.check_flags, s.name.clone())
            };
            if require_optional_properties
                || !target_prop_flags.intersects(SymbolFlags::OPTIONAL)
                    && !target_prop_check_flags.intersects(CheckFlags::PARTIAL)
            {
                let source_prop = self.get_property_of_type_name(source, &target_prop_name);
                if source_prop.is_nil() {
                    match props_out.as_deref_mut() {
                        None => return target_prop,
                        Some(out) => out.push(target_prop),
                    }
                } else if match_discriminant_properties {
                    let target_type = self.get_type_of_symbol(target_prop);
                    if self.ty(target_type).flags.intersects(TypeFlags::UNIT) {
                        let source_type = self.get_type_of_symbol(source_prop);
                        if !(self.ty(source_type).flags.intersects(TypeFlags::ANY) || {
                            let a = self.get_regular_type_of_literal_type(source_type);
                            let b = self.get_regular_type_of_literal_type(target_type);
                            a == b
                        }) {
                            match props_out.as_deref_mut() {
                                None => return target_prop,
                                Some(out) => out.push(target_prop),
                            }
                        }
                    }
                }
            }
        }
        SymbolId::NIL
    }

    // PORT: Go package function `excludeProperties` reads symbol names, so it
    // is a `Checker` method. Go `collections.Set[string]` is `FxHashSet<String>`.
    // Go: checker/relater.go:1008 excludeProperties
    // PORT: takes the list by value and returns it unchanged when nothing is
    // excluded, instead of copying it.
    pub fn exclude_properties(
        &self,
        properties: SharedList<SymbolId>,
        excluded_properties: &FxHashSet<String>,
    ) -> SharedList<SymbolId> {
        if excluded_properties.is_empty() || properties.is_empty() {
            return properties;
        }
        let mut reduced: Vec<SymbolId> = Vec::new();
        let mut excluded = false;
        for (i, &prop) in properties.iter().enumerate() {
            if !excluded_properties.contains(self.sym(prop).name.as_str()) {
                if excluded {
                    reduced.push(prop);
                }
            } else if !excluded {
                reduced = properties[..i].to_vec();
                excluded = true;
            }
        }
        if excluded {
            return reduced.into();
        }
        properties
    }
}

// PORT: Go interface `Discriminator`. The Go implementations hold a
// `*Checker`; here the checker is passed to `matches` instead. Go `int`
// indexes are `i32`.
// Go: checker/relater.go:1199 Discriminator
pub trait Discriminator {
    /// Number of discriminant properties
    fn len(&self) -> i32;
    /// Property name of index-th discriminator
    // PERF (jsxname1): a `Name`, not a `String` copy of its text, so the
    // property lookups compare name ids.
    fn name(&self, c: &Checker, index: i32) -> Name;
    /// True if index-th discriminator matches the given type
    fn matches(&mut self, c: &mut Checker, index: i32, t: TypeId) -> bool;
}

// PORT: Go `TypeDiscriminator{c, props, isRelatedTo}`. The `c` field is
// dropped (the checker is passed to each method). `isRelatedTo` is the
// borrowed Go func param.
// Go: checker/relater.go:1030 TypeDiscriminator
pub struct TypeDiscriminator<'a> {
    pub props: Vec<SymbolId>,
    pub is_related_to: &'a mut dyn FnMut(&mut Checker, TypeId, TypeId) -> Ternary,
}

impl<'a> Discriminator for TypeDiscriminator<'a> {
    // Go: checker/relater.go:1036 TypeDiscriminator.len
    fn len(&self) -> i32 {
        self.props.len() as i32
    }

    // Go: checker/relater.go:1040 TypeDiscriminator.name
    fn name(&self, c: &Checker, index: i32) -> Name {
        c.sym(self.props[index as usize]).name.clone()
    }

    // Go: checker/relater.go:1044 TypeDiscriminator.matches
    fn matches(&mut self, c: &mut Checker, index: i32, t: TypeId) -> bool {
        let prop_type = c.get_type_of_symbol(self.props[index as usize]);
        for &s in c.ty(prop_type).distributed().iter() {
            if (self.is_related_to)(c, s, t) != Ternary::FALSE {
                return true;
            }
        }
        false
    }
}

impl Checker {
    // Keep this up-to-date with the same logic within `getApparentTypeOfContextualType`, since they should behave similarly
    // Go: checker/relater.go:1055 findMatchingDiscriminantType
    pub fn find_matching_discriminant_type(
        &mut self,
        source: TypeId,
        target: TypeId,
        is_related_to: &mut dyn FnMut(&mut Checker, TypeId, TypeId) -> Ternary,
    ) -> TypeId {
        if self.ty(target).flags.intersects(TypeFlags::UNION)
            && self
                .ty(source)
                .flags
                .intersects(TypeFlags::INTERSECTION | TypeFlags::OBJECT)
        {
            let m = self.get_matching_union_constituent_for_type(target, source);
            if m.is_some() {
                return m;
            }
            let source_properties = self.get_properties_of_type(source);
            let discriminant_properties =
                self.find_discriminant_properties(&source_properties, target);
            if !discriminant_properties.is_empty() {
                let mut discriminator = TypeDiscriminator {
                    props: discriminant_properties,
                    is_related_to,
                };
                let discriminated =
                    self.discriminate_type_by_discriminable_items(target, &mut discriminator);
                if discriminated != target {
                    return discriminated;
                }
            }
        }
        TypeId::NIL
    }

    // Go: checker/relater.go:1070 findDiscriminantProperties
    pub fn find_discriminant_properties(
        &mut self,
        source_properties: &[SymbolId],
        target: TypeId,
    ) -> Vec<SymbolId> {
        let mut result: Vec<SymbolId> = Vec::new();
        for &source_property in source_properties {
            let name = self.sym(source_property).name.clone();
            if self.is_discriminant_property_key(target, TableKey::Name(&name)) {
                result.push(source_property);
            }
        }
        result
    }

    // Go: checker/relater.go:1080 isDiscriminantProperty
    // PORT: `name` is a `&str` or a `&Name` (`TableKey`).
    pub fn is_discriminant_property<'a>(
        &mut self,
        t: TypeId,
        name: impl Into<TableKey<'a>>,
    ) -> bool {
        self.is_discriminant_property_key(t, name.into())
    }

    /// `is_discriminant_property` by `TableKey`.
    // PERF: a caller that holds the `Name` passes it, so the union property
    // lookups in each constituent compare name ids, not texts.
    pub fn is_discriminant_property_key(&mut self, t: TypeId, name: TableKey<'_>) -> bool {
        if t.is_some() && self.ty(t).flags.intersects(TypeFlags::UNION) {
            let prop = self.get_union_or_intersection_property_key(
                t, name, false, /*skipObjectFunctionPropertyAugment*/
            );
            if prop.is_some()
                && self
                    .sym_check_flags(prop)
                    .intersects(CheckFlags::SYNTHETIC_PROPERTY)
            {
                if !self
                    .sym_check_flags(prop)
                    .intersects(CheckFlags::IS_DISCRIMINANT_COMPUTED)
                {
                    self.sym_mut(prop).check_flags |= CheckFlags::IS_DISCRIMINANT_COMPUTED;
                    if self
                        .sym_check_flags(prop)
                        .contains(CheckFlags::NON_UNIFORM_AND_LITERAL)
                        && {
                            let prop_type = self.get_type_of_symbol(prop);
                            !self.is_generic_type(prop_type)
                        }
                    {
                        self.sym_mut(prop).check_flags |= CheckFlags::IS_DISCRIMINANT;
                    }
                }
                return self
                    .sym_check_flags(prop)
                    .intersects(CheckFlags::IS_DISCRIMINANT);
            }
        }
        false
    }

    // Go: checker/relater.go:1096 getMatchingUnionConstituentForType
    pub fn get_matching_union_constituent_for_type(
        &mut self,
        union_type: TypeId,
        t: TypeId,
    ) -> TypeId {
        let key_property_name = self.get_key_property_name(union_type);
        if key_property_name.is_empty() {
            return TypeId::NIL;
        }
        let prop_type = self.get_type_of_property_of_type(t, &key_property_name);
        if prop_type.is_nil() {
            return TypeId::NIL;
        }
        self.get_constituent_type_for_key_type(union_type, prop_type)
    }

    // Return the name of a discriminant property for which it was possible and feasible to construct a map of
    // constituent types keyed by the literal types of the property by that name in each constituent type. Return
    // an empty string if no such discriminant property exists.
    // Go: checker/relater.go:1111 getKeyPropertyName
    pub fn get_key_property_name(&mut self, t: TypeId) -> String {
        if self.ty(t).as_union_type().key_property_name.is_empty() {
            let (key_property_name, constituent_map) = self.compute_key_property_name_and_map(t);
            let u = self.ty_mut(t).as_union_type_mut();
            u.key_property_name = key_property_name;
            u.constituent_map = constituent_map;
        }
        let u = self.ty(t).as_union_type();
        if u.key_property_name == INTERNAL_SYMBOL_NAME_MISSING {
            return String::new();
        }
        u.key_property_name.clone()
    }

    // Given a union type for which getKeyPropertyName returned a non-empty string, return the constituent
    // that corresponds to the given key type for that property name.
    // Go: checker/relater.go:1124 getConstituentTypeForKeyType
    pub fn get_constituent_type_for_key_type(&mut self, t: TypeId, key_type: TypeId) -> TypeId {
        let key = self.get_regular_type_of_literal_type(key_type);
        let result = self
            .ty(t)
            .as_union_type()
            .constituent_map
            .as_ref()
            .and_then(|m| m.get(&key).copied())
            .unwrap_or(TypeId::NIL);
        if result != self.unknown_type {
            return result;
        }
        TypeId::NIL
    }

    // Go: checker/relater.go:1132 computeKeyPropertyNameAndMap
    pub fn compute_key_property_name_and_map(
        &mut self,
        t: TypeId,
    ) -> (String, Option<FxHashMap<TypeId, TypeId>>) {
        let types = self.ty(t).types_list();
        if types.len() < 10
            || self
                .ty(t)
                .object_flags
                .intersects(ObjectFlags::PRIMITIVE_UNION)
            || types
                .iter()
                .filter(|&&t| self.is_object_or_instantiable_non_primitive(t))
                .count()
                < 10
        {
            return (INTERNAL_SYMBOL_NAME_MISSING.to_string(), None);
        }
        let key_property_name = self.get_key_property_candidate_name(&types);
        if key_property_name.is_empty() {
            return (INTERNAL_SYMBOL_NAME_MISSING.to_string(), None);
        }
        let map_by_key_property = self.map_types_by_key_property(&types, &key_property_name);
        if map_by_key_property.is_none() {
            return (INTERNAL_SYMBOL_NAME_MISSING.to_string(), None);
        }
        (key_property_name, map_by_key_property)
    }

    // PORT: Go package function; reads type data, so it is a `Checker` method.
    // Go: checker/relater.go:1148 isObjectOrInstantiableNonPrimitive
    pub fn is_object_or_instantiable_non_primitive(&self, t: TypeId) -> bool {
        self.ty(t)
            .flags
            .intersects(TypeFlags::OBJECT | TypeFlags::INSTANTIABLE_NON_PRIMITIVE)
    }

    // Go: checker/relater.go:1152 getKeyPropertyCandidateName
    pub fn get_key_property_candidate_name(&mut self, types: &[TypeId]) -> String {
        for &t in types {
            if self
                .ty(t)
                .flags
                .intersects(TypeFlags::OBJECT | TypeFlags::INSTANTIABLE_NON_PRIMITIVE)
            {
                for p in self.get_properties_of_type(t) {
                    let p_type = self.get_type_of_symbol(p);
                    if self.is_unit_type(p_type) {
                        return self.sym(p).name.to_string();
                    }
                }
            }
        }
        String::new()
    }

    // Given a set of constituent types and a property name, create and return a map keyed by the literal
    // types of the property by that name in each constituent type. No map is returned if some key property
    // has a non-literal type or if less than 10 or less than 50% of the constituents have a unique key.
    // Entries with duplicate keys have unknownType as the value.
    // Go: checker/relater.go:1169 mapTypesByKeyProperty
    pub fn map_types_by_key_property(
        &mut self,
        types: &[TypeId],
        key_property_name: &str,
    ) -> Option<FxHashMap<TypeId, TypeId>> {
        let mut types_by_key: FxHashMap<TypeId, TypeId> = FxHashMap::default();
        let mut count: usize = 0;
        for &t in types {
            if self.ty(t).flags.intersects(
                TypeFlags::OBJECT | TypeFlags::INTERSECTION | TypeFlags::INSTANTIABLE_NON_PRIMITIVE,
            ) {
                let discriminant = self.get_type_of_property_of_type(t, key_property_name);
                if discriminant.is_nil() || !self.is_literal_type(discriminant) {
                    return None;
                }
                let mut duplicate = false;
                for &d in self.ty(discriminant).distributed().iter() {
                    let key = self.get_regular_type_of_literal_type(d);
                    let existing = types_by_key.get(&key).copied().unwrap_or(TypeId::NIL);
                    if existing.is_nil() {
                        types_by_key.insert(key, t);
                    } else if existing != self.unknown_type {
                        types_by_key.insert(key, self.unknown_type);
                        duplicate = true;
                    }
                }
                if !duplicate {
                    count += 1;
                }
            }
        }
        if count >= 10 && count * 2 >= types.len() {
            return Some(types_by_key);
        }
        None
    }

    // Go: checker/relater.go:1205 discriminateTypeByDiscriminableItems
    pub fn discriminate_type_by_discriminable_items(
        &mut self,
        target: TypeId,
        discriminator: &mut dyn Discriminator,
    ) -> TypeId {
        let types = self.ty(target).types_list();
        let mut include: Vec<Ternary> = vec![Ternary::FALSE; types.len()];
        for (i, &t) in types.iter().enumerate() {
            if !self.ty(t).flags.intersects(TypeFlags::PRIMITIVE) && {
                let reduced = self.get_reduced_type(t);
                !self.ty(reduced).flags.intersects(TypeFlags::NEVER)
            } {
                include[i] = Ternary::TRUE;
            }
        }
        for n in 0..discriminator.len() {
            // If the remaining target types include at least one with a matching discriminant, eliminate those that
            // have non-matching discriminants. This ensures that we ignore erroneous discriminators and gradually
            // refine the target set without eliminating every constituent (which would lead to `never`).
            let mut matched = false;
            // PERF (jsxname1): Go reads `discriminator.name(n)` for each
            // constituent. The name does not change in this loop, so it is
            // read once.
            let name = discriminator.name(self, n);
            for i in 0..types.len() {
                if include[i] != Ternary::FALSE {
                    let target_type =
                        self.get_type_of_property_or_index_signature_of_type(types[i], &name);
                    if target_type.is_some() {
                        if discriminator.matches(self, n, target_type) {
                            matched = true;
                        } else {
                            include[i] = Ternary::MAYBE;
                        }
                    }
                }
            }
            // Turn each Ternary.Maybe into Ternary.False if there was a match. Otherwise, revert to Ternary.True.
            for i in 0..types.len() {
                if include[i] == Ternary::MAYBE {
                    if matched {
                        include[i] = Ternary::FALSE;
                    } else {
                        include[i] = Ternary::TRUE;
                    }
                }
            }
        }
        if include.contains(&Ternary::FALSE) {
            let mut filtered_types: Vec<TypeId> = Vec::new();
            for (i, &t) in types.iter().enumerate() {
                if include[i] == Ternary::TRUE {
                    filtered_types.push(t);
                }
            }
            let filtered =
                self.get_union_type_ex(&filtered_types, UnionReduction::NONE, None, TypeId::NIL);
            if !self.ty(filtered).flags.intersects(TypeFlags::NEVER) {
                return filtered;
            }
        }
        target
    }

    // Go: checker/relater.go:1256 filterPrimitivesIfContainsNonPrimitive
    pub fn filter_primitives_if_contains_non_primitive(&mut self, union_type: TypeId) -> TypeId {
        if self.maybe_type_of_kind(union_type, TypeFlags::NON_PRIMITIVE) {
            let result = self.filter_type(union_type, &mut |c: &mut Checker, t: TypeId| {
                c.is_non_primitive_type(t)
            });
            if !self.ty(result).flags.intersects(TypeFlags::NEVER) {
                return result;
            }
        }
        union_type
    }

    // PORT: Go package function; reads type data, so it is a `Checker` method.
    // Go: checker/relater.go:1266 isNonPrimitiveType
    pub fn is_non_primitive_type(&self, t: TypeId) -> bool {
        !self.ty(t).flags.intersects(TypeFlags::PRIMITIVE)
    }

    // Go: checker/relater.go:1270 getTypeNamesForErrorDisplay
    pub fn get_type_names_for_error_display(
        &mut self,
        left: TypeId,
        right: TypeId,
    ) -> (String, String) {
        let mut left_str: String;
        let left_symbol = self.ty(left).symbol;
        if self.symbol_value_declaration_is_context_sensitive(left_symbol) {
            let decl = self.sym(left_symbol).value_declaration;
            left_str = self.type_to_string_enclosing(left, decl);
        } else {
            left_str = self.type_to_string_exported(left);
        }
        let mut right_str: String;
        let right_symbol = self.ty(right).symbol;
        if self.symbol_value_declaration_is_context_sensitive(right_symbol) {
            let decl = self.sym(right_symbol).value_declaration;
            right_str = self.type_to_string_enclosing(right, decl);
        } else {
            right_str = self.type_to_string_exported(right);
        }
        if left_str == right_str {
            left_str = self.get_type_name_for_error_display(left);
            right_str = self.get_type_name_for_error_display(right);
        }
        (left_str, right_str)
    }

    // Go: checker/relater.go:1290 getTypeNameForErrorDisplay
    pub fn get_type_name_for_error_display(&mut self, t: TypeId) -> String {
        self.type_to_string_ex(
            t,
            Node::NIL, /*enclosingDeclaration*/
            TypeFormatFlags::USE_FULLY_QUALIFIED_TYPE,
            None,
        )
    }

    // Go: checker/relater.go:1294 symbolValueDeclarationIsContextSensitive
    pub fn symbol_value_declaration_is_context_sensitive(&mut self, symbol: SymbolId) -> bool {
        if symbol.is_nil() {
            return false;
        }
        let value_declaration = self.sym(symbol).value_declaration;
        value_declaration.is_some()
            && is_expression(value_declaration)
            && !self.is_context_sensitive(value_declaration)
    }

    // Go: checker/relater.go:1298 typeCouldHaveTopLevelSingletonTypes
    pub fn type_could_have_top_level_singleton_types(&mut self, t: TypeId) -> bool {
        // Okay, yes, 'boolean' is a union of 'true | false', but that's not useful
        // in error reporting scenarios. If you need to use this function but that detail matters,
        // feel free to add a flag.
        if self.ty(t).flags.intersects(TypeFlags::BOOLEAN) {
            return false;
        }
        if self
            .ty(t)
            .flags
            .intersects(TypeFlags::UNION_OR_INTERSECTION)
        {
            for i in 0..self.ty(t).types().len() {
                let s = self.type_at(t, i);
                if self.type_could_have_top_level_singleton_types(s) {
                    return true;
                }
            }
            return false;
        }
        if self.ty(t).flags.intersects(TypeFlags::INSTANTIABLE) {
            let constraint = self.get_constraint_of_type(t);
            if constraint.is_some() && constraint != t {
                return self.type_could_have_top_level_singleton_types(constraint);
            }
        }
        self.is_unit_type(t)
            || self.ty(t).flags.intersects(TypeFlags::TEMPLATE_LITERAL)
            || self.ty(t).flags.intersects(TypeFlags::STRING_MAPPING)
    }

    // Go: checker/relater.go:1317 getVariances
    pub fn get_variances(&mut self, t: TypeId) -> SharedList<VarianceFlags> {
        // Arrays and tuples are known to be covariant, no need to spend time computing this.
        if t == self.global_array_type
            || t == self.global_readonly_array_type
            || self.ty(t).object_flags.intersects(ObjectFlags::TUPLE)
        {
            return self.array_variances.clone();
        }
        let symbol = self.ty(t).symbol;
        // PORT: the cached result is returned before the type parameters are
        // copied; `get_variances_worker` would return the same list.
        if self.variance_links.has(symbol) {
            return self.variance_links.get(symbol).variances.clone();
        }
        let type_parameters = self.ty(t).as_interface_type().type_parameters().to_vec();
        self.get_variances_worker(symbol, &type_parameters)
    }

    // Go: checker/relater.go:1325 getAliasVariances
    pub fn get_alias_variances(&mut self, symbol: SymbolId) -> SharedList<VarianceFlags> {
        let type_parameters = self.type_alias_links.get(symbol).type_parameters.clone();
        self.get_variances_worker(symbol, &type_parameters)
    }

    // Return an array containing the variance of each type parameter. The variance is effectively
    // a digest of the type comparisons that occur for each type argument when instantiations of the
    // generic type are structurally compared. We infer the variance information by comparing
    // instantiations of the generic type for type arguments with known relations. The function
    // returns an empty slice when invoked recursively for the given generic type.
    //
    // PORT: Go distinguishes a nil `links.variances` (not computed) from an
    // empty slice (circular, or no type parameters). `VarianceLinks` holds a
    // `SharedList`, and `varianceLinks` is only used here, so "the link record
    // exists" stands for "variances != nil": the record is created only where
    // Go assigns `links.variances`. Go `len(links.variances)` is the length of
    // the record's list, or 0 without a record.
    // Go: checker/relater.go:1334 getVariancesWorker
    pub fn get_variances_worker(
        &mut self,
        symbol: SymbolId,
        type_parameters: &[TypeId],
    ) -> SharedList<VarianceFlags> {
        if !self.variance_links.has(symbol) {
            // Go defers the end of the event to the function return and adds
            // the final variances to its args first (see the end of this block).
            let mut trace = self.tracer.map(|tr| {
                let id = self.get_declared_type_of_symbol(symbol);
                tr.push(
                    crate::tracing::Phase::CheckTypes,
                    "getVariancesWorker",
                    vec![("arity", type_parameters.len().into()), ("id", id.into())],
                    true,
                )
            });
            if let Some(stack_index) = self.get_variance_stack_index(symbol) {
                // We've detected a circularity. Since we may compute different variances depending on where
                // we enter a circularity, we find the generic type with the "smallest" symbol in the circular
                // region of the variance stack and restart the computation from there if necessary. This
                // ensures stable results for circular generic types.
                let mut min_index = stack_index;
                for i in stack_index + 1..self.variance_stack.len() {
                    let (s, min) = (
                        self.variance_stack[i].symbol,
                        self.variance_stack[min_index].symbol,
                    );
                    if self.compare_symbols(s, min) < 0 {
                        min_index = i;
                    }
                }
                if min_index > stack_index {
                    let save_variance_stack = std::mem::take(&mut self.variance_stack);
                    let entry = &save_variance_stack[min_index];
                    let (min_symbol, min_type_parameters) =
                        (entry.symbol, entry.type_parameters.clone());
                    self.get_variances_worker(min_symbol, &min_type_parameters);
                    self.variance_stack = save_variance_stack;
                }
                // Store an empty slice to mark that we can't compute variances for this type. We treat type
                // parameters as co-variant in this case.
                if !self.variance_links.has(symbol) {
                    self.variance_links.get(symbol).variances = SharedList::default();
                }
            } else {
                let save_resolution_start = self.resolution_start;
                if self.variance_stack.is_empty() {
                    self.resolution_start = self.type_resolutions.len() as i32;
                }
                self.variance_stack.push(VarianceStackEntry {
                    symbol,
                    type_parameters: type_parameters.to_vec(),
                });
                let mut variances: Vec<VarianceFlags> =
                    vec![VarianceFlags::default(); type_parameters.len()];
                for (i, &tp) in type_parameters.iter().enumerate() {
                    let modifiers = self.get_type_parameter_modifiers(tp);
                    let mut variance: VarianceFlags;
                    if modifiers.intersects(ModifierFlags::OUT) {
                        if modifiers.intersects(ModifierFlags::IN) {
                            variance = VarianceFlags::INVARIANT;
                        } else {
                            variance = VarianceFlags::COVARIANT;
                        }
                    } else if modifiers.intersects(ModifierFlags::IN) {
                        variance = VarianceFlags::CONTRAVARIANT;
                    } else {
                        let save_reliability_flags = self.reliability_flags;
                        self.reliability_flags = RelationComparisonResult::NONE;
                        // We first compare instantiations where the type parameter is replaced with
                        // marker types that have a known subtype relationship. From this we can infer
                        // invariance, covariance, contravariance or bivariance.
                        let marker_super_type = self.marker_super_type;
                        let marker_sub_type = self.marker_sub_type;
                        let type_with_super =
                            self.create_marker_type(symbol, tp, marker_super_type);
                        let type_with_sub = self.create_marker_type(symbol, tp, marker_sub_type);
                        variance =
                            (if self.is_type_assignable_to(type_with_sub, type_with_super) {
                                VarianceFlags::COVARIANT
                            } else {
                                VarianceFlags::default()
                            }) | (if self.is_type_assignable_to(type_with_super, type_with_sub) {
                                VarianceFlags::CONTRAVARIANT
                            } else {
                                VarianceFlags::default()
                            });
                        // If the instantiations appear to be related bivariantly it may be because the
                        // type parameter is independent (i.e. it isn't witnessed anywhere in the generic
                        // type). To determine this we compare instantiations where the type parameter is
                        // replaced with marker types that are known to be unrelated.
                        if variance == VarianceFlags::BIVARIANT && {
                            let marker_other_type = self.marker_other_type;
                            let type_with_other =
                                self.create_marker_type(symbol, tp, marker_other_type);
                            self.is_type_assignable_to(type_with_other, type_with_super)
                        } {
                            variance = VarianceFlags::INDEPENDENT;
                        }
                        if self
                            .reliability_flags
                            .intersects(RelationComparisonResult::REPORTS_UNMEASURABLE)
                        {
                            variance |= VarianceFlags::UNMEASURABLE;
                        }
                        if self
                            .reliability_flags
                            .intersects(RelationComparisonResult::REPORTS_UNRELIABLE)
                        {
                            variance |= VarianceFlags::UNRELIABLE;
                        }
                        self.reliability_flags = save_reliability_flags;
                    }
                    // If variance computation was restarted due to a circularity we may have already
                    // computed variances for this generic type. If so, we exit early.
                    if self
                        .variance_links
                        .try_get(symbol)
                        .is_some_and(|links| !links.variances.is_empty())
                    {
                        break;
                    }
                    variances[i] = variance;
                }
                // Store the results unless a restarted computation has already stored them.
                if self
                    .variance_links
                    .try_get(symbol)
                    .is_none_or(|links| links.variances.is_empty())
                {
                    self.variance_links.get(symbol).variances = variances.into();
                }
                self.variance_stack.pop();
                if self.variance_stack.is_empty() {
                    self.resolution_start = save_resolution_start;
                }
            }
            if let Some(args) = trace.as_mut().and_then(crate::tracing::Pop::args_mut) {
                let formatted: Vec<String> = self
                    .variance_links
                    .get(symbol)
                    .variances
                    .iter()
                    .map(ToString::to_string)
                    .collect();
                args.push(("variances", formatted.into()));
            }
            drop(trace);
        }
        self.variance_links.get(symbol).variances.clone()
    }

    // PORT: Go returns -1 when the symbol is not on the stack; that is `None`.
    // Go: checker/relater.go:1437 getVarianceStackIndex
    pub fn get_variance_stack_index(&self, symbol: SymbolId) -> Option<usize> {
        self.variance_stack
            .iter()
            .position(|entry| entry.symbol == symbol)
    }

    // Go: checker/relater.go:1446 createMarkerType
    pub fn create_marker_type(
        &mut self,
        symbol: SymbolId,
        source: TypeId,
        target: TypeId,
    ) -> TypeId {
        let mapper = self.new_simple_type_mapper(source, target);
        let t = self.get_declared_type_of_symbol(symbol);
        if self.is_error_type(t) {
            return t;
        }
        let result: TypeId;
        if self.sym_flags(symbol).intersects(SymbolFlags::TYPE_ALIAS) {
            let type_parameters = self.type_alias_links.get(symbol).type_parameters.clone();
            let type_arguments = self.instantiate_types(&type_parameters, mapper);
            result = self.get_type_alias_instantiation(symbol, &type_arguments, None);
        } else {
            let type_parameters = self.ty(t).as_interface_type().type_parameters().to_vec();
            let type_arguments = self.instantiate_types(&type_parameters, mapper);
            result = self.create_type_reference(t, &type_arguments);
        }
        self.marker_types.insert(result);
        result
    }

    // Go: checker/relater.go:1462 isMarkerType
    pub fn is_marker_type(&self, t: TypeId) -> bool {
        self.marker_types.contains(&t)
    }

    // Go: checker/relater.go:1466 getTypeParameterModifiers
    pub fn get_type_parameter_modifiers(&mut self, tp: TypeId) -> ModifierFlags {
        let mut flags = ModifierFlags::default();
        let symbol = self.ty(tp).symbol;
        if symbol.is_some() {
            let declarations = self.sym(symbol).declarations.clone();
            for &d in declarations.iter() {
                flags |= d.modifier_flags();
            }
        }
        flags & (ModifierFlags::IN | ModifierFlags::OUT | ModifierFlags::CONST)
    }

    // Return true if the given type reference has a 'void' type argument for a covariant type parameter.
    // See comment at call in recursiveTypeRelatedTo for when this case matters.
    // Go: checker/relater.go:1478 hasCovariantVoidArgument
    pub fn has_covariant_void_argument(
        &self,
        type_arguments: &[TypeId],
        variances: &[VarianceFlags],
    ) -> bool {
        for (i, &v) in variances.iter().enumerate() {
            if v & VarianceFlags::VARIANCE_MASK == VarianceFlags::COVARIANT
                && self.ty(type_arguments[i]).flags.intersects(TypeFlags::VOID)
            {
                return true;
            }
        }
        false
    }

    // Go: checker/relater.go:1487 isSignatureAssignableTo
    pub fn is_signature_assignable_to(
        &mut self,
        source: SignatureId,
        target: SignatureId,
        ignore_return_types: bool,
    ) -> bool {
        let compare_types = self.compare_types_assignable.clone();
        self.compare_signatures_related(
            source,
            target,
            if ignore_return_types {
                SignatureCheckMode::IGNORE_RETURN_TYPES
            } else {
                SignatureCheckMode::NONE
            },
            false, /*reportErrors*/
            None,  /*errorReporter*/
            compare_types,
            MapperId::NIL, /*reportUnreliableMarkers*/
        ) != Ternary::FALSE
    }

    // PORT: a nil Go `ErrorReporter` is `None`. Go only calls it when
    // `reportErrors` is true, and then it is never nil.
    // Go: checker/relater.go:1491 compareSignaturesRelated
    pub fn compare_signatures_related(
        &mut self,
        source: SignatureId,
        target: SignatureId,
        check_mode: SignatureCheckMode,
        report_errors: bool,
        mut error_reporter: Option<ErrorReporter>,
        compare_types: TypeComparer,
        report_unreliable_markers: MapperId,
    ) -> Ternary {
        let mut source = source;
        let mut target = target;
        if source == target {
            return Ternary::TRUE;
        }
        if !(check_mode.intersects(SignatureCheckMode::STRICT_TOP_SIGNATURE)
            && self.is_top_signature(source))
            && self.is_top_signature(target)
        {
            return Ternary::TRUE;
        }
        if check_mode.intersects(SignatureCheckMode::STRICT_TOP_SIGNATURE)
            && self.is_top_signature(source)
            && !self.is_top_signature(target)
        {
            return Ternary::FALSE;
        }
        let target_count = self.get_parameter_count(target);
        let mut source_has_more_parameters = false;
        if !self.has_effective_rest_parameter(target) {
            if check_mode.intersects(SignatureCheckMode::STRICT_ARITY) {
                source_has_more_parameters = self.has_effective_rest_parameter(source)
                    || self.get_parameter_count(source) > target_count;
            } else {
                source_has_more_parameters = self.get_min_argument_count(source) > target_count;
            }
        }
        if source_has_more_parameters {
            if report_errors && !check_mode.intersects(SignatureCheckMode::STRICT_ARITY) {
                // the second condition should be redundant, because there is no error reporting when comparing signatures by strict arity
                // since it is only done for subtype reduction
                let min_argument_count = self.get_min_argument_count(source);
                let reporter = error_reporter.as_deref_mut().expect("call of nil func");
                reporter(
                    self,
                    diag::Target_signature_provides_too_few_arguments_Expected_0_or_more_but_got_1,
                    args![min_argument_count, target_count],
                );
            }
            return Ternary::FALSE;
        }
        // PORT: Go `core.Same` compares slice identity (same length and same
        // backing array), not the elements. See `same_signature_type_parameters`.
        if !self.sig(source).type_parameters.is_empty()
            && !self.same_signature_type_parameters(source, target)
        {
            target = self.get_canonical_signature(target);
            source = self.instantiate_signature_in_context_of(
                source,
                target,
                InferenceContextId::NIL, /*inferenceContext*/
                Some(compare_types.clone()),
            );
        }
        let source_count = self.get_parameter_count(source);
        let source_rest_type = self.get_non_array_rest_type(source);
        let target_rest_type = self.get_non_array_rest_type(target);
        if source_rest_type.is_some() || target_rest_type.is_some() {
            self.instantiate_type(
                if source_rest_type.is_some() {
                    source_rest_type
                } else {
                    target_rest_type
                },
                report_unreliable_markers,
            );
        }
        let mut kind = SyntaxKind::Unknown;
        let target_declaration = self.sig(target).declaration;
        if target_declaration.is_some() {
            kind = target_declaration.kind();
        }
        let strict_variance = !check_mode.intersects(SignatureCheckMode::CALLBACK)
            && self.strict_function_types
            && kind != SyntaxKind::MethodDeclaration
            && kind != SyntaxKind::MethodSignature
            && kind != SyntaxKind::Constructor;
        let mut result = Ternary::TRUE;
        let source_this_type = self.get_this_type_of_signature(source);
        if source_this_type.is_some() && source_this_type != self.void_type {
            let target_this_type = self.get_this_type_of_signature(target);
            if target_this_type.is_some() {
                // void sources are assignable to anything.
                let mut related = Ternary::FALSE;
                if !strict_variance {
                    related = compare_types(
                        self,
                        source_this_type,
                        target_this_type,
                        false, /*reportErrors*/
                    );
                }
                if related == Ternary::FALSE {
                    related =
                        compare_types(self, target_this_type, source_this_type, report_errors);
                }
                if related == Ternary::FALSE {
                    if report_errors {
                        let reporter = error_reporter.as_deref_mut().expect("call of nil func");
                        reporter(
                            self,
                            diag::The_this_types_of_each_signature_are_incompatible,
                            args![],
                        );
                    }
                    return Ternary::FALSE;
                }
                result &= related;
            }
        }
        let param_count: i32 = if source_rest_type.is_some() || target_rest_type.is_some() {
            source_count.min(target_count)
        } else {
            source_count.max(target_count)
        };
        let rest_index: i32 = if source_rest_type.is_some() || target_rest_type.is_some() {
            param_count - 1
        } else {
            -1
        };
        for i in 0..param_count {
            let source_type = if i == rest_index {
                self.get_rest_or_any_type_at_position(source, i)
            } else {
                self.try_get_type_at_position(source, i)
            };
            let target_type = if i == rest_index {
                self.get_rest_or_any_type_at_position(target, i)
            } else {
                self.try_get_type_at_position(target, i)
            };
            if source_type.is_some()
                && target_type.is_some()
                && (source_type != target_type
                    || check_mode.intersects(SignatureCheckMode::STRICT_ARITY))
            {
                // In order to ensure that any generic type Foo<T> is at least co-variant with respect to T no matter
                // how Foo uses T, we need to relate parameters bi-variantly (given that parameters are input positions,
                // they naturally relate only contra-variantly). However, if the source and target parameters both have
                // function types with a single call signature, we know we are relating two callback parameters. In
                // that case it is sufficient to only relate the parameters of the signatures co-variantly because,
                // similar to return values, callback parameters are output positions. This means that a Promise<T>,
                // where T is used only in callback parameter positions, will be co-variant (as opposed to bi-variant)
                // with respect to T.
                let mut source_sig = SignatureId::NIL;
                let mut target_sig = SignatureId::NIL;
                if !check_mode.intersects(SignatureCheckMode::CALLBACK)
                    && !self.is_instantiated_generic_parameter(source, i)
                {
                    let non_nullable = self.get_non_nullable_type(source_type);
                    source_sig = self.get_single_call_signature(non_nullable);
                }
                if !check_mode.intersects(SignatureCheckMode::CALLBACK)
                    && !self.is_instantiated_generic_parameter(target, i)
                {
                    let non_nullable = self.get_non_nullable_type(target_type);
                    target_sig = self.get_single_call_signature(non_nullable);
                }
                let callbacks = source_sig.is_some()
                    && target_sig.is_some()
                    && self.get_type_predicate_of_signature(source_sig).is_nil()
                    && self.get_type_predicate_of_signature(target_sig).is_nil()
                    && self.get_type_facts(source_type, TypeFacts::IS_UNDEFINED_OR_NULL)
                        == self.get_type_facts(target_type, TypeFacts::IS_UNDEFINED_OR_NULL);
                let mut related = Ternary::FALSE;
                if callbacks {
                    related = self.compare_signatures_related(
                        target_sig,
                        source_sig,
                        (check_mode & SignatureCheckMode::STRICT_ARITY)
                            | if strict_variance {
                                SignatureCheckMode::STRICT_CALLBACK
                            } else {
                                SignatureCheckMode::BIVARIANT_CALLBACK
                            },
                        report_errors,
                        error_reporter.as_deref_mut().map(|r| r as ErrorReporter),
                        compare_types.clone(),
                        report_unreliable_markers,
                    );
                } else {
                    if !check_mode.intersects(SignatureCheckMode::CALLBACK) && !strict_variance {
                        related = compare_types(
                            self,
                            source_type,
                            target_type,
                            false, /*reportErrors*/
                        );
                    }
                    if related == Ternary::FALSE {
                        related = compare_types(self, target_type, source_type, report_errors);
                    }
                }
                // With strict arity, (x: number | undefined) => void is a subtype of (x?: number | undefined) => void
                if related != Ternary::FALSE
                    && check_mode.intersects(SignatureCheckMode::STRICT_ARITY)
                    && i >= self.get_min_argument_count(source)
                    && i < self.get_min_argument_count(target)
                    && compare_types(self, source_type, target_type, false /*reportErrors*/)
                        != Ternary::FALSE
                {
                    related = Ternary::FALSE;
                }
                if related == Ternary::FALSE {
                    if report_errors {
                        let source_name = self.get_parameter_name_at_position(source, i);
                        let target_name = self.get_parameter_name_at_position(target, i);
                        let reporter = error_reporter.as_deref_mut().expect("call of nil func");
                        reporter(
                            self,
                            diag::Types_of_parameters_0_and_1_are_incompatible,
                            args![source_name, target_name],
                        );
                    }
                    return Ternary::FALSE;
                }
                result &= related;
            }
        }
        if !check_mode.intersects(SignatureCheckMode::IGNORE_RETURN_TYPES) {
            // If a signature resolution is already in-flight, skip issuing a circularity error
            // here and just use the `any` type directly
            let target_return_type = self.get_non_circular_return_type_of_signature(target);
            if target_return_type == self.void_type || target_return_type == self.any_type {
                return result;
            }
            let source_return_type = self.get_non_circular_return_type_of_signature(source);
            // The following block preserves behavior forbidding boolean returning functions from being assignable to type guard returning functions
            let target_type_predicate = self.get_type_predicate_of_signature(target);
            if target_type_predicate.is_some() {
                let source_type_predicate = self.get_type_predicate_of_signature(source);
                if source_type_predicate.is_some() {
                    result &= self.compare_type_predicate_related_to(
                        source_type_predicate,
                        target_type_predicate,
                        report_errors,
                        error_reporter.as_deref_mut().map(|r| r as ErrorReporter),
                        compare_types.clone(),
                    );
                } else if self.pred(target_type_predicate).kind == TypePredicateKind::IDENTIFIER
                    || self.pred(target_type_predicate).kind == TypePredicateKind::THIS
                {
                    if report_errors {
                        let source_str = self.signature_to_string(source);
                        let reporter = error_reporter.as_deref_mut().expect("call of nil func");
                        reporter(
                            self,
                            diag::Signature_0_must_be_a_type_predicate,
                            args![source_str],
                        );
                    }
                    return Ternary::FALSE;
                }
            } else {
                // When relating callback signatures, we still need to relate return types bi-variantly as otherwise
                // the containing type wouldn't be co-variant. For example, interface Foo<T> { add(cb: () => T): void }
                // wouldn't be co-variant for T without this rule.
                let mut related = Ternary::FALSE;
                if check_mode.intersects(SignatureCheckMode::BIVARIANT_CALLBACK) {
                    related = compare_types(
                        self,
                        target_return_type,
                        source_return_type,
                        false, /*reportErrors*/
                    );
                }
                if related == Ternary::FALSE {
                    related =
                        compare_types(self, source_return_type, target_return_type, report_errors);
                }
                result &= related;
                if result == Ternary::FALSE && report_errors {
                    // The errors reported here serve as markers that trigger error chain reduction in the (*Relater).reportError
                    // method. The markers are elided in the final diagnostic chain and never actually reported.
                    let message: &'static crate::diagnostics::Message;
                    let source_is_construct =
                        self.sig(source).flags.intersects(SignatureFlags::CONSTRUCT);
                    if self.sig(source).parameters.is_empty()
                        && self.sig(target).parameters.is_empty()
                    {
                        message = if source_is_construct {
                            diag::Construct_signatures_with_no_arguments_have_incompatible_return_types_0_and_1
                        } else {
                            diag::Call_signatures_with_no_arguments_have_incompatible_return_types_0_and_1
                        };
                    } else {
                        message = if source_is_construct {
                            diag::Construct_signature_return_types_0_and_1_are_incompatible
                        } else {
                            diag::Call_signature_return_types_0_and_1_are_incompatible
                        };
                    }
                    let source_str = self.type_to_string_exported(source_return_type);
                    let target_str = self.type_to_string_exported(target_return_type);
                    let reporter = error_reporter.as_deref_mut().expect("call of nil func");
                    reporter(self, message, args![source_str, target_str]);
                }
            }
        }
        result
    }

    // Go: checker/relater.go:1675 compareTypePredicateRelatedTo
    pub fn compare_type_predicate_related_to(
        &mut self,
        source: TypePredicateId,
        target: TypePredicateId,
        report_errors: bool,
        mut error_reporter: Option<ErrorReporter>,
        compare_types: TypeComparer,
    ) -> Ternary {
        let source_pred = self.pred(source).clone();
        let target_pred = self.pred(target).clone();
        if source_pred.kind != target_pred.kind {
            if report_errors {
                let reporter = error_reporter.as_deref_mut().expect("call of nil func");
                reporter(self, diag::A_this_based_type_guard_is_not_compatible_with_a_parameter_based_type_guard, args![]);
                let source_str = self.type_predicate_to_string(source);
                let target_str = self.type_predicate_to_string(target);
                reporter(
                    self,
                    diag::Type_predicate_0_is_not_assignable_to_1,
                    args![source_str, target_str],
                );
            }
            return Ternary::FALSE;
        }
        if source_pred.kind == TypePredicateKind::IDENTIFIER
            || source_pred.kind == TypePredicateKind::ASSERTS_IDENTIFIER
        {
            if source_pred.parameter_index != target_pred.parameter_index {
                if report_errors {
                    let reporter = error_reporter.as_deref_mut().expect("call of nil func");
                    reporter(
                        self,
                        diag::Parameter_0_is_not_in_the_same_position_as_parameter_1,
                        args![source_pred.parameter_name, target_pred.parameter_name],
                    );
                    let source_str = self.type_predicate_to_string(source);
                    let target_str = self.type_predicate_to_string(target);
                    reporter(
                        self,
                        diag::Type_predicate_0_is_not_assignable_to_1,
                        args![source_str, target_str],
                    );
                }
                return Ternary::FALSE;
            }
        }
        let related: Ternary = if source_pred.t == target_pred.t {
            Ternary::TRUE
        } else if source_pred.t.is_some() && target_pred.t.is_some() {
            compare_types(self, source_pred.t, target_pred.t, report_errors)
        } else {
            Ternary::FALSE
        };
        if related == Ternary::FALSE && report_errors {
            let source_str = self.type_predicate_to_string(source);
            let target_str = self.type_predicate_to_string(target);
            let reporter = error_reporter.as_deref_mut().expect("call of nil func");
            reporter(
                self,
                diag::Type_predicate_0_is_not_assignable_to_1,
                args![source_str, target_str],
            );
        }
        related
    }

    // Returns true if `s` is `(...args: A) => R` where `A` is `any`, `any[]`, `never`, or `never[]`, and `R` is `any` or `unknown`.
    // Go: checker/relater.go:1708 isTopSignature
    pub fn is_top_signature(&mut self, s: SignatureId) -> bool {
        let this_parameter = self.sig(s).this_parameter;
        if self.sig(s).type_parameters.is_empty()
            && (this_parameter.is_nil() || {
                let this_type = self.get_type_of_parameter(this_parameter);
                self.is_type_any(this_type)
            })
            && self.sig(s).parameters.len() == 1
            && self.signature_has_rest_parameter(s)
        {
            let first_parameter = self.sig(s).parameters[0];
            let param_type = self.get_type_of_parameter(first_parameter);
            let rest_type = if self.is_array_type(param_type) {
                self.type_arguments_of(param_type)[0]
            } else {
                param_type
            };
            return self
                .ty(rest_type)
                .flags
                .intersects(TypeFlags::ANY | TypeFlags::NEVER)
                && {
                    let return_type = self.get_return_type_of_signature(s);
                    self.ty(return_type)
                        .flags
                        .intersects(TypeFlags::ANY_OR_UNKNOWN)
                };
        }
        false
    }

    // Return the number of parameters in a signature. The rest parameter, if present, counts as one
    // parameter. For example, the parameter count of (x: number, y: number, ...z: string[]) is 3 and
    // the parameter count of (x: number, ...args: [number, ...string[], boolean])) is also 3. In the
    // latter example, the effective rest type is [...string[], boolean].
    // Go: checker/relater.go:1726 getParameterCount
    pub fn get_parameter_count(&mut self, signature: SignatureId) -> i32 {
        let length = self.sig(signature).parameters.len() as i32;
        if self.signature_has_rest_parameter(signature) {
            let last = self.sig(signature).parameters[(length - 1) as usize];
            let rest_type = self.get_type_of_symbol(last);
            if self.is_tuple_type(rest_type) {
                let tuple = self.target_tuple_type(rest_type);
                return length + tuple.fixed_length
                    - if tuple.combined_flags.intersects(ElementFlags::VARIABLE) {
                        0
                    } else {
                        1
                    };
            }
        }
        length
    }

    // Go: checker/relater.go:1737 getMinArgumentCount
    pub fn get_min_argument_count(&mut self, signature: SignatureId) -> i32 {
        self.get_min_argument_count_ex(signature, MinArgumentCountFlags::NONE)
    }

    // Go: checker/relater.go:1741 getMinArgumentCountEx
    pub fn get_min_argument_count_ex(
        &mut self,
        signature: SignatureId,
        flags: MinArgumentCountFlags,
    ) -> i32 {
        let strong_arity_for_untyped_js =
            flags & MinArgumentCountFlags::STRONG_ARITY_FOR_UNTYPED_JS;
        let void_is_non_optional = flags & MinArgumentCountFlags::VOID_IS_NON_OPTIONAL;
        if void_is_non_optional.0 != 0 || self.sig(signature).resolved_min_argument_count == -1 {
            let mut min_argument_count: i32 = -1;
            if self.signature_has_rest_parameter(signature) {
                let parameters_len = self.sig(signature).parameters.len();
                let last = self.sig(signature).parameters[parameters_len - 1];
                let rest_type = self.get_type_of_symbol(last);
                if self.is_tuple_type(rest_type) {
                    let tuple = self.target_tuple_type(rest_type);
                    let first_optional_index: i32 = tuple
                        .element_infos
                        .iter()
                        .position(|info| !info.flags.intersects(ElementFlags::REQUIRED))
                        .map_or(-1, |i| i as i32);
                    let mut required_count = first_optional_index;
                    if first_optional_index < 0 {
                        required_count = tuple.fixed_length;
                    }
                    if required_count > 0 {
                        min_argument_count = parameters_len as i32 - 1 + required_count;
                    }
                }
            }
            if min_argument_count == -1 {
                if strong_arity_for_untyped_js.0 == 0
                    && self
                        .sig(signature)
                        .flags
                        .intersects(SignatureFlags::IS_UNTYPED_SIGNATURE_IN_JS_FILE)
                {
                    return 0;
                }
                min_argument_count = self.sig(signature).min_argument_count;
            }
            if void_is_non_optional.0 != 0 {
                return min_argument_count;
            }
            let mut i = min_argument_count - 1;
            while i >= 0 {
                let t = self.get_type_at_position(signature, i);
                if !self.some_type(t, &mut |c: &mut Checker, t: TypeId| {
                    c.ty(t).flags.intersects(TypeFlags::VOID)
                }) {
                    break;
                }
                min_argument_count = i;
                i -= 1;
            }
            self.sig_mut(signature).resolved_min_argument_count = min_argument_count;
        }
        self.sig(signature).resolved_min_argument_count
    }

    // Go: checker/relater.go:1782 hasEffectiveRestParameter
    pub fn has_effective_rest_parameter(&mut self, signature: SignatureId) -> bool {
        if self.signature_has_rest_parameter(signature) {
            let parameters_len = self.sig(signature).parameters.len();
            let last = self.sig(signature).parameters[parameters_len - 1];
            let rest_type = self.get_type_of_symbol(last);
            return !self.is_tuple_type(rest_type)
                || self
                    .target_tuple_type(rest_type)
                    .combined_flags
                    .intersects(ElementFlags::VARIABLE);
        }
        false
    }

    // Go: checker/relater.go:1790 getTypeAtPosition
    pub fn get_type_at_position(&mut self, signature: SignatureId, pos: i32) -> TypeId {
        let t = self.try_get_type_at_position(signature, pos);
        if t.is_some() {
            return t;
        }
        self.any_type
    }

    // Go: checker/relater.go:1798 tryGetTypeAtPosition
    pub fn try_get_type_at_position(&mut self, signature: SignatureId, pos: i32) -> TypeId {
        let param_count = self.sig(signature).parameters.len() as i32
            - if self.signature_has_rest_parameter(signature) {
                1
            } else {
                0
            };
        if pos < param_count {
            // PORT: Go indexes with the int `pos`, so a negative one (an API
            // argument index) panics with the runtime text.
            if pos < 0 {
                crate::core::go_panic(format!("runtime error: index out of range [{pos}]"));
            }
            let parameter = self.sig(signature).parameters[pos as usize];
            return self.get_type_of_parameter(parameter);
        }
        if self.signature_has_rest_parameter(signature) {
            // We want to return the value undefined for an out of bounds parameter position,
            // so we need to check bounds here before calling getIndexedAccessType (which
            // otherwise would return the type 'undefined').
            let rest_parameter = self.sig(signature).parameters[param_count as usize];
            let rest_type = self.get_type_of_symbol(rest_parameter);
            let index = pos - param_count;
            if !self.is_tuple_type(rest_type)
                || self
                    .target_tuple_type(rest_type)
                    .combined_flags
                    .intersects(ElementFlags::VARIABLE)
                || index < self.target_tuple_type(rest_type).fixed_length
            {
                let index_type = self.get_number_literal_type(crate::jsnum::Number(index as f64));
                return self.get_indexed_access_type(rest_type, index_type);
            }
        }
        TypeId::NIL
    }

    // Return the rest type at the given position, transforming `any[]` into just `any`. We do this because
    // in signatures we want `any[]` in a rest position to be compatible with anything, but `any[]` isn't
    // assignable to tuple types with required elements.
    // Go: checker/relater.go:1819 getRestOrAnyTypeAtPosition
    pub fn get_rest_or_any_type_at_position(&mut self, source: SignatureId, pos: i32) -> TypeId {
        let rest_type = self.get_rest_type_at_position(source, pos, false);
        if rest_type.is_some() {
            let element_type = self.get_element_type_of_array_type(rest_type);
            if element_type.is_some() && self.is_type_any(element_type) {
                return self.any_type;
            }
        }
        rest_type
    }

    // Go: checker/relater.go:1829 getRestTypeAtPosition
    pub fn get_rest_type_at_position(
        &mut self,
        source: SignatureId,
        pos: i32,
        readonly: bool,
    ) -> TypeId {
        let parameter_count = self.get_parameter_count(source);
        let min_argument_count = self.get_min_argument_count(source);
        let rest_type = self.get_effective_rest_type(source);
        if rest_type.is_some() && pos >= parameter_count - 1 {
            if pos == parameter_count - 1 {
                return rest_type;
            } else {
                let number_type = self.number_type;
                let element_type = self.get_indexed_access_type(rest_type, number_type);
                return self.create_array_type(element_type);
            }
        }
        let length = parameter_count - pos;
        if length <= 0 {
            return self.create_tuple_type_ex(&[], &[], readonly);
        }
        let length = length as usize;
        let mut types: Vec<TypeId> = vec![TypeId::NIL; length];
        let mut infos: Vec<TupleElementInfo> = vec![TupleElementInfo::default(); length];
        for i in 0..length {
            let flags: ElementFlags;
            let position = i as i32 + pos;
            if rest_type.is_nil() || i < length - 1 {
                types[i] = self.get_type_at_position(source, position);
                flags = if position < min_argument_count {
                    ElementFlags::REQUIRED
                } else {
                    ElementFlags::OPTIONAL
                };
            } else {
                types[i] = rest_type;
                flags = ElementFlags::VARIADIC;
            }
            let labeled_declaration = self.get_nameable_declaration_at_position(source, position);
            infos[i] = TupleElementInfo {
                flags,
                labeled_declaration,
            };
        }
        self.create_tuple_type_ex(&types, &infos, readonly)
    }
}
