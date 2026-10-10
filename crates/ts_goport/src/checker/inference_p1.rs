use crate::prelude::*;
use smallvec::SmallVec;

// PORT: Go `*InferenceState` is pooled on the checker
// (`Checker::freeinference_state: Option<Rc<RefCell<InferenceState>>>`).
// PERF: `infer_types` holds the one mutable borrow of the pooled state for
// the whole inference, and every inference function takes
// `n: &mut InferenceState`, so the hot paths make no `RefCell` checks. A
// nested inference takes another state from the pool, never this one.

// Go: checker/inference.go:11 InferenceKey
// ts#64553 (Go N' inference.go:11): the key also holds the priority and the
// variance of the inference state, so a cached inference from type arguments
// is reused only in the same state.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct InferenceKey {
    pub source: TypeId,
    pub target: TypeId,
    pub priority: InferencePriority,
    pub contravariant: bool,
    pub bivariant: bool,
}

impl InferenceKey {
    /// PORT: perf. The two ids as one word.
    #[inline]
    fn ids_word(&self) -> u64 {
        ((self.source.0 as u64) << 32) | self.target.0 as u64
    }

    /// PORT: perf. The priority bits and the two variance flags as one word.
    #[inline]
    fn state_word(&self) -> u64 {
        ((self.priority.bits() as u32 as u64) << 2)
            | ((self.contravariant as u64) << 1)
            | self.bivariant as u64
    }
}

// PORT: the ids are hashed as one word and the state as a second word. No
// code iterates maps with these keys.
impl std::hash::Hash for InferenceKey {
    #[inline]
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        state.write_u64(self.ids_word());
        state.write_u64(self.state_word());
    }
}

// PORT: perf. The same words through an Fx mix, which is the hash that
// `FxHashMap` computes for this key.
impl FlatKey for InferenceKey {
    #[inline]
    fn flat_hash(&self) -> u64 {
        use std::hash::Hasher;
        let mut h = rustc_hash::FxHasher::default();
        h.write_u64(self.ids_word());
        h.write_u64(self.state_word());
        h.finish()
    }
}

/// The map type of `InferenceState::visited`. For an A/B run against
/// hashbrown, change it to `FxHashMap<InferenceKey, InferencePriority>`.
pub type InferenceVisitedMap = FlatMap<InferenceKey, InferencePriority>;

/// PORT: perf. Not in Go. `stack_recursion_id` as one word, so the inference
/// stacks store and compare ids with plain 8-byte moves. 0 is `None`. A node
/// handle has nonzero low 32 bits, so it is its own key. A symbol or type
/// id goes in the high 32 bits with zero low bits: a symbol as its id, a
/// type as its id with bit 31 set. An id that does not fit (a node with
/// zero low bits, a nil symbol, or a symbol or type id of 2^31 or more) is
/// stored as `None`. Such an entry is not a mapped type, an indexed access
/// or an intersection, so `has_matching_recursion_identity` on it only
/// compares `get_recursion_identity_from_target` and gives the same answer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RecursionKey(u64);

impl RecursionKey {
    pub const NONE: Self = Self(0);
    const TYPE_BIT: u32 = 1 << 31;

    #[inline]
    #[must_use]
    pub fn new(id: Option<RecursionId>) -> Self {
        match id {
            Some(RecursionId::Node(n)) if n.0 as u32 != 0 => Self(n.0),
            Some(RecursionId::Symbol(s)) if s.0 != 0 && s.0 < Self::TYPE_BIT => {
                Self(u64::from(s.0) << 32)
            }
            Some(RecursionId::Type(t)) if t.0 < Self::TYPE_BIT => {
                Self(u64::from(t.0 | Self::TYPE_BIT) << 32)
            }
            _ => Self::NONE,
        }
    }
}

impl From<RecursionKey> for Option<RecursionId> {
    #[inline]
    fn from(key: RecursionKey) -> Self {
        let high = (key.0 >> 32) as u32;
        if key.0 as u32 != 0 {
            Some(RecursionId::Node(Node(key.0)))
        } else if high == 0 {
            None
        } else if high & RecursionKey::TYPE_BIT != 0 {
            Some(RecursionId::Type(TypeId(high & !RecursionKey::TYPE_BIT)))
        } else {
            Some(RecursionId::Symbol(SymbolId(high)))
        }
    }
}

impl FlatKey for RecursionKey {
    // The murmur3 finalizer: node keys differ in both halves and symbol and
    // type keys only in the high half, so both halves are mixed into the
    // low bits.
    #[inline]
    fn flat_hash(&self) -> u64 {
        let mut x = self.0;
        x ^= x >> 33;
        x = x.wrapping_mul(0xff51_afd7_ed55_8ccd);
        x ^ (x >> 33)
    }
}

/// `StackEntry::prev` of an entry with no earlier same-id entry.
const NO_PREV: u32 = u32::MAX;

/// PORT: perf. Not in Go. Per entry of an `InferenceStack`: `id` is
/// `RecursionKey::new(stack_recursion_id(t))`, `prev` is the index of the
/// next lower entry with the same id (or `NO_PREV`), and `steps` counts the
/// same-id entry pairs up to this entry whose type id does not go down.
// PERF: the three fields share one 16-byte element, so a push reads the id
// and the steps of `prev` from one cache line.
#[derive(Clone, Copy, Debug)]
struct StackEntry {
    id: RecursionKey,
    prev: u32,
    steps: u32,
}

/// The id list that `is_deeply_nested_type_with_ids` reads.
impl From<StackEntry> for Option<RecursionId> {
    #[inline]
    fn from(entry: StackEntry) -> Self {
        entry.id.into()
    }
}

/// PORT: perf. Not in Go. Go `sourceStack` or `targetStack` of
/// `InferenceState`, with the recursion id of each entry and the data that
/// gives `is_deeply_nested_type` for the top entry in O(1).
///
/// The Go scan counts the first entry with the probe's id, then each later
/// entry with that id whose type id is not lower than the one before it.
/// The count only grows. When the probe is the top entry, `1 + steps` of the
/// top entry is that count, and each push finds the entry before it through
/// `top`. `top[id]` is the index of the topmost entry with `id` whenever
/// `id` is on the stack. An index left from a popped entry fails the
/// `prev < idx && entries[prev].id == id` test, so keys of `top` are never
/// removed.
#[derive(Default)]
pub struct InferenceStack {
    /// The entry types, in their own list for the scan.
    pub types: Vec<TypeId>,
    entries: Vec<StackEntry>,
    top: FlatMap<RecursionKey, u32>,
    /// Entries whose id is `RecursionKey::NONE`.
    none: u32,
}

impl InferenceStack {
    // PERF: `inline(always)`, because `#[inline]` left push and pop out of
    // line.
    #[inline(always)]
    pub fn push(&mut self, t: TypeId, id: RecursionKey) {
        let idx = self.types.len() as u32;
        let mut entry = StackEntry {
            id,
            prev: NO_PREV,
            steps: 0,
        };
        if id == RecursionKey::NONE {
            self.none += 1;
        } else if let Some(prev) = self.top.insert(id, idx)
            && prev < idx
        {
            let prev_entry = self.entries[prev as usize];
            if prev_entry.id == id {
                entry.prev = prev;
                entry.steps = prev_entry.steps + u32::from(t >= self.types[prev as usize]);
            }
        }
        self.types.push(t);
        self.entries.push(entry);
    }

    #[inline(always)]
    pub fn pop(&mut self) {
        self.types.pop();
        let entry = self.entries.pop().unwrap();
        if entry.id == RecursionKey::NONE {
            self.none -= 1;
        } else if entry.prev != NO_PREV {
            self.top.insert(entry.id, entry.prev);
        }
    }

    /// The Go deeply nested answer for the top entry as the probe, when it
    /// needs no scan: the top has an id and no entry lacks one.
    #[inline]
    #[must_use]
    pub fn top_deeply_nested(&self, max_depth: i32) -> Option<bool> {
        let top = self.entries.last()?;
        if top.id == RecursionKey::NONE || self.none != 0 {
            return None;
        }
        let steps = top.steps;
        let max_depth = i64::from(max_depth);
        Some(self.types.len() as i64 >= max_depth && 1 + i64::from(steps) >= max_depth)
    }

    /// Empties the stack and keeps the allocations. `top` keeps its stale
    /// indexes (they fail the validity test), but a large `top` is dropped
    /// so ids of earlier root inferences do not fill it.
    pub fn clear(&mut self) {
        self.types.clear();
        self.entries.clear();
        self.none = 0;
        if self.top.len() > 1024 {
            self.top = FlatMap::default();
        }
    }
}

// Go: checker/inference.go:16 InferenceState
// PORT: Go `inferences []*InferenceInfo` is the inference list of an inference
// context, so it is the `InferenceContextId` that owns the list (nil when the
// state is in the free pool). Callers that pass a fresh list in Go push a
// scratch context first. Go nil `visited` map is `None`.
#[derive(Default)]
pub struct InferenceState {
    pub inferences: InferenceContextId,
    pub original_source: TypeId,
    pub original_target: TypeId,
    pub priority: InferencePriority,
    pub inference_priority: InferencePriority,
    pub contravariant: bool,
    pub bivariant: bool,
    pub expanding_flags: ExpandingFlags,
    pub propagation_type: TypeId,
    pub visited: Option<InferenceVisitedMap>,
    // PORT: perf. Not in Go. The largest `visited` length that a root
    // inference on this pooled state reached (see `invoke_once`).
    pub max_visited_len: usize,
    // PORT: perf. Go `sourceStack` and `targetStack` with the recursion id
    // of each entry (see `InferenceStack`).
    pub source_stack: InferenceStack,
    pub target_stack: InferenceStack,
    pub next: Option<Rc<RefCell<InferenceState>>>,
}

impl Checker {
    // Go: checker/inference.go:32 getInferenceState
    pub fn get_inference_state(&mut self) -> Rc<RefCell<InferenceState>> {
        match self.freeinference_state.take() {
            // PERF: the pool link moves to the pool head, without a clone.
            Some(n) => {
                self.freeinference_state = n.borrow_mut().next.take();
                n
            }
            None => Rc::new(RefCell::new(InferenceState::default())),
        }
    }

