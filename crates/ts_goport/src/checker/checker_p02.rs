//! Port of typescript-go `internal/checker/checker.go` lines 1116-2055.
//!
//! PORT: Go `c.error(...)` returns the `*ast.Diagnostic` it already added, and
//! some Go callers mutate it afterwards (`AddRelatedInfo`). Our `Diagnostic`
//! is owned, so those callers build the diagnostic with
//! `new_diagnostic_for_node`, finish it, then call `add_diagnostic`. That is
//! exactly what Go `c.error` does, in the same order of effects.

use crate::diagnostics::Message;
use crate::prelude::*;
use std::cell::Cell;

// Go: checker/checker.go:1128 createFileIndexMap
pub fn create_file_index_map(files: &[Node]) -> FxHashMap<Node, i32> {
    let mut result: FxHashMap<Node, i32> = FxHashMap::default();
    result.reserve(files.len());
    for (i, file) in files.iter().enumerate() {
        result.insert(*file, i as i32);
    }
    result
}

impl Checker {
    // Go: checker/checker.go:1136 countGlobalSymbols
    // PORT: Go reads `len(file.Locals)` directly; the symbol tables live in the
    // checker's symbol arena, so this is a `Checker` method.
    pub fn count_global_symbols(&self, files: &[Node]) -> i32 {
        let mut count = 0;
        for &file in files {
            if !is_external_or_common_js_module(file) {
                count += self.symbols.len(file.locals()) as i32;
            }
        }
        count
    }

    // Go: checker/checker.go:1146 reportUnreliableWorker
    pub fn report_unreliable_worker(&mut self, t: TypeId) -> TypeId {
        if t == self.marker_super_type || t == self.marker_sub_type || t == self.marker_other_type {
            self.reliability_flags |= RelationComparisonResult::REPORTS_UNRELIABLE;
        }
        t
    }

    // Go: checker/checker.go:1153 reportUnmeasurableWorker
    pub fn report_unmeasurable_worker(&mut self, t: TypeId) -> TypeId {
        if t == self.marker_super_type || t == self.marker_sub_type || t == self.marker_other_type {
            self.reliability_flags |= RelationComparisonResult::REPORTS_UNMEASURABLE;
        }
        t
    }

    // Go: checker/checker.go:1161 getGlobalTypeResolver
    // Resolve to the global class or interface by the given name and arity, or emptyObjectType/emptyGenericType otherwise
    // PORT: Go `core.Memoize` becomes a closure over a `Cell` cache. Like Go,
    // the value is computed on the first call and returned afterwards.
    pub fn get_global_type_resolver(
        &self,
        name: &str,
        arity: i32,
        report_errors: bool,
    ) -> Rc<dyn Fn(&mut Checker) -> TypeId> {
        let name = name.to_string();
        let cache: Cell<Option<TypeId>> = Cell::new(None);
        Rc::new(move |c: &mut Checker| -> TypeId {
            if let Some(value) = cache.get() {
                return value;
            }
            let value = c.get_global_type(&name, arity, report_errors);
            cache.set(Some(value));
            value
        })
    }

    // Go: checker/checker.go:1168 getGlobalTypeAliasResolver
    // Resolve to the global type alias symbol by the given name and arity, or nil otherwise
    pub fn get_global_type_alias_resolver(
        &self,
        name: &str,
        arity: i32,
        report_errors: bool,
    ) -> Rc<dyn Fn(&mut Checker) -> SymbolId> {
        let name = name.to_string();
        let cache: Cell<Option<SymbolId>> = Cell::new(None);
        Rc::new(move |c: &mut Checker| -> SymbolId {
            if let Some(value) = cache.get() {
                return value;
            }
            let value = c.get_global_type_alias_symbol(&name, arity, report_errors);
            cache.set(Some(value));
            value
        })
    }

    // Go: checker/checker.go:1175 getGlobalValueSymbolResolver
    // Resolve to the global value symbol by the given name, or nil otherwise
    pub fn get_global_value_symbol_resolver(
        &self,
        name: &str,
        report_errors: bool,
    ) -> Rc<dyn Fn(&mut Checker) -> SymbolId> {
        let name = name.to_string();
        let cache: Cell<Option<SymbolId>> = Cell::new(None);
        Rc::new(move |c: &mut Checker| -> SymbolId {
            if let Some(value) = cache.get() {
                return value;
            }
            let value = c.get_global_symbol(
                &name,
                SymbolFlags::VALUE,
                if report_errors {
                    Some(diag::Cannot_find_global_value_0)
                } else {
                    None
                },
            );
            cache.set(Some(value));
            value
        })
    }

    // Go: checker/checker.go:1181 getGlobalTypeSymbolResolver
    pub fn get_global_type_symbol_resolver(
        &self,
        name: &str,
        report_errors: bool,
    ) -> Rc<dyn Fn(&mut Checker) -> SymbolId> {
        let name = name.to_string();
        let cache: Cell<Option<SymbolId>> = Cell::new(None);
        Rc::new(move |c: &mut Checker| -> SymbolId {
            if let Some(value) = cache.get() {
                return value;
            }
            let value = c.get_global_symbol(
                &name,
                SymbolFlags::TYPE,
                if report_errors {
                    Some(diag::Cannot_find_global_type_0)
                } else {
                    None
                },
            );
            cache.set(Some(value));
            value
        })
    }

    // Go: checker/checker.go:1187 getGlobalTypesResolver
    pub fn get_global_types_resolver(
        &self,
        names: &[&str],
        arity: i32,
        report_errors: bool,
    ) -> Rc<dyn Fn(&mut Checker) -> Vec<TypeId>> {
        let names: Vec<String> = names.iter().map(|s| s.to_string()).collect();
        let cache: RefCell<Option<Vec<TypeId>>> = RefCell::new(None);
        Rc::new(move |c: &mut Checker| -> Vec<TypeId> {
            if let Some(value) = cache.borrow().as_ref() {
                return value.clone();
            }
            let value: Vec<TypeId> = names
                .iter()
                .map(|name| c.get_global_type(name, arity, report_errors))
                .collect();
            *cache.borrow_mut() = Some(value.clone());
            value
        })
    }

    // Go: checker/checker.go:1195 getGlobalTypeAliasSymbol
    pub fn get_global_type_alias_symbol(
        &mut self,
        name: &str,
        arity: i32,
        report_errors: bool,
    ) -> SymbolId {
        let symbol = self.get_global_symbol(
            name,
            SymbolFlags::TYPE_ALIAS,
            if report_errors {
                Some(diag::Cannot_find_global_type_0)
            } else {
                None
            },
        );
        if symbol.is_nil() {
            return SymbolId::NIL;
        }
        // Resolve the declared type of the symbol. This resolves type parameters for the type alias so that we can check arity.
        self.get_declared_type_of_symbol(symbol);
        if self.type_alias_links.get(symbol).type_parameters.len() as i32 != arity {
            if report_errors {
                let decl = self
                    .sym(symbol)
                    .declarations
                    .iter()
                    .copied()
                    .find(|&d| is_type_alias_declaration(d))
                    .unwrap_or(Node::NIL);
                let symbol_name = symbol_name(&self.symbols, symbol);
                self.error(
                    decl,
                    diag::Global_type_0_must_have_1_type_parameter_s,
                    args![symbol_name, arity],
                );
            }
            return SymbolId::NIL;
        }
        symbol
    }

    // Go: checker/checker.go:1212 GetTypeAliasTypeParameters
    pub fn get_type_alias_type_parameters(&mut self, symbol: SymbolId) -> Vec<TypeId> {
        if !self.sym(symbol).flags.intersects(SymbolFlags::TYPE_ALIAS) {
            panic!("Attempted to fetch type alias parameters for non-type-alias symbol");
        }
        self.get_declared_type_of_symbol(symbol);
        self.type_alias_links.get(symbol).type_parameters.clone()
    }

    // Go: checker/checker.go:1220 getGlobalType
    pub fn get_global_type(&mut self, name: &str, arity: i32, report_errors: bool) -> TypeId {
        let symbol = self.get_global_symbol(
            name,
            SymbolFlags::TYPE,
            if report_errors {
                Some(diag::Cannot_find_global_type_0)
            } else {
                None
            },
        );
        if symbol.is_some() {
            if self
                .sym(symbol)
                .flags
                .intersects(SymbolFlags::CLASS | SymbolFlags::INTERFACE)
            {
                let t = self.get_declared_type_of_symbol(symbol);
                if self.ty(t).as_interface_type().type_parameters().len() as i32 == arity {
                    return t;
                }
                if report_errors {
                    let decl = self.get_global_type_declaration(symbol);
                    let symbol_name = symbol_name(&self.symbols, symbol);
                    self.error(
                        decl,
                        diag::Global_type_0_must_have_1_type_parameter_s,
                        args![symbol_name, arity],
                    );
                }
            } else if report_errors {
                let decl = self.get_global_type_declaration(symbol);
                let symbol_name = symbol_name(&self.symbols, symbol);
                self.error(
                    decl,
                    diag::Global_type_0_must_be_a_class_or_interface_type,
                    args![symbol_name],
                );
            }
        }
        if arity != 0 {
            return self.empty_generic_type;
        }
        self.empty_object_type
    }

    // Go: checker/checker.go:1241 getGlobalTypeDeclaration
    // PORT: Go package function reading `symbol.Declarations`; it needs the
    // symbol arena, so it is a `Checker` method.
    pub fn get_global_type_declaration(&self, symbol: SymbolId) -> Node {
        for &declaration in &self.sym(symbol).declarations {
            match declaration.kind() {
                SyntaxKind::ClassDeclaration
                | SyntaxKind::InterfaceDeclaration
                | SyntaxKind::EnumDeclaration
                | SyntaxKind::TypeAliasDeclaration => return declaration,
                _ => {}
            }
        }
        Node::NIL
    }

    // Go: checker/checker.go:1251 getGlobalSymbol
    // PORT: the nil-able Go `*diagnostics.Message` is `Option<&'static Message>`.
    pub fn get_global_symbol(
        &mut self,
        name: &str,
        meaning: SymbolFlags,
        diagnostic: Option<&'static Message>,
    ) -> SymbolId {
        // Don't track references for global symbols anyway, so value if `isReference` is arbitrary
        let resolve_name = self.resolve_name.clone();
        resolve_name(
            self,
            Node::NIL,
            name,
            meaning,
            diagnostic.map(NameNotFound::Message),
            false, /*isUse*/
            false, /*excludeGlobals*/
        )
    }

