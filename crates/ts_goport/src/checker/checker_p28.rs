//! Port of typescript-go `checker/checker.go` lines 24957-25911 (unit checker-28):
//! type constructors, literal types, widening helpers, `mapType` and union
//! type construction and reduction.

use crate::checker::relater_p3::TemplateLiteralEnds;
use crate::jsnum::{Number, PseudoBigInt};
use crate::prelude::*;
use rustc_hash::FxBuildHasher;
use smallvec::SmallVec;
use std::borrow::Cow;
use std::hash::{BuildHasher, Hasher};

impl Checker {
    // Go: checker/checker.go:25626 newUniqueESSymbolType
    pub fn new_unique_es_symbol_type(&mut self, symbol: SymbolId, name: &str) -> TypeId {
        let name = name.to_string();
        let t = self.new_type_with(TypeFlags::UNIQUE_ES_SYMBOL, ObjectFlags::NONE, move || {
            TypeData::UniqueESSymbol(UniqueESSymbolType { name })
        });
        self.ty_mut(t).symbol = symbol;
        t
    }

    // Go: checker/checker.go:25634 newObjectType
    pub fn new_object_type(&mut self, object_flags: ObjectFlags, symbol: SymbolId) -> TypeId {
        // PERF: the boxed kinds are allocated here, before `new_type_with`,
        // so its closure makes no call and the `Type` (with the large inline
        // `TypeReference` or `ObjectType`) is written straight into its arena
        // slot. An allocation in the closure kept a 192-byte `memcpy`.
        // `Kind` keeps the Go test order.
        enum Kind {
            Interface(Box<InterfaceType>),
            Tuple(Box<TupleType>),
            Reference,
            Mapped(Box<MappedType>),
            ReverseMapped(Box<ReverseMappedType>),
            EvolvingArray(Box<EvolvingArrayType>),
            InstantiationExpression(Box<InstantiationExpressionType>),
            Anonymous,
        }
        let kind = if object_flags.intersects(ObjectFlags::CLASS_OR_INTERFACE) {
            Kind::Interface(Box::default())
        } else if object_flags.intersects(ObjectFlags::TUPLE) {
            Kind::Tuple(Box::default())
        } else if object_flags.intersects(ObjectFlags::REFERENCE) {
            Kind::Reference
        } else if object_flags.intersects(ObjectFlags::MAPPED) {
            Kind::Mapped(Box::default())
        } else if object_flags.intersects(ObjectFlags::REVERSE_MAPPED) {
            Kind::ReverseMapped(Box::default())
        } else if object_flags.intersects(ObjectFlags::EVOLVING_ARRAY) {
            Kind::EvolvingArray(Box::default())
        } else if object_flags.intersects(ObjectFlags::INSTANTIATION_EXPRESSION_TYPE) {
            Kind::InstantiationExpression(Box::default())
        } else if object_flags.intersects(ObjectFlags::ANONYMOUS) {
            Kind::Anonymous
        } else {
            panic!("Unhandled case in newObjectType")
        };
        let t = self.new_type_with(TypeFlags::OBJECT, object_flags, move || match kind {
            Kind::Interface(d) => TypeData::Interface(d),
            Kind::Tuple(d) => TypeData::Tuple(d),
            Kind::Reference => TypeData::TypeReference(TypeReference::default()),
            Kind::Mapped(d) => TypeData::Mapped(d),
            Kind::ReverseMapped(d) => TypeData::ReverseMapped(d),
            Kind::EvolvingArray(d) => TypeData::EvolvingArray(d),
            Kind::InstantiationExpression(d) => TypeData::InstantiationExpression(d),
            Kind::Anonymous => TypeData::Object(ObjectType::default()),
        });
        self.ty_mut(t).symbol = symbol;
        t
    }

    // Go: checker/checker.go:25670 newAnonymousType
    pub fn new_anonymous_type(
        &mut self,
        symbol: SymbolId,
        members: SymbolTable,
        call_signatures: &[SignatureId],
        construct_signatures: &[SignatureId],
        index_infos: &[IndexInfoId],
    ) -> TypeId {
        let t = self.new_object_type(ObjectFlags::ANONYMOUS, symbol);
        self.set_structured_type_members(
            t,
            members,
            call_signatures,
            construct_signatures,
            index_infos,
        );
        t
    }

    // Go: checker/checker.go:25676 tryCreateTypeReference
    pub fn try_create_type_reference(
        &mut self,
        target: TypeId,
        type_arguments: &[TypeId],
    ) -> TypeId {
        if !type_arguments.is_empty() && target == self.empty_generic_type {
            return self.unknown_type;
        }
        self.create_type_reference(target, type_arguments)
    }

    // Go: checker/checker.go:25683 createTypeReference
    pub fn create_type_reference(&mut self, target: TypeId, type_arguments: &[TypeId]) -> TypeId {
        self.create_type_reference_ex(target, type_arguments, ObjectFlags::NONE)
    }

    // Go: checker/checker.go:25687 createTypeReferenceEx
    pub fn create_type_reference_ex(
        &mut self,
        target: TypeId,
        type_arguments: &[TypeId],
        object_flags: ObjectFlags,
    ) -> TypeId {
        let id = self.get_type_list_key(type_arguments);
        if let Some(instantiations) = &self.ty(target).as_interface_type().instantiations {
            if let Some(&t) = instantiations.get(&id) {
                return t;
            }
        }
        let propagating = self.get_propagating_flags_of_types(type_arguments, TypeFlags::NONE);
        let target_symbol = self.ty(target).symbol;
        let t = self.new_object_type(
            ObjectFlags::REFERENCE | object_flags | propagating,
            target_symbol,
        );
        {
            let d = self.ty_mut(t).as_type_reference_mut();
            d.object.target = target;
            d.resolved_type_arguments = type_arguments.into();
        }
        // PORT: Go writes into the interface's instantiations map, which
        // panics when the map is nil. The same happens here.
        self.ty_mut(target)
            .as_interface_type_mut()
            .instantiations
            .as_mut()
            .expect("assignment to entry in nil map")
            .insert(id, t);
        t
    }

    // Go: checker/checker.go:25701 createDeferredTypeReference
    pub fn create_deferred_type_reference(
        &mut self,
        target: TypeId,
        node: Node,
        mapper: MapperId,
        alias: Option<Rc<TypeAlias>>,
    ) -> TypeId {
        let mut alias = alias;
        if alias.is_none() {
            alias = self.get_alias_for_type_node(node);
            if let Some(a) = &alias {
                if mapper.is_some() {
                    // PORT: Go mutates the freshly created alias in place. The
                    // `Rc` is not shared yet, so a rebuilt alias is equivalent.
                    let symbol = a.symbol;
                    let type_arguments = a.type_arguments.clone();
                    let instantiated = self.instantiate_types(&type_arguments, mapper);
                    alias = Some(Rc::new(TypeAlias {
                        symbol,
                        type_arguments: instantiated,
                    }));
                }
            }
        }
        let target_symbol = self.ty(target).symbol;
        let t = self.new_object_type(ObjectFlags::REFERENCE, target_symbol);
        self.ty_mut(t).alias = alias;
        let d = self.ty_mut(t).as_type_reference_mut();
        d.object.target = target;
        d.object.mapper = mapper;
        d.node = node;
        t
    }

    // Go: checker/checker.go:25717 cloneTypeReference
    pub fn clone_type_reference(&mut self, source: TypeId) -> TypeId {
        let source_symbol = self.ty(source).symbol;
        let t = self.new_object_type(ObjectFlags::REFERENCE, source_symbol);
        let source_object_flags = self.ty(source).object_flags;
        let source_target = self.ty(source).as_type_reference().object.target;
        let source_args = self
            .ty(source)
            .as_type_reference()
            .resolved_type_arguments
            .clone();
        self.ty_mut(t).object_flags =
            source_object_flags.without(ObjectFlags::MEMBERS_RESOLVED | ObjectFlags::WEAK_BITS);
        self.ty_mut(t).as_type_reference_mut().object.target = source_target;
        self.ty_mut(t)
            .as_type_reference_mut()
            .resolved_type_arguments = source_args;
        t
    }

    // Go: checker/checker.go:25725 setStructuredTypeMembers
    pub fn set_structured_type_members(
        &mut self,
        t: TypeId,
        members: SymbolTable,
        call_signatures: &[SignatureId],
        construct_signatures: &[SignatureId],
        index_infos: &[IndexInfoId],
    ) {
        let signatures = if construct_signatures.is_empty() {
            call_signatures.into()
        } else if call_signatures.is_empty() {
            construct_signatures.into()
        } else {
            let mut signatures = call_signatures.to_vec();
            signatures.extend_from_slice(construct_signatures);
            signatures.into()
        };
        self.set_structured_type_members_ex(
            t,
            members,
            SymbolTable::NIL,
            signatures,
            call_signatures.len(),
            index_infos.into(),
        );
    }

    /// `set_structured_type_members` with the final lists: `signatures` is
    /// the call signatures and then the construct signatures, and the first
    /// `call_signature_count` of them are the call signatures. The lists are
    /// stored without a copy. When `declared` is not nil, `members` is
    /// `instantiate_symbol_table(declared, ..)` and its properties come from
    /// `get_named_members_of_instantiation` (the same list).
    ///
    /// PERF: `resolve_object_type_members` passes the declared lists of a
    /// type without instantiation, so the type shares them, as Go shares
    /// its slices.
    pub(crate) fn set_structured_type_members_ex(
        &mut self,
        t: TypeId,
        members: SymbolTable,
        declared: SymbolTable,
        signatures: SharedList<SignatureId>,
        call_signature_count: usize,
        index_infos: SharedList<IndexInfoId>,
    ) {
        self.ty_mut(t).object_flags |= ObjectFlags::MEMBERS_RESOLVED;
        self.ty_mut(t).as_structured_type_mut().members = members;
        let t_symbol = self.ty(t).symbol;
        // `get_named_members` returns the final list (one exact-size
        // allocation), so it is stored without a copy.
        let properties = if declared.is_some() {
            self.get_named_members_of_instantiation(members, declared, t_symbol)
        } else {
            self.get_named_members(members, t_symbol)
        };
        let ty = self.ty_mut(t);
        let data = ty.as_structured_type_mut();
        data.properties = properties;
        data.set_signatures(signatures, call_signature_count as i32, index_infos);
        // memberflags1: after the last write. `get_named_members` above can
        // resolve aliases and run relations, and a weak bit stored then saw
        // the old properties.
        ty.clear_weak_bits();
    }

