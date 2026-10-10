//! lazymem1: lazy member tables, a port of microsoft/TypeScript#64475
//! (open, head 2aefafb51b54, not in the pin; the Go model is in
//! `target/continuation-r97-goport/lazymem1/go-model.md`).
//!
//! An instantiated reference to a class or interface gets a lazy member
//! table in place of resolved members. The table has the signatures, the
//! index infos and the instantiated base types, made at the time the eager
//! path resolves the members, so types are made in the same order. A
//! declared member is instantiated only when a lookup asks for it, and the
//! full table (`resolve_lazy_members`) reuses it.
//!
//! `GOPORT_LAZY_MEMBERS=1` turns it on (`Checker::lazy_members`). Go has no
//! switch. With the switch off, nothing here runs and the checker does what
//! it did before. With it on, the output must equal Go at the pin with
//! #64475 applied.
//!
//! PERF (lazymem1 round 2): the hooks in the checker test
//! `MEMBERS_RESOLVED` once, read a resolved type in place and take an
//! unresolved type out of line, where they test the switch; only
//! `is_string_index_signature_only_type_worker` tests the switch for every
//! object type. They call this module through `opaque`, so it is not in the
//! call graph cycle of the checker. This module calls no checker function that
//! LLVM inlines at all its call sites: it has copies of those (see
//! `resolve_declared_members_lazy`). Round 1 called those functions and this
//! module directly, and the PGO + BOLT release build then inlined
//! differently on the member resolution and property access paths.
//!
//! PORT: Go keeps `*lazyMemberTable` in a map and a caller keeps its
//! pointer after a nested `resolveLazyMembers` deletes the entry. Here the
//! map holds an `Rc`, and a caller holds its own `Rc`: the code never looks
//! a table up again after a call that can run checker code. The ready parts
//! are one `OnceCell` (Go sets them together just before `ready = true`).
//! No borrow of `declared` is held across a call.

use crate::prelude::*;
use smallvec::SmallVec;
use std::cell::OnceCell;
use std::sync::OnceLock;

/// `GOPORT_LAZY_MEMBERS`, read once per process. Only `1` turns lazy member
/// tables on. `Checker::new` stores it in `Checker::lazy_members`.
pub fn lazy_members_from_env() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var("GOPORT_LAZY_MEMBERS").as_deref() == Ok("1"))
}

// Go: Checker.lazyMemberTables (#64475)
/// The lazy member tables of a checker, by type.
#[derive(Default)]
pub struct LazyMemberTables {
    map: FxHashMap<TypeId, Rc<LazyMemberTable>>,
    /// PERF: not in Go. The last ready table that
    /// `get_ready_lazy_member_table` gave: a shape query reads 3 to 5
    /// accessors of one type in a row. A table leaves the map only when its
    /// type is resolved, and a resolved type never reads it.
    last: Option<(TypeId, Rc<LazyMemberTable>)>,
}

impl LazyMemberTables {
    fn get(&self, t: TypeId) -> Option<&Rc<LazyMemberTable>> {
        self.map.get(&t)
    }

    #[inline]
    fn last_for(&self, t: TypeId) -> Option<Rc<LazyMemberTable>> {
        match &self.last {
            Some((last, lm)) if *last == t => Some(Rc::clone(lm)),
            _ => None,
        }
    }

    fn insert(&mut self, t: TypeId, lm: Rc<LazyMemberTable>) {
        self.map.insert(t, lm);
    }

    fn remove(&mut self, t: TypeId) {
        self.map.remove(&t);
        if self.last.as_ref().is_some_and(|(last, _)| *last == t) {
            self.last = None;
        }
    }

    /// The ready table of `t` for `resolve_type_reference_members` (Go
    /// `lm != nil && lm.ready`).
    pub(crate) fn ready(&self, t: TypeId) -> Option<Rc<LazyMemberTable>> {
        self.map.get(&t).filter(|lm| lm.is_ready()).cloned()
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    pub fn contains_key(&self, t: &TypeId) -> bool {
        self.map.contains_key(t)
    }
}

// Go: lazyMemberTable (#64475)
/// The lazy member table of one instantiated reference
/// (`Checker::lazy_member_tables`).
pub struct LazyMemberTable {
    mapper: MapperId,
    /// Go `ready` and the fields that `prepareLazyMembers` sets with it.
    ready: OnceCell<LazyMembersReady>,
    /// Go `declared` and `unaffected`, keyed by `Name::id`. The prepare
    /// step stores each unaffected member as itself, which is what Go
    /// `getLazyDeclaredMember` returns for a name in `unaffected`.
    declared: RefCell<FxHashMap<u32, SymbolId>>,
}

/// The parts of a lazy member table that the prepare step sets.
struct LazyMembersReady {
    /// The call signatures, then the construct signatures (the
    /// `StructuredSignatures` layout, so `resolve_lazy_members` stores the
    /// list without a copy).
    signatures: SharedList<SignatureId>,
    call_signature_count: usize,
    index_infos: SharedList<IndexInfoId>,
    base_types: SharedList<TypeId>,
}

impl LazyMemberTable {
    /// Go `lm.ready`.
    pub(crate) fn is_ready(&self) -> bool {
        self.ready.get().is_some()
    }

    fn ready(&self) -> &LazyMembersReady {
        self.ready
            .get()
            .expect("lazy member table read before it is ready")
    }