    // Go: checker/checker.go:1256 initializeClosures
    pub fn initialize_closures(&mut self) {
        self.is_primitive_or_object_or_empty_type = Rc::new(|c: &mut Checker, t: TypeId| -> bool {
            c.ty(t)
                .flags
                .intersects(TypeFlags::PRIMITIVE | TypeFlags::NON_PRIMITIVE)
                || c.is_empty_anonymous_object_type(t)
        });
        self.contains_missing_type = Rc::new(|c: &mut Checker, t: TypeId| -> bool {
            t == c.missing_type
                || c.ty(t).flags.intersects(TypeFlags::UNION)
                    && c.ty(t).types()[0] == c.missing_type
        });
        self.is_string_index_signature_only_type = Rc::new(|c: &mut Checker, t: TypeId| -> bool {
            c.is_string_index_signature_only_type_worker(t)
        });
        self.mark_node_assignments =
            Rc::new(|c: &mut Checker, node: Node| -> bool { c.mark_node_assignments_worker(node) });
        self.compare_types_assignable = Rc::new(
            |c: &mut Checker, s: TypeId, t: TypeId, report_errors: bool| -> Ternary {
                c.compare_types_assignable_worker(s, t, report_errors)
            },
        );
    }

    // Go: checker/checker.go:1269 initializeIterationResolvers
    pub fn initialize_iteration_resolvers(&mut self) {
        let sync_builtin = self.get_global_types_resolver(
            &[
                "ArrayIterator",
                "MapIterator",
                "SetIterator",
                "StringIterator",
            ],
            1,
            false, /*reportErrors*/
        );
        self.sync_iteration_types_resolver = Rc::new(IterationTypesResolver {
            iterator_symbol_name: "iterator".to_string(),
            get_global_iterator_type: self.get_global_iterator_type.clone(),
            get_global_iterable_type: self.get_global_iterable_type.clone(),
            get_global_iterable_type_checked: self.get_global_iterable_type_checked.clone(),
            get_global_iterable_iterator_type: self.get_global_iterable_iterator_type.clone(),
            get_global_iterable_iterator_type_checked: self
                .get_global_iterable_iterator_type_checked
                .clone(),
            get_global_iterator_object_type: self.get_global_iterator_object_type.clone(),
            get_global_generator_type: self.get_global_generator_type.clone(),
            get_global_builtin_iterator_types: sync_builtin,
            resolve_iteration_type: Rc::new(
                |_c: &mut Checker, t: TypeId, _error_node: Node| -> TypeId { t },
            ),
            must_have_a_next_method_diagnostic: diag::An_iterator_must_have_a_next_method,
            must_be_a_method_diagnostic: diag::The_0_property_of_an_iterator_must_be_a_method,
            must_have_a_value_diagnostic:
                diag::The_type_returned_by_the_0_method_of_an_iterator_must_have_a_value_property,
        });
        let async_builtin = self.get_global_types_resolver(
            &["ReadableStreamAsyncIterator"],
            1,
            false, /*reportErrors*/
        );
        self.async_iteration_types_resolver = Rc::new(IterationTypesResolver {
            iterator_symbol_name: "asyncIterator".to_string(),
            get_global_iterator_type: self.get_global_async_iterator_type.clone(),
            get_global_iterable_type: self.get_global_async_iterable_type.clone(),
            get_global_iterable_type_checked: self.get_global_async_iterable_type_checked.clone(),
            get_global_iterable_iterator_type: self.get_global_async_iterable_iterator_type.clone(),
            get_global_iterable_iterator_type_checked: self.get_global_async_iterable_iterator_type_checked.clone(),
            get_global_iterator_object_type: self.get_global_async_iterator_object_type.clone(),
            get_global_generator_type: self.get_global_async_generator_type.clone(),
            get_global_builtin_iterator_types: async_builtin,
            resolve_iteration_type: Rc::new(|c: &mut Checker, t: TypeId, error_node: Node| -> TypeId {
                c.get_awaited_type_ex(
                    t,
                    error_node,
                    Some(diag::Type_of_await_operand_must_either_be_a_valid_promise_or_must_not_contain_a_callable_then_member),
                    args![],
                )
            }),
            must_have_a_next_method_diagnostic: diag::An_async_iterator_must_have_a_next_method,
            must_be_a_method_diagnostic: diag::The_0_property_of_an_async_iterator_must_be_a_method,
            must_have_a_value_diagnostic: diag::The_type_returned_by_the_0_method_of_an_async_iterator_must_be_a_promise_for_a_type_with_a_value_property,
        });
    }

    // Go: checker/checker.go:1306 initializeChecker
    pub fn initialize_checker(&mut self) {
        // Initialize global symbol table
        let mut ambient_module_symbols: Vec<SymbolId> = Vec::new();
        let files = self.files.clone();
        let mut augmentations: Vec<Vec<Node>> = Vec::with_capacity(files.len());
        for &file in &files {
            if !is_external_or_common_js_module(file) {
                // It is an error for a non-external-module (i.e. script) to declare its own `globalThis`.
                let file_global_this_symbol = self.symbols.get(file.locals(), "globalThis");
                if file_global_this_symbol.is_some() {
                    let declarations = self.sym(file_global_this_symbol).declarations.clone();
                    for d in declarations {
                        self.add_diagnostic(new_diagnostic_for_node(
                            d,
                            diag::Declaration_name_conflicts_with_built_in_global_identifier_0,
                            args!["globalThis"],
                        ));
                    }
                }
                for symbol in self.symbols.values(file.locals()) {
                    // We defer merging of global ambient module declarations since they may require other global symbols
                    // and types to be resolved. See https://github.com/microsoft/typescript-go/issues/2953.
                    if self.sym(symbol).flags.intersects(SymbolFlags::MODULE)
                        && is_ambient_module_symbol_name(&self.sym(symbol).name)
                    {
                        ambient_module_symbols.push(symbol);
                    } else {
                        self.merge_global_symbol(symbol);
                    }
                }
            }
            self.pattern_ambient_modules
                .extend(file_bind_data(file).pattern_ambient_modules.iter().cloned());
            augmentations.push(source_file_info(file).module_augmentations.clone());
            if file.symbol().is_some() {
                // Merge in UMD exports with first-in-wins semantics (see #9771)
                for (name, symbol) in self.symbols.entries(file_bind_data(file).global_exports) {
                    if self.symbols.get_name(self.globals, &name).is_nil() {
                        self.symbols.set(self.globals, name, symbol);
                    }
                }
            }
        }
        // We do global augmentations separately from module augmentations (and before creating global types) because they
        //  1. Affect global types. We won't have the correct global types until global augmentations are merged. Also,
        //  2. Module augmentation instantiation requires creating the type of a module, which, in turn, can require
        //       checking for an export or property on the module (if export=) which, in turn, can fall back to the
        //       apparent type of the module - either globalObjectType or globalFunctionType - which wouldn't exist if we
        //       did module augmentations prior to finalizing the global types.
        for list in &augmentations {
            for &augmentation in list {
                // Merge 'global' module augmentations. This needs to be done after global symbol table is initialized to
                // make sure that all ambient modules are indexed
                if is_global_scope_augmentation(augmentation.parent()) {
                    self.merge_module_augmentation(augmentation);
                }
            }
        }
        self.add_undefined_to_globals_or_error_on_redeclaration();
        let undefined_symbol = self.undefined_symbol;
        let undefined_widening_type = self.undefined_widening_type;
        self.value_symbol_links
            .get_by_id(&self.symbols, undefined_symbol)
            .resolved_type = undefined_widening_type;
        let arguments_symbol = self.arguments_symbol;
        let iarguments_type =
            self.get_global_type("IArguments", 0 /*arity*/, true /*reportErrors*/);
        self.value_symbol_links
            .get_by_id(&self.symbols, arguments_symbol)
            .resolved_type = iarguments_type;
        let unknown_symbol = self.unknown_symbol;
        let error_type = self.error_type;
        self.value_symbol_links
            .get_by_id(&self.symbols, unknown_symbol)
            .resolved_type = error_type;
        let global_this_symbol = self.global_this_symbol;
        let global_this_object_type =
            self.new_object_type(ObjectFlags::ANONYMOUS, global_this_symbol);
        self.value_symbol_links
            .get_by_id(&self.symbols, global_this_symbol)
            .resolved_type = global_this_object_type;
        // Initialize special types
        self.global_array_type =
            self.get_global_type("Array", 1 /*arity*/, true /*reportErrors*/);
        self.global_object_type =
            self.get_global_type("Object", 0 /*arity*/, true /*reportErrors*/);
        self.global_function_type =
            self.get_global_type("Function", 0 /*arity*/, true /*reportErrors*/);
        self.global_callable_function_type =
            self.get_global_strict_function_type("CallableFunction");
        self.global_newable_function_type = self.get_global_strict_function_type("NewableFunction");
        self.global_string_type =
            self.get_global_type("String", 0 /*arity*/, true /*reportErrors*/);
        self.global_number_type =
            self.get_global_type("Number", 0 /*arity*/, true /*reportErrors*/);
        self.global_boolean_type =
            self.get_global_type("Boolean", 0 /*arity*/, true /*reportErrors*/);
        self.global_reg_exp_type =
            self.get_global_type("RegExp", 0 /*arity*/, true /*reportErrors*/);
        let any_type = self.any_type;
        self.any_array_type = self.create_array_type(any_type);
        let auto_type = self.auto_type;
        self.auto_array_type = self.create_array_type(auto_type);
        if self.auto_array_type == self.empty_object_type {
            // autoArrayType is used as a marker, so even if global Array type is not defined, it needs to be a unique type
            self.auto_array_type =
                self.new_anonymous_type(SymbolId::NIL, SymbolTable::NIL, &[], &[], &[]);
        }
        self.global_readonly_array_type = self.get_global_type(
            "ReadonlyArray",
            1,     /*arity*/
            false, /*reportErrors*/
        );
        if self.global_readonly_array_type == self.empty_generic_type {
            self.global_readonly_array_type = self.global_array_type;
        }
        let global_readonly_array_type = self.global_readonly_array_type;
        self.any_readonly_array_type =
            self.create_type_from_generic_global_type(global_readonly_array_type, &[any_type]);
        self.global_this_type =
            self.get_global_type("ThisType", 1 /*arity*/, false /*reportErrors*/);
        // Now merge global ambient module declarations
        for symbol in ambient_module_symbols {
            self.merge_global_symbol(symbol);
        }
        self.merge_pattern_ambient_modules();
        // merge _nonglobal_ module augmentations.
        // this needs to be done after global symbol table is initialized to make sure that all ambient modules are indexed
        for list in &augmentations {
            for &augmentation in list {
                if !is_global_scope_augmentation(augmentation.parent()) {
                    self.merge_module_augmentation(augmentation);
                    // PERF (propfilt1): the merge can add names to the
                    // members of Object or Function in place.
                    self.rebuild_augment_filters();
                }
            }
        }
        // PORT: port-only. Lets `get_suggestion_for_symbol_name_lookup` keep
        // results from the globals table (see there).
        self.globals_complete = true;
    }

