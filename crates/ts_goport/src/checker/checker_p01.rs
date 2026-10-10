//! Port of `checker/checker.go` lines 1-1115: checker-wide key and state
//! types, `Checker` itself (with the arenas from `PORTING.md`), and
//! `NewChecker` as `Checker::new`.
//!
//! PORT: Go consts in this range (`CheckMode`, `TypeSystemPropertyName`,
//! `WideningKind`, `CachedTypeKind`, `InferenceFlags`, `InferencePriority`,
//! `DeclarationMeaning`, `DeclarationSpaces`, `IntrinsicTypeKind`,
//! `MappedTypeModifiers`, `MappedTypeNameTypeKind`, `ReferenceHint`,
//! `TypeFacts`, `IterationUse`, `IterationTypeKind`) are generated in
//! `crate::flags` and are not repeated here.

use crate::diagnostics::Message;
use crate::gostd::Context;
use crate::jsnum::{Number, PseudoBigInt};
use crate::prelude::*;
use std::sync::LazyLock;

// Go: checker/checker.go:52 TypeSystemEntity
// PORT: Go `TypeSystemEntity any`. The Go callers of `pushTypeResolution`
// pass a declaration node, a symbol, a type or a signature, so the Rust type
// is an enum over those four handles. Go compares targets with `==` on the
// interface value, which is handle equality here.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TypeSystemEntity {
    Node(Node),
    Symbol(SymbolId),
    Type(TypeId),
    Signature(SignatureId),
}

impl From<Node> for TypeSystemEntity {
    fn from(node: Node) -> Self {
        TypeSystemEntity::Node(node)
    }
}

impl From<SymbolId> for TypeSystemEntity {
    fn from(symbol: SymbolId) -> Self {
        TypeSystemEntity::Symbol(symbol)
    }
}

impl From<TypeId> for TypeSystemEntity {
    fn from(t: TypeId) -> Self {
        TypeSystemEntity::Type(t)
    }
}

impl From<SignatureId> for TypeSystemEntity {
    fn from(sig: SignatureId) -> Self {
        TypeSystemEntity::Signature(sig)
    }
}

// Go: checker/checker.go:69 TypeResolution
#[derive(Clone, Copy, Debug)]
pub struct TypeResolution {
    pub target: TypeSystemEntity,
    pub property_name: TypeSystemPropertyName,
    pub result: bool,
}

// ContextualInfo

// Go: checker/checker.go:77 ContextualInfo
#[derive(Clone, Copy, Debug, Default)]
pub struct ContextualInfo {
    pub node: Node,
    pub t: TypeId,
    pub is_cache: bool,
}

// InferenceContextInfo

// Go: checker/checker.go:85 InferenceContextInfo
#[derive(Clone, Copy, Debug, Default)]
pub struct InferenceContextInfo {
    pub node: Node,
    pub context: InferenceContextId,
}

// EnumLiteralKey

// Go: checker/checker.go:103 EnumLiteralKey
// PORT: Go `value any` holds a `LiteralValue`. `LiteralValue` is not `Hash`
// (it holds `f64`), so the key stores `LiteralValueKey`, which compares like
// Go interface `==` (numbers as floats, -0 == +0). Build it with
// `LiteralValueKey::from(&value)`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct EnumLiteralKey {
    pub enum_symbol: SymbolId,
    pub value: LiteralValueKey,
}

// PORT: hashable form of `LiteralValue` for Go map keys of type `any`.
// NaN cannot match in Go; Go callers keep NaN out of these keys.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum LiteralValueKey {
    String(String),
    Number(NumberKey),
    Bool(bool),
    PseudoBigInt(PseudoBigIntKey),
}

impl From<&LiteralValue> for LiteralValueKey {
    fn from(v: &LiteralValue) -> Self {
        match v {
            LiteralValue::String(s) => LiteralValueKey::String(s.clone()),
            LiteralValue::Number(n) => LiteralValueKey::Number(NumberKey::from(Number(n.0))),
            LiteralValue::Bool(b) => LiteralValueKey::Bool(*b),
            LiteralValue::PseudoBigInt(p) => {
                LiteralValueKey::PseudoBigInt(PseudoBigIntKey::from(p))
            }
        }
    }
}

// EnumRelationKey

// Go: checker/checker.go:110 EnumRelationKey
// PORT: Go `ast.SymbolId` values are symbol handles here.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct EnumRelationKey {
    pub source_id: SymbolId,
    pub target_id: SymbolId,
}

// CachedTypeKey

// Go: checker/checker.go:146 CachedTypeKey
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct CachedTypeKey {
    pub kind: CachedTypeKind,
    pub type_id: TypeId,
}

// NarrowedTypeKey

// Go: checker/checker.go:153 NarrowedTypeKey
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct NarrowedTypeKey {
    pub t: TypeId,
    pub candidate: TypeId,
    pub assume_true: bool,
    pub check_derived: bool,
}

// UnionOfUnionKey

// Go: checker/checker.go:162 UnionOfUnionKey
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct UnionOfUnionKey {
    pub id1: TypeId,
    pub id2: TypeId,
    pub r: UnionReduction,
    pub a: CacheHashKey,
}

// CachedSignatureKey

// Go: checker/checker.go:171 CachedSignatureKey
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct CachedSignatureKey {
    pub sig: SignatureId,
    /// Type list key or one of the special keys below
    pub key: CacheHashKey,
}

// PORT: Go `CacheHashKey(xxh3.HashString128(s))`. A streaming xxh3 hash of
// the same bytes equals the one-shot hash, so this goes through the Go
// `keyBuilder` port (`KeyBuilder`) that owns the hashing.
fn signature_key_of(s: &str) -> CacheHashKey {
    let mut b = KeyBuilder::default();
    b.write_string(s);
    b.hash()
}

// Go: checker/checker.go:171 SignatureKeyErased
// PORT: Go package vars become lazily computed statics. Read with
// `*SIGNATURE_KEY_ERASED`.
pub static SIGNATURE_KEY_ERASED: LazyLock<CacheHashKey> = LazyLock::new(|| signature_key_of("-"));
// Go: checker/checker.go:172 SignatureKeyCanonical
pub static SIGNATURE_KEY_CANONICAL: LazyLock<CacheHashKey> =
    LazyLock::new(|| signature_key_of("*"));
// Go: checker/checker.go:173 SignatureKeyBase
pub static SIGNATURE_KEY_BASE: LazyLock<CacheHashKey> = LazyLock::new(|| signature_key_of("#"));
// Go: checker/checker.go:174 SignatureKeyInner
pub static SIGNATURE_KEY_INNER: LazyLock<CacheHashKey> = LazyLock::new(|| signature_key_of("<"));
// Go: checker/checker.go:175 SignatureKeyOuter
pub static SIGNATURE_KEY_OUTER: LazyLock<CacheHashKey> = LazyLock::new(|| signature_key_of(">"));

// StringMappingKey

// Go: checker/checker.go:186 StringMappingKey
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct StringMappingKey {
    pub s: SymbolId,
    pub t: TypeId,
}

// AssignmentReducedKey

// Go: checker/checker.go:193 AssignmentReducedKey
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct AssignmentReducedKey {
    pub id1: TypeId,
    pub id2: TypeId,
}

// DiscriminatedContextualTypeKey

// Go: checker/checker.go:200 DiscriminatedContextualTypeKey
// PORT: Go `ast.NodeId` is the node handle here (a `Node` is unique per
// node, like a Go node id).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct DiscriminatedContextualTypeKey {
    pub node_id: Node,
    pub type_id: TypeId,
}

// InstantiationExpressionKey

// Go: checker/checker.go:207 InstantiationExpressionKey
// PORT: Go `ast.NodeId` is the node handle here.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct InstantiationExpressionKey {
    pub node_id: Node,
    pub type_id: TypeId,
}

// SubstitutionTypeKey

// Go: checker/checker.go:214 SubstitutionTypeKey
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct SubstitutionTypeKey {
    pub base_id: TypeId,
    pub constraint_id: TypeId,
}

// ReverseMappedTypeKey

// Go: checker/checker.go:221 ReverseMappedTypeKey
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct ReverseMappedTypeKey {
    pub source_id: TypeId,
    pub target_id: TypeId,
    pub constraint_id: TypeId,
}

// IterationTypesKey

// Go: checker/checker.go:229 IterationTypesKey
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct IterationTypesKey {
    pub type_id: TypeId,
    pub use_: IterationUse,
}

// PropertiesTypesKey

// Go: checker/checker.go:236 PropertiesTypesKey
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct PropertiesTypesKey {
    pub type_id: TypeId,
    pub include: TypeFlags,
    pub include_origin: bool,
}

// NonExistentPropertyKey

// Go: checker/checker.go:244 NonExistentPropertyKey
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct NonExistentPropertyKey {
    pub prop_node: Node,
    pub containing_type: TypeId,
    pub is_unchecked_js: bool,
}

// FlowLoopKey

// Go: checker/checker.go:252 FlowLoopKey
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct FlowLoopKey {
    pub flow_node: FlowNodeId,
    pub ref_key: CacheHashKey,
}

// Go: checker/checker.go:257 FlowLoopInfo
#[derive(Clone)]
pub struct FlowLoopInfo {
    pub key: FlowLoopKey,
    pub types: Vec<TypeId>,
}

// InferenceContext

// Go: checker/checker.go:276 InferenceContext
// PERF (infermem1): 56 bytes (asserted below), not 120. The port keeps every context
// of a run (typebox: about 3.9M, almost all from conditional types with
// `infer`), so the fields that those contexts never set are in `rare`, a box
// that is made on the first write. The accessors give Go's zero values while
// it is absent.
#[derive(Clone)]
pub struct InferenceContext {
    /// Inferences made for each type parameter
    // PERF (infermem1): a boxed slice, 8 bytes less than a `Vec`. The list
    // never changes its length after the context is made.
    pub inferences: Box<[InferenceInfo]>,
    /// Generic signature for which inferences are made (if any)
    pub signature: SignatureId,
    /// Inference flags
    pub flags: InferenceFlags,
    /// Type comparer function
    pub compare_types: TypeComparer,
    /// Mapper that fixes inferences
    pub mapper: MapperId,
    /// Mapper that doesn't fix inferences
    pub non_fixing_mapper: MapperId,
    /// The other Go fields, or `None` while all of them are Go zero values.
    pub rare: Option<Box<InferenceContextRare>>,
}

// 32-bit targets (wasm32) have smaller pointers and do not check this.
#[cfg(target_pointer_width = "64")]
const _: () = assert!(std::mem::size_of::<InferenceContext>() == 56);

/// The fields of Go `InferenceContext` that only signature inference sets.
#[derive(Clone, Default)]
pub struct InferenceContextRare {
    /// Type mapper for inferences from return types (if any)
    pub return_mapper: MapperId,
    /// Type mapper for inferences from return types of outer function (if any)
    pub outer_return_mapper: MapperId,
    /// Inferred type parameters for function result
    pub inferred_type_parameters: Vec<TypeId>,
    // PORT: slice identity of `inferred_type_parameters` (see
    // `Signature::type_parameters_origin`); 0 while the list is empty.
    pub inferred_type_parameters_origin: u32,
    pub intra_expression_inference_sites: Vec<IntraExpressionInferenceSite>,
}

impl InferenceContext {
    /// Go `n.returnMapper`.
    #[must_use]
    pub fn return_mapper(&self) -> MapperId {
        self.rare
            .as_ref()
            .map_or(MapperId::NIL, |r| r.return_mapper)
    }

    /// Go `n.outerReturnMapper`.
    #[must_use]
    pub fn outer_return_mapper(&self) -> MapperId {
        self.rare
            .as_ref()
            .map_or(MapperId::NIL, |r| r.outer_return_mapper)
    }

    /// Go `n.inferredTypeParameters`.
    #[must_use]
    pub fn inferred_type_parameters(&self) -> &[TypeId] {
        self.rare
            .as_ref()
            .map_or(&[], |r| &r.inferred_type_parameters)
    }

    /// The slice identity of `inferred_type_parameters` (0 while empty).
    #[must_use]
    pub fn inferred_type_parameters_origin(&self) -> u32 {
        self.rare
            .as_ref()
            .map_or(0, |r| r.inferred_type_parameters_origin)
    }

    /// Go `n.intraExpressionInferenceSites`.
    #[must_use]
    pub fn intra_expression_inference_sites(&self) -> &[IntraExpressionInferenceSite] {
        self.rare
            .as_ref()
            .map_or(&[], |r| &r.intra_expression_inference_sites)
    }

