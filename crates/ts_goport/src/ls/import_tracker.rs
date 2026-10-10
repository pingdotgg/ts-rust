//! Port of Go `ls/importTracker.go`.

use crate::ls::prelude::*;

// PORT (whole file):
// - Go `*checker.Checker` parameters are `&mut Checker`. The `ImportTracker`
//   closure takes the checker as its first argument instead of capturing it
//   (map-ls-navigation 2.3), and borrows the source file list and set for
//   the lifetime `'a` of the search state.
// - Go closures that share local state (`getImportersForExport`,
//   `getSearchesFromDirectImports`, `getImportOrExportSymbol`) become a small
//   local struct with methods, or nested functions with explicit parameters.
//   The Go call order is kept.
// - Go `[]*ast.SourceFile` is `&[Node]` (file roots). The source file name
//   set (`*collections.Set[string]`) is `&FxHashSet<String>`.
// - Go `*ExportInfo` that can be nil is `Option<ExportInfo>`; a non-nil
//   parameter is `&ExportInfo`.

use crate::flags_macros::go_enum;
use crate::frontend::tspath;
use crate::gostd::Context;
use std::rc::Weak;

// Go: ls/importTracker.go:16 ImpExpKind
go_enum!(ImpExpKind, i32 {
    UNKNOWN = 0;
    IMPORT = 1;
    EXPORT = 2;
});

// Go: ls/importTracker.go:24 ImportExportSymbol
#[derive(Clone, Copy, Debug)]
pub struct ImportExportSymbol {
    pub kind: ImpExpKind,
    pub symbol: SymbolId,
    pub export_info: Option<ExportInfo>,
}

// Go: ls/importTracker.go:30 ExportKind
go_enum!(ExportKind, i32 {
    NAMED = 0;
    DEFAULT = 1;
    EXPORT_EQUALS = 2;
    UMD = 3;
    MODULE = 4;
});

// Go: ls/importTracker.go:40 ExportInfo
#[derive(Clone, Copy, Debug)]
pub struct ExportInfo {
    pub exporting_module_symbol: SymbolId,
    pub export_kind: ExportKind,
}

// Go: ls/importTracker.go:45 LocationAndSymbol
#[derive(Clone, Copy, Debug)]
pub struct LocationAndSymbol {
    pub import_location: Node,
    pub import_symbol: SymbolId,
}

// Go: ls/importTracker.go:50 ImportsResult
// PORT: Go `indirectUsers []*ast.SourceFile` holds file roots.
#[derive(Clone, Debug, Default)]
pub struct ImportsResult {
    pub import_searches: Vec<LocationAndSymbol>,
    pub single_references: Vec<Node>,
    pub indirect_users: Vec<Node>,
}

// Go: ls/importTracker.go:56 ImportTracker
// PORT: the checker is the first argument (Go captures it). Go returns
// `*ImportsResult`; the result is returned by value. `'a` is the lifetime of
// the source file list and set that the tracker borrows.
pub type ImportTracker<'a> =
    Rc<dyn Fn(&mut Checker, SymbolId, &ExportInfo, bool) -> ImportsResult + 'a>;

// Go: ls/importTracker.go:58 ModuleReferenceKind
go_enum!(ModuleReferenceKind, i32 {
    IMPORT = 0;
    REFERENCE = 1;
    IMPLICIT = 2;
});

// Go: ls/importTracker.go:67 ModuleReference
// ModuleReference represents a reference to a module, either via import, <reference>, or implicit reference
// PORT: Go `ref *ast.FileReference` is a copy of the reference, None for nil.
#[derive(Clone, Debug)]
pub struct ModuleReference {
    pub kind: ModuleReferenceKind,
    pub literal: Node, // for import and implicit kinds (StringLiteralLike)
    pub referencing_file: Node,
    pub ref_: Option<FileReference>, // for reference kind
}

// Go: ls/importTracker.go:75 createImportTracker
// Creates the imports map and returns an ImportTracker that uses it. Call this lazily to avoid calling `getDirectImportsMap` unnecessarily.
// PORT: the returned closure borrows `sourceFiles` and `sourceFilesSet`
// (Go captures the caller's slice and set, which do not change during a
// search). It takes the checker as its first argument.
// PERF: the map comes from `get_direct_imports_map_cached`, which reuses the
// map of an earlier search with the same checker.
pub fn create_import_tracker<'a, P: ProgramView>(
    ctx: &Context,
    program: &P,
    source_files: &'a [Node],
    source_files_set: &'a FxHashSet<String>,
    checker: &mut Checker,
) -> ImportTracker<'a> {
    let all_direct_imports = get_direct_imports_map_cached(ctx, program, source_files, checker);
    Rc::new(
        move |checker: &mut Checker,
              export_symbol: SymbolId,
              export_info: &ExportInfo,
              is_for_rename: bool|
              -> ImportsResult {
            let (direct_imports, indirect_users) = get_importers_for_export(
                source_files,
                source_files_set,
                &all_direct_imports,
                export_info,
                checker,
            );
            let (import_searches, single_references) = get_searches_from_direct_imports(
                &direct_imports,
                export_symbol,
                export_info.export_kind,
                checker,
                is_for_rename,
            );
            ImportsResult {
                import_searches,
                single_references,
                indirect_users,
            }
        },
    )
}

