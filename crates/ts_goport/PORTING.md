# ts_goport porting contract

This crate is a line-by-line port of pinned typescript-go
`dc37b5249ab60e2bbce936f71b883e6c8136167e`
(`~/.explore/repos/microsoft__typescript-go/internal`). About 70 agents port
separate Go ranges at the same time. Nobody can see the other files while
writing. The rules below make every file agree on names and types without
coordination. Follow them exactly. When a rule does not cover a case, choose
the most literal port and add a `// PORT:` comment that explains the choice.

## Ownership

- You own exactly one new file (two for the program unit). Write only that file.
- Do not edit `lib.rs`, `core.rs`, `prelude.rs`, `flags*.rs`, `diag.rs`,
  any `mod.rs`, `Cargo.toml`, or any other crate.
- Do not run `cargo`. Root runs builds and sends compile errors back later.
- Start every file with `use crate::prelude::*;`. Everything in the crate is
  glob-exported through the prelude.
- Port every Go function in your range, in Go order, with the Go logic intact.
  Keep Go comments that explain behavior. Do not simplify, merge, or "improve".
  Behavior must match Go exactly, including diagnostic order and type creation
  order (type ids decide union order).
- If you cannot port something (a missing dependency such as the node builder,
  printer, or node factory), call `unported!("goFunctionName")` at that point.
  Never return a guessed value.
- Skip Go code only for: tracing, language-service-only exported APIs
  (`GetXxx` wrappers used only by `ls`/`services`), and concurrency (mutexes,
  `sync.Once` become plain code).
- Port a Go `debug.*` call (`debug.Assert`, `debug.Fail`, `debug.AssertNever`,
  `debug.FailBadSyntaxKind`) with `gostd/debug.rs` (`go_assert!` and
  `debug::*`): the check stays on in release builds and fails with Go's
  "panic: Debug failure. ..." text and exit 2. A check that only the port
  has stays `debug_assert!`. Older ports keep `debug_assert!` at some Go
  `debug.Assert` sites (47 checker, 15 ls, 3 compiler, 5 parser
  (`parser_p3.rs`, `parser_p4.rs`) and 2 class fields transform
  (`class_fields.rs`, `class_fields_p2.rs`, Go `debug.AssertNever`) sites,
  state notes `sweepN3-go-assert-2026-10-01` and
  `portgaps1-decisions-2026-10-04`).

## Names

- Go `camelCase`/`PascalCase` function or method -> Rust `snake_case` with the
  same words. `getTypeOfSymbol` -> `get_type_of_symbol`,
  `IsTypeAny` -> `is_type_any`, `GetJSDocTags` -> `get_js_doc_tags`,
  `isESSymbolLikeType` -> `is_es_symbol_like_type`.
  Rule: insert `_` before each capital that follows a lowercase letter or
  digit, and before the last capital of an acronym run followed by a
  lowercase letter; then lowercase. (`getTypeOfJSXElement` ->
  `get_type_of_jsx_element`, `ESSymbol` -> `es_symbol`.)
  Rust keywords get a trailing underscore: `type_`, `match_`, `ref_`.
- Go struct fields -> `snake_case` by the same rule.
- Exported and unexported Go names with the same snake name only collide when
  both exist in one Go type; then suffix the exported one with `_exported`.
- Go consts of flag types: `ast.SymbolFlagsValue` -> `SymbolFlags::VALUE`,
  `TypeFlagsAny` -> `TypeFlags::ANY`, `ast.NodeFlagsAmbient` ->
  `NodeFlags::AMBIENT` (all from `crate::flags`, generated from Go, same
  numeric values). Operators: `a|b` works, `a&b != 0` -> `a.intersects(b)`,
  `a&b == b` -> `a.contains(b)`, `a&^b` -> `a.without(b)`, `a&b` -> `a & b`.
  Raw bits: `.0`.
- `ast.KindFoo` -> `SyntaxKind::Foo` (`astdata::SyntaxKind`). Only difference:
  Go `JSDoc...`/`JS...` kinds are spelled `JsDoc...`/`Js...`
  (`ast.KindJSDocTypeTag` -> `SyntaxKind::JsDocTypeTag`,
  `ast.KindJSImportDeclaration` -> `SyntaxKind::JsImportDeclaration`).
  Range markers: `ast.KindFirstTypeNode` -> `SyntaxKind::FIRST_TYPE_NODE`
  (compare with `>=`/`<=`; SyntaxKind is `Ord`).
- Diagnostics: `diagnostics.Type_0_is_not_assignable_to_type_1` ->
  `diag::Type_0_is_not_assignable_to_type_1` (exact Go name,
  `&'static diagnostics::Message`).
- Go package-level functions in `checker` -> `impl Checker` methods when they
  touch type, symbol, signature, mapper or checker data, else free `pub fn`.
  Package-level functions in `ast`, `scanner`, `binder` that take a symbol
  get `symbols: &SymbolArena` as the first parameter; all other `ast`
  functions are free `pub fn`s with the Go snake name.
- Go methods on `*Checker` -> `impl Checker` methods (`&mut self`; `&self`
  only when Go clearly only reads). Go methods on other structs -> methods on
  the Rust struct.

## Types

| Go | Rust |
|---|---|
| `*ast.Node` and every alias (`*ast.Expression`, `*ast.TypeNode`, `*ast.SourceFile`, `*ast.IdentifierNode`, `*ast.Declaration`, ...) | `Node` (Copy handle, `Node::NIL` = nil) |
| `*ast.NodeList` | `NodeList` (Copy handle, `NodeList::NIL` = nil) |
| `*ast.ModifierList` | `ModifierList` (Copy handle) |
| `*ast.Symbol` | `SymbolId` (`SymbolId::NIL`) |
| `ast.SymbolTable` | `SymbolTable` (Copy handle; nil map = `SymbolTable::NIL`) |
| `*ast.FlowNode` | `FlowNodeId` |
| `*Type` | `TypeId` |
| `*Signature` | `SignatureId` |
| `*IndexInfo` | `IndexInfoId` |
| `*TypePredicate` | `TypePredicateId` |
| `*TypeMapper` | `MapperId` |
| `*InferenceContext` | `InferenceContextId` |
| `*InferenceInfo` | `usize` index into `inference_contexts[ctx].inferences` (pass the context id too) |
| `*diagnostics.Message` | `&'static Message` (`diagnostics::Message`) |
| `*ast.Diagnostic` | `Diagnostic` (owned, `core::Diagnostic`) |
| `[]*T` param | `&[T]` ; `[]*T` field or return | `Vec<T>` |
| `string` param | `&str` ; field or return | `String` |
| `int` | `i32` ; `int64` | `i64` ; `uint32` | `u32` ; `jsnum.Number` | `jsnum::Number` |
| `bool` | `bool` |
| `any` literal value (LiteralType.value) | `LiteralValue` enum defined in `checker/types.rs` |
| `core.Tristate` | `Tristate` (in `options.rs`) |
| `func(*Type) bool` param | `&mut dyn FnMut(&mut Checker, TypeId) -> bool` |
| `func(*Type) *Type` param | `&mut dyn FnMut(&mut Checker, TypeId) -> TypeId` |
| stored func fields | `Rc<dyn Fn(&mut Checker, ...) -> ...>` |
| other `*Struct` shared/mutated across calls | `Rc<RefCell<Struct>>` |
| `map[K]V` | `FxHashMap<K, V>`; use `IndexMap` when Go iterates and order matters for output |
| `collections.Set[T]` / `OrderedSet` | `FxHashSet<T>` / `IndexSet<T>` |
| `(T, bool)` / multiple returns | tuple |
| `...any` diagnostic args | `Vec<String>`, built with `args![a, b]` |

Handles are nil when zero. Go `x == nil` -> `x.is_nil()`, `x != nil` ->
`x.is_some()`. Do not use `Option<Handle>`. Pointer equality -> `==` on
handles.

Borrowing: arenas live in `Checker`. Copy what you need out of an arena entry
before calling another `&mut self` method. Clone `Vec`s you iterate while
calling `&mut self` methods. After a call, re-fetch links
(`self.value_symbol_links.get_by_id(&self.symbols, s)`) instead of holding a
reference across it.

### Go `int` past the int32 range

Go `int` is 64 bits, and the port holds it as `i32` (the table above). This
is safe for real sizes and positions. Values from a `.tsbuildinfo` are not
safe: a diagnostic `pos`, `end` or `start+length` can be near ±2^31 (the i32
wrap class). Where Go adds or compares such values as `int`, do it in `i64`
and give Go's panic text (`write_code_snippet`,
`scanner_util::panic_past_text`).

PORT limit: in a text with bytes that are not valid UTF-8, the port form is
longer than Go's bytes (marker units, `gostring::go_byte_offset`). Past the
units, a port offset is `extra` more than its Go offset. A Go offset within
`extra` of `i32::MAX` has no `i32` port offset, so it wraps to a negative
one. Go panics `slice bounds out of range [:N]`. The port panics `index out
of range [-1]`, or with `--pretty` gives the text of the negative Go `end`
that has the same port offset, or writes the negative value to a
`.tsbuildinfo`. Do not keep such an offset in `i32`: that only moves the
error to the `extra` Go offsets below it (followups25 round b, 158 runs lost
against R172). The full fix needs i64 diagnostic offsets in core.rs.

## Checker data (owned by checker_p01 and types)

`checker/types.rs` defines (Go names, snake fields):
`Type { flags: TypeFlags, object_flags: ObjectFlags, id: TypeId, symbol: SymbolId, alias: Option<Rc<TypeAlias>>, data: TypeData }`,
`enum TypeData { Intrinsic(IntrinsicType), Literal(LiteralType), UniqueESSymbol(UniqueESSymbolType), TypeParameter(TypeParameter), Index(IndexType), IndexedAccess(IndexedAccessType), TemplateLiteral(TemplateLiteralType), StringMapping(StringMappingType), Substitution(SubstitutionType), Conditional(ConditionalType), Object(ObjectType), TypeReference(TypeReference), Interface(InterfaceType), Tuple(TupleType), InstantiationExpression(InstantiationExpressionType), Mapped(MappedType), ReverseMapped(ReverseMappedType), EvolvingArray(EvolvingArrayType), Union(UnionType), Intersection(IntersectionType) }`.
Go embedding becomes nesting: `TupleType { interface: InterfaceType, .. }`,
`InterfaceType { reference: TypeReference, .. }`, `TypeReference { object: ObjectType, .. }`,
`ObjectType { structured: StructuredType, .. }`, `StructuredType { constrained: ConstrainedType, .. }`,
`UnionType { union_or_intersection: UnionOrIntersectionType, .. }`, and so on
exactly as in Go. Every Go `AsX()` accessor on `*Type` is a method on `Type`:
`as_x(&self) -> &X` and `as_x_mut(&mut self) -> &mut X`, working for every
variant that embeds `X` (panic otherwise). Every other Go method on `*Type`
(`Target()`, `Types()`, `TargetTupleType()`, `Distributive()`, ...) is a
`Type` method with the snake name, returning handles by value and slices as
`&[T]`. Same for `Signature`, `IndexInfo`, `TypePredicate`, `TypeAlias`,
`ConditionalRoot`, and all `*Links` structs (all `Default`).

`checker/checker_p01.rs` defines `pub struct Checker` with every Go
`Checker` field (snake), and these arenas and accessors:
- `symbols: SymbolArena` (starts as a clone of the program's binder
  symbols, `program::bound_symbols()`).
  Access: `self.symbols.sym(s)`, `self.symbols.sym_mut(s)`; shorthand
  methods `self.sym(s) -> &Symbol` and `self.sym_mut(s)`.
- `types: ChunkedArena<Type>` -> `self.ty(t) -> &Type`, `self.ty_mut(t) -> &mut Type`.
- `signatures: Vec<Signature>` -> `self.sig(s)`, `self.sig_mut(s)`.
- `index_infos: Vec<IndexInfo>` -> `self.index_info(i)`, `self.index_info_mut(i)`.
- `type_predicates: Vec<TypePredicate>` -> `self.pred(p)`, `self.pred_mut(p)`.
- `mappers: ChunkedArena<TypeMapper>` -> `self.mapper(m)`, `self.mapper_mut(m)`.
- `inference_contexts: ChunkedArena<InferenceContext>` -> `self.inference_context(c)`, `self.inference_context_mut(c)`.
A `ChunkedArena` (`checker/types.rs`) keeps its entries in chunks of
8,192, so it does not copy every entry when it grows, as a doubling `Vec`
does (infermem1: the `inference_contexts` `Vec` kept buffers of 512 and
256 MiB on typebox).
Each arena has a dummy entry at index 0. New entries are pushed; ids are
`TypeId(len as u32)` etc. Go `c.newType`, `c.newSignature`,
`newIndexInfo`, `newTypePredicate`, mapper constructors push into these.
Go `t.id` equals the arena index, so Go's per-checker `TypeId` counter order
is kept. Link stores: `mapped_symbol_links: LinkStore<SymbolId, MappedSymbolLinks>`
etc. with the Go field names (`value_symbol_links` is a
`ValueSymbolLinkStore`, whose reads give symbol ids; see Threads). A store
keeps its values in 64-key pages, so a value over 32 bytes (a compile-time
check in `LinkStore`), or one that few keys have, goes in a `Box`
(`type_node_links: LinkStore<Node, Box<TypeNodeLinks>>`).

