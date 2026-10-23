//! Port of typescript-go `internal/binder/binder.go` lines 1-910.
//!
//! PORT: Go stores binder output on AST nodes and on `ast.SourceFile`. Here
//! the binder writes the symbol, the added flags and the flow node of a
//! node into its node record (`ast/store.rs`) while it runs, as Go writes
//! the node, and keeps the other node fields, `file_bind` and `flow_nodes`
//! in `Binder` (`NodeBindRecords`). `BoundFile::install` moves those into
//! the `GoFile` `OnceCell`s at the end.
//! Binder code must read and write node binder data through the `Binder`
//! helpers below (`node_symbol`, `set_node_symbol`, `node_flags`,
//! `get_locals`, `flow`, `flow_mut`, ...), not through `Node::symbol()` and
//! friends: before the install a record can hold the ids of the file's own
//! arena, and the other fields are not in the `GoFile` yet.

use crate::astdata::NodeData;
use crate::prelude::*;

// Go: binder/binder.go:44 ExpandoAssignmentInfo
#[derive(Clone, Copy, Debug, Default)]
pub struct ExpandoAssignmentInfo {
    pub node: Node,
    pub container: Node,
    pub block_scope_container: Node,
}

// Go: binder/binder.go:50 Binder
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
    /// Rust-only: the binder data of the nodes of `file`, in their node
    /// records (Go writes it into the nodes), indexed by `NodeId::index()`.
    /// It holds the parser flags of `file`, whose guard pins a freeable
    /// file version while the binder runs.
    pub node_bind: NodeBindRecords,
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
    /// Rust-only: the work stack of `bind_binary_expression_flow` (issue
    /// #99). Go binds each operand that is a BinaryExpression by recursion;
    /// here it gets a frame. Empty between binds.
    pub binary_frames: Vec<super::binder_p3::BindBinaryFrame>,
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
// binding ends. The binder writes the node records while it binds
// (`NodeBindRecords`), and the install stores the rest of its output in the
// file's `GoFile` `OnceCell`s (Go `file.BindOnce`). A file that is already
// bound is skipped. The caller publishes what the file added to `symbols`
// for the checkers' copies (`Lineage::add_file`).
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

/// The binder output of the nodes of the file being bound, written while it
/// binds, as Go writes it into the nodes (binder.go). The symbol, the added
/// flags and the flow node of a node go into its node record (`records`,
/// `ast::BindRecords`). The other fields of a node (local symbol, locals,
/// next container, end and return flow node) go into an extras entry, made
/// on the first write of one of them; the `bind` word of its record then
/// holds the index + 1 of the entry with `ast::BIND_EXTRA`, and the entry
/// holds its flow node (the layout of `ast::bind_store_records`). The
/// binder reads them back from there, as Go reads the node.
///
/// A bind into a new arena (`bind_source_file_detached` from a parallel or
/// `--singleThreaded` bind) writes the ids of that arena. `symbol_slots`
/// lists the slots whose record got a symbol, so `BoundFile::remap` moves
/// only those.
///
/// Until the install, a drop resets the records of the file
/// (`BindRecords::reset`): a panic in the bind, or a bind that
/// `program::bind_files_parallel` drops (it made thread-local state, or a
/// file before it failed). A later bind of the file then finds the records
/// as the parse left them.
// PERF: bindonce2. R214 kept this data in two zeroed u32 per node and a
// 48-byte entry per node with data (`NodeBindBuilder`), and the install
// walked every slot to write the records: 7.1% of VS Code st bind cycles
// (perfscan5).
pub struct NodeBindRecords {
    file: usize,
    records: crate::ast::BindRecords,
    /// The parser flags of the file (`GoFile::parser_flags`), which
    /// `remove_node_flags` and the reset keep. The guard pins a freeable
    /// file version until the install or the reset.
    parser_flags: FileRef<[NodeFlags]>,
    extras: Vec<NodeBindExtra>,
    symbol_slots: Vec<u32>,
    /// True from the start of the bind to the install.
    pending: bool,
    /// Test builds only: the bind panics when it has written this many
    /// symbols (`TEST_PANIC_IN_BIND`).
    #[cfg(test)]
    panic_at_symbols: Option<usize>,
}

/// Test builds only: `Some((file, symbols))` makes the next bind of file
/// index `file` panic in the binder when it has written `symbols` symbols
/// into the node records (`NodeBindRecords::set_symbol`). The bind that
/// takes it sets it back to `None`, so only that bind panics.
#[cfg(test)]
pub(crate) static TEST_PANIC_IN_BIND: std::sync::Mutex<Option<(usize, usize)>> =
    std::sync::Mutex::new(None);

impl Default for NodeBindRecords {
    fn default() -> Self {
        NodeBindRecords {
            file: 0,
            records: crate::ast::BindRecords::default(),
            parser_flags: FileRef::Static(&[]),
            extras: Vec::new(),
            symbol_slots: Vec::new(),
            pending: false,
            #[cfg(test)]
            panic_at_symbols: None,
        }
    }
}

impl Drop for NodeBindRecords {
    fn drop(&mut self) {
        if self.pending {
            self.records.reset(&self.parser_flags);
        }
    }
}

