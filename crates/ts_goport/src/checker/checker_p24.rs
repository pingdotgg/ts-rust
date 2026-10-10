//! Port of `checker/checker.go` lines 21264-22202: mixins, union and
//! intersection properties, apparent and reduced types, type arguments and
//! defaults, named members, and the core of type instantiation.

use crate::checker::utilities_p1::{NO_SORT_ORDER, SymbolSortKey};
use crate::prelude::*;
use smallvec::SmallVec;

/// Reusable buffers of `get_named_members` (the `Checker` field
/// `named_members_scratch`), so a call allocates only its result.
#[derive(Default)]
pub(crate) struct NamedMembersScratch {
    /// Candidate members, then the sorted result.
    symbols: Vec<SymbolId>,
    /// Sort keys: contained members first when the container is a class or
    /// interface, else all members.
    keys: Vec<SymbolSortKey>,
    /// Sort keys of the members that are not contained.
    other_keys: Vec<SymbolSortKey>,
    /// Ranges of the container's declarations, read once per call.
    container_ranges: Vec<TextRange>,
}

/// The `get_named_members` order of the member tables that
/// `instantiate_symbol_table` makes from one declared member table, for one
/// container (the `Checker` field `named_members_orders`, see
/// `get_named_members_of_instantiation`).
#[derive(Clone)]
pub(crate) struct NamedMembersOrder {
    /// Entry count of such a table.
    len: u32,
    /// Table positions of the result, in result order.
    positions: Rc<[u32]>,
}

/// `is_reserved_member_name` of a symbol table key.
// PERF: a reserved name starts with `INTERNAL_SYMBOL_NAME_PREFIX`, and
// exactly those names have the `Name::is_internal` bit (set from the text
// at intern time), so a normal name reads no text.
#[inline]
fn is_reserved_member_name_of(name: &Name) -> bool {
    let reserved = name.is_internal() && is_reserved_member_name(name.as_str());
    debug_assert_eq!(reserved, is_reserved_member_name(name.as_str()));
    reserved
}

impl Checker {
    // Go: checker/checker.go:21702 findMixins
    pub fn find_mixins(&mut self, types: &[TypeId]) -> (Vec<bool>, i32) {
        let mut mixin_flags: Vec<bool> = Vec::with_capacity(types.len());
        for &t in types {
            mixin_flags.push(self.is_mixin_constructor_type(t));
        }
        let mut constructor_type_count: i32 = 0;
        let mut mixin_count: i32 = 0;
        let mut first_mixin_index: i32 = -1;
        for (i, &t) in types.iter().enumerate() {
            if !self
                .get_signatures_of_type(t, SignatureKind::CONSTRUCT)
                .is_empty()
            {
                constructor_type_count += 1;
            }
            if mixin_flags[i] {
                if first_mixin_index < 0 {
                    first_mixin_index = i as i32;
                }
                mixin_count += 1;
            }
        }
        if constructor_type_count > 0 && constructor_type_count == mixin_count {
            mixin_flags[first_mixin_index as usize] = false;
            mixin_count -= 1;
        }
        (mixin_flags, mixin_count)
    }

    // Go: checker/checker.go:21724 includeMixinType
    pub fn include_mixin_type(
        &mut self,
        t: TypeId,
        types: &[TypeId],
        mixin_flags: &[bool],
        index: i32,
    ) -> TypeId {
        let mut mixed_types: Vec<TypeId> = Vec::new();
        for i in 0..types.len() {
            if i as i32 == index {
                mixed_types.push(t);
            } else if mixin_flags[i] {
                let sig = self.get_signatures_of_type(types[i], SignatureKind::CONSTRUCT)[0];
                let ret = self.get_return_type_of_signature(sig);
                mixed_types.push(ret);
            }
        }
        self.get_intersection_type(&mixed_types)
    }

    /**
     * If the given type is an object type and that type has a property by the given name,
     * return the symbol for that property. Otherwise return undefined.
     */
    // Go: checker/checker.go:21740 getPropertyOfObjectType
    pub fn get_property_of_object_type(&mut self, t: TypeId, name: &str) -> SymbolId {
        self.get_property_of_object_type_key(t, TableKey::Text(name))
    }

    /// `get_property_of_object_type` by `TableKey`. A `Name` key compares
    /// ids and hashes no text.
    pub fn get_property_of_object_type_key(&mut self, t: TypeId, name: TableKey<'_>) -> SymbolId {
        if self.ty(t).flags.intersects(TypeFlags::OBJECT) {
            let members = self.resolve_structured_type_members(t).members;
            let symbol = self.symbols.get_key(members, name);
            if symbol.is_some() && self.symbol_is_value(symbol) {
                return symbol;
            }
        }
        SymbolId::NIL
    }

    // Go: checker/checker.go:21751 getPropertyOfUnionOrIntersectionType
    pub fn get_property_of_union_or_intersection_type(
        &mut self,
        t: TypeId,
        name: &str,
        skip_object_function_property_augment: bool,
    ) -> SymbolId {
        self.get_property_of_union_or_intersection_type_key(
            t,
            TableKey::Text(name),
            skip_object_function_property_augment,
        )
    }

    /// `get_property_of_union_or_intersection_type` by `TableKey`.
    pub fn get_property_of_union_or_intersection_type_key(
        &mut self,
        t: TypeId,
        name: TableKey<'_>,
        skip_object_function_property_augment: bool,
    ) -> SymbolId {
        let prop = self.get_union_or_intersection_property_key(
            t,
            name,
            skip_object_function_property_augment,
        );
        // We need to filter out partial properties in union types
        if prop.is_some()
            && self
                .sym(prop)
                .check_flags
                .intersects(CheckFlags::READ_PARTIAL)
        {
            return SymbolId::NIL;
        }
        prop
    }

    // Return the symbol for a given property in a union or intersection type, or undefined if the property
    // does not exist in any constituent type. Note that the returned property may only be present in some
    // constituents, in which case the isPartial flag is set when the containing type is union type. We need
    // these partial properties when identifying discriminant properties, but otherwise they are filtered out
    // and do not appear to be present in the union type.
    // Go: checker/checker.go:21765 getUnionOrIntersectionProperty
    pub fn get_union_or_intersection_property(
        &mut self,
        t: TypeId,
        name: &str,
        skip_object_function_property_augment: bool,
    ) -> SymbolId {
        self.get_union_or_intersection_property_key(
            t,
            TableKey::Text(name),
            skip_object_function_property_augment,
        )
    }

    /// `get_union_or_intersection_property` by `TableKey`. The cache and the
    /// constituent lookups use the key; the stores use one `Name` (the key's
    /// own, or the text interned once).
    pub fn get_union_or_intersection_property_key(
        &mut self,
        t: TypeId,
        name: TableKey<'_>,
        skip_object_function_property_augment: bool,
    ) -> SymbolId {
        let cache = self.property_cache_table(t, skip_object_function_property_augment);
        let prop = self.symbols.get_key(cache, name);
        if prop.is_some() {
            return prop;
        }
        let prop = self.create_union_or_intersection_property(
            t,
            name,
            skip_object_function_property_augment,
        );
        if prop.is_some() {
            let name = table_key_name(name);
            self.symbols.set(cache, &name, prop);
            // Propagate an entry from the non-augmented cache to the augmented cache unless the property is partial.
            if skip_object_function_property_augment
                && !self.sym(prop).check_flags.intersects(CheckFlags::PARTIAL)
            {
                let augmented_cache = self.property_cache_table(t, false);
                // PORT: Go `if augmentedCache[name] == nil { ... = prop }` in
                // one lookup.
                self.symbols.set_if_absent(augmented_cache, &name, prop);
            }
        }
        prop
    }

    /// Go `ast.GetSymbolTable(&t.AsUnionOrIntersectionType().propertyCache)`,
    /// or of `propertyCacheWithoutFunctionPropertyAugment` when
    /// `without_augment`: the table, made when it is nil.
    // PERF: a table that exists is read with one cast and not written back.
    fn property_cache_table(&mut self, t: TypeId, without_augment: bool) -> SymbolTable {
        let u = self.ty(t).as_union_or_intersection_type();
        let table = if without_augment {
            u.property_cache_without_function_property_augment
        } else {
            u.property_cache
        };
        if table.is_some() {
            return table;
        }
        let table = self.symbols.new_table();
        let u = self.ty_mut(t).as_union_or_intersection_type_mut();
        if without_augment {
            u.property_cache_without_function_property_augment = table;
        } else {
            u.property_cache = table;
        }
        table
    }

