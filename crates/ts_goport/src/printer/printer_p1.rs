//! Go `printer/printer.go` lines 1 to 1344: the `Printer` struct, options,
//! print handlers, low-level writing, line terminator counts, custom emit
//! behavior checks, tokens, literals and names.
//!
//! Other parts (printer_p2 to printer_p5) add more `impl Printer` blocks.

use crate::flags_macros::go_enum;
use crate::gostd::debug::kind_string;
use crate::prelude::*;
use std::borrow::Cow;
use std::cell::{Cell, RefMut};

// Go: printer/printer.go:35 PrinterOptions
#[derive(Clone, Debug, Default)]
pub struct PrinterOptions {
    pub remove_comments: bool,
    pub new_line: NewLineKind,
    pub omit_trailing_semicolon: bool,
    pub no_emit_helpers: bool,
    // Module                        core.ModuleKind
    // ModuleResolution              core.ModuleResolutionKind
    pub target: ScriptTarget,
    pub source_map: bool,
    pub inline_source_map: bool,
    pub inline_sources: bool,
    pub omit_brace_source_map_positions: bool,
    // ExtendedDiagnostics           bool
    pub only_print_js_doc_style: bool,
    pub never_ascii_escape: bool,
    // StripInternal                 bool
    pub preserve_source_newlines: bool,
    pub terminate_unterminated_literals: bool, // !!!
}

// Go: printer/printer.go:55 PrintHandlers
// PORT: Go func fields become `Option<Rc<dyn Fn>>`. The commented-out `!!!`
// hooks in Go are not ported.
#[derive(Clone, Default)]
pub struct PrintHandlers {
    /// A hook used by the Printer when generating unique names to avoid collisions with
    /// globally defined names that exist outside of the current source file.
    pub has_global_name: Option<Rc<dyn Fn(&str) -> bool>>,
    /// MapSourcePosition composes source-map positions before they reach the generator.
    /// Returning ok=false emits a generated-only mapping.
    // PORT: Go takes and returns a `sourcemap.Source`. The printer's source is
    // a source file node when `emit_pos` calls the hook, so the hook gets that
    // node. It returns `None` for Go `ok == false`, `Some((None, pos))` when Go
    // returns the same source, and `Some((Some(source), pos))` when Go returns
    // another source.
    pub map_source_position: Option<
        Rc<dyn Fn(Node, i32) -> Option<(Option<Rc<dyn crate::sourcemap::source::Source>>, i32)>>,
    >,

    pub on_before_emit_node: Option<Rc<dyn Fn(Node)>>,
    pub on_after_emit_node: Option<Rc<dyn Fn(Node)>>,
    pub on_before_emit_node_list: Option<Rc<dyn Fn(NodeList)>>,
    pub on_after_emit_node_list: Option<Rc<dyn Fn(NodeList)>>,
    pub on_before_emit_token: Option<Rc<dyn Fn(Node)>>,
    pub on_after_emit_token: Option<Rc<dyn Fn(Node)>>,
}

// Go: sourcemap/source.go:5 Source
/// Go `sourcemap.Source` as the printer holds it. Go `*ast.SourceFile`
/// implements the interface, and the port keeps that source file as its
/// `Node` (nil is `SourceMapSource::NIL`). ts#63936:
/// `PrintHandlers.map_source_position` can return a source that is not a
/// node (Go `compiler.declarationMapSource` in `emitter.rs`).
#[derive(Clone)]
pub enum SourceMapSource {
    Node(Node),
    Other(Rc<dyn crate::sourcemap::source::Source>),
}

impl SourceMapSource {
    pub const NIL: SourceMapSource = SourceMapSource::Node(Node::NIL);

    /// Go `source == nil`.
    pub fn is_nil(&self) -> bool {
        matches!(self, SourceMapSource::Node(node) if node.is_nil())
    }

    /// Go `source != nil`.
    pub fn is_some(&self) -> bool {
        !self.is_nil()
    }

    /// Go `source.FileName()`.
    pub fn file_name(&self) -> &str {
        match self {
            SourceMapSource::Node(node) => source_file_file_name(*node),
            SourceMapSource::Other(source) => source.file_name(),
        }
    }

    /// Go `source.Text()`.
    // PORT: the text of a freeable file version is not `'static`
    // (`FileText`, textleak1), so it is copied. Only `inlineSources` reads
    // it.
    pub fn text(&self) -> Cow<'_, str> {
        match self {
            SourceMapSource::Node(node) => match source_file_text(*node) {
                FileText::Static(text) => Cow::Borrowed(text),
                text @ FileText::Shared(_) => Cow::Owned(text.to_string()),
            },
            SourceMapSource::Other(source) => Cow::Borrowed(source.text()),
        }
    }
}

/// Go interface equality: the same source file node, or the same pointer.
impl PartialEq for SourceMapSource {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (SourceMapSource::Node(a), SourceMapSource::Node(b)) => a == b,
            (SourceMapSource::Other(a), SourceMapSource::Other(b)) => {
                std::ptr::addr_eq(Rc::as_ptr(a), Rc::as_ptr(b))
            }
            _ => false,
        }
    }
}

/// Go `*sourcemap.Generator`.
pub use crate::sourcemap::generator::Generator as SourceMapGenerator;
/// Go `sourcemap.SourceIndex`.
pub use crate::sourcemap::generator::SourceIndex;

/// Go `file.ScriptKind` of a parsed or factory SourceFile.
// PORT: the printer prints transformed SourceFiles. Those are factory
// (synthetic) nodes, and `source_file_info` only reads parsed files.
pub(crate) fn source_file_script_kind(file: Node) -> ScriptKind {
    if is_synthetic_node(file) {
        return with_synthetic_source_file(file, |d| d.script_kind);
    }
    source_file_info(file).script_kind
}

/// Go `file.IsDeclarationFile` of a parsed or factory SourceFile.
// PORT: see `source_file_script_kind`.
pub(crate) fn source_file_is_declaration_file(file: Node) -> bool {
    if is_synthetic_node(file) {
        return with_synthetic_source_file(file, |d| d.is_declaration_file);
    }
    source_file_info(file).is_declaration_file
}

// Go: printer/printer.go:117 Printer
pub struct Printer {
    // PORT: Go embeds `PrintHandlers`. Embedding becomes nesting.
    pub print_handlers: PrintHandlers,
    pub options: PrinterOptions,
    pub(crate) emit_context: Rc<EmitContext>,
    pub(crate) current_source_file: Node,
    // PORT: a nil Go map is `None`. Go checks `uniqueHelperNames != nil`.
    pub(crate) unique_helper_names: Option<FxHashMap<String, Node>>,
    pub(crate) external_helpers_module_name: Node,
    pub(crate) next_list_element_pos: i32,
    // PORT: Go `EmitTextWriter` is an interface value shared with the caller.
    // It becomes a shared `Rc<RefCell<dyn EmitTextWriter>>`; nil is `None`.
    // Use `self.writer()` to borrow it for one call.
    pub(crate) writer: Option<Rc<RefCell<dyn EmitTextWriter>>>,
    pub(crate) own_writer: Option<Rc<RefCell<dyn EmitTextWriter>>>,
    pub(crate) write_kind: WriteKind,
    pub(crate) source_maps_disabled: bool,
    pub(crate) source_map_generator: Option<Rc<RefCell<SourceMapGenerator>>>,
    // PORT: Go `sourcemap.Source` is an interface; see `SourceMapSource`.
    pub(crate) source_map_source: SourceMapSource,
    pub(crate) source_map_source_index: SourceIndex,
    pub(crate) source_map_source_is_json: bool,
    // PORT: Go `*lineCharacterCache`; nil is `None`.
    pub(crate) source_map_line_char_cache: Option<LineCharacterCache>,
    pub(crate) most_recent_source_map_source: SourceMapSource,
    pub(crate) most_recent_source_map_source_index: SourceIndex,
    pub(crate) container_pos: i32,
    pub(crate) container_end: i32,
    pub(crate) declaration_list_container_end: i32,
    // PORT: Go `core.Stack[detachedCommentsInfo]` is a `Vec` used as a stack.
    pub(crate) detached_comments_info: Vec<DetachedCommentsInfo>,
    pub(crate) comments_disabled: bool,
    pub(crate) in_extends: bool, // whether we are emitting the `extends` clause of a ConditionalTypeNode or InferTypeNode
    pub(crate) name_generator: NameGenerator,
    // PORT: the name generator callbacks in Go read `p.currentSourceFile`
    // through the captured Printer. The Rust callbacks capture this cell
    // instead. `sync_name_generator` copies `current_source_file` into it
    // before each call into the name generator.
    pub(crate) name_generator_source_file: Rc<Cell<Node>>,
    // PORT: Go `makeFileLevelOptimisticUniqueName func(string) string` is a
    // closure over the Printer. Rust cannot store a closure that borrows its
    // owner, so it is the method `make_file_level_optimistic_unique_name`.
    // PORT: Go `commentStateArena` and `sourceMapStateArena` only allocate
    // states. `PrinterState` holds the states by value, so no arena is kept.
    // PORT: a nil Go map is `None`.
    pub id_to_symbol: Option<FxHashMap<Node, SymbolId>>,
    // PERF: one-entry caches keyed by `current_source_file`. See
    // `current_source_file_original` and `current_line_map`.
    // `set_source_file` clears them.
    pub(crate) current_original_cache: Cell<(Node, Node)>,
    pub(crate) current_line_map_cache: Cell<(Node, &'static [i32])>,
    pub(crate) current_text_cache: Cell<(Node, &'static str)>,
    pub(crate) skip_trivia_memo: SkipTriviaMemo,
}

/// One-entry memo of `skip_trivia(source_file_text(file), pos)`.
// PERF: nested nodes often start at the same pos. The source map pos of each
// node and the text of a leaf identifier skip the same trivia.
#[derive(Default)]
pub(crate) struct SkipTriviaMemo(Cell<(Node, i32, i32)>);

impl SkipTriviaMemo {
    pub(crate) fn skip_trivia(&self, file: Node, pos: i32) -> i32 {
        let (memo_file, memo_pos, memo_result) = self.0.get();
        if memo_file == file && memo_pos == pos && file.is_some() {
            return memo_result;
        }
        let result = skip_trivia(&source_file_text(file), pos);
        self.0.set((file, pos, result));
        result
    }
}

// Go: printer/printer.go:149 detachedCommentsInfo
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct DetachedCommentsInfo {
    pub(crate) node_pos: i32,
    pub(crate) detached_comment_end_pos: i32,
}

// Go: printer/printer.go:154 commentState
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct CommentState {
    pub(crate) emit_flags: EmitFlags, // holds the emit flags for the current node
    pub(crate) comment_range: TextRange, // holds the comment range calculated for the current node
    pub(crate) container_pos: i32, // captures the value of containerPos prior to entering an node
    pub(crate) container_end: i32, // captures the value of containerEnd prior to entering an node
    pub(crate) declaration_list_container_end: i32, // captures the value of declarationListContainerEnd prior to entering an node
}

// Go: printer/printer.go:162 sourceMapState
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct SourceMapState {
    pub(crate) emit_flags: EmitFlags, // holds the emit flags for the current node
    pub(crate) source_map_range: TextRange, // holds the source map range calculated for the current node
    pub(crate) has_token_source_map_range: bool, // captures whether the source map range was set for the current node
}

// Go: printer/printer.go:168 printerState
// PORT: Go holds arena pointers that may be nil. Rust holds the states by
// value; a nil pointer is `None`.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct PrinterState {
    pub(crate) comment_state: Option<CommentState>,
    pub(crate) source_map_state: Option<SourceMapState>,
}

// PORT: Go `ast.CommentRange` (ast/ast.go). No port of it exists yet, so it
// is defined here. Move it to the ast module when one lands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CommentRange {
    pub text_range: TextRange,
    pub kind: SyntaxKind,
    pub has_trailing_new_line: bool,
}

