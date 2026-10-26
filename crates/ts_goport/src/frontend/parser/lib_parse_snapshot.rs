//! Rust-only: the lib parse snapshot (perf9 round 3, R3-1).
//!
//! PERF: at 16 cores the parse of lib.dom is the parse critical path of
//! query (about 50 M cycles). The parse output of the large bundled lib
//! files is embedded in `lib_parse.bin`. `parse_source_file` and
//! `parse_source_file_detached` load it into the new store of the file
//! instead of parsing, when the key matches, and freeze the store as a
//! parse does. So adoption, publish and `files_parser.rs` do not change.
//! The loaded store equals the store of a live parse: the same slots, with
//! the same headers, node data, names and R2-5 links, and the same tables
//! that the U1 and U4 build entries and the freeze make from them. The
//! `ParsedSourceFile` fields and the two `DetachedParse` fields are stored
//! too.
//!
//! The key of a lib file (`ParseKey`) is a compile-time hash of the
//! parser, scanner, factory, store and node sources, astdata and this file
//! (`SOURCES_HASH`), a hash of the parse options that the parse reads (file
//! name, script kind, module indicator options) and two hashes of the text
//! (xxh3, and `const_hash`, which the embed build has from compile time;
//! see `lib_snapshot::text_matches`). Any mismatch or load error parses the file live, so a stale blob
//! costs time, not output. A lib file is found by its base name
//! (`bundled_lib_name`), so the embed and noembed builds share the blob.
//!
//! The load leaves the state of the thread as a live parse does: the names
//! of the file are interned in the order in which the parse interns them
//! (first use, in slot order), and it makes no synthetic node and no node
//! or symbol id (a lib parse makes none either; the tests check this).
//!
//! After a change to the parser, the scanner, the factory, the store,
//! astdata or a bundled lib, write the blob again (the integrator does this
//! after each merge round, before the lib bind generator):
//!
//! ```text
//! cargo test -p goport_core --lib frontend::parser::lib_parse_snapshot::tests::generate_lib_parse_snapshot -- --ignored --exact
//! ```
//!
//! `snapshot_matches_live_parse` fails while the blob is stale.
//! `GOPORT_LIB_PARSE_SNAPSHOT=0` turns the snapshot off (for A/B timing),
//! and `GOPORT_LIB_PARSE_SNAPSHOT=trace` writes one line per lib file to
//! stderr.
//!
//! Blob layout: `MAGIC`, then the sections as `lib_snapshot::write_blob`
//! writes them. A section is the key (four u64, little-endian), one byte
//! for `read_module_indicator_options`, then LEB128 varints ("zz" is the
//! zigzag form of a signed value):
//! - the slot count (slots after slot 0);
//! - the (kind, flags) palette: a count, then each kind and flags;
//! - the names: a count, then each as `lib_snapshot` stores a name (a u32
//!   stable id, or `TEXT_BIT` | length and the text);
//! - each slot `i`: its palette index, zz(pos - pos of slot i - 1),
//!   zz(end - pos), zz(parent - i), the two links (`link`), the name index
//!   for an Identifier or PrivateIdentifier, and the data: a variant tag
//!   (`SHARED_NAME` for the shared name node) and the fields in astdata
//!   order (`payload_codec!`). A child id is zz(id - i); a list range is
//!   zz(start - pos) and zz(end - start);
//! - the `ParsedSourceFile` fields in declaration order, with node handles
//!   as slot indexes (0 is nil), and the import specifiers.

use crate::ast::store::{
    LibListTable, LibParseSlot, load_lib_parse_slots, mark_lib_parse_snapshot_store,
    reset_file_store,
};
#[cfg(test)]
use crate::ast::store::{lib_parse_slot_views, lib_parse_store_dump};
use crate::astdata::NodeData;
use crate::binder::lib_snapshot::{
    MIN_TEXT_LEN, Mode, SnapshotEntry, SnapshotReader, TEXT_BIT, const_hash, mix, read_entries,
    text_matches,
};
use crate::frontend::prelude::*;
use std::sync::OnceLock;
use xxhash_rust::xxh3::{Xxh3, xxh3_64};

/// The embedded snapshot, written by `generate_lib_parse_snapshot`.
static BLOB: &[u8] = include_bytes!("lib_parse.bin");

/// The first bytes of `BLOB`.
const MAGIC: &[u8; 8] = b"TSLIBPRS";

/// The variant tag of a slot that points at the shared name node of its
/// kind (S1). The `payload_codec!` tags are below it.
const SHARED_NAME: u8 = u8::MAX;

// ──────────────────────────────────────────────────────────────────────
// Key
// ──────────────────────────────────────────────────────────────────────

/// A compile-time hash of the sources whose change can change the parse
/// output or the blob format: the parser, the scanner, the node factory,
/// the store and node reads, `core.rs` (names), the flag values, the lib
/// name ids, astdata, the blob helpers of `lib_snapshot.rs` and this file.
/// After a change there, every key misses until the blob is written again.
// PORT: the parser also calls helpers in other files (ast utilities, for
// example). `snapshot_matches_live_parse` finds a blob that such a change
// made stale.
const SOURCES_HASH: u64 = {
    let hashes = [
        SOURCE_PARSER_P1,
        SOURCE_PARSER_P2,
        SOURCE_PARSER_P3,
        SOURCE_PARSER_P4,
        SOURCE_PARSER_P5,
        SOURCE_JSDOC,
        SOURCE_REPARSER,
        SOURCE_REFERENCES,
        SOURCE_SOURCE_FILE,
        SOURCE_PARSER_UTILITIES,
        SOURCE_SCANNER_P1,
        SOURCE_SCANNER_P2,
        SOURCE_COMMENT_RANGES,
        SOURCE_FACTORY,
        SOURCE_FACTORY_P2,
        SOURCE_STORE,
        SOURCE_NODE,
        SOURCE_CORE,
        SOURCE_FLAGS,
        SOURCE_LIB_NAMES,
        SOURCE_ASTDATA,
        SOURCE_ASTDATA_MOD,
        SOURCE_LIB_SNAPSHOT,
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
const SOURCE_PARSER_P1: u64 = const_hash(include_bytes!("parser_p1.rs"));
const SOURCE_PARSER_P2: u64 = const_hash(include_bytes!("parser_p2.rs"));
const SOURCE_PARSER_P3: u64 = const_hash(include_bytes!("parser_p3.rs"));
const SOURCE_PARSER_P4: u64 = const_hash(include_bytes!("parser_p4.rs"));
const SOURCE_PARSER_P5: u64 = const_hash(include_bytes!("parser_p5.rs"));
const SOURCE_JSDOC: u64 = const_hash(include_bytes!("jsdoc.rs"));
const SOURCE_REPARSER: u64 = const_hash(include_bytes!("reparser.rs"));
const SOURCE_REFERENCES: u64 = const_hash(include_bytes!("references.rs"));
const SOURCE_SOURCE_FILE: u64 = const_hash(include_bytes!("source_file.rs"));
const SOURCE_PARSER_UTILITIES: u64 = const_hash(include_bytes!("utilities.rs"));
const SOURCE_SCANNER_P1: u64 = const_hash(include_bytes!("../scanner/scanner_p1.rs"));
const SOURCE_SCANNER_P2: u64 = const_hash(include_bytes!("../scanner/scanner_p2.rs"));
const SOURCE_COMMENT_RANGES: u64 = const_hash(include_bytes!("../scanner/comment_ranges.rs"));
const SOURCE_FACTORY: u64 = const_hash(include_bytes!("../../ast/factory.rs"));
const SOURCE_FACTORY_P2: u64 = const_hash(include_bytes!("../ast_factory_p2.rs"));
const SOURCE_STORE: u64 = const_hash(include_bytes!("../../ast/store.rs"));
const SOURCE_NODE: u64 = const_hash(include_bytes!("../../ast/node.rs"));
const SOURCE_CORE: u64 = const_hash(include_bytes!("../../core.rs"));
const SOURCE_FLAGS: u64 = const_hash(include_bytes!("../../flags.rs"));
const SOURCE_LIB_NAMES: u64 = const_hash(include_bytes!("../../core/lib_names.rs"));
const SOURCE_ASTDATA: u64 = const_hash(include_bytes!("../../astdata/ast_generated.rs"));
const SOURCE_ASTDATA_MOD: u64 = const_hash(include_bytes!("../../astdata/mod.rs"));
const SOURCE_LIB_SNAPSHOT: u64 = const_hash(include_bytes!("../../binder/lib_snapshot.rs"));
const SOURCE_SNAPSHOT: u64 = const_hash(include_bytes!("lib_parse_snapshot.rs"));

/// What must match for a lib file to load its section.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ParseKey {
    /// `SOURCES_HASH` of the build that wrote the section.
    sources: u64,
    /// `options_hash` of the parse.
    options: u64,
    /// xxh3 of the file text.
    text: u64,
    /// `const_hash` of the file text (`text_matches`).
    text_const: u64,
}

impl ParseKey {
    fn of(opts: &SourceFileParseOptions, text: &str, script_kind: ScriptKind) -> Self {
        ParseKey {
            sources: SOURCES_HASH,
            options: options_hash(opts, script_kind),
            text: xxh3_64(text.as_bytes()),
            text_const: const_hash(text.as_bytes()),
        }
    }

    fn write(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.sources.to_le_bytes());
        out.extend_from_slice(&self.options.to_le_bytes());
        out.extend_from_slice(&self.text.to_le_bytes());
        out.extend_from_slice(&self.text_const.to_le_bytes());
    }

    fn read(r: &mut SnapshotReader<'_>) -> Option<Self> {
        Some(ParseKey {
            sources: r.u64()?,
            options: r.u64()?,
            text: r.u64()?,
            text_const: r.u64()?,
        })
    }

    /// Compares this stored key with a parse, cheapest part first, so a
    /// stale blob does not hash the text. The error names the first part
    /// that differs.
    fn check(
        &self,
        opts: &SourceFileParseOptions,
        text: &str,
        script_kind: ScriptKind,
    ) -> Result<(), &'static str> {
        if self.sources != SOURCES_HASH {
            return Err("sources");
        }
        if self.options != options_hash(opts, script_kind) {
            return Err("options");
        }
        if !text_matches(self.text, self.text_const, text) {
            return Err("text");
        }
        Ok(())
    }
}