    // Go: checker/checker.go:1397 mergeGlobalSymbol
    pub fn merge_global_symbol(&mut self, symbol: SymbolId) {
        let name = self.sym(symbol).name.clone();
        // PERF: `get_name` finds the name by id with the interner's hash;
        // `get` hashes and compares the text. Same entry.
        let global_symbol = self.symbols.get_name(self.globals, &name);
        let merged;
        if global_symbol.is_some() {
            merged = self.merge_symbol(global_symbol, symbol, false /*unidirectional*/);
        } else {
            merged = self.get_merged_symbol(symbol);
        }
        self.symbols.set(self.globals, name, merged);
    }

    // Go: checker/checker.go:1409 mergePatternAmbientModules
    // Pattern ambient modules are merged together if they have the same pattern and identical import attributes type.
    // PORT: Go `module.Pattern.Text` is the prefix, the star and the suffix
    // (`PatternAmbientModule` stores the prefix and suffix).
    pub fn merge_pattern_ambient_modules(&mut self) {
        let mut groups_by_pattern: FxHashMap<String, Vec<usize>> = FxHashMap::default();
        let pattern_ambient_modules = self.pattern_ambient_modules.clone();
        let mut grouped: Vec<PatternAmbientModule> =
            Vec::with_capacity(pattern_ambient_modules.len());
        for module in &pattern_ambient_modules {
            let pattern_text = format!("{}*{}", module.pattern_prefix, module.pattern_suffix);
            let attributes_type = self.get_type_of_module_import_attributes(module.symbol);
            let mut group_index: isize = -1;
            let indexes = groups_by_pattern
                .get(&pattern_text)
                .cloned()
                .unwrap_or_default();
            for index in indexes {
                let other = self.get_type_of_module_import_attributes(grouped[index].symbol);
                if self.is_type_identical_to(attributes_type, other) {
                    group_index = index as isize;
                    break;
                }
            }
            if group_index == -1 {
                groups_by_pattern
                    .entry(pattern_text)
                    .or_default()
                    .push(grouped.len());
                grouped.push(PatternAmbientModule {
                    pattern_prefix: module.pattern_prefix.clone(),
                    pattern_suffix: module.pattern_suffix.clone(),
                    symbol: module.symbol,
                });
            } else {
                let group_index = group_index as usize;
                let target = grouped[group_index].symbol;
                grouped[group_index].symbol =
                    self.merge_symbol(target, module.symbol, false /*unidirectional*/);
            }
        }
        for module in &pattern_ambient_modules {
            let name = self.sym(module.symbol).name.clone();
            if self.symbols.get(self.globals, &name).is_some() {
                let merged = self.get_merged_symbol(module.symbol);
                self.symbols.set(self.globals, name, merged);
            }
        }
        self.pattern_ambient_modules = grouped;
    }

    // Go: checker/checker.go:1436 mergeModuleAugmentation
    pub fn merge_module_augmentation(&mut self, module_name: Node) {
        let module_node = module_name.parent();
        let module_augmentation_symbol = module_node.symbol();
        if self.sym(module_augmentation_symbol).declarations[0] != module_node {
            // this is a combined symbol for multiple augmentations within the same file.
            // its symbol already has accumulated information for all declarations
            // so we need to add it just once - do the work only for first declaration
            return;
        }
        if is_global_scope_augmentation(module_node) {
            let globals = self.globals;
            let exports = self.sym(module_augmentation_symbol).exports;
            self.merge_symbol_table(
                globals,
                exports,
                false,         /*unidirectional*/
                SymbolId::NIL, /*parent*/
            );
        } else {
            // find a module that about to be augmented
            // do not validate names of augmentations that are defined in ambient context
            let mut module_not_found_error: Option<&'static Message> = None;
            if !module_name
                .parent()
                .parent()
                .flags()
                .intersects(NodeFlags::AMBIENT)
            {
                module_not_found_error =
                    Some(diag::Invalid_module_name_in_augmentation_module_0_cannot_be_found);
            }
            // We ban import attributes on module augmentation declarations.
            let mut main_module = self.resolve_external_module_name_worker(
                module_name,
                module_name,
                module_not_found_error,
                false,       /*ignoreErrors*/
                true,        /*isForAugmentation*/
                TypeId::NIL, /*importAttributesType*/
            );
            if main_module.is_nil() {
                return;
            }
            // obtain item referenced by 'export='
            main_module =
                self.resolve_external_module_symbol(main_module, false /*dontResolveAlias*/);
            if self
                .sym(main_module)
                .flags
                .intersects(SymbolFlags::NAMESPACE)
            {
                // If we're merging an augmentation to a pattern ambient module, we want to
                // perform the merge unidirectionally from the augmentation ('a.foo') to
                // the pattern ('*.foo'), so that 'getMergedSymbol()' on a.foo gives you
                // all the exports both from the pattern and from the augmentation, but
                // 'getMergedSymbol()' on *.foo only gives you exports from *.foo.
                if (0..self.pattern_ambient_modules.len()).any(|i| {
                    let module_symbol = self.pattern_ambient_modules[i].symbol;
                    main_module == self.get_merged_symbol(module_symbol)
                }) {
                    let merged = self.merge_symbol(
                        module_augmentation_symbol,
                        main_module,
                        true, /*unidirectional*/
                    );
                    // moduleName will be a StringLiteral since this is not `declare global`.
                    let table = get_symbol_table(
                        &mut self.symbols,
                        &mut self.pattern_ambient_module_augmentations,
                    );
                    self.symbols.set(table, module_name.text(), merged);
                    let targets = get_symbol_table(
                        &mut self.symbols,
                        &mut self.pattern_ambient_module_augmentation_targets,
                    );
                    self.symbols.set(targets, module_name.text(), main_module);
                } else {
                    let main_exports = self.sym(main_module).exports;
                    let augmentation_exports = self.sym(module_augmentation_symbol).exports;
                    if self
                        .symbols
                        .get(main_exports, INTERNAL_SYMBOL_NAME_EXPORT_STAR)
                        .is_some()
                        && self.symbols.len(augmentation_exports) != 0
                    {
                        // We may need to merge the module augmentation's exports into the target symbols of the resolved exports
                        let resolved_exports = self.get_resolved_members_or_exports_of_symbol(
                            main_module,
                            MembersOrExportsResolutionKind::RESOLVED_EXPORTS,
                        );
                        for (key, value) in self.symbols.entries(augmentation_exports) {
                            let resolved = self.symbols.get(resolved_exports, &key);
                            let main_exports = self.sym(main_module).exports;
                            if resolved.is_some() && self.symbols.get(main_exports, &key).is_nil() {
                                self.merge_symbol(resolved, value, false /*unidirectional*/);
                            }
                        }
                    }
                    self.merge_symbol(
                        main_module,
                        module_augmentation_symbol,
                        false, /*unidirectional*/
                    );
                }
            } else {
                // moduleName will be a StringLiteral since this is not `declare global`.
                self.error(
                    module_name,
                    diag::Cannot_augment_module_0_because_it_resolves_to_a_non_module_entity,
                    args![module_name.text()],
                );
            }
        }
    }

    // Go: checker/checker.go:1493 addUndefinedToGlobalsOrErrorOnRedeclaration
    pub fn add_undefined_to_globals_or_error_on_redeclaration(&mut self) {
        let name = self.sym(self.undefined_symbol).name.clone();
        let target_symbol = self.symbols.get(self.globals, &name);
        if target_symbol.is_some() {
            let declarations = self.sym(target_symbol).declarations.clone();
            for declaration in declarations {
                if !is_type_declaration(declaration) {
                    self.add_diagnostic(create_diagnostic_for_node(
                        declaration,
                        diag::Declaration_name_conflicts_with_built_in_global_identifier_0,
                        args![name],
                    ));
                }
            }
        } else {
            let undefined_symbol = self.undefined_symbol;
            self.symbols.set(self.globals, name, undefined_symbol);
        }
    }