impl CommentRange {
    pub fn pos(&self) -> i32 {
        self.text_range.pos()
    }

    pub fn end(&self) -> i32 {
        self.text_range.end()
    }

    pub fn len(&self) -> i32 {
        self.text_range.len()
    }
}

// Go: printer/printer.go:173 NewPrinter
pub fn new_printer(
    options: PrinterOptions,
    handlers: PrintHandlers,
    emit_context: Option<Rc<EmitContext>>,
) -> Printer {
    // PORT: Go assigns the fields, then fills a nil emitContext. The Rust
    // struct needs the context at construction, so the nil check runs first.
    // wire up name generator
    let emit_context = match emit_context {
        Some(emit_context) => emit_context,
        None => new_emit_context(),
    };
    let comments_disabled = options.remove_comments;
    let mut printer = Printer {
        print_handlers: handlers,
        options,
        emit_context: Rc::clone(&emit_context),
        current_source_file: Node::NIL,
        unique_helper_names: None,
        external_helpers_module_name: Node::NIL,
        next_list_element_pos: 0,
        writer: None,
        own_writer: None,
        write_kind: WriteKind::NONE,
        source_maps_disabled: false,
        source_map_generator: None,
        source_map_source: SourceMapSource::NIL,
        source_map_source_index: 0,
        source_map_source_is_json: false,
        source_map_line_char_cache: None,
        most_recent_source_map_source: SourceMapSource::NIL,
        most_recent_source_map_source_index: 0,
        container_pos: 0,
        container_end: 0,
        declaration_list_container_end: 0,
        detached_comments_info: Vec::new(),
        comments_disabled: false,
        in_extends: false,
        name_generator: NameGenerator::default(),
        name_generator_source_file: Rc::new(Cell::new(Node::NIL)),
        id_to_symbol: None,
        current_original_cache: Cell::default(),
        current_line_map_cache: Cell::default(),
        current_text_cache: Cell::default(),
        skip_trivia_memo: SkipTriviaMemo::default(),
    };
    printer.name_generator.context = Some(Rc::clone(&emit_context));
    // PORT: the Go closures capture the Printer. The Rust closures capture the
    // Printer state that the Go methods read: the emit context, the target,
    // `HasGlobalName` and the current source file (through a shared cell).
    // `emitContext`, `Options` and `PrintHandlers` are not reassigned after
    // construction.
    {
        let source_file = Rc::clone(&printer.name_generator_source_file);
        let emit_context = Rc::clone(&emit_context);
        let target = printer.options.target;
        printer.name_generator.get_text_of_node =
            Some(Rc::new(move |generator: &mut NameGenerator, node: Node| {
                let current_source_file = source_file.get();
                let skip_trivia_memo = SkipTriviaMemo::default();
                let state = NodeTextState {
                    emit_context: &emit_context,
                    current_source_file,
                    current_original: emit_context.most_original(current_source_file),
                    target,
                    skip_trivia_memo: &skip_trivia_memo,
                };
                get_text_of_node_worker(generator, state, node, false).into_owned()
            }));
    }
    {
        let source_file = Rc::clone(&printer.name_generator_source_file);
        let has_global_name = printer.print_handlers.has_global_name.clone();
        let emit_context = Rc::clone(&emit_context);
        // Go: printer/printer.go:6143 isFileLevelUniqueNameInCurrentFile
        printer
            .name_generator
            .is_file_level_unique_name_in_current_file =
            Some(Rc::new(move |name: &str, _private_name: bool| {
                let current_source_file = source_file.get();
                if current_source_file.is_some() {
                    emit_context.is_file_level_unique_name(
                        current_source_file,
                        name,
                        has_global_name.as_deref(),
                    )
                } else {
                    true
                }
            }));
    }
    printer.container_pos = -1;
    printer.container_end = -1;
    printer.declaration_list_container_end = -1;
    printer.comments_disabled = comments_disabled;
    printer
}

impl Printer {
    /// Borrows the current writer for one call. Go reads `p.writer` directly.
    /// Panics when no writer is set, as a Go nil interface call would.
    pub(crate) fn writer(&self) -> RefMut<'_, dyn EmitTextWriter> {
        self.writer
            .as_ref()
            .expect("printer writer is nil")
            .borrow_mut()
    }

    // Go: printer/printer.go:183 (closure) makeFileLevelOptimisticUniqueName
    // PORT: the Go closure field becomes a method.
    pub(crate) fn make_file_level_optimistic_unique_name(&mut self, name: &str) -> String {
        self.sync_name_generator()
            .make_file_level_optimistic_unique_name(name)
    }

    /// Returns the name generator after its callbacks see the current
    /// source file. Use it for every call into the name generator.
    // PORT: no Go counterpart. See `name_generator_source_file`.
    pub(crate) fn sync_name_generator(&mut self) -> &mut NameGenerator {
        self.name_generator_source_file
            .set(self.current_source_file);
        &mut self.name_generator
    }

    // Go: printer/printer.go:196 getLiteralTextOfNode
    pub(crate) fn get_literal_text_of_node(
        &mut self,
        node: Node,
        source_file: Node,
        flags: GetLiteralTextFlags,
    ) -> String {
        self.get_literal_text_of_node_cow(node, source_file, flags)
            .into_owned()
    }

    /// `get_literal_text_of_node` that borrows a source text slice or
    /// `node.text()` in place of a new String.
    // PERF: for the hot callers in this file.
    pub(crate) fn get_literal_text_of_node_cow(
        &mut self,
        node: Node,
        source_file: Node,
        flags: GetLiteralTextFlags,
    ) -> Cow<'static, str> {
        let (state, generator) = self.node_text_parts();
        get_literal_text_of_node_worker(generator, state, node, source_file, flags)
    }

    // Go: printer/printer.go:230 getTextOfNode
    // `node` must be one of Identifier | PrivateIdentifier | LiteralExpression | JsxNamespacedName
    pub(crate) fn get_text_of_node(&mut self, node: Node, include_trivia: bool) -> String {
        self.get_text_of_node_cow(node, include_trivia).into_owned()
    }

    /// `get_text_of_node` that borrows a source text slice or `node.text()`
    /// in place of a new String.
    // PERF: for the hot callers in this file.
    pub(crate) fn get_text_of_node_cow(
        &mut self,
        node: Node,
        include_trivia: bool,
    ) -> Cow<'static, str> {
        let (state, generator) = self.node_text_parts();
        get_text_of_node_worker(generator, state, node, include_trivia)
    }

    /// The state the text workers read, and the name generator synced to
    /// the current source file (see `sync_name_generator`).
    // PORT: split field borrows, so the emit context `Rc` is not cloned.
    fn node_text_parts(&mut self) -> (NodeTextState<'_>, &mut NameGenerator) {
        let current_original = self.current_source_file_original();
        self.name_generator_source_file
            .set(self.current_source_file);
        let state = NodeTextState {
            emit_context: &self.emit_context,
            current_source_file: self.current_source_file,
            current_original,
            target: self.options.target,
            skip_trivia_memo: &self.skip_trivia_memo,
        };
        (state, &mut self.name_generator)
    }

    /// `emit_context.most_original(current_source_file)`.
    // PERF: cached for one file in `current_original_cache`.
    pub(crate) fn current_source_file_original(&self) -> Node {
        let file = self.current_source_file;
        let (cached_file, cached_original) = self.current_original_cache.get();
        if cached_file == file && file.is_some() {
            return cached_original;
        }
        let original = self.emit_context.most_original(file);
        self.current_original_cache.set((file, original));
        original
    }

    /// Go `p.currentSourceFile.Text()`.
    // PERF: a static text is cached for one file in `current_text_cache`,
    // so the comment reads of each node do no store lookup. The text of a
    // freeable file version (textleak1) is read each time: the cache keeps
    // no `FileText`.
    pub(crate) fn current_source_file_text(&self) -> FileText {
        let file = self.current_source_file;
        let (cached_file, cached_text) = self.current_text_cache.get();
        if cached_file == file && file.is_some() {
            return FileText::Static(cached_text);
        }
        let text = source_file_text(file);
        if let Some(static_text) = text.as_static() {
            self.current_text_cache.set((file, static_text));
        }
        text
    }

    /// Go `p.currentSourceFile.ECMALineMap()`.
    // PERF: a transformed SourceFile is a synthetic node with the text of its
    // most original parsed file. It reads the frozen line map of that file,
    // as `set_source_map_source` does. `source_file_ecma_line_map` of the
    // synthetic node would compute and leak a new map for each file. The
    // result is cached for one file in `current_line_map_cache`.
    // lsshells M3b: the map of a freeable file version is a pinned guard,
    // which the cache does not keep.
    pub(crate) fn current_line_map(&self) -> FileRef<[i32]> {
        let file = self.current_source_file;
        let (cached_file, cached_line_map) = self.current_line_map_cache.get();
        if cached_file == file && file.is_some() {
            return FileRef::Static(cached_line_map);
        }
        let mut line_source = file;
        if is_synthetic_node(file) {
            let original = self.current_source_file_original();
            if original.is_some() && !is_synthetic_node(original) && is_source_file(original) {
                let text = source_file_text(file);
                let original_text = source_file_text(original);
                let same_text =
                    std::ptr::eq::<str>(&*text, &*original_text) || text == original_text;
                debug_assert!(
                    same_text,
                    "transformed SourceFile text differs from its original"
                );
                // Same line map only for the same text.
                if same_text {
                    line_source = original;
                }
            }
        }
        let line_map = source_file_ecma_line_map(line_source);
        if let Some(static_map) = line_map.as_static() {
            self.current_line_map_cache.set((file, static_map));
        }
        line_map
    }
}

