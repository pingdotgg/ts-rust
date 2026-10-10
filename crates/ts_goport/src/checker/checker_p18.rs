//! Port of Go `checker/checker.go` lines 15735-16703: qualified name
//! resolution, module exports and late binding, alias resolution, symbol
//! flags, and the type of a symbol (`getTypeOfSymbol` and its helpers,
//! `getTypeForVariableLikeDeclaration`).
//!
//! PORT: Go symbol tables are maps with reference semantics; here they are
//! `SymbolTable` handles into `self.symbols`. `make(ast.SymbolTable)` is
//! `self.symbols.new_table()`, `t[k]` is `self.symbols.get(t, k)` and
//! `t[k] = v` is `self.symbols.set(t, k, v)`.

use crate::prelude::*;

// Go: checker/checker.go:16460 ExportCollision
// PERF: Go stores `specifierText`, the text of the first export node's
// module specifier. We store that export node and make the text only when a
// TS2308 diagnostic uses it. Most entries never report, so this saves one
// String per re-exported name.
#[derive(Clone, Debug, Default)]
pub struct ExportCollision {
    pub export_node: Node,
    pub exports_with_duplicate: Vec<Node>,
}

// Go: checker/checker.go:16465 ExportCollisionTable
// PORT: Go `map[string]*ExportCollision`. Go iterates this map to report
// diagnostics; Go map order is random, so we keep insertion order
// (`IndexMap`) to be deterministic. Diagnostics are sorted later.
// PERF: keyed by interned `Name`, so an insert copies no text. The Fx hasher
// does not change the order: an `IndexMap` iterates in insertion order.
pub type ExportCollisionTable = FxIndexMap<Name, ExportCollision>;

// PORT: state captured by the Go `visit` closure in `getExportsOfModuleWorker`.
struct ExportsOfModuleVisitState {
    visited_symbols: Vec<SymbolId>,
    // PERF: interned names, so collecting them copies no text.
    non_type_only_names: FxHashSet<Name>,
    // PORT: Go nil map is an empty map; Go only reads it with lookups.
    type_only_export_star_map: FxHashMap<String, Node>,
}

// Go: checker/utilities.go:217 entityNameToString
// PORT: the checker wrapper `entityNameToString(name)` has the same snake
// name as the ast function `EntityNameToString(name, getTextOfNode)` that is
// already ported as `entity_name_to_string(name, Option<..>)`. To avoid the
// glob-import clash, this file calls the ast function with
// `scanner.GetTextOfNode`, which is exactly what the checker wrapper does.
fn checker_entity_name_to_string(name: Node) -> String {
    entity_name_to_string(name, Some(&get_text_of_node))
}

impl Checker {
    // Go: checker/checker.go:16147 resolveQualifiedName
    pub fn resolve_qualified_name(
        &mut self,
        name: Node,
        left: Node,
        right: Node,
        meaning: SymbolFlags,
        ignore_errors: bool,
        location: Node,
    ) -> SymbolId {
        let mut namespace = self.resolve_entity_name(
            left,
            SymbolFlags::NAMESPACE,
            ignore_errors,
            false, /*dontResolveAlias*/
            location,
        );
        if namespace.is_nil() || node_is_missing(right) {
            return SymbolId::NIL;
        }
        if namespace == self.unknown_symbol {
            return namespace;
        }
        let value_declaration = self.sym(namespace).value_declaration;
        if value_declaration.is_some()
            && is_in_js_file(value_declaration)
            && self.compiler_options.get_module_resolution_kind() != ModuleResolutionKind::BUNDLER
            && is_variable_declaration(value_declaration)
            && value_declaration.initializer().is_some()
            && self.is_common_js_require(value_declaration.initializer())
        {
            let module_name = value_declaration.initializer().arguments().get(0);
            let module_sym = self.resolve_external_module_name(
                module_name,
                module_name,
                false, /*ignoreErrors*/
                TypeId::NIL,
            );
            if module_sym.is_some() {
                let resolved_module_symbol = self
                    .resolve_external_module_symbol(module_sym, false /*dontResolveAlias*/);
                if resolved_module_symbol.is_some() {
                    namespace = resolved_module_symbol;
                }
            }
        }
        let text = right.text();
        let exports = self.get_exports_of_symbol(namespace);
        let s = self.get_symbol(exports, text, meaning);
        let mut symbol = self.get_merged_symbol(s);
        if symbol.is_nil() && self.sym(namespace).flags.intersects(SymbolFlags::ALIAS) {
            // `namespace` can be resolved further if there was a symbol merge with a re-export
            let resolved = self.resolve_alias(namespace);
            let exports = self.get_exports_of_symbol(resolved);
            let s = self.get_symbol(exports, text, meaning);
            symbol = self.get_merged_symbol(s);
        }
        if symbol.is_nil() {
            if !ignore_errors {
                let namespace_name =
                    self.get_fully_qualified_name(namespace, Node::NIL /*containingLocation*/);
                let declaration_name = declaration_name_to_string(right);
                let suggestion_for_nonexistent_module =
                    self.get_suggested_symbol_for_nonexistent_module(right, namespace);
                if suggestion_for_nonexistent_module.is_some() {
                    let suggestion = self.symbol_to_string(suggestion_for_nonexistent_module);
                    self.error(
                        right,
                        diag::X_0_has_no_exported_member_named_1_Did_you_mean_2,
                        args![namespace_name, declaration_name, suggestion],
                    );
                    return SymbolId::NIL;
                }
                let mut containing_qualified_name = Node::NIL;
                if is_qualified_name(name) {
                    containing_qualified_name = get_containing_qualified_name_node(name);
                }
                let can_suggest_typeof = self.global_object_type.is_some()
                    && meaning.intersects(SymbolFlags::TYPE)
                    && containing_qualified_name.is_some()
                    && !is_type_of_expression(containing_qualified_name.parent())
                    && self
                        .try_get_qualified_name_as_value(containing_qualified_name)
                        .is_some();
                if can_suggest_typeof {
                    self.error(
                        containing_qualified_name,
                        diag::X_0_refers_to_a_value_but_is_being_used_as_a_type_here_Did_you_mean_typeof_0,
                        args![checker_entity_name_to_string(containing_qualified_name)],
                    );
                    return SymbolId::NIL;
                }
                if meaning.intersects(SymbolFlags::NAMESPACE) {
                    if is_qualified_name(name.parent()) {
                        let exports = self.get_exports_of_symbol(namespace);
                        let s = self.get_symbol(exports, text, SymbolFlags::TYPE);
                        let exported_type_symbol = self.get_merged_symbol(s);
                        if exported_type_symbol.is_some() {
                            let qualified_right = name.parent().right();
                            let type_symbol_text = self.symbol_to_string(exported_type_symbol);
                            self.error(
                                qualified_right,
                                diag::Cannot_access_0_1_because_0_is_a_type_but_not_a_namespace_Did_you_mean_to_retrieve_the_type_of_the_property_1_in_0_with_0_1,
                                args![type_symbol_text, qualified_right.text()],
                            );
                            return SymbolId::NIL;
                        }
                    }
                }
                self.error(
                    right,
                    diag::Namespace_0_has_no_exported_member_1,
                    args![namespace_name, declaration_name],
                );
            }
        }
        symbol
    }

    // Go: checker/checker.go:16210 tryGetQualifiedNameAsValue
    pub fn try_get_qualified_name_as_value(&mut self, node: Node) -> SymbolId {
        let id = get_first_identifier(node);
        let mut symbol = self.resolve_name(
            id,
            id.text(),
            SymbolFlags::VALUE,
            None,  /*nameNotFoundMessage*/
            true,  /*isUse*/
            false, /*excludeGlobals*/
        );
        if symbol.is_nil() {
            return SymbolId::NIL;
        }
        let mut n = id;
        while is_qualified_name(n.parent()) {
            let t = self.get_type_of_symbol(symbol);
            symbol = self.get_property_of_type(t, n.parent().right().text());
            if symbol.is_nil() {
                return SymbolId::NIL;
            }
            n = n.parent();
        }
        symbol
    }

