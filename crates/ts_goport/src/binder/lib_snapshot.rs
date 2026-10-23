//! Rust-only: the lib bind snapshot (perf9 round 2, `r2-lib-bind-snapshot`).
//!
//! PERF: binding lib.dom is most of the query bind window (about 10 ms on
//! one bind thread, 2.5 to 3 times Go). The bind output of the large bundled
//! lib files is embedded in `lib_bind.bin`, and `bind_source_file_detached`
//! loads it instead of binding when the key matches. The loaded output
//! equals a live bind of the file into a new arena: the same symbols in the
//! same order, the tables with their entries in insertion order, the same
//! ids, node data, flow nodes and file data (bind diagnostics included).
//! The file then joins the program arena like any other bound file.
//!
//! The key of a lib file (`SnapshotKey`) is two hashes of its text (xxh3,
//! and `const_hash`, which the embed build has from compile time), its
//! slot count, an xxh3 hash of its kinds, parser flags and names columns
//! and of the file facts that the binder reads, and a compile-time hash of
//! the binder sources (`SOURCES_HASH`). Any mismatch or load error binds the
//! file live, so a stale blob costs time, not output.
//!
//! Node references and flow ids are stored without the file index (the low
//! half of the handle), and the load adds the program file index. Symbol
//! and table ids are ids of the file arena. A name with a stable id
//! (`Name::stable_id`: the lib names of `core/lib_names.rs`) is stored by
//! id; any other name is stored as text and interned at load.
//!
//! After a change to the binder, `core.rs`, the parser or a bundled lib,
//! write the blob again (the integrator does this after each merge round,
//! as for PGO training):
//!
//! ```text
//! cargo test -p goport_core --lib binder::lib_snapshot::tests::generate_lib_bind_snapshot -- --ignored --exact
//! ```
//!
//! `snapshot_matches_live_bind` fails while the blob is stale.
//!
//! A snapshot lib must have no private class member: a private name holds
//! a hash of the lib's file name (`private_name_key`), and the key does not
//! hold the file name. `generate_lib_bind_snapshot` fails on one.
//! `GOPORT_LIB_SNAPSHOT=0` turns the snapshot off (for A/B timing), and
//! `GOPORT_LIB_SNAPSHOT=trace` writes one line per lib file to stderr.
//!
//! Blob layout, all integers little-endian: `MAGIC`, the lib count (u32),
//! then per lib its base name (`lib.dom.d.ts`) and its section, each as a
//! u32 length and the bytes. A section is the key (`SnapshotKey::write`),
//! the name, symbol, table and flow node counts (u32 each), the names, then
//! the body that `encode` writes.

use crate::frontend::bundled;
use crate::prelude::*;
use std::sync::OnceLock;
use xxhash_rust::xxh3::{Xxh3, xxh3_64};

/// The embedded snapshot, written by `generate_lib_bind_snapshot`.
static BLOB: &[u8] = include_bytes!("lib_bind.bin");

/// The first bytes of `BLOB`.
const MAGIC: &[u8; 8] = b"TSLIBBND";

/// Bundled lib files with at least this many text bytes get a snapshot
/// (see `bundled::SNAPSHOT_TEXT_MIN`).
pub const MIN_TEXT_LEN: usize = bundled::SNAPSHOT_TEXT_MIN;

/// Set in a stored name entry (and in a names column entry of the parse
/// hash) when text follows. The bits below it hold the text length. A
/// clear bit means the entry is a stable name id, which is always lower.
pub(crate) const TEXT_BIT: u32 = 1 << 31;

// Symbol mask bits: which optional fields follow.
const S_CHECK_FLAGS: u8 = 1;
const S_MEMBERS: u8 = 2;
const S_EXPORTS: u8 = 4;
const S_PARENT: u8 = 8;
const S_EXPORT_SYMBOL: u8 = 16;
/// `value_declaration` follows.
const S_VALUE_DECLARATION: u8 = 32;
/// `value_declaration` is the first declaration and is not stored.
const S_VALUE_IS_FIRST: u8 = 64;
/// Exactly one declaration follows, with no count.
const S_ONE_DECLARATION: u8 = 128;

// `NodeBindData` mask bits: which fields follow (the others are nil).
const N_SYMBOL: u8 = 1;
const N_LOCAL_SYMBOL: u8 = 2;
const N_LOCALS: u8 = 4;
const N_NEXT_CONTAINER: u8 = 8;
const N_FLOW_NODE: u8 = 16;
const N_END_FLOW_NODE: u8 = 32;
const N_RETURN_FLOW_NODE: u8 = 64;
const N_ADDED_FLAGS: u8 = 128;

// `FlowNode` mask bits.
const F_NODE: u8 = 1;
const F_ANTECEDENT: u8 = 2;
const F_ANTECEDENTS: u8 = 4;

// `Diagnostic` bool bits.
const D_REPORTS_UNNECESSARY: u8 = 1;
const D_REPORTS_DEPRECATED: u8 = 2;
const D_SKIPPED_ON_NO_EMIT: u8 = 4;

// ──────────────────────────────────────────────────────────────────────
// Key
// ──────────────────────────────────────────────────────────────────────

/// A compile-time hash of the sources whose change can change the bind
/// output or the blob format: the binder, `core.rs` (arena, node data,
/// names), the flag values, the lib name ids and this file. After a change
/// there, every key misses until the blob is written again.
const SOURCES_HASH: u64 = {
    let hashes = [
        SOURCE_BINDER_P1,
        SOURCE_BINDER_P2,
        SOURCE_BINDER_P3,
        SOURCE_CORE,
        SOURCE_FLAGS,
        SOURCE_LIB_NAMES,
        SOURCE_SNAPSHOT,
    ];
    let mut hash = 0;
    let mut i = 0;
    while i < hashes.len() {
        hash = mix(hash ^ hashes[i]);
        i += 1;
    }
    hash
};

// One item per file, so each compile-time evaluation stays short. The
// source text is only read at compile time and is not in the binary.
const SOURCE_BINDER_P1: u64 = const_hash(include_bytes!("binder_p1.rs"));
const SOURCE_BINDER_P2: u64 = const_hash(include_bytes!("binder_p2.rs"));
const SOURCE_BINDER_P3: u64 = const_hash(include_bytes!("binder_p3.rs"));
const SOURCE_CORE: u64 = const_hash(include_bytes!("../core.rs"));
const SOURCE_FLAGS: u64 = const_hash(include_bytes!("../flags.rs"));
const SOURCE_LIB_NAMES: u64 = const_hash(include_bytes!("../core/lib_names.rs"));
const SOURCE_SNAPSHOT: u64 = const_hash(include_bytes!("lib_snapshot.rs"));