/// An xxh3 hash of what the parse reads from its options besides the text:
/// the file name (`is_declaration_file_name`, the store file name), the
/// script kind and the module indicator options. The path is not read; the
/// load takes the options of the caller, as the parse does.
/// For a lib file only its base name is hashed: the parse reads only the
/// name's extension, so the embed name (`bundled:///libs/lib.dom.d.ts`) and
/// the noembed path (`<lib dir>/lib.dom.d.ts`) load the same section.
fn options_hash(opts: &SourceFileParseOptions, script_kind: ScriptKind) -> u64 {
    let name = bundled_lib_name(&opts.file_name).unwrap_or(&opts.file_name);
    let mut hasher = Xxh3::new();
    hasher.update(&(name.len() as u64).to_le_bytes());
    hasher.update(name.as_bytes());
    hasher.update(&script_kind.0.to_le_bytes());
    let indicator = opts.external_module_indicator_options;
    hasher.update(&[u8::from(indicator.jsx), u8::from(indicator.force)]);
    hasher.digest()
}

// ──────────────────────────────────────────────────────────────────────
// Load
// ──────────────────────────────────────────────────────────────────────

/// `GOPORT_LIB_PARSE_SNAPSHOT`: "0" turns the snapshot off, "trace" traces
/// it.
fn mode() -> Mode {
    static MODE: OnceLock<Mode> = OnceLock::new();
    *MODE.get_or_init(|| Mode::from_env("GOPORT_LIB_PARSE_SNAPSHOT"))
}

/// The lib files in `BLOB`, read once. Empty when the blob does not read.
fn entries() -> &'static [SnapshotEntry] {
    static ENTRIES: OnceLock<Vec<SnapshotEntry>> = OnceLock::new();
    ENTRIES.get_or_init(|| read_entries(BLOB, MAGIC).unwrap_or_default())
}

#[cfg(test)]
thread_local! {
    /// Set while a test parses live (`tests::live_parse`).
    static LIVE_ONLY: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// The snapshot loads on this thread.
    static LOADS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// A snapshot parse: the file, and the `DetachedParse` field that is not
/// in it. `read_module_indicator_options` is always false (see `load`).
pub(crate) struct LoadedParse {
    pub(crate) file: ParsedSourceFile,
    pub(crate) import_specifiers: Vec<String>,
}

/// The parse of `source_text` from the snapshot, into store `store`: the
/// store that the caller just made for this parse on this thread
/// (`new_file_store` or `new_detached_file_store`). The store is frozen, as
/// `Parser::parse_into_store` leaves it. `None` (parse live) when the file
/// is not a snapshot lib, the key does not match or the section does not
/// load; the store is then still empty.
// PORT: a snapshot keeps only a parse that did not read the module
// indicator options, so a detached load reports false as the parse would.
// A declaration file (every bundled lib) never reads them.
pub(crate) fn load(
    store: usize,
    opts: &SourceFileParseOptions,
    source_text: &'static str,
    script_kind: ScriptKind,
) -> Option<LoadedParse> {
    let mode = mode();
    if mode == Mode::Off || source_text.len() < MIN_TEXT_LEN {
        return None;
    }
    #[cfg(test)]
    if LIVE_ONLY.get() {
        return None;
    }
    let lib = bundled_lib_name(&opts.file_name)?;
    let entry = entries().iter().find(|entry| entry.name == lib)?;
    let start = (mode == Mode::Trace).then(std::time::Instant::now);
    let result = load_section(store, entry.section, opts, source_text, script_kind);
    if let Some(start) = start {
        let status = match &result {
            Ok(_) => "loaded",
            Err(part) => *part,
        };
        eprintln!(
            "goport lib parse snapshot: {lib} {status} {} us",
            start.elapsed().as_micros()
        );
    }
    #[cfg(test)]
    if result.is_ok() {
        LOADS.set(LOADS.get() + 1);
    }
    result.ok()
}

/// Checks the key in `section` against the parse and loads the section
/// into `store`. The error is the key part that differs
/// (`ParseKey::check`), "options read" for a parse that read the module
/// indicator options, or "load" for a section that does not decode. On an
/// error the store is empty.
fn load_section(
    store: usize,
    section: &[u8],
    opts: &SourceFileParseOptions,
    text: &'static str,
    script_kind: ScriptKind,
) -> Result<LoadedParse, &'static str> {
    let mut r = SnapshotReader::new(section);
    let key = ParseKey::read(&mut r).ok_or("load")?;
    key.check(opts, text, script_kind)?;
    match r.u8() {
        Some(0) => {}
        Some(1) => return Err("options read"),
        _ => return Err("load"),
    }
    match decode(store, r, opts, text) {
        Some(loaded) => Ok(loaded),
        None => {
            reset_file_store(store);
            Err("load")
        }
    }
}

/// Decodes a section body into `store` and freezes it. On `None` the store
/// may hold part of the slots, and nothing else was written.
fn decode(
    store: usize,
    r: SnapshotReader<'_>,
    opts: &SourceFileParseOptions,
    text: &'static str,
) -> Option<LoadedParse> {
    let mut d = Decoder {
        r,
        store,
        total: 1,
        index: 0,
        pos: 0,
        ids: Vec::new(),
    };
    let count = d.count()?;
    d.total = u32::try_from(count).ok()?.checked_add(1)?;
    let palette_len = d.count()?;
    let mut palette = Vec::with_capacity(palette_len);
    for _ in 0..palette_len {
        let kind = d.kind()?;
        palette.push((kind, NodeFlags(d.num()?)));
    }
    // The names in the order of their first use, which is the order in
    // which the parse interns them (`slot_text_name`).
    let name_count = d.count()?;
    let mut names = Vec::with_capacity(name_count);
    for _ in 0..name_count {
        let entry = d.r.u32()?;
        let name = if entry & TEXT_BIT == 0 {
            Name::from_stable_id(entry)?
        } else {
            Name::from(d.r.text((entry & !TEXT_BIT) as usize)?)
        };
        let text_is_keyword = get_identifier_token(name.as_str()) != SyntaxKind::Identifier;
        names.push((name, text_is_keyword));
    }
    if !load_lib_parse_slots(store, count, |lists| d.slot(&palette, &names, lists)) {
        return None;
    }
    let file = d.parsed_file(opts, text)?;
    let import_specifiers = d.strings()?;
    if d.r.remaining() != 0 {
        return None;
    }
    mark_lib_parse_snapshot_store(store);
    // The store writes of Go `finishSourceFile` (parser_p1.rs), then the
    // freeze of `parse_into_store`.
    set_file_store_js_doc_cache(store, &file.jsdoc_cache);
    set_file_store_parse_fields(store, file.language_variant, &file.diagnostics);
    if file.has_lazy_js_doc {
        set_file_store_lazy_js_doc(store, &file.parse_options, file.script_kind);
    }
    freeze_file_store(store);
    Some(LoadedParse {
        file,
        import_specifiers,
    })
}

/// The signed value of zigzag form `value`.
#[inline]
fn unzigzag(value: u64) -> i64 {
    (value >> 1) as i64 ^ -((value & 1) as i64)
}

/// The zigzag form of `value`: 0, -1, 1, -2, ... become 0, 1, 2, 3, ...
#[cfg(test)]
fn zigzag(value: i64) -> u64 {
    ((value << 1) ^ (value >> 63)) as u64
}

/// Reads a section body. Each id, slot and count is checked, so a bad
/// section fails here, not in a later read.
struct Decoder<'a> {
    r: SnapshotReader<'a>,
    /// The store id, the high half of each node handle.
    store: usize,
    /// The slot count of the store, slot 0 included.
    total: u32,
    /// The slot being read; child ids are relative to it.
    index: u32,
    /// `loc.pos()` of the slot being read (of the one before, until its
    /// position is read); list ranges are relative to it.
    pos: i64,
    /// The ids of the list being read (`list_parts`), kept between lists.
    ids: Vec<crate::astdata::NodeId>,
}

/// The `Decoder` of one node data, with the list table of the store, which
/// the list readers add to (astshrink1). The other field readers are the
/// `Decoder` methods (`Deref`).
struct PayloadDecoder<'d, 'a, 't, 'l> {
    dec: &'d mut Decoder<'a>,
    lists: &'t mut LibListTable<'l>,
}

impl<'a> std::ops::Deref for PayloadDecoder<'_, 'a, '_, '_> {
    type Target = Decoder<'a>;

    fn deref(&self) -> &Decoder<'a> {
        self.dec
    }
}

impl std::ops::DerefMut for PayloadDecoder<'_, '_, '_, '_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.dec
    }
}

impl PayloadDecoder<'_, '_, '_, '_> {
    fn list(&mut self) -> Option<crate::astdata::NodeList> {
        let (range, comma) = self.dec.list_parts()?;
        Some(
            self.lists
                .push(range, &self.dec.ids, comma, Default::default()),
        )
    }

    fn olist(&mut self) -> Option<Option<crate::astdata::NodeList>> {
        Some(if self.flag()? {
            Some(self.list()?)
        } else {
            None
        })
    }

    fn mods(&mut self) -> Option<Option<crate::astdata::ModifierList>> {
        if !self.flag()? {
            return Some(None);
        }
        let (range, comma) = self.dec.list_parts()?;
        let flags = crate::astdata::ModifierFlags(self.num()?);
        Some(Some(crate::astdata::ModifierList {
            list: self.lists.push(range, &self.dec.ids, comma, flags),
        }))
    }
}

