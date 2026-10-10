//! Port of Go `checker/types.go`: `Type`, `TypeData` and every type struct,
//! `Signature`, `IndexInfo`, `TypePredicate`, `TypeAlias`, `ConditionalRoot`,
//! `TupleElementInfo`, and every `*Links` struct.
//!
//! PORT: the Go flag and enum types declared in `types.go` (`ParseFlags`,
//! `SignatureKind`, `ContextFlags`, `TypeFormatFlags`, `SymbolFormatFlags`,
//! `ExternalEmitHelpers`, `VarianceFlags`, `AccessFlags`, `NodeCheckFlags`,
//! `TypeFlags`, `ObjectFlags`, `ElementFlags`, `IndexFlags`,
//! `SignatureFlags`, `TypePredicateKind`, `Ternary`,
//! `MembersOrExportsResolutionKind`) are generated in `crate::flags`. This
//! file only adds the Go constants the generator did not emit.
//!
//! PORT: Go `TypeId` and `SignatureId` are the arena handles in `crate::core`.
//!
//! PORT: all Go struct fields are `pub` so other port files can read and
//! write them like Go code in the same package does.

use crate::jsnum::{Number, PseudoBigInt};
use crate::leak_arena::LeakArena;
use crate::prelude::*;

/// An immutable list. It works like a Go slice over an array that is never
/// written again: a clone or a sub-slice copies no elements (a sub-slice of
/// an `Owned` list is a copy). An empty list does not allocate. An empty
/// list is Go nil (`is_nil`) unless `empty_non_nil` made it.
///
/// PORT: Go returns resolved member, signature, index info and type argument
/// slices without a copy. Callers read the list through `Deref<[T]>`.
///
/// PORT: layout only. A list of up to `SHARED_LIST_INLINE` elements is kept
/// inline, so it needs no heap copy and a read does not follow a pointer.
/// The list stays 24 bytes (asserted below). All element types are `Copy`
/// ids, and `Default` fills the unused inline slots. A longer list is in the
/// checker arena of a one-program process, else owned (`use_checker_arena`).
///
/// PERF: only an `Owned` list holds a resource, and only a multi-program
/// process makes one. `repr` is a `ManuallyDrop`, so a drop is the one tag
/// test in `Drop::drop`, and the `Owned` clone and drop code is out of line
/// (`clone_owned`, `drop_owned`) and not in every caller.
pub struct SharedList<T: 'static> {
    repr: std::mem::ManuallyDrop<SharedListRepr<T>>,
}

/// Number of elements a `SharedList` keeps inline.
const SHARED_LIST_INLINE: usize = 3;

enum SharedListRepr<T: 'static> {
    /// A longer list, in this thread's checker arena. Never freed.
    Arena(&'static [T]),
    /// A longer list of a multi-program process, freed with its last copy.
    /// `SharedList::drop` frees it.
    Owned(Rc<[T]>),
    /// `items[..len]` is the list. Empty is `len == 0`.
    Inline {
        len: InlineLen,
        items: [T; SHARED_LIST_INLINE],
    },
}

/// The length of an inline `SharedList`.
///
/// PERF: an enum and not a `u8`, so the compiler knows that the length is at
/// most `SHARED_LIST_INLINE` and `deref` has no bounds check.
#[derive(Clone, Copy)]
#[repr(u8)]
enum InlineLen {
    Zero = 0,
    One = 1,
    Two = 2,
    Three = 3,
}

// A list must stay 24 bytes, or `Type` grows past 2 cache lines. 32-bit
// targets (wasm32) have smaller pointers and do not check this.
#[cfg(target_pointer_width = "64")]
const _: () = assert!(std::mem::size_of::<SharedList<TypeId>>() == 24);
#[cfg(target_pointer_width = "64")]
const _: () = assert!(std::mem::size_of::<SharedList<SymbolId>>() == 24);
#[cfg(target_pointer_width = "64")]
const _: () = assert!(std::mem::size_of::<SharedList<SignatureId>>() == 24);
#[cfg(target_pointer_width = "64")]
const _: () = assert!(std::mem::size_of::<SharedList<IndexInfoId>>() == 24);
#[cfg(target_pointer_width = "64")]
const _: () = assert!(std::mem::size_of::<SharedList<VarianceFlags>>() == 24);

impl<T> SharedList<T> {
    #[inline(always)]
    fn from_repr(repr: SharedListRepr<T>) -> Self {
        Self {
            repr: std::mem::ManuallyDrop::new(repr),
        }
    }

    /// Go's empty non-nil slice: an empty list for which `is_nil` is false.
    /// Its clones keep this state.
    ///
    /// PORT: a Go cache that tests `== nil` for "not resolved" stores this
    /// for an empty result, so that the result is resolved once.
    pub const fn empty_non_nil() -> Self {
        Self {
            repr: std::mem::ManuallyDrop::new(SharedListRepr::Arena(&[])),
        }
    }

    /// Go `list == nil`: the list is empty and is not `empty_non_nil`.
    ///
    /// PORT: an empty list from `default` or `From` reads as nil. Use this
    /// only on a field whose resolver stores `non_nil` lists.
    #[inline]
    pub fn is_nil(&self) -> bool {
        matches!(
            &*self.repr,
            SharedListRepr::Inline {
                len: InlineLen::Zero,
                ..
            }
        )
    }
}

impl<T: Copy + Default> SharedList<T> {
    /// Go's non-nil slice of `items`: as `From`, but empty `items` give
    /// `empty_non_nil`.
    pub fn non_nil(items: &[T]) -> Self {
        if items.is_empty() {
            return Self::empty_non_nil();
        }
        Self::from(items)
    }

    /// An inline list of `items`, which has at most `SHARED_LIST_INLINE`
    /// elements.
    #[inline]
    fn inline(items: &[T]) -> Self {
        debug_assert!(items.len() <= SHARED_LIST_INLINE);
        // PERF: one arm for each length, so each element is one store into
        // the destination. LLVM turns a copy loop (or `copy_from_slice`) into
        // a `memcpy` call, and the list is then read back from the stack at
        // once. The arrays are `[T; 3]`, so they stop compiling if
        // `SHARED_LIST_INLINE` changes.
        let d = T::default();
        let (len, buffer): (InlineLen, [T; SHARED_LIST_INLINE]) = match *items {
            [] => (InlineLen::Zero, [d, d, d]),
            [a] => (InlineLen::One, [a, d, d]),
            [a, b] => (InlineLen::Two, [a, b, d]),
            [a, b, c] => (InlineLen::Three, [a, b, c]),
            _ => unreachable!("inline list longer than SHARED_LIST_INLINE"),
        };
        Self::from_repr(SharedListRepr::Inline { len, items: buffer })
    }

    /// A heap list with a copy of `items`, which has more than
    /// `SHARED_LIST_INLINE` elements.
    ///
    /// PERF: out of line, so the inlined `From<&[T]>` keeps only the short
    /// inline path at each call site. In a one-program process the copy goes
    /// in the checker arena, a pointer bump and not a malloc. It is never
    /// freed, like the types that hold it. Else it is an `Rc` copy, freed
    /// with the checker (`use_checker_arena`).
    #[inline(never)]
    fn heap(items: &[T]) -> Self {
        let repr = if use_checker_arena() {
            SharedListRepr::Arena(checker_arena().alloc_slice_copy(items))
        } else {
            SharedListRepr::Owned(Rc::from(items))
        };
        Self::from_repr(repr)
    }

    /// Go `core.Concatenate`: `a` followed by `b`. When one list is empty,
    /// the result is the other list itself, not a copy.
    pub fn concat(a: Self, b: Self) -> Self {
        if b.is_empty() {
            return a;
        }
        if a.is_empty() {
            return b;
        }
        let mut items = Vec::with_capacity(a.len() + b.len());
        items.extend_from_slice(&a);
        items.extend_from_slice(&b);
        Self::from(&items[..])
    }

    /// The sub-list `range` of this list. A long sub-list of an arena list
    /// shares the same storage; a short one is copied inline, and a long
    /// one of an owned list is a new owned copy.
    pub fn slice(&self, range: std::ops::Range<usize>) -> Self {
        assert!(range.start <= range.end && range.end <= self.len());
        if range.len() <= SHARED_LIST_INLINE {
            return Self::inline(&self[range]);
        }
        match &*self.repr {
            SharedListRepr::Arena(items) => Self::from_repr(SharedListRepr::Arena(&items[range])),
            SharedListRepr::Owned(items) => {
                Self::from_repr(SharedListRepr::Owned(Rc::from(&items[range])))
            }
            // An inline list is never longer than `SHARED_LIST_INLINE`.
            SharedListRepr::Inline { .. } => unreachable!(),
        }
    }

    /// The elements as a slice that does not borrow the list, so a caller
    /// can read them across a `&mut Checker` call: an arena list itself, or
    /// an inline list copied into `buf`. `None` for an owned list (a
    /// multi-program process): clone it instead.
    ///
    /// PERF: no `SharedList` copy. A clone kept on the stack can be moved
    /// as its bytes 2 to 16, and the reads of that copy then miss store
    /// forwarding (`has_base_type`: about 25% of its time).
    #[inline(always)]
    pub fn detach<'a>(&self, buf: &'a mut SharedListBuf<T>) -> Option<&'a [T]> {
        match &*self.repr {
            SharedListRepr::Arena(items) => Some(items),
            SharedListRepr::Inline { len, items } => {
                *buf = *items;
                Some(&buf[..*len as usize])
            }
            SharedListRepr::Owned(_) => None,
        }
    }
}

/// Room for the elements of an inline `SharedList` (`SharedList::detach`).
pub type SharedListBuf<T> = [T; SHARED_LIST_INLINE];

impl<T: Copy> Clone for SharedList<T> {
    #[inline]
    fn clone(&self) -> Self {
        Self::from_repr(match &*self.repr {
            SharedListRepr::Arena(items) => SharedListRepr::Arena(items),
            SharedListRepr::Owned(items) => clone_owned(items),
            SharedListRepr::Inline { len, items } => SharedListRepr::Inline {
                len: *len,
                items: *items,
            },
        })
    }
}

/// The clone of an `Owned` list (multi-program only), out of line.
#[cold]
#[inline(never)]
fn clone_owned<T: 'static>(items: &Rc<[T]>) -> SharedListRepr<T> {
    SharedListRepr::Owned(items.clone())
}

impl<T: 'static> Drop for SharedList<T> {
    // `repr` is a `ManuallyDrop`, so this is its only drop. An arena or
    // inline list owns nothing; an owned one is freed by `drop_owned`.
    #[inline]
    fn drop(&mut self) {
        if matches!(*self.repr, SharedListRepr::Owned(_)) {
            drop_owned(&mut self.repr);
        }
    }
}

/// Drops the `Rc` of an `Owned` list (multi-program only), out of line. The
/// empty arena list put in its place owns nothing.
#[cold]
#[inline(never)]
fn drop_owned<T: 'static>(repr: &mut SharedListRepr<T>) {
    drop(std::mem::replace(repr, SharedListRepr::Arena(&[])));
}

impl<T: Copy + Default> Default for SharedList<T> {
    #[inline]
    fn default() -> Self {
        Self::from_repr(SharedListRepr::Inline {
            len: InlineLen::Zero,
            items: [T::default(); SHARED_LIST_INLINE],
        })
    }
}

impl<T> std::ops::Deref for SharedList<T> {
    type Target = [T];

    #[inline]
    fn deref(&self) -> &[T] {
        match &*self.repr {
            SharedListRepr::Arena(items) => items,
            SharedListRepr::Owned(items) => items,
            SharedListRepr::Inline { len, items } => &items[..*len as usize],
        }
    }
}

impl<T: Copy + Default> From<Vec<T>> for SharedList<T> {
    fn from(items: Vec<T>) -> Self {
        Self::from(&items[..])
    }
}

impl<T: Copy + Default> From<&[T]> for SharedList<T> {
    #[inline]
    fn from(items: &[T]) -> Self {
        if items.len() <= SHARED_LIST_INLINE {
            return Self::inline(items);
        }
        Self::heap(items)
    }
}

/// Scratch list for the type set of a union or intersection under
/// construction. Most sets have at most 16 members, so they stay on the
/// stack. The created type still stores its own `SharedList` copy.
pub type TypeSet = smallvec::SmallVec<[TypeId; 16]>;

impl<T: std::fmt::Debug> std::fmt::Debug for SharedList<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.iter()).finish()
    }
}

impl<T: PartialEq> PartialEq for SharedList<T> {
    fn eq(&self, other: &Self) -> bool {
        **self == **other
    }
}

impl<T: Eq> Eq for SharedList<T> {}

impl<'a, T> IntoIterator for &'a SharedList<T> {
    type Item = &'a T;
    type IntoIter = std::slice::Iter<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<T: Copy> IntoIterator for SharedList<T> {
    type Item = T;
    type IntoIter = SharedListIter<T>;

    fn into_iter(self) -> Self::IntoIter {
        let back = self.len();
        SharedListIter {
            list: self,
            front: 0,
            back,
        }
    }
}

/// The owning iterator of a `SharedList`. It yields copies of the elements.
pub struct SharedListIter<T: 'static> {
    list: SharedList<T>,
    front: usize,
    back: usize,
}

impl<T: Copy> Iterator for SharedListIter<T> {
    type Item = T;

    fn next(&mut self) -> Option<T> {
        if self.front == self.back {
            return None;
        }
        let item = self.list[self.front];
        self.front += 1;
        Some(item)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let n = self.back - self.front;
        (n, Some(n))
    }
}

impl<T: Copy> DoubleEndedIterator for SharedListIter<T> {
    fn next_back(&mut self) -> Option<T> {
        if self.front == self.back {
            return None;
        }
        self.back -= 1;
        Some(self.list[self.back])
    }
}

impl<T: Copy> ExactSizeIterator for SharedListIter<T> {}