/// `SOURCES_HASH` and the text part of the keys (`SnapshotKey::text_const`)
/// use the compile-time hash of `frontend/bundled.rs`. The lib parse
/// snapshot (`frontend/parser/lib_parse_snapshot.rs`) uses it too.
pub(crate) use crate::frontend::bundled::{const_hash, mix};

/// What must match for a lib file to load its section.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SnapshotKey {
    /// `SOURCES_HASH` of the build that wrote the section.
    sources: u64,
    /// The slot count (`GoFile::parser_flags` length).
    nodes: u32,
    /// xxh3 of the file text.
    text: u64,
    /// `const_hash` of the file text (`text_matches`).
    text_const: u64,
    /// `parse_hash` of the file.
    parse: u64,
}

impl SnapshotKey {
    /// The key of `file` in this process. `None` when the file cannot have
    /// a snapshot (see `parse_hash`).
    fn of(file: Node) -> Option<SnapshotKey> {
        let go_file = crate::ast::go_file(file.file_index());
        Some(SnapshotKey {
            sources: SOURCES_HASH,
            nodes: u32::try_from(go_file.parser_flags.len()).ok()?,
            text: text_hash(file),
            text_const: const_hash(source_file_text(file).as_bytes()),
            parse: parse_hash(file, &go_file)?,
        })
    }

    fn write(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.sources.to_le_bytes());
        out.extend_from_slice(&self.nodes.to_le_bytes());
        out.extend_from_slice(&self.text.to_le_bytes());
        out.extend_from_slice(&self.text_const.to_le_bytes());
        out.extend_from_slice(&self.parse.to_le_bytes());
    }

    fn read(r: &mut SnapshotReader<'_>) -> Option<SnapshotKey> {
        Some(SnapshotKey {
            sources: r.u64()?,
            nodes: r.u32()?,
            text: r.u64()?,
            text_const: r.u64()?,
            parse: r.u64()?,
        })
    }

    /// Compares this stored key with `file`, cheapest part first, so a
    /// stale blob does not hash the text or the columns. The error names
    /// the first part that differs.
    fn check(&self, file: Node, go_file: &GoFile) -> Result<(), &'static str> {
        if self.sources != SOURCES_HASH {
            return Err("sources");
        }
        if self.nodes as usize != go_file.parser_flags.len() {
            return Err("nodes");
        }
        if !text_matches(self.text, self.text_const, &source_file_text(file)) {
            return Err("text");
        }
        if Some(self.parse) != parse_hash(file, go_file) {
            return Err("parse");
        }
        Ok(())
    }
}

/// xxh3 of the text of `file`.
fn text_hash(file: Node) -> u64 {
    xxh3_64(source_file_text(file).as_bytes())
}

/// Whether `text` has the stored text hashes of a key: `text_const` for an
/// embedded lib, whose `const_hash` is known at compile time, else `text`
/// (xxh3). The parse snapshot checks its key the same way.
///
/// PERF (rss2): an embedded lib is not read here. Hashing it brought each
/// page of lib.dom (2.3 MB of the binary) into the RSS of a program that
/// uses it. xxh3 stays for a text read from a file (noembed), where it is
/// four times faster than `const_hash`.
pub(crate) fn text_matches(text_hash: u64, text_const: u64, text: &str) -> bool {
    match bundled::embedded_text_hash(text) {
        Some(hash) => text_const == hash,
        None => text_hash == xxh3_64(text.as_bytes()),
    }
}

/// An xxh3 hash of what the binder reads from the parse of `file` besides
/// the text: the file facts (`SourceFileInfo`) and the kinds, parser flags
/// and names columns of its slots. `None` when the file has no node store,
/// or has an alias slot (a child that is not a node of the file), whose
/// node references a snapshot cannot keep.
fn parse_hash(file: Node, go_file: &GoFile) -> Option<u64> {
    let file_index = file.file_index();
    if !has_file_store(file_index) {
        return None;
    }
    let info = &go_file.info;
    let mut hasher = Xxh3::new();
    let facts = [
        u64::from(info.is_declaration_file),
        u64::from(info.has_lazy_js_doc),
        info.language_variant.0 as u64,
        info.script_kind.0 as u64,
        u64::from(local_node(file_index, info.external_module_indicator)?),
        info.diagnostics.len() as u64,
    ];
    for fact in facts {
        hasher.update(&fact.to_le_bytes());
    }
    // PERF: the columns go through a buffer, so xxh3 hashes long runs.
    const FLUSH: usize = 16 * 1024;
    let mut buffer: Vec<u8> = Vec::with_capacity(FLUSH + 64);
    let base = (file_index as u64) << 32;
    for (index, flags) in go_file.parser_flags.iter().enumerate().skip(1) {
        let node = Node(base | (index as u64 + 1));
        let kind = node.kind();
        if kind == SyntaxKind::Unknown {
            return None;
        }
        buffer.extend_from_slice(&(kind as u16).to_le_bytes());
        buffer.extend_from_slice(&flags.0.to_le_bytes());
        if matches!(kind, SyntaxKind::Identifier | SyntaxKind::PrivateIdentifier) {
            let name = store_identifier_name(node)?;
            match name.stable_id() {
                Some(id) => buffer.extend_from_slice(&id.to_le_bytes()),
                None => {
                    let len = u32::try_from(name.len()).ok()?;
                    buffer.extend_from_slice(&(TEXT_BIT | len).to_le_bytes());
                    buffer.extend_from_slice(name.as_bytes());
                }
            }
        }
        if buffer.len() >= FLUSH {
            hasher.update(&buffer);
            buffer.clear();
        }
    }
    hasher.update(&buffer);
    Some(hasher.digest())
}

/// `node` without its file index: the low half of the handle (slot + 1),
/// 0 for nil. `None` for a node of another file.
fn local_node(file: usize, node: Node) -> Option<u32> {
    if node.is_nil() {
        return Some(0);
    }
    (node.file_index() == file).then_some(node.0 as u32)
}