    // Go: checker/checker.go:16228 getSuggestedSymbolForNonexistentModule
    pub fn get_suggested_symbol_for_nonexistent_module(
        &mut self,
        name: Node,
        target_module: SymbolId,
    ) -> SymbolId {
        let exports = self.get_exports_of_module(target_module);
        let symbols = self.symbols.values(exports);
        self.get_spelling_suggestion_for_name(name.text(), &symbols, SymbolFlags::MODULE_MEMBER)
    }

    // Go: checker/checker.go:16232 getFullyQualifiedName
    pub fn get_fully_qualified_name(
        &mut self,
        symbol: SymbolId,
        containing_location: Node,
    ) -> String {
        let parent = self.sym(symbol).parent;
        if parent.is_some() {
            let parent_name = self.get_fully_qualified_name(parent, containing_location);
            return parent_name + "." + &self.symbol_to_string(symbol);
        }
        self.symbol_to_string_ex(
            symbol,
            containing_location,
            SymbolFlags::ALL,
            SymbolFormatFlags::DO_NOT_INCLUDE_SYMBOL_CHAIN | SymbolFormatFlags::ALLOW_ANY_NODE_KIND,
        )
    }

    // Go: checker/checker.go:16239 getExportsOfSymbol
    pub fn get_exports_of_symbol(&mut self, symbol: SymbolId) -> SymbolTable {
        let flags = self.sym(symbol).flags;
        if flags.intersects(SymbolFlags::LATE_BINDING_CONTAINER) {
            return self.get_resolved_members_or_exports_of_symbol(
                symbol,
                MembersOrExportsResolutionKind::RESOLVED_EXPORTS,
            );
        }
        if flags.intersects(SymbolFlags::MODULE) {
            return self.get_exports_of_module(symbol);
        }
        self.sym(symbol).exports
    }

    // Go: checker/checker.go:16249 getResolvedMembersOrExportsOfSymbol
    pub fn get_resolved_members_or_exports_of_symbol(
        &mut self,
        symbol: SymbolId,
        resolution_kind: MembersOrExportsResolutionKind,
    ) -> SymbolTable {
        let kind = resolution_kind.0 as usize;
        if self.members_and_exports_links.get(symbol)[kind].is_nil() {
            let is_static = resolution_kind == MembersOrExportsResolutionKind::RESOLVED_EXPORTS;
            let mut early_symbols = self.sym(symbol).exports;
            if !is_static {
                early_symbols = self.sym(symbol).members;
            } else if self.sym(symbol).flags.intersects(SymbolFlags::MODULE) {
                early_symbols = self.get_exports_of_module_worker(symbol).0;
            }
            self.members_and_exports_links.get(symbol)[kind] = early_symbols;
            // fill in any as-yet-unresolved late-bound members.
            let mut late_symbols = SymbolTable::NIL;
            let declarations = self.sym(symbol).declarations.clone();
            for &decl in declarations.iter() {
                for member in get_members_of_declaration(decl) {
                    if is_static == has_static_modifier(member) {
                        if self.has_late_bindable_name(member) {
                            if late_symbols.is_nil() {
                                late_symbols = self.symbols.new_table();
                            }
                            self.late_bind_member(symbol, early_symbols, late_symbols, member);
                        } else if self.has_late_bindable_index_signature(member) {
                            if late_symbols.is_nil() {
                                late_symbols = self.symbols.new_table();
                            }
                            self.late_bind_index_signature(symbol, early_symbols, late_symbols, member /* as LateBoundDeclaration | LateBoundBinaryExpressionDeclaration */);
                        }
                    }
                }
            }
            if is_static {
                let exports = self.sym(symbol).exports;
                let assignment_symbol = self
                    .symbols
                    .get(exports, INTERNAL_SYMBOL_NAME_ASSIGNMENT_DECLARATION);
                if assignment_symbol.is_some() {
                    let members = self.sym(assignment_symbol).declarations.clone();
                    for &member in members.iter() {
                        if self.has_late_bindable_name(member) {
                            if late_symbols.is_nil() {
                                late_symbols = self.symbols.new_table();
                            }
                            self.late_bind_member(symbol, early_symbols, late_symbols, member);
                        }
                    }
                }
            }
            let combined = self.combine_symbol_tables(early_symbols, late_symbols);
            self.members_and_exports_links.get(symbol)[kind] = combined;
        }
        self.members_and_exports_links.get(symbol)[kind]
    }

    // Go: checker/checker.go:16324 lateBindMember
    // Performs late-binding of a dynamic member. This performs the same function for
    // late-bound members that `declareSymbol` in binder.ts performs for early-bound
    // members.
    //
    // If a symbol is a dynamic name from a computed property, we perform an additional "late"
    // binding phase to attempt to resolve the name for the symbol from the type of the computed
    // property's expression. If the type of the expression is a string-literal, numeric-literal,
    // or unique symbol type, we can use that type as the name of the symbol.
    //
    // For example, given:
    //
    //	const x = Symbol();
    //
    //	interface I {
    //	  [x]: number;
    //	}
    //
    // The binder gives the property `[x]: number` a special symbol with the name "__computed".
    // In the late-binding phase we can type-check the expression `x` and see that it has a
    // unique symbol type which we can then use as the name of the member. This allows users
    // to define custom symbols that can be used in the members of an object type.
    //
    // @param parent The containing symbol for the member.
    // @param earlySymbols The early-bound symbols of the parent.
    // @param lateSymbols The late-bound symbols of the parent.
    // @param decl The member to bind.
    pub fn late_bind_member(
        &mut self,
        parent: SymbolId,
        early_symbols: SymbolTable,
        late_symbols: SymbolTable,
        decl: Node,
    ) -> SymbolId {
        debug_assert!(
            decl.symbol().is_some(),
            "The member is expected to have a symbol."
        );
        if self.symbol_node_links.get(decl).resolved_symbol.is_nil() {
            // In the event we attempt to resolve the late-bound name of this member recursively,
            // fall back to the early-bound name of this member.
            self.symbol_node_links.get(decl).resolved_symbol = decl.symbol();
            let decl_name = if is_binary_expression(decl) {
                decl.left()
            } else {
                decl.name()
            };
            let t = if is_element_access_expression(decl_name) {
                self.check_expression_cached(decl_name.argument_expression())
            } else {
                self.check_computed_property_name(decl_name)
            };
            if self.is_type_usable_as_property_name(t) {
                let member_name = self.get_property_name_from_type(t);
                let symbol_flags = self.sym(decl.symbol()).flags;
                // Get or add a late-bound symbol for the member. This allows us to merge late-bound accessor declarations.
                let mut late_symbol = self.symbols.get(late_symbols, &member_name);
                if late_symbol.is_nil() {
                    late_symbol =
                        self.new_symbol_ex(SymbolFlags::NONE, &member_name, CheckFlags::LATE);
                    self.symbols
                        .set(late_symbols, member_name.clone(), late_symbol);
                }
                // Report an error if there's a symbol declaration with the same name and conflicting flags.
                let early_symbol = self.symbols.get(early_symbols, &member_name);
                if self
                    .sym(late_symbol)
                    .flags
                    .intersects(get_excluded_symbol_flags(symbol_flags))
                {
                    // If we have an existing early-bound member, combine its declarations so that we can
                    // report an error at each declaration.
                    let declarations: Vec<Node> = if early_symbol.is_some() {
                        let mut d = self.sym(early_symbol).declarations.to_vec();
                        d.extend(self.sym(late_symbol).declarations.iter().copied());
                        d
                    } else {
                        self.sym(late_symbol).declarations.to_vec()
                    };
                    let mut name = member_name.clone();
                    if self.ty(t).flags.intersects(TypeFlags::UNIQUE_ES_SYMBOL) {
                        name = declaration_name_to_string(decl_name);
                    }
                    for d in declarations {
                        let name_of_declaration = get_name_of_declaration(d);
                        let location = if name_of_declaration.is_some() {
                            name_of_declaration
                        } else {
                            d
                        };
                        self.error(location, diag::Duplicate_identifier_0, args![name]);
                    }
                    let location = if decl_name.is_some() { decl_name } else { decl };
                    self.error(location, diag::Duplicate_identifier_0, args![name]);
                    let late_flags = self.sym(late_symbol).flags;
                    if late_flags.intersects(SymbolFlags::ACCESSOR)
                        && (late_flags & SymbolFlags::ACCESSOR)
                            != (symbol_flags & SymbolFlags::ACCESSOR)
                    {
                        self.sym_mut(late_symbol).flags |= SymbolFlags::ACCESSOR;
                    }
                    late_symbol =
                        self.new_symbol_ex(SymbolFlags::NONE, &member_name, CheckFlags::LATE);
                }
                self.value_symbol_links
                    .get_by_id(&self.symbols, late_symbol)
                    .name_type = t;
                self.add_declaration_to_late_bound_symbol(late_symbol, decl, symbol_flags);
                if self.sym(late_symbol).parent.is_nil() {
                    self.sym_mut(late_symbol).parent = parent;
                }
                self.symbol_node_links.get(decl).resolved_symbol = late_symbol;
            }
        }
        self.symbol_node_links.get(decl).resolved_symbol
    }