    // Go: checker/checker.go:1507 createNameResolver
    // PORT: Go binds checker methods as closures. Here each callback takes the
    // checker as its first argument (see `nameresolver.rs`). Go
    // `c.compilerOptions` is `program.Options()`, which is `self.compiler_options`.
    pub fn create_name_resolver(&self) -> NameResolver {
        let get_symbol_of_declaration: NameResolverGetSymbolOfDeclarationFn =
            Rc::new(|c: &mut Checker, node: Node| -> SymbolId {
                c.get_symbol_of_declaration(node)
            });
        let error: NameResolverErrorFn = Rc::new(
            |c: &mut Checker,
             location: Node,
             message: &'static Message,
             args: Vec<String>|
             -> Diagnostic { c.error(location, message, args) },
        );
        let lookup: NameResolverLookupFn = Rc::new(
            |c: &mut Checker,
             symbols: SymbolTable,
             key: TableKey<'_>,
             meaning: SymbolFlags|
             -> SymbolId { c.get_symbol_key(symbols, key, meaning) },
        );
        let symbol_referenced: NameResolverSymbolReferencedFn =
            Rc::new(|c: &mut Checker, symbol: SymbolId, meaning: SymbolFlags| {
                c.symbol_referenced(symbol, meaning)
            });
        let set_requires_scope_change_cache: NameResolverSetRequiresScopeChangeCacheFn =
            Rc::new(|c: &mut Checker, node: Node, value: Tristate| {
                c.set_requires_scope_change_cache(node, value)
            });
        let get_requires_scope_change_cache: NameResolverGetRequiresScopeChangeCacheFn =
            Rc::new(|c: &mut Checker, node: Node| -> Tristate {
                c.get_requires_scope_change_cache(node)
            });
        let on_property_with_invalid_initializer: NameResolverOnPropertyWithInvalidInitializerFn =
            Rc::new(
                |c: &mut Checker,
                 error_location: Node,
                 name: &str,
                 property_with_invalid_initializer: Node,
                 result: SymbolId|
                 -> bool {
                    c.check_and_report_error_for_invalid_initializer(
                        error_location,
                        name,
                        property_with_invalid_initializer,
                        result,
                    )
                },
            );
        let on_failed_to_resolve_symbol: NameResolverOnFailedToResolveSymbolFn = Rc::new(
            |c: &mut Checker,
             error_location: Node,
             name: &str,
             meaning: SymbolFlags,
             name_not_found_message: &'static Message| {
                c.on_failed_to_resolve_symbol(error_location, name, meaning, name_not_found_message)
            },
        );
        let on_successfully_resolved_symbol: NameResolverOnSuccessfullyResolvedSymbolFn = Rc::new(
            |c: &mut Checker,
             error_location: Node,
             result: SymbolId,
             meaning: SymbolFlags,
             last_location: Node,
             associated_declaration_for_containing_initializer_or_binding_name: Node,
             within_deferred_context: bool| {
                c.on_successfully_resolved_symbol(
                    error_location,
                    result,
                    meaning,
                    last_location,
                    associated_declaration_for_containing_initializer_or_binding_name,
                    within_deferred_context,
                )
            },
        );
        NameResolver {
            compiler_options: self.compiler_options,
            get_symbol_of_declaration: Some(get_symbol_of_declaration),
            error: Some(error),
            globals: self.globals,
            arguments_symbol: Cell::new(self.arguments_symbol),
            require_symbol: self.require_symbol,
            lookup: Some(lookup),
            symbol_referenced: Some(symbol_referenced),
            set_requires_scope_change_cache: Some(set_requires_scope_change_cache),
            get_requires_scope_change_cache: Some(get_requires_scope_change_cache),
            on_property_with_invalid_initializer: Some(on_property_with_invalid_initializer),
            on_failed_to_resolve_symbol: Some(on_failed_to_resolve_symbol),
            on_successfully_resolved_symbol: Some(on_successfully_resolved_symbol),
        }
    }

    // Go: checker/checker.go:1525 createNameResolverForSuggestion
    pub fn create_name_resolver_for_suggestion(&self) -> NameResolver {
        let get_symbol_of_declaration: NameResolverGetSymbolOfDeclarationFn =
            Rc::new(|c: &mut Checker, node: Node| -> SymbolId {
                c.get_symbol_of_declaration(node)
            });
        let error: NameResolverErrorFn = Rc::new(
            |c: &mut Checker,
             location: Node,
             message: &'static Message,
             args: Vec<String>|
             -> Diagnostic { c.error(location, message, args) },
        );
        let lookup: NameResolverLookupFn = Rc::new(
            |c: &mut Checker,
             symbols: SymbolTable,
             key: TableKey<'_>,
             meaning: SymbolFlags|
             -> SymbolId {
                c.get_suggestion_for_symbol_name_lookup(symbols, key, meaning)
            },
        );
        let symbol_referenced: NameResolverSymbolReferencedFn =
            Rc::new(|c: &mut Checker, symbol: SymbolId, meaning: SymbolFlags| {
                c.symbol_referenced(symbol, meaning)
            });
        let set_requires_scope_change_cache: NameResolverSetRequiresScopeChangeCacheFn =
            Rc::new(|c: &mut Checker, node: Node, value: Tristate| {
                c.set_requires_scope_change_cache(node, value)
            });
        let get_requires_scope_change_cache: NameResolverGetRequiresScopeChangeCacheFn =
            Rc::new(|c: &mut Checker, node: Node| -> Tristate {
                c.get_requires_scope_change_cache(node)
            });
        NameResolver {
            compiler_options: self.compiler_options,
            get_symbol_of_declaration: Some(get_symbol_of_declaration),
            error: Some(error),
            globals: self.globals,
            arguments_symbol: Cell::new(self.arguments_symbol),
            require_symbol: self.require_symbol,
            lookup: Some(lookup),
            symbol_referenced: Some(symbol_referenced),
            set_requires_scope_change_cache: Some(set_requires_scope_change_cache),
            get_requires_scope_change_cache: Some(get_requires_scope_change_cache),
            on_property_with_invalid_initializer: None,
            on_failed_to_resolve_symbol: None,
            on_successfully_resolved_symbol: None,
        }
    }

    // Go: checker/checker.go:1540 symbolReferenced
    pub fn symbol_referenced(&mut self, symbol: SymbolId, meaning: SymbolFlags) {
        self.symbol_reference_links.get(symbol).reference_kinds |= meaning;
    }

    // Go: checker/checker.go:1544 getRequiresScopeChangeCache
    pub fn get_requires_scope_change_cache(&mut self, node: Node) -> Tristate {
        self.node_links.get(node).declaration_requires_scope_change
    }

    // Go: checker/checker.go:1548 setRequiresScopeChangeCache
    pub fn set_requires_scope_change_cache(&mut self, node: Node, value: Tristate) {
        self.node_links.get(node).declaration_requires_scope_change = value;
    }

    // Go: checker/checker.go:1555 checkAndReportErrorForInvalidInitializer
    // The invalid initializer error is needed in two situation:
    // 1. When result is undefined, after checking for a missing "this."
    // 2. When result is defined
    pub fn check_and_report_error_for_invalid_initializer(
        &mut self,
        error_location: Node,
        name: &str,
        property_with_invalid_initializer: Node,
        result: SymbolId,
    ) -> bool {
        if !self.compiler_options.get_emit_standard_class_fields() {
            if error_location.is_some()
                && result.is_nil()
                && self.check_and_report_error_for_missing_prefix(error_location, name)
            {
                return true;
            }
            // We have a match, but the reference occurred within a property initializer and the identifier also binds
            // to a local variable in the constructor where the code will be emitted. Note that this is actually allowed
            // with emitStandardClassFields because the scope semantics are different.
            let prop = property_with_invalid_initializer;
            let prop_type = prop.type_();
            let message = if error_location.is_some()
                && prop_type.is_some()
                && prop_type.loc().contains_inclusive(error_location.pos())
            {
                diag::Type_of_instance_member_variable_0_cannot_reference_identifier_1_declared_in_the_constructor
            } else {
                diag::Initializer_of_instance_member_variable_0_cannot_reference_identifier_1_declared_in_the_constructor
            };
            self.error(
                error_location,
                message,
                args![declaration_name_to_string(prop.name()), name],
            );
            return true;
        }
        false
    }

    // Go: checker/checker.go:1573 checkAndReportErrorForMissingPrefix
    pub fn check_and_report_error_for_missing_prefix(
        &mut self,
        error_location: Node,
        name: &str,
    ) -> bool {
        if !is_identifier(error_location)
            || error_location.text() != name
            || is_type_reference_identifier(error_location)
            || is_in_type_query(error_location)
        {
            return false;
        }
        let container = self.get_this_container(
            error_location,
            false, /*includeArrowFunctions*/
            false, /*includeClassComputedPropertyName*/
        );
        let mut location = container;
        while location.parent().is_some() {
            if is_class_like(location.parent()) {
                let class_symbol = self.get_symbol_of_declaration(location.parent());
                if class_symbol.is_nil() {
                    break;
                }
                // Check to see if a static member exists.
                let constructor_type = self.get_type_of_symbol(class_symbol);
                if self.get_property_of_type(constructor_type, name).is_some() {
                    let class_name = self.symbol_to_string(class_symbol);
                    self.error(
                        error_location,
                        diag::Cannot_find_name_0_Did_you_mean_the_static_member_1_0,
                        args![name, class_name],
                    );
                    return true;
                }
                // No static member is present.
                // Check if we're in an instance method and look for a relevant instance member.
                if location == container && !is_static(location) {
                    let declared_type = self.get_declared_type_of_symbol(class_symbol);
                    let instance_type = self.ty(declared_type).as_interface_type().this_type;
                    // TODO: GH#18217
                    if self.get_property_of_type(instance_type, name).is_some() {
                        self.error(
                            error_location,
                            diag::Cannot_find_name_0_Did_you_mean_the_instance_member_this_0,
                            args![name],
                        );
                        return true;
                    }
                }
            }
            location = location.parent();
        }
        false
    }

    // Go: checker/checker.go:1605 onFailedToResolveSymbol
    pub fn on_failed_to_resolve_symbol(
        &mut self,
        error_location: Node,
        name: &str,
        meaning: SymbolFlags,
        name_not_found_message: &'static Message,
    ) {
        // The `const` in a `const` assertion (`x as const`) is a syntactic marker, not a real
        // type reference, and must never be resolved or reported as an unresolvable name.
        if is_const_type_reference_name(error_location) {
            return;
        }
        if error_location.is_some()
            && (error_location.parent().kind() == SyntaxKind::JsDocLink
                || self.check_and_report_error_for_missing_prefix(error_location, name)
                || self.check_and_report_error_for_extending_interface(error_location)
                || self.check_and_report_error_for_using_type_as_namespace(
                    error_location,
                    name,
                    meaning,
                )
                || self.check_and_report_error_for_exporting_primitive_type(error_location, name)
                || self.check_and_report_error_for_using_namespace_as_type_or_value(
                    error_location,
                    name,
                    meaning,
                )
                || self.check_and_report_error_for_using_type_as_value(
                    error_location,
                    name,
                    meaning,
                )
                || self.check_and_report_error_for_using_value_as_type(
                    error_location,
                    name,
                    meaning,
                ))
        {
            return;
        }
        let mut declaration_name = name.to_string();
        if error_location.is_some()
            && is_identifier(error_location)
            && error_location.text() == name
        {
            declaration_name = declaration_name_to_string(error_location); // use escape sequences from original file
        }
        // Report missing lib first
        let suggested_lib = self.get_suggested_lib_for_non_existent_name(name);
        if !suggested_lib.is_empty() {
            self.error(
                error_location,
                name_not_found_message,
                args![declaration_name, suggested_lib],
            );
            return;
        }
        // Then spelling suggestions
        let suggestion =
            self.get_suggested_symbol_for_nonexistent_symbol(error_location, name, meaning);
        if suggestion.is_some() && {
            let value_declaration = self.sym(suggestion).value_declaration;
            !(value_declaration.is_some()
                && is_ambient_module(value_declaration)
                && is_global_scope_augmentation(value_declaration))
        } {
            let suggestion_name = self.symbol_to_string(suggestion);
            let is_unchecked_js = self.is_unchecked_js_suggestion(
                error_location,
                suggestion,
                false, /*excludeClasses*/
            );
            let message = if meaning == SymbolFlags::NAMESPACE {
                diag::Cannot_find_namespace_0_Did_you_mean_1
            } else if is_unchecked_js {
                diag::Could_not_find_name_0_Did_you_mean_1
            } else {
                diag::Cannot_find_name_0_Did_you_mean_1
            };
            let mut diagnostic = new_diagnostic_for_node(
                error_location,
                message,
                args![declaration_name, suggestion_name],
            );
            let value_declaration = self.sym(suggestion).value_declaration;
            if value_declaration.is_some() {
                diagnostic.add_related_info(Some(new_diagnostic_for_node(
                    value_declaration,
                    diag::X_0_is_declared_here,
                    args![suggestion_name],
                )));
            }
            self.add_error_or_suggestion(!is_unchecked_js, diagnostic);
            return;
        }
        // And then fall back to unspecified "not found"
        self.error(
            error_location,
            name_not_found_message,
            args![declaration_name],
        );
    }

    // Go: checker/checker.go:1649 checkAndReportErrorForUsingTypeAsNamespace
    pub fn check_and_report_error_for_using_type_as_namespace(
        &mut self,
        error_location: Node,
        name: &str,
        meaning: SymbolFlags,
    ) -> bool {
        if meaning == SymbolFlags::NAMESPACE {
            let resolve_name = self.resolve_name.clone();
            let resolved = resolve_name(
                self,
                error_location,
                name,
                SymbolFlags::TYPE.without(SymbolFlags::NAMESPACE),
                None,  /*nameNotFoundMessage*/
                false, /*isUse*/
                false, /*excludeGlobals*/
            );
            let symbol = self.resolve_symbol(resolved);
            if symbol.is_some() {
                let parent = error_location.parent();
                if is_qualified_name(parent) {
                    debug_assert!(
                        parent.left() == error_location,
                        "Should only be resolving left side of qualified name as a namespace"
                    );
                    let prop_name = parent.right().text();
                    let declared_type = self.get_declared_type_of_symbol(symbol);
                    let prop_type = self.get_property_of_type(declared_type, prop_name);
                    if prop_type.is_some() {
                        self.error(
                            parent,
                            diag::Cannot_access_0_1_because_0_is_a_type_but_not_a_namespace_Did_you_mean_to_retrieve_the_type_of_the_property_1_in_0_with_0_1,
                            args![name, prop_name],
                        );
                        return true;
                    }
                }
                self.error(
                    error_location,
                    diag::X_0_only_refers_to_a_type_but_is_being_used_as_a_namespace_here,
                    args![name],
                );
                return true;
            }
        }
        false
    }

    // Go: checker/checker.go:1670 checkAndReportErrorForExportingPrimitiveType
    pub fn check_and_report_error_for_exporting_primitive_type(
        &mut self,
        error_location: Node,
        name: &str,
    ) -> bool {
        if is_primitive_type_name(name)
            && error_location.parent().kind() == SyntaxKind::ExportSpecifier
        {
            self.error(
                error_location,
                diag::Cannot_export_0_Only_local_declarations_can_be_exported_from_a_module,
                args![name],
            );
            return true;
        }
        false
    }
}