    fn signatures(&self, kind: SignatureKind) -> SharedList<SignatureId> {
        let ready = self.ready();
        let call_count = ready.call_signature_count;
        if kind == SignatureKind::CALL {
            return ready.signatures.slice(0..call_count);
        }
        ready.signatures.slice(call_count..ready.signatures.len())
    }
}

/// Go `isReservedMemberName` of a table key.
fn is_reserved_member_key(name: TableKey<'_>) -> bool {
    match name {
        TableKey::Name(name) => name.is_internal() && is_reserved_member_name(name.as_str()),
        TableKey::Text(text) => is_reserved_member_name(text),
    }
}

/// Copy of `SharedList::concat` (types.rs; see the PERF note at
/// `resolve_declared_members_lazy`).
fn concat_lazy<T: Copy + Default>(a: SharedList<T>, b: SharedList<T>) -> SharedList<T> {
    if b.is_empty() {
        return a;
    }
    if a.is_empty() {
        return b;
    }
    let mut items = Vec::with_capacity(a.len() + b.len());
    items.extend_from_slice(&a);
    items.extend_from_slice(&b);
    SharedList::from(&items[..])
}

/// `f`, through a value that LLVM cannot see through (`black_box`). The
/// hooks in the checker call this module with it.
/// PERF (lazymem1 round 2): this module calls back into the checker, so a
/// direct call from a hook put its functions in the call graph cycle of the
/// checker, and LLVM then inlined differently along that cycle in the PGO +
/// BOLT release build. An indirect call adds no call edge. With the switch
/// off it never runs, so PGO does not promote it to a direct call.
#[inline(always)]
pub(crate) fn opaque<F>(f: F) -> F {
    std::hint::black_box(f)
}

/// Copy of `is_thisless_variable_like_declaration` (checker_p23.rs; see
/// the PERF note at `resolve_declared_members_lazy`).
fn is_thisless_variable_like_declaration_lazy(node: Node) -> bool {
    let type_node = node.type_();
    if type_node.is_some() {
        return is_thisless_type(type_node);
    }
    node.initializer().is_nil()
}

/// Copy of `is_thisless_function_like_declaration` (checker_p23.rs).
fn is_thisless_function_like_declaration_lazy(node: Node) -> bool {
    let return_type = node.type_();
    (is_constructor_declaration(node) || return_type.is_some() && is_thisless_type(return_type))
        && node
            .parameters()
            .iter()
            .all(is_thisless_variable_like_declaration_lazy)
        && node
            .type_parameters()
            .iter()
            .all(is_thisless_type_parameter_lazy)
}

/// Copy of `is_thisless_type_parameter` (checker_p23.rs).
fn is_thisless_type_parameter_lazy(node: Node) -> bool {
    let constraint = node.constraint();
    constraint.is_nil() || is_thisless_type(constraint)
}

// Go: mayHaveLazyMembers (#64475)
#[inline]
fn may_have_lazy_members(t: &Type) -> bool {
    t.object_flags & (ObjectFlags::MEMBERS_RESOLVED | ObjectFlags::REFERENCE)
        == ObjectFlags::REFERENCE
}

impl Checker {
    // Go: getReferenceMemberTypeArguments (#64475)
    /// The type parameters of `source` and the type arguments of `t`, with
    /// `t` as the `this` argument when only that one is missing.
    // PORT: `resolve_type_reference_members` keeps its own copy; keep the
    // two in step.
    fn get_reference_member_type_arguments(
        &mut self,
        t: TypeId,
        source: TypeId,
    ) -> (Vec<TypeId>, Vec<TypeId>) {
        let type_parameters = self
            .ty(source)
            .as_interface_type()
            .all_type_parameters
            .clone();
        // One exact-size allocation: the arguments, then `t` as the `this`
        // argument when only that one is missing.
        let type_arguments = {
            let type_arguments = self.type_arguments_of(t);
            let pad = type_arguments.len() == type_parameters.len().wrapping_sub(1);
            let mut padded = Vec::with_capacity(type_arguments.len() + usize::from(pad));
            padded.extend_from_slice(&type_arguments);
            if pad {
                padded.push(t);
            }
            padded
        };
        (type_parameters, type_arguments)
    }

    // Go: getReadyLazyMemberTable (#64475)
    /// The lazy member table of `t`, or `None` when the switch is off, `t`
    /// is resolved or out of scope, or its table is still being prepared.
    /// The first call makes and prepares the table.
    #[inline]
    pub(crate) fn get_ready_lazy_member_table(&mut self, t: TypeId) -> Option<Rc<LazyMemberTable>> {
        if !self.lazy_members || !may_have_lazy_members(self.ty(t)) {
            return None;
        }
        if let Some(lm) = self.lazy_member_tables.last_for(t) {
            return Some(lm);
        }
        self.get_ready_lazy_member_table_worker(t)
    }

    // Go: getReadyLazyMemberTableWorker (#64475)
    #[inline(never)]
    fn get_ready_lazy_member_table_worker(&mut self, t: TypeId) -> Option<Rc<LazyMemberTable>> {
        let ty = self.ty(t);
        if !ty.flags.intersects(TypeFlags::OBJECT) {
            return None;
        }
        let source = ty.target();
        let symbol = ty.symbol;
        if source.is_nil() || source == t {
            return None;
        }
        let source_flags = self.ty(source).object_flags;
        if !source_flags.intersects(ObjectFlags::CLASS_OR_INTERFACE)
            || source_flags.intersects(ObjectFlags::TUPLE)
            || symbol.is_some() && self.sym(symbol).flags.intersects(SymbolFlags::VALUE_MODULE)
        {
            return None;
        }
        let lm = match self.lazy_member_tables.get(t) {
            Some(lm) => Rc::clone(lm),
            None => {
                let (type_parameters, type_arguments) =
                    self.get_reference_member_type_arguments(t, source);
                if type_parameters == type_arguments {
                    return None;
                }
                let mapper = self.new_type_mapper(&type_parameters, &type_arguments);
                let lm = Rc::new(LazyMemberTable {
                    mapper,
                    ready: OnceCell::new(),
                    declared: RefCell::default(),
                });
                self.lazy_member_tables.insert(t, Rc::clone(&lm));
                self.prepare_lazy_members(t, &lm, &type_arguments);
                lm
            }
        };
        if lm.ready.get().is_none()
            || self
                .ty(t)
                .object_flags
                .intersects(ObjectFlags::MEMBERS_RESOLVED)
        {
            return None;
        }
        self.lazy_member_tables.last = Some((t, Rc::clone(&lm)));
        Some(lm)
    }