/// `flow` without its file index: the low half of the id (index + 1), 0
/// for nil. `None` for a flow node of another file.
fn local_flow(file: usize, flow: FlowNodeId) -> Option<u32> {
    if flow.is_nil() {
        return Some(0);
    }
    (flow.file_index() == file).then_some(flow.0 as u32)
}

// ──────────────────────────────────────────────────────────────────────
// Load
// ──────────────────────────────────────────────────────────────────────

/// A snapshot switch (`GOPORT_LIB_SNAPSHOT`, and
/// `GOPORT_LIB_PARSE_SNAPSHOT` for the lib parse snapshot).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    Off,
    On,
    /// On, with one stderr line per lib file.
    Trace,
}

impl Mode {
    /// Environment variable `var`: "0" turns the snapshot off, "trace"
    /// traces it.
    pub(crate) fn from_env(var: &str) -> Mode {
        match std::env::var(var).as_deref() {
            Ok("0") => Mode::Off,
            Ok("trace") => Mode::Trace,
            _ => Mode::On,
        }
    }
}

/// `GOPORT_LIB_SNAPSHOT`: "0" turns the snapshot off, "trace" traces it.
fn mode() -> Mode {
    static MODE: OnceLock<Mode> = OnceLock::new();
    *MODE.get_or_init(|| Mode::from_env("GOPORT_LIB_SNAPSHOT"))
}

/// One lib file in a snapshot blob.
pub(crate) struct SnapshotEntry {
    /// The base name, for example `lib.dom.d.ts`.
    pub(crate) name: &'static str,
    pub(crate) section: &'static [u8],
}

/// The lib files in `BLOB`, read once. Empty when the blob does not read.
fn entries() -> &'static [SnapshotEntry] {
    static ENTRIES: OnceLock<Vec<SnapshotEntry>> = OnceLock::new();
    ENTRIES.get_or_init(|| read_entries(BLOB, MAGIC).unwrap_or_default())
}

/// The entries of a blob that `write_blob` wrote with `magic`. `None` when
/// the blob does not read.
pub(crate) fn read_entries(blob: &'static [u8], magic: &[u8; 8]) -> Option<Vec<SnapshotEntry>> {
    let mut r = SnapshotReader::new(blob);
    if r.bytes(magic.len())? != magic {
        return None;
    }
    let count = r.count()?;
    let mut entries = Vec::with_capacity(count);
    for _ in 0..count {
        let name = r.str()?;
        let len = r.count()?;
        let section = r.bytes(len)?;
        entries.push(SnapshotEntry { name, section });
    }
    Some(entries)
}

/// The bind output of `file` from the snapshot, with its symbols and
/// tables in `symbols`, which must be a new arena. `None` (bind live) when
/// `file` is not a snapshot lib, `symbols` is not new, the key does not
/// match or the section does not load. Then `symbols` is unchanged. A new
/// lineage arena (`SymbolArena::new`, a serial bind) gets the file arena
/// joined, with its ids moved (`SymbolArena::append_file_arena`).
pub fn load(file: Node, symbols: &mut SymbolArena) -> Option<BoundFile> {
    let mode = mode();
    if mode == Mode::Off || !symbols.is_new() {
        return None;
    }
    let info = source_file_info(file);
    let lib = bundled::bundled_lib_name(&info.file_name)?;
    let entry = entries().iter().find(|entry| entry.name == lib)?;
    let start = (mode == Mode::Trace).then(std::time::Instant::now);
    let result = load_section(file, entry.section);
    if let Some(start) = start {
        let status = match &result {
            Ok(_) => "loaded",
            Err(part) => *part,
        };
        eprintln!(
            "goport lib snapshot: {lib} {status} {} us",
            start.elapsed().as_micros()
        );
    }
    let (mut bound, arena) = result.ok()?;
    if symbols.is_interleaved() {
        let freeable = crate::ast::go_file(file.file_index()).version().is_some();
        bound.remap(symbols.append_file_arena(arena, freeable));
    } else {
        *symbols = arena;
    }
    Some(bound)
}

/// Checks the key in `section` against `file` and decodes the section.
/// The error is the key part that differs (`SnapshotKey::check`), or
/// "load" for a section that does not decode.
fn load_section(file: Node, section: &[u8]) -> Result<(BoundFile, SymbolArena), &'static str> {
    let go_file = crate::ast::go_file(file.file_index());
    let mut r = SnapshotReader::new(section);
    let key = SnapshotKey::read(&mut r).ok_or("load")?;
    key.check(file, &go_file)?;
    decode(file, key.nodes, r).ok_or("load")
}

/// A cursor over blob bytes. Every read returns `None` past the end.
pub(crate) struct SnapshotReader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> SnapshotReader<'a> {
    pub(crate) fn new(data: &'a [u8]) -> Self {
        SnapshotReader { data, pos: 0 }
    }

    pub(crate) fn remaining(&self) -> usize {
        self.data.len() - self.pos
    }

    pub(crate) fn bytes(&mut self, len: usize) -> Option<&'a [u8]> {
        let bytes = self.data.get(self.pos..self.pos.checked_add(len)?)?;
        self.pos += len;
        Some(bytes)
    }

    pub(crate) fn u8(&mut self) -> Option<u8> {
        let value = *self.data.get(self.pos)?;
        self.pos += 1;
        Some(value)
    }

    pub(crate) fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.bytes(4)?.try_into().ok()?))
    }

    pub(crate) fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.bytes(8)?.try_into().ok()?))
    }

    /// A count of items that take at least one byte each. A count past the
    /// end is an error, so a bad count cannot ask for a huge allocation.
    pub(crate) fn count(&mut self) -> Option<usize> {
        let count = self.u32()? as usize;
        (count <= self.remaining()).then_some(count)
    }

    pub(crate) fn text(&mut self, len: usize) -> Option<&'a str> {
        std::str::from_utf8(self.bytes(len)?).ok()
    }

    pub(crate) fn str(&mut self) -> Option<&'a str> {
        let len = self.u32()? as usize;
        self.text(len)
    }
}

/// Reads a section body into a new arena. Each id and reference is checked
/// against its count, so a bad section fails here, not in a later read.
struct Decoder<'a> {
    r: SnapshotReader<'a>,
    /// `file index << 32`: the high half of each node and flow id.
    base: u64,
    nodes: u32,
    symbols: u32,
    tables: u32,
    flows: u32,
    names: Vec<Name>,
}