// Go: checker/checker.go:1678 isPrimitiveTypeName
pub fn is_primitive_type_name(s: &str) -> bool {
    s == "any" || s == "string" || s == "number" || s == "boolean" || s == "never" || s == "unknown"
}

impl Checker {
    // Go: checker/checker.go:1682 checkAndReportErrorForUsingNamespaceAsTypeOrValue
    pub fn check_and_report_error_for_using_namespace_as_type_or_value(
        &mut self,
        error_location: Node,
        name: &str,
        meaning: SymbolFlags,
    ) -> bool {
        if meaning.intersects(SymbolFlags::VALUE.without(SymbolFlags::TYPE)) {
            let resolve_name = self.resolve_name.clone();
            let resolved = resolve_name(
                self,
                error_location,
                name,
                SymbolFlags::NAMESPACE_MODULE,
                None,  /*nameNotFoundMessage*/
                false, /*isUse*/
                false, /*excludeGlobals*/
            );
            let symbol = self.resolve_symbol(resolved);
            if symbol.is_some() {
                // `export = ns` may legitimately reference a namespace; checkExportAssignment decides
                // whether that is an error, so don't report "cannot use namespace as a value" here.
                if !is_export_assignment_expression_name(error_location) {
                    self.error(
                        error_location,
                        diag::Cannot_use_namespace_0_as_a_value,
                        args![name],
                    );
                }
                return true;
            }
        } else if meaning.intersects(SymbolFlags::TYPE.without(SymbolFlags::VALUE)) {
            let resolve_name = self.resolve_name.clone();
            let resolved = resolve_name(
                self,
                error_location,
                name,
                SymbolFlags::MODULE,
                None,  /*nameNotFoundMessage*/
                false, /*isUse*/
                false, /*excludeGlobals*/
            );
            let symbol = self.resolve_symbol(resolved);
            if symbol.is_some() {
                self.error(
                    error_location,
                    diag::Cannot_use_namespace_0_as_a_type,
                    args![name],
                );
                return true;
            }
        }
        false
    }

    // Go: checker/checker.go:1703 checkAndReportErrorForUsingTypeAsValue
    pub fn check_and_report_error_for_using_type_as_value(
        &mut self,
        error_location: Node,
        name: &str,
        meaning: SymbolFlags,
    ) -> bool {
        if meaning.intersects(SymbolFlags::VALUE) {
            if is_primitive_type_name(name) {
                let grandparent = error_location.parent().parent();
                if grandparent.is_some()
                    && grandparent.parent().is_some()
                    && is_heritage_clause(grandparent)
                {
                    let heritage_kind = grandparent.token();
                    let container_kind = grandparent.parent().kind();
                    if container_kind == SyntaxKind::InterfaceDeclaration
                        && heritage_kind == SyntaxKind::ExtendsKeyword
                    {
                        self.error(
                            error_location,
                            diag::An_interface_cannot_extend_a_primitive_type_like_0_It_can_only_extend_other_named_object_types,
                            args![name],
                        );
                    } else if is_class_like(grandparent.parent())
                        && heritage_kind == SyntaxKind::ExtendsKeyword
                    {
                        self.error(
                            error_location,
                            diag::A_class_cannot_extend_a_primitive_type_like_0_Classes_can_only_extend_constructable_values,
                            args![name],
                        );
                    } else if is_class_like(grandparent.parent())
                        && heritage_kind == SyntaxKind::ImplementsKeyword
                    {
                        self.error(
                            error_location,
                            diag::A_class_cannot_implement_a_primitive_type_like_0_It_can_only_implement_other_named_object_types,
                            args![name],
                        );
                    }
                } else {
                    self.error(
                        error_location,
                        diag::X_0_only_refers_to_a_type_but_is_being_used_as_a_value_here,
                        args![name],
                    );
                }
                return true;
            }
            let resolve_name = self.resolve_name.clone();
            let resolved = resolve_name(
                self,
                error_location,
                name,
                SymbolFlags::TYPE.without(SymbolFlags::VALUE),
                None,  /*nameNotFoundMessage*/
                false, /*isUse*/
                false, /*excludeGlobals*/
            );
            let symbol = self.resolve_symbol(resolved);
            if symbol.is_some() {
                let all_flags = self.get_symbol_flags(symbol);
                if !all_flags.intersects(SymbolFlags::VALUE) {
                    // `export = SomeType` may legitimately reference a type-only name; checkExportAssignment
                    // decides whether that is an error, so don't report "used as a value" here.
                    if is_export_assignment_expression_name(error_location) {
                        return true;
                    }
                    if is_es2015_or_later_constructor_name(name) {
                        self.error(
                            error_location,
                            diag::X_0_only_refers_to_a_type_but_is_being_used_as_a_value_here_Do_you_need_to_change_your_target_library_Try_changing_the_lib_compiler_option_to_es2015_or_later,
                            args![name],
                        );
                    } else if self.maybe_mapped_type(error_location, symbol) {
                        self.error(
                            error_location,
                            diag::X_0_only_refers_to_a_type_but_is_being_used_as_a_value_here_Did_you_mean_to_use_1_in_0,
                            args![name, if name == "K" { "P" } else { "K" }],
                        );
                    } else {
                        self.error(
                            error_location,
                            diag::X_0_only_refers_to_a_type_but_is_being_used_as_a_value_here,
                            args![name],
                        );
                    }
                    return true;
                }
            }
        }
        false
    }
}

// Go: checker/checker.go:1745 isES2015OrLaterConstructorName
pub fn is_es2015_or_later_constructor_name(s: &str) -> bool {
    s == "Promise" || s == "Symbol" || s == "Map" || s == "WeakMap" || s == "Set" || s == "WeakSet"
}

impl Checker {
    // Go: checker/checker.go:1749 maybeMappedType
    pub fn maybe_mapped_type(&mut self, node: Node, symbol: SymbolId) -> bool {
        let mut node = node;
        loop {
            node = node.parent();
            if !(is_computed_property_name(node) || is_property_signature_declaration(node)) {
                break;
            }
        }
        if is_type_literal_node(node) && node.members().len() == 1 {
            let t = self.get_declared_type_of_symbol(symbol);
            return self.ty(t).flags.intersects(TypeFlags::UNION)
                && self.all_types_assignable_to_kind_ex(
                    t,
                    TypeFlags::STRING_OR_NUMBER_LITERAL,
                    true, /*strict*/
                );
        }
        false
    }