    // Go: prepareLazyMembers (#64475)
    /// Mirrors `resolve_object_type_members` without making member symbols.
    fn prepare_lazy_members(&mut self, t: TypeId, lm: &LazyMemberTable, type_arguments: &[TypeId]) {
        let source = self.ty(t).target();
        let (
            declared_members,
            declared_call_signatures,
            declared_construct_signatures,
            declared_index_infos,
        ) = {
            let resolved = self.resolve_declared_members_lazy(source);
            (
                resolved.declared_members,
                resolved.declared_call_signatures.clone(),
                resolved.declared_construct_signatures.clone(),
                resolved.declared_index_infos.clone(),
            )
        };
        // Whether instantiate_symbol returns a member itself depends on what
        // is resolved now.
        // PORT: Go loops over a map (random order); this is table order.
        let entries: SmallVec<[(Name, SymbolId); 16]> =
            self.symbols.iter_names(declared_members).collect();
        for (id, symbol) in entries {
            if self.is_named_member(symbol, &id)
                && self.is_symbol_unaffected_by_instantiation(symbol, lm.mapper)
            {
                lm.declared.borrow_mut().insert(id.id(), symbol);
            }
        }
        let mapper = lm.mapper;
        let mut call_signatures = self.instantiate_shared_list_lazy(
            declared_call_signatures,
            mapper,
            Checker::instantiate_signature,
        );
        let mut construct_signatures = self.instantiate_shared_list_lazy(
            declared_construct_signatures,
            mapper,
            Checker::instantiate_signature,
        );
        let mut index_infos = self.instantiate_shared_list_lazy(
            declared_index_infos,
            mapper,
            Checker::instantiate_index_info,
        );
        let this_argument = type_arguments.last().copied().unwrap_or(TypeId::NIL);
        let source_base_types = self.get_base_types_shared(source);
        let mut base_types: Vec<TypeId> = Vec::with_capacity(source_base_types.len());
        for base_type in source_base_types {
            let mut instantiated_base_type = base_type;
            if this_argument.is_some() {
                let instantiated_type = self.instantiate_type(base_type, mapper);
                instantiated_base_type = self.get_type_with_this_argument(
                    instantiated_type,
                    this_argument,
                    false, /*needsApparentType*/
                );
            }
            base_types.push(instantiated_base_type);
            let reduced = self.get_reduced_apparent_type(instantiated_base_type);
            if self.get_ready_lazy_member_table(reduced).is_none() {
                self.get_properties_of_type(instantiated_base_type);
            }
            (call_signatures, construct_signatures, index_infos) = self
                .append_inherited_signatures_and_index_infos(
                    call_signatures,
                    construct_signatures,
                    index_infos,
                    instantiated_base_type,
                );
        }
        let call_signature_count = call_signatures.len();
        let ready = LazyMembersReady {
            signatures: concat_lazy(call_signatures, construct_signatures),
            call_signature_count,
            index_infos,
            base_types: base_types.into(),
        };
        if lm.ready.set(ready).is_err() {
            panic!("lazy member table prepared twice");
        }
        if self
            .ty(t)
            .object_flags
            .intersects(ObjectFlags::MEMBERS_RESOLVED)
        {
            // t was resolved while preparing; resolveObjectTypeMembers would
            // now replace its members.
            self.resolve_lazy_members(t, lm);
        }
    }

    // Go: resolveLazyMembers (#64475)
    /// Builds the full members table of `t` from its lazy table and drops
    /// the table.
    pub(crate) fn resolve_lazy_members(&mut self, t: TypeId, lm: &LazyMemberTable) {
        let source = self.ty(t).target();
        let declared_members = self.resolve_declared_members_lazy(source).declared_members;
        let mut members = SymbolTable::NIL;
        let declared_count = self.symbols.len(declared_members);
        if declared_count != 0 {
            members = self.symbols.new_table_with_capacity(declared_count);
            // PORT: Go loops over a map (random order); this is table order,
            // the order of `instantiate_symbol_table`.
            let entries: SmallVec<[(Name, SymbolId); 16]> =
                self.symbols.iter_names(declared_members).collect();
            for (id, symbol) in entries {
                if self.is_named_member(symbol, &id) {
                    let member = self.get_lazy_declared_member(lm, symbol, &id);
                    self.symbols.set(members, id, member);
                }
            }
        }
        let (signatures, call_signature_count, index_infos, base_types) = {
            let ready = lm.ready();
            (
                ready.signatures.clone(),
                ready.call_signature_count,
                ready.index_infos.clone(),
                ready.base_types.clone(),
            )
        };
        for &base_type in base_types.iter() {
            let base_properties = self.get_properties_of_type(base_type);
            members = self.add_inherited_members_lazy(members, &base_properties);
        }
        // PORT: with no base types the table holds the named members of the
        // declared table in its order, as `instantiate_symbol_table` makes
        // it, so its properties come from the same cache.
        let declared = if base_types.is_empty() {
            declared_members
        } else {
            SymbolTable::NIL
        };
        self.set_structured_type_members_ex(
            t,
            members,
            declared,
            signatures,
            call_signature_count,
            index_infos,
        );
        self.augment_members_set(t);
        self.lazy_member_tables.remove(t);
    }

    /// Whether `t` has a ready lazy member table (`get_ready_lazy_member_table`),
    /// for `resolve_structured_type_members_unless_lazy`.
    #[cold]
    #[inline(never)]
    pub(crate) fn has_ready_lazy_member_table(&mut self, t: TypeId) -> bool {
        self.get_ready_lazy_member_table(t).is_some()
    }

    /// The first test of Go `resolveTypeReferenceMembers` with #64475: a
    /// ready lazy member table of `t` gives its full members. True when it
    /// did. `resolve_type_reference_members` calls it with the switch on.
    #[cold]
    #[inline(never)]
    pub(crate) fn resolve_type_reference_members_lazy(&mut self, t: TypeId) -> bool {
        let Some(lm) = self.lazy_member_tables.ready(t) else {
            return false;
        };
        self.resolve_lazy_members(t, &lm);
        true
    }