thread_local! {
    /// Long `SharedList`s and union and intersection data of a one-program
    /// process. They live as long as the checker, which is never dropped
    /// there (`program.rs` forgets it at thread exit), so each one costs a
    /// pointer bump, not a malloc. The arena is leaked and never frees, like
    /// the AST arena in `ast/store.rs`. rss2: it grows in fixed-size chunks
    /// (`LeakArena`).
    static CHECKER_ARENA: &'static LeakArena = LeakArena::leak();
}

/// Whether new checker data goes in this thread's `CHECKER_ARENA`: only in
/// a one-program process. A multi-program process (watch, language server)
/// drops the checkers of a released program, so there the data is an `Rc`
/// or `Box` that is freed with its checker. Else each released program
/// would leave its arena data behind.
#[inline(always)]
fn use_checker_arena() -> bool {
    !crate::core::is_multi_program()
}

/// This thread's leaked checker arena. The `&'static LeakArena` is taken
/// out of the thread local first, so the allocation runs outside `with`.
#[inline]
fn checker_arena() -> &'static LeakArena {
    CHECKER_ARENA.with(|arena| *arena)
}

// Go: checker/types.go:36 IndexKind
// PORT: `flags.rs` was generated at an older pin, so this enum (ts#64264) is
// declared here.
crate::flags_macros::go_enum!(IndexKind, i32 {
    STRING = 0; // IndexKindString
    NUMBER = 1; // IndexKindNumber
});

// PORT: Go `TypeFormatFlagsNodeBuilderFlagsMask` is the last constant of the
// `TypeFormatFlags` block (ts#63911). `flags.rs` has only the single-bit
// values, so the mask stays here.
impl TypeFormatFlags {
    pub const NODE_BUILDER_FLAGS_MASK: Self = Self(
        Self::NO_TRUNCATION.0
            | Self::WRITE_ARRAY_AS_GENERIC_TYPE.0
            | Self::GENERATE_NAMES_FOR_SHADOWED_TYPE_PARAMS.0
            | Self::USE_STRUCTURAL_FALLBACK.0
            | Self::WRITE_TYPE_ARGUMENTS_OF_SIGNATURE.0
            | Self::USE_FULLY_QUALIFIED_TYPE.0
            | Self::SUPPRESS_ANY_RETURN_TYPE.0
            | Self::MULTILINE_OBJECT_LITERALS.0
            | Self::WRITE_CLASS_EXPRESSION_AS_TYPE_LITERAL.0
            | Self::USE_TYPE_OF_FUNCTION.0
            | Self::OMIT_PARAMETER_MODIFIERS.0
            | Self::USE_ALIAS_DEFINED_OUTSIDE_CURRENT_SCOPE.0
            | Self::ALLOW_UNIQUE_ES_SYMBOL_TYPE.0
            | Self::IN_TYPE_ALIAS.0
            | Self::USE_INSTANTIATION_EXPRESSIONS.0
            | Self::USE_SINGLE_QUOTES_FOR_STRING_LITERAL_TYPE.0
            | Self::NO_TYPE_REDUCTION.0
            | Self::OMIT_THIS_PARAMETER.0,
    );
}

// Go: checker/types.go:165 externalHelpersModuleNameText
pub const EXTERNAL_HELPERS_MODULE_NAME_TEXT: &str = "tslib";

// Links for referenced symbols

// Go: checker/types.go:176 SymbolReferenceLinks
#[derive(Clone, Debug, Default)]
pub struct SymbolReferenceLinks {
    pub reference_kinds: SymbolFlags, // Flags for the meanings of the symbol that were referenced
}

// Links for value symbols

// Go: checker/types.go:182 ValueSymbolLinks
#[derive(Clone, Debug, Default)]
pub struct ValueSymbolLinks {
    pub resolved_type: TypeId, // Type of value symbol
    pub write_type: TypeId,
    pub target: SymbolId,
    pub mapper: MapperId,
    pub name_type: TypeId,
    pub containing_type: TypeId, // Mapped type for mapped type property, containing union or intersection type for synthetic property
    pub function_or_constructor_checked: bool,
    /// PORT: memo, no Go counterpart. The declaration test of Go
    /// `getTypeOfParameter` (initializer or question token on
    /// `value_declaration`), set on the first call for this symbol. It fits
    /// in the padding after the bool.
    pub optional_parameter: Tristate,
}

// The memo byte must not grow the record.
const _: () = assert!(std::mem::size_of::<ValueSymbolLinks>() == 28);

// Additional links for mapped symbols

// Go: checker/types.go:194 MappedSymbolLinks
#[derive(Clone, Debug, Default)]
pub struct MappedSymbolLinks {
    pub key_type: TypeId,           // Key type for mapped type member
    pub synthetic_origin: SymbolId, // For a property on a mapped or spread type, points back to the original property
}

// Additional links for deferred type symbols

// Go: checker/types.go:201 DeferredSymbolLinks
#[derive(Clone, Debug, Default)]
pub struct DeferredSymbolLinks {
    pub parent: TypeId,            // Source union/intersection of a deferred type
    pub constituents: Vec<TypeId>, // Calculated list of constituents for a deferred type
    pub write_constituents: Vec<TypeId>, // Constituents of a deferred `writeType`
}

// Links for alias symbols

// Go: checker/types.go:209 AliasSymbolLinks
#[derive(Clone, Debug, Default)]
pub struct AliasSymbolLinks {
    pub immediate_target: SymbolId, // Immediate target of an alias. May be another alias. Do not access directly, use `checker.getImmediateAliasedSymbol` instead.
    pub alias_target: SymbolId,     // Resolved (non-alias) target of an alias
    pub referenced: bool, // True if alias symbol has been referenced as a value that can be emitted
    pub type_only_declaration: Node, // First resolved alias declaration that makes the symbol only usable in type constructs
}

// Links for module symbols

// Go: checker/types.go:218 ModuleSymbolLinks
#[derive(Clone, Debug, Default)]
pub struct ModuleSymbolLinks {
    pub resolved_exports: SymbolTable, // Resolved exports of module or combined early- and late-bound static members of a class.
    // PORT: Go nil map is an empty map here. Go only reads it with lookups.
    pub type_only_export_star_map: FxHashMap<String, Node>, // Set on a module symbol when some of its exports were resolved through a 'export type * from "mod"' declaration
    pub exports_checked: bool,
}

// Go: checker/types.go:224 ReverseMappedSymbolLinks
#[derive(Clone, Debug, Default)]
pub struct ReverseMappedSymbolLinks {
    pub property_type: TypeId,
    pub mapped_type: TypeId,     // References a mapped type
    pub constraint_type: TypeId, // References an index type
}

// Links for late-bound symbols

// Go: checker/types.go:232 LateBoundLinks
#[derive(Clone, Debug, Default)]
pub struct LateBoundLinks {
    pub late_symbol: SymbolId,
}

// Links for export type symbols

// Go: checker/types.go:238 ExportTypeLinks
#[derive(Clone, Debug, Default)]
pub struct ExportTypeLinks {
    pub target: SymbolId,         // Target symbol
    pub originating_import: Node, // Import declaration which produced the symbol, present if the symbol is marked as uncallable but had call signatures in `resolveESModuleSymbol`
}

// Links for type aliases

// Go: checker/types.go:245 TypeAliasLinks
#[derive(Clone, Default)]
pub struct TypeAliasLinks {
    pub declared_type: TypeId,
    pub type_parameters: Vec<TypeId>, // Type parameters of type alias (undefined if non-generic)
    // PORT: Go nil map (non-generic alias) is `None`.
    pub instantiations: Option<InstantiationMap>, // Instantiations of generic type alias (undefined if non-generic)
    pub is_constructor_declared_property: bool,
}

// Links for declared types (type parameters, class types, interface types, enums)

// Go: checker/types.go:254 DeclaredTypeLinks
#[derive(Clone, Debug, Default)]
pub struct DeclaredTypeLinks {
    pub declared_type: TypeId,
    pub interface_checked: bool,
    pub index_signatures_checked: bool,
    pub type_parameters_checked: bool,
    pub enum_checked: bool,
}

// Links for switch clauses

// Go: checker/types.go:264 ExhaustiveState
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ExhaustiveState(pub u8);

#[allow(non_upper_case_globals)]
impl ExhaustiveState {
    pub const UNKNOWN: Self = Self(0); // Exhaustive state not computed
    pub const COMPUTING: Self = Self(1); // Exhaustive state computation in progress
    pub const FALSE: Self = Self(2); // Switch statement is not exhaustive
    pub const TRUE: Self = Self(3); // Switch statement is exhaustive
}

// Go: checker/types.go:273 SwitchStatementLinks
#[derive(Clone, Debug, Default)]
pub struct SwitchStatementLinks {
    pub exhaustive_state: ExhaustiveState, // Switch statement exhaustiveness
    pub switch_types_computed: bool,
    pub witnesses_computed: bool,
    pub switch_types: Vec<TypeId>,
    pub witnesses: Vec<String>,
}

// Go: checker/types.go:281 ArrayLiteralLinks
#[derive(Clone, Debug, Default)]
pub struct ArrayLiteralLinks {
    pub indices_computed: bool,
    pub first_spread_index: i32, // Index of first spread expression (or -1 if none)
    pub last_spread_index: i32,  // Index of last spread expression (or -1 if none)
}

// Links for late-binding containers

// Go: checker/types.go:296 MembersAndExportsLinks
/// Indexed by `MembersOrExportsResolutionKind` (`links[kind.0 as usize]`).
pub type MembersAndExportsLinks = [SymbolTable; 2];

// Links for synthetic spread properties

// Go: checker/types.go:300 SpreadLinks
#[derive(Clone, Debug, Default)]
pub struct SpreadLinks {
    pub left_spread: SymbolId,  // Left source for synthetic spread property
    pub right_spread: SymbolId, // Right source for synthetic spread property
}

// Links for variances of type aliases and interface types

// Go: checker/types.go:307 VarianceLinks
#[derive(Clone, Debug, Default)]
pub struct VarianceLinks {
    pub variances: SharedList<VarianceFlags>,
}

// Go: checker/types.go:325 MarkedAssignmentSymbolLinks
#[derive(Clone, Debug, Default)]
pub struct MarkedAssignmentSymbolLinks {
    pub last_assignment_pos: i32,
    pub has_definite_assignment: bool, // Symbol is definitely assigned somewhere
}

// Go: checker/types.go:330 accessibleChainCacheKey
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct AccessibleChainCacheKey {
    pub use_only_external_aliasing: bool,
    pub location: Node,
    pub meaning: SymbolFlags,
}

// Go: checker/types.go:336 ContainingSymbolLinks
#[derive(Clone, Debug, Default)]
pub struct ContainingSymbolLinks {
    // PORT: Go keys by `ast.NodeId` of the file node; the `Node` handle of
    // the file is the same identity here.
    pub extended_containers_by_file: FxHashMap<Node, Vec<SymbolId>>, // Symbols of nodes which which logically contain this one, cached by file the request is made within
    // PORT: Go `*[]*ast.Symbol`; nil pointer is `None`.
    pub extended_containers: Option<Vec<SymbolId>>, // Containers (other than the parent) which this symbol is aliased in
    pub accessible_chain_cache: FxHashMap<AccessibleChainCacheKey, Vec<SymbolId>>,
}

// Common links

// Go: checker/types.go:375 NodeLinks
#[derive(Clone, Default)]
pub struct NodeLinks {
    pub flags: NodeCheckFlags, // Set of flags specific to Node
    pub declaration_requires_scope_change: Tristate, // Set by `useOuterVariableScopeInParameter` in checker when downlevel emit would change the name resolution scope inside of a parameter.
    pub has_reported_statement_in_ambient_context: bool, // Cache boolean if we report statements in ambient context
}

// Go: checker/types.go:381 SymbolNodeLinks
#[derive(Clone, Debug, Default)]
pub struct SymbolNodeLinks {
    pub resolved_symbol: SymbolId, // Resolved symbol associated with node
}

// Go: checker/types.go:385 TypeNodeLinks
#[derive(Clone, Debug, Default)]
pub struct TypeNodeLinks {
    pub resolved_type: TypeId, // Resolved type associated with node
    pub outer_type_parameters: Option<SharedList<TypeId>>, // Outer type parameters of anonymous object type. None until computed, like Go nil.
    /// PERF: memo of the `getConditionalFlowTypeOfType` parent walk from this
    /// node (AST facts only, see `Checker::get_conditional_flow_type_of_type`).
    /// False until `flow_steps` is computed.
    pub flow_steps_known: bool,
    /// The flow steps of the walk, in walk order. None is no step, so the
    /// memo of most nodes does not allocate.
    pub flow_steps: Option<Rc<[FlowStep]>>,
}

// Go: checker/types.go:390 ComputedNameNodeLinks
#[derive(Clone, Debug, Default)]
pub struct ComputedNameNodeLinks {
    pub has_name: Option<bool>, // If the node has a computable name. Go `*bool`; nil is `None`.
    pub name: String,           // Resolved name associated with the type of the node
}

/// One ancestor in the `getConditionalFlowTypeOfType` parent walk where a
/// branch of the loop body can add a constraint. Only the tests on the
/// type are left for each call.
#[derive(Clone, Copy, Debug)]
pub enum FlowStep {
    /// The walk comes up from the true type of this `ConditionalType` node.
    /// `covariant` is the walk variance at this step.
    Conditional { parent: Node, covariant: bool },
    /// The walk comes up from the type of this `MappedType` node, which has
    /// no name type.
    Mapped { parent: Node },
}

// Links for enum members

// Go: checker/types.go:397 EnumMemberLinks
#[derive(Clone, Default)]
pub struct EnumMemberLinks {
    pub value: EvaluatorResult, // Constant value of enum member
}

// Links for assertion expressions

// Go: checker/types.go:403 AssertionLinks
#[derive(Clone, Debug, Default)]
pub struct AssertionLinks {
    pub expr_type: TypeId, // Assertion expression type
}

