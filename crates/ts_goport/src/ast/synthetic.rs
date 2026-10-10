//! Synthetic AST nodes: the nodes Go creates with `ast.NodeFactory`.
//!
//! Go factory nodes are ordinary `*ast.Node` values. Here a synthetic node is
//! a `Node` handle whose file index is `SYNTHETIC_NODE_FILE`. Its low 32 bits
//! index a thread-local slot list (like `SYNTHETIC_FLOW_FILE` for flow nodes).
//!
//! Each node slot names a `crate::astdata::Node` (kind and `NodeData`) that the
//! thread owns, so the accessors in `node.rs` and `fields.rs` that match on
//! `NodeData` work on synthetic nodes too. Child ids inside that data are ids
//! in the synthetic id space:
//! - a synthetic child uses its own slot index;
//! - a parsed child (Go shares the pointer) uses an alias slot. `Node::new`
//!   resolves an alias slot to the parsed node, so identity is kept:
//!   `synthetic.name() == parsed_name` is true, like Go pointer equality;
//! - Go `nil` in a field that astdata stores as a required `NodeId` uses slot 0,
//!   which resolves to `Node::NIL`.
//!
//! Go mutates factory nodes after creation (`node.Parent = p`, `node.Loc = l`,
//! `node.Flags |= f`, `node.FlowNodeData().FlowNode = f`,
//! `node.AsX().Symbol = s`). Those fields live in the slot, not in the
//! `crate::astdata::Node`, and `node.rs` reads them through the hooks below.
//!
//! Ownership: the slots, the node data, the factory lists and the node
//! slices that reads make belong to the thread's arena. Nothing hands out a
//! reference into them that outlives a read. Data is read inside a closure
//! (`with_ast_data`, or the `with_data!` macro in hot code),
//! and a list field through a selector (`list_of!`, `modifiers_of!`). A list
//! of synthetic data is a `SyntheticList` handle (an index), not a pointer,
//! and a text is interned (`synthetic_text`). Only a parsed node of a
//! static parse gives `&'static` data (`static_ast_node`). A node that a
//! freeable parse owns (lsshells M3c, `ast/store.rs` `OwnedAst`) is read the
//! same way as a synthetic node: in a scope (`with_scoped_ast_node`,
//! `read_scoped_ast_node`), with lists as handles (`ast::StoreList`, whose
//! selector has a small id, `SelectorSite`). A checker worker of a released
//! program frees all of it (`free_synthetic_nodes`, called by
//! `program::release_program`). A one-program process leaks it at the end
//! instead (`forget_synthetic_nodes`), like the checker itself.
//!
//! Owners: the language server makes the nodes of every program version on
//! its dispatch thread. There each program version owns the entries made
//! while it is current (`open_synthetic_owner`), and its release frees them
//! (`free_synthetic_owner`, called by `ls_program`). Other entries belong to
//! the thread (the base owner). The tables are in chunks, and each chunk
//! has one owner, so a handle stays an index and keeps Go pointer identity
//! while its owner lives. A freed chunk leaves a hole that is never used
//! again: a read of a freed entry panics, and a handle never names another
//! node. The rules:
//! - a new node, a factory list and a list copy go to the current program
//!   version when it is an owner on this thread and no base scope
//!   (`enter_base_synthetic_owner`) is open, else to the base owner;
//! - a data write (`replace_node_data`, `set_node_kind`) goes to the owner
//!   of the node, so a base node never points into a program version;
//! - an alias slot goes to the file version of its parsed node when that is
//!   a freeable version (`ast/file_version.rs`), else to the base owner. An
//!   alias lives as long as the node it names: after the version dies, the
//!   next new alias or file scope frees the entries of that version
//!   (`free_dead_aliases`). A read of such an entry panics, as a read of
//!   the parsed node does;
//! - in a file scope (`enter_file_synthetic_owner`), a new node, factory
//!   list or list copy goes to the file version of a parsed node in the
//!   same way. Lazy JSDoc opens one: its cache keeps the JSDoc nodes of a
//!   parsed node for as long as that node lives;
//! - node slices always go to the base owner;
//! - in a print scope (`enter_print_scope`, a to-string call of the
//!   checker), a new node or list of a to-string factory
//!   (`NodeFactory::set_print_owned`) goes to the print owner of the
//!   thread. The end of the call frees them, unless the call is pinned
//!   (`pin_print_scope`): then they go to the current owner.
//!
//! Code whose nodes a cache keeps across program versions (the token cache,
//! parses) opens a base scope.
//!
//! d.ts twins: a checker thread and the thread that prints its d.ts files
//! (`program::send_dts_twin_job`) take new chunk numbers from one counter
//! per table (`share_synthetic_chunks`), so a chunk number names a chunk of
//! one of the two threads only. The twin starts with no entries
//! (`install_twin_synthetic_arena`) and gets a copy of the checker entries
//! that each tree it prints reaches, at their indexes (`print_pack`). A
//! read of an entry that no pack copied panics (`Slot::Absent`).
//!
//! PORT: the Go GC frees request garbage at once and checker nodes with
//! their checker. Here they stay until their program version is released.
//!
//! PORT: parsed nodes are immutable. The setters panic on a parsed node,
//! except on a node of a ported-parser file that the parser has not finished
//! (`ast/store.rs`). Go code that writes a field of a parsed node needs its
//! own port decision.

use crate::astdata::NodeData;
use crate::prelude::*;
use std::cell::{Cell, OnceCell};
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};

mod print_pack;
pub use print_pack::{PrintPack, export_print_pack, install_print_pack, release_print_packs};

/// File index of synthetic nodes. `SYNTHETIC_FLOW_FILE` is `0xffff_ffff`.
pub const SYNTHETIC_NODE_FILE: usize = 0xffff_fffe;

/// Slot 0: Go `nil` stored in a astdata field that has no `Option`.
const NIL_SLOT: u32 = 0;

/// One synthetic slot.
#[derive(Clone)]
enum Slot {
    /// Go `nil`. Only slot 0.
    Nil,
    /// A parsed node used as a child of a synthetic node.
    Alias(Node),
    /// A node the factory created.
    Node(SyntheticNode),
    /// A slot of a checker thread that no print pack copied to its d.ts
    /// twin (`print_pack`). A read panics.
    Absent,
    /// A node of a to-string call that ended (`exit_print_scope`). A read
    /// panics.
    Freed,
}

/// The mutable Go `NodeBase` fields of a factory node.
#[derive(Clone)]
struct SyntheticNode {
    /// The astdata node (kind and data): an entry of `SyntheticArena::datas`.
    /// A Go write to a data field or to the kind adds a new entry and moves
    /// the node to it, so a list handle taken before the write still reads
    /// the old list, like a Go `*NodeList` pointer.
    data: u32,
    /// The kind of the astdata node `data`, so a kind read needs no data
    /// lookup (emitast1 F3).
    kind: SyntaxKind,
    /// Go `CompositeBase.facts` (`Node::subtree_facts`; emitast1 F1).
    facts: SlotFacts,
    parent: Node,
    flags: NodeFlags,
    loc: TextRange,
    /// Go `DeclarationData().Symbol`, `FlowNodeData().FlowNode`, ... `None`
    /// until the first write.
    bind: Option<Box<NodeBindData>>,
    /// Go `SyntheticExpression.Type.(*Type)`. Nil for other kinds.
    synthetic_type: TypeId,
    /// The Go `ast.SourceFile` fields of a factory SourceFile. `None` for
    /// other kinds.
    source_file: Option<Box<SyntheticSourceFileData>>,
}

/// The cached `Node::subtree_facts` of a factory node: `COMPUTED` is set
/// once they are cached. A copy (`Clone`) has none cached: each thread
/// computes the facts of its nodes, as with the per-thread map of parsed
/// nodes (`ast::SUBTREE_FACTS`), so a node that a seed or a print pack
/// copies gets the facts of its data on the new thread.
#[derive(Default)]
struct SlotFacts(SubtreeFacts);

impl Clone for SlotFacts {
    fn clone(&self) -> Self {
        Self::default()
    }
}

/// The Go `ast.SourceFile` fields (other than `Statements` and
/// `EndOfFileToken`, which are in the node data) of a SourceFile that the
/// factory made: `NewSourceFile` sets the first group, `copyFrom` and later
/// Go writes (`result.AsSourceFile().IsDeclarationFile = true`) set the rest.
// PORT: a parsed SourceFile keeps these fields in `SourceFileInfo`. astdata
// node data cannot hold them, so a factory SourceFile keeps them in its slot.
// Go `parseOptions` is kept as its file name and path; the external module
// indicator options are only read by the parser.
#[derive(Clone, Debug, Default)]
pub struct SyntheticSourceFileData {
    // Fields set by NewSourceFile
    pub file_name: &'static str,
    pub path: String,
    pub text: FileText,

    // Fields set by copyFrom (Go "fields set by parser") and later writes
    pub language_variant: LanguageVariant,
    pub script_kind: ScriptKind,
    pub is_declaration_file: bool,
    pub uses_uri_style_node_core_modules: Tristate,
    pub imports: Vec<Node>,
    pub module_augmentations: Vec<Node>,
    pub ambient_module_names: Vec<String>,
    pub comment_directives: Vec<CommentDirective>,
    pub pragmas: Vec<Pragma>,
    pub referenced_files: Vec<FileReference>,
    pub type_reference_directives: Vec<FileReference>,
    pub lib_reference_directives: Vec<FileReference>,
    pub common_js_module_indicator: Node,
    pub external_module_indicator: Node,
    /// Go `contentMapperInfo` (tsgo#4712), set by `copyFrom`.
    pub content_mapper_info: Option<&'static ContentMapperFileInfo>,
}

/// A list that the factory made (`new_synthetic_node_list`,
/// `new_synthetic_modifier_list`) or that a read copied
/// (`copy_synthetic_list`).
#[derive(Clone)]
enum OwnList {
    Nodes(crate::astdata::NodeList),
    Modifiers(crate::astdata::ModifierList),
    /// A list of a to-string call that ended. A read panics.
    Freed,
}

impl OwnList {
    fn get(&self) -> AnyList<'_> {
        match self {
            Self::Nodes(l) => AnyList::Nodes(l),
            Self::Modifiers(m) => AnyList::Modifiers(m),
            Self::Freed => panic!("{PRINT_FREED}"),
        }
    }
}

/// Bytes per chunk of `SyntheticArena::slots`, `datas` and `lists`.
// PERF: 14 KiB is jemalloc's largest small size class, and small classes
// pack densely in slabs. A larger chunk is a large allocation, which starts
// at a random cache line offset and so touches one page more (10% for a
// 40 KiB chunk).
const CHUNK_BYTES: usize = 14 * 1024;

/// Slots per chunk of `SyntheticArena::slots`.
const SLOT_CHUNK: usize = CHUNK_BYTES / size_of::<Slot>();
const _: () = assert!(SLOT_CHUNK >= 128);

/// Cells per chunk of `SyntheticArena::datas`. The `Rc` header is 16 bytes.
const DATA_CHUNK: usize = (CHUNK_BYTES - 16) / size_of::<OnceCell<crate::astdata::Node>>();

/// Lists per chunk of `SyntheticArena::lists`.
const LIST_CHUNK: usize = CHUNK_BYTES / size_of::<OwnList>();

/// A chunk of `SyntheticArena::datas`: cells that fill in order.
type DataChunk = Rc<[OnceCell<crate::astdata::Node>]>;

/// The panic of a read of an entry whose owner was freed.
const FREED: &str = "synthetic entry of a released program or file version is read";

/// The panic of a read of an entry of a to-string call that ended
/// (`Slot::Freed`, `OwnList::Freed`; see `exit_print_scope`).
const PRINT_FREED: &str = "a synthetic node of a to-string call that ended is read";

/// The panic of a read on a d.ts twin of a checker entry that no print pack
/// copied (`Slot::Absent`).
const ABSENT: &str = "a d.ts twin reads a synthetic node that its print pack does not hold";

/// The tables of `SyntheticArena` that have chunk numbers, as indexes of
/// `SharedChunks::next`.
const SLOT_TABLE: usize = 0;
const DATA_TABLE: usize = 1;
const LIST_TABLE: usize = 2;

/// The next chunk number of each table (`SLOT_TABLE`, `DATA_TABLE`,
/// `LIST_TABLE`), shared by a checker thread and its d.ts twin
/// (`share_synthetic_chunks`).
pub struct SharedChunks {
    next: [AtomicU32; 3],
}

/// The owner of an arena chunk (see "Owners" in the module comment).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OwnerKey {
    /// The thread. Its chunks go only with the whole arena.
    Base,
    /// A language server program version (`GoProgram::id`).
    Program(u32),
    /// The alias slots of the nodes of a freeable file version (its file
    /// id) and the entries made in its file scopes, in chunks of their own.
    File(u32),
    /// The nodes and lists that a to-string factory makes in a print scope
    /// (`enter_print_scope`). One per thread. Its entries are freed when
    /// their call ends.
    Print,
}

/// The chunk numbers of one owner, oldest first. New entries of the owner
/// fill the last chunk of each table until it is full.
#[derive(Default)]
struct OwnerChunks {
    slots: Vec<u32>,
    datas: Vec<u32>,
    /// The number of filled cells in the last chunk of `datas`.
    data_fill: u32,
    lists: Vec<u32>,
}

impl OwnerChunks {
    const EMPTY: Self = Self {
        slots: Vec::new(),
        datas: Vec::new(),
        data_fill: 0,
        lists: Vec::new(),
    };
}

