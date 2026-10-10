//! Port of `checker/checker.go` lines 18536-19435: optionality helpers,
//! cached node/modifier flags, the type resolution stack, property, signature
//! and index info lookup, structured member resolution, base types, and
//! signature instantiation.

use crate::prelude::*;
use smallvec::SmallVec;
use std::borrow::Cow;

impl Checker {
    // Go: checker/checker.go:18969 addOptionality
    pub fn add_optionality(&mut self, t: TypeId) -> TypeId {
        self.add_optionality_ex(t, false /*isProperty*/, true /*isOptional*/)
    }

    // Go: checker/checker.go:18973 addOptionalityEx
    pub fn add_optionality_ex(
        &mut self,
        t: TypeId,
        is_property: bool,
        is_optional: bool,
    ) -> TypeId {
        if self.strict_null_checks && is_optional {
            return self.get_optional_type(t, is_property);
        }
        t
    }

    // Go: checker/checker.go:18980 getOptionalType
    pub fn get_optional_type(&mut self, t: TypeId, is_property: bool) -> TypeId {
        debug_assert!(self.strict_null_checks);
        let missing_or_undefined = if is_property {
            self.undefined_or_missing_type
        } else {
            self.undefined_type
        };
        if t == missing_or_undefined
            || self.ty(t).flags.intersects(TypeFlags::UNION)
                && self.ty(t).types()[0] == missing_or_undefined
        {
            return t;
        }
        self.get_union_type(&[t, missing_or_undefined])
    }

    // Go: checker/checker.go:18990 getNullableType
    // Add undefined or null or both to a type if they are missing.
    pub fn get_nullable_type(&mut self, t: TypeId, flags: TypeFlags) -> TypeId {
        let missing = (flags & !self.ty(t).flags) & (TypeFlags::UNDEFINED | TypeFlags::NULL);
        if missing.is_empty() {
            return t;
        } else if missing == TypeFlags::UNDEFINED {
            let undefined_type = self.undefined_type;
            return self.get_union_type(&[t, undefined_type]);
        } else if missing == TypeFlags::NULL {
            let null_type = self.null_type;
            return self.get_union_type(&[t, null_type]);
        }
        let undefined_type = self.undefined_type;
        let null_type = self.null_type;
        self.get_union_type(&[t, undefined_type, null_type])
    }

    // Go: checker/checker.go:19003 GetNonNullableType
    pub fn get_non_nullable_type(&mut self, t: TypeId) -> TypeId {
        if self.strict_null_checks {
            return self.get_adjusted_type_with_facts(t, TypeFacts::NE_UNDEFINED_OR_NULL);
        }
        t
    }

    // Go: checker/checker.go:19010 IsNullableType
    pub fn is_nullable_type(&mut self, t: TypeId) -> bool {
        self.has_type_facts(t, TypeFacts::IS_UNDEFINED_OR_NULL)
    }

    // Go: checker/checker.go:19014 getNonNullableTypeIfNeeded
    pub fn get_non_nullable_type_if_needed(&mut self, t: TypeId) -> TypeId {
        if self.is_nullable_type(t) {
            return self.get_non_nullable_type(t);
        }
        t
    }

    // Go: checker/checker.go:19021 getDeclarationNodeFlagsFromSymbol
    pub fn get_declaration_node_flags_from_symbol(&mut self, s: SymbolId) -> NodeFlags {
        let value_declaration = self.sym(s).value_declaration;
        if value_declaration.is_some() {
            return self.get_combined_node_flags_cached(value_declaration);
        }
        NodeFlags::NONE
    }

    // Go: checker/checker.go:19028 getCombinedNodeFlagsCached
    // PORT: the result has no binder-added bit (`BINDER_ADDED_FLAGS`). Every
    // caller (and `get_declaration_node_flags_from_symbol`) tests parser
    // bits only: block scope, `CONSTANT`, `AMBIENT` and the deprecated tag.
    // A caller that needs a binder bit uses `get_combined_node_flags`.
    // PERF: lsshells M3 repair. The walk reads no binder data, which is a
    // pinned read for the edited file in a language server; `narrow_type`
    // and the discriminant checks made it one of the most frequent reads of
    // that file.
    pub fn get_combined_node_flags_cached(&mut self, node: Node) -> NodeFlags {
        // we hold onto the last node and result to speed up repeated lookups against the same node.
        if self.last_get_combined_node_flags_node == node {
            return self.last_get_combined_node_flags_result;
        }
        self.last_get_combined_node_flags_node = node;
        self.last_get_combined_node_flags_result =
            get_combined_parser_flags(node, PARSER_ONLY_FLAGS);
        self.last_get_combined_node_flags_result
    }

    // Go: checker/checker.go:19038 isVarConstLike
    pub fn is_var_const_like(&mut self, node: Node) -> bool {
        let block_scope_kind = self.get_combined_node_flags_cached(node) & NodeFlags::BLOCK_SCOPED;
        block_scope_kind == NodeFlags::CONST
            || block_scope_kind == NodeFlags::USING
            || block_scope_kind == NodeFlags::AWAIT_USING
    }

    // Go: checker/checker.go:19043 getEffectivePropertyNameForPropertyNameNode
    // PERF: a name read from the tree is borrowed (`property_name_text`), so
    // most calls make no String.
    pub fn get_effective_property_name_for_property_name_node(
        &mut self,
        node: Node,
    ) -> (Cow<'static, str>, bool) {
        let name = property_name_text(node);
        if name != INTERNAL_SYMBOL_NAME_MISSING {
            return (name, true);
        } else if is_computed_property_name(node) {
            // This is cached so `getTypeOfExpression` isn't constantly reinvoked for every property name lookup
            let links = self.computed_name_links.get(node);
            if let Some(has_name) = links.has_name {
                return (Cow::Owned(links.name.clone()), has_name);
            }
            let t = self.get_type_of_expression(node.expression());
            let (name, exists) = self.try_get_name_from_type(t);
            let links = self.computed_name_links.get(node);
            links.name = name.clone();
            links.has_name = Some(exists);
            return (Cow::Owned(name), exists);
        }
        (Cow::Borrowed(""), false)
    }

    // Go: checker/checker.go:19062 tryGetNameFromType
    pub fn try_get_name_from_type(&mut self, t: TypeId) -> (String, bool) {
        let flags = self.ty(t).flags;
        if flags.intersects(TypeFlags::UNIQUE_ES_SYMBOL) {
            (self.ty(t).as_unique_es_symbol_type().name.clone(), true)
        } else if flags.intersects(TypeFlags::STRING_LITERAL) {
            let s = self.get_string_literal_value(t);
            (s, true)
        } else if flags.intersects(TypeFlags::NUMBER_LITERAL) {
            let s = self.get_number_literal_value(t).to_string();
            (s, true)
        } else {
            (String::new(), false)
        }
    }

    // Go: checker/checker.go:19077 getCombinedModifierFlagsCached
    pub fn get_combined_modifier_flags_cached(&mut self, node: Node) -> ModifierFlags {
        // we hold onto the last node and result to speed up repeated lookups against the same node.
        if self.last_get_combined_modifier_flags_node == node {
            return self.last_get_combined_modifier_flags_result;
        }
        self.last_get_combined_modifier_flags_node = node;
        self.last_get_combined_modifier_flags_result = get_combined_modifier_flags(node);
        self.last_get_combined_modifier_flags_result
    }

    // Go: checker/checker.go:19098 pushTypeResolution
    /// Push an entry on the type resolution stack. If an entry with the given target and the given property name
    /// is already on the stack, and no entries in between already have a type, then a circularity has occurred.
    /// In this case, the result values of the existing entry and all entries pushed after it are changed to false,
    /// and the value false is returned. Otherwise, the new entry is just pushed onto the stack, and true is returned.
    /// In order to see if the same query has already been done before, the target object and the propertyName both
    /// must match the one passed in.
    ///
    /// target: The symbol, type, or signature whose type is being queried
    /// property_name: The property name that should be used to query the target for its type
    pub fn push_type_resolution(
        &mut self,
        target: TypeSystemEntity,
        property_name: TypeSystemPropertyName,
    ) -> bool {
        let resolution_cycle_start_index =
            self.find_resolution_cycle_start_index(target, property_name);
        if resolution_cycle_start_index >= 0 {
            // A cycle was found
            for i in resolution_cycle_start_index as usize..self.type_resolutions.len() {
                self.type_resolutions[i].result = false;
            }
            return false;
        }
        self.type_resolutions.push(TypeResolution {
            target,
            property_name,
            result: true,
        });
        true
    }

    // Go: checker/checker.go:19115 popTypeResolution
    /// Pop an entry from the type resolution stack and return its associated result value. The result value will
    /// be true if no circularities were detected, or false if a circularity was found.
    pub fn pop_type_resolution(&mut self) -> bool {
        let last = self
            .type_resolutions
            .pop()
            .expect("popTypeResolution on empty stack");
        last.result
    }

    // Go: checker/checker.go:19123 findResolutionCycleStartIndex
    pub fn find_resolution_cycle_start_index(
        &mut self,
        target: TypeSystemEntity,
        property_name: TypeSystemPropertyName,
    ) -> i32 {
        let mut i = self.type_resolutions.len() as i32 - 1;
        while i >= self.resolution_start {
            let resolution = self.type_resolutions[i as usize];
            if self.type_resolution_has_property(&resolution) {
                return -1;
            }
            if resolution.target == target && resolution.property_name == property_name {
                return i;
            }
            i -= 1;
        }
        -1
    }

    // Go: checker/checker.go:19136 typeResolutionHasProperty
    pub fn type_resolution_has_property(&mut self, r: &TypeResolution) -> bool {
        let as_symbol = |e: TypeSystemEntity| match e {
            TypeSystemEntity::Symbol(s) => s,
            _ => panic!("interface conversion: TypeSystemEntity is not *ast.Symbol"),
        };
        let as_type = |e: TypeSystemEntity| match e {
            TypeSystemEntity::Type(t) => t,
            _ => panic!("interface conversion: TypeSystemEntity is not *Type"),
        };
        match r.property_name {
            // Go `Get` gives no id here: each push site read the symbol first.
            TypeSystemPropertyName::TYPE => self
                .value_symbol_links
                .get_noted(as_symbol(r.target))
                .resolved_type
                .is_some(),
            TypeSystemPropertyName::DECLARED_TYPE => self
                .type_alias_links
                .get(as_symbol(r.target))
                .declared_type
                .is_some(),
            TypeSystemPropertyName::RESOLVED_TYPE_ARGUMENTS => {
                // PORT: Go checks `resolvedTypeArguments != nil`. The Rust field
                // is a `SharedList`, so an empty list reads as unresolved.
                !self
                    .ty(as_type(r.target))
                    .as_type_reference()
                    .resolved_type_arguments
                    .is_empty()
            }
            TypeSystemPropertyName::RESOLVED_BASE_TYPES => {
                self.ty(as_type(r.target))
                    .as_interface_type()
                    .base_types_resolved
            }
            TypeSystemPropertyName::RESOLVED_BASE_CONSTRUCTOR_TYPE => self
                .ty(as_type(r.target))
                .as_interface_type()
                .resolved_base_constructor_type
                .is_some(),
            TypeSystemPropertyName::RESOLVED_RETURN_TYPE => match r.target {
                TypeSystemEntity::Signature(s) => self.sig(s).resolved_return_type.is_some(),
                _ => panic!("interface conversion: TypeSystemEntity is not *Signature"),
            },
            TypeSystemPropertyName::RESOLVED_BASE_CONSTRAINT => self
                .ty(as_type(r.target))
                .as_constrained_type()
                .resolved_base_constraint
                .is_some(),
            TypeSystemPropertyName::INITIALIZER_IS_UNDEFINED => match r.target {
                TypeSystemEntity::Node(n) => self
                    .node_links
                    .get(n)
                    .flags
                    .intersects(NodeCheckFlags::INITIALIZER_IS_UNDEFINED_COMPUTED),
                _ => panic!("interface conversion: TypeSystemEntity is not *ast.Node"),
            },
            // Go `Get` gives no id here: each push site read the symbol first.
            TypeSystemPropertyName::WRITE_TYPE => self
                .value_symbol_links
                .get_noted(as_symbol(r.target))
                .write_type
                .is_some(),
            TypeSystemPropertyName::ALIAS_TARGET => self
                .alias_symbol_links
                .get(as_symbol(r.target))
                .alias_target
                .is_some(),
            _ => panic!("Unhandled case in typeResolutionHasProperty"),
        }
    }