`checker/mapper.rs` defines `TypeMapper` (an enum over the Go mapper kinds)
and its constructors. Go `m.Map(t)` -> `self.mapper_map(m, t)`,
`m.Kind()` -> `self.mapper(m).kind()`, `m.MapsThisOnly()` ->
`self.mapper(m).maps_this_only()`.

The port keeps every mapper and inference context of a run (typebox: 21.4M
mappers, about 3.9M contexts), so these types are small (infermem1). A
compile-time assert holds each size on 64-bit targets. Keep the asserts: a
new field that few values set goes in the box.
- `TypeMapper` is 16 bytes. The `Array`, `ArrayToSingle`, `Deferred` and
  `Function` payloads are boxed. The `Array` bool (the cached Go
  `MapsThisOnly`) is outside its box, so the box is 48 bytes. Mapper ids
  and their order do not change.
- `InferenceContext` is 56 bytes. The fields that only signature inference
  sets (the return mappers, the inferred type parameters and their origin,
  the intra-expression inference sites) are in `rare`, a box made on the
  first write (`rare_mut`). Their accessors (`return_mapper()` and the
  others) give Go's zero values while it is absent. `inferences` is a boxed
  slice: its length never changes.
- `InferenceInfo` is 32 bytes. Its two candidate lists are in one box
  (`candidate_lists`) that the first candidate makes. `candidates()` and
  `contra_candidates()` give Go's nil lists while it is absent.

## AST (owned by ast/node.rs, ast/fields.rs, ast/misc.rs, ast/utilities_*)

Go reads the AST without a context. So do we: `crate::core::prog()` returns
the current `&'static GoProgram` of the thread (see Threads). `Node`
methods reach the AST through it.
- `node.Kind` -> `n.kind() -> SyntaxKind`; `node.Flags` -> `n.flags() -> NodeFlags`
  (Go flags: parser flags plus binder-added flags); `node.Parent` ->
  `n.parent() -> Node`; `node.Pos()`/`End()`/`Loc` -> `n.pos() -> i32`,
  `n.end() -> i32`, `n.loc() -> TextRange` (Go `core.TextRange`, defined in
  node.rs with `pos()`/`end()`/`len()`).
- `ast.GetSourceFileOfNode(n)` -> `get_source_file_of_node(n) -> Node`.
  File data: `n.go_file() -> FileRef<GoFile>` (any node);
  `file.AsSourceFile().X` for Go SourceFile fields -> `source_file_info(file).x`
  (parser/program fields, `program::SourceFileInfo`) or
  `file_bind_data(file).x` (binder fields, `core::FileBindData`). Text:
  `file.Text()` -> `source_file_text(file) -> &'static str`,
  `file.FileName()` -> `source_file_file_name(file) -> &'static str`.
- File data guards (lsshells M3b): a published file is static (never
  freed) or a freeable file version (`ast/file_version.rs`), so the file
  data accessors return a `FileRef<T>` guard, not a `&'static T`:
  `ast::go_file`, `try_go_file`, `source_file_info`, `file_bind_data`,
  `FlowNodeId::get_flow`, `source_file_diagnostics`,
  `source_file_js_diagnostics`, `source_file_jsdoc_diagnostics`,
  `source_file_bind_diagnostics`,
  `source_file_ecma_line_map`, `get_ecma_line_starts`,
  `source_file_get_name_table`, `source_file_get_position_map`,
  `source_file_get_declaration_map`, `get_pragma_from_source_file`. It
  derefs to the data; `as_static()` gives the `&'static` borrow of a static
  file. A guard keeps its file version alive, so read the fields you need
  and let it go; copy or clone to keep a value. Hot readers use a closure
  (`ast::with_go_file(id, |g| ..)`, `with_published_store` in `ast/store.rs`),
  which pins the version for the read only. A slice of a freeable file's
  data is a handle that reads at each use (`NodeSlice::from_file_imports`,
  `from_file_js_doc`). A read of a dead file version panics ("file version
  N is released"); it never reads another file.
- Binder data (Go fields set by the binder on nodes): `n.symbol()`,
  `n.local_symbol()`, `n.locals()`, `n.flow_node_data().flow_node`... use
  `n.bind() -> NodeBindData` (a copy) and its fields; Go `node.Symbol()` ->
  `n.symbol()`, `node.Locals()` -> `n.locals()`, `node.LocalSymbol()` ->
  `n.local_symbol()`, `node.FlowNodeData().FlowNode` -> `n.flow_node()`,
  `EndFlowNode`/`ReturnFlowNode` -> `n.end_flow_node()`/`n.return_flow_node()`.
  Flow data: `f.get_flow() -> FileRef<FlowNode>`.
- Go field access through `As*()`: `node.AsBinaryExpression().OperatorToken`
  -> `n.operator_token()`. One accessor per Go field name, generated in
  `ast/fields.rs`, that works for every kind that has that field (panics on
  other kinds). Return types: node field -> `Node`; `*NodeList` ->
  `NodeList`; `*ModifierList` -> `ModifierList`; `string` -> `&'static str`;
  `Kind` -> `SyntaxKind`; flags -> their flag type; bool -> bool.
  Name clashes with Go `Node` methods of the same name (`Name()`, `Body()`,
  `Type()`, `Expression()`, `Initializer()`, `Text()`, `Arguments()`,
  `Parameters()`, `Members()`, `Statements()`, `TypeArguments()`,
  `TypeParameters()`, `Elements()`, `Properties()`, `ModuleSpecifier()`,
  `ImportClause()`, `Label()`, `QuestionToken()`, `PostfixToken()`, ...):
  the Go method wins (in node.rs; returns nil instead of panicking on kinds
  without it); fields.rs does not generate a clashing name. Go methods that
  return `[]*Node` (`Arguments()`, `Parameters()`, `Members()`,
  `Statements()`, `Elements()`, `Properties()`, `TypeArguments()`,
  `TypeParameters()`, `ModifierNodes()`, ...) return `NodeSlice` (Copy;
  `len()`, `is_empty()`, `get(i) -> Node`, `iter()`, `first()`, `last()`,
  `to_vec()`; empty when nil). `NodeList` has `.nodes() -> NodeSlice`,
  `pos()`, `end()`, `loc()`, `has_trailing_comma()`, `is_nil()`.
  `ModifierList` has `.nodes()`, `.modifier_flags()`, `is_nil()`.
- `NodeList`, `ModifierList` and `NodeSlice` are Copy handles defined in
  `ast/node.rs`. A list of parsed data points at its astdata list (it lives
  for the process). A list of synthetic data is an index into the thread's
  synthetic arena (`SyntheticList`), so it is valid only on the thread
  that made it, like a synthetic `Node`. A list of a store that owns its
  nodes (a freeable parse, lsshells M3c) is a `StoreList` handle (file,
  node data cell or pending list, and the id of its selector,
  `SelectorSite`), read at each use; it is valid on any thread while its
  file version lives, and its `list_ptr` address can repeat after the
  version is freed, so keep that address only for keys of one request.
  Equality is Go pointer equality.
- Node factory (`c.factory.NewX`) is unported for now: `unported!("NewX")`.
- Go `ast.IsX(node)` predicates -> `is_x(n)` free functions.
- Node data reads are scoped, because a thread owns its synthetic nodes
  and frees them when its program is released (`ast/synthetic.rs`). Read
  a field with `by_data!`, `with_data!` or `with_ast_data(n, |d| ...)`,
  and a list field with `list_of!` or `modifiers_of!` (`list_by_data!` in
  `node.rs`). Nothing returns a reference into node data. Only the data of
  a static parse is `&'static` (`static_ast_node`); a node that a freeable
  parse owns (lsshells M3c) is read in a scope like a synthetic node
  (`with_scoped_ast_node` holds it, `read_scoped_ast_node` reads a field;
  the body of `with_data!` must not make or change nodes of the store of
  the node). The binder, which binds parsed nodes only, loads the data
  once with `parsed_node_data` for the `_in` reads (`data_accessor!`); it
  gives `LoadedData`, `None` for an owned node, and then an `_in` read reads
  the node again. `Node::bind()` returns the binder data by value, and the
  text of a synthetic node or an owned node is interned (`Name`; an owned
  node of a published file keeps it per thread in `JOINED_TEXT`).
- Synthetic node owners (`ast/synthetic.rs`): on the language server
  dispatch thread, each program version owns the synthetic nodes, lists
  and data writes made while it is current, and its release frees them
  (`open_synthetic_owner`, `free_synthetic_owner`). A freed handle panics
  on read and is never given to another node. Code whose synthetic nodes
  a cache keeps across program versions (token cache, lazy JSDoc, parses,
  files published outside a program) opens `enter_base_synthetic_owner()`
  so they belong to the thread. `GOPORT_SYNTHETIC_OWNERS=0` turns owners
  off.
- Store columns and the other registry tables of a published file are read
  through one file lookup, `file_block` in `ast/store.rs` (AST node records
  step 3). Each published file id has one block (`FileBlock`: kinds,
  records, kids and a `BlockFile` with the node column, facts, root,
  foreign parents, links, and the store and `GoFile` of a static
  publish), in a
  static array for ids below 2^16 (`FILE_BLOCKS`, one cache line per
  entry) and in chunks made on demand above (`HIGH_BLOCKS`, read in a cold
  block that calls the empty `high_block_path`). So the first program, a
  later program (`tsc -b`, watch, an edited file) and the node shell of a
  freeable version read the same way: a hot node read is two dependent
  loads, and has no call. The block of a freeable version is its node
  shell (records and kids in a pooled block, step 4; kinds and foreign
  parents leaked; no node column when its store owns its nodes; no link
  column), so its header and child reads are block reads, and its node
  data reads read the pinned version (`static_store_node`,
  `with_scoped_store_node`). A read of the store or the `GoFile` of a
  node shell reads the pinned version out of line
  (`with_published_store`, `try_with_go_file`); `static_go_file` gives
  `None` there. A new per-slot column goes into a record or kids word (the
  kind column is the one exception, see "Node records"); a new per-file
  table goes into `BlockFile`. Keep the fast paths free of
  calls: a call there made `Node::parent` go out of line (AST node records
  step 2b). A node shell has no link column, so `frozen_store_children`
  gives `None` and the caller reads the node data.
- Node records (AST node records plan steps 1 to 4 and astmem1 P3,
  `ast/store.rs`).
  Each store slot has one 24-byte `NodeRecord` (`flags`, `bind`, `loc`
  and `up`), a `SyntaxKind` in the kind column (`FileStore::kinds`)
  and one 16-byte `NodeKids` (the U4 and C2 child ids,
  and a word with the U1 name of an identifier or the U1 (b) modifier
  bits of any other slot). They replace the header, kind, name, modifier
  bit, child and resolved columns. `up` of a node slot holds the parent
  code (0 nil, slot + 1 for a parent in the store, the top bit and an
  index into the store's foreign parent table for any other parent) and
  the Go symbol; `up` of the nil slot or an alias slot holds its target.
  `bind` is 32 bits: the low half of the flow node (always in the same
  file), or with `BIND_EXTRA` (bit 31) the index + 1 of the node's
  `NodeBindExtra` (flow node, local symbol, locals, next container, end
  and return flow nodes) in `GoFile::node_bind`. About 6% of the slots
  have extras (locals containers, exported declarations, function-like
  nodes), so the flow read of every other node is one load. The record
  flags are the parser flags, and after the bind also the binder-added
  bits (`BINDER_ADDED_FLAGS`), so `parser_flags(mask)` stays exact for a
  mask without them, and the parser flags of a published file are the
  ones in its `GoFile`. The record bits (`SOURCE_FILE_ROOT`,
  `TEXT_IS_KEYWORD`, `NO_NODE`) are bits 29 to 31 of `flags`: Go
  `NodeFlags` ends at bit 28. `NodeRecord::flags` masks them, and a parse
  flags write checks that no Go flag is in them (`checked_flags`). The
  node column (`FileStore::nodes`, `BlockFile`) names a leaked `NodeData`
  (16 bytes) for each node slot, not a whole `astdata::Node` (astmem1
  P1): the kind is in the kind column and the header in the record, so no
  read needs the rest. The node reads (`static_ast_node`,
  `frozen_store_ast_node`) give `&NodeData`. The words are atomics that the reads load
  with `Relaxed`. The parse writes them through `get_mut`; after the
  publish only `BoundFile::install` writes a record (`bind_store_records`:
  symbol, added flags, `bind`), through a shared ref, before any other
  thread reads its binder fields. It ORs its bits into `flags`, which
  `NodeRecord::header` reads, and writes the high half of `up`, whose low
  half `header` reads; `header` reads each word once, so it never mixes
  the halves of one word. A live bind hands its builder
  (`NodeBindBuilder`) to the install; a lib bind snapshot load hands the
  compact form (`NodeBindParts`), which the lib bind blob keeps.
  The kind is a plain `SyntaxKind` column, not a record word (step 4): a
  pooled block takes another file's records through a shared ref, so a
  record word must be an atomic, and safe Rust has no cheap `u16` to
  `SyntaxKind`. An `AtomicU16` kind read through a 512-entry table cost
  `goport -p` +1.3% to +2.1% instructions against step 7, and a
  `transmute` of the atomic load (not allowed here) still +0.6% to
  +1.1%; `SyntaxKind::try_from` is a 351-case switch until late in LLVM's
  pipeline, so the inliner left it or `frozen_store_kind` out of line at
  thousands of call sites (+3% to +7.6%). The column costs 2 bytes per
  slot, and a node shell leaks its column unless one of the last 4 shells
  had the same kinds (`shell_kinds`). A new per-slot field of the hot reads goes into a record or kids
  word, not a new column. Debug builds check the records and kids against
  the node data at freeze (`debug_check_kids`), and each parent write
  against its stored form.
- Pooled node blocks and the owner check (AST node records step 4,
  `BlockPool` in `ast/store.rs`). The node shell of a freeable version
  copies its records and kids into a pooled block (a leaked block of
  records and kids, 40 bytes per slot, with room for about 1/8 more slots
  and 64 more). The version gives the block back when it dies; it waits
  in a quarantine until 2 more pin releases (`pin_epoch`: program
  releases, and lease releases that free a parse, below), then a
  later node shell of a size it fits (`len` to `2 * len + 64` slots) takes
  it. In a language server with no lease release between the edits, the
  version that dies in the release of edit N gives its block to the
  version of edit N + 3. `bind` of the nil slot
  holds the owner (the file id), written at the publish. A read that
  finds another owner panics with "file version N is released", as a read
  of a dead version's store does. With debug assertions every
  `file_block` read checks it. A release build checks it
  (`check_block_owner`) in the binder field reads (`frozen_owned_record`:
  the symbol, the flags, the `bind` word with the flow node and the
  extras index, `frozen_store_bind_and_file`, `frozen_store_any_symbol`)
  and in `bind_store_records`, the one writer of a published record.
  The check is one load of the nil slot record, a compare and a branch:
  `goport -p` 0.0% to +0.2% instructions, the editor long sessions +0.06%
  to +0.15% (ownercheck1, R147 reviewer option 2; the flags read is about
  two thirds of it). The hot header and kids reads (kind, parent, loc, child ids,
  name, modifier flags) are not checked in a release build (they are
  about 6 instructions). So a stale read of those gives the values of
  that node while its block waits, and the new owner's data after a
  reuse, never undefined behavior. Its kind column is never reused, so a
  stale kind read gives the old kind. Its store, `GoFile`, extras, flow,
  node data and list reads panic. Every holder of a node of a version
  holds the version, so a correct reader never sees a reuse. No standing
  run has debug assertions: the protected tests run `--release`
  (`build-goport-tests.sh`), and the corpus, editor and oracle runs use
  release bins. So those runs find a missed holder only through the
  release checks (the binder field reads above and the reads that
  panic). A debug test build (`cargo test` without `--release`) also
  checks each header and kids read. This is the owner check that the R141
  reviewer asked for before any step that reuses records: it replaces step
  2's tripwire (`freeable_version_owns_its_lists_and_a_stale_read_panics`
  reads the old values inside the quarantine), and
  `edited_file_blocks_wait_two_releases_then_get_reused` and
  `pooled_node_blocks_wait_two_releases_and_check_their_owner` check the
  panic after a reuse, in both builds for the binder field reads. The
  extras and flow nodes of a node shell stay in its `GoFile` (step 6
  moves them into pooled blocks).

## Program (owned by program.rs)

`core::GoProgram` and `core::GoFile` are fixed. **Pending Theo's approval
(A1, multi-program plan):** this model replaces one program per process.
The batch that adds it is not accepted until Theo approves.

- `GoProgram` is one program version (Go makes a new `Program` for each
  edit). It has an `id` (`core::next_program_id`), the file ids in Go order
  (`source_file_order`), options, binder symbols and its program state. It
  has no file list.
- `GoFile` is one file version. The file registry (`ast/store.rs`) owns it
  from `publish_file_stores` on, or its `FileVersion` for a freeable file
  version; read it with `ast::go_file(id)` (a `FileRef` guard) or
  `ast::with_go_file(id, |g| ..)`. Program versions share the file
  versions they have in common, as Go shares unchanged `SourceFile`
  objects.
- The per-version program tables (files by path, file metadata,
  diagnostics, checker file associations, the declaration diagnostic cache
  and the Go frontend copies in `GoSharedState`) are in
  `program::VersionTables`, behind an `Arc` in the leaked `ProgramState`.
  Read them with `with_tables(|tables| ..)`, which caches the current
  version's `Arc` in a thread-local, so a hit costs no atomic operation.
  A closure must not enter another program. An accessor that returned a
  `&'static` borrow of the tables returns an `Arc` clone
  (`get_redirect_for_resolution`, `get_project_reference_from_source`,
  `get_go_symlink_cache`, ...). The program of a one-program process
  leaks its tables and reads them with no lock or thread-local.
- `program::release_program` frees the checker pool, the emit pool, the
  frontend and the tables of a version. The frontend `NewProgram` goes
  with its last `Rc` holder. Its parses stay: the publish that gives a
  file its `GoFile` keeps that file's parse, because the `GoFile` borrows
  it (a freeable file version, below, is the exception). `GoSharedState`
  owns its copies of the frontend data. The module resolutions are not
  copied: the frontend keeps them in an `Arc` map of `Arc<ResolvedModule>`,
  and `GoSharedState` shares that map (a lookup borrows the name through
  `module::ModeAwareKey`). Each checker worker frees its
  checker and the synthetic nodes it made (`free_synthetic_nodes`), and
  each emit thread frees its synthetic nodes. A worker, bind, emit or
  search thread gets a copy of the tables `Arc` in its `WorkerSeed` and
  keeps it until it ends, so a thread can finish its work after the
  release, as a Go goroutine that holds the program does. A read of a
  released version's tables on a thread with no copy panics ("program
  version N is released"). `GOPORT_KEEP_VERSION_TABLES=1` keeps them (A/B
  runs and a field fallback). The `GoProgram` shell and the static file
  versions (the first version of each file, but a leased one, below) stay
  leaked for now. A
  one-program process forgets its checkers and the
  synthetic nodes of both pools at the end, like Go. Watch mode uses
  `program::release_program_in_background`: the old checker pool stops
  without a wait, and the old checkers are freed on the pool threads while
  the new build runs. A full build keeps the old frontend program until
  after the status report, so its free is not in the rebuild time. From
  its second build on, `tsc --watch` makes the new parses of published
  paths freeable file versions (below, watchfree1), so a rebuild frees the
  versions it replaced, as Go's GC does.
