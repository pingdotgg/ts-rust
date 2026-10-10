//! Port of `checker/links.go` (tsgo#4329).
//!
//! PORT: Go adds `core.PagedLinkStore` (pages of 256 values, a page list for
//! low page indexes and a map for high ones) and the two stores below on top
//! of it. `core::LinkStore` is paged the same way for every store: node keys
//! use per-file page tables of 64-key pages, arena keys (symbols) use one
//! page table, and a page holds the values. So both Go stores map onto
//! `core::LinkStore`; a store of large values holds `Box<V>`, like Go
//! `symbolArenaLinkStore`. `has` and `try_get` keep the exact "a record
//! exists" answer. Go
//! `nodeLinkStore.TryGet` also answers a zero record for a key in an
//! allocated page; its one reader (`tryGetResolvedSymbolFromTypeNode`) then
//! reads a nil `resolvedSymbol`, which is the same result as no record.

use crate::prelude::*;

// Go: checker/links.go:10 nodeLinkStore
/// A links store keyed by node references (Go stores the values in the pages).
pub type NodeLinkStore<V> = LinkStore<Node, V>;

// Go: checker/links.go:28 symbolArenaLinkStore
/// A links store keyed by symbol references (Go stores the values in an arena;
/// here they sit in the pages, as most keys of a page have a record).
pub type SymbolArenaLinkStore<V> = LinkStore<SymbolId, V>;

/// `Checker::value_symbol_links`, Go's one `symbolArenaLinkStore`. Go keys
/// the store by `ast.GetSymbolId`, so each `Get`, `TryGet` and `Has` gives
/// the symbol its id first. The ids count up in that order
/// (`ast::get_symbol_id`), and a late-bound name holds the id of its unique
/// symbol (`__@k@<id>`, Go `getESSymbolLikeTypeForNode`). The node builder
/// counts the length of that name toward truncation, so the ids must count
/// as Go's do.
// PORT: the store is still keyed by the arena index; only the id order is
// Go's. A record notes that its symbol has its id (`has_id`), so later reads
// skip `get_symbol_id` (each read with the call cost 1.3% (query) to 4.9%
// (zod) more user instructions). Ids are given in Go's order: no other id is
// given between the read and the id. The store is a type of its own, so a
// read that gives no id (`LinkStore::get`) does not compile: each read is
// one of the methods below.
#[derive(Default)]
pub struct ValueSymbolLinkStore(SymbolArenaLinkStore<ValueSymbolLinks>);

impl ValueSymbolLinkStore {
    /// Go `symbolArenaLinkStore.Get`.
    #[inline]
    pub fn get_by_id(&mut self, symbols: &SymbolArena, symbol: SymbolId) -> &mut ValueSymbolLinks {
        let links = self.0.get(symbol);
        if !links.has_id {
            give_id(symbols, symbol, links);
        }
        links
    }

    /// Go `symbolArenaLinkStore.Get` followed by writes to the new record
    /// (`LinkStore::insert_new`).
    #[inline]
    pub fn insert_new_by_id(
        &mut self,
        symbols: &SymbolArena,
        symbol: SymbolId,
        value: ValueSymbolLinks,
    ) -> &mut ValueSymbolLinks {
        get_symbol_id(symbols, symbol);
        let links = self.0.insert_new(symbol, value);
        links.has_id = true;
        links
    }

    /// Go `symbolArenaLinkStore.TryGet`.
    #[inline]
    pub fn try_get_by_id(
        &self,
        symbols: &SymbolArena,
        symbol: SymbolId,
    ) -> Option<&ValueSymbolLinks> {
        let links = self.0.try_get(symbol);
        if !links.is_some_and(|links| links.has_id) {
            give_id_without_record(symbols, symbol);
        }
        links
    }

    /// Go `symbolArenaLinkStore.Has`.
    #[inline]
    pub fn has_by_id(&self, symbols: &SymbolArena, symbol: SymbolId) -> bool {
        self.try_get_by_id(symbols, symbol).is_some()
    }

    /// Go `symbolArenaLinkStore.Get` of a symbol whose record exists and
    /// notes its id (`has_id`): the read gives no id, as Go's gives none.
    /// `type_resolution_has_property` reads the record of a pushed type
    /// resolution, and each push site reads the symbol with `get_by_id`
    /// first.
    #[inline]
    pub fn get_noted(&mut self, symbol: SymbolId) -> &mut ValueSymbolLinks {
        let links = self.0.get(symbol);
        debug_assert!(links.has_id, "a noted read of a symbol with no id");
        links
    }

    /// The record of `symbol`, if any, with no id: a read of a port memo
    /// where Go reads no links. A record exists only for a symbol that has
    /// its id: each read that adds one gives it.
    #[inline]
    pub fn try_get_without_id(&self, symbol: SymbolId) -> Option<&ValueSymbolLinks> {
        self.0.try_get(symbol)
    }
}

/// The first read of `symbol` in `get_by_id`: gives it its id and notes it.
// PERF: out of line, so each inlined read adds only the test.
#[cold]
#[inline(never)]
fn give_id(symbols: &SymbolArena, symbol: SymbolId, links: &mut ValueSymbolLinks) {
    get_symbol_id(symbols, symbol);
    links.has_id = true;
}

/// `try_get_by_id` and `has_by_id` of `symbol` without a noted id: gives it
/// its id (a no-op when it has one).
#[cold]
#[inline(never)]
fn give_id_without_record(symbols: &SymbolArena, symbol: SymbolId) {
    get_symbol_id(symbols, symbol);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each read of `ValueSymbolLinkStore` that Go makes gives the symbol
    /// its id once, also when no record exists (`try_get_by_id`,
    /// `has_by_id`: Go `TryGet` and `Has` key by `ast.GetSymbolId`). The
    /// port memo read gives none. A test thread starts with no ids.
    #[test]
    fn each_go_read_gives_the_symbol_its_id_once() {
        let mut symbols = SymbolArena::new();
        let [tried, had, got, inserted, memo] = ["tried", "had", "got", "inserted", "memo"]
            .map(|name| symbols.new_symbol(SymbolFlags::NONE, name));
        let mut store = ValueSymbolLinkStore::default();
        let next = || next_ids().1;
        assert_eq!(next(), 0);

        assert!(store.try_get_by_id(&symbols, tried).is_none());
        assert_eq!(next(), 1, "TryGet gives the id with no record");
        assert!(!store.has_by_id(&symbols, had));
        assert_eq!(next(), 2, "Has gives the id with no record");
        store.get_by_id(&symbols, got);
        store.get_by_id(&symbols, got);
        assert_eq!(next(), 3, "Get gives the id once");
        assert!(store.try_get_by_id(&symbols, got).is_some());
        store.insert_new_by_id(&symbols, inserted, ValueSymbolLinks::default());
        assert_eq!(next(), 4, "a new record gives the id");
        assert!(store.try_get_without_id(memo).is_none());
        assert_eq!(next(), 4, "the memo read gives no id");
        assert_eq!(
            [tried, had, got, inserted].map(|symbol| get_symbol_id(&symbols, symbol)),
            [1, 2, 3, 4]
        );
        assert_eq!(next(), 4);
    }
}
