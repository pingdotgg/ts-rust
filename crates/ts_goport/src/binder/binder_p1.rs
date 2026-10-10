//! Port of typescript-go `internal/binder/binder.go` lines 1-910.
//!
//! PORT: Go stores binder output on AST nodes and on `ast.SourceFile`. Here
//! the AST is immutable, so the binder keeps that output in `Binder`
//! (`node_bind`, `file_bind`, `flow_nodes`) while it runs, and
//! `bind_source_file` moves it into the node records (`ast/store.rs`) and
//! the `GoFile` `OnceCell`s at the end (`BoundFile::install`).
//! Binder code must read and write node binder data through the `Binder`
//! helpers below (`node_symbol`, `set_node_symbol`, `node_flags`,
//! `get_locals`, `flow`, `flow_mut`, ...), not through `Node::symbol()` and
//! friends, which only see the data after binding finishes.

use crate::astdata::NodeData;
use crate::prelude::*;

// Go: binder/binder.go:45 ExpandoAssignmentInfo
#[derive(Clone, Copy, Debug, Default)]
pub struct ExpandoAssignmentInfo {
    pub node: Node,
    pub container: Node,
    pub block_scope_container: Node,
}

// Go: binder/binder.go:51 Binder
//
// PORT: `bindFunc` (a cached closure over `b.bind`) is not stored; callers
// use a closure over `self.bind` at the call site. `symbolArena` becomes
// `symbols` (the shared `SymbolArena`, moved in for the duration of
// binding). `flowNodeArena` becomes `flow_nodes`. `flowListArena` and
// `singleDeclarationsArena` are not needed: `FlowList` is a
// `Vec<FlowNodeId>` and declaration lists are `Vec<Node>`. The last five
// fields are Rust-only storage for data Go writes onto nodes and the file.
#[derive(Default)]
pub struct Binder {
    pub file: Node,
    pub unreachable_flow: FlowNodeId,

    pub container: Node,
    pub this_container: Node,
    pub block_scope_container: Node,
    pub last_container: Node,
    pub current_flow: FlowNodeId,
    pub current_break_target: FlowNodeId,
    pub current_continue_target: FlowNodeId,
    pub current_return_target: FlowNodeId,
    pub current_true_target: FlowNodeId,
    pub current_false_target: FlowNodeId,
    pub current_exception_target: FlowNodeId,
    pub pre_switch_case_flow: FlowNodeId,
    pub active_label_list: Option<Rc<RefCell<ActiveLabel>>>,
    pub emit_flags: NodeFlags,
    pub seen_this_keyword: bool,
    pub has_explicit_return: bool,
    pub has_flow_effects: bool,
    pub in_assignment_pattern: bool,
    pub seen_parse_error: bool,
    pub symbol_count: i32,
    pub not_const_enum_only_modules: FxHashSet<SymbolId>,
    /// Go `symbolArena`. The program-wide symbol arena, moved in while binding.
    pub symbols: SymbolArena,
    /// Go `flowNodeArena`. Flow nodes of this file, indexed by
    /// `FlowNodeId::local_index`.
    pub flow_nodes: Vec<FlowNode>,
    pub expando_assignments: Vec<ExpandoAssignmentInfo>,

    /// Rust-only: `Node::file_index` of `file`.
    pub file_index: usize,
    /// Rust-only: parser flags of `file` (`GoFile::parser_flags`), indexed
    /// by `NodeId::index()`. Cached so `node_flags` skips the program lookup.
    /// The guard pins a freeable file version while the binder runs.
    pub parser_flags: FileRef<[NodeFlags]>,
    /// Rust-only: binder data per node of `file`, indexed by `NodeId::index()`.
    pub node_bind: NodeBindBuilder,
    /// Rust-only: binder fields of `ast.SourceFile` (`BindDiagnostics`,
    /// `EndFlowNode`, `PatternAmbientModules`, `GlobalExports`, ...).
    pub file_bind: FileBindData,
    /// Go `file.CommonJSModuleIndicator` while binding. It is copied to
    /// `file_bind` when the bind ends.
    pub common_js_module_indicator: Node,
    /// Rust-only: the child link column of `file` when it is a freeable
    /// file version (`ast::version_child_links`), for `bind_each_child`.
    /// The guard pins the version while the binder runs. `None` for a
    /// static file, whose block has the column.
    pub version_links: Option<crate::ast::VersionChildLinks>,
    /// Rust-only: how the child ids of `file` resolve when it is a
    /// published store (`ast::frozen_store_ids`), read once for the node
    /// views (`binary_view`). `None` for any other file.
    pub frozen_ids: Option<FrozenIds>,
}

// Go: binder/binder.go:84 ActiveLabel
#[derive(Clone, Debug, Default)]
pub struct ActiveLabel {
    pub next: Option<Rc<RefCell<ActiveLabel>>>,
    pub break_target: FlowNodeId,
    pub continue_target: FlowNodeId,
    pub name: String,
    pub referenced: bool,
}

impl ActiveLabel {
    // Go: binder/binder.go:92 BreakTarget
    pub fn break_target_exported(&self) -> FlowNodeId {
        self.break_target
    }

    // Go: binder/binder.go:93 ContinueTarget
    pub fn continue_target_exported(&self) -> FlowNodeId {
        self.continue_target
    }
}

impl FlowNode {
    /// Go `flow.Node.AsFlowSwitchClauseData()`.
    pub fn as_flow_switch_clause_data(&self) -> FlowSwitchClauseData {
        assert!(
            self.flags.intersects(FlowFlags::SWITCH_CLAUSE),
            "not a switch clause flow node"
        );
        FlowSwitchClauseData {
            switch_statement: self.node,
            clause_start: self.antecedents[0].0 as u32 as i32,
            clause_end: self.antecedents[1].0 as u32 as i32,
        }
    }

    /// Go `flow.Node.AsFlowReduceLabelData()`.
    pub fn as_flow_reduce_label_data(&self) -> FlowReduceLabelData {
        assert!(
            self.flags.intersects(FlowFlags::REDUCE_LABEL),
            "not a reduce label flow node"
        );
        FlowReduceLabelData {
            target: self.antecedents[0],
            antecedents: self.antecedents[1..].to_vec(),
        }
    }
}

// Go: binder/binder.go:95 BindSourceFile
// Go: binder/binder.go:120 bindSourceFile
//
// PORT: `getBinder`/`putBinder` (a sync.Pool) are not needed. `symbols` is
// the program-wide arena; it is moved into the binder and moved back when
// binding ends. The binder output is stored in the file's `GoFile`
// `OnceCell`s (Go `file.BindOnce`). A file that is already bound is skipped.
// The caller publishes what the file added to `symbols` for the checkers'
// copies (`Lineage::add_file`).
pub fn bind_source_file(file: Node, symbols: &mut SymbolArena) {
    if crate::ast::go_file(file.file_index())
        .file_bind
        .get()
        .is_some()
    {
        return;
    }
    bind_source_file_detached(file, symbols).install();
}

/// Binder data per node of the file being bound. Most nodes get no data, so
/// a node keeps a 4-byte slot into `entries`, and `entries[0]` is the empty
/// data, which is never written. The flow node of a node is kept in `flows`,
/// not in `entries`. The install writes both into the node records
/// (`nodes`, `ast::bind_store_records`); `compact` merges them into the
/// compact form of the lib bind snapshot.
// PERF: bind C. The binder gives a flow node to every identifier (Go
// binder.go `bind`), so a flow node in `entries` made a 40-byte entry for
// each of them. Now it is one 4-byte write.
#[derive(Default)]
pub struct NodeBindBuilder {
    /// Per node, by `NodeId::index()`: index into `entries`, 0 for none.
    slots: Vec<u32>,
    entries: Vec<NodeBindData>,
    /// Per node, by `NodeId::index()`: the low half of its flow node id
    /// (index + 1), 0 for nil.
    flows: Vec<u32>,
    /// The high half of every flow node id of the file: its file index
    /// (`FlowNodeId::new`).
    flow_file: u64,
    /// Nodes that got a flow node, counted when a flow goes from nil to
    /// set. With `entries` it bounds the nodes that have data.
    flow_count: usize,
}

impl NodeBindBuilder {
    /// A builder for the `node_count` nodes of file `file_index`, with room
    /// for `entries` entries before the first copy.
    #[must_use]
    pub fn new(file_index: usize, node_count: usize, entries: usize) -> Self {
        let mut data = Vec::with_capacity(entries.max(1));
        data.push(NodeBindData::default());
        // PERF: `zeroed_vec`, as `get_mut` and `set_flow_node` read a slot
        // before they write it.
        NodeBindBuilder {
            slots: zeroed_vec(node_count),
            entries: data,
            flows: zeroed_vec(node_count),
            flow_file: (file_index as u64) << 32,
            flow_count: 0,
        }
    }

    /// The data of node `index` (`NodeId::index()`), without its flow node.
    /// Binder code never reads a flow node back from a node.
    #[inline]
    #[must_use]
    pub fn get(&self, index: usize) -> &NodeBindData {
        &self.entries[self.slots[index] as usize]
    }

    /// The data of node `index`, made on first write. Set the flow node
    /// with `set_flow_node`, not through this reference.
    #[inline]
    pub fn get_mut(&mut self, index: usize) -> &mut NodeBindData {
        let mut slot = self.slots[index];
        if slot == 0 {
            slot = u32::try_from(self.entries.len()).expect("node bind overflow");
            self.entries.push(NodeBindData::default());
            self.slots[index] = slot;
        }
        &mut self.entries[slot as usize]
    }

    /// Go `node.FlowNodeData().FlowNode = flow` for node `index`.
    #[inline]
    pub fn set_flow_node(&mut self, index: usize, flow: FlowNodeId) {
        debug_assert!(
            flow.is_nil() || (flow.0 & !0xffff_ffff) == self.flow_file,
            "flow node from another file"
        );
        let low = flow.0 as u32;
        let old = std::mem::replace(&mut self.flows[index], low);
        self.flow_count += usize::from(old == 0 && low != 0);
    }

    /// Each node that has data or a flow node, in node order: its
    /// `NodeId::index()`, the index of its entry in `entries`, the data
    /// (`None` for a node with only a flow node) and the flow node.
    fn nodes(&self) -> impl Iterator<Item = (usize, usize, Option<&NodeBindData>, FlowNodeId)> {
        let flow_file = self.flow_file;
        self.slots
            .iter()
            .zip(&self.flows)
            .enumerate()
            .filter(|&(_, (&slot, &flow))| slot | flow != 0)
            .map(move |(index, (&slot, &flow))| {
                let flow = if flow == 0 {
                    FlowNodeId::NIL
                } else {
                    FlowNodeId(flow_file | u64::from(flow))
                };
                let data = (slot != 0).then(|| &self.entries[slot as usize]);
                (index, slot as usize, data, flow)
            })
    }