/// The Printer state that the text-of-node workers read.
// PORT: Go reads `p.emitContext`, `p.currentSourceFile` and
// `p.Options.Target`. The name generator callback has no Printer (see
// `new_printer`), so the workers take this view.
#[derive(Clone, Copy)]
struct NodeTextState<'a> {
    emit_context: &'a EmitContext,
    current_source_file: Node,
    /// `emit_context.most_original(current_source_file)`.
    current_original: Node,
    target: ScriptTarget,
    skip_trivia_memo: &'a SkipTriviaMemo,
}

/// Go `emitContext.textSource[node]`, or nil.
// PERF: no hash lookup while the map is empty.
fn text_source_of(emit_context: &EmitContext, node: Node) -> Node {
    let text_source = emit_context.text_source.borrow();
    if text_source.is_empty() {
        return Node::NIL;
    }
    text_source.get(&node).copied().unwrap_or(Node::NIL)
}

// Go: printer/printer.go:196 getLiteralTextOfNode
// PORT: body of the Printer method as a free function over the Printer state
// it reads (`NodeTextState` and `p.nameGenerator`). The name generator
// callback runs it without the Printer. See `new_printer`.
// PERF: returns a Cow, borrowed where the text is a source slice or
// `node.text()`.
fn get_literal_text_of_node_worker(
    generator: &mut NameGenerator,
    state: NodeTextState<'_>,
    node: Node,
    source_file: Node,
    flags: GetLiteralTextFlags,
) -> Cow<'static, str> {
    let emit_context = state.emit_context;
    let mut flags = flags;
    if is_string_literal(node) {
        let text_source_node = text_source_of(emit_context, node);
        if text_source_node.is_some() {
            let text: Cow<'static, str>;
            match text_source_node.kind() {
                SyntaxKind::NumericLiteral => {
                    text = Cow::Borrowed(text_source_node.text());
                }
                SyntaxKind::Identifier
                | SyntaxKind::PrivateIdentifier
                | SyntaxKind::JsxNamespacedName => {
                    text = get_text_of_node_worker(generator, state, text_source_node, false);
                }
                _ => {
                    return get_literal_text_of_node_worker(
                        generator,
                        state,
                        text_source_node,
                        get_source_file_of_node(text_source_node),
                        flags,
                    );
                }
            }

            if flags.intersects(GetLiteralTextFlags::JSX_ATTRIBUTE_ESCAPE) {
                return Cow::Owned(format!(
                    "\"{}\"",
                    escape_jsx_attribute_string(&text, QuoteChar::DOUBLE_QUOTE)
                ));
            } else if flags.intersects(GetLiteralTextFlags::NEVER_ASCII_ESCAPE)
                || emit_context
                    .emit_flags(node)
                    .intersects(EmitFlags::NO_ASCII_ESCAPING)
            {
                return Cow::Owned(format!(
                    "\"{}\"",
                    escape_string(&text, QuoteChar::DOUBLE_QUOTE)
                ));
            } else {
                return Cow::Owned(format!(
                    "\"{}\"",
                    escape_non_ascii_string(&text, QuoteChar::DOUBLE_QUOTE)
                ));
            }
        }
    }
    // !!! Printer option to control whether to terminate unterminated literals
    if emit_context
        .emit_flags(node)
        .intersects(EmitFlags::NO_ASCII_ESCAPING)
    {
        flags |= GetLiteralTextFlags::NEVER_ASCII_ESCAPE;
    }
    if state.target >= ScriptTarget::ES2021 {
        flags |= GetLiteralTextFlags::ALLOW_NUMERIC_SEPARATOR;
    }
    // Go: core.Coalesce(sourceFile, p.currentSourceFile)
    let source_file = if source_file.is_some() {
        source_file
    } else {
        state.current_source_file
    };
    get_literal_text_cow(node, source_file, flags, state.skip_trivia_memo)
}

// Go: printer/printer.go:230 getTextOfNode
// PORT: free function form of the Printer method. See
// `get_literal_text_of_node_worker`.
// `Printer::write_factory_name_text` takes the `node.Text()` return of the
// Identifier arm for a factory name. It uses the same two tests
// (`has_auto_generate_info` and `can_use_source_file`). A change to this
// function must change it too.
fn get_text_of_node_worker(
    generator: &mut NameGenerator,
    state: NodeTextState<'_>,
    node: Node,
    include_trivia: bool,
) -> Cow<'static, str> {
    let emit_context = state.emit_context;
    // PERF: printfast1. Go reads `node.Kind` for `IsMemberName`,
    // `IsStringLiteral` and the switch. A factory node reads its kind in
    // the thread's arena, so the port reads it once.
    let kind = node.kind();
    if matches!(kind, SyntaxKind::Identifier | SyntaxKind::PrivateIdentifier)
        && emit_context.has_auto_generate_info(node)
    {
        return Cow::Owned(generator.generate_name(node));
    }

    if kind == SyntaxKind::StringLiteral {
        let text_source_node = text_source_of(emit_context, node);
        if text_source_node.is_some() {
            return get_text_of_node_worker(generator, state, text_source_node, include_trivia);
        }
    }

    let current_source_file = state.current_source_file;

    match kind {
        SyntaxKind::Identifier | SyntaxKind::PrivateIdentifier | SyntaxKind::JsxNamespacedName => {
            if !can_use_source_file(current_source_file, node)
                || get_source_file_of_node(node) != state.current_original
            {
                return Cow::Borrowed(node.text());
            }
        }
        SyntaxKind::StringLiteral
        | SyntaxKind::NumericLiteral
        | SyntaxKind::BigIntLiteral
        | SyntaxKind::NoSubstitutionTemplateLiteral
        | SyntaxKind::TemplateHead
        | SyntaxKind::TemplateMiddle
        | SyntaxKind::TemplateTail => {
            return get_literal_text_of_node_worker(
                generator,
                state,
                node,
                Node::NIL, /*sourceFile*/
                GetLiteralTextFlags::NONE,
            );
        }
        kind => panic!("unexpected node: {}", kind_string(kind)),
    }
    source_text_of_node_cow(
        current_source_file,
        node,
        include_trivia,
        state.skip_trivia_memo,
    )
}

/// Go `canUseSourceFile` of `getTextOfNode` (printer.go:241): the text of
/// `node` can come from `current_source_file`. `get_text_of_node_worker`
/// and `Printer::write_factory_name_text` both use it.
fn can_use_source_file(current_source_file: Node, node: Node) -> bool {
    current_source_file.is_some() && node.parent().is_some() && !node_is_synthesized(node)
}

/// Go `scanner.GetSourceTextOfNodeFromSourceFile` that borrows the source
/// slice. The missing-node, reparser-literal and JSDoc cases use the String
/// form.
// PERF: no new String for the common case.
// PORT: the JSDoc test is a cheap superset of the private scanner
// `isJSDocTypeExpressionOrChild` (tsgo#4839). The String form makes the exact
// check and strips the ` * ` line prefixes of a JSDoc type.
fn source_text_of_node_cow(
    source_file: Node,
    node: Node,
    include_trivia: bool,
    skip_trivia_memo: &SkipTriviaMemo,
) -> Cow<'static, str> {
    if node_is_missing(node)
        || is_js_doc_type_expression(node)
        || node.flags().intersects(
            NodeFlags::REPARSER_TRANSFORMED_LITERAL | NodeFlags::JS_DOC | NodeFlags::REPARSED,
        )
    {
        return Cow::Owned(get_source_text_of_node_from_source_file(
            source_file,
            node,
            include_trivia,
        ));
    }
    let text = source_file_text(source_file);
    let pos = if include_trivia {
        node.pos()
    } else {
        skip_trivia_memo.skip_trivia(source_file, node.pos())
    };
    let range = pos as usize..node.end() as usize;
    // A freeable file version's text is not `'static` (`FileText`).
    match text.as_static() {
        Some(text) => Cow::Borrowed(&text[range]),
        None => Cow::Owned(text[range].to_string()),
    }
}

/// Go `getLiteralText` (printer/utilities.go) that borrows where the result
/// is a source slice or `node.text()`. Other cases call `get_literal_text`.
// PERF: no new String for the common literal cases.
fn get_literal_text_cow(
    node: Node,
    source_file: Node,
    flags: GetLiteralTextFlags,
    skip_trivia_memo: &SkipTriviaMemo,
) -> Cow<'static, str> {
    let text = if source_file.is_some() && can_use_original_text(node, flags) {
        source_text_of_node_cow(
            source_file,
            node,
            false, /*includeTrivia*/
            skip_trivia_memo,
        )
    } else if matches!(
        node.kind(),
        SyntaxKind::NumericLiteral | SyntaxKind::BigIntLiteral
    ) {
        Cow::Borrowed(node.text())
    } else {
        return Cow::Owned(get_literal_text(node, source_file, flags));
    };
    debug_assert_eq!(&*text, &*get_literal_text(node, source_file, flags));
    text
}

// Go: printer/utilities.go:199 canUseOriginalText
// PORT: a copy of the private `can_use_original_text` in utilities.rs, for
// `get_literal_text_cow`. Keep the two the same. The debug_assert in
// `get_literal_text_cow` checks the result against `get_literal_text`.
fn can_use_original_text(node: Node, flags: GetLiteralTextFlags) -> bool {
    // A synthetic node has no original text, nor does a node without a parent as we would be unable to find the
    // containing SourceFile. We also cannot use the original text if the literal was unterminated and the caller has
    // requested proper termination of unterminated literals
    if node_is_synthesized(node)
        || node.parent().is_nil()
        || flags.intersects(GetLiteralTextFlags::TERMINATE_UNTERMINATED_LITERALS)
            && is_unterminated_literal(node)
    {
        return false;
    }

    if node.kind() == SyntaxKind::NumericLiteral {
        let token_flags = node.token_flags();
        // For a numeric literal, we cannot use the original text if the original text was an invalid literal
        if token_flags.intersects(TokenFlags::IS_INVALID) {
            return false;
        }
        // We also cannot use the original text if the literal contains numeric separators, but numeric separators
        // are not permitted
        if token_flags.intersects(TokenFlags::CONTAINS_SEPARATOR) {
            return flags.intersects(GetLiteralTextFlags::ALLOW_NUMERIC_SEPARATOR);
        }
    }

    // Finally, we do not use the original text of a BigInt literal
    node.kind() != SyntaxKind::BigIntLiteral
}

