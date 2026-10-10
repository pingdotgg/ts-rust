use crate::prelude::*;
use std::cell::OnceCell;
use std::rc::Weak;

// Port of internal/checker/emitresolver.go lines 1 to 330 and of
// internal/checker/emitsupport.go (ts#64649) except its implicit undefined
// helpers, which are in emit_resolver_p2.rs with the rest of emitresolver.go.
//
// PORT: layout of the resolver.
// - Go `EmitResolver` holds `*Checker` and `checkerMu`. Here the checkers
//   live in the program pool, so the resolver keeps the pool index of its
//   checker and borrows the checker for each locking call through
//   `with_checker` (Go `checkerMu.Lock()`). A nested lock panics on the
//   pool `RefCell`, as the Go mutex would deadlock.
// - A language-service pool keeps its checkers on the dispatch thread as
//   `Rc<RefCell<Checker>>`, where there is no compile worker checker. A
//   resolver from `new_emit_resolver_of_shared_checker` also keeps a weak
//   link to that `Rc`, and `with_checker` borrows the checker through it.
// - Go unexported methods take `c: &mut Checker` as their first argument
//   (the checker that Go reaches through `r.checker`). The emitsupport.go
//   methods (ts#64649) are `impl Checker` methods, so callers that already
//   hold the checker (node builder, symbol accessibility) call them directly.
// - Go exported methods are the `crate::printer::EmitResolver` trait impl
//   below. The trait impl must be one block, so it is in this file. Methods
//   ported in part 2 are inherent methods of `EmitResolver` with the trait
//   signature (Go exported name in snake case; `_exported` suffix when an
//   unexported Go method has the same snake name), and the trait impl calls
//   them.
// - The link stores are on the checker (`emit_resolver_links`, ts#64649).
//   The resolver is shared as `Rc<EmitResolver>`.

// Go: checker/emitresolver.go:20 JSXLinks
// Links for jsx
#[derive(Clone, Copy, Default)]
pub struct JSXLinks {
    pub import_ref: Node,
}

// Go: checker/emitresolver.go:26 DeclarationLinks
// Links for declarations
#[derive(Clone, Copy, Default)]
pub struct DeclarationLinks {
    pub is_visible: Tristate, // if declaration is depended upon by exported declarations
}

// Go: checker/emitresolver.go:30 DeclarationFileLinks
#[derive(Clone, Copy, Default)]
pub struct DeclarationFileLinks {
    pub aliases_marked: bool, // if file has had alias visibility marked
}

// Go: checker/emitresolver.go:34 EmitResolverLinks (ts#64649)
// The emit resolver links live on the checker, so every resolver of one
// checker shares them.
#[derive(Default)]
pub struct EmitResolverLinks {
    pub jsx_links: LinkStore<Node, JSXLinks>,
    pub declaration_links: LinkStore<Node, DeclarationLinks>,
    pub declaration_file_links: LinkStore<Node, DeclarationFileLinks>,
}

// Go: checker/emitresolver.go:40 EmitResolver
// PORT: `checker` and `checkerMu` are `checker_index`, or `shared_checker`
// for a language-service checker (see the file comment).
// PORT: Go `isValueAliasDeclaration` and `aliasMarkingVisitor` are method
// values cached to avoid closure allocation. Here they are plain calls to
// `is_value_alias_declaration_worker` and `alias_marking_visitor_worker`.
// PORT: Go caches referenceResolver binder.ReferenceResolver; part 2 builds one per call in get_reference_resolver (it has no state when all hooks are set).
pub struct EmitResolver {
    /// Index of the owning checker in the program checker pool.
    pub checker_index: usize,
    /// The owning checker when a language-service pool shares it as
    /// `Rc<RefCell<Checker>>` (set by `new_emit_resolver_of_shared_checker`).
    /// Weak, so that a resolver does not keep its checker alive.
    shared_checker: OnceCell<Weak<RefCell<Checker>>>,
    emit_context: Rc<EmitContext>,
    request_node_builder: OnceCell<Rc<RefCell<NodeBuilder>>>,
}

// Go: checker/emitresolver.go:50 newEmitResolver (ts#64649 adds the emit context)
// PORT: Go `checker.id` is `checker_index + 1` (see `Checker::new`). Go
// panics on a nil emit context; here the type has no nil.
pub fn new_emit_resolver(checker: &Checker, emit_context: Rc<EmitContext>) -> EmitResolver {
    EmitResolver {
        checker_index: checker.id as usize - 1,
        shared_checker: OnceCell::new(),
        emit_context,
        request_node_builder: OnceCell::new(),
    }
}

impl Checker {
    // Go: checker/checker.go:32716 NewEmitResolver (ts#64649)
    pub fn new_emit_resolver(&self, emit_context: Rc<EmitContext>) -> Rc<EmitResolver> {
        Rc::new(new_emit_resolver(self, emit_context))
    }

    // Go: checker/checker.go:32651 GetEmitResolver (removed upstream by ts#64649)
    // PORT: not in Go N'. ts#64649 replaced it with `NewEmitResolver`. The
    // declarations symbol tracker and declaration diagnostics use it to
    // reach the unsafe resolver methods with the checker in hand (they read
    // no resolver state). Its resolver has its own new emit context, as the
    // Go N' callers make one. `sync.Once` is the `Option` in
    // `emit_resolver`.
    pub fn get_emit_resolver(&mut self) -> Rc<EmitResolver> {
        if self.emit_resolver.is_none() {
            self.emit_resolver = Some(Rc::new(new_emit_resolver(self, new_emit_context())));
        }
        self.emit_resolver.clone().expect("emit resolver")
    }
}

// Go: checker/checker.go:32716 NewEmitResolver (ts#64649), for a checker that
// a language-service pool shares as `Rc<RefCell<Checker>>`.
pub fn new_emit_resolver_of_shared_checker(
    checker: &Rc<RefCell<Checker>>,
    emit_context: Rc<EmitContext>,
) -> Rc<EmitResolver> {
    let resolver = checker.borrow().new_emit_resolver(emit_context);
    resolver
        .shared_checker
        .get_or_init(|| Rc::downgrade(checker));
    resolver
}

impl EmitResolver {
    /// Go `r.checkerMu.Lock(); defer r.checkerMu.Unlock()` followed by use
    /// of `r.checker`: borrows the owning checker for `f`, through the
    /// shared checker link when it is set, otherwise from the compile pool.
    pub(crate) fn with_checker<R>(&self, f: impl FnOnce(&mut Checker) -> R) -> R {
        if let Some(checker) = self.shared_checker.get() {
            let checker = checker
                .upgrade()
                .expect("the checker of an emit resolver was dropped");
            return f(&mut *checker.borrow_mut());
        }
        with_checker_at(self.checker_index, f)
    }

