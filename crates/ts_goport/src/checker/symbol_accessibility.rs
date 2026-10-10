//! Port of checker/symbolaccessibility.go.

use crate::prelude::*;
use crate::printer::{SymbolAccessibility, SymbolAccessibilityResult};

impl Checker {
    // Go: checker/symbolaccessibility.go:11 IsTypeSymbolAccessible
    pub fn is_type_symbol_accessible(
        &mut self,
        type_symbol: SymbolId,
        enclosing_declaration: Node,
    ) -> bool {
        let access = self.is_symbol_accessible_worker(
            type_symbol,
            enclosing_declaration,
            SymbolFlags::TYPE,
            false,
            true,
        );
        access.accessibility == SymbolAccessibility::ACCESSIBLE
    }

    // Go: checker/symbolaccessibility.go:16 IsValueSymbolAccessible
    pub fn is_value_symbol_accessible(
        &mut self,
        symbol: SymbolId,
        enclosing_declaration: Node,
    ) -> bool {
        let access = self.is_symbol_accessible_worker(
            symbol,
            enclosing_declaration,
            SymbolFlags::VALUE,
            false,
            true,
        );
        access.accessibility == SymbolAccessibility::ACCESSIBLE
    }

    // Go: checker/symbolaccessibility.go:21 IsSymbolAccessibleByFlags
    pub fn is_symbol_accessible_by_flags(
        &mut self,
        symbol: SymbolId,
        enclosing_declaration: Node,
        flags: SymbolFlags,
    ) -> bool {
        let access =
            self.is_symbol_accessible_worker(symbol, enclosing_declaration, flags, false, false);
        access.accessibility == SymbolAccessibility::ACCESSIBLE
    }

    // Go: checker/symbolaccessibility.go:26 IsAnySymbolAccessible
    // PORT: Go returns a `*printer.SymbolAccessibilityResult`; nil is `None`.
    pub fn is_any_symbol_accessible(
        &mut self,
        symbols: &[SymbolId],
        enclosing_declaration: Node,
        initial_symbol: SymbolId,
        meaning: SymbolFlags,
        should_compute_aliases_to_make_visible: bool,
        allow_modules: bool,
    ) -> Option<SymbolAccessibilityResult> {
        if symbols.is_empty() {
            return None;
        }

        let mut had_accessible_chain = SymbolId::NIL;
        let mut early_module_bail = false;
        for &symbol in symbols {
            // Symbol is accessible if it by itself is accessible
            let accessible_symbol_chain =
                self.get_accessible_symbol_chain(symbol, enclosing_declaration, meaning, false);
            if !accessible_symbol_chain.is_empty() {
                had_accessible_chain = symbol;
                // ts#64649, Go N' symbolaccessibility.go:40: the checker method.
                let has_accessible_declarations = self.has_visible_declarations(
                    accessible_symbol_chain[0],
                    should_compute_aliases_to_make_visible,
                );
                if has_accessible_declarations.is_some() {
                    return has_accessible_declarations;
                }
            }
            if allow_modules {
                let decls = self.sym(symbol).declarations.clone();
                if decls
                    .iter()
                    .any(|&d| has_non_global_augmentation_external_module_symbol(d))
                {
                    if should_compute_aliases_to_make_visible {
                        early_module_bail = true;
                        // Generally speaking, we want to use the aliases that already exist to refer to a module, if present
                        // In order to do so, we need to find those aliases in order to retain them in declaration emit; so
                        // if we are in declaration emit, we cannot use the fast path for module visibility until we've exhausted
                        // all other visibility options (in order to capture the possible aliases used to reference the module)
                        continue;
                    }
                    // Any meaning of a module symbol is always accessible via an `import` type
                    return Some(SymbolAccessibilityResult {
                        accessibility: SymbolAccessibility::ACCESSIBLE,
                        ..Default::default()
                    });
                }
            }

            // If we haven't got the accessible symbol, it doesn't mean the symbol is actually inaccessible.
            // It could be a qualified symbol and hence verify the path
            let containers = self.get_containers_of_symbol(symbol, enclosing_declaration, meaning);
            let mut next_meaning = meaning;
            if initial_symbol == symbol {
                next_meaning = get_qualified_left_meaning(meaning);
            }
            let parent_result = self.is_any_symbol_accessible(
                &containers,
                enclosing_declaration,
                initial_symbol,
                next_meaning,
                should_compute_aliases_to_make_visible,
                allow_modules,
            );
            if parent_result.is_some() {
                return parent_result;
            }
        }

        if early_module_bail {
            return Some(SymbolAccessibilityResult {
                accessibility: SymbolAccessibility::ACCESSIBLE,
                ..Default::default()
            });
        }

        if had_accessible_chain.is_some() {
            let mut module_name = String::new();
            if had_accessible_chain != initial_symbol {
                module_name = self.symbol_to_string_ex(
                    had_accessible_chain,
                    enclosing_declaration,
                    SymbolFlags::NAMESPACE,
                    SymbolFormatFlags::ALLOW_ANY_NODE_KIND,
                );
            }
            let error_symbol_name = self.symbol_to_string_ex(
                initial_symbol,
                enclosing_declaration,
                meaning,
                SymbolFormatFlags::ALLOW_ANY_NODE_KIND,
            );
            return Some(SymbolAccessibilityResult {
                accessibility: SymbolAccessibility::NOT_ACCESSIBLE,
                error_symbol_name,
                error_module_name: module_name,
                ..Default::default()
            });
        }
        None
    }