- The parse tasks of a load go with the loader. Go's garbage collector
  frees them. Here the `sub_tasks` and `loaded_task` links make an `Rc`
  cycle when files import each other, so the `FilesParser` drop takes
  those links out. A one-program process forgets the loader
  (`with_loader_state_forgotten`), so it does not pay for the free.
- Go's garbage collector frees old data in the background, never in a
  request. On the dispatch thread of the LSP server, the large frees
  wait until the answer is sent (`gostd::local::drop_later`): the tables
  and frontend program of a released version (`ls_program::release_now`)
  and the parse tasks of a load. The dispatch loop drops them after each
  message, while no message waits (`drop_garbage`); more than 16 are
  dropped even when messages wait. Other threads drop them at once.
- Freeable file versions (lsshells M3a to M3d, `ast/file_version.rs`). In
  a language server or API process (`project::new_session`), a parse cache
  parse of a path that a publish on this thread published before gets a
  `FileVersion`. An API source file lease (`acquire_source_file`, apimem1)
  notes its path before its parse, so its parse gets one even as the first
  version of the path, and the release of its last lease frees it, as Go's
  GC frees the leased `*ast.SourceFile` (`project::drop_released_lease`).
  In a `tsc --watch` process (`Watcher::start`,
  `ast::set_watch_process`; watchfree1) each build does the same for its
  new parses (`program::mark_freeable_parses`), and parses ahead
  (prefetch) only in a build with an empty source file cache: the first
  build, and a build after an overflow or a config change. A parse
  worker's parse of a published path is then a freeable parse too
  (`CompilerHost::freeable_worker_parses`, watchcfg1 round c): its detached
  store owns its nodes and its text is shared, so the file version frees
  it, and a worker parse that the loader does not take is freed with the
  pool after the load. Go parses every file again after a config change. Here the build after it keeps the parse of a file whose
  text is the same and that has no diagnostics in the last snapshot
  (`WatchCompilerHost::reuse_parse`, watchcfg1; Go's error summary groups
  errors by file object, and a copied diagnostic keeps the old one), when
  its parse options are the same or differ only in module indicator
  options that its parse did not read (a file with an import or export):
  then it keeps a copy with the new options (`parser::parse_with_options`,
  the parse worker rule of `read_module_indicator_options`). The parse
  workers skip these parses (`cached_source_file_refs`, each for the module
  indicator options that it read, `FileRefs::of_kept_parse`) and parse the
  other files, as Go does on goroutines. The watch file system is not the
  plain OS file system, so a changed lib is a live parse, not a
  `lib_parse.bin` load. A diagnostic that a watch build copies from the
  last build (Go `repopulateDiagnosticsOfFile`) points at the file
  versions of the build that made it, and Go prints it from them: the new
  snapshot holds them (`Snapshot::held_file_versions`). `tsc -b --watch`
  (`Orchestrator::start`) does the same for the program of each task. Go
  parses every file of each project that it builds again in each cycle
  (`resetCaches`); here a file keeps its parse while its modification time
  does not change, no watch event names it and no build of the cycle
  wrote it (`BuildHost::watch_source_file`, as Go `tsc --watch` keeps its
  files), so a cycle parses only the changed files, on the loading thread.
  A `.d.ts` or `.json` file still comes through `source_files` (Go
  `sourceFiles`) first, which keeps the first parse of a cycle until the
  cycle ends, as in Go: a project that builds beside an upstream project
  (no reference) can read the upstream `.d.ts` before that build writes
  it, and a downstream project of the same cycle then gets the old parse
  (bwsig1, `build_watch_keeps_the_first_dts_parse_of_a_cycle`).
  A config change keeps the parses too: their key holds the parse options,
  and a parse with other module indicator options that it did not read is
  kept as a copy, as in `tsc --watch`
  (`BuildHost::keep_watch_sources_for_config_change`). A config change
  that gives a project other module indicator inputs
  (`ModuleIndicatorInputs`) drops the kept parses of that project's files
  that read their options when the file's options under the new inputs
  are not those of the parse
  (`BuildHost::drop_kept_parses_whose_module_indicator_options_change`);
  the parses of the other projects stay. An overflow drops all. Parse
  workers parse ahead in the first build and in the cycles with a config
  change or an overflow (`BuildHost::prefetch`); they skip the kept
  parses, as in `tsc --watch`.
  A task keeps the versions that its errors point at until its next build
  or a reset of its status (`BuildTask::held_file_versions`). A file
  version's parse holds it (`ParsedSourceFile::version`), and so do the
  `VersionTables` of each program version that has the file, so a
  seeded thread keeps it too. A thread that reads it pins it until the
  next pin release (`release_file_version_pins`: run when a
  `ReleasedProgram` drops, and after the answer when a lease release lets
  go of the last holder of a freeable parse,
  `release_file_version_pins_later`) or its end, and a `FileRef` guard
  holds it. The
  registry keeps a `Weak`. At publish the version takes its `FileStore`
  and its `GoFile` (M3b). Its node records and kids (`NodeRecord`,
  `NodeKids`; 40 bytes per node, and 2 in the kind column) are in its node shell, the registry
  block of its id (a pooled block, AST node records step 4), so a header or child
  read of the edited file stays inline (a pinned read per node read made
  edits 3 to 4 ms slower); the child link column is dropped, and the
  binder's child walk reads the node data. With owned nodes (M3c; on by
  default in a language server or API process since M3g,
  `GOPORT_OWNED_NODES=0` turns them off) its parse was a freeable parse
  (`enter_freeable_parse`, opened by the parse cache), so its store owns
  its astdata nodes, pending lists, JSDoc cache and parse diagnostics
  (`OwnedAst`), and they are freed with its store, when the version dies
  or after it on a free thread (`FileVersion::take_data`); its node data
  reads are pinned reads, and its lists are `StoreList` handles. Only with
  them do 1000-edit sessions pass memory. Their cost against R139 (M3g, pin B):
  session instructions +6.40% on effect and +4.68% on query-core (+2.2%
  with owned nodes off); edit median +0.5 to +2.1 ms on effect and +0.8 to
  +1.7 ms on query-core in 200-edit sessions, +0.9 ms (query-core) and
  +1.0 to +1.2 ms (effect) in 1000-edit sessions. The CLI costs
  (instructions: query +1.38%, hono +0.48%, zod +0.18%, effect +0.19%)
  come from the M3 merge, not from owned nodes. The planned fix of the
  read cost is the AST node records plan
  (`target/continuation-r97-goport/ast-design/study.md`). A prefetched parse keeps its
  nodes leaked (its node column is in the shell), and so does every parse
  with owned nodes off, as before M3c. Its
  `SourceFileInfo` owns copies of
  the parse lists (`KeptData::Owned`), so the publish keeps no parse, and
  its name table, position map, declaration map and identifier set are
  fields of the version, as in Go. When its last holder lets go, the store
  and the `GoFile` are freed, a later read of the id panics, and each
  per-file
  thread-local map (`PerFileMap`: `SOURCE_FILE_DATA`, `TOKEN_CACHES`,
  `TOKEN_FACTORIES`, `NODE_IDS`, `SUBTREE_FACTS`, `JOINED_TEXT`,
  `DECORATORS`) forgets the entries of the file at its next write. Its
  symbols and tables in the binder lineage are whole chunks of its own
  (M3d); after it dies, the next bind frees them, and a read of one of its
  symbol or table ids panics (index out of bounds). Its pooled block goes
  back to the pool (see "Pooled node blocks" above); its small `BlockFile`,
  its foreign parents and its text stay leaked for now. The first publish, the
  first version of each file (but a leased one, above) and every CLI
  publish except `tsc --watch` and `tsc -b --watch` never get one.
  `GOPORT_FREE_FILE_VERSIONS=0` turns this off, `=1` turns it on in any
  process; there `update_program_version` (`goport_multiprog`) also gives
  each new parse of a published path a version, so a leak record can
  measure it.