    // Go: checker/emitresolver.go:61 EmitResolver.EmitContext (ts#64649)
    pub fn emit_context(&self) -> &Rc<EmitContext> {
        &self.emit_context
    }

    // Go: checker/emitresolver.go:65 EmitResolver.nodeBuilder (ts#64649)
    // One node builder per resolver, so its caches (module specifiers among
    // them) are shared by the requests of one emit.
    pub(crate) fn node_builder(&self, c: &Checker) -> Rc<RefCell<NodeBuilder>> {
        self.request_node_builder
            .get_or_init(|| Rc::new(RefCell::new(new_node_builder(c, self.emit_context.clone()))))
            .clone()
    }

    // Go: checker/emitsupport.go:12 Checker.isDeclarationVisible
    // PORT: not in Go N' (ts#64649 moved it to the checker). Kept for
    // `ls/findallreferences_p1.rs` until the ls lane ports its ts#64649 part.
    pub fn is_declaration_visible(&self, c: &mut Checker, node: Node) -> bool {
        c.is_declaration_visible(node)
    }

    // Go: checker/emitresolver.go:260 EmitResolver.aliasMarkingVisitorWorker
    // PORT: `paths` (aliaswalk1): when set, the walk enters only the
    // children on them (`AliasWalkPaths`). Go enters every child.
    fn alias_marking_visitor_worker(
        &self,
        c: &mut Checker,
        node: Node,
        paths: Option<&AliasWalkPaths>,
    ) -> bool {
        if let Some(target) = alias_marking_target(node) {
            self.mark_linked_aliases(c, target);
        }
        self.alias_marking_visit_children(c, node, paths)
    }

    // Go: `node.ForEachChild(r.aliasMarkingVisitor)`, with the children that
    // are not on `paths` left out.
    fn alias_marking_visit_children(
        &self,
        c: &mut Checker,
        node: Node,
        paths: Option<&AliasWalkPaths>,
    ) -> bool {
        node.for_each_child(|child| {
            paths.is_none_or(|paths| paths.enters(child))
                && self.alias_marking_visitor_worker(c, child, paths)
        })
    }

    // Go: checker/emitresolver.go:172 EmitResolver.markLinkedAliases
    // Sets the isVisible link on statements the Identifier or ExportName node points at
    // Follows chains of import d = a.b.c
    fn mark_linked_aliases(&self, c: &mut Checker, node: Node) {
        let mut export_symbol = SymbolId::NIL;
        if node.kind() != SyntaxKind::StringLiteral
            && node.parent().is_some()
            && (is_export_assignment(node.parent()) || is_common_js_module_exports(node.parent()))
        {
            export_symbol = c.resolve_name(
                node,
                node.text(),
                SymbolFlags::VALUE
                    | SymbolFlags::TYPE
                    | SymbolFlags::NAMESPACE
                    | SymbolFlags::ALIAS,
                None,  /*nameNotFoundMessage*/
                false, /*isUse*/
                false,
            );
        } else if node.parent().kind() == SyntaxKind::ExportSpecifier {
            export_symbol = c.get_target_of_export_specifier(
                node.parent(),
                SymbolFlags::VALUE
                    | SymbolFlags::TYPE
                    | SymbolFlags::NAMESPACE
                    | SymbolFlags::ALIAS,
                false,
            );
        }

        // PORT: Go keys `visited` by `ast.GetSymbolId`; the symbol handle is the same identity.
        // The Go call gives the symbol its id, so it is made too (`ValueSymbolLinkStore`).
        let mut visited: FxHashSet<SymbolId> = FxHashSet::default(); // guard against circular imports
        while export_symbol.is_some() {
            get_symbol_id(&c.symbols, export_symbol);
            if !visited.insert(export_symbol) {
                break;
            }

            let mut next_symbol = SymbolId::NIL;
            let declarations = c.sym(export_symbol).declarations.clone();
            for declaration in declarations {
                c.emit_resolver_links
                    .declaration_links
                    .get(declaration)
                    .is_visible = Tristate::True;

                if is_internal_module_import_equals_declaration(declaration) {
                    // Add the referenced top container visible
                    let internal_module_reference = declaration.module_reference();
                    let first_identifier = get_first_identifier(internal_module_reference);
                    let import_symbol = c.resolve_name(
                        declaration,
                        first_identifier.text(),
                        SymbolFlags::VALUE
                            | SymbolFlags::TYPE
                            | SymbolFlags::NAMESPACE
                            | SymbolFlags::ALIAS,
                        None,  /*nameNotFoundMessage*/
                        false, /*isUse*/
                        false,
                    );
                    next_symbol = import_symbol;
                }
            }

            export_symbol = next_symbol;
        }
    }

    // Go: checker/emitresolver.go:321 EmitResolver.RequiresAddingImplicitUndefinedUnsafe
    // PORT: body of Go `RequiresAddingImplicitUndefinedUnsafe` with the
    // checker passed in, for callers inside a node builder call.
    pub fn requires_adding_implicit_undefined_unsafe_worker(
        &self,
        c: &mut Checker,
        declaration: Node,
        symbol: SymbolId,
        enclosing_declaration: Node,
    ) -> bool {
        if !is_parse_tree_node(declaration) {
            return false;
        }
        // NO LOCKING - only should be called in contexts that already have a checker lock
        c.requires_adding_implicit_undefined(declaration, symbol, enclosing_declaration)
    }
}

// Go: checker/emitsupport.go (ts#64649): the declaration visibility and
// implicit undefined helpers that ts#64649 moved from the emit resolver to the
// checker.
impl Checker {
    // Go: checker/emitsupport.go:12 isDeclarationVisible (ts#64649 moved it from the emit resolver)
    pub fn is_declaration_visible(&mut self, node: Node) -> bool {
        if !is_parse_tree_node(node) {
            return false;
        }
        if node.is_nil() {
            return false;
        }

        let is_visible = self
            .emit_resolver_links
            .declaration_links
            .get(node)
            .is_visible;
        if is_visible == Tristate::Unknown {
            let value = if self.determine_if_declaration_is_visible(node) {
                Tristate::True
            } else {
                Tristate::False
            };
            self.emit_resolver_links
                .declaration_links
                .get(node)
                .is_visible = value;
        }
        self.emit_resolver_links
            .declaration_links
            .get(node)
            .is_visible
            == Tristate::True
    }

