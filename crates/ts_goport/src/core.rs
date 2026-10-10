//! Shared handles and stores for the Go port. Read `PORTING.md` first.
//!
//! Every Go pointer to a shared object becomes a `Copy` handle. Handle value
//! zero is Go `nil`, so Go `x == nil` ports to `x.is_nil()`.

use indexmap::IndexMap;
use rustc_hash::{FxBuildHasher, FxHashMap, FxHashSet};
use std::sync::Arc;

/// `IndexMap` with the Fx hasher. Keys are small ids or short names, so Fx
/// is faster than SipHash. Iteration order is still insertion order.
pub type FxIndexMap<K, V> = IndexMap<K, V, FxBuildHasher>;
/// `IndexSet` with the Fx hasher.
pub type FxIndexSet<K> = indexmap::IndexSet<K, FxBuildHasher>;

use crate::flags::{CheckFlags, FlowFlags, NodeFlags, SymbolFlags};

macro_rules! handle {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Default, Eq, Hash, Ord, PartialEq, PartialOrd, Debug)]
        pub struct $name(pub u32);

        impl $name {
            /// Go `nil`.
            pub const NIL: Self = Self(0);

            #[must_use]
            pub const fn is_nil(self) -> bool {
                self.0 == 0
            }

            #[must_use]
            pub const fn is_some(self) -> bool {
                self.0 != 0
            }

            /// Arena index. Arenas keep a dummy entry at index 0.
            #[must_use]
            pub const fn index(self) -> usize {
                self.0 as usize
            }

            /// Returns `None` for nil.
            #[must_use]
            pub const fn get(self) -> Option<Self> {
                if self.0 == 0 { None } else { Some(self) }
            }
        }
    };
}

handle!(
    /// Go `*Type`. Index into `Checker::types`.
    TypeId
);
handle!(
    /// Go `*ast.Symbol`. Index into `SymbolArena::symbols`.
    SymbolId
);
handle!(
    /// Go `*Signature`. Index into `Checker::signatures`.
    SignatureId
);
handle!(
    /// Go `*IndexInfo`. Index into `Checker::index_infos`.
    IndexInfoId
);
handle!(
    /// Go `*TypePredicate`. Index into `Checker::type_predicates`.
    TypePredicateId
);
handle!(
    /// Go `*TypeMapper`. Index into `Checker::mappers`.
    MapperId
);
handle!(
    /// Go `*InferenceContext`. Index into `Checker::inference_contexts`.
    InferenceContextId
);
handle!(
    /// Go `ast.SymbolTable` (a Go map, so it has reference semantics).
    /// Index into `SymbolArena::tables`. Nil is a nil map: reads see it as empty.
    SymbolTable
);

/// Go `*ast.FlowNode`. High 32 bits: file index. Low 32 bits: index + 1 into
/// that file's `GoFile::flow_nodes`. Zero is nil. Read with `flow.get_flow()`
/// (defined in `crate::ast`).
#[derive(Clone, Copy, Default, Eq, Hash, Ord, PartialEq, PartialOrd, Debug)]
pub struct FlowNodeId(pub u64);

impl FlowNodeId {
    pub const NIL: Self = Self(0);

    #[must_use]
    pub const fn is_nil(self) -> bool {
        self.0 == 0
    }

    #[must_use]
    pub const fn is_some(self) -> bool {
        self.0 != 0
    }

    #[must_use]
    pub const fn new(file: usize, index: usize) -> Self {
        Self(((file as u64) << 32) | (index as u64 + 1))
    }

    #[must_use]
    pub const fn file_index(self) -> usize {
        (self.0 >> 32) as usize
    }

    #[must_use]
    pub const fn local_index(self) -> usize {
        ((self.0 & 0xffff_ffff) - 1) as usize
    }
}

/// Go `*ast.Node` (and every alias: `*ast.Expression`, `*ast.TypeNode`,
/// `*ast.SourceFile`, ...). High 32 bits: file id in the file registry
/// (`ast/store.rs`). For a ported-parser file this is also its store id.
/// Low 32 bits: `crate::astdata::NodeId::index() + 1`. Zero is nil.
/// Node methods (kind, parent, fields, binder data) live in `crate::ast`.
#[derive(Clone, Copy, Default, Eq, Hash, Ord, PartialEq, PartialOrd, Debug)]
pub struct Node(pub u64);

impl Node {
    pub const NIL: Self = Self(0);

    #[must_use]
    pub const fn is_nil(self) -> bool {
        self.0 == 0
    }

    #[must_use]
    pub const fn is_some(self) -> bool {
        self.0 != 0
    }

    #[must_use]
    pub const fn get(self) -> Option<Self> {
        if self.0 == 0 { None } else { Some(self) }
    }

    #[inline]
    #[must_use]
    pub fn new(file: usize, node: crate::astdata::NodeId) -> Self {
        // After freeze, a store file resolves every child id with no store
        // borrow: an alias-free store gives the slot 0 value for id 0 and
        // the raw handle `(file << 32) | (id + 1)` for any other id, with no
        // table load; a store with alias slots reads the record of the id.
        // The synthetic file has no store, so it takes the slow path.
        if let Some(n) = crate::ast::frozen_resolve_store_id(file, node) {
            return n;
        }
        Self::new_slow(file, node)
    }

    #[inline(never)]
    fn new_slow(file: usize, node: crate::astdata::NodeId) -> Self {
        // Child ids inside factory-made nodes live in the synthetic id space.
        if file == crate::ast::SYNTHETIC_NODE_FILE {
            return crate::ast::resolve_synthetic_id(node);
        }
        // Child ids inside nodes of a ported-parser file are store slots.
        if let Some(n) = crate::ast::try_resolve_store_id(file, node) {
            return n;
        }
        Self(((file as u64) << 32) | (node.index() as u64 + 1))
    }

    /// File id in the file registry (`crate::ast::go_file`).
    #[must_use]
    pub const fn file_index(self) -> usize {
        (self.0 >> 32) as usize
    }

    #[must_use]
    pub fn node_id(self) -> crate::astdata::NodeId {
        crate::astdata::NodeId::new(((self.0 & 0xffff_ffff) - 1) as u32)
    }
}

/// A symbol name and symbol table key (Go `string`). Every distinct text is
/// stored once for the whole process (see `intern`), and a `Name` is the
/// 4-byte id of that text. Clones copy the id. Equal names have equal ids,
/// so `==` compares ids. It orders, hashes and prints like the `str` it
/// holds. The low bit of the id is set when the text starts with
/// `INTERNAL_SYMBOL_NAME_PREFIX` (`Name::is_internal`).
#[derive(PartialEq, Eq)]
pub struct Name(u32);

impl Name {
    /// The text. Interned text lives until the process ends.
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        intern::text(self.0)
    }

    /// True when the text starts with `INTERNAL_SYMBOL_NAME_PREFIX`.
    /// The id holds this bit, so this reads no text.
    #[inline]
    #[must_use]
    pub fn is_internal(&self) -> bool {
        self.0 & intern::INTERNAL_BIT != 0
    }

    /// True when the text is `INTERNAL_SYMBOL_NAME_DEFAULT` ("default").
    /// That name is interned first, at a fixed id, so this reads no text.
    #[inline]
    #[must_use]
    pub fn is_default_symbol_name(&self) -> bool {
        self.0 == intern::DEFAULT_ID
    }

    /// `table_hash` of the text. The interner keeps it, so this reads one
    /// number and does not touch the text.
    #[inline]
    fn table_hash(&self) -> u32 {
        intern::table_hash(self.0)
    }

    /// The id of this name when every process of this build gives the text
    /// the same id: "", "default" and the bundled lib names (`lib_names`).
    /// `None` for other names, whose ids depend on intern order. The lib
    /// bind snapshot (`binder/lib_snapshot.rs`) stores these names by id.
    #[inline]
    #[must_use]
    pub fn stable_id(&self) -> Option<u32> {
        intern::is_stable(self.0).then_some(self.0)
    }

    /// The name of stable id `id` (`stable_id`), ready for table lookups as
    /// if `Name::from` had made it. `None` when `id` is not a stable id.
    #[must_use]
    pub fn from_stable_id(id: u32) -> Option<Name> {
        intern::stable(id)
    }

    /// The 4-byte id, for a packed node column (`ast::store::NodeKids`).
    /// It is valid only in this process.
    #[inline]
    #[must_use]
    pub(crate) fn id(&self) -> u32 {
        self.0
    }

    /// The name whose `Name::id` in this process is `id`.
    #[inline]
    #[must_use]
    pub(crate) fn from_id(id: u32) -> Name {
        Name(id)
    }
}

// PORT: not `Copy`, so existing `.clone()` calls stay clean for clippy.
impl Clone for Name {
    #[inline]
    fn clone(&self) -> Self {
        Name(self.0)
    }
}

impl Default for Name {
    fn default() -> Self {
        Name(0)
    }
}

impl std::hash::Hash for Name {
    // Hashes the text, so `Borrow<str>` lookups stay correct.
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.as_str().hash(state);
    }
}

impl PartialOrd for Name {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Name {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        if self.0 == other.0 {
            return std::cmp::Ordering::Equal;
        }
        self.as_str().cmp(other.as_str())
    }
}

impl std::ops::Deref for Name {
    type Target = str;
    #[inline]
    fn deref(&self) -> &str {
        self.as_str()
    }
}

impl std::borrow::Borrow<str> for Name {
    fn borrow(&self) -> &str {
        self.as_str()
    }
}

impl AsRef<str> for Name {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl std::fmt::Debug for Name {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(self.as_str(), f)
    }
}

impl std::fmt::Display for Name {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(self.as_str(), f)
    }
}

impl From<&str> for Name {
    fn from(s: &str) -> Self {
        intern::intern(s)
    }
}

impl From<String> for Name {
    fn from(s: String) -> Self {
        intern::intern(&s)
    }
}

impl From<&String> for Name {
    fn from(s: &String) -> Self {
        intern::intern(s)
    }
}

impl From<&Name> for Name {
    fn from(s: &Name) -> Self {
        s.clone()
    }
}

impl From<Name> for String {
    fn from(s: Name) -> Self {
        s.as_str().to_owned()
    }
}

impl Name {
    /// True when the text is `other`. The `str` compares below use it.
    // PERF: the internal bit is set from this same prefix test at intern
    // time, so a name and a text that differ in it are not equal, and the
    // text is not read. For a constant `other` the prefix test folds at
    // compile time (`name != INTERNAL_SYMBOL_NAME_COMPUTED` reads no text
    // for a normal name).
    #[inline]
    fn eq_text(&self, other: &str) -> bool {
        self.is_internal() == other.starts_with(crate::ast::INTERNAL_SYMBOL_NAME_PREFIX)
            && self.as_str() == other
    }
}

impl PartialEq<str> for Name {
    #[inline]
    fn eq(&self, other: &str) -> bool {
        self.eq_text(other)
    }
}

impl PartialEq<&str> for Name {
    #[inline]
    fn eq(&self, other: &&str) -> bool {
        self.eq_text(other)
    }
}

impl PartialEq<String> for Name {
    #[inline]
    fn eq(&self, other: &String) -> bool {
        self.eq_text(other)
    }
}

impl PartialEq<Name> for str {
    #[inline]
    fn eq(&self, other: &Name) -> bool {
        other.eq_text(self)
    }
}

impl PartialEq<Name> for &str {
    #[inline]
    fn eq(&self, other: &Name) -> bool {
        other.eq_text(self)
    }
}

impl PartialEq<Name> for String {
    #[inline]
    fn eq(&self, other: &Name) -> bool {
        other.eq_text(self)
    }
}

/// The names of the bundled lib files, made by
/// `crates/ts_goport/scripts/gen-lib-names.py` (`intern::lib_name`).
#[cfg(not(target_family = "wasm"))]
mod lib_names;

/// wasm: an empty lib name table, so a lib name goes through the shards
/// like any other name. The table only makes interning faster. Only the lib
/// snapshots need its stable ids (`Name::stable_id`), and wasm reads no
/// snapshot (`bundled::bundled_lib_name`). No output depends on an id:
/// `Name` hashes and orders by its text, and tables compare ids only for
/// equality.
// PERF: the table is 184 KB of data. Without it the module is 185 KB
// smaller raw, 75 KB smaller with gzip -9 and 62 KB smaller with brotli
// -q 11. `scripts/wasm/bench.mjs` (query core, hono, zod; 10 interleaved
// rounds) put each warm and cold median within -2.1% to +0.9% of the
// module with the table. Packing the names with LZMA and building the
// same table on first use saved only 37 KB gzip, and was not faster.
#[cfg(target_family = "wasm")]
mod lib_names {
    pub(super) const COUNT: usize = 0;
    pub(super) const BUCKET_BITS: u32 = 0;
    pub(super) static TEXT: &str = "";
    pub(super) static OFFSETS: [u32; COUNT + 1] = [0];
    pub(super) static BUCKETS: [u16; (1 << BUCKET_BITS) + 1] = [0, 0];
}

/// The process-wide string interner behind `Name`. Text is copied once into
/// leaked blocks and never freed. A name id is `seq << 1 | internal`, where
/// `seq` is a sequence number that starts at 1 and is close to dense (see
/// `next_id`) and `internal` is `Name::is_internal`. The text tables are
/// indexed by `seq` (`id >> 1`). Id 0 is "". Sequence number 1 is
/// "default" (`DEFAULT_ID`), interned when the shards are made. The names of
/// the bundled lib files (`lib_names`) have the sequence numbers from
/// `LIB_FIRST_SEQ` on, in table order, and are never in the shards.
/// `text` reads without a lock; `intern` takes one shard lock on a miss in
/// the per-thread cache and the lib name table.
mod intern {
    use super::{Name, lib_names};
    use rustc_hash::{FxBuildHasher, FxHashMap};
    use std::cell::{Cell, OnceCell};
    use std::collections::HashMap;
    use std::hash::{BuildHasherDefault, Hasher};
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::{Mutex, OnceLock, PoisonError};

    /// Chunk `k` holds `FIRST_CHUNK << k` ids, so a few chunks cover all ids.
    const FIRST_CHUNK_SHIFT: u32 = 10;
    const CHUNKS: usize = 23;
    const SHARDS: usize = 32;
    const BLOCK: usize = 16 * 1024;
    /// `CACHE` sets. Each set holds `CACHE_WAYS` slots, so the cache holds
    /// 2048 names.
    const CACHE_SETS: usize = 1024;
    const CACHE_WAYS: usize = 2;
    /// Initial capacity of each shard map (at least 32K names in all), so
    /// interning a project rarely grows and rehashes a map under its lock.
    const SHARD_CAPACITY: usize = 1024;

    type Slots = Box<[OnceLock<&'static str>]>;
    static TEXTS: [OnceLock<Slots>; CHUNKS] = [const { OnceLock::new() }; CHUNKS];
    /// The table hash (`super::fold_hash`) of each id's text, in the same
    /// chunks as `TEXTS`. A table lookup by `Name` reads it instead of
    /// reading and hashing the text again.
    static HASHES: [OnceLock<Box<[AtomicU32]>>; CHUNKS] = [const { OnceLock::new() }; CHUNKS];
    /// The next sequence number block. Only `next_id` reads it. Sequence
    /// number 1 is `DEFAULT_ID`, and the lib names come next.
    static NEXT: AtomicU32 = AtomicU32::new(LIB_FIRST_SEQ + lib_names::COUNT as u32);
    /// The sequence number of lib name 0 (`lib_names`).
    const LIB_FIRST_SEQ: u32 = 2;
    /// Sequence numbers that a thread takes from `NEXT` at once.
    const ID_BLOCK: u32 = 64;
    /// Every sequence number is below this, so `seq << 1 | 1` fits in a
    /// `u32`.
    const SEQ_LIMIT: u32 = 1 << 31;
    /// The id bit that `Name::is_internal` reads.
    pub(super) const INTERNAL_BIT: u32 = 1;
    /// The id of `INTERNAL_SYMBOL_NAME_DEFAULT` ("default"): sequence
    /// number 1, not internal. `shards` interns it first.
    // PERF: effect P7-2. `is_static_private_identifier_property` tests for
    // this name with an id compare instead of a text load.
    pub(super) const DEFAULT_ID: u32 = 1 << 1;

    /// Hasher for `u64` keys that already are a `hash_str` hash, so a
    /// lookup does not hash the text again. The halves swap because the
    /// shard index uses the top bits, and the map also reads the top bits.
    #[derive(Default)]
    struct HashIsKey(u64);

    impl Hasher for HashIsKey {
        fn finish(&self) -> u64 {
            self.0
        }

        fn write(&mut self, _: &[u8]) {
            unreachable!("HashIsKey takes only u64 keys");
        }

        fn write_u64(&mut self, hash: u64) {
            self.0 = hash.rotate_left(32);
        }
    }

    struct Shard {
        /// `(text, id)` by the `hash_str` hash of the text.
        ids: HashMap<u64, (&'static str, u32), BuildHasherDefault<HashIsKey>>,
        /// Texts whose hash another text in `ids` already has.
        collisions: FxHashMap<&'static str, u32>,
        /// Unused tail of the current text block.
        free: &'static mut [u8],
    }

    /// A shard lock on its own 128 bytes, so threads that lock neighboring
    /// shards do not share a cache line (x86 fetches lines in pairs).
    #[repr(align(128))]
    struct PaddedShard(Mutex<Shard>);

    static SHARD_LOCKS: OnceLock<[PaddedShard; SHARDS]> = OnceLock::new();

    /// One `CACHE` slot: (hash, id, text). Id 0 marks an empty slot.
    type CacheSlot = Cell<(u64, u32, &'static str)>;

    /// One `CACHE` set: slot 0 is the most recently used. Two 32-byte
    /// slots on one 64-byte line, so a lookup reads one cache line.
    #[repr(align(64))]
    struct CacheSet([CacheSlot; CACHE_WAYS]);

    /// The `CACHE` of one thread (64 KiB).
    type Cache = [CacheSet; CACHE_SETS];

    /// An empty `Cache` on the heap.
    #[cold]
    fn new_cache() -> Box<Cache> {
        const EMPTY: CacheSet = CacheSet([const { Cell::new((0, 0, "")) }; CACHE_WAYS]);
        let sets: Box<[CacheSet]> = (0..CACHE_SETS).map(|_| EMPTY).collect();
        match sets.try_into() {
            Ok(cache) => cache,
            Err(_) => unreachable!("the iterator makes CACHE_SETS sets"),
        }
    }

    thread_local! {
        /// 2-way set-associative cache of recent `intern` results, made at
        /// the first `intern` of the thread. A hit needs no borrow flag or
        /// `text` lookup.
        // PERF: two ways in place of one direct-mapped slot, so two hot
        // names with the same set do not evict each other. The cache only
        // holds ids the shards gave out, so it never changes an id.
        // PERF (perfplan4 R1): on the heap, not in the static TLS block.
        // glibc copies that block into each new thread on the thread that
        // starts it, and the cache was 64 KiB of its 99 KiB (`tsc -b` starts
        // 20 to 350 threads, many of which never intern).
        static CACHE: OnceCell<Box<Cache>> = const { OnceCell::new() };

        /// The ids this thread took from `NEXT` and did not use yet:
        /// (next, end).
        static IDS: Cell<(u32, u32)> = const { Cell::new((0, 0)) };
    }

    /// A new sequence number. The name id is `seq << 1 | internal`.
    // PERF: a thread takes `ID_BLOCK` numbers from `NEXT` at once, so
    // threads that intern new names do not contend on one atomic, and the
    // names of one file get close numbers (their `HASHES` and `TEXTS` slots
    // share cache lines). An id only has to be unique: ids already came in
    // thread race order, and no output orders by id (`Name` orders by
    // text). A thread that ends leaves its unused numbers as empty slots.
    fn next_id() -> u32 {
        IDS.with(|ids| {
            let (mut next, mut end) = ids.get();
            if next == end {
                next = NEXT.fetch_add(ID_BLOCK, Ordering::Relaxed);
                // `end` is below `SEQ_LIMIT`, so no id is `u32::MAX`.
                end = next
                    .checked_add(ID_BLOCK)
                    .filter(|&end| end < SEQ_LIMIT)
                    .expect("name id overflow");
            }
            ids.set((next + 1, end));
            next
        })
    }

    /// Fx hash of `s`. Symbol tables use it too.
    #[inline]
    pub(super) fn hash_str(s: &str) -> u64 {
        #[cfg(target_pointer_width = "64")]
        {
            let mut hasher = rustc_hash::FxHasher::default();
            hasher.write(s.as_bytes());
            hasher.finish()
        }
        #[cfg(not(target_pointer_width = "64"))]
        fx_hash_64(s.as_bytes())
    }

    /// `rustc_hash::FxHasher` (2.1.2) of `bytes` as it is on a 64-bit
    /// target. On a 32-bit target (wasm32) FxHasher gives a 32-bit hash,
    /// whose top 32 bits are 0. A shard map (`HashIsKey`) on a 32-bit
    /// target reads only the top 32 bits, so it needs this full hash.
    #[cfg(any(test, not(target_pointer_width = "64")))]
    fn fx_hash_64(bytes: &[u8]) -> u64 {
        const K: u64 = 0xf135_7aea_2e62_a9c5;
        const SEED1: u64 = 0x243f_6a88_85a3_08d3;
        const SEED2: u64 = 0x1319_8a2e_0370_7344;
        const PREVENT_TRIVIAL_ZERO_COLLAPSE: u64 = 0xa409_3822_299f_31d0;
        let multiply_mix = |x: u64, y: u64| {
            let full = u128::from(x).wrapping_mul(u128::from(y));
            (full as u64) ^ ((full >> 64) as u64)
        };
        let word = |b: &[u8]| u64::from_le_bytes(b.try_into().expect("8 bytes"));
        let len = bytes.len();
        let mut s0 = SEED1;
        let mut s1 = SEED2;
        if len <= 16 {
            if len >= 8 {
                s0 ^= word(&bytes[0..8]);
                s1 ^= word(&bytes[len - 8..]);
            } else if len >= 4 {
                s0 ^= u64::from(u32::from_le_bytes(bytes[0..4].try_into().expect("4 bytes")));
                s1 ^= u64::from(u32::from_le_bytes(
                    bytes[len - 4..].try_into().expect("4 bytes"),
                ));
            } else if len > 0 {
                s0 ^= u64::from(bytes[0]);
                s1 ^= (u64::from(bytes[len - 1]) << 8) | u64::from(bytes[len / 2]);
            }
        } else {
            let mut bulk = &bytes[..len - 1];
            while let Some((chunk, rest)) = bulk.split_first_chunk::<16>() {
                let t = multiply_mix(
                    s0 ^ word(&chunk[..8]),
                    PREVENT_TRIVIAL_ZERO_COLLAPSE ^ word(&chunk[8..]),
                );
                s0 = s1;
                s1 = t;
                bulk = rest;
            }
            let suffix = &bytes[len - 16..];
            s0 ^= word(&suffix[0..8]);
            s1 ^= word(&suffix[8..16]);
        }
        // `FxHasher::write` adds the byte hash, and `finish` rotates.
        let hash = multiply_mix(s0, s1) ^ (len as u64);
        hash.wrapping_mul(K).rotate_left(26)
    }

    /// Chunk and slot of `id`, by its sequence number `id >> 1`.
    #[inline]
    fn slot(id: u32) -> (usize, usize) {
        let seq = id >> 1;
        let v = (seq >> FIRST_CHUNK_SHIFT) + 1;
        let k = 31 - v.leading_zeros();
        let start = ((1u32 << k) - 1) << FIRST_CHUNK_SHIFT;
        (k as usize, (seq - start) as usize)
    }

    /// Shard of a text with `hash_str` hash `hash`.
    #[inline]
    fn shard_index(hash: u64) -> usize {
        (hash >> 59) as usize % SHARDS
    }

    /// Records the text and table hash of new id `id`.
    fn set_slot(id: u32, hash: u64, stored: &'static str) {
        set_hash(id, hash);
        let (chunk, index) = slot(id);
        let chunk_len = 1usize << (FIRST_CHUNK_SHIFT as usize + chunk);
        let slots = TEXTS[chunk].get_or_init(|| (0..chunk_len).map(|_| OnceLock::new()).collect());
        let _ = slots[index].set(stored);
    }

    /// Records the table hash of id `id`, from its `hash_str` hash `hash`.
    /// Writes only a slot that does not hold it yet, so threads that find
    /// the same lib name do not write one cache line in turn.
    #[inline]
    fn set_hash(id: u32, hash: u64) {
        let (chunk, index) = slot(id);
        let chunk_len = 1usize << (FIRST_CHUNK_SHIFT as usize + chunk);
        let hashes =
            HASHES[chunk].get_or_init(|| (0..chunk_len).map(|_| AtomicU32::new(0)).collect());
        let folded = super::fold_hash(hash);
        if hashes[index].load(Ordering::Relaxed) != folded {
            hashes[index].store(folded, Ordering::Relaxed);
        }
    }

    /// The text of lib name `i` (`lib_names`).
    #[inline]
    fn lib_text(i: usize) -> &'static str {
        &lib_names::TEXT[lib_names::OFFSETS[i] as usize..lib_names::OFFSETS[i + 1] as usize]
    }

    /// The index of `s` in the lib name table, or `None`. `hash` is
    /// `hash_str(s)`. The result depends only on `s`, so a text in the table
    /// always gets the same id and never reaches the shards.
    // PERF: most probes miss (a name that no lib file has), so a name of
    // another length is skipped with the offsets alone, and byte slices
    // skip the `str` char boundary checks, which read the text.
    #[inline]
    fn find_lib_name(s: &str, hash: u64) -> Option<usize> {
        let bucket = (hash as usize) & ((1usize << lib_names::BUCKET_BITS) - 1);
        let start = lib_names::BUCKETS[bucket] as usize;
        let end = lib_names::BUCKETS[bucket + 1] as usize;
        let text = lib_names::TEXT.as_bytes();
        (start..end).find(|&i| {
            let from = lib_names::OFFSETS[i] as usize;
            let to = lib_names::OFFSETS[i + 1] as usize;
            to - from == s.len() && &text[from..to] == s.as_bytes()
        })
    }

    /// The id and text of `s` when it is a lib name. The id needs no lock
    /// and no text copy. Its table hash is recorded before the id is
    /// returned, as `intern_shared` does.
    #[inline]
    fn lib_name(s: &str, hash: u64) -> Option<(u32, &'static str)> {
        let i = find_lib_name(s, hash)?;
        // Lib names are ASCII, so none starts with the internal prefix and
        // the internal bit stays 0.
        let id = (LIB_FIRST_SEQ + i as u32) << 1;
        set_hash(id, hash);
        Some((id, lib_text(i)))
    }

    /// The shard locks, made on first use with "default" at `DEFAULT_ID`.
    /// "default" is not a lib name (the generator leaves it out), so the
    /// `DEFAULT_ID` name comes only from `intern_shared`, which calls this
    /// first, and the text of `DEFAULT_ID` is always set.
    fn shards() -> &'static [PaddedShard; SHARDS] {
        SHARD_LOCKS.get_or_init(|| {
            let mut shards: [PaddedShard; SHARDS] = std::array::from_fn(|_| {
                PaddedShard(Mutex::new(Shard {
                    ids: HashMap::with_capacity_and_hasher(SHARD_CAPACITY, Default::default()),
                    collisions: FxHashMap::with_hasher(FxBuildHasher),
                    free: Default::default(),
                }))
            });
            let text: &'static str = crate::ast::INTERNAL_SYMBOL_NAME_DEFAULT;
            debug_assert!(!text.starts_with(crate::ast::INTERNAL_SYMBOL_NAME_PREFIX));
            let hash = hash_str(text);
            set_slot(DEFAULT_ID, hash, text);
            shards[shard_index(hash)]
                .0
                .get_mut()
                .unwrap_or_else(PoisonError::into_inner)
                .ids
                .insert(hash, (text, DEFAULT_ID));
            shards
        })
    }

    /// The text of name id `id`.
    #[inline]
    pub(super) fn text(id: u32) -> &'static str {
        if id == 0 {
            return "";
        }
        // A lib name reads the static table; its `TEXTS` slot stays empty.
        let lib = (id >> 1).wrapping_sub(LIB_FIRST_SEQ) as usize;
        if lib < lib_names::COUNT {
            return lib_text(lib);
        }
        let (chunk, index) = slot(id);
        TEXTS[chunk]
            .get()
            .and_then(|slots| slots[index].get())
            .copied()
            .expect("unknown name id")
    }