    /// The compact form of the data, in node order.
    fn compact(&self) -> NodeBindParts {
        debug_assert!(
            self.entries.iter().all(|data| data.flow_node.is_nil()),
            "flow node written outside set_flow_node"
        );
        let entries = &self.entries;
        let flow_file = self.flow_file;
        let nodes = self.slots.iter().zip(&self.flows).map(|(&slot, &flow)| {
            if slot == 0 && flow == 0 {
                return None;
            }
            let mut data = entries[slot as usize];
            if flow != 0 {
                data.flow_node = FlowNodeId(flow_file | u64::from(flow));
            }
            Some(data)
        });
        NodeBindParts::new(nodes, entries.len() - 1 + self.flow_count)
    }
}

/// The binder data of the nodes of one file as the bind hands it over
/// (`BoundFile`): the builder of a live bind, or the compact form that the
/// lib bind snapshot keeps. The install writes either into the node records
/// (`ast::bind_store_records`).
// PERF: AST node records, step 2. A live bind hands over its builder: a
// compact form made on the bind thread would be read again by the install,
// and the compaction (`NodeBindParts::new`) was about 0.35% of the
// instructions of `goport -p` on effect.
pub enum BoundNodes {
    Built(NodeBindBuilder),
    Parts(NodeBindParts),
}

impl BoundNodes {
    /// The compact form, as the lib bind snapshot keeps it.
    #[must_use]
    pub fn compact(&self) -> std::borrow::Cow<'_, NodeBindParts> {
        match self {
            BoundNodes::Built(builder) => std::borrow::Cow::Owned(builder.compact()),
            BoundNodes::Parts(parts) => std::borrow::Cow::Borrowed(parts),
        }
    }

    /// The data entries, for remapping ids in place (`BoundFile::remap`).
    fn entries_mut(&mut self) -> &mut [NodeBindData] {
        match self {
            BoundNodes::Built(builder) => &mut builder.entries,
            BoundNodes::Parts(parts) => parts.entries_mut(),
        }
    }

    /// Writes the data into the node records of published file `file` and
    /// gives the extras (`ast::bind_store_records`).
    fn write_records(&self, file: usize) -> Vec<NodeBindExtra> {
        match self {
            BoundNodes::Built(builder) => crate::ast::bind_store_records(file, builder.nodes()),
            BoundNodes::Parts(parts) => crate::ast::bind_store_records(
                file,
                parts
                    .nodes()
                    .map(|(index, entry, data)| (index, entry, Some(data), data.flow_node)),
            ),
        }
    }
}

/// The binder output of one file before it is stored in its `GoFile`.
pub struct BoundFile {
    pub file: Node,
    pub node_bind: BoundNodes,
    pub flow_nodes: Vec<FlowNode>,
    pub file_bind: FileBindData,
}

/// Binds `file` into `symbols` and returns the output without storing it.
/// Parallel binding binds each file into its own arena with this, then
/// moves the symbols into the program arena (`BoundFile::remap`).
pub fn bind_source_file_detached(file: Node, symbols: &mut SymbolArena) -> BoundFile {
    // PERF: r2-lib-bind-snapshot. A large bundled lib file (lib.dom) bound
    // into a new arena loads its output from the embedded snapshot when the
    // key matches, instead of binding. The output equals a live bind (see
    // `lib_snapshot`). Any other file, or any mismatch, binds live.
    if let Some(bound) = super::lib_snapshot::load(file, symbols) {
        return bound;
    }
    bind_source_file_live(file, symbols)
}

/// `bind_source_file_detached` without the lib bind snapshot: always binds.
/// The snapshot generator and its test use it for the live side.
pub fn bind_source_file_live(file: Node, symbols: &mut SymbolArena) -> BoundFile {
    let file_index = file.file_index();
    let parser_flags =
        crate::ast::file_version::go_file_ref!(file_index, 0, |g, _key| g.parser_flags[..]);
    let node_count = parser_flags.len();
    // PERF: U1 (e). A store file has counts from its slot kinds, made when
    // it was frozen, so the flow nodes grow with no copy. Its entry count
    // also counts one entry per identifier flow node, which bind C keeps in
    // `NodeBindBuilder::flows`, so the entries are sized from the node
    // count. Capacity only: the output does not change.
    let flow_nodes = match frozen_store_bind_estimate(file_index) {
        Some((_, flow_nodes)) => Vec::with_capacity(flow_nodes),
        None => Vec::new(),
    };
    // About one node in four gets data other than a flow node (a
    // declaration, a container or a flagged node).
    let node_bind = NodeBindBuilder::new(file_index, node_count, node_count / 4 + 1);
    // PERF: bind D. About one symbol per 8 nodes and one table per 16 (lib
    // files have more), so the first arena chunk does not grow by doubling.
    // Capacity only.
    symbols.reserve_arena(node_count / 8 + 1, node_count / 16 + 1);
    let mut b = Binder {
        file,
        file_index,
        parser_flags,
        symbols: std::mem::take(symbols),
        node_bind,
        flow_nodes,
        version_links: crate::ast::version_child_links(file_index),
        frozen_ids: frozen_store_ids(file_index),
        ..Binder::default()
    };
    b.unreachable_flow = b.new_flow_node(FlowFlags::UNREACHABLE);
    b.bind(file);
    b.bind_deferred_expando_assignments();
    b.file_bind.symbol_count = b.symbol_count;
    b.file_bind.common_js_module_indicator = b.common_js_module_indicator;
    *symbols = std::mem::take(&mut b.symbols);
    let Binder {
        node_bind,
        file_bind,
        flow_nodes,
        ..
    } = b;
    debug_assert!(
        node_bind.entries.iter().all(|data| data.flow_node.is_nil()),
        "flow node written outside set_flow_node"
    );
    BoundFile {
        file,
        node_bind: BoundNodes::Built(node_bind),
        flow_nodes,
        file_bind,
    }
}

impl BoundFile {
    /// Stores the output in the file's `GoFile` `OnceLock`s (Go
    /// `file.BindOnce`). AST node records, step 2: first writes the symbol,
    /// the flow node and the added flags of each node into its record
    /// (`ast::bind_store_records`); the other fields go into the
    /// `FileNodeBind` of the file. The ids must be program ids already
    /// (`remap`). A parallel bind installs each file on its bind thread, as
    /// Go's `BindSourceFile` stores the output in the file on the goroutine
    /// that binds it (`program::bind_files_parallel`).
    pub fn install(self) {
        let file = self.file.file_index();
        let go_file = crate::ast::go_file(file);
        assert!(go_file.node_bind.get().is_none(), "file already bound");
        let extras = self.node_bind.write_records(file);
        assert!(
            go_file.node_bind.set(FileNodeBind::new(extras)).is_ok(),
            "file already bound"
        );
        assert!(
            go_file.flow_nodes.set(self.flow_nodes).is_ok(),
            "file already bound"
        );
        assert!(
            go_file.file_bind.set(self.file_bind).is_ok(),
            "file already bound"
        );
    }

    /// Moves the symbol and table ids of a file that was bound into its own
    /// arena to their place in the program arena (see
    /// `SymbolArena::append_file_arena`).
    pub fn remap(&mut self, offsets: ArenaOffsets) {
        // A live bind remaps its entry 0 (the empty data) too: nil stays nil.
        for data in self.node_bind.entries_mut() {
            data.symbol = offsets.symbol(data.symbol);
            data.local_symbol = offsets.symbol(data.local_symbol);
            data.locals = offsets.table(data.locals);
        }
        let file_bind = &mut self.file_bind;
        file_bind.global_exports = offsets.table(file_bind.global_exports);
        file_bind.js_global_augmentations = offsets.table(file_bind.js_global_augmentations);
        for module in &mut file_bind.pattern_ambient_modules {
            module.symbol = offsets.symbol(module.symbol);
        }
    }
}

/// Rust-only accessors for data Go stores on nodes, symbols and flow nodes.
impl Binder {
    /// Go `symbol` field reads: `self.sym(s).flags`.
    pub fn sym(&self, symbol: SymbolId) -> &Symbol {
        self.symbols.sym(symbol)
    }

    /// Go `symbol` field writes: `self.sym_mut(s).flags |= ...`.
    pub fn sym_mut(&mut self, symbol: SymbolId) -> &mut Symbol {
        self.symbols.sym_mut(symbol)
    }

    /// Binder data of a node in the file being bound.
    pub fn node_data(&self, node: Node) -> &NodeBindData {
        debug_assert!(node.is_some(), "nil node dereference");
        debug_assert_eq!(node.file_index(), self.file_index, "node from another file");
        self.node_bind.get(node.node_id().index())
    }

    /// Mutable binder data of a node in the file being bound.
    pub fn node_data_mut(&mut self, node: Node) -> &mut NodeBindData {
        debug_assert!(node.is_some(), "nil node dereference");
        debug_assert_eq!(node.file_index(), self.file_index, "node from another file");
        self.node_bind.get_mut(node.node_id().index())
    }

    /// Go `node.Symbol()` while binding.
    pub fn node_symbol(&self, node: Node) -> SymbolId {
        self.node_data(node).symbol
    }

    /// Go `node.DeclarationData().Symbol = symbol`.
    pub fn set_node_symbol(&mut self, node: Node, symbol: SymbolId) {
        self.node_data_mut(node).symbol = symbol;
    }

    /// Go `node.LocalSymbol()` while binding.
    pub fn node_local_symbol(&self, node: Node) -> SymbolId {
        self.node_data(node).local_symbol
    }

    /// Go `node.ExportableData().LocalSymbol = symbol`.
    pub fn set_node_local_symbol(&mut self, node: Node, symbol: SymbolId) {
        self.node_data_mut(node).local_symbol = symbol;
    }

    /// Go `node.Locals()` while binding (nil when not created).
    pub fn node_locals(&self, node: Node) -> SymbolTable {
        self.node_data(node).locals
    }

    /// Go `node.LocalsContainerData().Locals = locals`.
    pub fn set_node_locals(&mut self, node: Node, locals: SymbolTable) {
        // U1 (d): `Node::locals` reads nil for other kinds.
        debug_assert!(is_locals_container(node), "locals on a {:?}", node.kind());
        self.node_data_mut(node).locals = locals;
    }