    // Go: checker/checker.go:25756 newTypeParameter
    pub fn new_type_parameter(&mut self, symbol: SymbolId) -> TypeId {
        let t = self.new_type_with(TypeFlags::TYPE_PARAMETER, ObjectFlags::NONE, || {
            TypeData::TypeParameter(TypeParameter::default())
        });
        self.ty_mut(t).symbol = symbol;
        t
    }

    // This function is used to propagate certain flags when creating new object type references and union types.
    // It is only necessary to do so if a constituent type might be the undefined type, the null type, the type
    // of an object literal or a non-inferrable type. This is because there are operations in the type checker
    // that care about the presence of such types at arbitrary depth in a containing type.
    // Go: checker/checker.go:25766 getPropagatingFlagsOfTypes
    pub fn get_propagating_flags_of_types(
        &self,
        types: &[TypeId],
        exclude_kinds: TypeFlags,
    ) -> ObjectFlags {
        let mut result = ObjectFlags::NONE;
        for &t in types {
            let ty = self.ty(t);
            if !ty.flags.intersects(exclude_kinds) {
                result |= ty.object_flags;
            }
        }
        result & ObjectFlags::PROPAGATING_FLAGS
    }

    /// The member mask of a new union or intersection with these members
    /// (`Type::member_mask`). Not in Go (memberflags1). It reads only flags
    /// and masks of types that exist, so it makes no type and reports
    /// nothing.
    fn member_mask_of(&self, types: &[TypeId]) -> TypeFlags {
        let mut mask = TypeFlags::NONE;
        for &m in types {
            let ty = self.ty(m);
            mask |= ty.flags;
            if let Some(nested) = ty.member_mask() {
                mask |= nested;
            }
        }
        mask
    }

    // PERF: the data is made in its arena or heap slot (`new_with`) and
    // `types` is written there, so the 216-byte `UnionType` is not copied.
    // The `new_type_with` closure only moves the `ArenaBox` and makes no call.
    // Go: checker/checker.go:25776 newUnionType
    pub fn new_union_type(&mut self, object_flags: ObjectFlags, types: &[TypeId]) -> TypeId {
        let mask = self.member_mask_of(types);
        let types = SharedList::from(types);
        let mut data = ArenaBox::new_with(UnionType::default);
        data.union_or_intersection.types = types;
        self.new_type_with(TypeFlags::UNION, object_flags, move || {
            TypeData::Union(data, mask)
        })
    }

    // PERF: built in place, as in `new_union_type`.
    // Go: checker/checker.go:25782 newIntersectionType
    pub fn new_intersection_type(&mut self, object_flags: ObjectFlags, types: &[TypeId]) -> TypeId {
        let mask = self.member_mask_of(types);
        let types = SharedList::from(types);
        let mut data = ArenaBox::new_with(IntersectionType::default);
        data.union_or_intersection.types = types;
        self.new_type_with(TypeFlags::INTERSECTION, object_flags, move || {
            TypeData::Intersection(data, mask)
        })
    }

    // Go: checker/checker.go:25788 newIndexedAccessType
    pub fn new_indexed_access_type(
        &mut self,
        object_type: TypeId,
        index_type: TypeId,
        access_flags: AccessFlags,
    ) -> TypeId {
        self.new_type_with(TypeFlags::INDEXED_ACCESS, ObjectFlags::NONE, || {
            let mut data = IndexedAccessType::default();
            data.object_type = object_type;
            data.index_type = index_type;
            data.access_flags = access_flags;
            TypeData::IndexedAccess(data)
        })
    }

    // Go: checker/checker.go:25796 newIndexType
    pub fn new_index_type(&mut self, target: TypeId, index_flags: IndexFlags) -> TypeId {
        self.new_type_with(TypeFlags::INDEX, ObjectFlags::NONE, || {
            let mut data = IndexType::default();
            data.target = target;
            data.index_flags = index_flags;
            TypeData::Index(data)
        })
    }

    // Go: checker/checker.go:25803 newTemplateLiteralType
    pub fn new_template_literal_type(&mut self, texts: &[String], types: &[TypeId]) -> TypeId {
        // PERF: the lists are made first, so the `new_type_with` closure
        // makes no call.
        let texts: Rc<[String]> = texts.into();
        let types = SharedList::from(types);
        let go_plain = !texts
            .iter()
            .any(|text| crate::scanner_util::contains_go_string_marker(text));
        self.new_type_with(TypeFlags::TEMPLATE_LITERAL, ObjectFlags::NONE, move || {
            TypeData::TemplateLiteral(TemplateLiteralType {
                constrained: ConstrainedType::default(),
                texts,
                types,
                go_plain,
            })
        })
    }

    // Go: checker/checker.go:25810 newStringMappingType
    pub fn new_string_mapping_type(&mut self, symbol: SymbolId, target: TypeId) -> TypeId {
        let t = self.new_type_with(TypeFlags::STRING_MAPPING, ObjectFlags::NONE, || {
            let mut data = StringMappingType::default();
            data.target = target;
            TypeData::StringMapping(data)
        });
        self.ty_mut(t).symbol = symbol;
        t
    }

    // Go: checker/checker.go:25818 newConditionalType
    pub fn new_conditional_type(
        &mut self,
        root: Rc<RefCell<ConditionalRoot>>,
        mapper: MapperId,
        combined_mapper: MapperId,
    ) -> TypeId {
        let (root_check_type, root_extends_type) = {
            let r = root.borrow();
            (r.check_type, r.extends_type)
        };
        let check_type = self.instantiate_type(root_check_type, mapper);
        let extends_type = self.instantiate_type(root_extends_type, mapper);
        // PERF: every field is written here. `ConditionalType::default()`
        // made a default `root` (an `Rc` allocation) only to drop it, and the
        // data was copied into the slot.
        self.new_type_with(TypeFlags::CONDITIONAL, ObjectFlags::NONE, move || {
            TypeData::Conditional(ConditionalType {
                constrained: ConstrainedType::default(),
                root,
                check_type,
                extends_type,
                resolved_true_type: TypeId::NIL,
                resolved_false_type: TypeId::NIL,
                resolved_inferred_true_type: TypeId::NIL,
                resolved_default_constraint: TypeId::NIL,
                resolved_constraint_of_distributive: TypeId::NIL,
                mapper,
                combined_mapper,
            })
        })
    }

    // Go: checker/checker.go:25828 newSubstitutionType
    pub fn new_substitution_type(&mut self, base_type: TypeId, constraint: TypeId) -> TypeId {
        self.new_type_with(TypeFlags::SUBSTITUTION, ObjectFlags::NONE, || {
            let mut data = SubstitutionType::default();
            data.base_type = base_type;
            data.constraint = constraint;
            TypeData::Substitution(data)
        })
    }

    // Go: checker/checker.go:25835 newSignature
    // PORT: Go allocates from `signatureArena` and numbers signatures with
    // `SignatureCount`. Here the arena index is the id; both counters stay equal
    // because the arena has a dummy entry at index 0.
    pub fn new_signature(
        &mut self,
        flags: SignatureFlags,
        declaration: Node,
        type_parameters: &[TypeId],
        this_parameter: SymbolId,
        parameters: &[SymbolId],
        resolved_return_type: TypeId,
        resolved_type_predicate: TypePredicateId,
        min_argument_count: i32,
    ) -> SignatureId {
        self.new_signature_owned(
            flags,
            declaration,
            type_parameters.to_vec(),
            this_parameter,
            parameters.to_vec(),
            resolved_return_type,
            resolved_type_predicate,
            min_argument_count,
        )
    }

    /// `new_signature` for a caller that owns the lists, so the signature
    /// takes them without a copy.
    pub fn new_signature_owned(
        &mut self,
        flags: SignatureFlags,
        declaration: Node,
        type_parameters: Vec<TypeId>,
        this_parameter: SymbolId,
        parameters: Vec<SymbolId>,
        resolved_return_type: TypeId,
        resolved_type_predicate: TypePredicateId,
        min_argument_count: i32,
    ) -> SignatureId {
        self.signature_count += 1;
        let id = SignatureId(u32::try_from(self.signatures.len()).expect("signature overflow"));
        debug_assert_eq!(id.0, self.signature_count);
        self.signatures.push(Signature {
            id,
            flags,
            declaration,
            type_parameters,
            parameters,
            this_parameter,
            resolved_return_type,
            resolved_type_predicate,
            min_argument_count,
            resolved_min_argument_count: -1,
            ..Signature::default()
        });
        id
    }

    // PORT: helpers for Go slice identity of signature type parameter lists
    // (`Signature::type_parameters_origin`). A new origin stands for a newly
    // allocated Go slice.
    pub fn new_type_parameters_origin(&mut self) -> u32 {
        self.type_parameters_origin_count += 1;
        self.type_parameters_origin_count
    }

    // Returns the origin of the type parameter list of `sig`, giving the
    // list one first when Go is about to share its slice with another
    // signature. Empty lists need none (`core.Same` compares lengths first),
    // but an empty list that already has one (a non-nil empty Go slice, see
    // `class_type_parameters_origin`) keeps it.
    pub fn share_type_parameters_origin(&mut self, sig: SignatureId) -> u32 {
        let s = self.sig(sig);
        if s.type_parameters.is_empty() || s.type_parameters_origin != 0 {
            return s.type_parameters_origin;
        }
        let origin = self.new_type_parameters_origin();
        self.sig_mut(sig).type_parameters_origin = origin;
        origin
    }

    // Origin of the Go slice `classType.AsInterfaceType().LocalTypeParameters()`.
    // Go returns nil only when `allTypeParameters` is empty. A class always
    // holds its this-type there, so a non-generic class gets a non-nil empty
    // slice, and the origin marks it as non-nil (read by the language
    // service `getPossibleGenericSignatures`, `TypeParameters() != nil`).
    pub fn class_type_parameters_origin(&mut self, class_type: TypeId) -> u32 {
        if self
            .ty(class_type)
            .as_interface_type()
            .all_type_parameters
            .is_empty()
        {
            return 0;
        }
        if let Some(&origin) = self.class_type_parameters_origins.get(&class_type) {
            return origin;
        }
        let origin = self.new_type_parameters_origin();
        self.class_type_parameters_origins
            .insert(class_type, origin);
        origin
    }