/// The synthetic nodes of one thread. Each table is a list of chunks, and
/// each chunk has one owner. Entry `i` of a table with `N` entries per
/// chunk is entry `i % N` of chunk `i / N`. A freed chunk is `None`. Chunk
/// numbers only grow, so an index never names a second entry.
struct SyntheticArena {
    /// Node slots, in chunks of `SLOT_CHUNK`.
    slots: Vec<Option<Vec<Slot>>>,
    /// The owner of each chunk of `slots`.
    slot_owner: Vec<OwnerKey>,
    /// Alias slot of each parsed node, so one parsed node gets one slot.
    aliases: FxHashMap<Node, u32>,
    /// The astdata nodes of the node slots (see `SyntheticNode::data`), in
    /// chunks of `DATA_CHUNK`. A read holds the chunk of its node (an `Rc`),
    /// so it can hold the node while the arena is not borrowed, even when
    /// the owner of the chunk is freed meanwhile. A new node fills the next
    /// cell (`OnceCell::set` needs no `&mut`), so it can be added while such
    /// a read runs.
    // PERF: one allocation per chunk. An `Rc` per node made each 40-byte
    // node a 64-byte allocation plus an 8-byte pointer (goport_typesyms on
    // effect: +16% peak memory).
    datas: Vec<Option<DataChunk>>,
    /// Factory lists (`SyntheticList::Own`), in chunks of `LIST_CHUNK`. A
    /// chunk never grows past its capacity, so a list keeps its address
    /// (`synthetic_list_ptr`).
    lists: Vec<Option<Vec<OwnList>>>,
    /// Node slices that node reads made (`SyntheticList::Slice`), where Go
    /// allocates a new slice on each call. They belong to the thread.
    slices: Vec<Box<[Node]>>,
    /// The chunks of the thread (the base owner).
    base: OwnerChunks,
    /// The chunks of each program version that is an owner on this thread,
    /// by `GoProgram::id`.
    owners: FxHashMap<u32, OwnerChunks>,
    /// The chunks of each freeable file version (`OwnerKey::File`), by
    /// file id.
    alias_files: FxHashMap<u32, OwnerChunks>,
    /// The number of dead file versions when `free_dead_aliases` last ran
    /// (`ast::dead_file_versions`).
    dead_seen: usize,
    /// The last program that `current_owner` looked up, and its owner.
    last: Option<(&'static GoProgram, OwnerKey)>,
    /// The number of slots made on this thread, freed or not.
    slots_made: usize,
    /// Set on a checker thread that has a d.ts twin, and on the twin: new
    /// chunks take their numbers from it, and a table has no chunk (`None`)
    /// at the numbers of the other thread.
    shared: Option<Arc<SharedChunks>>,
    /// The chunks of `OwnerKey::Print` and the open print scopes.
    print: PrintState,
}

/// The print owner of a thread (`enter_print_scope`).
#[derive(Default)]
struct PrintState {
    chunks: OwnerChunks,
    /// Open print scopes. Only the outermost frees.
    depth: u32,
    /// The open call keeps its nodes (`pin_print_scope`).
    pinned: bool,
    /// When the outermost scope opened: the index in `chunks.slots` and the
    /// filled slots of that chunk.
    start: (usize, usize),
    /// The same for `chunks.datas` (filled cells) and `chunks.lists`.
    data_start: (usize, usize),
    list_start: (usize, usize),
}

impl PrintState {
    const EMPTY: Self = Self {
        chunks: OwnerChunks::EMPTY,
        depth: 0,
        pinned: false,
        start: (0, 0),
        data_start: (0, 0),
        list_start: (0, 0),
    };
}

impl SyntheticArena {
    /// An arena with no entry. The nil slot (slot 0) comes with the first
    /// slot (`push_slot`), but counts as made from the start
    /// (`slots_made`).
    // PERF: emitast1 F3. A `const` value, so `ARENA` is a `const` thread
    // local: each access is an inline thread pointer load, not a call to
    // the lazy initializer.
    const fn new() -> Self {
        Self {
            slots: Vec::new(),
            slot_owner: Vec::new(),
            aliases: FxHashMap::with_hasher(rustc_hash::FxBuildHasher),
            datas: Vec::new(),
            lists: Vec::new(),
            slices: Vec::new(),
            base: OwnerChunks::EMPTY,
            owners: FxHashMap::with_hasher(rustc_hash::FxBuildHasher),
            alias_files: FxHashMap::with_hasher(rustc_hash::FxBuildHasher),
            dead_seen: 0,
            last: None,
            slots_made: 1,
            shared: None,
            print: PrintState::EMPTY,
        }
    }

    /// Adds the nil slot to an arena with no slot.
    fn ensure_nil_slot(&mut self) {
        if self.slots.is_empty() {
            self.add_nil_slot();
        }
    }

    #[cold]
    #[inline(never)]
    fn add_nil_slot(&mut self) {
        let nil = self.push_slot_in(OwnerKey::Base, Slot::Nil);
        debug_assert_eq!(nil, NIL_SLOT);
    }

    /// The owner of a new node, factory list or list copy: the owner of the
    /// innermost open scope, else the current program version when it is
    /// an owner on this thread, else the thread. A thread with no program
    /// owner gives every entry to the thread.
    #[inline]
    fn current_owner(&mut self) -> OwnerKey {
        if self.owners.is_empty() {
            return OwnerKey::Base;
        }
        if let Some(owner) = SCOPE_OWNER.with(Cell::get) {
            return owner;
        }
        let Some(program) = crate::core::try_prog() else {
            return OwnerKey::Base;
        };
        if let Some((last, owner)) = self.last
            && std::ptr::eq(last, program)
        {
            return owner;
        }
        let owner = if self.owners.contains_key(&program.id) {
            OwnerKey::Program(program.id)
        } else {
            OwnerKey::Base
        };
        self.last = Some((program, owner));
        owner
    }

    /// The owner of a new node or list: the print owner for a to-string
    /// factory (`print`) in a print scope, else `current_owner`.
    #[inline]
    fn owner_for(&mut self, print: bool) -> OwnerKey {
        if print && self.print.depth > 0 {
            return OwnerKey::Print;
        }
        self.current_owner()
    }

    /// The owner of a new alias slot for parsed node `n`, or of a file
    /// scope (`enter_file_synthetic_owner`): its file version when that is
    /// a live freeable version, else the thread. It frees the entries of the
    /// versions that died since the last call first.
    fn alias_owner(&mut self, n: Node) -> OwnerKey {
        // PERF: two atomic loads in a process that frees no file version
        // (the CLI).
        if !super::file_version::any_freeable_published() || !owners_enabled() {
            return OwnerKey::Base;
        }
        if self.dead_seen != super::file_version::dead_file_versions() {
            self.free_dead_aliases();
        }
        let file = n.file_index() as u32;
        if !self.alias_files.contains_key(&file) {
            if super::file_version::file_version_probe(n).is_none() {
                return OwnerKey::Base;
            }
            self.alias_files.insert(file, OwnerChunks::default());
        }
        OwnerKey::File(file)
    }

    /// Frees the entries of the file versions that died since the last
    /// call (`OwnerKey::File`), and forgets their `aliases` entries.
    #[cold]
    #[inline(never)]
    fn free_dead_aliases(&mut self) {
        let (dead, count) = super::file_version::dead_files_since(self.dead_seen);
        self.dead_seen = count;
        if dead.is_empty() {
            return;
        }
        for &file in &dead {
            if let Some(chunks) = self.alias_files.remove(&(file as u32)) {
                for c in chunks.slots {
                    self.slots[c as usize] = None;
                }
                for c in chunks.datas {
                    self.datas[c as usize] = None;
                }
                for c in chunks.lists {
                    self.lists[c as usize] = None;
                }
            }
        }
        // PERF: emitast2. An editor edit kills one version: compare its id,
        // with no set lookup per alias.
        if let [file] = dead[..] {
            self.aliases.retain(|n, _| n.file_index() != file);
        } else {
            let dead: FxHashSet<usize> = dead.into_iter().collect();
            self.aliases.retain(|n, _| !dead.contains(&n.file_index()));
        }
    }

    /// Slot `index`. Panics when its owner was freed.
    #[inline]
    fn slot(&self, index: usize) -> &Slot {
        &self.slots[index / SLOT_CHUNK].as_ref().expect(FREED)[index % SLOT_CHUNK]
    }

    /// `slot` to write.
    fn slot_mut(&mut self, index: usize) -> &mut Slot {
        &mut self.slots[index / SLOT_CHUNK].as_mut().expect(FREED)[index % SLOT_CHUNK]
    }

    /// Adds a slot of `owner` and returns its index.
    fn push_slot(&mut self, owner: OwnerKey, slot: Slot) -> u32 {
        self.ensure_nil_slot();
        self.slots_made += 1;
        self.push_slot_in(owner, slot)
    }

    /// `push_slot` in an arena that has the nil slot (or for the nil slot).
    fn push_slot_in(&mut self, owner: OwnerKey, slot: Slot) -> u32 {
        let Self {
            slots,
            slot_owner,
            base,
            owners,
            alias_files,
            shared,
            print,
            ..
        } = self;
        let chunks = owner_chunks(base, owners, alias_files, &mut print.chunks, owner);
        push_slot_into(slots, slot_owner, shared, chunks, owner, slot)
    }

    /// Adds the data entry and the node slot of a new node of `owner`
    /// (`alloc_synthetic_node`) and returns the slot index. `slot` makes the
    /// slot from the data index.
    // PERF: emitast2. One lookup of the chunks of `owner` for both entries
    // (a hash lookup for a program owner); `push_data` and `push_slot` made
    // three.
    fn push_node(
        &mut self,
        owner: OwnerKey,
        node: crate::astdata::Node,
        slot: impl FnOnce(u32) -> Slot,
    ) -> u32 {
        self.ensure_nil_slot();
        self.slots_made += 1;
        let Self {
            slots,
            slot_owner,
            datas,
            base,
            owners,
            alias_files,
            shared,
            print,
            ..
        } = self;
        let chunks = owner_chunks(base, owners, alias_files, &mut print.chunks, owner);
        let data = push_data_into(datas, shared, chunks, node);
        push_slot_into(slots, slot_owner, shared, chunks, owner, slot(data))
    }

    /// The astdata node of data entry `index`. Panics when its owner was
    /// freed.
    #[inline]
    fn data(&self, index: u32) -> &crate::astdata::Node {
        let i = index as usize;
        self.datas[i / DATA_CHUNK].as_ref().expect(FREED)[i % DATA_CHUNK]
            .get()
            .expect("synthetic node data entry is not filled")
    }

    /// Adds a data entry of `owner` and returns its index.
    fn push_data(&mut self, owner: OwnerKey, node: crate::astdata::Node) -> u32 {
        let Self {
            datas,
            base,
            owners,
            alias_files,
            shared,
            print,
            ..
        } = self;
        let chunks = owner_chunks(base, owners, alias_files, &mut print.chunks, owner);
        push_data_into(datas, shared, chunks, node)
    }

    /// The factory list of entry `index`. Panics when its owner was freed.
    #[inline]
    fn own_list(&self, index: u32) -> &OwnList {
        let i = index as usize;
        &self.lists[i / LIST_CHUNK].as_ref().expect(FREED)[i % LIST_CHUNK]
    }

    /// Adds a factory list of `owner` and returns its index.
    fn push_list(&mut self, owner: OwnerKey, list: OwnList) -> u32 {
        let Self {
            lists,
            base,
            owners,
            alias_files,
            shared,
            print,
            ..
        } = self;
        let chunks = owner_chunks(base, owners, alias_files, &mut print.chunks, owner);
        let open = chunks
            .lists
            .last()
            .copied()
            .filter(|&c| lists[c as usize].as_ref().expect(FREED).len() < LIST_CHUNK);
        let c = match open {
            Some(c) => c,
            None => {
                let c = new_chunk_number(shared, LIST_TABLE, lists.len(), LIST_CHUNK);
                set_chunk(lists, c, Vec::with_capacity(LIST_CHUNK));
                chunks.lists.push(c);
                c
            }
        };
        let chunk = lists[c as usize].as_mut().expect(FREED);
        chunk.push(list);
        (c as usize * LIST_CHUNK + chunk.len() - 1) as u32
    }

    /// The node slot of synthetic handle `n`.
    // PERF: emitast1. Out of line, as before: inlined at every synthetic
    // field read, it made `tsgo -p` effect check (16 threads) 0.9% to
    // 1.5% slower on mini-743d with the same instruction count (runs t3,
    // t5, t6); out of line it is +0.15% there, with the same emit gain.
    #[inline(never)]
    fn node(&self, n: Node) -> &SyntheticNode {
        match self.slot(slot_index(n)) {
            Slot::Node(s) => s,
            other => not_a_node_slot(other),
        }
    }

    /// The Go node that synthetic-space id `index` stands for.
    fn resolve(&self, index: usize) -> Node {
        // The nil id, also in an arena with no slot yet.
        if index == NIL_SLOT as usize {
            return Node::NIL;
        }
        match self.slot(index) {
            Slot::Nil => Node::NIL,
            Slot::Alias(target) => *target,
            Slot::Node(_) => handle(index as u32),
            Slot::Absent => panic!("{ABSENT}"),
            Slot::Freed => panic!("{PRINT_FREED}"),
        }
    }

    /// The list that `list` names. Panics on a node slice.
    fn list(&self, list: SyntheticList) -> AnyList<'_> {
        match list {
            SyntheticList::Field { data, sel } => sel(&self.data(data).data)
                .flatten()
                .expect("synthetic list field is not in its node data"),
            SyntheticList::Own { index } => self.own_list(index).get(),
            SyntheticList::Slice { .. } => panic!("a node slice is not a NodeList"),
        }
    }
}

/// The number of a new chunk of `table`, which has `len` chunks with
/// `per_chunk` entries each: `len`, or the next number of the shared
/// counter (`SyntheticArena::shared`). Entry ids are `u32` and are not used
/// again, so a thread that makes about 4 billion entries of one kind runs
/// out of them.
fn new_chunk_number(
    shared: &Option<Arc<SharedChunks>>,
    table: usize,
    len: usize,
    per_chunk: usize,
) -> u32 {
    let c = match shared {
        None => len,
        Some(shared) => shared.next[table].fetch_add(1, Ordering::Relaxed) as usize,
    };
    assert!(
        (c + 1) * per_chunk < u32::MAX as usize,
        "synthetic entry ids exhausted"
    );
    c as u32
}

/// The chunks of `owner`, which must be open (a file owner opens with
/// `SyntheticArena::alias_owner`), from the owner fields of an arena, so the
/// caller can still borrow its tables.
fn owner_chunks<'a>(
    base: &'a mut OwnerChunks,
    owners: &'a mut FxHashMap<u32, OwnerChunks>,
    alias_files: &'a mut FxHashMap<u32, OwnerChunks>,
    print: &'a mut OwnerChunks,
    owner: OwnerKey,
) -> &'a mut OwnerChunks {
    match owner {
        OwnerKey::Base => base,
        OwnerKey::Print => print,
        OwnerKey::Program(id) => owners.get_mut(&id).expect("synthetic owner is not open"),
        OwnerKey::File(file) => alias_files
            .get_mut(&file)
            .expect("synthetic owner is not open"),
    }
}