impl<'a> Decoder<'a> {
    // PERF (queryload1): keep the whole loop inline. The inlineout1 split
    // (`var_rest` out of line) cut `load_lib_parse_slots` from 238 KB to
    // 119 KB under PGO, but cost Query load-only +0.5 to +1.0% st
    // instructions (int104b leave-one-out).
    #[inline]
    fn var(&mut self) -> Option<u64> {
        let mut value = 0u64;
        let mut shift = 0;
        loop {
            let byte = self.r.u8()?;
            value |= u64::from(byte & 0x7f) << shift;
            if byte < 0x80 {
                return Some(value);
            }
            shift += 7;
            if shift >= 64 {
                return None;
            }
        }
    }

    #[inline]
    fn zz(&mut self) -> Option<i64> {
        Some(unzigzag(self.var()?))
    }

    /// A count of items that take at least one byte each (see
    /// `SnapshotReader::count`).
    fn count(&mut self) -> Option<usize> {
        let count = usize::try_from(self.var()?).ok()?;
        (count <= self.r.remaining()).then_some(count)
    }

    fn i32(&mut self) -> Option<i32> {
        i32::try_from(self.zz()?).ok()
    }

    /// Slot `index + delta` of the store.
    #[inline]
    fn slot_at(&self, delta: i64) -> Option<u32> {
        let slot = i64::from(self.index) + delta;
        (0..i64::from(self.total))
            .contains(&slot)
            .then_some(slot as u32)
    }

    /// A slot relative to the slot being read.
    #[inline]
    fn rel_slot(&mut self) -> Option<u32> {
        let delta = self.zz()?;
        self.slot_at(delta)
    }

    /// An R2-5 link value (`LibParseSlot::first_child`).
    #[inline]
    fn link(&mut self) -> Option<u32> {
        match self.var()? {
            0 => Some(LibParseSlot::LINK_NONE),
            1 => Some(LibParseSlot::LINK_END),
            value => self.slot_at(unzigzag(value - 2)).filter(|&slot| slot != 0),
        }
    }

    /// The next slot of the store.
    fn slot(
        &mut self,
        palette: &[(SyntaxKind, NodeFlags)],
        names: &[(Name, bool)],
        lists: &mut LibListTable<'_>,
    ) -> Option<LibParseSlot> {
        self.index += 1;
        let (kind, flags) = *palette.get(usize::try_from(self.var()?).ok()?)?;
        let pos_delta = self.zz()?;
        let pos = self.pos + pos_delta;
        let end = pos + self.zz()?;
        self.pos = pos;
        let loc = TextRange::new(i32::try_from(pos).ok()?, i32::try_from(end).ok()?);
        let parent = self.rel_slot()?;
        let first_child = self.link()?;
        let next_sibling = self.link()?;
        let is_name = matches!(kind, SyntaxKind::Identifier | SyntaxKind::PrivateIdentifier);
        let (name, text_is_keyword) = if is_name {
            names.get(usize::try_from(self.var()?).ok()?)?.clone()
        } else {
            (Name::default(), false)
        };
        let variant = self.r.u8()?;
        let data = if variant == SHARED_NAME {
            if !is_name {
                return None;
            }
            None
        } else {
            let data = decode_payload(&mut PayloadDecoder { dec: self, lists }, variant)?;
            if !data.matches_syntax_kind(kind) {
                return None;
            }
            Some(data)
        };
        Some(LibParseSlot {
            kind,
            flags,
            loc,
            parent,
            first_child,
            next_sibling,
            data,
            name,
            text_is_keyword,
        })
    }

    // The `payload_codec!` field readers.

    #[inline]
    fn id(&mut self) -> Option<crate::astdata::NodeId> {
        Some(crate::astdata::NodeId::new(self.rel_slot()?))
    }

    #[inline]
    fn oid(&mut self) -> Option<Option<crate::astdata::NodeId>> {
        match self.var()? {
            0 => Some(None),
            value => Some(Some(crate::astdata::NodeId::new(
                self.slot_at(unzigzag(value - 1))?,
            ))),
        }
    }

    /// The range and the missing-list bit of a list; its ids go to `ids`.
    fn list_parts(&mut self) -> Option<(crate::astdata::text::TextRange, bool)> {
        let start_delta = self.zz()?;
        let start = u32::try_from(self.pos + start_delta).ok()?;
        let end = u32::try_from(i64::from(start) + self.zz()?).ok()?;
        let head = self.var()?;
        let len = usize::try_from(head >> 1).ok()?;
        if len > self.r.remaining() {
            return None;
        }
        self.ids.clear();
        for _ in 0..len {
            let id = self.id()?;
            self.ids.push(id);
        }
        Some((
            crate::astdata::text::TextRange::new(
                crate::astdata::text::TextPos::new(start),
                crate::astdata::text::TextPos::new(end),
            ),
            head & 1 != 0,
        ))
    }

    fn opaque(&mut self) -> Option<crate::astdata::OpaqueValue> {
        Some(crate::astdata::OpaqueValue)
    }

    fn flag(&mut self) -> Option<bool> {
        match self.r.u8()? {
            0 => Some(false),
            1 => Some(true),
            _ => None,
        }
    }

    fn num(&mut self) -> Option<u32> {
        u32::try_from(self.var()?).ok()
    }

    fn str(&mut self) -> Option<&'a str> {
        let len = self.count()?;
        self.r.text(len)
    }

    fn string(&mut self) -> Option<String> {
        Some(self.str()?.to_owned())
    }

    fn tflags(&mut self) -> Option<crate::astdata::TokenFlags> {
        Some(crate::astdata::TokenFlags(self.num()?))
    }

    fn kind(&mut self) -> Option<SyntaxKind> {
        SyntaxKind::try_from(u16::try_from(self.var()?).ok()?).ok()
    }

    fn okind(&mut self) -> Option<Option<SyntaxKind>> {
        Some(if self.flag()? {
            Some(self.kind()?)
        } else {
            None
        })
    }

    fn strings(&mut self) -> Option<Vec<String>> {
        let len = self.count()?;
        let mut list = Vec::with_capacity(len);
        for _ in 0..len {
            list.push(self.string()?);
        }
        Some(list)
    }

    fn ids(&mut self) -> Option<Vec<crate::astdata::NodeId>> {
        let len = self.count()?;
        let mut list = Vec::with_capacity(len);
        for _ in 0..len {
            list.push(self.id()?);
        }
        Some(list)
    }

    fn oids(&mut self) -> Option<Option<Vec<crate::astdata::NodeId>>> {
        Some(if self.flag()? {
            Some(self.ids()?)
        } else {
            None
        })
    }

    // The `ParsedSourceFile` readers.

    /// A node handle of the store, stored as its slot (0 is nil).
    fn node(&mut self) -> Option<Node> {
        let slot = self.num()?;
        if slot == 0 {
            return Some(Node::NIL);
        }
        (slot < self.total).then(|| {
            // `store::handle`: the high half is the store id, the low half
            // is the slot index + 1.
            Node(((self.store as u64) << 32) | (u64::from(slot) + 1))
        })
    }

    fn nodes(&mut self) -> Option<Vec<Node>> {
        let len = self.count()?;
        let mut list = Vec::with_capacity(len);
        for _ in 0..len {
            list.push(self.node()?);
        }
        Some(list)
    }

    fn range(&mut self) -> Option<TextRange> {
        let pos = self.i32()?;
        Some(TextRange::new(pos, self.i32()?))
    }

    fn diagnostics(&mut self) -> Option<Vec<Diagnostic>> {
        let len = self.count()?;
        let mut list = Vec::with_capacity(len);
        for _ in 0..len {
            list.push(self.diagnostic()?);
        }
        Some(list)
    }

    fn diagnostic(&mut self) -> Option<Diagnostic> {
        let file = self.node()?;
        let pos = self.i32()?;
        let end = self.i32()?;
        let code = self.i32()?;
        let category = match self.r.u8()? {
            0 => crate::diagnostics::Category::Warning,
            1 => crate::diagnostics::Category::Error,
            2 => crate::diagnostics::Category::Suggestion,
            3 => crate::diagnostics::Category::Message,
            _ => return None,
        };
        let message = crate::diagnostics::message_by_key(self.str()?)?;
        let message_args = self.strings()?;
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
            reports_unnecessary: bits & 1 != 0,
            reports_deprecated: bits & 2 != 0,
            skipped_on_no_emit: bits & 4 != 0,
            repopulate_info: None,
        })
    }

    fn tristate(&mut self) -> Option<Tristate> {
        match self.r.u8()? {
            0 => Some(Tristate::Unknown),
            1 => Some(Tristate::False),
            2 => Some(Tristate::True),
            _ => None,
        }
    }

    fn comment_directives(&mut self) -> Option<Vec<CommentDirective>> {
        let len = self.count()?;
        let mut list = Vec::with_capacity(len);
        for _ in 0..len {
            list.push(CommentDirective {
                loc: self.range()?,
                kind: CommentDirectiveKind(self.i32()?),
            });
        }
        Some(list)
    }

    fn jsdoc_cache(&mut self) -> Option<FxHashMap<Node, Vec<Node>>> {
        let len = self.count()?;
        let mut cache = FxHashMap::default();
        cache.reserve(len);
        for _ in 0..len {
            let node = self.node()?;
            let jsdocs = self.nodes()?;
            cache.insert(node, jsdocs);
        }
        Some(cache)
    }

    fn pragmas(&mut self) -> Option<Vec<Pragma>> {
        let len = self.count()?;
        let mut list = Vec::with_capacity(len);
        for _ in 0..len {
            let name = self.string()?;
            let arg_count = self.count()?;
            let mut args = IndexMap::new();
            for _ in 0..arg_count {
                let key = self.string()?;
                let arg = PragmaArgument {
                    name: self.string()?,
                    value: self.string()?,
                    range: self.range()?,
                };
                args.insert(key, arg);
            }
            list.push(Pragma {
                name,
                args,
                range: self.range()?,
                kind: self.kind()?,
            });
        }
        Some(list)
    }

    fn file_references(&mut self) -> Option<Vec<FileReference>> {
        let len = self.count()?;
        let mut list = Vec::with_capacity(len);
        for _ in 0..len {
            list.push(FileReference {
                range: self.range()?,
                file_name: self.string()?,
                resolution_mode: ModuleKind(self.i32()?),
                preserve: self.flag()?,
            });
        }
        Some(list)
    }

    fn check_js_directive(&mut self) -> Option<Option<CheckJsDirective>> {
        if !self.flag()? {
            return Some(None);
        }
        Some(Some(CheckJsDirective {
            enabled: self.flag()?,
            range: self.range()?,
        }))
    }

    fn usize(&mut self) -> Option<usize> {
        usize::try_from(self.var()?).ok()
    }

    /// The `ParsedSourceFile` fields, in declaration order (`encode_file`).
    fn parsed_file(
        &mut self,
        opts: &SourceFileParseOptions,
        text: &'static str,
    ) -> Option<ParsedSourceFile> {
        // Struct fields are read in the order they are written here.
        Some(ParsedSourceFile {
            store: self.store,
            root: self.node()?,
            parse_options: opts.clone(),
            text: FileText::Static(text),
            end_of_file_token: self.node()?,
            hash: std::cell::Cell::new(None),
            diagnostics: self.diagnostics()?,
            js_diagnostics: self.diagnostics()?,
            jsdoc_diagnostics: self.diagnostics()?,
            language_variant: LanguageVariant(self.i32()?),
            script_kind: ScriptKind(self.i32()?),
            is_declaration_file: self.flag()?,
            uses_uri_style_node_core_modules: self.tristate()?,
            identifier_count: self.zz()?,
            imports: self.nodes()?,
            module_augmentations: self.nodes()?,
            ambient_module_names: self.strings()?,
            comment_directives: self.comment_directives()?,
            jsdoc_cache: self.jsdoc_cache()?,
            has_lazy_js_doc: self.flag()?,
            reparsed_clones: self.nodes()?,
            pragmas: self.pragmas()?,
            referenced_files: self.file_references()?,
            type_reference_directives: self.file_references()?,
            lib_reference_directives: self.file_references()?,
            check_js_directive: self.check_js_directive()?,
            node_count: self.usize()?,
            text_count: self.usize()?,
            common_js_module_indicator: self.node()?,
            external_module_indicator: self.node()?,
            has_decorated_parameter: self.flag()?,
            // A lib file is never freed.
            version: std::cell::OnceCell::new(),
        })
    }
}