// Go: ls/importTracker.go:85 getDirectImportsMap
// Returns a map from a module symbol to all import statements that directly reference the module
pub fn get_direct_imports_map<P: ProgramView>(
    ctx: &Context,
    program: &P,
    source_files: &[Node],
    checker: &mut Checker,
) -> FxHashMap<SymbolId, Vec<Node>> {
    let mut result: FxHashMap<SymbolId, Vec<Node>> = FxHashMap::default();
    for &source_file in source_files {
        if ctx.err().is_some() {
            return result;
        }
        for_each_import(
            program,
            source_file,
            &mut |import_decl, module_specifier| {
                let module_symbol = checker.get_symbol_at_location_exported(module_specifier);
                if module_symbol.is_some() {
                    result.entry(module_symbol).or_default().push(import_decl);
                }
            },
        );
    }
    result
}

/// A direct imports map that `get_direct_imports_map_cached` keeps for one
/// checker of one program.
struct CachedDirectImportsMap {
    /// `ProgramView::identity` of the program.
    program: usize,
    /// The checker's `identity_relation` (see `get_direct_imports_map_cached`).
    checker: Weak<RefCell<crate::checker::Relation>>,
    map: Rc<FxHashMap<SymbolId, Vec<Node>>>,
}

thread_local! {
    // PORT: a checker stays on the thread that made it: the dispatch thread
    // or a cross-project search thread (search_thread.rs). Each thread keeps
    // the maps of its own checkers.
    static DIRECT_IMPORTS_MAPS: RefCell<Vec<CachedDirectImportsMap>> =
        const { RefCell::new(Vec::new()) };
}

/// `get_direct_imports_map` for a search over all files of `program`, kept
/// for the checker and reused by later searches with the same checker.
// PERF: Go builds the map again for each `refState` that searches imports:
// once per project in each references, rename and code lens request, and
// once per queued node of an implementation search. On a monorepo with many
// projects this is a large part of those requests. The map is the same for
// each build with one checker and one file list: `getSymbolAtLocation` of a
// module specifier (errors off) only reads the program's module resolutions
// and the merged symbols and globals that the checker made at its start. It
// makes no checker state. So a reused map gives the same answers.
// Only the full file list of the program (`provideSymbolsAndEntries`) is
// kept. Other lists (document highlights: one file) build their map as in
// Go. A build that the context cancelled is not kept, and a search whose
// context is already cancelled builds the map as in Go (empty).
// `Checker::id` is the pool slot, so a new checker made after the idle
// disposal has the same id. The checker is known by its `identity_relation`
// Rc instead, which only `Checker::new` makes. The `Weak` keeps that address from being
// used again, and its strong count is 0 after the checker is dropped. The
// maps of dropped checkers are removed at the next call.
fn get_direct_imports_map_cached<P: ProgramView>(
    ctx: &Context,
    program: &P,
    source_files: &[Node],
    checker: &mut Checker,
) -> Rc<FxHashMap<SymbolId, Vec<Node>>> {
    let all_program_files =
        ctx.err().is_none() && source_files == program.source_file_roots().as_slice();
    if !all_program_files {
        return Rc::new(get_direct_imports_map(ctx, program, source_files, checker));
    }
    let checker_key = Rc::as_ptr(&checker.identity_relation);
    let cached = DIRECT_IMPORTS_MAPS.with(|maps| {
        let mut maps = maps.borrow_mut();
        maps.retain(|entry| entry.checker.strong_count() != 0);
        maps.iter()
            .find(|entry| {
                entry.program == program.identity()
                    && std::ptr::eq(entry.checker.as_ptr(), checker_key)
            })
            .map(|entry| Rc::clone(&entry.map))
    });
    if let Some(map) = cached {
        return map;
    }
    let map = Rc::new(get_direct_imports_map(ctx, program, source_files, checker));
    if ctx.err().is_none() {
        DIRECT_IMPORTS_MAPS.with(|maps| {
            maps.borrow_mut().push(CachedDirectImportsMap {
                program: program.identity(),
                checker: Rc::downgrade(&checker.identity_relation),
                map: Rc::clone(&map),
            });
        });
    }
    map
}

// Go: ls/importTracker.go:101 forEachImport
// Calls `action` for each import, re-export, or require() in a file
// PORT: Go `sourceFile.Path()` is the installed program's path of the file
// (`source_file_info(file).path`).
pub fn for_each_import<P: ProgramView>(
    program: &P,
    source_file: Node,
    action: &mut dyn FnMut(Node, Node), /*importStatement, imported*/
) {
    let mut implicit_imports: Vec<Node> = Vec::new();
    let source_file_path = tspath::Path(source_file_info(source_file).path.clone());
    let jsx_specifier = program.jsx_runtime_import_specifier(&source_file_path);
    if jsx_specifier.is_some() {
        implicit_imports.push(jsx_specifier);
    }
    let import_helpers_specifier = program.import_helpers_import_specifier(&source_file_path);
    if import_helpers_specifier.is_some() {
        implicit_imports.push(import_helpers_specifier);
    }
    if source_file_info(source_file)
        .external_module_indicator
        .is_some()
        || source_file_imports(source_file).len() + implicit_imports.len() != 0
    {
        for i in source_file_imports(source_file) {
            action(import_from_module_specifier(i), i);
        }
        for &i in &implicit_imports {
            action(import_from_module_specifier(i), i);
        }
    } else {
        for_each_possible_import_or_export_statement(source_file, &mut |node| {
            match node.kind() {
                SyntaxKind::ExportDeclaration
                | SyntaxKind::ImportDeclaration
                | SyntaxKind::JsImportDeclaration => {
                    let specifier = node.module_specifier();
                    if specifier.is_some() && is_string_literal(specifier) {
                        action(node, specifier);
                    }
                }
                SyntaxKind::ImportEqualsDeclaration => {
                    if is_external_module_import_equals(node) {
                        action(node, node.module_reference().expression());
                    }
                }
                _ => {}
            }
            false
        });
    }
}