// SourceFile links

// Go: checker/types.go:409 SourceFileLinks
#[derive(Clone, Debug, Default)]
pub struct SourceFileLinks {
    pub type_checked: bool,
    pub unused_checked: bool,
    /// Effect-TS/tsgo: the Effect rules ran on this file (see `check_source_file`).
    pub effect_checked: bool,
    pub external_helpers_module: SymbolId,
    pub requested_external_emit_helpers: ExternalEmitHelpers,
    pub deferred_nodes: IndexSet<Node>,
    pub identifier_check_nodes: Vec<Node>,
    pub local_jsx_namespace: String,
    pub local_jsx_fragment_namespace: String,
    pub local_jsx_factory: Node,
    pub local_jsx_fragment_factory: Node,
    pub jsx_fragment_type: TypeId,
}

// Signature specific links

// Go: checker/types.go:425 SignatureLinks
#[derive(Clone, Debug, Default)]
pub struct SignatureLinks {
    pub resolved_signature: SignatureId, // Cached signature of signature node or call expression
    pub effects_signature: SignatureId,  // Signature with possible control flow effects
    pub decorator_signature: SignatureId, // Signature for decorator as if invoked by the runtime
}

// Go: checker/types.go:520 typeFlagNames
static TYPE_FLAG_NAMES: [(TypeFlags, &str); 29] = [
    (TypeFlags::ANY, "Any"),
    (TypeFlags::UNKNOWN, "Unknown"),
    (TypeFlags::UNDEFINED, "Undefined"),
    (TypeFlags::NULL, "Null"),
    (TypeFlags::VOID, "Void"),
    (TypeFlags::STRING, "String"),
    (TypeFlags::NUMBER, "Number"),
    (TypeFlags::BIG_INT, "BigInt"),
    (TypeFlags::BOOLEAN, "Boolean"),
    (TypeFlags::ES_SYMBOL, "ESSymbol"),
    (TypeFlags::STRING_LITERAL, "StringLiteral"),
    (TypeFlags::NUMBER_LITERAL, "NumberLiteral"),
    (TypeFlags::BIG_INT_LITERAL, "BigIntLiteral"),
    (TypeFlags::BOOLEAN_LITERAL, "BooleanLiteral"),
    (TypeFlags::UNIQUE_ES_SYMBOL, "UniqueESSymbol"),
    (TypeFlags::ENUM_LITERAL, "EnumLiteral"),
    (TypeFlags::ENUM, "Enum"),
    (TypeFlags::NON_PRIMITIVE, "NonPrimitive"),
    (TypeFlags::NEVER, "Never"),
    (TypeFlags::TYPE_PARAMETER, "TypeParameter"),
    (TypeFlags::OBJECT, "Object"),
    (TypeFlags::INDEX, "Index"),
    (TypeFlags::TEMPLATE_LITERAL, "TemplateLiteral"),
    (TypeFlags::STRING_MAPPING, "StringMapping"),
    (TypeFlags::SUBSTITUTION, "Substitution"),
    (TypeFlags::INDEXED_ACCESS, "IndexedAccess"),
    (TypeFlags::CONDITIONAL, "Conditional"),
    (TypeFlags::UNION, "Union"),
    (TypeFlags::INTERSECTION, "Intersection"),
];

// FormatTypeFlags returns the individual flag names as a slice of strings.
// Go: checker/types.go:556 FormatTypeFlags
pub fn format_type_flags(flags: TypeFlags) -> Vec<String> {
    let mut result: Vec<String> = Vec::with_capacity(flags.0.count_ones() as usize);
    for (flag, name) in TYPE_FLAG_NAMES.iter() {
        if flags.intersects(*flag) {
            result.push((*name).to_string());
        }
    }
    if result.is_empty() {
        result.push("None".to_string());
    }
    result
}

// String returns a pipe-separated string of flag names.
// PORT: Go `String()` methods on flag types become `Display` impls so Go
// `%v` formatting ports to `{}`.
// Go: checker/types.go:570 TypeFlags.String
impl std::fmt::Display for TypeFlags {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&format_type_flags(*self).join("|"))
    }
}

// Go: checker/types.go:574 VarianceFlags.String
impl std::fmt::Display for VarianceFlags {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let variance = *self & VarianceFlags::VARIANCE_MASK;
        let mut result = match variance {
            VarianceFlags::INVARIANT => "in out".to_string(),
            VarianceFlags::BIVARIANT => "[bivariant]".to_string(),
            VarianceFlags::CONTRAVARIANT => "in".to_string(),
            VarianceFlags::COVARIANT => "out".to_string(),
            VarianceFlags::INDEPENDENT => "[independent]".to_string(),
            _ => String::new(),
        };
        if self.intersects(VarianceFlags::UNMEASURABLE) {
            result += " (unmeasurable)";
        } else if self.intersects(VarianceFlags::UNRELIABLE) {
            result += " (unreliable)";
        }
        f.write_str(&result)
    }
}

// TypeAlias

// Go: checker/types.go:664 TypeAlias
#[derive(Clone, Debug, Default)]
pub struct TypeAlias {
    pub symbol: SymbolId,
    pub type_arguments: Vec<TypeId>,
}

impl TypeAlias {
    // Go: checker/types.go:669 TypeAlias.Symbol
    pub fn symbol(&self) -> SymbolId {
        self.symbol
    }

    // Go: checker/types.go:676 TypeAlias.TypeArguments
    pub fn type_arguments(&self) -> &[TypeId] {
        &self.type_arguments
    }
}

/// Go calls `(*TypeAlias).Symbol()` and `TypeArguments()` on nil aliases and
/// gets nil. `Type::alias` is `Option<Rc<TypeAlias>>`, so this trait gives the
/// same nil-safe calls: `ty.alias.symbol()`, `ty.alias.type_arguments()`.
pub trait TypeAliasExt {
    fn symbol(&self) -> SymbolId;
    fn type_arguments(&self) -> &[TypeId];
}

impl TypeAliasExt for Option<Rc<TypeAlias>> {
    // Go: checker/types.go:669 TypeAlias.Symbol
    fn symbol(&self) -> SymbolId {
        match self {
            None => SymbolId::NIL,
            Some(a) => a.symbol,
        }
    }

    // Go: checker/types.go:676 TypeAlias.TypeArguments
    fn type_arguments(&self) -> &[TypeId] {
        match self {
            None => &[],
            Some(a) => &a.type_arguments,
        }
    }
}

// Type

// PORT: Go `Type.checker` is dropped; every type lives in the arena of the
// checker that created it. Go `TypeBase` (the embedded `Type` inside each data
// struct) disappears: `Type` owns its `data`.
// PORT: layout only. `repr(C)` keeps the hot header (flags, ids, alias) in
// front of `data`, and `align(64)` starts each arena type on a cache line, so
// a flags test and the `TypeData` tag read share one line. The header is 24
// bytes and `TypeData` 104 (`TypeReference` is the largest kind), so the size
// is 128 (2 lines).
// Go: checker/types.go:685 Type
#[derive(Clone, Default)]
#[repr(C, align(64))]
pub struct Type {
    pub flags: TypeFlags,
    pub object_flags: ObjectFlags,
    pub id: TypeId,
    pub symbol: SymbolId,
    pub alias: Option<Rc<TypeAlias>>,
    pub data: TypeData, // Type specific data
}

// A type must stay 2 cache lines. A rare or large field of a common kind
// goes out of line (see `ObjectType` and `StructuredType`).
const _: () = assert!(std::mem::size_of::<Type>() <= 128);
// The first line holds the header and the first 40 bytes of `TypeData`: for
// a type reference, the memo (the `TypeData` tag), the type arguments, the
// target and the mapper (see `TypeReference`).
const _: () = assert!(std::mem::offset_of!(Type, data) == 24);
const _: () = assert!(std::mem::offset_of!(TypeReference, object.mapper) + 4 <= 40);
const _: () = assert!(std::mem::size_of::<TypeData>() == std::mem::size_of::<TypeReference>());

/// Go's runtime panic for a failed type assertion `t.data.(*want)` on data
/// of another struct (types.go:709-727).
#[cold]
#[inline(never)]
fn type_cast_panic(have: &TypeData, want: &str) -> ! {
    crate::core::go_panic(format!(
        "interface conversion: checker.TypeData is *checker.{}, not *checker.{want}",
        have.go_struct_name()
    ))
}

impl Type {
    // Go: checker/types.go:695 Type.Id
    #[inline]
    pub fn id(&self) -> TypeId {
        self.id
    }

    // Go: checker/types.go:699 Type.Flags
    #[inline]
    pub fn flags(&self) -> TypeFlags {
        self.flags
    }

    // Go: checker/types.go:703 Type.ObjectFlags
    #[inline]
    pub fn object_flags(&self) -> ObjectFlags {
        self.object_flags
    }

    // Casts for concrete struct types

    // Go: checker/types.go:709 Type.AsIntrinsicType
    #[inline]
    pub fn as_intrinsic_type(&self) -> &IntrinsicType {
        match &self.data {
            TypeData::Intrinsic(d) => d,
            other => type_cast_panic(other, "IntrinsicType"),
        }
    }
    #[inline]
    pub fn as_intrinsic_type_mut(&mut self) -> &mut IntrinsicType {
        match &mut self.data {
            TypeData::Intrinsic(d) => d,
            other => type_cast_panic(other, "IntrinsicType"),
        }
    }

    // Go: checker/types.go:710 Type.AsLiteralType
    #[inline]
    pub fn as_literal_type(&self) -> &LiteralType {
        match &self.data {
            TypeData::Literal(d) => d,
            other => type_cast_panic(other, "LiteralType"),
        }
    }
    #[inline]
    pub fn as_literal_type_mut(&mut self) -> &mut LiteralType {
        match &mut self.data {
            TypeData::Literal(d) => d,
            other => type_cast_panic(other, "LiteralType"),
        }
    }

    // Go: checker/types.go:711 Type.AsUniqueESSymbolType
    #[inline]
    pub fn as_unique_es_symbol_type(&self) -> &UniqueESSymbolType {
        match &self.data {
            TypeData::UniqueESSymbol(d) => d,
            other => type_cast_panic(other, "UniqueESSymbolType"),
        }
    }
    #[inline]
    pub fn as_unique_es_symbol_type_mut(&mut self) -> &mut UniqueESSymbolType {
        match &mut self.data {
            TypeData::UniqueESSymbol(d) => d,
            other => type_cast_panic(other, "UniqueESSymbolType"),
        }
    }

    // Go: checker/types.go:712 Type.AsTupleType
    #[inline]
    pub fn as_tuple_type(&self) -> &TupleType {
        match &self.data {
            TypeData::Tuple(d) => d,
            other => type_cast_panic(other, "TupleType"),
        }
    }
    #[inline]
    pub fn as_tuple_type_mut(&mut self) -> &mut TupleType {
        match &mut self.data {
            TypeData::Tuple(d) => d,
            other => type_cast_panic(other, "TupleType"),
        }
    }

    // Go: checker/types.go:713 Type.AsInstantiationExpressionType
    #[inline]
    pub fn as_instantiation_expression_type(&self) -> &InstantiationExpressionType {
        match &self.data {
            TypeData::InstantiationExpression(d) => d,
            other => type_cast_panic(other, "InstantiationExpressionType"),
        }
    }
    #[inline]
    pub fn as_instantiation_expression_type_mut(&mut self) -> &mut InstantiationExpressionType {
        match &mut self.data {
            TypeData::InstantiationExpression(d) => d,
            other => type_cast_panic(other, "InstantiationExpressionType"),
        }
    }

    // Go: checker/types.go:716 Type.AsMappedType
    #[inline]
    pub fn as_mapped_type(&self) -> &MappedType {
        match &self.data {
            TypeData::Mapped(d) => d,
            other => type_cast_panic(other, "MappedType"),
        }
    }
    #[inline]
    pub fn as_mapped_type_mut(&mut self) -> &mut MappedType {
        match &mut self.data {
            TypeData::Mapped(d) => d,
            other => type_cast_panic(other, "MappedType"),
        }
    }

    // Go: checker/types.go:717 Type.AsReverseMappedType
    #[inline]
    pub fn as_reverse_mapped_type(&self) -> &ReverseMappedType {
        match &self.data {
            TypeData::ReverseMapped(d) => d,
            other => type_cast_panic(other, "ReverseMappedType"),
        }
    }
    #[inline]
    pub fn as_reverse_mapped_type_mut(&mut self) -> &mut ReverseMappedType {
        match &mut self.data {
            TypeData::ReverseMapped(d) => d,
            other => type_cast_panic(other, "ReverseMappedType"),
        }
    }

    // Go: checker/types.go:718 Type.AsEvolvingArrayType
    #[inline]
    pub fn as_evolving_array_type(&self) -> &EvolvingArrayType {
        match &self.data {
            TypeData::EvolvingArray(d) => d,
            other => type_cast_panic(other, "EvolvingArrayType"),
        }
    }
    #[inline]
    pub fn as_evolving_array_type_mut(&mut self) -> &mut EvolvingArrayType {
        match &mut self.data {
            TypeData::EvolvingArray(d) => d,
            other => type_cast_panic(other, "EvolvingArrayType"),
        }
    }

    // Go: checker/types.go:719 Type.AsTypeParameter
    #[inline]
    pub fn as_type_parameter(&self) -> &TypeParameter {
        match &self.data {
            TypeData::TypeParameter(d) => d,
            other => type_cast_panic(other, "TypeParameter"),
        }
    }
    #[inline]
    pub fn as_type_parameter_mut(&mut self) -> &mut TypeParameter {
        match &mut self.data {
            TypeData::TypeParameter(d) => d,
            other => type_cast_panic(other, "TypeParameter"),
        }
    }

