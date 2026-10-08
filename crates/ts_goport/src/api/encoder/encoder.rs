//! Port of Go `api/encoder/encoder.go`: the binary AST encoder.
//!
//! PORT notes for the whole file:
//! - Go `int` byte lengths and offsets are `usize` (they index byte buffers).
//! - Go `appendUint32s` and the msgpack writers return the grown slice; here
//!   they append to a `&mut Vec<u8>` in place.
//! - Go `n := node.AsX()` type assertions are the per-field `Node` accessors,
//!   which panic on other kinds like the Go assertion.
//! - Go `sourceFile.Hash`, `ParseOptions()`, `NodeCount` and `TextCount` are
//!   not on the ts_goport SourceFile node. They are in its `ParsedSourceFile`.
//!   A caller that holds that record gives it (`encode_parsed_source_file`,
//!   the leased file of an api `createSourceFile`). Otherwise the encoder
//!   finds it by the root node: see `source_file_content_hash` and
//!   `parsed_source_file_of`.
//! - The other Go `SourceFile` fields that the parser sets (`Path()`,
//!   `Imports()`, `ModuleAugmentations`, ...) are read through
//!   `source_file_fields`: from the record that the caller gives, else from
//!   the published file, else from the parse of a file that is parsed
//!   outside a program.

use crate::api::encoder::prelude::*;

use crate::ast::node::{ChildField, for_each_child_field};
use crate::ast::source_file_ls::{SourceFileDataKey, new_source_file_data_key};
use crate::ast::store::{FileNodeReader, FileNodes, frozen_store_text_name};
use crate::astdata::NodeData;
use crate::frontend::core_binarysearch::binary_search_unique_func;
use crate::frontend::parser::{ExternalModuleIndicatorOptions, ParsedSourceFile};
use std::borrow::Cow;
use std::cell::OnceCell;
use std::sync::LazyLock;

// Go: api/encoder/encoder.go:16 init
// PORT: the Go `init()` guard is a compile-time assertion.
const _: () = assert!(
    (SyntaxKind::LAST_UNARY_OPERATOR as u32) <= 0x3f,
    "KindLastUnaryOperator exceeds the 6-bit commonData capacity (max 63)"
);

// Go: api/encoder/encoder.go:21 NodeOffsetKind ... NodeSize
pub const NODE_OFFSET_KIND: usize = 0;
pub const NODE_OFFSET_POS: usize = 4;
pub const NODE_OFFSET_END: usize = 8;
pub const NODE_OFFSET_NEXT: usize = 12;
pub const NODE_OFFSET_PARENT: usize = 16;
pub const NODE_OFFSET_DATA: usize = 20;
pub const NODE_OFFSET_FLAGS: usize = 24;
/// NodeSize is the number of bytes that represents a single node in the encoded format.
pub const NODE_SIZE: usize = 28;

// Go: api/encoder/encoder.go:33 NodeDataTypeChildren ...
pub const NODE_DATA_TYPE_CHILDREN: u32 = 0 << 30;
pub const NODE_DATA_TYPE_STRING: u32 = 1 << 30;
pub const NODE_DATA_TYPE_EXTENDED_DATA: u32 = 2 << 30;

// Go: api/encoder/encoder.go:39 NodeDataTypeMask ...
pub const NODE_DATA_TYPE_MASK: u32 = 0xc0_00_00_00;
pub const NODE_DATA_CHILD_MASK: u32 = 0x00_00_00_ff;
pub const NODE_DATA_STRING_INDEX_MASK: u32 = 0x00_ff_ff_ff;

// Go: api/encoder/encoder.go:45 SyntaxKindNodeList
pub const SYNTAX_KIND_NODE_LIST: u32 = u32::MAX;

// Go: api/encoder/encoder.go:49 HeaderOffsetMetadata ... HeaderSize
pub const HEADER_OFFSET_METADATA: usize = 0;
pub const HEADER_OFFSET_HASH_LO0: usize = 4;
pub const HEADER_OFFSET_HASH_LO1: usize = 8;
pub const HEADER_OFFSET_HASH_HI0: usize = 12;
pub const HEADER_OFFSET_HASH_HI1: usize = 16;
pub const HEADER_OFFSET_PARSE_OPTIONS: usize = 20;
pub const HEADER_OFFSET_STRING_OFFSETS: usize = 24;
pub const HEADER_OFFSET_STRING_DATA: usize = 28;
pub const HEADER_OFFSET_EXTENDED_DATA: usize = 32;
pub const HEADER_OFFSET_STRUCTURED_DATA: usize = 36;
pub const HEADER_OFFSET_NODES: usize = 40;
// ts#64434: source file ID and lease ID (uint64 each) and the binder data
// offset.
pub const HEADER_OFFSET_SOURCE_FILE_ID: usize = 44;
pub const HEADER_OFFSET_SOURCE_FILE_LEASE: usize = 52;
pub const HEADER_OFFSET_BINDER_DATA: usize = 60;
pub const HEADER_SIZE: usize = 64;

// Go: api/encoder/encoder.go:71 ProtocolVersion
// ts#63957: 7 -> 8. ts#64434: 8 -> 9.
pub const PROTOCOL_VERSION: u8 = 9;

// Source File Binary Format
// =========================
//
// The following defines a protocol for serializing TypeScript SourceFile objects to a compact binary format. All integer
// values are little-endian.
//
// Overview
// --------
//
// The format comprises seven sections:
//
// | Section            | Length             | Description                                                                                     |
// | ------------------ | ------------------ | ----------------------------------------------------------------------------------------------- |
// | Header             | 64 bytes           | Contains the content hash, parse options, flags, file metadata, and byte offsets to the start of each section. |
// | String offsets     | 8 bytes per string | Pairs of starting byte offsets and ending byte offsets into the **string data** section.        |
// | String data        | variable           | UTF-8 encoded string data.                                                                      |
// | Extended node data | variable           | Extra data for some kinds of nodes.                                                             |
// | Structured data    | variable           | Msgpack-encoded metadata blobs (e.g. file references).                                         |
// | Nodes              | 28 bytes per node  | Defines the AST structure of the file, with references to strings and extended data.            |
//
// Header (64 bytes)
// -----------------
//
// The header contains the following fields:
//
// | Byte offset | Type      | Field                                             |
// | ----------- | --------- | ------------------------------------------------- |
// | 0           | uint8     | Protocol version                                  |
// | 1-3         |           | Reserved                                          |
// | 4-19        | uint128   | Source file content hash (xxh3, LE)               |
// | 20-23       | uint32    | Parse options (bitmask; bit 0: JSX, bit 1: Force) |
// | 24-27       | uint32    | Byte offset to string offsets section             |
// | 28-31       | uint32    | Byte offset to string data section                |
// | 32-35       | uint32    | Byte offset to extended node data section         |
// | 36-39       | uint32    | Byte offset to structured data section            |
// | 40-43       | uint32    | Byte offset to nodes section                      |
// | 44-51       | uint64    | Source file ID (0 = none)                          |
// | 52-59       | uint64    | Source file lease ID (0 = none)                    |
// | 60-63       | uint32    | Byte offset to binder data (0 = none)              |
//
// String offsets (8 bytes per string)
// -----------------------------------
//
// Each string offset entry consists of two 4-byte unsigned integers, representing the start and end byte offsets into the
// **string data** section.
//
// String data (variable)
// ----------------------
//
// The string data section contains UTF-8 encoded string data, with WTF-8 used for JS strings containing lone UTF-16
// surrogates. In typical cases, the entirety of the string data is the source file text, and individual nodes with
// string properties reference their positional slice of the file text. In cases where a node's string property is not
// equal to the slice of file text at its position, the unique string is appended to the string data section after the
// file text.
//
// Extended node data (variable)
// -----------------------------
//
// The extended node data section contains additional data for specific node types. The length and meaning of each entry
// is defined by the node type.
//
// Currently, the only node types that use this section are `TemplateHead`, `TemplateMiddle`, `TemplateTail`, and
// `SourceFile`. The extended data format for the first three is:
//
// | Byte offset | Type   | Field                                            |
// | ----------- | ------ | ------------------------------------------------ |
// | 0-4         | uint32 | Index of `text` in the string offsets section    |
// | 4-8         | uint32 | Index of `rawText` in the string offsets section |
// | 8-12        | uint32 | Value of `templateFlags`                         |
//
// and for `SourceFile` is:
//
// | Byte offset | Type   | Field                                                          |
// | ----------- | ------ | -------------------------------------------------------------- |
// | 0-4         | uint32 | Index of `text` in the string offsets section                  |
// | 4-8         | uint32 | Index of `fileName` in the string offsets section              |
// | 8-12        | uint32 | Index of `path` in the string offsets section                  |
// | 12-16       | uint32 | Value of `languageVariant`                                    |
// | 16-20       | uint32 | Value of `scriptKind`                                         |
// | 20-24       | uint32 | Byte offset of `referencedFiles` in structured data section   |
// | 24-28       | uint32 | Byte offset of `typeReferenceDirectives` in structured data   |
// | 28-32       | uint32 | Byte offset of `libReferenceDirectives` in structured data    |
// | 32-36       | uint32 | Byte offset of `imports` node index array in structured data  |
// | 36-40       | uint32 | Byte offset of `moduleAugmentations` node index array         |
// | 40-44       | uint32 | Byte offset of `ambientModuleNames` string array              |
// | 44-48       | uint32 | Node index of `externalModuleIndicator` (0 = nil)             |
// | 48-52       | uint32 | Index of `originalText` in the string offsets section         |
// | 52-56       | uint32 | Byte offset of `spanMap` in structured data                    |
// | 56-60       | uint32 | Byte offset of `supplementalSourceFileNames` in structured data |
// | 60-64       | uint32 | Index of `canonicalSourceFileName`, or noStructuredData         |
// | 64-68       | uint32 | Index of `contentMapper`, or noStructuredData                   |
// | 68-72       | uint32 | Index of `virtualFileName`, or noStructuredData                 |
// | 72-76       | uint32 | Byte offset of `diagnosticDirectives` in structured data       |
//
// Structured data (variable)
// --------------------------
//
// The structured data section contains msgpack-encoded metadata blobs. Each blob is a self-contained
// msgpack value. File reference arrays use the following tuple format:
//
//   [pos: uint, end: uint, fileName: string, resolutionMode: uint, preserve: bool]
//
// Node index arrays (imports, moduleAugmentations) are msgpack arrays of uint values, where each
// value is a node index into the nodes section. String arrays (ambientModuleNames) are msgpack
// arrays of string values.
//
// Span maps are msgpack arrays of tuples in UTF-16 coordinates:
//
//   [virtualStart: uint, virtualLength: uint, originalStart: uint, originalLength: uint, kind: uint, features?: uint]
//
// Diagnostic directives are msgpack arrays of normalized tuples in UTF-16 coordinates:
//
//   [originalStart: uint, originalLength: uint, virtualStart: uint, virtualLength: uint, policy: uint, unusedCode: uint]
//
// An offset of 0xFFFFFFFF indicates no data (empty array).
//
// Nodes (28 bytes per node)
// -------------------------
//
// The nodes section contains the AST structure of the file. Nodes are represented in a flat array in source order,
// heavily inspired by https://marvinh.dev/blog/speeding-up-javascript-ecosystem-part-11/. Each node has the following
// structure:
//
// | Byte offset | Type   | Field                      |
// | ----------- | ------ | -------------------------- |
// | 0-4         | uint32 | Kind                       |
// | 4-8         | uint32 | Pos                        |
// | 8-12        | uint32 | End                        |
// | 12-16       | uint32 | Node index of next sibling |
// | 16-20       | uint32 | Node index of parent       |
// | 20-24       |        | Node data                  |
// | 24-28       | uint32 | Node flags                 |
//
// The first 28 bytes of the nodes section are zeros representing a nil node, such that nodes without a parent or next
// sibling can unambiuously use `0` for those indices.
//
// NodeLists are represented as normal nodes with the special `kind` value `0xff_ff_ff_ff`. They are considered the parent
// of their contents in the encoded format. A client reconstructing an AST similar to TypeScript's internal representation
// should instead set the `parent` pointers of a NodeList's children to the NodeList's parent. A NodeList's `data` field
// is the uint32 length of the list, and does not use one of the data types described below. A NodeList's `flags` field
// is not used for AST node flags (NodeLists have none); bit 0 instead encodes `HasTrailingComma`.
//
// For node types other than NodeList, the node data field encodes one of the following, determined by the first 2 bits of
// the field:
//
// | Value | Data type | Description                                                                          |
// | ----- | --------- | ------------------------------------------------------------------------------------ |
// | 0b00  | Children  | Disambiguates which named properties of the node its children should be assigned to. |
// | 0b01  | String    | The index of the node's string property in the **string offsets** section.           |
// | 0b10  | Extended  | The byte offset of the node's extended data into the **extended node data** section. |
// | 0b11  | Reserved  | Reserved for future use.                                                             |
//
// In all node data types, the remaining 6 bits of the first byte are used to encode small values specific to the node
// type. For most node types, these are individual boolean flags. For unary expressions, all 6 bits encode the operator's
// SyntaxKind value (e.g., PlusPlusToken=45, TildeToken=54), which fits because KindLastUnaryOperator (54) <= 0x3f (63).
//
// | Node type                    | Bits 0-5                              | Notes                          |
// | ---------------------------- | ------------------------------------- | ------------------------------ |
// | `ImportSpecifier`            | Bit 0: `isTypeOnly`                   |                                |
// | `ImportClause`               | Bit 0: `isTypeOnly`, Bit 1: `isDefer` |                                |
// | `ExportSpecifier`            | Bit 0: `isTypeOnly`                   |                                |
// | `ImportEqualsDeclaration`    | Bit 0: `isTypeOnly`                   |                                |
// | `ExportDeclaration`          | Bit 0: `isTypeOnly`                   |                                |
// | `ImportTypeNode`             | Bit 0: `isTypeOf`                     |                                |
// | `ExportAssignment`           | Bit 0: `isExportEquals`               |                                |
// | `Block`                      | Bit 0: `multiline`                    |                                |
// | `ArrayLiteralExpression`     | Bit 0: `multiline`                    |                                |
// | `ObjectLiteralExpression`    | Bit 0: `multiline`                    |                                |
// | `JsxText`                    | Bit 0: `containsOnlyTriviaWhiteSpaces`|                                |
// | `JSDocTypeLiteral`           | Bit 0: `isArrayType`                  |                                |
// | `JsDocPropertyTag`           | Bit 0: `isBracketed`, Bit 1: `isNameFirst` |                           |
// | `JsDocParameterTag`          | Bit 0: `isBracketed`, Bit 1: `isNameFirst` |                           |
// | `VariableDeclarationList`    | Bit 0: is `let`, Bit 1: is `const`    |                                |
// | `ImportAttributes`           | Bit 0: `multiline`, Bit 1: is `assert`|                                |
// | `PrefixUnaryExpression`      | Bits 0-5: operator SyntaxKind         | e.g., `!`, `~`, `++`, `--`    |
// | `PostfixUnaryExpression`     | Bits 0-5: operator SyntaxKind         | e.g., `++`, `--`               |
//
// The remaining 3 bytes of the node data field vary by data type:
//
// ### Children (0b00)
//
// If a node has fewer children than its type allows, additional data is needed to determine which properties the children
// correspond to. The last byte of the 4-byte data field is a bitmask representing the child properties of the node type,
// in visitor order, where `1` indicates that the child at that property is present and `0` indicates that the property is
// nil. For example, a `MethodDeclaration` has the following child properties:
//
// | Property name  | Bit position |
// | -------------- | ------------ |
// | modifiers      | 0            |
// | asteriskToken  | 1            |
// | name           | 2            |
// | postfixToken   | 3            |
// | typeParameters | 4            |
// | parameters     | 5            |
// | returnType     | 6            |
// | body           | 7            |
//
// A bitmask with value `0b01100101` would indicate that the next four direct descendants (i.e., node records that have a
// `parent` set to the node index of the `MethodDeclaration`) of the node are its `modifiers`, `name`, `parameters`, and
// `body` properties, in that order. The remaining properties are nil. (To reconstruct the node with named properties, the
// client must consult a static table of each node type's child property names.)
//
// The bitmask may be zero for node types that can only have a single child, since no disambiguation is needed.
// Additionally, the children data type may be used for nodes that can never have children, but do not require other
// data types.
//
// ### String (0b01)
//
// The string data type is used for nodes with a single string property. (Currently, the name of that property is always
// `text`.) The last three bytes of the 4-byte data field form a single 24-bit unsigned integer (i.e.,
// `uint32(0x00_ff_ff_ff & node.data)`) _N_ that is an index into the **string offsets** section. The *N*th 32-bit
// unsigned integer in the **string offsets** section is the byte offset of the start of the string in the **string data**
// section, and the *N+1*th 32-bit unsigned integer is the byte offset of the end of the string in the
// **string data** section.
//
// ### Extended (0b10)
//
// The extended data type is used for nodes with properties that don't fit into either the children or string data types.
// The last three bytes of the 4-byte data field form a single 24-bit unsigned integer (i.e.,
// `uint32(0x00_ff_ff_ff & node.data)`) _N_ that is a byte offset into the **extended node data** section. The length and
// meaning of the data at that offset is defined by the node type. See the **Extended node data** section for details on
// the format of the extended data for specific node types.
//
// Encoding Arbitrary Nodes
// ------------------------
//
// The same binary format can be used to encode an arbitrary subtree of a SourceFile, not just a whole SourceFile. When
// encoding a non-SourceFile node, the format is identical with the following differences:
//
// - The content hash fields in the header (bytes 4-19) are zero.
// - The parse options field in the header (bytes 20-23) is zero.
// - The root node in the nodes section uses its actual node kind and data encoding (via getNodeData) rather than the
//   SourceFile-specific extended data format.
//
// The string data section contains only the strings referenced by nodes in the subtree, rather than the full source
// file text. The EncodeNode function provides this entrypoint.