    // Go: checker/emitsupport.go:31 determineIfDeclarationIsVisible (ts#64649 moved it from the emit resolver)
    fn determine_if_declaration_is_visible(&mut self, node: Node) -> bool {
        match node.kind() {
            SyntaxKind::JsDocCallbackTag
            // ast.KindJSDocEnumTag, // !!! TODO: JSDoc @enum support?
            | SyntaxKind::JsDocTypedefTag => {
                // Top-level jsdoc type aliases are considered exported
                // First parent is comment node, second is hosting declaration or token; we only care about those tokens or declarations whose parent is a source file
                node.parent().is_some()
                    && node.parent().parent().is_some()
                    && node.parent().parent().parent().is_some()
                    && is_source_file(node.parent().parent().parent())
            }
            SyntaxKind::BindingElement => self.is_declaration_visible(node.parent().parent()),
            SyntaxKind::VariableDeclaration
            | SyntaxKind::ModuleDeclaration
            | SyntaxKind::ClassDeclaration
            | SyntaxKind::InterfaceDeclaration
            | SyntaxKind::TypeAliasDeclaration
            | SyntaxKind::JsTypeAliasDeclaration
            | SyntaxKind::FunctionDeclaration
            | SyntaxKind::EnumDeclaration
            | SyntaxKind::ImportEqualsDeclaration => {
                if is_variable_declaration(node) {
                    if is_binding_pattern(node.name()) && node.name().elements().is_empty() {
                        // If the binding pattern is empty, this variable declaration is not visible
                        return false;
                    }
                    // falls through
                }
                // External module augmentation is always visible
                // A @typedef at top-level in an external module is always visible
                if is_external_module_augmentation(node) || is_implicitly_exported_js_doc_declaration(node) {
                    return true;
                }
                let parent = get_declaration_container(node);
                // If the node is not exported or it is not ambient module element (except import declaration)
                if !self.get_combined_modifier_flags_cached(node).intersects(ModifierFlags::EXPORT)
                    && !(node.kind() != SyntaxKind::ImportEqualsDeclaration
                        && parent.kind() != SyntaxKind::SourceFile
                        && parent.flags().intersects(NodeFlags::AMBIENT))
                {
                    return is_global_source_file(parent);
                }
                // Exported members/ambient module elements (exception import declaration) are visible if parent is visible
                self.is_declaration_visible(parent)
            }

            SyntaxKind::PropertyDeclaration
            | SyntaxKind::PropertySignature
            | SyntaxKind::GetAccessor
            | SyntaxKind::SetAccessor
            | SyntaxKind::MethodDeclaration
            | SyntaxKind::MethodSignature => {
                if self.get_effective_declaration_flags(node, ModifierFlags::PRIVATE | ModifierFlags::PROTECTED)
                    != ModifierFlags::NONE
                {
                    // Private/protected properties/methods are not visible
                    return false;
                }
                // Public properties/methods are visible if its parents are visible, so:
                self.is_declaration_visible(node.parent())
            }

            SyntaxKind::Constructor
            | SyntaxKind::ConstructSignature
            | SyntaxKind::CallSignature
            | SyntaxKind::IndexSignature
            | SyntaxKind::Parameter
            | SyntaxKind::ModuleBlock
            | SyntaxKind::FunctionType
            | SyntaxKind::ConstructorType
            | SyntaxKind::TypeLiteral
            | SyntaxKind::TypeReference
            | SyntaxKind::ArrayType
            | SyntaxKind::TupleType
            | SyntaxKind::UnionType
            | SyntaxKind::IntersectionType
            | SyntaxKind::ParenthesizedType
            | SyntaxKind::NamedTupleMember => self.is_declaration_visible(node.parent()),

            // Default binding, import specifier and namespace import is visible
            // only on demand so by default it is not visible
            SyntaxKind::ImportClause | SyntaxKind::NamespaceImport | SyntaxKind::ImportSpecifier => false,

            // Type parameters are always visible
            SyntaxKind::TypeParameter => true,
            // Source file and namespace export are always visible
            SyntaxKind::SourceFile | SyntaxKind::NamespaceExportDeclaration => true,

            // Export assignments do not create name bindings outside the module
            SyntaxKind::ExportAssignment => false,

            // An `export {X}` (without a module specifier) is itself a visible re-export of
            // the named binding; it contributes to the symbol's external visibility.
            SyntaxKind::ExportSpecifier => {
                let export_decl = node.parent().parent();
                if is_export_declaration(export_decl) && export_decl.module_specifier().is_nil() {
                    return self.is_declaration_visible(export_decl.parent());
                }
                false
            }

            _ => false,
        }
    }

    // Go: checker/emitsupport.go:159 isEntityNameVisible (ts#64649 moved it from the emit resolver)
    pub fn is_entity_name_visible(
        &mut self,
        entity_name: Node,
        enclosing_declaration: Node,
        should_compute_alias_to_make_visible: bool,
    ) -> SymbolAccessibilityResult {
        if !is_parse_tree_node(entity_name) {
            return SymbolAccessibilityResult {
                accessibility: SymbolAccessibility::NOT_ACCESSIBLE,
                ..Default::default()
            };
        }

        let meaning = get_meaning_of_entity_name_reference(entity_name);
        let first_identifier = get_first_identifier(entity_name);

        let symbol = self.resolve_name(
            enclosing_declaration,
            first_identifier.text(),
            meaning,
            None,
            false,
            false,
        );

        if symbol.is_some()
            && self
                .sym(symbol)
                .flags
                .intersects(SymbolFlags::TYPE_PARAMETER)
            && meaning.intersects(SymbolFlags::TYPE)
        {
            return SymbolAccessibilityResult {
                accessibility: SymbolAccessibility::ACCESSIBLE,
                ..Default::default()
            };
        }

        if symbol.is_nil() && is_this_identifier(first_identifier) {
            let this_container = self.get_this_container(first_identifier, false, false);
            let sym = self.get_symbol_of_declaration(this_container);
            if self
                .is_symbol_accessible(sym, enclosing_declaration, meaning, false)
                .accessibility
                == SymbolAccessibility::ACCESSIBLE
            {
                return SymbolAccessibilityResult {
                    accessibility: SymbolAccessibility::ACCESSIBLE,
                    ..Default::default()
                };
            }
        }

        if symbol.is_nil() {
            return SymbolAccessibilityResult {
                accessibility: SymbolAccessibility::NOT_RESOLVED,
                error_symbol_name: first_identifier.text().to_string(),
                error_node: first_identifier,
                ..Default::default()
            };
        }

        let visible = self.has_visible_declarations(symbol, should_compute_alias_to_make_visible);
        if let Some(visible) = visible {
            return visible;
        }

        SymbolAccessibilityResult {
            accessibility: SymbolAccessibility::NOT_ACCESSIBLE,
            error_symbol_name: first_identifier.text().to_string(),
            error_node: first_identifier,
            ..Default::default()
        }
    }