    // Go: checker/checker.go:21789 createUnionOrIntersectionProperty
    // PORT: `name` is a `TableKey`, so a `Name` key looks up each constituent
    // by id and names the new symbol without an intern.
    pub fn create_union_or_intersection_property(
        &mut self,
        containing_type: TypeId,
        name: TableKey<'_>,
        skip_object_function_property_augment: bool,
    ) -> SymbolId {
        let mut prop_flags = SymbolFlags::NONE;
        let mut single_prop = SymbolId::NIL;
        // PORT: Go `orderedSet[*ast.Symbol]` is a list here, kept free of
        // duplicates by `ordered_symbol_set_add`. Most sets hold a few symbols.
        // PERF: `prop_set`, `index_types` and `prop_types` are short, so they
        // live on the stack up to their inline size.
        let mut prop_set: SmallVec<[SymbolId; 8]> = SmallVec::new();
        let mut prop_set_index: FxHashSet<SymbolId> = FxHashSet::default();
        let mut index_types: SmallVec<[TypeId; 4]> = SmallVec::new();
        let is_union = self.ty(containing_type).flags.intersects(TypeFlags::UNION);
        // Flags we want to propagate to the result if they exist in all source symbols
        let mut check_flags = CheckFlags::NONE;
        let mut optional_flag = SymbolFlags::NONE;
        if !is_union {
            check_flags = CheckFlags::READONLY;
            optional_flag = SymbolFlags::OPTIONAL;
        }
        let mut synthetic_flag = CheckFlags::SYNTHETIC_METHOD;
        let mut merged_instantiations = false;
        // PERF: the constituents are read from a copy of the list (no
        // element copy), not from the containing type at each step.
        let types = self.ty(containing_type).types_list();
        // PERF: Go `getApplicableIndexInfoForName(t, name)` tests
        // `isLateBoundName(name)` and makes or finds the string literal type
        // of `name` for each constituent that lacks the property. Here the
        // test is made once, and the type at the first such constituent,
        // where Go first makes it; later constituents use it.
        let late_bound_name = is_late_bound_name(name.text());
        let mut name_literal_type = TypeId::NIL;
        for &current in types.iter() {
            let t = self.get_apparent_type(current);
            if !self.is_error_type(t) && !self.ty(t).flags.intersects(TypeFlags::NEVER) {
                let prop = self.get_property_of_type_ex(
                    t,
                    name,
                    skip_object_function_property_augment,
                    false,
                );
                let modifiers;
                if prop.is_some() {
                    modifiers = self.get_declaration_modifier_flags_from_symbol(prop);
                    let write_modifiers = self
                        .get_declaration_modifier_flags_from_symbol_ex(prop, true /*isWrite*/);
                    let prop_symbol_flags = self.sym(prop).flags;
                    if prop_symbol_flags.intersects(SymbolFlags::CLASS_MEMBER) {
                        if is_union {
                            optional_flag |= prop_symbol_flags & SymbolFlags::OPTIONAL;
                        } else {
                            optional_flag = optional_flag & prop_symbol_flags;
                        }
                    }
                    if single_prop.is_nil() {
                        single_prop = prop;
                        let accessor = prop_symbol_flags & SymbolFlags::ACCESSOR;
                        prop_flags = if accessor != SymbolFlags::NONE {
                            accessor
                        } else {
                            SymbolFlags::PROPERTY
                        };
                    } else if prop != single_prop {
                        let is_instantiation =
                            self.get_target_symbol(prop) == self.get_target_symbol(single_prop);
                        // If the symbols are instances of one another with identical types - consider the symbols
                        // equivalent and just use the first one, which thus allows us to avoid eliding private
                        // members when intersecting a (this-)instantiations of a class with its raw base or another instance
                        if is_instantiation
                            && self.compare_properties(
                                single_prop,
                                prop,
                                &mut |_c: &mut Checker, s: TypeId, t: TypeId| {
                                    compare_types_equal(s, t)
                                },
                            ) == Ternary::TRUE
                        {
                            // If we merged instantiations of a generic type, we replicate the symbol parent resetting behavior we used
                            // to do when we recorded multiple distinct symbols so that we still get, eg, `Array<T>.length` printed
                            // back and not `Array<string>.length` when we're looking at a `.length` access on a `string[] | number[]`
                            let single_parent = self.sym(single_prop).parent;
                            merged_instantiations = single_parent.is_some()
                                && !self
                                    .get_local_type_parameters_of_class_or_interface_or_type_alias(
                                        single_parent,
                                    )
                                    .is_empty();
                        } else {
                            if prop_set.is_empty() {
                                ordered_symbol_set_add(
                                    &mut prop_set,
                                    &mut prop_set_index,
                                    single_prop,
                                );
                            }
                            ordered_symbol_set_add(&mut prop_set, &mut prop_set_index, prop);
                        }
                        // classes created by mixins are represented as intersections
                        // and overriding a property in a derived class redefines it completely at runtime
                        // so a get accessor can't be merged with a set accessor in a base class,
                        // for that reason the accessor flags are only used when they are the same in all constituents
                        if prop_flags.intersects(SymbolFlags::ACCESSOR)
                            && (prop_symbol_flags & SymbolFlags::ACCESSOR)
                                != (prop_flags & SymbolFlags::ACCESSOR)
                        {
                            prop_flags =
                                prop_flags.without(SymbolFlags::ACCESSOR) | SymbolFlags::PROPERTY;
                        }
                    }
                    if is_union && self.is_readonly_symbol(prop) {
                        check_flags |= CheckFlags::READONLY;
                    } else if !is_union && !self.is_readonly_symbol(prop) {
                        check_flags = check_flags.without(CheckFlags::READONLY);
                    }
                    if modifiers.intersects(ModifierFlags::PROTECTED)
                        && !modifiers.intersects(ModifierFlags::PUBLIC)
                    {
                        check_flags |= CheckFlags::CONTAINS_PROTECTED;
                    } else if modifiers.intersects(ModifierFlags::PRIVATE)
                        && !modifiers.intersects(ModifierFlags::PUBLIC)
                    {
                        check_flags |= CheckFlags::CONTAINS_PRIVATE;
                    } else {
                        check_flags |= CheckFlags::CONTAINS_PUBLIC;
                    }
                    if write_modifiers.intersects(ModifierFlags::PROTECTED)
                        && !write_modifiers.intersects(ModifierFlags::PUBLIC)
                    {
                        check_flags |= CheckFlags::CONTAINS_WRITE_PROTECTED;
                    } else if write_modifiers.intersects(ModifierFlags::PRIVATE)
                        && !write_modifiers.intersects(ModifierFlags::PUBLIC)
                    {
                        check_flags |= CheckFlags::CONTAINS_WRITE_PRIVATE;
                    } else {
                        check_flags |= CheckFlags::CONTAINS_WRITE_PUBLIC;
                    }
                    if modifiers.intersects(ModifierFlags::STATIC) {
                        check_flags |= CheckFlags::CONTAINS_STATIC;
                    }
                    if !self.is_prototype_property(prop) {
                        synthetic_flag = CheckFlags::SYNTHETIC_PROPERTY;
                    }
                } else if is_union {
                    let mut index_info = IndexInfoId::NIL;
                    if !late_bound_name {
                        if name_literal_type.is_nil() {
                            name_literal_type = self.get_string_literal_type(name.text());
                        }
                        index_info = self.get_applicable_index_info(t, name_literal_type);
                    }
                    if index_info.is_some() {
                        prop_flags =
                            prop_flags.without(SymbolFlags::ACCESSOR) | SymbolFlags::PROPERTY;
                        let readonly = if self.index_info(index_info).is_readonly {
                            CheckFlags::READONLY
                        } else {
                            CheckFlags::NONE
                        };
                        check_flags |= CheckFlags::WRITE_PARTIAL | readonly;
                        if self.is_tuple_type(t) {
                            let mut index_type = self.get_rest_type_of_tuple_type(t);
                            if index_type.is_nil() {
                                index_type = self.undefined_type;
                            }
                            index_types.push(index_type);
                        } else {
                            index_types.push(self.index_info(index_info).value_type);
                        }
                    } else if self.is_object_literal_type(t)
                        && !self
                            .ty(t)
                            .object_flags
                            .intersects(ObjectFlags::CONTAINS_SPREAD)
                    {
                        check_flags |= CheckFlags::WRITE_PARTIAL;
                        index_types.push(self.undefined_type);
                    } else {
                        check_flags |= CheckFlags::READ_PARTIAL;
                    }
                }
            }
        }
        if single_prop.is_nil() {
            // No property was found
            return SymbolId::NIL;
        }
        if is_union
            && (!prop_set.is_empty() || check_flags.intersects(CheckFlags::PARTIAL))
            && check_flags.intersects(
                CheckFlags::CONTAINS_PRIVATE
                    | CheckFlags::CONTAINS_PROTECTED
                    | CheckFlags::CONTAINS_WRITE_PRIVATE
                    | CheckFlags::CONTAINS_WRITE_PROTECTED,
            )
            && !(!prop_set.is_empty() && self.has_common_declaration(&prop_set))
        {
            // A property in a union has a private or protected declaration in one constituent, but is missing
            // or has a different declaration in another constituent. If the private or protected declaration is
            // for reading, we don't create a property.
            if check_flags.intersects(CheckFlags::CONTAINS_PRIVATE | CheckFlags::CONTAINS_PROTECTED)
            {
                return SymbolId::NIL;
            }
            // Otherwise, if the private or protected declaration is for writing, reduce accessibility to that of
            // the most restricted constituent.
            if check_flags.intersects(CheckFlags::CONTAINS_WRITE_PRIVATE) {
                check_flags = check_flags.without(
                    CheckFlags::CONTAINS_WRITE_PUBLIC | CheckFlags::CONTAINS_WRITE_PROTECTED,
                );
            } else if check_flags.intersects(CheckFlags::CONTAINS_WRITE_PROTECTED) {
                check_flags = check_flags.without(CheckFlags::CONTAINS_WRITE_PUBLIC);
            }
        }
        if prop_set.is_empty()
            && !check_flags.intersects(CheckFlags::READ_PARTIAL)
            && index_types.is_empty()
        {
            if !merged_instantiations {
                return single_prop;
            }
            // No symbol from a union/intersection should have a `.parent` set (since unions/intersections don't act as symbol parents)
            // Unless that parent is "reconstituted" from the "first value declaration" on the symbol (which is likely different than its instantiated parent!)
            // They also have a `.containingType` set, which affects some services endpoints behavior, like `getRootSymbol`
            let mut single_prop_type = TypeId::NIL;
            let mut single_prop_mapper = MapperId::NIL;
            if self
                .sym(single_prop)
                .flags
                .intersects(SymbolFlags::TRANSIENT)
            {
                let links = self
                    .value_symbol_links
                    .get_by_id(&self.symbols, single_prop);
                single_prop_type = links.resolved_type;
                single_prop_mapper = links.mapper;
            }
            let clone = self.create_symbol_with_type(single_prop, single_prop_type);
            let value_declaration = self.sym(single_prop).value_declaration;
            if value_declaration.is_some() {
                let parent = self.sym(value_declaration.symbol()).parent;
                self.sym_mut(clone).parent = parent;
            }
            let write_type = self.get_write_type_of_symbol(single_prop);
            let links = self.value_symbol_links.get_by_id(&self.symbols, clone);
            links.containing_type = containing_type;
            links.mapper = single_prop_mapper;
            links.write_type = write_type;
            return clone;
        }
        if prop_set.is_empty() {
            prop_set.push(single_prop);
        }
        let declaration_count = prop_set
            .iter()
            .map(|&prop| self.sym(prop).declarations.len())
            .sum();
        // PERF: the declarations are collected on the stack. A list of 0 or 1
        // declarations is stored with no heap list (no malloc). A longer list
        // moves into one `Vec` for `Declarations::from`, as before.
        let mut declarations: SmallVec<[Node; 4]> = SmallVec::with_capacity(declaration_count);
        // PERF: Go `core.AppendIfUnique` scans the list for each declaration,
        // which is quadratic in a long list (blueprint: 43k calls with 181
        // declarations, 178 of them distinct; the scans were about half of
        // its check). A long list keeps a set of its nodes beside it
        // (`DeclarationSet`); the list and its order stay Go's.
        let mut declaration_set = DeclarationSet::default();
        let mut first_type = TypeId::NIL;
        let mut name_type = TypeId::NIL;
        let mut prop_types: SmallVec<[TypeId; 4]> =
            SmallVec::with_capacity(prop_set.len() + index_types.len());
        // PORT: Go nil slice `writeTypes` is `None`.
        let mut write_types: Option<Vec<TypeId>> = None;
        let mut first_value_declaration = Node::NIL;
        let mut has_non_uniform_value_declaration = false;
        for &prop in &prop_set {
            let prop_value_declaration = self.sym(prop).value_declaration;
            if first_value_declaration.is_nil() {
                first_value_declaration = prop_value_declaration;
            } else if prop_value_declaration.is_some()
                && prop_value_declaration != first_value_declaration
            {
                has_non_uniform_value_declaration = true;
            }
            for &declaration in self.sym(prop).declarations.iter() {
                // Go: core.AppendIfUnique
                declaration_set.add(&mut declarations, declaration, declaration_count);
            }
            let t = self.get_type_of_symbol(prop);
            if first_type.is_nil() {
                first_type = t;
                name_type = self
                    .value_symbol_links
                    .get_by_id(&self.symbols, prop)
                    .name_type;
            }
            let write_type = self.get_write_type_of_symbol(prop);
            if write_types.is_some() || write_type != t {
                if write_types.is_none() {
                    write_types = Some(prop_types.to_vec());
                }
                write_types.as_mut().unwrap().push(write_type);
            }
            if t != first_type {
                check_flags |= CheckFlags::HAS_NON_UNIFORM_TYPE;
            }
            if self.is_literal_type(t) || self.is_pattern_literal_type(t) {
                check_flags |= CheckFlags::HAS_LITERAL_TYPE;
            }
            if self.ty(t).flags.intersects(TypeFlags::NEVER) && t != self.unique_literal_type {
                check_flags |= CheckFlags::HAS_NEVER_TYPE;
            }
            prop_types.push(t);
        }
        prop_types.extend(index_types.iter().copied());
        let result = self.new_symbol_ex(
            prop_flags | optional_flag,
            table_key_name(name),
            check_flags | synthetic_flag,
        );
        let declarations = if declarations.len() < 2 {
            let mut list = Declarations::default();
            if let Some(&node) = declarations.first() {
                list.push(node);
            }
            list
        } else {
            Declarations::from(declarations.into_vec())
        };
        self.sym_mut(result).declarations = declarations;
        if !has_non_uniform_value_declaration && first_value_declaration.is_some() {
            self.sym_mut(result).value_declaration = first_value_declaration;
            // Inherit information about parent type.
            let parent = self.sym(first_value_declaration.symbol()).parent;
            self.sym_mut(result).parent = parent;
        }
        {
            let links = self.value_symbol_links.get_by_id(&self.symbols, result);
            links.containing_type = containing_type;
            links.name_type = name_type;
        }
        if prop_types.len() > 2 {
            // When `propTypes` has the potential to explode in size when normalized, defer normalization until absolutely needed
            self.sym_mut(result).check_flags |= CheckFlags::DEFERRED_TYPE;
            let deferred = self.deferred_symbol_links.get(result);
            deferred.parent = containing_type;
            deferred.constituents = prop_types.into_vec();
            deferred.write_constituents = write_types.unwrap_or_default();
            return result;
        }
        let resolved_type = if is_union {
            self.get_union_type(&prop_types)
        } else {
            self.get_intersection_type(&prop_types)
        };
        self.value_symbol_links
            .get_by_id(&self.symbols, result)
            .resolved_type = resolved_type;
        if let Some(write_types) = write_types {
            let write_type = if is_union {
                self.get_union_type(&write_types)
            } else {
                self.get_intersection_type(&write_types)
            };
            self.value_symbol_links
                .get_by_id(&self.symbols, result)
                .write_type = write_type;
        }
        result
    }

    // Go: checker/checker.go:22020 getTargetSymbol
    pub fn get_target_symbol(&mut self, s: SymbolId) -> SymbolId {
        // if symbol is instantiated its flags are not copied from the 'target'
        // so we'll need to get back original 'target' symbol to work with correct set of flags
        if s.is_some() && self.sym(s).check_flags.intersects(CheckFlags::INSTANTIATED) {
            return self.value_symbol_links.get_by_id(&self.symbols, s).target;
        }
        s
    }