    // Go: getLazyDeclaredMember (#64475)
    fn get_lazy_declared_member(
        &mut self,
        lm: &LazyMemberTable,
        symbol: SymbolId,
        name: &Name,
    ) -> SymbolId {
        if let Some(&result) = lm.declared.borrow().get(&name.id()) {
            return result;
        }
        let result = self.new_instantiated_symbol(symbol, lm.mapper);
        lm.declared.borrow_mut().insert(name.id(), result);
        result
    }

    // Go: getMemberOfStructuredType (#64475)
    /// The member `name` of the object type `t`, from its lazy table when it
    /// has one.
    pub(crate) fn get_member_of_structured_type<'a>(
        &mut self,
        t: TypeId,
        name: impl Into<TableKey<'a>>,
    ) -> SymbolId {
        let name = name.into();
        let ty = self.ty(t);
        if ty.object_flags.intersects(ObjectFlags::MEMBERS_RESOLVED) {
            return self.symbols.get_key(ty.as_structured_type().members, name);
        }
        self.get_member_of_unresolved_structured_type(t, name)
    }

    // Go: getMemberOfUnresolvedStructuredType (#64475)
    #[inline(never)]
    pub(crate) fn get_member_of_unresolved_structured_type(
        &mut self,
        t: TypeId,
        name: TableKey<'_>,
    ) -> SymbolId {
        let lm = match self.get_ready_lazy_member_table(t) {
            Some(lm) if !is_reserved_member_key(name) => lm,
            _ => {
                let members = self.resolve_structured_type_members(t).members;
                return self.symbols.get_key(members, name);
            }
        };
        // The declared member, else the first base type's property (see
        // addInheritedMembers).
        let mut result = SymbolId::NIL;
        let source = self.ty(t).target();
        let declared_members = self.resolve_declared_members_lazy(source).declared_members;
        let decl = self.symbols.get_key(declared_members, name);
        if decl.is_some() {
            // The table holds the name, so the text is interned and `from`
            // adds nothing.
            let id = match name {
                TableKey::Name(name) => name.clone(),
                TableKey::Text(text) => Name::from(text),
            };
            if self.is_named_member(decl, &id) {
                result = self.get_lazy_declared_member(&lm, decl, &id);
            }
        }
        let base_types = lm.ready().base_types.clone();
        for &base_type in base_types.iter() {
            if result.is_some() && self.sym(result).flags.intersects(SymbolFlags::VALUE) {
                break;
            }
            // PERF: by the key's own type, so `create_union_or_intersection_property`
            // stays the one caller of the `TableKey` copy (LLVM then inlines
            // it there, as before this lane).
            let prop = match name {
                TableKey::Name(name) => self.get_property_of_type_ex(
                    base_type, name, true,  /*skipObjectFunctionPropertyAugment*/
                    false, /*includeTypeOnlyMembers*/
                ),
                TableKey::Text(text) => self.get_property_of_type_ex(
                    base_type, text, true,  /*skipObjectFunctionPropertyAugment*/
                    false, /*includeTypeOnlyMembers*/
                ),
            };
            if prop.is_some() && !self.is_static_private_identifier_property_lazy(prop) {
                result = prop;
            }
        }
        result
    }

    /// The object type case of `get_property_of_type_ex` (Go with #64475)
    /// for a type with a ready lazy member table, with the switch on.
    #[cold]
    #[inline(never)]
    pub(crate) fn get_property_of_unresolved_object_type_lazy(
        &mut self,
        t: TypeId,
        name: TableKey<'_>,
        skip_object_function_property_augment: bool,
        include_type_only_members: bool,
    ) -> SymbolId {
        // PORT: the object type case of `get_property_of_type_ex`, with the
        // member and the signature counts read through the accessors. Keep
        // the two in step.
        let mut symbol = self.get_member_of_unresolved_structured_type(t, name);
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
        // PERF (propfilt1): see `get_property_of_type_ex`.
        if let TableKey::Name(key) = name {
            if self.augment_lookups_miss_lazy(t, key) {
                return SymbolId::NIL;
            }
        }
        let (call_count, construct_count) = self.lazy_signature_counts(t);
        let function_type = if t == self.any_function_type {
            self.global_function_type
        } else if call_count != 0 {
            self.global_callable_function_type
        } else if construct_count != 0 {
            self.global_newable_function_type
        } else {
            TypeId::NIL
        };
        if function_type.is_some() {
            symbol = self.get_property_of_object_type_key(function_type, name);
            if symbol.is_some() {
                return symbol;
            }
        }
        let global_object_type = self.global_object_type;
        self.get_property_of_object_type_key(global_object_type, name)
    }

    /// Go `len(getSignaturesOfStructuredType(t, kind))` of both kinds.
    #[inline(never)]
    fn lazy_signature_counts(&mut self, t: TypeId) -> (usize, usize) {
        let call_count = self
            .get_signatures_of_structured_type(t, SignatureKind::CALL)
            .len();
        let construct_count = self
            .get_signatures_of_structured_type(t, SignatureKind::CONSTRUCT)
            .len();
        (call_count, construct_count)
    }

    /// The signatures of `kind` of `t` from its lazy table, when it has a
    /// ready one (the first test of Go `getSignaturesOfStructuredType` with
    /// #64475). `get_signatures_of_structured_type` calls it for a type
    /// whose members are not resolved, with the switch on.
    #[cold]
    #[inline(never)]
    pub(crate) fn lazy_signatures_of_structured_type(
        &mut self,
        t: TypeId,
        kind: SignatureKind,
    ) -> Option<SharedList<SignatureId>> {
        self.get_ready_lazy_member_table(t)
            .map(|lm| lm.signatures(kind))
    }

    /// The index infos of `t` from its lazy table, when it has a ready one
    /// (the first test of Go `getIndexInfosOfStructuredType` with #64475).
    /// `get_index_infos_of_structured_type` calls it as above.
    #[cold]
    #[inline(never)]
    pub(crate) fn lazy_index_infos_of_structured_type(
        &mut self,
        t: TypeId,
    ) -> Option<SharedList<IndexInfoId>> {
        self.get_ready_lazy_member_table(t)
            .map(|lm| lm.ready().index_infos.clone())
    }