impl Decoder<'_> {
    fn node(&mut self) -> Option<Node> {
        let local = self.r.u32()?;
        match local {
            0 => Some(Node::NIL),
            _ if local <= self.nodes => Some(Node(self.base | u64::from(local))),
            _ => None,
        }
    }

    fn flow(&mut self) -> Option<FlowNodeId> {
        let local = self.r.u32()?;
        match local {
            0 => Some(FlowNodeId::NIL),
            _ if local <= self.flows => Some(FlowNodeId(self.base | u64::from(local))),
            _ => None,
        }
    }

    fn symbol(&mut self) -> Option<SymbolId> {
        let id = self.r.u32()?;
        (id <= self.symbols).then_some(SymbolId(id))
    }

    fn table(&mut self) -> Option<SymbolTable> {
        let id = self.r.u32()?;
        (id <= self.tables).then_some(SymbolTable(id))
    }

    fn name(&mut self) -> Option<Name> {
        let index = self.r.u32()? as usize;
        self.names.get(index).cloned()
    }

    fn symbol_value(&mut self) -> Option<Symbol> {
        let flags = SymbolFlags(self.r.u32()?);
        let name = self.name()?;
        let mask = self.r.u8()?;
        let check_flags = if mask & S_CHECK_FLAGS != 0 {
            CheckFlags(self.r.u32()?)
        } else {
            CheckFlags::default()
        };
        let members = if mask & S_MEMBERS != 0 {
            self.table()?
        } else {
            SymbolTable::NIL
        };
        let exports = if mask & S_EXPORTS != 0 {
            self.table()?
        } else {
            SymbolTable::NIL
        };
        let parent = if mask & S_PARENT != 0 {
            self.symbol()?
        } else {
            SymbolId::NIL
        };
        let export_symbol = if mask & S_EXPORT_SYMBOL != 0 {
            self.symbol()?
        } else {
            SymbolId::NIL
        };
        let mut value_declaration = if mask & S_VALUE_DECLARATION != 0 {
            self.node()?
        } else {
            Node::NIL
        };
        // `push` keeps one declaration inline, as the binder's pushes do.
        let mut declarations = Declarations::default();
        if mask & S_ONE_DECLARATION != 0 {
            declarations.push(self.node()?);
        } else {
            let count = self.r.count()?;
            if count > 0 {
                let mut list = Vec::with_capacity(count);
                for _ in 0..count {
                    list.push(self.node()?);
                }
                declarations = Declarations::from(list);
            }
        }
        if mask & S_VALUE_IS_FIRST != 0 {
            value_declaration = *declarations.first()?;
        }
        Some(Symbol {
            flags,
            check_flags,
            data: SymbolData {
                name,
                declarations,
                value_declaration,
                members,
                exports,
                parent,
                export_symbol,
            },
        })
    }

    fn node_bind_data(&mut self) -> Option<NodeBindData> {
        let mask = self.r.u8()?;
        let mut data = NodeBindData::default();
        if mask & N_SYMBOL != 0 {
            data.symbol = self.symbol()?;
        }
        if mask & N_LOCAL_SYMBOL != 0 {
            data.local_symbol = self.symbol()?;
        }
        if mask & N_LOCALS != 0 {
            data.locals = self.table()?;
        }
        if mask & N_NEXT_CONTAINER != 0 {
            data.next_container = self.node()?;
        }
        if mask & N_FLOW_NODE != 0 {
            data.flow_node = self.flow()?;
        }
        if mask & N_END_FLOW_NODE != 0 {
            data.end_flow_node = self.flow()?;
        }
        if mask & N_RETURN_FLOW_NODE != 0 {
            data.return_flow_node = self.flow()?;
        }
        if mask & N_ADDED_FLAGS != 0 {
            data.added_flags = NodeFlags(self.r.u32()?);
        }
        Some(data)
    }

    fn flow_node(&mut self) -> Option<FlowNode> {
        let flags = FlowFlags(self.r.u32()?);
        let mask = self.r.u8()?;
        let node = if mask & F_NODE != 0 {
            self.node()?
        } else {
            Node::NIL
        };
        let antecedent = if mask & F_ANTECEDENT != 0 {
            self.flow()?
        } else {
            FlowNodeId::NIL
        };
        let mut antecedents = Vec::new();
        if mask & F_ANTECEDENTS != 0 {
            let count = self.r.count()?;
            antecedents.reserve_exact(count);
            // A switch clause keeps two clause indexes here, not flow ids
            // (`create_flow_switch_clause`).
            let raw = flags.intersects(FlowFlags::SWITCH_CLAUSE);
            for _ in 0..count {
                antecedents.push(if raw {
                    FlowNodeId(u64::from(self.r.u32()?))
                } else {
                    self.flow()?
                });
            }
        }
        Some(FlowNode {
            flags,
            node,
            antecedent,
            antecedents,
        })
    }

    fn diagnostics(&mut self) -> Option<Vec<Diagnostic>> {
        let count = self.r.count()?;
        let mut list = Vec::with_capacity(count);
        for _ in 0..count {
            list.push(self.diagnostic()?);
        }
        Some(list)
    }

    fn diagnostic(&mut self) -> Option<Diagnostic> {
        let file = self.node()?;
        let pos = self.r.u32()? as i32;
        let end = self.r.u32()? as i32;
        let code = self.r.u32()? as i32;
        let category = match self.r.u8()? {
            0 => crate::diagnostics::Category::Warning,
            1 => crate::diagnostics::Category::Error,
            2 => crate::diagnostics::Category::Suggestion,
            3 => crate::diagnostics::Category::Message,
            _ => return None,
        };
        let message = crate::diagnostics::message_by_key(self.r.str()?)?;
        let arg_count = self.r.count()?;
        let mut message_args = Vec::with_capacity(arg_count);
        for _ in 0..arg_count {
            message_args.push(self.r.str()?.to_owned());
        }
        let message_chain = self.diagnostics()?;
        let related_information = self.diagnostics()?;
        let bits = self.r.u8()?;
        Some(Diagnostic {
            file,
            pos,
            end,
            code,
            category,
            // tsgo#4712: a lib file has no external diagnostic.
            source: String::new(),
            message,
            message_text: String::new(),
            message_args,
            message_chain,
            related_information,
            reports_unnecessary: bits & D_REPORTS_UNNECESSARY != 0,
            reports_deprecated: bits & D_REPORTS_DEPRECATED != 0,
            skipped_on_no_emit: bits & D_SKIPPED_ON_NO_EMIT != 0,
            repopulate_info: None,
        })
    }

    fn file_bind(&mut self) -> Option<FileBindData> {
        let bind_diagnostics = self.diagnostics()?;
        let symbol_count = self.r.u32()? as i32;
        let pattern_count = self.r.count()?;
        let mut pattern_ambient_modules = Vec::with_capacity(pattern_count);
        for _ in 0..pattern_count {
            pattern_ambient_modules.push(PatternAmbientModule {
                pattern_prefix: self.r.str()?.to_owned(),
                pattern_suffix: self.r.str()?.to_owned(),
                symbol: self.symbol()?,
            });
        }
        Some(FileBindData {
            bind_diagnostics,
            symbol_count,
            pattern_ambient_modules,
            global_exports: self.table()?,
            js_global_augmentations: self.table()?,
            common_js_module_indicator: self.node()?,
            has_expando_assignments: self.r.u8()? != 0,
            // A snapshot lib has no private class member (see the module
            // docs), so the section does not keep the list.
            private_name_classes: Vec::new(),
        })
    }
}