    /// The lib name index of name id `id`, or `None` when `id` is not a lib
    /// name id.
    #[inline]
    fn lib_index(id: u32) -> Option<usize> {
        let lib = (id >> 1).wrapping_sub(LIB_FIRST_SEQ) as usize;
        (id & INTERNAL_BIT == 0 && lib < lib_names::COUNT).then_some(lib)
    }

    /// True for the ids that do not depend on intern order: 0 (""),
    /// `DEFAULT_ID` and the lib name ids (see `Name::stable_id`).
    #[inline]
    pub(super) fn is_stable(id: u32) -> bool {
        id == 0 || id == DEFAULT_ID || lib_index(id).is_some()
    }

    /// The name of stable id `id`, as `intern` of its text gives it: a lib
    /// name gets its table hash recorded (`lib_name`), and "default" goes
    /// through the shards, which intern it first.
    pub(super) fn stable(id: u32) -> Option<Name> {
        if id == 0 {
            return Some(Name(0));
        }
        if id == DEFAULT_ID {
            return Some(intern(crate::ast::INTERNAL_SYMBOL_NAME_DEFAULT));
        }
        let lib = lib_index(id)?;
        set_hash(id, hash_str(lib_text(lib)));
        Some(Name(id))
    }

    /// The table hash of the text of name id `id`.
    #[inline]
    pub(super) fn table_hash(id: u32) -> u32 {
        if id == 0 {
            return super::fold_hash(hash_str(""));
        }
        let (chunk, index) = slot(id);
        // Relaxed is enough: a thread gets `id` from `intern_shared` under
        // the shard lock, from `lib_name` after its own store, or through a
        // later handoff, and each orders a store of this hash before this
        // load.
        HASHES[chunk].get().expect("unknown name id")[index].load(Ordering::Relaxed)
    }

    /// The name for `s`, created on first use.
    pub(super) fn intern(s: &str) -> Name {
        if s.is_empty() {
            return Name(0);
        }
        let hash = hash_str(s);
        let set = (hash as usize) & (CACHE_SETS - 1);
        // After the thread-local destructors ran (`try_with` fails), a
        // name goes to the shared map without the cache.
        let cached = CACHE.try_with(|cache| {
            let [first, second] = &cache.get_or_init(new_cache)[set].0;
            let (h, id, stored) = first.get();
            if id != 0 && h == hash && stored == s {
                return Some(id);
            }
            let entry = second.get();
            let (h, id, stored) = entry;
            if id != 0 && h == hash && stored == s {
                // Least recently used goes second.
                second.set(first.get());
                first.set(entry);
                return Some(id);
            }
            None
        });
        if let Ok(Some(id)) = cached {
            return Name(id);
        }
        // PERF: bind B2. A lib name (lib.dom alone brings about 7k names
        // that no file interned before) gets its id from the static table,
        // with no shard lock, map insert, text copy or `OnceLock` set. The
        // table is probed before the shards on every miss, so a text never
        // gets both a table id and a shard id. Ids are never ordered or
        // printed (see `next_id`), so the reserved range changes no output.
        let (id, stored) = match lib_name(s, hash) {
            Some(found) => found,
            None => intern_shared(s, hash),
        };
        let _ = CACHE.try_with(|cache| {
            if let Some(cache) = cache.get() {
                let [first, second] = &cache[set].0;
                second.set(first.get());
                first.set((hash, id, stored));
            }
        });
        Name(id)
    }

    /// The id and stored text of `s`, from the shared map.
    fn intern_shared(s: &str, hash: u64) -> (u32, &'static str) {
        let mut shard = shards()[shard_index(hash)]
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let hash_taken = match shard.ids.get(&hash) {
            Some(&(stored, id)) if stored == s => return (id, stored),
            Some(_) => {
                if let Some((&stored, &id)) = shard.collisions.get_key_value(s) {
                    return (id, stored);
                }
                true
            }
            None => false,
        };
        if shard.free.len() < s.len() {
            shard.free = Box::leak(vec![0u8; BLOCK.max(s.len())].into_boxed_slice());
        }
        let (head, tail) = std::mem::take(&mut shard.free).split_at_mut(s.len());
        head.copy_from_slice(s.as_bytes());
        shard.free = tail;
        let head: &'static [u8] = head;
        let stored = std::str::from_utf8(head).expect("interned text is UTF-8");
        let internal = stored.starts_with(crate::ast::INTERNAL_SYMBOL_NAME_PREFIX);
        // `INTERNAL_BIT` is the low bit.
        let id = (next_id() << 1) | u32::from(internal);
        set_slot(id, hash, stored);
        if hash_taken {
            shard.collisions.insert(stored, id);
        } else {
            shard.ids.insert(hash, (stored, id));
        }
        (id, stored)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// `fx_hash_64` gives the 64-bit FxHasher values of rustc-hash
        /// 2.1.2's own `tests::bytes`, and the 64-bit `hash_str` for texts
        /// of each length class of its byte hash.
        #[test]
        fn fx_hash_64_matches_fx_hasher() {
            for (bytes, want) in [
                (&b""[..], 17_606_491_139_363_777_937),
                (b"\x00", 5_448_590_020_104_574_886),
                (b"\x00\x00\x00\x00\x00\x00", 16_766_921_560_080_789_783),
                (b"\x01", 5_922_447_956_811_044_110),
                (b"\x02", 5_229_781_508_510_959_783),
                (b"uwu", 7_168_164_714_682_931_527),
                (
                    b"These are some bytes for testing rustc_hash.",
                    2_349_210_501_944_688_211,
                ),
            ] {
                assert_eq!(fx_hash_64(bytes), want, "{bytes:?}");
            }
            #[cfg(target_pointer_width = "64")]
            for len in 0..40u32 {
                let text: String = (0..len)
                    .map(|i| char::from(b'a' + (i * 7 % 26) as u8))
                    .collect();
                assert_eq!(fx_hash_64(text.as_bytes()), hash_str(&text), "{text:?}");
            }
        }

        /// Every lib name is found at its own index. A failure after a
        /// rustc-hash update means the `hash_str` copy in
        /// `scripts/gen-lib-names.py` is out of date: fix it and run it.
        #[test]
        fn lib_names_find_themselves() {
            let mut seen = rustc_hash::FxHashSet::default();
            for i in 0..lib_names::COUNT {
                let text = lib_text(i);
                assert!(!text.is_empty() && text.is_ascii(), "{text:?}");
                assert_ne!(text, crate::ast::INTERNAL_SYMBOL_NAME_DEFAULT);
                assert!(seen.insert(text), "lib name {text:?} twice");
                assert_eq!(find_lib_name(text, hash_str(text)), Some(i), "{text:?}");
            }
            assert_eq!(
                lib_names::OFFSETS[lib_names::COUNT] as usize,
                lib_names::TEXT.len()
            );
            assert_eq!(
                lib_names::BUCKETS[1usize << lib_names::BUCKET_BITS] as usize,
                lib_names::COUNT
            );
        }

        /// A lib name gets a table id, a text and a table hash like any other
        /// name, and one id per text.
        #[test]
        fn lib_name_ids() {
            let lib = Name::from("addEventListener");
            assert!(((lib.0 >> 1) - LIB_FIRST_SEQ) < lib_names::COUNT as u32);
            assert!(!lib.is_internal());
            assert_eq!(lib.as_str(), "addEventListener");
            assert_eq!(
                lib.table_hash(),
                super::super::table_hash("addEventListener")
            );
            assert_eq!(Name::from(String::from("addEventListener")).0, lib.0);
            let other = Name::from("notALibName_u5");
            assert!((other.0 >> 1) >= LIB_FIRST_SEQ + lib_names::COUNT as u32);
            assert_eq!(other.as_str(), "notALibName_u5");
            assert!(Name::from("default").is_default_symbol_name());
        }

        /// "", "default" and the lib names have stable ids, and a name made
        /// from a stable id equals the interned name.
        #[test]
        fn stable_ids() {
            for text in ["", "default", "addEventListener"] {
                let name = Name::from(text);
                let id = name.stable_id().expect(text);
                let loaded = Name::from_stable_id(id).expect(text);
                assert_eq!(loaded, name);
                assert_eq!(loaded.table_hash(), super::super::table_hash(text));
            }
            assert_eq!(Name::from("notALibName_u6").stable_id(), None);
            assert!(Name::from_stable_id(u32::MAX).is_none());
        }
    }
}

/// `Symbol::declarations` (Go `[]*ast.Node`). An empty list allocates
/// nothing and a single declaration is stored inline. Longer lists share one
/// `Vec` between clones; the first write copies it, so each symbol still owns
/// its own list, like a Go slice that is copied before an append. Program
/// symbols keep longer lists in a leaked `Vec` (`make_static`), with the same
/// copy on the first write.
// PERF (memper1): every list is behind a thin pointer, so a `Declarations`
// is 16 bytes and a `Symbol` 56 (a leaked `&'static [Node]` made both 8
// bytes larger). A static list reads through its `Vec` header, as a shared
// list reads through its `Arc`.
#[derive(Clone, Default)]
pub struct Declarations(DeclarationList);

#[derive(Clone, Default)]
enum DeclarationList {
    #[default]
    Empty,
    One(Node),
    Many(Arc<Vec<Node>>),
    Static(&'static Vec<Node>),
}

impl Declarations {
    /// Go `append(declarations, node)`.
    pub fn push(&mut self, node: Node) {
        match &mut self.0 {
            DeclarationList::Empty => self.0 = DeclarationList::One(node),
            DeclarationList::One(first) => {
                self.0 = DeclarationList::Many(Arc::new(vec![*first, node]));
            }
            DeclarationList::Many(list) => Arc::make_mut(list).push(node),
            DeclarationList::Static(list) => {
                let mut owned = Vec::with_capacity(list.len() + 1);
                owned.extend_from_slice(*list);
                owned.push(node);
                self.0 = DeclarationList::Many(Arc::new(owned));
            }
        }
    }

    /// Moves a `Many` list into a leaked `Vec`. Call it only for symbols
    /// that live until exit (program symbols, like the AST).
    // PERF: a clone of a `Static` list copies a pointer. A clone of a `Many`
    // list changes a reference count that the checker threads share
    // (`instantiate_symbol`, `clone_symbol`, member code).
    fn make_static(&mut self) {
        if matches!(self.0, DeclarationList::Many(_)) {
            if let DeclarationList::Many(list) = std::mem::take(&mut self.0) {
                self.0 = DeclarationList::Static(Box::leak(Box::new(Arc::unwrap_or_clone(list))));
            }
        }
    }
}

impl std::ops::Deref for Declarations {
    type Target = [Node];
    #[inline]
    fn deref(&self) -> &[Node] {
        match &self.0 {
            DeclarationList::Empty => &[],
            DeclarationList::One(node) => std::slice::from_ref(node),
            DeclarationList::Many(list) => list,
            DeclarationList::Static(list) => list,
        }
    }
}

impl std::ops::DerefMut for Declarations {
    fn deref_mut(&mut self) -> &mut [Node] {
        if let DeclarationList::Static(list) = self.0 {
            // The first write copies the list, so the symbol owns it.
            self.0 = DeclarationList::Many(Arc::new(list.to_vec()));
        }
        match &mut self.0 {
            DeclarationList::Empty => &mut [],
            DeclarationList::One(node) => std::slice::from_mut(node),
            DeclarationList::Many(list) => Arc::make_mut(list).as_mut_slice(),
            DeclarationList::Static(_) => unreachable!("static list was just copied"),
        }
    }
}

impl std::fmt::Debug for Declarations {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(&**self, f)
    }
}

impl From<Vec<Node>> for Declarations {
    fn from(v: Vec<Node>) -> Self {
        Declarations(match v.as_slice() {
            [] => DeclarationList::Empty,
            [node] => DeclarationList::One(*node),
            _ => DeclarationList::Many(Arc::new(v)),
        })
    }
}

impl From<Declarations> for Vec<Node> {
    fn from(d: Declarations) -> Self {
        match d.0 {
            DeclarationList::Empty => Vec::new(),
            DeclarationList::One(node) => vec![node],
            DeclarationList::Many(list) => Arc::unwrap_or_clone(list),
            DeclarationList::Static(list) => list.to_vec(),
        }
    }
}

/// By-value iterator over `Declarations`. It reads a shared or static list
/// in place.
pub struct DeclarationsIntoIter {
    list: Declarations,
    next: usize,
}

impl Iterator for DeclarationsIntoIter {
    type Item = Node;
    #[inline]
    fn next(&mut self) -> Option<Node> {
        let node = self.list.get(self.next).copied()?;
        self.next += 1;
        Some(node)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let left = self.list.len() - self.next;
        (left, Some(left))
    }
}

impl ExactSizeIterator for DeclarationsIntoIter {}

impl IntoIterator for Declarations {
    type Item = Node;
    type IntoIter = DeclarationsIntoIter;
    fn into_iter(self) -> Self::IntoIter {
        DeclarationsIntoIter {
            list: self,
            next: 0,
        }
    }
}

impl<'a> IntoIterator for &'a Declarations {
    type Item = &'a Node;
    type IntoIter = std::slice::Iter<'a, Node>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

/// Go `*ast.Symbol`. Field names follow Go.
#[derive(Clone, Debug, Default)]
pub struct Symbol {
    pub flags: SymbolFlags,
    pub check_flags: CheckFlags,
    pub name: Name,
    pub declarations: Declarations,
    pub value_declaration: Node,
    pub members: SymbolTable,
    pub exports: SymbolTable,
    pub parent: SymbolId,
    pub export_symbol: SymbolId,
}

// PERF (memper1): a checker makes millions of symbols, in chunks of
// `COW_CHUNK_LEN` (14 KiB at 56 bytes, a jemalloc size class).
#[cfg(target_pointer_width = "64")]
const _: () = assert!(std::mem::size_of::<Symbol>() == 56);

/// A growable array split into fixed-size chunks that clones share. Index
/// `i` is value `i % COW_CHUNK_LEN` of chunk `i / COW_CHUNK_LEN`. The values
/// after the last chunk are the tail, one `Vec` of at most one chunk. A
/// chunk is owned by this array (written without an atomic operation),
/// static (a chunk that lives until exit, `freeze_from`), shared with clones
/// by `Arc` (`share_from`) or freed (`free_chunks`). A clone copies the owned
/// chunks and the tail, and shares the others. The first write to a static
/// or full shared chunk takes it back (a copy, unless no clone uses a shared
/// chunk).
///
/// An owned or static chunk is always full. Only a shared chunk can hold
/// fewer values: the tail that `share_from` or `end_chunk` shared. A write
/// to it keeps it shared, and the next push takes it back as the tail.
/// So a read at or above the length panics (index out of bounds), as a
/// `Vec` read does.
///
/// Holes (lsshells M3d): `end_chunk` moves the length to the next chunk
/// start, so the indexes left in the last chunk hold no value, and
/// `free_chunks` frees whole chunks. A hole is in a shared chunk that holds
/// only the values before it, or in a freed chunk, so a read of a hole
/// panics. Indexes never move and are never used again.
///
/// Interleaved arrays (apisym1c, `new_interleaved`): the binder lineage
/// puts its values in the even chunks only, and a checker copy of it
/// (`for_checker`) puts its own values in the odd chunks only. A sealed
/// chunk is followed by a freed chunk, so the tail keeps its parity. So a
/// later lineage index is never one of a checker's own, and a checker can
/// add the chunks that the lineage bound after its copy (`catch_up`), as a
/// Go checker reads every bound file. In a checker copy, the lineage chunk
/// that held the copy's last values is an owned chunk padded with default
/// values: its indexes after those values read as default, not as holes.
// PERF: an owned or static chunk is a fixed array behind a thin pointer at
// the same place in the chunk entry, so `get` of one is one bounds test (the
// chunk list), one kind test and one load, with no branch between the two
// kinds and no pointer hop. A read of the tail is one bounds test more (its
// `Vec`), and that test also stops a read at or above the length. Shared and
// freed chunks (only in the language service) go out of line.
// PERF (apisym1c): the parity is in the index, so `get` does not select a
// part (a second array cost 0.8 to 2.5 percent instructions in a check).
#[derive(Clone, Debug)]
pub struct CowChunks<T: 'static> {
    chunks: Vec<Chunk<T>>,
    /// The values from index `chunks.len() * COW_CHUNK_LEN` on. Empty, with
    /// no capacity, while the length is inside the last chunk (a partial
    /// shared chunk).
    tail: Vec<T>,
    len: usize,
    /// Each sealed chunk is followed by a freed chunk (`new_interleaved`).
    interleaved: bool,
}