    /// The rare fields for a write. Makes the box on the first write.
    pub fn rare_mut(&mut self) -> &mut InferenceContextRare {
        self.rare.get_or_insert_default()
    }
}

// PORT: `Default` exists only for the dummy entry at index 0 of
// `Checker::inference_contexts`. Go never has a nil `compareTypes`, so the
// placeholder comparer panics like a call of a nil Go func.
impl Default for InferenceContext {
    fn default() -> Self {
        InferenceContext {
            inferences: Box::default(),
            signature: SignatureId::NIL,
            flags: InferenceFlags::NONE,
            compare_types: nil_type_comparer(),
            mapper: MapperId::NIL,
            non_fixing_mapper: MapperId::NIL,
            rare: None,
        }
    }
}

// Go: checker/checker.go:289 InferenceInfo
// PORT: Go `*InferenceInfo` is an index into `InferenceContext::inferences`.
// PERF (infermem1): 32 bytes (asserted below), not 72. The two candidate
// lists are in one box that the first candidate makes (typebox: about 6M
// infos, under a third of them with candidates). `candidates()` and
// `contra_candidates()` give Go's nil lists while it is absent.
#[derive(Clone, Debug, Default)]
pub struct InferenceInfo {
    /// Type parameter for which inferences are being made
    pub type_parameter: TypeId,
    /// Go `candidates` and `contraCandidates`, or `None` while both are nil.
    pub candidate_lists: Option<Box<InferenceCandidates>>,
    /// Cache for resolved inferred type
    pub inferred_type: TypeId,
    /// Priority of current inference set
    pub priority: InferencePriority,
    /// True if all inferences are to top level occurrences
    pub top_level: bool,
    /// True if inferences are fixed
    pub is_fixed: bool,
    /// Implied arity (or -1)
    pub implied_arity: i32,
}

// 32-bit targets (wasm32) have smaller pointers and do not check this.
#[cfg(target_pointer_width = "64")]
const _: () = assert!(std::mem::size_of::<InferenceInfo>() == 32);

/// The candidate lists of an `InferenceInfo`.
#[derive(Clone, Debug, Default)]
pub struct InferenceCandidates {
    /// Candidates in covariant positions in decreasing depth order
    pub candidates: Vec<TypeId>,
    /// Candidates in contravariant positions
    pub contra_candidates: Vec<TypeId>,
}

impl InferenceInfo {
    /// Go `info.candidates`.
    #[must_use]
    pub fn candidates(&self) -> &[TypeId] {
        self.candidate_lists.as_ref().map_or(&[], |l| &l.candidates)
    }

    /// Go `info.contraCandidates`.
    #[must_use]
    pub fn contra_candidates(&self) -> &[TypeId] {
        self.candidate_lists
            .as_ref()
            .map_or(&[], |l| &l.contra_candidates)
    }

    /// The candidate lists for a write. Makes the box on the first write.
    pub fn candidate_lists_mut(&mut self) -> &mut InferenceCandidates {
        self.candidate_lists.get_or_insert_default()
    }
}

// Go: checker/checker.go:321 IntraExpressionInferenceSite
#[derive(Clone, Copy, Debug, Default)]
pub struct IntraExpressionInferenceSite {
    pub node: Node,
    pub t: TypeId,
}

// Go: checker/checker.go:360 intrinsicTypeKinds
// PORT: Go package map var; read with `INTRINSIC_TYPE_KINDS.get(name)`.
pub static INTRINSIC_TYPE_KINDS: LazyLock<FxHashMap<&'static str, IntrinsicTypeKind>> =
    LazyLock::new(|| {
        let mut m = FxHashMap::default();
        m.insert("Uppercase", IntrinsicTypeKind::UPPERCASE);
        m.insert("Lowercase", IntrinsicTypeKind::LOWERCASE);
        m.insert("Capitalize", IntrinsicTypeKind::CAPITALIZE);
        m.insert("Uncapitalize", IntrinsicTypeKind::UNCAPITALIZE);
        m.insert("NoInfer", IntrinsicTypeKind::NO_INFER);
        m
    });

// Go: checker/checker.go:507 IterationTypes
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct IterationTypes {
    pub yield_type: TypeId,
    pub return_type: TypeId,
    pub next_type: TypeId,
}

// Go: checker/checker.go:521 IterationTypesResolver
// PORT: Go func fields are `Rc<dyn Fn(&mut Checker, ...)>`. Go
// `*diagnostics.Message` fields are always set by
// `initializeIterationResolvers`, so they are plain `&'static Message`.
#[derive(Clone)]
pub struct IterationTypesResolver {
    pub iterator_symbol_name: String,
    pub get_global_iterator_type: GlobalTypeFn,
    pub get_global_iterable_type: GlobalTypeFn,
    pub get_global_iterable_type_checked: GlobalTypeFn,
    pub get_global_iterable_iterator_type: GlobalTypeFn,
    pub get_global_iterable_iterator_type_checked: GlobalTypeFn,
    pub get_global_iterator_object_type: GlobalTypeFn,
    pub get_global_generator_type: GlobalTypeFn,
    pub get_global_builtin_iterator_types: GlobalTypesFn,
    pub resolve_iteration_type: Rc<dyn Fn(&mut Checker, TypeId, Node) -> TypeId>,
    pub must_have_a_next_method_diagnostic: &'static Message,
    pub must_be_a_method_diagnostic: &'static Message,
    pub must_have_a_value_diagnostic: &'static Message,
}

// Go: checker/checker.go:537 WideningContext
// PORT: Go `*WideningContext` links become `Rc<RefCell<WideningContext>>`;
// a nil parent is `None`.
#[derive(Clone, Default)]
pub struct WideningContext {
    /// Parent context
    // PORT: weak, so a parent and its `child_contexts` do not form an `Rc`
    // cycle that leaks (Go's GC frees them). A child is only used while its
    // parent is alive: the widening recursion holds the parent.
    pub parent: Option<std::rc::Weak<RefCell<WideningContext>>>,
    /// Name of property in parent
    pub property_name: String,
    /// Types of siblings
    pub siblings: Vec<TypeId>,
    /// Properties occurring in sibling object literals
    pub resolved_properties: Vec<SymbolId>,
    pub child_contexts: FxHashMap<String, Rc<RefCell<WideningContext>>>,
    pub widened_types: FxHashMap<TypeId, TypeId>,
}

// Go: checker/checker.go:546 VarianceStackEntry
#[derive(Clone, Debug, Default)]
pub struct VarianceStackEntry {
    pub symbol: SymbolId,
    pub type_parameters: Vec<TypeId>,
}

// Go: checker/checker.go:551 maxSerializationLevel
pub const MAX_SERIALIZATION_LEVEL: i32 = 2;

// PORT: Go `Program` and `Host` interfaces (checker.go:555-583) are not
// ported as types. The checker reads the installed program through
// `prog()` and the free functions in `program.rs`.

// PORT: Go `map[jsnum.Number]*Type` key. `crate::jsnum::Number` is not `Hash`.
// Go compares float map keys with `==`, so -0 and +0 are one key. NaN never
// matches in Go; `getNumberLiteralType` caches NaN separately, so NaN never
// reaches this key.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct NumberKey(pub u64);

impl From<Number> for NumberKey {
    fn from(n: Number) -> Self {
        let v = if n.0 == 0.0 { 0.0 } else { n.0 };
        NumberKey(v.to_bits())
    }
}

// PORT: Go `map[jsnum.PseudoBigInt]*Type` key. `crate::jsnum::PseudoBigInt` is
// not `Hash`. Go compares the struct fields, like this key.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct PseudoBigIntKey {
    pub negative: bool,
    pub base10_value: String,
}

impl From<&PseudoBigInt> for PseudoBigIntKey {
    fn from(v: &PseudoBigInt) -> Self {
        PseudoBigIntKey {
            negative: v.negative,
            base10_value: v.base10_value.clone(),
        }
    }
}

impl From<PseudoBigInt> for PseudoBigIntKey {
    fn from(v: PseudoBigInt) -> Self {
        PseudoBigIntKey {
            negative: v.negative,
            base10_value: v.base10_value,
        }
    }
}

// PORT: Go `symbolTableID` (symbolaccessibility.go:402) is used as a map key
// in a `Checker` field, so its Rust type name is fixed here. The owner of
// symbolaccessibility.go defines its constants and constructors.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SymbolTableID(pub u64);

// PORT: type aliases for the Go func-typed `Checker` fields. They are plain
// aliases, so code that spells out the `Rc<dyn Fn ...>` type still matches.
/// Go `func() *Type`.
pub type GlobalTypeFn = Rc<dyn Fn(&mut Checker) -> TypeId>;
/// Go `func() *ast.Symbol`.
pub type GlobalSymbolFn = Rc<dyn Fn(&mut Checker) -> SymbolId>;
/// Go `func() []*Type`.
pub type GlobalTypesFn = Rc<dyn Fn(&mut Checker) -> Vec<TypeId>>;
/// Go `func(*Type) bool`.
pub type TypeTestFn = Rc<dyn Fn(&mut Checker, TypeId) -> bool>;
/// Go `func(*ast.Node) bool`.
pub type NodeTestFn = Rc<dyn Fn(&mut Checker, Node) -> bool>;
/// Go `func(location, name, meaning, nameNotFoundMessage, isUse, excludeGlobals) *ast.Symbol`.
/// A nil Go message is `None` (see `NameNotFound`).
pub type ResolveNameFn =
    Rc<dyn Fn(&mut Checker, Node, &str, SymbolFlags, Option<NameNotFound>, bool, bool) -> SymbolId>;
/// Go `func(*ast.Symbol, *ast.Symbol) int`.
pub type CompareSymbolsFn = Rc<dyn Fn(&mut Checker, SymbolId, SymbolId) -> i32>;
/// Go `func([]*ast.Symbol, []*ast.Symbol) int`.
pub type CompareSymbolChainsFn = Rc<dyn Fn(&mut Checker, &[SymbolId], &[SymbolId]) -> i32>;

// PORT: placeholders for Go func fields before `NewChecker` assigns them.
// Calling one panics, like calling a nil Go func.
fn nil_global_type_fn() -> GlobalTypeFn {
    Rc::new(|_: &mut Checker| -> TypeId { panic!("call of nil func") })
}

fn nil_global_symbol_fn() -> GlobalSymbolFn {
    Rc::new(|_: &mut Checker| -> SymbolId { panic!("call of nil func") })
}

fn nil_type_test_fn() -> TypeTestFn {
    Rc::new(|_: &mut Checker, _: TypeId| -> bool { panic!("call of nil func") })
}

fn nil_node_test_fn() -> NodeTestFn {
    Rc::new(|_: &mut Checker, _: Node| -> bool { panic!("call of nil func") })
}

fn nil_resolve_name_fn() -> ResolveNameFn {
    Rc::new(
        |_: &mut Checker,
         _: Node,
         _: &str,
         _: SymbolFlags,
         _: Option<NameNotFound>,
         _: bool,
         _: bool|
         -> SymbolId { panic!("call of nil func") },
    )
}

fn nil_compare_symbols_fn() -> CompareSymbolsFn {
    Rc::new(|_: &mut Checker, _: SymbolId, _: SymbolId| -> i32 { panic!("call of nil func") })
}

fn nil_compare_symbol_chains_fn() -> CompareSymbolChainsFn {
    Rc::new(|_: &mut Checker, _: &[SymbolId], _: &[SymbolId]| -> i32 { panic!("call of nil func") })
}

fn nil_type_comparer() -> TypeComparer {
    Rc::new(
        |_: &mut Checker, _: TypeId, _: TypeId, _: bool| -> Ternary { panic!("call of nil func") },
    )
}

fn nil_evaluator() -> Evaluator {
    Rc::new(|_: &mut Checker, _: Node, _: Node| -> EvaluatorResult { panic!("call of nil func") })
}

// PORT: stands for a nil `*IterationTypesResolver`. Every func field panics.
// The message fields need some `&'static Message`; Go never reads them from
// a nil resolver, so the values are never observed.
fn nil_iteration_types_resolver() -> Rc<IterationTypesResolver> {
    Rc::new(IterationTypesResolver {
        iterator_symbol_name: String::new(),
        get_global_iterator_type: nil_global_type_fn(),
        get_global_iterable_type: nil_global_type_fn(),
        get_global_iterable_type_checked: nil_global_type_fn(),
        get_global_iterable_iterator_type: nil_global_type_fn(),
        get_global_iterable_iterator_type_checked: nil_global_type_fn(),
        get_global_iterator_object_type: nil_global_type_fn(),
        get_global_generator_type: nil_global_type_fn(),
        get_global_builtin_iterator_types: Rc::new(|_: &mut Checker| -> Vec<TypeId> {
            panic!("call of nil func")
        }),
        resolve_iteration_type: Rc::new(|_: &mut Checker, _: TypeId, _: Node| -> TypeId {
            panic!("call of nil func")
        }),
        must_have_a_next_method_diagnostic: diag::An_iterator_must_have_a_next_method,
        must_be_a_method_diagnostic: diag::The_0_property_of_an_iterator_must_be_a_method,
        must_have_a_value_diagnostic:
            diag::The_type_returned_by_the_0_method_of_an_iterator_must_have_a_value_property,
    })
}