    // Go: checker/checker.go:1763 checkAndReportErrorForUsingValueAsType
    pub fn check_and_report_error_for_using_value_as_type(
        &mut self,
        error_location: Node,
        name: &str,
        meaning: SymbolFlags,
    ) -> bool {
        if meaning.intersects(SymbolFlags::TYPE.without(SymbolFlags::NAMESPACE)) {
            let resolve_name = self.resolve_name.clone();
            let resolved = resolve_name(
                self,
                error_location,
                name,
                !SymbolFlags::TYPE & SymbolFlags::VALUE,
                None,  /*nameNotFoundMessage*/
                false, /*isUse*/
                false, /*excludeGlobals*/
            );
            let symbol = self.resolve_symbol(resolved);
            if symbol.is_some() && !self.sym(symbol).flags.intersects(SymbolFlags::NAMESPACE) {
                self.error(
                    error_location,
                    diag::X_0_refers_to_a_value_but_is_being_used_as_a_type_here_Did_you_mean_typeof_0,
                    args![name],
                );
                return true;
            }
        }
        false
    }

    // Go: checker/checker.go:1774 getSuggestedLibForNonExistentName
    pub fn get_suggested_lib_for_non_existent_name(&mut self, name: &str) -> String {
        let feature_map = get_feature_map();
        if let Some(type_features) = feature_map.get(name) {
            return type_features[0].lib.to_string();
        }
        String::new()
    }

    // Go: checker/checker.go:1782 getSuggestedSymbolForNonexistentSymbol
    pub fn get_suggested_symbol_for_nonexistent_symbol(
        &mut self,
        location: Node,
        outer_name: &str,
        meaning: SymbolFlags,
    ) -> SymbolId {
        let resolve_name_for_symbol_suggestion = self.resolve_name_for_symbol_suggestion.clone();
        resolve_name_for_symbol_suggestion(
            self, location, outer_name, meaning, None,  /*nameNotFoundMessage*/
            false, /*isUse*/
            false, /*excludeGlobals*/
        )
    }

    // Go: checker/checker.go:1786 primitiveTypeAliasSuggestions
    // Go: checker/checker.go:1804 getPrimitiveTypeAliasSuggestions
    // PORT: Go builds the six transient suggestion symbols once per process
    // (`sync.OnceValue`) and yields those whose builtin name is present in
    // `symbols`, in random map order. Symbols live in the checker's arena
    // here, so they are created in this checker on each call, in the listed
    // order, and only for builtins that are present.
    pub fn get_primitive_type_alias_suggestions(&mut self, symbols: SymbolTable) -> Vec<SymbolId> {
        const PRIMITIVE_TYPE_ALIAS_SUGGESTIONS: [(&str, &str); 6] = [
            ("string", "String"),
            ("number", "Number"),
            ("boolean", "Boolean"),
            ("object", "Object"),
            ("bigint", "BigInt"),
            ("symbol", "Symbol"),
        ];
        let mut result = Vec::new();
        for (primitive, builtin) in PRIMITIVE_TYPE_ALIAS_SUGGESTIONS {
            if self.symbols.get(symbols, builtin).is_some() {
                result.push(
                    self.symbols
                        .new_symbol(SymbolFlags::TYPE_ALIAS | SymbolFlags::TRANSIENT, primitive),
                );
            }
        }
        result
    }

    // Go: checker/checker.go:1816 getSuggestionForSymbolNameLookup
    // PORT: the name is a `TableKey` because this is a `NameResolver` lookup
    // callback (see `NameResolverLookupFn`).
    // PERF: the globals table gives the same suggestion for the same name and
    // meaning while its names and the flags and declarations of its symbols
    // stay the same, so the result is kept in `global_spelling_suggestions`
    // (port-only).
    // - Names: the table is complete after `initialize_checker`. During it,
    //   names are added and flags change (a non-global module augmentation
    //   adds flags to a merged global namespace that an `export =` names), and
    //   suggestions are asked too (a failed `export =`, the import attribute
    //   types of pattern ambient modules). So the memo is read and filled only
    //   when `globals_complete` is set. Before that, each lookup scans the
    //   table, as in Go.
    // - Flags and declarations: after `initialize_checker`, only
    //   `merge_symbol` changes a symbol that already exists:
    //   `combine_symbol_tables` merges a late-bound member into a transient
    //   early member in place. When `globalThis` is also a class with a
    //   static computed member, its early exports are the globals table, so a
    //   global gets new flags. Each entry keeps the `merge_version` from
    //   before its scan and is used only while `merge_version` is the same. A
    //   merge during the scan (from an alias candidate's resolution) makes the
    //   entry old at once.
    // - Aliases: while an alias candidate's target is being resolved,
    //   `try_resolve_alias` gives nil and the candidate is left out. Such a
    //   result is not kept. A target can still change after a kept scan
    //   read it, with no merge: a nested resolution (after
    //   `get_resolved_signature` moves `resolution_start`, Go
    //   checker.go:8591) sets it, and the outer one (checker.go:16603 to
    //   16606) writes it again, so a kept entry can have seen `unknown`
    //   where a late member is now. Both are Property-like value symbols,
    //   so no meaning that a name lookup asks for tells them apart (spell1
    //   skeptic case c17 matches Go).
    // The primitive type alias symbols are made on each call, as before, so a
    // hit makes the same symbols as a miss.
    // A project where a test runner's types are missing asks this for
    // `expect` thousands of times (nestjs-graphql: 24% of the check).
    pub fn get_suggestion_for_symbol_name_lookup(
        &mut self,
        symbols: SymbolTable,
        name: TableKey<'_>,
        meaning: SymbolFlags,
    ) -> SymbolId {
        let symbol = self.get_symbol_key(symbols, name, meaning);
        if symbol.is_some() {
            return symbol;
        }
        let memo_key = match name {
            TableKey::Name(name) if symbols == self.globals && self.globals_complete => {
                Some((name.id(), meaning))
            }
            _ => None,
        };
        let merge_version = self.merge_version;
        let memo = memo_key.and_then(|key| match self.global_spelling_suggestions.get(&key) {
            Some(&(version, memo)) if version == merge_version => Some(memo),
            _ => None,
        });
        // PORT: Go `core.ConcatenateSeq(maps.Values(symbols), extras)` -> one Vec.
        let mut candidates = if memo.is_some() {
            Vec::new()
        } else {
            self.symbols.values(symbols)
        };
        let table_len = candidates.len();
        if meaning.intersects(SymbolFlags::GLOBAL_LOOKUP) {
            let extras = self.get_primitive_type_alias_suggestions(symbols);
            candidates.extend(extras);
        }
        match memo {
            Some(GlobalSpellingSuggestion::NotFound) => return SymbolId::NIL,
            Some(GlobalSpellingSuggestion::Table(symbol)) => return symbol,
            Some(GlobalSpellingSuggestion::Extra(index)) => return candidates[index],
            None => {}
        }
        let mut alias_in_progress = false;
        let named = self.get_spelling_candidate_names(&candidates, meaning, &mut alias_in_progress);
        let best = self.pick_spelling_suggestion(name.text(), named);
        if let Some(key) = memo_key
            && !alias_in_progress
        {
            let memo = if best.is_nil() {
                GlobalSpellingSuggestion::NotFound
            } else if let Some(index) = candidates[table_len..].iter().position(|&s| s == best) {
                GlobalSpellingSuggestion::Extra(index)
            } else {
                GlobalSpellingSuggestion::Table(best)
            };
            self.global_spelling_suggestions
                .insert(key, (merge_version, memo));
        }
        best
    }

    // Go: checker/checker.go:1841 getSpellingSuggestionForName
    // Given a name and a list of symbols whose names are *not* equal to the name, return a spelling suggestion if there is
    // one that is close enough. Names less than length 3 only check for case-insensitive equality, not levenshtein distance.
    //
    // If there is a candidate that's the same except for case, return that.
    // If there is a candidate that's within one edit of the name, return that.
    // Otherwise, return the candidate with the smallest Levenshtein distance,
    //
    // Except for candidates:
    //   - With no name
    //   - Whose meaning doesn't match the `meaning` parameter.
    //   - Whose length differs from the target name by more than 0.34 of the length of the name.
    //   - Whose levenshtein distance is more than 0.4 of the length of the name (0.4 allows 1 substitution/transposition
    //     for every 5 characters, and 1 insertion/deletion at 3 characters)
    // PORT: Go `iter.Seq[*ast.Symbol]` -> slice. Go computes each candidate
    // name lazily inside `core.GetSpellingSuggestion`; here all names are
    // computed first (same order), because the name callback and the
    // `compareSymbols` callback both need `&mut self`. The selection result
    // is the same.
    // PERF: the names are `&'static str` borrows of interned or node text,
    // so no candidate makes a `String`.
    pub fn get_spelling_suggestion_for_name(
        &mut self,
        name: &str,
        symbols: &[SymbolId],
        meaning: SymbolFlags,
    ) -> SymbolId {
        let named = self.get_spelling_candidate_names(symbols, meaning, &mut false);
        self.pick_spelling_suggestion(name, named)
    }

    // PORT: the first half of Go getSpellingSuggestionForName: each
    // candidate with its `getCandidateName` result, in order.
    // `alias_in_progress` is set when an alias candidate was left out because
    // it is being resolved (see `get_suggestion_for_symbol_name_lookup`).
    fn get_spelling_candidate_names(
        &mut self,
        symbols: &[SymbolId],
        meaning: SymbolFlags,
        alias_in_progress: &mut bool,
    ) -> Vec<(SymbolId, &'static str)> {
        symbols
            .iter()
            .map(|&candidate| {
                let candidate_name = self.get_candidate_name_for_spelling_suggestion(
                    candidate,
                    meaning,
                    alias_in_progress,
                );
                (candidate, candidate_name)
            })
            .collect()
    }