/// Go `xxh3.Uint128`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Uint128 {
    hi: u64,
    lo: u64,
}

/// Go's value of the hash `h` of the file `parsed`: `h`, or the content hash
/// of the Go bytes of its text when `h` is the content hash of its port form.
// PORT: Go hashes the Go bytes of the file content (`xxh3.HashString128`,
// project/overlayfs.go:86 newCachedFile, :134 newOverlay, and
// project/snapshotfs.go:411, :441 and :567), and the parse cache sets
// `file.Hash = fh.Hash()` (project/parsecache.go:79). The port's file handles
// hash the port form (project/overlayfs.rs, snapshotfs.rs), which is the same
// identity for the parse cache keys. The two differ only for a text with a
// marker unit (an invalid byte, a WTF-8 surrogate or a real U+FDD0, see
// `scanner_util::GO_STRING_MARKER`), and the API header is the only place
// where Go shows the value. So the hash is made again here, for such a text
// only. A content-mapped file's hash is the hash of its cache key, not of
// its text, so it is kept: Go's key holds Go's hash of the raw content
// (project/parsecache.go:44), which the port does not give for a raw
// content with a marker unit.
fn go_content_hash(parsed: &ParsedSourceFile, h: u128) -> u128 {
    let text = parsed.text();
    if !crate::scanner_util::contains_go_string_marker(text)
        || h != xxhash_rust::xxh3::xxh3_128(text.as_bytes())
    {
        return h;
    }
    xxhash_rust::xxh3::xxh3_128(&crate::scanner_util::go_string_bytes(text))
}

/// Go `sourceFile.Hash`.
// PORT: Go `SourceFile.Hash` is `ParsedSourceFile::hash`. At pin B only the
// two project parse caches set it:
// - project/parsecache.go NewParseCache sets `fh.Hash()`, the xxh3-128 of
//   the file content, which is the parsed text.
// - project/compilerhost.go GetContentMappedSourceFiles (tsgo#4712) sets
//   the hash of the cache key on the canonical file and on each
//   supplemental file of a content-mapped file.
// This reads it (`ParsedSourceFile::source_hash`, the xxh3-128 of the text
// for a file that has none) for a file of a language server program, which
// the parse caches made. Any other parse
// (Go `parser.ParseSourceFile`, for example a tsconfig from
// `tsoptions.NewTsconfigSourceFileFromFilePath`) keeps Go Hash 0. Such a
// file is in no program, also when a publish kept its parse (the root
// config of a project, api/session.go handleGetConfigSourceFile at pin B).
// So the test is "a program made here has this file"
// (`program_parsed_source_file`), not "some parse of this file exists"
// (`parsed_source_file_of`, which also finds that root config).
// At pin N a file of no program can also come from the parse cache:
// api/session.go createSourceFile (ts#64434) takes its file from the
// snapshot host's parse cache, so its Go `Hash` is `fh.Hash()`, and a
// program that has the same file shares it (the client keeps one object per
// content hash). Such a file keeps the hash that the parse cache set
// (`ParsedSourceFile::hash`, `parse_cache_hash`); the root config has none.
// The encode of a lease reads it from the lease's record
// (`encode_parsed_source_file`), with no lookup.
fn source_file_content_hash(source_file: Node) -> Uint128 {
    if source_file.is_nil() || is_synthetic_node(source_file) {
        return Uint128::default();
    }
    match crate::program::ls_program::program_parsed_source_file(source_file) {
        Some(parsed) => uint128_of(go_content_hash(&parsed, parsed.source_hash())),
        None => parsed_source_file_of(source_file)
            .map_or_else(Uint128::default, |parsed| parse_cache_hash(&parsed)),
    }
}

/// Go `sourceFile.Hash` of the file of `parsed`: the hash that a parse cache
/// set (Go `file.Hash = fh.Hash()`, project/parsecache.go:79), else 0.
// PORT: for a parse cache file this equals the program branch of
// `source_file_content_hash` (`source_hash()`), since the parse cache always
// sets `hash` (project/parsecache.rs).
fn parse_cache_hash(parsed: &ParsedSourceFile) -> Uint128 {
    parsed
        .hash
        .get()
        .map_or_else(Uint128::default, |h| uint128_of(go_content_hash(parsed, h)))
}

/// Go `Uint128{Hi, Lo}` of a 128-bit hash.
fn uint128_of(h: u128) -> Uint128 {
    Uint128 {
        hi: (h >> 64) as u64,
        lo: h as u64,
    }
}

/// The parsed-file record that holds the Go `SourceFile` fields
/// `ParseOptions()`, `NodeCount` and `TextCount`, or None.
// PORT: Go keeps these fields on the SourceFile node. ts_goport keeps them in
// `ParsedSourceFile`, which the language server programs hold. A file that
// is parsed outside a program (Go `parser.ParseSourceFile`) has the parse
// that `program::note_parsed_source_file` recorded. A factory SourceFile has
// none. A leased file of an api `createSourceFile` does not use this lookup:
// its lease holds the record, also after the program that loaded the file is
// released (`encode_parsed_source_file`).
fn parsed_source_file_of(source_file: Node) -> Option<Rc<ParsedSourceFile>> {
    if source_file.is_nil() || is_synthetic_node(source_file) {
        return None;
    }
    crate::program::ls_program::parsed_source_file(source_file)
}