    // Go: checker/checker.go:16387 lateBindIndexSignature
    pub fn late_bind_index_signature(
        &mut self,
        parent: SymbolId,
        early_symbols: SymbolTable,
        late_symbols: SymbolTable,
        decl: Node,
    ) {
        // First, late bind the index symbol itself, if needed
        let mut index_symbol = self.symbols.get(late_symbols, INTERNAL_SYMBOL_NAME_INDEX);
        if index_symbol.is_nil() {
            let early = self.symbols.get(early_symbols, INTERNAL_SYMBOL_NAME_INDEX);
            if early.is_nil() {
                index_symbol = self.new_symbol_ex(
                    SymbolFlags::NONE,
                    INTERNAL_SYMBOL_NAME_INDEX,
                    CheckFlags::LATE,
                );
            } else {
                index_symbol = self.clone_symbol(early);
                self.sym_mut(index_symbol).check_flags |= CheckFlags::LATE;
            }
            self.symbols
                .set(late_symbols, INTERNAL_SYMBOL_NAME_INDEX, index_symbol);
        }
        // Then just add the computed name as a late bound declaration
        // (note: unlike `addDeclarationToLateBoundSymbol` we do not set up a `.lateSymbol` on `decl`'s links,
        // since that would point at an index symbol and not a single property symbol, like most consumers would expect)
        if self.sym(index_symbol).declarations.is_empty()
            || !self
                .sym(decl.symbol())
                .flags
                .intersects(SymbolFlags::REPLACEABLE_BY_METHOD)
        {
            self.sym_mut(index_symbol).declarations.push(decl);
        }
    }

    // Go: checker/checker.go:16408 isNotReplacableByMethod
    // PORT: Go package function; it reads symbol flags, so it is a Checker method.
    pub fn is_not_replacable_by_method(&self, decl: Node) -> bool {
        !self
            .sym(decl.symbol())
            .flags
            .intersects(SymbolFlags::REPLACEABLE_BY_METHOD)
    }

    // Go: checker/checker.go:16415 addDeclarationToLateBoundSymbol
    // Adds a declaration to a late-bound dynamic member. This performs the same function for
    // late-bound members that `addDeclarationToSymbol` in binder.ts performs for early-bound
    // members.
    pub fn add_declaration_to_late_bound_symbol(
        &mut self,
        symbol: SymbolId,
        member: Node,
        symbol_flags: SymbolFlags,
    ) {
        debug_assert!(
            self.sym(symbol).check_flags.intersects(CheckFlags::LATE),
            "Expected a late-bound symbol."
        );
        self.late_bound_links.get(member.symbol()).late_symbol = symbol;
        let member_symbol_flags = self.sym(member.symbol()).flags;
        if self.sym(symbol).declarations.is_empty()
            || !member_symbol_flags.intersects(SymbolFlags::REPLACEABLE_BY_METHOD)
        {
            let s = self.sym_mut(symbol);
            s.flags |= symbol_flags;
            s.declarations.push(member);
        } else if self
            .sym(symbol)
            .flags
            .intersects(SymbolFlags::REPLACEABLE_BY_METHOD)
            && member_symbol_flags.intersects(SymbolFlags::METHOD)
        {
            // Remove all replacable-by-method members, along with their flags.
            let old_declarations = self.sym(symbol).declarations.clone();
            let mut declarations: Vec<Node> = old_declarations
                .into_iter()
                .filter(|&d| self.is_not_replacable_by_method(d))
                .collect();
            declarations.push(member);
            let old_flags = self.sym(symbol).flags;
            // ts#64518 (Go N' checker.go:16492): the late-bound symbol stays transient.
            let mut flags = SymbolFlags::TRANSIENT;
            for &d in &declarations {
                flags |= self.sym(d.symbol()).flags;
            }
            if old_flags.intersects(SymbolFlags::ACCESSOR) {
                flags |= SymbolFlags::ACCESSOR;
            }
            let s = self.sym_mut(symbol);
            s.declarations = declarations.into();
            s.flags = flags;
        }
        if symbol_flags.intersects(SymbolFlags::VALUE) {
            set_value_declaration(&mut self.symbols, symbol, member);
        }
    }

    // Go: checker/checker.go:16443 getMembersOfSymbol
    // Gets a SymbolTable containing both the early- and late-bound members of a symbol.
    //
    // For a description of late-binding, see `lateBindMember`.
    pub fn get_members_of_symbol(&mut self, symbol: SymbolId) -> SymbolTable {
        if self
            .sym(symbol)
            .flags
            .intersects(SymbolFlags::LATE_BINDING_CONTAINER)
        {
            return self.get_resolved_members_or_exports_of_symbol(
                symbol,
                MembersOrExportsResolutionKind::RESOLVED_MEMBERS,
            );
        }
        self.sym(symbol).members
    }

    // Go: checker/checker.go:16450 getExportsOfModule
    pub fn get_exports_of_module(&mut self, module_symbol: SymbolId) -> SymbolTable {
        if self
            .module_symbol_links
            .get(module_symbol)
            .resolved_exports
            .is_nil()
        {
            let (exports, type_only_export_star_map) =
                self.get_exports_of_module_worker(module_symbol);
            let links = self.module_symbol_links.get(module_symbol);
            links.resolved_exports = exports;
            links.type_only_export_star_map = type_only_export_star_map;
        }
        self.module_symbol_links.get(module_symbol).resolved_exports
    }

