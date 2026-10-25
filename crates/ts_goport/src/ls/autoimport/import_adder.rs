use crate::ls::autoimport::prelude::*;

// Port of Go `ls/autoimport/import_adder.go`.
//
// PORT (whole file):
// - Pinned decision (map-ls-completions 2.5): Go `importAdder` stores
//   `checker *checker.Checker`. The Rust adder does not store the checker;
//   the methods that use it take `&mut Checker`, and `NewImportAdder` drops
//   the `checker` parameter. Functions that reach those methods
//   (`TypeNodeToAutoImportableTypeNode`, `importSymbols`) also take it.
// - The Go interface is the trait `ImportAdder`; the Go struct
//   `importAdder` is `ImportAdderImpl` (Rust type names cannot differ only
//   in case).
// - Go `map[*ast.ImportClauseOrBindingPattern]*addToExistingState` and
//   `map[string]*importsCollection` are `IndexMap`s in insertion order.
//   PORT: Go map order is random. The change tracker sorts edits by
//   position and `insertImports` sorts new declarations, so only ties can
//   come out in another order.
// - Go `*importsCollection` values are shared between the map and the
//   caller of `getNewImportEntry`: `Rc<RefCell<ImportsCollection>>`.
// - Go `symbol.Name` / `symbol.Parent` reads take the symbol arena, as the
//   ast helpers do (`symbols: &SymbolArena`).

use crate::frontend::compiler;
use crate::gostd::Context;
use crate::locale;
use crate::ls::{change, lsconv, lsutil};
use crate::lsp::lsproto;

// Go: ls/autoimport/import_adder.go:24 ImportAdder
pub trait ImportAdder {
    fn has_fixes(&self) -> bool;
    fn add_import_from_exported_symbol(
        &mut self,
        type_checker: &mut Checker,
        symbol: SymbolId,
        is_valid_type_only_use_site: bool,
    );
    fn add_import_fix(&mut self, fix: Rc<Fix>);
    fn edits(&mut self) -> Vec<lsproto::TextEdit>;
}

// Go: ls/autoimport/import_adder.go:32 addToExistingState
// addToExistingState tracks modifications to an existing import clause or binding pattern
// PORT: Go always makes `namedImports`, so it is a plain map; Go reads it
// only through `sortedNamedImports`, which sorts the keys.
#[derive(Clone, Debug, Default)]
pub struct AddToExistingState {
    pub import_clause_or_binding_pattern: Node,
    pub default_import: Option<NewImportBinding>,
    pub named_imports: FxHashMap<String, NewImportBinding>,
}

// Go: ls/autoimport/import_adder.go:39 importsCollection
// importsCollection tracks new imports to be created for a given module specifier
// PORT: Go `namedImports` starts nil and is made on first use: `None`.
#[derive(Clone, Debug, Default)]
pub struct ImportsCollection {
    pub default_import: Option<NewImportBinding>,
    pub named_imports: Option<FxHashMap<String, NewImportBinding>>,
    pub namespace_like_import: Option<NewImportBinding>,
    pub use_require: bool,
}

// Go: ls/autoimport/import_adder.go:46 newImportsKey
fn new_imports_key(module_specifier: &str, top_level_type_only: bool) -> String {
    if top_level_type_only {
        return format!("1|{module_specifier}");
    }
    format!("0|{module_specifier}")
}

// Go: ls/autoimport/import_adder.go:53 importAdder
// PORT: see the file header (no `checker` field; Go `*View` is `Rc<View>`,
// Go `*lsconv.Converters` is `Rc<lsconv::Converters>`).
pub struct ImportAdderImpl {
    // Context
    pub ctx: Context,
    pub view: Rc<View>,
    pub format_options: lsutil::FormatCodeSettings,
    pub converters: Rc<lsconv::Converters>,
    pub preferences: lsutil::UserPreferences,

    // State
    pub add_to_namespace: Vec<Rc<Fix>>, // Namespace fixes don't conflict, so just build a list
    pub import_type: Vec<Rc<Fix>>,      // JSDoc type import fixes
    pub add_to_existing: IndexMap<Node, AddToExistingState>, // importClauseOrBindingPattern -> default or named bindings
    pub new_imports: IndexMap<String, Rc<RefCell<ImportsCollection>>>, // module specifier + type only -> imports
                                                                       // !!! removeExisting, verbatimImports?
}