/// Go `sourceFile.NodeCount`.
// PORT: Go uses it and `TextCount` only as slice capacities. Without a
// parsed-file record the capacity is 0; the output does not change.
// `encode_tree` reads both from its record.
fn source_file_node_count(source_file: Node) -> usize {
    parsed_source_file_of(source_file).map_or(0, |file| file.node_count)
}

/// The Go `SourceFile` fields that the parser sets and the encoder reads:
/// `Path()`, `Imports()`, `ModuleAugmentations`, `AmbientModuleNames`,
/// `ExternalModuleIndicator`, the file references, `LanguageVariant` and
/// `ScriptKind`.
// PORT: Go reads them from the `*ast.SourceFile`. A published file has them
// in its `GoFile` (`source_file_info`). A file that is parsed outside a
// program (Go `parser.ParseSourceFile`, as the encoder tests do) is not
// published and has no `GoFile`; its `ParsedSourceFile` has them. The
// leased file of an api `createSourceFile` is published, but its encode
// reads the `ParsedSourceFile` that the lease holds, as Go reads the leased
// object. The `GoFile` has copies of the same lists
// (`program_file_info`), so the output is the same.
enum SourceFileFields {
    Published(FileRef<SourceFileInfo>),
    Parsed(Rc<ParsedSourceFile>),
}

/// The same field of either `SourceFileFields` variant.
macro_rules! source_file_field {
    ($fields:expr, $field:ident) => {
        match $fields {
            SourceFileFields::Published(info) => &info.$field,
            SourceFileFields::Parsed(file) => &file.$field,
        }
    };
}

impl SourceFileFields {
    fn path(&self) -> &str {
        match self {
            SourceFileFields::Published(info) => &info.path,
            SourceFileFields::Parsed(file) => &file.path().0,
        }
    }

    fn imports(&self) -> &[Node] {
        source_file_field!(self, imports)
    }

    fn module_augmentations(&self) -> &[Node] {
        source_file_field!(self, module_augmentations)
    }

    fn ambient_module_names(&self) -> &[String] {
        source_file_field!(self, ambient_module_names)
    }

    fn external_module_indicator(&self) -> Node {
        *source_file_field!(self, external_module_indicator)
    }

    fn referenced_files(&self) -> &[FileReference] {
        source_file_field!(self, referenced_files)
    }

    fn type_reference_directives(&self) -> &[FileReference] {
        source_file_field!(self, type_reference_directives)
    }

    fn lib_reference_directives(&self) -> &[FileReference] {
        source_file_field!(self, lib_reference_directives)
    }

    fn language_variant(&self) -> LanguageVariant {
        *source_file_field!(self, language_variant)
    }

    fn script_kind(&self) -> ScriptKind {
        *source_file_field!(self, script_kind)
    }
}

/// `SourceFileFields` of `sf`. `given` is the parsed-file record of `sf`
/// when the caller of `encode_tree` holds it (`encode_parsed_source_file`).
// PORT: Go reads the fields from the `*ast.SourceFile`. The leased file of
// an api `createSourceFile` has them in its record, as Go has them on the
// leased object, so its encode reads them there.
fn source_file_fields(sf: Node, given: Option<&Rc<ParsedSourceFile>>) -> SourceFileFields {
    if let Some(file) = given {
        return SourceFileFields::Parsed(Rc::clone(file));
    }
    if !is_synthetic_node(sf)
        && is_file_store_before_program(sf.file_index())
        && let Some(parsed) = parsed_source_file_of(sf)
    {
        return SourceFileFields::Parsed(parsed);
    }
    SourceFileFields::Published(source_file_info(sf))
}

/// Go `%v` of an `ast.Kind` (the generated stringer): "KindX", or "Kind(N)"
/// out of range.
// PORT: `crate::astdata::SyntaxKind::as_str` gives the Go names without the `Kind`
// prefix. Go `ast.Kind` is an `int16`.
pub fn go_kind_string(kind: i16) -> String {
    if kind >= 0
        && let Ok(k) = SyntaxKind::try_from(kind as u16)
    {
        return format!("Kind{}", k.as_str());
    }
    format!("Kind({kind})")
}

// Go: api/encoder/encoder.go:312 SourceFileHash
/// SourceFileHash returns the 128-bit content hash for a source file as a hex string.
pub fn source_file_hash(source_file: Node) -> String {
    let h = source_file_content_hash(source_file);
    format!("{:016x}{:016x}", h.hi, h.lo)
}

// Go: api/encoder/encoder.go:318 encodeParseOptions
/// encodeParseOptions encodes the per-file ExternalModuleIndicatorOptions as a uint32 bitmask.
fn encode_parse_options(opts: ExternalModuleIndicatorOptions) -> u32 {
    let mut bits: u32 = 0;
    if opts.jsx {
        bits |= 1;
    }
    if opts.force {
        bits |= 2;
    }
    bits
}

// Go: api/encoder/encoder.go:330 NodeIndexTable
/// NodeIndexTable maps between AST nodes and their encoder indices for O(1) node handle resolution.
// PORT: Go `sortedOnce sync.Once` and `sortedIdx []uint32` are one `OnceCell`.
pub struct NodeIndexTable {
    /// index → node (for resolution). `Node::NIL` for a NodeList slot.
    pub nodes: Vec<Node>,
    /// indices into Nodes, sorted by node ID; built lazily
    sorted_idx: OnceCell<Vec<u32>>,
}

// Go: api/encoder/encoder.go:336 nodeIndexTableKey
static NODE_INDEX_TABLE_KEY: LazyLock<SourceFileDataKey<Rc<NodeIndexTable>>> =
    LazyLock::new(new_source_file_data_key::<Rc<NodeIndexTable>>);

impl NodeIndexTable {
    // Go: api/encoder/encoder.go:342 (*NodeIndexTable).GetIndex
    /// GetIndex returns the encoder index for the given node.
    /// On the first call the sortedIdx array is built (O(n log n) sort on a flat []uint32),
    /// then subsequent calls use binary search (O(log n)). This turns out to be much faster than
    /// building a map[*ast.Node]uint32 and not significantly slower for lookups.
    pub fn get_index(&self, node: Node) -> u32 {
        let sorted_idx = self.sorted_idx.get_or_init(|| {
            let mut idx: Vec<u32> = Vec::with_capacity(self.nodes.len());
            for (i, n) in self.nodes.iter().enumerate() {
                if n.is_some() {
                    idx.push(i as u32);
                }
            }
            let nodes = &self.nodes;
            crate::gostd::slices::sort_func(&mut idx, |a: &u32, b: &u32| {
                get_node_id(nodes[*a as usize]).cmp(&get_node_id(nodes[*b as usize])) as i32
            });
            idx
        });
        let target = get_node_id(node);
        let (i, found) = binary_search_unique_func(sorted_idx, |_: i32, el: u32| {
            get_node_id(self.nodes[el as usize]).cmp(&target) as i32
        });
        if found {
            return sorted_idx[i as usize];
        }
        0
    }
}

/// PORT: the counter and table that Go `BuildNodeIndexTable` closures
/// capture. They live in the visitor's `ctx` (see `ast/visitor.rs`).
struct BuildNodeIndexTableState {
    node_count: u32,
    node_table: Vec<Node>,
    source_file: Node,
}

// Go: api/encoder/encoder.go:370 BuildNodeIndexTable
/// BuildNodeIndexTable walks the AST in the same order as encodeTree and builds
/// a NodeIndexTable without performing the full binary encoding. This is used to
/// eagerly create index tables for files that need node handles before getSourceFile
/// is called. The indices produced are guaranteed to match those from EncodeSourceFile.
pub fn build_node_index_table(source_file: Node) -> Rc<NodeIndexTable> {
    let node_count: u32 = 0;
    let mut node_table: Vec<Node> = Vec::with_capacity(source_file_node_count(source_file) + 1);
    node_table.push(Node::NIL); // index 0 = nil sentinel

    // PORT: Go builds the visitor with only `Hooks` and sets `Visit` after;
    // the Rust callbacks get the visitor as an argument instead.
    let hooks: NodeVisitorHooks<'_, BuildNodeIndexTableState> = NodeVisitorHooks {
        visit_nodes: Some(Rc::new(
            |node_list: NodeList,
             visitor: &mut NodeVisitor<'_, BuildNodeIndexTableState>|
             -> NodeList {
                if node_list.is_nil() {
                    return node_list;
                }
                visitor.ctx.node_count += 1;
                visitor.ctx.node_table.push(Node::NIL); // NodeLists are not *ast.Node
                // PERF: (apiperf1) in place, as in `encode_tree`.
                let _ = visitor.visit_slice_changed(node_list.nodes().iter());
                node_list
            },
        )),
        visit_modifiers: Some(Rc::new(
            |modifiers: ModifierList,
             visitor: &mut NodeVisitor<'_, BuildNodeIndexTableState>|
             -> ModifierList {
                if modifiers.is_some() && !modifiers.nodes().is_empty() {
                    let visit_nodes = visitor
                        .hooks
                        .visit_nodes
                        .clone()
                        .expect("VisitNodes hook is set");
                    visit_nodes(modifiers.node_list(), visitor);
                }
                modifiers
            },
        )),
        ..NodeVisitorHooks::default()
    };
    let mut visitor = new_node_visitor(
        |node: Node, visitor: &mut NodeVisitor<'_, BuildNodeIndexTableState>| -> Node {
            visitor.ctx.node_count += 1;
            visitor.ctx.node_table.push(node);
            visitor.visit_each_child(node);
            let source_file = visitor.ctx.source_file;
            for jsdoc in node.js_doc(source_file) {
                let visit = visitor.visit.clone().expect("NodeVisitor.Visit is nil");
                visit(jsdoc, visitor);
            }
            node
        },
        None,
        hooks,
        BuildNodeIndexTableState {
            node_count,
            node_table,
            source_file,
        },
    );

    let root_node = source_file;
    // Index 1 = root node (matches encodeTree)
    visitor.ctx.node_count += 1;
    visitor.ctx.node_table.push(root_node);

    visitor.visit_each_child(root_node);
    for jsdoc in root_node.js_doc(source_file) {
        let visit = visitor.visit.clone().expect("NodeVisitor.Visit is nil");
        visit(jsdoc, &mut visitor);
    }

    Rc::new(NodeIndexTable {
        nodes: visitor.ctx.node_table,
        sorted_idx: OnceCell::new(),
    })
}

// Go: api/encoder/encoder.go:416 GetNodeIndexTable
pub fn get_node_index_table(source_file: Node) -> Rc<NodeIndexTable> {
    crate::ast::source_file_ls::source_file_get_or_compute_data(
        source_file,
        &*NODE_INDEX_TABLE_KEY,
        build_node_index_table,
    )
}

// Go: api/encoder/encoder.go:422 EncodeSourceFile
/// EncodeSourceFile encodes an entire source file AST into the binary format.
/// Returns the encoded bytes and a NodeIndexTable mapping encoder indices to AST nodes.
pub fn encode_source_file(source_file: Node) -> Result<(Vec<u8>, Rc<NodeIndexTable>), GoError> {
    encode_source_file_of(source_file, None)
}

/// Go `EncodeSourceFile` of the file of `file`, a record that the caller
/// holds: the leased file of api/session.go:1876 encodeLeasedSourceFile.
// PORT: Go reads `Hash` and `ParseOptions()` from the leased
// `*ast.SourceFile` (api/encoder/encoder.go:595-596). The lease holds the
// `ParsedSourceFile`, so the encoder reads them from it. A lookup by the root
// node finds no record once the program that loaded the file is released.
pub fn encode_parsed_source_file(
    file: &Rc<ParsedSourceFile>,
) -> Result<(Vec<u8>, Rc<NodeIndexTable>), GoError> {
    encode_source_file_of(file.root, Some(file))
}

