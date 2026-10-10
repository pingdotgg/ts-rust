use crate::prelude::*;
use std::sync::LazyLock;

// Port of checker/services.go (all except GetConstantValue at services.go:859,
// which is in emit_resolver_p2.rs).
//
// PORT: names follow `checker-api-tools/names_exports_services.tsv` column 5.
// An exported Go method whose unexported twin has the same snake name ends
// in `_exported`.
// PORT: Go package-level functions that read checker data
// (`runWithInferenceBlockedFromSourceNode`, `GetResolvedSignatureForSignatureHelp`,
// `runWithoutResolvedSignatureCaching`) are Checker methods. The others
// (`isExportSpecifierAlias`, `getPossibleSymbolReferenceNodes`,
// `getPossibleSymbolReferencePositions`, `isKnownGenericTypeName`) are free
// functions.
// PORT: a Go `[]*T` result is a `Vec<T>`.

impl Checker {
    // Go: checker/services.go:17 GetSymbolsInScope
    pub fn get_symbols_in_scope_exported(
        &mut self,
        location: Node,
        meaning: SymbolFlags,
    ) -> Vec<SymbolId> {
        self.get_symbols_in_scope(location, meaning)
    }

    // Go: checker/services.go:21 getSymbolsInScope
    // PORT: Go ranges over Go maps (`for _, symbol := range source`), so the
    // Go result order is random. The symbol tables keep insertion order, so
    // this order is stable. Compare results with Go only after sorting.
    pub fn get_symbols_in_scope(&mut self, location: Node, meaning: SymbolFlags) -> Vec<SymbolId> {
        if location.flags().intersects(NodeFlags::IN_WITH_STATEMENT) {
            // We cannot answer semantic questions within a with block, do not proceed any further
            return Vec::new();
        }

        let symbols = self.symbols.new_table();
        let mut is_static_symbol = false;

        // Copy the given symbol into symbol tables if the symbol has the given meaning
        // and it doesn't already exists in the symbol table.
        let copy_symbol = |c: &mut Checker, symbol: SymbolId, meaning: SymbolFlags| {
            if c.sym(symbol)
                .combined_local_and_export_symbol_flags(&c.symbols)
                .intersects(meaning)
            {
                let id = c.sym(symbol).name.clone();
                // We will copy all symbol regardless of its reserved name because
                // symbolsToArray will check whether the key is a reserved name and
                // it will not copy symbol with reserved name to the array
                if c.symbols.get_name(symbols, &id).is_nil() {
                    c.symbols.set(symbols, id, symbol);
                }
            }
        };

        let copy_symbols = |c: &mut Checker, source: SymbolTable, meaning: SymbolFlags| {
            if !meaning.is_empty() {
                for symbol in c.symbols.values(source) {
                    copy_symbol(c, symbol, meaning);
                }
            }
        };

        let copy_locally_visible_export_symbols =
            |c: &mut Checker, source: SymbolTable, meaning: SymbolFlags| {
                if !meaning.is_empty() {
                    for symbol in c.symbols.values(source) {
                        // Similar condition as in `resolveNameHelper`
                        if get_declaration_of_kind(&c.symbols, symbol, SyntaxKind::ExportSpecifier)
                            .is_nil()
                            && get_declaration_of_kind(
                                &c.symbols,
                                symbol,
                                SyntaxKind::NamespaceExport,
                            )
                            .is_nil()
                            && c.sym(symbol).name != INTERNAL_SYMBOL_NAME_DEFAULT
                        {
                            copy_symbol(c, symbol, meaning);
                        }
                    }
                }
            };

        let mut location = location;
        let mut populate_symbols = |c: &mut Checker| {
            let mut last_location = Node::NIL;
            while location.is_some() {
                if is_module_declaration(location)
                    && location.attributes().is_some()
                    && last_location == location.attributes()
                {
                    // Module declaration is not in scope inside its attributes.
                    last_location = location;
                    location = location.parent();
                    continue;
                }

                if can_have_locals(location)
                    && location.locals().is_some()
                    && !is_global_source_file(location)
                {
                    copy_symbols(c, location.locals(), meaning);
                }

                match location.kind() {
                    SyntaxKind::SourceFile | SyntaxKind::ModuleDeclaration => {
                        // Go: `case KindSourceFile`: break unless the file is an
                        // external module, else fall through to KindModuleDeclaration.
                        if location.kind() == SyntaxKind::ModuleDeclaration
                            || is_external_module(location)
                        {
                            let symbol = c.get_symbol_of_declaration(location);
                            let exports = c.sym(symbol).exports;
                            copy_locally_visible_export_symbols(
                                c,
                                exports,
                                meaning & SymbolFlags::MODULE_MEMBER,
                            );
                        }
                    }
                    SyntaxKind::EnumDeclaration => {
                        let symbol = c.get_symbol_of_declaration(location);
                        let exports = c.sym(symbol).exports;
                        copy_symbols(c, exports, meaning & SymbolFlags::ENUM_MEMBER);
                    }
                    SyntaxKind::ClassExpression
                    | SyntaxKind::ClassDeclaration
                    | SyntaxKind::InterfaceDeclaration => {
                        if location.kind() == SyntaxKind::ClassExpression {
                            let class_name = location.name();
                            if class_name.is_some() {
                                copy_symbol(c, location.symbol(), meaning);
                            }
                            // this fall-through is necessary because we would like to handle
                            // type parameter inside class expression similar to how we handle it in classDeclaration and interface Declaration.
                        }
                        // If we didn't come from static member of class or interface,
                        // add the type parameters into the symbol table
                        // (type parameters of classDeclaration/classExpression and interface are in member property of the symbol.
                        // Note: that the memberFlags come from previous iteration.
                        if !is_static_symbol {
                            let symbol = c.get_symbol_of_declaration(location);
                            let members = c.get_members_of_symbol(symbol);
                            copy_symbols(c, members, meaning & SymbolFlags::TYPE);
                        }
                    }
                    SyntaxKind::FunctionExpression => {
                        let func_name = location.name();
                        if func_name.is_some() {
                            copy_symbol(c, location.symbol(), meaning);
                        }
                    }
                    _ => {}
                }

                if introduces_arguments_exotic_object(location) {
                    let arguments_symbol = c.arguments_symbol;
                    copy_symbol(c, arguments_symbol, meaning);
                }

                is_static_symbol = is_static(location);
                last_location = location;
                location = location.parent();
            }

            let globals = c.globals;
            copy_symbols(c, globals, meaning);
        };

        populate_symbols(self);

        self.symbols.delete(symbols, INTERNAL_SYMBOL_NAME_THIS); // Not a symbol, a keyword
        self.symbols_to_array(symbols)
    }

    // Go: checker/services.go:132 GetExportsOfModule
    pub fn get_exports_of_module_exported(&mut self, symbol: SymbolId) -> Vec<SymbolId> {
        let exports = self.get_exports_of_module(symbol);
        self.symbols_to_array(exports)
    }

