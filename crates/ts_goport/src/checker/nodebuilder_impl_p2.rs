//! Port of `checker/nodebuilderimpl.go` lines 1019 to 2155: type parameter
//! lookup, symbol chains, module specifiers, mapped types, type predicates,
//! parameters, signatures and index infos.
//!
//! Methods follow the NodeBuilder pattern: `impl Checker` methods take
//! `b: &Rc<RefCell<NodeBuilderImpl>>`. No `RefCell` borrow of `b` is held
//! across a Checker call.

use crate::prelude::*;
use crate::printer::{EmitContext, EmitFlags};

type Nb = Rc<RefCell<NodeBuilderImpl>>;

// PORT: small readers for `b.ctx` and `b.e`. Each one takes and drops the
// borrow, so callers never hold it across a Checker call.
// Go `b.ctx`. The context is its own `Rc<RefCell<..>>`, so clone the handle
// out and drop the `b` borrow first.
fn nb_ctx(b: &Nb) -> Rc<RefCell<NodeBuilderContext>> {
    b.borrow().ctx.clone()
}

fn nb_flags(b: &Nb) -> NodeBuilderFlags {
    nb_ctx(b).borrow().flags
}

fn nb_clear_flags(b: &Nb, flags: NodeBuilderFlags) {
    let ctx = nb_ctx(b);
    let mut ctx = ctx.borrow_mut();
    ctx.flags = ctx.flags.without(flags);
}

fn nb_enclosing_declaration(b: &Nb) -> Node {
    nb_ctx(b).borrow().enclosing_declaration
}

fn nb_enclosing_file(b: &Nb) -> Node {
    nb_ctx(b).borrow().enclosing_file
}

fn nb_mapper(b: &Nb) -> MapperId {
    nb_ctx(b).borrow().mapper
}

fn nb_add_approximate_length(b: &Nb, n: i32) {
    nb_ctx(b).borrow_mut().approximate_length += n;
}

// Go `b.ctx.tracker`. Clone the tracker out so no borrow is held across the
// tracker call.
fn nb_tracker(b: &Nb) -> Rc<dyn SymbolTracker> {
    nb_ctx(b).borrow().tracker.clone()
}

// PORT: Go `b.e`. Node creation uses `b.e.factory` for Go `b.f`, since the
// printer NodeFactory derefs to the same `ast::NodeFactory`.
fn nb_e(b: &Nb) -> Rc<EmitContext> {
    b.borrow().e.clone()
}

// PORT: Go `modulespecifiers.CountPathComponents`. The modulespecifiers
// package is not ported; this is the full Go body.
// Go: modulespecifiers/compare.go:7 CountPathComponents
fn count_path_components(path: &str) -> i32 {
    let initial = if path.starts_with("./") { 2 } else { 0 };
    path[initial..].matches('/').count() as i32
}

// Go: checker/nodebuilderimpl.go:1081 sortedSymbolNamePair
struct SortedSymbolNamePair {
    sym: SymbolId,
    name: String,
}

// Go: checker/nodebuilderimpl.go:1858 SignatureToSignatureDeclarationOptions
#[derive(Default)]
pub struct SignatureToSignatureDeclarationOptions {
    pub modifiers: Vec<Node>,
    pub name: Node,
    pub question_token: Node,
}

// Go: checker/nodebuilderimpl.go:1172 canHaveModuleSpecifier
pub fn can_have_module_specifier(node: Node) -> bool {
    if node.is_nil() {
        return false;
    }
    matches!(
        node.kind(),
        SyntaxKind::VariableDeclaration
            | SyntaxKind::BindingElement
            | SyntaxKind::ImportDeclaration
            | SyntaxKind::ExportDeclaration
            | SyntaxKind::ImportEqualsDeclaration
            | SyntaxKind::ImportClause
            | SyntaxKind::NamespaceExport
            | SyntaxKind::NamespaceImport
            | SyntaxKind::ExportSpecifier
            | SyntaxKind::ImportSpecifier
            | SyntaxKind::ImportType
    )
}

// Go: checker/nodebuilderimpl.go:1193 TryGetModuleSpecifierFromDeclaration
// Go: checker/nodebuilderimpl.go:1201 tryGetModuleSpecifierFromDeclarationWorker
// PORT: both are already ported as `try_get_module_specifier_from_declaration`
// in checker/extras.rs. This file calls that function.

// Go: checker/nodebuilderimpl.go:2234 hasTypeAnnotation
pub fn has_type_annotation(declaration: Node) -> bool {
    if declaration.is_nil() || declaration.type_().is_nil() {
        return false;
    }
    // Type alias declarations have a .Type() that is their type definition, not a type annotation on a value.
    // Exclude them so callers don't mistake them for annotated value declarations.
    if is_type_alias_declaration(declaration) || is_js_type_alias_declaration(declaration) {
        return false;
    }
    true
}

impl Checker {
    // Go: checker/nodebuilderimpl.go:1036 lookupTypeParameterNodes
    // PORT: `typeParameterSymbolList` is keyed by `SymbolId` for Go
    // `ast.GetSymbolId(symbol)`. The two are one to one.
    pub fn lookup_type_parameter_nodes(
        &mut self,
        b: &Nb,
        chain: &[SymbolId],
        index: i32,
    ) -> NodeList {
        debug_assert!(!chain.is_empty() && 0 <= index && (index as usize) < chain.len());
        let symbol = chain[index as usize];
        // Go keys the list by `ast.GetSymbolId(symbol)`, which gives the
        // symbol its id (`ValueSymbolLinkStore`).
        get_symbol_id(&self.symbols, symbol);
        {
            let ctx = nb_ctx(b);
            let mut ctx = ctx.borrow_mut();
            if ctx.type_parameter_symbol_list.has(&symbol) {
                return NodeList::NIL;
            }
            ctx.type_parameter_symbol_list.add(symbol);
        }

        if nb_flags(b).intersects(NodeBuilderFlags::WRITE_TYPE_PARAMETERS_IN_QUALIFIED_NAME)
            && (index as usize) < chain.len() - 1
        {
            let type_argument_nodes =
                self.lookup_instantiated_type_argument_nodes(b, chain, index as usize);
            if type_argument_nodes.is_some() {
                return type_argument_nodes;
            }
            let type_parameter_nodes =
                self.type_parameters_to_type_parameter_declarations(b, symbol);
            if !type_parameter_nodes.is_empty() {
                return nb_e(b).factory.new_node_list(&type_parameter_nodes);
            }
            return NodeList::NIL;
        }

        NodeList::NIL
    }

    // Go: checker/nodebuilderimpl.go:1061 lookupSymbolChain
    pub fn lookup_symbol_chain(
        &mut self,
        b: &Nb,
        symbol: SymbolId,
        meaning: SymbolFlags,
        yield_module_symbol: bool,
    ) -> Vec<SymbolId> {
        let enclosing = nb_enclosing_declaration(b);
        nb_tracker(b).track_symbol(self, symbol, enclosing, meaning);
        self.lookup_symbol_chain_worker(b, symbol, meaning, yield_module_symbol)
    }

    // Go: checker/nodebuilderimpl.go:1066 lookupSymbolChainWorker
    pub fn lookup_symbol_chain_worker(
        &mut self,
        b: &Nb,
        symbol: SymbolId,
        meaning: SymbolFlags,
        yield_module_symbol: bool,
    ) -> Vec<SymbolId> {
        // Try to get qualified name if the symbol is not a type parameter and there is an enclosing declaration.
        let mut chain: Vec<SymbolId> = Vec::new();
        let is_type_parameter = self
            .sym(symbol)
            .flags
            .intersects(SymbolFlags::TYPE_PARAMETER);
        let (has_enclosing, flags, internal_flags) = {
            let ctx = nb_ctx(b);
            let ctx = ctx.borrow();
            (
                ctx.enclosing_declaration.is_some(),
                ctx.flags,
                ctx.internal_flags,
            )
        };
        if !is_type_parameter
            && (has_enclosing || flags.intersects(NodeBuilderFlags::USE_FULLY_QUALIFIED_TYPE))
            && !internal_flags.intersects(InternalNodeBuilderFlags::DO_NOT_INCLUDE_SYMBOL_CHAIN)
        {
            let res = self.get_symbol_chain(
                b,
                symbol,
                meaning,
                true, /*endOfChain*/
                yield_module_symbol,
            );
            chain = res;
            debug_assert!(!chain.is_empty());
        } else {
            chain.push(symbol);
        }
        chain
    }