    // Go: everyPropertyOfStructuredType (#64475)
    /// `f` may see a declared member instead of its instantiation, which has
    /// the same flags.
    pub(crate) fn every_property_of_structured_type(
        &mut self,
        t: TypeId,
        f: fn(&Symbol) -> bool,
    ) -> bool {
        if let Some(lm) = self.get_ready_lazy_member_table(t) {
            let mut seen = FxHashSet::default();
            return self.every_lazy_property(t, &lm, &mut seen, f);
        }
        self.resolve_structured_type_members(t);
        let resolved = self.ty(t).as_structured_type();
        resolved.properties.iter().all(|&p| f(self.sym(p)))
    }

    // Go: hasPropertiesOfStructuredType (#64475)
    pub(crate) fn has_properties_of_structured_type(&mut self, t: TypeId) -> bool {
        !self.every_property_of_structured_type(t, |_| false)
    }

    // Go: everyLazyProperty (#64475)
    /// `seen` has the ids of the names of properties that hide inherited
    /// ones, as in addInheritedMembers.
    fn every_lazy_property(
        &mut self,
        t: TypeId,
        lm: &LazyMemberTable,
        seen: &mut FxHashSet<u32>,
        f: fn(&Symbol) -> bool,
    ) -> bool {
        let source = self.ty(t).target();
        let declared_members = self.resolve_declared_members_lazy(source).declared_members;
        // PORT: Go loops over a map (random order); this is table order. The
        // answer is the same: `f` reads only flags.
        // PERF: `is_named_member` only reads a member that is not an alias,
        // so the loop runs in place up to the first alias, and goes on over
        // a snapshot of the rest from there.
        let mut alias_at = None;
        for (position, (id, symbol)) in self.symbols.iter_names(declared_members).enumerate() {
            let flags = self.sym(symbol).flags;
            let named = if id.is_internal() && is_reserved_member_name(id.as_str()) {
                false
            } else if flags.intersects(SymbolFlags::VALUE) {
                true
            } else if flags.intersects(SymbolFlags::ALIAS) {
                alias_at = Some(position);
                break;
            } else {
                false
            };
            if named && seen.insert(id.id()) && !f(self.sym(symbol)) {
                return false;
            }
        }
        if let Some(position) = alias_at {
            let entries: SmallVec<[(Name, SymbolId); 16]> = self
                .symbols
                .iter_names(declared_members)
                .skip(position)
                .collect();
            for (id, symbol) in entries {
                if self.is_named_member(symbol, &id) && seen.insert(id.id()) && !f(self.sym(symbol))
                {
                    return false;
                }
            }
        }
        let base_types = lm.ready().base_types.clone();
        for &base_type in base_types.iter() {
            let reduced = self.get_reduced_apparent_type(base_type);
            if let Some(base_table) = self.get_ready_lazy_member_table(reduced) {
                if !self.every_lazy_property(reduced, &base_table, seen, f) {
                    return false;
                }
                continue;
            }
            let properties = self.get_properties_of_type(base_type);
            for &prop in properties.iter() {
                if !self.is_static_private_identifier_property_lazy(prop)
                    && seen.insert(self.sym(prop).name.id())
                    && !f(self.sym(prop))
                {
                    return false;
                }
            }
        }
        true
    }

    // Go: isSymbolUnaffectedByInstantiation (#64475)
    /// Can change from false to true once the type of the symbol is
    /// resolved.
    // PORT: the first half of `instantiate_symbol` (#64475 splits Go's
    // instantiateSymbol in two). `instantiate_symbol` keeps its one body, so
    // keep the two in step.
    pub(crate) fn is_symbol_unaffected_by_instantiation(
        &mut self,
        symbol: SymbolId,
        m: MapperId,
    ) -> bool {
        // The links read gives the symbol its id first, as Go's `Get`.
        let (links_resolved_type, links_write_type) = {
            let links = self.value_symbol_links.get_by_id(&self.symbols, symbol);
            (links.resolved_type, links.write_type)
        };
        if m.is_some() && self.mapper(m).maps_this_only() && self.is_thisless_lazy(symbol) {
            return true;
        }
        // If the type of the symbol is already resolved, and if that type could not possibly
        // be affected by instantiation, simply return the symbol itself.
        if links_resolved_type.is_some() && !self.could_contain_type_variables(links_resolved_type)
        {
            if !self.sym(symbol).flags.intersects(SymbolFlags::SET_ACCESSOR) {
                return true;
            }
            // If we're a setter, check writeType.
            if links_write_type.is_some() && !self.could_contain_type_variables(links_write_type) {
                return true;
            }
        }
        false
    }

    // Go: newInstantiatedSymbol (#64475)
    // PORT: the second half of `instantiate_symbol`; keep the two in step.
    pub(crate) fn new_instantiated_symbol(&mut self, symbol: SymbolId, m: MapperId) -> SymbolId {
        let (links_target, links_mapper, links_name_type) = {
            let links = self.value_symbol_links.get_by_id(&self.symbols, symbol);
            (links.target, links.mapper, links.name_type)
        };
        let mut symbol = symbol;
        let mut m = m;
        if self
            .sym(symbol)
            .check_flags
            .intersects(CheckFlags::INSTANTIATED)
        {
            // If symbol being instantiated is itself a instantiation, fetch the original target and combine the
            // type mappers. This ensures that original type identities are properly preserved and that aliases
            // always reference a non-aliases.
            symbol = links_target;
            m = self.combine_type_mappers(links_mapper, m);
        }
        // Keep the flags from the symbol we're instantiating.  Mark that is instantiated, and
        // also transient so that we can just store data on it directly.
        // PORT: as in `instantiate_symbol`, the full symbol is pushed once.
        let full = {
            let s = self.sym(symbol);
            Symbol {
                flags: s.flags | SymbolFlags::TRANSIENT,
                name: s.name.clone(),
                check_flags: CheckFlags::INSTANTIATED
                    | s.check_flags
                        & (CheckFlags::READONLY
                            | CheckFlags::LATE
                            | CheckFlags::OPTIONAL_PARAMETER
                            | CheckFlags::REST_PARAMETER),
                declarations: s.declarations.clone(),
                parent: s.parent,
                value_declaration: s.value_declaration,
                ..Symbol::default()
            }
        };
        self.symbol_count += 1;
        let result = self.symbols.push_symbol(full);
        self.value_symbol_links.insert_new_by_id(
            &self.symbols,
            result,
            ValueSymbolLinks {
                target: symbol,
                mapper: m,
                name_type: links_name_type,
                ..ValueSymbolLinks::default()
            },
        );
        result
    }

