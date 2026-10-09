//! Port of Go `api/encoder/decoder.go`: rebuilds AST nodes from the binary
//! format that `encoder.rs` writes.
//!
//! PORT notes for the whole file:
//! - Go `int` is 64-bit. The decoder reads offsets and node indices from
//!   untrusted `uint32` fields, so Go `int` is `i64` here and keeps Go's range
//!   checks (`readLE32` returns 0 out of range; a bad index panics in both).
//! - Nodes are made by a factory `NodeFactory` (synthetic arena), as Go makes
//!   them with `ast.NewNodeFactory`.

use crate::api::encoder::prelude::*;

use crate::frontend::parser::{ExternalModuleIndicatorOptions, SourceFileParseOptions};
use crate::frontend::tspath::Path;
use crate::gostd::errors;

// Go: api/encoder/decoder.go:14 astDecoder
/// astDecoder reconstructs real *ast.Node objects from binary-encoded data.
// PORT: Go `nodeArena` batch-allocates the `[]*ast.Node` slices of
// NodeLists. There is no arena here; each slice is its own `Vec`.
pub struct AstDecoder<'d> {
    pub raw: &'d [u8],
    pub str_table: u32,
    pub str_data: u32,
    pub ext_data: u32,
    pub node_off: u32,
    pub node_count: i64,
    pub factory: NodeFactory,
    pub child_buf: Vec<i64>,
    /// Single Go string covering all string data; substrings are zero-alloc slices.
    // PORT: the raw bytes; `get_string` makes each substring.
    pub all_string_data: &'d [u8],
    // Results
    pub nodes: Vec<Node>,
    pub node_lists: Vec<NodeList>,
}

// Go: api/encoder/decoder.go:33 DecodeSourceFile
/// DecodeSourceFile decodes binary-encoded data into an *ast.SourceFile.
pub fn decode_source_file(data: &[u8]) -> Result<Node, GoError> {
    let node = decode_nodes(data)?;
    if node.kind() != SyntaxKind::SourceFile {
        return Err(errors::errorf(
            format!(
                "expected SourceFile root, got {}",
                go_kind_string(node.kind() as i16)
            ),
            Vec::new(),
        ));
    }
    Ok(node)
}

// Go: api/encoder/decoder.go:45 DecodeNodes
/// DecodeNodes decodes binary-encoded AST data into a tree of *ast.Node objects.
pub fn decode_nodes(data: &[u8]) -> Result<Node, GoError> {
    let mut d = new_ast_decoder(data)?;
    d.decode()
}

// Go: api/encoder/decoder.go:53 newASTDecoder
pub fn new_ast_decoder(data: &[u8]) -> Result<AstDecoder<'_>, GoError> {
    if data.len() < HEADER_SIZE {
        return Err(errors::errorf(
            format!("data too short for header: {} bytes", data.len()),
            Vec::new(),
        ));
    }
    let version = data[HEADER_OFFSET_METADATA + 3];
    if version != PROTOCOL_VERSION {
        return Err(errors::errorf(
            format!("unsupported protocol version {version} (expected {PROTOCOL_VERSION})"),
            Vec::new(),
        ));
    }

    let str_table = read_le32(data, HEADER_OFFSET_STRING_OFFSETS as i64);
    let str_data = read_le32(data, HEADER_OFFSET_STRING_DATA as i64);
    let ext_data = read_le32(data, HEADER_OFFSET_EXTENDED_DATA as i64);
    let node_off = read_le32(data, HEADER_OFFSET_NODES as i64);

    let data_len = data.len() as u32;

    // Validate that all offsets are within the buffer.
    if str_table > data_len || str_data > data_len || ext_data > data_len || node_off > data_len {
        return Err(errors::errorf(
            format!("invalid AST header offsets: offsets exceed data length ({data_len})"),
            Vec::new(),
        ));
    }

    // Validate monotonic non-decreasing order of regions.
    if !(str_table <= str_data && str_data <= ext_data && ext_data <= node_off) {
        return Err(errors::errorf(
            format!(
                "invalid AST header offsets: expected strTable <= strData <= extData <= nodeOff (got {str_table}, {str_data}, {ext_data}, {node_off})"
            ),
            Vec::new(),
        ));
    }

    let mut d = AstDecoder {
        raw: data,
        str_table,
        str_data,
        ext_data,
        node_off,
        node_count: 0,
        factory: NodeFactory::new_with_hooks(NodeFactoryHooks::default()),
        child_buf: Vec::new(),
        all_string_data: &[],
        nodes: Vec::new(),
        node_lists: Vec::new(),
    };

    d.node_count = (data.len() as i64 - d.node_off as i64) / NODE_SIZE as i64;

    // Convert entire string data region to a single Go string upfront.
    // Substringing a Go string shares the backing array, so subsequent
    // getString calls produce substrings with zero allocations.
    d.all_string_data = &data[d.str_data as usize..];

    Ok(d)
}