    /// Go `ast.GetLocals(container)`: creates the locals table on first use.
    pub fn get_locals(&mut self, container: Node) -> SymbolTable {
        let locals = self.node_data(container).locals;
        if locals.is_some() {
            return locals;
        }
        let locals = self
            .symbols
            .new_table_with_capacity(locals_size_hint(container));
        // U1 (d): `Node::locals` reads nil for other kinds.
        debug_assert!(
            is_locals_container(container),
            "locals on a {:?}",
            container.kind()
        );
        self.node_data_mut(container).locals = locals;
        locals
    }

    /// Go `ast.GetMembers(symbol)` where `symbol` is the symbol of
    /// `container`. A new table is sized for the members of `container`.
    pub fn get_container_members(&mut self, container: Node, symbol: SymbolId) -> SymbolTable {
        let members = self.symbols.sym(symbol).members;
        if members.is_some() {
            return members;
        }
        let members = self
            .symbols
            .new_table_with_capacity(members_size_hint(container));
        self.symbols.sym_mut(symbol).members = members;
        members
    }

    /// Go `ast.GetExports(symbol)` where `symbol` is the symbol of
    /// `container`. A new table is sized for the exports of `container`.
    pub fn get_container_exports(&mut self, container: Node, symbol: SymbolId) -> SymbolTable {
        let exports = self.symbols.sym(symbol).exports;
        if exports.is_some() {
            return exports;
        }
        let exports = self
            .symbols
            .new_table_with_capacity(exports_size_hint(container));
        self.symbols.sym_mut(symbol).exports = exports;
        exports
    }

    /// Go `ast.GetSymbolTable(&b.file.GlobalExports)`.
    pub fn get_global_exports(&mut self) -> SymbolTable {
        if self.file_bind.global_exports.is_nil() {
            self.file_bind.global_exports = self.symbols.new_table();
        }
        self.file_bind.global_exports
    }

    /// Go `node.Flags` while binding: parser flags plus binder-added flags.
    #[inline]
    pub fn node_flags(&self, node: Node) -> NodeFlags {
        debug_assert_eq!(node.file_index(), self.file_index, "node from another file");
        let index = node.node_id().index();
        self.parser_flags[index] | self.node_bind.get(index).added_flags
    }

    /// Go `node.AsBinaryExpression()` of BinaryExpression `node` of `file`,
    /// with its fields read once (`BinaryView`).
    // The view reads the child ids of the bound file (`frozen_ids`), so a
    // node of another file would give wrong ids with no panic. The check
    // stays in release builds: one compare per binary expression, with no
    // measurable cost (followups25).
    #[inline]
    pub fn binary_view(&self, node: Node) -> BinaryView {
        assert_eq!(node.file_index(), self.file_index, "node from another file");
        BinaryView::with_ids(node, self.frozen_ids)
    }

    /// Go `node.Flags |= flags`.
    pub fn add_node_flags(&mut self, node: Node, flags: NodeFlags) {
        let data = self.node_data_mut(node);
        data.added_flags = data.added_flags | flags;
    }

    /// Go `node.Flags &^= flags`.
    ///
    /// PORT: only binder-added bits can be cleared. The binder only clears
    /// flags it sets itself (`ExportContext`, ...), so this matches Go.
    pub fn remove_node_flags(&mut self, node: Node, flags: NodeFlags) {
        let data = self.node_data_mut(node);
        data.added_flags = data.added_flags.without(flags);
    }

    /// Go `*ast.FlowNode` field reads.
    pub fn flow(&self, flow: FlowNodeId) -> &FlowNode {
        debug_assert!(flow.is_some(), "nil flow node dereference");
        debug_assert_eq!(
            flow.file_index(),
            self.file_index,
            "flow node from another file"
        );
        &self.flow_nodes[flow.local_index()]
    }

    /// Go `*ast.FlowNode` field writes.
    pub fn flow_mut(&mut self, flow: FlowNodeId) -> &mut FlowNode {
        debug_assert!(flow.is_some(), "nil flow node dereference");
        debug_assert_eq!(
            flow.file_index(),
            self.file_index,
            "flow node from another file"
        );
        &mut self.flow_nodes[flow.local_index()]
    }

    /// Go `b.file.IsDeclarationFile`.
    pub fn file_is_declaration_file(&self) -> bool {
        with_source_file_info(self.file, |info| info.is_declaration_file)
    }

    /// Go `ast.IsExternalOrCommonJSModule(b.file)` while binding. The binder
    /// owns `CommonJSModuleIndicator`, so it reads its own copy.
    pub fn file_is_external_or_common_js_module(&self) -> bool {
        is_external_module(self.file) || self.common_js_module_indicator.is_some()
    }
}

impl Binder {
    // Go: binder/binder.go:132 newSymbol
    pub fn new_symbol(&mut self, flags: SymbolFlags, name: impl Into<Name>) -> SymbolId {
        self.symbol_count += 1;
        self.symbols.new_symbol(flags, name)
    }

    /**
     * Declares a Symbol for the node and adds it to symbols. Reports errors for conflicting identifier names.
     * @param symbolTable - The symbol table which node will be added to.
     * @param parent - node's parent declaration.
     * @param node - The declaration to be added to the symbol table
     * @param includes - The SymbolFlags that node has in addition to its declaration type (eg: export, ambient, etc.)
     * @param excludes - The flags which node cannot be declared alongside in a symbol table. Used to report forbidden declarations.
     */
    // Go: binder/binder.go:148 declareSymbol
    pub fn declare_symbol(
        &mut self,
        symbol_table: SymbolTable,
        parent: SymbolId,
        node: Node,
        includes: SymbolFlags,
        excludes: SymbolFlags,
    ) -> SymbolId {
        self.declare_symbol_ex(
            symbol_table,
            parent,
            node,
            includes,
            excludes,
            false, /*isReplaceableByMethod*/
            false, /*isComputedName*/
        )
    }

    // Go: binder/binder.go:152 declareSymbolEx
    pub fn declare_symbol_ex(
        &mut self,
        symbol_table: SymbolTable,
        parent: SymbolId,
        node: Node,
        includes: SymbolFlags,
        excludes: SymbolFlags,
        is_replaceable_by_method: bool,
        is_computed_name: bool,
    ) -> SymbolId {
        // PERF: query Q7-3. The node data is looked up once here, and the
        // field reads below (modifiers, name) use it. The reads and their
        // order are the same as Go's.
        let data = parsed_node_data(node);
        debug_assert!(is_computed_name || !has_dynamic_name_in(node, data));
        let is_default_export = has_syntactic_modifier_in(node, data, ModifierFlags::DEFAULT)
            || is_export_specifier(node) && module_export_name_is_default(node.name_in(data));
        // The exported symbol for an export default function/class node is always named "default"
        let name: Name = if is_computed_name {
            Name::from(INTERNAL_SYMBOL_NAME_COMPUTED)
        } else if is_default_export && parent.is_some() {
            Name::from(INTERNAL_SYMBOL_NAME_DEFAULT)
        } else {
            self.get_declaration_name_in(node, data)
        };
        let mut symbol: SymbolId;
        if name == INTERNAL_SYMBOL_NAME_MISSING {
            symbol = self.new_symbol(SymbolFlags::NONE, INTERNAL_SYMBOL_NAME_MISSING);
        } else {
            // Check and see if the symbol table already has a symbol with this name.  If not,
            // create a new symbol with this name and add it to the table.  Note that we don't
            // give the new symbol any flags *yet*.  This ensures that it will not conflict
            // with the 'excludes' flags we pass in.
            //
            // If we do get an existing symbol, see if it conflicts with the new symbol we're
            // creating.  For example, a 'var' symbol and a 'class' symbol will conflict within
            // the same symbol table.  If we have a conflict, report the issue on each
            // declaration we have for this symbol, and then create a new symbol for this
            // declaration.
            //
            // Note that when properties declared in Javascript constructors
            // (marked by isReplaceableByMethod) conflict with another symbol, the property loses.
            // Always. This allows the common Javascript pattern of overwriting a prototype method
            // with an bound instance method of the same type: `this.method = this.method.bind(this)`
            //
            // If we created a new symbol, either because we didn't have a symbol with this name
            // in the symbol table, or we conflicted with an existing symbol, then just add this
            // node as the sole declaration of the new symbol.
            //
            // Otherwise, we'll be merging into a compatible existing symbol (for example when
            // you have multiple 'vars' with the same name in the same container).  In this case
            // just add this node into the declarations list of the symbol.
            //
            // PERF: one table lookup finds the symbol and the slot to store
            // a new one; `new_symbol` does not change tables.
            let (found, slot) = self.symbols.get_slot(symbol_table, &name);
            symbol = found;
            if symbol.is_nil() {
                symbol = self.new_symbol(SymbolFlags::NONE, name);
                self.symbols.set_slot(slot, symbol);
                if is_replaceable_by_method {
                    self.sym_mut(symbol).flags |= SymbolFlags::REPLACEABLE_BY_METHOD;
                }
            } else if is_replaceable_by_method
                && !self
                    .sym(symbol)
                    .flags
                    .intersects(SymbolFlags::REPLACEABLE_BY_METHOD)
            {
                // A symbol already exists, so don't add this as a declaration.
                return symbol;
            } else if self.sym(symbol).flags.intersects(excludes) {
                let symbol_flags = self.sym(symbol).flags;
                if symbol_flags.intersects(SymbolFlags::REPLACEABLE_BY_METHOD) {
                    // Javascript constructor-declared symbols can be discarded in favor of
                    // prototype symbols like methods.
                    symbol = self.new_symbol(SymbolFlags::NONE, name.clone());
                    self.symbols.set(symbol_table, name, symbol);
                } else if !(includes.intersects(SymbolFlags::VARIABLE)
                    && symbol_flags.intersects(SymbolFlags::ASSIGNMENT)
                    || includes.intersects(SymbolFlags::ASSIGNMENT)
                        && symbol_flags.intersects(SymbolFlags::VARIABLE))
                {
                    // Assignment declarations are allowed to merge with variables, no matter what other flags they have.
                    // Report errors every position with duplicate declaration
                    // Report errors on previous encountered declarations
                    let mut message: &'static crate::diagnostics::Message =
                        if symbol_flags.intersects(SymbolFlags::BLOCK_SCOPED_VARIABLE) {
                            diag::Cannot_redeclare_block_scoped_variable_0
                        } else {
                            diag::Duplicate_identifier_0
                        };
                    let mut message_needs_name = true;
                    if symbol_flags.intersects(SymbolFlags::ENUM)
                        || includes.intersects(SymbolFlags::ENUM)
                    {
                        message = diag::Enum_declarations_can_only_merge_with_namespace_or_other_enum_declarations;
                        message_needs_name = false;
                    }
                    let mut multiple_default_exports = false;
                    let declarations = self.sym(symbol).declarations.clone();
                    if !declarations.is_empty() {
                        // If the current node is a default export of some sort, then check if
                        // there are any other default exports that we need to error on.
                        // We'll know whether we have other default exports depending on if `symbol` already has a declaration list set.
                        if is_default_export {
                            message = diag::A_module_cannot_have_multiple_default_exports;
                            message_needs_name = false;
                            multiple_default_exports = true;
                        } else {
                            // This is to properly report an error in the case "export default { }" is after export default of class declaration or function declaration.
                            // Error on multiple export default in the following case:
                            // 1. multiple export default of class declaration or function declaration by checking NodeFlags.Default
                            // 2. multiple export default of export assignment. This one doesn't have NodeFlags.Default on (as export default doesn't considered as modifiers)
                            if !declarations.is_empty()
                                && is_export_assignment(node)
                                && !node.is_export_equals()
                            {
                                message = diag::A_module_cannot_have_multiple_default_exports;
                                message_needs_name = false;
                                multiple_default_exports = true;
                            }
                        }
                    }
                    let mut declaration_name = get_name_of_declaration_in(node, data);
                    if declaration_name.is_nil() {
                        declaration_name = node;
                    }
                    let mut diagnostic = if message_needs_name {
                        let display_name = self.get_display_name(node);
                        self.create_diagnostic_for_node(
                            declaration_name,
                            message,
                            args![display_name],
                        )
                    } else {
                        self.create_diagnostic_for_node(declaration_name, message, vec![])
                    };
                    if is_type_alias_declaration(node)
                        && node_is_missing(node.type_())
                        && has_syntactic_modifier(node, ModifierFlags::EXPORT)
                        && symbol_flags.intersects(
                            SymbolFlags::ALIAS | SymbolFlags::TYPE | SymbolFlags::NAMESPACE,
                        )
                    {
                        // export type T; - may have meant export type { T }?
                        let related = self.create_diagnostic_for_node(
                            node,
                            diag::Did_you_mean_0,
                            args![format!("export type {{ {} }}", node.name().text())],
                        );
                        diagnostic.related_information.push(related);
                    }
                    for (index, &declaration) in declarations.iter().enumerate() {
                        let mut decl = get_name_of_declaration(declaration);
                        if decl.is_nil() {
                            decl = declaration;
                        }
                        let mut d = if message_needs_name {
                            let display_name = self.get_display_name(declaration);
                            self.create_diagnostic_for_node(decl, message, args![display_name])
                        } else {
                            self.create_diagnostic_for_node(decl, message, vec![])
                        };
                        if multiple_default_exports {
                            let related = self.create_diagnostic_for_node(
                                declaration_name,
                                if index == 0 {
                                    diag::Another_export_default_is_here
                                } else {
                                    diag::X_and_here
                                },
                                vec![],
                            );
                            d.related_information.push(related);
                        }
                        self.add_diagnostic(d);
                        if multiple_default_exports {
                            let related = self.create_diagnostic_for_node(
                                decl,
                                diag::The_first_export_default_is_here,
                                vec![],
                            );
                            diagnostic.related_information.push(related);
                        }
                    }
                    self.add_diagnostic(diagnostic);
                    // When get or set accessor conflicts with a non-accessor or an accessor of a different kind, we mark
                    // the symbol as a full accessor such that all subsequent declarations are considered conflicting. This
                    // for example ensures that a get accessor followed by a non-accessor followed by a set accessor with the
                    // same name are all marked as duplicates.
                    let symbol_flags = self.sym(symbol).flags;
                    if symbol_flags.intersects(SymbolFlags::ACCESSOR)
                        && (symbol_flags & SymbolFlags::ACCESSOR)
                            != (includes & SymbolFlags::ACCESSOR)
                    {
                        self.sym_mut(symbol).flags |= SymbolFlags::ACCESSOR;
                    }
                    symbol = self.new_symbol(SymbolFlags::NONE, name);
                }
            }
        }
        self.add_declaration_to_symbol(symbol, node, includes);
        let existing_parent = self.sym(symbol).parent;
        if existing_parent.is_nil() {
            self.sym_mut(symbol).parent = parent;
        } else if existing_parent != parent {
            panic!("Existing symbol parent should match new one");
        }
        symbol
    }