/// The instantiation cache of one active mapper (an entry of Go
/// `activeTypeMappersCaches`, keyed by a `keyBuilder` hash).
// PERF: Go hashes a key with no alias from the type id and a 0 byte, a
// bijection of the id, so `plain` keys it by the type id itself, in a
// `FlatMap` with 8-byte entries (the key and value share a cache line). A
// key with an alias stays a hashed key in `aliased`. The maps are never
// iterated, so their layout cannot change any output.
#[derive(Default)]
pub struct ActiveMapperCache {
    /// Written only through `insert_plain` and `clear_plain`, so
    /// `plain_keys` stays in sync.
    plain: FlatMap<TypeId, TypeId>,
    /// The first keys put in `plain` since it was last empty, up to the
    /// inline size. The list never spills to the heap.
    plain_keys: smallvec::SmallVec<[TypeId; 8]>,
    pub aliased: CacheKeyMap<TypeId>,
}

impl ActiveMapperCache {
    #[inline]
    #[must_use]
    pub fn get_plain(&self, key: TypeId) -> Option<&TypeId> {
        self.plain.get(&key)
    }

    #[inline]
    pub fn insert_plain(&mut self, key: TypeId, value: TypeId) {
        if self.plain.insert(key, value).is_none()
            && self.plain_keys.len() < self.plain_keys.inline_size()
        {
            self.plain_keys.push(key);
        }
    }

    /// Whether both maps are empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.plain.len() == 0 && self.aliased.is_empty()
    }

    /// Empties `plain` and keeps its table for reuse.
    ///
    /// PERF: most calls find `plain` empty, so that test is inline and the
    /// rest is out of line (`clear_plain_keys`). The test is exact:
    /// `plain_keys` holds only keys of `plain`, so it is empty too.
    #[inline]
    pub fn clear_plain(&mut self, drop_sparse: bool) {
        if self.plain.len() == 0 {
            return;
        }
        self.clear_plain_keys(drop_sparse);
    }

    /// `clear_plain` for a `plain` with keys.
    ///
    /// PERF: a popped mapper often leaves a few keys in a large table. When
    /// every key is in `plain_keys`, only those keys are removed, instead of
    /// a reset of every slot. Else, with `drop_sparse`, a table of more than
    /// 256 keys that is less than 1/8 full is dropped, because a clear costs
    /// time in the table size, and any other table is cleared.
    #[inline(never)]
    fn clear_plain_keys(&mut self, drop_sparse: bool) {
        let plain = &mut self.plain;
        if plain.len() == self.plain_keys.len() {
            for key in self.plain_keys.drain(..) {
                plain.remove(&key);
            }
            return;
        }
        self.plain_keys.clear();
        if drop_sparse && plain.capacity() > 256 && plain.len() < plain.capacity() / 8 {
            *plain = FlatMap::default();
        } else {
            plain.clear();
        }
    }
}

// PORT: perf. `FlatMap` indexes by the low bits of the hash, and type ids
// are dense, so the id is multiplied by the 64-bit golden ratio and the high
// half is folded down to mix the low bits. The nil id is the empty-slot
// marker; `FlatMap` keeps that key in its side slot.
impl FlatKey for TypeId {
    #[inline]
    fn flat_hash(&self) -> u64 {
        let x = u64::from(self.0).wrapping_mul(0x9E37_79B9_7F4A_7C15);
        x ^ (x >> 32)
    }
}

// Checker