    // Go `core.Same(source.typeParameters, target.typeParameters)`: same
    // length, and either empty or the same backing slice.
    pub fn same_signature_type_parameters(&self, source: SignatureId, target: SignatureId) -> bool {
        let (s, t) = (self.sig(source), self.sig(target));
        s.type_parameters.len() == t.type_parameters.len()
            && (s.type_parameters.is_empty()
                || source == target
                || s.type_parameters_origin != 0
                    && s.type_parameters_origin == t.type_parameters_origin)
    }

    // Go: checker/checker.go:25851 newIndexInfo
    pub fn new_index_info(
        &mut self,
        key_type: TypeId,
        value_type: TypeId,
        is_readonly: bool,
        declaration: Node,
        components: &[Node],
    ) -> IndexInfoId {
        let id = IndexInfoId(u32::try_from(self.index_infos.len()).expect("index info overflow"));
        self.index_infos.push(IndexInfo {
            key_type,
            value_type,
            is_readonly,
            declaration,
            components: components.to_vec(),
            ..IndexInfo::default()
        });
        id
    }

    // Go: checker/checker.go:25861 getRegularTypeOfLiteralType
    pub fn get_regular_type_of_literal_type(&mut self, t: TypeId) -> TypeId {
        let flags = self.ty(t).flags;
        if flags.intersects(TypeFlags::FRESHABLE) {
            return self.ty(t).as_literal_type().regular_type;
        }
        if flags.intersects(TypeFlags::UNION) {
            if self.ty(t).as_union_type().regular_type.is_nil() {
                let regular = self.map_type(t, &mut |c: &mut Checker, t: TypeId| {
                    c.get_regular_type_of_literal_type(t)
                });
                self.ty_mut(t).as_union_type_mut().regular_type = regular;
            }
            return self.ty(t).as_union_type().regular_type;
        }
        t
    }

    // Go: checker/checker.go:25875 getFreshTypeOfLiteralType
    pub fn get_fresh_type_of_literal_type(&mut self, t: TypeId) -> TypeId {
        let flags = self.ty(t).flags;
        if flags.intersects(TypeFlags::FRESHABLE) {
            if self.ty(t).as_literal_type().fresh_type.is_nil() {
                let value = self.ty(t).as_literal_type().value.clone();
                let f = self.new_literal_type(flags, value, t);
                let symbol = self.ty(t).symbol;
                self.ty_mut(f).symbol = symbol;
                self.ty_mut(f).as_literal_type_mut().fresh_type = f;
                self.ty_mut(t).as_literal_type_mut().fresh_type = f;
            }
            return self.ty(t).as_literal_type().fresh_type;
        }
        t
    }

    // Go: checker/checker.go:25889 isFreshLiteralType
    pub fn is_fresh_literal_type(&self, t: TypeId) -> bool {
        let ty = self.ty(t);
        ty.flags.intersects(TypeFlags::FRESHABLE) && ty.as_literal_type().fresh_type == t
    }

    // Go: checker/checker.go:25893 getStringLiteralType
    pub fn get_string_literal_type(&mut self, value: &str) -> TypeId {
        self.get_string_literal_type_cow(Cow::Borrowed(value))
    }

    /// `get_string_literal_type` for an owned value: a new type keeps
    /// `value` without a copy. `get_template_literal_type` moves its joined
    /// text in, as Go `sb.String()` gives the text without a copy.
    pub fn get_string_literal_type_owned(&mut self, value: String) -> TypeId {
        self.get_string_literal_type_cow(Cow::Owned(value))
    }

    // Go: checker/checker.go:25893 getStringLiteralType
    // PORT: perf (tcsplit1). In Go the map key and the type share one
    // string. Here `string_literal_types` keys on the FxHash of the value and
    // a lookup compares the value of each type in the bucket, so the value
    // is stored once, in the type. Types are made at the same calls as Go.
    fn get_string_literal_type_cow(&mut self, value: Cow<'_, str>) -> TypeId {
        let hash = FxBuildHasher.hash_one(&*value);
        if let Some(bucket) = self.string_literal_types.get(&hash) {
            for &t in bucket {
                if self.get_string_literal_value_ref(t) == value {
                    return t;
                }
            }
        }
        let mut value = value.into_owned();
        value.shrink_to_fit();
        let t = self.new_literal_type(
            TypeFlags::STRING_LITERAL,
            Some(LiteralValue::String(value)),
            TypeId::NIL,
        );
        self.string_literal_types.entry(hash).or_default().push(t);
        t
    }

    // Go: checker/checker.go:25902 getNumberLiteralType
    pub fn get_number_literal_type(&mut self, value: Number) -> TypeId {
        // NaN cannot be used as a Go map key because NaN != NaN in IEEE 754,
        // so Go map lookups for NaN always miss. Cache NaN type separately.
        if value.is_nan() {
            if self.nan_type.is_nil() {
                self.nan_type = self.new_literal_type(
                    TypeFlags::NUMBER_LITERAL,
                    Some(LiteralValue::Number(value)),
                    TypeId::NIL,
                );
            }
            return self.nan_type;
        }
        let key = NumberKey::from(value);
        let mut t = self
            .number_literal_types
            .get(&key)
            .copied()
            .unwrap_or_default();
        if t.is_nil() {
            t = self.new_literal_type(
                TypeFlags::NUMBER_LITERAL,
                Some(LiteralValue::Number(value)),
                TypeId::NIL,
            );
            self.number_literal_types.insert(key, t);
        }
        t
    }

    // Go: checker/checker.go:25919 getBigIntLiteralType
    pub fn get_big_int_literal_type(&mut self, value: PseudoBigInt) -> TypeId {
        let key = PseudoBigIntKey::from(&value);
        let mut t = self
            .bigint_literal_types
            .get(&key)
            .copied()
            .unwrap_or_default();
        if t.is_nil() {
            t = self.new_literal_type(
                TypeFlags::BIG_INT_LITERAL,
                Some(LiteralValue::PseudoBigInt(value)),
                TypeId::NIL,
            );
            self.bigint_literal_types.insert(key, t);
        }
        t
    }

    // text is a valid bigint string excluding a trailing `n`, but including a possible prefix `-`.
    // Use `isValidBigIntString(text, roundTripOnly)` before calling this function.
    // Go: checker/checker.go:25930 parseBigIntLiteralType
    pub fn parse_big_int_literal_type(&mut self, text: &str) -> TypeId {
        self.get_big_int_literal_type(PseudoBigInt::parse_valid(text))
    }

    // Go: checker/checker.go:25934 getStringLiteralValue
    pub fn get_string_literal_value(&self, t: TypeId) -> String {
        self.get_string_literal_value_ref(t).to_string()
    }

    // PORT: borrowing form of `getStringLiteralValue`. Callers that only read
    // the value use it to skip the `String` clone.
    // Go: checker/checker.go:25934 getStringLiteralValue
    pub fn get_string_literal_value_ref(&self, t: TypeId) -> &str {
        match self.ty(t).as_literal_type().value.as_ref() {
            Some(LiteralValue::String(s)) => s,
            _ => panic!("interface conversion: interface {{}} is not string"),
        }
    }

    // PORT: perf, no Go function. True when the string literal type `t` has
    // a value with no `scanner_util::GO_STRING_MARKER`, so the value is its own
    // Go bytes and needs no `go_string_bytes`. Computed once per type.
    pub fn string_literal_go_plain(&self, t: TypeId) -> bool {
        let literal = self.ty(t).as_literal_type();
        *literal
            .go_plain
            .get_or_init(|| match literal.value.as_ref() {
                Some(LiteralValue::String(s)) => !crate::scanner_util::contains_go_string_marker(s),
                _ => false,
            })
    }

    // Go: checker/checker.go:25938 getNumberLiteralValue
    pub fn get_number_literal_value(&self, t: TypeId) -> Number {
        match self.ty(t).as_literal_type().value.as_ref() {
            Some(LiteralValue::Number(n)) => *n,
            _ => panic!("interface conversion: interface {{}} is not jsnum.Number"),
        }
    }

    // Go: checker/checker.go:25942 getBigIntLiteralValue
    pub fn get_big_int_literal_value(&self, t: TypeId) -> PseudoBigInt {
        match self.ty(t).as_literal_type().value.as_ref() {
            Some(LiteralValue::PseudoBigInt(v)) => v.clone(),
            _ => panic!("interface conversion: interface {{}} is not jsnum.PseudoBigInt"),
        }
    }

    // Go: checker/checker.go:25946 getBooleanLiteralValue
    pub fn get_boolean_literal_value(&self, t: TypeId) -> bool {
        match self.ty(t).as_literal_type().value.as_ref() {
            Some(LiteralValue::Bool(b)) => *b,
            _ => panic!("interface conversion: interface {{}} is not bool"),
        }
    }

    // Go: checker/checker.go:25950 getEnumLiteralType
    pub fn get_enum_literal_type(
        &mut self,
        value: LiteralValue,
        enum_symbol: SymbolId,
        symbol: SymbolId,
    ) -> TypeId {
        let flags;
        match &value {
            LiteralValue::String(_) => {
                flags = TypeFlags::ENUM_LITERAL | TypeFlags::STRING_LITERAL;
            }
            LiteralValue::Number(v) => {
                flags = TypeFlags::ENUM_LITERAL | TypeFlags::NUMBER_LITERAL;
                // NaN cannot be used as a Go map key because NaN != NaN in IEEE 754,
                // so Go map lookups for NaN always miss. Cache NaN enum types separately by enum symbol.
                if v.is_nan() {
                    let mut t = self
                        .enum_nan_literal_types
                        .get(&enum_symbol)
                        .copied()
                        .unwrap_or_default();
                    if t.is_nil() {
                        t = self.new_literal_type(flags, Some(value.clone()), TypeId::NIL);
                        self.ty_mut(t).symbol = symbol;
                        self.enum_nan_literal_types.insert(enum_symbol, t);
                    }
                    return t;
                }
            }
            _ => panic!("Unhandled case in getEnumLiteralType"),
        }
        let key = EnumLiteralKey {
            enum_symbol,
            value: LiteralValueKey::from(&value),
        };
        let mut t = self
            .enum_literal_types
            .get(&key)
            .copied()
            .unwrap_or_default();
        if t.is_nil() {
            t = self.new_literal_type(flags, Some(value), TypeId::NIL);
            self.ty_mut(t).symbol = symbol;
            self.enum_literal_types.insert(key, t);
        }
        t
    }