// Go: ls/importTracker.go:135 forEachPossibleImportOrExportStatement
pub fn for_each_possible_import_or_export_statement(
    source_file_like: Node,
    action: &mut dyn FnMut(Node) -> bool, /*statement*/
) -> bool {
    for statement in get_statements_of_source_file_like(source_file_like) {
        if action(statement)
            || is_ambient_module_declaration(statement)
                && for_each_possible_import_or_export_statement(statement, &mut *action)
        {
            return true;
        }
    }
    false
}

// Go: ls/importTracker.go:144 getSourceFileLikeForImportDeclaration
pub fn get_source_file_like_for_import_declaration(node: Node) -> Node {
    if is_call_expression(node) || is_js_doc_import_tag(node) {
        return get_source_file_of_node(node);
    }
    let parent = node.parent();
    if is_source_file(parent) {
        return parent;
    }
    crate::go_assert!(is_module_block(parent) && is_ambient_module_declaration(parent.parent()));
    parent.parent()
}

// Go: ls/importTracker.go:156 isAmbientModuleDeclaration
pub fn is_ambient_module_declaration(node: Node) -> bool {
    is_module_declaration(node) && is_string_literal(node.name())
}

// Go: ls/importTracker.go:160 getStatementsOfSourceFileLike
pub fn get_statements_of_source_file_like(node: Node) -> NodeSlice {
    if is_source_file(node) {
        return node.statements();
    }
    let body = node.body();
    if body.is_some() {
        return body.statements();
    }
    NodeSlice::NIL
}

// Go: ls/importTracker.go:170 getImportersForExport
// PORT: the Go closures share `directImports`, `indirectUserDeclarations`
// and the two seen trackers. They are the methods of `ImportersForExport`,
// in Go order.
pub fn get_importers_for_export(
    source_files: &[Node],
    source_files_set: &FxHashSet<String>,
    all_direct_imports: &FxHashMap<SymbolId, Vec<Node>>,
    export_info: &ExportInfo,
    checker: &mut Checker,
) -> (Vec<Node>, Vec<Node>) {
    let mark_seen_direct_import = node_seen_tracker();
    let mark_seen_indirect_user = node_seen_tracker();
    let is_available_through_global = is_source_file_with_global_exports(
        checker
            .sym(export_info.exporting_module_symbol)
            .value_declaration,
    );

    let mut state = ImportersForExport {
        source_files,
        source_files_set,
        all_direct_imports,
        export_info,
        checker,
        direct_imports: Vec::new(),
        indirect_user_declarations: Vec::new(),
        mark_seen_direct_import,
        mark_seen_indirect_user,
        is_available_through_global,
    };

    state.handle_direct_imports(export_info.exporting_module_symbol);
    let direct_imports = std::mem::take(&mut state.direct_imports);
    (direct_imports, state.get_indirect_users())
}

/// The shared locals of Go `getImportersForExport`.
// PORT: `F` is the type `nodeSeenTracker()` returns.
struct ImportersForExport<'a, F: FnMut(Node) -> bool> {
    source_files: &'a [Node],
    source_files_set: &'a FxHashSet<String>,
    all_direct_imports: &'a FxHashMap<SymbolId, Vec<Node>>,
    export_info: &'a ExportInfo,
    checker: &'a mut Checker,
    direct_imports: Vec<Node>,
    indirect_user_declarations: Vec<Node>,
    mark_seen_direct_import: F,
    mark_seen_indirect_user: F,
    is_available_through_global: bool,
}