    // Go: checker/inference.go:41 putInferenceState
    // PERF: the fields are reset in place, so the buffers stay where they
    // are and no old value is dropped. The pattern lists every field, so a
    // new field must be reset here too.
    pub fn put_inference_state(&mut self, n: Rc<RefCell<InferenceState>>) {
        {
            let mut guard = n.borrow_mut();
            let InferenceState {
                inferences,
                original_source,
                original_target,
                priority,
                inference_priority,
                contravariant,
                bivariant,
                expanding_flags,
                propagation_type,
                visited,
                max_visited_len,
                source_stack,
                target_stack,
                next,
            } = &mut *guard;
            *max_visited_len =
                (*max_visited_len).max(visited.as_ref().map_or(0, InferenceVisitedMap::len));
            if let Some(v) = visited.as_mut() {
                // PORT: Go `clear(n.visited)`. Clearing costs time in the map
                // capacity, so a mostly empty large map is dropped instead.
                if v.capacity() > 256 && v.len() < v.capacity() / 8 {
                    *v = InferenceVisitedMap::default();
                } else {
                    v.clear();
                }
            }
            source_stack.clear();
            target_stack.clear();
            // PORT: Go `inferences: n.inferences[:0]` keeps only the slice
            // capacity; the context id has none, so it becomes nil.
            *inferences = InferenceContextId::NIL;
            *original_source = TypeId::default();
            *original_target = TypeId::default();
            *priority = InferencePriority::default();
            *inference_priority = InferencePriority::default();
            *contravariant = false;
            *bivariant = false;
            *expanding_flags = ExpandingFlags::default();
            *propagation_type = TypeId::default();
            *next = self.freeinference_state.take();
        }
        self.freeinference_state = Some(n);
    }

    // Go: checker/inference.go:53 inferTypes
    pub fn infer_types(
        &mut self,
        inferences: InferenceContextId,
        original_source: TypeId,
        original_target: TypeId,
        priority: InferencePriority,
        contravariant: bool,
    ) {
        let n = self.get_inference_state();
        {
            let mut state = n.borrow_mut();
            let s = &mut *state;
            s.inferences = inferences;
            s.original_source = original_source;
            s.original_target = original_target;
            s.priority = priority;
            s.inference_priority = InferencePriority::MAX_VALUE;
            s.contravariant = contravariant;
            self.infer_from_types(s, original_source, original_target);
        }
        self.put_inference_state(n);
    }

    // Go: checker/inference.go:65 inferFromTypes
    pub fn infer_from_types(&mut self, n: &mut InferenceState, source: TypeId, target: TypeId) {
        let mut source = source;
        let mut target = target;
        if !self.could_contain_type_variables(target) || self.is_no_infer_type(target) {
            return;
        }
        if source == self.wildcard_type || source == self.blocked_string_type {
            // We are inferring from an 'any' type. We want to infer this type for every type parameter
            // referenced in the target type, so we record it as the propagation type and infer from the
            // target to itself. Then, as we find candidates we substitute the propagation type.
            let save_propagation_type = n.propagation_type;
            n.propagation_type = source;
            self.infer_from_types(n, target, target);
            n.propagation_type = save_propagation_type;
            return;
        }
        // PORT: the aliases are compared by reference and cloned only when
        // they name the same symbol.
        let same_alias = matches!(
            (&self.ty(source).alias, &self.ty(target).alias),
            (Some(sa), Some(ta)) if sa.symbol == ta.symbol
        );
        if same_alias {
            let (sa_empty, ta_empty) = (
                self.ty(source)
                    .alias
                    .as_ref()
                    .unwrap()
                    .type_arguments
                    .is_empty(),
                self.ty(target)
                    .alias
                    .as_ref()
                    .unwrap()
                    .type_arguments
                    .is_empty(),
            );
            if !sa_empty || !ta_empty {
                // ts#64553 (Go N' inference.go:84)
                self.invoke_once(n, source, target, Checker::infer_from_alias_type_arguments);
            }
            // And if there weren't any type arguments, there's no reason to run inference as the types must be the same.
            return;
        }
        if source == target
            && self
                .ty(source)
                .flags
                .intersects(TypeFlags::UNION_OR_INTERSECTION)
        {
            // When source and target are the same union or intersection type, just relate each constituent
            // type to itself.
            for i in 0..self.ty(source).types().len() {
                let t = self.type_at(source, i);
                self.infer_from_types(n, t, t);
            }
            return;
        }
        if self.ty(target).flags.intersects(TypeFlags::UNION) {
            // PORT: a single source is read from a stack array, not a new list.
            let (source_list, single);
            let source_types: &[TypeId] = if self.ty(source).flags.intersects(TypeFlags::UNION) {
                source_list = self.ty(source).types_list();
                &source_list
            } else {
                single = [source];
                &single
            };
            // First, infer between identically matching source and target constituents and remove the
            // matching types.
            // PORT: perf. The shared list copies no elements.
            let target_types = self.ty(target).types_list();
            let (temp_sources, temp_targets) = self.infer_from_matching_types(
                n,
                source_types,
                &target_types,
                &mut |c: &mut Checker, s: TypeId, t: TypeId| c.is_type_or_base_identical_to(s, t),
                false, /*sort*/
            );
            // Next, infer between closely matching source and target constituents and remove
            // the matching types. Types closely match when they are instantiations of the same
            // object type or instantiations of the same type alias.
            let (sources, targets) = self.infer_from_matching_types(
                n,
                &temp_sources,
                &temp_targets,
                &mut |c: &mut Checker, s: TypeId, t: TypeId| c.is_type_closely_matched_by(s, t),
                true, /*sort*/
            );
            if targets.is_empty() {
                return;
            }
            target = self.get_union_type(&targets);
            if sources.is_empty() {
                // All source constituents have been matched and there is nothing further to infer from.
                // However, simply making no inferences is undesirable because it could ultimately mean
                // inferring a type parameter constraint. Instead, make a lower priority inference from
                // the full source to whatever remains in the target. For example, when inferring from
                // string to 'string | T', make a lower priority inference of string for T.
                self.infer_with_priority(n, source, target, InferencePriority::NAKED_TYPE_VARIABLE);
                return;
            }
            source = self.get_union_type(&sources);
        } else if self.ty(target).flags.intersects(TypeFlags::INTERSECTION)
            && !(0..self.ty(target).types().len())
                .all(|i| self.is_non_generic_object_type(self.type_at(target, i)))
        {
            // We reduce intersection types unless they're simple combinations of object types. For example,
            // when inferring from 'string[] & { extra: any }' to 'string[] & T' we want to remove string[] and
            // infer { extra: any } for T. But when inferring to 'string[] & Iterable<T>' we want to keep the
            // string[] on the source side and infer string for T.
            if !self.ty(source).flags.intersects(TypeFlags::UNION) {
                let (source_list, single);
                let source_types: &[TypeId] =
                    if self.ty(source).flags.intersects(TypeFlags::INTERSECTION) {
                        source_list = self.ty(source).types_list();
                        &source_list
                    } else {
                        single = [source];
                        &single
                    };
                // Infer between identically matching source and target constituents and remove the matching types.
                let target_types = self.ty(target).types_list();
                let (sources, targets) = self.infer_from_matching_types(
                    n,
                    source_types,
                    &target_types,
                    &mut |c: &mut Checker, s: TypeId, t: TypeId| c.is_type_identical_to(s, t),
                    false, /*sort*/
                );
                if sources.is_empty() || targets.is_empty() {
                    return;
                }
                source = self.get_intersection_type(&sources);
                target = self.get_intersection_type(&targets);
            }
        }
        if self
            .ty(target)
            .flags
            .intersects(TypeFlags::INDEXED_ACCESS | TypeFlags::SUBSTITUTION)
        {
            if self.is_no_infer_type(target) {
                return;
            }
            target = self.get_actual_type_variable(target);
        }
        if self.ty(target).flags.intersects(TypeFlags::TYPE_VARIABLE) {
            // Skip inference if the source is "blocked", which is used by the language service to
            // prevent inference on nodes currently being edited.
            if self.is_from_inference_blocked_source(source) {
                return;
            }
            let inference = self.get_inference_info_for_type(n, target);
            if let Some(inference) = inference {
                // If target is a type parameter, make an inference, unless the source type contains
                // a "non-inferrable" type. Types with this flag set are markers used to prevent inference.
                //
                // For example:
                //     - anyFunctionType is a wildcard type that's used to avoid contextually typing functions;
                //       it's internal, so should not be exposed to the user by adding it as a candidate.
                //     - autoType (and autoArrayType) is a special "any" used in control flow; like anyFunctionType,
                //       it's internal and should not be observable.
                //     - silentNeverType is returned by getInferredType when instantiating a generic function for
                //       inference (and a type variable has no mapping).
                //
                // This flag is infectious; if we produce Box<never> (where never is silentNeverType), Box<never> is
                // also non-inferrable.
                //
                // As a special case, also ignore nonInferrableAnyType, which is a special form of the any type
                // used as a stand-in for binding elements when they are being inferred.
                if self
                    .ty(source)
                    .object_flags
                    .intersects(ObjectFlags::NON_INFERRABLE_TYPE)
                    || source == self.non_inferrable_any_type
                {
                    return;
                }
                let ctx = n.inferences;
                if !self.inference_context(ctx).inferences[inference].is_fixed {
                    let propagation_type = n.propagation_type;
                    let candidate = if propagation_type.is_some() {
                        propagation_type
                    } else {
                        source
                    };
                    if candidate == self.blocked_string_type {
                        return;
                    }
                    let (priority, contravariant, bivariant) = {
                        let s = &*n;
                        (s.priority, s.contravariant, s.bivariant)
                    };
                    {
                        let info = &mut self.inference_context_mut(ctx).inferences[inference];
                        if priority < info.priority {
                            info.candidate_lists = None;
                            info.top_level = true;
                            info.priority = priority;
                        }
                    }
                    if priority == self.inference_context(ctx).inferences[inference].priority {
                        // We make contravariant inferences only if we are in a pure contravariant position,
                        // i.e. only if we have not descended into a bivariant position.
                        if contravariant && !bivariant {
                            if !self.inference_context(ctx).inferences[inference]
                                .contra_candidates()
                                .contains(&candidate)
                            {
                                self.inference_context_mut(ctx).inferences[inference]
                                    .candidate_lists_mut()
                                    .contra_candidates
                                    .push(candidate);
                                self.clear_cached_inferences(ctx);
                            }
                        } else if !self.inference_context(ctx).inferences[inference]
                            .candidates()
                            .contains(&candidate)
                        {
                            self.inference_context_mut(ctx).inferences[inference]
                                .candidate_lists_mut()
                                .candidates
                                .push(candidate);
                            self.clear_cached_inferences(ctx);
                        }
                    }
                    if !priority.intersects(InferencePriority::RETURN_TYPE)
                        && self.ty(target).flags.intersects(TypeFlags::TYPE_PARAMETER)
                        && self.inference_context(ctx).inferences[inference].top_level
                    {
                        let original_target = n.original_target;
                        if !self.is_type_parameter_at_top_level(original_target, target, 0) {
                            self.inference_context_mut(ctx).inferences[inference].top_level = false;
                            self.clear_cached_inferences(ctx);
                        }
                    }
                }
                {
                    let s = &mut *n;
                    s.inference_priority = std::cmp::min(s.inference_priority, s.priority);
                }
                return;
            }
            // Infer to the simplified version of an indexed access, if possible, to (hopefully) expose more bare type parameters to the inference engine
            let simplified = self.get_simplified_type(target, false /*writing*/);
            if simplified != target {
                self.infer_from_types(n, source, simplified);
            } else if self.ty(target).flags.intersects(TypeFlags::INDEXED_ACCESS) {
                let target_index_type = self.ty(target).as_indexed_access_type().index_type;
                let index_type =
                    self.get_simplified_type(target_index_type, false /*writing*/);
                // Generally simplifications of instantiable indexes are avoided to keep relationship checking correct, however if our target is an access, we can consider
                // that key of that access to be "instantiated", since we're looking to find the infernce goal in any way we can.
                if self
                    .ty(index_type)
                    .flags
                    .intersects(TypeFlags::INSTANTIABLE)
                {
                    let target_object_type = self.ty(target).as_indexed_access_type().object_type;
                    let simplified_object_type =
                        self.get_simplified_type(target_object_type, false /*writing*/);
                    let simplified = self.distribute_index_over_object_type(
                        simplified_object_type,
                        index_type,
                        false, /*writing*/
                    );
                    if simplified.is_some() && simplified != target {
                        self.infer_from_types(n, source, simplified);
                    }
                }
            }
        }
        let source_flags = self.ty(source).flags;
        let target_flags = self.ty(target).flags;
        if self
            .ty(source)
            .object_flags
            .intersects(ObjectFlags::REFERENCE)
            && self
                .ty(target)
                .object_flags
                .intersects(ObjectFlags::REFERENCE)
            && (self.ty(source).as_type_reference().object.target
                == self.ty(target).as_type_reference().object.target
                || self.is_array_type(source) && self.is_array_type(target))
            && !(self.ty(source).as_type_reference().node.is_some()
                && self.ty(target).as_type_reference().node.is_some())
        {
            // If source and target are references to the same generic type, infer from type arguments
            // ts#64553 (Go N' inference.go:230)
            self.invoke_once(
                n,
                source,
                target,
                Checker::infer_from_reference_type_arguments,
            );
        } else if source_flags.intersects(TypeFlags::INDEX)
            && target_flags.intersects(TypeFlags::INDEX)
        {
            let s = self.ty(source).as_index_type().target;
            let t = self.ty(target).as_index_type().target;
            self.infer_from_contravariant_types(n, s, t);
        } else if (self.is_literal_type(source) || source_flags.intersects(TypeFlags::STRING))
            && target_flags.intersects(TypeFlags::INDEX)
        {
            let empty = self.create_empty_object_type_from_string_literal(source);
            let t = self.ty(target).as_index_type().target;
            self.infer_from_contravariant_types_with_priority(
                n,
                empty,
                t,
                InferencePriority::LITERAL_KEYOF,
            );
        } else if source_flags.intersects(TypeFlags::INDEXED_ACCESS)
            && target_flags.intersects(TypeFlags::INDEXED_ACCESS)
        {
            let (s_object, s_index) = {
                let a = self.ty(source).as_indexed_access_type();
                (a.object_type, a.index_type)
            };
            let (t_object, t_index) = {
                let a = self.ty(target).as_indexed_access_type();
                (a.object_type, a.index_type)
            };
            self.infer_from_types(n, s_object, t_object);
            self.infer_from_types(n, s_index, t_index);
        } else if source_flags.intersects(TypeFlags::STRING_MAPPING)
            && target_flags.intersects(TypeFlags::STRING_MAPPING)
        {
            if self.ty(source).symbol == self.ty(target).symbol {
                let s = self.ty(source).as_string_mapping_type().target;
                let t = self.ty(target).as_string_mapping_type().target;
                self.infer_from_types(n, s, t);
            }
        } else if source_flags.intersects(TypeFlags::SUBSTITUTION) {
            let base_type = self.ty(source).as_substitution_type().base_type;
            self.infer_from_types(n, base_type, target);
            // Make substitute inference at a lower priority
            let substitution_intersection = self.get_substitution_intersection(source);
            self.infer_with_priority(
                n,
                substitution_intersection,
                target,
                InferencePriority::SUBSTITUTE_SOURCE,
            );
        } else if target_flags.intersects(TypeFlags::CONDITIONAL) {
            self.invoke_once(n, source, target, Checker::infer_to_conditional_type);
        } else if target_flags.intersects(TypeFlags::UNION_OR_INTERSECTION) {
            let target_types = self.ty(target).types_list();
            self.infer_to_multiple_types(n, source, &target_types, target_flags);
        } else if source_flags.intersects(TypeFlags::UNION) {
            // Source is a union or intersection type, infer from each constituent type
            for i in 0..self.ty(source).types().len() {
                let source_type = self.type_at(source, i);
                self.infer_from_types(n, source_type, target);
            }
        } else if target_flags.intersects(TypeFlags::TEMPLATE_LITERAL) {
            let template_literal = self.ty(target).as_template_literal_type().clone();
            self.infer_to_template_literal_type(n, source, &template_literal);
        } else {
            source = self.get_reduced_type(source);
            if self.is_generic_mapped_type(source) && self.is_generic_mapped_type(target) {
                self.invoke_once(n, source, target, Checker::infer_from_generic_mapped_types);
            }
            let priority = n.priority;
            if !(priority.intersects(InferencePriority::NO_CONSTRAINTS)
                && self
                    .ty(source)
                    .flags
                    .intersects(TypeFlags::INTERSECTION | TypeFlags::INSTANTIABLE))
            {
                let apparent_source = self.get_apparent_type(source);
                // getApparentType can return _any_ type, since an indexed access or conditional may simplify to any other type.
                // If that occurs and it doesn't simplify to an object or intersection, we'll need to restart `inferFromTypes`
                // with the simplified source.
                if apparent_source != source
                    && !self
                        .ty(apparent_source)
                        .flags
                        .intersects(TypeFlags::OBJECT | TypeFlags::INTERSECTION)
                {
                    self.infer_from_types(n, apparent_source, target);
                    return;
                }
                source = apparent_source;
            }
            if self
                .ty(source)
                .flags
                .intersects(TypeFlags::OBJECT | TypeFlags::INTERSECTION)
            {
                self.invoke_once(n, source, target, Checker::infer_from_object_types);
            }
        }
    }

