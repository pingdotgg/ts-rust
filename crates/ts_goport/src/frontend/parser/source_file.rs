//! Port of the parser fields of Go `ast.SourceFile` (`ast/ast.go:2467`).
//!
//! The Go parser returns a `*ast.SourceFile`, which is a node with extra
//! fields. Here the node lives in the node store of the file (`root`), and
//! the extra fields live in `ParsedSourceFile`. The parser (parser.go),
//! references.go and parseoptions.go set them.

use crate::frontend::prelude::*;

/// The Go `ast.SourceFile` fields that the parser sets.
// PORT: Go keeps these fields on the `SourceFile` node data. astdata cannot
// hold them, so the parser returns them next to the root node. The binder,
// ECMA line map and language service fields of Go `SourceFile` are not here:
// the parser does not set them. The Go mutexes are not needed (one thread).
// Go setters (`SetDiagnostics`, `SetJSDocCache`, ...) are field writes.
#[derive(Clone, Debug)]
pub struct ParsedSourceFile {
    /// Node store id of the file (`new_file_store`).
    pub store: usize,
    /// The `SourceFile` node.
    pub root: Node,

    // Fields set by NewSourceFile
    pub parse_options: SourceFileParseOptions,
    /// The file text, shared with the store of the file (`FileText`).
    pub text: FileText,
    pub end_of_file_token: Node,
    /// Go `SourceFile.Hash`: set by the language server parse caches
    /// (project/parsecache.go, compilerhost.go), else `None`. Read it with
    /// `source_hash`.
    pub hash: std::cell::Cell<Option<u128>>,

    // Fields set by parser
    pub diagnostics: Vec<Diagnostic>,
    pub js_diagnostics: Vec<Diagnostic>,
    pub jsdoc_diagnostics: Vec<Diagnostic>,
    pub language_variant: LanguageVariant,
    pub script_kind: ScriptKind,
    pub is_declaration_file: bool,
    pub uses_uri_style_node_core_modules: Tristate,
    pub identifier_count: i32,
    pub imports: Vec<Node>,
    pub module_augmentations: Vec<Node>,
    pub ambient_module_names: Vec<String>,
    pub comment_directives: Vec<CommentDirective>,
    pub jsdoc_cache: FxHashMap<Node, Vec<Node>>,
    pub has_lazy_js_doc: bool,
    pub reparsed_clones: Vec<Node>,
    pub pragmas: Vec<Pragma>,
    pub referenced_files: Vec<FileReference>,
    pub type_reference_directives: Vec<FileReference>,
    pub lib_reference_directives: Vec<FileReference>,
    pub check_js_directive: Option<CheckJsDirective>,
    pub node_count: usize,
    pub text_count: usize,
    pub common_js_module_indicator: Node,
    /// If this is the SourceFile itself, then this module was "forced"
    /// to be an external module (previously "true").
    pub external_module_indicator: Node,
    /// The owner of this file version when it can be freed: set by the
    /// language server parse cache for a path that was published before
    /// (`ast::freeable_path`). Every holder of the parse keeps the version
    /// alive (lsshells M3a).
    // PORT: no Go field. Go's GC frees the `SourceFile` when no program and
    // no parse cache entry holds it.
    pub version: std::cell::OnceCell<std::sync::Arc<crate::ast::FileVersion>>,
}

impl ParsedSourceFile {
    /// The fields that Go `NewSourceFile` sets. `root` is the node that
    /// `NodeFactory::new_source_file` made in store `store`.
    // PORT: Go `NewSourceFile` makes the node and these fields in one call.
    #[must_use]
    pub fn new(
        store: usize,
        root: Node,
        parse_options: SourceFileParseOptions,
        text: FileText,
        end_of_file_token: Node,
    ) -> Self {
        Self {
            store,
            root,
            parse_options,
            text,
            end_of_file_token,
            hash: std::cell::Cell::new(None),
            diagnostics: Vec::new(),
            js_diagnostics: Vec::new(),
            jsdoc_diagnostics: Vec::new(),
            language_variant: LanguageVariant::default(),
            script_kind: ScriptKind::default(),
            is_declaration_file: false,
            uses_uri_style_node_core_modules: Tristate::Unknown,
            identifier_count: 0,
            imports: Vec::new(),
            module_augmentations: Vec::new(),
            ambient_module_names: Vec::new(),
            comment_directives: Vec::new(),
            jsdoc_cache: FxHashMap::default(),
            has_lazy_js_doc: false,
            reparsed_clones: Vec::new(),
            pragmas: Vec::new(),
            referenced_files: Vec::new(),
            type_reference_directives: Vec::new(),
            lib_reference_directives: Vec::new(),
            check_js_directive: None,
            node_count: 0,
            text_count: 0,
            common_js_module_indicator: Node::NIL,
            external_module_indicator: Node::NIL,
            version: std::cell::OnceCell::new(),
        }
    }

