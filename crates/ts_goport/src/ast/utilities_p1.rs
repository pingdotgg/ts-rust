//! Port of typescript-go `internal/ast/utilities.go` lines 1-905.

use crate::prelude::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, PoisonError};

// Atomic ids

// PORT: Go keeps `nextNodeId`/`nextSymbolId`/`nextSymbolBlockId` as
// process-wide atomics and stores the lazily assigned id on the node or
// symbol. Our AST and symbol arena entries have no id slot, so the lazily
// assigned ids live in thread-local maps keyed by handle. Ids are still
// assigned on first request in request order, like Go. Go's
// `nextNodeBlockId` has no port: node link stores give no node ids (their
// ids reach no output).
thread_local! {
    static NEXT_NODE_ID: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    static NEXT_SYMBOL_ID: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    // Go `nextSymbolBlockId`: the start of the next block of symbol ids
    // that a link store takes (`SymbolIdGenerator`).
    static NEXT_SYMBOL_BLOCK_ID: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    // Forgets the ids of dead file versions (`PerFileMap`, lsshells M3a).
    // Their nodes are not read again, and an id is never given twice: a
    // node of a dead version that has no id here panics (`get_node_id`).
    static NODE_IDS: RefCell<PerFileMap<u64>> = const { RefCell::new(PerFileMap::new()) };
    // By the lineage index of a binder symbol (`LineageIds`). Binder symbols
    // only: every arena gives an index the same binder symbol
    // (`SymbolArena::id_slot`, key 0).
    static SYMBOL_IDS: RefCell<LineageIds> = const { RefCell::new(LineageIds::new()) };
    // The ids of the symbols that checker arenas add, by arena.
    static OWN_SYMBOL_IDS: RefCell<OwnSymbolIds> = const { RefCell::new(OwnSymbolIds::new()) };
    // On a checker worker that counts them (`record_symbol_ids`): the block
    // ids that it gave (`SymbolIdGenerator::at_last`). A carry reads it from
    // the log (`IdLog`). None on other threads, which pay nothing per block
    // id.
    static BLOCK_IDS_GIVEN: std::cell::Cell<Option<u64>> = const { std::cell::Cell::new(None) };
    // On a checker worker whose ids go back to the loading thread: the
    // global ids that it gave and the blocks that it took
    // (`record_symbol_ids`).
    static ID_LOG: RefCell<Option<IdLog>> = const { RefCell::new(None) };
    // The lineage ranges whose ids this thread forgot
    // (`forget_lineage_symbol_ids`).
    static FORGOTTEN: RefCell<ForgetLog> = const { RefCell::new(ForgetLog::new()) };
}

/// PORT: not in Go (watchreset1r). The lineage ranges whose ids a thread
/// forgot (`forget_lineage_symbol_ids`), each with the number of the forget
/// (1, 2, ...). A pool notes the count when it is made
/// (`forgotten_count`); its carry drops the ids that its workers have in a
/// range forgotten after that (`carry_symbol_ids`): they are the ids of the
/// symbols of the old parse.
struct ForgetLog {
    count: u64,
    ranges: Vec<(std::ops::Range<usize>, u64)>,
}

impl ForgetLog {
    const fn new() -> Self {
        Self {
            count: 0,
            ranges: Vec::new(),
        }
    }
}

/// The number of forgets on this thread so far
/// (`forget_lineage_symbol_ids`).
#[must_use]
pub fn forgotten_count() -> u64 {
    FORGOTTEN.with(|log| log.borrow().count)
}

/// PORT: not in Go (watchreset1r). Forgets this thread's ids of the binder
/// lineage symbols in `ranges`, so they get ids again as the symbols of a
/// new parse and bind do. `tsc -b --watch` keeps a parse that Go parses
/// again (`program::bind_all`). Go's new symbols have no id, and its
/// counters count on. Unlike `free_lineage_symbol_ids`, the symbols live
/// on, and a later read gives a new id. The log keeps the ranges for the
/// carry of each pool made before this forget (`carry_symbol_ids`); a
/// range of a forget at or before `kept_from` goes: no such pool is left.
pub(crate) fn forget_lineage_symbol_ids(ranges: &[std::ops::Range<usize>], kept_from: u64) {
    FORGOTTEN.with(|log| {
        let mut log = log.borrow_mut();
        log.ranges.retain(|(_, count)| *count > kept_from);
        if ranges.is_empty() {
            return;
        }
        log.count += 1;
        let count = log.count;
        log.ranges
            .extend(ranges.iter().map(|range| (range.clone(), count)));
    });
    SYMBOL_IDS.with(|ids| {
        let mut ids = ids.borrow_mut();
        for range in ranges {
            ids.forget(range.clone());
        }
    });
}

/// The ranges that this thread forgot after forget `seen`, sorted and
/// joined where they touch.
fn forgotten_after(seen: u64) -> Vec<std::ops::Range<usize>> {
    let mut ranges: Vec<std::ops::Range<usize>> = FORGOTTEN.with(|log| {
        log.borrow()
            .ranges
            .iter()
            .filter(|(_, count)| *count > seen)
            .map(|(range, _)| range.clone())
            .collect()
    });
    ranges.sort_unstable_by_key(|range| range.start);
    let mut joined: Vec<std::ops::Range<usize>> = Vec::with_capacity(ranges.len());
    for range in ranges {
        match joined.last_mut() {
            Some(last) if range.start <= last.end => last.end = last.end.max(range.end),
            _ => joined.push(range),
        }
    }
    joined
}

/// Zeroes the cells of `cells`, the id chunk `chunk`, that `ranges`
/// (sorted and apart, `forgotten_after`) hold.
fn drop_forgotten_cells(
    cells: &mut [IdCell; LINEAGE_ID_CHUNK],
    chunk: usize,
    ranges: &[std::ops::Range<usize>],
) {
    let first = chunk << LINEAGE_ID_SHIFT;
    let end = first + LINEAGE_ID_CHUNK;
    let from = ranges.partition_point(|range| range.end <= first);
    for range in ranges[from..].iter().take_while(|range| range.start < end) {
        cells[range.start.max(first) - first..range.end.min(end) - first].fill(0);
    }
}

/// A symbol id as an id table keeps it: 0 for no id yet (Go ids start at
/// 1), `BIG_ID` for an id that has no cell (in the table's `big` map),
/// `BLOCK_CELL | n` for the block id `BLOCK_ID_OFFSET + n`, else the id.
// PERF: 4 bytes per symbol. Each checker gives an id to most symbols that it
// reads, up to 2 million of its own on zod. Most are block ids (Go
// `symbolLinkStore` reads), the rest global ids (Go `GetSymbolId`). Both
// count per thread from 0, so a CLI run stays far below `BIG_ID`.
type IdCell = u32;
const BIG_ID: IdCell = IdCell::MAX;
const BLOCK_CELL: IdCell = 1 << 31;

// Go: ast/utilities.go:48 BlockIdOffset
/// The first block id. A block id is always 15 digits long, and the text
/// of a late-bound name holds it (`__@k@<id>`): the node builder counts its
/// length, and `compare_symbols` compares names as text before ids.
pub const BLOCK_ID_OFFSET: u64 = 0x1_0000_0000_0000;
// Go: ast/utilities.go:49 BlockIdSize (core.LinkPageSize)
const BLOCK_ID_SIZE: u64 = 256;

/// The cell of `id` (not 0): `BIG_ID` when it has none.
#[inline]
fn id_cell(id: u64) -> IdCell {
    if id < u64::from(BLOCK_CELL) {
        return id as IdCell;
    }
    match id.checked_sub(BLOCK_ID_OFFSET) {
        Some(n) if n < u64::from(BIG_ID - BLOCK_CELL) => BLOCK_CELL | n as IdCell,
        _ => BIG_ID,
    }
}

/// The id in `cell`, or None for an empty cell (`cell_id`).
#[inline(always)]
fn known_cell_id<K: Eq + std::hash::Hash>(
    cell: IdCell,
    big: &FxHashMap<K, u64>,
    place: K,
) -> Option<u64> {
    match cell {
        0 => None,
        BIG_ID => Some(big[&place]),
        // A block cell is `BLOCK_CELL | n`: no branch.
        id => Some(u64::from(id & !BLOCK_CELL) + (u64::from(id >> 31) * BLOCK_ID_OFFSET)),
    }
}

/// The id in `cell`, assigned now by `give` when it has none. `big` keeps
/// the ids that have no cell, by `place`.
#[inline(always)]
fn cell_id<K: Eq + std::hash::Hash>(
    cell: &mut IdCell,
    big: &mut FxHashMap<K, u64>,
    place: K,
    give: impl FnOnce() -> u64,
) -> u64 {
    match *cell {
        0 => {
            let id = give();
            *cell = id_cell(id);
            if *cell == BIG_ID {
                big.insert(place, id);
            }
            id
        }
        BIG_ID => big[&place],
        // A block cell is `BLOCK_CELL | n`: no branch.
        id => u64::from(id & !BLOCK_CELL) + (u64::from(id >> 31) * BLOCK_ID_OFFSET),
    }
}

/// The ids of the binder lineage symbols on one thread, by lineage index, in
/// chunks of the symbol chunk size of `SymbolArena` (`LINEAGE_ID_CHUNK`),
/// as `IdCell`s. A chunk is made at the first id in it.
///
/// lsshells M3d: when `program::Lineage` frees the chunks of a dead file
/// version (`free_lineage_symbol_ids`), each thread frees its id chunks of
/// that range the next time it makes a chunk. Only a new chunk grows the
/// table, so the table does not grow with edits. A later id read of a freed
/// chunk panics, as a read of the symbol in the lineage does.
#[derive(Clone, Debug, Default)]
struct LineageIds {
    /// None: no id in this chunk yet, or a freed chunk (`freed`).
    chunks: Vec<Option<Box<[IdCell; LINEAGE_ID_CHUNK]>>>,
    /// The ids that have no cell (`BIG_ID`), by lineage index.
    big: FxHashMap<usize, u64>,
    /// The freed chunks, one bit each.
    freed: Vec<u64>,
    /// The entries of `FREED_LINEAGE` that this table has freed.
    seen: usize,
}

/// The symbols of one `SymbolArena` chunk (core.rs `COW_CHUNK_LEN`), so a
/// freed lineage range frees whole id chunks. A test checks that the two
/// agree (`lineage_id_chunks_are_symbol_chunks`).
// PORT: not shared with core.rs, which is a lib blob key.
const LINEAGE_ID_SHIFT: usize = 8;
const LINEAGE_ID_CHUNK: usize = 1 << LINEAGE_ID_SHIFT;

/// A lineage range that `program::Lineage` freed: the symbol chunks from
/// `first` to `end` of file version `file`.
#[derive(Clone, Copy, Debug)]
struct FreedLineage {
    file: usize,
    first: u32,
    end: u32,
}

/// The lineage ranges freed so far, in order. Each `LineageIds` frees the
/// entries after its `seen` count.
// PERF: one entry per dead file version, so it grows by 16 bytes per edit.
static FREED_LINEAGE: Mutex<Vec<FreedLineage>> = Mutex::new(Vec::new());

/// The length of `FREED_LINEAGE`, so a table with nothing to free does not
/// lock it.
static FREED_LINEAGE_COUNT: AtomicUsize = AtomicUsize::new(0);

/// Notes that `program::Lineage` freed the symbols of file version `file`
/// at lineage indexes `symbols`, whose ends are chunk starts
/// (`SymbolArena::free_range`). Each thread frees its ids of those symbols
/// when it next makes an id chunk (`LineageIds`).
pub(crate) fn free_lineage_symbol_ids(file: usize, symbols: std::ops::Range<usize>) {
    let chunk = |chunk: usize| u32::try_from(chunk).expect("symbol overflow");
    // Only whole chunks, which the range is (`SymbolArena::end_chunk`).
    let first = chunk(symbols.start.div_ceil(LINEAGE_ID_CHUNK));
    let end = chunk(symbols.end >> LINEAGE_ID_SHIFT);
    if first >= end {
        return;
    }
    let mut freed = FREED_LINEAGE.lock().unwrap_or_else(PoisonError::into_inner);
    freed.push(FreedLineage { file, first, end });
    FREED_LINEAGE_COUNT.store(freed.len(), Ordering::Release);
}

impl LineageIds {
    const fn new() -> Self {
        Self {
            chunks: Vec::new(),
            big: FxHashMap::with_hasher(rustc_hash::FxBuildHasher),
            freed: Vec::new(),
            seen: 0,
        }
    }

    /// The id of lineage index `index`, assigned now when it has none.
    // PERF: the hot path of `get_symbol_id` is one more load and test than
    // a dense `Vec`; a new chunk and the frees are out of line.
    #[inline(always)]
    fn id(&mut self, index: usize, give: impl FnOnce() -> u64) -> u64 {
        if let Some(Some(chunk)) = self.chunks.get_mut(index >> LINEAGE_ID_SHIFT) {
            return cell_id(
                &mut chunk[index & (LINEAGE_ID_CHUNK - 1)],
                &mut self.big,
                index,
                give,
            );
        }
        self.id_in_new_chunk(index, give)
    }

    /// The id of lineage index `index` when it has one.
    #[inline(always)]
    fn known(&self, index: usize) -> Option<u64> {
        let Some(Some(chunk)) = self.chunks.get(index >> LINEAGE_ID_SHIFT) else {
            return None;
        };
        known_cell_id(chunk[index & (LINEAGE_ID_CHUNK - 1)], &self.big, index)
    }

    /// `id` when the chunk of `index` has no ids: frees the chunks of the
    /// lineage ranges freed since the last call, then makes the chunk.
    /// Panics when the chunk is freed.
    #[cold]
    #[inline(never)]
    fn id_in_new_chunk(&mut self, index: usize, give: impl FnOnce() -> u64) -> u64 {
        self.free_dead();
        let chunk = index >> LINEAGE_ID_SHIFT;
        if self
            .freed
            .get(chunk / 64)
            .is_some_and(|bits| bits & (1 << (chunk % 64)) != 0)
        {
            freed_lineage_read(chunk);
        }
        if chunk >= self.chunks.len() {
            self.chunks.resize_with(chunk + 1, || None);
        }
        let ids = self.chunks[chunk].insert(Box::new([0; LINEAGE_ID_CHUNK]));
        cell_id(
            &mut ids[index & (LINEAGE_ID_CHUNK - 1)],
            &mut self.big,
            index,
            give,
        )
    }

    /// Frees the chunks of the lineage ranges freed since the last call.
    fn free_dead(&mut self) {
        if self.seen == FREED_LINEAGE_COUNT.load(Ordering::Acquire) {
            return;
        }
        let freed = FREED_LINEAGE.lock().unwrap_or_else(PoisonError::into_inner);
        for range in &freed[self.seen..] {
            for chunk in range.first as usize..range.end as usize {
                if chunk / 64 >= self.freed.len() {
                    self.freed.resize(chunk / 64 + 1, 0);
                }
                self.freed[chunk / 64] |= 1 << (chunk % 64);
                if let Some(ids) = self.chunks.get_mut(chunk) {
                    *ids = None;
                }
            }
        }
        self.seen = freed.len();
    }

    /// Forgets the ids of lineage indexes `range` (`forget_lineage_symbol_ids`).
    fn forget(&mut self, range: std::ops::Range<usize>) {
        let mut index = range.start;
        while index < range.end {
            let chunk = index >> LINEAGE_ID_SHIFT;
            let end = ((chunk + 1) << LINEAGE_ID_SHIFT).min(range.end);
            if let Some(Some(cells)) = self.chunks.get_mut(chunk) {
                let first = chunk << LINEAGE_ID_SHIFT;
                cells[index - first..end - first].fill(0);
            }
            index = end;
        }
        if !self.big.is_empty() {
            self.big.retain(|index, _| !range.contains(index));
        }
    }

    /// True when `free_dead` freed id chunk `chunk`.
    fn is_freed(&self, chunk: usize) -> bool {
        self.freed
            .get(chunk / 64)
            .is_some_and(|bits| bits & (1 << (chunk % 64)) != 0)
    }
}