    // Go: checker/services.go:136 ForEachExportAndPropertyOfModule
    // PORT: Go ranges over Go maps; the tables keep insertion order.
    pub fn for_each_export_and_property_of_module(
        &mut self,
        module_symbol: SymbolId,
        cb: &mut dyn FnMut(&mut Checker, SymbolId, &str),
    ) {
        let exports = self.get_exports_of_module(module_symbol);
        for (key, exported_symbol) in self.symbols.entries(exports) {
            if !is_reserved_member_name(&key) {
                cb(self, exported_symbol, &key);
            }
        }

        let export_equals =
            self.resolve_external_module_symbol(module_symbol, false /*dontResolveAlias*/);
        if export_equals == module_symbol {
            return;
        }

        let type_of_symbol = self.get_type_of_symbol(export_equals);
        if !self.should_treat_properties_of_external_module_as_exports(type_of_symbol) {
            return;
        }

        // forEachPropertyOfType
        let reduced_type = self.get_reduced_apparent_type(type_of_symbol);
        if !self
            .ty(reduced_type)
            .flags
            .intersects(TypeFlags::STRUCTURED_TYPE)
        {
            return;
        }
        let members = self.resolve_structured_type_members(reduced_type).members;
        for (name, symbol) in self.symbols.entries(members) {
            if self.is_named_member(symbol, &name) {
                cb(self, symbol, &name);
            }
        }
    }

    // Go: checker/services.go:165 IsValidPropertyAccess
    pub fn is_valid_property_access_exported(&mut self, node: Node, property_name: &str) -> bool {
        self.is_valid_property_access(node, property_name)
    }

    // Go: checker/services.go:169 isValidPropertyAccess
    pub fn is_valid_property_access(&mut self, node: Node, property_name: &str) -> bool {
        match node.kind() {
            SyntaxKind::PropertyAccessExpression => {
                let expression = node.expression();
                let expression_type = self.check_expression(expression);
                let t = self.get_widened_type(expression_type);
                return self.is_valid_property_access_with_type(
                    node,
                    expression.kind() == SyntaxKind::SuperKeyword,
                    property_name,
                    t,
                );
            }
            SyntaxKind::QualifiedName => {
                let left_type = self.check_expression(node.left());
                let t = self.get_widened_type(left_type);
                return self.is_valid_property_access_with_type(
                    node,
                    false, /*isSuper*/
                    property_name,
                    t,
                );
            }
            SyntaxKind::ImportType => {
                let t = self.get_type_from_type_node(node);
                return self.is_valid_property_access_with_type(
                    node,
                    false, /*isSuper*/
                    property_name,
                    t,
                );
            }
            _ => {}
        }
        // PORT: Go prints `node.Kind.String()`; the Rust kind name is the Debug text.
        panic!(
            "Unexpected node kind in isValidPropertyAccess: {:?}",
            node.kind()
        );
    }

    // Go: checker/services.go:181 isValidPropertyAccessWithType
    pub fn is_valid_property_access_with_type(
        &mut self,
        node: Node,
        is_super: bool,
        property_name: &str,
        t: TypeId,
    ) -> bool {
        // Short-circuiting for improved performance.
        if self.is_type_any(t) {
            return true;
        }

        let prop = self.get_property_of_type(t, property_name);
        prop.is_some()
            && self.is_property_accessible(node, is_super, false /*isWrite*/, t, prop)
    }

    // Go: checker/services.go:199 IsValidPropertyAccessForCompletions
    // Checks if an existing property access is valid for completions purposes.
    // node: a property access-like node where we want to check if we can access a property.
    // This node does not need to be an access of the property we are checking.
    // e.g. in completions, this node will often be an incomplete property access node, as in `foo.`.
    // Besides providing a location (i.e. scope) used to check property accessibility, we use this node for
    // computing whether this is a `super` property access.
    // type: the type whose property we are checking.
    // property: the accessed property's symbol.
    pub fn is_valid_property_access_for_completions_exported(
        &mut self,
        node: Node,
        t: TypeId,
        property: SymbolId,
    ) -> bool {
        self.is_property_accessible(
            node,
            node.kind() == SyntaxKind::PropertyAccessExpression
                && node.expression().kind() == SyntaxKind::SuperKeyword,
            false, /*isWrite*/
            t,
            property,
        )
        // Previously we validated the 'this' type of methods but this adversely affected performance. See #31377 for more context.
    }

    // Go: checker/services.go:210 GetAllPossiblePropertiesOfTypes
    // PORT: Go returns `maps.Values(props)` (random order). The table keeps
    // insertion order.
    pub fn get_all_possible_properties_of_types(&mut self, types: &[TypeId]) -> Vec<SymbolId> {
        let union_type = self.get_union_type(types);
        if !self.ty(union_type).flags.intersects(TypeFlags::UNION) {
            return self.get_augmented_properties_of_type(union_type);
        }

        let props = self.symbols.new_table();
        for &member_type in types {
            let augmented_props = self.get_augmented_properties_of_type(member_type);
            for p in augmented_props {
                let name = self.sym(p).name.clone();
                if self.symbols.get_name(props, &name).is_nil() {
                    let prop = self.create_union_or_intersection_property(
                        union_type,
                        TableKey::Name(&name),
                        false, /*skipObjectFunctionPropertyAugment*/
                    );
                    // May be undefined if the property is private
                    if prop.is_some() {
                        self.symbols.set(props, name, prop);
                    }
                }
            }
        }
        self.symbols.values(props)
    }

    // Go: checker/services.go:232 IsUnknownSymbol
    pub fn is_unknown_symbol(&self, symbol: SymbolId) -> bool {
        symbol == self.unknown_symbol
    }

    // Go: checker/services.go:236 IsUndefinedSymbol
    pub fn is_undefined_symbol(&self, symbol: SymbolId) -> bool {
        symbol == self.undefined_symbol
    }

    // Go: checker/services.go:240 IsArgumentsSymbol
    pub fn is_arguments_symbol(&self, symbol: SymbolId) -> bool {
        symbol == self.arguments_symbol
    }

    // Go: checker/services.go:245 GetNonOptionalType
    // Originally from services.ts
    pub fn get_non_optional_type(&mut self, t: TypeId) -> TypeId {
        self.remove_optional_type_marker(t)
    }

    // Go: checker/services.go:249 GetStringIndexType
    pub fn get_string_index_type(&mut self, t: TypeId) -> TypeId {
        let string_type = self.string_type;
        self.get_index_type_of_type(t, string_type)
    }

    // Go: checker/services.go:253 GetNumberIndexType
    pub fn get_number_index_type(&mut self, t: TypeId) -> TypeId {
        let number_type = self.number_type;
        self.get_index_type_of_type(t, number_type)
    }

    // Go: checker/services.go:257 GetElementTypeOfArrayType
    pub fn get_element_type_of_array_type_exported(&mut self, t: TypeId) -> TypeId {
        self.get_element_type_of_array_type(t)
    }

    // Go: checker/services.go:261 GetCallSignatures
    pub fn get_call_signatures(&mut self, t: TypeId) -> Vec<SignatureId> {
        self.get_signatures_of_type(t, SignatureKind::CALL).to_vec()
    }

    // Go: checker/services.go:265 GetConstructSignatures
    pub fn get_construct_signatures(&mut self, t: TypeId) -> Vec<SignatureId> {
        self.get_signatures_of_type(t, SignatureKind::CONSTRUCT)
            .to_vec()
    }