/// `encode_source_file` with the parsed-file record of `source_file` when
/// the caller has it.
fn encode_source_file_of(
    source_file: Node,
    parsed: Option<&Rc<ParsedSourceFile>>,
) -> Result<(Vec<u8>, Rc<NodeIndexTable>), GoError> {
    let (data, node_table) = encode_tree(source_file, source_file, parsed)?;
    let node_table = crate::ast::source_file_ls::source_file_get_or_compute_data(
        source_file,
        &*NODE_INDEX_TABLE_KEY,
        |_: Node| node_table,
    );
    Ok((data, node_table))
}

// Go: api/encoder/encoder.go SetSourceFileLease (ts#64434)
/// SetSourceFileLease sets the session-scoped lease ID in an encoded source file.
pub fn set_source_file_lease(data: &mut [u8], lease: u64) {
    data[HEADER_OFFSET_SOURCE_FILE_LEASE..HEADER_OFFSET_SOURCE_FILE_LEASE + 8]
        .copy_from_slice(&lease.to_le_bytes());
}

// Go: api/encoder/encoder.go:442 EncodeNode
/// EncodeNode encodes an arbitrary AST node and its descendants into the binary format.
/// The sourceFile is needed to provide the source text for efficient string encoding.
/// When encoding a non-SourceFile node, the header hash and parse options fields will be zero.
/// Returns the encoded bytes and a NodeIndexTable mapping encoder indices to AST nodes.
// PORT: Go nil `sourceFile` is `Node::NIL`.
pub fn encode_node(
    node: Node,
    source_file: Node,
) -> Result<(Vec<u8>, Rc<NodeIndexTable>), GoError> {
    encode_tree(node, source_file, None)
}

/// PORT: the local variables that Go `encodeTree` closures capture. They
/// live in the visitor's `ctx` (see `ast/visitor.rs`), so the callbacks
/// reach them through the visitor they get.
struct EncodeTreeState {
    parent_index: u32,
    node_count: u32,
    prev_index: u32,
    extended_data: Vec<u8>,
    structured_data: Vec<u8>,
    strs: StringTable,
    position_map: Cow<'static, PositionMap>,
    nodes: Vec<u8>,
    node_table: Vec<Node>,
    node_index_map: Option<FxHashMap<Node, u32>>,
    source_file: Node,
}

impl EncodeTreeState {
    /// Go closure `utf16` in `encodeTree`.
    fn utf16(&self, pos: i32) -> u32 {
        self.position_map.utf8_to_utf16(pos) as u32
    }
}

// Go: api/encoder/encoder.go:446 encodeTree
// PORT: `given` is the parsed-file record of `source_file` when the caller
// holds it (`encode_parsed_source_file`, where `root_node` is `source_file`).
// It gives the Go `SourceFile` fields `Hash`, `ParseOptions()`, `NodeCount`,
// `TextCount` and the parser fields (`source_file_fields`).
fn encode_tree(
    root_node: Node,
    source_file: Node,
    given: Option<&Rc<ParsedSourceFile>>,
) -> Result<(Vec<u8>, Rc<NodeIndexTable>), GoError> {
    // Protocol 9 has no code-selector representation. Refuse export rather than
    // silently turning a selective directive into a broad one for API clients.
    if root_node.kind() == SyntaxKind::SourceFile
        && source_file_diagnostic_directives(root_node)
            .iter()
            .any(|directive| directive.diagnostic_codes.is_some())
    {
        return Err(errors::errorf(
            "binary AST protocol 9 cannot encode diagnostic code selectors".to_string(),
            Vec::new(),
        ));
    }
    let parsed = given
        .cloned()
        .or_else(|| parsed_source_file_of(source_file));
    let (parent_index, node_count, prev_index): (u32, u32, u32) = (0, 0, 0);
    let extended_data: Vec<u8> = Vec::new();
    let structured_data: Vec<u8> = Vec::new();
    let strs: StringTable;
    let mut position_map: Option<FileRef<PositionMap>> = None;
    if root_node.kind() == SyntaxKind::SourceFile {
        strs = new_string_table(
            source_file_text(source_file),
            parsed.as_ref().map_or(0, |file| file.text_count),
        );
        position_map = Some(source_file_get_position_map(source_file));
    } else {
        strs = new_string_table("", 0);
        if source_file.is_some() {
            position_map = Some(source_file_get_position_map(source_file));
        }
    }
    // lsshells M3b: the map of a freeable file version is copied, so the
    // encoder keeps no file version alive.
    let position_map: Cow<'static, PositionMap> = match position_map {
        Some(map) => match map.as_static() {
            Some(map) => Cow::Borrowed(map),
            None => Cow::Owned((*map).clone()),
        },
        None => Cow::Owned(compute_position_map("")),
    };
    let mut initial_node_count: usize = 0;
    if source_file.is_some() {
        initial_node_count = parsed.as_ref().map_or(0, |file| file.node_count);
    }
    let nodes: Vec<u8> = Vec::with_capacity((initial_node_count + 1) * NODE_SIZE);

    // Build node index table for O(1) handle resolution.
    // Index 0 is a nil sentinel; real nodes start at index 1.
    let mut node_table: Vec<Node> = Vec::with_capacity(initial_node_count + 1);
    node_table.push(Node::NIL); // index 0 = nil sentinel

    // Build a small map of nodes we need to track indices for (imports + moduleAugmentations).
    // Values start at 0 and are filled in during the walk.
    let mut node_index_map: Option<FxHashMap<Node, u32>> = None;
    let sf_extended_data_offset: usize; // byte offset in extendedData where SourceFile fields start
    if root_node.kind() == SyntaxKind::SourceFile {
        let sf = root_node;
        let fields = source_file_fields(sf, given);
        let mut total = fields.imports().len() + fields.module_augmentations().len();
        if fields.external_module_indicator().is_some()
            && fields.external_module_indicator() != root_node
        {
            total += 1;
        }
        if total > 0 {
            let mut map: FxHashMap<Node, u32> =
                FxHashMap::with_capacity_and_hasher(total, Default::default());
            for &imp in fields.imports() {
                map.insert(imp, 0);
            }
            for &aug in fields.module_augmentations() {
                map.insert(aug, 0);
            }
            if fields.external_module_indicator().is_some()
                && fields.external_module_indicator() != root_node
            {
                map.insert(fields.external_module_indicator(), 0);
            }
            node_index_map = Some(map);
        }
    }

    // PERF: (apiperf2) the nodes of a published store file are encoded from
    // their node data (`StoreFile`, `encode_visit`).
    let file_nodes = FileNodes::of(root_node.file_index()).filter(|_| !visitor_only());
    let store_file = file_nodes.as_ref().map(|nodes| StoreFile {
        file: root_node.file_index(),
        nodes: nodes.reader(root_node.file_index()),
    });
    let fast = store_file.as_ref();

    // PORT: Go builds the visitor with only `Hooks` and sets `Visit` after;
    // the Rust callbacks get the visitor as an argument instead.
    let hooks: NodeVisitorHooks<'_, EncodeTreeState> = NodeVisitorHooks {
        visit_nodes: Some(Rc::new(
            move |node_list: NodeList, visitor: &mut EncodeVisitor<'_>| -> NodeList {
                encode_visit_nodes(node_list, visitor, fast)
            },
        )),
        visit_modifiers: Some(Rc::new(
            |modifiers: ModifierList, visitor: &mut EncodeVisitor<'_>| -> ModifierList {
                if modifiers.is_some() && !modifiers.nodes().is_empty() {
                    let visit_nodes = visitor
                        .hooks
                        .visit_nodes
                        .clone()
                        .expect("VisitNodes hook is set");
                    visit_nodes(modifiers.node_list(), visitor);
                }
                modifiers
            },
        )),
        ..NodeVisitorHooks::default()
    };
    let mut visitor = new_node_visitor(
        move |node: Node, visitor: &mut EncodeVisitor<'_>| -> Node {
            encode_visit(node, visitor, fast)
        },
        None,
        hooks,
        EncodeTreeState {
            parent_index,
            node_count,
            prev_index,
            extended_data,
            structured_data,
            strs,
            position_map,
            nodes,
            node_table,
            node_index_map,
            source_file,
        },
    );

    {
        let st = &mut visitor.ctx;
        append_uint32s(&mut st.nodes, &[0, 0, 0, 0, 0, 0, 0]);

        st.node_count += 1;
        st.parent_index += 1;
        st.node_table.push(root_node); // index 1 = root node

        sf_extended_data_offset = st.extended_data.len();
        let kind = root_node.kind() as u32;
        let pos = st.utf16(root_node.pos());
        let end = st.utf16(root_node.end());
        let data = get_node_data(
            root_node,
            &mut st.strs,
            &st.position_map,
            &mut st.extended_data,
            &mut st.structured_data,
            given,
        );
        let flags = root_node.flags().0;
        append_uint32s(&mut st.nodes, &[kind, pos, end, 0, 0, data, flags]);
    }

    encode_each_child(root_node, &mut visitor, fast);
    if source_file.is_some() {
        for jsdoc in root_node.js_doc(source_file) {
            let visit = visitor.visit.clone().expect("NodeVisitor.Visit is nil");
            visit(jsdoc, &mut visitor);
        }
    }

    let EncodeTreeState {
        mut extended_data,
        mut structured_data,
        strs,
        nodes,
        node_table,
        node_index_map,
        ..
    } = visitor.ctx;

    let mut hash = Uint128::default();
    let mut parse_opts: u32 = 0;
    if root_node.kind() == SyntaxKind::SourceFile {
        let parsed = parsed.unwrap_or_else(|| unported!("SourceFile.ParseOptions"));
        hash = if given.is_some() {
            parse_cache_hash(&parsed)
        } else {
            source_file_content_hash(source_file)
        };
        parse_opts = encode_parse_options(parsed.parse_options().external_module_indicator_options);

        // Encode imports, moduleAugmentations, and ambientModuleNames into structured data,
        // and patch the placeholder offsets in the SourceFile extended data.
        let sf = root_node;
        let fields = source_file_fields(sf, given);
        let imports_offset = encode_node_index_array(
            fields.imports(),
            node_index_map.as_ref(),
            &mut structured_data,
        );
        let module_augmentations_offset = encode_module_augmentations(
            fields.module_augmentations(),
            node_index_map.as_ref(),
            &mut structured_data,
        );
        let ambient_module_names_offset =
            encode_string_array(fields.ambient_module_names(), &mut structured_data);
        // Patch the 3 placeholder uint32s at sfExtendedDataOffset + 32, 36, 40
        put_uint32(
            &mut extended_data,
            sf_extended_data_offset + 32,
            imports_offset,
        );
        put_uint32(
            &mut extended_data,
            sf_extended_data_offset + 36,
            module_augmentations_offset,
        );
        put_uint32(
            &mut extended_data,
            sf_extended_data_offset + 40,
            ambient_module_names_offset,
        );
        // Patch externalModuleIndicator node index at offset 44
        let mut external_module_indicator_index: u32 = 0;
        if fields.external_module_indicator().is_some() {
            if fields.external_module_indicator() == root_node {
                external_module_indicator_index = 1; // root node index
            } else {
                external_module_indicator_index = node_index_map
                    .as_ref()
                    .and_then(|map| map.get(&fields.external_module_indicator()).copied())
                    .unwrap_or(0);
            }
        }
        put_uint32(
            &mut extended_data,
            sf_extended_data_offset + 44,
            external_module_indicator_index,
        );
    }

    let metadata = (PROTOCOL_VERSION as u32) << 24;
    let offset_string_table_offsets = HEADER_SIZE;
    let offset_string_table_data = HEADER_SIZE + strs.offsets.len() * 4;
    let offset_extended_data = offset_string_table_data + strs.string_length();
    let offset_structured_data = offset_extended_data + extended_data.len();
    let offset_nodes = offset_structured_data + structured_data.len();

    let header: [u32; 16] = [
        metadata,
        hash.lo as u32,
        (hash.lo >> 32) as u32,
        hash.hi as u32,
        (hash.hi >> 32) as u32,
        parse_opts,
        offset_string_table_offsets as u32,
        offset_string_table_data as u32,
        offset_extended_data as u32,
        offset_structured_data as u32,
        offset_nodes as u32,
        // ts#64434
        0,
        0, // source file ID
        0,
        0, // source file lease ID
        0, // binder data offset
    ];

    let mut header_bytes: Vec<u8> = Vec::new();
    append_uint32s(&mut header_bytes, &header);
    let strs_bytes = strs.encode();

    Ok((
        [
            header_bytes.as_slice(),
            strs_bytes.as_slice(),
            extended_data.as_slice(),
            structured_data.as_slice(),
            nodes.as_slice(),
        ]
        .concat(),
        Rc::new(NodeIndexTable {
            nodes: node_table,
            sorted_idx: OnceCell::new(),
        }),
    ))
}