    // Go: checker/inference.go:280 inferFromAliasTypeArguments (Go N', ts#64553)
    pub fn infer_from_alias_type_arguments(
        &mut self,
        n: &mut InferenceState,
        source: TypeId,
        target: TypeId,
    ) {
        let sa = self.ty(source).alias.clone().unwrap();
        let ta = self.ty(target).alias.clone().unwrap();
        // Source and target are types originating in the same generic type alias declaration.
        // Simply infer from source type arguments to target type arguments, with defaults applied.
        let params_len = self.type_alias_links.get(sa.symbol).type_parameters.len();
        let node_is_in_js_file = is_in_js_file(self.sym(sa.symbol).value_declaration);
        // PERF: when no argument is missing and the alias is not in a
        // JS file, Go fillMissingTypeArguments returns its input and
        // getMinTypeArgumentCount only reads declarations, so the
        // argument lists are read in place, without the copies.
        if params_len != 0
            && !node_is_in_js_file
            && sa.type_arguments.len() >= params_len
            && ta.type_arguments.len() >= params_len
        {
            let variances = self.get_alias_variances(sa.symbol);
            self.infer_from_type_arguments(n, &sa.type_arguments, &ta.type_arguments, &variances);
            return;
        }
        let params = self.type_alias_links.get(sa.symbol).type_parameters.clone();
        let min_params = self.get_min_type_argument_count(&params);
        let source_types = self.fill_missing_type_arguments(
            &sa.type_arguments,
            &params,
            min_params,
            node_is_in_js_file,
        );
        let target_types = self.fill_missing_type_arguments(
            &ta.type_arguments,
            &params,
            min_params,
            node_is_in_js_file,
        );
        let variances = self.get_alias_variances(sa.symbol);
        self.infer_from_type_arguments(n, &source_types, &target_types, &variances);
    }

    // Go: checker/inference.go:291 inferFromReferenceTypeArguments (Go N', ts#64553)
    pub fn infer_from_reference_type_arguments(
        &mut self,
        n: &mut InferenceState,
        source: TypeId,
        target: TypeId,
    ) {
        let source_type_arguments = self.get_type_arguments(source);
        let target_type_arguments = self.get_type_arguments(target);
        let source_target = self.ty(source).as_type_reference().object.target;
        let variances = self.get_variances(source_target);
        self.infer_from_type_arguments(
            n,
            &source_type_arguments,
            &target_type_arguments,
            &variances,
        );
    }

    // Go: checker/inference.go:284 inferFromTypeArguments
    pub fn infer_from_type_arguments(
        &mut self,
        n: &mut InferenceState,
        source_types: &[TypeId],
        target_types: &[TypeId],
        variances: &[VarianceFlags],
    ) {
        for i in 0..std::cmp::min(source_types.len(), target_types.len()) {
            if i < variances.len()
                && variances[i] & VarianceFlags::VARIANCE_MASK == VarianceFlags::CONTRAVARIANT
            {
                self.infer_from_contravariant_types(n, source_types[i], target_types[i]);
            } else {
                self.infer_from_types(n, source_types[i], target_types[i]);
            }
        }
    }

    // Go: checker/inference.go:294 inferWithPriority
    pub fn infer_with_priority(
        &mut self,
        n: &mut InferenceState,
        source: TypeId,
        target: TypeId,
        new_priority: InferencePriority,
    ) {
        let save_priority = n.priority;
        n.priority |= new_priority;
        self.infer_from_types(n, source, target);
        n.priority = save_priority;
    }

    // Go: checker/inference.go:301 inferFromContravariantTypesWithPriority
    pub fn infer_from_contravariant_types_with_priority(
        &mut self,
        n: &mut InferenceState,
        source: TypeId,
        target: TypeId,
        new_priority: InferencePriority,
    ) {
        let save_priority = n.priority;
        n.priority |= new_priority;
        self.infer_from_contravariant_types(n, source, target);
        n.priority = save_priority;
    }

    // Go: checker/inference.go:308 inferFromContravariantTypes
    pub fn infer_from_contravariant_types(
        &mut self,
        n: &mut InferenceState,
        source: TypeId,
        target: TypeId,
    ) {
        {
            let s = &mut *n;
            s.contravariant = !s.contravariant;
        }
        self.infer_from_types(n, source, target);
        {
            let s = &mut *n;
            s.contravariant = !s.contravariant;
        }
    }

