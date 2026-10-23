//! Port of `checker/links.go` (tsgo#4329, ts#64711).
//!
//! PORT: Go adds `core.PagedLinkStore` (pages of 256 values in an arena)
//! and the two dense stores below on top of it; its other link stores are
//! hash maps (`core.LinkStore`). A dense store keys a node or symbol that
//! has no id by a block id from its own generator (ts#64711), so its links
//! of one page sit together. `core::LinkStore` is paged for every store:
//! node keys use per-file page tables of 64-key pages, arena keys (symbols)
//! use one page table. A dense page holds a place for the value of every
//! key; a sparse page (`SparseLinkStore`) only the values that its keys
//! have (the `Checker` fields say which stores are sparse). `has` and
//! `try_get` answer "a record exists", as Go's `TryGet` does.

use crate::prelude::*;

// Go: checker/links.go:16 nodeLinkStore
/// A links store keyed by node references (Go stores the values in the pages).
// PORT: sparse pages: about 20% of the keys of a page have a record. Go's
// store gives each node it reads a block node id (`NodeIdGenerator`); the
// port gives none, as no node id reaches output.
pub type NodeLinkStore<V> = SparseLinkStore<Node, V>;

// Go: checker/links.go:42 symbolLinkStore
/// `Checker::value_symbol_links`, Go's one `symbolLinkStore`. Go keys the
/// store by the symbol's id, so each `Get`, `TryGet` and `Has` gives a
/// symbol that has no id a block id of the store's generator first
/// (`SymbolIdGenerator`, `BLOCK_ID_OFFSET` + n). Only the explicit
/// `ast.GetSymbolId` calls give global ids. A late-bound name holds the id
/// of its unique symbol (`__@k@<id>`, Go `getESSymbolLikeTypeForNode`): the
/// node builder counts the length of that name toward truncation, and
/// `compare_symbols` compares such names as text. So each read must give
/// the id that Go's gives, in Go's order.
// PORT: the store is keyed by the arena index; only the ids are Go's. A
// record notes that its symbol has an id (`has_id`), so later reads skip the
// id call (each read with the call cost 1.3% (query) to 4.9% (zod) more user
// instructions). No other id is given between the read and the id. The
// store is a type of its own, so a read that gives no id
// (`LinkStore::get`) does not compile: each read is one of the methods
// below.
#[derive(Default)]
pub struct ValueSymbolLinkStore {
    ids: SymbolIdGenerator,
    links: LinkStore<SymbolId, ValueSymbolLinks>,
}

impl ValueSymbolLinkStore {
    /// Go `symbolLinkStore.Get`.
    #[inline]
    pub fn get_by_id(&mut self, symbols: &SymbolArena, symbol: SymbolId) -> &mut ValueSymbolLinks {
        let links = self.links.get(symbol);
        if !links.has_id {
            give_id(&self.ids, symbols, symbol, links);
        }
        links
    }

    /// Go `symbolLinkStore.Get` followed by writes to the new record
    /// (`LinkStore::insert_new`).
    #[inline]
    pub fn insert_new_by_id(
        &mut self,
        symbols: &SymbolArena,
        symbol: SymbolId,
        value: ValueSymbolLinks,
    ) -> &mut ValueSymbolLinks {
        self.ids.get_symbol_id(symbols, symbol);
        let links = self.links.insert_new(symbol, value);
        links.has_id = true;
        links
    }

    /// Go `symbolLinkStore.TryGet`.
    #[inline]
    pub fn try_get_by_id(
        &self,
        symbols: &SymbolArena,
        symbol: SymbolId,
    ) -> Option<&ValueSymbolLinks> {
        let links = self.links.try_get(symbol);
        if !links.is_some_and(|links| links.has_id) {
            give_id_without_record(&self.ids, symbols, symbol);
        }
        links
    }

    /// Go `symbolLinkStore.Has`.
    #[inline]
    pub fn has_by_id(&self, symbols: &SymbolArena, symbol: SymbolId) -> bool {
        self.try_get_by_id(symbols, symbol).is_some()
    }

    /// Go `symbolLinkStore.Get` of a symbol whose record exists and notes
    /// its id (`has_id`): the read gives no id, as Go's gives none.
    /// `type_resolution_has_property` reads the record of a pushed type
    /// resolution, and each push site reads the symbol with `get_by_id`
    /// first.
    #[inline]
    pub fn get_noted(&mut self, symbol: SymbolId) -> &mut ValueSymbolLinks {
        let links = self.links.get(symbol);
        debug_assert!(links.has_id, "a noted read of a symbol with no id");
        links
    }