/// Panics for an id read of freed lineage chunk `chunk`, with the file
/// version whose symbols it held.
#[cold]
#[inline(never)]
fn freed_lineage_read(chunk: usize) -> ! {
    let file = FREED_LINEAGE
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .iter()
        .find(|range| (range.first as usize..range.end as usize).contains(&chunk))
        .map(|range| range.file);
    match file {
        Some(file) => crate::ast::file_version::released(file),
        None => unreachable!("lineage id chunk {chunk} is not freed"),
    }
}

/// The ids of the symbols that checker arenas add on one thread, by arena
/// key. Such a symbol has an index that other checkers and later binds use
/// for other symbols, so its id cannot live in `SYMBOL_IDS`. A checker
/// worker has one arena; a thread that runs checkers of several programs
/// (the language server's) switches between them.
struct OwnSymbolIds {
    /// The arena whose ids are in `ids`, or 0.
    key: u32,
    /// By place among the arena's own symbols (`SymbolIds::own_place`), as
    /// `IdCell`s. The places are dense until a catch-up of the arena passes
    /// its own tail. After that, each odd chunk that the lineage passed is a
    /// gap of 256 places (1 KiB) here.
    ids: Vec<IdCell>,
    /// The ids of the other arenas.
    others: FxHashMap<u32, Vec<IdCell>>,
    /// The ids that have no cell (`BIG_ID`), by arena and place.
    big: FxHashMap<(u32, usize), u64>,
    /// The arenas whose ids stay after the arena drops
    /// (`keep_own_symbol_ids`).
    kept: Vec<u32>,
}

impl OwnSymbolIds {
    const fn new() -> Self {
        Self {
            key: 0,
            ids: Vec::new(),
            others: FxHashMap::with_hasher(rustc_hash::FxBuildHasher),
            big: FxHashMap::with_hasher(rustc_hash::FxBuildHasher),
            kept: Vec::new(),
        }
    }

    /// The id of the own symbol at `place` of arena `key`, assigned now when
    /// it has none.
    #[inline(always)]
    fn id(&mut self, key: u32, place: usize, give: impl FnOnce() -> u64) -> u64 {
        if self.key != key {
            self.switch_to(key);
        }
        match self.ids.get_mut(place) {
            Some(cell) => cell_id(cell, &mut self.big, (key, place), give),
            None => self.id_past_end(key, place, give),
        }
    }

    /// The id of the own symbol at `place` of arena `key` when it has one.
    /// None also when `key` is not the arena in `ids` (`id` switches).
    #[inline(always)]
    fn known(&self, key: u32, place: usize) -> Option<u64> {
        if self.key != key {
            return None;
        }
        known_cell_id(*self.ids.get(place)?, &self.big, (key, place))
    }

    /// `id` for a place past the end of `ids`: grows it to twice its length
    /// or more (zero cells have no id), so a run of new symbols grows it
    /// rarely.
    #[cold]
    #[inline(never)]
    fn id_past_end(&mut self, key: u32, place: usize, give: impl FnOnce() -> u64) -> u64 {
        let len = (place + 1).max(self.ids.len() * 2).max(LINEAGE_ID_CHUNK);
        self.ids.resize(len, 0);
        cell_id(&mut self.ids[place], &mut self.big, (key, place), give)
    }

    /// Makes arena `key` the one in `ids`.
    #[cold]
    #[inline(never)]
    fn switch_to(&mut self, key: u32) {
        let ids = self.others.remove(&key).unwrap_or_default();
        let previous = std::mem::replace(&mut self.ids, ids);
        if self.key != 0 {
            self.others.insert(self.key, previous);
        }
        self.key = key;
    }
}

/// Frees this thread's ids of the own symbols of arena `key`. A checker
/// arena calls it when it drops. The ids of a kept arena stay.
pub(crate) fn forget_own_symbol_ids(key: u32) {
    // The thread may be ending, after its thread-locals.
    let _ = OWN_SYMBOL_IDS.try_with(|own| {
        let mut own = own.borrow_mut();
        if own.kept.contains(&key) {
            return;
        }
        if own.key == key {
            own.key = 0;
            own.ids = Vec::new();
        } else {
            own.others.remove(&key);
        }
        if !own.big.is_empty() {
            own.big.retain(|&(arena, _), _| arena != key);
        }
    });
}

/// Keeps this thread's ids of the own symbols of arena `key` after the
/// arena drops. A shadow of one of those symbols in another arena
/// (`SymbolArena::push_shadow`) keeps its id there, as Go keeps the id on
/// the symbol for as long as any checker holds it.
pub(crate) fn keep_own_symbol_ids(key: u32) {
    OWN_SYMBOL_IDS.with(|own| {
        let mut own = own.borrow_mut();
        if !own.kept.contains(&key) {
            own.kept.push(key);
        }
    });
}

/// The node and symbol ids of one thread (see `id_seed`).
#[derive(Clone, Debug, Default)]
pub struct IdSeed {
    next_node_id: u64,
    next_symbol_id: u64,
    next_symbol_block_id: u64,
    node_ids: PerFileMap<u64>,
    symbol_ids: LineageIds,
}

/// The ids assigned on this thread so far. A checker worker starts from
/// the ids of the loading thread (`install_id_seed`), so a node or symbol
/// that got an id before the checkers started keeps it on every thread.
/// The ids of checker symbols stay: a new thread has none of those checkers.
// PORT: Go shares its atomic counters between checker goroutines, so ids
// that checkers assign race. Here each checker thread counts on its own
// from the same start, which keeps its ids deterministic. The block
// counter comes too: a lineage cell can hold a block id, so a thread that
// counted blocks from 0 again would give that id to a second symbol.
#[must_use]
pub fn id_seed() -> IdSeed {
    IdSeed {
        next_node_id: NEXT_NODE_ID.with(std::cell::Cell::get),
        next_symbol_id: NEXT_SYMBOL_ID.with(std::cell::Cell::get),
        next_symbol_block_id: NEXT_SYMBOL_BLOCK_ID.with(std::cell::Cell::get),
        node_ids: NODE_IDS.with(|ids| {
            let mut ids = ids.borrow_mut();
            // The copy has no ids of dead file versions.
            ids.write();
            ids.clone()
        }),
        symbol_ids: SYMBOL_IDS.with(|ids| {
            let mut ids = ids.borrow_mut();
            // The copy has no ids of freed lineage chunks.
            ids.free_dead();
            ids.clone()
        }),
    }
}

/// The next node and symbol ids of this thread.
#[must_use]
pub fn next_ids() -> (u64, u64) {
    (
        NEXT_NODE_ID.with(std::cell::Cell::get),
        NEXT_SYMBOL_ID.with(std::cell::Cell::get),
    )
}

/// The start of the next block of symbol ids of this thread (Go
/// `nextSymbolBlockId`, `SymbolIdGenerator`).
#[must_use]
pub fn next_symbol_block_id() -> u64 {
    NEXT_SYMBOL_BLOCK_ID.with(std::cell::Cell::get)
}

/// Skips `count` symbol ids on this thread: the next id is `count` higher.
/// A checker worker skips the ids that the other checkers of its pool gave
/// as they were made (`program::new_pool_checker`).
pub fn skip_symbol_ids(count: u64) {
    NEXT_SYMBOL_ID.with(|next| next.set(next.get() + count));
}

/// The number of global ids (Go `GetSymbolId`, not block ids) that this
/// thread gave to symbols that checker arenas made (not binder symbols), in
/// every arena. A checker worker counts the global ids that its
/// `NewChecker` gave to its own symbols with it
/// (`program::new_pool_checker`).
#[must_use]
pub fn own_symbol_id_count() -> u64 {
    OWN_SYMBOL_IDS.with(|own| {
        let own = own.borrow();
        let given =
            |ids: &[IdCell]| ids.iter().filter(|&&id| id != 0 && id < BLOCK_CELL).count() as u64;
        let big = own.big.values().filter(|&&id| id < BLOCK_ID_OFFSET).count() as u64;
        given(&own.ids) + own.others.values().map(|ids| given(ids)).sum::<u64>() + big
    })
}

/// The global ids that a checker worker gave and the blocks that it took,
/// in order, since `record_symbol_ids`. A carry orders them in time by
/// their clock: the ids that the worker gave before them, of both kinds
/// (`IdLog::clock`).
#[derive(Debug, Default)]
struct IdLog {
    globals: Vec<GlobalIdGiven>,
    /// Each block: its start and the block ids that the worker gave before
    /// it. `worker_symbol_ids` fills it from `first_block` and the block
    /// counter: a worker has one link store (its checker's), so it gives
    /// the ids of a block one after the other, and takes the next block
    /// after the last of them; only the worker moves its block counter, so
    /// its blocks follow each other, and block k has `k * BLOCK_ID_SIZE`
    /// block ids before it.
    blocks: Vec<(u64, u64)>,
    /// The block counter of the worker at `record_symbol_ids`.
    first_block: u64,
}

/// A global id (Go `GetSymbolId`) of an `IdLog`.
#[derive(Clone, Copy, Debug)]
struct GlobalIdGiven {
    id: u64,
    /// The block ids that the worker gave before it.
    blocks_before: u64,
    /// The lineage index of a binder symbol; None for a checker symbol.
    /// `worker_symbol_ids` finds it: the worker's global ids come after
    /// the ids of its seed, so a lineage cell with one is that symbol.
    lineage: Option<usize>,
}

impl IdLog {
    /// The global ids given before the block id that came after `blocks`
    /// block ids.
    fn globals_before(&self, blocks: u64) -> u64 {
        self.globals
            .partition_point(|global| global.blocks_before <= blocks) as u64
    }

    /// The index of the logged block of block id `BLOCK_ID_OFFSET + n`.
    fn block_of(&self, n: u64) -> Option<usize> {
        let in_block = |block: usize| {
            self.blocks
                .get(block)
                .is_some_and(|&(start, _)| (start..start + BLOCK_ID_SIZE).contains(&n))
        };
        let first = self.blocks.first()?.0;
        let block = (n.checked_sub(first)? / BLOCK_ID_SIZE) as usize;
        if in_block(block) {
            return Some(block);
        }
        let block = self
            .blocks
            .partition_point(|&(start, _)| start <= n)
            .checked_sub(1)?;
        in_block(block).then_some(block)
    }

    /// The index of the logged global id `id`.
    fn global_of(&self, id: u64) -> Option<usize> {
        self.globals
            .binary_search_by_key(&id, |global| global.id)
            .ok()
    }

    /// The clock of `id`, which this worker gave: the ids that it gave
    /// before it. None for an id that it did not give.
    fn clock(&self, id: u64) -> Option<u64> {
        if let Some(n) = id.checked_sub(BLOCK_ID_OFFSET) {
            let (start, before) = self.blocks[self.block_of(n)?];
            let blocks = before + (n - start);
            return Some(blocks + self.globals_before(blocks));
        }
        let index = self.global_of(id)?;
        Some(index as u64 + self.globals[index].blocks_before)
    }
}

/// Starts the `IdLog` of this thread: a checker worker of a pool whose ids
/// go back to the loading thread (`program::CheckerPool::carry_symbol_ids`).
/// With `count_block_ids` (a pool of more than one worker), it also counts
/// the block ids that it gives, for the clock (`IdLog::clock`).
// PERF: only a merge of two workers or more reads the clock. With one
// worker the log order is the clock order, so its log keeps 0 for the block
// ids before, and its block ids cost nothing more
// (`SymbolIdGenerator::at_last`).
pub fn record_symbol_ids(count_block_ids: bool) {
    BLOCK_IDS_GIVEN.with(|given| given.set(count_block_ids.then_some(0)));
    let first_block = NEXT_SYMBOL_BLOCK_ID.with(std::cell::Cell::get);
    ID_LOG.with(|log| {
        *log.borrow_mut() = Some(IdLog {
            first_block,
            ..IdLog::default()
        });
    });
}

/// The block ids that this thread gave since `record_symbol_ids`: 0 when it
/// does not count them.
fn block_ids_given() -> u64 {
    BLOCK_IDS_GIVEN.with(std::cell::Cell::get).unwrap_or(0)
}

/// The lineage ids of the loading thread while the workers of a pool read
/// them (`carry_symbol_ids`).
#[derive(Clone)]
pub struct SymbolIdBase(std::sync::Arc<LineageIds>);

/// The symbol ids that one checker worker gave (`worker_symbol_ids`): the
/// id chunks of its lineage ids that the loading thread has none for, and
/// its log.
pub struct WorkerSymbolIds {
    /// By chunk, in chunk order. A cell that the loading thread has an id
    /// for is 0.
    chunks: Vec<(usize, Box<[IdCell; LINEAGE_ID_CHUNK]>)>,
    /// The ids of the `BIG_ID` cells of `chunks`, by lineage index.
    big: FxHashMap<usize, u64>,
    log: IdLog,
    /// A worker that kept no log (`record_symbol_ids`): its next symbol id
    /// and block. It is the only worker of its pool, and the loading thread
    /// gave no id since its seed (`program::IdCarry::only_from`), so its
    /// ids keep their numbers and its counters become the loading
    /// thread's, as in the copy of the carry before (R228).
    unlogged: Option<(u64, u64)>,
}

impl WorkerSymbolIds {
    /// The id in cell `cell` (not 0) of lineage index `index`.
    fn id(&self, index: usize, cell: IdCell) -> u64 {
        match cell {
            BIG_ID => self.big[&index],
            cell => u64::from(cell & !BLOCK_CELL) + (u64::from(cell >> 31) * BLOCK_ID_OFFSET),
        }
    }

    /// The cell of lineage index `index` in `chunks`: 0 when it has none.
    fn cell(&self, index: usize) -> IdCell {
        let chunk = index >> LINEAGE_ID_SHIFT;
        self.chunks
            .binary_search_by_key(&chunk, |&(chunk, _)| chunk)
            .map_or(0, |at| self.chunks[at].1[index & (LINEAGE_ID_CHUNK - 1)])
    }
}

/// On a checker worker: the lineage ids of this thread that `base` has
/// none for, and the log (`record_symbol_ids`), which ends here.
#[must_use]
pub fn worker_symbol_ids(base: &SymbolIdBase) -> WorkerSymbolIds {
    let base = &*base.0;
    let counted = BLOCK_IDS_GIVEN.with(|given| given.replace(None));
    let next_block = NEXT_SYMBOL_BLOCK_ID.with(std::cell::Cell::get);
    let log = ID_LOG.with(|log| log.borrow_mut().take());
    let unlogged = log
        .is_none()
        .then(|| (NEXT_SYMBOL_ID.with(std::cell::Cell::get), next_block));
    let mut log = log.unwrap_or_default();
    if unlogged.is_none() {
        log.blocks = (log.first_block..next_block)
            .step_by(BLOCK_ID_SIZE as usize)
            .zip((0..).map(|block: u64| counted.map_or(0, |_| block * BLOCK_ID_SIZE)))
            .collect();
        // Each block but the last is used up.
        debug_assert!(counted.is_none_or(|count| {
            let blocks = log.blocks.len() as u64;
            count <= blocks * BLOCK_ID_SIZE && count + BLOCK_ID_SIZE > blocks * BLOCK_ID_SIZE
        }));
    }
    // The worker's global ids come after the ids of its seed.
    let first_global = log.globals.first().map_or(u64::MAX, |global| global.id);
    let (chunks, big) = SYMBOL_IDS.with(|ids| {
        let mut ids = ids.borrow_mut();
        ids.free_dead();
        let mut chunks = Vec::new();
        let mut big = FxHashMap::default();
        for (chunk, cells) in ids.chunks.iter().enumerate() {
            let Some(cells) = cells else {
                continue;
            };
            if base.is_freed(chunk) {
                continue;
            }
            let base_cells = base.chunks.get(chunk).and_then(Option::as_deref);
            if base_cells == Some(&**cells) {
                continue;
            }
            let start = chunk << LINEAGE_ID_SHIFT;
            for (place, &cell) in cells.iter().enumerate() {
                let id = match cell {
                    0 => continue,
                    BIG_ID => {
                        let id = ids.big[&(start | place)];
                        if base_cells.is_none_or(|base_cells| base_cells[place] == 0) {
                            big.insert(start | place, id);
                        }
                        id
                    }
                    cell if cell < BLOCK_CELL => u64::from(cell),
                    _ => continue,
                };
                if id >= first_global
                    && id < BLOCK_ID_OFFSET
                    && let Some(global) = log.global_of(id)
                {
                    log.globals[global].lineage = Some(start | place);
                }
            }
            let mut new = cells.clone();
            if let Some(base_cells) = base_cells {
                for (cell, &base_cell) in new.iter_mut().zip(base_cells) {
                    if base_cell != 0 {
                        *cell = 0;
                    }
                }
            }
            chunks.push((chunk, new));
        }
        (chunks, big)
    });
    WorkerSymbolIds {
        chunks,
        big,
        log,
        unlogged,
    }
}