    // Go: checker/emitsupport.go:200 noopAddVisibleAlias
    // Go: checker/emitsupport.go:202 hasVisibleDeclarations (ts#64649 moved it from the emit resolver)
    // PORT: Go picks `addVisibleAlias` or `noopAddVisibleAlias` once; here
    // one closure checks `should_compute_alias_to_make_visible`.
    // PORT: Go collects `maps.Values` of a map keyed by node id, so the order
    // of `AliasesToMakeVisible` is unspecified in Go. An `IndexMap` keeps
    // insertion order.
    pub fn has_visible_declarations(
        &mut self,
        symbol: SymbolId,
        should_compute_alias_to_make_visible: bool,
    ) -> Option<SymbolAccessibilityResult> {
        let mut aliases_to_make_visible_set: IndexMap<Node, Node> = IndexMap::new();

        let mut add_visible_alias =
            |c: &mut Checker, declaration: Node, aliasing_statement: Node| {
                if should_compute_alias_to_make_visible {
                    c.emit_resolver_links
                        .declaration_links
                        .get(declaration)
                        .is_visible = Tristate::True;
                    aliases_to_make_visible_set.insert(declaration, aliasing_statement);
                }
            };

        let symbol_flags = self.sym(symbol).flags;
        let declarations = self.sym(symbol).declarations.clone();
        for declaration in declarations {
            if is_identifier(declaration) {
                continue;
            }
            if !self.is_declaration_visible(declaration) {
                // Mark the unexported alias as visible if its parent is visible
                // because these kind of aliases can be used to name types in declaration file
                let any_import_syntax = get_any_import_syntax(declaration);
                if any_import_syntax.is_some()
                    && !has_syntactic_modifier(any_import_syntax, ModifierFlags::EXPORT) // import clause without export
                    && self.is_declaration_visible(any_import_syntax.parent())
                {
                    add_visible_alias(self, declaration, any_import_syntax);
                    continue;
                }
                if is_variable_declaration(declaration)
                    && is_variable_statement(declaration.parent().parent())
                    && !has_syntactic_modifier(declaration.parent().parent(), ModifierFlags::EXPORT) // unexported variable statement
                    && self.is_declaration_visible(declaration.parent().parent().parent())
                {
                    add_visible_alias(self, declaration, declaration.parent().parent());
                    continue;
                }
                if is_late_visibility_painted_statement(declaration) // unexported top-level statement
                    && !has_syntactic_modifier(declaration, ModifierFlags::EXPORT)
                    && self.is_declaration_visible(declaration.parent())
                {
                    add_visible_alias(self, declaration, declaration);
                    continue;
                }
                if is_binding_element(declaration) {
                    if symbol_flags.intersects(SymbolFlags::ALIAS)
                        && is_in_js_file(declaration)
                        && declaration.parent().is_some()
                        && declaration.parent().parent().is_some() // exported import-like top-level JS require statement
                        && is_variable_declaration(declaration.parent().parent())
                        && declaration.parent().parent().parent().parent().is_some()
                        && is_variable_statement(declaration.parent().parent().parent().parent())
                        && !has_syntactic_modifier(declaration.parent().parent().parent().parent(), ModifierFlags::EXPORT)
                        && declaration.parent().parent().parent().parent().parent().is_some() // check if the thing containing the variable statement is visible (ie, the file)
                        && self.is_declaration_visible(declaration.parent().parent().parent().parent().parent())
                    {
                        add_visible_alias(
                            self,
                            declaration,
                            declaration.parent().parent().parent().parent(),
                        );
                        continue;
                    }
                    if symbol_flags.intersects(SymbolFlags::BLOCK_SCOPED_VARIABLE) {
                        let root_declaration = walk_up_binding_elements_and_patterns(declaration);
                        if is_parameter_declaration(root_declaration) {
                            return None;
                        }
                        let variable_statement = root_declaration.parent().parent();
                        if !is_variable_statement(variable_statement) {
                            return None;
                        }
                        if has_syntactic_modifier(variable_statement, ModifierFlags::EXPORT) {
                            continue; // no alias to add, already exported
                        }
                        if !self.is_declaration_visible(variable_statement.parent()) {
                            return None; // not visible
                        }
                        add_visible_alias(self, declaration, variable_statement);
                        continue;
                    }
                }

                // Declaration is not visible
                return None;
            }
        }

        Some(SymbolAccessibilityResult {
            accessibility: SymbolAccessibility::ACCESSIBLE,
            aliases_to_make_visible: aliases_to_make_visible_set.values().copied().collect(),
            ..Default::default()
        })
    }

    // Go: checker/emitsupport.go:285 requiresAddingImplicitUndefined (ts#64649 moved it from the emit resolver)
    pub fn requires_adding_implicit_undefined(
        &mut self,
        declaration: Node,
        symbol: SymbolId,
        enclosing_declaration: Node,
    ) -> bool {
        if !is_parse_tree_node(declaration) {
            return false;
        }
        match declaration.kind() {
            SyntaxKind::PropertyDeclaration
            | SyntaxKind::PropertySignature
            | SyntaxKind::JsDocPropertyTag => {
                let mut symbol = symbol;
                if symbol.is_nil() {
                    symbol = self.get_symbol_of_declaration(declaration);
                }
                let t = self.get_type_of_symbol(symbol);
                let _ = self.mapped_symbol_links.has(symbol);
                let flags = self.sym(symbol).flags;
                flags.intersects(SymbolFlags::PROPERTY)
                    && flags.intersects(SymbolFlags::OPTIONAL)
                    && is_optional_declaration(declaration)
                    && self.reverse_mapped_symbol_links.has(symbol)
                    && self
                        .reverse_mapped_symbol_links
                        .get(symbol)
                        .mapped_type
                        .is_some()
                    && self.contains_non_missing_undefined_type(t)
            }
            SyntaxKind::Parameter | SyntaxKind::JsDocParameterTag => {
                self.requires_adding_implicit_undefined_worker(declaration, enclosing_declaration)
            }
            _ => panic!("Node cannot possibly require adding undefined"),
        }
    }
}

// Go: checker/emitresolver.go:249 isCommonJSModuleExports
pub(super) fn is_common_js_module_exports(node: Node) -> bool {
    if is_binary_expression(node)
        && is_expression_statement(node.parent())
        && is_source_file(node.parent().parent())
        && with_source_file_info(node.parent().parent(), |info| {
            info.common_js_module_indicator
        })
        .is_some()
    {
        let kind = get_assignment_declaration_kind(node);
        if kind == JSDeclarationKind::MODULE_EXPORTS || kind == JSDeclarationKind::EXPORTS_PROPERTY
        {
            return true;
        }
    }
    false
}