    /**
     * Return whether this symbol is a member of a prototype somewhere
     * Note that this is not tracked well within the compiler, so the answer may be incorrect.
     */
    // Go: checker/checker.go:22033 isPrototypeProperty
    pub fn is_prototype_property(&self, symbol: SymbolId) -> bool {
        let s = self.sym(symbol);
        s.flags.intersects(SymbolFlags::METHOD)
            || s.check_flags.intersects(CheckFlags::SYNTHETIC_METHOD)
    }

    // Go: checker/checker.go:22037 hasCommonDeclaration
    pub fn has_common_declaration(&self, symbols: &[SymbolId]) -> bool {
        // PORT: Go `collections.Set[*ast.Node]`; only its size is observed, so
        // iteration order does not matter.
        let mut common_declarations: FxHashSet<Node> = FxHashSet::default();
        for &symbol in symbols {
            let decls = &self.sym(symbol).declarations;
            if decls.is_empty() {
                return false;
            }
            if common_declarations.is_empty() {
                for &d in decls {
                    common_declarations.insert(d);
                }
                continue;
            }
            common_declarations.retain(|d| decls.contains(d));
            if common_declarations.is_empty() {
                return false;
            }
        }
        !common_declarations.is_empty()
    }

    // Go: checker/checker.go:22061 createSymbolWithType
    pub fn create_symbol_with_type(&mut self, source: SymbolId, t: TypeId) -> SymbolId {
        let (flags, name, check_flags, declarations, parent, value_declaration) = {
            let s = self.sym(source);
            (
                s.flags,
                s.name.clone(),
                s.check_flags,
                s.declarations.clone(),
                s.parent,
                s.value_declaration,
            )
        };
        let symbol = self.new_symbol_ex(flags, &name, check_flags & CheckFlags::READONLY);
        {
            let s = self.sym_mut(symbol);
            s.declarations = declarations;
            s.parent = parent;
            s.value_declaration = value_declaration;
        }
        // Go reads the links of `symbol` before those of `source`.
        let links = self.value_symbol_links.get_by_id(&self.symbols, symbol);
        links.resolved_type = t;
        links.target = source;
        let source_name_type = self
            .value_symbol_links
            .get_by_id(&self.symbols, source)
            .name_type;
        self.value_symbol_links
            .get_by_id(&self.symbols, symbol)
            .name_type = source_name_type;
        symbol
    }

    // Go: checker/checker.go:22073 isMappedTypeGenericIndexedAccess
    pub fn is_mapped_type_generic_indexed_access(&mut self, t: TypeId) -> bool {
        if self.ty(t).flags.intersects(TypeFlags::INDEXED_ACCESS) {
            let object_type = self.ty(t).as_indexed_access_type().object_type;
            let index_type = self.ty(t).as_indexed_access_type().index_type;
            return self
                .ty(object_type)
                .object_flags
                .intersects(ObjectFlags::MAPPED)
                && !self.is_generic_mapped_type(object_type)
                && self.is_generic_index_type(index_type)
                && !self
                    .get_mapped_type_modifiers(object_type)
                    .intersects(MappedTypeModifiers::EXCLUDE_OPTIONAL)
                && self
                    .ty(object_type)
                    .as_mapped_type()
                    .declaration
                    .name_type()
                    .is_nil();
        }
        false
    }

    /**
     * For a type parameter, return the base constraint of the type parameter. For the string, number,
     * boolean, and symbol primitive types, return the corresponding object types. Otherwise return the
     * type itself.
     */
    // Go: checker/checker.go:22087 getApparentType
    pub fn get_apparent_type(&mut self, t: TypeId) -> TypeId {
        let original_type = t;
        let mut t = t;
        if self.ty(t).flags.intersects(TypeFlags::INSTANTIABLE) {
            t = self.get_base_constraint_of_type(t);
            if t.is_nil() {
                t = self.unknown_type;
            }
        }
        let flags = self.ty(t).flags;
        let object_flags = self.ty(t).object_flags;
        if object_flags.intersects(ObjectFlags::MAPPED) {
            // PERF: the cached apparent type of a mapped type returns here,
            // without the call that would read it.
            let cached = self.ty(t).as_mapped_type().resolved_apparent_type;
            if cached.is_some() {
                return cached;
            }
            return self.get_apparent_type_of_mapped_type(t);
        } else if object_flags.intersects(ObjectFlags::REFERENCE) && t != original_type {
            return self.get_type_with_this_argument(
                t,
                original_type,
                false, /*needsApparentType*/
            );
        } else if flags.intersects(TypeFlags::INTERSECTION) {
            return self.get_apparent_type_of_intersection_type(t, original_type);
        } else if flags.intersects(TypeFlags::STRING_LIKE) {
            return self.global_string_type;
        } else if flags.intersects(TypeFlags::NUMBER_LIKE) {
            return self.global_number_type;
        } else if flags.intersects(TypeFlags::BIG_INT_LIKE) {
            let get_global_big_int_type = self.get_global_big_int_type.clone();
            return get_global_big_int_type(self);
        } else if flags.intersects(TypeFlags::BOOLEAN_LIKE) {
            return self.global_boolean_type;
        } else if flags.intersects(TypeFlags::ES_SYMBOL_LIKE) {
            let get_global_es_symbol_type = self.get_global_es_symbol_type.clone();
            return get_global_es_symbol_type(self);
        } else if flags.intersects(TypeFlags::NON_PRIMITIVE) {
            return self.empty_object_type;
        } else if flags.intersects(TypeFlags::INDEX) {
            return self.string_number_symbol_type;
        } else if flags.intersects(TypeFlags::UNKNOWN) && !self.strict_null_checks {
            return self.empty_object_type;
        }
        t
    }

    // Go: checker/checker.go:22122 getApparentTypeOfMappedType
    pub fn get_apparent_type_of_mapped_type(&mut self, t: TypeId) -> TypeId {
        if self.ty(t).as_mapped_type().resolved_apparent_type.is_nil() {
            let resolved = self.get_resolved_apparent_type_of_mapped_type(t);
            self.ty_mut(t).as_mapped_type_mut().resolved_apparent_type = resolved;
        }
        self.ty(t).as_mapped_type().resolved_apparent_type
    }

    // Go: checker/checker.go:22130 getResolvedApparentTypeOfMappedType
    pub fn get_resolved_apparent_type_of_mapped_type(&mut self, t: TypeId) -> TypeId {
        let mapped_target = self.ty(t).as_mapped_type().object.target;
        let target = if mapped_target.is_some() {
            mapped_target
        } else {
            t
        };
        let type_variable = self.get_homomorphic_type_variable(target);
        if type_variable.is_some()
            && self
                .ty(target)
                .as_mapped_type()
                .declaration
                .name_type()
                .is_nil()
        {
            // We have a homomorphic mapped type or an instantiation of a homomorphic mapped type, i.e. a type
            // of the form { [P in keyof T]: X }. Obtain the modifiers type (the T of the keyof T), and if it is
            // another generic mapped type, recursively obtain its apparent type. Otherwise, obtain its base
            // constraint. Then, if every constituent of the base constraint is an array or tuple type, apply
            // this mapped type to the base constraint. It is safe to recurse when the modifiers type is a
            // mapped type because we protect again circular constraints in getTypeFromMappedTypeNode.
            let modifiers_type = self.get_modifiers_type_from_mapped_type(t);
            let base_constraint = if self.is_generic_mapped_type(modifiers_type) {
                self.get_apparent_type_of_mapped_type(modifiers_type)
            } else {
                self.get_base_constraint_of_type(modifiers_type)
            };
            if base_constraint.is_some()
                && self.every_type(base_constraint, &mut |c: &mut Checker, t: TypeId| {
                    c.is_array_or_tuple_type(t) || c.is_array_or_tuple_or_intersection(t)
                })
            {
                let mapper = self.ty(t).as_mapped_type().object.mapper;
                let m = self.prepend_type_mapping(type_variable, base_constraint, mapper);
                return self.instantiate_type(target, m);
            }
        }
        t
    }

    // Go: checker/checker.go:22154 getApparentTypeOfIntersectionType
    pub fn get_apparent_type_of_intersection_type(
        &mut self,
        t: TypeId,
        this_argument: TypeId,
    ) -> TypeId {
        if t == this_argument {
            if self
                .ty(t)
                .as_intersection_type()
                .resolved_apparent_type
                .is_nil()
            {
                let resolved = self.get_type_with_this_argument(
                    t,
                    this_argument,
                    true, /*needApparentType*/
                );
                self.ty_mut(t)
                    .as_intersection_type_mut()
                    .resolved_apparent_type = resolved;
            }
            return self.ty(t).as_intersection_type().resolved_apparent_type;
        }
        let key = CachedTypeKey {
            kind: CachedTypeKind::APPARENT_TYPE,
            type_id: this_argument,
        };
        let mut result = self.cached_types.get(&key).copied().unwrap_or_default();
        if result.is_nil() {
            result =
                self.get_type_with_this_argument(t, this_argument, true /*needApparentType*/);
            self.cached_types.insert(key, result);
        }
        result
    }

    /**
     * Return the reduced form of the given type. For a union type, it is a union of the normalized constituent types.
     * For an intersection of types containing one or more mututally exclusive discriminant properties, it is 'never'.
     * For all other types, it is simply the type itself. Discriminant properties are considered mutually exclusive when
     * no constituent property has type 'never', but the intersection of the constituent property types is 'never'.
     */
    // Go: checker/checker.go:22177 getReducedType
    // PORT: split in two. This part returns `t` for the common case (not a
    // union with intersections and not an intersection) without the frame of
    // the slow part. The slow part repeats the same tests.
    #[inline]
    pub fn get_reduced_type(&mut self, t: TypeId) -> TypeId {
        let ty = self.ty(t);
        if ty.flags.intersects(TypeFlags::UNION) {
            if !ty
                .object_flags
                .intersects(ObjectFlags::CONTAINS_INTERSECTIONS)
            {
                return t;
            }
        } else if !ty.flags.intersects(TypeFlags::INTERSECTION) {
            return t;
        }
        self.get_reduced_type_slow(t)
    }

    /// The full `get_reduced_type` body. Only `get_reduced_type` calls it.
    #[inline(never)]
    fn get_reduced_type_slow(&mut self, t: TypeId) -> TypeId {
        let flags = self.ty(t).flags;
        if flags.intersects(TypeFlags::UNION) {
            if self
                .ty(t)
                .object_flags
                .intersects(ObjectFlags::CONTAINS_INTERSECTIONS)
            {
                let reduced_type = self.ty(t).as_union_type().resolved_reduced_type;
                if reduced_type.is_some() {
                    return reduced_type;
                }
                let reduced_type = self.get_reduced_union_type(t);
                self.ty_mut(t).as_union_type_mut().resolved_reduced_type = reduced_type;
                return reduced_type;
            }
        } else if flags.intersects(TypeFlags::INTERSECTION) {
            if !self
                .ty(t)
                .object_flags
                .intersects(ObjectFlags::IS_NEVER_INTERSECTION_COMPUTED)
            {
                self.ty_mut(t).object_flags |= ObjectFlags::IS_NEVER_INTERSECTION_COMPUTED;
                let types = self.ty(t).types().to_vec();
                if !self.is_mapping_of_same_object_type(&types)
                    && self.some_property_reduces_to_never(t)
                {
                    self.ty_mut(t).object_flags |= ObjectFlags::IS_NEVER_INTERSECTION;
                }
            }
            if self
                .ty(t)
                .object_flags
                .intersects(ObjectFlags::IS_NEVER_INTERSECTION)
            {
                return self.never_type;
            }
        }
        t
    }

    // Go: checker/checker.go:22202 isMappingOfSameObjectType
    pub fn is_mapping_of_same_object_type(&mut self, types: &[TypeId]) -> bool {
        if !types.is_empty()
            && self
                .ty(types[0])
                .object_flags
                .intersects(ObjectFlags::MAPPED)
        {
            let first_type = self.get_modifiers_type_from_mapped_type(types[0]);
            if self.ty(first_type).flags.intersects(TypeFlags::OBJECT) {
                for &t in &types[1..] {
                    if !self.ty(t).object_flags.intersects(ObjectFlags::MAPPED)
                        || self.get_modifiers_type_from_mapped_type(t) != first_type
                    {
                        return false;
                    }
                }
                return true;
            }
        }
        false
    }