    // Go: checker/checker.go:16467 getExportsOfModuleWorker
    pub fn get_exports_of_module_worker(
        &mut self,
        module_symbol: SymbolId,
    ) -> (SymbolTable, FxHashMap<String, Node>) {
        let mut module_symbol = module_symbol;
        let size_hint = if module_symbol.is_some() {
            self.symbols.len(self.sym(module_symbol).exports)
        } else {
            0
        };
        let mut state = ExportsOfModuleVisitState {
            visited_symbols: Vec::new(),
            non_type_only_names: FxHashSet::with_capacity_and_hasher(size_hint, Default::default()),
            type_only_export_star_map: FxHashMap::default(),
        };
        let mut original_module = SymbolId::NIL;
        if module_symbol.is_some() {
            let export_equals = self.symbols.get(
                self.sym(module_symbol).exports,
                INTERNAL_SYMBOL_NAME_EXPORT_EQUALS,
            );
            if self
                .resolve_symbol_ex(export_equals, false /*dontResolveAlias*/)
                .is_some()
            {
                original_module = module_symbol;
            }
        }
        // A module defined by an 'export=' consists of one export that needs to be resolved
        module_symbol =
            self.resolve_external_module_symbol(module_symbol, false /*dontResolveAlias*/);
        let mut exports =
            self.get_exports_of_module_worker_visit(&mut state, module_symbol, Node::NIL, false);
        if exports.is_nil() {
            exports = self.symbols.new_table();
        }
        // A CommonJS module defined by an 'export=' might also export typedefs, stored on the original module
        if original_module.is_some() && self.symbols.len(self.sym(original_module).exports) > 1 {
            let original_exports = self.symbols.values(self.sym(original_module).exports);
            for symbol in original_exports {
                let symbol_name = self.sym(symbol).name.clone();
                if symbol_name == INTERNAL_SYMBOL_NAME_EXPORT_EQUALS
                    || symbol_name == INTERNAL_SYMBOL_NAME_EXPORT_STAR
                {
                    continue;
                }
                let flags = self.get_symbol_flags(symbol);
                if flags.intersects(SymbolFlags::TYPE | SymbolFlags::NAMESPACE)
                    && !flags.intersects(SymbolFlags::VALUE)
                    && self.symbols.get(exports, &symbol_name).is_nil()
                {
                    self.symbols.set(exports, symbol_name, symbol);
                }
            }
        }
        for name in &state.non_type_only_names {
            state.type_only_export_star_map.remove(name.as_str());
        }
        (exports, state.type_only_export_star_map)
    }

    // Go: checker/checker.go:16473 getExportsOfModuleWorker.visit
    // PORT: the Go recursive closure `visit`; the captured variables live in
    // `state`.
    // The ES6 spec permits export * declarations in a module to circularly reference the module itself. For example,
    // module 'a' can 'export * from "b"' and 'b' can 'export * from "a"' without error.
    fn get_exports_of_module_worker_visit(
        &mut self,
        state: &mut ExportsOfModuleVisitState,
        symbol: SymbolId,
        export_star: Node,
        is_type_only: bool,
    ) -> SymbolTable {
        if !is_type_only && symbol.is_some() {
            // Add non-type-only names before checking if we've visited this module,
            // because we might have visited it via an 'export type *', and visiting
            // again with 'export *' will override the type-onlyness of its exports.
            for (name, _) in self.symbols.entries(self.sym(symbol).exports) {
                state.non_type_only_names.insert(name);
            }
        }
        if symbol.is_nil()
            || self.sym(symbol).exports.is_nil()
            || state.visited_symbols.contains(&symbol)
        {
            return SymbolTable::NIL;
        }
        state.visited_symbols.push(symbol);
        let symbol_exports = self.sym(symbol).exports;
        // Go: `symbols := maps.Clone(symbol.Exports)`. Exports is not nil here.
        let symbols = self.symbols.clone_table(symbol_exports);
        // All export * declarations are collected in an __export symbol by the binder
        let export_stars = self
            .symbols
            .get(symbol_exports, INTERNAL_SYMBOL_NAME_EXPORT_STAR);
        if export_stars.is_some() {
            let nested_symbols = self.symbols.new_table();
            let mut lookup_table = ExportCollisionTable::default();
            let declarations = self.sym(export_stars).declarations.clone();
            for &node in declarations.iter() {
                // PORT: Go `node.ModuleSpecifier()` panics for other kinds,
                // and the port `module_specifier()` returns nil. An export
                // specifier named with the byte 0xFE + "export" declares this
                // symbol too, and Go panics on it, so the port does too.
                if !matches!(
                    node.kind(),
                    SyntaxKind::ImportDeclaration
                        | SyntaxKind::JsImportDeclaration
                        | SyntaxKind::ExportDeclaration
                        | SyntaxKind::JsDocImportTag
                ) {
                    go_panic(format!(
                        "Unhandled case in Node.ModuleSpecifier: Kind{:?}",
                        node.kind()
                    ));
                }
                let import_attributes_type =
                    self.get_type_from_import_attributes(get_import_attributes(node));
                let resolved_module = self.resolve_external_module_name(
                    node,
                    node.module_specifier(),
                    false, /*ignoreErrors*/
                    import_attributes_type,
                );
                let exported_symbols = self.get_exports_of_module_worker_visit(
                    state,
                    resolved_module,
                    node,
                    is_type_only || node.is_type_only(),
                );
                self.extend_export_symbols(
                    nested_symbols,
                    exported_symbols,
                    Some(&mut lookup_table),
                    node,
                );
            }
            for (id, s) in &lookup_table {
                // It's not an error if the file with multiple `export *`s with duplicate names exports a member with that name itself
                // PERF: the empty test is first; it skips most entries without
                // a text compare. The three tests have no side effects.
                if s.exports_with_duplicate.is_empty()
                    || id == INTERNAL_SYMBOL_NAME_EXPORT_EQUALS
                    || self.symbols.get_name(symbols, id).is_some()
                {
                    continue;
                }
                // Go `s.specifierText`, made here (see `ExportCollision`).
                let specifier_text = get_text_of_node(s.export_node.module_specifier());
                for &node in &s.exports_with_duplicate {
                    let diagnostic = create_diagnostic_for_node(
                        node,
                        diag::Module_0_has_already_exported_a_member_named_1_Consider_explicitly_re_exporting_to_resolve_the_ambiguity,
                        args![specifier_text, id],
                    );
                    self.add_diagnostic(diagnostic);
                }
            }
            self.extend_export_symbols(symbols, nested_symbols, None, Node::NIL);
        }
        if export_star.is_some() && export_star.is_type_only() {
            for (name, _) in self.symbols.entries(symbols) {
                state
                    .type_only_export_star_map
                    .insert(name.to_string(), export_star);
            }
        }
        symbols
    }

    // Go: checker/checker.go:16554 extendExportSymbols
    // Extends one symbol table with another while collecting information on name collisions for error message generation into the `lookupTable` argument
    // Not passing `lookupTable` and `exportNode` disables this collection, and just extends the tables
    pub fn extend_export_symbols(
        &mut self,
        target: SymbolTable,
        source: SymbolTable,
        mut lookup_table: Option<&mut ExportCollisionTable>,
        export_node: Node,
    ) {
        // PERF: names are interned, so equal texts have equal ids. `default`
        // is interned once here and each name test is an id compare. The
        // target lookup uses the stored hash and an id compare (`get_slot`),
        // and the store reuses that slot. The loop changes `target` and
        // resolves symbols, so it walks an `entries` snapshot.
        let default_name = Name::from(INTERNAL_SYMBOL_NAME_DEFAULT);
        for (id, source_symbol) in self.symbols.entries(source) {
            if id == default_name {
                continue;
            }
            let (target_symbol, slot) = self.symbols.get_slot(target, &id);
            if target_symbol.is_nil() {
                self.symbols.set_slot(slot, source_symbol);
                if let Some(table) = lookup_table.as_deref_mut() {
                    if export_node.is_some() {
                        table.insert(
                            id,
                            ExportCollision {
                                export_node,
                                exports_with_duplicate: Vec::new(),
                            },
                        );
                    }
                }
            } else if lookup_table.is_some()
                && export_node.is_some()
                && self.resolve_symbol(target_symbol) != self.resolve_symbol(source_symbol)
            {
                let table = lookup_table.as_deref_mut().unwrap();
                // PORT: Go dereferences the map entry; a missing entry is a nil
                // pointer panic there.
                let s = table.get_mut(&id).expect("nil ExportCollision");
                s.exports_with_duplicate.push(export_node);
            }
        }
    }

    // Go: checker/checker.go:16574 ResolveAlias
    pub fn resolve_alias_exported(&mut self, symbol: SymbolId) -> (SymbolId, bool) {
        if symbol.is_nil() {
            return (SymbolId::NIL, false);
        }
        let resolved = self.resolve_alias(symbol);
        (resolved, resolved != self.unknown_symbol)
    }