// Go: ls/autoimport/import_adder.go:70 NewImportAdder
// PORT: the Go `checker` parameter is dropped (pinned decision, see the
// file header). Go does not read `program` or `file` either; they stay so
// callers pass the Go arguments.
pub fn new_import_adder(
    ctx: &Context,
    _program: &compiler::NewProgram,
    _file: Node,
    view: Rc<View>,
    format_options: lsutil::FormatCodeSettings,
    converters: Rc<lsconv::Converters>,
    preferences: lsutil::UserPreferences,
) -> Box<dyn ImportAdder> {
    Box::new(ImportAdderImpl {
        ctx: ctx.clone(),
        view,
        format_options,
        converters,
        preferences,
        add_to_namespace: Vec::new(),
        import_type: Vec::new(),
        add_to_existing: IndexMap::new(),
        new_imports: IndexMap::new(),
    })
}

impl ImportAdder for ImportAdderImpl {
    // Go: ls/autoimport/import_adder.go:94 HasFixes
    fn has_fixes(&self) -> bool {
        !self.add_to_namespace.is_empty()
            || !self.import_type.is_empty()
            || !self.add_to_existing.is_empty()
            || !self.new_imports.is_empty()
    }

    // Go: ls/autoimport/import_adder.go:102 AddImportFromExportedSymbol
    // !!! referenceImport
    fn add_import_from_exported_symbol(
        &mut self,
        type_checker: &mut Checker,
        exported_symbol: SymbolId,
        is_valid_type_only_use_site: bool,
    ) {
        let skipped = type_checker.skip_alias_exported(exported_symbol);
        let symbol = type_checker.get_merged_symbol_exported(skipped);
        let export_infos = self.get_all_exports_for_symbol(type_checker, symbol);
        if export_infos.is_empty() {
            // If no exportInfo is found, this means export could not be resolved when we have filtered for autoImportFileExcludePatterns,
            //     so we should not generate an import.
            // debug.Assert(len(adder.ls.UserPreferences().AutoImportFileExcludePatterns) > 0)
            return;
        }
        let view = self.view.clone();
        let fix = self.get_import_fix_for_symbol(
            type_checker,
            &view,
            &export_infos,
            is_valid_type_only_use_site,
        );
        if let Some(fix) = fix {
            // !!! referenceImport -> propertyName
            self.add_import_fix(fix);
        }
    }

    // Go: ls/autoimport/import_adder.go:118 Edits
    // PORT: Go ranges over the `addToExisting` and `newImports` maps (random
    // order); see the file header.
    fn edits(&mut self) -> Vec<lsproto::TextEdit> {
        // !!! organize imports?
        let view = Rc::clone(&self.view);
        let program = &*view.program;
        let importing_file = self.view.importing_file;
        let mut tracker = change::new_tracker(
            &self.ctx,
            program.options(),
            self.format_options.clone(),
            self.converters.clone(),
        );
        let quote_preference = lsutil::get_quote_preference(importing_file, &self.preferences);
        for fix in &self.add_to_namespace {
            add_namespace_qualifier(fix, &mut tracker, importing_file, &locale::DEFAULT);
        }
        for fix in &self.import_type {
            add_import_type(
                fix,
                importing_file,
                &self.preferences,
                &mut tracker,
                &locale::DEFAULT,
            );
        }
        for (&clause_or_pattern, entry) in &self.add_to_existing {
            add_to_existing_import(
                &mut tracker,
                importing_file,
                clause_or_pattern,
                entry.default_import.as_ref(),
                &sorted_named_imports(Some(&entry.named_imports)),
                &self.preferences,
            );
        }

        let mut new_declarations: Vec<Node> = Vec::new();
        for (key, new_import) in &self.new_imports {
            let module_specifier = &key[2..]; // From `${0 | 1}|${moduleSpecifier}` format
            let new_import = new_import.borrow();
            let declarations = if new_import.use_require {
                get_new_requires(
                    &mut tracker,
                    module_specifier,
                    quote_preference,
                    new_import.default_import.as_ref(),
                    &sorted_named_imports(new_import.named_imports.as_ref()),
                    new_import.namespace_like_import.as_ref(),
                    program.options(),
                )
            } else {
                get_new_imports(
                    &mut tracker,
                    module_specifier,
                    quote_preference,
                    new_import.default_import.as_ref(),
                    &sorted_named_imports(new_import.named_imports.as_ref()),
                    new_import.namespace_like_import.as_ref(),
                    program.options(),
                    &self.preferences,
                )
            };
            new_declarations.extend(declarations);
        }

        if !new_declarations.is_empty() {
            insert_imports(
                &mut tracker,
                importing_file,
                &new_declarations,
                true, /*blankLineBetween*/
                &self.preferences,
            );
        }

        // Unmappable files are dropped by GetChanges, so a content-mapped importing file that cannot be
        // faithfully rewritten yields no edits rather than a corrupting one.
        // PORT: Go `changes[fileName]` is a nil slice for a missing key. Go
        // `OriginalFileName()` is the `lsconv::Script` method of a file root `Node`.
        let (mut changes, _) = tracker.get_changes();
        changes
            .shift_remove(lsconv::Script::original_file_name(&importing_file))
            .unwrap_or_default()
    }