const COW_CHUNK_SHIFT: usize = 8;
const COW_CHUNK_LEN: usize = 1 << COW_CHUNK_SHIFT;
const COW_CHUNK_MASK: usize = COW_CHUNK_LEN - 1;

type ChunkValues<T> = [T; COW_CHUNK_LEN];

#[derive(Clone, Debug)]
enum Chunk<T: 'static> {
    /// Read in place (`get` tests for this kind once). Always full.
    Fixed(Fixed<T>),
    /// Only the first `len()` indexes hold values.
    Shared(Arc<Vec<T>>),
    Freed,
}

// PERF: the two kinds keep the pointer at the same place, so `get` reads
// both with one load and no branch.
#[derive(Clone, Debug)]
enum Fixed<T: 'static> {
    Owned(Box<ChunkValues<T>>),
    /// A leaked full chunk (`CowChunks::freeze_from`).
    Static(&'static ChunkValues<T>),
}

/// An owned chunk of `values`, which must be exactly `COW_CHUNK_LEN`. No
/// copy when their capacity is one chunk.
fn full_chunk<T>(values: Vec<T>) -> Chunk<T> {
    let values: Box<ChunkValues<T>> = values
        .into_boxed_slice()
        .try_into()
        .unwrap_or_else(|_| panic!("a fixed chunk holds {COW_CHUNK_LEN} values"));
    Chunk::Fixed(Fixed::Owned(values))
}

impl<T> Fixed<T> {
    #[inline(always)]
    fn values(&self) -> &ChunkValues<T> {
        match self {
            Fixed::Owned(values) => values,
            Fixed::Static(values) => values,
        }
    }
}

impl<T: Clone> Chunk<T> {
    /// Value `k` for a write. Takes the chunk back first if it is not owned.
    #[inline]
    fn value_mut(&mut self, k: usize) -> &mut T {
        if !matches!(self, Chunk::Fixed(Fixed::Owned(_))) {
            return self.take_back_value(k);
        }
        match self {
            Chunk::Fixed(Fixed::Owned(values)) => &mut values[k],
            _ => unreachable!("chunk was just tested"),
        }
    }

    /// `value_mut` of a chunk that is not owned. A static or full shared
    /// chunk becomes owned. A partial shared chunk stays shared (a copy when
    /// a clone still uses it), so its holes still panic.
    #[cold]
    #[inline(never)]
    fn take_back_value(&mut self, k: usize) -> &mut T {
        if !matches!(self, Chunk::Shared(values) if values.len() < COW_CHUNK_LEN) {
            *self = match std::mem::replace(self, Chunk::Freed) {
                Chunk::Fixed(Fixed::Static(values)) => full_chunk(values.to_vec()),
                Chunk::Shared(values) => full_chunk(Arc::unwrap_or_clone(values)),
                Chunk::Fixed(Fixed::Owned(values)) => Chunk::Fixed(Fixed::Owned(values)),
                Chunk::Freed => panic!("write to a freed chunk"),
            };
        }
        match self {
            Chunk::Fixed(Fixed::Owned(values)) => &mut values[k],
            Chunk::Shared(values) => &mut Arc::make_mut(values)[k],
            _ => unreachable!("chunk was just taken back"),
        }
    }

    /// The values, moved out (or cloned from a chunk that a clone still
    /// uses).
    fn into_values(self) -> Vec<T> {
        match self {
            Chunk::Fixed(Fixed::Owned(values)) => Vec::from(values as Box<[T]>),
            Chunk::Fixed(Fixed::Static(values)) => values.to_vec(),
            Chunk::Shared(values) => Arc::unwrap_or_clone(values),
            Chunk::Freed => Vec::new(),
        }
    }

    fn has_values(&self) -> bool {
        match self {
            Chunk::Fixed(_) => true,
            Chunk::Shared(values) => !values.is_empty(),
            Chunk::Freed => false,
        }
    }
}

impl<T: Clone> CowChunks<T> {
    #[must_use]
    pub fn new() -> Self {
        Self {
            chunks: Vec::new(),
            tail: Vec::new(),
            len: 0,
            interleaved: false,
        }
    }

    /// An empty interleaved array (see `CowChunks`): the values of the
    /// binder lineage, in the even chunks.
    #[must_use]
    pub fn new_interleaved() -> Self {
        Self {
            interleaved: true,
            ..Self::new()
        }
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.len
    }

    /// The index that the next push gets: the length, or in an interleaved
    /// array whose tail is full, the start of the chunk after the freed
    /// chunk that the push puts after the tail (`seal_full_tail`).
    #[must_use]
    pub fn next_index(&self) -> usize {
        if self.interleaved && self.tail.len() == COW_CHUNK_LEN {
            self.len + COW_CHUNK_LEN
        } else {
            self.len
        }
    }

    /// The number of indexes below `next_index` that the array's parity
    /// holds: in an interleaved binder array, the lineage indexes (even
    /// chunks), as `ArenaOffsets` counts them; else `next_index`.
    #[must_use]
    pub fn dense_len(&self) -> usize {
        let next = self.next_index();
        if self.interleaved {
            next - ((next >> (COW_CHUNK_SHIFT + 1)) << COW_CHUNK_SHIFT)
        } else {
            next
        }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The index of the first tail value.
    #[inline(always)]
    fn tail_start(&self) -> usize {
        self.chunks.len() << COW_CHUNK_SHIFT
    }

    // PERF: the value is written into the tail inline when it has room, and
    // the out-of-line `make_room` takes no value. So an inlined caller builds
    // the value in its slot and does not copy it from the stack.
    #[inline(always)]
    pub fn push(&mut self, value: T) {
        if self.tail.len() == self.tail.capacity() {
            self.make_room();
        }
        self.tail.push(value);
        self.len += 1;
    }

    /// `push` when the tail has no room: takes a partial shared last chunk
    /// back as the tail, makes a full tail a chunk, and grows the tail (as
    /// `Vec::push` does while the array has no chunk, else to one chunk).
    #[cold]
    #[inline(never)]
    fn make_room(&mut self) {
        if self.len < self.tail_start() {
            self.take_back_tail();
        }
        self.seal_full_tail();
        let room = if self.chunks.is_empty() {
            (self.tail.capacity() * 2).clamp(4, COW_CHUNK_LEN)
        } else {
            COW_CHUNK_LEN
        };
        self.tail.reserve_exact(room - self.tail.len());
    }

    /// Moves the values of the last chunk back into the tail: a partial
    /// shared chunk, which holds the length (a copy when a clone still uses
    /// it).
    fn take_back_tail(&mut self) {
        debug_assert!(self.tail.is_empty(), "a tail after a partial chunk");
        self.tail = match self.chunks.pop() {
            Some(Chunk::Shared(values)) => Arc::unwrap_or_clone(values),
            _ => panic!("write to a freed chunk"),
        };
    }

    /// Makes a full tail an owned chunk. In an interleaved array a freed
    /// chunk follows it, and the length moves to the chunk after that.
    fn seal_full_tail(&mut self) {
        if self.tail.len() == COW_CHUNK_LEN {
            let values = std::mem::take(&mut self.tail);
            self.chunks.push(full_chunk(values));
            if self.interleaved {
                self.chunks.push(Chunk::Freed);
                self.len = self.tail_start();
            }
        }
    }

    /// Makes room for `additional` more values, so the pushes that follow
    /// do not grow step by step: in the tail up to one chunk while the array
    /// has no chunk, and in the chunk list when the values need more than
    /// one chunk. Capacity only.
    // PERF: bind D. A file arena started with a one-value first chunk,
    // which `push` grew by doubling for every bound file. The tail gets at
    // most one chunk of room: the chunks after it free one by one in
    // `into_aligned`, so a large file arena (a lib snapshot load) does not
    // hold its whole buffer while its values move.
    pub fn reserve(&mut self, additional: usize) {
        if self.chunks.is_empty() {
            let room = COW_CHUNK_LEN.saturating_sub(self.tail.len());
            self.tail.reserve_exact(additional.min(room));
        }
        if self.len + additional > COW_CHUNK_LEN {
            let chunks = (self.len + additional) >> COW_CHUNK_SHIFT;
            self.chunks
                .reserve(chunks.saturating_sub(self.chunks.len()));
        }
    }

    #[inline(always)]
    #[must_use]
    pub fn get(&self, i: usize) -> &T {
        match self.chunks.get(i >> COW_CHUNK_SHIFT) {
            Some(Chunk::Fixed(values)) => &values.values()[i & COW_CHUNK_MASK],
            Some(_) => self.get_other(i),
            None => &self.tail[i - self.tail_start()],
        }
    }

    /// `get` from a shared or freed chunk.
    #[cold]
    #[inline(never)]
    fn get_other(&self, i: usize) -> &T {
        match &self.chunks[i >> COW_CHUNK_SHIFT] {
            Chunk::Shared(values) => &values[i & COW_CHUNK_MASK],
            _ => panic!("read of freed index {i}"),
        }
    }

    /// Takes the chunk back first if it is not owned.
    #[inline]
    pub fn get_mut(&mut self, i: usize) -> &mut T {
        let tail_start = self.tail_start();
        match self.chunks.get_mut(i >> COW_CHUNK_SHIFT) {
            Some(chunk) => chunk.value_mut(i & COW_CHUNK_MASK),
            None => &mut self.tail[i - tail_start],
        }
    }

    /// Makes the owned chunks that hold values from index `from` on shared,
    /// so clones copy none of them. The tail becomes a shared chunk too when
    /// `from` is in or before its chunk. Pass 0 to share every chunk. Static
    /// and freed chunks stay as they are.
    pub fn share_from(&mut self, from: usize) {
        self.seal_full_tail();
        let first = from >> COW_CHUNK_SHIFT;
        for chunk in self.chunks.iter_mut().skip(first) {
            if matches!(chunk, Chunk::Fixed(Fixed::Owned(_))) {
                let values = std::mem::replace(chunk, Chunk::Freed).into_values();
                *chunk = Chunk::Shared(Arc::new(values));
            }
        }
        if !self.tail.is_empty() && first <= self.chunks.len() {
            let values = std::mem::take(&mut self.tail);
            self.chunks.push(Chunk::Shared(Arc::new(values)));
        }
    }

    /// Leaks the owned chunks from index `from` on, so clones share them
    /// with no copy and no reference count. Only for values that live until
    /// exit, which are never freed (`free_chunks`). A later write to one
    /// copies it (`Chunk::value_mut`) and leaves the leaked chunk. The tail
    /// stays owned until it is full.
    pub fn freeze_from(&mut self, from: usize) {
        self.seal_full_tail();
        for chunk in self.chunks.iter_mut().skip(from >> COW_CHUNK_SHIFT) {
            if matches!(chunk, Chunk::Fixed(Fixed::Owned(_))) {
                let Chunk::Fixed(Fixed::Owned(values)) = std::mem::replace(chunk, Chunk::Freed)
                else {
                    unreachable!("chunk was just tested")
                };
                *chunk = Chunk::Fixed(Fixed::Static(Box::leak(values)));
            }
        }
    }

    /// Moves the length to the start of the next chunk, so the next value
    /// starts a new chunk. The indexes left in the last chunk hold no value:
    /// the tail becomes a shared chunk that holds its values only. Does
    /// nothing when the length is at a chunk start.
    ///
    /// In an interleaved array the next value starts the next chunk of the
    /// same parity: a freed chunk follows the ended one, and a full tail is
    /// sealed now.
    pub fn end_chunk(&mut self) {
        if self.len & COW_CHUNK_MASK == 0 {
            if self.interleaved {
                self.seal_full_tail();
            }
            return;
        }
        self.share_from(self.len);
        self.len = self.len.next_multiple_of(COW_CHUNK_LEN);
        if self.interleaved {
            self.chunks.push(Chunk::Freed);
            self.len += COW_CHUNK_LEN;
        }
    }

    /// Frees the chunks of the indexes from `from` to `to`, which are chunk
    /// starts (`end_chunk`). A read of one of these indexes panics and later
    /// indexes do not move. A clone that shares a freed chunk keeps its
    /// values until it drops. In an interleaved array only the even chunks
    /// (the lineage's) are freed: an odd chunk is a checker's own.
    pub fn free_chunks(&mut self, from: usize, to: usize) {
        debug_assert!(
            from & COW_CHUNK_MASK == 0 && to & COW_CHUNK_MASK == 0,
            "freed indexes {from}..{to} are not whole chunks"
        );
        // A tail that pushes filled to a chunk end (`end_chunk` does
        // nothing there) is a chunk that the range can hold.
        self.seal_full_tail();
        let end = (to >> COW_CHUNK_SHIFT).min(self.chunks.len());
        let first = from.div_ceil(COW_CHUNK_LEN).min(end);
        let interleaved = self.interleaved;
        for (position, chunk) in self.chunks.iter_mut().enumerate().take(end).skip(first) {
            if !interleaved || position % 2 == 0 {
                *chunk = Chunk::Freed;
            }
        }
    }

    /// The number of chunks that hold values (freed chunks do not count),
    /// with the tail.
    #[must_use]
    pub fn live_chunks(&self) -> usize {
        usize::from(!self.tail.is_empty())
            + self
                .chunks
                .iter()
                .filter(|chunk| chunk.has_values())
                .count()
    }

    /// Moves the values from index `skip` on into chunks for `append_aligned`
    /// at index `at` of another array, and runs `f` on each value first, in
    /// order. The chunks start where the chunks of that array start, so the
    /// append moves whole chunks and moves no value, except the values that
    /// fill a partial tail there (`head`).
    // PERF: a bind thread does the id remap (`f`) and the moves here, so
    // the loading thread only appends chunks (`SymbolArena::append_file_arena`).
    pub fn into_aligned(
        mut self,
        skip: usize,
        at: usize,
        mut f: impl FnMut(&mut T),
    ) -> AlignedChunks<T> {
        debug_assert!(!self.interleaved, "a file arena is not interleaved");
        let len = self.len.saturating_sub(skip);
        let room = (COW_CHUNK_LEN - (at & COW_CHUNK_MASK)) & COW_CHUNK_MASK;
        let head_len = room.min(len);
        let mut head = Vec::with_capacity(head_len);
        let mut chunks = Vec::with_capacity((len - head_len) >> COW_CHUNK_SHIFT);
        let mut tail = Vec::new();
        let mut add = |mut value: T| {
            f(&mut value);
            if head.len() < room {
                head.push(value);
                return;
            }
            if tail.capacity() == 0 {
                tail.reserve_exact(COW_CHUNK_LEN);
            }
            tail.push(value);
            if tail.len() == COW_CHUNK_LEN {
                chunks.push(full_chunk(std::mem::take(&mut tail)));
            }
        };
        std::mem::take(&mut self.chunks)
            .into_iter()
            .flat_map(Chunk::into_values)
            .chain(std::mem::take(&mut self.tail))
            .skip(skip)
            .for_each(&mut add);
        debug_assert_eq!(head.len(), head_len, "aligned head length");
        debug_assert_eq!(
            chunks.len(),
            (len - head_len) >> COW_CHUNK_SHIFT,
            "aligned chunk count"
        );
        AlignedChunks {
            at,
            len,
            head,
            chunks,
            tail,
        }
    }

    /// Appends the values of `aligned`, which `into_aligned` made for the
    /// current length (`dense_len`). In an interleaved array a freed chunk
    /// follows each of its chunks.
    pub fn append_aligned(&mut self, aligned: AlignedChunks<T>) {
        assert_eq!(
            self.dense_len(),
            aligned.at,
            "aligned chunks made for another index"
        );
        if self.len < self.tail_start() {
            self.take_back_tail();
        }
        // The head fills the tail up to a chunk end at most.
        self.tail.reserve_exact(aligned.head.len());
        self.tail.extend(aligned.head);
        if !aligned.chunks.is_empty() || !aligned.tail.is_empty() {
            self.seal_full_tail();
            debug_assert!(self.tail.is_empty(), "aligned chunks after a partial tail");
            if self.interleaved {
                self.chunks.reserve(2 * aligned.chunks.len());
                for chunk in aligned.chunks {
                    self.chunks.extend([chunk, Chunk::Freed]);
                }
            } else {
                self.chunks.extend(aligned.chunks);
            }
            self.tail = aligned.tail;
        }
        self.len = self.tail_start() + self.tail.len();
    }

    /// The values that chunk `position` holds: the tail at the chunk after
    /// the last, and none after it.
    fn chunk_values(&self, position: usize) -> &[T] {
        match self.chunks.get(position) {
            Some(Chunk::Fixed(values)) => values.values(),
            Some(Chunk::Shared(values)) => values,
            Some(Chunk::Freed) => &[],
            None if position == self.chunks.len() => &self.tail,
            None => &[],
        }
    }

    /// The indexes of the values in the odd chunks, the tail too when it is
    /// in one, in order: the own values of a checker copy (`for_checker`).
    pub fn odd_indexes(&self) -> impl Iterator<Item = usize> + '_ {
        (1..=self.chunks.len())
            .step_by(2)
            .flat_map(move |position| {
                let start = position << COW_CHUNK_SHIFT;
                start..start + self.chunk_values(position).len()
            })
    }
}

/// An owned chunk that holds `values` (at most one chunk) and default
/// values after them (see `CowChunks`).
fn padded_chunk<T: Clone + Default>(values: &[T]) -> Chunk<T> {
    let mut padded = Vec::with_capacity(COW_CHUNK_LEN);
    padded.extend_from_slice(values);
    padded.resize_with(COW_CHUNK_LEN, T::default);
    full_chunk(padded)
}

// apisym1c: the checker copies of the binder lineage.
impl<T: Clone + Default> CowChunks<T> {
    /// A checker copy of this interleaved lineage array. It shares the
    /// lineage chunks as a clone does, holds the lineage's tail as an owned
    /// chunk padded with default values, and pushes its own values from the
    /// next odd chunk on.
    #[must_use]
    pub fn for_checker(&self) -> CowChunks<T> {
        debug_assert!(self.interleaved, "a checker copies the binder lineage");
        let mut chunks = Vec::with_capacity(self.chunks.len() + 2);
        chunks.extend(self.chunks.iter().cloned());
        if !self.tail.is_empty() {
            chunks.push(padded_chunk(&self.tail));
        }
        if chunks.len() % 2 == 0 {
            chunks.push(Chunk::Freed);
        }
        CowChunks {
            len: chunks.len() << COW_CHUNK_SHIFT,
            chunks,
            tail: Vec::new(),
            interleaved: true,
        }
    }

    /// Adds the lineage values of `from` from index `seen` on. `from` is
    /// the binder lineage now, and this array a checker copy of it
    /// (`for_checker`) whose lineage length was `seen`, so each lineage
    /// index below `seen` holds the same value in both, except where this
    /// array wrote. The values here do not change. The later lineage chunks
    /// are shared as `from` holds them (static, shared by `Arc`, or freed),
    /// and a chunk that `from` still fills is padded. When the lineage
    /// passes this array's tail, the tail ends in its chunk, as `end_chunk`
    /// ends one, and the next own value starts after the lineage.
    pub fn catch_up(&mut self, seen: usize, from: &CowChunks<T>) {
        debug_assert!(
            self.interleaved && from.interleaved,
            "a checker copy catches up to the binder lineage"
        );
        if from.len <= seen {
            return;
        }
        // The last even chunk with indexes below the lineage length.
        let last = ((from.len - 1) >> COW_CHUNK_SHIFT) & !1;
        if last >= self.chunks.len() {
            let mut own = std::mem::take(&mut self.tail);
            self.chunks.push(match own.len() {
                0 => Chunk::Freed,
                COW_CHUNK_LEN => full_chunk(own),
                // The tail has room for a whole chunk (`make_room`), and no
                // value goes in this chunk again: keep only its values.
                // PERF (followups31): the shrink copies the values (1 to 255)
                // to an allocation of their size. On the apisym1c catch-up
                // traces (alvin, THP off for the server, 20 runs per side,
                // alternating, medians) it lowered peak RSS from 87.8 to
                // 86.1 MiB on k3-memloop-60 and from 76.1 to 75.7 MiB on
                // k3-memloop-10. m-kl-loop-10 and m-kl-loop-50 stayed at
                // 182.2 and 194.8 MiB. Server CPU and wall time did not
                // change (each p > 0.29). A first reading on zbook showed a
                // rise of 2 to 4%, inside zbook's noise.
                _ => {
                    own.shrink_to_fit();
                    Chunk::Shared(Arc::new(own))
                }
            });
            self.chunks.resize_with(last + 1, || Chunk::Freed);
            self.len = self.tail_start();
        }
        let first = seen >> COW_CHUNK_SHIFT;
        for position in (first..=last).filter(|position| position % 2 == 0) {
            let values = from.chunk_values(position);
            let start = position << COW_CHUNK_SHIFT;
            if start < seen {
                // This array holds the values before `seen` in this chunk.
                let held = seen - start;
                if values.len() > held {
                    let chunk = self.padded_chunk_mut(position);
                    chunk[held..values.len()].clone_from_slice(&values[held..]);
                }
                continue;
            }
            debug_assert!(
                matches!(self.chunks[position], Chunk::Freed),
                "lineage chunk {position} is new here"
            );
            self.chunks[position] = match from.chunks.get(position) {
                Some(
                    chunk @ (Chunk::Fixed(Fixed::Static(_)) | Chunk::Shared(_) | Chunk::Freed),
                ) => chunk.clone(),
                _ if values.is_empty() => Chunk::Freed,
                // An owned chunk of `from`, or its tail.
                _ => padded_chunk(values),
            };
        }
    }