/// Decodes the section body after the key: the arena of `file` and its
/// `BoundFile`, as a live bind into a new arena makes them.
fn decode(file: Node, nodes: u32, r: SnapshotReader<'_>) -> Option<(BoundFile, SymbolArena)> {
    let mut d = Decoder {
        r,
        base: (file.file_index() as u64) << 32,
        nodes,
        symbols: 0,
        tables: 0,
        flows: 0,
        names: Vec::new(),
    };
    let name_count = d.r.count()?;
    d.symbols = u32::try_from(d.r.count()?).ok()?;
    d.tables = u32::try_from(d.r.count()?).ok()?;
    d.flows = u32::try_from(d.r.count()?).ok()?;
    d.names.reserve_exact(name_count);
    for _ in 0..name_count {
        let entry = d.r.u32()?;
        let name = if entry & TEXT_BIT == 0 {
            Name::from_stable_id(entry)?
        } else {
            Name::from(d.r.text((entry & !TEXT_BIT) as usize)?)
        };
        d.names.push(name);
    }

    // Symbols, then tables, then private names, in arena order, so every
    // id equals its index as in the live bind.
    let mut arena = SymbolArena::new_file();
    arena.reserve_arena(d.symbols as usize, d.tables as usize);
    for _ in 0..d.symbols {
        let symbol = d.symbol_value()?;
        arena.push_symbol(symbol);
    }
    let mut entries: Vec<(Name, SymbolId)> = Vec::new();
    for _ in 0..d.tables {
        let len = d.r.count()?;
        for _ in 0..len {
            let name = d.name()?;
            let symbol = d.symbol()?;
            entries.push((name, symbol));
        }
        arena.push_table_from_entries(entries.drain(..));
    }
    let private_count = d.r.count()?;
    for _ in 0..private_count {
        let name = d.name()?;
        arena.note_private_name(&name);
    }

    let mut flow_nodes = Vec::with_capacity(d.flows as usize);
    for _ in 0..d.flows {
        flow_nodes.push(d.flow_node()?);
    }

    let slot_count = d.r.count()?;
    if slot_count != nodes as usize {
        return None;
    }
    let slots = d.r.bytes(slot_count)?.to_vec();
    let base_count = d.r.count()?;
    let mut bases = Vec::with_capacity(base_count);
    for _ in 0..base_count {
        bases.push(d.r.u32()?);
    }
    let entry_count = d.r.count()?;
    let mut node_entries = Vec::with_capacity(entry_count);
    for _ in 0..entry_count {
        node_entries.push(d.node_bind_data()?);
    }
    let node_bind = NodeBindParts::from_parts(slots, bases, node_entries)?;

    let file_bind = d.file_bind()?;
    if d.r.remaining() != 0 {
        return None;
    }
    Some((
        BoundFile {
            file,
            node_bind: BoundNodes::Parts(node_bind),
            flow_nodes,
            file_bind,
        },
        arena,
    ))
}

// ──────────────────────────────────────────────────────────────────────
// Dump (the generator and its test)
// ──────────────────────────────────────────────────────────────────────

/// Writes a section body. `names` numbers each name at its first use.
struct Encoder {
    file: usize,
    body: Vec<u8>,
    names: FxHashMap<Name, u32>,
    name_list: Vec<Name>,
}

impl Encoder {
    fn u8(&mut self, value: u8) {
        self.body.push(value);
    }

    fn u32(&mut self, value: u32) {
        self.body.extend_from_slice(&value.to_le_bytes());
    }

    fn count(&mut self, count: usize) -> Result<(), String> {
        self.u32(u32::try_from(count).map_err(|_| format!("count {count} too large"))?);
        Ok(())
    }

    fn str(&mut self, text: &str) -> Result<(), String> {
        self.count(text.len())?;
        self.body.extend_from_slice(text.as_bytes());
        Ok(())
    }

    fn node(&mut self, node: Node) -> Result<(), String> {
        let local =
            local_node(self.file, node).ok_or_else(|| format!("{node:?} is in another file"))?;
        self.u32(local);
        Ok(())
    }

    fn flow(&mut self, flow: FlowNodeId) -> Result<(), String> {
        let local =
            local_flow(self.file, flow).ok_or_else(|| format!("{flow:?} is in another file"))?;
        self.u32(local);
        Ok(())
    }

    fn name(&mut self, name: &Name) {
        let index = match self.names.get(name) {
            Some(&index) => index,
            None => {
                let index = self.name_list.len() as u32;
                self.names.insert(name.clone(), index);
                self.name_list.push(name.clone());
                index
            }
        };
        self.u32(index);
    }