impl AstDecoder<'_> {
    // Go: api/encoder/decoder.go:100 (*astDecoder).allocNodeSlice
    /// allocNodeSlice returns a zero-length slice with the given capacity, backed by
    /// the pre-allocated nodeArena. This avoids a heap allocation per NodeList.
    // PORT: no arena (see `AstDecoder`); a new empty `Vec` with that capacity.
    pub fn alloc_node_slice(&self, capacity: i64) -> Vec<Node> {
        Vec::with_capacity(capacity as usize)
    }

    // Go: api/encoder/decoder.go:107 (*astDecoder).nodeField
    /// nodeField reads a uint32 field from node i at the given field offset.
    pub fn node_field(&self, i: i64, field: usize) -> u32 {
        read_le32(
            self.raw,
            self.node_off as i64 + i * NODE_SIZE as i64 + field as i64,
        )
    }

    // Go: api/encoder/decoder.go:111 (*astDecoder).getString
    // PORT: Go substrings may hold any bytes. A Rust `String` is UTF-8, so
    // bytes that are not UTF-8 become U+FFFD. The encoder writes UTF-8 text
    // only, so its output decodes unchanged. A bad range panics as in Go.
    pub fn get_string(&self, idx: u32) -> String {
        String::from_utf8_lossy(self.get_string_bytes(idx)).into_owned()
    }

    /// PORT: the bytes of Go `getString(idx)`, for a Go `%q` of them.
    fn get_string_bytes(&self, idx: u32) -> &[u8] {
        let off_base = self.str_table as i64 + idx as i64 * 4;
        let start = read_le32(self.raw, off_base);
        let end = read_le32(self.raw, off_base + 4);
        &self.all_string_data[start as usize..end as usize]
    }

    // Go: api/encoder/decoder.go:120 (*astDecoder).collectChildren
    /// collectChildren returns indices of direct children of node i.
    /// The returned slice is reused across calls; callers must not retain it.
    // PORT: the reused buffer moves out to the caller, which puts it back in
    // `child_buf` when it is done with it.
    pub fn collect_children(&mut self, i: i64) -> Vec<i64> {
        let mut child_buf = std::mem::take(&mut self.child_buf);
        child_buf.clear();
        if i + 1 >= self.node_count {
            return child_buf;
        }
        let first_child = i + 1;
        if self.node_field(first_child, NODE_OFFSET_PARENT) != i as u32 {
            return child_buf;
        }
        child_buf.push(first_child);
        let mut next = self.node_field(first_child, NODE_OFFSET_NEXT) as i64;
        while next != 0 {
            child_buf.push(next);
            next = self.node_field(next, NODE_OFFSET_NEXT) as i64;
        }
        child_buf
    }

    // Go: api/encoder/decoder.go:138 (*astDecoder).decode
    pub fn decode(&mut self) -> Result<Node, GoError> {
        if self.node_count < 2 {
            return Err(errors::new("no nodes to decode"));
        }

        self.nodes = vec![Node::NIL; self.node_count as usize];
        self.node_lists = vec![NodeList::NIL; self.node_count as usize];
        // Pre-allocate arena for NodeList child slices. Each node can appear as a
        // child at most once, so nodeCount is an upper bound on total child pointers.
        // PORT: no arena (see `AstDecoder`).

        // Process bottom-up so children exist before parents.
        let mut i = self.node_count - 1;
        while i >= 1 {
            let kind = self.node_field(i, NODE_OFFSET_KIND);
            let pos = self.node_field(i, NODE_OFFSET_POS);
            let end = self.node_field(i, NODE_OFFSET_END);
            let data = self.node_field(i, NODE_OFFSET_DATA);
            let child_indices = self.collect_children(i);

            if kind == SYNTAX_KIND_NODE_LIST {
                let mut child_nodes = self.alloc_node_slice(child_indices.len() as i64);
                for &ci in &child_indices {
                    if self.nodes[ci as usize].is_some() {
                        child_nodes.push(self.nodes[ci as usize]);
                    }
                }
                // PORT: Go sets `nl.Loc` after `NewNodeList`. A factory list
                // fixes its Loc when it is made.
                let nl = self
                    .factory
                    .new_node_list_with_loc(&child_nodes, TextRange::new(pos as i32, end as i32));
                self.node_lists[i as usize] = nl;
                self.child_buf = child_indices;
                i -= 1;
                continue;
            }

            // PORT: Go `ast.Kind(kind)` turns any uint32 into the int16 Kind. A
            // value that is no SyntaxKind reaches the default arm of the Go
            // switch; `create_node_for_invalid_kind` returns that arm's result.
            let result = match SyntaxKind::try_from(kind as u16) {
                Ok(k) => self.create_node(k, data, &child_indices),
                Err(()) => self.create_node_for_invalid_kind(kind as i16, data, &child_indices),
            };
            self.child_buf = child_indices;
            let node = match result {
                Ok(node) => node,
                Err(err) => {
                    return Err(errors::errorf(
                        format!(
                            "at node {i} (kind {}): {}",
                            go_kind_string(kind as i16),
                            err.error()
                        ),
                        vec![err],
                    ));
                }
            };
            set_node_loc(node, TextRange::new(pos as i32, end as i32));
            set_node_flags(node, NodeFlags(self.node_field(i, NODE_OFFSET_FLAGS)));
            // ts#64320
            if kind == SyntaxKind::SourceFile as u32 {
                let is_declaration_file =
                    NodeFlags(self.node_field(i, NODE_OFFSET_FLAGS)).intersects(NodeFlags::AMBIENT);
                update_synthetic_source_file(node, |d| d.is_declaration_file = is_declaration_file);
            }
            self.nodes[i as usize] = node;
            i -= 1;
        }

        Ok(self.nodes[1])
    }

    // Go: api/encoder/decoder.go:183 (*astDecoder).getModifierList
    /// getModifierList creates a *ast.ModifierList from a child index that is a NodeList.
    pub fn get_modifier_list(&self, ci: i64) -> ModifierList {
        let nl = self.node_lists[ci as usize];
        if nl.is_nil() {
            return ModifierList::NIL;
        }
        // PORT: Go sets `ml.Loc = nl.Loc` after `NewModifierList`. A factory
        // list fixes its Loc when it is made.
        self.factory
            .new_modifier_list_with_loc(&nl.nodes().to_vec(), nl.loc())
    }
}