impl<F: FnMut(Node) -> bool> ImportersForExport<'_, F> {
    // Go: ls/importTracker.go:183 getDirectImports (closure)
    // PORT: returns a copy of the list, because callers add to the state
    // while they walk it (Go shares the slice).
    fn get_direct_imports(&self, module_symbol: SymbolId) -> Vec<Node> {
        self.all_direct_imports
            .get(&module_symbol)
            .cloned()
            .unwrap_or_default()
    }

    // Go: ls/importTracker.go:188 addIndirectUser (closure)
    // Adds a module and all of its transitive dependencies as possible indirect users
    fn add_indirect_user(&mut self, source_file_like: Node, add_transitive_dependencies: bool) {
        // When isAvailableThroughGlobal, getIndirectUsers already returns all source files,
        // so indirectUserDeclarations is never consulted. Nothing to do here.
        if self.is_available_through_global {
            return;
        }
        if !(self.mark_seen_indirect_user)(source_file_like) {
            return;
        }
        self.indirect_user_declarations.push(source_file_like);
        if !add_transitive_dependencies {
            return;
        }
        let module_symbol = self
            .checker
            .get_merged_symbol_exported(source_file_like.symbol());
        if module_symbol.is_nil() {
            return;
        }
        crate::go_assert!(
            self.checker
                .sym(module_symbol)
                .flags
                .intersects(SymbolFlags::MODULE)
        );
        for direct_import in self.get_direct_imports(module_symbol) {
            if !is_import_type_node(direct_import) {
                self.add_indirect_user(
                    get_source_file_like_for_import_declaration(direct_import),
                    true, /*addTransitiveDependencies*/
                );
            }
        }
    }

    // Go: ls/importTracker.go:224 handleImportCall (closure)
    fn handle_import_call(&mut self, import_call: Node) {
        let mut top = find_ancestor(import_call, is_ambient_module_declaration);
        if top.is_nil() {
            top = get_source_file_of_node(import_call);
        }
        self.add_indirect_user(
            top,
            importers_is_exported(import_call, true /*stopAtAmbientModule*/),
        );
    }

    // Go: ls/importTracker.go:232 handleNamespaceImport (closure)
    fn handle_namespace_import(
        &mut self,
        import_declaration: Node,
        name: Node,
        is_re_export: bool,
        already_added_direct: bool,
    ) {
        if self.export_info.export_kind == ExportKind::EXPORT_EQUALS {
            // This is a direct import, not import-as-namespace.
            if !already_added_direct {
                self.direct_imports.push(import_declaration);
            }
        } else if !self.is_available_through_global {
            let source_file_like = get_source_file_like_for_import_declaration(import_declaration);
            crate::go_assert!(
                is_source_file(source_file_like) || is_module_declaration(source_file_like)
            );
            let add_transitive_dependencies =
                is_re_export || find_namespace_re_exports(source_file_like, name, self.checker);
            self.add_indirect_user(source_file_like, add_transitive_dependencies);
        }
    }

    // Go: ls/importTracker.go:245 handleDirectImports (closure)
    fn handle_direct_imports(&mut self, exporting_module_symbol: SymbolId) {
        let these_direct_imports = self.get_direct_imports(exporting_module_symbol);
        for direct in these_direct_imports {
            if !(self.mark_seen_direct_import)(direct) {
                continue;
            }
            // !!! cancellation
            match direct.kind() {
                SyntaxKind::CallExpression => {
                    if is_import_call(direct) {
                        self.handle_import_call(direct);
                    } else if !self.is_available_through_global {
                        let parent = direct.parent();
                        if self.export_info.export_kind == ExportKind::EXPORT_EQUALS
                            && is_variable_declaration(parent)
                        {
                            let name = parent.name();
                            if is_identifier(name) {
                                self.direct_imports.push(name);
                            }
                        }
                    }
                }
                SyntaxKind::Identifier => {
                    // Nothing
                }
                SyntaxKind::ImportEqualsDeclaration => {
                    self.handle_namespace_import(
                        direct,
                        direct.name(),
                        has_syntactic_modifier(direct, ModifierFlags::EXPORT),
                        false, /*alreadyAddedDirect*/
                    );
                }
                SyntaxKind::ImportDeclaration
                | SyntaxKind::JsImportDeclaration
                | SyntaxKind::JsDocImportTag => {
                    self.direct_imports.push(direct);
                    let import_clause = direct.import_clause();
                    if import_clause.is_some() {
                        let named_bindings = import_clause.named_bindings();
                        if named_bindings.is_some() && is_namespace_import(named_bindings) {
                            self.handle_namespace_import(
                                direct,
                                named_bindings.name(),
                                false, /*isReExport*/
                                true,  /*alreadyAddedDirect*/
                            );
                            // PORT: Go `break` leaves the switch; nothing follows it in the loop.
                            continue;
                        }
                    }
                    if !self.is_available_through_global && is_default_import(direct) {
                        self.add_indirect_user(
                            get_source_file_like_for_import_declaration(direct),
                            false,
                        );
                        // Add a check for indirect uses to handle synthetic default imports
                    }
                }
                SyntaxKind::ExportDeclaration => {
                    let export_clause = direct.export_clause();
                    if export_clause.is_nil() {
                        // This is `export * from "foo"`, so imports of this module may import the export too.
                        let containing_module_symbol =
                            get_containing_module_symbol(direct, self.checker);
                        self.handle_direct_imports(containing_module_symbol);
                    } else if is_namespace_export(export_clause) {
                        // `export * as foo from "foo"` add to indirect uses
                        self.add_indirect_user(
                            get_source_file_like_for_import_declaration(direct),
                            true, /*addTransitiveDependencies*/
                        );
                    } else {
                        // This is `export { foo } from "foo"` and creates an alias symbol, so recursive search will get handle re-exports.
                        self.direct_imports.push(direct);
                    }
                }
                SyntaxKind::ImportType => {
                    // Only check for typeof import('xyz')
                    if !self.is_available_through_global
                        && direct.is_type_of()
                        && direct.qualifier().is_nil()
                        && importers_is_exported(direct, false)
                    {
                        self.add_indirect_user(
                            get_source_file_of_node(direct),
                            true, /*addTransitiveDependencies*/
                        );
                    }
                    self.direct_imports.push(direct);
                }
                _ => crate::gostd::debug::fail_bad_syntax_kind(
                    direct.kind(),
                    Some("Unexpected import kind."),
                ),
            }
        }
    }

    // Go: ls/importTracker.go:306 getIndirectUsers (closure)
    fn get_indirect_users(&mut self) -> Vec<Node> {
        if self.is_available_through_global {
            // It has `export as namespace`, so anything could potentially use it.
            return self.source_files.to_vec();
        }
        // Module augmentations may use this module's exports without importing it.
        let declarations = self
            .checker
            .sym(self.export_info.exporting_module_symbol)
            .declarations
            .clone();
        for decl in declarations {
            if is_external_module_augmentation(decl)
                && self
                    .source_files_set
                    .contains(source_file_file_name(get_source_file_of_node(decl)))
            {
                self.add_indirect_user(decl, false);
            }
        }
        // This may return duplicates (if there are multiple module declarations in a single source file, all importing the same thing as a namespace), but `State.markSearchedSymbol` will handle that.
        self.indirect_user_declarations
            .iter()
            .map(|&decl| get_source_file_of_node(decl))
            .collect()
    }
}

// Go: ls/importTracker.go:214 isExported (closure in getImportersForExport)
// PORT: the closure reads no shared state, so it is a free function. The
// name carries the enclosing function to keep it apart from other
// `is_exported` names.
fn importers_is_exported(mut node: Node, stop_at_ambient_module: bool) -> bool {
    while node.is_some() && !(stop_at_ambient_module && is_ambient_module_declaration(node)) {
        if has_syntactic_modifier(node, ModifierFlags::EXPORT) {
            return true;
        }
        node = node.parent();
    }
    false
}