    // Go: checker/services.go:269 GetApparentProperties
    pub fn get_apparent_properties(&mut self, t: TypeId) -> Vec<SymbolId> {
        self.get_augmented_properties_of_type(t)
    }

    // Go: checker/services.go:273 getAugmentedPropertiesOfType
    pub fn get_augmented_properties_of_type(&mut self, t: TypeId) -> Vec<SymbolId> {
        let t = self.get_apparent_type(t);
        let properties = self.get_properties_of_type(t);
        let mut props_by_name = self.create_symbol_table(&properties);
        let mut function_type = TypeId::NIL;
        if !self
            .get_signatures_of_type(t, SignatureKind::CALL)
            .is_empty()
        {
            function_type = self.global_callable_function_type;
        } else if !self
            .get_signatures_of_type(t, SignatureKind::CONSTRUCT)
            .is_empty()
        {
            function_type = self.global_newable_function_type;
        }

        if props_by_name.is_nil() {
            props_by_name = self.symbols.new_table();
        }
        if function_type.is_some() {
            let function_properties = self.get_properties_of_type(function_type);
            for &p in function_properties.iter() {
                let name = self.sym(p).name.clone();
                if self.symbols.get_name(props_by_name, &name).is_nil() {
                    self.symbols.set(props_by_name, name, p);
                }
            }
        }
        self.get_named_members(props_by_name, SymbolId::NIL)
            .to_vec()
    }

    // Go: checker/services.go:296 TryGetMemberInModuleExportsAndProperties
    pub fn try_get_member_in_module_exports_and_properties(
        &mut self,
        member_name: &str,
        module_symbol: SymbolId,
    ) -> SymbolId {
        let symbol = self.try_get_member_in_module_exports(member_name, module_symbol);
        if symbol.is_some() {
            return symbol;
        }

        let export_equals =
            self.resolve_external_module_symbol(module_symbol, false /*dontResolveAlias*/);
        if export_equals == module_symbol {
            return SymbolId::NIL;
        }

        let t = self.get_type_of_symbol(export_equals);
        if self.should_treat_properties_of_external_module_as_exports(t) {
            return self.get_property_of_type(t, member_name);
        }
        SymbolId::NIL
    }

    // Go: checker/services.go:314 TryGetMemberInModuleExports
    pub fn try_get_member_in_module_exports(
        &mut self,
        member_name: &str,
        module_symbol: SymbolId,
    ) -> SymbolId {
        let symbol_table = self.get_exports_of_module(module_symbol);
        self.symbols.get(symbol_table, member_name)
    }

    // Go: checker/services.go:319 shouldTreatPropertiesOfExternalModuleAsExports
    pub fn should_treat_properties_of_external_module_as_exports(
        &self,
        resolved_external_module_type: TypeId,
    ) -> bool {
        !self
            .ty(resolved_external_module_type)
            .flags
            .intersects(TypeFlags::PRIMITIVE)
            || self
                .ty(resolved_external_module_type)
                .object_flags
                .intersects(ObjectFlags::CLASS)
            // `isArrayOrTupleLikeType` is too expensive to use in this auto-imports hot path.
            || self.is_array_type(resolved_external_module_type)
            || self.is_tuple_type(resolved_external_module_type)
    }

    // Go: checker/services.go:327 GetContextualType
    pub fn get_contextual_type_exported(
        &mut self,
        node: Node,
        context_flags: ContextFlags,
    ) -> TypeId {
        if context_flags.intersects(ContextFlags::IGNORE_NODE_INFERENCES) {
            return self.run_with_inference_blocked_from_source_node(node, |c| {
                c.get_contextual_type(node, context_flags)
            });
        }
        self.get_contextual_type(node, context_flags)
    }

    // Go: checker/services.go:334 runWithInferenceBlockedFromSourceNode
    // PORT: no guard restores the flags if `f` panics; Go has no defer here either.
    pub fn run_with_inference_blocked_from_source_node<T>(
        &mut self,
        node: Node,
        f: impl FnOnce(&mut Checker) -> T,
    ) -> T {
        let containing_call = find_ancestor(node, is_call_like_expression);
        if containing_call.is_some() {
            let mut to_mark_skip = node;
            loop {
                self.skip_direct_inference_nodes.insert(to_mark_skip);
                to_mark_skip = to_mark_skip.parent();
                if to_mark_skip.is_nil() || to_mark_skip == containing_call {
                    break;
                }
            }
        }

        self.is_inference_partially_blocked = true;
        let result = self.run_without_resolved_signature_caching(node, f);
        self.is_inference_partially_blocked = false;

        self.skip_direct_inference_nodes.clear();
        result
    }

    // Go: checker/services.go:355 GetResolvedSignatureForSignatureHelp
    pub fn get_resolved_signature_for_signature_help(
        &mut self,
        node: Node,
        argument_count: i32,
    ) -> (SignatureId, Vec<SignatureId>) {
        self.run_without_resolved_signature_caching(node, |c| {
            c.get_resolved_signature_worker(node, CheckMode::IS_FOR_SIGNATURE_HELP, argument_count)
        })
    }

    // Go: checker/services.go:367 runWithoutResolvedSignatureCaching
    // PORT: Go saves the links by pointer (`map[*SignatureLinks]*Signature`,
    // `map[*ValueSymbolLinks]*Type`). Here the saved values are keyed by
    // node and symbol and written back through the link stores after `f`.
    // No guard restores them if `f` panics; Go has no defer here either.
    pub fn run_without_resolved_signature_caching<T>(
        &mut self,
        node: Node,
        f: impl FnOnce(&mut Checker) -> T,
    ) -> T {
        let mut ancestor_node = find_ancestor(node, is_call_like_or_function_like_expression);
        if ancestor_node.is_some() {
            let mut cached_resolved_signatures: FxHashMap<Node, SignatureId> = FxHashMap::default();
            let mut cached_types: FxHashMap<SymbolId, TypeId> = FxHashMap::default();
            while ancestor_node.is_some() {
                let signature_links = self.signature_links.get(ancestor_node);
                cached_resolved_signatures
                    .insert(ancestor_node, signature_links.resolved_signature);
                signature_links.resolved_signature = SignatureId::NIL;
                if is_function_expression_or_arrow_function(ancestor_node) {
                    let symbol = self.get_symbol_of_declaration(ancestor_node);
                    let symbol_links = self.value_symbol_links.get_by_id(&self.symbols, symbol);
                    let resolved_type = symbol_links.resolved_type;
                    cached_types.insert(symbol, resolved_type);
                    symbol_links.resolved_type = TypeId::NIL;
                }
                ancestor_node = find_ancestor(
                    ancestor_node.parent(),
                    is_call_like_or_function_like_expression,
                );
            }
            let result = f(self);
            for (links_node, resolved_signature) in cached_resolved_signatures {
                self.signature_links.get(links_node).resolved_signature = resolved_signature;
            }
            for (symbol, resolved_type) in cached_types {
                self.value_symbol_links
                    .get_by_id(&self.symbols, symbol)
                    .resolved_type = resolved_type;
            }
            return result;
        }
        f(self)
    }