    // Go: checker/checker.go:16585 resolveAlias
    // Resolve an alias symbol to the first target symbol in the resolution chain that includes some other
    // meaning. Pure aliases are eagerly resolved and any type-only markers are back-propagated to the original
    // symbol. The function panics if the argument is not a symbol with an alias meaning.
    pub fn resolve_alias(&mut self, symbol: SymbolId) -> SymbolId {
        if !self.sym(symbol).flags.intersects(SymbolFlags::ALIAS) {
            panic!("Should only get alias here");
        }
        if self.alias_symbol_links.get(symbol).alias_target.is_nil() {
            if !self.push_type_resolution(
                TypeSystemEntity::Symbol(symbol),
                TypeSystemPropertyName::ALIAS_TARGET,
            ) {
                return self.unknown_symbol;
            }
            let node = self.get_declaration_of_alias_symbol(symbol);
            if node.is_nil() {
                panic!(
                    "Unexpected nil in resolveAlias for symbol: {}",
                    self.symbol_to_string(symbol)
                );
            }
            let mut target = self.get_target_of_alias_declaration(node);
            if is_non_local_alias(
                &self.symbols,
                target,
                SymbolFlags::VALUE | SymbolFlags::TYPE | SymbolFlags::NAMESPACE,
            ) {
                // When the target is a pure alias, we transitively resolve and propagate any typeOnlyDeclaration
                target = self.resolve_indirection_alias(symbol, target);
            }
            let alias_target = if target.is_some() {
                target
            } else {
                self.unknown_symbol
            };
            self.alias_symbol_links.get(symbol).alias_target = alias_target;
            if !self.pop_type_resolution() {
                let symbol_text = self.symbol_to_string(symbol);
                self.error(
                    node,
                    diag::Circular_definition_of_import_alias_0,
                    args![symbol_text],
                );
                let unknown_symbol = self.unknown_symbol;
                self.alias_symbol_links.get(symbol).alias_target = unknown_symbol;
            }
        }
        self.alias_symbol_links.get(symbol).alias_target
    }

    // Go: checker/checker.go:16612 resolveIndirectionAlias
    pub fn resolve_indirection_alias(&mut self, source: SymbolId, target: SymbolId) -> SymbolId {
        let resolved = self.resolve_alias(target);
        let result = self.get_merged_symbol(resolved);
        let target_type_only_declaration =
            self.alias_symbol_links.get(target).type_only_declaration;
        if target_type_only_declaration.is_some() {
            let source_links = self.alias_symbol_links.get(source);
            if source_links.type_only_declaration.is_nil() {
                source_links.type_only_declaration = target_type_only_declaration;
            }
        }
        result
    }

    // Go: checker/checker.go:16622 tryResolveAlias
    pub fn try_resolve_alias(&mut self, symbol: SymbolId) -> SymbolId {
        let alias_target = self.alias_symbol_links.get(symbol).alias_target;
        if alias_target.is_some()
            || self.find_resolution_cycle_start_index(
                TypeSystemEntity::Symbol(symbol),
                TypeSystemPropertyName::ALIAS_TARGET,
            ) < 0
        {
            return self.resolve_alias(symbol);
        }
        SymbolId::NIL
    }

    // Go: checker/checker.go:16630 resolveAliasWithDeprecationCheck
    pub fn resolve_alias_with_deprecation_check(
        &mut self,
        symbol: SymbolId,
        location: Node,
    ) -> SymbolId {
        let mut symbol = symbol;
        if !self.sym(symbol).flags.intersects(SymbolFlags::ALIAS)
            || self.is_deprecated_symbol(symbol)
            || self.get_declaration_of_alias_symbol(symbol).is_nil()
        {
            return symbol;
        }
        let target_symbol = self.resolve_alias(symbol);
        if target_symbol == self.unknown_symbol {
            return target_symbol;
        }
        while self.sym(symbol).flags.intersects(SymbolFlags::ALIAS) {
            let target = self.get_immediate_aliased_symbol(symbol);
            if target.is_some() {
                if target == target_symbol {
                    break;
                }
                if !self.sym(target).declarations.is_empty() {
                    if self.is_deprecated_symbol(target) {
                        let declarations = self.sym(target).declarations.clone();
                        let name = self.sym(target).name.clone();
                        self.add_deprecated_suggestion(location, &declarations, &name);
                        break;
                    } else {
                        if symbol == target_symbol {
                            break;
                        }
                        symbol = target;
                    }
                }
            } else {
                break;
            }
        }
        target_symbol
    }

    // Go: checker/checker.go:16682 getSymbolFlags
    // Gets combined flags of a `symbol` and all alias targets it resolves to. `resolveAlias`
    // is typically recursive over chains of aliases, but stops mid-chain if an alias is merged
    // with another exported symbol, e.g.
    // ```ts
    // // a.ts
    // export const a = 0;
    // // b.ts
    // export { a } from "./a";
    // export type a = number;
    // // c.ts
    // import { a } from "./b";
    // ```
    // Calling `resolveAlias` on the `a` in c.ts would stop at the merged symbol exported
    // from b.ts, even though there is still more alias to resolve. Consequently, if we were
    // trying to determine if the `a` in c.ts has a value meaning, looking at the flags on
    // the local symbol and on the symbol returned by `resolveAlias` is not enough.
    // @returns SymbolFlags.All if `symbol` is an alias that ultimately resolves to `unknown`;
    // combined flags of all alias targets otherwise.
    pub fn get_symbol_flags(&mut self, symbol: SymbolId) -> SymbolFlags {
        self.get_symbol_flags_ex(
            symbol, false, /*excludeTypeOnlyMeanings*/
            false, /*excludeLocalMeanings*/
        )
    }

    // Go: checker/checker.go:16686 getSymbolFlagsEx
    pub fn get_symbol_flags_ex(
        &mut self,
        symbol: SymbolId,
        exclude_type_only_meanings: bool,
        exclude_local_meanings: bool,
    ) -> SymbolFlags {
        let mut symbol = symbol;
        let mut seen_symbols: FxHashSet<SymbolId> = FxHashSet::default();
        let mut flags = SymbolFlags::NONE;
        if !exclude_local_meanings {
            flags = self.sym(symbol).flags;
        }
        while self.sym(symbol).flags.intersects(SymbolFlags::ALIAS) {
            if exclude_type_only_meanings && self.get_type_only_alias_declaration(symbol).is_some()
            {
                break;
            }
            let resolved = self.resolve_alias(symbol);
            let target = self.get_export_symbol_of_value_symbol_if_exported(resolved);
            if target == self.unknown_symbol {
                return SymbolFlags::ALL;
            }
            if self.sym(target).flags.intersects(SymbolFlags::ALIAS) {
                // Optimization - try to avoid creating or adding to `seenSymbols` if possible
                if target == symbol || seen_symbols.contains(&target) {
                    break;
                }
                if seen_symbols.is_empty() {
                    seen_symbols.insert(symbol);
                }
                seen_symbols.insert(target);
            }
            flags |= self.sym(target).flags;
            symbol = target;
        }
        flags
    }

    // Go: checker/checker.go:16716 getDeclarationOfAliasSymbol
    pub fn get_declaration_of_alias_symbol(&self, symbol: SymbolId) -> Node {
        self.sym(symbol)
            .declarations
            .iter()
            .rev()
            .copied()
            .find(|&d| is_alias_symbol_declaration(d))
            .unwrap_or(Node::NIL)
    }

    // Go: checker/checker.go:16720 getTypeOfSymbolWithDeferredType
    pub fn get_type_of_symbol_with_deferred_type(&mut self, symbol: SymbolId) -> TypeId {
        // One link lookup on the cached hit. The miss returns the value it
        // just stored, as Go returns links.resolvedType.
        let cached = self
            .value_symbol_links
            .get_by_id(&self.symbols, symbol)
            .resolved_type;
        if cached.is_some() {
            return cached;
        }
        let deferred = self.deferred_symbol_links.get(symbol);
        let parent = deferred.parent;
        if parent.is_nil() {
            // Go reads `deferred.parent.flags`: a symbol of another checker
            // has no links here.
            go_nil_dereference();
        }
        let constituents = deferred.constituents.clone();
        let resolved_type = if self.ty(parent).flags.intersects(TypeFlags::UNION) {
            self.get_union_type(&constituents)
        } else {
            self.get_intersection_type(&constituents)
        };
        self.value_symbol_links
            .get_by_id(&self.symbols, symbol)
            .resolved_type = resolved_type;
        resolved_type
    }