    // Go: checker/nodebuilderimpl.go:1087 getSymbolChain
    /// `end_of_chain` is false for recursive calls. Non-recursive calls always output something.
    fn get_symbol_chain(
        &mut self,
        b: &Nb,
        symbol: SymbolId,
        meaning: SymbolFlags,
        end_of_chain: bool,
        yield_module_symbol: bool,
    ) -> Vec<SymbolId> {
        let enclosing = nb_enclosing_declaration(b);
        let use_only_external_aliasing =
            nb_flags(b).intersects(NodeBuilderFlags::USE_ONLY_EXTERNAL_ALIASING);
        let mut accessible_symbol_chain = self.get_accessible_symbol_chain(
            symbol,
            enclosing,
            meaning,
            use_only_external_aliasing,
        );
        let mut qualifier_meaning = meaning;
        if accessible_symbol_chain.len() > 1 {
            qualifier_meaning = get_qualified_left_meaning(meaning);
        }
        if accessible_symbol_chain.is_empty()
            || self.needs_qualification(accessible_symbol_chain[0], enclosing, qualifier_meaning)
        {
            // Go up and add our parent.
            let root = if !accessible_symbol_chain.is_empty() {
                accessible_symbol_chain[0]
            } else {
                symbol
            };
            let parents = self.get_containers_of_symbol(root, enclosing, meaning);
            if !parents.is_empty() {
                let mut parent_specifiers: Vec<SortedSymbolNamePair> =
                    Vec::with_capacity(parents.len());
                for parent in parents {
                    let declarations = self.sym(parent).declarations.clone();
                    if declarations
                        .iter()
                        .any(|&d| has_non_global_augmentation_external_module_symbol(d))
                    {
                        let name = self
                            .get_specifier_for_module_symbol(b, parent, ResolutionMode::NONE)
                            .specifier;
                        parent_specifiers.push(SortedSymbolNamePair { sym: parent, name });
                    } else {
                        parent_specifiers.push(SortedSymbolNamePair {
                            sym: parent,
                            name: String::new(),
                        });
                    }
                }
                // Go: checker/nodebuilderimpl.go:1108 slices.SortStableFunc(parentSpecifiers, b.sortByBestName)
                crate::gostd::slices::sort_stable_func(&mut parent_specifiers, |x, y| {
                    self.sort_by_best_name(x, y)
                });
                for pair in &parent_specifiers {
                    let parent = pair.sym;
                    let parent_chain = self.get_symbol_chain(
                        b,
                        parent,
                        get_qualified_left_meaning(meaning),
                        false,
                        yield_module_symbol,
                    );
                    if !parent_chain.is_empty() {
                        let exports = self.sym(parent).exports;
                        if exports.is_some() {
                            let exported = self
                                .symbols
                                .get(exports, INTERNAL_SYMBOL_NAME_EXPORT_EQUALS);
                            if exported.is_some()
                                && self
                                    .get_symbol_if_same_reference(exported, symbol)
                                    .is_some()
                            {
                                // parentChain root _is_ symbol - symbol is a module export=, so it kinda looks like it's own parent
                                // No need to lookup an alias for the symbol in itself
                                accessible_symbol_chain = parent_chain;
                                break;
                            }
                        }
                        let mut next_syms = accessible_symbol_chain;
                        if next_syms.is_empty() {
                            let mut fallback =
                                self.get_alias_for_symbol_in_container(parent, symbol);
                            if fallback.is_nil() {
                                fallback = symbol;
                            }
                            next_syms.push(fallback);
                        }
                        let mut joined = parent_chain;
                        joined.extend(next_syms);
                        accessible_symbol_chain = joined;
                        break;
                    }
                }
            }
        }
        if !accessible_symbol_chain.is_empty() {
            return accessible_symbol_chain;
        }
        let symbol_flags = self.sym(symbol).flags;
        // If this is the last part of outputting the symbol, always output. The cases apply only to parent symbols.
        if end_of_chain
            // If a parent symbol is an anonymous type, don't write it.
            || !symbol_flags.intersects(SymbolFlags::TYPE_LITERAL | SymbolFlags::OBJECT_LITERAL)
        {
            // If a parent symbol is an external module, don't write it. (We prefer just `x` vs `"foo/bar".x`.)
            if !end_of_chain
                && !yield_module_symbol
                && self
                    .sym(symbol)
                    .declarations
                    .iter()
                    .any(|&d| has_non_global_augmentation_external_module_symbol(d))
            {
                return Vec::new();
            }
            return vec![symbol];
        }
        Vec::new()
    }

    // Go: checker/nodebuilderimpl.go:1153 sortByBestName
    // PORT: the Go receiver is NodeBuilderImpl but only `b_.ch` is used, so
    // this is a Checker method without `b`.
    fn sort_by_best_name(&mut self, a: &SortedSymbolNamePair, b: &SortedSymbolNamePair) -> i32 {
        let specifier_a = &a.name;
        let specifier_b = &b.name;
        if !specifier_a.is_empty() && !specifier_b.is_empty() {
            let is_b_relative = crate::frontend::tspath::path_is_relative(specifier_b);
            if crate::frontend::tspath::path_is_relative(specifier_a) == is_b_relative {
                // Both relative or both non-relative, sort by number of parts
                return count_path_components(specifier_a) - count_path_components(specifier_b);
            }
            if is_b_relative {
                // A is non-relative, B is relative: prefer A
                return -1;
            }
            // A is relative, B is non-relative: prefer B
            return 1;
        }
        self.compare_symbols(a.sym, b.sym) // must sort symbols for stable ordering
    }

    // Go: checker/nodebuilderimpl.go:1249 getSpecifierForModuleSymbol
    // PORT: the specifier cache key is `(path, mode)` for Go
    // `module.ModeAwareCacheKey{Name, Mode}`.
    pub fn get_specifier_for_module_symbol(
        &mut self,
        b: &Nb,
        symbol: SymbolId,
        override_import_mode: ResolutionMode,
    ) -> ModuleSpecifierResult {
        let e = nb_e(b);
        let enclosing_declaration = e.most_original(nb_enclosing_declaration(b));
        let mut original_module_specifier = Node::NIL;
        if can_have_module_specifier(enclosing_declaration) {
            original_module_specifier =
                try_get_module_specifier_from_declaration(enclosing_declaration);
        }
        let mut original_import_attributes_type = TypeId::NIL;
        if original_module_specifier.is_some() {
            original_import_attributes_type =
                self.get_import_attributes_type_for_module_specifier(original_module_specifier);
        }

        let mut file = get_declaration_of_kind(&self.symbols, symbol, SyntaxKind::SourceFile);
        if file.is_nil() {
            let declarations = self.sym(symbol).declarations.clone();
            let mut equivalent_symbol = SymbolId::NIL;
            for d in declarations {
                let s = self.get_file_symbol_if_file_symbol_export_equals_container(d, symbol);
                if s.is_some() {
                    equivalent_symbol = s;
                    break;
                }
            }
            if equivalent_symbol.is_some() {
                file = get_declaration_of_kind(
                    &self.symbols,
                    equivalent_symbol,
                    SyntaxKind::SourceFile,
                );
            }
        }

        let symbol_name = self.sym(symbol).name.clone();
        if file.is_nil() {
            let declaration = self
                .sym(symbol)
                .declarations
                .iter()
                .copied()
                .find(|&d| is_module_with_string_literal_name(d))
                .unwrap_or(Node::NIL);
            if declaration.is_some() {
                let specifier = declaration.name().text().to_string();
                if original_import_attributes_type.is_some()
                    && self.module_specifier_resolves_to_symbol(
                        b,
                        &specifier,
                        original_import_attributes_type,
                        symbol,
                    )
                {
                    return ModuleSpecifierResult {
                        specifier,
                        import_attributes_type: original_import_attributes_type,
                    };
                }
                let import_attributes_type = self.get_type_of_module_import_attributes(symbol);
                return ModuleSpecifierResult {
                    specifier,
                    import_attributes_type,
                };
            }
            if let Some(specifier) = try_get_ambient_module_name_from_symbol_name(&symbol_name) {
                return ModuleSpecifierResult {
                    specifier: specifier.to_string(),
                    import_attributes_type: TypeId::NIL,
                };
            }
        }
        let enclosing_file = nb_enclosing_file(b);
        if enclosing_file.is_nil() {
            if let Some(specifier) = try_get_ambient_module_name_from_symbol_name(&symbol_name) {
                return ModuleSpecifierResult {
                    specifier: specifier.to_string(),
                    import_attributes_type: TypeId::NIL,
                };
            }
            return ModuleSpecifierResult {
                specifier: source_file_file_name(get_source_file_of_module(&self.symbols, symbol))
                    .to_string(),
                import_attributes_type: TypeId::NIL,
            };
        }

        let context_file = enclosing_file;
        let mut resolution_mode = override_import_mode;
        if resolution_mode == ResolutionMode::NONE && original_module_specifier.is_some() {
            resolution_mode = get_mode_for_usage_location(context_file, original_module_specifier);
        } else if resolution_mode == ResolutionMode::NONE && context_file.is_some() {
            resolution_mode = get_default_resolution_mode_for_file(context_file);
        }
        let cache_key = (source_file_info(context_file).path.clone(), resolution_mode);
        let cached = {
            let mut nb = b.borrow_mut();
            let links = nb.symbol_links.get(symbol);
            let cache = links.specifier_cache.get_or_insert_with(FxHashMap::default);
            cache.get(&cache_key).cloned()
        };
        if let Some(result) = cached {
            return self.module_specifier_result_for_symbol(
                b,
                result,
                original_import_attributes_type,
                symbol,
            );
        }
        // For declaration bundles, we need to generate absolute paths relative to the common source dir for imports,
        // just like how the declaration emitter does for the ambient module declarations - we can easily accomplish this
        // using the `baseUrl` compiler option (which we would otherwise never use in declaration emit) and a non-relative
        // specifier preference
        // PORT: Go uses `b.ctx.host`, which is the program. The program state
        // is global here, so `ProgramHost` has no fields.
        let module_specifiers_result = {
            use crate::modulespecifiers as ms;
            let host = ms::ProgramHost;
            let specifier_compiler_options = self.compiler_options;
            let specifier_pref = ms::ImportModuleSpecifierPreference::ProjectRelative;
            let mut ending_pref = ms::ImportModuleSpecifierEndingPreference::None;
            if resolution_mode == ResolutionMode::ESM {
                ending_pref = ms::ImportModuleSpecifierEndingPreference::Js;
            }
            ms::get_module_specifiers(
                symbol,
                self,
                specifier_compiler_options,
                &context_file,
                &host,
                &ms::UserPreferences {
                    import_module_specifier_preference: specifier_pref,
                    import_module_specifier_ending: ending_pref,
                    ..Default::default()
                },
                ms::ModuleSpecifierOptions {
                    override_import_mode,
                },
                false, /*forAutoImports*/
            )
        };
        debug_assert!(!module_specifiers_result.specifiers.is_empty());
        let mut result = ModuleSpecifierResult {
            specifier: module_specifiers_result.specifiers[0].clone(),
            import_attributes_type: TypeId::NIL,
        };
        if module_specifiers_result.ambient_module_symbol.is_some() {
            result.import_attributes_type = self.get_type_of_module_import_attributes(
                module_specifiers_result.ambient_module_symbol,
            );
        }
        b.borrow_mut()
            .symbol_links
            .get(symbol)
            .specifier_cache
            .get_or_insert_with(FxHashMap::default)
            .insert(cache_key, result.clone());
        self.module_specifier_result_for_symbol(b, result, original_import_attributes_type, symbol)
    }