    // Go: checker/services.go:396 SkipAlias
    // PORT: the Go package function `SkipAlias(symbol, checker)`
    // (utilities.go:1622) is already `Checker::skip_alias`, so this method
    // ends in `_exported`.
    pub fn skip_alias_exported(&mut self, symbol: SymbolId) -> SymbolId {
        if self.sym(symbol).flags.intersects(SymbolFlags::ALIAS) {
            return self.get_aliased_symbol(symbol);
        }
        symbol
    }

    // Go: checker/services.go:403 GetRootSymbols
    pub fn get_root_symbols(&mut self, symbol: SymbolId) -> Vec<SymbolId> {
        let roots = self.get_immediate_root_symbols(symbol);
        if roots.is_empty() {
            return vec![symbol];
        }
        let mut result = Vec::new();
        for root in roots {
            result.extend(self.get_root_symbols(root));
        }
        result
    }

    // Go: checker/services.go:415 GetMappedTypeSymbolOfProperty
    pub fn get_mapped_type_symbol_of_property(&self, symbol: SymbolId) -> SymbolId {
        if let Some(value_links) = self.value_symbol_links.try_get_by_id(&self.symbols, symbol) {
            return self.ty(value_links.containing_type).symbol;
        }
        SymbolId::NIL
    }

    // Go: checker/services.go:422 getImmediateRootSymbols
    pub fn get_immediate_root_symbols(&mut self, symbol: SymbolId) -> Vec<SymbolId> {
        if self
            .sym(symbol)
            .check_flags
            .intersects(CheckFlags::SYNTHETIC)
        {
            let containing_type = self
                .value_symbol_links
                .get_by_id(&self.symbols, symbol)
                .containing_type;
            let types = self.ty(containing_type).types().to_vec();
            let name = self.sym(symbol).name.clone();
            // core.MapNonNil
            let mut result = Vec::new();
            for t in types {
                let prop = self.get_property_of_type(t, name.as_str());
                if prop.is_some() {
                    result.push(prop);
                }
            }
            return result;
        }
        if self.sym(symbol).flags.intersects(SymbolFlags::TRANSIENT) {
            if self.spread_links.has(symbol) {
                let left_spread = self.spread_links.get(symbol).left_spread;
                let right_spread = self.spread_links.get(symbol).right_spread;
                if left_spread.is_some() {
                    return vec![left_spread, right_spread];
                }
            }
            if self.mapped_symbol_links.has(symbol) {
                let synthetic_origin = self.mapped_symbol_links.get(symbol).synthetic_origin;
                if synthetic_origin.is_some() {
                    return vec![synthetic_origin];
                }
            }
            let target = self.try_get_target(symbol);
            if target.is_some() {
                return vec![target];
            }
        }
        Vec::new()
    }

    // Go: checker/services.go:453 tryGetTarget
    pub fn try_get_target(&mut self, symbol: SymbolId) -> SymbolId {
        let mut target = SymbolId::NIL;
        let mut next = symbol;
        loop {
            if self.value_symbol_links.has_by_id(&self.symbols, next) {
                next = self
                    .value_symbol_links
                    .get_by_id(&self.symbols, next)
                    .target;
            } else if self.export_type_links.has(next) {
                next = self.export_type_links.get(next).target;
            } else {
                next = SymbolId::NIL;
            }
            if next.is_nil() {
                break;
            }
            target = next;
        }
        target
    }

    // Go: checker/services.go:472 GetExportSymbolOfSymbol
    pub fn get_export_symbol_of_symbol(&self, symbol: SymbolId) -> SymbolId {
        let export_symbol = self.sym(symbol).export_symbol;
        self.get_merged_symbol(if export_symbol.is_some() {
            export_symbol
        } else {
            symbol
        })
    }

    // Go: checker/services.go:476 GetExportSpecifierLocalTargetSymbol
    pub fn get_export_specifier_local_target_symbol(&mut self, node: Node) -> SymbolId {
        // node should be ExportSpecifier | Identifier
        match node.kind() {
            SyntaxKind::ExportSpecifier => {
                if node.parent().parent().module_specifier().is_some() {
                    return self.get_external_module_member(
                        node.parent().parent(),
                        node,
                        false, /*dontResolveAlias*/
                    );
                }
                let name = node.property_name_or_name();
                if name.kind() == SyntaxKind::StringLiteral {
                    // Skip for invalid syntax like this: export { "x" }
                    return SymbolId::NIL;
                }
                return self.resolve_entity_name(
                    name,
                    SymbolFlags::VALUE
                        | SymbolFlags::TYPE
                        | SymbolFlags::NAMESPACE
                        | SymbolFlags::ALIAS,
                    true, /*ignoreErrors*/
                    false,
                    Node::NIL,
                );
            }
            SyntaxKind::Identifier => {
                return self.resolve_entity_name(
                    node,
                    SymbolFlags::VALUE
                        | SymbolFlags::TYPE
                        | SymbolFlags::NAMESPACE
                        | SymbolFlags::ALIAS,
                    true, /*ignoreErrors*/
                    false,
                    Node::NIL,
                );
            }
            _ => {}
        }
        panic!(
            "Unhandled case in getExportSpecifierLocalTargetSymbol, node should be ExportSpecifier | Identifier"
        );
    }

    // Go: checker/services.go:495 GetShorthandAssignmentValueSymbol
    pub fn get_shorthand_assignment_value_symbol(&mut self, location: Node) -> SymbolId {
        if location.is_some() && location.kind() == SyntaxKind::ShorthandPropertyAssignment {
            return self.resolve_entity_name(
                location.name(),
                SymbolFlags::VALUE | SymbolFlags::ALIAS,
                true, /*ignoreErrors*/
                false,
                Node::NIL,
            );
        }
        SymbolId::NIL
    }

    // Go: checker/services.go:508 GetSymbolsOfParameterPropertyDeclaration
    /**
     * Get symbols that represent parameter-property-declaration as parameter and as property declaration
     * @param parameter a parameterDeclaration node
     * @param parameterName a name of the parameter to get the symbols for.
     * @return a tuple of two symbols
     */
    pub fn get_symbols_of_parameter_property_declaration(
        &mut self,
        parameter: Node, /*ParameterPropertyDeclaration*/
        parameter_name: &str,
    ) -> (SymbolId, SymbolId) {
        let constructor_declaration = parameter.parent();
        let class_declaration = parameter.parent().parent();

        let parameter_symbol = self.get_symbol(
            constructor_declaration.locals(),
            parameter_name,
            SymbolFlags::VALUE,
        );
        let members = self.get_members_of_symbol(class_declaration.symbol());
        let property_symbol = self.get_symbol(members, parameter_name, SymbolFlags::VALUE);

        if parameter_symbol.is_some() && property_symbol.is_some() {
            return (parameter_symbol, property_symbol);
        }

        panic!(
            "There should exist two symbols, one as property declaration and one as parameter declaration"
        );
    }