    // Go: checker/symbolaccessibility.go:117 getWithAlternativeContainers
    pub fn get_with_alternative_containers(
        &mut self,
        container: SymbolId,
        symbol: SymbolId,
        enclosing_declaration: Node,
        meaning: SymbolFlags,
    ) -> Vec<SymbolId> {
        let container_decls = self.sym(container).declarations.clone();
        let mut additional_containers = Vec::new();
        for d in container_decls {
            let s = self.get_file_symbol_if_file_symbol_export_equals_container(d, container);
            if s.is_some() {
                additional_containers.push(s);
            }
        }
        let mut reexport_containers = Vec::new();
        if enclosing_declaration.is_some() {
            reexport_containers =
                self.get_alternative_containing_modules(symbol, enclosing_declaration);
        }
        let object_literal_container =
            self.get_variable_declaration_of_object_literal(container, meaning);
        let left_meaning = get_qualified_left_meaning(meaning);
        if enclosing_declaration.is_some()
            && self.sym(container).flags.intersects(left_meaning)
            && !self
                .get_accessible_symbol_chain(
                    container,
                    enclosing_declaration,
                    SymbolFlags::NAMESPACE,
                    false,
                )
                .is_empty()
        {
            // This order expresses a preference for the real container if it is in scope
            let mut res = vec![container];
            res.extend(additional_containers);
            res.extend(reexport_containers);
            if object_literal_container.is_some() {
                res.push(object_literal_container);
            }
            return res;
        }
        // we potentially have a symbol which is a member of the instance side of something - look for a variable in scope with the container's type
        // which may be acting like a namespace (eg, `Symbol` acts like a namespace when looking up `Symbol.toStringTag`)
        let mut variable_matches: Vec<SymbolId> = Vec::new();
        let container_flags = self.sym(container).flags;
        if (meaning == SymbolFlags::VALUE && !container_flags.intersects(left_meaning))
            && container_flags.intersects(SymbolFlags::TYPE)
            && {
                let declared = self.get_declared_type_of_symbol(container);
                self.ty(declared).flags.intersects(TypeFlags::OBJECT)
            }
        {
            self.some_symbol_table_in_scope(enclosing_declaration, &mut |c, t, _, _, _, _| {
                let mut found = false;
                for s in c.symbols.values(t) {
                    if c.sym(s).flags.intersects(left_meaning) {
                        let st = c.get_type_of_symbol(s);
                        let dt = c.get_declared_type_of_symbol(container);
                        if st == dt {
                            variable_matches.push(s);
                            found = true;
                        }
                    }
                }
                found
            });
            self.sort_symbols(&mut variable_matches);
        }

        let mut res = Vec::new();
        res.extend(variable_matches);
        res.extend(additional_containers);
        res.push(container);
        if object_literal_container.is_some() {
            res.push(object_literal_container);
        }
        res.extend(reexport_containers);
        res
    }

    // Go: checker/symbolaccessibility.go:168 getAlternativeContainingModules
    pub fn get_alternative_containing_modules(
        &mut self,
        symbol: SymbolId,
        enclosing_declaration: Node,
    ) -> Vec<SymbolId> {
        if enclosing_declaration.is_nil() {
            return Vec::new();
        }
        let containing_file = get_source_file_of_node(enclosing_declaration);
        // PORT: Go keys `extendedContainersByFile` by the node id of the file.
        // The Rust link keys by the file `Node` handle, which is the same identity.
        let id = containing_file;
        // PORT: Go stores a nil slice under a key as "no entry" (`existing != nil`).
        // An empty Vec here plays the same role.
        if let Some(existing) = self
            .symbol_container_links
            .get(symbol)
            .extended_containers_by_file
            .get(&id)
        {
            if !existing.is_empty() {
                return existing.clone();
            }
        }
        let mut results: Vec<SymbolId> = Vec::new();
        let imports = source_file_info(containing_file).imports.clone();
        // PERF: not in Go. Go runs the import loop again on each call whose
        // earlier loop found nothing (it keeps only a non-empty result). For
        // the same symbol and enclosing declaration the loop reads only
        // cached answers (module resolution, module exports, alias targets),
        // so it finds nothing again and has no effect: skip it. Only parsed
        // enclosing declarations are kept: a synthetic node id is not stable.
        let import_miss_key = (symbol, enclosing_declaration);
        let keep_import_miss = !crate::ast::synthetic::is_synthetic_node(enclosing_declaration);
        if !imports.is_empty()
            && !(keep_import_miss
                && self
                    .alternative_module_import_misses
                    .contains(&import_miss_key))
        {
            // Try to make an import using an import already in the enclosing file, if possible
            for import_ref in imports {
                if node_is_synthesized(import_ref) {
                    // Synthetic names can't be resolved by `resolveExternalModuleName` - they'll cause a debug assert if they error
                    continue;
                }
                let resolved_module = {
                    let import_attributes_type =
                        self.get_import_attributes_type_for_module_specifier(import_ref);
                    self.resolve_external_module_name(
                        enclosing_declaration,
                        import_ref, /*ignoreErrors*/
                        true,
                        import_attributes_type,
                    )
                };
                if resolved_module.is_nil() {
                    continue;
                }
                let r#ref = self.get_alias_for_symbol_in_container(resolved_module, symbol);
                if r#ref.is_nil() {
                    continue;
                }
                results.push(resolved_module);
            }
            if !results.is_empty() {
                self.symbol_container_links
                    .get(symbol)
                    .extended_containers_by_file
                    .insert(id, results.clone());
                return results;
            }
            if keep_import_miss {
                self.alternative_module_import_misses
                    .insert(import_miss_key);
            }
        }