    // Go: checker/nodebuilderimpl.go:1340 moduleSpecifierResultForSymbol
    pub fn module_specifier_result_for_symbol(
        &mut self,
        b: &Nb,
        mut result: ModuleSpecifierResult,
        import_attributes_type: TypeId,
        symbol: SymbolId,
    ) -> ModuleSpecifierResult {
        if import_attributes_type.is_some()
            && self.module_specifier_resolves_to_symbol(
                b,
                &result.specifier,
                import_attributes_type,
                symbol,
            )
        {
            result.import_attributes_type = import_attributes_type;
        }
        result
    }

    // Go: checker/nodebuilderimpl.go:1347 moduleSpecifierResolvesToSymbol
    pub fn module_specifier_resolves_to_symbol(
        &mut self,
        b: &Nb,
        specifier: &str,
        import_attributes_type: TypeId,
        symbol: SymbolId,
    ) -> bool {
        let mut location = nb_enclosing_declaration(b);
        let enclosing_file = nb_enclosing_file(b);
        if location.is_nil() && enclosing_file.is_some() {
            location = enclosing_file;
        }
        if location.is_nil() {
            return false;
        }
        let resolved = self.resolve_external_module(
            location,
            specifier,
            None,
            Node::NIL,
            false, /*isForAugmentation*/
            import_attributes_type,
        );
        resolved.is_some() && self.get_merged_symbol(resolved) == self.get_merged_symbol(symbol)
    }

    // Go: checker/nodebuilderimpl.go:1359 createImportAttributesForModuleSpecifier
    pub fn create_import_attributes_for_module_specifier(
        &mut self,
        b: &Nb,
        result: &ModuleSpecifierResult,
        import_mode_override: ResolutionMode,
    ) -> Node {
        let is_empty_attributes_type = result.import_attributes_type.is_nil()
            || self.is_empty_object_type(result.import_attributes_type);
        if is_empty_attributes_type && import_mode_override == ResolutionMode::NONE {
            return Node::NIL;
        }

        let mut properties: Vec<SymbolId> = Vec::new();
        if !is_empty_attributes_type {
            properties = self
                .get_properties_of_type(result.import_attributes_type)
                .to_vec();
        }
        // PORT: Go `strings.Compare` on the names; the port form compares the
        // same way for these names.
        crate::gostd::slices::stable_sort_by(&mut properties, |&x, &y| {
            self.sym(x).name.as_str().cmp(self.sym(y).name.as_str())
        });
        let e = nb_e(b);
        let f = &e.factory;
        let mut attributes: Vec<Node> = Vec::with_capacity(properties.len() + 1);
        if import_mode_override != ResolutionMode::NONE {
            let resolution_mode = if import_mode_override == ResolutionMode::ESM {
                "import"
            } else {
                "require"
            };
            let name = self.nb_new_string_literal(b, "resolution-mode");
            let value = self.nb_new_string_literal(b, resolution_mode);
            attributes.push(f.new_import_attribute(name, value));
            nb_add_approximate_length(
                b,
                (go_len("resolution-mode") + go_len(resolution_mode) + 6) as i32,
            ); // `"resolution-mode": "value"`
        }
        for property in properties {
            let name = self.sym(property).name.as_str().to_string();
            let property_type = self.get_type_of_symbol(property);
            if !self
                .ty(property_type)
                .flags
                .intersects(TypeFlags::STRING_LITERAL)
            {
                continue;
            }
            let value = self.get_string_literal_value(property_type);
            let name_node;
            if is_identifier_text(&name, LanguageVariant::STANDARD) {
                name_node = f.new_identifier(&name);
            } else {
                name_node = self.nb_new_string_literal(b, &name);
                nb_add_approximate_length(b, 2);
            }
            let value_node = self.nb_new_string_literal(b, &value);
            attributes.push(f.new_import_attribute(name_node, value_node));
            nb_add_approximate_length(b, (go_len(&name) + go_len(&value) + 4) as i32); // `name: "value"`
        }
        if attributes.is_empty() {
            return Node::NIL;
        }
        nb_add_approximate_length(b, 16 + 2 * (attributes.len() as i32 - 1)); // `, { with: { , ... , } }`
        f.new_import_attributes(SyntaxKind::WithKeyword, f.new_node_list(&attributes), false)
    }

    // Go: checker/nodebuilderimpl.go:1401 typeParameterToDeclarationWithConstraint
    pub fn type_parameter_to_declaration_with_constraint(
        &mut self,
        b: &Nb,
        type_parameter: TypeId,
        constraint_node: Node,
    ) -> Node {
        let restore_flags = self.save_restore_flags(b);
        nb_clear_flags(b, NodeBuilderFlags::WRITE_TYPE_PARAMETERS_IN_QUALIFIED_NAME); // Avoids potential infinite loop when building for a claimspace with a generic
        let e = nb_e(b);
        let modifier_flags = self.get_type_parameter_modifiers(type_parameter);
        let modifiers = create_modifiers_from_modifier_flags(modifier_flags, &mut |k| {
            e.factory.new_modifier(k)
        });
        let mut modifiers_list = ModifierList::NIL;
        if !modifiers.is_empty() {
            modifiers_list = e.factory.new_modifier_list(&modifiers);
        }
        let name = self.type_parameter_to_name(b, type_parameter);
        let default_parameter = self.get_default_from_type_parameter(type_parameter);
        let mut default_parameter_declaration_node = Node::NIL;
        if default_parameter.is_some() {
            default_parameter_declaration_node = self.type_to_type_node(b, default_parameter);
        }
        restore_flags();
        e.factory.new_type_parameter_declaration(
            modifiers_list,
            name,
            constraint_node,
            Node::NIL, // expression
            default_parameter_declaration_node,
        )
    }

    // Go: checker/nodebuilderimpl.go:1434 setTextRange
    /// Unlike the utilities `setTextRange`, this checks if the `location` we're trying to set on `range` is within the
    /// same file as the active context. If not, the range is not applied. This prevents us from copying ranges across files,
    /// which will confuse the node printer (as it assumes all node ranges are within the current file).
    /// Additionally, if `range` _isn't synthetic_, or isn't in the current file, it will _copy_ it to _remove_ its' position
    /// information.
    ///
    /// It also calls `setOriginalNode` to setup a `.original` pointer, since you basically *always* want these in the node builder.
    pub fn set_text_range(&mut self, b: &Nb, mut range: Node, location: Node) -> Node {
        if range.is_nil() {
            return range;
        }
        let e = nb_e(b);
        let enclosing_file = nb_enclosing_file(b);
        if !node_is_synthesized(range)
            || !range.flags().intersects(NodeFlags::SYNTHESIZED)
            || enclosing_file.is_nil()
            || enclosing_file != get_source_file_of_node(e.most_original(range))
        {
            let original = range;
            range = e.factory.clone_node(range); // if `range` is synthesized or originates in another file, copy it so it definitely has synthetic positions
            set_node_loc(range, TextRange::new(-1, -1));
            let mut nb = b.borrow_mut();
            if let Some(&symbol) = nb.id_to_symbol.get(&original) {
                nb.id_to_symbol.insert(range, symbol);
            }
        }
        if range == location || location.is_nil() {
            return range;
        }
        // Don't overwrite the original node if `range` has an `original` node that points either directly or indirectly to `location`
        let mut original = e.original(range);
        while original.is_some() && original != location {
            original = e.original(original);
        }
        if original.is_nil() {
            e.set_original_ex(range, location, true);
        }

        // only set positions if range comes from the same file since copying text across files isn't supported by the emitter
        if enclosing_file.is_some()
            && enclosing_file == get_source_file_of_node(e.most_original(location))
        {
            set_node_loc(range, location.loc());
            return range;
        } else {
            set_node_loc(range, TextRange::new(-1, -1));
        }
        range
    }

    // Go: checker/nodebuilderimpl.go:1468 typeParameterShadowsOtherTypeParameterInScope
    fn type_parameter_shadows_other_type_parameter_in_scope(
        &mut self,
        b: &Nb,
        name: &str,
        type_parameter: TypeId,
    ) -> bool {
        let enclosing = nb_enclosing_declaration(b);
        let result = self.resolve_name(enclosing, name, SymbolFlags::TYPE, None, false, false);
        if result.is_some()
            && self
                .sym(result)
                .flags
                .intersects(SymbolFlags::TYPE_PARAMETER)
        {
            return result != self.ty(type_parameter).symbol;
        }
        false
    }