    // Go: checker/services.go:524 IsDeclarationUsed
    // IsDeclarationUsed checks if an import declaration identifier is used in the source file.
    // This is primarily used for organizing imports to determine which imports can be removed.
    pub fn is_declaration_used(
        &mut self,
        source_file: Node,
        identifier: Node,
        jsx_elements_present: bool,
        jsx_mode_needs_explicit_import: bool,
    ) -> bool {
        if jsx_elements_present && jsx_mode_needs_explicit_import {
            let jsx_namespace = self.get_jsx_namespace(source_file);
            let jsx_fragment_factory = self.get_jsx_fragment_factory(source_file);
            let identifier_text = identifier.text();
            if identifier_text == jsx_namespace {
                return true;
            }
            if !jsx_fragment_factory.is_empty() && identifier_text == jsx_fragment_factory {
                return true;
            }
        }

        let symbol = self.get_symbol_at_location_exported(identifier);
        if symbol.is_nil() {
            return true;
        }

        self.is_symbol_referenced_in_file(source_file, identifier, symbol)
    }

    // Go: checker/services.go:552 IsSymbolReferencedInFile
    // IsSymbolReferencedInFile checks if a symbol is referenced in the source file (besides its definition).
    // This is used as a quick check for whether a symbol is used at all in a file.
    pub fn is_symbol_referenced_in_file(
        &mut self,
        source_file: Node,
        definition: Node,
        symbol: SymbolId,
    ) -> bool {
        let identifier_text = definition.text();
        for token in get_possible_symbol_reference_nodes(source_file, identifier_text, source_file)
        {
            if !is_identifier(token) {
                continue;
            }
            if token == definition || token.text() != identifier_text {
                continue;
            }
            let ref_symbol = self.get_symbol_at_location_exported(token);
            if ref_symbol == symbol {
                return true;
            }
            if token.parent().is_some()
                && token.parent().kind() == SyntaxKind::ShorthandPropertyAssignment
            {
                let shorthand_symbol = self.get_shorthand_assignment_value_symbol(token.parent());
                if shorthand_symbol == symbol {
                    return true;
                }
            }
            if token.parent().is_some() && is_export_specifier(token.parent()) {
                let local_symbol =
                    self.get_local_symbol_for_export_specifier(token, ref_symbol, token.parent());
                if local_symbol == symbol {
                    return true;
                }
            }
        }
        false
    }

    // Go: checker/services.go:587 GetReferencesToSymbolInFile
    // GetReferencesToSymbolInFile returns all identifier nodes in the file that reference the given symbol.
    pub fn get_references_to_symbol_in_file(
        &mut self,
        source_file: Node,
        symbol: SymbolId,
    ) -> Vec<Node> {
        let identifier_text = self.sym(symbol).name.as_str();
        let mut result = Vec::new();
        for token in get_possible_symbol_reference_nodes(source_file, identifier_text, source_file)
        {
            if !is_identifier(token) {
                continue;
            }
            if token.text() != identifier_text {
                continue;
            }
            let ref_symbol = self.get_symbol_at_location_exported(token);
            if ref_symbol == symbol {
                result.push(token);
                continue;
            }
            if token.parent().is_some()
                && token.parent().kind() == SyntaxKind::ShorthandPropertyAssignment
            {
                let shorthand_symbol = self.get_shorthand_assignment_value_symbol(token.parent());
                if shorthand_symbol == symbol {
                    result.push(token);
                    continue;
                }
            }
            if token.parent().is_some() && is_export_specifier(token.parent()) {
                let local_symbol =
                    self.get_local_symbol_for_export_specifier(token, ref_symbol, token.parent());
                if local_symbol == symbol {
                    result.push(token);
                    continue;
                }
            }
        }
        result
    }

    // Go: checker/services.go:624 getLocalSymbolForExportSpecifier
    pub fn get_local_symbol_for_export_specifier(
        &mut self,
        reference_location: Node,
        reference_symbol: SymbolId,
        export_specifier: Node,
    ) -> SymbolId {
        if is_export_specifier_alias(reference_location, export_specifier) {
            let symbol = self.get_export_specifier_local_target_symbol(export_specifier);
            if symbol.is_some() {
                return symbol;
            }
        }
        reference_symbol
    }
}

// Go: checker/services.go:633 isExportSpecifierAlias
pub fn is_export_specifier_alias(reference_location: Node, export_specifier: Node) -> bool {
    go_assert!(
        export_specifier.property_name() == reference_location
            || export_specifier.name() == reference_location,
        "referenceLocation is not export specifier name or property name"
    );
    let property_name = export_specifier.property_name();
    if property_name.is_some() {
        // Given `export { foo as bar } [from "someModule"]`: It's an alias at `foo`, but at `bar` it's a new symbol.
        property_name == reference_location
    } else {
        // `export { foo } from "foo"` is a re-export.
        // `export { foo };` is not a re-export, it creates an alias for the local variable `foo`.
        export_specifier
            .parent()
            .parent()
            .module_specifier()
            .is_nil()
    }
}

// Go: checker/services.go:646 getPossibleSymbolReferenceNodes
pub fn get_possible_symbol_reference_nodes(
    source_file: Node,
    symbol_name: &str,
    container: Node,
) -> Vec<Node> {
    // core.MapNonNil
    let mut result = Vec::new();
    for pos in get_possible_symbol_reference_positions(source_file, symbol_name, container) {
        let reference_location = crate::astnav::get_touching_property_name(source_file, pos);
        let mapped = if reference_location != source_file {
            reference_location
        } else {
            Node::NIL
        };
        if mapped.is_some() {
            result.push(mapped);
        }
    }
    result
}

// Go: checker/services.go:655 getPossibleSymbolReferencePositions
// PORT: Go has two copies of this function with the same body (here and
// ls/findallreferences.go:1646). The port keeps one: the ls copy, which
// searches Go bytes and reads the bytes next to a match as Go bytes (see
// its PORT notes). Before, this copy read port bytes there, so a match
// next to an invalid byte unit tested a port byte (0x85 for Go's 0xC5).
pub fn get_possible_symbol_reference_positions(
    source_file: Node,
    symbol_name: &str,
    container: Node,
) -> Vec<i32> {
    crate::ls::findallreferences_p2::get_possible_symbol_reference_positions(
        source_file,
        symbol_name,
        container,
    )
}

impl Checker {
    // Go: checker/services.go:700 GetTypeArgumentConstraint
    pub fn get_type_argument_constraint_exported(&mut self, node: Node) -> TypeId {
        if !is_type_node(node) {
            return TypeId::NIL;
        }
        self.get_type_argument_constraint(node)
    }

    // Go: checker/services.go:708 getUninstantiatedSignatures
    // getUninstantiatedSignatures gets generic signatures from the function's/constructor's type.
    pub fn get_uninstantiated_signatures(&mut self, node: Node) -> Vec<SignatureId> {
        match node.kind() {
            SyntaxKind::CallExpression | SyntaxKind::Decorator => {
                let t = self.get_type_of_expression(node.expression());
                return self.get_signatures_of_type(t, SignatureKind::CALL).to_vec();
            }
            SyntaxKind::NewExpression => {
                let t = self.get_type_of_expression(node.expression());
                return self
                    .get_signatures_of_type(t, SignatureKind::CONSTRUCT)
                    .to_vec();
            }
            SyntaxKind::JsxSelfClosingElement | SyntaxKind::JsxOpeningElement => {
                if is_jsx_intrinsic_tag_name(node.tag_name()) {
                    return Vec::new();
                }
                let t = self.get_type_of_expression(node.tag_name());
                return self.get_signatures_of_type(t, SignatureKind::CALL).to_vec();
            }
            SyntaxKind::TaggedTemplateExpression => {
                let t = self.get_type_of_expression(node.tag());
                return self.get_signatures_of_type(t, SignatureKind::CALL).to_vec();
            }
            SyntaxKind::BinaryExpression | SyntaxKind::JsxOpeningFragment => {
                return Vec::new();
            }
            _ => {}
        }
        Vec::new()
    }