        if let Some(existing) = &self.symbol_container_links.get(symbol).extended_containers {
            return existing.clone();
        }
        // No results from files already being imported by this file - expand search (not location-specific, so cached)
        // ts#64469, Go N' symbolaccessibility.go:211
        let results = self.get_external_module_containers(symbol);
        self.symbol_container_links.get(symbol).extended_containers = Some(results.clone());
        results
    }

    // Go: checker/symbolaccessibility.go:229 getExternalModuleContainers (ts#64469)
    pub fn get_external_module_containers(&mut self, symbol: SymbolId) -> Vec<SymbolId> {
        if self.external_module_containers.is_none() {
            self.build_external_module_container_index();
        }
        let complete = self
            .external_module_containers
            .as_ref()
            .is_some_and(|index| index.complete);
        if !complete {
            // Re-entered from an alias resolved while building the index; answer this query without it.
            return self.scan_external_module_containers(symbol);
        }
        let target = self.get_resolved_target(symbol);
        let parent = self.get_parent_of_symbol(symbol);
        let index = self
            .external_module_containers
            .as_ref()
            .expect("external module container index");
        let containers = index
            .containers_by_target
            .get(&target)
            .cloned()
            .unwrap_or_default();
        let Some(&parent_order) = index.module_order.get(&parent) else {
            return containers;
        };
        // The parent module contains the symbol even when the symbol is absent from its exports.
        match containers
            .binary_search_by(|container| index.module_order[container].cmp(&parent_order))
        {
            Ok(_) => containers,
            Err(at) => {
                let mut containers = containers;
                containers.insert(at, parent);
                containers
            }
        }
    }

    // Go: checker/symbolaccessibility.go:253 buildExternalModuleContainerIndex (ts#64469)
    pub fn build_external_module_container_index(&mut self) {
        let files = source_files();
        self.external_module_containers = Some(Box::new(ExternalModuleContainerIndex {
            complete: false,
            containers_by_target: FxHashMap::default(),
            module_order: FxHashMap::with_capacity_and_hasher(files.len(), Default::default()),
        }));
        for file in files {
            if !is_external_module(file) {
                continue;
            }
            let container = self.get_symbol_of_declaration(file);
            self.external_module_container_index().add_module(container);
            let exports = self.get_exports_of_symbol(container);
            for exported in self.symbols.values(exports) {
                let target = self.get_resolved_target(exported);
                self.external_module_container_index()
                    .add(target, container);
            }
            let export_equals = self.symbols.get(
                self.sym(container).exports,
                INTERNAL_SYMBOL_NAME_EXPORT_EQUALS,
            );
            if export_equals.is_some() {
                let target = self.get_resolved_target(export_equals);
                self.external_module_container_index()
                    .add(target, container);
            }
        }
        self.external_module_container_index().complete = true;
    }

    // PORT: Go `c.externalModuleContainers` after the build set it.
    fn external_module_container_index(&mut self) -> &mut ExternalModuleContainerIndex {
        self.external_module_containers
            .as_mut()
            .expect("external module container index")
    }

    // Go: checker/symbolaccessibility.go:275 scanExternalModuleContainers (ts#64469)
    // The search that getAlternativeContainingModules made before ts#64469.
    pub fn scan_external_module_containers(&mut self, symbol: SymbolId) -> Vec<SymbolId> {
        let mut containers = Vec::new();
        for file in source_files() {
            if !is_external_module(file) {
                continue;
            }
            let container = self.get_symbol_of_declaration(file);
            if self
                .get_alias_for_symbol_in_container(container, symbol)
                .is_some()
            {
                containers.push(container);
            }
        }
        containers
    }

    // Go: checker/symbolaccessibility.go:226 getVariableDeclarationOfObjectLiteral
    pub fn get_variable_declaration_of_object_literal(
        &mut self,
        symbol: SymbolId,
        meaning: SymbolFlags,
    ) -> SymbolId {
        // If we're trying to reference some object literal in, eg `var a = { x: 1 }`, the symbol for the literal, `__object`, is distinct
        // from the symbol of the declaration it is being assigned to. Since we can use the declaration to refer to the literal, however,
        // we'd like to make that connection here - potentially causing us to paint the declaration's visibility, and therefore the literal.
        if !meaning.intersects(SymbolFlags::VALUE) {
            return SymbolId::NIL;
        }
        let Some(&first_decl) = self.sym(symbol).declarations.first() else {
            return SymbolId::NIL;
        };
        let parent = first_decl.parent();
        if parent.is_nil() {
            return SymbolId::NIL;
        }
        if !is_variable_declaration(parent) {
            return SymbolId::NIL;
        }
        if is_object_literal_expression(first_decl) && first_decl == parent.initializer()
            || is_type_literal_node(first_decl) && first_decl == parent.type_()
        {
            return self.get_symbol_of_declaration(parent);
        }
        SymbolId::NIL
    }

    // Go: checker/symbolaccessibility.go:253 getExternalModuleContainer
    pub fn get_external_module_container(&mut self, declaration: Node) -> SymbolId {
        let node = find_ancestor(declaration, has_external_module_symbol);
        if node.is_nil() {
            return SymbolId::NIL;
        }
        self.get_symbol_of_declaration(node)
    }

    // Go: checker/symbolaccessibility.go:261 getFileSymbolIfFileSymbolExportEqualsContainer
    pub fn get_file_symbol_if_file_symbol_export_equals_container(
        &mut self,
        d: Node,
        container: SymbolId,
    ) -> SymbolId {
        let file_symbol = self.get_external_module_container(d);
        if file_symbol.is_nil() || self.sym(file_symbol).exports.is_nil() {
            return SymbolId::NIL;
        }
        let exported = self.symbols.get(
            self.sym(file_symbol).exports,
            INTERNAL_SYMBOL_NAME_EXPORT_EQUALS,
        );
        if exported.is_nil() {
            return SymbolId::NIL;
        }
        if self
            .get_symbol_if_same_reference(exported, container)
            .is_some()
        {
            return file_symbol;
        }
        SymbolId::NIL
    }

    /// Attempts to find the symbol corresponding to the container a symbol is in - usually this
    /// is just its' `.parent`, but for locals, this value is `undefined`
    // Go: checker/symbolaccessibility.go:280 getContainersOfSymbol
    pub fn get_containers_of_symbol(
        &mut self,
        symbol: SymbolId,
        enclosing_declaration: Node,
        meaning: SymbolFlags,
    ) -> Vec<SymbolId> {
        let container = self.get_parent_of_symbol(symbol);
        // Type parameters end up in the `members` lists but are not externally visible
        if container.is_some()
            && !self
                .sym(symbol)
                .flags
                .intersects(SymbolFlags::TYPE_PARAMETER)
        {
            return self.get_with_alternative_containers(
                container,
                symbol,
                enclosing_declaration,
                meaning,
            );
        }
        let mut candidates: Vec<SymbolId> = Vec::new();
        let decls = self.sym(symbol).declarations.clone();
        for d in decls {
            let d_parent = d.parent();
            if !is_ambient_module(d) && d_parent.is_some() {
                // direct children of a module
                if has_non_global_augmentation_external_module_symbol(d_parent) {
                    let sym = self.get_symbol_of_declaration(d_parent);
                    if sym.is_some() && !candidates.contains(&sym) {
                        candidates.push(sym);
                    }
                    continue;
                }
                // export ='d member of an ambient module
                if is_module_block(d_parent) && d_parent.parent().is_some() && {
                    let s = self.get_symbol_of_declaration(d_parent.parent());
                    self.resolve_external_module_symbol(s, false) == symbol
                } {
                    let sym = self.get_symbol_of_declaration(d_parent.parent());
                    if sym.is_some() && !candidates.contains(&sym) {
                        candidates.push(sym);
                    }
                    continue;
                }
            }
            if is_class_expression(d)
                && is_binary_expression(d_parent)
                && d_parent.operator_token().kind() == SyntaxKind::EqualsToken
                && is_access_expression(d_parent.left())
                && is_entity_name_expression(d_parent.left().expression())
            {
                let left = d_parent.left();
                if is_module_exports_access_expression(left)
                    || is_exports_identifier(left.expression())
                {
                    let sym = self.get_symbol_of_declaration(get_source_file_of_node(d));
                    if sym.is_some() && !candidates.contains(&sym) {
                        candidates.push(sym);
                    }
                    continue;
                }
                self.check_expression_cached(left.expression());
                let sym = self
                    .symbol_node_links
                    .get(left.expression())
                    .resolved_symbol;
                if sym.is_some() && !candidates.contains(&sym) {
                    candidates.push(sym);
                }
                continue;
            }
        }
        if candidates.is_empty() {
            return Vec::new();
        }

        let mut best_containers = Vec::new();
        let mut alternative_containers = Vec::new();
        for container in candidates {
            if self
                .get_alias_for_symbol_in_container(container, symbol)
                .is_nil()
            {
                continue;
            }
            let all_alts = self.get_with_alternative_containers(
                container,
                symbol,
                enclosing_declaration,
                meaning,
            );
            if all_alts.is_empty() {
                continue;
            }
            best_containers.push(all_alts[0]);
            alternative_containers.extend_from_slice(&all_alts[1..]);
        }
        best_containers.extend(alternative_containers);
        best_containers
    }

    // Go: checker/symbolaccessibility.go:342 getAliasForSymbolInContainer
    pub fn get_alias_for_symbol_in_container(
        &mut self,
        container: SymbolId,
        symbol: SymbolId,
    ) -> SymbolId {
        if container == self.get_parent_of_symbol(symbol) {
            // fast path, `symbol` is either already the alias or isn't aliased
            return symbol;
        }
        // Check if container is a thing with an `export=` which points directly at `symbol`, and if so, return
        // the container itself as the alias for the symbol
        let container_exports = self.sym(container).exports;
        if container_exports.is_some() {
            let export_equals = self
                .symbols
                .get(container_exports, INTERNAL_SYMBOL_NAME_EXPORT_EQUALS);
            if export_equals.is_some()
                && self
                    .get_symbol_if_same_reference(export_equals, symbol)
                    .is_some()
            {
                return container;
            }
        }
        let exports = self.get_exports_of_symbol(container);
        let symbol_name = self.sym(symbol).name.clone();
        let quick = self.symbols.get_name(exports, &symbol_name);
        if quick.is_some() && self.get_symbol_if_same_reference(quick, symbol).is_some() {
            return quick;
        }
        let mut candidates = Vec::new();
        for exported in self.symbols.values(exports) {
            if self
                .get_symbol_if_same_reference(exported, symbol)
                .is_some()
            {
                candidates.push(exported);
            }
        }
        if !candidates.is_empty() {
            self.sort_symbols(&mut candidates); // _must_ sort exports for stable results - symbol table is randomly iterated
            return candidates[0];
        }
        SymbolId::NIL
    }

    // Go: checker/symbolaccessibility.go:373 getAccessibleSymbolChain
    pub fn get_accessible_symbol_chain(
        &mut self,
        symbol: SymbolId,
        enclosing_declaration: Node,
        meaning: SymbolFlags,
        use_only_external_aliasing: bool,
    ) -> Vec<SymbolId> {
        self.get_accessible_symbol_chain_ex(&AccessibleSymbolChainContext {
            symbol,
            enclosing_declaration,
            meaning,
            use_only_external_aliasing,
            visited_symbol_tables_map: Rc::default(),
        })
    }

    // Go: checker/symbolaccessibility.go:382 GetAccessibleSymbolChain
    pub fn get_accessible_symbol_chain_exported(
        &mut self,
        symbol: SymbolId,
        enclosing_declaration: Node,
        meaning: SymbolFlags,
        use_only_external_aliasing: bool,
    ) -> Vec<SymbolId> {
        self.get_accessible_symbol_chain(
            symbol,
            enclosing_declaration,
            meaning,
            use_only_external_aliasing,
        )
    }

    // Go: checker/symbolaccessibility.go:441 getAccessibleSymbolChainEx
    pub(crate) fn get_accessible_symbol_chain_ex(
        &mut self,
        ctx: &AccessibleSymbolChainContext,
    ) -> Vec<SymbolId> {
        if ctx.symbol.is_nil() {
            return Vec::new();
        }
        if is_property_or_method_declaration_symbol(&self.symbols, ctx.symbol) {
            return Vec::new();
        }
        // Go from enclosingDeclaration to the first scope we check, so the cache is keyed off the scope and thus shared more
        let mut first_relevant_location = Node::NIL;
        self.some_symbol_table_in_scope(ctx.enclosing_declaration, &mut |_, _, _, _, _, node| {
            first_relevant_location = node;
            true
        });
        let link_key = AccessibleChainCacheKey {
            use_only_external_aliasing: ctx.use_only_external_aliasing,
            location: first_relevant_location,
            meaning: ctx.meaning,
        };
        if let Some(existing) = self
            .symbol_container_links
            .get(ctx.symbol)
            .accessible_chain_cache
            .get(&link_key)
        {
            return existing.clone();
        }

        let mut result = Vec::new();
        self.some_symbol_table_in_scope(
            ctx.enclosing_declaration,
            &mut |c, t, table_id, ignore_qualification, is_local_name_lookup, _| {
                let res = c.get_accessible_symbol_chain_from_symbol_table(
                    ctx,
                    t,
                    table_id,
                    ignore_qualification,
                    is_local_name_lookup,
                );
                if !res.is_empty() {
                    result = res;
                    return true;
                }
                false
            },
        );
        self.symbol_container_links
            .get(ctx.symbol)
            .accessible_chain_cache
            .insert(link_key, result.clone());
        result
    }

    /// `ignore_qualification` is set when a symbol is being looked for through the exports of another symbol (meaning we have a route to qualify it already)
    // Go: checker/symbolaccessibility.go:481 getAccessibleSymbolChainFromSymbolTable
    pub(crate) fn get_accessible_symbol_chain_from_symbol_table(
        &mut self,
        ctx: &AccessibleSymbolChainContext,
        t: SymbolTable,
        table_id: SymbolTableID,
        ignore_qualification: bool,
        is_local_name_lookup: bool,
    ) -> Vec<SymbolId> {
        // PORT: Go keys the visited map by `ast.GetSymbolId`; the `SymbolId`
        // handle has the same identity. The Go call gives the symbol its id,
        // so it is made too (`ValueSymbolLinkStore`).
        get_symbol_id(&self.symbols, ctx.symbol);
        {
            let mut visited = ctx.visited_symbol_tables_map.borrow_mut();
            let visited_symbol_tables = visited.entry(ctx.symbol).or_default();
            if visited_symbol_tables.contains(&table_id) {
                return Vec::new();
            }
            visited_symbol_tables.insert(table_id);
        }

        let res =
            self.try_symbol_table(ctx, t, table_id, ignore_qualification, is_local_name_lookup);

        if let Some(visited_symbol_tables) = ctx
            .visited_symbol_tables_map
            .borrow_mut()
            .get_mut(&ctx.symbol)
        {
            visited_symbol_tables.remove(&table_id);
        }
        res
    }

    /// Returns only the alias symbols from a symbol table, caching the result by
    /// table id to avoid repeated iteration over large tables.
    // Go: checker/symbolaccessibility.go:505 getSymbolTableAliases
    pub(crate) fn get_symbol_table_aliases(
        &mut self,
        symbols: SymbolTable,
        table_id: SymbolTableID,
    ) -> Vec<SymbolId> {
        let kind = table_id.kind();
        // Members tables never contain alias symbols; skip entirely.
        if kind == SymbolTableID::KIND_MEMBERS {
            return Vec::new();
        }
        let cached_kind = kind == SymbolTableID::KIND_GLOBALS
            || kind == SymbolTableID::KIND_EXPORTS
            || kind == SymbolTableID::KIND_RESOLVED_EXPORTS;
        // Cache globals and exports tables (which are large and revisited often).
        // Locals tables are small and per-scope, so they are filtered but not cached.
        if cached_kind {
            if let Some(aliases) = self.symbol_table_alias_cache.get(&table_id) {
                return aliases.clone();
            }
        }
        let mut aliases = Vec::new();
        for sym in self.symbols.values(symbols) {
            if self.sym(sym).flags.intersects(SymbolFlags::ALIAS) {
                aliases.push(sym);
            }
        }
        if cached_kind {
            self.symbol_table_alias_cache
                .insert(table_id, aliases.clone());
        }
        aliases
    }

    // Go: checker/symbolaccessibility.go:535 trySymbolTable
    pub(crate) fn try_symbol_table(
        &mut self,
        ctx: &AccessibleSymbolChainContext,
        symbols: SymbolTable,
        table_id: SymbolTableID,
        ignore_qualification: bool,
        is_local_name_lookup: bool,
    ) -> Vec<SymbolId> {
        let is_globals = table_id == SymbolTableID::KIND_GLOBALS;
        // If symbol is directly available by its name in the symbol table
        let name = self.sym(ctx.symbol).name.clone();
        let res = self.symbols.get_name(symbols, &name);
        if res.is_some() && self.is_accessible(ctx, res, SymbolId::NIL, ignore_qualification) {
            return vec![ctx.symbol];
        }

        let mut candidate_chains: Vec<Vec<SymbolId>> = Vec::new();

        // Check for ExportSymbol by direct name lookup rather than discovering it during
        // the alias iteration below (where it would never match, since only alias-flagged
        // symbols are iterated).
        if res.is_some() && self.sym(res).export_symbol.is_some() {
            let merged = self.get_merged_symbol(self.sym(res).export_symbol);
            if self.is_accessible(ctx, merged, SymbolId::NIL, ignore_qualification) {
                candidate_chains.push(vec![ctx.symbol]);
            }
        }

        // Iterate only alias symbols from the table (cached per tableId).
        // This avoids iterating thousands of non-alias symbols in large tables like globals.
        for symbol_from_symbol_table in self.get_symbol_table_aliases(symbols, table_id) {
            let s_name = self.sym(symbol_from_symbol_table).name.clone();
            let s_name = s_name.as_str();
            let s_decls = self.sym(symbol_from_symbol_table).declarations.clone();
            // for every non-default, non-export= alias symbol in scope, check if it refers to or can chain to the target symbol
            if s_name != INTERNAL_SYMBOL_NAME_EXPORT_EQUALS
                && s_name != INTERNAL_SYMBOL_NAME_DEFAULT
                && !(is_umd_export_symbol(&self.symbols, symbol_from_symbol_table)
                    && ctx.enclosing_declaration.is_some()
                    && is_external_module(get_source_file_of_node(ctx.enclosing_declaration)))
                // If `!useOnlyExternalAliasing`, we can use any type of alias to get the name
                && (!ctx.use_only_external_aliasing || s_decls.iter().any(|&d| is_external_module_import_equals_declaration(d)))
                // If we're looking up a local name to reference directly, omit namespace reexports, otherwise when we're trawling through an export list to make a dotted name, we can keep it
                && (is_local_name_lookup && !s_decls.iter().any(|&d| is_namespace_reexport_declaration(d)) || !is_local_name_lookup)
                // While exports are generally considered to be in scope, export-specifier declared symbols are _not_
                // See similar comment in `resolveName` for details
                && (ignore_qualification
                    || self.get_declarations_of_kind(symbol_from_symbol_table, SyntaxKind::ExportSpecifier).is_empty())
            {
                let resolved_imported_symbol = self.resolve_alias(symbol_from_symbol_table);
                let candidate = self.get_candidate_list_for_symbol(
                    ctx,
                    symbol_from_symbol_table,
                    resolved_imported_symbol,
                    ignore_qualification,
                );
                if !candidate.is_empty() {
                    candidate_chains.push(candidate);
                }
            }
        }

        if !candidate_chains.is_empty() {
            // pick first, shortest
            // Go: checker/symbolaccessibility.go:584 slices.SortStableFunc(candidateChains, c.compareSymbolChains)
            // PORT: compareSymbolChains calls compareSymbols, which is not a
            // total order, so this uses Go's stable sort to get Go's order.
            crate::gostd::slices::sort_stable_func(&mut candidate_chains, |a, b| {
                self.compare_symbol_chains(a, b)
            });
            return candidate_chains.swap_remove(0);
        }

        // If there's no result and we're looking at the global symbol table, treat `globalThis` like an alias and try to lookup thru that
        if is_globals {
            let global_this = self.global_this_symbol;
            return self.get_candidate_list_for_symbol(
                ctx,
                global_this,
                global_this,
                ignore_qualification,
            );
        }
        Vec::new()
    }

    // PORT: Go checker/symbolaccessibility.go:618 compareSymbolChainsWorker is
    // ported in checker/extras.rs.

    // Go: checker/symbolaccessibility.go:620 getCandidateListForSymbol
    pub(crate) fn get_candidate_list_for_symbol(
        &mut self,
        ctx: &AccessibleSymbolChainContext,
        symbol_from_symbol_table: SymbolId,
        resolved_imported_symbol: SymbolId,
        ignore_qualification: bool,
    ) -> Vec<SymbolId> {
        if self.is_accessible(
            ctx,
            symbol_from_symbol_table,
            resolved_imported_symbol,
            ignore_qualification,
        ) {
            return vec![symbol_from_symbol_table];
        }

        // Look in the exported members, if we can find accessibleSymbolChain, symbol is accessible using this chain
        // but only if the symbolFromSymbolTable can be qualified
        let candidate_table = self.get_exports_of_symbol(resolved_imported_symbol);
        if candidate_table.is_nil() {
            return Vec::new();
        }
        let candidate_table_id =
            SymbolTableID::from_resolved_exports(&self.symbols, resolved_imported_symbol);
        let accessible_symbols_from_exports = self.get_accessible_symbol_chain_from_symbol_table(
            ctx,
            candidate_table,
            candidate_table_id,
            true,
            false,
        );
        if accessible_symbols_from_exports.is_empty() {
            return Vec::new();
        }
        if !self.can_qualify_symbol(
            ctx,
            symbol_from_symbol_table,
            get_qualified_left_meaning(ctx.meaning),
        ) {
            return Vec::new();
        }
        let mut result = vec![symbol_from_symbol_table];
        result.extend(accessible_symbols_from_exports);
        result
    }

    // Go: checker/symbolaccessibility.go:647 isAccessible
    pub(crate) fn is_accessible(
        &mut self,
        ctx: &AccessibleSymbolChainContext,
        symbol_from_symbol_table: SymbolId,
        resolved_alias_symbol: SymbolId,
        ignore_qualification: bool,
    ) -> bool {
        let mut like_symbols = false;
        if ctx.symbol == resolved_alias_symbol {
            like_symbols = true;
        }
        if ctx.symbol == symbol_from_symbol_table {
            like_symbols = true;
        }
        let symbol = self.get_merged_symbol(ctx.symbol);
        if symbol == self.get_merged_symbol(resolved_alias_symbol) {
            like_symbols = true;
        }
        if symbol == self.get_merged_symbol(symbol_from_symbol_table) {
            like_symbols = true;
        }
        // ts#64573 (Go N' symbolaccessibility.go:729): follow an alias chain
        // (an `export =` of a class with a top-level `export type`).
        if !like_symbols
            && resolved_alias_symbol.is_some()
            && self
                .sym(resolved_alias_symbol)
                .flags
                .intersects(SymbolFlags::ALIAS)
        {
            let mut resolved_alias_symbol = resolved_alias_symbol;
            let mut seen_aliases: FxHashSet<SymbolId> = FxHashSet::default();
            // PORT: Go `resolveAlias` never returns nil; the `is_some` test
            // only guards the read.
            while resolved_alias_symbol.is_some()
                && self
                    .sym(resolved_alias_symbol)
                    .flags
                    .intersects(SymbolFlags::ALIAS)
                && seen_aliases.insert(resolved_alias_symbol)
            {
                let target = self.resolve_alias(resolved_alias_symbol);
                resolved_alias_symbol = self.get_merged_symbol(target);
                if symbol == resolved_alias_symbol {
                    like_symbols = true;
                    break;
                }
            }
        }
        if !like_symbols {
            return false;
        }
        // if the symbolFromSymbolTable is not external module (it could be if it was determined as ambient external module and would be in globals table)
        // and if symbolFromSymbolTable or alias resolution matches the symbol,
        // check the symbol can be qualified, it is only then this symbol is accessible
        let decls = self.sym(symbol_from_symbol_table).declarations.clone();
        !decls
            .iter()
            .any(|&d| has_non_global_augmentation_external_module_symbol(d))
            && (ignore_qualification || {
                let merged = self.get_merged_symbol(symbol_from_symbol_table);
                self.can_qualify_symbol(ctx, merged, ctx.meaning)
            })
    }

    // Go: checker/symbolaccessibility.go:677 canQualifySymbol
    pub(crate) fn can_qualify_symbol(
        &mut self,
        ctx: &AccessibleSymbolChainContext,
        symbol_from_symbol_table: SymbolId,
        meaning: SymbolFlags,
    ) -> bool {
        let parent = self.sym(symbol_from_symbol_table).parent;
        // If the symbol is equivalent and doesn't need further qualification, this symbol is accessible
        !self.needs_qualification(symbol_from_symbol_table, ctx.enclosing_declaration, meaning)
            // If symbol needs qualification, make sure that parent is accessible, if it is then this symbol is accessible too
            || !self
                .get_accessible_symbol_chain_ex(&AccessibleSymbolChainContext {
                    symbol: parent,
                    enclosing_declaration: ctx.enclosing_declaration,
                    meaning: get_qualified_left_meaning(meaning),
                    use_only_external_aliasing: ctx.use_only_external_aliasing,
                    visited_symbol_tables_map: ctx.visited_symbol_tables_map.clone(),
                })
                .is_empty()
    }

    // Go: checker/symbolaccessibility.go:688 needsQualification
    pub fn needs_qualification(
        &mut self,
        symbol: SymbolId,
        enclosing_declaration: Node,
        meaning: SymbolFlags,
    ) -> bool {
        let mut qualify = false;
        let name = self.sym(symbol).name.clone();
        self.some_symbol_table_in_scope(
            enclosing_declaration,
            &mut |c, symbol_table, _, _, _, _| {
                // If symbol of this name is not available in the symbol table we are ok
                let res = c.symbols.get_name(symbol_table, &name);
                if res.is_nil() {
                    return false;
                }
                let mut symbol_from_symbol_table = c.get_merged_symbol(res);
                if symbol_from_symbol_table.is_nil() {
                    // Continue to the next symbol table
                    return false;
                }
                // If the symbol with this name is present it should refer to the symbol
                if symbol_from_symbol_table == symbol {
                    // No need to qualify
                    return true;
                }

                // Qualify if the symbol from symbol table has same meaning as expected
                let should_resolve_alias = c
                    .sym(symbol_from_symbol_table)
                    .flags
                    .intersects(SymbolFlags::ALIAS)
                    && get_declaration_of_kind(
                        &c.symbols,
                        symbol_from_symbol_table,
                        SyntaxKind::ExportSpecifier,
                    )
                    .is_nil();
                if should_resolve_alias {
                    symbol_from_symbol_table = c.resolve_alias(symbol_from_symbol_table);
                }
                let mut flags = c.sym(symbol_from_symbol_table).flags;
                if should_resolve_alias {
                    flags = c.get_symbol_flags(symbol_from_symbol_table);
                }
                if flags.intersects(meaning) {
                    qualify = true;
                    return true;
                }

                // Continue to the next symbol table
                false
            },
        );

        qualify
    }

    // Go: checker/symbolaccessibility.go:746 someSymbolTableInScope
    // PORT: the Go callback is a closure over `c`; here it takes the checker
    // as its first argument.
    pub(crate) fn some_symbol_table_in_scope(
        &mut self,
        enclosing_declaration: Node,
        callback: &mut dyn FnMut(
            &mut Checker,
            SymbolTable,
            SymbolTableID,
            bool,
            bool,
            Node,
        ) -> bool,
    ) -> bool {
        let mut location = enclosing_declaration;
        while location.is_some() {
            // Locals of a source file are not in scope (because they get merged into the global symbol table)
            if can_have_locals(location)
                && location.locals().is_some()
                && !is_global_source_file(location)
            {
                if callback(
                    self,
                    location.locals(),
                    SymbolTableID::from_locals(location),
                    false,
                    true,
                    location,
                ) {
                    return true;
                }
            }
            match location.kind() {
                SyntaxKind::SourceFile | SyntaxKind::ModuleDeclaration => {
                    if !(is_source_file(location) && !is_external_or_common_js_module(location)) {
                        let sym =
                            self.get_symbol_of_declaration(get_reparsed_node_for_node(location));
                        let exports = self.sym(sym).exports;
                        if callback(
                            self,
                            exports,
                            SymbolTableID::from_exports(&self.symbols, sym),
                            false,
                            true,
                            location,
                        ) {
                            return true;
                        }
                    }
                }
                SyntaxKind::ClassDeclaration
                | SyntaxKind::ClassExpression
                | SyntaxKind::InterfaceDeclaration => {
                    // Type parameters are bound into `members` lists so they can merge across declarations
                    // This is troublesome, since in all other respects, they behave like locals :cries:
                    // These can never be latebound, so the symbol's raw members are sufficient. `getMembersOfNode` cannot be used, as it would
                    // trigger resolving late-bound names, which we may already be in the process of doing while we're here!
                    let mut table = SymbolTable::NIL;
                    let sym = self.get_symbol_of_declaration(location);
                    // PORT: Go builds a fresh map on each call. Here each call adds a
                    // fresh table to the symbol arena.
                    for (key, member_symbol) in self.symbols.entries(self.sym(sym).members) {
                        if self
                            .sym(member_symbol)
                            .flags
                            .intersects(SymbolFlags::TYPE.without(SymbolFlags::ASSIGNMENT))
                        {
                            if table.is_nil() {
                                table = self.symbols.new_table();
                            }
                            self.symbols.set(table, key, member_symbol);
                        }
                    }
                    if table.is_some()
                        && callback(
                            self,
                            table,
                            SymbolTableID::from_members(&self.symbols, sym),
                            false,
                            false,
                            location,
                        )
                    {
                        return true;
                    }
                    // Class expression names (e.g., `B` in `class B {}`) are not stored in any
                    // scope table — the binder uses bindAnonymousDeclaration. Expose the name
                    // binding here so getAccessibleSymbolChain can resolve self-references.
                    if is_class_expression(location) && location.name().is_some() {
                        let name_table = self.get_class_expression_name_table(location);
                        if name_table.is_some()
                            && callback(
                                self,
                                name_table,
                                SymbolTableID::from_locals(location),
                                false,
                                true,
                                location,
                            )
                        {
                            return true;
                        }
                    }
                }
                _ => {}
            }
            location = location.parent();
        }

        let globals = self.globals;
        callback(
            self,
            globals,
            SymbolTableID::from_globals(),
            false,
            true,
            Node::NIL,
        )
    }

    /// Returns a cached symbol table containing the class expression's name
    /// binding. Class expression names are bound via bindAnonymousDeclaration and
    /// aren't stored in any container's locals, so this synthesized table lets
    /// some_symbol_table_in_scope expose them during accessibility checks.
    // Go: checker/symbolaccessibility.go:810 getClassExpressionNameTable
    pub(crate) fn get_class_expression_name_table(&mut self, location: Node) -> SymbolTable {
        // PORT: Go keys by `ast.GetNodeId(location)`; the `Node` handle has the same identity.
        if let Some(&table) = self.class_expression_name_tables.get(&location) {
            return table;
        }
        let class_symbol = self.get_symbol_of_declaration(location);
        let name_text = location.name().text();
        if name_text.is_empty() || class_symbol.is_nil() {
            return SymbolTable::NIL;
        }
        let table = self.symbols.new_table();
        self.symbols.set(table, name_text, class_symbol);
        self.class_expression_name_tables.insert(location, table);
        table
    }

    /// Check if the given symbol in given enclosing declaration is accessible and mark all associated alias to be visible if requested
    // Go: checker/symbolaccessibility.go:839 IsSymbolAccessible
    pub fn is_symbol_accessible(
        &mut self,
        symbol: SymbolId,
        enclosing_declaration: Node,
        meaning: SymbolFlags,
        should_compute_aliases_to_make_visible: bool,
    ) -> SymbolAccessibilityResult {
        self.is_symbol_accessible_worker(
            symbol,
            enclosing_declaration,
            meaning,
            should_compute_aliases_to_make_visible,
            true,
        )
    }

    // Go: checker/symbolaccessibility.go:843 isSymbolAccessibleWorker
    pub fn is_symbol_accessible_worker(
        &mut self,
        symbol: SymbolId,
        enclosing_declaration: Node,
        meaning: SymbolFlags,
        should_compute_aliases_to_make_visible: bool,
        allow_modules: bool,
    ) -> SymbolAccessibilityResult {
        if symbol.is_some() && enclosing_declaration.is_some() {
            let result = self.is_any_symbol_accessible(
                &[symbol],
                enclosing_declaration,
                symbol,
                meaning,
                should_compute_aliases_to_make_visible,
                allow_modules,
            );
            if let Some(result) = result {
                return result;
            }

            // This could be a symbol that is not exported in the external module
            // or it could be a symbol from different external module that is not aliased and hence cannot be named
            let mut symbol_external_module = SymbolId::NIL;
            for d in self.sym(symbol).declarations.clone() {
                symbol_external_module = self.get_external_module_container(d);
                if symbol_external_module.is_some() {
                    break;
                }
            }
            if symbol_external_module.is_some() {
                let enclosing_external_module =
                    self.get_external_module_container(enclosing_declaration);
                if symbol_external_module != enclosing_external_module {
                    // name from different external module that is not visible
                    let error_symbol_name = self.symbol_to_string_ex(
                        symbol,
                        enclosing_declaration,
                        meaning,
                        SymbolFormatFlags::ALLOW_ANY_NODE_KIND,
                    );
                    let error_module_name = self.symbol_to_string(symbol_external_module);
                    return SymbolAccessibilityResult {
                        accessibility: SymbolAccessibility::CANNOT_BE_NAMED,
                        error_symbol_name,
                        error_module_name,
                        error_node: if is_in_js_file(enclosing_declaration) {
                            enclosing_declaration
                        } else {
                            Node::NIL
                        },
                        ..Default::default()
                    };
                }
            }

            // Just a local name that is not accessible
            let error_symbol_name = self.symbol_to_string_ex(
                symbol,
                enclosing_declaration,
                meaning,
                SymbolFormatFlags::ALLOW_ANY_NODE_KIND,
            );
            return SymbolAccessibilityResult {
                accessibility: SymbolAccessibility::NOT_ACCESSIBLE,
                error_symbol_name,
                ..Default::default()
            };
        }

        SymbolAccessibilityResult {
            accessibility: SymbolAccessibility::ACCESSIBLE,
            ..Default::default()
        }
    }
}