/// `SyntheticArena::push_slot_in` on the arena tables: `chunks` are the
/// chunks of `owner`.
fn push_slot_into(
    slots: &mut Vec<Option<Vec<Slot>>>,
    slot_owner: &mut Vec<OwnerKey>,
    shared: &Option<Arc<SharedChunks>>,
    chunks: &mut OwnerChunks,
    owner: OwnerKey,
    slot: Slot,
) -> u32 {
    let open = chunks
        .slots
        .last()
        .copied()
        .filter(|&c| slots[c as usize].as_ref().expect(FREED).len() < SLOT_CHUNK);
    let c = match open {
        Some(c) => c,
        None => {
            let c = new_chunk_number(shared, SLOT_TABLE, slots.len(), SLOT_CHUNK);
            set_chunk(slots, c, Vec::with_capacity(SLOT_CHUNK));
            if slot_owner.len() <= c as usize {
                slot_owner.resize(c as usize + 1, OwnerKey::Base);
            }
            slot_owner[c as usize] = owner;
            chunks.slots.push(c);
            c
        }
    };
    let chunk = slots[c as usize].as_mut().expect(FREED);
    chunk.push(slot);
    (c as usize * SLOT_CHUNK + chunk.len() - 1) as u32
}

/// `SyntheticArena::push_data` on the arena tables: `chunks` are the chunks
/// of the owner.
fn push_data_into(
    datas: &mut Vec<Option<DataChunk>>,
    shared: &Option<Arc<SharedChunks>>,
    chunks: &mut OwnerChunks,
    node: crate::astdata::Node,
) -> u32 {
    let fill = chunks.data_fill;
    let open = chunks
        .datas
        .last()
        .copied()
        .filter(|_| (fill as usize) < DATA_CHUNK);
    let (c, cell) = match open {
        Some(c) => (c, fill),
        None => {
            let c = new_chunk_number(shared, DATA_TABLE, datas.len(), DATA_CHUNK);
            set_chunk(datas, c, empty_data_chunk());
            chunks.datas.push(c);
            (c, 0)
        }
    };
    chunks.data_fill = cell + 1;
    assert!(
        datas[c as usize].as_ref().expect(FREED)[cell as usize]
            .set(node)
            .is_ok(),
        "synthetic node data entry is filled twice"
    );
    (c as usize * DATA_CHUNK + cell as usize) as u32
}

/// The panic of `SyntheticArena::node` on a slot that is not a node slot.
#[cold]
#[inline(never)]
fn not_a_node_slot(slot: &Slot) -> ! {
    match slot {
        Slot::Absent => panic!("{ABSENT}"),
        Slot::Freed => panic!("{PRINT_FREED}"),
        _ => panic!("synthetic handle does not name a node slot"),
    }
}

/// Puts `chunk` at number `c` of a table, after `None` holes for the
/// numbers that the other thread of a shared counter took.
fn set_chunk<T>(table: &mut Vec<Option<T>>, c: u32, chunk: T) {
    let c = c as usize;
    if table.len() <= c {
        table.resize_with(c + 1, || None);
    }
    debug_assert!(table[c].is_none(), "synthetic chunk number used twice");
    table[c] = Some(chunk);
}

/// A chunk of `SyntheticArena::datas` with no filled cell.
fn empty_data_chunk() -> DataChunk {
    (0..DATA_CHUNK).map(|_| OnceCell::new()).collect()
}

static EMPTY_BIND: NodeBindData = NodeBindData {
    symbol: SymbolId::NIL,
    local_symbol: SymbolId::NIL,
    locals: SymbolTable::NIL,
    next_container: Node::NIL,
    flow_node: FlowNodeId::NIL,
    end_flow_node: FlowNodeId::NIL,
    return_flow_node: FlowNodeId::NIL,
    added_flags: NodeFlags::NONE,
};

thread_local! {
    static ARENA: RefCell<SyntheticArena> = const { RefCell::new(SyntheticArena::new()) };
    /// The owner of the innermost open scope on this thread
    /// (`enter_base_synthetic_owner`, `enter_file_synthetic_owner`).
    static SCOPE_OWNER: Cell<Option<OwnerKey>> = const { Cell::new(None) };
}

/// False when `GOPORT_SYNTHETIC_OWNERS=0`: then no program version becomes
/// an owner, and every entry belongs to its thread, as before owners (for
/// A/B runs, and as a fallback).
fn owners_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| !matches!(std::env::var("GOPORT_SYNTHETIC_OWNERS").as_deref(), Ok("0")))
}

/// Makes program version `id` an owner on this thread: the synthetic
/// entries made while it is current go to its own chunks, until
/// `free_synthetic_owner(id)` frees them. `ls_program` calls it for each
/// language server program version. Other threads and programs have no
/// owners, so all their entries belong to their thread.
pub fn open_synthetic_owner(id: u32) {
    if !owners_enabled() {
        return;
    }
    ARENA.with(|a| {
        let mut a = a.borrow_mut();
        assert!(
            a.owners.insert(id, OwnerChunks::default()).is_none(),
            "synthetic owner {id} is opened twice"
        );
        a.last = None;
    });
}

/// Frees the synthetic entries of program version `id` on this thread.
/// Their handles must not be read again: a read panics. It does nothing
/// when `id` is not an owner. A read that holds a node (`with_ast_data`)
/// keeps the data chunk of that node until the read ends. The chunks leave
/// the tables now; in the first release after a client pause their memory
/// is freed after the answer (`gostd::local::drop_after_pause`).
// Not in Go: the GC frees the nodes that nothing reaches.
// PERF (freecheck1): hono frees 30 MiB (4.1 ms) here on each edit once the
// lsMix declaration of the long ls_edit_bench plan grows the Hono class type.
pub fn free_synthetic_owner(id: u32) {
    let freed = ARENA.with(|a| {
        let mut a = a.borrow_mut();
        let chunks = a.owners.remove(&id)?;
        a.last = None;
        let slots: Vec<_> = chunks
            .slots
            .iter()
            .map(|&c| a.slots[c as usize].take())
            .collect();
        let datas: Vec<_> = chunks
            .datas
            .iter()
            .map(|&c| a.datas[c as usize].take())
            .collect();
        let lists: Vec<_> = chunks
            .lists
            .iter()
            .map(|&c| a.lists[c as usize].take())
            .collect();
        Some((slots, datas, lists))
    });
    if let Some(freed) = freed {
        crate::gostd::local::drop_after_pause(Box::new(freed));
    }
}

// ──────────────────────────────────────────────────────────────────────
// Print scopes (Go `getNodeBuilder` release: `Factory.ReleaseArenas`)
// ──────────────────────────────────────────────────────────────────────

/// Opens a print scope on this thread, for a to-string call of the checker
/// (Go `getNodeBuilder`, whose callers defer its `release`). While a scope
/// is open, the new nodes and lists of a to-string factory
/// (`NodeFactory::set_print_owned`) belong to the print owner. A nested
/// call joins the outermost scope. `exit_print_scope` closes it.
// PORT: Go `release` (`Factory.ReleaseArenas`) only drops the factory's
// arena slices, and the GC frees the nodes that nothing reaches. Here the end
// of the outermost scope frees the nodes of the call at once, unless a cache
// that a later call reads keeps one of them (`pin_print_scope`).
pub fn enter_print_scope() {
    ARENA.with(|a| {
        let mut a = a.borrow_mut();
        let SyntheticArena {
            slots,
            lists,
            print,
            ..
        } = &mut *a;
        print.depth += 1;
        if print.depth > 1 {
            return;
        }
        print.pinned = false;
        // (the index in the chunk list of the chunk that the next entry
        // goes to, the filled entries of that chunk).
        let next = |chunks: &[u32], fill: Option<usize>, per_chunk: usize| match fill {
            Some(fill) if fill < per_chunk => (chunks.len() - 1, fill),
            _ => (chunks.len(), 0),
        };
        let c = &print.chunks;
        print.start = next(
            &c.slots,
            c.slots
                .last()
                .map(|&c| slots[c as usize].as_ref().expect(FREED).len()),
            SLOT_CHUNK,
        );
        print.data_start = next(
            &c.datas,
            c.datas.last().map(|_| c.data_fill as usize),
            DATA_CHUNK,
        );
        print.list_start = next(
            &c.lists,
            c.lists
                .last()
                .map(|&c| lists[c as usize].as_ref().expect(FREED).len()),
            LIST_CHUNK,
        );
    });
}

/// Keeps the nodes of the open print call: a cache that a later call reads
/// (the node builder's serialized types under an enclosing declaration)
/// stored one of them, or the call returns one. Does nothing outside a print
/// scope.
pub fn pin_print_scope() {
    ARENA.with(|a| {
        let mut a = a.borrow_mut();
        if a.print.depth > 0 {
            a.print.pinned = true;
        }
    });
}

/// True when `n` is a node of the print owner: a node of the open print
/// call.
#[must_use]
pub fn is_print_node(n: Node) -> bool {
    is_synthetic_node(n)
        && ARENA.with(|a| {
            a.borrow().slot_owner.get(slot_index(n) / SLOT_CHUNK) == Some(&OwnerKey::Print)
        })
}

/// The handles of the nodes in the slot index ranges of an ended print call
/// (`exit_print_scope`).
pub fn print_range_nodes(ranges: &[(u32, u32)]) -> impl Iterator<Item = Node> + '_ {
    ranges.iter().flat_map(|&(lo, hi)| (lo..hi).map(handle))
}

/// Closes a print scope (`enter_print_scope`). At the end of the outermost
/// scope of a call that is not pinned, it frees the entries of the call:
/// their slots become `Slot::Freed`, their lists `OwnList::Freed` and their
/// data cells empty, so a later read panics. The print chunks that are full
/// go. It returns the slot index ranges `[lo, hi)` of the freed nodes, so
/// the caller drops their side data (emit records, idToSymbol entries). The
/// chunks of a pinned call go to the current owner and live on, as before
/// print scopes. During a panic it frees nothing (as for a pinned call).
/// Returns `None` when it frees nothing.
pub fn exit_print_scope() -> Option<Vec<(u32, u32)>> {
    let panicking = std::thread::panicking();
    let (ranges, freed) = ARENA.with(|a| {
        let mut a = a.borrow_mut();
        // 0 when the arena was replaced while the scope was open
        // (`free_synthetic_nodes`, a seed): there is nothing to close.
        if a.print.depth == 0 {
            return (None, None);
        }
        a.print.depth -= 1;
        if a.print.depth > 0 {
            return (None, None);
        }
        if std::mem::take(&mut a.print.pinned) || panicking {
            a.adopt_print_chunks();
            return (None, None);
        }
        let ranges = a.print_call_ranges();
        let SyntheticArena {
            slots,
            datas,
            lists,
            print,
            ..
        } = &mut *a;
        clear_print_call(print, slots, datas, lists, &ranges);
        let freed = take_full_print_chunks(&mut print.chunks, slots, datas, lists);
        (Some(ranges), Some(freed))
    });
    // The payloads drop after the arena borrow ends.
    drop(freed);
    ranges
}

impl SyntheticArena {
    /// The slot index ranges of the open print call: from `print.start` to
    /// the end of each print slot chunk.
    fn print_call_ranges(&self) -> Vec<(u32, u32)> {
        let (first, filled) = self.print.start;
        let mut ranges = Vec::new();
        for (i, &c) in self.print.chunks.slots.iter().enumerate().skip(first) {
            let len = self.slots[c as usize].as_ref().expect(FREED).len();
            let lo = if i == first { filled } else { 0 };
            if len > lo {
                let base = c as usize * SLOT_CHUNK;
                ranges.push(((base + lo) as u32, (base + len) as u32));
            }
        }
        ranges
    }

    /// Gives all print chunks to the current owner, before the chunk that
    /// the owner fills now, so the owner keeps filling that one. The next
    /// print call starts new chunks.
    fn adopt_print_chunks(&mut self) {
        let owner = self.current_owner();
        let chunks = std::mem::take(&mut self.print.chunks);
        for &c in &chunks.slots {
            self.slot_owner[c as usize] = owner;
        }
        let SyntheticArena {
            base,
            owners,
            alias_files,
            print,
            ..
        } = self;
        let target = owner_chunks(base, owners, alias_files, &mut print.chunks, owner);
        fn put(dst: &mut Vec<u32>, src: Vec<u32>) {
            let at = dst.len().saturating_sub(1);
            dst.splice(at..at, src);
        }
        if target.datas.is_empty() {
            target.data_fill = chunks.data_fill;
        }
        put(&mut target.slots, chunks.slots);
        put(&mut target.datas, chunks.datas);
        put(&mut target.lists, chunks.lists);
    }
}

/// Frees in place the entries of the ended print call (`ranges`: its slot
/// ranges) that are in the last print chunk of each table, the chunk that
/// the next call fills. Its slots become `Slot::Freed`, its lists
/// `OwnList::Freed`, and its data cells are emptied (their payloads are
/// freed now, while they are hot). `take_full_print_chunks` takes the other
/// chunks whole.
fn clear_print_call(
    print: &PrintState,
    slots: &mut [Option<Vec<Slot>>],
    datas: &mut [Option<DataChunk>],
    lists: &mut [Option<Vec<OwnList>>],
    ranges: &[(u32, u32)],
) {
    let c = &print.chunks;
    if let (Some(&last), Some(&(lo, hi))) = (c.slots.last(), ranges.last())
        && lo as usize / SLOT_CHUNK == last as usize
    {
        let base = last as usize * SLOT_CHUNK;
        let chunk = slots[last as usize].as_mut().expect(FREED);
        for slot in &mut chunk[lo as usize - base..hi as usize - base] {
            *slot = Slot::Freed;
        }
    }
    // The filled entries of the call in the last chunk of a table: from its
    // start when the call started in that chunk, else from 0.
    // `None` when the call put no entry in the table.
    let call_from = |(first, filled): (usize, usize), chunk_count: usize| {
        if first >= chunk_count {
            None
        } else if first + 1 == chunk_count {
            Some(filled)
        } else {
            Some(0)
        }
    };
    if let Some(&last) = c.datas.last()
        && let Some(lo) = call_from(print.data_start, c.datas.len())
        // A read that still holds the chunk (none at a call end) keeps it.
        && let Some(cells) = datas[last as usize].as_mut().and_then(Rc::get_mut)
    {
        for cell in &mut cells[lo..(c.data_fill as usize).max(lo)] {
            drop(cell.take());
        }
    }
    if let Some(&last) = c.lists.last()
        && let Some(lo) = call_from(print.list_start, c.lists.len())
    {
        let chunk = lists[last as usize].as_mut().expect(FREED);
        for list in chunk.iter_mut().skip(lo) {
            *list = OwnList::Freed;
        }
    }
}

/// Chunks that `take_full_print_chunks` took out of the tables.
type FreedChunks = (
    Vec<Option<Vec<Slot>>>,
    Vec<Option<DataChunk>>,
    Vec<Option<Vec<OwnList>>>,
);