    /// Moves the node handles of a detached parse to the real store id
    /// (`adopt_detached_parse`).
    pub fn remap_store(&mut self, remap: StoreRemap) {
        let node = |n: &mut Node| *n = remap.node(*n);
        self.store = remap.store();
        node(&mut self.root);
        node(&mut self.end_of_file_token);
        node(&mut self.common_js_module_indicator);
        node(&mut self.external_module_indicator);
        self.imports.iter_mut().for_each(node);
        self.module_augmentations.iter_mut().for_each(node);
        self.reparsed_clones.iter_mut().for_each(node);
        for diagnostics in [
            &mut self.diagnostics,
            &mut self.js_diagnostics,
            &mut self.jsdoc_diagnostics,
        ] {
            for d in diagnostics {
                remap_diagnostic(d, remap);
            }
        }
        self.jsdoc_cache = std::mem::take(&mut self.jsdoc_cache)
            .into_iter()
            .map(|(key, mut jsdocs)| {
                jsdocs.iter_mut().for_each(node);
                (remap.node(key), jsdocs)
            })
            .collect();
    }

    // Go: ast/ast.go:2539 ParseOptions
    #[must_use]
    pub fn parse_options(&self) -> &SourceFileParseOptions {
        &self.parse_options
    }

    // Go: ast/ast.go:2566 Text
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Go `file.Hash` where the language server reads it: the hash that a
    /// parse cache set (`hash`), else the xxh3-128 of the text, which is Go
    /// `fh.Hash()` for a file that the parse cache made.
    #[must_use]
    pub fn source_hash(&self) -> u128 {
        self.hash
            .get()
            .unwrap_or_else(|| xxhash_rust::xxh3::xxh3_128(self.text.as_bytes()))
    }

    // Go: ast/ast.go:2703 FileName
    #[must_use]
    pub fn file_name(&self) -> &str {
        &self.parse_options.file_name
    }