    // Go: checker/checker.go:19162 reportCircularityError
    pub fn report_circularity_error(&mut self, symbol: SymbolId) -> TypeId {
        let declaration = self.sym(symbol).value_declaration;
        // Check if variable has type annotation that circularly references the variable itself
        if declaration.is_some() {
            if declaration.type_().is_some() {
                let name = self.symbol_to_string(symbol);
                self.error(
                    declaration,
                    diag::X_0_is_referenced_directly_or_indirectly_in_its_own_type_annotation,
                    args![name],
                );
                return self.error_type;
            }
            // Check if variable has initializer that circularly references the variable itself
            if self.no_implicit_any
                && (!is_parameter_declaration(declaration) || declaration.initializer().is_some())
            {
                let name = self.symbol_to_string(symbol);
                self.error(
                    declaration,
                    diag::X_0_implicitly_has_type_any_because_it_does_not_have_a_type_annotation_and_is_referenced_directly_or_indirectly_in_its_own_initializer,
                    args![name],
                );
            }
        } else if self.sym(symbol).flags.intersects(SymbolFlags::ALIAS) {
            let node = self.get_declaration_of_alias_symbol(symbol);
            if node.is_some() {
                let name = self.symbol_to_string(symbol);
                self.error(
                    node,
                    diag::Circular_definition_of_import_alias_0,
                    args![name],
                );
            }
        }
        // Circularities could also result from parameters in function expressions that end up
        // having themselves as contextual types following type argument inference. In those cases
        // we have already reported an implicit any error so we don't report anything here.
        self.any_type
    }

    // Go: checker/checker.go:19186 getPropertiesOfType
    pub fn get_properties_of_type(&mut self, t: TypeId) -> SharedList<SymbolId> {
        let t = self.get_reduced_apparent_type(t);
        if self
            .ty(t)
            .flags
            .intersects(TypeFlags::UNION_OR_INTERSECTION)
        {
            return self.get_properties_of_union_or_intersection_type(t);
        }
        self.get_properties_of_object_type(t)
    }

    /// `get_properties_of_type(t).len()` without a copy of the list.
    pub fn get_properties_of_type_count(&mut self, t: TypeId) -> usize {
        let t = self.get_reduced_apparent_type(t);
        if self
            .ty(t)
            .flags
            .intersects(TypeFlags::UNION_OR_INTERSECTION)
        {
            self.resolve_properties_of_union_or_intersection_type(t);
            return self
                .ty(t)
                .as_union_or_intersection_type()
                .resolved_properties
                .len();
        }
        if self.ty(t).flags.intersects(TypeFlags::OBJECT) {
            return self.resolve_structured_type_members(t).properties.len();
        }
        0
    }

    // Go: checker/checker.go:19194 getPropertiesOfObjectType
    pub fn get_properties_of_object_type(&mut self, t: TypeId) -> SharedList<SymbolId> {
        if self.ty(t).flags.intersects(TypeFlags::OBJECT) {
            return self.resolve_structured_type_members(t).properties.clone();
        }
        SharedList::default()
    }

    // Go: checker/checker.go:19201 getPropertiesOfUnionOrIntersectionType
    pub fn get_properties_of_union_or_intersection_type(
        &mut self,
        t: TypeId,
    ) -> SharedList<SymbolId> {
        self.resolve_properties_of_union_or_intersection_type(t);
        self.ty(t)
            .as_union_or_intersection_type()
            .resolved_properties
            .clone()
    }

    /// The body of Go `getPropertiesOfUnionOrIntersectionType`. It stores the
    /// list in `resolved_properties`.
    fn resolve_properties_of_union_or_intersection_type(&mut self, t: TypeId) {
        // PORT: Go checks `resolvedProperties == nil`. The Rust field is a
        // `SharedList`, so an empty result is recomputed; the recomputation is
        // idempotent because the property lookups are cached.
        if self
            .ty(t)
            .as_union_or_intersection_type()
            .resolved_properties
            .is_empty()
        {
            let mut checked: FxHashSet<Name> = FxHashSet::default();
            let mut props: Vec<SymbolId> = Vec::new();
            let t_flags = self.ty(t).flags;
            for i in 0..self.ty(t).types().len() {
                let current = self.type_at(t, i);
                let current_props = self.get_properties_of_type(current);
                // PORT: room for this constituent's properties up front. A
                // union usually stops after its first constituent, so the set
                // and the list grow once.
                checked.reserve(current_props.len());
                props.reserve(current_props.len());
                for prop in current_props {
                    let prop_name = self.sym(prop).name.clone();
                    if checked.insert(prop_name.clone()) {
                        // PORT: a `Name` key, so the lookups compare ids.
                        let combined_prop = self.get_property_of_union_or_intersection_type_key(
                            t,
                            TableKey::Name(&prop_name),
                            t_flags.intersects(TypeFlags::INTERSECTION), /*skipObjectFunctionPropertyAugment*/
                        );
                        if combined_prop.is_some() {
                            props.push(combined_prop);
                        }
                    }
                }
                // The properties of a union type are those that are present in all constituent types, so
                // we only need to check the properties of the first type without index signature
                if t_flags.intersects(TypeFlags::UNION)
                    && self.get_index_infos_of_type(current).is_empty()
                {
                    break;
                }
            }
            self.ty_mut(t)
                .as_union_or_intersection_type_mut()
                .resolved_properties = props.into();
        }
    }

    // Go: checker/checker.go:19227 getPropertyOfType
    pub fn get_property_of_type(&mut self, t: TypeId, name: &str) -> SymbolId {
        self.get_property_of_type_ex(
            t, name, false, /*skipObjectFunctionPropertyAugment*/
            false, /*includeTypeOnlyMembers*/
        )
    }

    // PORT: `get_property_of_type` for a caller that holds the `Name`. The
    // member lookup compares name ids and hashes nothing.
    pub fn get_property_of_type_name(&mut self, t: TypeId, name: &Name) -> SymbolId {
        self.get_property_of_type_ex(
            t, name, false, /*skipObjectFunctionPropertyAugment*/
            false, /*includeTypeOnlyMembers*/
        )
    }