/// Takes all print chunks but the last of each table out of the tables (the
/// holes stay `None`, so a read of their entries panics). Each of them is
/// full, and every call that put entries in it has ended.
fn take_full_print_chunks(
    chunks: &mut OwnerChunks,
    slots: &mut [Option<Vec<Slot>>],
    datas: &mut [Option<DataChunk>],
    lists: &mut [Option<Vec<OwnList>>],
) -> FreedChunks {
    fn all_but_last(v: &mut Vec<u32>) -> Vec<u32> {
        let last = v.pop();
        let full = std::mem::take(v);
        v.extend(last);
        full
    }
    (
        all_but_last(&mut chunks.slots)
            .into_iter()
            .map(|c| slots[c as usize].take())
            .collect(),
        all_but_last(&mut chunks.datas)
            .into_iter()
            .map(|c| datas[c as usize].take())
            .collect(),
        all_but_last(&mut chunks.lists)
            .into_iter()
            .map(|c| lists[c as usize].take())
            .collect(),
    )
}

/// New synthetic entries of this thread belong to the thread (the base
/// owner) while the scope lives, whatever program is current. Open it
/// around code whose nodes a cache keeps across program versions: the token
/// cache and parses.
#[must_use = "new entries go to the thread only while the scope lives"]
pub fn enter_base_synthetic_owner() -> SyntheticOwnerScope {
    enter_owner_scope(OwnerKey::Base)
}

/// New synthetic entries of this thread belong to the file version of
/// parsed node `n` while the scope lives, when that is a live freeable
/// version and this thread has program owners; else to the thread, as in a
/// base scope. After the version dies, they go with its alias slots
/// (`free_dead_aliases`). Open it around code whose nodes a cache keeps for
/// as long as `n` lives: lazy JSDoc.
#[must_use = "new entries go to the file version only while the scope lives"]
pub fn enter_file_synthetic_owner(n: Node) -> SyntheticOwnerScope {
    let owner = ARENA.with(|a| {
        let mut a = a.borrow_mut();
        if a.owners.is_empty() {
            OwnerKey::Base
        } else {
            a.alias_owner(n)
        }
    });
    enter_owner_scope(owner)
}

fn enter_owner_scope(owner: OwnerKey) -> SyntheticOwnerScope {
    SyntheticOwnerScope {
        outer: SCOPE_OWNER.with(|scope| scope.replace(Some(owner))),
        _not_send: std::marker::PhantomData,
    }
}

/// From `enter_base_synthetic_owner` or `enter_file_synthetic_owner`. It
/// is `!Send`, so it drops on the thread that made it.
pub struct SyntheticOwnerScope {
    /// The owner of the scope that was open around this one.
    outer: Option<OwnerKey>,
    _not_send: std::marker::PhantomData<*const ()>,
}

impl Drop for SyntheticOwnerScope {
    fn drop(&mut self) {
        SCOPE_OWNER.with(|scope| scope.set(self.outer));
    }
}

/// A copy of the synthetic nodes of one thread (see `synthetic_seed`). It
/// owns deep copies of the node data, so it can move to another thread.
pub struct SyntheticSeed {
    slots: Vec<Option<Vec<Slot>>>,
    aliases: FxHashMap<Node, u32>,
    /// The cells of each chunk of `SyntheticArena::datas`, up to the last
    /// filled one (`None` for an empty cell).
    datas: Vec<Option<Vec<Option<crate::astdata::Node>>>>,
    lists: Vec<Option<Vec<OwnList>>>,
    slices: Vec<Box<[Node]>>,
    slots_made: usize,
}

/// A copy of the synthetic nodes made on this thread so far whose owner is
/// not freed. Each entry keeps its index (a freed chunk stays a hole). A
/// checker worker starts from the nodes of the loading thread
/// (`install_synthetic_seed`), so the nodes that the parser and the binder
/// made keep their handles on every thread.
// PORT: Go factory nodes are shared pointers. Each checker thread owns a
// copy of the nodes made before the checkers started, and the nodes it
// makes itself.
#[must_use]
pub fn synthetic_seed() -> SyntheticSeed {
    ARENA.with(|a| {
        let a = a.borrow();
        debug_assert_eq!(a.print.depth, 0, "a synthetic seed in a print scope");
        SyntheticSeed {
            slots: a.slots.clone(),
            aliases: a.aliases.clone(),
            datas: a
                .datas
                .iter()
                .map(|chunk| {
                    chunk.as_ref().map(|chunk| {
                        // Empty cells stay in place: a print call that ended
                        // empties its cells, and a chunk that a pinned call
                        // gave to another owner can have live cells after
                        // them (`exit_print_scope`).
                        let mut cells: Vec<_> =
                            chunk.iter().map(|cell| cell.get().cloned()).collect();
                        while cells.last().is_some_and(Option::is_none) {
                            cells.pop();
                        }
                        cells
                    })
                })
                .collect(),
            lists: a.lists.clone(),
            slices: a.slices.clone(),
            slots_made: a.slots_made,
        }
    })
}

/// The number of synthetic slots made on this thread, freed or not. Work
/// that must not make synthetic nodes compares it before and after.
#[must_use]
pub fn synthetic_slot_count() -> usize {
    ARENA.with(|a| a.borrow().slots_made)
}

/// The number of synthetic slots on this thread whose owner is not freed.
#[must_use]
pub fn synthetic_live_slot_count() -> usize {
    ARENA.with(|a| a.borrow().slots.iter().flatten().map(Vec::len).sum())
}

/// Makes `seed` the synthetic nodes of this thread. Every chunk of the seed
/// belongs to the thread, and new entries start new chunks.
pub fn install_synthetic_seed(seed: SyntheticSeed) {
    let arena = SyntheticArena {
        slot_owner: vec![OwnerKey::Base; seed.slots.len()],
        slots: seed.slots,
        aliases: seed.aliases,
        datas: seed
            .datas
            .into_iter()
            .map(|chunk| {
                chunk.map(|cells| {
                    cells
                        .into_iter()
                        .map(|cell| cell.map_or_else(OnceCell::new, OnceCell::from))
                        .collect::<DataChunk>()
                })
            })
            .collect(),
        lists: seed.lists,
        slices: seed.slices,
        base: OwnerChunks::default(),
        owners: FxHashMap::default(),
        alias_files: FxHashMap::default(),
        dead_seen: 0,
        last: None,
        slots_made: seed.slots_made,
        shared: None,
        print: PrintState::EMPTY,
    };
    ARENA.with(|a| *a.borrow_mut() = arena);
}

/// Makes the chunk counters of this thread's arena shared, for a d.ts twin
/// (`install_twin_synthetic_arena`), and returns them. Each counter starts
/// at the number of chunks that the table has. A second call returns the
/// same counters.
pub fn share_synthetic_chunks() -> Arc<SharedChunks> {
    ARENA.with(|a| {
        let mut a = a.borrow_mut();
        // The nil chunk is this thread's: the twin's own chunks come after.
        a.ensure_nil_slot();
        let lens = [a.slots.len(), a.datas.len(), a.lists.len()];
        Arc::clone(a.shared.get_or_insert_with(|| {
            Arc::new(SharedChunks {
                next: lens.map(|len| AtomicU32::new(len as u32)),
            })
        }))
    })
}

/// Makes the arena of this thread the empty arena of a d.ts twin whose
/// checker thread returned `shared` (`share_synthetic_chunks`). Its own
/// entries take chunk numbers from `shared`, and the entries of its checker
/// come in print packs (`install_print_pack`). Slot 0 (Go nil) is the
/// checker's, so the twin gets it with no other slot of its chunk.
pub fn install_twin_synthetic_arena(shared: Arc<SharedChunks>) {
    let mut nil_chunk = vec![Slot::Absent; SLOT_CHUNK];
    nil_chunk[NIL_SLOT as usize] = Slot::Nil;
    let arena = SyntheticArena {
        slots: vec![Some(nil_chunk)],
        slot_owner: vec![OwnerKey::Base],
        aliases: FxHashMap::default(),
        datas: Vec::new(),
        lists: Vec::new(),
        slices: Vec::new(),
        base: OwnerChunks::default(),
        owners: FxHashMap::default(),
        alias_files: FxHashMap::default(),
        dead_seen: 0,
        last: None,
        slots_made: 0,
        shared: Some(shared),
        print: PrintState::EMPTY,
    };
    ARENA.with(|a| *a.borrow_mut() = arena);
}

/// Leaks the synthetic nodes of this thread instead of freeing them when the
/// thread ends. A checker worker of a one-program process calls it at the
/// end of the process, where freeing only costs time (like the checker, see
/// `program::create_checkers`).
pub fn forget_synthetic_nodes() {
    ARENA.with(|a| {
        debug_assert_eq!(
            a.borrow().print.depth,
            0,
            "synthetic nodes forgotten in a print scope"
        );
        std::mem::forget(std::mem::replace(
            &mut *a.borrow_mut(),
            SyntheticArena::new(),
        ));
    });
}

/// Frees the synthetic nodes of this thread. Their handles must not be read
/// again on this thread. A checker worker of a released program calls it
/// (`program::release_program`).
pub fn free_synthetic_nodes() {
    let nodes = ARENA.with(|a| {
        debug_assert_eq!(
            a.borrow().print.depth,
            0,
            "synthetic nodes freed in a print scope"
        );
        std::mem::replace(&mut *a.borrow_mut(), SyntheticArena::new())
    });
    drop(nodes);
}

/// The handle of slot `index`. Does not resolve aliases.
const fn handle(index: u32) -> Node {
    Node(((SYNTHETIC_NODE_FILE as u64) << 32) | (index as u64 + 1))
}

/// Slot index of a synthetic handle.
fn slot_index(n: Node) -> usize {
    ((n.0 & 0xffff_ffff) - 1) as usize
}

/// True when `n` is a node that the factory created.
#[must_use]
pub fn is_synthetic_node(n: Node) -> bool {
    n.is_some() && n.file_index() == SYNTHETIC_NODE_FILE
}

/// Hook for `Node::new(SYNTHETIC_NODE_FILE, id)`: the Go node that a child id
/// inside synthetic `NodeData` stands for.
#[must_use]
pub fn resolve_synthetic_id(id: crate::astdata::NodeId) -> Node {
    ARENA.with(|a| a.borrow().resolve(id.index()))
}

/// Reads the node slot of a synthetic handle.
fn with_node<R>(n: Node, f: impl FnOnce(&SyntheticNode) -> R) -> R {
    ARENA.with(|a| f(a.borrow().node(n)))
}

/// Writes the node slot of a synthetic handle. Panics on a parsed node.
fn with_node_mut<R>(n: Node, f: impl FnOnce(&mut SyntheticNode) -> R) -> R {
    assert!(
        is_synthetic_node(n),
        "cannot mutate a parsed node (kind {:?})",
        n.kind()
    );
    ARENA.with(|a| match a.borrow_mut().slot_mut(slot_index(n)) {
        Slot::Node(s) => f(s),
        _ => panic!("synthetic handle does not name a node slot"),
    })
}

// ──────────────────────────────────────────────────────────────────────
// Node data reads
// ──────────────────────────────────────────────────────────────────────

/// The astdata node of a synthetic node, held apart from the arena by its
/// chunk (see `SyntheticArena::datas`).
struct HeldNode {
    chunk: DataChunk,
    cell: usize,
}

impl std::ops::Deref for HeldNode {
    type Target = crate::astdata::Node;

    #[inline]
    fn deref(&self) -> &crate::astdata::Node {
        self.chunk[self.cell]
            .get()
            .expect("synthetic node data entry is not filled")
    }
}

/// The astdata node (kind and data) of synthetic node `n`, held apart from
/// the arena, so the reader can make and change synthetic nodes.
#[cold]
#[inline(never)]
fn synthetic_ast_node(n: Node) -> HeldNode {
    ARENA.with(|a| {
        let a = a.borrow();
        let i = a.node(n).data as usize;
        HeldNode {
            chunk: Rc::clone(a.datas[i / DATA_CHUNK].as_ref().expect(FREED)),
            cell: i % DATA_CHUNK,
        }
    })
}

/// The node data of a parsed (store) node with `'static` data, or `None`
/// for a synthetic node and for a node that a freeable parse owns (lsshells
/// M3c): read those with `with_scoped_ast_node`. Go dereferences the
/// pointer, so nil panics. A store keeps only the data of a node (astmem1
/// P1): its kind and the Go `NodeBase` fields are in the slot.
// In a one-program process almost every read after the publish is a published
// store node, so only that path is inlined into callers. The synthetic file
// index is never a store id, so checking the store tables first gives the
// same result as the order that `static_ast_node_slow` keeps (synthetic,
// store).
// PERF: step 4b. `inline(always)`: the node column read (`BlockFile`) is one
// load longer since step 4, and LLVM then left it out of line at about 60
// call sites, the binder's child walks among them.
#[inline(always)]
#[must_use]
pub fn static_ast_node(n: Node) -> Option<&'static NodeData> {
    assert!(n.is_some(), "nil node dereference");
    match frozen_store_ast_node(n) {
        Some(node) => Some(node),
        None => static_ast_node_slow(n),
    }
}

/// `static_ast_node` for a node that is not a published store node of a
/// static tier: a synthetic node (`None`), a node of a freeable file
/// version (`None`: its node shell has no node column, lsshells M3c) or an
/// unpublished (built or detached) store node (`None` when the store owns
/// it). Panics for any other node.
#[cold]
#[inline(never)]
fn static_ast_node_slow(n: Node) -> Option<&'static NodeData> {
    if n.file_index() == SYNTHETIC_NODE_FILE {
        return None;
    }
    match crate::ast::store::static_store_node(n) {
        StaticNode::Static(node) => Some(node),
        StaticNode::Scoped => None,
        StaticNode::NoStore => panic!("node {n:?} is not synthetic and has no store"),
    }
}

/// The data of a parsed node that a caller loads once for several field
/// reads (`parsed_node_data`, the `_in` accessors of `node.rs`): `Some` for
/// a node with `'static` data, `None` for a node that a freeable parse owns
/// (lsshells M3c). With `None` an `_in` accessor reads the node again, as
/// its plain accessor does (most of them read a store column).
pub type LoadedData = Option<&'static NodeData>;

/// The astdata data of parsed node `n`, for code that reads parsed nodes
/// only: the binder, which loads the data of a node once and passes it to
/// the `_in` field reads (`data_accessor!`). `None` for a node that a
/// freeable parse owns (see `LoadedData`). Panics on a synthetic node.
#[inline]
#[must_use]
pub fn parsed_node_data(n: Node) -> LoadedData {
    match static_ast_node(n) {
        Some(data) => Some(data),
        None => {
            assert!(
                n.file_index() != SYNTHETIC_NODE_FILE,
                "synthetic node where a parsed node is read"
            );
            None
        }
    }
}