    // Go: checker/services.go:727 getTypeParameterConstraintForPositionAcrossSignatures
    pub fn get_type_parameter_constraint_for_position_across_signatures(
        &mut self,
        signatures: &[SignatureId],
        position: i32,
    ) -> TypeId {
        let mut relevant_constraints = Vec::new();
        for &signature in signatures {
            if position >= self.sig(signature).type_parameters.len() as i32 {
                continue;
            }
            let relevant_type_parameter = self.sig(signature).type_parameters[position as usize];
            let relevant_constraint =
                self.get_constraint_of_type_parameter(relevant_type_parameter);
            if relevant_constraint.is_some() {
                relevant_constraints.push(relevant_constraint);
            }
        }
        self.get_union_type(&relevant_constraints)
    }

    // Go: checker/services.go:742 getTypeArgumentConstraint
    pub fn get_type_argument_constraint(&mut self, node: Node) -> TypeId {
        let mut type_argument_position: i32 = -1;
        if has_type_arguments(node.parent()) {
            let type_args = node.parent().type_arguments();
            for (i, arg) in type_args.iter().enumerate() {
                if arg == node {
                    type_argument_position = i as i32;
                    break;
                }
            }
        }

        if type_argument_position >= 0 {
            // The node could be a type argument of a call, a `new` expression, a decorator, an
            // instantiation expression, or a generic type instantiation.

            if is_call_like_expression(node.parent()) {
                let signatures = self.get_uninstantiated_signatures(node.parent());
                return self.get_type_parameter_constraint_for_position_across_signatures(
                    &signatures,
                    type_argument_position,
                );
            }

            if is_decorator(node.parent().parent()) {
                let signatures = self.get_uninstantiated_signatures(node.parent().parent());
                return self.get_type_parameter_constraint_for_position_across_signatures(
                    &signatures,
                    type_argument_position,
                );
            }

            if is_expression_with_type_arguments(node.parent())
                && is_expression_statement(node.parent().parent())
            {
                let uninstantiated_type = self.check_expression(node.parent().expression());

                let call_signatures =
                    self.get_signatures_of_type(uninstantiated_type, SignatureKind::CALL);
                let call_constraint = self
                    .get_type_parameter_constraint_for_position_across_signatures(
                        &call_signatures,
                        type_argument_position,
                    );
                let construct_signatures =
                    self.get_signatures_of_type(uninstantiated_type, SignatureKind::CONSTRUCT);
                let construct_constraint = self
                    .get_type_parameter_constraint_for_position_across_signatures(
                        &construct_signatures,
                        type_argument_position,
                    );

                // An instantiation expression instantiates both call and construct signatures, so
                // if both exist type arguments must be assignable to both constraints.
                if self
                    .ty(construct_constraint)
                    .flags
                    .intersects(TypeFlags::NEVER)
                {
                    return call_constraint;
                }
                if self.ty(call_constraint).flags.intersects(TypeFlags::NEVER) {
                    return construct_constraint;
                }
                return self.get_intersection_type(&[call_constraint, construct_constraint]);
            }

            if is_type_reference_type(node.parent()) {
                let type_parameters =
                    self.get_type_parameters_for_type_reference_or_import(node.parent());
                if type_parameters.is_empty() {
                    return TypeId::NIL;
                }
                if type_argument_position >= type_parameters.len() as i32 {
                    return TypeId::NIL;
                }
                let relevant_type_parameter = type_parameters[type_argument_position as usize];
                let constraint = self.get_constraint_of_type_parameter(relevant_type_parameter);
                if constraint.is_some() {
                    let type_arguments =
                        self.get_effective_type_arguments(node.parent(), &type_parameters);
                    let mapper = self.new_type_mapper(&type_parameters, &type_arguments);
                    return self.instantiate_type(constraint, mapper);
                }
            }
        }
        TypeId::NIL
    }

    // Go: checker/services.go:816 IsTypeInvalidDueToUnionDiscriminant
    pub fn is_type_invalid_due_to_union_discriminant(
        &mut self,
        contextual_type: TypeId,
        obj: Node,
    ) -> bool {
        let properties = obj.properties();
        // core.Some
        for property in properties.iter() {
            let mut name_type = TypeId::NIL;
            let property_name = property.name();
            if property_name.is_some() {
                if is_jsx_namespaced_name(property_name) {
                    name_type = self.get_string_literal_type(property_name.text());
                } else {
                    name_type = self.get_literal_type_from_property_name(property_name);
                }
            }
            let mut name = String::new();
            if name_type.is_some() && self.is_type_usable_as_property_name(name_type) {
                name = self.get_property_name_from_type(name_type);
            }
            let mut expected = TypeId::NIL;
            if !name.is_empty() {
                expected = self.get_type_of_property_of_type(contextual_type, &name);
            }
            // PORT: the last `&&` operand of Go's return is the nested `if`.
            if expected.is_some() && self.is_literal_type(expected) {
                let property_type = self.get_type_of_node(property);
                if !self.is_type_assignable_to(property_type, expected) {
                    return true;
                }
            }
        }
        false
    }

    // Go: checker/services.go:841 GetExportsAndPropertiesOfModule
    // Unlike `getExportsOfModule`, this includes properties of an `export =` value.
    pub fn get_exports_and_properties_of_module(
        &mut self,
        module_symbol: SymbolId,
    ) -> Vec<SymbolId> {
        let mut exports = self.get_exports_of_module_as_array(module_symbol);
        let export_equals =
            self.resolve_external_module_symbol(module_symbol, false /*dontResolveAlias*/);
        if export_equals != module_symbol {
            let t = self.get_type_of_symbol(export_equals);
            if self.should_treat_properties_of_external_module_as_exports(t) {
                let properties = self.get_properties_of_type(t);
                exports.extend(properties.iter().copied());
            }
        }
        exports
    }

    // Go: checker/services.go:853 getExportsOfModuleAsArray
    pub fn get_exports_of_module_as_array(&mut self, module_symbol: SymbolId) -> Vec<SymbolId> {
        let exports = self.get_exports_of_module(module_symbol);
        self.symbols_to_array(exports)
    }

    // Go: checker/services.go:858 GetJsxIntrinsicTagNamesAt
    // Returns all the properties of the Jsx.IntrinsicElements interface.
    // PORT: Go `JsxNames.IntrinsicElements` is the string "IntrinsicElements"
    // (as in jsx_p2.rs).
    pub fn get_jsx_intrinsic_tag_names_at(&mut self, location: Node) -> Vec<SymbolId> {
        let intrinsics = self.get_jsx_type("IntrinsicElements", location);
        if intrinsics.is_nil() {
            return Vec::new();
        }
        self.get_properties_of_type_exported(intrinsics)
    }