    // Go: checker/nodebuilderimpl.go:1476 typeParameterToName
    // PORT: `typeParameterNames` is keyed by `TypeId` for Go `typeParameter.id`.
    pub fn type_parameter_to_name(&mut self, b: &Nb, type_parameter: TypeId) -> Node {
        if nb_flags(b).intersects(NodeBuilderFlags::GENERATE_NAMES_FOR_SHADOWED_TYPE_PARAMS) {
            if let Some(cached) = nb_ctx(b)
                .borrow()
                .type_parameter_names
                .get(&type_parameter)
                .copied()
            {
                return cached;
            }
        }
        let tp_symbol = self.ty(type_parameter).symbol;
        let mut result = self.symbol_to_name(
            b,
            tp_symbol,
            SymbolFlags::TYPE,
            true, /*expectsIdentifier*/
        );
        if !is_identifier(result) {
            return nb_e(b).factory.new_identifier("(Missing type parameter)");
        }
        if tp_symbol.is_some() && !self.sym(tp_symbol).declarations.is_empty() {
            let decl = self.sym(tp_symbol).declarations[0];
            if decl.is_some() && is_type_parameter_declaration(decl) {
                result = self.set_text_range(b, result, decl.name());
            }
        }
        if nb_flags(b).intersects(NodeBuilderFlags::GENERATE_NAMES_FOR_SHADOWED_TYPE_PARAMS) {
            let raw_text = result.text().to_string();
            let mut i: i32 = nb_ctx(b)
                .borrow()
                .type_parameter_names_by_text_next_name_count
                .get(&raw_text)
                .copied()
                .unwrap_or(0);
            let mut text = raw_text.clone();

            loop {
                let taken = nb_ctx(b).borrow().type_parameter_names_by_text.has(&text);
                if !taken
                    && !self.type_parameter_shadows_other_type_parameter_in_scope(
                        b,
                        &text,
                        type_parameter,
                    )
                {
                    break;
                }
                i += 1;
                text = format!("{raw_text}_{i}");
            }

            if text != raw_text {
                // !!! TODO: smuggle type arguments out
                // const typeArguments = getIdentifierTypeArguments(result);
                result = self.nb_new_identifier(b, &text, tp_symbol);
                // setIdentifierTypeArguments(result, typeArguments);
            }

            // avoiding iterations of the above loop turns out to be worth it when `i` starts to get large, so we cache the max
            // `i` we've used thus far, to save work later
            let ctx = nb_ctx(b);
            let mut ctx = ctx.borrow_mut();
            ctx.type_parameter_names_by_text_next_name_count
                .set(raw_text, i);
            ctx.type_parameter_names.set(type_parameter, result);
            ctx.type_parameter_names_by_text.add(text);
        }

        result
    }

    // Go: checker/nodebuilderimpl.go:1522 isMappedTypeHomomorphic
    fn is_mapped_type_homomorphic(&mut self, mapped: TypeId) -> bool {
        self.get_homomorphic_type_variable(mapped).is_some()
    }

    // Go: checker/nodebuilderimpl.go:1526 isHomomorphicMappedTypeWithNonHomomorphicInstantiation
    fn is_homomorphic_mapped_type_with_non_homomorphic_instantiation(
        &mut self,
        mapped: TypeId,
    ) -> bool {
        let target = self.ty(mapped).as_mapped_type().object.target;
        target.is_some()
            && !self.is_mapped_type_homomorphic(mapped)
            && self.is_mapped_type_homomorphic(target)
    }

    // Go: checker/nodebuilderimpl.go:1530 createMappedTypeNodeFromType
    pub fn create_mapped_type_node_from_type(&mut self, b: &Nb, t: TypeId) -> Node {
        debug_assert!(self.ty(t).flags.intersects(TypeFlags::OBJECT));
        let e = nb_e(b);
        let (declaration, mapped_mapper) = {
            let mapped = self.ty(t).as_mapped_type();
            (mapped.declaration, mapped.object.mapper)
        };
        let mut readonly_token = Node::NIL;
        if declaration.readonly_token().is_some() {
            readonly_token = e.factory.new_token(declaration.readonly_token().kind());
        }
        let mut question_token = Node::NIL;
        if declaration.question_token().is_some() {
            question_token = e.factory.new_token(declaration.question_token().kind());
        }
        let appropriate_constraint_type_node;
        let mut new_type_variable = Node::NIL;
        let mut template_type = self.get_template_type_from_mapped_type(t);
        let type_parameter = self.get_type_parameter_from_mapped_type(t);

        // If the mapped type isn't `keyof` constraint-declared, _but_ still has modifiers preserved, and its naive instantiation won't preserve modifiers because its constraint isn't `keyof` constrained, we have work to do
        // PORT: the Go `&&` chain is split into ifs to keep its order without nested borrows.
        let mut needs_modifier_preserving_wrapper =
            !self.is_mapped_type_with_keyof_constraint_declaration(t);
        if needs_modifier_preserving_wrapper {
            let modifiers_type = self.get_modifiers_type_from_mapped_type(t);
            needs_modifier_preserving_wrapper =
                !self.ty(modifiers_type).flags.intersects(TypeFlags::UNKNOWN);
        }
        if needs_modifier_preserving_wrapper {
            needs_modifier_preserving_wrapper =
                nb_flags(b).intersects(NodeBuilderFlags::GENERATE_NAMES_FOR_SHADOWED_TYPE_PARAMS);
        }
        if needs_modifier_preserving_wrapper {
            let constraint_type = self.get_constraint_type_from_mapped_type(t);
            let mut keyof_constrained = self
                .ty(constraint_type)
                .flags
                .intersects(TypeFlags::TYPE_PARAMETER);
            if keyof_constrained {
                let constraint_type = self.get_constraint_type_from_mapped_type(t);
                let constraint = self.get_constraint_of_type_parameter(constraint_type);
                keyof_constrained = self.ty(constraint).flags.intersects(TypeFlags::INDEX);
            }
            needs_modifier_preserving_wrapper = !keyof_constrained;
        }

        if self.is_mapped_type_with_keyof_constraint_declaration(t) {
            // We have a { [P in keyof T]: X }
            // We do this to ensure we retain the toplevel keyof-ness of the type which may be lost due to keyof distribution during `getConstraintTypeFromMappedType`
            if nb_flags(b).intersects(NodeBuilderFlags::GENERATE_NAMES_FOR_SHADOWED_TYPE_PARAMS)
                && self.is_homomorphic_mapped_type_with_non_homomorphic_instantiation(t)
            {
                let new_symbol = self.new_symbol(SymbolFlags::TYPE_PARAMETER, "T");
                let new_constraint_param = self.new_type_parameter(new_symbol);
                let name = self.type_parameter_to_name(b, new_constraint_param);
                let target = self.ty(t).target();
                new_type_variable = e.factory.new_type_reference_node(name, NodeList::NIL);
                let target_template = self.get_template_type_from_mapped_type(target);
                let target_type_parameter = self.get_type_parameter_from_mapped_type(target);
                let target_modifiers = self.get_modifiers_type_from_mapped_type(target);
                let mapper = self.new_type_mapper(
                    &[target_type_parameter, target_modifiers],
                    &[type_parameter, new_constraint_param],
                );
                template_type = self.instantiate_type(target_template, mapper);
            }
            let mut index_target = new_type_variable;
            if index_target.is_nil() {
                let modifiers_type = self.get_modifiers_type_from_mapped_type(t);
                index_target = self.type_to_type_node(b, modifiers_type);
            }
            appropriate_constraint_type_node = e
                .factory
                .new_type_operator_node(SyntaxKind::KeyOfKeyword, index_target);
        } else if needs_modifier_preserving_wrapper {
            // So, step 1: new type variable
            let new_symbol = self.new_symbol(SymbolFlags::TYPE_PARAMETER, "T");
            let new_param = self.new_type_parameter(new_symbol);
            let name = self.type_parameter_to_name(b, new_param);
            new_type_variable = e.factory.new_type_reference_node(name, NodeList::NIL);
            // step 2: make that new type variable itself the constraint node, making the mapped type `{[K in T_1]: Template}`
            appropriate_constraint_type_node = new_type_variable;
        } else {
            let constraint_type = self.get_constraint_type_from_mapped_type(t);
            appropriate_constraint_type_node = self.type_to_type_node(b, constraint_type);
        }

        // nameType and templateType nodes have to be in the new scope
        // PORT: Go passes nil expandedParams, originalParameters and mapper;
        // empty slices and MapperId::NIL are the Rust nil values.
        let scope_type_parameter = self.get_type_parameter_from_mapped_type(t);
        let cleanup = self.enter_new_scope(
            b,
            declaration,
            &[],
            &[scope_type_parameter],
            &[],
            MapperId::NIL,
        );
        let type_parameter_declaration_node = self.type_parameter_to_declaration_with_constraint(
            b,
            type_parameter,
            appropriate_constraint_type_node,
        );
        let mut name_type_node = Node::NIL;
        if declaration.name_type().is_some() {
            let name_type = self.get_name_type_from_mapped_type(t);
            name_type_node = self.type_to_type_node(b, name_type);
        }
        let is_optional = self
            .get_mapped_type_modifiers(t)
            .intersects(MappedTypeModifiers::INCLUDE_OPTIONAL);
        let without_missing = self.remove_missing_type(template_type, is_optional);
        let template_type_node = self.type_to_type_node(b, without_missing);
        cleanup(self);
        let result = e.factory.new_mapped_type_node(
            readonly_token,
            type_parameter_declaration_node,
            name_type_node,
            question_token,
            template_type_node,
            NodeList::NIL,
        );
        nb_add_approximate_length(b, 10);
        e.add_emit_flags(result, EmitFlags::SINGLE_LINE);

        if nb_flags(b).intersects(NodeBuilderFlags::GENERATE_NAMES_FOR_SHADOWED_TYPE_PARAMS)
            && self.is_homomorphic_mapped_type_with_non_homomorphic_instantiation(t)
        {
            // homomorphic mapped type with a non-homomorphic naive inlining
            // wrap it with a conditional like `SomeModifiersType extends infer U ? {..the mapped type...} : never` to ensure the resulting
            // type stays homomorphic

            let constraint_operand = declaration.type_parameter().constraint().type_();
            let mut raw_constraint_type_from_declaration =
                self.nb_get_type_from_type_node(b, constraint_operand, false);
            if raw_constraint_type_from_declaration.is_some() {
                raw_constraint_type_from_declaration =
                    self.get_constraint_of_type_parameter(raw_constraint_type_from_declaration);
            }
            if raw_constraint_type_from_declaration.is_nil() {
                raw_constraint_type_from_declaration = self.unknown_type;
            }
            let original_constraint =
                self.instantiate_type(raw_constraint_type_from_declaration, mapped_mapper);

            let mut original_constraint_node = Node::NIL;
            if !self
                .ty(original_constraint)
                .flags
                .intersects(TypeFlags::UNKNOWN)
            {
                original_constraint_node = self.type_to_type_node(b, original_constraint);
            }

            let modifiers_type = self.get_modifiers_type_from_mapped_type(t);
            let check_type = self.type_to_type_node(b, modifiers_type);
            let infer_name = e.factory.clone_node(new_type_variable.type_name());
            return e.factory.new_conditional_type_node(
                check_type,
                e.factory
                    .new_infer_type_node(e.factory.new_type_parameter_declaration(
                        ModifierList::NIL,
                        infer_name,
                        original_constraint_node,
                        Node::NIL,
                        Node::NIL,
                    )),
                result,
                e.factory.new_keyword_type_node(SyntaxKind::NeverKeyword),
            );
        } else if needs_modifier_preserving_wrapper {
            // and step 3: once the mapped type is reconstructed, create a `ConstraintType extends infer T_1 extends keyof ModifiersType ? {[K in T_1]: Template} : never`
            // subtly different from the `keyof` constraint case, by including the `keyof` constraint on the `infer` type parameter, it doesn't rely on the constraint type being itself
            // constrained to a `keyof` type to preserve its modifier-preserving behavior. This is all basically because we preserve modifiers for a wider set of mapped types than
            // just homomorphic ones.
            let constraint_type = self.get_constraint_type_from_mapped_type(t);
            let check_type = self.type_to_type_node(b, constraint_type);
            let infer_name = e.factory.clone_node(new_type_variable.type_name());
            let modifiers_type = self.get_modifiers_type_from_mapped_type(t);
            let modifiers_type_node = self.type_to_type_node(b, modifiers_type);
            return e.factory.new_conditional_type_node(
                check_type,
                e.factory.new_infer_type_node(
                    e.factory.new_type_parameter_declaration(
                        ModifierList::NIL,
                        infer_name,
                        e.factory
                            .new_type_operator_node(SyntaxKind::KeyOfKeyword, modifiers_type_node),
                        Node::NIL,
                        Node::NIL,
                    ),
                ),
                result,
                e.factory.new_keyword_type_node(SyntaxKind::NeverKeyword),
            );
        }

        result
    }