- A `tsc -b` build (`goport_build`, `tsgo -b`) is a multi-program process,
  like Go: each project's program is a version made with `new_program` and
  `program::new_program_version`, and it is released when its task
  reports. The build host shares its parsed `.d.ts` and `.json` files
  between the programs, and the parse workers of a later program do not
  parse them again (`CompilerHost::cached_source_file_names`, not in Go).
  A file that one program parsed and left out (a deduplicated package)
  can be a program file of a later one, so the build
  host notes each parse that it keeps (`program::note_parsed_source_file`)
  and a publish gives it its complete `GoFile`. The publish asserts that
  every program file is a source file. The programs are made on one
  thread, but each program's check starts on its own checker pool when
  the program is made (`incremental::Program::start_check`), and a
  released pool frees its checkers in the background
  (`program::release_program_in_background`). So the pools of up to 4
  started projects work at the same time, like Go's goroutines. A
  project's emit starts behind its check when `Program::start_emit`
  allows it, and its writes wait in a buffer
  (`buffer_early_emit_writes`) until the task finishes. A task finishes
  when its check and its early emit (when it has one) have ended (the
  barrier jobs behind them also
  wait for the d.ts twins and the emit pool,
  `program::send_checker_barrier`), in the order they end, or in build
  order when the outputs of the tasks overlap. A project's emit
  runs on its own checker threads and its own emit pool
  (see Threads): the emit resolver needs the file's checker, which lives
  on its worker thread, and synthetic nodes are thread-local
  (`ast/synthetic.rs`).

  Known gaps (README "Known problems", K2). Go loads the programs of the
  started tasks at the same time; here they load one at a time on one
  thread. So a project that imports another project's output without a
  reference can read it before or after Go's reader does:
  - G1: the reader references a project that builds before the writer.
  - G2 (the rest): several large projects with the default builders.
    Their affected-file walks wait for the loading thread: for an
    emit-only task, and for a checked task too
    (`incremental::Program::start_check`; Go runs `collectAllAffectedFiles`
    on the task's goroutine). k2redis1 repro: 3 checked projects.
  - G3: a `noEmitOnError` project has no early emit. It finishes when its
    check ends, then emits and writes; Go's builder writes when that emit
    ends. Starting its emit when its check ends (k2gaps1 round 1) broke
    small shapes where Go and the port agreed (state note
    `k2gaps1-round2-2026-10-08`).
  - G4: a project that is not `incremental` or `composite` has no
    incremental state and no early emit. It finishes when its check ends,
    then emits and writes on the loading thread; Go's builder emits the
    whole program and writes when that emit ends. k2gaps1 rounds 1 and 2
    gave it an early emit of the whole program with buffered writes. That
    broke two things where Go and the port agreed (state note
    `k2gaps1-round3-2026-10-08`):
    - `--verbose` on a no-op build named another oldest output than Go:
      the buffered writes fall within about a millisecond.
    - Probe `inv_noeoe_noninc_m6` (`tsc -b p1 p2 p3 --builders 2`, none
      `incremental`): p1 is a large `noEmitOnError` writer, p2 a checked
      project, p3 reads p1's output without a reference. Go and the port
      give TS2305: p2 finishes first. With G4 fixed and G3 not, p2 under
      load finished after p1's check, so p3 read p1's new output. Fix G3
      and G4 together.
  - A large `noEmitOnError` project with a syntax error (k2gaps1 probes
    `noeoe_syn_comp`, `noeoe_syn_inc`): it has no checker work, so it
    finishes at once, in start order. This is G1's family.
  - G5: the other projects with no early emit finish when their check
    ends, then emit and write, as in G3; Go's builder writes when that
    emit ends. These are the projects where `check_cannot_see_outputs`
    fails (F1: node16 or nodenext with a checked relative module name
    without an extension; F2: a program file inside `outDir` or
    `declarationDir`; F3: a `node_modules` segment in either), and the
    other `early_emit_options_allow` cases: `preserveSymlinks` (F4),
    `outFile`, `--generateTrace`, and every project with
    `GOPORT_EARLY_EMIT=0`. `--singleThreaded` is not a gap: Go's build
    then runs one task at a time (execute/build/orchestrator.go:925
    `rangeTasks`).
  - C1 rate shift (int56; state note `int56-decision-2026-10-09`). C1
    makes a task with early emit finish when its emit pool jobs and d.ts
    twins end (`program::send_checker_barrier`), as Go's task finishes
    when its emit ends. In probes `c1_fan_imp_b2` and `c1_fan_nc_k600_b2x`
    a small project k (one big file) and a big emit project w build
    together, and readers read w's output without a reference. On loaded
    zbook the rare answer (rc 0) came from int56 in 13 of 90 runs, from
    R184 in 1 of 90 and from Go in 2 of 90 (Fisher p about 0.001). The
    answer stays in Go's set, and quiet hosts gave one answer. Cause: in
    the full build the port's k finishes late (350 to 460 ms against Go's
    140 to 180 ms), so k and w almost tie, and C1 moves k a little later.
  - G6, partial writes: Go writes each output when the emit of its file
    ends, so a task that loads during that emit reads some files old and
    some new. The port keeps an early emit's writes until the task
    finishes (`buffer_early_emit_writes`), so it reads all old or all new.
    node-redis step 2: Go reads `commands/index.d.ts` old and
    `AGGREGATE.d.ts` and `CREATE.d.ts` new in 6 of 6 runs; the port reads
    all old (k2redis1). A Go clock for G1 alone does not fix this.

`program.rs` defines `SourceFileInfo`, `load`, `bind_all`, the Go
`Program` methods as free functions with Go snake names (`get_resolved_module(file, name, mode)` ->
`*module.ResolvedModule` port as `Option<ResolvedModule>` struct with Go
fields, `get_source_file_for_resolved_module(name) -> Node`, ...), the
checker pool, and diagnostic sorting. `options.rs` defines Go-shaped
`CompilerOptions`; read it from `prog().options`.

## Exit codes

`tsgo`, `goport`, `goport_emit` and `goport_build` compile and report
through the shared `execute::execute_tsc` (Go execute/tsc.go) and
`execute::tsc` modules, as Go `tsc` does. `tsgo` returns the Go status.
`goport` and `goport_emit` return the tsc status (Go
execute/tsc/emit.go:65): 0 success, 1 diagnostics with emit skipped, 2
diagnostics with emit not skipped. Under
noEmit, a program with no emittable file (no inputs, or only `.d.ts`
files) exits 2. `goport_build` returns the Go build status, which can also
be 3 or 4. Unported code, any other panic and a worker-thread failure
exit `execute::tsc::EXIT_UNPORTED` (70, `EX_SOFTWARE`). Go uses 0 to 5 (3
in cmd/tsc/sys.go:128, 4 in build mode, 5 NotImplemented), so a harness
must treat a goport exit of 70 as a crash, never as a tsgo status.

A site where the pinned Go panics on the same input uses
`core::go_panic(message)`, not `panic!`. It is not a port gap: the guards
that keep a run going pass it on (`core::resume_go_panic`), and the bins
end the run as the Go runtime does. The output written so far stays,
stderr gets `panic: <message>` (then the port site in place of the
goroutine trace), and the exit code is 2 (`core::EXIT_GO_PANIC`).

Go `sync.WaitGroup.Go(f)` recovers a panic in `f` and panics again, so the
runtime line ends with ` [recovered, repanicked]`. Where the port runs
such a goroutine on the dispatch thread (background queue tasks, their
timer continuations, the telemetry ticker, the idle auto-import warm), it
runs `f` under `core::go_wait_group_task`. Where the port runs it inline
inside work that a Go `recover()` guards (the auto-import registry build
under a request), it runs `f` under `core::go_wait_group_goroutine`: a Go
panic ends the process there, because Go's recover sees only its own
goroutine.

## Process start

`tsgo` starts as the Go runtime and the Go `syscall` package start a Go
process (bin/tsgo.rs `unblock_go_signals`, `go_runtime_start`).

- Signals (Linux, Go runtime/sigtab_linux_generic.go): the signals that Go
  drops when nothing asks for them (USR1, USR2, ALRM, CHLD, URG, XCPU,
  XFSZ, VTALRM, PROF, WINCH, IO, PWR and the real-time signals 35 to 64)
  get a handler that does nothing, also when they were ignored at start.
  It is not SIG_IGN, so a process that tsgo starts gets the default
  actions, as from Go, and a wait for a child works when the caller
  ignored SIGCHLD (the kernel reaps the children of a process that
  ignores it). SIGQUIT, SIGSTKFLT and SIGSYS print the Go name (`SIGQUIT:
  quit`) and exit 2, also when they were ignored at start (PORT: Go then
  prints the goroutines). SIGINT and SIGTERM go to `notify_context`.
  SIGHUP ends the process by SIGHUP, as Go's `dieFromSignal`, unless it
  was ignored at start (the `SigIgn` line of /proc/self/status; Go then
  keeps it ignored, so a process that tsgo starts gets it ignored too).
  PORT: SIGABRT and SIGTRAP keep their default actions. Other systems keep
  the default actions.
- A failed exec of `set_malloc_tunables` (a binary that is gone, for
  example) leaves SIGPIPE with its default action (std `Command` sets it
  for the new image). tsgo then gives SIGPIPE a handler that does nothing,
  so a write to a closed pipe or socket gets EPIPE again, as after std's
  start and as in Go (a write to fd 1 or 2 still ends the run,
  execute/tsc/stdio.rs `sigpipe`). Before, a second SIGINT or SIGTERM
  ended such a run by SIGPIPE: `notify_context` writes to its closed
  self-pipe.
- The pid 1 of a PID namespace (`docker run` without `--init`, `unshare
  -pf`, `bwrap --as-pid-1`): the kernel drops each signal with the default
  action that such a process gets from its namespace or sends itself. Go
  catches every signal above, so its SIGHUP still ends the run: Go's
  `dieFromSignal` raises the signal, and when that returns it exits
  128 + N, as a shell reports a process that a signal ended. tsgo does the
  same (bin/tsgo.rs `end_by_signal`): SIGHUP exits 129, and a launcher
  whose worker a signal ended exits 128 + N. PORT: std and rustix have no
  safe `SIG_DFL`, and signal-hook's default action calls `abort` when the
  raise returns, which ends a pid 1 by SIGSEGV (rc 139). So a pid 1 does
  not raise the signal; it exits 128 + N at once. So does the SIGPIPE of
  a broken stdout or stderr (execute/tsc/stdio.rs `sigpipe`): a pid 1 that
  runs the work itself (no worker) exits 141, as Go does.
- PORT: SIGILL, SIGBUS, SIGFPE and SIGSEGV keep their default actions,
  also when another process sends them (`kill`). Go throws a sent one
  (`sigFromUser`) as it throws SIGQUIT: it prints the name (`SIGSEGV:
  segmentation violation`), the PC line and the goroutines and exits 2. In
  the port the process ends by the signal (128 + N, a core dump where the
  limit allows it). A SIGILL, SIGBUS, SIGFPE, SIGABRT or SIGTRAP that was
  ignored at start stays ignored, also in a process that tsgo starts; Go
  catches it, so a process that Go starts gets the default action. There
  is no safe way to give the child the default actions: std `Command` has
  no attribute for them (it resets only SIGPIPE), rustix has no
  `posix_spawn`, a `pre_exec` hook is `unsafe`, and signal-hook refuses a
  handler for SIGILL and SIGFPE (and a handler that returns from a real
  fault runs the fault again). nix's `posix_spawn` (with
  `PosixSpawnAttr::set_sigdefault`) is safe, but its file actions have no
  `chdir`, which the content mapper start needs (Go `cmd.Dir`).
- The signal mask. At start, before any thread, tsgo unblocks the signals
  that Go unblocks on each of its threads (`GO_UNBLOCKED`: sigtab
  `_SigUnblock`, `_SigKill` or `_SigThrow`, and SIGURG: SIGHUP, SIGINT,
  SIGQUIT, SIGILL, SIGTRAP, SIGABRT, SIGBUS, SIGFPE, SIGSEGV, SIGTERM,
  SIGSTKFLT, SIGCHLD, SIGURG, SIGPROF and SIGSYS), so every thread of a
  launcher and of its worker gets them, as Go's threads do. Other blocked
  signals stay blocked. A process that tsgo starts gets the mask of the
  thread that starts it (std `Command` keeps it): the caller's mask
  without these signals, as Go gives its children the mask of the thread
  before the fork (`syscall_runtime_BeforeFork`). PORT: Go also unblocks
  the signals 32 to 34; nix's `SigSet` has no real-time signals, so a
  caller's blocked signal 34 stays blocked in tsgo and its children
  (glibc does not block 32 and 33). PORT: Go lets a caller block SIGURG
  under `GODEBUG=asyncpreemptoff=1`; tsgo does not read `GODEBUG`, and
  both drop SIGURG.