//
// Low-level writing
//

// Go: printer/printer.go:268 WriteKind
go_enum!(WriteKind, i32 {
    NONE = 0; // WriteKindNone
    KEYWORD = 1; // WriteKindKeyword
    OPERATOR = 2; // WriteKindOperator
    PUNCTUATION = 3; // WriteKindPunctuation
    STRING_LITERAL = 4; // WriteKindStringLiteral
    PARAMETER = 5; // WriteKindParameter
    PROPERTY = 6; // WriteKindProperty
    COMMENT = 7; // WriteKindComment
    LITERAL = 8; // WriteKindLiteral
});

impl Printer {
    // Go: printer/printer.go:282 writeAs
    pub(crate) fn write_as(&mut self, text: &str, write_kind: WriteKind) {
        match write_kind {
            WriteKind::NONE => self.writer().write(text),
            WriteKind::PARAMETER => self.write_parameter(text),
            WriteKind::KEYWORD => self.write_keyword(text),
            WriteKind::OPERATOR => self.write_operator(text),
            WriteKind::PROPERTY => self.write_property(text),
            WriteKind::PUNCTUATION => self.write_punctuation(text),
            WriteKind::STRING_LITERAL => self.writer().write_string_literal(text),
            WriteKind::COMMENT => self.write_comment(text),
            WriteKind::LITERAL => self.write_literal(text),
            _ => panic!("unexpected printer.WriteKind: {:?}", write_kind),
        }
    }

    // Go: printer/printer.go:307 write
    pub(crate) fn write(&mut self, text: &str) {
        self.write_as(text, self.write_kind);
    }

    // Go: printer/printer.go:311 setWriteKind
    pub(crate) fn set_write_kind(&mut self, kind: WriteKind) -> WriteKind {
        let previous = self.write_kind;
        self.write_kind = kind;
        previous
    }

    // Go: printer/printer.go:317 writeSymbol
    pub(crate) fn write_symbol(&mut self, text: &str, opt_symbol: SymbolId) {
        if opt_symbol.is_nil() {
            self.write(text);
        } else {
            self.writer().write_symbol(text, opt_symbol);
        }
    }

    // Go: printer/printer.go:325 writeLiteral
    pub(crate) fn write_literal(&mut self, text: &str) {
        self.writer().write_literal(text);
    }

    // Go: printer/printer.go:329 writePunctuation
    pub(crate) fn write_punctuation(&mut self, text: &str) {
        self.writer().write_punctuation(text);
    }

    // Go: printer/printer.go:333 writeOperator
    pub(crate) fn write_operator(&mut self, text: &str) {
        self.writer().write_operator(text);
    }

    // Go: printer/printer.go:337 writeKeyword
    pub(crate) fn write_keyword(&mut self, text: &str) {
        self.writer().write_keyword(text);
    }

    // Go: printer/printer.go:341 writeProperty
    pub(crate) fn write_property(&mut self, text: &str) {
        self.writer().write_property(text);
    }

    // Go: printer/printer.go:345 writeParameter
    pub(crate) fn write_parameter(&mut self, text: &str) {
        self.writer().write_parameter(text);
    }

    // Go: printer/printer.go:349 writeComment
    pub(crate) fn write_comment(&mut self, text: &str) {
        self.writer().write_comment(text);
    }

    // Go: printer/printer.go:353 writeSpace
    pub(crate) fn write_space(&mut self) {
        self.writer().write_space(" ");
    }

    // Go: printer/printer.go:357 writeLine
    pub(crate) fn write_line(&mut self) {
        self.writer().write_line();
    }

    // Go: printer/printer.go:361 writeLineRepeat
    pub(crate) fn write_line_repeat(&mut self, count: i32) {
        for _ in 0..count {
            self.write_line();
        }
    }

    // Go: printer/printer.go:367 writeLines
    pub(crate) fn write_lines(&mut self, text: &str) {
        let lines = split_lines(text);
        let indentation = guess_indentation(&lines);
        for line in lines {
            let mut line = line;
            if indentation > 0 {
                line = &line[indentation..];
            }
            if !line.is_empty() {
                self.write_line();
                self.write(line);
            }
        }
    }

    // Go: printer/printer.go:381 writeTrailingSemicolon
    pub(crate) fn write_trailing_semicolon(&mut self) {
        self.writer().write_trailing_semicolon(";");
    }

    // Go: printer/printer.go:385 increaseIndent
    pub(crate) fn increase_indent(&mut self) {
        self.writer().increase_indent();
    }

    // Go: printer/printer.go:389 decreaseIndent
    pub(crate) fn decrease_indent(&mut self) {
        self.writer().decrease_indent();
    }

    // Go: printer/printer.go:393 increaseIndentIf
    pub(crate) fn increase_indent_if(&mut self, indent_requested: bool) {
        if indent_requested {
            self.increase_indent();
        }
    }

    // Go: printer/printer.go:399 decreaseIndentIf
    pub(crate) fn decrease_indent_if(&mut self, indent_requested: bool) {
        if indent_requested {
            self.decrease_indent();
        }
    }

    // Go: printer/printer.go:405 writeLineOrSpace
    pub(crate) fn write_line_or_space(
        &mut self,
        parent_node: Node,
        prev_child_node: Node,
        next_child_node: Node,
    ) {
        if self.should_emit_on_single_line(parent_node) {
            self.write_space();
        } else if self.options.preserve_source_newlines {
            let lines = self.get_lines_between_nodes(parent_node, prev_child_node, next_child_node);
            if lines > 0 {
                self.write_line_repeat(lines);
            } else {
                self.write_space();
            }
        } else {
            self.write_line();
        }
    }

    // Go: printer/printer.go:420 writeLinesAndIndent
    pub(crate) fn write_lines_and_indent(
        &mut self,
        line_count: i32,
        write_space_if_not_indenting: bool,
    ) {
        if line_count > 0 {
            self.increase_indent();
            self.write_line_repeat(line_count);
        } else if write_space_if_not_indenting {
            self.write_space();
        }
    }

    // Go: printer/printer.go:429 writeLineSeparatorsAndIndentBefore
    pub(crate) fn write_line_separators_and_indent_before(
        &mut self,
        node: Node,
        parent: Node,
    ) -> bool {
        if self.options.preserve_source_newlines {
            let leading_newlines =
                self.get_leading_line_terminator_count(parent, node, ListFormat::NONE);
            if leading_newlines > 0 {
                self.write_lines_and_indent(
                    leading_newlines,
                    false, /*writeSpaceIfNotIndenting*/
                );
                return true;
            }
        }
        false
    }

    // Go: printer/printer.go:440 writeLineSeparatorsAfter
    pub(crate) fn write_line_separators_after(&mut self, node: Node, parent: Node) {
        if self.options.preserve_source_newlines {
            let trailing_newlines = self.get_closing_line_terminator_count(
                parent,
                node,
                ListFormat::NONE,
                TextRange::new(-1, -1), /*childrenTextRange*/
            );
            if trailing_newlines > 0 {
                self.write_line_repeat(trailing_newlines);
            }
        }
    }

    // Go: printer/printer.go:449 getLinesBetweenNodes
    pub(crate) fn get_lines_between_nodes(&self, parent: Node, node1: Node, node2: Node) -> i32 {
        if self.should_elide_indentation(parent) {
            return 0;
        }

        let parent = skip_synthesized_parentheses(parent);
        let node1 = skip_synthesized_parentheses(node1);
        let node2 = skip_synthesized_parentheses(node2);

        // Always use a newline for synthesized code if the synthesizer desires it.
        if self.should_emit_on_new_line(node2, ListFormat::NONE) {
            return 1;
        }

        if self.current_source_file.is_some()
            && !node_is_synthesized(parent)
            && !node_is_synthesized(node1)
            && !node_is_synthesized(node2)
        {
            let current_source_file = self.current_source_file;
            if self.options.preserve_source_newlines {
                return self.get_effective_lines(&|include_comments| {
                    get_lines_between_range_end_and_range_start(
                        node1.loc(),
                        node2.loc(),
                        current_source_file,
                        include_comments,
                    )
                });
            }
            return if range_end_is_on_same_line_as_range_start(
                node1.loc(),
                node2.loc(),
                current_source_file,
            ) {
                0
            } else {
                1
            };
        }

        0
    }

    // Go: printer/printer.go:482 getEffectiveLines
    pub(crate) fn get_effective_lines(&self, get_line_difference: &dyn Fn(bool) -> i32) -> i32 {
        // If 'preserveSourceNewlines' is disabled, we should never call this function
        // because it could be more expensive than alternative approximations.
        if !self.options.preserve_source_newlines {
            panic!("Should not be called when preserveSourceNewlines is false");
        }
        // We start by measuring the line difference from a position to its adjacent comments,
        // so that this is counted as a one-line difference, not two:
        //
        //   node1;
        //   // NODE2 COMMENT
        //   node2;
        let lines = get_line_difference(true /*includeComments*/);
        if lines == 0 {
            // However, if the line difference considering comments was 0, we might have this:
            //
            //   node1; // NODE2 COMMENT
            //   node2;
            //
            // in which case we should be ignoring node2's comment, so this too is counted as
            // a one-line difference, not zero.
            return get_line_difference(false /*includeComments*/);
        }
        lines
    }