    // Go: checker/inference.go:314 inferFromContravariantTypesIfStrictFunctionTypes
    pub fn infer_from_contravariant_types_if_strict_function_types(
        &mut self,
        n: &mut InferenceState,
        source: TypeId,
        target: TypeId,
    ) {
        let priority = n.priority;
        if self.strict_function_types || priority.intersects(InferencePriority::ALWAYS_STRICT) {
            self.infer_from_contravariant_types(n, source, target);
        } else {
            self.infer_from_types(n, source, target);
        }
    }

    // Ensure an inference action is performed only once for the given source and target types.
    // This includes two things:
    // Avoiding inferring between the same pair of source and target types,
    // and avoiding circularly inferring between source and target types.
    // For an example of the last, consider if we are inferring between source type
    // `type Deep<T> = { next: Deep<Deep<T>> }` and target type `type Loop<U> = { next: Loop<U> }`.
    // We would then infer between the types of the `next` property: `Deep<Deep<T>>` = `{ next: Deep<Deep<Deep<T>>> }` and `Loop<U>` = `{ next: Loop<U> }`.
    // We will then infer again between the types of the `next` property:
    // `Deep<Deep<Deep<T>>>` and `Loop<U>`, and so on, such that we would be forever inferring
    // between instantiations of the same types `Deep` and `Loop`.
    // In particular, we would be inferring from increasingly deep instantiations of `Deep` to `Loop`,
    // such that we would go on inferring forever, even though we would never infer
    // between the same pair of types.
    // Go: checker/inference.go:335 invokeOnce
    pub fn invoke_once(
        &mut self,
        n: &mut InferenceState,
        source: TypeId,
        target: TypeId,
        action: fn(&mut Checker, &mut InferenceState, TypeId, TypeId),
    ) {
        // PORT: a type handle equals its Go `id`, so the key needs no type read.
        let key = InferenceKey {
            source,
            target,
            priority: n.priority,
            contravariant: n.contravariant,
            bivariant: n.bivariant,
        };
        // PORT: perf. The map is made first; a lookup on Go's nil map misses,
        // and the miss path makes the map anyway, so the map contents are the
        // same. The insert after a miss finds the probed slots in cache.
        {
            let s = &mut *n;
            let visited = s.visited.get_or_insert_with(InferenceVisitedMap::default);
            if let Some(&p) = visited.get(&key) {
                s.inference_priority = std::cmp::min(s.inference_priority, p);
                return;
            }
            // PORT: perf. Not in Go. `put_inference_state` drops a mostly
            // empty large map, so a later large inference would grow it again
            // by doubling. Once this inference reaches a quarter of the
            // largest length seen, the map grows in one step to that length.
            // A map sized so is not dropped at that fill, and smaller
            // inferences never make it. The map is never iterated.
            if visited.len() == s.max_visited_len / 4 {
                visited.reserve(s.max_visited_len - visited.len());
            }
            visited.insert(key, InferencePriority::CIRCULARITY);
        }
        let save_inference_priority;
        let save_expanding_flags;
        // PORT: perf. Each stack entry keeps its recursion id, so the deeply
        // nested checks below compare ids instead of reading every entry's
        // type again (see `is_deeply_nested_type_with_ids`).
        let source_id = RecursionKey::new(self.stack_recursion_id(source));
        let target_id = RecursionKey::new(self.stack_recursion_id(target));
        {
            let s = &mut *n;
            save_inference_priority = s.inference_priority;
            s.inference_priority = InferencePriority::MAX_VALUE;
            // We stop inferring and report a circularity if we encounter duplicate recursion identities on both
            // the source side and the target side.
            save_expanding_flags = s.expanding_flags;
            s.source_stack.push(source, source_id);
            s.target_stack.push(target, target_id);
        }
        // PORT: the stacks are read in place through `n`, instead of cloned.
        let source_nested = {
            let s = &*n;
            self.is_deeply_nested_inference_top(&s.source_stack, 2)
        };
        if source_nested {
            n.expanding_flags |= ExpandingFlags::SOURCE;
        }
        let target_nested = {
            let s = &*n;
            self.is_deeply_nested_inference_top(&s.target_stack, 2)
        };
        if target_nested {
            n.expanding_flags |= ExpandingFlags::TARGET;
        }
        let expanding_flags = n.expanding_flags;
        if expanding_flags != ExpandingFlags::BOTH {
            action(self, n, source, target);
        } else {
            n.inference_priority = InferencePriority::CIRCULARITY;
        }
        {
            let s = &mut *n;
            s.target_stack.pop();
            s.source_stack.pop();
            s.expanding_flags = save_expanding_flags;
            let inference_priority = s.inference_priority;
            // PORT: the key was inserted above, so this overwrites it in
            // place (one probe).
            s.visited
                .get_or_insert_with(InferenceVisitedMap::default)
                .insert(key, inference_priority);
            s.inference_priority = std::cmp::min(s.inference_priority, save_inference_priority);
        }
    }

    // PORT: perf. Go `isDeeplyNestedType(t, stack, maxDepth)` where `t` is
    // the entry just pushed on `stack`. When the top entry has an id and no
    // entry lacks one, the Go scan only compares ids, so the answer comes
    // from `InferenceStack` with no scan (debug builds also scan and
    // compare). Else the scan runs as in Go, with
    // `has_matching_recursion_identity` for the same types in the same order.
    // Go: checker/relater.go:766 isDeeplyNestedType
    #[inline]
    fn is_deeply_nested_inference_top(&mut self, stack: &InferenceStack, max_depth: i32) -> bool {
        let fast = stack.top_deeply_nested(max_depth);
        if let Some(nested) = fast
            && !cfg!(debug_assertions)
        {
            return nested;
        }
        let t = *stack.types.last().unwrap();
        let probe_id: Option<RecursionId> = (*stack.entries.last().unwrap()).into();
        let nested = self.is_deeply_nested_type_with_ids(
            t,
            probe_id,
            &stack.types,
            &stack.entries,
            max_depth,
        );
        debug_assert!(fast.is_none_or(|fast| fast == nested));
        nested
    }

    // Go: checker/inference.go:370 inferFromMatchingTypes
    pub fn infer_from_matching_types(
        &mut self,
        n: &mut InferenceState,
        sources: &[TypeId],
        targets: &[TypeId],
        matches: &mut dyn FnMut(&mut Checker, TypeId, TypeId) -> bool,
        sort: bool,
    ) -> (SmallVec<[TypeId; 8]>, SmallVec<[TypeId; 8]>) {
        // PORT: perf. The matched and returned lists are short temporaries,
        // so they live on the stack up to 8 entries.
        let mut matched_sources: SmallVec<[TypeId; 8]> = SmallVec::new();
        let mut matched_targets: SmallVec<[TypeId; 8]> = SmallVec::new();
        for &t in targets {
            for &s in sources {
                if matches(self, s, t) {
                    if !sort {
                        self.infer_from_types(n, s, t);
                    }
                    if !matched_sources.contains(&s) {
                        matched_sources.push(s);
                    }
                    if !matched_targets.contains(&t) {
                        matched_targets.push(t);
                    }
                }
            }
        }
        if sort {
            // Sort target types by decreasing depth of generic instantiations. Intuitively, a successful
            // inference from a type argument with deeper nesting is of higher quality because we've stripped
            // away more layers of type instantiations that otherwise might skew the results. For example,
            // when inferring from string[] | string[][] to T[] | T[][], the inference of string we make from
            // relating string[][] to T[][] is of higher quality than the inference of string[] we make relating
            // string[][] to T[].
            // PORT: Go `slices.SortFunc` is `gostd::slices::sort_func` (Go
            // pdqsort), so the comparisons, and the type arguments that
            // `get_type_depth` resolves, come in Go's order.
            crate::gostd::slices::sort_func(
                &mut matched_targets[..],
                |&t1: &TypeId, &t2: &TypeId| self.compare_types_and_depth(t1, t2),
            );
            for &t in &matched_targets {
                for &s in &matched_sources {
                    if matches(self, s, t) {
                        self.infer_from_types(n, s, t);
                    }
                }
            }
        }
        // Copies the list once, without the matched types.
        let unmatched = |list: &[TypeId], matched: &[TypeId]| -> SmallVec<[TypeId; 8]> {
            if matched.is_empty() {
                return SmallVec::from_slice(list);
            }
            list.iter()
                .copied()
                .filter(|t| !matched.contains(t))
                .collect()
        };
        (
            unmatched(sources, &matched_sources),
            unmatched(targets, &matched_targets),
        )
    }

    // Compare two types first by depth and then by the regular type ordering.
    // Go: checker/inference.go:410 compareTypesAndDepth
    // PORT: Go package function; a checker method here because
    // `get_type_depth` resolves type arguments.
    pub fn compare_types_and_depth(&mut self, t1: TypeId, t2: TypeId) -> i32 {
        let d1 = self.get_type_depth(t1, 3);
        let d2 = self.get_type_depth(t2, 3);
        if d1 != d2 {
            return d2 - d1; // Largest depth sorts first
        }
        self.compare_types(t1, t2)
    }

    // Return the depth of the given type up to the given maximum depth. For generic aliased types
    // and type references, the depth is one plus the largest type argument depth. For union and
    // intersection types, the depth is the largest constituent type depth. For all other types,
    // the depth is zero. The maximum depth limits infinite recursion of circular types.
    // Go: checker/inference.go:423 getTypeDepth
    pub fn get_type_depth(&mut self, t: TypeId, max_depth: i32) -> i32 {
        if max_depth != 0 {
            if let Some(alias) = self
                .ty(t)
                .alias
                .clone()
                .filter(|alias| !alias.type_arguments.is_empty())
            {
                return self.get_type_list_depth(&alias.type_arguments, max_depth - 1) + 1;
            }
            if self.ty(t).object_flags.intersects(ObjectFlags::REFERENCE) {
                let type_arguments = self.get_type_arguments(t);
                if !type_arguments.is_empty() {
                    return self.get_type_list_depth(&type_arguments, max_depth - 1) + 1;
                }
            }
            if self
                .ty(t)
                .flags
                .intersects(TypeFlags::UNION_OR_INTERSECTION)
            {
                let types = self.ty(t).types_list();
                return self.get_type_list_depth(&types, max_depth);
            }
        }
        0
    }

    // Go: checker/inference.go:440 getTypeListDepth
    pub fn get_type_list_depth(&mut self, types: &[TypeId], max_depth: i32) -> i32 {
        let mut depth = 0;
        for &t in types {
            depth = std::cmp::max(depth, self.get_type_depth(t, max_depth));
        }
        depth
    }