    /// The values of chunk `position` for a write, as an owned chunk padded
    /// with default values.
    fn padded_chunk_mut(&mut self, position: usize) -> &mut ChunkValues<T> {
        let chunk = &mut self.chunks[position];
        if !matches!(chunk, Chunk::Fixed(Fixed::Owned(_))) {
            *chunk = match std::mem::replace(chunk, Chunk::Freed) {
                Chunk::Fixed(Fixed::Static(values)) => full_chunk(values.to_vec()),
                Chunk::Shared(values) => padded_chunk(&values),
                _ => padded_chunk(&[]),
            };
        }
        match chunk {
            Chunk::Fixed(Fixed::Owned(values)) => values,
            _ => unreachable!("chunk was just padded"),
        }
    }
}

/// Values moved out of a `CowChunks` for `CowChunks::append_aligned` at
/// index `at`: the values that fill the partial tail at `at`, then whole
/// chunks and the values after them (the new tail).
#[derive(Debug)]
pub struct AlignedChunks<T: 'static> {
    at: usize,
    len: usize,
    head: Vec<T>,
    chunks: Vec<Chunk<T>>,
    tail: Vec<T>,
}

impl<T: Clone> Default for CowChunks<T> {
    fn default() -> Self {
        Self::new()
    }
}

// PORT: no Go counterpart. Holes of the binder lineage (lsshells M3d).
#[cfg(test)]
mod hole_tests {
    use super::*;

    /// Pushes each index as its value until the length is `end`.
    fn push_to(values: &mut CowChunks<usize>, end: usize) {
        while values.len() < end {
            let index = values.len();
            values.push(index);
        }
    }

    // A range that `end_chunk` starts and ends is whole chunks. Freeing it
    // makes its indexes (and the padding before it) holes that panic on a
    // read, keeps every other index, and a clone keeps the freed values.
    #[test]
    fn freed_chunks_are_holes_and_other_indexes_stay() {
        let mut values = CowChunks::new();
        push_to(&mut values, 300);
        values.end_chunk();
        let start = values.len();
        assert_eq!(start, 2 * COW_CHUNK_LEN);
        push_to(&mut values, start + 600);
        values.end_chunk();
        let end = values.len();
        assert_eq!(end, 5 * COW_CHUNK_LEN);
        push_to(&mut values, end + 10);
        values.share_from(0);
        let clone = values.clone();
        assert_eq!(values.live_chunks(), 6);

        values.free_chunks(start, end);
        assert_eq!(values.live_chunks(), 3);
        assert_eq!((*values.get(299), *values.get(end + 9)), (299, end + 9));
        let read = |index: usize| std::panic::catch_unwind(|| *values.get(index)).is_err();
        assert!(read(start + 1), "a read of a freed index panics");
        assert!(read(400), "a read of the padding panics");
        assert_eq!(
            *clone.get(start + 1),
            start + 1,
            "the clone keeps its chunks"
        );
        assert_eq!(clone.live_chunks(), 6);

        values.share_from(0);
        assert_eq!(values.live_chunks(), 3, "a freed chunk stays empty");
        values.push(end + 10);
        assert_eq!((values.len(), *values.get(end + 10)), (end + 11, end + 10));
    }

    // A range that pushes filled to its last index is freed too: there
    // `end_chunk` does nothing, so the range's last chunk is still the tail
    // when no `share_from` came between (gaps147 skeptic, cowfuzz latent2).
    #[test]
    fn a_full_tail_in_the_range_is_freed() {
        let mut values = CowChunks::new();
        push_to(&mut values, 300);
        values.end_chunk();
        let start = values.len();
        push_to(&mut values, start + COW_CHUNK_LEN);
        values.end_chunk();
        let end = values.len();
        assert_eq!((start, end), (2 * COW_CHUNK_LEN, 3 * COW_CHUNK_LEN));
        assert_eq!(values.live_chunks(), 3);

        values.free_chunks(start, end);
        assert_eq!(values.live_chunks(), 2);
        let read = |index: usize| std::panic::catch_unwind(|| *values.get(index)).is_err();
        assert!(read(start + 5), "a read of a freed index panics");
        assert_eq!(*values.get(299), 299);
        values.push(end);
        assert_eq!((values.len(), *values.get(end)), (end + 1, end));
    }

    // A read at or above the length panics, as a `Vec` read does: in the
    // tail with no chunk, in the tail after chunks, in a partial shared
    // chunk that a write took back, and after a static chunk.
    #[test]
    fn reads_at_or_above_the_length_panic() {
        let read = |values: &CowChunks<usize>, index: usize| {
            std::panic::catch_unwind(|| *values.get(index)).is_err()
        };
        let mut values = CowChunks::new();
        push_to(&mut values, 10);
        assert!(!read(&values, 9) && read(&values, 10), "no chunk");
        push_to(&mut values, 300);
        assert!(!read(&values, 299) && read(&values, 300) && read(&values, 511));

        values.share_from(0);
        *values.get_mut(260) = 1;
        assert!(
            read(&values, 300),
            "a written partial chunk keeps its holes"
        );
        values.push(300);
        assert_eq!((*values.get(260), *values.get(300)), (1, 300));
        assert!(read(&values, 301));

        push_to(&mut values, 600);
        values.freeze_from(0);
        assert!(!read(&values, 599) && read(&values, 600) && read(&values, 767));
        *values.get_mut(10) = 2;
        assert_eq!((*values.get(10), values.len()), (2, 600));
    }

    // The values of a freed chunk drop with the last clone that shares it.
    #[test]
    fn freed_values_drop_with_the_last_clone() {
        let token = Arc::new(());
        let mut values = CowChunks::new();
        for _ in 0..10 {
            values.push(Arc::clone(&token));
        }
        values.end_chunk();
        values.share_from(0);
        let clone = values.clone();
        values.free_chunks(0, COW_CHUNK_LEN);
        assert_eq!(Arc::strong_count(&token), 11);
        drop(clone);
        assert_eq!(Arc::strong_count(&token), 1);
    }

    // A file arena joins at aligned offsets as it would bind there: the
    // offsets are `ArenaOffsets::aligned`, its ids start at a chunk start
    // (the next even chunk of the interleaved lineage), and its tables
    // point to its symbols. Freeing its range keeps the symbols before it.
    #[test]
    fn file_arena_joins_at_a_chunk_start() {
        let mut arena = SymbolArena::new();
        let before = arena.new_symbol(SymbolFlags::NONE, "before");
        for _ in 0..299 {
            arena.new_symbol(SymbolFlags::NONE, "other");
        }
        let offsets = arena.next_file_offsets();
        arena.end_chunk();
        assert_eq!(arena.next_file_offsets(), offsets.aligned());
        assert_eq!(offsets.aligned().aligned(), offsets.aligned());

        let mut file = SymbolArena::new_file();
        let symbol = file.new_symbol(SymbolFlags::NONE, "f");
        let members = file.new_table();
        file.set(members, "f", symbol);
        file.sym_mut(symbol).members = members;
        let start = arena.mark();
        let moved = arena.append_file_arena(file, true);
        assert_eq!(moved, offsets.aligned());
        let id = moved.symbol(symbol);
        assert_eq!(id.index(), 4 * COW_CHUNK_LEN);
        assert_eq!(arena.sym(id).name.as_str(), "f");
        assert_eq!(arena.get(arena.sym(id).members, "f"), id);

        arena.end_chunk();
        let live = arena.live_chunk_count();
        arena.free_range(start, arena.mark());
        assert_eq!(arena.live_chunk_count(), live - 2);
        assert_eq!(arena.sym(before).name.as_str(), "before");
    }

    /// Pushes `count` values, each its own index, and returns the indexes.
    fn push_indexes(values: &mut CowChunks<usize>, count: usize) -> Vec<usize> {
        (0..count)
            .map(|_| {
                let index = values.next_index();
                values.push(index);
                assert_eq!(values.len() - 1, index, "next_index");
                index
            })
            .collect()
    }

    // apisym1c: an interleaved array puts its values in the even chunks, a
    // freed chunk after each, so a read of an odd index panics. `end_chunk`
    // and a full tail move to the next even chunk, and `dense_len` counts
    // the even indexes.
    #[test]
    fn an_interleaved_array_skips_the_odd_chunks() {
        let mut values = CowChunks::new_interleaved();
        let pushed = push_indexes(&mut values, 600);
        let chunk = |index: usize| index >> COW_CHUNK_SHIFT;
        assert!(pushed.iter().all(|&index| chunk(index) % 2 == 0));
        assert_eq!(
            (pushed[255], pushed[256], pushed[599]),
            (255, 2 * COW_CHUNK_LEN, 4 * COW_CHUNK_LEN + 87)
        );
        assert!(pushed.iter().all(|&index| *values.get(index) == index));
        let read = |values: &CowChunks<usize>, index: usize| {
            std::panic::catch_unwind(|| *values.get(index)).is_err()
        };
        assert!(read(&values, COW_CHUNK_LEN) && read(&values, 3 * COW_CHUNK_LEN + 1));
        assert_eq!(values.dense_len(), 600);
        values.end_chunk();
        assert_eq!((values.len(), values.dense_len()), (6 * COW_CHUNK_LEN, 768));
        push_indexes(&mut values, COW_CHUNK_LEN);
        assert_eq!(values.next_index(), 8 * COW_CHUNK_LEN, "a full tail");
        assert_eq!(values.dense_len(), 4 * COW_CHUNK_LEN);
        values.end_chunk();
        assert_eq!(values.len(), 8 * COW_CHUNK_LEN);
        values.free_chunks(6 * COW_CHUNK_LEN, 8 * COW_CHUNK_LEN);
        assert!(read(&values, 6 * COW_CHUNK_LEN) && !read(&values, pushed[599]));
    }

    // apisym1c: a checker copy of the lineage makes its own values in the
    // odd chunks and catches up to later lineage values. It keeps what it
    // wrote and its own values, takes the lineage chunks bound since (a
    // partial one padded), ends its tail when the lineage passes it, and
    // catches up again inside a chunk.
    #[test]
    fn a_checker_copy_catches_up_to_later_lineage_values() {
        let mut lineage = CowChunks::new_interleaved();
        let mut bound = push_indexes(&mut lineage, 300);
        lineage.freeze_from(0);
        let mut checker = lineage.for_checker();
        let seen = lineage.len();
        let own: Vec<usize> = (0..300)
            .map(|n| {
                checker.push(10_000 + n);
                checker.len() - 1
            })
            .collect();
        assert!(own.iter().all(|&index| (index >> COW_CHUNK_SHIFT) % 2 == 1));
        assert_eq!(own[0], 3 * COW_CHUNK_LEN, "after the lineage's tail chunk");
        assert_eq!(checker.odd_indexes().collect::<Vec<_>>(), own);
        *checker.get_mut(5) = 1005;
        *checker.get_mut(bound[290]) = 1290;

        // A static file fills the tail chunk and two more; then a freeable
        // version on chunks of its own, and the tail of a static file.
        bound.extend(push_indexes(&mut lineage, 600));
        lineage.freeze_from(0);
        lineage.end_chunk();
        let start = lineage.len();
        let version = push_indexes(&mut lineage, 10);
        lineage.end_chunk();
        lineage.share_from(start);
        bound.extend(push_indexes(&mut lineage, 3));
        checker.catch_up(seen, &lineage);
        for &index in bound.iter().chain(&version) {
            let expected = match index {
                5 => 1005,
                _ if index == bound[290] => 1290,
                _ => index,
            };
            assert_eq!(*checker.get(index), expected, "{index}");
        }
        for (n, &index) in own.iter().enumerate() {
            assert_eq!(*checker.get(index), 10_000 + n, "own {index}");
        }
        // The lineage passed the own tail (chunk 5): the next own value
        // starts after the lineage's last chunk.
        checker.push(20_000);
        let next = checker.len() - 1;
        assert_eq!(
            next >> COW_CHUNK_SHIFT,
            (lineage.len() >> COW_CHUNK_SHIFT) + 1
        );
        let mut all_own = own.clone();
        all_own.push(next);
        assert_eq!(checker.odd_indexes().collect::<Vec<_>>(), all_own);

        // More values in the lineage's tail chunk: a catch-up inside it.
        let seen = lineage.len();
        let more = push_indexes(&mut lineage, 5);
        checker.catch_up(seen, &lineage);
        assert!(more.iter().all(|&index| *checker.get(index) == index));
        assert_eq!(*checker.get(next), 20_000);

        // The version dies: its chunks go, the own chunks stay.
        checker.free_chunks(start, start + 2 * COW_CHUNK_LEN);
        let read = |index: usize| std::panic::catch_unwind(|| *checker.get(index)).is_err();
        assert!(read(version[0]));
        assert_eq!(*checker.get(own[299]), 10_299);
    }

    // followups31 (R177 reviewer item 4): when a catch-up passes the own
    // tail, the tail becomes a shared chunk with its values only, not the
    // room of a whole chunk that it had for later values.
    #[test]
    fn a_catch_up_past_the_own_tail_keeps_only_its_values() {
        let mut lineage = CowChunks::new_interleaved();
        push_indexes(&mut lineage, 10);
        let mut checker = lineage.for_checker();
        let seen = lineage.len();
        let own: Vec<usize> = (0..44)
            .map(|n| {
                checker.push(10_000 + n);
                checker.len() - 1
            })
            .collect();
        let own_chunk = own[0] >> COW_CHUNK_SHIFT;
        assert_eq!(checker.tail.capacity(), COW_CHUNK_LEN, "room for a chunk");
        let later = push_indexes(&mut lineage, 3 * COW_CHUNK_LEN);
        checker.catch_up(seen, &lineage);
        match &checker.chunks[own_chunk] {
            Chunk::Shared(values) => assert_eq!((values.len(), values.capacity()), (44, 44)),
            _ => panic!("the own tail is a shared chunk"),
        }
        for (n, &index) in own.iter().enumerate() {
            assert_eq!(*checker.get(index), 10_000 + n, "own {index}");
        }
        assert!(later.iter().all(|&index| *checker.get(index) == index));
    }

    // apisym1c: a checker arena makes its own symbols and tables in the odd
    // chunks. It catches up to the lineage: it reads a later binder symbol,
    // its table and its members by the lineage ids, keeps its own symbols,
    // and frees the ranges that the lineage freed.
    #[test]
    fn a_checker_arena_catches_up_to_the_lineage() {
        let mut lineage = SymbolArena::new();
        let early = lineage.new_symbol(SymbolFlags::NONE, "early");
        let mut checker = lineage.for_checker();
        let own = checker.new_symbol(SymbolFlags::TRANSIENT, "own");
        let own_table = checker.new_table();
        checker.set(own_table, "early", early);
        assert!(is_own_index(own.0) && is_own_index(own_table.0));
        assert_eq!(checker.symbol_count(), 2);
        assert_eq!(checker.own_symbols().collect::<Vec<_>>(), [own]);

        // A freeable version binds after the copy, then dies.
        lineage.end_chunk();
        let start = lineage.mark();
        let later = lineage.new_symbol(SymbolFlags::NONE, "later");
        let members = lineage.new_table();
        lineage.set(members, "m", later);
        lineage.sym_mut(later).members = members;
        lineage.end_chunk();
        lineage.share_since(start);
        let end = lineage.mark();
        lineage.set_lineage_seen(LineageSeen {
            generation: 1,
            freed: 0,
        });
        checker.catch_up(&lineage, &[]);
        assert_eq!(checker.sym(later).name.as_str(), "later");
        assert_eq!(checker.get(checker.sym(later).members, "m"), later);
        assert_eq!(checker.sym(own).name.as_str(), "own");
        assert_eq!(checker.get(own_table, "early"), early);
        assert_eq!(
            checker.id_slot(later),
            IdSlot {
                key: 0,
                place: later.0
            }
        );
        assert_eq!(checker.symbol_at_slot(checker.id_slot(own)), Some(own));
        assert_eq!(checker.own_symbols().collect::<Vec<_>>(), [own]);
        let live = checker.live_chunk_count();

        lineage.free_range(start, end);
        let freed = [(start, end)];
        lineage.set_lineage_seen(LineageSeen {
            generation: 2,
            freed: 1,
        });
        checker.catch_up(&lineage, &freed);
        assert_eq!(checker.lineage_seen().freed, 1);
        assert_eq!(checker.live_chunk_count(), live - 2);
        assert!(std::panic::catch_unwind(|| checker.sym(later).flags).is_err());
        assert_eq!(checker.sym(early).name.as_str(), "early");
        assert_eq!(checker.sym(own).name.as_str(), "own");
    }
}

/// `vec![T::default(); len]` for a large table that a probe reads before it
/// writes. Not a Go port. `T` is an integer: its `Default` is all zero
/// bytes, so `vec!` is calloc, and it is at most 4096 bytes, so the stores
/// below write each 4 KiB page. A compile-time assert checks the size, and a
/// debug assert checks that `T::default()` is 0.
///
/// PERF: prefault1. `vec![0; len]` is calloc, and jemalloc does not write a
/// fresh extent. A read first maps the huge zero page over its 2 MiB block.
/// On Linux up to 6.12 (6.13: "Do not shatter hugezeropage on wp-fault")
/// the first write splits it, and each 4 KiB page then takes a
/// copy-on-write fault with a TLB shootdown to the other checker threads
/// (zod MT: about 3,200 such faults per run). One store per 4 KiB page,
/// and one on the last element, makes the first touch a write. Linux only:
/// it was measured only there, and wasm has no page faults.
// PERF: `inline(never)`, so a call site stays as small as the `vec!` call it
// replaces. Inlined, the check and the loop made `Table::push` (through
// `reindex` and `empty_index`) too large to inline into its callers, and zod
// ST ran 0.55% more instructions.
#[inline(never)]
pub fn zeroed_vec<T: Copy + Default + PartialEq + From<u8>>(len: usize) -> Vec<T> {
    const {
        assert!(
            size_of::<T>() <= 4096,
            "zeroed_vec: T is larger than a page"
        )
    };
    debug_assert!(
        T::default() == T::from(0),
        "zeroed_vec: T::default() is not 0"
    );
    let mut vec = vec![T::default(); len];
    if cfg!(target_os = "linux") && size_of_val(vec.as_slice()) >= 16 << 10 {
        let step = 4096 / size_of::<T>().max(1);
        for i in (0..len).step_by(step).chain([len - 1]) {
            // black_box: LLVM removes a plain store of 0 into calloc memory.
            vec[i] = std::hint::black_box(T::default());
        }
    }
    vec
}

/// One symbol table entry: a name id and its symbol. The table hash of the
/// name is in the interner (`TableEntry::hash`), not here.
// PERF (memper1 B): 8 bytes, not 12 with the hash. A lookup by `Name` (most
// lookups) compares ids. A lookup by text and a reindex read the interner's
// hash of each entry they compare or index. One-byte index slots for tables
// of up to 255 entries (half the index) were measured and dropped: the slot
// width test kept the lookup and insert loops out of line (+1.6%
// instructions on zod, single-threaded).
#[derive(Clone, Copy, Debug)]
struct TableEntry {
    name: u32,
    symbol: SymbolId,
}

const _: () = assert!(std::mem::size_of::<TableEntry>() == 8);

impl TableEntry {
    /// The table hash of the name.
    #[inline]
    fn hash(&self) -> u32 {
        intern::table_hash(self.name)
    }
}

/// Tables up to this size are searched linearly and have no index.
const TABLE_LINEAR_MAX: usize = 8;

/// A Go `ast.SymbolTable`: entries in insertion order. Larger tables also
/// keep an open-addressing index. A slot holds `position % INDEX_MOD + 1`
/// (0 is empty); a table longer than `INDEX_MOD` checks every position with
/// that remainder.
#[derive(Clone, Debug, Default)]
struct Table {
    entries: Vec<TableEntry>,
    index: Box<[u16]>,
    /// The `filter_bit` of the hash of each entry. A name whose bit is clear
    /// is not in the table, so most lookups of an absent name end here and
    /// read no entry and no index slot, as most Go map lookups of an absent
    /// key end at the control bytes of one group.
    filter: u64,
}

/// The bit of a table hash in `Table::filter`: its top 6 bits, which the
/// index slot (the low bits) does not use.
#[inline]
fn filter_bit(hash: u32) -> u64 {
    1 << (hash >> 26)
}

const INDEX_MOD: usize = u16::MAX as usize;

/// The table hash of `name`. `Name::table_hash` gives the same value
/// without hashing.
#[inline]
fn table_hash(name: &str) -> u32 {
    fold_hash(intern::hash_str(name))
}

/// Folds an `intern::hash_str` hash to the 32-bit table hash.
#[inline]
fn fold_hash(hash: u64) -> u32 {
    (hash ^ (hash >> 32)) as u32
}

thread_local! {
    /// (address, length, table hash) of the text of the innermost
    /// `with_text_hash` call on this thread. Zeros when there is none: a
    /// `str` address is never 0.
    static TEXT_HASH: std::cell::Cell<(usize, usize, u32)> =
        const { std::cell::Cell::new((0, 0, 0)) };
}

/// Runs `f`. While it runs, `SymbolArena::get` reuses one hash of `text`
/// for lookups of this same `text` (same address and length). It is for
/// code that looks one text up in many tables through APIs that take
/// `&str`, such as `NameResolver::resolve` (one table per scope, through a
/// lookup callback).
///
/// This is exact: `text` stays borrowed until `f` returns, so any `str` with
/// the same address and length that is alive during `f` has the same bytes.
/// Calls nest; each one restores the outer text when it returns or unwinds.
pub fn with_text_hash<R>(text: &str, f: impl FnOnce() -> R) -> R {
    struct Restore((usize, usize, u32));
    impl Drop for Restore {
        fn drop(&mut self) {
            TEXT_HASH.set(self.0);
        }
    }
    let entry = (text.as_ptr().addr(), text.len(), table_hash(text));
    let _restore = Restore(TEXT_HASH.replace(entry));
    f()
}

/// The table hash of `text`, from the innermost `with_text_hash` when it
/// covers this exact text.
#[inline]
fn lookup_hash(text: &str) -> u32 {
    let (address, len, hash) = TEXT_HASH.get();
    if address == text.as_ptr().addr() && len == text.len() {
        hash
    } else {
        table_hash(text)
    }
}