    // Go: ls/autoimport/import_adder.go:189 AddImportFix
    // AddImportFix adds a fix to the import adder, accumulating it with other fixes
    // so that multiple imports from the same module are coalesced into a single import statement.
    fn add_import_fix(&mut self, fix: Rc<Fix>) {
        let symbol_name = fix.name.clone();
        let view = Rc::clone(&self.view);
        let program = &*view.program;
        let compiler_options = program.options();

        match fix.kind {
            lsproto::AutoImportFixKind::USE_NAMESPACE => {
                self.add_to_namespace.push(fix);
            }
            lsproto::AutoImportFixKind::JSDOC_TYPE_IMPORT => {
                self.import_type.push(fix);
            }
            lsproto::AutoImportFixKind::ADD_TO_EXISTING => {
                let existing_fix = get_add_to_existing_import_fix(self.view.importing_file, &fix);
                let entry = self
                    .add_to_existing
                    .entry(existing_fix.import_clause_or_binding_pattern)
                    .or_insert_with(|| AddToExistingState {
                        import_clause_or_binding_pattern: existing_fix
                            .import_clause_or_binding_pattern,
                        default_import: None,
                        named_imports: FxHashMap::default(),
                    });

                if fix.import_kind == lsproto::ImportKind::NAMED {
                    let prev_import = entry.named_imports.get(&symbol_name);
                    let mut prev_type_only = lsproto::AddAsTypeOnly::default();
                    if let Some(prev_import) = prev_import {
                        prev_type_only = prev_import.add_as_type_only;
                    }
                    let binding = NewImportBinding {
                        kind: lsproto::ImportKind::NAMED,
                        name: symbol_name.clone(),
                        add_as_type_only: reduce_add_as_type_only_values(
                            prev_type_only,
                            fix.add_as_type_only,
                        ),
                        property_name: existing_fix
                            .named_import
                            .as_ref()
                            .unwrap_or_else(|| crate::core::go_nil_dereference())
                            .property_name
                            .clone(),
                    };
                    entry.named_imports.insert(symbol_name, binding);
                } else {
                    // Default import
                    crate::go_assert!(
                        entry
                            .default_import
                            .as_ref()
                            .is_none_or(|d| d.name == symbol_name),
                        "(Add to Existing) Default import should be missing or match symbolName"
                    );
                    let mut prev_type_only = lsproto::AddAsTypeOnly::default();
                    if let Some(default_import) = &entry.default_import {
                        prev_type_only = default_import.add_as_type_only;
                    }
                    entry.default_import = Some(NewImportBinding {
                        kind: lsproto::ImportKind::DEFAULT,
                        name: symbol_name,
                        add_as_type_only: reduce_add_as_type_only_values(
                            prev_type_only,
                            fix.add_as_type_only,
                        ),
                        ..Default::default()
                    });
                }
            }

            lsproto::AutoImportFixKind::ADD_NEW => {
                let entry = self.get_new_import_entry(
                    &fix.module_specifier,
                    fix.import_kind,
                    fix.use_require,
                    fix.add_as_type_only,
                );
                let mut entry = entry.borrow_mut();
                crate::go_assert!(
                    entry.use_require == fix.use_require,
                    "(Add new) Tried to add an `import` and a `require` for the same module"
                );

                match fix.import_kind {
                    lsproto::ImportKind::DEFAULT => {
                        crate::go_assert!(
                            entry
                                .default_import
                                .as_ref()
                                .is_none_or(|d| d.name == symbol_name),
                            "(Add new) Default import should be missing or match symbolName"
                        );
                        let mut prev_type_only = lsproto::AddAsTypeOnly::default();
                        if let Some(default_import) = &entry.default_import {
                            prev_type_only = default_import.add_as_type_only;
                        }
                        entry.default_import = Some(NewImportBinding {
                            kind: lsproto::ImportKind::DEFAULT,
                            name: symbol_name,
                            add_as_type_only: reduce_add_as_type_only_values(
                                prev_type_only,
                                fix.add_as_type_only,
                            ),
                            ..Default::default()
                        });
                    }

                    lsproto::ImportKind::NAMED => {
                        if entry.named_imports.is_none() {
                            entry.named_imports = Some(FxHashMap::default());
                        }
                        let named_imports =
                            entry.named_imports.as_mut().expect("set above when nil");
                        let prev_import = named_imports.get(&symbol_name);
                        let mut prev_type_only = lsproto::AddAsTypeOnly::default();
                        if let Some(prev_import) = prev_import {
                            prev_type_only = prev_import.add_as_type_only;
                        }
                        named_imports.insert(
                            symbol_name.clone(),
                            NewImportBinding {
                                kind: lsproto::ImportKind::NAMED,
                                name: symbol_name,
                                add_as_type_only: reduce_add_as_type_only_values(
                                    prev_type_only,
                                    fix.add_as_type_only,
                                ),
                                // !!! propertyName
                                ..Default::default()
                            },
                        );
                    }

                    lsproto::ImportKind::COMMON_JS => {
                        if compiler_options.verbatim_module_syntax == Tristate::True {
                            if entry.named_imports.is_none() {
                                entry.named_imports = Some(FxHashMap::default());
                            }
                            let named_imports =
                                entry.named_imports.as_mut().expect("set above when nil");
                            let prev_import = named_imports.get(&symbol_name);
                            let mut prev_type_only = lsproto::AddAsTypeOnly::default();
                            if let Some(prev_import) = prev_import {
                                prev_type_only = prev_import.add_as_type_only;
                            }
                            named_imports.insert(
                                symbol_name.clone(),
                                NewImportBinding {
                                    kind: lsproto::ImportKind::COMMON_JS,
                                    name: symbol_name,
                                    add_as_type_only: reduce_add_as_type_only_values(
                                        prev_type_only,
                                        fix.add_as_type_only,
                                    ),
                                    // !!! propertyName
                                    ..Default::default()
                                },
                            );
                        } else {
                            crate::go_assert!(
                                entry
                                    .namespace_like_import
                                    .as_ref()
                                    .is_none_or(|n| n.name == symbol_name),
                                "Namespacelike import should be missing or match symbolName"
                            );
                            entry.namespace_like_import = Some(NewImportBinding {
                                kind: lsproto::ImportKind::COMMON_JS,
                                name: symbol_name,
                                add_as_type_only: fix.add_as_type_only,
                                ..Default::default()
                            });
                        }
                    }

                    lsproto::ImportKind::NAMESPACE => {
                        crate::go_assert!(
                            entry
                                .namespace_like_import
                                .as_ref()
                                .is_none_or(|n| n.name == symbol_name),
                            "Namespacelike import should be missing or match symbolName"
                        );
                        entry.namespace_like_import = Some(NewImportBinding {
                            kind: lsproto::ImportKind::NAMESPACE,
                            name: symbol_name,
                            add_as_type_only: fix.add_as_type_only,
                            ..Default::default()
                        });
                    }

                    _ => {}
                }
            }

            lsproto::AutoImportFixKind::PROMOTE_TYPE_ONLY => {
                // Excluding from fix-all
            }
            _ => crate::gostd::debug::fail(&format!("Unexpected fix kind: {}", fix.kind.string())),
        }
    }
}