/// Merges the symbol ids of the workers of a pool into this thread (the
/// loading thread). `collect` gets this thread's lineage ids and returns
/// `worker_symbol_ids` of each worker, in pool order.
// PORT: Go's two symbol id counters (global and block) are per process, and
// a symbol keeps its id in every checker and later program. Here each
// checker worker counts from the ids of the loading thread (`id_seed`), so
// two workers give the same numbers to different symbols. The merge gives
// Go's counts and keeps each id unique:
// - A lineage symbol that this thread has an id for keeps it. Else the
//   worker that gave it an id first wins, by clock (`IdLog::clock`), then
//   by pool order. The clock models Go's time: the first id wins there.
// - The global counter counts on by the winning global ids and every
//   global id of the workers' own symbols (which die with their checkers),
//   in clock order. The block counter counts on by every block of every
//   worker, by whole block in clock order of the takes; a block id keeps
//   its place in its block.
// With one worker and no id on this thread since the pool was made, the
// merge keeps every id: it makes the worker's ids this thread's. A worker
// with no log (`WorkerSymbolIds::unlogged`) is only merged so.
// PERF: by id chunk. A chunk that one worker has keeps its cells when the
// worker's ids keep their numbers (a one-checker pool), as in the copy of
// the carry before (R228).
// watchreset1r: `forgets_seen` is `forgotten_count` when the pool was made.
// A lineage range that this thread forgot after that
// (`forget_lineage_symbol_ids`) keeps none of the workers' ids: they are
// ids of the old parse, which Go's new symbols do not have. Its global ids
// and blocks still count, as those of a freed symbol do.
pub fn carry_symbol_ids(
    forgets_seen: u64,
    collect: impl FnOnce(&SymbolIdBase) -> Vec<WorkerSymbolIds>,
) {
    let base = SYMBOL_IDS.with(|ids| {
        let mut ids = ids.borrow_mut();
        ids.free_dead();
        SymbolIdBase(std::sync::Arc::new(std::mem::take(&mut *ids)))
    });
    let mut workers = collect(&base);
    // Each worker dropped its copy of `base` before it sent its ids.
    let mut ids = std::sync::Arc::try_unwrap(base.0).unwrap_or_else(|ids| (*ids).clone());
    merge_symbol_ids(&mut ids, &mut workers, &forgotten_after(forgets_seen));
    SYMBOL_IDS.with(|cell| *cell.borrow_mut() = ids);
}

/// `carry_symbol_ids` after the collect: merges `workers` into `ids` and
/// the counters of this thread, with no id in the lineage ranges
/// `forgotten` (sorted and apart).
fn merge_symbol_ids(
    ids: &mut LineageIds,
    workers: &mut [WorkerSymbolIds],
    forgotten: &[std::ops::Range<usize>],
) {
    // A cell that more workers have keeps the id with the lowest clock, then
    // of the first worker; the others drop it.
    let mut shared: Vec<(usize, usize, usize)> = Vec::new();
    for (worker, given) in workers.iter().enumerate() {
        shared.extend(
            given
                .chunks
                .iter()
                .enumerate()
                .map(|(at, &(chunk, _))| (chunk, worker, at)),
        );
    }
    shared.sort_unstable();
    for group in shared
        .chunk_by(|a, b| a.0 == b.0)
        .filter(|group| group.len() > 1)
    {
        // PERF: the workers that have an id in each cell, counted chunk by
        // chunk; only a cell that two have needs clocks.
        let mut given = [0u16; LINEAGE_ID_CHUNK];
        for &(_, worker, at) in group {
            for (given, &cell) in given.iter_mut().zip(workers[worker].chunks[at].1.iter()) {
                *given += u16::from(cell != 0);
            }
        }
        for place in (0..LINEAGE_ID_CHUNK).filter(|&place| given[place] > 1) {
            let index = (group[0].0 << LINEAGE_ID_SHIFT) | place;
            let mut first: Option<(u64, usize, usize)> = None;
            for &(_, worker, at) in group {
                let cell = workers[worker].chunks[at].1[place];
                if cell == 0 {
                    continue;
                }
                let given = &workers[worker];
                let clock = given.log.clock(given.id(index, cell)).unwrap_or(u64::MAX);
                let lost = match first {
                    Some((first_clock, ..)) if first_clock <= clock => (worker, at),
                    _ => {
                        let lost = first.map(|(_, worker, at)| (worker, at));
                        first = Some((clock, worker, at));
                        match lost {
                            Some(lost) => lost,
                            None => continue,
                        }
                    }
                };
                workers[lost.0].chunks[lost.1].1[place] = 0;
            }
        }
    }
    // Global ids in clock order: each winning cell, and every id of a
    // checker symbol. A freed symbol still counts.
    let mut globals: Vec<(u64, usize, usize)> = Vec::new();
    for (worker, given) in workers.iter().enumerate() {
        for (place, global) in given.log.globals.iter().enumerate() {
            let counts = global.lineage.is_none_or(|index| {
                let cell = given.cell(index);
                (cell != 0 && given.id(index, cell) == global.id)
                    || ids.is_freed(index >> LINEAGE_ID_SHIFT)
            });
            if counts {
                globals.push((place as u64 + global.blocks_before, worker, place));
            }
        }
    }
    globals.sort_unstable();
    let mut new_globals: Vec<Vec<u64>> = workers
        .iter()
        .map(|given| vec![0; given.log.globals.len()])
        .collect();
    for &(_, worker, place) in &globals {
        new_globals[worker][place] = next_global_id();
    }
    // Blocks in clock order of their takes.
    let mut blocks: Vec<(u64, usize, usize)> = Vec::new();
    for (worker, given) in workers.iter().enumerate() {
        for (block, &(_, before)) in given.log.blocks.iter().enumerate() {
            blocks.push((before + given.log.globals_before(before), worker, block));
        }
    }
    blocks.sort_unstable();
    let mut new_starts: Vec<Vec<u64>> = workers
        .iter()
        .map(|given| vec![0; given.log.blocks.len()])
        .collect();
    NEXT_SYMBOL_BLOCK_ID.with(|next| {
        for &(_, worker, block) in &blocks {
            new_starts[worker][block] = next.replace(next.get() + BLOCK_ID_SIZE);
        }
    });
    for (worker, given) in workers.iter_mut().enumerate() {
        if let Some((next_symbol_id, next_symbol_block_id)) = given.unlogged {
            debug_assert!(given.log.globals.is_empty() && given.log.blocks.is_empty());
            NEXT_SYMBOL_ID.with(|next| next.set(next.get().max(next_symbol_id)));
            NEXT_SYMBOL_BLOCK_ID.with(|next| next.set(next.get().max(next_symbol_block_id)));
        }
        let log = &given.log;
        let keeps = given.unlogged.is_some()
            || log
                .globals
                .iter()
                .zip(&new_globals[worker])
                .all(|(global, &new)| new == 0 || new == global.id)
                && log
                    .blocks
                    .iter()
                    .zip(&new_starts[worker])
                    .all(|(&(start, _), &new)| new == start);
        // PERF: when the worker's blocks follow each other (they do) and
        // every new block id has a cell, a block cell moves by its block's
        // move, in cell form.
        let moves_from = log.blocks.first().map_or(0, |&(start, _)| start);
        let block_moves: Option<Vec<IdCell>> = log
            .blocks
            .iter()
            .zip(&new_starts[worker])
            .enumerate()
            .all(|(block, (&(start, _), &new))| {
                start == moves_from + block as u64 * BLOCK_ID_SIZE
                    && new + BLOCK_ID_SIZE <= u64::from(BIG_ID - BLOCK_CELL)
            })
            .then(|| {
                log.blocks
                    .iter()
                    .zip(&new_starts[worker])
                    .map(|(&(start, _), &new)| (new as IdCell).wrapping_sub(start as IdCell))
                    .collect()
            });
        let moves_from = moves_from as IdCell;
        for (chunk, mut cells) in std::mem::take(&mut given.chunks) {
            if ids.is_freed(chunk) {
                continue;
            }
            if !forgotten.is_empty() {
                drop_forgotten_cells(&mut cells, chunk, forgotten);
            }
            let first = chunk << LINEAGE_ID_SHIFT;
            if !keeps || !given.big.is_empty() {
                for (place, cell) in cells.iter_mut().enumerate() {
                    if *cell == 0 {
                        continue;
                    }
                    if !keeps
                        && let Some(moves) = &block_moves
                        && *cell != BIG_ID
                        && *cell & BLOCK_CELL != 0
                        && let Some(&by) = moves.get(
                            ((*cell & !BLOCK_CELL).wrapping_sub(moves_from)
                                / BLOCK_ID_SIZE as IdCell) as usize,
                        )
                    {
                        *cell = cell.wrapping_add(by);
                        continue;
                    }
                    let index = first | place;
                    let id = given.id(index, *cell);
                    let new = if keeps {
                        Some(id)
                    } else {
                        match id.checked_sub(BLOCK_ID_OFFSET) {
                            Some(n) => log.block_of(n).map(|block| {
                                BLOCK_ID_OFFSET
                                    + new_starts[worker][block]
                                    + (n - log.blocks[block].0)
                            }),
                            None => log
                                .global_of(id)
                                .map(|global| new_globals[worker][global])
                                .filter(|&new| new != 0),
                        }
                    };
                    let Some(new) = new else {
                        debug_assert!(false, "worker {worker} did not log id {id}");
                        *cell = 0;
                        continue;
                    };
                    *cell = id_cell(new);
                    if *cell == BIG_ID {
                        ids.big.insert(index, new);
                    }
                }
            }
            if chunk >= ids.chunks.len() {
                ids.chunks.resize_with(chunk + 1, || None);
            }
            match &mut ids.chunks[chunk] {
                // A cell of `cells` is 0 where `old` has an id: the worker
                // dropped this thread's ids, and no two workers keep a cell.
                Some(old) => {
                    for (old, cell) in old.iter_mut().zip(cells.iter()) {
                        *old |= cell;
                    }
                }
                slot => *slot = Some(cells),
            }
        }
    }
}

/// Makes `seed` the id state of this thread.
pub fn install_id_seed(seed: IdSeed) {
    NEXT_NODE_ID.with(|next| next.set(seed.next_node_id));
    NEXT_SYMBOL_ID.with(|next| next.set(seed.next_symbol_id));
    NEXT_SYMBOL_BLOCK_ID.with(|next| next.set(seed.next_symbol_block_id));
    NODE_IDS.with(|ids| *ids.borrow_mut() = seed.node_ids);
    SYMBOL_IDS.with(|ids| *ids.borrow_mut() = seed.symbol_ids);
    OWN_SYMBOL_IDS.with(|own| *own.borrow_mut() = OwnSymbolIds::new());
    BLOCK_IDS_GIVEN.with(|given| given.set(None));
    ID_LOG.with(|log| *log.borrow_mut() = None);
}

/// wasm: the ids of one checker of the inline checker pool
/// (`program::WorkerState`), which a native checker worker keeps in its
/// own thread. A job swaps them in (`swap_id_state`).
// PORT: not in Go.
#[cfg(target_family = "wasm")]
pub struct IdState {
    next_node_id: u64,
    next_symbol_id: u64,
    next_symbol_block_id: u64,
    node_ids: PerFileMap<u64>,
    symbol_ids: LineageIds,
    own_symbol_ids: OwnSymbolIds,
    block_ids_given: Option<u64>,
    id_log: Option<IdLog>,
}

#[cfg(target_family = "wasm")]
impl From<IdSeed> for IdState {
    /// The ids of a new worker, as `install_id_seed` sets them.
    fn from(seed: IdSeed) -> Self {
        IdState {
            next_node_id: seed.next_node_id,
            next_symbol_id: seed.next_symbol_id,
            next_symbol_block_id: seed.next_symbol_block_id,
            node_ids: seed.node_ids,
            symbol_ids: seed.symbol_ids,
            own_symbol_ids: OwnSymbolIds::new(),
            block_ids_given: None,
            id_log: None,
        }
    }
}

/// wasm: swaps the ids of this thread with `state`.
// PORT: not in Go.
#[cfg(target_family = "wasm")]
pub fn swap_id_state(state: &mut IdState) {
    NEXT_NODE_ID.with(|next| state.next_node_id = next.replace(state.next_node_id));
    NEXT_SYMBOL_ID.with(|next| state.next_symbol_id = next.replace(state.next_symbol_id));
    NEXT_SYMBOL_BLOCK_ID.with(|next| {
        state.next_symbol_block_id = next.replace(state.next_symbol_block_id);
    });
    NODE_IDS.with(|ids| std::mem::swap(&mut *ids.borrow_mut(), &mut state.node_ids));
    SYMBOL_IDS.with(|ids| std::mem::swap(&mut *ids.borrow_mut(), &mut state.symbol_ids));
    OWN_SYMBOL_IDS.with(|own| std::mem::swap(&mut *own.borrow_mut(), &mut state.own_symbol_ids));
    BLOCK_IDS_GIVEN.with(|given| state.block_ids_given = given.replace(state.block_ids_given));
    ID_LOG.with(|log| std::mem::swap(&mut *log.borrow_mut(), &mut state.id_log));
}

/// The next symbol id of this thread (Go `nextSymbolId.Add(1)`).
fn next_global_id() -> u64 {
    NEXT_SYMBOL_ID.with(|next| {
        let id = next.get() + 1;
        next.set(id);
        id
    })
}

/// Logs global id `id`, which this thread just gave, when it has a log
/// (`record_symbol_ids`).
fn log_global_id(id: u64) {
    ID_LOG.with(|log| {
        if let Some(log) = log.borrow_mut().as_mut() {
            log.globals.push(GlobalIdGiven {
                id,
                blocks_before: block_ids_given(),
                lineage: None,
            });
        }
    });
}

// Go: ast/utilities.go:24 GetNodeId
// PORT: Go `ast.NodeId` is a `uint64`; returned here as `u64`.
pub fn get_node_id(node: Node) -> u64 {
    NODE_IDS.with(|ids| {
        let mut ids = ids.borrow_mut();
        if let Some(id) = ids.get(&node) {
            return *id;
        }
        ids.write();
        if ids.is_dead(node) {
            crate::ast::file_version::released(node.file_index());
        }
        let id = NEXT_NODE_ID.with(|next| {
            let id = next.get() + 1;
            next.set(id);
            id
        });
        ids.write().insert(node, id);
        id
    })
}

// Go: ast/utilities.go:36 GetSymbolId
// PORT: Go `ast.SymbolId` is a `uint64`; returned here as `u64`. A symbol
// handle is an index into `symbols`, which gives the slot where the id is
// kept (see `SymbolArena::id_slot`).
// PERF: an id that this thread has is read with no call and no test more
// than R229's; a new id is given out of line (`give_symbol_id`), so its
// log call holds no register of this path (a log call between the read and
// the store of the id cell cost hono 0.007% instructions, a flag 0.004%).
pub fn get_symbol_id(symbols: &SymbolArena, symbol: SymbolId) -> u64 {
    match known_symbol_id(symbols, symbol) {
        Some(id) => id,
        None => give_symbol_id(symbols, symbol),
    }
}

