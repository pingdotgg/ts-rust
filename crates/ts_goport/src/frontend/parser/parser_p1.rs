//! Port of typescript-go `internal/parser/parser.go` lines 1 to 1408: the
//! `Parser` struct, `ParseSourceFile`, JSON text, parser state, errors,
//! mark and rewind, token helpers, the source file worker, the list
//! functions, the `parseExpected*` helpers and `parseStatement` to
//! `parseCaseClause`.
//!
//! Parts U5 to U10 (parser_p2 to parser_p5, jsdoc.rs) add more
//! `impl Parser` blocks. They read and write the `Parser` fields directly.

use crate::frontend::prelude::*;
// PORT: explicit imports. `scanner_util` (in the prelude) still has the old
// `Scanner` and `new_scanner`. An explicit import wins over the glob.
use crate::frontend::scanner::scanner_p1::{
    ErrorCallback, Scanner, ScannerState, TEXT_TO_KEYWORD, new_scanner,
};
use smallvec::SmallVec;
use std::cell::OnceCell;
use std::sync::LazyLock;

// Go: parser.go:19 ParsingContext
/// Go `ParsingContext`. The value is the bit index in `ParsingContexts`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(i32)]
pub enum ParsingContext {
    SourceElements,           // Elements in source file
    BlockStatements,          // Statements in block
    SwitchClauses,            // Clauses in switch statement
    SwitchClauseStatements,   // Statements in switch clause
    TypeMembers,              // Members in interface or type literal
    ClassMembers,             // Members in class declaration
    EnumMembers,              // Members in enum declaration
    HeritageClauseElement,    // Elements in a heritage clause
    VariableDeclarations,     // Variable declarations in variable statement
    ObjectBindingElements,    // Binding elements in object binding list
    ArrayBindingElements,     // Binding elements in array binding list
    ArgumentExpressions,      // Expressions in argument list
    ObjectLiteralMembers,     // Members in object literal
    JsxAttributes,            // Attributes in jsx element
    JsxChildren,              // Things between opening and closing JSX tags
    ArrayLiteralMembers,      // Members in array literal
    Parameters,               // Parameters in parameter list
    JsDocParameters,          // JSDoc parameters in parameter list of JSDoc function type
    RestProperties,           // Property names in a rest type list
    TypeParameters,           // Type parameters in type parameter list
    TypeArguments,            // Type arguments in type argument list
    TupleElementTypes,        // Element types in tuple element type list
    HeritageClauses,          // Heritage clauses for a class or interface declaration.
    ImportOrExportSpecifiers, // Named import clause's import specifier list
    ImportAttributes,         // Import attributes
    JsDocComment,             // Parsing via JSDocParser
    Count,                    // Number of parsing contexts
}

impl ParsingContext {
    /// Go `range PCCount`: every context before `Count`, in order.
    // PORT: a Rust enum cannot be iterated as a Go int range.
    pub const ALL: [ParsingContext; ParsingContext::Count as usize] = [
        ParsingContext::SourceElements,
        ParsingContext::BlockStatements,
        ParsingContext::SwitchClauses,
        ParsingContext::SwitchClauseStatements,
        ParsingContext::TypeMembers,
        ParsingContext::ClassMembers,
        ParsingContext::EnumMembers,
        ParsingContext::HeritageClauseElement,
        ParsingContext::VariableDeclarations,
        ParsingContext::ObjectBindingElements,
        ParsingContext::ArrayBindingElements,
        ParsingContext::ArgumentExpressions,
        ParsingContext::ObjectLiteralMembers,
        ParsingContext::JsxAttributes,
        ParsingContext::JsxChildren,
        ParsingContext::ArrayLiteralMembers,
        ParsingContext::Parameters,
        ParsingContext::JsDocParameters,
        ParsingContext::RestProperties,
        ParsingContext::TypeParameters,
        ParsingContext::TypeArguments,
        ParsingContext::TupleElementTypes,
        ParsingContext::HeritageClauses,
        ParsingContext::ImportOrExportSpecifiers,
        ParsingContext::ImportAttributes,
        ParsingContext::JsDocComment,
    ];
}

// Go: parser.go:51 ParsingContexts
pub type ParsingContexts = i32;

// Go: parser.go:53 JSDocInfo
#[derive(Clone, Debug)]
pub struct JsDocInfo {
    pub parent: Node,
    pub js_docs: Vec<Node>,
}

// Go: parser.go:58 jsdocScannerInfo
pub type JsdocScannerInfo = u8;

pub const JSDOC_SCANNER_INFO_HAS_JS_DOC: JsdocScannerInfo = 1 << 0;
pub const JSDOC_SCANNER_INFO_HAS_DEPRECATED: JsdocScannerInfo = 1 << 1;
pub const JSDOC_SCANNER_INFO_HAS_SEE_OR_LINK: JsdocScannerInfo = 1 << 2;

/// Go `p.diagnostics` and `p.hasParseError`.
// PORT: Go `p.scanError` is a method value that writes the parser fields.
// Here the scanner error callback and the parser share these two fields
// through `Rc<RefCell<..>>` (plan contract 5), so the diagnostics keep the
// Go order and the "same position" check.
#[derive(Debug, Default)]
pub struct ParseDiagnostics {
    pub diagnostics: Vec<Diagnostic>,
    pub has_parse_error: bool,
}

impl ParseDiagnostics {
    // Go: parser.go:330 parseErrorAtRange
    /// Returns the index of the new diagnostic, or `None` when Go returns nil.
    // PORT: Go returns the `*ast.Diagnostic`. Rust diagnostics are owned
    // values, so the index in `diagnostics` is returned.
    pub fn parse_error_at_range(
        &mut self,
        loc: TextRange,
        message: &'static crate::diagnostics::Message,
        args: Vec<String>,
    ) -> Option<usize> {
        // Don't report another error if it would just be at the same location as the last error
        let mut result = None;
        if self
            .diagnostics
            .last()
            .is_none_or(|last| last.pos != loc.pos())
        {
            self.diagnostics
                .push(new_diagnostic(Node::NIL, loc, message, args));
            result = Some(self.diagnostics.len() - 1);
        }
        self.has_parse_error = true;
        result
    }
}

// Go: parser.go:66 Parser
// PORT: Go `nodeSliceArena` and `stringSliceArena` are allocation arenas.
// Rust vectors own their data, so they are not ported. `hasParseError` is in
// `ParseDiagnostics` (see there). Go `setParentFromContext` is a closure
// field; p5 `override_parent_in_immediate_children` uses `current_parent`
// directly. `store` is the node store of the file (Go has no store).
pub struct Parser<'a> {
    pub scanner: Scanner<'a>,
    pub factory: NodeFactory,

    pub opts: SourceFileParseOptions,
    /// The text of the parsed file. The parse borrows it (textleak1): the
    /// text of a freeable file version is not leaked.
    pub source_text: &'a str,

    pub script_kind: ScriptKind,
    pub language_variant: LanguageVariant,
    pub diagnostics: Rc<RefCell<ParseDiagnostics>>,
    pub js_diagnostics: Vec<Diagnostic>,
    pub jsdoc_diagnostics: Vec<Diagnostic>,

    pub token: SyntaxKind,
    pub source_flags: NodeFlags,
    pub context_flags: NodeFlags,
    pub parsing_contexts: ParsingContexts,
    pub statement_has_await_identifier: bool,
    pub has_deprecated_tag: bool,

    pub identifier_count: i32,
    pub not_parenthesized_arrow: FxHashSet<i32>,
    pub jsdoc_infos: Vec<JsDocInfo>,
    pub possible_await_spans: Vec<i32>,
    pub jsdoc_comments_space: Vec<std::borrow::Cow<'a, str>>,
    pub jsdoc_comment_ranges_space: Vec<CommentRange>,
    pub jsdoc_tag_comments_space: Vec<std::borrow::Cow<'a, str>>,
    pub jsdoc_tag_comments_parts_space: Vec<Node>,
    pub reparse_list: Vec<Node>,

    pub current_parent: Node,
    pub reparsed_clones: Vec<Node>,

    pub store: usize,

    /// The owner of a JSDoc text whose cut splits a char, given by the
    /// caller of the parse (see `parse_js_doc_comment`).
    pub jsdoc_cut_text: Option<&'a OnceCell<String>>,
    /// When the cut of the JSDoc text in parse splits a char: the position
    /// after the first kept byte in the cut text, else `i32::MAX`. A node,
    /// list or diagnostic that ends there or later may need a map to the
    /// file (`jsdoc_tail_pos`).
    pub jsdoc_tail_first: i32,
}

// Go: parser.go:104 newParser
// PORT: Go keeps parsers in a `sync.Pool` (`getParser`/`putParser`). A new
// parser is made for each parse here.
#[must_use]
pub fn new_parser<'a>() -> Parser<'a> {
    let mut res = Parser {
        scanner: new_scanner(),
        factory: NodeFactory::new(),
        opts: SourceFileParseOptions::default(),
        source_text: "",
        script_kind: ScriptKind::default(),
        language_variant: LanguageVariant::default(),
        diagnostics: Rc::new(RefCell::new(ParseDiagnostics::default())),
        js_diagnostics: Vec::new(),
        jsdoc_diagnostics: Vec::new(),
        token: SyntaxKind::Unknown,
        source_flags: NodeFlags::NONE,
        context_flags: NodeFlags::NONE,
        parsing_contexts: 0,
        statement_has_await_identifier: false,
        has_deprecated_tag: false,
        identifier_count: 0,
        not_parenthesized_arrow: FxHashSet::default(),
        jsdoc_infos: Vec::new(),
        possible_await_spans: Vec::new(),
        jsdoc_comments_space: Vec::new(),
        jsdoc_comment_ranges_space: Vec::new(),
        jsdoc_tag_comments_space: Vec::new(),
        jsdoc_tag_comments_parts_space: Vec::new(),
        reparse_list: Vec::new(),
        current_parent: Node::NIL,
        reparsed_clones: Vec::new(),
        store: 0,
        jsdoc_cut_text: None,
        jsdoc_tail_first: i32::MAX,
    };
    res.initialize_closures();
    res
}

// Go: parser.go:110 viableKeywordSuggestions
// PORT: Go calls `scanner.GetViableKeywordSuggestions()` (scanner.go:2288),
// which ranges over the `textToKeyword` map. Go map order is random. Here the
// order is the keyword table order.
static VIABLE_KEYWORD_SUGGESTIONS: LazyLock<Vec<String>> = LazyLock::new(|| {
    TEXT_TO_KEYWORD
        .iter()
        .filter(|(text, _)| text.len() > 2)
        .map(|(text, _)| (*text).to_string())
        .collect()
});

/// Go `viableKeywordSuggestions`.
#[must_use]
pub fn viable_keyword_suggestions() -> &'static [String] {
    &VIABLE_KEYWORD_SUGGESTIONS
}

// Go: parser.go:116 isMissingNodeList
// PORT: Go marks a missing list by its shared `missingListNodes` backing
// array. Rust lists do not share backing arrays, so `create_missing_list`
// marks the list with the astdata `has_trailing_comma` bit on an empty list.
// The store never sets that bit (Go computes the trailing comma from the
// list ends) and an empty list cannot have a trailing comma.
#[must_use]
pub fn is_missing_node_list(list: NodeList) -> bool {
    !list.is_nil() && list.nodes().is_empty() && list.stored_trailing_comma()
}