/// The visitor of `encode_tree`.
type EncodeVisitor<'a> = NodeVisitor<'a, EncodeTreeState>;

#[cfg(test)]
thread_local! {
    /// Tests: `encode_tree` encodes every node with the visitor, as Go does.
    static VISITOR_ONLY: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// True when `encode_tree` encodes every node with the visitor (tests).
#[inline]
fn visitor_only() -> bool {
    #[cfg(test)]
    return VISITOR_ONLY.with(std::cell::Cell::get);
    #[cfg(not(test))]
    false
}

/// The published store file of the node that `encode_tree` encodes, whose
/// nodes `encode_visit` encodes from their node data.
// PERF: (apiperf2) Go `encodeTree` walks the tree with `VisitEachChild`.
// Here the visitor read each child field of a node three times (the
// property mask, the visit, the `Update` compare), each with a store
// lookup, and a node of a freeable file version (an API source file lease)
// with a pin per read: the encoder took 3.7 times Go's cycles.
struct StoreFile<'a> {
    file: usize,
    nodes: FileNodeReader<'a>,
}

/// True when `encode_visit` encodes the children of a node of kind `kind`
/// of the store file from its node data: Go `VisitEachChild` visits the
/// fields of `ForEachChild` (`for_each_child_field`) in the same order.
/// JSDoc nodes and a SyntaxList take the visitor.
#[inline]
fn walks_node_data(kind: SyntaxKind) -> bool {
    !is_js_doc_kind(kind) && kind != SyntaxKind::SyntaxList
}

/// The node data of `node`, of kind `kind`, when `encode_visit` encodes it
/// from its node data: a node of store file `fast` (`walks_node_data`).
#[inline]
fn store_node_data<'f>(
    node: Node,
    kind: SyntaxKind,
    fast: Option<&StoreFile<'f>>,
) -> Option<&'f NodeData> {
    let fast = fast?;
    if node.file_index() != fast.file || !walks_node_data(kind) {
        return None;
    }
    Some(fast.nodes.data(node))
}

// Go: api/encoder/encoder.go:540 (the visitor.Visit func of encodeTree)
fn encode_visit(node: Node, visitor: &mut EncodeVisitor<'_>, fast: Option<&StoreFile<'_>>) -> Node {
    // PERF: (apiperf2) a node of the store file reads its record once.
    let in_file = fast.filter(|fast| node.file_index() == fast.file);
    let (kind, loc, flags) = match in_file {
        Some(fast) => fast.nodes.header(node),
        None => (node.kind(), node.loc(), node.flags()),
    };
    let data_node = in_file
        .filter(|_| walks_node_data(kind))
        .map(|fast| fast.nodes.data(node));
    let st = &mut visitor.ctx;
    st.node_count += 1;
    st.node_table.push(node);
    if st.prev_index != 0 {
        // this is the next sibling of `prevNode`
        let (b0, b1, b2, b3) = (
            st.node_count as u8,
            (st.node_count >> 8) as u8,
            (st.node_count >> 16) as u8,
            (st.node_count >> 24) as u8,
        );
        let base = st.prev_index as usize * NODE_SIZE + NODE_OFFSET_NEXT;
        st.nodes[base + 0] = b0;
        st.nodes[base + 1] = b1;
        st.nodes[base + 2] = b2;
        st.nodes[base + 3] = b3;
    }

    let (node_pos, node_end) = (loc.pos(), loc.end());
    let pos = st.utf16(node_pos);
    let end = st.utf16(node_end);
    let parent_index = st.parent_index;
    let data_word = match data_node {
        Some(data) => store_node_data_word(node, kind, data, node_pos, node_end, st),
        None => get_node_data(
            node,
            &mut st.strs,
            &st.position_map,
            &mut st.extended_data,
            &mut st.structured_data,
            None,
        ),
    };
    append_node(
        &mut st.nodes,
        [kind as u32, pos, end, 0, parent_index, data_word, flags.0],
    );

    let node_count = st.node_count;
    if let Some(map) = &mut st.node_index_map
        && map.contains_key(&node)
    {
        map.insert(node, node_count);
    }

    let save_parent_index = st.parent_index;

    let current_index = st.node_count;
    st.prev_index = 0;
    st.parent_index = current_index;
    match (data_node, fast) {
        (Some(data), Some(fast)) => {
            let mask = encode_store_children(kind, data, visitor, fast);
            if data_word & NODE_DATA_TYPE_MASK == NODE_DATA_TYPE_CHILDREN {
                // The low byte of the data word (`NODE_DATA_CHILD_MASK`).
                visitor.ctx.nodes[current_index as usize * NODE_SIZE + NODE_OFFSET_DATA] |= mask;
            }
        }
        _ => {
            visitor.visit_each_child(node);
        }
    }
    let source_file = visitor.ctx.source_file;
    // PERF: (apiperf2) `Node::js_doc` is nil without the parser flag
    // `HAS_JS_DOC`, which `flags` has.
    if source_file.is_some() && flags.intersects(NodeFlags::HAS_JS_DOC) {
        for jsdoc in node.js_doc(source_file) {
            encode_visit(jsdoc, visitor, fast);
        }
    }
    visitor.ctx.prev_index = current_index;
    visitor.ctx.parent_index = save_parent_index;
    node
}

/// Go `getNodeData(node, ...)` of `node` (kind `kind`, Go `Pos` and `End`
/// `pos` and `end`), read from its node data `data`, with no children mask:
/// `encode_store_children` gives it.
fn store_node_data_word(
    node: Node,
    kind: SyntaxKind,
    data: &NodeData,
    pos: i32,
    end: i32,
    st: &mut EncodeTreeState,
) -> u32 {
    let t = node_data_type_of_kind(kind);
    let common = match data {
        // Go `getNodeCommonData`: `n.MultiLine`, read from the node data.
        NodeData::Block(d) => u32::from(d.multi_line) << 24,
        NodeData::ArrayLiteralExpression(d) => u32::from(d.multi_line) << 24,
        NodeData::ObjectLiteralExpression(d) => u32::from(d.multi_line) << 24,
        _ => node_common_data_of_kind(node, kind),
    };
    match t {
        NODE_DATA_TYPE_CHILDREN => t | common,
        // Go `recordNodeStrings`.
        NODE_DATA_TYPE_STRING => {
            t | common
                | match data {
                    // Go `node.AsIdentifier().Text`: the name of the slot
                    // (`Node::text`).
                    NodeData::Identifier(_) | NodeData::PrivateIdentifier(_) => {
                        let text = frozen_store_text_name(node)
                            .map_or_else(|| node.text(), |name| name.as_str());
                        st.strs.add(text, kind, pos, end)
                    }
                    NodeData::JsxText(d) => st.strs.add(&d.text, kind, pos, end),
                    _ => record_node_strings(node, &mut st.strs),
                }
        }
        // Go `recordExtendedData`: the text and flags of a literal, and the
        // raw text of a template literal part.
        _ => {
            let (text, raw_text, flags) = match data {
                NodeData::StringLiteral(d) => (d.text.as_str(), None, d.token_flags.0),
                NodeData::NumericLiteral(d) => (d.text.as_str(), None, d.token_flags.0),
                NodeData::BigIntLiteral(d) => (d.text.as_str(), None, d.token_flags.0),
                NodeData::RegularExpressionLiteral(d) => (d.text.as_str(), None, d.token_flags.0),
                NodeData::NoSubstitutionTemplateLiteral(d) => {
                    (d.text.as_str(), None, d.template_flags.0)
                }
                NodeData::TemplateHead(d) => (
                    d.text.as_str(),
                    Some(d.raw_text.as_str()),
                    d.template_flags.0,
                ),
                NodeData::TemplateMiddle(d) => (
                    d.text.as_str(),
                    Some(d.raw_text.as_str()),
                    d.template_flags.0,
                ),
                NodeData::TemplateTail(d) => (
                    d.text.as_str(),
                    Some(d.raw_text.as_str()),
                    d.template_flags.0,
                ),
                _ => {
                    return t
                        | common
                        | record_extended_data(
                            node,
                            &mut st.strs,
                            &st.position_map,
                            &mut st.extended_data,
                            &mut st.structured_data,
                            None,
                        );
                }
            };
            let offset = st.extended_data.len() as u32;
            let text_index = st.strs.add(text, kind, pos, end);
            match raw_text {
                Some(raw_text) => {
                    let raw_text_index = st.strs.add(raw_text, kind, pos, end);
                    append_uint32s(&mut st.extended_data, &[text_index, raw_text_index, flags]);
                }
                None => append_uint32s(&mut st.extended_data, &[text_index, flags]),
            }
            t | common | offset
        }
    }
}

/// Go `visitor.VisitEachChild(node)` in `encodeTree`, for any node: from its
/// node data when it has some (`store_node_data`), else with the visitor.
fn encode_each_child(node: Node, visitor: &mut EncodeVisitor<'_>, fast: Option<&StoreFile<'_>>) {
    let kind = node.kind();
    match (store_node_data(node, kind, fast), fast) {
        (Some(data), Some(fast)) => {
            encode_store_children(kind, data, visitor, fast);
        }
        _ => {
            visitor.visit_each_child(node);
        }
    }
}

