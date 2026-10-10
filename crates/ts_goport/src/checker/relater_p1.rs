//! Port of Go `checker/relater.go` lines 1-915: relation types, the public
//! relation entry points (`isTypeRelatedTo`, `checkTypeRelatedToEx`, ...),
//! simple and enum relations, error elaboration, weak type and known
//! property checks, recursion identities, and best matching type selection.

use crate::diagnostics::Message;
use crate::prelude::*;

// PORT: the Go flag types `SignatureCheckMode`, `MinArgumentCountFlags`,
// `IntersectionState`, `RecursionFlags`, `ExpandingFlags` and
// `RelationComparisonResult` (relater.go:18-75) live in `crate::flags`.
// PORT: Go `*Relation` is `Rc<RefCell<Relation>>`; a `*Relation` param is
// `&Rc<RefCell<Relation>>`. Go pointer equality on relations is `Rc::ptr_eq`.
// PORT: Go `diagnosticOutput *[]*ast.Diagnostic` is `Option<&mut Vec<Diagnostic>>`.
// PORT: a nullable Go `*diagnostics.Message` is `Option<&'static Message>`.

// Go: checker/relater.go:77 DiagnosticAndArguments
#[derive(Clone, Debug)]
pub struct DiagnosticAndArguments {
    pub message: &'static Message,
    pub arguments: Vec<String>,
}

// Go: checker/relater.go:82 ErrorOutputContainer
#[derive(Clone, Debug, Default)]
pub struct ErrorOutputContainer {
    pub errors: Vec<Diagnostic>,
    pub skip_logging: bool,
}

// Go: checker/relater.go:87 ErrorReporter
// PORT: Go `ErrorReporter func(message *diagnostics.Message, args ...any)`.
// A nil-able `ErrorReporter` param is `Option<ErrorReporter<'_>>`. The
// callback gets the checker as its first argument (Go closures capture it).
// Use `reborrow_error_reporter` to pass an optional reporter on more than once.
pub type ErrorReporter<'a> = &'a mut dyn FnMut(&mut Checker, &'static Message, Vec<String>);

// PORT: helper with no Go equivalent. Go passes the same `ErrorReporter`
// value to several callees; in Rust the `Option<&mut dyn FnMut>` must be
// reborrowed for each call.
pub fn reborrow_error_reporter<'s>(
    error_reporter: &'s mut Option<&mut dyn FnMut(&mut Checker, &'static Message, Vec<String>)>,
) -> Option<&'s mut dyn FnMut(&mut Checker, &'static Message, Vec<String>)> {
    match error_reporter {
        Some(f) => {
            let f: &mut dyn FnMut(&mut Checker, &'static Message, Vec<String>) = &mut **f;
            Some(f)
        }
        None => None,
    }
}

// Go: checker/relater.go:89 RecursionId
// PORT: Go `RecursionId{value any}` holds one of `*ast.Node`, `*ast.Symbol`
// or `*Type`. Go interface equality compares the dynamic type and pointer,
// which this enum reproduces.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RecursionId {
    Node(Node),
    Symbol(SymbolId),
    Type(TypeId),
}

impl Default for RecursionId {
    fn default() -> Self {
        RecursionId::Type(TypeId::NIL)
    }
}

impl From<Node> for RecursionId {
    fn from(value: Node) -> Self {
        RecursionId::Node(value)
    }
}

impl From<SymbolId> for RecursionId {
    fn from(value: SymbolId) -> Self {
        RecursionId::Symbol(value)
    }
}

impl From<TypeId> for RecursionId {
    fn from(value: TypeId) -> Self {
        RecursionId::Type(value)
    }
}

// Go: checker/relater.go:94 asRecursionId
// This function exists to constrain the types of values that can be used as recursion IDs.
pub fn as_recursion_id<T: Into<RecursionId>>(value: T) -> RecursionId {
    value.into()
}

/// Hasher for maps keyed by `CacheHashKey`. The key is already an xxh3 hash
/// and `CacheHashKey` hashes only its low half, so that half is the map hash
/// as is. Iteration order differs from `FxHashMap`, so use it only for maps
/// that no code iterates.
#[derive(Clone, Copy, Default)]
pub struct CacheKeyHasher(u64);

impl std::hash::Hasher for CacheKeyHasher {
    #[inline]
    fn finish(&self) -> u64 {
        self.0
    }

    #[inline]
    fn write_u64(&mut self, value: u64) {
        self.0 = value;
    }

    // Only reached if a key other than `CacheHashKey` is used.
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 = (self.0.rotate_left(5) ^ u64::from(b)).wrapping_mul(0x517c_c1b7_2722_0a95);
        }
    }
}

/// A map keyed by `CacheHashKey` that hashes with `CacheKeyHasher`.
pub type CacheKeyMap<V> =
    std::collections::HashMap<CacheHashKey, V, std::hash::BuildHasherDefault<CacheKeyHasher>>;

// PORT: perf. `CacheHashKey` is already an xxh3 hash, so `FlatMap` uses its
// low half as is (the same bits `CacheKeyHasher` uses).
impl FlatKey for CacheHashKey {
    #[inline]
    fn flat_hash(&self) -> u64 {
        self.lo
    }
}

// PORT: perf. Not in Go. Go keys every relation result by the xxh3 hash of
// the key bytes. A plain key has only 12 bytes of content (Go writes `'s'`,
// the source and target ids and the intersection state), so the port keeps
// those values as the key: it needs no xxh3 and compares exactly. A generic
// key (`'g'`, with type references) stays Go's xxh3 hash. Go's xxh3 keys
// collide with a chance of about 2^-128; plain keys here never do.
/// Go `getRelationKey` result: the key of a `Relation` result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RelationKey {
    /// A key without generic type references, as its key bytes.
    Plain(PlainRelationKey),
    /// Go's xxh3 hash of a key with generic type references.
    Generic(CacheHashKey),
}

/// The content of a plain relation key (Go `getRelationKey` writes `'s'`,
/// then these three values).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PlainRelationKey {
    pub source: u32,
    pub target: u32,
    pub intersection_state: u32,
}

/// Bits of the source and of the target id in a packed plain key.
const PACKED_ID_BITS: u32 = 28;

impl PlainRelationKey {
    /// The key in 58 bits for `PlainResultTable`: the source and target ids
    /// in 28 bits each and the intersection state in 2. `None` when a value
    /// does not fit, or when the packed key is 0 (an empty slot).
    #[inline]
    fn packed(&self) -> Option<u64> {
        if (self.source | self.target) >> PACKED_ID_BITS != 0 || self.intersection_state >> 2 != 0 {
            return None;
        }
        let p = u64::from(self.source)
            | u64::from(self.target) << PACKED_ID_BITS
            | u64::from(self.intersection_state) << (2 * PACKED_ID_BITS);
        (p != 0).then_some(p)
    }
}

impl FlatKey for PlainRelationKey {
    // A 64x64 to 128-bit multiply folded to 64 bits puts every key bit into
    // the low bits that pick the slot.
    #[inline]
    fn flat_hash(&self) -> u64 {
        let ids = u64::from(self.source) | u64::from(self.target) << 32;
        fold_mul(ids, u64::from(self.intersection_state))
    }
}

/// A 64x64 to 128-bit multiply of `x` and a constant changed by `y`, folded
/// to 64 bits, so every bit of `x` reaches the low bits.
#[inline]
fn fold_mul(x: u64, y: u64) -> u64 {
    let p = u128::from(x ^ 0x243f_6a88_85a3_08d3) * u128::from(0x9e37_79b9_7f4a_7c15 ^ y);
    (p >> 64) as u64 ^ p as u64
}

// Only for `RelationKeySet`: `CacheKeyHasher` takes one `u64` as the hash.
impl std::hash::Hash for RelationKey {
    #[inline]
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        state.write_u64(match self {
            RelationKey::Plain(key) => key.flat_hash(),
            RelationKey::Generic(key) => key.lo,
        });
    }
}

/// A set of `RelationKey` (the relater's maybe keys).
pub type RelationKeySet =
    std::collections::HashSet<RelationKey, std::hash::BuildHasherDefault<CacheKeyHasher>>;

/// Bits of the result in a `PlainResultTable` entry.
const RESULT_BITS: u32 = 6;
const _: () = assert!(
    (RelationComparisonResult::SUCCEEDED.bits()
        | RelationComparisonResult::FAILED.bits()
        | RelationComparisonResult::REPORTS_MASK.bits()
        | RelationComparisonResult::OVERFLOW.bits())
        >> RESULT_BITS
        == 0
);

// PORT: perf. Not in Go. The plain results of one relation, each in one
// `u64`: the packed key (`PlainRelationKey::packed`) in the high 58 bits and
// the result in the low 6 bits, so a cache line holds 8 results (a table of
// 12-byte keys and 4-byte results holds 4). Open addressing with linear
// probing, a power-of-two slot count and a load of at most 1/2; 0 is an
// empty slot. There is no iteration, so the layout cannot change any output.
#[derive(Clone, Debug, Default)]
pub struct PlainResultTable {
    /// Empty (no allocation) until the first insert; else a power of two.
    slots: Box<[u64]>,
    used: usize,
}

impl PlainResultTable {
    #[inline]
    fn home(packed: u64, mask: usize) -> usize {
        fold_mul(packed, 0) as usize & mask
    }

    #[inline]
    fn get(&self, packed: u64) -> RelationComparisonResult {
        if self.slots.is_empty() {
            return RelationComparisonResult::NONE;
        }
        let mask = self.slots.len() - 1;
        let mut i = Self::home(packed, mask);
        loop {
            let entry = self.slots[i];
            if entry >> RESULT_BITS == packed {
                return RelationComparisonResult((entry & ((1 << RESULT_BITS) - 1)) as u32);
            }
            if entry == 0 {
                return RelationComparisonResult::NONE;
            }
            i = (i + 1) & mask;
        }
    }

    #[inline]
    fn insert(&mut self, packed: u64, result: RelationComparisonResult) {
        debug_assert!(result.bits() >> RESULT_BITS == 0);
        let entry = packed << RESULT_BITS | u64::from(result.bits());
        if !self.slots.is_empty() {
            let mask = self.slots.len() - 1;
            let mut i = Self::home(packed, mask);
            loop {
                let old = self.slots[i];
                if old >> RESULT_BITS == packed {
                    self.slots[i] = entry;
                    return;
                }
                if old == 0 {
                    if self.used < self.slots.len() / 2 {
                        self.slots[i] = entry;
                        self.used += 1;
                        return;
                    }
                    break;
                }
                i = (i + 1) & mask;
            }
        }
        // The key is absent and the table is empty or full to its load.
        self.grow();
        self.insert_absent(entry);
        self.used += 1;
    }