    // Go: checker/checker.go:16733 getWriteTypeOfSymbolWithDeferredType
    pub fn get_write_type_of_symbol_with_deferred_type(&mut self, symbol: SymbolId) -> TypeId {
        if self
            .value_symbol_links
            .get_by_id(&self.symbols, symbol)
            .write_type
            .is_nil()
        {
            let deferred = self.deferred_symbol_links.get(symbol);
            let parent = deferred.parent;
            let write_constituents = deferred.write_constituents.clone();
            let write_type = if !write_constituents.is_empty() {
                if self.ty(parent).flags.intersects(TypeFlags::UNION) {
                    self.get_union_type(&write_constituents)
                } else {
                    self.get_intersection_type(&write_constituents)
                }
            } else {
                self.get_type_of_symbol_with_deferred_type(symbol)
            };
            self.value_symbol_links
                .get_by_id(&self.symbols, symbol)
                .write_type = write_type;
        }
        self.value_symbol_links
            .get_by_id(&self.symbols, symbol)
            .write_type
    }

    // Go: checker/checker.go:16753 getWriteTypeOfSymbol
    // Distinct write types come only from set accessors, but synthetic union and intersection
    // properties deriving from set accessors will either pre-compute or defer the union or
    // intersection of the writeTypes of their constituents.
    pub fn get_write_type_of_symbol(&mut self, symbol: SymbolId) -> TypeId {
        if symbol.is_nil() {
            // Go reads `symbol.CheckFlags` and panics on nil, for example
            // on the target of an instantiated symbol of another checker,
            // which has no links here.
            go_nil_dereference();
        }
        let check_flags = self.sym(symbol).check_flags;
        let flags = self.sym(symbol).flags;
        if check_flags.intersects(CheckFlags::SYNTHETIC_PROPERTY) {
            if check_flags.intersects(CheckFlags::DEFERRED_TYPE) {
                return self.get_write_type_of_symbol_with_deferred_type(symbol);
            }
            let links = self.value_symbol_links.get_by_id(&self.symbols, symbol);
            return if links.write_type.is_some() {
                links.write_type
            } else {
                links.resolved_type
            };
        }
        if flags.intersects(SymbolFlags::PROPERTY) {
            let t = self.get_type_of_symbol(symbol);
            return self.remove_missing_type(t, flags.intersects(SymbolFlags::OPTIONAL));
        }
        if flags.intersects(SymbolFlags::ACCESSOR) {
            if check_flags.intersects(CheckFlags::INSTANTIATED) {
                return self.get_write_type_of_instantiated_symbol(symbol);
            }
            return self.get_write_type_of_accessors(symbol);
        }
        self.get_type_of_symbol(symbol)
    }

    // Go: checker/checker.go:16773 GetTypeOfSymbolAtLocation
    pub fn get_type_of_symbol_at_location(&mut self, symbol: SymbolId, location: Node) -> TypeId {
        let symbol = self.get_export_symbol_of_value_symbol_if_exported(symbol);
        let mut location = location;
        if location.is_some() {
            // If we have an identifier or a property access at the given location, if the location is
            // an dotted name expression, and if the location is not an assignment target, obtain the type
            // of the expression (which will reflect control flow analysis). If the expression indeed
            // resolved to the given symbol, return the narrowed type.
            if (is_identifier(location) || is_private_identifier(location))
                && !(is_jsx_tag_name(location)
                    || is_jsx_attribute(location.parent())
                    || is_jsx_namespaced_name(location.parent()))
            {
                if is_right_side_of_qualified_name_or_property_access(location) {
                    location = location.parent();
                }
                if is_expression_node(location)
                    && (!is_assignment_target(location) || is_write_access(location))
                {
                    let t = if is_write_access(location)
                        && location.kind() == SyntaxKind::PropertyAccessExpression
                    {
                        self.check_property_access_expression(
                            location,
                            CheckMode::NORMAL,
                            true, /*writeOnly*/
                        )
                    } else {
                        self.get_type_of_expression(location)
                    };
                    let resolved_symbol = self.symbol_node_links.get(location).resolved_symbol;
                    if self.get_export_symbol_of_value_symbol_if_exported(resolved_symbol) == symbol
                    {
                        return self.remove_optional_type_marker(t);
                    }
                }
            }
            if is_declaration_name(location)
                && is_set_accessor_declaration(location.parent())
                && self
                    .get_annotated_accessor_type_node(location.parent())
                    .is_some()
            {
                return self.get_write_type_of_accessors(location.parent().symbol());
            }
            // The location isn't a reference to the given symbol, meaning we're being asked
            // a hypothetical question of what type the symbol would have if there was a reference
            // to it at the given location. Since we have no control flow information for the
            // hypothetical reference (control flow information is created and attached by the
            // binder), we simply return the declared type of the symbol.
            if is_right_side_of_access_expression(location) && is_write_access(location.parent()) {
                return self.get_write_type_of_symbol(symbol);
            }
        }
        self.get_non_missing_type_of_symbol(symbol)
    }

    // Go: checker/checker.go:16812 getTypeOfSymbol
    pub fn get_type_of_symbol(&mut self, symbol: SymbolId) -> TypeId {
        if symbol.is_nil() {
            // Go reads `symbol.CheckFlags` and panics on nil, for example
            // on the target of an instantiated symbol of another checker,
            // which has no links here.
            go_nil_dereference();
        }
        let s = self.sym(symbol);
        let (check_flags, flags) = (s.check_flags, s.flags);
        if check_flags.intersects(CheckFlags::DEFERRED_TYPE) {
            return self.get_type_of_symbol_with_deferred_type(symbol);
        }
        if check_flags.intersects(CheckFlags::INSTANTIATED) {
            return self.get_type_of_instantiated_symbol(symbol);
        }
        if check_flags.intersects(CheckFlags::MAPPED) {
            return self.get_type_of_mapped_symbol(symbol);
        }
        if check_flags.intersects(CheckFlags::REVERSE_MAPPED) {
            return self.get_type_of_reverse_mapped_symbol(symbol);
        }
        if flags.intersects(SymbolFlags::ACCESSOR) {
            return self.get_type_of_accessors(symbol);
        }
        if flags.intersects(SymbolFlags::VARIABLE | SymbolFlags::PROPERTY) {
            return self.get_type_of_variable_or_parameter_or_property(symbol);
        }
        if flags.intersects(
            SymbolFlags::FUNCTION
                | SymbolFlags::METHOD
                | SymbolFlags::CLASS
                | SymbolFlags::ENUM
                | SymbolFlags::VALUE_MODULE,
        ) {
            return self.get_type_of_func_class_enum_module(symbol);
        }
        if flags.intersects(SymbolFlags::ENUM_MEMBER) {
            return self.get_type_of_enum_member(symbol);
        }
        if flags.intersects(SymbolFlags::ALIAS) {
            return self.get_type_of_alias(symbol);
        }
        self.error_type
    }

    // Go: checker/checker.go:16843 getNonMissingTypeOfSymbol
    pub fn get_non_missing_type_of_symbol(&mut self, symbol: SymbolId) -> TypeId {
        let t = self.get_type_of_symbol(symbol);
        let is_optional = self.sym(symbol).flags.intersects(SymbolFlags::OPTIONAL);
        self.remove_missing_type(t, is_optional)
    }