/// Calls `f` with the node data of any node, parsed or synthetic. Go
/// dereferences the pointer, so nil panics. `f` cannot keep a reference
/// into the data. For a synthetic node the arena is not borrowed while `f`
/// runs, and a node that a freeable parse owns is held apart from its
/// store, so `f` can make and change nodes. Hot node reads use the
/// `with_data!` macro instead.
#[inline]
pub fn with_ast_data<R>(n: Node, f: impl FnOnce(&NodeData) -> R) -> R {
    // One call of `f`, so a small `f` is inlined (see `with_data!`).
    let scoped;
    let data = match static_ast_node(n) {
        Some(data) => data,
        None => {
            scoped = scoped_ast_node(n);
            &*scoped
        }
    };
    f(data)
}

/// The node data of a node with no `'static` data, held for its reader:
/// a synthetic node (its data chunk, `HeldNode`) or a node that a freeable
/// parse owns (`ast::HeldStoreNode`: its cell chunk, or its pinned file
/// version).
enum ScopedNode {
    Synthetic(HeldNode),
    Store(crate::ast::store::HeldStoreNode),
}

impl std::ops::Deref for ScopedNode {
    type Target = NodeData;

    #[inline]
    fn deref(&self) -> &NodeData {
        match self {
            Self::Synthetic(node) => &node.data,
            Self::Store(node) => &**node,
        }
    }
}

/// `ScopedNode` of non-nil node `n`, which has no `'static` node.
#[cold]
#[inline(never)]
fn scoped_ast_node(n: Node) -> ScopedNode {
    if n.file_index() == SYNTHETIC_NODE_FILE {
        ScopedNode::Synthetic(synthetic_ast_node(n))
    } else {
        ScopedNode::Store(crate::ast::store::held_store_node(n))
    }
}

/// `with_ast_data` for a node with no `'static` data, out of line: a
/// synthetic node or a node that a freeable parse owns (lsshells M3c). The
/// node is held, so `f` can make and change nodes.
#[cold]
#[inline(never)]
pub fn with_scoped_ast_node<R>(n: Node, f: impl FnOnce(&NodeData) -> R) -> R {
    // A synthetic id is never a hot file.
    if crate::ast::store::is_hot_store_node(n) {
        return crate::ast::store::with_hot_store_node(n, f);
    }
    with_held_ast_node(n, f)
}

/// `with_scoped_ast_node` for a node that is not of the hot file version.
// PERF: lsshells M3f. Its own function, so the hot read above saves few
// registers.
#[cold]
#[inline(never)]
fn with_held_ast_node<R>(n: Node, f: impl FnOnce(&NodeData) -> R) -> R {
    f(&scoped_ast_node(n))
}

/// `with_scoped_ast_node` for a field read (`with_data!`): `f` must not
/// make or change nodes of the store of `n`. A node that a freeable parse
/// owns is read in place (pinned, or with its store borrowed), with no
/// chunk or pin clone (`ast::with_scoped_store_node`).
#[cold]
#[inline(never)]
pub fn read_scoped_ast_node<R>(n: Node, f: impl FnOnce(&NodeData) -> R) -> R {
    if n.file_index() == SYNTHETIC_NODE_FILE {
        return f(&synthetic_ast_node(n).data);
    }
    crate::ast::store::with_scoped_store_node(n, f)
}

/// The inline part of `static_ast_node`: the node data of a published
/// store node of a static tier, or `None` for any other node (`with_data!`
/// then reads it with `read_ast_node_miss`). Go dereferences the pointer,
/// so nil panics.
#[inline]
#[must_use]
pub fn static_tier_ast_node(n: Node) -> Option<&'static NodeData> {
    assert!(n.is_some(), "nil node dereference");
    frozen_store_ast_node(n)
}

/// `read` on the node data of node `n` when it is not a node of a static
/// tier (`static_tier_ast_node`): a synthetic node, an unpublished store
/// node or a node of a freeable file version (lsshells M3c). `read` must not
/// make or change nodes of the store of `n` (a field read,
/// `read_scoped_ast_node`).
// PERF: lsshells M3c. One out-of-line call for the whole miss: a node data
// read of the edited file made three (`static_ast_node_slow`,
// `read_scoped_ast_node`, `with_scoped_store_node`) and looked the file up
// twice.
#[cold]
#[inline(never)]
pub fn read_ast_node_miss<R>(n: Node, read: impl FnOnce(&NodeData) -> R) -> R {
    // lsshells M3f: a node of the hot file version (a synthetic id never
    // is), with the other misses out of line, so this path saves few
    // registers.
    if crate::ast::store::is_hot_store_node(n) {
        return crate::ast::store::with_hot_store_node(n, read);
    }
    read_ast_node_cold(n, read)
}

/// `read_ast_node_miss` for a node that is not of the hot file version.
#[cfg(not(target_family = "wasm"))]
#[cold]
#[inline(never)]
fn read_ast_node_cold<R>(n: Node, read: impl FnOnce(&NodeData) -> R) -> R {
    if n.file_index() == SYNTHETIC_NODE_FILE {
        return read(&synthetic_ast_node(n).data);
    }
    crate::ast::store::read_store_node_miss(n, read)
}

/// wasm: `read_ast_node_miss` for a node that is not of the hot file
/// version, with the miss code once (`read_ast_node_cold_dyn`): `read` runs
/// through `&mut dyn FnMut`. One copy per reader type was 119 KB of the
/// module (101 copies).
#[cfg(target_family = "wasm")]
#[cold]
#[inline(never)]
fn read_ast_node_cold<R>(n: Node, read: impl FnOnce(&NodeData) -> R) -> R {
    let mut read = Some(read);
    let mut result = None;
    read_ast_node_cold_dyn(n, &mut |node| {
        result = Some((read.take().expect("the node is read once"))(node));
    });
    result.expect("the node is read once")
}

/// wasm: `read_ast_node_cold` with one copy for all readers.
#[cfg(target_family = "wasm")]
#[cold]
#[inline(never)]
fn read_ast_node_cold_dyn(n: Node, read: &mut dyn FnMut(&NodeData)) {
    if n.file_index() == SYNTHETIC_NODE_FILE {
        return read(&synthetic_ast_node(n).data);
    }
    crate::ast::store::read_store_node_miss(n, read)
}

/// `$body` with `$d` bound to the astdata data (`&NodeData`) of node `$n`,
/// parsed or synthetic, like `with_ast_data`. Go dereferences the pointer, so
/// nil panics. `$body` cannot keep a reference into the data, and it cannot
/// `return` from the caller. It is a field read: it must not make or change
/// nodes of the store of `$n` (`read_scoped_ast_node`).
// PERF: the macro writes `$body` twice: inline for a node of a static tier
// (the `&'static` read, with no closure call) and in a closure that runs
// out of line for any other node (`read_ast_node_miss`): a synthetic node,
// an unpublished store node, or a node that a freeable parse owns (lsshells
// M3c). That closure holds the synthetic node data apart
// from the arena, so `$body` can make synthetic nodes. A closure with two
// call sites is not inlined, which cost about 1% of instructions in
// multiprog. To write `$body` once instead, make this
// `with_ast_data($n, |$d| $body)`; no call site changes.
macro_rules! with_data {
    ($n:expr, |$d:ident| $body:expr) => {{
        let n__: $crate::core::Node = $n;
        match $crate::ast::synthetic::static_tier_ast_node(n__) {
            Some(data__) => {
                let $d: &crate::astdata::NodeData = data__;
                $body
            }
            None => $crate::ast::synthetic::read_ast_node_miss(n__, |data__| {
                let $d: &crate::astdata::NodeData = data__;
                $body
            }),
        }
    }};
}
pub(crate) use with_data;

/// The `NodeList` that the selector `|$d| $body` (a `ListSel` body) finds in
/// the data of node `$n` (nil for a Go `nil` field), or `None` when the kind
/// of `$n` has no such field. Like `with_data!`, the selector runs inline for
/// a node of a static tier; any other node (a synthetic node, an unpublished
/// store node, a node that a freeable parse owns) gets it as a `ListSel`,
/// with the id of this site (`SelectorSite`).
macro_rules! list_of {
    ($n:expr, |$d:ident| $body:expr) => {{
        let n__: $crate::core::Node = $n;
        match $crate::ast::synthetic::static_tier_ast_node(n__) {
            Some(data__) => {
                let $d: &'static crate::astdata::NodeData = data__;
                let found: Option<Option<$crate::ast::synthetic::AnyList<'static>>> = $body;
                found.map(|l| {
                    $crate::ast::NodeList::from_ts(
                        n__.file_index(),
                        l.map($crate::ast::synthetic::AnyList::nodes),
                    )
                })
            }
            None => {
                static SITE: $crate::ast::synthetic::SelectorSite =
                    $crate::ast::synthetic::SelectorSite::new();
                $crate::ast::synthetic::scoped_node_list_of(n__, &SITE, |$d| $body)
            }
        }
    }};
}
pub(crate) use list_of;

/// `list_of!` for a modifier list field.
macro_rules! modifiers_of {
    ($n:expr, |$d:ident| $body:expr) => {{
        let n__: $crate::core::Node = $n;
        match $crate::ast::synthetic::static_tier_ast_node(n__) {
            Some(data__) => {
                let $d: &'static crate::astdata::NodeData = data__;
                let found: Option<Option<$crate::ast::synthetic::AnyList<'static>>> = $body;
                found.map(|m| {
                    $crate::ast::ModifierList::from_ts(
                        n__.file_index(),
                        m.map($crate::ast::synthetic::AnyList::modifiers),
                    )
                })
            }
            None => {
                static SITE: $crate::ast::synthetic::SelectorSite =
                    $crate::ast::synthetic::SelectorSite::new();
                $crate::ast::synthetic::scoped_modifiers_of(n__, &SITE, |$d| $body)
            }
        }
    }};
}
pub(crate) use modifiers_of;

/// A list or modifier list read from node data.
#[derive(Clone, Copy)]
pub enum AnyList<'a> {
    Nodes(&'a crate::astdata::NodeList),
    Modifiers(&'a crate::astdata::ModifierList),
}

impl<'a> AnyList<'a> {
    /// The node list (for a modifier list, its `NodeList`).
    #[must_use]
    pub fn nodes(self) -> &'a crate::astdata::NodeList {
        match self {
            Self::Nodes(l) => l,
            Self::Modifiers(m) => &m.list,
        }
    }

    /// The modifier list. Panics on a node list.
    #[must_use]
    pub fn modifiers(self) -> &'a crate::astdata::ModifierList {
        match self {
            Self::Modifiers(m) => m,
            Self::Nodes(_) => panic!("a NodeList is not a ModifierList"),
        }
    }

    /// The address of the node list, for Go pointer equality.
    fn ptr(self) -> *const () {
        std::ptr::from_ref(self.nodes()).cast()
    }
}

/// Finds a list field in node data. `None`: the data has no such field (a
/// kind without it). `Some(None)`: the field is Go `nil`.
pub type ListSel = for<'a> fn(&'a NodeData) -> Option<Option<AnyList<'a>>>;

/// The most list selector sites (`SelectorSite`) in the process.
const SELECTOR_LIMIT: usize = 1024;

/// The selectors that `SelectorSite::id` gave ids, by id.
static SELECTORS: [OnceLock<ListSel>; SELECTOR_LIMIT] = [const { OnceLock::new() }; SELECTOR_LIMIT];

/// The number of ids that `SelectorSite::id` gave.
static SELECTOR_COUNT: AtomicUsize = AtomicUsize::new(0);

/// The list selector of one `list_of!` or `modifiers_of!` site, with a
/// small process-wide id (lsshells M3c): a list handle of a store that owns
/// its nodes (`ast::StoreList`) names its selector with 4 bytes, so
/// `NodeList` and `ModifierList` stay 16 bytes. Each site gets its id at its
/// first scoped read.
// PORT: Go keeps a `*NodeList` pointer. A list of a freeable file version
// cannot be borrowed past its read, so the handle names the node data cell
// and the selector, and the list is found again at each use.
pub struct SelectorSite(AtomicU32);

impl SelectorSite {
    #[must_use]
    pub const fn new() -> Self {
        Self(AtomicU32::new(0))
    }

    /// The id of `sel`, the selector of this site (`list_selector`).
    #[inline]
    pub fn id(&self, sel: ListSel) -> u32 {
        match self.0.load(Ordering::Acquire) {
            0 => self.new_id(sel),
            id => id - 1,
        }
    }

    /// The first `id` of this site.
    #[cold]
    #[inline(never)]
    fn new_id(&self, sel: ListSel) -> u32 {
        let id = SELECTOR_COUNT.fetch_add(1, Ordering::Relaxed);
        // The last id is `ast::store::PENDING_SEL`.
        assert!(
            id < SELECTOR_LIMIT - 1,
            "more than {} list selector sites",
            SELECTOR_LIMIT - 1
        );
        // A new id: this is its only write.
        let _ = SELECTORS[id].set(sel);
        let id = id as u32;
        // Two threads can give one site an id at once. The first id stays;
        // the other one is not used again.
        match self
            .0
            .compare_exchange(0, id + 1, Ordering::AcqRel, Ordering::Acquire)
        {
            Ok(_) => id,
            Err(first) => first - 1,
        }
    }
}

impl Default for SelectorSite {
    fn default() -> Self {
        Self::new()
    }
}

/// The list selector with id `id` (`SelectorSite::id`).
#[inline]
#[must_use]
pub fn list_selector(id: u32) -> ListSel {
    *SELECTORS[id as usize]
        .get()
        .expect("a list selector id has its selector")
}

/// `list_of!` for a node that is not a node of a static tier
/// (`static_tier_ast_node`): a `Field` handle for a synthetic node, a
/// `StoreList` handle for a node that a freeable parse owns (lsshells M3c),
/// and a static handle for any other store node (an unpublished store).
#[cold]
#[inline(never)]
#[must_use]
pub fn scoped_node_list_of(n: Node, site: &SelectorSite, sel: ListSel) -> Option<NodeList> {
    if n.file_index() == SYNTHETIC_NODE_FILE {
        return synthetic_node_list_of(n, sel);
    }
    let file = n.file_index();
    scoped_store_list_of(n, site.id(sel), sel).map(|found| match found {
        None => NodeList::NIL,
        Some(ScopedList::Static(l)) => NodeList::from_ts(file, Some(l.nodes())),
        Some(ScopedList::Store(l)) => NodeList::store(l),
    })
}