// ──────────────────────────────────────────────────────────────────────
// Node data
// ──────────────────────────────────────────────────────────────────────

/// Makes `decode_payload` and (tests) `encode_payload`, which read and
/// write the node data of one slot: the variant tag, then each field with
/// the `Decoder` (`Encoder`) method named by its codec. The table lists
/// every `NodeData` variant and every field of its astdata struct, in
/// astdata order, so a new variant or field in astdata does not compile here (the
/// match must be exhaustive, the struct pattern and literal must name every
/// field). `src/astdata/ast_generated.rs` is in `SOURCES_HASH`.
macro_rules! payload_codec {
    ($($variant_tag:literal $variant:ident($data:ident) {
        $($field:ident: $codec:ident),* $(,)?
    })*) => {
        /// The node data with variant tag `variant_tag`, from `dec`, in the
        /// AST arena of this thread (astchunks2: no box per node).
        fn decode_payload(dec: &mut PayloadDecoder<'_, '_, '_, '_>, variant_tag: u8) -> Option<NodeData<'static>> {
            Some(match variant_tag {
                $($variant_tag => NodeData::$variant(crate::ast::leak_in_ast_arena(crate::astdata::$data {
                    $($field: dec.$codec()?,)*
                })),)*
                _ => return None,
            })
        }

        /// Writes the variant tag and the fields of `payload`.
        #[cfg(test)]
        fn encode_payload(enc: &mut Encoder, payload: NodeData<'_>) -> Result<(), String> {
            match payload {
                $(NodeData::$variant(payload) => {
                    let crate::astdata::$data { $($field),* } = payload;
                    enc.u8($variant_tag);
                    $(enc.$codec($field)?;)*
                })*
            }
            Ok(())
        }
    };
}