// Go: ls/autoimport/import_adder.go:178 sortedNamedImports
// PORT: a Go nil map is `None`. Go `slices.Sorted` on string keys is byte
// order, as `String` ordering.
fn sorted_named_imports(m: Option<&FxHashMap<String, NewImportBinding>>) -> Vec<NewImportBinding> {
    let Some(m) = m else {
        return Vec::new();
    };
    let mut keys: Vec<&String> = m.keys().collect();
    keys.sort();
    let mut result: Vec<NewImportBinding> = Vec::with_capacity(keys.len());
    for k in keys {
        result.push(m[k].clone());
    }
    result
}

// Go: ls/autoimport/import_adder.go:330 reduceAddAsTypeOnlyValues
// `NotAllowed` overrides `Required` because one addition of a new import might be required to be type-only
// because of `--importsNotUsedAsValues=error`, but if a second addition of the same import is `NotAllowed`
// to be type-only, the reason the first one was `Required` - the unused runtime dependency - is now moot.
// Alternatively, if one addition is `Required` because it has no value meaning under `--preserveValueImports`
// and `--isolatedModules`, it should be impossible for another addition to be `NotAllowed` since that would
// mean a type is being referenced in a value location.
fn reduce_add_as_type_only_values(
    prev_value: lsproto::AddAsTypeOnly,
    new_value: lsproto::AddAsTypeOnly,
) -> lsproto::AddAsTypeOnly {
    if new_value > prev_value {
        return new_value;
    }
    prev_value
}