    // Go: checker/checker.go:25981 isLiteralType
    pub fn is_literal_type(&self, t: TypeId) -> bool {
        let ty = self.ty(t);
        if ty.flags.intersects(TypeFlags::BOOLEAN) {
            return true;
        }
        if ty.flags.intersects(TypeFlags::UNION) {
            if ty.flags.intersects(TypeFlags::ENUM_LITERAL) {
                return true;
            }
            return ty.types().iter().all(|&u| self.is_unit_type(u));
        }
        self.is_unit_type(t)
    }

    // Go: checker/checker.go:25994 isNeitherUnitTypeNorNever
    pub fn is_neither_unit_type_nor_never(&self, t: TypeId) -> bool {
        !self
            .ty(t)
            .flags
            .intersects(TypeFlags::UNIT | TypeFlags::NEVER)
    }

    // Go: checker/checker.go:25998 isUnitType
    pub fn is_unit_type(&self, t: TypeId) -> bool {
        self.ty(t).flags.intersects(TypeFlags::UNIT)
    }

    // Go: checker/checker.go:26002 isUnitLikeType
    pub fn is_unit_like_type(&mut self, t: TypeId) -> bool {
        // Intersections that reduce to 'never' (e.g. 'T & null' where 'T extends {}') are not unit types.
        let t = self.get_base_constraint_or_type(t);
        // Scan intersections such that tagged literal types are considered unit types.
        if self.ty(t).flags.intersects(TypeFlags::INTERSECTION) {
            return self
                .ty(t)
                .as_intersection_type()
                .union_or_intersection
                .types
                .iter()
                .any(|&u| self.is_unit_type(u));
        }
        self.is_unit_type(t)
    }

    // Go: checker/checker.go:26012 extractUnitType
    pub fn extract_unit_type(&self, t: TypeId) -> TypeId {
        if self.ty(t).flags.intersects(TypeFlags::INTERSECTION) {
            let u = self
                .ty(t)
                .as_intersection_type()
                .union_or_intersection
                .types
                .iter()
                .copied()
                .find(|&u| self.is_unit_type(u))
                .unwrap_or_default();
            if u.is_some() {
                return u;
            }
        }
        t
    }

    // Go: checker/checker.go:26022 getBaseTypeOfLiteralType
    pub fn get_base_type_of_literal_type(&mut self, t: TypeId) -> TypeId {
        let flags = self.ty(t).flags;
        if flags.intersects(TypeFlags::ENUM_LIKE) {
            return self.get_base_type_of_enum_like_type(t);
        } else if flags.intersects(
            TypeFlags::STRING_LITERAL | TypeFlags::TEMPLATE_LITERAL | TypeFlags::STRING_MAPPING,
        ) {
            return self.string_type;
        } else if flags.intersects(TypeFlags::NUMBER_LITERAL) {
            return self.number_type;
        } else if flags.intersects(TypeFlags::BIG_INT_LITERAL) {
            return self.bigint_type;
        } else if flags.intersects(TypeFlags::BOOLEAN_LITERAL) {
            return self.boolean_type;
        } else if flags.intersects(TypeFlags::UNION) {
            return self.get_base_type_of_literal_type_union(t);
        }
        t
    }

    // This like getBaseTypeOfLiteralType, but instead treats enum literals as strings/numbers instead
    // of returning their enum base type (which depends on the types of other literals in the enum).
    // Go: checker/checker.go:26042 getBaseTypeOfLiteralTypeForComparison
    pub fn get_base_type_of_literal_type_for_comparison(&mut self, t: TypeId) -> TypeId {
        let flags = self.ty(t).flags;
        if flags.intersects(
            TypeFlags::STRING_LITERAL | TypeFlags::TEMPLATE_LITERAL | TypeFlags::STRING_MAPPING,
        ) {
            return self.string_type;
        } else if flags.intersects(TypeFlags::NUMBER_LITERAL | TypeFlags::ENUM) {
            return self.number_type;
        } else if flags.intersects(TypeFlags::BIG_INT_LITERAL) {
            return self.bigint_type;
        } else if flags.intersects(TypeFlags::BOOLEAN_LITERAL) {
            return self.boolean_type;
        } else if flags.intersects(TypeFlags::UNION) {
            return self.map_type(t, &mut |c: &mut Checker, t: TypeId| {
                c.get_base_type_of_literal_type_for_comparison(t)
            });
        }
        t
    }

    // Go: checker/checker.go:26058 getBaseTypeOfEnumLikeType
    pub fn get_base_type_of_enum_like_type(&mut self, t: TypeId) -> TypeId {
        let flags = self.ty(t).flags;
        let symbol = self.ty(t).symbol;
        if flags.intersects(TypeFlags::ENUM_LIKE)
            && self.sym(symbol).flags.intersects(SymbolFlags::ENUM_MEMBER)
        {
            let parent = self.get_parent_of_symbol(symbol);
            return self.get_declared_type_of_symbol(parent);
        }
        t
    }

    // Go: checker/checker.go:26065 getBaseTypeOfLiteralTypeUnion
    pub fn get_base_type_of_literal_type_union(&mut self, t: TypeId) -> TypeId {
        let key = CachedTypeKey {
            kind: CachedTypeKind::LITERAL_UNION_BASE_TYPE,
            type_id: t,
        };
        if let Some(&cached) = self.cached_types.get(&key) {
            return cached;
        }
        let result = self.map_type(t, &mut |c: &mut Checker, t: TypeId| {
            c.get_base_type_of_literal_type(t)
        });
        self.cached_types.insert(key, result);
        result
    }

    // Go: checker/checker.go:26075 getWidenedLiteralType
    pub fn get_widened_literal_type(&mut self, t: TypeId) -> TypeId {
        let flags = self.ty(t).flags;
        if flags.intersects(TypeFlags::ENUM_LIKE) && self.is_fresh_literal_type(t) {
            return self.get_base_type_of_enum_like_type(t);
        } else if flags.intersects(TypeFlags::STRING_LITERAL) && self.is_fresh_literal_type(t) {
            return self.string_type;
        } else if flags.intersects(TypeFlags::NUMBER_LITERAL) && self.is_fresh_literal_type(t) {
            return self.number_type;
        } else if flags.intersects(TypeFlags::BIG_INT_LITERAL) && self.is_fresh_literal_type(t) {
            return self.bigint_type;
        } else if flags.intersects(TypeFlags::BOOLEAN_LITERAL) && self.is_fresh_literal_type(t) {
            return self.boolean_type;
        } else if flags.intersects(TypeFlags::UNION) {
            return self.map_type(t, &mut |c: &mut Checker, t: TypeId| {
                c.get_widened_literal_type(t)
            });
        }
        t
    }

    // Go: checker/checker.go:26093 getWidenedUniqueESSymbolType
    pub fn get_widened_unique_es_symbol_type(&mut self, t: TypeId) -> TypeId {
        let flags = self.ty(t).flags;
        if flags.intersects(TypeFlags::UNIQUE_ES_SYMBOL) {
            return self.es_symbol_type;
        } else if flags.intersects(TypeFlags::UNION) {
            return self.map_type(t, &mut |c: &mut Checker, t: TypeId| {
                c.get_widened_unique_es_symbol_type(t)
            });
        }
        t
    }

    // Go: checker/checker.go:26103 getWidenedLiteralLikeTypeForContextualType
    pub fn get_widened_literal_like_type_for_contextual_type(
        &mut self,
        t: TypeId,
        contextual_type: TypeId,
    ) -> TypeId {
        let mut t = t;
        if !self.is_literal_of_contextual_type(t, contextual_type) {
            let widened = self.get_widened_literal_type(t);
            t = self.get_widened_unique_es_symbol_type(widened);
        }
        self.get_regular_type_of_literal_type(t)
    }

    // Go: checker/checker.go:26110 isLiteralOfContextualType
    pub fn is_literal_of_contextual_type(
        &mut self,
        candidate_type: TypeId,
        contextual_type: TypeId,
    ) -> bool {
        if contextual_type.is_some() {
            let contextual_flags = self.ty(contextual_type).flags;
            if contextual_flags.intersects(TypeFlags::UNION_OR_INTERSECTION) {
                return (0..self.ty(contextual_type).types().len()).any(|i| {
                    let t = self.type_at(contextual_type, i);
                    self.is_literal_of_contextual_type(candidate_type, t)
                });
            }
            if contextual_flags.intersects(TypeFlags::INSTANTIABLE_NON_PRIMITIVE) {
                // If the contextual type is a type variable constrained to a primitive type, consider
                // this a literal context for literals of that primitive type. For example, given a
                // type parameter 'T extends string', infer string literal types for T.
                let mut constraint = self.get_base_constraint_of_type(contextual_type);
                if constraint.is_nil() {
                    constraint = self.unknown_type;
                }
                return self.maybe_type_of_kind(constraint, TypeFlags::STRING)
                    && self.maybe_type_of_kind(candidate_type, TypeFlags::STRING_LITERAL)
                    || self.maybe_type_of_kind(constraint, TypeFlags::NUMBER)
                        && self.maybe_type_of_kind(candidate_type, TypeFlags::NUMBER_LITERAL)
                    || self.maybe_type_of_kind(constraint, TypeFlags::BIG_INT)
                        && self.maybe_type_of_kind(candidate_type, TypeFlags::BIG_INT_LITERAL)
                    || self.maybe_type_of_kind(constraint, TypeFlags::ES_SYMBOL)
                        && self.maybe_type_of_kind(candidate_type, TypeFlags::UNIQUE_ES_SYMBOL)
                    || self.is_literal_of_contextual_type(candidate_type, constraint);
            }
            // If the contextual type is a literal of a particular primitive type, we consider this a
            // literal context for all literals of that primitive type.
            return contextual_flags.intersects(
                TypeFlags::STRING_LITERAL
                    | TypeFlags::INDEX
                    | TypeFlags::TEMPLATE_LITERAL
                    | TypeFlags::STRING_MAPPING,
            ) && self.maybe_type_of_kind(candidate_type, TypeFlags::STRING_LITERAL)
                || contextual_flags.intersects(TypeFlags::NUMBER_LITERAL)
                    && self.maybe_type_of_kind(candidate_type, TypeFlags::NUMBER_LITERAL)
                || contextual_flags.intersects(TypeFlags::BIG_INT_LITERAL)
                    && self.maybe_type_of_kind(candidate_type, TypeFlags::BIG_INT_LITERAL)
                || contextual_flags.intersects(TypeFlags::BOOLEAN_LITERAL)
                    && self.maybe_type_of_kind(candidate_type, TypeFlags::BOOLEAN_LITERAL)
                || contextual_flags.intersects(TypeFlags::UNIQUE_ES_SYMBOL)
                    && self.maybe_type_of_kind(candidate_type, TypeFlags::UNIQUE_ES_SYMBOL);
        }
        false
    }