    // Should not be called on a declaration with a computed property name,
    // unless it is a well known Symbol.
    // Go: binder/binder.go:301 getDeclarationName
    // PORT: returns the interned name, so a declaration interns its text once
    // and allocates nothing for an identifier name. `&mut self` only to note
    // a private identifier name in the arena (`note_private_name`).
    pub fn get_declaration_name(&mut self, node: Node) -> Name {
        self.get_declaration_name_in(node, parsed_node_data(node))
    }

    /// `get_declaration_name` on `d`, the data of `node` that the caller
    /// already loaded with `parsed_node_data` (query Q7-3). It holds the body.
    pub fn get_declaration_name_in(&mut self, node: Node, d: LoadedData) -> Name {
        if is_export_assignment(node) {
            return if node.is_export_equals() {
                Name::from(INTERNAL_SYMBOL_NAME_EXPORT_EQUALS)
            } else {
                Name::from(INTERNAL_SYMBOL_NAME_DEFAULT)
            };
        }
        let name = get_name_of_declaration_in(node, d);
        if name.is_some() {
            if is_ambient_module(node) {
                let module_name = name.text();
                if is_global_scope_augmentation(node) {
                    return Name::from(INTERNAL_SYMBOL_NAME_GLOBAL);
                }
                let pattern = try_parse_pattern(module_name);
                if pattern.is_valid() && pattern.star_index >= 0 {
                    let attributes = node.attributes();
                    if attributes.is_some() {
                        return Name::from(format!(
                            "{INTERNAL_SYMBOL_NAME_PREFIX}\"{module_name}\"pattern@{}",
                            get_node_id(attributes)
                        ));
                    }
                }
                return Name::from(format!("\"{module_name}\""));
            }
            if is_private_identifier(name) {
                // containingClass exists because private names only allowed inside classes
                let containing_class = get_containing_class(node);
                if containing_class.is_nil() {
                    // we can get here in cases where there is already a parse error.
                    return Name::from(INTERNAL_SYMBOL_NAME_MISSING);
                }
                let private_name = Name::from(get_symbol_name_for_private_identifier(
                    &self.symbols,
                    self.node_symbol(containing_class),
                    name.text(),
                ));
                self.symbols.note_private_name(&private_name);
                return private_name;
            }
            if is_property_name_literal(name) || is_jsx_namespaced_name(name) {
                // PERF: U1 (a). An identifier name reads the name interned at
                // parse (`Node::text_name`), with no text load and no intern.
                if is_identifier(name) {
                    return name.text_name();
                }
                return Name::from(name.text());
            }
            if is_computed_property_name(name) {
                let name_expression = name.expression();
                // treat computed property names where expression is string/numeric literal as just string/numeric literal
                if is_string_or_numeric_literal_like(name_expression) {
                    return Name::from(name_expression.text());
                }
                if is_signed_numeric_literal(name_expression) {
                    return Name::from(format!(
                        "{}{}",
                        token_to_string(name_expression.operator()),
                        name_expression.operand().text()
                    ));
                }
                panic!("Only computed properties with literal names have declaration names");
            }
            return Name::from(INTERNAL_SYMBOL_NAME_MISSING);
        }
        match node.kind() {
            SyntaxKind::Constructor => return Name::from(INTERNAL_SYMBOL_NAME_CONSTRUCTOR),
            SyntaxKind::FunctionType | SyntaxKind::CallSignature => {
                return Name::from(INTERNAL_SYMBOL_NAME_CALL);
            }
            SyntaxKind::ConstructorType | SyntaxKind::ConstructSignature => {
                return Name::from(INTERNAL_SYMBOL_NAME_NEW);
            }
            SyntaxKind::IndexSignature => return Name::from(INTERNAL_SYMBOL_NAME_INDEX),
            SyntaxKind::ExportDeclaration => return Name::from(INTERNAL_SYMBOL_NAME_EXPORT_STAR),
            SyntaxKind::SourceFile | SyntaxKind::BinaryExpression => {
                return Name::from(INTERNAL_SYMBOL_NAME_EXPORT_EQUALS);
            }
            _ => {}
        }
        Name::from(INTERNAL_SYMBOL_NAME_MISSING)
    }

    // Go: binder/binder.go:362 getDisplayName
    pub fn get_display_name(&mut self, node: Node) -> String {
        let name_node = node.name();
        if name_node.is_some() {
            return declaration_name_to_string(name_node);
        }
        let name = self.get_declaration_name(node);
        if name != INTERNAL_SYMBOL_NAME_MISSING {
            return name.into();
        }
        "(Missing)".to_string()
    }
}

// Go: binder/binder.go:374 GetSymbolNameForPrivateIdentifier
//
// PORT: Go `ast.GetSymbolId` hands out lazy global ids. Here the arena index
// is the symbol id. It is stable because every checker arena starts as a
// clone of the binder arena, and it is unique, which is all the name needs
// inside the process. Go binds files in parallel, and each file into its own
// arena here, so a bind cannot give the Go id. Where Go shows `symbol.Name`
// outside the process (the API), `go_symbol_name` puts the id back.
pub fn get_symbol_name_for_private_identifier(
    symbols: &SymbolArena,
    containing_class_symbol: SymbolId,
    description: &str,
) -> String {
    let _ = symbols;
    format!(
        "{}#{}@{}",
        INTERNAL_SYMBOL_NAME_PREFIX, containing_class_symbol.0, description
    )
}

/// Rust-only: the text of Go `symbol.Name` for `symbol`, as the API returns
/// it (Go api/session.go:75 `newSymbolResponse`). A private identifier name
/// holds the arena index of its class
/// (`get_symbol_name_for_private_identifier`). Go holds `ast.GetSymbolId` of
/// the class there, which is also the API handle of the class (Go
/// api/proto.go:40 `SymbolHandle`). This returns the name with that id
/// (`get_symbol_id`). Other names come back as they are. Only a symbol that
/// a private identifier declares has such a name, as in
/// `SymbolArena::prepare_file_arena`; a name that source text spells stays.
#[must_use]
pub fn go_symbol_name(symbols: &SymbolArena, symbol: SymbolId) -> String {
    let s = symbols.sym(symbol);
    let name = s.name.as_str();
    if !s.name.is_internal() {
        return name.to_string();
    }
    let Some(rest) = name
        .strip_prefix(INTERNAL_SYMBOL_NAME_PREFIX)
        .and_then(|rest| rest.strip_prefix('#'))
    else {
        return name.to_string();
    };
    let Some(at) = rest.find('@') else {
        return name.to_string();
    };
    let Ok(class) = rest[..at].parse::<u32>() else {
        return name.to_string();
    };
    if class == 0
        || class as usize >= symbols.symbol_count()
        || !s
            .declarations
            .iter()
            .any(|&declaration| is_private_identifier(get_name_of_declaration(declaration)))
    {
        return name.to_string();
    }
    format!(
        "{}#{}{}",
        INTERNAL_SYMBOL_NAME_PREFIX,
        get_symbol_id(symbols, SymbolId(class)),
        &rest[at..]
    )
}