    // Go: checker/inference.go:448 inferToMultipleTypes
    pub fn infer_to_multiple_types(
        &mut self,
        n: &mut InferenceState,
        source: TypeId,
        targets: &[TypeId],
        target_flags: TypeFlags,
    ) {
        let mut type_variable_count = 0;
        if target_flags.intersects(TypeFlags::UNION) {
            let mut naked_type_variable = TypeId::NIL;
            let (source_list, single);
            let sources: &[TypeId] = if self.ty(source).flags.intersects(TypeFlags::UNION) {
                source_list = self.ty(source).types_list();
                &source_list
            } else {
                single = [source];
                &single
            };
            // PERF: the flags and the unmatched list below are temporaries,
            // so they live on the stack up to 16 and 8 entries.
            let mut matched: SmallVec<[bool; 16]> = smallvec::smallvec![false; sources.len()];
            let mut inference_circularity = false;
            // First infer to types that are not naked type variables. For each source type we
            // track whether inferences were made from that particular type to some target with
            // equal priority (i.e. of equal quality) to what we would infer for a naked type
            // parameter.
            for &t in targets {
                if self.get_inference_info_for_type(n, t).is_some() {
                    naked_type_variable = t;
                    type_variable_count += 1;
                } else {
                    for i in 0..sources.len() {
                        let save_inference_priority = n.inference_priority;
                        n.inference_priority = InferencePriority::MAX_VALUE;
                        self.infer_from_types(n, sources[i], t);
                        let s = &mut *n;
                        if s.inference_priority == s.priority {
                            matched[i] = true;
                        }
                        inference_circularity = inference_circularity
                            || s.inference_priority == InferencePriority::CIRCULARITY;
                        s.inference_priority =
                            std::cmp::min(s.inference_priority, save_inference_priority);
                    }
                }
            }
            if type_variable_count == 0 {
                // If every target is an intersection of types containing a single naked type variable,
                // make a lower priority inference to that type variable. This handles inferring from
                // 'A | B' to 'T & (X | Y)' where we want to infer 'A | B' for T.
                let intersection_type_variable =
                    self.get_single_type_variable_from_intersection_types(n, targets);
                if intersection_type_variable.is_some() {
                    self.infer_with_priority(
                        n,
                        source,
                        intersection_type_variable,
                        InferencePriority::NAKED_TYPE_VARIABLE,
                    );
                }
                return;
            }
            // If the target has a single naked type variable and no inference circularities were
            // encountered above (meaning we explored the types fully), create a union of the source
            // types from which no inferences have been made so far and infer from that union to the
            // naked type variable.
            if type_variable_count == 1 && !inference_circularity {
                let mut unmatched: SmallVec<[TypeId; 8]> = SmallVec::new();
                for (i, &s) in sources.iter().enumerate() {
                    if !matched[i] {
                        unmatched.push(s);
                    }
                }
                if !unmatched.is_empty() {
                    let union = self.get_union_type(&unmatched);
                    self.infer_from_types(n, union, naked_type_variable);
                    return;
                }
            }
        } else {
            // We infer from types that are not naked type variables first so that inferences we
            // make from nested naked type variables and given slightly higher priority by virtue
            // of being first in the candidates array.
            for &t in targets {
                if self.get_inference_info_for_type(n, t).is_some() {
                    type_variable_count += 1;
                } else {
                    self.infer_from_types(n, source, t);
                }
            }
        }
        // Inferences directly to naked type variables are given lower priority as they are
        // less specific. For example, when inferring from Promise<string> to T | Promise<T>,
        // we want to infer string for T, not Promise<string> | string. For intersection types
        // we only infer to single naked type variables.
        if target_flags.intersects(TypeFlags::INTERSECTION) && type_variable_count == 1
            || !target_flags.intersects(TypeFlags::INTERSECTION) && type_variable_count > 0
        {
            for &t in targets {
                if self.get_inference_info_for_type(n, t).is_some() {
                    self.infer_with_priority(n, source, t, InferencePriority::NAKED_TYPE_VARIABLE);
                }
            }
        }
    }

    // PORT: Go package function; it reads the inference list through the
    // checker, so it is a `Checker` method.
    // Go: checker/inference.go:532 getSingleTypeVariableFromIntersectionTypes
    pub fn get_single_type_variable_from_intersection_types(
        &mut self,
        n: &mut InferenceState,
        types: &[TypeId],
    ) -> TypeId {
        let mut type_variable = TypeId::NIL;
        for &t in types {
            if !self.ty(t).flags.intersects(TypeFlags::INTERSECTION) {
                return TypeId::NIL;
            }
            let v = {
                let state = &*n;
                self.ty(t)
                    .types()
                    .iter()
                    .copied()
                    .find(|&t| self.get_inference_info_for_type(state, t).is_some())
                    .unwrap_or(TypeId::NIL)
            };
            if v.is_nil() || type_variable.is_some() && v != type_variable {
                return TypeId::NIL;
            }
            type_variable = v;
        }
        type_variable
    }

    // Go: checker/inference.go:547 inferToMultipleTypesWithPriority
    pub fn infer_to_multiple_types_with_priority(
        &mut self,
        n: &mut InferenceState,
        source: TypeId,
        targets: &[TypeId],
        target_flags: TypeFlags,
        new_priority: InferencePriority,
    ) {
        let save_priority = n.priority;
        n.priority |= new_priority;
        self.infer_to_multiple_types(n, source, targets, target_flags);
        n.priority = save_priority;
    }

    // Go: checker/inference.go:554 inferToConditionalType
    pub fn infer_to_conditional_type(
        &mut self,
        n: &mut InferenceState,
        source: TypeId,
        target: TypeId,
    ) {
        if self.ty(source).flags.intersects(TypeFlags::CONDITIONAL) {
            let (s_check, s_extends) = {
                let c = self.ty(source).as_conditional_type();
                (c.check_type, c.extends_type)
            };
            let (t_check, t_extends) = {
                let c = self.ty(target).as_conditional_type();
                (c.check_type, c.extends_type)
            };
            let s_check = self.get_non_distributed_type_parameter(s_check);
            self.infer_from_types(n, s_check, t_check);
            let s_extends = self.get_non_distributed_type_parameter(s_extends);
            self.infer_from_types(n, s_extends, t_extends);
            let s_true = self.get_true_type_from_conditional_type(source);
            let s_true = self.get_non_distributed_type_parameter(s_true);
            let t_true = self.get_true_type_from_conditional_type(target);
            self.infer_from_types(n, s_true, t_true);
            let s_false = self.get_false_type_from_conditional_type(source);
            let s_false = self.get_non_distributed_type_parameter(s_false);
            let t_false = self.get_false_type_from_conditional_type(target);
            self.infer_from_types(n, s_false, t_false);
        } else {
            let target_types = [
                self.get_true_type_from_conditional_type(target),
                self.get_false_type_from_conditional_type(target),
            ];
            let priority = if n.contravariant {
                InferencePriority::CONTRAVARIANT_CONDITIONAL
            } else {
                InferencePriority::NONE
            };
            let target_flags = self.ty(target).flags;
            self.infer_to_multiple_types_with_priority(
                n,
                source,
                &target_types,
                target_flags,
                priority,
            );
        }
    }