    /// Doubles the slot count and puts every entry back.
    // PERF: `zeroed_vec`, as the probe reads a slot before it writes one.
    #[cold]
    fn grow(&mut self) {
        let len = (self.slots.len() * 2).max(16);
        let old = std::mem::replace(&mut self.slots, zeroed_vec(len).into_boxed_slice());
        for &entry in &*old {
            if entry != 0 {
                self.insert_absent(entry);
            }
        }
    }

    /// Writes an entry whose key is not in `slots` into its first empty slot.
    #[inline]
    fn insert_absent(&mut self, entry: u64) {
        let mask = self.slots.len() - 1;
        let mut i = Self::home(entry >> RESULT_BITS, mask);
        while self.slots[i] != 0 {
            i = (i + 1) & mask;
        }
        self.slots[i] = entry;
    }
}

// Go: checker/relater.go:98 Relation
// PORT: Go has one map of xxh3 keys. Here plain keys are in `plain` (or in
// `plain_wide` when they do not pack) and generic keys in `generic` (see
// `RelationKey`). There is no iteration, so this cannot change any output.
#[derive(Clone, Debug, Default)]
pub struct Relation {
    pub plain: PlainResultTable,
    /// Plain keys with an id of 2^28 or more.
    pub plain_wide: FlatMap<PlainRelationKey, RelationComparisonResult>,
    pub generic: FlatMap<CacheHashKey, RelationComparisonResult>,
}

impl Relation {
    // Go: checker/relater.go:102 Relation.get
    #[inline]
    pub fn get(&self, key: RelationKey) -> RelationComparisonResult {
        match key {
            RelationKey::Plain(key) => match key.packed() {
                Some(packed) => self.plain.get(packed),
                None => self.plain_wide.get(&key).copied().unwrap_or_default(),
            },
            RelationKey::Generic(key) => self.generic.get(&key).copied().unwrap_or_default(),
        }
    }

    // Go: checker/relater.go:106 Relation.set
    #[inline]
    pub fn set(&mut self, key: RelationKey, result: RelationComparisonResult) {
        match key {
            RelationKey::Plain(key) => match key.packed() {
                Some(packed) => self.plain.insert(packed, result),
                None => {
                    self.plain_wide.insert(key, result);
                }
            },
            RelationKey::Generic(key) => {
                self.generic.insert(key, result);
            }
        }
    }

    // Go: checker/relater.go:113 Relation.size
    pub fn size(&self) -> i32 {
        (self.plain.used + self.plain_wide.len() + self.generic.len()) as i32
    }
}

// PORT: perf. Not in Go. Go tests a relation by pointer (`relation ==
// c.identityRelation`). Hot code here tests a `RelationKind`, so it does not
// clone the `Rc` of the relation to keep it while `self` is borrowed.
/// One of the checker's five relations (see `Checker::relation_of`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RelationKind {
    #[default]
    Identity,
    Subtype,
    StrictSubtype,
    Assignable,
    Comparable,
}

impl Checker {
    /// The relation of `kind`.
    #[inline]
    pub fn relation_of(&self, kind: RelationKind) -> &Rc<RefCell<Relation>> {
        match kind {
            RelationKind::Identity => &self.identity_relation,
            RelationKind::Subtype => &self.subtype_relation,
            RelationKind::StrictSubtype => &self.strict_subtype_relation,
            RelationKind::Assignable => &self.assignable_relation,
            RelationKind::Comparable => &self.comparable_relation,
        }
    }

    /// The kind of `relation`, which is one of the checker's five relations.
    #[inline]
    pub fn relation_kind(&self, relation: &Rc<RefCell<Relation>>) -> RelationKind {
        if Rc::ptr_eq(relation, &self.assignable_relation) {
            RelationKind::Assignable
        } else if Rc::ptr_eq(relation, &self.subtype_relation) {
            RelationKind::Subtype
        } else if Rc::ptr_eq(relation, &self.strict_subtype_relation) {
            RelationKind::StrictSubtype
        } else if Rc::ptr_eq(relation, &self.comparable_relation) {
            RelationKind::Comparable
        } else {
            debug_assert!(Rc::ptr_eq(relation, &self.identity_relation));
            RelationKind::Identity
        }
    }
}

impl Checker {
    // Go: checker/relater.go:117 isTypeIdenticalTo
    pub fn is_type_identical_to(&mut self, source: TypeId, target: TypeId) -> bool {
        self.is_type_related_to_kind(source, target, RelationKind::Identity)
    }

    // Go: checker/relater.go:121 compareTypesIdentical
    pub fn compare_types_identical(&mut self, source: TypeId, target: TypeId) -> Ternary {
        if self.is_type_related_to_kind(source, target, RelationKind::Identity) {
            return Ternary::TRUE;
        }
        Ternary::FALSE
    }

    // Go: checker/relater.go:128 compareTypesAssignableSimple
    pub fn compare_types_assignable_simple(&mut self, source: TypeId, target: TypeId) -> Ternary {
        if self.is_type_related_to_kind(source, target, RelationKind::Assignable) {
            return Ternary::TRUE;
        }
        Ternary::FALSE
    }

    // Go: checker/relater.go:135 compareTypesAssignableWorker
    pub fn compare_types_assignable_worker(
        &mut self,
        source: TypeId,
        target: TypeId,
        report_errors: bool,
    ) -> Ternary {
        if self.is_type_related_to_kind(source, target, RelationKind::Assignable) {
            return Ternary::TRUE;
        }
        Ternary::FALSE
    }

    // Go: checker/relater.go:142 compareTypesSubtypeOf
    pub fn compare_types_subtype_of(&mut self, source: TypeId, target: TypeId) -> Ternary {
        if self.is_type_related_to_kind(source, target, RelationKind::Subtype) {
            return Ternary::TRUE;
        }
        Ternary::FALSE
    }

    // Go: checker/relater.go:149 isTypeAssignableTo
    pub fn is_type_assignable_to(&mut self, source: TypeId, target: TypeId) -> bool {
        self.is_type_related_to_kind(source, target, RelationKind::Assignable)
    }

    // Go: checker/relater.go:153 isTypeSubtypeOf
    pub fn is_type_subtype_of(&mut self, source: TypeId, target: TypeId) -> bool {
        self.is_type_related_to_kind(source, target, RelationKind::Subtype)
    }

    // Go: checker/relater.go:157 isTypeStrictSubtypeOf
    pub fn is_type_strict_subtype_of(&mut self, source: TypeId, target: TypeId) -> bool {
        self.is_type_related_to_kind(source, target, RelationKind::StrictSubtype)
    }

    // Go: checker/relater.go:161 isTypeComparableTo
    pub fn is_type_comparable_to(&mut self, source: TypeId, target: TypeId) -> bool {
        self.is_type_related_to_kind(source, target, RelationKind::Comparable)
    }

    // Go: checker/relater.go:165 areTypesComparable
    pub fn are_types_comparable(&mut self, type1: TypeId, type2: TypeId) -> bool {
        self.is_type_comparable_to(type1, type2) || self.is_type_comparable_to(type2, type1)
    }

    // Go: checker/relater.go:169 isTypeRelatedTo
    #[inline]
    pub fn is_type_related_to(
        &mut self,
        source: TypeId,
        target: TypeId,
        relation: &Rc<RefCell<Relation>>,
    ) -> bool {
        let kind = self.relation_kind(relation);
        self.is_type_related_to_kind(source, target, kind)
    }

    // PORT: perf. Go isTypeRelatedTo with the relation as a `RelationKind`.
    pub fn is_type_related_to_kind(
        &mut self,
        source: TypeId,
        target: TypeId,
        kind: RelationKind,
    ) -> bool {
        let mut source = source;
        let mut target = target;
        if self.is_fresh_literal_type(source) {
            source = self.ty(source).as_literal_type().regular_type;
        }
        if self.is_fresh_literal_type(target) {
            target = self.ty(target).as_literal_type().regular_type;
        }
        if source == target {
            return true;
        }
        let is_identity = kind == RelationKind::Identity;
        if !is_identity {
            if kind == RelationKind::Comparable
                && !self.ty(target).flags.intersects(TypeFlags::NEVER)
                && self.is_simple_type_related_to_kind(target, source, kind, None)
                || self.is_simple_type_related_to_kind(source, target, kind, None)
            {
                return true;
            }
        } else if !(self.ty(source).flags | self.ty(target).flags).intersects(
            TypeFlags::UNION_OR_INTERSECTION
                | TypeFlags::INDEXED_ACCESS
                | TypeFlags::CONDITIONAL
                | TypeFlags::SUBSTITUTION,
        ) {
            // We have excluded types that may simplify to other forms, so types must have identical flags
            if self.ty(source).flags != self.ty(target).flags {
                return false;
            }
            if self.ty(source).flags.intersects(TypeFlags::SINGLETON) {
                return true;
            }
        }
        if self.ty(source).flags.intersects(TypeFlags::OBJECT)
            && self.ty(target).flags.intersects(TypeFlags::OBJECT)
        {
            let (id, _) =
                self.get_relation_key(source, target, IntersectionState::NONE, is_identity, false);
            let related = self.relation_of(kind).borrow().get(id);
            if related != RelationComparisonResult::NONE {
                return related.intersects(RelationComparisonResult::SUCCEEDED);
            }
        }
        if self
            .ty(source)
            .flags
            .intersects(TypeFlags::STRUCTURED_OR_INSTANTIABLE)
            || self
                .ty(target)
                .flags
                .intersects(TypeFlags::STRUCTURED_OR_INSTANTIABLE)
        {
            let relation = self.relation_of(kind).clone();
            return self.check_type_related_to(
                source,
                target,
                &relation,
                Node::NIL, /*errorNode*/
            );
        }
        false
    }