    // Go: checker/checker.go:26142 mapTypeWithAlias
    pub fn map_type_with_alias(
        &mut self,
        t: TypeId,
        f: &mut dyn FnMut(&mut Checker, TypeId) -> TypeId,
        alias: Option<Rc<TypeAlias>>,
    ) -> TypeId {
        if self.ty(t).flags.intersects(TypeFlags::UNION) && alias.is_some() {
            let mapped = self.map_constituents(t, f);
            return self.get_union_type_ex(&mapped, UnionReduction::LITERAL, alias, TypeId::NIL);
        }
        self.map_type(t, f)
    }

    // Go: checker/checker.go:26149 mapType
    pub fn map_type(
        &mut self,
        t: TypeId,
        f: &mut dyn FnMut(&mut Checker, TypeId) -> TypeId,
    ) -> TypeId {
        self.map_type_ex(t, f, false /*noReductions*/)
    }

    // Go: checker/checker.go:26153 mapTypeEx
    pub fn map_type_ex(
        &mut self,
        t: TypeId,
        f: &mut dyn FnMut(&mut Checker, TypeId) -> TypeId,
        no_reductions: bool,
    ) -> TypeId {
        let flags = self.ty(t).flags;
        if flags.intersects(TypeFlags::NEVER) {
            return t;
        }
        if !flags.intersects(TypeFlags::UNION) {
            return f(self, t);
        }
        let origin = self.ty(t).as_union_type().origin;
        let owner = if origin.is_some() && self.ty(origin).flags.intersects(TypeFlags::UNION) {
            origin
        } else {
            t
        };
        // PORT: the constituents are read in place (`type_at`). Until the
        // first changed element every mapped type equals its non-nil source,
        // so the mapped list is built only from the first change, and a
        // mapping that changes nothing costs no copy (Go `changed` is
        // `mapped_types.is_some()`).
        let count = self.ty(owner).types().len();
        let mut mapped_types: Option<Vec<TypeId>> = None;
        for i in 0..count {
            let s = self.type_at(owner, i);
            let mapped = if self.ty(s).flags.intersects(TypeFlags::UNION) {
                self.map_type_ex(s, f, no_reductions)
            } else {
                f(self, s)
            };
            if let Some(list) = &mut mapped_types {
                if mapped.is_some() {
                    list.push(mapped);
                }
            } else if mapped != s {
                let mut list: Vec<TypeId> = Vec::with_capacity(count.max(16));
                list.extend_from_slice(&self.ty(owner).types()[..i]);
                if mapped.is_some() {
                    list.push(mapped);
                }
                mapped_types = Some(list);
            }
        }
        if let Some(mapped_types) = mapped_types {
            if mapped_types.is_empty() {
                return TypeId::NIL;
            }
            let reduction = if no_reductions {
                UnionReduction::NONE
            } else {
                UnionReduction::LITERAL
            };
            return self.get_union_type_ex(
                &mapped_types,
                reduction,
                None,        /*alias*/
                TypeId::NIL, /*origin*/
            );
        }
        t
    }

    // Go: checker/checker.go:26208 getUnionOrIntersectionType
    pub fn get_union_or_intersection_type(
        &mut self,
        types: &[TypeId],
        is_union: bool,
        union_reduction: UnionReduction,
    ) -> TypeId {
        if is_union {
            return self.get_union_type_ex(types, union_reduction, None, TypeId::NIL);
        }
        self.get_intersection_type(types)
    }

    // Go: checker/checker.go:26215 getUnionType
    pub fn get_union_type(&mut self, types: &[TypeId]) -> TypeId {
        self.get_union_type_ex(
            types,
            UnionReduction::LITERAL,
            None,        /*alias*/
            TypeId::NIL, /*origin*/
        )
    }

    // We sort and deduplicate the constituent types based on object identity. If the subtypeReduction
    // flag is specified we also reduce the constituent type set to only include types that aren't subtypes
    // of other types. Subtype reduction is expensive for large union types and is possible only when union
    // types are known not to circularly reference themselves (as is the case with union types created by
    // expression constructs such as array literals and the || and ?: operators). Named types can
    // circularly reference themselves and therefore cannot be subtype reduced during their declaration.
    // For example, "type Item = string | (() => Item" is a named type that circularly references itself.
    // Go: checker/checker.go:26226 getUnionTypeEx
    pub fn get_union_type_ex(
        &mut self,
        types: &[TypeId],
        union_reduction: UnionReduction,
        alias: Option<Rc<TypeAlias>>,
        origin: TypeId,
    ) -> TypeId {
        if types.is_empty() {
            return self.never_type;
        }
        if types.len() == 1 {
            return types[0];
        }
        // We optimize for the common case of unioning a union type with some other type (such as `undefined`).
        if types.len() == 2
            && origin.is_nil()
            && (self.ty(types[0]).flags.intersects(TypeFlags::UNION)
                || self.ty(types[1]).flags.intersects(TypeFlags::UNION))
        {
            let mut id1 = self.ty(types[0]).id;
            let mut id2 = self.ty(types[1]).id;
            if id1 > id2 {
                std::mem::swap(&mut id1, &mut id2);
            }
            let key = UnionOfUnionKey {
                id1,
                id2,
                r: union_reduction,
                a: self.get_alias_key(alias.as_deref()),
            };
            let mut t = self
                .union_of_union_types
                .get(&key)
                .copied()
                .unwrap_or_default();
            if t.is_nil() {
                t = self.get_union_type_worker(
                    types,
                    union_reduction,
                    alias,
                    TypeId::NIL, /*origin*/
                );
                self.union_of_union_types.insert(key, t);
            }
            return t;
        }
        // PERF (unionmemo1, not in Go): the memo of recent calls.
        if origin.is_nil() && union_reduction != UnionReduction::SUBTYPE {
            return self.get_union_type_kept(types, union_reduction, alias);
        }
        self.get_union_type_worker(types, union_reduction, alias, origin)
    }

    /// PERF (unionmemo1, not in Go): `get_union_type_worker` for a call with
    /// no origin and no subtype reduction, through the memo of recent calls
    /// (`UnionCalls`). Go runs the worker again on every call.
    fn get_union_type_kept(
        &mut self,
        types: &[TypeId],
        union_reduction: UnionReduction,
        alias: Option<Rc<TypeAlias>>,
    ) -> TypeId {
        let call = UnionCall {
            reduction: union_reduction,
            alias: alias.as_deref(),
            types,
        };
        let hash = call.hash();
        if let Some(t) = self.union_calls.get(hash, &call) {
            if umv::on() {
                // unionmemo1 verify: run Go's worker anyway; it must give the
                // kept result and make or move nothing.
                let before = self.umv_snapshot();
                let mark = self.union_calls.mark;
                let w =
                    self.get_union_type_worker(types, union_reduction, alias.clone(), TypeId::NIL);
                let after = self.umv_snapshot();
                umv::add(&umv::HITS);
                if w != t || before != after || mark != self.union_calls.mark {
                    if umv::add(&umv::BAD) < 20 {
                        eprintln!(
                            "unionmemo1 verify: BAD hit kept {t:?} worker {w:?} mark moved {} before {before:?} after {after:?}",
                            mark != self.union_calls.mark
                        );
                    }
                }
                return w;
            }
            return t;
        }
        let mark = self.union_calls.mark;
        let t = self.get_union_type_worker(types, union_reduction, alias.clone(), TypeId::NIL);
        umv::add(&umv::MISSES);
        if self.union_calls.mark == mark && t != self.error_type {
            umv::add(&umv::KEPT);
            self.union_calls.put(hash, &call, t);
        } else if self.union_calls.mark != mark {
            umv::add(&umv::UNSAFE);
        } else {
            umv::add(&umv::ERROR);
        }
        t
    }

    /// unionmemo1 verify: the counts that a hit must not change.
    fn umv_snapshot(&self) -> [u64; 10] {
        [
            u64::from(self.type_count),
            u64::from(self.symbol_count),
            u64::from(self.signature_count),
            u64::from(self.instantiation_count),
            self.diagnostics.count as u64,
            self.suggestion_diagnostics.count as u64,
            self.union_types.len() as u64,
            self.union_of_union_types.len() as u64,
            self.intersection_types.len() as u64,
            crate::ast::utilities_p1::peek_next_symbol_id(),
        ]
    }

