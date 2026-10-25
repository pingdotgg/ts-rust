//! Per-file Go node stores (the nodes the ported Go parser creates) and the
//! file registry (the published stores and the `GoFile` of each file id).
//!
//! Go parser nodes are ordinary `*ast.Node` values made by `ast.NodeFactory`.
//! Here each parsed file gets one store. The store id is the file id, so a
//! store node is a normal `Node` handle: high 32 bits are the file id, low 32
//! bits are the slot index + 1.
//!
//! Each slot has a header (Go kind plus the mutable Go `NodeBase` fields:
//! parent, flags, loc) and, for a node slot, a `NodeData` view of its data
//! in the AST arena (the Go node data; the kind is in the header). A
//! freeable parse owns the data instead, boxed (`OwnedAst`).
//! Child ids inside that data are slot indexes of the same store:
//! - a child in the same file uses its own slot index;
//! - a child from another file or a synthetic child (Go shares the pointer)
//!   uses an alias slot, which `Node::new` resolves to that node;
//! - Go `nil` in a field that astdata stores as a required `NodeId` uses
//!   slot 0, which resolves to `Node::NIL`.
//!
//! `node.rs` reads kind, loc, flags and parent from the header of every
//! parsed node. Only synthetic nodes have no store.
//!
//! Phases of a store:
//! - Build: the parser runs on one thread and writes the stores of that
//!   thread (`BUILD`). The header and the data can change until the parser
//!   finishes the file (`finishNode`, parent setting, JSDoc flags,
//!   reparser.go writes). Then the parser freezes the file, which also
//!   builds the per-store tables that a publish puts in the registry. The
//!   build stores of a thread get consecutive ids from `BuildStores::base`,
//!   the published count when its first store was made. The last store
//!   made on a thread is its `ACTIVE` store: the parser reads and writes it
//!   without a store lookup.
//! - Detached: a parse worker (`files_parser.rs` prefetch) parses one file
//!   into a store with a provisional id (`DETACHED_STORE_BASE` + job) that
//!   only its thread sees (`DETACHED`). The loading thread adopts the
//!   finished store when the loader asks for that file
//!   (`adopt_detached_store`). The store then gets the next real id, so ids
//!   still follow the serial parse order.
//! - Published: the loader calls `publish_file_stores` with the `GoFile` of
//!   each build store before it installs the program. The stores and their
//!   `GoFile`s move into the process-wide, read-only registry. Node reads
//!   then need no thread-local and no `RefCell` borrow, and any thread can
//!   read them. Writes to a published store panic. A table that needs the
//!   real id of an adopted store is built then, on scoped threads for a
//!   large publish (`publish_stores`).
//!
//! The registry:
//! - One file id is one file version. Ids only grow (`PUBLISHED` is the
//!   next unused id), and a published file is never changed. A static
//!   published file is never freed; a freeable file version is freed with
//!   its last holder (lsshells M3b, `ast/file_version.rs`), and a read of
//!   its id after that panics. A new program version shares the ids of its
//!   unchanged files.
//! - AST node records, step 3: each published file id has one entry, its
//!   block (`FileBlock`, `file_block`): a static array for the ids below
//!   `LOW_BLOCKS` (`FILE_BLOCKS`), chunks made on demand above
//!   (`HIGH_BLOCKS`). It holds the per-slot columns of the file (records,
//!   kids, node column), and the rest of the file one load away
//!   (`BlockFile`: facts, root, foreign parents, links, and the store and
//!   `GoFile` of a static publish). The first program and every later one
//!   (edited files, other programs) read the same way.
//! - A freeable file version (an edited file in a language server or API
//!   process): its `FileVersion` owns its store and `GoFile`
//!   (`VersionStore`), and the block of its id is its node shell, with only
//!   its node columns leaked (`node_shell`). With owned nodes on
//!   (the default there, `owned_nodes_enabled`) its parse is a freeable
//!   parse (`enter_freeable_parse`, lsshells M3c), so its store also owns
//!   its astdata nodes, its pending lists and its parse lists (`OwnedAst`):
//!   the shell has no node column, and a node data read of the file is a
//!   scoped read (`read_store_node_miss`). With `GOPORT_OWNED_NODES=0` the
//!   parse is a static parse, whose node column is in the shell, as before
//!   M3c.
//! - Every read of a published store goes through one inline lookup
//!   (`file_block`). An id below `LOW_BLOCKS` reads with no call; a higher
//!   id reads in a cold block that calls the empty `high_block_path`. So
//!   the hot node reads in `node.rs`, the node records (`NodeRecord`,
//!   `NodeKids`) and the perf columns (links, facts) answer for the nodes
//!   of every program, and return `None` on a miss. A read of the store or
//!   the `GoFile` of a node shell reads the freeable version, out of line
//!   and pinned while the read runs (`with_published_store`,
//!   `try_with_go_file`); the accessors that return a borrow return a
//!   `FileRef` guard.
//! - After a registry miss, a synthetic id has no store (a few compares, no
//!   call). Any other id takes one cold call: the detached store, then the
//!   build stores of this thread.
//!
//! AST node records, step 2: the binder writes the symbol, the flow node
//! and the added flags of each node into its record while it binds
//! (`BindRecords`, bindonce2; a lib snapshot load writes them at its install,
//! `bind_store_records`). The other binder fields go to
//! `GoFile::node_bind`, by the index in the record.

use crate::astdata::{AstData, BoxedData, NodeData};
use crate::frontend::parser::SourceFileParseOptions;
use crate::leak_arena::LeakArena;
use crate::prelude::*;
use std::cell::Cell;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU8, AtomicU16, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

/// Slot 0: Go `nil` stored in a astdata field that has no `Option`.
const NIL_SLOT: u32 = 0;

/// Position that marks a Go `nil` list in a astdata list field that has no
/// `Option`. Go positions are byte offsets, so they never reach it, and
/// `u32::MAX` is already the undefined position `-1`.
// PORT: astdata cannot change under the R97 rules. For store nodes the factory
// stores Go `nil` in a required list field as an empty list at this position,
// and `NodeList::is_nil` reads it back as nil (plan risk 1).
pub const NIL_LIST_POS: u32 = u32::MAX - 1;

/// The Go kind and the mutable Go `NodeBase` fields of a store node, as the
/// node reads see them: the value form of its `NodeRecord`
/// (`NodeRecord::header`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NodeHeader {
    /// Go `node.Parent`.
    pub parent: Node,
    pub loc: TextRange,
    pub flags: NodeFlags,
    pub kind: SyntaxKind,
    /// Set by `mark_source_file_roots`: Go `GetSourceFileOfNode(node)` is the
    /// root of this store (`FileStore::root`). False means "walk the parents".
    source_file_is_root: bool,
    /// Set when the slot is made or its data is replaced, for an Identifier
    /// or PrivateIdentifier slot: Go `scanner.GetIdentifierToken(node.Text())
    /// != KindIdentifier` (`frozen_store_text_is_keyword`). False for other
    /// slots.
    text_is_keyword: bool,
}

impl NodeHeader {
    /// The stored form of `parent` for a node of store `file`.
    #[inline]
    fn stored_parent(file: usize, parent: Node) -> Node {
        if parent.is_some() && parent.file_index() == file {
            handle(LOCAL_STORE, slot_index(parent) as u32)
        } else {
            parent
        }
    }
}

/// The Go nodes of one parsed file. Slot `i` is `kinds[i]`, `records[i]`,
/// `kids[i]` and `nodes[i]`.
/// The default value is an empty placeholder with no slots.
#[derive(Default)]
struct FileStore {
    file_name: &'static str,
    /// The file text. A freeable file version shares it (`FileText::Shared`)
    /// with its parse and program inputs, and it goes with them.
    text: FileText,
    /// AST node records, step 1: the `NodeRecord` of every slot, pushed when
    /// the slot is made and written by the parse (`get_mut`). The publish
    /// reads them in place (`FileBlock::records`).
    records: Vec<NodeRecord>,
    /// AST node records, step 4: the Go `node.Kind` of every slot
    /// (`Unknown` for the nil slot and alias slots), pushed with its record.
    /// The publish reads it in place (`FileBlock::kinds`).
    kinds: Vec<SyntaxKind>,
    /// AST node records, step 2: the parents in another store of node
    /// slots (`ParentCode`), in the order the parse wrote them. The publish
    /// reads them in place (`BlockFile::foreign`).
    foreign_parents: Vec<Node>,
    /// AST node records, step 1: the `NodeKids` of every slot, pushed with
    /// its record and made again when its data is replaced
    /// (`FileBlock::kids`). U1 (d): the only copy of the text of a node
    /// that `alloc_store_name_node` or `alloc_store_shared_name_node` made
    /// (`store_identifier_name`).
    // PERF: the text is interned and the child ids are read while the new
    // node data is hot, not in a pass over every slot after the parse,
    // which loaded each data box again when it was cold.
    kids: Vec<NodeKids>,
    /// The node data of each node slot (its kind is in `kinds`). `None` for
    /// the nil slot and alias slots. A node that a freeable parse owns has
    /// the marker data here (`owned_marker`); its data is in `owned`.
    // PERF: astmem1 P1. An entry names the data, not a leaked astdata node
    // (40 B) whose kind, flags, range and parent no read used: the header of
    // a slot is in its record and `kinds`. astchunks2: the entry is the
    // 16 B (tag, reference) view, as Go's `nodeData` word, and the data is
    // in the AST arena, with no box and no 16 B `NodeData` value between.
    nodes: Vec<Option<ColumnData>>,
    /// astshrink1: the list table of a static store. Entry `i` is the list
    /// that astdata list field `NodeList::new(i)` names in the static node
    /// data of this store (`StaticList`); entry `NIL_LIST_INDEX` is the Go
    /// `nil` of a required list field. A store that owns its nodes keeps
    /// the lists of its owned data in `OwnedAst::pending`. A static publish
    /// keeps this table in the `BlockFile`.
    lists: Vec<&'static StaticList>,
    /// lsshells M3c: the astdata nodes, pending lists and parse lists of a
    /// freeable parse (`enter_freeable_parse`), which the store owns. `None`
    /// for a static parse, whose nodes are in the leaked AST arena.
    owned: Option<Box<OwnedAst>>,
    /// Alias slot of each foreign node, so one node gets one slot. Emptied
    /// by `publish_file_stores`.
    aliases: FxHashMap<Node, u32>,
    /// Set when the parser has finished the file.
    frozen: bool,
    /// Slot of the SourceFile node that `mark_source_file_roots` found when
    /// the file was frozen, or 0 (the nil slot).
    root_slot: u32,
    /// `file_store_parser_flags`, made on the parsing thread when the file
    /// is frozen. The loader takes it once.
    parser_flags: Option<Vec<NodeFlags>>,
    /// Go `file.jsdocCache`, set by `finishSourceFile`. Node reads use it
    /// until the file is published (its `GoFile` holds it then), so
    /// `publish_file_stores` empties it.
    jsdoc_cache: FxHashMap<Node, &'static [Node]>,
    /// Go `file.hasLazyJSDoc`, set by `finishSourceFile` for a non-JS file,
    /// with the inputs that Go `parseJSDocForNode` reads from the file
    /// (`ParseOptions()` and `ScriptKind`; the text is `text`). Node reads
    /// use it until the file is published. Then the `SourceFileInfo` of the
    /// file keeps the options and the script kind, and the store keeps the
    /// text (`program::resolve_lazy_js_doc`), so `publish_file_stores` drops
    /// it.
    lazy_js_doc: Option<(SourceFileParseOptions, ScriptKind)>,
    /// Go `file.jsdocCache` entries that `resolveJSDoc` adds before the file
    /// is published.
    // PORT: the parsed JSDoc nodes are synthetic nodes of the thread that
    // parsed them, so they are kept apart from `jsdoc_cache`.
    // `adopt_detached_store` (another thread) and `publish_file_stores`
    // drop them.
    lazy_jsdoc_cache: FxHashMap<Node, &'static [Node]>,
    /// Go `file.LanguageVariant` and the parse `file.Diagnostics()`,
    /// written by `finishSourceFile`. Reads of a file that is not published
    /// use them (`ast::source_file_language_variant`,
    /// `ast::source_file_diagnostics`), for example the format tests, which
    /// parse a file with no program.
    language_variant: LanguageVariant,
    diagnostics: &'static [Diagnostic],
    /// The SourceFile node of this store, set by `publish_file_stores`.
    root: Node,
    /// Go `SourceFile.ECMALineMap()`, computed on first use after publish
    /// and shared by every thread.
    ecma_line_starts: OnceLock<Box<[i32]>>,
    /// Set when the file is frozen, by the pass that makes `facts`,
    /// `bind_estimate` and `links` (`make_facts`).
    facts_made: bool,
    /// Made by `make_facts`, valid when `facts_made` is set.
    facts: StoreFacts,
    /// R2-5: the `SlotLinks` of every slot (`BlockFile::links`). Moved
    /// from `build_links` by `make_facts`.
    links: Box<[SlotLinks]>,
    /// R2-5 while the parser runs: the `links` entry of every slot, pushed
    /// (`SlotLinks::NONE`) when the slot is made and written by
    /// `StoreChildLinks` and `replace_store_node_data`.
    build_links: Vec<SlotLinks>,
    /// U1 (e): binder capacity hints, made with `facts`.
    bind_estimate: BindEstimate,
    /// What `make_facts` needs from the slots, kept as they are made and
    /// written (`BuildFacts`).
    build_facts: BuildFacts,
    /// The identifier name cache of the parsing thread (`identifier_name`)
    /// while the parse runs: the store takes it from the thread
    /// (`IDENTIFIER_NAMES`) on its first name, and `end_parse` or the drop
    /// of the store gives it back (`StoreIdents`).
    // PERF: identparse1. A name slot reads the cache with no thread-local
    // access and no `RefCell` borrow.
    idents: StoreIdents,
}

/// storefast1: the parts of `StoreFacts` and `BindCounts` that the slots of
/// an unfinished store give, kept as the slots are made (`kind`) and their
/// flags written (`flags`). Slot kinds never change, so `make_facts` reads
/// no record or kind for them. For the parent and deprecated tag facts it
/// reads the records only when a parent in another store or a flags write
/// with the tag was seen.
// PERF: storefast1. `make_facts` loaded every record and kind of the store
// at the freeze (Go has no such pass): 0.88 G instructions on VS Code.
#[derive(Default)]
struct BuildFacts {
    counts: BindCounts,
    has_alias_slot: bool,
    has_export_alias_kind: bool,
    has_flow_constraint_kind: bool,
    /// A flags write of a slot had `POSSIBLY_CONTAINS_DEPRECATED_TAG`.
    deprecated_tag_written: bool,
}

impl BuildFacts {
    /// A new node slot of kind `kind` (`StoreFacts::add` and
    /// `BindCounts::add` of its kind).
    #[inline]
    fn node(&mut self, kind: SyntaxKind) {
        self.counts.add(kind);
        match kind {
            SyntaxKind::ExportAssignment | SyntaxKind::ExportSpecifier => {
                self.has_export_alias_kind = true;
            }
            SyntaxKind::ConditionalType | SyntaxKind::MappedType => {
                self.has_flow_constraint_kind = true;
            }
            _ => {}
        }
    }

    /// A flags write of `flags` into a slot record.
    #[inline]
    fn flags(&mut self, flags: NodeFlags) {
        self.deprecated_tag_written |=
            flags.intersects(NodeFlags::POSSIBLY_CONTAINS_DEPRECATED_TAG);
    }
}

/// Facts about the slots of one finished store (`FileStore::make_facts`,
/// from `BuildFacts`). They do not depend on the store id, so a detached
/// store gets them when its parse ends.
#[derive(Clone, Copy, Debug, Default)]
pub struct StoreFacts {
    /// Every slot after slot 0 holds a node (the store has no alias slot),
    /// so `record_resolve(file, i) == handle(file, i)` for every `i >= 1`,
    /// and a child walk from a node of this store stays in this store.
    pub alias_free: bool,
    /// The parent of every node slot is nil or a node of this store, so a
    /// parent walk from a node of this store stays in this store.
    pub parents_local: bool,
    /// Some node slot has kind `ExportAssignment` or `ExportSpecifier`.
    pub has_export_alias_kind: bool,
    /// Some node slot has kind `ConditionalType` or `MappedType`.
    pub has_flow_constraint_kind: bool,
    /// U4 (CH7): some node slot has the parser flag
    /// `POSSIBLY_CONTAINS_DEPRECATED_TAG`. The parse of a frozen store is
    /// over, and the bind never adds that bit (`BINDER_ADDED_FLAGS` in
    /// node.rs), so without it no node of the store has the bit in Go
    /// `node.Flags` (`frozen_store_lacks_deprecated_tag`).
    pub has_deprecated_tag: bool,
}

impl StoreFacts {
    /// The facts of a store that has only slot 0. `add` adds the others.
    const ONLY_NIL_SLOT: Self = Self {
        alias_free: true,
        parents_local: true,
        has_export_alias_kind: false,
        has_flow_constraint_kind: false,
        has_deprecated_tag: false,
    };

    /// Adds the slot of `record`, after slot 0 (the pass that
    /// `debug_scan_facts` checks `BuildFacts` against).
    #[cfg(debug_assertions)]
    fn add(&mut self, record: &NodeRecord, kind: SyntaxKind) {
        if !record.is_node() {
            self.alias_free = false;
            return;
        }
        if record.has_foreign_parent() {
            self.parents_local = false;
        }
        if record
            .flags()
            .intersects(NodeFlags::POSSIBLY_CONTAINS_DEPRECATED_TAG)
        {
            self.has_deprecated_tag = true;
        }
        match kind {
            SyntaxKind::ExportAssignment | SyntaxKind::ExportSpecifier => {
                self.has_export_alias_kind = true;
            }
            SyntaxKind::ConditionalType | SyntaxKind::MappedType => {
                self.has_flow_constraint_kind = true;
            }
            _ => {}
        }
    }

    /// The facts as the bits of a `FactsCell`. It names every field, so a
    /// new field does not compile until it has a bit.
    const fn bits(self) -> u8 {
        let Self {
            alias_free,
            parents_local,
            has_export_alias_kind,
            has_flow_constraint_kind,
            has_deprecated_tag,
        } = self;
        (alias_free as u8)
            | (parents_local as u8) << 1
            | (has_export_alias_kind as u8) << 2
            | (has_flow_constraint_kind as u8) << 3
            | (has_deprecated_tag as u8) << 4
    }

    /// The facts of the bits of a `FactsCell`.
    #[inline]
    const fn from_bits(bits: u8) -> Self {
        Self {
            alias_free: bits & 1 != 0,
            parents_local: bits & 1 << 1 != 0,
            has_export_alias_kind: bits & 1 << 2 != 0,
            has_flow_constraint_kind: bits & 1 << 3 != 0,
            has_deprecated_tag: bits & 1 << 4 != 0,
        }
    }
}

/// edleak1: the `StoreFacts` of a `BlockFile` in one atomic byte, so the
/// `BlockFile` of a pooled block takes the facts of each node shell that
/// reuses the block (`PoolBlock::file`). A static publish sets it once.
struct FactsCell(AtomicU8);

impl FactsCell {
    const fn new(facts: StoreFacts) -> Self {
        Self(AtomicU8::new(facts.bits()))
    }

    #[inline]
    fn get(&self) -> StoreFacts {
        StoreFacts::from_bits(self.0.load(Ordering::Relaxed))
    }

    fn set(&self, facts: StoreFacts) {
        self.0.store(facts.bits(), Ordering::Relaxed);
    }
}

/// U4 (CH6, bind A): two child ids of a node slot, so that `Node::name`,
/// `Node::expression`, `Node::postfix_token` and `Node::question_token` of a
/// published store node need no load of its astdata node and data
/// (`NodeKids`, `frozen_store_child`). C2 adds a third field for
/// `Node::type_`, `Node::initializer`, `Node::type_name` and
/// `Node::type_argument_list`. The ids are store-local
/// slot indexes, like the child ids in the node data, so they stay valid
/// when a detached store gets its real id. `node.rs`
/// (`store_node_children`) makes the value from the node data with the arm
/// lists of those accessors.
///
/// - `name`: the id of the Go `Name()` child, 0 for nil or for a kind
///   without the field, or `UNKNOWN_ID`.
/// - `other`: a tag in the top two bits and an id below. Each kind has at
///   most one of these fields: `TAG_EXPRESSION` (Go `Expression()`, also
///   tag 0 with id 0 for a kind with none of the fields), `TAG_POSTFIX` (Go
///   `PostfixToken()`) or `TAG_QUESTION` (the node's own Go
///   `QuestionToken` field). All ones (`UNKNOWN_ID`) is unknown.
/// - `typed` (C2): a tag in the top three bits, the `NO_TYPE_ARGUMENTS` bit
///   and an id below, for Go `Type()`, `Initializer()` and
///   `AsTypeReference().TypeName`. `TYPED_TYPE`: `Type()` is the id and
///   `Initializer()` is nil (also id 0 for a node where both are nil, as for
///   a kind with neither field). `TYPED_INITIALIZER`: `Type()` is nil and
///   `Initializer()` is the id. `TYPED_TYPE_WITH_INITIALIZER`: `Type()` is
///   the id, and `Initializer()` is set too but not held. `TYPED_TYPE_NAME`
///   (TypeReference): `TypeName` is the id, `Type()` and `Initializer()`
///   are nil. `NO_TYPE_ARGUMENTS` is set when Go `TypeArgumentList()` is nil
///   and the node data has no list (`Node::type_argument_list` then gives
///   `NodeList::NIL`).
///
/// Unknown means "read the node data": nil and alias slots, the kinds whose
/// accessor arm is not a plain field read (QualifiedName,
/// CaseOrDefaultClause) and an id that does not fit.
// PERF: 12 bytes of the `NodeKids` of the slot. A read is one kids word
// and the per-store record of `Node::new`, not the chain kind table, node
// pointer, node tag, data box, field.
// PERF: perfP2 item 2. Two words, `name` and `other` in one: a struct of two
// scalars comes back from `store_node_children` in two registers. Three
// `u32` fields came back through memory, three 4-byte stores, and the caller
// read `name` and `other` with one 8-byte load right after the call. That
// load cannot take its value from two stores still in the store buffer, so
// it waited for them: two thirds of the cycles of `alloc_store_slot_node`
// and of the lib snapshot load.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SlotChildren {
    /// `name` in the low half, `other` in the high half.
    name_other: u64,
    typed: u32,
}

impl SlotChildren {
    /// A field value that means "read the node data".
    const UNKNOWN_ID: u32 = u32::MAX;
    const TAG_SHIFT: u32 = 30;
    /// The largest id that `other` can hold.
    const MAX_TAGGED_ID: u32 = (1 << Self::TAG_SHIFT) - 1;
    pub(crate) const TAG_EXPRESSION: u32 = 0;
    pub(crate) const TAG_POSTFIX: u32 = 1;
    pub(crate) const TAG_QUESTION: u32 = 2;
    const TAG_UNKNOWN: u32 = 3;

    const TYPED_TAG_SHIFT: u32 = 29;
    /// C2: Go `TypeArgumentList()` is nil (see the type doc).
    const NO_TYPE_ARGUMENTS: u32 = 1 << 28;
    /// The largest id that `typed` can hold.
    const MAX_TYPED_ID: u32 = Self::NO_TYPE_ARGUMENTS - 1;
    pub(crate) const TYPED_TYPE: u32 = 0;
    pub(crate) const TYPED_INITIALIZER: u32 = 1;
    pub(crate) const TYPED_TYPE_WITH_INITIALIZER: u32 = 2;
    pub(crate) const TYPED_TYPE_NAME: u32 = 3;
    /// Tags 4 to 6 are not used.
    const TYPED_UNKNOWN: u32 = 7;
    /// `typed` when unknown: every read takes the node data. The
    /// `NO_TYPE_ARGUMENTS` bit is clear.
    const TYPED_UNKNOWN_VALUE: u32 = Self::TYPED_UNKNOWN << Self::TYPED_TAG_SHIFT;

    /// The entry of a slot whose data has none of the fields
    /// (`store_node_children`), such as an Identifier: no name,
    /// none of the `other` fields (tag 0, id 0), no type or initializer
    /// (`TYPED_TYPE`, id 0) and no type argument list.
    pub(crate) const NO_FIELDS: Self = Self {
        name_other: 0,
        typed: Self::NO_TYPE_ARGUMENTS,
    };
    /// Every read takes the node data.
    pub(crate) const UNKNOWN: Self = Self {
        name_other: Self::name_other_of(Self::UNKNOWN_ID, Self::UNKNOWN_ID),
        typed: Self::TYPED_UNKNOWN_VALUE,
    };
    /// The `name_other` word of field values `name` and `other`.
    #[inline]
    const fn name_other_of(name: u32, other: u32) -> u64 {
        (name as u64) | ((other as u64) << 32)
    }
    /// The `name` field value.
    #[inline]
    fn name(self) -> u32 {
        self.name_other as u32
    }
    /// The `other` field value.
    #[inline]
    fn other(self) -> u32 {
        (self.name_other >> 32) as u32
    }

    /// `name` is the name child id (0 for nil or no field, `None` for
    /// unknown); `other` is `(tag, id)` (`None` for unknown). The `typed`
    /// field is unknown (see `with_typed`).
    #[inline]
    pub(crate) fn new(name: Option<u32>, other: Option<(u32, u32)>) -> Self {
        let other = match other {
            Some((tag, id)) if tag < Self::TAG_UNKNOWN && id <= Self::MAX_TAGGED_ID => {
                (tag << Self::TAG_SHIFT) | id
            }
            _ => Self::UNKNOWN_ID,
        };
        Self {
            // `UNKNOWN_ID` itself is not a slot id a store can reach, and
            // it reads as unknown, which is always exact.
            name_other: Self::name_other_of(name.unwrap_or(Self::UNKNOWN_ID), other),
            typed: Self::TYPED_UNKNOWN_VALUE,
        }
    }

    /// C2: this entry with `typed` set from `typed` (`(tag, id)`, `None`
    /// for unknown) and `no_type_arguments`.
    #[inline]
    pub(crate) fn with_typed(self, typed: Option<(u32, u32)>, no_type_arguments: bool) -> Self {
        let typed = match typed {
            Some((tag, id)) if tag <= Self::TYPED_TYPE_NAME && id <= Self::MAX_TYPED_ID => {
                (tag << Self::TYPED_TAG_SHIFT) | id
            }
            _ => Self::TYPED_UNKNOWN_VALUE,
        };
        let no_type_arguments = if no_type_arguments {
            Self::NO_TYPE_ARGUMENTS
        } else {
            0
        };
        Self {
            typed: typed | no_type_arguments,
            ..self
        }
    }

    /// The tag and id of an `other` field value.
    #[inline]
    fn other_parts_of(other: u32) -> (u32, u32) {
        (other >> Self::TAG_SHIFT, other & Self::MAX_TAGGED_ID)
    }

    /// C2: the tag and id of a `typed` field value.
    #[inline]
    fn typed_parts_of(typed: u32) -> (u32, u32) {
        (typed >> Self::TYPED_TAG_SHIFT, typed & Self::MAX_TYPED_ID)
    }

    /// C2: a `typed` field value says Go `TypeArgumentList()` is nil.
    #[inline]
    fn has_no_type_arguments_in(typed: u32) -> bool {
        typed & Self::NO_TYPE_ARGUMENTS != 0
    }

    /// The tag and id of `other`.
    #[cfg(test)]
    fn other_parts(self) -> (u32, u32) {
        Self::other_parts_of(self.other())
    }

    /// C2: the tag and id of `typed`.
    #[cfg(test)]
    fn typed_parts(self) -> (u32, u32) {
        Self::typed_parts_of(self.typed)
    }

    /// C2: Go `TypeArgumentList()` is nil.
    #[cfg(test)]
    fn has_no_type_arguments(self) -> bool {
        Self::has_no_type_arguments_in(self.typed)
    }
}

/// AST node records (`ast-design/study.md`): the Go `NodeBase` fields and
/// the hot binder fields of one slot in 24 bytes (`FileStore::records`).
/// `NodeHeader` is the value form of its parse fields. The words are
/// atomics that the reads load with `Relaxed` (a plain load on x86-64 and
/// aarch64). The parse writes them through `get_mut`; the binder writes its
/// fields into the published record (`BindRecords`, `bind_store_records`).
///
/// - The Go `node.Kind` of the slot is not in its record but in the kind
///   column (`FileStore::kinds`, `FileBlock::kinds`).
/// - `flags`: Go `node.Flags` below `RECORD_BITS`: the parser flags, and
///   after the bind also the bits the binder added (`BINDER_ADDED_FLAGS` in
///   node.rs). In `RECORD_BITS` (the top 3 bits, which no Go `NodeFlags`
///   value uses): `SOURCE_FILE_ROOT`, `TEXT_IS_KEYWORD` and `NO_NODE`.
/// - `loc`: Go `node.Loc`, pos in the low half and end in the high half.
/// - `up` of a node slot: the parent code in the low half (`ParentCode`),
///   and the Go symbol (`SymbolId`) in the high half, 0 before the bind.
///   `up` of the nil slot or an alias slot: the whole target `Node`.
/// - `bind` without `BIND_EXTRA`: the low half of the Go
///   `FlowNodeData().FlowNode` (the flow node is in the same file; 0 is
///   nil). With `BIND_EXTRA`: the other bits are the index + 1 of the
///   binder fields of the node in `FileNodeBind` (`NodeBindExtra`), which
///   hold its flow node too. AST node records, step 4: `bind` of the nil
///   slot (slot 0) is the owner, the file id of the block
///   (`block_is_owned`). The publish writes it; the bind never writes the
///   nil slot.
///
/// Only the parse and the bind write a record. The bind writes after the
/// publish, and changes only `flags` (it ORs in the bits it adds, so the
/// record bits stay), the high half of `up` and `bind`; a bind that is not
/// installed sets them back (`BindRecords::reset`). `header` reads each
/// word once and takes only
/// the low half of `up` of a node slot (the bind never writes `up` of the
/// nil slot or an alias slot), so it sees the parse fields and the flags
/// from before or after the bind, never a mix of one word. The bind of a
/// file ends before any reader of its binder fields starts: the checker
/// threads start after the bind, or get their work through a lock or a
/// channel, which orders the writes before their reads.
// PORT: AST node records, step 4: every word is an atomic, so a pooled
// block (`BlockPool`) can take the records of another file through a shared
// borrow, with no `unsafe`. The kind of a slot never changes after the slot
// is made (`replace_store_node_data` keeps it), so it is in a plain column
// of its own (`FileStore::kinds`). edleak1: a node shell whose plain
// column would pass a budget (`plain_kinds`) copies its kinds into an
// atomic column of its pooled block (`BlockFile::shell_kinds`), so a later
// version reuses it.
// PERF: step 4 first had an `AtomicU16` kind in the record, read through a
// 512-entry table (safe Rust has no cheaper `u16` to `SyntaxKind`):
// `goport -p` +1.3% to +2.1% instructions against step 7, and a plain
// `transmute` of the atomic load still +0.6% to +1.1%
// (ast-design/step4b). The column adds 2 bytes per slot.
// PERF: astmem1 P3. 24 bytes, not 32: the record bits are in the top of
// `flags`, and the `bind` word is 32 bits. The extras index of the 60% of
// node slots with no binder data took 4 bytes, and `bits` 4 bytes with its
// padding. A slot with extras (locals containers, exported declarations,
// function-like nodes: about 6% of slots) keeps its flow node in its
// extras entry, so the flow read of every other node is one load, as before.
#[repr(C)]
#[derive(Debug)]
pub struct NodeRecord {
    flags: AtomicU32,
    bind: AtomicU32,
    loc: AtomicU64,
    up: AtomicU64,
}

const _: () = assert!(std::mem::size_of::<NodeRecord>() == 24);

// The bind ORs only `BINDER_ADDED_FLAGS` into a record's `flags`
// (`bind_store_records` checks it), so it keeps the record bits.
const _: () = assert!(super::node::BINDER_ADDED_FLAGS.0 & NodeRecord::RECORD_BITS == 0);

/// The bit of a `NodeRecord::bind` word that holds an extras index
/// (`NodeBindExtra`), not a flow node.
pub const BIND_EXTRA: u32 = 1 << 31;

// The owner word (`NodeRecord::owner_word`) is a file id.
const _: () = assert!(FILE_ID_LIMIT <= 1 << 31);

/// AST node records, step 2: the low half of `NodeRecord::up` of a node
/// slot. 0 is a nil parent. `1..FOREIGN_PARENT` is a parent in the same
/// store: its slot index + 1, so the handle is `(file << 32) | code` and
/// needs no rewrite when the store id changes (`adopt_detached_store`).
/// `FOREIGN_PARENT | i` is a parent in another store (or a synthetic
/// parent): entry `i` of the foreign parents of the store
/// (`FileStore::foreign_parents`, `BlockFile::foreign`).
type ParentCode = u32;

/// The `ParentCode` bit of a parent in another store.
const FOREIGN_PARENT: ParentCode = 1 << 31;

impl NodeRecord {
    /// Go `GetSourceFileOfNode(node)` is the root of the store
    /// (`mark_source_file_roots`).
    const SOURCE_FILE_ROOT: u32 = 1 << 29;
    /// U1 (a): Go `scanner.GetIdentifierToken(node.Text()) !=
    /// KindIdentifier` of an Identifier or PrivateIdentifier slot.
    const TEXT_IS_KEYWORD: u32 = 1 << 30;
    /// The slot holds no node: the nil slot or an alias slot. Its target is
    /// in `up`.
    const NO_NODE: u32 = 1 << 31;
    /// The record bits in the `flags` word. Go `NodeFlags` ends at bit 28
    /// (`REPARSER_TRANSFORMED_LITERAL`); a flags write checks it: the parse
    /// writes in `checked_flags`, the bind writes in `bind_store_records`
    /// (`BINDER_ADDED_FLAGS`, which has no record bit).
    const RECORD_BITS: u32 = Self::SOURCE_FILE_ROOT | Self::TEXT_IS_KEYWORD | Self::NO_NODE;

    /// The record of a new node slot: Go `newNode` (undefined loc, nil
    /// parent, no flags).
    #[inline]
    fn node(text_is_keyword: bool) -> Self {
        Self::new(
            if text_is_keyword {
                Self::TEXT_IS_KEYWORD
            } else {
                0
            },
            NodeFlags::NONE,
            TextRange::undefined(),
            0,
        )
    }

    /// The nil slot (`target` nil) or an alias slot.
    #[inline]
    fn target(target: Node) -> Self {
        Self::new(
            Self::NO_NODE,
            NodeFlags::NONE,
            TextRange::undefined(),
            target.0,
        )
    }

    /// A slot with word `up` (see `NodeRecord`) and no binder fields.
    #[inline]
    fn new(bits: u32, flags: NodeFlags, loc: TextRange, up: u64) -> Self {
        debug_assert_eq!(bits & !Self::RECORD_BITS, 0);
        Self {
            flags: AtomicU32::new(Self::checked_flags(flags) | bits),
            bind: AtomicU32::new(0),
            loc: AtomicU64::new(Self::loc_word(loc)),
            up: AtomicU64::new(up),
        }
    }

    /// `flags.0`, which must not use `RECORD_BITS`.
    #[inline]
    fn checked_flags(flags: NodeFlags) -> u32 {
        assert_eq!(
            flags.0 & Self::RECORD_BITS,
            0,
            "Go NodeFlags {:#x} in the node record bits",
            flags.0
        );
        flags.0
    }

    #[inline]
    fn loc_word(loc: TextRange) -> u64 {
        u64::from(loc.pos() as u32) | (u64::from(loc.end() as u32) << 32)
    }

    /// A record of a fresh pool block (`BlockPool`): all words 0.
    fn zero() -> Self {
        Self {
            flags: AtomicU32::new(0),
            bind: AtomicU32::new(0),
            loc: AtomicU64::new(0),
            up: AtomicU64::new(0),
        }
    }

    /// The record bits (`RECORD_BITS`).
    #[inline]
    fn bits(&self) -> u32 {
        self.flags.load(Ordering::Relaxed) & Self::RECORD_BITS
    }

    #[inline]
    fn has_bit(&self, bit: u32) -> bool {
        self.bits() & bit != 0
    }

    /// True for a node slot, false for the nil slot and alias slots.
    #[inline]
    fn is_node(&self) -> bool {
        !self.has_bit(Self::NO_NODE)
    }

    #[inline]
    fn flags(&self) -> NodeFlags {
        NodeFlags(self.flags.load(Ordering::Relaxed) & !Self::RECORD_BITS)
    }

    #[inline]
    fn loc(&self) -> TextRange {
        let word = self.loc.load(Ordering::Relaxed);
        TextRange::new(word as u32 as i32, (word >> 32) as u32 as i32)
    }

    /// The target of the nil slot or an alias slot (`NO_NODE`).
    #[inline]
    fn target_node(&self) -> Node {
        Node(self.up.load(Ordering::Relaxed))
    }

    /// The `ParentCode` of a node slot.
    #[inline]
    fn parent_code(&self) -> ParentCode {
        self.up.load(Ordering::Relaxed) as u32
    }

    /// The slot index of the parent of a node slot when it is a node of the
    /// same store, else `None` (nil or another store).
    #[inline]
    fn local_parent(&self) -> Option<usize> {
        let code = self.parent_code();
        (code != 0 && code & FOREIGN_PARENT == 0).then(|| code as usize - 1)
    }

    /// True when the parent of a node slot is in another store.
    #[inline]
    fn has_foreign_parent(&self) -> bool {
        self.parent_code() & FOREIGN_PARENT != 0
    }

    /// The header of this slot of store `file`, whose kind is `kind` and
    /// whose foreign parents are `foreign`, as the node reads see it. The
    /// parent of the nil slot or an alias slot is its target.
    #[inline]
    fn header(&self, kind: SyntaxKind, file: usize, foreign: &[Node]) -> NodeHeader {
        let word = self.flags.load(Ordering::Relaxed);
        let bits = word & Self::RECORD_BITS;
        let up = self.up.load(Ordering::Relaxed);
        let code = up as ParentCode;
        NodeHeader {
            parent: if bits & Self::NO_NODE != 0 {
                Node(up)
            } else if code & FOREIGN_PARENT != 0 {
                // No call and no panic, so a reader that does not use the
                // parent loses this code after inlining.
                let parent = foreign.get((code & !FOREIGN_PARENT) as usize).copied();
                debug_assert!(parent.is_some(), "foreign parent {code:#x}");
                parent.unwrap_or_default()
            } else {
                local_parent_of_code(code, file)
            },
            loc: self.loc(),
            flags: NodeFlags(word & !Self::RECORD_BITS),
            kind,
            source_file_is_root: bits & Self::SOURCE_FILE_ROOT != 0,
            text_is_keyword: bits & Self::TEXT_IS_KEYWORD != 0,
        }
    }

    /// AST node records, step 2: Go `node.Symbol()` of a node slot (nil
    /// before the bind).
    #[inline]
    fn symbol(&self) -> SymbolId {
        SymbolId((self.up.load(Ordering::Relaxed) >> 32) as u32)
    }

    /// AST node records, step 2: the `bind` word (see `NodeRecord`).
    #[inline]
    fn bind_word(&self) -> u32 {
        self.bind.load(Ordering::Relaxed)
    }

    /// AST node records, step 4: writes every word of `from` into this
    /// record of a pooled block (`node_shell`), with `Relaxed` stores. The
    /// publish of the block (`set_file_block`) orders them before the reads
    /// of the new owner.
    #[inline]
    fn store_from(&self, from: &NodeRecord) {
        let relaxed = Ordering::Relaxed;
        self.flags.store(from.flags.load(relaxed), relaxed);
        self.bind.store(from.bind.load(relaxed), relaxed);
        self.loc.store(from.loc.load(relaxed), relaxed);
        self.up.store(from.up.load(relaxed), relaxed);
    }

    /// AST node records, step 4: the owner word of the nil slot of the
    /// block of file `file` (see `NodeRecord`, `bind`).
    // PERF: ownercheck1. The file id itself, not id + 1: the owner check
    // (`check_block_owner`) compares it with the file id of the handle, which
    // the read has in a register already, with no add. A fresh pool block
    // is all zero, but no read reaches it before `node_shell` writes its
    // owner.
    #[inline]
    fn owner_word(file: usize) -> u32 {
        // `FILE_ID_LIMIT` fits (see the assert after `BIND_EXTRA`).
        file as u32
    }

    /// AST node records, step 4: true when this nil slot record is the one
    /// of the block of file `file` (`block_is_owned`).
    #[inline]
    fn is_owned_by(&self, file: usize) -> bool {
        self.bind_word() == Self::owner_word(file)
    }

    /// Writes the parent code of a node slot.
    #[inline]
    fn set_parent_code(&mut self, code: ParentCode) {
        let up = self.up.get_mut();
        *up = (*up & !0xffff_ffff) | u64::from(code);
    }

    #[inline]
    fn set_loc(&mut self, loc: TextRange) {
        *self.loc.get_mut() = Self::loc_word(loc);
    }

    /// Writes the Go flags and keeps the record bits.
    #[inline]
    fn set_flags(&mut self, flags: NodeFlags) {
        let word = self.flags.get_mut();
        *word = (*word & Self::RECORD_BITS) | Self::checked_flags(flags);
    }

    /// `set_bit(SOURCE_FILE_ROOT, on)` through a shared borrow:
    /// `mark_roots` reads other records of the store while it writes.
    #[inline]
    fn set_root_bit(&self, on: bool) {
        let word = self.flags.load(Ordering::Relaxed);
        let word = if on {
            word | Self::SOURCE_FILE_ROOT
        } else {
            word & !Self::SOURCE_FILE_ROOT
        };
        self.flags.store(word, Ordering::Relaxed);
    }

    #[inline]
    fn set_bit(&mut self, bit: u32, on: bool) {
        debug_assert_eq!(bit & !Self::RECORD_BITS, 0);
        let word = self.flags.get_mut();
        if on {
            *word |= bit;
        } else {
            *word &= !bit;
        }
    }

    /// AST node records, step 2: writes the binder fields of a node slot of
    /// a published store (`bind_store_records`): the symbol into `up`, the
    /// added flags into `flags` (the record bits stay), and the `bind` word.
    /// The bind is the only writer of a published record, and
    /// `bind_store_records` writes each record once. `added` has no record
    /// bit: `bind_store_records`
    /// checks each data entry against `BINDER_ADDED_FLAGS` in release
    /// builds too, and those flags have none (const assert after
    /// `NodeRecord`).
    #[inline]
    fn write_bind(&self, symbol: SymbolId, added: NodeFlags, bind: u32) {
        debug_assert_eq!(added.0 & Self::RECORD_BITS, 0);
        if symbol.is_some() {
            let up = self.up.load(Ordering::Relaxed);
            self.up.store(
                (up & 0xffff_ffff) | (u64::from(symbol.0) << 32),
                Ordering::Relaxed,
            );
        }
        if !added.is_empty() {
            let flags = self.flags.load(Ordering::Relaxed);
            self.flags.store(flags | added.0, Ordering::Relaxed);
        }
        if bind != 0 {
            self.bind.store(bind, Ordering::Relaxed);
        }
    }
}

/// The `ParentCode` of `stored`, nil or a `LOCAL_STORE` handle
/// (`NodeHeader::stored_parent`).
#[inline]
fn local_parent_code(stored: Node) -> ParentCode {
    debug_assert!(stored.is_nil() || stored.file_index() == LOCAL_STORE);
    let code = stored.0 as u32;
    assert!(code < FOREIGN_PARENT, "too many slots in a store");
    code
}

/// Go `node.Parent` of a node slot of store `file` whose parent code
/// `code` is nil or a slot of the store (not `FOREIGN_PARENT`).
#[inline]
fn local_parent_of_code(code: ParentCode, file: usize) -> Node {
    debug_assert_eq!(code & FOREIGN_PARENT, 0);
    if code == 0 {
        Node::NIL
    } else {
        Node(((file as u64) << 32) | u64::from(code))
    }
}

/// AST node records, step 1: the child ids and the name or modifier word of
/// one slot in 16 bytes (`FileStore::kids`), next to its `NodeRecord`.
/// `name`, `other` and `typed` are the fields of `SlotChildren` (U4, C2).
/// `word` is the interned Go `node.Text()` (`Name::id`) of an Identifier or
/// PrivateIdentifier slot (U1 (a) (d)), and the U1 (b) modifier bits
/// (`ModifierList::modifier_flags` of the node's own list) of any other
/// slot. The parse writes them through `get_mut`.
#[derive(Debug)]
pub struct NodeKids {
    name: AtomicU32,
    other: AtomicU32,
    typed: AtomicU32,
    word: AtomicU32,
}

const _: () = assert!(std::mem::size_of::<NodeKids>() == 16);

impl NodeKids {
    /// Every child read takes the node data; no name, no modifier bits. The
    /// nil slot and alias slots.
    #[inline]
    fn unknown() -> Self {
        Self::new(SlotChildren::UNKNOWN, 0)
    }

    #[inline]
    fn new(children: SlotChildren, word: u32) -> Self {
        Self {
            name: AtomicU32::new(children.name()),
            other: AtomicU32::new(children.other()),
            typed: AtomicU32::new(children.typed),
            word: AtomicU32::new(word),
        }
    }

    /// The word of a slot of kind `kind` with name `name` (U1 (a)) and
    /// modifier bits `modifier_bits` (U1 (b)).
    #[inline]
    fn word_of(kind: SyntaxKind, name: &Name, modifier_bits: u32) -> u32 {
        if is_name_kind(kind) {
            debug_assert_eq!(modifier_bits, 0, "U1 (b): modifier bits on a name slot");
            name.id()
        } else {
            debug_assert_eq!(*name, Name::default(), "U1 (a): a name on a non-name slot");
            modifier_bits
        }
    }

    #[inline]
    fn children(&self) -> SlotChildren {
        SlotChildren {
            name_other: SlotChildren::name_other_of(
                self.name.load(Ordering::Relaxed),
                self.other.load(Ordering::Relaxed),
            ),
            typed: self.typed.load(Ordering::Relaxed),
        }
    }

    #[inline]
    fn name_id(&self) -> u32 {
        self.name.load(Ordering::Relaxed)
    }

    #[inline]
    fn other(&self) -> u32 {
        self.other.load(Ordering::Relaxed)
    }

    #[inline]
    fn typed(&self) -> u32 {
        self.typed.load(Ordering::Relaxed)
    }

    #[inline]
    fn word(&self) -> u32 {
        self.word.load(Ordering::Relaxed)
    }

    /// U1 (a): the name of a slot of name kind `kind`; `Name::default()`
    /// for any other kind.
    #[inline]
    fn text_name(&self, kind: SyntaxKind) -> Name {
        if is_name_kind(kind) {
            Name::from_id(self.word())
        } else {
            Name::default()
        }
    }

    /// U1 (b): the modifier bits of a slot of kind `kind`; 0 for a name
    /// kind.
    #[inline]
    fn modifier_bits(&self, kind: SyntaxKind) -> u32 {
        if is_name_kind(kind) { 0 } else { self.word() }
    }

    #[inline]
    fn set_children(&mut self, children: SlotChildren) {
        *self.name.get_mut() = children.name();
        *self.other.get_mut() = children.other();
        *self.typed.get_mut() = children.typed;
    }

    #[inline]
    fn set_word(&mut self, word: u32) {
        *self.word.get_mut() = word;
    }

    /// AST node records, step 4: writes every word of `from` into this
    /// entry of a pooled block (`NodeRecord::store_from`).
    #[inline]
    fn store_from(&self, from: &NodeKids) {
        let relaxed = Ordering::Relaxed;
        self.name.store(from.name.load(relaxed), relaxed);
        self.other.store(from.other.load(relaxed), relaxed);
        self.typed.store(from.typed.load(relaxed), relaxed);
        self.word.store(from.word.load(relaxed), relaxed);
    }
}

/// Identifier or PrivateIdentifier: a kind whose slot has a U1 (a) name.
#[inline]
fn is_name_kind(kind: SyntaxKind) -> bool {
    matches!(kind, SyntaxKind::Identifier | SyntaxKind::PrivateIdentifier)
}

/// Which `SlotChildren` field `frozen_store_child` reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StoreChild {
    /// Go `node.Name()`.
    Name,
    /// Go `node.Expression()`.
    Expression,
    /// Go `node.PostfixToken()`.
    PostfixToken,
    /// Go `node.QuestionToken()`: the own field, else the postfix token when
    /// it is a `?` token.
    QuestionToken,
    /// C2: Go `node.Type()`.
    Type,
    /// C2: Go `node.Initializer()`.
    Initializer,
    /// C2: Go `node.AsTypeReference().TypeName`. `None` (read the data, which
    /// panics like Go) for other kinds.
    TypeName,
}

/// R2-5: the binder child links of one slot (`FileStore::links`): the
/// children of a node in Go `ForEachChild` order as a chain, first child
/// and next sibling, so `Binder::bind_each_child` walks them without the
/// node data. The ids are store-local slot indexes, so they stay valid when
/// a detached store gets its real id.
///
/// - `first_child`: the first child of this node, `LINK_END` for a node
///   with no child, `LINK_NONE` when the chain of this node is not known.
/// - `next_sibling`: the next child in the chain that holds this slot,
///   `LINK_END` for the last one, `LINK_NONE` when no chain holds it.
///
/// Rules that keep a known chain equal to Go `ForEachChild` of its node:
/// - The parser links a node when it sets the parents of its children
///   (`StoreChildLinks`), in the same visit. A chain holds only node slots
///   of the same store.
/// - A slot is in at most one chain. A child that another chain holds
///   (a node that two parents share, or a child twice in one node), a
///   child of another store and an alias child make the node unknown.
/// - A new data write to a node (`replace_store_node_data`) frees its chain
///   and makes it unknown until the parser links it again.
/// - A store whose parse did not finish gets no links (`publish`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SlotLinks {
    first_child: u32,
    next_sibling: u32,
}

/// R2-5: a `SlotLinks` field value: the chain ends (slot 0, the nil slot,
/// is never a child in a chain).
const LINK_END: u32 = NIL_SLOT;
/// R2-5: a `SlotLinks` field value: not known (`first_child`) or in no
/// chain (`next_sibling`).
const LINK_NONE: u32 = u32::MAX;

impl SlotLinks {
    /// A slot with no known chain that no chain holds.
    const NONE: Self = Self {
        first_child: LINK_NONE,
        next_sibling: LINK_NONE,
    };
}

/// U1 (e): capacity hints for the binder of a store file, counted from the
/// slot kinds when the store is frozen (`frozen_store_bind_estimate`).
/// Only capacities: the binder output does not depend on them.
// bindonce2: the estimate of the `NodeBindBuilder` entries went with it.
#[derive(Clone, Copy, Debug, Default)]
struct BindEstimate {
    /// `Binder::flow_nodes`: about the flow nodes the binder makes.
    flow_nodes: u32,
}

/// The slot kind counts of a `BindEstimate`, one `add` per slot kind.
// PERF: U1 (e). Each new node slot adds its kind (`BuildFacts::node`),
// so the estimate needs no second pass over the kinds.
#[derive(Default)]
struct BindCounts {
    flow_nodes: usize,
}

impl BindCounts {
    #[inline]
    fn add(&mut self, kind: SyntaxKind) {
        // The nil slot, alias slots and identifiers make no flow node
        // (`flow_node_weight` is 0 for them).
        self.flow_nodes += flow_node_weight(kind);
    }

    fn estimate(&self) -> BindEstimate {
        // The unreachable flow node and the source file start node.
        let flow_nodes = self.flow_nodes + 2;
        BindEstimate {
            flow_nodes: u32::try_from(flow_nodes).unwrap_or(u32::MAX),
        }
    }
}

/// About how many flow nodes the binder makes for a node of kind `kind`
/// (binder.go `bindContainer` and the `bind*Flow` functions).
// PERF: storefast1. A table read per new slot (`BuildFacts::node`), not a
// jump table.
#[inline]
fn flow_node_weight(kind: SyntaxKind) -> usize {
    let weight = FLOW_NODE_WEIGHTS[kind as usize];
    debug_assert_eq!(weight, flow_node_weight_of(kind), "{kind:?}");
    usize::from(weight)
}

/// `flow_node_weight` of every kind, by `kind as usize`.
static FLOW_NODE_WEIGHTS: [u8; SyntaxKind::COUNT] = {
    let mut table = [0u8; SyntaxKind::COUNT];
    let mut i = 0;
    while i < FLOW_NODE_WEIGHT_KINDS.len() {
        let kind = FLOW_NODE_WEIGHT_KINDS[i];
        table[kind as usize] = flow_node_weight_of(kind);
        i += 1;
    }
    table
};

/// The kinds whose `flow_node_weight_of` is not 0.
const FLOW_NODE_WEIGHT_KINDS: [SyntaxKind; 32] = [
    SyntaxKind::MethodSignature,
    SyntaxKind::CallSignature,
    SyntaxKind::ConstructSignature,
    SyntaxKind::FunctionType,
    SyntaxKind::ConstructorType,
    SyntaxKind::FunctionDeclaration,
    SyntaxKind::MethodDeclaration,
    SyntaxKind::GetAccessor,
    SyntaxKind::SetAccessor,
    SyntaxKind::FunctionExpression,
    SyntaxKind::ArrowFunction,
    SyntaxKind::ModuleBlock,
    SyntaxKind::CallExpression,
    SyntaxKind::VariableDeclaration,
    SyntaxKind::BindingElement,
    SyntaxKind::BinaryExpression,
    SyntaxKind::PostfixUnaryExpression,
    SyntaxKind::DeleteExpression,
    SyntaxKind::Constructor,
    SyntaxKind::ClassStaticBlockDeclaration,
    SyntaxKind::CaseClause,
    SyntaxKind::DefaultClause,
    SyntaxKind::LabeledStatement,
    SyntaxKind::IfStatement,
    SyntaxKind::ConditionalExpression,
    SyntaxKind::WhileStatement,
    SyntaxKind::DoStatement,
    SyntaxKind::ForStatement,
    SyntaxKind::ForInStatement,
    SyntaxKind::ForOfStatement,
    SyntaxKind::SwitchStatement,
    SyntaxKind::TryStatement,
];

/// `flow_node_weight` as a match (the table source).
const fn flow_node_weight_of(kind: SyntaxKind) -> u8 {
    match kind {
        // A start node for a control flow container, a flow call, a flow
        // assignment, or a logical or assignment operator.
        SyntaxKind::MethodSignature
        | SyntaxKind::CallSignature
        | SyntaxKind::ConstructSignature
        | SyntaxKind::FunctionType
        | SyntaxKind::ConstructorType
        | SyntaxKind::FunctionDeclaration
        | SyntaxKind::MethodDeclaration
        | SyntaxKind::GetAccessor
        | SyntaxKind::SetAccessor
        | SyntaxKind::FunctionExpression
        | SyntaxKind::ArrowFunction
        | SyntaxKind::ModuleBlock
        | SyntaxKind::CallExpression
        | SyntaxKind::VariableDeclaration
        | SyntaxKind::BindingElement
        | SyntaxKind::BinaryExpression
        | SyntaxKind::PostfixUnaryExpression
        | SyntaxKind::DeleteExpression => 1,
        // A start node and a return label, or a clause and its label.
        SyntaxKind::Constructor
        | SyntaxKind::ClassStaticBlockDeclaration
        | SyntaxKind::CaseClause
        | SyntaxKind::DefaultClause
        | SyntaxKind::LabeledStatement => 2,
        // Branch or loop labels and the true and false conditions.
        SyntaxKind::IfStatement
        | SyntaxKind::ConditionalExpression
        | SyntaxKind::WhileStatement
        | SyntaxKind::DoStatement
        | SyntaxKind::ForStatement
        | SyntaxKind::ForInStatement
        | SyntaxKind::ForOfStatement
        | SyntaxKind::SwitchStatement
        | SyntaxKind::TryStatement => 5,
        _ => 0,
    }
}

/// A store that is not published yet. It lives in a leaked cell of the
/// thread that made it, so `ACTIVE` can keep a plain reference to it.
type StoreCell = &'static RefCell<FileStore>;

/// The build stores of one thread. `stores[i]` has file id `base + i`.
#[derive(Default)]
struct BuildStores {
    /// `PUBLISHED` when the first of `stores` was made.
    base: usize,
    stores: Vec<StoreCell>,
}

impl BuildStores {
    /// The file id of the next store of this thread. Only a publish changes
    /// `PUBLISHED`, so the ids of one build are consecutive.
    fn next_id(&mut self) -> usize {
        // A test build waits while another test thread builds stores.
        #[cfg(test)]
        if self.stores.is_empty() {
            crate::test_lock::begin_build();
        }
        let published = PUBLISHED.load(Ordering::Acquire);
        if self.stores.is_empty() {
            self.base = published;
        } else {
            assert_eq!(
                self.base, published,
                "another thread published node stores while this thread built stores"
            );
        }
        let id = self.base + self.stores.len();
        assert!(id < file_id_cap(), "too many file ids");
        id
    }
}

thread_local! {
    /// The stores of this thread, while the parser runs.
    static BUILD: RefCell<BuildStores> = const {
        RefCell::new(BuildStores {
            base: 0,
            stores: Vec::new(),
        })
    };
    /// The detached store of a parse worker and its provisional id.
    static DETACHED: Cell<Option<(usize, StoreCell)>> = const { Cell::new(None) };
    /// The emptied cell of the last detached store of this thread. The next
    /// detached store reuses it.
    static SPARE_CELL: Cell<Option<StoreCell>> = const { Cell::new(None) };
    /// The cells of the build stores that `publish_file_stores` emptied on
    /// this thread. `new_file_store` and `adopt_detached_store` reuse them
    /// (`build_store_cell`), so an edit parse in a language server does not
    /// leak a new cell in the AST arena.
    static SPARE_BUILD_CELLS: RefCell<Vec<StoreCell>> = const { RefCell::new(Vec::new()) };
    /// The store this thread made or adopted last, and its id: during a
    /// parse, the store of the file the parser reads and writes. It is also
    /// in `BUILD` or `DETACHED`, so clearing it is always safe.
    // PERF: query Q8. Node reads have no parser or factory to ask, so the
    // store of the parsed file is found here. The type has no destructor,
    // so a read is one thread-local load and an id compare: no registry
    // check, no detached check and no `RefCell` borrow of `BUILD`.
    // `publish_file_stores` and `take_detached_file_store` clear it before
    // they empty its cell, and a thread publishes only its own build
    // stores, so an active store is never published. (One loading thread
    // builds at a time: `BuildStores::next_id` and the publish check it.)
    static ACTIVE: Cell<Option<(usize, StoreCell)>> = const { Cell::new(None) };
}

/// The cell of store `file` when it is the active store of this thread.
#[inline]
fn active_store(file: usize) -> Option<StoreCell> {
    match ACTIVE.get() {
        Some((id, store)) if id == file => Some(store),
        _ => None,
    }
}

/// The unpublished store `file` of this thread (active, detached or
/// built), or `None` when this thread has no such store.
#[inline]
fn build_store(file: usize) -> Option<StoreCell> {
    match active_store(file) {
        Some(store) => Some(store),
        None => inactive_build_store(file),
    }
}

/// `build_store` without the `ACTIVE` check.
#[inline(never)]
fn inactive_build_store(file: usize) -> Option<StoreCell> {
    if is_detached_id(file) {
        return DETACHED
            .get()
            .and_then(|(id, store)| (id == file).then_some(store));
    }
    BUILD.with(|b| {
        let b = b.borrow();
        b.stores.get(file.wrapping_sub(b.base)).copied()
    })
}

/// File index that marks a parent in the same store inside a stored
/// header. Reads give the handle of the store (`NodeHeader::read`), so a
/// store keeps its records when its id changes (`adopt_detached_store`).
const LOCAL_STORE: usize = 0x7fff_ffff;

/// First provisional store id. Real file ids stay below `FILE_ID_LIMIT`;
/// synthetic node and flow ids are above every provisional id.
pub const DETACHED_STORE_BASE: usize = 0x8000_0000;
/// Number of provisional ids.
pub const DETACHED_STORE_LIMIT: usize = 0x4000_0000;

/// Every real file id is below this (`file_block`). Ids are never used
/// again: each new file version takes one, and so does each file of a
/// whole parse again (a parse option change, a reopen, a `tsc -w` tsconfig
/// edit).
// fileid1: 2^22 ids took 116 hours at 10 language server edits per second
// (one id per edit). At 2^28 that is 7,456 hours, and the 64-byte entry of
// each id from `LOW_BLOCKS` on (16 GiB at the cap) plus the other memory
// of each edit runs out first.
pub(crate) const FILE_ID_LIMIT: usize = 1 << 28;
/// AST node records, step 3: a file id below this has its entry in
/// `FILE_BLOCKS`; a higher one in a chunk of `HIGH_BLOCKS`.
#[cfg(not(target_family = "wasm"))]
const LOW_BLOCKS: usize = 1 << 16;
/// Entries per `HIGH_BLOCKS` chunk: 1 MiB of entries per chunk, and 16,380
/// chunk cells (256 KiB of `.data`) up to `FILE_ID_LIMIT`.
#[cfg(not(target_family = "wasm"))]
const HIGH_CHUNK: usize = 1 << 14;
/// wasm: linear memory holds all of `FILE_BLOCKS` from the start, set or
/// not, so it has 8,192 entries (512 KiB, where 65,536 took 4 MiB). A large
/// program can have more files (VS Code has 11,014), and file ids are never
/// used again, so one big program, or `tsc -b` with more programs, goes
/// past it. Each id from 8,192 on then has its entry in a `HIGH_BLOCKS`
/// chunk (512 KiB, made on demand) and its reads take the slower path; the
/// results are the same. Chunks of the same size: 32,767 chunk cells
/// (256 KiB).
#[cfg(target_family = "wasm")]
const LOW_BLOCKS: usize = 1 << 13;
#[cfg(target_family = "wasm")]
const HIGH_CHUNK: usize = 1 << 13;

// A real file id is never `LOCAL_STORE`, and the chunks end at the limit.
const _: () = assert!(FILE_ID_LIMIT <= LOCAL_STORE);
const _: () = assert!((FILE_ID_LIMIT - LOW_BLOCKS) % HIGH_CHUNK == 0);

/// The end of the new file ids (`BuildStores::next_id`,
/// `publish_file_stores`).
#[cfg(not(test))]
#[inline]
fn file_id_cap() -> usize {
    FILE_ID_LIMIT
}

#[cfg(test)]
thread_local! {
    /// The `file_id_cap` of this test thread, so a test can reach it.
    static TEST_FILE_ID_CAP: std::cell::Cell<usize> = const { std::cell::Cell::new(FILE_ID_LIMIT) };
}

/// `file_id_cap` in a test: the lower cap of the thread
/// (`TEST_FILE_ID_CAP`).
#[cfg(test)]
fn file_id_cap() -> usize {
    TEST_FILE_ID_CAP.get().min(FILE_ID_LIMIT)
}

#[inline]
fn is_detached_id(file: usize) -> bool {
    (DETACHED_STORE_BASE..DETACHED_STORE_BASE + DETACHED_STORE_LIMIT).contains(&file)
}

/// True for an id that never has a store: a synthetic node or flow id, or
/// any other id at or above `FILE_ID_LIMIT` that is not a provisional id.
#[inline]
fn is_storeless_id(file: usize) -> bool {
    file >= FILE_ID_LIMIT && !is_detached_id(file)
}

/// AST node records, step 3: the registry entry of one published file id
/// (`file_block`), set once by its publish. It holds the per-slot columns
/// of the file, so a hot node read is two dependent loads (the entry,
/// then the slot) for every published file: the first program, a later
/// one (`tsc -b`, watch, an edited file) and the node shell of a freeable
/// file version (`node_shell`). The rest of the file is in `BlockFile`.
// PERF: study B (`ast-design/study.md`). It replaces the tiers of the
// publishes (steps 1 and 2): one table of the first publish (tier 0) and
// one per later publish in a two-level table (tier 1). A tier 0 read loaded
// the table of the publish, then the slice of the file, then the slot; a
// tier 1 read two loads more. 56 bytes, so an entry and its `OnceLock`
// state fill one cache line (`BlockEntry`). Step 4 added `kinds` and moved
// the node column into `BlockFile`: a 72-byte entry took two cache lines
// and 4 MiB more `.data`, and cost 0.1% fewer `goport -p` instructions.
struct FileBlock {
    /// AST node records, step 4: `FileStore::kinds`. A `Node::kind` read
    /// reads only this (`FileBlock::kind`). In a node shell whose kinds are
    /// in its pooled block (`plain_kinds`, `BlockFile::shell_kinds`), only
    /// the slots before its first new kind: a prefix of an earlier column of
    /// its file, maybe empty.
    kinds: &'static [SyntaxKind],
    /// `FileStore::records`. The header reads (`Node::kind`, `parent`,
    /// `loc`, `flags`) and the binder fields in the records read only
    /// this.
    records: &'static [NodeRecord],
    /// `FileStore::kids`.
    kids: &'static [NodeKids],
    file: &'static BlockFile,
}

/// The rest of a `FileBlock`: per-file facts and tables, and the store and
/// `GoFile` of a static publish.
struct BlockFile {
    /// `FileStore::nodes`. Empty in the node shell of a store that owns its
    /// astdata nodes (`FileBlock::node_column`). Here and not in the
    /// `FileBlock` since step 4 (see there): a node data read loads one
    /// word more.
    nodes: &'static [Option<ColumnData>],
    /// astshrink1: `FileStore::lists`, the list table of the node column's
    /// data. Empty in the node shell of a store that owns its astdata nodes.
    lists: &'static [&'static StaticList],
    /// `FileStore::facts`. The child reads load it next to the kids
    /// (`block_resolve_slot`). Atomic for the `BlockFile` of a pooled
    /// block, which each node shell that takes the block sets (edleak1).
    facts: FactsCell,
    /// `FileStore::root` (`frozen_source_file_of_node`), as `facts`.
    root: AtomicU64,
    /// AST node records, step 2: `FileStore::foreign_parents`
    /// (`ParentCode`).
    foreign: &'static [Node],
    /// R2-5: `FileStore::links`. Empty in a node shell: the store of the
    /// version keeps it (`version_child_links`).
    links: &'static [SlotLinks],
    /// edleak1: the kinds of a node shell that has no plain column
    /// (`plain_kinds`), in its pooled block (`PoolBlock::kinds`), so a
    /// later version reuses them with the block. Empty in a static
    /// publish, whose plain column `FileBlock::kinds` has them.
    shell_kinds: &'static [AtomicU16],
    /// The store of a static publish. `None` in a node shell: the freeable
    /// file version owns the store (`with_published_store`).
    store: Option<&'static FileStore>,
    /// The `GoFile` of a static publish, in place, so a `GoFile` read is
    /// one load after the entry (`static_go_file`). `None` in a node shell:
    /// the freeable file version owns it.
    go_file: Option<GoFile>,
    /// Go `CompositeBase.facts` of the nodes of a static publish: one word
    /// per slot (`static_facts_word`), made at the first facts read of the
    /// file. Never made in a node shell: the nodes of a freeable file
    /// version keep their facts in the thread-local map of
    /// `Node::subtree_facts`, which forgets them with the version.
    // PERF: factscol1b. Go keeps the facts in an atomic field of the node
    // (`ast.go:1602`), shared by all threads. The thread-local map cost a
    // `LocalKey::with`, a `RefCell` borrow and a hash lookup per read.
    subtree_facts: OnceLock<Box<[AtomicU32]>>,
}

impl FileBlock {
    /// The node column, or `None` in the node shell of a freeable file
    /// version whose store owns its astdata nodes (lsshells M3c): a node
    /// data read of that version is a scoped read. Every store has the nil
    /// slot, so every other column has an entry.
    #[inline]
    fn node_column(&self) -> Option<&'static [Option<ColumnData>]> {
        let nodes = self.file.nodes;
        (!nodes.is_empty()).then_some(nodes)
    }

    /// Go `node.Kind` of slot `index`: from the plain column, or from the
    /// pooled block of a node shell past its plain prefix (`plain_kinds`,
    /// `BlockFile::shell_kinds`).
    // PERF: edleak1. A plain read is the compare, branch and load of
    // `kinds[index]`: the branch to the pooled read takes the place of the
    // bounds-check panic.
    #[inline]
    fn kind(&self, index: usize) -> SyntaxKind {
        match self.kinds.get(index) {
            Some(&kind) => kind,
            None => self.file.shell_kind(index),
        }
    }
}

impl BlockFile {
    /// `FileStore::facts` of the file.
    #[inline]
    fn facts(&self) -> StoreFacts {
        self.facts.get()
    }

    /// `FileStore::root` of the file.
    #[inline]
    fn root(&self) -> Node {
        Node(self.root.load(Ordering::Relaxed))
    }

    /// `FileBlock::kind` of slot `index` of a node shell. A shell that uses
    /// the `BlockFile` of its pooled block reads the whole block column, so
    /// an index past its slots and inside the block gives a stale kind of an
    /// earlier owner of the block (only an invalid handle reads there). Past
    /// the column it panics.
    // PERF (edleak3): a table read, with no call. `SyntaxKind::try_from`
    // is in goport_util, so it was a call, and `expect` a second branch.
    // The read stays out of line: inline at each kind read (as `try_from`
    // or as this table read), `--singleThreaded --noEmit` cost up to
    // +0.40% instructions on effect, +0.15% on hono, with no pooled column.
    #[cold]
    #[inline(never)]
    fn shell_kind(&self, index: usize) -> SyntaxKind {
        let word = self.shell_kinds[index].load(Ordering::Relaxed);
        KIND_OF_WORD[usize::from(word) & (KIND_OF_WORD.len() - 1)]
    }
}

/// Each `SyntaxKind` at the index of its value, then `Unknown`: the kind of
/// a word of a pooled kind column (`BlockFile::shell_kind`). Only
/// `node_shell` writes the column, from kinds. A test checks the table
/// against `SyntaxKind::try_from` (`pooled_kind_table_has_every_kind`).
static KIND_OF_WORD: [SyntaxKind; 512] = {
    use SyntaxKind as K;
    #[rustfmt::skip]
    const ALL: [SyntaxKind; SyntaxKind::COUNT] = [
        K::Unknown, K::EndOfFile, K::SingleLineCommentTrivia, K::MultiLineCommentTrivia,
        K::NewLineTrivia, K::WhitespaceTrivia, K::ConflictMarkerTrivia, K::NonTextFileMarkerTrivia,
        K::NumericLiteral, K::BigIntLiteral, K::StringLiteral, K::JsxText, K::JsxTextAllWhiteSpaces,
        K::RegularExpressionLiteral, K::NoSubstitutionTemplateLiteral, K::TemplateHead,
        K::TemplateMiddle, K::TemplateTail, K::OpenBraceToken, K::CloseBraceToken, K::OpenParenToken,
        K::CloseParenToken, K::OpenBracketToken, K::CloseBracketToken, K::DotToken, K::DotDotDotToken,
        K::SemicolonToken, K::CommaToken, K::QuestionDotToken, K::LessThanToken, K::LessThanSlashToken,
        K::GreaterThanToken, K::LessThanEqualsToken, K::GreaterThanEqualsToken, K::EqualsEqualsToken,
        K::ExclamationEqualsToken, K::EqualsEqualsEqualsToken, K::ExclamationEqualsEqualsToken,
        K::EqualsGreaterThanToken, K::PlusToken, K::MinusToken, K::AsteriskToken,
        K::AsteriskAsteriskToken, K::SlashToken, K::PercentToken, K::PlusPlusToken, K::MinusMinusToken,
        K::LessThanLessThanToken, K::GreaterThanGreaterThanToken,
        K::GreaterThanGreaterThanGreaterThanToken, K::AmpersandToken, K::BarToken, K::CaretToken,
        K::ExclamationToken, K::TildeToken, K::AmpersandAmpersandToken, K::BarBarToken,
        K::QuestionToken, K::ColonToken, K::AtToken, K::QuestionQuestionToken, K::BacktickToken,
        K::HashToken, K::EqualsToken, K::PlusEqualsToken, K::MinusEqualsToken, K::AsteriskEqualsToken,
        K::AsteriskAsteriskEqualsToken, K::SlashEqualsToken, K::PercentEqualsToken,
        K::LessThanLessThanEqualsToken, K::GreaterThanGreaterThanEqualsToken,
        K::GreaterThanGreaterThanGreaterThanEqualsToken, K::AmpersandEqualsToken, K::BarEqualsToken,
        K::BarBarEqualsToken, K::AmpersandAmpersandEqualsToken, K::QuestionQuestionEqualsToken,
        K::CaretEqualsToken, K::Identifier, K::PrivateIdentifier, K::JsDocCommentTextToken,
        K::BreakKeyword, K::CaseKeyword, K::CatchKeyword, K::ClassKeyword, K::ConstKeyword,
        K::ContinueKeyword, K::DebuggerKeyword, K::DefaultKeyword, K::DeleteKeyword, K::DoKeyword,
        K::ElseKeyword, K::EnumKeyword, K::ExportKeyword, K::ExtendsKeyword, K::FalseKeyword,
        K::FinallyKeyword, K::ForKeyword, K::FunctionKeyword, K::IfKeyword, K::ImportKeyword,
        K::InKeyword, K::InstanceOfKeyword, K::NewKeyword, K::NullKeyword, K::ReturnKeyword,
        K::SuperKeyword, K::SwitchKeyword, K::ThisKeyword, K::ThrowKeyword, K::TrueKeyword,
        K::TryKeyword, K::TypeOfKeyword, K::VarKeyword, K::VoidKeyword, K::WhileKeyword, K::WithKeyword,
        K::ImplementsKeyword, K::InterfaceKeyword, K::LetKeyword, K::PackageKeyword, K::PrivateKeyword,
        K::ProtectedKeyword, K::PublicKeyword, K::StaticKeyword, K::YieldKeyword, K::AbstractKeyword,
        K::AccessorKeyword, K::AsKeyword, K::AssertsKeyword, K::AssertKeyword, K::AnyKeyword,
        K::AsyncKeyword, K::AwaitKeyword, K::BooleanKeyword, K::ConstructorKeyword, K::DeclareKeyword,
        K::GetKeyword, K::ImmediateKeyword, K::InferKeyword, K::IntrinsicKeyword, K::IsKeyword,
        K::KeyOfKeyword, K::ModuleKeyword, K::NamespaceKeyword, K::NeverKeyword, K::OutKeyword,
        K::ReadonlyKeyword, K::RequireKeyword, K::NumberKeyword, K::ObjectKeyword, K::SatisfiesKeyword,
        K::SetKeyword, K::StringKeyword, K::SymbolKeyword, K::TypeKeyword, K::UndefinedKeyword,
        K::UniqueKeyword, K::UnknownKeyword, K::UsingKeyword, K::FromKeyword, K::GlobalKeyword,
        K::BigIntKeyword, K::OverrideKeyword, K::OfKeyword, K::DeferKeyword, K::SourceKeyword,
        K::QualifiedName, K::ComputedPropertyName, K::TypeParameter, K::Parameter, K::Decorator,
        K::PropertySignature, K::PropertyDeclaration, K::MethodSignature, K::MethodDeclaration,
        K::ClassStaticBlockDeclaration, K::Constructor, K::GetAccessor, K::SetAccessor,
        K::CallSignature, K::ConstructSignature, K::IndexSignature, K::TypePredicate, K::TypeReference,
        K::FunctionType, K::ConstructorType, K::TypeQuery, K::TypeLiteral, K::ArrayType, K::TupleType,
        K::OptionalType, K::RestType, K::UnionType, K::IntersectionType, K::ConditionalType,
        K::InferType, K::ParenthesizedType, K::ThisType, K::TypeOperator, K::IndexedAccessType,
        K::MappedType, K::LiteralType, K::NamedTupleMember, K::TemplateLiteralType,
        K::TemplateLiteralTypeSpan, K::ImportType, K::ObjectBindingPattern, K::ArrayBindingPattern,
        K::BindingElement, K::ArrayLiteralExpression, K::ObjectLiteralExpression,
        K::PropertyAccessExpression, K::ElementAccessExpression, K::CallExpression, K::NewExpression,
        K::TaggedTemplateExpression, K::TypeAssertionExpression, K::ParenthesizedExpression,
        K::FunctionExpression, K::ArrowFunction, K::DeleteExpression, K::TypeOfExpression,
        K::VoidExpression, K::AwaitExpression, K::PrefixUnaryExpression, K::PostfixUnaryExpression,
        K::BinaryExpression, K::ConditionalExpression, K::TemplateExpression, K::YieldExpression,
        K::SpreadElement, K::ClassExpression, K::OmittedExpression, K::ExpressionWithTypeArguments,
        K::AsExpression, K::NonNullExpression, K::MetaProperty, K::SyntheticExpression,
        K::SatisfiesExpression, K::TemplateSpan, K::SemicolonClassElement, K::Block, K::EmptyStatement,
        K::VariableStatement, K::ExpressionStatement, K::IfStatement, K::DoStatement, K::WhileStatement,
        K::ForStatement, K::ForInStatement, K::ForOfStatement, K::ContinueStatement, K::BreakStatement,
        K::ReturnStatement, K::WithStatement, K::SwitchStatement, K::LabeledStatement,
        K::ThrowStatement, K::TryStatement, K::DebuggerStatement, K::VariableDeclaration,
        K::VariableDeclarationList, K::FunctionDeclaration, K::ClassDeclaration,
        K::InterfaceDeclaration, K::TypeAliasDeclaration, K::EnumDeclaration, K::ModuleDeclaration,
        K::ModuleBlock, K::CaseBlock, K::NamespaceExportDeclaration, K::ImportEqualsDeclaration,
        K::ImportDeclaration, K::ImportClause, K::NamespaceImport, K::NamedImports, K::ImportSpecifier,
        K::ExportAssignment, K::ExportDeclaration, K::NamedExports, K::NamespaceExport,
        K::ExportSpecifier, K::MissingDeclaration, K::ExternalModuleReference, K::JsxElement,
        K::JsxSelfClosingElement, K::JsxOpeningElement, K::JsxClosingElement, K::JsxFragment,
        K::JsxOpeningFragment, K::JsxClosingFragment, K::JsxAttribute, K::JsxAttributes,
        K::JsxSpreadAttribute, K::JsxExpression, K::JsxNamespacedName, K::CaseClause, K::DefaultClause,
        K::HeritageClause, K::CatchClause, K::ImportAttributes, K::ImportAttribute,
        K::PropertyAssignment, K::ShorthandPropertyAssignment, K::SpreadAssignment, K::EnumMember,
        K::SourceFile, K::JsDocTypeExpression, K::JsDocNameReference, K::JsDocAllType,
        K::JsDocNullableType, K::JsDocNonNullableType, K::JsDocOptionalType, K::JsDocVariadicType,
        K::JsDoc, K::JsDocText, K::JsDocTypeLiteral, K::JsDocSignature, K::JsDocLink, K::JsDocLinkCode,
        K::JsDocLinkPlain, K::JsDocUnknownTag, K::JsDocAugmentsTag, K::JsDocImplementsTag,
        K::JsDocDeprecatedTag, K::JsDocPublicTag, K::JsDocPrivateTag, K::JsDocProtectedTag,
        K::JsDocReadonlyTag, K::JsDocOverrideTag, K::JsDocCallbackTag, K::JsDocOverloadTag,
        K::JsDocParameterTag, K::JsDocReturnTag, K::JsDocThisTag, K::JsDocTypeTag, K::JsDocTemplateTag,
        K::JsDocTypedefTag, K::JsDocSeeTag, K::JsDocPropertyTag, K::JsDocThrowsTag,
        K::JsDocSatisfiesTag, K::JsDocImportTag, K::SyntaxList, K::JsTypeAliasDeclaration,
        K::JsImportDeclaration, K::NotEmittedStatement, K::PartiallyEmittedExpression,
        K::SyntheticReferenceExpression, K::NotEmittedTypeElement,
    ];
    let mut table = [K::Unknown; 512];
    let mut i = 0;
    while i < ALL.len() {
        table[i] = ALL[i];
        i += 1;
    }
    table
};

/// One `FileBlock` and its `OnceLock` state in one cache line.
#[repr(align(64))]
struct BlockEntry(OnceLock<FileBlock>);

const _: () = assert!(std::mem::size_of::<BlockEntry>() == 64);

/// AST node records, step 3: the entries of the file ids below
/// `LOW_BLOCKS`. A `OnceLock` is not all zero bytes, so this is `.data` in
/// the binary. Native: 4 MiB, and a page of it costs memory only when an
/// entry in it is set. wasm: 512 KiB, all of it in linear memory from the
/// start.
static FILE_BLOCKS: [BlockEntry; LOW_BLOCKS] = [const { BlockEntry(OnceLock::new()) }; LOW_BLOCKS];

/// The entries of the file ids from `LOW_BLOCKS` on, in chunks of
/// `HIGH_CHUNK` ids, each made by the first publish of an id in it. A
/// process with many programs (a test binary, a long watch) gets there.
static HIGH_BLOCKS: [OnceLock<Box<[BlockEntry; HIGH_CHUNK]>>;
    (FILE_ID_LIMIT - LOW_BLOCKS) / HIGH_CHUNK] =
    [const { OnceLock::new() }; (FILE_ID_LIMIT - LOW_BLOCKS) / HIGH_CHUNK];

/// The next unused file id. Only `publish_file_stores` changes it.
static PUBLISHED: AtomicUsize = AtomicUsize::new(0);

/// Marks the `HIGH_BLOCKS` part of `file_block` as cold. It does nothing.
// PERF: rustc gives a branch into a block that calls a `#[cold]` function
// a low weight (`find_cold_blocks`), and LLVM puts that block after the hot
// code. `std::hint::cold_path` does the same, but is stable only from Rust
// 1.95. `inline(never)` keeps the call in the MIR until codegen reads it.
// LLVM keeps the call too: each copy of `file_block` has a call to this
// empty function (through the GOT) in its cold block, so an out-of-line
// reader copy is not a leaf function. The hot path runs no call.
#[cold]
#[inline(never)]
fn high_block_path() {}

/// The one file lookup of the registry reads (PORTING.md "AST": store
/// columns are read through one helper): the block of published file
/// `file`, the node shell of a freeable file version included. `None` for
/// any other id: unpublished, synthetic or provisional.
// PERF: M3 `goport -p` cost (mp3). Code for a later program inline in each
// hot read site made `goport -p` 1.5% to 3.5% slower (more icache and
// branch misses). An id below `LOW_BLOCKS` takes the hot path; a synthetic
// id leaves at one more compare; the other ids read `HIGH_BLOCKS` inline in
// a cold block, which calls the empty `high_block_path` and nothing else.
// AST node records, step 4: with debug assertions, it panics when the
// block of `file` is a pooled block that another file version took
// (`block_is_owned`), as a read of a dead version's store does ("file
// version N is released"). A release build checks only the binder field
// reads (`check_block_owner`), not the hot header and kids reads
// (PORTING.md, "AST": a stale read after a reuse gives the new owner's
// data).
// PERF: step 4b. The release body is the one of step 3, with no call to
// `registry_block`: in that form (and a `cfg!` check) LLVM made the entry
// address after the `OnceLock` state test, one more instruction in every
// hot read (`goport -p` about +0.3%).
#[inline]
fn file_block(file: usize) -> Option<&'static FileBlock> {
    #[cfg(debug_assertions)]
    if let Some(b) = registry_block(file)
        && !block_is_owned(b, file)
    {
        super::file_version::released(file);
    }
    if let Some(entry) = FILE_BLOCKS.get(file) {
        return entry.0.get();
    }
    // A synthetic or provisional id is in no publish. It leaves here,
    // outside the cold block.
    if file >= FILE_ID_LIMIT {
        return None;
    }
    high_block_path();
    high_file_block(file)
}

/// AST node records, step 4: true when block `b` of file `file` still
/// belongs to that file: the owner word in its nil slot record is `file`
/// (`NodeRecord`, `bind`). False only for the node shell of a dead
/// freeable file version whose pooled block another version took
/// (`BlockPool`).
#[inline]
fn block_is_owned(b: &FileBlock, file: usize) -> bool {
    b.records
        .get(NIL_SLOT as usize)
        .is_some_and(|r| r.is_owned_by(file))
}

/// `file_block` with no owner check: the registry entry of `file`, for the
/// owner check and the test hooks.
fn registry_block(file: usize) -> Option<&'static FileBlock> {
    match FILE_BLOCKS.get(file) {
        Some(entry) => entry.0.get(),
        None if file >= FILE_ID_LIMIT => None,
        None => high_file_block(file),
    }
}

/// `file_block` for an id from `LOW_BLOCKS` to `FILE_ID_LIMIT`.
// `inline(always)`: in `file_block` it is in a cold block, where LLVM
// inlines less.
#[inline(always)]
fn high_file_block(file: usize) -> Option<&'static FileBlock> {
    let i = file - LOW_BLOCKS;
    HIGH_BLOCKS.get(i / HIGH_CHUNK)?.get()?[i % HIGH_CHUNK]
        .0
        .get()
}

/// Sets the entry of file `file`, once, at its publish.
fn set_file_block(file: usize, block: FileBlock) {
    debug_assert!(block_is_owned(&block, file), "the block of file {file}");
    let entry = match FILE_BLOCKS.get(file) {
        Some(entry) => entry,
        None => {
            assert!(file < FILE_ID_LIMIT, "too many file ids");
            let i = file - LOW_BLOCKS;
            &HIGH_BLOCKS[i / HIGH_CHUNK].get_or_init(new_high_chunk)[i % HIGH_CHUNK]
        }
    };
    assert!(
        entry.0.set(block).is_ok(),
        "file {file} is already published"
    );
}

/// A new `HIGH_BLOCKS` chunk, made on the heap: `Box::new` of the array
/// can make it on the stack first (1 MiB), which a test thread or wasm
/// does not have.
fn new_high_chunk() -> Box<[BlockEntry; HIGH_CHUNK]> {
    let chunk: Box<[BlockEntry]> = (0..HIGH_CHUNK)
        .map(|_| BlockEntry(OnceLock::new()))
        .collect();
    let Ok(chunk) = chunk.try_into() else {
        unreachable!("a chunk has HIGH_CHUNK entries")
    };
    chunk
}

/// `try_resolve_store_id(file, index)` for published store `file`, whose
/// block is `b`.
// PERF: effect P7-1. Most stores have no alias slot. For them a child id is
// the slot handle (or nil for slot 0), so `Node::new` needs no load from a
// per-slot table. A store with alias slots reads the record of the id.
#[inline]
fn block_resolve_slot(b: &FileBlock, file: usize, index: usize) -> Node {
    if !b.file.facts().alias_free {
        return record_resolve(file, index, b.records);
    }
    // Slot 0 always resolves to nil (its record never changes).
    let n = if index == NIL_SLOT as usize {
        Node::NIL
    } else {
        handle(file, index as u32)
    };
    debug_assert_eq!(n, record_resolve(file, index, b.records));
    n
}

/// The store and `GoFile` of a freeable file version (lsshells M3b), owned
/// by its `FileVersion` (`ast::file_version`). The publish moves them here
/// instead of leaking them. They are freed when the version dies, or after
/// it on a free thread when a pin release of the stdio API or the build
/// takes them out of the dying version (`FileVersion::take_data`). The
/// block of its id is its node shell (`node_shell`), whose `BlockFile` has
/// no store and no `GoFile`.
pub(crate) struct VersionStore {
    /// The store, without its node columns, which are in the node shell,
    /// and without the columns that only cache node data (`node_shell`). A
    /// store that owns its astdata nodes (`FileStore::owned`, lsshells M3c)
    /// keeps them and its node column (`FileStore::nodes`).
    store: FileStore,
    go_file: GoFile,
    /// AST node records, step 4: the pooled block that holds the records
    /// and kids of the node shell. It goes back to the pool with the
    /// version (`Drop`).
    block: PoolBlock,
}

impl VersionStore {
    /// The `GoFile` of the version.
    #[inline]
    pub(crate) fn go_file(&self) -> &GoFile {
        &self.go_file
    }
}

impl Drop for VersionStore {
    // The version is dead: `FileVersion::drop` put its id in the dead ids
    // and took it out of the registry before its fields drop, or before a
    // free thread drops this store (`FileVersion::take_data`).
    fn drop(&mut self) {
        give_back_pool_block(std::mem::take(&mut self.block));
    }
}

/// AST node records, step 4: the records and kids of one pooled block
/// (`BlockPool`), leaked once and then reused by the node shells of
/// freeable file versions. Not `Copy` and not `Clone`, so a block is in one
/// place only: a `VersionStore`, the quarantine or a free list. The
/// default is the empty block, which is not pooled.
#[derive(Default)]
struct PoolBlock {
    records: &'static [NodeRecord],
    kids: &'static [NodeKids],
    /// edleak1: the kind column of the node shell (`BlockFile::shell_kinds`).
    kinds: &'static [AtomicU16],
    /// edleak1 part 2: the `BlockFile` of the node shells of this block,
    /// leaked with it. Each node shell that takes the block sets its facts
    /// and root (`node_shell`), so a shell leaks no `BlockFile`. Its
    /// `shell_kinds` are all of `kinds`. `None` in the empty block.
    file: Option<&'static BlockFile>,
}

impl PoolBlock {
    /// A fresh block of `cap` slots, all words 0, leaked.
    fn new(cap: usize) -> Self {
        let kinds: &'static [AtomicU16] = Vec::leak((0..cap).map(|_| AtomicU16::new(0)).collect());
        Self {
            records: Vec::leak((0..cap).map(|_| NodeRecord::zero()).collect()),
            kids: Vec::leak((0..cap).map(|_| NodeKids::unknown()).collect()),
            kinds,
            file: Some(Box::leak(Box::new(BlockFile {
                nodes: &[],
                lists: &[],
                facts: FactsCell::new(StoreFacts::ONLY_NIL_SLOT),
                root: AtomicU64::new(Node::NIL.0),
                foreign: &[],
                links: &[],
                shell_kinds: kinds,
                store: None,
                go_file: None,
                subtree_facts: OnceLock::new(),
            }))),
        }
    }

    /// The number of slots.
    fn cap(&self) -> usize {
        self.records.len()
    }
}

/// AST node records, step 4: the blocks of the node shells of dead freeable
/// file versions, for the node shells of later versions. An edit then
/// reuses the 42 bytes per node (a record, its kids and its kind, 24 + 16 +
/// 2) of an older version of the file, where each shell leaked them before.
/// The blocks never go back to the allocator.
/// - A version's store gives its block back when it drops (`VersionStore`):
///   when the version dies, or a little after on a free thread when a pin
///   release or the build takes the store out of the dying version
///   (`FileVersion::take_data`). The pin epoch is read at that drop. The
///   block waits in `quarantine` until `QUARANTINE_RELEASES` more pin
///   releases (`file_version::pin_epoch`) have happened, then goes to the
///   free list of its size class (`pool_class`). A pin release is a
///   program release, or the release of a source file lease that held the
///   last holder of a freeable parse
///   (`file_version::release_file_version_pins_later`, apimem1).
/// - A node shell of `len` slots takes the first free block of `len` to
///   `2 * len + 64` slots (`take_pool_block`), or a fresh one of
///   `len + len / 8 + 64` slots, so a file that grows a little while it is
///   edited still fits its old block; `len + len / 2 + 64` when the pool has
///   a block that the file outgrew (edleak1 part 2).
// PORT: Go frees a dead `*ast.SourceFile` with the GC. Every holder of a
// node of a version holds the version, so no read of it can follow its
// death; the quarantine is a margin for a thread that keeps a node handle
// and reads only its header, which does not pin the version. A client that
// releases leases fast makes the pin releases come sooner, so the margin is
// shorter in time. After a reuse such a read gives the new owner's data;
// with debug assertions it panics (`file_block`, `block_is_owned`).
struct BlockPool {
    /// The free blocks by size class.
    free: Vec<Vec<PoolBlock>>,
    /// The given-back blocks with the pin epoch from which they are free,
    /// in the order they came back (the epochs only grow).
    quarantine: VecDeque<(usize, PoolBlock)>,
    /// Each plain kind column of a node shell (`plain_kinds`), by the hash
    /// of its kinds.
    kind_columns: FxHashMap<u64, Vec<&'static [SyntaxKind]>>,
    /// The newest plain kind column of each file whose node shells have
    /// one, by the hash of its name (`plain_kinds`).
    kind_names: FxHashMap<u64, &'static [SyntaxKind]>,
    /// The bytes of the plain kind columns of later versions of files
    /// (`LATER_KINDS_BUDGET`).
    later_kind_bytes: usize,
}

/// The number of pin releases a given-back block waits (`BlockPool`). A
/// version usually dies inside a release, after its epoch bump, and a node
/// shell takes its block before the release of its edit: in a language
/// server with no lease release between the edits, the version that dies
/// in the release of edit N gives its block to the version of edit N + 3.
const QUARANTINE_RELEASES: usize = 2;

static POOL: Mutex<BlockPool> = Mutex::new(BlockPool {
    free: Vec::new(),
    quarantine: VecDeque::new(),
    kind_columns: FxHashMap::with_hasher(rustc_hash::FxBuildHasher),
    kind_names: FxHashMap::with_hasher(rustc_hash::FxBuildHasher),
    later_kind_bytes: 0,
});

/// The bytes of the plain kind columns that `plain_kinds` leaks for later
/// versions of files. Past it, the new kinds of a later version go in its
/// pooled block.
// PERF: edleak1 round 2. A plain column read is the compare, branch and
// load of a static file. A pooled column read is one cold call
// (`FileBlock::kind`), and an edit reads the nodes of the edited file
// about 400,000 times: with the kinds of every node shell pooled, a
// 400-edit query-core editor session cost +4.2% instructions:u, and with
// those of later versions pooled (no budget, `SyntaxKind::try_from` inline
// in the cold read) +2.75%. The pooled read
// inline at each kind read cost +1.95% there, and +0.15% to +0.39% on the
// 4 `--singleThreaded` gate projects. 4 MiB is about 2% of the server's
// RSS on query-core; before edleak1 its shells leaked that in about 1,400
// stock edits.
// PERF (edleak3): the budget stays. With none (every later version
// pooled), a 400-edit session cost +1.79% instructions:u on query-core and
// +0.28% on hono with a table read inline at each kind read, and +0.46%
// and +0.26% with the plain prefix too (`plain_kinds`); the inline read
// cost `--singleThreaded --noEmit` up to +0.40% on effect. Past the budget
// a shell reads its plain prefix, and the rest with the cold table read
// (`BlockFile::shell_kind`).
const LATER_KINDS_BUDGET: usize = 4 << 20;

/// The plain kind column (`FileBlock::kinds`) of a node shell of file
/// `name` whose store has kinds `kinds` (`Ok`), or the plain prefix of a
/// shell whose kinds go in its pooled block (`Err`,
/// `BlockFile::shell_kinds`):
/// - an equal column that an earlier shell leaked (a column never changes,
///   so shells can share it). An edit inside a token or a literal keeps
///   every kind of the file, and a file parsed again with the same text (a
///   `tsc --watch` build after a config change) has the same kinds;
/// - else `kinds`, leaked (it takes the vector), for the first shell of
///   `name`, and for a later one while the columns of later versions stay
///   in `LATER_KINDS_BUDGET`;
/// - else (`Err`) the slots of the newest plain column of `name` up to the
///   first kind that differs, which the shell reads plain.
// PORT: Go frees the kinds with the file. A plain column is not atomic, so
// a later file cannot reuse it: before edleak1 each new column leaked, about
// 0.4 columns per edit in a long editor session. The first columns grow
// only with the files the process reads.
// PERF: watchfree1. A column is hashed and compared once, about the cost
// of the compare with the last column before.
// PERF (edleak3): the prefix. The parser makes the slots of a file in text
// order, so the slots before an edit keep their kinds, and a pooled shell
// reads them plain, at no cost: the prefix is part of a column that is
// already leaked. Past the budget, a 400-edit query-core session (every
// later version pooled) cost +0.46% instructions:u with the prefix, +1.79%
// without it (an inline pooled read in both).
fn plain_kinds(
    name: &str,
    kinds: &mut Vec<SyntaxKind>,
) -> Result<&'static [SyntaxKind], &'static [SyntaxKind]> {
    use std::hash::BuildHasher;
    let hash = rustc_hash::FxBuildHasher.hash_one(kinds.as_slice());
    let name_hash = rustc_hash::FxBuildHasher.hash_one(name);
    let mut pool = lock_pool();
    let pool = &mut *pool;
    if let Some(column) = pool
        .kind_columns
        .get(&hash)
        .and_then(|columns| columns.iter().find(|column| ***column == **kinds))
        .copied()
    {
        if let Some(newest) = pool.kind_names.get_mut(&name_hash) {
            *newest = column;
        }
        return Ok(column);
    }
    if let Some(&newest) = pool.kind_names.get(&name_hash) {
        // A later version of the file, with new kinds.
        let bytes = pool.later_kind_bytes + std::mem::size_of_val(kinds.as_slice());
        if bytes > LATER_KINDS_BUDGET {
            let same = newest
                .iter()
                .zip(kinds.iter())
                .take_while(|(old, new)| old == new)
                .count();
            return Err(&newest[..same]);
        }
        pool.later_kind_bytes = bytes;
    }
    let column: &'static [SyntaxKind] = Vec::leak(std::mem::take(kinds));
    pool.kind_columns.entry(hash).or_default().push(column);
    pool.kind_names.insert(name_hash, column);
    Ok(column)
}

fn lock_pool() -> std::sync::MutexGuard<'static, BlockPool> {
    POOL.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The size class of a block of `cap` slots: `floor(log2(cap))`.
#[inline]
fn pool_class(cap: usize) -> usize {
    cap.max(1).ilog2() as usize
}

/// A block of at least `len` slots for a node shell (`BlockPool`).
fn take_pool_block(len: usize) -> PoolBlock {
    let most = 2 * len + 64;
    let mut pool = lock_pool();
    let BlockPool {
        free, quarantine, ..
    } = &mut *pool;
    let epoch = super::file_version::pin_epoch();
    let ready = quarantine
        .iter()
        .take_while(|(from, _)| *from <= epoch)
        .count();
    for (_, block) in quarantine.drain(..ready) {
        let class = pool_class(block.cap());
        if free.len() <= class {
            free.resize_with(class + 1, Vec::new);
        }
        free[class].push(block);
    }
    let found = free
        .iter_mut()
        .take(pool_class(most) + 1)
        .skip(pool_class(len))
        .find_map(|list| {
            let i = list
                .iter()
                .position(|block| (len..=most).contains(&block.cap()))?;
            Some(list.swap_remove(i))
        });
    if let Some(block) = found {
        return block;
    }
    // edleak1 part 2: a file that outgrew its blocks (a pooled block of 3/4
    // of `len` to `len`) gets half its length as headroom, else an eighth.
    // The outgrown blocks stay in the pool. A file that grows on each edit
    // left about 4 / h times the size of its block there (h the headroom):
    // 8.8 MiB of blocks in 10,000 query-core edits with an eighth.
    let outgrown = |block: &PoolBlock| block.cap() < len && 4 * block.cap() >= 3 * len;
    let grows = free.iter().flatten().any(outgrown) || quarantine.iter().any(|(_, b)| outgrown(b));
    drop(pool);
    PoolBlock::new(len + if grows { len / 2 } else { len / 8 } + 64)
}

/// Puts the block of a dead version in the quarantine (`BlockPool`).
fn give_back_pool_block(block: PoolBlock) {
    if block.cap() == 0 {
        return;
    }
    let mut pool = lock_pool();
    // Read inside the lock, so the quarantine stays in epoch order.
    let ready = super::file_version::pin_epoch() + QUARANTINE_RELEASES;
    pool.quarantine.push_back((ready, block));
}

/// Empties the pool (its blocks stay leaked), so a test sees only the
/// blocks it gave back.
#[cfg(test)]
fn clear_block_pool() {
    let mut pool = lock_pool();
    pool.free.clear();
    pool.quarantine.clear();
    pool.kind_columns.clear();
    pool.kind_names.clear();
    pool.later_kind_bytes = 0;
}

/// Test hook (AST node records, step 4): the address of the records of the
/// block of the file of store node `n`, with no owner check. Two versions
/// whose node shells share a pooled block give the same address.
#[doc(hidden)]
#[must_use]
pub fn node_block_addr(n: Node) -> Option<usize> {
    registry_block(n.file_index()).map(|b| b.records.as_ptr() as usize)
}

/// Test hook (AST node records, step 4): true when the file of store node
/// `n` is published and its block still belongs to it (`block_is_owned`).
#[doc(hidden)]
#[must_use]
pub fn node_block_is_owned(n: Node) -> bool {
    registry_block(n.file_index()).is_some_and(|b| block_is_owned(b, n.file_index()))
}

/// Runs `read` on the store of published file `file`: the store in its
/// block, or, when the block is the node shell of a freeable file version,
/// the store of that version, pinned while `read` runs (panics for a dead
/// version). `None` for any other file; `read` then did not run.
#[inline]
fn with_published_store<R>(file: usize, read: impl FnOnce(&FileStore) -> R) -> Option<R> {
    let block = file_block(file)?;
    match block.file.store {
        Some(store) => Some(read(store)),
        None => with_shell_version(file, |version| read(&version.store)),
    }
}

/// `with_version_store` for a node shell block, out of line.
// PERF: lsshells M3b. A call to the pinned read in each inline fast path
// made those paths non-leaf code: LLVM saved registers on each call and
// stopped inlining some readers, and `goport -p` ran 2% to 3.4% more
// instructions (lsshells/m3/M3b/prof).
#[cold]
#[inline(never)]
fn with_shell_version<R>(file: usize, read: impl FnOnce(&VersionStore) -> R) -> Option<R> {
    with_version_store(file, read)
}

/// `read` on the store and `GoFile` of live freeable file version `file`
/// (lsshells M3c), pinned while `read` runs. `None` for any other id and
/// before the version is published; `read` then did not run. Panics for a
/// dead version.
// PERF: lsshells M3c. The node data reads of the edited file come here
// (`with_scoped_store_node`, `with_store_list`), with no view of the
// version.
#[inline]
fn with_version_store<R>(file: usize, read: impl FnOnce(&VersionStore) -> R) -> Option<R> {
    if super::file_version::is_hot(file) {
        return Some(super::file_version::with_hot(read));
    }
    if !super::file_version::any_freeable_published()
        || file >= FILE_ID_LIMIT
        || file >= PUBLISHED.load(Ordering::Acquire)
    {
        return None;
    }
    super::file_version::with_file_version(file, |version| version.published().map(read)).flatten()
}

/// The live freeable version of published file `file` as a pin, for a
/// `FileRef::Pinned` guard. `None` for any other id and before the
/// version is published. Panics for a dead version.
#[cold]
#[inline(never)]
fn published_version(file: usize) -> Option<super::file_version::VersionPin> {
    if super::file_version::is_hot(file) {
        return Some(super::file_version::hot_pin());
    }
    if file >= FILE_ID_LIMIT
        || !super::file_version::any_freeable_published()
        || file >= PUBLISHED.load(Ordering::Acquire)
    {
        return None;
    }
    super::file_version::pinned_file_version(file).filter(|version| version.published().is_some())
}

/// The node data of the nodes of one published store file, for a walk
/// that reads every node of that file (the API encoder, `encode_tree`).
/// Not in Go (perf, apiperf2): a read (`FileNodeReader::data`) is a few
/// loads, with no file lookup and no pin per node (`with_scoped_store_node`
/// makes both for a node of a freeable file version that is not hot).
pub enum FileNodes {
    /// The node column of a static publish (`FileBlock::node_column`).
    Static(&'static [Option<ColumnData>]),
    /// A freeable file version (lsshells M3c), pinned while this value
    /// lives.
    Pinned(super::file_version::VersionPin),
}

impl FileNodes {
    /// The nodes of published store file `file`. `None` for a synthetic
    /// id and for a store that is not published.
    #[must_use]
    pub fn of(file: usize) -> Option<Self> {
        if let Some(nodes) = file_block(file).and_then(FileBlock::node_column) {
            return Some(Self::Static(nodes));
        }
        published_version(file).map(Self::Pinned)
    }

    /// The reader of the nodes of file `file`, the file of `FileNodes::of`.
    #[must_use]
    pub fn reader(&self, file: usize) -> FileNodeReader<'_> {
        // The records of a freeable version are in its node shell, which
        // lives while the version is pinned.
        let block = file_block(file).expect("a published store has a block");
        check_block_owner(block, file);
        let column = match self {
            Self::Static(nodes) => NodeColumn::Static(nodes),
            Self::Pinned(version) => NodeColumn::Store(
                &version
                    .published()
                    .expect("a pinned file version is published")
                    .store,
            ),
        };
        FileNodeReader { block, column }
    }
}

/// The node reads of a `FileNodes`.
#[derive(Clone, Copy)]
pub struct FileNodeReader<'a> {
    block: &'a FileBlock,
    column: NodeColumn<'a>,
}

#[derive(Clone, Copy)]
enum NodeColumn<'a> {
    Static(&'static [Option<ColumnData>]),
    Store(&'a FileStore),
}

impl<'a> FileNodeReader<'a> {
    /// The node data of `n`, a node of this file (`Node::file_index`):
    /// what `with_ast_data` reads for it.
    #[inline]
    #[must_use]
    pub fn data(self, n: Node) -> NodeData<'a> {
        let index = slot_index(n);
        match self.column {
            NodeColumn::Static(nodes) => slot_node(nodes[index]),
            NodeColumn::Store(store) => store.slot_ast_node(index),
        }
    }

    /// Go `node.Kind`, `node.Loc` and `node.Flags` of `n`, a node of this
    /// file: what `Node::kind`, `Node::loc` and `Node::flags` read, with one
    /// record read.
    #[inline]
    #[must_use]
    pub fn header(self, n: Node) -> (SyntaxKind, TextRange, NodeFlags) {
        let index = slot_index(n);
        let record = &self.block.records[index];
        (self.block.kind(index), record.loc(), record.flags())
    }
}

/// Node reads of an unpublished store of this thread under one borrow of
/// the store (`with_build_store_reads`): each read is a column load. A
/// read of a node that is not a node of this store with `'static` data
/// gives `None`; the caller then reads it with the `Node` accessors.
// PERF: pathsdiag1. A `Node` read of a store that is not published and not
// the active store finds the store in `BUILD` on each read, and a kind
// read there makes the whole header (`try_store_header_slow`). The config
// file checks of a program read the config file before the publish: a
// `paths` diagnostic reads every `paths` key before its own, as Go
// (`for_each_property_assignment`).
pub(crate) struct BuildStoreReads<'a> {
    file: usize,
    store: &'a FileStore,
    /// The store has no alias slot, so an id needs no record read
    /// (`FrozenIds::Direct`).
    alias_free: bool,
}

impl BuildStoreReads<'_> {
    /// `Node::new(file, id)` for child id `id` of a node of this store.
    #[inline]
    #[must_use]
    pub fn node(&self, id: crate::astdata::NodeId) -> Node {
        let index = id.index();
        if !self.alias_free {
            self.store.slot_resolve(self.file, index)
        } else if index == NIL_SLOT as usize {
            Node::NIL
        } else {
            handle(self.file, index as u32)
        }
    }

    /// The slot of `n` when it is a node of this store with `'static`
    /// data.
    #[inline]
    fn static_slot(&self, n: Node) -> Option<(usize, NodeData<'static>)> {
        if n.is_nil() || n.file_index() != self.file {
            return None;
        }
        let index = slot_index(n);
        match (self.store.cell_of(index), *self.store.nodes.get(index)?) {
            (NO_CELL, Some(data)) => Some((index, data)),
            _ => None,
        }
    }

    /// Go `n.Kind` and the node data of `n` (`Node::kind`,
    /// `static_ast_node`).
    #[inline]
    #[must_use]
    pub fn kind_and_data(&self, n: Node) -> Option<(SyntaxKind, NodeData<'static>)> {
        let (index, data) = self.static_slot(n)?;
        Some((self.store.kinds[index], data))
    }

    /// Go `n.Text()` (`Node::text`) of `n` when its kind is Identifier,
    /// PrivateIdentifier, StringLiteral, NumericLiteral, BigIntLiteral or
    /// NoSubstitutionTemplateLiteral (the first case of Go
    /// `TryGetTextOfPropertyName`). `None` for other nodes.
    #[inline]
    #[must_use]
    pub fn literal_text(&self, n: Node) -> Option<&'static str> {
        let (index, data) = self.static_slot(n)?;
        // An Identifier data text that is empty is in the name of its slot
        // (`name_node_text`).
        let name_text = |text: &'static str| {
            if text.is_empty() {
                self.store.slot_text_name_of(index).as_str()
            } else {
                text
            }
        };
        Some(match (self.store.kinds[index], data) {
            (SyntaxKind::Identifier, NodeData::Identifier(d)) => name_text(&d.text),
            (SyntaxKind::PrivateIdentifier, NodeData::PrivateIdentifier(d)) => name_text(&d.text),
            (SyntaxKind::StringLiteral, NodeData::StringLiteral(d)) => &d.text,
            (SyntaxKind::NumericLiteral, NodeData::NumericLiteral(d)) => &d.text,
            (SyntaxKind::BigIntLiteral, NodeData::BigIntLiteral(d)) => &d.text,
            (
                SyntaxKind::NoSubstitutionTemplateLiteral,
                NodeData::NoSubstitutionTemplateLiteral(d),
            ) => &d.text,
            _ => return None,
        })
    }
}

/// `read` on the reads of unpublished store `file` of this thread (built
/// or detached). `None` when `file` is not such a store (published, of
/// another thread or synthetic) and when the store is borrowed for a
/// write. `read` must not make or change nodes of the store: a write in
/// `read` panics on the store's `RefCell`. Crate-private for that reason:
/// its one caller is `find_property_assignment` (tsoptions).
pub(crate) fn with_build_store_reads<R>(
    file: usize,
    read: impl FnOnce(&BuildStoreReads<'_>) -> R,
) -> Option<R> {
    if is_storeless_id(file) || file_block(file).is_some() {
        return None;
    }
    let store = build_store(file)?.try_borrow().ok()?;
    Some(read(&BuildStoreReads {
        file,
        store: &store,
        alias_free: store.aliases.is_empty(),
    }))
}

/// The handle of slot `index` in store `file`. Does not resolve aliases.
const fn handle(file: usize, index: u32) -> Node {
    Node(((file as u64) << 32) | (index as u64 + 1))
}

/// Slot index of a store handle.
#[inline]
fn slot_index(n: Node) -> usize {
    ((n.0 & 0xffff_ffff) - 1) as usize
}

/// The unpublished store `file` of this thread (active, detached or built),
/// after a registry miss (`file_block`). Before the first publish every
/// store is here. After it, a synthetic id has no store (a few compares,
/// no call), and any other id takes one cold call.
#[inline]
fn unpublished_store(file: usize) -> Option<StoreCell> {
    if PUBLISHED.load(Ordering::Relaxed) == 0 {
        build_store(file)
    } else if is_storeless_id(file) {
        None
    } else {
        unpublished_store_after_publish(file)
    }
}

/// The cold part of `unpublished_store`.
#[cold]
#[inline(never)]
fn unpublished_store_after_publish(file: usize) -> Option<StoreCell> {
    build_store(file)
}

/// Runs `f` on store `file`: published (static or a freeable file
/// version), or an unpublished store of this thread. `None` when this
/// thread cannot see a store `file`.
#[inline]
fn try_with_store<R>(file: usize, f: impl FnOnce(&FileStore) -> R) -> Option<R> {
    let mut f = Some(f);
    // `with_published_store` gives `None` only when it did not call `read`.
    if let Some(result) = with_published_store(file, |store| {
        (f.take().expect("the store is read once"))(store)
    }) {
        return Some(result);
    }
    let f = f.take()?;
    unpublished_store(file).map(|store| f(&store.borrow()))
}

fn with_store<R>(file: usize, f: impl FnOnce(&FileStore) -> R) -> R {
    try_with_store(file, f)
        .unwrap_or_else(|| panic!("file {file:#x} has no node store on this thread"))
}

/// Runs `f` on unpublished store `file` of this thread. Panics when `file`
/// is published or this thread has no store `file`.
fn with_store_mut<R>(file: usize, f: impl FnOnce(&mut FileStore) -> R) -> R {
    // PERF: query Q8. The parse writes the active store, which is never
    // published (see `ACTIVE`), so it needs no publish check.
    let store = match active_store(file) {
        Some(store) => store,
        None => inactive_unpublished_store(file),
    };
    f(&mut store.borrow_mut())
}

/// The store that `with_store_mut` writes when it is not the active store.
#[inline(never)]
fn inactive_unpublished_store(file: usize) -> StoreCell {
    assert!(
        !is_published(file),
        "cannot change the node store of published file {file:#x}"
    );
    inactive_build_store(file)
        .unwrap_or_else(|| panic!("file {file:#x} has no node store on this thread"))
}

/// Runs `f` on the store and the slot index of the node slot of a store
/// handle, to write its record. Panics on a frozen store. Data writes go
/// through `replace_store_node_data`, which keeps the U1, U4 and R2-5 build
/// entries of the slot.
fn with_slot_mut<R>(n: Node, f: impl FnOnce(&mut FileStore, usize) -> R) -> R {
    with_store_mut(n.file_index(), |s| {
        assert!(!s.frozen, "cannot mutate a node of a finished file");
        let index = slot_index(n);
        match s.nodes[index] {
            Some(_) => f(s, index),
            None => panic!("store handle does not name a node slot"),
        }
    })
}

// ──────────────────────────────────────────────────────────────────────
// Stores
// ──────────────────────────────────────────────────────────────────────

/// A cell for build store `store`: an emptied cell of an earlier publish on
/// this thread (`SPARE_BUILD_CELLS`), else a new one in the AST arena.
fn build_store_cell(store: FileStore) -> StoreCell {
    match SPARE_BUILD_CELLS.with(|cells| cells.borrow_mut().pop()) {
        Some(cell) => {
            let old = cell.replace(store);
            debug_assert!(old.records.is_empty(), "a spare store cell is not empty");
            cell
        }
        None => leak_in_ast_arena(RefCell::new(store)),
    }
}

/// Makes the store of the next parsed file and returns its file id. Ids
/// follow parse order (see `BuildStores`). The store becomes the active
/// store of this thread. Inside a freeable parse scope
/// (`enter_freeable_parse`) the store owns its astdata nodes (`OwnedAst`).
pub fn new_file_store(file_name: &'static str, text: impl Into<FileText>) -> usize {
    let mut new = FileStore::new(file_name, text.into());
    if FREEABLE_PARSE.get() {
        new.owned = Some(Box::new(OwnedAst::new(new.records.capacity())));
    }
    let store: StoreCell = build_store_cell(new);
    let id = BUILD.with(|b| {
        let mut b = b.borrow_mut();
        let id = b.next_id();
        b.stores.push(store);
        id
    });
    ACTIVE.set(Some((id, store)));
    id
}

/// Text bytes per slot that a new store reserves for (`FileStore::new`).
// PERF: U1 (e). The TypeScript parser makes one node for about 6 to 8 text
// bytes of the Query, Zod and Hono sources, and one for about 14 to 21 bytes
// of the lib and `@types` declaration files and the Effect sources (long
// JSDoc comments). At 12 a declaration file, such as lib.dom, fills its
// vectors without a `realloc` copy. `end_parse` gives the unused capacity
// back.
const STORE_TEXT_BYTES_PER_SLOT: usize = 12;

/// `STORE_TEXT_BYTES_PER_SLOT` for a file that is not a declaration file.
// PERF: storefast2. At 12 a source file grew each column once (a doubling,
// to the capacity of 6, and a copy). At 6 it has that capacity at once.
const SOURCE_TEXT_BYTES_PER_SLOT: usize = 6;

impl FileStore {
    fn new(file_name: &'static str, text: FileText) -> Self {
        let bytes_per_slot = if crate::frontend::tspath::is_declaration_file_name(file_name) {
            STORE_TEXT_BYTES_PER_SLOT
        } else {
            SOURCE_TEXT_BYTES_PER_SLOT
        };
        let slots = text.len() / bytes_per_slot + 1;
        let mut records = Vec::with_capacity(slots);
        records.push(NodeRecord::target(Node::NIL));
        let mut kinds = Vec::with_capacity(slots);
        kinds.push(SyntaxKind::Unknown);
        let mut kids = Vec::with_capacity(slots);
        kids.push(NodeKids::unknown());
        let mut nodes = Vec::with_capacity(slots);
        nodes.push(None);
        let mut build_links = Vec::with_capacity(slots);
        build_links.push(SlotLinks::NONE);
        // astshrink1: about one list per 6 nodes (the census of astchunks0),
        // so the table of a large file does not grow through many copies.
        let mut lists = Vec::with_capacity(slots / 6 + 1);
        lists.push(nil_static_list());
        Self {
            file_name,
            text,
            records,
            kinds,
            kids,
            nodes,
            lists,
            build_links,
            ..Self::default()
        }
    }

    /// The U1 (b) modifier bits of static node data whose own modifier list
    /// field is `modifiers` (`slot_kids_parts`).
    #[inline]
    fn static_modifier_bits(&self, modifiers: Option<crate::astdata::ModifierList>) -> u32 {
        modifiers.map_or(0, |m| self.lists[m.list.index()].modifier_flags().0)
    }

    /// The Go `ModifiersToFlags` bits of modifier list field `m` of node data
    /// of this store: data that the store owns (`owned`) names a list of
    /// `OwnedAst::pending`, static data a list of `lists`.
    fn modifier_list_bits(&self, owned: bool, m: crate::astdata::ModifierList) -> u32 {
        if owned {
            self.owned_ref().pending[m.list.index()]
                .flags
                .map_or(0, |flags| flags.0)
        } else {
            self.lists[m.list.index()].modifier_flags().0
        }
    }

    /// U1 (a): the name and the `text_is_keyword` bit of a new slot
    /// of kind `kind` with data `data`. The text is `text`, or the data text
    /// when `text` is `None`. `Name::default()` and false for a kind other
    /// than Identifier and PrivateIdentifier.
    // PERF: storefast1. The kind test is inline, so the other two thirds of
    // the slots make no call.
    #[inline]
    fn slot_text_name(
        &mut self,
        kind: SyntaxKind,
        data: NodeData<'_>,
        text: Option<&str>,
    ) -> (Name, bool) {
        if !is_name_kind(kind) {
            return (Name::default(), false);
        }
        let idents = self.idents.0.get_or_insert_with(take_thread_idents);
        identifier_name(idents, text.unwrap_or_else(|| identifier_text(data)))
    }

    /// The record, kind, kids and link vectors have one entry per slot.
    #[inline]
    fn debug_assert_build_columns(&self) {
        let slots = self.records.len();
        debug_assert_eq!(self.kinds.len(), slots);
        debug_assert_eq!(self.kids.len(), slots);
        debug_assert_eq!(self.nodes.len(), slots);
        debug_assert_eq!(self.build_links.len(), slots);
        debug_assert!(
            self.owned
                .as_deref()
                .is_none_or(|owned| owned.cell_of.len() == slots)
        );
    }

    /// R2-5: frees the chain of slot `index` (no chain holds its old
    /// children then) and makes the chain of `index` unknown.
    fn unlink_children(&mut self, index: usize) {
        unlink_children(&mut self.build_links, index);
    }

    /// R2-5: appends slot `child` to the chain of slot `parent`, whose last
    /// child is `*last` (`LINK_END` before the first). False, with nothing
    /// written, when a chain already holds `child`.
    #[inline]
    fn link_child(&mut self, parent: usize, last: &mut u32, child: usize) -> bool {
        link_child(&mut self.build_links, parent, last, child)
    }

    /// Go `node.Parent = parent` on slot `index`, with `stored` the stored
    /// form of the parent (`NodeHeader::stored_parent`).
    #[inline]
    fn set_slot_parent(&mut self, index: usize, stored: Node) {
        let code = if stored.file_index() == LOCAL_STORE || stored.is_nil() {
            local_parent_code(stored)
        } else {
            self.foreign_parent_code(stored)
        };
        self.records[index].set_parent_code(code);
        debug_assert_eq!(
            self.slot_stored_parent(index),
            stored,
            "stored parent of slot {index}"
        );
    }

    /// The `ParentCode` of `parent`, a parent in another store: a new entry
    /// of `foreign_parents`.
    #[cold]
    #[inline(never)]
    fn foreign_parent_code(&mut self, parent: Node) -> ParentCode {
        let index = ParentCode::try_from(self.foreign_parents.len()).expect("foreign parents");
        assert!(index < FOREIGN_PARENT, "too many foreign parents");
        self.foreign_parents.push(parent);
        FOREIGN_PARENT | index
    }

    /// The stored parent of node slot `index` (`NodeHeader::stored_parent`):
    /// nil, a `LOCAL_STORE` handle or a node of another store.
    fn slot_stored_parent(&self, index: usize) -> Node {
        let code = self.records[index].parent_code();
        if code & FOREIGN_PARENT != 0 {
            self.foreign_parents[(code & !FOREIGN_PARENT) as usize]
        } else if code == 0 {
            Node::NIL
        } else {
            handle(LOCAL_STORE, code - 1)
        }
    }

    /// Go `node.Loc = loc` on slot `index`.
    #[inline]
    fn set_slot_loc(&mut self, index: usize, loc: TextRange) {
        self.records[index].set_loc(loc);
    }

    /// Go `node.Flags = flags` on slot `index`.
    #[inline]
    fn set_slot_flags(&mut self, index: usize, flags: NodeFlags) {
        self.build_facts.flags(flags);
        self.records[index].set_flags(flags);
    }

    /// The header of slot `index` of this store, which has id `file`, as the
    /// node reads see it.
    #[inline]
    fn slot_header(&self, file: usize, index: usize) -> NodeHeader {
        self.records[index].header(self.kinds[index], file, &self.foreign_parents)
    }

    /// `record_resolve` of slot `index` of this store, which has id `file`.
    #[inline]
    fn slot_resolve(&self, file: usize, index: usize) -> Node {
        record_resolve(file, index, &self.records)
    }

    /// U1 (a) (d): the name of slot `index` (`store_identifier_name`).
    #[inline]
    fn slot_text_name_of(&self, index: usize) -> Name {
        self.kids[index].text_name(self.kinds[index])
    }
}

/// The process-wide intern of identifier text `text` (`Name::from`) and Go
/// `scanner.GetIdentifierToken(text) != KindIdentifier`, through the
/// parsing thread's cache `names` (`FileStore::idents`).
// Port only: Go stopped interning parser texts (`internIdentifier`) in
// tsgo#4731.
// PERF: storefast1: the cache lives for the thread, not for one file, so
// the process-wide intern (a shard `Mutex`) and the keyword lookup run once
// per distinct text of the thread, not of each file. identparse1: a hit
// reads the text bytes (`IdentKey`) and one set of a table that keeps the
// recent texts (`IdentNames`), with no text hash and no full clear (VS Code
// parse: 0.42 M to 0.33 M interns for 6.6 M identifiers). Only a miss
// hashes the text and interns it. A text the cache has is interned already,
// so the cache does not change which call interns a text first: name ids
// and their order stay as without it.
// PERF: scanintern1 C3. A text of at most `IdentKey::WHOLE` bytes is found
// by its key alone (`IdentNames::find_whole`), so a hit makes no call and
// saves no registers. A longer text and a miss go to `identifier_name_slow`
// (VS Code load: 70 instructions per call, 93% first-way hits, 5% misses).
#[inline(never)]
fn identifier_name(names: &mut IdentNames, text: &str) -> (Name, bool) {
    // `Name::from("")` is `Name::default()`, and "" is no keyword. Only a
    // missing identifier has no text.
    if text.is_empty() {
        return (Name::default(), false);
    }
    let key = IdentKey::new(text.as_bytes());
    if key.len <= IdentKey::WHOLE
        && let Some(found) = names.find_whole(key)
    {
        return found;
    }
    identifier_name_slow(names, text)
}

/// `identifier_name` of a text that `IdentNames::find_whole` did not find:
/// a text over `IdentKey::WHOLE` bytes (found with a text compare) or a text
/// that is not in the cache (interned, then put in).
// PERF: this makes the key again, and does not take it from
// `identifier_name`: a 32-byte argument goes through the stack, which the
// hit path of `identifier_name` would pay for. Making it again costs only
// the misses.
#[cold]
#[inline(never)]
fn identifier_name_slow(names: &mut IdentNames, text: &str) -> (Name, bool) {
    let key = IdentKey::new(text.as_bytes());
    if key.len > IdentKey::WHOLE
        && let Some(found) = names.find_long(key, text)
    {
        return found;
    }
    let found = intern_identifier(text);
    names.insert(key, found.0.id(), found.1);
    found
}

/// `identifier_name` of a text that is not in the thread's cache.
#[cold]
#[inline(never)]
fn intern_identifier(text: &str) -> (Name, bool) {
    let (name, _) = Name::intern_hashed(text, Name::text_hash(text));
    (name, get_identifier_token(text) != SyntaxKind::Identifier)
}

/// An identifier text as `IdentNames` compares it: its length and up to 24
/// of its bytes. A text of 16 or more bytes keeps its first 16 bytes in `a`
/// and `c` and its last 8 in `b`, a text of 8 to 15 bytes its first and its
/// last 8 in `a` and `b` (the parts overlap when the text is shorter than
/// their sum), a text of 4 to 7 bytes its first and its last 4 in `a`, and a
/// shorter one its first, middle and last byte in `a`. So two texts of at
/// most 24 bytes are equal when their keys are. For a longer text a match
/// also compares the whole text.
#[derive(Clone, Copy)]
struct IdentKey {
    a: u64,
    b: u64,
    c: u64,
    len: u32,
}

impl IdentKey {
    /// The longest text whose key is the whole text.
    const WHOLE: u32 = 24;

    #[inline]
    fn new(t: &[u8]) -> Self {
        let n = t.len();
        let (a, b, c) = if n >= 8 {
            let c = if n >= 16 { le_u64(&t[8..16]) } else { 0 };
            (le_u64(&t[..8]), le_u64(&t[n - 8..]), c)
        } else if n >= 4 {
            (le_u32(&t[..4]) | le_u32(&t[n - 4..]) << 32, 0, 0)
        } else if n > 0 {
            let a = u64::from(t[0]) | u64::from(t[n / 2]) << 8 | u64::from(t[n - 1]) << 16;
            (a, 0, 0)
        } else {
            (0, 0, 0)
        };
        Self {
            a,
            b,
            c,
            len: u32::try_from(n).expect("identifier length"),
        }
    }

    /// The hash whose top bits pick the `IdentNames` set.
    #[inline]
    fn set_hash(self) -> u64 {
        (self.a ^ self.b.rotate_left(21) ^ self.c.rotate_left(42) ^ u64::from(self.len) << 56)
            .wrapping_mul(0x9e37_79b9_7f4a_7c15)
    }
}

/// The 8 bytes `b` as a little-endian number.
#[inline]
fn le_u64(b: &[u8]) -> u64 {
    u64::from_le_bytes(b.try_into().expect("8 bytes"))
}

/// The 4 bytes `b` as a little-endian number.
#[inline]
fn le_u32(b: &[u8]) -> u64 {
    u64::from(u32::from_le_bytes(b.try_into().expect("4 bytes")))
}

/// One `IdentNames` entry (32 bytes): the key of a text, its `Name::id` (0
/// marks an empty entry: only "" has id 0) and its keyword bit
/// (`IDENT_KEYWORD`, in `len_keyword` with the text length).
#[derive(Clone, Copy, Default)]
struct IdentEntry {
    a: u64,
    b: u64,
    c: u64,
    len_keyword: u32,
    id: u32,
}

/// The keyword bit of `IdentEntry::len_keyword`.
const IDENT_KEYWORD: u32 = 1 << 31;

impl IdentEntry {
    /// True when this entry holds the text `text`, whose key is `key`. The
    /// text is compared too, so this is right for a text of any length
    /// (`holds_key` is enough for at most `IdentKey::WHOLE` bytes).
    #[inline]
    fn holds(&self, key: IdentKey, text: &str) -> bool {
        self.holds_key(key) && Name::from_id(self.id).as_str() == text
    }

    /// True when this entry has the key `key`: it holds the text of `key`
    /// when the text has at most `IdentKey::WHOLE` bytes.
    #[inline]
    fn holds_key(&self, key: IdentKey) -> bool {
        self.a == key.a
            && self.b == key.b
            && self.c == key.c
            && self.len_keyword & !IDENT_KEYWORD == key.len
    }

    #[inline]
    fn value(&self) -> (Name, bool) {
        (
            Name::from_id(self.id),
            self.len_keyword & IDENT_KEYWORD != 0,
        )
    }

    fn key(&self) -> IdentKey {
        IdentKey {
            a: self.a,
            b: self.b,
            c: self.c,
            len: self.len_keyword & !IDENT_KEYWORD,
        }
    }
}

/// One `IdentNames` set: 2 entries, one cache line, the first entry the
/// most recently used.
#[derive(Clone, Copy, Default)]
#[repr(align(64))]
struct IdentSet([IdentEntry; 2]);

/// `identifier_name`'s cache: a 2-way set-associative table of identifier
/// texts. It starts at `IDENT_FIRST_SETS` sets and doubles each time it has
/// taken as many texts as it has entries, up to `IDENT_MAX_SETS` (2 MiB, 64K
/// texts). A text that comes into a full set drops the less recently used
/// entry, so a language server that parses for days keeps a bounded table.
/// The default table is empty: it finds nothing, and its first insert
/// makes the sets.
#[derive(Default)]
struct IdentNames {
    sets: Vec<IdentSet>,
    /// `64 - log2(sets.len())`: `set_hash >> shift` is the set index (0,
    /// and no set, before the first insert).
    shift: u32,
    /// Texts put in since the table last grew.
    inserts: usize,
}

/// `IdentNames` sets of a new table (32 KiB).
const IDENT_FIRST_SETS: usize = 1 << 9;
/// The most `IdentNames` sets: 2 MiB per parsing thread on 64-bit (32,768
/// sets of 64 bytes, two 32-byte entries each, so 65,536 texts). A full
/// table does not grow or clear: a new text drops an entry of its set.
const IDENT_MAX_SETS: usize = 1 << 15;

impl IdentNames {
    /// The name and keyword bit of `text` (key `key`) when the table has it.
    /// For a text over `IdentKey::WHOLE` bytes: a shorter one uses
    /// `find_whole`, which compares no text.
    #[inline]
    fn find_long(&mut self, key: IdentKey, text: &str) -> Option<(Name, bool)> {
        let set = self.sets.get_mut((key.set_hash() >> self.shift) as usize)?;
        let [first, second] = &mut set.0;
        if first.holds(key, text) {
            return Some(first.value());
        }
        if second.holds(key, text) {
            std::mem::swap(first, second);
            return Some(first.value());
        }
        None
    }

    /// `find_long` of a text of at most `IdentKey::WHOLE` bytes, whose key is
    /// the whole text: no text compare.
    #[inline]
    fn find_whole(&mut self, key: IdentKey) -> Option<(Name, bool)> {
        debug_assert!(key.len <= IdentKey::WHOLE);
        let set = self.sets.get_mut((key.set_hash() >> self.shift) as usize)?;
        let [first, second] = &mut set.0;
        if first.holds_key(key) {
            return Some(first.value());
        }
        if second.holds_key(key) {
            std::mem::swap(first, second);
            return Some(first.value());
        }
        None
    }

    /// Puts the text of key `key` in, with name id `id` and keyword bit
    /// `keyword`, as the most recently used entry of its set.
    fn insert(&mut self, key: IdentKey, id: u32, keyword: bool) {
        if id == 0 {
            return;
        }
        if self.inserts >= 2 * self.sets.len() && self.sets.len() < IDENT_MAX_SETS {
            self.grow();
        }
        self.inserts += 1;
        let entry = IdentEntry {
            a: key.a,
            b: key.b,
            c: key.c,
            len_keyword: key.len | if keyword { IDENT_KEYWORD } else { 0 },
            id,
        };
        let set = &mut self.sets[(key.set_hash() >> self.shift) as usize].0;
        set[1] = set[0];
        set[0] = entry;
    }

    /// Doubles the sets (the first table: `IDENT_FIRST_SETS`) and puts the
    /// entries back, the less recently used first.
    #[cold]
    #[inline(never)]
    fn grow(&mut self) {
        let len = (2 * self.sets.len()).max(IDENT_FIRST_SETS);
        let old = std::mem::replace(&mut self.sets, vec![IdentSet::default(); len]);
        self.shift = 64 - len.trailing_zeros();
        self.inserts = 0;
        for way in [1, 0] {
            for set in &old {
                let entry = set.0[way];
                if entry.id != 0 {
                    let new = &mut self.sets[(entry.key().set_hash() >> self.shift) as usize].0;
                    new[1] = new[0];
                    new[0] = entry;
                }
            }
        }
    }
}

thread_local! {
    /// The `identifier_name` cache of this thread while no store of the
    /// thread holds it (`FileStore::idents`).
    static IDENTIFIER_NAMES: Cell<Option<Box<IdentNames>>> = const { Cell::new(None) };
}

/// The identifier name cache of this thread, for a store that parses (an
/// empty one when another store of the thread has it, or after the
/// thread-local destructors ran: `try_with` fails, as in
/// `Name::intern_hashed`, where `LocalKey::take` would panic).
fn take_thread_idents() -> Box<IdentNames> {
    IDENTIFIER_NAMES
        .try_with(Cell::take)
        .ok()
        .flatten()
        .unwrap_or_default()
}

/// Gives the identifier name cache `idents` of a store whose parse ended,
/// or that dropped, back to this thread. When the thread has one already
/// (two stores of the thread parsed at once), the larger stays.
fn give_back_thread_idents(idents: Box<IdentNames>) {
    let _ = IDENTIFIER_NAMES.try_with(|spare| {
        let keep = match spare.take() {
            Some(other) if other.sets.len() > idents.sets.len() => other,
            _ => idents,
        };
        spare.set(Some(keep));
    });
}

/// The identifier name cache that a store took from its thread
/// (`FileStore::idents`). It goes back to the thread of the drop when it
/// drops: at `end_parse`, or with a store whose parse never ended (a parse
/// that panicked, whose detached store a parse worker drops).
#[derive(Default)]
struct StoreIdents(Option<Box<IdentNames>>);

impl Drop for StoreIdents {
    fn drop(&mut self) {
        if let Some(idents) = self.0.take() {
            give_back_thread_idents(idents);
        }
    }
}

/// `FileStore::unlink_children` on the link column `links`.
fn unlink_children(links: &mut [SlotLinks], index: usize) {
    let mut child = std::mem::replace(&mut links[index].first_child, LINK_NONE);
    if child == LINK_NONE {
        return;
    }
    while child != LINK_END {
        let entry = &mut links[child as usize];
        child = std::mem::replace(&mut entry.next_sibling, LINK_NONE);
        debug_assert_ne!(child, LINK_NONE, "R2-5 chain without an end");
    }
}

/// `FileStore::link_child` on the link column `links`.
#[inline]
fn link_child(links: &mut [SlotLinks], parent: usize, last: &mut u32, child: usize) -> bool {
    if links[child].next_sibling != LINK_NONE {
        return false;
    }
    links[child].next_sibling = LINK_END;
    let child = child as u32;
    if *last == LINK_END {
        links[parent].first_child = child;
    } else {
        links[*last as usize].next_sibling = child;
    }
    *last = child;
    true
}

// ──────────────────────────────────────────────────────────────────────
// Owned nodes of a freeable parse (lsshells M3c)
// ──────────────────────────────────────────────────────────────────────

thread_local! {
    /// Set while a freeable parse with owned nodes runs on this thread
    /// (`enter_freeable_parse`, `owned_nodes_enabled`): `new_file_store`
    /// makes a store that owns its astdata nodes.
    static FREEABLE_PARSE: Cell<bool> = const { Cell::new(false) };
    /// Set while any freeable parse runs on this thread
    /// (`is_freeable_parse`).
    static FREEABLE_VERSION_PARSE: Cell<bool> = const { Cell::new(false) };
}

/// Starts a freeable parse on this thread (`is_freeable_parse`). With
/// owned nodes on (`owned_nodes_enabled`) the stores that
/// `new_file_store` makes until the scope ends own their astdata nodes,
/// pending lists and parse lists (`OwnedAst`), so they are freed with the
/// store, not leaked in the AST arena. The language server parse cache and
/// the compiler host (`tsc --watch`, `goport_multiprog`) open it for a new
/// version of a published path (`ast::freeable_path`), the version that
/// gets a `FileVersion`.
// PORT: Go nodes are heap objects that the GC frees with their file. A
// static parse keeps its nodes in the leaked AST arena, so its node reads
// stay `'static`.
#[must_use]
pub fn enter_freeable_parse() -> FreeableParseScope {
    FreeableParseScope {
        previous: FREEABLE_PARSE.replace(owned_nodes_enabled()),
        previous_version: FREEABLE_VERSION_PARSE.replace(true),
    }
}

/// True when a freeable parse owns its nodes (lsshells M3c). Off, it is a
/// static parse (its nodes stay in the leaked AST arena, and its node shell
/// has a node column), as before M3c. `GOPORT_OWNED_NODES` is read once:
/// `0` is off, `1` is on; else it is on in a language server, API or
/// `tsc --watch` process only (`ast::frees_file_versions_by_default`),
/// where the freeable versions are. Another CLI process makes freeable
/// parses only with `GOPORT_FREE_FILE_VERSIONS=1` (`goport_multiprog`), and
/// they stay static there by default.
// PERF: on by default since lsshells M3g (root decision
// m3-owned-default-2026-09-29). Only with it on do 1000-edit sessions pass
// memory (M3f, mini-abf9: effect 0.46 MiB/edit and +480 MiB, off 1.23 and
// +1216, limits 1.02 and 686). It costs edit time: every node data and list
// read of the edited file is a pinned read (about 150,000 per edit on
// effect). M3g against R139 (pin B): session instructions +6.40% on effect
// and +4.68% on query-core with it on, +2.2% with it off; edit median +0.5
// to +2.1 ms on effect and +0.8 to +1.7 ms on query-core in 200-edit
// sessions, +0.9 ms (query-core) and +1.0 to +1.2 ms (effect) in 1000-edit
// sessions. The CLI costs (instructions: query +1.38%, hono +0.48%, zod
// +0.18%, effect +0.19%) come from the M3 merge, not from this switch. The
// planned fix of the read cost is the AST node records plan
// (`target/continuation-r97-goport/ast-design/study.md`).
fn owned_nodes_enabled() -> bool {
    static FLAG: OnceLock<Option<bool>> = OnceLock::new();
    let flag = *FLAG.get_or_init(|| match std::env::var("GOPORT_OWNED_NODES").as_deref() {
        Ok("0") => Some(false),
        Ok("1") => Some(true),
        _ => None,
    });
    flag.unwrap_or_else(crate::ast::frees_file_versions_by_default)
}

/// `enter_freeable_parse` with owned nodes on, whatever the flag, for the
/// unit tests of the owned stores. Not `cfg(test)`: the api encoder tests in
/// ts_goport use it too, and `cfg(test)` is per crate.
#[doc(hidden)]
pub fn enter_owned_parse() -> FreeableParseScope {
    FreeableParseScope {
        previous: FREEABLE_PARSE.replace(true),
        previous_version: FREEABLE_VERSION_PARSE.replace(true),
    }
}

/// The scope of `enter_freeable_parse`.
pub struct FreeableParseScope {
    previous: bool,
    previous_version: bool,
}

impl Drop for FreeableParseScope {
    fn drop(&mut self) {
        FREEABLE_PARSE.set(self.previous);
        FREEABLE_VERSION_PARSE.set(self.previous_version);
    }
}

/// True while a freeable parse runs on this thread (`enter_freeable_parse`),
/// with or without owned nodes: the parse of a new version of an edited
/// file, which leaks nothing per version (`parse_source_file`).
#[must_use]
pub fn is_freeable_parse() -> bool {
    FREEABLE_VERSION_PARSE.get()
}

/// Cells per chunk of `OwnedAst::chunks`.
// PERF: 256 cells of 16 bytes (`BoxedData`, astmem1 P1) is a 4 KiB chunk,
// one small size class of jemalloc.
const OWNED_CHUNK: usize = 256;

/// `OwnedAst::cell_of` of a slot that has no owned node: the nil slot, an
/// alias slot, or a node slot with a static node (a shared name node,
/// `alloc_store_shared_name_node`, or a lib snapshot slot).
const NO_CELL: u32 = u32::MAX;

/// A sealed chunk of `OwnedAst::chunks` (up to `OWNED_CHUNK` cells), or
/// the flat table of every cell after the parse (`OwnedAst::flat`). A held
/// read shares it (`HeldStoreNode::Owned`).
pub type OwnedChunk = Arc<Vec<BoxedData>>;

/// The number of node datas that the live freeable stores own
/// (`OwnedAst`), for tests.
static OWNED_NODES: AtomicUsize = AtomicUsize::new(0);

/// The number of node datas that the live stores of freeable parses own
/// (lsshells M3c). A freeable file version frees its nodes when it dies, so
/// this does not grow with the edits. Tests use it.
#[must_use]
pub fn owned_node_count() -> usize {
    OWNED_NODES.load(Ordering::Relaxed)
}

/// The node datas, pending lists and parse lists of a freeable parse
/// (lsshells M3c, `enter_freeable_parse`), owned by its store and so, after
/// the publish, by its `FileVersion`: they are freed with its store, when
/// the version dies or after it on a free thread (`VersionStore`). A static
/// parse keeps them in the leaked AST arena (`AST_ARENA`).
///
/// A node slot names its data by cell (`cell_of`). Its `FileStore::nodes`
/// entry is the marker data (`owned_marker`), so the node column keeps its
/// meaning (a node slot is `Some`); the marker is never read as data (every
/// read of a slot goes through `FileStore::slot_ast_node` or
/// `FileStore::static_node_of`). A data write (`replace_store_node_data`)
/// fills a new cell and moves the slot to it, so a list handle taken before
/// the write (`StoreList`, which names the cell) still reads the old list,
/// like a Go `*NodeList` pointer.
pub(crate) struct OwnedAst {
    /// While the parser runs: the sealed chunks, whose nodes never move, so
    /// a held read can share one apart from the store borrow
    /// (`HeldStoreNode`). Cell `c` is node `c % OWNED_CHUNK` of chunk
    /// `c / OWNED_CHUNK`; the open chunk (`open`) has the chunk index
    /// `chunks.len()`.
    chunks: Vec<OwnedChunk>,
    /// While the parser runs: the open chunk. A new node fills its next
    /// cell (a plain push); a full chunk, or one that a held read needs, is
    /// sealed (`seal`), and the next node starts a new chunk.
    open: Vec<BoxedData>,
    /// After the parse (`finish`): every cell, `flat[cell]`, in one table.
    /// A cell that an early seal skipped holds `hole_data`.
    // PERF: lsshells M3f. A node read of a published version is one index
    // (after `cell_of`), not a chunk and a cell.
    flat: OwnedChunk,
    /// The number of nodes in the chunks.
    len: usize,
    /// The part of `len` that `OWNED_NODES` counts (`count_owned_nodes`).
    counted: usize,
    /// The cell of each slot, `NO_CELL` for a slot with no owned node. One
    /// entry per header.
    cell_of: Vec<u32>,
    /// The list table of the owned node data (astshrink1, as
    /// `FileStore::lists` for static data): the lists of the parse, which a
    /// list field of owned node data names by index (`StoreList` with
    /// `PENDING_SEL`).
    pending: Vec<OwnedPending>,
    /// Go `file.jsdocCache` before the publish: the entries of
    /// `set_file_store_js_doc_cache` and of `resolve_file_store_js_doc`. A
    /// static parse leaks them (`FileStore::jsdoc_cache`).
    jsdoc: FxHashMap<Node, Box<[Node]>>,
    /// The parse diagnostics before the publish
    /// (`set_file_store_parse_fields`). A static parse leaks them
    /// (`FileStore::diagnostics`).
    diagnostics: Box<[Diagnostic]>,
}

impl OwnedAst {
    /// The owned part of a new store with room for `slots` slots, with the
    /// entry of the nil slot.
    fn new(slots: usize) -> Self {
        let mut cell_of = Vec::with_capacity(slots);
        cell_of.push(NO_CELL);
        Self {
            chunks: Vec::new(),
            open: Vec::new(),
            flat: Arc::default(),
            len: 0,
            counted: 0,
            cell_of,
            // astshrink1: entry `NIL_LIST_INDEX` is the Go `nil` of a
            // required list field, as in `FileStore::lists`.
            pending: vec![OwnedPending {
                range: nil_static_list().range,
                nodes: Box::default(),
                has_trailing_comma: false,
                flags: None,
            }],
            jsdoc: FxHashMap::default(),
            diagnostics: Box::default(),
        }
    }

    /// The node data in cell `cell`.
    #[inline(always)]
    fn node(&self, cell: u32) -> NodeData<'_> {
        match self.flat.get(cell as usize) {
            Some(node) => node.view(),
            None => self.build_node(cell as usize),
        }
    }

    /// `node` while the parser runs.
    #[inline]
    fn build_node(&self, cell: usize) -> NodeData<'_> {
        match self.chunks.get(cell / OWNED_CHUNK) {
            Some(chunk) => chunk[cell % OWNED_CHUNK].view(),
            None => self.open[cell % OWNED_CHUNK].view(),
        }
    }

    /// The node in cell `cell`, held apart from the store borrow. A cell of
    /// the open chunk seals it first (`&mut`). `None` for a cell of the
    /// open chunk when the caller has only a shared borrow.
    fn held(&self, cell: u32) -> Option<HeldStoreNode> {
        let cell = cell as usize;
        if cell < self.flat.len() {
            return Some(HeldStoreNode::Owned {
                chunk: Arc::clone(&self.flat),
                index: cell,
            });
        }
        let chunk = self.chunks.get(cell / OWNED_CHUNK)?;
        Some(HeldStoreNode::Owned {
            chunk: Arc::clone(chunk),
            index: cell % OWNED_CHUNK,
        })
    }

    /// `held`, which seals the open chunk when it has the cell.
    fn held_mut(&mut self, cell: u32) -> HeldStoreNode {
        if self.flat.is_empty() && cell as usize / OWNED_CHUNK == self.chunks.len() {
            self.seal();
        }
        self.held(cell).expect("a sealed cell")
    }

    /// Moves the open chunk to the sealed chunks.
    fn seal(&mut self) {
        let open = std::mem::replace(&mut self.open, Vec::with_capacity(OWNED_CHUNK));
        self.chunks.push(Arc::new(open));
    }

    /// Puts `node` in the next cell and returns that cell.
    // PERF: lsshells M3f. A plain push into the open chunk; M3c tested
    // `Arc::get_mut` (an atomic compare-exchange) per node.
    #[inline]
    fn push(&mut self, node: BoxedData) -> u32 {
        debug_assert!(self.flat.is_empty(), "a node pushed after the parse");
        self.len += 1;
        if self.open.len() == OWNED_CHUNK {
            self.seal();
        } else if self.open.capacity() == 0 {
            self.open.reserve_exact(OWNED_CHUNK);
        }
        self.open.push(node);
        owned_cell(self.chunks.len(), self.open.len() - 1)
    }

    /// Moves every cell into `flat` (end of the parse). A sealed chunk that
    /// a held read still shares is copied.
    fn finish(&mut self) {
        let chunks = std::mem::take(&mut self.chunks);
        let mut flat = Vec::with_capacity(chunks.len() * OWNED_CHUNK + self.open.len());
        for chunk in chunks {
            let mut cells = Arc::try_unwrap(chunk).unwrap_or_else(|chunk| (*chunk).clone());
            flat.append(&mut cells);
            // An early seal (`held_mut`) leaves the rest of its chunk unused.
            flat.resize_with(flat.len().next_multiple_of(OWNED_CHUNK), hole_data);
        }
        flat.append(&mut self.open);
        self.open = Vec::new();
        self.flat = Arc::new(flat);
    }

    /// Adds the nodes of this store that `OWNED_NODES` does not count yet.
    // PERF: once per parse (`FileStore::end_parse`), not an atomic write per
    // node.
    fn count_owned_nodes(&mut self) {
        OWNED_NODES.fetch_add(self.len - self.counted, Ordering::Relaxed);
        self.counted = self.len;
    }

    /// Adds pending list `list` and returns its handle in store `file`.
    fn push_pending(&mut self, file: usize, list: OwnedPending) -> StoreList {
        let key = u32::try_from(self.pending.len()).expect("too many pending lists");
        let handle = StoreList::new(file, key, PENDING_SEL, StoreListView::Pending(&list));
        self.pending.push(list);
        handle
    }
}

impl Drop for OwnedAst {
    fn drop(&mut self) {
        OWNED_NODES.fetch_sub(self.counted, Ordering::Relaxed);
    }
}

/// The list table that a walk over the node data of a store reads the list
/// fields from (`for_each_store_child_id`): the static table, or the owned
/// table of a store that owns its nodes.
#[derive(Clone, Copy)]
pub(crate) enum ListIds<'l> {
    Static(&'l [&'static StaticList]),
    Owned(&'l [OwnedPending]),
}

impl<'l> ListIds<'l> {
    /// The ids of list field `l`, `None` for the Go `nil` marker of a
    /// required list field (`NodeList::is_nil`).
    #[inline(always)]
    pub(crate) fn ids(self, l: crate::astdata::NodeList) -> Option<&'l [crate::astdata::NodeId]> {
        match self {
            Self::Static(lists) => {
                let list = lists[l.index()];
                (!is_nil_list_range(&list.range)).then_some(list.nodes)
            }
            Self::Owned(lists) => {
                let list = &lists[l.index()];
                (!is_nil_list_range(&list.range)).then_some(&list.nodes[..])
            }
        }
    }
}

/// The data in a cell that an early seal skipped (`OwnedAst::finish`). No
/// slot names it. Its data box has no size, so it allocates nothing.
fn hole_data() -> BoxedData {
    BoxedData::Token(Box::new(crate::astdata::TokenData))
}

/// The cell of node `index` of chunk `chunk` (`OwnedAst::chunks`).
fn owned_cell(chunk: usize, index: usize) -> u32 {
    let cell = chunk
        .checked_mul(OWNED_CHUNK)
        .and_then(|first| u32::try_from(first + index).ok())
        .expect("too many owned nodes in one store");
    assert_ne!(cell, NO_CELL, "too many owned nodes in one store");
    cell
}

/// The data that `FileStore::nodes` holds for a slot whose data a freeable
/// parse owns (`OwnedAst`). It is never read as data.
fn owned_marker() -> ColumnData {
    static MARKER: crate::astdata::TokenData = crate::astdata::TokenData;
    NodeData::Token(&MARKER)
}

/// A list of a store that owns its nodes (a freeable parse): what
/// `StaticList` is for a static store.
#[derive(Debug)]
pub struct OwnedPending {
    /// Go `list.Loc` in astdata form (`ts_range`).
    range: crate::astdata::text::TextRange,
    /// The store ids of the nodes (`store_child_id`).
    nodes: Box<[crate::astdata::NodeId]>,
    /// The astdata bit (`NodeList::stored_trailing_comma`).
    has_trailing_comma: bool,
    /// Go `ModifiersToFlags(nodes)` of a modifier list; `None` for a node
    /// list.
    flags: Option<crate::astdata::ModifierFlags>,
}

/// The selector bits of every `StoreList` (`StoreList::sel_id`): the handle
/// names a list table entry (astshrink1; it named a list field of node data
/// by its selector before).
pub(crate) const PENDING_SEL: u32 = (1 << StoreList::SEL_BITS) - 1;

/// A list of a store that owns its astdata nodes (a freeable parse,
/// lsshells M3c), as a `NodeList`, `ModifierList` or `NodeSlice` names it:
/// entry `key` of the list table of store `file` (`OwnedAst::pending`;
/// `sel` is `PENDING_SEL`). A list field of owned node data names the same
/// entry (astshrink1). It is read at each use (`with_store_list`), so it
/// keeps no borrow. 12 bytes, so the list handles stay 16 bytes.
// PERF: lsshells M3f. The handle also keeps what the hot list reads need
// and a list never changes (`sel`: the Go nil marker, the astdata
// `has_trailing_comma` bit and the length), so `NodeList::is_nil`,
// `NodeList::nodes` and `NodeSlice::len` read no store.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StoreList {
    file: u32,
    key: u32,
    /// Bits 0 to 9: `PENDING_SEL`. Bit 10: the range
    /// is the Go nil marker (`is_nil_list_range`). Bit 11: the astdata
    /// `has_trailing_comma` bit. Bits 12 to 31: the length, or `LEN_MAX`
    /// for a longer list (then read at each use).
    sel: u32,
}

impl StoreList {
    const SEL_BITS: u32 = 10;
    const NIL_BIT: u32 = 1 << 10;
    const COMMA_BIT: u32 = 1 << 11;
    const LEN_SHIFT: u32 = 12;
    const LEN_MAX: u32 = u32::MAX >> Self::LEN_SHIFT;

    /// The handle of list `view` of store `file`: list table entry `key`
    /// (`sel` is `PENDING_SEL`).
    fn new(file: usize, key: u32, sel: u32, view: StoreListView<'_>) -> Self {
        debug_assert!(sel <= PENDING_SEL);
        let mut bits = sel;
        if is_nil_list_range(&view.range()) {
            bits |= Self::NIL_BIT;
        }
        if view.has_trailing_comma() {
            bits |= Self::COMMA_BIT;
        }
        let len =
            u32::try_from(view.ids().len()).map_or(Self::LEN_MAX, |len| len.min(Self::LEN_MAX));
        Self {
            file: file as u32,
            key,
            sel: bits | (len << Self::LEN_SHIFT),
        }
    }

    /// The store id of the list.
    #[inline]
    #[must_use]
    pub fn file(self) -> usize {
        self.file as usize
    }

    /// The selector id, or `PENDING_SEL` for a pending list.
    #[inline]
    fn sel_id(self) -> u32 {
        self.sel & PENDING_SEL
    }

    /// True when the list is the Go `nil` marker (`NIL_LIST_POS`): Go
    /// `list == nil`.
    #[inline]
    #[must_use]
    pub fn is_nil_marker(self) -> bool {
        self.sel & Self::NIL_BIT != 0
    }

    /// The astdata `has_trailing_comma` bit.
    #[inline]
    #[must_use]
    pub fn stored_trailing_comma(self) -> bool {
        self.sel & Self::COMMA_BIT != 0
    }

    /// The number of nodes in the list.
    #[inline]
    #[must_use]
    pub fn len(self) -> usize {
        match self.sel >> Self::LEN_SHIFT {
            Self::LEN_MAX => self.long_len(),
            len => len as usize,
        }
    }

    /// `len` of a list of `LEN_MAX` nodes or more.
    #[cold]
    #[inline(never)]
    fn long_len(self) -> usize {
        with_store_list(self, |l| l.ids().len())
    }

    /// True when the list has no nodes.
    #[inline]
    #[must_use]
    pub fn is_empty(self) -> bool {
        self.sel >> Self::LEN_SHIFT == 0
    }

    /// A slot of a direct-mapped cache of `slots` entries (a power of two)
    /// for this handle.
    #[inline]
    #[must_use]
    pub fn hash_slot(self, slots: usize) -> usize {
        let key = (u64::from(self.file) << 32 | u64::from(self.key)) ^ u64::from(self.sel) << 20;
        (key.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 48) as usize & (slots - 1)
    }
}

/// A list of a store that owns its nodes, read in place
/// (`with_store_list`): an entry of its list table.
#[derive(Clone, Copy)]
pub enum StoreListView<'a> {
    Pending(&'a OwnedPending),
}

impl<'a> StoreListView<'a> {
    /// Go `list.Loc` in astdata form.
    #[must_use]
    pub fn range(self) -> crate::astdata::text::TextRange {
        match self {
            Self::Pending(p) => p.range,
        }
    }

    /// The store ids of the nodes.
    #[must_use]
    pub fn ids(self) -> &'a [crate::astdata::NodeId] {
        match self {
            Self::Pending(p) => &p.nodes,
        }
    }

    /// The astdata `has_trailing_comma` bit.
    #[must_use]
    pub fn has_trailing_comma(self) -> bool {
        match self {
            Self::Pending(p) => p.has_trailing_comma,
        }
    }

    /// Go `modifiers.ModifierFlags` of a modifier list. Panics on a node
    /// list.
    #[must_use]
    pub fn modifier_flags(self) -> ModifierFlags {
        match self {
            Self::Pending(p) => ModifierFlags(p.flags.expect("a pending modifier list").0),
        }
    }

    /// The address of the list, for Go pointer compares and keys
    /// (`NodeList::list_ptr`). It stays the same while the store lives.
    #[must_use]
    pub fn ptr(self) -> *const () {
        match self {
            Self::Pending(p) => std::ptr::from_ref(p).cast(),
        }
    }
}

/// `read` on list `list`, of a store that owns its nodes: a freeable file
/// version (pinned while `read` runs), an unpublished store of this thread
/// (borrowed while `read` runs, so `read` must not change that store), or
/// a static store whose freeable parse lost its version before the publish
/// (`FileStore::leak_owned_nodes`). Panics for a dead version.
#[inline]
pub fn with_store_list<R>(list: StoreList, read: impl FnOnce(StoreListView<'_>) -> R) -> R {
    let file = list.file();
    if super::file_version::is_hot(file) {
        return super::file_version::with_hot(|version| read(version.store.list_view(list)));
    }
    let mut read = Some(read);
    if let Some(result) = with_version_store(file, |version| {
        (read.take().expect("the list is read once"))(version.store.list_view(list))
    }) {
        return result;
    }
    let read = read.take().expect("the list is read once");
    with_store(file, |s| read(s.list_view(list)))
}

/// Go pointer equality of two lists of stores that own their nodes: the
/// same list table entry.
#[must_use]
pub fn same_store_list(a: StoreList, b: StoreList) -> bool {
    a == b
}

/// A new list of unpublished static store `file`: `range`, no nodes, and
/// the astdata missing-list bit (parser `createMissingList`,
/// `NodeList::with_missing_marker`).
#[must_use]
pub fn new_static_missing_list(
    file: usize,
    range: crate::astdata::text::TextRange,
) -> &'static StaticList {
    let arena: &'static LeakArena = AST_ARENA.with(|a| *a);
    let bits = StaticList::bits(true, Default::default());
    with_store_mut(file, |s| {
        push_static_list(&mut s.lists, arena, range, &[], bits)
    })
}

/// The `NodeList` handle of list field `l` of the node data of store node
/// `n`, of any tier: the static table of a published static store
/// (`BlockFile::lists`), else the table of the store that holds `n` (a
/// freeable file version, or an unpublished store), by the tier of its data
/// (`FileStore::data_list`).
#[must_use]
pub fn store_data_list(n: Node, l: crate::astdata::NodeList) -> NodeList {
    let file = n.file_index();
    if let Some(block) = file_block(file)
        && block.node_column().is_some()
    {
        return NodeList::from_static(file, block.file.lists[l.index()]);
    }
    store_data_list_scoped(n, l)
}

/// `store_data_list` for a node with no static block.
#[cold]
#[inline(never)]
fn store_data_list_scoped(n: Node, l: crate::astdata::NodeList) -> NodeList {
    let (file, index) = (n.file_index(), slot_index(n));
    if super::file_version::is_hot(file) {
        return super::file_version::with_hot(|version| version.store.data_list(file, index, l));
    }
    if let Some(list) = with_version_store(file, |version| version.store.data_list(file, index, l))
    {
        return list;
    }
    match unpublished_store(file) {
        Some(store) => store.borrow().data_list(file, index, l),
        None => panic!("node {n:?} is not synthetic and has no store"),
    }
}

/// A new pending list of unpublished store `file`, which owns its nodes:
/// `range`, no nodes, and the astdata `has_trailing_comma` bit (parser
/// `createMissingList`, `NodeList::with_missing_marker`).
#[must_use]
pub fn new_store_missing_list(file: usize, range: crate::astdata::text::TextRange) -> StoreList {
    with_store_mut(file, |s| {
        s.owned_mut().push_pending(
            file,
            OwnedPending {
                range,
                nodes: Box::default(),
                has_trailing_comma: true,
                flags: None,
            },
        )
    })
}

/// The node data of a store slot, held apart from the store, so the
/// reader can make and change nodes of that store: a static node, an
/// owned cell with its chunk, or a node of a freeable file version with
/// its pin.
pub enum HeldStoreNode {
    Static(NodeData<'static>),
    Owned {
        chunk: OwnedChunk,
        index: usize,
    },
    Pinned {
        version: super::file_version::VersionPin,
        slot: usize,
    },
}

impl HeldStoreNode {
    /// The held node data.
    #[inline(always)]
    #[must_use]
    pub fn data(&self) -> NodeData<'_> {
        match self {
            Self::Static(node) => *node,
            Self::Owned { chunk, index } => chunk[*index].view(),
            Self::Pinned { version, slot } => version
                .published()
                .expect("a pinned file version is published")
                .store
                .slot_ast_node(*slot),
        }
    }
}

/// What `static_store_node` found for a store node.
pub enum StaticNode {
    /// The node has `'static` data.
    Static(NodeData<'static>),
    /// The node has no `'static` data: a node of a freeable file version or
    /// an owned node of an unpublished store. Read it with
    /// `with_scoped_store_node` (lsshells M3c).
    Scoped,
    /// No store of this thread or of the registry has the node.
    NoStore,
}

/// The `'static` node data of non-nil store node `n`, or what keeps it
/// from one (`StaticNode`). The caller already missed the static tiers
/// (`frozen_store_ast_node`). Panics on a nil or alias slot.
// PERF: query Q8. The active store is checked first and inline, as in
// `try_store_ast_node`: the parse reads its own nodes here.
#[inline]
#[must_use]
pub fn static_store_node(n: Node) -> StaticNode {
    let (file, index) = (n.file_index(), slot_index(n));
    // A published store is never a build store. The static tiers missed,
    // so the hot version has no node column: it owns its nodes.
    if super::file_version::is_hot(file) {
        return StaticNode::Scoped;
    }
    if let Some(store) = active_store(file) {
        return store.borrow().static_node_of(index);
    }
    static_store_node_slow(file, index)
}

/// `static_store_node` for a node that is not in the active store.
// PERF: lsshells M3c. Inline: its callers are out of line already
// (`static_ast_node_slow`), and a node data read of the edited file comes
// here, so one call fewer per read.
#[inline]
fn static_store_node_slow(file: usize, index: usize) -> StaticNode {
    // A node shell has no node column (lsshells M3c), and a freeable file
    // version owns its nodes: the scoped read reads them.
    if let Some(b) = file_block(file) {
        return match b.node_column() {
            Some(nodes) => StaticNode::Static(slot_node(nodes[index])),
            None => StaticNode::Scoped,
        };
    }
    match unpublished_store(file) {
        Some(store) => store.borrow().static_node_of(index),
        None => StaticNode::NoStore,
    }
}

/// `read` on the node data of store node `n` when it has no `'static`
/// node (`StaticNode::Scoped`): a node of a freeable file version, pinned
/// while `read` runs, or an owned node of an unpublished store of this
/// thread, borrowed while `read` runs. `read` must not change the store of
/// `n` (a field read); `held_store_node` gives a node that a reader that
/// makes nodes can hold. Panics for a node with no store and for a dead
/// version.
// PERF: lsshells M3c. A pin hit is a thread-local borrow and a short scan
// (`with_file_version`), with no atomic write, as the binder data reads of
// the edited file (`Node::bind_field_slow`).
pub fn with_scoped_store_node<R>(n: Node, read: impl FnOnce(NodeData<'_>) -> R) -> R {
    let (file, index) = (n.file_index(), slot_index(n));
    if super::file_version::is_hot(file) {
        return super::file_version::with_hot(|version| read(version.store.slot_ast_node(index)));
    }
    let mut read = Some(read);
    if let Some(result) = with_version_store(file, |version| {
        (read.take().expect("the node is read once"))(version.store.slot_ast_node(index))
    }) {
        return result;
    }
    let read = read.take().expect("the node is read once");
    match unpublished_store(file) {
        Some(store) => read(store.borrow().slot_ast_node(index)),
        None => panic!("node {n:?} is not synthetic and has no store"),
    }
}

/// `read_ast_node_miss` for store node `n`, which is not a node of a static
/// tier: an unpublished store node (read in place; a node that the store
/// owns with the store borrowed while `read` runs), or a node of a live
/// freeable file version, pinned while `read` runs. Panics for a node with
/// no store and for a dead version.
// PERF: lsshells M3c. The active store first (the parse reads its own
// nodes, as `static_store_node`), then the version store: after the publish
// nearly every miss of a store node is a node of the edited file.
#[inline]
pub fn read_store_node_miss<R>(n: Node, read: impl FnOnce(NodeData<'_>) -> R) -> R {
    let (file, index) = (n.file_index(), slot_index(n));
    if super::file_version::is_hot(file) {
        return super::file_version::with_hot(|version| read(version.store.slot_ast_node(index)));
    }
    if let Some(store) = active_store(file) {
        let s = store.borrow();
        if s.cell_of(index) == NO_CELL {
            // A static node: no borrow while `read` runs, as before M3c.
            let node = slot_node(s.nodes[index]);
            drop(s);
            return read(node);
        }
        return read(s.slot_ast_node(index));
    }
    let mut read = Some(read);
    if let Some(result) = with_version_store(file, |version| {
        (read.take().expect("the node is read once"))(version.store.slot_ast_node(index))
    }) {
        return result;
    }
    let read = read.take().expect("the node is read once");
    match static_store_node_slow(file, index) {
        StaticNode::Static(node) => read(node),
        StaticNode::Scoped => match unpublished_store(file) {
            Some(store) => read(store.borrow().slot_ast_node(index)),
            None => panic!("node {n:?} is not synthetic and has no store"),
        },
        StaticNode::NoStore => panic!("node {n:?} is not synthetic and has no store"),
    }
}

/// True when `file` is the hot file version of this thread
/// (`ast::file_version::is_hot`, lsshells M3f).
#[inline(always)]
#[must_use]
pub fn is_hot_file(file: usize) -> bool {
    super::file_version::is_hot(file)
}

/// `read` on the `GoFile` of the hot file version (`is_hot_file`). `read`
/// runs inside the thread-local borrow: keep it small.
#[inline(always)]
pub fn with_hot_go_file<R>(read: impl FnOnce(&GoFile) -> R) -> R {
    super::file_version::with_hot(|version| read(&version.go_file))
}

/// True when store node `n` is a node of the hot file version of this
/// thread (`ast::file_version::is_hot`, lsshells M3f).
#[inline(always)]
#[must_use]
pub fn is_hot_store_node(n: Node) -> bool {
    super::file_version::is_hot(n.file_index())
}

/// `read` on the node data of node `n` of the hot file version
/// (`is_hot_store_node`). `read` can make nodes (of other stores: a
/// published store takes no new node).
#[inline(always)]
pub fn with_hot_store_node<R>(n: Node, read: impl FnOnce(NodeData<'_>) -> R) -> R {
    let index = slot_index(n);
    super::file_version::with_hot(|version| read(version.store.slot_ast_node(index)))
}

/// The node data of store node `n` held apart from its store
/// (`HeldStoreNode`), so the reader can make and change nodes of that
/// store. For a node with no `'static` node, as `with_scoped_store_node`.
#[must_use]
pub fn held_store_node(n: Node) -> HeldStoreNode {
    let (file, index) = (n.file_index(), slot_index(n));
    if let Some(version) = published_version(file) {
        return HeldStoreNode::Pinned {
            version,
            slot: index,
        };
    }
    // A held read inside a scoped read of the same store (a shared borrow)
    // cannot seal the open chunk, so it clones the node.
    match unpublished_store(file) {
        Some(store) => match store.try_borrow_mut() {
            Ok(mut s) => s.held_slot_node_mut(index),
            Err(_) => store.borrow().held_slot_node(index),
        },
        None => panic!("node {n:?} is not synthetic and has no store"),
    }
}

/// A list field that a list selector found in the data of a store node
/// with no `'static` node (`scoped_store_list_of`).
#[derive(Clone, Copy)]
pub enum ScopedList {
    /// The node is static after all (a node of an unpublished static store,
    /// or a lib snapshot node in a store that owns its nodes): a list of the
    /// static table.
    Static(&'static StaticList),
    /// A list that the store owns.
    Store(StoreList),
}

/// Entries of `HOT_LISTS`.
const HOT_LIST_SLOTS: usize = 256;

/// One entry of `HOT_LISTS`: a node, a selector id and what
/// `scoped_store_list_of` gave for them.
#[derive(Clone, Copy)]
struct HotListEntry {
    node: Node,
    sel_id: u32,
    found: Option<Option<ScopedList>>,
}

thread_local! {
    /// The last `scoped_store_list_of` answers for nodes of hot file
    /// versions (`file_version::is_hot`), by node and selector id, direct
    /// mapped (lsshells M3f). A published version never changes and a file
    /// id is never reused, so an entry stays true; a node of a dead
    /// version is never hot, so its entries are not read.
    // PERF: the checker asks for the same lists of the edited file many
    // times (parameters, type parameters, members).
    static HOT_LISTS: RefCell<[HotListEntry; HOT_LIST_SLOTS]> = const {
        RefCell::new(
            [HotListEntry {
                node: Node::NIL,
                sel_id: 0,
                found: None,
            }; HOT_LIST_SLOTS],
        )
    };
}

/// `scoped_store_list_of` for node `n` of the hot version: the entry of
/// `HOT_LISTS`, or `pick` (the read), whose answer becomes the entry.
#[inline]
fn hot_list_of(
    n: Node,
    sel_id: u32,
    pick: impl FnOnce() -> Option<Option<ScopedList>>,
) -> Option<Option<ScopedList>> {
    let slot = ((n.0 ^ (u64::from(sel_id) << 44)).wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 56)
        as usize
        % HOT_LIST_SLOTS;
    let hit = HOT_LISTS
        .try_with(|lists| {
            let entry = lists.borrow()[slot];
            (entry.node == n && entry.sel_id == sel_id).then_some(entry.found)
        })
        .ok()
        .flatten();
    if let Some(found) = hit {
        return found;
    }
    let found = pick();
    let _ = HOT_LISTS.try_with(|lists| {
        if let Ok(mut lists) = lists.try_borrow_mut() {
            lists[slot] = HotListEntry {
                node: n,
                sel_id,
                found,
            };
        }
    });
    found
}

/// The list field that `sel` finds in the data of store node `n`, which
/// has no `'static` node (see `with_scoped_store_node`): `None` when the
/// data has no such field, `Some(None)` when the field is Go `nil`.
/// `sel_id` is the id of `sel` (`SelectorSite::id`).
// PERF: lsshells M3c. Inline in its two callers (`scoped_node_list_of`,
// `scoped_modifiers_of`), which are out of line.
#[inline]
#[must_use]
pub fn scoped_store_list_of(n: Node, sel_id: u32, sel: ListSel) -> Option<Option<ScopedList>> {
    if super::file_version::is_hot(n.file_index()) {
        let index = slot_index(n);
        return hot_list_of(n, sel_id, || {
            super::file_version::with_hot(|version| pick_store_list(&version.store, n, index, sel))
        });
    }
    scoped_store_list_cold(n, sel)
}

/// The list field that `sel` finds in the data of
/// slot `index` (node `n`) of store `s` (`scoped_store_list_of`).
#[inline(always)]
fn pick_store_list(
    s: &FileStore,
    n: Node,
    index: usize,
    sel: ListSel,
) -> Option<Option<ScopedList>> {
    s.pick_list(n.file_index(), index, sel)
}

/// `scoped_store_list_of` for a node that is not of the hot file version.
#[cold]
#[inline(never)]
fn scoped_store_list_cold(n: Node, sel: ListSel) -> Option<Option<ScopedList>> {
    let (file, index) = (n.file_index(), slot_index(n));
    let pick = |s: &FileStore| s.pick_list(file, index, sel);
    if let Some(found) = with_version_store(file, |version| pick(&version.store)) {
        return found;
    }
    match unpublished_store(file) {
        Some(store) => pick(&store.borrow()),
        None => panic!("node {n:?} is not synthetic and has no store"),
    }
}

impl FileStore {
    /// The list field that `sel` finds in the data of slot `index` of this
    /// store, store `file` (`scoped_store_list_of`).
    #[inline(always)]
    fn pick_list(&self, file: usize, index: usize, sel: ListSel) -> Option<Option<ScopedList>> {
        match self.cell_of(index) {
            NO_CELL => {
                let data: NodeData<'static> = slot_node(self.nodes[index]);
                sel(data).map(|found| found.map(|l| ScopedList::Static(self.lists[l.index()])))
            }
            cell => sel(self.owned_ref().node(cell))
                .map(|found| found.map(|l| ScopedList::Store(self.owned_list(file, l.index())))),
        }
    }

    /// The owned cell of slot `index`, `NO_CELL` for a static store.
    #[inline]
    fn cell_of(&self, index: usize) -> u32 {
        self.owned
            .as_deref()
            .map_or(NO_CELL, |owned| owned.cell_of[index])
    }

    /// The owned part of a store that owns its nodes. Panics for a static
    /// store.
    fn owned_ref(&self) -> &OwnedAst {
        self.owned
            .as_deref()
            .expect("a store that owns its nodes (a freeable parse)")
    }

    /// `owned_ref` for a write.
    fn owned_mut(&mut self) -> &mut OwnedAst {
        self.owned
            .as_deref_mut()
            .expect("a store that owns its nodes (a freeable parse)")
    }

    /// The node data of node slot `index`: the data that the store owns or
    /// its static data. Panics on the nil slot and alias slots.
    #[inline(always)]
    fn slot_ast_node(&self, index: usize) -> NodeData<'_> {
        match self.cell_of(index) {
            NO_CELL => slot_node(self.nodes[index]),
            cell => self.owned_ref().node(cell),
        }
    }

    /// `slot_ast_node` as `StaticNode`: `Scoped` for a node that the store
    /// owns.
    #[inline]
    fn static_node_of(&self, index: usize) -> StaticNode {
        match self.cell_of(index) {
            NO_CELL => StaticNode::Static(slot_node(self.nodes[index])),
            _ => StaticNode::Scoped,
        }
    }

    /// `slot_ast_node`, held apart from the store borrow. A node of the
    /// open chunk is cloned: this borrow is shared (see `held_store_node`).
    fn held_slot_node(&self, index: usize) -> HeldStoreNode {
        match self.cell_of(index) {
            NO_CELL => HeldStoreNode::Static(slot_node(self.nodes[index])),
            cell => {
                let owned = self.owned_ref();
                owned.held(cell).unwrap_or_else(|| HeldStoreNode::Owned {
                    chunk: Arc::new(vec![owned.node(cell).to_boxed()]),
                    index: 0,
                })
            }
        }
    }

    /// `held_slot_node` with the store borrowed for a write: a node of the
    /// open chunk seals it.
    fn held_slot_node_mut(&mut self, index: usize) -> HeldStoreNode {
        match self.cell_of(index) {
            NO_CELL => HeldStoreNode::Static(slot_node(self.nodes[index])),
            cell => self.owned_mut().held_mut(cell),
        }
    }

    /// The node data of each slot, `None` for the nil slot and alias
    /// slots (`slot_ast_node`).
    fn slot_nodes(&self) -> impl Iterator<Item = Option<NodeData<'_>>> + '_ {
        (0..self.nodes.len()).map(|i| self.nodes[i].map(|_| self.slot_ast_node(i)))
    }

    /// The list that `list` names in this store (`with_store_list`).
    #[inline]
    fn list_view(&self, list: StoreList) -> StoreListView<'_> {
        debug_assert_eq!(list.sel_id(), PENDING_SEL);
        StoreListView::Pending(&self.owned_ref().pending[list.key as usize])
    }

    /// The handle of entry `index` of the list table of the owned data of
    /// this store, store `file`.
    fn owned_list(&self, file: usize, index: usize) -> StoreList {
        let key = u32::try_from(index).expect("too many lists in one store");
        StoreList::new(
            file,
            key,
            PENDING_SEL,
            StoreListView::Pending(&self.owned_ref().pending[index]),
        )
    }

    /// The handle of list field `l` of the data of slot `index` of this
    /// store, store `file`: an entry of the owned table for owned data, of
    /// the static table for static data.
    fn data_list(&self, file: usize, index: usize, l: crate::astdata::NodeList) -> NodeList {
        match self.cell_of(index) {
            NO_CELL => NodeList::from_static(file, self.lists[l.index()]),
            _ => NodeList::store(self.owned_list(file, l.index())),
        }
    }

    /// Adds the `cell_of` entry of a new slot with no owned node, in a
    /// store that owns its nodes.
    #[inline]
    fn push_no_cell(&mut self) {
        if let Some(owned) = self.owned.as_deref_mut() {
            owned.cell_of.push(NO_CELL);
        }
    }

    /// A static publish of a store that owns its nodes (a freeable parse
    /// whose `FileVersion` died before the publish): its nodes are leaked
    /// and its node column names them, so the static reads find them. Its
    /// list table is leaked too, as the static table at the same indexes
    /// (astshrink1), so the list fields of the leaked data name the same
    /// lists. The owned part stays, for the list handles that name it.
    fn leak_owned_nodes(&mut self) {
        let Some(owned) = self.owned.as_deref() else {
            return;
        };
        let flat: &'static OwnedChunk = Box::leak(Box::new(Arc::clone(&owned.flat)));
        let mut any_owned = false;
        for (slot, &cell) in owned.cell_of.iter().enumerate() {
            if cell != NO_CELL {
                self.nodes[slot] = Some(flat[cell as usize].view());
                any_owned = true;
            }
        }
        if !any_owned {
            return;
        }
        // The lists of a store that owns its nodes are all in its owned
        // table (`new_store_list`). The table below replaces the static
        // table, so a static list would be lost: check it in every build
        // (one compare per publish).
        assert_eq!(
            self.lists.len(),
            1,
            "static lists in a store with owned nodes"
        );
        let arena: &'static LeakArena = AST_ARENA.with(|a| *a);
        self.lists = owned
            .pending
            .iter()
            .enumerate()
            .map(|(index, p)| {
                let ids = arena.alloc_slice_fill_iter(p.nodes.len(), p.nodes.iter().copied());
                leak_in_ast_arena(StaticList {
                    range: p.range,
                    nodes: ids,
                    index: index as u32,
                    bits: StaticList::bits(p.has_trailing_comma, p.flags.unwrap_or_default()),
                })
            })
            .collect();
    }
}

// ──────────────────────────────────────────────────────────────────────
// Detached stores (parse workers)
// ──────────────────────────────────────────────────────────────────────

/// A finished store that a parse worker made with a provisional id. Only
/// `adopt_detached_store` can make it part of the program.
pub struct DetachedStore {
    id: usize,
    store: FileStore,
}

impl DetachedStore {
    /// The provisional id of the store.
    #[must_use]
    pub fn id(&self) -> usize {
        self.id
    }

    /// True when every node the store names is its own. A store that names
    /// a node of another store (an alias slot, for example a synthetic
    /// node of the worker) cannot be adopted.
    #[must_use]
    pub fn is_self_contained(&self) -> bool {
        self.store.frozen && self.store.aliases.is_empty()
    }
}

/// Makes the detached store of this thread with provisional id
/// `DETACHED_STORE_BASE + job` and returns that id. A parse worker parses
/// one file at a time; the store stays until `take_detached_file_store`.
/// Inside a freeable parse scope the store owns its astdata nodes, as in
/// `new_file_store`, so they go with the store, whichever thread drops it.
pub fn new_detached_file_store(
    job: usize,
    file_name: &'static str,
    text: impl Into<FileText>,
) -> usize {
    assert!(job < DETACHED_STORE_LIMIT, "too many detached stores");
    let id = DETACHED_STORE_BASE + job;
    assert!(
        DETACHED.get().is_none(),
        "this thread already has a detached store"
    );
    // PERF: watchcfg1 round c. The first cell of a thread is leaked with its
    // own malloc, not in the AST arena: the first value in a thread's arena
    // leaks a 128 KiB chunk, and a watch build after a config change starts
    // new parse workers, whose owned parses put nothing else there. With
    // the arena an x1 session grew 0.24 MiB per op over 500 config edits.
    let store = SPARE_CELL
        .take()
        .unwrap_or_else(|| Box::leak(Box::default()));
    let mut new = FileStore::new(file_name, text.into());
    if FREEABLE_PARSE.get() {
        new.owned = Some(Box::new(OwnedAst::new(new.records.capacity())));
    }
    *store.borrow_mut() = new;
    DETACHED.set(Some((id, store)));
    ACTIVE.set(Some((id, store)));
    id
}

/// Removes the detached store of this thread, if any.
pub fn take_detached_file_store() -> Option<DetachedStore> {
    let (id, cell) = DETACHED.take()?;
    if active_store(id).is_some() {
        ACTIVE.set(None);
    }
    let store = cell.take();
    SPARE_CELL.set(Some(cell));
    Some(DetachedStore { id, store })
}

/// Maps the node handles of an adopted detached store to its real id.
#[derive(Clone, Copy, Debug)]
pub struct StoreRemap {
    from: usize,
    to: usize,
}

impl StoreRemap {
    /// The real store id.
    #[must_use]
    pub fn store(self) -> usize {
        self.to
    }

    /// `n` with the provisional store id replaced by the real one. Other
    /// handles (nil, other stores) do not change.
    #[inline]
    #[must_use]
    pub fn node(self, n: Node) -> Node {
        if n.is_some() && n.file_index() == self.from {
            handle(self.to, slot_index(n) as u32)
        } else {
            n
        }
    }
}

/// Gives a self-contained detached store the next real store id on this
/// thread, as `new_file_store` would have at this point, and returns the
/// handle map for the values the parse returned with it. The store becomes
/// the active store of this thread.
pub fn adopt_detached_store(detached: DetachedStore) -> StoreRemap {
    assert!(
        detached.is_self_contained(),
        "cannot adopt a store that names other stores"
    );
    let DetachedStore { id, mut store } = detached;
    let (remap, cell) = BUILD.with(|b| {
        let mut b = b.borrow_mut();
        let remap = StoreRemap {
            from: id,
            to: b.next_id(),
        };
        store.jsdoc_cache = store
            .jsdoc_cache
            .iter()
            .map(|(&node, &jsdocs)| {
                let jsdocs: &'static [Node] = if jsdocs.iter().any(|&n| remap.node(n) != n) {
                    Box::leak(jsdocs.iter().map(|&n| remap.node(n)).collect())
                } else {
                    jsdocs
                };
                (remap.node(node), jsdocs)
            })
            .collect();
        // The lazy JSDoc nodes are synthetic nodes of the parse worker.
        store.lazy_jsdoc_cache = FxHashMap::default();
        // A store that owns its nodes keeps the JSDoc cache there.
        if let Some(owned) = store.owned.as_deref_mut() {
            owned.jsdoc = std::mem::take(&mut owned.jsdoc)
                .into_iter()
                .map(|(node, jsdocs)| {
                    let jsdocs = jsdocs.iter().map(|&n| remap.node(n)).collect();
                    (remap.node(node), jsdocs)
                })
                .collect();
        }
        // The records, kids and R2-5 links are slot-indexed and hold no
        // store id (a parent in the store is a `LOCAL_STORE` handle, and a
        // self-contained store has no alias slot), so they stay.
        let cell: StoreCell = build_store_cell(store);
        b.stores.push(cell);
        (remap, cell)
    });
    ACTIVE.set(Some((remap.to, cell)));
    remap
}

// ──────────────────────────────────────────────────────────────────────
// File registry
// ──────────────────────────────────────────────────────────────────────

/// The `GoFile` of published file `file` (static or a freeable file
/// version). Panics when it is not published, and for a dead file
/// version. The guard pins a freeable version while it lives; hot readers
/// use `with_go_file`, which pins nothing past the read.
#[inline]
#[must_use]
pub fn go_file(file: usize) -> FileRef<GoFile> {
    match try_go_file(file) {
        Some(go_file) => go_file,
        None => not_published(file),
    }
}

#[cold]
#[inline(never)]
fn not_published(file: usize) -> ! {
    panic!("file {file} is not published")
}

/// `go_file`, or `None` for a store still being built, a synthetic id or
/// an unknown id.
#[inline]
#[must_use]
pub fn try_go_file(file: usize) -> Option<FileRef<GoFile>> {
    if let Some(go_file) = static_go_file(file) {
        return Some(FileRef::Static(go_file));
    }
    let version = published_version(file)?;
    Some(FileRef::Pinned {
        version,
        key: 0,
        get: |version, _| version.go_file(),
    })
}

/// The `GoFile` of file `file` when it is in a static publish (its block
/// holds it). `None` for any other file, a freeable file version included.
/// Hot readers use it inline and read any other file out of line.
#[inline]
#[must_use]
pub fn static_go_file(file: usize) -> Option<&'static GoFile> {
    file_block(file)?.file.go_file.as_ref()
}

/// Runs `f` on the `GoFile` of published file `file`, or gives `None` as
/// `try_go_file`. It pins a freeable version only while `f` runs, so it
/// costs no guard: hot readers (`Node::bind`) use it.
#[inline]
pub fn try_with_go_file<R>(file: usize, f: impl FnOnce(&GoFile) -> R) -> Option<R> {
    if let Some(go_file) = static_go_file(file) {
        return Some(f(go_file));
    }
    try_with_go_file_slow(file, f)
}

/// `try_with_go_file` after the static blocks missed: the hot file version
/// (lsshells M3f), else a pinned freeable file version.
#[cold]
#[inline(never)]
fn try_with_go_file_slow<R>(file: usize, f: impl FnOnce(&GoFile) -> R) -> Option<R> {
    with_version_store(file, |version| f(&version.go_file))
}

/// `try_with_go_file`, which panics as `go_file` when `file` is not
/// published.
#[inline]
pub fn with_go_file<R>(file: usize, f: impl FnOnce(&GoFile) -> R) -> R {
    match try_with_go_file(file, f) {
        Some(result) => result,
        None => not_published(file),
    }
}

/// True when `file` has a `GoFile` in the registry (a static publish or a
/// freeable file version).
#[inline]
#[must_use]
pub fn is_published(file: usize) -> bool {
    try_with_go_file(file, |_| ()).is_some()
}

/// Ids of the stores that this thread built and did not publish, in id
/// order. The next publish of this thread gives them their `GoFile`s.
#[must_use]
pub fn unpublished_file_ids() -> std::ops::Range<usize> {
    BUILD.with(|b| {
        let b = b.borrow();
        if b.stores.is_empty() {
            let next = PUBLISHED.load(Ordering::Acquire);
            return next..next;
        }
        b.base..b.base + b.stores.len()
    })
}

/// True when file `file` was parsed by the ported parser.
#[inline]
#[must_use]
pub fn has_file_store(file: usize) -> bool {
    file_block(file).is_some() || unpublished_store(file).is_some()
}

/// True when `n` is a node of a store file.
#[inline]
#[must_use]
pub fn is_store_node(n: Node) -> bool {
    n.is_some() && has_file_store(n.file_index())
}

/// Go `file.FileName()` of a store file.
#[must_use]
pub fn file_store_file_name(file: usize) -> &'static str {
    with_store(file, |s| s.file_name)
}

/// Go `file.Text()` of a store file.
#[must_use]
pub fn file_store_text(file: usize) -> FileText {
    with_store(file, |s| s.text.clone())
}

/// Go `result.jsdocCache = p.createJSDocCache()` in `finishSourceFile`.
// PORT: the lists are leaked so reads can return `&'static` slices, like
// `GoFile::info.jsdoc_cache` after the program is installed. A store that
// owns its nodes (a freeable parse, lsshells M3c) keeps them instead.
pub fn set_file_store_js_doc_cache(file: usize, cache: &FxHashMap<Node, Vec<Node>>) {
    with_store_mut(file, |s| match s.owned.as_deref_mut() {
        Some(owned) => {
            owned.jsdoc = cache
                .iter()
                .map(|(node, jsdocs)| (*node, jsdocs.clone().into_boxed_slice()))
                .collect();
        }
        None => {
            s.jsdoc_cache = cache
                .iter()
                .map(|(node, jsdocs)| (*node, &*Box::leak(jsdocs.clone().into_boxed_slice())))
                .collect();
        }
    });
}

/// Go `result.LanguageVariant` and `result.diagnostics` in
/// `finishSourceFile`.
// PORT: the diagnostics are leaked so reads can return a `&'static` slice,
// like `GoFile::info.diagnostics` after the publish. A parse without errors
// leaks nothing. A store that owns its nodes (a freeable parse, lsshells
// M3c) keeps them instead.
pub fn set_file_store_parse_fields(
    file: usize,
    language_variant: LanguageVariant,
    diagnostics: &[Diagnostic],
) {
    with_store_mut(file, |s| {
        s.language_variant = language_variant;
        match s.owned.as_deref_mut() {
            Some(owned) => owned.diagnostics = diagnostics.into(),
            None => s.diagnostics = Box::leak(diagnostics.to_vec().into_boxed_slice()),
        }
    });
}

/// Go `file.LanguageVariant` of a store file.
#[must_use]
pub fn file_store_language_variant(file: usize) -> LanguageVariant {
    with_store(file, |s| s.language_variant)
}

/// Go `file.Diagnostics()` (the parse diagnostics) of a store file.
// PORT: a store that owns its nodes (lsshells M3c) keeps its diagnostics,
// so this read leaks a copy. Only a read before the publish comes here
// (`source_file_diagnostics`), and the language server makes none.
#[must_use]
pub fn file_store_diagnostics(file: usize) -> &'static [Diagnostic] {
    with_store(file, |s| match s.owned.as_deref() {
        Some(owned) if !owned.diagnostics.is_empty() => &*Box::leak(owned.diagnostics.clone()),
        _ => s.diagnostics,
    })
}

/// Go `file.jsdocCache[node]` of a store file whose program is not
/// installed yet. It never parses (Go `EagerJSDoc`). The list of a store
/// that owns its nodes (lsshells M3c) is read at each use
/// (`NodeSlice::from_file_js_doc`).
#[must_use]
pub fn file_store_js_doc(file: usize, node: Node) -> Option<NodeSlice> {
    let found = with_store(file, |s| match s.owned.as_deref() {
        Some(owned) => owned.jsdoc.contains_key(&node).then_some(None),
        None => s
            .jsdoc_cache
            .get(&node)
            .or_else(|| s.lazy_jsdoc_cache.get(&node))
            .map(|&jsdocs| Some(jsdocs)),
    })?;
    // The slice of an owned list reads the store, so it is made after the
    // borrow ends.
    Some(match found {
        Some(jsdocs) => NodeSlice::from_nodes(jsdocs),
        None => NodeSlice::from_file_js_doc(file, node),
    })
}

/// `read` on the JSDoc cache entry of `node` in unpublished store `file`,
/// which owns its nodes (lsshells M3c; `file_store_js_doc`). `None` when
/// `file` is no such store or the entry is missing.
pub fn with_owned_store_js_doc<R>(
    file: usize,
    node: Node,
    read: impl FnOnce(&[Node]) -> R,
) -> Option<R> {
    try_with_store(file, |s| {
        s.owned
            .as_deref()
            .and_then(|owned| owned.jsdoc.get(&node))
            .map(|jsdocs| read(&jsdocs[..]))
    })
    .flatten()
}

/// Go `result.SetHasLazyJSDoc(true)` in `finishSourceFile`. The store keeps
/// the parse options and script kind of the file for
/// `resolve_file_store_js_doc`.
pub fn set_file_store_lazy_js_doc(
    file: usize,
    parse_options: &SourceFileParseOptions,
    script_kind: ScriptKind,
) {
    let lazy = Some((parse_options.clone(), script_kind));
    with_store_mut(file, |s| s.lazy_js_doc = lazy);
}

// Go: ast/ast.go:2672 (*SourceFile).resolveJSDoc
/// Go `node.JSDoc(file)` of a store file whose program is not installed
/// yet: the cache, then, in a lazy file (`set_file_store_lazy_js_doc`), Go
/// `parseJSDocForNode`, whose result the cache keeps. None on a cache miss
/// in a file that is not lazy.
// PORT: Go takes `jsdocMu`. A store that is not published has one thread.
// The lists are leaked so reads can return `&'static` slices; a store that
// owns its nodes keeps them (lsshells M3c).
#[must_use]
pub fn resolve_file_store_js_doc(file: usize, node: Node) -> Option<NodeSlice> {
    if let Some(jsdocs) = file_store_js_doc(file, node) {
        return Some(jsdocs);
    }
    let (parse_options, script_kind, text) = with_store(file, |s| {
        s.lazy_js_doc
            .clone()
            .map(|(parse_options, script_kind)| (parse_options, script_kind, s.text.clone()))
    })?;
    let jsdocs =
        crate::frontend::parser::parse_js_doc_for_node(&parse_options, &text, script_kind, node);
    let leaked = with_store_mut(file, |s| match s.owned.as_deref_mut() {
        Some(owned) => {
            owned.jsdoc.insert(node, jsdocs.into_boxed_slice());
            None
        }
        None => {
            let jsdocs: &'static [Node] = Box::leak(jsdocs.into_boxed_slice());
            s.lazy_jsdoc_cache.insert(node, jsdocs);
            Some(jsdocs)
        }
    });
    // The slice of an owned list reads the store, so it is made after the
    // borrow ends.
    Some(match leaked {
        Some(jsdocs) => NodeSlice::from_nodes(jsdocs),
        None => NodeSlice::from_file_js_doc(file, node),
    })
}

/// True when node reads of store file `file` must use the store, because
/// the file is not published yet (the parser is still running).
#[must_use]
pub fn is_file_store_before_program(file: usize) -> bool {
    !is_published(file) && has_file_store(file)
}

/// Ends the parse of a file. Record and data writes panic after this.
/// Records cannot change after this, so it also marks the source file roots
/// (`mark_source_file_roots`) and makes the facts that the publish puts in
/// the registry (`FileStore::facts`) on the parsing thread.
pub fn freeze_file_store(file: usize) {
    with_store_mut(file, |s| s.freeze(file));
}

impl FileStore {
    /// `freeze_file_store` for this store, which has id `file`.
    fn freeze(&mut self, file: usize) {
        self.end_parse();
        self.parser_flags = Some(self.records.iter().map(NodeRecord::flags).collect());
        self.make_facts();
    }

    /// Makes `facts` and the U1 (e) `bind_estimate` from `build_facts`.
    /// Then moves the R2-5 build vector into `links`.
    // PERF: no pass over the node data. Debug builds check the records and
    // kids against the node data (`debug_check_kids`). storefast1: no pass
    // over the records or kinds either, unless a parent in another store or
    // a deprecated tag was written (`BuildFacts`). Debug builds check the
    // result against the pass (`debug_scan_facts`).
    fn make_facts(&mut self) {
        // `build_facts` is taken, so a second pass would write the facts of
        // an empty store (R202 review).
        assert!(!self.facts_made, "make_facts ran twice on one store");
        let built = std::mem::take(&mut self.build_facts);
        // `StoreFacts::add` of every node slot after slot 0.
        let node_records = || self.records.iter().skip(1).filter(|r| r.is_node());
        let facts = StoreFacts {
            alias_free: !built.has_alias_slot,
            // A record gets a foreign parent code only with a new entry of
            // `foreign_parents` (`foreign_parent_code`).
            parents_local: self.foreign_parents.is_empty()
                || node_records().all(|r| !r.has_foreign_parent()),
            has_export_alias_kind: built.has_export_alias_kind,
            has_flow_constraint_kind: built.has_flow_constraint_kind,
            has_deprecated_tag: built.deprecated_tag_written
                && node_records().any(|r| {
                    r.flags()
                        .intersects(NodeFlags::POSSIBLY_CONTAINS_DEPRECATED_TAG)
                }),
        };
        #[cfg(debug_assertions)]
        self.debug_scan_facts(facts, built.counts.estimate());
        self.facts = facts;
        self.facts_made = true;
        self.bind_estimate = built.counts.estimate();
        #[cfg(debug_assertions)]
        self.debug_check_kids();
        // R2-5: a link written after the slot was made must have gone
        // through `replace_store_node_data` or the parser's parent writes.
        // The chains are checked against Go `ForEachChild` when the binder
        // walks them (`bind_each_child`).
        let slots = self.records.len();
        let links = std::mem::take(&mut self.build_links);
        self.links = if links.len() == slots {
            links.into_boxed_slice()
        } else {
            vec![SlotLinks::NONE; slots].into_boxed_slice()
        };
    }

    /// storefast1, debug builds: `facts` and `estimate` (from `BuildFacts`)
    /// equal the ones of one pass over the records and kinds.
    #[cfg(debug_assertions)]
    fn debug_scan_facts(&self, facts: StoreFacts, estimate: BindEstimate) {
        let mut scanned = StoreFacts::ONLY_NIL_SLOT;
        let mut counts = BindCounts::default();
        for (i, (record, &kind)) in self.records.iter().zip(&self.kinds).enumerate() {
            counts.add(kind);
            if i != NIL_SLOT as usize {
                scanned.add(record, kind);
            }
        }
        assert_eq!(
            format!("{facts:?} {estimate:?}"),
            format!("{scanned:?} {:?}", counts.estimate()),
            "storefast1: store facts kept as slots are made"
        );
    }

    /// AST node records, debug builds: checks every record and kids entry
    /// against the node data of its slot.
    /// - U4: the child fields are unknown or equal the value
    ///   `store_node_children` makes from the final node data (a child
    ///   written after the slot was made must have gone through
    ///   `replace_store_node_data`, which makes the entry again; C2: the
    ///   `typed` field too).
    /// - U1 (a) (d): an identifier slot whose data has a text has that
    ///   name; one whose data text is empty (`alloc_store_name_node`,
    ///   `alloc_store_shared_name_node`) keeps the name it was made with. The
    ///   keyword bit follows the name.
    /// - U1 (b): the modifier bits equal the flags of the node's own list.
    /// - The kind fits the node data, and the `NO_NODE` bit agrees with the
    ///   node column.
    fn debug_check_kids(&self) {
        for (i, ((record, kids), &kind)) in self
            .records
            .iter()
            .zip(&self.kids)
            .zip(&self.kinds)
            .enumerate()
        {
            let keyword = record.has_bit(NodeRecord::TEXT_IS_KEYWORD);
            let node = self.nodes[i].map(|_| self.slot_ast_node(i));
            assert_eq!(record.is_node(), node.is_some(), "node bit of slot {i}");
            let Some(node) = node else {
                assert_eq!(kind, SyntaxKind::Unknown, "kind of empty slot {i}");
                assert_eq!(kids.children(), SlotChildren::UNKNOWN, "kids of slot {i}");
                assert_eq!(kids.word(), 0, "word of empty slot {i}");
                assert!(!keyword, "keyword bit on empty slot {i}");
                continue;
            };
            assert!(node.matches_syntax_kind(kind), "kind of slot {i}");
            let entry = kids.children();
            if i != NIL_SLOT as usize && entry != SlotChildren::UNKNOWN {
                assert_eq!(
                    entry,
                    super::node::store_node_children(kind, node),
                    "U4 children of slot {i} differ from the node data"
                );
            }
            assert_eq!(
                kids.modifier_bits(kind),
                super::node::store_node_modifier_bits(kind, node, |m| {
                    self.modifier_list_bits(self.cell_of(i) != NO_CELL, m)
                }),
                "U1 (b) modifier bits of slot {i} differ from the node data"
            );
            if is_name_kind(kind) {
                let name = kids.text_name(kind);
                let text = identifier_text(node);
                assert!(
                    text.is_empty() || name.as_str() == text,
                    "U1 (a) name of slot {i} differs from the node data"
                );
                assert_eq!(
                    keyword,
                    get_identifier_token(name.as_str()) != SyntaxKind::Identifier,
                    "U1 (a) keyword bit of slot {i} differs from the name"
                );
            } else {
                assert!(!keyword, "U1 (a) keyword bit on non-name slot {i}");
            }
        }
    }

    /// Marks the store finished and marks its source file roots.
    fn end_parse(&mut self) {
        self.frozen = true;
        // Gives the identifier name cache back (`StoreIdents::drop`).
        drop(std::mem::take(&mut self.idents));
        self.records.shrink_to_fit();
        self.kinds.shrink_to_fit();
        self.kids.shrink_to_fit();
        self.nodes.shrink_to_fit();
        if let Some(owned) = self.owned.as_deref_mut() {
            owned.cell_of.shrink_to_fit();
            owned.pending.shrink_to_fit();
            owned.count_owned_nodes();
            owned.finish();
        }
        mark_source_file_roots(self);
    }

    /// The part of `publish_file_stores` for this store, which has id
    /// `file`: drops what only the parse and the loader used, and makes the
    /// facts when `freeze_file_store` did not.
    fn publish(&mut self, file: usize) {
        if !self.frozen {
            // PORT: a store whose parse did not finish (a parse panic).
            self.end_parse();
            // R2-5: a panic can stop the parser between two children of a
            // node, so its chains may be partial. No links: the binder uses
            // Go `ForEachChild` (`make_facts`).
            self.build_links = Vec::new();
        }
        self.aliases = FxHashMap::default();
        self.jsdoc_cache = FxHashMap::default();
        self.lazy_js_doc = None;
        self.lazy_jsdoc_cache = FxHashMap::default();
        self.parser_flags = None;
        // lsshells M3c: the `GoFile` holds the JSDoc cache and the
        // diagnostics of the file from now on.
        if let Some(owned) = self.owned.as_deref_mut() {
            owned.jsdoc = FxHashMap::default();
            owned.diagnostics = Box::default();
        }
        if self.root_slot != NIL_SLOT {
            self.root = handle(file, self.root_slot);
        }
        if !self.facts_made {
            self.make_facts();
        }
    }

    /// About how much work `publish` does, in slots: a fixed part for the
    /// maps it drops and one per slot when it still makes the facts.
    fn publish_work(&self) -> usize {
        let mut work = PUBLISH_WORK_PER_STORE;
        if !self.facts_made {
            work += self.records.len();
        }
        work
    }
}

/// `FileStore::publish_work` of one store without tables to make: about
/// the cost of its map drops, in slots.
const PUBLISH_WORK_PER_STORE: usize = 32;
/// `publish_stores` uses scoped threads from this much work on. Below it
/// the thread starts cost more than they save.
const PARALLEL_PUBLISH_WORK: usize = 1 << 17;
/// Threads for `publish_stores`, the calling thread included.
const PUBLISH_THREADS: usize = 4;

/// `FileStore::publish` for every store; store `i` has id `base + i`.
// PERF: effect R2-13. The stores are independent, so a large program is
// split into `PUBLISH_THREADS` runs of about equal work, one per scoped
// thread. The result does not depend on the split.
fn publish_stores(stores: &mut [FileStore], base: usize) {
    let total: usize = stores.iter().map(FileStore::publish_work).sum();
    // wasm has one thread.
    if cfg!(target_family = "wasm") || total < PARALLEL_PUBLISH_WORK {
        publish_run(stores, base);
        return;
    }
    let share = total.div_ceil(PUBLISH_THREADS);
    std::thread::scope(|scope| {
        let mut rest = stores;
        let mut first = base;
        for _ in 1..PUBLISH_THREADS {
            if rest.is_empty() {
                break;
            }
            let mut end = 0;
            let mut work = 0;
            while end < rest.len() && work < share {
                work += rest[end].publish_work();
                end += 1;
            }
            let (run, tail) = std::mem::take(&mut rest).split_at_mut(end);
            scope.spawn(move || publish_run(run, first));
            first += end;
            rest = tail;
        }
        publish_run(rest, first);
    });
}

/// `FileStore::publish` for `stores`, whose first store has id `first`.
fn publish_run(stores: &mut [FileStore], first: usize) {
    for (i, store) in stores.iter_mut().enumerate() {
        store.publish(first + i);
    }
}

/// True when the parse of store file `file` is over: the file is published
/// or `freeze_file_store` ran.
#[must_use]
pub fn is_file_store_frozen(file: usize) -> bool {
    is_published(file) || with_store(file, |s| s.frozen)
}

/// Number of slots (nil, alias and node slots). Per-node vectors that are
/// indexed by `NodeId::index()` (`GoFile::parser_flags`) need this length.
#[must_use]
pub fn file_store_slot_count(file: usize) -> usize {
    // A freeable file version keeps its records in its node shell.
    file_block(file)
        .map(|b| b.records.len())
        .unwrap_or_else(|| with_store(file, |s| s.records.len()))
}

/// The parser `node.Flags` of every slot, indexed by slot index. Nil and
/// alias slots give no flags. The loader fills `GoFile::parser_flags` from
/// this for the binder, before the publish.
#[must_use]
pub fn file_store_parser_flags(file: usize) -> Vec<NodeFlags> {
    // AST node records, step 2: the records of a published file also hold
    // the flags that the binder added, so its parser flags are the ones
    // that its `GoFile` took before the publish.
    if let Some(flags) = try_with_go_file(file, |g| g.parser_flags.clone()) {
        return flags;
    }
    let computed = |s: &FileStore| s.records.iter().map(NodeRecord::flags).collect();
    with_store_mut(file, |s| {
        s.parser_flags.take().unwrap_or_else(|| computed(s))
    })
}

/// Publishes the build stores of this thread: `go_files[i]` is the
/// `GoFile` of id `unpublished_file_ids().start + i`. The loader calls this
/// once per program, before `core::set_prog`. The stores are then
/// read-only, and any thread can read them through the block of their id
/// (`file_block`). A freeable file version (lsshells M3b) owns its store
/// and `GoFile`, and the block of its id is its node shell (`node_shell`).
/// An empty publish does nothing.
// PORT: Go needs no publish; its nodes are heap objects. The publish also
// computes `NodeHeader::source_file_is_root` for `get_source_file_of_node`.
pub fn publish_file_stores(go_files: Vec<GoFile>) {
    // A test build gives the build lock back after the publish.
    #[cfg(test)]
    let _build_end = crate::test_lock::BuildEnd;
    // The cells stay leaked and empty. Nothing reads them after this, and
    // the next build stores of this thread reuse them (`build_store_cell`).
    ACTIVE.set(None);
    let BuildStores {
        base,
        stores: cells,
    } = BUILD.with(|b| std::mem::take(&mut *b.borrow_mut()));
    let base = if cells.is_empty() {
        PUBLISHED.load(Ordering::Acquire)
    } else {
        base
    };
    assert!(
        go_files.len() == cells.len(),
        "a publish needs one GoFile per store ({} stores, {} GoFiles)",
        cells.len(),
        go_files.len()
    );
    let count = cells.len();
    if count == 0 {
        return;
    }
    assert!(base + count <= file_id_cap(), "too many file ids");
    if let Err(published) =
        PUBLISHED.compare_exchange(base, base + count, Ordering::AcqRel, Ordering::Acquire)
    {
        panic!(
            "another thread published file ids while this thread built ids {base}.. (now {published})"
        );
    }
    let mut stores: Vec<FileStore> = cells.iter().map(|cell| cell.take()).collect();
    SPARE_BUILD_CELLS.with(|spare| spare.borrow_mut().extend(cells));
    publish_stores(&mut stores, base);
    // lsshells M3b: a freeable file version (a live `FileVersion` of the
    // id, made by the language server parse cache) takes its store and
    // `GoFile`, and the block of its id is its node shell. The other files
    // are static, in runs of consecutive ids.
    let versions: FxHashMap<usize, std::sync::Arc<super::FileVersion>> =
        super::file_version::live_file_versions(base..base + count)
            .into_iter()
            .map(|version| (version.file(), version))
            .collect();
    if versions.is_empty() {
        publish_static(base, stores, go_files);
        return;
    }
    let mut run_start = base;
    let mut run_stores = Vec::new();
    let mut run_files = Vec::new();
    for (i, (store, go_file)) in stores.into_iter().zip(go_files).enumerate() {
        let file = base + i;
        let Some(version) = versions.get(&file) else {
            run_stores.push(store);
            run_files.push(go_file);
            continue;
        };
        if !run_stores.is_empty() {
            publish_static(
                run_start,
                std::mem::take(&mut run_stores),
                std::mem::take(&mut run_files),
            );
        }
        run_start = file + 1;
        let mut store = store;
        let (shell, block) = node_shell(file, &mut store);
        version.set_published(VersionStore {
            store,
            go_file,
            block,
        });
        set_file_block(file, shell);
    }
    if !run_stores.is_empty() {
        publish_static(run_start, run_stores, run_files);
    }
}

/// Leaks the published `stores` and `go_files` of a static publish, whose
/// first file id is `base`, and sets the block of each id.
// PERF: query Q7. The per-store tables are made already, so this only
// collects slices.
fn publish_static(base: usize, mut stores: Vec<FileStore>, go_files: Vec<GoFile>) {
    // lsshells M3c: a freeable parse whose version died before the publish
    // is published static, so its nodes are leaked now.
    for (i, store) in stores.iter_mut().enumerate() {
        store.leak_owned_nodes();
        // AST node records, step 4: the owner word (`block_is_owned`).
        *store.records[NIL_SLOT as usize].bind.get_mut() = NodeRecord::owner_word(base + i);
    }
    let stores: &'static [FileStore] = Box::leak(stores.into_boxed_slice());
    let files: &'static [BlockFile] = Box::leak(
        stores
            .iter()
            .zip(go_files)
            .map(|(s, go_file)| BlockFile {
                nodes: &s.nodes,
                lists: &s.lists,
                facts: FactsCell::new(s.facts),
                root: AtomicU64::new(s.root.0),
                foreign: &s.foreign_parents,
                links: &s.links,
                shell_kinds: &[],
                store: Some(s),
                go_file: Some(go_file),
                subtree_facts: OnceLock::new(),
            })
            .collect::<Box<[_]>>(),
    );
    for (i, (s, file)) in stores.iter().zip(files).enumerate() {
        debug_assert_eq!(
            record_resolve(base + i, NIL_SLOT as usize, &s.records),
            Node::NIL
        );
        set_file_block(
            base + i,
            FileBlock {
                kinds: &s.kinds,
                records: &s.records,
                kids: &s.kids,
                file,
            },
        );
    }
}

/// The node shell of freeable file version `file`, whose store is `store`:
/// the block of its id, with the node columns of the store: its records and
/// kids, copied into a pooled block (`BlockPool`, AST node records, step
/// 4), which the caller gives to the version, its kinds (a plain column,
/// or past a budget the pooled block after a plain prefix: `plain_kinds`,
/// edleak1, edleak3), and its
/// foreign parents, moved out of `store` and leaked. Its
/// `BlockFile` has no store, no `GoFile` and no child link column: the
/// `BlockFile` of the pooled block (`PoolBlock::file`, edleak1 part 2), or
/// a leaked one when the shell has foreign parents or a node column. A store
/// that owns its astdata nodes (lsshells M3c) keeps its node column and its
/// nodes, and the shell has none (`FileBlock::node_column`), so a node data
/// read reads the version (`static_store_node`, `StaticNode::Scoped`); a
/// store with leaked nodes moves its node column into the shell.
/// The store keeps its link column, and the binder walks it through
/// `version_child_links` (bindfast1). Every read of the store or the
/// `GoFile` finds no static one in the shell and reads the version
/// (`with_published_store`, `try_with_go_file`). The caller sets the block
/// after the version is published.
// PERF: lsshells M3 repair. A pinned read of the version (a thread-local
// pin lookup) for each node read of the edited file made query-core and
// effect edits 3 to 4 ms slower than R134 (lsshells/m3/final/editor-off),
// and 1.5 to 2 ms with only the kind column static
// (lsshells/m3/repair/prof): an edit reads the nodes of the edited file
// about 400,000 times. The node columns that the header and child reads
// need (42 bytes per node: 24 + 16 + 2 with the node records and the kind
// column) are read inline, as a static file, from a pooled block (step 4)
// and a plain kind column (a pooled one past a budget takes one cold call,
// edleak1: `plain_kinds`).
// Without the children and modifier columns, the child reads of the edited
// file read the node data, and edits were about 0.3 ms slower
// (lsshells/m3/repair/r4). The store maps and lists, the node column, the
// astdata nodes, the link column and the `GoFile` (binder and flow data,
// parse lists) are freed with the version, and its pooled block goes back
// to the pool (step 4). A stale header or child read gives the data of
// that node until another version takes the block, then the data of that
// version (a panic with debug assertions, `file_block`); a stale binder
// field read after the reuse (`check_block_owner`) and a stale node data
// read panic.
// PERF: bindfast1. The link column stays in the store (8 bytes per node,
// freed with the version): the bind of an edited file walked its children
// through the node data (`for_each_child`, scoped reads), and took 0.39 ms
// on effect's Option.ts against Go's 0.26 ms.
fn node_shell(file: usize, store: &mut FileStore) -> (FileBlock, PoolBlock) {
    debug_assert_eq!(
        record_resolve(file, NIL_SLOT as usize, &store.records),
        Node::NIL
    );
    // lsshells M3c: a store that owns its nodes keeps its node column. A
    // store with leaked nodes (a prefetched parse, or owned nodes off)
    // leaks it here, as before M3c, so its node data reads stay inline.
    // astshrink1: the list table goes with the node column.
    let (nodes, lists): (
        &'static [Option<ColumnData>],
        &'static [&'static StaticList],
    ) = if store.owned.is_some() {
        (&[], &[])
    } else {
        (
            Vec::leak(std::mem::take(&mut store.nodes)),
            Vec::leak(std::mem::take(&mut store.lists)),
        )
    };
    // AST node records, step 4: the records and kids go into a pooled
    // block. The nil slot first, with the owner word, so from here on a
    // stale read of the last owner of the block fails its owner check.
    let len = store.records.len();
    debug_assert_eq!(store.kids.len(), len);
    let block = take_pool_block(len);
    let records: &'static [NodeRecord] = &block.records[..len];
    let kids: &'static [NodeKids] = &block.kids[..len];
    let nil = NIL_SLOT as usize;
    records[nil].store_from(&store.records[nil]);
    records[nil]
        .bind
        .store(NodeRecord::owner_word(file), Ordering::Relaxed);
    for (to, from) in records.iter().zip(&store.records).skip(nil + 1) {
        to.store_from(from);
    }
    for (to, from) in kids.iter().zip(&store.kids) {
        to.store_from(from);
    }
    debug_assert_eq!(store.kinds.len(), len);
    let shell_kinds: &'static [AtomicU16] = &block.kinds[..len];
    let plain = plain_kinds(store.file_name, &mut store.kinds);
    if plain.is_err() {
        for (to, &from) in shell_kinds.iter().zip(&store.kinds) {
            to.store(from as u16, Ordering::Relaxed);
        }
    }
    store.records = Vec::new();
    store.kids = Vec::new();
    store.kinds = Vec::new();
    // edleak1 part 2: a shell with no leaked column takes the `BlockFile`
    // of its block, so it leaks nothing. A store with leaked nodes or
    // foreign parents leaks a `BlockFile` that names them, as before.
    let file = match block.file {
        Some(file) if nodes.is_empty() && store.foreign_parents.is_empty() => {
            file.facts.set(store.facts);
            file.root.store(store.root.0, Ordering::Relaxed);
            file
        }
        _ => Box::leak(Box::new(BlockFile {
            nodes,
            lists,
            facts: FactsCell::new(store.facts),
            root: AtomicU64::new(store.root.0),
            foreign: Vec::leak(std::mem::take(&mut store.foreign_parents)),
            links: &[],
            shell_kinds,
            store: None,
            go_file: None,
            subtree_facts: OnceLock::new(),
        })),
    };
    let shell = FileBlock {
        kinds: plain.unwrap_or_else(|prefix| prefix),
        records,
        kids,
        file,
    };
    (shell, block)
}

/// Sets `store.root_slot` and `NodeHeader::source_file_is_root` of each node
/// slot whose Go parent walk (`GetSourceFileOfNode`) ends at that root.
/// Headers are frozen, so the walk result cannot change. A walk that leaves
/// the store (a synthetic or foreign parent) stays unmarked and is walked
/// at read time.
fn mark_source_file_roots(store: &mut FileStore) {
    // PERF: step 4b, storefast1. The walk reads the record and kind columns
    // through one iterator, so it has no bounds checks for them.
    store.root_slot = mark_roots(&store.records, &store.kinds);
}

/// `mark_source_file_roots` on the columns of a store: the root slot (0 for
/// none).
///
/// The result of a slot is a function of the slot alone: true when its
/// parent chain (parents in this store) reaches slot `root` before any other
/// SourceFile slot, a nil or foreign parent, or a slot it has passed. The
/// walk goes down from the last slot, so a node slot above the current one
/// already has its bit.
// PERF: storefast1. The parser makes a parent after its children, so
// nearly every node slot copies the bit of a node slot above it, with no
// state vector. Only a parent at or below the slot (or a parent slot that
// holds no node) takes `RootWalk::mark`, which keeps the state of the walks
// that R200 made for every slot. The bit goes in through the atomic flags
// word, so the walk reads the columns through iterators, with one bounds
// check per slot (the parent).
fn mark_roots(records: &[NodeRecord], kinds: &[SyntaxKind]) -> u32 {
    assert_eq!(records.len(), kinds.len(), "one kind per record");
    // PORT: the parser makes the SourceFile node last, so the root is the
    // last SourceFile slot. Any other SourceFile node stays unmarked.
    let Some(root) = (0..records.len())
        .rev()
        .find(|&i| records[i].is_node() && kinds[i] == SyntaxKind::SourceFile)
    else {
        return 0;
    };
    let mut walk = RootWalk::default();
    let slots = records.iter().zip(kinds).enumerate().skip(1).rev();
    for (start, (record, &kind)) in slots {
        if !record.is_node() {
            continue;
        }
        let is_root = if kind == SyntaxKind::SourceFile {
            start == root
        } else {
            match record.local_parent() {
                None => false,
                Some(parent) => match records.get(parent) {
                    Some(up) if parent > start && up.is_node() => {
                        up.has_bit(NodeRecord::SOURCE_FILE_ROOT)
                    }
                    _ => walk.mark(records, kinds, root, start),
                },
            }
        };
        record.set_root_bit(is_root);
    }
    root as u32
}

/// The state of the slots that `mark_roots` walked: the slot walk of R200,
/// for the rare slot whose parent is not a node slot above it.
#[derive(Default)]
struct RootWalk {
    /// `UNSEEN`, `ON_PATH`, `ROOT` or `NOT_ROOT` per slot, made by the first
    /// walk.
    state: Vec<u8>,
    path: Vec<usize>,
}

impl RootWalk {
    const UNSEEN: u8 = 0;
    const ON_PATH: u8 = 1;
    const ROOT: u8 = 2;
    const NOT_ROOT: u8 = 3;

    /// The bit of slot `start` (and of each slot of its walk, which gets it
    /// too): its parent chain up to a node slot above `start` (whose bit is
    /// made), a SourceFile slot, a nil or foreign parent, or a slot that
    /// the walk has passed (a parent cycle: Go never returns; the walk at
    /// read time handles it).
    #[cold]
    #[inline(never)]
    fn mark(
        &mut self,
        records: &[NodeRecord],
        kinds: &[SyntaxKind],
        root: usize,
        start: usize,
    ) -> bool {
        if self.state.is_empty() {
            self.state = vec![Self::UNSEEN; records.len()];
        }
        let state = &mut self.state;
        let mut cur = start;
        let is_root = loop {
            if cur > start && records[cur].is_node() {
                break records[cur].has_bit(NodeRecord::SOURCE_FILE_ROOT);
            }
            match state[cur] {
                Self::ROOT => break true,
                Self::NOT_ROOT | Self::ON_PATH => break false,
                _ => {}
            }
            state[cur] = Self::ON_PATH;
            self.path.push(cur);
            if kinds[cur] == SyntaxKind::SourceFile {
                break cur == root;
            }
            let Some(parent) = records[cur].local_parent() else {
                break false;
            };
            cur = parent;
        };
        let result = if is_root { Self::ROOT } else { Self::NOT_ROOT };
        for i in self.path.drain(..) {
            state[i] = result;
            records[i].set_root_bit(is_root);
        }
        is_root
    }
}

// ──────────────────────────────────────────────────────────────────────
// Read hooks for node.rs and core.rs
// ──────────────────────────────────────────────────────────────────────

/// Hook for `Node::new(file, id)` on a store file: the Go node that a child
/// id inside store `NodeData` stands for.
#[inline]
#[must_use]
pub fn resolve_store_id(file: usize, id: crate::astdata::NodeId) -> Node {
    let index = id.index();
    // The nil slot or an alias slot: the target is in the record
    // (`record_resolve`). A node shell has the records too.
    if let Some(b) = file_block(file) {
        return record_resolve(file, index, b.records);
    }
    with_store(file, |s| s.slot_resolve(file, index))
}

/// Hook for `raw(n)`: the Go node data of a store node. Panics for a node
/// with no `'static` data (a node that a freeable parse owns, lsshells
/// M3c): read it with `ast::with_ast_data`.
#[inline]
#[must_use]
pub fn store_ast_node(n: Node) -> NodeData<'static> {
    if let Some(nodes) = file_block(n.file_index()).and_then(FileBlock::node_column) {
        return slot_node(nodes[slot_index(n)]);
    }
    match static_store_node(n) {
        StaticNode::Static(node) => node,
        StaticNode::Scoped => {
            panic!("node {n:?} is owned by a freeable parse and has no 'static data")
        }
        StaticNode::NoStore => panic!(
            "file {:#x} has no node store on this thread",
            n.file_index()
        ),
    }
}

/// Hook for `Node::kind`, `flags`, `parent` and `loc` on a store node.
#[inline]
#[must_use]
pub fn store_header(n: Node) -> NodeHeader {
    let (file, index) = (n.file_index(), slot_index(n));
    if let Some(b) = file_block(file) {
        let record = &b.records[index];
        debug_assert!(record.is_node(), "store handle does not name a node slot");
        return record.header(b.kind(index), file, b.file.foreign);
    }
    with_store(file, |s| {
        debug_assert!(
            s.records[index].is_node(),
            "store handle does not name a node slot"
        );
        s.slot_header(file, index)
    })
}

/// The header of `n` when it is a store node (kind, parser flags, parent,
/// loc): one thread-local load for the active store, one block lookup in
/// the registry (`file_block`), one more thread-local access for another
/// unpublished store. `None` for nil and synthetic nodes. With it a
/// node read needs no separate `has_file_store` call.
// PERF: query Q8. The active store is checked first and inline; every
// other case is out of line.
#[inline]
#[must_use]
pub fn try_store_header(n: Node) -> Option<NodeHeader> {
    if n.is_nil() {
        return None;
    }
    match active_store_header(n) {
        Some(header) => Some(header),
        None => try_store_header_slow(n.file_index(), slot_index(n)),
    }
}

/// `try_store_header` for a node that is not in the active store.
#[inline(never)]
fn try_store_header_slow(file: usize, index: usize) -> Option<NodeHeader> {
    if let Some(b) = file_block(file) {
        return Some(b.records[index].header(b.kind(index), file, b.file.foreign));
    }
    unpublished_store(file).map(|store| store.borrow().slot_header(file, index))
}

/// The header of `n` when `n` is a node of the active store of this thread
/// (see `ACTIVE`): the parse fast path of the node reads. That store is not
/// published. `None` for any other node.
#[inline]
#[must_use]
pub fn active_store_header(n: Node) -> Option<NodeHeader> {
    if n.is_nil() {
        return None;
    }
    let file = n.file_index();
    let store = active_store(file)?;
    Some(store.borrow().slot_header(file, slot_index(n)))
}

/// Go `node.Kind` of `n` when `n` is a node of the active store of this
/// thread: the kind column only (`active_store_header` makes the whole
/// header). `None` for any other node.
// PERF: storefast1. The parser reads the kind of the nodes it has just
// made (`Node::kind_slow`).
#[inline]
#[must_use]
pub fn active_store_kind(n: Node) -> Option<SyntaxKind> {
    if n.is_nil() {
        return None;
    }
    let store = active_store(n.file_index())?;
    Some(store.borrow().kinds[slot_index(n)])
}

/// Go `node.Flags` of `n` when `n` is a node of the active store of this
/// thread: the flags word of its record only. The active store is not
/// published, so it has no binder flags. `None` for any other node.
// PERF: storefast1, as `active_store_kind` (`Node::flags_slow`).
#[inline]
#[must_use]
pub fn active_store_flags(n: Node) -> Option<NodeFlags> {
    if n.is_nil() {
        return None;
    }
    let store = active_store(n.file_index())?;
    Some(store.borrow().records[slot_index(n)].flags())
}

/// Go `node.Kind` of a published store node (`file_block`, the node shell
/// of a freeable file version included). `None` for any other node: nil,
/// synthetic and unpublished store nodes.
#[inline]
#[must_use]
pub fn frozen_store_kind(n: Node) -> Option<SyntaxKind> {
    if n.is_nil() {
        return None;
    }
    file_block(n.file_index()).map(|b| b.kind(slot_index(n)))
}

/// Go `node.facts` (`CompositeBase`) of published store node `n` of a
/// static publish: its word in the facts column of its file
/// (`BlockFile::subtree_facts`), which the first call for that file makes.
/// `None` for any other node: nil, synthetic, unpublished, or a node of a
/// freeable file version (its node shell has no column).
#[inline]
pub(crate) fn static_facts_word(n: Node) -> Option<&'static AtomicU32> {
    if n.is_nil() {
        return None;
    }
    let b = file_block(n.file_index())?;
    let column = match b.file.subtree_facts.get() {
        Some(column) => column,
        None => new_static_facts_column(b)?,
    };
    column.get(slot_index(n))
}

/// The facts column of block `b`, made now with all words 0 (no facts
/// computed), or `None` when `b` is a node shell (no static store).
#[cold]
#[inline(never)]
fn new_static_facts_column(b: &'static FileBlock) -> Option<&'static [AtomicU32]> {
    if b.file.store.is_none() {
        return None;
    }
    let column = b
        .file
        .subtree_facts
        .get_or_init(|| (0..b.kinds.len()).map(|_| AtomicU32::new(0)).collect());
    Some(column)
}

/// The record of a published store node, by reference, so a read of one
/// field loads one word. `None` as for `frozen_store_kind`.
// PERF: lsshells M3 repair. Not generic over the read, as in R134: a
// generic `frozen_header<R>` was out of line in 21 copies, and `goport -p`
// ran more instructions in the header reads.
#[inline]
fn frozen_record(n: Node) -> Option<&'static NodeRecord> {
    if n.is_nil() {
        return None;
    }
    file_block(n.file_index()).map(|b| &b.records[slot_index(n)])
}

/// AST node records, step 2: entry `index` of the foreign parents
/// (`ParentCode`) of published store `file` (the node shell of a freeable
/// file version included).
#[cold]
#[inline(never)]
fn frozen_foreign_parent(file: usize, index: usize) -> Node {
    file_block(file)
        .expect("the foreign parents of a published store")
        .file
        .foreign[index]
}

/// Parser `node.Flags` of a published store node (see
/// `frozen_owned_record`), with the binder bits after its file is bound.
#[inline]
#[must_use]
pub fn frozen_store_flags(n: Node) -> Option<NodeFlags> {
    frozen_owned_record(n).map(NodeRecord::flags)
}

/// `frozen_record` with the owner check (`check_block_owner`), for the
/// binder field reads: the symbol, the `bind` word (the flow node and the
/// extras) and the flags.
#[inline]
fn frozen_owned_record(n: Node) -> Option<&'static NodeRecord> {
    if n.is_nil() {
        return None;
    }
    let file = n.file_index();
    let b = file_block(file)?;
    let r = &b.records[slot_index(n)];
    check_block_owner(b, file);
    Some(r)
}

/// AST node records, step 4: the owner check of a release build. Panics
/// as a read of a dead version's store does ("file version N is
/// released") when block `b` of file `file` is a pooled block that another
/// file version took (`block_is_owned`). The binder field reads
/// (`frozen_owned_record`) and the bind (`bind_records`,
/// `bind_store_records`) run it; the header and kids reads do not
/// (PORTING.md, "AST").
// PERF: ownercheck1. One load of the owner word (the nil slot record of
// the block, next to the `records` address that the read loaded), one
// compare and one branch to a cold call. A hot header read is about 6
// instructions, so the kind, parent and loc reads do not check.
#[inline]
fn check_block_owner(b: &FileBlock, file: usize) {
    if !block_is_owned(b, file) {
        super::file_version::released(file);
    }
}

/// AST node records, step 2: Go `node.Symbol()` of a published store node
/// (see `frozen_owned_record`), nil before its file is bound.
#[inline]
#[must_use]
pub fn frozen_store_symbol(n: Node) -> Option<SymbolId> {
    frozen_owned_record(n).map(|r| {
        debug_assert!(r.is_node(), "store handle does not name a node slot");
        r.symbol()
    })
}

/// AST node records, step 2: the `bind` word of a published store node
/// (see `NodeRecord` and `frozen_owned_record`), 0 before its file is
/// bound.
#[inline]
#[must_use]
pub fn frozen_store_bind_word(n: Node) -> Option<u32> {
    frozen_owned_record(n).map(|r| {
        debug_assert!(r.is_node(), "store handle does not name a node slot");
        r.bind_word()
    })
}

/// AST node records, step 3: `frozen_store_bind_word(n)` and the `GoFile`
/// of the file of `n` when it is static (`None` in the node shell of a
/// freeable file version), with one block lookup: the extras of a node
/// (`Node::bind_extra`) are in that `GoFile`. `None` as for
/// `frozen_record`; panics as `check_block_owner`.
#[inline]
#[must_use]
pub fn frozen_store_bind_and_file(n: Node) -> Option<(u32, Option<&'static GoFile>)> {
    if n.is_nil() {
        return None;
    }
    let file = n.file_index();
    let b = file_block(file)?;
    let r = &b.records[slot_index(n)];
    check_block_owner(b, file);
    debug_assert!(r.is_node(), "store handle does not name a node slot");
    Some((r.bind_word(), b.file.go_file.as_ref()))
}

/// AST node records, step 2: true when `test` is true for the symbol of
/// some node slot of published store `file`. `None` when `file` is not a
/// published store; panics as `check_block_owner`.
#[must_use]
pub fn frozen_store_any_symbol(
    file: usize,
    mut test: impl FnMut(SymbolId) -> bool,
) -> Option<bool> {
    file_block(file).map(|b| {
        check_block_owner(b, file);
        b.records.iter().any(|r| r.is_node() && test(r.symbol()))
    })
}

/// AST node records, step 2 (`BoundFile::install` of a lib snapshot load;
/// a live bind writes the records itself, `BindRecords`): writes the binder
/// output of published store file `file` into its records (its block: a
/// static file or the node shell of a freeable file version). `nodes` gives, in slot
/// order, each node slot that has binder data or a flow node: its slot
/// index, the index of its data entry, the data (`None` for a node with
/// only a flow node) and the flow node (`binder::BoundNodes`). The symbol
/// and the added flags go into the record. A node with no other fields
/// keeps its flow node in the record (`bind`); the other fields of any
/// other node go into the returned extras with its flow node, and the
/// record keeps their index + 1 with `BIND_EXTRA` (`NodeRecord`, `bind`).
/// Slots that share a data entry and a flow node share one extras entry.
// PORT: only this writes a published record (see `NodeRecord`). The
// writes are `Relaxed` stores; the bind of a file ends before its binder
// fields are read on another thread.
// PERF: the fields of an entry are found once for a run of slots that
// share it, and a node with only a flow node writes one word.
pub fn bind_store_records<'d>(
    file: usize,
    nodes: impl Iterator<Item = (usize, usize, Option<&'d NodeBindData>, FlowNodeId)>,
) -> Vec<NodeBindExtra> {
    let block = file_block(file).unwrap_or_else(|| panic!("file {file} has no published records"));
    // AST node records, step 4: the bind writes the records of a live
    // version only.
    check_block_owner(block, file);
    let records: &'static [NodeRecord] = block.records;
    let flow_file = (file as u64) << 32;
    let mut extras: Vec<NodeBindExtra> = Vec::new();
    // The last data entry, and its symbol, added flags and extras (`None`
    // for none).
    let mut last = (usize::MAX, SymbolId::NIL, NodeFlags::NONE, None);
    // The flow node and the `bind` word of the last extras entry of `last`
    // (0 for none yet).
    let mut last_extra = (FlowNodeId::NIL, 0u32);
    for (index, entry, data, flow) in nodes {
        // The bind never writes the nil slot: its `bind` is the owner word.
        assert_ne!(
            index, NIL_SLOT as usize,
            "binder data on the nil slot of file {file}"
        );
        let record = &records[index];
        assert!(
            flow.is_nil() || flow.0 & !0xffff_ffff == flow_file,
            "flow node of slot {index} of file {file} is in another file"
        );
        let flow_word = flow.0 as u32;
        assert_eq!(
            flow_word & BIND_EXTRA,
            0,
            "too many flow nodes in file {file}"
        );
        let Some(data) = data else {
            record.write_bind(SymbolId::NIL, NodeFlags::NONE, flow_word);
            continue;
        };
        if last.0 != entry {
            // Once per data entry, in release builds too: a flag outside
            // `BINDER_ADDED_FLAGS` could set a record bit (`write_bind`).
            // One mask compare per entry, not per record.
            assert!(
                super::node::BINDER_ADDED_FLAGS.contains(data.added_flags),
                "binder added {:#x}, outside BINDER_ADDED_FLAGS",
                data.added_flags
                    .without(super::node::BINDER_ADDED_FLAGS)
                    .bits()
            );
            let extra = NodeBindExtra::of(data);
            let extra = (extra != NodeBindExtra::NONE).then_some(extra);
            last = (entry, data.symbol, data.added_flags, extra);
            last_extra = (FlowNodeId::NIL, 0);
        }
        let (_, symbol, added, extra) = last;
        let bind = match extra {
            None => flow_word,
            Some(mut extra) => {
                if last_extra.1 == 0 || last_extra.0 != flow {
                    extra.flow_node = flow;
                    extras.push(extra);
                    let index = u32::try_from(extras.len()).expect("binder extras");
                    assert_eq!(index & BIND_EXTRA, 0, "binder extras");
                    last_extra = (flow, BIND_EXTRA | index);
                }
                last_extra.1
            }
        };
        // A symbol goes into `up`, which holds the target of a slot with
        // no node.
        assert!(
            symbol.is_nil() || record.is_node(),
            "binder data on slot {index} of file {file}, not a node"
        );
        record.write_bind(symbol, added, bind);
    }
    extras
}

/// bindonce2: the node records of one published file, as a live bind
/// writes them (`binder::NodeBindRecords`). Go's binder writes
/// `node.Symbol`, `node.Flags` and `FlowNode` into the node while it binds
/// and reads them back from there (binder.go); so does the binder here,
/// with the layout that `bind_store_records` writes: the symbol in the high
/// half of `up`, the added flags in `flags`, and the `bind` word. A copy
/// is valid while the file version is (the binder holds its pin). Index
/// with `NodeId::index()` of a node slot.
// PORT: the bind is the only writer of a published record, and only the
// thread that binds the file writes its records. Its readers start after
// the install (see `NodeRecord`).
#[derive(Clone, Copy, Default)]
pub struct BindRecords(&'static [NodeRecord]);

/// The records of published store file `file` for its bind. Panics when
/// `file` has no published records, or (as `check_block_owner`) when its
/// block belongs to another file version.
#[must_use]
pub fn bind_records(file: usize) -> BindRecords {
    let block = file_block(file).unwrap_or_else(|| panic!("file {file} has no published records"));
    check_block_owner(block, file);
    BindRecords(block.records)
}

impl BindRecords {
    /// The slot count of the file.
    #[must_use]
    pub fn len(self) -> usize {
        self.0.len()
    }

    /// True for a file with no slots (only the default value).
    #[must_use]
    pub fn is_empty(self) -> bool {
        self.0.is_empty()
    }

    /// Go `node.Symbol()` of node slot `index` while binding.
    #[inline]
    #[must_use]
    pub fn symbol(self, index: usize) -> SymbolId {
        let record = &self.0[index];
        debug_assert!(record.is_node(), "binder read of slot {index}, not a node");
        record.symbol()
    }

    /// Go `node.DeclarationData().Symbol = symbol` of node slot `index`.
    /// Returns true when the slot had no symbol before.
    #[inline]
    pub fn set_symbol(self, index: usize, symbol: SymbolId) -> bool {
        let record = &self.0[index];
        // A symbol goes into `up`, which holds the target of a slot with
        // no node.
        assert!(record.is_node(), "binder data on slot {index}, not a node");
        let up = record.up.load(Ordering::Relaxed);
        record.up.store(
            (up & 0xffff_ffff) | (u64::from(symbol.0) << 32),
            Ordering::Relaxed,
        );
        up >> 32 == 0
    }

    /// Go `node.Flags` of slot `index` while binding: the parser flags and
    /// the bits the binder added so far.
    #[inline]
    #[must_use]
    pub fn flags(self, index: usize) -> NodeFlags {
        self.0[index].flags()
    }

    /// Go `node.Flags |= flags` of slot `index`. `flags` must be in
    /// `BINDER_ADDED_FLAGS`, which has no record bit (checked in release
    /// builds too, as `bind_store_records` does).
    #[inline]
    pub fn add_flags(self, index: usize, flags: NodeFlags) {
        assert!(
            super::node::BINDER_ADDED_FLAGS.contains(flags),
            "binder added {:#x}, outside BINDER_ADDED_FLAGS",
            flags.without(super::node::BINDER_ADDED_FLAGS).bits()
        );
        assert_ne!(index, NIL_SLOT as usize, "binder data on the nil slot");
        let record = &self.0[index];
        let word = record.flags.load(Ordering::Relaxed);
        record.flags.store(word | flags.0, Ordering::Relaxed);
    }

    /// Go `node.Flags &^= flags` of slot `index`, for flags in
    /// `BINDER_ADDED_FLAGS` (the record bits stay).
    #[inline]
    pub fn clear_flags(self, index: usize, flags: NodeFlags) {
        debug_assert!(super::node::BINDER_ADDED_FLAGS.contains(flags));
        let record = &self.0[index];
        let word = record.flags.load(Ordering::Relaxed);
        record.flags.store(
            word & !(flags.0 & !NodeRecord::RECORD_BITS),
            Ordering::Relaxed,
        );
    }

    /// The `bind` word of slot `index` (see `NodeRecord`).
    #[inline]
    #[must_use]
    pub fn bind_word(self, index: usize) -> u32 {
        self.0[index].bind_word()
    }

    /// Writes the `bind` word of slot `index`: a flow node of the file (its
    /// low half) or an extras index with `BIND_EXTRA`. Never the nil slot,
    /// whose `bind` is the owner word (checked in release builds too).
    #[inline]
    pub fn set_bind_word(self, index: usize, word: u32) {
        assert_ne!(index, NIL_SLOT as usize, "binder data on the nil slot");
        self.0[index].bind.store(word, Ordering::Relaxed);
    }

    /// Moves the symbol of each slot of `slots` (slots that `set_symbol`
    /// gave a symbol) with `f` (`BoundFile::remap`).
    pub fn map_symbols(self, slots: &[u32], f: impl Fn(SymbolId) -> SymbolId) {
        for &index in slots {
            let record = &self.0[index as usize];
            let up = record.up.load(Ordering::Relaxed);
            let symbol = f(SymbolId((up >> 32) as u32));
            record.up.store(
                (up & 0xffff_ffff) | (u64::from(symbol.0) << 32),
                Ordering::Relaxed,
            );
        }
    }

    /// The binder fields of slot `index` that are in its record: the
    /// symbol, Go `node.Flags` and the `bind` word. `None` for the nil slot
    /// and alias slots. The lib snapshot generator reads a live bind back
    /// with it (`binder::BoundNodes::compact`).
    #[must_use]
    pub fn node_fields(self, index: usize) -> Option<(SymbolId, NodeFlags, u32)> {
        let record = &self.0[index];
        record
            .is_node()
            .then(|| (record.symbol(), record.flags(), record.bind_word()))
    }

    /// Undoes the writes of a bind that is not installed (a failed or
    /// panicked bind, `binder::NodeBindRecords`): every slot but the nil
    /// slot gets its parse state back, with no symbol, a `bind` word of 0
    /// and only the flags of `parser_flags` (the binder only adds bits to
    /// them). Runs during unwinding, so it does no lookup that can panic.
    pub fn reset(self, parser_flags: &[NodeFlags]) {
        for (index, record) in self.0.iter().enumerate().skip(1) {
            if record.is_node() {
                let up = record.up.load(Ordering::Relaxed);
                record.up.store(up & 0xffff_ffff, Ordering::Relaxed);
            }
            record.bind.store(0, Ordering::Relaxed);
            let parser = parser_flags.get(index).map_or(0, |flags| flags.0);
            let word = record.flags.load(Ordering::Relaxed);
            let added = word & super::node::BINDER_ADDED_FLAGS.0 & !parser;
            record.flags.store(word & !added, Ordering::Relaxed);
        }
    }
}

/// Go `node.Loc` of a published store node (see `frozen_record`).
#[inline]
#[must_use]
pub fn frozen_store_loc(n: Node) -> Option<TextRange> {
    frozen_record(n).map(NodeRecord::loc)
}

/// Go `node.Parent` of a published store node (see `frozen_record`) when
/// it is nil or a node of the same store. `None` for a parent in another
/// store (`ParentCode`): the caller reads the header (`try_store_header`).
// PERF: AST node records, step 2. No call here, so `Node::parent` stays
// inline: with the foreign table read in this path, it went out of line
// and `goport -p` on effect ran about 1.4% more instructions in it.
#[inline]
#[must_use]
pub fn frozen_store_parent(n: Node) -> Option<Node> {
    let r = frozen_record(n)?;
    debug_assert!(r.is_node(), "store handle does not name a node slot");
    let code = r.parent_code();
    if code & FOREIGN_PARENT != 0 {
        return None;
    }
    Some(local_parent_of_code(code, n.file_index()))
}

/// The node data of a published store node: the inlined fast path of
/// `static_ast_node`. `None` as for `frozen_store_kind`. Panics like
/// `try_store_ast_node` on a nil or alias slot.
#[inline]
#[must_use]
pub fn frozen_store_ast_node(n: Node) -> Option<NodeData<'static>> {
    if n.is_nil() {
        return None;
    }
    file_block(n.file_index())?
        .node_column()
        .map(|nodes| slot_node(nodes[slot_index(n)]))
}

/// `frozen_store_ast_node` with the list table of the data
/// (`BlockFile::lists`), from one registry lookup: the `list_of!` read.
#[inline]
#[must_use]
pub fn frozen_store_ast_node_lists(
    n: Node,
) -> Option<(NodeData<'static>, &'static [&'static StaticList])> {
    if n.is_nil() {
        return None;
    }
    let block = file_block(n.file_index())?;
    let data = slot_node(block.node_column()?[slot_index(n)]);
    Some((data, block.file.lists))
}

/// A published store node with its registry block, read once: the node
/// shell of a freeable file version included (`frozen_store_kind`). Its
/// reads are plain loads from the block's columns, so a caller that reads
/// several fields of one node does one registry lookup, not one per field.
/// Lives no longer than a node read: the block of a freeable file version
/// is valid only while the version is.
// PERF: noderef1 (getsym1 report, W1). Go reads `n.Kind` and `n.data` from
// one struct. Each `Node` read here goes through `file_block`, whose
// `OnceLock` acquire load stops LLVM from merging repeated lookups.
#[derive(Clone, Copy)]
pub struct FrozenNode {
    block: &'static FileBlock,
    file: usize,
    index: usize,
}

impl FrozenNode {
    /// Go `node.Kind` (`frozen_store_kind`).
    #[inline]
    #[must_use]
    pub fn kind(self) -> SyntaxKind {
        self.block.kind(self.index)
    }

    /// The node data (`frozen_store_ast_node`). `None` in the node shell of
    /// a freeable file version whose store owns its astdata nodes
    /// (lsshells M3c): read those with `with_scoped_ast_node`.
    #[inline]
    #[must_use]
    pub fn data(self) -> Option<NodeData<'static>> {
        self.block
            .node_column()
            .map(|nodes| slot_node(nodes[self.index]))
    }

    /// The list table of the data of this node (`BlockFile::lists`).
    #[inline]
    #[must_use]
    pub fn lists(self) -> &'static [&'static StaticList] {
        self.block.file.lists
    }

    /// How `Node::new(file, id)` maps the child ids in the data of this
    /// node (`frozen_store_ids`).
    // PERF: noderef1 W2. A child id becomes its handle with no call and no
    // second registry lookup.
    #[inline]
    #[must_use]
    pub fn ids(self) -> FrozenIds {
        block_ids(self.block, self.file)
    }

    /// `frozen_store_children` of this node: its children in Go
    /// `ForEachChild` order from the link column of its block, or `None`
    /// when its chain is not known or the block has no link column (a
    /// node shell).
    // PERF: perfP1 item 1. No second registry lookup.
    #[inline]
    #[must_use]
    pub fn children(self) -> Option<StoreChildren<'static>> {
        let links = self.block.file.links;
        let first = links.get(self.index)?.first_child;
        (first != LINK_NONE).then_some(StoreChildren {
            file: self.file,
            links,
            next: first,
        })
    }
}

/// The `FrozenNode` of `n`. `None` as for `frozen_store_kind`: nil,
/// synthetic and unpublished store nodes.
#[inline]
#[must_use]
pub fn frozen_store_node(n: Node) -> Option<FrozenNode> {
    if n.is_nil() {
        return None;
    }
    let file = n.file_index();
    Some(FrozenNode {
        block: file_block(file)?,
        file,
        index: slot_index(n),
    })
}

/// A node with the registry block of its file, read once (`file_block`):
/// Go `*ast.Node` as a predicate reads it. Its reads of the kind, parent,
/// flags, loc and the column children are plain loads from the block's
/// columns, and `parent` of a node whose parent is in the same store keeps
/// the block. Any other node (nil, synthetic, unpublished; a parent in
/// another store makes its own lookup) reads through the `Node` methods,
/// which give the same values. Lives no longer than a node read: the block
/// of a freeable file version is valid only while the version is.
// PERF: noderef1 G1 (getsym1 report). Each `Node` read goes through
// `file_block`, whose `OnceLock` acquire load stops LLVM from merging
// repeated lookups: `node.parent().kind()` three times in one predicate was
// six lookups. Go reads struct fields.
#[derive(Clone, Copy)]
pub struct NodeRef {
    node: Node,
    /// The block of `node`'s file, `None` for a node that is not a
    /// published store node.
    block: Option<&'static FileBlock>,
}

impl std::fmt::Debug for NodeRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "NodeRef({:?})", self.node)
    }
}

impl NodeRef {
    /// The nil node.
    pub const NIL: NodeRef = NodeRef {
        node: Node::NIL,
        block: None,
    };

    /// The view of `n`: one registry lookup for a published store node.
    #[inline]
    #[must_use]
    pub fn new(n: Node) -> Self {
        if n.is_nil() {
            return Self::NIL;
        }
        NodeRef {
            node: n,
            block: file_block(n.file_index()),
        }
    }

    #[inline]
    #[must_use]
    pub fn node(self) -> Node {
        self.node
    }

    #[inline]
    #[must_use]
    pub fn is_nil(self) -> bool {
        self.node.is_nil()
    }

    #[inline]
    #[must_use]
    pub fn is_some(self) -> bool {
        self.node.is_some()
    }

    /// Go `node.Kind` (`Node::kind`).
    #[inline]
    #[must_use]
    pub fn kind(self) -> SyntaxKind {
        match self.block {
            Some(b) => b.kind(slot_index(self.node)),
            None => node_ref_kind(self.node),
        }
    }

    /// Go `node.Kind` of a node that may be nil: `SyntaxKind::Unknown` for
    /// nil (`node_parent_and_kind`).
    #[inline]
    #[must_use]
    pub fn kind_or_unknown(self) -> SyntaxKind {
        match self.block {
            Some(b) => b.kind(slot_index(self.node)),
            None if self.node.is_nil() => SyntaxKind::Unknown,
            None => node_ref_kind(self.node),
        }
    }

    /// Go `node.Parent` (`Node::parent`). A parent in the same store keeps
    /// the block; a parent in another store makes its own lookup.
    // PERF: parentname1. The body is only the same-store step (and the nil
    // parent at the root), as Go loads `node.Parent`. A node with no block
    // and a parent in another store take the cold `parent_far`, so the body
    // is small enough to inline at the hot sites. In the PGO build the old
    // body, with two `NodeRef::new` lookups, was out of line, with about 20
    // instructions of call overhead per step (checkline1).
    #[inline]
    #[must_use]
    pub fn parent(self) -> NodeRef {
        if let Some(b) = self.block {
            let r = &b.records[slot_index(self.node)];
            debug_assert!(r.is_node(), "store handle does not name a node slot");
            let code = r.parent_code();
            if code & FOREIGN_PARENT == 0 {
                return NodeRef {
                    node: local_parent_of_code(code, self.node.file_index()),
                    block: if code == 0 { None } else { Some(b) },
                };
            }
        }
        self.parent_far()
    }

    /// `parent` of a node with no block (nil, synthetic, unpublished) or
    /// whose parent is in another store (`FOREIGN_PARENT`).
    #[cold]
    #[inline(never)]
    fn parent_far(self) -> NodeRef {
        let Some(b) = self.block else {
            return NodeRef::new(self.node.parent());
        };
        let code = b.records[slot_index(self.node)].parent_code();
        debug_assert_ne!(code & FOREIGN_PARENT, 0);
        NodeRef::new(frozen_foreign_parent(
            self.node.file_index(),
            (code & !FOREIGN_PARENT) as usize,
        ))
    }

    /// Go `node.Flags` (`Node::flags`), with the owner check of the binder
    /// field reads (`frozen_owned_record`).
    #[inline]
    #[must_use]
    pub fn flags(self) -> NodeFlags {
        match self.block {
            Some(b) => {
                let r = &b.records[slot_index(self.node)];
                check_block_owner(b, self.node.file_index());
                r.flags()
            }
            None => node_ref_flags(self.node),
        }
    }

    /// Go `node.Flags & mask` for a `mask` without a binder-added bit
    /// (`Node::parser_flags`).
    #[inline]
    #[must_use]
    pub fn parser_flags(self, mask: NodeFlags) -> NodeFlags {
        debug_assert!(
            !mask.intersects(super::node::BINDER_ADDED_FLAGS),
            "parser_flags mask {:#x} has binder bits",
            mask.bits()
        );
        // checkid1, neutral: a node that is not a published store node
        // reads through `Node::parser_flags`. That is the same read as the
        // old path (`Node::flags`: the frozen record, then `flags_slow`),
        // and neither needs binder data, so the value and the cost do not
        // change. It keeps this fallback in the shape of `modifier_flags`.
        match self.block {
            Some(_) => self.flags() & mask,
            None => node_ref_parser_flags(self.node, mask),
        }
    }

    /// Go `node.ModifierFlags()` (`Node::modifier_flags`): from the
    /// modifier word of the block's kids (`frozen_store_modifier_flags`).
    // PERF: checkid1. For `get_combined_modifier_flags`.
    #[inline]
    #[must_use]
    pub fn modifier_flags(self) -> ModifierFlags {
        match self.block {
            Some(b) => {
                let index = slot_index(self.node);
                let flags = ModifierFlags(b.kids[index].modifier_bits(b.kind(index)));
                debug_assert_eq!(flags, self.node.modifier_flags(), "NodeRef modifier flags");
                flags
            }
            None => node_ref_modifier_flags(self.node),
        }
    }

    /// Go `node.Loc` (`Node::loc`).
    #[inline]
    #[must_use]
    pub fn loc(self) -> TextRange {
        match self.block {
            Some(b) => b.records[slot_index(self.node)].loc(),
            None => node_ref_loc(self.node),
        }
    }

    /// Go `GetSourceFileOfNode(node)` when the node's parent walk ends at
    /// its store root (`frozen_source_file_of_node`), else `None`.
    #[inline]
    #[must_use]
    pub fn source_file_root(self) -> Option<Node> {
        let b = self.block?;
        b.records[slot_index(self.node)]
            .has_bit(NodeRecord::SOURCE_FILE_ROOT)
            .then(|| b.file.root())
    }

    /// The node data (`static_ast_node`), or `None` for a node with no
    /// node column read: nil, synthetic, unpublished, or the node shell of
    /// a freeable file version whose store owns its astdata nodes. The
    /// caller then reads the data through the `Node` (`with_data!`).
    #[inline]
    #[must_use]
    pub fn data(self) -> Option<NodeData<'static>> {
        let nodes = self.block?.node_column()?;
        Some(slot_node(nodes[slot_index(self.node)]))
    }

    /// The column child `which` (`frozen_store_child`), else `None`.
    // PERF: inlinefix1, as `frozen_store_child`.
    #[inline(always)]
    fn column_child(self, which: StoreChild) -> Option<Node> {
        column_store_child(self.block?, self.node, which)
    }

    /// Go `node.Name()` (`Node::name`).
    #[inline]
    #[must_use]
    pub fn name(self) -> Node {
        match self.column_child(StoreChild::Name) {
            Some(name) => {
                debug_assert_eq!(name, self.node.name(), "NodeRef name column");
                name
            }
            None => node_ref_name(self.node),
        }
    }

    /// Go `node.Expression()` (`Node::expression`).
    #[inline]
    #[must_use]
    pub fn expression(self) -> Node {
        match self.column_child(StoreChild::Expression) {
            Some(expression) => {
                debug_assert_eq!(expression, self.node.expression(), "NodeRef expression");
                expression
            }
            None => node_ref_expression(self.node),
        }
    }

    /// Go `node.Initializer()` (`Node::initializer`).
    #[inline]
    #[must_use]
    pub fn initializer(self) -> Node {
        match self.column_child(StoreChild::Initializer) {
            Some(initializer) => {
                debug_assert_eq!(initializer, self.node.initializer(), "NodeRef initializer");
                initializer
            }
            None => node_ref_initializer(self.node),
        }
    }

    /// Go `node.Type()` (`Node::type_`).
    #[inline]
    #[must_use]
    pub fn type_(self) -> Node {
        match self.column_child(StoreChild::Type) {
            Some(type_) => {
                debug_assert_eq!(type_, self.node.type_(), "NodeRef type");
                type_
            }
            None => node_ref_type(self.node),
        }
    }

    /// Go `node.AsTypeReference().TypeName` (`Node::type_name`).
    #[inline]
    #[must_use]
    pub fn type_name(self) -> Node {
        match self.column_child(StoreChild::TypeName) {
            Some(type_name) => {
                debug_assert_eq!(type_name, self.node.type_name(), "NodeRef type name");
                type_name
            }
            None => node_ref_type_name(self.node),
        }
    }
}

// The `NodeRef` reads of a node that is not a published store node. Out of
// line, so the view's hot reads stay small.
#[cold]
#[inline(never)]
fn node_ref_kind(n: Node) -> SyntaxKind {
    n.kind()
}

#[cold]
#[inline(never)]
fn node_ref_flags(n: Node) -> NodeFlags {
    n.flags()
}

#[cold]
#[inline(never)]
fn node_ref_loc(n: Node) -> TextRange {
    n.loc()
}

#[cold]
#[inline(never)]
fn node_ref_parser_flags(n: Node, mask: NodeFlags) -> NodeFlags {
    n.parser_flags(mask)
}

#[cold]
#[inline(never)]
fn node_ref_modifier_flags(n: Node) -> ModifierFlags {
    n.modifier_flags()
}

#[inline(never)]
fn node_ref_name(n: Node) -> Node {
    n.name()
}

#[inline(never)]
fn node_ref_expression(n: Node) -> Node {
    n.expression()
}

#[inline(never)]
fn node_ref_initializer(n: Node) -> Node {
    n.initializer()
}

#[inline(never)]
fn node_ref_type_name(n: Node) -> Node {
    n.type_name()
}

#[inline(never)]
fn node_ref_type(n: Node) -> Node {
    n.type_()
}

/// `store_ast_node(n)` when `n` is a store node, in one lookup (see
/// `try_store_header`). `None` for nil and synthetic nodes, and for a node
/// with no `'static` data (a node that a freeable parse owns, lsshells M3c;
/// `static_store_node` tells them apart).
#[inline]
#[must_use]
pub fn try_store_ast_node(n: Node) -> Option<NodeData<'static>> {
    if n.is_nil() {
        return None;
    }
    match static_store_node(n) {
        StaticNode::Static(node) => Some(node),
        StaticNode::Scoped | StaticNode::NoStore => None,
    }
}

/// A node column entry (`FileStore::nodes`): the data of a node slot, as
/// Go's `nodeData` interface word (tag and reference, 16 B). A static parse
/// moves the data into the AST arena (`alloc_store_slot`); the entry of a
/// slot whose data a freeable parse owns is the marker (`owned_marker`).
type ColumnData = NodeData<'static>;

/// The node data of a node slot. Panics on the nil slot and alias slots.
#[inline]
fn slot_node(slot: Option<ColumnData>) -> NodeData<'static> {
    slot.expect("store handle does not name a node slot")
}

/// `resolve_store_id(file, id)` when `file` has a store, in one lookup (see
/// `try_store_header`). `None` when it has none.
#[inline]
#[must_use]
pub fn try_resolve_store_id(file: usize, id: crate::astdata::NodeId) -> Option<Node> {
    let index = id.index();
    // PERF: query Q8, see `ACTIVE`.
    if let Some(store) = active_store(file) {
        return Some(store.borrow().slot_resolve(file, index));
    }
    try_resolve_store_id_slow(file, index)
}

/// `try_resolve_store_id` for a file that is not the active store.
#[inline(never)]
fn try_resolve_store_id_slow(file: usize, index: usize) -> Option<Node> {
    if let Some(b) = file_block(file) {
        return Some(block_resolve_slot(b, file, index));
    }
    unpublished_store(file).map(|store| store.borrow().slot_resolve(file, index))
}

/// The text in the data of an Identifier or PrivateIdentifier node. Empty
/// for a store node made by `alloc_store_name_node` or
/// `alloc_store_shared_name_node` (see `store_identifier_name`) and for
/// other kinds.
fn identifier_text(data: NodeData<'_>) -> &str {
    match data {
        NodeData::Identifier(d) => &d.text,
        NodeData::PrivateIdentifier(d) => &d.text,
        _ => "",
    }
}

/// U1 (d): Go `node.Text()` of an Identifier or PrivateIdentifier store
/// node, from the name word of its slot (`NodeKids`), in a published store
/// (the node shell of a freeable file version included) or an unpublished
/// store of this thread. `None` for nil and synthetic nodes. `Name::default()` (the empty
/// text) for other slots.
#[inline]
#[must_use]
pub fn store_identifier_name(n: Node) -> Option<Name> {
    if n.is_nil() {
        return None;
    }
    let (file, index) = (n.file_index(), slot_index(n));
    if let Some(b) = file_block(file) {
        return Some(block_text_name_of(b, index));
    }
    store_identifier_name_slow(file, index)
}

/// U1 (a) (d): the name of slot `index` of the published store whose block
/// is `b`.
#[inline]
fn block_text_name_of(b: &FileBlock, index: usize) -> Name {
    b.kids[index].text_name(b.kind(index))
}

/// `store_identifier_name` for a node that is not published: an
/// unpublished (built or detached) store of this thread.
// PORT: the name word is the only copy of the text, so every store that a
// thread can read must answer, not only the published ones.
#[inline(never)]
fn store_identifier_name_slow(file: usize, index: usize) -> Option<Name> {
    unpublished_store(file).map(|store| store.borrow().slot_text_name_of(index))
}

/// The node for slot `index` of store `file`, whose records are `records`:
/// the slot itself when it holds a node, else the target in its record
/// (`NodeRecord::NO_NODE`).
#[inline]
fn record_resolve(file: usize, index: usize, records: &[NodeRecord]) -> Node {
    let record = &records[index];
    if record.is_node() {
        handle(file, index as u32)
    } else {
        record.target_node()
    }
}

/// Hook for `Node::new(file, id)` on a published store: `try_resolve_store_id(file, id)`
/// without a per-slot table load for an alias-free store. `None` for any
/// other file (unpublished or synthetic).
#[inline]
#[must_use]
pub fn frozen_resolve_store_id(file: usize, id: crate::astdata::NodeId) -> Option<Node> {
    file_block(file).map(|b| block_resolve_slot(b, file, id.index()))
}

/// The `StoreFacts` of published store `file`. `None` for any other file.
#[inline]
#[must_use]
pub fn frozen_store_facts(file: usize) -> Option<StoreFacts> {
    file_block(file).map(|b| b.file.facts())
}

/// The `StoreFacts` of the store of `n` when it is a published store node.
/// `None` for any other node: nil, synthetic and unpublished store
/// nodes.
#[inline]
#[must_use]
pub fn frozen_node_store_facts(n: Node) -> Option<StoreFacts> {
    if n.is_nil() {
        return None;
    }
    frozen_store_facts(n.file_index())
}

/// Go `SourceFile.ECMALineMap()` of published store file `file`. It is
/// computed once and shared by every thread. `None` for an unpublished
/// store or a file without a store. A freeable file version keeps it in its
/// store, so the guard pins the version.
#[must_use]
pub fn frozen_file_ecma_line_starts(file: usize) -> Option<FileRef<[i32]>> {
    if let Some(store) = file_block(file).and_then(|b| b.file.store) {
        return Some(FileRef::Static(line_starts(store)));
    }
    let version = published_version(file)?;
    Some(FileRef::Pinned {
        version,
        key: 0,
        get: |version, _| {
            let store = version
                .published()
                .expect("a pinned file version is published");
            line_starts(&store.store)
        },
    })
}

/// The ECMA line map of published store `file` (see
/// `frozen_file_ecma_line_starts`), made once.
fn line_starts(store: &FileStore) -> &[i32] {
    store.ecma_line_starts.get_or_init(|| {
        crate::scanner_util::compute_ecma_line_starts(&store.text).into_boxed_slice()
    })
}

/// `read` on the ECMA line map of published store `file`, with no guard: a
/// freeable file version is pinned only while `read` runs. `None` as for
/// `frozen_file_ecma_line_starts`, and then `read` did not run.
#[inline]
pub fn with_frozen_file_ecma_line_starts<R>(
    file: usize,
    read: impl FnOnce(&[i32]) -> R,
) -> Option<R> {
    with_published_store(file, |store| read(line_starts(store)))
}

/// Go `GetSourceFileOfNode(n)` in O(1), when `n` is a published store node
/// whose parent walk ends at its store root. `None` means "walk".
#[inline]
#[must_use]
pub fn frozen_source_file_of_node(n: Node) -> Option<Node> {
    if n.is_nil() {
        return None;
    }
    let b = file_block(n.file_index())?;
    b.records[slot_index(n)]
        .has_bit(NodeRecord::SOURCE_FILE_ROOT)
        .then(|| b.file.root())
}

/// U1 (a): Go `node.Text()` of a published Identifier or PrivateIdentifier
/// store node, interned when its slot was made (the
/// name word of its `NodeKids`). `None` for other kinds and for any other node:
/// nil, synthetic and unpublished store nodes.
#[inline]
#[must_use]
pub fn frozen_store_text_name(n: Node) -> Option<Name> {
    if n.is_nil() {
        return None;
    }
    let index = slot_index(n);
    let b = file_block(n.file_index())?;
    is_name_kind(b.kind(index)).then(|| Name::from_id(b.kids[index].word()))
}

/// U1 (a): Go `scanner.GetIdentifierToken(node.Text()) != KindIdentifier` of
/// a published Identifier or PrivateIdentifier store node, from its record.
/// `None` as for `frozen_store_text_name`.
#[inline]
#[must_use]
pub fn frozen_store_text_is_keyword(n: Node) -> Option<bool> {
    if n.is_nil() {
        return None;
    }
    let index = slot_index(n);
    let b = file_block(n.file_index())?;
    is_name_kind(b.kind(index)).then(|| b.records[index].has_bit(NodeRecord::TEXT_IS_KEYWORD))
}

/// U1 (b): Go `node.ModifierFlags()` (the flags of the node's own modifier
/// list) of a published store node, from the modifier
/// word of its `NodeKids`. `None` for any other node: nil, synthetic and
/// unpublished store nodes.
#[inline]
#[must_use]
pub fn frozen_store_modifier_flags(n: Node) -> Option<ModifierFlags> {
    if n.is_nil() {
        return None;
    }
    let index = slot_index(n);
    let b = file_block(n.file_index())?;
    Some(ModifierFlags(b.kids[index].modifier_bits(b.kind(index))))
}

/// U4 (CH6, bind A): Go `n.Name()`, `n.Expression()`, `n.PostfixToken()` or
/// `n.QuestionToken()` (`which`) of a published store node, from the
/// child fields of its `NodeKids` (`SlotChildren`),
/// resolved like `Node::new`. C2: also Go `n.Type()`, `n.Initializer()` and
/// `n.AsTypeReference().TypeName`. `None` when the entry is unknown and for
/// any other node (nil, synthetic, unpublished): the caller reads
/// the node data.
// PERF: inlinefix1. Always inline, here and in `column_store_child`: every
// caller passes a constant `which`, and the match of `column_store_child`
// folds to one arm. With `#[inline]` alone, LLVM kept an out-of-line copy
// (with the whole match) in some 16-codegen-unit release builds after
// unrelated changes moved code between units, and every `Node::expression`,
// `type_` and `initializer` read paid for it (+0.3 to +0.9% instructions
// in extchain1, linkgrow1 and pathsdiag1).
#[inline(always)]
#[must_use]
pub fn frozen_store_child(n: Node, which: StoreChild) -> Option<Node> {
    if n.is_nil() {
        return None;
    }
    column_store_child(file_block(n.file_index())?, n, which)
}

/// `frozen_store_child` in `b`, the block of `n`.
#[inline(always)]
fn column_store_child(b: &FileBlock, n: Node, which: StoreChild) -> Option<Node> {
    let file = n.file_index();
    let kids = b.kids.get(slot_index(n))?;
    let id = match which {
        StoreChild::Name => {
            let id = kids.name_id();
            if id == SlotChildren::UNKNOWN_ID {
                return None;
            }
            id
        }
        StoreChild::Expression | StoreChild::PostfixToken | StoreChild::QuestionToken => {
            let (tag, id) = SlotChildren::other_parts_of(kids.other());
            match (which, tag) {
                (_, SlotChildren::TAG_UNKNOWN) => return None,
                (StoreChild::Expression, SlotChildren::TAG_EXPRESSION)
                | (StoreChild::PostfixToken, SlotChildren::TAG_POSTFIX)
                | (StoreChild::QuestionToken, SlotChildren::TAG_QUESTION) => id,
                // Go `QuestionToken()` of a kind with a postfix token: that
                // token when it is a `?` token (`question_of_postfix`).
                (StoreChild::QuestionToken, SlotChildren::TAG_POSTFIX) => {
                    let postfix = column_child(b, file, id);
                    let is_question =
                        postfix.is_some() && postfix.kind() == SyntaxKind::QuestionToken;
                    return Some(if is_question { postfix } else { Node::NIL });
                }
                // The kind of `n` has no such field.
                _ => NIL_SLOT,
            }
        }
        StoreChild::Type | StoreChild::Initializer | StoreChild::TypeName => {
            let (tag, id) = SlotChildren::typed_parts_of(kids.typed());
            match (which, tag) {
                (_, SlotChildren::TYPED_UNKNOWN) => return None,
                (
                    StoreChild::Type,
                    SlotChildren::TYPED_TYPE | SlotChildren::TYPED_TYPE_WITH_INITIALIZER,
                )
                | (StoreChild::Initializer, SlotChildren::TYPED_INITIALIZER)
                | (StoreChild::TypeName, SlotChildren::TYPED_TYPE_NAME) => id,
                // An initializer that the column does not hold.
                (StoreChild::Initializer, SlotChildren::TYPED_TYPE_WITH_INITIALIZER) => {
                    return None;
                }
                // Go `AsTypeReference()` panics on other kinds; the data read
                // keeps the panic.
                (StoreChild::TypeName, _) => return None,
                // The field is nil, or the kind of `n` has no such field.
                _ => NIL_SLOT,
            }
        }
    };
    Some(column_child(b, file, id))
}

/// C2: true when `n` is a published store node whose Go `TypeArgumentList()`
/// is nil with no list in its node data (`SlotChildren` `NO_TYPE_ARGUMENTS`),
/// so `Node::type_argument_list` is `NodeList::NIL`. False when not known and
/// for any other node (nil, synthetic, unpublished).
#[inline]
#[must_use]
pub fn frozen_store_lacks_type_arguments(n: Node) -> bool {
    if n.is_nil() {
        return false;
    }
    file_block(n.file_index()).is_some_and(|b| {
        b.kids
            .get(slot_index(n))
            .is_some_and(|kids| SlotChildren::has_no_type_arguments_in(kids.typed()))
    })
}

/// R2-5: the children of a published store node in Go
/// `ForEachChild` order, from the link column of its store (`SlotLinks`),
/// without its node data. `None` when the chain of `n` is not known, for
/// a node of a node shell (the binder reads its version's column,
/// `version_child_links`) and for any other node (nil, synthetic,
/// unpublished): the caller uses `for_each_child`.
#[inline]
#[must_use]
pub fn frozen_store_children(n: Node) -> Option<StoreChildren<'static>> {
    if n.is_nil() {
        return None;
    }
    let file = n.file_index();
    let links = file_block(file)?.file.links;
    let first = links.get(slot_index(n))?.first_child;
    (first != LINK_NONE).then_some(StoreChildren {
        file,
        links,
        next: first,
    })
}

/// R2-5: the link column of freeable file version `file` (`node_shell`
/// keeps it in the version's store), pinned while the guard lives, for the
/// binder's child walk (`Binder::bind_each_child`). `None` for any other
/// file (a static publish has its column in its block, and
/// `frozen_store_children` reads it) and for a store with no link column.
#[must_use]
pub fn version_child_links(file: usize) -> Option<VersionChildLinks> {
    if file_block(file)?.file.store.is_some() {
        return None;
    }
    let links: FileRef<[SlotLinks]> = FileRef::Pinned {
        version: published_version(file)?,
        key: 0,
        get: |version, _| {
            &version
                .published()
                .expect("a pinned file version is published")
                .store
                .links
        },
    };
    (!links.is_empty()).then_some(VersionChildLinks { file, links })
}

/// R2-5: the link column of one freeable file version
/// (`version_child_links`). A clone shares the pin.
#[derive(Clone)]
pub struct VersionChildLinks {
    file: usize,
    links: FileRef<[SlotLinks]>,
}

impl VersionChildLinks {
    /// `frozen_store_children` for node `n` of this version: its children
    /// in Go `ForEachChild` order. `None` for a node of another file and
    /// when the chain of `n` is not known: the caller uses `for_each_child`.
    #[inline]
    #[must_use]
    pub fn children(&self, n: Node) -> Option<StoreChildren<'_>> {
        if n.is_nil() || n.file_index() != self.file {
            return None;
        }
        let links: &[SlotLinks] = &self.links;
        let first = links.get(slot_index(n))?.first_child;
        (first != LINK_NONE).then_some(StoreChildren {
            file: self.file,
            links,
            next: first,
        })
    }
}

/// R2-5: the iterator of `frozen_store_children` and
/// `VersionChildLinks::children`. A chain holds only node slots of its
/// store, so each child is the handle of its slot.
#[derive(Clone, Copy, Debug)]
pub struct StoreChildren<'a> {
    file: usize,
    links: &'a [SlotLinks],
    /// The next child, `LINK_END` at the end.
    next: u32,
}

impl Iterator for StoreChildren<'_> {
    type Item = Node;

    #[inline]
    fn next(&mut self) -> Option<Node> {
        if self.next == LINK_END {
            return None;
        }
        let index = self.next;
        self.next = self.links[index as usize].next_sibling;
        Some(handle(self.file, index))
    }
}

/// The node of child id `id` of published store `file`, whose block is
/// `b`, in a `children` entry.
#[inline]
fn column_child(b: &FileBlock, file: usize, id: u32) -> Node {
    // Slot 0 stands for Go nil, as for an optional field that is nil and a
    // kind without the field: it resolves to nil (the record of slot 0
    // never changes).
    if id == NIL_SLOT {
        return Node::NIL;
    }
    block_resolve_slot(b, file, id as usize)
}

/// U4 (CH7): what `frozen_find_ancestor` found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AncestorWalk {
    /// The first node for which the callback was true, or nil when the walk
    /// passed the root.
    Found(Node),
    /// The walk reached this parent, which is not in the store. Walk on
    /// from it.
    Next(Node),
}

/// U4 (CH7): the part of Go `FindAncestor(n, callback)` that is inside the
/// published store of `n`. `callback(node, kind)` gets `n` and then each
/// parent, with its Go `node.Kind`. The kind and header tables of the store
/// are found once, not once per `kind()` and `parent()` read. `None` when
/// `n` is not a published store node (nil included).
#[inline]
pub fn frozen_find_ancestor(
    n: Node,
    callback: impl FnMut(Node, SyntaxKind) -> bool,
) -> Option<AncestorWalk> {
    if n.is_nil() {
        return None;
    }
    let file = n.file_index();
    let b = file_block(file)?;
    let index = slot_index(n);
    if b.kinds.len() < b.records.len() {
        // A node shell with pooled kinds (`BlockFile::shell_kinds`) past
        // its plain prefix.
        return Some(store_find_ancestor(
            file,
            b.records,
            |slot| b.kind(slot),
            index,
            callback,
        ));
    }
    // The columns have one entry per slot. The same length lets LLVM drop
    // the bounds check of `kinds` in the loop.
    let kinds = &b.kinds[..b.records.len()];
    Some(store_find_ancestor(
        file,
        b.records,
        |slot| kinds[slot],
        index,
        callback,
    ))
}

/// `frozen_find_ancestor` in store `file`, whose records are `records` and
/// the kind of slot `i` is `kind(i)`, from slot `index`.
#[inline]
fn store_find_ancestor(
    file: usize,
    records: &[NodeRecord],
    kind: impl Fn(usize) -> SyntaxKind,
    mut index: usize,
    mut callback: impl FnMut(Node, SyntaxKind) -> bool,
) -> AncestorWalk {
    loop {
        let node = handle(file, index as u32);
        let record = &records[index];
        if callback(node, kind(index)) {
            return AncestorWalk::Found(node);
        }
        // The parent code (`ParentCode`): a node of this store, nil or a
        // node of another store.
        let code = record.parent_code();
        if code == 0 {
            return AncestorWalk::Found(Node::NIL);
        }
        if code & FOREIGN_PARENT != 0 {
            return AncestorWalk::Next(frozen_foreign_parent(
                file,
                (code & !FOREIGN_PARENT) as usize,
            ));
        }
        index = code as usize - 1;
    }
}

/// U4 (CH7): Go `n.Parent` and `n.Parent.Kind` of a published store node
/// whose parent is a node of the same store, with one store lookup. `None` for
/// any other node or parent (nil, another store): the caller reads them one
/// by one.
#[inline]
#[must_use]
pub fn frozen_store_parent_kind(n: Node) -> Option<(Node, SyntaxKind)> {
    if n.is_nil() {
        return None;
    }
    let file = n.file_index();
    let b = file_block(file)?;
    b.records[slot_index(n)]
        .local_parent()
        .map(|index| (handle(file, index as u32), b.kind(index)))
}

/// U4 (CH7): true when `n` is a published store node and no node of its store
/// has `POSSIBLY_CONTAINS_DEPRECATED_TAG` (`StoreFacts::has_deprecated_tag`)
/// while every parent in the store is local (`parents_local`). Then no
/// ancestor of `n` has the bit, so Go `GetCombinedNodeFlags(n)` lacks it
/// and `IsDeprecatedDeclaration(n)` is false. False for any other node.
#[inline]
#[must_use]
pub fn frozen_store_lacks_deprecated_tag(n: Node) -> bool {
    if n.is_nil() {
        return false;
    }
    file_block(n.file_index()).is_some_and(|b| {
        let facts = b.file.facts();
        facts.parents_local && !facts.has_deprecated_tag
    })
}

/// U1 (c): how `Node::new(file, id)` maps the child ids of one published
/// store, read once for a loop over many ids (`NodeSliceIter`).
#[derive(Clone, Copy, Debug)]
pub enum FrozenIds {
    /// A store with alias slots: its records (`record_resolve`), and `base`
    /// as for `Direct`.
    Records {
        base: u64,
        records: &'static [NodeRecord],
    },
    /// An alias-free store: id 0 is nil (what slot 0 resolves to), and
    /// any other id is the handle `base | (id + 1)`, `base` = `file << 32`.
    Direct { base: u64 },
}

impl FrozenIds {
    /// `Node::new(file, id)` for the store this value was read for.
    #[inline]
    #[must_use]
    pub fn node(self, id: crate::astdata::NodeId) -> Node {
        match self {
            FrozenIds::Records { base, records } => {
                let record = &records[id.index()];
                if record.is_node() {
                    Node(base | (id.index() as u64 + 1))
                } else {
                    record.target_node()
                }
            }
            FrozenIds::Direct { base } => {
                if id.index() == NIL_SLOT as usize {
                    Node::NIL
                } else {
                    Node(base | (id.index() as u64 + 1))
                }
            }
        }
    }
}

/// U1 (c): the `FrozenIds` of published store `file`. `None` for any other
/// file (unpublished or synthetic): the caller resolves each id.
#[inline]
#[must_use]
pub fn frozen_store_ids(file: usize) -> Option<FrozenIds> {
    file_block(file).map(|b| block_ids(b, file))
}

/// The `FrozenIds` of published store `file`, whose block is `b`.
#[inline]
fn block_ids(b: &FileBlock, file: usize) -> FrozenIds {
    let base = (file as u64) << 32;
    if b.file.facts().alias_free {
        FrozenIds::Direct { base }
    } else {
        FrozenIds::Records {
            base,
            records: b.records,
        }
    }
}

/// U1 (e): the capacity hint for the flow nodes of the bind of published
/// store file `file`, static or a freeable file version. `None` for an
/// unpublished store and a file without a store.
#[must_use]
pub fn frozen_store_bind_estimate(file: usize) -> Option<usize> {
    with_published_store(file, |store| store.bind_estimate.flow_nodes as usize)
}

// ──────────────────────────────────────────────────────────────────────
// Go field writes during the parse
// ──────────────────────────────────────────────────────────────────────

/// The cell of the unpublished store of this thread (built or detached)
/// that holds `n`, with one thread-local access. `None` when `n` is not a
/// node of such a store (for example nil, a synthetic or a published node).
#[inline]
fn thread_build_cell(n: Node) -> Option<StoreCell> {
    if n.is_nil() {
        return None;
    }
    let file = n.file_index();
    // PERF: query Q8. The parse writes the active store, which is never
    // published (see `ACTIVE`).
    if let Some(store) = active_store(file) {
        return Some(store);
    }
    // A published node or a synthetic node leaves here, inline.
    if is_storeless_id(file) || file_block(file).is_some() {
        return None;
    }
    inactive_build_store(file)
}

/// Runs `f` on the store and the slot index of `n` when `n` is a node slot
/// of an unpublished store of this thread (built or detached), with one
/// thread-local access. False when `n` is not such a node (for example a
/// synthetic or a published node). Panics on a finished store, like
/// `with_slot_mut`.
fn try_with_build_slot(n: Node, f: impl FnOnce(&mut FileStore, usize)) -> bool {
    let Some(store) = thread_build_cell(n) else {
        return false;
    };
    let mut s = store.borrow_mut();
    assert!(!s.frozen, "cannot mutate a node of a finished file");
    let index = slot_index(n);
    assert!(
        s.nodes[index].is_some(),
        "store handle does not name a node slot"
    );
    f(&mut *s, index);
    true
}

/// What `finish_store_node_and_parents` wrote.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FinishedStoreNode {
    /// Nothing: the node is not a node of an unfinished store of this
    /// thread.
    NotStore,
    /// The loc and the flags. The parents of the children are not all set
    /// (an alias child, a nil list entry): the caller runs the generic walk,
    /// as after `set_parent_in_store_children` gave false.
    LocAndFlags,
    /// The loc, the flags, and the parents and the R2-5 chain of the
    /// children (`set_parent_in_store_children`).
    All,
}

/// parser.go `finishNodeWithEnd` on a node of an unfinished store of this
/// thread, in one store access: `node.Loc = loc`, `node.Flags |= flags`, and
/// then `overrideParentInImmediateChildren` as `set_parent_in_store_children`
/// does it. Panics on a finished store, like `with_slot_mut`.
// PERF: perfP2 item 2. One thread-local lookup, `RefCell` borrow and slot
// check per finished node, not one for the loc and flags and one more for
// the parents.
pub fn finish_store_node_and_parents(
    n: Node,
    loc: TextRange,
    flags: NodeFlags,
) -> FinishedStoreNode {
    let Some(store) = thread_build_cell(n) else {
        return FinishedStoreNode::NotStore;
    };
    let mut guard = store.borrow_mut();
    let s = &mut *guard;
    assert!(!s.frozen, "cannot mutate a node of a finished file");
    let index = slot_index(n);
    // PERF: storefast1. One record access for the slot check (the record
    // bit agrees with the node column, `debug_check_kids`), the loc and the
    // flags. Only a node with child fields reads its node column.
    let record = &mut s.records[index];
    assert!(record.is_node(), "store handle does not name a node slot");
    record.set_loc(loc);
    let flags = record.flags() | flags;
    record.set_flags(flags);
    s.build_facts.flags(flags);
    // PERF: storefast1. A leaf (Go `ForEachChild` has no case for its
    // kind, so the walk visits nothing) only gets its empty R2-5 chain:
    // what `set_slot_child_parents` writes for it, with no walk. Tokens,
    // identifiers and literals are about half of the finished nodes. Go
    // `overrideParentInImmediateChildren` on a leaf is a call that returns.
    if super::node::has_no_child_fields(s.kinds[index]) {
        s.unlink_children(index);
        s.build_links[index].first_child = LINK_END;
        return FinishedStoreNode::All;
    }
    let node = s.nodes[index].expect("a node slot has node data");
    if s.set_slot_child_parents(index, node) {
        FinishedStoreNode::All
    } else {
        FinishedStoreNode::LocAndFlags
    }
}

/// Go `node.Parent = parent` on a node of an unfinished store, in one store
/// access. False (nothing written) when `n` is not a store node of this
/// thread.
pub fn try_set_store_node_parent(n: Node, parent: Node) -> bool {
    let parent = NodeHeader::stored_parent(n.file_index(), parent);
    try_with_build_slot(n, |s, index| s.set_slot_parent(index, parent))
}

/// R2-5: Go `child.Parent = parent` for each child of `parent` in Go
/// `ForEachChild` order (parser.go `overrideParentInImmediateChildren`),
/// which also makes the chain of `parent` in the link column (`SlotLinks`).
/// `new` starts the chain; `set_parent` sets the parent of one child and
/// links it, in the same store access.
pub struct StoreChildLinks {
    parent: Node,
    /// The last child linked so far, `LINK_END` before the first.
    last: u32,
    /// True while the chain of `parent` is made. False when `parent` is not
    /// a node of an unfinished store of this thread, or a child could not be
    /// linked (the chain of `parent` is then unknown).
    linking: bool,
}

impl StoreChildLinks {
    /// Starts the chain of `parent`: frees its old chain (the parser can set
    /// the parents of the same children again) and marks it as a node with
    /// no child yet.
    #[must_use]
    pub fn new(parent: Node) -> Self {
        let linking = thread_build_cell(parent).is_some_and(|store| {
            let mut s = store.borrow_mut();
            let index = slot_index(parent);
            // Nothing to link in a finished file; a child write panics there.
            if s.frozen || s.nodes[index].is_none() {
                return false;
            }
            s.unlink_children(index);
            s.build_links[index].first_child = LINK_END;
            true
        });
        Self {
            parent,
            last: LINK_END,
            linking,
        }
    }

    /// Go `child.Parent = parent` for the next child of `parent`, and links
    /// `child` after the children before it. False (nothing written) when
    /// `child` is not a store node of this thread; the caller then sets the
    /// parent itself (`set_node_parent`).
    pub fn set_parent(&mut self, child: Node) -> bool {
        let parent = self.parent;
        let stored = NodeHeader::stored_parent(child.file_index(), parent);
        // A chain holds only node slots of the store of `parent`.
        let link = self.linking && child.file_index() == parent.file_index();
        let last = &mut self.last;
        let mut linked = false;
        let written = try_with_build_slot(child, |s, index| {
            s.set_slot_parent(index, stored);
            linked = link && s.link_child(slot_index(parent), last, index);
        });
        if self.linking && !linked {
            // PORT: Go has no such case; only the link column cannot hold it
            // (see `SlotLinks`). The binder then uses Go `ForEachChild`.
            self.linking = false;
            try_with_build_slot(self.parent, |s, index| s.unlink_children(index));
        }
        written
    }
}

/// R3-2: parser.go `overrideParentInImmediateChildren` for `parent` in one
/// borrow of its store: Go `child.Parent = parent` for each child in Go
/// `ForEachChild` order (`for_each_store_child_id`), and the R2-5 chain of
/// `parent`, with the same writes as `StoreChildLinks`. False when `parent`
/// is not a node of an unfinished store of this thread, or when a child is
/// not a node slot of that store (an alias slot, or a nil list entry). The
/// caller then runs the `StoreChildLinks` walk, which gives the same result
/// from any state this left: it frees the chain of `parent` first, and sets
/// the same parents again.
// PERF: R3-2. The `StoreChildLinks` walk found the store once per child
// (`try_with_build_slot`: the registry check, the thread-local lookup and a
// `RefCell` borrow) and read the data and each child through the node
// reads. This reads the data from the slot and writes the children by slot
// index.
pub fn set_parent_in_store_children(parent: Node) -> bool {
    let Some(store) = thread_build_cell(parent) else {
        return false;
    };
    let mut guard = store.borrow_mut();
    let s = &mut *guard;
    let index = slot_index(parent);
    // A finished file: the `StoreChildLinks` walk panics on the first write.
    if s.frozen {
        return false;
    }
    let Some(node) = s.nodes[index] else {
        return false;
    };
    s.set_slot_child_parents(index, node)
}

impl FileStore {
    /// `set_parent_in_store_children` for node slot `index` of this
    /// unfinished store, whose node column entry is `node`.
    #[inline]
    fn set_slot_child_parents(&mut self, index: usize, node: NodeData<'static>) -> bool {
        // lsshells M3c: a node that the store owns (a freeable parse).
        if self.cell_of(index) != NO_CELL {
            return set_parent_in_owned_store_children(self, index);
        }
        let kind = self.kinds[index];
        // The `ParentCode` of `parent` in a child of its store
        // (`set_slot_parent` of a `LOCAL_STORE` parent).
        let code = local_parent_code(handle(LOCAL_STORE, index as u32));
        // `StoreChildLinks::new`.
        self.unlink_children(index);
        self.build_links[index].first_child = LINK_END;
        // The fields are borrowed apart, so the walk reads the list table
        // while the records and the links change. Go's walk cannot panic
        // here; only a port bug can (a list or child id that is not in the
        // store). When an LSP request recovers that panic, the table is
        // still in place.
        let FileStore {
            lists,
            records,
            build_links,
            ..
        } = self;
        let sink = ParentLinks {
            records,
            links: build_links,
            parent: index,
            code,
            last: LINK_END,
            linking: true,
        };
        !super::node::for_each_store_child_id(kind, node, ListIds::Static(lists), sink)
    }
}

/// `set_slot_child_parents` for each child: Go `child.Parent = parent`
/// (`StoreChildLinks::set_parent`) and the next link of the R2-5 chain of
/// `parent`, whose last child so far is `last`. `code` is the `ParentCode`
/// of `parent` in its store; `linking` is false once a child could not be
/// linked (the chain of `parent` is then unknown).
struct ParentLinks<'a> {
    records: &'a mut [NodeRecord],
    links: &'a mut [SlotLinks],
    parent: usize,
    code: ParentCode,
    last: u32,
    linking: bool,
}

impl super::node::StoreChildSink for ParentLinks<'_> {
    // PERF: storefast1. Inlined into each arm of the child walk: no call
    // per child, and the state stays in registers.
    #[inline(always)]
    fn child(&mut self, child: u32) -> bool {
        let child = child as usize;
        // PERF: perfP2 item 2. The record bit, not the node column: the
        // parent write below reads and writes the same record.
        if !self.records[child].is_node() {
            return true;
        }
        self.records[child].set_parent_code(self.code);
        if self.linking && !link_child(self.links, self.parent, &mut self.last, child) {
            self.linking = false;
            unlink_children(self.links, self.parent);
        }
        false
    }
}

/// `set_parent_in_store_children` for slot `index` of `s`, whose node the
/// store owns (lsshells M3c). The fields are borrowed apart, so the node
/// data is read while the records and the links change.
#[inline(never)]
fn set_parent_in_owned_store_children(s: &mut FileStore, index: usize) -> bool {
    let FileStore {
        nodes,
        records,
        kinds,
        build_links,
        owned,
        ..
    } = s;
    let owned = owned.as_deref().expect("a store that owns its nodes");
    let node = owned.node(owned.cell_of[index]);
    let kind = kinds[index];
    let code = local_parent_code(handle(LOCAL_STORE, index as u32));
    unlink_children(build_links, index);
    build_links[index].first_child = LINK_END;
    let mut last = LINK_END;
    let mut linking = true;
    let lists = ListIds::Owned(&owned.pending);
    let stopped = super::node::for_each_store_child_id(kind, node, lists, |child| {
        let child = child as usize;
        if nodes[child].is_none() {
            return true;
        }
        records[child].set_parent_code(code);
        if linking && !link_child(build_links, index, &mut last, child) {
            linking = false;
            unlink_children(build_links, index);
        }
        false
    });
    !stopped
}

/// R3-2, debug builds: the slot index, stored parent and R2-5 links of
/// `parent` and of each child that the generic walk (`Node::iter_children`)
/// visits. Every child must be a node of the store of `parent`, an
/// unfinished store of this thread (`set_parent_in_store_children` made
/// its parents).
#[cfg(debug_assertions)]
pub fn debug_store_child_link_state(parent: Node) -> Vec<(u32, Node, u32, u32)> {
    // The walk reads the store, so it runs before the borrow below.
    let children = parent.iter_children();
    let store = thread_build_cell(parent).expect("R3-2: parent is not a build store node");
    let s = store.borrow();
    std::iter::once(parent)
        .chain(children)
        .map(|n| {
            assert_eq!(
                n.file_index(),
                parent.file_index(),
                "R3-2: a child in another store"
            );
            let index = slot_index(n);
            let links = s.build_links[index];
            (
                index as u32,
                s.slot_stored_parent(index),
                links.first_child,
                links.next_sibling,
            )
        })
        .collect()
}

/// Go `node.Parent = parent` on a node of an unfrozen file.
pub fn set_store_node_parent(n: Node, parent: Node) {
    let parent = NodeHeader::stored_parent(n.file_index(), parent);
    with_slot_mut(n, |s, index| s.set_slot_parent(index, parent));
}

/// Go `node.Loc = loc` on a node of an unfrozen file.
pub fn set_store_node_loc(n: Node, loc: TextRange) {
    with_slot_mut(n, |s, index| s.set_slot_loc(index, loc));
}

/// Go `node.Flags = flags` on a node of an unfrozen file.
pub fn set_store_node_flags(n: Node, flags: NodeFlags) {
    with_slot_mut(n, |s, index| s.set_slot_flags(index, flags));
}

/// Go write to a data field of a node of an unfrozen file (reparser.go).
/// The new data replaces the old; the old data leaks (a freeable parse
/// keeps it in its store until the store is freed, so a list handle taken
/// before the write still reads the old list). The U1 and U4 build entries
/// and the keyword bit of the slot follow the new data, and its R2-5 chain
/// becomes unknown.
pub fn replace_store_node_data(n: Node, data: BoxedData) {
    with_store_mut(n.file_index(), |s| {
        assert!(!s.frozen, "cannot mutate a node of a finished file");
        let index = slot_index(n);
        if s.nodes[index].is_none() {
            panic!("store handle does not name a node slot");
        }
        // The same code as `alloc_store_node`, on the slot kind, which a
        // data write keeps (the kind `debug_check_kids` reads).
        let kind = s.kinds[index];
        let view = data.view();
        debug_assert!(
            view.matches_syntax_kind(kind),
            "{kind:?} does not fit its NodeData"
        );
        // PORT: U1 (d). Data cloned from a store identifier has an empty text
        // (`alloc_store_name_node`, `alloc_store_shared_name_node`), so an
        // empty text keeps the slot name. A new text replaces it. Go writes
        // no empty identifier text here. S1: the new data gets a new leak;
        // the shared name data does not change.
        let keeps_name = is_name_kind(kind) && identifier_text(view).is_empty();
        let owned = s.owned.is_some();
        let modifier_bits =
            super::node::store_node_modifier_bits(kind, view, |m| s.modifier_list_bits(owned, m));
        if !keeps_name {
            let (name, text_is_keyword) = s.slot_text_name(kind, view, None);
            s.records[index].set_bit(NodeRecord::TEXT_IS_KEYWORD, text_is_keyword);
            s.kids[index].set_word(NodeKids::word_of(kind, &name, modifier_bits));
        }
        // U4: a reparser write can change a child that the kids hold. C2:
        // the same for the `typed` field.
        s.kids[index].set_children(super::node::store_node_children(kind, view));
        // lsshells M3c: a freeable parse keeps the new data in a new cell.
        match s.owned.as_deref_mut() {
            Some(owned) => {
                let cell = owned.push(data);
                owned.cell_of[index] = cell;
                s.nodes[index] = Some(owned_marker());
            }
            // astchunks2: the box stays (leaked); the column views its
            // payload. Rare: a reparser write.
            None => s.nodes[index] = Some(leak_in_ast_arena(data).view()),
        }
        // R2-5: the children of the node can change, so its chain is freed
        // and unknown until the parser sets their parents again
        // (`StoreChildLinks`; reparser.go `finishMutatedNode`).
        s.unlink_children(index);
    });
}

// ──────────────────────────────────────────────────────────────────────
// Lib parse snapshot (R3-1, frontend/parser/lib_parse_snapshot.rs)
// ──────────────────────────────────────────────────────────────────────

/// R3-1, astshrink1: the static list table of the store that a lib parse
/// snapshot loads into. The snapshot decoder adds the lists of each node
/// data here before the slot, and the data names them by index.
pub(crate) struct LibListTable<'a>(&'a mut Vec<&'static StaticList>);

impl LibListTable<'_> {
    /// Adds a list at `range` over `ids` with missing-list bit `comma` and
    /// modifier flags `flags`, and returns its astdata list field. The Go
    /// `nil` marker of a required list field is entry `NIL_LIST_INDEX`.
    pub(crate) fn push(
        &mut self,
        range: crate::astdata::text::TextRange,
        ids: &[crate::astdata::NodeId],
        comma: bool,
        flags: crate::astdata::ModifierFlags,
    ) -> crate::astdata::NodeList {
        if is_nil_list_range(&range) && ids.is_empty() && !comma && flags.0 == 0 {
            return crate::astdata::NodeList::new(NIL_LIST_INDEX);
        }
        let arena: &'static LeakArena = AST_ARENA.with(|a| *a);
        let ids = arena.alloc_slice_fill_iter(ids.len(), ids.iter().copied());
        let list = push_static_list(self.0, arena, range, ids, StaticList::bits(comma, flags));
        crate::astdata::NodeList::new(list.index)
    }
}

/// R3-1: one node slot of a lib parse snapshot: what the parse left in the
/// slot when it froze the file, besides what the freeze and the U1 (b) and
/// U4 kids entries make from it. A snapshot store has no alias slot, and slot
/// 0 (nil) is not in the snapshot.
pub(crate) struct LibParseSlot {
    pub(crate) kind: SyntaxKind,
    pub(crate) flags: NodeFlags,
    pub(crate) loc: TextRange,
    /// The slot of the Go parent, 0 for nil.
    pub(crate) parent: u32,
    /// The R2-5 links (`SlotLinks`): a slot, `Self::LINK_END` or
    /// `Self::LINK_NONE`.
    pub(crate) first_child: u32,
    pub(crate) next_sibling: u32,
    /// The node data. `None`: the slot points at the shared name data of
    /// `kind` (S1, `alloc_store_shared_name_node`).
    pub(crate) data: Option<NodeData<'static>>,
    /// The name word and the keyword bit (`slot_text_name`):
    /// `Name::default()` and false for a kind other than Identifier and
    /// PrivateIdentifier.
    pub(crate) name: Name,
    pub(crate) text_is_keyword: bool,
}

impl LibParseSlot {
    pub(crate) const LINK_END: u32 = LINK_END;
    pub(crate) const LINK_NONE: u32 = LINK_NONE;
}

/// R3-1: fills store `file`, which `new_file_store` or
/// `new_detached_file_store` just made on this thread, with `count` slots
/// after slot 0 from `next`, in slot order, as the parse made them: the
/// record, data, name and link entries of each slot, and the U1 (b) and U4
/// kids entries made from the data as `alloc_store_slot_node` makes them.
/// The caller freezes the store, which makes the other tables as for a live
/// parse. False when `next` fails; the store then holds part of the slots
/// (`reset_file_store` empties it).
// PERF: R3-1. One store borrow for all slots. The vectors are made at their
// final size, which `freeze` keeps.
pub(crate) fn load_lib_parse_slots(
    file: usize,
    count: usize,
    mut next: impl FnMut(&mut LibListTable) -> Option<LibParseSlot>,
) -> bool {
    with_store_mut(file, |s| {
        assert!(
            !s.frozen && s.records.len() == 1,
            "a lib parse snapshot loads into a new store"
        );
        s.records.reserve_exact(count);
        s.kinds.reserve_exact(count);
        s.kids.reserve_exact(count);
        s.nodes.reserve_exact(count);
        s.build_links.reserve_exact(count);
        for _ in 0..count {
            let Some(slot) = next(&mut LibListTable(&mut s.lists)) else {
                return false;
            };
            s.push_lib_parse_slot(slot);
        }
        // PORT: a snapshot node data is leaked in the AST arena, also in a store
        // that owns its nodes (a lib file is not edited), so no slot has a
        // cell.
        if let Some(owned) = s.owned.as_deref_mut() {
            owned.cell_of.resize(s.records.len(), NO_CELL);
        }
        s.debug_assert_build_columns();
        true
    })
}

impl FileStore {
    /// `load_lib_parse_slots` for one slot.
    fn push_lib_parse_slot(&mut self, slot: LibParseSlot) {
        let LibParseSlot {
            kind,
            flags,
            loc,
            parent,
            first_child,
            next_sibling,
            data,
            name,
            text_is_keyword,
        } = slot;
        let node = match data {
            Some(data) => data,
            None => shared_name_node(kind),
        };
        // The `ParentCode` of a parent in this store (nil for `NIL_SLOT`).
        let parent = if parent == NIL_SLOT {
            0
        } else {
            local_parent_code(handle(LOCAL_STORE, parent))
        };
        let bits = if text_is_keyword {
            NodeRecord::TEXT_IS_KEYWORD
        } else {
            0
        };
        self.records
            .push(NodeRecord::new(bits, flags, loc, u64::from(parent)));
        self.kinds.push(kind);
        self.build_facts.node(kind);
        self.build_facts.flags(flags);
        self.nodes.push(Some(node));
        let (modifiers, children) = slot_kids_parts(kind, node);
        let modifier_bits = self.static_modifier_bits(modifiers);
        self.kids.push(NodeKids::new(
            children,
            NodeKids::word_of(kind, &name, modifier_bits),
        ));
        self.build_links.push(SlotLinks {
            first_child,
            next_sibling,
        });
    }
}

/// R3-1: empties store `file`, an unfinished store of this thread, to what
/// `new_file_store` made, after a snapshot load failed part way. The file
/// is then parsed into it, so its id does not change.
pub(crate) fn reset_file_store(file: usize) {
    with_store_mut(file, |s| {
        assert!(!s.frozen, "cannot reset a finished store");
        let owns_nodes = s.owned.is_some();
        let idents = std::mem::take(&mut s.idents);
        *s = FileStore::new(s.file_name, s.text.clone());
        s.idents = idents;
        if owns_nodes {
            s.owned = Some(Box::new(OwnedAst::new(s.records.capacity())));
        }
    });
}

/// R3-1, tests: one slot of a finished store as a snapshot keeps it
/// (`LibParseSlot`), with the data by reference.
#[cfg(test)]
pub(crate) struct LibParseSlotView {
    pub(crate) kind: SyntaxKind,
    pub(crate) flags: NodeFlags,
    pub(crate) loc: TextRange,
    pub(crate) parent: u32,
    pub(crate) first_child: u32,
    pub(crate) next_sibling: u32,
    pub(crate) data: NodeData<'static>,
    /// The slot points at the shared name data of its kind (S1).
    pub(crate) shared_name: bool,
    pub(crate) name: Name,
    pub(crate) text_is_keyword: bool,
}

/// R3-1, tests: the slots after slot 0 of store `file`, a store of this
/// thread whose parse is finished and that is not published. An error when
/// a snapshot cannot keep the store: an alias slot, a parent in another
/// store, or node data that does not fit the slot kind.
#[cfg(test)]
pub(crate) fn lib_parse_slot_views(file: usize) -> Result<Vec<LibParseSlotView>, String> {
    with_store(file, |s| {
        if !s.frozen || !s.facts_made || s.links.len() != s.records.len() {
            return Err("the store is not frozen".into());
        }
        if !s.aliases.is_empty() {
            return Err("the store has alias slots".into());
        }
        let mut views = Vec::with_capacity(s.records.len());
        for index in 1..s.records.len() {
            let record = &s.records[index];
            let kind = s.kinds[index];
            let data = s.nodes[index].ok_or_else(|| format!("slot {index} is not a node slot"))?;
            if !data.matches_syntax_kind(kind) {
                return Err(format!("slot {index}: the node data does not fit {kind:?}"));
            }
            let parent = match (record.local_parent(), record.parent_code()) {
                (_, 0) => NIL_SLOT,
                (Some(parent), _) => parent as u32,
                _ => return Err(format!("slot {index}: a parent in another store")),
            };
            let shared_name = is_name_kind(kind) && data.same(shared_name_node(kind));
            let links = s.links[index];
            views.push(LibParseSlotView {
                kind,
                flags: record.flags(),
                loc: record.loc(),
                parent,
                first_child: links.first_child,
                next_sibling: links.next_sibling,
                data,
                shared_name,
                name: s.kids[index].text_name(kind),
                text_is_keyword: record.has_bit(NodeRecord::TEXT_IS_KEYWORD),
            });
        }
        Ok(views)
    })
}

/// R3-1, tests: the list table of store `file`, a static store of this
/// thread (astshrink1).
#[cfg(test)]
pub(crate) fn lib_parse_list_table(file: usize) -> Vec<&'static StaticList> {
    with_store(file, |s| s.lists.clone())
}

/// R3-1, tests: one line for each part of store `file` (a store of this
/// thread whose parse is finished) that the snapshot load and the freeze
/// make, in a form that does not depend on the store id, so the store of a
/// live parse and of a snapshot load can be compared. The node data is not
/// in it (the snapshot test compares it field by field).
#[cfg(test)]
pub(crate) fn lib_parse_store_dump(file: usize) -> Vec<String> {
    let local = |n: Node| {
        if n.is_nil() {
            "nil".to_string()
        } else if n.file_index() == file {
            format!("#{}", slot_index(n))
        } else {
            format!("{n:?}")
        }
    };
    with_store(file, |s| {
        let mut jsdoc: Vec<String> = s
            .jsdoc_cache
            .iter()
            .map(|(&n, jsdocs)| {
                let list: Vec<String> = jsdocs.iter().map(|&j| local(j)).collect();
                format!("{} {list:?}", local(n))
            })
            .collect();
        jsdoc.sort();
        let diagnostics: Vec<String> = s
            .diagnostics
            .iter()
            .map(|d| {
                format!(
                    "{} {} {} {} {} {:?}",
                    local(d.file),
                    d.pos,
                    d.end,
                    d.code,
                    d.message.key(),
                    d.message_args
                )
            })
            .collect();
        let mut lines = vec![
            format!("file {} text {}", s.file_name, s.text.len()),
            format!(
                "frozen {} facts {} root_slot {} root {} aliases {}",
                s.frozen,
                s.facts_made,
                s.root_slot,
                local(s.root),
                s.aliases.len(),
            ),
            format!("parser_flags {:?}", s.parser_flags),
            format!("jsdoc {jsdoc:?}"),
            format!(
                "lazy {:?} lazy_cache {}",
                s.lazy_js_doc,
                s.lazy_jsdoc_cache.len()
            ),
            format!(
                "variant {:?} diagnostics {diagnostics:?}",
                s.language_variant
            ),
            format!("facts {:?} bind {:?}", s.facts, s.bind_estimate),
            format!(
                "build {} {}",
                s.build_links.len(),
                s.ecma_line_starts.get().is_some()
            ),
            format!(
                "columns {} {} {} {}",
                s.records.len(),
                s.kids.len(),
                s.nodes.len(),
                s.links.len()
            ),
        ];
        for index in 0..s.records.len() {
            let kind = s.kinds[index];
            let shared = is_name_kind(kind)
                && s.nodes[index].is_some_and(|data| data.same(shared_name_node(kind)));
            let record = &s.records[index];
            let kids = &s.kids[index];
            lines.push(format!(
                "{index}: record {:?} {} {:?} {:?} {} bind {} node {} shared {shared} kids {:?} {:?} {} links {:?}",
                kind,
                record.bits(),
                record.flags(),
                record.loc(),
                local(record.header(kind, file, &s.foreign_parents).parent),
                record.bind.load(Ordering::Relaxed),
                s.nodes[index].is_some(),
                kids.children(),
                kids.text_name(kind).as_str(),
                kids.modifier_bits(kind),
                s.links.get(index),
            ));
        }
        lines
    })
}

// ──────────────────────────────────────────────────────────────────────
// Allocation (used by factory.rs through `NodeFactory::for_file`)
// ──────────────────────────────────────────────────────────────────────

thread_local! {
    /// Parsed AST nodes and lists live for the whole process. One leaked
    /// bump arena per parsing thread holds them, so each node costs a
    /// pointer bump, not a malloc. The arena never drops, like the
    /// `Box::leak` it replaces. Synthetic nodes are not here: the synthetic
    /// arena (`ast/synthetic.rs`) owns them, so checker workers make no AST
    /// arena. rss2: it grows in fixed-size chunks (`LeakArena`).
    static AST_ARENA: &'static LeakArena = LeakArena::leak();
}

/// Moves `value` into this thread's leaked AST arena.
// PERF: U1 (b). Only the arena reference comes out of the out-of-line
// `LocalKey::with`, so `value` is not copied through its closure.
#[inline]
pub(crate) fn leak_in_ast_arena<T>(value: T) -> &'static T {
    let arena: &'static LeakArena = AST_ARENA.with(|a| *a);
    arena.alloc(value)
}

/// S1: the one node data that every store Identifier (or
/// PrivateIdentifier, by `kind`) made by `alloc_store_shared_name_node`
/// points at. Its data is the Go factory payload with an empty text (the
/// name column holds the text).
// astchunks2: `static`, not `const`, so the data has one address (the
// snapshot writer finds shared names by `NodeData::same`).
fn shared_name_node(kind: SyntaxKind) -> ColumnData {
    static IDENTIFIER: crate::astdata::IdentifierData = crate::astdata::IdentifierData {
        text: String::new(),
    };
    static PRIVATE_IDENTIFIER: crate::astdata::PrivateIdentifierData =
        crate::astdata::PrivateIdentifierData {
            text: String::new(),
        };
    match kind {
        SyntaxKind::Identifier => NodeData::Identifier(&IDENTIFIER),
        SyntaxKind::PrivateIdentifier => NodeData::PrivateIdentifier(&PRIVATE_IDENTIFIER),
        _ => panic!("{kind:?} is not a name kind"),
    }
}

/// S1, debug builds: panics unless `data` (the payload the factory builds
/// for a store name node of kind `kind`) equals the data of the shared node
/// of `kind`. The struct patterns name every field, so a new astdata field
/// does not compile here until it is checked.
#[cfg(debug_assertions)]
pub fn debug_assert_shared_name_data(kind: SyntaxKind, data: NodeData<'_>) {
    let same = match (data, shared_name_node(kind)) {
        (NodeData::Identifier(a), NodeData::Identifier(b)) => {
            let crate::astdata::IdentifierData { text } = a;
            *text == b.text
        }
        (NodeData::PrivateIdentifier(a), NodeData::PrivateIdentifier(b)) => {
            let crate::astdata::PrivateIdentifierData { text } = a;
            *text == b.text
        }
        _ => false,
    };
    assert!(
        same,
        "S1: the factory payload of a store {kind:?} differs from the shared node"
    );
}

/// Go `newNode(kind, data, hooks)` in store `file`: `Loc =
/// UndefinedTextRange()`, nil parent, no flags.
#[inline]
pub fn alloc_store_node<T: AstData>(file: usize, kind: SyntaxKind, data: T) -> Node {
    alloc_store_slot(file, kind, data, None)
}

/// `alloc_store_node` for an Identifier or PrivateIdentifier with Go text
/// `text`, whose `data` has an empty text: the name column of the slot
/// holds the text (`store_identifier_name`).
// PERF: U1 (d). The node data needs no `String` for the text.
pub fn alloc_store_name_node<T: AstData>(
    file: usize,
    kind: SyntaxKind,
    data: T,
    text: &str,
) -> Node {
    debug_assert!(
        matches!(kind, SyntaxKind::Identifier | SyntaxKind::PrivateIdentifier),
        "{kind:?} is not a name kind"
    );
    alloc_store_slot(file, kind, data, Some(text))
}

/// S1: `alloc_store_name_node` for the Go factory payload of a name node
/// (empty data text), without a new data box or leaked data:
/// the slot points at the process-wide data of `kind` (`shared_name_node`).
/// The name column holds `text`. The factory checks its payload against the
/// shared one in debug builds (`debug_assert_shared_name_data`).
// PERF: S1. Saves one malloc and about 88 bytes per identifier (a 32-byte
// data box and a 40-byte arena node, 16 bytes since astmem1 P1). Sharing
// one data is safe because node data is never changed in place: it has no
// interior mutability, the crate forbids unsafe code, and a data write
// gives the slot a new data (`replace_store_node_data`). No code uses the
// address of node data as an identity (the `data_accessor!` `_in` debug
// check only compares the data of one node with itself).
pub fn alloc_store_shared_name_node(file: usize, kind: SyntaxKind, text: &str) -> Node {
    alloc_store_slot_node(file, kind, shared_name_node(kind), Some(text))
}

/// `alloc_store_node` with the name text `text` (`slot_text_name`).
// PERF: lsshells M3c. A static parse (every CLI parse) leaks the data first
// and passes a reference, as before M3c: one thread-local load picks the
// path, and the data value is not moved into the store closure.
// PERF: astchunks2. The data goes by value into the AST arena, with no box
// (Go: the typed arenas of `NodeFactory`). Only the arena bump and the view
// are generic; the rest is `alloc_store_slot_node`.
#[inline]
fn alloc_store_slot<T: AstData>(
    file: usize,
    kind: SyntaxKind,
    data: T,
    text: Option<&str>,
) -> Node {
    debug_assert!(
        data.view().matches_syntax_kind(kind),
        "{kind:?} does not fit its NodeData"
    );
    if FREEABLE_PARSE.get() {
        return alloc_store_boxed_slot(file, kind, data, text);
    }
    alloc_store_slot_node(file, kind, leak_in_ast_arena(data).view(), text)
}

/// `alloc_store_slot` inside a freeable parse: the data goes in a box
/// (owned file versions keep boxed data, `OwnedAst`).
#[inline(never)]
fn alloc_store_boxed_slot<T: AstData>(
    file: usize,
    kind: SyntaxKind,
    data: T,
    text: Option<&str>,
) -> Node {
    alloc_store_owned_slot(file, kind, data.into_boxed(), text)
}

/// `alloc_store_slot` for node data that is already leaked (or the shared
/// name data). In a store that owns its nodes the slot has no cell, like a
/// lib snapshot slot.
#[inline]
fn alloc_store_slot_node(
    file: usize,
    kind: SyntaxKind,
    node: ColumnData,
    text: Option<&str>,
) -> Node {
    debug_assert!(
        text.is_none() || identifier_text(node).is_empty(),
        "a name node keeps its text in the name column"
    );
    // PERF: U1 (b), U4. Read while the new data is hot, not at freeze.
    let (modifiers, children) = slot_kids_parts(kind, node);
    with_store_mut(file, |s| {
        let modifier_bits = s.static_modifier_bits(modifiers);
        assert!(!s.frozen, "cannot create a node in a finished file");
        let index = s.records.len() as u32;
        let (name, text_is_keyword) = s.slot_text_name(kind, node, text);
        s.push_node_header(kind, text_is_keyword);
        s.nodes.push(Some(node));
        s.push_no_cell();
        s.push_build_columns(kind, name, modifier_bits, children);
        handle(file, index)
    })
}

/// The own modifier list field (for the U1 (b) modifier bits, from the list
/// table: `FileStore::static_modifier_bits`) and the U4 child entry of a new
/// node slot of kind `kind` with data `node` (`store_node_modifier_list`,
/// `store_node_children`).
// PERF: perfP2 item 2. About a third of the slots are identifiers, whose
// entries are fixed (no modifier list, none of the child fields): no call.
// A test for the other kinds with fixed entries (tokens, keyword types,
// string and numeric literals) took fewer instructions but more cycles.
#[inline]
fn slot_kids_parts(
    kind: SyntaxKind,
    node: NodeData<'_>,
) -> (Option<crate::astdata::ModifierList>, SlotChildren) {
    if kind == SyntaxKind::Identifier {
        debug_assert_eq!(super::node::store_node_modifier_list(kind, node), None);
        debug_assert_eq!(
            super::node::store_node_children(kind, node),
            SlotChildren::NO_FIELDS
        );
        return (None, SlotChildren::NO_FIELDS);
    }
    (
        super::node::store_node_modifier_list(kind, node),
        super::node::store_node_children(kind, node),
    )
}

/// `alloc_store_slot` inside a freeable parse (lsshells M3c): a store that
/// owns its nodes keeps `node` in a new cell. Any other store leaks it.
#[inline(never)]
fn alloc_store_owned_slot(
    file: usize,
    kind: SyntaxKind,
    node: BoxedData,
    text: Option<&str>,
) -> Node {
    if !with_store(file, |s| s.owned.is_some()) {
        return alloc_store_slot_node(file, kind, leak_in_ast_arena(node).view(), text);
    }
    debug_assert!(
        text.is_none() || identifier_text(node.view()).is_empty(),
        "a name node keeps its text in the name column"
    );
    let children = super::node::store_node_children(kind, node.view());
    with_store_mut(file, |s| {
        let modifier_bits = super::node::store_node_modifier_bits(kind, node.view(), |m| {
            s.modifier_list_bits(true, m)
        });
        assert!(!s.frozen, "cannot create a node in a finished file");
        let index = s.records.len() as u32;
        let (name, text_is_keyword) = s.slot_text_name(kind, node.view(), text);
        s.push_node_header(kind, text_is_keyword);
        let owned = s.owned_mut();
        let cell = owned.push(node);
        owned.cell_of.push(cell);
        s.nodes.push(Some(owned_marker()));
        s.push_build_columns(kind, name, modifier_bits, children);
        handle(file, index)
    })
}

impl FileStore {
    /// The record of a new node slot (`alloc_store_slot_node`).
    #[inline]
    fn push_node_header(&mut self, kind: SyntaxKind, text_is_keyword: bool) {
        // PERF: factscol1b round c. The push of the fast arm knows there
        // is room and makes no capacity check of its own. Each arm makes
        // the record where it pushes it: made before a push that can
        // grow, it went through the stack.
        if self.records.len() < self.records.capacity() {
            self.records.push(NodeRecord::node(text_is_keyword));
        } else {
            self.push_record_into_full_column(text_is_keyword);
        }
        self.kinds.push(kind);
        self.build_facts.node(kind);
    }

    /// `push_node_header` when the record column is full: `Vec::push`
    /// doubles it, and each other slot column doubles at its own push, as
    /// in R173. So every column has the capacity of R173 for every input.
    // factscol1b round d: rounds a to c grew the full columns of a dense
    // text by more than a doubling, to an estimate of its slots. When such
    // a growth just fit, its unused room left too little memory for the
    // rest of the parse, and inputs that R173 parses ran out of memory
    // (wasm32, and native under `ulimit -v`). It gave no instruction gain.
    #[cold]
    #[inline(never)]
    fn push_record_into_full_column(&mut self, text_is_keyword: bool) {
        self.records.push(NodeRecord::node(text_is_keyword));
    }

    /// The kids (U1, U4) and link entries of a new node slot of kind
    /// `kind`, after its record and node (`alloc_store_slot_node`).
    #[inline]
    fn push_build_columns(
        &mut self,
        kind: SyntaxKind,
        name: Name,
        modifier_bits: u32,
        children: SlotChildren,
    ) {
        self.kids.push(NodeKids::new(
            children,
            NodeKids::word_of(kind, &name, modifier_bits),
        ));
        self.build_links.push(SlotLinks::NONE);
        self.debug_assert_build_columns();
    }
}

/// The store-local id that stands for `n` inside `NodeData` of store `file`.
/// Nil maps to the nil slot. A node of another file gets (or reuses) an
/// alias slot.
// PERF: U4 (4). The factory calls this for every child and list entry, and
// nearly all are nil or of the same file. Those two tests are inline; the
// alias path is out of line, so the function body stays small.
#[inline]
#[must_use]
pub fn store_child_id(file: usize, n: Node) -> crate::astdata::NodeId {
    if n.is_nil() {
        return crate::astdata::NodeId::new(NIL_SLOT);
    }
    if n.file_index() == file {
        return crate::astdata::NodeId::new(slot_index(n) as u32);
    }
    store_alias_id(file, n)
}

/// The alias slot of foreign node `n` in store `file` (`store_child_id`).
#[inline(never)]
fn store_alias_id(file: usize, n: Node) -> crate::astdata::NodeId {
    with_store_mut(file, |s| {
        if let Some(&index) = s.aliases.get(&n) {
            return crate::astdata::NodeId::new(index);
        }
        let index = s.records.len() as u32;
        s.records.push(NodeRecord::target(n));
        s.kinds.push(SyntaxKind::Unknown);
        s.build_facts.has_alias_slot = true;
        s.kids.push(NodeKids::unknown());
        s.nodes.push(None);
        s.push_no_cell();
        s.build_links.push(SlotLinks::NONE);
        s.debug_assert_build_columns();
        s.aliases.insert(n, index);
        crate::astdata::NodeId::new(index)
    })
}

/// Like `store_child_id`, for astdata fields that are `Option<NodeId>`.
#[must_use]
pub fn store_opt_child_id(file: usize, n: Node) -> Option<crate::astdata::NodeId> {
    if n.is_nil() {
        None
    } else {
        Some(store_child_id(file, n))
    }
}

/// A Go `core.TextRange` in astdata form (`-1` is stored as `u32::MAX`).
fn ts_range(loc: TextRange) -> crate::astdata::text::TextRange {
    crate::astdata::text::TextRange {
        start: crate::astdata::text::TextPos::new(loc.pos() as u32),
        end: crate::astdata::text::TextPos::new(loc.end() as u32),
    }
}

/// A list of a static store (every CLI parse, a lib snapshot load, a
/// freeable parse whose version died before the publish): Go `*NodeList`,
/// with its ids in the AST bump arena (Go `nodeSliceArena`). It lives for
/// the process. The store's list table (`FileStore::lists`) names it by
/// `index`, and that index is the astdata list field of the node data that
/// holds it: 4 bytes, with no copy of the ids (astshrink1). A `NodeList`
/// handle of a static store names it directly, so a list that the parser
/// made and the same list read from node data are one list, like the Go
/// pointer.
#[derive(Debug)]
pub struct StaticList {
    /// Go `list.Loc` in astdata form (`ts_range`).
    pub(crate) range: crate::astdata::text::TextRange,
    /// The store ids of the nodes (`store_child_id`).
    pub(crate) nodes: &'static [crate::astdata::NodeId],
    /// The entry of the list in its store's list table.
    pub(crate) index: u32,
    /// Bit 31: the astdata missing-list bit (`NodeList::stored_trailing_comma`).
    /// Bits 0 to 30: Go `ModifiersToFlags` of a modifier list (the syntactic
    /// modifier bits), 0 for a node list.
    bits: u32,
}

impl StaticList {
    const COMMA_BIT: u32 = 1 << 31;

    /// The `bits` of a list with missing-list bit `comma` and modifier flags
    /// `flags`.
    fn bits(comma: bool, flags: crate::astdata::ModifierFlags) -> u32 {
        debug_assert_eq!(flags.0 & Self::COMMA_BIT, 0, "syntactic modifier flags");
        flags.0 | if comma { Self::COMMA_BIT } else { 0 }
    }

    /// The astdata missing-list bit (`NodeList::stored_trailing_comma`).
    #[inline]
    #[must_use]
    pub fn has_trailing_comma(&self) -> bool {
        self.bits & Self::COMMA_BIT != 0
    }

    /// Go `modifiers.ModifierFlags` of a modifier list (0 for a node list).
    #[inline]
    #[must_use]
    pub fn modifier_flags(&self) -> crate::astdata::ModifierFlags {
        crate::astdata::ModifierFlags(self.bits & !Self::COMMA_BIT)
    }
}

/// Entry 0 of every static list table: the Go `nil` of a required list field
/// (`NIL_LIST_POS`), which `NodeList::is_nil` reads as nil.
fn nil_static_list() -> &'static StaticList {
    static NIL: StaticList = StaticList {
        range: crate::astdata::text::TextRange {
            start: crate::astdata::text::TextPos::new(NIL_LIST_POS),
            end: crate::astdata::text::TextPos::new(NIL_LIST_POS),
        },
        nodes: &[],
        index: NIL_LIST_INDEX,
        bits: 0,
    };
    &NIL
}

/// The list table entry of the Go `nil` marker of a required list field, in
/// every store table (static and owned).
pub(crate) const NIL_LIST_INDEX: u32 = 0;

/// Adds a list over `ids` to static list table `lists` and returns it. The
/// list goes into `arena`, this thread's AST arena.
// PERF: storefast2. Inline: the parser makes about one list for 5 nodes.
#[inline]
fn push_static_list(
    lists: &mut Vec<&'static StaticList>,
    arena: &'static LeakArena,
    range: crate::astdata::text::TextRange,
    ids: &'static [crate::astdata::NodeId],
    bits: u32,
) -> &'static StaticList {
    let index = u32::try_from(lists.len()).expect("too many lists in one store");
    let list = arena.alloc(StaticList {
        range,
        nodes: ids,
        index,
        bits,
    });
    lists.push(list);
    list
}

/// A new list of store `file` over `nodes` at `loc`, with missing-list bit
/// `comma` and modifier flags `flags` (`None` for a node list), as a list
/// table index. A store that owns its nodes (a freeable parse, lsshells
/// M3c) keeps it in its own table (`OwnedPending`); a static store leaks it
/// (`StaticList`).
fn new_store_list(
    file: usize,
    nodes: &[Node],
    loc: crate::astdata::text::TextRange,
    comma: bool,
    flags: Option<crate::astdata::ModifierFlags>,
) -> StoreListRef {
    if FREEABLE_PARSE.get()
        && let Some(list) = new_owned_pending(file, nodes, loc, comma, flags)
    {
        return StoreListRef::Owned(list);
    }
    // The ids make the alias slots of the nodes in order, before the store
    // borrow below.
    let arena: &'static LeakArena = AST_ARENA.with(|a| *a);
    let ids = arena.alloc_slice_fill_with(nodes.len(), |i| store_child_id(file, nodes[i]));
    let bits = StaticList::bits(comma, flags.unwrap_or_default());
    StoreListRef::Static(with_store_mut(file, |s| {
        push_static_list(&mut s.lists, arena, loc, ids, bits)
    }))
}

/// A list that `new_store_list` made.
enum StoreListRef {
    Static(&'static StaticList),
    Owned(StoreList),
}

impl StoreListRef {
    /// The list table index (the astdata list field).
    fn index(&self) -> u32 {
        match self {
            Self::Static(list) => list.index,
            Self::Owned(list) => list.key,
        }
    }
}

/// Go `f.NewNodeList(nodes)` followed by `list.Loc = loc`, in store `file`.
// PERF: astshrink1. The ids go into the AST bump arena once, and node data
// that holds the list stores its 4-byte table index (`store_list_value`):
// no copy into the data, no malloc.
#[must_use]
pub fn new_store_node_list(file: usize, nodes: &[Node], loc: TextRange) -> NodeList {
    match new_store_list(file, nodes, ts_range(loc), false, None) {
        StoreListRef::Static(list) => NodeList::from_static(file, list),
        StoreListRef::Owned(list) => NodeList::store(list),
    }
}

/// The pending list of `nodes` at `loc` (with `flags` for a modifier
/// list) in store `file` when that store owns its nodes (lsshells M3c).
/// `None`, with nothing made, for a static store.
// PERF: the callers test `FREEABLE_PARSE` inline, so a static parse (every
// CLI parse) does not call this.
#[inline(never)]
fn new_owned_pending(
    file: usize,
    nodes: &[Node],
    range: crate::astdata::text::TextRange,
    has_trailing_comma: bool,
    flags: Option<crate::astdata::ModifierFlags>,
) -> Option<StoreList> {
    // A store that owns its nodes is made and filled only inside a freeable
    // parse scope.
    if !FREEABLE_PARSE.get() || !with_store(file, |s| s.owned.is_some()) {
        return None;
    }
    // The alias slots are made in order, as `new_store_list` does, before
    // the store borrow below.
    let nodes: Box<[crate::astdata::NodeId]> =
        nodes.iter().map(|&n| store_child_id(file, n)).collect();
    Some(with_store_mut(file, |s| {
        s.owned_mut().push_pending(
            file,
            OwnedPending {
                range,
                nodes,
                has_trailing_comma,
                flags,
            },
        )
    }))
}

/// Go `f.NewModifierList(nodes)` followed by `list.Loc = loc`, in store
/// `file`. `ModifierFlags = ModifiersToFlags(nodes)` as in Go.
// PERF: astshrink1, as `new_store_node_list`.
#[must_use]
pub fn new_store_modifier_list(file: usize, nodes: &[Node], loc: TextRange) -> ModifierList {
    let flags = crate::astdata::ModifierFlags(modifiers_to_flags(nodes).0 as u32);
    match new_store_list(file, nodes, ts_range(loc), false, Some(flags)) {
        StoreListRef::Static(list) => ModifierList::from_static(file, list),
        StoreListRef::Owned(list) => ModifierList::store(list),
    }
}

/// True when new node data of store `file` goes into a cell of the store
/// (a freeable parse of a store that owns its nodes, `alloc_store_slot`),
/// so its list fields name the owned list table; false when it is static
/// and names the static table. `new_store_list` makes its lists by the
/// same test.
#[inline]
fn owns_new_data(file: usize) -> bool {
    FREEABLE_PARSE.get() && with_store(file, |s| s.owned.is_some())
}

/// A list value to store inside new `NodeData` of store `file`: its list
/// table index. Go stores the `*NodeList` pointer. A list of the same store
/// is stored as is (its index); any other list is rebuilt over ids of this
/// store with its own `Loc` (and no missing-list bit).
// PERF: storefast2. The parser stores lists that it has just made in its
// own static store: the index, read inline (Go: one pointer store). The
// rest is out of line (`other_store_list_value`).
#[inline]
#[must_use]
pub fn store_list_value(file: usize, list: NodeList) -> Option<crate::astdata::NodeList> {
    // An optional list field is nil in about half the calls.
    if list.is_nil_handle() {
        return None;
    }
    if let Some((list_file, l)) = list.static_parts()
        && list_file == file
        && !FREEABLE_PARSE.get()
    {
        // `owns_new_data(file)` is false outside a freeable parse.
        return (!is_nil_list_range(&l.range)).then(|| crate::astdata::NodeList::new(l.index));
    }
    other_store_list_value(file, list)
}

/// `store_list_value` for a list that is not nil or a static list of store
/// `file`, or inside a freeable parse.
#[inline(never)]
fn other_store_list_value(file: usize, list: NodeList) -> Option<crate::astdata::NodeList> {
    if list.is_nil() {
        return None;
    }
    if list.file() == file {
        match (list.static_list(), list.store_list(), owns_new_data(file)) {
            (Some(l), _, false) => return Some(crate::astdata::NodeList::new(l.index)),
            // lsshells M3c: a list of a store that owns its nodes.
            (_, Some(l), true) => return Some(crate::astdata::NodeList::new(l.key)),
            _ => {}
        }
    }
    let nodes = list.nodes().to_vec();
    let index = new_store_list(file, &nodes, ts_range(list.loc()), false, None).index();
    Some(crate::astdata::NodeList::new(index))
}

/// Like `store_list_value` for a list field that astdata requires. Go `nil`
/// becomes the list table entry `NIL_LIST_INDEX` (`NIL_LIST_POS`), which
/// `NodeList::is_nil` reads as nil.
#[inline]
#[must_use]
pub fn store_req_list_value(file: usize, list: NodeList) -> crate::astdata::NodeList {
    store_list_value(file, list).unwrap_or(crate::astdata::NodeList::new(NIL_LIST_INDEX))
}

/// A modifier list value to store inside new `NodeData` of store `file`.
// PERF: storefast2. As `store_list_value`: the index of a static list of
// store `file` inline, the rest out of line.
#[inline]
#[must_use]
pub fn store_modifiers_value(
    file: usize,
    modifiers: ModifierList,
) -> Option<crate::astdata::ModifierList> {
    // Most nodes have no modifiers.
    if modifiers.is_nil() {
        return None;
    }
    if let Some((list_file, m)) = modifiers.node_list().static_parts()
        && list_file == file
        && !FREEABLE_PARSE.get()
    {
        return Some(crate::astdata::ModifierList {
            list: crate::astdata::NodeList::new(m.index),
        });
    }
    other_store_modifiers_value(file, modifiers)
}

/// `store_modifiers_value` for a list that is not nil or a static list of
/// store `file`, or inside a freeable parse.
#[inline(never)]
fn other_store_modifiers_value(
    file: usize,
    modifiers: ModifierList,
) -> Option<crate::astdata::ModifierList> {
    if modifiers.is_nil() {
        return None;
    }
    if modifiers.file() == file {
        let index = match (
            modifiers.static_list(),
            modifiers.store_list(),
            owns_new_data(file),
        ) {
            (Some(m), _, false) => Some(m.index),
            // lsshells M3c: a list of a store that owns its nodes.
            (_, Some(m), true) => Some(m.key),
            _ => None,
        };
        if let Some(index) = index {
            return Some(crate::astdata::ModifierList {
                list: crate::astdata::NodeList::new(index),
            });
        }
    }
    let nodes = modifiers.nodes().to_vec();
    let flags = crate::astdata::ModifierFlags(modifiers_to_flags(&nodes).0 as u32);
    let index = new_store_list(file, &nodes, ts_range(modifiers.loc()), false, Some(flags)).index();
    Some(crate::astdata::ModifierList {
        list: crate::astdata::NodeList::new(index),
    })
}

/// True when `range` is the range of a Go `nil` marker list
/// (`NIL_LIST_POS`).
#[inline]
#[must_use]
pub fn is_nil_list_range(range: &crate::astdata::text::TextRange) -> bool {
    range.start.get() == NIL_LIST_POS && range.end.get() == NIL_LIST_POS
}

#[cfg(test)]
mod tests {
    use super::*;

    /// astchunks2: a node column entry is Go's 16 B interface word (tag and
    /// reference), and the owning enum has the same layout (`view` is a copy).
    #[test]
    #[cfg(target_pointer_width = "64")]
    fn node_column_entry_is_16_bytes() {
        assert_eq!(std::mem::size_of::<Option<ColumnData>>(), 16);
        assert_eq!(std::mem::size_of::<BoxedData>(), 16);
    }

    #[test]
    fn parser_factory_writes_into_the_file_store() {
        let file = new_file_store("/a.ts", "a.b");
        let f = NodeFactory::for_file(file);
        let a = f.new_identifier("a");
        let b = f.new_identifier("b");
        set_node_loc(a, TextRange::new(0, 1));
        let q = f.new_qualified_name(a, b);
        set_node_parent(a, q);
        set_node_flags(q, NodeFlags::AMBIENT);

        assert_eq!(q.file_index(), file);
        assert_eq!(q.kind(), SyntaxKind::QualifiedName);
        assert_eq!(q.left(), a);
        assert_eq!(a.parent(), q);
        assert_eq!(a.loc(), TextRange::new(0, 1));
        assert_eq!(q.loc(), TextRange::undefined());
        assert_eq!(q.flags(), NodeFlags::AMBIENT);
        assert_eq!(source_file_text(q), "a.b");

        // A synthetic child is an alias slot that resolves to the same node.
        let s = NodeFactory::new().new_identifier("s");
        let q2 = f.new_qualified_name(q, s);
        assert_eq!(q2.right(), s);

        // Go nil in a required list field reads back as nil.
        let decls = f.new_variable_declaration_list(NodeList::NIL, NodeFlags::NONE);
        assert!(decls.declarations().is_nil());
        let list = f.new_node_list_with_loc(&[a], TextRange::new(0, 3));
        assert_eq!(list.loc(), TextRange::new(0, 3));
        assert_eq!(list.nodes().get(0), a);

        freeze_file_store(file);
        assert!(std::panic::catch_unwind(|| set_node_parent(b, q)).is_err());
    }

    #[test]
    fn freeze_makes_the_name_and_modifier_columns() {
        let file = new_file_store("/c.ts", "");
        let f = NodeFactory::for_file(file);
        let keyword = f.new_identifier("await");
        let plain = f.new_identifier("b");
        let private = f.new_private_identifier("#c");
        let export = f.new_modifier(SyntaxKind::ExportKeyword);
        let declare = f.new_modifier(SyntaxKind::DeclareKeyword);
        let modifiers = f.new_modifier_list(&[export, declare]);
        let list = f.new_variable_declaration_list(NodeList::NIL, NodeFlags::NONE);
        let statement = f.new_variable_statement(modifiers, list);
        freeze_file_store(file);

        with_store(file, |s| {
            let names = |n: Node| s.slot_text_name_of(slot_index(n));
            assert_eq!(names(keyword), Name::from("await"));
            assert_eq!(names(plain), Name::from("b"));
            assert_eq!(names(private), Name::from("#c"));
            assert_eq!(names(statement), Name::default());
            let is_keyword =
                |n: Node| s.records[slot_index(n)].has_bit(NodeRecord::TEXT_IS_KEYWORD);
            assert!(is_keyword(keyword));
            assert!(!is_keyword(plain));
            assert!(!is_keyword(private));
            let bits = |n: Node| {
                let i = slot_index(n);
                s.kids[i].modifier_bits(s.kinds[i])
            };
            assert_eq!(
                bits(statement),
                (ModifierFlags::EXPORT | ModifierFlags::AMBIENT).0
            );
            assert_eq!(bits(list), 0);
            assert_eq!(bits(export), 0);
        });
    }

    #[test]
    fn replaced_data_updates_the_name_and_modifier_columns() {
        let file = new_file_store("/d.ts", "");
        let f = NodeFactory::for_file(file);
        let id = f.new_identifier("a");
        let export = f.new_modifier(SyntaxKind::ExportKeyword);
        let modifiers = f.new_modifier_list(&[export]);
        let list = f.new_variable_declaration_list(NodeList::NIL, NodeFlags::NONE);
        let statement = f.new_variable_statement(modifiers, list);
        with_store(file, |s| {
            assert_eq!(s.slot_text_name_of(slot_index(id)), Name::from("a"));
            assert_ne!(
                s.kids[slot_index(statement)].modifier_bits(SyntaxKind::VariableStatement),
                0
            );
        });

        let mut data = store_ast_node(id).to_boxed();
        if let BoxedData::Identifier(d) = &mut data {
            d.text = "await".to_string();
        }
        replace_store_node_data(id, data);
        let mut data = store_ast_node(statement).to_boxed();
        if let BoxedData::VariableStatement(d) = &mut data {
            d.modifiers = None;
        }
        replace_store_node_data(statement, data);
        freeze_file_store(file);

        with_store(file, |s| {
            assert_eq!(s.slot_text_name_of(slot_index(id)), Name::from("await"));
            assert!(s.records[slot_index(id)].has_bit(NodeRecord::TEXT_IS_KEYWORD));
            assert_eq!(
                s.kids[slot_index(statement)].modifier_bits(SyntaxKind::VariableStatement),
                0
            );
        });
    }

    #[test]
    fn slots_get_the_children_column() {
        let file = new_file_store("/g.ts", "");
        let f = NodeFactory::for_file(file);
        let id = |n: Node| slot_index(n) as u32;
        let a = f.new_identifier("a");
        let b = f.new_identifier("b");
        let access = f.new_property_access_expression(a, Node::NIL, b, NodeFlags::NONE);
        let statement = f.new_expression_statement(access);
        let question = f.new_token(SyntaxKind::QuestionToken);
        let p = f.new_identifier("p");
        let signature = f.new_property_signature_declaration(
            ModifierList::NIL,
            p,
            question,
            Node::NIL,
            Node::NIL,
        );
        let x = f.new_identifier("x");
        let parameter = f.new_parameter_declaration(
            ModifierList::NIL,
            Node::NIL,
            x,
            Node::NIL,
            Node::NIL,
            Node::NIL,
        );
        let qualified = f.new_qualified_name(a, b);
        // A synthetic child gets an alias slot.
        let s = NodeFactory::new().new_identifier("s");
        let aliased = f.new_property_access_expression(s, Node::NIL, b, NodeFlags::NONE);
        // A reparser write replaces a child that the column holds.
        let c = f.new_identifier("c");
        let mut data = store_ast_node(statement).to_boxed();
        if let BoxedData::ExpressionStatement(d) = &mut data {
            d.expression = store_child_id(file, c);
        }
        replace_store_node_data(statement, data);
        freeze_file_store(file);

        with_store(file, |st| {
            let entry = |n: Node| st.kids[slot_index(n)].children();
            let expression = SlotChildren::TAG_EXPRESSION;
            // C2: none of these nodes has a type, an initializer or type
            // arguments.
            let untyped = |e: SlotChildren| e.with_typed(Some((SlotChildren::TYPED_TYPE, 0)), true);
            let no_type =
                |e: SlotChildren| e.with_typed(Some((SlotChildren::TYPED_INITIALIZER, 0)), true);
            assert_eq!(
                entry(access),
                untyped(SlotChildren::new(Some(id(b)), Some((expression, id(a)))))
            );
            assert_eq!(
                entry(statement),
                untyped(SlotChildren::new(Some(0), Some((expression, id(c)))))
            );
            assert_eq!(
                entry(signature),
                no_type(SlotChildren::new(
                    Some(id(p)),
                    Some((SlotChildren::TAG_POSTFIX, id(question)))
                ))
            );
            assert_eq!(
                entry(parameter),
                no_type(SlotChildren::new(
                    Some(id(x)),
                    Some((SlotChildren::TAG_QUESTION, 0))
                ))
            );
            assert_eq!(
                entry(a),
                untyped(SlotChildren::new(Some(0), Some((expression, 0))))
            );
            assert_eq!(entry(qualified), untyped(SlotChildren::UNKNOWN));
            // The nil slot and alias slots are unknown; an aliased child has
            // its alias slot id, as in the node data.
            assert_eq!(st.kids[NIL_SLOT as usize].children(), SlotChildren::UNKNOWN);
            let alias = st.aliases[&s];
            assert_eq!(st.kids[alias as usize].children(), SlotChildren::UNKNOWN);
            assert_eq!(
                entry(aliased),
                untyped(SlotChildren::new(Some(id(b)), Some((expression, alias))))
            );
            st.debug_check_kids();
        });
    }

    #[test]
    fn slots_get_the_typed_column() {
        let file = new_file_store("/i.ts", "");
        let f = NodeFactory::for_file(file);
        let id = |n: Node| slot_index(n) as u32;
        let x = f.new_identifier("x");
        let t = f.new_keyword_type_node(SyntaxKind::NumberKeyword);
        let one = f.new_numeric_literal("1", TokenFlags::NONE);
        let both = f.new_variable_declaration(x, Node::NIL, t, one);
        let only_initializer = f.new_variable_declaration(x, Node::NIL, Node::NIL, one);
        let only_type = f.new_variable_declaration(x, Node::NIL, t, Node::NIL);
        let name = f.new_identifier("T");
        let plain_reference = f.new_type_reference_node(name, NodeList::NIL);
        let generic_reference = f.new_type_reference_node(name, f.new_node_list(&[t]));
        let call = f.new_call_expression(
            x,
            Node::NIL,
            NodeList::NIL,
            f.new_node_list(&[]),
            NodeFlags::NONE,
        );
        freeze_file_store(file);

        with_store(file, |st| {
            let typed = |n: Node| {
                let entry = st.kids[slot_index(n)].children();
                (entry.typed_parts(), entry.has_no_type_arguments())
            };
            // Both set: the column holds the type only.
            assert_eq!(
                typed(both),
                ((SlotChildren::TYPED_TYPE_WITH_INITIALIZER, id(t)), true)
            );
            // One set: the column holds it, and the other reads as nil.
            assert_eq!(
                typed(only_initializer),
                ((SlotChildren::TYPED_INITIALIZER, id(one)), true)
            );
            assert_eq!(typed(only_type), ((SlotChildren::TYPED_TYPE, id(t)), true));
            assert_eq!(
                typed(plain_reference),
                ((SlotChildren::TYPED_TYPE_NAME, id(name)), true)
            );
            assert_eq!(
                typed(generic_reference),
                ((SlotChildren::TYPED_TYPE_NAME, id(name)), false)
            );
            assert_eq!(typed(call), ((SlotChildren::TYPED_TYPE, 0), true));
            assert_eq!(typed(x), ((SlotChildren::TYPED_TYPE, 0), true));
            // Unknown never claims a nil type argument list.
            assert!(!SlotChildren::UNKNOWN.has_no_type_arguments());
            st.debug_check_kids();
        });
    }

    /// R2-5: the chain of `n` in the build links of its store, or `None`
    /// when it is not known.
    fn build_chain(n: Node) -> Option<Vec<Node>> {
        let file = n.file_index();
        with_store(file, |s| {
            let mut next = s.build_links[slot_index(n)].first_child;
            if next == LINK_NONE {
                return None;
            }
            let mut chain = Vec::new();
            while next != LINK_END {
                chain.push(handle(file, next));
                next = s.build_links[next as usize].next_sibling;
            }
            Some(chain)
        })
    }

    /// R2-5: what `override_parent_in_immediate_children` does for `parent`.
    fn link_children(parent: Node) {
        let mut links = StoreChildLinks::new(parent);
        parent.for_each_child(|child| {
            if !links.set_parent(child) {
                set_node_parent(child, parent);
            }
            false
        });
    }

    // The child walk of `set_parent_in_store_children` borrows the list
    // table of the store and does not move it out, so a panic in the walk
    // leaves the table in place. Go's walk cannot panic; only a port bug
    // can (here a list field that names no list). An LSP request that
    // recovers that panic must still find the other lists.
    #[test]
    fn set_parent_in_store_children_keeps_the_lists_after_a_panic() {
        let file = new_file_store("/walk-panic.ts", "a;");
        let f = NodeFactory::for_file(file);
        let a = f.new_identifier("a");
        let statement = f.new_expression_statement(a);
        let block = f.new_block(f.new_node_list(&[statement]), false);
        let other = f.new_block(f.new_node_list(&[statement]), false);
        let lists = with_store(file, |s| s.lists.len());
        assert!(lists > 1, "the blocks have static lists");

        let mut data = with_ast_data(block, |d| d.to_boxed());
        if let BoxedData::Block(d) = &mut data {
            d.statements = crate::astdata::NodeList::new(u32::MAX - 1);
        }
        replace_store_node_data(block, data);
        let walk = std::panic::catch_unwind(|| set_parent_in_store_children(block));
        assert!(walk.is_err(), "the walk reads a list that is not there");

        assert_eq!(with_store(file, |s| s.lists.len()), lists);
        assert_eq!(other.statements().to_vec(), vec![statement]);
        assert!(set_parent_in_store_children(other));
        assert_eq!(statement.parent(), other);
    }

    // A store that owns its nodes keeps all its lists in its owned table,
    // and the static publish (`leak_owned_nodes`) replaces the static table:
    // a static list there would be lost. The check runs in every build.
    #[test]
    fn leak_owned_nodes_with_a_static_list_panics_in_every_build() {
        let file = {
            let _scope = enter_owned_parse();
            let file = new_file_store("/owned-static-list.ts", "");
            let f = NodeFactory::for_file(file);
            let _block = f.new_block(f.new_node_list(&[]), false);
            freeze_file_store(file);
            file
        };
        let leak = std::panic::catch_unwind(|| {
            with_store_mut(file, |s| {
                let nil_list = s.lists[0];
                s.lists.push(nil_list);
                s.leak_owned_nodes();
            });
        });
        let payload = leak.expect_err("a static list in a store with owned nodes");
        let message = payload.downcast_ref::<String>().map_or("", String::as_str);
        assert!(
            message.contains("static lists in a store with owned nodes"),
            "{message}"
        );
    }

    #[test]
    fn child_links_follow_the_parent_writes() {
        let file = new_file_store("/h.ts", "");
        let f = NodeFactory::for_file(file);
        let a = f.new_identifier("a");
        let b = f.new_identifier("b");
        let q = f.new_qualified_name(a, b);
        link_children(q);
        assert_eq!(build_chain(q), Some(vec![a, b]));
        assert_eq!(a.parent(), q);
        // Setting the parents again makes the same chain.
        link_children(q);
        assert_eq!(build_chain(q), Some(vec![a, b]));
        // A leaf has an empty chain; a node the parser did not finish has
        // none.
        link_children(a);
        assert_eq!(build_chain(a), Some(Vec::new()));
        assert_eq!(build_chain(b), None);
        // A second parent of linked children is unknown; the first keeps
        // its chain, as Go keeps them in its fields.
        let q2 = f.new_qualified_name(a, b);
        link_children(q2);
        assert_eq!(build_chain(q2), None);
        assert_eq!(build_chain(q), Some(vec![a, b]));
        assert_eq!(a.parent(), q2);
        // A data write frees the chain, so the children can be linked again.
        replace_store_node_data(q, store_ast_node(q).to_boxed());
        assert_eq!(build_chain(q), None);
        link_children(q2);
        assert_eq!(build_chain(q2), Some(vec![a, b]));
        // A child twice in one node, or a child of another store, makes the
        // chain unknown and frees the children linked before it.
        let twice = f.new_qualified_name(q, q);
        link_children(twice);
        assert_eq!(build_chain(twice), None);
        let synthetic = NodeFactory::new().new_identifier("s");
        let mixed = f.new_qualified_name(q, synthetic);
        link_children(mixed);
        assert_eq!(build_chain(mixed), None);
        assert_eq!(synthetic.parent(), mixed);
        link_children(twice);
        assert_eq!(build_chain(twice), None);
        freeze_file_store(file);
        with_store(file, |s| {
            assert_eq!(s.links.len(), s.records.len());
            assert_eq!(s.links[slot_index(q2)].first_child, slot_index(a) as u32);
            assert_eq!(s.links[slot_index(a)].next_sibling, slot_index(b) as u32);
            assert_eq!(s.links[slot_index(b)].next_sibling, LINK_END);
        });
    }

    #[test]
    fn parsed_chains_equal_for_each_child() {
        use crate::frontend::parser::{SourceFileParseOptions, parse_source_file};
        let text = "function f(a: number, b = 1) { return a + b; }\n\
            let x: Array<string> = [1, 2].map(y => `${y}`);\n\
            type T = { a?: string } | number;\n\
            class C<U> extends Object { m(): U | undefined { return undefined; } }\n";
        let opts = SourceFileParseOptions {
            file_name: "/links.ts".to_string(),
            ..Default::default()
        };
        let parsed = parse_source_file(&opts, text, ScriptKind::TS);
        let file = parsed.store;
        let links = with_store(file, |s| s.links.clone());
        assert_eq!(links.len(), file_store_slot_count(file));
        let mut known = 0;
        for index in 1..links.len() as u32 {
            let mut next = links[index as usize].first_child;
            if next == LINK_NONE {
                continue;
            }
            known += 1;
            let mut chain = Vec::new();
            while next != LINK_END {
                chain.push(handle(file, next));
                next = links[next as usize].next_sibling;
            }
            let n = handle(file, index);
            assert_eq!(
                chain,
                n.iter_children().collect::<Vec<_>>(),
                "{:?}",
                n.kind()
            );
        }
        assert!(known > 0);
        // The parser links the root too.
        assert_ne!(links[slot_index(parsed.root)].first_child, LINK_NONE);
    }

    /// The R200 `mark_roots`: a state per slot, a walk for every slot whose
    /// parent has no state yet.
    fn mark_roots_with_state(records: &mut [NodeRecord], kinds: &[SyntaxKind]) -> u32 {
        let Some(root) = (0..records.len())
            .rev()
            .find(|&i| records[i].is_node() && kinds[i] == SyntaxKind::SourceFile)
        else {
            return 0;
        };
        const UNSEEN: u8 = 0;
        const ON_PATH: u8 = 1;
        const ROOT: u8 = 2;
        const NOT_ROOT: u8 = 3;
        let mut state = vec![UNSEEN; records.len()];
        let mut path = Vec::new();
        for start in (1..records.len()).rev() {
            if !records[start].is_node() {
                continue;
            }
            if kinds[start] != SyntaxKind::SourceFile
                && let Some(parent) = records[start].local_parent()
                && matches!(state[parent], ROOT | NOT_ROOT)
            {
                state[start] = state[parent];
                let on = state[start] == ROOT;
                records[start].set_bit(NodeRecord::SOURCE_FILE_ROOT, on);
                continue;
            }
            let mut cur = start;
            let result = loop {
                match state[cur] {
                    ROOT | NOT_ROOT => break state[cur],
                    ON_PATH => break NOT_ROOT,
                    _ => {}
                }
                state[cur] = ON_PATH;
                path.push(cur);
                if kinds[cur] == SyntaxKind::SourceFile {
                    break if cur == root { ROOT } else { NOT_ROOT };
                }
                let Some(parent) = records[cur].local_parent() else {
                    break NOT_ROOT;
                };
                cur = parent;
            };
            for i in path.drain(..) {
                state[i] = result;
                records[i].set_bit(NodeRecord::SOURCE_FILE_ROOT, result == ROOT);
            }
        }
        root as u32
    }

    #[test]
    fn mark_roots_equals_the_walk_of_every_slot() {
        // xorshift, so the shapes are the same in every run.
        let mut seed = 0x9e37_79b9_7f4a_7c15_u64;
        let mut next = |n: usize| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % n as u64) as usize
        };
        for _ in 0..2000 {
            let slots = 2 + next(40);
            let mut kinds = vec![SyntaxKind::Unknown];
            let mut records = vec![NodeRecord::target(Node::NIL)];
            for i in 1..slots {
                if next(12) == 0 {
                    // An alias slot whose target has a local parent code.
                    let target = (2u64 << 32) | next(slots) as u64;
                    records.push(NodeRecord::target(Node(target)));
                    kinds.push(SyntaxKind::Unknown);
                    continue;
                }
                let parent = match next(10) {
                    0 => 0,
                    1 => FOREIGN_PARENT,
                    // Mostly a slot above, as the parser makes them.
                    2..=7 if i + 1 < slots => (i + 1 + next(slots - i - 1)) as u32 + 1,
                    _ => next(slots) as u32 + 1,
                };
                records.push(NodeRecord::new(
                    0,
                    NodeFlags::NONE,
                    TextRange::undefined(),
                    u64::from(parent),
                ));
                kinds.push(if next(8) == 0 {
                    SyntaxKind::SourceFile
                } else {
                    SyntaxKind::Block
                });
            }
            let mut expected: Vec<NodeRecord> = records.iter().map(copy_record).collect();
            let expected_root = mark_roots_with_state(&mut expected, &kinds);
            let root = mark_roots(&mut records, &kinds);
            assert_eq!(root, expected_root);
            for (i, (a, b)) in records.iter().zip(&expected).enumerate() {
                assert_eq!(
                    a.has_bit(NodeRecord::SOURCE_FILE_ROOT),
                    b.has_bit(NodeRecord::SOURCE_FILE_ROOT),
                    "slot {i} of {slots}"
                );
            }
        }
    }

    fn copy_record(r: &NodeRecord) -> NodeRecord {
        let copy = NodeRecord::zero();
        copy.store_from(r);
        copy
    }

    #[test]
    fn slot_children_tags_round_trip() {
        let entry = SlotChildren::new(Some(7), Some((SlotChildren::TAG_QUESTION, 9)));
        assert_eq!(entry.name(), 7);
        assert_eq!(entry.other_parts(), (SlotChildren::TAG_QUESTION, 9));
        // An id that does not fit next to the tag is unknown.
        let big = SlotChildren::new(Some(1), Some((SlotChildren::TAG_POSTFIX, 1 << 30)));
        assert_eq!(big.other(), SlotChildren::UNKNOWN_ID);
        assert_eq!(big.other_parts().0, SlotChildren::TAG_UNKNOWN);
        assert_eq!(SlotChildren::new(None, None), SlotChildren::UNKNOWN);
        assert_eq!(
            SlotChildren::new(Some(0), Some((SlotChildren::TAG_EXPRESSION, 0)))
                .with_typed(Some((SlotChildren::TYPED_TYPE, 0)), true),
            SlotChildren::NO_FIELDS
        );
        // C2: the tag, the id and the type arguments bit round trip, and an
        // id that does not fit is unknown.
        let typed = entry.with_typed(Some((SlotChildren::TYPED_TYPE_NAME, 11)), true);
        assert_eq!(typed.typed_parts(), (SlotChildren::TYPED_TYPE_NAME, 11));
        assert!(typed.has_no_type_arguments());
        let big = entry.with_typed(Some((SlotChildren::TYPED_TYPE, 1 << 28)), false);
        assert_eq!(big.typed_parts().0, SlotChildren::TYPED_UNKNOWN);
        // perfP2 item 2: two words, so `store_node_children` returns it in
        // two registers. The kids column keeps its 12 bytes in `NodeKids`.
        assert_eq!(std::mem::size_of::<SlotChildren>(), 16);
        assert_eq!(std::mem::size_of::<NodeKids>(), 16);
    }

    #[test]
    fn store_identifier_text_lives_in_the_name_column() {
        let file = new_file_store("/e.ts", "");
        let f = NodeFactory::for_file(file);
        let id = f.new_identifier("abc");
        let private = f.new_private_identifier("#p");
        let missing = f.new_identifier("");
        assert!(identifier_text(store_ast_node(id)).is_empty());
        // S1: every store identifier points at one shared astdata node.
        assert!(store_ast_node(id).same(store_ast_node(missing)));
        assert!(store_ast_node(id).same(shared_name_node(SyntaxKind::Identifier)));
        assert!(store_ast_node(private).same(shared_name_node(SyntaxKind::PrivateIdentifier)));
        assert_eq!(id.text(), "abc");
        assert_eq!(private.text(), "#p");
        assert_eq!(missing.text(), "");
        // Data cloned from a store identifier has no text; the name stays.
        replace_store_node_data(id, store_ast_node(id).to_boxed());
        assert_eq!(id.text(), "abc");
        // S1: the write gives the slot its own node; the shared one stays.
        assert!(!store_ast_node(id).same(store_ast_node(missing)));
        assert_eq!(missing.text(), "");
        // A synthetic identifier keeps its text in the data.
        assert_eq!(NodeFactory::new().new_identifier("syn").text(), "syn");
        freeze_file_store(file);
        assert_eq!(id.text(), "abc");
        assert_eq!(missing.text(), "");
    }

    #[test]
    fn pending_lists_are_built_once_into_node_data() {
        assert_eq!(std::mem::size_of::<NodeList>(), 16);
        assert_eq!(std::mem::size_of::<ModifierList>(), 16);
        let file = new_file_store("/f.ts", "");
        let f = NodeFactory::for_file(file);
        let a = f.new_identifier("a");
        let list = f.new_node_list_with_loc(&[a], TextRange::new(1, 4));
        assert!(list.static_list().is_some());
        assert_eq!(list, list);
        assert_eq!(list.nodes().get(0), a);
        assert_eq!(list.loc(), TextRange::new(1, 4));
        let decls = f.new_variable_declaration_list(list, NodeFlags::NONE);
        let stored = decls.declarations();
        assert!(stored.static_list().is_some());
        assert_eq!(stored, decls.declarations());
        // astshrink1: the node data names the list that the parser made, so
        // the two are one list, like the Go `*NodeList` pointer.
        assert_eq!(stored, list);
        assert_eq!(stored.nodes().get(0), a);
        assert_eq!(stored.loc(), TextRange::new(1, 4));

        let export = f.new_modifier(SyntaxKind::ExportKeyword);
        let modifiers = f.new_modifier_list(&[export]);
        assert!(modifiers.static_list().is_some());
        assert_eq!(modifiers.modifier_flags(), ModifierFlags::EXPORT);
        let statement = f.new_variable_statement(modifiers, decls);
        assert_eq!(
            statement.modifiers().modifier_flags(),
            ModifierFlags::EXPORT
        );
        assert_eq!(statement.modifiers(), modifiers);

        let empty = f.new_node_list_with_loc(&[], TextRange::new(2, 2));
        let missing = empty.with_missing_marker();
        assert!(!crate::frontend::parser::parser_p1::is_missing_node_list(
            empty
        ));
        assert!(crate::frontend::parser::parser_p1::is_missing_node_list(
            missing
        ));
        let holder = f.new_variable_declaration_list(missing, NodeFlags::NONE);
        assert!(crate::frontend::parser::parser_p1::is_missing_node_list(
            holder.declarations()
        ));
    }

    #[test]
    fn adopted_detached_parse_equals_a_serial_parse() {
        use crate::frontend::parser::{
            ParsedSourceFile, SourceFileParseOptions, adopt_detached_parse, parse_source_file,
            parse_source_file_detached,
        };
        let text =
            "/** doc */\nexport function f(a: number) { return a + 1; }\nlet x = <T>(y: T) => y;\n";
        let opts = SourceFileParseOptions {
            file_name: "/a.ts".to_string(),
            ..Default::default()
        };
        let serial = parse_source_file(&opts, text, ScriptKind::TS);
        let worker_opts = opts.clone();
        let detached = std::thread::spawn(move || {
            parse_source_file_detached(7, &worker_opts, text, ScriptKind::TS)
        })
        .join()
        .unwrap();
        assert_eq!(detached.store.id(), DETACHED_STORE_BASE + 7);
        let adopted: ParsedSourceFile = adopt_detached_parse(detached, &opts);

        assert_eq!(adopted.store, serial.store + 1);
        let slots = file_store_slot_count(serial.store);
        assert_eq!(file_store_slot_count(adopted.store), slots);
        let to_serial = |n: Node| {
            if n.is_some() && n.file_index() == adopted.store {
                handle(serial.store, slot_index(n) as u32)
            } else {
                n
            }
        };
        for index in 1..slots as u32 {
            let (a, b) = (handle(serial.store, index), handle(adopted.store, index));
            let (ha, hb) = (store_header(a), store_header(b));
            assert_eq!(ha.kind, hb.kind);
            assert_eq!(ha.flags, hb.flags);
            assert_eq!(ha.loc, hb.loc);
            assert_eq!(ha.parent, to_serial(hb.parent));
        }
        assert_eq!(serial.root, to_serial(adopted.root));
        assert_eq!(serial.imports.len(), adopted.imports.len());
        assert_eq!(serial.jsdoc_cache.len(), adopted.jsdoc_cache.len());
        for (node, jsdocs) in &adopted.jsdoc_cache {
            let expected: Vec<Node> = jsdocs.iter().map(|&n| to_serial(n)).collect();
            assert_eq!(serial.jsdoc_cache[&to_serial(*node)], expected);
            assert_eq!(
                file_store_js_doc(adopted.store, *node).map(NodeSlice::to_vec),
                Some(jsdocs.clone())
            );
        }
    }

    // watchcfg1 round c: a parse worker's parse in a freeable parse scope
    // owns its nodes (`new_detached_file_store`), the adopted parse reads
    // like a static parse of the same text with its JSDoc cache on the real
    // store id, and a worker parse that the loader does not take frees its
    // store and text when it is dropped.
    #[test]
    fn owned_detached_parse_reads_like_a_static_parse() {
        use crate::frontend::parser::{
            SourceFileParseOptions, adopt_detached_parse, parse_source_file,
            parse_source_file_detached,
        };
        // A JS file: its parse makes the JSDoc cache (a TS file's is lazy).
        let text = "/** @param {number} a */\nexport async function f(a, b = 1) { return a + b; }\n\
            /** @type {string[]} */\nlet x = [1, 2].map(y => `${y}`);\n";
        let opts = |name: &str| SourceFileParseOptions {
            file_name: name.to_string(),
            ..Default::default()
        };
        let fixed = parse_source_file(&opts("/static.js"), text, ScriptKind::JS);
        let worker = |job: usize, name: &'static str| {
            let worker_opts = opts(name);
            let text = FileText::Shared(Arc::from(text));
            let weak = text.weak().expect("a shared text");
            let parse = std::thread::spawn(move || {
                let _scope = enter_owned_parse();
                parse_source_file_detached(job, &worker_opts, text, ScriptKind::JS)
            })
            .join()
            .unwrap();
            (parse, weak)
        };

        let (detached, _) = worker(8, "/owned.js");
        assert!(detached.freeable);
        assert!(detached.store.is_self_contained());
        assert!(
            detached.store.store.owned.is_some(),
            "the worker parse owns its nodes"
        );
        let owned = adopt_detached_parse(detached, &opts("/owned.js"));
        assert!(with_store(owned.store, |s| s.owned.is_some()));
        let (a, b) = (tree(fixed.root), tree(owned.root));
        assert_eq!(a.len(), b.len());
        for (&a, &b) in a.iter().zip(&b) {
            assert_eq!(twin_facts(a), twin_facts(b), "{:?}", a.kind());
        }
        assert_eq!(owned.jsdoc_cache.len(), 2);
        for (node, jsdocs) in &owned.jsdoc_cache {
            assert_eq!(node.file_index(), owned.store);
            assert_eq!(
                file_store_js_doc(owned.store, *node).map(NodeSlice::to_vec),
                Some(jsdocs.clone())
            );
        }

        let (untaken, text) = worker(9, "/untaken.js");
        assert!(text.upgrade().is_some());
        drop(untaken);
        assert!(
            text.upgrade().is_none(),
            "a worker parse that the loader does not take is freed"
        );
    }

    /// The nodes of the tree of `root` in `for_each_child` order.
    fn tree(root: Node) -> Vec<Node> {
        fn walk(n: Node, out: &mut Vec<Node>) {
            out.push(n);
            n.for_each_child(|child| {
                walk(child, out);
                false
            });
        }
        let mut out = Vec::new();
        walk(root, &mut out);
        out
    }

    /// What the node reads give for `n` (lsshells M3c twin parses): header,
    /// text, modifiers, JSDoc and the lists of its data.
    fn twin_facts(n: Node) -> String {
        let mut lists = Vec::new();
        n.for_each_child_and_lists(&mut |_| false, &mut |l, is_mod| {
            lists.push((l.nodes().len(), l.pos(), l.end(), is_mod));
        });
        format!(
            "{:?} {:?} {:?} {:?} {:?} {:?} {} {:?}",
            n.kind(),
            n.loc(),
            n.flags(),
            n.text(),
            n.modifier_flags(),
            n.modifiers().nodes().to_vec().len(),
            n.js_doc(Node::NIL).len(),
            lists
        )
    }

    // lsshells M3c: a freeable parse keeps its astdata nodes in its store
    // and answers every node read as a static parse of the same text.
    #[test]
    fn owned_parse_reads_like_a_static_parse() {
        use crate::frontend::parser::{SourceFileParseOptions, parse_source_file};
        let text = "/** doc */\nexport async function f(a: number, b = 1) { return a + b; }\n\
            let x: Array<string> = [1, 2].map(y => `${y}`);\n\
            type T = { a?: string } | number;\n\
            @dec class C<U> extends Object { private m(): U | undefined { return undefined; } }\n\
            const re = /ab+c/g, s = 'str', n = 0x10, big = 10n;\n";
        let opts = |name: &str| SourceFileParseOptions {
            file_name: name.to_string(),
            ..Default::default()
        };
        let fixed = parse_source_file(&opts("/static.ts"), text, ScriptKind::TS);
        let before = owned_node_count();
        let owned = {
            let _scope = enter_owned_parse();
            parse_source_file(&opts("/owned.ts"), text, ScriptKind::TS)
        };
        assert!(!is_freeable_parse(), "the scope ends");
        assert!(
            owned_node_count() > before,
            "the freeable parse owns its nodes"
        );
        assert!(with_store(owned.store, |s| s.owned.is_some()));
        assert!(with_store(fixed.store, |s| s.owned.is_none()));
        assert!(try_store_ast_node(fixed.root).is_some());
        assert!(
            try_store_ast_node(owned.root).is_none(),
            "an owned node has no 'static data"
        );
        let (a, b) = (tree(fixed.root), tree(owned.root));
        assert_eq!(a.len(), b.len());
        for (&a, &b) in a.iter().zip(&b) {
            assert_eq!(twin_facts(a), twin_facts(b), "{:?}", a.kind());
        }
        assert!(owned.root.statement_list().store_list().is_some());
        assert_eq!(fixed.root.statements().len(), owned.root.statements().len());
    }

    // lsshells M3c: a data write in a store that owns its nodes fills a new
    // cell, so a list handle of the old data still reads the old list (a Go
    // `*NodeList` pointer), and the pending lists are the store's.
    #[test]
    fn owned_data_write_keeps_old_list_handles() {
        let _scope = enter_owned_parse();
        let file = new_file_store("/owned-write.ts", "");
        let f = NodeFactory::for_file(file);
        let export = f.new_modifier(SyntaxKind::ExportKeyword);
        let modifiers = f.new_modifier_list(&[export]);
        assert!(modifiers.store_list().is_some(), "an owned pending list");
        let list = f.new_variable_declaration_list(NodeList::NIL, NodeFlags::NONE);
        let statement = f.new_variable_statement(modifiers, list);
        let old = statement.modifiers();
        assert!(old.store_list().is_some());
        assert_eq!(old.nodes().to_vec(), vec![export]);
        assert_eq!(old.modifier_flags(), ModifierFlags::EXPORT);
        assert_eq!(old, statement.modifiers(), "one list, one handle identity");
        assert!(list.declarations().is_nil());

        let mut data = with_ast_data(statement, |d| d.to_boxed());
        if let BoxedData::VariableStatement(d) = &mut data {
            d.modifiers = None;
        }
        replace_store_node_data(statement, data);
        assert!(statement.modifiers().is_nil());
        assert_eq!(old.nodes().to_vec(), vec![export], "the old list stays");
        assert_ne!(old, statement.modifiers());
        freeze_file_store(file);
        assert_eq!(statement.kind(), SyntaxKind::VariableStatement);
        assert_eq!(statement.declaration_list(), list);
        assert!(with_store(file, |s| s.owned_ref().len >= 4));
    }

    // AST node records, steps 2 and 3: a node whose Go parent is a node of
    // another store keeps that parent in the foreign parent table of its
    // store (`ParentCode`, `FOREIGN_PARENT`). The protected tests never make
    // one (step 2 skeptic), so this test does, and reads the parents before
    // the publish (the store), after it (the block of a static file and the
    // node shell of a freeable file version), and after that version dies
    // (its records stay leaked). Its version dies and gives its block to
    // the pool, so it holds the store build lock (`crate::test_lock::hold`).
    #[test]
    fn foreign_parents_read_through_the_file_block() {
        crate::test_lock::hold();
        let a = new_file_store("/foreign/a.ts", "x;");
        let b = new_file_store("/foreign/b.ts", "y; w;");
        let c = new_file_store("/foreign/c.ts", "v;");
        let (fa, fb, fc) = (
            NodeFactory::for_file(a),
            NodeFactory::for_file(b),
            NodeFactory::for_file(c),
        );
        let x = fa.new_identifier("x");
        let statement = fa.new_expression_statement(x);
        let z = fa.new_identifier("z");
        let y = fb.new_identifier("y");
        let w = fb.new_identifier("w");
        let v = fc.new_identifier("v");
        set_node_parent(x, statement);
        set_node_parent(statement, y);
        set_node_parent(z, w);
        set_node_parent(v, x);
        let check = |when: &str| {
            assert_eq!(x.parent(), statement, "{when}");
            assert_eq!(statement.parent(), y, "{when}");
            assert_eq!(z.parent(), w, "{when}");
            assert_eq!(v.parent(), x, "{when}");
            assert_eq!(y.parent(), Node::NIL, "{when}");
            // noderef1 G1: the view reads what the node reads read, before
            // the publish (no block), in a static block and in a node shell,
            // with local, foreign and nil parents.
            for n in [x, statement, z, y, w, v] {
                let r = NodeRef::new(n);
                assert_eq!(r.node(), n, "{when}");
                assert_eq!(r.kind(), n.kind(), "{when}");
                assert_eq!(r.loc(), n.loc(), "{when}");
                assert_eq!(r.flags(), n.flags(), "{when}");
                assert_eq!(r.parent().node(), n.parent(), "{when}");
                let parent_kind = if n.parent().is_some() {
                    n.parent().kind()
                } else {
                    SyntaxKind::Unknown
                };
                assert_eq!(r.parent().kind_or_unknown(), parent_kind, "{when}");
            }
            assert_eq!(NodeRef::new(statement).expression(), x, "{when}");
            assert_eq!(
                NodeRef::new(v).parent().parent().node(),
                statement,
                "{when}"
            );
            assert!(NodeRef::new(y).parent().is_nil(), "{when}");
            assert_eq!(store_header(statement).parent, y, "{when}");
            assert_eq!(store_header(v).parent, x, "{when}");
        };
        check("built");
        for file in [a, b, c] {
            freeze_file_store(file);
        }
        with_store(a, |s| {
            assert_eq!(s.foreign_parents, [y, w]);
            assert!(!s.facts.parents_local);
        });
        check("frozen");

        // c is a freeable file version: the block of its id is its node
        // shell, and the version owns its store.
        let version = super::super::file_version::FileVersion::new(c);
        crate::program::publish_parsed_files("/");
        assert!(file_block(a).is_some_and(|block| block.file.store.is_some()));
        assert!(file_block(c).is_some_and(|block| block.file.store.is_none()
            && block.file.go_file.is_none()
            && *block.file.foreign == [x]));
        check("published");
        // A parent in the same store keeps the block of the view; a foreign
        // parent takes the block of its own file.
        let block_of = |r: NodeRef| r.block.map(|b| b as *const FileBlock);
        let block_of_file = |file: usize| file_block(file).map(|b| b as *const FileBlock);
        assert_eq!(block_of(NodeRef::new(x).parent()), block_of_file(a));
        assert_eq!(block_of(NodeRef::new(statement).parent()), block_of_file(b));
        assert_eq!(block_of(NodeRef::new(v).parent()), block_of_file(a));
        assert_eq!(frozen_store_parent(x), Some(statement));
        assert_eq!(frozen_store_parent(statement), None, "the header has it");
        assert_eq!(frozen_store_parent(v), None, "the header has it");
        assert_eq!(
            frozen_store_parent_kind(x),
            Some((statement, SyntaxKind::ExpressionStatement))
        );
        assert_eq!(frozen_store_parent_kind(statement), None);
        assert_eq!(
            frozen_find_ancestor(x, |_, _| false),
            Some(AncestorWalk::Next(y))
        );
        assert_eq!(
            frozen_find_ancestor(v, |_, _| false),
            Some(AncestorWalk::Next(x))
        );
        let parents_local = |file| frozen_store_facts(file).map(|facts| facts.parents_local);
        assert_eq!(parents_local(a), Some(false));
        assert_eq!(parents_local(b), Some(true));
        assert_eq!(parents_local(c), Some(false));
        assert!(
            !frozen_store_lacks_deprecated_tag(x),
            "parents leave store a"
        );

        // A record read of a dead version gives the values of that node.
        assert_eq!(Arc::strong_count(&version), 1, "no pin holds the version");
        drop(version);
        assert_eq!(v.parent(), x);
        assert_eq!(
            frozen_find_ancestor(v, |_, _| false),
            Some(AncestorWalk::Next(x))
        );
    }

    // AST node records, step 3: a file id from `LOW_BLOCKS` on has its
    // block in a chunk of `HIGH_BLOCKS` (a process with many programs).
    // This test process never gets that many file ids, so the test sets
    // the block of the last file id, which no publish reaches, and reads
    // its node through the node reads.
    #[test]
    fn high_file_ids_read_their_block_from_a_chunk() {
        set_and_read_high_file_block(FILE_ID_LIMIT - 1);
    }

    // fileid1: ids are never used again, so a long session (one id per
    // edit) or a long watch could reach the old limit of 2^22 ids and
    // panic ("too many file ids"). The id at the old limit has a block now.
    #[test]
    fn file_ids_past_the_old_limit_read_their_block() {
        set_and_read_high_file_block(1 << 22);
    }

    // fileid1: with a test cap, the last id below the cap is given and
    // published, and the next new store stops at the cap. It reads the
    // published count, so it holds the store build lock
    // (`crate::test_lock::hold`).
    #[test]
    fn new_file_ids_stop_at_the_cap() {
        use std::panic::{AssertUnwindSafe, catch_unwind};
        let message = std::thread::spawn(|| {
            crate::test_lock::hold();
            let cap = PUBLISHED.load(Ordering::Acquire) + 1;
            TEST_FILE_ID_CAP.set(cap);
            let last = new_file_store("/fileidcap/a.ts", "a;");
            assert_eq!(last, cap - 1);
            NodeFactory::for_file(last).new_identifier("a");
            freeze_file_store(last);
            crate::program::publish_parsed_files("/");
            assert_eq!(PUBLISHED.load(Ordering::Acquire), cap);
            assert!(file_block(last).is_some(), "the last id is published");
            let next = catch_unwind(AssertUnwindSafe(|| new_file_store("/fileidcap/b.ts", "b;")));
            let payload = next.expect_err("a new store at the cap must panic");
            payload
                .downcast_ref::<&str>()
                .map(|m| (*m).to_owned())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_default()
        })
        .join()
        .unwrap();
        assert_eq!(message, "too many file ids");
    }

    // factscol1b: a static publish gets a facts column at its first facts
    // read, one word per slot, shared by all threads. A node shell (a
    // freeable file version) gets none, so its facts stay in the
    // thread-local map that forgets them with the version. The test sets
    // the blocks of two file ids that no publish reaches.
    #[test]
    fn static_files_keep_subtree_facts_in_a_column() {
        fn block(file: usize, store: Option<&'static FileStore>) -> FileBlock {
            let mut records = vec![NodeRecord::target(Node::NIL), NodeRecord::node(false)];
            *records[0].bind.get_mut() = NodeRecord::owner_word(file);
            FileBlock {
                kinds: Vec::leak(vec![SyntaxKind::Unknown, SyntaxKind::Block]),
                records: Vec::leak(records),
                kids: Vec::leak(vec![NodeKids::unknown(), NodeKids::unknown()]),
                file: Box::leak(Box::new(BlockFile {
                    nodes: &[],
                    lists: &[],
                    facts: FactsCell::new(StoreFacts::ONLY_NIL_SLOT),
                    root: AtomicU64::new(Node::NIL.0),
                    foreign: &[],
                    links: &[],
                    shell_kinds: &[],
                    store,
                    go_file: None,
                    subtree_facts: OnceLock::new(),
                })),
            }
        }
        let (static_file, shell_file) = (FILE_ID_LIMIT - 5, FILE_ID_LIMIT - 6);
        let store: &'static FileStore = Box::leak(Box::default());
        set_file_block(static_file, block(static_file, Some(store)));
        set_file_block(shell_file, block(shell_file, None));
        let n = handle(static_file, 1);
        let word = static_facts_word(n).expect("a static file has a column");
        assert_eq!(word.load(Ordering::Relaxed), 0, "no facts computed yet");
        word.store(7, Ordering::Relaxed);
        let column = file_block(static_file).unwrap().file.subtree_facts.get();
        assert_eq!(column.map(|c| c.len()), Some(2), "one word per slot");
        let other_thread =
            std::thread::spawn(move || static_facts_word(n).map(|w| w.load(Ordering::Relaxed)));
        assert_eq!(other_thread.join().unwrap(), Some(7));
        assert!(static_facts_word(handle(shell_file, 1)).is_none());
        assert!(static_facts_word(Node::NIL).is_none());
    }

    /// Sets the block of unpublished file id `file` (from `LOW_BLOCKS` on)
    /// and reads its node through the node reads.
    fn set_and_read_high_file_block(file: usize) {
        let mut records = vec![NodeRecord::target(Node::NIL), NodeRecord::node(false)];
        records[1].set_flags(NodeFlags::AMBIENT);
        records[1].set_loc(TextRange::new(3, 7));
        // AST node records, step 4: the owner word (`block_is_owned`).
        *records[0].bind.get_mut() = NodeRecord::owner_word(file);
        let block = FileBlock {
            kinds: Vec::leak(vec![SyntaxKind::Unknown, SyntaxKind::Identifier]),
            records: Vec::leak(records),
            kids: Vec::leak(vec![NodeKids::unknown(), NodeKids::unknown()]),
            file: Box::leak(Box::new(BlockFile {
                nodes: &[],
                lists: &[],
                facts: FactsCell::new(StoreFacts::ONLY_NIL_SLOT),
                root: AtomicU64::new(Node::NIL.0),
                foreign: &[],
                links: &[],
                shell_kinds: &[],
                store: None,
                go_file: None,
                subtree_facts: OnceLock::new(),
            })),
        };
        assert!(file_block(file).is_none());
        set_file_block(file, block);
        let n = handle(file, 1);
        assert_eq!(n.kind(), SyntaxKind::Identifier);
        assert_eq!(n.flags(), NodeFlags::AMBIENT);
        assert_eq!(n.loc(), TextRange::new(3, 7));
        assert_eq!(n.parent(), Node::NIL);
        assert_eq!(
            resolve_store_id(file, crate::astdata::NodeId::new(0)),
            Node::NIL
        );
        assert!(file_block(file - 1).is_none(), "the id before");
        assert!(file_block(file - HIGH_CHUNK).is_none(), "another chunk");
        assert!(file_block(LOW_BLOCKS).is_none(), "the first chunk");
        assert!(file_block(FILE_ID_LIMIT).is_none(), "no file id");
    }

    // textleak1 A1: a publish keeps the emptied cells of its build stores,
    // and the next build store of the thread reuses one, so a language
    // server edit leaks no store cell in the AST arena.
    #[test]
    fn next_build_stores_reuse_published_cells() {
        std::thread::spawn(|| {
            let a = new_file_store("/cells/a.ts", "a;");
            let a_cell = active_store(a).expect("a is the active store");
            NodeFactory::for_file(a).new_identifier("a");
            freeze_file_store(a);
            crate::program::publish_parsed_files("/");
            let b = new_file_store("/cells/b.ts", "b;");
            let b_cell = active_store(b).expect("b is the active store");
            assert!(
                std::ptr::eq(a_cell, b_cell),
                "b does not reuse the cell of a"
            );
            assert_eq!(file_store_text(b), "b;");
            assert_eq!(file_store_text(a), "a;");
        })
        .join()
        .unwrap();
    }

    // astmem1 P3: the bind ORs its added flags into the `flags` word of a
    // record, whose top bits are the record bits (`RECORD_BITS`). A flag
    // outside `BINDER_ADDED_FLAGS` (a binder bug) stops the bind in every
    // build, before it writes the record. Its build takes the store build
    // lock (`crate::test_lock`).
    #[test]
    fn a_bind_flag_outside_binder_added_flags_panics_in_every_build() {
        use std::panic::{AssertUnwindSafe, catch_unwind};
        let file = new_file_store("/bindflags/a.ts", "x;");
        let x = NodeFactory::for_file(file).new_identifier("x");
        freeze_file_store(file);
        crate::program::publish_parsed_files("/");
        let data = NodeBindData {
            added_flags: NodeFlags(NodeRecord::SOURCE_FILE_ROOT),
            ..NodeBindData::default()
        };
        let record = || &file_block(file).expect("a is published").records[slot_index(x)];
        let bits = record().bits();
        let bind = || {
            bind_store_records(
                file,
                std::iter::once((slot_index(x), 0, Some(&data), FlowNodeId::NIL)),
            )
        };
        let payload = catch_unwind(AssertUnwindSafe(bind)).expect_err("the bind must panic");
        let message = payload
            .downcast_ref::<String>()
            .map(String::as_str)
            .unwrap_or_default();
        assert!(
            message.contains("outside BINDER_ADDED_FLAGS"),
            "{message:?}"
        );
        assert_eq!(record().bits(), bits, "the bind changed the record bits");
    }

    // AST node records, step 4: the node shell of a freeable file version
    // takes a pooled block (`BlockPool`). A dead version gives its block
    // back, the block waits for two pin releases, and then the next
    // node shell that fits it takes it. A stale read of the dead version
    // then fails the owner check, and panics as a read of a dead version's
    // store does: a binder field read (the symbol, the flags, the flow node
    // and the extras) in every build (`check_block_owner`), every other
    // read with debug assertions. Without them a header read gives the new
    // owner's data (these shells have plain kind columns, which are not
    // pooled; shells with equal kinds share one: `plain_kinds`). It reads
    // the block pool and counts pin releases, so it holds the store build
    // lock (`crate::test_lock::hold`).
    #[test]
    fn pooled_node_blocks_wait_two_releases_and_check_their_owner() {
        use super::super::file_version::{FileVersion, release_file_version_pins};
        use std::panic::{AssertUnwindSafe, catch_unwind};
        crate::test_lock::hold();
        clear_block_pool();
        // Publishes a freeable version of one identifier per name.
        let identifiers = |name: &'static str, names: &[&str]| {
            let text: &'static str = Box::leak(names.join(";").into_boxed_str());
            let file = new_file_store(name, text);
            let factory = NodeFactory::for_file(file);
            let nodes: Vec<Node> = names.iter().map(|n| factory.new_identifier(*n)).collect();
            freeze_file_store(file);
            let version = FileVersion::new(file);
            crate::program::publish_parsed_files("/");
            (version, nodes)
        };
        let addr = |n: Node| node_block_addr(n).expect("a published store node");

        // c, bound: its second node has a symbol and a flow node.
        let (c, c_nodes) = identifiers("/pool/c.ts", &["x", "y"]);
        let (c_file, cy) = (c.file(), c_nodes[1]);
        let c_block = addr(cy);
        assert!(node_block_is_owned(cy));
        file_block(c_file).expect("c is published").records[slot_index(cy)].write_bind(
            SymbolId(7),
            NodeFlags::NONE,
            3,
        );
        assert_eq!(frozen_store_symbol(cy), Some(SymbolId(7)));
        assert_eq!(Arc::strong_count(&c), 1, "no pin holds the version");
        drop(c);
        assert!(node_block_is_owned(cy), "the block of a dead version waits");
        assert_eq!(cy.kind(), SyntaxKind::Identifier);

        // Its block waits for two pin releases.
        let (d, d_nodes) = identifiers("/pool/d.ts", &["z", "w"]);
        assert_ne!(addr(d_nodes[0]), c_block, "no release yet");
        // Shells with equal kinds share one kind column (`plain_kinds`).
        let kinds_of = |n: Node| {
            registry_block(n.file_index())
                .expect("a published store node")
                .kinds
                .as_ptr()
        };
        assert_eq!(kinds_of(d_nodes[0]), kinds_of(cy), "one kind column");
        release_file_version_pins();
        let (d1, d1_nodes) = identifiers("/pool/d1.ts", &["u", "t"]);
        assert_ne!(addr(d1_nodes[0]), c_block, "one release");
        release_file_version_pins();

        // e has as many slots as c, so it takes c's block.
        let e_file = new_file_store("/pool/e.ts", "q;");
        let factory = NodeFactory::for_file(e_file);
        let q = factory.new_identifier("q");
        let statement = factory.new_expression_statement(q);
        set_node_parent(q, statement);
        freeze_file_store(e_file);
        let e = FileVersion::new(e_file);
        crate::program::publish_parsed_files("/");
        assert_eq!(slot_index(statement), slot_index(cy));
        assert_eq!(addr(statement), c_block, "two releases");
        assert!(node_block_is_owned(statement));
        assert!(!node_block_is_owned(cy), "c's block has a new owner");
        assert_eq!(statement.kind(), SyntaxKind::ExpressionStatement);
        assert_eq!(q.parent(), statement);
        assert_eq!(q.kind(), SyntaxKind::Identifier);
        assert_eq!(
            frozen_store_symbol(statement),
            Some(SymbolId::NIL),
            "not bound"
        );
        assert_eq!(frozen_store_bind_word(statement), Some(0), "not bound");

        assert_ne!(kinds_of(statement), kinds_of(cy), "other kinds");

        // A stale read of c.
        let released = format!("file version {c_file} is released");
        let panic_message = |read: &dyn Fn()| {
            catch_unwind(AssertUnwindSafe(read)).err().map(|payload| {
                payload
                    .downcast_ref::<String>()
                    .cloned()
                    .unwrap_or_default()
            })
        };
        let binder_reads: [(&str, &dyn Fn()); 6] = [
            ("symbol", &|| {
                let _ = cy.symbol();
            }),
            ("flags", &|| {
                let _ = cy.flags();
            }),
            ("parser flags", &|| {
                let _ = cy.parser_flags(NodeFlags::AMBIENT);
            }),
            ("flow node", &|| {
                let _ = cy.flow_node();
            }),
            ("extras", &|| {
                let _ = cy.local_symbol();
            }),
            ("any symbol", &|| {
                let _ = frozen_store_any_symbol(c_file, |_| false);
            }),
        ];
        for (name, read) in binder_reads {
            assert_eq!(
                panic_message(read).as_deref(),
                Some(released.as_str()),
                "a stale {name} read of a reused block"
            );
        }
        let kind = panic_message(&|| {
            let _ = cy.kind();
        });
        if cfg!(debug_assertions) {
            assert_eq!(kind.as_deref(), Some(released.as_str()));
        } else {
            // The kind column is not pooled, so the kind is c's. A header
            // read gives the record of e: x has the parent of q, in c.
            assert_eq!(kind, None);
            assert_eq!(cy.kind(), SyntaxKind::Identifier);
            assert_eq!(slot_index(c_nodes[0]), slot_index(q));
            assert_eq!(c_nodes[0].parent(), cy);
        }

        // A version with more slots than a free block has gets a fresh
        // block; one that fits takes it.
        let (d_block, d1_block) = (addr(d_nodes[0]), addr(d1_nodes[0]));
        drop((d, d1));
        release_file_version_pins();
        release_file_version_pins();
        let names: Vec<String> = (0..80).map(|i| format!("n{i}")).collect();
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        let (f, f_nodes) = identifiers("/pool/f.ts", &names);
        assert!(![c_block, d_block, d1_block].contains(&addr(f_nodes[0])));
        assert_eq!(f_nodes[79].kind(), SyntaxKind::Identifier);
        let (g, g_nodes) = identifiers("/pool/g.ts", &["s"]);
        assert!([d_block, d1_block].contains(&addr(g_nodes[0])));
        assert_eq!(g_nodes[0].kind(), SyntaxKind::Identifier);
        drop((e, f, g));
    }

    // watchfree1: a node shell shares the kind column of any earlier shell
    // with equal kinds, not only of the last few (`plain_kinds`): a
    // `tsc --watch` build after a config change parses every file again.
    // It reads the block pool, so it holds the store build lock
    // (`crate::test_lock::hold`).
    #[test]
    fn node_shells_share_any_equal_kind_column() {
        use super::super::file_version::FileVersion;
        crate::test_lock::hold();
        clear_block_pool();
        // Publishes a freeable version of `count` identifiers, and gives
        // the address of its kind column.
        let column = |count: usize| {
            let names: Vec<String> = (0..count).map(|i| format!("n{i}")).collect();
            let text: &'static str = Box::leak(names.join(";").into_boxed_str());
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let id = NEXT.fetch_add(1, Ordering::Relaxed);
            let name: &'static str = Box::leak(format!("/kinds/{id}.ts").into_boxed_str());
            let file = new_file_store(name, text);
            let factory = NodeFactory::for_file(file);
            let node = names
                .iter()
                .map(|n| factory.new_identifier(n.as_str()))
                .last()
                .expect("one name");
            freeze_file_store(file);
            let version = FileVersion::new(file);
            crate::program::publish_parsed_files("/");
            let kinds = registry_block(node.file_index())
                .expect("a published store node")
                .kinds
                .as_ptr();
            (version, kinds)
        };
        let (_first, first) = column(1);
        let others: Vec<_> = (2..9).map(column).collect();
        assert!(
            others.iter().all(|(_, kinds)| *kinds != first),
            "other kinds, other columns"
        );
        let (_again, again) = column(1);
        assert_eq!(again, first, "equal kinds share the first column");
    }

    /// identparse1: `identifier_name` gives the name and keyword bit of the
    /// intern for texts of each key form (up to 3, 4 to 7, 8 to 15, 16 to 24
    /// and more bytes), for long texts whose keys are equal, and after the cache grew
    /// and dropped entries.
    #[test]
    fn identifier_name_cache_gives_the_interned_name() {
        let mut names = IdentNames::default();
        let mut check = |text: &str| {
            let (name, keyword) = identifier_name(&mut names, text);
            assert_eq!(name, Name::from(text), "{text:?}");
            assert_eq!(name.as_str(), text);
            assert_eq!(
                keyword,
                get_identifier_token(text) != SyntaxKind::Identifier,
                "{text:?}"
            );
        };
        let mut texts: Vec<String> = ["", "a", "ab", "abc", "acb", "if", "await", "instanceof"]
            .map(String::from)
            .to_vec();
        for len in 1..=40 {
            for fill in ['x', 'y'] {
                let mut text: Vec<char> = std::iter::repeat_n('m', len).collect();
                text[len / 2] = fill;
                texts.push(text.into_iter().collect());
            }
        }
        // More texts than the largest table has entries, so sets drop some.
        texts.extend((0..3 * 2 * IDENT_MAX_SETS).map(|i| format!("identparse1_{i}")));
        for _ in 0..2 {
            for text in &texts {
                check(text);
            }
        }
    }

    // edleak3: the pooled kind table names each kind at its value, and
    // `Unknown` past the last (`BlockFile::shell_kind`).
    #[test]
    fn pooled_kind_table_has_every_kind() {
        for (word, &kind) in KIND_OF_WORD.iter().enumerate() {
            let expected = u16::try_from(word)
                .ok()
                .and_then(|word| SyntaxKind::try_from(word).ok())
                .unwrap_or(SyntaxKind::Unknown);
            assert_eq!(kind, expected, "word {word}");
        }
        assert!(SyntaxKind::COUNT <= KIND_OF_WORD.len());
    }

    // edleak1 round 2: the first node shell of a file gets a plain kind
    // column. A later version with new kinds gets one while the columns of
    // later versions stay in `LATER_KINDS_BUDGET`; past it, its kinds go
    // in its pooled block (`BlockFile::shell_kinds`), and its plain column
    // is the prefix of the newest column of the file that has the same
    // kinds (edleak3). Equal kinds share a plain column at any time. It
    // reads the block pool, so it holds the store build lock
    // (`crate::test_lock::hold`).
    #[test]
    fn later_new_kinds_go_in_the_pooled_block_past_the_budget() {
        use super::super::file_version::FileVersion;
        crate::test_lock::hold();
        // Empties the pool again when the test ends, after its versions
        // drop (it is made first, so it drops last), so a later test in the
        // process does not start with the budget used up.
        struct ClearPoolAtEnd;
        impl Drop for ClearPoolAtEnd {
            fn drop(&mut self) {
                clear_block_pool();
            }
        }
        let _clear = ClearPoolAtEnd;
        clear_block_pool();
        // Publishes a freeable version of file `name` with `count`
        // identifiers, and gives its plain kind column (a prefix when its
        // kinds are pooled) and its last node.
        let version = |name: &'static str, count: usize| {
            let names: Vec<String> = (0..count).map(|i| format!("n{i}")).collect();
            let text: &'static str = Box::leak(names.join(";").into_boxed_str());
            let file = new_file_store(name, text);
            let factory = NodeFactory::for_file(file);
            let node = names
                .iter()
                .map(|n| factory.new_identifier(n.as_str()))
                .last()
                .expect("one name");
            freeze_file_store(file);
            let version = FileVersion::new(file);
            crate::program::publish_parsed_files("/");
            let b = registry_block(node.file_index()).expect("a published store node");
            assert_eq!(node.kind(), SyntaxKind::Identifier);
            (version, b.kinds, node)
        };
        let later_bytes = || lock_pool().later_kind_bytes;

        let (_v1, first, _) = version("/later/a.ts", 1);
        assert!(!first.is_empty(), "the first shell of a file");
        assert_eq!(later_bytes(), 0, "a first column is not in the budget");
        let (_v2, second, _) = version("/later/a.ts", 2);
        assert!(!second.is_empty(), "new kinds in the budget");
        assert_eq!(later_bytes(), std::mem::size_of_val(second));

        lock_pool().later_kind_bytes = LATER_KINDS_BUDGET;
        let (_v3, third, node) = version("/later/a.ts", 3);
        assert!(
            third.len() <= slot_index(node),
            "new kinds past the budget are pooled"
        );
        assert_eq!(
            (third.as_ptr(), third.len()),
            (second.as_ptr(), second.len()),
            "the slots before the first new kind read the newest column"
        );
        let pooled = registry_block(node.file_index()).expect("a published store node");
        assert_eq!(pooled.kind(slot_index(node)), SyntaxKind::Identifier);
        assert_eq!(pooled.kind(NIL_SLOT as usize), first[NIL_SLOT as usize]);
        assert_eq!(later_bytes(), LATER_KINDS_BUDGET);

        let (_v4, again, _) = version("/later/a.ts", 2);
        assert_eq!(
            again.as_ptr(),
            second.as_ptr(),
            "equal kinds share a column"
        );
        let (_b1, other, _) = version("/later/b.ts", 4);
        assert!(!other.is_empty(), "the first shell of another file");
        assert_eq!(later_bytes(), LATER_KINDS_BUDGET);
    }

    /// followups55 item 3: a store whose parse never ends (a parse worker
    /// drops the detached store of a parse that panicked) gives the
    /// identifier name cache of its thread back when it drops.
    #[test]
    fn unfinished_store_gives_the_identifier_cache_back_on_drop() {
        // Its own thread, so it has its own cache and no detached store.
        std::thread::spawn(|| {
            let thread_has_cache = || {
                IDENTIFIER_NAMES.with(|spare| {
                    let idents = spare.take();
                    let has = idents
                        .as_ref()
                        .is_some_and(|idents| !idents.sets.is_empty());
                    spare.set(idents);
                    has
                })
            };
            let file = new_detached_file_store(0, "/idents-drop.ts", "a");
            NodeFactory::for_file(file).new_identifier("a");
            assert!(!thread_has_cache(), "the store holds the cache");
            let store = take_detached_file_store().expect("the detached store");
            assert!(!store.store.frozen);
            drop(store);
            assert!(thread_has_cache(), "the drop gives the cache back");
        })
        .join()
        .expect("the test thread");
    }
}