    // Go: checker/checker.go:19239 getPropertyOfTypeEx
    /// Return the symbol for the property with the given name in the given type. Creates synthetic union properties when
    /// necessary, maps primitive types and type parameters are to their apparent types, and augments with properties from
    /// Object and Function as appropriate.
    ///
    /// t: a type to look up property from
    /// name: a name of property to look up in a given type
    // PORT: `name` is a `&str` or a `&Name` (`TableKey`). With a `Name`, the
    // members lookup compares ids instead of hashing and comparing text.
    pub fn get_property_of_type_ex<'a>(
        &mut self,
        t: TypeId,
        name: impl Into<TableKey<'a>>,
        skip_object_function_property_augment: bool,
        include_type_only_members: bool,
    ) -> SymbolId {
        let name: TableKey<'a> = name.into();
        let t = self.get_reduced_apparent_type(t);
        let flags = self.ty(t).flags;
        if flags.intersects(TypeFlags::OBJECT) {
            let members = self.resolve_structured_type_members(t).members;
            let mut symbol = self.symbols.get_key(members, name);
            if symbol.is_some() {
                let t_symbol = self.ty(t).symbol;
                if !include_type_only_members
                    && t_symbol.is_some()
                    && self
                        .sym(t_symbol)
                        .flags
                        .intersects(SymbolFlags::VALUE_MODULE)
                    && self
                        .module_symbol_links
                        .get(t_symbol)
                        .type_only_export_star_map
                        .get(name.text())
                        .is_some_and(|n| n.is_some())
                {
                    // If this is the type of a module, `resolved.members.get(name)` might have effectively skipped over
                    // an `export type * from './foo'`, leaving `symbolIsValue` unable to see that the symbol is being
                    // viewed through a type-only export.
                    return SymbolId::NIL;
                }
                if self.symbol_is_value_ex(symbol, include_type_only_members) {
                    return symbol;
                }
            }
            if skip_object_function_property_augment {
                return SymbolId::NIL;
            }
            // PERF (propfilt1): Go next reads the signatures of `t` and
            // looks the name up in the function type and in Object
            // (checker.go:19259-19274, getPropertyOfObjectType at :21740).
            // When those types are resolved, each lookup is one map read,
            // so a name that the filters of their member names reject
            // gives nil and changes no state. The filters return that nil
            // with no lookups. A text key takes the lookups.
            if let TableKey::Name(key) = name {
                if self.augment_lookups_miss(t, key) {
                    return SymbolId::NIL;
                }
            }
            let resolved = self.ty(t).as_structured_type();
            let call_count = resolved.call_signatures().len();
            let construct_count = resolved.construct_signatures().len();
            let function_type = if t == self.any_function_type {
                self.global_function_type
            } else if call_count != 0 {
                self.global_callable_function_type
            } else if construct_count != 0 {
                self.global_newable_function_type
            } else {
                TypeId::NIL
            };
            // PORT: the fallbacks below get the key, not its text, so a
            // `Name` key stays an id compare down the whole lookup.
            if function_type.is_some() {
                symbol = self.get_property_of_object_type_key(function_type, name);
                if symbol.is_some() {
                    return symbol;
                }
            }
            let global_object_type = self.global_object_type;
            return self.get_property_of_object_type_key(global_object_type, name);
        } else if flags.intersects(TypeFlags::INTERSECTION) {
            let prop = self.get_property_of_union_or_intersection_type_key(
                t, name, true, /*skipObjectFunctionPropertyAugment*/
            );
            if prop.is_some() {
                return prop;
            }
            if !skip_object_function_property_augment {
                return self.get_property_of_union_or_intersection_type_key(
                    t,
                    name,
                    skip_object_function_property_augment,
                );
            }
            return SymbolId::NIL;
        } else if flags.intersects(TypeFlags::UNION) {
            return self.get_property_of_union_or_intersection_type_key(
                t,
                name,
                skip_object_function_property_augment,
            );
        }
        SymbolId::NIL
    }

    /// True when Go's lookups of `name` in the function type of `t` and in
    /// Object give nil, by the filters of their member names. False when a
    /// filter keeps the name, and while a type that the lookups read is
    /// not resolved: the lookups then run and resolve it, as in Go. No Go
    /// counterpart.
    #[inline]
    fn augment_lookups_miss(&mut self, t: TypeId, name: &Name) -> bool {
        let bits = augment_filter_bits(name);
        match self.augment_filters.all {
            Some(all) => !augment_filter_has(&all, bits),
            None => self.augment_lookups_miss_partial(t, bits),
        }
    }

    /// `augment_lookups_miss` while not all 4 filters are built: tests the
    /// filters of the two types that Go reads for `t`.
    #[inline(never)]
    fn augment_lookups_miss_partial(&mut self, t: TypeId, bits: (usize, usize)) -> bool {
        // The function type of checker.go:19262-19268.
        let resolved = self.ty(t).as_structured_type();
        let function_slot = if t == self.any_function_type {
            Some(AUGMENT_FUNCTION)
        } else if !resolved.call_signatures().is_empty() {
            Some(AUGMENT_CALLABLE)
        } else if !resolved.construct_signatures().is_empty() {
            Some(AUGMENT_NEWABLE)
        } else {
            None
        };
        function_slot.is_none_or(|slot| self.augment_filter_rejects(slot, bits))
            && self.augment_filter_rejects(AUGMENT_OBJECT, bits)
    }

    /// True when the filter of `slot` rejects the name with `bits`. Builds
    /// the filter first when its type has members that `augment_members_set`
    /// did not see: they were set before the checker set the global, or
    /// the type has no table. False while the type is nil or not resolved.
    #[inline]
    fn augment_filter_rejects(&mut self, slot: usize, bits: (usize, usize)) -> bool {
        let g = self.augment_globals()[slot];
        if g.is_nil() {
            return false;
        }
        if self.augment_filters.of[slot] != g {
            let ty = self.ty(g);
            if ty.flags.intersects(TypeFlags::OBJECT)
                && !ty.object_flags.intersects(ObjectFlags::MEMBERS_RESOLVED)
            {
                return false;
            }
            self.build_augment_filter(g);
        }
        !augment_filter_has(&self.augment_filters.bits[slot], bits)
    }

    /// The types of the slots of `augment_filters`, in slot order.
    fn augment_globals(&self) -> [TypeId; 4] {
        [
            self.global_function_type,
            self.global_callable_function_type,
            self.global_newable_function_type,
            self.global_object_type,
        ]
    }

    /// Builds the filters of `t` from the members that
    /// `resolve_object_type_members` just set, when `t` is one of the 4
    /// types. A nested resolution of `t` (a late bound name of `t` that
    /// needs `t`) sets members that the outer one then replaces, so each
    /// set builds again.
    #[inline]
    fn augment_members_set(&mut self, t: TypeId) {
        if self.augment_globals().contains(&t) {
            self.build_augment_filter(t);
        }
    }

    /// Builds again the filters that are built, from the current tables of
    /// their types. Go `mergeModuleAugmentation` can add names in place to
    /// the members map of Object or Function (`mergeSymbol`, checker.go:1484
    /// and :14414), and a type with no late bound members has that map as
    /// its members (`combineSymbolTables` returns it, :14331-14336).
    /// `initialize_checker` calls this after each module augmentation
    /// merge. No Go counterpart.
    pub fn rebuild_augment_filters(&mut self) {
        let of = self.augment_filters.of;
        self.augment_filters = AugmentFilters::default();
        for (slot, g) in of.into_iter().enumerate() {
            if g.is_some() && self.augment_filters.of[slot] != g {
                self.build_augment_filter(g);
            }
        }
    }

    /// Builds the filter of `g` from its members table, in each slot of
    /// `g` (with `strictBindCallApply` off, the 3 function slots are all
    /// Function), and the union when all 4 types are set and their slots
    /// are built. While `initialize_checker` has not set a type, its slot
    /// is nil, and a union then would miss its names when it is set. `g`
    /// is resolved, or not an object type: then it has no table (Go
    /// `getPropertyOfObjectType` gives nil for it), and its filter is
    /// empty.
    #[cold]
    #[inline(never)]
    fn build_augment_filter(&mut self, g: TypeId) {
        let mut bits = [0u64; 4];
        let ty = self.ty(g);
        if ty.flags.intersects(TypeFlags::OBJECT) {
            for (name, _) in self.symbols.iter_names(ty.as_structured_type().members) {
                let (a, b) = augment_filter_bits(&name);
                bits[a >> 6] |= 1 << (a & 63);
                bits[b >> 6] |= 1 << (b & 63);
            }
        }
        let globals = self.augment_globals();
        let filters = &mut self.augment_filters;
        for (slot, global) in globals.into_iter().enumerate() {
            if global == g {
                filters.of[slot] = g;
                filters.bits[slot] = bits;
            }
        }
        filters.all = None;
        if globals.iter().all(|global| global.is_some()) && filters.of == globals {
            let mut all = [0u64; 4];
            for slot_bits in &filters.bits {
                for (word, slot_word) in all.iter_mut().zip(slot_bits) {
                    *word |= slot_word;
                }
            }
            filters.all = Some(all);
        }
    }

    /// Drops the filters of `t` and the union at the `get_base_types`
    /// reset: the members of `t` resolve again, and
    /// `augment_members_set` builds them then.
    #[inline]
    fn drop_augment_filter_of(&mut self, t: TypeId) {
        let filters = &mut self.augment_filters;
        for of in &mut filters.of {
            if *of == t {
                *of = TypeId::NIL;
                filters.all = None;
            }
        }
    }

    // Go: checker/checker.go:19291 getTypeOfPropertyOfType
    // Return the type of the given property in the given type, or nil if no such property exists
    pub fn get_type_of_property_of_type(&mut self, t: TypeId, name: &str) -> TypeId {
        let prop = self.get_property_of_type(t, name);
        if prop.is_some() {
            return self.get_type_of_symbol(prop);
        }
        TypeId::NIL
    }

    // Go: checker/checker.go:19299 getSignaturesOfType
    pub fn get_signatures_of_type(
        &mut self,
        t: TypeId,
        kind: SignatureKind,
    ) -> SharedList<SignatureId> {
        let reduced = self.get_reduced_apparent_type(t);
        self.get_signatures_of_structured_type(reduced, kind)
    }

    // Go: checker/checker.go:19303 getSignaturesOfStructuredType
    pub fn get_signatures_of_structured_type(
        &mut self,
        t: TypeId,
        kind: SignatureKind,
    ) -> SharedList<SignatureId> {
        if !self.ty(t).flags.intersects(TypeFlags::STRUCTURED_TYPE) {
            return SharedList::default();
        }
        let Some(d) = &self.resolve_structured_type_members(t).signatures_data else {
            return SharedList::default();
        };
        let call_count = d.call_signature_count as usize;
        if kind == SignatureKind::CALL {
            return d.signatures.slice(0..call_count);
        }
        d.signatures.slice(call_count..d.signatures.len())
    }

    /// `instantiate_signatures(&get_signatures_of_type(t, kind), m)` without a
    /// copy of the source list. Resolved members never change, so the
    /// signatures are read in place, in order.
    pub fn instantiate_signatures_of_type(
        &mut self,
        t: TypeId,
        kind: SignatureKind,
        m: MapperId,
    ) -> Vec<SignatureId> {
        let t = self.get_reduced_apparent_type(t);
        if !self.ty(t).flags.intersects(TypeFlags::STRUCTURED_TYPE) {
            return Vec::new();
        }
        let resolved = self.resolve_structured_type_members(t);
        let call_count = resolved.call_signature_count() as usize;
        let range = if kind == SignatureKind::CALL {
            0..call_count
        } else {
            call_count..resolved.signatures().len()
        };
        let mut result = Vec::with_capacity(range.len());
        for i in range {
            let signature = self.ty(t).as_structured_type().signatures()[i];
            result.push(self.instantiate_signature(signature, m));
        }
        result
    }

    // Go: checker/checker.go:19314 getIndexInfosOfType
    pub fn get_index_infos_of_type(&mut self, t: TypeId) -> SharedList<IndexInfoId> {
        let reduced = self.get_reduced_apparent_type(t);
        self.get_index_infos_of_structured_type(reduced)
    }

    // Go: checker/checker.go:19318 getIndexInfosOfStructuredType
    pub fn get_index_infos_of_structured_type(&mut self, t: TypeId) -> SharedList<IndexInfoId> {
        if self.ty(t).flags.intersects(TypeFlags::STRUCTURED_TYPE) {
            return self.resolve_structured_type_members(t).index_infos_list();
        }
        SharedList::default()
    }

    // Go: checker/checker.go:19327 getIndexInfoOfType
    // Return the indexing info of the given kind in the given type. Creates synthetic union index types when necessary and
    // maps primitive types and type parameters are to their apparent types.
    pub fn get_index_info_of_type(&mut self, t: TypeId, key_type: TypeId) -> IndexInfoId {
        let index_infos = self.get_index_infos_of_type(t);
        self.find_index_info(&index_infos, key_type)
    }

    // Go: checker/checker.go:19333 getIndexTypeOfType
    // Return the index type of the given kind in the given type. Creates synthetic union index types when necessary and
    // maps primitive types and type parameters are to their apparent types.
    pub fn get_index_type_of_type(&mut self, t: TypeId, key_type: TypeId) -> TypeId {
        let info = self.get_index_info_of_type(t, key_type);
        if info.is_some() {
            return self.index_info(info).value_type;
        }
        TypeId::NIL
    }

    // Go: checker/checker.go:19341 getIndexTypeOfTypeEx
    pub fn get_index_type_of_type_ex(
        &mut self,
        t: TypeId,
        key_type: TypeId,
        default_type: TypeId,
    ) -> TypeId {
        let result = self.get_index_type_of_type(t, key_type);
        if result.is_some() {
            return result;
        }
        default_type
    }

    // Go: checker/checker.go:19348 getApplicableIndexInfo
    pub fn get_applicable_index_info(&mut self, t: TypeId, key_type: TypeId) -> IndexInfoId {
        let index_infos = self.get_index_infos_of_type(t);
        self.find_applicable_index_info(&index_infos, key_type)
    }

    // Go: checker/checker.go:19352 getApplicableIndexInfoForName
    pub fn get_applicable_index_info_for_name(&mut self, t: TypeId, name: &str) -> IndexInfoId {
        if is_late_bound_name(name) {
            let es_symbol_type = self.es_symbol_type;
            return self.get_applicable_index_info(t, es_symbol_type);
        }
        let key_type = self.get_string_literal_type(name);
        self.get_applicable_index_info(t, key_type)
    }

    // Go: checker/checker.go:19359 findApplicableIndexInfo
    pub fn find_applicable_index_info(
        &mut self,
        index_infos: &[IndexInfoId],
        key_type: TypeId,
    ) -> IndexInfoId {
        // Index signatures for type 'string' are considered only when no other index signatures apply.
        let mut string_index_info = IndexInfoId::NIL;
        // PORT: most types have a few index infos, so the lists stay on the
        // stack.
        let mut applicable_infos: SmallVec<[IndexInfoId; 8]> = SmallVec::new();
        for &info in index_infos {
            let info_key_type = self.index_info(info).key_type;
            if info_key_type == self.string_type {
                string_index_info = info;
            } else if self.is_applicable_index_type(key_type, info_key_type) {
                applicable_infos.push(info);
            }
        }
        // When more than one index signature is applicable we create a synthetic IndexInfo. Instead of computing
        // the intersected key type, we just use unknownType for the key type as nothing actually depends on the
        // keyType property of the returned IndexInfo.
        match applicable_infos.len() {
            0 => {
                let string_type = self.string_type;
                if string_index_info.is_some()
                    && self.is_applicable_index_type(key_type, string_type)
                {
                    return string_index_info;
                }
                IndexInfoId::NIL
            }
            1 => applicable_infos[0],
            _ => {
                let mut is_readonly = true;
                let mut types: SmallVec<[TypeId; 8]> =
                    SmallVec::with_capacity(applicable_infos.len());
                for &info in &applicable_infos {
                    types.push(self.index_info(info).value_type);
                    if !self.index_info(info).is_readonly {
                        is_readonly = false;
                    }
                }
                let unknown_type = self.unknown_type;
                let value_type = self.get_intersection_type(&types);
                self.new_index_info(unknown_type, value_type, is_readonly, Node::NIL, &[])
            }
        }
    }

    // Go: checker/checker.go:19394 isApplicableIndexType
    pub fn is_applicable_index_type(&mut self, source: TypeId, target: TypeId) -> bool {
        // A 'string' index signature applies to types assignable to 'string' or 'number', and a 'number' index
        // signature applies to types assignable to 'number', `${number}` and numeric string literal types.
        if self.is_type_assignable_to(source, target) {
            return true;
        }
        if target == self.string_type {
            let number_type = self.number_type;
            if self.is_type_assignable_to(source, number_type) {
                return true;
            }
        }
        target == self.number_type
            && (source == self.numeric_string_type
                || self.ty(source).flags.intersects(TypeFlags::STRING_LITERAL)
                    && is_numeric_literal_name(self.get_string_literal_value_ref(source)))
    }

    // Go: checker/checker.go:19402 resolveStructuredTypeMembers
    // PORT: Go returns `*StructuredType`. Rust returns a shared borrow of the
    // resolved type's `StructuredType`; callers that need to call other
    // checker methods copy what they need out of it first.
    // PORT: the resolved case is the common one, so it stays inline and the
    // dispatch below is out of line.
    // PERF: the resolved path returns from inside the flags test, so the
    // test and the cast read the same `&Type` (one arena lookup). With the
    // cast after the `if`, the slow path joins the fast path before the cast
    // and the arena was read again.
    #[inline]
    pub fn resolve_structured_type_members(&mut self, t: TypeId) -> &StructuredType {
        if self
            .ty(t)
            .object_flags
            .intersects(ObjectFlags::MEMBERS_RESOLVED)
        {
            return self.ty(t).as_structured_type();
        }
        self.resolve_structured_type_members_slow(t)
    }

    /// The member resolution dispatch of `resolve_structured_type_members`.
    #[inline(never)]
    fn resolve_structured_type_members_slow(&mut self, t: TypeId) -> &StructuredType {
        let flags = self.ty(t).flags;
        let object_flags = self.ty(t).object_flags;
        if flags.intersects(TypeFlags::OBJECT) {
            if object_flags.intersects(ObjectFlags::REFERENCE) {
                self.resolve_type_reference_members(t);
            } else if object_flags.intersects(ObjectFlags::CLASS_OR_INTERFACE) {
                self.resolve_class_or_interface_members(t);
            } else if object_flags.intersects(ObjectFlags::REVERSE_MAPPED) {
                self.resolve_reverse_mapped_type_members(t);
            } else if object_flags.intersects(ObjectFlags::ANONYMOUS) {
                self.resolve_anonymous_type_members(t);
            } else if object_flags.intersects(ObjectFlags::MAPPED) {
                self.resolve_mapped_type_members(t);
            } else {
                panic!("Unhandled case in resolveStructuredTypeMembers");
            }
        } else if flags.intersects(TypeFlags::UNION) {
            self.resolve_union_type_members(t);
        } else if flags.intersects(TypeFlags::INTERSECTION) {
            self.resolve_intersection_type_members(t);
        } else {
            panic!("Unhandled case in resolveStructuredTypeMembers");
        }
        self.ty(t).as_structured_type()
    }

    // Go: checker/checker.go:19431 resolveClassOrInterfaceMembers
    pub fn resolve_class_or_interface_members(&mut self, t: TypeId) {
        self.resolve_object_type_members(t, t, &[], &[]);
    }

    // Go: checker/checker.go:19435 resolveTypeReferenceMembers
    pub fn resolve_type_reference_members(&mut self, t: TypeId) {
        let source = self.ty(t).target();
        let type_parameters = self
            .ty(source)
            .as_interface_type()
            .all_type_parameters
            .clone();
        // One exact-size allocation: the arguments, then `t` as the `this`
        // argument when only that one is missing.
        let padded_type_arguments = {
            let type_arguments = self.type_arguments_of(t);
            let pad = type_arguments.len() == type_parameters.len().wrapping_sub(1);
            let mut padded = Vec::with_capacity(type_arguments.len() + usize::from(pad));
            padded.extend_from_slice(&type_arguments);
            if pad {
                padded.push(t);
            }
            padded
        };
        self.resolve_object_type_members(t, source, &type_parameters, &padded_type_arguments);
    }

    // Go: checker/checker.go:19446 resolveObjectTypeMembers
    // PERF: the declared lists are `SharedList`s. A type without
    // instantiation stores them without a copy, and an instantiated list
    // that does not change is the declared list, as in Go.
    pub fn resolve_object_type_members(
        &mut self,
        t: TypeId,
        source: TypeId,
        type_parameters: &[TypeId],
        type_arguments: &[TypeId],
    ) {
        let mut mapper = MapperId::NIL;
        let mut members: SymbolTable;
        let mut call_signatures: SharedList<SignatureId>;
        let mut construct_signatures: SharedList<SignatureId>;
        let mut index_infos: SharedList<IndexInfoId>;
        let mut instantiated = false;
        let (
            declared_members,
            declared_call_signatures,
            declared_construct_signatures,
            declared_index_infos,
        ) = {
            let resolved = self.resolve_declared_members(source);
            (
                resolved.declared_members,
                resolved.declared_call_signatures.clone(),
                resolved.declared_construct_signatures.clone(),
                resolved.declared_index_infos.clone(),
            )
        };
        if type_parameters == type_arguments {
            members = declared_members;
            call_signatures = declared_call_signatures;
            construct_signatures = declared_construct_signatures;
            index_infos = declared_index_infos;
        } else {
            instantiated = true;
            mapper = self.new_type_mapper(type_parameters, type_arguments);
            members = self.instantiate_symbol_table(declared_members, mapper);
            call_signatures = self.instantiate_shared_list(
                declared_call_signatures,
                mapper,
                Checker::instantiate_signature,
            );
            construct_signatures = self.instantiate_shared_list(
                declared_construct_signatures,
                mapper,
                Checker::instantiate_signature,
            );
            index_infos = self.instantiate_shared_list(
                declared_index_infos,
                mapper,
                Checker::instantiate_index_info,
            );
        }
        // PERF: an instantiated table reuses the named members order of
        // `declared_members` (`get_named_members_of_instantiation`) until
        // inherited members are added to it.
        let instantiated_from = if instantiated {
            declared_members
        } else {
            SymbolTable::NIL
        };
        let base_types = self.get_base_types_shared(source);
        if !base_types.is_empty() {
            if !instantiated {
                // PORT: Go `maps.Clone(members)`; a nil map clones to nil.
                members = self.symbols.clone_table(members);
            }
            let this_argument = type_arguments.last().copied().unwrap_or(TypeId::NIL);
            for base_type in base_types {
                let mut instantiated_base_type = base_type;
                if this_argument.is_some() {
                    let instantiated_type = self.instantiate_type(base_type, mapper);
                    instantiated_base_type = self.get_type_with_this_argument(
                        instantiated_type,
                        this_argument,
                        false, /*needsApparentType*/
                    );
                }
                let base_properties = self.get_properties_of_type(instantiated_base_type);
                members = self.add_inherited_members(members, &base_properties);
                call_signatures = SharedList::concat(
                    call_signatures,
                    self.get_signatures_of_type(instantiated_base_type, SignatureKind::CALL),
                );
                construct_signatures = SharedList::concat(
                    construct_signatures,
                    self.get_signatures_of_type(instantiated_base_type, SignatureKind::CONSTRUCT),
                );
                let inherited_index_infos: SharedList<IndexInfoId> =
                    if instantiated_base_type != self.any_type {
                        self.get_index_infos_of_type(instantiated_base_type)
                    } else {
                        SharedList::from(&[self.any_base_type_index_info][..])
                    };
                let filtered: Vec<IndexInfoId> = inherited_index_infos
                    .iter()
                    .copied()
                    .filter(|&info| {
                        let key_type = self.index_info(info).key_type;
                        self.find_index_info(&index_infos, key_type).is_nil()
                    })
                    .collect();
                index_infos = SharedList::concat(index_infos, SharedList::from(filtered));
            }
            let call_signature_count = call_signatures.len();
            self.set_structured_type_members_ex(
                t,
                members,
                SymbolTable::NIL,
                SharedList::concat(call_signatures, construct_signatures),
                call_signature_count,
                index_infos,
            );
            self.augment_members_set(t);
            return;
        }
        let call_signature_count = call_signatures.len();
        self.set_structured_type_members_ex(
            t,
            members,
            instantiated_from,
            SharedList::concat(call_signatures, construct_signatures),
            call_signature_count,
            index_infos,
        );
        self.augment_members_set(t);
    }

    /// Go `instantiateList` on a shared list. When no element changes, the
    /// result is `values` itself, as Go returns the input slice.
    fn instantiate_shared_list<T: Copy + Default + PartialEq>(
        &mut self,
        values: SharedList<T>,
        m: MapperId,
        instantiator: fn(&mut Checker, T, MapperId) -> T,
    ) -> SharedList<T> {
        match self.instantiate_list_if_changed(&values, m, instantiator) {
            Some(list) => list.into(),
            None => values,
        }
    }
}