// Go: ls/importTracker.go:325 getContainingModuleSymbol
pub fn get_containing_module_symbol(importer: Node, checker: &mut Checker) -> SymbolId {
    checker
        .get_merged_symbol_exported(get_source_file_like_for_import_declaration(importer).symbol())
}

// Go: ls/importTracker.go:330 findNamespaceReExports
// Returns 'true' is the namespace 'name' is re-exported from this module, and 'false' if it is only used locally
pub fn find_namespace_re_exports(
    source_file_like: Node,
    name: Node,
    checker: &mut Checker,
) -> bool {
    let namespace_import_symbol = checker.get_symbol_at_location_exported(name);
    for_each_possible_import_or_export_statement(source_file_like, &mut |statement| {
        if !is_export_declaration(statement) {
            return false;
        }
        let export_clause = statement.export_clause();
        let module_specifier = statement.module_specifier();
        module_specifier.is_nil()
            && export_clause.is_some()
            && is_named_exports(export_clause)
            && export_clause.elements().iter().any(|element| {
                checker.get_export_specifier_local_target_symbol(element) == namespace_import_symbol
            })
    })
}

// Go: ls/importTracker.go:344 getSearchesFromDirectImports
// PORT: the Go closures share `importSearches` and `singleReferences`. They
// are the methods of `SearchesFromDirectImports`, in Go order.
pub fn get_searches_from_direct_imports(
    direct_imports: &[Node],
    export_symbol: SymbolId,
    export_kind: ExportKind,
    checker: &mut Checker,
    is_for_rename: bool,
) -> (Vec<LocationAndSymbol>, Vec<Node>) {
    let mut state = SearchesFromDirectImports {
        export_symbol,
        export_kind,
        checker,
        is_for_rename,
        import_searches: Vec::new(),
        single_references: Vec::new(),
    };
    for &decl in direct_imports {
        state.handle_import(decl);
    }
    (state.import_searches, state.single_references)
}

/// The shared locals of Go `getSearchesFromDirectImports`.
struct SearchesFromDirectImports<'a> {
    export_symbol: SymbolId,
    export_kind: ExportKind,
    checker: &'a mut Checker,
    is_for_rename: bool,
    import_searches: Vec<LocationAndSymbol>,
    single_references: Vec<Node>,
}

impl SearchesFromDirectImports<'_> {
    // Go: ls/importTracker.go:354 addSearch (closure)
    fn add_search(&mut self, location: Node, symbol: SymbolId) {
        self.import_searches.push(LocationAndSymbol {
            import_location: location,
            import_symbol: symbol,
        });
    }

    // Go: ls/importTracker.go:358 isNameMatch (closure)
    fn is_name_match(&self, name: &str) -> bool {
        // Use name of "default" even in `export =` case because we may have allowSyntheticDefaultImports
        name == self.checker.sym(self.export_symbol).name.as_str()
            || self.export_kind != ExportKind::NAMED && name == INTERNAL_SYMBOL_NAME_DEFAULT
    }

    // Go: ls/importTracker.go:366 handleNamespaceImportLike (closure)
    // `import x = require("./x")` or `import * as x from "./x"`.
    // An `export =` may be imported by this syntax, so it may be a direct import.
    // If it's not a direct import, it will be in `indirectUsers`, so we don't have to do anything here.
    fn handle_namespace_import_like(&mut self, import_name: Node) {
        // Don't rename an import that already has a different name than the export.
        if self.export_kind == ExportKind::EXPORT_EQUALS
            && (!self.is_for_rename || self.is_name_match(import_name.text()))
        {
            let symbol = self.checker.get_symbol_at_location_exported(import_name);
            self.add_search(import_name, symbol);
        }
    }

    // Go: ls/importTracker.go:373 searchForNamedImport (closure)
    fn search_for_named_import(&mut self, named_bindings: Node) {
        if named_bindings.is_nil() {
            return;
        }
        for element in named_bindings.elements() {
            let name = element.name();
            let property_name = element.property_name();
            let property_name_or_name = if property_name.is_some() {
                property_name
            } else {
                name
            };
            if !self.is_name_match(property_name_or_name.text()) {
                continue;
            }
            if property_name.is_some() {
                // This is `import { foo as bar } from "./a"` or `export { foo as bar } from "./a"`. `foo` isn't a local in the file, so just add it as a single reference.
                self.single_references.push(property_name);
                // If renaming `{ foo as bar }`, don't touch `bar`, just `foo`.
                // But do rename `foo` in ` { default as foo }` if that's the original export name.
                if !self.is_for_rename
                    || name.text() == self.checker.sym(self.export_symbol).name.as_str()
                {
                    // Search locally for `bar`.
                    let symbol = self.checker.get_symbol_at_location_exported(name);
                    self.add_search(name, symbol);
                }
            } else {
                let local_symbol =
                    if is_export_specifier(element) && element.property_name().is_some() {
                        self.checker
                            .get_export_specifier_local_target_symbol(element)
                    } else {
                        self.checker.get_symbol_at_location_exported(name)
                    };
                self.add_search(name, local_symbol);
            }
        }
    }

    // Go: ls/importTracker.go:404 handleImport (closure)
    fn handle_import(&mut self, decl: Node) {
        if is_import_equals_declaration(decl) {
            if is_external_module_import_equals(decl) {
                self.handle_namespace_import_like(decl.name());
            }
            return;
        }
        if is_identifier(decl) {
            self.handle_namespace_import_like(decl);
            return;
        }
        if is_import_type_node(decl) {
            let qualifier = decl.qualifier();
            if qualifier.is_some() {
                let first_identifier = get_first_identifier(qualifier);
                if first_identifier.text() == symbol_name(&self.checker.symbols, self.export_symbol)
                {
                    self.single_references.push(first_identifier);
                }
            } else if self.export_kind == ExportKind::EXPORT_EQUALS {
                self.single_references.push(decl.argument().literal());
            }
            return;
        }
        // Ignore if there's a grammar error
        if !is_string_literal(decl.module_specifier()) {
            return;
        }
        if is_export_declaration(decl) {
            let export_clause = decl.export_clause();
            if export_clause.is_some() && is_named_exports(export_clause) {
                self.search_for_named_import(export_clause);
            }
            return;
        }
        let import_clause = decl.import_clause();
        if import_clause.is_some() {
            let named_bindings = import_clause.named_bindings();
            if named_bindings.is_some() {
                match named_bindings.kind() {
                    SyntaxKind::NamespaceImport => {
                        self.handle_namespace_import_like(named_bindings.name());
                    }
                    SyntaxKind::NamedImports => {
                        // 'default' might be accessed as a named import `{ default as foo }`.
                        if self.export_kind == ExportKind::NAMED
                            || self.export_kind == ExportKind::DEFAULT
                        {
                            self.search_for_named_import(named_bindings);
                        }
                    }
                    _ => {}
                }
            }
            // `export =` might be imported by a default import if `--allowSyntheticDefaultImports` is on, so this handles both ExportKind.Default and ExportKind.ExportEquals.
            // If a default import has the same name as the default export, allow to rename it.
            // Given `import f` and `export default function f`, we will rename both, but for `import g` we will rename just that.
            let name = import_clause.name();
            if name.is_some()
                && (self.export_kind == ExportKind::DEFAULT
                    || self.export_kind == ExportKind::EXPORT_EQUALS)
                && (!self.is_for_rename
                    || name.text()
                        == symbol_name_no_default(&self.checker.symbols, self.export_symbol))
            {
                let default_import_alias = self.checker.get_symbol_at_location_exported(name);
                self.add_search(name, default_import_alias);
            }
        }
    }
}