    // PORT: the second half of Go getSpellingSuggestionForName:
    // `core.GetSpellingSuggestion` with `c.compareSymbols`.
    fn pick_spelling_suggestion(
        &self,
        name: &str,
        named: Vec<(SymbolId, &'static str)>,
    ) -> SymbolId {
        let (best, _) = get_spelling_suggestion(
            name,
            named,
            |entry: &(SymbolId, &'static str)| entry.1,
            |a: &(SymbolId, &'static str), b: &(SymbolId, &'static str)| {
                self.compare_symbols_worker(a.0, b.0)
            },
        );
        best
    }

    // Go: checker/checker.go:1842 getSpellingSuggestionForName.getCandidateName
    // PORT: Go closure inside getSpellingSuggestionForName.
    fn get_candidate_name_for_spelling_suggestion(
        &mut self,
        candidate: SymbolId,
        meaning: SymbolFlags,
        alias_in_progress: &mut bool,
    ) -> &'static str {
        // PERF: the text of `symbol_name` without its `String` copy. Both
        // sources (a private identifier's text and the interned symbol name)
        // live until the process ends.
        let candidate_name: &'static str = {
            let s = self.sym(candidate);
            if s.value_declaration.is_some()
                && is_private_identifier_class_element_declaration(s.value_declaration)
            {
                s.value_declaration.name().text()
            } else {
                s.name.as_str()
            }
        };
        // PORT: Go compares the first byte with '\xFE', which is
        // INTERNAL_SYMBOL_NAME_PREFIX in the port form.
        if candidate_name.is_empty()
            || candidate_name.starts_with('"')
            || candidate_name.starts_with(INTERNAL_SYMBOL_NAME_PREFIX)
        {
            return "";
        }
        if self.sym(candidate).flags.intersects(meaning) {
            return candidate_name;
        }
        if self.sym(candidate).flags.intersects(SymbolFlags::ALIAS) {
            let alias = self.try_resolve_alias(candidate);
            if alias.is_some() && self.sym(alias).flags.intersects(meaning) {
                return candidate_name;
            }
            // `try_resolve_alias` gives nil only while the alias is being resolved.
            *alias_in_progress |= alias.is_nil();
        }
        ""
    }

    // Go: checker/checker.go:1861 onSuccessfullyResolvedSymbol
    pub fn on_successfully_resolved_symbol(
        &mut self,
        error_location: Node,
        result: SymbolId,
        meaning: SymbolFlags,
        last_location: Node,
        associated_declaration_for_containing_initializer_or_binding_name: Node,
        within_deferred_context: bool,
    ) {
        let name = self.sym(result).name.clone();
        let is_in_external_module = last_location.is_some()
            && is_source_file(last_location)
            && is_external_or_common_js_module(last_location);
        // Only check for block-scoped variable if we have an error location and are looking for the
        // name with variable meaning
        //      For example,
        //          declare module foo {
        //              interface bar {}
        //          }
        //      const foo/*1*/: foo/*2*/.bar;
        // The foo at /*1*/ and /*2*/ will share same symbol with two meanings:
        // block-scoped variable and namespace module. However, only when we
        // try to resolve name in /*1*/ which is used in variable position,
        // we want to check for block-scoped
        if error_location.is_some()
            && (meaning.intersects(SymbolFlags::BLOCK_SCOPED_VARIABLE)
                || meaning.intersects(SymbolFlags::CLASS | SymbolFlags::ENUM)
                    && (meaning & SymbolFlags::VALUE) == SymbolFlags::VALUE)
        {
            let export_or_local_symbol = self.get_export_symbol_of_value_symbol_if_exported(result);
            if self.sym(export_or_local_symbol).flags.intersects(
                SymbolFlags::BLOCK_SCOPED_VARIABLE | SymbolFlags::CLASS | SymbolFlags::ENUM,
            ) {
                self.check_resolved_block_scoped_variable(export_or_local_symbol, error_location);
            }
        }
        // If we're in an external module, we can't reference value symbols created from UMD export declarations
        if is_in_external_module
            && (meaning & SymbolFlags::VALUE) == SymbolFlags::VALUE
            && !error_location.flags().intersects(NodeFlags::JS_DOC)
        {
            let merged = self.get_merged_symbol(result);
            let declarations = self.sym(merged).declarations.clone();
            if !declarations.is_empty()
                && declarations.iter().all(|&d| {
                    is_namespace_export_declaration(d)
                        || is_source_file(d) && file_bind_data(d).global_exports.is_some()
                })
            {
                self.error_or_suggestion(
                    self.compiler_options.allow_umd_global_access != Tristate::True,
                    error_location,
                    diag::X_0_refers_to_a_UMD_global_but_the_current_file_is_a_module_Consider_adding_an_import_instead,
                    args![name],
                );
            }
        }
        // If we're in a parameter initializer or binding name, we can't reference the values of the parameter whose initializer we're within or parameters to the right
        let assoc = associated_declaration_for_containing_initializer_or_binding_name;
        if assoc.is_some()
            && !within_deferred_context
            && (meaning & SymbolFlags::VALUE) == SymbolFlags::VALUE
        {
            let late_bound = self.get_late_bound_symbol(result);
            let candidate = self.get_merged_symbol(late_bound);
            let root = get_root_declaration(assoc);
            // A parameter initializer or binding pattern initializer within a parameter cannot refer to itself
            if candidate == self.get_symbol_of_declaration(assoc) {
                self.error(
                    error_location,
                    diag::Parameter_0_cannot_reference_itself,
                    args![declaration_name_to_string(assoc.name())],
                );
            } else {
                let candidate_value_declaration = self.sym(candidate).value_declaration;
                if candidate_value_declaration.is_some()
                    && candidate_value_declaration.pos() > assoc.pos()
                    && root.parent().locals().is_some()
                    && {
                        let candidate_name = self.sym(candidate).name.clone();
                        self.get_symbol(root.parent().locals(), &candidate_name, meaning)
                            == candidate
                    }
                {
                    self.error(
                        error_location,
                        diag::Parameter_0_cannot_reference_identifier_1_declared_after_it,
                        args![
                            declaration_name_to_string(assoc.name()),
                            declaration_name_to_string(error_location)
                        ],
                    );
                }
            }
        }
        if error_location.is_some()
            && meaning.intersects(SymbolFlags::VALUE)
            && self.sym(result).flags.intersects(SymbolFlags::ALIAS)
            && !self.sym(result).flags.intersects(SymbolFlags::VALUE)
            && !is_valid_type_only_alias_use_site(error_location)
        {
            let type_only_declaration =
                self.get_type_only_alias_declaration_ex(result, SymbolFlags::VALUE);
            if type_only_declaration.is_some() {
                let message = if node_kind_is(
                    type_only_declaration,
                    &[
                        SyntaxKind::ExportSpecifier,
                        SyntaxKind::ExportDeclaration,
                        SyntaxKind::NamespaceExport,
                    ],
                ) {
                    diag::X_0_cannot_be_used_as_a_value_because_it_was_exported_using_export_type
                } else {
                    diag::X_0_cannot_be_used_as_a_value_because_it_was_imported_using_import_type
                };
                // PORT: Go passes the diagnostic returned by `c.error` and mutates it
                // in place. Here it is built, given the related info, then added.
                let diagnostic = new_diagnostic_for_node(error_location, message, args![name]);
                let diagnostic = self.add_type_only_declaration_related_info(
                    diagnostic,
                    type_only_declaration,
                    &name,
                );
                self.add_diagnostic(diagnostic);
            }
        }
        // Look at 'compilerOptions.isolatedModules' and not 'getIsolatedModules(...)' (which considers 'verbatimModuleSyntax')
        // here because 'verbatimModuleSyntax' will already have an error for importing a type without 'import type'.
        if self.compiler_options.isolated_modules == Tristate::True
            && result.is_some()
            && is_in_external_module
            && (meaning & SymbolFlags::VALUE) == SymbolFlags::VALUE
        {
            let globals = self.globals;
            let is_global = self.get_symbol(globals, &name, meaning) == result;
            let mut non_value_symbol = SymbolId::NIL;
            if is_global && is_source_file(last_location) {
                non_value_symbol =
                    self.get_symbol(last_location.locals(), &name, !SymbolFlags::VALUE);
            }
            if non_value_symbol.is_some() {
                let import_decl = self
                    .sym(non_value_symbol)
                    .declarations
                    .iter()
                    .copied()
                    .find(|&d| {
                        node_kind_is(
                            d,
                            &[
                                SyntaxKind::ImportSpecifier,
                                SyntaxKind::ImportClause,
                                SyntaxKind::NamespaceImport,
                                SyntaxKind::ImportEqualsDeclaration,
                            ],
                        )
                    })
                    .unwrap_or(Node::NIL);
                if import_decl.is_some() && !is_type_only_import_declaration(import_decl) {
                    self.error(
                        import_decl,
                        diag::Import_0_conflicts_with_global_value_used_in_this_file_so_must_be_declared_with_a_type_only_import_when_isolatedModules_is_enabled,
                        args![name],
                    );
                }
            }
        }
    }

    // Go: checker/checker.go:1929 checkResolvedBlockScopedVariable
    pub fn check_resolved_block_scoped_variable(&mut self, result: SymbolId, error_location: Node) {
        let result_flags = self.sym(result).flags;
        debug_assert!(
            result_flags.intersects(SymbolFlags::BLOCK_SCOPED_VARIABLE)
                || result_flags.intersects(SymbolFlags::CLASS)
                || result_flags.intersects(SymbolFlags::ENUM)
        );
        if result_flags.intersects(
            SymbolFlags::FUNCTION | SymbolFlags::FUNCTION_SCOPED_VARIABLE | SymbolFlags::ASSIGNMENT,
        ) && result_flags.intersects(SymbolFlags::CLASS)
        {
            // constructor functions aren't block scoped
            return;
        }
        // Block-scoped variables cannot be used before their definition
        let declaration = self
            .sym(result)
            .declarations
            .iter()
            .copied()
            .find(|&d| is_block_or_catch_scoped(d) || is_class_like(d) || is_enum_declaration(d))
            .unwrap_or(Node::NIL);
        if declaration.is_nil() {
            panic!("checkResolvedBlockScopedVariable could not find block-scoped declaration");
        }
        if !declaration.flags().intersects(NodeFlags::AMBIENT)
            && !self.is_block_scoped_name_declared_before_use(declaration, error_location)
        {
            // PORT: Go keeps the `*ast.Diagnostic` returned by `c.error` and adds
            // related info afterwards. Here the diagnostic is built, finished,
            // then added.
            let mut diagnostic: Option<Diagnostic> = None;
            let declaration_name = declaration_name_to_string(get_name_of_declaration(declaration));
            if result_flags.intersects(SymbolFlags::BLOCK_SCOPED_VARIABLE) {
                diagnostic = Some(new_diagnostic_for_node(
                    error_location,
                    diag::Block_scoped_variable_0_used_before_its_declaration,
                    args![declaration_name],
                ));
            } else if result_flags.intersects(SymbolFlags::CLASS) {
                diagnostic = Some(new_diagnostic_for_node(
                    error_location,
                    diag::Class_0_used_before_its_declaration,
                    args![declaration_name],
                ));
            } else if result_flags.intersects(SymbolFlags::REGULAR_ENUM) {
                diagnostic = Some(new_diagnostic_for_node(
                    error_location,
                    diag::Enum_0_used_before_its_declaration,
                    args![declaration_name],
                ));
            } else {
                debug_assert!(result_flags.intersects(SymbolFlags::CONST_ENUM));
                if self.compiler_options.get_isolated_modules() {
                    diagnostic = Some(new_diagnostic_for_node(
                        error_location,
                        diag::Enum_0_used_before_its_declaration,
                        args![declaration_name],
                    ));
                }
            }
            if let Some(mut diagnostic) = diagnostic {
                diagnostic.add_related_info(Some(create_diagnostic_for_node(
                    declaration,
                    diag::X_0_is_declared_here,
                    args![declaration_name],
                )));
                self.add_diagnostic(diagnostic);
            }
        }
    }