- PORT: `GOTRACEBACK` does nothing. With `GOTRACEBACK=crash`, Go ends a
  thrown signal or a fatal panic with SIGABRT (`crash`, a core dump) after
  the goroutines; the port exits 2 as with the default setting.
- The launcher (`launch`) drops the same signals and sends SIGINT,
  SIGTERM, SIGHUP and the thrown signals on to its worker. It takes
  SIGHUP only after the worker has started, so the worker gets the
  caller's action for it (an exec keeps an ignored signal and gives a
  caught one its default action). When a signal ends the worker, the
  launcher ends by the same signal (a pid 1 exits 128 + N). The worker
  gets no environment variable and no file descriptor from the launcher,
  so its children get none: its `arg0` names the launcher, and at exit it
  opens the launcher's end of the pipe through /proc (first with
  `O_PATH`, so the type, device and inode check opens no file of another
  process).
  - A forwarded signal waits until the worker catches it (its `SigCgt` in
    /proc), so a signal that comes before the worker's handlers does what
    it does in Go after the start: a plain compile goes on after SIGINT
    and SIGTERM, and SIGQUIT prints its name and exits 2. The wait ends
    2 s after the worker starts (`HOLD_LIMIT`); a later signal goes on at
    once. Each signal waits on its own, so one that waits does not hold a
    later one: a SIGQUIT that comes after a SIGINT that came before the
    worker's `notify_context` goes on once the worker catches SIGQUIT.
    PORT: signal-hook sets the handler (the bit) a moment before it
    publishes the action that the handler runs, so a signal sent on in
    between does nothing. Followups9 round b also waited for the worker's
    threads that started after the registration (then `go-signals`, which
    startexit1 removed: the main thread now waits for those signals,
    `wait_go_signals`; and `signal.NotifyContext`). A thread gets its name
    only when it first runs, so that wait held signals longer: with a
    launcher, an up-to-date `tsgo -b` under CPU load (zbook) lost 110 and
    134 of 1000 SIGQUIT and SIGHUP that came in its first 9 or 19 ms, where
    the same build without that wait (goport-int35) lost 81 and 90
    (followups9e).
  - The worker ends with its launcher: a parent-death SIGKILL, and a
    worker whose launcher died before that (its parent is not the named
    launcher, and the named launcher is gone or a zombie) kills itself. A
    killed Go tsgo stops at once. A process whose `arg0` names a live
    launcher that is not its parent runs as a plain tsgo.
  - The launcher and the worker read /proc/<pid> only when /proc is the
    one of their PID namespace (`own_proc`, the `NSpid` line of
    /proc/self/status). With the /proc of another namespace (`bwrap
    --unshare-pid` without `--proc`, `unshare -pf` without `--mount-proc`)
    /proc/<pid> is another process or none. There a signal goes on at once
    (no wait), the ended-launcher check uses `kill` with no signal (a gone
    launcher ends the worker, a zombie one gives a plain tsgo), and the
    launcher takes the code from the worker's exit, which waits for the
    worker's memory to unmap.
  - When the worker cannot start, the launcher runs the work itself.
    SIGINT, SIGTERM, SIGQUIT and SIGSYS get their default actions back
    until the run sets its own handlers, as in a run that never was a
    launcher. PORT: SIGSTKFLT does nothing there (signal-hook has no
    default action for it). In a pid 1 they do nothing there, as their
    default actions do.
- Stdout (execute/tsc/stdio.rs): each write of the tsc output is one write
  of fd 1, in Go's pieces (Go `fmt.Fprint` on the unbuffered
  `os.Stdout`). PORT: when fd 1 is a regular file, the writes of one report
  (the diagnostics of an emit with the file list, the statistics table,
  the `--showConfig` value) are kept and go out together, at most 64 KiB
  at a time: the same bytes in fewer writes. A process that ends inside a
  report by SIGKILL, an OOM kill or `core::go_fatal_newosproc` loses the
  kept bytes (up to 64 KiB of that report); Go's file has the pieces
  written so far. A thrown signal, SIGHUP and a panic write them first.
- Open files (Go syscall/rlimit.go): the soft RLIMIT_NOFILE goes up to one
  below the hard limit. The content mapper and npm starts
  (execute/tsc/compile.rs, cmd/tsgo/lsp.rs) go through
  `gostd::rlimit::spawn`, so the child gets the original limit, as from
  Go. PORT: Go sets it in the child between fork and exec, which needs
  `unsafe`; the port sets its own soft limit back for the length of the
  start (Linux only; see there). The launcher does not raise its limit,
  so its worker starts with the original limit, as a Go child does.
- `GOMAXPROCS` (`gostd::runtime::gomaxprocs`): the variable, else the CPUs
  of the affinity mask, lowered to the CPU limit of the process's cgroup
  (rounded up, at least 2), as in Go 1.25 and later. It sizes the parse,
  bind and emit pools (`program::available_cores`), the auto-import
  checker pool and the search threads, and the LSP telemetry event
  reports it as `goMaxProcs` (Go runtime/metrics
  `/sched/gomaxprocs:threads`). PORT: the checker threads (`--checkers`)
  all run at once; Go runs them on GOMAXPROCS threads. Go reads the value
  again while it runs; the port reads it once.
- `GOGC` and `GOMEMLIMIT` (the VS Code `goMemLimit` setting) do nothing:
  the port has no garbage collector.

## Threads

- `prog()` is the current program of the thread. A one-program process
  calls `core::set_prog` once, and every thread with no current program
  reads that program. `WorkerSeed` sets it on checker and bind threads. A
  multi-program process (watch, `tsc -b`, language server, tests)
  registers each version with `core::register_program_version` and makes
  one current for a scope with `core::enter_program`. There, `prog()`
  panics on a thread with no current program.
- Programs and the program state are read only after load, so they hold
  only thread-safe data (`Arc`, `OnceLock`, `Mutex`).
- Files bind in parallel, each into its own arena, and join the binder
  lineage in file order (`program::bind_all`). The ids equal a serial bind.
  A file version binds once (Go `BindOnce`): a later program version binds
  only its new file versions and adds them to the same lineage. The
  program's binder symbols (`program::bound_symbols()`, a `BoundSymbols`
  guard) are a copy of the lineage in its `VersionTables`, so a release
  frees them (lsshells M2c). A freeable file version starts and ends on a
  chunk start (`SymbolArena::end_chunk`), so its ids and the ids after it
  skip to that start, and its chunks are freed when it dies (M3d). Ids are
  never used again.
- Each program has its own checker pool. Each checker is made on its own
  worker thread and stays there (Go `checkerPool`: 4 checkers, file `i`
  goes to checker `i % 4`). The loading thread sends jobs and merges the
  results in file order. `program::release_program` joins the workers of
  the pool; the released program must not be current on the calling thread.
- Thread-local state (synthetic nodes, node and symbol ids, lazy JSDoc,
  caches) is per thread. A worker starts from a copy of the loading
  thread's state (`WorkerSeed`), so each checker's results depend only on
  its own files, not on thread timing.
- Symbol ids: the port gives a symbol its id where Go calls
  `ast.GetSymbolId` (every `valueSymbolLinks` read through
  `ValueSymbolLinkStore`, and the node builder, symbol accessibility, enum
  relation and emit resolver maps), so one checker counts ids as Go does.
  The store is a type of its own, so a read that gives no id does not
  compile; the 2 reads of a pushed type resolution (`get_noted`) and the
  parameter memo (`try_get_without_id`) are the named exceptions.
  Late-bound names hold ids (`__@k@<id>`), and the node builder counts their
  length toward truncation. Go's checkers share one counter: a worker skips
  the ids that the other checkers' `NewChecker` gave to their own symbols
  (`program::new_pool_checker`). It does not skip the ids of binder
  symbols that `NewChecker` gave (merge error texts): Go gives each of those
  once in the pool. It also does not skip the ids that other checkers give
  while they check, which race in Go. Go also gives each class with private
  names an id at bind time; the port does not
  (`get_symbol_name_for_private_identifier`), so such a class gets its id
  later, at its first id site. The 4 check-time private name sites give the
  class its id, as Go does (`Checker::private_identifier_symbol_name`).
- Several programs in one process: Go's one counter runs on from one
  program to the next, and a bound file keeps the ids of its symbols. With
  `--singleThreaded` (one checker) the port hands the ids of a program's
  checker to its loading thread, so the next program's checker starts from
  them (`program::CheckerPool::carry_symbol_ids`): when the next pool is made
  (watch makes the next program before it releases the last) or when the
  program is released (`tsc -b`). So `tsc -b --singleThreaded` and the
  watch cycles of `--singleThreaded` give Go's ids. Other cases do not
  carry ids, and each program's checkers count from the loading thread's
  ids: with more checkers (the default pool, `tsc -b` projects built at once)
  Go's ids race, and per-thread counters cannot give Go's process count
  without one shared counter. The language server's programs and its search
  threads do not carry ids either. Go's ids are also fixed with
  `--checkers 1` when the programs run one at a time (`tsc -b --builders 1
  --checkers 1`, watch with `--checkers 1`), but the port does not carry
  them there: `program.rs` cannot tell that the programs run one at a time,
  and with more builders a carry would make one program wait for the
  checker of another.
- One thread can hold checkers of several programs (the language server's
  dispatch thread). Make a checker's program current while the checker runs
  (`core::enter_program`): the `program.rs` functions that checker code
  calls read `prog()`. The ids of the symbols that a checker adds are kept
  per checker arena (`SymbolArena::for_checker`, `ast::get_symbol_id`), and
  the module specifier caches per program (`modulespecifiers/host.rs`), so
  checkers of different programs do not share them.
- The Go frontend program is not thread-safe. Only the loading thread reads
  it (one frontend per program version); checker code reads the copies in
  `program::go_frontend::GoSharedState`.
- Emit (`emitter/`, `transformers/`, `printer/`, `bin/goport_emit.rs`)
  runs each file on its checker's thread with no checker borrowed
  (`program::run_on_checker_threads_for_files`); the emit resolver borrows
  the checker itself.
- Each program also has an emit pool (not in Go; `program::send_emit_pool_jobs`):
  up to 32 threads with no checker, one per core, made on the first emit
  that uses it. There is no pool when the cores are not more than the
  checkers: then it has no spare core and only slows the checker threads.
  The JS part of a file goes there when its transforms make no checker
  call (`emitter::emitter::js_emit_needs_checker`: Go's binder
  reference resolver case of `getScriptTransformers`, and no enum in the
  file). The d.ts part, and a JS part that needs the checker, stay on the
  checker thread, so each checker gets the same calls in the same order.
  The pool's emit resolver panics on every call
  (`emitter::no_checker`), so a wrong rule ends the run (exit 70) and
  cannot change an output. The binder reference resolver reads the
  program's binder symbols (`transformers::reference_resolver::BinderSymbols`),
  on every thread. A d.ts part waits for its file's JS part before it
  writes, so a file's outputs are written in Go's order. The pool is off
  with `--singleThreaded`, `--generateTrace`, an emit called on a checker
  thread and `GOPORT_EMIT_THREADS=0` (the variable sets the thread count,
  also when no core is spare).
  With `noEmit` or `emitDeclarationOnly` no JS part moves. An emit that
  moves no JS part runs as with the pool off and makes no pool. The
  language server does not emit through `program_emit`.
- While the emit pool is on, each checker thread also has a twin (not in
  Go; `program::send_dts_twin_job`): a thread with no checker that prints
  and writes the parts whose transforms ran on the checker, so the
  checker goes on with its next file. The d.ts part of a split file prints
  there. So do both parts of a file whose JS transforms call the checker
  (jstwin1, `program_emit::emit_on_twin`): the checker runs the JS
  transforms, then the declaration transforms, as before, and the twin
  prints and writes the JS part, then the d.ts part, in Go's order (map,
  JS, declaration map, d.ts). Each job brings the transformed tree, a copy
  of the synthetic nodes that it reaches (`PrintPack`) and the side
  tables of its emit context. The twin's emit host has no checker, so a
  checker call in a print ends the run (exit 70). A panic in the
  declaration transforms goes on after the twin wrote the JS part, as in
  Go. A panic in the JS print on the twin comes after the checker ran the
  declaration transforms, which Go does not run then (`emit_on_twin` says
  what differs). A forced emit and a builder signature emit stay on the
  checker. `GOPORT_DTS_TWIN=0` turns the twins off.
  `GOPORT_DTS_TWIN_CHECK=1` also prints each part on the checker, and the
  twin panics when its writes differ.
- The content mapper host is dispatch-thread state (`contentmapper`
  module docs). Each mapper connection (`contentmapper::muxconn::MuxConn`)
  reads on its own thread, as Go's `AsyncConn.Run` goroutine does, and
  answers a request from the mapper on a short thread (Go `handlers.Go`).
  Once the loader's transform of a first file of a mapper opened the
  mapper project, the parse workers send the transform requests of the
  later files of that mapper (`ConcurrentTransform`) and parse the virtual
  texts, as Go's parse goroutines do. The loader takes each result in load
  order (`take_prefetched_mapped`), so a file gets one request, and the
  ids, diagnostics and failure budget are those of a serial load. Mapped
  jobs have their own workers (`GOPORT_MAPPED_THREADS`, default the parse
  worker count), because such a job mostly waits for the mapper. The
  compiler host, the `tsc -b` project host and the watch host take part
  (`CompilerHost::prefetch_content_mapped`); the language server
  transforms on its dispatch thread. `GOPORT_MAPPED_PREFETCH=0` turns the
  worker transforms off.