/// A symbol table key for code that is called with either a `&Name` or a
/// `&str`. A `Name` finds its entry by id with the hash the interner keeps,
/// so it needs no hashing and no text compare.
#[derive(Clone, Copy, Debug)]
pub enum TableKey<'a> {
    Text(&'a str),
    Name(&'a Name),
}

impl<'a> TableKey<'a> {
    /// The key text.
    #[must_use]
    pub fn text(self) -> &'a str {
        match self {
            TableKey::Text(text) => text,
            TableKey::Name(name) => name.as_str(),
        }
    }
}

impl<'a> From<&'a str> for TableKey<'a> {
    fn from(text: &'a str) -> Self {
        TableKey::Text(text)
    }
}

impl<'a> From<&'a Name> for TableKey<'a> {
    fn from(name: &'a Name) -> Self {
        TableKey::Name(name)
    }
}

impl Table {
    fn with_capacity(capacity: usize) -> Self {
        Table {
            entries: Vec::with_capacity(capacity),
            // PERF: a table sized past `TABLE_LINEAR_MAX` gets its index now,
            // so filling it never reindexes. The index is lookup only.
            index: Self::empty_index(capacity),
            filter: 0,
        }
    }

    /// An empty index for `len` entries, or no index when a table of that
    /// size is searched linearly.
    // PERF: `zeroed_vec`, as `index_insert` reads a slot before it writes one.
    fn empty_index(len: usize) -> Box<[u16]> {
        if len <= TABLE_LINEAR_MAX {
            return Box::default();
        }
        zeroed_vec((len * 2).next_power_of_two()).into_boxed_slice()
    }

    /// True when `additional` more entries fit without growing the entries
    /// or rebuilding the index.
    fn has_room(&self, additional: usize) -> bool {
        let len = self.entries.len() + additional;
        self.entries.capacity() >= len && (len <= TABLE_LINEAR_MAX || self.index.len() >= len * 2)
    }

    /// Makes room for `additional` more entries (see `has_room`). Entry
    /// order does not change.
    fn reserve(&mut self, additional: usize) {
        self.entries.reserve(additional);
        let len = self.entries.len() + additional;
        if len > TABLE_LINEAR_MAX && self.index.len() < len * 2 {
            let mut index = Self::empty_index(len);
            for (position, entry) in self.entries.iter().enumerate() {
                Self::index_insert(&mut index, entry.hash(), position);
            }
            self.index = index;
        }
    }

    /// The position of `name`, whose `table_hash` is `hash`.
    #[inline]
    fn find(&self, hash: u32, name: &str) -> Option<usize> {
        self.find_by(hash, |e| e.hash() == hash && intern::text(e.name) == name)
    }

    /// The position of name id `id`, whose `table_hash` is `hash`. Equal
    /// texts intern to one id, so the ids compare.
    #[inline]
    fn find_id(&self, hash: u32, id: u32) -> Option<usize> {
        self.find_by(hash, |e| e.name == id)
    }

    /// The position of the entry with hash `hash` that `is_name` accepts.
    #[inline]
    fn find_by(&self, hash: u32, is_name: impl Fn(&TableEntry) -> bool) -> Option<usize> {
        if self.filter & filter_bit(hash) == 0 {
            return None;
        }
        if self.index.is_empty() {
            return self.entries.iter().position(is_name);
        }
        let mask = self.index.len() - 1;
        let mut slot = hash as usize & mask;
        loop {
            let stored = self.index[slot];
            if stored == 0 {
                return None;
            }
            let mut position = stored as usize - 1;
            while position < self.entries.len() {
                let entry = &self.entries[position];
                if is_name(entry) {
                    return Some(position);
                }
                position += INDEX_MOD;
            }
            slot = (slot + 1) & mask;
        }
    }

    /// Adds `position` to the index. The index has a free slot.
    fn index_insert(index: &mut [u16], hash: u32, position: usize) {
        let mask = index.len() - 1;
        let mut slot = hash as usize & mask;
        while index[slot] != 0 {
            slot = (slot + 1) & mask;
        }
        index[slot] = u16::try_from(position % INDEX_MOD + 1).expect("index slot");
    }

    /// Rebuilds the index and the filter for the current entries.
    // PERF: one hash read per entry, for the filter and the index. Out of
    // line, so `push`, which calls it when a table grows, stays small enough
    // to inline into its callers.
    #[inline(never)]
    fn reindex(&mut self) {
        let mut filter = 0;
        let mut index = Self::empty_index(self.entries.len());
        let indexed = !index.is_empty();
        for (position, entry) in self.entries.iter().enumerate() {
            let hash = entry.hash();
            filter |= filter_bit(hash);
            if indexed {
                Self::index_insert(&mut index, hash, position);
            }
        }
        self.filter = filter;
        self.index = index;
    }

    /// Go `table[name] = symbol`. A new name goes last, like `IndexMap`.
    fn insert(&mut self, name: &Name, symbol: SymbolId) {
        let hash = name.table_hash();
        match self.find_id(hash, name.0) {
            Some(position) => self.entries[position].symbol = symbol,
            None => self.push(hash, name.0, symbol),
        }
    }

    /// Appends an entry for name id `name`, which is not in the table.
    // PERF: inline, as before the filter: out of line, the binder's inserts
    // paid a call per new entry (+0.2% instructions on Hono).
    #[inline]
    fn push(&mut self, hash: u32, name: u32, symbol: SymbolId) {
        self.entries.push(TableEntry { name, symbol });
        self.filter |= filter_bit(hash);
        let len = self.entries.len();
        // A table can have an index before it passes `TABLE_LINEAR_MAX`
        // (`with_capacity`, `reserve`). Every entry of an indexed table is in
        // the index.
        if self.index.len() >= len * 2 {
            Self::index_insert(&mut self.index, hash, len - 1);
        } else if len > TABLE_LINEAR_MAX {
            self.reindex();
        }
    }

    /// Go `delete(table, name)`. Later entries keep their order.
    fn remove(&mut self, name: &str) {
        if let Some(position) = self.find(table_hash(name), name) {
            self.entries.remove(position);
            self.reindex();
        }
    }
}

/// Owns all symbols and symbol tables. The binder fills one arena. Each
/// checker starts from a copy (`for_checker`), so binder ids stay valid and
/// checker (transient) symbols stay private to that checker. The API can add
/// a copy of a symbol of another checker (`push_shadow`). The copy shares
/// the binder's symbol and table chunks; a checker copies a chunk only when
/// it first writes to it. The binder writes without atomic operations and
/// then publishes what it wrote, so the copy copies at most the last chunk:
/// the full chunks of a static file leak (`freeze_since`), because its
/// symbols live until exit, and the chunks of a freeable file version are
/// shared (`share_since`). The binder lineage (`program.rs`) binds a
/// freeable version into whole chunks of its own (`end_chunk`) and frees
/// them when the version dies (`free_range`, lsshells M3d).
///
/// apisym1c: the binder lineage and its copies are interleaved
/// (`CowChunks::new_interleaved`): lineage symbols and tables are in the
/// even chunks, and a checker arena makes its own in the odd chunks
/// (`is_own_index`). So a lineage index names one lineage symbol or table
/// in every arena, as a Go `*ast.Symbol` is one object for every checker,
/// and a checker can add the lineage chunks bound after its copy
/// (`catch_up`). A file arena (`new_file`) is not interleaved: its ids move
/// to lineage ids when it joins (`ArenaOffsets`).
#[derive(Clone, Debug)]
pub struct SymbolArena {
    symbols: CowChunks<Symbol>,
    tables: CowChunks<Table>,
    /// The names that the binder gave private identifier symbols
    /// (`get_symbol_name_for_private_identifier`), as intern ids. Each holds
    /// a symbol id, which `prepare_file_arena` moves.
    private_names: Vec<u32>,
    /// Where `crate::ast::get_symbol_id` keeps the ids of these symbols.
    ids: SymbolIds,
    /// The state of the binder lineage that this arena copies
    /// (`program::catch_up_checker`).
    seen: LineageSeen,
}

/// A state of the binder lineage (`program.rs`): its generation, which each
/// bind and free makes new, and the number of ranges that it has freed. A
/// copy of the lineage keeps the state that it copies, so a checker knows
/// when it must catch up (`SymbolArena::catch_up`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LineageSeen {
    pub generation: u64,
    pub freed: usize,
}

/// Whether symbol or table index `index` of a checker arena is one that the
/// checker made (an odd chunk, see `SymbolArena`), not a lineage index.
#[inline(always)]
#[must_use]
pub const fn is_own_index(index: u32) -> bool {
    index as usize & COW_CHUNK_LEN != 0
}

/// Where `crate::ast::get_symbol_id` keeps the ids of the symbols of one
/// arena. Every binder arena of the process (the binder lineage in
/// `program.rs` and its copies) gives a symbol one id, like Go, where a
/// bound file keeps its symbols in every program. A checker arena
/// (`SymbolArena::for_checker`) makes its own symbols (`is_own_index`),
/// which other checkers do not have, so those symbols have ids of their
/// own, kept by `key`.
#[derive(Debug)]
struct SymbolIds {
    /// `COW_CHUNK_LEN`, the index bit of an own symbol, in a checker arena;
    /// 0 in a binder arena, whose symbols all have shared ids.
    own_bit: u32,
    /// The first own chunk of a checker arena, as a chunk pair (chunk
    /// `2 * own_base + 1`): own symbol places count from it (`own_place`).
    own_base: u32,
    /// The lineage symbol and table lengths that a checker arena copies
    /// (`catch_up` moves them); `u32::MAX` in a binder arena.
    lineage_symbols: u32,
    lineage_tables: u32,
    /// Keys the ids of the own symbols. 0 in a binder arena.
    key: u32,
    /// The shadows in this arena (`SymbolArena::push_shadow`), if any.
    shadows: Option<Box<Shadows>>,
}

/// The shadows of one checker arena: copies of symbols of other arenas.
/// A shadow keeps its id in the slot of its origin.
#[derive(Debug, Default)]
struct Shadows {
    /// The id slot of each shadow's origin, by shadow index.
    origins: FxHashMap<u32, IdSlot>,
    /// The shadow of each origin.
    by_origin: FxHashMap<IdSlot, SymbolId>,
}

impl SymbolIds {
    /// Every symbol has the shared id of its index.
    const SHARED: SymbolIds = SymbolIds {
        own_bit: 0,
        own_base: 0,
        lineage_symbols: u32::MAX,
        lineage_tables: u32::MAX,
        key: 0,
        shadows: None,
    };

    /// The own symbols (from chunk pair `own_base` on) have ids of their
    /// own, under a new key.
    fn own(own_base: u32, lineage_symbols: usize, lineage_tables: usize) -> SymbolIds {
        static NEXT_KEY: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(1);
        SymbolIds {
            own_bit: COW_CHUNK_LEN as u32,
            own_base,
            lineage_symbols: u32::try_from(lineage_symbols).expect("symbol overflow"),
            lineage_tables: u32::try_from(lineage_tables).expect("table overflow"),
            key: NEXT_KEY.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            shadows: None,
        }
    }

    /// The place of own symbol index `index` among the own symbols: its
    /// index without the even chunks, from chunk pair `own_base` on. The
    /// places are dense until the first catch-up that passes the own tail
    /// (`CowChunks::catch_up`): the next own symbol then starts after the
    /// lineage, so each odd chunk that the lineage passed is a gap of
    /// `COW_CHUNK_LEN` places, 2 KiB in the own id table once a later own
    /// symbol has an id (`ast::utilities_p1::OwnSymbolIds`).
    #[inline]
    fn own_place(&self, index: u32) -> u32 {
        (((index >> (COW_CHUNK_SHIFT + 1)) - self.own_base) << COW_CHUNK_SHIFT)
            | (index & COW_CHUNK_MASK as u32)
    }

    /// The own symbol index of place `place` (`own_place`).
    fn own_index(&self, place: u32) -> u32 {
        (((place >> COW_CHUNK_SHIFT) + self.own_base) << (COW_CHUNK_SHIFT + 1))
            | self.own_bit
            | (place & COW_CHUNK_MASK as u32)
    }
}

impl Clone for SymbolIds {
    /// A copy of a checker arena holds copies of its own symbols, which are
    /// other symbols, so they get new ids. Copies of shadows too.
    fn clone(&self) -> Self {
        if self.key == 0 {
            SymbolIds::SHARED
        } else {
            SymbolIds::own(
                self.own_base,
                self.lineage_symbols as usize,
                self.lineage_tables as usize,
            )
        }
    }
}

impl Drop for SymbolIds {
    /// Frees this thread's ids of the arena's own symbols.
    fn drop(&mut self) {
        if self.key != 0 {
            crate::ast::forget_own_symbol_ids(self.key);
        }
    }
}

/// Where `crate::ast::get_symbol_id` keeps the id of one symbol
/// (`SymbolArena::id_slot`). Go keeps the id on the `*ast.Symbol`, so a
/// slot stands for one Go symbol: two symbols with one slot, in any
/// arenas, are the same Go symbol.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct IdSlot {
    /// 0 for a binder lineage symbol, whose id every arena shares. Else the
    /// key of the checker arena that made the symbol.
    pub key: u32,
    /// The lineage index (key 0), or the place of the symbol among the own
    /// symbols of that arena.
    pub place: u32,
}

impl Default for SymbolArena {
    fn default() -> Self {
        Self::new()
    }
}

impl SymbolArena {
    /// A binder lineage arena: interleaved (see `SymbolArena`).
    #[must_use]
    pub fn new() -> Self {
        Self::with_arrays(CowChunks::new_interleaved(), CowChunks::new_interleaved())
    }

    /// A file arena: one file binds into it on its own, and it joins a
    /// lineage arena (`append_file_arena`, `prepare_file_arena`). It is not
    /// interleaved, so its ids are dense, as the lib bind snapshot keeps
    /// them.
    #[must_use]
    pub fn new_file() -> Self {
        Self::with_arrays(CowChunks::new(), CowChunks::new())
    }

    fn with_arrays(mut symbols: CowChunks<Symbol>, mut tables: CowChunks<Table>) -> Self {
        symbols.push(Symbol::default());
        tables.push(Table::default());
        Self {
            symbols,
            tables,
            private_names: Vec::new(),
            ids: SymbolIds::SHARED,
            seen: LineageSeen::default(),
        }
    }

    /// Whether this is a lineage arena or a copy of one (`new`), not a
    /// file arena (`new_file`).
    #[must_use]
    pub fn is_interleaved(&self) -> bool {
        self.symbols.interleaved
    }

    /// A copy of this binder arena for a checker (Go `NewChecker` reads the
    /// bound program). The symbols and tables that the checker adds go to
    /// the odd chunks (`is_own_index`) and have ids of their own
    /// (`crate::ast::get_symbol_id`), so checkers of any program can share
    /// a thread.
    #[must_use]
    pub fn for_checker(&self) -> SymbolArena {
        debug_assert!(
            self.ids.key == 0 && self.is_interleaved(),
            "a checker arena is made from a binder lineage arena"
        );
        let symbols = self.symbols.for_checker();
        let own_base = u32::try_from(symbols.chunks.len() >> 1).expect("symbol overflow");
        SymbolArena {
            symbols,
            tables: self.tables.for_checker(),
            private_names: self.private_names.clone(),
            ids: SymbolIds::own(own_base, self.symbols.len(), self.tables.len()),
            seen: self.seen,
        }
    }

    /// The state of the binder lineage that this arena copies (or is, for
    /// the lineage itself).
    #[must_use]
    pub fn lineage_seen(&self) -> LineageSeen {
        self.seen
    }

    /// Notes that this binder arena (the lineage) is now in state `seen`, so
    /// the copies made from now on keep it.
    pub fn set_lineage_seen(&mut self, seen: LineageSeen) {
        debug_assert!(self.ids.key == 0, "the lineage is a binder arena");
        self.seen = seen;
    }

    /// Brings the lineage copy of this checker arena up to `lineage`, the
    /// binder lineage now, which has freed the ranges `freed`
    /// (`free_range`), in order. It adds the chunks that the lineage bound
    /// after the copy (`CowChunks::catch_up`) and frees the ranges freed
    /// after it, so a symbol of any live file version is the same index
    /// here as in the lineage. The values that this arena holds stay as
    /// they are, so its own writes to lineage symbols stay, and its own
    /// symbols and tables keep their indexes.
    // PORT: Go has one object per symbol, so a checker reads any bound file
    // with no step like this. The frees are the lineage's (lsshells M3d):
    // a request reaches a file version only while a snapshot holds it.
    pub fn catch_up(&mut self, lineage: &SymbolArena, freed: &[(ArenaMark, ArenaMark)]) {
        debug_assert!(
            self.ids.key != 0 && lineage.ids.key == 0,
            "a checker arena catches up to the binder lineage"
        );
        debug_assert_eq!(
            lineage.seen.freed,
            freed.len(),
            "the lineage's freed ranges"
        );
        self.symbols
            .catch_up(self.ids.lineage_symbols as usize, &lineage.symbols);
        self.tables
            .catch_up(self.ids.lineage_tables as usize, &lineage.tables);
        self.ids.lineage_symbols = u32::try_from(lineage.symbols.len()).expect("symbol overflow");
        self.ids.lineage_tables = u32::try_from(lineage.tables.len()).expect("table overflow");
        for &(start, end) in &freed[self.seen.freed..] {
            self.free_range(start, end);
        }
        self.seen = lineage.seen;
    }

    /// Where `crate::ast::get_symbol_id` keeps the id of `symbol`: the
    /// shared slot of its index for a binder symbol, else a slot of this
    /// arena's own ids. A shadow (`push_shadow`) uses the slot of its
    /// origin.
    #[inline]
    #[must_use]
    pub fn id_slot(&self, symbol: SymbolId) -> IdSlot {
        if symbol.0 & self.ids.own_bit == 0 {
            return IdSlot {
                key: 0,
                place: symbol.0,
            };
        }
        if self.ids.shadows.is_some() {
            if let Some(origin) = self.shadow_origin(symbol) {
                return origin;
            }
        }
        IdSlot {
            key: self.ids.key,
            place: self.ids.own_place(symbol.0),
        }
    }

    /// The origin of `symbol` when it is a shadow.
    #[cold]
    #[inline(never)]
    fn shadow_origin(&self, symbol: SymbolId) -> Option<IdSlot> {
        let shadows = self.ids.shadows.as_ref()?;
        shadows.origins.get(&symbol.0).copied()
    }

    /// The symbol of this arena whose id slot is `slot` (`id_slot`): the
    /// symbol itself, or its shadow. None when this arena has neither.
    #[must_use]
    pub fn symbol_at_slot(&self, slot: IdSlot) -> Option<SymbolId> {
        if slot.key == 0 && slot.place < self.ids.lineage_symbols {
            return Some(SymbolId(slot.place));
        }
        if slot.key != 0 && slot.key == self.ids.key {
            return Some(SymbolId(self.ids.own_index(slot.place)));
        }
        let shadows = self.ids.shadows.as_ref()?;
        shadows.by_origin.get(&slot).copied()
    }

    /// Pushes `symbol` as a shadow and returns it. A shadow is a copy of the
    /// symbol of another arena whose id slot is `origin`. It stands for the
    /// same Go symbol, so it keeps its id in `origin` (`id_slot`), and
    /// `symbol_at_slot(origin)` finds it. Only a checker arena has shadows.
    pub fn push_shadow(&mut self, symbol: Symbol, origin: IdSlot) -> SymbolId {
        debug_assert!(self.ids.key != 0, "a shadow goes into a checker arena");
        debug_assert!(
            self.symbol_at_slot(origin).is_none(),
            "one symbol per id slot"
        );
        let shadow = self.push_symbol(symbol);
        let shadows = self.ids.shadows.get_or_insert_with(Box::default);
        shadows.origins.insert(shadow.0, origin);
        shadows.by_origin.insert(origin, shadow);
        if origin.key != 0 {
            crate::ast::keep_own_symbol_ids(origin.key);
        }
        shadow
    }

    /// The index after the last lineage symbol, with the nil symbol at
    /// index 0: in a binder arena the next symbol's index, in a checker
    /// arena that of the lineage copy.
    #[must_use]
    pub fn symbol_count(&self) -> usize {
        if self.ids.key == 0 {
            self.symbols.next_index()
        } else {
            self.ids.lineage_symbols as usize
        }
    }