// Go: checker/symbolaccessibility.go:105 hasNonGlobalAugmentationExternalModuleSymbol
pub fn has_non_global_augmentation_external_module_symbol(declaration: Node) -> bool {
    is_module_with_string_literal_name(declaration)
        || (declaration.kind() == SyntaxKind::SourceFile
            && is_external_or_common_js_module(declaration))
}

// Go: checker/symbolaccessibility.go:109 getQualifiedLeftMeaning
pub fn get_qualified_left_meaning(right_meaning: SymbolFlags) -> SymbolFlags {
    // If we are looking in value space, the parent meaning is value, other wise it is namespace
    if right_meaning == SymbolFlags::VALUE {
        return SymbolFlags::VALUE;
    }
    SymbolFlags::NAMESPACE
}

// Go: checker/symbolaccessibility.go:249 hasExternalModuleSymbol
pub fn has_external_module_symbol(declaration: Node) -> bool {
    is_ambient_module(declaration)
        || (declaration.kind() == SyntaxKind::SourceFile
            && is_external_or_common_js_module(declaration))
}

// Go: checker/symbolaccessibility.go:391 accessibleSymbolChainContext
// PORT: Go shares `visitedSymbolTablesMap` by map reference between contexts.
// Here the map is an `Rc<RefCell<..>>` so clones share it the same way.
#[derive(Clone)]
pub(crate) struct AccessibleSymbolChainContext {
    pub symbol: SymbolId,
    pub enclosing_declaration: Node,
    pub meaning: SymbolFlags,
    pub use_only_external_aliasing: bool,
    pub visited_symbol_tables_map: Rc<RefCell<FxHashMap<SymbolId, FxHashSet<SymbolTableID>>>>,
}