impl Binder {
    // Go: binder/binder.go:378 declareModuleMember
    pub fn declare_module_member(
        &mut self,
        node: Node,
        symbol_flags: SymbolFlags,
        symbol_excludes: SymbolFlags,
    ) -> SymbolId {
        let container = self.container;
        let has_export_modifier = get_combined_modifier_flags(node)
            .intersects(ModifierFlags::EXPORT)
            || self.is_implicitly_exported_js_doc_declaration(node);
        if symbol_flags.intersects(SymbolFlags::ALIAS) {
            if node.kind() == SyntaxKind::ExportSpecifier
                || (node.kind() == SyntaxKind::ImportEqualsDeclaration && has_export_modifier)
            {
                let container_symbol = self.node_symbol(container);
                let exports = self.get_container_exports(container, container_symbol);
                return self.declare_symbol(
                    exports,
                    container_symbol,
                    node,
                    symbol_flags,
                    symbol_excludes,
                );
            }
            let locals = self.get_locals(container);
            return self.declare_symbol(
                locals,
                SymbolId::NIL, /*parent*/
                node,
                symbol_flags,
                symbol_excludes,
            );
        }
        // Exported module members are given 2 symbols: A local symbol that is classified with an ExportValue flag,
        // and an associated export symbol with all the correct flags set on it. There are 2 main reasons:
        //
        //   1. We treat locals and exports of the same name as mutually exclusive within a container.
        //      That means the binder will issue a Duplicate Identifier error if you mix locals and exports
        //      with the same name in the same container.
        //      TODO: Make this a more specific error and decouple it from the exclusion logic.
        //   2. When we checkIdentifier in the checker, we set its resolved symbol to the local symbol,
        //      but return the export symbol (by calling getExportSymbolOfValueSymbolIfExported). That way
        //      when the emitter comes back to it, it knows not to qualify the name if it was found in a containing scope.
        //
        // NOTE: Nested ambient modules always should go to to 'locals' table to prevent their automatic merge
        //       during global merging in the checker. Why? The only case when ambient module is permitted inside another module is module augmentation
        //       and this case is specially handled. Module augmentations should only be merged with original module definition
        //       and should never be merged directly with other augmentation, and the latter case would be possible if automatic merge is allowed.
        if !is_ambient_module(node)
            && (has_export_modifier
                || self
                    .node_flags(container)
                    .intersects(NodeFlags::EXPORT_CONTEXT))
        {
            if !is_locals_container(container)
                || (has_syntactic_modifier(node, ModifierFlags::DEFAULT)
                    && self.get_declaration_name(node) == INTERNAL_SYMBOL_NAME_MISSING)
            {
                let container_symbol = self.node_symbol(container);
                let exports = self.get_container_exports(container, container_symbol);
                return self.declare_symbol(
                    exports,
                    container_symbol,
                    node,
                    symbol_flags,
                    symbol_excludes,
                );
                // No local symbol for an unnamed default!
            }
            let export_kind = if symbol_flags.intersects(SymbolFlags::VALUE) {
                SymbolFlags::EXPORT_VALUE
            } else {
                SymbolFlags::NONE
            };
            let locals = self.get_locals(container);
            let local = self.declare_symbol(
                locals,
                SymbolId::NIL, /*parent*/
                node,
                export_kind,
                symbol_excludes,
            );
            let container_symbol = self.node_symbol(container);
            let exports = self.get_container_exports(container, container_symbol);
            let export_symbol = self.declare_symbol(
                exports,
                container_symbol,
                node,
                symbol_flags,
                symbol_excludes,
            );
            self.sym_mut(local).export_symbol = export_symbol;
            self.set_node_local_symbol(node, local);
            return local;
        }
        let locals = self.get_locals(container);
        self.declare_symbol(
            locals,
            SymbolId::NIL, /*parent*/
            node,
            symbol_flags,
            symbol_excludes,
        )
    }

    // Go: binder/binder.go:419 declareClassMember
    pub fn declare_class_member(
        &mut self,
        node: Node,
        symbol_flags: SymbolFlags,
        symbol_excludes: SymbolFlags,
    ) -> SymbolId {
        let container_symbol = self.node_symbol(self.container);
        if is_static(node) {
            let exports = get_exports(&mut self.symbols, container_symbol);
            return self.declare_symbol(
                exports,
                container_symbol,
                node,
                symbol_flags,
                symbol_excludes,
            );
        }
        let members = self.get_container_members(self.container, container_symbol);
        self.declare_symbol(
            members,
            container_symbol,
            node,
            symbol_flags,
            symbol_excludes,
        )
    }

    // Go: binder/binder.go:426 declareSourceFileMember
    pub fn declare_source_file_member(
        &mut self,
        node: Node,
        symbol_flags: SymbolFlags,
        symbol_excludes: SymbolFlags,
    ) -> SymbolId {
        if is_external_module(self.file) {
            return self.declare_module_member(node, symbol_flags, symbol_excludes);
        }
        let locals = self.get_locals(self.file);
        self.declare_symbol(
            locals,
            SymbolId::NIL, /*parent*/
            node,
            symbol_flags,
            symbol_excludes,
        )
    }

    // Go: binder/binder.go:433 declareSymbolAndAddToSymbolTable
    pub fn declare_symbol_and_add_to_symbol_table(
        &mut self,
        node: Node,
        symbol_flags: SymbolFlags,
        symbol_excludes: SymbolFlags,
    ) -> SymbolId {
        match self.container.kind() {
            SyntaxKind::ModuleDeclaration => {
                return self.declare_module_member(node, symbol_flags, symbol_excludes);
            }
            SyntaxKind::SourceFile => {
                return self.declare_source_file_member(node, symbol_flags, symbol_excludes);
            }
            SyntaxKind::ClassExpression | SyntaxKind::ClassDeclaration => {
                return self.declare_class_member(node, symbol_flags, symbol_excludes);
            }
            SyntaxKind::EnumDeclaration => {
                let container_symbol = self.node_symbol(self.container);
                let exports = self.get_container_exports(self.container, container_symbol);
                return self.declare_symbol(
                    exports,
                    container_symbol,
                    node,
                    symbol_flags,
                    symbol_excludes,
                );
            }
            SyntaxKind::TypeLiteral
            | SyntaxKind::ObjectLiteralExpression
            | SyntaxKind::InterfaceDeclaration
            | SyntaxKind::JsxAttributes => {
                let container_symbol = self.node_symbol(self.container);
                let members = self.get_container_members(self.container, container_symbol);
                return self.declare_symbol(
                    members,
                    container_symbol,
                    node,
                    symbol_flags,
                    symbol_excludes,
                );
            }
            SyntaxKind::FunctionType
            | SyntaxKind::ConstructorType
            | SyntaxKind::CallSignature
            | SyntaxKind::ConstructSignature
            | SyntaxKind::IndexSignature
            | SyntaxKind::MethodDeclaration
            | SyntaxKind::MethodSignature
            | SyntaxKind::Constructor
            | SyntaxKind::GetAccessor
            | SyntaxKind::SetAccessor
            | SyntaxKind::FunctionDeclaration
            | SyntaxKind::FunctionExpression
            | SyntaxKind::ArrowFunction
            | SyntaxKind::ClassStaticBlockDeclaration
            | SyntaxKind::TypeAliasDeclaration
            | SyntaxKind::JsTypeAliasDeclaration
            | SyntaxKind::MappedType => {
                let locals = self.get_locals(self.container);
                return self.declare_symbol(
                    locals,
                    SymbolId::NIL, /*parent*/
                    node,
                    symbol_flags,
                    symbol_excludes,
                );
            }
            _ => {}
        }
        panic!("Unhandled case in declareSymbolAndAddToSymbolTable");
    }

    // Go: binder/binder.go:454 newFlowNode
    pub fn new_flow_node(&mut self, flags: FlowFlags) -> FlowNodeId {
        let id = FlowNodeId::new(self.file_index, self.flow_nodes.len());
        self.flow_nodes.push(FlowNode {
            flags,
            ..FlowNode::default()
        });
        id
    }

    // Go: binder/binder.go:460 newFlowNodeEx
    pub fn new_flow_node_ex(
        &mut self,
        flags: FlowFlags,
        node: Node,
        antecedent: FlowNodeId,
    ) -> FlowNodeId {
        let result = self.new_flow_node(flags);
        let flow = self.flow_mut(result);
        flow.node = node;
        flow.antecedent = antecedent;
        result
    }

    // Go: binder/binder.go:467 createLoopLabel
    pub fn create_loop_label(&mut self) -> FlowNodeId {
        self.new_flow_node(FlowFlags::LOOP_LABEL)
    }

    // Go: binder/binder.go:471 createBranchLabel
    pub fn create_branch_label(&mut self) -> FlowNodeId {
        self.new_flow_node(FlowFlags::BRANCH_LABEL)
    }

    // Go: binder/binder.go:475 createReduceLabel
    //
    // PORT: the Go `FlowReduceLabelData` node is stored in the flow node's
    // `antecedents` as `[target, antecedents...]`; see `FlowReduceLabelData`.
    pub fn create_reduce_label(
        &mut self,
        target: FlowNodeId,
        antecedents: &[FlowNodeId],
        antecedent: FlowNodeId,
    ) -> FlowNodeId {
        let result = self.new_flow_node_ex(FlowFlags::REDUCE_LABEL, Node::NIL, antecedent);
        let mut data = Vec::with_capacity(antecedents.len() + 1);
        data.push(target);
        data.extend_from_slice(antecedents);
        self.flow_mut(result).antecedents = data;
        result
    }