/// Go `visitor.VisitEachChild(node)` in `encodeTree` for a node of kind
/// `kind` of store file `fast` with node data `data`: the fields of
/// `for_each_child_field`, each visited as Go `VisitEachChild` visits it
/// with the encoder's hooks. Returns Go `getChildrenPropertyMask(node)`: a
/// bit per field, in field order, for a field that is not Go `nil` (a
/// modifier list that is not empty, Go `hasModifiers`). `FullSignature` has
/// no bit.
fn encode_store_children(
    kind: SyntaxKind,
    data: &NodeData,
    visitor: &mut EncodeVisitor<'_>,
    fast: &StoreFile<'_>,
) -> u8 {
    let mut mask: u8 = 0;
    let mut bit: u32 = 0;
    // Go shifts a byte: a bit from 8 on is lost.
    let mut field = |present: bool| {
        if present && bit < 8 {
            mask |= 1 << bit;
        }
        bit += 1;
    };
    for_each_child_field(kind, fast.file, data, |child| match child {
        // Go `v.visitNode(child)`.
        ChildField::Node(child) => {
            field(child.is_some());
            if child.is_some() {
                encode_visit(child, visitor, Some(fast));
            }
        }
        ChildField::FullSignature(child) => {
            if child.is_some() {
                encode_visit(child, visitor, Some(fast));
            }
        }
        // Go `v.visitNodes(list)`: the VisitNodes hook.
        ChildField::List(list) => {
            field(list.is_some());
            if let Some(list) = list {
                encode_store_list(list, visitor, fast);
            }
        }
        // Go `v.visitModifiers(modifiers)`: the VisitModifiers hook.
        ChildField::Modifiers(modifiers) => {
            let modifiers = modifiers.filter(|modifiers| !modifiers.list.nodes.is_empty());
            field(modifiers.is_some());
            if let Some(modifiers) = modifiers {
                encode_store_list(&modifiers.list, visitor, fast);
            }
        }
        ChildField::Ids(_) => unreachable!("a SyntaxList or a JSDoc node takes the visitor"),
    });
    mask
}

// Go: api/encoder/encoder.go:503 (the VisitNodes hook of encodeTree)
fn encode_visit_nodes(
    node_list: NodeList,
    visitor: &mut EncodeVisitor<'_>,
    fast: Option<&StoreFile<'_>>,
) -> NodeList {
    if node_list.is_nil() {
        return node_list;
    }

    let nodes = node_list.nodes();
    let current_index = begin_node_list(
        &mut visitor.ctx,
        node_list.pos(),
        node_list.end(),
        nodes.len(),
        node_list.has_trailing_comma(),
    );
    let save_parent_index = visitor.ctx.parent_index;
    visitor.ctx.prev_index = 0;
    visitor.ctx.parent_index = current_index;
    // PERF: (apiperf1) Go `VisitSlice` over the list in place
    // (`visit_slice_changed`). The visit returns each node as it is, so
    // nothing is copied.
    let _ = visitor.visit_slice_changed(nodes.iter());
    visitor.ctx.prev_index = current_index;
    visitor.ctx.parent_index = save_parent_index;

    node_list
}

/// `encode_visit_nodes` for a non-nil list of the node data of store file
/// `fast`.
fn encode_store_list(
    list: &crate::astdata::NodeList,
    visitor: &mut EncodeVisitor<'_>,
    fast: &StoreFile<'_>,
) {
    let file = fast.file;
    // Go `NodeList.Loc` and `HasTrailingComma` (the last node ends before
    // the list).
    let (pos, end) = (list.range.start.get() as i32, list.range.end.get() as i32);
    let has_trailing_comma = list
        .nodes
        .last()
        .is_some_and(|&last| Node::new(file, last).end() < end);
    let current_index = begin_node_list(
        &mut visitor.ctx,
        pos,
        end,
        list.nodes.len(),
        has_trailing_comma,
    );
    let save_parent_index = visitor.ctx.parent_index;
    visitor.ctx.prev_index = 0;
    visitor.ctx.parent_index = current_index;
    // Go `visitor.VisitSlice(nodeList.Nodes)`.
    for &id in &list.nodes {
        encode_visit(Node::new(file, id), visitor, Some(fast));
    }
    visitor.ctx.prev_index = current_index;
    visitor.ctx.parent_index = save_parent_index;
}

/// The start of the VisitNodes hook of Go `encodeTree`: the entry of a
/// list at `pos`..`end` with `len` nodes. Returns its index.
fn begin_node_list(
    st: &mut EncodeTreeState,
    pos: i32,
    end: i32,
    len: usize,
    has_trailing_comma: bool,
) -> u32 {
    st.node_count += 1;
    st.node_table.push(Node::NIL); // NodeLists are not *ast.Node
    if st.prev_index != 0 {
        // this is the next sibling of `prevNode`
        let (b0, b1, b2, b3) = (
            st.node_count as u8,
            (st.node_count >> 8) as u8,
            (st.node_count >> 16) as u8,
            (st.node_count >> 24) as u8,
        );
        let base = st.prev_index as usize * NODE_SIZE + NODE_OFFSET_NEXT;
        st.nodes[base + 0] = b0;
        st.nodes[base + 1] = b1;
        st.nodes[base + 2] = b2;
        st.nodes[base + 3] = b3;
    }

    let values = [
        SYNTAX_KIND_NODE_LIST,
        st.utf16(pos),
        st.utf16(end),
        0,
        st.parent_index,
        len as u32,
        // ts#63957
        u32::from(bool_to_byte(has_trailing_comma)),
    ];
    append_uint32s(&mut st.nodes, &values);
    st.node_count
}

// Go: api/encoder/encoder.go:655 appendUint32s
pub fn append_uint32s(buf: &mut Vec<u8>, values: &[u32]) {
    for &value in values {
        buf.extend_from_slice(&value.to_le_bytes());
    }
}

/// Go `appendUint32s(nodes, ...)` of the 7 fields of one node entry.
// PERF: (apiperf2) one 28-byte append, not 7 of 4 bytes.
#[inline]
fn append_node(buf: &mut Vec<u8>, values: [u32; 7]) {
    let mut bytes = [0u8; NODE_SIZE];
    for (chunk, value) in bytes.chunks_exact_mut(4).zip(values) {
        chunk.copy_from_slice(&value.to_le_bytes());
    }
    buf.extend_from_slice(&bytes);
}