    // Go: checker/checker.go:16847 getTypeOfInstantiatedSymbol
    pub fn get_type_of_instantiated_symbol(&mut self, symbol: SymbolId) -> TypeId {
        // One link lookup on the cached hit. The miss returns the value it
        // just stored, as Go returns links.resolvedType.
        let links = self.value_symbol_links.get_by_id(&self.symbols, symbol);
        if links.resolved_type.is_some() {
            return links.resolved_type;
        }
        let (target, mapper) = (links.target, links.mapper);
        let t = self.get_type_of_symbol(target);
        let resolved_type = self.instantiate_type(t, mapper);
        self.value_symbol_links
            .get_by_id(&self.symbols, symbol)
            .resolved_type = resolved_type;
        resolved_type
    }

    // Go: checker/checker.go:16855 getWriteTypeOfInstantiatedSymbol
    pub fn get_write_type_of_instantiated_symbol(&mut self, symbol: SymbolId) -> TypeId {
        // One link lookup on the cached hit, as in get_type_of_instantiated_symbol.
        let links = self.value_symbol_links.get_by_id(&self.symbols, symbol);
        if links.write_type.is_some() {
            return links.write_type;
        }
        let (target, mapper) = (links.target, links.mapper);
        let t = self.get_write_type_of_symbol(target);
        let write_type = self.instantiate_type(t, mapper);
        self.value_symbol_links
            .get_by_id(&self.symbols, symbol)
            .write_type = write_type;
        write_type
    }

    // Go: checker/checker.go:16863 getTypeOfVariableOrParameterOrProperty
    pub fn get_type_of_variable_or_parameter_or_property(&mut self, symbol: SymbolId) -> TypeId {
        // One link lookup on the cached hit.
        let cached = self
            .value_symbol_links
            .get_by_id(&self.symbols, symbol)
            .resolved_type;
        if cached.is_some() {
            return cached;
        }
        let t = self.get_type_of_variable_or_parameter_or_property_worker(symbol);
        if t.is_nil() {
            panic!("Unexpected nil type");
        }
        // For a contextually typed parameter it is possible that a type has already
        // been assigned (in assignTypeToParameterAndFixTypeParameters), and we want
        // to preserve this type. In fact, we need to _prefer_ that type, but it won't
        // be assigned until contextual typing is complete, so we need to defer in
        // cases where contextual typing may take place.
        // The worker can set resolved_type, so read the links again here.
        if self
            .value_symbol_links
            .get_by_id(&self.symbols, symbol)
            .resolved_type
            .is_nil()
            && !self.is_parameter_of_context_sensitive_signature(symbol)
        {
            self.value_symbol_links
                .get_by_id(&self.symbols, symbol)
                .resolved_type = t;
        }
        t
    }

    // Go: checker/checker.go:16883 isParameterOfContextSensitiveSignature
    pub fn is_parameter_of_context_sensitive_signature(&mut self, symbol: SymbolId) -> bool {
        let mut decl = self.sym(symbol).value_declaration;
        if decl.is_nil() {
            return false;
        }
        if is_binding_element(decl) {
            decl = walk_up_binding_elements_and_patterns(decl);
        }
        if is_parameter_declaration(decl) {
            return self.is_context_sensitive_function_or_object_literal_method(decl.parent());
        }
        false
    }

    // Go: checker/checker.go:16897 getTypeOfVariableOrParameterOrPropertyWorker
    pub fn get_type_of_variable_or_parameter_or_property_worker(
        &mut self,
        symbol: SymbolId,
    ) -> TypeId {
        // Handle prototype property
        if self.sym(symbol).flags.intersects(SymbolFlags::PROTOTYPE) {
            return self.get_type_of_prototype_property(symbol);
        }
        // CommonsJS require and module both have type any.
        if symbol == self.require_symbol {
            return self.any_type;
        }
        let declaration = self.sym(symbol).value_declaration;
        go_assert!(declaration.is_some());
        if is_source_file(declaration) && is_json_source_file(declaration) {
            let statements = declaration.statements();
            if statements.is_empty() {
                return self.empty_object_type;
            }
            let t = self.check_expression(statements.get(0).expression());
            let t = self.get_widened_literal_type(t);
            return self.get_widened_type(t);
        }
        // Handle variable, parameter or property
        if !self.push_type_resolution(
            TypeSystemEntity::Symbol(symbol),
            TypeSystemPropertyName::TYPE,
        ) {
            return self.report_circularity_error(symbol);
        }
        if self
            .sym(symbol)
            .flags
            .intersects(SymbolFlags::MODULE_EXPORTS)
        {
            if self.sym(symbol).name == "exports" {
                let value_declaration_symbol = self.sym(symbol).value_declaration.symbol();
                let module_symbol = self.resolve_external_module_symbol(
                    value_declaration_symbol,
                    false, /*dontResolveAlias*/
                );
                return self.get_type_of_symbol(module_symbol);
            }
            let members = self.sym(symbol).members;
            return self.new_anonymous_type(symbol, members, &[], &[], &[]);
        }
        let result = match declaration.kind() {
            SyntaxKind::Parameter
            | SyntaxKind::PropertyDeclaration
            | SyntaxKind::PropertySignature
            | SyntaxKind::VariableDeclaration
            | SyntaxKind::BindingElement => {
                // only report diagnostics for context-insensitive parameters - context-sensitive ones may have their type fixed to something else
                let report_errors = !self.is_parameter_of_context_sensitive_signature(symbol);
                self.get_widened_type_for_variable_like_declaration(declaration, report_errors)
            }
            SyntaxKind::PropertyAssignment => {
                self.check_property_assignment(declaration, CheckMode::NORMAL)
            }
            SyntaxKind::ShorthandPropertyAssignment => {
                self.check_shorthand_property_assignment(
                    declaration,
                    true, /*inDestructuringPattern*/
                    CheckMode::NORMAL,
                )
            }
            SyntaxKind::MethodDeclaration => {
                self.check_object_literal_method(declaration, CheckMode::NORMAL)
            }
            SyntaxKind::ExportAssignment => {
                if declaration.type_().is_some() {
                    self.get_type_from_type_node(declaration.type_())
                } else {
                    let t = self.check_expression_cached(declaration.expression());
                    self.widen_type_for_variable_like_declaration(
                        t,
                        declaration,
                        false, /*reportErrors*/
                    )
                }
            }
            SyntaxKind::BinaryExpression | SyntaxKind::CallExpression => {
                self.get_widened_type_for_assignment_declaration(symbol)
            }
            SyntaxKind::JsxAttribute => self.check_jsx_attribute(declaration, CheckMode::NORMAL),
            SyntaxKind::EnumMember => self.get_type_of_enum_member(symbol),
            kind => panic!(
                "Unhandled case in getTypeOfVariableOrParameterOrPropertyWorker: {:?}",
                kind
            ),
        };
        if !self.pop_type_resolution() {
            return self.report_circularity_error(symbol);
        }
        result
    }

    // Go: checker/checker.go:16966 getWidenedTypeForVariableLikeDeclaration
    // Return the type associated with a variable, parameter, or property declaration. In the simple case this is the type
    // specified in a type annotation or inferred from an initializer. However, in the case of a destructuring declaration it
    // is a bit more involved. For example:
    //
    //	var [x, s = ""] = [1, "one"];
    //
    // Here, the array literal [1, "one"] is contextually typed by the type [any, string], which is the implied type of the
    // binding pattern [x, s = ""]. Because the contextual type is a tuple type, the resulting type of [1, "one"] is the
    // tuple type [number, string]. Thus, the type inferred for 'x' is number and the type inferred for 's' is string.
    pub fn get_widened_type_for_variable_like_declaration(
        &mut self,
        declaration: Node,
        report_errors: bool,
    ) -> TypeId {
        let t = self.get_type_for_variable_like_declaration(
            declaration,
            true, /*includeOptionality*/
            CheckMode::NORMAL,
        );
        self.widen_type_for_variable_like_declaration(t, declaration, report_errors)
    }