- A thread that runs Go code (the work thread of a binary, the parse,
  bind, checker, emit, search and goroutine threads, the `tsc -b` config
  and build info threads, the file watcher thread and the LSP read thread)
  gets the stack size of `gostd::stack::max_stack_size`: 1 GiB, the Go
  maximum goroutine stack. A Rust stack does not grow, so it is reserved
  at the start. Under an address space or data limit (`ulimit -v`,
  `ulimit -d`), the size is 1/64 of the limit and glibc malloc gets one
  arena (`ThreadBudget::glibc_tunables`), so the threads start and the
  heap keeps room. The 1/64 share does not count the threads, so with many
  checkers a run under a low limit can still fail where Go runs (see
  `gostd/stack.rs`).
- `tsc -p` with an incremental program starts its emit with the check (not
  in Go; `incremental::Program::start_check_and_emit`). Go waits for the
  whole check, reads the global diagnostics again, then emits. Here the
  loading thread sends each checker its check job, its global diagnostics
  job and its emit jobs in that order, with no wait between them, and the
  pool jobs go out at the same time. Each checker thread runs the same jobs
  in the same order as with the waits, so each checker emits when its own
  check ends and the pool emits the JS parts during the check. All state
  that emit writes is per thread, per checker, per emit, loading thread
  only or a pure cache, except the file system: a check can probe files
  (the TS2834/TS2835 import extension suggestion, module specifiers in type
  text). `program_emit::emit_can_start_with_check` starts early only when
  no such probe can reach an output (no extensionless relative import in a
  checked file with node16 or nodenext, no program file in `outDir` or
  `declarationDir`, no `node_modules` in them, no `preserveSymlinks`, no
  `outFile`). `noEmit`, `noEmitOnError`, `--singleThreaded`, a trace and
  `GOPORT_EARLY_EMIT=0` keep Go's order. `tsc -b`, watch, the plain
  program and the goport bins do not start early. With the early start
  (and in `tsc -b`, which starts the check when it makes the program),
  `--extendedDiagnostics` "Check time" is the time that `start_check`
  spent on the affected files (`Program::take_started_check_time`) plus
  the wait for the check, less the nested declaration emit time, as in Go.
  "Emit time" is that nested emit time plus the wait for the rest of the
  emit.
- Transformers return factory (synthetic) SourceFiles. `source_file_info`
  and the printer's identifier set map one to the parsed file with the same
  path (Go `copyFrom`). `get_ecma_line_starts` caches its line map by node,
  because all synthetic nodes share one file index. Read fields that the
  transforms set with `source_file_parser_fields`.

## Release builds

- Correctness evidence (gate, bound runs, sweeps, corpus, oracle checks) uses
  plain `--release`: `scripts/run-cargo-capped.sh build --release -p ts_goport --bins`.
  Fat LTO does not change output, and it costs 7 to 20 minutes per build.
- Timing and shipped binaries use the workspace `goport` profile:
  `scripts/run-cargo-capped.sh build --profile goport -p ts_goport --bins`.
  It inherits `release` and adds `lto = "fat"` and `codegen-units = 1`.
  Other crates keep the default release settings. The binaries land in
  `<target>/goport/`, not `<target>/release/`.
- Allocator: glibc malloc is the default. `bin/goport.rs`
  `set_malloc_tunables` re-execs once with `GLIBC_TUNABLES` set; `tsgo` and
  `goport_build` have copies. `top_pad=67108864` makes each thread heap
  read-write in full when glibc makes it, so THP `always` maps it with
  2 MiB pages. Without it, glibc before 2.44 (cup2, alvin) grows the heaps
  in 4 KiB steps and every new page faults. On cup2 the settings cut
  `tsgo` wall time by 26% to 42% (perf9 round 1). `arena_max` and the
  parse and bind thread caps come from one budget (`program::ThreadBudget`),
  which each binary installs at start. `goport` and `tsgo`
  (`ThreadBudget::one_program`) have one arena for each thread that is
  alive while the checkers run: 6 in `goport`, 7 in `tsgo` (its signal
  thread). The parse mallocs most, so it runs at most 5 threads (4 workers
  and the loading thread), which fit these arenas; with 8 parse threads at
  16 cores the parse threads shared arena locks. A large program (128 or
  more root tasks, `program::note_program_load`: hono, zod, effect,
  elysia) adds up to 3 parse workers at 8 or more cores, and the budget
  has one spare arena for each (9 in `goport`, 10 in `tsgo`). The bind
  mallocs little: a large program binds on 8 threads, which share arenas.
  When the process may run on 16 or more physical cores (the CPUs of
  `Cpus_allowed_list` with SMT siblings counted once), a large program
  parses on 16 threads (15 workers) and binds on 16 threads; the arenas do
  not change.
  A program that is not large (query) binds on 4 threads when there are
  spare arenas, so its bind threads take the arenas of the ended parse
  workers and it makes no more arenas than with 6 or 7 (query at 16
  threads with 8 bind threads: 10 arenas, +4 MB). `goport_build` (about 20
  threads per program) and bins that install no budget keep
  `ThreadBudget::WIDE`: 8 parse and 8 bind threads, 16 arenas in
  `goport_build`. Each arena in use raises peak RSS. Keep query peak RSS
  under 1.15 times Go tsgo (119 MB, so 137 MB): at 16 cores `tsgo` has 129
  to 132 MB (7 parse threads with 10 arenas: 143 MB). jemalloc and
  mimalloc have the same speed as these settings on cup2 but more RSS
  (jemalloc query 143 to 153 MB), so the `jemalloc` feature stays off.
- PGO: `scripts/build-pgo.sh [out-dir]` does an instrumented build
  (`-Cprofile-generate`), trains on query, hono, zod, effect, elysia and
  about 200 corpus cases (plus `tsgo --noEmit` on the five projects and
  `goport_emit` on query and hono), merges with `llvm-profdata` and builds
  with `-Cprofile-use`. Output must stay byte-identical to the plain
  `goport` build; verify the PGO binary like any other. The gate does not
  run `tsgo`, so check it separately. The binaries (with `tsgo`) land in
  `<out-dir>/target-use/goport/`.
  Round 5 (1.95): 9% faster on query and 12 to 15% on hono, zod, effect and
  elysia, with the same peak RSS.
  Retrain when the allocator or hot code changes.
- PGO toolchain: the script sets `RUSTUP_TOOLCHAIN=1.95.0` unless it is
  already set. The system `llvm-profdata` is LLVM 22, which matches rustc
  1.95 (LLVM 22). rustc 1.93 (LLVM 21) cannot read the indexed format 13
  that LLVM 22 writes, and it only warns and builds without the profile.
  The script checks that `llvm-profdata` is not newer than rustc's LLVM and
  stops on a mismatch. For another toolchain, use
  `rustup component add llvm-tools` or set `LLVM_PROFDATA`.
- BOLT: `scripts/build-bolt.sh <bin-dir> [out-dir]` makes BOLT copies of
  `tsgo` and `goport`, normally of the PGO bins. It runs on zbook (perf
  LBR samples; perf cannot profile on cup2 or alvin): it records the five
  projects at 4 cores and 16 threads, then runs `perf2bolt`, `merge-fdata`
  and `llvm-bolt`. BOLT uses relocation mode (it also orders functions)
  when the bin has `.rela.text`; the PGO use build links with
  `--emit-relocs` for this. The script writes the BOLT bins only when their
  stdout, stderr and exit code equal the input bins on every run. BOLT
  rewrites machine code, so run the gate, the language-server batteries and
  the `tsgo` stdout check against Go tsgo on the BOLT bins. `build-pgo.sh`
  `PGO_LINK=nopie` or `static` links a non-PIE bin, like Go tsgo; BOLT
  refuses static bins. Round 3 on cup2 (PGO with `PGO_LINK=nopie`, then
  BOLT, against the plain `goport` build of the same source): query 15 to
  20% faster, hono 13 to 20%, zod 13 to 18%, effect 21 to 24%. BOLT alone
  (against the PGO bins) gave 0 to 9%.

## Style

- No `unsafe`. No new dependencies beyond these: `bumpalo` (the AST arena
  in `ast/store.rs`) and the optional `tikv-jemallocator` (feature
  `jemalloc`). Lints are relaxed crate-wide; still write clean Rust.
- Keep a `// Go: file.go:LINE funcName` comment above each ported function.
- Use `#[allow]` sparingly; do not add crate attributes.

## Language service

These rules add to the rules above for the language-service port: Go
`internal/{ls,lsp,project,format,astnav,api,fswatch,jsonrpc}`,
`cmd/tsc` (`cmd/tsgo` before pin N), and the small parts of other packages
they need. The wave plan is `target/continuation-r97-goport/ls-port/plan.md`
in the main checkout. Where this section and a rule above differ, this
section wins for these files.

The shared-shape sections of the area maps in the same directory are also
binding, except where this section says otherwise: `map-lsproto.md` section
3, `map-ls-completions.md` section 2, `map-ls-navigation.md` section 2,
`map-ls-edits.md` section 3, `map-project.md` section 4 and the U1 contract
in `map-watch-api.md`.

### Ownership

- A unit owns the files the plan lists for it. Some units own several new
  files. A few also own one existing file for one wave. Write nothing else.
- Root (the integrator) owns `lib.rs`, every `mod.rs`, the `mod` lines of
  the module-root file `program.rs`, `Cargo.toml` and `Cargo.lock`. Root
  also writes each package `prelude` (inside that package `mod.rs`).
- Nobody edits `src/prelude.rs` or `src/frontend/prelude.rs` for this
  port. Language-service packages are never glob-exported into them.
- Do not run cargo.
- The rule "skip language-service-only exported APIs" above no longer
  applies. `checker/exports.go` and `checker/services.go` are ported now.

### Modules, files and imports

- A Go package becomes the Rust module at the same path:
  `internal/ls/lsutil` -> `crate::ls::lsutil`, `internal/lsp/lsproto` ->
  `crate::lsp::lsproto`, `internal/project/dirty` ->
  `crate::project::dirty`. Go `cmd/tsc` (`cmd/tsgo` before pin N) is
  `crate::cmd::tsgo`.
- A Go file becomes one Rust file with the Go base name in snake case
  (`importTracker.go` -> `import_tracker.rs`, `box.go` -> `box_.rs`). A Go
  file split by line ranges uses `_p1`, `_p2`, ... in Go order.
- Small parts of other Go packages go in the files the plan names:
  `src/frontend/core_*.rs` (Go `internal/core`),
  `src/frontend/stringutil_ls.rs`, `src/frontend/scanner/scanner_ls.rs`,
  `src/ast/source_file_ls.rs`, `src/program/ls_program.rs`.
- Every file of a language-service package starts with exactly one glob
  import, its own package prelude, by absolute path. Examples:
  `use crate::ls::prelude::*;`, `use crate::ls::lsutil::prelude::*;`,
  `use crate::lsp::lsproto::prelude::*;` (also in generated files),
  `use crate::project::dirty::prelude::*;`. Add no other glob. Import
  anything else by explicit path.
- A package prelude re-exports the crate prelude (not in `lsproto`, see
  below), every item of the package's own files, the packages that the Go
  package imports as module names, and `Context`, `GoError`, `LspAny`.
  When two globs export the same name, root adds an explicit pick in the
  prelude. The package's own item wins, as in Go.
- Module names in the preludes: `lsproto`, `jsonrpc`, `lsutil`, `lsconv`,
  `change`, `autoimport`, `astnav`, `format`, `ls`, `project`, `dirty`,
  `logging`, `background`, `ata`, `fswatch`, `lspwatcher`, `api`, `lsp`,
  `gostd`, `locale`, `compiler` (`frontend::compiler`), `tsoptions`,
  `tspath`, `vfs`, `module`, `packagejson`, `modulespecifiers`,
  `sourcemap`, `json_ext`, `scanner_ls`, `ls_program`
  (`program::ls_program`), `ipc` (api and contentmapper preludes),
  `spanmap` (api encoder and contentmapper preludes), `ast` (contentmapper
  prelude; the package's own `Diagnostic` and `MappedDiagnosticDirective`
  win there, so write `crate::core::Diagnostic` and
  `ast::MappedDiagnosticDirective` for the AST ones).
- Call another Go package through its name, as Go does:
  `lsproto::Hover`, `lsutil::UserPreferences`,
  `astnav::get_token_at_position(file, pos)`,
  `format::format_document(ctx, file)`. Always write
  `lsutil::UserPreferences` in full; `modulespecifiers` has its own
  `UserPreferences`.
- A new file in an existing module keeps that module's header:
  `use crate::prelude::*;` (checker, printer, ast, sourcemap,
  modulespecifiers), `use crate::frontend::prelude::*;` (frontend),
  `use crate::ipc::prelude::*;` (ipc),
  `use crate::contentmapper::prelude::*;` (contentmapper), or
  `use super::*;` (children of `program`).
- `lsproto` files do not see the crate prelude. It exports `Diagnostic`,
  `FormattingOptions` and `Message`, and lsproto defines the same names.
  The lsproto prelude holds the JSON items, `json_ext`, `IndexMap`, `Cow`,
  `gostd`, `tspath` and `unported`. Inside lsproto, write
  `crate::jsonrpc::X` in full (lsproto has its own `jsonrpc` module).
- A Go package-private name that another Go file of the same package uses
  is `pub` in Rust. Never rename a Go name to avoid a clash; the prelude
  picks.