    // Go: printer/printer.go:508 getLeadingLineTerminatorCount
    pub(crate) fn get_leading_line_terminator_count(
        &self,
        parent_node: Node,
        first_child: Node,
        format: ListFormat,
    ) -> i32 {
        if format.intersects(ListFormat::PRESERVE_LINES) || self.options.preserve_source_newlines {
            if format.intersects(ListFormat::PREFER_NEW_LINE) {
                return 1;
            }

            if first_child.is_nil() {
                return if parent_node.is_nil()
                    || self.current_source_file.is_some()
                        && range_is_on_single_line(parent_node.loc(), self.current_source_file)
                {
                    0
                } else {
                    1
                };
            }
            if self.next_list_element_pos > 0 && first_child.pos() == self.next_list_element_pos {
                // If this child starts at the beginning of a list item in a parent list, its leading
                // line terminators have already been written as the separating line terminators of the
                // parent list. Example:
                //
                // class Foo {
                //   constructor() {}
                //   public foo() {}
                // }
                //
                // The outer list is the list of class members, with one line terminator between the
                // constructor and the method. The constructor is written, the separating line terminator
                // is written, and then we start emitting the method. Its modifiers ([public]) constitute an inner
                // list, so we look for its leading line terminators. If we didn't know that we had already
                // written a newline as part of the parent list, it would appear that we need to write a
                // leading newline to start the modifiers.
                return 0;
            }
            if first_child.kind() == SyntaxKind::JsxText {
                // JsxText will be written with its leading whitespace, so don't add more manually.
                return 0;
            }
            if self.current_source_file.is_some()
                && parent_node.is_some()
                && !position_is_synthesized(parent_node.pos())
                && !node_is_synthesized(first_child)
                && (first_child.parent().is_nil()/*|| getOriginalNode(firstChild.Parent) == getOriginalNode(parentNode)*/)
            {
                let current_source_file = self.current_source_file;
                if self.options.preserve_source_newlines {
                    return self.get_effective_lines(&|include_comments| {
                        get_lines_between_position_and_preceding_non_whitespace_character(
                            first_child.pos(),
                            parent_node.pos(),
                            current_source_file,
                            include_comments,
                        )
                    });
                }
                return if range_start_positions_are_on_same_line(
                    parent_node.loc(),
                    first_child.loc(),
                    current_source_file,
                ) {
                    0
                } else {
                    1
                };
            }
            if self.should_emit_on_new_line(first_child, format) {
                return 1;
            }
        }
        if format.intersects(ListFormat::MULTI_LINE) {
            1
        } else {
            0
        }
    }

    // Go: printer/printer.go:564 getSeparatingLineTerminatorCount
    pub(crate) fn get_separating_line_terminator_count(
        &self,
        previous_node: Node,
        next_node: Node,
        format: ListFormat,
    ) -> i32 {
        if format.intersects(ListFormat::PRESERVE_LINES) || self.options.preserve_source_newlines {
            if previous_node.is_nil() || next_node.is_nil() {
                return 0;
            }
            if next_node.kind() == SyntaxKind::JsxText {
                // JsxText will be written with its leading whitespace, so don't add more manually.
                return 0;
            } else if self.current_source_file.is_some()
                && !node_is_synthesized(previous_node)
                && !node_is_synthesized(next_node)
            {
                let current_source_file = self.current_source_file;
                if self.options.preserve_source_newlines
                    && sibling_node_positions_are_comparable(
                        &self.emit_context,
                        previous_node,
                        next_node,
                    )
                {
                    return self.get_effective_lines(&|include_comments| {
                        get_lines_between_range_end_and_range_start(
                            previous_node.loc(),
                            next_node.loc(),
                            current_source_file,
                            include_comments,
                        )
                    });
                } else if !self.options.preserve_source_newlines
                    && original_nodes_have_same_parent(&self.emit_context, previous_node, next_node)
                {
                    // If `preserveSourceNewlines` is `false` we do not intend to preserve the effective lines between the
                    // previous and next node. Instead we naively check whether nodes are on separate lines within the
                    // same node parent. If so, we intend to preserve a single line terminator. This is less precise and
                    // expensive than checking with `preserveSourceNewlines` as above, but the goal is not to preserve the
                    // effective source lines between two sibling nodes.
                    return if range_end_is_on_same_line_as_range_start(
                        previous_node.loc(),
                        next_node.loc(),
                        current_source_file,
                    ) {
                        0
                    } else {
                        1
                    };
                }
                // If the two nodes are not comparable, add a line terminator based on the format that can indicate
                // whether new lines are preferred or not.
                return if format.intersects(ListFormat::PREFER_NEW_LINE) {
                    1
                } else {
                    0
                };
            } else if self.should_emit_on_new_line(previous_node, format)
                || self.should_emit_on_new_line(next_node, format)
            {
                return 1;
            }
        } else if self.should_emit_on_new_line(next_node, ListFormat::NONE) {
            return 1;
        }
        if format.intersects(ListFormat::MULTI_LINE) {
            1
        } else {
            0
        }
    }

    // Go: printer/printer.go:604 getClosingLineTerminatorCount
    pub(crate) fn get_closing_line_terminator_count(
        &self,
        parent_node: Node,
        last_child: Node,
        format: ListFormat,
        children_text_range: TextRange,
    ) -> i32 {
        if format.intersects(ListFormat::PRESERVE_LINES) || self.options.preserve_source_newlines {
            if format.intersects(ListFormat::PREFER_NEW_LINE) {
                return 1;
            }
            if last_child.is_nil() {
                return if parent_node.is_nil()
                    || self.current_source_file.is_some()
                        && range_is_on_single_line(parent_node.loc(), self.current_source_file)
                {
                    0
                } else {
                    1
                };
            }
            if self.current_source_file.is_some()
                && parent_node.is_some()
                && !position_is_synthesized(parent_node.pos())
                && !node_is_synthesized(last_child)
                && (last_child.parent().is_nil() || last_child.parent() == parent_node)
            {
                let current_source_file = self.current_source_file;
                if self.options.preserve_source_newlines {
                    let end = greatest_end(last_child.end(), &[&children_text_range]);
                    return self.get_effective_lines(&|include_comments| {
                        get_lines_between_position_and_next_non_whitespace_character(
                            end,
                            parent_node.end(),
                            current_source_file,
                            include_comments,
                        )
                    });
                }
                return if range_end_positions_are_on_same_line(
                    parent_node.loc(),
                    last_child.loc(),
                    current_source_file,
                ) {
                    0
                } else {
                    1
                };
            }
            if self.should_emit_on_new_line(last_child, format) {
                return 1;
            }
        }
        if format.intersects(ListFormat::MULTI_LINE)
            && !format.intersects(ListFormat::NO_TRAILING_NEW_LINE)
        {
            return 1;
        }
        0
    }

    // Go: printer/printer.go:638 writeCommentRange
    pub(crate) fn write_comment_range(&mut self, comment: CommentRange) {
        if self.current_source_file.is_nil() {
            return;
        }

        let text = self.current_source_file_text();
        let line_map = self.current_line_map();
        self.write_comment_range_worker(&text, &line_map, comment.kind, comment.text_range);
    }

    // Go: printer/printer.go:648 writeCommentRangeWorker
    pub(crate) fn write_comment_range_worker(
        &mut self,
        text: &str,
        line_map: &[i32],
        kind: SyntaxKind,
        loc: TextRange,
    ) {
        if kind == SyntaxKind::MultiLineCommentTrivia {
            let indent_size = get_default_indent_size();
            let first_line = compute_line_of_position(line_map, loc.pos());
            let line_count = line_map.len() as i32;
            let mut first_comment_line_indent: i32 = -1;
            let mut pos = loc.pos();
            let mut current_line = first_line;
            while pos < loc.end() {
                let next_line_start: i32 = if current_line + 1 == line_count {
                    text.len() as i32 + 1
                } else {
                    line_map[(current_line + 1) as usize]
                };

                if pos != loc.pos() {
                    // If we are not emitting first line, we need to write the spaces to adjust the alignment
                    if first_comment_line_indent == -1 {
                        first_comment_line_indent =
                            calculate_indent(text, line_map[first_line as usize], loc.pos());
                    }

                    // These are number of spaces writer is going to write at current indent
                    let current_writer_indent_spacing = self.writer().get_indent() * indent_size;

                    // Number of spaces we want to be writing
                    // eg: Assume writer indent
                    // module m {
                    //         /* starts at character 9 this is line 1
                    //    * starts at character pos 4 line                        --1  = 8 - 8 + 3
                    //   More left indented comment */                            --2  = 8 - 8 + 2
                    //     class c { }
                    // }
                    // module m {
                    //     /* this is line 1 -- Assume current writer indent 8
                    //      * line                                                --3 = 8 - 4 + 5
                    //            More right indented comment */                  --4 = 8 - 4 + 11
                    //     class c { }
                    // }
                    let spaces_to_emit = current_writer_indent_spacing - first_comment_line_indent
                        + calculate_indent(text, pos, next_line_start);
                    if spaces_to_emit > 0 {
                        let mut number_of_single_spaces_to_emit = spaces_to_emit % indent_size;
                        let indent_size_space_string = get_indent_string(
                            (spaces_to_emit - number_of_single_spaces_to_emit) / indent_size,
                            indent_size,
                        );

                        // Write indent size string ( in eg 1: = "", 2: "" , 3: string with 8 spaces 4: string with 12 spaces
                        self.writer().raw_write(&indent_size_space_string);

                        // Emit the single spaces (in eg: 1: 3 spaces, 2: 2 spaces, 3: 1 space, 4: 3 spaces)
                        while number_of_single_spaces_to_emit > 0 {
                            self.writer().raw_write(" ");
                            number_of_single_spaces_to_emit -= 1;
                        }
                    } else {
                        // No spaces to emit write empty string
                        self.writer().raw_write("");
                    }
                }

                // Write the comment line text
                let mut end = loc.end().min(next_line_start);
                let mut scan = pos;
                while scan < end {
                    // Go: utf8.DecodeRuneInString(text[scan:end])
                    let Some(ch) = text[scan as usize..end as usize].chars().next() else {
                        break;
                    };
                    if is_line_break(ch) {
                        end = scan;
                        break;
                    }
                    scan += ch.len_utf8() as i32;
                }
                // PORT: Go `strings.TrimSpace` trims Unicode White_Space, as `str::trim` does.
                let current_line_text = text[pos as usize..end as usize].trim();
                if !current_line_text.is_empty() {
                    self.write_comment(current_line_text);
                    if end != loc.end() {
                        self.write_line();
                    }
                } else {
                    // Empty string - make sure we write empty line
                    self.writer().write_line_force(true);
                }

                pos = next_line_start;
                current_line += 1;
            }
        } else {
            // Single line comment of style //....
            self.write_comment(&text[loc.pos() as usize..loc.end() as usize]);
        }
    }

    //
    // Custom emit behavior stubs (i.e., from `EmitNode`, `EmitFlags`, etc.)
    //

    // Go: printer/printer.go:742 shouldEmitComments
    pub(crate) fn should_emit_comments(&self, node: Node) -> bool {
        !self.comments_disabled && self.current_source_file.is_some() && !is_source_file(node)
    }