    // Go: checker/checker.go:26251 getUnionTypeWorker
    pub fn get_union_type_worker(
        &mut self,
        types: &[TypeId],
        union_reduction: UnionReduction,
        alias: Option<Rc<TypeAlias>>,
        origin: TypeId,
    ) -> TypeId {
        let mut origin = origin;
        // PORT: the set is built in a stack `TypeSet` and reduced in place.
        // `get_union_type_from_sorted_list` copies it only when it creates
        // the union.
        let (mut type_set, includes) = self.add_types_to_union(types);
        if union_reduction != UnionReduction::NONE {
            if includes.intersects(TypeFlags::ANY_OR_UNKNOWN) {
                if includes.intersects(TypeFlags::ANY) {
                    if includes.intersects(TypeFlags::INCLUDES_WILDCARD) {
                        return self.wildcard_type;
                    } else if includes.intersects(TypeFlags::INCLUDES_ERROR) {
                        return self.error_type;
                    }
                    return self.any_type;
                }
                return self.unknown_type;
            }
            if includes.intersects(TypeFlags::UNDEFINED) {
                // If type set contains both undefinedType and missingType, remove missingType
                if type_set.len() >= 2
                    && type_set[0] == self.undefined_type
                    && type_set[1] == self.missing_type
                {
                    type_set.remove(1);
                }
            }
            if includes.intersects(
                TypeFlags::ENUM
                    | TypeFlags::LITERAL
                    | TypeFlags::UNIQUE_ES_SYMBOL
                    | TypeFlags::TEMPLATE_LITERAL
                    | TypeFlags::STRING_MAPPING,
            ) || includes.intersects(TypeFlags::VOID)
                && includes.intersects(TypeFlags::UNDEFINED)
            {
                self.remove_redundant_literal_types(
                    &mut type_set,
                    includes,
                    (union_reduction.0 & UnionReduction::SUBTYPE.0) != 0,
                );
            }
            if includes.intersects(TypeFlags::STRING_LITERAL)
                && includes.intersects(TypeFlags::TEMPLATE_LITERAL | TypeFlags::STRING_MAPPING)
            {
                self.union_calls.mark += 1;
                self.remove_string_literals_matched_by_template_literals(&mut type_set);
            }
            if includes.intersects(TypeFlags::INCLUDES_CONSTRAINED_TYPE_VARIABLE) {
                self.union_calls.mark += 1;
                self.remove_constrained_type_variables(&mut type_set);
            }
            if union_reduction == UnionReduction::SUBTYPE
                && !self.remove_subtypes(&mut type_set, includes.intersects(TypeFlags::OBJECT))
            {
                return self.error_type;
            }
            if type_set.is_empty() {
                if includes.intersects(TypeFlags::NULL) {
                    if includes.intersects(TypeFlags::INCLUDES_NON_WIDENING_TYPE) {
                        return self.null_type;
                    }
                    return self.null_widening_type;
                } else if includes.intersects(TypeFlags::UNDEFINED) {
                    if includes.intersects(TypeFlags::INCLUDES_NON_WIDENING_TYPE) {
                        return self.undefined_type;
                    }
                    return self.undefined_widening_type;
                }
                return self.never_type;
            }
        }
        if origin.is_nil() && includes.intersects(TypeFlags::UNION) {
            let named_unions = self.add_named_unions(Vec::new(), types);
            let mut reduced_types: Vec<TypeId> = Vec::new();
            // Go: checker/checker.go:26311 `containsType(u.Types(), t)` for
            // each `t` and named union `u`, until one contains `t`.
            // PERF: unionsort1. The keys of each named union's types are read
            // once, when it is first searched, and the key of each `t` once.
            // The keyed searches make Go's comparisons (`search_keyed_types`).
            // Before, these searches made most of the union comparisons.
            let mut named_keys: Vec<Vec<(u128, TypeId)>> = vec![Vec::new(); named_unions.len()];
            for &t in &type_set {
                let key = self.union_sort_key(t);
                let mut in_named = false;
                for (i, &u) in named_unions.iter().enumerate() {
                    if named_keys[i].is_empty() {
                        named_keys[i] = self.union_sort_keys(self.ty(u).types());
                    }
                    if self.search_keyed_types(&named_keys[i], key, t).1 {
                        in_named = true;
                        break;
                    }
                }
                if !in_named {
                    reduced_types.push(t);
                }
            }
            if alias.is_none() && named_unions.len() == 1 && reduced_types.is_empty() {
                return named_unions[0];
            }
            // We create a denormalized origin type only when the union was created from one or more named unions
            // (unions with alias symbols or origins) and when there is no overlap between those named unions.
            let mut named_types_count = 0;
            for &u in &named_unions {
                named_types_count += self.ty(u).types().len();
            }
            if named_types_count + reduced_types.len() == type_set.len() {
                for &t in &named_unions {
                    let (inserted, _) = self.insert_type(&reduced_types, t);
                    reduced_types = inserted;
                }
                self.union_calls.mark += 1;
                origin = self.new_union_type(ObjectFlags::NONE, &reduced_types);
            }
        }
        let object_flags = (if includes.intersects(TypeFlags::NOT_PRIMITIVE_UNION) {
            ObjectFlags::NONE
        } else {
            ObjectFlags::PRIMITIVE_UNION
        }) | (if includes.intersects(TypeFlags::INTERSECTION) {
            ObjectFlags::CONTAINS_INTERSECTIONS
        } else {
            ObjectFlags::NONE
        });
        self.get_union_type_from_sorted_list(&type_set, object_flags, alias, origin)
    }

    // This function assumes the constituent type list is sorted and deduplicated.
    // Go: checker/checker.go:26337 getUnionTypeFromSortedList
    pub fn get_union_type_from_sorted_list(
        &mut self,
        types: &[TypeId],
        precomputed_object_flags: ObjectFlags,
        alias: Option<Rc<TypeAlias>>,
        origin: TypeId,
    ) -> TypeId {
        if types.is_empty() {
            return self.never_type;
        }
        if types.len() == 1 {
            return types[0];
        }
        let key = self.get_union_key(types, origin, alias.as_deref());
        let mut t = self.union_types.get(&key).copied().unwrap_or_default();
        if t.is_nil() {
            let propagating = self.get_propagating_flags_of_types(types, TypeFlags::NULLABLE);
            t = self.new_union_type(precomputed_object_flags | propagating, types);
            self.ty_mut(t).as_union_type_mut().origin = origin;
            self.ty_mut(t).alias = alias;
            if types.len() == 2
                && self
                    .ty(types[0])
                    .flags
                    .intersects(TypeFlags::BOOLEAN_LITERAL)
                && self
                    .ty(types[1])
                    .flags
                    .intersects(TypeFlags::BOOLEAN_LITERAL)
            {
                self.ty_mut(t).flags |= TypeFlags::BOOLEAN;
            }
            self.union_types.insert(key, t);
        }
        t
    }

    // Go: checker/checker.go:26358 UnionTypes
    // PORT: Go returns `iter.Seq[*Type]` over the map values; this returns
    // them as a `Vec` (map order, like Go).
    pub fn union_types(&self) -> Vec<TypeId> {
        self.union_types.values().copied().collect()
    }

    // Go: checker/checker.go:26362 addTypesToUnion
    pub fn add_types_to_union(&mut self, source_types: &[TypeId]) -> (TypeSet, TypeFlags) {
        let mut types = TypeSet::with_capacity(source_types.len());
        let mut includes = TypeFlags::NONE;
        // PORT: Go closure `addType`; it reads the checker and grows `types` and `includes`.
        fn add_type(c: &Checker, types: &mut TypeSet, includes: &mut TypeFlags, t: TypeId) {
            let flags = c.ty(t).flags;
            let object_flags = c.ty(t).object_flags;
            // We ignore 'never' types in unions
            if flags.intersects(TypeFlags::NEVER) {
                return;
            }
            *includes |= flags & TypeFlags::INCLUDES_MASK;
            if flags.intersects(TypeFlags::INSTANTIABLE) {
                *includes |= TypeFlags::INCLUDES_INSTANTIABLE;
            }
            if flags.intersects(TypeFlags::INTERSECTION)
                && object_flags.intersects(ObjectFlags::IS_CONSTRAINED_TYPE_VARIABLE)
            {
                *includes |= TypeFlags::INCLUDES_CONSTRAINED_TYPE_VARIABLE;
            }
            if t == c.wildcard_type {
                *includes |= TypeFlags::INCLUDES_WILDCARD;
            }
            if c.is_error_type(t) {
                *includes |= TypeFlags::INCLUDES_ERROR;
            }
            if !c.strict_null_checks && flags.intersects(TypeFlags::NULLABLE) {
                if !object_flags.intersects(ObjectFlags::CONTAINS_WIDENING_TYPE) {
                    *includes |= TypeFlags::INCLUDES_NON_WIDENING_TYPE;
                }
                return;
            }
            types.push(t);
        }
        let mut last_type = TypeId::NIL;
        for &t in source_types {
            if t != last_type {
                if self.ty(t).flags.intersects(TypeFlags::UNION) {
                    let ty = self.ty(t);
                    let u = ty.as_union_type();
                    if ty.alias.is_some() || u.origin.is_some() {
                        includes |= TypeFlags::UNION;
                    }
                    for &s in u.union_or_intersection.types.iter() {
                        add_type(self, &mut types, &mut includes, s);
                    }
                } else {
                    add_type(self, &mut types, &mut includes, t);
                }
                last_type = t;
            }
        }
        if types.len() >= 2 {
            // Sort and deduplicate types
            // Go: checker/checker.go:26410 slices.SortStableFunc(types, CompareTypes)
            self.sort_union_types(&mut types);
            types.dedup();
        }
        (types, includes)
    }

    // Go: checker/checker.go:26424 addNamedUnions
    pub fn add_named_unions(&self, named_unions: Vec<TypeId>, types: &[TypeId]) -> Vec<TypeId> {
        let mut named_unions = named_unions;
        for &t in types {
            let ty = self.ty(t);
            if ty.flags.intersects(TypeFlags::UNION) {
                let origin = ty.as_union_type().origin;
                let origin_is_union =
                    origin.is_some() && self.ty(origin).flags.intersects(TypeFlags::UNION);
                if ty.alias.is_some() || origin.is_some() && !origin_is_union {
                    if !named_unions.contains(&t) {
                        named_unions.push(t);
                    }
                } else if origin.is_some() && origin_is_union {
                    named_unions = self.add_named_unions(named_unions, self.ty(origin).types());
                }
            }
        }
        named_unions
    }