impl ImportAdderImpl {
    // Go: ls/autoimport/import_adder.go:337 getNewImportEntry
    pub fn get_new_import_entry(
        &mut self,
        module_specifier: &str,
        import_kind: lsproto::ImportKind,
        use_require: bool,
        add_as_type_only: lsproto::AddAsTypeOnly,
    ) -> Rc<RefCell<ImportsCollection>> {
        // A default import that requires type-only makes the whole import type-only.
        // (We could add `default` as a named import, but that style seems undesirable.)
        // Under `--preserveValueImports` and `--importsNotUsedAsValues=error`, if a
        // module default-exports a type but named-exports some values (weird), you would
        // have to use a type-only default import and non-type-only named imports. These
        // require two separate import declarations, so we build this into the map key.
        let type_only_key = new_imports_key(module_specifier, true /*topLevelTypeOnly*/);
        let non_type_only_key = new_imports_key(module_specifier, false /*topLevelTypeOnly*/);
        let type_only_entry = self.new_imports.get(&type_only_key).cloned();
        let non_type_only_entry = self.new_imports.get(&non_type_only_key).cloned();
        let new_entry = Rc::new(RefCell::new(ImportsCollection {
            use_require,
            ..Default::default()
        }));

        if import_kind == lsproto::ImportKind::DEFAULT
            && add_as_type_only == lsproto::AddAsTypeOnly::REQUIRED
        {
            if let Some(type_only_entry) = type_only_entry {
                return type_only_entry;
            }
            self.new_imports.insert(type_only_key, new_entry.clone());
            return new_entry;
        }

        if add_as_type_only == lsproto::AddAsTypeOnly::ALLOWED
            && (type_only_entry.is_some() || non_type_only_entry.is_some())
        {
            if let Some(type_only_entry) = type_only_entry {
                return type_only_entry;
            }
            return non_type_only_entry.expect("checked above");
        }

        if let Some(non_type_only_entry) = non_type_only_entry {
            return non_type_only_entry;
        }

        self.new_imports
            .insert(non_type_only_key, new_entry.clone());
        new_entry
    }

    // Go: ls/autoimport/import_adder.go:375 getAllExportsForSymbol
    // PORT: `ch` is the Go `adder.checker` (pinned decision).
    pub fn get_all_exports_for_symbol(
        &self,
        ch: &mut Checker,
        symbol: SymbolId,
    ) -> Vec<Rc<Export>> {
        if let Some(export) = symbol_to_export(symbol, ch) {
            return self.view.search_by_export_id(&export.export_id);
        }
        Vec::new()
    }
}