    // Go: printer/printer.go:748 shouldWriteComment
    pub(crate) fn should_write_comment(&self, comment: CommentRange) -> bool {
        !self.options.only_print_js_doc_style
            || self.current_source_file.is_some()
                && is_js_doc_like_text(&self.current_source_file_text(), comment)
            || self.current_source_file.is_some()
                && is_pinned_comment(&self.current_source_file_text(), comment)
    }

    // Go: printer/printer.go:754 shouldEmitIndented
    pub(crate) fn should_emit_indented(&self, node: Node) -> bool {
        self.emit_context
            .emit_flags(node)
            .intersects(EmitFlags::INDENTED)
    }

    // Go: printer/printer.go:758 shouldElideIndentation
    pub(crate) fn should_elide_indentation(&self, node: Node) -> bool {
        self.emit_context
            .emit_flags(node)
            .intersects(EmitFlags::NO_INDENTATION)
    }

    // Go: printer/printer.go:762 shouldEmitOnSingleLine
    pub(crate) fn should_emit_on_single_line(&self, node: Node) -> bool {
        self.emit_context
            .emit_flags(node)
            .intersects(EmitFlags::SINGLE_LINE)
    }

    // Go: printer/printer.go:766 shouldEmitOnMultipleLines
    pub(crate) fn should_emit_on_multiple_lines(&self, node: Node) -> bool {
        self.emit_context
            .emit_flags(node)
            .intersects(EmitFlags::MULTI_LINE)
    }

    // Go: printer/printer.go:770 shouldEmitBlockFunctionBodyOnSingleLine
    pub(crate) fn should_emit_block_function_body_on_single_line(&self, body: Node) -> bool {
        // We must emit a function body as a single-line body in the following case:
        // * The body has NodeEmitFlags.SingleLine specified.

        // We must emit a function body as a multi-line body in the following cases:
        // * The body is explicitly marked as multi-line.
        // * A non-synthesized body's start and end position are on different lines.
        // * Any statement in the body starts on a new line.

        if self.should_emit_on_single_line(body) {
            return true;
        }

        if body.multi_line() {
            return false;
        }

        if !node_is_synthesized(body)
            && self.current_source_file.is_some()
            && !range_is_on_single_line(body.loc(), self.current_source_file)
        {
            return false;
        }

        let statements = body.statements();
        if self.get_leading_line_terminator_count(
            body,
            statements.first().unwrap_or(Node::NIL),
            ListFormat::PRESERVE_LINES,
        ) > 0
            || self.get_closing_line_terminator_count(
                body,
                statements.last().unwrap_or(Node::NIL),
                ListFormat::PRESERVE_LINES,
                body.statement_list().loc(),
            ) > 0
        {
            return false;
        }

        let mut previous_statement = Node::NIL;
        for statement in statements.iter() {
            if self.get_separating_line_terminator_count(
                previous_statement,
                statement,
                ListFormat::PRESERVE_LINES,
            ) > 0
            {
                return false;
            }

            previous_statement = statement;
        }

        true
    }

    // Go: printer/printer.go:808 shouldEmitOnNewLine
    pub(crate) fn should_emit_on_new_line(&self, node: Node, format: ListFormat) -> bool {
        if self
            .emit_context
            .emit_flags(node)
            .intersects(EmitFlags::START_ON_NEW_LINE)
        {
            return true;
        }
        format.intersects(ListFormat::PREFER_NEW_LINE)
    }

    // Go: printer/printer.go:815 shouldEmitSourceMaps
    pub(crate) fn should_emit_source_maps(&self, node: Node) -> bool {
        !self.source_maps_disabled
            && self.source_map_source.is_some()
            && !is_source_file(node)
            && !is_in_json_file(node)
    }

    // Go: printer/printer.go:822 shouldEmitTokenSourceMaps
    pub(crate) fn should_emit_token_source_maps(
        &self,
        token: SyntaxKind,
        pos: i32,
        context_node: Node,
        flags: TokenEmitFlags,
    ) -> bool {
        // We don't emit source positions for most tokens as it tends to be quite noisy, however
        // we need to emit source positions for open and close braces so that tools like istanbul
        // can map branches for code coverage. However, we still omit brace source positions when
        // the output is a declaration file.
        !flags.intersects(TokenEmitFlags::NO_SOURCE_MAPS)
            && self.should_emit_source_maps(context_node)
            && !self.options.omit_brace_source_map_positions
            && (token == SyntaxKind::OpenBraceToken || token == SyntaxKind::CloseBraceToken)
    }

    // Go: printer/printer.go:832 shouldEmitLeadingComments
    pub(crate) fn should_emit_leading_comments(&self, node: Node) -> bool {
        !self
            .emit_context
            .emit_flags(node)
            .intersects(EmitFlags::NO_LEADING_COMMENTS)
    }

    // Go: printer/printer.go:836 shouldEmitTrailingComments
    pub(crate) fn should_emit_trailing_comments(&self, node: Node) -> bool {
        !self
            .emit_context
            .emit_flags(node)
            .intersects(EmitFlags::NO_TRAILING_COMMENTS)
    }

    // Go: printer/printer.go:840 shouldEmitNestedComments
    pub(crate) fn should_emit_nested_comments(&self, node: Node) -> bool {
        !self
            .emit_context
            .emit_flags(node)
            .intersects(EmitFlags::NO_NESTED_COMMENTS)
    }

    // Go: printer/printer.go:844 shouldEmitDetachedComments
    pub(crate) fn should_emit_detached_comments(&self, node: Node) -> bool {
        if !is_source_file(node) {
            return true;
        }

        let file = node;

        // Emit detached comment if there are no prologue directives or if the first node is synthesized.
        // The synthesized node will have no leading comment so some comments may be missed.
        let statements = file.statements();
        statements.is_empty()
            || !is_prologue_directive(statements.get(0))
            || node_is_synthesized(statements.get(0))
    }

    // Go: printer/printer.go:858 hasCommentsAtPosition
    pub(crate) fn has_comments_at_position(&self, pos: i32) -> bool {
        if self.current_source_file.is_nil() {
            return false;
        }

        let factory = self.emit_context.factory().as_node_factory();
        let text = self.current_source_file_text();
        if !crate::frontend::scanner::get_trailing_comment_ranges(factory, &text, pos + 1)
            .is_empty()
        {
            return true;
        }
        if !crate::frontend::scanner::get_leading_comment_ranges(factory, &text, pos + 1).is_empty()
        {
            return true;
        }
        false
    }

    // Go: printer/printer.go:872 shouldEmitIndirectCall
    pub(crate) fn should_emit_indirect_call(&self, node: Node) -> bool {
        self.emit_context
            .emit_flags(node)
            .intersects(EmitFlags::INDIRECT_CALL)
    }

    // Go: printer/printer.go:876 shouldAllowTrailingComma
    pub(crate) fn should_allow_trailing_comma(&self, node: Node, list: NodeList) -> bool {
        if self.current_source_file.is_nil()
            || source_file_script_kind(self.current_source_file) == ScriptKind::JSON
        {
            return false;
        }

        match node.kind() {
            SyntaxKind::ObjectLiteralExpression => true,
            SyntaxKind::ArrayLiteralExpression
            | SyntaxKind::ArrowFunction
            | SyntaxKind::Constructor
            | SyntaxKind::GetAccessor
            | SyntaxKind::SetAccessor
            | SyntaxKind::TypeAliasDeclaration
            | SyntaxKind::JsTypeAliasDeclaration
            | SyntaxKind::FunctionType
            | SyntaxKind::ConstructorType
            | SyntaxKind::CallSignature
            | SyntaxKind::ConstructSignature
            | SyntaxKind::TaggedTemplateExpression
            | SyntaxKind::ObjectBindingPattern
            | SyntaxKind::ArrayBindingPattern
            | SyntaxKind::NamedImports
            | SyntaxKind::NamedExports
            | SyntaxKind::ImportAttributes => true,
            SyntaxKind::ClassExpression
            | SyntaxKind::ClassDeclaration
            | SyntaxKind::InterfaceDeclaration => {
                // PORT: Go compares `*NodeList` pointers. NodeList handles
                // are compared by list identity.
                same_node_list(list, node.type_parameter_list())
            }
            SyntaxKind::FunctionDeclaration
            | SyntaxKind::FunctionExpression
            | SyntaxKind::MethodDeclaration => true,
            SyntaxKind::CallExpression => true,
            SyntaxKind::NewExpression => true,
            _ => false,
        }
    }

    //
    // Tokens/Keywords
    //

    // Go: printer/printer.go:923 writeTokenText
    pub(crate) fn write_token_text(
        &mut self,
        token: SyntaxKind,
        write_kind: WriteKind,
        pos: i32,
    ) -> i32 {
        // !!! emit leading and trailing comments
        // !!! emit leading and trailing source maps
        let token_string = token_to_string(token);
        self.write_as(token_string, write_kind);
        if position_is_synthesized(pos) {
            pos
        } else {
            pos + token_string.len() as i32
        }
    }

    // Go: printer/printer.go:935 emitToken
    pub(crate) fn emit_token(
        &mut self,
        token: SyntaxKind,
        pos: i32,
        write_kind: WriteKind,
        context_node: Node,
    ) -> i32 {
        self.emit_token_ex(token, pos, write_kind, context_node, TokenEmitFlags::NONE)
    }

    // Go: printer/printer.go:939 emitTokenEx
    pub(crate) fn emit_token_ex(
        &mut self,
        token: SyntaxKind,
        pos: i32,
        write_kind: WriteKind,
        context_node: Node,
        flags: TokenEmitFlags,
    ) -> i32 {
        let (state, pos) = self.enter_token(token, pos, context_node, flags);
        let pos = self.write_token_text(token, write_kind, pos);
        self.exit_token(token, pos, context_node, state);
        pos
    }

    // Go: printer/printer.go:946 emitKeywordNode
    pub(crate) fn emit_keyword_node(&mut self, node: Node) {
        self.emit_keyword_node_ex(node, TokenEmitFlags::NONE);
    }

    // Go: printer/printer.go:950 emitKeywordNodeEx
    pub(crate) fn emit_keyword_node_ex(&mut self, node: Node, flags: TokenEmitFlags) {
        if node.is_nil() {
            return;
        }

        let state = self.enter_token_node(node, flags);
        // PERF: printfast1. Go `writeTokenText(node.Kind, WriteKindKeyword,
        // node.Pos())` and drops the result, the only use of the position.
        // A factory node reads its position in the thread's arena, so the
        // port writes the token text with no position read.
        self.write_as(token_to_string(node.kind()), WriteKind::KEYWORD);
        self.exit_token_node(node, state);
    }