    // Go: checker/checker.go:16971 getTypeForVariableLikeDeclaration
    // Return the inferred type for a variable, parameter, or property declaration
    pub fn get_type_for_variable_like_declaration(
        &mut self,
        declaration: Node,
        include_optionality: bool,
        check_mode: CheckMode,
    ) -> TypeId {
        // A variable declared in a for..in statement is of type string, or of type keyof T when the
        // right hand expression is of a type parameter type.
        if is_variable_declaration(declaration) {
            let grand_parent = declaration.parent().parent();
            match grand_parent.kind() {
                SyntaxKind::ForInStatement => {
                    let t = self.check_expression_ex(
                        grand_parent.expression(),
                        check_mode, /*checkMode*/
                    );
                    let t = self.get_non_nullable_type_if_needed(t);
                    let index_type = self.get_index_type(t);
                    if self
                        .ty(index_type)
                        .flags
                        .intersects(TypeFlags::TYPE_PARAMETER | TypeFlags::INDEX)
                    {
                        return self.get_extract_string_type(index_type);
                    }
                    return self.string_type;
                }
                SyntaxKind::ForOfStatement => {
                    // checkRightHandSideOfForOf will return undefined if the for-of expression type was
                    // missing properties/signatures required to get its iteratedType (like
                    // [Symbol.iterator] or next). This may be because we accessed properties from anyType,
                    // or it may have led to an error inside getElementTypeOfIterable.
                    return self.check_right_hand_side_of_for_of(grand_parent);
                }
                _ => {}
            }
        } else if is_binding_element(declaration) {
            return self.get_type_for_binding_element(declaration);
        }
        let is_property = is_property_declaration(declaration)
            && !has_accessor_modifier(declaration)
            || is_property_signature_declaration(declaration);
        let is_optional = include_optionality && is_optional_declaration(declaration);
        // Use type from type annotation if one is present
        let declared_type = self.try_get_type_from_type_node(declaration);
        if is_catch_clause_variable_declaration_or_binding_element(declaration) {
            if declared_type.is_some() {
                // If the catch clause is explicitly annotated with any or unknown, accept it, otherwise error.
                if self
                    .ty(declared_type)
                    .flags
                    .intersects(TypeFlags::ANY_OR_UNKNOWN)
                {
                    return declared_type;
                }
                return self.error_type;
            }
            // If the catch clause is not explicitly annotated, treat it as though it were explicitly
            // annotated with unknown or any, depending on useUnknownInCatchVariables.
            if self.use_unknown_in_catch_variables {
                return self.unknown_type;
            } else {
                return self.any_type;
            }
        }
        if declared_type.is_some() {
            return self.add_optionality_ex(declared_type, is_property, is_optional);
        }
        if self.no_implicit_any
            && is_variable_declaration(declaration)
            && !is_binding_pattern(declaration.name())
            && !self
                .get_combined_modifier_flags_cached(declaration)
                .intersects(ModifierFlags::EXPORT)
            && !declaration.flags().intersects(NodeFlags::AMBIENT)
        {
            // If --noImplicitAny is on or the declaration is in a Javascript file,
            // use control flow tracked 'any' type for non-ambient, non-exported var or let variables with no
            // initializer or a 'null' or 'undefined' initializer.
            let initializer = declaration.initializer();
            if !self
                .get_combined_node_flags_cached(declaration)
                .intersects(NodeFlags::CONSTANT)
                && (initializer.is_nil() || self.is_null_or_undefined(initializer))
            {
                return self.auto_type;
            }
            // Use control flow tracked 'any[]' type for non-ambient, non-exported variables with an empty array
            // literal initializer.
            if initializer.is_some() && is_empty_array_literal(initializer) {
                return self.auto_array_type;
            }
        }
        if is_parameter_declaration(declaration) {
            if declaration.symbol().is_nil() {
                // parameters of function types defined in JSDoc in TS files don't have symbols
                return TypeId::NIL;
            }
            let fn_ = declaration.parent();
            // For a parameter of a set accessor, use the type of the get accessor if one is present
            if is_set_accessor_declaration(fn_) && self.has_bindable_name(fn_) {
                let accessor_symbol = self.get_symbol_of_declaration(declaration.parent());
                let getter = get_declaration_of_kind(
                    &self.symbols,
                    accessor_symbol,
                    SyntaxKind::GetAccessor,
                );
                if getter.is_some() {
                    let getter_signature = self.get_signature_from_declaration(getter);
                    let this_parameter = self.get_accessor_this_parameter(fn_);
                    if this_parameter.is_some() && declaration == this_parameter {
                        // Use the type from the *getter*
                        debug_assert!(this_parameter.type_().is_nil());
                        let getter_this_parameter = self.sig(getter_signature).this_parameter;
                        return self.get_type_of_symbol(getter_this_parameter);
                    }
                    return self.get_return_type_of_signature(getter_signature);
                }
            }
            let t = self.get_parameter_type_of_full_signature(fn_, declaration);
            if t.is_some() {
                return t;
            }
            // Use contextual parameter type if one is available
            let t = if self.sym(declaration.symbol()).name == INTERNAL_SYMBOL_NAME_THIS {
                self.get_contextual_this_parameter_type(fn_)
            } else {
                self.get_contextually_typed_parameter_type(declaration)
            };
            if t.is_some() {
                return self.add_optionality_ex(t, false /*isProperty*/, is_optional);
            }
        }
        // Use the type of the initializer expression if one is present and the declaration is
        // not a parameter of a contextually typed function
        if declaration.initializer().is_some() {
            let initializer_type = self.check_declaration_initializer(
                declaration,
                check_mode,
                TypeId::NIL, /*contextualType*/
            );
            let t = self.widen_type_inferred_from_initializer(declaration, initializer_type);
            return self.add_optionality_ex(t, is_property, is_optional);
        }
        if self.no_implicit_any && is_property_declaration(declaration) {
            // We have a property declaration with no type annotation or initializer, in noImplicitAny mode or a .js file.
            // Use control flow analysis of this.xxx assignments in the constructor or static block to determine the type of the property.
            if !has_static_modifier(declaration) {
                let constructor = find_constructor_declaration(declaration.parent());
                let t = if constructor.is_some() {
                    self.get_flow_type_in_constructor(declaration.symbol(), constructor)
                } else if declaration
                    .modifier_flags()
                    .intersects(ModifierFlags::AMBIENT)
                {
                    self.get_type_of_property_in_base_class(declaration.symbol())
                } else {
                    TypeId::NIL
                };
                if t.is_nil() {
                    return TypeId::NIL;
                }
                return self.add_optionality_ex(t, true /*isProperty*/, is_optional);
            } else {
                let static_blocks: Vec<Node> = declaration
                    .parent()
                    .members()
                    .iter()
                    .filter(|&m| is_class_static_block_declaration(m))
                    .collect();
                let t = if !static_blocks.is_empty() {
                    self.get_flow_type_in_static_blocks(declaration.symbol(), &static_blocks)
                } else if declaration
                    .modifier_flags()
                    .intersects(ModifierFlags::AMBIENT)
                {
                    self.get_type_of_property_in_base_class(declaration.symbol())
                } else {
                    TypeId::NIL
                };
                if t.is_nil() {
                    return TypeId::NIL;
                }
                return self.add_optionality_ex(t, true /*isProperty*/, is_optional);
            }
        }
        if is_jsx_attribute(declaration) {
            // if JSX attribute doesn't have initializer, by default the attribute will have boolean value of true.
            // I.e <Elem attr /> is sugar for <Elem attr={true} />
            return self.true_type;
        }
        // If the declaration specifies a binding pattern and is not a parameter of a contextually
        // typed function, use the type implied by the binding pattern
        if is_binding_pattern(declaration.name()) {
            return self.get_type_from_binding_pattern(
                declaration.name(),
                false, /*includePatternInType*/
                true,  /*reportErrors*/
            );
        }
        // No type specified and nothing can be inferred
        TypeId::NIL
    }
}