### Threads

One thread runs all language-service state: the LSP dispatch thread. It
loads programs and owns the session, projects, file systems
(`Rc<dyn Fs>`), programs, checkers and language services. It runs every
request. These types use `Rc` and `RefCell` and are not `Send`. Other
threads (stdin reader, stdout writer, progress reporter, parent watchdog,
fswatch backends and debouncers, timer wake-ups) touch only `Send` data.
Factory nodes and cached tokens are thread-local, which is correct
because every request runs on the dispatch thread.

One exception: the cross-project search (`ls/crossproject.rs`,
`ls/search_thread.rs`). Go searches each project of a references,
implementations or rename request on its own goroutine. Here the search of
each project other than the default one runs on the search thread of its
program version:
- One long-lived thread per program version. It starts from a
  `program::WorkerSeed` taken after the program is bound, makes its own
  checker, drops it after 30 s with no job, and ends when the program is
  released (`ls::release_search_thread`).
- A job gets and returns only `Send` data. The thread reads the program
  through `ls::ProgramView` (a copy of the data the search reads) and
  program files from the AST store. Other reads go to the dispatch thread.
- Session calls, the default project's search and the merge stay on the
  dispatch thread. Items commit in queue order, so the results are those
  of a serial run in Go start order.
- The search code is generic over `ProgramView`
  (`LanguageService<P = NewProgram>` holds `Rc<P>`; the search code takes
  `&P`). Keep new code on that path generic, and keep `Rc` values and
  checkers on their thread.

### Go runtime (`crate::gostd`)

| Go | Rust |
|---|---|
| `ctx context.Context` param | `ctx: &Context` (`gostd::context::Context`, `Clone + Send + Sync`) |
| `context.Background`, `WithCancel`, `WithCancelCause`, `WithTimeout`, `WithDeadline`, `WithValue`, `AfterFunc`, `Cause` | `gostd::context::{background, with_cancel, with_cancel_cause, with_timeout, with_deadline, with_value, after_func, cause}` |
| `ctx.Err()`, `ctx.Done()` | `ctx.err() -> Option<GoError>`, `ctx.done() -> Option<Done>` (`None` is Go's nil channel) |
| context key and value | `pub static KEY: ContextKey<T> = ContextKey::new("goName");`, `with_value(&ctx, &KEY, v)`, `ctx.value(&KEY) -> Option<Arc<T>>` (`T: Send + Sync + 'static`) |
| `error` | `GoError` (`gostd::errors`, `Clone + Send + Sync`) |
| `(T, error)` / `error` result | `Result<T, GoError>` / `Result<(), GoError>`; an `error` param or field that can be nil is `Option<GoError>` |
| package var `errors.New("x")` | `pub static ERR_X: LazyLock<GoError> = LazyLock::new(\|\| errors::new("x"));` |
| `fmt.Errorf("..%w..", a, b)` | `errors::errorf(text, vec![a, b])`, text built with `format!` |
| typed error value (`lsproto.ErrorCode`, a struct) | `errors::from_value(v)` |
| `errors.Is`, `errors.As` / `AsType[T]`, `errors.Join` | `errors::is(&err, &target)`, `errors::as_type::<T>(&err)`, `errors::join(errs)` |
| `io.EOF`, `context.Canceled`, `context.DeadlineExceeded` | `errors::EOF`, `context::CANCELED`, `context::DEADLINE_EXCEEDED` |
| `err.Error()` | `err.error()` |
| `go f()` that touches dispatch-thread state | `gostd::local::go(Box::new(f))`: FIFO on the dispatch thread, run by `local::run_pending()` |
| `go f()` over `Send` data only | `std::thread::spawn` |
| `go f()` that waits for other-thread work (a child process, a channel), then touches dispatch-thread state | the wait on a `std::thread::spawn` thread that calls `post()` on the handle of `gostd::local::post_later(Box::new(rest))` when it ends; `rest` then runs in `local::run_pending()`, with no poll before (ATA npm) |
| `sync.WaitGroup`, `wg.Go`, `core.WorkGroup`, `errgroup` over dispatch-thread state | serial, in Go start order, like Go's single-threaded `WorkGroup`; keep the `ctx.err()` checks (the cross-project search is the one exception, see "Threads") |
| `errgroup.WithContext` over `Send` loops | `gostd::errgroup` (real threads) |
| `chan T` with capacity n / unbuffered | `std::sync::mpsc::sync_channel(n)` / `sync_channel(0)`; `select` with `default` is `try_send` / `try_recv`; `select` on `ctx.Done()` is a `recv_timeout` loop that checks `ctx.err()` (PORT note) |
| `sync.Mutex`, `RWMutex`, `atomic.*` on dispatch-thread data | a plain field, `Cell` or `RefCell` (drop the lock) |
| the same on cross-thread data | `std::sync::{Mutex, RwLock, atomic}` |
| `sync.Once`, `OnceValue`, `OnceFunc` | `OnceCell`, `OnceLock`, `LazyLock`, or a `Cell<bool>` guard |
| `time.AfterFunc(d, f)` that touches dispatch-thread state | `gostd::local::after_func(d, Box::new(f)) -> LocalTimer` (`stop() -> bool`, `reset(d)`); `f` runs on the dispatch thread |
| `time.Timer`, `Ticker`, `AfterFunc` over `Send` data | `gostd::timer::{Timer, Ticker, after_func}` |
| `time.Now`, `time.Since`, `time.Duration` | `std::time::{Instant, SystemTime, Duration}` |
| `defer f()` | a guard, or an explicit call on every return path |
| `recover()` | `std::panic::catch_unwind(AssertUnwindSafe(..))`; `unported!` panics are recovered like Go panics |
| `panic(x)` | `panic!` with the Go text |
| `slices.SortFunc`, `sort.Sort`, `sort.Slice` (not stable) | `gostd::slices::sort_func(&mut v, cmp)`, `gostd::slices::sort_slice(&mut v, less)` (Go pdqsort: equal elements end where Go puts them) |
| `slices.SortStableFunc`, `sort.Stable`, `sort.SliceStable` | `gostd::slices::sort_stable_func(&mut v, cmp)` |
| `slices.Sort`, `sort.Strings` (ordered values) | std `v.sort()` (equal values are the same value) |
| any sort with a comparator | never std `sort_by`, `sort_by_key` or `sort_unstable_by`: they can panic when the comparator is not a total order, and Go's sorts do not. The `gostd::slices` sorts give Go's order for any comparator. A port-only sort on an `Ord` key can use std. |
| `slices.BinarySearchFunc` | `gostd::slices::binary_search_func(&v, target, cmp) -> (usize, bool)` |
| `strconv.Quote`, `%q` | `gostd::strconv::quote(s)` |
| `net/url` (`Parse`, `PathEscape`, `QueryEscape`, `PathUnescape`), `net/netip.ParseAddr` | `gostd::url::{parse, path_escape, query_escape, path_unescape}`, `gostd::netip` |
| `regexp` with `\p{..}` classes, `unicode` range tables | `gostd::regexp`, `gostd::unicode_tables` (go1.27.1, Unicode 17.0.0, generated by `scripts/goport/gen/unicode/gen.sh`) |
| x/text `collate`, `unicode/norm` (organize imports) | `gostd::collate`, `gostd::norm` (x/text v0.42.0; tables in `gostd/data/`, generated: norm by `scripts/goport/gen/unicode/gen.sh`); `language.Compose`, `Parent`, `TypeForKey`, compact tags in `locale.rs` |
| `signal.NotifyContext(ctx, os.Interrupt, syscall.SIGTERM)` | `cmd::tsgo::main::notify_context` (`signal-hook`) |
| `panic(x)` that the pinned Go reaches on the same input and nothing recovers | `core::go_panic(text)`: the bins print `panic: <text>` and exit 2 |
| `%v`, `%+v`, `%T` in log text | `format!("{:?}", x)` with a PORT note (log text is not compared) |
| Go map iteration that reaches output | `IndexMap` in insertion order and `// PORT: Go map order is random`; the oracle compares that output without order |
| `collections.OrderedMap` | `IndexMap` (`Delete` is `shift_remove`) |
| `collections.SyncMap`, `SyncSet`, `Set`, `MultiMap` | `RefCell<FxHashMap>`, `FxHashSet`, `IndexMap<K, Vec<V>>` |
| `core.IfElse(c, a, b)` | `if c { a } else { b }`; evaluate both first only if an argument has a side effect (Go evaluates both) |
| `core.Filter`, `Map`, `Find`, `Some`, `Every`, `FlatMap`, `FirstOrNil`, ... | iterator code with the same order and the same nil-versus-empty result |
| `diagnostics.X.Localize(loc, args...)`, `locale.FromContext(ctx)` | `diagnostics_loc::message_localize(diag::X, &loc, &args![..])`, `locale::from_context(ctx)`; `Locale` is the Go `language.Tag` |
| `stringutil.Compare*`, `EquateStringCaseInsensitive`, `TruncateByRunes` | `crate::frontend::stringutil_ls` |
| `stringutil.IsLineBreak`, `IsWhiteSpace*`, `StripQuotes` | the existing `scanner_util` names |
| Go `string` indexes and positions | byte offsets (`as_bytes()`); decode runes with the `pub(crate)` `utf8_decode_rune_in_string` / `utf8_decode_last_rune_in_string` in `frontend/scanner/scanner_p1.rs`; never slice a `&str` inside a character |

Dispatch loop contract (lsp server and session): the server calls
`gostd::local::set_waker(f)` once. A due `LocalTimer` calls the waker from
its timer thread. The dispatch loop calls `local::run_pending()` after
each message and after each wake-up. Go `WaitForBackgroundTasks` runs
`local::run_pending()` until the queue is empty.

### Programs and checkers

- Go `*compiler.Program` in ls, project and api code is
  `Rc<compiler::NewProgram>` where it is stored (a project, a checker
  pool, a language service, the `ls_program` registry, `FRONTENDS`) and
  `&compiler::NewProgram` where code only reads it. Holding the `Rc`
  keeps the program alive, as a Go pointer does. Pointer equality is
  `Rc::ptr_eq` (`std::ptr::eq` for two borrows). A map key is the
  address only while the map's owner holds the `Rc`
  (`ls_program` registry, `programCounter`); a per-thread cache that can
  outlive the program uses the program version id
  (`ProgramView::identity`).
- Go `compiler.NewProgram(opts)` is `ls_program::new_program(opts,
  create_checker_pool)`; `p.UpdateProgram(..)` is
  `ls_program::update_program(p, ..)`. Go `ProgramOptions.CreateCheckerPool`
  is the extra `create_checker_pool` argument (PORT).
- Every program made by `ls_program::new_program` or
  `ls_program::update_program` is a program version of the process
  (`program::new_program_version`), as Go makes a new `Program` for each
  snapshot change. Versions share the file versions they have in common.
  The checkers of every version are made on the dispatch thread, except
  the checkers of the cross-project search threads (see "Threads").
- Current program: checker code reads `prog()`. `ls_program::enter(p)`
  makes `p` current while its `ProgramGuard` lives; the last guard that is
  still alive wins, so guards can drop in any order. A language service
  holds a guard for its program, the `get_type_checker*` functions return
  a `Release` that holds one, and the `ls_program` diagnostics functions
  enter `p`. A caller that uses several language services in turn calls
  `ls.enter_program()` for each. A checker from a pool of its own gets its
  guard with `ProgramGuard::with_release`.
- Release: `ls_program::release_program(p)` is Go's program drop (the
  snapshot `programCounter.Deref`). It frees the checker pools of `p`, its
  program version and the synthetic nodes that the dispatch thread made
  while the version was current, now or when the last guard of `p` drops.
  The version's tables go with it (see "Program"). The registry drops its
  `Rc` of `p`, so the `NewProgram` is freed with its last holder. The
  `GoProgram` shell and the static file versions stay leaked; a freeable
  file version goes with its last holder (see "Program"). A compiler
  host drops its data
  (`CompilerHost::release`, not in Go) when no live program uses it. A
  program uses its own host and the host of the load that made its files:
  a clone shares the old program's processed files (Go `UpdateProgram`),
  whose resolver reads that load's host. When that load host has no live
  program, the load's resolver caches go too
  (`NewProgram::release_resolver_caches`).
- When the release frees memory (not in Go: the GC frees it in the
  background): on the dispatch thread the program tables and the frontend
  program wait for `gostd::local::drop_garbage` after the answer. The
  checkers of a released pool and the version's synthetic chunks wait in
  the same way only for the first release after a client pause of 20 ms or
  more, and only when the client has not sent its next edit yet (no
  notification waits) and nothing from an earlier message waits
  (`gostd::local::drop_after_pause`). So only the releases of one message
  wait, and the check after a pause holds the old checkers too (hono HWM
  +31 to +39 MiB in paced typing and errfix rounds, up to about +71 MiB in
  paced mix and long rounds). On an API connection they are the releases
  of a pipelined burst, which wait until its inbox is empty
  (`ApiConnProtocol`). Every other release frees them at once, because
  the next check then reuses their memory (freecheck1, freecheck2). A
  synthetic entry leaves its table at the release in both cases, so a read
  of it panics at once. Other threads free at once.
- Parse workers (`CompilerHost::prefetch_parses`, not in Go) run only for
  the first program load of a project. A later load gets its files from
  the parse cache.