impl Type {
    /// The resolved base types of a class, interface or tuple: Go
    /// `getBaseTypes` (checker.go:19504) when it has no work to do. `None`
    /// when they are not resolved yet or this is another kind of type; then
    /// call `get_base_types_shared`, which has side effects.
    ///
    /// PERF: no arena read and no call. `has_base_type` uses it on each
    /// step of its walk.
    #[inline(always)]
    pub fn resolved_base_types(&self) -> Option<&SharedList<TypeId>> {
        if !self
            .object_flags
            .intersects(ObjectFlags::CLASS_OR_INTERFACE | ObjectFlags::TUPLE)
        {
            return None;
        }
        match self.data.as_interface_type() {
            Some(data) if data.base_types_resolved => Some(&data.resolved_base_types),
            // PORT: data that is not an interface panics in
            // `get_base_types_shared`, as Go `AsInterfaceType` does.
            _ => None,
        }
    }
}

// Go: checker/checker.go:19495 findIndexInfo
// PORT: package-level Go function that reads index info data, so it is a
// `Checker` method (`&self`).
impl Checker {
    pub fn find_index_info(&self, index_infos: &[IndexInfoId], key_type: TypeId) -> IndexInfoId {
        for &info in index_infos {
            if self.index_info(info).key_type == key_type {
                return info;
            }
        }
        IndexInfoId::NIL
    }

    // Go: checker/checker.go:19504 getBaseTypes
    pub fn get_base_types(&mut self, t: TypeId) -> Vec<TypeId> {
        self.get_base_types_shared(t).to_vec()
    }

    /// Go `getBaseTypes` as a shared list: no element copy. `has_base_type`
    /// tries `Type::resolved_base_types` first; keep the two in sync.
    pub fn get_base_types_shared(&mut self, t: TypeId) -> SharedList<TypeId> {
        if !self
            .ty(t)
            .object_flags
            .intersects(ObjectFlags::CLASS_OR_INTERFACE | ObjectFlags::TUPLE)
        {
            return SharedList::default();
        }
        if !self.ty(t).as_interface_type().base_types_resolved {
            if !self.push_type_resolution(
                TypeSystemEntity::Type(t),
                TypeSystemPropertyName::RESOLVED_BASE_TYPES,
            ) {
                return self.ty(t).as_interface_type().resolved_base_types.clone();
            }
            let t_symbol = self.ty(t).symbol;
            if self.ty(t).object_flags.intersects(ObjectFlags::TUPLE) {
                let base = self.get_tuple_base_type(t);
                self.ty_mut(t).as_interface_type_mut().resolved_base_types =
                    SharedList::from(&[base][..]);
            } else if self
                .sym(t_symbol)
                .flags
                .intersects(SymbolFlags::CLASS | SymbolFlags::INTERFACE)
            {
                if self.sym(t_symbol).flags.intersects(SymbolFlags::CLASS) {
                    self.resolve_base_types_of_class(t);
                }
                if self.sym(t_symbol).flags.intersects(SymbolFlags::INTERFACE) {
                    self.resolve_base_types_of_interface(t);
                }
            } else {
                panic!("Unhandled case in getBaseTypes");
            }
            // PORT: Go checks `t.symbol.Declarations != nil`; an empty list
            // has nothing to report either way.
            if !self.pop_type_resolution()
                && t_symbol.is_some()
                && !self.sym(t_symbol).declarations.is_empty()
            {
                let declarations = self.sym(t_symbol).declarations.clone();
                for &declaration in declarations.iter() {
                    if is_class_declaration(declaration) || is_interface_declaration(declaration) {
                        self.report_circular_base_type(declaration, t);
                    }
                }
            }
            // In general, base type resolution always precedes member resolution. However, it is possible
            // for resolution of type parameter defaults to cause circularity errors, possibly leaving
            // members partially resolved. Here we ensure any such partial resolution is reset.
            // See https://github.com/microsoft/TypeScript/issues/16861 for an example.
            {
                let object_flags = self
                    .ty(t)
                    .object_flags
                    .without(ObjectFlags::MEMBERS_RESOLVED);
                self.ty_mut(t).object_flags = object_flags;
            }
            self.drop_augment_filter_of(t);
            self.ty_mut(t).as_interface_type_mut().base_types_resolved = true;
        }
        self.ty(t).as_interface_type().resolved_base_types.clone()
    }

    // Go: checker/checker.go:19543 getTupleBaseType
    pub fn get_tuple_base_type(&mut self, t: TypeId) -> TypeId {
        let type_parameters = self.ty(t).as_interface_type().type_parameters().to_vec();
        let element_flags: Vec<ElementFlags> = self
            .ty(t)
            .as_tuple_type()
            .element_infos
            .iter()
            .map(|info| info.flags)
            .collect();
        let mut element_types: Vec<TypeId> = Vec::with_capacity(type_parameters.len());
        for (i, &tp) in type_parameters.iter().enumerate() {
            if element_flags[i].intersects(ElementFlags::VARIADIC) {
                let number_type = self.number_type;
                element_types.push(self.get_indexed_access_type(tp, number_type));
            } else {
                element_types.push(tp);
            }
        }
        let readonly = self.ty(t).as_tuple_type().readonly;
        let element_type = self.get_union_type(&element_types);
        self.create_array_type_ex(element_type, readonly)
    }

    // Go: checker/checker.go:19557 resolveBaseTypesOfClass
    pub fn resolve_base_types_of_class(&mut self, t: TypeId) {
        let base_constructor_type_of_class = self.get_base_constructor_type_of_class(t);
        let base_constructor_type = self.get_apparent_type(base_constructor_type_of_class);
        if !self
            .ty(base_constructor_type)
            .flags
            .intersects(TypeFlags::OBJECT | TypeFlags::INTERSECTION | TypeFlags::ANY)
        {
            return;
        }
        let base_type_node = self.get_base_type_node_of_class(t);
        let base_type: TypeId;
        let mut original_base_type = TypeId::NIL;
        let base_constructor_symbol = self.ty(base_constructor_type).symbol;
        if base_constructor_symbol.is_some() {
            original_base_type = self.get_declared_type_of_symbol(base_constructor_symbol);
        }
        if base_constructor_symbol.is_some()
            && self
                .sym(base_constructor_symbol)
                .flags
                .intersects(SymbolFlags::CLASS)
            && self.are_all_outer_type_parameters_applied(original_base_type)
        {
            // When base constructor type is a class with no captured type arguments we know that the constructors all have the same type parameters as the
            // class and all return the instance type of the class. There is no need for further checks and we can apply the
            // type arguments in the same manner as a type reference to get the same error reporting experience.
            base_type = self.get_type_from_class_or_interface_reference(
                base_type_node,
                base_constructor_symbol,
            );
        } else if self
            .ty(base_constructor_type)
            .flags
            .intersects(TypeFlags::ANY)
        {
            base_type = base_constructor_type;
        } else {
            // The class derives from a "class-like" constructor function, check that we have at least one construct signature
            // with a matching number of type parameters and use the return type of the first instantiated signature. Elsewhere
            // we check that all instantiated signatures return the same type.
            let type_argument_nodes = base_type_node.type_arguments().to_vec();
            let constructors = self.get_instantiated_constructors_for_type_arguments(
                base_constructor_type,
                &type_argument_nodes,
                base_type_node,
            );
            if constructors.is_empty() {
                self.error(
                    base_type_node.expression(),
                    diag::No_base_constructor_has_the_specified_number_of_type_arguments,
                    args![],
                );
                return;
            }
            base_type = self.get_return_type_of_signature(constructors[0]);
        }
        if self.is_error_type(base_type) {
            return;
        }
        let reduced_base_type = self.get_reduced_type(base_type);
        if !self.is_valid_base_type(reduced_base_type) {
            let error_node = base_type_node.expression();
            let diagnostic = self.elaborate_never_intersection(None, error_node, base_type);
            let type_string = self.type_to_string_exported(reduced_base_type);
            let diagnostic = new_diagnostic_chain_for_node(
                diagnostic,
                error_node,
                diag::Base_constructor_return_type_0_is_not_an_object_type_or_intersection_of_object_types_with_statically_known_members,
                args![type_string],
            );
            self.add_diagnostic(diagnostic);
            return;
        }
        if t == reduced_base_type || self.has_base_type(reduced_base_type, t) {
            let value_declaration = self.sym(self.ty(t).symbol).value_declaration;
            let type_string = self.type_to_string_exported(t);
            self.error(
                value_declaration,
                diag::Type_0_recursively_references_itself_as_a_base_type,
                args![type_string],
            );
            return;
        }
        self.ty_mut(t).as_interface_type_mut().resolved_base_types =
            SharedList::from(&[reduced_base_type][..]);
    }