impl NodeBindRecords {
    /// The records of published file `file`, whose parser flags are
    /// `parser_flags`, for a bind. Panics when the file has binder output:
    /// the bind would write over it.
    fn start(file: usize, parser_flags: FileRef<[NodeFlags]>) -> Self {
        assert!(
            crate::ast::go_file(file).node_bind.get().is_none(),
            "file already bound"
        );
        let records = crate::ast::bind_records(file);
        debug_assert_eq!(records.len(), parser_flags.len());
        let slots = records.len();
        NodeBindRecords {
            file,
            records,
            parser_flags,
            // About one node in 16 has extras (locals containers, exported
            // declarations, function-like nodes) and one in 8 a symbol, so
            // the lists do not grow by doubling. Capacity only.
            extras: Vec::with_capacity(slots / 16 + 1),
            symbol_slots: Vec::with_capacity(slots / 8 + 1),
            pending: true,
            #[cfg(test)]
            panic_at_symbols: {
                let mut hook = TEST_PANIC_IN_BIND
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                match *hook {
                    Some((hook_file, symbols)) if hook_file == file => {
                        *hook = None;
                        Some(symbols)
                    }
                    _ => None,
                }
            },
        }
    }

    /// Go `node.Symbol()` of node `index` (`NodeId::index()`).
    #[inline]
    fn symbol(&self, index: usize) -> SymbolId {
        self.records.symbol(index)
    }

    /// Go `node.DeclarationData().Symbol = symbol`.
    #[inline]
    fn set_symbol(&mut self, index: usize, symbol: SymbolId) {
        // A slot that went from a symbol back to nil and on to a new one
        // would be listed twice and move twice. The binder never writes nil.
        assert!(symbol.is_some(), "binder wrote a nil symbol");
        if self.records.set_symbol(index, symbol) {
            self.symbol_slots
                .push(u32::try_from(index).expect("slot index"));
            #[cfg(test)]
            if self.panic_at_symbols == Some(self.symbol_slots.len()) {
                panic!(
                    "test: a bind panics after it wrote {} symbols",
                    self.symbol_slots.len()
                );
            }
        }
    }

    /// The extras entry of node `index`, or `None` for a node with none.
    #[inline]
    fn extra(&self, index: usize) -> Option<&NodeBindExtra> {
        let word = self.records.bind_word(index);
        (word & crate::ast::BIND_EXTRA != 0)
            .then(|| &self.extras[(word & !crate::ast::BIND_EXTRA) as usize - 1])
    }

    /// Runs `write` on the extras entry of node `index`, made with the
    /// flow node of its record on first use. A nil value (`nil`) makes no
    /// entry: R214 made one only for a node with a field that is not nil.
    fn write_extra(&mut self, index: usize, nil: bool, write: impl FnOnce(&mut NodeBindExtra)) {
        let word = self.records.bind_word(index);
        let entry = if word & crate::ast::BIND_EXTRA != 0 {
            (word & !crate::ast::BIND_EXTRA) as usize - 1
        } else if nil {
            return;
        } else {
            let flow_node = if word == 0 {
                FlowNodeId::NIL
            } else {
                FlowNodeId(((self.file as u64) << 32) | u64::from(word))
            };
            self.extras.push(NodeBindExtra {
                flow_node,
                ..NodeBindExtra::NONE
            });
            let extra = u32::try_from(self.extras.len()).expect("binder extras");
            assert_eq!(extra & crate::ast::BIND_EXTRA, 0, "binder extras");
            self.records
                .set_bind_word(index, crate::ast::BIND_EXTRA | extra);
            self.extras.len() - 1
        };
        write(&mut self.extras[entry]);
    }

    /// Go `node.FlowNodeData().FlowNode = flow` for node `index`.
    #[inline]
    pub fn set_flow_node(&mut self, index: usize, flow: FlowNodeId) {
        debug_assert!(
            flow.is_nil() || flow.file_index() == self.file,
            "flow node from another file"
        );
        let word = self.records.bind_word(index);
        if word & crate::ast::BIND_EXTRA != 0 {
            self.extras[(word & !crate::ast::BIND_EXTRA) as usize - 1].flow_node = flow;
        } else {
            // `bind_source_file_live` checks the flow node count.
            debug_assert_eq!(flow.0 as u32 & crate::ast::BIND_EXTRA, 0);
            self.records.set_bind_word(index, flow.0 as u32);
        }
    }

    /// Moves the ids of a bind into a new arena to their place in the
    /// program arena (`BoundFile::remap`): the symbols of the records that
    /// got one, and the ids in the extras.
    fn remap(&mut self, offsets: ArenaOffsets) {
        self.records
            .map_symbols(&self.symbol_slots, |symbol| offsets.symbol(symbol));
        for extra in &mut self.extras {
            extra.local_symbol = offsets.symbol(extra.local_symbol);
            extra.locals = offsets.table(extra.locals);
        }
    }

    /// Ends the bind for the install: the records stay, and the extras go to
    /// the file's `FileNodeBind`.
    fn finish(mut self) -> Vec<NodeBindExtra> {
        self.pending = false;
        std::mem::take(&mut self.extras)
    }