    // Go: checker/types.go:720 Type.AsUnionType
    #[inline]
    pub fn as_union_type(&self) -> &UnionType {
        match &self.data {
            TypeData::Union(d) => d,
            other => type_cast_panic(other, "UnionType"),
        }
    }
    #[inline]
    pub fn as_union_type_mut(&mut self) -> &mut UnionType {
        match &mut self.data {
            TypeData::Union(d) => d,
            other => type_cast_panic(other, "UnionType"),
        }
    }

    // Go: checker/types.go:721 Type.AsIntersectionType
    #[inline]
    pub fn as_intersection_type(&self) -> &IntersectionType {
        match &self.data {
            TypeData::Intersection(d) => d,
            other => type_cast_panic(other, "IntersectionType"),
        }
    }
    #[inline]
    pub fn as_intersection_type_mut(&mut self) -> &mut IntersectionType {
        match &mut self.data {
            TypeData::Intersection(d) => d,
            other => type_cast_panic(other, "IntersectionType"),
        }
    }

    // Go: checker/types.go:722 Type.AsIndexType
    #[inline]
    pub fn as_index_type(&self) -> &IndexType {
        match &self.data {
            TypeData::Index(d) => d,
            other => type_cast_panic(other, "IndexType"),
        }
    }
    #[inline]
    pub fn as_index_type_mut(&mut self) -> &mut IndexType {
        match &mut self.data {
            TypeData::Index(d) => d,
            other => type_cast_panic(other, "IndexType"),
        }
    }

    // Go: checker/types.go:723 Type.AsIndexedAccessType
    #[inline]
    pub fn as_indexed_access_type(&self) -> &IndexedAccessType {
        match &self.data {
            TypeData::IndexedAccess(d) => d,
            other => type_cast_panic(other, "IndexedAccessType"),
        }
    }
    #[inline]
    pub fn as_indexed_access_type_mut(&mut self) -> &mut IndexedAccessType {
        match &mut self.data {
            TypeData::IndexedAccess(d) => d,
            other => type_cast_panic(other, "IndexedAccessType"),
        }
    }

    // Go: checker/types.go:724 Type.AsTemplateLiteralType
    #[inline]
    pub fn as_template_literal_type(&self) -> &TemplateLiteralType {
        match &self.data {
            TypeData::TemplateLiteral(d) => d,
            other => type_cast_panic(other, "TemplateLiteralType"),
        }
    }
    #[inline]
    pub fn as_template_literal_type_mut(&mut self) -> &mut TemplateLiteralType {
        match &mut self.data {
            TypeData::TemplateLiteral(d) => d,
            other => type_cast_panic(other, "TemplateLiteralType"),
        }
    }

    // Go: checker/types.go:725 Type.AsStringMappingType
    #[inline]
    pub fn as_string_mapping_type(&self) -> &StringMappingType {
        match &self.data {
            TypeData::StringMapping(d) => d,
            other => type_cast_panic(other, "StringMappingType"),
        }
    }
    #[inline]
    pub fn as_string_mapping_type_mut(&mut self) -> &mut StringMappingType {
        match &mut self.data {
            TypeData::StringMapping(d) => d,
            other => type_cast_panic(other, "StringMappingType"),
        }
    }

    // Go: checker/types.go:726 Type.AsSubstitutionType
    #[inline]
    pub fn as_substitution_type(&self) -> &SubstitutionType {
        match &self.data {
            TypeData::Substitution(d) => d,
            other => type_cast_panic(other, "SubstitutionType"),
        }
    }
    #[inline]
    pub fn as_substitution_type_mut(&mut self) -> &mut SubstitutionType {
        match &mut self.data {
            TypeData::Substitution(d) => d,
            other => type_cast_panic(other, "SubstitutionType"),
        }
    }

    // Go: checker/types.go:727 Type.AsConditionalType
    #[inline]
    pub fn as_conditional_type(&self) -> &ConditionalType {
        match &self.data {
            TypeData::Conditional(d) => d,
            other => type_cast_panic(other, "ConditionalType"),
        }
    }
    #[inline]
    pub fn as_conditional_type_mut(&mut self) -> &mut ConditionalType {
        match &mut self.data {
            TypeData::Conditional(d) => d,
            other => type_cast_panic(other, "ConditionalType"),
        }
    }

    // Casts for embedded struct types
    // PORT: Go returns nil for kinds without the embedded struct, and the
    // caller then panics on field access. These panic at the cast instead,
    // with Go's nil dereference text. Use `self.data.as_x()` (returns
    // `Option`) for Go nil checks.

    // Go: checker/types.go:731 Type.AsConstrainedType
    #[inline]
    pub fn as_constrained_type(&self) -> &ConstrainedType {
        self.data
            .as_constrained_type()
            .unwrap_or_else(|| crate::core::go_nil_dereference())
    }
    #[inline]
    pub fn as_constrained_type_mut(&mut self) -> &mut ConstrainedType {
        self.data
            .as_constrained_type_mut()
            .unwrap_or_else(|| crate::core::go_nil_dereference())
    }

    // Go: checker/types.go:732 Type.AsStructuredType
    #[inline]
    pub fn as_structured_type(&self) -> &StructuredType {
        self.data
            .as_structured_type()
            .unwrap_or_else(|| crate::core::go_nil_dereference())
    }
    #[inline]
    pub fn as_structured_type_mut(&mut self) -> &mut StructuredType {
        self.data
            .as_structured_type_mut()
            .unwrap_or_else(|| crate::core::go_nil_dereference())
    }

    // Go: checker/types.go:733 Type.AsObjectType
    #[inline]
    pub fn as_object_type(&self) -> &ObjectType {
        self.data
            .as_object_type()
            .unwrap_or_else(|| crate::core::go_nil_dereference())
    }
    #[inline]
    pub fn as_object_type_mut(&mut self) -> &mut ObjectType {
        self.data
            .as_object_type_mut()
            .unwrap_or_else(|| crate::core::go_nil_dereference())
    }

    // Go: checker/types.go:734 Type.AsTypeReference
    #[inline(always)]
    pub fn as_type_reference(&self) -> &TypeReference {
        self.data
            .as_type_reference()
            .unwrap_or_else(|| crate::core::go_nil_dereference())
    }
    #[inline]
    pub fn as_type_reference_mut(&mut self) -> &mut TypeReference {
        self.data
            .as_type_reference_mut()
            .unwrap_or_else(|| crate::core::go_nil_dereference())
    }

    // Go: checker/types.go:735 Type.AsInterfaceType
    #[inline]
    pub fn as_interface_type(&self) -> &InterfaceType {
        self.data
            .as_interface_type()
            .unwrap_or_else(|| crate::core::go_nil_dereference())
    }
    #[inline]
    pub fn as_interface_type_mut(&mut self) -> &mut InterfaceType {
        self.data
            .as_interface_type_mut()
            .unwrap_or_else(|| crate::core::go_nil_dereference())
    }

    // Go: checker/types.go:736 Type.AsUnionOrIntersectionType
    #[inline]
    pub fn as_union_or_intersection_type(&self) -> &UnionOrIntersectionType {
        self.data
            .as_union_or_intersection_type()
            .unwrap_or_else(|| crate::core::go_nil_dereference())
    }
    #[inline]
    pub fn as_union_or_intersection_type_mut(&mut self) -> &mut UnionOrIntersectionType {
        self.data
            .as_union_or_intersection_type_mut()
            .unwrap_or_else(|| crate::core::go_nil_dereference())
    }

    // PORT: Go returns a fresh slice (`[]*Type{t}`) for the default case, so
    // this returns an owned `Vec`.
    // Go: checker/types.go:740 Type.Distributed
    pub fn distributed(&self) -> Vec<TypeId> {
        if self.flags.intersects(TypeFlags::UNION) {
            return self.as_union_type().union_or_intersection.types.to_vec();
        } else if self.flags.intersects(TypeFlags::NEVER) {
            return Vec::new();
        }
        vec![self.id]
    }

    // Common accessors

    // Go: checker/types.go:752 Type.Target
    // PERF: `inline(always)`: with a plain `#[inline]` hint LLVM kept this
    // out of line (elysia: 0.8 to 1.0% self).
    #[inline(always)]
    pub fn target(&self) -> TypeId {
        if self.flags.intersects(TypeFlags::OBJECT) {
            return self.as_object_type().target;
        } else if self.flags.intersects(TypeFlags::TYPE_PARAMETER) {
            return self.as_type_parameter().target;
        } else if self.flags.intersects(TypeFlags::INDEX) {
            return self.as_index_type().target;
        } else if self.flags.intersects(TypeFlags::STRING_MAPPING) {
            return self.as_string_mapping_type().target;
        } else if self.flags.intersects(TypeFlags::OBJECT)
            && self.object_flags.intersects(ObjectFlags::MAPPED)
        {
            // PORT: unreachable in Go too (the first case already matches).
            return self.as_mapped_type().object.target;
        }
        panic!("Unhandled case in Type.Target")
    }

    // Go: checker/types.go:768 Type.Mapper
    #[inline]
    pub fn mapper(&self) -> MapperId {
        if self.flags.intersects(TypeFlags::OBJECT) {
            return self.as_object_type().mapper;
        } else if self.flags.intersects(TypeFlags::TYPE_PARAMETER) {
            return self.as_type_parameter().mapper;
        } else if self.flags.intersects(TypeFlags::CONDITIONAL) {
            return self.as_conditional_type().mapper;
        }
        panic!("Unhandled case in Type.Mapper")
    }

    // Go: checker/types.go:780 Type.Types
    #[inline(always)]
    pub fn types(&self) -> &[TypeId] {
        if self.flags.intersects(TypeFlags::UNION_OR_INTERSECTION) {
            return &self.as_union_or_intersection_type().types;
        } else if self.flags.intersects(TypeFlags::TEMPLATE_LITERAL) {
            return &self.as_template_literal_type().types;
        }
        panic!("Unhandled case in Type.Types")
    }

    /// `types()` as a shared list. The clone copies no elements, so callers
    /// can keep it across checker calls that need `&mut self`.
    pub fn types_list(&self) -> SharedList<TypeId> {
        if self.flags.intersects(TypeFlags::UNION_OR_INTERSECTION) {
            return self.as_union_or_intersection_type().types.clone();
        } else if self.flags.intersects(TypeFlags::TEMPLATE_LITERAL) {
            return self.as_template_literal_type().types.clone();
        }
        panic!("Unhandled case in Type.Types")
    }

    // PORT: Go `TargetInterfaceType()` and `TargetTupleType()` follow the
    // target pointer into another type. They need the arena, so they are
    // `Checker` methods: `self.target_interface_type(t)`,
    // `self.target_tuple_type(t)` (defined below).

    // Go: checker/types.go:798 Type.Symbol
    pub fn symbol(&self) -> SymbolId {
        self.symbol
    }

    // Go: checker/types.go:802 Type.Alias
    pub fn alias(&self) -> Option<Rc<TypeAlias>> {
        self.alias.clone()
    }

    // Go: checker/types.go:806 Type.IsUnion
    pub fn is_union(&self) -> bool {
        self.flags.intersects(TypeFlags::UNION)
    }

    // Go: checker/types.go:810 Type.IsString
    pub fn is_string(&self) -> bool {
        self.flags.intersects(TypeFlags::STRING)
    }

    // Go: checker/types.go:814 Type.IsIntersection
    pub fn is_intersection(&self) -> bool {
        self.flags.intersects(TypeFlags::INTERSECTION)
    }

    // Go: checker/types.go:818 Type.IsStringLiteral
    pub fn is_string_literal(&self) -> bool {
        self.flags.intersects(TypeFlags::STRING_LITERAL)
    }

    // Go: checker/types.go:822 Type.IsNumberLiteral
    pub fn is_number_literal(&self) -> bool {
        self.flags.intersects(TypeFlags::NUMBER_LITERAL)
    }

    // Go: checker/types.go:826 Type.IsBigIntLiteral
    pub fn is_big_int_literal(&self) -> bool {
        self.flags.intersects(TypeFlags::BIG_INT_LITERAL)
    }

    // Go: checker/types.go:830 Type.IsEnumLiteral
    pub fn is_enum_literal(&self) -> bool {
        self.flags.intersects(TypeFlags::ENUM_LITERAL)
    }

    // Go: checker/types.go:834 Type.IsBooleanLike
    pub fn is_boolean_like(&self) -> bool {
        self.flags.intersects(TypeFlags::BOOLEAN_LIKE)
    }

    // Go: checker/types.go:838 Type.IsStringLike
    pub fn is_string_like(&self) -> bool {
        self.flags.intersects(TypeFlags::STRING_LIKE)
    }

    // Go: checker/types.go:842 Type.IsClass
    pub fn is_class(&self) -> bool {
        self.object_flags.intersects(ObjectFlags::CLASS)
    }

    // Go: checker/types.go:846 Type.IsTypeParameter
    pub fn is_type_parameter(&self) -> bool {
        self.flags.intersects(TypeFlags::TYPE_PARAMETER)
    }

    // Go: checker/types.go:850 Type.IsIndex
    pub fn is_index(&self) -> bool {
        self.flags.intersects(TypeFlags::INDEX)
    }

    // PORT: Go `Type.IsTupleType()` only wraps the package function
    // `isTupleType` (a `Checker` method `is_tuple_type(t)` in the checker
    // port) and only `ls` calls it. It is skipped as a language-service API.
}

impl Checker {
    // Go: checker/types.go:790 Type.TargetInterfaceType
    pub fn target_interface_type(&self, t: TypeId) -> &InterfaceType {
        let target = self.ty(t).as_type_reference().object.target;
        self.ty(target).as_interface_type()
    }

    // Go: checker/types.go:794 Type.TargetTupleType
    pub fn target_tuple_type(&self, t: TypeId) -> &TupleType {
        let target = self.ty(t).as_type_reference().object.target;
        self.ty(target).as_tuple_type()
    }
}