    // Go: checker/services.go:866 GetContextualTypeForJsxAttribute
    pub fn get_contextual_type_for_jsx_attribute_exported(&mut self, attribute: Node) -> TypeId {
        self.get_contextual_type_for_jsx_attribute(attribute, ContextFlags::NONE)
    }

    // Go: checker/services.go:899 getResolvedSignatureWorker
    pub fn get_resolved_signature_worker(
        &mut self,
        node: Node,
        check_mode: CheckMode,
        argument_count: i32,
    ) -> (SignatureId, Vec<SignatureId>) {
        self.apparent_argument_count = Some(argument_count);
        let mut candidates_out_array: Vec<SignatureId> = Vec::new();
        let mut res = SignatureId::NIL;
        // ts#64649, Go N' services.go:902: a parse tree test, no emit context.
        if node.is_some() && is_parse_tree_node(node) {
            res = self.get_resolved_signature(node, Some(&mut candidates_out_array), check_mode);
        }
        self.apparent_argument_count = None;
        (res, candidates_out_array)
    }

    // Go: checker/services.go:911 GetCandidateSignaturesForStringLiteralCompletions
    pub fn get_candidate_signatures_for_string_literal_completions(
        &mut self,
        call: Node,
        editing_argument: Node,
    ) -> Vec<SignatureId> {
        // first, get candidates when inference is blocked from the source node.
        let mut candidates =
            self.run_with_inference_blocked_from_source_node(editing_argument, |c| {
                let (_, blocked_inference_candidates) =
                    c.get_resolved_signature_worker(call, CheckMode::NORMAL, 0);
                blocked_inference_candidates
            });
        let candidates_set: FxHashSet<SignatureId> = candidates.iter().copied().collect();

        // next, get candidates where the source node is considered for inference.
        let other_candidates = self.run_without_resolved_signature_caching(editing_argument, |c| {
            let (_, inference_candidates) =
                c.get_resolved_signature_worker(call, CheckMode::NORMAL, 0);
            inference_candidates
        });

        for candidate in other_candidates {
            if candidates_set.contains(&candidate) {
                continue;
            }
            candidates.push(candidate);
        }

        candidates
    }

    // Go: checker/services.go:936 GetTypeAtPosition
    // GetTypeAtPosition returns the type of a parameter at a given index in a signature.
    pub fn get_type_at_position_exported(&mut self, s: SignatureId, pos: i32) -> TypeId {
        self.get_type_at_position(s, pos)
    }

    // Go: checker/services.go:940 GetTypeParameterAtPosition
    pub fn get_type_parameter_at_position(&mut self, s: SignatureId, pos: i32) -> TypeId {
        let t = self.get_type_at_position(s, pos);
        if self.ty(t).is_index() {
            let target = self.ty(t).as_index_type().target;
            if self.is_this_type_parameter(target) {
                let constraint = self.get_base_constraint_of_type(target);
                if constraint.is_some() {
                    return self.get_index_type(constraint);
                }
            }
        }
        t
    }

    // Go: checker/services.go:953 GetContextualTypeForArrayLiteralAtPosition
    // GetContextualTypeForArrayLiteralAtPosition returns the contextual type for an element at the given position
    // in an array with the given contextual type.
    pub fn get_contextual_type_for_array_literal_at_position(
        &mut self,
        contextual_array_type: TypeId,
        array_literal: Node,
        position: i32,
    ) -> TypeId {
        if contextual_array_type.is_nil() {
            return TypeId::NIL;
        }
        let (mut first_spread_index, mut last_spread_index): (i32, i32) = (-1, -1);
        let mut element_index: i32 = 0;
        let elements = array_literal.elements();
        for (i, elem) in elements.iter().enumerate() {
            if elem.pos() < position {
                element_index += 1;
            }
            if is_spread_element(elem) {
                if first_spread_index == -1 {
                    first_spread_index = i as i32;
                }
                last_spread_index = i as i32;
            }
        }
        // The array may be incomplete, so we don't know its final length.
        self.get_contextual_type_for_element_expression(
            contextual_array_type,
            element_index,
            -1, /*length*/
            first_spread_index,
            last_spread_index,
        )
    }
}

// Go: checker/services.go:981 knownGenericTypeNames
static KNOWN_GENERIC_TYPE_NAMES: LazyLock<FxHashSet<&'static str>> = LazyLock::new(|| {
    [
        "Array",
        "ArrayLike",
        "ReadonlyArray",
        "Promise",
        "PromiseLike",
        "Iterable",
        "IterableIterator",
        "AsyncIterable",
        "Set",
        "WeakSet",
        "ReadonlySet",
        "Map",
        "WeakMap",
        "ReadonlyMap",
        "Partial",
        "Required",
        "Readonly",
        "Pick",
        "Omit",
        "NonNullable",
    ]
    .into_iter()
    .collect()
});

// Go: checker/services.go:1004 isKnownGenericTypeName
pub fn is_known_generic_type_name(name: &str) -> bool {
    KNOWN_GENERIC_TYPE_NAMES.contains(name)
}

impl Checker {
    // Go: checker/services.go:1009 GetFirstTypeArgumentFromKnownType
    pub fn get_first_type_argument_from_known_type(&mut self, t: TypeId) -> TypeId {
        let object_flags = self.ty(t).object_flags;
        let symbol = self.ty(t).symbol;
        if object_flags.intersects(ObjectFlags::REFERENCE)
            && symbol.is_some()
            && is_known_generic_type_name(self.sym(symbol).name.as_str())
        {
            let name = self.sym(symbol).name.as_str();
            let global_symbol = self.get_global_symbol(name, SymbolFlags::TYPE, None);
            let target = self.ty(t).target();
            if global_symbol.is_some() && global_symbol == self.ty(target).symbol {
                // core.FirstOrNil
                return self
                    .get_type_arguments(t)
                    .first()
                    .copied()
                    .unwrap_or(TypeId::NIL);
            }
        }
        if let Some(alias) = self.ty(t).alias.clone() {
            if is_known_generic_type_name(self.sym(alias.symbol).name.as_str()) {
                let name = self.sym(alias.symbol).name.as_str();
                let global_symbol = self.get_global_symbol(name, SymbolFlags::TYPE, None);
                if global_symbol.is_some() && global_symbol == alias.symbol {
                    // core.FirstOrNil
                    return alias.type_arguments.first().copied().unwrap_or(TypeId::NIL);
                }
            }
        }
        TypeId::NIL
    }