    // Go: binder/binder.go:479 createFlowCondition
    pub fn create_flow_condition(
        &mut self,
        flags: FlowFlags,
        antecedent: FlowNodeId,
        expression: Node,
    ) -> FlowNodeId {
        if self
            .flow(antecedent)
            .flags
            .intersects(FlowFlags::UNREACHABLE)
        {
            return antecedent;
        }
        if expression.is_nil() {
            if flags.intersects(FlowFlags::TRUE_CONDITION) {
                return antecedent;
            }
            return self.unreachable_flow;
        }
        if (expression.kind() == SyntaxKind::TrueKeyword
            && flags.intersects(FlowFlags::FALSE_CONDITION)
            || expression.kind() == SyntaxKind::FalseKeyword
                && flags.intersects(FlowFlags::TRUE_CONDITION))
            && !is_expression_of_optional_chain_root(expression)
            && !is_nullish_coalesce(expression.parent())
        {
            return self.unreachable_flow;
        }
        if !is_narrowing_expression(expression) {
            return antecedent;
        }
        self.set_flow_node_referenced(antecedent);
        self.new_flow_node_ex(flags, expression, antecedent)
    }

    // Go: binder/binder.go:499 createFlowMutation
    pub fn create_flow_mutation(
        &mut self,
        flags: FlowFlags,
        antecedent: FlowNodeId,
        node: Node,
    ) -> FlowNodeId {
        self.set_flow_node_referenced(antecedent);
        self.has_flow_effects = true;
        let result = self.new_flow_node_ex(flags, node, antecedent);
        if self.current_exception_target.is_some() {
            let target = self.current_exception_target;
            self.add_antecedent(target, result);
        }
        result
    }

    // Go: binder/binder.go:509 createFlowSwitchClause
    //
    // PORT: the Go `FlowSwitchClauseData` node is stored as `node` =
    // switch statement and `antecedents` = `[clause_start, clause_end]`
    // (raw values); see `FlowSwitchClauseData`.
    pub fn create_flow_switch_clause(
        &mut self,
        antecedent: FlowNodeId,
        switch_statement: Node,
        clause_start: i32,
        clause_end: i32,
    ) -> FlowNodeId {
        self.set_flow_node_referenced(antecedent);
        let result = self.new_flow_node_ex(FlowFlags::SWITCH_CLAUSE, switch_statement, antecedent);
        self.flow_mut(result).antecedents = vec![
            FlowNodeId(u64::from(clause_start as u32)),
            FlowNodeId(u64::from(clause_end as u32)),
        ];
        result
    }

    // Go: binder/binder.go:514 createFlowCall
    pub fn create_flow_call(&mut self, antecedent: FlowNodeId, node: Node) -> FlowNodeId {
        self.set_flow_node_referenced(antecedent);
        self.has_flow_effects = true;
        self.new_flow_node_ex(FlowFlags::CALL, node, antecedent)
    }

    // Go: binder/binder.go:520 newFlowList
    //
    // PORT: `*ast.FlowList` is a `Vec<FlowNodeId>`; nil is empty.
    pub fn new_flow_list(&mut self, head: FlowNodeId, tail: &[FlowNodeId]) -> Vec<FlowNodeId> {
        let mut result = Vec::with_capacity(tail.len() + 1);
        result.push(head);
        result.extend_from_slice(tail);
        result
    }

    // Go: binder/binder.go:527 combineFlowLists
    pub fn combine_flow_lists(
        &mut self,
        head: &[FlowNodeId],
        tail: &[FlowNodeId],
    ) -> Vec<FlowNodeId> {
        if head.is_empty() {
            return tail.to_vec();
        }
        let rest = self.combine_flow_lists(&head[1..], tail);
        self.new_flow_list(head[0], &rest)
    }

    // Go: binder/binder.go:534 newSingleDeclaration
    // PORT: returns `Declarations`, which holds one node inline, so no list
    // is allocated.
    pub fn new_single_declaration(&mut self, declaration: Node) -> Declarations {
        let mut declarations = Declarations::default();
        declarations.push(declaration);
        declarations
    }

    // Go: binder/binder.go:538 setFlowNodeReferenced
    //
    // PORT: a package function in Go; a `Binder` method here because flow
    // nodes live in `Binder::flow_nodes` while binding.
    pub fn set_flow_node_referenced(&mut self, flow: FlowNodeId) {
        let flow = self.flow_mut(flow);
        // On first reference we set the Referenced flag, thereafter we set the Shared flag
        if !flow.flags.intersects(FlowFlags::REFERENCED) {
            flow.flags |= FlowFlags::REFERENCED;
        } else {
            flow.flags |= FlowFlags::SHARED;
        }
    }

    // Go: binder/binder.go:547 addAntecedent
    pub fn add_antecedent(&mut self, label: FlowNodeId, antecedent: FlowNodeId) {
        if self
            .flow(antecedent)
            .flags
            .intersects(FlowFlags::UNREACHABLE)
        {
            return;
        }
        // If antecedent isn't already on the Antecedents list, add it to the end of the list
        if self.flow(label).antecedents.contains(&antecedent) {
            return;
        }
        self.flow_mut(label).antecedents.push(antecedent);
        self.set_flow_node_referenced(antecedent);
    }

    // Go: binder/binder.go:567 finishFlowLabel
    pub fn finish_flow_label(&mut self, label: FlowNodeId) -> FlowNodeId {
        let antecedents = &self.flow(label).antecedents;
        if antecedents.is_empty() {
            return self.unreachable_flow;
        }
        if antecedents.len() == 1 {
            return antecedents[0];
        }
        label
    }
}