// Go: parser.go:135 ParseSourceFile
// PORT: Go `NewSourceFile` makes a heap node. Here the parse makes a node
// store for the file first and freezes it at the end. The store keeps the
// file name as `&'static str`, so the name is leaked once per file. The
// store and the result keep the text (`FileText`), and the parser borrows
// it while it runs.
#[must_use]
pub fn parse_source_file(
    opts: &SourceFileParseOptions,
    source_text: impl Into<FileText>,
    script_kind: ScriptKind,
) -> ParsedSourceFile {
    let source_text: FileText = source_text.into();
    let jsdoc_cut_text = OnceCell::new();
    let mut p = new_parser();
    p.initialize_state(opts, &source_text, script_kind);
    p.jsdoc_cut_text = Some(&jsdoc_cut_text);
    // lsshells M3c: each version of an edited file is a new parse, so a
    // freeable parse (`crate::ast::is_freeable_parse`) interns the name,
    // which all its versions share, instead of leaking it again.
    let freeable = crate::ast::is_freeable_parse();
    let file_name: &'static str = if freeable {
        crate::core::Name::from(opts.file_name.as_str()).as_str()
    } else {
        Box::leak(opts.file_name.clone().into_boxed_str())
    };
    p.store = new_file_store(file_name, source_text.clone());
    // PERF: R3-1. A large bundled lib whose snapshot key matches is loaded
    // into the new store from `lib_parse.bin` (`lib_parse_snapshot.rs`),
    // with the same store and result as a parse. A lib text is static.
    let loaded = source_text
        .as_static()
        .and_then(|text| super::lib_parse_snapshot::load(p.store, opts, text, script_kind));
    let result = match loaded {
        Some(loaded) => loaded.file,
        None => {
            p.skip_go_string_marker_scans();
            p.parse_into_store()
        }
    };
    // PORT: only the tsconfig parser reads this table, and it parses no
    // file of the parse cache. A freeable parse (an edited source file)
    // does not add to it, so an edit leaks no entry (lsshells M3c).
    if !freeable {
        set_source_file_diagnostics(result.root, result.diagnostics.clone());
    }
    result
}

/// A parse that a parse worker made into a detached store
/// (`parse_source_file_detached`).
pub struct DetachedParse {
    pub file: ParsedSourceFile,
    pub store: DetachedStore,
    /// True when the parse read `opts.external_module_indicator_options`.
    /// When false, any options with the same file name and path give the
    /// same parse.
    pub read_module_indicator_options: bool,
    /// The text of each `file.imports` node. The nodes cannot be read after
    /// the store leaves the worker thread.
    pub import_specifiers: Vec<String>,
    /// True for a freeable parse (`crate::ast::is_freeable_parse` on the
    /// worker): it interned the file name and its store owns its nodes when
    /// owned nodes are on, as `parse_source_file` does in that scope.
    pub freeable: bool,
}

/// `parse_source_file` on a parse worker thread. The nodes go into the
/// detached store of this thread with a provisional id (`job`), so the
/// parse does not take a store id. The loading thread gives the store its
/// real id with `adopt_detached_parse`.
// PORT: Go parses files on many goroutines (fileloader.go work group) and
// fixes the file order afterwards. Here store ids follow the serial parse
// order, so a worker parse gets its id only when the loader asks for it.
#[must_use]
pub fn parse_source_file_detached(
    job: usize,
    opts: &SourceFileParseOptions,
    source_text: impl Into<FileText>,
    script_kind: ScriptKind,
) -> DetachedParse {
    parse_source_file_detached_reading(job, opts, source_text, script_kind, |_| ()).0
}

/// `parse_source_file_detached`, and what `read` reads of the parse while
/// its nodes are on this thread (the parse workers' import names,
/// `ImportNames`).
#[must_use]
pub fn parse_source_file_detached_reading<R>(
    job: usize,
    opts: &SourceFileParseOptions,
    source_text: impl Into<FileText>,
    script_kind: ScriptKind,
    read: impl FnOnce(&ParsedSourceFile) -> R,
) -> (DetachedParse, R) {
    let source_text: FileText = source_text.into();
    let jsdoc_cut_text = OnceCell::new();
    let mut p = new_parser();
    p.initialize_state(opts, &source_text, script_kind);
    p.jsdoc_cut_text = Some(&jsdoc_cut_text);
    // As in `parse_source_file`.
    let freeable = crate::ast::is_freeable_parse();
    let file_name: &'static str = if freeable {
        crate::core::Name::from(opts.file_name.as_str()).as_str()
    } else {
        Box::leak(opts.file_name.clone().into_boxed_str())
    };
    // Drop what a parse that panicked left on this thread.
    let _ = take_detached_file_store();
    p.store = new_detached_file_store(job, file_name, source_text.clone());
    reset_module_indicator_options_read();
    // PERF: R3-1, as in `parse_source_file`. A snapshot parse did not read
    // the module indicator options.
    if let Some(loaded) = source_text
        .as_static()
        .and_then(|text| super::lib_parse_snapshot::load(p.store, opts, text, script_kind))
    {
        let read = read(&loaded.file);
        let parse = DetachedParse {
            file: loaded.file,
            store: take_detached_file_store().expect("the detached store of the parse"),
            read_module_indicator_options: false,
            import_specifiers: loaded.import_specifiers,
            freeable,
        };
        return (parse, read);
    }
    p.skip_go_string_marker_scans();
    let file = p.parse_into_store();
    let import_specifiers = file.imports.iter().map(|n| n.text().to_string()).collect();
    let read = read(&file);
    let parse = DetachedParse {
        file,
        store: take_detached_file_store().expect("the detached store of the parse"),
        read_module_indicator_options: module_indicator_options_read(),
        import_specifiers,
        freeable,
    };
    (parse, read)
}

/// Makes a detached parse part of the program on the loading thread, as if
/// `parse_source_file(opts, ..)` had run here now. `opts` must have the file
/// name and path of the parse; the parse must not have read other module
/// indicator options than `opts` has.
#[must_use]
pub fn adopt_detached_parse(
    parse: DetachedParse,
    opts: &SourceFileParseOptions,
) -> ParsedSourceFile {
    let DetachedParse {
        mut file,
        store,
        freeable,
        ..
    } = parse;
    let remap = adopt_detached_store(store);
    file.remap_store(remap);
    file.parse_options = opts.clone();
    if file.has_lazy_js_doc {
        set_file_store_lazy_js_doc(file.store, &file.parse_options, file.script_kind);
    }
    // As in `parse_source_file`: a freeable parse leaks no table entry.
    if !freeable {
        set_source_file_diagnostics(file.root, file.diagnostics.clone());
    }
    file
}

impl<'a> Parser<'a> {
    /// The part of Go `ParseSourceFile` after `initializeState`, for the
    /// store in `self.store`. Freezes the store at the end.
    fn parse_into_store(&mut self) -> ParsedSourceFile {
        self.factory = NodeFactory::for_file(self.store);
        self.next_token();
        let result = if self.script_kind == ScriptKind::JSON {
            self.parse_json_text()
        } else {
            self.parse_source_file_worker()
        };
        freeze_file_store(self.store);
        result
    }

    // Go: parser.go:146 initializeClosures
    // PORT: Go sets the `setParentFromContext` closure field. Rust closures
    // cannot borrow the parser that owns them, so p5
    // `override_parent_in_immediate_children` reads `current_parent` directly.
    pub fn initialize_closures(&mut self) {}

    // Go: parser.go:153 isJavaScript
    #[must_use]
    pub fn is_javascript(&self) -> bool {
        self.script_kind == ScriptKind::JS || self.script_kind == ScriptKind::JSX
    }

    // Go: parser.go:157 parseJSONText
    pub fn parse_json_text(&mut self) -> ParsedSourceFile {
        let pos = self.node_pos();
        let statements;
        let eof;

        if self.token == SyntaxKind::EndOfFile {
            let end = self.node_pos();
            statements = self.new_node_list(TextRange::new(pos, end), &[]);
            eof = self.parse_token_node();
        } else {
            // PORT: Go keeps `any` (one expression or a slice). A vector holds
            // both cases here; one element is the Go single expression.
            let mut expressions: Vec<Node> = Vec::new();

            while self.token != SyntaxKind::EndOfFile {
                let expression = match { self.token } {
                    SyntaxKind::OpenBracketToken => self.parse_array_literal_expression(),
                    SyntaxKind::TrueKeyword
                    | SyntaxKind::FalseKeyword
                    | SyntaxKind::NullKeyword => self.parse_token_node(),
                    SyntaxKind::MinusToken => {
                        if self.look_ahead(|p: &mut Parser<'a>| {
                            p.next_token() == SyntaxKind::NumericLiteral
                                && p.next_token() != SyntaxKind::ColonToken
                        }) {
                            self.parse_prefix_unary_expression()
                        } else {
                            self.parse_object_literal_expression()
                        }
                    }
                    SyntaxKind::NumericLiteral | SyntaxKind::StringLiteral
                        if self.look_ahead(|p: &mut Parser<'a>| {
                            p.next_token() != SyntaxKind::ColonToken
                        }) =>
                    {
                        self.parse_literal_expression()
                    }
                    _ => self.parse_object_literal_expression(),
                };

                // Error recovery: collect multiple top-level expressions
                let first = expressions.is_empty();
                expressions.push(expression);
                if first && self.token != SyntaxKind::EndOfFile {
                    self.parse_error_at_current_token(diag::Unexpected_token, args![]);
                }
            }