// The rows come from `src/astdata/ast_generated.rs` (one per
// `NodeData` variant, in enum order; the codec of each field follows from
// its type). The tags stay below `SHARED_NAME`.
payload_codec! {
    0 ArrayLiteralExpression(ArrayLiteralExpressionData) { elements: list, multi_line: flag }
    1 ArrayTypeNode(ArrayTypeNodeData) { element_type: id }
    2 ArrowFunction(ArrowFunctionData) {
        asterisk_token: oid, body: id, equals_greater_than_token: id, full_signature: oid,
        parameters: list, type_: oid, type_parameters: olist, modifiers: mods,
    }
    3 AsExpression(AsExpressionData) { expression: id, type_: id }
    4 AwaitExpression(AwaitExpressionData) { expression: id }
    5 BigIntLiteral(BigIntLiteralData) { text: string, token_flags: tflags }
    6 BinaryExpression(BinaryExpressionData) {
        left: id, operator_token: id, right: id, type_: oid, modifiers: mods,
    }
    7 BindingElement(BindingElementData) {
        dot_dot_dot_token: oid, initializer: oid, property_name: oid, name: oid,
    }
    8 BindingPattern(BindingPatternData) { elements: list }
    9 Block(BlockData) { multi_line: flag, statements: list }
    10 BreakStatement(BreakStatementData) { label: oid }
    11 CallExpression(CallExpressionData) {
        arguments: list, expression: id, question_dot_token: oid, type_arguments: olist,
    }
    12 CallSignatureDeclaration(CallSignatureDeclarationData) {
        full_signature: oid, parameters: list, type_: oid, type_parameters: olist,
    }
    13 CaseBlock(CaseBlockData) { clauses: list }
    14 CaseOrDefaultClause(CaseOrDefaultClauseData) { expression: id, statements: list }
    15 CatchClause(CatchClauseData) { block: id, variable_declaration: oid }
    16 ClassDeclaration(ClassDeclarationData) {
        heritage_clauses: olist, members: list, type_parameters: olist, modifiers: mods, name: oid,
    }
    17 ClassExpression(ClassExpressionData) {
        heritage_clauses: olist, members: list, type_parameters: olist, modifiers: mods, name: oid,
    }
    18 ClassStaticBlockDeclaration(ClassStaticBlockDeclarationData) { body: id, modifiers: mods }
    19 ComputedPropertyName(ComputedPropertyNameData) { expression: id }
    20 ConditionalExpression(ConditionalExpressionData) {
        colon_token: id, condition: id, question_token: id, when_false: id, when_true: id,
    }
    21 ConditionalTypeNode(ConditionalTypeNodeData) {
        check_type: id, extends_type: id, false_type: id, true_type: id,
    }
    22 ConstructSignatureDeclaration(ConstructSignatureDeclarationData) {
        full_signature: oid, parameters: list, type_: oid, type_parameters: olist,
    }
    23 ConstructorDeclaration(ConstructorDeclarationData) {
        asterisk_token: oid, body: oid, full_signature: oid, parameters: list, type_: oid,
        type_parameters: olist, modifiers: mods,
    }
    24 ConstructorTypeNode(ConstructorTypeNodeData) {
        full_signature: oid, parameters: list, type_: oid, type_parameters: olist, modifiers: mods,
    }
    25 ContinueStatement(ContinueStatementData) { label: oid }
    26 DebuggerStatement(DebuggerStatementData) {}
    27 Decorator(DecoratorData) { expression: id }
    28 DeleteExpression(DeleteExpressionData) { expression: id }
    29 DoStatement(DoStatementData) { expression: id, statement: id }
    30 ElementAccessExpression(ElementAccessExpressionData) {
        argument_expression: id, expression: id, question_dot_token: oid,
    }
    31 EmptyStatement(EmptyStatementData) {}
    32 EnumDeclaration(EnumDeclarationData) { members: list, modifiers: mods, name: id }
    33 EnumMember(EnumMemberData) {
        initializer: oid, postfix_token: oid, modifiers: mods, name: id,
    }
    34 ExportAssignment(ExportAssignmentData) {
        expression: id, is_export_equals: flag, type_: oid, modifiers: mods,
    }
    35 ExportDeclaration(ExportDeclarationData) {
        attributes: oid, export_clause: oid, is_type_only: flag, module_specifier: oid,
        modifiers: mods,
    }
    36 ExportSpecifier(ExportSpecifierData) { is_type_only: flag, property_name: oid, name: id }
    37 ExpressionStatement(ExpressionStatementData) { expression: id }
    38 ExpressionWithTypeArguments(ExpressionWithTypeArgumentsData) {
        expression: id, type_arguments: olist,
    }
    39 ExternalModuleReference(ExternalModuleReferenceData) { expression: id }
    40 ForInOrOfStatement(ForInOrOfStatementData) {
        await_modifier: oid, expression: id, initializer: id, statement: id,
    }
    41 ForStatement(ForStatementData) {
        condition: oid, incrementor: oid, initializer: oid, statement: id,
    }
    42 FunctionDeclaration(FunctionDeclarationData) {
        asterisk_token: oid, body: oid, full_signature: oid, parameters: list, type_: oid,
        type_parameters: olist, modifiers: mods, name: oid,
    }
    43 FunctionExpression(FunctionExpressionData) {
        asterisk_token: oid, body: id, full_signature: oid, parameters: list, type_: oid,
        type_parameters: olist, modifiers: mods, name: oid,
    }
    44 FunctionTypeNode(FunctionTypeNodeData) {
        full_signature: oid, parameters: list, type_: oid, type_parameters: olist, modifiers: mods,
    }
    45 GetAccessorDeclaration(GetAccessorDeclarationData) {
        asterisk_token: oid, body: oid, full_signature: oid, parameters: list, postfix_token: oid,
        type_: oid, type_parameters: olist, modifiers: mods, name: id,
    }
    46 HeritageClause(HeritageClauseData) { token: kind, types: list }
    47 Identifier(IdentifierData) { text: string }
    48 IfStatement(IfStatementData) { else_statement: oid, expression: id, then_statement: id }
    49 ImportAttribute(ImportAttributeData) { value: id, name: id }
    50 ImportAttributes(ImportAttributesData) { attributes: list, multi_line: flag, token: kind }
    51 ImportClause(ImportClauseData) { named_bindings: oid, phase_modifier: okind, name: oid }
    52 ImportDeclaration(ImportDeclarationData) {
        attributes: oid, import_clause: oid, module_specifier: id, modifiers: mods,
    }
    53 ImportEqualsDeclaration(ImportEqualsDeclarationData) {
        is_type_only: flag, module_reference: id, modifiers: mods, name: id,
    }
    54 ImportSpecifier(ImportSpecifierData) { is_type_only: flag, property_name: oid, name: id }
    55 ImportTypeNode(ImportTypeNodeData) {
        argument: id, attributes: oid, is_type_of: flag, qualifier: oid, type_arguments: olist,
    }
    56 IndexSignatureDeclaration(IndexSignatureDeclarationData) {
        full_signature: oid, parameters: list, type_: id, type_parameters: olist, modifiers: mods,
    }
    57 IndexedAccessTypeNode(IndexedAccessTypeNodeData) { index_type: id, object_type: id }
    58 InferTypeNode(InferTypeNodeData) { type_parameter: id }
    59 InterfaceDeclaration(InterfaceDeclarationData) {
        heritage_clauses: olist, members: list, type_parameters: olist, modifiers: mods, name: id,
    }
    60 IntersectionTypeNode(IntersectionTypeNodeData) { types: list }
    61 JsDoc(JsDocData) { comment: list, tags: olist }
    62 JsDocAllType(JsDocAllTypeData) {}
    63 JsDocAugmentsTag(JsDocAugmentsTagData) { class_name: id, comment: olist, tag_name: id }
    64 JsDocCallbackTag(JsDocCallbackTagData) {
        comment: olist, tag_name: id, type_expression: id, name: oid,
    }
    65 JsDocDeprecatedTag(JsDocDeprecatedTagData) { comment: olist, tag_name: id }
    66 JsDocImplementsTag(JsDocImplementsTagData) { class_name: id, comment: olist, tag_name: id }
    67 JsDocImportTag(JsDocImportTagData) {
        attributes: oid, comment: olist, import_clause: oid, module_specifier: id, tag_name: id,
    }
    68 JsDocLink(JsDocLinkData) { name: oid, text: strings }
    69 JsDocLinkCode(JsDocLinkCodeData) { name: oid, text: strings }
    70 JsDocLinkPlain(JsDocLinkPlainData) { name: oid, text: strings }
    71 JsDocNameReference(JsDocNameReferenceData) { name: id }
    72 JsDocNonNullableType(JsDocNonNullableTypeData) { type_: id }
    73 JsDocNullableType(JsDocNullableTypeData) { type_: id }
    74 JsDocOptionalType(JsDocOptionalTypeData) { type_: id }
    75 JsDocOverloadTag(JsDocOverloadTagData) { comment: olist, tag_name: id, type_expression: id }
    76 JsDocOverrideTag(JsDocOverrideTagData) { comment: olist, tag_name: id }
    77 JsDocParameterOrPropertyTag(JsDocParameterOrPropertyTagData) {
        comment: olist, is_bracketed: flag, is_name_first: flag, tag_name: id, type_expression: oid,
        name: id,
    }
    78 JsDocPrivateTag(JsDocPrivateTagData) { comment: olist, tag_name: id }
    79 JsDocProtectedTag(JsDocProtectedTagData) { comment: olist, tag_name: id }
    80 JsDocPublicTag(JsDocPublicTagData) { comment: olist, tag_name: id }
    81 JsDocReadonlyTag(JsDocReadonlyTagData) { comment: olist, tag_name: id }
    82 JsDocReturnTag(JsDocReturnTagData) { comment: olist, tag_name: id, type_expression: oid }
    83 JsDocSatisfiesTag(JsDocSatisfiesTagData) {
        comment: olist, tag_name: id, type_expression: id,
    }
    84 JsDocSeeTag(JsDocSeeTagData) { comment: olist, name_expression: id, tag_name: id }
    85 JsDocSignature(JsDocSignatureData) {
        full_signature: oid, parameters: list, type_: oid, type_parameters: olist,
    }
    86 JsDocTemplateTag(JsDocTemplateTagData) {
        comment: olist, constraint: id, tag_name: id, type_parameters: list,
    }
    87 JsDocText(JsDocTextData) { text: strings }
    88 JsDocThisTag(JsDocThisTagData) { comment: olist, tag_name: id, type_expression: id }
    89 JsDocThrowsTag(JsDocThrowsTagData) { comment: olist, tag_name: id, type_expression: oid }
    90 JsDocTypeExpression(JsDocTypeExpressionData) { type_: id }
    91 JsDocTypeLiteral(JsDocTypeLiteralData) { is_array_type: flag, js_doc_property_tags: oids }
    92 JsDocTypeTag(JsDocTypeTagData) { comment: olist, tag_name: id, type_expression: id }
    93 JsDocTypedefTag(JsDocTypedefTagData) {
        comment: olist, tag_name: id, type_expression: oid, name: oid,
    }
    94 JsDocUnknownTag(JsDocUnknownTagData) { comment: olist, tag_name: id }
    95 JsDocVariadicType(JsDocVariadicTypeData) { type_: id }
    96 JsxAttribute(JsxAttributeData) { initializer: oid, name: id }
    97 JsxAttributes(JsxAttributesData) { properties: list }
    98 JsxClosingElement(JsxClosingElementData) { tag_name: id }
    99 JsxClosingFragment(JsxClosingFragmentData) {}
    100 JsxElement(JsxElementData) { children: list, closing_element: id, opening_element: id }
    101 JsxExpression(JsxExpressionData) { dot_dot_dot_token: oid, expression: oid }
    102 JsxFragment(JsxFragmentData) { children: list, closing_fragment: id, opening_fragment: id }
    103 JsxNamespacedName(JsxNamespacedNameData) { namespace: id, name: id }
    104 JsxOpeningElement(JsxOpeningElementData) {
        attributes: id, tag_name: id, type_arguments: olist,
    }
    105 JsxOpeningFragment(JsxOpeningFragmentData) {}
    106 JsxSelfClosingElement(JsxSelfClosingElementData) {
        attributes: id, tag_name: id, type_arguments: olist,
    }
    107 JsxSpreadAttribute(JsxSpreadAttributeData) { expression: id }
    108 JsxText(JsxTextData) {
        contains_only_trivia_white_spaces: flag, text: string, token_flags: tflags,
    }
    109 KeywordExpression(KeywordExpressionData) {}
    110 KeywordTypeNode(KeywordTypeNodeData) {}
    111 LabeledStatement(LabeledStatementData) { label: id, statement: id }
    112 LiteralTypeNode(LiteralTypeNodeData) { literal: id }
    113 MappedTypeNode(MappedTypeNodeData) {
        members: olist, name_type: oid, question_token: oid, readonly_token: oid, type_: oid,
        type_parameter: id,
    }
    114 MetaProperty(MetaPropertyData) { keyword_token: kind, name: id }
    115 MethodDeclaration(MethodDeclarationData) {
        asterisk_token: oid, body: oid, full_signature: oid, parameters: list, postfix_token: oid,
        type_: oid, type_parameters: olist, modifiers: mods, name: id,
    }
    116 MethodSignatureDeclaration(MethodSignatureDeclarationData) {
        full_signature: oid, parameters: list, postfix_token: oid, type_: oid,
        type_parameters: olist, modifiers: mods, name: id,
    }
    117 MissingDeclaration(MissingDeclarationData) { modifiers: mods }
    118 ModuleBlock(ModuleBlockData) { statements: list }
    119 ModuleDeclaration(ModuleDeclarationData) {
        asterisk_token: oid, attributes: oid, body: oid, keyword: kind, modifiers: mods, name: id,
    }
    120 NamedExports(NamedExportsData) { elements: list }
    121 NamedImports(NamedImportsData) { elements: list }
    122 NamedTupleMember(NamedTupleMemberData) {
        dot_dot_dot_token: oid, question_token: oid, type_: id, name: id,
    }
    123 NamespaceExport(NamespaceExportData) { name: id }
    124 NamespaceExportDeclaration(NamespaceExportDeclarationData) { modifiers: mods, name: id }
    125 NamespaceImport(NamespaceImportData) { name: id }
    126 NewExpression(NewExpressionData) { arguments: olist, expression: id, type_arguments: olist }
    127 NoSubstitutionTemplateLiteral(NoSubstitutionTemplateLiteralData) {
        raw_text: string, template_flags: tflags, text: string, token_flags: tflags,
    }
    128 NonNullExpression(NonNullExpressionData) { expression: id }
    129 NotEmittedStatement(NotEmittedStatementData) {}
    130 NotEmittedTypeElement(NotEmittedTypeElementData) {}
    131 NumericLiteral(NumericLiteralData) { text: string, token_flags: tflags }
    132 ObjectLiteralExpression(ObjectLiteralExpressionData) { multi_line: flag, properties: list }
    133 OmittedExpression(OmittedExpressionData) {}
    134 OptionalTypeNode(OptionalTypeNodeData) { type_: id }
    135 ParameterDeclaration(ParameterDeclarationData) {
        dot_dot_dot_token: oid, initializer: oid, question_token: oid, type_: oid, modifiers: mods,
        name: id,
    }
    136 ParenthesizedExpression(ParenthesizedExpressionData) { expression: id }
    137 ParenthesizedTypeNode(ParenthesizedTypeNodeData) { type_: id }
    138 PartiallyEmittedExpression(PartiallyEmittedExpressionData) { expression: id }
    139 PostfixUnaryExpression(PostfixUnaryExpressionData) { operand: id, operator: kind }
    140 PrefixUnaryExpression(PrefixUnaryExpressionData) { operand: id, operator: kind }
    141 PrivateIdentifier(PrivateIdentifierData) { text: string }
    142 PropertyAccessExpression(PropertyAccessExpressionData) {
        expression: id, question_dot_token: oid, name: id,
    }
    143 PropertyAssignment(PropertyAssignmentData) {
        initializer: id, postfix_token: oid, type_: oid, modifiers: mods, name: id,
    }
    144 PropertyDeclaration(PropertyDeclarationData) {
        initializer: oid, postfix_token: oid, type_: oid, modifiers: mods, name: id,
    }
    145 PropertySignatureDeclaration(PropertySignatureDeclarationData) {
        initializer: id, postfix_token: oid, type_: id, modifiers: mods, name: id,
    }
    146 QualifiedName(QualifiedNameData) { left: id, right: id }
    147 RegularExpressionLiteral(RegularExpressionLiteralData) { text: string, token_flags: tflags }
    148 RestTypeNode(RestTypeNodeData) { type_: id }
    149 ReturnStatement(ReturnStatementData) { expression: oid }
    150 SatisfiesExpression(SatisfiesExpressionData) { expression: id, type_: id }
    151 SemicolonClassElement(SemicolonClassElementData) {}
    152 SetAccessorDeclaration(SetAccessorDeclarationData) {
        asterisk_token: oid, body: oid, full_signature: oid, parameters: list, postfix_token: oid,
        type_: oid, type_parameters: olist, modifiers: mods, name: id,
    }
    153 ShorthandPropertyAssignment(ShorthandPropertyAssignmentData) {
        equals_token: oid, object_assignment_initializer: oid, postfix_token: oid, type_: oid,
        modifiers: mods, name: id,
    }
    154 SourceFile(SourceFileData) { end_of_file_token: id, statements: list }
    155 SpreadAssignment(SpreadAssignmentData) { expression: id }
    156 SpreadElement(SpreadElementData) { expression: id }
    157 StringLiteral(StringLiteralData) { text: string, token_flags: tflags }
    158 SwitchStatement(SwitchStatementData) { case_block: id, expression: id }
    159 SyntaxList(SyntaxListData) { children: ids }
    160 SyntheticExpression(SyntheticExpressionData) {
        is_spread: flag, tuple_name_source: oid, type_: opaque,
    }
    161 SyntheticReferenceExpression(SyntheticReferenceExpressionData) {
        expression: id, this_arg: id,
    }
    162 TaggedTemplateExpression(TaggedTemplateExpressionData) {
        question_dot_token: oid, tag: id, template: id, type_arguments: olist,
    }
    163 TemplateExpression(TemplateExpressionData) { head: id, template_spans: list }
    164 TemplateHead(TemplateHeadData) {
        raw_text: string, template_flags: tflags, text: string, token_flags: tflags,
    }
    165 TemplateLiteralTypeNode(TemplateLiteralTypeNodeData) { head: id, template_spans: list }
    166 TemplateLiteralTypeSpan(TemplateLiteralTypeSpanData) { literal: id, type_: id }
    167 TemplateMiddle(TemplateMiddleData) {
        raw_text: string, template_flags: tflags, text: string, token_flags: tflags,
    }
    168 TemplateSpan(TemplateSpanData) { expression: id, literal: id }
    169 TemplateTail(TemplateTailData) {
        raw_text: string, template_flags: tflags, text: string, token_flags: tflags,
    }
    170 ThisTypeNode(ThisTypeNodeData) {}
    171 ThrowStatement(ThrowStatementData) { expression: id }
    172 Token(TokenData) {}
    173 TryStatement(TryStatementData) { catch_clause: oid, finally_block: oid, try_block: id }
    174 TupleTypeNode(TupleTypeNodeData) { elements: list }
    175 TypeAliasDeclaration(TypeAliasDeclarationData) {
        type_: id, type_parameters: olist, modifiers: mods, name: id,
    }
    176 TypeAssertion(TypeAssertionData) { expression: id, type_: id }
    177 TypeLiteralNode(TypeLiteralNodeData) { members: list }
    178 TypeOfExpression(TypeOfExpressionData) { expression: id }
    179 TypeOperatorNode(TypeOperatorNodeData) { operator: kind, type_: id }
    180 TypeParameterDeclaration(TypeParameterDeclarationData) {
        constraint: oid, default_type: oid, expression: oid, modifiers: mods, name: id,
    }
    181 TypePredicateNode(TypePredicateNodeData) {
        asserts_modifier: oid, parameter_name: id, type_: oid,
    }
    182 TypeQueryNode(TypeQueryNodeData) { expr_name: id, type_arguments: olist }
    183 TypeReferenceNode(TypeReferenceNodeData) { type_arguments: olist, type_name: id }
    184 UnionTypeNode(UnionTypeNodeData) { types: list }
    185 VariableDeclaration(VariableDeclarationData) {
        exclamation_token: oid, initializer: oid, type_: oid, name: id,
    }
    186 VariableDeclarationList(VariableDeclarationListData) { declarations: list }
    187 VariableStatement(VariableStatementData) { declaration_list: id, modifiers: mods }
    188 VoidExpression(VoidExpressionData) { expression: id }
    189 WhileStatement(WhileStatementData) { expression: id, statement: id }
    190 WithStatement(WithStatementData) { expression: id, statement: id }
    191 YieldExpression(YieldExpressionData) { asterisk_token: oid, expression: oid }
}