impl Binder {
    // Go: binder/binder.go:577 bind
    pub fn bind(&mut self, node: Node) -> bool {
        if node.is_nil() {
            return false;
        }
        // Even though in the AST the jsdoc @typedef node belongs to the current node,
        // its symbol might be in the same scope with the current node's symbol. Consider:
        //
        //     /** @typedef {string | number} MyType */
        //     function foo();
        //
        // Here the current node is "foo", which is a container, but the scope of "MyType" should
        // not be inside "foo". Therefore we always bind @typedef before bind the parent node,
        // and skip binding this tag later when binding all the other jsdoc tags.

        // First we bind declaration nodes to a symbol if possible. We'll both create a symbol
        // and then potentially add the symbol to an appropriate symbol table. Possible
        // destination symbol tables are:
        //
        //  1) The 'exports' table of the current container's symbol.
        //  2) The 'members' table of the current container's symbol.
        //  3) The 'locals' table of the current container.
        //
        // However, not all symbols will end up in any of these tables. 'Anonymous' symbols
        // (like TypeLiterals for example) will not be put in any table.
        // PORT: the kind is read once. The binder never changes a node's kind.
        // PERF: query Q7-3. The property, method and accessor arms load the
        // node data once (`parsed_node_data`) and pass it on, so their field reads
        // (`postfix_token`, `modifiers`, `name`) skip the node lookup.
        let kind = node.kind();
        // PERF: binderview1. The fields of a BinaryExpression, read once in
        // its arm below for the arm and for `bind_children_of_kind`.
        let mut binary = None;
        match kind {
            SyntaxKind::Identifier => {
                let flow = self.current_flow;
                self.node_bind.set_flow_node(node.node_id().index(), flow);
                self.check_contextual_identifier(node);
            }
            SyntaxKind::ThisKeyword | SyntaxKind::SuperKeyword => {
                if kind == SyntaxKind::ThisKeyword {
                    self.seen_this_keyword = true;
                }
                let flow = self.current_flow;
                self.node_bind.set_flow_node(node.node_id().index(), flow);
            }
            SyntaxKind::QualifiedName => {
                if self.current_flow.is_some() && is_part_of_type_query(node) {
                    let flow = self.current_flow;
                    self.node_bind.set_flow_node(node.node_id().index(), flow);
                }
            }
            SyntaxKind::MetaProperty => {
                let flow = self.current_flow;
                self.node_bind.set_flow_node(node.node_id().index(), flow);
            }
            SyntaxKind::PrivateIdentifier => {
                self.check_private_identifier(node);
            }
            SyntaxKind::PropertyAccessExpression | SyntaxKind::ElementAccessExpression => {
                if self.current_flow.is_some() && is_narrowable_reference(node) {
                    let flow = self.current_flow;
                    self.set_flow_node(node, flow);
                }
            }
            SyntaxKind::BinaryExpression => {
                let bin = self.binary_view(node);
                match get_binary_assignment_declaration_kind(&bin) {
                    JSDeclarationKind::MODULE_EXPORTS => self.bind_module_exports_assignment(node),
                    JSDeclarationKind::EXPORTS_PROPERTY => {
                        self.bind_exports_or_object_define_property(node)
                    }
                    JSDeclarationKind::PROPERTY => self.bind_expando_property_assignment(node),
                    JSDeclarationKind::THIS_PROPERTY => self.bind_this_property_assignment(node),
                    _ => {}
                }
                self.check_strict_mode_binary_expression(node, &bin);
                binary = Some(bin);
            }
            SyntaxKind::CatchClause => {
                self.check_strict_mode_catch_clause(node);
            }
            SyntaxKind::DeleteExpression => {
                self.check_strict_mode_delete_expression(node);
            }
            SyntaxKind::PostfixUnaryExpression => {
                self.check_strict_mode_postfix_unary_expression(node);
            }
            SyntaxKind::PrefixUnaryExpression => {
                self.check_strict_mode_prefix_unary_expression(node);
            }
            SyntaxKind::WithStatement => {
                self.check_strict_mode_with_statement(node);
            }
            SyntaxKind::LabeledStatement => {
                self.check_strict_mode_labeled_statement(node);
            }
            SyntaxKind::ThisType => {
                self.seen_this_keyword = true;
            }
            SyntaxKind::TypeParameter => {
                self.bind_type_parameter(node);
            }
            SyntaxKind::Parameter => {
                self.bind_parameter(node);
            }
            SyntaxKind::VariableDeclaration => {
                self.bind_variable_declaration_or_binding_element(node);
            }
            SyntaxKind::BindingElement => {
                let flow = self.current_flow;
                self.node_bind.set_flow_node(node.node_id().index(), flow);
                self.bind_variable_declaration_or_binding_element(node);
            }
            SyntaxKind::PropertyDeclaration | SyntaxKind::PropertySignature => {
                self.bind_property_worker(node, parsed_node_data(node));
            }
            SyntaxKind::PropertyAssignment | SyntaxKind::ShorthandPropertyAssignment => {
                self.bind_property_or_method_or_accessor(
                    node,
                    parsed_node_data(node),
                    SymbolFlags::PROPERTY,
                    SymbolFlags::PROPERTY_EXCLUDES,
                );
            }
            SyntaxKind::EnumMember => {
                self.bind_property_or_method_or_accessor(
                    node,
                    parsed_node_data(node),
                    SymbolFlags::ENUM_MEMBER,
                    SymbolFlags::ENUM_MEMBER_EXCLUDES,
                );
            }
            SyntaxKind::CallSignature
            | SyntaxKind::ConstructSignature
            | SyntaxKind::IndexSignature => {
                self.declare_symbol_and_add_to_symbol_table(
                    node,
                    SymbolFlags::SIGNATURE,
                    SymbolFlags::NONE,
                );
            }
            SyntaxKind::MethodDeclaration | SyntaxKind::MethodSignature => {
                let d = parsed_node_data(node);
                // Go `ast.IsObjectLiteralMethod(node)` with the kind known.
                let excludes = if kind == SyntaxKind::MethodDeclaration
                    && node.parent().kind() == SyntaxKind::ObjectLiteralExpression
                {
                    SymbolFlags::VALUE
                } else {
                    SymbolFlags::METHOD_EXCLUDES
                };
                self.bind_property_or_method_or_accessor(
                    node,
                    d,
                    SymbolFlags::METHOD | get_optional_symbol_flag_for_node(node, d),
                    excludes,
                );
            }
            SyntaxKind::FunctionDeclaration => {
                self.bind_function_declaration(node);
            }
            SyntaxKind::Constructor => {
                self.declare_symbol_and_add_to_symbol_table(
                    node,
                    SymbolFlags::CONSTRUCTOR,
                    SymbolFlags::NONE,
                );
            }
            SyntaxKind::GetAccessor => {
                self.bind_property_or_method_or_accessor(
                    node,
                    parsed_node_data(node),
                    SymbolFlags::GET_ACCESSOR,
                    SymbolFlags::GET_ACCESSOR_EXCLUDES,
                );
            }
            SyntaxKind::SetAccessor => {
                self.bind_property_or_method_or_accessor(
                    node,
                    parsed_node_data(node),
                    SymbolFlags::SET_ACCESSOR,
                    SymbolFlags::SET_ACCESSOR_EXCLUDES,
                );
            }
            SyntaxKind::FunctionType | SyntaxKind::ConstructorType => {
                self.bind_function_or_constructor_type(node);
            }
            SyntaxKind::TypeLiteral | SyntaxKind::MappedType => {
                self.bind_anonymous_declaration(
                    node,
                    SymbolFlags::TYPE_LITERAL,
                    INTERNAL_SYMBOL_NAME_TYPE,
                );
            }
            SyntaxKind::ObjectLiteralExpression => {
                self.bind_anonymous_declaration(
                    node,
                    SymbolFlags::OBJECT_LITERAL,
                    INTERNAL_SYMBOL_NAME_OBJECT,
                );
            }
            SyntaxKind::FunctionExpression | SyntaxKind::ArrowFunction => {
                self.bind_function_expression(node);
            }
            SyntaxKind::ClassExpression | SyntaxKind::ClassDeclaration => {
                self.bind_class_like_declaration(node);
            }
            SyntaxKind::InterfaceDeclaration => {
                self.bind_block_scoped_declaration(
                    node,
                    SymbolFlags::INTERFACE,
                    SymbolFlags::INTERFACE_EXCLUDES,
                );
            }
            SyntaxKind::CallExpression => {
                match get_assignment_declaration_kind(node) {
                    JSDeclarationKind::OBJECT_DEFINE_PROPERTY_VALUE => {
                        self.bind_expando_property_assignment(node)
                    }
                    JSDeclarationKind::OBJECT_DEFINE_PROPERTY_EXPORTS => {
                        self.bind_exports_or_object_define_property(node)
                    }
                    _ => {}
                }
                if is_in_js_file(node) {
                    self.bind_call_expression(node);
                }
            }
            SyntaxKind::TypeAliasDeclaration => {
                self.bind_block_scoped_declaration(
                    node,
                    SymbolFlags::TYPE_ALIAS,
                    SymbolFlags::TYPE_ALIAS_EXCLUDES,
                );
            }
            SyntaxKind::JsTypeAliasDeclaration => {
                // Top-level JSTypeAliasDeclaration nodes are processed in bindContainer
                if !is_source_file(self.block_scope_container) {
                    self.bind_block_scoped_declaration(
                        node,
                        SymbolFlags::TYPE_ALIAS,
                        SymbolFlags::TYPE_ALIAS_EXCLUDES,
                    );
                }
            }
            SyntaxKind::EnumDeclaration => {
                self.bind_enum_declaration(node);
            }
            SyntaxKind::ModuleDeclaration => {
                self.bind_module_declaration(node);
            }
            SyntaxKind::ImportEqualsDeclaration
            | SyntaxKind::NamespaceImport
            | SyntaxKind::ImportSpecifier
            | SyntaxKind::ExportSpecifier => {
                self.declare_symbol_and_add_to_symbol_table(
                    node,
                    SymbolFlags::ALIAS,
                    SymbolFlags::ALIAS_EXCLUDES,
                );
            }
            SyntaxKind::NamespaceExportDeclaration => {
                self.bind_namespace_export_declaration(node);
            }
            SyntaxKind::ImportClause => {
                self.bind_import_clause(node);
            }
            SyntaxKind::ExportDeclaration => {
                self.bind_export_declaration(node);
            }
            SyntaxKind::ExportAssignment => {
                self.bind_export_assignment(node);
            }
            SyntaxKind::SourceFile => {
                self.bind_source_file_if_external_module();
            }
            SyntaxKind::JsxAttributes => {
                self.bind_jsx_attributes(node);
            }
            SyntaxKind::JsxAttribute => {
                self.bind_jsx_attribute(
                    node,
                    SymbolFlags::PROPERTY,
                    SymbolFlags::PROPERTY_EXCLUDES,
                );
            }
            _ => {}
        }
        // Then we recurse into the children of the node to bind them as well. For certain
        // symbols we do specialized work when we recurse. For example, we'll keep track of
        // the current 'container' node when it changes. This helps us know which symbol table
        // a local should go into for example. Since terminal nodes are known not to have
        // children, as an optimization we don't process those.
        let mut this_node_or_any_subnodes_has_error = self
            .node_flags(node)
            .intersects(NodeFlags::THIS_NODE_HAS_ERROR);
        if (kind as u16) > (SyntaxKind::LAST_TOKEN as u16) {
            let save_seen_parse_error = self.seen_parse_error;
            self.seen_parse_error = false;
            let container_flags = get_container_flags_of_kind(node, kind);
            if container_flags == ContainerFlags::NONE {
                self.bind_children_of_kind(node, kind, binary.as_ref());
            } else {
                self.bind_container(node, container_flags);
            }
            if self.seen_parse_error {
                this_node_or_any_subnodes_has_error = true;
            }
            self.seen_parse_error = save_seen_parse_error;
        }
        if this_node_or_any_subnodes_has_error {
            self.add_node_flags(node, NodeFlags::THIS_NODE_OR_ANY_SUB_NODES_HAS_ERROR);
            self.seen_parse_error = true;
        }
        false
    }

    // Go: binder/binder.go:752 bindPropertyWorker
    // PERF: query Q7-3. `d` is the data of `node`, loaded once by `bind`.
    pub fn bind_property_worker(&mut self, node: Node, d: LoadedData) {
        // Go `ast.IsAutoAccessorPropertyDeclaration(node)` on the loaded data.
        let is_auto_accessor = is_property_declaration(node)
            && has_syntactic_modifier_in(node, d, ModifierFlags::ACCESSOR);
        let includes = if is_auto_accessor {
            SymbolFlags::ACCESSOR
        } else {
            SymbolFlags::PROPERTY
        };
        let excludes = if is_auto_accessor {
            SymbolFlags::ACCESSOR_EXCLUDES
        } else {
            SymbolFlags::PROPERTY_EXCLUDES
        };
        self.bind_property_or_method_or_accessor(
            node,
            d,
            includes | get_optional_symbol_flag_for_node(node, d),
            excludes,
        );
    }

    // Go: binder/binder.go:759 bindSourceFileIfExternalModule
    pub fn bind_source_file_if_external_module(&mut self) {
        let file = self.file;
        self.set_export_context_flag(file);
        if self.file_is_external_or_common_js_module() {
            self.bind_source_file_as_external_module();
        } else if is_json_source_file(file) {
            self.bind_source_file_as_external_module();
            // Create symbol equivalent for the module.exports = {}
            let original_symbol = self.node_symbol(file);
            let exports = get_exports(&mut self.symbols, original_symbol);
            self.declare_symbol(
                exports,
                original_symbol,
                file,
                SymbolFlags::PROPERTY,
                SymbolFlags::ALL,
            );
            self.set_node_symbol(file, original_symbol);
        }
    }

    // Go: binder/binder.go:772 bindSourceFileAsExternalModule
    pub fn bind_source_file_as_external_module(&mut self) {
        let name = format!(
            "\"{}\"",
            remove_file_extension(source_file_file_name(self.file))
        );
        self.bind_anonymous_declaration(self.file, SymbolFlags::VALUE_MODULE, &name);
    }

    // Go: binder/binder.go:776 bindModuleDeclaration
    pub fn bind_module_declaration(&mut self, node: Node) {
        self.set_export_context_flag(node);
        if is_ambient_module(node) {
            if has_syntactic_modifier(node, ModifierFlags::EXPORT) {
                self.error_on_first_token(
                    node,
                    diag::X_export_modifier_cannot_be_applied_to_ambient_modules_and_module_augmentations_since_they_are_always_visible,
                    vec![],
                );
            }
            if is_module_augmentation_external(node) {
                self.declare_module_symbol(node);
            } else {
                let name = node.name();
                let symbol = self.declare_symbol_and_add_to_symbol_table(
                    node,
                    SymbolFlags::VALUE_MODULE,
                    SymbolFlags::VALUE_MODULE_EXCLUDES,
                );

                if is_string_literal(name) {
                    let attributes = node.attributes();
                    let pattern = try_parse_pattern(name.text());
                    if !pattern.is_valid() {
                        // An invalid pattern - must have multiple wildcards.
                        self.error_on_first_token(
                            name,
                            diag::Pattern_0_can_have_at_most_one_Asterisk_character,
                            args![name.text()],
                        );
                    } else if pattern.star_index >= 0 {
                        let star = pattern.star_index as usize;
                        self.file_bind
                            .pattern_ambient_modules
                            .push(PatternAmbientModule {
                                pattern_prefix: pattern.text[..star].to_string(),
                                pattern_suffix: pattern.text[star + 1..].to_string(),
                                symbol,
                            });
                    } else if attributes.is_some() {
                        self.error_on_node(
                            name,
                            diag::An_ambient_module_declaration_with_import_attributes_must_use_a_pattern_name_with_an_Asterisk_character,
                            args![],
                        );
                    }
                }
            }
        } else {
            let state = self.declare_module_symbol(node);
            if state != ModuleInstanceState::NON_INSTANTIATED {
                let symbol = self.node_symbol(node);
                let symbol_flags = self.sym(symbol).flags;
                // if module was already merged with some function, class or non-const enum, treat it as non-const-enum-only
                let const_enum_only_module = !symbol_flags
                    .intersects(SymbolFlags::FUNCTION | SymbolFlags::CLASS | SymbolFlags::REGULAR_ENUM)
                    // Current must be `const enum` only
                    && state == ModuleInstanceState::CONST_ENUM_ONLY
                    // Can't have been set to 'false' in a previous merged symbol. ('undefined' OK)
                    && !self.not_const_enum_only_modules.contains(&symbol);
                if const_enum_only_module {
                    self.sym_mut(symbol).flags |= SymbolFlags::CONST_ENUM_ONLY_MODULE;
                } else {
                    let flags = self.sym(symbol).flags;
                    self.sym_mut(symbol).flags = flags.without(SymbolFlags::CONST_ENUM_ONLY_MODULE);
                    self.not_const_enum_only_modules.insert(symbol);
                }
            }
        }
    }