/// A value in this thread's checker arena, or a `Box` in a multi-program
/// process (`use_checker_arena`). It holds the union and intersection data
/// of `TypeData`.
///
/// PERF: a `Box` cost one malloc for each union or intersection type. The
/// arena costs a pointer bump. In a one-program process types are never
/// dropped (the checker is leaked), so the arena memory is never freed
/// either. When an arena `ArenaBox` is dropped, the value is not: its own
/// heap fields stay allocated. A released program of a multi-program
/// process frees its checkers, so there the value is a `Box`.
pub struct ArenaBox<T: 'static>(ArenaBoxRepr<T>);

enum ArenaBoxRepr<T: 'static> {
    Arena(&'static mut T),
    Owned(Box<T>),
}

impl<T> ArenaBox<T> {
    /// Moves `value` into this thread's checker arena, or into a `Box`.
    #[inline(always)]
    pub fn new(value: T) -> Self {
        Self::new_with(move || value)
    }

    /// Puts the value that `make` returns in this thread's checker arena,
    /// or in a `Box`.
    ///
    /// PERF: the slot is made first and `make` runs after, so a large value
    /// is built straight in its slot (`Bump::alloc_with`, `Box::write`). A
    /// value built before the call is copied with `memcpy`.
    #[inline(always)]
    pub fn new_with(make: impl FnOnce() -> T) -> Self {
        ArenaBox(if use_checker_arena() {
            ArenaBoxRepr::Arena(checker_arena().alloc_with(make))
        } else {
            ArenaBoxRepr::Owned(Box::write(Box::new_uninit(), make()))
        })
    }
}

impl<T> std::ops::Deref for ArenaBox<T> {
    type Target = T;

    #[inline(always)]
    fn deref(&self) -> &T {
        match &self.0 {
            ArenaBoxRepr::Arena(value) => value,
            ArenaBoxRepr::Owned(value) => value,
        }
    }
}

impl<T> std::ops::DerefMut for ArenaBox<T> {
    #[inline(always)]
    fn deref_mut(&mut self) -> &mut T {
        match &mut self.0 {
            ArenaBoxRepr::Arena(value) => value,
            ArenaBoxRepr::Owned(value) => value,
        }
    }
}

impl<T: std::fmt::Debug> std::fmt::Debug for ArenaBox<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(&**self, f)
    }
}

/// A clone is a new copy, like a `Box` clone.
impl<T: Clone> Clone for ArenaBox<T> {
    fn clone(&self) -> Self {
        ArenaBox::new((**self).clone())
    }
}

// TypeData

// PORT: Go `TypeData` is an interface over pointers to the concrete structs
// below. Here it is an enum that owns the concrete struct. The Go interface
// methods `AsConstrainedType` ... `AsUnionOrIntersectionType` return `None`
// where Go returns nil. Go `AsType()` has no port: `Type` owns its data.
// PORT: the large variants are boxed so `Type` stays small in the arena.
// Interface, tuple, mapped, reverse mapped, evolving array and instantiation
// expression types are rare; union and intersection data is large, and in a
// one-program process it is in the checker arena (`ArenaBox`), not a malloc
// block.
// Go: checker/types.go:860 TypeData
#[derive(Clone)]
pub enum TypeData {
    Intrinsic(IntrinsicType),
    Literal(LiteralType),
    UniqueESSymbol(UniqueESSymbolType),
    TypeParameter(TypeParameter),
    Index(IndexType),
    IndexedAccess(IndexedAccessType),
    TemplateLiteral(TemplateLiteralType),
    StringMapping(StringMappingType),
    Substitution(SubstitutionType),
    Conditional(ConditionalType),
    Object(ObjectType),
    TypeReference(TypeReference),
    Interface(Box<InterfaceType>),
    Tuple(Box<TupleType>),
    InstantiationExpression(Box<InstantiationExpressionType>),
    Mapped(Box<MappedType>),
    ReverseMapped(Box<ReverseMappedType>),
    EvolvingArray(Box<EvolvingArrayType>),
    Union(ArenaBox<UnionType>),
    Intersection(ArenaBox<IntersectionType>),
}

impl TypeData {
    /// The name of the Go struct of this data (`*checker.<name>`).
    fn go_struct_name(&self) -> &'static str {
        match self {
            TypeData::Intrinsic(_) => "IntrinsicType",
            TypeData::Literal(_) => "LiteralType",
            TypeData::UniqueESSymbol(_) => "UniqueESSymbolType",
            TypeData::TypeParameter(_) => "TypeParameter",
            TypeData::Index(_) => "IndexType",
            TypeData::IndexedAccess(_) => "IndexedAccessType",
            TypeData::TemplateLiteral(_) => "TemplateLiteralType",
            TypeData::StringMapping(_) => "StringMappingType",
            TypeData::Substitution(_) => "SubstitutionType",
            TypeData::Conditional(_) => "ConditionalType",
            TypeData::Object(_) => "ObjectType",
            TypeData::TypeReference(_) => "TypeReference",
            TypeData::Interface(_) => "InterfaceType",
            TypeData::Tuple(_) => "TupleType",
            TypeData::InstantiationExpression(_) => "InstantiationExpressionType",
            TypeData::Mapped(_) => "MappedType",
            TypeData::ReverseMapped(_) => "ReverseMappedType",
            TypeData::EvolvingArray(_) => "EvolvingArrayType",
            TypeData::Union(_) => "UnionType",
            TypeData::Intersection(_) => "IntersectionType",
        }
    }
}

// PORT: the arena keeps a dummy `Type` at index 0, so `TypeData` needs a
// default. It has no Go counterpart.
impl Default for TypeData {
    fn default() -> Self {
        TypeData::Intrinsic(IntrinsicType::default())
    }
}

impl TypeData {
    // Go: checker/types.go:877 TypeBase.AsConstrainedType
    #[inline]
    pub fn as_constrained_type(&self) -> Option<&ConstrainedType> {
        match self {
            TypeData::Intrinsic(_) | TypeData::Literal(_) | TypeData::UniqueESSymbol(_) => None,
            TypeData::TypeParameter(d) => Some(&d.constrained),
            TypeData::Index(d) => Some(&d.constrained),
            TypeData::IndexedAccess(d) => Some(&d.constrained),
            TypeData::TemplateLiteral(d) => Some(&d.constrained),
            TypeData::StringMapping(d) => Some(&d.constrained),
            TypeData::Substitution(d) => Some(&d.constrained),
            TypeData::Conditional(d) => Some(&d.constrained),
            _ => self.as_structured_type().map(|s| &s.constrained),
        }
    }

    #[inline]
    pub fn as_constrained_type_mut(&mut self) -> Option<&mut ConstrainedType> {
        match self {
            TypeData::Intrinsic(_) | TypeData::Literal(_) | TypeData::UniqueESSymbol(_) => None,
            TypeData::TypeParameter(d) => Some(&mut d.constrained),
            TypeData::Index(d) => Some(&mut d.constrained),
            TypeData::IndexedAccess(d) => Some(&mut d.constrained),
            TypeData::TemplateLiteral(d) => Some(&mut d.constrained),
            TypeData::StringMapping(d) => Some(&mut d.constrained),
            TypeData::Substitution(d) => Some(&mut d.constrained),
            TypeData::Conditional(d) => Some(&mut d.constrained),
            TypeData::Union(d) => Some(&mut d.union_or_intersection.structured.constrained),
            TypeData::Intersection(d) => Some(&mut d.union_or_intersection.structured.constrained),
            TypeData::Object(d) => Some(&mut d.structured.constrained),
            TypeData::InstantiationExpression(d) => Some(&mut d.object.structured.constrained),
            TypeData::Mapped(d) => Some(&mut d.object.structured.constrained),
            TypeData::ReverseMapped(d) => Some(&mut d.object.structured.constrained),
            TypeData::EvolvingArray(d) => Some(&mut d.object.structured.constrained),
            TypeData::TypeReference(d) => Some(&mut d.object.structured.constrained),
            TypeData::Interface(d) => Some(&mut d.reference.object.structured.constrained),
            TypeData::Tuple(d) => Some(&mut d.interface.reference.object.structured.constrained),
        }
    }

    // Go: checker/types.go:878 TypeBase.AsStructuredType
    // PERF: the common kinds are tests at the call site, and the other kinds
    // are out of line (`as_structured_type_other`). With every kind in one
    // match, LLVM makes a jump table: a load and an indirect jump for every
    // cast. `if let` tests before the match do not help, because LLVM merges
    // them into the jump table.
    #[inline]
    pub fn as_structured_type(&self) -> Option<&StructuredType> {
        match self {
            TypeData::Object(d) => Some(&d.structured),
            TypeData::TypeReference(d) => Some(&d.object.structured),
            TypeData::Interface(d) => Some(&d.reference.object.structured),
            _ => self.as_structured_type_other(),
        }
    }

    /// `as_structured_type` for the kinds it does not test inline.
    #[inline(never)]
    fn as_structured_type_other(&self) -> Option<&StructuredType> {
        match self {
            TypeData::Union(d) => Some(&d.union_or_intersection.structured),
            TypeData::Intersection(d) => Some(&d.union_or_intersection.structured),
            _ => self.as_object_type().map(|o| &o.structured),
        }
    }

    #[inline]
    pub fn as_structured_type_mut(&mut self) -> Option<&mut StructuredType> {
        match self {
            TypeData::Union(d) => Some(&mut d.union_or_intersection.structured),
            TypeData::Intersection(d) => Some(&mut d.union_or_intersection.structured),
            TypeData::Object(d) => Some(&mut d.structured),
            TypeData::InstantiationExpression(d) => Some(&mut d.object.structured),
            TypeData::Mapped(d) => Some(&mut d.object.structured),
            TypeData::ReverseMapped(d) => Some(&mut d.object.structured),
            TypeData::EvolvingArray(d) => Some(&mut d.object.structured),
            TypeData::TypeReference(d) => Some(&mut d.object.structured),
            TypeData::Interface(d) => Some(&mut d.reference.object.structured),
            TypeData::Tuple(d) => Some(&mut d.interface.reference.object.structured),
            _ => None,
        }
    }

    // Go: checker/types.go:879 TypeBase.AsObjectType
    #[inline]
    pub fn as_object_type(&self) -> Option<&ObjectType> {
        match self {
            TypeData::Object(d) => Some(d),
            TypeData::InstantiationExpression(d) => Some(&d.object),
            TypeData::Mapped(d) => Some(&d.object),
            TypeData::ReverseMapped(d) => Some(&d.object),
            TypeData::EvolvingArray(d) => Some(&d.object),
            _ => self.as_type_reference().map(|r| &r.object),
        }
    }

    #[inline]
    pub fn as_object_type_mut(&mut self) -> Option<&mut ObjectType> {
        match self {
            TypeData::Object(d) => Some(d),
            TypeData::InstantiationExpression(d) => Some(&mut d.object),
            TypeData::Mapped(d) => Some(&mut d.object),
            TypeData::ReverseMapped(d) => Some(&mut d.object),
            TypeData::EvolvingArray(d) => Some(&mut d.object),
            TypeData::TypeReference(d) => Some(&mut d.object),
            TypeData::Interface(d) => Some(&mut d.reference.object),
            TypeData::Tuple(d) => Some(&mut d.interface.reference.object),
            _ => None,
        }
    }

    // Go: checker/types.go:880 TypeBase.AsTypeReference
    #[inline]
    pub fn as_type_reference(&self) -> Option<&TypeReference> {
        match self {
            TypeData::TypeReference(d) => Some(d),
            _ => self.as_interface_type().map(|i| &i.reference),
        }
    }

    #[inline]
    pub fn as_type_reference_mut(&mut self) -> Option<&mut TypeReference> {
        match self {
            TypeData::TypeReference(d) => Some(d),
            TypeData::Interface(d) => Some(&mut d.reference),
            TypeData::Tuple(d) => Some(&mut d.interface.reference),
            _ => None,
        }
    }

    // Go: checker/types.go:881 TypeBase.AsInterfaceType
    #[inline]
    pub fn as_interface_type(&self) -> Option<&InterfaceType> {
        match self {
            TypeData::Interface(d) => Some(d),
            TypeData::Tuple(d) => Some(&d.interface),
            _ => None,
        }
    }

    #[inline]
    pub fn as_interface_type_mut(&mut self) -> Option<&mut InterfaceType> {
        match self {
            TypeData::Interface(d) => Some(d),
            TypeData::Tuple(d) => Some(&mut d.interface),
            _ => None,
        }
    }

    // Go: checker/types.go:882 TypeBase.AsUnionOrIntersectionType
    #[inline]
    pub fn as_union_or_intersection_type(&self) -> Option<&UnionOrIntersectionType> {
        match self {
            TypeData::Union(d) => Some(&d.union_or_intersection),
            TypeData::Intersection(d) => Some(&d.union_or_intersection),
            _ => None,
        }
    }

    #[inline]
    pub fn as_union_or_intersection_type_mut(&mut self) -> Option<&mut UnionOrIntersectionType> {
        match self {
            TypeData::Union(d) => Some(&mut d.union_or_intersection),
            TypeData::Intersection(d) => Some(&mut d.union_or_intersection),
            _ => None,
        }
    }
}

// IntrinsicTypeData

// Go: checker/types.go:886 IntrinsicType
#[derive(Clone, Debug, Default)]
pub struct IntrinsicType {
    pub intrinsic_name: String,
}

impl IntrinsicType {
    // Go: checker/types.go:891 IntrinsicType.IntrinsicName
    pub fn intrinsic_name(&self) -> &str {
        &self.intrinsic_name
    }
}

// LiteralTypeData

/// Go `any` value of a literal type or evaluator result:
/// `string | jsnum.Number | bool | jsnum.PseudoBigInt`. Go `nil` (a computed
/// enum value) is `Option::None` around this.
#[derive(Clone, Debug, PartialEq)]
pub enum LiteralValue {
    String(String),
    Number(Number),
    Bool(bool),
    PseudoBigInt(PseudoBigInt),
}