    // Go: checker/nodebuilderimpl.go:1646 typePredicateToTypePredicateNode
    pub fn type_predicate_to_type_predicate_node(
        &mut self,
        b: &Nb,
        predicate: TypePredicateId,
    ) -> Node {
        let e = nb_e(b);
        let (kind, parameter_name_text, pt) = {
            let p = self.pred(predicate);
            (p.kind, p.parameter_name.clone(), p.t)
        };
        let mut asserts_modifier = Node::NIL;
        if kind == TypePredicateKind::ASSERTS_IDENTIFIER || kind == TypePredicateKind::ASSERTS_THIS
        {
            asserts_modifier = e.factory.new_token(SyntaxKind::AssertsKeyword);
        }
        let parameter_name;
        if kind == TypePredicateKind::IDENTIFIER || kind == TypePredicateKind::ASSERTS_IDENTIFIER {
            parameter_name = e.factory.new_identifier(parameter_name_text);
            e.add_emit_flags(parameter_name, EmitFlags::NO_ASCII_ESCAPING);
        } else {
            parameter_name = e.factory.new_this_type_node();
        }
        let mut type_node = Node::NIL;
        if pt.is_some() {
            type_node = self.type_to_type_node(b, pt);
        }
        e.factory
            .new_type_predicate_node(asserts_modifier, parameter_name, type_node)
    }

    // Go: checker/nodebuilderimpl.go:1669 typeToTypeNodeHelperWithPossibleReusableTypeNode
    fn type_to_type_node_helper_with_possible_reusable_type_node(
        &mut self,
        b: &Nb,
        t: TypeId,
        type_node: Node,
    ) -> Node {
        if t.is_nil() {
            return nb_e(b)
                .factory
                .new_keyword_type_node(SyntaxKind::AnyKeyword);
        }
        if !self.is_actively_expanding(b)
            && type_node.is_some()
            && self.nb_get_type_from_type_node(b, type_node, false) == t
        {
            let reused = self.try_reuse_existing_node_helper(b, type_node);
            if reused.is_some() {
                self.check_type_expandability(b, t);
                return reused;
            }
        }
        self.type_to_type_node(b, t)
    }

    // Go: checker/nodebuilderimpl.go:1683 typeParameterToDeclaration
    pub fn type_parameter_to_declaration(&mut self, b: &Nb, parameter: TypeId) -> Node {
        let constraint = self.get_constraint_of_type_parameter(parameter);
        let mut constraint_node = Node::NIL;
        if constraint.is_some() {
            let constraint_declaration = self.get_constraint_declaration(parameter);
            constraint_node = self.type_to_type_node_helper_with_possible_reusable_type_node(
                b,
                constraint,
                constraint_declaration,
            );
        }
        self.type_parameter_to_declaration_with_constraint(b, parameter, constraint_node)
    }

    // Go: checker/nodebuilderimpl.go:1692 symbolToTypeParameterDeclarations
    pub fn symbol_to_type_parameter_declarations(&mut self, b: &Nb, symbol: SymbolId) -> Vec<Node> {
        self.type_parameters_to_type_parameter_declarations(b, symbol)
    }

    // Go: checker/nodebuilderimpl.go:1696 typeParametersToTypeParameterDeclarations
    pub fn type_parameters_to_type_parameter_declarations(
        &mut self,
        b: &Nb,
        symbol: SymbolId,
    ) -> Vec<Node> {
        let target_symbol = self.get_target_symbol(symbol);
        let target_flags = self.sym(target_symbol).flags;
        if target_flags.intersects(SymbolFlags::CLASS | SymbolFlags::INTERFACE | SymbolFlags::ALIAS)
        {
            let mut results = Vec::new();
            let params = self.get_local_type_parameters_of_class_or_interface_or_type_alias(symbol);
            for param in params {
                results.push(self.type_parameter_to_declaration(b, param));
            }
            return results;
        } else if target_flags.intersects(SymbolFlags::FUNCTION) {
            let mut results = Vec::new();
            let value_declaration = self.sym(symbol).value_declaration;
            for param in self.get_type_parameters_from_declaration(value_declaration) {
                results.push(self.type_parameter_to_declaration(b, param));
            }
            return results;
        }
        Vec::new()
    }

    // Go: checker/nodebuilderimpl.go:1715 getEffectiveParameterDeclaration
    // PORT: a free function in Go. It reads symbols, so it is a `&self`
    // Checker method here.
    pub fn get_effective_parameter_declaration(&self, symbol: SymbolId) -> Node {
        let parameter_declaration =
            get_declaration_of_kind(&self.symbols, symbol, SyntaxKind::Parameter);
        if parameter_declaration.is_some() {
            return parameter_declaration;
        }
        if !self.sym(symbol).flags.intersects(SymbolFlags::TRANSIENT) {
            return get_declaration_of_kind(&self.symbols, symbol, SyntaxKind::JsDocParameterTag);
        }
        Node::NIL
    }

    // Go: checker/nodebuilderimpl.go:1726 symbolToParameterDeclaration
    pub fn symbol_to_parameter_declaration(
        &mut self,
        b: &Nb,
        parameter_symbol: SymbolId,
        preserve_modifier_flags: bool,
    ) -> Node {
        let parameter_declaration = self.get_effective_parameter_declaration(parameter_symbol);

        let parameter_type = self.get_type_of_symbol(parameter_symbol);
        let parameter_type_node = self.serialize_type_for_declaration(
            b,
            parameter_declaration,
            parameter_type,
            parameter_symbol,
            true,
        );
        let e = nb_e(b);
        let mut modifiers = ModifierList::NIL;
        if !nb_flags(b).intersects(NodeBuilderFlags::OMIT_PARAMETER_MODIFIERS)
            && preserve_modifier_flags
            && parameter_declaration.is_some()
            && can_have_modifiers(parameter_declaration)
        {
            let clones: Vec<Node> = parameter_declaration
                .modifier_nodes()
                .iter()
                .filter(|&n| is_modifier(n))
                .map(|n| e.factory.clone_node(n))
                .collect();
            if !clones.is_empty() {
                modifiers = e.factory.new_modifier_list(&clones);
            }
        }
        let check_flags = self.sym(parameter_symbol).check_flags;
        let is_rest = parameter_declaration.is_some() && is_rest_parameter(parameter_declaration)
            || check_flags.intersects(CheckFlags::REST_PARAMETER);
        let mut dot_dot_dot_token = Node::NIL;
        if is_rest {
            dot_dot_dot_token = e.factory.new_token(SyntaxKind::DotDotDotToken);
        }
        let name = self.parameter_to_parameter_declaration_name(
            b,
            parameter_symbol,
            parameter_declaration,
        );
        let is_optional = parameter_declaration.is_some()
            && self.is_optional_parameter(parameter_declaration)
            || check_flags.intersects(CheckFlags::OPTIONAL_PARAMETER);
        let mut question_token = Node::NIL;
        if is_optional {
            question_token = e.factory.new_token(SyntaxKind::QuestionToken);
        }

        let parameter_node = e.factory.new_parameter_declaration(
            modifiers,
            dot_dot_dot_token,
            name,
            question_token,
            parameter_type_node,
            Node::NIL, // initializer
        );
        let name_len = go_len(&self.sym(parameter_symbol).name) as i32;
        nb_add_approximate_length(b, name_len + 3);
        parameter_node
    }