    // Go: appendInheritedSignaturesAndIndexInfos (#64475)
    // PORT: the end of the base loop body of `resolve_object_type_members`,
    // which keeps its own copy; keep the two in step.
    fn append_inherited_signatures_and_index_infos(
        &mut self,
        call_signatures: SharedList<SignatureId>,
        construct_signatures: SharedList<SignatureId>,
        index_infos: SharedList<IndexInfoId>,
        base_type: TypeId,
    ) -> (
        SharedList<SignatureId>,
        SharedList<SignatureId>,
        SharedList<IndexInfoId>,
    ) {
        let call_signatures = concat_lazy(
            call_signatures,
            self.get_signatures_of_type(base_type, SignatureKind::CALL),
        );
        let construct_signatures = concat_lazy(
            construct_signatures,
            self.get_signatures_of_type(base_type, SignatureKind::CONSTRUCT),
        );
        let inherited_index_infos: SharedList<IndexInfoId> = if base_type != self.any_type {
            self.get_index_infos_of_type(base_type)
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
        let index_infos = concat_lazy(index_infos, SharedList::from(filtered));
        (call_signatures, construct_signatures, index_infos)
    }

    // PERF (lazymem1 round 2): copies of checker functions that LLVM inlines
    // at every call site (their only callers are on the member resolution
    // path). A call from here kept a separate copy, and in the PGO + BOLT
    // release build LLVM then did not inline `add_inherited_members` and
    // `is_thisless` in `resolve_structured_type_members_slow`, and changed
    // the inlining around them. Keep each copy in step with its original.

    /// Copy of `resolve_declared_members` (checker_p22.rs).
    fn resolve_declared_members_lazy(&mut self, t: TypeId) -> &InterfaceType {
        if !self.ty(t).as_interface_type().declared_members_resolved {
            let symbol = self.ty(t).symbol;
            let members = self.get_members_of_symbol(symbol);
            {
                let d = self.ty_mut(t).as_interface_type_mut();
                d.declared_members_resolved = true;
                d.declared_members = members;
            }
            let call_symbol = self.symbols.get(members, INTERNAL_SYMBOL_NAME_CALL);
            let call_signatures = self.get_signatures_of_symbol(call_symbol);
            self.ty_mut(t)
                .as_interface_type_mut()
                .declared_call_signatures = call_signatures.into();
            let new_symbol = self.symbols.get(members, INTERNAL_SYMBOL_NAME_NEW);
            let construct_signatures = self.get_signatures_of_symbol(new_symbol);
            self.ty_mut(t)
                .as_interface_type_mut()
                .declared_construct_signatures = construct_signatures.into();
            let index_infos = self.get_index_infos_of_symbol(symbol);
            self.ty_mut(t).as_interface_type_mut().declared_index_infos = index_infos.into();
        }
        self.ty(t).as_interface_type()
    }

    /// Copy of `add_inherited_members` (checker_p22.rs).
    fn add_inherited_members_lazy(
        &mut self,
        symbols: SymbolTable,
        base_symbols: &[SymbolId],
    ) -> SymbolTable {
        let mut symbols = symbols;
        self.symbols.reserve(symbols, base_symbols.len());
        for &base in base_symbols {
            if !self.is_static_private_identifier_property_lazy(base) {
                let base_name = self.sym(base).name.clone();
                if symbols.is_nil() {
                    symbols = self.symbols.new_table_with_capacity(base_symbols.len());
                }
                self.symbols
                    .set_if_absent_or(symbols, &base_name, base, |s| {
                        !s.flags.intersects(SymbolFlags::VALUE)
                    });
            }
        }
        symbols
    }

    /// Copy of `is_static_private_identifier_property` (utilities_p1.rs).
    fn is_static_private_identifier_property_lazy(&self, s: SymbolId) -> bool {
        let sym = self.sym(s);
        if !sym.name.is_internal() && !sym.name.is_default_symbol_name() {
            return false;
        }
        let value_declaration = sym.value_declaration;
        value_declaration.is_some()
            && is_private_identifier_class_element_declaration(value_declaration)
            && is_static(value_declaration)
    }

    /// Copy of `is_thisless` (checker_p23.rs).
    fn is_thisless_lazy(&self, symbol: SymbolId) -> bool {
        let declarations = &self.sym(symbol).declarations;
        if declarations.len() == 1 {
            let declaration = declarations[0];
            if declaration.is_some() {
                match declaration.kind() {
                    SyntaxKind::Parameter
                    | SyntaxKind::PropertyDeclaration
                    | SyntaxKind::PropertySignature => {
                        return is_thisless_variable_like_declaration_lazy(declaration);
                    }
                    SyntaxKind::MethodDeclaration
                    | SyntaxKind::MethodSignature
                    | SyntaxKind::Constructor
                    | SyntaxKind::GetAccessor
                    | SyntaxKind::SetAccessor => {
                        return is_thisless_function_like_declaration_lazy(declaration);
                    }
                    _ => {}
                }
            }
        }
        false
    }

    /// Copy of `instantiate_shared_list` (checker_p21.rs).
    fn instantiate_shared_list_lazy<T: Copy + Default + PartialEq>(
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

    /// Copy of `augment_lookups_miss` (checker_p21.rs) for a type whose
    /// members may not be resolved: the signature counts come through the
    /// accessors (Go `len(getSignaturesOfStructuredType(t, kind))` with
    /// #64475, checker.go:19262-19268).
    fn augment_lookups_miss_lazy(&mut self, t: TypeId, name: &Name) -> bool {
        let bits = augment_filter_bits(name);
        if let Some(all) = self.augment_filters.all {
            return !augment_filter_has(&all, bits);
        }
        let (call_count, construct_count) = self.lazy_signature_counts(t);
        let function_slot = if t == self.any_function_type {
            Some(AUGMENT_FUNCTION)
        } else if call_count != 0 {
            Some(AUGMENT_CALLABLE)
        } else if construct_count != 0 {
            Some(AUGMENT_NEWABLE)
        } else {
            None
        };
        function_slot.is_none_or(|slot| self.augment_filter_rejects(slot, bits))
            && self.augment_filter_rejects(AUGMENT_OBJECT, bits)
    }

    // The 4 shape queries of #64475. Their callers run these bodies when
    // the switch is on, for every type (step 2 check, plan change P1), and
    // keep their own bodies when it is off.

    // Go: isWeakType (relater.go:679 with #64475), the object type case
    #[cold]
    #[inline(never)]
    pub(crate) fn is_weak_object_type_lazy(&mut self, t: TypeId) -> bool {
        self.get_signatures_of_structured_type(t, SignatureKind::CALL)
            .is_empty()
            && self
                .get_signatures_of_structured_type(t, SignatureKind::CONSTRUCT)
                .is_empty()
            && self.get_index_infos_of_structured_type(t).is_empty()
            && self.has_properties_of_structured_type(t)
            && self
                .every_property_of_structured_type(t, |p| p.flags.intersects(SymbolFlags::OPTIONAL))
    }

    // Go: getSingleSignature (checker.go:19690 with #64475), the object
    // type case
    #[cold]
    #[inline(never)]
    pub(crate) fn get_single_signature_lazy(
        &mut self,
        t: TypeId,
        kind: SignatureKind,
        allow_members: bool,
    ) -> SignatureId {
        if allow_members
            || !self.has_properties_of_structured_type(t)
                && self.get_index_infos_of_structured_type(t).is_empty()
        {
            let call_signatures = self.get_signatures_of_structured_type(t, SignatureKind::CALL);
            let construct_signatures =
                self.get_signatures_of_structured_type(t, SignatureKind::CONSTRUCT);
            if kind == SignatureKind::CALL
                && call_signatures.len() == 1
                && construct_signatures.is_empty()
            {
                return call_signatures[0];
            }
            if kind == SignatureKind::CONSTRUCT
                && construct_signatures.len() == 1
                && call_signatures.is_empty()
            {
                return construct_signatures[0];
            }
        }
        SignatureId::NIL
    }

    // Go: isFunctionObjectType (checker.go:31624 with #64475), after the
    // evolving array test
    #[cold]
    #[inline(never)]
    pub(crate) fn is_function_object_type_lazy(&mut self, t: TypeId) -> bool {
        // We do a quick check for a "bind" property before performing the more expensive subtype
        // check. This gives us a quicker out in the common case where an object type is not a function.
        if !self
            .get_signatures_of_structured_type(t, SignatureKind::CALL)
            .is_empty()
            || !self
                .get_signatures_of_structured_type(t, SignatureKind::CONSTRUCT)
                .is_empty()
        {
            return true;
        }
        static BIND: std::sync::LazyLock<Name> = std::sync::LazyLock::new(|| Name::from("bind"));
        let global_function_type = self.global_function_type;
        self.get_member_of_structured_type(t, &*BIND).is_some()
            && self.is_type_subtype_of(t, global_function_type)
    }

    // Go: isStringIndexSignatureOnlyTypeWorker (checker.go:27830 with
    // #64475), the object type case
    #[cold]
    #[inline(never)]
    pub(crate) fn is_string_index_signature_only_object_type_lazy(&mut self, t: TypeId) -> bool {
        !self.is_generic_mapped_type(t)
            && !self.has_properties_of_structured_type(t)
            && self.get_index_infos_of_type(t).len() == 1
            && {
                let string_type = self.string_type;
                self.get_index_info_of_type(t, string_type).is_some()
            }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The test of microsoft/TypeScript#64475 (head 2aefafb51b54):
    /// `testdata/tests/cases/compiler/instantiatedReferenceLazyMembers.ts`
    /// and its `.errors.txt` baseline, which the PR made on unmodified main.
    const PR_CASE_NAME: &str = "instantiatedReferenceLazyMembers.ts";
    const PR_CASE: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/lazy_members/instantiatedReferenceLazyMembers.ts"
    ));
    const PR_ERRORS: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/lazy_members/instantiatedReferenceLazyMembers.errors.txt"
    ));

    /// Loads `name` with `text` (the PR case options: strict, target
    /// esnext, noEmit), sets `lazy_members` on its checker, checks the file,
    /// and calls `f` with the checker, the diagnostics in the `.errors.txt`
    /// header format and the types of the file's type aliases in source
    /// order. Each call writes its file in a temp dir of its own.
    fn with_checked<R: Send + 'static>(
        name: &'static str,
        text: &str,
        lazy: bool,
        f: impl FnOnce(&mut Checker, String, Vec<TypeId>) -> R + Send + 'static,
    ) -> R {
        static CALLS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let call = CALLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("ts_goport_lazymem_{}_{call}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(name), text).unwrap();
        std::fs::write(
            dir.join("tsconfig.json"),
            format!(
                r#"{{ "compilerOptions": {{ "strict": true, "target": "esnext", "noEmit": true }}, "files": [{name:?}] }}"#
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
            .find(|file| file.info.file_name.ends_with(&format!("/{name}")))
            .expect("the test file is not in the program")
            .root;
        let result = crate::program::with_type_checker_for_file(file, move |checker| {
            checker.lazy_members = lazy;
            let ctx = crate::gostd::context::background();
            let mut errors = String::new();
            for diagnostic in checker.get_diagnostics_exported(&ctx, file) {
                let text = crate::program::format_diagnostic(&diagnostic);
                // Keep the path from the file name on, as the baseline has it.
                for line in text.lines() {
                    errors.push_str(line.find(name).map_or(line, |i| &line[i..]));
                    errors.push('\n');
                }
            }
            let types = file
                .statements()
                .iter()
                .filter(|s| s.kind() == SyntaxKind::TypeAliasDeclaration)
                .map(|alias| checker.get_type_from_type_node(alias.type_()))
                .collect();
            f(checker, errors, types)
        });
        drop(scope);
        crate::program::release_program(program);
        result
    }

    /// The PR case as the compiler runner compiles it: the `// @` option
    /// lines and the blank line after them are not in the unit, so the
    /// baseline lines count from the first comment line.
    fn pr_case_unit() -> String {
        PR_CASE
            .lines()
            .skip_while(|line| line.starts_with("// @") || line.trim().is_empty())
            .map(|line| format!("{line}\n"))
            .collect()
    }

    /// The diagnostic lines at the top of a `.errors.txt` baseline.
    fn baseline_header(baseline: &str) -> String {
        let mut header = String::new();
        for line in baseline.lines().map(|line| line.trim_end_matches('\r')) {
            if line.is_empty() {
                break;
            }
            header.push_str(line);
            header.push('\n');
        }
        header
    }

    fn members_resolved(c: &Checker, t: TypeId) -> bool {
        c.ty(t)
            .object_flags
            .intersects(ObjectFlags::MEMBERS_RESOLVED)
    }

    /// The PR case gives Go's diagnostics in both modes. With the switch on,
    /// lazy tables are made and some types end the check with no full table.
    #[test]
    fn pr_case_diagnostics_match_go_in_both_modes() {
        let expected = baseline_header(PR_ERRORS);
        assert_eq!(expected.lines().count(), 13, "PR baseline header");
        for lazy in [false, true] {
            let (errors, tables) =
                with_checked(PR_CASE_NAME, &pr_case_unit(), lazy, |c, errors, _| {
                    (errors, c.lazy_member_tables.len())
                });
            assert_eq!(errors, expected, "diagnostics with lazy_members={lazy}");
            if lazy {
                assert!(tables > 0, "no lazy member table is left after the check");
            } else {
                assert_eq!(tables, 0, "a lazy member table with the switch off");
            }
        }
    }

    const LOOKUPS: &str = r#"
interface Base<T> { value: T; shared: string; }
interface Derived<T> extends Base<T[]> { own: T; shared: "derived"; }
type T0 = Derived<number>;
"#;

    /// With the switch off, a member lookup resolves the full table and
    /// makes no lazy table (the default path).
    #[test]
    fn switch_off_lookup_resolves_full_table() {
        with_checked("a.ts", LOOKUPS, false, |c, _, types| {
            let t = types[0];
            assert!(!members_resolved(c, t));
            let own = c.get_property_of_type(t, "own");
            assert!(own.is_some());
            assert!(members_resolved(c, t), "the lookup resolved the full table");
            assert!(c.lazy_member_tables.is_empty());
        });
    }

    /// With the switch on, a member lookup reads the lazy table, an
    /// inherited member comes from the instantiated base, and the full
    /// table keeps the symbols that lookups made. The full table and its
    /// types equal those of the eager path.
    #[test]
    fn switch_on_lookup_is_lazy_and_full_table_reuses_members() {
        let describe = |c: &mut Checker, t: TypeId| -> Vec<String> {
            c.get_properties_of_type(t)
                .iter()
                .map(|&p| {
                    let ty = c.get_type_of_symbol(p);
                    let name = c.sym(p).name.clone();
                    format!("{name}: {}", c.type_to_string(ty))
                })
                .collect()
        };
        let eager = with_checked("a.ts", LOOKUPS, false, move |c, _, types| {
            describe(c, types[0])
        });
        let lazy = with_checked("a.ts", LOOKUPS, true, move |c, _, types| {
            let t = types[0];
            let own = c.get_property_of_type(t, "own");
            let value = c.get_property_of_type(t, "value");
            assert!(own.is_some() && value.is_some());
            assert!(!members_resolved(c, t), "a lookup resolved the full table");
            assert!(c.lazy_member_tables.contains_key(&t));
            let value_type = c.get_type_of_symbol(value);
            assert_eq!(c.type_to_string(value_type), "number[]");
            let described = describe(c, t);
            assert!(members_resolved(c, t));
            assert!(
                !c.lazy_member_tables.contains_key(&t),
                "the table is dropped"
            );
            let members = c.ty(t).as_structured_type().members;
            assert_eq!(
                c.symbols.get(members, "own"),
                own,
                "the full table reuses the member"
            );
            described
        });
        assert_eq!(lazy, eager);
        assert_eq!(
            lazy,
            ["own: number", "shared: \"derived\"", "value: number[]"]
        );
    }

    /// `A<number>` is resolved inside its own prepare step (the base
    /// `C<U["x"]>` reads `A<number>["x"]`). The first lookup then reads the
    /// members that the prepare step set and does not resolve them again:
    /// Go with #64475 makes as many types and instantiations here as Go at
    /// the pin, and so does the port.
    #[test]
    fn resolve_inside_prepare_is_not_repeated() {
        const NESTED: &str = r#"
interface A<T> extends B<A<T>> { x: T; }
interface B<U extends { x: unknown }> extends C<U["x"]> {}
interface C<V> { v: V; }
declare const a: A<number>;
const ax = a.x;
const av = a.v;
"#;
        let run = |lazy| {
            with_checked("a.ts", NESTED, lazy, |c, errors, _| {
                (errors, c.type_count, c.total_instantiation_count)
            })
        };
        let (eager, lazy) = (run(false), run(true));
        assert_eq!(eager.0.matches("error TS5114").count(), 2, "{}", eager.0);
        assert_eq!(lazy, eager);
    }

    /// A reserved member name takes the full table, as in Go.
    #[test]
    fn switch_on_reserved_name_resolves_full_table() {
        with_checked("a.ts", LOOKUPS, true, |c, _, types| {
            let t = types[0];
            c.get_property_of_type(t, "own");
            assert!(!members_resolved(c, t));
            c.get_property_of_type(t, INTERNAL_SYMBOL_NAME_CALL);
            assert!(
                members_resolved(c, t),
                "a reserved name resolved the full table"
            );
            assert!(!c.lazy_member_tables.contains_key(&t));
        });
    }
}