    /// The symbols that this checker arena made (`is_own_index`), in
    /// order. None in a binder arena.
    pub fn own_symbols(&self) -> impl Iterator<Item = SymbolId> + '_ {
        let own = self.ids.key != 0;
        self.symbols
            .odd_indexes()
            .filter(move |_| own)
            .map(|index| SymbolId(u32::try_from(index).expect("symbol overflow")))
    }

    // The dump and load API of the lib bind snapshot
    // (`binder/lib_snapshot.rs`). A dump reads symbols `1..symbol_count()`
    // (`sym`), tables `1..table_count()` (`iter_names`) and
    // `private_names`. A load pushes them in the same order into a new
    // file arena (`new_file`, `is_new`): `push_symbol`,
    // `push_table_from_entries` and `note_private_name`. Ids are indexes, so
    // they equal the dumped ones.

    /// True for an arena as `SymbolArena::new` or `new_file` makes it: only
    /// the nil symbol and table, and no private names.
    #[must_use]
    pub fn is_new(&self) -> bool {
        self.symbols.len() == 1 && self.tables.len() == 1 && self.private_names.is_empty()
    }

    /// The index after the last lineage table, with the nil table at index
    /// 0, as `symbol_count`.
    #[must_use]
    pub fn table_count(&self) -> usize {
        if self.ids.key == 0 {
            self.tables.next_index()
        } else {
            self.ids.lineage_tables as usize
        }
    }

    /// The names given to `note_private_name`, in order.
    pub fn private_names(&self) -> impl Iterator<Item = Name> + '_ {
        self.private_names.iter().map(|&id| Name(id))
    }

    /// Pushes a new table that holds `entries` in this order and returns
    /// it: Go `make(ast.SymbolTable)` plus `table[name] = symbol` for each
    /// entry. The names must all differ, as in any table (`Table::insert`
    /// keeps one entry per name). The entries and the index are built once.
    pub fn push_table_from_entries(
        &mut self,
        entries: impl ExactSizeIterator<Item = (Name, SymbolId)>,
    ) -> SymbolTable {
        let mut table = Table {
            entries: Vec::with_capacity(entries.len()),
            index: Box::default(),
            filter: 0,
        };
        table
            .entries
            .extend(entries.map(|(name, symbol)| TableEntry {
                name: name.0,
                symbol,
            }));
        table.reindex();
        debug_assert!(
            table
                .entries
                .iter()
                .enumerate()
                .all(|(position, e)| table.find_id(e.hash(), e.name) == Some(position)),
            "table entries with the same name"
        );
        self.push_table(table)
    }

    /// Notes that the binder gave `name` to a private identifier symbol
    /// (`get_symbol_name_for_private_identifier`), so the name holds a
    /// symbol id.
    pub fn note_private_name(&mut self, name: &Name) {
        self.private_names.push(name.0);
    }

    /// The current symbol and table counts, for `share_since`.
    #[must_use]
    pub fn mark(&self) -> ArenaMark {
        ArenaMark {
            symbols: self.symbols.next_index(),
            tables: self.tables.next_index(),
        }
    }

    /// Shares the symbols and tables added since `mark` with future clones
    /// (`CowChunks::share_from`). Call it after binding, before the arena is
    /// cloned.
    pub fn share_since(&mut self, mark: ArenaMark) {
        self.symbols.share_from(mark.symbols);
        self.tables.share_from(mark.tables);
    }

    /// Leaks the full chunks of the symbols and tables added since `mark`
    /// (`CowChunks::freeze_from`), so clones share them for free. Only for
    /// the binder lineage after it binds a static file, whose symbols live
    /// until exit and are never freed (`free_range`).
    // PERF: a read of a static chunk has no pointer hop, and a checker copy
    // changes no reference count.
    pub fn freeze_since(&mut self, mark: ArenaMark) {
        self.symbols.freeze_from(mark.symbols);
        self.tables.freeze_from(mark.tables);
    }

    /// Ends the last symbol chunk and the last table chunk, so the next
    /// symbol and table start new chunks (`CowChunks::end_chunk`). The ids
    /// skipped here hold nothing, and the ended chunks are shared.
    /// `next_file_offsets` is then `ArenaOffsets::aligned`.
    pub fn end_chunk(&mut self) {
        self.symbols.end_chunk();
        self.tables.end_chunk();
    }

    /// Frees the symbols and tables from `start` to `end`: the range of one
    /// file version that `end_chunk` started and ended. Their ids become
    /// holes, and a read of one panics. Other ids do not move. A clone that
    /// shares the chunks (a program copy, a checker arena) keeps them until
    /// it drops.
    pub fn free_range(&mut self, start: ArenaMark, end: ArenaMark) {
        self.symbols.free_chunks(start.symbols, end.symbols);
        self.tables.free_chunks(start.tables, end.tables);
    }

    /// The symbol and table chunks that hold values. Tests use it to see
    /// that `free_range` frees.
    #[must_use]
    pub fn live_chunk_count(&self) -> usize {
        self.symbols.live_chunks() + self.tables.live_chunks()
    }

    /// Go `&ast.Symbol{Flags: flags, Name: name}`.
    #[inline]
    pub fn new_symbol(&mut self, flags: SymbolFlags, name: impl Into<Name>) -> SymbolId {
        self.push_symbol(Symbol {
            flags,
            name: name.into(),
            ..Symbol::default()
        })
    }

    /// Pushes a complete symbol and returns its id. Use it instead of
    /// `new_symbol` plus `sym_mut` writes when every field is known.
    // PERF: inlined with `CowChunks::push`, so the symbol is built in its
    // chunk slot.
    // apisym1c: the id is read after the push, which can move the tail to
    // the next chunk of its parity (`CowChunks::seal_full_tail`).
    #[inline]
    pub fn push_symbol(&mut self, symbol: Symbol) -> SymbolId {
        self.symbols.push(symbol);
        SymbolId(u32::try_from(self.symbols.len() - 1).expect("symbol overflow"))
    }

    fn push_table(&mut self, table: Table) -> SymbolTable {
        self.tables.push(table);
        SymbolTable(u32::try_from(self.tables.len() - 1).expect("table overflow"))
    }

    /// Makes room for `symbols` more symbols and `tables` more tables, so
    /// binding a file does not grow the arena step by step. Capacity only:
    /// ids and contents do not change.
    pub fn reserve_arena(&mut self, symbols: usize, tables: usize) {
        self.symbols.reserve(symbols);
        self.tables.reserve(tables);
    }

    /// Go `make(ast.SymbolTable)`.
    pub fn new_table(&mut self) -> SymbolTable {
        self.push_table(Table::default())
    }

    /// Go `make(ast.SymbolTable, capacity)`.
    pub fn new_table_with_capacity(&mut self, capacity: usize) -> SymbolTable {
        self.push_table(Table::with_capacity(capacity))
    }

    /// Makes room for `additional` more entries in `table`, so the inserts
    /// that follow do not grow it or rebuild its index. Entry order and
    /// lookups do not change. A nil table stays nil. A table that has room
    /// is not written, so a shared chunk is not taken back.
    pub fn reserve(&mut self, table: SymbolTable, additional: usize) {
        if table.is_nil() || self.tables.get(table.index()).has_room(additional) {
            return;
        }
        self.tables.get_mut(table.index()).reserve(additional);
    }

    /// Go `table[name]`, plus the slot of `name` in `table`, so that
    /// `set_slot` stores it without a second lookup. The table must not
    /// change between the two calls. A nil table reads as empty.
    // PERF: always inlined, so the (id, slot) pair stays in registers in the
    // caller and is not returned through memory.
    #[inline(always)]
    #[must_use]
    pub fn get_slot(&self, table: SymbolTable, name: &Name) -> (SymbolId, TableSlot) {
        let hash = name.table_hash();
        let mut found = (SymbolId::NIL, None);
        if table.is_some() {
            let current = self.tables.get(table.index());
            if let Some(position) = current.find_id(hash, name.0) {
                found = (current.entries[position].symbol, Some(position));
            }
        }
        let slot = TableSlot {
            table,
            hash,
            name: name.0,
            position: found.1,
        };
        (found.0, slot)
    }

    /// Go `table[name] = symbol` for the `table` and `name` of `slot`
    /// (`get_slot`). Panics on a nil table, like Go.
    // PERF: always inlined, so the slot is passed in registers.
    #[inline(always)]
    pub fn set_slot(&mut self, slot: TableSlot, symbol: SymbolId) {
        assert!(slot.table.is_some(), "assignment to entry in nil map");
        let current = self.tables.get_mut(slot.table.index());
        match slot.position {
            Some(position) => current.entries[position].symbol = symbol,
            None => {
                debug_assert!(
                    current.find_id(slot.hash, slot.name).is_none(),
                    "table changed after get_slot"
                );
                current.push(slot.hash, slot.name, symbol);
            }
        }
    }

    /// The id offsets that a file arena appended now gets
    /// (`append_file_arena`): every id moves by the number of entries already
    /// here, counted without the odd chunks (`CowChunks::dense_len`).
    #[must_use]
    pub fn next_file_offsets(&self) -> ArenaOffsets {
        debug_assert!(
            self.ids.key == 0 && self.is_interleaved(),
            "a file arena joins a binder lineage arena"
        );
        ArenaOffsets {
            symbols: u32::try_from(self.symbols.dense_len() - 1).expect("symbol overflow"),
            tables: u32::try_from(self.tables.dense_len() - 1).expect("table overflow"),
        }
    }

    /// Appends the symbols and tables of `file_arena`, an arena that one
    /// file was bound into on its own, and returns how its ids moved. The
    /// ids get the values that binding the file into this arena would give:
    /// every id moves by the number of entries already here. `freeable` is
    /// as in `prepare_file_arena`.
    pub fn append_file_arena(&mut self, file_arena: SymbolArena, freeable: bool) -> ArenaOffsets {
        let offsets = self.next_file_offsets();
        self.append_prepared_file_arena(file_arena.prepare_file_arena(offsets, freeable))
    }

    /// Moves every id in this file arena by `offsets`, in place, and puts
    /// the entries in chunks for `append_prepared_file_arena` with the same
    /// offsets. It can run on another thread once the offsets are known.
    /// The declaration lists of a static file leak (`make_static`), because
    /// its symbols live until exit. A freeable file version (`freeable`,
    /// lsshells M3d) keeps them owned, so they go with its chunks.
    // PORT: the binder writes a symbol id into the names of private
    // identifier symbols (`get_symbol_name_for_private_identifier`), so those
    // names move with the ids. Source text can spell a name of the same form
    // (the byte 0xFE + "#1@#p", see `ast::INTERNAL_SYMBOL_NAME_PREFIX`). Go
    // keeps such a name as written, so only a symbol that a private
    // identifier declares moves, with its entries in the tables. Go gives
    // symbol ids in a different order, so a source name that is equal to a
    // private identifier name in Go is not always equal to it here.
    // PERF: the entries are changed in place and moved by chunk. Rebuilding
    // each symbol in an iterator chain was most of the join time.
    #[must_use]
    pub fn prepare_file_arena(self, offsets: ArenaOffsets, freeable: bool) -> PreparedFileArena {
        let SymbolArena {
            symbols,
            tables,
            private_names,
            ids: _,
            seen: _,
        } = self;
        // The file symbols whose names hold a symbol id.
        // apisym1c: ids move with offset 0 too, from the second chunk on.
        let mut private_symbols = FxHashSet::default();
        if !private_names.is_empty() {
            let private_names: FxHashSet<u32> = private_names.into_iter().collect();
            for i in 1..symbols.len() {
                let symbol = symbols.get(i);
                if private_names.contains(&symbol.name.0)
                    && symbol.declarations.iter().any(|&declaration| {
                        crate::ast::is_private_identifier(crate::ast::get_name_of_declaration(
                            declaration,
                        ))
                    })
                {
                    private_symbols.insert(i as u32);
                }
            }
        }
        // `into_aligned` visits the symbols in order from index 1.
        let mut index = 0u32;
        let symbols = symbols.into_aligned(1, offsets.symbols as usize + 1, |symbol| {
            index += 1;
            if private_symbols.contains(&index) {
                symbol.name = offsets.private_name(&symbol.name);
            }
            symbol.members = offsets.table(symbol.members);
            symbol.exports = offsets.table(symbol.exports);
            symbol.parent = offsets.symbol(symbol.parent);
            symbol.export_symbol = offsets.symbol(symbol.export_symbol);
            // The symbols of a static file live until exit, so their lists
            // can leak.
            if !freeable {
                symbol.declarations.make_static();
            }
        });
        let tables = tables.into_aligned(1, offsets.tables as usize + 1, |table| {
            let mut renamed = false;
            for entry in &mut table.entries {
                if private_symbols.contains(&entry.symbol.0) {
                    let name = offsets.private_name(&Name(entry.name));
                    if name.0 != entry.name {
                        renamed = true;
                        entry.name = name.0;
                    }
                }
                entry.symbol = offsets.symbol(entry.symbol);
            }
            if renamed {
                table.reindex();
            }
        });
        PreparedFileArena {
            symbols,
            tables,
            offsets,
        }
    }

    /// Appends a file arena that `prepare_file_arena` prepared with the
    /// offsets that `next_file_offsets` gives now, and returns them. The
    /// new chunks are owned; the caller publishes them (`freeze_since` or
    /// `share_since`).
    pub fn append_prepared_file_arena(&mut self, prepared: PreparedFileArena) -> ArenaOffsets {
        let offsets = self.next_file_offsets();
        assert_eq!(
            offsets, prepared.offsets,
            "file arena prepared for other offsets"
        );
        // The entries were moved on the bind thread; their chunks stay in
        // use here.
        self.symbols.append_aligned(prepared.symbols);
        self.tables.append_aligned(prepared.tables);
        offsets
    }

    /// Go `maps.Clone(table)`. A nil table clones to nil.
    pub fn clone_table(&mut self, table: SymbolTable) -> SymbolTable {
        if table.is_nil() {
            return SymbolTable::NIL;
        }
        let cloned = self.tables.get(table.index()).clone();
        self.push_table(cloned)
    }

    #[inline(always)]
    #[must_use]
    pub fn sym(&self, symbol: SymbolId) -> &Symbol {
        debug_assert!(symbol.is_some(), "nil symbol dereference");
        self.symbols.get(symbol.index())
    }

    #[inline]
    pub fn sym_mut(&mut self, symbol: SymbolId) -> &mut Symbol {
        debug_assert!(symbol.is_some(), "nil symbol dereference");
        self.symbols.get_mut(symbol.index())
    }

    /// Go `table[name]`. A nil table reads as empty.
    #[must_use]
    pub fn get(&self, table: SymbolTable, name: &str) -> SymbolId {
        if table.is_nil() {
            return SymbolId::NIL;
        }
        let table = self.tables.get(table.index());
        if table.entries.is_empty() {
            return SymbolId::NIL;
        }
        table
            .find(lookup_hash(name), name)
            .map_or(SymbolId::NIL, |position| table.entries[position].symbol)
    }

    /// Go `table[name]` for a caller that has a `Name`. It compares ids
    /// instead of texts, with the hash the interner keeps.
    #[must_use]
    pub fn get_name(&self, table: SymbolTable, name: &Name) -> SymbolId {
        if table.is_nil() {
            return SymbolId::NIL;
        }
        let table = self.tables.get(table.index());
        if table.entries.is_empty() {
            return SymbolId::NIL;
        }
        table
            .find_id(name.table_hash(), name.0)
            .map_or(SymbolId::NIL, |position| table.entries[position].symbol)
    }

    /// Go `table[name]` by `get` or `get_name`, whichever `key` holds.
    #[inline]
    #[must_use]
    pub fn get_key(&self, table: SymbolTable, key: TableKey<'_>) -> SymbolId {
        match key {
            TableKey::Text(text) => self.get(table, text),
            TableKey::Name(name) => self.get_name(table, name),
        }
    }

    /// Go `table[name] = symbol`. Panics on a nil table, like Go.
    pub fn set(&mut self, table: SymbolTable, name: impl Into<Name>, symbol: SymbolId) {
        assert!(table.is_some(), "assignment to entry in nil map");
        let name = name.into();
        self.tables.get_mut(table.index()).insert(&name, symbol);
    }

    /// Go `if table[name] == nil { table[name] = symbol }` with one lookup.
    /// Returns true when it stored `symbol`. Panics on a nil table, like Go.
    pub fn set_if_absent(&mut self, table: SymbolTable, name: &Name, symbol: SymbolId) -> bool {
        self.set_if_absent_or(table, name, symbol, |_| false)
    }

    /// Like `set_if_absent`, but it also replaces a stored symbol that
    /// `replace` accepts: Go
    /// `if s := table[name]; s == nil || replace(s) { table[name] = symbol }`.
    /// One lookup. A replaced entry keeps its position.
    pub fn set_if_absent_or(
        &mut self,
        table: SymbolTable,
        name: &Name,
        symbol: SymbolId,
        replace: impl FnOnce(&Symbol) -> bool,
    ) -> bool {
        assert!(table.is_some(), "assignment to entry in nil map");
        let hash = name.table_hash();
        let found = {
            let current = self.tables.get(table.index());
            current
                .find_id(hash, name.0)
                .map(|position| (position, current.entries[position].symbol))
        };
        if let Some((_, old)) = found {
            if old.is_some() && !replace(self.sym(old)) {
                return false;
            }
        }
        // Only a write takes a shared table chunk back, like `set`.
        let current = self.tables.get_mut(table.index());
        match found {
            Some((position, _)) => current.entries[position].symbol = symbol,
            None => current.push(hash, name.0, symbol),
        }
        true
    }

    /// Go `delete(table, name)`.
    pub fn delete(&mut self, table: SymbolTable, name: &str) {
        if table.is_some() {
            self.tables.get_mut(table.index()).remove(name);
        }
    }

    /// Go `len(table)`.
    #[must_use]
    pub fn len(&self, table: SymbolTable) -> usize {
        if table.is_nil() {
            0
        } else {
            self.tables.get(table.index()).entries.len()
        }
    }

    fn table_entries(&self, table: SymbolTable) -> &[TableEntry] {
        if table.is_nil() {
            &[]
        } else {
            &self.tables.get(table.index()).entries
        }
    }

    /// Snapshot of `(name, symbol)` pairs in insertion order. Go map order is
    /// random, so Go code never depends on it; ours is deterministic.
    #[must_use]
    pub fn entries(&self, table: SymbolTable) -> Vec<(Name, SymbolId)> {
        self.table_entries(table)
            .iter()
            .map(|e| (Name(e.name), e.symbol))
            .collect()
    }

    /// Borrowed `(name, symbol)` pairs in insertion order. Use it instead of
    /// `entries` when the table does not change during the loop.
    pub fn iter(&self, table: SymbolTable) -> impl Iterator<Item = (&str, SymbolId)> {
        self.table_entries(table)
            .iter()
            .map(|e| (intern::text(e.name), e.symbol))
    }

    /// Borrowed `(name, symbol)` pairs in insertion order, like `iter`, but
    /// it yields the `Name` and reads no text. Use it when the loop tests
    /// the name by id or bit (`Name::is_internal`) or stores it.
    pub fn iter_names(&self, table: SymbolTable) -> impl Iterator<Item = (Name, SymbolId)> {
        self.table_entries(table)
            .iter()
            .map(|e| (Name(e.name), e.symbol))
    }

    /// Snapshot of the values in insertion order.
    #[must_use]
    pub fn values(&self, table: SymbolTable) -> Vec<SymbolId> {
        self.table_entries(table).iter().map(|e| e.symbol).collect()
    }
}

/// Where a name is, or goes, in one symbol table (`SymbolArena::get_slot`).
#[derive(Clone, Copy, Debug)]
pub struct TableSlot {
    table: SymbolTable,
    hash: u32,
    name: u32,
    position: Option<usize>,
}

/// Symbol and table counts of a `SymbolArena` at one point
/// (`SymbolArena::mark`).
#[derive(Clone, Copy, Debug)]
pub struct ArenaMark {
    symbols: usize,
    tables: usize,
}

/// Prefix of private identifier symbol names (`<prefix>#<id>@<description>`):
/// `ast::INTERNAL_SYMBOL_NAME_PREFIX` + "#".
const PRIVATE_PREFIX: &str = "\u{FDD0}\u{10F7FE}#";

/// A file arena with its ids moved to program ids
/// (`SymbolArena::prepare_file_arena`).
#[derive(Debug)]
pub struct PreparedFileArena {
    symbols: AlignedChunks<Symbol>,
    tables: AlignedChunks<Table>,
    offsets: ArenaOffsets,
}

/// How the ids of a file arena moved in `SymbolArena::append_file_arena`.
/// An offset counts the lineage entries before the file, without the nil
/// entry and the odd chunks (`CowChunks::dense_len`), so a file arena id
/// plus its offset is a dense lineage place (`lineage_index`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ArenaOffsets {
    symbols: u32,
    tables: u32,
}

/// The lineage index of dense lineage place `place`: chunk `n` of the
/// places is the even chunk `2 * n` of the interleaved lineage
/// (`SymbolArena`).
#[inline]
const fn lineage_index(place: u32) -> u32 {
    place + (place & !(COW_CHUNK_MASK as u32))
}

impl ArenaOffsets {
    /// These offsets moved to the next chunk start: the offsets that
    /// `SymbolArena::next_file_offsets` gives after `SymbolArena::end_chunk`.
    /// Parallel binding gives a freeable file version, and the file after
    /// it, these offsets (lsshells M3d).
    #[must_use]
    pub fn aligned(self) -> ArenaOffsets {
        // An offset is the dense place of the next entry minus 1 (the nil
        // entry). A dense chunk start is an even lineage chunk start.
        let align = |offset: u32| {
            let next = (offset as usize + 1).next_multiple_of(COW_CHUNK_LEN);
            u32::try_from(next - 1).expect("arena overflow")
        };
        ArenaOffsets {
            symbols: align(self.symbols),
            tables: align(self.tables),
        }
    }

    /// The offsets of the file after a file with these offsets, where
    /// `file_arena` is the `SymbolArena::mark` of that file's own arena.
    /// Parallel binding adds the counts of the earlier files this way.
    #[must_use]
    pub fn after(self, file_arena: ArenaMark) -> ArenaOffsets {
        let count = |len: usize| u32::try_from(len - 1).expect("arena overflow");
        ArenaOffsets {
            symbols: self
                .symbols
                .checked_add(count(file_arena.symbols))
                .expect("symbol overflow"),
            tables: self
                .tables
                .checked_add(count(file_arena.tables))
                .expect("table overflow"),
        }
    }

    /// The program id of file arena symbol `symbol`.
    #[must_use]
    pub fn symbol(self, symbol: SymbolId) -> SymbolId {
        if symbol.is_nil() {
            symbol
        } else {
            SymbolId(lineage_index(symbol.0 + self.symbols))
        }
    }

    /// The program id of file arena table `table`.
    #[must_use]
    pub fn table(self, table: SymbolTable) -> SymbolTable {
        if table.is_nil() {
            table
        } else {
            SymbolTable(lineage_index(table.0 + self.tables))
        }
    }

    /// The private identifier name `name` (`<prefix>#<id>@<description>`,
    /// see `SymbolArena::note_private_name`) with its symbol id moved.
    #[cold]
    fn private_name(self, name: &Name) -> Name {
        let Some(rest) = name.strip_prefix(PRIVATE_PREFIX) else {
            return name.clone();
        };
        let Some(at) = rest.find('@') else {
            return name.clone();
        };
        let Ok(id) = rest[..at].parse::<u32>() else {
            return name.clone();
        };
        let moved = self.symbol(SymbolId(id));
        Name::from(format!("{PRIVATE_PREFIX}{}{}", moved.0, &rest[at..]))
    }
}

/// Go `*ast.FlowNode`. `antecedents` replaces the Go `FlowList` linked list
/// in the same order.
#[derive(Clone, Debug, Default)]
pub struct FlowNode {
    pub flags: FlowFlags,
    pub node: Node,
    pub antecedent: FlowNodeId,
    pub antecedents: Vec<FlowNodeId>,
}

/// Go `ast.PatternAmbientModule`.
#[derive(Clone, Debug)]
pub struct PatternAmbientModule {
    pub pattern_prefix: String,
    pub pattern_suffix: String,
    pub symbol: SymbolId,
}