    // Go: checker/nodebuilderimpl.go:1763 parameterToParameterDeclarationName
    pub(crate) fn parameter_to_parameter_declaration_name(
        &mut self,
        b: &Nb,
        parameter_symbol: SymbolId,
        parameter_declaration: Node,
    ) -> Node {
        if parameter_declaration.is_nil() || parameter_declaration.name().is_nil() {
            let name = self.sym(parameter_symbol).name.clone();
            return self.nb_new_identifier(b, &name, parameter_symbol);
        }

        let name = parameter_declaration.name();
        let e = nb_e(b);
        match name.kind() {
            SyntaxKind::Identifier => {
                let cloned = e.factory.deep_clone_node(name);
                e.set_emit_flags(cloned, EmitFlags::NO_ASCII_ESCAPING);
                b.borrow_mut().id_to_symbol.insert(cloned, parameter_symbol);
                cloned
            }
            SyntaxKind::QualifiedName => {
                let cloned = e.factory.deep_clone_node(name.right());
                e.set_emit_flags(cloned, EmitFlags::NO_ASCII_ESCAPING);
                b.borrow_mut().id_to_symbol.insert(cloned, parameter_symbol);
                cloned
            }
            _ => self.clone_binding_name(b, name),
        }
    }

    // Go: checker/nodebuilderimpl.go:1785 cloneBindingName
    // PORT: Go keeps one `b.cloneBindingNameVisitor` whose visit function is
    // `b.cloneBindingName`. Here the visitor is built per call, because its
    // callback needs `&mut self`. The checker and `b` travel in the visitor
    // `ctx`.
    fn clone_binding_name(&mut self, b: &Nb, node: Node) -> Node {
        if is_computed_property_name(node) && self.is_late_bindable_name(node) {
            let enclosing = nb_enclosing_declaration(b);
            self.track_computed_name(b, node.expression(), enclosing);
        }

        let e = nb_e(b);
        let mut visited = {
            let mut visitor = new_node_visitor(
                |n: Node, v: &mut NodeVisitor<'_, (&mut Checker, &Nb)>| {
                    let b = v.ctx.1;
                    v.ctx.0.clone_binding_name(b, n)
                },
                Some(e.factory().as_node_factory()),
                NodeVisitorHooks::default(),
                (&mut *self, b),
            );
            visitor.visit_each_child(node)
        };

        if is_binding_element(visited) {
            visited = e.factory.update_binding_element(
                visited,
                visited.dot_dot_dot_token(),
                visited.property_name(),
                visited.name(),
                Node::NIL, // remove initializer
            );
        }

        if !node_is_synthesized(visited) {
            visited = e.factory.deep_clone_node(visited);
        }