// Go: ls/importTracker.go:463 getImportOrExportSymbol
// PORT: the Go closures `exportInfo`, `getExport` (with its nested
// closures) and `getImport` are nested functions that take the captured
// values as parameters.
pub fn get_import_or_export_symbol(
    node: Node,
    symbol: SymbolId,
    checker: &mut Checker,
    coming_from_export: bool,
) -> Option<ImportExportSymbol> {
    // Go: ls/importTracker.go:464 exportInfo (closure)
    fn export_info(
        checker: &mut Checker,
        symbol: SymbolId,
        kind: ExportKind,
    ) -> Option<ImportExportSymbol> {
        if let Some(export_info) = get_export_info(symbol, kind, checker) {
            return Some(ImportExportSymbol {
                kind: ImpExpKind::EXPORT,
                symbol,
                export_info: Some(export_info),
            });
        }
        None
    }

    // Go: ls/importTracker.go:476 getExportAssignmentExport (closure in getExport)
    fn get_export_assignment_export(
        checker: &mut Checker,
        symbol: SymbolId,
        ex: Node,
    ) -> Option<ImportExportSymbol> {
        // Get the symbol for the `export =` node; its parent is the module it's the export of.
        let ex_symbol_parent = checker.sym(ex.symbol()).parent;
        if ex_symbol_parent.is_nil() {
            return None;
        }
        let export_kind = if ex.is_export_equals() {
            ExportKind::EXPORT_EQUALS
        } else {
            ExportKind::DEFAULT
        };
        Some(ImportExportSymbol {
            kind: ImpExpKind::EXPORT,
            symbol,
            export_info: Some(ExportInfo {
                exporting_module_symbol: ex_symbol_parent,
                export_kind,
            }),
        })
    }

    // Go: ls/importTracker.go:493 getExportKindForDeclaration (closure in getExport)
    // Not meant for use with export specifiers or export assignment.
    fn get_export_kind_for_declaration(node: Node) -> ExportKind {
        if has_syntactic_modifier(node, ModifierFlags::DEFAULT) {
            return ExportKind::DEFAULT;
        }
        ExportKind::NAMED
    }

    // Go: ls/importTracker.go:500 getSpecialPropertyExport (closure in getExport)
    fn get_special_property_export(
        checker: &mut Checker,
        symbol: SymbolId,
        node: Node,
        use_lhs_symbol: bool,
    ) -> Option<ImportExportSymbol> {
        let kind = match get_assignment_declaration_kind(node) {
            JSDeclarationKind::EXPORTS_PROPERTY => ExportKind::NAMED,
            JSDeclarationKind::MODULE_EXPORTS => ExportKind::EXPORT_EQUALS,
            _ => return None,
        };
        let mut sym = symbol;
        if use_lhs_symbol {
            sym = node.symbol();
        }
        if sym.is_nil() {
            return None;
        }
        export_info(checker, sym, kind)
    }

    // Go: ls/importTracker.go:475 getExport (closure)
    fn get_export(
        node: Node,
        symbol: SymbolId,
        checker: &mut Checker,
        coming_from_export: bool,
    ) -> Option<ImportExportSymbol> {
        let parent = node.parent();
        let grandparent = parent.parent();
        let export_symbol = checker.sym(symbol).export_symbol;
        if export_symbol.is_some() {
            if is_property_access_expression(parent) {
                // When accessing an export of a JS module, there's no alias. The symbol will still be flagged as an export even though we're at the use.
                // So check that we are at the declaration.
                if is_binary_expression(grandparent)
                    && checker.sym(symbol).declarations.contains(&parent)
                {
                    return get_special_property_export(
                        checker,
                        symbol,
                        grandparent,
                        false, /*useLhsSymbol*/
                    );
                }
                return None;
            }
            return export_info(
                checker,
                export_symbol,
                get_export_kind_for_declaration(parent),
            );
        } else {
            let export_node = get_export_node(parent, node);
            if export_node.is_some()
                && (has_syntactic_modifier(export_node, ModifierFlags::EXPORT)
                    || is_implicitly_exported_js_doc_declaration(export_node))
            {
                if is_import_equals_declaration(export_node)
                    && export_node.module_reference() == node
                {
                    // We're at `Y` in `export import X = Y`. This is not the exported symbol, the left-hand-side is. So treat this as an import statement.
                    if coming_from_export {
                        return None;
                    }
                    let lhs_symbol = checker.get_symbol_at_location_exported(export_node.name());
                    return Some(ImportExportSymbol {
                        kind: ImpExpKind::IMPORT,
                        symbol: lhs_symbol,
                        export_info: None,
                    });
                }
                return export_info(
                    checker,
                    symbol,
                    get_export_kind_for_declaration(export_node),
                );
            } else if is_namespace_export(parent) {
                return export_info(checker, symbol, ExportKind::NAMED);
            } else if is_export_assignment(parent) {
                return get_export_assignment_export(checker, symbol, parent);
            } else if is_export_assignment(grandparent) {
                return get_export_assignment_export(checker, symbol, grandparent);
            } else if is_binary_expression(parent) {
                return get_special_property_export(
                    checker, symbol, parent, true, /*useLhsSymbol*/
                );
            } else if is_binary_expression(grandparent) {
                return get_special_property_export(
                    checker,
                    symbol,
                    grandparent,
                    true, /*useLhsSymbol*/
                );
            } else if is_js_doc_typedef_tag(parent) || is_js_doc_callback_tag(parent) {
                return export_info(checker, symbol, ExportKind::NAMED);
            }
        }
        None
    }

    // Go: ls/importTracker.go:565 getImport (closure)
    fn get_import(
        node: Node,
        symbol: SymbolId,
        checker: &mut Checker,
    ) -> Option<ImportExportSymbol> {
        if !is_node_import(node) {
            return None;
        }
        // JS destructuring from `require(...)` is import-like for references, but the binding element
        // itself is still a local variable symbol rather than an alias.
        let mut imported_symbol = if checker.sym(symbol).flags.intersects(SymbolFlags::ALIAS) {
            checker.get_immediate_aliased_symbol_exported(symbol)
        } else {
            get_property_symbol_of_object_binding_pattern_without_property_name(symbol, checker)
        };
        if imported_symbol.is_nil() {
            return None;
        }
        // Search on the local symbol in the exporting module, not the exported symbol.
        imported_symbol = skip_export_specifier_symbol(imported_symbol, checker);
        if imported_symbol.is_nil() {
            return None;
        }
        // Similarly, skip past the symbol for 'export ='
        if checker.sym(imported_symbol).name.as_str() == "export=" {
            imported_symbol = get_export_equals_local_symbol(imported_symbol, checker);
            if imported_symbol.is_nil() {
                return None;
            }
        }
        // If the import has a different name than the export, do not continue searching.
        // If `importedName` is undefined, do continue searching as the export is anonymous.
        // (All imports returned from this function will be ignored anyway if we are in rename and this is a not a named export.)
        let imported_name = symbol_name_no_default(&checker.symbols, imported_symbol);
        if imported_name.is_empty()
            || imported_name == INTERNAL_SYMBOL_NAME_DEFAULT
            || imported_name == checker.sym(symbol).name.as_str()
        {
            return Some(ImportExportSymbol {
                kind: ImpExpKind::IMPORT,
                symbol: imported_symbol,
                export_info: None,
            });
        }
        None
    }

    let mut result = get_export(node, symbol, checker, coming_from_export);
    if result.is_none() && !coming_from_export {
        result = get_import(node, symbol, checker);
    }
    result
}