    // Go: checker/services.go:1026 GetPropertySymbolsFromContextualType
    // Gets all symbols for one property. Does not get symbols for every property.
    pub fn get_property_symbols_from_contextual_type(
        &mut self,
        node: Node,
        contextual_type: TypeId,
        union_symbol_ok: bool,
    ) -> Vec<SymbolId> {
        let name = get_text_of_property_name(node.name());
        if name.is_empty() {
            return Vec::new();
        }
        if !self.ty(contextual_type).flags.intersects(TypeFlags::UNION) {
            let symbol = self.get_property_of_type(contextual_type, &name);
            if symbol.is_some() {
                return vec![symbol];
            }
            return Vec::new();
        }
        let mut filtered_types = self.ty(contextual_type).types().to_vec();
        if is_object_literal_expression(node.parent()) || is_jsx_attributes(node.parent()) {
            // core.Filter
            let mut kept = Vec::with_capacity(filtered_types.len());
            for t in filtered_types {
                if !self.is_type_invalid_due_to_union_discriminant(t, node.parent()) {
                    kept.push(t);
                }
            }
            filtered_types = kept;
        }
        // core.MapNonNil
        let mut discriminated_property_symbols = Vec::new();
        for &t in &filtered_types {
            let symbol = self.get_property_of_type(t, &name);
            if symbol.is_some() {
                discriminated_property_symbols.push(symbol);
            }
        }
        if union_symbol_ok
            && (discriminated_property_symbols.is_empty()
                || discriminated_property_symbols.len() == self.ty(contextual_type).types().len())
        {
            let symbol = self.get_property_of_type(contextual_type, &name);
            if symbol.is_some() {
                return vec![symbol];
            }
        }
        if filtered_types.is_empty() && discriminated_property_symbols.is_empty() {
            // Bad discriminant -- do again without discriminating
            // core.MapNonNil
            let types = self.ty(contextual_type).types().to_vec();
            let mut result = Vec::new();
            for t in types {
                let symbol = self.get_property_of_type(t, &name);
                if symbol.is_some() {
                    result.push(symbol);
                }
            }
            return result;
        }
        // by eliminating duplicates we might even end up with a single symbol
        // that helps with displaying better quick infos on properties of union types
        // core.Deduplicate: keeps the first of each value, in order.
        let mut result: Vec<SymbolId> = Vec::with_capacity(discriminated_property_symbols.len());
        for symbol in discriminated_property_symbols {
            if !result.contains(&symbol) {
                result.push(symbol);
            }
        }
        result
    }

    // Go: checker/services.go:1071 GetPropertySymbolOfDestructuringAssignment
    // Gets the property symbol corresponding to the property in destructuring assignment
    // 'property1' from
    //
    //	for ( { property1: a } of elems) {
    //	}
    //
    // 'property1' at location 'a' from:
    //
    //	[a] = [ property1, property2 ]
    pub fn get_property_symbol_of_destructuring_assignment(&mut self, location: Node) -> SymbolId {
        if is_array_literal_or_object_literal_destructuring_pattern(location.parent().parent()) {
            // Get the type of the object or array literal and then look for property of given name in the type
            let type_of_object_literal =
                self.get_type_of_assignment_pattern(location.parent().parent());
            if type_of_object_literal.is_some() {
                return self.get_property_of_type(type_of_object_literal, location.text());
            }
        }
        SymbolId::NIL
    }

    // Go: checker/services.go:1090 getTypeOfAssignmentPattern
    // Gets the type of object literal or array literal of destructuring assignment.
    // { a } from
    //
    //	for ( { a } of elems) {
    //	}
    //
    // [ a ] from
    //
    //	[a] = [ some array ...]
    pub fn get_type_of_assignment_pattern(&mut self, expr: Node) -> TypeId {
        // If this is from "for of"
        //     for ( { a } of elems) {
        //     }
        if is_for_of_statement(expr.parent()) {
            let iterated_type = self.check_right_hand_side_of_for_of(expr.parent());
            let source_type = if iterated_type.is_some() {
                iterated_type
            } else {
                self.error_type
            };
            return self.check_destructuring_assignment(
                expr,
                source_type,
                CheckMode::NORMAL,
                false,
            );
        }
        // If this is from "for" initializer
        //     for ({a } = elems[0];.....) { }
        if is_binary_expression(expr.parent()) {
            let iterated_type = self.get_type_of_expression(expr.parent().right());
            let source_type = if iterated_type.is_some() {
                iterated_type
            } else {
                self.error_type
            };
            return self.check_destructuring_assignment(
                expr,
                source_type,
                CheckMode::NORMAL,
                false,
            );
        }
        // If this is from nested object binding pattern
        //     for ({ skills: { primary, secondary } } = multiRobot, i = 0; i < 1; i++) {
        if is_property_assignment(expr.parent()) {
            let node = expr.parent().parent();
            let parent_type = self.get_type_of_assignment_pattern(node);
            let type_of_parent_object_literal = if parent_type.is_some() {
                parent_type
            } else {
                self.error_type
            };
            // slices.Index
            let property_index = node
                .properties()
                .iter()
                .position(|property| property == expr.parent())
                .map_or(-1, |index| index as i32);
            return self.check_object_literal_destructuring_property_assignment(
                node,
                type_of_parent_object_literal,
                property_index,
                NodeList::NIL,
                false,
            );
        }
        // Array literal assignment - array destructuring pattern
        let node = expr.parent();
        //    [{ property1: p1, property2 }] = elems;
        let array_type = self.get_type_of_assignment_pattern(node);
        let type_of_array_literal = if array_type.is_some() {
            array_type
        } else {
            self.error_type
        };
        let undefined_type = self.undefined_type;
        let iterated_type = self.check_iterated_type_or_element_type(
            IterationUse::DESTRUCTURING,
            type_of_array_literal,
            undefined_type,
            expr.parent(),
        );
        let element_type = if iterated_type.is_some() {
            iterated_type
        } else {
            self.error_type
        };
        // slices.Index
        let element_index = node
            .elements()
            .iter()
            .position(|element| element == expr)
            .map_or(-1, |index| index as i32);
        self.check_array_literal_destructuring_element_assignment(
            node,
            type_of_array_literal,
            element_index,
            element_type,
            CheckMode::NORMAL,
        )
    }

    // Go: checker/services.go:1120 GetSignatureFromDeclaration
    pub fn get_signature_from_declaration_exported(&mut self, node: Node) -> SignatureId {
        self.get_signature_from_declaration(node)
    }

    // Go: checker/services.go:1125 IsLibSymbolForHoverVerbosity
    // IsLibSymbolForHoverVerbosity returns true if a symbol is declared in a lib file.
    // PORT: Go `c.program.IsSourceFileDefaultLibrary(sf.Path())` is the
    // program.rs free function over the installed program; `Path()` is the
    // `SourceFileInfo.path` field.
    pub fn is_lib_symbol_for_hover_verbosity(&self, symbol: SymbolId) -> bool {
        if symbol.is_nil() {
            return false;
        }
        for decl in self.sym(symbol).declarations.clone() {
            let sf = get_source_file_of_node(decl);
            if sf.is_some() && is_source_file_default_library(&source_file_info(sf).path) {
                return true;
            }
        }
        false
    }

    // Go: checker/services.go:1140 IsLibTypeForHoverVerbosity
    // IsLibTypeForHoverVerbosity returns true if a type is declared in a lib file.
    // Don't expand types like Array or Promise, instead treating them as opaque.
    pub fn is_lib_type_for_hover_verbosity(&self, t: TypeId) -> bool {
        let symbol = if self.ty(t).object_flags.intersects(ObjectFlags::REFERENCE) {
            let target = self.ty(t).target();
            self.ty(target).symbol
        } else {
            self.ty(t).symbol
        };
        if self.is_lib_symbol_for_hover_verbosity(symbol) {
            return true;
        }
        self.is_tuple_type(t)
    }
}