/// Binder output for one node. Go stores these on the node itself.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NodeBindData {
    pub symbol: SymbolId,
    pub local_symbol: SymbolId,
    pub locals: SymbolTable,
    pub next_container: Node,
    pub flow_node: FlowNodeId,
    pub end_flow_node: FlowNodeId,
    pub return_flow_node: FlowNodeId,
    /// Go `NodeFlags` bits the binder ORs into `node.Flags`.
    pub added_flags: NodeFlags,
}

/// AST node records, step 2: the binder fields of one node that are not in
/// its `NodeRecord` (`ast/store.rs`, which holds the symbol, the added
/// flags and, for a node with no entry here, the flow node). Most nodes
/// have none of them: only locals containers, exported declarations and
/// function-like nodes do.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NodeBindExtra {
    pub local_symbol: SymbolId,
    pub locals: SymbolTable,
    pub next_container: Node,
    pub end_flow_node: FlowNodeId,
    pub return_flow_node: FlowNodeId,
    /// Go `FlowNodeData().FlowNode` of the node: astmem1 P3, its record
    /// has the extras index in place of the flow node (`ast::BIND_EXTRA`).
    /// The install sets it (`ast::bind_store_records`); `of` leaves it nil.
    pub flow_node: FlowNodeId,
}

impl NodeBindExtra {
    /// The fields of `data` that are not in the record, with a nil flow
    /// node.
    #[inline]
    #[must_use]
    pub fn of(data: &NodeBindData) -> Self {
        NodeBindExtra {
            local_symbol: data.local_symbol,
            locals: data.locals,
            next_container: data.next_container,
            end_flow_node: data.end_flow_node,
            return_flow_node: data.return_flow_node,
            flow_node: FlowNodeId::NIL,
        }
    }

    /// The empty fields: a node with no extras entry.
    pub const NONE: Self = NodeBindExtra {
        local_symbol: SymbolId::NIL,
        locals: SymbolTable::NIL,
        next_container: Node::NIL,
        end_flow_node: FlowNodeId::NIL,
        return_flow_node: FlowNodeId::NIL,
        flow_node: FlowNodeId::NIL,
    };
}

/// AST node records, step 2: the binder fields of the nodes of one bound
/// file that are not in their records (`NodeBindExtra`). The `bind` word of
/// a record holds the index + 1 of its entry with `ast::BIND_EXTRA`, or a
/// flow node (`FileNodeBind::extra`).
/// `BoundFile::install` makes it (`ast::bind_store_records`).
#[derive(Debug, Default)]
pub struct FileNodeBind {
    extras: Box<[NodeBindExtra]>,
}

impl FileNodeBind {
    /// The extras entries that the install made.
    #[must_use]
    pub fn new(extras: Vec<NodeBindExtra>) -> Self {
        FileNodeBind {
            extras: extras.into_boxed_slice(),
        }
    }

    /// The extras entry whose index + 1 is `extra`: the `bind` word of a
    /// record that has `ast::BIND_EXTRA`, with that bit cleared (never 0).
    #[inline]
    #[must_use]
    pub fn extra(&self, extra: u32) -> &NodeBindExtra {
        &self.extras[extra as usize - 1]
    }

    /// The extras entries.
    #[must_use]
    pub fn extras(&self) -> &[NodeBindExtra] {
        &self.extras
    }
}

/// The binder data of every node of one file, stored compactly, as the lib
/// bind snapshot keeps it (`parts`, `binder::BoundNodes`). Most nodes have
/// no data, or the same data as the node before them (a run of identifiers
/// in one flow region), so they share one entry. Each node keeps one byte:
/// the offset of its entry from the first entry of its block. The install
/// writes it into the node records and a `FileNodeBind`
/// (`ast::bind_store_records`).
#[derive(Clone, Debug, Default)]
pub struct NodeBindParts {
    /// Per node, by `NodeId::index()`: entry offset in its block, or
    /// `NO_NODE_BIND` for the empty data.
    slots: Vec<u8>,
    /// Per block of `NODE_BIND_BLOCK` nodes: the index of its first entry.
    bases: Vec<u32>,
    entries: Vec<NodeBindData>,
}

const NODE_BIND_BLOCK_BITS: usize = 7;
const NODE_BIND_BLOCK: usize = 1 << NODE_BIND_BLOCK_BITS;
const NO_NODE_BIND: u8 = u8::MAX;

/// The empty binder data.
static EMPTY_NODE_BIND: NodeBindData = NodeBindData {
    symbol: SymbolId::NIL,
    local_symbol: SymbolId::NIL,
    locals: SymbolTable::NIL,
    next_container: Node::NIL,
    flow_node: FlowNodeId::NIL,
    end_flow_node: FlowNodeId::NIL,
    return_flow_node: FlowNodeId::NIL,
    added_flags: NodeFlags::NONE,
};

impl NodeBindParts {
    /// Compacts the per-node data of a bound file, given in node order.
    /// `None` is a node with no data. At most `max_entries` items are
    /// `Some`.
    // PERF: bind C. The binder keeps flow nodes in their own per-node array
    // (`NodeBindBuilder`) and merges them here. A node with no data and no
    // flow node is `None`, so it costs one test and no 40-byte compare, and
    // its slot is already `NO_NODE_BIND` from the fill.
    #[must_use]
    pub fn new(
        nodes: impl ExactSizeIterator<Item = Option<NodeBindData>>,
        max_entries: usize,
    ) -> Self {
        let mut slots = vec![NO_NODE_BIND; nodes.len()];
        let mut bases = Vec::with_capacity(nodes.len().div_ceil(NODE_BIND_BLOCK));
        // The entries do not grow and copy step by step. `shrink_to_fit`
        // below gives back the unused part.
        let mut entries: Vec<NodeBindData> = Vec::with_capacity(max_entries.min(nodes.len()));
        let mut block_start = 0;
        for (index, data) in nodes.enumerate() {
            if index & (NODE_BIND_BLOCK - 1) == 0 {
                block_start = entries.len();
                bases.push(u32::try_from(block_start).expect("node bind overflow"));
            }
            let Some(data) = data else { continue };
            if data == EMPTY_NODE_BIND {
                continue;
            }
            // Entries are shared only inside a block, so offsets stay small.
            if entries.len() == block_start || entries.last() != Some(&data) {
                entries.push(data);
            }
            slots[index] = u8::try_from(entries.len() - 1 - block_start).expect("block offset");
        }
        entries.shrink_to_fit();
        NodeBindParts {
            slots,
            bases,
            entries,
        }
    }

    /// Each node that has data, in node order: its `NodeId::index()`, the
    /// index of its entry (nodes that share an entry give the same index)
    /// and the data.
    pub fn nodes(&self) -> impl Iterator<Item = (usize, usize, &NodeBindData)> {
        self.slots
            .iter()
            .enumerate()
            .filter(|&(_, &offset)| offset != NO_NODE_BIND)
            .map(|(index, &offset)| {
                let entry = self.bases[index >> NODE_BIND_BLOCK_BITS] as usize + offset as usize;
                (index, entry, &self.entries[entry])
            })
    }

    /// The distinct data entries, for remapping ids in place. Non-empty data
    /// stays non-empty and distinct under an id remap.
    pub fn entries_mut(&mut self) -> &mut [NodeBindData] {
        &mut self.entries
    }

    /// The stored form: the per-node slots, the block bases and the
    /// entries. The lib bind snapshot (`binder/lib_snapshot.rs`) dumps it.
    #[must_use]
    pub fn parts(&self) -> (&[u8], &[u32], &[NodeBindData]) {
        (&self.slots, &self.bases, &self.entries)
    }

    /// The value whose `parts` these are. `None` when they do not fit
    /// together: a base count that does not match the slots, or a base or
    /// slot that points past the entries of its block.
    #[must_use]
    pub fn from_parts(slots: Vec<u8>, bases: Vec<u32>, entries: Vec<NodeBindData>) -> Option<Self> {
        if bases.len() != slots.len().div_ceil(NODE_BIND_BLOCK) {
            return None;
        }
        for (block, block_slots) in slots.chunks(NODE_BIND_BLOCK).enumerate() {
            // `new` gives a block the entries from its base to the next base.
            let start = bases[block] as usize;
            let end = bases
                .get(block + 1)
                .map_or(entries.len(), |&next| next as usize);
            let count = end.checked_sub(start)?;
            if end > entries.len()
                || block_slots
                    .iter()
                    .any(|&slot| slot != NO_NODE_BIND && usize::from(slot) >= count)
            {
                return None;
            }
        }
        Some(NodeBindParts {
            slots,
            bases,
            entries,
        })
    }
}

/// Binder output for one source file. Go stores these on `ast.SourceFile`.
#[derive(Clone, Debug, Default)]
pub struct FileBindData {
    pub bind_diagnostics: Vec<Diagnostic>,
    pub symbol_count: i32,
    // PORT: Go also keeps `ClassifiableNames` here. Nothing reads it, so the
    // binder does not collect it.
    pub pattern_ambient_modules: Vec<PatternAmbientModule>,
    pub global_exports: SymbolTable,
    /// Go `JSGlobalAugmentations`.
    pub js_global_augmentations: SymbolTable,
    /// Go `SourceFile.CommonJSModuleIndicator` (set by the binder).
    pub common_js_module_indicator: Node,
    /// Rust-only: the binder gave an expando assignment (Go
    /// `bindDeferredExpandoAssignment`) a symbol. When false, no node of the
    /// file has an `ASSIGNMENT` symbol, so the declaration transformer skips
    /// its expando walk (`transform_source_file`).
    pub has_expando_assignments: bool,
}

/// Go `*ast.Diagnostic`. Positions are Go positions (UTF-8 byte offsets).
/// `file` is the source file node, or nil for a global diagnostic.
#[derive(Clone, Debug)]
pub struct Diagnostic {
    pub file: Node,
    pub pos: i32,
    pub end: i32,
    pub code: i32,
    pub category: crate::diagnostics::Category,
    /// Go `source` (tsgo#4712). When non-empty, a custom prefix (e.g. a
    /// content mapper's name) shown instead of "TS" before the code. It
    /// marks the diagnostic as coming from an external source whose ranges
    /// point into the file's original, untransformed text.
    pub source: String,
    /// Go `message`. The Go nil message is `crate::ast::NIL_MESSAGE`.
    pub message: &'static crate::diagnostics::Message,
    /// Go `messageText` (tsgo#4712): an already-localized message used when
    /// the message is nil, e.g. a diagnostic deserialized from an external
    /// process that owns its own localization.
    pub message_text: String,
    pub message_args: Vec<String>,
    pub message_chain: Vec<Diagnostic>,
    pub related_information: Vec<Diagnostic>,
    pub reports_unnecessary: bool,
    pub reports_deprecated: bool,
    /// Go `skippedOnNoEmit`: dropped from semantic diagnostics when noEmit is set.
    pub skipped_on_no_emit: bool,
    /// Go `repopulateInfo`: lets an incremental build recompute a module
    /// resolution diagnostic chain. `Arc` because diagnostics cross the
    /// checker threads.
    pub repopulate_info: Option<std::sync::Arc<crate::ast::RepopulateDiagnosticInfo>>,
}

/// Go `...any` diagnostic arguments. Go formats each with `%v`; we use
/// `ToString`. Example: `args![self.type_to_string(t), count]`.
#[macro_export]
macro_rules! args {
    ($($arg:expr),* $(,)?) => { vec![$(::std::string::ToString::to_string(&$arg)),*] };
}

/// Where a `LinkStore` keeps the record of a key.
#[derive(Clone, Copy, Debug)]
pub enum LinkSlot {
    /// An arena handle (`TypeId`, ...): an index into one page table for
    /// the whole arena.
    Arena(usize),
    /// A binder lineage index (`SymbolId`). The lineage never reuses an
    /// index, so in a long watch or editor session most indexes belong to
    /// dead file versions, which a new checker never reads. Indexes below
    /// `LINK_FLAT_IDS` use the arena table, as `Arena` does. The others use
    /// one page table per block of indexes that the store reads
    /// (`LinkStore::far_page`), so a store pays only for the blocks it uses.
    Lineage(usize),
    /// A node or flow node handle: (file, local index + 1). Each file has
    /// its own page table.
    File(usize, usize),
    /// Any other key (a synthetic node): the hash map.
    Map,
}

impl LinkSlot {
    /// The index of a key with slots in its table. Its place in its page is
    /// `index % LINK_PAGE_SIZE`, also for a lineage key in a block table, as
    /// `LINK_FLAT_IDS` and the block size are multiples of the page size.
    #[inline(always)]
    fn index(self) -> usize {
        match self {
            LinkSlot::Arena(index) | LinkSlot::Lineage(index) | LinkSlot::File(_, index) => index,
            LinkSlot::Map => unreachable!("map keys have no slot"),
        }
    }
}

/// A `LinkStore` key. Arena handles are dense small indexes, and node and
/// flow node handles are dense small indexes within a file, so their links
/// live in pages instead of a hash map.
pub trait LinkKey: Copy + Eq + std::hash::Hash {
    /// Where the record of this key lives.
    fn link_slot(self) -> LinkSlot;
}

macro_rules! arena_link_key {
    ($($name:ident),*) => {$(
        impl LinkKey for $name {
            #[inline]
            fn link_slot(self) -> LinkSlot {
                LinkSlot::Arena(self.index())
            }
        }
    )*};
}

arena_link_key!(
    TypeId,
    SignatureId,
    IndexInfoId,
    TypePredicateId,
    MapperId,
    InferenceContextId,
    SymbolTable
);

impl LinkKey for SymbolId {
    #[inline]
    fn link_slot(self) -> LinkSlot {
        LinkSlot::Lineage(self.index())
    }
}

/// Lineage indexes below this use the flat arena table: at most 512 KiB of
/// page pointers per store. A CLI build stays below it, so its reads are the
/// reads of an `Arena` key. (wgrowth1: a flat table for all indexes grew by
/// 8 bytes per 64 indexes ever bound, in each symbol link store of each
/// checker: about 7 MiB per config edit of a 600-file project.)
const LINK_FLAT_IDS: usize = 1 << 22;

/// Lineage indexes from `LINK_FLAT_IDS` on are in blocks of this many, with
/// one page table per block.
const LINK_BLOCK_SHIFT: usize = 16;

const _: () = assert!(
    LINK_FLAT_IDS % LINK_PAGE_SIZE == 0 && (1 << LINK_BLOCK_SHIFT) % LINK_PAGE_SIZE == 0,
    "a block page holds the keys of one flat page (`LinkSlot::index`)"
);

/// The block (an index into `LinkStore::files`) and the index in its block
/// of lineage index `index`. An index below `LINK_FLAT_IDS` wraps to a block
/// past every block table, so a lookup of it finds no page.
#[inline]
fn far_slot(index: usize) -> (usize, usize) {
    let far = index.wrapping_sub(LINK_FLAT_IDS);
    (far >> LINK_BLOCK_SHIFT, far & ((1 << LINK_BLOCK_SHIFT) - 1))
}

/// Files with an index below this get paged node slots. Synthetic nodes
/// (a file index near `u32::MAX`) use the hash map.
const LINK_MAX_FILES: u64 = 1 << 20;

/// The slot of a (file << 32 | local + 1) handle.
#[inline]
fn file_link_slot(handle: u64) -> LinkSlot {
    let file = handle >> 32;
    if file < LINK_MAX_FILES {
        LinkSlot::File(file as usize, (handle & 0xffff_ffff) as usize)
    } else {
        LinkSlot::Map
    }
}

impl LinkKey for Node {
    #[inline]
    fn link_slot(self) -> LinkSlot {
        file_link_slot(self.0)
    }
}

impl LinkKey for FlowNodeId {
    #[inline]
    fn link_slot(self) -> LinkSlot {
        file_link_slot(self.0)
    }
}

/// Keys per page. Pages are made on first use, so a store that few keys use
/// stays small.
const LINK_PAGE_SIZE: usize = 1 << 6;

/// The records of the `LINK_PAGE_SIZE` keys of one page of a `LinkStore`.
/// `DenseLinkPage` holds a place for every key, `SparseLinkPage` only the
/// records that its keys have. `index` is the index of a key of the page in
/// its table (`LinkSlot`); the page uses `index % LINK_PAGE_SIZE`.
pub trait LinkPage<V: Default> {
    /// A page with no record.
    fn new_page() -> Box<Self>;
    /// The record of key `index`.
    fn record(&self, index: usize) -> Option<&V>;
    /// The record of key `index`, made with `V::default()` if it has none.
    fn record_or_default(&mut self, index: usize) -> &mut V;
    /// Adds the record of key `index`, which has none, with `value`.
    fn insert_record(&mut self, index: usize, value: V) -> &mut V;
}

/// The records of `LINK_PAGE_SIZE` keys in place, `None` for a key with no
/// record. A page costs 64 values, used or not, so a value must be 32 bytes
/// or less (a page of 2 KiB at most).
pub type DenseLinkPage<V> = [Option<V>; LINK_PAGE_SIZE];

impl<V: Default> LinkPage<V> for DenseLinkPage<V> {
    fn new_page() -> Box<Self> {
        const {
            assert!(
                size_of::<Option<V>>() <= 32,
                "a link value over 32 bytes goes in a Box or a SparseLinkStore"
            );
        }
        Box::new(std::array::from_fn(|_| None))
    }

    #[inline(always)]
    fn record(&self, index: usize) -> Option<&V> {
        self[index % LINK_PAGE_SIZE].as_ref()
    }

    #[inline(always)]
    fn record_or_default(&mut self, index: usize) -> &mut V {
        let cell = &mut self[index % LINK_PAGE_SIZE];
        match cell {
            Some(value) => value,
            None => new_link_record(cell),
        }
    }

    #[inline(always)]
    fn insert_record(&mut self, index: usize, value: V) -> &mut V {
        self[index % LINK_PAGE_SIZE].insert(value)
    }
}

/// The records of the keys of a page that have one, in the order they were
/// made, and the place of each key's record. A page costs 88 bytes and its
/// records, so it suits a store where few keys of a page have a record
/// (lspage1: 2% to 30% in most checker stores). A read is one more load
/// than a `DenseLinkPage` read, and a record of any size stays in place.
#[derive(Clone, Debug)]
pub struct SparseLinkPage<V> {
    records: Vec<V>,
    /// For each key of the page, 1 + the index of its record in `records`,
    /// or 0 when it has none.
    places: [u8; LINK_PAGE_SIZE],
}

impl<V> SparseLinkPage<V> {
    /// The capacity of `records` in a new page: 64 bytes, and at least 4
    /// records. Then it doubles, as a `Vec` does.
    // PERF: each growth is a `realloc`. Growing one record at a time cost
    // 0.5% more instructions on zod; from 4 records, about 0.1% more on
    // effect than from 64 bytes.
    const FIRST: usize = {
        let size = if size_of::<V>() == 0 {
            1
        } else {
            size_of::<V>()
        };
        if 64 / size > 4 { 64 / size } else { 4 }
    };

    /// Adds the record of key `index`, which has none.
    // PERF: out of line, so an inlined `get` stays small.
    #[inline(never)]
    fn push(&mut self, index: usize, value: V) -> &mut V {
        self.records.push(value);
        let place = self.records.len();
        self.places[index % LINK_PAGE_SIZE] = place as u8;
        &mut self.records[place - 1]
    }
}

impl<V: Default> LinkPage<V> for SparseLinkPage<V> {
    fn new_page() -> Box<Self> {
        // A page is made for a new record, so its records are allocated now.
        Box::new(Self {
            records: Vec::with_capacity(Self::FIRST),
            places: [0; LINK_PAGE_SIZE],
        })
    }

    #[inline(always)]
    fn record(&self, index: usize) -> Option<&V> {
        // Place 0 (no record) wraps to an index past every record.
        self.records
            .get(usize::from(self.places[index % LINK_PAGE_SIZE]).wrapping_sub(1))
    }

    #[inline(always)]
    fn record_or_default(&mut self, index: usize) -> &mut V {
        let at = usize::from(self.places[index % LINK_PAGE_SIZE]).wrapping_sub(1);
        if at < self.records.len() {
            return &mut self.records[at];
        }
        self.push(index, V::default())
    }

    #[inline(always)]
    fn insert_record(&mut self, index: usize, value: V) -> &mut V {
        self.push(index, value)
    }
}

/// A page table: for each page of `LINK_PAGE_SIZE` keys, the page, or `None`
/// when no key of the page has a record.
type PageTable<P> = Vec<Option<Box<P>>>;

/// Makes the record of a key that has none.
// PERF: out of line, so an inlined `get` stays small; not `#[cold]`, because
// `get` calls it for every new record.
#[inline(never)]
fn new_link_record<V: Default>(cell: &mut Option<V>) -> &mut V {
    cell.insert(V::default())
}

/// Go `core.LinkStore[K, V]`: lazily created per-key link records.
/// Dense keys keep their records in pages (Go `core.PagedLinkStore`,
/// tsgo#4329). Other keys use a hash map. `get` reads an existing record
/// inline and adds a new page out of line.
///
/// A key finds its page in 3 dependent loads (the list of file tables, the
/// table of the file, the page) for a node key, 2 for an arena key. The page
/// kind `P` is set by the store:
/// - `DenseLinkPage` (the default) holds the values in place, so a read is
///   one more load. Each page costs 64 values, used or not, so it suits
///   stores where most keys of a page have a record, of 32 bytes or less.
///   Put a larger value in a `Box` (`LinkStore<Node, Box<V>>`, like Go
///   `symbolArenaLinkStore`): a page then costs 8 bytes per key, and a read
///   one more load.
/// - `SparseLinkPage` (`SparseLinkStore`) holds only the records that its
///   keys have, of any size: a read is two more loads.
#[derive(Clone, Debug)]
pub struct LinkStore<K: LinkKey, V: Default, P: LinkPage<V> = DenseLinkPage<V>> {
    /// Page table of arena keys: one flat table for the whole arena (for
    /// lineage keys, up to `LINK_FLAT_IDS`).
    arena: PageTable<P>,
    /// Page tables of file keys, by file. A store of lineage keys has no
    /// file keys: it keeps the page tables of its blocks here (`far_slot`).
    files: Vec<PageTable<P>>,
    map: FxHashMap<K, V>,
}

/// A `LinkStore` of `SparseLinkPage`s, for records that few keys of a page
/// have (lspage1: on VS Code, 7% of the keys of a `type_node_links` page).
// PORT: Go keeps one page kind (`core.PagedLinkStore`); the page kind does
// not change which keys have a record.
pub type SparseLinkStore<K, V> = LinkStore<K, V, SparseLinkPage<V>>;