// Go: checker/symbolaccessibility.go:402 symbolTableID constants
// The high 3 bits encode the kind, and the remaining bits encode the
// NodeId or SymbolId of the source.
impl SymbolTableID {
    const KIND_SHIFT: u32 = 61;
    pub const KIND_LOCALS: Self = Self(0 << Self::KIND_SHIFT);
    pub const KIND_EXPORTS: Self = Self(1 << Self::KIND_SHIFT);
    pub const KIND_MEMBERS: Self = Self(2 << Self::KIND_SHIFT);
    pub const KIND_GLOBALS: Self = Self(3 << Self::KIND_SHIFT);
    /// Resolved/derived exports from getExportsOfSymbol, distinct from raw sym.Exports.
    pub const KIND_RESOLVED_EXPORTS: Self = Self(4 << Self::KIND_SHIFT);
    /// Go `stKindMask = (iota - 1) << stKindShift`, with iota = 5.
    pub const KIND_MASK: Self = Self(4 << Self::KIND_SHIFT);

    /// Go `tableId & stKindMask`.
    #[must_use]
    pub fn kind(self) -> Self {
        Self(self.0 & Self::KIND_MASK.0)
    }

    // Go: checker/symbolaccessibility.go:417 symbolTableIDFromLocals
    #[must_use]
    pub fn from_locals(node: Node) -> Self {
        Self(Self::KIND_LOCALS.0 | get_node_id(node))
    }