            let expression = if expressions.len() > 1 {
                let end = self.node_pos();
                let elements = self.new_node_list(TextRange::new(pos, end), &expressions);
                let array = self.factory.new_array_literal_expression(elements, false);
                self.finish_node(array, pos)
            } else {
                expressions[0]
            };
            let statement = self.factory.new_expression_statement(expression);
            let statement = self.finish_node(statement, pos);
            let end = self.node_pos();
            statements = self.new_node_list(TextRange::new(pos, end), &[statement]);
            eof = self.parse_expected_token(SyntaxKind::EndOfFile);
        }
        let node =
            self.factory
                .new_parsed_source_file(&self.opts, self.source_text, statements, eof);
        let node = self.finish_node(node, pos);
        let mut result = ParsedSourceFile::new(
            self.store,
            node,
            self.opts.clone(),
            file_store_text(self.store),
            eof,
        );
        let first = result.statements().nodes();
        if !first.is_empty() {
            self.validate_json_value(&result, first.get(0).expression());
        }
        self.finish_source_file(&mut result, false);
        result
    }

    // Go: parser.go:235 validateJsonValue
    pub fn validate_json_value(&mut self, source_file: &ParsedSourceFile, value_expression: Node) {
        if value_expression.is_nil() {
            return;
        }
        match value_expression.kind() {
            SyntaxKind::TrueKeyword
            | SyntaxKind::FalseKeyword
            | SyntaxKind::NullKeyword
            | SyntaxKind::NumericLiteral => {
                return;
            }
            SyntaxKind::StringLiteral => {
                if !is_double_quoted_string(value_expression) {
                    let d = new_diagnostic(
                        source_file.root,
                        get_error_span_for_node(self.source_text, value_expression),
                        diag::String_literal_with_double_quotes_expected,
                        args![],
                    );
                    self.diagnostics.borrow_mut().diagnostics.push(d);
                }
                return;
            }
            SyntaxKind::PrefixUnaryExpression => {
                if value_expression.operator() == SyntaxKind::MinusToken
                    && value_expression.operand().kind() == SyntaxKind::NumericLiteral
                {
                    return;
                }
                // not valid JSON syntax
            }
            SyntaxKind::ObjectLiteralExpression => {
                self.validate_json_object_literal(source_file, value_expression);
                return;
            }
            SyntaxKind::ArrayLiteralExpression => {
                for element in value_expression.elements().iter() {
                    self.validate_json_value(source_file, element);
                }
                return;
            }
            _ => {}
        }
        let d = new_diagnostic(
            source_file.root,
            get_error_span_for_node(self.source_text, value_expression),
            diag::Property_value_can_only_be_string_literal_numeric_literal_true_false_null_object_literal_or_array_literal,
            args![],
        );
        self.diagnostics.borrow_mut().diagnostics.push(d);
    }

    // Go: parser.go:269 validateJsonObjectLiteral
    /// validateJsonObjectLiteral validates properties of a JSON object literal.
    pub fn validate_json_object_literal(&mut self, source_file: &ParsedSourceFile, node: Node) {
        for element in node.properties().iter() {
            if element.kind() != SyntaxKind::PropertyAssignment {
                let d = new_diagnostic(
                    source_file.root,
                    get_error_span_for_node(self.source_text, element),
                    diag::Property_assignment_expected,
                    args![],
                );
                self.diagnostics.borrow_mut().diagnostics.push(d);
                continue;
            }
            let name = element.name();
            if !name.is_nil() && !is_double_quoted_string(name) {
                let d = new_diagnostic(
                    source_file.root,
                    get_error_span_for_node(self.source_text, name),
                    diag::String_literal_with_double_quotes_expected,
                    args![],
                );
                self.diagnostics.borrow_mut().diagnostics.push(d);
            }
            self.validate_json_value(source_file, element.initializer());
        }
    }

    // Go: parser.go:291 initializeState
    pub fn initialize_state(
        &mut self,
        opts: &SourceFileParseOptions,
        source_text: &'a str,
        script_kind: ScriptKind,
    ) {
        if script_kind == ScriptKind::UNKNOWN {
            panic!(
                "ScriptKind must be specified when parsing source file: {}",
                opts.file_name
            );
        }

        // PORT: `new_parser` always makes the scanner, so Go `NewScanner`
        // is not needed here.
        self.scanner.reset();
        self.opts = opts.clone();
        self.source_text = source_text;
        self.script_kind = script_kind;
        self.language_variant = get_language_variant(self.script_kind);
        self.context_flags = match self.script_kind {
            ScriptKind::JS | ScriptKind::JSX => NodeFlags::JAVA_SCRIPT_FILE,
            ScriptKind::JSON => NodeFlags::JAVA_SCRIPT_FILE | NodeFlags::JSON_FILE,
            _ => NodeFlags::NONE,
        };
        self.scanner.set_text(self.source_text);
        // Go: p.scanner.SetOnError(p.scanError)
        let diagnostics = Rc::clone(&self.diagnostics);
        let on_error: ErrorCallback = Box::new(move |message, pos, length, args| {
            scan_error(&diagnostics, message, pos, length, args);
        });
        self.scanner.set_on_error(Some(on_error));
        self.scanner.set_language_variant(self.language_variant);
    }

    /// Scans the whole text once for `GO_STRING_MARKER` and tells the
    /// scanner, so string token values skip the scan of each value
    /// (`Scanner::set_text_has_no_go_string_marker`). A whole-file parse
    /// calls it right before `parse_into_store`, not for a lib that loads
    /// from the snapshot: the scan would read the whole lib text, which
    /// the snapshot load does not (2.3 MiB more RSS on query-core, mostly
    /// lib.dom.d.ts). A lazy JSDoc parse (`parse_js_doc_for_node`) does not
    /// call it either: it reads a few values of a file that it would scan
    /// whole on each call.
    // PERF: parsefast2. Go uses the slice as is.
    fn skip_go_string_marker_scans(&mut self) {
        let no_marker = !crate::scanner_util::contains_go_string_marker(self.source_text);
        self.scanner.set_text_has_no_go_string_marker(no_marker);
    }

    // Go: parser.go:322 parseErrorAt
    pub fn parse_error_at(
        &mut self,
        pos: i32,
        end: i32,
        message: &'static crate::diagnostics::Message,
        args: Vec<String>,
    ) -> Option<usize> {
        self.parse_error_at_range(TextRange::new(pos, end), message, args)
    }

    // Go: parser.go:326 parseErrorAtCurrentToken
    pub fn parse_error_at_current_token(
        &mut self,
        message: &'static crate::diagnostics::Message,
        args: Vec<String>,
    ) -> Option<usize> {
        let range = self.scanner.token_range();
        self.parse_error_at_range(range, message, args)
    }

    // Go: parser.go:330 parseErrorAtRange
    // PORT: see `ParseDiagnostics::parse_error_at_range`.
    pub fn parse_error_at_range(
        &mut self,
        loc: TextRange,
        message: &'static crate::diagnostics::Message,
        args: Vec<String>,
    ) -> Option<usize> {
        self.diagnostics
            .borrow_mut()
            .parse_error_at_range(loc, message, args)
    }

    /// Go `p.hasParseError`.
    // PORT: the field is in `ParseDiagnostics`, because the scanner error
    // callback sets it.
    #[must_use]
    pub fn has_parse_error(&self) -> bool {
        self.diagnostics.borrow().has_parse_error
    }

    /// Go `p.hasParseError = value`.
    pub fn set_has_parse_error(&mut self, value: bool) {
        self.diagnostics.borrow_mut().has_parse_error = value;
    }

    // Go: parser.go:352 mark
    #[must_use]
    pub fn mark(&self) -> ParserState<'a> {
        let diagnostics = self.diagnostics.borrow();
        ParserState {
            scanner_state: self.scanner.mark(),
            context_flags: self.context_flags,
            diagnostics_len: diagnostics.diagnostics.len(),
            js_diagnostics_len: self.js_diagnostics.len(),
            jsdoc_infos_len: self.jsdoc_infos.len(),
            reparsed_clones_len: self.reparsed_clones.len(),
            statement_has_await_identifier: self.statement_has_await_identifier,
            has_parse_error: diagnostics.has_parse_error,
        }
    }

    // Go: parser.go:365 rewind
    pub fn rewind(&mut self, state: ParserState<'a>) {
        self.scanner.rewind(state.scanner_state);
        self.token = self.scanner.token();
        self.context_flags = state.context_flags;
        {
            let mut diagnostics = self.diagnostics.borrow_mut();
            diagnostics.diagnostics.truncate(state.diagnostics_len);
            diagnostics.has_parse_error = state.has_parse_error;
        }
        self.js_diagnostics.truncate(state.js_diagnostics_len);
        self.jsdoc_infos.truncate(state.jsdoc_infos_len);
        self.reparsed_clones.truncate(state.reparsed_clones_len);
        self.statement_has_await_identifier = state.statement_has_await_identifier;
    }

    // Go: parser.go:377 lookAhead
    pub fn look_ahead(&mut self, callback: impl FnOnce(&mut Parser<'a>) -> bool) -> bool {
        let state = self.mark();
        let result = callback(self);
        self.rewind(state);
        result
    }

    // Go: parser.go:384 nextToken
    pub fn next_token(&mut self) -> SyntaxKind {
        // if the keyword had an escape
        // PERF: U1 (c). The escape flags are tested first. Both tests are
        // pure, so the result is the same, and most tokens skip the load of
        // `self.token` that was just stored.
        if (self.scanner.has_unicode_escape() || self.scanner.has_extended_unicode_escape())
            && is_keyword(self.token)
        {
            // issue a parse error for the escape
            self.parse_error_at_current_token(
                diag::Keywords_cannot_contain_escape_characters,
                args![],
            );
        }
        self.token = self.scanner.scan();
        self.token
    }

    // Go: parser.go:394 nextTokenWithoutCheck
    pub fn next_token_without_check(&mut self) -> SyntaxKind {
        self.token = self.scanner.scan();
        self.token
    }

    // Go: parser.go:399 nextTokenJSDoc
    pub fn next_token_js_doc(&mut self) -> SyntaxKind {
        self.token = self.scanner.scan_js_doc_token();
        self.token
    }

    // Go: parser.go:404 nextJSDocCommentTextToken
    pub fn next_js_doc_comment_text_token(&mut self, in_backticks: bool) -> SyntaxKind {
        self.token = self.scanner.scan_js_doc_comment_text_token(in_backticks);
        self.token
    }

    // Go: parser.go:409 nodePos
    #[must_use]
    pub fn node_pos(&self) -> i32 {
        self.scanner.token_full_start()
    }

    // Go: parser.go:413 hasPrecedingLineBreak
    #[must_use]
    pub fn has_preceding_line_break(&self) -> bool {
        self.scanner.has_preceding_line_break()
    }

    // Go: parser.go:417 jsdocScannerInfo
    #[must_use]
    pub fn jsdoc_scanner_info(&self) -> JsdocScannerInfo {
        if !self.scanner.has_preceding_js_doc_comment() {
            return 0;
        }
        let mut info = JSDOC_SCANNER_INFO_HAS_JS_DOC;
        if self.scanner.has_preceding_js_doc_with_deprecated_tag() {
            info |= JSDOC_SCANNER_INFO_HAS_DEPRECATED;
        }
        if self.scanner.has_preceding_js_doc_with_see_or_link() {
            info |= JSDOC_SCANNER_INFO_HAS_SEE_OR_LINK;
        }
        info
    }

    // Go: parser.go:431 parseSourceFileWorker
    pub fn parse_source_file_worker(&mut self) -> ParsedSourceFile {
        let is_declaration_file = is_declaration_file_name(&self.opts.file_name);
        if is_declaration_file {
            self.context_flags |= NodeFlags::AMBIENT;
        }
        let pos = self.node_pos();
        let mut statements = self.parse_list_index(
            ParsingContext::SourceElements,
            Parser::parse_toplevel_statement,
        );
        let end = self.node_pos();
        let end_js_doc = self.jsdoc_scanner_info();
        let eof = self.parse_token_node();
        self.with_js_doc(eof, end_js_doc);
        if eof.kind() != SyntaxKind::EndOfFile {
            panic!("Expected end of file token from scanner.");
        }
        if !self.reparse_list.is_empty() {
            statements.extend(self.reparse_list.drain(..));
        }
        let list = self.new_node_list(TextRange::new(pos, end), &statements);
        let node = self
            .factory
            .new_parsed_source_file(&self.opts, self.source_text, list, eof);
        let node = self.finish_node(node, pos);
        let mut result = ParsedSourceFile::new(
            self.store,
            node,
            self.opts.clone(),
            file_store_text(self.store),
            eof,
        );
        self.finish_source_file(&mut result, is_declaration_file);
        if !result.is_declaration_file
            && !result.external_module_indicator.is_nil()
            && !self.possible_await_spans.is_empty()
        {
            let reparse = self.reparse_top_level_await(&result);
            let reparse = self.finish_node(reparse, pos);
            if node != reparse {
                result = ParsedSourceFile::new(
                    self.store,
                    reparse,
                    self.opts.clone(),
                    result.text.clone(),
                    result.end_of_file_token,
                );
                self.finish_source_file(&mut result, is_declaration_file);
            }
        }
        collect_external_module_references(&mut result);
        if is_in_js_file(node) {
            result.js_diagnostics =
                attach_file_to_diagnostics(self.js_diagnostics.clone(), result.root);
        }
        result
    }

    // Go: parser.go:466 finishSourceFile
    pub fn finish_source_file(&mut self, result: &mut ParsedSourceFile, is_declaration_file: bool) {
        result.comment_directives = self.scanner.comment_directives().to_vec();
        result.pragmas = get_comment_pragmas(&self.factory, self.source_text);
        self.process_pragmas_into_fields(result);
        let diagnostics = self.diagnostics.borrow().diagnostics.clone();
        result.diagnostics = attach_file_to_diagnostics(diagnostics, result.root);
        result.jsdoc_diagnostics =
            attach_file_to_diagnostics(self.jsdoc_diagnostics.clone(), result.root);
        result.is_declaration_file = is_declaration_file;
        result.language_variant = self.language_variant;
        result.script_kind = self.script_kind;
        set_node_flags(result.root, result.root.flags() | self.source_flags);
        result.node_count = self.factory.node_count();
        result.text_count = self.factory.text_count();
        result.identifier_count = self.identifier_count;
        result.jsdoc_cache = self.create_js_doc_cache();
        set_file_store_js_doc_cache(result.store, &result.jsdoc_cache);
        set_file_store_parse_fields(result.store, result.language_variant, &result.diagnostics);
        // For non-JS files, enable lazy JSDoc parsing on demand
        if !self.is_javascript() {
            result.has_lazy_js_doc = true;
            // PORT: node reads before a program use the store (see
            // `resolve_file_store_js_doc`).
            set_file_store_lazy_js_doc(result.store, &result.parse_options, result.script_kind);
        }
        // Go: parser/parser.go:484 slices.SortFunc(p.reparsedClones, ast.CompareNodePositions)
        crate::gostd::slices::sort_func(&mut self.reparsed_clones, |a, b| {
            compare_node_positions(*a, *b)
        });
        result.reparsed_clones = self.reparsed_clones.clone();
        set_external_module_indicator(result, self.opts.external_module_indicator_options);
    }

    // Go: parser.go:489 createJSDocCache
    // PORT: Go returns a nil map when there is no JSDoc. An empty map here.
    #[must_use]
    pub fn create_js_doc_cache(&self) -> FxHashMap<Node, Vec<Node>> {
        let mut result = FxHashMap::default();
        if self.jsdoc_infos.is_empty() {
            return result;
        }
        result.reserve(self.jsdoc_infos.len());
        for info in &self.jsdoc_infos {
            result.insert(info.parent, info.js_docs.clone());
        }
        result
    }

    // Go: parser.go:500 parseToplevelStatement
    pub fn parse_toplevel_statement(&mut self, i: i32) -> Node {
        self.statement_has_await_identifier = false;
        let statement = self.parse_statement();
        // Reparsed nodes (e.g. JSDoc @typedef) produced while parsing this statement are inserted
        // into the statement list before this statement, so account for them when recording the
        // statement's index for possibleAwaitSpans.
        let i = i + self.reparse_list.len() as i32;
        if self.statement_has_await_identifier
            && !statement.flags().intersects(NodeFlags::AWAIT_CONTEXT)
        {
            if self.possible_await_spans.last() != Some(&i) {
                self.possible_await_spans.push(i);
                self.possible_await_spans.push(i + 1);
            } else if let Some(last) = self.possible_await_spans.last_mut() {
                *last = i + 1;
            }
        }
        statement
    }

    // Go: parser.go:517 reparseTopLevelAwait
    pub fn reparse_top_level_await(&mut self, source_file: &ParsedSourceFile) -> Node {
        if self.possible_await_spans.len() % 2 == 1 {
            panic!("possibleAwaitSpans malformed: odd number of indices, not paired into spans.");
        }
        let source_statements = source_file.statements().nodes().to_vec();
        let mut statements: Vec<Node> = Vec::new();
        let saved_parse_diagnostics =
            std::mem::take(&mut self.diagnostics.borrow_mut().diagnostics);

        let mut after_await_statement: usize = 0;
        let mut i = 0;
        while i < self.possible_await_spans.len() {
            let next_await_statement = self.possible_await_spans[i] as usize;
            // append all non-await statements between afterAwaitStatement and nextAwaitStatement
            let prev_statement = source_statements[after_await_statement];
            let next_statement = source_statements[next_await_statement];
            statements
                .extend_from_slice(&source_statements[after_await_statement..next_await_statement]);

            // append all diagnostics associated with the copied range
            let diagnostic_start = saved_parse_diagnostics
                .iter()
                .position(|d| d.pos >= prev_statement.pos());
            if let Some(diagnostic_start) = diagnostic_start {
                let diagnostic_end = saved_parse_diagnostics[diagnostic_start..]
                    .iter()
                    .position(|d| d.pos >= next_statement.pos());
                let slice = match diagnostic_end {
                    Some(diagnostic_end) => {
                        &saved_parse_diagnostics
                            [diagnostic_start..diagnostic_start + diagnostic_end]
                    }
                    None => &saved_parse_diagnostics[diagnostic_start..],
                };
                self.diagnostics
                    .borrow_mut()
                    .diagnostics
                    .extend_from_slice(slice);
            }

            let mut state = self.mark();
            // reparse all statements between start and pos. We skip existing diagnostics for the same range and allow the parser to generate new ones.
            self.context_flags |= NodeFlags::AWAIT_CONTEXT;
            self.scanner.reset_pos(next_statement.pos());
            self.next_token();

            after_await_statement = self.possible_await_spans[i + 1] as usize;
            while self.token != SyntaxKind::EndOfFile {
                let start_pos = self.scanner.token_full_start();
                let statement = self.parse_statement();
                statements.push(statement);
                if start_pos == self.scanner.token_full_start() {
                    self.next_token();
                }
                if after_await_statement < source_statements.len() {
                    let last_await_statement = source_statements[after_await_statement - 1];
                    if statement.end() == last_await_statement.end() {
                        // done reparsing this section
                        break;
                    }
                    if statement.end() > last_await_statement.end() {
                        // we ate into the next statement, so we must continue reparsing the next span
                        i += 2;
                        if i < self.possible_await_spans.len() {
                            after_await_statement = self.possible_await_spans[i + 1] as usize;
                        } else {
                            after_await_statement = source_statements.len();
                        }
                    }
                }
            }

            // Keep diagnostics from the reparse
            state.diagnostics_len = self.diagnostics.borrow().diagnostics.len();
            self.rewind(state);
            i += 2;
        }

        // append all statements between pos and the end of the list
        if after_await_statement < source_statements.len() {
            let prev_statement = source_statements[after_await_statement];
            statements.extend_from_slice(&source_statements[after_await_statement..]);

            // append all diagnostics associated with the copied range
            let diagnostic_start = saved_parse_diagnostics
                .iter()
                .position(|d| d.pos >= prev_statement.pos());
            if let Some(diagnostic_start) = diagnostic_start {
                self.diagnostics
                    .borrow_mut()
                    .diagnostics
                    .extend_from_slice(&saved_parse_diagnostics[diagnostic_start..]);
            }
        }

        let loc = source_file.statements().loc();
        let list = self.new_node_list(loc, &statements);
        let result = self.factory.new_parsed_source_file(
            source_file.parse_options(),
            self.source_text,
            list,
            source_file.end_of_file_token,
        );
        for s in statements {
            set_node_parent(s, result); // force (re)set parent to reparsed source file
        }
        result
    }

    // Go: parser.go:613 parseListIndex
    // PERF: identparse1. Go's `make([]*ast.Node, 0, 16)` stays on the stack
    // (the list is cloned into the arena at the end), so the scratch list is
    // a `SmallVec` of 16, not a heap `Vec` per list.
    pub fn parse_list_index(
        &mut self,
        kind: ParsingContext,
        mut parse_element: impl FnMut(&mut Parser<'a>, i32) -> Node,
    ) -> SmallVec<[Node; 16]> {
        let save_parsing_contexts = self.parsing_contexts;
        self.parsing_contexts |= 1 << (kind as i32);
        let mut outer_reparse_list = std::mem::take(&mut self.reparse_list);
        let mut list: SmallVec<[Node; 16]> = SmallVec::new();
        while !self.is_list_terminator(kind) {
            if self.is_list_element(kind, false /*inErrorRecovery*/) {
                let elt = parse_element(self, list.len() as i32);
                if !self.reparse_list.is_empty() {
                    for e in std::mem::take(&mut self.reparse_list) {
                        // Propagate @typedef type alias declarations outwards to a context that permits them.
                        if (is_js_type_alias_declaration(e) || is_js_import_declaration(e))
                            && kind != ParsingContext::SourceElements
                            && kind != ParsingContext::BlockStatements
                        {
                            outer_reparse_list.push(e);
                        } else {
                            list.push(e);
                        }
                    }
                }
                list.push(elt);
                continue;
            }
            if self.abort_parsing_list_or_move_to_next_token(kind) {
                break;
            }
        }
        self.reparse_list = outer_reparse_list;
        self.parsing_contexts = save_parsing_contexts;
        list
    }

    // Go: parser.go:645 parseList
    pub fn parse_list(
        &mut self,
        kind: ParsingContext,
        mut parse_element: impl FnMut(&mut Parser<'a>) -> Node,
    ) -> NodeList {
        let pos = self.node_pos();
        let nodes = self.parse_list_index(kind, |p: &mut Parser<'a>, _: i32| parse_element(p));
        let end = self.node_pos();
        self.new_node_list(TextRange::new(pos, end), &nodes)
    }

    // Go: parser.go:652 parseDelimitedList
    /// Return a non-nil (but possibly empty) list if parsing was successful, or nil if parseElement returned nil
    pub fn parse_delimited_list(
        &mut self,
        kind: ParsingContext,
        mut parse_element: impl FnMut(&mut Parser<'a>) -> Node,
    ) -> NodeList {
        let pos = self.node_pos();
        let save_parsing_contexts = self.parsing_contexts;
        self.parsing_contexts |= 1 << (kind as i32);
        // PERF: `new_node_list` copies the nodes out, so the scratch list
        // stays on the stack. Most lists hold 16 nodes or fewer.
        let mut list: SmallVec<[Node; 16]> = SmallVec::new();
        loop {
            if self.is_list_element(kind, false /*inErrorRecovery*/) {
                let start_pos = self.node_pos();
                let element = parse_element(self);
                if element.is_nil() {
                    self.parsing_contexts = save_parsing_contexts;
                    // Return nil to indicate parseElement failed
                    return NodeList::NIL;
                }
                list.push(element);
                if self.parse_optional(SyntaxKind::CommaToken) {
                    // No need to check for a zero length node since we know we parsed a comma
                    continue;
                }
                if self.is_list_terminator(kind) {
                    break;
                }
                // We didn't get a comma, and the list wasn't terminated, explicitly parse
                // out a comma so we give a good error message.
                if self.token != SyntaxKind::CommaToken && kind == ParsingContext::EnumMembers {
                    self.parse_error_at_current_token(
                        diag::An_enum_member_name_must_be_followed_by_a_or,
                        args![],
                    );
                } else {
                    self.parse_expected(SyntaxKind::CommaToken);
                }
                // If the token was a semicolon, and the caller allows that, then skip it and
                // continue.  This ensures we get back on track and don't result in tons of
                // parse errors.  For example, this can happen when people do things like use
                // a semicolon to delimit object literal members.   Note: we'll have already
                // reported an error when we called parseExpected above.
                if (kind == ParsingContext::ObjectLiteralMembers
                    || kind == ParsingContext::ImportAttributes)
                    && self.token == SyntaxKind::SemicolonToken
                    && !self.has_preceding_line_break()
                {
                    self.next_token();
                }
                if start_pos == self.node_pos() {
                    // What we're parsing isn't actually remotely recognizable as a element and we've consumed no tokens whatsoever
                    // Consume a token to advance the parser in some way and avoid an infinite loop
                    // This can happen when we're speculatively parsing parenthesized expressions which we think may be arrow functions,
                    // or when a modifier keyword which is disallowed as a parameter name (ie, `static` in strict mode) is supplied
                    self.next_token();
                }
                continue;
            }
            if self.is_list_terminator(kind) {
                break;
            }
            if self.abort_parsing_list_or_move_to_next_token(kind) {
                break;
            }
        }
        self.parsing_contexts = save_parsing_contexts;
        let end = self.node_pos();
        self.new_node_list(TextRange::new(pos, end), &list)
    }

    // Go: parser.go:711 parseBracketedList
    /// Return a non-nil (but possibly empty) NodeList if parsing was successful, a missing NodeList if the opening
    /// token wasn't found, or nil if parseElement returned nil.
    pub fn parse_bracketed_list(
        &mut self,
        kind: ParsingContext,
        parse_element: impl FnMut(&mut Parser<'a>) -> Node,
        opening: SyntaxKind,
        closing: SyntaxKind,
    ) -> NodeList {
        if self.parse_expected(opening) {
            let result = self.parse_delimited_list(kind, parse_element);
            self.parse_expected(closing);
            return result;
        }
        self.create_missing_list()
    }

    // Go: parser.go:720 parseEmptyNodeList
    pub fn parse_empty_node_list(&mut self) -> NodeList {
        let pos = self.node_pos();
        self.new_node_list(TextRange::new(pos, pos), &[])
    }

    // Go: parser.go:724 createMissingList
    // PORT: see `is_missing_node_list`. The empty list is copied with the
    // marker bit set and leaked like every other list
    // (`NodeList::with_missing_marker`).
    pub fn create_missing_list(&mut self) -> NodeList {
        self.parse_empty_node_list().with_missing_marker()
    }

    // Go: parser.go:731 abortParsingListOrMoveToNextToken
    /// Returns true if we should abort parsing.
    pub fn abort_parsing_list_or_move_to_next_token(&mut self, kind: ParsingContext) -> bool {
        self.parsing_context_errors(kind);
        if self.is_in_some_parsing_context() {
            return true;
        }
        self.next_token();
        false
    }

    // Go: parser.go:741 isInSomeParsingContext
    /// True if positioned at element or terminator of the current list or any enclosing list
    pub fn is_in_some_parsing_context(&mut self) -> bool {
        // We should be in at least one parsing context, be it SourceElements while parsing
        // a SourceFile, or JSDocComment when lazily parsing JSDoc.
        go_assert!(self.parsing_contexts != 0, "Missing parsing context");
        for kind in ParsingContext::ALL {
            if self.parsing_contexts & (1 << (kind as i32)) != 0
                && (self.is_list_element(kind, true /*inErrorRecovery*/)
                    || self.is_list_terminator(kind))
            {
                return true;
            }
        }
        false
    }

    // Go: parser.go:755 parsingContextErrors
    pub fn parsing_context_errors(&mut self, context: ParsingContext) {
        use ParsingContext as PC;
        match context {
            PC::SourceElements => {
                if self.token == SyntaxKind::DefaultKeyword {
                    self.parse_error_at_current_token(diag::X_0_expected, args!["export"]);
                } else {
                    self.parse_error_at_current_token(
                        diag::Declaration_or_statement_expected,
                        args![],
                    );
                }
            }
            PC::BlockStatements => {
                self.parse_error_at_current_token(diag::Declaration_or_statement_expected, args![]);
            }
            PC::SwitchClauses => {
                self.parse_error_at_current_token(diag::X_case_or_default_expected, args![]);
            }
            PC::SwitchClauseStatements => {
                self.parse_error_at_current_token(diag::Statement_expected, args![]);
            }
            PC::RestProperties | PC::TypeMembers => {
                self.parse_error_at_current_token(diag::Property_or_signature_expected, args![]);
            }
            PC::ClassMembers => {
                self.parse_error_at_current_token(
                    diag::Unexpected_token_A_constructor_method_accessor_or_property_was_expected,
                    args![],
                );
            }
            PC::EnumMembers => {
                self.parse_error_at_current_token(diag::Enum_member_expected, args![]);
            }
            PC::HeritageClauseElement => {
                self.parse_error_at_current_token(diag::Expression_expected, args![]);
            }
            PC::VariableDeclarations => {
                if is_keyword(self.token) {
                    self.parse_error_at_current_token(
                        diag::X_0_is_not_allowed_as_a_variable_declaration_name,
                        args![token_to_string(self.token)],
                    );
                } else {
                    self.parse_error_at_current_token(diag::Variable_declaration_expected, args![]);
                }
            }
            PC::ObjectBindingElements => {
                self.parse_error_at_current_token(
                    diag::Property_destructuring_pattern_expected,
                    args![],
                );
            }
            PC::ArrayBindingElements => {
                self.parse_error_at_current_token(
                    diag::Array_element_destructuring_pattern_expected,
                    args![],
                );
            }
            PC::ArgumentExpressions => {
                self.parse_error_at_current_token(diag::Argument_expression_expected, args![]);
            }
            PC::ObjectLiteralMembers => {
                self.parse_error_at_current_token(diag::Property_assignment_expected, args![]);
            }
            PC::ArrayLiteralMembers => {
                self.parse_error_at_current_token(diag::Expression_or_comma_expected, args![]);
            }
            PC::JsDocParameters => {
                self.parse_error_at_current_token(diag::Parameter_declaration_expected, args![]);
            }
            PC::Parameters => {
                if is_keyword(self.token) {
                    self.parse_error_at_current_token(
                        diag::X_0_is_not_allowed_as_a_parameter_name,
                        args![token_to_string(self.token)],
                    );
                } else {
                    self.parse_error_at_current_token(
                        diag::Parameter_declaration_expected,
                        args![],
                    );
                }
            }
            PC::TypeParameters => {
                self.parse_error_at_current_token(
                    diag::Type_parameter_declaration_expected,
                    args![],
                );
            }
            PC::TypeArguments => {
                self.parse_error_at_current_token(diag::Type_argument_expected, args![]);
            }
            PC::TupleElementTypes => {
                self.parse_error_at_current_token(diag::Type_expected, args![]);
            }
            PC::HeritageClauses => {
                self.parse_error_at_current_token(diag::Unexpected_token_expected, args![]);
            }
            PC::ImportOrExportSpecifiers => {
                if self.token == SyntaxKind::FromKeyword {
                    self.parse_error_at_current_token(diag::X_0_expected, args!["}"]);
                } else {
                    self.parse_error_at_current_token(diag::Identifier_expected, args![]);
                }
            }
            PC::JsxAttributes | PC::JsxChildren | PC::JsDocComment => {
                self.parse_error_at_current_token(diag::Identifier_expected, args![]);
            }
            PC::ImportAttributes => {
                self.parse_error_at_current_token(
                    diag::Identifier_or_string_literal_expected,
                    args![],
                );
            }
            PC::Count => panic!("Unhandled case in parsingContextErrors"),
        }
    }

    // Go: parser.go:824 isListElement
    pub fn is_list_element(
        &mut self,
        parsing_context: ParsingContext,
        in_error_recovery: bool,
    ) -> bool {
        use ParsingContext as PC;
        match parsing_context {
            PC::SourceElements | PC::BlockStatements | PC::SwitchClauseStatements => {
                // If we're in error recovery, then we don't want to treat ';' as an empty statement.
                // The problem is that ';' can show up in far too many contexts, and if we see one
                // and assume it's a statement, then we may bail out inappropriately from whatever
                // we're parsing.  For example, if we have a semicolon in the middle of a class, then
                // we really don't want to assume the class is over and we're on a statement in the
                // outer module.  We just want to consume and move on.
                !(self.token == SyntaxKind::SemicolonToken && in_error_recovery)
                    && self.is_start_of_statement()
            }
            PC::SwitchClauses => {
                self.token == SyntaxKind::CaseKeyword || self.token == SyntaxKind::DefaultKeyword
            }
            PC::TypeMembers => self.look_ahead(Parser::scan_type_member_start),
            PC::ClassMembers => {
                // We allow semicolons as class elements (as specified by ES6) as long as we're
                // not in error recovery.  If we're in error recovery, we don't want an errant
                // semicolon to be treated as a class member (since they're almost always used
                // for statements.
                self.look_ahead(Parser::scan_class_member_start)
                    || self.token == SyntaxKind::SemicolonToken && !in_error_recovery
            }
            PC::EnumMembers => {
                // Include open bracket computed properties. This technically also lets in indexers,
                // which would be a candidate for improved error reporting.
                self.token == SyntaxKind::OpenBracketToken || self.is_literal_property_name()
            }
            PC::ObjectLiteralMembers => match self.token {
                // Not an object literal member, but don't want to close the object (see `tests/cases/fourslash/completionsDotInObjectLiteral.ts`)
                SyntaxKind::OpenBracketToken
                | SyntaxKind::AsteriskToken
                | SyntaxKind::DotDotDotToken
                | SyntaxKind::DotToken => true,
                _ => self.is_literal_property_name(),
            },
            PC::RestProperties => self.is_literal_property_name(),
            PC::ObjectBindingElements => {
                self.token == SyntaxKind::OpenBracketToken
                    || self.token == SyntaxKind::DotDotDotToken
                    || self.is_literal_property_name()
            }
            PC::ImportAttributes => self.is_import_attribute_name(),
            PC::HeritageClauseElement => {
                // If we see `{ ... }` then only consume it as an expression if it is followed by `,` or `{`
                // That way we won't consume the body of a class in its heritage clause.
                if self.token == SyntaxKind::OpenBraceToken {
                    return self.is_valid_heritage_clause_object_literal();
                }
                if !in_error_recovery {
                    return self.is_start_of_left_hand_side_expression()
                        && !self.is_heritage_clause_extends_or_implements_keyword();
                }
                // If we're in error recovery we tighten up what we're willing to match.
                // That way we don't treat something like "this" as a valid heritage clause
                // element during recovery.
                self.is_identifier() && !self.is_heritage_clause_extends_or_implements_keyword()
            }
            PC::VariableDeclarations => {
                self.is_binding_identifier_or_private_identifier_or_pattern()
            }
            PC::ArrayBindingElements => {
                self.token == SyntaxKind::CommaToken
                    || self.token == SyntaxKind::DotDotDotToken
                    || self.is_binding_identifier_or_private_identifier_or_pattern()
            }
            PC::TypeParameters => {
                self.token == SyntaxKind::InKeyword
                    || self.token == SyntaxKind::ConstKeyword
                    || self.is_identifier()
            }
            PC::ArrayLiteralMembers | PC::ArgumentExpressions => {
                // Not an array literal member, but don't want to close the array (see `tests/cases/fourslash/completionsDotInArrayLiteralInObjectLiteral.ts`)
                if parsing_context == PC::ArrayLiteralMembers
                    && (self.token == SyntaxKind::CommaToken || self.token == SyntaxKind::DotToken)
                {
                    return true;
                }
                // Go: fallthrough to PCArgumentExpressions
                self.token == SyntaxKind::DotDotDotToken || self.is_start_of_expression()
            }
            PC::Parameters => self.is_start_of_parameter(false /*isJSDocParameter*/),
            PC::JsDocParameters => self.is_start_of_parameter(true /*isJSDocParameter*/),
            PC::TypeArguments | PC::TupleElementTypes => {
                self.token == SyntaxKind::CommaToken
                    || self.is_start_of_type(false /*inStartOfParameter*/)
            }
            PC::HeritageClauses => self.is_heritage_clause(),
            PC::ImportOrExportSpecifiers => {
                // bail out if the next token is [FromKeyword StringLiteral].
                // That means we're in something like `import { from "mod"`. Stop here can give better error message.
                if self.token == SyntaxKind::FromKeyword
                    && self.look_ahead(Parser::next_token_is_token_string_literal)
                {
                    return false;
                }
                if self.token == SyntaxKind::StringLiteral {
                    return true; // For "arbitrary module namespace identifiers"
                }
                token_is_identifier_or_keyword(self.token)
            }
            PC::JsxAttributes => {
                token_is_identifier_or_keyword(self.token)
                    || self.token == SyntaxKind::OpenBraceToken
            }
            PC::JsxChildren => true,
            PC::JsDocComment => true,
            PC::Count => panic!("Unhandled case in isListElement"),
        }
    }

    // Go: parser.go:916 isListTerminator
    pub fn is_list_terminator(&mut self, kind: ParsingContext) -> bool {
        use ParsingContext as PC;
        if self.token == SyntaxKind::EndOfFile {
            return true;
        }
        match kind {
            PC::BlockStatements
            | PC::SwitchClauses
            | PC::TypeMembers
            | PC::ClassMembers
            | PC::EnumMembers
            | PC::ObjectLiteralMembers
            | PC::ObjectBindingElements
            | PC::ImportOrExportSpecifiers
            | PC::ImportAttributes => self.token == SyntaxKind::CloseBraceToken,
            PC::SwitchClauseStatements => {
                self.token == SyntaxKind::CloseBraceToken
                    || self.token == SyntaxKind::CaseKeyword
                    || self.token == SyntaxKind::DefaultKeyword
            }
            PC::HeritageClauseElement => {
                self.token == SyntaxKind::OpenBraceToken
                    || self.token == SyntaxKind::ExtendsKeyword
                    || self.token == SyntaxKind::ImplementsKeyword
            }
            PC::VariableDeclarations => {
                // If we can consume a semicolon (either explicitly, or with ASI), then consider us done
                // with parsing the list of variable declarators.
                // In the case where we're parsing the variable declarator of a 'for-in' statement, we
                // are done if we see an 'in' keyword in front of us. Same with for-of
                // ERROR RECOVERY TWEAK:
                // For better error recovery, if we see an '=>' then we just stop immediately.  We've got an
                // arrow function here and it's going to be very unlikely that we'll resynchronize and get
                // another variable declaration.
                self.can_parse_semicolon()
                    || self.token == SyntaxKind::InKeyword
                    || self.token == SyntaxKind::OfKeyword
                    || self.token == SyntaxKind::EqualsGreaterThanToken
            }
            PC::TypeParameters => {
                // Tokens other than '>' are here for better error recovery
                self.token == SyntaxKind::GreaterThanToken
                    || self.token == SyntaxKind::OpenParenToken
                    || self.token == SyntaxKind::OpenBraceToken
                    || self.token == SyntaxKind::ExtendsKeyword
                    || self.token == SyntaxKind::ImplementsKeyword
            }
            PC::ArgumentExpressions => {
                // Tokens other than ')' are here for better error recovery
                self.token == SyntaxKind::CloseParenToken
                    || self.token == SyntaxKind::SemicolonToken
            }
            PC::ArrayLiteralMembers | PC::TupleElementTypes | PC::ArrayBindingElements => {
                self.token == SyntaxKind::CloseBracketToken
            }
            PC::JsDocParameters | PC::Parameters | PC::RestProperties => {
                // Tokens other than ')' and ']' (the latter for index signatures) are here for better error recovery
                self.token == SyntaxKind::CloseParenToken
                    || self.token == SyntaxKind::CloseBracketToken /*|| token == ast.KindOpenBraceToken*/
            }
            PC::TypeArguments => {
                // All other tokens should cause the type-argument to terminate except comma token
                self.token != SyntaxKind::CommaToken
            }
            PC::HeritageClauses => {
                self.token == SyntaxKind::OpenBraceToken
                    || self.token == SyntaxKind::CloseBraceToken
            }
            PC::JsxAttributes => {
                self.token == SyntaxKind::GreaterThanToken || self.token == SyntaxKind::SlashToken
            }
            PC::JsxChildren => {
                self.token == SyntaxKind::LessThanToken
                    && self.look_ahead(Parser::next_token_is_slash)
            }
            _ => false,
        }
    }

    // Go: parser.go:962 parseExpectedJSDoc
    pub fn parse_expected_js_doc(&mut self, kind: SyntaxKind) -> bool {
        if self.token == kind {
            self.next_token_js_doc();
            return true;
        }
        if !is_keyword_or_punctuation(kind) {
            panic!("Invalid JSDoc kind: expected keyword or punctuation");
        }
        self.parse_error_at_current_token(diag::X_0_expected, args![token_to_string(kind)]);
        false
    }

    // Go: parser.go:974 parseExpectedMatchingBrackets
    pub fn parse_expected_matching_brackets(
        &mut self,
        open_kind: SyntaxKind,
        close_kind: SyntaxKind,
        open_parsed: bool,
        open_position: i32,
    ) {
        if self.token == close_kind {
            self.next_token();
            return;
        }
        let last_error = self
            .parse_error_at_current_token(diag::X_0_expected, args![token_to_string(close_kind)]);
        if !open_parsed {
            return;
        }
        if let Some(last_error) = last_error {
            let related = new_diagnostic(
                Node::NIL,
                TextRange::new(open_position, open_position),
                diag::The_parser_expected_to_find_a_1_to_match_the_0_token_here,
                args![token_to_string(open_kind), token_to_string(close_kind)],
            );
            self.diagnostics.borrow_mut().diagnostics[last_error].add_related_info(Some(related));
        }
    }

    // Go: parser.go:989 parseOptional
    pub fn parse_optional(&mut self, token: SyntaxKind) -> bool {
        if self.token == token {
            self.next_token();
            return true;
        }
        false
    }

    // Go: parser.go:997 parseExpected
    pub fn parse_expected(&mut self, kind: SyntaxKind) -> bool {
        self.parse_expected_with_diagnostic(kind, None, true)
    }

    // Go: parser.go:1001 parseExpectedWithoutAdvancing
    pub fn parse_expected_without_advancing(&mut self, kind: SyntaxKind) -> bool {
        self.parse_expected_with_diagnostic(kind, None, false)
    }

    // Go: parser.go:1005 parseExpectedWithDiagnostic
    // PERF: parsefast2. The matched token is inline; the error is out of
    // line (`parse_expected_failed`).
    #[inline]
    pub fn parse_expected_with_diagnostic(
        &mut self,
        kind: SyntaxKind,
        message: Option<&'static crate::diagnostics::Message>,
        should_advance: bool,
    ) -> bool {
        if self.token == kind {
            if should_advance {
                self.next_token();
            }
            return true;
        }
        self.parse_expected_failed(kind, message)
    }

    /// The error of `parse_expected_with_diagnostic`.
    #[inline(never)]
    fn parse_expected_failed(
        &mut self,
        kind: SyntaxKind,
        message: Option<&'static crate::diagnostics::Message>,
    ) -> bool {
        // Report specific message if provided with one.  Otherwise, report generic fallback message.
        match message {
            Some(message) => {
                self.parse_error_at_current_token(message, args![]);
            }
            None => {
                self.parse_error_at_current_token(diag::X_0_expected, args![token_to_string(kind)]);
            }
        }
        false
    }

    // Go: parser.go:1021 parseTokenNode
    pub fn parse_token_node(&mut self) -> Node {
        let pos = self.node_pos();
        let kind = self.token;
        self.next_token();
        let token = self.factory.new_token(kind);
        self.finish_node(token, pos)
    }

    // Go: parser.go:1028 parseExpectedToken
    pub fn parse_expected_token(&mut self, kind: SyntaxKind) -> Node {
        let mut token = self.parse_optional_token(kind);
        if token.is_nil() {
            self.parse_error_at_current_token(diag::X_0_expected, args![token_to_string(kind)]);
            let pos = self.node_pos();
            let missing = self.factory.new_token(kind);
            token = self.finish_node(missing, pos);
        }
        token
    }

    // Go: parser.go:1037 parseOptionalToken
    pub fn parse_optional_token(&mut self, kind: SyntaxKind) -> Node {
        if self.token == kind {
            return self.parse_token_node();
        }
        Node::NIL
    }

    // Go: parser.go:1044 parseExpectedTokenJSDoc
    pub fn parse_expected_token_js_doc(&mut self, kind: SyntaxKind) -> Node {
        let mut optional = self.parse_optional_token_js_doc(kind);
        if optional.is_nil() {
            if !is_keyword_or_punctuation(kind) {
                panic!("expected keyword or punctuation");
            }
            self.parse_error_at_current_token(diag::X_0_expected, args![token_to_string(kind)]);
            let pos = self.node_pos();
            let missing = self.factory.new_token(kind);
            optional = self.finish_node(missing, pos);
        }
        optional
    }

    // Go: parser.go:1056 parseOptionalTokenJSDoc
    pub fn parse_optional_token_js_doc(&mut self, kind: SyntaxKind) -> Node {
        if self.token == kind {
            return self.parse_token_node();
        }
        Node::NIL
    }

    // Go: parser.go:1063 parseStatement
    pub fn parse_statement(&mut self) -> Node {
        match self.token {
            SyntaxKind::SemicolonToken => return self.parse_empty_statement(),
            SyntaxKind::OpenBraceToken => {
                return self.parse_block(false /*ignoreMissingOpenBrace*/, None);
            }
            SyntaxKind::VarKeyword => {
                let (pos, jsdoc) = (self.node_pos(), self.jsdoc_scanner_info());
                return self.parse_variable_statement(
                    pos,
                    jsdoc,
                    ModifierList::NIL, /*modifiers*/
                );
            }
            SyntaxKind::LetKeyword => {
                if self.is_let_declaration() {
                    let (pos, jsdoc) = (self.node_pos(), self.jsdoc_scanner_info());
                    return self.parse_variable_statement(
                        pos,
                        jsdoc,
                        ModifierList::NIL, /*modifiers*/
                    );
                }
            }
            SyntaxKind::AwaitKeyword => {
                if self.is_await_using_declaration() {
                    let (pos, jsdoc) = (self.node_pos(), self.jsdoc_scanner_info());
                    return self.parse_variable_statement(
                        pos,
                        jsdoc,
                        ModifierList::NIL, /*modifiers*/
                    );
                }
            }
            SyntaxKind::UsingKeyword => {
                if self.is_using_declaration() {
                    let (pos, jsdoc) = (self.node_pos(), self.jsdoc_scanner_info());
                    return self.parse_variable_statement(
                        pos,
                        jsdoc,
                        ModifierList::NIL, /*modifiers*/
                    );
                }
            }
            SyntaxKind::FunctionKeyword => {
                let (pos, jsdoc) = (self.node_pos(), self.jsdoc_scanner_info());
                return self.parse_function_declaration(
                    pos,
                    jsdoc,
                    ModifierList::NIL, /*modifiers*/
                );
            }
            SyntaxKind::ClassKeyword => {
                let (pos, jsdoc) = (self.node_pos(), self.jsdoc_scanner_info());
                return self.parse_class_declaration(
                    pos,
                    jsdoc,
                    ModifierList::NIL, /*modifiers*/
                );
            }
            SyntaxKind::IfKeyword => return self.parse_if_statement(),
            SyntaxKind::DoKeyword => return self.parse_do_statement(),
            SyntaxKind::WhileKeyword => return self.parse_while_statement(),
            SyntaxKind::ForKeyword => return self.parse_for_or_for_in_or_for_of_statement(),
            SyntaxKind::ContinueKeyword => return self.parse_continue_statement(),
            SyntaxKind::BreakKeyword => return self.parse_break_statement(),
            SyntaxKind::ReturnKeyword => return self.parse_return_statement(),
            SyntaxKind::WithKeyword => return self.parse_with_statement(),
            SyntaxKind::SwitchKeyword => return self.parse_switch_statement(),
            SyntaxKind::ThrowKeyword => return self.parse_throw_statement(),
            SyntaxKind::TryKeyword | SyntaxKind::CatchKeyword | SyntaxKind::FinallyKeyword => {
                return self.parse_try_statement();
            }
            SyntaxKind::DebuggerKeyword => return self.parse_debugger_statement(),
            SyntaxKind::AtToken => return self.parse_declaration(),
            SyntaxKind::AsyncKeyword
            | SyntaxKind::InterfaceKeyword
            | SyntaxKind::TypeKeyword
            | SyntaxKind::ModuleKeyword
            | SyntaxKind::NamespaceKeyword
            | SyntaxKind::DeclareKeyword
            | SyntaxKind::ConstKeyword
            | SyntaxKind::EnumKeyword
            | SyntaxKind::ExportKeyword
            | SyntaxKind::ImportKeyword
            | SyntaxKind::PrivateKeyword
            | SyntaxKind::ProtectedKeyword
            | SyntaxKind::PublicKeyword
            | SyntaxKind::AbstractKeyword
            | SyntaxKind::AccessorKeyword
            | SyntaxKind::StaticKeyword
            | SyntaxKind::ReadonlyKeyword
            | SyntaxKind::GlobalKeyword => {
                if self.is_start_of_declaration() {
                    return self.parse_declaration();
                }
            }
            _ => {}
        }
        self.parse_expression_or_labeled_statement()
    }

    // Go: parser.go:1124 parseDeclaration
    pub fn parse_declaration(&mut self) -> Node {
        // `parseListElement` attempted to get the reused node at this position,
        // but the ambient context flag was not yet set, so the node appeared
        // not reusable in that context.
        let pos = self.node_pos();
        let jsdoc = self.jsdoc_scanner_info();
        let modifiers = self.parse_modifiers_ex(
            true,  /*allowDecorators*/
            false, /*permitConstAsModifier*/
            false, /*stopOnStartOfClassStaticBlock*/
        );
        let is_ambient = modifiers.is_some() && modifiers.nodes().iter().any(is_declare_modifier);
        if is_ambient {
            // !!! incremental parsing
            // node := p.tryReuseAmbientDeclaration(pos)
            // if node {
            // 	return node
            // }
            for m in modifiers.nodes().iter() {
                set_node_flags(m, m.flags() | NodeFlags::AMBIENT);
            }
            let save_context_flags = self.context_flags;
            self.set_context_flags(NodeFlags::AMBIENT, true);
            let result = self.parse_declaration_worker(pos, jsdoc, modifiers);
            self.context_flags = save_context_flags;
            result
        } else {
            self.parse_declaration_worker(pos, jsdoc, modifiers)
        }
    }

    // Go: parser.go:1151 parseDeclarationWorker
    pub fn parse_declaration_worker(
        &mut self,
        pos: i32,
        jsdoc: JsdocScannerInfo,
        modifiers: ModifierList,
    ) -> Node {
        match self.token {
            SyntaxKind::VarKeyword
            | SyntaxKind::LetKeyword
            | SyntaxKind::ConstKeyword
            | SyntaxKind::UsingKeyword => {
                return self.parse_variable_statement(pos, jsdoc, modifiers);
            }
            SyntaxKind::AwaitKeyword => {
                if self.is_await_using_declaration() {
                    return self.parse_variable_statement(pos, jsdoc, modifiers);
                }
            }
            SyntaxKind::FunctionKeyword => {
                return self.parse_function_declaration(pos, jsdoc, modifiers);
            }
            SyntaxKind::ClassKeyword => return self.parse_class_declaration(pos, jsdoc, modifiers),
            SyntaxKind::InterfaceKeyword => {
                return self.parse_interface_declaration(pos, jsdoc, modifiers);
            }
            SyntaxKind::TypeKeyword => {
                return self.parse_type_alias_declaration(pos, jsdoc, modifiers);
            }
            SyntaxKind::EnumKeyword => return self.parse_enum_declaration(pos, jsdoc, modifiers),
            SyntaxKind::GlobalKeyword
            | SyntaxKind::ModuleKeyword
            | SyntaxKind::NamespaceKeyword => {
                return self.parse_module_declaration(pos, jsdoc, modifiers);
            }
            SyntaxKind::ImportKeyword => {
                return self
                    .parse_import_declaration_or_import_equals_declaration(pos, jsdoc, modifiers);
            }
            SyntaxKind::ExportKeyword => {
                self.next_token();
                return match self.token {
                    SyntaxKind::DefaultKeyword | SyntaxKind::EqualsToken => {
                        self.parse_export_assignment(pos, jsdoc, modifiers)
                    }
                    SyntaxKind::AsKeyword => {
                        self.parse_namespace_export_declaration(pos, jsdoc, modifiers)
                    }
                    _ => self.parse_export_declaration(pos, jsdoc, modifiers),
                };
            }
            _ => {}
        }
        if modifiers.is_some() {
            // We reached this point because we encountered decorators and/or modifiers and assumed a declaration
            // would follow. For recovery and error reporting purposes, return an incomplete declaration.
            let at = self.node_pos();
            self.parse_error_at(at, at, diag::Declaration_expected, args![]);
            let missing = self.factory.new_missing_declaration(modifiers);
            return self.finish_node(missing, pos);
        }
        panic!("Unhandled case in parseDeclarationWorker");
    }

    // Go: parser.go:1197 isLetDeclaration
    pub fn is_let_declaration(&mut self) -> bool {
        // In ES6 'let' always starts a lexical declaration if followed by an identifier or {
        // or [.
        self.look_ahead(Parser::next_token_is_binding_identifier_or_start_of_destructuring)
    }

    // Go: parser.go:1203 nextTokenIsBindingIdentifierOrStartOfDestructuring
    pub fn next_token_is_binding_identifier_or_start_of_destructuring(&mut self) -> bool {
        self.next_token();
        self.is_binding_identifier()
            || self.token == SyntaxKind::OpenBraceToken
            || self.token == SyntaxKind::OpenBracketToken
    }

    // Go: parser.go:1208 parseBlock
    pub fn parse_block(
        &mut self,
        ignore_missing_open_brace: bool,
        diagnostic_message: Option<&'static crate::diagnostics::Message>,
    ) -> Node {
        let pos = self.node_pos();
        let jsdoc = self.jsdoc_scanner_info();
        let open_brace_position = self.scanner.token_start();
        let open_brace_parsed = self.parse_expected_with_diagnostic(
            SyntaxKind::OpenBraceToken,
            diagnostic_message,
            true, /*shouldAdvance*/
        );
        let mut multiline = false;
        if open_brace_parsed || ignore_missing_open_brace {
            multiline = self.has_preceding_line_break();
            let statements =
                self.parse_list(ParsingContext::BlockStatements, Parser::parse_statement);
            self.parse_expected_matching_brackets(
                SyntaxKind::OpenBraceToken,
                SyntaxKind::CloseBraceToken,
                open_brace_parsed,
                open_brace_position,
            );
            let block = self.factory.new_block(statements, multiline);
            let result = self.finish_node(block, pos);
            self.with_js_doc(result, jsdoc);
            if self.token == SyntaxKind::EqualsToken {
                self.parse_error_at_current_token(diag::Declaration_or_statement_expected_This_follows_a_block_of_statements_so_if_you_intended_to_write_a_destructuring_assignment_you_might_need_to_wrap_the_whole_assignment_in_parentheses, args![]);
                self.next_token();
            }
            return result;
        }
        let missing = self.create_missing_list();
        let block = self.factory.new_block(missing, multiline);
        let result = self.finish_node(block, pos);
        self.with_js_doc(result, jsdoc);
        result
    }

    // Go: parser.go:1231 parseEmptyStatement
    pub fn parse_empty_statement(&mut self) -> Node {
        let pos = self.node_pos();
        let jsdoc = self.jsdoc_scanner_info();
        self.parse_expected(SyntaxKind::SemicolonToken);
        let statement = self.factory.new_empty_statement();
        let result = self.finish_node(statement, pos);
        self.with_js_doc(result, jsdoc);
        result
    }

    // Go: parser.go:1240 parseIfStatement
    pub fn parse_if_statement(&mut self) -> Node {
        let pos = self.node_pos();
        let jsdoc = self.jsdoc_scanner_info();
        self.parse_expected(SyntaxKind::IfKeyword);
        let open_paren_position = self.scanner.token_start();
        let open_paren_parsed = self.parse_expected(SyntaxKind::OpenParenToken);
        let expression = self.parse_expression_allow_in();
        self.parse_expected_matching_brackets(
            SyntaxKind::OpenParenToken,
            SyntaxKind::CloseParenToken,
            open_paren_parsed,
            open_paren_position,
        );
        let then_statement = self.parse_statement();
        let mut else_statement = Node::NIL;
        if self.parse_optional(SyntaxKind::ElseKeyword) {
            else_statement = self.parse_statement();
        }
        let statement = self
            .factory
            .new_if_statement(expression, then_statement, else_statement);
        let result = self.finish_node(statement, pos);
        self.with_js_doc(result, jsdoc);
        result
    }

    // Go: parser.go:1258 parseDoStatement
    pub fn parse_do_statement(&mut self) -> Node {
        let pos = self.node_pos();
        let jsdoc = self.jsdoc_scanner_info();
        self.parse_expected(SyntaxKind::DoKeyword);
        let statement = self.parse_statement();
        self.parse_expected(SyntaxKind::WhileKeyword);
        let open_paren_position = self.scanner.token_start();
        let open_paren_parsed = self.parse_expected(SyntaxKind::OpenParenToken);
        let expression = self.parse_expression_allow_in();
        self.parse_expected_matching_brackets(
            SyntaxKind::OpenParenToken,
            SyntaxKind::CloseParenToken,
            open_paren_parsed,
            open_paren_position,
        );
        // From: https://mail.mozilla.org/pipermail/es-discuss/2011-August/016188.html
        // 157 min --- All allen at wirfs-brock.com CONF --- "do{;}while(false)false" prohibited in
        // spec but allowed in consensus reality. Approved -- this is the de-facto standard whereby
        //  do;while(0)x will have a semicolon inserted before x.
        self.parse_optional(SyntaxKind::SemicolonToken);
        let node = self.factory.new_do_statement(statement, expression);
        let result = self.finish_node(node, pos);
        self.with_js_doc(result, jsdoc);
        result
    }

    // Go: parser.go:1278 parseWhileStatement
    pub fn parse_while_statement(&mut self) -> Node {
        let pos = self.node_pos();
        let jsdoc = self.jsdoc_scanner_info();
        self.parse_expected(SyntaxKind::WhileKeyword);
        let open_paren_position = self.scanner.token_start();
        let open_paren_parsed = self.parse_expected(SyntaxKind::OpenParenToken);
        let expression = self.parse_expression_allow_in();
        self.parse_expected_matching_brackets(
            SyntaxKind::OpenParenToken,
            SyntaxKind::CloseParenToken,
            open_paren_parsed,
            open_paren_position,
        );
        let statement = self.parse_statement();
        let node = self.factory.new_while_statement(expression, statement);
        let result = self.finish_node(node, pos);
        self.with_js_doc(result, jsdoc);
        result
    }

    // Go: parser.go:1292 parseForOrForInOrForOfStatement
    pub fn parse_for_or_for_in_or_for_of_statement(&mut self) -> Node {
        let pos = self.node_pos();
        let jsdoc = self.jsdoc_scanner_info();
        self.parse_expected(SyntaxKind::ForKeyword);
        let await_token = self.parse_optional_token(SyntaxKind::AwaitKeyword);
        self.parse_expected(SyntaxKind::OpenParenToken);
        let mut initializer = Node::NIL;
        if self.token != SyntaxKind::SemicolonToken {
            if self.token == SyntaxKind::VarKeyword
                || self.token == SyntaxKind::LetKeyword
                || self.token == SyntaxKind::ConstKeyword
                || self.token == SyntaxKind::UsingKeyword
                    && self.look_ahead(Parser::next_token_is_binding_identifier_or_start_of_destructuring_on_same_line_disallow_of)
                // this one is meant to allow of
                || self.token == SyntaxKind::AwaitKeyword
                    && self.look_ahead(Parser::next_is_using_keyword_then_binding_identifier_or_start_of_object_destructuring_on_same_line)
            {
                initializer = self.parse_variable_declaration_list(true /*inForStatementInitializer*/);
            } else {
                initializer = self.do_in_context(NodeFlags::DISALLOW_IN_CONTEXT, true, Parser::parse_expression);
            }
        }
        let result;
        if await_token.is_some() && self.parse_expected(SyntaxKind::OfKeyword)
            || await_token.is_nil() && self.parse_optional(SyntaxKind::OfKeyword)
        {
            let expression = self.do_in_context(
                NodeFlags::DISALLOW_IN_CONTEXT,
                false,
                Parser::parse_assignment_expression_or_higher,
            );
            self.parse_expected(SyntaxKind::CloseParenToken);
            let statement = self.parse_statement();
            result = self.factory.new_for_in_or_of_statement(
                SyntaxKind::ForOfStatement,
                await_token,
                initializer,
                expression,
                statement,
            );
        } else if self.parse_optional(SyntaxKind::InKeyword) {
            let expression = self.parse_expression_allow_in();
            self.parse_expected(SyntaxKind::CloseParenToken);
            let statement = self.parse_statement();
            result = self.factory.new_for_in_or_of_statement(
                SyntaxKind::ForInStatement,
                Node::NIL, /*awaitToken*/
                initializer,
                expression,
                statement,
            );
        } else {
            self.parse_expected(SyntaxKind::SemicolonToken);
            let mut condition = Node::NIL;
            if self.token != SyntaxKind::SemicolonToken && self.token != SyntaxKind::CloseParenToken
            {
                condition = self.parse_expression_allow_in();
            }
            self.parse_expected(SyntaxKind::SemicolonToken);
            let mut incrementor = Node::NIL;
            if self.token != SyntaxKind::CloseParenToken {
                incrementor = self.parse_expression_allow_in();
            }
            self.parse_expected(SyntaxKind::CloseParenToken);
            let statement = self.parse_statement();
            result = self
                .factory
                .new_for_statement(initializer, condition, incrementor, statement);
        }
        let result = self.finish_node(result, pos);
        self.with_js_doc(result, jsdoc);
        result
    }

    // Go: parser.go:1338 parseBreakStatement
    pub fn parse_break_statement(&mut self) -> Node {
        let pos = self.node_pos();
        let jsdoc = self.jsdoc_scanner_info();
        self.parse_expected(SyntaxKind::BreakKeyword);
        let label = self.parse_identifier_unless_at_semicolon();
        self.parse_semicolon();
        let node = self.factory.new_break_statement(label);
        let result = self.finish_node(node, pos);
        self.with_js_doc(result, jsdoc);
        result
    }

    // Go: parser.go:1349 parseContinueStatement
    pub fn parse_continue_statement(&mut self) -> Node {
        let pos = self.node_pos();
        let jsdoc = self.jsdoc_scanner_info();
        self.parse_expected(SyntaxKind::ContinueKeyword);
        let label = self.parse_identifier_unless_at_semicolon();
        self.parse_semicolon();
        let node = self.factory.new_continue_statement(label);
        let result = self.finish_node(node, pos);
        self.with_js_doc(result, jsdoc);
        result
    }

    // Go: parser.go:1360 parseIdentifierUnlessAtSemicolon
    pub fn parse_identifier_unless_at_semicolon(&mut self) -> Node {
        if !self.can_parse_semicolon() {
            return self.parse_identifier();
        }
        Node::NIL
    }

    // Go: parser.go:1367 parseReturnStatement
    pub fn parse_return_statement(&mut self) -> Node {
        let pos = self.node_pos();
        let jsdoc = self.jsdoc_scanner_info();
        self.parse_expected(SyntaxKind::ReturnKeyword);
        let mut expression = Node::NIL;
        if !self.can_parse_semicolon() {
            expression = self.parse_expression_allow_in();
        }
        self.parse_semicolon();
        let node = self.factory.new_return_statement(expression);
        let result = self.finish_node(node, pos);
        self.with_js_doc(result, jsdoc);
        result
    }

    // Go: parser.go:1381 parseWithStatement
    pub fn parse_with_statement(&mut self) -> Node {
        let pos = self.node_pos();
        let jsdoc = self.jsdoc_scanner_info();
        self.parse_expected(SyntaxKind::WithKeyword);
        let open_paren_position = self.scanner.token_start();
        let open_paren_parsed = self.parse_expected(SyntaxKind::OpenParenToken);
        let expression = self.parse_expression_allow_in();
        self.parse_expected_matching_brackets(
            SyntaxKind::OpenParenToken,
            SyntaxKind::CloseParenToken,
            open_paren_parsed,
            open_paren_position,
        );
        let statement =
            self.do_in_context(NodeFlags::IN_WITH_STATEMENT, true, Parser::parse_statement);
        let node = self.factory.new_with_statement(expression, statement);
        let result = self.finish_node(node, pos);
        self.with_js_doc(result, jsdoc);
        result
    }

    // Go: parser.go:1395 parseCaseClause
    pub fn parse_case_clause(&mut self) -> Node {
        let pos = self.node_pos();
        let jsdoc = self.jsdoc_scanner_info();
        self.parse_expected(SyntaxKind::CaseKeyword);
        let expression = self.parse_expression_allow_in();
        self.parse_expected(SyntaxKind::ColonToken);
        let statements = self.parse_list(
            ParsingContext::SwitchClauseStatements,
            Parser::parse_statement,
        );
        let node =
            self.factory
                .new_case_or_default_clause(SyntaxKind::CaseClause, expression, statements);
        let result = self.finish_node(node, pos);
        self.with_js_doc(result, jsdoc);
        result
    }
}

// Go: parser.go:318 scanError
// PORT: Go `p.scanError` is a method. The scanner callback cannot borrow the
// parser, so it writes the shared `ParseDiagnostics` (see there).
fn scan_error(
    diagnostics: &Rc<RefCell<ParseDiagnostics>>,
    message: &'static crate::diagnostics::Message,
    pos: i32,
    length: i32,
    args: Vec<String>,
) {
    diagnostics
        .borrow_mut()
        .parse_error_at_range(TextRange::new(pos, pos + length), message, args);
}

// Go: parser.go:227 getErrorSpanForNode
#[must_use]
pub fn get_error_span_for_node(source_text: &str, node: Node) -> TextRange {
    let mut pos = node.pos();
    if !node_is_missing(node) {
        pos = skip_trivia(source_text, pos);
    }
    TextRange::new(pos, node.end())
}

// Go: parser.go:264 isDoubleQuotedString
#[must_use]
pub fn is_double_quoted_string(node: Node) -> bool {
    is_string_literal(node) && !node.token_flags().intersects(TokenFlags::SINGLE_QUOTE)
}

// Go: parser.go:282 ParseIsolatedEntityName
// PORT: the nodes use the synthetic factory of `new_parser`, as there is
// no source file.
#[must_use]
pub fn parse_isolated_entity_name(text: &str) -> Node {
    let mut p = new_parser();
    p.initialize_state(&SourceFileParseOptions::default(), text, ScriptKind::JS);
    p.next_token();
    let entity_name = p.parse_entity_name(true, false, None);
    if p.token == SyntaxKind::EndOfFile && p.diagnostics.borrow().diagnostics.is_empty() {
        entity_name
    } else {
        Node::NIL
    }
}

// Go: parser.go:341 ParserState
#[derive(Clone, Copy, Debug)]
pub struct ParserState<'a> {
    pub scanner_state: ScannerState<'a>,
    pub context_flags: NodeFlags,
    pub diagnostics_len: usize,
    pub js_diagnostics_len: usize,
    pub jsdoc_infos_len: usize,
    pub reparsed_clones_len: usize,
    pub statement_has_await_identifier: bool,
    pub has_parse_error: bool,
}

// Go: parser.go:1193 isDeclareModifier
#[must_use]
pub fn is_declare_modifier(modifier: Node) -> bool {
    modifier.kind() == SyntaxKind::DeclareKeyword
}

#[cfg(test)]
mod debug_site_tests {
    use super::new_parser;
    use crate::core::go_panic_text;

    // Go `isInSomeParsingContext` (parser.go:744) runs
    // `debug.Assert(p.parsingContexts != 0, "Missing parsing context")`.
    #[test]
    fn missing_parsing_context_is_a_go_debug_failure() {
        let text = go_panic_text(|| {
            new_parser().is_in_some_parsing_context();
        });
        assert_eq!(
            text,
            "Debug failure. False expression: Missing parsing context"
        );
    }
}