/// `modifiers_of!` for a node that is not a node of a static tier (see
/// `scoped_node_list_of`).
#[cold]
#[inline(never)]
#[must_use]
pub fn scoped_modifiers_of(n: Node, site: &SelectorSite, sel: ListSel) -> Option<ModifierList> {
    if n.file_index() == SYNTHETIC_NODE_FILE {
        return synthetic_modifiers_of(n, sel);
    }
    let file = n.file_index();
    scoped_store_list_of(n, site.id(sel), sel).map(|found| match found {
        None => ModifierList::NIL,
        Some(ScopedList::Static(m)) => ModifierList::from_ts(file, Some(m.modifiers())),
        Some(ScopedList::Store(m)) => ModifierList::store(m),
    })
}

/// `list_of!` for a synthetic node: a `Field` handle.
#[cold]
#[inline(never)]
#[must_use]
pub fn synthetic_node_list_of(n: Node, sel: ListSel) -> Option<NodeList> {
    synthetic_list_of(n, sel).map(|l| match l {
        Some(l) => NodeList::synthetic(l),
        None => NodeList::NIL,
    })
}

/// `modifiers_of!` for a synthetic node: a `Field` handle.
#[cold]
#[inline(never)]
#[must_use]
pub fn synthetic_modifiers_of(n: Node, sel: ListSel) -> Option<ModifierList> {
    synthetic_list_of(n, sel).map(|m| match m {
        Some(m) => ModifierList::synthetic(m),
        None => ModifierList::NIL,
    })
}

/// Go `node.Text()` (and the `RawText` of template literals) of synthetic
/// node `n`, or of a node that a freeable parse owns (lsshells M3c): the
/// text that `text` finds in its data, or "" for `None`. The text is
/// interned (`Name`), so it lives for the process, but each distinct text
/// is kept once.
// PORT: the text is in the node data, which the thread (or the file
// version) frees. `Node::text` returns `&'static str`, so the text is
// interned instead.
#[must_use]
pub fn synthetic_text(n: Node, text: impl FnOnce(&NodeData) -> Option<&str>) -> &'static str {
    read_scoped_ast_node(n, |data| match text(data) {
        Some(s) => Name::from(s).as_str(),
        None => "",
    })
}

/// Hook for `Node::flags` on a synthetic node.
#[must_use]
pub fn synthetic_flags(n: Node) -> NodeFlags {
    with_node(n, |s| s.flags)
}

/// Hook for `Node::parent` on a synthetic node.
#[must_use]
pub fn synthetic_parent(n: Node) -> Node {
    with_node(n, |s| s.parent)
}

/// Hook for `Node::loc` on a synthetic node.
#[must_use]
pub fn synthetic_loc(n: Node) -> TextRange {
    with_node(n, |s| s.loc)
}

/// Hook for `Node::kind` on a synthetic node. It reads the kind in the
/// slot, so it does not look up the data entry or clone its chunk
/// (`synthetic_ast_node`).
#[must_use]
pub fn synthetic_kind(n: Node) -> SyntaxKind {
    ARENA.with(|a| {
        let a = a.borrow();
        let s = a.node(n);
        debug_assert_eq!(s.kind, a.data(s.data).kind, "synthetic slot kind");
        s.kind
    })
}

/// The cached `Node::subtree_facts` of synthetic node `n`, if any.
#[must_use]
pub fn synthetic_subtree_facts(n: Node) -> Option<SubtreeFacts> {
    with_node(n, |s| {
        s.facts
            .0
            .intersects(SubtreeFacts::COMPUTED)
            .then(|| s.facts.0.without(SubtreeFacts::COMPUTED))
    })
}

/// Caches `facts` as the `Node::subtree_facts` of synthetic node `n`.
pub fn set_synthetic_subtree_facts(n: Node, facts: SubtreeFacts) {
    with_node_mut(n, |s| s.facts = SlotFacts(facts | SubtreeFacts::COMPUTED));
}

/// Hook for `Node::bind` on a synthetic node. The data belongs to the
/// thread, so it is read by value.
#[must_use]
pub fn synthetic_bind(n: Node) -> NodeBindData {
    with_node(n, |s| s.bind.as_deref().copied().unwrap_or(EMPTY_BIND))
}

/// Go `node.AsSyntheticExpression().Type.(*Type)`.
#[must_use]
pub fn synthetic_expression_type(n: Node) -> TypeId {
    debug_assert!(n.kind() == SyntaxKind::SyntheticExpression);
    with_node(n, |s| s.synthetic_type)
}

// ──────────────────────────────────────────────────────────────────────
// Lists of synthetic data
// ──────────────────────────────────────────────────────────────────────

/// A list of synthetic data: the `NodeList`, `ModifierList` and `NodeSlice`
/// form of a list that this thread's arena owns. It is an index, so it is
/// only valid on the thread that made it (like a synthetic `Node`).
#[derive(Clone, Copy)]
pub enum SyntheticList {
    /// The list field that `sel` finds in node data `data`
    /// (`SyntheticNode::data`).
    Field { data: u32, sel: ListSel },
    /// A factory list.
    Own { index: u32 },
    /// A node slice that a read made (`NodeSlice` only).
    Slice { index: u32 },
}

impl std::fmt::Debug for SyntheticList {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Field { data, .. } => write!(f, "Field({data})"),
            Self::Own { index } => write!(f, "Own({index})"),
            Self::Slice { index } => write!(f, "Slice({index})"),
        }
    }
}

/// The list field that `sel` finds in the data of synthetic node `n`. `None`
/// when the data has no such field; `Some(None)` when it is Go `nil`.
#[must_use]
pub fn synthetic_list_of(n: Node, sel: ListSel) -> Option<Option<SyntheticList>> {
    ARENA.with(|a| {
        let a = a.borrow();
        let data = a.node(n).data;
        sel(&a.data(data).data).map(|l| l.map(|_| SyntheticList::Field { data, sel }))
    })
}

/// Calls `f` with the list that `list` names. Panics on a node slice. The
/// arena stays borrowed while `f` runs, so `f` must not make or change
/// synthetic nodes.
pub fn with_synthetic_list<R>(list: SyntheticList, f: impl FnOnce(AnyList<'_>) -> R) -> R {
    ARENA.with(|a| f(a.borrow().list(list)))
}

/// The number of nodes in `list`.
#[must_use]
pub fn synthetic_list_len(list: SyntheticList) -> usize {
    ARENA.with(|a| {
        let a = a.borrow();
        match list {
            SyntheticList::Slice { index } => a.slices[index as usize].len(),
            _ => a.list(list).nodes().nodes.len(),
        }
    })
}

/// Node `i` of `list`. Panics when `i` is out of range, like Go.
#[must_use]
pub fn synthetic_list_node(list: SyntheticList, i: usize) -> Node {
    ARENA.with(|a| {
        let a = a.borrow();
        match list {
            SyntheticList::Slice { index } => a.slices[index as usize][i],
            _ => a.resolve(a.list(list).nodes().nodes[i].index()),
        }
    })
}

/// Go pointer equality of two lists.
#[must_use]
pub fn synthetic_list_eq(x: SyntheticList, y: SyntheticList) -> bool {
    match (x, y) {
        (SyntheticList::Own { index: i }, SyntheticList::Own { index: j })
        | (SyntheticList::Slice { index: i }, SyntheticList::Slice { index: j }) => i == j,
        (SyntheticList::Field { data: d, .. }, SyntheticList::Field { data: e, .. }) => {
            d == e
                && ARENA.with(|a| {
                    let a = a.borrow();
                    a.list(x).ptr() == a.list(y).ptr()
                })
        }
        _ => false,
    }
}

/// The address of the node list that `list` names, for Go pointer keys
/// (`NodeList::list_ptr`). It stays the same while the owner of the list
/// lives: node data and factory lists are in chunks that do not move.
#[must_use]
pub fn synthetic_list_ptr(list: SyntheticList) -> *const () {
    ARENA.with(|a| a.borrow().list(list).ptr())
}

/// A new node slice of this thread (Go allocates one on each call, for
/// example in `Node.Decorators()`). It belongs to the thread: `node.rs`
/// keeps one per node.
#[must_use]
pub fn new_synthetic_slice(nodes: Vec<Node>) -> SyntheticList {
    ARENA.with(|a| {
        let mut a = a.borrow_mut();
        a.slices.push(nodes.into_boxed_slice());
        SyntheticList::Slice {
            index: (a.slices.len() - 1) as u32,
        }
    })
}

/// A factory list with a copy of `list`, a list of synthetic node data that
/// a read has in place (the `Node::for_each_child_and_lists` hook). The
/// thread's arena owns the copy.
#[must_use]
pub fn copy_synthetic_list(list: &crate::astdata::NodeList) -> NodeList {
    NodeList::synthetic(push_own_list(OwnList::Nodes(list.clone())))
}

/// Parser `createMissingList` for synthetic `list` (see
/// `NodeList::with_missing_marker`): a new empty factory list at the `Loc`
/// of `list`, with the astdata `has_trailing_comma` bit set.
#[must_use]
pub fn synthetic_missing_list(list: SyntheticList) -> SyntheticList {
    let range = with_synthetic_list(list, |l| l.nodes().range);
    push_own_list(OwnList::Nodes(crate::astdata::NodeList {
        range,
        nodes: Vec::new(),
        has_trailing_comma: true,
    }))
}

fn push_own_list(list: OwnList) -> SyntheticList {
    push_own_list_in(false, list)
}

fn push_own_list_in(print: bool, list: OwnList) -> SyntheticList {
    ARENA.with(|a| {
        let mut a = a.borrow_mut();
        let owner = a.owner_for(print);
        SyntheticList::Own {
            index: a.push_list(owner, list),
        }
    })
}

// ──────────────────────────────────────────────────────────────────────
// Go field writes on factory nodes
// ──────────────────────────────────────────────────────────────────────

/// Go `node.Parent = parent`. Works on factory nodes and on nodes of a
/// ported-parser file that is not finished.
pub fn set_node_parent(n: Node, parent: Node) {
    if is_store_node(n) {
        return set_store_node_parent(n, parent);
    }
    with_node_mut(n, |s| s.parent = parent);
}

/// Go `node.Loc = loc`.
pub fn set_node_loc(n: Node, loc: TextRange) {
    if is_store_node(n) {
        return set_store_node_loc(n, loc);
    }
    with_node_mut(n, |s| s.loc = loc);
}

/// Go `node.Flags = flags`.
pub fn set_node_flags(n: Node, flags: NodeFlags) {
    if is_store_node(n) {
        return set_store_node_flags(n, flags);
    }
    with_node_mut(n, |s| s.flags = flags);
}

/// Go `node.Flags |= flags`.
// PERF: emitast2. One arena access for a synthetic node (the factory
// `onCreate` hook), not a flag read and a flag write.
pub fn add_node_flags(n: Node, flags: NodeFlags) {
    if is_store_node(n) {
        return set_store_node_flags(n, n.flags() | flags);
    }
    with_node_mut(n, |s| s.flags |= flags);
}

/// Gives synthetic node `n` a new astdata node that `f` makes from the
/// current one. The old one stays in the arena for the list handles taken
/// before (see `SyntheticNode::data`). The new one belongs to the owner of
/// `n`. `f` must not make or change synthetic nodes.
fn replace_synthetic_ast_node(
    n: Node,
    f: impl FnOnce(&crate::astdata::Node) -> crate::astdata::Node,
) {
    assert!(
        is_synthetic_node(n),
        "cannot mutate a parsed node (kind {:?})",
        n.kind()
    );
    ARENA.with(|a| {
        let mut a = a.borrow_mut();
        let index = slot_index(n);
        let node = f(a.data(a.node(n).data));
        let kind = node.kind;
        let owner = a.slot_owner[index / SLOT_CHUNK];
        let data = a.push_data(owner, node);
        let Slot::Node(s) = a.slot_mut(index) else {
            panic!("synthetic handle does not name a node slot");
        };
        s.data = data;
        s.kind = kind;
    });
}

/// A Go write to a data field of a node (`node.AsX().Field = v`): `data` is
/// the node's data with that field changed. Works on factory nodes and on
/// nodes of a ported-parser file that is not finished.
// PORT: astdata data is shared, so the node gets a new astdata node with the
// same kind. Writes are rare.
pub fn replace_node_data(n: Node, data: NodeData) {
    if is_store_node(n) {
        return replace_store_node_data(n, data);
    }
    replace_synthetic_ast_node(n, |old| {
        let kind = old.kind;
        debug_assert!(
            data.matches_syntax_kind(kind),
            "{kind:?} does not fit its NodeData"
        );
        new_ts_node(kind, data)
    });
}

/// Go `node.Kind = kind` on a factory node. The new kind must fit the node
/// data (Go only does this between kinds with one data struct, such as
/// `KindJSImportDeclaration` to `KindImportDeclaration`).
// PORT: the kind lives in the astdata node, so the node gets a new astdata
// node with the same data.
pub fn set_node_kind(n: Node, kind: SyntaxKind) {
    replace_synthetic_ast_node(n, |old| {
        let data = old.data.clone();
        assert!(
            data.matches_syntax_kind(kind),
            "{kind:?} does not fit the data of {:?}",
            old.kind
        );
        new_ts_node(kind, data)
    });
}

/// A astdata node for synthetic data. Only kind and data are read; the
/// header lives in the slot.
fn new_ts_node(kind: SyntaxKind, data: NodeData) -> crate::astdata::Node {
    crate::astdata::Node {
        kind,
        flags: crate::astdata::NodeFlags(0),
        range: undefined_ts_range(),
        parent: None,
        data,
    }
}

/// Go `node.AsMutable().SetModifiers(modifiers)`.
// Go: ast/ast.go:227 (n *MutableNode) SetModifiers
// PORT: Go dispatches to `setModifiers` on the data. The kinds with a
// `modifiers` field set it (ModifiersBase, NamedMemberBase,
// BinaryExpression); other kinds do nothing, like Go `NodeDefault`.
pub fn set_node_modifiers(n: Node, modifiers: ModifierList) {
    let mods = synthetic_modifiers_value(modifiers);
    let mut data = with_ast_data(n, Clone::clone);
    match &mut data {
        NodeData::ArrowFunction(d) => d.modifiers = mods,
        NodeData::BinaryExpression(d) => d.modifiers = mods,
        NodeData::ClassDeclaration(d) => d.modifiers = mods,
        NodeData::ClassExpression(d) => d.modifiers = mods,
        NodeData::ClassStaticBlockDeclaration(d) => d.modifiers = mods,
        NodeData::ConstructorDeclaration(d) => d.modifiers = mods,
        NodeData::ConstructorTypeNode(d) => d.modifiers = mods,
        NodeData::EnumDeclaration(d) => d.modifiers = mods,
        NodeData::EnumMember(d) => d.modifiers = mods,
        NodeData::ExportAssignment(d) => d.modifiers = mods,
        NodeData::ExportDeclaration(d) => d.modifiers = mods,
        NodeData::FunctionDeclaration(d) => d.modifiers = mods,
        NodeData::FunctionExpression(d) => d.modifiers = mods,
        NodeData::FunctionTypeNode(d) => d.modifiers = mods,
        NodeData::GetAccessorDeclaration(d) => d.modifiers = mods,
        NodeData::ImportDeclaration(d) => d.modifiers = mods,
        NodeData::ImportEqualsDeclaration(d) => d.modifiers = mods,
        NodeData::IndexSignatureDeclaration(d) => d.modifiers = mods,
        NodeData::InterfaceDeclaration(d) => d.modifiers = mods,
        NodeData::MethodDeclaration(d) => d.modifiers = mods,
        NodeData::MethodSignatureDeclaration(d) => d.modifiers = mods,
        NodeData::MissingDeclaration(d) => d.modifiers = mods,
        NodeData::ModuleDeclaration(d) => d.modifiers = mods,
        NodeData::NamespaceExportDeclaration(d) => d.modifiers = mods,
        NodeData::ParameterDeclaration(d) => d.modifiers = mods,
        NodeData::PropertyAssignment(d) => d.modifiers = mods,
        NodeData::PropertyDeclaration(d) => d.modifiers = mods,
        NodeData::PropertySignatureDeclaration(d) => d.modifiers = mods,
        NodeData::SetAccessorDeclaration(d) => d.modifiers = mods,
        NodeData::ShorthandPropertyAssignment(d) => d.modifiers = mods,
        NodeData::TypeAliasDeclaration(d) => d.modifiers = mods,
        NodeData::TypeParameterDeclaration(d) => d.modifiers = mods,
        NodeData::VariableStatement(d) => d.modifiers = mods,
        _ => return,
    }
    replace_node_data(n, data);
}

/// Changes the binder data of a factory node: Go
/// `node.FlowNodeData().FlowNode = f`, `node.AsX().Symbol = s`, ...
pub fn update_node_bind(n: Node, f: impl FnOnce(&mut NodeBindData)) {
    with_node_mut(n, |s| f(s.bind.get_or_insert_with(|| Box::new(EMPTY_BIND))));
}

/// Go `node.FlowNodeData().FlowNode = flow`.
pub fn set_node_flow_node(n: Node, flow: FlowNodeId) {
    update_node_bind(n, |b| b.flow_node = flow);
}

/// Go `node.AsX().Symbol = symbol` (declaration data).
pub fn set_node_symbol(n: Node, symbol: SymbolId) {
    update_node_bind(n, |b| b.symbol = symbol);
}

/// Go `node.AsX().LocalSymbol = symbol`.
pub fn set_node_local_symbol(n: Node, symbol: SymbolId) {
    update_node_bind(n, |b| b.local_symbol = symbol);
}

/// Go `node.AsX().Locals = locals`.
pub fn set_node_locals(n: Node, locals: SymbolTable) {
    update_node_bind(n, |b| b.locals = locals);
}

// ──────────────────────────────────────────────────────────────────────
// Allocation (used by factory.rs)
// ──────────────────────────────────────────────────────────────────────

/// Go `newNode(kind, data, hooks)`: a new factory node with
/// `Loc = UndefinedTextRange()`, nil parent, no flags and no binder data. It
/// belongs to the current owner (see "Owners" in the module comment).
pub fn alloc_synthetic_node(kind: SyntaxKind, data: NodeData) -> Node {
    alloc_synthetic_node_in(false, kind, data)
}

/// `alloc_synthetic_node` for a to-string factory
/// (`NodeFactory::set_print_owned`): in a print scope the node belongs to
/// the print owner.
pub fn alloc_synthetic_print_node(kind: SyntaxKind, data: NodeData) -> Node {
    alloc_synthetic_node_in(true, kind, data)
}

fn alloc_synthetic_node_in(print: bool, kind: SyntaxKind, data: NodeData) -> Node {
    debug_assert!(
        data.matches_syntax_kind(kind),
        "{kind:?} does not fit its NodeData"
    );
    let node = new_ts_node(kind, data);
    ARENA.with(|a| {
        let mut a = a.borrow_mut();
        let owner = a.owner_for(print);
        handle(a.push_node(owner, node, |data| {
            Slot::Node(SyntheticNode {
                data,
                kind,
                facts: SlotFacts::default(),
                parent: Node::NIL,
                flags: NodeFlags::NONE,
                loc: TextRange::undefined(),
                bind: None,
                synthetic_type: TypeId::NIL,
                source_file: None,
            })
        }))
    })
}

/// Sets the Go `SyntheticExpression.Type` of a new node.
pub(crate) fn set_synthetic_expression_type(n: Node, t: TypeId) {
    with_node_mut(n, |s| s.synthetic_type = t);
}

// ──────────────────────────────────────────────────────────────────────
// Factory SourceFile fields
// ──────────────────────────────────────────────────────────────────────

/// Attaches the Go `ast.SourceFile` fields to a new factory SourceFile.
pub(crate) fn set_synthetic_source_file_data(n: Node, data: SyntheticSourceFileData) {
    debug_assert!(n.kind() == SyntaxKind::SourceFile);
    with_node_mut(n, |s| s.source_file = Some(Box::new(data)));
}

/// True when `n` is a SourceFile that the factory made.
#[must_use]
pub fn is_synthetic_source_file(n: Node) -> bool {
    is_synthetic_node(n) && with_node(n, |s| s.source_file.is_some())
}

/// Reads the Go `ast.SourceFile` fields of a factory SourceFile.
pub fn with_synthetic_source_file<R>(n: Node, f: impl FnOnce(&SyntheticSourceFileData) -> R) -> R {
    with_node(n, |s| {
        f(s.source_file
            .as_deref()
            .expect("node is not a factory SourceFile"))
    })
}

/// Go writes to the fields of a factory SourceFile
/// (`file.AsSourceFile().IsDeclarationFile = true`, ...). Panics on a
/// parsed SourceFile (see the module comment).
pub fn update_synthetic_source_file<R>(
    n: Node,
    f: impl FnOnce(&mut SyntheticSourceFileData) -> R,
) -> R {
    with_node_mut(n, |s| {
        f(s.source_file
            .as_deref_mut()
            .expect("node is not a factory SourceFile"))
    })
}

/// Go `file.Text()` of a factory SourceFile.
#[must_use]
pub fn synthetic_source_file_text(n: Node) -> FileText {
    with_synthetic_source_file(n, |d| d.text.clone())
}

/// Go `file.FileName()` of a factory SourceFile.
#[must_use]
pub fn synthetic_source_file_file_name(n: Node) -> &'static str {
    with_synthetic_source_file(n, |d| d.file_name)
}