    fn symbol_value(&mut self, symbol: &Symbol) -> Result<(), String> {
        let declarations = &*symbol.declarations;
        let mut mask = 0;
        if !symbol.check_flags.is_empty() {
            mask |= S_CHECK_FLAGS;
        }
        if symbol.members.is_some() {
            mask |= S_MEMBERS;
        }
        if symbol.exports.is_some() {
            mask |= S_EXPORTS;
        }
        if symbol.parent.is_some() {
            mask |= S_PARENT;
        }
        if symbol.export_symbol.is_some() {
            mask |= S_EXPORT_SYMBOL;
        }
        if symbol.value_declaration.is_some() {
            if declarations.first() == Some(&symbol.value_declaration) {
                mask |= S_VALUE_IS_FIRST;
            } else {
                mask |= S_VALUE_DECLARATION;
            }
        }
        if declarations.len() == 1 {
            mask |= S_ONE_DECLARATION;
        }
        self.u32(symbol.flags.0);
        self.name(&symbol.name);
        self.u8(mask);
        if mask & S_CHECK_FLAGS != 0 {
            self.u32(symbol.check_flags.0);
        }
        if mask & S_MEMBERS != 0 {
            self.u32(symbol.members.0);
        }
        if mask & S_EXPORTS != 0 {
            self.u32(symbol.exports.0);
        }
        if mask & S_PARENT != 0 {
            self.u32(symbol.parent.0);
        }
        if mask & S_EXPORT_SYMBOL != 0 {
            self.u32(symbol.export_symbol.0);
        }
        if mask & S_VALUE_DECLARATION != 0 {
            self.node(symbol.value_declaration)?;
        }
        if mask & S_ONE_DECLARATION == 0 {
            self.count(declarations.len())?;
        }
        for &declaration in declarations {
            self.node(declaration)?;
        }
        Ok(())
    }

    fn node_bind_data(&mut self, data: &NodeBindData) -> Result<(), String> {
        let mut mask = 0;
        let fields = [
            (data.symbol.is_some(), N_SYMBOL),
            (data.local_symbol.is_some(), N_LOCAL_SYMBOL),
            (data.locals.is_some(), N_LOCALS),
            (data.next_container.is_some(), N_NEXT_CONTAINER),
            (data.flow_node.is_some(), N_FLOW_NODE),
            (data.end_flow_node.is_some(), N_END_FLOW_NODE),
            (data.return_flow_node.is_some(), N_RETURN_FLOW_NODE),
            (!data.added_flags.is_empty(), N_ADDED_FLAGS),
        ];
        for (set, bit) in fields {
            if set {
                mask |= bit;
            }
        }
        self.u8(mask);
        if mask & N_SYMBOL != 0 {
            self.u32(data.symbol.0);
        }
        if mask & N_LOCAL_SYMBOL != 0 {
            self.u32(data.local_symbol.0);
        }
        if mask & N_LOCALS != 0 {
            self.u32(data.locals.0);
        }
        if mask & N_NEXT_CONTAINER != 0 {
            self.node(data.next_container)?;
        }
        if mask & N_FLOW_NODE != 0 {
            self.flow(data.flow_node)?;
        }
        if mask & N_END_FLOW_NODE != 0 {
            self.flow(data.end_flow_node)?;
        }
        if mask & N_RETURN_FLOW_NODE != 0 {
            self.flow(data.return_flow_node)?;
        }
        if mask & N_ADDED_FLAGS != 0 {
            self.u32(data.added_flags.0);
        }
        Ok(())
    }

    fn flow_node(&mut self, flow: &FlowNode) -> Result<(), String> {
        let mut mask = 0;
        if flow.node.is_some() {
            mask |= F_NODE;
        }
        if flow.antecedent.is_some() {
            mask |= F_ANTECEDENT;
        }
        if !flow.antecedents.is_empty() {
            mask |= F_ANTECEDENTS;
        }
        self.u32(flow.flags.0);
        self.u8(mask);
        if mask & F_NODE != 0 {
            self.node(flow.node)?;
        }
        if mask & F_ANTECEDENT != 0 {
            self.flow(flow.antecedent)?;
        }
        if mask & F_ANTECEDENTS != 0 {
            self.count(flow.antecedents.len())?;
            let raw = flow.flags.intersects(FlowFlags::SWITCH_CLAUSE);
            for &antecedent in &flow.antecedents {
                if raw {
                    let value = u32::try_from(antecedent.0)
                        .map_err(|_| format!("switch clause value {antecedent:?}"))?;
                    self.u32(value);
                } else {
                    self.flow(antecedent)?;
                }
            }
        }
        Ok(())
    }

    fn diagnostics(&mut self, list: &[Diagnostic]) -> Result<(), String> {
        self.count(list.len())?;
        for diagnostic in list {
            if diagnostic.repopulate_info.is_some() {
                return Err("a bind diagnostic with repopulate info".into());
            }
            self.node(diagnostic.file)?;
            self.u32(diagnostic.pos as u32);
            self.u32(diagnostic.end as u32);
            self.u32(diagnostic.code as u32);
            self.u8(diagnostic.category.0 as u8);
            let key = diagnostic.message.key();
            if crate::diagnostics::message_by_key(key) != Some(diagnostic.message) {
                return Err(format!("message key {key} does not find its message"));
            }
            self.str(key)?;
            self.count(diagnostic.message_args.len())?;
            for arg in &diagnostic.message_args {
                self.str(arg)?;
            }
            self.diagnostics(&diagnostic.message_chain)?;
            self.diagnostics(&diagnostic.related_information)?;
            let mut bits = 0;
            if diagnostic.reports_unnecessary {
                bits |= D_REPORTS_UNNECESSARY;
            }
            if diagnostic.reports_deprecated {
                bits |= D_REPORTS_DEPRECATED;
            }
            if diagnostic.skipped_on_no_emit {
                bits |= D_SKIPPED_ON_NO_EMIT;
            }
            self.u8(bits);
        }
        Ok(())
    }

    fn file_bind(&mut self, data: &FileBindData) -> Result<(), String> {
        self.diagnostics(&data.bind_diagnostics)?;
        self.u32(data.symbol_count as u32);
        self.count(data.pattern_ambient_modules.len())?;
        for module in &data.pattern_ambient_modules {
            self.str(&module.pattern_prefix)?;
            self.str(&module.pattern_suffix)?;
            self.u32(module.symbol.0);
        }
        self.u32(data.global_exports.0);
        self.u32(data.js_global_augmentations.0);
        self.node(data.common_js_module_indicator)?;
        self.u8(u8::from(data.has_expando_assignments));
        Ok(())
    }
}