// Go: checker/checker.go:585 Checker
// PORT: Go fields keep their order and snake names. Differences:
// - `program Program` is `&'static GoProgram` (the installed `prog()`).
// - `symbolArena`, `signatureArena`, `indexInfoArena` are replaced by the
//   arenas at the end (`symbols`, `types`, `signatures`, `index_infos`,
//   `type_predicates`, `mappers`, `inference_contexts`), see `PORTING.md`.
// - `regExpScanner` (Go scanner) and `mu` are out of scope (scanner object,
//   concurrency) and are not fields. The nil-able Go `ctx` is
//   `Option<Context>`. `tracer` is the last field (see `crate::tracing`).
//   `emitResolver` plus `emitResolverOnce` is the `Option<Rc<EmitResolver>>`
//   field `emit_resolver`.
// - `sync.Once` fields become `bool` "done" flags.
// - `*T` pools and shared structs (`*Relation`, `*Relater`, `*FlowState`,
//   `*InferenceState`) are `Rc<RefCell<T>>`; nil-able ones are `Option`.
// - `ast.NodeId` map keys are `Node` handles.
pub struct Checker {
    pub id: u32,
    pub program: &'static GoProgram,
    pub compiler_options: &'static CompilerOptions,
    pub files: Vec<Node>,
    pub file_index_map: FxHashMap<Node, i32>,
    pub compare_symbols: CompareSymbolsFn,
    pub compare_symbol_chains: CompareSymbolChainsFn,
    pub type_count: u32,
    pub symbol_count: u32,
    pub signature_count: u32,
    // PORT: counter for `Signature::type_parameters_origin`, and the origin
    // of each class's `LocalTypeParameters()` slice (Go returns the same
    // slice on every call).
    pub type_parameters_origin_count: u32,
    pub class_type_parameters_origins: FxHashMap<TypeId, u32>,
    pub total_instantiation_count: u32,
    pub instantiation_count: u32,
    pub instantiation_stack: Vec<TypeId>,
    pub conditional_constraint_depth: u32,
    pub inline_level: i32,
    pub serialization_level: i32,
    pub current_node: Node,
    pub variance_type_parameter: TypeId,
    pub language_version: ScriptTarget,
    pub module_kind: ModuleKind,
    pub module_resolution_kind: ModuleResolutionKind,
    pub is_inference_partially_blocked: bool,
    pub legacy_decorators: bool,
    pub emit_standard_class_fields: bool,
    pub strict_null_checks: bool,
    pub strict_function_types: bool,
    pub strict_bind_call_apply: bool,
    pub strict_property_initialization: bool,
    pub strict_builtin_iterator_return: bool,
    pub no_implicit_any: bool,
    pub no_implicit_this: bool,
    pub use_unknown_in_catch_variables: bool,
    pub exact_optional_property_types: bool,
    pub can_collect_symbol_alias_accessibility_data: bool,
    pub emit_resolver: Option<Rc<crate::checker::emit_resolver_p1::EmitResolver>>,
    pub was_canceled: bool,
    pub array_variances: SharedList<VarianceFlags>,
    pub globals: SymbolTable,
    pub evaluate: Evaluator,
    // PORT: perf (tcsplit1). Go keys on the value. Here the key is the
    // FxHash of the value, and the types of a bucket hold the values (see
    // `get_string_literal_type_cow`).
    pub string_literal_types: FxHashMap<u64, smallvec::SmallVec<[TypeId; 1]>>,
    pub number_literal_types: FxHashMap<NumberKey, TypeId>,
    pub nan_type: TypeId,
    pub bigint_literal_types: FxHashMap<PseudoBigIntKey, TypeId>,
    pub enum_literal_types: FxHashMap<EnumLiteralKey, TypeId>,
    pub enum_nan_literal_types: FxHashMap<SymbolId, TypeId>,
    pub indexed_access_types: CacheKeyMap<TypeId>,
    pub template_literal_types: CacheKeyMap<TypeId>,
    pub string_mapping_types: FxHashMap<StringMappingKey, TypeId>,
    pub unique_es_symbol_types: FxHashMap<SymbolId, TypeId>,
    pub this_expando_kinds: FxHashMap<SymbolId, ThisAssignmentDeclarationKind>,
    pub this_expando_locations: FxHashMap<SymbolId, Node>,
    pub subtype_reduction_cache: CacheKeyMap<Vec<TypeId>>,
    pub cached_types: FxHashMap<CachedTypeKey, TypeId>,
    pub cached_signatures: FxHashMap<CachedSignatureKey, SignatureId>,
    pub undefined_properties: FxHashMap<String, SymbolId>,
    pub narrowed_types: FxHashMap<NarrowedTypeKey, TypeId>,
    pub assignment_reduced_types: FxHashMap<AssignmentReducedKey, TypeId>,
    pub discriminated_contextual_types: FxHashMap<DiscriminatedContextualTypeKey, TypeId>,
    pub instantiation_expression_types: FxHashMap<InstantiationExpressionKey, TypeId>,
    pub substitution_types: FxHashMap<SubstitutionTypeKey, TypeId>,
    pub reverse_mapped_cache: FxHashMap<ReverseMappedTypeKey, TypeId>,
    pub reverse_homomorphic_mapped_cache: FxHashMap<ReverseMappedTypeKey, TypeId>,
    pub iteration_types_cache: FxHashMap<IterationTypesKey, IterationTypes>,
    pub marker_types: FxHashSet<TypeId>,
    pub resolving_explicit_type_of_symbol: FxHashSet<SymbolId>,
    pub undefined_symbol: SymbolId,
    pub arguments_symbol: SymbolId,
    pub require_symbol: SymbolId,
    pub unknown_symbol: SymbolId,
    pub unresolved_symbols: FxHashMap<String, SymbolId>,
    pub error_types: CacheKeyMap<TypeId>,
    pub module_symbols: FxHashMap<Node, SymbolId>,
    pub global_this_symbol: SymbolId,
    pub symbol_table_alias_cache: FxHashMap<SymbolTableID, Vec<SymbolId>>,
    pub class_expression_name_tables: FxHashMap<Node, SymbolTable>,
    pub resolve_name: ResolveNameFn,
    pub resolve_name_for_symbol_suggestion: ResolveNameFn,
    /// PORT: port-only memo of the spelling suggestion from the globals
    /// table, by name id and meaning (`get_suggestion_for_symbol_name_lookup`),
    /// with the `merge_version` from before its scan. Read and filled only
    /// when `globals_complete` is set.
    pub global_spelling_suggestions: FxHashMap<(u32, SymbolFlags), (u64, GlobalSpellingSuggestion)>,
    /// PORT: port-only. Set at the end of `initialize_checker`. After that,
    /// nothing adds names to the globals table or removes them.
    pub globals_complete: bool,
    pub tuple_types: CacheKeyMap<TypeId>,
    pub union_types: FxHashMap<CacheHashKey, TypeId>,
    pub union_of_union_types: FxHashMap<UnionOfUnionKey, TypeId>,
    pub intersection_types: CacheKeyMap<TypeId>,
    pub properties_types: FxHashMap<PropertiesTypesKey, TypeId>,
    pub diagnostics: DiagnosticsCollection,
    pub suggestion_diagnostics: DiagnosticsCollection,
    pub merged_symbols: FxHashMap<SymbolId, SymbolId>,
    pub factory: NodeFactory,
    pub node_links: LinkStore<Node, NodeLinks>,
    pub signature_links: LinkStore<Node, Box<SignatureLinks>>,
    pub symbol_node_links: NodeLinkStore<SymbolNodeLinks>,
    pub type_node_links: LinkStore<Node, Box<TypeNodeLinks>>,
    pub enum_member_links: LinkStore<Node, Box<EnumMemberLinks>>,
    pub assertion_links: LinkStore<Node, AssertionLinks>,
    pub array_literal_links: LinkStore<Node, ArrayLiteralLinks>,
    pub switch_statement_links: LinkStore<Node, Box<SwitchStatementLinks>>,
    pub jsx_element_links: LinkStore<Node, Box<JsxElementLinks>>,
    pub computed_name_links: LinkStore<Node, Box<ComputedNameNodeLinks>>,
    pub symbol_reference_links: LinkStore<SymbolId, SymbolReferenceLinks>,
    pub value_symbol_links: ValueSymbolLinkStore,
    pub mapped_symbol_links: LinkStore<SymbolId, MappedSymbolLinks>,
    pub deferred_symbol_links: LinkStore<SymbolId, Box<DeferredSymbolLinks>>,
    pub alias_symbol_links: LinkStore<SymbolId, Box<AliasSymbolLinks>>,
    pub module_symbol_links: LinkStore<SymbolId, Box<ModuleSymbolLinks>>,
    pub late_bound_links: LinkStore<SymbolId, LateBoundLinks>,
    pub export_type_links: LinkStore<SymbolId, Box<ExportTypeLinks>>,
    pub members_and_exports_links: LinkStore<SymbolId, MembersAndExportsLinks>,
    pub type_alias_links: LinkStore<SymbolId, Box<TypeAliasLinks>>,
    pub declared_type_links: LinkStore<SymbolId, DeclaredTypeLinks>,
    pub spread_links: LinkStore<SymbolId, SpreadLinks>,
    pub variance_links: LinkStore<SymbolId, Box<VarianceLinks>>,
    /// Go `ReverseMappedSymbolLinks` (exported field name).
    pub reverse_mapped_symbol_links: LinkStore<SymbolId, ReverseMappedSymbolLinks>,
    pub marked_assignment_symbol_links: LinkStore<SymbolId, MarkedAssignmentSymbolLinks>,
    pub symbol_container_links: LinkStore<SymbolId, Box<ContainingSymbolLinks>>,
    pub source_file_links: LinkStore<Node, Box<SourceFileLinks>>,
    pub pattern_for_type: FxHashMap<TypeId, Node>,
    pub context_free_types: FxHashMap<Node, TypeId>,
    pub any_type: TypeId,
    pub auto_type: TypeId,
    pub wildcard_type: TypeId,
    pub blocked_string_type: TypeId,
    pub error_type: TypeId,
    pub unresolved_type: TypeId,
    pub non_inferrable_any_type: TypeId,
    pub intrinsic_marker_type: TypeId,
    pub unknown_type: TypeId,
    pub undefined_type: TypeId,
    pub undefined_widening_type: TypeId,
    pub missing_type: TypeId,
    pub undefined_or_missing_type: TypeId,
    pub optional_type: TypeId,
    pub null_type: TypeId,
    pub null_widening_type: TypeId,
    pub string_type: TypeId,
    pub number_type: TypeId,
    pub bigint_type: TypeId,
    pub regular_false_type: TypeId,
    pub false_type: TypeId,
    pub regular_true_type: TypeId,
    pub true_type: TypeId,
    pub boolean_type: TypeId,
    pub es_symbol_type: TypeId,
    pub void_type: TypeId,
    pub never_type: TypeId,
    pub silent_never_type: TypeId,
    pub implicit_never_type: TypeId,
    pub unreachable_never_type: TypeId,
    pub non_primitive_type: TypeId,
    pub string_or_number_type: TypeId,
    pub string_number_symbol_type: TypeId,
    pub number_or_big_int_type: TypeId,
    pub template_constraint_type: TypeId,
    pub numeric_string_type: TypeId,
    pub unique_literal_type: TypeId,
    pub unique_literal_mapper: MapperId,
    pub reliability_flags: RelationComparisonResult,
    pub report_unreliable_mapper: MapperId,
    pub report_unmeasurable_mapper: MapperId,
    pub restrictive_mapper: MapperId,
    pub permissive_mapper: MapperId,
    pub empty_object_type: TypeId,
    pub empty_jsx_object_type: TypeId,
    pub empty_fresh_jsx_object_type: TypeId,
    pub empty_type_literal_type: TypeId,
    pub unknown_empty_object_type: TypeId,
    pub unknown_union_type: TypeId,
    pub empty_generic_type: TypeId,
    pub any_function_type: TypeId,
    pub no_constraint_type: TypeId,
    pub circular_constraint_type: TypeId,
    pub resolving_default_type: TypeId,
    pub marker_super_type: TypeId,
    pub marker_sub_type: TypeId,
    pub marker_other_type: TypeId,
    pub marker_super_type_for_check: TypeId,
    pub marker_sub_type_for_check: TypeId,
    pub no_type_predicate: TypePredicateId,
    pub any_signature: SignatureId,
    pub unknown_signature: SignatureId,
    pub resolving_signature: SignatureId,
    pub silent_never_signature: SignatureId,
    pub cached_arguments_referenced: FxHashMap<Node, bool>,
    pub enum_number_index_info: IndexInfoId,
    pub any_base_type_index_info: IndexInfoId,
    pub pattern_ambient_modules: Vec<PatternAmbientModule>,
    pub pattern_ambient_module_augmentations: SymbolTable,
    pub pattern_ambient_module_augmentation_targets: SymbolTable,
    pub module_import_attributes_types: FxHashMap<SymbolId, TypeId>,
    pub global_object_type: TypeId,
    pub global_function_type: TypeId,
    pub global_callable_function_type: TypeId,
    pub global_newable_function_type: TypeId,
    /// PERF (propfilt1): `get_property_of_type_ex`'s filters of the member
    /// names of the 4 types above (`augment_lookups_miss`). A type's filter
    /// is built when its members are set (`augment_members_set`), dropped
    /// at the base types reset (`drop_augment_filter_of`), and built again
    /// after each module augmentation merge (`rebuild_augment_filters`).
    pub augment_filters: AugmentFilters,
    pub global_array_type: TypeId,
    pub global_readonly_array_type: TypeId,
    pub global_string_type: TypeId,
    pub global_number_type: TypeId,
    pub global_boolean_type: TypeId,
    pub global_reg_exp_type: TypeId,
    pub global_this_type: TypeId,
    pub any_array_type: TypeId,
    pub auto_array_type: TypeId,
    pub any_readonly_array_type: TypeId,
    pub deferred_global_import_meta_expression_type: TypeId,
    pub contextual_binding_patterns: Vec<Node>,
    pub empty_string_type: TypeId,
    pub zero_type: TypeId,
    pub zero_big_int_type: TypeId,
    pub typeof_type: TypeId,
    pub type_resolutions: Vec<TypeResolution>,
    pub resolution_start: i32,
    pub variance_stack: Vec<VarianceStackEntry>,
    pub call_resolution_stack: Vec<Node>,
    /// Go `*int`; nil is `None`.
    pub apparent_argument_count: Option<i32>,
    pub last_get_combined_node_flags_node: Node,
    pub last_get_combined_node_flags_result: NodeFlags,
    pub last_get_combined_modifier_flags_node: Node,
    pub last_get_combined_modifier_flags_result: ModifierFlags,
    pub freeinference_state: Option<Rc<RefCell<InferenceState>>>,
    pub free_flow_state: Option<Rc<RefCell<FlowState>>>,
    pub flow_loop_cache: FxHashMap<FlowLoopKey, TypeId>,
    pub flow_loop_stack: Vec<FlowLoopInfo>,
    pub shared_flows: Vec<SharedFlow>,
    pub antecedent_types: Vec<TypeId>,
    pub flow_analysis_disabled: bool,
    pub flow_invocation_count: i32,
    pub flow_type_cache: FxHashMap<Node, TypeId>,
    /// PERF (cfcache1, not in Go): `get_control_flow_container`'s memo, a
    /// direct-mapped table of (node, container). A nil key is an empty slot.
    pub control_flow_containers: Box<[(Node, Node); CONTROL_FLOW_CONTAINER_SLOTS]>,
    pub last_flow_node: FlowNodeId,
    pub last_flow_node_reachable: bool,
    pub flow_node_reachable: FxHashMap<FlowNodeId, bool>,
    pub flow_node_post_super: FxHashMap<FlowNodeId, bool>,
    pub renamed_binding_elements_in_types: Vec<Node>,
    pub contextual_infos: Vec<ContextualInfo>,
    pub inference_context_infos: Vec<InferenceContextInfo>,
    pub awaited_type_stack: Vec<TypeId>,
    pub reverse_mapped_source_stack: Vec<TypeId>,
    pub reverse_mapped_target_stack: Vec<TypeId>,
    pub reverse_expanding_flags: ExpandingFlags,
    pub free_relater: Option<Rc<RefCell<Relater>>>,
    pub subtype_relation: Rc<RefCell<Relation>>,
    pub strict_subtype_relation: Rc<RefCell<Relation>>,
    pub assignable_relation: Rc<RefCell<Relation>>,
    pub comparable_relation: Rc<RefCell<Relation>>,
    pub identity_relation: Rc<RefCell<Relation>>,
    pub enum_relation: FxHashMap<EnumRelationKey, RelationComparisonResult>,
    pub get_global_es_symbol_type: GlobalTypeFn,
    pub get_global_big_int_type: GlobalTypeFn,
    pub get_global_import_meta_type: GlobalTypeFn,
    pub get_global_import_attributes_type: GlobalTypeFn,
    pub get_global_import_attributes_type_checked: GlobalTypeFn,
    pub get_global_non_nullable_type_alias_or_nil: GlobalSymbolFn,
    pub get_global_extract_symbol: GlobalSymbolFn,
    pub get_global_disposable_type: GlobalTypeFn,
    pub get_global_async_disposable_type: GlobalTypeFn,
    pub get_global_awaited_symbol: GlobalSymbolFn,
    pub get_global_awaited_symbol_or_nil: GlobalSymbolFn,
    pub get_global_na_n_symbol_or_nil: GlobalSymbolFn,
    pub get_global_record_symbol: GlobalSymbolFn,
    pub get_global_template_strings_array_type: GlobalTypeFn,
    pub get_global_es_symbol_constructor_symbol_or_nil: GlobalSymbolFn,
    pub get_global_es_symbol_constructor_type_symbol_or_nil: GlobalSymbolFn,
    pub get_global_import_call_options_type: GlobalTypeFn,
    pub get_global_import_call_options_type_checked: GlobalTypeFn,
    pub get_global_promise_type: GlobalTypeFn,
    pub get_global_promise_type_checked: GlobalTypeFn,
    pub get_global_promise_like_type: GlobalTypeFn,
    pub get_global_promise_constructor_symbol: GlobalSymbolFn,
    pub get_global_promise_constructor_symbol_or_nil: GlobalSymbolFn,
    pub get_global_omit_symbol: GlobalSymbolFn,
    pub get_global_no_infer_symbol_or_nil: GlobalSymbolFn,
    pub get_global_iterator_type: GlobalTypeFn,
    pub get_global_iterable_type: GlobalTypeFn,
    pub get_global_iterable_type_checked: GlobalTypeFn,
    pub get_global_iterable_iterator_type: GlobalTypeFn,
    pub get_global_iterable_iterator_type_checked: GlobalTypeFn,
    pub get_global_iterator_object_type: GlobalTypeFn,
    pub get_global_generator_type: GlobalTypeFn,
    pub get_global_async_iterator_type: GlobalTypeFn,
    pub get_global_async_iterable_type: GlobalTypeFn,
    pub get_global_async_iterable_type_checked: GlobalTypeFn,
    pub get_global_async_iterable_iterator_type: GlobalTypeFn,
    pub get_global_async_iterable_iterator_type_checked: GlobalTypeFn,
    pub get_global_async_iterator_object_type: GlobalTypeFn,
    pub get_global_async_generator_type: GlobalTypeFn,
    pub get_global_iterator_yield_result_type: GlobalTypeFn,
    pub get_global_iterator_return_result_type: GlobalTypeFn,
    pub get_global_typed_property_descriptor_type: GlobalTypeFn,
    pub get_global_class_decorator_context_type: GlobalTypeFn,
    pub get_global_class_method_decorator_context_type: GlobalTypeFn,
    pub get_global_class_getter_decorator_context_type: GlobalTypeFn,
    pub get_global_class_setter_decorator_context_type: GlobalTypeFn,
    /// Go field with a typo; Go never assigns it.
    pub get_global_class_accessor_decorator_contxt_type: GlobalTypeFn,
    pub get_global_class_accessor_decorator_context_type: GlobalTypeFn,
    pub get_global_class_accessor_decorator_target_type: GlobalTypeFn,
    pub get_global_class_accessor_decorator_result_type: GlobalTypeFn,
    pub get_global_class_field_decorator_context_type: GlobalTypeFn,
    /// Go `*IterationTypesResolver`. PORT: not an `Option`; Go reads these
    /// only after `initializeIterationResolvers`, so a placeholder whose func
    /// fields panic (see `nil_iteration_types_resolver`) stands for nil.
    pub sync_iteration_types_resolver: Rc<IterationTypesResolver>,
    pub async_iteration_types_resolver: Rc<IterationTypesResolver>,
    pub is_primitive_or_object_or_empty_type: TypeTestFn,
    pub contains_missing_type: TypeTestFn,
    // PORT: Go field `couldContainTypeVariables` is the `#[inline]` method
    // `Checker::could_contain_type_variables` (fnfields.rs), not a stored fn.
    pub is_string_index_signature_only_type: TypeTestFn,
    pub mark_node_assignments: NodeTestFn,
    pub compare_types_assignable: TypeComparer,
    pub _jsx_namespace: String,
    pub _jsx_factory_entity: Node,
    pub skip_direct_inference_nodes: FxHashSet<Node>,
    pub ctx: Option<Context>,
    pub packages_map: FxHashMap<String, bool>,
    pub active_mappers: Vec<MapperId>,
    pub active_type_mappers_caches: Vec<ActiveMapperCache>,
    /// Go `ambientModulesOnce sync.Once`: true once the Go `Do` body ran.
    pub ambient_modules_once: bool,
    pub ambient_modules: Vec<SymbolId>,
    pub within_unreachable_code: bool,
    pub reported_unreachable_nodes: FxHashSet<Node>,
    pub non_existent_properties: FxHashSet<NonExistentPropertyKey>,
    pub deferred_diagnostic_callbacks: Vec<Rc<dyn Fn(&mut Checker)>>,
    /// Go `typeToStringNodeBuilder` (`getNodeBuilder` caches it).
    pub type_to_string_nodebuilder: Option<Rc<RefCell<NodeBuilder>>>,
    /// PORT: not in Go. Reusable buffers of `get_named_members`.
    pub(crate) named_members_scratch: crate::checker::checker_p24::NamedMembersScratch,
    /// PERF: not in Go. The `(symbol, enclosing declaration)` calls of
    /// `get_alternative_containing_modules` whose import loop found nothing.
    pub(crate) alternative_module_import_misses: FxHashSet<(SymbolId, Node)>,
    /// PERF: not in Go. See `get_named_members_of_instantiation`.
    pub(crate) named_members_orders:
        FxHashMap<(SymbolTable, SymbolId), Option<crate::checker::checker_p24::NamedMembersOrder>>,
    /// PERF: not in Go. See `MatchingReferenceMemo`.
    pub(crate) matching_reference_memo: crate::checker::flow_p2::MatchingReferenceMemo,
    /// flowskip1: the index and state of the flow walk skip (flow_skip.rs).
    pub flow_skip: crate::checker::flow_skip::FlowSkip,
    /// PERF: not in Go. Counts the merges (`merge_symbol` and
    /// `record_merged_symbol`), so a memo of a merged symbol or of the flags
    /// a merge adds knows when to read them again (`MatchingReferenceMemo`,
    /// `global_spelling_suggestions`).
    pub(crate) merge_version: u64,
    /// PERF: not in Go. See `is_type_subset_of_union`.
    pub(crate) union_subset_answers: FxHashMap<(TypeId, TypeId), bool>,