    // Go: checker/symbolaccessibility.go:421 symbolTableIDFromExports
    #[must_use]
    pub fn from_exports(symbols: &SymbolArena, sym: SymbolId) -> Self {
        Self(Self::KIND_EXPORTS.0 | get_symbol_id(symbols, sym))
    }

    // Go: checker/symbolaccessibility.go:429 symbolTableIDFromResolvedExports
    #[must_use]
    pub fn from_resolved_exports(symbols: &SymbolArena, sym: SymbolId) -> Self {
        Self(Self::KIND_RESOLVED_EXPORTS.0 | get_symbol_id(symbols, sym))
    }

    // Go: checker/symbolaccessibility.go:433 symbolTableIDFromMembers
    #[must_use]
    pub fn from_members(symbols: &SymbolArena, sym: SymbolId) -> Self {
        Self(Self::KIND_MEMBERS.0 | get_symbol_id(symbols, sym))
    }

    // Go: checker/symbolaccessibility.go:437 symbolTableIDFromGlobals
    #[must_use]
    pub fn from_globals() -> Self {
        Self::KIND_GLOBALS
    }
}

// Go: checker/symbolaccessibility.go:612 isUMDExportSymbol
pub fn is_umd_export_symbol(symbols: &SymbolArena, symbol: SymbolId) -> bool {
    symbol.is_some()
        && symbols
            .sym(symbol)
            .declarations
            .first()
            .is_some_and(|&d| d.is_some() && is_namespace_export_declaration(d))
}