    // Go: printer/printer.go:960 emitPunctuationNode
    pub(crate) fn emit_punctuation_node(&mut self, node: Node) {
        self.emit_punctuation_node_ex(node, TokenEmitFlags::NONE);
    }

    // Go: printer/printer.go:964 emitPunctuationNodeEx
    pub(crate) fn emit_punctuation_node_ex(&mut self, node: Node, flags: TokenEmitFlags) {
        if node.is_nil() {
            return;
        }

        let state = self.enter_token_node(node, flags);
        self.write_token_text(node.kind(), WriteKind::PUNCTUATION, node.pos());
        self.exit_token_node(node, state);
    }

    // Go: printer/printer.go:974 emitTokenNode
    pub(crate) fn emit_token_node(&mut self, node: Node) {
        self.emit_token_node_ex(node, TokenEmitFlags::NONE);
    }

    // Go: printer/printer.go:978 emitTokenNodeEx
    pub(crate) fn emit_token_node_ex(&mut self, node: Node, flags: TokenEmitFlags) {
        if node.is_nil() {
            return;
        }

        if is_keyword_kind(node.kind()) {
            self.emit_keyword_node_ex(node, flags);
        } else if is_punctuation_kind(node.kind()) {
            self.emit_punctuation_node_ex(node, flags);
        } else {
            panic!("unexpected TokenNode: {}", kind_string(node.kind()));
        }
    }

    //
    // Literals
    //

    // Go: printer/printer.go:1007 emitLiteral
    // Emits literals of the following kinds
    //
    //	SyntaxKindNumericLiteral
    //	SyntaxKindBigIntLiteral
    //	SyntaxKindStringLiteral
    //	SyntaxKindNoSubstitutionTemplateLiteral
    //	SyntaxKindRegularExpressionLiteral
    //	SyntaxKindTemplateHead
    //	SyntaxKindTemplateMiddle
    //	SyntaxKindTemplateTail
    pub(crate) fn emit_literal(&mut self, node: Node, flags: GetLiteralTextFlags) {
        let mut flags = flags;
        // Add NeverAsciiEscape flag if the printer option is set
        if self.options.never_ascii_escape {
            flags |= GetLiteralTextFlags::NEVER_ASCII_ESCAPE;
        }
        if self.options.terminate_unterminated_literals {
            flags |= GetLiteralTextFlags::TERMINATE_UNTERMINATED_LITERALS;
        }

        let text = self.get_literal_text_of_node_cow(node, Node::NIL /*sourceFile*/, flags);

        // !!! Printer option to control source map emit, which causes us to use a different write method on the
        // emit text writer:

        ////if (
        ////	(printerOptions.sourceMap || printerOptions.inlineSourceMap)
        ////	&& (node.kind === SyntaxKindStringLiteral || isTemplateLiteralKind(node.kind))
        ////) {
        ////	writeLiteral(text);
        ////} else {

        // Quick info expects all literals to be called with writeStringLiteral, as there's no specific type for
        // numberLiterals
        self.writer().write_string_literal(&text);

        // }
    }

    // Go: printer/printer.go:1035 emitNumericLiteral
    pub(crate) fn emit_numeric_literal(&mut self, node: Node) {
        let state = self.enter_node(node);
        self.emit_literal(node, GetLiteralTextFlags::NONE);
        self.exit_node(node, state);
    }

    // Go: printer/printer.go:1041 emitBigIntLiteral
    pub(crate) fn emit_big_int_literal(&mut self, node: Node) {
        let state = self.enter_node(node);
        self.emit_literal(node, GetLiteralTextFlags::NONE); // TODO: Preserve numeric literal separators after Strada migration
        self.exit_node(node, state);
    }

    // Go: printer/printer.go:1047 emitStringLiteral
    pub(crate) fn emit_string_literal(&mut self, node: Node) {
        let state = self.enter_node(node);
        self.emit_literal(node, GetLiteralTextFlags::NONE);
        self.exit_node(node, state);
    }

    // Go: printer/printer.go:1053 emitNoSubstitutionTemplateLiteral
    pub(crate) fn emit_no_substitution_template_literal(&mut self, node: Node) {
        let state = self.enter_node(node);
        self.emit_literal(node, GetLiteralTextFlags::NONE);
        self.exit_node(node, state);
    }

    // Go: printer/printer.go:1059 emitRegularExpressionLiteral
    pub(crate) fn emit_regular_expression_literal(&mut self, node: Node) {
        let state = self.enter_node(node);
        self.emit_literal(node, GetLiteralTextFlags::NONE);
        self.exit_node(node, state);
    }

    //
    // Pseudo-literals
    //

    // Go: printer/printer.go:1069 emitTemplateHead
    pub(crate) fn emit_template_head(&mut self, node: Node) {
        let state = self.enter_node(node);
        self.emit_literal(node, GetLiteralTextFlags::NONE);
        self.exit_node(node, state);
    }

    // Go: printer/printer.go:1075 emitTemplateMiddle
    pub(crate) fn emit_template_middle(&mut self, node: Node) {
        let state = self.enter_node(node);
        self.emit_literal(node, GetLiteralTextFlags::NONE);
        self.exit_node(node, state);
    }

    // Go: printer/printer.go:1081 emitTemplateTail
    pub(crate) fn emit_template_tail(&mut self, node: Node) {
        let state = self.enter_node(node);
        self.emit_literal(node, GetLiteralTextFlags::NONE);
        self.exit_node(node, state);
    }

    // Go: printer/printer.go:1087 emitTemplateMiddleTail
    pub(crate) fn emit_template_middle_tail(&mut self, node: Node) {
        match node.kind() {
            SyntaxKind::TemplateMiddle => self.emit_template_middle(node),
            SyntaxKind::TemplateTail => self.emit_template_tail(node),
            _ => {}
        }
    }

    //
    // Snippet Elements
    //

    // Go: printer/printer.go:1100 emitSnippetNode
    pub(crate) fn emit_snippet_node(&mut self, node: Node, snippet_element: &SnippetElement) {
        match snippet_element.kind {
            SnippetKind::TAB_STOP => self.emit_tab_stop(node, snippet_element),
            kind => panic!("Unhandled snippet element kind: {}", kind.0),
        }
    }

    // Go: printer/printer.go:1109 emitTabStop
    pub(crate) fn emit_tab_stop(&mut self, node: Node, snippet_element: &SnippetElement) {
        debug_assert!(
            node.kind() == SyntaxKind::EmptyStatement,
            "Snippet tab stops can only be emitted on empty statements"
        );
        self.writer()
            .raw_write(&format!("${}", snippet_element.order));
    }

    //
    // Names
    //

    // Go: printer/printer.go:1118 emitIdentifierText
    pub(crate) fn emit_identifier_text(&mut self, node: Node) {
        // PERF: printfast1. Go evaluates `GetSourceFileOfNode` for its
        // `debug.Assert` in every build. It only reads, so a release build
        // skips the parent walk.
        debug_assert!(
            {
                let f = get_source_file_of_node(node);
                f.is_nil()
                    || self.current_source_file.is_nil()
                    || source_file_file_name(f) == source_file_file_name(self.current_source_file)
            },
            "assertion failed: {{ let f = get_source_file_of_node(node); f.is_nil() || self.current_source_file.is_nil() || source_file_file_name(f) == source_file_file_name(self.current_source_file) }}"
        );
        if is_synthetic_node(node) && self.write_factory_name_text(node) {
            return;
        }
        let text = self.get_text_of_node_cow(node, false /*includeTrivia*/);

        let symbol = self
            .id_to_symbol
            .as_ref()
            .and_then(|id_to_symbol| id_to_symbol.get(&node).copied());
        if let Some(symbol) = symbol {
            self.write_symbol(&text, symbol);
            return;
        }
        self.write(&text);
    }

    /// `emitIdentifierText` of factory name `node` when Go `getTextOfNode`
    /// (printer.go:230) gives `node.Text()`: the name has no auto-generate
    /// record and Go `canUseSourceFile` is false. Writes the text of its
    /// node data. False, with nothing written, in any other case.
    // PERF: printfast1. `Node::text` of a factory name interns the text of
    // its node data at each read (`scoped_node_text`). Go reads a field.
    // Type text (`typeToString`) prints its names here.
    // PORT: the tests are the ones of the Identifier arm of
    // `get_text_of_node_worker` (`has_auto_generate_info`,
    // `can_use_source_file`), so the two cannot differ.
    fn write_factory_name_text(&mut self, node: Node) -> bool {
        if !matches!(
            node.kind(),
            SyntaxKind::Identifier | SyntaxKind::PrivateIdentifier
        ) || self.emit_context.has_auto_generate_info(node)
            || can_use_source_file(self.current_source_file, node)
        {
            return false;
        }
        let symbol = self
            .id_to_symbol
            .as_ref()
            .and_then(|id_to_symbol| id_to_symbol.get(&node).copied());
        with_ast_data(node, |data| {
            let text = match data {
                crate::astdata::NodeData::Identifier(d) => d.text.as_str(),
                crate::astdata::NodeData::PrivateIdentifier(d) => d.text.as_str(),
                _ => return false,
            };
            // `Node::text` of an empty data text reads the store name.
            if text.is_empty() {
                return false;
            }
            match symbol {
                Some(symbol) => self.write_symbol(text, symbol),
                None => self.write(text),
            }
            true
        })
    }

    // Go: printer/printer.go:1132 emitIdentifierName
    pub(crate) fn emit_identifier_name(&mut self, node: Node) {
        let state = self.enter_node(node);
        self.emit_identifier_text(node);
        self.exit_node(node, state);
    }

    // Go: printer/printer.go:1138 emitIdentifierNameNode
    pub(crate) fn emit_identifier_name_node(&mut self, node: Node) {
        if node.is_nil() {
            return;
        }
        self.emit_identifier_name(node);
    }