        e.set_emit_flags(
            visited,
            EmitFlags::SINGLE_LINE | EmitFlags::NO_ASCII_ESCAPING,
        );
        visited
    }

    // Go: checker/nodebuilderimpl.go:1811 serializeTypeForExpression
    pub fn serialize_type_for_expression(&mut self, b: &Nb, expr: Node) -> Node {
        // !!! TODO: shim, add node reuse
        let regular = self.get_regular_type_of_expression(expr);
        let widened = self.get_widened_type(regular);
        let mapper = nb_mapper(b);
        let t = self.instantiate_type(widened, mapper);
        self.type_to_type_node(b, t)
    }

    // Go: checker/nodebuilderimpl.go:1817 serializeInferredReturnTypeForSignature
    fn serialize_inferred_return_type_for_signature(
        &mut self,
        b: &Nb,
        signature: SignatureId,
        return_type: TypeId,
    ) -> Node {
        let old_suppress_report_inference_fallback = {
            let ctx = nb_ctx(b);
            let mut ctx = ctx.borrow_mut();
            let old = ctx.suppress_report_inference_fallback;
            ctx.suppress_report_inference_fallback = true;
            old
        };
        let type_predicate = self.get_type_predicate_of_signature(signature);
        let return_type_node;
        if type_predicate.is_some() {
            let mapper = nb_mapper(b);
            let predicate = if mapper.is_some() {
                self.instantiate_type_predicate(type_predicate, mapper)
            } else {
                type_predicate
            };
            return_type_node = self.type_predicate_to_type_predicate_node_helper(b, predicate);
        } else {
            return_type_node = self.type_to_type_node(b, return_type);
        }
        nb_ctx(b).borrow_mut().suppress_report_inference_fallback =
            old_suppress_report_inference_fallback;
        return_type_node
    }

    // Go: checker/nodebuilderimpl.go:1837 typePredicateToTypePredicateNodeHelper
    fn type_predicate_to_type_predicate_node_helper(
        &mut self,
        b: &Nb,
        type_predicate: TypePredicateId,
    ) -> Node {
        let e = nb_e(b);
        let (kind, parameter_name_text, pt) = {
            let p = self.pred(type_predicate);
            (p.kind, p.parameter_name.clone(), p.t)
        };
        let asserts_modifier = if kind == TypePredicateKind::ASSERTS_THIS
            || kind == TypePredicateKind::ASSERTS_IDENTIFIER
        {
            e.factory.new_token(SyntaxKind::AssertsKeyword)
        } else {
            Node::NIL
        };
        let parameter_name;
        if kind == TypePredicateKind::IDENTIFIER || kind == TypePredicateKind::ASSERTS_IDENTIFIER {
            parameter_name =
                self.nb_new_identifier(b, &parameter_name_text, SymbolId::NIL /*symbol*/);
            e.set_emit_flags(parameter_name, EmitFlags::NO_ASCII_ESCAPING);
        } else {
            parameter_name = e.factory.new_this_type_node();
        }
        let mut type_node = Node::NIL;
        if pt.is_some() {
            type_node = self.type_to_type_node(b, pt);
        }
        e.factory
            .new_type_predicate_node(asserts_modifier, parameter_name, type_node)
    }

    // Go: checker/nodebuilderimpl.go:1864 signatureToSignatureDeclarationHelper
    pub fn signature_to_signature_declaration_helper(
        &mut self,
        b: &Nb,
        signature: SignatureId,
        kind: SyntaxKind,
        options: Option<&SignatureToSignatureDeclarationOptions>,
    ) -> Node {
        let mut type_parameters: Vec<Node> = Vec::new();

        let (expanded_params, cleanup) = self.enter_signature_scope(b, signature);
        nb_add_approximate_length(b, 3);
        // Usually a signature contributes a few more characters than this, but 3 is the minimum

        let (sig_target, sig_mapper, sig_type_parameters, sig_parameters, sig_flags) = {
            let s = self.sig(signature);
            (
                s.target,
                s.mapper,
                s.type_parameters.clone(),
                s.parameters.clone(),
                s.flags,
            )
        };
        let target_type_parameters = if sig_target.is_some() {
            self.sig(sig_target).type_parameters.clone()
        } else {
            Vec::new()
        };
        if nb_flags(b).intersects(NodeBuilderFlags::WRITE_TYPE_ARGUMENTS_OF_SIGNATURE)
            && sig_target.is_some()
            && sig_mapper.is_some()
            && !target_type_parameters.is_empty()
        {
            for parameter in target_type_parameters {
                let instantiated = self.instantiate_type(parameter, sig_mapper);
                type_parameters.push(self.type_to_type_node(b, instantiated));
            }
        } else {
            for parameter in sig_type_parameters {
                type_parameters.push(self.type_parameter_to_declaration(b, parameter));
            }
        }

        let restore_flags = self.save_restore_flags(b);
        nb_clear_flags(b, NodeBuilderFlags::SUPPRESS_ANY_RETURN_TYPE);
        // If the expanded parameter list had a variadic in a non-trailing position, don't expand it
        let has_non_trailing_rest = expanded_params.iter().any(|&p| {
            p != expanded_params[expanded_params.len() - 1]
                && self
                    .sym(p)
                    .check_flags
                    .intersects(CheckFlags::REST_PARAMETER)
        });
        let source_params = if has_non_trailing_rest {
            sig_parameters
        } else {
            expanded_params
        };
        let mut parameters: Vec<Node> = Vec::with_capacity(source_params.len() + 1);
        for parameter in source_params {
            parameters.push(self.symbol_to_parameter_declaration(
                b,
                parameter,
                kind == SyntaxKind::Constructor,
            ));
        }
        let this_parameter = if nb_flags(b).intersects(NodeBuilderFlags::OMIT_THIS_PARAMETER) {
            Node::NIL
        } else {
            self.try_get_this_parameter_declaration(b, signature)
        };
        if this_parameter.is_some() {
            parameters.insert(0, this_parameter);
        }
        restore_flags();

        let mut return_type_node = self.serialize_return_type_for_signature(b, signature, true);

        let e = nb_e(b);
        let mut modifiers: Vec<Node> = Vec::new();
        if let Some(options) = options {
            modifiers = options.modifiers.clone();
        }
        if kind == SyntaxKind::ConstructorType && sig_flags.intersects(SignatureFlags::ABSTRACT) {
            let flags = modifiers_to_flags(&modifiers);
            modifiers =
                create_modifiers_from_modifier_flags(flags | ModifierFlags::ABSTRACT, &mut |k| {
                    e.factory.new_modifier(k)
                });
        }

        let param_list = e.factory.new_node_list(&parameters);
        let mut type_param_list = NodeList::NIL;
        if !type_parameters.is_empty() {
            type_param_list = e.factory.new_node_list(&type_parameters);
        }
        let mut modifier_list = ModifierList::NIL;
        if !modifiers.is_empty() {
            modifier_list = e.factory.new_modifier_list(&modifiers);
        }
        let mut name = Node::NIL;
        if let Some(options) = options {
            name = options.name;
        }
        if name.is_nil() {
            name = e.factory.new_identifier("");
        }

        let f = &e.factory;
        let node = match kind {
            SyntaxKind::CallSignature => {
                f.new_call_signature_declaration(type_param_list, param_list, return_type_node)
            }
            SyntaxKind::ConstructSignature => {
                f.new_construct_signature_declaration(type_param_list, param_list, return_type_node)
            }
            SyntaxKind::MethodSignature => {
                let question_token = options.map_or(Node::NIL, |o| o.question_token);
                f.new_method_signature_declaration(
                    modifier_list,
                    name,
                    question_token,
                    type_param_list,
                    param_list,
                    return_type_node,
                )
            }
            SyntaxKind::MethodDeclaration => f.new_method_declaration(
                modifier_list,
                Node::NIL, // asteriskToken
                name,
                Node::NIL, // questionToken
                type_param_list,
                param_list,
                return_type_node,
                Node::NIL, // fullSignature
                Node::NIL, // body
            ),
            SyntaxKind::Constructor => f.new_constructor_declaration(
                modifier_list,
                NodeList::NIL, // typeParamList
                param_list,
                Node::NIL, // returnTypeNode
                Node::NIL, // fullSignature
                Node::NIL, // body
            ),
            SyntaxKind::GetAccessor => f.new_get_accessor_declaration(
                modifier_list,
                name,
                NodeList::NIL, // typeParamList
                param_list,
                return_type_node,
                Node::NIL, // fullSignature
                Node::NIL, // body
            ),
            SyntaxKind::SetAccessor => f.new_set_accessor_declaration(
                modifier_list,
                name,
                NodeList::NIL, // typeParamList
                param_list,
                Node::NIL, // returnTypeNode
                Node::NIL, // fullSignature
                Node::NIL, // body
            ),
            SyntaxKind::IndexSignature => {
                f.new_index_signature_declaration(modifier_list, param_list, return_type_node)
            }
            // !!! JSDoc Support
            // case kind == ast.KindJSDocFunctionType:
            // 	node = b.f.NewJSDocFunctionType(parameters, returnTypeNode)
            SyntaxKind::FunctionType => {
                if return_type_node.is_nil() {
                    return_type_node =
                        f.new_type_reference_node(f.new_identifier(""), NodeList::NIL);
                }
                f.new_function_type_node(type_param_list, param_list, return_type_node)
            }
            SyntaxKind::ConstructorType => {
                if return_type_node.is_nil() {
                    return_type_node =
                        f.new_type_reference_node(f.new_identifier(""), NodeList::NIL);
                }
                f.new_constructor_type_node(
                    modifier_list,
                    type_param_list,
                    param_list,
                    return_type_node,
                )
            }
            // TODO: assert name is Identifier
            SyntaxKind::FunctionDeclaration => f.new_function_declaration(
                modifier_list,
                Node::NIL, // asteriskToken
                name,
                type_param_list,
                param_list,
                return_type_node,
                Node::NIL, // fullSignature
                Node::NIL, // body
            ),
            // TODO: assert name is Identifier
            SyntaxKind::FunctionExpression => f.new_function_expression(
                modifier_list,
                Node::NIL, // asteriskToken
                name,
                type_param_list,
                param_list,
                return_type_node,
                Node::NIL, // fullSignature
                f.new_block(f.new_node_list(&[]), false),
            ),
            SyntaxKind::ArrowFunction => f.new_arrow_function(
                modifier_list,
                type_param_list,
                param_list,
                return_type_node,
                Node::NIL, // fullSignature
                Node::NIL, // equalsGreaterThanToken
                f.new_block(f.new_node_list(&[]), false),
            ),
            _ => panic!("Unhandled kind in signatureToSignatureDeclarationHelper"),
        };

        // !!! TODO: Smuggle type arguments of signatures out for quickinfo
        // if typeArguments != nil {
        // 	node.TypeArguments = b.f.NewNodeList(typeArguments)
        // }

        cleanup(self);
        node
    }

    // Go: checker/nodebuilderimpl.go:1984 getExpandedParameters
    pub fn get_expanded_parameters(
        &mut self,
        sig: SignatureId,
        skip_union_expanding: bool,
    ) -> Vec<Vec<SymbolId>> {
        let sig_parameters = self.sig(sig).parameters.clone();
        if self.signature_has_rest_parameter(sig) {
            let rest_index = sig_parameters.len() - 1;
            let rest_symbol = sig_parameters[rest_index];
            let rest_type = self.get_type_of_symbol(rest_symbol);

            if self.is_tuple_type(rest_type) {
                return vec![self.get_expanded_parameters_expand_tuple_members(
                    &sig_parameters,
                    rest_type,
                    rest_index,
                    rest_symbol,
                )];
            } else if !skip_union_expanding
                && self.ty(rest_type).flags.intersects(TypeFlags::UNION)
                && self
                    .ty(rest_type)
                    .as_union_type()
                    .union_or_intersection
                    .types
                    .iter()
                    .all(|&t| self.is_tuple_type(t))
            {
                let types = self
                    .ty(rest_type)
                    .as_union_type()
                    .union_or_intersection
                    .types
                    .clone();
                return types
                    .into_iter()
                    .map(|t| {
                        self.get_expanded_parameters_expand_tuple_members(
                            &sig_parameters,
                            t,
                            rest_index,
                            rest_symbol,
                        )
                    })
                    .collect();
            }
        }
        vec![sig_parameters]
    }

    // Go: checker/nodebuilderimpl.go:1989 getExpandedParameters.getUniqAssociatedNamesFromTupleType
    // PORT: a Go closure inside getExpandedParameters. It captures nothing
    // but `c`, so it is a private method. The name has a prefix because
    // checker.go has a different function with the Go name.
    fn get_expanded_parameters_uniq_associated_names(
        &mut self,
        t: TypeId,
        rest_symbol: SymbolId,
    ) -> Vec<String> {
        let target = self.ty(t).target();
        let element_infos = self.ty(target).as_tuple_type().element_infos.clone();
        let mut names: Vec<String> = element_infos
            .iter()
            .enumerate()
            .map(|(i, &info)| self.get_tuple_element_label(info, rest_symbol, i as i32))
            .collect();
        if !names.is_empty() {
            let mut duplicates: Vec<usize> = Vec::new();
            let mut unique_names: FxHashSet<String> = FxHashSet::default();
            for (i, name) in names.iter().enumerate() {
                if unique_names.contains(name) {
                    duplicates.push(i);
                } else {
                    unique_names.insert(name.clone());
                }
            }
            let mut counters: FxHashMap<String, i32> = FxHashMap::default();
            for i in duplicates {
                let mut counter = counters.get(&names[i]).copied().unwrap_or(1);
                let name;
                loop {
                    let candidate = format!("{}_{}", names[i], counter);
                    if unique_names.contains(&candidate) {
                        counter += 1;
                        continue;
                    }
                    unique_names.insert(candidate.clone());
                    name = candidate;
                    break;
                }
                names[i] = name;
                // PORT: Go keys the counter by the renamed name, as here.
                counters.insert(names[i].clone(), counter + 1);
            }
        }
        names
    }

    // Go: checker/nodebuilderimpl.go:2028 getExpandedParameters.expandSignatureParametersWithTupleMembers
    // PORT: a Go closure inside getExpandedParameters. `sig.parameters` is
    // passed in as `sig_parameters`. The name has a prefix because
    // checker.go has a different function with the Go name.
    fn get_expanded_parameters_expand_tuple_members(
        &mut self,
        sig_parameters: &[SymbolId],
        rest_type: TypeId,
        rest_index: usize,
        rest_symbol: SymbolId,
    ) -> Vec<SymbolId> {
        let element_types = self.get_type_arguments(rest_type);
        let associated_names =
            self.get_expanded_parameters_uniq_associated_names(rest_type, rest_symbol);
        let target = self.ty(rest_type).target();
        let mut rest_params: Vec<SymbolId> = Vec::with_capacity(element_types.len());
        for (i, &t) in element_types.iter().enumerate() {
            // Lookup the label from the individual tuple passed in before falling back to the signature `rest` parameter name
            // TODO: getTupleElementLabel can no longer fail, investigate if this lack of falliability meaningfully changes output
            let name = associated_names[i].clone();
            let flags = self.ty(target).as_tuple_type().element_infos[i].flags;
            let check_flags = if flags.intersects(ElementFlags::VARIABLE) {
                CheckFlags::REST_PARAMETER
            } else if flags.intersects(ElementFlags::OPTIONAL) {
                CheckFlags::OPTIONAL_PARAMETER
            } else {
                CheckFlags::NONE
            };
            let symbol =
                self.new_symbol_ex(SymbolFlags::FUNCTION_SCOPED_VARIABLE, &name, check_flags);
            let resolved_type = if flags.intersects(ElementFlags::REST) {
                self.create_array_type(t)
            } else {
                t
            };
            self.value_symbol_links
                .get_by_id(&self.symbols, symbol)
                .resolved_type = resolved_type;
            rest_params.push(symbol);
        }
        let mut result = sig_parameters[0..rest_index].to_vec();
        result.extend(rest_params);
        result
    }

    // Go: checker/nodebuilderimpl.go:2072 tryGetThisParameterDeclaration
    fn try_get_this_parameter_declaration(&mut self, b: &Nb, signature: SignatureId) -> Node {
        let (this_parameter, declaration) = {
            let s = self.sig(signature);
            (s.this_parameter, s.declaration)
        };
        if this_parameter.is_some() {
            return self.symbol_to_parameter_declaration(b, this_parameter, false);
        }
        if declaration.is_some() && is_in_js_file(declaration) {
            // !!! JSDoc Support
            // thisTag := getJSDocThisTag(signature.declaration)
            // if (thisTag && thisTag.typeExpression) {
            // 	return factory.createParameterDeclaration(
            // 		/*modifiers*/ undefined,
            // 		/*dotDotDotToken*/ undefined,
            // 		"this",
            // 		/*questionToken*/ undefined,
            // 		typeToTypeNodeHelper(getTypeFromTypeNode(context, thisTag.typeExpression), context),
            // 	);
            // }
        }
        Node::NIL
    }

    // Go: checker/nodebuilderimpl.go:2095 serializeReturnTypeForSignature
    /// Serializes the return type of the signature by first trying to use the syntactic printer if possible and falling back to the checker type if not.
    // PORT: `enclosingSymbolTypes` is keyed by `SymbolId` for Go
    // `ast.GetSymbolId(symbol)`. The pseudochecker result is held in an
    // `Option` so the Go `pt = nil` step has a value.
    pub fn serialize_return_type_for_signature(
        &mut self,
        b: &Nb,
        signature: SignatureId,
        try_reuse: bool,
    ) -> Node {
        let suppress_any = nb_flags(b).intersects(NodeBuilderFlags::SUPPRESS_ANY_RETURN_TYPE);
        let restore_flags = self.save_restore_flags(b);
        if suppress_any {
            nb_clear_flags(b, NodeBuilderFlags::SUPPRESS_ANY_RETURN_TYPE); // suppress only toplevel `any`s
        }
        let mut return_type_node = Node::NIL;

        let declaration = self.sig(signature).declaration;
        let return_type;
        if declaration.is_some() && !node_is_synthesized(declaration) {
            let symbol = self.get_symbol_of_declaration(declaration);
            // Go keys the map by `ast.GetSymbolId(symbol)`, which gives the
            // symbol its id (`ValueSymbolLinkStore`).
            get_symbol_id(&self.symbols, symbol);
            let cached = nb_ctx(b)
                .borrow()
                .enclosing_symbol_types
                .get(&symbol)
                .copied();
            match cached {
                Some(t) if t.is_some() => return_type = t,
                _ => {
                    let signature_return_type = self.get_return_type_of_signature(signature);
                    let mapper = nb_mapper(b);
                    return_type = self.instantiate_type(signature_return_type, mapper);
                }
            }
        } else {
            return_type = self.get_return_type_of_signature(signature);
        }
        if !(suppress_any && self.is_type_any(return_type)) {
            if !self.is_actively_expanding(b)
                && try_reuse
                && nb_enclosing_declaration(b).is_some()
                && declaration.is_some()
                && !node_is_synthesized(declaration)
            {
                let declaration_symbol = self.get_symbol_of_declaration(declaration);
                let restore = self.add_symbol_type_to_context(b, declaration_symbol, return_type);
                let pc = b.borrow().pc.clone();
                let mut pt = Some(pc.get_return_type_of_signature(&self.symbols, declaration));
                let report_fallback = !nb_ctx(b).borrow().suppress_report_inference_fallback;
                let equivalent = match &pt {
                    Some(p) => self.pseudo_type_equivalent_to_type(
                        b,
                        p,
                        return_type,
                        false,
                        report_fallback,
                    ),
                    None => false,
                };
                if equivalent {
                    // Also verify the pseudo type captures any inferred type predicate, not just the boolean return type.
                    // The pseudochecker is unaware of inferred type predicates, so it produces boolean where
                    // the checker infers e.g. `x is string`.
                    let type_predicate = self.get_type_predicate_of_signature(signature);
                    if type_predicate.is_some() {
                        let matches = match &pt {
                            Some(p) => {
                                self.pseudo_return_type_matches_predicate(b, p, type_predicate)
                            }
                            None => false,
                        };
                        if !matches {
                            if !nb_ctx(b).borrow().suppress_report_inference_fallback {
                                nb_tracker(b).report_inference_fallback(self, declaration);
                            }
                            pt = None;
                        }
                    }
                    if let Some(p) = &pt {
                        // !!! TODO: If annotated type node is a reference with insufficient type arguments, we should still fall back to type serialization
                        // see: canReuseTypeNodeAnnotation in strada for context
                        return_type_node =
                            self.pseudo_type_to_node_with_checker_fallback(b, p, return_type);
                    }
                }
                restore();
            }
            if return_type_node.is_nil() {
                return_type_node =
                    self.serialize_inferred_return_type_for_signature(b, signature, return_type);
            }
        }

        if return_type_node.is_nil() && !suppress_any {
            return_type_node = nb_e(b)
                .factory
                .new_keyword_type_node(SyntaxKind::AnyKeyword);
        }
        restore_flags();
        return_type_node
    }

    // Go: checker/nodebuilderimpl.go:2150 isTriviallySerializableComputedName
    fn is_trivially_serializable_computed_name(&mut self, b: &Nb, e: Node) -> bool {
        let shape_good = e.is_some()
            && e.name().is_some()
            && is_computed_property_name(e.name())
            && is_entity_name_expression(e.name().expression());
        if !shape_good {
            return false;
        }
        // ts#64649, Go N' nodebuilderimpl.go:2156: the checker method, not the emit resolver.
        let enclosing_declaration = nb_ctx(b).borrow().enclosing_declaration;
        self.is_entity_name_visible(e.name().expression(), enclosing_declaration, false)
            .accessibility
            == SymbolAccessibility::ACCESSIBLE
    }

    // Go: checker/nodebuilderimpl.go:2159 indexInfoToObjectComputedNamesOrSignatureDeclaration
    pub fn index_info_to_object_computed_names_or_signature_declaration(
        &mut self,
        b: &Nb,
        index_info: IndexInfoId,
        type_node: Node,
    ) -> Vec<Node> {
        let (components, is_readonly) = {
            let info = self.index_info(index_info);
            (info.components.clone(), info.is_readonly)
        };
        if !components.is_empty() {
            // Index info is derived from object or class computed property names (plus explicit named members) - we can clone those instead of writing out the result computed index signature
            let mut all_component_computed_names_serializable =
                nb_enclosing_declaration(b).is_some();
            if all_component_computed_names_serializable {
                for &c in &components {
                    if !self.is_trivially_serializable_computed_name(b, c) {
                        all_component_computed_names_serializable = false;
                        break;
                    }
                }
            }
            if all_component_computed_names_serializable {
                // Only use computed name serialization form if all components are visible and take the `a.b.c` form
                let mut new_components: Vec<Node> = Vec::new();
                for c in components {
                    // skip late bound props that contribute to the index signature - they'll be created by property creation anyway
                    if !self.has_late_bindable_name(c) {
                        new_components.push(c);
                    }
                }
                let e_ctx = nb_e(b);
                let mut bailed = false;
                // PORT: Go `core.Map` visits every component even after a bail, so this loop does too.
                let mut results: Vec<Node> = Vec::with_capacity(new_components.len());
                for e in new_components {
                    let name = self.reuse_node(b, e.name());
                    if name.is_some() {
                        // Still need to track visibility even if we've already checked it to paint references as used
                        let enclosing = nb_enclosing_declaration(b);
                        self.track_computed_name(b, e.name().expression(), enclosing);
                        let mut mods = ModifierList::NIL;
                        if is_readonly {
                            mods = e_ctx.factory.new_modifier_list(&[e_ctx
                                .factory
                                .new_modifier(SyntaxKind::ReadonlyKeyword)]);
                        }
                        let mut postfix_token = Node::NIL;
                        if e.postfix_token().is_some() {
                            postfix_token = e_ctx.factory.clone_node(e.postfix_token());
                        }
                        let current_type_node = if type_node.is_some() {
                            e_ctx.factory.deep_clone_node(type_node)
                        } else {
                            let t = self.get_type_of_symbol(e.symbol());
                            self.type_to_type_node(b, t)
                        };
                        let sig = e_ctx.factory.new_property_signature_declaration(
                            mods,
                            name,
                            postfix_token,
                            current_type_node,
                            Node::NIL,
                        );
                        set_node_loc(sig, e.loc());
                        results.push(sig);
                        continue;
                    }
                    bailed = true;
                    results.push(Node::NIL);
                }
                if !bailed {
                    return results;
                }
            }
        }
        vec![self.index_info_to_index_signature_declaration_helper(b, index_info, type_node)]
    }

    // Go: checker/nodebuilderimpl.go:2210 indexInfoToIndexSignatureDeclarationHelper
    pub fn index_info_to_index_signature_declaration_helper(
        &mut self,
        b: &Nb,
        index_info: IndexInfoId,
        mut type_node: Node,
    ) -> Node {
        let name = self.get_name_from_index_info(index_info);
        let (key_type, value_type, is_readonly) = {
            let info = self.index_info(index_info);
            (info.key_type, info.value_type, info.is_readonly)
        };
        let indexer_type_node = self.type_to_type_node(b, key_type);

        let name_node = self.nb_new_identifier(b, &name, SymbolId::NIL /*symbol*/);
        let e = nb_e(b);
        let indexing_parameter = e.factory.new_parameter_declaration(
            ModifierList::NIL,
            Node::NIL,
            name_node,
            Node::NIL,
            indexer_type_node,
            Node::NIL,
        );
        if type_node.is_nil() {
            if value_type.is_nil() {
                type_node = e.factory.new_keyword_type_node(SyntaxKind::AnyKeyword);
            } else {
                type_node = self.type_to_type_node(b, value_type);
            }
        }
        if value_type.is_nil()
            && !nb_flags(b).intersects(NodeBuilderFlags::ALLOW_EMPTY_INDEX_INFO_TYPE)
        {
            nb_ctx(b).borrow_mut().encountered_error = true;
        }
        nb_add_approximate_length(b, go_len(&name) as i32 + 4);
        let mut modifiers = ModifierList::NIL;
        if is_readonly {
            nb_add_approximate_length(b, 9);
            modifiers = e
                .factory
                .new_modifier_list(&[e.factory.new_modifier(SyntaxKind::ReadonlyKeyword)]);
        }
        e.factory.new_index_signature_declaration(
            modifiers,
            e.factory.new_node_list(&[indexing_parameter]),
            type_node,
        )
    }
}