/// The nodes that the alias marking walk (`alias_marking_visitor_worker`)
/// must enter in a published, alias-free store with local parents: each
/// node of the store that the walk can act on (an ExportAssignment, an
/// ExportSpecifier, and in a CommonJS file a BinaryExpression that is the
/// expression of a source file statement, see `is_common_js_module_exports`)
/// and its ancestors. One bit per slot.
// PERF: aliaswalk1. Go walks every node of the file. The port did the
// same, and in an edited file each node data read is a scoped read: on an
// effect Option.ts edit the walk took 133 us against 56 us in Go (stable
// build, mini-743d, bpftrace). A scan of the kind column (no node data
// read) finds the nodes that the walk can act on, and their parent chains
// give the paths to them: 17 us.
// The Go parser sets the parent of each child of a node when it finishes
// the node (`overrideParentInImmediateChildren`, `set_parent_in_store_children`
// here), so the parent chain of a node of the tree is the path of
// `ForEachChild` calls that reaches it. The walk then enters the nodes on
// those paths in Go order and leaves out only subtrees with no node to act
// on: it marks the same aliases in the same order. A node that is not in
// the tree (JSDoc, a discarded speculative parse) can add a path that the
// walk never reaches.
struct AliasWalkPaths {
    /// The store id.
    file: usize,
    bits: Vec<u64>,
}

/// The node that the alias marking walk passes to `mark_linked_aliases` at
/// `node` (Go `aliasMarkingVisitorWorker`), if any.
#[inline]
fn alias_marking_target(node: Node) -> Option<Node> {
    match node.kind() {
        SyntaxKind::BinaryExpression => {
            (is_common_js_module_exports(node) && is_identifier(node.right())).then(|| node.right())
        }
        SyntaxKind::ExportAssignment => {
            (node.expression().kind() == SyntaxKind::Identifier).then(|| node.expression())
        }
        SyntaxKind::ExportSpecifier => Some(node.property_name_or_name()),
        _ => None,
    }
}

/// The `alias_marking_target`s of the walk under `node`, in walk order. It
/// leaves out the children that are not on `paths`; `None` enters every
/// child, as Go does. The debug check of `AliasWalkPaths` compares the two
/// lists. It reads only syntax, so it changes nothing.
fn alias_marking_targets(node: Node, paths: Option<&AliasWalkPaths>, targets: &mut Vec<Node>) {
    node.for_each_child(|child| {
        if paths.is_none_or(|paths| paths.enters(child)) {
            targets.extend(alias_marking_target(child));
            alias_marking_targets(child, paths, targets);
        }
        false
    });
}

impl AliasWalkPaths {
    /// The paths of source file `file`, a node of a published, alias-free
    /// store with local parents. `common_js` is true when the file has a
    /// `common_js_module_indicator`.
    fn new(file: Node, common_js: bool) -> Self {
        let store = file.file_index();
        let slots = file_store_slot_count(store);
        let mut paths = Self {
            file: store,
            bits: vec![0; slots.div_ceil(64)],
        };
        // Slot 0 is nil. Every other slot of an alias-free store is a node,
        // whose `Node::new` is the slot handle (`core::Node`).
        // PERF: the handle with no `Node::new`, so a slot costs one store
        // lookup (the kind read), not two.
        let base = (store as u64) << 32;
        for index in 1..slots {
            let node = Node(base | (index as u64 + 1));
            debug_assert_eq!(
                node,
                Node::new(store, crate::astdata::NodeId::new(index as u32)),
                "the slot handle of an alias-free store is its node"
            );
            let acts = match node.kind() {
                SyntaxKind::ExportAssignment | SyntaxKind::ExportSpecifier => true,
                // The tests of `is_common_js_module_exports` before the one
                // of the assignment kind. A node that is not in the tree can
                // have a nil parent, which those tests do not read.
                SyntaxKind::BinaryExpression => {
                    common_js && {
                        let statement = node.parent();
                        statement.is_some()
                            && is_expression_statement(statement)
                            && statement.parent().is_some()
                            && is_source_file(statement.parent())
                    }
                }
                _ => false,
            };
            if acts {
                paths.add_path(node);
            }
        }
        paths
    }

    /// Adds `node` and its ancestors, up to the first one that is on a path.
    fn add_path(&mut self, mut node: Node) {
        while node.is_some() && node.file_index() == self.file {
            let index = node.node_id().index();
            let (word, bit) = (index / 64, 1u64 << (index % 64));
            if self.bits[word] & bit != 0 {
                return;
            }
            self.bits[word] |= bit;
            node = node.parent();
        }
    }

    /// False when the walk can leave `node` out: a node of the store that is
    /// on no path. A node of another store is entered.
    fn enters(&self, node: Node) -> bool {
        if node.file_index() != self.file {
            return true;
        }
        let index = node.node_id().index();
        self.bits[index / 64] & (1u64 << (index % 64)) != 0
    }
}

// Go: checker/emitsupport.go:136 getMeaningOfEntityNameReference (ts#64649 moved it from emitresolver.go)
fn get_meaning_of_entity_name_reference(entity_name: Node) -> SymbolFlags {
    let parent = entity_name.parent();
    // get symbol of the first identifier of the entityName
    if parent.kind() == SyntaxKind::TypeQuery
        || parent.kind() == SyntaxKind::ExpressionWithTypeArguments && !is_part_of_type_node(parent)
        || parent.kind() == SyntaxKind::ComputedPropertyName
        || parent.kind() == SyntaxKind::TypePredicate && parent.parameter_name() == entity_name
        || parent.kind() == SyntaxKind::BinaryExpression
    {
        // Typeof value
        return SymbolFlags::VALUE | SymbolFlags::EXPORT_VALUE;
    }
    if entity_name.kind() == SyntaxKind::QualifiedName
        || entity_name.kind() == SyntaxKind::PropertyAccessExpression
        || parent.kind() == SyntaxKind::ImportEqualsDeclaration
        || (parent.kind() == SyntaxKind::QualifiedName && parent.left() == entity_name)
        || (parent.kind() == SyntaxKind::PropertyAccessExpression
            && parent.expression() == entity_name)
        || (parent.kind() == SyntaxKind::ElementAccessExpression
            && parent.expression() == entity_name)
    {
        // Left identifier from type reference or TypeAlias
        // Entity name of the import declaration
        return SymbolFlags::NAMESPACE;
    }
    // Type Reference or TypeAlias entity = Identifier
    SymbolFlags::TYPE
}

// Go: checker/emitresolver.go:17 `var _ printer.EmitResolver = (*EmitResolver)(nil)`
// PORT: Go exported methods. Methods from emitresolver.go lines 1 to 600 are
// ported here in full. The others call the part 2 inherent methods.
impl crate::printer::EmitResolver for EmitResolver {
    // Go: checker/emitresolver.go:61 EmitResolver.EmitContext (ts#64649)
    fn emit_context(&self) -> &Rc<EmitContext> {
        EmitResolver::emit_context(self)
    }

    // PORT: not in Go (Go `make(ast.SymbolTable)` in the declaration transformer).
    fn make_symbol_table(&self, entries: &[(&str, SymbolId)]) -> SymbolTable {
        self.with_checker(|c| {
            let table = c.symbols.new_table();
            for &(name, symbol) in entries {
                c.symbols.set(table, name, symbol);
            }
            table
        })
    }