- Go `*ast.SourceFile` is `Node` (the file root). An `Rc<ParsedSourceFile>`
  from a `NewProgram` method becomes `file.root`.
- Go `p.X(..)` on a program: call `NewProgram::x` when it exists; else
  `ls_program::x(p, ..)` when the plan lists it (checker and diagnostics
  methods); else the `program::x` free function (it reads the current
  program); else `unported!("Program.X")`.
- `c, done := p.GetTypeCheckerForFile(ctx, file); defer done()` becomes
  `let (checker, done) = ls_program::get_type_checker_for_file(p, ctx, file);`
  and `let c = &mut *checker.borrow_mut();`. Keep `done` (a `Release`
  guard) alive to the end of the scope; it releases once, on drop or on
  `done.call()`. Helper functions take `c: &mut Checker`, never the `Rc`.
  Never borrow the checker twice.
- Go `compiler.CheckerPool` is the trait `ls_program::CheckerPool`
  (`get_checker(&self, ctx: &Context, file: Node) -> (Rc<RefCell<Checker>>, Release)`,
  `file` may be `Node::NIL`). The project pool implements it. Go
  `checker.NewChecker(program)` is `ls_program::new_checker(program)`.
- Checker API names: `checker-api-tools/names_exports_services.tsv`
  (the new `checker/exports.rs` and `checker/services.rs`) and
  `checker-api-tools/ported_ls_names.tsv` (existing ports), both next to
  the maps. When Go calls an exported wrapper whose unexported twin also
  exists, the Rust name ends in `_exported`. Never call the twin in its
  place.
- Node builder `idToSymbol`: Go shares one map between the builder and its
  caller. Build with `new_node_builder_ex(c, ec, Some(FxHashMap::default()))`
  and read the filled map back from `nb.impl_.borrow().id_to_symbol`. For
  a printer, move the map into `Printer.id_to_symbol`. Add a PORT note.
- A file that the language server parses outside a program load (the
  parse cache, `getOrParseSourceFile` in sourcedefinition.go) is recorded
  with `program::note_parsed_source_file`, so the next publish gives it
  its parser fields. To read it before a program includes it, publish it
  with `program::publish_parsed_files` and bind it with
  `program::bind_file_outside_program` (Go `BindSourceFile`).
- The autoimport `aliasResolver` (a checker over node_modules files that no
  program holds) makes its own program version
  (`program::new_alias_resolver_program`). Before it makes the checker, it
  reads every file that a string module name in its files can resolve to
  (imports, exports, `require`, import types, JSDoc imports, relative names
  inside `declare module "x" {}`), so the checker arena holds them. It can
  read more files than Go; it never reads fewer. A file that this walk
  misses reaches `unported!("aliasResolver.GetSourceFile after NewChecker")`.

### Scanning

- All code scans with the Go scanner `frontend::scanner::Scanner`.
- Go `scanner.GetScannerForSourceFile(f, pos)` is
  `scanner_ls::get_scanner_for_source_file(f, pos)` and
  `scanner.GetECMAPositionOfLineAndByteOffset` is
  `scanner_ls::get_ecma_position_of_line_and_byte_offset`. Other Go
  `scanner.X` free functions (`SkipTrivia`, `GetTokenPosOfNode`,
  `TokenToString`, ...) are the existing `scanner_util` names.

### Protocol types (`lsproto`)

- `src/lsp/lsproto/lsp_generated/*.rs` is generated by
  `src/lsp/lsproto/_generate/generate.mts` (a port of Go's generator) from
  the pinned `metaModel.json`. Never edit the output by hand. Change the
  generator, then run `node --experimental-strip-types generate.mts`.
- Names and shapes follow `map-lsproto.md` section 3. Short form: type
  names keep the Go spelling (`HoverParams`, `URI`, `DocumentUri`); fields
  are snake case (`text_document`, `type_`); `*T` is `Option<T>`
  (`Option<Box<T>>` only on a type cycle); `*[]T` is `Option<Vec<T>>`;
  `[]T` and `[]*T` are `Vec<T>`, except that `[]*T` (also as a map value,
  `WorkspaceEdit.changes`) is `Vec<Option<T>>`
  in a type that the server decodes (client-to-server params,
  server-to-client results, and the types they hold), so a JSON null
  element is Go's nil element and a nil element encodes as null (the
  generator's `decodedTypes`). Code that builds such a list wraps each
  element in `Some`; `map[K]V` is `IndexMap<K, V>`; LSPAny is
  `LspAny`; a string enum is `pub struct MarkupKind(pub Cow<'static, str>)`
  with consts such as `MarkupKind::PLAIN_TEXT`; an int enum is
  `pub struct CompletionItemKind(pub i32)` with consts; method consts are
  `Method::TEXT_DOCUMENT_HOVER`; `TextDocumentHoverInfo` is
  `TEXT_DOCUMENT_HOVER_INFO: RequestInfo<HoverParams, TextDocumentHoverResponse>`;
  a union is a struct of `Option` fields (all `None` is null); a literal is
  a unit struct; `*Response` aliases are `pub type`; `(v *X) resolve()` is
  `X::resolve(v: Option<&X>)`.
- Go `any` in `RequestMessage.Params` and `ResponseMessage.Result` is
  `Box<dyn AnyValue>`. Read it with `downcast_ref::<HoverParams>()`.
- Client capabilities in a context:
  `lsproto::with_client_capabilities(&ctx, caps) -> Context` and
  `lsproto::get_client_capabilities(&ctx) -> Arc<ResolvedClientCapabilities>`.
- `lsproto::ErrorCode` is also a `GoError` value
  (`errors::from_value(code)`), so `errors::as_type::<lsproto::ErrorCode>`
  finds it in a wrap chain.

### JSON

- All JSON uses `frontend/json.rs` (`MarshalerTo`, `UnmarshalerFrom`,
  `JsonDecoder`) and `frontend/json_ext.rs` (the Go JSON v2 default
  arshalers for integers, floats, `Option`, `Box`, `Vec`, `[u32; 2]`,
  `IndexMap`, `LspAny`, `JsonValue`, one-line field helpers, and
  `marshal_indent`). No serde.
- A hand-written Go struct with `json:"..."` tags (Go marshals it by
  reflection) gets a hand-written `MarshalerTo` and `UnmarshalerFrom` with
  the v2 default rules: fields in declaration order; `omitzero` skips the
  zero value; `omitempty` skips `null`, `""`, `[]` and `{}`; a non-omit
  `None` writes `null`; a non-omit nil slice or map writes `[]` or `{}`;
  input names match exactly and unknown names are skipped; `null` input
  sets the zero value.
- Go `json.Value` is `JsonValue(Vec<u8>)`. Go `map[string]any` is
  `IndexMap<String, LspAny>`.
- A `float64` field writes the ES6 number text (as v2). An integer field
  rejects `1.0` and `1e2` (as v2).
- `null`, `[]` and a missing key are different values. Maps write keys in
  insertion order. Go writes random order unless it sets
  `json.Deterministic(true)`; then sort the keys as Go does.

### `goport --lsp`

- `bin/goport.rs` sends `--lsp` and `--api` to `cmd::tsgo::run_main` before
  anything is written to stdout.
- A request that reaches `unported!` gets a `-32603` error response through
  the server's `recover`. At exit, goport returns the Go status (0 or 1)
  unless unported code ran; then it prints the unported report on stderr
  and exits 70 (`EXIT_UNPORTED`).

### Not ported (plan level)

- Go's FSEvents FFI (macOS). The file watchers are ported with safe crates
  (D-W1, no `libc`, no `unsafe`): inotify and fanotify
  (`src/fswatch/{inotify,fanotify}_linux.rs` on the `unix.rs` shim:
  `nix::sys::fanotify`, `name-to-handle-at`, `rustix`, `std`), kqueue
  (`kqueue.rs` on `unix_bsd.rs`), FSEvents (`fsevents_darwin.rs` on the
  `notify` crate's `FsEventWatcher`) and Windows (`windows.rs` on the
  `notify` crate). `fsevents_darwin.rs` lists what `notify` changes: its
  stream flags and latency, no event IDs (the event list's own sequence),
  one event per flag (every remove and rename checks with lstat) and no
  stream for a root that is gone. The server makes the in-process LSP
  watcher (`lsp/lspwatcher`) only when the default watcher has a fast
  recursive backend: on macOS and Windows, never on Linux (as in Go).
- Native path folding in fswatch (ts#64210, `pathcompare.rs`). Go ignores
  case only for fsevents and kqueue watches on a darwin volume that
  `pathconf` reports as case-insensitive, and folds with CoreFoundation.
  The port has no safe `pathconf` and no CoreFoundation, so its path
  comparer compares bytes on every target, as Go does on Linux.
- One dispatch thread (see "Threads"): the server answers requests in
  arrival order, where Go runs the async part of a request on a goroutine
  and answers in finish order. Timers and background tasks run at message
  boundaries. Without an API session, the results of LSP requests are
  Go's and only order and timing differ. With an API session, the limits
  below also change which messages are answered and when.
- The end of a run: when SIGINT, SIGTERM, the parent watchdog or the end
  of stdin ends the context while the dispatch thread runs work that Go
  runs on a goroutine (the async part of a request, an API session), Go's
  `Run` (`lsp/server.go:859`) returns without that work and the process
  ends at once. The port waits until that work ends, then ends with Go's
  exit code and message. A request that the end cancels can also log
  "error handling method" on stderr. Go and the port both wait for the
  sync part of a handler (`server.go:1013`).
  - A fix ends the run from a watcher thread while the dispatch thread is
    in Go's goroutine work. So it tracks the phase of Go's dispatch
    goroutine: in `requestQueue.Get`, in the sync part of a handler, or in
    goroutine work.
  - Every dispatch level must set the phase of its own turn, and give the
    outer phase back when it returns. This includes the inner
    `dispatch_next` that an API connection's read runs while it waits, and
    the `dispatch_request` that it runs while an API request waits for a
    client call (`ApiConnProtocol::read_message`).
  - The apisig1 lane (branch `goport-apisig1`, `lsp/run_end.rs`) set the
    phase only at the outermost level. With an API session connection
    open, the sync part of an LSP message ran in `Work`, so a signal or
    the end of stdin ended the run at once, where Go waits.
- API sessions of the LSP server (`custom/initializeAPISession`) are
  served on the dispatch thread too (`lsp/server.rs` `ApiConnProtocol`).
  LSP messages and API requests do not run at the same time. These
  limits are more than order and timing:
  - While an API request waits for a client callback, the server serves
    `didChange`, `didClose`, `didSave` and `$/setTrace` in arrival order.
    Other LSP messages, and all messages after the first of them, wait
    until the client answers. `shutdown` and `exit` are served wherever
    they are in the queue, so the server still stops; a message that
    waits behind them is answered later, or not at all after `exit`. Go
    answers the waiting messages, except those that need the snapshot
    that an API request builds for the session.
  - A second connected API session holds the first until it closes. A
    client that waits for the first before it closes the second
    deadlocks.
  - A client callback from an LSP message served while an API connection
    waits returns an error. Go makes the call.
  - `--api --async` and the API sessions run requests one at a time
    (`ipc/conn_async.rs`). A pipelined request sees the result of the one
    before it. Go runs them at the same time.
  - The async connection reads its next message only after the running
    request. So the end of the input during a request does not cancel the
    request's context: Go's read loop sees the end at once and returns
    (`ipc/conn_async.go:89-91`), and its deferred `cancelHandlers` (`:73`)
    cancels it, so a long check answers early with what it has. The port
    answers in full and then ends, with Go's exit code.
  - The end on SIGINT or SIGTERM in `--api --async` is an open Go
    difference (state note `r187-repair-withdraw-2026-10-09`, lane
    apisig4). Go's read loop checks the context at the top of each turn
    (`ipc/conn_async.go:83`) and then waits in the read while a request
    runs on its own goroutine (`:98`). The port runs the request inline
    and checks the context after it (`run_loop`). The cases:
    - A signal during a request: Go answers it, then ends after the next
      message (it answers that message too) or at the end of the input.
      The port ends right after the answer and reads no more.
    - Pipelined requests: a signal during request 1 while request 2 is
      already sent. Go answers both (2 first) and then waits for the next
      message. The port answers request 1 and ends: request 2 gets no
      answer.
    - A signal while a request waits for a client callback: Go's `Call`
      returns the context error at once (`:290`), so request 1 is answered
      at the signal. The port's `call` waits in its read, so it answers
      request 1 after the next message. Both end after that message.
  - apisig2 (R187) moved the run's check before an inline request to
    match the first case. The run then read until the end of the input in
    the callback case. Its repair apisig3 closed the pending calls at the
    signal, so the callbacks that a handler makes after the signal
    (writeFile, removeFile) lost their requests. Both are withdrawn, and
    `ipc/conn_async.rs` is R186's code.
- Go runtime profiles (pprof) have no samples: the port writes Go's file
  names, errors and log lines and valid empty profiles. `runtime.GC` is a
  no-op. `runtime/metrics` reads as `KindBad`, so the Go runtime fields of
  performance telemetry are 0.
- API handles across checkers: a type, signature or checker-made symbol of
  one project sent with another project of the same snapshot stays
  `unported!` (a Rust id indexes one checker's arena).
- Windows named pipes (`--api --pipe` on Windows) use miow's safe
  `NamedPipe` (D-W1), not winio's overlapped I/O: `Close` does not end an
  `Accept` that waits on another thread (see `ipc/transport.rs`).