// Go: ls/autoimport/import_adder.go:382 TypeToAutoImportableTypeNode
// PORT: Go shares `idToSymbol` with the node builder through
// `c.TypeToTypeNode`. The port builds the node builder as
// `Checker::type_to_type_node_exported` does and reads the filled map back
// from `nb.impl_.borrow().id_to_symbol` (PORTING "Programs and checkers").
pub fn type_to_auto_importable_type_node(
    c: &mut Checker,
    import_adder: Option<&mut dyn ImportAdder>,
    t: TypeId,
    context_node: Node, // !!! flags
) -> Node {
    let id_to_symbol: FxHashMap<Node, SymbolId> = FxHashMap::default();
    let node_builder = c.get_node_builder_ex(Some(id_to_symbol));
    let type_node = c.node_builder_type_to_type_node(
        &node_builder,
        t,
        context_node,
        NodeBuilderFlags::NONE,
        InternalNodeBuilderFlags::NONE,
        None,
    );
    let id_to_symbol = node_builder.borrow().impl_.borrow().id_to_symbol.clone();
    if type_node.is_nil() {
        return Node::NIL;
    }
    type_node_to_auto_importable_type_node(c, type_node, import_adder, &id_to_symbol)
}

// Go: ls/autoimport/import_adder.go:398 TypeNodeToAutoImportableTypeNode
// TypeNodeToAutoImportableTypeNode converts import type references in a type node to
// simple type references and registers needed imports with the import adder.
// PORT: `c` is added first (the adder's `AddImportFromExportedSymbol` takes
// the checker, pinned decision); Go `importAdder` may be nil (`None`).
pub fn type_node_to_auto_importable_type_node(
    c: &mut Checker,
    type_node: Node,
    import_adder: Option<&mut dyn ImportAdder>,
    id_to_symbol: &FxHashMap<Node, SymbolId>,
) -> Node {
    let mut type_node = type_node;
    let (reference_type_node, importable_symbols) =
        try_get_auto_importable_reference_from_type_node(&c.symbols, type_node, id_to_symbol);
    if reference_type_node.is_some() {
        if let Some(import_adder) = import_adder {
            import_symbols(c, import_adder, &importable_symbols);
        }
        type_node = reference_type_node;
    }

    // !!! handle type node reuse: nodes needs to be fresh here but also preserve symbols
    type_node
}

// Go: ls/autoimport/import_adder.go:415 importSymbols
fn import_symbols(c: &mut Checker, import_adder: &mut dyn ImportAdder, symbols: &[SymbolId]) {
    for &symbol in symbols {
        import_adder
            .add_import_from_exported_symbol(c, symbol, true /*isValidTypeOnlyUseSite*/);
    }
}

// Go: ls/autoimport/import_adder.go:427 TryGetAutoImportableReferenceFromTypeNode
// Given a type node containing 'import("./a").SomeType<import("./b").OtherType<...>>',
// returns an equivalent type reference node with any nested ImportTypeNodes also replaced
// with type references, and a list of symbols that must be imported to use the type reference.
// TryGetAutoImportableReferenceFromTypeNode converts import type references in a type node
// to simple type references and returns the transformed type node and the symbols that need
// to be imported.
// PORT: `symbols` is the arena that owns the `idToSymbol` symbols (a
// checker's `symbols`), read by `getNameForExportedSymbol`. The Go closure
// appends to the local `symbols` slice; here that list is the visitor's
// `ctx`. Go `map[*ast.IdentifierNode]*ast.Symbol` is `FxHashMap<Node, SymbolId>`.
pub fn try_get_auto_importable_reference_from_type_node(
    symbols: &SymbolArena,
    import_type_node: Node,
    id_to_symbol: &FxHashMap<Node, SymbolId>,
) -> (Node, Vec<SymbolId>) {
    let factory = NodeFactory::new_with_hooks(NodeFactoryHooks::default());
    let mut visitor = new_node_visitor(
        |node: Node, visitor: &mut NodeVisitor<'_, Vec<SymbolId>>| -> Node {
            if is_literal_import_type_node(node) && node.qualifier().is_some() {
                let import_type_node = node;
                // Symbol for the left-most thing after the dot
                let first_identifier = get_first_identifier(import_type_node.qualifier());
                let symbol = id_to_symbol
                    .get(&first_identifier)
                    .copied()
                    .unwrap_or(SymbolId::NIL);
                if symbol.is_nil() {
                    // if symbol is missing then this doesn't come from a synthesized import type node
                    // it has to be an import type node authored by the user and thus it has to be valid
                    // it can't refer to reserved internal symbol names and such
                    return node.visit_each_child(visitor);
                }
                let name =
                    get_name_for_exported_symbol(symbols, symbol, false /*preferCapitalized*/);
                let qualifier = if name != first_identifier.text() {
                    let new_identifier = factory.new_identifier(name);
                    replace_first_identifier_of_entity_name(
                        &factory,
                        import_type_node.qualifier(),
                        new_identifier,
                    )
                } else {
                    import_type_node.qualifier()
                };
                visitor.ctx.push(symbol);
                let type_arguments = visitor.visit_nodes(import_type_node.type_argument_list());
                return factory.new_type_reference_node(qualifier, type_arguments);
            }
            visitor.visit_each_child(node)
        },
        Some(&factory),
        NodeVisitorHooks::default(),
        Vec::new(),
    );

    let type_node = visitor.visit_node(import_type_node);
    crate::go_assert!(
        type_node.is_nil() || is_type_node(type_node),
        "expected a type node"
    );
    let symbols_to_import = std::mem::take(&mut visitor.ctx);
    (type_node, symbols_to_import)
}