    // Arenas (PORTING.md "Checker data"). Index 0 of each is a dummy entry
    // so handle value 0 stays nil.
    pub symbols: SymbolArena,
    pub types: ChunkedArena<Type>,
    /// PORT: Go `ObjectType.instantiations` of the object types that are
    /// not interfaces or tuples, at `ObjectType::instantiations` (see
    /// `ObjectType`). Index 0 is a dummy, so `InstantiationMapId::NIL` is 0.
    pub object_type_instantiations: Vec<InstantiationMap>,
    pub signatures: Vec<Signature>,
    pub index_infos: Vec<IndexInfo>,
    pub type_predicates: Vec<TypePredicate>,
    pub mappers: ChunkedArena<TypeMapper>,
    // PERF (infermem1): a `ChunkedArena`, not a `Vec`: a doubling `Vec` kept
    // up to half its room unused and copied every context when it grew
    // (typebox: buffers of 512 and 256 MiB).
    pub inference_contexts: ChunkedArena<InferenceContext>,

    /// Go `tracer *Tracer` (checker.go:897): optional tracer for trace
    /// events and type recording (for --generateTrace). None is Go nil.
    pub tracer: Option<crate::tracing::Tracer>,

    /// Effect-TS/tsgo patch 023 `EffectLinks`: the Effect type parser caches
    /// (`effect::typeparser`). None until a rule first runs.
    pub effect_links: Option<Box<crate::effect::typeparser::EffectLinks>>,
    /// Effect-TS/tsgo patches 002 and 004 `SourceFileLinks.relationErrors`:
    /// the relation errors of each source file, for Effect rules. Kept only
    /// when the program has Effect diagnostics options.
    pub effect_relation_errors: FxHashMap<Node, Vec<crate::effect::RelationError>>,
}

// Arena accessors (PORTING.md "Checker data"). Handles index their arena
// directly; index 0 is the nil dummy.
impl Checker {
    #[must_use]
    #[inline(always)]
    pub fn sym(&self, s: SymbolId) -> &Symbol {
        self.symbols.sym(s)
    }

    #[inline(always)]
    pub fn sym_mut(&mut self, s: SymbolId) -> &mut Symbol {
        self.symbols.sym_mut(s)
    }

    #[must_use]
    #[inline(always)]
    pub fn ty(&self, t: TypeId) -> &Type {
        debug_assert!(t.is_some(), "nil type dereference");
        &self.types[t.index()]
    }

    #[inline(always)]
    pub fn ty_mut(&mut self, t: TypeId) -> &mut Type {
        debug_assert!(t.is_some(), "nil type dereference");
        &mut self.types[t.index()]
    }

    #[must_use]
    #[inline(always)]
    pub fn sig(&self, s: SignatureId) -> &Signature {
        debug_assert!(s.is_some(), "nil signature dereference");
        &self.signatures[s.index()]
    }

    pub fn sig_mut(&mut self, s: SignatureId) -> &mut Signature {
        debug_assert!(s.is_some(), "nil signature dereference");
        &mut self.signatures[s.index()]
    }

    #[must_use]
    pub fn index_info(&self, i: IndexInfoId) -> &IndexInfo {
        debug_assert!(i.is_some(), "nil index info dereference");
        &self.index_infos[i.index()]
    }

    pub fn index_info_mut(&mut self, i: IndexInfoId) -> &mut IndexInfo {
        debug_assert!(i.is_some(), "nil index info dereference");
        &mut self.index_infos[i.index()]
    }

    #[must_use]
    pub fn pred(&self, p: TypePredicateId) -> &TypePredicate {
        debug_assert!(p.is_some(), "nil type predicate dereference");
        &self.type_predicates[p.index()]
    }

    pub fn pred_mut(&mut self, p: TypePredicateId) -> &mut TypePredicate {
        debug_assert!(p.is_some(), "nil type predicate dereference");
        &mut self.type_predicates[p.index()]
    }

    #[must_use]
    pub fn mapper(&self, m: MapperId) -> &TypeMapper {
        debug_assert!(m.is_some(), "nil mapper dereference");
        &self.mappers[m.index()]
    }

    pub fn mapper_mut(&mut self, m: MapperId) -> &mut TypeMapper {
        debug_assert!(m.is_some(), "nil mapper dereference");
        &mut self.mappers[m.index()]
    }

    #[must_use]
    pub fn inference_context(&self, c: InferenceContextId) -> &InferenceContext {
        debug_assert!(c.is_some(), "nil inference context dereference");
        &self.inference_contexts[c.index()]
    }

    pub fn inference_context_mut(&mut self, c: InferenceContextId) -> &mut InferenceContext {
        debug_assert!(c.is_some(), "nil inference context dereference");
        &mut self.inference_contexts[c.index()]
    }
}

// Go: checker/checker.go:911 NewChecker
// PORT: Go `NewChecker(program) (*Checker, *sync.Mutex)` becomes
// `Checker::new(checker_index)`. The program is the installed `prog()`; the
// mutex is dropped, and the tracer comes from the process tracing session
// (`crate::tracing::new_checker_tracer`). Go `c.id = nextCheckerID.Add(1)` numbers
// checkers from 1 in creation order; the pool creates them in index order,
// so `id = checker_index + 1`.
// PORT: Go binds every file (`program.BindSourceFiles`) before it creates
// checkers. Here the checker binds the program first if nothing has
// (`program::bind_all`), then copies the program's binder symbols
// (`program::bound_symbols`) as its own symbol arena
// (`SymbolArena::for_checker`). `with_symbols` takes another copy (the
// API's persistent checker, `ls_program::new_api_checker`).
// PORT: the checker reads its program through `program` and
// `compiler_options`, but the `program.rs` functions it calls read the
// current program (`prog()`). A thread that holds checkers of several
// programs makes the checker's program current while it uses the checker
// (`core::enter_program`).
// PORT: Go `make(map...)` initializations are the `Default` values in the
// struct literal; Go nil fields not set here keep their nil value.
impl Checker {
    pub fn new(checker_index: usize) -> Checker {
        bind_all();
        Self::with_symbols(checker_index, crate::program::bound_symbols().for_checker())
    }