// Go: api/encoder/decoder.go:197 childIterator
/// childIterator helps walk through children based on a bitmask.
pub struct ChildIterator<'a> {
    indices: &'a [i64],
    pos: i64,
}

// Go: api/encoder/decoder.go:202 newChildIter
pub fn new_child_iter(indices: &[i64]) -> ChildIterator<'_> {
    ChildIterator { indices, pos: 0 }
}

impl ChildIterator<'_> {
    // Go: api/encoder/decoder.go:204 (*childIterator).next
    /// next returns the index of the next child, advancing the position.
    pub fn next(&mut self) -> i64 {
        if self.pos >= self.indices.len() as i64 {
            return 0;
        }
        let ci = self.indices[self.pos as usize];
        self.pos += 1;
        ci
    }

    // Go: api/encoder/decoder.go:214 (*childIterator).nextIf
    /// nextIf returns the index of the next child if the corresponding mask bit is set.
    pub fn next_if(&mut self, mask: u8, bit: u8) -> i64 {
        if mask & (1 << bit) == 0 {
            return 0;
        }
        self.next()
    }
}

impl AstDecoder<'_> {
    // Go: api/encoder/decoder.go:221 (*astDecoder).nodeAt
    pub fn node_at(&self, ci: i64) -> Node {
        if ci == 0 {
            return Node::NIL;
        }
        self.nodes[ci as usize]
    }

    // Go: api/encoder/decoder.go:228 (*astDecoder).nodeListAt
    pub fn node_list_at(&self, ci: i64) -> NodeList {
        if ci == 0 {
            return NodeList::NIL;
        }
        self.node_lists[ci as usize]
    }

    // Go: api/encoder/decoder.go:235 (*astDecoder).modifierListAt
    pub fn modifier_list_at(&self, ci: i64) -> ModifierList {
        if ci == 0 {
            return ModifierList::NIL;
        }
        self.get_modifier_list(ci)
    }

    // Go: api/encoder/decoder.go:242 (*astDecoder).createNode
    pub fn create_node(
        &self,
        kind: SyntaxKind,
        data: u32,
        child_indices: &[i64],
    ) -> Result<Node, GoError> {
        let data_type = data & NODE_DATA_TYPE_MASK;
        let common_data = ((data >> 24) & 0x3f) as u8;

        match data_type {
            NODE_DATA_TYPE_STRING => self.create_string_node(kind, data, common_data),
            NODE_DATA_TYPE_EXTENDED_DATA => {
                self.create_extended_node(kind, data, child_indices, common_data)
            }
            _ => self.create_children_node(kind, data, child_indices, common_data),
        }
    }

    /// Go `createNode` for a raw kind that is no SyntaxKind: the default arm
    /// of the Go switch that `createNode` picks by data type.
    // PORT: see `decode`. `go_kind_string` prints Go's "Kind(N)".
    fn create_node_for_invalid_kind(
        &self,
        kind: i16,
        data: u32,
        child_indices: &[i64],
    ) -> Result<Node, GoError> {
        let data_type = data & NODE_DATA_TYPE_MASK;
        match data_type {
            NODE_DATA_TYPE_STRING => {
                // Go `createStringNode` reads the string before its switch.
                let str_idx = data & NODE_DATA_STRING_INDEX_MASK;
                let _text = self.get_string(str_idx);
                Err(errors::errorf(
                    format!("unknown string node kind {}", go_kind_string(kind)),
                    Vec::new(),
                ))
            }
            NODE_DATA_TYPE_EXTENDED_DATA => Err(errors::errorf(
                format!("unknown extended data node kind {}", go_kind_string(kind)),
                Vec::new(),
            )),
            _ => Err(errors::errorf(
                format!(
                    "unhandled node kind {} with {} children",
                    go_kind_string(kind),
                    child_indices.len()
                ),
                Vec::new(),
            )),
        }
    }

    // Go: api/encoder/decoder.go:256 (*astDecoder).decodeExtendedData_SourceFile
    pub fn decode_extended_data_source_file(
        &self,
        data: u32,
        child_indices: &[i64],
        _common_data: u8,
    ) -> Result<Node, GoError> {
        let ext_off = self.ext_data as i64 + (data & NODE_DATA_STRING_INDEX_MASK) as i64;

        let text_idx = read_le32(self.raw, ext_off);
        let file_name_idx = read_le32(self.raw, ext_off + 4);
        let path_idx = read_le32(self.raw, ext_off + 8);
        // ts#64320
        let language_variant = LanguageVariant(read_le32(self.raw, ext_off + 12) as i32);
        let script_kind = ScriptKind(read_le32(self.raw, ext_off + 16) as i32);
        let text = self.get_string(text_idx);
        let file_name = self.get_string(file_name_idx);
        let path = self.get_string(path_idx);
        // ts#64159 (Go N' api/encoder/decoder.go:270): the path is a
        // canonical path key and the file name a rooted normalized path.
        if !crate::api::try_path_key_from_canonical(&path) {
            return Err(errors::new(format!(
                "invalid source file path {}",
                crate::gostd::strconv::quote_bytes(self.get_string_bytes(path_idx))
            )));
        }
        // ts#64216
        if !crate::api::try_rooted_path_from_normalized(&file_name) {
            // Go `%q` of the name's bytes (`file_name` has U+FFFD for
            // bytes that are not valid UTF-8).
            return Err(errors::new(format!(
                "invalid source file name {}",
                crate::gostd::strconv::quote_bytes(self.get_string_bytes(file_name_idx))
            )));
        }

        // Recover parse options from header.
        let parse_opts = read_le32(self.raw, HEADER_OFFSET_PARSE_OPTIONS as i64);
        let opts = SourceFileParseOptions {
            file_name,
            path: Path(path),
            external_module_indicator_options: ExternalModuleIndicatorOptions {
                jsx: parse_opts & 1 != 0,
                force: parse_opts & 2 != 0,
            },
        };

        // Collect children: first is statements NodeList, second is EndOfFile.
        let mut stmts = NodeList::NIL;
        let mut end_of_file = Node::NIL;
        for &ci in child_indices {
            if self.node_field(ci, NODE_OFFSET_KIND) == SYNTAX_KIND_NODE_LIST {
                stmts = self.node_list_at(ci);
            } else if self.nodes[ci as usize].is_some()
                && self.nodes[ci as usize].kind() == SyntaxKind::EndOfFile
            {
                end_of_file = self.nodes[ci as usize];
            }
        }
        if end_of_file.is_nil() {
            end_of_file = self.factory.new_token(SyntaxKind::EndOfFile);
        }
        // PORT: Go `NewSourceFile(opts, text, ...)`. The factory SourceFile
        // keeps only `FileName` and `Path` of `opts` (see
        // `NodeFactory::new_source_file`), and it takes a `&'static str` file
        // name. Go keeps both alive with the node; here they leak (the text
        // as a static `FileText`).
        let file_name: &'static str = Box::leak(opts.file_name.clone().into_boxed_str());
        let text: &'static str = Box::leak(text.into_boxed_str());
        let node = self
            .factory
            .new_source_file(file_name, &opts.path.0, text, stmts, end_of_file);
        // ts#64320
        update_synthetic_source_file(node, |source_file| {
            source_file.language_variant = language_variant;
            source_file.script_kind = script_kind;
        });
        Ok(node)
    }

    // Go: api/encoder/decoder.go:293 (*astDecoder).decodeExtendedData_TemplateHead
    pub fn decode_extended_data_template_head(
        &self,
        data: u32,
        _child_indices: &[i64],
        _common_data: u8,
    ) -> Result<Node, GoError> {
        let ext_off = self.ext_data as i64 + (data & NODE_DATA_STRING_INDEX_MASK) as i64;
        let text_idx = read_le32(self.raw, ext_off);
        let raw_text_idx = read_le32(self.raw, ext_off + 4);
        let flags = read_le32(self.raw, ext_off + 8);
        Ok(self.factory.new_template_head(
            self.get_string(text_idx),
            self.get_string(raw_text_idx),
            TokenFlags(flags as i32),
        ))
    }

    // Go: api/encoder/decoder.go:301 (*astDecoder).decodeExtendedData_TemplateMiddle
    pub fn decode_extended_data_template_middle(
        &self,
        data: u32,
        _child_indices: &[i64],
        _common_data: u8,
    ) -> Result<Node, GoError> {
        let ext_off = self.ext_data as i64 + (data & NODE_DATA_STRING_INDEX_MASK) as i64;
        let text_idx = read_le32(self.raw, ext_off);
        let raw_text_idx = read_le32(self.raw, ext_off + 4);
        let flags = read_le32(self.raw, ext_off + 8);
        Ok(self.factory.new_template_middle(
            self.get_string(text_idx),
            self.get_string(raw_text_idx),
            TokenFlags(flags as i32),
        ))
    }

    // Go: api/encoder/decoder.go:309 (*astDecoder).decodeExtendedData_TemplateTail
    pub fn decode_extended_data_template_tail(
        &self,
        data: u32,
        _child_indices: &[i64],
        _common_data: u8,
    ) -> Result<Node, GoError> {
        let ext_off = self.ext_data as i64 + (data & NODE_DATA_STRING_INDEX_MASK) as i64;
        let text_idx = read_le32(self.raw, ext_off);
        let raw_text_idx = read_le32(self.raw, ext_off + 4);
        let flags = read_le32(self.raw, ext_off + 8);
        Ok(self.factory.new_template_tail(
            self.get_string(text_idx),
            self.get_string(raw_text_idx),
            TokenFlags(flags as i32),
        ))
    }

    // Go: api/encoder/decoder.go:317 (*astDecoder).singleChild
    pub fn single_child(&self, child_indices: &[i64]) -> Node {
        if child_indices.is_empty() {
            return Node::NIL;
        }
        self.nodes[child_indices[0] as usize]
    }

    // Go: api/encoder/decoder.go:324 (*astDecoder).singleNodeListChild
    pub fn single_node_list_child(&self, child_indices: &[i64]) -> NodeList {
        if child_indices.is_empty() {
            return NodeList::NIL;
        }
        self.node_lists[child_indices[0] as usize]
    }
}