// Go: ls/autoimport/import_adder.go:465 getNameForExportedSymbol
// If a type checker and multiple files are available, consider using `forEachNameOfDefaultExport`
// instead, which searches for names of re-exported defaults/namespaces in target files.
fn get_name_for_exported_symbol(
    symbols: &SymbolArena,
    symbol: SymbolId,
    prefer_capitalized: bool,
) -> String {
    let symbol_name = symbols.sym(symbol).name.as_str();
    if symbol_name == INTERNAL_SYMBOL_NAME_EXPORT_EQUALS
        || symbol_name == INTERNAL_SYMBOL_NAME_DEFAULT
    {
        // Names for default exports:
        // - export default foo => foo
        // - export { foo as default } => foo
        // - export default 0 => filename converted to camelCase
        let name = get_default_like_export_name_from_declaration(symbols, symbol);
        if !name.is_empty() {
            return name;
        }
        crate::go_assert!(
            symbols.sym(symbol).parent.is_some(),
            "Expected exported symbol to have module symbol as parent"
        );
        return lsutil::module_symbol_to_valid_identifier(
            symbols,
            symbols.sym(symbol).parent,
            prefer_capitalized,
        );
    }
    symbol_name.to_string()
}

// Go: ls/autoimport/import_adder.go:481 replaceFirstIdentifierOfEntityName
fn replace_first_identifier_of_entity_name(
    factory: &NodeFactory,
    name: Node,
    new_identifier: Node,
) -> Node {
    if name.kind() == SyntaxKind::Identifier {
        return new_identifier;
    }
    let left = replace_first_identifier_of_entity_name(factory, name.left(), new_identifier);
    factory.new_qualified_name(left, name.right())
}

impl ImportAdderImpl {
    // Go: ls/autoimport/import_adder.go:491 getImportFixForSymbol
    // PORT: `ch` is the Go `adder.checker` (Go `view.checker`, the same
    // checker) that `View.GetFixes` reads (pinned decision).
    pub fn get_import_fix_for_symbol(
        &self,
        ch: &mut Checker,
        view: &View,
        exports: &[Rc<Export>],
        is_valid_type_only_use_site: bool,
    ) -> Option<Rc<Fix>> {
        // Go: core.FlatMap
        let mut fixes: Vec<Rc<Fix>> = Vec::new();
        for export in exports {
            fixes.extend(view.get_fixes(
                ch,
                export,
                false, /*forJSX*/
                is_valid_type_only_use_site,
                None, /*usagePosition*/
            ));
        }
        gostd::slices::sort_func(&mut fixes, |a, b| view.compare_fixes_for_ranking(a, b));
        if !fixes.is_empty() {
            return Some(fixes[0].clone());
        }
        None
    }
}