    /// `new` with `symbols` as the checker's symbol arena: a checker copy
    /// (`SymbolArena::for_checker`) of a binder lineage copy that holds the
    /// files of the current program, which is bound (`bind_all`).
    pub fn with_symbols(checker_index: usize, symbols: SymbolArena) -> Checker {
        let program = prog();
        let compiler_options = &program.options;
        let files: Vec<Node> = program.source_files().map(|f| f.root).collect();
        let file_index_map = create_file_index_map(&files);
        let mut c = Checker {
            id: u32::try_from(checker_index + 1).expect("checker id overflow"),
            program,
            compiler_options,
            files,
            file_index_map,
            compare_symbols: nil_compare_symbols_fn(),
            compare_symbol_chains: nil_compare_symbol_chains_fn(),
            type_count: 0,
            symbol_count: 0,
            signature_count: 0,
            type_parameters_origin_count: 0,
            class_type_parameters_origins: FxHashMap::default(),
            total_instantiation_count: 0,
            instantiation_count: 0,
            instantiation_stack: Vec::new(),
            conditional_constraint_depth: 0,
            inline_level: 0,
            serialization_level: 0,
            current_node: Node::NIL,
            variance_type_parameter: TypeId::NIL,
            language_version: compiler_options.get_emit_script_target(),
            module_kind: compiler_options.get_emit_module_kind(),
            module_resolution_kind: compiler_options.get_module_resolution_kind(),
            is_inference_partially_blocked: false,
            legacy_decorators: compiler_options.experimental_decorators == Tristate::True,
            emit_standard_class_fields: compiler_options.get_emit_standard_class_fields(),
            strict_null_checks: compiler_options
                .get_strict_option_value(compiler_options.strict_null_checks),
            strict_function_types: compiler_options
                .get_strict_option_value(compiler_options.strict_function_types),
            strict_bind_call_apply: compiler_options
                .get_strict_option_value(compiler_options.strict_bind_call_apply),
            strict_property_initialization: compiler_options
                .get_strict_option_value(compiler_options.strict_property_initialization),
            strict_builtin_iterator_return: compiler_options
                .get_strict_option_value(compiler_options.strict_builtin_iterator_return),
            no_implicit_any: compiler_options
                .get_strict_option_value(compiler_options.no_implicit_any),
            no_implicit_this: compiler_options
                .get_strict_option_value(compiler_options.no_implicit_this),
            use_unknown_in_catch_variables: compiler_options
                .get_strict_option_value(compiler_options.use_unknown_in_catch_variables),
            exact_optional_property_types: compiler_options.exact_optional_property_types
                == Tristate::True,
            can_collect_symbol_alias_accessibility_data: compiler_options
                .verbatim_module_syntax
                .is_false_or_unknown(),
            emit_resolver: None,
            was_canceled: false,
            array_variances: vec![VarianceFlags::COVARIANT].into(),
            globals: SymbolTable::NIL,
            evaluate: nil_evaluator(),
            string_literal_types: FxHashMap::default(),
            number_literal_types: FxHashMap::default(),
            nan_type: TypeId::NIL,
            bigint_literal_types: FxHashMap::default(),
            enum_literal_types: FxHashMap::default(),
            enum_nan_literal_types: FxHashMap::default(),
            indexed_access_types: CacheKeyMap::default(),
            template_literal_types: CacheKeyMap::default(),
            string_mapping_types: FxHashMap::default(),
            unique_es_symbol_types: FxHashMap::default(),
            this_expando_kinds: FxHashMap::default(),
            this_expando_locations: FxHashMap::default(),
            subtype_reduction_cache: CacheKeyMap::default(),
            cached_types: FxHashMap::default(),
            cached_signatures: FxHashMap::default(),
            undefined_properties: FxHashMap::default(),
            narrowed_types: FxHashMap::default(),
            assignment_reduced_types: FxHashMap::default(),
            discriminated_contextual_types: FxHashMap::default(),
            instantiation_expression_types: FxHashMap::default(),
            substitution_types: FxHashMap::default(),
            reverse_mapped_cache: FxHashMap::default(),
            reverse_homomorphic_mapped_cache: FxHashMap::default(),
            iteration_types_cache: FxHashMap::default(),
            marker_types: FxHashSet::default(),
            resolving_explicit_type_of_symbol: FxHashSet::default(),
            undefined_symbol: SymbolId::NIL,
            arguments_symbol: SymbolId::NIL,
            require_symbol: SymbolId::NIL,
            unknown_symbol: SymbolId::NIL,
            unresolved_symbols: FxHashMap::default(),
            error_types: CacheKeyMap::default(),
            module_symbols: FxHashMap::default(),
            global_this_symbol: SymbolId::NIL,
            symbol_table_alias_cache: FxHashMap::default(),
            class_expression_name_tables: FxHashMap::default(),
            resolve_name: nil_resolve_name_fn(),
            resolve_name_for_symbol_suggestion: nil_resolve_name_fn(),
            global_spelling_suggestions: FxHashMap::default(),
            globals_complete: false,
            tuple_types: CacheKeyMap::default(),
            union_types: FxHashMap::default(),
            union_of_union_types: FxHashMap::default(),
            intersection_types: CacheKeyMap::default(),
            properties_types: FxHashMap::default(),
            diagnostics: DiagnosticsCollection::default(),
            suggestion_diagnostics: DiagnosticsCollection::default(),
            merged_symbols: FxHashMap::default(),
            // Go leaves `factory` as the zero `ast.NodeFactory`.
            factory: NodeFactory::new(),
            node_links: LinkStore::default(),
            signature_links: LinkStore::default(),
            symbol_node_links: LinkStore::default(),
            type_node_links: LinkStore::default(),
            enum_member_links: LinkStore::default(),
            assertion_links: LinkStore::default(),
            array_literal_links: LinkStore::default(),
            switch_statement_links: LinkStore::default(),
            jsx_element_links: LinkStore::default(),
            computed_name_links: LinkStore::default(),
            symbol_reference_links: LinkStore::default(),
            value_symbol_links: ValueSymbolLinkStore::default(),
            mapped_symbol_links: LinkStore::default(),
            deferred_symbol_links: LinkStore::default(),
            alias_symbol_links: LinkStore::default(),
            module_symbol_links: LinkStore::default(),
            late_bound_links: LinkStore::default(),
            export_type_links: LinkStore::default(),
            members_and_exports_links: LinkStore::default(),
            type_alias_links: LinkStore::default(),
            declared_type_links: LinkStore::default(),
            spread_links: LinkStore::default(),
            variance_links: LinkStore::default(),
            reverse_mapped_symbol_links: LinkStore::default(),
            marked_assignment_symbol_links: LinkStore::default(),
            symbol_container_links: LinkStore::default(),
            source_file_links: LinkStore::default(),
            pattern_for_type: FxHashMap::default(),
            context_free_types: FxHashMap::default(),
            any_type: TypeId::NIL,
            auto_type: TypeId::NIL,
            wildcard_type: TypeId::NIL,
            blocked_string_type: TypeId::NIL,
            error_type: TypeId::NIL,
            unresolved_type: TypeId::NIL,
            non_inferrable_any_type: TypeId::NIL,
            intrinsic_marker_type: TypeId::NIL,
            unknown_type: TypeId::NIL,
            undefined_type: TypeId::NIL,
            undefined_widening_type: TypeId::NIL,
            missing_type: TypeId::NIL,
            undefined_or_missing_type: TypeId::NIL,
            optional_type: TypeId::NIL,
            null_type: TypeId::NIL,
            null_widening_type: TypeId::NIL,
            string_type: TypeId::NIL,
            number_type: TypeId::NIL,
            bigint_type: TypeId::NIL,
            regular_false_type: TypeId::NIL,
            false_type: TypeId::NIL,
            regular_true_type: TypeId::NIL,
            true_type: TypeId::NIL,
            boolean_type: TypeId::NIL,
            es_symbol_type: TypeId::NIL,
            void_type: TypeId::NIL,
            never_type: TypeId::NIL,
            silent_never_type: TypeId::NIL,
            implicit_never_type: TypeId::NIL,
            unreachable_never_type: TypeId::NIL,
            non_primitive_type: TypeId::NIL,
            string_or_number_type: TypeId::NIL,
            string_number_symbol_type: TypeId::NIL,
            number_or_big_int_type: TypeId::NIL,
            template_constraint_type: TypeId::NIL,
            numeric_string_type: TypeId::NIL,
            unique_literal_type: TypeId::NIL,
            unique_literal_mapper: MapperId::NIL,
            reliability_flags: RelationComparisonResult::default(),
            report_unreliable_mapper: MapperId::NIL,
            report_unmeasurable_mapper: MapperId::NIL,
            restrictive_mapper: MapperId::NIL,
            permissive_mapper: MapperId::NIL,
            empty_object_type: TypeId::NIL,
            empty_jsx_object_type: TypeId::NIL,
            empty_fresh_jsx_object_type: TypeId::NIL,
            empty_type_literal_type: TypeId::NIL,
            unknown_empty_object_type: TypeId::NIL,
            unknown_union_type: TypeId::NIL,
            empty_generic_type: TypeId::NIL,
            any_function_type: TypeId::NIL,
            no_constraint_type: TypeId::NIL,
            circular_constraint_type: TypeId::NIL,
            resolving_default_type: TypeId::NIL,
            marker_super_type: TypeId::NIL,
            marker_sub_type: TypeId::NIL,
            marker_other_type: TypeId::NIL,
            marker_super_type_for_check: TypeId::NIL,
            marker_sub_type_for_check: TypeId::NIL,
            no_type_predicate: TypePredicateId::NIL,
            any_signature: SignatureId::NIL,
            unknown_signature: SignatureId::NIL,
            resolving_signature: SignatureId::NIL,
            silent_never_signature: SignatureId::NIL,
            cached_arguments_referenced: FxHashMap::default(),
            enum_number_index_info: IndexInfoId::NIL,
            any_base_type_index_info: IndexInfoId::NIL,
            pattern_ambient_modules: Vec::new(),
            pattern_ambient_module_augmentations: SymbolTable::NIL,
            pattern_ambient_module_augmentation_targets: SymbolTable::NIL,
            module_import_attributes_types: FxHashMap::default(),
            global_object_type: TypeId::NIL,
            global_function_type: TypeId::NIL,
            global_callable_function_type: TypeId::NIL,
            global_newable_function_type: TypeId::NIL,
            augment_filters: Default::default(),
            global_array_type: TypeId::NIL,
            global_readonly_array_type: TypeId::NIL,
            global_string_type: TypeId::NIL,
            global_number_type: TypeId::NIL,
            global_boolean_type: TypeId::NIL,
            global_reg_exp_type: TypeId::NIL,
            global_this_type: TypeId::NIL,
            any_array_type: TypeId::NIL,
            auto_array_type: TypeId::NIL,
            any_readonly_array_type: TypeId::NIL,
            deferred_global_import_meta_expression_type: TypeId::NIL,
            contextual_binding_patterns: Vec::new(),
            empty_string_type: TypeId::NIL,
            zero_type: TypeId::NIL,
            zero_big_int_type: TypeId::NIL,
            typeof_type: TypeId::NIL,
            type_resolutions: Vec::new(),
            resolution_start: 0,
            variance_stack: Vec::new(),
            call_resolution_stack: Vec::new(),
            apparent_argument_count: None,
            last_get_combined_node_flags_node: Node::NIL,
            last_get_combined_node_flags_result: NodeFlags::default(),
            last_get_combined_modifier_flags_node: Node::NIL,
            last_get_combined_modifier_flags_result: ModifierFlags::default(),
            freeinference_state: None,
            free_flow_state: None,
            flow_loop_cache: FxHashMap::default(),
            flow_loop_stack: Vec::new(),
            shared_flows: Vec::new(),
            antecedent_types: Vec::new(),
            flow_analysis_disabled: false,
            flow_invocation_count: 0,
            flow_type_cache: FxHashMap::default(),
            control_flow_containers: Box::new(
                [(Node::NIL, Node::NIL); CONTROL_FLOW_CONTAINER_SLOTS],
            ),
            last_flow_node: FlowNodeId::NIL,
            last_flow_node_reachable: false,
            flow_node_reachable: FxHashMap::default(),
            flow_node_post_super: FxHashMap::default(),
            renamed_binding_elements_in_types: Vec::new(),
            contextual_infos: Vec::new(),
            inference_context_infos: Vec::new(),
            awaited_type_stack: Vec::new(),
            reverse_mapped_source_stack: Vec::new(),
            reverse_mapped_target_stack: Vec::new(),
            reverse_expanding_flags: ExpandingFlags::default(),
            free_relater: None,
            subtype_relation: Rc::new(RefCell::new(Relation::default())),
            strict_subtype_relation: Rc::new(RefCell::new(Relation::default())),
            assignable_relation: Rc::new(RefCell::new(Relation::default())),
            comparable_relation: Rc::new(RefCell::new(Relation::default())),
            identity_relation: Rc::new(RefCell::new(Relation::default())),
            enum_relation: FxHashMap::default(),
            get_global_es_symbol_type: nil_global_type_fn(),
            get_global_big_int_type: nil_global_type_fn(),
            get_global_import_meta_type: nil_global_type_fn(),
            get_global_import_attributes_type: nil_global_type_fn(),
            get_global_import_attributes_type_checked: nil_global_type_fn(),
            get_global_non_nullable_type_alias_or_nil: nil_global_symbol_fn(),
            get_global_extract_symbol: nil_global_symbol_fn(),
            get_global_disposable_type: nil_global_type_fn(),
            get_global_async_disposable_type: nil_global_type_fn(),
            get_global_awaited_symbol: nil_global_symbol_fn(),
            get_global_awaited_symbol_or_nil: nil_global_symbol_fn(),
            get_global_na_n_symbol_or_nil: nil_global_symbol_fn(),
            get_global_record_symbol: nil_global_symbol_fn(),
            get_global_template_strings_array_type: nil_global_type_fn(),
            get_global_es_symbol_constructor_symbol_or_nil: nil_global_symbol_fn(),
            get_global_es_symbol_constructor_type_symbol_or_nil: nil_global_symbol_fn(),
            get_global_import_call_options_type: nil_global_type_fn(),
            get_global_import_call_options_type_checked: nil_global_type_fn(),
            get_global_promise_type: nil_global_type_fn(),
            get_global_promise_type_checked: nil_global_type_fn(),
            get_global_promise_like_type: nil_global_type_fn(),
            get_global_promise_constructor_symbol: nil_global_symbol_fn(),
            get_global_promise_constructor_symbol_or_nil: nil_global_symbol_fn(),
            get_global_omit_symbol: nil_global_symbol_fn(),
            get_global_no_infer_symbol_or_nil: nil_global_symbol_fn(),
            get_global_iterator_type: nil_global_type_fn(),
            get_global_iterable_type: nil_global_type_fn(),
            get_global_iterable_type_checked: nil_global_type_fn(),
            get_global_iterable_iterator_type: nil_global_type_fn(),
            get_global_iterable_iterator_type_checked: nil_global_type_fn(),
            get_global_iterator_object_type: nil_global_type_fn(),
            get_global_generator_type: nil_global_type_fn(),
            get_global_async_iterator_type: nil_global_type_fn(),
            get_global_async_iterable_type: nil_global_type_fn(),
            get_global_async_iterable_type_checked: nil_global_type_fn(),
            get_global_async_iterable_iterator_type: nil_global_type_fn(),
            get_global_async_iterable_iterator_type_checked: nil_global_type_fn(),
            get_global_async_iterator_object_type: nil_global_type_fn(),
            get_global_async_generator_type: nil_global_type_fn(),
            get_global_iterator_yield_result_type: nil_global_type_fn(),
            get_global_iterator_return_result_type: nil_global_type_fn(),
            get_global_typed_property_descriptor_type: nil_global_type_fn(),
            get_global_class_decorator_context_type: nil_global_type_fn(),
            get_global_class_method_decorator_context_type: nil_global_type_fn(),
            get_global_class_getter_decorator_context_type: nil_global_type_fn(),
            get_global_class_setter_decorator_context_type: nil_global_type_fn(),
            get_global_class_accessor_decorator_contxt_type: nil_global_type_fn(),
            get_global_class_accessor_decorator_context_type: nil_global_type_fn(),
            get_global_class_accessor_decorator_target_type: nil_global_type_fn(),
            get_global_class_accessor_decorator_result_type: nil_global_type_fn(),
            get_global_class_field_decorator_context_type: nil_global_type_fn(),
            sync_iteration_types_resolver: nil_iteration_types_resolver(),
            async_iteration_types_resolver: nil_iteration_types_resolver(),
            is_primitive_or_object_or_empty_type: nil_type_test_fn(),
            contains_missing_type: nil_type_test_fn(),
            is_string_index_signature_only_type: nil_type_test_fn(),
            mark_node_assignments: nil_node_test_fn(),
            compare_types_assignable: nil_type_comparer(),
            _jsx_namespace: String::new(),
            _jsx_factory_entity: Node::NIL,
            skip_direct_inference_nodes: FxHashSet::default(),
            ctx: None,
            packages_map: FxHashMap::default(),
            active_mappers: Vec::new(),
            active_type_mappers_caches: Vec::new(),
            ambient_modules_once: false,
            ambient_modules: Vec::new(),
            within_unreachable_code: false,
            reported_unreachable_nodes: FxHashSet::default(),
            non_existent_properties: FxHashSet::default(),
            deferred_diagnostic_callbacks: Vec::new(),
            type_to_string_nodebuilder: None,
            named_members_scratch: Default::default(),
            named_members_orders: FxHashMap::default(),
            alternative_module_import_misses: FxHashSet::default(),
            matching_reference_memo: Default::default(),
            flow_skip: Default::default(),
            merge_version: 0,
            union_subset_answers: FxHashMap::default(),
            symbols,
            types: ChunkedArena::with_nil(Type::default()),
            object_type_instantiations: vec![InstantiationMap::default()],
            signatures: vec![Signature::default()],
            index_infos: vec![IndexInfo::default()],
            type_predicates: vec![TypePredicate::default()],
            mappers: ChunkedArena::with_nil(TypeMapper::default()),
            inference_contexts: ChunkedArena::with_nil(InferenceContext::default()),
            // Go: compiler/checkerpool.go:104 makes the tracer when the pool
            // has a tracing session; NewChecker stores it (checker.go:905).
            tracer: crate::tracing::new_checker_tracer(checker_index),
            effect_links: None,
            effect_relation_errors: FxHashMap::default(),
        };
        // Closure optimization
        c.compare_symbols = Rc::new(|c: &mut Checker, s1: SymbolId, s2: SymbolId| -> i32 {
            c.compare_symbols_worker(s1, s2)
        });
        // Closure optimization
        c.compare_symbol_chains =
            Rc::new(|c: &mut Checker, a: &[SymbolId], b: &[SymbolId]| -> i32 {
                c.compare_symbol_chains_worker(a, b)
            });
        // PORT: Go `make(ast.SymbolTable, n)` sizes the map; the arena table
        // reserves the same capacity. `countGlobalSymbols` (checker_p02) is a
        // `Checker` method because it reads the symbol arena.
        let global_symbol_count =
            usize::try_from(c.count_global_symbols(&c.files)).expect("negative symbol count");
        c.globals = c.symbols.new_table_with_capacity(global_symbol_count);
        c.evaluate = new_evaluator(
            Rc::new(
                |c: &mut Checker, expr: Node, location: Node| -> EvaluatorResult {
                    c.evaluate_entity(expr, location)
                },
            ),
            OuterExpressionKinds::OEK_PARENTHESES,
        );
        c.undefined_symbol = c.new_symbol(SymbolFlags::PROPERTY, "undefined");
        c.arguments_symbol = c.new_symbol(SymbolFlags::PROPERTY, "arguments");
        c.require_symbol = c.new_symbol(SymbolFlags::PROPERTY, "require");
        c.unknown_symbol = c.new_symbol(SymbolFlags::PROPERTY, "unknown");
        c.global_this_symbol =
            c.new_symbol_ex(SymbolFlags::MODULE, "globalThis", CheckFlags::READONLY);
        let globals = c.globals;
        let global_this_symbol = c.global_this_symbol;
        c.sym_mut(global_this_symbol).exports = globals;
        let global_this_name = c.sym(global_this_symbol).name.clone();
        c.symbols.set(globals, global_this_name, global_this_symbol);
        // PORT: Go stores the bound method `resolver.Resolve`. The resolver is
        // moved into the closure, which forwards the checker.
        let resolver = c.create_name_resolver();
        c.resolve_name = Rc::new(
            move |c: &mut Checker,
                  location: Node,
                  name: &str,
                  meaning: SymbolFlags,
                  name_not_found_message: Option<NameNotFound>,
                  is_use: bool,
                  exclude_globals: bool|
                  -> SymbolId {
                resolver.resolve(
                    c,
                    location,
                    name,
                    meaning,
                    name_not_found_message,
                    is_use,
                    exclude_globals,
                )
            },
        );
        let suggestion_resolver = c.create_name_resolver_for_suggestion();
        c.resolve_name_for_symbol_suggestion = Rc::new(
            move |c: &mut Checker,
                  location: Node,
                  name: &str,
                  meaning: SymbolFlags,
                  name_not_found_message: Option<NameNotFound>,
                  is_use: bool,
                  exclude_globals: bool|
                  -> SymbolId {
                suggestion_resolver.resolve(
                    c,
                    location,
                    name,
                    meaning,
                    name_not_found_message,
                    is_use,
                    exclude_globals,
                )
            },
        );
        c.any_type = c.new_intrinsic_type(TypeFlags::ANY, "any");
        c.auto_type =
            c.new_intrinsic_type_ex(TypeFlags::ANY, "any", ObjectFlags::NON_INFERRABLE_TYPE);
        c.wildcard_type = c.new_intrinsic_type(TypeFlags::ANY, "any");
        c.blocked_string_type = c.new_intrinsic_type(TypeFlags::ANY, "any");
        c.error_type = c.new_intrinsic_type(TypeFlags::ANY, "error");
        c.unresolved_type = c.new_intrinsic_type(TypeFlags::ANY, "unresolved");
        c.non_inferrable_any_type =
            c.new_intrinsic_type_ex(TypeFlags::ANY, "any", ObjectFlags::CONTAINS_WIDENING_TYPE);
        c.intrinsic_marker_type = c.new_intrinsic_type(TypeFlags::ANY, "intrinsic");
        c.unknown_type = c.new_intrinsic_type(TypeFlags::UNKNOWN, "unknown");
        c.undefined_type = c.new_intrinsic_type(TypeFlags::UNDEFINED, "undefined");
        c.undefined_widening_type = c.create_widening_type(c.undefined_type);
        c.missing_type = c.new_intrinsic_type(TypeFlags::UNDEFINED, "undefined");
        c.undefined_or_missing_type = if c.exact_optional_property_types {
            c.missing_type
        } else {
            c.undefined_type
        };
        c.optional_type = c.new_intrinsic_type(TypeFlags::UNDEFINED, "undefined");
        c.null_type = c.new_intrinsic_type(TypeFlags::NULL, "null");
        c.null_widening_type = c.create_widening_type(c.null_type);
        c.string_type = c.new_intrinsic_type(TypeFlags::STRING, "string");
        c.number_type = c.new_intrinsic_type(TypeFlags::NUMBER, "number");
        c.bigint_type = c.new_intrinsic_type(TypeFlags::BIG_INT, "bigint");
        // PORT: Go `value any` is `Option<LiteralValue>` (see `LiteralType`).
        c.regular_false_type = c.new_literal_type(
            TypeFlags::BOOLEAN_LITERAL,
            Some(LiteralValue::Bool(false)),
            TypeId::NIL,
        );
        c.false_type = c.new_literal_type(
            TypeFlags::BOOLEAN_LITERAL,
            Some(LiteralValue::Bool(false)),
            c.regular_false_type,
        );
        let (regular_false_type, false_type) = (c.regular_false_type, c.false_type);
        c.ty_mut(regular_false_type)
            .as_literal_type_mut()
            .fresh_type = false_type;
        c.ty_mut(false_type).as_literal_type_mut().fresh_type = false_type;
        c.regular_true_type = c.new_literal_type(
            TypeFlags::BOOLEAN_LITERAL,
            Some(LiteralValue::Bool(true)),
            TypeId::NIL,
        );
        c.true_type = c.new_literal_type(
            TypeFlags::BOOLEAN_LITERAL,
            Some(LiteralValue::Bool(true)),
            c.regular_true_type,
        );
        let (regular_true_type, true_type) = (c.regular_true_type, c.true_type);
        c.ty_mut(regular_true_type).as_literal_type_mut().fresh_type = true_type;
        c.ty_mut(true_type).as_literal_type_mut().fresh_type = true_type;
        c.boolean_type = c.get_union_type(&[c.regular_false_type, c.regular_true_type]);
        c.es_symbol_type = c.new_intrinsic_type(TypeFlags::ES_SYMBOL, "symbol");
        c.void_type = c.new_intrinsic_type(TypeFlags::VOID, "void");
        c.never_type = c.new_intrinsic_type(TypeFlags::NEVER, "never");
        c.silent_never_type =
            c.new_intrinsic_type_ex(TypeFlags::NEVER, "never", ObjectFlags::NON_INFERRABLE_TYPE);
        c.implicit_never_type = c.new_intrinsic_type(TypeFlags::NEVER, "never");
        c.unreachable_never_type = c.new_intrinsic_type(TypeFlags::NEVER, "never");
        c.non_primitive_type = c.new_intrinsic_type(TypeFlags::NON_PRIMITIVE, "object");
        c.string_or_number_type = c.get_union_type(&[c.string_type, c.number_type]);
        c.string_number_symbol_type =
            c.get_union_type(&[c.string_type, c.number_type, c.es_symbol_type]);
        c.number_or_big_int_type = c.get_union_type(&[c.number_type, c.bigint_type]);
        // The `${number}` type
        c.numeric_string_type =
            c.get_template_literal_type(&[String::new(), String::new()], &[c.number_type]);
        c.template_constraint_type = c.get_union_type(&[
            c.string_type,
            c.number_type,
            c.boolean_type,
            c.bigint_type,
            c.null_type,
            c.undefined_type,
        ]);
        // Special `never` flagged by union reduction to behave as a literal
        c.unique_literal_type = c.new_intrinsic_type(TypeFlags::NEVER, "never");
        c.unique_literal_mapper =
            c.new_function_type_mapper(Rc::new(|c: &mut Checker, t: TypeId| -> TypeId {
                c.get_unique_literal_type_for_type_parameter(t)
            }));
        c.report_unreliable_mapper =
            c.new_function_type_mapper(Rc::new(|c: &mut Checker, t: TypeId| -> TypeId {
                c.report_unreliable_worker(t)
            }));
        c.report_unmeasurable_mapper =
            c.new_function_type_mapper(Rc::new(|c: &mut Checker, t: TypeId| -> TypeId {
                c.report_unmeasurable_worker(t)
            }));
        c.restrictive_mapper =
            c.new_function_type_mapper(Rc::new(|c: &mut Checker, t: TypeId| -> TypeId {
                c.restrictive_mapper_worker(t)
            }));
        c.permissive_mapper =
            c.new_function_type_mapper(Rc::new(|c: &mut Checker, t: TypeId| -> TypeId {
                c.permissive_mapper_worker(t)
            }));
        c.empty_object_type = c.new_anonymous_type(
            SymbolId::NIL, /*symbol*/
            SymbolTable::NIL,
            &[],
            &[],
            &[],
        );
        c.empty_jsx_object_type = c.new_anonymous_type(
            SymbolId::NIL, /*symbol*/
            SymbolTable::NIL,
            &[],
            &[],
            &[],
        );
        c.empty_fresh_jsx_object_type = c.new_anonymous_type(
            SymbolId::NIL, /*symbol*/
            SymbolTable::NIL,
            &[],
            &[],
            &[],
        );
        let type_literal_symbol =
            c.new_symbol(SymbolFlags::TYPE_LITERAL, INTERNAL_SYMBOL_NAME_TYPE);
        c.empty_type_literal_type =
            c.new_anonymous_type(type_literal_symbol, SymbolTable::NIL, &[], &[], &[]);
        c.unknown_empty_object_type = c.new_anonymous_type(
            SymbolId::NIL, /*symbol*/
            SymbolTable::NIL,
            &[],
            &[],
            &[],
        );
        c.unknown_union_type = c.create_unknown_union_type();
        c.empty_generic_type = c.new_anonymous_type(
            SymbolId::NIL, /*symbol*/
            SymbolTable::NIL,
            &[],
            &[],
            &[],
        );
        let empty_generic_type = c.empty_generic_type;
        c.object_instantiations_mut(empty_generic_type);
        c.any_function_type = c.new_anonymous_type(
            SymbolId::NIL, /*symbol*/
            SymbolTable::NIL,
            &[],
            &[],
            &[],
        );
        let any_function_type = c.any_function_type;
        c.ty_mut(any_function_type).object_flags |= ObjectFlags::NON_INFERRABLE_TYPE;
        c.no_constraint_type = c.new_anonymous_type(
            SymbolId::NIL, /*symbol*/
            SymbolTable::NIL,
            &[],
            &[],
            &[],
        );
        c.circular_constraint_type = c.new_anonymous_type(
            SymbolId::NIL, /*symbol*/
            SymbolTable::NIL,
            &[],
            &[],
            &[],
        );
        c.resolving_default_type = c.new_anonymous_type(
            SymbolId::NIL, /*symbol*/
            SymbolTable::NIL,
            &[],
            &[],
            &[],
        );
        c.marker_super_type = c.new_type_parameter(SymbolId::NIL);
        c.marker_sub_type = c.new_type_parameter(SymbolId::NIL);
        let (marker_sub_type, marker_super_type) = (c.marker_sub_type, c.marker_super_type);
        c.ty_mut(marker_sub_type).as_type_parameter_mut().constraint = marker_super_type;
        c.marker_other_type = c.new_type_parameter(SymbolId::NIL);
        c.marker_super_type_for_check = c.new_type_parameter(SymbolId::NIL);
        c.marker_sub_type_for_check = c.new_type_parameter(SymbolId::NIL);
        let (marker_sub_type_for_check, marker_super_type_for_check) =
            (c.marker_sub_type_for_check, c.marker_super_type_for_check);
        c.ty_mut(marker_sub_type_for_check)
            .as_type_parameter_mut()
            .constraint = marker_super_type_for_check;
        // PORT: Go builds `&TypePredicate{...}` directly (no constructor), so
        // it is pushed into the arena here.
        c.no_type_predicate = TypePredicateId(
            u32::try_from(c.type_predicates.len()).expect("type predicate overflow"),
        );
        let any_type = c.any_type;
        c.type_predicates.push(TypePredicate {
            kind: TypePredicateKind::IDENTIFIER,
            parameter_index: 0,
            parameter_name: "<<unresolved>>".to_string(),
            t: any_type,
        });
        c.any_signature = c.new_signature(
            SignatureFlags::NONE,
            Node::NIL,
            &[],
            SymbolId::NIL,
            &[],
            c.any_type,
            TypePredicateId::NIL,
            0,
        );
        c.unknown_signature = c.new_signature(
            SignatureFlags::NONE,
            Node::NIL,
            &[],
            SymbolId::NIL,
            &[],
            c.error_type,
            TypePredicateId::NIL,
            0,
        );
        c.resolving_signature = c.new_signature(
            SignatureFlags::NONE,
            Node::NIL,
            &[],
            SymbolId::NIL,
            &[],
            c.any_type,
            TypePredicateId::NIL,
            0,
        );
        c.silent_never_signature = c.new_signature(
            SignatureFlags::NONE,
            Node::NIL,
            &[],
            SymbolId::NIL,
            &[],
            c.silent_never_type,
            TypePredicateId::NIL,
            0,
        );
        // PORT: Go builds `&IndexInfo{...}` directly (no constructor), so they
        // are pushed into the arena here.
        c.enum_number_index_info =
            IndexInfoId(u32::try_from(c.index_infos.len()).expect("index info overflow"));
        let (number_type, string_type) = (c.number_type, c.string_type);
        c.index_infos.push(IndexInfo {
            key_type: number_type,
            value_type: string_type,
            is_readonly: true,
            ..IndexInfo::default()
        });
        c.any_base_type_index_info =
            IndexInfoId(u32::try_from(c.index_infos.len()).expect("index info overflow"));
        c.index_infos.push(IndexInfo {
            key_type: string_type,
            value_type: any_type,
            is_readonly: false,
            ..IndexInfo::default()
        });
        c.empty_string_type = c.get_string_literal_type("");
        c.zero_type = c.get_number_literal_type(Number(0.0));
        c.zero_big_int_type = c.get_big_int_literal_type(PseudoBigInt::default());
        // PORT: Go `slices.Sorted(maps.Keys(typeofNEFacts))` (flow.go:623).
        // The sorted keys are written out; they must match that map.
        let typeof_types: Vec<TypeId> = [
            "bigint",
            "boolean",
            "function",
            "number",
            "object",
            "string",
            "symbol",
            "undefined",
        ]
        .iter()
        .map(|s| c.get_string_literal_type(s))
        .collect();
        c.typeof_type = c.get_union_type(&typeof_types);
        c.get_global_es_symbol_type =
            c.get_global_type_resolver("Symbol", 0 /*arity*/, false /*reportErrors*/);
        c.get_global_big_int_type =
            c.get_global_type_resolver("BigInt", 0 /*arity*/, false /*reportErrors*/);
        c.get_global_import_meta_type =
            c.get_global_type_resolver("ImportMeta", 0 /*arity*/, true /*reportErrors*/);
        c.get_global_import_attributes_type = c.get_global_type_resolver(
            "ImportAttributes",
            0,     /*arity*/
            false, /*reportErrors*/
        );
        c.get_global_import_attributes_type_checked = c.get_global_type_resolver(
            "ImportAttributes",
            0,    /*arity*/
            true, /*reportErrors*/
        );
        c.get_global_non_nullable_type_alias_or_nil = c.get_global_type_alias_resolver(
            "NonNullable",
            1,     /*arity*/
            false, /*reportErrors*/
        );
        c.get_global_extract_symbol = c.get_global_type_alias_resolver(
            "Extract", 2,    /*arity*/
            true, /*reportErrors*/
        );
        c.get_global_disposable_type =
            c.get_global_type_resolver("Disposable", 0 /*arity*/, true /*reportErrors*/);
        c.get_global_async_disposable_type = c.get_global_type_resolver(
            "AsyncDisposable",
            0,    /*arity*/
            true, /*reportErrors*/
        );
        c.get_global_awaited_symbol = c.get_global_type_alias_resolver(
            "Awaited", 1,    /*arity*/
            true, /*reportErrors*/
        );
        c.get_global_awaited_symbol_or_nil = c.get_global_type_alias_resolver(
            "Awaited", 1,     /*arity*/
            false, /*reportErrors*/
        );
        c.get_global_na_n_symbol_or_nil =
            c.get_global_value_symbol_resolver("NaN", false /*reportErrors*/);
        c.get_global_record_symbol = c
            .get_global_type_alias_resolver("Record", 2 /*arity*/, true /*reportErrors*/);
        c.get_global_template_strings_array_type = c.get_global_type_resolver(
            "TemplateStringsArray",
            0,    /*arity*/
            true, /*reportErrors*/
        );
        c.get_global_es_symbol_constructor_symbol_or_nil =
            c.get_global_value_symbol_resolver("Symbol", false /*reportErrors*/);
        c.get_global_es_symbol_constructor_type_symbol_or_nil =
            c.get_global_type_symbol_resolver("SymbolConstructor", false /*reportErrors*/);
        c.get_global_import_call_options_type = c.get_global_type_resolver(
            "ImportCallOptions",
            0,     /*arity*/
            false, /*reportErrors*/
        );
        c.get_global_import_call_options_type_checked = c.get_global_type_resolver(
            "ImportCallOptions",
            0,    /*arity*/
            true, /*reportErrors*/
        );
        c.get_global_promise_type =
            c.get_global_type_resolver("Promise", 1 /*arity*/, false /*reportErrors*/);
        c.get_global_promise_type_checked =
            c.get_global_type_resolver("Promise", 1 /*arity*/, true /*reportErrors*/);
        c.get_global_promise_like_type =
            c.get_global_type_resolver("PromiseLike", 1 /*arity*/, true /*reportErrors*/);
        c.get_global_promise_constructor_symbol =
            c.get_global_value_symbol_resolver("Promise", true /*reportErrors*/);
        c.get_global_promise_constructor_symbol_or_nil =
            c.get_global_value_symbol_resolver("Promise", false /*reportErrors*/);
        c.get_global_omit_symbol =
            c.get_global_type_alias_resolver("Omit", 2 /*arity*/, true /*reportErrors*/);
        c.get_global_no_infer_symbol_or_nil = c.get_global_type_alias_resolver(
            "NoInfer", 1,     /*arity*/
            false, /*reportErrors*/
        );
        c.get_global_iterator_type =
            c.get_global_type_resolver("Iterator", 3 /*arity*/, false /*reportErrors*/);
        c.get_global_iterable_type =
            c.get_global_type_resolver("Iterable", 3 /*arity*/, false /*reportErrors*/);
        c.get_global_iterable_type_checked =
            c.get_global_type_resolver("Iterable", 3 /*arity*/, true /*reportErrors*/);
        c.get_global_iterable_iterator_type = c.get_global_type_resolver(
            "IterableIterator",
            3,     /*arity*/
            false, /*reportErrors*/
        );
        c.get_global_iterable_iterator_type_checked = c.get_global_type_resolver(
            "IterableIterator",
            3,    /*arity*/
            true, /*reportErrors*/
        );
        c.get_global_iterator_object_type = c.get_global_type_resolver(
            "IteratorObject",
            3,     /*arity*/
            false, /*reportErrors*/
        );
        c.get_global_generator_type =
            c.get_global_type_resolver("Generator", 3 /*arity*/, false /*reportErrors*/);
        c.get_global_async_iterator_type = c.get_global_type_resolver(
            "AsyncIterator",
            3,     /*arity*/
            false, /*reportErrors*/
        );
        c.get_global_async_iterable_type = c.get_global_type_resolver(
            "AsyncIterable",
            3,     /*arity*/
            false, /*reportErrors*/
        );
        c.get_global_async_iterable_type_checked = c.get_global_type_resolver(
            "AsyncIterable",
            3,    /*arity*/
            true, /*reportErrors*/
        );
        c.get_global_async_iterable_iterator_type = c.get_global_type_resolver(
            "AsyncIterableIterator",
            3,     /*arity*/
            false, /*reportErrors*/
        );
        c.get_global_async_iterable_iterator_type_checked = c.get_global_type_resolver(
            "AsyncIterableIterator",
            3,    /*arity*/
            true, /*reportErrors*/
        );
        c.get_global_async_iterator_object_type = c.get_global_type_resolver(
            "AsyncIteratorObject",
            3,     /*arity*/
            false, /*reportErrors*/
        );
        c.get_global_async_generator_type = c.get_global_type_resolver(
            "AsyncGenerator",
            3,     /*arity*/
            false, /*reportErrors*/
        );
        c.get_global_iterator_yield_result_type = c.get_global_type_resolver(
            "IteratorYieldResult",
            1,     /*arity*/
            false, /*reportErrors*/
        );
        c.get_global_iterator_return_result_type = c.get_global_type_resolver(
            "IteratorReturnResult",
            1,     /*arity*/
            false, /*reportErrors*/
        );
        c.get_global_typed_property_descriptor_type = c.get_global_type_resolver(
            "TypedPropertyDescriptor",
            1,    /*arity*/
            true, /*reportErrors*/
        );
        c.get_global_class_decorator_context_type = c.get_global_type_resolver(
            "ClassDecoratorContext",
            1,    /*arity*/
            true, /*reportErrors*/
        );
        c.get_global_class_method_decorator_context_type = c.get_global_type_resolver(
            "ClassMethodDecoratorContext",
            2,    /*arity*/
            true, /*reportErrors*/
        );
        c.get_global_class_getter_decorator_context_type = c.get_global_type_resolver(
            "ClassGetterDecoratorContext",
            2,    /*arity*/
            true, /*reportErrors*/
        );
        c.get_global_class_setter_decorator_context_type = c.get_global_type_resolver(
            "ClassSetterDecoratorContext",
            2,    /*arity*/
            true, /*reportErrors*/
        );
        c.get_global_class_accessor_decorator_context_type = c.get_global_type_resolver(
            "ClassAccessorDecoratorContext",
            2,    /*arity*/
            true, /*reportErrors*/
        );
        c.get_global_class_accessor_decorator_target_type = c.get_global_type_resolver(
            "ClassAccessorDecoratorTarget",
            2,    /*arity*/
            true, /*reportErrors*/
        );
        c.get_global_class_accessor_decorator_result_type = c.get_global_type_resolver(
            "ClassAccessorDecoratorResult",
            2,    /*arity*/
            true, /*reportErrors*/
        );
        c.get_global_class_field_decorator_context_type = c.get_global_type_resolver(
            "ClassFieldDecoratorContext",
            2,    /*arity*/
            true, /*reportErrors*/
        );
        c.initialize_closures();
        c.initialize_iteration_resolvers();
        c.initialize_checker();
        c
    }
}