// ──────────────────────────────────────────────────────────────────────
// Dump (the generator and its test)
// ──────────────────────────────────────────────────────────────────────

/// Writes a section. The mirror of `Decoder`.
#[cfg(test)]
struct Encoder {
    out: Vec<u8>,
    /// The store id of the parse.
    store: usize,
    /// The slot being written (see `Decoder::index`).
    index: u32,
    /// See `Decoder::pos`.
    pos: i64,
    /// The list table of the store (astshrink1), which the list fields
    /// of its node data name.
    lists: Vec<&'static crate::ast::store::StaticList>,
}

#[cfg(test)]
impl Encoder {
    fn u8(&mut self, value: u8) {
        self.out.push(value);
    }

    fn var(&mut self, mut value: u64) {
        loop {
            let byte = (value & 0x7f) as u8;
            value >>= 7;
            if value == 0 {
                self.out.push(byte);
                return;
            }
            self.out.push(byte | 0x80);
        }
    }

    fn zz(&mut self, value: i64) {
        self.var(zigzag(value));
    }

    fn count(&mut self, len: usize) {
        self.var(len as u64);
    }

    /// `slot` relative to the slot being written.
    fn rel(&self, slot: u32) -> i64 {
        i64::from(slot) - i64::from(self.index)
    }

    fn link(&mut self, value: u32) {
        match value {
            LibParseSlot::LINK_NONE => self.var(0),
            LibParseSlot::LINK_END => self.var(1),
            slot => self.var(2 + zigzag(self.rel(slot))),
        }
    }

    fn slot_of(id: &crate::astdata::NodeId) -> Result<u32, String> {
        u32::try_from(id.index()).map_err(|_| format!("node id {id:?} too large"))
    }

    // The `payload_codec!` field writers.

    fn id(&mut self, id: &crate::astdata::NodeId) -> Result<(), String> {
        let slot = Self::slot_of(id)?;
        self.zz(self.rel(slot));
        Ok(())
    }

    fn oid(&mut self, id: &Option<crate::astdata::NodeId>) -> Result<(), String> {
        match id {
            None => self.var(0),
            Some(id) => {
                let slot = Self::slot_of(id)?;
                self.var(zigzag(self.rel(slot)) + 1);
            }
        }
        Ok(())
    }

    fn list(&mut self, l: &crate::astdata::NodeList) -> Result<(), String> {
        let l = self.lists[l.index()];
        let start = i64::from(l.range.start.get());
        let end = i64::from(l.range.end.get());
        self.zz(start - self.pos);
        self.zz(end - start);
        self.var(((l.nodes.len() as u64) << 1) | u64::from(l.has_trailing_comma()));
        for id in l.nodes {
            self.id(id)?;
        }
        Ok(())
    }

    fn olist(&mut self, l: &Option<crate::astdata::NodeList>) -> Result<(), String> {
        match l {
            None => self.u8(0),
            Some(l) => {
                self.u8(1);
                self.list(l)?;
            }
        }
        Ok(())
    }

    fn mods(&mut self, m: &Option<crate::astdata::ModifierList>) -> Result<(), String> {
        match m {
            None => self.u8(0),
            Some(m) => {
                self.u8(1);
                self.list(&m.list)?;
                let flags = self.lists[m.list.index()].modifier_flags();
                self.var(u64::from(flags.0));
            }
        }
        Ok(())
    }

    fn opaque(&mut self, _: &crate::astdata::OpaqueValue) -> Result<(), String> {
        Ok(())
    }

    fn flag(&mut self, value: &bool) -> Result<(), String> {
        self.u8(u8::from(*value));
        Ok(())
    }

    fn num(&mut self, value: &u32) -> Result<(), String> {
        self.var(u64::from(*value));
        Ok(())
    }

    fn str(&mut self, text: &str) {
        self.count(text.len());
        self.out.extend_from_slice(text.as_bytes());
    }