// Go: checker/types.go:895 LiteralType
#[derive(Clone, Debug, Default)]
pub struct LiteralType {
    pub value: Option<LiteralValue>, // string | jsnum.Number | bool | PseudoBigInt | nil (computed enum)
    pub fresh_type: TypeId,          // Fresh version of type
    pub regular_type: TypeId,        // Regular version of type
    // PORT: no Go field. The property name of a string or number literal,
    // interned on first use by `get_property_name_from_type_as_name`. The
    // value never changes, so the name does not either.
    pub property_name: std::cell::OnceCell<Name>,
    // PORT: perf, no Go field. For a string literal, true when the value has
    // no `scanner_util::GO_STRING_MARKER`, so its port form is its Go bytes
    // (see `Checker::string_literal_go_plain`). Set on first use.
    pub go_plain: std::cell::OnceCell<bool>,
}

impl LiteralType {
    // Go: checker/types.go:902 LiteralType.Value
    pub fn value(&self) -> Option<&LiteralValue> {
        self.value.as_ref()
    }

    // Go: checker/types.go:906 LiteralType.FreshType
    pub fn fresh_type(&self) -> TypeId {
        self.fresh_type
    }

    // Go: checker/types.go:910 LiteralType.RegularType
    pub fn regular_type(&self) -> TypeId {
        self.regular_type
    }

    // Formats the raw value like Go `%v` on `LiteralType.value`. A nil value
    // (computed enum) prints as Go's `<nil>`.
    // Go: checker/checker.go:26996 indexType.AsLiteralType().value
    pub fn value_arg(&self) -> String {
        match &self.value {
            Some(value) => value.to_string(),
            None => "<nil>".to_string(),
        }
    }

    // PORT: Go `ValueToString(nil)` panics; unwrapping here panics the same way.
    // Go: checker/types.go:914 LiteralType.String
    pub fn string(&self) -> String {
        value_to_string(
            self.value
                .as_ref()
                .expect("unhandled value type in valueToString"),
        )
    }
}

// PORT: Go passes a literal type's raw `value` (an `any`) as a diagnostic
// argument, and diagnostics format arguments with `%v`. This `Display` impl
// gives the same text: a string prints raw (no quotes), a number uses its
// `String()`, a bool prints true/false and a bigint prints with no `n`.
// Go: diagnostics/diagnostics.go:146 fmt.Sprintf("%v", arg)
impl std::fmt::Display for LiteralValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LiteralValue::String(value) => f.write_str(value),
            LiteralValue::Number(value) => write!(f, "{value}"),
            LiteralValue::Bool(value) => write!(f, "{value}"),
            LiteralValue::PseudoBigInt(value) => write!(f, "{value}"),
        }
    }
}

// UniqueESSymbolTypeData

// Go: checker/types.go:920 UniqueESSymbolType
#[derive(Clone, Debug, Default)]
pub struct UniqueESSymbolType {
    pub name: String,
}

// ConstrainedType (type with computed base constraint)

// Go: checker/types.go:927 ConstrainedType
#[derive(Clone, Debug, Default)]
pub struct ConstrainedType {
    pub resolved_base_constraint: TypeId,
}

// StructuredType (base of all types with members)

// PORT: layout only. Go `signatures`, `callSignatureCount`, `indexInfos` and
// `objectTypeWithoutAbstractConstructSignatures` are in `StructuredSignatures`,
// out of line, so `Type` fits in 2 cache lines (see `Type`). Most type
// references have none of them (effect: 8.5% have signatures and 16% index
// infos), and the lists are 24 bytes each. Read them with the methods below
// and write them with `set_signatures` and
// `set_object_type_without_abstract_construct_signatures`.
// PORT: layout only. `repr(C)` keeps this field order (see `ObjectType`).
// Go: checker/types.go:936 StructuredType
#[derive(Clone, Debug, Default)]
#[repr(C)]
pub struct StructuredType {
    pub constrained: ConstrainedType,
    pub members: SymbolTable,
    /// `None` is empty lists, a zero count and a nil
    /// `object_type_without_abstract_construct_signatures`.
    pub signatures_data: Option<ArenaBox<StructuredSignatures>>,
    pub properties: SharedList<SymbolId>,
}

/// The out-of-line fields of `StructuredType`.
///
/// PERF: `align(64)`, so a record never spans 2 cache lines. `repr(C)` puts
/// the signatures and their count first.
// Go: checker/types.go:936 StructuredType
#[derive(Clone, Debug, Default)]
#[repr(C, align(64))]
pub struct StructuredSignatures {
    pub signatures: SharedList<SignatureId>, // Signatures (call + construct)
    pub call_signature_count: i32,           // Count of call signatures
    pub object_type_without_abstract_construct_signatures: TypeId,
    pub index_infos: SharedList<IndexInfoId>,
}

const _: () = assert!(std::mem::size_of::<StructuredSignatures>() == 64);

impl StructuredType {
    /// Go `signatures`: the call signatures, then the construct signatures.
    #[inline]
    pub fn signatures(&self) -> &[SignatureId] {
        match &self.signatures_data {
            Some(d) => &d.signatures,
            None => &[],
        }
    }

    /// Go `signatures` as a shared list (a copy of no elements).
    pub fn signatures_list(&self) -> SharedList<SignatureId> {
        match &self.signatures_data {
            Some(d) => d.signatures.clone(),
            None => SharedList::default(),
        }
    }

    /// Go `callSignatureCount`.
    #[inline]
    pub fn call_signature_count(&self) -> i32 {
        self.signatures_data
            .as_ref()
            .map_or(0, |d| d.call_signature_count)
    }

    /// Go `indexInfos`.
    #[inline]
    pub fn index_infos(&self) -> &[IndexInfoId] {
        match &self.signatures_data {
            Some(d) => &d.index_infos,
            None => &[],
        }
    }

    /// Go `indexInfos` as a shared list (a copy of no elements).
    pub fn index_infos_list(&self) -> SharedList<IndexInfoId> {
        match &self.signatures_data {
            Some(d) => d.index_infos.clone(),
            None => SharedList::default(),
        }
    }

    /// Go `objectTypeWithoutAbstractConstructSignatures`.
    pub fn object_type_without_abstract_construct_signatures(&self) -> TypeId {
        self.signatures_data.as_ref().map_or(TypeId::NIL, |d| {
            d.object_type_without_abstract_construct_signatures
        })
    }

    /// Sets Go `signatures`, `callSignatureCount` and `indexInfos`. Empty
    /// lists on a type with no `signatures_data` make none.
    pub fn set_signatures(
        &mut self,
        signatures: SharedList<SignatureId>,
        call_signature_count: i32,
        index_infos: SharedList<IndexInfoId>,
    ) {
        match &mut self.signatures_data {
            Some(d) => {
                d.signatures = signatures;
                d.call_signature_count = call_signature_count;
                d.index_infos = index_infos;
            }
            None if signatures.is_empty() && index_infos.is_empty() => {}
            None => {
                self.signatures_data = Some(ArenaBox::new(StructuredSignatures {
                    signatures,
                    call_signature_count,
                    object_type_without_abstract_construct_signatures: TypeId::NIL,
                    index_infos,
                }));
            }
        }
    }

    /// Sets Go `objectTypeWithoutAbstractConstructSignatures`.
    pub fn set_object_type_without_abstract_construct_signatures(&mut self, t: TypeId) {
        self.signatures_data
            .get_or_insert_with(|| ArenaBox::new(StructuredSignatures::default()))
            .object_type_without_abstract_construct_signatures = t;
    }

    // Go: checker/types.go:949 StructuredType.CallSignatures
    pub fn call_signatures(&self) -> &[SignatureId] {
        match &self.signatures_data {
            Some(d) => &d.signatures[..d.call_signature_count as usize],
            None => &[],
        }
    }

    // Go: checker/types.go:953 StructuredType.ConstructSignatures
    pub fn construct_signatures(&self) -> &[SignatureId] {
        match &self.signatures_data {
            Some(d) => &d.signatures[d.call_signature_count as usize..],
            None => &[],
        }
    }

    // Go: checker/types.go:957 StructuredType.Properties
    pub fn properties(&self) -> &[SymbolId] {
        &self.properties
    }
}

// Except for tuple type references and reverse mapped types, all object types have an associated symbol.
// Possible object type instances are listed in the following.

// InterfaceType:
// ObjectFlagsClass: Originating non-generic class type
// ObjectFlagsClass|ObjectFlagsReference: Originating generic class type
// ObjectFlagsInterface: Originating non-generic interface type
// ObjectFlagsInterface|ObjectFlagsReference: Originating generic interface type

// TupleType:
// ObjectFlagsReference|ObjectFlagsTuple: Originating generic tuple type (synthesized)

// TypeReference
// ObjectFlagsReference: Instantiated generic class, interface, or tuple type

// ObjectType:
// ObjectFlagsAnonymous: Originating anonymous object type
// ObjectFlagsAnonymous|ObjectFlagsInstantiated: Instantiated anonymous object type

// MappedType:
// ObjectFlagsMapped: Originating mapped type
// ObjectFlagsMapped|ObjectFlagsInstantiated: Instantiated mapped type

// InstantiationExpressionType:
// ObjectFlagsAnonymous|ObjectFlagsInstantiationExpression: Originating instantiation expression type
// ObjectFlagsAnonymous|ObjectFlagsInstantiated|ObjectFlagsInstantiationExpression: Instantiated instantiation expression type

// ReverseMappedType:
// ObjectFlagsAnonymous|ObjectFlagsReverseMapped: Reverse mapped type

// EvolvingArrayType:
// ObjectFlagsEvolvingArray: Evolving array type

/// The instantiation cache of a generic target: `ObjectType`,
/// `TypeAliasLinks` and `ConditionalRoot` `instantiations` (Go
/// `map[CacheHashKey]*Type`).
///
/// PERF: a `FlatMap` probe reads the key and value in one cache line;
/// hashbrown reads a control group first. The maps are never iterated, so
/// the layout cannot change any output. For an A/B run against hashbrown,
/// change it to `CacheKeyMap<TypeId>`.
pub type InstantiationMap = FlatMap<CacheHashKey, TypeId>;

// PORT: Go `instantiations` (the map of type instantiations) is not here.
// Few object types have one (effect: 7.6k anonymous object types of 129k,
// 95 type references of 165k), so the 32-byte map would grow every `Type`.
// An interface or tuple keeps it in `InterfaceType`; any other object type
// has it in `Checker::object_type_instantiations`, at the index in
// `instantiations`. Read and write it with `Checker::object_instantiations`
// and `object_instantiations_mut`.
// PORT: layout only. `repr(C)` keeps `target` and `mapper` first. An
// anonymous object type then has them, its members table and its
// signatures on the first cache line of its `Type` (see `TypeReference`).
// Go: checker/types.go:994 ObjectType
#[derive(Clone, Default)]
#[repr(C)]
pub struct ObjectType {
    pub target: TypeId,   // Target of instantiated type
    pub mapper: MapperId, // Type mapper for instantiated type
    pub structured: StructuredType,
    pub instantiations: InstantiationMapId, // Map of type instantiations
}

/// The index of an object type's instantiation map in
/// `Checker::object_type_instantiations`. `NIL` is a Go nil map, and it is
/// always `NIL` on an interface or tuple (see `ObjectType`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InstantiationMapId(pub u32);

impl InstantiationMapId {
    pub const NIL: Self = Self(0);
}

// TypeReference (instantiation of an InterfaceType)

// PORT: layout only. `repr(C)` keeps this field order, so the first cache
// line of a type reference's `Type` holds the `TypeData` tag (the memo, see
// `GenericArgumentsMemo`), the type arguments, the target and the mapper.
// The resolved members and `node` are on the second line.
// Go: checker/types.go:1005 TypeReference
#[derive(Clone, Default)]
#[repr(C)]
pub struct TypeReference {
    // PORT: no Go field. Memo of `is_type_reference_with_generic_arguments`
    // for a non-deferred reference with a non-empty resolved list.
    pub generic_arguments_memo: GenericArgumentsMemo,
    pub resolved_type_arguments: SharedList<TypeId>,
    pub object: ObjectType,
    pub node: Node, // TypeReferenceNode | ArrayTypeNode | TupleTypeNode when deferred, else nil
}

/// Memo state of `Checker::is_type_reference_with_generic_arguments`.
///
/// PORT: layout only. An enum (not an integer) leaves invalid values that
/// `TypeData` uses for its tag, so the tag does not grow `Type`. It is
/// `u64`: the tags of `SharedList` and `Option<ArenaBox>` are 8 bytes wide
/// too, and of fields with the same number of invalid values the compiler
/// takes the first one. So the `TypeData` tag is this field, on the first
/// cache line, and the other kinds fit after it (with a `u32` memo the
/// compiler took a list tag on the second line, and `TypeData` grew by an
/// 8-byte tag). The field has 8 bytes of room anyway: the list after it is
/// 8-byte aligned.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u64)]
pub enum GenericArgumentsMemo {
    #[default]
    Unknown = 0,
    False = 1,
    True = 2,
}

// InterfaceType (when generic, serves as reference to instantiation of itself)

// Go: checker/types.go:1015 InterfaceType
#[derive(Clone, Default)]
pub struct InterfaceType {
    pub reference: TypeReference,
    pub all_type_parameters: Vec<TypeId>, // Type parameters (outer + local + thisType)
    pub outer_type_parameter_count: i32,  // Count of outer type parameters
    pub this_type: TypeId,                // The "this" type (nil if none)
    pub base_types_resolved: bool,
    pub declared_members_resolved: bool,
    pub resolved_base_constructor_type: TypeId,
    // PERF: shared, so `get_base_types_shared` returns it without a copy.
    pub resolved_base_types: SharedList<TypeId>,
    pub declared_members: SymbolTable, // Declared members
    // PERF: shared, so `resolve_object_type_members` reads them without a
    // copy, and a type without instantiation stores the same lists, as Go
    // shares its slices.
    pub declared_call_signatures: SharedList<SignatureId>, // Declared call signatures
    pub declared_construct_signatures: SharedList<SignatureId>, // Declared construct signatures
    pub declared_index_infos: SharedList<IndexInfoId>,     // Declared index signatures
    // PORT: Go `ObjectType.instantiations` of an interface or tuple (see
    // `ObjectType`): the type references of a generic target, keyed by
    // their type list. Go nil map is `None`.
    pub instantiations: Option<InstantiationMap>,
}