    /// The record of `symbol`, if any, with no id: a read of a port memo
    /// where Go reads no links. A record exists only for a symbol that has
    /// its id: each read that adds one gives it.
    #[inline]
    pub fn try_get_without_id(&self, symbol: SymbolId) -> Option<&ValueSymbolLinks> {
        self.links.try_get(symbol)
    }
}

/// The first read of `symbol` in `get_by_id`: gives it its id and notes it.
// PERF: out of line, so each inlined read adds only the test.
#[cold]
#[inline(never)]
fn give_id(
    ids: &SymbolIdGenerator,
    symbols: &SymbolArena,
    symbol: SymbolId,
    links: &mut ValueSymbolLinks,
) {
    ids.get_symbol_id(symbols, symbol);
    links.has_id = true;
}

/// `try_get_by_id` and `has_by_id` of `symbol` without a noted id: gives it
/// its id (a no-op when it has one).
#[cold]
#[inline(never)]
fn give_id_without_record(ids: &SymbolIdGenerator, symbols: &SymbolArena, symbol: SymbolId) {
    ids.get_symbol_id(symbols, symbol);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each read of `ValueSymbolLinkStore` that Go makes gives the symbol
    /// an id once, also when no record exists (`try_get_by_id`,
    /// `has_by_id`: Go `TryGet` and `Has` key by the id). At the Go pin
    /// (ts#64711) that id is a block id of the store's generator, not a
    /// global id: the global counter does not move. A symbol with a global
    /// id keeps it. The port memo read gives none. A second store (another
    /// checker) takes the next block. A test thread starts with no ids.
    #[test]
    fn each_go_read_gives_the_symbol_its_id_once() {
        let mut symbols = SymbolArena::new();
        let [tried, had, got, inserted, memo, global, other] =
            ["tried", "had", "got", "inserted", "memo", "global", "other"]
                .map(|name| symbols.new_symbol(SymbolFlags::NONE, name));
        let mut store = ValueSymbolLinkStore::default();
        let next = || (next_ids().1, next_symbol_block_id());
        assert_eq!(next(), (0, 0));

        assert!(store.try_get_by_id(&symbols, tried).is_none());
        assert_eq!(next(), (0, 256), "TryGet takes a block, with no record");
        assert!(!store.has_by_id(&symbols, had));
        store.get_by_id(&symbols, got);
        store.get_by_id(&symbols, got);
        assert!(store.try_get_by_id(&symbols, got).is_some());
        store.insert_new_by_id(&symbols, inserted, ValueSymbolLinks::default());
        assert!(store.try_get_without_id(memo).is_none());
        assert_eq!(get_symbol_id(&symbols, global), 1, "an explicit call");
        store.get_by_id(&symbols, global);
        assert_eq!(next(), (1, 256), "one block, one global id");
        let block = |n: u64| BLOCK_ID_OFFSET + n;
        assert_eq!(
            [tried, had, got, inserted, global].map(|symbol| get_symbol_id(&symbols, symbol)),
            [block(0), block(1), block(2), block(3), 1],
            "Get, TryGet and Has give block ids in read order"
        );
        assert_eq!(get_symbol_id(&symbols, memo), 2, "the memo read gave none");
        let second = ValueSymbolLinkStore::default();
        second.has_by_id(&symbols, other);
        assert_eq!(get_symbol_id(&symbols, other), block(256));
        assert_eq!(next(), (2, 512));
        // The text of a block id has 15 digits (late-bound names).
        assert_eq!(block(0).to_string(), "281474976710656");
    }

    /// The TS2322 message texts of `a.ts` (`source`) with `options`, in a
    /// program loaded on a new thread, with and without `singleThreaded`.
    fn messages(name: &str, options: &str, source: &str) -> [Vec<String>; 2] {
        [false, true].map(|single_threaded| {
            let dir = std::env::temp_dir().join(format!(
                "goport-link-ids-{name}-{single_threaded}-{}",
                std::process::id()
            ));
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("a.ts"), source).unwrap();
            let config = dir.join("tsconfig.json");
            std::fs::write(
                &config,
                format!(
                    r#"{{ "compilerOptions": {{ "strict": true, "noEmit": true, "types": [],
                         "singleThreaded": {single_threaded}, {options} }}, "files": ["a.ts"] }}"#
                ),
            )
            .unwrap();
            let messages = std::thread::spawn(move || {
                let program = crate::program::try_load_version(&config.to_string_lossy(), |_| {})
                    .unwrap_or_else(|e| panic!("cannot load {}: {e}", config.display()));
                let scope = crate::core::enter_program(Some(program));
                let messages: Vec<String> = program
                    .source_files()
                    .filter(|file| file.info.file_name.ends_with("/a.ts"))
                    .flat_map(|file| crate::program::get_semantic_diagnostics(file.root))
                    .map(|d| crate::program::format_diagnostic(&d))
                    .collect();
                drop(scope);
                crate::program::release_program(program);
                messages
            })
            .join()
            .unwrap();
            let _ = std::fs::remove_dir_all(&dir);
            messages
        })
    }

    /// linkids1 (ts#64711): the links of a unique symbol give it a block
    /// id, so its late-bound name (`__@k1@<id>`) has 15 digits, and the
    /// node builder counts that length toward truncation. Go at the pin
    /// prints these texts with and without `--singleThreaded`; with global
    /// ids (Go fed0bf24149f) it printed more members.
    #[test]
    fn late_bound_names_hold_block_ids() {
        let keys: Vec<String> = (1..=12).map(|i| format!("k{i}")).collect();
        let mut source: String = keys
            .iter()
            .map(|k| format!("declare const {k}: unique symbol;\n"))
            .collect();
        let members: Vec<String> = keys.iter().map(|k| format!("[{k}]: number; ")).collect();
        source += &format!(
            "declare const o: {{ {}}};\nconst x: string = o;\n",
            members.concat()
        );
        let lib = r#""target": "es2022", "lib": ["es2022"]"#;
        for messages in messages("trunc", lib, &source) {
            assert!(
                messages.len() == 1
                    && messages[0].contains(
                        "Type '{ [k1]: number; [k2]: number; [k3]: number; [k4]: number; \
                         [k5]: number; [k6]: number; ... 5 more ...; [k12]: number; }' \
                         is not assignable to type 'string'."
                    ),
                "{messages:?}"
            );
        }
        let props: Vec<String> = (1..=12).map(|i| format!("p{i}: number; ")).collect();
        let source = format!(
            "declare const o: {{ [Symbol.iterator](): Iterator<number>; \
             [Symbol.asyncIterator](): AsyncIterator<number>; \
             [Symbol.hasInstance](v: unknown): boolean; [Symbol.toPrimitive](h: string): number; \
             {}}};\nconst x: string = o;\n",
            props.concat()
        );
        for messages in messages("iter", lib, &source) {
            assert!(
                messages.len() == 1
                    && messages[0].contains(
                        "Type '{ [Symbol.iterator](): Iterator<number, any, any>; \
                         [Symbol.asyncIterator](): AsyncIterator<number, any, any>; \
                         ... 13 more ...; p12: number; }' is not assignable to type 'string'."
                    ),
                "{messages:?}"
            );
        }
    }

    /// linkids1 (go-note-check case mapord): the members of a mapped type
    /// over unique symbols have no declaration and are named
    /// `__@k@<id>`, so `compare_symbols` orders them by the text of their
    /// ids. Block ids all have 15 digits, so the text order is the read
    /// order: Go at the pin prints 0 to 13. With global ids ("10" < "5")
    /// Go fed0bf24149f printed 5 to 13, then 0 to 4.
    #[test]
    fn mapped_members_over_unique_symbols_keep_read_order() {
        let mut source = String::new();
        for global in [
            "Array<T>",
            "Boolean",
            "Function",
            "CallableFunction",
            "NewableFunction",
            "IArguments",
            "Number",
            "Object",
            "RegExp",
            "String",
            "Symbol",
        ] {
            source += &format!("interface {global} {{}}\n");
        }
        for i in 0..14 {
            source += &format!("namespace N{i} {{ export declare const k: unique symbol; }}\n");
        }
        let keys: Vec<String> = (0..14).map(|i| format!("typeof N{i}.k")).collect();
        let value: String = (0..14)
            .map(|i| format!("P extends typeof N{i}.k ? {i} : "))
            .collect();
        source += &format!(
            "declare const m: {{ [P in {}]: {value}never }};\nconst x: string = m;\n",
            keys.join(" | ")
        );
        let members: String = (0..14).map(|i| format!("[k]: {i}; ")).collect();
        let expected = format!("Type '{{ {members}}}' is not assignable to type 'string'.");
        let options = r#""noLib": true, "noErrorTruncation": true"#;
        for messages in messages("mapord", options, &source) {
            assert!(
                messages.len() == 1 && messages[0].contains(&expected),
                "{messages:?}"
            );
        }
    }
}