    fn string(&mut self, text: &String) -> Result<(), String> {
        self.str(text);
        Ok(())
    }

    fn tflags(&mut self, flags: &crate::astdata::TokenFlags) -> Result<(), String> {
        self.var(u64::from(flags.0));
        Ok(())
    }

    fn kind(&mut self, kind: &SyntaxKind) -> Result<(), String> {
        self.var(*kind as u64);
        Ok(())
    }

    fn okind(&mut self, kind: &Option<SyntaxKind>) -> Result<(), String> {
        match kind {
            None => self.u8(0),
            Some(kind) => {
                self.u8(1);
                self.kind(kind)?;
            }
        }
        Ok(())
    }

    fn strings(&mut self, list: &Vec<String>) -> Result<(), String> {
        self.count(list.len());
        for text in list {
            self.str(text);
        }
        Ok(())
    }

    fn ids(&mut self, list: &Vec<crate::astdata::NodeId>) -> Result<(), String> {
        self.count(list.len());
        for id in list {
            self.id(id)?;
        }
        Ok(())
    }

    fn oids(&mut self, list: &Option<Vec<crate::astdata::NodeId>>) -> Result<(), String> {
        match list {
            None => self.u8(0),
            Some(list) => {
                self.u8(1);
                self.ids(list)?;
            }
        }
        Ok(())
    }

    // The `ParsedSourceFile` writers.

    fn node(&mut self, n: Node) -> Result<(), String> {
        if n.is_nil() {
            self.var(0);
            return Ok(());
        }
        if n.file_index() != self.store {
            return Err(format!("{n:?} is in another store"));
        }
        self.var((n.0 & 0xffff_ffff) - 1);
        Ok(())
    }

    fn nodes(&mut self, list: &[Node]) -> Result<(), String> {
        self.count(list.len());
        for &n in list {
            self.node(n)?;
        }
        Ok(())
    }

    fn range(&mut self, range: TextRange) {
        self.zz(i64::from(range.pos()));
        self.zz(i64::from(range.end()));
    }

    fn diagnostics(&mut self, list: &[Diagnostic]) -> Result<(), String> {
        self.count(list.len());
        for diagnostic in list {
            if diagnostic.repopulate_info.is_some() {
                return Err("a parse diagnostic with repopulate info".into());
            }
            self.node(diagnostic.file)?;
            self.zz(i64::from(diagnostic.pos));
            self.zz(i64::from(diagnostic.end));
            self.zz(i64::from(diagnostic.code));
            self.u8(diagnostic.category.0 as u8);
            let key = diagnostic.message.key();
            if crate::diagnostics::message_by_key(key) != Some(diagnostic.message) {
                return Err(format!("message key {key} does not find its message"));
            }
            self.str(key);
            self.strings(&diagnostic.message_args)?;
            self.diagnostics(&diagnostic.message_chain)?;
            self.diagnostics(&diagnostic.related_information)?;
            self.u8(u8::from(diagnostic.reports_unnecessary)
                | (u8::from(diagnostic.reports_deprecated) << 1)
                | (u8::from(diagnostic.skipped_on_no_emit) << 2));
        }
        Ok(())
    }

    /// The `ParsedSourceFile` fields that `Decoder::parsed_file` reads. The
    /// pattern names every field, so a new field does not compile here.
    fn parsed_file(&mut self, file: &ParsedSourceFile) -> Result<(), String> {
        let ParsedSourceFile {
            store,
            root,
            parse_options: _,
            text: _,
            end_of_file_token,
            hash: _,
            diagnostics,
            js_diagnostics,
            jsdoc_diagnostics,
            language_variant,
            script_kind,
            is_declaration_file,
            uses_uri_style_node_core_modules,
            identifier_count,
            imports,
            module_augmentations,
            ambient_module_names,
            comment_directives,
            jsdoc_cache,
            has_lazy_js_doc,
            reparsed_clones,
            pragmas,
            referenced_files,
            type_reference_directives,
            lib_reference_directives,
            check_js_directive,
            node_count,
            text_count,
            common_js_module_indicator,
            external_module_indicator,
            has_decorated_parameter,
            version: _,
        } = file;
        if *store != self.store {
            return Err("the file is in another store".into());
        }
        self.node(*root)?;
        self.node(*end_of_file_token)?;
        self.diagnostics(diagnostics)?;
        self.diagnostics(js_diagnostics)?;
        self.diagnostics(jsdoc_diagnostics)?;
        self.zz(i64::from(language_variant.0));
        self.zz(i64::from(script_kind.0));
        self.u8(u8::from(*is_declaration_file));
        self.u8(*uses_uri_style_node_core_modules as u8);
        self.zz(*identifier_count);
        self.nodes(imports)?;
        self.nodes(module_augmentations)?;
        self.strings(ambient_module_names)?;
        self.count(comment_directives.len());
        for directive in comment_directives {
            self.range(directive.loc);
            self.zz(i64::from(directive.kind.0));
        }
        // By slot, so the blob does not depend on the map order. The load
        // inserts in this order; the cache is only read by key.
        let mut cache: Vec<(&Node, &Vec<Node>)> = jsdoc_cache.iter().collect();
        cache.sort_by_key(|(node, _)| **node);
        self.count(cache.len());
        for (node, jsdocs) in cache {
            self.node(*node)?;
            self.nodes(jsdocs)?;
        }
        self.u8(u8::from(*has_lazy_js_doc));
        self.nodes(reparsed_clones)?;
        self.count(pragmas.len());
        for pragma in pragmas {
            self.str(&pragma.name);
            self.count(pragma.args.len());
            for (key, arg) in &pragma.args {
                self.str(key);
                self.str(&arg.name);
                self.str(&arg.value);
                self.range(arg.range);
            }
            self.range(pragma.range);
            self.kind(&pragma.kind)?;
        }
        for references in [
            referenced_files,
            type_reference_directives,
            lib_reference_directives,
        ] {
            self.count(references.len());
            for reference in references {
                self.range(reference.range);
                self.str(&reference.file_name);
                self.zz(i64::from(reference.resolution_mode.0));
                self.u8(u8::from(reference.preserve));
            }
        }
        match check_js_directive {
            None => self.u8(0),
            Some(directive) => {
                self.u8(1);
                self.u8(u8::from(directive.enabled));
                self.range(directive.range);
            }
        }
        self.count(*node_count);
        self.count(*text_count);
        self.node(*common_js_module_indicator)?;
        self.node(*external_module_indicator)?;
        self.u8(u8::from(*has_decorated_parameter));
        Ok(())
    }
}