    // Go: checker/checker.go:19604 getBaseTypeNodeOfClass
    // PORT: package-level Go function that reads type data, so it is a
    // `Checker` method.
    pub fn get_base_type_node_of_class(&self, t: TypeId) -> Node {
        let decl = get_class_like_declaration_of_symbol(&self.symbols, self.ty(t).symbol);
        if decl.is_some() {
            return get_class_extends_heritage_element(decl);
        }
        Node::NIL
    }

    // Go: checker/checker.go:19612 getInstantiatedConstructorsForTypeArguments
    pub fn get_instantiated_constructors_for_type_arguments(
        &mut self,
        t: TypeId,
        type_argument_nodes: &[Node],
        location: Node,
    ) -> Vec<SignatureId> {
        let signatures = self.get_constructors_for_type_arguments(t, type_argument_nodes, location);
        let type_arguments: Vec<TypeId> = type_argument_nodes
            .iter()
            .map(|&n| self.get_type_from_type_node(n))
            .collect();
        let mut result = Vec::with_capacity(signatures.len());
        for sig in signatures {
            if !self.sig(sig).type_parameters.is_empty() {
                result.push(self.get_signature_instantiation(
                    sig,
                    &type_arguments,
                    is_in_js_file(location),
                    &[],
                    0,
                ));
            } else {
                result.push(sig);
            }
        }
        result
    }

    // Go: checker/checker.go:19623 getConstructorsForTypeArguments
    pub fn get_constructors_for_type_arguments(
        &mut self,
        t: TypeId,
        type_argument_nodes: &[Node],
        location: Node,
    ) -> Vec<SignatureId> {
        let type_arg_count = type_argument_nodes.len() as i32;
        let signatures = self.get_signatures_of_type(t, SignatureKind::CONSTRUCT);
        let mut result = Vec::new();
        for sig in signatures {
            let type_parameters = self.sig(sig).type_parameters.clone();
            if type_arg_count >= self.get_min_type_argument_count(&type_parameters)
                && type_arg_count <= type_parameters.len() as i32
            {
                result.push(sig);
            }
        }
        result
    }

    // Go: checker/checker.go:19630 getSignatureInstantiation
    pub fn get_signature_instantiation(
        &mut self,
        sig: SignatureId,
        type_arguments: &[TypeId],
        is_java_script: bool,
        inferred_type_parameters: &[TypeId],
        // PORT: slice identity of `inferred_type_parameters`
        // (`InferenceContext::inferred_type_parameters_origin`; 0 when empty).
        inferred_type_parameters_origin: u32,
    ) -> SignatureId {
        let type_parameters = self.sig(sig).type_parameters.clone();
        let min_type_argument_count = self.get_min_type_argument_count(&type_parameters);
        let filled = self.fill_missing_type_arguments(
            type_arguments,
            &type_parameters,
            min_type_argument_count,
            is_java_script,
        );
        let instantiated_signature =
            self.get_signature_instantiation_without_filling_in_type_arguments(sig, &filled);
        if !inferred_type_parameters.is_empty() {
            let return_type = self.get_return_type_of_signature(instantiated_signature);
            let return_signature = self.get_single_call_or_construct_signature(return_type);
            if return_signature.is_some() {
                let new_return_signature = self.clone_signature(return_signature);
                let r = self.sig_mut(new_return_signature);
                r.type_parameters = inferred_type_parameters.to_vec();
                r.type_parameters_origin = inferred_type_parameters_origin;
                let new_return_type = self.get_or_create_type_from_signature(new_return_signature);
                let instantiated_mapper = self.sig(instantiated_signature).mapper;
                self.ty_mut(new_return_type).as_object_type_mut().mapper = instantiated_mapper;
                let new_instantiated_signature = self.clone_signature(instantiated_signature);
                self.sig_mut(new_instantiated_signature)
                    .resolved_return_type = new_return_type;
                return new_instantiated_signature;
            }
        }
        instantiated_signature
    }

    // Go: checker/checker.go:19647 cloneSignature
    pub fn clone_signature(&mut self, sig: SignatureId) -> SignatureId {
        let s = self.sig(sig);
        let flags = s.flags & SignatureFlags::PROPAGATING_FLAGS;
        let declaration = s.declaration;
        let type_parameters = s.type_parameters.clone();
        let this_parameter = s.this_parameter;
        let parameters = s.parameters.clone();
        let min_argument_count = s.min_argument_count;
        let target = s.target;
        let mapper = s.mapper;
        let composite = s.composite.clone();
        let result = self.new_signature(
            flags,
            declaration,
            &type_parameters,
            this_parameter,
            &parameters,
            TypeId::NIL,
            TypePredicateId::NIL,
            min_argument_count,
        );
        // Go shares the type parameter slice with the clone.
        let origin = self.share_type_parameters_origin(sig);
        let r = self.sig_mut(result);
        r.target = target;
        r.mapper = mapper;
        r.composite = composite;
        r.type_parameters_origin = origin;
        result
    }

    // Go: checker/checker.go:19655 getSignatureInstantiationWithoutFillingInTypeArguments
    pub fn get_signature_instantiation_without_filling_in_type_arguments(
        &mut self,
        sig: SignatureId,
        type_arguments: &[TypeId],
    ) -> SignatureId {
        let key = CachedSignatureKey {
            sig,
            key: get_type_list_key(type_arguments),
        };
        let mut instantiation = self
            .cached_signatures
            .get(&key)
            .copied()
            .unwrap_or_default();
        if instantiation.is_nil() {
            instantiation = self.create_signature_instantiation(sig, type_arguments);
            self.cached_signatures.insert(key, instantiation);
        }
        instantiation
    }

    // Go: checker/checker.go:19665 createSignatureInstantiation
    pub fn create_signature_instantiation(
        &mut self,
        sig: SignatureId,
        type_arguments: &[TypeId],
    ) -> SignatureId {
        let mapper = self.create_signature_type_mapper(sig, type_arguments);
        self.instantiate_signature_ex(sig, mapper, true /*eraseTypeParameters*/)
    }

    // Go: checker/checker.go:19669 createSignatureTypeMapper
    pub fn create_signature_type_mapper(
        &mut self,
        sig: SignatureId,
        type_arguments: &[TypeId],
    ) -> MapperId {
        let type_parameters = self.get_type_parameters_for_mapper(sig);
        self.new_type_mapper(&type_parameters, type_arguments)
    }

    // Go: checker/checker.go:19673 getTypeParametersForMapper
    pub fn get_type_parameters_for_mapper(&mut self, sig: SignatureId) -> Vec<TypeId> {
        let type_parameters = self.sig(sig).type_parameters.clone();
        let mut result = Vec::with_capacity(type_parameters.len());
        for tp in type_parameters {
            let mapper = self.ty(tp).mapper();
            result.push(self.instantiate_type(tp, mapper));
        }
        result
    }

    // Go: checker/checker.go:19678 getSingleCallSignature
    // If type has a single call signature and no other members, return that signature. Otherwise, return nil.
    pub fn get_single_call_signature(&mut self, t: TypeId) -> SignatureId {
        self.get_single_signature(t, SignatureKind::CALL, false /*allowMembers*/)
    }

    // Go: checker/checker.go:19682 getSingleCallOrConstructSignature
    pub fn get_single_call_or_construct_signature(&mut self, t: TypeId) -> SignatureId {
        let call_sig =
            self.get_single_signature(t, SignatureKind::CALL, false /*allowMembers*/);
        if call_sig.is_some() {
            return call_sig;
        }
        self.get_single_signature(t, SignatureKind::CONSTRUCT, false /*allowMembers*/)
    }

    // Go: checker/checker.go:19690 getSingleSignature
    pub fn get_single_signature(
        &mut self,
        t: TypeId,
        kind: SignatureKind,
        allow_members: bool,
    ) -> SignatureId {
        if self.ty(t).flags.intersects(TypeFlags::OBJECT) {
            let resolved = self.resolve_structured_type_members(t);
            if allow_members || resolved.properties.is_empty() && resolved.index_infos().is_empty()
            {
                if kind == SignatureKind::CALL
                    && resolved.call_signatures().len() == 1
                    && resolved.construct_signatures().is_empty()
                {
                    return resolved.call_signatures()[0];
                }
                if kind == SignatureKind::CONSTRUCT
                    && resolved.construct_signatures().len() == 1
                    && resolved.call_signatures().is_empty()
                {
                    return resolved.construct_signatures()[0];
                }
            }
        }
        SignatureId::NIL
    }

    // Go: checker/checker.go:19705 getOrCreateTypeFromSignature
    pub fn get_or_create_type_from_signature(&mut self, sig: SignatureId) -> TypeId {
        // There are two ways to declare a construct signature, one is by declaring a class constructor
        // using the constructor keyword, and the other is declaring a bare construct signature in an
        // object type literal or interface (using the new keyword). Each way of declaring a constructor
        // will result in a different declaration kind.
        if self.sig(sig).isolated_signature_type.is_nil() {
            let declaration = self.sig(sig).declaration;
            let kind = if declaration.is_some() {
                declaration.kind()
            } else {
                SyntaxKind::Unknown
            };
            // If declaration is undefined, it is likely to be the signature of the default constructor.
            let is_constructor = kind == SyntaxKind::Unknown
                || kind == SyntaxKind::Constructor
                || kind == SyntaxKind::ConstructSignature
                || kind == SyntaxKind::ConstructorType;

            let symbol = if declaration.is_some() {
                declaration.symbol()
            } else {
                SymbolId::NIL
            };
            let t = self.new_object_type(
                ObjectFlags::ANONYMOUS | ObjectFlags::SINGLE_SIGNATURE_TYPE,
                symbol,
            );
            if is_constructor {
                self.set_structured_type_members(t, SymbolTable::NIL, &[], &[sig], &[]);
            } else {
                self.set_structured_type_members(t, SymbolTable::NIL, &[sig], &[], &[]);
            }
            self.sig_mut(sig).isolated_signature_type = t;
        }
        self.sig(sig).isolated_signature_type
    }

    // Go: checker/checker.go:19733 getErasedSignature
    pub fn get_erased_signature(&mut self, signature: SignatureId) -> SignatureId {
        if self.sig(signature).type_parameters.is_empty() {
            return signature;
        }
        let key = CachedSignatureKey {
            sig: signature,
            key: *SIGNATURE_KEY_ERASED,
        };
        let mut erased = self
            .cached_signatures
            .get(&key)
            .copied()
            .unwrap_or_default();
        if erased.is_nil() {
            let type_parameters = self.sig(signature).type_parameters.clone();
            let any_type = self.any_type;
            let mapper = self.new_array_to_single_type_mapper(&type_parameters, any_type);
            erased =
                self.instantiate_signature_ex(signature, mapper, true /*eraseTypeParameters*/);
            self.cached_signatures.insert(key, erased);
        }
        erased
    }

    // Go: checker/checker.go:19746 getCanonicalSignature
    pub fn get_canonical_signature(&mut self, signature: SignatureId) -> SignatureId {
        if self.sig(signature).type_parameters.is_empty() {
            return signature;
        }
        let key = CachedSignatureKey {
            sig: signature,
            key: *SIGNATURE_KEY_CANONICAL,
        };
        let mut canonical = self
            .cached_signatures
            .get(&key)
            .copied()
            .unwrap_or_default();
        if canonical.is_nil() {
            canonical = self.create_canonical_signature(signature);
            self.cached_signatures.insert(key, canonical);
        }
        canonical
    }