/// The section of `file` with key `key`, for its bind output `bound` in
/// the file arena `arena`. `decode` reads it back.
fn encode(
    file: Node,
    key: &SnapshotKey,
    bound: &BoundFile,
    arena: &SymbolArena,
) -> Result<Vec<u8>, String> {
    let mut e = Encoder {
        file: file.file_index(),
        body: Vec::new(),
        names: FxHashMap::default(),
        name_list: Vec::new(),
    };
    for index in 1..arena.symbol_count() {
        e.symbol_value(arena.sym_full(SymbolId(index as u32)))?;
    }
    for index in 1..arena.table_count() {
        let table = SymbolTable(index as u32);
        e.count(arena.len(table))?;
        for (name, symbol) in arena.iter_names(table) {
            e.name(&name);
            e.u32(symbol.0);
        }
    }
    let private_names: Vec<Name> = arena.private_names().collect();
    e.count(private_names.len())?;
    for name in &private_names {
        e.name(name);
    }
    for flow in &bound.flow_nodes {
        e.flow_node(flow)?;
    }
    let compact = bound.node_bind.compact();
    let (slots, bases, entries) = compact.parts();
    e.count(slots.len())?;
    e.body.extend_from_slice(slots);
    e.count(bases.len())?;
    for &base in bases {
        e.u32(base);
    }
    e.count(entries.len())?;
    for data in entries {
        e.node_bind_data(data)?;
    }
    e.file_bind(&bound.file_bind)?;

    let mut out = Vec::with_capacity(e.body.len() + 16 * e.name_list.len() + 64);
    key.write(&mut out);
    let counts = [
        e.name_list.len(),
        arena.symbol_count() - 1,
        arena.table_count() - 1,
        bound.flow_nodes.len(),
    ];
    for count in counts {
        let count = u32::try_from(count).map_err(|_| format!("count {count} too large"))?;
        out.extend_from_slice(&count.to_le_bytes());
    }
    for name in &e.name_list {
        match name.stable_id() {
            Some(id) if id & TEXT_BIT == 0 => out.extend_from_slice(&id.to_le_bytes()),
            _ => {
                let len = u32::try_from(name.len())
                    .ok()
                    .filter(|&len| len & TEXT_BIT == 0)
                    .ok_or_else(|| format!("name of {} bytes", name.len()))?;
                out.extend_from_slice(&(TEXT_BIT | len).to_le_bytes());
                out.extend_from_slice(name.as_bytes());
            }
        }
    }
    out.extend_from_slice(&e.body);
    Ok(out)
}

/// The blob with `magic` and `sections` (lib base name, section), in this
/// order (`read_entries`).
pub(crate) fn write_blob(magic: &[u8; 8], sections: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let mut out = magic.to_vec();
    let put_len = |out: &mut Vec<u8>, len: usize| {
        out.extend_from_slice(&u32::try_from(len).expect("blob length").to_le_bytes());
    };
    put_len(&mut out, sections.len());
    for (name, section) in sections {
        put_len(&mut out, name.len());
        out.extend_from_slice(name.as_bytes());
        put_len(&mut out, section.len());
        out.extend_from_slice(section);
    }
    out
}

/// The bundled libs that get a snapshot, largest first: every lib with at
/// least `MIN_TEXT_LEN` text bytes. The lib parse snapshot uses the same
/// list.
#[cfg(test)]
pub(crate) fn snapshot_libs() -> Vec<&'static str> {
    let mut libs: Vec<(&'static str, usize)> = bundled::LIB_NAMES
        .iter()
        .filter_map(|&name| {
            let text = lib_text(name)?;
            (text.len() >= MIN_TEXT_LEN).then_some((name, text.len()))
        })
        .collect();
    libs.sort_by_key(|&(_, len)| std::cmp::Reverse(len));
    libs.into_iter().map(|(name, _)| name).collect()
}