/// The id of `symbol` when this thread has one.
// PERF: `borrow_mut`, as the give path: a shared `borrow` takes 3 more
// instructions.
#[inline(always)]
fn known_symbol_id(symbols: &SymbolArena, symbol: SymbolId) -> Option<u64> {
    let slot = symbols.id_slot(symbol);
    let place = slot.place as usize;
    if slot.key == 0 {
        SYMBOL_IDS.with(|ids| ids.borrow_mut().known(place))
    } else {
        OWN_SYMBOL_IDS.with(|own| own.borrow_mut().known(slot.key, place))
    }
}

/// `get_symbol_id` of a symbol that `known_symbol_id` has no id for: gives
/// it one (Go `nextSymbolId.Add(1)`) unless it has one after all, and logs
/// a new one (`log_global_id`).
#[cold]
#[inline(never)]
fn give_symbol_id(symbols: &SymbolArena, symbol: SymbolId) -> u64 {
    let mut given = false;
    let id = symbol_id_or(symbols, symbol, || {
        given = true;
        next_global_id()
    });
    if given {
        log_global_id(id);
    }
    id
}

/// The id of `symbol`, assigned now by `give` when it has none.
#[inline(always)]
fn symbol_id_or(symbols: &SymbolArena, symbol: SymbolId, give: impl FnOnce() -> u64) -> u64 {
    let slot = symbols.id_slot(symbol);
    let place = slot.place as usize;
    if slot.key == 0 {
        SYMBOL_IDS.with(|ids| ids.borrow_mut().id(place, give))
    } else {
        OWN_SYMBOL_IDS.with(|own| own.borrow_mut().id(slot.key, place, give))
    }
}

// Go: ast/utilities.go:74 SymbolIdGenerator
/// The block ids of one link store (`checker::ValueSymbolLinkStore`). It
/// takes a block of `BLOCK_ID_SIZE` ids from this thread's block counter
/// when it has given the last one, and gives them in order. The rest of a
/// block is lost with the store, as in Go.
// PORT: `Cell`s, as the store's `TryGet` reads take `&self`.
#[derive(Debug, Default)]
pub struct SymbolIdGenerator {
    next: std::cell::Cell<u64>,
    /// `next_id` takes its other path at this id (`at_last`): the end of
    /// the block, or on a thread that counts its block ids
    /// (`record_symbol_ids`) the next id, so each one is counted.
    last: std::cell::Cell<u64>,
    /// The end of the block of `next`.
    end: std::cell::Cell<u64>,
}

impl SymbolIdGenerator {
    // Go: ast/utilities.go:79 (*SymbolIdGenerator).GetSymbolId
    /// The id of `symbol`: a block id of this generator when it has no id.
    /// A symbol with an id (global or block) keeps it, and takes nothing.
    // PERF: one call with the id path inlined, as `get_symbol_id`. Inlined
    // into `ValueSymbolLinkStore::insert_new_by_id`, the path was split into
    // 3 calls (hono +0.6% instructions).
    #[inline(never)]
    pub fn get_symbol_id(&self, symbols: &SymbolArena, symbol: SymbolId) -> u64 {
        symbol_id_or(symbols, symbol, || self.next_id())
    }

    /// The next block id, from a new block when the last one is used up.
    fn next_id(&self) -> u64 {
        let mut next = self.next.get();
        if next == self.last.get() {
            next = self.at_last(next);
        }
        self.next.set(next + 1);
        BLOCK_ID_OFFSET + next
    }

    /// `next_id` at `last`: takes a new block when `next` is at its end
    /// (Go `nextSymbolBlockId.Add(BlockIdSize) - BlockIdSize`), and counts
    /// the id on a thread that counts its block ids (`BLOCK_IDS_GIVEN`).
    /// Returns the id to give.
    // PERF: no call, so no value of `get_symbol_id` lives across one (a call
    // here cost hono 0.018% instructions). A counting thread comes here for
    // each id.
    #[inline(always)]
    fn at_last(&self, next: u64) -> u64 {
        let next = if next == self.end.get() {
            let start =
                NEXT_SYMBOL_BLOCK_ID.with(|block| block.replace(block.get() + BLOCK_ID_SIZE));
            self.end.set(start + BLOCK_ID_SIZE);
            start
        } else {
            next
        };
        let last = BLOCK_IDS_GIVEN.with(|given| match given.get() {
            Some(count) => {
                given.set(Some(count + 1));
                next + 1
            }
            None => self.end.get(),
        });
        self.last.set(last);
        next
    }
}

// Go: ast/utilities.go:95 GetSymbolTable
// PORT: Go takes `*SymbolTable` and allocates the map in place. The arena
// must be mutable to allocate a table.
pub fn get_symbol_table(symbols: &mut SymbolArena, data: &mut SymbolTable) -> SymbolTable {
    if data.is_nil() {
        *data = symbols.new_table();
    }
    *data
}

// Go: ast/utilities.go:102 GetMembers
pub fn get_members(symbols: &mut SymbolArena, symbol: SymbolId) -> SymbolTable {
    let mut members = symbols.sym(symbol).members;
    let table = get_symbol_table(symbols, &mut members);
    symbols.sym_mut(symbol).members = members;
    table
}

// Go: ast/utilities.go:106 GetExports
pub fn get_exports(symbols: &mut SymbolArena, symbol: SymbolId) -> SymbolTable {
    let mut exports = symbols.sym(symbol).exports;
    let table = get_symbol_table(symbols, &mut exports);
    symbols.sym_mut(symbol).exports = exports;
    table
}

// Go: ast/utilities.go:110 GetLocals
// PORT: Go writes `container.LocalsContainerData().Locals`. Installed binder
// data (`Node::locals`) is not written again, so the binder passes the
// mutable per-node bind data of the file it is binding, indexed by
// `NodeId::index()`.
pub fn get_locals(
    symbols: &mut SymbolArena,
    node_bind: &mut [NodeBindData],
    container: Node,
) -> SymbolTable {
    let index = container.node_id().index();
    let mut locals = node_bind[index].locals;
    let table = get_symbol_table(symbols, &mut locals);
    node_bind[index].locals = locals;
    table
}

// Determines if a node is missing (either `nil` or empty)
// Go: ast/utilities.go:115 NodeIsMissing
// PERF: noderef1 G1. The loc and the kind are read through one view
// (`NodeRef`).
pub fn node_is_missing(node: Node) -> bool {
    if node.is_nil() {
        return true;
    }
    let node = NodeRef::new(node);
    let loc = node.loc();
    loc.pos() == loc.end() && loc.pos() >= 0 && node.kind() != SyntaxKind::EndOfFile
}

// Determines if a node is present
// Go: ast/utilities.go:120 NodeIsPresent
pub fn node_is_present(node: Node) -> bool {
    !node_is_missing(node)
}

// Determines if a node contains synthetic positions
// Go: ast/utilities.go:125 NodeIsSynthesized
pub fn node_is_synthesized(node: Node) -> bool {
    position_is_synthesized(node.loc().pos()) || position_is_synthesized(node.loc().end())
}

// Go: ast/utilities.go:129 RangeIsSynthesized
pub fn range_is_synthesized(loc: TextRange) -> bool {
    position_is_synthesized(loc.pos()) || position_is_synthesized(loc.end())
}

// Determines whether a position is synthetic
// Go: ast/utilities.go:134 PositionIsSynthesized
pub fn position_is_synthesized(pos: i32) -> bool {
    pos < 0
}

// Go: ast/utilities.go:138 FindLastVisibleNode
pub fn find_last_visible_node(nodes: &[Node]) -> Node {
    let mut from_end = 1usize;
    while from_end <= nodes.len()
        && nodes[nodes.len() - from_end]
            .flags()
            .intersects(NodeFlags::REPARSED)
    {
        from_end += 1;
    }
    if from_end <= nodes.len() {
        return nodes[nodes.len() - from_end];
    }
    Node::NIL
}

// Go: ast/utilities.go:149 NodeKindIs
pub fn node_kind_is(node: Node, kinds: &[SyntaxKind]) -> bool {
    kinds.contains(&node.kind())
}

// Go: ast/utilities.go:153 IsModifier
pub fn is_modifier(node: Node) -> bool {
    is_modifier_kind(node.kind())
}

// Go: ast/utilities.go:157 IsModifierLike
pub fn is_modifier_like(node: Node) -> bool {
    is_modifier(node) || is_decorator(node)
}

// Go: ast/utilities.go:161 IsCompoundAssignment
pub fn is_compound_assignment(token: SyntaxKind) -> bool {
    (token as u16) >= (SyntaxKind::FIRST_COMPOUND_ASSIGNMENT as u16)
        && (token as u16) <= (SyntaxKind::LAST_COMPOUND_ASSIGNMENT as u16)
}

/// The fields of a BinaryExpression, read once: Go `node.AsBinaryExpression()`
/// and its fields, plus `OperatorToken.Kind`. The binder reads them through
/// this view (`Binder::binary_view`) in `bind` and passes it on.
// PERF: binderview1 (cliperf1 rank 4a). Each accessor read (`left`,
// `operator_token`, `right`, `type_`) looks the node up again: about 45
// instructions and 2 file block lookups, and the binder made about 7
// `operator_token` reads per BinaryExpression. A view loads the node data
// once and resolves the child ids with no lookup (`FrozenIds`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BinaryView {
    pub left: Node,
    pub operator_token: Node,
    /// Go `OperatorToken.Kind`.
    pub operator: SyntaxKind,
    pub right: Node,
    pub type_: Node,
}

impl BinaryView {
    /// The view of BinaryExpression `node`. `ids` is `frozen_store_ids` of
    /// the file of `node` (a caller that reads many nodes of one file reads
    /// it once). A node with no published data (synthetic, unpublished or
    /// of a freeable file version), or no `ids`, reads each field with its
    /// accessor. Panics as the accessors do when `node` is no
    /// BinaryExpression.
    #[inline]
    #[must_use]
    pub fn with_ids(node: Node, ids: Option<FrozenIds>) -> Self {
        let view = match (frozen_store_ast_node(node), ids) {
            (Some(crate::astdata::NodeData::BinaryExpression(d)), Some(ids)) => {
                let operator_token = ids.node(d.operator_token);
                BinaryView {
                    left: ids.node(d.left),
                    operator_token,
                    operator: operator_token.kind(),
                    right: ids.node(d.right),
                    type_: d.type_.map_or(Node::NIL, |id| ids.node(id)),
                }
            }
            _ => Self::read(node),
        };
        debug_assert_eq!(view, Self::read(node), "binary view of {node:?}");
        view
    }

    /// `with_ids` with the ids of the file of `node`.
    #[inline]
    #[must_use]
    pub fn new(node: Node) -> Self {
        Self::with_ids(node, frozen_store_ids(node.file_index()))
    }

    /// The view from the field accessors.
    #[cold]
    #[inline(never)]
    fn read(node: Node) -> Self {
        let operator_token = node.operator_token();
        BinaryView {
            left: node.left(),
            operator_token,
            operator: operator_token.kind(),
            right: node.right(),
            type_: node.type_(),
        }
    }

    /// `is_destructuring_assignment` of the node of this view. An object or
    /// array literal is a left-hand side expression kind, so the kind test
    /// of `left` also gives Go `IsLeftHandSideExpression(Left)`.
    #[must_use]
    pub fn is_destructuring_assignment(&self) -> bool {
        self.operator == SyntaxKind::EqualsToken
            && matches!(
                self.left.kind(),
                SyntaxKind::ObjectLiteralExpression | SyntaxKind::ArrayLiteralExpression
            )
    }
}

// Go: ast/utilities.go:165 IsAssignmentExpression
pub fn is_assignment_expression(node: Node, exclude_compound_assignment: bool) -> bool {
    if node.kind() == SyntaxKind::BinaryExpression {
        let operator = node.operator_token().kind();
        return (operator == SyntaxKind::EqualsToken
            || !exclude_compound_assignment && is_assignment_operator(operator))
            && is_left_hand_side_expression(node.left());
    }
    false
}

// Go: ast/utilities.go:174 GetRightMostAssignedExpression
pub fn get_right_most_assigned_expression(node: Node) -> Node {
    let mut node = node;
    while is_assignment_expression(node, false /*excludeCompoundAssignment*/) {
        node = node.right();
    }
    node
}

// Go: ast/utilities.go:181 IsDestructuringAssignment
pub fn is_destructuring_assignment(node: Node) -> bool {
    if is_assignment_expression(node, true /*excludeCompoundAssignment*/) {
        let kind = node.left().kind();
        return kind == SyntaxKind::ObjectLiteralExpression
            || kind == SyntaxKind::ArrayLiteralExpression;
    }
    false
}

// Go: ast/utilities.go:189 IsObjectBindingOrAssignmentElement
pub fn is_object_binding_or_assignment_element(node: Node) -> bool {
    matches!(
        node.kind(),
        SyntaxKind::BindingElement
            | SyntaxKind::PropertyAssignment
            | SyntaxKind::ShorthandPropertyAssignment
            | SyntaxKind::SpreadAssignment
    )
}

// Go: ast/utilities.go:200 IsArrayBindingOrAssignmentElement
pub fn is_array_binding_or_assignment_element(node: Node) -> bool {
    match node.kind() {
        SyntaxKind::BindingElement
        | SyntaxKind::OmittedExpression
        | SyntaxKind::SpreadElement
        | SyntaxKind::ArrayLiteralExpression
        | SyntaxKind::ObjectLiteralExpression
        | SyntaxKind::Identifier
        | SyntaxKind::PropertyAccessExpression
        | SyntaxKind::ElementAccessExpression => return true,
        _ => {}
    }
    is_assignment_expression(node, true /*excludeCompoundAssignment*/)
}

// Go: ast/utilities.go:215 IsBindingPattern
// PERF: noderef1 G3. One kind read, inline.
#[inline]
pub fn is_binding_pattern(node: Node) -> bool {
    matches!(
        node.kind(),
        SyntaxKind::ObjectBindingPattern | SyntaxKind::ArrayBindingPattern
    )
}

// Go: ast/utilities.go:219 IsForInOrOfStatement
pub fn is_for_in_or_of_statement(node: Node) -> bool {
    node.is_some()
        && (node.kind() == SyntaxKind::ForInStatement || node.kind() == SyntaxKind::ForOfStatement)
}

// A node is an assignment target if it is on the left hand side of an '=' token, if it is parented by a property
// assignment in an object literal that is an assignment target, or if it is parented by an array literal that is
// an assignment target. Examples include 'a = xxx', '{ p: a } = xxx', '[{ a }] = xxx'.
// (Note that `p` is not a target in the above examples, only `a`.)
// Go: ast/utilities.go:227 IsAssignmentTarget
pub fn is_assignment_target(node: Node) -> bool {
    get_assignment_target(node).is_some()
}

// Returns the BinaryExpression, PrefixUnaryExpression, PostfixUnaryExpression, or ForInOrOfStatement that references
// the given node as an assignment target
// Go: ast/utilities.go:233 GetAssignmentTarget
// PERF: U4 (CH7). A step inside a published store reads the parent and its
// kind with one store lookup (`frozen_store_parent_kind`).
// PERF: deferbody1. Most calls end at the first step, at a parent kind that
// the walk does not handle (a call, a property access, a statement). That
// step is inline at the caller; the walk is out of line.
#[inline(always)]
pub fn get_assignment_target(node: Node) -> Node {
    let (parent, parent_kind) = assignment_target_parent(node);
    match parent_kind {
        SyntaxKind::BinaryExpression
        | SyntaxKind::PrefixUnaryExpression
        | SyntaxKind::PostfixUnaryExpression
        | SyntaxKind::ForInStatement
        | SyntaxKind::ForOfStatement
        | SyntaxKind::ParenthesizedExpression
        | SyntaxKind::ArrayLiteralExpression
        | SyntaxKind::SpreadElement
        | SyntaxKind::NonNullExpression
        | SyntaxKind::SpreadAssignment
        | SyntaxKind::ShorthandPropertyAssignment
        | SyntaxKind::PropertyAssignment => get_assignment_target_from(node, parent, parent_kind),
        _ => Node::NIL,
    }
}