impl InterfaceType {
    // Go: checker/types.go:1053 InterfaceType.ThisType
    pub fn this_type(&self) -> TypeId {
        self.this_type
    }

    // Go: checker/types.go:1032 InterfaceType.OuterTypeParameters
    pub fn outer_type_parameters(&self) -> &[TypeId] {
        if self.all_type_parameters.is_empty() {
            return &[];
        }
        &self.all_type_parameters[..self.outer_type_parameter_count as usize]
    }

    // Go: checker/types.go:1039 InterfaceType.LocalTypeParameters
    pub fn local_type_parameters(&self) -> &[TypeId] {
        if self.all_type_parameters.is_empty() {
            return &[];
        }
        &self.all_type_parameters
            [self.outer_type_parameter_count as usize..self.all_type_parameters.len() - 1]
    }

    // Go: checker/types.go:1046 InterfaceType.TypeParameters
    pub fn type_parameters(&self) -> &[TypeId] {
        if self.all_type_parameters.is_empty() {
            return &[];
        }
        &self.all_type_parameters[..self.all_type_parameters.len() - 1]
    }
}

// TupleType

// Go: checker/types.go:1073 TupleElementInfo
#[derive(Clone, Copy, Debug, Default)]
pub struct TupleElementInfo {
    pub flags: ElementFlags,
    pub labeled_declaration: Node, // NamedTupleMember | ParameterDeclaration | nil
}

impl TupleElementInfo {
    // Go: checker/types.go:1078 TupleElementInfo.TupleElementFlags
    pub fn tuple_element_flags(&self) -> ElementFlags {
        self.flags
    }

    // Go: checker/types.go:1079 TupleElementInfo.LabeledDeclaration
    pub fn labeled_declaration(&self) -> Node {
        self.labeled_declaration
    }
}

// Go: checker/types.go:1081 TupleType
#[derive(Clone, Default)]
pub struct TupleType {
    pub interface: InterfaceType,
    pub element_infos: Vec<TupleElementInfo>,
    pub min_length: i32,   // Number of required or variadic elements
    pub fixed_length: i32, // Number of initial required or optional elements
    pub combined_flags: ElementFlags,
    pub readonly: bool,
}

impl TupleType {
    // Go: checker/types.go:1090 TupleType.FixedLength
    pub fn fixed_length(&self) -> i32 {
        self.fixed_length
    }

    // Go: checker/types.go:1091 TupleType.IsReadonly
    pub fn is_readonly(&self) -> bool {
        self.readonly
    }

    // Go: checker/types.go:1092 TupleType.ElementFlags
    pub fn element_flags(&self) -> Vec<ElementFlags> {
        let mut element_flags = vec![ElementFlags::NONE; self.element_infos.len()];
        for (i, info) in self.element_infos.iter().enumerate() {
            element_flags[i] = info.flags;
        }
        element_flags
    }

    // Go: checker/types.go:1099 TupleType.ElementInfos
    pub fn element_infos(&self) -> &[TupleElementInfo] {
        &self.element_infos
    }
}

// InstantiationExpressionType

// Go: checker/types.go:1103 InstantiationExpressionType
#[derive(Clone, Default)]
pub struct InstantiationExpressionType {
    pub object: ObjectType,
    pub node: Node,
}

// MappedType

// Go: checker/types.go:1110 MappedType
#[derive(Clone, Default)]
pub struct MappedType {
    pub object: ObjectType,
    pub declaration: Node, // *ast.MappedTypeNode
    pub type_parameter: TypeId,
    pub constraint_type: TypeId,
    pub name_type: TypeId,
    pub template_type: TypeId,
    pub modifiers_type: TypeId,
    pub resolved_apparent_type: TypeId,
    pub contains_error: bool,
}

impl MappedType {
    // Go: checker/types.go:1122 MappedType.TypeParameter
    pub fn type_parameter(&self) -> TypeId {
        self.type_parameter
    }

    // Go: checker/types.go:1123 MappedType.ConstraintType
    pub fn constraint_type(&self) -> TypeId {
        self.constraint_type
    }

    // Go: checker/types.go:1124 MappedType.NameType
    pub fn name_type(&self) -> TypeId {
        self.name_type
    }

    // Go: checker/types.go:1125 MappedType.TemplateType
    pub fn template_type(&self) -> TypeId {
        self.template_type
    }

    // Go: checker/types.go:1126 MappedType.ResolveComponents
    // PORT: the Go receiver is unused; the mapped type lives in the checker's
    // arena, so this takes the checker and the type and no `self`.
    pub fn resolve_components(c: &mut Checker, typ: TypeId) {
        c.get_type_parameter_from_mapped_type(typ);
        c.get_constraint_type_from_mapped_type(typ);
        c.get_name_type_from_mapped_type(typ);
        c.get_template_type_from_mapped_type(typ);
    }
}

// ReverseMappedType

// Go: checker/types.go:1135 ReverseMappedType
#[derive(Clone, Default)]
pub struct ReverseMappedType {
    pub object: ObjectType,
    pub source: TypeId,
    pub mapped_type: TypeId,
    pub constraint_type: TypeId,
}

// EvolvingArrayType

// Go: checker/types.go:1144 EvolvingArrayType
#[derive(Clone, Default)]
pub struct EvolvingArrayType {
    pub object: ObjectType,
    pub element_type: TypeId,
    pub final_array_type: TypeId,
}

// UnionOrIntersectionTypeData

// Go: checker/types.go:1152 UnionOrIntersectionType
#[derive(Clone, Debug, Default)]
pub struct UnionOrIntersectionType {
    pub structured: StructuredType,
    pub types: SharedList<TypeId>,
    pub property_cache: SymbolTable,
    pub property_cache_without_function_property_augment: SymbolTable,
    pub resolved_properties: SharedList<SymbolId>,
}

impl UnionOrIntersectionType {
    // Go: checker/types.go:1162 UnionOrIntersectionType.Types
    pub fn types(&self) -> &[TypeId] {
        &self.types
    }
}

// UnionType

// Go: checker/types.go:1168 UnionType
#[derive(Clone, Debug, Default)]
pub struct UnionType {
    pub union_or_intersection: UnionOrIntersectionType,
    pub resolved_reduced_type: TypeId,
    pub regular_type: TypeId,
    pub origin: TypeId, // Denormalized union, intersection, or index type in which union originates
    pub key_property_name: String, // Property with unique unit type that exists in every object/intersection in union type
    // PORT: Go nil map is `None`.
    pub constituent_map: Option<FxHashMap<TypeId, TypeId>>, // Constituents keyed by unit type discriminants
}

// IntersectionType

// Go: checker/types.go:1179 IntersectionType
#[derive(Clone, Debug, Default)]
pub struct IntersectionType {
    pub union_or_intersection: UnionOrIntersectionType,
    pub resolved_apparent_type: TypeId,
    pub unique_literal_filled_instantiation: TypeId, // Instantiation with type parameters mapped to never type
}

// TypeParameter

// Go: checker/types.go:1187 TypeParameter
#[derive(Clone, Debug, Default)]
pub struct TypeParameter {
    pub constrained: ConstrainedType,
    pub constraint: TypeId,
    pub target: TypeId,
    pub mapper: MapperId,
    pub is_this_type: bool,
    pub is_distributed: bool,
    pub resolved_default_type: TypeId,
    pub distributed_type: TypeId,
}

impl TypeParameter {
    // Go: checker/types.go:1198 TypeParameter.IsThisType
    pub fn is_this_type(&self) -> bool {
        self.is_this_type
    }
}

// IndexType

// Go: checker/types.go:1213 IndexType
#[derive(Clone, Debug, Default)]
pub struct IndexType {
    pub constrained: ConstrainedType,
    pub target: TypeId,
    pub index_flags: IndexFlags,
}

impl IndexType {
    // Go: checker/types.go:1219 IndexType.Target
    #[inline]
    pub fn target(&self) -> TypeId {
        self.target
    }
}

// IndexedAccessType

// Go: checker/types.go:1223 IndexedAccessType
#[derive(Clone, Debug, Default)]
pub struct IndexedAccessType {
    pub constrained: ConstrainedType,
    pub object_type: TypeId,
    pub index_type: TypeId,
    pub access_flags: AccessFlags, // Only includes AccessFlags.Persistent
}

impl IndexedAccessType {
    // Go: checker/types.go:1230 IndexedAccessType.ObjectType
    pub fn object_type(&self) -> TypeId {
        self.object_type
    }

    // Go: checker/types.go:1231 IndexedAccessType.IndexType
    pub fn index_type(&self) -> TypeId {
        self.index_type
    }
}

// PORT: Go shares the `texts` and `types` slices; `Rc<[_]>` makes the
// clones cheap. The contents never change after creation.
// Go: checker/types.go:1233 TemplateLiteralType
#[derive(Clone, Debug, Default)]
pub struct TemplateLiteralType {
    pub constrained: ConstrainedType,
    pub texts: Rc<[String]>,       // Always one element longer than types
    pub types: SharedList<TypeId>, // Always at least one element
    // PORT: perf, no Go field. True when no text has a
    // `scanner_util::GO_STRING_MARKER`, so each text's port form is its Go
    // bytes. Set by `new_template_literal_type`; false (the default) means
    // "convert first".
    pub go_plain: bool,
}

impl TemplateLiteralType {
    // Go: checker/types.go:1239 TemplateLiteralType.Texts
    pub fn texts(&self) -> &[String] {
        &self.texts
    }

    // Go: checker/types.go:1240 TemplateLiteralType.Types
    pub fn types(&self) -> &[TypeId] {
        &self.types
    }
}

// Go: checker/types.go:1242 StringMappingType
#[derive(Clone, Debug, Default)]
pub struct StringMappingType {
    pub constrained: ConstrainedType,
    pub target: TypeId,
}

impl StringMappingType {
    // Go: checker/types.go:1247 StringMappingType.Target
    #[inline]
    pub fn target(&self) -> TypeId {
        self.target
    }
}

// Go: checker/types.go:1249 SubstitutionType
#[derive(Clone, Debug, Default)]
pub struct SubstitutionType {
    pub constrained: ConstrainedType,
    pub base_type: TypeId,  // Target type
    pub constraint: TypeId, // Constraint that target type is known to satisfy
}

impl SubstitutionType {
    // Go: checker/types.go:1255 SubstitutionType.BaseType
    pub fn base_type(&self) -> TypeId {
        self.base_type
    }

    // Go: checker/types.go:1256 SubstitutionType.SubstConstraint
    pub fn subst_constraint(&self) -> TypeId {
        self.constraint
    }
}

// PORT: Go shares one `*ConditionalRoot` between a conditional type and all
// its instantiations and mutates `instantiations`, so `ConditionalType.root`
// is `Rc<RefCell<ConditionalRoot>>`.
// Go: checker/types.go:1258 ConditionalRoot
#[derive(Clone, Default)]
pub struct ConditionalRoot {
    pub node: Node, // *ast.ConditionalTypeNode
    pub check_type: TypeId,
    pub extends_type: TypeId,
    pub is_distributive: bool,
    pub infer_type_parameters: Vec<TypeId>,
    // PORT: shared, so an instantiation miss hands it to the new mapper
    // without a copy. It never changes after the root is created.
    pub outer_type_parameters: SharedList<TypeId>,
    // PORT: Go nil map is `None`.
    pub instantiations: Option<InstantiationMap>,
    pub alias: Option<Rc<TypeAlias>>,
    // PERF: not in Go. The answer of `is_distribution_dependent` once its
    // first walk ends (`None` before), so a repeat call does not walk again.
    pub distribution_dependent: Option<bool>,
}

// Go: checker/types.go:1269 ConditionalType
#[derive(Clone, Default)]
pub struct ConditionalType {
    pub constrained: ConstrainedType,
    pub root: Rc<RefCell<ConditionalRoot>>,
    pub check_type: TypeId,
    pub extends_type: TypeId,
    pub resolved_true_type: TypeId,
    pub resolved_false_type: TypeId,
    pub resolved_inferred_true_type: TypeId, // The `trueType` instantiated with the `combinedMapper`, if present
    pub resolved_default_constraint: TypeId,
    pub resolved_constraint_of_distributive: TypeId,
    pub mapper: MapperId,
    pub combined_mapper: MapperId,
}

impl ConditionalType {
    // Go: checker/types.go:1283 ConditionalType.CheckType
    pub fn check_type(&self) -> TypeId {
        self.check_type
    }

    // Go: checker/types.go:1284 ConditionalType.ExtendsType
    pub fn extends_type(&self) -> TypeId {
        self.extends_type
    }
}

// Signature

// Go: checker/types.go:1312 Signature
#[derive(Clone, Debug, Default)]
pub struct Signature {
    pub id: SignatureId,
    pub flags: SignatureFlags,
    pub min_argument_count: i32,
    pub resolved_min_argument_count: i32,
    pub declaration: Node,
    pub type_parameters: Vec<TypeId>,
    // PORT: Go compares signature type parameter lists by slice identity
    // (`core.Same` in compareSignaturesRelated). A Rust signature owns its
    // `Vec`, so a list that Go shares between signatures (cloneSignature,
    // class local type parameters, inferred type parameters, ...) carries
    // the same nonzero origin here. 0 means the list belongs only to this
    // signature. See `Checker::same_signature_type_parameters`.
    pub type_parameters_origin: u32,
    pub parameters: Vec<SymbolId>,
    pub this_parameter: SymbolId,
    pub resolved_return_type: TypeId,
    pub resolved_type_predicate: TypePredicateId,
    pub target: SignatureId,
    pub mapper: MapperId,
    pub isolated_signature_type: TypeId,
    // PORT: Go `*CompositeSignature` is created once and only read; nil is `None`.
    pub composite: Option<Rc<CompositeSignature>>,
}