    // Go: checker/relater.go:205 isSimpleTypeRelatedTo
    #[inline]
    pub fn is_simple_type_related_to(
        &mut self,
        source: TypeId,
        target: TypeId,
        relation: &Rc<RefCell<Relation>>,
        error_reporter: Option<&mut dyn FnMut(&mut Checker, &'static Message, Vec<String>)>,
    ) -> bool {
        let kind = self.relation_kind(relation);
        self.is_simple_type_related_to_kind(source, target, kind, error_reporter)
    }

    // PORT: perf. Go isSimpleTypeRelatedTo with the relation as a
    // `RelationKind`.
    pub fn is_simple_type_related_to_kind(
        &mut self,
        source: TypeId,
        target: TypeId,
        kind: RelationKind,
        mut error_reporter: Option<&mut dyn FnMut(&mut Checker, &'static Message, Vec<String>)>,
    ) -> bool {
        let s = self.ty(source).flags;
        let t = self.ty(target).flags;
        if t.intersects(TypeFlags::ANY)
            || s.intersects(TypeFlags::NEVER)
            || source == self.wildcard_type
        {
            return true;
        }
        if t.intersects(TypeFlags::UNKNOWN)
            && !(kind == RelationKind::StrictSubtype && s.intersects(TypeFlags::ANY))
        {
            return true;
        }
        if t.intersects(TypeFlags::NEVER) {
            return false;
        }
        // PORT: perf. Every rule from here to the null rule needs one of
        // these source flags, so one test skips them all for other sources.
        const PRIMITIVE_RULE_SOURCE: TypeFlags = TypeFlags(
            TypeFlags::STRING_LIKE.bits()
                | TypeFlags::NUMBER_LIKE.bits()
                | TypeFlags::BIG_INT_LIKE.bits()
                | TypeFlags::BOOLEAN_LIKE.bits()
                | TypeFlags::ES_SYMBOL_LIKE.bits()
                | TypeFlags::ENUM.bits()
                | TypeFlags::ENUM_LITERAL.bits()
                | TypeFlags::UNDEFINED.bits()
                | TypeFlags::NULL.bits(),
        );
        if s.intersects(PRIMITIVE_RULE_SOURCE) {
            if s.intersects(TypeFlags::STRING_LIKE) && t.intersects(TypeFlags::STRING) {
                return true;
            }
            if s.intersects(TypeFlags::STRING_LITERAL)
                && s.intersects(TypeFlags::ENUM_LITERAL)
                && t.intersects(TypeFlags::STRING_LITERAL)
                && !t.intersects(TypeFlags::ENUM_LITERAL)
                && self.ty(source).as_literal_type().value
                    == self.ty(target).as_literal_type().value
            {
                return true;
            }
            if s.intersects(TypeFlags::NUMBER_LIKE) && t.intersects(TypeFlags::NUMBER) {
                return true;
            }
            if s.intersects(TypeFlags::NUMBER_LITERAL)
                && s.intersects(TypeFlags::ENUM_LITERAL)
                && t.intersects(TypeFlags::NUMBER_LITERAL)
                && !t.intersects(TypeFlags::ENUM_LITERAL)
                && self.ty(source).as_literal_type().value
                    == self.ty(target).as_literal_type().value
            {
                return true;
            }
            if s.intersects(TypeFlags::BIG_INT_LIKE) && t.intersects(TypeFlags::BIG_INT) {
                return true;
            }
            if s.intersects(TypeFlags::BOOLEAN_LIKE) && t.intersects(TypeFlags::BOOLEAN) {
                return true;
            }
            if s.intersects(TypeFlags::ES_SYMBOL_LIKE) && t.intersects(TypeFlags::ES_SYMBOL) {
                return true;
            }
            let source_symbol = self.ty(source).symbol;
            let target_symbol = self.ty(target).symbol;
            if s.intersects(TypeFlags::ENUM)
                && t.intersects(TypeFlags::ENUM)
                && self.sym(source_symbol).name == self.sym(target_symbol).name
                && self.is_enum_type_related_to(
                    source_symbol,
                    target_symbol,
                    reborrow_error_reporter(&mut error_reporter),
                )
            {
                return true;
            }
            if s.intersects(TypeFlags::ENUM_LITERAL) && t.intersects(TypeFlags::ENUM_LITERAL) {
                if s.intersects(TypeFlags::UNION)
                    && t.intersects(TypeFlags::UNION)
                    && self.is_enum_type_related_to(
                        source_symbol,
                        target_symbol,
                        reborrow_error_reporter(&mut error_reporter),
                    )
                {
                    return true;
                }
                if s.intersects(TypeFlags::LITERAL)
                    && t.intersects(TypeFlags::LITERAL)
                    && self.ty(source).as_literal_type().value
                        == self.ty(target).as_literal_type().value
                    && self.is_enum_type_related_to(
                        source_symbol,
                        target_symbol,
                        reborrow_error_reporter(&mut error_reporter),
                    )
                {
                    return true;
                }
            }
            // In non-strictNullChecks mode, `undefined` and `null` are assignable to anything except `never`.
            // Since unions and intersections may reduce to `never`, we exclude them here.
            if s.intersects(TypeFlags::UNDEFINED)
                && (!self.strict_null_checks && !t.intersects(TypeFlags::UNION_OR_INTERSECTION)
                    || t.intersects(TypeFlags::UNDEFINED | TypeFlags::VOID))
            {
                return true;
            }
            if s.intersects(TypeFlags::NULL)
                && (!self.strict_null_checks && !t.intersects(TypeFlags::UNION_OR_INTERSECTION)
                    || t.intersects(TypeFlags::NULL))
            {
                return true;
            }
        }
        if s.intersects(TypeFlags::OBJECT)
            && t.intersects(TypeFlags::NON_PRIMITIVE)
            && !(kind == RelationKind::StrictSubtype
                && self.is_empty_anonymous_object_type(source)
                && !self
                    .ty(source)
                    .object_flags
                    .intersects(ObjectFlags::FRESH_LITERAL))
        {
            return true;
        }
        if kind == RelationKind::Assignable || kind == RelationKind::Comparable {
            if s.intersects(TypeFlags::ANY) {
                return true;
            }
            // Type number is assignable to any computed numeric enum type or any numeric enum literal type, and
            // a numeric literal type is assignable any computed numeric enum type or any numeric enum literal type
            // with a matching value. These rules exist such that enums can be used for bit-flag purposes.
            if s.intersects(TypeFlags::NUMBER)
                && (t.intersects(TypeFlags::ENUM)
                    || t.intersects(TypeFlags::NUMBER_LITERAL)
                        && t.intersects(TypeFlags::ENUM_LITERAL))
            {
                return true;
            }
            if s.intersects(TypeFlags::NUMBER_LITERAL)
                && !s.intersects(TypeFlags::ENUM_LITERAL)
                && (t.intersects(TypeFlags::ENUM)
                    || t.intersects(TypeFlags::NUMBER_LITERAL)
                        && t.intersects(TypeFlags::ENUM_LITERAL)
                        && self.ty(source).as_literal_type().value
                            == self.ty(target).as_literal_type().value)
            {
                return true;
            }
            // Anything is assignable to a union containing undefined, null, and {}
            if self.is_unknown_like_union_type(target) {
                return true;
            }
        }
        false
    }

    // Go: checker/relater.go:281 isEnumTypeRelatedTo
    pub fn is_enum_type_related_to(
        &mut self,
        source: SymbolId,
        target: SymbolId,
        mut error_reporter: Option<&mut dyn FnMut(&mut Checker, &'static Message, Vec<String>)>,
    ) -> bool {
        let source_symbol = if self.sym(source).flags.intersects(SymbolFlags::ENUM_MEMBER) {
            self.get_parent_of_symbol(source)
        } else {
            source
        };
        let target_symbol = if self.sym(target).flags.intersects(SymbolFlags::ENUM_MEMBER) {
            self.get_parent_of_symbol(target)
        } else {
            target
        };
        if source_symbol == target_symbol {
            return true;
        }
        if self.sym(source_symbol).name != self.sym(target_symbol).name
            || !self
                .sym(source_symbol)
                .flags
                .intersects(SymbolFlags::REGULAR_ENUM)
            || !self
                .sym(target_symbol)
                .flags
                .intersects(SymbolFlags::REGULAR_ENUM)
        {
            return false;
        }
        // PORT: Go keys by `ast.GetSymbolId`; `EnumRelationKey` holds symbol handles.
        // The Go calls give the symbols their ids, so they are made too
        // (`ValueSymbolLinkStore`).
        get_symbol_id(&self.symbols, source_symbol);
        get_symbol_id(&self.symbols, target_symbol);
        let key = EnumRelationKey {
            source_id: source_symbol,
            target_id: target_symbol,
        };
        let entry = self.enum_relation.get(&key).copied().unwrap_or_default();
        if entry != RelationComparisonResult::NONE
            && !(entry.intersects(RelationComparisonResult::FAILED) && error_reporter.is_some())
        {
            return entry.intersects(RelationComparisonResult::SUCCEEDED);
        }
        let target_enum_type = self.get_type_of_symbol(target_symbol);
        let source_enum_type = self.get_type_of_symbol(source_symbol);
        for source_property in self.get_properties_of_type(source_enum_type) {
            if self
                .sym(source_property)
                .flags
                .intersects(SymbolFlags::ENUM_MEMBER)
            {
                let source_property_name = self.sym(source_property).name.clone();
                let target_property =
                    self.get_property_of_type(target_enum_type, &source_property_name);
                if target_property.is_nil()
                    || !self
                        .sym(target_property)
                        .flags
                        .intersects(SymbolFlags::ENUM_MEMBER)
                {
                    if let Some(reporter) = error_reporter.as_mut() {
                        let property_string = self.symbol_to_string(source_property);
                        let declared_type = self.get_declared_type_of_symbol(target_symbol);
                        let type_string = self.type_to_string_ex(
                            declared_type,
                            Node::NIL, /*enclosingDeclaration*/
                            TypeFormatFlags::USE_FULLY_QUALIFIED_TYPE,
                            None,
                        );
                        (**reporter)(
                            self,
                            diag::Property_0_is_missing_in_type_1,
                            args![property_string, type_string],
                        );
                    }
                    self.enum_relation
                        .insert(key, RelationComparisonResult::FAILED);
                    return false;
                }
                let source_declaration =
                    get_declaration_of_kind(&self.symbols, source_property, SyntaxKind::EnumMember);
                let source_value = self.get_enum_member_value(source_declaration).value;
                let target_declaration =
                    get_declaration_of_kind(&self.symbols, target_property, SyntaxKind::EnumMember);
                let target_value = self.get_enum_member_value(target_declaration).value;
                if source_value != target_value {
                    // If we have 2 enums with *known* values that differ, they are incompatible.
                    if let (Some(sv), Some(tv)) = (&source_value, &target_value) {
                        if let Some(reporter) = error_reporter.as_mut() {
                            let target_symbol_string = self.symbol_to_string(target_symbol);
                            let target_property_string = self.symbol_to_string(target_property);
                            // PORT: Go `c.valueToString(v)` is a wrapper around `ValueToString`.
                            let target_value_string = value_to_string(tv);
                            let source_value_string = value_to_string(sv);
                            (**reporter)(
                                self,
                                diag::Each_declaration_of_0_1_differs_in_its_value_where_2_was_expected_but_3_was_given,
                                args![target_symbol_string, target_property_string, target_value_string, source_value_string],
                            );
                        }
                        self.enum_relation
                            .insert(key, RelationComparisonResult::FAILED);
                        return false;
                    }
                    // At this point we know that at least one of the values is 'undefined'.
                    // This may mean that we have an opaque member from an ambient enum declaration,
                    // or that we were not able to calculate it (which is basically an error).
                    //
                    // Either way, we can assume that it's numeric.
                    // If the other is a string, we have a mismatch in types.
                    let source_is_string = matches!(source_value, Some(LiteralValue::String(_)));
                    let target_is_string = matches!(target_value, Some(LiteralValue::String(_)));
                    if source_is_string || target_is_string {
                        if let Some(reporter) = error_reporter.as_mut() {
                            // Go: core.OrElse(sourceValue, targetValue)
                            let known_string_value = if source_value.is_some() {
                                &source_value
                            } else {
                                &target_value
                            };
                            let target_symbol_string = self.symbol_to_string(target_symbol);
                            let target_property_string = self.symbol_to_string(target_property);
                            let known_string = value_to_string(
                                known_string_value
                                    .as_ref()
                                    .expect("unhandled value type in valueToString"),
                            );
                            (**reporter)(
                                self,
                                diag::One_value_of_0_1_is_the_string_2_and_the_other_is_assumed_to_be_an_unknown_numeric_value,
                                args![target_symbol_string, target_property_string, known_string],
                            );
                        }
                        self.enum_relation
                            .insert(key, RelationComparisonResult::FAILED);
                        return false;
                    }
                }
            }
        }
        self.enum_relation
            .insert(key, RelationComparisonResult::SUCCEEDED);
        true
    }

    // Go: checker/relater.go:339 checkTypeAssignableTo
    pub fn check_type_assignable_to(
        &mut self,
        source: TypeId,
        target: TypeId,
        error_node: Node,
        head_message: Option<&'static Message>,
    ) -> bool {
        let relation = self.assignable_relation.clone();
        self.check_type_related_to_ex(source, target, &relation, error_node, head_message, None)
    }

    // Go: checker/relater.go:343 checkTypeAssignableToEx
    pub fn check_type_assignable_to_ex(
        &mut self,
        source: TypeId,
        target: TypeId,
        error_node: Node,
        head_message: Option<&'static Message>,
        diagnostic_output: Option<&mut Vec<Diagnostic>>,
    ) -> bool {
        let relation = self.assignable_relation.clone();
        self.check_type_related_to_ex(
            source,
            target,
            &relation,
            error_node,
            head_message,
            diagnostic_output,
        )
    }

    // Go: checker/relater.go:347 checkTypeComparableTo
    pub fn check_type_comparable_to(
        &mut self,
        source: TypeId,
        target: TypeId,
        error_node: Node,
        head_message: Option<&'static Message>,
    ) -> bool {
        let relation = self.comparable_relation.clone();
        self.check_type_related_to_ex(source, target, &relation, error_node, head_message, None)
    }

    // Go: checker/relater.go:351 checkTypeRelatedTo
    pub fn check_type_related_to(
        &mut self,
        source: TypeId,
        target: TypeId,
        relation: &Rc<RefCell<Relation>>,
        error_node: Node,
    ) -> bool {
        self.check_type_related_to_ex(source, target, relation, error_node, None, None)
    }

    // Go: checker/relater.go:358 checkTypeRelatedToEx
    // Check that source is related to target according to the given relation. When errorNode is non-nil, errors are
    // reported to the checker's diagnostic collection or through diagnosticOutput when non-nil. Callers can assume that
    // this function only reports zero or one error to diagnosticOutput (unlike checkTypeRelatedToAndOptionallyElaborate).
    // PORT: `Relater` is `Rc<RefCell<Relater>>` (see `Checker::free_relater`).
    // Relater methods take the checker as their first argument, since the
    // Go `r.c` back pointer cannot be stored.
    pub fn check_type_related_to_ex(
        &mut self,
        source: TypeId,
        target: TypeId,
        relation: &Rc<RefCell<Relation>>,
        error_node: Node,
        head_message: Option<&'static Message>,
        diagnostic_output: Option<&mut Vec<Diagnostic>>,
    ) -> bool {
        let mut error_node = error_node;
        let r = self.get_relater();
        {
            let kind = self.relation_kind(relation);
            let relation_size = relation.borrow().size();
            let mut rb = r.borrow_mut();
            rb.kind = kind;
            rb.error_node = error_node;
            rb.relation_count = (16_000_000 - relation_size) / 8;
        }
        let result = self.is_related_to_ex(
            &r,
            source,
            target,
            RecursionFlags::BOTH,
            error_node.is_some(), /*reportErrors*/
            head_message,
            IntersectionState::NONE,
        );
        let (overflow, has_error_chain) = {
            let rb = r.borrow();
            (rb.overflow, rb.error_chain.is_some())
        };
        if overflow {
            // Record this relation as having failed such that we don't attempt the overflowing operation again.
            let is_identity = Rc::ptr_eq(relation, &self.identity_relation);
            let (id, _) = self.get_relation_key(
                source,
                target,
                IntersectionState::NONE,
                is_identity,
                false, /*ignoreConstraints*/
            );
            relation.borrow_mut().set(
                id,
                RelationComparisonResult::FAILED | RelationComparisonResult::COMPLEXITY_OVERFLOW,
            );
            if let Some(tr) = self.tracer {
                let (depth, target_depth) = {
                    let rb = r.borrow();
                    (rb.source_stack.len(), rb.target_stack.len())
                };
                tr.instant(
                    crate::tracing::Phase::CheckTypes,
                    "checkTypeRelatedTo_DepthLimit",
                    vec![
                        ("sourceId", source.into()),
                        ("targetId", target.into()),
                        ("depth", depth.into()),
                        ("targetDepth", target_depth.into()),
                    ],
                );
            }
            if error_node.is_nil() {
                error_node = self.current_node;
            }
            let source_string = self.type_to_string(source);
            let target_string = self.type_to_string(target);
            let diagnostic = new_diagnostic_for_node(
                error_node,
                diag::Excessive_complexity_comparing_types_0_and_1,
                args![source_string, target_string],
            );
            self.report_diagnostic(diagnostic, diagnostic_output);
        } else if has_error_chain {
            // Check if we should issue an extra diagnostic to produce a quickfix for a slightly incorrect import statement
            let source_symbol = self.ty(source).symbol;
            if head_message.is_some()
                && error_node.is_some()
                && result == Ternary::FALSE
                && source_symbol.is_some()
                && self.export_type_links.has(source_symbol)
            {
                let (originating_import, links_target) = {
                    let links = self.export_type_links.get(source_symbol);
                    (links.originating_import, links.target)
                };
                if originating_import.is_some() && !is_import_call(originating_import) {
                    let links_target_type = self.get_type_of_symbol(links_target);
                    let helpful_retry = self.check_type_related_to(
                        links_target_type,
                        target,
                        relation,
                        Node::NIL, /*errorNode*/
                    );
                    if helpful_retry {
                        // Likely an incorrect import. Issue a helpful diagnostic to produce a quickfix to change the import
                        r.borrow_mut().related_info.push(create_diagnostic_for_node(
                            originating_import,
                            diag::Type_originates_at_this_import_A_namespace_style_import_cannot_be_called_or_constructed_and_will_cause_a_failure_at_runtime_Consider_using_a_default_import_or_import_require_here_instead,
                            args![],
                        ));
                    }
                }
            }
            let diagnostic = {
                let rb = r.borrow();
                create_diagnostic_chain_from_error_chain(
                    rb.error_chain.as_deref(),
                    rb.error_node,
                    rb.related_info.as_slice(),
                )
            };
            // PORT: Go `reportDiagnostic` ignores a nil diagnostic.
            if let Some(diagnostic) = diagnostic {
                self.report_diagnostic(diagnostic, diagnostic_output);
            }
        }
        self.put_relater(r);
        result != Ternary::FALSE
    }
}

// Go: checker/relater.go:400 createDiagnosticChainFromErrorChain
// PORT: Go `*ErrorChain` is read through `Option<&ErrorChain>`; callers pass
// `relater.error_chain.as_deref()`. A nil result is `None`.
pub fn create_diagnostic_chain_from_error_chain(
    chain: Option<&ErrorChain>,
    error_node: Node,
    related_info: &[Diagnostic],
) -> Option<Diagnostic> {
    let mut chain = chain;
    while let Some(c) = chain {
        if !c.message.elided_in_compatibility_pyramid() {
            break;
        }
        chain = c.next.as_deref();
    }
    let chain = chain?;
    let next =
        create_diagnostic_chain_from_error_chain(chain.next.as_deref(), error_node, related_info);
    match next {
        None => {
            let mut diagnostic =
                new_diagnostic_for_node(error_node, chain.message, chain.args.clone());
            diagnostic.set_related_info(related_info.to_vec());
            Some(diagnostic)
        }
        Some(next) => Some(new_diagnostic_chain(
            Some(next),
            chain.message,
            chain.args.clone(),
        )),
    }
}

impl Checker {
    // Go: checker/relater.go:414 reportDiagnostic
    // PORT: Go takes a nil-able `*ast.Diagnostic`; every Go caller except
    // `checkTypeRelatedToEx` passes a non-nil value, so this takes an owned
    // `Diagnostic` and that caller checks for nil itself.
    pub fn report_diagnostic(
        &mut self,
        diagnostic: Diagnostic,
        diagnostic_output: Option<&mut Vec<Diagnostic>>,
    ) {
        if let Some(diagnostic_output) = diagnostic_output {
            diagnostic_output.push(diagnostic);
        } else {
            self.add_diagnostic(diagnostic);
        }
    }

    // Go: checker/relater.go:424 checkTypeAssignableToAndOptionallyElaborate
    pub fn check_type_assignable_to_and_optionally_elaborate(
        &mut self,
        source: TypeId,
        target: TypeId,
        error_node: Node,
        expr: Node,
        head_message: Option<&'static Message>,
        diagnostic_output: Option<&mut Vec<Diagnostic>>,
    ) -> bool {
        let relation = self.assignable_relation.clone();
        self.check_type_related_to_and_optionally_elaborate(
            source,
            target,
            &relation,
            error_node,
            expr,
            head_message,
            diagnostic_output,
        )
    }

    // Go: checker/relater.go:428 checkTypeRelatedToAndOptionallyElaborate
    pub fn check_type_related_to_and_optionally_elaborate(
        &mut self,
        source: TypeId,
        target: TypeId,
        relation: &Rc<RefCell<Relation>>,
        error_node: Node,
        expr: Node,
        head_message: Option<&'static Message>,
        mut diagnostic_output: Option<&mut Vec<Diagnostic>>,
    ) -> bool {
        if self.is_type_related_to(source, target, relation) {
            return true;
        }
        if error_node.is_some()
            && !self.elaborate_error(
                expr,
                source,
                target,
                relation,
                head_message,
                diagnostic_output.as_deref_mut(),
            )
        {
            return self.check_type_related_to_ex(
                source,
                target,
                relation,
                error_node,
                head_message,
                diagnostic_output,
            );
        }
        false
    }

    // Go: checker/relater.go:438 elaborateError
    pub fn elaborate_error(
        &mut self,
        node: Node,
        source: TypeId,
        target: TypeId,
        relation: &Rc<RefCell<Relation>>,
        head_message: Option<&'static Message>,
        mut diagnostic_output: Option<&mut Vec<Diagnostic>>,
    ) -> bool {
        if node.is_nil() || self.is_or_has_generic_conditional(target) {
            return false;
        }
        if self.compiler_options.no_check.is_true() {
            return false;
        }
        if self.elaborate_did_you_mean_to_call_or_construct(
            node,
            source,
            target,
            relation,
            SignatureKind::CONSTRUCT,
            head_message,
            diagnostic_output.as_deref_mut(),
        ) || self.elaborate_did_you_mean_to_call_or_construct(
            node,
            source,
            target,
            relation,
            SignatureKind::CALL,
            head_message,
            diagnostic_output.as_deref_mut(),
        ) {
            return true;
        }
        match node.kind() {
            SyntaxKind::AsExpression
            | SyntaxKind::JsxExpression
            | SyntaxKind::ParenthesizedExpression => {
                // Go: `case KindAsExpression: if !IsConstAssertion(node) { break }; fallthrough`
                if !(node.kind() == SyntaxKind::AsExpression && !is_const_assertion(node)) {
                    return self.elaborate_error(
                        node.expression(),
                        source,
                        target,
                        relation,
                        head_message,
                        diagnostic_output,
                    );
                }
            }
            SyntaxKind::BinaryExpression => match node.operator_token().kind() {
                SyntaxKind::EqualsToken | SyntaxKind::CommaToken => {
                    return self.elaborate_error(
                        node.right(),
                        source,
                        target,
                        relation,
                        head_message,
                        diagnostic_output,
                    );
                }
                _ => {}
            },
            SyntaxKind::ObjectLiteralExpression => {
                return self.elaborate_object_literal(
                    node,
                    source,
                    target,
                    relation,
                    diagnostic_output,
                );
            }
            SyntaxKind::ArrayLiteralExpression => {
                return self.elaborate_array_literal(
                    node,
                    source,
                    target,
                    relation,
                    diagnostic_output,
                );
            }
            SyntaxKind::ArrowFunction => {
                return self.elaborate_arrow_function(
                    node,
                    source,
                    target,
                    relation,
                    diagnostic_output,
                );
            }
            SyntaxKind::JsxAttributes => {
                return self.elaborate_jsx_components(
                    node,
                    source,
                    target,
                    relation,
                    diagnostic_output,
                );
            }
            _ => {}
        }
        false
    }

    // Go: checker/relater.go:474 isOrHasGenericConditional
    pub fn is_or_has_generic_conditional(&self, t: TypeId) -> bool {
        self.ty(t).flags.intersects(TypeFlags::CONDITIONAL)
            || (self.ty(t).flags.intersects(TypeFlags::INTERSECTION)
                && self
                    .ty(t)
                    .types()
                    .iter()
                    .any(|&t| self.is_or_has_generic_conditional(t)))
    }

    // Go: checker/relater.go:478 elaborateDidYouMeanToCallOrConstruct
    pub fn elaborate_did_you_mean_to_call_or_construct(
        &mut self,
        node: Node,
        source: TypeId,
        target: TypeId,
        relation: &Rc<RefCell<Relation>>,
        kind: SignatureKind,
        head_message: Option<&'static Message>,
        diagnostic_output: Option<&mut Vec<Diagnostic>>,
    ) -> bool {
        let mut some = false;
        for s in self.get_signatures_of_type(source, kind) {
            let return_type = self.get_return_type_of_signature(s);
            if !self
                .ty(return_type)
                .flags
                .intersects(TypeFlags::ANY | TypeFlags::NEVER)
                && self.check_type_related_to(
                    return_type,
                    target,
                    relation,
                    Node::NIL, /*errorNode*/
                )
            {
                some = true;
                break;
            }
        }
        if some {
            let mut diags: Vec<Diagnostic> = Vec::new();
            if !self.check_type_related_to_ex(
                source,
                target,
                relation,
                node,
                head_message,
                Some(&mut diags),
            ) {
                let mut diagnostic = diags.swap_remove(0);
                let message = if kind == SignatureKind::CONSTRUCT {
                    diag::Did_you_mean_to_use_new_with_this_expression
                } else {
                    diag::Did_you_mean_to_call_this_expression
                };
                diagnostic.add_related_info(Some(create_diagnostic_for_node(
                    node,
                    message,
                    args![],
                )));
                self.report_diagnostic(diagnostic, diagnostic_output);
                return true;
            }
        }
        false
    }

    // Go: checker/relater.go:496 elaborateObjectLiteral
    pub fn elaborate_object_literal(
        &mut self,
        node: Node,
        source: TypeId,
        target: TypeId,
        relation: &Rc<RefCell<Relation>>,
        mut diagnostic_output: Option<&mut Vec<Diagnostic>>,
    ) -> bool {
        if self
            .ty(target)
            .flags
            .intersects(TypeFlags::PRIMITIVE | TypeFlags::NEVER)
        {
            return false;
        }
        let mut reported_error = false;
        for prop in node.properties() {
            if is_spread_assignment(prop) {
                continue;
            }
            let prop_symbol = self.get_symbol_of_declaration(prop);
            let name_type = self.get_literal_type_from_property(
                prop_symbol,
                TypeFlags::STRING_OR_NUMBER_LITERAL_OR_UNIQUE,
                false,
            );
            if name_type.is_nil() || self.ty(name_type).flags.intersects(TypeFlags::NEVER) {
                continue;
            }
            match prop.kind() {
                SyntaxKind::SetAccessor
                | SyntaxKind::GetAccessor
                | SyntaxKind::MethodDeclaration
                | SyntaxKind::ShorthandPropertyAssignment => {
                    reported_error = self.elaborate_element(
                        source,
                        target,
                        relation,
                        prop.name(),
                        Node::NIL,
                        name_type,
                        None,
                        None,
                        diagnostic_output.as_deref_mut(),
                    ) || reported_error;
                }
                SyntaxKind::PropertyAssignment => {
                    let message = if is_computed_non_literal_name(prop.name()) {
                        Some(diag::Type_of_computed_property_s_value_is_0_which_is_not_assignable_to_type_1)
                    } else {
                        None
                    };
                    reported_error = self.elaborate_element(
                        source,
                        target,
                        relation,
                        prop.name(),
                        prop.initializer(),
                        name_type,
                        message,
                        None,
                        diagnostic_output.as_deref_mut(),
                    ) || reported_error;
                }
                _ => {}
            }
        }
        reported_error
    }

    // Go: checker/relater.go:520 elaborateArrayLiteral
    pub fn elaborate_array_literal(
        &mut self,
        node: Node,
        source: TypeId,
        target: TypeId,
        relation: &Rc<RefCell<Relation>>,
        mut diagnostic_output: Option<&mut Vec<Diagnostic>>,
    ) -> bool {
        let mut source = source;
        if self
            .ty(target)
            .flags
            .intersects(TypeFlags::PRIMITIVE | TypeFlags::NEVER)
        {
            return false;
        }
        if !self.is_tuple_like_type(source) {
            self.push_contextual_type(node, target, false /*isCache*/);
            source = self.check_array_literal(node, CheckMode::CONTEXTUAL | CheckMode::FORCE_TUPLE);
            self.pop_contextual_type();
            if !self.is_tuple_like_type(source) {
                return false;
            }
        }
        let mut reported_error = false;
        for (i, element) in node.elements().iter().enumerate() {
            if is_omitted_expression(element)
                || self.is_tuple_like_type(target)
                    && self
                        .get_property_of_type(target, &crate::jsnum::Number(i as f64).to_string())
                        .is_nil()
            {
                continue;
            }
            let name_type = self.get_number_literal_type(crate::jsnum::Number(i as f64));
            let check_node = self.get_effective_check_node(element);
            reported_error = self.elaborate_element(
                source,
                target,
                relation,
                check_node,
                check_node,
                name_type,
                None,
                None,
                diagnostic_output.as_deref_mut(),
            ) || reported_error;
        }
        reported_error
    }

    // Go: checker/relater.go:544 elaborateElement
    // PORT: Go `diagnosticFactory func(prop *ast.Node) *ast.Diagnostic` (nil-able)
    // is `Option<&mut dyn FnMut(&mut Checker, Node) -> Diagnostic>`.
    pub fn elaborate_element(
        &mut self,
        source: TypeId,
        target: TypeId,
        relation: &Rc<RefCell<Relation>>,
        prop: Node,
        next: Node,
        name_type: TypeId,
        error_message: Option<&'static Message>,
        diagnostic_factory: Option<&mut dyn FnMut(&mut Checker, Node) -> Diagnostic>,
        mut diagnostic_output: Option<&mut Vec<Diagnostic>>,
    ) -> bool {
        let mut target_prop_type =
            self.get_best_match_indexed_access_type_or_undefined(source, target, name_type);
        if target_prop_type.is_nil()
            || self
                .ty(target_prop_type)
                .flags
                .intersects(TypeFlags::INDEXED_ACCESS)
        {
            // Don't elaborate on indexes on generic variables
            return false;
        }
        let mut source_prop_type = self.get_indexed_access_type_or_undefined(
            source,
            name_type,
            AccessFlags::NONE,
            Node::NIL,
            None,
        );
        if source_prop_type.is_nil()
            || self.check_type_related_to(
                source_prop_type,
                target_prop_type,
                relation,
                Node::NIL, /*errorNode*/
            )
        {
            // Don't elaborate on indexes on generic variables or when types match
            return false;
        }
        if next.is_some()
            && self.elaborate_error(
                next,
                source_prop_type,
                target_prop_type,
                relation,
                None, /*headMessage*/
                diagnostic_output.as_deref_mut(),
            )
        {
            return true;
        }
        // Issue error on the prop itself, since the prop couldn't elaborate the error
        let mut diags: Vec<Diagnostic> = Vec::new();
        // Use the expression type, if available
        let mut specific_source = source_prop_type;
        if next.is_some() {
            specific_source = self
                .check_expression_for_mutable_location_with_contextual_type(next, source_prop_type);
        }
        if let Some(diagnostic_factory) = diagnostic_factory {
            // Use the custom diagnostic factory if provided (e.g., for JSX text children with dynamic error messages)
            diags.push(diagnostic_factory(self, prop));
        } else if self.exact_optional_property_types
            && self.is_exact_optional_property_mismatch(specific_source, target_prop_type)
        {
            let specific_source_string = self.type_to_string(specific_source);
            let target_prop_type_string = self.type_to_string(target_prop_type);
            diags.push(create_diagnostic_for_node(
                prop,
                diag::Type_0_is_not_assignable_to_type_1_with_exactOptionalPropertyTypes_Colon_true_Consider_adding_undefined_to_the_type_of_the_target,
                args![specific_source_string, target_prop_type_string],
            ));
        } else {
            let prop_name =
                self.get_property_name_from_index(name_type, Node::NIL /*accessNode*/);
            let mut target_prop = self.get_property_of_type(target, &prop_name);
            if target_prop.is_nil() {
                target_prop = self.unknown_symbol;
            }
            let target_is_optional = self
                .sym(target_prop)
                .flags
                .intersects(SymbolFlags::OPTIONAL);
            let mut source_prop = self.get_property_of_type(source, &prop_name);
            if source_prop.is_nil() {
                source_prop = self.unknown_symbol;
            }
            let source_is_optional = self
                .sym(source_prop)
                .flags
                .intersects(SymbolFlags::OPTIONAL);
            target_prop_type = self.remove_missing_type(target_prop_type, target_is_optional);
            source_prop_type = self
                .remove_missing_type(source_prop_type, target_is_optional && source_is_optional);
            let result = self.check_type_related_to_ex(
                specific_source,
                target_prop_type,
                relation,
                prop,
                error_message,
                Some(&mut diags),
            );
            if result && specific_source != source_prop_type {
                // If for whatever reason the expression type doesn't yield an error, make sure we still issue an error on the sourcePropType
                self.check_type_related_to_ex(
                    source_prop_type,
                    target_prop_type,
                    relation,
                    prop,
                    error_message,
                    Some(&mut diags),
                );
            }
        }
        if diags.is_empty() {
            return false;
        }
        let mut diagnostic = diags.swap_remove(0);
        let mut property_name = String::new();
        let mut target_prop = SymbolId::NIL;
        if self.is_type_usable_as_property_name(name_type) {
            property_name = self.get_property_name_from_type(name_type);
            target_prop = self.get_property_of_type(target, &property_name);
        }
        let mut issued_elaboration = false;
        if target_prop.is_nil() {
            let index_info = self.get_applicable_index_info(target, name_type);
            if index_info.is_some() {
                let declaration = self.index_info(index_info).declaration;
                // PORT: Go `c.program.IsSourceFileDefaultLibrary(file.Path())` is a
                // Program method, ported as a free function; `Path()` reads the
                // `SourceFileInfo.path` field.
                if declaration.is_some()
                    && !is_source_file_default_library(
                        &source_file_info(get_source_file_of_node(declaration)).path,
                    )
                {
                    issued_elaboration = true;
                    diagnostic.add_related_info(Some(create_diagnostic_for_node(
                        declaration,
                        diag::The_expected_type_comes_from_this_index_signature,
                        args![],
                    )));
                }
            }
        }
        let target_symbol = self.ty(target).symbol;
        if !issued_elaboration
            && (target_prop.is_some() && !self.sym(target_prop).declarations.is_empty()
                || target_symbol.is_some() && !self.sym(target_symbol).declarations.is_empty())
        {
            let target_node =
                if target_prop.is_some() && !self.sym(target_prop).declarations.is_empty() {
                    self.sym(target_prop).declarations[0]
                } else {
                    self.sym(target_symbol).declarations[0]
                };
            if property_name.is_empty()
                || self
                    .ty(name_type)
                    .flags
                    .intersects(TypeFlags::UNIQUE_ES_SYMBOL)
            {
                property_name = self.type_to_string(name_type);
            }
            if !is_source_file_default_library(
                &source_file_info(get_source_file_of_node(target_node)).path,
            ) {
                let target_string = self.type_to_string(target);
                diagnostic.add_related_info(Some(create_diagnostic_for_node(
                    target_node,
                    diag::The_expected_type_comes_from_property_0_which_is_declared_here_on_type_1,
                    args![property_name, target_string],
                )));
            }
        }
        self.report_diagnostic(diagnostic, diagnostic_output);
        true
    }

    // Go: checker/relater.go:618 getBestMatchIndexedAccessTypeOrUndefined
    pub fn get_best_match_indexed_access_type_or_undefined(
        &mut self,
        source: TypeId,
        target: TypeId,
        name_type: TypeId,
    ) -> TypeId {
        let idx = self.get_indexed_access_type_or_undefined(
            target,
            name_type,
            AccessFlags::NONE,
            Node::NIL,
            None,
        );
        if idx.is_some() {
            return idx;
        }
        if self.ty(target).flags.intersects(TypeFlags::UNION) {
            let best = self.get_best_matching_type(
                source,
                target,
                &mut |c: &mut Checker, s: TypeId, t: TypeId| {
                    c.compare_types_assignable_simple(s, t)
                },
            );
            if best.is_some() {
                return self.get_indexed_access_type_or_undefined(
                    best,
                    name_type,
                    AccessFlags::NONE,
                    Node::NIL,
                    None,
                );
            }
        }
        TypeId::NIL
    }

    // Go: checker/relater.go:632 checkExpressionForMutableLocationWithContextualType
    pub fn check_expression_for_mutable_location_with_contextual_type(
        &mut self,
        next: Node,
        source_prop_type: TypeId,
    ) -> TypeId {
        self.push_contextual_type(next, source_prop_type, false /*isCache*/);
        let result = self.check_expression_for_mutable_location(next, CheckMode::CONTEXTUAL);
        self.pop_contextual_type();
        result
    }

    // Go: checker/relater.go:639 elaborateArrowFunction
    pub fn elaborate_arrow_function(
        &mut self,
        node: Node,
        source: TypeId,
        target: TypeId,
        relation: &Rc<RefCell<Relation>>,
        mut diagnostic_output: Option<&mut Vec<Diagnostic>>,
    ) -> bool {
        // Don't elaborate blocks or functions with annotated parameter types
        if is_block(node.body()) || node.parameters().iter().any(|p| has_type(p)) {
            return false;
        }
        let source_sig = self.get_single_call_signature(source);
        if source_sig.is_nil() {
            return false;
        }
        let target_signatures = self.get_signatures_of_type(target, SignatureKind::CALL);
        if target_signatures.is_empty() {
            return false;
        }
        let return_expression = node.body();
        let source_return = self.get_return_type_of_signature(source_sig);
        let mut target_return_types: Vec<TypeId> = Vec::with_capacity(target_signatures.len());
        for &sig in &target_signatures {
            target_return_types.push(self.get_return_type_of_signature(sig));
        }
        let target_return = self.get_union_type(&target_return_types);
        if self.check_type_related_to(
            source_return,
            target_return,
            relation,
            Node::NIL, /*errorNode*/
        ) {
            return false;
        }
        if return_expression.is_some()
            && self.elaborate_error(
                return_expression,
                source_return,
                target_return,
                relation,
                None, /*headMessage*/
                diagnostic_output.as_deref_mut(),
            )
        {
            return true;
        }
        let mut diags: Vec<Diagnostic> = Vec::new();
        self.check_type_related_to_ex(
            source_return,
            target_return,
            relation,
            return_expression,
            None, /*headMessage*/
            Some(&mut diags),
        );
        if !diags.is_empty() {
            let mut diagnostic = diags.swap_remove(0);
            let target_symbol = self.ty(target).symbol;
            if target_symbol.is_some() && !self.sym(target_symbol).declarations.is_empty() {
                let declaration = self.sym(target_symbol).declarations[0];
                diagnostic.add_related_info(Some(create_diagnostic_for_node(
                    declaration,
                    diag::The_expected_type_comes_from_the_return_type_of_this_signature,
                    args![],
                )));
            }
            if !get_function_flags(node).intersects(FunctionFlags::ASYNC)
                && self
                    .get_type_of_property_of_type(source_return, "then")
                    .is_nil()
            {
                let promise_type = self.create_promise_type(source_return);
                if self.check_type_related_to(
                    promise_type,
                    target_return,
                    relation,
                    Node::NIL, /*errorNode*/
                ) {
                    diagnostic.add_related_info(Some(create_diagnostic_for_node(
                        node,
                        diag::Did_you_mean_to_mark_this_function_as_async,
                        args![],
                    )));
                }
            }
            self.report_diagnostic(diagnostic, diagnostic_output);
            return true;
        }
        false
    }

    // Go: checker/relater.go:679 isWeakType
    // A type is 'weak' if it is an object type with at least one optional property
    // and no required properties, call/construct signatures or index signatures
    pub fn is_weak_type(&mut self, t: TypeId) -> bool {
        if self.ty(t).flags.intersects(TypeFlags::OBJECT) {
            // lazymem1: with the switch on, the Go body of #64475
            // (lazy_members.rs). PERF: a resolved type does not test it.
            if !self
                .ty(t)
                .object_flags
                .intersects(ObjectFlags::MEMBERS_RESOLVED)
                && self.lazy_members
            {
                return self.is_weak_object_type_lazy(t);
            }
            // PORT: the resolved members are read in place, not copied.
            self.resolve_structured_type_members(t);
            let resolved = self.ty(t).as_structured_type();
            return resolved.signatures().is_empty()
                && resolved.index_infos().is_empty()
                && !resolved.properties.is_empty()
                && resolved
                    .properties
                    .iter()
                    .all(|&p| self.sym(p).flags.intersects(SymbolFlags::OPTIONAL));
        }
        if self.ty(t).flags.intersects(TypeFlags::SUBSTITUTION) {
            let base_type = self.ty(t).as_substitution_type().base_type;
            return self.is_weak_type(base_type);
        }
        if self.ty(t).flags.intersects(TypeFlags::INTERSECTION) {
            for i in 0..self.ty(t).types().len() {
                if !self.is_weak_type(self.type_at(t, i)) {
                    return false;
                }
            }
            return true;
        }
        false
    }

    // Go: checker/relater.go:695 hasCommonProperties
    pub fn has_common_properties(
        &mut self,
        source: TypeId,
        target: TypeId,
        is_comparing_jsx_attributes: bool,
    ) -> bool {
        for prop in self.get_properties_of_type(source) {
            let name = self.sym(prop).name.clone();
            if self.is_known_property(target, &name, is_comparing_jsx_attributes) {
                return true;
            }
        }
        false
    }

    // Go: checker/relater.go:717 isKnownProperty
    /**
     * Check if a property with the given name is known anywhere in the given type. In an object type, a property
     * is considered known if
     * 1. the object type is empty and the check is for assignability, or
     * 2. if the object type has index signatures, or
     * 3. if the property is actually declared in the object type
     *    (this means that 'toString', for example, is not usually a known property).
     * 4. In a union or intersection type,
     *    a property is considered known if it is known in any constituent type.
     * @param targetType a type to search a given name in
     * @param name a property name to search
     * @param isComparingJsxAttributes a boolean flag indicating whether we are searching in JsxAttributesType
     */
    pub fn is_known_property(
        &mut self,
        target_type: TypeId,
        name: &str,
        is_comparing_jsx_attributes: bool,
    ) -> bool {
        if self.ty(target_type).flags.intersects(TypeFlags::OBJECT) {
            // For backwards compatibility a symbol-named property is satisfied by a string index signature. This
            // is incorrect and inconsistent with element access expressions, where it is an error, so eventually
            // we should remove this exception.
            if self
                .get_property_of_object_type(target_type, name)
                .is_some()
                || self
                    .get_applicable_index_info_for_name(target_type, name)
                    .is_some()
                || is_late_bound_name(name) && {
                    let string_type = self.string_type;
                    self.get_index_info_of_type(target_type, string_type)
                        .is_some()
                }
                || is_comparing_jsx_attributes && is_hyphenated_jsx_name(name)
            {
                // For JSXAttributes, if the attribute has a hyphenated name, consider that the attribute to be known.
                return true;
            }
        }
        if self
            .ty(target_type)
            .flags
            .intersects(TypeFlags::SUBSTITUTION)
        {
            let base_type = self.ty(target_type).as_substitution_type().base_type;
            return self.is_known_property(base_type, name, is_comparing_jsx_attributes);
        }
        if self
            .ty(target_type)
            .flags
            .intersects(TypeFlags::UNION_OR_INTERSECTION)
            && self.is_excess_property_check_target(target_type)
        {
            for i in 0..self.ty(target_type).types().len() {
                let t = self.type_at(target_type, i);
                if self.is_known_property(t, name, is_comparing_jsx_attributes) {
                    return true;
                }
            }
        }
        false
    }

    // Go: checker/relater.go:747 isExcessPropertyCheckTarget
    pub fn is_excess_property_check_target(&self, t: TypeId) -> bool {
        let ty = self.ty(t);
        ty.flags.intersects(TypeFlags::OBJECT)
            && !ty
                .object_flags
                .intersects(ObjectFlags::OBJECT_LITERAL_PATTERN_WITH_COMPUTED_PROPERTIES)
            || ty.flags.intersects(TypeFlags::NON_PRIMITIVE)
            || ty.flags.intersects(TypeFlags::SUBSTITUTION)
                && self.is_excess_property_check_target(ty.as_substitution_type().base_type)
            || ty.flags.intersects(TypeFlags::UNION)
                && ty
                    .types()
                    .iter()
                    .any(|&t| self.is_excess_property_check_target(t))
            || ty.flags.intersects(TypeFlags::INTERSECTION)
                && ty
                    .types()
                    .iter()
                    .all(|&t| self.is_excess_property_check_target(t))
    }

    // Go: checker/relater.go:766 isDeeplyNestedType
    // Return true if the given type is deeply nested. We consider this to be the case when the given stack contains
    // maxDepth or more occurrences of types with the same recursion identity as the given type. The recursion identity
    // provides a shared identity for type instantiations that repeat in some (possibly infinite) pattern. For example,
    // in `type Deep<T> = { next: Deep<Deep<T>> }`, repeatedly referencing the `next` property leads to an infinite
    // sequence of ever deeper instantiations with the same recursion identity (in this case the symbol associated with
    // the object type literal).
    // A homomorphic mapped type is considered deeply nested if its target type is deeply nested, and an intersection is
    // considered deeply nested if any constituent of the intersection is deeply nested.
    // It is possible, though highly unlikely, for the deeply nested check to be true in a situation where a chain of
    // instantiations is not infinitely expanding. Effectively, we will generate a false positive when two types are
    // structurally equal to at least maxDepth levels, but unequal at some level beyond that.
    pub fn is_deeply_nested_type(&mut self, t: TypeId, stack: &[TypeId], max_depth: i32) -> bool {
        if stack.len() as i32 >= max_depth {
            let target = self.get_recursion_identity_target(t);
            if self.ty(target).flags.intersects(TypeFlags::INTERSECTION) {
                for i in 0..self.ty(target).types().len() {
                    if self.is_deeply_nested_type(self.type_at(target, i), stack, max_depth) {
                        return true;
                    }
                }
            } else {
                let identity = self.get_recursion_identity_from_target(target);
                let mut count: i32 = 0;
                let mut last_type_id = TypeId(0);
                for &t in stack {
                    if self.has_matching_recursion_identity(t, identity) {
                        // We only count occurrences with a higher type id than the previous occurrence, since higher
                        // type ids are an indicator of newer instantiations caused by recursion.
                        let id = t;
                        if id >= last_type_id {
                            count += 1;
                            if count >= max_depth {
                                return true;
                            }
                        }
                        last_type_id = id;
                    }
                }
            }
        }
        false
    }

    // PORT: perf. The recursion id kept for each entry of the inference
    // stacks (`invoke_once`) and the relater stacks
    // (`is_deeply_nested_relater_type`). `None` marks an entry that still needs
    // `has_matching_recursion_identity`: an instantiated mapped type (its
    // mapped target can make types), an indexed access (its object type can
    // be a mapped type) or an intersection (its parts can be mapped types).
    // Any other type is its own `get_recursion_identity_target`, so that
    // function is `get_recursion_identity_from_target(t) == identity`, which
    // reads only type data that is fixed at type creation.
    pub fn stack_recursion_id(&self, t: TypeId) -> Option<RecursionId> {
        let ty = self.ty(t);
        if ty.object_flags.contains(ObjectFlags::INSTANTIATED_MAPPED)
            || ty
                .flags
                .intersects(TypeFlags::INTERSECTION | TypeFlags::INDEXED_ACCESS)
        {
            return None;
        }
        Some(self.get_recursion_identity_from_target(t))
    }

    // PORT: perf. `is_deeply_nested_type` for stacks that keep recursion
    // ids (the inference stacks and, through
    // `is_deeply_nested_relater_type`, the relater stacks). `ids[i]` is
    // `stack_recursion_id(stack[i])`. An entry with an id is compared
    // directly. `probe_id` is `stack_recursion_id(t)` when the caller has it,
    // else `None`. A probe with an id is its own recursion identity target
    // and not an intersection, so the Go code would only compute that same
    // id. The other probes and the `None` entries run the Go code, so
    // `get_recursion_identity_target` runs for the same types in Go order.
    // `ids` holds `Option<RecursionId>` (relater stacks) or `RecursionKey`
    // (inference stacks, which convert to the same `Option<RecursionId>`).
    // Go: checker/relater.go:766 isDeeplyNestedType
    pub fn is_deeply_nested_type_with_ids<I: Copy + Into<Option<RecursionId>>>(
        &mut self,
        t: TypeId,
        probe_id: Option<RecursionId>,
        stack: &[TypeId],
        ids: &[I],
        max_depth: i32,
    ) -> bool {
        debug_assert_eq!(stack.len(), ids.len());
        debug_assert!(probe_id.is_none() || probe_id == self.stack_recursion_id(t));
        if stack.len() as i32 >= max_depth {
            let identity = match probe_id {
                Some(id) => id,
                None => {
                    let target = self.get_recursion_identity_target(t);
                    if self.ty(target).flags.intersects(TypeFlags::INTERSECTION) {
                        for i in 0..self.ty(target).types().len() {
                            if self.is_deeply_nested_type_with_ids(
                                self.type_at(target, i),
                                None,
                                stack,
                                ids,
                                max_depth,
                            ) {
                                return true;
                            }
                        }
                        return false;
                    }
                    self.get_recursion_identity_from_target(target)
                }
            };
            let mut count: i32 = 0;
            let mut last_type_id = TypeId(0);
            for (&t, &id) in stack.iter().zip(ids) {
                let id: Option<RecursionId> = id.into();
                let matching = match id {
                    Some(id) => id == identity,
                    None => self.has_matching_recursion_identity(t, identity),
                };
                if matching {
                    // We only count occurrences with a higher type id than the previous occurrence, since higher
                    // type ids are an indicator of newer instantiations caused by recursion.
                    let id = t;
                    if id >= last_type_id {
                        count += 1;
                        if count >= max_depth {
                            return true;
                        }
                    }
                    last_type_id = id;
                }
            }
        }
        false
    }

    // PORT: perf. `is_deeply_nested_type` for the relater stacks. `ids`
    // holds `stack_recursion_id` for a prefix of `stack`. It is filled up to
    // the full stack only when the check reaches the scan, and the caller
    // truncates it when the stack pops. The ids read only type data fixed
    // at type creation, so a late fill gives the same ids.
    // Go: checker/relater.go:766 isDeeplyNestedType
    pub fn is_deeply_nested_relater_type(
        &mut self,
        t: TypeId,
        stack: &[TypeId],
        ids: &mut Vec<Option<RecursionId>>,
        max_depth: i32,
    ) -> bool {
        if (stack.len() as i32) < max_depth {
            return false;
        }
        debug_assert!(ids.len() <= stack.len());
        for &s in &stack[ids.len()..] {
            ids.push(self.stack_recursion_id(s));
        }
        // The probe is usually the entry just pushed, whose id is known.
        let probe_id = match (stack.last(), ids.last()) {
            (Some(&top), Some(&id)) if top == t => id,
            _ => self.stack_recursion_id(t),
        };
        self.is_deeply_nested_type_with_ids(t, probe_id, stack, ids, max_depth)
    }

    // Go: checker/relater.go:797 hasMatchingRecursionIdentity
    // PORT: Go package functions `hasMatchingRecursionIdentity`,
    // `getRecursionIdentity` and `getRecursionIdentityTarget` reach the
    // checker through `t.checker`, so they are Checker methods here.
    pub fn has_matching_recursion_identity(&mut self, t: TypeId, identity: RecursionId) -> bool {
        let target = self.get_recursion_identity_target(t);
        if self.ty(target).flags.intersects(TypeFlags::INTERSECTION) {
            for i in 0..self.ty(target).types().len() {
                if self.has_matching_recursion_identity(self.type_at(target, i), identity) {
                    return true;
                }
            }
            return false;
        }
        self.get_recursion_identity_from_target(target) == identity
    }

    // Go: checker/relater.go:810 getRecursionIdentity
    pub fn get_recursion_identity(&mut self, t: TypeId) -> RecursionId {
        let target = self.get_recursion_identity_target(t);
        self.get_recursion_identity_from_target(target)
    }

    // Go: checker/relater.go:820 getRecursionIdentityTarget
    // Get the recursion identity target type from a type. Recursively (a) obtain the target object type of an
    // indexed access (i.e. the T in T[K]), and (b) unwrap nested homomorphic mapped types and return the deepest
    // target type that has a symbol. The unwrapping better preserves unique type identities for mapped types applied
    // to explicitly written object literals. For example in `Mapped<{ x: Mapped<{ x: Mapped<{ x: string }>}>}>`,
    // each of the mapped type applications will have a unique recursion identity (that of their target object type
    // literal) and thus avoid appearing deeply nested.
    pub fn get_recursion_identity_target(&mut self, t: TypeId) -> TypeId {
        if self.ty(t).flags.intersects(TypeFlags::INDEXED_ACCESS) {
            let object_type = self.ty(t).as_indexed_access_type().object_type;
            return self.get_recursion_identity_target(object_type);
        }
        if self
            .ty(t)
            .object_flags
            .contains(ObjectFlags::INSTANTIATED_MAPPED)
        {
            let target = self.get_modifiers_type_from_mapped_type(t);
            if target.is_some()
                && (self.ty(target).symbol.is_some()
                    || self.ty(target).flags.intersects(TypeFlags::INTERSECTION)
                        && self
                            .ty(target)
                            .types()
                            .iter()
                            .any(|&t| self.ty(t).symbol.is_some()))
            {
                return self.get_recursion_identity_target(target);
            }
        }
        t
    }

    // Go: checker/relater.go:840 getRecursionIdentityFromTarget
    // The recursion identity of a type is an object identity that is shared among multiple instantiations of the type.
    // We track recursion identities in order to identify deeply nested and possibly infinite type instantiations with
    // the same origin. For example, when type parameters are in scope in an object type such as { x: T }, all
    // instantiations of that type have the same recursion identity. The default recursion identity is the object
    // identity of the type, meaning that every type is unique. Generally, types with constituents that could circularly
    // reference the type have a recursion identity that differs from the object identity.
    pub fn get_recursion_identity_from_target(&self, t: TypeId) -> RecursionId {
        let flags = self.ty(t).flags;
        let object_flags = self.ty(t).object_flags;
        let symbol = self.ty(t).symbol;
        // Object and array literals are known not to contain recursive references and don't need a recursion identity.
        if flags.intersects(TypeFlags::OBJECT) && !self.is_object_or_array_literal_type(t) {
            if object_flags.intersects(ObjectFlags::REFERENCE)
                && self.ty(t).as_type_reference().node.is_some()
            {
                // Deferred type references are tracked through their associated AST node. This gives us finer
                // granularity than using their associated target because each manifest type reference has a
                // unique AST node.
                return as_recursion_id(self.ty(t).as_type_reference().node);
            }
            if symbol.is_some()
                && !(object_flags.intersects(ObjectFlags::ANONYMOUS)
                    && self.sym(symbol).flags.intersects(SymbolFlags::CLASS))
                && !object_flags.intersects(ObjectFlags::FROM_TYPE_NODE)
            {
                // We track object types that have a symbol by that symbol (representing the origin of the type), but
                // exclude the static sides of classes (since they share their symbols with the instance sides) and type
                // references that originate in resolution of AST type nodes (since such type nodes cannot be the source
                // of generative recursion without first being instantiated).
                return as_recursion_id(symbol);
            }
            if self.is_tuple_type(t) && !object_flags.intersects(ObjectFlags::FROM_TYPE_NODE) {
                return as_recursion_id(self.ty(t).target());
            }
        }
        if flags.intersects(TypeFlags::TYPE_PARAMETER) && symbol.is_some() {
            // We use the symbol of the type parameter such that all "fresh" instantiations of that type parameter
            // have the same recursion identity.
            return as_recursion_id(symbol);
        }
        if flags.intersects(TypeFlags::CONDITIONAL) {
            // The root object represents the origin of the conditional type
            let node = self.ty(t).as_conditional_type().root.borrow().node;
            return as_recursion_id(node);
        }
        as_recursion_id(t)
    }

    // Go: checker/relater.go:872 getBestMatchingType
    pub fn get_best_matching_type(
        &mut self,
        source: TypeId,
        target: TypeId,
        is_related_to: &mut dyn FnMut(&mut Checker, TypeId, TypeId) -> Ternary,
    ) -> TypeId {
        let t = self.find_matching_discriminant_type(source, target, is_related_to);
        if t.is_some() {
            return t;
        }
        let t = self.find_matching_type_reference_or_type_alias_reference(source, target);
        if t.is_some() {
            return t;
        }
        let t = self.find_best_type_for_object_literal(source, target);
        if t.is_some() {
            return t;
        }
        let t = self.find_best_type_for_invokable(source, target, SignatureKind::CALL);
        if t.is_some() {
            return t;
        }
        let t = self.find_best_type_for_invokable(source, target, SignatureKind::CONSTRUCT);
        if t.is_some() {
            return t;
        }
        self.find_most_overlappy_type(source, target)
    }

    // Go: checker/relater.go:891 findMatchingTypeReferenceOrTypeAliasReference
    pub fn find_matching_type_reference_or_type_alias_reference(
        &self,
        source: TypeId,
        union_target: TypeId,
    ) -> TypeId {
        let source_object_flags = self.ty(source).object_flags;
        if source_object_flags.intersects(ObjectFlags::REFERENCE | ObjectFlags::ANONYMOUS)
            && self.ty(union_target).flags.intersects(TypeFlags::UNION)
        {
            for &target in self.ty(union_target).types() {
                if self.ty(target).flags.intersects(TypeFlags::OBJECT) {
                    let overlap_obj_flags = source_object_flags & self.ty(target).object_flags;
                    if overlap_obj_flags.intersects(ObjectFlags::REFERENCE)
                        && self.ty(source).target() == self.ty(target).target()
                    {
                        return target;
                    }
                    if overlap_obj_flags.intersects(ObjectFlags::ANONYMOUS) {
                        if let (Some(source_alias), Some(target_alias)) =
                            (&self.ty(source).alias, &self.ty(target).alias)
                        {
                            if source_alias.symbol == target_alias.symbol {
                                return target;
                            }
                        }
                    }
                }
            }
        }
        TypeId::NIL
    }
}

// Go: checker/relater.go:743 isHyphenatedJsxName
pub fn is_hyphenated_jsx_name(name: &str) -> bool {
    name.contains('-')
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `Relation` against a `HashMap`, with plain keys that pack, plain keys
    /// that do not, and generic keys, so all three tables grow and overwrite.
    #[test]
    fn relation_matches_hash_map() {
        let mut relation = Relation::default();
        let mut want = std::collections::HashMap::new();
        let mut rng: u64 = 0x9e37_79b9_7f4a_7c15;
        let mut next = || {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            rng
        };
        let results = [
            RelationComparisonResult::SUCCEEDED,
            RelationComparisonResult::FAILED,
            RelationComparisonResult::FAILED | RelationComparisonResult::COMPLEXITY_OVERFLOW,
            RelationComparisonResult::SUCCEEDED | RelationComparisonResult::REPORTS_MASK,
        ];
        for n in 0..20_000u64 {
            let v = next();
            let key = match v % 8 {
                0 => RelationKey::Generic(CacheHashKey {
                    hi: v >> 8 & 0xff,
                    lo: v >> 16 & 0x3ff,
                }),
                1 => RelationKey::Plain(PlainRelationKey {
                    source: (1 << PACKED_ID_BITS) + (v >> 8 & 0x3f) as u32,
                    target: (v >> 16 & 0x3f) as u32,
                    intersection_state: 0,
                }),
                _ => RelationKey::Plain(PlainRelationKey {
                    source: (v >> 8 & 0x3f) as u32,
                    target: (v >> 16 & 0x3f) as u32 | if v % 16 == 2 { 0xfff_ffc0 } else { 0 },
                    intersection_state: (v >> 24 & 3) as u32,
                }),
            };
            if n % 3 == 0 {
                let result = results[(v >> 32) as usize % results.len()];
                relation.set(key, result);
                want.insert(key, result);
                assert_eq!(relation.size() as usize, want.len());
            }
            let got = relation.get(key);
            assert_eq!(got, want.get(&key).copied().unwrap_or_default());
        }
    }
}