/// Go `node.Parent` and `node.Parent.Kind`: one store lookup for a parent in
/// the same published store (`frozen_store_parent_kind`).
#[inline(always)]
fn assignment_target_parent(node: Node) -> (Node, SyntaxKind) {
    match frozen_store_parent_kind(node) {
        Some(parent_and_kind) => parent_and_kind,
        None => node_parent_and_kind_slow(node),
    }
}

/// `node.Parent` and its kind, read one by one: a parent in another store,
/// a nil parent (its kind read panics, as Go does) or a node that is not in
/// a published store.
#[cold]
#[inline(never)]
fn node_parent_and_kind_slow(node: Node) -> (Node, SyntaxKind) {
    let parent = node.parent();
    (parent, parent.kind())
}

/// The walk of `get_assignment_target` from `node`, whose parent `parent`
/// of kind `parent_kind` the caller has read.
#[inline(never)]
fn get_assignment_target_from(node: Node, parent: Node, parent_kind: SyntaxKind) -> Node {
    let mut node = node;
    let (mut parent, mut parent_kind) = (parent, parent_kind);
    loop {
        match parent_kind {
            SyntaxKind::BinaryExpression => {
                if is_assignment_operator(parent.operator_token().kind()) && parent.left() == node {
                    return parent;
                }
                return Node::NIL;
            }
            SyntaxKind::PrefixUnaryExpression => {
                if parent.operator() == SyntaxKind::PlusPlusToken
                    || parent.operator() == SyntaxKind::MinusMinusToken
                {
                    return parent;
                }
                return Node::NIL;
            }
            SyntaxKind::PostfixUnaryExpression => {
                if parent.operator() == SyntaxKind::PlusPlusToken
                    || parent.operator() == SyntaxKind::MinusMinusToken
                {
                    return parent;
                }
                return Node::NIL;
            }
            SyntaxKind::ForInStatement | SyntaxKind::ForOfStatement => {
                if parent.initializer() == node {
                    return parent;
                }
                return Node::NIL;
            }
            SyntaxKind::ParenthesizedExpression
            | SyntaxKind::ArrayLiteralExpression
            | SyntaxKind::SpreadElement
            | SyntaxKind::NonNullExpression => {
                node = parent;
            }
            SyntaxKind::SpreadAssignment => {
                node = parent.parent();
            }
            SyntaxKind::ShorthandPropertyAssignment => {
                if parent.name() != node {
                    return Node::NIL;
                }
                node = parent.parent();
            }
            SyntaxKind::PropertyAssignment => {
                if parent.name() == node {
                    return Node::NIL;
                }
                node = parent.parent();
            }
            _ => return Node::NIL,
        }
        (parent, parent_kind) = assignment_target_parent(node);
    }
}

// Go: ast/utilities.go:277 IsLogicalBinaryOperator
pub fn is_logical_binary_operator(token: SyntaxKind) -> bool {
    token == SyntaxKind::BarBarToken || token == SyntaxKind::AmpersandAmpersandToken
}

// Go: ast/utilities.go:281 IsLogicalOrCoalescingBinaryOperator
pub fn is_logical_or_coalescing_binary_operator(token: SyntaxKind) -> bool {
    is_logical_binary_operator(token) || token == SyntaxKind::QuestionQuestionToken
}

// Go: ast/utilities.go:285 IsLogicalOrCoalescingBinaryExpression
pub fn is_logical_or_coalescing_binary_expression(expr: Node) -> bool {
    is_binary_expression(expr)
        && is_logical_or_coalescing_binary_operator(expr.operator_token().kind())
}

// Go: ast/utilities.go:289 IsLogicalOrCoalescingAssignmentExpression
pub fn is_logical_or_coalescing_assignment_expression(expr: Node) -> bool {
    is_binary_expression(expr)
        && is_logical_or_coalescing_assignment_operator(expr.operator_token().kind())
}

// Go: ast/utilities.go:293 IsLogicalExpression
pub fn is_logical_expression(node: Node) -> bool {
    let mut node = node;
    loop {
        if node.kind() == SyntaxKind::ParenthesizedExpression {
            node = node.expression();
        } else if node.kind() == SyntaxKind::PrefixUnaryExpression
            && node.operator() == SyntaxKind::ExclamationToken
        {
            node = node.operand();
        } else {
            return is_logical_or_coalescing_binary_expression(node);
        }
    }
}

// Go: ast/utilities.go:305 IsAccessor
pub fn is_accessor(node: Node) -> bool {
    node.kind() == SyntaxKind::GetAccessor || node.kind() == SyntaxKind::SetAccessor
}

// Go: ast/utilities.go:309 IsPropertyNameLiteral
pub fn is_property_name_literal(node: Node) -> bool {
    matches!(
        node.kind(),
        SyntaxKind::Identifier
            | SyntaxKind::StringLiteral
            | SyntaxKind::NoSubstitutionTemplateLiteral
            | SyntaxKind::NumericLiteral
    )
}

// Go: ast/utilities.go:320 IsMemberName
pub fn is_member_name(node: Node) -> bool {
    node.kind() == SyntaxKind::Identifier || node.kind() == SyntaxKind::PrivateIdentifier
}

// Go: ast/utilities.go:324 IsEntityName
// PERF: noderef1 G3. One kind read, inline.
#[inline]
pub fn is_entity_name(node: Node) -> bool {
    matches!(
        node.kind(),
        SyntaxKind::Identifier | SyntaxKind::QualifiedName
    )
}

// Go: ast/utilities.go:328 IsPropertyName
pub fn is_property_name(node: Node) -> bool {
    matches!(
        node.kind(),
        SyntaxKind::Identifier
            | SyntaxKind::PrivateIdentifier
            | SyntaxKind::StringLiteral
            | SyntaxKind::NumericLiteral
            | SyntaxKind::ComputedPropertyName
    )
}

// Return true if the given identifier is classified as an IdentifierName by inspecting the parent of the node
// Go: ast/utilities.go:341 IsIdentifierName
pub fn is_identifier_name(node: Node) -> bool {
    let parent = node.parent();
    match parent.kind() {
        SyntaxKind::PropertyDeclaration
        | SyntaxKind::PropertySignature
        | SyntaxKind::MethodDeclaration
        | SyntaxKind::MethodSignature
        | SyntaxKind::GetAccessor
        | SyntaxKind::SetAccessor
        | SyntaxKind::EnumMember
        | SyntaxKind::PropertyAssignment
        | SyntaxKind::PropertyAccessExpression => parent.name() == node,
        SyntaxKind::QualifiedName => parent.right() == node,
        SyntaxKind::BindingElement => parent.property_name() == node,
        SyntaxKind::ImportSpecifier => parent.property_name() == node,
        SyntaxKind::ExportSpecifier
        | SyntaxKind::JsxAttribute
        | SyntaxKind::JsxSelfClosingElement
        | SyntaxKind::JsxOpeningElement
        | SyntaxKind::JsxClosingElement => true,
        _ => false,
    }
}

// Go: ast/utilities.go:359 IsPushOrUnshiftIdentifier
pub fn is_push_or_unshift_identifier(node: Node) -> bool {
    let text = node.text();
    text == "push" || text == "unshift"
}

// Go: ast/utilities.go:364 IsBooleanLiteral
pub fn is_boolean_literal(node: Node) -> bool {
    node.kind() == SyntaxKind::TrueKeyword || node.kind() == SyntaxKind::FalseKeyword
}

// Go: ast/utilities.go:368 IsLiteralExpression
pub fn is_literal_expression(node: Node) -> bool {
    is_literal_kind(node.kind())
}

// Go: ast/utilities.go:372 IsStringLiteralLike
pub fn is_string_literal_like(node: Node) -> bool {
    matches!(
        node.kind(),
        SyntaxKind::StringLiteral | SyntaxKind::NoSubstitutionTemplateLiteral
    )
}

// Go: ast/utilities.go:380 IsStringOrNumericLiteralLike
// PERF: binderview1. The kind is read once (see `is_access_expression`).
pub fn is_string_or_numeric_literal_like(node: Node) -> bool {
    matches!(
        node.kind(),
        SyntaxKind::StringLiteral
            | SyntaxKind::NoSubstitutionTemplateLiteral
            | SyntaxKind::NumericLiteral
    )
}

// Go: ast/utilities.go:384 IsSignedNumericLiteral
pub fn is_signed_numeric_literal(node: Node) -> bool {
    if node.kind() == SyntaxKind::PrefixUnaryExpression {
        return (node.operator() == SyntaxKind::PlusToken
            || node.operator() == SyntaxKind::MinusToken)
            && is_numeric_literal(node.operand());
    }
    false
}

// Determines if a node is part of an OptionalChain
// Go: ast/utilities.go:393 IsOptionalChain
// PERF: U4 (CH7). The kind test comes first (both tests are pure, so the
// result is the same) and `OPTIONAL_CHAIN` is a parser bit
// (`Node::parser_flags`), so most nodes load neither their flags nor their
// binder data.
// PERF: noderef1 G2. The kind and the flags through one view (`NodeRef`).
pub fn is_optional_chain(node: Node) -> bool {
    let node = NodeRef::new(node);
    matches!(
        node.kind(),
        SyntaxKind::PropertyAccessExpression
            | SyntaxKind::ElementAccessExpression
            | SyntaxKind::CallExpression
            | SyntaxKind::NonNullExpression
    ) && !node.parser_flags(NodeFlags::OPTIONAL_CHAIN).is_empty()
}

// Go: ast/utilities.go:406 getQuestionDotToken
fn get_question_dot_token(node: Node) -> Node {
    node.question_dot_token()
}

// Determines if node is the root expression of an OptionalChain
// Go: ast/utilities.go:411 IsOptionalChainRoot
pub fn is_optional_chain_root(node: Node) -> bool {
    is_optional_chain(node)
        && !is_non_null_expression(node)
        && get_question_dot_token(node).is_some()
}

// Determines whether a node is the outermost `OptionalChain` in an ECMAScript `OptionalExpression`:
//
//  1. For `a?.b.c`, the outermost chain is `a?.b.c` (`c` is the end of the chain starting at `a?.`)
//  2. For `a?.b!`, the outermost chain is `a?.b` (`b` is the end of the chain starting at `a?.`)
//  3. For `(a?.b.c).d`, the outermost chain is `a?.b.c` (`c` is the end of the chain starting at `a?.` since parens end the chain)
//  4. For `a?.b.c?.d`, both `a?.b.c` and `a?.b.c?.d` are outermost (`c` is the end of the chain starting at `a?.`, and `d` is
//     the end of the chain starting at `c?.`)
//  5. For `a?.(b?.c).d`, both `b?.c` and `a?.(b?.c)d` are outermost (`c` is the end of the chain starting at `b`, and `d` is
//     the end of the chain starting at `a?.`)
// Go: ast/utilities.go:424 IsOutermostOptionalChain
pub fn is_outermost_optional_chain(node: Node) -> bool {
    let parent = node.parent();
    !is_optional_chain(parent) || // cases 1, 2, and 3
        is_optional_chain_root(parent) || // case 4
        node != parent.expression() // case 5
}

// Determines whether a node is the expression preceding an optional chain (i.e. `a` in `a?.b`).
// Go: ast/utilities.go:432 IsExpressionOfOptionalChainRoot
pub fn is_expression_of_optional_chain_root(node: Node) -> bool {
    is_optional_chain_root(node.parent()) && node.parent().expression() == node
}

// Go: ast/utilities.go:436 IsNullishCoalesce
pub fn is_nullish_coalesce(node: Node) -> bool {
    node.kind() == SyntaxKind::BinaryExpression
        && node.operator_token().kind() == SyntaxKind::QuestionQuestionToken
}

// Go: ast/utilities.go:440 IsAssertionExpression
pub fn is_assertion_expression(node: Node) -> bool {
    let kind = node.kind();
    kind == SyntaxKind::TypeAssertionExpression || kind == SyntaxKind::AsExpression
}

// Go: ast/utilities.go:445 isLeftHandSideExpressionKind
fn is_left_hand_side_expression_kind(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        SyntaxKind::PropertyAccessExpression
            | SyntaxKind::ElementAccessExpression
            | SyntaxKind::NewExpression
            | SyntaxKind::CallExpression
            | SyntaxKind::JsxElement
            | SyntaxKind::JsxSelfClosingElement
            | SyntaxKind::JsxFragment
            | SyntaxKind::TaggedTemplateExpression
            | SyntaxKind::ArrayLiteralExpression
            | SyntaxKind::ParenthesizedExpression
            | SyntaxKind::ObjectLiteralExpression
            | SyntaxKind::ClassExpression
            | SyntaxKind::FunctionExpression
            | SyntaxKind::Identifier
            | SyntaxKind::PrivateIdentifier
            | SyntaxKind::RegularExpressionLiteral
            | SyntaxKind::NumericLiteral
            | SyntaxKind::BigIntLiteral
            | SyntaxKind::StringLiteral
            | SyntaxKind::NoSubstitutionTemplateLiteral
            | SyntaxKind::TemplateExpression
            | SyntaxKind::FalseKeyword
            | SyntaxKind::NullKeyword
            | SyntaxKind::ThisKeyword
            | SyntaxKind::TrueKeyword
            | SyntaxKind::SuperKeyword
            | SyntaxKind::NonNullExpression
            | SyntaxKind::ExpressionWithTypeArguments
            | SyntaxKind::MetaProperty
            | SyntaxKind::ImportKeyword
            | SyntaxKind::MissingDeclaration
    )
}

// Determines whether a node is a LeftHandSideExpression based only on its kind.
// Go: ast/utilities.go:460 IsLeftHandSideExpression
// PERF: binderview1. The kind is read once. Only a PartiallyEmittedExpression
// is an outer expression of `OEK_PARTIALLY_EMITTED_EXPRESSIONS`, so any other
// node is its own `SkipPartiallyEmittedExpressions`.
pub fn is_left_hand_side_expression(node: Node) -> bool {
    let kind = node.kind();
    if kind != SyntaxKind::PartiallyEmittedExpression {
        return is_left_hand_side_expression_kind(kind);
    }
    is_left_hand_side_expression_kind(skip_partially_emitted_expressions(node).kind())
}

// Go: ast/utilities.go:464 isUnaryExpressionKind
fn is_unary_expression_kind(kind: SyntaxKind) -> bool {
    match kind {
        SyntaxKind::PrefixUnaryExpression
        | SyntaxKind::PostfixUnaryExpression
        | SyntaxKind::DeleteExpression
        | SyntaxKind::TypeOfExpression
        | SyntaxKind::VoidExpression
        | SyntaxKind::AwaitExpression
        | SyntaxKind::TypeAssertionExpression => true,
        _ => is_left_hand_side_expression_kind(kind),
    }
}

// Determines whether a node is a UnaryExpression based only on its kind.
// Go: ast/utilities.go:479 IsUnaryExpression
pub fn is_unary_expression(node: Node) -> bool {
    is_unary_expression_kind(skip_partially_emitted_expressions(node).kind())
}

// Go: ast/utilities.go:483 isExpressionKind
fn is_expression_kind(kind: SyntaxKind) -> bool {
    match kind {
        SyntaxKind::ConditionalExpression
        | SyntaxKind::YieldExpression
        | SyntaxKind::ArrowFunction
        | SyntaxKind::BinaryExpression
        | SyntaxKind::SpreadElement
        | SyntaxKind::AsExpression
        | SyntaxKind::OmittedExpression
        | SyntaxKind::PartiallyEmittedExpression
        | SyntaxKind::SatisfiesExpression => true,
        _ => is_unary_expression_kind(kind),
    }
}

// Determines whether a node is an expression based only on its kind.
// Go: ast/utilities.go:500 IsExpression
pub fn is_expression(node: Node) -> bool {
    is_expression_kind(skip_partially_emitted_expressions(node).kind())
}

// Go: ast/utilities.go:504 IsCommaExpression
pub fn is_comma_expression(node: Node) -> bool {
    node.kind() == SyntaxKind::BinaryExpression
        && node.operator_token().kind() == SyntaxKind::CommaToken
}