/// The text of bundled lib `name`: the embedded text, or in a `noembed`
/// build the file in `bundled::lib_path()` (read and leaked on each call).
/// The lib snapshot tests use it, so they run on both builds.
#[cfg(test)]
pub(crate) fn lib_text(name: &str) -> Option<&'static str> {
    let path = format!("{}/{name}", bundled::lib_path());
    bundled::bundled_text(&path).or_else(|| {
        let text = std::fs::read_to_string(&path).ok()?;
        Some(Box::leak(text.into_boxed_str()))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// How to write the blob again.
    const REGENERATE: &str = "run `cargo test -p goport_core --lib \
        binder::lib_snapshot::tests::generate_lib_bind_snapshot -- --ignored --exact` \
        (through scripts/run-cargo-capped.sh)";

    /// Loads a program version that holds every snapshot lib and runs `f`
    /// on each lib file, in `snapshot_libs` order, with the program current.
    fn for_each_lib_file(label: &str, mut f: impl FnMut(&'static str, Node)) {
        let libs = snapshot_libs();
        assert!(libs.contains(&"lib.dom.d.ts"), "no snapshot for lib.dom");
        let options: Vec<String> = libs
            .iter()
            .map(|name| {
                let option = name
                    .strip_prefix("lib.")
                    .and_then(|rest| rest.strip_suffix(".d.ts"))
                    .unwrap_or_else(|| panic!("no lib option for {name}"));
                format!("\"{option}\"")
            })
            .collect();
        let dir = std::env::temp_dir().join(format!(
            "ts_goport_lib_snapshot_{label}_{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.ts"), "export {};\n").unwrap();
        std::fs::write(
            dir.join("tsconfig.json"),
            format!(
                r#"{{ "compilerOptions": {{ "lib": [{}], "types": [] }}, "files": ["a.ts"] }}"#,
                options.join(", ")
            ),
        )
        .unwrap();
        let config = dir.join("tsconfig.json");
        let program = crate::program::try_load_version(&config.to_string_lossy(), |_| {})
            .unwrap_or_else(|e| panic!("cannot load {}: {e}", config.display()));
        let _ = std::fs::remove_dir_all(&dir);
        let _scope = crate::core::enter_program(Some(program));
        for lib in libs {
            let file = program
                .source_files()
                .find(|file| bundled::bundled_lib_name(&file.info.file_name) == Some(lib))
                .unwrap_or_else(|| panic!("{lib} is not in the program"))
                .root;
            f(lib, file);
        }
    }

    /// A live bind of `file` into a new arena. It fails when the bind
    /// changes state of this thread that a snapshot load would not change.
    fn live_bind(file: Node) -> Result<(BoundFile, SymbolArena), String> {
        let before = (synthetic_slot_count(), next_ids());
        let mut arena = SymbolArena::new_file();
        let bound = bind_source_file_live(file, &mut arena);
        if (synthetic_slot_count(), next_ids()) != before {
            return Err("the live bind made synthetic nodes or ids".into());
        }
        Ok((bound, arena))
    }

    /// Writes `lib_bind.bin` from a live bind of each snapshot lib, and
    /// prints the sizes and times (times mean something only in an
    /// optimized test build).
    #[test]
    #[ignore = "writes src/binder/lib_bind.bin"]
    fn generate_lib_bind_snapshot() {
        let mut sections = Vec::new();
        for_each_lib_file("generate", |lib, file| {
            // The process-wide unported counts: run this test alone.
            let unported = unported_report();
            let start = std::time::Instant::now();
            let live = live_bind(file).unwrap_or_else(|e| panic!("{lib}: {e}"));
            let bind_time = start.elapsed();
            assert_eq!(
                unported_report(),
                unported,
                "{lib}: the live bind hit unported code, which a load would skip"
            );
            let private: Vec<Name> = live.1.private_names().collect();
            assert!(
                private.is_empty(),
                "{lib}: the lib has private class members ({private:?}). A private name holds a \
                 hash of the lib's file name (`private_name_key`), and the snapshot key does not \
                 hold the file name, so a load at another lib path (a noembed build, another lib \
                 dir) would give the old path's names. Put the file name in the key first, or \
                 key private names by something that does not depend on the path"
            );
            let key = SnapshotKey::of(file).unwrap_or_else(|| panic!("{lib}: no key"));
            let section =
                encode(file, &key, &live.0, &live.1).unwrap_or_else(|e| panic!("{lib}: {e}"));
            let start = std::time::Instant::now();
            let loaded = load_section(file, &section)
                .unwrap_or_else(|part| panic!("{lib}: the new section does not load ({part})"));
            let load_time = start.elapsed();
            assert_same_bind(lib, &live, &loaded);
            eprintln!(
                "{lib}: text {} bytes, {} slots, section {} bytes, bind {bind_time:?}, load {load_time:?}",
                source_file_text(file).len(),
                key.nodes,
                section.len()
            );
            sections.push((lib, section));
        });
        let blob = write_blob(MAGIC, &sections);
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../src/binder/lib_bind.bin");
        std::fs::write(path, &blob).unwrap_or_else(|e| panic!("cannot write {path}: {e}"));
        eprintln!("wrote {path}: {} bytes", blob.len());
    }

    /// The embedded blob holds a section for each snapshot lib that equals
    /// a live bind of it now, and its load gives that live bind.
    #[test]
    fn snapshot_matches_live_bind() {
        for_each_lib_file("compare", |lib, file| {
            let live = live_bind(file).unwrap_or_else(|e| panic!("{lib}: {e}"));
            let key = SnapshotKey::of(file).unwrap_or_else(|| panic!("{lib}: no key"));
            let section =
                encode(file, &key, &live.0, &live.1).unwrap_or_else(|e| panic!("{lib}: {e}"));
            let entry = entries()
                .iter()
                .find(|entry| entry.name == lib)
                .unwrap_or_else(|| panic!("lib_bind.bin has no {lib}: {REGENERATE}"));
            if entry.section != &section[..] {
                let stored = SnapshotKey::read(&mut SnapshotReader::new(entry.section));
                panic!(
                    "lib_bind.bin is stale for {lib} (stored key {stored:?}, key now {key:?}): \
                     {REGENERATE}"
                );
            }
            let loaded = load_section(file, entry.section)
                .unwrap_or_else(|part| panic!("{lib}: the section does not load ({part})"));
            assert_same_bind(lib, &live, &loaded);
        });
    }

    /// Asserts that `loaded` equals `live`, field by field, and that the
    /// loaded tables find each entry.
    fn assert_same_bind(
        lib: &str,
        live: &(BoundFile, SymbolArena),
        loaded: &(BoundFile, SymbolArena),
    ) {
        let (live_file, live_arena) = live;
        let (file, arena) = loaded;
        assert_eq!(
            arena.symbol_count(),
            live_arena.symbol_count(),
            "{lib}: symbol count"
        );
        for index in 1..arena.symbol_count() {
            let id = SymbolId(index as u32);
            let (a, b) = (arena.sym(id), live_arena.sym(id));
            assert!(
                a.flags == b.flags
                    && a.check_flags == b.check_flags
                    && a.name == b.name
                    && *a.declarations == *b.declarations
                    && a.value_declaration == b.value_declaration
                    && a.members == b.members
                    && a.exports == b.exports
                    && a.parent == b.parent
                    && a.export_symbol == b.export_symbol,
                "{lib}: symbol {index}: {a:?} != {b:?}"
            );
        }
        assert_eq!(
            arena.table_count(),
            live_arena.table_count(),
            "{lib}: table count"
        );
        for index in 1..arena.table_count() {
            let table = SymbolTable(index as u32);
            let entries = live_arena.entries(table);
            assert_eq!(arena.entries(table), entries, "{lib}: table {index}");
            for (name, symbol) in &entries {
                assert_eq!(arena.get_name(table, name), *symbol, "{lib}: {name:?}");
                assert_eq!(arena.get(table, name), *symbol, "{lib}: {name:?}");
            }
        }
        assert_eq!(
            arena.private_names().collect::<Vec<_>>(),
            live_arena.private_names().collect::<Vec<_>>(),
            "{lib}: private names"
        );
        assert!(
            file.node_bind.compact().parts() == live_file.node_bind.compact().parts(),
            "{lib}: node data"
        );
        assert_eq!(
            file.flow_nodes.len(),
            live_file.flow_nodes.len(),
            "{lib}: flow node count"
        );
        for (index, (a, b)) in file
            .flow_nodes
            .iter()
            .zip(&live_file.flow_nodes)
            .enumerate()
        {
            assert!(
                a.flags == b.flags
                    && a.node == b.node
                    && a.antecedent == b.antecedent
                    && a.antecedents == b.antecedents,
                "{lib}: flow node {index}: {a:?} != {b:?}"
            );
        }
        assert_eq!(
            format!("{:?}", file.file_bind),
            format!("{:?}", live_file.file_bind),
            "{lib}: file data"
        );
    }
}