    // Go: ast/ast.go:2705 Path (at 673a5f17d713; ts#64159 renames it PathKey, ast/ast.go:2707)
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.parse_options.path
    }

    /// Go `node.Statements`.
    #[must_use]
    pub fn statements(&self) -> NodeList {
        self.root.statement_list()
    }

    // Go: ast/ast.go:2788 IsJS (IsSourceFileJS, ast/utilities.go:1294)
    // Go tests the script kind, not the JAVA_SCRIPT_FILE flag: the parser
    // also sets that flag on JSON files (parser.go:309), and a JSON file
    // must not get the implicit jsx-runtime and tslib imports.
    #[must_use]
    pub fn is_js(&self) -> bool {
        self.script_kind == ScriptKind::JS || self.script_kind == ScriptKind::JSX
    }

    // ── Content mapper info (tsgo#4712) ────────────────────────────────
    // PORT: Go keeps `contentMapperInfo` on the SourceFile. Here the node
    // side (`ContentMapperFileInfo`) is in the process table of `crate::ast`,
    // keyed by the file id and the file name address of its store (see
    // `content_mapper_key` there), and the `Rc` links are in
    // `CONTENT_MAPPER_LINKS` of this thread (an `Rc<ParsedSourceFile>` stays
    // on its thread). A clone of a `ParsedSourceFile` has the same store, so
    // it has the same info, as a Go copy of the pointer does.

    // Go: ast/ast.go:2548 OriginalText
    // OriginalText returns the untransformed source text for content-mapped files, or Text() otherwise.
    #[must_use]
    pub fn original_text(&self) -> &str {
        match source_file_content_mapper_info(self.root) {
            Some(info) if !info.content_mapper.is_empty() => &info.original_text,
            _ => &self.text,
        }
    }

    // Go: ast/ast.go:2556 OriginalFileName
    // OriginalFileName returns the canonical filename associated with a supplemental source file, or FileName() otherwise.
    // PORT: reads the canonical SourceFile node, so it does not depend on
    // the canonical `ParsedSourceFile` being alive (see
    // `canonical_source_file`).
    #[must_use]
    pub fn original_file_name(&self) -> &str {
        let canonical = source_file_canonical_source_file(self.root);
        if canonical.is_some() {
            return source_file_file_name(canonical);
        }
        self.file_name()
    }

    // Go: ast/ast.go:2566 SpanMap
    // SpanMap returns the span map that maps positions in this file's transformed Text() back to its
    // original, untransformed content, or nil if the file is not content-mapped (or is a failure stub).
    // The returned map is nil-safe: a nil map maps positions identically.
    #[must_use]
    pub fn span_map(&self) -> Option<&'static crate::spanmap::SpanMap> {
        source_file_span_map(self.root)
    }

    // Go: ast/ast.go:2575 ContentMapper
    // ContentMapper returns the identity of the content mapper that produced this file, or "" if the file
    // was not produced by a content mapper (or the mapper did not identify itself).
    #[must_use]
    pub fn content_mapper(&self) -> &'static str {
        source_file_content_mapper(self.root)
    }

    // Go: ast/ast.go:2584 IsContentMapperFailureStub
    // IsContentMapperFailureStub reports whether this file is the empty placeholder produced when a content
    // mapper's transform failed.
    #[must_use]
    pub fn is_content_mapper_failure_stub(&self) -> bool {
        source_file_is_content_mapper_failure_stub(self.root)
    }

    // Go: ast/ast.go:2588 ContentMapperTransformIdentity
    #[must_use]
    pub fn content_mapper_transform_identity(&self) -> &'static str {
        source_file_content_mapper_transform_identity(self.root)
    }

    // Go: ast/ast.go:2595 VirtualFileName
    #[must_use]
    pub fn virtual_file_name(&self) -> &'static str {
        source_file_virtual_file_name(self.root)
    }

    // Go: ast/ast.go:2631 ContentMapperParseOptions
    // ContentMapperParseOptions returns the parse options used to acquire this file from the mapped parse cache.
    #[must_use]
    pub fn content_mapper_parse_options(&self) -> &'static SourceFileParseOptions {
        source_file_content_mapper_parse_options(self.root)
    }

    // Go: ast/ast.go:2639 SetContentMapperInfo
    // SetContentMapperInfo initializes all content-mapper metadata before the source file is published.
    // PORT: takes `&self`, because callers hold the file in an `Rc`. Panics
    // when the info is already set, as Go does.
    // PORT: the canonical and supplemental files point at each other. The
    // canonical file owns its supplemental files (`Rc`), and a supplemental
    // file points back with a `Weak`, so the links make no `Rc` cycle.
    pub fn set_content_mapper_info(&self, info: ContentMapperSourceFileInfo) {
        let ContentMapperSourceFileInfo {
            content_mapper,
            transform_identity,
            parse_options,
            virtual_file_name,
            original_text,
            span_map,
            diagnostic_directives,
            supplemental_source_files,
            canonical_source_file,
        } = info;
        set_source_file_content_mapper_info(
            self.root,
            ContentMapperFileInfo {
                content_mapper,
                transform_identity,
                parse_options,
                virtual_file_name,
                original_text,
                span_map,
                diagnostic_directives,
                supplemental_source_files: supplemental_source_files
                    .iter()
                    .map(|file| file.root)
                    .collect(),
                canonical_source_file: canonical_source_file
                    .as_ref()
                    .map_or(Node::NIL, |file| file.root),
            },
        );
        let links = ContentMapperLinks {
            supplemental_source_files,
            canonical_source_file: canonical_source_file
                .as_ref()
                .map_or_else(std::rc::Weak::new, Rc::downgrade),
        };
        CONTENT_MAPPER_LINKS.with(|m| m.borrow_mut().insert(self.root.file_index(), links));
    }

    // Go: ast/ast.go:2646 DiagnosticDirectives
    #[must_use]
    pub fn diagnostic_directives(&self) -> &'static [MappedDiagnosticDirective] {
        source_file_diagnostic_directives(self.root)
    }

    // Go: ast/ast.go:2654 SupplementalSourceFiles
    // SupplementalSourceFiles returns the additional outputs produced from this canonical source file.
    // PORT: returns a copy of the `Rc` list (the list is in a thread table).
    #[must_use]
    pub fn supplemental_source_files(&self) -> Vec<Rc<ParsedSourceFile>> {
        CONTENT_MAPPER_LINKS.with(|m| {
            m.borrow()
                .get(&self.root.file_index())
                .map(|links| links.supplemental_source_files.clone())
                .unwrap_or_default()
        })
    }

    // Go: ast/ast.go:2662 CanonicalSourceFile
    // CanonicalSourceFile returns the canonical output associated with this supplemental source file.
    // PORT: the link is a `Weak` (see `set_content_mapper_info`), so this is
    // `None` after the canonical file is dropped. A supplemental file is
    // reached through its canonical file or a program that holds both, so a
    // reader does not see that case.
    #[must_use]
    pub fn canonical_source_file(&self) -> Option<Rc<ParsedSourceFile>> {
        CONTENT_MAPPER_LINKS.with(|m| {
            m.borrow()
                .get(&self.root.file_index())
                .and_then(|links| links.canonical_source_file.upgrade())
        })
    }

    // Go: ast/ast.go:2670 IsContentMapperSupplemental
    // IsContentMapperSupplemental reports whether this is an unnamed supplemental mapper output.
    // PORT: reads the canonical SourceFile node (see `original_file_name`).
    #[must_use]
    pub fn is_content_mapper_supplemental(&self) -> bool {
        source_file_is_content_mapper_supplemental(self.root)
    }
}