impl<K: LinkKey, V: Default, P: LinkPage<V>> Default for LinkStore<K, V, P> {
    fn default() -> Self {
        Self {
            arena: Vec::new(),
            files: Vec::new(),
            map: FxHashMap::default(),
        }
    }
}

impl<K: LinkKey, V: Default, P: LinkPage<V>> LinkStore<K, V, P> {
    /// The page of a key with slots, or `None` when it is absent.
    // PERF: a lineage key inside the flat table reads like an arena key.
    #[inline(always)]
    fn page(&self, slot: LinkSlot) -> Option<&P> {
        let (table, index) = match slot {
            LinkSlot::Arena(index) => (&self.arena, index),
            LinkSlot::Lineage(index) if index / LINK_PAGE_SIZE < self.arena.len() => {
                (&self.arena, index)
            }
            LinkSlot::Lineage(index) => return self.far_page(index),
            LinkSlot::File(file, index) => (self.files.get(file)?, index),
            LinkSlot::Map => return None,
        };
        table.get(index / LINK_PAGE_SIZE)?.as_deref()
    }

    /// `page` of a lineage key past the flat table: its block page, or
    /// `None` when the page is absent or the key belongs in the flat table.
    #[inline(never)]
    fn far_page(&self, index: usize) -> Option<&P> {
        self.far_lookup(index)
    }

    #[inline(always)]
    fn far_lookup(&self, index: usize) -> Option<&P> {
        let (block, index) = far_slot(index);
        self.files
            .get(block)?
            .get(index / LINK_PAGE_SIZE)?
            .as_deref()
    }

    /// `page_mut` of a lineage key past the flat table.
    // PERF: the page lookup runs twice, as in `page_mut`, and LLVM merges
    // them. `#[cold]` keeps the callers laid out for flat keys, as a CLI
    // build has only those; a long session that passes `LINK_FLAT_IDS` pays
    // the call.
    #[cold]
    #[inline(never)]
    fn far_page_mut(&mut self, key: K, index: usize) -> &mut P {
        if self.far_lookup(index).is_none() {
            return self.add_page(key);
        }
        let (block, index) = far_slot(index);
        self.files[block][index / LINK_PAGE_SIZE]
            .as_deref_mut()
            .expect("link page")
    }

    /// The page of a key with slots. Adds the page if it is absent.
    // PERF: the page lookup runs twice in the source (a shared lookup, then
    // the mutable one, which the borrow checker needs), but the second one
    // reads the same memory with no store between, so LLVM merges them.
    // A lineage key past the flat table goes to `far_page_mut` first, so
    // `page` takes only its flat arm here and the merge above still holds.
    #[inline(always)]
    fn page_mut(&mut self, key: K, slot: LinkSlot) -> &mut P {
        if let LinkSlot::Lineage(index) = slot
            && index / LINK_PAGE_SIZE >= self.arena.len()
        {
            return self.far_page_mut(key, index);
        }
        if self.page(slot).is_none() {
            return self.add_page(key);
        }
        let (table, index) = match slot {
            LinkSlot::Arena(index) | LinkSlot::Lineage(index) => (&mut self.arena, index),
            LinkSlot::File(file, index) => (&mut self.files[file], index),
            LinkSlot::Map => unreachable!("map keys have no slot"),
        };
        table[index / LINK_PAGE_SIZE]
            .as_deref_mut()
            .expect("link page")
    }

    /// `page_mut` when the page of the key is absent: adds the page (and the
    /// file or block table) first. The key, not its 24-byte `LinkSlot`, is
    /// passed, so it goes in a register.
    #[cold]
    #[inline(never)]
    fn add_page(&mut self, key: K) -> &mut P {
        let (file, index) = match key.link_slot() {
            LinkSlot::Arena(index) => return Self::table_page(&mut self.arena, index),
            LinkSlot::Lineage(index) if index < LINK_FLAT_IDS => {
                // The flat table stops at `LINK_FLAT_IDS`; so does its capacity.
                let pages = index / LINK_PAGE_SIZE + 1;
                if pages > self.arena.capacity() {
                    let capacity =
                        (2 * self.arena.capacity()).clamp(pages, LINK_FLAT_IDS / LINK_PAGE_SIZE);
                    self.arena.reserve_exact(capacity - self.arena.len());
                }
                return Self::table_page(&mut self.arena, index);
            }
            LinkSlot::Lineage(index) => far_slot(index),
            LinkSlot::File(file, index) => (file, index),
            LinkSlot::Map => unreachable!("map keys have no slot"),
        };
        if file >= self.files.len() {
            self.files.resize_with(file + 1, Vec::new);
        }
        Self::table_page(&mut self.files[file], index)
    }

    /// The page of `index` in `table`, added if it is absent.
    fn table_page(table: &mut PageTable<P>, index: usize) -> &mut P {
        let entry = index / LINK_PAGE_SIZE;
        if entry >= table.len() {
            table.resize_with(entry + 1, || None);
        }
        table[entry].get_or_insert_with(P::new_page)
    }

    /// Go `store.Get(key)`: creates the record on first use.
    #[inline]
    pub fn get(&mut self, key: K) -> &mut V {
        let slot = key.link_slot();
        if matches!(slot, LinkSlot::Map) {
            return self.map_get(key);
        }
        self.page_mut(key, slot).record_or_default(slot.index())
    }

    #[cold]
    #[inline(never)]
    fn map_get(&mut self, key: K) -> &mut V {
        self.map.entry(key).or_default()
    }

    /// Go `store.Get(key)` followed by writes to the new record, for a key
    /// that has no record (a symbol or type made just before): adds the
    /// record with `value` and returns it.
    // PERF: an inlined caller builds `value` in its dense page slot, and the
    // default record is not written first.
    #[inline(always)]
    pub fn insert_new(&mut self, key: K, value: V) -> &mut V {
        debug_assert!(!self.has(key), "insert_new on a key with a record");
        let slot = key.link_slot();
        if matches!(slot, LinkSlot::Map) {
            return self.map_insert_new(key, value);
        }
        self.page_mut(key, slot).insert_record(slot.index(), value)
    }

    #[cold]
    #[inline(never)]
    fn map_insert_new(&mut self, key: K, value: V) -> &mut V {
        self.map.entry(key).insert_entry(value).into_mut()
    }

    /// Go `store.Has(key)`.
    #[inline]
    #[must_use]
    pub fn has(&self, key: K) -> bool {
        match key.link_slot() {
            LinkSlot::Map => self.map.contains_key(&key),
            slot => self
                .page(slot)
                .is_some_and(|page| page.record(slot.index()).is_some()),
        }
    }

    /// Go `store.TryGet(key)`.
    #[inline]
    #[must_use]
    pub fn try_get(&self, key: K) -> Option<&V> {
        match key.link_slot() {
            LinkSlot::Map => self.map.get(&key),
            slot => self.page(slot)?.record(slot.index()),
        }
    }
}

// PORT: no Go counterpart. The filter of a symbol table (`Table::filter`)
// never hides an entry: every way to add, remove or rename entries keeps it.
#[cfg(test)]
mod table_filter_tests {
    use super::*;

    #[test]
    fn table_filter_keeps_every_entry_findable() {
        let mut arena = SymbolArena::new();
        let name = |n: usize| Name::from(format!("corefix1_name_{n}").as_str());
        for size in [0, 1, 7, 8, 9, 40, 255, 256, 300] {
            let set = arena.new_table();
            let built =
                arena.push_table_from_entries((0..size).map(|n| (name(n), SymbolId(n as u32 + 1))));
            for n in 0..size {
                arena.set(set, name(n), SymbolId(n as u32 + 1));
            }
            for n in (0..size).step_by(3) {
                arena.delete(set, name(n).as_str());
            }
            for n in 0..size + 50 {
                let expected = if n < size {
                    SymbolId(n as u32 + 1)
                } else {
                    SymbolId::NIL
                };
                assert_eq!(
                    arena.get_name(built, &name(n)),
                    expected,
                    "built {size} {n}"
                );
                assert_eq!(
                    arena.get(built, name(n).as_str()),
                    expected,
                    "built {size} {n}"
                );
                let expected = if n % 3 == 0 { SymbolId::NIL } else { expected };
                assert_eq!(arena.get_name(set, &name(n)), expected, "set {size} {n}");
                assert_eq!(arena.get(set, name(n).as_str()), expected, "set {size} {n}");
            }
            let copy = arena.clone_table(set);
            for n in 0..size {
                assert_eq!(
                    arena.get_name(copy, &name(n)),
                    arena.get_name(set, &name(n))
                );
            }
        }
        // A table made with room for 200 entries grows past its index, and
        // `reserve` grows it again.
        let grown = arena.new_table_with_capacity(200);
        for n in 0..300 {
            arena.set(grown, name(n), SymbolId(n as u32 + 1));
            if n == 280 {
                arena.reserve(grown, 1000);
            }
        }
        for n in 0..350 {
            let expected = if n < 300 {
                SymbolId(n as u32 + 1)
            } else {
                SymbolId::NIL
            };
            assert_eq!(arena.get_name(grown, &name(n)), expected, "grown {n}");
            assert_eq!(arena.get(grown, name(n).as_str()), expected, "grown {n}");
        }
    }
}

// PORT: no Go counterpart. The pages of `LinkStore` (AST node records step 7).
#[cfg(test)]
mod link_store_tests {
    use super::*;

    // `get` makes a default record, `insert_new` a record with a value, and
    // `has` and `try_get` see only keys with a record: not the other keys of
    // a page, a file or the map. File keys, synthetic (map) keys and arena
    // keys with a boxed value.
    #[test]
    fn link_store_pages_keep_records_by_key() {
        let node = |file: u64, local: u64| Node(file << 32 | local);
        let mut nodes = LinkStore::<Node, u32>::default();
        let a = node(3, 5);
        assert!(!nodes.has(a) && nodes.try_get(a).is_none());
        *nodes.get(a) = 7;
        assert_eq!(nodes.try_get(a), Some(&7));
        assert!(!nodes.has(node(3, 6)), "a key in the same page");
        assert_eq!(*nodes.get(node(3, 6)), 0, "get makes a default record");
        assert!(nodes.has(node(3, 6)));
        assert!(!nodes.has(node(1, 5)), "a key of another file");
        let far = node(0, 64 * 1000 + 1);
        *nodes.insert_new(far, 9) += 1;
        assert_eq!(nodes.try_get(far), Some(&10));
        let synthetic = node(u64::from(u32::MAX) - 1, 5);
        *nodes.get(synthetic) = 4;
        assert_eq!(nodes.try_get(synthetic), Some(&4));
        assert!(!nodes.has(node(u64::from(u32::MAX) - 1, 6)));

        let mut symbols = LinkStore::<SymbolId, Box<[u64; 8]>>::default();
        symbols.get(SymbolId(70))[1] = 2;
        symbols.insert_new(SymbolId(1), Box::new([1; 8]));
        assert_eq!(symbols.try_get(SymbolId(70)).map(|v| v[1]), Some(2));
        assert_eq!(symbols.try_get(SymbolId(1)).map(|v| v[7]), Some(1));
        assert!(!symbols.has(SymbolId(71)) && !symbols.has(SymbolId(64 * 50)));
    }

    // Symbol keys from `LINK_FLAT_IDS` on keep their records in pages by
    // block: a store with a few far keys has no page table entries for the
    // indexes between them, and every key still finds its own record. Both
    // page kinds.
    #[test]
    fn link_store_far_symbol_keys_use_blocks() {
        far_symbol_keys_use_blocks::<DenseLinkPage<u64>>();
        far_symbol_keys_use_blocks::<SparseLinkPage<u64>>();
    }

    fn far_symbol_keys_use_blocks<P: LinkPage<u64>>() {
        let flat = LINK_FLAT_IDS as u32;
        let block = 1u32 << LINK_BLOCK_SHIFT;
        let mut symbols = LinkStore::<SymbolId, u64, P>::default();
        let keys = [
            SymbolId(5),
            SymbolId(flat - 1),
            SymbolId(flat),
            SymbolId(flat + 70),
            SymbolId(flat + 900 * block + 3),
            SymbolId(u32::MAX - 1),
        ];
        for (n, key) in keys.into_iter().enumerate() {
            assert!(!symbols.has(key) && symbols.try_get(key).is_none());
            *symbols.get(key) = n as u64 + 1;
        }
        *symbols.insert_new(SymbolId(flat + 900 * block + 4), 40) += 2;
        for (n, key) in keys.into_iter().enumerate() {
            assert_eq!(symbols.try_get(key), Some(&(n as u64 + 1)));
            *symbols.get(key) += 10;
            assert_eq!(symbols.try_get(key), Some(&(n as u64 + 11)));
        }
        assert_eq!(symbols.try_get(SymbolId(flat + 900 * block + 4)), Some(&42));
        for absent in [
            6,
            flat - 2,
            flat + 1,
            flat + 64,
            flat + 899 * block,
            u32::MAX - 2,
        ] {
            assert!(!symbols.has(SymbolId(absent)), "{absent}");
        }
        assert_eq!(symbols.arena.len(), LINK_FLAT_IDS / LINK_PAGE_SIZE);
        let far_entries: usize = symbols.files.iter().map(Vec::len).sum();
        assert!(far_entries < 3 * (1 << LINK_BLOCK_SHIFT) / LINK_PAGE_SIZE);
    }

    // A sparse page keeps the record of each key that has one, made by `get`
    // or `insert_new` in any order, up to every key of the page. The other
    // keys of the page and of the next pages have none.
    #[test]
    fn sparse_link_pages_keep_records_by_key() {
        let node = |local: u64| Node(2 << 32 | local);
        let mut nodes = SparseLinkStore::<Node, [u32; 5]>::default();
        let order: Vec<u64> = (0..64).map(|n| 64 + n * 37 % 64).collect();
        for (n, &local) in order.iter().enumerate() {
            assert!(order[n..].iter().all(|&later| !nodes.has(node(later))));
            if n % 2 == 0 {
                nodes.get(node(local))[0] = local as u32;
            } else {
                nodes.insert_new(node(local), [local as u32; 5]);
            }
        }
        for &local in &order {
            nodes.get(node(local))[4] += 1;
            let record = nodes.try_get(node(local)).expect("record");
            assert_eq!(record[0], local as u32);
            assert_eq!(
                record[4],
                if local % 2 == 0 { 1 } else { local as u32 + 1 },
                "{local}"
            );
        }
        assert!(!nodes.has(node(63)) && !nodes.has(node(128)));
        assert!(nodes.try_get(node(200)).is_none());
    }
}

// Go panics and unported hits: `gopanic.rs` in goport_util.
pub use goport_util::core::*;

/// Runs `f` and returns the text of the Go panic that it raises. Fails the
/// test when `f` does not panic or raises a plain Rust panic. Tests of the
/// Go `debug.*` sites (`gostd::debug`) use it.
#[cfg(test)]
pub(crate) fn go_panic_text(f: impl FnOnce()) -> String {
    let payload = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).expect_err("no panic");
    match payload.downcast::<GoPanic>() {
        Ok(p) => p.message,
        Err(_) => panic!("not a GoPanic"),
    }
}

/// One version of a loaded source file: one per file id. The file registry
/// (`ast/store.rs`) owns it from `publish_file_stores` on, or the
/// `FileVersion` of a freeable file version (lsshells M3b); read it with
/// `crate::ast::go_file` (a `FileRef` guard) or `crate::ast::with_go_file`.
/// Program versions that share a file version share this value. Parser
/// data is ready when the file is published. The binder fills the
/// `OnceLock` fields once per file version.
pub struct GoFile {
    /// The `SourceFile` node. Its nodes live in a node store (`ast::store`).
    pub root: Node,
    /// Go `node.Flags` from the parser for each node, indexed by
    /// `NodeId::index()`. It includes the Go parser context flags.
    pub parser_flags: Vec<NodeFlags>,
    /// Go `ast.SourceFile` fields that the parser and program set.
    pub info: crate::program::SourceFileInfo,
    /// The binder fields of the nodes that are not in their node records
    /// (`ast/store.rs`, `NodeRecord`): set by the install with the records.
    pub node_bind: std::sync::OnceLock<FileNodeBind>,
    pub file_bind: std::sync::OnceLock<FileBindData>,
    pub flow_nodes: std::sync::OnceLock<Vec<FlowNode>>,
}

/// Go `Program` as the checker sees it: one per program version (Go makes a
/// new `Program` for each edit). Read the current one with `prog()`. Its
/// files live in the file registry (`crate::ast::go_file`), and versions
/// share the file versions they have in common.
pub struct GoProgram {
    /// Unique in the process, from `next_program_id`. It keys the checker
    /// pool and the frontend of the program on the loading thread.
    pub id: u32,
    /// One leaked copy per distinct options value
    /// (`program::intern_compiler_options`), so program versions with the
    /// same options share it.
    pub options: &'static crate::options::CompilerOptions,
    // The binder symbols of the program and its file order
    // (`source_file_order`) are in its tables (`program::bound_symbols`,
    // lsshells M2c), so a release frees them.
    /// Program state (`program::state()`). Set once, after the files are
    /// published.
    // PORT: the shell (this struct with its state) stays leaked, because
    // checker code keeps `&'static GoProgram` borrows. It holds only small
    // values; the per-version data is in the tables.
    pub(crate) state: std::sync::OnceLock<crate::program::ProgramState>,
}

static NEXT_PROGRAM_ID: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(1);

/// A new `GoProgram::id`: 1 for the first program of the process, then 2, 3...
#[must_use]
pub fn next_program_id() -> u32 {
    NEXT_PROGRAM_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

thread_local! {
    /// The current program of this thread. Const init with no destructor,
    /// so `prog()` reads it with one thread-local load.
    static CURRENT: std::cell::Cell<Option<&'static GoProgram>> =
        const { std::cell::Cell::new(None) };
}

/// The program of a one-program process (`set_prog`). A thread with no
/// current program reads it. A multi-program process never sets it, so a
/// missed `enter_program` panics and does not read the wrong program.
static DEFAULT: std::sync::OnceLock<&'static GoProgram> = std::sync::OnceLock::new();

/// Set by `register_program_version`. Then `set_prog` panics.
static MULTI: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// True in a multi-program process (watch, language server, tests): a
/// program version was registered, and `release_program` can free checkers.
#[inline]
pub fn is_multi_program() -> bool {
    MULTI.load(std::sync::atomic::Ordering::Relaxed)
}

/// Installs the program of a one-program process and makes it current on
/// this thread. Call once. The caller publishes the files of the program
/// first (`crate::ast::publish_file_stores`).
pub fn set_prog(program: &'static GoProgram) {
    assert!(
        !MULTI.load(std::sync::atomic::Ordering::SeqCst),
        "set_prog in a multi-program process"
    );
    assert!(DEFAULT.set(program).is_ok(), "GoProgram already installed");
    CURRENT.set(Some(program));
}

/// Registers a program version of a multi-program process (watch, language
/// server, tests). It does not make `program` current; use `enter_program`.
pub fn register_program_version(program: &'static GoProgram) {
    MULTI.store(true, std::sync::atomic::Ordering::SeqCst);
    assert!(
        DEFAULT.get().is_none(),
        "program version {} registered in a one-program process",
        program.id
    );
}

/// Sets the current program of this thread with no restore. Only for thread
/// setup (`program::WorkerSeed` on checker and bind threads).
pub fn set_thread_program(program: Option<&'static GoProgram>) {
    CURRENT.set(program);
}

/// Makes `program` current on this thread until the scope drops. `None`
/// clears it, for example while the frontend parses a new version.
#[must_use = "the program is current only while the scope lives"]
pub fn enter_program(program: Option<&'static GoProgram>) -> ProgramScope {
    ProgramScope {
        previous: CURRENT.replace(program),
        _not_send: std::marker::PhantomData,
    }
}

/// From `enter_program`. On drop it restores the previous current program.
/// It is `!Send`, so it drops on the thread that made it.
pub struct ProgramScope {
    previous: Option<&'static GoProgram>,
    _not_send: std::marker::PhantomData<*const ()>,
}

impl Drop for ProgramScope {
    fn drop(&mut self) {
        CURRENT.set(self.previous);
    }
}

/// The current program of this thread, or `None`. The ported parser reads
/// nodes before the program exists.
#[inline]
#[must_use]
pub fn try_prog() -> Option<&'static GoProgram> {
    CURRENT.get().or_else(default_prog)
}

/// `try_prog` on a thread with no current program.
#[cold]
#[inline(never)]
fn default_prog() -> Option<&'static GoProgram> {
    DEFAULT.get().copied()
}

/// The current program of this thread. Go code reads nodes without a
/// context; this is how node accessors reach the AST.
#[inline]
#[must_use]
pub fn prog() -> &'static GoProgram {
    try_prog().expect("no current program; use core::enter_program")
}

// Go: core/version.go:8 version
// This is a var so it can be overridden by ldflags.
// PORT: Go release builds set it with `-ldflags -X ...core.version=<v>`
// (Herebyfile.mjs getReleaseBuildFlags). The port reads the build-time env
// var GOPORT_BUILD_VERSION instead (build-release.sh: RELEASE_VERSION).
// Without it the value is Go's default, as in the pinned reference build.
pub(crate) const VERSION: &str = match option_env!("GOPORT_BUILD_VERSION") {
    Some(version) => version,
    None => "7.1.0-dev",
};

// Go: core/version.go:10 Version
pub fn version() -> &'static str {
    VERSION
}

// Go: core/version.go:14 versionMajorMinor
// PORT: a const, so a GOPORT_BUILD_VERSION with no second '.' stops the
// build where Go panics at start.
const VERSION_MAJOR_MINOR: &str = {
    let bytes = VERSION.as_bytes();
    let mut seen_major = false;
    let mut i = 0;
    loop {
        assert!(i < bytes.len(), "invalid version string");
        if bytes[i] == b'.' {
            if seen_major {
                break;
            }
            seen_major = true;
        }
        i += 1;
    }
    VERSION.split_at(i).0
};

// Go: core/version.go:31 VersionMajorMinor
pub fn version_major_minor() -> &'static str {
    VERSION_MAJOR_MINOR
}