// Go: api/encoder/decoder.go:343 readLE32
fn read_le32(data: &[u8], offset: i64) -> u32 {
    if offset < 0 || offset + 4 > data.len() as i64 {
        return 0;
    }
    let o = offset as usize;
    u32::from_le_bytes([data[o], data[o + 1], data[o + 2], data[o + 3]])
}

// Hand-written commonData decoding functions. Each extracts the original values
// from the 6-bit commonData that were packed by the corresponding
// getNodeCommonData_* function.

// Go: api/encoder/decoder.go:354 decodeNodeCommonData_SyntheticExpression
// PORT: Go returns `(any, bool)`; the `any` is the checker type, `TypeId` here.
pub fn decode_node_common_data_synthetic_expression(_common_data: u8) -> (TypeId, bool) {
    panic!("SyntheticExpression should never be decoded")
}

// Hand-written extended data decoding functions for literal nodes.

impl AstDecoder<'_> {
    // Go: api/encoder/decoder.go:348 (*astDecoder).decodeExtendedData_StringLiteral
    pub fn decode_extended_data_string_literal(
        &self,
        data: u32,
        _child_indices: &[i64],
        _common_data: u8,
    ) -> Result<Node, GoError> {
        let ext_off = self.ext_data as i64 + (data & NODE_DATA_STRING_INDEX_MASK) as i64;
        let text_idx = read_le32(self.raw, ext_off);
        let flags = read_le32(self.raw, ext_off + 4);
        Ok(self
            .factory
            .new_string_literal(self.get_string(text_idx), TokenFlags(flags as i32)))
    }

    // Go: api/encoder/decoder.go:355 (*astDecoder).decodeExtendedData_NumericLiteral
    pub fn decode_extended_data_numeric_literal(
        &self,
        data: u32,
        _child_indices: &[i64],
        _common_data: u8,
    ) -> Result<Node, GoError> {
        let ext_off = self.ext_data as i64 + (data & NODE_DATA_STRING_INDEX_MASK) as i64;
        let text_idx = read_le32(self.raw, ext_off);
        let flags = read_le32(self.raw, ext_off + 4);
        Ok(self
            .factory
            .new_numeric_literal(self.get_string(text_idx), TokenFlags(flags as i32)))
    }

    // Go: api/encoder/decoder.go:362 (*astDecoder).decodeExtendedData_BigIntLiteral
    pub fn decode_extended_data_big_int_literal(
        &self,
        data: u32,
        _child_indices: &[i64],
        _common_data: u8,
    ) -> Result<Node, GoError> {
        let ext_off = self.ext_data as i64 + (data & NODE_DATA_STRING_INDEX_MASK) as i64;
        let text_idx = read_le32(self.raw, ext_off);
        let flags = read_le32(self.raw, ext_off + 4);
        Ok(self
            .factory
            .new_big_int_literal(self.get_string(text_idx), TokenFlags(flags as i32)))
    }

    // Go: api/encoder/decoder.go:369 (*astDecoder).decodeExtendedData_RegularExpressionLiteral
    pub fn decode_extended_data_regular_expression_literal(
        &self,
        data: u32,
        _child_indices: &[i64],
        _common_data: u8,
    ) -> Result<Node, GoError> {
        let ext_off = self.ext_data as i64 + (data & NODE_DATA_STRING_INDEX_MASK) as i64;
        let text_idx = read_le32(self.raw, ext_off);
        let flags = read_le32(self.raw, ext_off + 4);
        Ok(self
            .factory
            .new_regular_expression_literal(self.get_string(text_idx), TokenFlags(flags as i32)))
    }

    // Go: api/encoder/decoder.go:376 (*astDecoder).decodeExtendedData_NoSubstitutionTemplateLiteral
    pub fn decode_extended_data_no_substitution_template_literal(
        &self,
        data: u32,
        _child_indices: &[i64],
        _common_data: u8,
    ) -> Result<Node, GoError> {
        let ext_off = self.ext_data as i64 + (data & NODE_DATA_STRING_INDEX_MASK) as i64;
        let text_idx = read_le32(self.raw, ext_off);
        let flags = read_le32(self.raw, ext_off + 4);
        Ok(self.factory.new_no_substitution_template_literal(
            self.get_string(text_idx),
            TokenFlags(flags as i32),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flags::ScriptKind;
    use crate::frontend::parser;
    use crate::scanner_util::go_string_from_bytes;
    use std::rc::Rc;

    // Go `%q` of a bad source file name quotes its bytes (decoder.go:271,
    // wrapped at :172). The encoder writes the Go bytes of the name
    // (`StringTable`); the test then makes the name relative. A byte that is
    // not valid UTF-8 is `\x..` and a real U+FDD0 stays one char.
    #[test]
    fn a_bad_file_name_is_quoted_from_its_go_bytes() {
        let go_name: &[u8] = b"/x\xff\xef\xb7\x90\xef\xb7\x90.ts";
        let name = go_string_from_bytes(go_name.to_vec());
        let file = Rc::new(parser::parse_source_file(
            &SourceFileParseOptions {
                file_name: name.clone(),
                path: Path(name),
                ..Default::default()
            },
            "let x = 1;",
            ScriptKind::TS,
        ));
        crate::program::note_parsed_source_file(&file);
        let (mut buf, _) = crate::api::encoder::encode_source_file(file.root).expect("encode");
        let at = buf
            .windows(go_name.len())
            .position(|bytes| bytes == go_name)
            .expect("the name's Go bytes in the string data");
        buf[at] = b'_';
        assert_eq!(
            decode_source_file(&buf).expect_err("a bad name").error(),
            r#"at node 1 (kind KindSourceFile): invalid source file name "_x\xff\ufdd0\ufdd0.ts""#
        );
    }
}
