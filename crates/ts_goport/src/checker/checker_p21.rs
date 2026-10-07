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
            TypeSystemPropertyName::TYPE => self
                .value_symbol_links
                .get(as_symbol(r.target))
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
            TypeSystemPropertyName::WRITE_TYPE => self
                .value_symbol_links
                .get(as_symbol(r.target))
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
            // PORT: not in the pinned Go (pingdotgg/ts-rust#20). See
            // `get_resolving_members`.
            if !self
                .ty(t)
                .object_flags
                .intersects(ObjectFlags::MEMBERS_RESOLVED)
            {
                let symbol = self.get_declared_property_of_resolving_type(t, name);
                if symbol.is_some() {
                    return symbol;
                }
            }
            self.resolve_structured_type_members(t);
            let members = self.ty(t).as_structured_type().members;
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
            self.resolving_members_stack.push(ResolvingMembers {
                t,
                declared_members,
                members,
            });
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
            self.resolving_members_stack.pop();
            let call_signature_count = call_signatures.len();
            self.set_structured_type_members_ex(
                t,
                members,
                SymbolTable::NIL,
                SharedList::concat(call_signatures, construct_signatures),
                call_signature_count,
                index_infos,
            );
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
    }

    // While `resolve_object_type_members` adds the inherited members of a
    // class or interface type, or a reference to one, the type arguments of
    // its base types may refer back to the type. Inherited members never
    // replace a declared value member (`add_inherited_members`), so a
    // property the type declares itself is known before its base types are
    // resolved. This lets such a circular reference see the declared
    // properties without resolving the members of the type again (which
    // repeats the same instantiations until the depth limit) and without
    // storing a partial resolution on the type.
    // PORT: not in the pinned Go (pingdotgg/ts-rust#20, microsoft/TypeScript#64605).
    fn get_resolving_members(&self, t: TypeId) -> Option<&ResolvingMembers> {
        self.resolving_members_stack.iter().rev().find(|r| r.t == t)
    }

    /// The property with the given name that `t` declares itself, if `t` is
    /// a type whose members are being resolved.
    pub fn get_declared_property_of_resolving_type<'a>(
        &self,
        t: TypeId,
        name: impl Into<TableKey<'a>>,
    ) -> SymbolId {
        if let Some(r) = self.get_resolving_members(t) {
            let name = name.into();
            let symbol = self.symbols.get_key(r.declared_members, name);
            if symbol.is_some() && self.sym(symbol).flags.intersects(SymbolFlags::VALUE) {
                return self.symbols.get_key(r.members, name);
            }
        }
        SymbolId::NIL
    }

    /// Whether `t` is a type whose members are being resolved and that
    /// declares a property itself.
    pub fn resolving_type_declares_properties(&self, t: TypeId) -> bool {
        self.get_resolving_members(t).is_some_and(|r| {
            self.symbols.iter(r.declared_members).any(|(name, symbol)| {
                self.sym(symbol).flags.intersects(SymbolFlags::VALUE)
                    && !is_reserved_member_name(name)
            })
        })
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

/// The declared members of a type whose base types
/// `resolve_object_type_members` is resolving, and its members table.
pub(crate) struct ResolvingMembers {
    t: TypeId,
    declared_members: SymbolTable,
    members: SymbolTable,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The code and 1-based line of each semantic diagnostic of `source`, a
    /// strict es2020 file with no `types`. `first` runs on the checker of the
    /// file before it reports them.
    fn diagnostics(
        source: &'static str,
        first: impl FnOnce(&mut Checker, Node) + Send + 'static,
    ) -> Vec<(i32, usize)> {
        static CALLS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let call = CALLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "ts_goport_resolving_members_{}_{call}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.ts"), source).unwrap();
        std::fs::write(
            dir.join("tsconfig.json"),
            r#"{ "compilerOptions": { "strict": true, "target": "es2020", "types": [] }, "files": ["a.ts"] }"#,
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
            first(checker, file);
            crate::program::get_semantic_diagnostics_with_checker(
                &crate::gostd::context::background(),
                checker,
                file,
            )
            .iter()
            .map(|d| {
                let line = 1 + source[..d.pos() as usize].matches('\n').count();
                (d.code(), line)
            })
            .collect()
        });
        drop(scope);
        crate::program::release_program(program);
        result
    }

    /// pingdotgg/ts-rust#20, microsoft/TypeScript#64605. The type arguments
    /// of a base type read a property that the derived type declares itself
    /// (`Json["_zod"]`, `JsonInternals["input"]`, the `optin` of
    /// `RecordInternals<Json>`) while the members of the derived type are
    /// being resolved. tsc 7.0.2 and Go before microsoft/TypeScript#64372
    /// report nothing. The pinned Go resolves the members again, repeats the
    /// same instantiations and reports TS5115.
    #[test]
    fn base_type_arguments_read_declared_members_of_the_derived_type() {
        // The example of microsoft/TypeScript#64605.
        const ISSUE: &str = r#"
interface Internals<I = unknown> { input: I }
interface Schema { _zod: Internals }
type RecordInput<V extends Schema> = V extends unknown ? Record<string, V["_zod"]["input"]> : never;
interface RecordSchema<V extends Schema> extends Schema { _zod: Internals<RecordInput<V>> }
interface UnionInternals<T extends readonly Schema[]> extends Internals<T[number]["_zod"]["input"]> {}
interface UnionSchema<T extends readonly Schema[]> extends Schema { _zod: UnionInternals<T> }
type JsonValue = { [k: string]: JsonValue };
type _Json = UnionSchema<[RecordSchema<Json>]>;
type _JsonInternals = _Json["_zod"];
interface JsonInternals extends _JsonInternals { input: JsonValue }
interface Json extends _Json { _zod: JsonInternals }
"#;
        // The shape of the Zod 4.6 `z.json()` types: the record internals
        // declare `optin`, and the record input asks for it.
        const ZOD: &str = r#"
interface Internals<O = unknown, I = unknown> { output: O; input: I; optin?: "optional" | undefined }
interface Schema<Z extends Internals = Internals> { _zod: Z }
type OptionalIn = { _zod: { optin: "optional" } };
type RecordInput<V extends Schema> = [V] extends [OptionalIn] ? Partial<Record<string, V["_zod"]["input"]>> : Record<string, V["_zod"]["input"]>;
interface RecordInternals<V extends Schema> extends Internals<unknown, RecordInput<V>> { optin?: "optional" | undefined }
interface RecordSchema<V extends Schema> extends Schema<RecordInternals<V>> { _zod: RecordInternals<V> }
type IsOptionalIn<T extends Schema> = T extends OptionalIn ? true : false;
interface UnionInternals<T extends readonly Schema[]> extends Internals {
    optin: IsOptionalIn<T[number]> extends false ? "optional" | undefined : "optional";
}
interface UnionSchema<T extends readonly Schema[]> extends Schema<UnionInternals<T>> { _zod: UnionInternals<T> }
type _Json = UnionSchema<[RecordSchema<Json>]>;
interface JsonInternals extends UnionInternals<[RecordSchema<Json>]> { input: unknown }
interface Json extends _Json { _zod: JsonInternals }
export const r: RecordInput<Json> = { a: 1 };
export const o: Json["_zod"]["optin"] = undefined;
export const bad: Json["_zod"]["optin"] = "required";
"#;
        assert_eq!(diagnostics(ISSUE, |_, _| {}), vec![]);
        assert_eq!(diagnostics(ZOD, |_, _| {}), vec![(2322, 18)]);
    }

    /// What microsoft/TypeScript#64372 fixed stays fixed. In its fourslash
    /// test `noGhostErrors` (microsoft/TypeScript#62180) the type of the
    /// `parent` getter comes first, and then there is no error and the
    /// `output` of `Category` has its properties. A base type argument that
    /// needs an inherited member (`keyof PersonModel` in
    /// `keyofGenericExtendingClassDoubleLayer`) is still infinitely circular.
    #[test]
    fn inherited_members_are_not_read_before_the_base_types_resolve() {
        const NO_GHOST_ERRORS: &str = r#"
interface ZodType<T> {
  optional: "true" | "false";
  output: T;
}
interface ZodString extends ZodType<string> {
  optional: "false";
}
type ZodShape = Record<string, any>;
type Prettify<T> = { [K in keyof T]: T[K] } & {};
type InferObjectType<Shape extends ZodShape> = Prettify<
  {
    [k in keyof Shape as Shape[k] extends { optional: "true" }
      ? k
      : never]?: Shape[k]["output"];
  } & {
    [k in keyof Shape as Shape[k] extends { optional: "true" }
      ? never
      : k]: Shape[k]["output"];
  }
>;
interface ZodObject<T extends ZodShape> extends ZodType<InferObjectType<T>> {
  optional: "false";
}
interface ZodOptional<T extends ZodType<any>>
  extends ZodType<T["output"] | undefined> {
  optional: "true";
}
declare function object<T extends ZodShape>(shape: T): ZodObject<T>;
declare function string(): ZodString;
declare function optional<T extends ZodType<any>>(schema: T): ZodOptional<T>;
const Category = object({
  name: string(),
  get parent() {
    return optional(Category);
  },
});
export const output = Category.output;
export const name: string = output.name;
export const bad: number = output.name;
"#;
        const DOUBLE_LAYER: &str = r#"
class Model<Attributes = any> {
    public createdAt!: Date;
}
type ModelAttributes<T> = Omit<T, keyof Model>;
class AutoModel<T> extends Model<ModelAttributes<T>> {}
class PersonModel extends AutoModel<PersonModel> {
    public age!: number;
}
"#;
        let parent_first = |checker: &mut Checker, file: Node| {
            let category = file
                .statements()
                .iter()
                .find(|s| s.kind() == SyntaxKind::VariableStatement)
                .expect("const Category");
            let declaration = category.declaration_list().declarations().nodes().get(0);
            let shape = declaration.initializer().arguments().get(0);
            let parent = shape.properties().get(1);
            let symbol = checker.get_symbol_of_declaration(parent);
            checker.get_type_of_symbol(symbol);
        };
        assert_eq!(diagnostics(NO_GHOST_ERRORS, parent_first), vec![(2322, 40)]);
        assert_eq!(diagnostics(NO_GHOST_ERRORS, |_, _| {}), vec![(2322, 40)]);
        assert_eq!(diagnostics(DOUBLE_LAYER, |_, _| {}), vec![(5115, 7)]);
    }
}