    // Go: checker/checker.go:22216 somePropertyReducesToNever
    // PORT: Go N ranged over a map, so its order was random. Since ts#64521
    // (Go N' checker.go:22283) Go counts in a `collections.OrderedMap` and
    // tests the names in first-seen order, so it makes the combined
    // properties in the same order every time. Every order gives the same
    // answer; the order only decides how many synthetic properties are made
    // before the first match. The port keeps its own deterministic order
    // (a PERF deviation from Go N' first-seen order):
    // first the names that are not a public method in some constituent, then
    // the names that are a public method in every constituent, each group in
    // the order in which the names are first seen (constituent order, then
    // property order). A method's type is a function type, never a literal
    // type, so a method is no discriminant. A method reduces to never only as
    // a conflicting private member, so a method that can be private (the
    // `getDeclarationModifierFlagsFromSymbol` test that sets
    // `CheckFlagsContainsPrivate`) stays in the first group, and a
    // conflicting private method is found as early as in first-seen order.
    // A `#private` name sets no such flag and is unique to its class, so it is
    // no conflicting private member. In first-seen order alone, the `length`
    // of a tuple intersection (`[] & [a: A] & ...`) came after the array
    // methods, and the mongodb test check made 2.2 times Go's symbols.
    // PERF: the counts are keyed by the name id with FxHash, and each lookup
    // passes the `Name`, so no name text is hashed, compared or copied (Go
    // hashes the text).
    pub fn some_property_reduces_to_never(&mut self, t: TypeId) -> bool {
        // Collect declaration counts for each property across all constituent types of the intersection.
        // PORT: with each count, whether every declaration of the name is a
        // public method (`is_public_method`), and the value declaration of
        // the first property of the name.
        // PERF: a name seen again with that value declaration is not tested
        // again. That is almost always the same member (an instantiation of
        // it). It can be another symbol: a union or intersection property of
        // two or more properties is a property, not a method, with their
        // value declaration when they have one and the same (Go
        // `createUnionOrIntersectionProperty`, checker.go:21821 and :21988).
        // So in `A<string> & T`, where `T extends A<string> | A<number>` and
        // `A.m` is a public method, the `m` of `T` (a property of its
        // apparent type `A<string> | A<number>`, checker.go:19187) is
        // skipped, and `m` stays with the public methods
        // (`tests::a_skipped_property_can_be_a_union_property`). An
        // intersection has no union constituent: Go distributes
        // `A & (A | B)` over the union (checker.go:26508), so that type is
        // the union `A | (A & B)`. That changes only the order of the tests
        // below, not the answer: both groups are tested. The tuples of an intersection
        // share the lib declarations of their array methods, so this saves
        // a test for each method of each tuple (mongodb test check: the test
        // cost 6.5% more instructions, 1.6% with the parent of the first
        // declaration as the key). The key is not the parent: an instance
        // and a static member of one class have the same parent, so in
        // `A & typeof A` a `private static m()` after a public `m()` went
        // last, after every property of the class.
        let mut counts: FxIndexMap<u32, (i32, bool, Node)> = FxIndexMap::default();
        for i in 0..self.ty(t).types().len() {
            let u = self.type_at(t, i);
            let props = self.get_properties_of_type(u);
            for &prop in props.iter() {
                let symbol = self.sym(prop);
                match counts.entry(symbol.name.id()) {
                    indexmap::map::Entry::Occupied(entry) => {
                        let entry = entry.into_mut();
                        entry.0 += 1;
                        if entry.1 && symbol.value_declaration != entry.2 {
                            entry.1 = self.is_public_method(prop);
                        }
                    }
                    indexmap::map::Entry::Vacant(entry) => {
                        entry.insert((1, self.is_public_method(prop), symbol.value_declaration));
                    }
                }
            }
        }
        // Check if any property appears in more than one constituent type and reduces to 'never'.
        for public_methods in [false, true] {
            for (&id, &(count, all_public_methods, _)) in &counts {
                if count > 1 && all_public_methods == public_methods {
                    let prop_name = Name::from_id(id);
                    let prop = self.get_property_of_union_or_intersection_type_key(
                        t,
                        TableKey::Name(&prop_name),
                        true, /*skipObjectFunctionPropertyAugment*/
                    );
                    if prop.is_some() && self.is_never_reduced_property(prop) {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Whether `prop` is a method that cannot be private, for the name order
    /// of `some_property_reduces_to_never`.
    // PORT: no Go counterpart. A method is never synthetic, and Go
    // `getDeclarationModifierFlagsFromSymbol` drops the accessibility
    // modifiers of a member whose parent is no class, so only a class method
    // needs the modifier test. A method that is also a property takes the
    // property's type, so it does not count as a method.
    fn is_public_method(&self, prop: SymbolId) -> bool {
        let symbol = self.sym(prop);
        symbol.flags & (SymbolFlags::METHOD | SymbolFlags::PROPERTY) == SymbolFlags::METHOD
            && !(symbol.parent.is_some()
                && self.sym(symbol.parent).flags.intersects(SymbolFlags::CLASS)
                && self
                    .get_declaration_modifier_flags_from_symbol(prop)
                    .intersects(ModifierFlags::PRIVATE))
    }

    // Go: checker/checker.go:22235 getReducedUnionType
    pub fn get_reduced_union_type(&mut self, union_type: TypeId) -> TypeId {
        // PORT: Go `core.SameMap` returns the input slice when no element
        // changes, so `core.Same` is "no element changed" (`None`) here.
        let count = self.ty(union_type).types().len();
        let Some(reduced_types) =
            self.map_stored_types_if_changed(union_type, count, Checker::type_at, &mut |c, t| {
                c.get_reduced_type(t)
            })
        else {
            return union_type;
        };
        let reduced = self.get_union_type(&reduced_types);
        if self.ty(reduced).flags.intersects(TypeFlags::UNION) {
            self.ty_mut(reduced)
                .as_union_type_mut()
                .resolved_reduced_type = reduced;
        }
        reduced
    }

    // Go: checker/checker.go:22247 isNeverReducedProperty
    pub fn is_never_reduced_property(&mut self, prop: SymbolId) -> bool {
        self.is_discriminant_with_never_type(prop) || self.is_conflicting_private_property(prop)
    }

    // Go: checker/checker.go:22251 getReducedApparentType
    pub fn get_reduced_apparent_type(&mut self, t: TypeId) -> TypeId {
        // PERF: an object type is its own reduced type. Its apparent type is
        // itself, or for a mapped type the cached apparent type, read with no
        // effect (`get_apparent_type`). So these return without the calls
        // (property lookups in the constituents of large unions).
        let ty = self.ty(t);
        if ty.flags.intersects(TypeFlags::OBJECT) {
            if !ty.object_flags.intersects(ObjectFlags::MAPPED) {
                return t;
            }
            let cached = ty.as_mapped_type().resolved_apparent_type;
            if cached.is_some() {
                return self.get_reduced_type(cached);
            }
        }
        // Since getApparentType may return a non-reduced union or intersection type, we need to perform
        // type reduction both before and after obtaining the apparent type. For example, given a type parameter
        // 'T extends A | B', the type 'T & X' becomes 'A & X | B & X' after obtaining the apparent type, and
        // that type may need further reduction to remove empty intersections.
        let reduced = self.get_reduced_type(t);
        let apparent = self.get_apparent_type(reduced);
        self.get_reduced_type(apparent)
    }

    // Go: checker/checker.go:22259 elaborateNeverIntersection
    pub fn elaborate_never_intersection(
        &mut self,
        chain: Option<Diagnostic>,
        node: Node,
        t: TypeId,
    ) -> Option<Diagnostic> {
        if self.ty(t).flags.intersects(TypeFlags::INTERSECTION)
            && self
                .ty(t)
                .object_flags
                .intersects(ObjectFlags::IS_NEVER_INTERSECTION)
        {
            let props = self
                .get_properties_of_union_or_intersection_type(t)
                .to_vec();
            let mut never_prop = SymbolId::NIL;
            for &p in &props {
                if self.is_discriminant_with_never_type(p) {
                    never_prop = p;
                    break;
                }
            }
            if never_prop.is_some() {
                let type_string =
                    self.type_to_string_ex(t, Node::NIL, TypeFormatFlags::NO_TYPE_REDUCTION, None);
                let prop_string = self.symbol_to_string(never_prop);
                return Some(new_diagnostic_chain_for_node(
                    chain,
                    node,
                    diag::The_intersection_0_was_reduced_to_never_because_property_1_has_conflicting_types_in_some_constituents,
                    args![type_string, prop_string],
                ));
            }
            let props = self
                .get_properties_of_union_or_intersection_type(t)
                .to_vec();
            let mut private_prop = SymbolId::NIL;
            for &p in &props {
                if self.is_conflicting_private_property(p) {
                    private_prop = p;
                    break;
                }
            }
            if private_prop.is_some() {
                let type_string =
                    self.type_to_string_ex(t, Node::NIL, TypeFormatFlags::NO_TYPE_REDUCTION, None);
                let prop_string = self.symbol_to_string(private_prop);
                return Some(new_diagnostic_chain_for_node(
                    chain,
                    node,
                    diag::The_intersection_0_was_reduced_to_never_because_property_1_exists_in_multiple_constituents_and_is_private_in_some,
                    args![type_string, prop_string],
                ));
            }
        }
        chain
    }

    // Go: checker/checker.go:22273 isDiscriminantWithNeverType
    pub fn is_discriminant_with_never_type(&mut self, prop: SymbolId) -> bool {
        // Return true for a synthetic non-optional property with non-uniform types, where at least one is
        // a literal type and none is never, that reduces to never.
        let (flags, check_flags) = {
            let s = self.sym(prop);
            (s.flags, s.check_flags)
        };
        if flags.intersects(SymbolFlags::OPTIONAL) {
            return false;
        }
        if check_flags & (CheckFlags::NON_UNIFORM_AND_LITERAL | CheckFlags::HAS_NEVER_TYPE)
            != CheckFlags::NON_UNIFORM_AND_LITERAL
        {
            return false;
        }
        let t = self.get_type_of_symbol(prop);
        self.ty(t).flags.intersects(TypeFlags::NEVER)
    }

    // Go: checker/checker.go:22279 isConflictingPrivateProperty
    pub fn is_conflicting_private_property(&self, prop: SymbolId) -> bool {
        // Return true for a synthetic property with multiple declarations, at least one of which is private.
        let s = self.sym(prop);
        s.value_declaration.is_nil() && s.check_flags.intersects(CheckFlags::CONTAINS_PRIVATE)
    }

    /// Constituent `i` of the union, intersection or template literal type
    /// `t`. These lists never change after the type is created, so a loop can
    /// read them by index across checker calls instead of copying them.
    #[inline]
    pub fn type_at(&self, t: TypeId, i: usize) -> TypeId {
        self.ty(t).types()[i]
    }

    /// Go `core.Map(t.Types(), f)`: maps each constituent of `t` in order,
    /// read in place instead of from a copy of the list.
    pub fn map_constituents(
        &mut self,
        t: TypeId,
        f: &mut dyn FnMut(&mut Checker, TypeId) -> TypeId,
    ) -> Vec<TypeId> {
        let count = self.ty(t).types().len();
        let mut mapped = Vec::with_capacity(count);
        for i in 0..count {
            let u = self.type_at(t, i);
            mapped.push(f(self, u));
        }
        mapped
    }

    /// Go `core.SameMap` + `core.Same` over a type list stored on `owner`
    /// that never changes once set (constituents, resolved type arguments).
    /// `read(c, owner, i)` reads element `i` in place, so the list is copied
    /// only when an element changes. Each element is mapped once, in order.
    /// `None` means no element changed. Callers only read the result as a
    /// slice, so a short list stays on the stack.
    pub fn map_stored_types_if_changed(
        &mut self,
        owner: TypeId,
        count: usize,
        read: fn(&Checker, TypeId, usize) -> TypeId,
        map: &mut dyn FnMut(&mut Checker, TypeId) -> TypeId,
    ) -> Option<SmallVec<[TypeId; 8]>> {
        for i in 0..count {
            let value = read(self, owner, i);
            let mapped = map(self, value);
            if mapped != value {
                let mut result: SmallVec<[TypeId; 8]> = SmallVec::with_capacity(count);
                result.extend((0..i).map(|j| read(self, owner, j)));
                result.push(mapped);
                for j in i + 1..count {
                    let value = read(self, owner, j);
                    result.push(map(self, value));
                }
                return Some(result);
            }
        }
        None
    }

    // Go: checker/checker.go:22284 getTypeArguments
    pub fn get_type_arguments(&mut self, t: TypeId) -> SharedList<TypeId> {
        if let Some(count) = self.resolve_type_arguments(t) {
            return vec![self.error_type; count].into();
        }
        self.ty(t)
            .as_type_reference()
            .resolved_type_arguments
            .clone()
    }

    /// `get_type_arguments` without a copy of the resolved list. Use it when
    /// the caller only reads the list before its next checker call.
    pub fn type_arguments_of(&mut self, t: TypeId) -> std::borrow::Cow<'_, [TypeId]> {
        if let Some(count) = self.resolve_type_arguments(t) {
            return std::borrow::Cow::Owned(vec![self.error_type; count]);
        }
        std::borrow::Cow::Borrowed(&self.ty(t).as_type_reference().resolved_type_arguments[..])
    }

    /// The body of Go `getTypeArguments`. It stores the resolved list on `t`.
    /// When the type arguments circularly reference themselves while they
    /// resolve, it stores nothing and returns the count of error types that
    /// Go returns in place of the list.
    fn resolve_type_arguments(&mut self, t: TypeId) -> Option<usize> {
        // PORT: Go tests `resolvedTypeArguments == nil`. The Rust field is a
        // `SharedList`, so an empty list reads as unresolved. An empty list resolves
        // to an empty list again, so the result is the same.
        if self
            .ty(t)
            .as_type_reference()
            .resolved_type_arguments
            .is_empty()
        {
            let target = self.ty(t).as_type_reference().object.target;
            let type_parameter_count = self.ty(target).as_interface_type().type_parameters().len();
            if !self.push_type_resolution(
                TypeSystemEntity::Type(t),
                TypeSystemPropertyName::RESOLVED_TYPE_ARGUMENTS,
            ) {
                return Some(type_parameter_count);
            }
            let mut type_arguments: Vec<TypeId> = Vec::new();
            let node = self.ty(t).as_type_reference().node;
            if node.is_some() {
                match node.kind() {
                    SyntaxKind::TypeReference => {
                        let outer = self
                            .ty(target)
                            .as_interface_type()
                            .outer_type_parameters()
                            .to_vec();
                        let local = self
                            .ty(target)
                            .as_interface_type()
                            .local_type_parameters()
                            .to_vec();
                        type_arguments = outer;
                        let effective = self.get_effective_type_arguments(node, &local);
                        type_arguments.extend(effective);
                    }
                    SyntaxKind::ArrayType => {
                        type_arguments = vec![self.get_type_from_type_node(node.element_type())];
                    }
                    SyntaxKind::TupleType => {
                        for e in node.elements() {
                            let et = self.get_type_from_type_node(e);
                            type_arguments.push(et);
                        }
                    }
                    _ => panic!("Unhandled case in getTypeArguments"),
                }
            }
            if self.pop_type_resolution() {
                if self
                    .ty(t)
                    .as_type_reference()
                    .resolved_type_arguments
                    .is_empty()
                {
                    let mapper = self.ty(t).as_type_reference().object.mapper;
                    let resolved = self.instantiate_types(&type_arguments, mapper);
                    self.ty_mut(t)
                        .as_type_reference_mut()
                        .resolved_type_arguments = resolved.into();
                }
            } else {
                if self
                    .ty(t)
                    .as_type_reference()
                    .resolved_type_arguments
                    .is_empty()
                {
                    let error_type = self.error_type;
                    self.ty_mut(t)
                        .as_type_reference_mut()
                        .resolved_type_arguments = vec![error_type; type_parameter_count].into();
                }
                let error_node = if node.is_some() {
                    node
                } else {
                    self.current_node
                };
                let target_symbol = self.ty(target).symbol;
                if target_symbol.is_some() {
                    let symbol_string = self.symbol_to_string(target_symbol);
                    self.error(
                        error_node,
                        diag::Type_arguments_for_0_circularly_reference_themselves,
                        args![symbol_string],
                    );
                } else {
                    self.error(
                        error_node,
                        diag::Tuple_type_arguments_circularly_reference_themselves,
                        args![],
                    );
                }
            }
        }
        None
    }

    // Go: checker/checker.go:22324 getEffectiveTypeArguments
    pub fn get_effective_type_arguments(
        &mut self,
        node: Node,
        type_parameters: &[TypeId],
    ) -> Vec<TypeId> {
        let mut type_arguments: Vec<TypeId> = Vec::new();
        for a in node.type_arguments() {
            let at = self.get_type_from_type_node(a);
            type_arguments.push(at);
        }
        let min_type_argument_count = self.get_min_type_argument_count(type_parameters);
        self.fill_missing_type_arguments(
            &type_arguments,
            type_parameters,
            min_type_argument_count,
            is_in_js_file(node),
        )
    }

    // Gets the minimum number of type arguments needed to satisfy all non-optional type parameters.
    // Go: checker/checker.go:22329 getMinTypeArgumentCount
    pub fn get_min_type_argument_count(&mut self, type_parameters: &[TypeId]) -> i32 {
        let mut min_type_argument_count: i32 = 0;
        for (i, &type_parameter) in type_parameters.iter().enumerate() {
            if !self.has_type_parameter_default(type_parameter) {
                min_type_argument_count = i as i32 + 1;
            }
        }
        min_type_argument_count
    }

    // Go: checker/checker.go:22339 hasTypeParameterDefault
    pub fn has_type_parameter_default(&self, t: TypeId) -> bool {
        let symbol = self.ty(t).symbol;
        symbol.is_some()
            && self
                .sym(symbol)
                .declarations
                .iter()
                .any(|&d| is_type_parameter_declaration(d) && d.default_type().is_some())
    }

    // Go: checker/checker.go:22345 fillMissingTypeArguments
    pub fn fill_missing_type_arguments(
        &mut self,
        type_arguments: &[TypeId],
        type_parameters: &[TypeId],
        min_type_argument_count: i32,
        is_java_script_implicit_any: bool,
    ) -> Vec<TypeId> {
        let num_type_parameters = type_parameters.len();
        if num_type_parameters == 0 {
            return Vec::new();
        }
        let num_type_arguments = type_arguments.len();
        if is_java_script_implicit_any || num_type_arguments < num_type_parameters {
            let mut result = vec![TypeId::NIL; num_type_parameters];
            // PORT: Go `copy` copies min(len(dst), len(src)) elements.
            let copied = num_type_arguments.min(num_type_parameters);
            result[..copied].copy_from_slice(&type_arguments[..copied]);
            // Map invalid forward references in default types to the error type
            for i in num_type_arguments..num_type_parameters {
                result[i] = self.error_type;
            }
            let base_default_type =
                self.get_default_type_argument_type(is_java_script_implicit_any);
            for i in num_type_arguments..num_type_parameters {
                let mut default_type = self.get_default_from_type_parameter(type_parameters[i]);

                if is_java_script_implicit_any
                    && default_type.is_some()
                    && (self.is_type_identical_to(default_type, self.unknown_type)
                        || self.is_type_identical_to(default_type, self.empty_object_type))
                {
                    default_type = self.any_type;
                }

                if default_type.is_some() {
                    // PORT: Go's mapper shares the `result` backing array, so a
                    // mapper kept by a lazily resolved instantiation sees later
                    // writes. `new_type_mapper` copies the targets here.
                    let m = self.new_type_mapper(type_parameters, &result);
                    result[i] = self.instantiate_type(default_type, m);
                } else {
                    result[i] = base_default_type;
                }
            }
            return result;
        }
        type_arguments.to_vec()
    }

    // Go: checker/checker.go:22377 getDefaultTypeArgumentType
    pub fn get_default_type_argument_type(&self, is_in_java_script_file: bool) -> TypeId {
        if is_in_java_script_file {
            return self.any_type;
        }
        self.unknown_type
    }

    // Gets the default type for a type parameter. If the type parameter is the result of an instantiation,
    // this gets the instantiated default type of its target. If the type parameter has no default type or
    // the default is circular, `undefined` is returned.
    // Go: checker/checker.go:22387 getDefaultFromTypeParameter
    pub fn get_default_from_type_parameter(&mut self, t: TypeId) -> TypeId {
        if !self.ty(t).flags.intersects(TypeFlags::TYPE_PARAMETER) {
            return TypeId::NIL;
        }
        let default_type = self.get_resolved_type_parameter_default(t);
        if default_type != self.no_constraint_type && default_type != self.circular_constraint_type
        {
            return default_type;
        }
        TypeId::NIL
    }

    // Go: checker/checker.go:22398 getResolvedTypeParameterDefault
    pub fn get_resolved_type_parameter_default(&mut self, t: TypeId) -> TypeId {
        let resolved_default_type = self.ty(t).as_type_parameter().resolved_default_type;
        if resolved_default_type.is_nil() {
            let target = self.ty(t).as_type_parameter().target;
            if target.is_some() {
                let target_default = self.get_resolved_type_parameter_default(target);
                if target_default.is_some() {
                    let mapper = self.ty(t).as_type_parameter().mapper;
                    let instantiated = self.instantiate_type(target_default, mapper);
                    self.ty_mut(t).as_type_parameter_mut().resolved_default_type = instantiated;
                } else {
                    let no_constraint_type = self.no_constraint_type;
                    self.ty_mut(t).as_type_parameter_mut().resolved_default_type =
                        no_constraint_type;
                }
            } else {
                // To block recursion, set the initial value to the resolvingDefaultType.
                let resolving_default_type = self.resolving_default_type;
                self.ty_mut(t).as_type_parameter_mut().resolved_default_type =
                    resolving_default_type;
                let mut default_type = self.no_constraint_type;
                let symbol = self.ty(t).symbol;
                if symbol.is_some() {
                    let mut default_declaration = Node::NIL;
                    for &decl in &self.sym(symbol).declarations {
                        if is_type_parameter_declaration(decl) {
                            let d = decl.default_type();
                            if d.is_some() {
                                default_declaration = d;
                                break;
                            }
                        }
                    }
                    if default_declaration.is_some() {
                        default_type = self.get_type_from_type_node(default_declaration);
                    }
                }
                if self.ty(t).as_type_parameter().resolved_default_type
                    == self.resolving_default_type
                {
                    // If we have not been called recursively, set the correct default type.
                    self.ty_mut(t).as_type_parameter_mut().resolved_default_type = default_type;
                }
            }
        } else if resolved_default_type == self.resolving_default_type {
            // If we are called recursively for this type parameter, mark the default as circular.
            let circular_constraint_type = self.circular_constraint_type;
            self.ty_mut(t).as_type_parameter_mut().resolved_default_type = circular_constraint_type;
        }
        self.ty(t).as_type_parameter().resolved_default_type
    }

    // Go: checker/checker.go:22435 getDefaultOrUnknownFromTypeParameter
    pub fn get_default_or_unknown_from_type_parameter(&mut self, t: TypeId) -> TypeId {
        let result = self.get_default_from_type_parameter(t);
        if result.is_some() {
            result
        } else {
            self.unknown_type
        }
    }

    // Go: checker/checker.go:22440 getNamedMembers
    // PORT: returns the final `SharedList` that `set_structured_type_members`
    // stores (Go stores the returned slice), so the result is one exact-size
    // allocation. Go returns nil for an empty table; the empty list is nil.
    pub fn get_named_members(
        &mut self,
        members: SymbolTable,
        container: SymbolId,
    ) -> SharedList<SymbolId> {
        if self.symbols.len(members) == 0 {
            return SharedList::default();
        }
        // For classes and interfaces, we store explicitly declared members ahead of inherited members. This ensures we process
        // explicitly declared members first in type relations, which is beneficial because explicitly declared members are more
        // likely to contain discriminating differences. See for example https://github.com/microsoft/typescript-go/issues/1968.
        //
        // PORT: one pass over the table in table order does all the Go
        // tests. `is_named_member` is `!is_reserved_member_name(id)` and then
        // `symbol_is_value`. The first test only reads the table, so it runs
        // while the snapshot of candidates is built. `symbol_is_value` can
        // resolve aliases, so it runs on the snapshot, in table order as in
        // Go, and the containment test follows it as in the Go first loop.
        // The Go second loop repeats both tests and gets the same answers
        // (alias resolution is cached, containment only reads), so each
        // symbol is tested once and its sort key goes to the contained or
        // the other list. Both lists keep table order and get the stable
        // sort and comparator of `sort_symbols`, so the lazy
        // `get_symbol_id` calls in the comparisons happen in the same order.
        //
        // The buffers are reused across calls. They are taken out of the
        // checker so the loop can call `&mut self` methods; a nested call
        // (through alias resolution) finds them empty and uses its own.
        let mut scratch = std::mem::take(&mut self.named_members_scratch);
        let NamedMembersScratch {
            symbols: candidates,
            keys,
            other_keys,
            container_ranges,
        } = &mut scratch;
        candidates.clear();
        keys.clear();
        other_keys.clear();
        container_ranges.clear();
        candidates.extend(
            self.symbols
                .iter_names(members)
                .filter(|(id, _)| !is_reserved_member_name_of(id))
                .map(|(_, symbol)| symbol),
        );
        let is_class_or_interface_container = container.is_some()
            && self
                .sym(container)
                .flags
                .intersects(SymbolFlags::CLASS | SymbolFlags::INTERFACE);
        // `is_declaration_contained_by` reads the container's declaration
        // ranges for each member. They do not change during the loop, so they
        // are read once here and each member range is tested against them.
        if is_class_or_interface_container {
            container_ranges.extend(self.sym(container).declarations.iter().map(|d| d.loc()));
        }
        let mut last_file = (Node::NIL, 0);
        for &symbol in candidates.iter() {
            if !self.symbol_is_value(symbol) {
                continue;
            }
            let key = self.symbol_sort_key(symbol, &mut last_file);
            if is_class_or_interface_container
                && !self.is_declaration_contained_in(symbol, container_ranges)
            {
                other_keys.push(key);
            } else {
                keys.push(key);
            }
        }
        let contained_count = if is_class_or_interface_container {
            keys.len()
        } else {
            0
        };
        keys.append(other_keys);
        self.sort_symbol_sort_keys(&mut keys[..contained_count]);
        self.sort_symbol_sort_keys(&mut keys[contained_count..]);
        candidates.clear();
        candidates.extend(keys.iter().map(|key| key.symbol));
        let result = SharedList::from(candidates.as_slice());
        self.named_members_scratch = scratch;
        result
    }

    /// `get_named_members(members, container)` for a table `members` that
    /// `instantiate_symbol_table` made from the declared member table
    /// `declared` (Go `resolveObjectTypeMembers`).
    // PERF: not in Go. Such a table holds, in the order of `declared`, the
    // named members of `declared` or their instantiations. An instantiation
    // keeps the flags, name, declarations and value declaration of its
    // member, so each position of every such table has the same
    // `get_named_members` inputs, except the symbol id. When no member is an
    // alias (then `symbol_is_value` reads only the flags) and each sorted part
    // is in strictly increasing sort order (`SymbolSortKey::order`: the
    // packed declaration position, or for a member with no declaration the
    // name prefix; then the sort compares only those orders: no name text
    // and no `get_symbol_id`), every such table gives the same result
    // positions. The first table runs `get_named_members` and keeps its
    // positions for `(declared, container)`. Later tables read their result
    // at those positions: the same list, with none of the reads and no sort.
    // Debug builds compare it with `get_named_members`.
    pub(crate) fn get_named_members_of_instantiation(
        &mut self,
        members: SymbolTable,
        declared: SymbolTable,
        container: SymbolId,
    ) -> SharedList<SymbolId> {
        let key = (declared, container);
        match self.named_members_orders.get(&key) {
            Some(Some(order)) if order.len as usize == self.symbols.len(members) => {
                let positions = Rc::clone(&order.positions);
                let values: SmallVec<[SymbolId; 32]> = self
                    .symbols
                    .iter_names(members)
                    .map(|(_, symbol)| symbol)
                    .collect();
                let mut scratch = std::mem::take(&mut self.named_members_scratch);
                scratch.symbols.clear();
                scratch
                    .symbols
                    .extend(positions.iter().map(|&position| values[position as usize]));
                let result = SharedList::from(scratch.symbols.as_slice());
                self.named_members_scratch = scratch;
                debug_assert_eq!(
                    &result[..],
                    &self.get_named_members(members, container)[..],
                    "named members order of an instantiated table"
                );
                result
            }
            Some(_) => self.get_named_members(members, container),
            None => {
                let result = self.get_named_members(members, container);
                let order = self.named_members_order(members, container, &result);
                self.named_members_orders.insert(key, order);
                result
            }
        }
    }

    /// The table positions of `result`, the `get_named_members(members,
    /// container)` of an instantiated table, when every table instantiated
    /// from the same declared table gives the same positions (see
    /// `get_named_members_of_instantiation`); `None` otherwise.
    fn named_members_order(
        &self,
        members: SymbolTable,
        container: SymbolId,
        result: &[SymbolId],
    ) -> Option<NamedMembersOrder> {
        let len = self.symbols.len(members);
        let mut positions_of: FxHashMap<SymbolId, u32> = FxHashMap::default();
        for (position, (_, symbol)) in self.symbols.iter_names(members).enumerate() {
            if self.sym(symbol).flags.intersects(SymbolFlags::ALIAS)
                || positions_of
                    .insert(symbol, u32::try_from(position).ok()?)
                    .is_some()
            {
                return None;
            }
        }
        let is_class_or_interface_container = container.is_some()
            && self
                .sym(container)
                .flags
                .intersects(SymbolFlags::CLASS | SymbolFlags::INTERFACE);
        let container_ranges: Vec<TextRange> = if is_class_or_interface_container {
            self.sym(container)
                .declarations
                .iter()
                .map(|d| d.loc())
                .collect()
        } else {
            Vec::new()
        };
        let mut last_file = (Node::NIL, 0);
        let mut previous: Option<(bool, u64)> = None;
        let mut positions = Vec::with_capacity(result.len());
        for &symbol in result {
            let contained = !is_class_or_interface_container
                || self.is_declaration_contained_in(symbol, &container_ranges);
            let order = self.symbol_sort_key(symbol, &mut last_file).order;
            if order == NO_SORT_ORDER {
                return None;
            }
            match previous {
                // A new part starts where the contained members end.
                Some((was_contained, previous_order)) => {
                    if was_contained == contained {
                        if previous_order >= order {
                            return None;
                        }
                    } else if !(was_contained && !contained) {
                        return None;
                    }
                }
                None => {}
            }
            previous = Some((contained, order));
            positions.push(*positions_of.get(&symbol)?);
        }
        Some(NamedMembersOrder {
            len: u32::try_from(len).ok()?,
            positions: positions.into(),
        })
    }

    // Go: checker/checker.go:22467 isDeclarationContainedBy
    pub fn is_declaration_contained_by(&self, symbol: SymbolId, container: SymbolId) -> bool {
        let declaration = self.sym(symbol).value_declaration;
        if declaration.is_some() {
            for &d in &self.sym(container).declarations {
                // PORT: Go `core.TextRange.ContainedBy` is the `TextRange`
                // method `contained_by`.
                if declaration.loc().contained_by(d.loc()) {
                    return true;
                }
            }
        }
        false
    }

    /// `is_declaration_contained_by` with the container's declaration ranges
    /// already read (`get_named_members`).
    fn is_declaration_contained_in(
        &self,
        symbol: SymbolId,
        container_ranges: &[TextRange],
    ) -> bool {
        let declaration = self.sym(symbol).value_declaration;
        if declaration.is_some() {
            let range = declaration.loc();
            return container_ranges.iter().any(|&d| range.contained_by(d));
        }
        false
    }

    // Go: checker/checker.go:22478 isNamedMember
    // PORT: takes the table key as a `Name`, so a normal name reads no text
    // (`is_reserved_member_name_of`).
    pub fn is_named_member(&mut self, symbol: SymbolId, id: &Name) -> bool {
        !is_reserved_member_name_of(id) && self.symbol_is_value(symbol)
    }

    // Go: checker/checker.go:22482 symbolIsValue
    pub fn symbol_is_value(&mut self, symbol: SymbolId) -> bool {
        self.symbol_is_value_ex(symbol, false /*includeTypeOnlyMembers*/)
    }

    // Go: checker/checker.go:22486 symbolIsValueEx
    pub fn symbol_is_value_ex(
        &mut self,
        symbol: SymbolId,
        include_type_only_members: bool,
    ) -> bool {
        let flags = self.sym(symbol).flags;
        flags.intersects(SymbolFlags::VALUE)
            || flags.intersects(SymbolFlags::ALIAS)
                && self
                    .get_symbol_flags_ex(
                        symbol,
                        !include_type_only_members,
                        false, /*excludeLocalMeanings*/
                    )
                    .intersects(SymbolFlags::VALUE)
    }

    // Go: checker/checker.go:22491 instantiateType
    pub fn instantiate_type(&mut self, t: TypeId, m: MapperId) -> TypeId {
        self.instantiate_type_with_alias(t, m, None /*alias*/)
    }

    // Go: checker/checker.go:22495 instantiateTypeWithAlias
    pub fn instantiate_type_with_alias(
        &mut self,
        t: TypeId,
        m: MapperId,
        alias: Option<Rc<TypeAlias>>,
    ) -> TypeId {
        // Check for type variables in the alias, so things like `type Brand<T> = number & {}` can potentially be copied with new alias type args, despite them being unreferenced.
        // This is the behavior most people using aliases expect, and prevents the cache from leaking type parameters outside their scope of validity.
        // tests/cases/compiler/declarationEmitArrowFunctionNoRenaming.ts contains an example of this, which previously only worked in strada via some input node reuse logic instead.
        if t.is_nil() || m.is_nil() {
            return t;
        }
        let could_contain = self.could_contain_type_variables(t) || {
            // PORT: the alias list does not change after the type is made, so
            // it is read by index instead of copied.
            let count = self.ty(t).alias.type_arguments().len();
            (0..count).any(|i| {
                let a = self.ty(t).alias.type_arguments()[i];
                self.could_contain_type_variables(a)
            })
        };
        if !could_contain {
            return t;
        }
        if self.instantiation_stack.len() == 100 || self.instantiation_count >= 5_000_000 {
            // We have reached 100 recursive type instantiations, or 5M type instantiations caused by the same statement
            // or expression. There is a very high likelihood we're dealing with a combination of infinite generic types
            // that perpetually generate new type identities, so we stop the recursion here by yielding the error type.
            if let Some(tr) = self.tracer {
                tr.instant(
                    crate::tracing::Phase::CheckTypes,
                    "instantiateType_DepthLimit",
                    vec![
                        ("typeId", t.into()),
                        (
                            "instantiationDepth",
                            (self.instantiation_stack.len() as u32).into(),
                        ),
                        ("instantiationCount", self.instantiation_count.into()),
                    ],
                );
            }
            let circular_type_names = self.get_circular_type_names();
            let current_node = self.current_node;
            if circular_type_names.len() == 1 {
                self.error(
                    current_node,
                    diag::Instantiations_of_type_0_appear_infinitely_circular,
                    args![circular_type_names[0].clone()],
                );
            } else if circular_type_names.len() > 1 {
                self.error(
                    current_node,
                    diag::Instantiations_of_the_following_types_appear_infinitely_circular_Colon_0,
                    args![quoted_and_comma_separated(&circular_type_names)],
                );
            } else {
                self.error(
                    current_node,
                    diag::Type_instantiation_is_excessively_deep_and_possibly_infinite,
                    args![],
                );
            }
            return self.error_type;
        }
        let index = self.find_active_mapper(m);
        if index == -1 {
            self.push_active_mapper(m);
        }
        // PERF: a key with no alias is `t` itself, in the `plain` map
        // (`ActiveMapperCache`). `None` here means that key.
        let alias_key = alias.as_deref().map(|alias| {
            let mut b = KeyBuilder::default();
            b.write_type(t);
            b.write_alias(&self.symbols, Some(alias));
            b.hash()
        });
        let cache_index = if index != -1 {
            index as usize
        } else {
            self.active_mappers.len() - 1
        };
        let cache = &self.active_type_mappers_caches[cache_index];
        let cached = match &alias_key {
            None => cache.get_plain(t),
            Some(key) => cache.aliased.get(key),
        };
        if let Some(&cached_type) = cached {
            return cached_type;
        }
        self.total_instantiation_count += 1;
        self.instantiation_count += 1;
        self.instantiation_stack.push(t);
        let result = self.instantiate_type_worker(t, m, alias);
        if index == -1 {
            self.pop_active_mapper();
        } else {
            let cache = &mut self.active_type_mappers_caches[cache_index];
            match alias_key {
                None => cache.insert_plain(t, result),
                Some(key) => {
                    cache.aliased.insert(key, result);
                }
            }
        }
        self.instantiation_stack.pop();
        result
    }

    // Go: checker/checker.go:22546 getCircularTypeNames
    pub fn get_circular_type_names(&mut self) -> Vec<String> {
        let mut type_counts: FxHashMap<TypeId, i32> = FxHashMap::default();
        let mut circular_type_names: Vec<String> = Vec::new();
        let instantiation_stack = self.instantiation_stack.clone();
        for t in instantiation_stack {
            let count = type_counts.entry(t).or_insert(0);
            *count += 1;
            if *count == 3 {
                let mut symbol = self.ty(t).symbol;
                if let Some(alias) = &self.ty(t).alias {
                    symbol = alias.symbol;
                }
                if symbol.is_some() && {
                    let name = &self.sym(symbol).name;
                    !name.is_empty() && !name.starts_with(INTERNAL_SYMBOL_NAME_PREFIX)
                } {
                    let name = self.symbol_to_string_exported(symbol);
                    if !circular_type_names.contains(&name) {
                        circular_type_names.push(name);
                    }
                }
            }
        }
        circular_type_names
    }

    // Go: checker/checker.go:22566 pushActiveMapper
    // PORT: like Go, cleared maps stay in `active_type_mappers_caches` past
    // the active length for reuse. The active length is
    // `active_mappers.len()`; maps past it are always empty.
    pub fn push_active_mapper(&mut self, mapper: MapperId) {
        let last_index = self.active_mappers.len();
        self.active_mappers.push(mapper);
        if last_index >= self.active_type_mappers_caches.len() {
            self.active_type_mappers_caches
                .push(ActiveMapperCache::default());
        }
    }

    // Go: checker/checker.go:22581 popActiveMapper
    pub fn pop_active_mapper(&mut self) {
        // PORT: clearing costs time in the map capacity, so a mostly empty
        // large map is dropped instead. Both maps of the cache get this rule.
        fn drop_or_clear<K, V, S: Default>(map: &mut std::collections::HashMap<K, V, S>) {
            if map.capacity() > 256 && map.len() < map.capacity() / 8 {
                *map = std::collections::HashMap::default();
            } else if !map.is_empty() {
                map.clear();
            }
        }
        self.active_mappers.pop();
        // Clear the map, but leave it in the list for later reuse.
        let cache = &mut self.active_type_mappers_caches[self.active_mappers.len()];
        // The same rule on `plain`, when it has keys that were not recorded.
        cache.clear_plain(true);
        drop_or_clear(&mut cache.aliased);
    }

    // Go: checker/checker.go:22591 findActiveMapper
    pub fn find_active_mapper(&self, mapper: MapperId) -> i32 {
        match self.active_mappers.iter().rposition(|&m| m == mapper) {
            Some(i) => i as i32,
            None => -1,
        }
    }

    // Go: checker/checker.go:22595 clearActiveMapperCaches
    // PORT: Go `popActiveMapper` reslices `activeTypeMappersCaches` to the
    // active length, so Go clears only the active caches. Here the caches
    // past `active_mappers.len()` stay in the list for reuse and are always
    // empty (`pop_active_mapper` clears them), so they are skipped too.
    pub fn clear_active_mapper_caches(&mut self) {
        let active = self.active_mappers.len();
        debug_assert!(
            self.active_type_mappers_caches[active..]
                .iter()
                .all(ActiveMapperCache::is_empty)
        );
        for cache in &mut self.active_type_mappers_caches[..active] {
            cache.clear_plain(false);
            if !cache.aliased.is_empty() {
                cache.aliased.clear();
            }
        }
    }

    // Return true if the given type could possibly reference a type parameter for which
    // we perform type inference (i.e. a type parameter of a generic function). We cache
    // results for union and intersection types for performance reasons.
    // Go: checker/checker.go:22604 couldContainTypeVariablesWorker
    pub fn could_contain_type_variables_worker(&mut self, t: TypeId) -> bool {
        let flags = self.ty(t).flags;
        if !flags.intersects(TypeFlags::STRUCTURED_OR_INSTANTIABLE) {
            return false;
        }
        let object_flags = self.ty(t).object_flags;
        if object_flags.intersects(ObjectFlags::COULD_CONTAIN_TYPE_VARIABLES_COMPUTED) {
            return object_flags.intersects(ObjectFlags::COULD_CONTAIN_TYPE_VARIABLES);
        }
        let result = flags.intersects(TypeFlags::INSTANTIABLE)
            || flags.intersects(TypeFlags::OBJECT)
                && !self.is_non_generic_top_level_type(t)
                && (object_flags.intersects(ObjectFlags::REFERENCE)
                    && (self.ty(t).as_type_reference().node.is_some() || {
                        let type_arguments = self.get_type_arguments(t);
                        type_arguments
                            .iter()
                            .any(|&a| self.could_contain_type_variables(a))
                    })
                    || object_flags.intersects(ObjectFlags::ANONYMOUS) && {
                        let symbol = self.ty(t).symbol;
                        // PORT: Go `t.symbol.Declarations != nil`; a nil
                        // declaration list is empty here.
                        symbol.is_some()
                            && self.sym(symbol).flags.intersects(
                                SymbolFlags::FUNCTION
                                    | SymbolFlags::METHOD
                                    | SymbolFlags::CLASS
                                    | SymbolFlags::TYPE_LITERAL
                                    | SymbolFlags::OBJECT_LITERAL,
                            )
                            && !self.sym(symbol).declarations.is_empty()
                    }
                    || object_flags.intersects(
                        ObjectFlags::MAPPED
                            | ObjectFlags::REVERSE_MAPPED
                            | ObjectFlags::OBJECT_REST_TYPE
                            | ObjectFlags::INSTANTIATION_EXPRESSION_TYPE,
                    ))
            || flags.intersects(TypeFlags::UNION_OR_INTERSECTION)
                && !flags.intersects(TypeFlags::ENUM_LITERAL)
                && !self.is_non_generic_top_level_type(t)
                && {
                    (0..self.ty(t).types().len()).any(|i| {
                        let u = self.type_at(t, i);
                        self.could_contain_type_variables(u)
                    })
                };
        self.ty_mut(t).object_flags |= ObjectFlags::COULD_CONTAIN_TYPE_VARIABLES_COMPUTED
            | if result {
                ObjectFlags::COULD_CONTAIN_TYPE_VARIABLES
            } else {
                ObjectFlags::NONE
            };
        result
    }

    // Go: checker/checker.go:22621 isNonGenericTopLevelType
    pub fn is_non_generic_top_level_type(&self, t: TypeId) -> bool {
        if let Some(alias) = self.ty(t).alias.as_deref() {
            if alias.type_arguments.is_empty() {
                let mut declaration = get_declaration_of_kind(
                    &self.symbols,
                    alias.symbol,
                    SyntaxKind::TypeAliasDeclaration,
                );
                if declaration.is_nil() {
                    declaration = get_declaration_of_kind(
                        &self.symbols,
                        alias.symbol,
                        SyntaxKind::JsTypeAliasDeclaration,
                    );
                }
                return declaration.is_some()
                    && find_ancestor_or_quit(declaration.parent(), |n: Node| match n.kind() {
                        SyntaxKind::SourceFile => FindAncestorResult::FIND_ANCESTOR_TRUE,
                        SyntaxKind::ModuleDeclaration => FindAncestorResult::FIND_ANCESTOR_FALSE,
                        _ => FindAncestorResult::FIND_ANCESTOR_QUIT,
                    })
                    .is_some();
            }
        }
        false
    }

    // Go: checker/checker.go:22640 instantiateTypeWorker
    pub fn instantiate_type_worker(
        &mut self,
        t: TypeId,
        m: MapperId,
        alias: Option<Rc<TypeAlias>>,
    ) -> TypeId {
        let mut alias = alias;
        let flags = self.ty(t).flags;
        if flags.intersects(TypeFlags::TYPE_PARAMETER) {
            return self.get_mapped_type(t, m);
        } else if flags.intersects(TypeFlags::OBJECT) {
            let object_flags = self.ty(t).object_flags;
            if object_flags
                .intersects(ObjectFlags::REFERENCE | ObjectFlags::ANONYMOUS | ObjectFlags::MAPPED)
            {
                if object_flags.intersects(ObjectFlags::REFERENCE)
                    && self.ty(t).as_type_reference().node.is_nil()
                {
                    // PORT: Go `core.Same` on the `instantiateTypes` result
                    // is "no element changed" (`None`) here.
                    let count = self.ty(t).as_type_reference().resolved_type_arguments.len();
                    let Some(new_type_arguments) = self.map_stored_types_if_changed(
                        t,
                        count,
                        |c, t, i| c.ty(t).as_type_reference().resolved_type_arguments[i],
                        &mut |c, a| c.instantiate_type(a, m),
                    ) else {
                        return t;
                    };
                    let target = self.ty(t).target();
                    return self.create_normalized_type_reference(target, &new_type_arguments);
                }
                if object_flags.intersects(ObjectFlags::REVERSE_MAPPED) {
                    return self.instantiate_reverse_mapped_type(t, m);
                }
                return self.get_object_type_instantiation(t, m, alias);
            }
            return t;
        } else if flags.intersects(TypeFlags::UNION_OR_INTERSECTION) {
            let mut source = t;
            if self.ty(t).flags.intersects(TypeFlags::UNION) {
                let origin = self.ty(t).as_union_type().origin;
                if origin.is_some()
                    && self
                        .ty(origin)
                        .flags
                        .intersects(TypeFlags::UNION_OR_INTERSECTION)
                {
                    source = origin;
                }
            }
            let count = self.ty(source).types().len();
            let changed =
                self.map_stored_types_if_changed(source, count, Checker::type_at, &mut |c, u| {
                    c.instantiate_type(u, m)
                });
            if changed.is_none() && alias.symbol() == self.ty(t).alias.symbol() {
                return t;
            }
            let unchanged;
            let new_types: &[TypeId] = if let Some(types) = &changed {
                types
            } else {
                unchanged = self.ty(source).types_list();
                &unchanged
            };
            if alias.is_none() {
                let t_alias = self.ty(t).alias.clone();
                alias = self.instantiate_type_alias(t_alias, m);
            }
            if self.ty(source).flags.intersects(TypeFlags::INTERSECTION) {
                return self.get_intersection_type_ex(new_types, IntersectionFlags::NONE, alias);
            }
            return self.get_union_type_ex(
                new_types,
                UnionReduction::LITERAL,
                alias,
                TypeId::NIL, /*origin*/
            );
        } else if flags.intersects(TypeFlags::INDEX) {
            let target = self.ty(t).target();
            let instantiated = self.instantiate_type(target, m);
            return self.get_index_type(instantiated);
        } else if flags.intersects(TypeFlags::INDEXED_ACCESS) {
            if alias.is_none() {
                let t_alias = self.ty(t).alias.clone();
                alias = self.instantiate_type_alias(t_alias, m);
            }
            let (object_type, index_type, access_flags) = {
                let d = self.ty(t).as_indexed_access_type();
                (d.object_type, d.index_type, d.access_flags)
            };
            let new_object_type = self.instantiate_type(object_type, m);
            let new_index_type = self.instantiate_type(index_type, m);
            return self.get_indexed_access_type_ex(
                new_object_type,
                new_index_type,
                access_flags,
                Node::NIL, /*accessNode*/
                alias,
            );
        } else if flags.intersects(TypeFlags::TEMPLATE_LITERAL) {
            let (texts, types) = {
                let d = self.ty(t).as_template_literal_type();
                (d.texts.clone(), d.types.clone())
            };
            let new_types = self.instantiate_types(&types, m);
            return self.get_template_literal_type(&texts, &new_types);
        } else if flags.intersects(TypeFlags::STRING_MAPPING) {
            let symbol = self.ty(t).symbol;
            let target = self.ty(t).as_string_mapping_type().target;
            let instantiated = self.instantiate_type(target, m);
            return self.get_string_mapping_type(symbol, instantiated);
        } else if flags.intersects(TypeFlags::CONDITIONAL) {
            let conditional_mapper = self.ty(t).as_conditional_type().mapper;
            return self.get_conditional_type_instantiation_combined(
                t,
                conditional_mapper,
                m,
                false, /*forConstraint*/
                alias,
            );
        } else if flags.intersects(TypeFlags::SUBSTITUTION) {
            let (base_type, constraint) = {
                let d = self.ty(t).as_substitution_type();
                (d.base_type, d.constraint)
            };
            let new_base_type = self.instantiate_type(base_type, m);
            if self.is_no_infer_type(t) {
                return self.get_no_infer_type(new_base_type);
            }
            let new_constraint = self.instantiate_type(constraint, m);
            // A substitution type originates in the true branch of a conditional type and can be resolved
            // to just the base type in the same cases as the conditional type resolves to its true branch
            // (because the base type is then known to satisfy the constraint).
            if self
                .ty(new_base_type)
                .flags
                .intersects(TypeFlags::TYPE_VARIABLE)
                && self.is_generic_type(new_constraint)
            {
                return self.get_substitution_type(new_base_type, new_constraint);
            }
            if self
                .ty(new_constraint)
                .flags
                .intersects(TypeFlags::ANY_OR_UNKNOWN)
                || {
                    let restrictive_base = self.get_restrictive_instantiation(new_base_type);
                    let restrictive_constraint = self.get_restrictive_instantiation(new_constraint);
                    self.is_type_assignable_to(restrictive_base, restrictive_constraint)
                }
            {
                return new_base_type;
            }
            if self
                .ty(new_base_type)
                .flags
                .intersects(TypeFlags::TYPE_VARIABLE)
            {
                return self.get_substitution_type(new_base_type, new_constraint);
            }
            return self.get_intersection_type(&[new_constraint, new_base_type]);
        }
        t
    }
}

/// The `Name` of a table key: a `Name` key is copied, a text key is
/// interned.
fn table_key_name(key: TableKey<'_>) -> Name {
    match key {
        TableKey::Text(text) => Name::from(text),
        TableKey::Name(name) => name.clone(),
    }
}

/// Go `core.AppendIfUnique` for the declarations list of
/// `create_union_or_intersection_property`, which can grow long. While the
/// list is short, an add scans it. From `SCAN` nodes on, an open-addressing
/// table of the listed nodes answers instead. A node is pushed only when it
/// is new, so the list keeps Go's order.
// Go: core/core.go:380 AppendIfUnique
#[derive(Default)]
struct DeclarationSet {
    /// Empty until the list reaches `SCAN` nodes, then a power-of-two table
    /// at most half full. `Node::NIL` marks a free slot.
    slots: Vec<Node>,
}

impl DeclarationSet {
    /// The list length from which the table answers. Lists are mostly
    /// shorter than 8 or longer than 32; blueprint runs fastest from 8 to 16.
    const SCAN: usize = 16;

    /// Pushes `node` to `list` unless the list has it. `len_hint` is the
    /// expected final list length, which sizes the table. The table doubles
    /// before it is more than half full, so a longer list is also correct.
    /// `Node::NIL` cannot go in the table and takes the scan.
    fn add(&mut self, list: &mut SmallVec<[Node; 4]>, node: Node, len_hint: usize) {
        if list.len() < Self::SCAN || node.is_nil() {
            if !list.contains(&node) {
                list.push(node);
                if list.len() == Self::SCAN {
                    self.fill(list, (len_hint.max(Self::SCAN) * 2).next_power_of_two());
                }
            }
            return;
        }
        if list.len() * 2 >= self.slots.len() {
            self.fill(list, self.slots.len() * 2);
        }
        if self.insert(node) {
            list.push(node);
        }
    }

    /// Makes a table of `len` slots (a power of two) that has each listed
    /// node.
    fn fill(&mut self, list: &[Node], len: usize) {
        self.slots = vec![Node::NIL; len];
        for &listed in list {
            self.insert(listed);
        }
    }

    /// Puts `node` in the table. False when the table has it.
    fn insert(&mut self, node: Node) -> bool {
        let mask = self.slots.len() - 1;
        // Fibonacci hashing: the top bits of the product pick the first slot.
        let shift = 64 - self.slots.len().trailing_zeros();
        let mut i = (node.0.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> shift) as usize;
        loop {
            let slot = self.slots[i];
            if slot.is_nil() {
                self.slots[i] = node;
                return true;
            }
            if slot == node {
                return false;
            }
            i = (i + 1) & mask;
        }
    }
}

/// Go `orderedSet.Add` for a small symbol set kept as a list. Lookups scan the
/// list while it is short; from `ORDERED_SYMBOL_SET_SCAN` symbols on, `index`
/// holds every member and answers them instead.
fn ordered_symbol_set_add(
    list: &mut SmallVec<[SymbolId; 8]>,
    index: &mut FxHashSet<SymbolId>,
    s: SymbolId,
) {
    const ORDERED_SYMBOL_SET_SCAN: usize = 32;
    if list.len() < ORDERED_SYMBOL_SET_SCAN {
        if !list.contains(&s) {
            list.push(s);
            if list.len() == ORDERED_SYMBOL_SET_SCAN {
                index.extend(list.iter().copied());
            }
        }
    } else if index.insert(s) {
        list.push(s);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::checker::utilities_p1::union_sort_tests::with_alias_types;

    /// The example of `some_property_reduces_to_never`. In
    /// `A<string> & T & { kind: "b" }`, where
    /// `T extends A<string> | A<number>`, the first `m` is the public method
    /// of `A<string>`. The `m` of `T` comes next: the union property of
    /// `A<string> | A<number>`, a property with the same value declaration
    /// (Go `createUnionOrIntersectionProperty`, checker.go:21821 and
    /// :21988). So it is skipped, and `m` stays with the public methods:
    /// `kind` (`"a" & "a" & "b"` is never, a discriminant, Go
    /// `isDiscriminantWithNeverType`, checker.go:22273) is tested first,
    /// though `m` is seen first, and the test returns there. The property
    /// cache of the intersection then has no `m`. With no skip, `m` would
    /// be tested first. Go keeps the order of an intersection
    /// (`addTypeToIntersection`, checker.go:26723), and distributes an
    /// intersection over a union (checker.go:26508), so
    /// `A<string> & (A<string> | B)` is a union.
    #[test]
    fn a_skipped_property_can_be_a_union_property() {
        const SOURCE: &str = r#"
interface A<X> { m(): X; kind: "a" }
interface B { m: number }
type D = A<string> & (A<string> | B);
type I<T extends A<string> | A<number>> = A<string> & T & { kind: "b" };
"#;
        let got = with_alias_types(SOURCE, |c, types| {
            let (d, i) = (types[0], types[1]);
            let constituents = c.ty(i).types().to_vec();
            let props: Vec<SymbolId> = constituents[..2]
                .iter()
                .map(|&u| {
                    let props = c.get_properties_of_type(u);
                    props
                        .iter()
                        .copied()
                        .find(|&p| c.sym(p).name == "m")
                        .expect("A<string> and T have m")
                })
                .collect();
            let public_methods: Vec<bool> = props.iter().map(|&p| c.is_public_method(p)).collect();
            let reduces = c.some_property_reduces_to_never(i);
            let cache = c
                .ty(i)
                .as_union_or_intersection_type()
                .property_cache_without_function_property_augment;
            (
                c.ty(d).flags.intersects(TypeFlags::UNION),
                c.ty(i).flags.intersects(TypeFlags::INTERSECTION),
                constituents.len(),
                c.sym(props[0]).value_declaration.is_some()
                    && c.sym(props[0]).value_declaration == c.sym(props[1]).value_declaration,
                public_methods,
                reduces,
                ["kind", "m"].map(|name| c.symbols.get(cache, name).is_some()),
            )
        });
        assert_eq!(
            got,
            (true, true, 3, true, vec![true, false], true, [true, false])
        );
    }

    /// `get_reduced_apparent_type` returns early for object types (plain,
    /// mapped, homomorphic mapped over an array) with the type that Go's
    /// `getReducedType(getApparentType(getReducedType(t)))` gives
    /// (checker.go:22251). A union property takes the value type of the
    /// index signature of each member that lacks the property and has one
    /// for the name's literal type (`a${string}` holds `"a"`, `x${string}`
    /// does not), with the literal type made once (Go
    /// `getApplicableIndexInfoForName`, checker.go:19352, per member).
    #[test]
    fn reduced_apparent_types_and_index_members_of_union_properties() {
        const SOURCE: &str = r#"
interface A { a: string }
interface B { [k: string]: number }
interface C { [k: string]: boolean }
interface D { [k: `x${string}`]: null }
interface E { [k: `a${string}`]: bigint }
type U = A | B | C | D | E;
type M = Record<"a" | "b", 1>;
type H<T> = { [K in keyof T]: T[K] };
type HA = H<string[]>;
type I = A & M;
"#;
        let got = with_alias_types(SOURCE, |c, types| {
            let same: Vec<bool> = types
                .iter()
                .map(|&t| {
                    let fast = c.get_reduced_apparent_type(t);
                    let reduced = c.get_reduced_type(t);
                    let apparent = c.get_apparent_type(reduced);
                    fast == c.get_reduced_type(apparent)
                })
                .collect();
            let u = types[0];
            let a = c.get_union_or_intersection_property(u, "a", false);
            let partial = c.sym(a).check_flags & CheckFlags::PARTIAL;
            let a_type = c.get_type_of_symbol(a);
            (same, partial, c.type_to_string(a_type))
        });
        assert_eq!(
            got,
            (
                vec![true; got.0.len()],
                CheckFlags::READ_PARTIAL | CheckFlags::WRITE_PARTIAL,
                "string | number | bigint | boolean".to_string(),
            )
        );
        assert!(got.0.len() >= 4);
    }

    /// `DeclarationSet::add` gives the list of Go `core.AppendIfUnique`
    /// (a scan per add), before and after the table takes over, for nodes
    /// of several files.
    #[test]
    fn declaration_set_keeps_the_append_if_unique_list() {
        let mut seed = 0x2545_f491_4f6c_dd1d_u64;
        let mut cases: Vec<Vec<Node>> = Vec::new();
        for (adds, distinct) in [
            (10, 4),
            (16, 16),
            (40, 16),
            (17, 17),
            (700, 300),
            (2000, 2000),
        ] {
            let nodes = (0..adds)
                .map(|_| {
                    seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
                    let k = (seed >> 33) % distinct;
                    Node(((k % 3 + 1) << 32) | (k * 7 + 1))
                })
                .collect();
            cases.push(nodes);
        }
        // Every node distinct: the table is filled to its bound.
        cases.push((1..=64).map(|k| Node((1 << 32) | k)).collect());
        for nodes in cases {
            let mut want: Vec<Node> = Vec::new();
            for &node in &nodes {
                if !want.contains(&node) {
                    want.push(node);
                }
            }
            let mut set = DeclarationSet::default();
            let mut got: SmallVec<[Node; 4]> = SmallVec::new();
            for &node in &nodes {
                set.add(&mut got, node, nodes.len());
            }
            assert_eq!(got.as_slice(), want.as_slice());
        }
    }

    /// `DeclarationSet::add` gives the `core.AppendIfUnique` list when the
    /// list grows past `len_hint`: the table doubles before it is more than
    /// half full (a full table would loop forever). `Node::NIL`, which marks
    /// a free slot, takes the scan and is listed once.
    #[test]
    fn declaration_set_grows_past_its_hint() {
        let node = |k: u64| Node(((k % 3 + 1) << 32) | k);
        let mut nodes: Vec<Node> = (1..=300).map(node).collect();
        nodes.extend((1..=300).step_by(7).map(node));
        nodes.insert(40, Node::NIL);
        nodes.push(Node::NIL);
        let mut want: Vec<Node> = Vec::new();
        for &node in &nodes {
            if !want.contains(&node) {
                want.push(node);
            }
        }
        for len_hint in [0, 16, 40] {
            let mut set = DeclarationSet::default();
            let mut got: SmallVec<[Node; 4]> = SmallVec::new();
            for &node in &nodes {
                set.add(&mut got, node, len_hint);
                let used = set.slots.iter().filter(|slot| slot.is_some()).count();
                assert!(
                    used * 2 <= set.slots.len(),
                    "len_hint {len_hint}: {used} used"
                );
            }
            assert_eq!(got.as_slice(), want.as_slice(), "len_hint {len_hint}");
        }
    }

    /// `Node::NIL` added while the list is scanned, before the table takes
    /// over (R183 reviewer item 7): the fill puts each other listed node in
    /// the table, NIL stays listed once, and the later adds of NIL and of
    /// listed nodes (the first 20 again) push nothing. NIL as the add that fills the table
    /// (the 16th node) too. Go `core.AppendIfUnique` (core.go:380) lists a
    /// nil element like any other.
    #[test]
    fn declaration_set_lists_nil_added_before_the_table() {
        let node = |k: u64| Node(((k % 3 + 1) << 32) | k);
        for nil_at in [0, 3, DeclarationSet::SCAN - 1] {
            let mut nodes: Vec<Node> = (1..=100).map(node).collect();
            nodes.insert(nil_at, Node::NIL);
            nodes.extend([Node::NIL, node(50), Node::NIL, node(100)]);
            nodes.extend((1..=20).map(node));
            let mut want: Vec<Node> = Vec::new();
            for &node in &nodes {
                if !want.contains(&node) {
                    want.push(node);
                }
            }
            for len_hint in [0, 16, 200] {
                let mut set = DeclarationSet::default();
                let mut got: SmallVec<[Node; 4]> = SmallVec::new();
                for (i, &node) in nodes.iter().enumerate() {
                    set.add(&mut got, node, len_hint);
                    if i == DeclarationSet::SCAN {
                        assert!(!set.slots.is_empty(), "nil at {nil_at}: no table");
                    }
                }
                assert_eq!(
                    got.as_slice(),
                    want.as_slice(),
                    "nil at {nil_at}, len_hint {len_hint}"
                );
                let listed = got.iter().filter(|node| node.is_some()).count();
                let used = set.slots.iter().filter(|slot| slot.is_some()).count();
                assert_eq!(used, listed, "nil at {nil_at}, len_hint {len_hint}");
            }
        }
    }
}