/// Go `binary.LittleEndian.PutUint32(buf[offset:], value)`.
fn put_uint32(buf: &mut [u8], offset: usize, value: u32) {
    buf[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

// Go: api/encoder/encoder.go:662 getNodeData
// PORT: `given` is the record that `encode_tree` was given for a SourceFile
// `node` (see `source_file_fields`), else None.
pub fn get_node_data(
    node: Node,
    strs: &mut StringTable,
    position_map: &PositionMap,
    extended_data: &mut Vec<u8>,
    structured_data: &mut Vec<u8>,
    given: Option<&Rc<ParsedSourceFile>>,
) -> u32 {
    let t = get_node_data_type(node);
    match t {
        NODE_DATA_TYPE_CHILDREN => {
            t | get_node_common_data(node) | get_children_property_mask(node) as u32
        }
        NODE_DATA_TYPE_STRING => t | get_node_common_data(node) | record_node_strings(node, strs),
        NODE_DATA_TYPE_EXTENDED_DATA => {
            t | get_node_common_data(node)
                | record_extended_data(
                    node,
                    strs,
                    position_map,
                    extended_data,
                    structured_data,
                    given,
                )
        }
        _ => panic!("unreachable"),
    }
}

// Go: api/encoder/encoder.go:676 noStructuredData
const NO_STRUCTURED_DATA: u32 = 0xFFFFFFFF;

// Go: api/encoder/encoder.go:678 recordExtendedData_SourceFile
// PORT: `given` as in `get_node_data`.
pub fn record_extended_data_source_file(
    node: Node,
    strs: &mut StringTable,
    position_map: &PositionMap,
    extended_data: &mut Vec<u8>,
    structured_data: &mut Vec<u8>,
    given: Option<&Rc<ParsedSourceFile>>,
) {
    let sf = node;
    let fields = source_file_fields(sf, given);
    let text_index = strs.add(&source_file_text(sf), sf.kind(), sf.pos(), sf.end());
    let original_text = source_file_original_text(sf);
    let mut original_text_index = text_index;
    if original_text != source_file_text(sf) {
        original_text_index = strs.add(&original_text, SyntaxKind::Unknown, 0, 0);
    }
    let file_name_index = strs.add(source_file_file_name(sf), SyntaxKind::Unknown, 0, 0);
    let path_index = strs.add(fields.path(), SyntaxKind::Unknown, 0, 0);
    let referenced_files_offset =
        encode_file_references(fields.referenced_files(), position_map, structured_data);
    let type_ref_directives_offset = encode_file_references(
        fields.type_reference_directives(),
        position_map,
        structured_data,
    );
    let lib_ref_directives_offset = encode_file_references(
        fields.lib_reference_directives(),
        position_map,
        structured_data,
    );
    // PORT: Go computes the position map of the original text for each of
    // `encodeSpanMap` and `encodeDiagnosticDirectives`. The port computes it
    // once, and only when a span map or a directive is encoded. The output
    // is the same.
    let original_positions_cell: OnceCell<PositionMap> = OnceCell::new();
    let original_positions =
        || original_positions_cell.get_or_init(|| compute_position_map(&original_text));
    let mut span_map_offset = NO_STRUCTURED_DATA;
    if let Some(span_map) = source_file_span_map(sf) {
        span_map_offset = encode_span_map(
            Some(span_map),
            position_map,
            original_positions(),
            structured_data,
        );
    }
    let supplemental_file_names: Vec<String> = source_file_supplemental_source_files(sf)
        .iter()
        .map(|file| source_file_file_name(*file).to_string())
        .collect();
    let supplemental_file_names_offset =
        encode_string_array(&supplemental_file_names, structured_data);
    let mut canonical_file_name_index = NO_STRUCTURED_DATA;
    let canonical = source_file_canonical_source_file(sf);
    if canonical.is_some() {
        canonical_file_name_index =
            strs.add(source_file_file_name(canonical), SyntaxKind::Unknown, 0, 0);
    }
    let mut content_mapper_index = NO_STRUCTURED_DATA;
    let content_mapper = source_file_content_mapper(sf);
    if !content_mapper.is_empty() {
        content_mapper_index = strs.add(content_mapper, SyntaxKind::Unknown, 0, 0);
    }
    let mut virtual_file_name_index = NO_STRUCTURED_DATA;
    let virtual_file_name = source_file_virtual_file_name(sf);
    if !virtual_file_name.is_empty() {
        virtual_file_name_index = strs.add(virtual_file_name, SyntaxKind::Unknown, 0, 0);
    }
    let diagnostic_directives = source_file_diagnostic_directives(sf);
    let mut diagnostic_directives_offset = NO_STRUCTURED_DATA;
    if !diagnostic_directives.is_empty() {
        diagnostic_directives_offset = encode_diagnostic_directives(
            diagnostic_directives,
            position_map,
            original_positions(),
            structured_data,
        );
    }
    // imports, moduleAugmentations, ambientModuleNames offsets are placeholders;
    // they will be patched after the tree walk when node indices are known.
    append_uint32s(
        extended_data,
        &[
            text_index,
            file_name_index,
            path_index,
            fields.language_variant().0 as u32,
            fields.script_kind().0 as u32,
            referenced_files_offset,
            type_ref_directives_offset,
            lib_ref_directives_offset,
            NO_STRUCTURED_DATA,
            NO_STRUCTURED_DATA,
            NO_STRUCTURED_DATA,
            0,
            original_text_index,
            span_map_offset,
            supplemental_file_names_offset,
            canonical_file_name_index,
            content_mapper_index,
            virtual_file_name_index,
            diagnostic_directives_offset,
        ],
    );
}

// Go: api/encoder/encoder.go:714 recordExtendedData_TemplateHead
pub fn record_extended_data_template_head(
    node: Node,
    strs: &mut StringTable,
    _position_map: &PositionMap,
    extended_data: &mut Vec<u8>,
    _structured_data: &mut Vec<u8>,
) {
    let n = node;
    let text_index = strs.add(n.text(), node.kind(), node.pos(), node.end());
    let raw_text_index = strs.add(n.raw_text(), node.kind(), node.pos(), node.end());
    append_uint32s(
        extended_data,
        &[text_index, raw_text_index, n.template_flags().0 as u32],
    );
}

// Go: api/encoder/encoder.go:721 recordExtendedData_TemplateMiddle
pub fn record_extended_data_template_middle(
    node: Node,
    strs: &mut StringTable,
    _position_map: &PositionMap,
    extended_data: &mut Vec<u8>,
    _structured_data: &mut Vec<u8>,
) {
    let n = node;
    let text_index = strs.add(n.text(), node.kind(), node.pos(), node.end());
    let raw_text_index = strs.add(n.raw_text(), node.kind(), node.pos(), node.end());
    append_uint32s(
        extended_data,
        &[text_index, raw_text_index, n.template_flags().0 as u32],
    );
}

// Go: api/encoder/encoder.go:728 recordExtendedData_TemplateTail
pub fn record_extended_data_template_tail(
    node: Node,
    strs: &mut StringTable,
    _position_map: &PositionMap,
    extended_data: &mut Vec<u8>,
    _structured_data: &mut Vec<u8>,
) {
    let n = node;
    let text_index = strs.add(n.text(), node.kind(), node.pos(), node.end());
    let raw_text_index = strs.add(n.raw_text(), node.kind(), node.pos(), node.end());
    append_uint32s(
        extended_data,
        &[text_index, raw_text_index, n.template_flags().0 as u32],
    );
}

// Go: api/encoder/encoder.go:735 boolToByte
pub fn bool_to_byte(b: bool) -> u8 {
    if b {
        return 1;
    }
    0
}

// Go: api/encoder/encoder.go:743 hasModifiers
/// hasModifiers returns true if the modifier list is non-nil and has at least one modifier.
pub fn has_modifiers(modifiers: ModifierList) -> bool {
    modifiers.is_some() && !modifiers.nodes().is_empty()
}

// Go: api/encoder/encoder.go:750 encodeFileReferences
/// encodeFileReferences encodes a slice of FileReferences as a msgpack array of tuples
/// into the structured data buffer. Returns the byte offset into the buffer, or
/// noStructuredData (0xFFFFFFFF) if the slice is empty.
fn encode_file_references(
    refs: &[FileReference],
    position_map: &PositionMap,
    buf: &mut Vec<u8>,
) -> u32 {
    if refs.is_empty() {
        return NO_STRUCTURED_DATA;
    }
    let offset = buf.len() as u32;
    msgpack_write_array_header(buf, refs.len());
    for r in refs {
        // Each entry is a 5-element tuple: [pos, end, fileName, resolutionMode, preserve]
        msgpack_write_array_header(buf, 5);
        msgpack_write_uint(buf, position_map.utf8_to_utf16(r.range.pos()) as u32);
        msgpack_write_uint(buf, position_map.utf8_to_utf16(r.range.end()) as u32);
        msgpack_write_string(buf, &r.file_name);
        msgpack_write_uint(buf, r.resolution_mode.0 as u32);
        msgpack_write_bool(buf, r.preserve);
    }
    offset
}

// Go: api/encoder/encoder.go:771 encodeNodeIndexArray
/// encodeNodeIndexArray encodes a slice of LiteralLikeNodes as a msgpack array of
/// uint node indices. Returns the byte offset into the buffer, or noStructuredData
/// if the slice is empty.
// PORT: a nil Go map is `None`; Go reads 0 from it.
fn encode_node_index_array(
    nodes: &[Node],
    index_map: Option<&FxHashMap<Node, u32>>,
    buf: &mut Vec<u8>,
) -> u32 {
    if nodes.is_empty() {
        return NO_STRUCTURED_DATA;
    }
    let offset = buf.len() as u32;
    msgpack_write_array_header(buf, nodes.len());
    for &node in nodes {
        msgpack_write_uint(
            buf,
            index_map
                .and_then(|map| map.get(&node).copied())
                .unwrap_or(0),
        );
    }
    offset
}

// Go: api/encoder/encoder.go:786 encodeModuleAugmentations
/// encodeModuleAugmentations encodes a slice of ModuleName nodes as a msgpack array
/// of uint node indices. Returns the byte offset into the buffer, or noStructuredData
/// if the slice is empty.
// PORT: a nil Go map is `None`; Go reads 0 from it.
fn encode_module_augmentations(
    nodes: &[Node],
    index_map: Option<&FxHashMap<Node, u32>>,
    buf: &mut Vec<u8>,
) -> u32 {
    if nodes.is_empty() {
        return NO_STRUCTURED_DATA;
    }
    let offset = buf.len() as u32;
    msgpack_write_array_header(buf, nodes.len());
    for &node in nodes {
        msgpack_write_uint(
            buf,
            index_map
                .and_then(|map| map.get(&node).copied())
                .unwrap_or(0),
        );
    }
    offset
}

// Go: api/encoder/encoder.go:800 encodeStringArray
/// encodeStringArray encodes a slice of strings as a msgpack array of strings.
/// Returns the byte offset into the buffer, or noStructuredData if the slice is empty.
fn encode_string_array(strs: &[String], buf: &mut Vec<u8>) -> u32 {
    if strs.is_empty() {
        return NO_STRUCTURED_DATA;
    }
    let offset = buf.len() as u32;
    msgpack_write_array_header(buf, strs.len());
    for s in strs {
        msgpack_write_string(buf, s);
    }
    offset
}

// Go: api/encoder/encoder.go:812 encodeSpanMap
fn encode_span_map(
    m: Option<&spanmap::SpanMap>,
    virtual_positions: &PositionMap,
    original_positions: &PositionMap,
    buf: &mut Vec<u8>,
) -> u32 {
    if m.is_none() {
        return NO_STRUCTURED_DATA;
    }
    let segments = spanmap::SpanMap::segments(m);
    let offset = buf.len() as u32;
    msgpack_write_array_header(buf, segments.len());
    for segment in &segments {
        let mut tuple_length = 5;
        if segment.features != spanmap::Feature::ALL {
            tuple_length = 6;
        }
        msgpack_write_array_header(buf, tuple_length);
        let virtual_start = virtual_positions.utf8_to_utf16(segment.virtual_start);
        let virtual_end = virtual_positions.utf8_to_utf16(segment.virtual_end);
        let original_start = original_positions.utf8_to_utf16(segment.original_start);
        let original_end = original_positions.utf8_to_utf16(segment.original_end);
        msgpack_write_uint(buf, virtual_start as u32);
        msgpack_write_uint(buf, (virtual_end - virtual_start) as u32);
        msgpack_write_uint(buf, original_start as u32);
        msgpack_write_uint(buf, (original_end - original_start) as u32);
        msgpack_write_uint(buf, segment.kind.0 as u32);
        if tuple_length == 6 {
            msgpack_write_uint(buf, segment.features.0 as u32);
        }
    }
    offset
}

// Go: api/encoder/encoder.go:841 encodeDiagnosticDirectives
fn encode_diagnostic_directives(
    directives: &[MappedDiagnosticDirective],
    virtual_positions: &PositionMap,
    original_positions: &PositionMap,
    buf: &mut Vec<u8>,
) -> u32 {
    if directives.is_empty() {
        return NO_STRUCTURED_DATA;
    }
    let offset = buf.len() as u32;
    msgpack_write_array_header(buf, directives.len());
    for directive in directives {
        msgpack_write_array_header(buf, 6);
        let original_start = original_positions.utf8_to_utf16(directive.original_range.pos());
        let original_end = original_positions.utf8_to_utf16(directive.original_range.end());
        let virtual_start = virtual_positions.utf8_to_utf16(directive.virtual_range.pos());
        let virtual_end = virtual_positions.utf8_to_utf16(directive.virtual_range.end());
        msgpack_write_uint(buf, original_start as u32);
        msgpack_write_uint(buf, (original_end - original_start) as u32);
        msgpack_write_uint(buf, virtual_start as u32);
        msgpack_write_uint(buf, (virtual_end - virtual_start) as u32);
        msgpack_write_uint(buf, u32::from(directive.policy.0));
        msgpack_write_uint(buf, directive.unused_code as u32);
    }
    offset
}

// Minimal msgpack writers for the structured data section.

// Go: api/encoder/encoder.go:865 msgpackWriteArrayHeader
fn msgpack_write_array_header(buf: &mut Vec<u8>, length: usize) {
    if length <= 0x0f {
        buf.push(0x90 | length as u8);
        return;
    }
    if length <= 0xffff {
        buf.extend_from_slice(&[0xdc, (length >> 8) as u8, length as u8]);
        return;
    }
    buf.extend_from_slice(&[
        0xdd,
        (length >> 24) as u8,
        (length >> 16) as u8,
        (length >> 8) as u8,
        length as u8,
    ]);
}

// Go: api/encoder/encoder.go:875 msgpackWriteUint
fn msgpack_write_uint(buf: &mut Vec<u8>, value: u32) {
    if value <= 0x7f {
        buf.push(value as u8);
        return;
    }
    if value <= 0xff {
        buf.extend_from_slice(&[0xcc, value as u8]);
        return;
    }
    if value <= 0xffff {
        buf.extend_from_slice(&[0xcd, (value >> 8) as u8, value as u8]);
        return;
    }
    buf.extend_from_slice(&[
        0xce,
        (value >> 24) as u8,
        (value >> 16) as u8,
        (value >> 8) as u8,
        value as u8,
    ]);
}

// Go: api/encoder/encoder.go:888 msgpackWriteString
fn msgpack_write_string(buf: &mut Vec<u8>, s: &str) {
    let n = s.len();
    if n <= 0x1f {
        buf.push(0xa0 | n as u8);
    } else if n <= 0xff {
        buf.extend_from_slice(&[0xd9, n as u8]);
    } else if n <= 0xffff {
        buf.extend_from_slice(&[0xda, (n >> 8) as u8, n as u8]);
    } else {
        buf.extend_from_slice(&[
            0xdb,
            (n >> 24) as u8,
            (n >> 16) as u8,
            (n >> 8) as u8,
            n as u8,
        ]);
    }
    buf.extend_from_slice(s.as_bytes());
}

// Go: api/encoder/encoder.go:902 msgpackWriteBool
fn msgpack_write_bool(buf: &mut Vec<u8>, value: bool) {
    if value {
        buf.push(0xc3);
        return;
    }
    buf.push(0xc2);
}

// Hand-written commonData encoding functions for nodes whose non-bool data
// members cannot be automatically encoded by the generator. Each function
// packs relevant fields into the 6-bit commonData area (bits 24-29) of the
// 32-bit node data word.

// Go: api/encoder/encoder.go:914 getNodeCommonData_SyntheticExpression
pub fn get_node_common_data_synthetic_expression(_node: Node) -> u32 {
    // SyntheticExpression is an internal compiler node that is never part of a parsed AST.
    // It should never be encoded.
    panic!("SyntheticExpression should never be encoded")
}

// Hand-written extended data encoding functions for literal nodes that were
// previously string-type but whose TokenFlags/TemplateFlags cannot fit in 6 bits.

// Go: api/encoder/encoder.go:923 recordExtendedData_StringLiteral
pub fn record_extended_data_string_literal(
    node: Node,
    strs: &mut StringTable,
    _position_map: &PositionMap,
    extended_data: &mut Vec<u8>,
    _structured_data: &mut Vec<u8>,
) {
    let n = node;
    let text_index = strs.add(n.text(), node.kind(), node.pos(), node.end());
    append_uint32s(extended_data, &[text_index, n.token_flags().0 as u32]);
}

// Go: api/encoder/encoder.go:929 recordExtendedData_NumericLiteral
pub fn record_extended_data_numeric_literal(
    node: Node,
    strs: &mut StringTable,
    _position_map: &PositionMap,
    extended_data: &mut Vec<u8>,
    _structured_data: &mut Vec<u8>,
) {
    let n = node;
    let text_index = strs.add(n.text(), node.kind(), node.pos(), node.end());
    append_uint32s(extended_data, &[text_index, n.token_flags().0 as u32]);
}

// Go: api/encoder/encoder.go:935 recordExtendedData_BigIntLiteral
pub fn record_extended_data_big_int_literal(
    node: Node,
    strs: &mut StringTable,
    _position_map: &PositionMap,
    extended_data: &mut Vec<u8>,
    _structured_data: &mut Vec<u8>,
) {
    let n = node;
    let text_index = strs.add(n.text(), node.kind(), node.pos(), node.end());
    append_uint32s(extended_data, &[text_index, n.token_flags().0 as u32]);
}

// Go: api/encoder/encoder.go:941 recordExtendedData_RegularExpressionLiteral
pub fn record_extended_data_regular_expression_literal(
    node: Node,
    strs: &mut StringTable,
    _position_map: &PositionMap,
    extended_data: &mut Vec<u8>,
    _structured_data: &mut Vec<u8>,
) {
    let n = node;
    let text_index = strs.add(n.text(), node.kind(), node.pos(), node.end());
    append_uint32s(extended_data, &[text_index, n.token_flags().0 as u32]);
}

// Go: api/encoder/encoder.go:947 recordExtendedData_NoSubstitutionTemplateLiteral
pub fn record_extended_data_no_substitution_template_literal(
    node: Node,
    strs: &mut StringTable,
    _position_map: &PositionMap,
    extended_data: &mut Vec<u8>,
    _structured_data: &mut Vec<u8>,
) {
    let n = node;
    let text_index = strs.add(n.text(), node.kind(), node.pos(), node.end());
    append_uint32s(extended_data, &[text_index, n.template_flags().0 as u32]);
}

#[cfg(test)]
mod store_data_tests {
    use super::*;
    use crate::ast::{FileText, FileVersion};
    use crate::frontend::parser::{SourceFileParseOptions, parse_source_file};

    /// Texts with every syntax kind that a parse makes outside JSDoc.
    const SAMPLES: &[(&str, &str)] = &[
        (
            "a.ts",
            r#"import def, { a as b, type c } from "./m";
import * as ns from "./n";
import x = require("x");
import type { T } from "./t" with { type: "json" };
export { b as default2, c };
export * from "./star";
export * as nsx from "./nsx";
export default class<T extends object = {}> extends Base<T> implements I, J {
    static #p?: number = 1;
    declare readonly q!: string;
    @dec() @dec2 method<U>(this: X, a?: U, ...rest: U[]): asserts a is U { super.m(); }
    get g(): number { return this.#p ?? 0; }
    set g(v) {}
    constructor(private readonly z: number, public w?: string) { super(); }
    static { label: for (;;) { break label; } }
    [key: string]: any;
    m2?(): void;
    *gen() { yield* other(); yield 1; }
    async am() { await p; for await (const v of it) {} }
    "quoted"() {}
    [computed]: number;
    123: number;
}
abstract class A { abstract m(): void; protected abstract get p(): number; }
function f(overload: string): void;
function f(overload: number): void;
function f(overload: any) {}
declare function df(): void;
declare module "mod" { export const x: number; }
declare global { interface Window { w: number } }
namespace N.M { export namespace O {} }
module Old {}
enum E { A = 1, B = A << 2, "C" }
const enum CE { X }
type U<T> = T extends (infer V extends string)[] ? V : never;
type M = { readonly [K in keyof T as `get${K & string}`]-?: T[K] };
type F = new (...a: any[]) => unknown;
type F2 = abstract new () => void;
type Tup = [a: string, b?: number, ...c: boolean[]];
type L = "lit" | 1 | -1 | true | null | undefined | 1n | `t${string}x${number}`;
type Q = typeof import("./q", { with: { "resolution-mode": "import" } }).Z<number>;
type Idx = T["k"][number];
type Pred = (x: unknown) => x is string;
type Op = keyof T | unique symbol | readonly string[];
type Paren = (string | number)[];
type Opt = { a?: number; b(): void; new (): X; (): Y; get c(): number; set c(v: number) };
type Q2 = typeof x.y<string>;
interface I extends J, K<number> { [i: number]: string; m?<T>(): void }
let v1: any = <any>x, v2 = x as const, v3 = x satisfies T, v4 = x!;
var { a1, b1: [c1, , ...d1] = [], ...e1 } = obj, [f1 = 1] = arr;
const o = { a, b: 1, [c]: 2, ...d, m() {}, get g() { return 1; }, set s(v) {}, async *ag() {}, "s": 1, 2: 3 };
const arrow = async <T,>(a: T): Promise<T> => a, arrow2 = x => ({ x });
const fe = function* named() {}, ce = class Named {};
const t = tag<string>`a${b}c${d}e`, nst = `plain`, re = /ab+c/gi, big = 123n, num = 0x1F, str = 'q\'s';
if (a) b; else if (c) { d; } else e;
do x++; while (--y);
while (true) continue;
for (let i = 0, j; i < 10; i++) {}
for (const k in o) {}
for (const [k, v] of m) {}
switch (s) { case 1: case 2: break; default: throw new Error(`e`); }
try { a(); } catch { } finally { }
try { a(); } catch (e: unknown) { }
with (o) { }
debugger;
;
x = a ? b : c, y ||= z, w ??= v, u **= 2;
delete o.p; void 0; typeof x; ++x; x--; -x; ~x; !x; +x;
new Foo<number>(1)?.bar?.[0]?.(2);
new.target; import.meta.url; import("dyn"); super.x;
a?.b!.c;
label2: { }
using res = getRes();
export = something;
"#,
        ),
        (
            "b.tsx",
            r#"const el = <div className="a" {...props} key={1} data-x='y' ns:attr="z" disabled>
    text {expr} {/* comment */} {...spread}
    <Child<number> a={<b />} />
    <></>
    <ns:tag />
    <a.b.c>more</a.b.c>
</div>;
const g = <T,>(x: T) => x;
"#,
        ),
        (
            "c.js",
            r#"/** @type {number} */
var n = 1;
/**
 * @param {string} a desc
 * @param {{ x: number }} [b]
 * @returns {Promise<void>}
 * @template T
 * @typedef {Object} Td
 * @property {string} p
 * @callback Cb
 * @see {@link Foo} and {@linkcode Bar} {@linkplain Baz}
 * @deprecated
 */
function jsf(a, b) { return a; }
/** @enum {string} */
const En = { A: "a" };
class JC { /** @private */ p = 1; #q; static s; }
module.exports = { jsf };
exports.x = 1;
"#,
        ),
        (
            "d.d.ts",
            "declare const x: number;\nexport declare function f(): void;\nexport as namespace NS;\n",
        ),
        (
            "e.ts",
            "let s = '\\ud800'; let id\\u0061 = 1; let \u{e9} = 'é'; // non-ASCII\nconst bad = `\\x`;\n",
        ),
        ("f.ts", "class C { m( { } \nlet = ; }} ) => ;\nfunction (\n"),
        // followups32: fields that the texts above leave out. A JS
        // function's `@type` gives its `FullSignature`, which has no mask
        // bit (parser/reparser.go:396), and an assignment declaration's
        // `@type` gives its BinaryExpression a `Type` (:377).
        (
            "g.js",
            r#"/** @type {(a: string, b?: number) => void} */
function typed(a, b) {}
/** @type {number} */
exports.y = 1;
/** @type {string} */
module.exports.z = "z";
"#,
        ),
        // A shorthand property with an initializer in a destructuring
        // assignment, a definite assignment `!`, optional tuple members and
        // two MissingDeclarations: decorators with no declaration
        // (parser/parser.go:1188) and a decorated expression (:5790).
        (
            "h.ts",
            "({ a = 1, b: [c] = [] } = o);\nlet x!: number;\ntype OptTup = [string?, number?];\n@dec;\nconst md = @dec 1;\n",
        ),
    ];

    /// Encodes `text` as `file_name` with the node data walk and with the
    /// visitor only, and asserts equal bytes. `freeable`: a freeable file
    /// version with owned nodes (an API source file lease), else a static
    /// publish. Both are published and bound, as the parse cache of the API
    /// does (`project::acquire_bound`).
    fn assert_same_encoding(file_name: &str, text: &str, freeable: bool) {
        let options = SourceFileParseOptions {
            file_name: file_name.to_string(),
            path: tspath::Path(file_name.to_string()),
            ..Default::default()
        };
        let script_kind = crate::frontend::core_ext::ensure_script_kind_from_file_name(file_name);
        let file = {
            let _scope = freeable.then(crate::ast::store::enter_owned_parse);
            parse_source_file(
                &options,
                FileText::new(text.to_string(), freeable),
                script_kind,
            )
        };
        if freeable {
            assert!(file.version.set(FileVersion::new(file.store)).is_ok());
        }
        let file = Rc::new(file);
        crate::program::note_parsed_source_file(&file);
        crate::program::publish_parsed_files("/");
        crate::program::bind_file_outside_program(file.root);
        match FileNodes::of(file.root.file_index()) {
            Some(FileNodes::Pinned(_)) => assert!(freeable, "{file_name}: a freeable version"),
            Some(FileNodes::Static(_)) => assert!(!freeable, "{file_name}: a static publish"),
            None => panic!("{file_name}: the file is not published"),
        }
        let (walk, _) = encode_parsed_source_file(&file).expect("encode");
        VISITOR_ONLY.with(|v| v.set(true));
        let (visitor, _) = encode_parsed_source_file(&file).expect("encode");
        VISITOR_ONLY.with(|v| v.set(false));
        assert!(
            walk == visitor,
            "{file_name}: the node data walk and the visitor differ"
        );
        drop(file);
        // The pins of this thread hold a freeable version until here.
        crate::ast::release_file_version_pins();
    }

    #[test]
    fn the_node_data_walk_encodes_as_the_visitor() {
        for (i, (name, text)) in SAMPLES.iter().enumerate() {
            assert_same_encoding(&format!("/apiperf2/static/{i}/{name}"), text, false);
            assert_same_encoding(&format!("/apiperf2/freeable/{i}/{name}"), text, true);
        }
        // Not run by the test suite: set GOPORT_ENCODE_CORPUS to directories
        // (`:` between them) to compare every .ts, .tsx, .js, .jsx, .mts and
        // .cts file under them, outside node_modules.
        let Ok(dirs) = std::env::var("GOPORT_ENCODE_CORPUS") else {
            return;
        };
        let mut stack: Vec<std::path::PathBuf> =
            dirs.split(':').map(std::path::PathBuf::from).collect();
        let mut count = 0;
        while let Some(dir) = stack.pop() {
            let mut entries: Vec<_> = std::fs::read_dir(&dir)
                .expect("read the corpus directory")
                .map(|e| e.expect("a corpus entry").path())
                .collect();
            entries.sort();
            for path in entries {
                if path.is_dir() {
                    if !path.ends_with("node_modules") {
                        stack.push(path);
                    }
                    continue;
                }
                let name = path.to_string_lossy().to_string();
                if ![".ts", ".tsx", ".js", ".jsx", ".mts", ".cts"]
                    .iter()
                    .any(|e| name.ends_with(e))
                {
                    continue;
                }
                let Ok(text) = std::fs::read_to_string(&path) else {
                    continue;
                };
                count += 1;
                // A static publish is never freed: only small files.
                if text.len() <= 16 * 1024 {
                    assert_same_encoding(&format!("/apiperf2/c{count}/static{name}"), &text, false);
                }
                assert_same_encoding(&format!("/apiperf2/c{count}/freeable{name}"), &text, true);
            }
        }
        eprintln!("compared {count} corpus files");
    }
}