    // Go: checker/emitresolver.go:858 EmitResolver.GetReferencedExportContainer
    fn get_referenced_export_container(&self, node: Node, prefix_locals: bool) -> Node {
        EmitResolver::get_referenced_export_container(self, node, prefix_locals)
    }

    // Go: checker/emitresolver.go:875 EmitResolver.GetReferencedImportDeclaration
    fn get_referenced_import_declaration(&self, node: Node) -> Node {
        EmitResolver::get_referenced_import_declaration(self, node)
    }

    // Go: checker/emitresolver.go:904 EmitResolver.GetReferencedValueDeclarations
    fn get_referenced_value_declarations(&self, node: Node) -> Vec<Node> {
        EmitResolver::get_referenced_value_declarations(self, node)
    }

    // Go: checker/emitresolver.go:889 EmitResolver.GetReferencedValueDeclaration
    fn get_referenced_value_declaration(&self, node: Node) -> Node {
        EmitResolver::get_referenced_value_declaration(self, node)
    }

    // Go: checker/emitresolver.go:924 EmitResolver.GetElementAccessExpressionName
    fn get_element_access_expression_name(&self, expression: Node) -> String {
        EmitResolver::get_element_access_expression_name(self, expression)
    }

    // Go: checker/emitresolver.go:935 EmitResolver.GetReferencedMemberValueDeclaration
    fn get_referenced_member_value_declaration(&self, node: Node) -> Node {
        EmitResolver::get_referenced_member_value_declaration(self, node)
    }

    // PORT: not in Go (Go reads `symbol.ValueDeclaration` directly).
    fn symbol_value_declaration(&self, symbol: SymbolId) -> Node {
        self.with_checker(|c| c.sym(symbol).value_declaration)
    }

    // Go: checker/emitresolver.go:697 EmitResolver.IsReferencedAliasDeclaration
    fn is_referenced_alias_declaration(&self, node: Node) -> bool {
        EmitResolver::is_referenced_alias_declaration(self, node)
    }

    // Go: checker/emitresolver.go:723 EmitResolver.IsValueAliasDeclaration
    fn is_value_alias_declaration(&self, node: Node) -> bool {
        EmitResolver::is_value_alias_declaration(self, node)
    }

    // Go: checker/emitresolver.go:789 EmitResolver.IsTopLevelValueImportEqualsWithEntityName
    fn is_top_level_value_import_equals_with_entity_name(&self, node: Node) -> bool {
        EmitResolver::is_top_level_value_import_equals_with_entity_name(self, node)
    }

    // Go: checker/emitresolver.go:808 EmitResolver.MarkLinkedReferencesRecursively
    fn mark_linked_references_recursively(&self, file: Node) {
        EmitResolver::mark_linked_references_recursively(self, file)
    }

    // Go: checker/emitresolver.go:832 EmitResolver.GetExternalModuleFileFromDeclaration
    fn get_external_module_file_from_declaration(&self, node: Node) -> Node {
        EmitResolver::get_external_module_file_from_declaration(self, node)
    }

    // Go: checker/emitresolver.go:1151 EmitResolver.GetEffectiveDeclarationFlags
    fn get_effective_declaration_flags(&self, node: Node, flags: ModifierFlags) -> ModifierFlags {
        EmitResolver::get_effective_declaration_flags(self, node, flags)
    }

    // Go: checker/emitresolver.go:1165 EmitResolver.GetTypeReferenceSerializationKind
    fn get_type_reference_serialization_kind(
        &self,
        name: Node,
        serial_scope: Node,
    ) -> TypeReferenceSerializationKind {
        EmitResolver::get_type_reference_serialization_kind(self, name, serial_scope)
    }

    // Go: checker/emitresolver.go:1158 EmitResolver.GetConstantValue
    fn get_constant_value(&self, node: Node) -> Option<LiteralValue> {
        EmitResolver::get_constant_value(self, node)
    }

    // Go: checker/emitresolver.go:53 EmitResolver.GetJsxFactoryEntity
    fn get_jsx_factory_entity(&self, location: Node) -> Node {
        self.with_checker(|c| c.get_jsx_factory_entity(location))
    }

    // Go: checker/emitresolver.go:59 EmitResolver.GetJsxFragmentFactoryEntity
    fn get_jsx_fragment_factory_entity(&self, location: Node) -> Node {
        self.with_checker(|c| c.get_jsx_fragment_factory_entity(location))
    }

    // Go: checker/emitresolver.go:869 EmitResolver.SetReferencedImportDeclaration
    fn set_referenced_import_declaration(&self, node: Node, ref_: Node) {
        EmitResolver::set_referenced_import_declaration(self, node, ref_)
    }

    // Go: checker/emitresolver.go:130 EmitResolver.PrecalculateDeclarationEmitVisibility
    fn precalculate_declaration_emit_visibility(&self, file: Node) {
        self.with_checker(|c| {
            if c.emit_resolver_links
                .declaration_file_links
                .get(file)
                .aliases_marked
            {
                return;
            }
            c.emit_resolver_links
                .declaration_file_links
                .get(file)
                .aliases_marked = true;
            // PERF: hono P7-C2, query Q7-1. The walk acts only on an
            // ExportAssignment, an ExportSpecifier or a BinaryExpression
            // that `is_common_js_module_exports` accepts, which needs the
            // file's `common_js_module_indicator`. In an alias-free store the
            // walk visits only nodes of this store, and with local parents
            // the source file that test reads is this file. So when the
            // store has neither kind and the file has no indicator, the walk
            // does nothing. Synthetic files have no store facts and walk.
            // aliaswalk1: otherwise it enters only the paths to the nodes
            // it can act on (`AliasWalkPaths`).
            let mut paths = None;
            if let Some(facts) = frozen_node_store_facts(file)
                && facts.alias_free
                && facts.parents_local
            {
                let common_js =
                    with_source_file_info(file, |info| info.common_js_module_indicator).is_some();
                if !facts.has_export_alias_kind && !common_js {
                    return;
                }
                paths = Some(AliasWalkPaths::new(file, common_js));
            }
            // TODO: Does this even *have* to be an upfront walk? If it's not possible for a
            // import a = a.b.c statement to chain into exposing a statement in a sibling scope,
            // it could at least be pushed into scope entry -  then it wouldn't need to be recursive.
            if let Some(paths) = paths.as_ref() {
                // The pruned walk marks the aliases of the full walk, in
                // the same order (debug builds only).
                debug_assert_eq!(
                    {
                        let mut targets = Vec::new();
                        alias_marking_targets(file, Some(paths), &mut targets);
                        targets
                    },
                    {
                        let mut targets = Vec::new();
                        alias_marking_targets(file, None, &mut targets);
                        targets
                    },
                    "the alias walk paths of {} leave out a target",
                    source_file_file_name(file)
                );
            }
            self.alias_marking_visit_children(c, file, paths.as_ref());
        })
    }