    // Go: checker/inference.go:566 inferToTemplateLiteralType
    pub fn infer_to_template_literal_type(
        &mut self,
        n: &mut InferenceState,
        source: TypeId,
        target: &TemplateLiteralType,
    ) {
        let comparer = self.compare_types_assignable.clone();
        let matches = self.infer_types_from_template_literal_type(
            source,
            target,
            &mut |c: &mut Checker, s: TypeId, t: TypeId, r: bool| comparer(c, s, t, r),
        );
        let types = target.types.clone();
        // When the target template literal contains only placeholders (meaning that inference is intended to extract
        // single characters and remainder strings) and inference fails to produce matches, we want to infer 'never' for
        // each placeholder such that instantiation with the inferred value(s) produces 'never', a type for which an
        // assignment check will fail. If we make no inferences, we'll likely end up with the constraint 'string' which,
        // upon instantiation, would collapse all the placeholders to just 'string', and an assignment check might
        // succeed. That would be a pointless and confusing outcome.
        if !matches.is_empty() || target.texts.iter().all(|s| s.is_empty()) {
            'outer: for (i, &target) in types.iter().enumerate() {
                let source = if !matches.is_empty() {
                    matches[i]
                } else {
                    self.never_type
                };
                // If we are inferring from a string literal type to a type variable whose constraint includes one of the
                // allowed template literal placeholder types, infer from a literal type corresponding to the constraint.
                if self.ty(source).flags.intersects(TypeFlags::STRING_LITERAL)
                    && self.ty(target).flags.intersects(TypeFlags::TYPE_VARIABLE)
                {
                    let inference_context = self.get_inference_info_for_type(n, target);
                    if let Some(inference_context) = inference_context {
                        let ctx = n.inferences;
                        let type_parameter = self.inference_context(ctx).inferences
                            [inference_context]
                            .type_parameter;
                        let constraint = self.get_base_constraint_of_type(type_parameter);
                        if constraint.is_some() && !self.is_type_any(constraint) {
                            let mut all_type_flags = TypeFlags::NONE;
                            for t in self.ty(constraint).distributed() {
                                all_type_flags |= self.ty(t).flags;
                            }
                            // If the constraint contains `string`, we don't need to look for a more preferred type
                            if !all_type_flags.intersects(TypeFlags::STRING) {
                                let str = self.get_string_literal_value(source);
                                // If the type contains `number` or a number literal and the string isn't a valid number, exclude numbers
                                if all_type_flags.intersects(TypeFlags::NUMBER_LIKE)
                                    && !is_valid_number_string(&str, true /*roundTripOnly*/)
                                {
                                    all_type_flags = all_type_flags.without(TypeFlags::NUMBER_LIKE);
                                }
                                // If the type contains `bigint` or a bigint literal and the string isn't a valid bigint, exclude bigints
                                if all_type_flags.intersects(TypeFlags::BIG_INT_LIKE)
                                    && !is_valid_big_int_string(&str, true /*roundTripOnly*/)
                                {
                                    all_type_flags =
                                        all_type_flags.without(TypeFlags::BIG_INT_LIKE);
                                }
                                let choose = |c: &mut Checker,
                                              left: TypeId,
                                              right: TypeId|
                                 -> TypeId {
                                    let left_flags = c.ty(left).flags;
                                    let right_flags = c.ty(right).flags;
                                    if !right_flags.intersects(all_type_flags) {
                                        left
                                    } else if left_flags.intersects(TypeFlags::STRING) {
                                        left
                                    } else if right_flags.intersects(TypeFlags::STRING) {
                                        source
                                    } else if left_flags.intersects(TypeFlags::TEMPLATE_LITERAL) {
                                        left
                                    } else if right_flags.intersects(TypeFlags::TEMPLATE_LITERAL)
                                        // PERF (perffu1): Go's first test, before the clone.
                                        && !c.string_literal_misses_template_literal_ends(source, right)
                                        && {
                                            let right_template =
                                                c.ty(right).as_template_literal_type().clone();
                                            let cmp = c.compare_types_assignable.clone();
                                            c.is_type_matched_by_template_literal_type(
                                            source,
                                            &right_template,
                                            &mut |c: &mut Checker, s: TypeId, t: TypeId, r: bool| cmp(c, s, t, r),
                                        )
                                        }
                                    {
                                        source
                                    } else if left_flags.intersects(TypeFlags::STRING_MAPPING) {
                                        left
                                    } else if right_flags.intersects(TypeFlags::STRING_MAPPING)
                                        && str == c.apply_string_mapping(c.ty(right).symbol, &str)
                                    {
                                        source
                                    } else if left_flags.intersects(TypeFlags::STRING_LITERAL) {
                                        left
                                    } else if right_flags.intersects(TypeFlags::STRING_LITERAL)
                                        && c.get_string_literal_value_ref(right) == str
                                    {
                                        right
                                    } else if left_flags.intersects(TypeFlags::NUMBER) {
                                        left
                                    } else if right_flags.intersects(TypeFlags::NUMBER) {
                                        c.get_number_literal_type(crate::jsnum::from_string(&str))
                                    } else if left_flags.intersects(TypeFlags::ENUM) {
                                        left
                                    } else if right_flags.intersects(TypeFlags::ENUM) {
                                        c.get_number_literal_type(crate::jsnum::from_string(&str))
                                    } else if left_flags.intersects(TypeFlags::NUMBER_LITERAL) {
                                        left
                                    } else if right_flags.intersects(TypeFlags::NUMBER_LITERAL)
                                        && c.get_number_literal_value(right)
                                            == crate::jsnum::from_string(&str)
                                    {
                                        right
                                    } else if left_flags.intersects(TypeFlags::BIG_INT) {
                                        left
                                    } else if right_flags.intersects(TypeFlags::BIG_INT) {
                                        c.parse_big_int_literal_type(&str)
                                    } else if left_flags.intersects(TypeFlags::BIG_INT_LITERAL) {
                                        left
                                    } else if right_flags.intersects(TypeFlags::BIG_INT_LITERAL)
                                        && pseudo_big_int_to_string(
                                            &c.get_big_int_literal_value(right),
                                        ) == str
                                    {
                                        right
                                    } else if left_flags.intersects(TypeFlags::BOOLEAN) {
                                        left
                                    } else if right_flags.intersects(TypeFlags::BOOLEAN) {
                                        if str == "true" {
                                            c.true_type
                                        } else if str == "false" {
                                            c.false_type
                                        } else {
                                            c.boolean_type
                                        }
                                    } else if left_flags.intersects(TypeFlags::BOOLEAN_LITERAL) {
                                        left
                                    } else if right_flags.intersects(TypeFlags::BOOLEAN_LITERAL)
                                        && (if c.get_boolean_literal_value(right) {
                                            "true"
                                        } else {
                                            "false"
                                        }) == str
                                    {
                                        right
                                    } else if left_flags.intersects(TypeFlags::UNDEFINED) {
                                        left
                                    } else if right_flags.intersects(TypeFlags::UNDEFINED)
                                        && c.ty(right).as_intrinsic_type().intrinsic_name() == str
                                    {
                                        right
                                    } else if left_flags.intersects(TypeFlags::NULL) {
                                        left
                                    } else if right_flags.intersects(TypeFlags::NULL)
                                        && c.ty(right).as_intrinsic_type().intrinsic_name() == str
                                    {
                                        right
                                    } else {
                                        left
                                    }
                                };
                                let mut matching_type = self.never_type;
                                for t in self.ty(constraint).distributed() {
                                    matching_type = choose(self, matching_type, t);
                                }
                                if !self.ty(matching_type).flags.intersects(TypeFlags::NEVER) {
                                    self.infer_from_types(n, matching_type, target);
                                    continue 'outer;
                                }
                            }
                        }
                    }
                }
                self.infer_from_types(n, source, target);
            }
        }
    }

    // Go: checker/inference.go:687 inferFromGenericMappedTypes
    pub fn infer_from_generic_mapped_types(
        &mut self,
        n: &mut InferenceState,
        source: TypeId,
        target: TypeId,
    ) {
        // The source and target types are generic types { [P in S]: X } and { [P in T]: Y }, so we infer
        // from S to T and from X to Y.
        let s = self.get_constraint_type_from_mapped_type(source);
        let t = self.get_constraint_type_from_mapped_type(target);
        self.infer_from_types(n, s, t);
        let s = self.get_template_type_from_mapped_type(source);
        let t = self.get_template_type_from_mapped_type(target);
        self.infer_from_types(n, s, t);
        let source_name_type = self.get_name_type_from_mapped_type(source);
        let target_name_type = self.get_name_type_from_mapped_type(target);
        if source_name_type.is_some() && target_name_type.is_some() {
            self.infer_from_types(n, source_name_type, target_name_type);
        }
    }

    // Go: checker/inference.go:699 inferFromObjectTypes
    pub fn infer_from_object_types(
        &mut self,
        n: &mut InferenceState,
        source: TypeId,
        target: TypeId,
    ) {
        if self
            .ty(source)
            .object_flags
            .intersects(ObjectFlags::REFERENCE)
            && self
                .ty(target)
                .object_flags
                .intersects(ObjectFlags::REFERENCE)
            && (self.ty(source).target() == self.ty(target).target()
                || self.is_array_type(source) && self.is_array_type(target))
        {
            // If source and target are references to the same generic type, infer from type arguments
            // ts#64553 (Go N' inference.go:713)
            self.infer_from_reference_type_arguments(n, source, target);
            return;
        }
        if self.is_generic_mapped_type(source) && self.is_generic_mapped_type(target) {
            self.infer_from_generic_mapped_types(n, source, target);
        }
        if self.ty(target).object_flags.intersects(ObjectFlags::MAPPED)
            && self
                .ty(target)
                .as_mapped_type()
                .declaration
                .name_type()
                .is_nil()
        {
            let constraint_type = self.get_constraint_type_from_mapped_type(target);
            if self.infer_to_mapped_type(n, source, target, constraint_type) {
                return;
            }
        }
        // Infer from the members of source and target only if the two types are possibly related
        if self.types_definitely_unrelated(source, target) {
            return;
        }
        if self.is_array_or_tuple_type(source) {
            if self.is_tuple_type(target) {
                let source_arity = self.get_type_reference_arity(source);
                let target_arity = self.get_type_reference_arity(target);
                let element_types = self.get_type_arguments(target);
                // PERF: Go reads the element infos in place; the port reads
                // their flags through `target_element_flags`, not a copy.
                let target_element_flags =
                    |c: &Checker, i: usize| c.target_tuple_type(target).element_infos[i].flags;
                // When source and target are tuple types with the same structure (fixed, variadic, and rest are matched
                // to the same kind in each position), simply infer between the element types.
                if self.is_tuple_type(source)
                    && self.is_tuple_type_structure_matching(source, target)
                {
                    for i in 0..target_arity as usize {
                        let s = self.type_arguments_of(source)[i];
                        self.infer_from_types(n, s, element_types[i]);
                    }
                    return;
                }
                let mut start_length: i32 = 0;
                let mut end_length: i32 = 0;
                if self.is_tuple_type(source) {
                    start_length = std::cmp::min(
                        self.target_tuple_type(source).fixed_length,
                        self.target_tuple_type(target).fixed_length,
                    );
                    if self
                        .target_tuple_type(target)
                        .combined_flags
                        .intersects(ElementFlags::VARIABLE)
                    {
                        end_length = std::cmp::min(
                            get_end_element_count(
                                self.target_tuple_type(source),
                                ElementFlags::FIXED,
                            ),
                            get_end_element_count(
                                self.target_tuple_type(target),
                                ElementFlags::FIXED,
                            ),
                        );
                    }
                }
                // Infer between starting fixed elements.
                for i in 0..start_length as usize {
                    let s = self.type_arguments_of(source)[i];
                    self.infer_from_types(n, s, element_types[i]);
                }
                if !self.is_tuple_type(source)
                    || source_arity - start_length - end_length == 1
                        && self.target_tuple_type(source).element_infos[start_length as usize]
                            .flags
                            .intersects(ElementFlags::REST)
                {
                    // Single rest element remains in source, infer from that to every element in target
                    let rest_type = self.type_arguments_of(source)[start_length as usize];
                    for i in start_length..target_arity - end_length {
                        let mut t = rest_type;
                        if target_element_flags(self, i as usize).intersects(ElementFlags::VARIADIC)
                        {
                            t = self.create_array_type(t);
                        }
                        self.infer_from_types(n, t, element_types[i as usize]);
                    }
                } else {
                    let middle_length = target_arity - start_length - end_length;
                    let sl = start_length as usize;
                    if middle_length == 2 {
                        if (target_element_flags(self, sl) & target_element_flags(self, sl + 1))
                            .intersects(ElementFlags::VARIADIC)
                        {
                            // Middle of target is [...T, ...U] and source is tuple type
                            let target_info =
                                self.get_inference_info_for_type(n, element_types[sl]);
                            if let Some(target_info) = target_info {
                                let ctx = n.inferences;
                                let implied_arity = self.inference_context(ctx).inferences
                                    [target_info]
                                    .implied_arity;
                                if implied_arity >= 0 {
                                    // Infer slices from source based on implied arity of T.
                                    let slice = self.slice_tuple_type(
                                        source,
                                        start_length,
                                        end_length + source_arity - implied_arity,
                                    );
                                    self.infer_from_types(n, slice, element_types[sl]);
                                    let slice = self.slice_tuple_type(
                                        source,
                                        start_length + implied_arity,
                                        end_length,
                                    );
                                    self.infer_from_types(n, slice, element_types[sl + 1]);
                                }
                            }
                        } else if target_element_flags(self, sl).intersects(ElementFlags::VARIADIC)
                            && target_element_flags(self, sl + 1).intersects(ElementFlags::REST)
                        {
                            // Middle of target is [...T, ...rest] and source is tuple type
                            // if T is constrained by a fixed-size tuple we might be able to use its arity to infer T
                            let info = self.get_inference_info_for_type(n, element_types[sl]);
                            if let Some(info) = info {
                                let ctx = n.inferences;
                                let type_parameter =
                                    self.inference_context(ctx).inferences[info].type_parameter;
                                let constraint = self.get_base_constraint_of_type(type_parameter);
                                if constraint.is_some()
                                    && self.is_tuple_type(constraint)
                                    && !self
                                        .target_tuple_type(constraint)
                                        .combined_flags
                                        .intersects(ElementFlags::VARIABLE)
                                {
                                    let implied_arity =
                                        self.target_tuple_type(constraint).fixed_length;
                                    let slice = self.slice_tuple_type(
                                        source,
                                        start_length,
                                        source_arity - (start_length + implied_arity),
                                    );
                                    self.infer_from_types(n, slice, element_types[sl]);
                                    let rest_type = self.get_element_type_of_slice_of_tuple_type(
                                        source,
                                        start_length + implied_arity,
                                        end_length,
                                        false,
                                        false,
                                    );
                                    if rest_type.is_some() {
                                        self.infer_from_types(n, rest_type, element_types[sl + 1]);
                                    }
                                }
                            }
                        } else if target_element_flags(self, sl).intersects(ElementFlags::REST)
                            && target_element_flags(self, sl + 1).intersects(ElementFlags::VARIADIC)
                        {
                            // Middle of target is [...rest, ...T] and source is tuple type
                            // if T is constrained by a fixed-size tuple we might be able to use its arity to infer T
                            let info = self.get_inference_info_for_type(n, element_types[sl + 1]);
                            if let Some(info) = info {
                                let ctx = n.inferences;
                                let type_parameter =
                                    self.inference_context(ctx).inferences[info].type_parameter;
                                let constraint = self.get_base_constraint_of_type(type_parameter);
                                if constraint.is_some()
                                    && self.is_tuple_type(constraint)
                                    && !self
                                        .target_tuple_type(constraint)
                                        .combined_flags
                                        .intersects(ElementFlags::VARIABLE)
                                {
                                    let implied_arity =
                                        self.target_tuple_type(constraint).fixed_length;
                                    let end_index = source_arity
                                        - get_end_element_count(
                                            self.target_tuple_type(target),
                                            ElementFlags::FIXED,
                                        );
                                    let start_index = end_index - implied_arity;
                                    if start_index >= start_length {
                                        let source_type_arguments = self.get_type_arguments(source);
                                        let source_element_infos: Vec<TupleElementInfo> =
                                            self.target_tuple_type(source).element_infos
                                                [start_index as usize..end_index as usize]
                                                .to_vec();
                                        let trailing_slice = self.create_tuple_type_ex(
                                            &source_type_arguments
                                                [start_index as usize..end_index as usize],
                                            &source_element_infos,
                                            false, /*readonly*/
                                        );
                                        let rest_type = self
                                            .get_element_type_of_slice_of_tuple_type(
                                                source,
                                                start_length,
                                                end_length + implied_arity,
                                                false,
                                                false,
                                            );
                                        if rest_type.is_some() {
                                            self.infer_from_types(n, rest_type, element_types[sl]);
                                        }
                                        self.infer_from_types(
                                            n,
                                            trailing_slice,
                                            element_types[sl + 1],
                                        );
                                    }
                                }
                            }
                        }
                    } else if middle_length == 1
                        && target_element_flags(self, sl).intersects(ElementFlags::VARIADIC)
                    {
                        // Middle of target is exactly one variadic element. Infer the slice between the fixed parts in the source.
                        // If target ends in optional element(s), make a lower priority a speculative inference.
                        let priority = if target_element_flags(self, (target_arity - 1) as usize)
                            .intersects(ElementFlags::OPTIONAL)
                        {
                            InferencePriority::SPECULATIVE_TUPLE
                        } else {
                            InferencePriority::NONE
                        };
                        let source_slice = self.slice_tuple_type(source, start_length, end_length);
                        self.infer_with_priority(n, source_slice, element_types[sl], priority);
                    } else if middle_length == 1
                        && target_element_flags(self, sl).intersects(ElementFlags::REST)
                    {
                        // Middle of target is exactly one rest element. If middle of source is not empty, infer union of middle element types.
                        let rest_type = self.get_element_type_of_slice_of_tuple_type(
                            source,
                            start_length,
                            end_length,
                            false,
                            false,
                        );
                        if rest_type.is_some() {
                            self.infer_from_types(n, rest_type, element_types[sl]);
                        }
                    }
                }
                // Infer between ending fixed elements
                for i in 0..end_length {
                    let s = self.type_arguments_of(source)[(source_arity - i - 1) as usize];
                    self.infer_from_types(n, s, element_types[(target_arity - i - 1) as usize]);
                }
                return;
            }
            if self.is_array_type(target) {
                self.infer_from_index_types(n, source, target);
                return;
            }
        }
        self.infer_from_properties(n, source, target);
        self.infer_from_signatures(n, source, target, SignatureKind::CALL);
        self.infer_from_signatures(n, source, target, SignatureKind::CONSTRUCT);
        self.infer_from_index_types(n, source, target);
    }

    // Go: checker/inference.go:828 inferFromProperties
    pub fn infer_from_properties(
        &mut self,
        n: &mut InferenceState,
        source: TypeId,
        target: TypeId,
    ) {
        let properties = self.get_properties_of_object_type(target);
        for target_prop in properties {
            // PORT: perf. The lookup takes the `Name`, so the member table
            // compares ids instead of hashing and comparing text.
            let name = self.sym(target_prop).name.clone();
            let source_prop = self.get_property_of_type_name(source, &name);
            // PERF: the skip set is empty outside the language service, and
            // then no declaration is in it, so the scan is skipped.
            if source_prop.is_some()
                && (self.skip_direct_inference_nodes.is_empty()
                    || !self
                        .sym(source_prop)
                        .declarations
                        .iter()
                        .any(|&d| self.is_skip_direct_inference_node(d)))
            {
                let source_type = self.get_type_of_symbol(source_prop);
                let source_optional = self
                    .sym(source_prop)
                    .flags
                    .intersects(SymbolFlags::OPTIONAL);
                let s = self.remove_missing_type(source_type, source_optional);
                let target_type = self.get_type_of_symbol(target_prop);
                let target_optional = self
                    .sym(target_prop)
                    .flags
                    .intersects(SymbolFlags::OPTIONAL);
                let t = self.remove_missing_type(target_type, target_optional);
                self.infer_from_types(n, s, t);
            }
        }
    }

    // Go: checker/inference.go:838 inferFromSignatures
    pub fn infer_from_signatures(
        &mut self,
        n: &mut InferenceState,
        source: TypeId,
        target: TypeId,
        kind: SignatureKind,
    ) {
        let source_signatures = self.get_signatures_of_type(source, kind);
        let source_len = source_signatures.len() as i32;
        if source_len > 0 {
            // We match source and target signatures from the bottom up, and if the source has fewer signatures
            // than the target, we infer from the first source signature to the excess target signatures.
            let target_signatures = self.get_signatures_of_type(target, kind);
            let target_len = target_signatures.len() as i32;
            for i in 0..target_len {
                let source_index = std::cmp::max(source_len - target_len + i, 0);
                let s = self.get_base_signature(source_signatures[source_index as usize]);
                let t = self.get_erased_signature(target_signatures[i as usize]);
                self.infer_from_signature(n, s, t);
            }
        }
    }

    // Go: checker/inference.go:853 inferFromSignature
    pub fn infer_from_signature(
        &mut self,
        n: &mut InferenceState,
        source: SignatureId,
        target: SignatureId,
    ) {
        if !self
            .sig(source)
            .flags
            .intersects(SignatureFlags::IS_NON_INFERRABLE)
        {
            let save_bivariant = n.bivariant;
            let mut kind = SyntaxKind::Unknown;
            let declaration = self.sig(target).declaration;
            if declaration.is_some() {
                kind = declaration.kind();
            }
            // Once we descend into a bivariant signature we remain bivariant for all nested inferences
            {
                let s = &mut *n;
                s.bivariant = s.bivariant
                    || kind == SyntaxKind::MethodDeclaration
                    || kind == SyntaxKind::MethodSignature
                    || kind == SyntaxKind::Constructor;
            }
            self.apply_to_parameter_types(
                source,
                target,
                &mut |c: &mut Checker, s: TypeId, t: TypeId| {
                    c.infer_from_contravariant_types_if_strict_function_types(n, s, t)
                },
            );
            n.bivariant = save_bivariant;
        }
        self.apply_to_return_types(
            source,
            target,
            &mut |c: &mut Checker, s: TypeId, t: TypeId| c.infer_from_types(n, s, t),
        );
    }

    // PORT: Go `callback func(s, t *Type)` closes over the checker; the Rust
    // callback receives it as its first argument.
    // Go: checker/inference.go:868 applyToParameterTypes
    pub fn apply_to_parameter_types(
        &mut self,
        source: SignatureId,
        target: SignatureId,
        callback: &mut dyn FnMut(&mut Checker, TypeId, TypeId),
    ) {
        let source_count = self.get_parameter_count(source);
        let target_count = self.get_parameter_count(target);
        let source_rest_type = self.get_effective_rest_type(source);
        let target_rest_type = self.get_effective_rest_type(target);
        let mut target_non_rest_count = target_count;
        if target_rest_type.is_some() {
            target_non_rest_count -= 1;
        }
        let mut param_count = target_non_rest_count;
        if source_rest_type.is_nil() {
            param_count = std::cmp::min(source_count, target_non_rest_count);
        }
        let source_this_type = self.get_this_type_of_signature(source);
        if source_this_type.is_some() {
            let target_this_type = self.get_this_type_of_signature(target);
            if target_this_type.is_some() {
                callback(self, source_this_type, target_this_type);
            }
        }
        for i in 0..param_count {
            let s = self.get_type_at_position(source, i);
            let t = self.get_type_at_position(target, i);
            callback(self, s, t);
        }
        if target_rest_type.is_some() {
            let readonly = self.is_const_type_variable(target_rest_type, 0)
                && !self.some_type(target_rest_type, &mut |c: &mut Checker, t: TypeId| {
                    c.is_mutable_array_like_type(t)
                });
            let s = self.get_rest_type_at_position(source, param_count, readonly /*readonly*/);
            callback(self, s, target_rest_type);
        }
    }

    // Go: checker/inference.go:896 applyToReturnTypes
    pub fn apply_to_return_types(
        &mut self,
        source: SignatureId,
        target: SignatureId,
        callback: &mut dyn FnMut(&mut Checker, TypeId, TypeId),
    ) {
        let target_type_predicate = self.get_type_predicate_of_signature(target);
        if target_type_predicate.is_some() {
            let source_type_predicate = self.get_type_predicate_of_signature(source);
            if source_type_predicate.is_some()
                && self.type_predicate_kinds_match(source_type_predicate, target_type_predicate)
                && self.pred(source_type_predicate).t.is_some()
                && self.pred(target_type_predicate).t.is_some()
            {
                let s = self.pred(source_type_predicate).t;
                let t = self.pred(target_type_predicate).t;
                callback(self, s, t);
                return;
            }
        }
        let target_return_type = self.get_return_type_of_signature(target);
        if self.could_contain_type_variables(target_return_type) {
            let s = self.get_return_type_of_signature(source);
            callback(self, s, target_return_type);
        }
    }

    // Go: checker/inference.go:911 inferFromIndexTypes
    pub fn infer_from_index_types(
        &mut self,
        n: &mut InferenceState,
        source: TypeId,
        target: TypeId,
    ) {
        // Inferences across mapped type index signatures are pretty much the same a inferences to homomorphic variables
        let mut priority = InferencePriority::NONE;
        if (self.ty(source).object_flags & self.ty(target).object_flags)
            .intersects(ObjectFlags::MAPPED)
        {
            priority = InferencePriority::HOMOMORPHIC_MAPPED_TYPE;
        }
        let index_infos = self.get_index_infos_of_type(target);
        if self.is_object_type_with_inferable_index(source) {
            for &target_info in &index_infos {
                let target_key_type = self.index_info(target_info).key_type;
                let target_value_type = self.index_info(target_info).value_type;
                // PERF: a temporary list, on the stack up to 8 types.
                let mut prop_types: SmallVec<[TypeId; 8]> = SmallVec::new();
                for prop in self.get_properties_of_type(source) {
                    let literal = self.get_literal_type_from_property(
                        prop,
                        TypeFlags::STRING_OR_NUMBER_LITERAL_OR_UNIQUE,
                        false,
                    );
                    if self.is_applicable_index_type(literal, target_key_type) {
                        let mut prop_type = self.get_type_of_symbol(prop);
                        if self.sym(prop).flags.intersects(SymbolFlags::OPTIONAL) {
                            prop_type = self.remove_missing_or_undefined_type(prop_type);
                        }
                        prop_types.push(prop_type);
                    }
                }
                for info in self.get_index_infos_of_type(source) {
                    let key_type = self.index_info(info).key_type;
                    if self.is_applicable_index_type(key_type, target_key_type) {
                        prop_types.push(self.index_info(info).value_type);
                    }
                }
                if !prop_types.is_empty() {
                    let union = self.get_union_type(&prop_types);
                    self.infer_with_priority(n, union, target_value_type, priority);
                }
            }
        }
        for &target_info in &index_infos {
            let target_key_type = self.index_info(target_info).key_type;
            let source_info = self.get_applicable_index_info(source, target_key_type);
            if source_info.is_some() {
                let s = self.index_info(source_info).value_type;
                let t = self.index_info(target_info).value_type;
                self.infer_with_priority(n, s, t, priority);
            }
        }
    }

    // Go: checker/inference.go:948 inferToMappedType
    pub fn infer_to_mapped_type(
        &mut self,
        n: &mut InferenceState,
        source: TypeId,
        target: TypeId,
        constraint_type: TypeId,
    ) -> bool {
        if self.ty(constraint_type).flags.intersects(TypeFlags::UNION)
            || self
                .ty(constraint_type)
                .flags
                .intersects(TypeFlags::INTERSECTION)
        {
            let mut result = false;
            for t in self.ty(constraint_type).types_list() {
                let r = self.infer_to_mapped_type(n, source, target, t);
                result = r || result;
            }
            return result;
        }
        if self.ty(constraint_type).flags.intersects(TypeFlags::INDEX) {
            // We're inferring from some source type S to a homomorphic mapped type { [P in keyof T]: X },
            // where T is a type variable. Use inferTypeForHomomorphicMappedType to infer a suitable source
            // type and then make a secondary inference from that type to T. We make a secondary inference
            // such that direct inferences to T get priority over inferences to Partial<T>, for example.
            let index_target = self.ty(constraint_type).as_index_type().target;
            let inference = self.get_inference_info_for_type(n, index_target);
            if let Some(inference) = inference {
                let ctx = n.inferences;
                let (is_fixed, type_parameter) = {
                    let info = &self.inference_context(ctx).inferences[inference];
                    (info.is_fixed, info.type_parameter)
                };
                if !is_fixed && !self.is_from_inference_blocked_source(source) {
                    let inferred_type = self.infer_type_for_homomorphic_mapped_type(
                        source,
                        target,
                        constraint_type,
                    );
                    if inferred_type.is_some() {
                        // We assign a lower priority to inferences made from types containing non-inferrable
                        // types because we may only have a partial result (i.e. we may have failed to make
                        // reverse inferences for some properties).
                        let priority = if self
                            .ty(source)
                            .object_flags
                            .intersects(ObjectFlags::NON_INFERRABLE_TYPE)
                        {
                            InferencePriority::PARTIAL_HOMOMORPHIC_MAPPED_TYPE
                        } else {
                            InferencePriority::HOMOMORPHIC_MAPPED_TYPE
                        };
                        self.infer_with_priority(n, inferred_type, type_parameter, priority);
                    }
                }
            }
            return true;
        }
        if self
            .ty(constraint_type)
            .flags
            .intersects(TypeFlags::TYPE_PARAMETER)
        {
            // We're inferring from some source type S to a mapped type { [P in K]: X }, where K is a type
            // parameter. First infer from 'keyof S' to K.
            let index_flags = if self
                .pattern_for_type
                .get(&source)
                .is_some_and(|p| p.is_some())
            {
                IndexFlags::NO_INDEX_SIGNATURES
            } else {
                IndexFlags::NONE
            };
            let index_type = self.get_index_type_ex(source, index_flags);
            self.infer_with_priority(
                n,
                index_type,
                constraint_type,
                InferencePriority::MAPPED_TYPE_CONSTRAINT,
            );
            // If K is constrained to a type C, also infer to C. Thus, for a mapped type { [P in K]: X },
            // where K extends keyof T, we make the same inferences as for a homomorphic mapped type
            // { [P in keyof T]: X }. This enables us to make meaningful inferences when the target is a
            // Pick<T, K>.
            let extended_constraint = self.get_constraint_of_type(constraint_type);
            if extended_constraint.is_some()
                && self.infer_to_mapped_type(n, source, target, extended_constraint)
            {
                return true;
            }
            // If no inferences can be made to K's constraint, infer from a union of the property types
            // in the source to the template type X.
            let mut prop_types: Vec<TypeId> = Vec::new();
            for prop in self.get_properties_of_type(source) {
                prop_types.push(self.get_type_of_symbol(prop));
            }
            let mut index_types: Vec<TypeId> = Vec::new();
            for info in self.get_index_infos_of_type(source) {
                if info != self.enum_number_index_info {
                    index_types.push(self.index_info(info).value_type);
                } else {
                    index_types.push(self.never_type);
                }
            }
            prop_types.extend(index_types);
            let union = self.get_union_type(&prop_types);
            let template_type = self.get_template_type_from_mapped_type(target);
            self.infer_from_types(n, union, template_type);
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recursion_key_round_trip() {
        let ids = [
            RecursionId::Node(Node(1)),
            RecursionId::Node(Node((7 << 32) | 3)),
            RecursionId::Node(Node((0xffff_fffe << 32) | 0xffff_ffff)),
            RecursionId::Symbol(SymbolId(1)),
            RecursionId::Symbol(SymbolId(0x7fff_ffff)),
            RecursionId::Type(TypeId(0)),
            RecursionId::Type(TypeId(1)),
            RecursionId::Type(TypeId(0x7fff_ffff)),
        ];
        for (i, &a) in ids.iter().enumerate() {
            let key = RecursionKey::new(Some(a));
            assert_ne!(key, RecursionKey::NONE);
            assert_eq!(Option::<RecursionId>::from(key), Some(a));
            for &b in &ids[i + 1..] {
                assert_ne!(key, RecursionKey::new(Some(b)));
            }
        }
        for id in [
            None,
            Some(RecursionId::Node(Node(5 << 32))),
            Some(RecursionId::Symbol(SymbolId(0))),
            Some(RecursionId::Symbol(SymbolId(0x8000_0000))),
            Some(RecursionId::Type(TypeId(0x8000_0000))),
        ] {
            assert_eq!(RecursionKey::new(id), RecursionKey::NONE);
        }
    }

    /// The Go scan of `isDeeplyNestedType` with the top entry as the probe,
    /// comparing stored ids.
    fn scan(stack: &InferenceStack, max_depth: i32) -> bool {
        if (stack.types.len() as i32) < max_depth {
            return false;
        }
        let probe = stack.entries.last().unwrap().id;
        let mut count = 0;
        let mut last = TypeId(0);
        for (&t, entry) in stack.types.iter().zip(&stack.entries) {
            if entry.id == probe {
                if t >= last {
                    count += 1;
                    if count >= max_depth {
                        return true;
                    }
                }
                last = t;
            }
        }
        false
    }

    #[test]
    fn stack_answer_matches_scan() {
        let keys = [
            RecursionKey::NONE,
            RecursionKey::new(Some(RecursionId::Node(Node((1 << 32) | 9)))),
            RecursionKey::new(Some(RecursionId::Symbol(SymbolId(9)))),
            RecursionKey::new(Some(RecursionId::Type(TypeId(9)))),
            RecursionKey::new(Some(RecursionId::Type(TypeId(10)))),
        ];
        let mut stack = InferenceStack::default();
        let mut rng: u64 = 0x9e37_79b9_7f4a_7c15;
        let mut fast_answers = 0;
        for _ in 0..200_000 {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            let r = rng >> 16;
            match r % 100 {
                0 => stack.clear(),
                1..=54 if stack.types.len() < 30 => {
                    // NONE entries are rare, so most stacks have none.
                    let key = if r % 1000 < 20 {
                        keys[0]
                    } else {
                        keys[1 + (r / 100 % 4) as usize]
                    };
                    stack.push(TypeId((r / 1000 % 6) as u32 + 1), key);
                    let has_none = stack.entries.iter().any(|e| e.id == RecursionKey::NONE);
                    for max_depth in 1..=4 {
                        let fast = stack.top_deeply_nested(max_depth);
                        if has_none {
                            assert_eq!(fast, None);
                        } else {
                            assert_eq!(fast, Some(scan(&stack, max_depth)));
                            fast_answers += 1;
                        }
                    }
                }
                _ if !stack.types.is_empty() => stack.pop(),
                _ => {}
            }
        }
        assert!(fast_answers > 1000);
    }
}