/// Go `file.AsSourceFile().ReferencedFiles`, `TypeReferenceDirectives` and
/// `LibReferenceDirectives`, `IsDeclarationFile` of any SourceFile, parsed
/// or factory-made.
// PORT: `SourceFileInfo` is `&'static` for parsed files only, so readers that
// must also see a factory SourceFile use this copy.
#[must_use]
pub fn source_file_parser_fields(file: Node) -> SyntheticSourceFileData {
    if is_synthetic_node(file) {
        return with_synthetic_source_file(file, Clone::clone);
    }
    let info = source_file_info(file);
    SyntheticSourceFileData {
        file_name: source_file_file_name(file),
        path: info.path.clone(),
        text: source_file_text(file),
        language_variant: info.language_variant,
        script_kind: info.script_kind,
        is_declaration_file: info.is_declaration_file,
        uses_uri_style_node_core_modules: info.uses_uri_style_node_core_modules,
        imports: info.imports.clone(),
        module_augmentations: info.module_augmentations.clone(),
        ambient_module_names: info.ambient_module_names.clone(),
        comment_directives: info.comment_directives.clone(),
        pragmas: info.pragmas.clone(),
        referenced_files: info.referenced_files.clone(),
        type_reference_directives: info.type_reference_directives.clone(),
        lib_reference_directives: info.lib_reference_directives.clone(),
        common_js_module_indicator: info.common_js_module_indicator,
        external_module_indicator: info.external_module_indicator,
        content_mapper_info: source_file_content_mapper_info(file),
    }
}

// Go: ast/ast.go:2805 (node *SourceFile) copyFrom
/// Copies the parser fields of `other` (parsed or factory-made) to the
/// factory SourceFile `node`.
pub fn source_file_copy_from(node: Node, other: Node) {
    // Do not copy fields set by NewSourceFile (Text, FileName, Path, or Statements)
    let o = source_file_parser_fields(other);
    update_synthetic_source_file(node, |d| {
        // tsgo#4712
        if let Some(info) = o.content_mapper_info {
            // Go: node.SetContentMapperInfo(*other.contentMapperInfo)
            if d.content_mapper_info.is_some() {
                panic!("content mapper source file info already set");
            }
            d.content_mapper_info = Some(info);
        }
        d.language_variant = o.language_variant;
        d.script_kind = o.script_kind;
        d.is_declaration_file = o.is_declaration_file;
        d.uses_uri_style_node_core_modules = o.uses_uri_style_node_core_modules;
        d.imports = o.imports;
        d.module_augmentations = o.module_augmentations;
        d.ambient_module_names = o.ambient_module_names;
        d.comment_directives = o.comment_directives;
        d.pragmas = o.pragmas;
        d.referenced_files = o.referenced_files;
        d.type_reference_directives = o.type_reference_directives;
        d.lib_reference_directives = o.lib_reference_directives;
        d.common_js_module_indicator = o.common_js_module_indicator;
        d.external_module_indicator = o.external_module_indicator;
    });
    set_node_flags(node, node.flags() | other.flags());
}

/// The synthetic-space id that stands for `n` inside synthetic `NodeData`.
/// Nil maps to the nil slot. A parsed node gets (or reuses) an alias slot,
/// which belongs to its file version or to the thread (`alias_owner`).
#[must_use]
pub fn synthetic_child_id(n: Node) -> crate::astdata::NodeId {
    if n.is_nil() {
        return crate::astdata::NodeId::new(NIL_SLOT);
    }
    if n.file_index() == SYNTHETIC_NODE_FILE {
        return crate::astdata::NodeId::new(slot_index(n) as u32);
    }
    ARENA.with(|a| {
        let mut a = a.borrow_mut();
        if let Some(&index) = a.aliases.get(&n) {
            return crate::astdata::NodeId::new(index);
        }
        let owner = a.alias_owner(n);
        let index = a.push_slot(owner, Slot::Alias(n));
        a.aliases.insert(n, index);
        crate::astdata::NodeId::new(index)
    })
}

/// Like `synthetic_child_id`, for astdata fields that are `Option<NodeId>`.
#[must_use]
pub fn synthetic_opt_child_id(n: Node) -> Option<crate::astdata::NodeId> {
    if n.is_nil() {
        None
    } else {
        Some(synthetic_child_id(n))
    }
}

/// Go `core.UndefinedTextRange()` in astdata form. `TextPos` is `u32`; the
/// `as i32` in `node.rs` `text_range_of` turns `u32::MAX` back into `-1`.
fn undefined_ts_range() -> crate::astdata::text::TextRange {
    ts_range(TextRange::undefined())
}

/// A Go `core.TextRange` in astdata form (`-1` is stored as `u32::MAX`).
fn ts_range(loc: TextRange) -> crate::astdata::text::TextRange {
    crate::astdata::text::TextRange {
        start: crate::astdata::text::TextPos::new(loc.pos() as u32),
        end: crate::astdata::text::TextPos::new(loc.end() as u32),
    }
}

/// The astdata list for a synthetic node's list field.
fn ts_list(nodes: &[Node], loc: TextRange, has_trailing_comma: bool) -> crate::astdata::NodeList {
    crate::astdata::NodeList {
        range: ts_range(loc),
        nodes: nodes.iter().map(|&n| synthetic_child_id(n)).collect(),
        has_trailing_comma,
    }
}

/// Go `f.NewNodeList(nodes)` with a given `Loc`.
// PORT: Go list `Loc` is mutable; here a synthetic list fixes its `Loc` at
// creation. Callers that set `list.Loc` later pass it here instead.
#[must_use]
pub fn new_synthetic_node_list(nodes: &[Node], loc: TextRange) -> NodeList {
    new_synthetic_node_list_in(false, nodes, loc)
}

/// `new_synthetic_node_list`; with `print` (a to-string factory), in a
/// print scope the list belongs to the print owner
/// (`alloc_synthetic_print_node`).
#[must_use]
pub fn new_synthetic_node_list_in(print: bool, nodes: &[Node], loc: TextRange) -> NodeList {
    let list = ts_list(nodes, loc, false);
    NodeList::synthetic(push_own_list_in(print, OwnList::Nodes(list)))
}

/// Go `f.NewModifierList(nodes)` with a given `Loc`. `modifier_flags()` in
/// node.rs recomputes `ModifiersToFlags(nodes)`, as the Go factory does.
#[must_use]
pub fn new_synthetic_modifier_list(nodes: &[Node], loc: TextRange) -> ModifierList {
    new_synthetic_modifier_list_in(false, nodes, loc)
}

/// `new_synthetic_modifier_list` (`print`: see `new_synthetic_node_list_in`).
#[must_use]
pub fn new_synthetic_modifier_list_in(print: bool, nodes: &[Node], loc: TextRange) -> ModifierList {
    let list = crate::astdata::ModifierList {
        list: ts_list(nodes, loc, false),
        flags: crate::astdata::ModifierFlags(modifiers_to_flags(nodes).0 as u32),
    };
    ModifierList::synthetic(push_own_list_in(print, OwnList::Modifiers(list)))
}

/// A list value to store inside new synthetic `NodeData`. Go stores the
/// `*NodeList` pointer; a list that already is synthetic is copied as is, and
/// a parsed list is rebuilt over alias ids with its own `Loc`.
// PORT: astdata stores lists by value, so a synthetic node that takes a
// parsed (or another synthetic) list gets a copy. `NodeList` equality on the
// copy is false where Go compares equal pointers.
#[must_use]
pub fn synthetic_list_value(list: NodeList) -> Option<crate::astdata::NodeList> {
    if list.is_nil() {
        return None;
    }
    if let Some(l) = list.synthetic_list() {
        return Some(with_synthetic_list(l, |l| l.nodes().clone()));
    }
    let nodes = list.nodes().to_vec();
    Some(ts_list(&nodes, list.loc(), list.stored_trailing_comma()))
}