    // Go: binder/binder.go:821 declareModuleSymbol
    pub fn declare_module_symbol(&mut self, node: Node) -> ModuleInstanceState {
        let state = get_module_instance_state(node);
        let instantiated = state != ModuleInstanceState::NON_INSTANTIATED;
        self.declare_symbol_and_add_to_symbol_table(
            node,
            if instantiated {
                SymbolFlags::VALUE_MODULE
            } else {
                SymbolFlags::NAMESPACE_MODULE
            },
            if instantiated {
                SymbolFlags::VALUE_MODULE_EXCLUDES
            } else {
                SymbolFlags::NAMESPACE_MODULE_EXCLUDES
            },
        );
        state
    }

    // Go: binder/binder.go:828 bindNamespaceExportDeclaration
    pub fn bind_namespace_export_declaration(&mut self, node: Node) {
        if !node.modifiers().is_nil() {
            self.error_on_node(node, diag::Modifiers_cannot_appear_here, vec![]);
        }
        let parent = node.parent();
        if !is_source_file(parent) {
            self.error_on_node(
                node,
                diag::Global_module_exports_may_only_appear_at_top_level,
                vec![],
            );
        } else if !is_external_module(parent) {
            self.error_on_node(
                node,
                diag::Global_module_exports_may_only_appear_in_module_files,
                vec![],
            );
        } else if !with_source_file_info(parent, |info| info.is_declaration_file) {
            self.error_on_node(
                node,
                diag::Global_module_exports_may_only_appear_in_declaration_files,
                vec![],
            );
        } else {
            let global_exports = self.get_global_exports();
            let file_symbol = self.node_symbol(self.file);
            self.declare_symbol(
                global_exports,
                file_symbol,
                node,
                SymbolFlags::ALIAS,
                SymbolFlags::ALIAS_EXCLUDES,
            );
        }
    }

    // Go: binder/binder.go:844 bindImportClause
    pub fn bind_import_clause(&mut self, node: Node) {
        if node.name().is_some() {
            self.declare_symbol_and_add_to_symbol_table(
                node,
                SymbolFlags::ALIAS,
                SymbolFlags::ALIAS_EXCLUDES,
            );
        }
    }

    // Go: binder/binder.go:850 bindExportDeclaration
    pub fn bind_export_declaration(&mut self, node: Node) {
        let export_clause = node.export_clause();
        let container_symbol = self.node_symbol(self.container);
        if container_symbol.is_nil() {
            // Export * in some sort of block construct
            let name = self.get_declaration_name(node);
            self.bind_anonymous_declaration(node, SymbolFlags::EXPORT_STAR, &name);
        } else if export_clause.is_nil() {
            // All export * declarations are collected in an __export symbol
            let exports = self.get_container_exports(self.container, container_symbol);
            self.declare_symbol(
                exports,
                container_symbol,
                node,
                SymbolFlags::EXPORT_STAR,
                SymbolFlags::NONE,
            );
        } else if is_namespace_export(export_clause) {
            let exports = self.get_container_exports(self.container, container_symbol);
            self.declare_symbol(
                exports,
                container_symbol,
                export_clause,
                SymbolFlags::ALIAS,
                SymbolFlags::ALIAS_EXCLUDES,
            );
        }
    }

    // Go: binder/binder.go:863 bindExportAssignment
    pub fn bind_export_assignment(&mut self, node: Node) {
        let container = self.container;
        let container_symbol = self.node_symbol(container);
        if container_symbol.is_nil() && is_export_assignment(node) {
            // Incorrect export assignment in some sort of block construct
            let name = self.get_declaration_name(node);
            self.bind_anonymous_declaration(node, SymbolFlags::VALUE, &name);
        } else {
            // If there is an `export default x;` alias declaration, can't `export default` anything else.
            // (In contrast, you can still have `export default function f() {}` and `export default interface I {}`.)
            let flags = if expression_is_alias(node.expression()) {
                SymbolFlags::ALIAS
            } else {
                SymbolFlags::PROPERTY
            };
            let exports = self.get_container_exports(container, container_symbol);
            let symbol =
                self.declare_symbol(exports, container_symbol, node, flags, SymbolFlags::ALL);
            if node.is_export_equals() {
                // Ensure export assignments have a ValueDeclaration set.
                set_value_declaration(&mut self.symbols, symbol, node);
            }
        }
    }

    // Go: binder/binder.go:880 bindJsxAttributes
    pub fn bind_jsx_attributes(&mut self, node: Node) {
        self.bind_anonymous_declaration(
            node,
            SymbolFlags::OBJECT_LITERAL,
            INTERNAL_SYMBOL_NAME_JSX_ATTRIBUTES,
        );
    }

    // Go: binder/binder.go:884 bindJsxAttribute
    pub fn bind_jsx_attribute(
        &mut self,
        node: Node,
        symbol_flags: SymbolFlags,
        symbol_excludes: SymbolFlags,
    ) {
        self.declare_symbol_and_add_to_symbol_table(node, symbol_flags, symbol_excludes);
    }

    // Go: binder/binder.go:888 setExportContextFlag
    pub fn set_export_context_flag(&mut self, node: Node) {
        // A declaration source file or ambient module declaration that contains no export declarations (but possibly regular
        // declarations with export modifiers) is an export context in which declarations are implicitly exported.
        if self.node_flags(node).intersects(NodeFlags::AMBIENT)
            && !self.has_export_declarations(node)
        {
            self.add_node_flags(node, NodeFlags::EXPORT_CONTEXT);
        } else {
            self.remove_node_flags(node, NodeFlags::EXPORT_CONTEXT);
        }
    }

    // Go: binder/binder.go:898 hasExportDeclarations
    pub fn has_export_declarations(&self, node: Node) -> bool {
        let mut statements: Vec<Node> = Vec::new();
        match node.kind() {
            SyntaxKind::SourceFile => {
                statements = node.statements().to_vec();
            }
            SyntaxKind::ModuleDeclaration => {
                let body = node.body();
                if body.is_some() && is_module_block(body) {
                    statements = body.statements().to_vec();
                }
            }
            _ => {}
        }
        statements
            .iter()
            .any(|&s| is_export_declaration(s) || is_export_assignment(s))
    }
}

// PERF: size hints for a new members, exports or locals table of a
// container, from its member or statement count, so that filling the table
// does not grow it or rebuild its index. A hint only sizes the table: entry
// order does not change. 0 gives an unsized table.

/// About the number of members of `container`.
fn members_size_hint(container: Node) -> usize {
    match container.kind() {
        SyntaxKind::ClassDeclaration
        | SyntaxKind::ClassExpression
        | SyntaxKind::InterfaceDeclaration
        | SyntaxKind::TypeLiteral => container.members().len(),
        SyntaxKind::ObjectLiteralExpression | SyntaxKind::JsxAttributes => {
            container.properties().len()
        }
        _ => 0,
    }
}

/// About the number of exports of `container`.
fn exports_size_hint(container: Node) -> usize {
    match container.kind() {
        SyntaxKind::EnumDeclaration => container.members().len(),
        _ => statement_count_hint(container),
    }
}

/// About the number of locals of `container`.
fn locals_size_hint(container: Node) -> usize {
    statement_count_hint(container)
}

/// The statement count of a source file or of a namespace body, else 0.
fn statement_count_hint(container: Node) -> usize {
    match container.kind() {
        SyntaxKind::SourceFile => container.statements().len(),
        SyntaxKind::ModuleDeclaration => {
            let body = container.body();
            if body.is_some() && body.kind() == SyntaxKind::ModuleBlock {
                body.statements().len()
            } else {
                0
            }
        }
        _ => 0,
    }
}

// Go: core/pattern.go:5 Pattern
//
// PORT: private copy of `core.Pattern` / `core.TryParsePattern` for
// `bindModuleDeclaration`; the contract names no crate module for Go `core`
// helpers. `PatternAmbientModule` stores the prefix and suffix instead.
struct Pattern {
    text: String,
    star_index: i32, // -1 for exact match
}

impl Pattern {
    // Go: core/pattern.go:18 IsValid
    fn is_valid(&self) -> bool {
        self.star_index == -1 || (self.star_index as usize) < self.text.len()
    }
}

// Go: core/pattern.go:10 TryParsePattern
fn try_parse_pattern(pattern: &str) -> Pattern {
    let star_index = pattern.find('*');
    match star_index {
        None => Pattern {
            text: pattern.to_string(),
            star_index: -1,
        },
        Some(i) if !pattern[i + 1..].contains('*') => Pattern {
            text: pattern.to_string(),
            star_index: i as i32,
        },
        Some(_) => Pattern {
            text: String::new(),
            star_index: 0,
        },
    }
}

// Go: tspath/extension.go:45 RemoveFileExtension
//
// PORT: private copy; the contract names no crate module for Go `tspath`.
fn remove_file_extension(path: &str) -> &str {
    // Go: tspath/extension.go:43 extensionsToRemove
    const EXTENSIONS_TO_REMOVE: [&str; 12] = [
        ".d.ts", ".d.mts", ".d.cts", ".mjs", ".mts", ".cjs", ".cts", ".ts", ".js", ".tsx", ".jsx",
        ".json",
    ];
    // Remove any known extension even if it has more than one dot
    for ext in EXTENSIONS_TO_REMOVE {
        if let Some(stripped) = path.strip_suffix(ext) {
            return stripped;
        }
    }
    path
}