    // Go: printer/printer.go:1145 getUniqueHelperName
    pub(crate) fn get_unique_helper_name(&mut self, name: &str) -> Node {
        let helper_name = self
            .unique_helper_names
            .as_ref()
            .and_then(|names| names.get(name).copied())
            .unwrap_or(Node::NIL);
        if helper_name.is_nil() {
            let helper_name = self.emit_context.factory.new_unique_name_ex(
                name,
                AutoGenerateOptions {
                    flags: GeneratedIdentifierFlags::FILE_LEVEL
                        | GeneratedIdentifierFlags::OPTIMISTIC,
                    ..Default::default()
                },
            );
            self.generate_name(helper_name);
            // PORT: Go writes to a nil map would panic; callers only reach
            // here with a non-nil map.
            self.unique_helper_names
                .as_mut()
                .expect("assignment to entry in nil map")
                .insert(name.to_string(), helper_name);
            return helper_name;
        }
        self.emit_context.factory.clone_node(helper_name)
    }

    // Go: printer/printer.go:1156 emitIdentifierReference
    pub(crate) fn emit_identifier_reference(&mut self, node: Node) {
        let mut node = node;
        if (self.external_helpers_module_name.is_some() || self.unique_helper_names.is_some())
            && self
                .emit_context
                .emit_flags(node)
                .intersects(EmitFlags::HELPER_NAME)
        {
            if self.external_helpers_module_name.is_some() {
                // Substitute `__helper` with `tslib_1.__helper`
                let factory = &self.emit_context.factory;
                let helper = factory.new_property_access_expression(
                    factory.clone_node(self.external_helpers_module_name),
                    Node::NIL, /*questionDotToken*/
                    factory.clone_node(node),
                    NodeFlags::NONE,
                );
                self.emit_context
                    .assign_comment_and_source_map_ranges(helper, node);
                self.emit_property_access_expression(helper);
                return;
            }
            if self.unique_helper_names.is_some() {
                // Substitute `__helper` with `__helper_1` if there is a conflict in an ES module.
                let helper_name = self.get_unique_helper_name(node.text());
                self.emit_context
                    .assign_comment_and_source_map_ranges(helper_name, node);
                node = helper_name;
            }
        }

        let state = self.enter_node(node);
        self.emit_identifier_text(node);
        self.exit_node(node, state);
    }

    // Go: printer/printer.go:1184 emitBindingIdentifier
    pub(crate) fn emit_binding_identifier(&mut self, node: Node) {
        let mut node = node;
        if self.unique_helper_names.is_some()
            && self
                .emit_context
                .emit_flags(node)
                .intersects(EmitFlags::HELPER_NAME)
        {
            // Substitute `__helper` with `__helper_1` if there is a conflict in an ES module.
            let helper_name = self.get_unique_helper_name(node.text());
            self.emit_context
                .assign_comment_and_source_map_ranges(helper_name, node);
            node = helper_name;
        }

        let state = self.enter_node(node);
        self.emit_identifier_text(node);
        self.exit_node(node, state);
    }

    // Go: printer/printer.go:1198 emitLabelIdentifier
    pub(crate) fn emit_label_identifier(&mut self, node: Node) {
        let state = self.enter_node(node);
        self.emit_identifier_text(node);
        self.exit_node(node, state);
    }

    // Go: printer/printer.go:1204 emitPrivateIdentifier
    pub(crate) fn emit_private_identifier(&mut self, node: Node) {
        let state = self.enter_node(node);
        let text = self.get_text_of_node_cow(node, false /*includeTrivia*/);
        self.write(&text);
        self.exit_node(node, state);
    }

    // Go: printer/printer.go:1210 emitQualifiedName
    pub(crate) fn emit_qualified_name(&mut self, node: Node) {
        let state = self.enter_node(node);
        self.emit_entity_name(node.left());
        self.write_punctuation(".");
        self.emit_member_name(node.right());
        self.exit_node(node, state);
    }

    // Go: printer/printer.go:1218 emitComputedPropertyName
    pub(crate) fn emit_computed_property_name(&mut self, node: Node) {
        let state = self.enter_node(node);
        self.write_punctuation("[");
        self.emit_expression(node.expression(), OperatorPrecedence::DISALLOW_COMMA);
        self.write_punctuation("]");
        self.exit_node(node, state);
    }

    // Go: printer/printer.go:1226 emitEntityName
    pub(crate) fn emit_entity_name(&mut self, node: Node) {
        match node.kind() {
            SyntaxKind::Identifier => self.emit_identifier_reference(node),
            SyntaxKind::QualifiedName => self.emit_qualified_name(node),
            SyntaxKind::PropertyAccessExpression => {
                // TypeQuery nodes may have PropertyAccessExpression as exprName (e.g. typeof foo.x).
                // TS's emitter handles this via generic emit(); we dispatch to expression emitter here.
                self.emit_expression(node, OperatorPrecedence::DISALLOW_COMMA);
            }
            kind => panic!("unexpected EntityName: {}", kind_string(kind)),
        }
    }

    // Go: printer/printer.go:1241 emitBindingName
    pub(crate) fn emit_binding_name(&mut self, node: Node) {
        if node.is_nil() {
            return;
        }

        match node.kind() {
            SyntaxKind::Identifier => self.emit_binding_identifier(node),
            SyntaxKind::ObjectBindingPattern => self.emit_object_binding_pattern(node),
            SyntaxKind::ArrayBindingPattern => self.emit_array_binding_pattern(node),
            kind => panic!("unexpected BindingName: {}", kind_string(kind)),
        }
    }

    // Go: printer/printer.go:1258 emitPropertyName
    pub(crate) fn emit_property_name(&mut self, node: Node) {
        if node.is_nil() {
            return;
        }

        let saved_write_kind = self.write_kind;
        self.write_kind = WriteKind::PROPERTY;

        match node.kind() {
            SyntaxKind::Identifier => self.emit_identifier_name(node),
            SyntaxKind::PrivateIdentifier => self.emit_private_identifier(node),
            SyntaxKind::StringLiteral => self.emit_string_literal(node),
            SyntaxKind::NoSubstitutionTemplateLiteral => {
                self.emit_no_substitution_template_literal(node)
            }
            SyntaxKind::NumericLiteral => self.emit_numeric_literal(node),
            SyntaxKind::BigIntLiteral => self.emit_big_int_literal(node),
            SyntaxKind::ComputedPropertyName => self.emit_computed_property_name(node),
            kind => panic!("unexpected PropertyName: {}", kind_string(kind)),
        }

        self.write_kind = saved_write_kind;
    }

    // Go: printer/printer.go:1288 emitMemberName
    pub(crate) fn emit_member_name(&mut self, node: Node) {
        if node.is_nil() {
            return;
        }

        match node.kind() {
            SyntaxKind::Identifier => self.emit_identifier_name(node),
            SyntaxKind::PrivateIdentifier => self.emit_private_identifier(node),
            kind => panic!("unexpected MemberName: {}", kind_string(kind)),
        }
    }

    // Go: printer/printer.go:1303 emitModuleName
    pub(crate) fn emit_module_name(&mut self, node: Node) {
        if node.is_nil() {
            return;
        }

        match node.kind() {
            SyntaxKind::Identifier => self.emit_binding_identifier(node),
            SyntaxKind::StringLiteral => self.emit_string_literal(node),
            kind => panic!("unexpected ModuleName: {}", kind_string(kind)),
        }
    }

    // Go: printer/printer.go:1318 emitModuleExportName
    pub(crate) fn emit_module_export_name(&mut self, node: Node) {
        if node.is_nil() {
            return;
        }

        match node.kind() {
            SyntaxKind::Identifier => self.emit_identifier_name(node),
            SyntaxKind::StringLiteral => self.emit_string_literal(node),
            kind => panic!("unexpected ModuleExportName: {}", kind_string(kind)),
        }
    }

    // Go: printer/printer.go:1333 emitImportAttributeName
    pub(crate) fn emit_import_attribute_name(&mut self, node: Node) {
        match node.kind() {
            SyntaxKind::Identifier => self.emit_identifier_name(node),
            SyntaxKind::StringLiteral => self.emit_string_literal(node),
            kind => panic!("unexpected ImportAttributeName: {}", kind_string(kind)),
        }
    }

    // Go: printer/printer.go:1344 emitNestedModuleName
    pub(crate) fn emit_nested_module_name(&mut self, node: Node) {
        if node.is_nil() {
            return;
        }

        match node.kind() {
            SyntaxKind::Identifier => self.emit_identifier_name(node),
            SyntaxKind::StringLiteral => self.emit_string_literal(node),
            kind => panic!("unexpected ModuleName: {}", kind_string(kind)),
        }
    }
}

// PORT: Go compares `*ast.NodeList` pointers. Two handles name the same list
// when both are nil, or both point at the same list in the same file.
fn same_node_list(a: NodeList, b: NodeList) -> bool {
    match (a.list_ptr(), b.list_ptr()) {
        (None, None) => true,
        (Some(x), Some(y)) => a.file() == b.file() && x == y,
        _ => false,
    }
}

// Go: stringutil/util.go:87 SplitLines
// PORT: the stringutil package has no port yet; this private copy serves writeLines.
fn split_lines(text: &str) -> Vec<&str> {
    let bytes = text.as_bytes();
    let mut lines = Vec::with_capacity(text.matches('\n').count() + 1); // preallocate
    let mut start = 0;
    let mut pos = 0;
    while pos < bytes.len() {
        match bytes[pos] {
            b'\r' => {
                if pos + 1 < bytes.len() && bytes[pos + 1] == b'\n' {
                    lines.push(&text[start..pos]);
                    pos += 2;
                    start = pos;
                    continue;
                }
                // fallthrough
                lines.push(&text[start..pos]);
                pos += 1;
                start = pos;
                continue;
            }
            b'\n' => {
                lines.push(&text[start..pos]);
                pos += 1;
                start = pos;
                continue;
            }
            _ => {}
        }
        pos += 1;
    }
    if start < bytes.len() {
        lines.push(&text[start..]);
    }
    lines
}

// Go: stringutil/util.go:115 GuessIndentation
// PORT: the stringutil package has no port yet; this private copy serves writeLines.
fn guess_indentation(lines: &[&str]) -> usize {
    const MAX_SMI_X86: usize = 0x3fff_ffff;
    let mut indentation = MAX_SMI_X86;
    for line in lines {
        if line.is_empty() {
            continue;
        }
        let mut i = 0;
        while i < line.len() && i < indentation {
            let Some(ch) = line[i..].chars().next() else {
                break;
            };
            if !is_white_space_like(ch) {
                break;
            }
            i += ch.len_utf8();
        }
        if i < indentation {
            indentation = i;
        }
        if indentation == 0 {
            return 0;
        }
    }
    if indentation == MAX_SMI_X86 {
        return 0;
    }
    indentation
}