    // Go: checker/emitresolver.go:685 EmitResolver.IsSymbolAccessible
    fn is_symbol_accessible(
        &self,
        symbol: SymbolId,
        enclosing_declaration: Node,
        meaning: SymbolFlags,
        should_compute_alias_to_mark_visible: bool,
    ) -> SymbolAccessibilityResult {
        EmitResolver::is_symbol_accessible_exported(
            self,
            symbol,
            enclosing_declaration,
            meaning,
            should_compute_alias_to_mark_visible,
        )
    }

    // Go: checker/emitresolver.go:205 EmitResolver.IsEntityNameVisible
    fn is_entity_name_visible(
        &self,
        entity_name: Node,
        enclosing_declaration: Node,
    ) -> SymbolAccessibilityResult {
        self.with_checker(|c| c.is_entity_name_visible(entity_name, enclosing_declaration, true))
    }

    // Go: checker/emitresolver.go:675 EmitResolver.IsExpandoFunctionDeclaration
    fn is_expando_function_declaration(&self, node: Node) -> bool {
        EmitResolver::is_expando_function_declaration(self, node)
    }

    // Go: checker/emitresolver.go:660 EmitResolver.IsExpandoFunctionDeclarationUnsafe
    fn is_expando_function_declaration_unsafe(&self, node: Node) -> bool {
        EmitResolver::is_expando_function_declaration_unsafe(self, node)
    }

    // Go: checker/emitresolver.go:643 EmitResolver.IsLiteralConstDeclaration
    fn is_literal_const_declaration(&self, node: Node) -> bool {
        EmitResolver::is_literal_const_declaration(self, node)
    }

    // Go: checker/emitresolver.go:312 EmitResolver.RequiresAddingImplicitUndefined
    fn requires_adding_implicit_undefined(
        &self,
        node: Node,
        symbol: SymbolId,
        enclosing_declaration: Node,
    ) -> bool {
        if !is_parse_tree_node(node) {
            return false;
        }
        self.with_checker(|c| {
            c.requires_adding_implicit_undefined(node, symbol, enclosing_declaration)
        })
    }

    // Go: checker/emitresolver.go:123 EmitResolver.IsDeclarationVisible
    fn is_declaration_visible(&self, node: Node) -> bool {
        // Only lock on external API func to prevent deadlocks
        self.with_checker(|c| c.is_declaration_visible(node))
    }

    // Go: checker/emitresolver.go:916 EmitResolver.IsNameResolvable
    fn is_name_resolvable(&self, location: Node, name: &str) -> bool {
        EmitResolver::is_name_resolvable(self, location, name)
    }

    // Go: checker/emitresolver.go:508 EmitResolver.IsImportRequiredByAugmentation
    fn is_import_required_by_augmentation(&self, decl: Node) -> bool {
        // node = r.emitContext.ParseNode(node)
        if !is_parse_tree_node(decl) {
            return false;
        }
        let file = get_source_file_of_node(decl);
        let file_symbol = file.symbol();
        if file_symbol.is_nil() {
            // script file
            return false;
        }
        let import_target =
            crate::printer::EmitResolver::get_external_module_file_from_declaration(self, decl);
        if import_target.is_nil() {
            return false;
        }
        if import_target == file {
            return false;
        }
        self.with_checker(|c| {
            let exports = c.get_exports_of_module(file_symbol);
            for s in c.symbols.values(exports) {
                let merged = c.get_merged_symbol(s);
                if merged != s {
                    let merged_declarations = c.sym(merged).declarations.clone();
                    if !merged_declarations.is_empty() {
                        for d in merged_declarations {
                            let decl_file = get_source_file_of_node(d);
                            if decl_file == import_target {
                                return true;
                            }
                        }
                    }
                }
            }
            false
        })
    }

    // Go: checker/emitresolver.go:544 EmitResolver.IsDefinitelyReferenceToGlobalSymbolObject
    fn is_definitely_reference_to_global_symbol_object(&self, node: Node) -> bool {
        if !is_property_access_expression(node)
            || !is_identifier(node.name())
            || !is_property_access_expression(node.expression())
                && !is_identifier(node.expression())
        {
            return false;
        }
        if node.expression().kind() == SyntaxKind::Identifier {
            if node.expression().text() != "Symbol" {
                return false;
            }
            // Exactly `Symbol.something` and `Symbol` either does not resolve or definitely resolves to the global Symbol
            return self.with_checker(|c| {
                let resolved = c.get_resolved_symbol(node.expression());
                let global = c.get_global_symbol(
                    "Symbol",
                    SymbolFlags::VALUE | SymbolFlags::EXPORT_VALUE,
                    None, /*diagnostic*/
                );
                resolved == global
            });
        }
        if node.expression().expression().kind() != SyntaxKind::Identifier
            || node.expression().expression().text() != "globalThis"
            || node.expression().name().text() != "Symbol"
        {
            return false;
        }
        // Exactly `globalThis.Symbol.something` and `globalThis` resolves to the global `globalThis`
        self.with_checker(|c| {
            c.get_resolved_symbol(node.expression().expression()) == c.global_this_symbol
        })
    }

    // Go: checker/emitresolver.go:467 EmitResolver.IsImplementationOfOverload
    fn is_implementation_of_overload(&self, node: Node) -> bool {
        // node = r.emitContext.ParseNode(node)
        if !is_parse_tree_node(node) {
            return false;
        }
        if node_is_present(node.body()) {
            if is_get_accessor_declaration(node) || is_set_accessor_declaration(node) {
                return false; // Get or set accessors can never be overload implementations, but can have up to 2 signatures
            }
            return self.with_checker(|c| {
                let symbol = c.get_symbol_of_declaration(node);
                let signatures_of_symbol = c.get_signatures_of_symbol(symbol);
                // If this function body corresponds to function with multiple signature, it is implementation of overload
                // e.g.: function foo(a: string): string;
                //       function foo(a: number): number;
                //       function foo(a: any) { // This is implementation of the overloads
                //           return a;
                //       }
                if signatures_of_symbol.len() > 1 {
                    return true;
                }
                // If there is single signature for the symbol, it is overload if that signature isn't coming from the node
                // e.g.: function foo(a: string): string;
                //       function foo(a: any) { // This is implementation of the overloads
                //           return a;
                //       }
                if signatures_of_symbol.len() == 1 {
                    let signature = signatures_of_symbol[0];
                    if signature == c.get_signature_of_full_signature_type(node) {
                        return false;
                    }
                    let declaration = c.sig(signature).declaration;
                    if declaration != node && !declaration.flags().intersects(NodeFlags::JS_DOC) {
                        return true;
                    }
                }
                false
            });
        }
        false
    }