// Go: checker/symbolaccessibility.go:616 isNamespaceReexportDeclaration
pub fn is_namespace_reexport_declaration(node: Node) -> bool {
    is_namespace_export(node) && node.parent().module_specifier().is_some()
}

// Go: checker/symbolaccessibility.go:728 isPropertyOrMethodDeclarationSymbol
pub fn is_property_or_method_declaration_symbol(symbols: &SymbolArena, symbol: SymbolId) -> bool {
    let decls = &symbols.sym(symbol).declarations;
    if !decls.is_empty() {
        for declaration in decls {
            match declaration.kind() {
                SyntaxKind::PropertyDeclaration
                | SyntaxKind::MethodDeclaration
                | SyntaxKind::GetAccessor
                | SyntaxKind::SetAccessor => continue,
                _ => return false,
            }
        }
        return true;
    }
    false
}

// Go: checker/symbolaccessibility.go:216 externalModuleContainerIndex (ts#64469)
// The external modules that export each resolved symbol, in program order.
pub struct ExternalModuleContainerIndex {
    pub complete: bool,
    pub containers_by_target: FxHashMap<SymbolId, Vec<SymbolId>>,
    pub module_order: FxHashMap<SymbolId, usize>,
}

impl ExternalModuleContainerIndex {
    // Go: checker/symbolaccessibility.go:222 externalModuleContainerIndex.add (ts#64469)
    fn add(&mut self, target: SymbolId, container: SymbolId) {
        // Modules are indexed one at a time, so a repeat of this container is always the last entry.
        let existing = self.containers_by_target.entry(target).or_default();
        if existing.last() != Some(&container) {
            existing.push(container);
        }
    }

    // PORT: Go `index.moduleOrder[container] = len(index.moduleOrder)` in
    // buildExternalModuleContainerIndex.
    fn add_module(&mut self, container: SymbolId) {
        let order = self.module_order.len();
        self.module_order.insert(container, order);
    }
}