    // Go: checker/checker.go:26438 removeRedundantLiteralTypes
    // PORT: Go returns the filtered slice; Rust filters `types` in place.
    pub fn remove_redundant_literal_types(
        &mut self,
        types: &mut TypeSet,
        includes: TypeFlags,
        reduce_void_undefined: bool,
    ) {
        let mut i = types.len();
        while i > 0 {
            i -= 1;
            let t = types[i];
            let flags = self.ty(t).flags;
            let remove = flags.intersects(
                TypeFlags::STRING_LITERAL | TypeFlags::TEMPLATE_LITERAL | TypeFlags::STRING_MAPPING,
            ) && includes.intersects(TypeFlags::STRING)
                || flags.intersects(TypeFlags::NUMBER_LITERAL)
                    && includes.intersects(TypeFlags::NUMBER)
                || flags.intersects(TypeFlags::BIG_INT_LITERAL)
                    && includes.intersects(TypeFlags::BIG_INT)
                || flags.intersects(TypeFlags::UNIQUE_ES_SYMBOL)
                    && includes.intersects(TypeFlags::ES_SYMBOL)
                || reduce_void_undefined
                    && flags.intersects(TypeFlags::UNDEFINED)
                    && includes.intersects(TypeFlags::VOID)
                || self.is_fresh_literal_type(t)
                    && self.contains_type(types, self.ty(t).as_literal_type().regular_type);
            if remove {
                types.remove(i);
            }
        }
    }

    // Go: checker/checker.go:26457 removeStringLiteralsMatchedByTemplateLiterals
    // PORT: Go returns the filtered slice; Rust filters `types` in place.
    // PERF (sortmisc1): Go calls `isTypeMatchedByTemplateLiteralOrStringMapping`
    // for each template until one matches. For a template literal, that call
    // first tests the start and end texts (Go `inferFromLiteralPartsToTemplateLiteral`)
    // and returns false with no other effect when they do not match, which
    // is almost every pair. So the end texts of all templates are read once
    // (`TemplateLiteralEnds`), each string literal is tested against them,
    // and only the templates that pass (and string mappings) get Go's call,
    // in Go's order. The calls with an effect and their order are Go's.
    pub fn remove_string_literals_matched_by_template_literals(&mut self, types: &mut TypeSet) {
        let templates: Vec<TypeId> = types
            .iter()
            .copied()
            .filter(|&t| self.is_pattern_literal_type(t))
            .collect();
        if !templates.is_empty() {
            let ends = TemplateLiteralEnds::new(self, &templates);
            let mut candidates: SmallVec<[TypeId; 4]> = SmallVec::new();
            let mut i = types.len();
            while i > 0 {
                i -= 1;
                let t = types[i];
                if !self.ty(t).flags.intersects(TypeFlags::STRING_LITERAL) {
                    continue;
                }
                candidates.clear();
                self.template_candidates_of_string_literal(t, &templates, &ends, &mut candidates);
                if candidates.iter().any(|&template| {
                    self.is_type_matched_by_template_literal_or_string_mapping(t, template)
                }) {
                    types.remove(i);
                }
            }
        }
    }

    // Go: checker/checker.go:26474 isTypeMatchedByTemplateLiteralOrStringMapping
    pub fn is_type_matched_by_template_literal_or_string_mapping(
        &mut self,
        t: TypeId,
        template: TypeId,
    ) -> bool {
        if self
            .ty(template)
            .flags
            .intersects(TypeFlags::TEMPLATE_LITERAL)
        {
            // PERF (chkmid1): most pairs fail the start and end text test,
            // so it runs before the template and the comparer are cloned.
            if self.ty(t).flags.intersects(TypeFlags::STRING_LITERAL)
                && self.string_literal_misses_template_literal_ends(t, template)
            {
                return false;
            }
            let template_literal = self.ty(template).as_template_literal_type().clone();
            let comparer = self.compare_types_assignable.clone();
            return self.is_type_matched_by_template_literal_type(
                t,
                &template_literal,
                &mut |c: &mut Checker, s: TypeId, t: TypeId, r: bool| comparer(c, s, t, r),
            );
        }
        self.is_member_of_string_mapping(t, template)
    }

    // Go: checker/checker.go:26481 removeConstrainedTypeVariables
    // PORT: Go returns the new slice; Rust changes `types` in place.
    pub fn remove_constrained_type_variables(&mut self, types: &mut TypeSet) {
        let mut type_variables: Vec<TypeId> = Vec::new();
        // First collect a list of the type variables occurring in constraining intersections.
        for &t in types.iter() {
            if let Some((variable, _)) = self.constrained_type_variable_parts_p28(t) {
                if !type_variables.contains(&variable) {
                    type_variables.push(variable);
                }
            }
        }
        // For each type variable, check if the constraining intersections for that type variable fully
        // cover the constraint of the type variable; if so, remove the constraining intersections and
        // substitute the type variable.
        for type_variable in type_variables {
            let mut primitives: Vec<TypeId> = Vec::new();
            // First collect the primitive types from the constraining intersections.
            for &t in types.iter() {
                if let Some((variable, primitive)) = self.constrained_type_variable_parts_p28(t) {
                    if variable == type_variable {
                        (primitives, _) = self.insert_type(&primitives, primitive);
                    }
                }
            }
            // If every constituent in the type variable's constraint is covered by an intersection of the type
            // variable and that constituent, remove those intersections and substitute the type variable.
            let constraint = self.get_base_constraint_of_type(type_variable);
            let covered = {
                let primitives = &primitives;
                self.every_type(constraint, &mut |c: &mut Checker, t: TypeId| {
                    c.contains_type(primitives, t)
                })
            };
            if covered {
                let mut i = types.len();
                while i > 0 {
                    i -= 1;
                    let t = types[i];
                    if let Some((variable, primitive)) = self.constrained_type_variable_parts_p28(t)
                    {
                        if variable == type_variable && self.contains_type(&primitives, primitive) {
                            types.remove(i);
                        }
                    }
                }
                // Go `insertType(types, typeVariable)`, in place.
                let (index, found) = self.search_union_types(types, type_variable);
                if !found {
                    types.insert(index, type_variable);
                }
            }
        }
    }

    // PORT: shared helper for the three identical blocks in Go
    // removeConstrainedTypeVariables. For an intersection flagged
    // IsConstrainedTypeVariable it returns (types[index], types[1-index]),
    // where index is 0 when types[0] is a type variable and 1 otherwise.
    fn constrained_type_variable_parts_p28(&self, t: TypeId) -> Option<(TypeId, TypeId)> {
        let ty = self.ty(t);
        if ty.flags.intersects(TypeFlags::INTERSECTION)
            && ty
                .object_flags
                .intersects(ObjectFlags::IS_CONSTRAINED_TYPE_VARIABLE)
        {
            let members = &ty.as_intersection_type().union_or_intersection.types;
            let index = if !self
                .ty(members[0])
                .flags
                .intersects(TypeFlags::TYPE_VARIABLE)
            {
                1
            } else {
                0
            };
            return Some((members[index], members[1 - index]));
        }
        None
    }

    // Go: checker/checker.go:26534 removeSubtypes
    // PORT: Go returns the reduced slice, or nil when the union is too
    // complex. Rust reduces `types` in place and returns false for nil.
    pub fn remove_subtypes(&mut self, types: &mut TypeSet, has_object_types: bool) -> bool {
        // [] and [T] immediately reduce to [] and [T] respectively
        if types.len() < 2 {
            return true;
        }
        let key = self.get_type_list_key(types);
        if let Some(cached) = self.subtype_reduction_cache.get(&key) {
            types.clear();
            types.extend_from_slice(cached);
            return true;
        }
        // We assume that redundant primitive types have already been removed from the types array and that there
        // are no any and unknown types in the array. Thus, the only possible supertypes for primitive types are empty
        // object types, and if none of those are present we can exclude primitive types from the subtype check.
        let mut has_empty_object = false;
        if has_object_types {
            for &t in types.iter() {
                if self.ty(t).flags.intersects(TypeFlags::OBJECT) && !self.is_generic_mapped_type(t)
                {
                    self.resolve_structured_type_members(t);
                    if self.is_empty_resolved_type(t) {
                        has_empty_object = true;
                        break;
                    }
                }
            }
        }
        let length = types.len();
        let mut i = length;
        let mut count: usize = 0;
        while i > 0 {
            i -= 1;
            let source = types[i];
            let source_flags = self.ty(source).flags;
            if has_empty_object || source_flags.intersects(TypeFlags::STRUCTURED_OR_INSTANTIABLE) {
                // A type parameter with a union constraint may be a subtype of some union, but not a subtype of the
                // individual constituents of that union. For example, `T extends A | B` is a subtype of `A | B`, but not
                // a subtype of just `A` or just `B`. When we encounter such a type parameter, we therefore check if the
                // type parameter is a subtype of a union of all the other types.
                if source_flags.intersects(TypeFlags::TYPE_PARAMETER) {
                    let base = self.get_base_constraint_or_type(source);
                    if self.ty(base).flags.intersects(TypeFlags::UNION) {
                        let never = self.never_type;
                        let others: Vec<TypeId> = types
                            .iter()
                            .map(|&t| if t == source { never } else { t })
                            .collect();
                        let others_union = self.get_union_type(&others);
                        let relation = self.strict_subtype_relation.clone();
                        if self.is_type_related_to(source, others_union, &relation) {
                            types.remove(i);
                        }
                        continue;
                    }
                }
                // Find the first property with a unit type, if any. When constituents have a property by the same name
                // but of a different unit type, we can quickly disqualify them from subtype checks. This helps subtype
                // reduction of large discriminated union types.
                let mut key_property = SymbolId::NIL;
                let mut key_property_type = TypeId::NIL;
                if source_flags.intersects(
                    TypeFlags::OBJECT
                        | TypeFlags::INTERSECTION
                        | TypeFlags::INSTANTIABLE_NON_PRIMITIVE,
                ) {
                    for p in self.get_properties_of_type(source) {
                        let prop_type = self.get_type_of_symbol(p);
                        if self.is_unit_type(prop_type) {
                            key_property = p;
                            break;
                        }
                    }
                }
                if key_property.is_some() {
                    let prop_type = self.get_type_of_symbol(key_property);
                    key_property_type = self.get_regular_type_of_literal_type(prop_type);
                }
                // PORT: Go ranges over the slice captured at loop start and
                // breaks right after a delete, so indexing the live Vec matches.
                let mut j = 0;
                while j < types.len() {
                    let target = types[j];
                    j += 1;
                    if source != target {
                        if count == 100000 {
                            // After 100000 subtype checks we estimate the remaining amount of work by assuming the
                            // same ratio of checks per element. If the estimated number of remaining type checks is
                            // greater than 1M we deem the union type too complex to represent. This for example
                            // caps union types at 1000 unique object types.
                            let estimated_count = (count / (length - i)) * length;
                            if estimated_count > 1000000 {
                                if let Some(tr) = self.tracer {
                                    tr.instant(
                                        crate::tracing::Phase::CheckTypes,
                                        "removeSubtypes_DepthLimit",
                                        vec![("estimatedCount", estimated_count.into())],
                                    );
                                }
                                let node = self.current_node;
                                self.error(node, diag::Expression_produces_a_union_type_that_is_too_complex_to_represent, args![]);
                                return false;
                            }
                        }
                        count += 1;
                        if key_property.is_some()
                            && self.ty(target).flags.intersects(
                                TypeFlags::OBJECT
                                    | TypeFlags::INTERSECTION
                                    | TypeFlags::INSTANTIABLE_NON_PRIMITIVE,
                            )
                        {
                            let name = self.sym(key_property).name.clone();
                            let t = self.get_type_of_property_of_type(target, &name);
                            if t.is_some()
                                && self.is_unit_type(t)
                                && self.get_regular_type_of_literal_type(t) != key_property_type
                            {
                                continue;
                            }
                        }
                        if (source == self.empty_object_type
                            || source == self.unknown_empty_object_type)
                            && self.ty(target).symbol.is_some()
                            && self.is_empty_anonymous_object_type(target)
                        {
                            continue;
                        }
                        let relation = self.strict_subtype_relation.clone();
                        if self.is_type_related_to(source, target, &relation)
                            && (!self
                                .ty(self.get_target_type(source))
                                .object_flags
                                .intersects(ObjectFlags::CLASS)
                                || !self
                                    .ty(self.get_target_type(target))
                                    .object_flags
                                    .intersects(ObjectFlags::CLASS)
                                || self.is_type_derived_from(source, target))
                        {
                            types.remove(i);
                            break;
                        }
                    }
                }
            }
        }
        self.subtype_reduction_cache.insert(key, types.to_vec());
        true
    }
}