    // Go: checker/checker.go:19759 createCanonicalSignature
    pub fn create_canonical_signature(&mut self, signature: SignatureId) -> SignatureId {
        // Create an instantiation of the signature where each unconstrained type parameter is replaced with
        // its original. When a generic class or interface is instantiated, each generic method in the class or
        // interface is instantiated with a fresh set of cloned type parameters (which we need to handle scenarios
        // where different generations of the same type parameter are in scope). This leads to a lot of new type
        // identities, and potentially a lot of work comparing those identities, so here we create an instantiation
        // that uses the original type identities for all unconstrained type parameters.
        let type_parameters = self.sig(signature).type_parameters.clone();
        let mut type_arguments = Vec::with_capacity(type_parameters.len());
        for tp in type_parameters {
            let target = self.ty(tp).target();
            if target.is_some() && self.get_constraint_of_type_parameter(target).is_nil() {
                type_arguments.push(target);
            } else {
                type_arguments.push(tp);
            }
        }
        let declaration = self.sig(signature).declaration;
        self.get_signature_instantiation(
            signature,
            &type_arguments,
            is_in_js_file(declaration),
            &[], /*inferredTypeParameters*/
            0,
        )
    }

    // Go: checker/checker.go:19775 getBaseSignature
    pub fn get_base_signature(&mut self, signature: SignatureId) -> SignatureId {
        if self.sig(signature).type_parameters.is_empty() {
            return signature;
        }
        let key = CachedSignatureKey {
            sig: signature,
            key: *SIGNATURE_KEY_BASE,
        };
        if let Some(&cached) = self.cached_signatures.get(&key) {
            if cached.is_some() {
                return cached;
            }
        }
        // PORT: copied only on a cache miss, like `get_erased_signature`.
        // PERF: the type parameters, the constraints and the passes use
        // stack lists (`base_constraints` and `scratch` swap after each
        // pass). The calls and their order are Go's; each mapper still keeps
        // its own `SharedList` copy.
        let type_parameters: SmallVec<[TypeId; 8]> = self
            .sig(signature)
            .type_parameters
            .iter()
            .copied()
            .collect();
        let mut constraints: SmallVec<[TypeId; 8]> = SmallVec::with_capacity(type_parameters.len());
        for &tp in &type_parameters {
            let constraint = self.get_constraint_of_type_parameter(tp);
            constraints.push(if constraint.is_some() {
                constraint
            } else {
                self.unknown_type
            });
        }
        let base_constraint_mapper = self.new_type_mapper(&type_parameters, &constraints);
        let mut base_constraints: SmallVec<[TypeId; 8]> = SmallVec::new();
        self.instantiate_types_into(
            &type_parameters,
            base_constraint_mapper,
            &mut base_constraints,
        );
        let mut scratch: SmallVec<[TypeId; 8]> = SmallVec::new();
        // Run the immediate constraint mapper N-1 times so non-circular interdependent type parameters
        // resolve to their external dependencies without adding an extra expansion step for self-recursive constraints.
        for _ in 0..type_parameters.len() - 1 {
            self.instantiate_types_into(&base_constraints, base_constraint_mapper, &mut scratch);
            std::mem::swap(&mut base_constraints, &mut scratch);
        }
        // and then apply a type eraser to remove any remaining circularly dependent type parameters
        let any_type = self.any_type;
        let eraser = self.new_array_to_single_type_mapper(&type_parameters, any_type);
        self.instantiate_types_into(&base_constraints, eraser, &mut scratch);
        std::mem::swap(&mut base_constraints, &mut scratch);
        let mapper = self.new_type_mapper(&type_parameters, &base_constraints);
        let result =
            self.instantiate_signature_ex(signature, mapper, true /*eraseTypeParameters*/);
        self.cached_signatures.insert(key, result);
        result
    }

    // Go: checker/checker.go:19803 instantiateSignatureInContextOf
    // Instantiate a generic signature in the context of a non-generic signature (section 3.8.5 in TypeScript spec)
    pub fn instantiate_signature_in_context_of(
        &mut self,
        signature: SignatureId,
        contextual_signature: SignatureId,
        inference_context: InferenceContextId,
        compare_types: Option<TypeComparer>,
    ) -> SignatureId {
        let type_parameters = self.get_type_parameters_for_mapper(signature);
        let context = self.new_inference_context(
            &type_parameters,
            signature,
            InferenceFlags::NONE,
            compare_types,
        );
        // We clone the inferenceContext to avoid fixing. For example, when the source signature is <T>(x: T) => T[] and
        // the contextual signature is (...args: A) => B, we want to infer the element type of A's constraint (say 'any')
        // for T but leave it possible to later infer '[any]' back to A.
        let rest_type = self.get_effective_rest_type(contextual_signature);
        let mut mapper = MapperId::NIL;
        if inference_context.is_some() {
            if rest_type.is_some()
                && self
                    .ty(rest_type)
                    .flags
                    .intersects(TypeFlags::TYPE_PARAMETER)
            {
                mapper = self.inference_context(inference_context).non_fixing_mapper;
            } else {
                mapper = self.inference_context(inference_context).mapper;
            }
        }
        let source_signature = if mapper.is_some() {
            self.instantiate_signature(contextual_signature, mapper)
        } else {
            contextual_signature
        };
        self.apply_to_parameter_types(
            source_signature,
            signature,
            &mut |c: &mut Checker, source: TypeId, target: TypeId| {
                // Type parameters from outer context referenced by source type are fixed by instantiation of the source type
                c.infer_types(context, source, target, InferencePriority::NONE, false);
            },
        );
        if inference_context.is_nil() {
            self.apply_to_return_types(
                contextual_signature,
                signature,
                &mut |c: &mut Checker, source: TypeId, target: TypeId| {
                    c.infer_types(
                        context,
                        source,
                        target,
                        InferencePriority::RETURN_TYPE,
                        false,
                    );
                },
            );
        }
        let inferred_types = self.get_inferred_types(context);
        let declaration = self.sig(contextual_signature).declaration;
        self.get_signature_instantiation(
            signature,
            &inferred_types,
            is_in_js_file(declaration),
            &[], /*inferredTypeParameters*/
            0,
        )
    }

    // Go: checker/checker.go:19835 resolveBaseTypesOfInterface
    pub fn resolve_base_types_of_interface(&mut self, t: TypeId) {
        let declarations = self.sym(self.ty(t).symbol).declarations.clone();
        for &declaration in declarations.iter() {
            if is_interface_declaration(declaration) {
                for node in get_extends_heritage_clause_elements(declaration) {
                    let type_from_node = self.get_type_from_type_node(node);
                    let base_type = self.get_reduced_type(type_from_node);
                    if !self.is_error_type(base_type) {
                        if self.is_valid_base_type(base_type) {
                            if t != base_type && !self.has_base_type(base_type, t) {
                                // Go `append`. The list is shared, so it is
                                // rebuilt with the new element; a reentrant
                                // read sees each partial list, like Go.
                                let base_types =
                                    &mut self.ty_mut(t).as_interface_type_mut().resolved_base_types;
                                let mut appended: SmallVec<[TypeId; 8]> =
                                    SmallVec::with_capacity(base_types.len() + 1);
                                appended.extend(base_types.iter().copied());
                                appended.push(base_type);
                                *base_types = SharedList::from(&appended[..]);
                            } else {
                                self.report_circular_base_type(declaration, t);
                            }
                        } else {
                            self.error(
                                node,
                                diag::An_interface_can_only_extend_an_object_type_or_intersection_of_object_types_with_statically_known_members,
                                args![],
                            );
                        }
                    }
                }
            }
        }
    }

    // Go: checker/checker.go:19857 areAllOuterTypeParametersApplied
    pub fn are_all_outer_type_parameters_applied(&mut self, t: TypeId) -> bool {
        // An unapplied type parameter has its symbol still the same as the matching argument symbol.
        // Since parameters are applied outer-to-inner, only the last outer parameter needs to be checked.
        let outer_type_parameters = self
            .ty(t)
            .as_interface_type()
            .outer_type_parameters()
            .to_vec();
        if !outer_type_parameters.is_empty() {
            let last = outer_type_parameters.len() - 1;
            let last_type_argument = self.type_arguments_of(t)[last];
            return self.ty(outer_type_parameters[last]).symbol
                != self.ty(last_type_argument).symbol;
        }
        true
    }

    // Go: checker/checker.go:19869 reportCircularBaseType
    pub fn report_circular_base_type(&mut self, node: Node, t: TypeId) {
        let type_string = self.type_to_string_ex(
            t,
            Node::NIL,
            TypeFormatFlags::WRITE_ARRAY_AS_GENERIC_TYPE,
            None,
        );
        self.error(
            node,
            diag::Type_0_recursively_references_itself_as_a_base_type,
            args![type_string],
        );
    }
}

/// PERF (propfilt1): 256-bit filters of the member names of the 4 types
/// that `get_property_of_type_ex` falls back to, 2 bits for each name
/// (`augment_filter_bits`). No Go counterpart.
#[derive(Clone, Copy, Default)]
pub struct AugmentFilters {
    /// For each slot (`AUGMENT_FUNCTION` and the others), the type that
    /// `bits` holds the names of. Nil while the slot has no filter.
    pub of: [TypeId; 4],
    pub bits: [[u64; 4]; 4],
    /// The union of the 4 filters, once all 4 are built.
    pub all: Option<[u64; 4]>,
}

/// The slots of `Checker::augment_filters`: Go `globalFunctionType`,
/// `globalCallableFunctionType`, `globalNewableFunctionType` and
/// `globalObjectType`.
pub const AUGMENT_FUNCTION: usize = 0;
pub const AUGMENT_CALLABLE: usize = 1;
pub const AUGMENT_NEWABLE: usize = 2;
pub const AUGMENT_OBJECT: usize = 3;

/// The two bits of `name` in an `AugmentFilters` filter, from its id.
/// Equal texts have one id.
#[inline]
fn augment_filter_bits(name: &Name) -> (usize, usize) {
    let mixed = name.id().wrapping_mul(0x9E37_79B9);
    ((mixed >> 24) as usize, ((mixed >> 16) & 255) as usize)
}

/// False when `filter` rejects the name with `bits`.
#[inline]
fn augment_filter_has(filter: &[u64; 4], (a, b): (usize, usize)) -> bool {
    filter[a >> 6] & (1 << (a & 63)) != 0 && filter[b >> 6] & (1 << (b & 63)) != 0
}

#[cfg(test)]
mod augment_filter_tests {
    use super::*;

    /// The types of the type aliases of `a.ts`, in source order. Each
    /// scenario declares the same 10 aliases, so every lookup runs on a
    /// plain object, a callable, a newable, a type with both signatures,
    /// a class constructor, `Function`, `Object`, an array, `{}` and a
    /// primitive (its apparent type).
    const ALIASES: &str = r#"
type T0 = { a: number };
type T1 = () => void;
type T2 = new () => {};
type T3 = { (): void; new (): {} };
type T4 = typeof K;
type T5 = Function;
type T6 = Object;
type T7 = string[];
type T8 = {};
type T9 = string;
"#;