    /// The compact form of the data, as the lib bind snapshot keeps it: the
    /// data of each node read back from its record and extras entry. The
    /// added flags are the record flags that are not parser flags.
    fn compact(&self) -> NodeBindParts {
        let flow_file = (self.file as u64) << 32;
        let nodes = (0..self.records.len()).map(|index| {
            let (symbol, flags, word) = self.records.node_fields(index)?;
            let added_flags = flags.without(self.parser_flags[index]);
            let data = match self.extra(index) {
                Some(extra) => NodeBindData {
                    symbol,
                    local_symbol: extra.local_symbol,
                    locals: extra.locals,
                    next_container: extra.next_container,
                    flow_node: extra.flow_node,
                    end_flow_node: extra.end_flow_node,
                    return_flow_node: extra.return_flow_node,
                    added_flags,
                },
                None => NodeBindData {
                    symbol,
                    flow_node: if word == 0 {
                        FlowNodeId::NIL
                    } else {
                        FlowNodeId(flow_file | u64::from(word))
                    },
                    added_flags,
                    ..NodeBindData::default()
                },
            };
            (data != NodeBindData::default()).then_some(data)
        });
        NodeBindParts::new(nodes, self.records.len())
    }
}

/// The binder data of the nodes of one file as the bind hands it over
/// (`BoundFile`): the records that a live bind wrote, or the compact form
/// that the lib bind snapshot keeps, which the install writes into the
/// node records (`ast::bind_store_records`).
pub enum BoundNodes {
    Written(NodeBindRecords),
    Parts(NodeBindParts),
}