// Go: ls/importTracker.go:612 getExportInfo
pub fn get_export_info(
    export_symbol: SymbolId,
    export_kind: ExportKind,
    c: &mut Checker,
) -> Option<ExportInfo> {
    // Parent can be nil if an `export` is not at the top-level (which is a compile error).
    let parent = c.sym(export_symbol).parent;
    if parent.is_some() {
        let exporting_module_symbol = c.get_merged_symbol_exported(parent);
        // `export` may appear in a namespace. In that case, just rely on global search.
        if c.is_external_module_symbol(exporting_module_symbol) {
            return Some(ExportInfo {
                exporting_module_symbol,
                export_kind,
            });
        }
    }
    None
}

// Go: ls/importTracker.go:629 getExportNode
// If a reference is a class expression, the exported node would be its parent.
// If a reference is a variable declaration, the exported node would be the variable statement.
pub fn get_export_node(parent: Node, node: Node) -> Node {
    let mut declaration = Node::NIL;
    if is_variable_declaration(parent) {
        declaration = parent;
    } else if is_binding_element(parent) {
        declaration = walk_up_binding_elements_and_patterns(parent);
    }
    if declaration.is_some() {
        if parent.name() == node
            && !is_catch_clause(declaration.parent())
            && is_variable_statement(declaration.parent().parent())
        {
            return declaration.parent().parent();
        }
        return Node::NIL;
    }
    parent
}