    // Go: checker/checker.go:1963 isBlockScopedNameDeclaredBeforeUse
    pub fn is_block_scoped_name_declared_before_use(
        &mut self,
        declaration: Node,
        usage: Node,
    ) -> bool {
        let declaration_file = get_source_file_of_node(declaration);
        let use_file = get_source_file_of_node(usage);
        if declaration_file != use_file {
            // nodes are in different files and order cannot be determined
            return true;
        }
        // deferred usage in a type context is always OK regardless of the usage position:
        if usage.flags().intersects(NodeFlags::JS_DOC)
            || is_in_type_query(usage)
            || self.is_in_ambient_or_type_node(usage)
        {
            return true;
        }
        // PERF: Go reads `declContainer` before the two early returns above.
        // It is a walk up the parents that changes nothing, so it is read
        // after them.
        let decl_container = get_enclosing_block_scope_container(declaration);
        if declaration.pos() <= usage.pos()
            && !(is_property_declaration(declaration)
                && is_this_property(usage.parent())
                && declaration.initializer().is_nil()
                && !is_exclamation_token(declaration.postfix_token()))
        {
            // declaration is before usage
            if declaration.kind() == SyntaxKind::BindingElement {
                // still might be illegal if declaration and usage are both binding elements (eg var [a = b, b = b] = [1, 2])
                let error_binding_element = find_ancestor_kind(usage, SyntaxKind::BindingElement);
                if error_binding_element.is_some() {
                    return find_ancestor(error_binding_element, is_binding_element)
                        != find_ancestor(declaration, is_binding_element)
                        || declaration.pos() < error_binding_element.pos();
                }
                // or it might be illegal if usage happens before parent variable is declared (eg var [a] = a)
                return self.is_block_scoped_name_declared_before_use(
                    find_ancestor_kind(declaration, SyntaxKind::VariableDeclaration),
                    usage,
                );
            } else if declaration.kind() == SyntaxKind::VariableDeclaration {
                // still might be illegal if usage is in the initializer of the variable declaration (eg var a = a)
                return !is_immediately_used_in_initializer_of_block_scoped_variable(
                    declaration,
                    usage,
                    decl_container,
                );
            } else if is_class_like(declaration) {
                // still might be illegal if the usage is within a computed property name in the class (eg class A { static p = "a"; [A.p]() {} })
                // or when used within a decorator in the class (e.g. `@dec(A.x) class A { static x = "x" }`),
                // except when used in a function that is not an IIFE (e.g., `@dec(() => A.x) class A { ... }`)
                let mut container = usage;
                while container.is_some() && container != declaration {
                    if is_computed_property_name(container)
                        && container.parent().parent() == declaration
                        || !self.legacy_decorators
                            && is_decorator(container)
                            && (container.parent() == declaration
                                || is_method_declaration(container.parent())
                                    && container.parent().parent() == declaration
                                || is_accessor(container.parent())
                                    && container.parent().parent() == declaration
                                || is_property_declaration(container.parent())
                                    && container.parent().parent() == declaration
                                || is_parameter_declaration(container.parent())
                                    && container.parent().parent().parent() == declaration)
                    {
                        break;
                    }
                    container = container.parent();
                }
                if container.is_nil() || container == declaration {
                    return true;
                }
                if !self.legacy_decorators && is_decorator(container) {
                    let mut n = usage;
                    while n.is_some() && n != container {
                        if is_function_like(n)
                            && get_immediately_invoked_function_expression(n).is_nil()
                        {
                            break;
                        }
                        n = n.parent();
                    }
                    return n.is_some() && n != container;
                }
                return false;
            } else if is_property_declaration(declaration) {
                // still might be illegal if a self-referencing property initializer (eg private x = this.x)
                return !is_property_immediately_referenced_within_declaration(
                    declaration,
                    usage,
                    false, /*stopAtAnyPropertyDeclaration*/
                );
            } else if is_parameter_property_declaration(declaration, declaration.parent()) {
                // foo = this.bar is illegal in emitStandardClassFields when bar is a parameter property
                return !(self.emit_standard_class_fields
                    && get_containing_class(declaration) == get_containing_class(usage)
                    && self.is_used_in_function_or_instance_property(
                        usage,
                        declaration,
                        decl_container,
                    ));
            }
            return true;
        }
        // declaration is after usage, but it can still be legal if usage is deferred:
        // 1. inside an export specifier
        // 2. inside a function
        // 3. inside an instance property initializer, a reference to a non-instance property
        //    (except when emitStandardClassFields: true and the reference is to a parameter property)
        // 4. inside a static property initializer, a reference to a static method in the same class
        // 5. inside a TS export= declaration (since we will move the export statement during emit to avoid TDZ)
        if is_export_specifier(usage.parent())
            || is_export_assignment(usage.parent()) && usage.parent().is_export_equals()
        {
            // export specifiers do not use the variable, they only make it available for use
            return true;
        }
        // When resolving symbols for exports, the `usage` location passed in can be the export site directly
        if is_export_assignment(usage) && usage.is_export_equals() {
            return true;
        }
        if self.is_used_in_function_or_instance_property(usage, declaration, decl_container) {
            if self.emit_standard_class_fields
                && get_containing_class(declaration).is_some()
                && (is_property_declaration(declaration)
                    || is_parameter_property_declaration(declaration, declaration.parent()))
            {
                return !is_property_immediately_referenced_within_declaration(
                    declaration,
                    usage,
                    true, /*stopAtAnyPropertyDeclaration*/
                );
            }
            return true;
        }
        false
    }

    // Go: checker/checker.go:2052 isUsedInFunctionOrInstanceProperty
    pub fn is_used_in_function_or_instance_property(
        &mut self,
        usage: Node,
        declaration: Node,
        decl_container: Node,
    ) -> bool {
        find_ancestor_or_quit(usage, |current: Node| -> FindAncestorResult {
            if current == decl_container {
                return FindAncestorResult::FIND_ANCESTOR_QUIT;
            }
            if is_function_like(current) {
                return to_find_ancestor_result(
                    get_immediately_invoked_function_expression(current).is_nil(),
                );
            }
            if is_class_static_block_declaration(current) {
                return to_find_ancestor_result(declaration.pos() < usage.pos());
            }

            if current.parent().is_some() && is_property_declaration(current.parent()) {
                let property_declaration = current.parent();
                let initializer_of_property = property_declaration.initializer() == current;
                if initializer_of_property {
                    if is_static(current.parent()) {
                        if is_method_declaration(declaration) {
                            return FindAncestorResult::FIND_ANCESTOR_TRUE;
                        }
                        if is_property_declaration(declaration)
                            && get_containing_class(usage) == get_containing_class(declaration)
                        {
                            let prop_name = declaration.name();
                            if is_identifier(prop_name) || is_private_identifier(prop_name) {
                                let declaration_symbol =
                                    self.get_symbol_of_declaration(declaration);
                                let t = self.get_type_of_symbol(declaration_symbol);
                                let static_blocks: Vec<Node> = declaration
                                    .parent()
                                    .members()
                                    .iter()
                                    .filter(|&m| is_class_static_block_declaration(m))
                                    .collect();
                                if self.is_property_initialized_in_static_blocks(
                                    prop_name,
                                    t,
                                    &static_blocks,
                                    declaration.parent().pos(),
                                    current.pos(),
                                ) {
                                    return FindAncestorResult::FIND_ANCESTOR_TRUE;
                                }
                            }
                        }
                    } else {
                        let is_declaration_instance_property =
                            is_property_declaration(declaration) && !is_static(declaration);
                        if !is_declaration_instance_property
                            || get_containing_class(usage) != get_containing_class(declaration)
                        {
                            return FindAncestorResult::FIND_ANCESTOR_TRUE;
                        }
                    }
                }
            }

            if current.parent().is_some() && is_decorator(current.parent()) {
                let decorator = current.parent();
                if decorator.expression() == current {
                    if is_parameter_declaration(decorator.parent()) {
                        if self.is_used_in_function_or_instance_property(
                            decorator.parent().parent().parent(),
                            declaration,
                            decl_container,
                        ) {
                            return FindAncestorResult::FIND_ANCESTOR_TRUE;
                        }
                        return FindAncestorResult::FIND_ANCESTOR_QUIT;
                    }
                    if is_method_declaration(decorator.parent()) {
                        if self.is_used_in_function_or_instance_property(
                            decorator.parent().parent(),
                            declaration,
                            decl_container,
                        ) {
                            return FindAncestorResult::FIND_ANCESTOR_TRUE;
                        }
                        return FindAncestorResult::FIND_ANCESTOR_QUIT;
                    }
                }
            }

            FindAncestorResult::FIND_ANCESTOR_FALSE
        })
        .is_some()
    }
}

/// A spelling suggestion from the globals table, kept by
/// `get_suggestion_for_symbol_name_lookup`.
#[derive(Clone, Copy, Debug)]
pub enum GlobalSpellingSuggestion {
    /// No candidate is close enough.
    NotFound,
    /// A symbol of the globals table.
    Table(SymbolId),
    /// The symbol at this index of `get_primitive_type_alias_suggestions`,
    /// which makes new symbols on each call.
    Extra(usize),
}