/// Like `synthetic_list_value` for a list field that astdata requires. Go
/// `nil` becomes an empty list with an undefined `Loc`.
// PORT: Go keeps `nil`; `NodeList::is_nil` on that field is false here.
#[must_use]
pub fn synthetic_req_list_value(list: NodeList) -> crate::astdata::NodeList {
    synthetic_list_value(list).unwrap_or_else(|| ts_list(&[], TextRange::undefined(), false))
}

/// A modifier list value to store inside new synthetic `NodeData`.
#[must_use]
pub fn synthetic_modifiers_value(modifiers: ModifierList) -> Option<crate::astdata::ModifierList> {
    if modifiers.is_nil() {
        return None;
    }
    if let Some(m) = modifiers.synthetic_list() {
        return Some(with_synthetic_list(m, |m| m.modifiers().clone()));
    }
    let nodes = modifiers.nodes().to_vec();
    Some(crate::astdata::ModifierList {
        list: ts_list(
            &nodes,
            modifiers.loc(),
            modifiers.node_list().stored_trailing_comma(),
        ),
        flags: crate::astdata::ModifierFlags(modifiers_to_flags(&nodes).0 as u32),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // The test runs on its own thread, so `free_synthetic_nodes` frees only
    // the nodes it made.
    #[test]
    fn synthetic_lists_keep_go_pointer_identity_until_freed() {
        std::thread::spawn(|| {
            let f = NodeFactory::new();
            let a = f.new_identifier("a");
            let statements = f.new_node_list(&[a]);
            let block = f.new_block(statements, false);

            // Reads of one list field give one list. The node data holds a
            // copy of the factory list (`synthetic_list_value`).
            let read = block.statement_list();
            assert_eq!(read, block.statement_list());
            assert_eq!(read.list_ptr(), block.statement_list().list_ptr());
            assert_ne!(read, statements);
            assert_eq!(read.nodes().to_vec(), vec![a]);
            assert_eq!(a.text(), "a");

            // A data write gives the field a new list. The old handle still
            // reads the old list, like a Go pointer.
            replace_node_data(block, with_ast_data(block, Clone::clone));
            assert_ne!(block.statement_list(), read);
            assert_eq!(read.nodes().get(0), a);

            assert!(synthetic_slot_count() > 1);
            free_synthetic_nodes();
            assert_eq!(synthetic_slot_count(), 1);
        })
        .join()
        .expect("test thread panicked");
    }

    // Node data and factory lists fill chunks. A new chunk does not move an
    // earlier entry, and a read that holds a node can make more nodes.
    #[test]
    fn synthetic_entries_stay_in_place_across_chunks() {
        std::thread::spawn(|| {
            let f = NodeFactory::new();
            let first = f.new_identifier("first");
            let list = f.new_node_list(&[first]);
            let ptr = list.list_ptr();
            let last = with_ast_data(first, |_| {
                let mut last = NodeList::NIL;
                for i in 0..2 * DATA_CHUNK.max(LIST_CHUNK) {
                    last = f.new_node_list(&[f.new_identifier(format!("n{i}"))]);
                }
                last
            });
            assert_eq!(list.list_ptr(), ptr);
            assert_eq!(list.nodes().to_vec(), vec![first]);
            assert_eq!(first.text(), "first");
            let last_text = format!("n{}", 2 * DATA_CHUNK.max(LIST_CHUNK) - 1);
            assert_eq!(last.nodes().get(0).text(), last_text);

            // The seed copy keeps every index.
            let seed = synthetic_seed();
            std::thread::spawn(move || {
                install_synthetic_seed(seed);
                assert_eq!(list.nodes().to_vec(), vec![first]);
                assert_eq!(first.text(), "first");
                assert_eq!(last.nodes().get(0).text(), last_text);
            })
            .join()
            .expect("seed thread panicked");
            free_synthetic_nodes();
        })
        .join()
        .expect("test thread panicked");
    }

    /// The message of a caught panic.
    fn panic_message(payload: &(dyn std::any::Any + Send)) -> Option<&str> {
        payload
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| payload.downcast_ref::<&str>().copied())
    }

    // A program version that is an owner owns the entries made while it is
    // current, and keeps their identity while it lives. Its free drops them
    // and keeps the base entries. A freed handle panics, and a new node gets
    // a new index.
    #[test]
    fn program_owner_frees_its_entries() {
        std::thread::spawn(|| {
            let program: &'static GoProgram = Box::leak(Box::new(GoProgram {
                id: next_program_id(),
                options: Box::leak(Box::default()),
                state: std::sync::OnceLock::new(),
            }));
            let f = NodeFactory::new();
            let base = f.new_identifier("base");
            let base_slots = synthetic_live_slot_count();

            open_synthetic_owner(program.id);
            let (statement, list, block) = {
                let _program = enter_program(Some(program));
                let statement = f.new_expression_statement(base);
                let list = f.new_node_list(&[statement]);
                let block = f.new_block(list, false);
                assert_eq!(block.statement_list(), block.statement_list());
                assert_eq!(block.statement_list().nodes().to_vec(), vec![statement]);
                assert_eq!(list.nodes().to_vec(), vec![statement]);
                // A data write goes to the owner of the node: the base.
                replace_node_data(base, with_ast_data(base, Clone::clone));
                assert!(synthetic_live_slot_count() > base_slots);
                (statement, list, block)
            };
            free_synthetic_owner(program.id);

            assert_eq!(synthetic_live_slot_count(), base_slots);
            assert_eq!(base.text(), "base");
            let freed = std::panic::catch_unwind(|| block.kind())
                .expect_err("a node of a freed owner is read");
            assert_eq!(panic_message(&*freed), Some(FREED));
            let freed =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| list.nodes().to_vec()))
                    .expect_err("a list of a freed owner is read");
            assert_eq!(panic_message(&*freed), Some(FREED));
            let fresh = f.new_identifier("fresh");
            assert!(fresh != statement && fresh != block);
            assert_eq!(fresh.text(), "fresh");

            // A seed keeps the base indexes and the holes.
            let seed = synthetic_seed();
            std::thread::spawn(move || {
                install_synthetic_seed(seed);
                assert_eq!(base.text(), "base");
                assert_eq!(fresh.text(), "fresh");
                assert!(std::panic::catch_unwind(|| block.kind()).is_err());
            })
            .join()
            .expect("seed thread panicked");
            free_synthetic_nodes();
        })
        .join()
        .expect("test thread panicked");
    }

    // After a client pause the free of an owner takes its entries out of the
    // tables at once (a read panics), but its chunks wait for
    // `drop_garbage` (`gostd::local::drop_after_pause`).
    #[test]
    fn owner_chunks_wait_for_drop_garbage_after_a_client_pause() {
        use crate::gostd::local;
        std::thread::spawn(|| {
            let program: &'static GoProgram = Box::leak(Box::new(GoProgram {
                id: next_program_id(),
                options: Box::leak(Box::default()),
                state: std::sync::OnceLock::new(),
            }));
            local::keep_garbage();
            local::note_message_gap(std::time::Duration::from_millis(20));
            let f = NodeFactory::new();
            let base = f.new_identifier("base");
            let base_slots = synthetic_live_slot_count();

            open_synthetic_owner(program.id);
            let owned = {
                let _program = enter_program(Some(program));
                f.new_identifier("owned")
            };
            free_synthetic_owner(program.id);

            assert_eq!(synthetic_live_slot_count(), base_slots);
            let freed = std::panic::catch_unwind(|| owned.kind())
                .expect_err("a node of a freed owner is read");
            assert_eq!(panic_message(&*freed), Some(FREED));
            assert_eq!(base.text(), "base");
            assert_eq!(local::garbage_len(), 1, "the chunks do not wait");
            local::drop_garbage(|| false);
            assert_eq!(local::garbage_len(), 0);
            free_synthetic_nodes();
        })
        .join()
        .expect("test thread panicked");
    }

    /// The number of chunks of each table of the print owner of this
    /// thread.
    fn print_chunk_counts() -> (usize, usize, usize) {
        ARENA.with(|a| {
            let c = &a.borrow().print.chunks;
            (c.slots.len(), c.datas.len(), c.lists.len())
        })
    }

    /// The panic message of `read`, or `None` when it does not panic.
    fn read_panic<R>(read: impl FnOnce() -> R) -> Option<String> {
        let payload = std::panic::catch_unwind(std::panic::AssertUnwindSafe(read)).err()?;
        Some(panic_message(&*payload).unwrap_or("").to_string())
    }

    // The end of a print scope frees the nodes and lists of the to-string
    // factory made in it: a read of one panics. The nodes of other
    // factories and of earlier calls stay, and a handle is never used again.
    #[test]
    fn print_scope_frees_the_nodes_of_its_call() {
        std::thread::spawn(|| {
            let printer = NodeFactory::new();
            printer.set_print_owned();
            let plain = NodeFactory::new();
            let before = printer.new_identifier("before");

            enter_print_scope();
            let a = printer.new_identifier("a");
            let list = printer.new_node_list(&[a, before]);
            let block = printer.new_block(list, false);
            let cloned = printer.new_synthetic_node_list(&[a], TextRange::new(-1, -1));
            let other = plain.new_identifier("other");
            assert!(is_print_node(a) && is_print_node(block));
            assert!(!is_print_node(before) && !is_print_node(other));
            assert_eq!(block.statement_list().nodes().to_vec(), vec![a, before]);
            let ranges = exit_print_scope().expect("an unpinned call frees");

            let freed: Vec<Node> = print_range_nodes(&ranges).collect();
            assert_eq!(freed, vec![a, block]);
            for read in [read_panic(|| a.text()), read_panic(|| block.kind())] {
                assert_eq!(read.as_deref(), Some(PRINT_FREED));
            }
            assert_eq!(
                read_panic(|| list.nodes().to_vec()).as_deref(),
                Some(PRINT_FREED)
            );
            assert_eq!(
                read_panic(|| cloned.nodes().to_vec()).as_deref(),
                Some(PRINT_FREED)
            );
            assert_eq!(before.text(), "before");
            assert_eq!(other.text(), "other");

            // Outside a scope the to-string factory makes ordinary nodes.
            let after = printer.new_identifier("after");
            assert!(!is_print_node(after));
            enter_print_scope();
            let b = printer.new_identifier("b");
            assert!(b != a && b != block && b != after);
            exit_print_scope();
            assert_eq!(after.text(), "after");
            free_synthetic_nodes();
        })
        .join()
        .expect("test thread panicked");
    }

    // A pinned call keeps its nodes: they go to the current owner. A seed
    // keeps them in place, also behind the emptied cells of an earlier call
    // in the same data chunk.
    #[test]
    fn a_pinned_print_scope_keeps_its_nodes() {
        std::thread::spawn(|| {
            let printer = NodeFactory::new();
            printer.set_print_owned();
            enter_print_scope();
            let gone = printer.new_identifier("gone");
            exit_print_scope();

            enter_print_scope();
            let kept = printer.new_identifier("kept");
            let list = printer.new_node_list(&[kept]);
            pin_print_scope();
            assert_eq!(exit_print_scope(), None);
            assert!(!is_print_node(kept));
            assert_eq!(print_chunk_counts(), (0, 0, 0));
            assert_eq!(kept.text(), "kept");
            assert_eq!(list.nodes().to_vec(), vec![kept]);
            assert_eq!(read_panic(|| gone.text()).as_deref(), Some(PRINT_FREED));

            // The next call starts new chunks, and the base keeps filling its own.
            enter_print_scope();
            let next = printer.new_identifier("next");
            exit_print_scope();
            assert!(read_panic(|| next.text()).is_some());
            let base = printer.new_identifier("base");

            let seed = synthetic_seed();
            std::thread::spawn(move || {
                install_synthetic_seed(seed);
                assert_eq!(kept.text(), "kept");
                assert_eq!(list.nodes().to_vec(), vec![kept]);
                assert_eq!(base.text(), "base");
                assert!(read_panic(|| gone.text()).is_some());
            })
            .join()
            .expect("seed thread panicked");
            free_synthetic_nodes();
        })
        .join()
        .expect("test thread panicked");
    }

    // A nested scope joins the outermost one: its nodes live until the
    // outermost scope ends.
    #[test]
    fn nested_print_scopes_free_at_the_outermost_end() {
        std::thread::spawn(|| {
            let printer = NodeFactory::new();
            printer.set_print_owned();
            enter_print_scope();
            let outer = printer.new_identifier("outer");
            enter_print_scope();
            let inner = printer.new_identifier("inner");
            assert_eq!(exit_print_scope(), None);
            assert_eq!(inner.text(), "inner");
            let late = printer.new_identifier("late");
            let ranges = exit_print_scope().expect("the outermost scope frees");
            assert_eq!(
                print_range_nodes(&ranges).collect::<Vec<_>>(),
                vec![outer, inner, late]
            );
            for n in [outer, inner, late] {
                assert!(read_panic(|| n.text()).is_some());
            }
            free_synthetic_nodes();
        })
        .join()
        .expect("test thread panicked");
    }

    // Calls across many chunks: each ended call's nodes panic, the full
    // chunks go, and the print owner keeps one chunk per table.
    #[test]
    fn print_scopes_free_full_chunks() {
        std::thread::spawn(|| {
            let printer = NodeFactory::new();
            printer.set_print_owned();
            let mut last = Vec::new();
            for i in 0..3 * SLOT_CHUNK {
                enter_print_scope();
                let id = printer.new_identifier(format!("n{i}"));
                let list = printer.new_node_list(&[id, id]);
                last = vec![printer.new_type_literal_node(list)];
                assert_eq!(list.nodes().len(), 2);
                exit_print_scope().expect("an unpinned call frees");
                assert!(print_chunk_counts().0 <= 1);
            }
            assert_eq!(print_chunk_counts(), (1, 1, 1));
            assert!(read_panic(|| last[0].kind()).is_some());
            // Freed slots stay in the kept chunk; the rest went.
            assert!(synthetic_live_slot_count() <= 1 + SLOT_CHUNK);
            free_synthetic_nodes();
        })
        .join()
        .expect("test thread panicked");
    }

    // A scope that ends while its call panics keeps the call's nodes.
    #[test]
    fn a_print_scope_that_unwinds_keeps_its_nodes() {
        struct Scope;
        impl Drop for Scope {
            fn drop(&mut self) {
                assert_eq!(exit_print_scope(), None);
            }
        }
        std::thread::spawn(|| {
            let printer = NodeFactory::new();
            printer.set_print_owned();
            let made = std::cell::Cell::new(Node::NIL);
            let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                enter_print_scope();
                let _scope = Scope;
                made.set(printer.new_identifier("made"));
                panic!("in a to-string call");
            }));
            assert!(caught.is_err());
            assert_eq!(made.get().text(), "made");
            free_synthetic_nodes();
        })
        .join()
        .expect("test thread panicked");
    }
}