    /// Loads `files` with the `compilerOptions` body `options`, checks
    /// `a.ts`, and calls `f` on its checker with the codes of its
    /// diagnostics and the types of its type aliases. Each call writes its
    /// files in a temp dir of its own.
    fn with_checked_a<R: Send + 'static>(
        files: &[(&str, String)],
        options: &str,
        f: impl FnOnce(&mut Checker, Vec<i32>, Vec<TypeId>) -> R + Send + 'static,
    ) -> R {
        static CALLS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let call = CALLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("ts_goport_augfilter_{}_{call}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for (name, text) in files {
            std::fs::write(dir.join(name), text).unwrap();
        }
        let list: Vec<String> = files.iter().map(|(name, _)| format!("{name:?}")).collect();
        std::fs::write(
            dir.join("tsconfig.json"),
            format!(
                r#"{{ "compilerOptions": {{ {options} }}, "files": [{}] }}"#,
                list.join(", ")
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
            let ctx = crate::gostd::context::background();
            let codes = checker
                .get_diagnostics_exported(&ctx, file)
                .iter()
                .map(|d| d.code)
                .collect();
            let types = file
                .statements()
                .iter()
                .filter(|s| s.kind() == SyntaxKind::TypeAliasDeclaration)
                .map(|alias| checker.get_type_from_type_node(alias.type_()))
                .collect();
            f(checker, codes, types)
        });
        drop(scope);
        crate::program::release_program(program);
        result
    }

    /// Go `getPropertyOfTypeEx(t, name, false, false)` for a type whose
    /// apparent type is an object type, with the two lookups of
    /// checker.go:19259-19274 always made.
    fn go_property_of_type(c: &mut Checker, t: TypeId, name: &Name) -> SymbolId {
        let own = c.get_property_of_type_ex(t, name, true, false);
        if own.is_some() {
            return own;
        }
        let t = c.get_reduced_apparent_type(t);
        let resolved = c.resolve_structured_type_members(t);
        let (calls, constructs) = (
            resolved.call_signatures().len(),
            resolved.construct_signatures().len(),
        );
        let function_type = if t == c.any_function_type {
            c.global_function_type
        } else if calls != 0 {
            c.global_callable_function_type
        } else if constructs != 0 {
            c.global_newable_function_type
        } else {
            TypeId::NIL
        };
        if function_type.is_some() {
            let symbol = c.get_property_of_object_type_key(function_type, TableKey::Name(name));
            if symbol.is_some() {
                return symbol;
            }
        }
        let object = c.global_object_type;
        c.get_property_of_object_type_key(object, TableKey::Name(name))
    }

    /// The names to look up: the members of the 4 filter types, every
    /// global and the members and exports of each, `extra`, and 256 names
    /// that no declaration has.
    fn names_to_try(c: &Checker, extra: &[&str]) -> Vec<Name> {
        let mut seen = FxHashSet::default();
        let mut names = Vec::new();
        let mut add = |name: Name| {
            if seen.insert(name.id()) {
                names.push(name);
            }
        };
        for g in [
            c.global_function_type,
            c.global_callable_function_type,
            c.global_newable_function_type,
            c.global_object_type,
        ] {
            for (name, _) in c.symbols.entries(c.ty(g).as_structured_type().members) {
                add(name);
            }
        }
        for (name, symbol) in c.symbols.entries(c.globals) {
            add(name);
            for table in [c.sym(symbol).members, c.sym(symbol).exports] {
                for (member, _) in c.symbols.entries(table) {
                    add(member);
                }
            }
        }
        for &text in extra {
            add(Name::from(text));
        }
        for i in 0..256 {
            add(Name::from(format!("noSuchMember{i}")));
        }
        names
    }

    /// True when each slot of `augment_filters` holds the filter of its
    /// type, and the union is built.
    fn all_filters_built(c: &Checker) -> bool {
        c.augment_filters.of == c.augment_globals() && c.augment_filters.all.is_some()
    }

    /// Resolves the 4 filter types by lookups that Go makes too, then
    /// misses once on each kind of type, so the 4 filters are built.
    fn build_filter(c: &mut Checker, types: &[TypeId]) {
        let (plain, callable, newable) = (types[0], types[1], types[2]);
        let miss = Name::from("noSuchMember0");
        for t in [callable, newable, c.any_function_type, plain] {
            c.get_property_of_type(t, "toString");
            assert!(c.get_property_of_type_ex(t, &miss, false, false).is_nil());
        }
        assert!(all_filters_built(c), "the filters are not built");
    }

    /// Every name on every alias type (and on Go's any function type) gives
    /// Go's symbol. Returns how many names the filters reject on a
    /// callable, so a test can check that it skipped lookups.
    fn assert_lookups_match_go(c: &mut Checker, types: &[TypeId], extra: &[&str]) -> usize {
        build_filter(c, types);
        let names = names_to_try(c, extra);
        let mut all = types.to_vec();
        all.push(c.any_function_type);
        for &t in &all {
            let apparent = c.get_reduced_apparent_type(t);
            assert!(c.ty(apparent).flags.intersects(TypeFlags::OBJECT));
            for name in &names {
                let want = go_property_of_type(c, t, name);
                let got = c.get_property_of_type_ex(t, name, false, false);
                assert_eq!(got, want, "{:?} on type {t:?}", name.as_str());
            }
        }
        assert!(all_filters_built(c), "a filter was dropped");
        names
            .iter()
            .filter(|name| c.augment_lookups_miss(types[1], name))
            .count()
    }

    /// True when `name` on `t` is found and declared in `a.ts` or `b.ts`.
    fn found_in_test_file(c: &mut Checker, t: TypeId, name: &str) -> bool {
        let name = Name::from(name);
        let symbol = c.get_property_of_type_ex(t, &name, false, false);
        symbol.is_some()
            && c.sym(symbol).declarations.iter().any(|d| {
                let file_name = source_file_file_name(get_source_file_of_node(*d));
                file_name.ends_with("/a.ts") || file_name.ends_with("/b.ts")
            })
    }

    const STRICT: &str = r#""strict": true, "target": "es2020", "types": []"#;

    /// `declare global` adds a member to each of the 4 types. The filter
    /// holds them, so a plain object, a callable and a newable find them
    /// through the fallback.
    #[test]
    fn filter_keeps_global_augmentations() {
        let a = format!(
            r#"export {{}};
declare global {{
    interface Object {{ objAug(): void }}
    interface Function {{ fnAug: number }}
    interface CallableFunction {{ callAug: string }}
    interface NewableFunction {{ newAug: boolean }}
}}
declare const plain: {{ a: number }};
declare const call: () => void;
declare const ctor: new () => {{}};
declare const both: {{ (): void; new (): {{}} }};
declare class K {{ static s: number }}
declare const u: {{ a: number }} | {{ b: number }};
call.apply; ctor.bind; plain.toString;
if ("a" in u) {{}}
plain.objAug(); call.objAug(); call.fnAug; call.callAug;
ctor.newAug; ctor.fnAug; ctor.objAug(); both.callAug; K.newAug; K.fnAug;
plain.absentMember;
{ALIASES}"#
        );
        let (codes, rejected, found) = with_checked_a(&[("a.ts", a)], STRICT, |c, codes, types| {
            let found = [
                (types[0], "objAug"),
                (types[1], "objAug"),
                (types[1], "fnAug"),
                (types[1], "callAug"),
                (types[2], "newAug"),
                (types[2], "fnAug"),
                (types[4], "newAug"),
                (types[5], "objAug"),
                (types[9], "objAug"),
            ]
            .map(|(t, name)| found_in_test_file(c, t, name));
            let extra = ["objAug", "fnAug", "callAug", "newAug", "absentMember"];
            (codes, assert_lookups_match_go(c, &types, &extra), found)
        });
        // Go N gives only TS2339 for `absentMember`.
        assert_eq!(codes, [2339]);
        assert_eq!(found, [true; 9]);
        assert!(rejected > 128, "the filter rejected {rejected} names");
    }

    /// A script merges members into the global interfaces, also with late
    /// bound names (`[fnKey]`, `[objKey]`). The filter holds the merged
    /// tables.
    #[test]
    fn filter_keeps_merged_and_late_bound_members() {
        let b = r#"interface Object { scriptObj: number }
interface Function { scriptFn(): void }
declare const fnKey: unique symbol;
declare const objKey: unique symbol;
interface Function { [fnKey]: number }
interface Object { [objKey]: string }
"#;
        let a = format!(
            r#"export {{}};
declare const plain: {{ a: number }};
declare const call: () => void;
declare const ctor: new () => {{}};
declare class K {{ static s: number }}
call.apply; ctor.bind; plain.toString;
const n: number = call[fnKey];
const m: number = ctor[fnKey];
const s: string = plain[objKey];
const t: string = call[objKey];
plain.scriptObj; call.scriptFn(); call.scriptObj; K.scriptFn();
{ALIASES}"#
        );
        let (codes, rejected, found) = with_checked_a(
            &[("a.ts", a), ("b.ts", b.to_string())],
            STRICT,
            |c, codes, types| {
                let found = [
                    (types[0], "scriptObj"),
                    (types[1], "scriptFn"),
                    (types[1], "scriptObj"),
                    (types[2], "scriptFn"),
                ]
                .map(|(t, name)| found_in_test_file(c, t, name));
                let extra = ["scriptObj", "scriptFn"];
                (codes, assert_lookups_match_go(c, &types, &extra), found)
            },
        );
        assert_eq!(codes, Vec::<i32>::new());
        assert_eq!(found, [true; 4]);
        assert!(rejected > 128, "the filter rejected {rejected} names");
    }

    /// With `strictBindCallApply` off, CallableFunction and NewableFunction
    /// are the Function type, and the es5 lib has no `Symbol` members.
    #[test]
    fn filter_follows_lib_es5_without_strict_bind_call_apply() {
        let a = format!(
            r#"export {{}};
declare global {{
    interface Function {{ fnAug: number }}
}}
declare const plain: {{ a: number }};
declare const call: () => void;
declare const ctor: new () => {{}};
declare class K {{ static s: number }}
call.apply; ctor.bind; plain.toString;
call.fnAug; ctor.fnAug; K.fnAug; plain.fnAug;
{ALIASES}"#
        );
        let options = r#""strict": true, "strictBindCallApply": false, "target": "es2015", "lib": ["es5"], "types": []"#;
        let (codes, rejected, same) = with_checked_a(&[("a.ts", a)], options, |c, codes, types| {
            let same = c.global_callable_function_type == c.global_function_type
                && c.global_newable_function_type == c.global_function_type;
            (codes, assert_lookups_match_go(c, &types, &["fnAug"]), same)
        });
        // Go N gives only TS2339 for `plain.fnAug`.
        assert_eq!(codes, [2339]);
        assert!(same);
        assert!(rejected > 128, "the filter rejected {rejected} names");
    }

    /// With `noLib`, the globals come from a file of the project. There
    /// NewableFunction is a type alias, so Go `getGlobalType` gives the
    /// empty object type, which has no members.
    #[test]
    fn filter_follows_no_lib_globals() {
        let globals = r#"interface Object { o1: number }
interface Function { f1: number }
interface CallableFunction extends Function { c1: number }
interface Array<T> { length: number }
interface String {}
interface Boolean {}
interface Number {}
interface RegExp {}
interface IArguments {}
type NewableFunction = { n1: number }
"#;
        let a = format!(
            r#"export {{}};
declare const plain: {{ a: number }};
declare const call: () => void;
declare const ctor: new () => {{}};
declare class K {{ static s: number }}
call.c1; call.f1; call.o1; ctor.o1; plain.o1;
ctor.f1; ctor.n1;
{ALIASES}"#
        );
        let options = r#""strict": true, "noLib": true, "target": "es2020", "types": []"#;
        let (codes, rejected, empty) = with_checked_a(
            &[("a.ts", a), ("globals.d.ts", globals.to_string())],
            options,
            |c, codes, types| {
                let empty = c.global_newable_function_type == c.empty_object_type;
                (
                    codes,
                    assert_lookups_match_go(c, &types, &["o1", "f1", "c1", "n1"]),
                    empty,
                )
            },
        );
        // Go N gives TS2339 for `ctor.f1` and `ctor.n1` (and TS2316 in
        // globals.d.ts).
        assert_eq!(codes, [2339, 2339]);
        assert!(empty);
        assert!(rejected > 128, "the filter rejected {rejected} names");
    }

    /// When the members of Function are set again (the outer resolution
    /// of a nested one, here with a member added to the declared table),
    /// its filter is built again from the new table. The `get_base_types`
    /// reset drops the filter of its type.
    #[test]
    fn filter_follows_members_changes() {
        let a = format!(
            r#"export {{}};
declare class K {{ static s: number }}
{ALIASES}"#
        );
        let (added_found, reset_dropped) = with_checked_a(&[("a.ts", a)], STRICT, |c, _, types| {
            build_filter(c, &types);
            let function = c.global_function_type;
            // A name that the filters reject (ids, and so bits, depend on
            // intern order).
            let any_function = c.any_function_type;
            let added = (0..)
                .map(|i| Name::from(format!("addedLater{i}")))
                .find(|name| c.augment_lookups_miss(any_function, name))
                .unwrap();
            let table = c
                .symbols
                .clone_table(c.ty(function).as_interface_type().declared_members);
            let symbol = c.new_symbol(SymbolFlags::PROPERTY, &added);
            c.symbols.set(table, &added, symbol);
            c.ty_mut(function).as_interface_type_mut().declared_members = table;
            let flags = c
                .ty(function)
                .object_flags
                .without(ObjectFlags::MEMBERS_RESOLVED);
            c.ty_mut(function).object_flags = flags;
            c.resolve_structured_type_members(function);
            assert!(all_filters_built(c));
            assert!(!c.augment_lookups_miss(any_function, &added));
            let added_found =
                c.get_property_of_type_ex(any_function, &added, false, false) == symbol;

            build_filter(c, &types);
            let callable = c.global_callable_function_type;
            c.ty_mut(callable)
                .as_interface_type_mut()
                .base_types_resolved = false;
            c.ty_mut(callable)
                .as_interface_type_mut()
                .resolved_base_types = SharedList::default();
            c.get_base_types(callable);
            let reset_dropped = c.augment_filters.of[AUGMENT_CALLABLE].is_nil()
                && c.augment_filters.all.is_none()
                && !c
                    .ty(callable)
                    .object_flags
                    .intersects(ObjectFlags::MEMBERS_RESOLVED);
            assert_lookups_match_go(c, &types, &[added.as_str()]);
            (added_found, reset_dropped)
        });
        assert!(added_found);
        assert!(reset_dropped);
    }

    /// While NewableFunction is not resolved, misses on a callable and on
    /// a plain object use the filters of the resolved types, with no union,
    /// and build nothing again. A miss on a newable makes Go's lookups,
    /// which resolve NewableFunction and so build only its filter, and the
    /// union.
    #[test]
    fn filter_of_each_type_is_built_once() {
        let a = r#"export {};
type T0 = { a: number };
type T1 = () => void;
type T2 = new () => {};
"#;
        with_checked_a(&[("a.ts", a.to_string())], STRICT, |c, _, types| {
            let (plain, callable, newable) = (types[0], types[1], types[2]);
            let newable_function = c.global_newable_function_type;
            let resolved = |c: &Checker| {
                c.ty(newable_function)
                    .object_flags
                    .intersects(ObjectFlags::MEMBERS_RESOLVED)
            };
            assert!(!resolved(c), "NewableFunction is resolved too early");
            let miss = Name::from("noSuchMember0");
            for t in [callable, plain] {
                c.get_property_of_type(t, "toString");
                assert!(c.get_property_of_type_ex(t, &miss, false, false).is_nil());
            }
            let of = c.augment_filters.of;
            assert_eq!(of[AUGMENT_CALLABLE], c.global_callable_function_type);
            assert_eq!(of[AUGMENT_OBJECT], c.global_object_type);
            assert!(of[AUGMENT_NEWABLE].is_nil());
            assert!(c.augment_filters.all.is_none());
            // A new build would write the real bits over these.
            for slot in [AUGMENT_CALLABLE, AUGMENT_OBJECT] {
                c.augment_filters.bits[slot] = [!0; 4];
            }
            for i in 0..64 {
                let miss = Name::from(format!("noSuchMember{i}"));
                for t in [plain, callable] {
                    assert!(c.get_property_of_type_ex(t, &miss, false, false).is_nil());
                }
            }
            assert!(!resolved(c));
            assert!(
                c.get_property_of_type_ex(newable, &miss, false, false)
                    .is_nil()
            );
            assert!(
                resolved(c),
                "the newable miss did not resolve NewableFunction"
            );
            assert!(all_filters_built(c));
            for slot in [AUGMENT_CALLABLE, AUGMENT_OBJECT] {
                assert_eq!(
                    c.augment_filters.bits[slot], [!0; 4],
                    "slot {slot} was built again"
                );
            }
        });
    }

    /// The union waits for all 4 types (R183 reviewer item 2).
    /// `initialize_checker` sets Object before Function, CallableFunction
    /// and NewableFunction (Go checker.go:1364-1367). A filter of Object
    /// built between them must not make a union of Object's names alone:
    /// once Function is set, the lookups of its names would skip it.
    #[test]
    fn union_waits_for_all_4_types() {
        let a = r#"export {};
type T0 = { a: number };
type T1 = () => void;
type T2 = new () => {};
"#;
        let (early_union, skipped) =
            with_checked_a(&[("a.ts", a.to_string())], STRICT, |c, _, types| {
                build_filter(c, &types);
                let globals = c.augment_globals();
                c.global_function_type = TypeId::NIL;
                c.global_callable_function_type = TypeId::NIL;
                c.global_newable_function_type = TypeId::NIL;
                c.augment_filters = AugmentFilters::default();
                c.build_augment_filter(globals[AUGMENT_OBJECT]);
                let early_union = c.augment_filters.all.is_some();
                c.global_function_type = globals[AUGMENT_FUNCTION];
                c.global_callable_function_type = globals[AUGMENT_CALLABLE];
                c.global_newable_function_type = globals[AUGMENT_NEWABLE];
                let function = c.ty(globals[AUGMENT_FUNCTION]).as_structured_type().members;
                let names: Vec<Name> = c
                    .symbols
                    .iter_names(function)
                    .map(|(name, _)| name)
                    .collect();
                let any_function = c.any_function_type;
                let skipped: Vec<String> = names
                    .iter()
                    .filter(|name| c.augment_lookups_miss(any_function, name))
                    .map(|name| name.as_str().to_string())
                    .collect();
                assert_lookups_match_go(c, &types, &[]);
                (early_union, skipped)
            });
        assert!(!early_union, "a union of Object's names alone");
        assert_eq!(skipped, Vec::<String>::new());
    }

    /// The source of 40 names `{prefix}0` to `{prefix}39` that a module
    /// augmentation adds to `iface`, and of an assignment of `value` to a
    /// type with each name, which needs the name on the apparent type of
    /// `value`.
    fn late_members(iface: &str, prefix: &str, value: &str) -> (String, String) {
        let decls = (0..40)
            .map(|i| format!("  interface {iface} {{ {prefix}{i}: number }}\n"))
            .collect();
        let uses = (0..40)
            .map(|i| format!("const s{i}: {{ {prefix}{i}: number }} = {value};\n"))
            .collect();
        (decls, uses)
    }

    /// Checks `a.ts` of `files` with `options` and the commonjs module.
    /// Gives the codes of `a.ts`, the types that the 4 slots hold, the 4
    /// types, and whether the union is built.
    fn check_augmentation(
        files: &[(&str, String)],
        options: &str,
    ) -> (Vec<i32>, [TypeId; 4], [TypeId; 4], bool) {
        let options = format!(r#"{options}, "module": "commonjs""#);
        with_checked_a(files, &options, |c, codes, _| {
            let filters = c.augment_filters;
            (
                codes,
                filters.of,
                c.augment_globals(),
                filters.all.is_some(),
            )
        })
    }

    /// `x.ts` with an `export=` whose check resolves Object (and with
    /// `calls`, the function types), so their filters are built in the
    /// module augmentation loop of `initialize_checker`.
    fn x_ts(calls: bool) -> (&'static str, String) {
        let text = if calls {
            r#"declare function f(): void;
declare class C {}
declare const p: { a: number };
const o = { r1: f.call, r2: C.apply, r3: p.toString, r4: p.missingX };
export = o.r1;
"#
        } else {
            r#"declare const p: { a: number };
const o = { r3: p.toString };
export = o.r3;
"#
        };
        ("x.ts", text.to_string())
    }

    fn g_ts() -> (&'static str, String) {
        ("g.ts", "export = globalThis;\n".to_string())
    }

    /// The augmentation of `./x` resolves Object, then the augmentation of
    /// `./g` (`export = globalThis`) adds 3 names to the members of the
    /// merged Object in place (Go `mergeSymbol`, checker.go:1484 and
    /// :14414). The filter is built again with them.
    #[test]
    fn filter_follows_augmentation_merged_into_object() {
        let a = r#"import "./x";
import "./g";
declare global { interface Object { a1: number } }
declare module "./x" { interface Q {} }
declare module "./g" { interface Object { late1: number; late2: number; late3: number } }
declare const p2: { b: number };
const s1: { late1: number } = p2;
const s2 = p2["late2"];
const s3: { late3: number } = p2;
"#;
        let files = [("a.ts", a.to_string()), x_ts(false), g_ts()];
        let (codes, of, globals, _) = check_augmentation(&files, STRICT);
        // Go N gives only TS2671 for `./x`.
        assert_eq!(codes, [2671]);
        assert_eq!(of[AUGMENT_OBJECT], globals[AUGMENT_OBJECT]);
    }

    /// As above with 40 names, after the check of `./x` resolved all 4
    /// types, so lookups test the union.
    #[test]
    fn union_follows_augmentation_merged_into_object() {
        let (decls, uses) = late_members("Object", "zq", "p2");
        let reads: Vec<String> = (0..40).map(|i| format!("p2[\"zq{i}\"]")).collect();
        let a = format!(
            r#"import "./x";
import "./g";
declare global {{
  interface Object {{ a1: number }}
  interface Function {{ f1: number }}
}}
declare module "./x" {{ interface Q {{ q: number }} }}
declare module "./g" {{
{decls}}}
declare const p2: {{ b: number }};
export const r = [{}];
{uses}"#,
            reads.join(", ")
        );
        let files = [("a.ts", a), x_ts(true), g_ts()];
        let (codes, of, globals, all) = check_augmentation(&files, STRICT);
        // Go N gives only TS2671 for `./x` (and TS2339 in `x.ts`).
        assert_eq!(codes, [2671]);
        assert_eq!(of, globals);
        assert!(all);
    }

    /// As above when only Object is resolved, so lookups test its filter
    /// alone.
    #[test]
    fn object_filter_follows_augmentation_merged_into_object() {
        let (decls, uses) = late_members("Object", "zq", "p2");
        let a = format!(
            r#"import "./x";
import "./g";
declare global {{ interface Object {{ a1: number }} }}
declare module "./x" {{ interface Q {{ q: number }} }}
declare module "./g" {{
{decls}}}
declare const p2: {{ b: number }};
{uses}"#
        );
        let files = [("a.ts", a), x_ts(false), g_ts()];
        let (codes, of, globals, all) = check_augmentation(&files, STRICT);
        // Go N gives only TS2671 for `./x`.
        assert_eq!(codes, [2671]);
        assert_eq!(of[AUGMENT_OBJECT], globals[AUGMENT_OBJECT]);
        assert!(!all);
    }

    /// The augmentation adds 40 names to Function. With the es5 lib and
    /// `strictBindCallApply` off, the 3 function slots are all Function.
    #[test]
    fn function_filter_follows_augmentation_merged_into_function() {
        let (decls, uses) = late_members("Function", "fq", "f2");
        let a = format!(
            r#"import "./x";
import "./g";
declare global {{ interface Function {{ f1: number }} }}
declare module "./x" {{ interface Q {{ q: number }} }}
declare module "./g" {{
{decls}}}
declare function f2(): void;
{uses}"#
        );
        let (_, x) = x_ts(true);
        let x = x.replace("export =", "const t1: { fx1: number } = f;\nexport =");
        let files = [("a.ts", a), ("x.ts", x), g_ts()];
        let options = r#""strict": true, "strictBindCallApply": false, "target": "es2015", "lib": ["es5"], "types": []"#;
        let (codes, of, globals, _) = check_augmentation(&files, options);
        // Go N gives only TS2671 for `./x` (and TS2322 and TS2339 in
        // `x.ts`).
        assert_eq!(codes, [2671]);
        assert_eq!(globals[..3], [globals[AUGMENT_FUNCTION]; 3]);
        assert_eq!(of[..3], globals[..3]);
    }

    /// The augmented module is ambient: `declare module "glob" { export =
    /// globalThis; }`.
    #[test]
    fn filter_follows_augmentation_of_ambient_module() {
        let (decls, uses) = late_members("Object", "zq", "p2");
        let a = format!(
            r#"import "./x";
import "glob";
declare global {{ interface Object {{ a1: number }} }}
declare module "./x" {{ interface Q {{ q: number }} }}
declare module "glob" {{
{decls}}}
declare const p2: {{ b: number }};
{uses}"#
        );
        let ambient = r#"declare module "glob" { export = globalThis; }"#;
        let files = [("amb.d.ts", ambient.to_string()), ("a.ts", a), x_ts(false)];
        let (codes, of, globals, _) = check_augmentation(&files, STRICT);
        // Go N gives only TS2671 for `./x`.
        assert_eq!(codes, [2671]);
        assert_eq!(of[AUGMENT_OBJECT], globals[AUGMENT_OBJECT]);
    }

    /// Skeptic program e25 (propfilt1-skeptic-c, R183 reviewer item 3).
    /// The merge of `./x` resolves Object (its `export=` reads
    /// `p.toString`), the merge of `./g` (`export = globalThis`) adds
    /// `late1` and `late2` to the members of Object in place, and the merge
    /// of `./y` then reads them. So the filter must be built again after
    /// each merge, not once after the loop (mutant M2).
    #[test]
    fn filter_follows_each_module_augmentation_merge() {
        let a = r#"import "./x";
import "./y";
import "./g";
declare global { interface Object { a1: number } }
declare module "./x" { interface Q {} }
declare module "./g" { interface Object { late1: number; late2: number } }
declare module "./y" { interface R {} }
export {};
"#;
        let x = r#"declare const p: { a: number };
const o = { s: p.toString };
export = o.s;
"#;
        let y = r#"declare const q: { a: number };
const o = { s: q["late1"], t: q satisfies { late2: number }, u: q["missingY"] };
export = o.s;
"#;
        let files = [
            ("a.ts", a.to_string()),
            ("x.ts", x.to_string()),
            ("y.ts", y.to_string()),
            g_ts(),
        ];
        let options = format!(r#"{STRICT}, "module": "commonjs""#);
        let (a_codes, y_codes) = with_checked_a(&files, &options, |c, codes, _| {
            let y = c
                .files
                .iter()
                .copied()
                .find(|&file| source_file_file_name(file).ends_with("/y.ts"))
                .expect("y.ts is not in the program");
            let ctx = crate::gostd::context::background();
            let y_codes: Vec<i32> = c
                .get_diagnostics_exported(&ctx, y)
                .iter()
                .map(|d| d.code)
                .collect();
            (codes, y_codes)
        });
        // Go N (tsgo-oracle-673a5f17d713): TS2671 for `./x` and `./y`, and
        // in `y.ts` only TS7053 for `missingY`.
        assert_eq!(a_codes, [2671, 2671]);
        assert_eq!(y_codes, [7053]);
    }
}