/// One call of `Checker::get_union_type_kept`, the key of `UnionCalls`.
struct UnionCall<'a> {
    reduction: UnionReduction,
    alias: Option<&'a TypeAlias>,
    /// The types as the caller passed them (not flattened, sorted or
    /// deduplicated).
    types: &'a [TypeId],
}

impl UnionCall<'_> {
    #[inline]
    fn alias_symbol(&self) -> u32 {
        self.alias.map_or(u32::MAX, |alias| alias.symbol.0)
    }

    #[inline]
    fn alias_type_arguments(&self) -> &[TypeId] {
        self.alias.map_or(&[], |alias| &alias.type_arguments)
    }

    #[inline]
    fn hash(&self) -> u64 {
        let mut h = rustc_hash::FxHasher::default();
        h.write_u32(self.reduction.0 as u32);
        h.write_u32(self.alias_symbol());
        for t in self.alias_type_arguments() {
            h.write_u32(t.0);
        }
        h.write_usize(self.types.len());
        for t in self.types {
            h.write_u32(t.0);
        }
        h.finish()
    }
}

/// A slot of `UnionCalls`.
#[derive(Clone, Copy, Default)]
struct KeptUnionCall {
    hash: u64,
    /// The call's types, then its alias type arguments, start here in
    /// `UnionCalls::types`.
    start: u32,
    /// 0 in an empty slot: a kept call has at least 2 types.
    types: u32,
    alias_type_arguments: u32,
    alias: u32,
    reduction: i32,
    answer: TypeId,
}

/// PERF (unionmemo1, not in Go; Bun `UnionCalls`, `check/unions.rs`):
/// `Checker::union_calls`, the results of recent `get_union_type_ex` calls
/// that have no origin and no subtype reduction and do not take Go's 2-type
/// union-of-unions path. The key is (reduction, alias, the types in call
/// order). A hit skips `add_types_to_union`, the reductions and the union
/// key. Go runs `getUnionTypeWorker` again on every call.
///
/// A kept result is what Go's worker returns for the same call at any later
/// time, and that later worker makes nothing:
/// - It reads only data that does not change once a type exists: type flags,
///   the object flags that it tests, alias, union origin and members, the
///   fresh and regular links of literals, and the sort data of
///   `compare_types` (memberflags1 Go note section 1.4; Go writes `alias`
///   and `origin` only directly after it makes the type).
/// - Its sort makes the comparisons of the first call, so it asks only for
///   lazy symbol ids that the first call assigned.
/// - `union_types` only grows, so its lookup in
///   `get_union_type_from_sorted_list` finds the union that the first call
///   found or made. So a hit makes no type: type ids and creation order stay
///   Go's.
/// - A call is kept only if `mark` did not move during its worker. The
///   template literal and constrained type variable reductions move it (they
///   relate and instantiate), and so does a new named-union origin (Go makes
///   a new origin type on each such call). An `error_type` result is not
///   kept.
///
/// Direct-mapped, at most 4096 slots. The types of the kept calls take at
/// most 2^18 places (1 MiB): when they are full, the types of overwritten
/// calls go (`compact`), and all calls go when the rest fill half of them. A
/// call with more than 2^16 types is not kept.
pub(crate) struct UnionCalls {
    /// A power of two of slots.
    slots: Box<[KeptUnionCall]>,
    /// A hash shifted right by this amount is a slot index.
    shift: u32,
    /// Calls kept since the table last grew or was cleared.
    added: usize,
    /// The types and alias type arguments of the kept calls, end to end.
    types: Vec<TypeId>,
    /// Moves at each step of `get_union_type_worker` that makes its call
    /// unsafe to keep.
    pub(crate) mark: u64,
}

impl Default for UnionCalls {
    fn default() -> Self {
        Self::with_slots(64)
    }
}

impl UnionCalls {
    const MOST_SLOTS: usize = 1 << 12;
    const MOST_TYPES: usize = 1 << 18;

    fn with_slots(len: usize) -> Self {
        Self {
            slots: vec![KeptUnionCall::default(); len].into_boxed_slice(),
            shift: 64 - len.trailing_zeros(),
            added: 0,
            types: Vec::new(),
            mark: 0,
        }
    }

    #[inline]
    fn slot(&self, hash: u64) -> usize {
        (hash.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> self.shift) as usize
    }

    #[inline]
    fn get(&self, hash: u64, call: &UnionCall<'_>) -> Option<TypeId> {
        let kept = &self.slots[self.slot(hash)];
        let args = call.alias_type_arguments();
        if kept.hash != hash
            || kept.types as usize != call.types.len()
            || kept.alias_type_arguments as usize != args.len()
            || kept.reduction != call.reduction.0
            || kept.alias != call.alias_symbol()
        {
            return None;
        }
        let (types, rest) = self.types[kept.start as usize..].split_at(call.types.len());
        (types == call.types && &rest[..args.len()] == args).then_some(kept.answer)
    }

    fn put(&mut self, hash: u64, call: &UnionCall<'_>, answer: TypeId) {
        let args = call.alias_type_arguments();
        let len = call.types.len() + args.len();
        if len > Self::MOST_TYPES / 4 {
            return;
        }
        if self.types.len() + len > Self::MOST_TYPES {
            self.compact();
            if self.types.len() + len > Self::MOST_TYPES / 2 {
                self.clear();
            }
        }
        if self.added == self.slots.len() && self.slots.len() < Self::MOST_SLOTS {
            self.grow();
        }
        self.added += 1;
        let slot = self.slot(hash);
        self.slots[slot] = KeptUnionCall {
            hash,
            start: self.types.len() as u32,
            types: call.types.len() as u32,
            alias_type_arguments: args.len() as u32,
            alias: call.alias_symbol(),
            reduction: call.reduction.0,
            answer,
        };
        self.types.extend_from_slice(call.types);
        self.types.extend_from_slice(args);
    }

    #[cold]
    fn grow(&mut self) {
        let len = self.slots.len() * 4;
        let old = std::mem::replace(
            &mut self.slots,
            vec![KeptUnionCall::default(); len].into_boxed_slice(),
        );
        self.shift = 64 - len.trailing_zeros();
        self.added = 0;
        for kept in old.iter().filter(|kept| kept.types != 0) {
            let slot = self.slot(kept.hash);
            self.slots[slot] = *kept;
        }
    }

    /// Keeps only the types of the calls in the slots.
    #[cold]
    fn compact(&mut self) {
        let mut types = Vec::with_capacity(self.types.capacity());
        for kept in self.slots.iter_mut().filter(|kept| kept.types != 0) {
            let start = kept.start as usize;
            let end = start + (kept.types + kept.alias_type_arguments) as usize;
            kept.start = types.len() as u32;
            types.extend_from_slice(&self.types[start..end]);
        }
        self.types = types;
    }

    #[cold]
    fn clear(&mut self) {
        self.slots.fill(KeptUnionCall::default());
        self.types.clear();
        self.added = 0;
    }
}

/// unionmemo1 verify (never merged): counters, written at exit to the file
/// that `GOPORT_UNIONMEMO_VERIFY` names. With it set, a hit runs the worker
/// too and checks it.
pub mod umv {
    use std::sync::OnceLock;
    use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

    pub static HITS: AtomicU64 = AtomicU64::new(0);
    pub static MISSES: AtomicU64 = AtomicU64::new(0);
    pub static KEPT: AtomicU64 = AtomicU64::new(0);
    pub static UNSAFE: AtomicU64 = AtomicU64::new(0);
    pub static ERROR: AtomicU64 = AtomicU64::new(0);
    pub static BAD: AtomicU64 = AtomicU64::new(0);

    pub fn on() -> bool {
        static ON: OnceLock<bool> = OnceLock::new();
        *ON.get_or_init(|| std::env::var_os("GOPORT_UNIONMEMO_VERIFY").is_some())
    }

    #[inline]
    pub fn add(c: &AtomicU64) -> u64 {
        if on() { c.fetch_add(1, Relaxed) } else { 0 }
    }

    pub fn dump() {
        let Some(path) = std::env::var_os("GOPORT_UNIONMEMO_VERIFY") else {
            return;
        };
        let out = format!(
            "hits {}\nmisses {}\nkept {}\nunsafe {}\nerror {}\nbad {}\n",
            HITS.load(Relaxed),
            MISSES.load(Relaxed),
            KEPT.load(Relaxed),
            UNSAFE.load(Relaxed),
            ERROR.load(Relaxed),
            BAD.load(Relaxed)
        );
        let _ = std::fs::write(path, out);
    }
}