// Go: ls/importTracker.go:646 isNodeImport
pub fn is_node_import(node: Node) -> bool {
    let parent = node.parent();
    match parent.kind() {
        SyntaxKind::ImportEqualsDeclaration => {
            parent.name() == node && is_external_module_import_equals(parent)
        }
        SyntaxKind::ImportSpecifier => {
            // For a rename import `{ foo as bar }`, don't search for the imported symbol. Just find local uses of `bar`.
            parent.property_name().is_nil()
        }
        SyntaxKind::ImportClause | SyntaxKind::NamespaceImport => {
            crate::go_assert!(parent.name() == node);
            true
        }
        SyntaxKind::BindingElement => {
            is_in_js_file(node)
                && is_variable_declaration_initialized_to_bare_or_accessed_require(
                    parent.parent().parent(),
                )
        }
        _ => false,
    }
}

// Go: ls/importTracker.go:663 isExternalModuleImportEquals
pub fn is_external_module_import_equals(node: Node) -> bool {
    let module_reference = node.module_reference();
    is_external_module_reference(module_reference)
        && module_reference.expression().kind() == SyntaxKind::StringLiteral
}

// Go: ls/importTracker.go:669 skipExportSpecifierSymbol
// If at an export specifier, go to the symbol it refers to. */
pub fn skip_export_specifier_symbol(symbol: SymbolId, checker: &mut Checker) -> SymbolId {
    // For `export { foo } from './bar", there's nothing to skip, because it does not create a new alias. But `export { foo } does.
    let declarations = checker.sym(symbol).declarations.clone();
    for declaration in declarations {
        if is_export_specifier(declaration)
            && declaration.property_name().is_nil()
            && declaration.parent().parent().module_specifier().is_nil()
        {
            // core.OrElse
            let local_target = checker.get_export_specifier_local_target_symbol(declaration);
            return if local_target.is_some() {
                local_target
            } else {
                symbol
            };
        } else if is_property_access_expression(declaration)
            && is_module_exports_access_expression(declaration.expression())
            && !is_private_identifier(declaration.name())
        {
            // Export of form 'module.exports.propName = expr';
            return checker.get_symbol_at_location_exported(declaration);
        } else if is_shorthand_property_assignment(declaration)
            && is_binary_expression(declaration.parent().parent())
            && get_assignment_declaration_kind(declaration.parent().parent())
                == JSDeclarationKind::MODULE_EXPORTS
        {
            return checker.get_export_specifier_local_target_symbol(declaration.name());
        }
    }
    symbol
}

// Go: ls/importTracker.go:685 getExportEqualsLocalSymbol
pub fn get_export_equals_local_symbol(
    imported_symbol: SymbolId,
    checker: &mut Checker,
) -> SymbolId {
    if checker
        .sym(imported_symbol)
        .flags
        .intersects(SymbolFlags::ALIAS)
    {
        return checker.get_immediate_aliased_symbol_exported(imported_symbol);
    }
    let decl = checker.sym(imported_symbol).value_declaration;
    crate::go_assert!(decl.is_some());
    if is_export_assignment(decl) {
        return decl.expression().symbol();
    } else if is_binary_expression(decl) {
        return decl.right().symbol();
    } else if is_source_file(decl) {
        return decl.symbol();
    }
    SymbolId::NIL
}

// Go: ls/importTracker.go:702 symbolNameNoDefault
pub fn symbol_name_no_default(symbols: &SymbolArena, symbol: SymbolId) -> String {
    let s = symbols.sym(symbol);
    if s.name.as_str() != INTERNAL_SYMBOL_NAME_DEFAULT {
        return s.name.as_str().to_string();
    }
    for &decl in s.declarations.iter() {
        let name = get_name_of_declaration(decl);
        if name.is_some() && is_identifier(name) {
            return name.text().to_string();
        }
    }
    String::new()
}

// Go: ls/importTracker.go:717 findModuleReferences
// findModuleReferences finds all references to a module symbol across the given source files.
// This includes import statements, <reference> directives, and implicit references (e.g., JSX runtime imports).
// PORT: the `<reference path>` and `<reference types>` checks read the
// program's parsed file; they are `ProgramView::references_to_file`
// (program_view.rs), in the same order.
pub fn find_module_references<P: ProgramView>(
    program: &P,
    source_files: &[Node],
    search_module_symbol: SymbolId,
    checker: &mut Checker,
) -> Vec<ModuleReference> {
    let mut refs: Vec<ModuleReference> = Vec::new();

    for &referencing_file in source_files {
        let search_source_file = checker.sym(search_module_symbol).value_declaration;
        if search_source_file.is_some() && search_source_file.kind() == SyntaxKind::SourceFile {
            // Check <reference path> directives, then <reference types> directives
            for ref_ in program.references_to_file(referencing_file, search_source_file) {
                refs.push(ModuleReference {
                    kind: ModuleReferenceKind::REFERENCE,
                    literal: Node::NIL,
                    referencing_file,
                    ref_: Some(ref_),
                });
            }
        }

        // Check all imports (including require() calls)
        for_each_import(
            program,
            referencing_file,
            &mut |import_decl, module_specifier| {
                let module_symbol = checker.get_symbol_at_location_exported(module_specifier);
                if module_symbol == search_module_symbol {
                    if node_is_synthesized(import_decl) {
                        refs.push(ModuleReference {
                            kind: ModuleReferenceKind::IMPLICIT,
                            literal: module_specifier,
                            referencing_file,
                            ref_: None,
                        });
                    } else {
                        // PORT: Go leaves `referencingFile` nil for this kind.
                        refs.push(ModuleReference {
                            kind: ModuleReferenceKind::IMPORT,
                            literal: module_specifier,
                            referencing_file: Node::NIL,
                            ref_: None,
                        });
                    }
                }
            },
        );
    }

    refs
}