// Go: ast/utilities.go:508 IsCommaSequence
pub fn is_comma_sequence(node: Node) -> bool {
    is_comma_expression(node)
}

// Go: ast/utilities.go:512 IsIterationStatement
pub fn is_iteration_statement(node: Node, look_in_labeled_statements: bool) -> bool {
    match node.kind() {
        SyntaxKind::ForStatement
        | SyntaxKind::ForInStatement
        | SyntaxKind::ForOfStatement
        | SyntaxKind::DoStatement
        | SyntaxKind::WhileStatement => true,
        SyntaxKind::LabeledStatement => {
            look_in_labeled_statements
                && is_iteration_statement(node.statement(), look_in_labeled_statements)
        }
        _ => false,
    }
}

// Determines if a node is a property or element access expression
// Go: ast/utilities.go:528 IsAccessExpression
// PERF: binderview1. The kind is read once: a kind read is a store lookup
// that the compiler does not merge with a second one.
pub fn is_access_expression(node: Node) -> bool {
    matches!(
        node.kind(),
        SyntaxKind::PropertyAccessExpression | SyntaxKind::ElementAccessExpression
    )
}

/// A fixed set of `SyntaxKind`s, one bit per kind, built at compile time.
// PERF: the hot kind tests below (`is_statement`, `is_function_like`,
// `is_private_identifier_class_element_declaration`) are one table load and
// one bit test, not a chain of compares. The answers are the same as the Go
// `switch` statements, whose kind lists are kept here in Go order.
#[derive(Clone, Copy)]
struct KindSet([u64; 8]);

// `KindSet::contains` masks the word index with `& 7`, so 8 words (512 bits)
// must cover every kind. The mask removes the bounds check.
const _: () = assert!(SyntaxKind::COUNT <= 512);

impl KindSet {
    const fn of(kinds: &[SyntaxKind]) -> Self {
        Self::EMPTY.with(kinds)
    }

    const EMPTY: Self = Self([0; 8]);

    /// This set plus `kinds`.
    const fn with(mut self, kinds: &[SyntaxKind]) -> Self {
        let mut i = 0;
        while i < kinds.len() {
            let k = kinds[i] as usize;
            self.0[k >> 6] |= 1 << (k & 63);
            i += 1;
        }
        self
    }

    #[inline]
    const fn contains(&self, kind: SyntaxKind) -> bool {
        let k = kind as usize;
        self.0[(k >> 6) & 7] & (1 << (k & 63)) != 0
    }
}

// The kinds of Go `isFunctionLikeDeclarationKind` (ast/utilities.go:483).
const FUNCTION_LIKE_DECLARATION_KIND_LIST: &[SyntaxKind] = &[
    SyntaxKind::FunctionDeclaration,
    SyntaxKind::MethodDeclaration,
    SyntaxKind::Constructor,
    SyntaxKind::GetAccessor,
    SyntaxKind::SetAccessor,
    SyntaxKind::FunctionExpression,
    SyntaxKind::ArrowFunction,
];

static FUNCTION_LIKE_DECLARATION_KINDS: KindSet = KindSet::of(FUNCTION_LIKE_DECLARATION_KIND_LIST);

// The kinds of Go `IsFunctionLikeKind` (ast/utilities.go:503): these kinds,
// then the function-like declaration kinds.
static FUNCTION_LIKE_KINDS: KindSet = KindSet::of(&[
    SyntaxKind::MethodSignature,
    SyntaxKind::CallSignature,
    SyntaxKind::JsDocSignature,
    SyntaxKind::ConstructSignature,
    SyntaxKind::IndexSignature,
    SyntaxKind::FunctionType,
    SyntaxKind::ConstructorType,
])
.with(FUNCTION_LIKE_DECLARATION_KIND_LIST);

// Go: ast/utilities.go:532 isFunctionLikeDeclarationKind
#[inline]
pub fn is_function_like_declaration_kind(kind: SyntaxKind) -> bool {
    FUNCTION_LIKE_DECLARATION_KINDS.contains(kind)
}

// Determines if a node is function-like (but is not a signature declaration)
// Go: ast/utilities.go:547 IsFunctionLikeDeclaration
pub fn is_function_like_declaration(node: Node) -> bool {
    // TODO(rbuckton): Move `node != nil` test to call sites
    node.is_some() && is_function_like_declaration_kind(node.kind())
}

// Go: ast/utilities.go:552 IsFunctionLikeKind
#[inline]
pub fn is_function_like_kind(kind: SyntaxKind) -> bool {
    FUNCTION_LIKE_KINDS.contains(kind)
}

// Determines if a node is function- or signature-like.
// Go: ast/utilities.go:567 IsFunctionLike
#[inline]
pub fn is_function_like(node: Node) -> bool {
    // TODO(rbuckton): Move `node != nil` test to call sites
    node.is_some() && is_function_like_kind(node.kind())
}

// Go: ast/utilities.go:572 IsFunctionLikeOrClassStaticBlockDeclaration
pub fn is_function_like_or_class_static_block_declaration(node: Node) -> bool {
    node.is_some() && (is_function_like(node) || is_class_static_block_declaration(node))
}

// Go: ast/utilities.go:576 IsFunctionOrSourceFile
pub fn is_function_or_source_file(node: Node) -> bool {
    is_function_like(node) || is_source_file(node)
}

// Go: ast/utilities.go:580 IsClassLike
pub fn is_class_like(node: Node) -> bool {
    node.kind() == SyntaxKind::ClassDeclaration || node.kind() == SyntaxKind::ClassExpression
}

// Go: ast/utilities.go:584 IsClassOrInterfaceLike
pub fn is_class_or_interface_like(node: Node) -> bool {
    node.kind() == SyntaxKind::ClassDeclaration
        || node.kind() == SyntaxKind::ClassExpression
        || node.kind() == SyntaxKind::InterfaceDeclaration
}

// Go: ast/utilities.go:588 IsClassElement
pub fn is_class_element(node: Node) -> bool {
    matches!(
        node.kind(),
        SyntaxKind::Constructor
            | SyntaxKind::PropertyDeclaration
            | SyntaxKind::MethodDeclaration
            | SyntaxKind::GetAccessor
            | SyntaxKind::SetAccessor
            | SyntaxKind::IndexSignature
            | SyntaxKind::ClassStaticBlockDeclaration
            | SyntaxKind::SemicolonClassElement
    )
}

// Go: ast/utilities.go:603 IsMethodOrAccessor
pub fn is_method_or_accessor(node: Node) -> bool {
    matches!(
        node.kind(),
        SyntaxKind::MethodDeclaration | SyntaxKind::GetAccessor | SyntaxKind::SetAccessor
    )
}

// The kinds of `IsPropertyDeclaration || IsMethodOrAccessor` in Go
// `IsPrivateIdentifierClassElementDeclaration` (ast/utilities.go:562).
static PRIVATE_IDENTIFIER_CLASS_ELEMENT_KINDS: KindSet = KindSet::of(&[
    SyntaxKind::PropertyDeclaration,
    SyntaxKind::MethodDeclaration,
    SyntaxKind::GetAccessor,
    SyntaxKind::SetAccessor,
]);

// Go: ast/utilities.go:611 IsPrivateIdentifierClassElementDeclaration
#[inline]
pub fn is_private_identifier_class_element_declaration(node: Node) -> bool {
    // PERF: one kind lookup for `IsPropertyDeclaration || IsMethodOrAccessor`.
    PRIVATE_IDENTIFIER_CLASS_ELEMENT_KINDS.contains(node.kind())
        && is_private_identifier(node.name())
}

// Go: ast/utilities.go:615 IsObjectLiteralOrClassExpressionMethodOrAccessor
pub fn is_object_literal_or_class_expression_method_or_accessor(node: Node) -> bool {
    let kind = node.kind();
    (kind == SyntaxKind::MethodDeclaration
        || kind == SyntaxKind::GetAccessor
        || kind == SyntaxKind::SetAccessor)
        && (node.parent().kind() == SyntaxKind::ObjectLiteralExpression
            || node.parent().kind() == SyntaxKind::ClassExpression)
}

// Go: ast/utilities.go:621 IsTypeElement
pub fn is_type_element(node: Node) -> bool {
    matches!(
        node.kind(),
        SyntaxKind::ConstructSignature
            | SyntaxKind::CallSignature
            | SyntaxKind::PropertySignature
            | SyntaxKind::MethodSignature
            | SyntaxKind::IndexSignature
            | SyntaxKind::GetAccessor
            | SyntaxKind::SetAccessor
            | SyntaxKind::NotEmittedTypeElement
    )
}

// Go: ast/utilities.go:636 IsObjectLiteralElement
pub fn is_object_literal_element(node: Node) -> bool {
    matches!(
        node.kind(),
        SyntaxKind::PropertyAssignment
            | SyntaxKind::ShorthandPropertyAssignment
            | SyntaxKind::SpreadAssignment
            | SyntaxKind::MethodDeclaration
            | SyntaxKind::GetAccessor
            | SyntaxKind::SetAccessor
    )
}

// Go: ast/utilities.go:649 IsObjectLiteralMethod
pub fn is_object_literal_method(node: Node) -> bool {
    node.is_some()
        && node.kind() == SyntaxKind::MethodDeclaration
        && node.parent().kind() == SyntaxKind::ObjectLiteralExpression
}

// Go: ast/utilities.go:653 IsAutoAccessorPropertyDeclaration
pub fn is_auto_accessor_property_declaration(node: Node) -> bool {
    is_property_declaration(node) && has_accessor_modifier(node)
}

// Go: ast/utilities.go:657 IsParameterPropertyDeclaration
pub fn is_parameter_property_declaration(node: Node, parent: Node) -> bool {
    is_parameter_declaration(node)
        && has_syntactic_modifier(node, ModifierFlags::PARAMETER_PROPERTY_MODIFIER)
        && parent.kind() == SyntaxKind::Constructor
}

// Go: ast/utilities.go:661 IsJsxChild
pub fn is_jsx_child(node: Node) -> bool {
    matches!(
        node.kind(),
        SyntaxKind::JsxElement
            | SyntaxKind::JsxExpression
            | SyntaxKind::JsxSelfClosingElement
            | SyntaxKind::JsxText
            | SyntaxKind::JsxFragment
    )
}

// Go: ast/utilities.go:673 IsJsxAttributeLike
pub fn is_jsx_attribute_like(node: Node) -> bool {
    is_jsx_attribute(node) || is_jsx_spread_attribute(node)
}

// The kinds of Go `isDeclarationStatementKind` (ast/utilities.go:628).
const DECLARATION_STATEMENT_KIND_LIST: &[SyntaxKind] = &[
    SyntaxKind::FunctionDeclaration,
    SyntaxKind::MissingDeclaration,
    SyntaxKind::ClassDeclaration,
    SyntaxKind::InterfaceDeclaration,
    SyntaxKind::TypeAliasDeclaration,
    SyntaxKind::JsTypeAliasDeclaration,
    SyntaxKind::EnumDeclaration,
    SyntaxKind::ModuleDeclaration,
    SyntaxKind::ImportDeclaration,
    SyntaxKind::JsImportDeclaration,
    SyntaxKind::ImportEqualsDeclaration,
    SyntaxKind::ExportDeclaration,
    SyntaxKind::ExportAssignment,
    SyntaxKind::NamespaceExportDeclaration,
];

static DECLARATION_STATEMENT_KINDS: KindSet = KindSet::of(DECLARATION_STATEMENT_KIND_LIST);

// Go: ast/utilities.go:677 isDeclarationStatementKind
#[inline]
fn is_declaration_statement_kind(kind: SyntaxKind) -> bool {
    DECLARATION_STATEMENT_KINDS.contains(kind)
}

// Determines whether a node is a DeclarationStatement. Ideally this does not use Parent pointers, but it may use them
// to rule out a Block node that is part of `try` or `catch` or is the Block-like body of a function.
//
// NOTE: ECMA262 would just call this a Declaration
// Go: ast/utilities.go:702 IsDeclarationStatement
pub fn is_declaration_statement(node: Node) -> bool {
    is_declaration_statement_kind(node.kind())
}

// The kinds of Go `isStatementKindButNotDeclarationKind` (ast/utilities.go:657).
const STATEMENT_BUT_NOT_DECLARATION_KIND_LIST: &[SyntaxKind] = &[
    SyntaxKind::BreakStatement,
    SyntaxKind::ContinueStatement,
    SyntaxKind::DebuggerStatement,
    SyntaxKind::DoStatement,
    SyntaxKind::ExpressionStatement,
    SyntaxKind::EmptyStatement,
    SyntaxKind::ForInStatement,
    SyntaxKind::ForOfStatement,
    SyntaxKind::ForStatement,
    SyntaxKind::IfStatement,
    SyntaxKind::LabeledStatement,
    SyntaxKind::ReturnStatement,
    SyntaxKind::SwitchStatement,
    SyntaxKind::ThrowStatement,
    SyntaxKind::TryStatement,
    SyntaxKind::VariableStatement,
    SyntaxKind::WhileStatement,
    SyntaxKind::WithStatement,
    SyntaxKind::NotEmittedStatement,
];

static STATEMENT_BUT_NOT_DECLARATION_KINDS: KindSet =
    KindSet::of(STATEMENT_BUT_NOT_DECLARATION_KIND_LIST);

// The two statement kind lists together, for `is_statement`.
static STATEMENT_KINDS: KindSet =
    KindSet::of(STATEMENT_BUT_NOT_DECLARATION_KIND_LIST).with(DECLARATION_STATEMENT_KIND_LIST);

// Go: ast/utilities.go:706 isStatementKindButNotDeclarationKind
#[inline]
fn is_statement_kind_but_not_declaration_kind(kind: SyntaxKind) -> bool {
    STATEMENT_BUT_NOT_DECLARATION_KINDS.contains(kind)
}

// Determines whether a node is a Statement that is not also a Declaration. Ideally this does not use Parent pointers,
// but it may use them to rule out a Block node that is part of `try` or `catch` or is the Block-like body of a function.
//
// NOTE: ECMA262 would just call this a Statement
// Go: ast/utilities.go:736 IsStatementButNotDeclaration
pub fn is_statement_but_not_declaration(node: Node) -> bool {
    is_statement_kind_but_not_declaration_kind(node.kind())
}

// Determines whether a node is a Statement. Ideally this does not use Parent pointers, but it may use
// them to rule out a Block node that is part of `try` or `catch` or is the Block-like body of a function.
//
// NOTE: ECMA262 would call this either a StatementListItem or ModuleListItem
// Go: ast/utilities.go:744 IsStatement
#[inline]
pub fn is_statement(node: Node) -> bool {
    is_statement_ref(NodeRef::new(node))
}

/// `is_statement` on the view of the node.
// PERF: one lookup for `isStatementKindButNotDeclarationKind(kind) ||
// isDeclarationStatementKind(kind)`. Neither list holds `Block`. noderef1
// G2: the Block test reads the parent through the view (`NodeRef`).
#[inline]
pub fn is_statement_ref(node: NodeRef) -> bool {
    let kind = node.kind();
    STATEMENT_KINDS.contains(kind) || kind == SyntaxKind::Block && is_block_statement_ref(node)
}

// Determines whether a node is a BlockStatement. If parents are available, this ensures the Block is
// not part of a `try` statement, `catch` clause, or the Block-like body of a function
// Go: ast/utilities.go:751 isBlockStatement
// PERF: noderef1 G2. The node and its parent are read through one view
// (`NodeRef`): one registry lookup, where the Go reads made eight.
fn is_block_statement_ref(node: NodeRef) -> bool {
    if node.kind() != SyntaxKind::Block {
        return false;
    }
    let parent = node.parent();
    if parent.is_some()
        && matches!(
            parent.kind(),
            SyntaxKind::TryStatement | SyntaxKind::CatchClause
        )
    {
        return false;
    }
    // `!is_function_block(node)`: `node` is a Block.
    !(parent.is_some() && is_function_like_kind(parent.kind()))
}