impl BoundNodes {
    /// The compact form, as the lib bind snapshot keeps it.
    #[must_use]
    pub fn compact(&self) -> std::borrow::Cow<'_, NodeBindParts> {
        match self {
            BoundNodes::Written(nodes) => std::borrow::Cow::Owned(nodes.compact()),
            BoundNodes::Parts(parts) => std::borrow::Cow::Borrowed(parts),
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
    // it was frozen, so the flow nodes grow with no copy. Capacity only:
    // the output does not change.
    let flow_nodes = match frozen_store_bind_estimate(file_index) {
        Some(flow_nodes) => Vec::with_capacity(flow_nodes),
        None => Vec::new(),
    };
    // From here a panic resets the records of the file (`NodeBindRecords`).
    let node_bind = NodeBindRecords::start(file_index, parser_flags);
    // PERF: bind D. About one symbol per 8 nodes and one table per 16 (lib
    // files have more), so the first arena chunk does not grow by doubling.
    // Capacity only.
    symbols.reserve_arena(node_count / 8 + 1, node_count / 16 + 1);
    let mut b = Binder {
        file,
        file_index,
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
    // A record's `bind` word holds the low half of a flow node id, without
    // `BIND_EXTRA` (R214 checked each record at the install).
    assert!(
        flow_nodes.len() < crate::ast::BIND_EXTRA as usize,
        "too many flow nodes in file {file_index}"
    );
    BoundFile {
        file,
        node_bind: BoundNodes::Written(node_bind),
        flow_nodes,
        file_bind,
    }
}

impl BoundFile {
    /// Stores the output in the file's `GoFile` `OnceLock`s (Go
    /// `file.BindOnce`). A live bind wrote the node records already
    /// (`NodeBindRecords`), so this moves its extras into the
    /// `FileNodeBind` of the file. A lib snapshot load writes the symbol,
    /// the flow node and the added flags of each node into its record here
    /// (`ast::bind_store_records`). The ids must be program ids already
    /// (`remap`). A parallel bind installs each file on its bind thread, as
    /// Go's `BindSourceFile` stores the output in the file on the goroutine
    /// that binds it (`program::bind_files_parallel`).
    pub fn install(self) {
        let file = self.file.file_index();
        let go_file = crate::ast::go_file(file);
        assert!(go_file.node_bind.get().is_none(), "file already bound");
        let extras = match self.node_bind {
            BoundNodes::Written(nodes) => nodes.finish(),
            BoundNodes::Parts(parts) => crate::ast::bind_store_records(
                file,
                parts
                    .nodes()
                    .map(|(index, entry, data)| (index, entry, Some(data), data.flow_node)),
            ),
        };
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
        match &mut self.node_bind {
            BoundNodes::Written(nodes) => nodes.remap(offsets),
            BoundNodes::Parts(parts) => {
                for data in parts.entries_mut() {
                    data.symbol = offsets.symbol(data.symbol);
                    data.local_symbol = offsets.symbol(data.local_symbol);
                    data.locals = offsets.table(data.locals);
                }
            }
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
        self.symbols.sym_full(symbol)
    }

    /// Go `symbol` field writes: `self.sym_mut(s).flags |= ...`.
    pub fn sym_mut(&mut self, symbol: SymbolId) -> &mut Symbol {
        self.symbols.sym_mut(symbol)
    }

    /// `NodeId::index()` of `node`, a node of the file being bound.
    #[inline]
    fn bind_index(&self, node: Node) -> usize {
        debug_assert!(node.is_some(), "nil node dereference");
        debug_assert_eq!(node.file_index(), self.file_index, "node from another file");
        node.node_id().index()
    }

    /// Go `node.Symbol()` while binding.
    #[inline]
    pub fn node_symbol(&self, node: Node) -> SymbolId {
        self.node_bind.symbol(self.bind_index(node))
    }

    /// Go `node.DeclarationData().Symbol = symbol`.
    #[inline]
    pub fn set_node_symbol(&mut self, node: Node, symbol: SymbolId) {
        let index = self.bind_index(node);
        self.node_bind.set_symbol(index, symbol);
    }

    /// Go `node.LocalSymbol()` while binding.
    pub fn node_local_symbol(&self, node: Node) -> SymbolId {
        let index = self.bind_index(node);
        self.node_bind
            .extra(index)
            .map_or(SymbolId::NIL, |extra| extra.local_symbol)
    }

    /// Go `node.ExportableData().LocalSymbol = symbol`.
    pub fn set_node_local_symbol(&mut self, node: Node, symbol: SymbolId) {
        let index = self.bind_index(node);
        self.node_bind
            .write_extra(index, symbol.is_nil(), |extra| extra.local_symbol = symbol);
    }

    /// Go `node.Locals()` while binding (nil when not created).
    pub fn node_locals(&self, node: Node) -> SymbolTable {
        let index = self.bind_index(node);
        self.node_bind
            .extra(index)
            .map_or(SymbolTable::NIL, |extra| extra.locals)
    }

    /// Go `node.LocalsContainerData().Locals = locals`.
    pub fn set_node_locals(&mut self, node: Node, locals: SymbolTable) {
        // U1 (d): `Node::locals` reads nil for other kinds.
        debug_assert!(is_locals_container(node), "locals on a {:?}", node.kind());
        let index = self.bind_index(node);
        self.node_bind
            .write_extra(index, locals.is_nil(), |extra| extra.locals = locals);
    }

    /// Go `ast.GetLocals(container)`: creates the locals table on first use.
    pub fn get_locals(&mut self, container: Node) -> SymbolTable {
        let locals = self.node_locals(container);
        if locals.is_some() {
            return locals;
        }
        let locals = self
            .symbols
            .new_table_with_capacity(locals_size_hint(container));
        self.set_node_locals(container, locals);
        locals
    }

    /// Go `LocalsContainerData().NextContainer = next`.
    pub fn set_next_container(&mut self, container: Node, next: Node) {
        let index = self.bind_index(container);
        self.node_bind
            .write_extra(index, next.is_nil(), |extra| extra.next_container = next);
    }

    /// Go `bodyData.EndFlowNode = flow` of a function-like or module node.
    pub fn set_end_flow_node(&mut self, node: Node, flow: FlowNodeId) {
        let index = self.bind_index(node);
        self.node_bind
            .write_extra(index, flow.is_nil(), |extra| extra.end_flow_node = flow);
    }

    /// Go `ReturnFlowNode = flow` of a constructor, function or class
    /// static block (`set_return_flow_node` checks the kind).
    pub fn set_node_return_flow_node(&mut self, node: Node, flow: FlowNodeId) {
        let index = self.bind_index(node);
        self.node_bind
            .write_extra(index, flow.is_nil(), |extra| extra.return_flow_node = flow);
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

    /// Go `node.Flags` while binding: parser flags plus the flags that the
    /// binder added so far, from the node record (`NodeBindRecords`).
    #[inline]
    pub fn node_flags(&self, node: Node) -> NodeFlags {
        debug_assert_eq!(node.file_index(), self.file_index, "node from another file");
        self.node_bind.records.flags(node.node_id().index())
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

    /// Go `node.Flags |= flags`. `flags` must be in `BINDER_ADDED_FLAGS`.
    #[inline]
    pub fn add_node_flags(&mut self, node: Node, flags: NodeFlags) {
        let index = self.bind_index(node);
        self.node_bind.records.add_flags(index, flags);
    }

    /// Go `node.Flags &^= flags`.
    ///
    /// PORT: only binder-added bits are cleared: a parser flag stays, so the
    /// reset of a bind that is not installed (`BindRecords::reset`) gives
    /// the parse state back. This clears what Go clears: Go clears
    /// ExportContext on a SourceFile or ModuleDeclaration (binder.go:893),
    /// ReachabilityAndEmitFlags and ContainsThis on a control flow container
    /// (:1554) and ContainsThis on an interface (:1600). The parser sets
    /// none of these bits on those nodes. Of these bits it sets only
    /// IdentifierIsInJSDocNamespace (the HasAsyncFunctions bit), on an
    /// Identifier (parser/jsdoc.go:1013), which is never a container.
    pub fn remove_node_flags(&mut self, node: Node, flags: NodeFlags) {
        let index = self.bind_index(node);
        let parser_flags = self.node_bind.parser_flags[index];
        debug_assert!(
            !flags.intersects(parser_flags),
            "binder clears parser flags {:#x} of {:?}",
            (flags & parser_flags).bits(),
            node.kind()
        );
        self.node_bind
            .records
            .clear_flags(index, flags.without(parser_flags));
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
        // PERF: parsefast1 item 2. The kind is read once (Go reads `Kind`).
        let kind = node.kind();
        // Go `ast.IsExportSpecifier(node)` (binder.go:153), inline. Its kind
        // at pin fed0bf24149f:
        // Go: ast/ast_generated.go:3015 IsExportSpecifier
        let is_default_export = has_syntactic_modifier_in(node, data, ModifierFlags::DEFAULT)
            || kind == SyntaxKind::ExportSpecifier
                && module_export_name_is_default(node.name_in(data));
        // The exported symbol for an export default function/class node is always named "default"
        let name: Name = if is_computed_name {
            Name::from(INTERNAL_SYMBOL_NAME_COMPUTED)
        } else if is_default_export && parent.is_some() {
            Name::from(INTERNAL_SYMBOL_NAME_DEFAULT)
        } else {
            self.get_declaration_name_of_kind(node, kind, data)
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
                // Go `newSymbol(SymbolFlagsNone, name)`, the table write, the
                // `SymbolFlagsReplaceableByMethod` flag, `addDeclarationToSymbol`
                // and the parent write below, in one symbol write.
                let flags = if is_replaceable_by_method {
                    SymbolFlags::REPLACEABLE_BY_METHOD
                } else {
                    SymbolFlags::NONE
                };
                symbol = self.new_declared_symbol(name, flags, node, includes, parent);
                self.symbols.set_slot(slot, symbol);
                return symbol;
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
        self.get_declaration_name_of_kind(node, node.kind(), d)
    }

    /// `get_declaration_name_in` for a caller that already read `node.kind()`
    /// (`kind`).
    // PERF: parsefast1 item 2. The node kind and the name kind are each read
    // once, as Go reads its `Kind` fields (a port kind read goes through
    // the node store).
    pub fn get_declaration_name_of_kind(
        &mut self,
        node: Node,
        kind: SyntaxKind,
        d: LoadedData,
    ) -> Name {
        if kind == SyntaxKind::ExportAssignment {
            return if node.is_export_equals() {
                Name::from(INTERNAL_SYMBOL_NAME_EXPORT_EQUALS)
            } else {
                Name::from(INTERNAL_SYMBOL_NAME_DEFAULT)
            };
        }
        let name = get_name_of_declaration_in(node, d);
        if name.is_some() {
            let name_kind = name.kind();
            // Go `ast.IsAmbientModule(node)` (binder.go:306), inline: the
            // name of a module declaration is `name`. Its kinds at pin
            // fed0bf24149f:
            // Go: ast/utilities.go:1707 IsAmbientModule
            if kind == SyntaxKind::ModuleDeclaration
                && (name_kind == SyntaxKind::StringLiteral || is_global_scope_augmentation(node))
            {
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
            if name_kind == SyntaxKind::PrivateIdentifier {
                // containingClass exists because private names only allowed inside classes
                let containing_class = get_containing_class(node);
                if containing_class.is_nil() {
                    // we can get here in cases where there is already a parse error.
                    return Name::from(INTERNAL_SYMBOL_NAME_MISSING);
                }
                let private_name = Name::from(get_symbol_name_for_private_identifier(
                    containing_class,
                    name.text(),
                ));
                self.symbols.note_private_name(&private_name);
                // PORT: Go gives the class symbol its id here
                // (`ast.GetSymbolId`); `program::bind_all` gives it after
                // the bind, in Go's bind order.
                let classes = &mut self.file_bind.private_name_classes;
                if classes.last() != Some(&containing_class) && !classes.contains(&containing_class)
                {
                    classes.push(containing_class);
                }
                return private_name;
            }
            // Go `ast.IsPropertyNameLiteral(name) || ast.IsJsxNamespacedName(name)`
            // (binder.go:327), inline. Their kinds at pin fed0bf24149f:
            // Go: ast/utilities.go:309 IsPropertyNameLiteral
            // Go: ast/ast_generated.go:6539 IsJsxNamespacedName
            if matches!(
                name_kind,
                SyntaxKind::Identifier
                    | SyntaxKind::StringLiteral
                    | SyntaxKind::NoSubstitutionTemplateLiteral
                    | SyntaxKind::NumericLiteral
                    | SyntaxKind::JsxNamespacedName
            ) {
                // PERF: U1 (a). An identifier name reads the name interned at
                // parse (`Node::text_name`), with no text load and no intern.
                if name_kind == SyntaxKind::Identifier {
                    return name.text_name();
                }
                return Name::from(name.text());
            }
            if name_kind == SyntaxKind::ComputedPropertyName {
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
        match kind {
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
// PORT: Go puts `ast.GetSymbolId(containingClassSymbol)`, a lazy global id,
// in the name. Here the number is the key of the class node
// (`private_name_key`), so the binder and the checker give the class the
// same names, and `go_symbol_name` puts the Go id back where Go shows
// `symbol.Name`: the API and the node builder
// (`create_access_from_symbol_chain`). The binder notes the class
// (`FileBindData::private_name_classes`), and `program::bind_all` gives it
// its id after the bind, as Go's bind does. The binder passes the
// containing class node; the checker passes the class symbol
// (`get_symbol_name_for_private_identifier_of_symbol`).
// PORT: edleak1. The number was the arena index of the class symbol, so
// each bind of an edited file (a new class symbol) interned new names, and
// the process-wide interner never frees them (query-core: about 4 KiB per
// edit, from 24 private members). Go frees them with the file.
#[must_use]
pub fn get_symbol_name_for_private_identifier(containing_class: Node, description: &str) -> String {
    format!(
        "{}#{}@{}",
        INTERNAL_SYMBOL_NAME_PREFIX,
        private_name_key(containing_class),
        description
    )
}

/// Rust-only (edleak1): the number in the private names of class-like node
/// `class` (`get_symbol_name_for_private_identifier`). It is unique among
/// the classes of a program: the hash of the file name in the high bits,
/// and the store handle of the class name (of the class node when it has
/// none) in the low 32 bits. The next version of an edited file gives the
/// same class the same key when the edit adds or removes no node before its
/// name (a node gets its handle when the parser makes it, and a class node
/// after its members), so its names are interned again only then. The key
/// is at least 2^32, above every symbol id
/// (`get_symbol_name_for_private_identifier_of_symbol`).
// PORT: a hash of the file name, not an id, so the key needs no shared
// table: two file names with one 64-bit hash and one handle would share
// names.
#[must_use]
pub fn private_name_key(class: Node) -> u128 {
    let name = class.name();
    let identity = if name.is_some() { name } else { class };
    let file = source_file_file_name(get_source_file_of_node(class));
    let file_hash = xxhash_rust::xxh3::xxh3_64(file.as_bytes());
    ((u128::from(file_hash) + 1) << 32) | u128::from(identity.0 as u32)
}

/// Go `GetSymbolNameForPrivateIdentifier(containingClassSymbol, description)`
/// for the checker, which has the symbol. A symbol that a class-like node
/// declares (`node.symbol()`, the binder symbol) gets the names that the
/// binder gave its members. Any other symbol (an interface, a merged copy
/// of a class) gets its id as the number: Go's id of such a symbol differs
/// from the class's too, so no member of the class matches, and an id is
/// below 2^32, so it matches no class key.
#[must_use]
pub fn get_symbol_name_for_private_identifier_of_symbol(
    symbols: &SymbolArena,
    containing_class_symbol: SymbolId,
    description: &str,
) -> String {
    let class = symbols
        .sym(containing_class_symbol)
        .declarations
        .iter()
        .copied()
        .find(|&declaration| {
            is_class_like(declaration) && declaration.symbol() == containing_class_symbol
        });
    match class {
        Some(class) => get_symbol_name_for_private_identifier(class, description),
        None => format!(
            "{}#{}@{}",
            INTERNAL_SYMBOL_NAME_PREFIX, containing_class_symbol.0, description
        ),
    }
}

/// Rust-only: the text of Go `symbol.Name` for `symbol`, as the API returns
/// it (Go api/session.go:75 `newSymbolResponse`). A private identifier name
/// holds the key of its class (`get_symbol_name_for_private_identifier`).
/// Go holds `ast.GetSymbolId` of the class there, which is also the API
/// handle of the class (Go api/proto.go:40 `SymbolHandle`). This returns the
/// name with that id (`get_symbol_id`). Other names come back as they are.
/// Only a symbol that a private identifier declares, in a class whose key
/// the name holds, has such a name; a name that source text spells stays.
#[must_use]
pub fn go_symbol_name(symbols: &SymbolArena, symbol: SymbolId) -> String {
    go_private_name(symbols, symbol).unwrap_or_else(|| symbols.sym(symbol).name.to_string())
}

/// `go_symbol_name` for a private identifier name only: None when the name
/// of `symbol` holds no class key, so the caller keeps the name it has.
#[must_use]
pub fn go_private_name(symbols: &SymbolArena, symbol: SymbolId) -> Option<String> {
    let s = symbols.sym(symbol);
    if !s.name.is_internal() {
        return None;
    }
    let rest = s
        .name
        .as_str()
        .strip_prefix(INTERNAL_SYMBOL_NAME_PREFIX)?
        .strip_prefix('#')?;
    let at = rest.find('@')?;
    let key = rest[..at].parse::<u128>().ok()?;
    // The binder names a private identifier declaration after its
    // containing class (`get_declaration_name_of_kind`).
    let class = s
        .declarations
        .iter()
        .filter(|&&declaration| is_private_identifier(get_name_of_declaration(declaration)))
        .map(|&declaration| get_containing_class(declaration))
        .find(|&class| class.is_some() && private_name_key(class) == key)?;
    Some(format!(
        "{}#{}{}",
        INTERNAL_SYMBOL_NAME_PREFIX,
        get_symbol_id(symbols, class.symbol()),
        &rest[at..]
    ))
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
        Declarations::one(declaration)
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
                self.bind_binary_expression_kind(node, &bin);
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

    /// The BinaryExpression arm of `bind` (Go binder.go:622-633): the JS
    /// assignment declarations, then the strict mode check. `bind` and the
    /// binary frames (`bind_binary_expression_flow`) run it.
    #[inline]
    pub fn bind_binary_expression_kind(&mut self, node: Node, bin: &BinaryView) {
        match get_binary_assignment_declaration_kind(bin) {
            JSDeclarationKind::MODULE_EXPORTS => self.bind_module_exports_assignment(node),
            JSDeclarationKind::EXPORTS_PROPERTY => {
                self.bind_exports_or_object_define_property(node)
            }
            JSDeclarationKind::PROPERTY => self.bind_expando_property_assignment(node),
            JSDeclarationKind::THIS_PROPERTY => self.bind_this_property_assignment(node),
            _ => {}
        }
        self.check_strict_mode_binary_expression(node, bin);
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The binder fields in the records of each slot of `file`.
    fn record_fields(file: usize) -> Vec<Option<(SymbolId, NodeFlags, u32)>> {
        let records = crate::ast::bind_records(file);
        (0..records.len())
            .map(|index| records.node_fields(index))
            .collect()
    }

    /// bindonce2: the binder writes the node records while it binds. A bind
    /// that is not installed (a parallel bind that is dropped, or a panic)
    /// sets them back as the parse left them, so the serial bind after it
    /// gives the output of a first bind.
    #[test]
    fn a_bind_that_is_not_installed_resets_the_node_records() {
        let dir = std::env::temp_dir().join(format!("ts_goport_bindonce2_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("a.ts"),
            "export class C { x = 1; m() { return this.x; } }\n\
             export function f(a: number) { if (a) { return a; } }\n\
             namespace N { export const v = 1; }\n\
             outer: for (const i of [1]) { break outer; }\n\
             let y = f(1) ?? N.v;\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("tsconfig.json"),
            r#"{ "compilerOptions": { "noLib": true, "types": [] }, "files": ["a.ts"] }"#,
        )
        .unwrap();
        let config = dir.join("tsconfig.json");
        let program = crate::program::try_load_version(&config.to_string_lossy(), |_| {})
            .unwrap_or_else(|e| panic!("cannot load {}: {e}", config.display()));
        let _ = std::fs::remove_dir_all(&dir);
        let _scope = crate::core::enter_program(Some(program));
        let file = program
            .source_files()
            .find(|file| file.info.file_name.ends_with("/a.ts"))
            .expect("a.ts is in the program")
            .root;
        let parsed = record_fields(file.file_index());
        let bind = || bind_source_file_live(file, &mut SymbolArena::new_file());

        let first = bind();
        assert_ne!(
            record_fields(file.file_index()),
            parsed,
            "the bind wrote no record"
        );
        let parts = first.node_bind.compact().into_owned();
        drop(first);
        assert_eq!(
            record_fields(file.file_index()),
            parsed,
            "a dropped bind left its writes"
        );

        let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _bound = bind();
            panic!("test: a bind thread panics after the bind");
        }));
        assert!(panicked.is_err());
        assert_eq!(
            record_fields(file.file_index()),
            parsed,
            "a panicked bind left its writes"
        );

        let again = bind();
        assert!(
            again.node_bind.compact().parts() == parts.parts(),
            "a bind after the reset gives other data"
        );
    }

    /// The binder data of one file of a program (`program_bind_data`).
    /// Symbols are places (`place`), flow nodes and nodes are their indexes
    /// in the file.
    #[derive(Debug, PartialEq)]
    struct FileBindData {
        /// Each slot's record: symbol, Go `node.Flags` and `bind` word.
        records: Vec<Option<(Option<usize>, NodeFlags, u32)>>,
        /// Each extras entry: local symbol, locals (name and symbol), next
        /// container, flow node, end flow node, return flow node.
        #[allow(clippy::type_complexity)]
        extras: Vec<(
            Option<usize>,
            Vec<(String, usize)>,
            Option<usize>,
            Option<usize>,
            Option<usize>,
            Option<usize>,
        )>,
        flow_nodes: usize,
    }

    /// The binder data of each file of the current program `program`, which
    /// is bound. A symbol is given as its place among the lineage symbols
    /// from the first symbol of the program, as in the test
    /// `a_bind_that_gives_a_node_id_on_a_bind_thread_matches_the_serial_bind`
    /// (program.rs): two loads of the same files then give the same data
    /// when they bound the same way.
    fn program_bind_data(program: &'static crate::core::GoProgram) -> Vec<FileBindData> {
        let symbols = crate::program::bound_symbols();
        let files: Vec<usize> = program
            .source_files()
            .map(|f| f.root.file_index())
            .collect();
        let ids = files.iter().flat_map(|&file| {
            let records = crate::ast::bind_records(file);
            (0..records.len()).filter_map(move |index| {
                let (symbol, ..) = records.node_fields(index)?;
                symbol.is_some().then_some(symbol.0)
            })
        });
        let first = ids.min().expect("the program has symbols");
        let place = |symbol: SymbolId| {
            symbol.is_some().then(|| {
                assert!(
                    !crate::core::is_own_index(symbol.0),
                    "symbol {} is in an odd chunk",
                    symbol.0
                );
                (first..symbol.0)
                    .filter(|&i| !crate::core::is_own_index(i))
                    .count()
            })
        };
        let flow = |flow: FlowNodeId| flow.is_some().then(|| flow.local_index());
        files
            .into_iter()
            .map(|file| {
                let go_file = crate::ast::go_file(file);
                let records = crate::ast::bind_records(file);
                let records = (0..records.len())
                    .map(|index| {
                        let (symbol, flags, word) = records.node_fields(index)?;
                        Some((place(symbol), flags, word))
                    })
                    .collect();
                let extras = go_file
                    .node_bind
                    .get()
                    .expect("file is bound")
                    .extras()
                    .iter()
                    .map(|extra| {
                        let locals = symbols
                            .iter(extra.locals)
                            .map(|(name, symbol)| (name.to_string(), place(symbol).unwrap()))
                            .collect();
                        (
                            place(extra.local_symbol),
                            locals,
                            (!extra.next_container.is_nil())
                                .then(|| extra.next_container.node_id().index()),
                            flow(extra.flow_node),
                            flow(extra.end_flow_node),
                            flow(extra.return_flow_node),
                        )
                    })
                    .collect();
                let flow_nodes = go_file.flow_nodes.get().expect("file is bound").len();
                FileBindData {
                    records,
                    extras,
                    flow_nodes,
                }
            })
            .collect()
    }

    /// R219 reviewer item b: a bind that panics in the binder after it wrote
    /// records, on a bind thread (`program::run_bind_link`). The binder
    /// unwinds, so its records go back to the parse state
    /// (`NodeBindRecords` drop). `bind_all` then binds that file and the
    /// files after it serially, and the binder data of every file is that of
    /// a serial bind with no panic.
    #[test]
    fn a_bind_that_panics_on_a_bind_thread_rebinds_as_a_clean_serial_bind() {
        let dir = std::env::temp_dir().join(format!("ts_goport_bind_panic_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let files = [
            ("a.ts", "export const a = 1;\n"),
            (
                "b.ts",
                "export class C { x = 1; m() { return this.x; } }\n\
                 export function f(n: number) { if (n) { return n; } }\n\
                 namespace N { export const v = 1; }\n\
                 outer: for (const i of [1]) { break outer; }\n\
                 export let y = f(1) ?? N.v;\n",
            ),
            (
                "c.ts",
                "import { C } from \"./b\";\nexport const c = new C().m();\n",
            ),
            (
                "d.ts",
                "export interface I { m(): this; }\n\
                 export function g(this: I) { return this; }\n",
            ),
            ("e.ts", "export const e = async () => 1;\n"),
        ];
        for (name, text) in files {
            std::fs::write(dir.join(name), text).unwrap();
        }
        std::fs::write(
            dir.join("tsconfig.json"),
            r#"{ "compilerOptions": { "noLib": true, "types": [] },
                 "files": ["a.ts", "b.ts", "c.ts", "d.ts", "e.ts"] }"#,
        )
        .unwrap();
        let config = dir.join("tsconfig.json");
        // Loads and binds the program on `threads` bind threads, each load
        // on a new thread. With `panic_after`, the first bind of b.ts panics
        // when it has written that many symbols. Returns the binder data,
        // whether the panic hook is still set, and the indexes of the files
        // that `bind_all` bound serially.
        let bind = |threads: usize, panic_after: Option<usize>| {
            let config = config.clone();
            std::thread::spawn(move || {
                crate::program::TEST_BIND_THREADS.set(Some(threads));
                let program = crate::program::try_load_version(&config.to_string_lossy(), |_| {})
                    .unwrap_or_else(|e| panic!("cannot load {}: {e}", config.display()));
                let _program = crate::core::enter_program(Some(program));
                let roots: Vec<Node> = program.source_files().map(|f| f.root).collect();
                if let Some(symbols) = panic_after {
                    *TEST_PANIC_IN_BIND.lock().unwrap() = Some((roots[1].file_index(), symbols));
                }
                crate::program::bind_all();
                let hook_left = TEST_PANIC_IN_BIND.lock().unwrap().take().is_some();
                let serial_binds: Vec<usize> = crate::program::TEST_SERIAL_BINDS
                    .take()
                    .into_iter()
                    .map(|root| roots.iter().position(|&r| r == root).unwrap())
                    .collect();
                (program_bind_data(program), hook_left, serial_binds)
            })
            .join()
            .unwrap()
        };
        let clean = bind(1, None);
        let rebound = bind(4, Some(4));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(clean.2, [0, 1, 2, 3, 4], "a serial bind binds every file");
        assert!(!rebound.1, "the bind of b.ts did not take the panic hook");
        // The bind threads bind a.ts. The bind of b.ts panics there, so
        // bind_all binds b.ts and the files after it serially.
        assert_eq!(
            rebound.2,
            [1, 2, 3, 4],
            "b.ts and the files after it bind serially"
        );
        assert!(
            clean.0[1]
                .records
                .iter()
                .flatten()
                .any(|&(_, _, word)| word != 0)
                && !clean.0[1].extras.is_empty(),
            "b.ts has flow nodes and extras"
        );
        assert_eq!(rebound.0, clean.0);
    }
}