/// The section of a finished parse of `text` with `opts` and
/// `script_kind`: `file` and its store (a store of this thread that is not
/// published), and the two `DetachedParse` fields. `load_section` reads it
/// back.
#[cfg(test)]
fn encode_section(
    opts: &SourceFileParseOptions,
    text: &str,
    script_kind: ScriptKind,
    file: &ParsedSourceFile,
    read_module_indicator_options: bool,
    import_specifiers: &[String],
) -> Result<Vec<u8>, String> {
    let views = lib_parse_slot_views(file.store)?;
    let mut body = Encoder {
        out: Vec::new(),
        store: file.store,
        index: 0,
        pos: 0,
        lists: crate::ast::store::lib_parse_list_table(file.store),
    };
    let mut palette: Vec<(SyntaxKind, NodeFlags)> = Vec::new();
    let mut palette_index: FxHashMap<(SyntaxKind, NodeFlags), u64> = FxHashMap::default();
    let mut names: Vec<Name> = Vec::new();
    let mut name_index: FxHashMap<Name, u64> = FxHashMap::default();
    for (i, view) in views.iter().enumerate() {
        body.index = u32::try_from(i + 1).map_err(|_| "too many slots".to_string())?;
        let pair = *palette_index
            .entry((view.kind, view.flags))
            .or_insert_with(|| {
                palette.push((view.kind, view.flags));
                palette.len() as u64 - 1
            });
        body.var(pair);
        let pos = i64::from(view.loc.pos());
        let end = i64::from(view.loc.end());
        body.zz(pos - body.pos);
        body.zz(end - pos);
        body.pos = pos;
        body.zz(body.rel(view.parent));
        body.link(view.first_child);
        body.link(view.next_sibling);
        if matches!(
            view.kind,
            SyntaxKind::Identifier | SyntaxKind::PrivateIdentifier
        ) {
            let keyword = get_identifier_token(view.name.as_str()) != SyntaxKind::Identifier;
            if keyword != view.text_is_keyword {
                return Err(format!("slot {}: the keyword bit differs", i + 1));
            }
            let index = *name_index.entry(view.name.clone()).or_insert_with(|| {
                names.push(view.name.clone());
                names.len() as u64 - 1
            });
            body.var(index);
        } else if view.name != Name::default() || view.text_is_keyword {
            return Err(format!("slot {}: a name on a {:?} slot", i + 1, view.kind));
        }
        if view.shared_name {
            body.u8(SHARED_NAME);
        } else {
            encode_payload(&mut body, view.data)?;
        }
    }
    let mut out = Vec::with_capacity(body.out.len() + 16 * names.len() + 1024);
    ParseKey::of(opts, text, script_kind).write(&mut out);
    out.push(u8::from(read_module_indicator_options));
    let mut e = Encoder {
        out,
        store: file.store,
        index: 0,
        pos: 0,
        lists: Vec::new(),
    };
    e.count(views.len());
    e.count(palette.len());
    for (kind, flags) in &palette {
        e.kind(kind)?;
        e.num(&flags.0)?;
    }
    e.count(names.len());
    for name in &names {
        match name.stable_id() {
            Some(id) if id & TEXT_BIT == 0 => e.out.extend_from_slice(&id.to_le_bytes()),
            _ => {
                let text = name.as_str();
                let len = u32::try_from(text.len())
                    .ok()
                    .filter(|&len| len & TEXT_BIT == 0)
                    .ok_or_else(|| format!("name of {} bytes", text.len()))?;
                e.out.extend_from_slice(&(TEXT_BIT | len).to_le_bytes());
                e.out.extend_from_slice(text.as_bytes());
            }
        }
    }
    e.out.extend_from_slice(&body.out);
    e.parsed_file(file)?;
    e.count(import_specifiers.len());
    for text in import_specifiers {
        e.str(text);
    }
    Ok(e.out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binder::lib_snapshot::{lib_text, snapshot_libs, write_blob};
    use crate::frontend::parser::utilities::{
        module_indicator_options_read, reset_module_indicator_options_read,
    };

    /// How to write the blob again.
    const REGENERATE: &str = "run `cargo test -p goport_core --lib \
        frontend::parser::lib_parse_snapshot::tests::generate_lib_parse_snapshot -- --ignored --exact` \
        (through scripts/run-cargo-capped.sh)";

    /// The parse options and the text of bundled lib `lib`, as the program
    /// loader gives them (the path is not read by the parse).
    fn lib_input(lib: &str) -> (SourceFileParseOptions, &'static str) {
        let file_name = format!("{}/{lib}", crate::frontend::bundled::lib_path());
        let text = lib_text(lib).unwrap_or_else(|| panic!("no bundled {lib}"));
        let opts = SourceFileParseOptions {
            file_name,
            ..Default::default()
        };
        (opts, text)
    }

    /// The state of this thread that a lib parse must not change.
    fn thread_state() -> (usize, (u64, u64)) {
        (synthetic_slot_count(), next_ids())
    }

    /// A parse and the two `DetachedParse` fields.
    struct Parse {
        file: ParsedSourceFile,
        read_module_indicator_options: bool,
        import_specifiers: Vec<String>,
    }

    /// A live serial parse (no snapshot). It fails when the parse changes
    /// state of this thread that a load would not change.
    fn live_parse(opts: &SourceFileParseOptions, text: &'static str) -> Result<Parse, String> {
        let before = thread_state();
        reset_module_indicator_options_read();
        LIVE_ONLY.set(true);
        let file = parse_source_file(opts, text, ScriptKind::TS);
        LIVE_ONLY.set(false);
        let read_module_indicator_options = module_indicator_options_read();
        if thread_state() != before {
            return Err("the live parse made synthetic nodes or ids".into());
        }
        let import_specifiers = file.imports.iter().map(|n| n.text().to_string()).collect();
        Ok(Parse {
            file,
            read_module_indicator_options,
            import_specifiers,
        })
    }

    /// The section of `parse`.
    fn section(opts: &SourceFileParseOptions, text: &str, parse: &Parse) -> Vec<u8> {
        encode_section(
            opts,
            text,
            ScriptKind::TS,
            &parse.file,
            parse.read_module_indicator_options,
            &parse.import_specifiers,
        )
        .unwrap_or_else(|e| panic!("{}: {e}", opts.file_name))
    }

    /// Asserts that `loaded` equals the live parse whose section is
    /// `expected`: the same section (every slot and every file field) and
    /// the same store tables.
    fn assert_same_parse(
        lib: &str,
        opts: &SourceFileParseOptions,
        text: &'static str,
        live: &Parse,
        expected: &[u8],
        loaded: &Parse,
    ) {
        assert_eq!(loaded.file.parse_options, live.file.parse_options, "{lib}");
        assert!(
            std::ptr::eq(
                loaded.file.text.as_static().expect("a lib text is static"),
                live.file.text.as_static().expect("a lib text is static")
            ),
            "{lib}: text"
        );
        assert!(
            section(opts, text, loaded) == expected,
            "{lib}: the loaded parse differs from the live parse"
        );
        let a = lib_parse_store_dump(live.file.store);
        let b = lib_parse_store_dump(loaded.file.store);
        if let Some(i) = (0..a.len().max(b.len())).find(|&i| a.get(i) != b.get(i)) {
            panic!(
                "{lib}: store line {i} differs:\n  live:   {:?}\n  loaded: {:?}",
                a.get(i),
                b.get(i)
            );
        }
    }

    /// Writes `lib_parse.bin` from a live parse of each lib, and prints
    /// the sizes and times (times mean something only in an optimized test
    /// build).
    #[test]
    #[ignore = "writes src/frontend/parser/lib_parse.bin"]
    fn generate_lib_parse_snapshot() {
        let mut sections = Vec::new();
        for lib in snapshot_libs() {
            let (opts, text) = lib_input(lib);
            // The process-wide unported counts: run this test alone.
            let unported = unported_report();
            let start = std::time::Instant::now();
            let live = live_parse(&opts, text).unwrap_or_else(|e| panic!("{lib}: {e}"));
            let parse_time = start.elapsed();
            assert_eq!(
                unported_report(),
                unported,
                "{lib}: the live parse hit unported code, which a load would skip"
            );
            let section = section(&opts, text, &live);
            // Load it into a new store, as `parse_source_file` does.
            let before = thread_state();
            let file_name: &'static str = Box::leak(opts.file_name.clone().into_boxed_str());
            let store = new_file_store(file_name, text);
            let start = std::time::Instant::now();
            let loaded = load_section(store, &section, &opts, text, ScriptKind::TS)
                .unwrap_or_else(|part| panic!("{lib}: the new section does not load ({part})"));
            let load_time = start.elapsed();
            assert_eq!(
                thread_state(),
                before,
                "{lib}: the load made synthetic nodes or ids"
            );
            let loaded = Parse {
                file: loaded.file,
                read_module_indicator_options: false,
                import_specifiers: loaded.import_specifiers,
            };
            assert_same_parse(lib, &opts, text, &live, &section, &loaded);
            eprintln!(
                "{lib}: text {} bytes, {} slots, section {} bytes, parse {parse_time:?}, load {load_time:?}",
                text.len(),
                file_store_slot_count(live.file.store),
                section.len()
            );
            sections.push((lib, section));
        }
        let blob = write_blob(MAGIC, &sections);
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../src/frontend/parser/lib_parse.bin"
        );
        std::fs::write(path, &blob).unwrap_or_else(|e| panic!("cannot write {path}: {e}"));
        eprintln!("wrote {path}: {} bytes", blob.len());
    }

    /// The embedded blob holds a section for each lib that equals a live
    /// parse of it now, and `parse_source_file` and
    /// `parse_source_file_detached` load it into a store that equals the
    /// live one, with no change to the state of the thread.
    #[test]
    fn snapshot_matches_live_parse() {
        for lib in snapshot_libs() {
            let (opts, text) = lib_input(lib);
            let live = live_parse(&opts, text).unwrap_or_else(|e| panic!("{lib}: {e}"));
            let expected = section(&opts, text, &live);
            let entry = entries()
                .iter()
                .find(|entry| entry.name == lib)
                .unwrap_or_else(|| panic!("lib_parse.bin has no {lib}: {REGENERATE}"));
            if entry.section != &expected[..] {
                let stored = ParseKey::read(&mut SnapshotReader::new(entry.section));
                let now = ParseKey::of(&opts, text, ScriptKind::TS);
                panic!(
                    "lib_parse.bin is stale for {lib} (stored key {stored:?}, key now {now:?}): \
                     {REGENERATE}"
                );
            }

            let before = thread_state();
            let loads = LOADS.get();
            let serial = parse_source_file(&opts, text, ScriptKind::TS);
            assert_eq!(
                LOADS.get(),
                loads + 1,
                "{lib}: not loaded from the snapshot"
            );
            assert_eq!(
                thread_state(),
                before,
                "{lib}: the load changed the thread state"
            );
            assert_eq!(
                parsed_source_file_diagnostics(serial.root).len(),
                serial.diagnostics.len(),
                "{lib}: set_source_file_diagnostics"
            );
            let serial = Parse {
                file: serial,
                read_module_indicator_options: false,
                import_specifiers: live.import_specifiers.clone(),
            };
            assert_same_parse(lib, &opts, text, &live, &expected, &serial);

            // A parse worker loads it too, and the loader adopts the store.
            let worker_opts = opts.clone();
            let (detached, worker_loads) = std::thread::spawn(move || {
                let parse = parse_source_file_detached(1, &worker_opts, text, ScriptKind::TS);
                (parse, LOADS.get())
            })
            .join()
            .unwrap();
            assert_eq!(
                worker_loads, 1,
                "{lib}: the worker did not load the snapshot"
            );
            assert!(detached.store.is_self_contained(), "{lib}");
            assert_eq!(
                detached.read_module_indicator_options, live.read_module_indicator_options,
                "{lib}"
            );
            assert_eq!(detached.import_specifiers, live.import_specifiers, "{lib}");
            let import_specifiers = detached.import_specifiers.clone();
            let adopted = Parse {
                file: adopt_detached_parse(detached, &opts),
                read_module_indicator_options: false,
                import_specifiers,
            };
            assert_same_parse(lib, &opts, text, &live, &expected, &adopted);
        }
    }

    /// A section that does not decode leaves the store empty, so the file
    /// is parsed live into the same store.
    #[test]
    fn bad_section_parses_live() {
        let lib = snapshot_libs()[0];
        let (opts, text) = lib_input(lib);
        let live = live_parse(&opts, text).unwrap_or_else(|e| panic!("{lib}: {e}"));
        let mut section = section(&opts, text, &live);
        // Cut the file fields: the slots load, then the decode fails.
        section.truncate(section.len() - 8);
        let file_name: &'static str = Box::leak(opts.file_name.clone().into_boxed_str());
        let store = new_file_store(file_name, text);
        assert_eq!(
            load_section(store, &section, &opts, text, ScriptKind::TS).err(),
            Some("load")
        );
        assert_eq!(file_store_slot_count(store), 1, "the store is not empty");
        assert!(!is_file_store_frozen(store));
    }
}