// Determines whether a node is the Block-like body of a function by walking the parent of the node
// Go: ast/utilities.go:762 IsFunctionBlock
// PERF: noderef1 G2. One view (`NodeRef`) for the node and its parent.
pub fn is_function_block(node: Node) -> bool {
    if node.is_nil() {
        return false;
    }
    let node = NodeRef::new(node);
    node.kind() == SyntaxKind::Block && {
        let parent = node.parent();
        parent.is_some() && is_function_like_kind(parent.kind())
    }
}

// Go: ast/utilities.go:766 IsBlockOrCatchScoped
// PERF: lsshells M3 repair. `BLOCK_SCOPED` has no binder bit, so the walk
// reads no binder data (`get_combined_parser_flags`).
pub fn is_block_or_catch_scoped(declaration: Node) -> bool {
    !get_combined_parser_flags(declaration, NodeFlags::BLOCK_SCOPED).is_empty()
        || is_catch_clause_variable_declaration_or_binding_element(declaration)
}

// Go: ast/utilities.go:770 IsCatchClauseVariableDeclarationOrBindingElement
pub fn is_catch_clause_variable_declaration_or_binding_element(declaration: Node) -> bool {
    let node = get_root_declaration(declaration);
    node.kind() == SyntaxKind::VariableDeclaration
        && node.parent().kind() == SyntaxKind::CatchClause
}

// Go: ast/utilities.go:775 IsTypeNodeKind
pub fn is_type_node_kind(kind: SyntaxKind) -> bool {
    match kind {
        SyntaxKind::AnyKeyword
        | SyntaxKind::UnknownKeyword
        | SyntaxKind::NumberKeyword
        | SyntaxKind::BigIntKeyword
        | SyntaxKind::ObjectKeyword
        | SyntaxKind::BooleanKeyword
        | SyntaxKind::StringKeyword
        | SyntaxKind::SymbolKeyword
        | SyntaxKind::VoidKeyword
        | SyntaxKind::UndefinedKeyword
        | SyntaxKind::NeverKeyword
        | SyntaxKind::IntrinsicKeyword
        | SyntaxKind::ExpressionWithTypeArguments
        | SyntaxKind::JsDocAllType
        | SyntaxKind::JsDocNullableType
        | SyntaxKind::JsDocNonNullableType
        | SyntaxKind::JsDocOptionalType
        | SyntaxKind::JsDocVariadicType => return true,
        _ => {}
    }
    (kind as u16) >= (SyntaxKind::FIRST_TYPE_NODE as u16)
        && (kind as u16) <= (SyntaxKind::LAST_TYPE_NODE as u16)
}

// Go: ast/utilities.go:800 IsTypeNode
pub fn is_type_node(node: Node) -> bool {
    is_type_node_kind(node.kind())
}

// Go: ast/utilities.go:804 IsJSDocKind
pub fn is_js_doc_kind(kind: SyntaxKind) -> bool {
    (SyntaxKind::FIRST_JS_DOC_NODE as u16) <= (kind as u16)
        && (kind as u16) <= (SyntaxKind::LAST_JS_DOC_NODE as u16)
}

// Go: ast/utilities.go:808 IsJSDocTypeAssertion
pub fn is_js_doc_type_assertion(node: Node) -> bool {
    if node.is_nil() || !is_parenthesized_expression(node) || !is_in_js_file(node) {
        return false;
    }
    let expr = node.expression();
    is_as_expression(expr)
        && expr.type_().is_some()
        && expr.type_().flags().intersects(NodeFlags::REPARSED)
}

// Go: ast/utilities.go:816 IsPrologueDirective
pub fn is_prologue_directive(node: Node) -> bool {
    node.kind() == SyntaxKind::ExpressionStatement
        && node.expression().kind() == SyntaxKind::StringLiteral
}

// PORT: `crate::flags` defines the single-bit `OEK*` consts. The composite
// consts from the same Go const block are defined here.
impl OuterExpressionKinds {
    pub const OEK_ASSERTIONS: Self =
        Self(Self::OEK_TYPE_ASSERTIONS.0 | Self::OEK_NON_NULL_ASSERTIONS.0 | Self::OEK_SATISFIES.0);
    pub const OEK_ALL: Self = Self(
        Self::OEK_PARENTHESES.0
            | Self::OEK_ASSERTIONS.0
            | Self::OEK_PARTIALLY_EMITTED_EXPRESSIONS.0
            | Self::OEK_EXPRESSIONS_WITH_TYPE_ARGUMENTS.0,
    );
    pub const OEK_ALL_EXCEPT_ASSERTIONS_OR_EXPRESSIONS_WITH_TYPE_ARGUMENTS: Self = Self(
        Self::OEK_ALL.0 & !Self::OEK_ASSERTIONS.0 & !Self::OEK_EXPRESSIONS_WITH_TYPE_ARGUMENTS.0,
    );
    pub const OEK_EXPRESSION_TYPE_PASSTHROUGH: Self =
        Self(Self::OEK_PARENTHESES.0 | Self::OEK_ASSIGNMENTS.0 | Self::OEK_COMMA.0);
}

// Determines whether node is an "outer expression" of the provided kinds
// Go: ast/utilities.go:840 IsOuterExpression
pub fn is_outer_expression(node: Node, kinds: OuterExpressionKinds) -> bool {
    match node.kind() {
        SyntaxKind::ParenthesizedExpression => {
            return kinds.intersects(OuterExpressionKinds::OEK_PARENTHESES)
                && !(kinds.intersects(OuterExpressionKinds::OEK_EXCLUDE_JS_DOC_TYPE_ASSERTION)
                    && is_js_doc_type_assertion(node));
        }
        SyntaxKind::TypeAssertionExpression | SyntaxKind::AsExpression => {
            return kinds.intersects(OuterExpressionKinds::OEK_TYPE_ASSERTIONS);
        }
        SyntaxKind::SatisfiesExpression => {
            return kinds.intersects(
                OuterExpressionKinds::OEK_EXPRESSIONS_WITH_TYPE_ARGUMENTS
                    | OuterExpressionKinds::OEK_SATISFIES,
            );
        }
        SyntaxKind::ExpressionWithTypeArguments => {
            return kinds.intersects(OuterExpressionKinds::OEK_EXPRESSIONS_WITH_TYPE_ARGUMENTS);
        }
        SyntaxKind::NonNullExpression => {
            return kinds.intersects(OuterExpressionKinds::OEK_NON_NULL_ASSERTIONS);
        }
        SyntaxKind::PartiallyEmittedExpression => {
            return kinds.intersects(OuterExpressionKinds::OEK_PARTIALLY_EMITTED_EXPRESSIONS);
        }
        // PERF: binderview1. With neither the assignment nor the comma bit in
        // `kinds` the answer is false for every operator, so the operator
        // token is not read (`skip_partially_emitted_expressions`).
        SyntaxKind::BinaryExpression
            if kinds.intersects(
                OuterExpressionKinds::OEK_ASSIGNMENTS | OuterExpressionKinds::OEK_COMMA,
            ) =>
        {
            match node.operator_token().kind() {
                SyntaxKind::EqualsToken => {
                    return kinds.intersects(OuterExpressionKinds::OEK_ASSIGNMENTS);
                }
                SyntaxKind::CommaToken => {
                    return kinds.intersects(OuterExpressionKinds::OEK_COMMA);
                }
                _ => {}
            }
        }
        _ => {}
    }
    false
}

// Descends into an expression, skipping past "outer expressions" of the provided kinds
// Go: ast/utilities.go:866 SkipOuterExpressions
pub fn skip_outer_expressions(node: Node, kinds: OuterExpressionKinds) -> Node {
    let mut node = node;
    while is_outer_expression(node, kinds) {
        if is_binary_expression(node) {
            node = node.right();
        } else {
            node = node.expression();
        }
    }
    node
}

// Skips past the parentheses of an expression
// Go: ast/utilities.go:878 SkipParentheses
pub fn skip_parentheses(node: Node) -> Node {
    skip_outer_expressions(node, OuterExpressionKinds::OEK_PARENTHESES)
}

// Go: ast/utilities.go:882 SkipTypeParentheses
pub fn skip_type_parentheses(node: Node) -> Node {
    let mut node = node;
    while is_parenthesized_type_node(node) {
        node = node.type_();
    }
    node
}

// Go: ast/utilities.go:889 SkipPartiallyEmittedExpressions
pub fn skip_partially_emitted_expressions(node: Node) -> Node {
    skip_outer_expressions(
        node,
        OuterExpressionKinds::OEK_PARTIALLY_EMITTED_EXPRESSIONS,
    )
}

// Walks up the parents of a parenthesized expression to find the containing node
// Go: ast/utilities.go:894 WalkUpParenthesizedExpressions
pub fn walk_up_parenthesized_expressions(node: Node) -> Node {
    let mut node = node;
    while node.is_some() && node.kind() == SyntaxKind::ParenthesizedExpression {
        node = node.parent();
    }
    node
}

// Walks up the parents of a parenthesized type to find the containing node
// Go: ast/utilities.go:902 WalkUpParenthesizedTypes
pub fn walk_up_parenthesized_types(node: Node) -> Node {
    let mut node = node;
    while node.is_some() && node.kind() == SyntaxKind::ParenthesizedType {
        node = node.parent();
    }
    node
}

// Walks up the parents of a node to find the containing SourceFile
// Go: ast/utilities.go:910 GetSourceFileOfNode
// PORT: a frozen store node whose parent walk ends at its store root gets
// that root in O(1) (`ast::store::frozen_source_file_of_node`). Other nodes
// (synthetic nodes, nodes before the freeze) walk the parents like Go.
pub fn get_source_file_of_node(node: Node) -> Node {
    if let Some(root) = frozen_source_file_of_node(node) {
        debug_assert_eq!(root, walk_to_source_file(node));
        return root;
    }
    walk_to_source_file(node)
}

/// The Go `GetSourceFileOfNode` parent walk.
// PERF: noderef1 G2. Through views (`NodeRef`): no registry lookup for a
// parent in the same store.
fn walk_to_source_file(node: Node) -> Node {
    let mut node = NodeRef::new(node);
    while node.is_some() {
        if node.kind() == SyntaxKind::SourceFile {
            return node.node();
        }
        node = node.parent();
    }
    Node::NIL
}

// Go: ast/utilities.go:926 newParentInChildrenSetter
// PORT: Go keeps `parent` in a closure state and recurses through
// `node.ForEachChild(state.visit)`. Rust carries the same state in a
// struct. Go pools the closure with `sync.Pool`; that is only an
// allocation cache, so it is not ported.
struct ParentInChildrenSetter {
    parent: Node,
}

impl ParentInChildrenSetter {
    fn visit(&mut self, node: Node) -> bool {
        if self.parent.is_some() {
            crate::ast::synthetic::set_node_parent(node, self.parent);
        }
        let save_parent = self.parent;
        self.parent = node;
        node.for_each_child(|child| self.visit(child));
        self.parent = save_parent;
        false
    }
}

fn new_parent_in_children_setter() -> ParentInChildrenSetter {
    ParentInChildrenSetter { parent: Node::NIL }
}

// Go: ast/utilities.go:948 SetParentInChildren
pub fn set_parent_in_children(node: Node) {
    let mut f = new_parent_in_children_setter();
    f.visit(node);
}

#[cfg(test)]
mod symbol_id_tests {
    use super::*;

    /// Two checker arenas made from one binder arena share the ids of the
    /// binder symbols. Their own symbols (`is_own_index`) get ids of their
    /// own, also at one index in both arenas, and no later bind uses their
    /// index, so a checker that catches up reads the later binder symbol
    /// with its shared id.
    #[test]
    fn checker_symbols_have_ids_of_their_own() {
        let mut binder = SymbolArena::new();
        let shared = binder.new_symbol(SymbolFlags::NONE, "shared");
        let mut first = binder.for_checker();
        // A later program binds more files into the same binder arena.
        let later = binder.new_symbol(SymbolFlags::NONE, "later");
        let mut second = binder.for_checker();
        let own_first = first.new_symbol(SymbolFlags::NONE, "own");
        let own_second = second.new_symbol(SymbolFlags::NONE, "own");
        assert_eq!(
            own_first, own_second,
            "the test needs one index for two symbols"
        );
        assert!(is_own_index(own_first.0) && !is_own_index(later.0));

        let id_later = get_symbol_id(&second, later);
        let id_own_first = get_symbol_id(&first, own_first);
        let id_own_second = get_symbol_id(&second, own_second);
        assert_ne!(id_own_first, id_later);
        assert_ne!(id_own_first, id_own_second);
        assert_ne!(id_own_second, id_later);
        assert_eq!(get_symbol_id(&binder, later), id_later);
        assert_eq!(
            get_symbol_id(&first, shared),
            get_symbol_id(&second, shared)
        );
        // An id stays when the thread switches between arenas.
        assert_eq!(get_symbol_id(&first, own_first), id_own_first);
        assert_eq!(get_symbol_id(&second, own_second), id_own_second);
        // `first` catches up to the later bind.
        assert_eq!(first.symbol_at_slot(second.id_slot(later)), None);
        first.catch_up(&binder, &[]);
        assert_eq!(first.symbol_at_slot(second.id_slot(later)), Some(later));
        assert_eq!(first.sym(later).name.as_str(), "later");
        assert_eq!(get_symbol_id(&first, later), id_later);
        assert_eq!(get_symbol_id(&first, own_first), id_own_first);

        // A dropped checker arena frees its ids on this thread.
        let key = first.id_slot(own_first).key;
        assert_ne!(key, 0, "a checker symbol has an id of its own");
        drop(first);
        OWN_SYMBOL_IDS.with(|own| {
            let own = own.borrow();
            assert!(own.key != key && !own.others.contains_key(&key));
        });
    }

    /// A shadow (`SymbolArena::push_shadow`) is the same Go symbol as its
    /// origin: one id, given on first use through either, and kept after
    /// the arena of the origin drops.
    #[test]
    fn shadows_have_the_id_of_their_origin() {
        let mut binder = SymbolArena::new();
        let mut first = binder.for_checker();
        // A later program binds a symbol that `first` does not have.
        let later = binder.new_symbol(SymbolFlags::NONE, "later");
        let mut second = binder.for_checker();
        let own = first.new_symbol(SymbolFlags::TRANSIENT, "own");

        let origin = first.id_slot(own);
        let shadow = second.push_shadow(first.sym(own).clone(), origin);
        assert_eq!(second.symbol_at_slot(origin), Some(shadow));
        assert_eq!(first.symbol_at_slot(second.id_slot(shadow)), Some(own));
        let id = get_symbol_id(&second, shadow);
        assert_eq!(get_symbol_id(&first, own), id);

        let lineage = second.id_slot(later);
        assert_eq!(lineage.key, 0, "a binder symbol has the shared slot");
        assert_eq!(first.symbol_at_slot(lineage), None);
        let shadow_later = first.push_shadow(second.sym(later).clone(), lineage);
        assert_eq!(
            get_symbol_id(&first, shadow_later),
            get_symbol_id(&binder, later)
        );

        drop(first);
        assert_eq!(get_symbol_id(&second, shadow), id);
    }