impl Signature {
    // Go: checker/types.go:1329 Signature.Id
    pub fn id(&self) -> SignatureId {
        self.id
    }

    // Go: checker/types.go:1333 Signature.Flags
    pub fn flags(&self) -> SignatureFlags {
        self.flags
    }

    // Go: checker/types.go:1337 Signature.TypeParameters
    pub fn type_parameters(&self) -> &[TypeId] {
        &self.type_parameters
    }

    // Go: checker/types.go:1341 Signature.Declaration
    pub fn declaration(&self) -> Node {
        self.declaration
    }

    // Go: checker/types.go:1345 Signature.Target
    #[inline]
    pub fn target(&self) -> SignatureId {
        self.target
    }

    // Go: checker/types.go:1349 Signature.ThisParameter
    pub fn this_parameter(&self) -> SymbolId {
        self.this_parameter
    }

    // Go: checker/types.go:1353 Signature.Parameters
    pub fn parameters(&self) -> &[SymbolId] {
        &self.parameters
    }

    // Go: checker/types.go:1357 Signature.HasRestParameter
    pub fn has_rest_parameter(&self) -> bool {
        self.flags.intersects(SignatureFlags::HAS_REST_PARAMETER)
    }

    // Go: checker/types.go:1361 Signature.MinArgumentCount
    pub fn min_argument_count(&self) -> i32 {
        self.min_argument_count
    }
}

// Go: checker/types.go:1365 CompositeSignature
#[derive(Clone, Debug, Default)]
pub struct CompositeSignature {
    pub is_union: bool,               // True for union, false for intersection
    pub signatures: Vec<SignatureId>, // Individual signatures
}

// Go: checker/types.go:1379 TypePredicate
#[derive(Clone, Debug, Default)]
pub struct TypePredicate {
    pub kind: TypePredicateKind,
    pub parameter_index: i32,
    pub parameter_name: String,
    // PORT: Go field `t`.
    pub t: TypeId,
}

impl TypePredicate {
    // Go: checker/types.go:1386 TypePredicate.Type
    pub fn type_(&self) -> TypeId {
        self.t
    }

    // Go: checker/types.go:1390 TypePredicate.Kind
    pub fn kind(&self) -> TypePredicateKind {
        self.kind
    }

    // Go: checker/types.go:1394 TypePredicate.ParameterIndex
    pub fn parameter_index(&self) -> i32 {
        self.parameter_index
    }

    // Go: checker/types.go:1398 TypePredicate.ParameterName
    pub fn parameter_name(&self) -> &str {
        &self.parameter_name
    }
}

// IndexInfo

// Go: checker/types.go:1404 IndexInfo
#[derive(Clone, Debug, Default)]
pub struct IndexInfo {
    pub key_type: TypeId,
    pub value_type: TypeId,
    pub is_readonly: bool,
    pub declaration: Node,      // IndexSignatureDeclaration
    pub index_symbol: SymbolId, // Synthetic property symbol for this index signature
    pub components: Vec<Node>,  // ElementWithComputedPropertyName
}

impl IndexInfo {
    // Go: checker/types.go:1413 IndexInfo.KeyType
    pub fn key_type(&self) -> TypeId {
        self.key_type
    }

    // Go: checker/types.go:1417 IndexInfo.ValueType
    pub fn value_type(&self) -> TypeId {
        self.value_type
    }

    // Go: checker/types.go:1421 IndexInfo.IsReadonly
    pub fn is_readonly(&self) -> bool {
        self.is_readonly
    }

    // Go: checker/types.go:1425 IndexInfo.Declaration
    pub fn declaration(&self) -> Node {
        self.declaration
    }
}

/*
 * Ternary values are defined such that
 * x & y picks the lesser in the order False < Unknown < Maybe < True, and
 * x | y picks the greater in the order False < Unknown < Maybe < True.
 * Generally, Ternary.Maybe is used as the result of a relation that depends on itself, and
 * Ternary.Unknown is used as the result of a variance check that depends on itself. We make
 * a distinction because we don't want to cache circular variance check results.
 */
// PORT: `Ternary` itself is generated in `crate::flags` (a `go_enum!`).
// These operators give Go's `x & y` and `x | y` on the underlying int8.
impl std::ops::BitAnd for Ternary {
    type Output = Self;
    fn bitand(self, rhs: Self) -> Self {
        Self(self.0 & rhs.0)
    }
}

impl std::ops::BitAndAssign for Ternary {
    fn bitand_assign(&mut self, rhs: Self) {
        self.0 &= rhs.0;
    }
}

impl std::ops::BitOr for Ternary {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

impl std::ops::BitOrAssign for Ternary {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

// PORT: Go `TypeComparer` is a func value stored in checker fields and
// inference contexts, so it is an `Rc<dyn Fn>` that gets the checker first.
// Go: checker/types.go:1446 TypeComparer
pub type TypeComparer = Rc<dyn Fn(&mut Checker, TypeId, TypeId, bool) -> Ternary>;

// Go: checker/types.go:1448 LanguageFeatureMinimumTargetMap
#[derive(Clone, Copy, Debug)]
pub struct LanguageFeatureMinimumTargetMap {
    pub exponentiation: ScriptTarget,
    pub async_functions: ScriptTarget,
    pub for_await_of: ScriptTarget,
    pub async_generators: ScriptTarget,
    pub async_iteration: ScriptTarget,
    pub object_spread_rest: ScriptTarget,
    pub regular_expression_flags_dot_all: ScriptTarget,
    pub bindingless_catch: ScriptTarget,
    pub big_int: ScriptTarget,
    pub nullish_coalesce: ScriptTarget,
    pub optional_chaining: ScriptTarget,
    pub logical_assignment: ScriptTarget,
    pub top_level_await: ScriptTarget,
    pub class_fields: ScriptTarget,
    pub private_names_and_class_static_blocks: ScriptTarget,
    pub regular_expression_flags_has_indices: ScriptTarget,
    pub shebang_comments: ScriptTarget,
    pub using_and_await_using: ScriptTarget,
    pub class_and_class_element_decorators: ScriptTarget,
    pub regular_expression_flags_unicode_sets: ScriptTarget,
}

// PORT: Go package var `LanguageFeatureMinimumTarget` is never mutated, so
// it is a Rust const.
// Go: checker/types.go:1471 LanguageFeatureMinimumTarget
pub const LANGUAGE_FEATURE_MINIMUM_TARGET: LanguageFeatureMinimumTargetMap =
    LanguageFeatureMinimumTargetMap {
        exponentiation: ScriptTarget::ES2016,
        async_functions: ScriptTarget::ES2017,
        for_await_of: ScriptTarget::ES2018,
        async_generators: ScriptTarget::ES2018,
        async_iteration: ScriptTarget::ES2018,
        object_spread_rest: ScriptTarget::ES2018,
        regular_expression_flags_dot_all: ScriptTarget::ES2018,
        bindingless_catch: ScriptTarget::ES2019,
        big_int: ScriptTarget::ES2020,
        nullish_coalesce: ScriptTarget::ES2020,
        optional_chaining: ScriptTarget::ES2020,
        logical_assignment: ScriptTarget::ES2021,
        top_level_await: ScriptTarget::ES2022,
        class_fields: ScriptTarget::ES2022,
        private_names_and_class_static_blocks: ScriptTarget::ES2022,
        regular_expression_flags_has_indices: ScriptTarget::ES2022,
        shebang_comments: ScriptTarget::ES_NEXT,
        using_and_await_using: ScriptTarget::ES_NEXT,
        class_and_class_element_decorators: ScriptTarget::ES_NEXT,
        regular_expression_flags_unicode_sets: ScriptTarget::ES_NEXT,
    };

// Aliases for types
// Go: checker/types.go:1495 StringLiteralType
pub type StringLiteralType = Type;

// PORT: no Go counterpart. Go allocates each `Type` and `TypeMapper` on its
// own; the port keeps them in index arenas. A plain `Vec` arena doubles and
// copies every entry when it grows, and keeps up to half its capacity unused.
// `ChunkedArena` stores entries in chunks of `ARENA_CHUNK_LEN`, so growing it
// never moves an entry of a later chunk. Index 0 is the nil dummy, like the
// other arenas.
//
// PERF (rss2): the first chunk starts at `ARENA_FIRST_CAPACITY` entries. It
// doubles up to `ARENA_CHUNK_STEP` entries, then grows in steps of as many
// until it is full. A first chunk made at full size was mostly allocated but
// never written in a small program (query: about 5,300 of 8,192 `Type` slots
// in each checker), and jemalloc gives the heap huge pages, which make such
// memory resident. Later chunks are made at full size: a program that fills
// the first chunk is large, and growing each chunk too cost about 1.5% of
// check time on hono, zod and effect (rss2 grid: copies and page faults).
pub struct ChunkedArena<T> {
    chunks: Vec<Vec<T>>,
    len: usize,
}

// PERF: 8,192 entries a chunk. Every `ty(t)` reads the chunk table before the
// entry, so a smaller table stays in L1 more (elysia: 720 chunks, 17 KB, was
// 1,440). One chunk of each element type stays under 2 MiB, so a small project
// does not get a 2 MiB huge page for each arena. `Type` is 128 bytes, so 13 is
// the largest shift under that limit.
const ARENA_CHUNK_SHIFT: usize = 13;
const ARENA_CHUNK_LEN: usize = 1 << ARENA_CHUNK_SHIFT;
const ARENA_CHUNK_MASK: usize = ARENA_CHUNK_LEN - 1;
const ARENA_CHUNK_MAX_BYTES: usize = 2 << 20;
/// Each step of the first chunk after it has this many entries adds as many.
const ARENA_CHUNK_STEP: usize = ARENA_CHUNK_LEN / 4;
/// The room of the first chunk when the arena is made.
const ARENA_FIRST_CAPACITY: usize = 64;
const _: () = assert!(std::mem::size_of::<Type>() * ARENA_CHUNK_LEN < ARENA_CHUNK_MAX_BYTES);
const _: () = assert!(std::mem::size_of::<TypeMapper>() * ARENA_CHUNK_LEN < ARENA_CHUNK_MAX_BYTES);

impl<T> ChunkedArena<T> {
    /// Creates an arena that holds only `nil`, the dummy entry at index 0.
    pub fn with_nil(nil: T) -> Self {
        let mut arena = ChunkedArena {
            chunks: vec![Vec::with_capacity(ARENA_FIRST_CAPACITY)],
            len: 1,
        };
        arena.chunks[0].push(nil);
        arena
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    // PERF: always inlined, so the caller builds `value` straight in its
    // chunk slot. Out of line, the caller wrote the value to the stack with
    // small stores and this function read it back at once with 16-byte
    // loads (a store forwarding stall), or copied a `Type` with `memmove`.
    // The rare new-chunk path stays out of line.
    #[inline(always)]
    pub fn push(&mut self, value: T) {
        self.push_with(move || value);
    }

    /// Pushes the value `make` returns. `make` runs after the chunk slot is
    /// ready, so a large value (a `Type`) is built straight in its slot.
    ///
    /// PERF: with `push(make())` the value is built first, and the grow
    /// branch of `Vec::push` then sits between building and storing, so the
    /// value goes to the stack and is copied with `memcpy`. Here the room
    /// tests come first. The second one never fails: a full first chunk gets
    /// its next step (`grow_chunk`), a later chunk is made with room for
    /// `ARENA_CHUNK_LEN` entries, and the entry after a full chunk gets a new
    /// chunk. When `make` makes no call, LLVM knows the chunk did not change
    /// after the second test and drops the grow branch of the `Vec::push`
    /// below. When the first test fails (no step), the second one is known to
    /// fail too, so the hot path makes one test, as before.
    #[inline(always)]
    pub fn push_with(&mut self, make: impl FnOnce() -> T) {
        if self.len & ARENA_CHUNK_MASK == 0 {
            self.add_chunk();
        }
        let chunk = self.chunks.last_mut().expect("arena chunk");
        if chunk.len() == chunk.capacity() {
            grow_chunk(chunk);
        }
        if chunk.len() == chunk.capacity() {
            arena_chunk_full();
        }
        chunk.push(make());
        self.len += 1;
    }

    /// Adds the empty chunk for the next `ARENA_CHUNK_LEN` entries.
    #[cold]
    #[inline(never)]
    fn add_chunk(&mut self) {
        self.chunks.push(Vec::with_capacity(ARENA_CHUNK_LEN));
    }
}

/// Gives the full first chunk room for its next `ARENA_CHUNK_STEP` entries,
/// or doubles it while it is smaller than that. Later chunks are made full
/// size and never get here.
#[cold]
#[inline(never)]
fn grow_chunk<T>(chunk: &mut Vec<T>) {
    chunk.reserve_exact(chunk.len().min(ARENA_CHUNK_STEP));
}

/// The last arena chunk has no room after `grow_chunk`. `push_with` never
/// gets here.
#[cold]
#[inline(never)]
fn arena_chunk_full() -> ! {
    panic!("arena chunk is full")
}

impl<T> std::ops::Index<usize> for ChunkedArena<T> {
    type Output = T;

    #[inline]
    fn index(&self, i: usize) -> &T {
        &self.chunks[i >> ARENA_CHUNK_SHIFT][i & ARENA_CHUNK_MASK]
    }
}

impl<T> std::ops::IndexMut<usize> for ChunkedArena<T> {
    #[inline]
    fn index_mut(&mut self, i: usize) -> &mut T {
        &mut self.chunks[i >> ARENA_CHUNK_SHIFT][i & ARENA_CHUNK_MASK]
    }
}