    // Go: checker/emitresolver.go:89 EmitResolver.GetEnumMemberValue
    fn get_enum_member_value(&self, node: Node) -> EvaluatorResult {
        // node = r.emitContext.ParseNode(node)
        if !is_parse_tree_node(node) {
            return new_result(None, false, false, false);
        }
        self.with_checker(|c| {
            c.compute_enum_member_values(node.parent());
            if !c.enum_member_links.has(node) {
                return new_result(None, false, false, false);
            }
            c.enum_member_links.get(node).value.clone()
        })
    }

    // Go: checker/emitresolver.go:71 EmitResolver.IsLateBound
    fn is_late_bound(&self, node: Node) -> bool {
        // TODO: Require an emitContext to construct an EmitResolver, remove all emitContext arguments
        // node = r.emitContext.ParseNode(node)
        if node.is_nil() {
            return false;
        }
        if !is_parse_tree_node(node) {
            return false;
        }
        self.with_checker(|c| {
            let symbol = c.get_symbol_of_declaration(node);
            if symbol.is_nil() {
                return false;
            }
            c.sym(symbol).check_flags.intersects(CheckFlags::LATE)
        })
    }

    // Go: checker/emitresolver.go:65 EmitResolver.IsOptionalParameter
    fn is_optional_parameter(&self, node: Node) -> bool {
        self.with_checker(|c| self.is_optional_parameter(c, node))
    }

    // Go: checker/emitresolver.go:1292 EmitResolver.IsThisPropertyAssignmentDeclarationRedundant
    fn is_this_property_assignment_declaration_redundant(&self, node: Node) -> bool {
        EmitResolver::is_this_property_assignment_declaration_redundant(self, node)
    }

    // Go: checker/emitresolver.go:1257 EmitResolver.GetPropertiesOfContainerFunction
    fn get_properties_of_container_function(&self, node: Node) -> Vec<SymbolId> {
        EmitResolver::get_properties_of_container_function(self, node)
    }

    // Go: checker/emitresolver.go:577 EmitResolver.RequiresAddingImplicitUndefinedUnsafe
    // PORT: Go takes no lock because its callers already hold it. The trait
    // method has no checker argument, so it borrows the checker from the pool.
    // It must not be called while a checker is lent out. Callers that hold
    // the checker use `requires_adding_implicit_undefined_unsafe_worker`.
    fn requires_adding_implicit_undefined_unsafe(
        &self,
        node: Node,
        symbol: SymbolId,
        enclosing_declaration: Node,
    ) -> bool {
        self.with_checker(|c| {
            self.requires_adding_implicit_undefined_unsafe_worker(
                c,
                node,
                symbol,
                enclosing_declaration,
            )
        })
    }

    // Go: checker/emitresolver.go:900 EmitResolver.GetReferencedValueDeclarationUnsafe
    // PORT: see `requires_adding_implicit_undefined_unsafe`. Callers that
    // hold the checker use `get_referenced_value_declaration_unsafe_worker`.
    fn get_referenced_value_declaration_unsafe(&self, node: Node) -> Node {
        EmitResolver::get_referenced_value_declaration_unsafe(self, node)
    }

    // Go: checker/emitresolver.go:662 EmitResolver.CreateTypeOfDeclaration
    fn create_type_of_declaration(
        &self,
        declaration: Node,
        enclosing_declaration: Node,
        flags: NodeBuilderFlags,
        internal_flags: InternalNodeBuilderFlags,
        tracker: EmitSymbolTracker,
    ) -> Node {
        EmitResolver::create_type_of_declaration(
            self,
            declaration,
            enclosing_declaration,
            flags,
            internal_flags,
            tracker,
        )
    }

    // Go: checker/emitresolver.go:640 EmitResolver.CreateReturnTypeOfSignatureDeclaration
    fn create_return_type_of_signature_declaration(
        &self,
        signature_declaration: Node,
        enclosing_declaration: Node,
        flags: NodeBuilderFlags,
        internal_flags: InternalNodeBuilderFlags,
        tracker: EmitSymbolTracker,
    ) -> Node {
        EmitResolver::create_return_type_of_signature_declaration(
            self,
            signature_declaration,
            enclosing_declaration,
            flags,
            internal_flags,
            tracker,
        )
    }

    // Go: checker/emitresolver.go:651 EmitResolver.CreateTypeParametersOfSignatureDeclaration
    fn create_type_parameters_of_signature_declaration(
        &self,
        signature_declaration: Node,
        enclosing_declaration: Node,
        flags: NodeBuilderFlags,
        internal_flags: InternalNodeBuilderFlags,
        tracker: EmitSymbolTracker,
    ) -> Vec<Node> {
        EmitResolver::create_type_parameters_of_signature_declaration(
            self,
            signature_declaration,
            enclosing_declaration,
            flags,
            internal_flags,
            tracker,
        )
    }

    // Go: checker/emitresolver.go:675 EmitResolver.CreateLiteralConstValue
    fn create_literal_const_value(&self, node: Node, tracker: EmitSymbolTracker) -> Node {
        EmitResolver::create_literal_const_value(self, node, tracker)
    }

    // Go: checker/emitresolver.go:735 EmitResolver.CreateTypeOfExpression
    fn create_type_of_expression(
        &self,
        expression: Node,
        enclosing_declaration: Node,
        flags: NodeBuilderFlags,
        internal_flags: InternalNodeBuilderFlags,
        tracker: EmitSymbolTracker,
    ) -> Node {
        EmitResolver::create_type_of_expression(
            self,
            expression,
            enclosing_declaration,
            flags,
            internal_flags,
            tracker,
        )
    }

    // Go: checker/emitresolver.go:746 EmitResolver.CreateLateBoundIndexSignatures
    fn create_late_bound_index_signatures(
        &self,
        container: Node,
        enclosing_declaration: Node,
        flags: NodeBuilderFlags,
        internal_flags: InternalNodeBuilderFlags,
        tracker: EmitSymbolTracker,
    ) -> Vec<Node> {
        EmitResolver::create_late_bound_index_signatures(
            self,
            container,
            enclosing_declaration,
            flags,
            internal_flags,
            tracker,
        )
    }

    // Go: checker/emitresolver.go:957 EmitResolver.TryJSTypeNodeToTypeNode
    fn try_js_type_node_to_type_node(
        &self,
        type_node: Node,
        enclosing_declaration: Node,
        flags: NodeBuilderFlags,
        internal_flags: InternalNodeBuilderFlags,
        tracker: EmitSymbolTracker,
    ) -> Node {
        EmitResolver::try_js_type_node_to_type_node(
            self,
            type_node,
            enclosing_declaration,
            flags,
            internal_flags,
            tracker,
        )
    }
}