    /// linkids1: a block id lives in the 4-byte cell of a binder symbol
    /// and of a checker symbol. The block counter comes with the ids to a
    /// new thread (`id_seed`) and back to the loading thread
    /// (`carry_symbol_ids`), so a generator there takes the next block, not
    /// the block of an id that a cell already holds. A thread with ids from
    /// 0 and no counter would give `BLOCK_ID_OFFSET` to a second symbol.
    #[test]
    fn block_ids_come_with_the_block_counter() {
        let mut binder = SymbolArena::new();
        let [a, b] = ["a", "b"].map(|name| binder.new_symbol(SymbolFlags::NONE, name));
        let mut checker = binder.for_checker();
        let own = checker.new_symbol(SymbolFlags::NONE, "own");
        let ids = SymbolIdGenerator::default();
        assert_eq!(ids.get_symbol_id(&checker, a), BLOCK_ID_OFFSET);
        assert_eq!(ids.get_symbol_id(&checker, own), BLOCK_ID_OFFSET + 1);
        assert_eq!(get_symbol_id(&checker, own), BLOCK_ID_OFFSET + 1);
        assert_eq!(own_symbol_id_count(), 0, "a block id is not a global id");
        let binder = std::sync::Arc::new(binder);
        let seed = id_seed();
        let worker = std::sync::Arc::clone(&binder);
        std::thread::spawn(move || {
            install_id_seed(seed);
            assert_eq!(get_symbol_id(&worker, a), BLOCK_ID_OFFSET);
            let ids = SymbolIdGenerator::default();
            assert_eq!(ids.get_symbol_id(&worker, b), BLOCK_ID_OFFSET + 256);
        })
        .join()
        .unwrap();
        // The carry of an empty worker moves nothing, with a log or without.
        for log in [Some(true), Some(false), None] {
            let seed = id_seed();
            carry_symbol_ids(forgotten_count(), |base| {
                let base = base.clone();
                vec![
                    std::thread::spawn(move || {
                        install_id_seed(seed);
                        if let Some(count_block_ids) = log {
                            record_symbol_ids(count_block_ids);
                        }
                        worker_symbol_ids(&base)
                    })
                    .join()
                    .unwrap(),
                ]
            });
            assert_eq!(get_symbol_id(&binder, a), BLOCK_ID_OFFSET);
            assert_eq!(next_symbol_block_id(), 256);
        }
        let ids = SymbolIdGenerator::default();
        assert_eq!(ids.get_symbol_id(&binder, b), BLOCK_ID_OFFSET + 256);
        // A global id and a block id past the cell range go to `big`.
        let mut big = FxHashMap::default();
        for id in [1, (1 << 31) - 1, 1 << 31, BLOCK_ID_OFFSET + (1 << 31)] {
            let mut cell = 0;
            assert_eq!(cell_id(&mut cell, &mut big, id, || id), id);
            assert_eq!(cell_id(&mut cell, &mut big, id, || 0), id);
        }
        assert_eq!(big.len(), 2);
    }

    /// idcarry1: the carry of two workers into the loading thread
    /// (`carry_symbol_ids`). A symbol that both workers gave an id keeps the
    /// one with the lower clock, a block id or a global id. A symbol that
    /// the loading thread gave an id after the seed keeps it. The global
    /// counter counts the winning global ids and the ids of checker
    /// symbols, and the blocks of the two workers get blocks of their own.
    /// With one worker every id stays.
    #[test]
    fn carried_ids_take_the_first_id_and_stay_unique() {
        const O: u64 = BLOCK_ID_OFFSET;
        let mut binder = SymbolArena::new();
        let names = [
            "pre", "p", "q", "r", "s", "t", "u", "v", "w", "x", "late", "y", "z", "y2", "z2",
        ];
        let [pre, p, q, r, s, t, u, v, w, x, late, y, z, y2, z2] =
            names.map(|name| binder.new_symbol(SymbolFlags::NONE, name));
        let binder = std::sync::Arc::new(binder);
        assert_eq!(get_symbol_id(&binder, pre), 1);
        // Runs `f` on a worker that starts from `seed` and records with
        // `log` (`record_symbol_ids`; None: no log).
        let worker = |seed: IdSeed,
                      base: &SymbolIdBase,
                      log: Option<bool>,
                      f: Box<dyn FnOnce(&SymbolArena) + Send>| {
            let (binder, base) = (std::sync::Arc::clone(&binder), base.clone());
            std::thread::spawn(move || {
                install_id_seed(seed);
                if let Some(count_block_ids) = log {
                    record_symbol_ids(count_block_ids);
                }
                f(&binder);
                worker_symbol_ids(&base)
            })
            .join()
            .unwrap()
        };
        let seed = id_seed();
        // The loading thread gives `late` an id after the seed.
        assert_eq!(get_symbol_id(&binder, late), 2);
        carry_symbol_ids(forgotten_count(), |base| {
            let a = worker(
                seed.clone(),
                base,
                Some(true),
                Box::new(move |binder| {
                    let ids = SymbolIdGenerator::default();
                    assert_eq!(ids.get_symbol_id(binder, p), O); // clock 0
                    assert_eq!(ids.get_symbol_id(binder, q), O + 1); // clock 1
                    let mut checker = binder.for_checker();
                    let own = checker.new_symbol(SymbolFlags::NONE, "own");
                    assert_eq!(get_symbol_id(&checker, own), 2); // clock 2
                    assert_eq!(get_symbol_id(binder, r), 3); // clock 3: wins
                    assert_eq!(ids.get_symbol_id(binder, s), O + 2); // clock 4: wins
                }),
            );
            let b = worker(
                seed,
                base,
                Some(true),
                Box::new(move |binder| {
                    let ids = SymbolIdGenerator::default();
                    for (n, symbol) in [t, u, v, w, x].into_iter().enumerate() {
                        assert_eq!(ids.get_symbol_id(binder, symbol), O + n as u64);
                    }
                    assert_eq!(ids.get_symbol_id(binder, r), O + 5); // clock 5
                    assert_eq!(get_symbol_id(binder, s), 2); // clock 6
                    assert_eq!(get_symbol_id(binder, late), 3);
                }),
            );
            vec![a, b]
        });
        assert_eq!(get_symbol_id(&binder, pre), 1);
        assert_eq!(get_symbol_id(&binder, late), 2);
        // The checker symbol of worker a took 3.
        assert_eq!(get_symbol_id(&binder, r), 4);
        for (symbol, id) in [(p, O), (q, O + 1), (s, O + 2)] {
            assert_eq!(get_symbol_id(&binder, symbol), id);
        }
        for (n, symbol) in [t, u, v, w, x].into_iter().enumerate() {
            assert_eq!(get_symbol_id(&binder, symbol), O + 256 + n as u64);
        }
        assert_eq!(next_ids().1, 4);
        assert_eq!(next_symbol_block_id(), 512);

        // One worker, and no id on this thread since the seed: every id
        // stays, with a log that does not count block ids (a one-checker
        // pool of `tsc -b`) or with no log (`program::IdCarry::only_from`).
        for (log, [y, z], next) in [(Some(false), [y, z], 5), (None, [y2, z2], 7)] {
            let seed = id_seed();
            let block = next_symbol_block_id();
            carry_symbol_ids(forgotten_count(), |base| {
                vec![worker(
                    seed,
                    base,
                    log,
                    Box::new(move |binder| {
                        let ids = SymbolIdGenerator::default();
                        assert_eq!(ids.get_symbol_id(binder, y), O + block);
                        assert_eq!(get_symbol_id(binder, z), next);
                        let mut checker = binder.for_checker();
                        let own = checker.new_symbol(SymbolFlags::NONE, "own");
                        assert_eq!(get_symbol_id(&checker, own), next + 1);
                    }),
                )]
            });
            assert_eq!(get_symbol_id(&binder, y), O + block);
            assert_eq!(get_symbol_id(&binder, z), next);
            assert_eq!(next_ids().1, next + 1);
            assert_eq!(next_symbol_block_id(), block + 256);
        }
    }

    /// A worker that counts its block ids (`record_symbol_ids(true)`) takes
    /// the other path for each one (`SymbolIdGenerator::at_last`), and
    /// gives the same ids as one that does not: whole blocks, in order. Its
    /// log has the block ids before each block and global id. When the log
    /// ends (`worker_symbol_ids`) the generator goes on in its block.
    #[test]
    fn counted_block_ids_are_the_same_ids() {
        const O: u64 = BLOCK_ID_OFFSET;
        let mut binder = SymbolArena::new();
        let symbols: Vec<_> = (0..300)
            .map(|n| binder.new_symbol(SymbolFlags::NONE, &format!("s{n}")))
            .collect();
        let global = binder.new_symbol(SymbolFlags::NONE, "global");
        let binder = std::sync::Arc::new(binder);
        let mut given = Vec::new();
        for count_block_ids in [false, true] {
            let seed = id_seed();
            let binder = std::sync::Arc::clone(&binder);
            let symbols = symbols.clone();
            let base = SymbolIdBase(std::sync::Arc::new(LineageIds::new()));
            given.push(
                std::thread::spawn(move || {
                    install_id_seed(seed);
                    record_symbol_ids(count_block_ids);
                    let ids = SymbolIdGenerator::default();
                    let mut given = Vec::new();
                    for (n, &symbol) in symbols[..299].iter().enumerate() {
                        if n == 260 {
                            assert_eq!(get_symbol_id(&binder, global), 1);
                        }
                        given.push(ids.get_symbol_id(&binder, symbol));
                    }
                    let carried = worker_symbol_ids(&base);
                    assert!(carried.unlogged.is_none());
                    let log = (carried.log.blocks, carried.log.globals[0].blocks_before);
                    given.push(ids.get_symbol_id(&binder, symbols[299]));
                    assert_eq!(ids.last.get(), ids.end.get());
                    (given, log)
                })
                .join()
                .unwrap(),
            );
        }
        let ids: Vec<u64> = (0..300).map(|n| O + n).collect();
        assert_eq!(given[0].0, ids);
        assert_eq!(given[1].0, ids);
        assert_eq!(given[0].1, (vec![(0, 0), (256, 0)], 0));
        assert_eq!(given[1].1, (vec![(0, 0), (256, 256)], 260));
    }

    /// watchreset1r: a forgotten lineage range (`forget_lineage_symbol_ids`)
    /// gets new ids, as the symbols of a new parse do. The carry of a pool
    /// made before the forget keeps none of its workers' ids of the range,
    /// the old one or a new one, but counts its global ids. The carry of a
    /// pool made after it keeps them.
    #[test]
    fn forgotten_lineage_symbols_get_new_ids_and_stay_out_of_older_carries() {
        let mut binder = SymbolArena::new();
        let [a, b, c, d] =
            ["a", "b", "c", "d"].map(|name| binder.new_symbol(SymbolFlags::NONE, name));
        let binder = std::sync::Arc::new(binder);
        let (a_at, b_at) = (
            binder.id_slot(a).place as usize,
            binder.id_slot(b).place as usize,
        );
        assert_eq!(b_at, a_at + 1);
        assert_eq!(get_symbol_id(&binder, a), 1);
        assert_eq!(get_symbol_id(&binder, c), 2);
        let worker =
            |seed: IdSeed, base: &SymbolIdBase, f: Box<dyn FnOnce(&SymbolArena) + Send>| {
                let (binder, base) = (std::sync::Arc::clone(&binder), base.clone());
                std::thread::spawn(move || {
                    install_id_seed(seed);
                    record_symbol_ids(false);
                    f(&binder);
                    worker_symbol_ids(&base)
                })
                .join()
                .unwrap()
            };

        // A pool made before the forget.
        let (seed, seen) = (id_seed(), forgotten_count());
        forget_lineage_symbol_ids(&[a_at..b_at + 1], seen);
        carry_symbol_ids(seen, |base| {
            vec![worker(
                seed,
                base,
                Box::new(move |binder| {
                    assert_eq!(get_symbol_id(binder, a), 1);
                    assert_eq!(get_symbol_id(binder, b), 3);
                    assert_eq!(get_symbol_id(binder, d), 4);
                }),
            )]
        });
        assert_eq!(get_symbol_id(&binder, c), 2);
        assert_eq!(get_symbol_id(&binder, d), 4);
        assert_eq!(next_ids().1, 4, "b's id counts");

        // A pool made after the forget.
        let (seed, seen) = (id_seed(), forgotten_count());
        carry_symbol_ids(seen, |base| {
            vec![worker(
                seed,
                base,
                Box::new(move |binder| assert_eq!(get_symbol_id(binder, a), 5)),
            )]
        });
        assert_eq!(get_symbol_id(&binder, a), 5);
        assert_eq!(get_symbol_id(&binder, b), 6);

        // With no pool made before it, a forget leaves the log.
        assert_eq!(forgotten_after(0), vec![a_at..b_at + 1]);
        forget_lineage_symbol_ids(&[], forgotten_count());
        assert!(forgotten_after(0).is_empty());
        assert_eq!(get_symbol_id(&binder, a), 5);
    }

    /// An id chunk of `LineageIds` holds the symbols of one `SymbolArena`
    /// chunk, so a freed lineage range frees whole id chunks.
    #[test]
    fn lineage_id_chunks_are_symbol_chunks() {
        let mut arena = SymbolArena::new_file();
        arena.new_symbol(SymbolFlags::NONE, "a");
        arena.end_chunk();
        assert_eq!(arena.symbol_count(), LINEAGE_ID_CHUNK);
    }

    /// When the lineage frees the symbols of a dead file version, a thread
    /// frees its id chunks of them when it next makes a chunk. The other
    /// ids stay, and an id read of a freed symbol panics.
    #[test]
    #[should_panic(expected = "file version 4194297 is released")]
    fn freed_lineage_symbols_free_their_ids() {
        const FILE: usize = (1 << 22) - 7;
        // Far above the indexes that other tests bind: every thread of the
        // process frees this range.
        const START: usize = 1 << 26;
        let mut ids = LineageIds::new();
        let kept = ids.id(START - 1, next_global_id);
        let dying = ids.id(START + 5, next_global_id);
        assert_eq!(ids.id(START + 5, next_global_id), dying);

        free_lineage_symbol_ids(FILE, START..START + 2 * LINEAGE_ID_CHUNK);
        // Until the next new chunk the id stays.
        assert_eq!(ids.id(START + 5, next_global_id), dying);
        let later = ids.id(START + 2 * LINEAGE_ID_CHUNK, next_global_id);
        assert!(later > dying);
        assert!(ids.chunks[START >> LINEAGE_ID_SHIFT].is_none());
        assert_eq!(ids.id(START - 1, next_global_id), kept);
        ids.id(START + LINEAGE_ID_CHUNK + 1, next_global_id);
    }
}

#[cfg(test)]
mod binary_view_tests {
    use super::*;

    /// binderview1: a `BinaryView` holds what the field accessors read:
    /// before the publish (the accessor path), after it in an alias-free
    /// store (`FrozenIds::Direct`) and in a store with an alias slot (a
    /// synthetic right operand, `FrozenIds::Records`), and with no ids. Its
    /// build takes the store build lock (`crate::test_lock`).
    #[test]
    fn binary_view_reads_what_the_accessors_read() {
        let plain = new_file_store("/binaryview/a.ts", "x = y;");
        let aliased = new_file_store("/binaryview/b.ts", "x + s;");
        let make = |file: usize, operator: SyntaxKind, right: Node| {
            let f = NodeFactory::for_file(file);
            let left = f.new_identifier("x");
            let operator_token = f.new_token(operator);
            f.new_binary_expression(ModifierList::NIL, left, Node::NIL, operator_token, right)
        };
        let a = make(
            plain,
            SyntaxKind::EqualsToken,
            NodeFactory::for_file(plain).new_identifier("y"),
        );
        let b = make(
            aliased,
            SyntaxKind::PlusToken,
            NodeFactory::new().new_identifier("s"),
        );
        let check = |when: &str| {
            for n in [a, b] {
                let operator_token = n.operator_token();
                let read = BinaryView {
                    left: n.left(),
                    operator_token,
                    operator: operator_token.kind(),
                    right: n.right(),
                    type_: n.type_(),
                };
                assert_eq!(BinaryView::new(n), read, "{when}");
                assert_eq!(BinaryView::with_ids(n, None), read, "{when}");
            }
        };
        check("built");
        freeze_file_store(plain);
        freeze_file_store(aliased);
        crate::program::publish_parsed_files("/");
        assert!(frozen_store_ast_node(a).is_some(), "a is published");
        assert!(matches!(
            frozen_store_ids(plain),
            Some(FrozenIds::Direct { .. })
        ));
        assert!(matches!(
            frozen_store_ids(aliased),
            Some(FrozenIds::Records { .. })
        ));
        check("published");
        assert_eq!(BinaryView::new(a).operator, SyntaxKind::EqualsToken);
        assert_eq!(BinaryView::new(b).operator, SyntaxKind::PlusToken);
    }
}