/// The links of a content-mapped file (Go
/// `ContentMapperSourceFileInfo.SupplementalSourceFiles` and
/// `CanonicalSourceFile`).
struct ContentMapperLinks {
    supplemental_source_files: Vec<Rc<ParsedSourceFile>>,
    canonical_source_file: std::rc::Weak<ParsedSourceFile>,
}

thread_local! {
    /// The content mapper links of the parsed files of this thread, by file
    /// id. Like the node-side table, it is never cleared.
    static CONTENT_MAPPER_LINKS: RefCell<FxHashMap<usize, ContentMapperLinks>> =
        RefCell::new(FxHashMap::default());
}

fn remap_diagnostic(d: &mut Diagnostic, remap: StoreRemap) {
    d.file = remap.node(d.file);
    for d in d
        .message_chain
        .iter_mut()
        .chain(d.related_information.iter_mut())
    {
        remap_diagnostic(d, remap);
    }
}

// PORT: Go `(*SourceFile).Diagnostics()` and `SetDiagnostics` read the
// node itself. Code that only holds the `SourceFile` node of a parsed store
// file (the tsconfig parser) reads the diagnostics from this table, keyed by
// store id. `parse_source_file` fills it.
thread_local! {
    static PARSED_FILE_DIAGNOSTICS: RefCell<FxHashMap<usize, &'static [Diagnostic]>> =
        RefCell::new(FxHashMap::default());
}

// Go: ast.go (*SourceFile).SetDiagnostics
pub fn set_source_file_diagnostics(file: Node, diagnostics: Vec<Diagnostic>) {
    let diagnostics: &'static [Diagnostic] = Box::leak(diagnostics.into_boxed_slice());
    PARSED_FILE_DIAGNOSTICS.with(|m| m.borrow_mut().insert(file.file_index(), diagnostics));
}

// Go: ast.go (*SourceFile).Diagnostics, for a parsed store file.
#[must_use]
pub fn parsed_source_file_diagnostics(file: Node) -> &'static [Diagnostic] {
    PARSED_FILE_DIAGNOSTICS.with(|m| m.borrow().get(&file.file_index()).copied().unwrap_or(&[]))
}
