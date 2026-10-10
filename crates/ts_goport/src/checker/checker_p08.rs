//! Port of Go `checker/checker.go` lines 6626-7527 (checker-08).
//! Iterator result types, alias checks, type alias checks, unused
//! identifier reporting, quick expression types and non-null checks.

use crate::prelude::*;

impl Checker {
    // Gets the *yield* and *return* types of an `IteratorResult`-like type.
    //
    // If we are unable to determine a *yield* or a *return* type, `noIterationTypes` is
    // returned to indicate to the caller that it should handle the error. Otherwise, an
    // `IterationTypes` record is returned.
    // Go: checker/checker.go:6819 getIterationTypesOfIteratorResult
    pub fn get_iteration_types_of_iterator_result(&mut self, t: TypeId) -> IterationTypes {
        if self.is_type_any(t) {
            return IterationTypes {
                yield_type: self.any_type,
                return_type: self.any_type,
                next_type: self.any_type,
            };
        }
        // As an optimization, if the type is an instantiation of one of the global `IteratorYieldResult<T>`
        // or `IteratorReturnResult<TReturn>` types, then just grab its type argument.
        // PORT: Go `c.getGlobalIteratorYieldResultType` is a stored func field
        // (`Rc<dyn Fn(&mut Checker) -> TypeId>`).
        let get_global_iterator_yield_result_type =
            self.get_global_iterator_yield_result_type.clone();
        let global_yield = get_global_iterator_yield_result_type(self);
        if self.is_reference_to_type(t, global_yield) {
            let arg = self.get_type_arguments(t)[0];
            return IterationTypes {
                yield_type: arg,
                return_type: TypeId::NIL,
                next_type: TypeId::NIL,
            };
        }
        let get_global_iterator_return_result_type =
            self.get_global_iterator_return_result_type.clone();
        let global_return = get_global_iterator_return_result_type(self);
        if self.is_reference_to_type(t, global_return) {
            let arg = self.get_type_arguments(t)[0];
            return IterationTypes {
                yield_type: TypeId::NIL,
                return_type: arg,
                next_type: TypeId::NIL,
            };
        }
        // Choose any constituents that can produce the requested iteration type.
        let yield_iterator_result = self.filter_type(t, &mut |c: &mut Checker, t: TypeId| {
            c.is_yield_iterator_result(t)
        });
        let mut yield_type = TypeId::NIL;
        if yield_iterator_result != self.never_type {
            yield_type = self.get_type_of_property_of_type(
                yield_iterator_result,
                "value", /* as __String */
            );
        }
        let return_iterator_result = self.filter_type(t, &mut |c: &mut Checker, t: TypeId| {
            c.is_return_iterator_result(t)
        });
        let mut return_type = TypeId::NIL;
        if return_iterator_result != self.never_type {
            return_type = self.get_type_of_property_of_type(
                return_iterator_result,
                "value", /* as __String */
            );
        }
        if yield_type.is_nil() && return_type.is_nil() {
            return IterationTypes {
                yield_type: TypeId::NIL,
                return_type: TypeId::NIL,
                next_type: TypeId::NIL,
            };
        }
        // From https://tc39.github.io/ecma262/#sec-iteratorresult-interface
        // > ... If the iterator does not have a return value, `value` is `undefined`. In that case, the
        // > `value` property may be absent from the conforming object if it does not inherit an explicit
        // > `value` property.
        let return_type = if return_type.is_some() {
            return_type
        } else {
            self.void_type
        };
        IterationTypes {
            yield_type,
            return_type,
            next_type: TypeId::NIL,
        }
    }

    // Go: checker/checker.go:6852 isYieldIteratorResult
    pub fn is_yield_iterator_result(&mut self, t: TypeId) -> bool {
        self.is_iterator_result(t, IterationTypeKind::YIELD)
    }

    // Go: checker/checker.go:6856 isReturnIteratorResult
    pub fn is_return_iterator_result(&mut self, t: TypeId) -> bool {
        self.is_iterator_result(t, IterationTypeKind::RETURN)
    }

    // Go: checker/checker.go:6860 isIteratorResult
    pub fn is_iterator_result(&mut self, t: TypeId, kind: IterationTypeKind) -> bool {
        // From https://tc39.github.io/ecma262/#sec-iteratorresult-interface:
        // > [done] is the result status of an iterator `next` method call. If the end of the iterator was reached `done` is `true`.
        // > If the end was not reached `done` is `false` and a value is available.
        // > If a `done` property (either own or inherited) does not exist, it is consider to have the value `false`.
        let done = self.get_type_of_property_of_type(t, "done");
        let done_type = if done.is_some() {
            done
        } else {
            self.false_type
        };
        let source = if kind == IterationTypeKind::YIELD {
            self.false_type
        } else {
            self.true_type
        };
        self.is_type_assignable_to(source, done_type)
    }

    // Go: checker/checker.go:6869 reportTypeNotIterableError
    pub fn report_type_not_iterable_error(
        &mut self,
        error_node: Node,
        t: TypeId,
        allow_async_iterables: bool,
    ) -> Diagnostic {
        let message: &'static crate::diagnostics::Message = if allow_async_iterables {
            diag::Type_0_must_have_a_Symbol_asyncIterator_method_that_returns_an_async_iterator
        } else {
            diag::Type_0_must_have_a_Symbol_iterator_method_that_returns_an_iterator
        };
        let suggest_await = self.get_awaited_type_of_promise(t).is_some()
            || (!allow_async_iterables
                && is_for_of_statement(error_node.parent())
                && error_node.parent().expression() == error_node
                && {
                    let get_global_async_iterable_type =
                        self.get_global_async_iterable_type.clone();
                    get_global_async_iterable_type(self) != self.empty_generic_type
                }
                && {
                    let get_global_async_iterable_type =
                        self.get_global_async_iterable_type.clone();
                    let global = get_global_async_iterable_type(self);
                    let any = self.any_type;
                    let target =
                        self.create_type_from_generic_global_type(global, &[any, any, any]);
                    self.is_type_assignable_to(t, target)
                });
        let type_string = self.type_to_string_exported(t);
        self.error_and_maybe_suggest_await(error_node, suggest_await, message, args![type_string])
    }

    // Go: checker/checker.go:6884 getIterationDiagnosticDetails
    pub fn get_iteration_diagnostic_details(
        &mut self,
        use_: IterationUse,
        input_type: TypeId,
        allows_strings: bool,
    ) -> (&'static crate::diagnostics::Message, bool) {
        let yield_type = self.get_iteration_type_of_iterable(
            use_,
            IterationTypeKind::YIELD,
            input_type,
            Node::NIL, /*errorNode*/
        );
        if yield_type.is_some() {
            return (diag::Type_0_can_only_be_iterated_through_when_using_the_downlevelIteration_flag_or_with_a_target_of_es2015_or_higher, false);
        }
        let input_symbol = self.ty(input_type).symbol;
        if input_symbol.is_some() && is_es2015_or_later_iterable(&self.sym(input_symbol).name) {
            return (diag::Type_0_can_only_be_iterated_through_when_using_the_downlevelIteration_flag_or_with_a_target_of_es2015_or_higher, true);
        }
        if allows_strings {
            return (diag::Type_0_is_not_an_array_type_or_a_string_type, true);
        }
        (diag::Type_0_is_not_an_array_type, true)
    }
}

// Go: checker/checker.go:6898 isES2015OrLaterIterable
pub fn is_es2015_or_later_iterable(n: &str) -> bool {
    matches!(
        n,
        "Float32Array"
            | "Float64Array"
            | "Int16Array"
            | "Int32Array"
            | "Int8Array"
            | "NodeList"
            | "Uint16Array"
            | "Uint32Array"
            | "Uint8Array"
            | "Uint8ClampedArray"
    )
}

impl Checker {
    // Go: checker/checker.go:6906 checkAliasSymbol
    pub fn check_alias_symbol(&mut self, node: Node) {
        let mut symbol = self.get_symbol_of_declaration(node);
        let target = self.resolve_alias(symbol);
        if target == self.unknown_symbol {
            return;
        }
        // For external modules, `symbol` represents the local symbol for an alias.
        // This local symbol will merge any other local declarations (excluding other aliases)
        // and symbol.flags will contains combined representation for all merged declaration.
        // Based on symbol.flags we can compute a set of excluded meanings (meaning that resolved alias should not have,
        // otherwise it will conflict with some local declaration). Note that in addition to normal flags we include matching SymbolFlags.Export*
        // in order to prevent collisions with declarations that were exported from the current module (they still contribute to local names).
        let export_symbol = self.sym(symbol).export_symbol;
        symbol = self.get_merged_symbol(if export_symbol.is_some() {
            export_symbol
        } else {
            symbol
        });
        let target_flags = self.get_symbol_flags(target);
        // A type-only import/export will already have a grammar error in a JS file, so no need to issue more errors within
        if is_in_js_file(node)
            && !target_flags.intersects(SymbolFlags::VALUE)
            && !is_type_only_import_or_export_declaration(node)
        {
            let property_name_or_name = node.property_name_or_name();
            let error_node = if property_name_or_name.is_some() {
                property_name_or_name
            } else {
                node
            };
            debug_assert!(node.kind() != SyntaxKind::NamespaceExport);
            if is_export_specifier(node) {
                // PORT: Go mutates the diagnostic returned by `c.error` after it
                // was added. `Diagnostic` is owned here, so this builds it,
                // adds the related info, then adds it (the same steps as `c.error`).
                let mut diagnostic = new_diagnostic_for_node(
                    error_node,
                    diag::Types_cannot_appear_in_export_declarations_in_JavaScript_files,
                    args![],
                );
                let source_symbol = get_source_file_of_node(node).symbol();
                if source_symbol.is_some() {
                    let exports = self.sym(source_symbol).exports;
                    let already_exported_symbol = self
                        .symbols
                        .get(exports, node.property_name_or_name().text());
                    if already_exported_symbol == target {
                        let exporting_declaration = self
                            .sym(already_exported_symbol)
                            .declarations
                            .iter()
                            .copied()
                            .find(|d| is_js_type_alias_declaration(*d))
                            .unwrap_or(Node::NIL);
                        if exporting_declaration.is_some() {
                            let name = self.sym(already_exported_symbol).name.clone();
                            diagnostic.add_related_info(Some(new_diagnostic_for_node(
                                exporting_declaration,
                                diag::X_0_is_automatically_exported_here,
                                args![name],
                            )));
                        }
                    }
                }
                self.add_diagnostic(diagnostic);
            } else {
                let mut identifier_text = self.sym(symbol).name.to_string();
                if is_identifier(error_node) {
                    identifier_text = error_node.text().to_string();
                }
                let mut specifier_text = "...".to_string();
                let import_declaration = find_ancestor(node, |n: Node| {
                    is_import_or_import_equals_declaration(n) || is_variable_declaration(n)
                });
                if import_declaration.is_some() {
                    let module_specifier =
                        try_get_module_specifier_from_declaration(import_declaration);
                    if module_specifier.is_some() {
                        specifier_text = module_specifier.text().to_string();
                    }
                }
                let mut import_text = format!("import(\"{specifier_text}\")");
                if is_import_specifier(node) {
                    import_text = format!("{import_text}.{identifier_text}");
                }
                self.error(
                    error_node,
                    diag::X_0_is_a_type_and_cannot_be_imported_in_JavaScript_files_Use_1_in_a_JSDoc_type_annotation,
                    args![identifier_text, import_text],
                );
            }
            return;
        }
        let symbol_flags = self.sym(symbol).flags;
        let excluded_meanings =
            (if symbol_flags.intersects(SymbolFlags::VALUE | SymbolFlags::EXPORT_VALUE) {
                SymbolFlags::VALUE
            } else {
                SymbolFlags::NONE
            }) | (if symbol_flags.intersects(SymbolFlags::TYPE) {
                SymbolFlags::TYPE
            } else {
                SymbolFlags::NONE
            }) | (if symbol_flags.intersects(SymbolFlags::NAMESPACE) {
                SymbolFlags::NAMESPACE
            } else {
                SymbolFlags::NONE
            });
        if target_flags.intersects(excluded_meanings) {
            let message = if is_export_specifier(node) {
                diag::Export_declaration_conflicts_with_exported_declaration_of_0
            } else {
                diag::Import_declaration_conflicts_with_local_declaration_of_0
            };
            let symbol_string = self.symbol_to_string(symbol);
            self.error(node, message, args![symbol_string]);
        } else if !is_export_specifier(node) {
            // Look at 'compilerOptions.isolatedModules' and not 'getIsolatedModules(...)' (which considers 'verbatimModuleSyntax')
            // here because 'verbatimModuleSyntax' will already have an error for importing a type without 'import type'.
            let appears_valuey_to_transpiler = self.compiler_options.isolated_modules.is_true()
                && find_ancestor(node, is_type_only_import_or_export_declaration).is_nil();
            if appears_valuey_to_transpiler
                && symbol_flags.intersects(SymbolFlags::VALUE | SymbolFlags::EXPORT_VALUE)
            {
                let symbol_string = self.symbol_to_string(symbol);
                let flag_name = self.get_isolated_modules_like_flag_name();
                self.error(
                    node,
                    diag::Import_0_conflicts_with_local_value_so_must_be_declared_with_a_type_only_import_when_isolatedModules_is_enabled,
                    args![symbol_string, flag_name],
                );
            }
        }
        if self.compiler_options.get_isolated_modules()
            && !is_type_only_import_or_export_declaration(node)
            && !node.flags().intersects(NodeFlags::AMBIENT)
        {
            let type_only_alias = self.get_type_only_alias_declaration(symbol);
            let is_type = !target_flags.intersects(SymbolFlags::VALUE);
            if is_type || type_only_alias.is_some() {
                match node.kind() {
                    SyntaxKind::ImportClause
                    | SyntaxKind::ImportSpecifier
                    | SyntaxKind::ImportEqualsDeclaration => {
                        if self.compiler_options.verbatim_module_syntax.is_true() {
                            debug_assert!(
                                node.name().is_some(),
                                "An ImportClause with a symbol should have a name"
                            );
                            let message: &'static crate::diagnostics::Message = if self
                                .compiler_options
                                .verbatim_module_syntax
                                .is_true()
                                && is_internal_module_import_equals_declaration(node)
                            {
                                diag::An_import_alias_cannot_resolve_to_a_type_or_type_only_declaration_when_verbatimModuleSyntax_is_enabled
                            } else if is_type {
                                diag::X_0_is_a_type_and_must_be_imported_using_a_type_only_import_when_verbatimModuleSyntax_is_enabled
                            } else {
                                diag::X_0_resolves_to_a_type_only_declaration_and_must_be_imported_using_a_type_only_import_when_verbatimModuleSyntax_is_enabled
                            };
                            let name = node.property_name_or_name().text().to_string();
                            // PORT: Go adds related info to the diagnostic returned by
                            // `c.error`. Build, annotate, then add (same steps as `c.error`).
                            let diagnostic = new_diagnostic_for_node(node, message, args![name]);
                            let diagnostic = self.add_type_only_declaration_related_info(
                                diagnostic,
                                if is_type { Node::NIL } else { type_only_alias },
                                &name,
                            );
                            self.add_diagnostic(diagnostic);
                        }
                        if is_type
                            && node.kind() == SyntaxKind::ImportEqualsDeclaration
                            && has_modifier(node, ModifierFlags::EXPORT)
                        {
                            let flag_name = self.get_isolated_modules_like_flag_name();
                            self.error(
                                node,
                                diag::Cannot_use_export_import_on_a_type_or_type_only_namespace_when_0_is_enabled,
                                args![flag_name],
                            );
                        }
                    }
                    SyntaxKind::ExportSpecifier => {
                        // Don't allow re-exporting an export that will be elided when `--isolatedModules` is set.
                        // The exception is that `import type { A } from './a'; export { A }` is allowed
                        // because single-file analysis can determine that the export should be dropped.
                        // PORT: Go `ast.GetSourceFileOfNode(nil)` returns nil.
                        let type_only_alias_file = if type_only_alias.is_some() {
                            get_source_file_of_node(type_only_alias)
                        } else {
                            Node::NIL
                        };
                        if self.compiler_options.verbatim_module_syntax.is_true()
                            || type_only_alias_file != get_source_file_of_node(node)
                        {
                            let name = node.property_name_or_name().text().to_string();
                            let flag_name = self.get_isolated_modules_like_flag_name();
                            // PORT: build, annotate, then add (same steps as `c.error`).
                            let diagnostic = if is_type {
                                new_diagnostic_for_node(
                                    node,
                                    diag::Re_exporting_a_type_when_0_is_enabled_requires_using_export_type,
                                    args![flag_name],
                                )
                            } else {
                                new_diagnostic_for_node(
                                    node,
                                    diag::X_0_resolves_to_a_type_only_declaration_and_must_be_re_exported_using_a_type_only_re_export_when_1_is_enabled,
                                    args![name, flag_name],
                                )
                            };
                            let diagnostic = self.add_type_only_declaration_related_info(
                                diagnostic,
                                if is_type { Node::NIL } else { type_only_alias },
                                &name,
                            );
                            self.add_diagnostic(diagnostic);
                        }
                    }
                    _ => {}
                }
            }
            if self.compiler_options.verbatim_module_syntax.is_true()
                && !is_import_equals_declaration(node)
                && !is_in_js_file(node)
                && get_emit_module_format_of_file(get_source_file_of_node(node))
                    == ModuleKind::COMMON_JS
            {
                self.error(
                    node,
                    get_verbatim_module_syntax_error_message(node),
                    args![],
                );
            } else if self.module_kind == ModuleKind::PRESERVE
                && !is_import_equals_declaration(node)
                && !is_variable_declaration(node)
                && !is_binding_element(node)
                && get_emit_module_format_of_file(get_source_file_of_node(node))
                    == ModuleKind::COMMON_JS
            {
                // In `--module preserve`, ESM input syntax emits ESM output syntax, but there will be times
                // when we look at the `impliedNodeFormat` of this file and decide it's CommonJS (i.e., currently,
                // only if the file extension is .cjs/.cts). To avoid that inconsistency, we disallow ESM syntax
                // in files that are unambiguously CommonJS in this mode.
                self.error(
                    node,
                    diag::ECMAScript_module_syntax_is_not_allowed_in_a_CommonJS_module_when_module_is_set_to_preserve,
                    args![],
                );
            }
            if self.compiler_options.verbatim_module_syntax.is_true()
                && !is_type_only_import_or_export_declaration(node)
                && !node.flags().intersects(NodeFlags::AMBIENT)
                && target_flags.intersects(SymbolFlags::CONST_ENUM)
            {
                let const_enum_declaration = self.sym(target).value_declaration;
                if const_enum_declaration.is_some()
                    && const_enum_declaration
                        .flags()
                        .intersects(NodeFlags::AMBIENT)
                {
                    // PORT: Go `c.program.GetProjectReferenceFromOutputDts(file.Path())` is a
                    // Program method, ported as a free function; `Path()` reads the
                    // `SourceFileInfo.path` field.
                    let redirect = get_project_reference_from_output_dts(
                        &source_file_info(get_source_file_of_node(const_enum_declaration)).path,
                    );
                    if match &redirect {
                        None => true,
                        Some(redirect) => !redirect
                            .resolved
                            .compiler_options()
                            .should_preserve_const_enums(),
                    } {
                        let flag_name = self.get_isolated_modules_like_flag_name();
                        self.error(
                            node,
                            diag::Cannot_access_ambient_const_enums_when_0_is_enabled,
                            args![flag_name],
                        );
                    }
                }
            }
        }
        if is_import_specifier(node) {
            let target_symbol = self.resolve_alias_with_deprecation_check(symbol, node);
            // PORT: Go checks `targetSymbol.Declarations != nil`; an empty slice here.
            if self.is_deprecated_symbol(target_symbol)
                && !self.sym(target_symbol).declarations.is_empty()
            {
                let declarations = self.sym(target_symbol).declarations.clone();
                let name = self.sym(target_symbol).name.clone();
                self.add_deprecated_suggestion(node, &declarations, &name);
            }
        }
    }

    // Go: checker/checker.go:7036 areDeclarationFlagsIdentical
    pub fn are_declaration_flags_identical(&self, left: Node, right: Node) -> bool {
        if is_parameter_declaration(left) && is_variable_declaration(right)
            || is_variable_declaration(left) && is_parameter_declaration(right)
        {
            // Differences in optionality between parameters and variables are allowed.
            return true;
        }
        if is_optional_declaration(left) != is_optional_declaration(right) {
            return false;
        }
        let interesting_flags = ModifierFlags::PRIVATE
            | ModifierFlags::PROTECTED
            | ModifierFlags::ASYNC
            | ModifierFlags::ABSTRACT
            | ModifierFlags::READONLY
            | ModifierFlags::STATIC;
        get_selected_modifier_flags(left, interesting_flags)
            == get_selected_modifier_flags(right, interesting_flags)
    }

    // Go: checker/checker.go:7048 checkTypeAliasDeclaration
    pub fn check_type_alias_declaration(&mut self, node: Node) {
        // Grammar checking
        self.check_grammar_modifiers(node);
        self.check_type_name_is_reserved(node.name(), diag::Type_alias_name_cannot_be_0);
        if !self.container_allows_block_scoped_variable(node.parent()) {
            self.grammar_error_on_node(
                node,
                diag::X_0_declarations_can_only_be_declared_inside_a_block,
                args!["type"],
            );
        }
        self.check_exports_on_merged_declarations(node);

        let type_node = node.type_();
        let type_parameters = node.type_parameters();
        self.check_type_parameters(type_parameters);
        if type_node.is_some() && type_node.kind() == SyntaxKind::IntrinsicKeyword {
            let name_text = node.name().text();
            if !(type_parameters.is_empty() && name_text == "BuiltinIteratorReturn"
                || type_parameters.len() == 1
                    && INTRINSIC_TYPE_KINDS
                        .get(name_text)
                        .copied()
                        .unwrap_or(IntrinsicTypeKind::UNKNOWN)
                        != IntrinsicTypeKind::UNKNOWN)
            {
                self.error(
                    type_node,
                    diag::The_intrinsic_keyword_can_only_be_used_to_declare_compiler_provided_intrinsic_types,
                    args![],
                );
            }
            // The `intrinsic` keyword is a leaf type node with no child nodes to check,
            // so skipping the checkSourceElement below visits nothing.
            return;
        }
        self.check_source_element(type_node);
        self.register_for_unused_identifiers_check(node);
    }

    // Go: checker/checker.go:7073 checkTypeNameIsReserved
    pub fn check_type_name_is_reserved(
        &mut self,
        name: Node,
        message: &'static crate::diagnostics::Message,
    ) {
        // TS 1.0 spec (April 2014): 3.6.1
        // The predefined type keywords are reserved and cannot be used as names of user defined types.
        match name.text() {
            "any" | "unknown" | "never" | "number" | "bigint" | "boolean" | "string" | "symbol"
            | "void" | "object" | "undefined" => {
                self.error(name, message, args![name.text()]);
            }
            _ => {}
        }
    }

    // Go: checker/checker.go:7082 checkExportsOnMergedDeclarations
    pub fn check_exports_on_merged_declarations(&mut self, node: Node) {
        // If localSymbol is defined on node then node itself is exported - check is required.
        let mut symbol = node.local_symbol();
        if symbol.is_nil() {
            // Local symbol is undefined => this declaration is non-exported.
            // However, symbol might contain other declarations that are exported.
            symbol = self.get_symbol_of_declaration(node);
            if self.sym(symbol).export_symbol.is_nil() {
                // This is a pure local symbol (all declarations are non-exported) - no need to check anything.
                return;
            }
        }
        // Run the check only for the first declaration in the list.
        if get_declaration_of_kind(&self.symbols, symbol, node.kind()) != node {
            return;
        }
        let mut exported_declaration_spaces = DeclarationSpaces::NONE;
        let mut non_exported_declaration_spaces = DeclarationSpaces::NONE;
        let mut default_exported_declaration_spaces = DeclarationSpaces::NONE;
        let declarations = self.sym(symbol).declarations.clone();
        for &d in &declarations {
            let declaration_spaces = self.get_declaration_spaces(d);
            let effective_declaration_flags = self
                .get_effective_declaration_flags(d, ModifierFlags::EXPORT | ModifierFlags::DEFAULT);
            if effective_declaration_flags.intersects(ModifierFlags::EXPORT) {
                if effective_declaration_flags.intersects(ModifierFlags::DEFAULT) {
                    default_exported_declaration_spaces = DeclarationSpaces(
                        default_exported_declaration_spaces.0 | declaration_spaces.0,
                    );
                } else {
                    exported_declaration_spaces =
                        DeclarationSpaces(exported_declaration_spaces.0 | declaration_spaces.0);
                }
            } else {
                non_exported_declaration_spaces =
                    DeclarationSpaces(non_exported_declaration_spaces.0 | declaration_spaces.0);
            }
        }
        // Spaces for anything not declared a 'default export'.
        let non_default_exported_declaration_spaces =
            exported_declaration_spaces.0 | non_exported_declaration_spaces.0;
        let common_declaration_spaces_for_exports_and_locals =
            exported_declaration_spaces.0 & non_exported_declaration_spaces.0;
        let common_declaration_spaces_for_default_and_non_default =
            default_exported_declaration_spaces.0 & non_default_exported_declaration_spaces;
        if common_declaration_spaces_for_exports_and_locals != 0
            || common_declaration_spaces_for_default_and_non_default != 0
        {
            // declaration spaces for exported and non-exported declarations intersect
            for &d in &declarations {
                let declaration_spaces = self.get_declaration_spaces(d);
                let name = get_name_of_declaration(d);
                // Only error on the declarations that contributed to the intersecting spaces.
                if declaration_spaces.0 & common_declaration_spaces_for_default_and_non_default != 0
                {
                    self.error(
                        name,
                        diag::Merged_declaration_0_cannot_include_a_default_export_declaration_Consider_adding_a_separate_export_default_0_declaration_instead,
                        args![declaration_name_to_string(name)],
                    );
                } else if declaration_spaces.0 & common_declaration_spaces_for_exports_and_locals
                    != 0
                {
                    self.error(
                        name,
                        diag::Individual_declarations_in_merged_declaration_0_must_be_all_exported_or_all_local,
                        args![declaration_name_to_string(name)],
                    );
                }
            }
        }
    }

    // Go: checker/checker.go:7133 getDeclarationSpaces
    pub fn get_declaration_spaces(&mut self, node: Node) -> DeclarationSpaces {
        // PORT: Go `fallthrough` from the export assignment case into the
        // alias case is modeled with `resolve_alias_spaces`.
        let mut resolve_alias_spaces = false;
        match node.kind() {
            SyntaxKind::InterfaceDeclaration
            | SyntaxKind::TypeAliasDeclaration
            | SyntaxKind::JsTypeAliasDeclaration
            | SyntaxKind::JsDocTypedefTag
            | SyntaxKind::JsDocCallbackTag => return DeclarationSpaces::EXPORT_TYPE,
            SyntaxKind::ModuleDeclaration => {
                if is_ambient_module(node)
                    || get_module_instance_state(node) != ModuleInstanceState::NON_INSTANTIATED
                {
                    return DeclarationSpaces(
                        DeclarationSpaces::EXPORT_NAMESPACE.0 | DeclarationSpaces::EXPORT_VALUE.0,
                    );
                }
                return DeclarationSpaces::EXPORT_NAMESPACE;
            }
            SyntaxKind::ClassDeclaration | SyntaxKind::EnumDeclaration | SyntaxKind::EnumMember => {
                return DeclarationSpaces(
                    DeclarationSpaces::EXPORT_TYPE.0 | DeclarationSpaces::EXPORT_VALUE.0,
                );
            }
            SyntaxKind::SourceFile => {
                return DeclarationSpaces(
                    DeclarationSpaces::EXPORT_TYPE.0
                        | DeclarationSpaces::EXPORT_VALUE.0
                        | DeclarationSpaces::EXPORT_NAMESPACE.0,
                );
            }
            SyntaxKind::ExportAssignment | SyntaxKind::BinaryExpression => {
                let expression = if is_export_assignment(node) {
                    node.expression()
                } else {
                    node.right()
                };
                // Export assigned entity name expressions act as aliases and should fall through, otherwise they export values.
                if !is_entity_name_expression(expression) || {
                    let s = self.get_symbol_of_declaration(node);
                    !self.sym(s).flags.intersects(SymbolFlags::ALIAS)
                } {
                    return DeclarationSpaces::EXPORT_VALUE;
                }
                // The below options all declare an Alias, which is allowed to merge with other values within the importing module.
                resolve_alias_spaces = true;
            }
            SyntaxKind::ImportEqualsDeclaration
            | SyntaxKind::NamespaceImport
            | SyntaxKind::ImportClause => {
                resolve_alias_spaces = true;
            }
            SyntaxKind::VariableDeclaration
            | SyntaxKind::BindingElement
            | SyntaxKind::FunctionDeclaration
            | SyntaxKind::ImportSpecifier => return DeclarationSpaces::EXPORT_VALUE,
            SyntaxKind::MethodSignature | SyntaxKind::PropertySignature => {
                return DeclarationSpaces::EXPORT_TYPE;
            }
            _ => {}
        }
        if resolve_alias_spaces {
            let mut result = DeclarationSpaces::NONE;
            let s = self.get_symbol_of_declaration(node);
            let target = self.resolve_alias(s);
            let declarations = self.sym(target).declarations.clone();
            for d in declarations {
                result = DeclarationSpaces(result.0 | self.get_declaration_spaces(d).0);
            }
            return result;
        }
        panic!("Unhandled case in getDeclarationSpaces: {:?}", node.kind());
    }

    // Go: checker/checker.go:7174 checkTypeParameters
    // PERF: takes the `NodeSlice` (program data), so callers do not copy the
    // list into a `Vec`.
    pub fn check_type_parameters(&mut self, type_parameter_declarations: NodeSlice) {
        let mut seen_default = false;
        for (i, node) in type_parameter_declarations.iter().enumerate() {
            self.check_type_parameter(node);
            let default_type_node = node.default_type();
            if default_type_node.is_some() {
                seen_default = true;
                self.check_type_parameters_not_referenced(
                    default_type_node,
                    type_parameter_declarations,
                    i as i32,
                );
            } else if seen_default {
                self.error(
                    node,
                    diag::Required_type_parameters_may_not_follow_optional_type_parameters,
                    args![],
                );
            }
            for j in 0..i {
                if type_parameter_declarations.get(j).symbol() == node.symbol() {
                    self.error(
                        node.name(),
                        diag::Duplicate_identifier_0,
                        args![declaration_name_to_string(node.name())],
                    );
                }
            }
        }
    }

    // Check that type parameter defaults only reference previously declared type parameters */
    // Go: checker/checker.go:7194 checkTypeParametersNotReferenced
    pub fn check_type_parameters_not_referenced(
        &mut self,
        root: Node,
        type_parameters: NodeSlice,
        index: i32,
    ) {
        fn visit(c: &mut Checker, node: Node, type_parameters: NodeSlice, index: i32) -> bool {
            if is_type_reference_node(node) {
                let t = c.get_type_from_type_reference(node);
                if c.ty(t).flags.intersects(TypeFlags::TYPE_PARAMETER) {
                    for i in (index as usize)..type_parameters.len() {
                        let tp_symbol = c.get_symbol_of_declaration(type_parameters.get(i));
                        if c.ty(t).symbol == tp_symbol {
                            c.error(
                                node,
                                diag::Type_parameter_defaults_can_only_reference_previously_declared_type_parameters,
                                args![],
                            );
                        }
                    }
                }
            }
            node.for_each_child(&mut |child: Node| visit(c, child, type_parameters, index))
        }
        visit(self, root, type_parameters, index);
    }

    // Go: checker/checker.go:7212 registerForUnusedIdentifiersCheck
    pub fn register_for_unused_identifiers_check(&mut self, node: Node) {
        let source_file = get_source_file_of_node(node);
        let links = self.source_file_links.get(source_file);
        links.identifier_check_nodes.push(node);
    }

    // Go: checker/checker.go:7218 checkUnusedIdentifiers
    pub fn check_unused_identifiers(&mut self, potentially_unused_identifiers: &[Node]) {
        for &node in potentially_unused_identifiers {
            match node.kind() {
                SyntaxKind::ClassDeclaration | SyntaxKind::ClassExpression => {
                    self.check_unused_class_members(node);
                    self.check_unused_type_parameters(node);
                }
                SyntaxKind::SourceFile
                | SyntaxKind::ModuleDeclaration
                | SyntaxKind::Block
                | SyntaxKind::CaseBlock
                | SyntaxKind::ForStatement
                | SyntaxKind::ForInStatement
                | SyntaxKind::ForOfStatement
                | SyntaxKind::ClassStaticBlockDeclaration => {
                    self.check_unused_locals_and_parameters(node);
                }
                SyntaxKind::Constructor
                | SyntaxKind::FunctionExpression
                | SyntaxKind::FunctionDeclaration
                | SyntaxKind::ArrowFunction
                | SyntaxKind::MethodDeclaration
                | SyntaxKind::GetAccessor
                | SyntaxKind::SetAccessor => {
                    // Only report unused parameters on the implementation, not overloads.
                    if node.body().is_some() {
                        self.check_unused_locals_and_parameters(node);
                    }
                    self.check_unused_type_parameters(node);
                }
                SyntaxKind::MethodSignature
                | SyntaxKind::CallSignature
                | SyntaxKind::ConstructSignature
                | SyntaxKind::FunctionType
                | SyntaxKind::ConstructorType
                | SyntaxKind::TypeAliasDeclaration
                | SyntaxKind::JsTypeAliasDeclaration
                | SyntaxKind::InterfaceDeclaration => {
                    self.check_unused_type_parameters(node);
                }
                SyntaxKind::InferType => {
                    self.check_unused_infer_type_parameter(node);
                }
                _ => panic!("Unhandled case in checkUnusedIdentifiers"),
            }
        }
    }

    // Go: checker/checker.go:7245 isReferenced
    pub fn is_referenced(&mut self, symbol: SymbolId) -> bool {
        !self
            .symbol_reference_links
            .get(symbol)
            .reference_kinds
            .is_empty()
    }

    // PORT: Go `type UnusedKind` and its consts are generated in `crate::flags`.

    // Go: checker/checker.go:7256 reportUnusedVariable
    pub fn report_unused_variable(&mut self, mut location: Node, diagnostic: Diagnostic) {
        while is_binding_element(location) || is_binding_pattern(location) {
            location = location.parent();
        }
        let kind = if is_parameter_declaration(location) {
            UnusedKind::PARAMETER
        } else {
            UnusedKind::LOCAL
        };
        self.report_unused(location, kind, diagnostic);
    }

    // Go: checker/checker.go:7263 reportUnused
    pub fn report_unused(&mut self, location: Node, kind: UnusedKind, diagnostic: Diagnostic) {
        if !location
            .flags()
            .intersects(NodeFlags::AMBIENT | NodeFlags::THIS_NODE_OR_ANY_SUB_NODES_HAS_ERROR)
        {
            let is_error = self.unused_is_error(kind);
            if is_error {
                self.add_diagnostic(diagnostic);
            } else {
                let mut suggestion = diagnostic;
                suggestion.set_category(crate::diagnostics::Category::Suggestion);
                self.add_suggestion_diagnostic(suggestion);
            }
        }
    }

    // Go: checker/checker.go:7276 unusedIsError
    pub fn unused_is_error(&self, kind: UnusedKind) -> bool {
        if kind == UnusedKind::LOCAL {
            self.compiler_options.no_unused_locals.is_true()
        } else if kind == UnusedKind::PARAMETER {
            self.compiler_options.no_unused_parameters.is_true()
        } else {
            panic!("Unhandled case in unusedIsError")
        }
    }

    // Go: checker/checker.go:7287 checkUnusedClassMembers
    pub fn check_unused_class_members(&mut self, node: Node) {
        for member in node.members() {
            match member.kind() {
                SyntaxKind::MethodDeclaration
                | SyntaxKind::PropertyDeclaration
                | SyntaxKind::GetAccessor
                | SyntaxKind::SetAccessor => {
                    if is_set_accessor_declaration(member)
                        && self
                            .sym(member.symbol())
                            .flags
                            .intersects(SymbolFlags::GET_ACCESSOR)
                    {
                        continue; // Already would have reported an error on the getter.
                    }
                    let symbol = self.get_symbol_of_declaration(member);
                    if !self.is_referenced(symbol)
                        && (has_modifier(member, ModifierFlags::PRIVATE)
                            || member.name().is_some() && is_private_identifier(member.name()))
                        && !member.flags().intersects(NodeFlags::AMBIENT)
                    {
                        let symbol_string = self.symbol_to_string(symbol);
                        self.report_unused(
                            member,
                            UnusedKind::LOCAL,
                            new_diagnostic_for_node(
                                member.name(),
                                diag::X_0_is_declared_but_its_value_is_never_read,
                                args![symbol_string],
                            ),
                        );
                    }
                }
                SyntaxKind::Constructor => {
                    for parameter in member.parameters() {
                        if !self.is_referenced(parameter.symbol())
                            && has_syntactic_modifier(parameter, ModifierFlags::PRIVATE)
                        {
                            let name = symbol_name(&self.symbols, parameter.symbol());
                            self.report_unused(
                                parameter,
                                UnusedKind::LOCAL,
                                new_diagnostic_for_node(
                                    parameter.name(),
                                    diag::Property_0_is_declared_but_its_value_is_never_read,
                                    args![name],
                                ),
                            );
                        }
                    }
                }
                SyntaxKind::IndexSignature
                | SyntaxKind::SemicolonClassElement
                | SyntaxKind::ClassStaticBlockDeclaration
                | SyntaxKind::JsTypeAliasDeclaration => {
                    // Can't be private
                }
                _ => panic!("Unhandled case in checkUnusedClassMembers"),
            }
        }
    }

    // Go: checker/checker.go:7312 checkUnusedLocalsAndParameters
    pub fn check_unused_locals_and_parameters(&mut self, node: Node) {
        // PORT: Go iterates a set and maps in random order; insertion-ordered
        // collections keep this deterministic. Diagnostics are sorted later.
        let mut variable_parents: IndexSet<Node> = IndexSet::new();
        let mut import_clauses: IndexMap<Node, Vec<Node>> = IndexMap::new();
        for local in self.symbols.values(node.locals()) {
            let reference_kinds = self.symbol_reference_links.get(local).reference_kinds;
            let local_flags = self.sym(local).flags;
            let local_export_symbol = self.sym(local).export_symbol;
            if local_flags.intersects(SymbolFlags::TYPE_PARAMETER)
                && (!local_flags.intersects(SymbolFlags::VARIABLE)
                    || reference_kinds.intersects(SymbolFlags::VARIABLE))
                || !local_flags.intersects(SymbolFlags::TYPE_PARAMETER)
                    && (!reference_kinds.is_empty()
                        || local_export_symbol.is_some()
                        || local_flags.intersects(SymbolFlags::MODULE_EXPORTS))
            {
                continue;
            }
            let declarations = self.sym(local).declarations.clone();
            for declaration in declarations {
                if is_variable_declaration(declaration)
                    || is_parameter_declaration(declaration)
                    || is_binding_element(declaration)
                {
                    variable_parents.insert(get_root_declaration(declaration).parent());
                } else if is_import_clause(declaration)
                    || is_import_specifier(declaration)
                    || is_namespace_import(declaration)
                {
                    if !is_identifier_that_starts_with_underscore(declaration.name()) {
                        let import_clause = import_clause_from_imported(declaration);
                        import_clauses
                            .entry(import_clause)
                            .or_default()
                            .push(declaration);
                    }
                } else if !is_type_parameter_declaration(declaration)
                    && !is_ambient_module(declaration)
                {
                    let name = symbol_name(&self.symbols, local);
                    self.report_unused_local(declaration, &name);
                }
            }
        }
        for declaration in variable_parents {
            if is_variable_declaration_list(declaration) {
                self.report_unused_variables(declaration);
            } else {
                self.report_unused_parameters(declaration);
            }
        }
        for (declaration, unuseds) in import_clauses {
            self.report_unused_imports(declaration, &unuseds);
        }
    }

    // Go: checker/checker.go:7353 reportUnusedLocal
    pub fn report_unused_local(&mut self, node: Node, name: &str) {
        let message = if is_type_declaration(node) {
            diag::X_0_is_declared_but_never_used
        } else {
            diag::X_0_is_declared_but_its_value_is_never_read
        };
        let node_name = node.name();
        let location = if node_name.is_some() { node_name } else { node };
        self.report_unused(
            node,
            UnusedKind::LOCAL,
            new_diagnostic_for_node(location, message, args![name]),
        );
    }

    // Go: checker/checker.go:7358 reportUnusedVariables
    pub fn report_unused_variables(&mut self, node: Node) {
        let declarations = node.declarations().nodes().to_vec();
        if declarations.len() > 1 && self.every_unreferenced_variable_declaration(&declarations) {
            self.report_unused_variable(
                node,
                new_diagnostic_for_node(node, diag::All_variables_are_unused, args![]),
            );
        } else {
            self.report_unused_variable_declarations(&declarations);
        }
    }

    // PORT: Go `core.Every(declarations, c.isUnreferencedVariableDeclaration)`.
    fn every_unreferenced_variable_declaration(&mut self, declarations: &[Node]) -> bool {
        for &declaration in declarations {
            if !self.is_unreferenced_variable_declaration(declaration) {
                return false;
            }
        }
        true
    }

    // Go: checker/checker.go:7367 reportUnusedParameters
    pub fn report_unused_parameters(&mut self, node: Node) {
        let parameters = node.parameters().to_vec();
        self.report_unused_variable_declarations(&parameters);
    }

    // Go: checker/checker.go:7371 reportUnusedBindingElements
    pub fn report_unused_binding_elements(&mut self, node: Node) {
        let declarations = node.elements().to_vec();
        if declarations.len() > 1 && self.every_unreferenced_variable_declaration(&declarations) {
            self.report_unused_variable(
                node,
                new_diagnostic_for_node(node, diag::All_destructured_elements_are_unused, args![]),
            );
        } else {
            self.report_unused_variable_declarations(&declarations);
        }
    }

    // Go: checker/checker.go:7380 reportUnusedVariableDeclarations
    pub fn report_unused_variable_declarations(&mut self, declarations: &[Node]) {
        for &declaration in declarations {
            let name = declaration.name();
            if name.is_some()
                && !is_parameter_property_declaration(declaration, declaration.parent())
                && !is_this_parameter(declaration)
            {
                if is_binding_pattern(name) {
                    self.report_unused_binding_elements(name);
                } else if self.is_unreferenced_variable_declaration(declaration) {
                    self.report_unused_variable(
                        declaration,
                        new_diagnostic_for_node(
                            name,
                            diag::X_0_is_declared_but_its_value_is_never_read,
                            args![name.text()],
                        ),
                    );
                }
            }
        }
    }

    // Go: checker/checker.go:7393 isUnreferencedVariableDeclaration
    pub fn is_unreferenced_variable_declaration(&mut self, node: Node) -> bool {
        let name = node.name();
        if name.is_nil() {
            return true;
        }
        if is_binding_pattern(name) {
            let elements = node.name().elements().to_vec();
            return self.every_unreferenced_variable_declaration(&elements);
        }
        let symbol = self.get_symbol_of_declaration(node);
        if self
            .symbol_reference_links
            .get(symbol)
            .reference_kinds
            .intersects(SymbolFlags::VARIABLE)
        {
            return false;
        }
        if is_binding_element(node) && is_object_binding_pattern(node.parent()) {
            // In `{ a, ...b }, `a` is considered used since it removes a property from `b`. `b` may still be unused though.
            let elements = node.parent().elements();
            let last_element = if elements.is_empty() {
                Node::NIL
            } else {
                elements.get(elements.len() - 1)
            };
            if node != last_element && has_dot_dot_dot_token(last_element) {
                return false;
            }
        }
        if (is_parameter_declaration(node)
            || is_variable_declaration(node)
                && (is_for_in_or_of_statement(node.parent().parent())
                    || self
                        .get_combined_node_flags_cached(node)
                        .intersects(NodeFlags::USING))
            || is_binding_element(node)
                && !(is_object_binding_pattern(node.parent()) && node.property_name().is_nil()))
            && is_identifier_that_starts_with_underscore(name)
        {
            return false;
        }
        true
    }

    // Go: checker/checker.go:7420 reportUnusedImports
    pub fn report_unused_imports(&mut self, node: Node, unuseds: &[Node]) {
        let mut declaration_count: usize = if node.name().is_some() { 1 } else { 0 };
        let named_bindings = node.named_bindings();
        if named_bindings.is_some() {
            if is_namespace_import(named_bindings) {
                declaration_count += 1;
            } else {
                declaration_count += named_bindings.elements().len();
            }
        }
        if declaration_count > 1 && declaration_count == unuseds.len() {
            self.report_unused(
                node,
                UnusedKind::LOCAL,
                new_diagnostic_for_node(
                    node.parent(),
                    diag::All_imports_in_import_declaration_are_unused,
                    args![],
                ),
            );
        } else {
            for &unused in unuseds {
                let text = unused.name().text();
                self.report_unused_local(unused, text);
            }
        }
    }
}

// Go: checker/checker.go:7439 isIdentifierThatStartsWithUnderscore
pub fn is_identifier_that_starts_with_underscore(node: Node) -> bool {
    is_identifier(node) && !node.text().is_empty() && node.text().as_bytes()[0] == b'_'
}

// Go: checker/checker.go:7443 importClauseFromImported
pub fn import_clause_from_imported(node: Node) -> Node {
    match node.kind() {
        SyntaxKind::ImportClause => node,
        SyntaxKind::NamespaceImport => node.parent(),
        _ => node.parent().parent(),
    }
}

impl Checker {
    // Go: checker/checker.go:7454 checkUnusedInferTypeParameter
    pub fn check_unused_infer_type_parameter(&mut self, node: Node) {
        let type_parameter = node.type_parameter();
        if self.is_unreferenced_type_parameter(type_parameter) {
            let name = type_parameter.name().text().to_string();
            self.report_unused(
                node,
                UnusedKind::PARAMETER,
                new_diagnostic_for_node(
                    type_parameter.name(),
                    diag::X_0_is_declared_but_never_used,
                    args![name],
                ),
            );
        }
    }

    // Go: checker/checker.go:7461 checkUnusedTypeParameters
    pub fn check_unused_type_parameters(&mut self, node: Node) {
        let symbol = self.get_symbol_of_declaration(node);
        // PORT: Go package fn `allDeclarationsInSameSourceFile(symbol)` reads
        // symbol data, so it is a Checker method.
        if !self.all_declarations_in_same_source_file(symbol) {
            return;
        }
        let type_parameter_list = node.type_parameter_list();
        if type_parameter_list.is_nil() {
            return;
        }
        let type_parameters = type_parameter_list.nodes();
        if type_parameters.len() > 1 && {
            let mut all = true;
            for tp in type_parameters {
                if !self.is_unreferenced_type_parameter(tp) {
                    all = false;
                    break;
                }
            }
            all
        } {
            let file = get_source_file_of_node(node);
            let loc = range_of_type_parameters(file, type_parameter_list);
            self.report_unused(
                node,
                UnusedKind::PARAMETER,
                new_diagnostic(file, loc, diag::All_type_parameters_are_unused, args![]),
            );
        } else {
            for type_parameter in type_parameters {
                if self.is_unreferenced_type_parameter(type_parameter) {
                    let name = type_parameter.name().text().to_string();
                    self.report_unused(
                        node,
                        UnusedKind::PARAMETER,
                        new_diagnostic_for_node(
                            type_parameter,
                            diag::X_0_is_declared_but_never_used,
                            args![name],
                        ),
                    );
                }
            }
        }
    }

    // Go: checker/checker.go:7482 isUnreferencedTypeParameter
    pub fn is_unreferenced_type_parameter(&mut self, type_parameter: Node) -> bool {
        let symbol = self.get_merged_symbol(type_parameter.symbol());
        !self
            .symbol_reference_links
            .get(symbol)
            .reference_kinds
            .intersects(SymbolFlags::TYPE_PARAMETER)
            && !is_identifier_that_starts_with_underscore(type_parameter.name())
    }

    // Go: checker/checker.go:7486 checkUnusedRenamedBindingElements
    pub fn check_unused_renamed_binding_elements(&mut self) {
        let nodes = self.renamed_binding_elements_in_types.clone();
        for node in nodes {
            let symbol = self.get_symbol_of_declaration(node);
            if self
                .symbol_reference_links
                .get(symbol)
                .reference_kinds
                .is_empty()
            {
                let wrapping_declaration = walk_up_binding_elements_and_patterns(node);
                debug_assert!(
                    is_part_of_parameter_declaration(wrapping_declaration),
                    "Only parameter declaration should be checked here"
                );
                let mut diagnostic = new_diagnostic_for_node(
                    node.name(),
                    diag::X_0_is_an_unused_renaming_of_1_Did_you_intend_to_use_it_as_a_type_annotation,
                    args![declaration_name_to_string(node.name()), declaration_name_to_string(node.property_name())],
                );
                if wrapping_declaration.type_().is_nil() {
                    // entire parameter does not have type annotation, suggest adding an annotation
                    diagnostic.add_related_info(Some(new_diagnostic(
                        get_source_file_of_node(wrapping_declaration),
                        TextRange::new(wrapping_declaration.end(), wrapping_declaration.end()),
                        diag::We_can_only_write_a_type_for_0_by_adding_a_type_for_the_entire_parameter_here,
                        args![declaration_name_to_string(node.property_name())],
                    )));
                }
                self.add_diagnostic(diagnostic);
            }
        }
    }

    // Go: checker/checker.go:7501 checkExpressionStatement
    pub fn check_expression_statement(&mut self, node: Node) {
        // Grammar checking
        self.check_grammar_statement_in_ambient_context(node);
        self.check_expression(node.expression());
    }

    // Returns the type of an expression. Unlike checkExpression, this function is simply concerned
    // with computing the type and may not fully check all contained sub-expressions for errors.
    // Go: checker/checker.go:7509 getTypeOfExpression
    pub fn get_type_of_expression(&mut self, node: Node) -> TypeId {
        // Don't bother caching types that require no flow analysis and are quick to compute.
        let quick_type = self.get_quick_type_of_expression(node);
        if quick_type.is_some() {
            return quick_type;
        }
        // If a type has been cached for the node, return it.
        if let Some(&cached_type) = self.flow_type_cache.get(&node) {
            if cached_type.is_some() {
                return cached_type;
            }
        }
        let start_invocation_count = self.flow_invocation_count;
        let t = self.check_expression_ex(node, CheckMode::TYPE_ONLY);
        // If control flow analysis was required to determine the type, it is worth caching.
        if self.flow_invocation_count != start_invocation_count {
            // PORT: Go lazily makes the nil map; the Rust map always exists.
            self.flow_type_cache.insert(node, t);
        }
        t
    }

    // Returns the type of an expression. Unlike checkExpression, this function is simply concerned
    // with computing the type and may not fully check all contained sub-expressions for errors.
    // Go: checker/checker.go:7533 getQuickTypeOfExpression
    pub fn get_quick_type_of_expression(&mut self, node: Node) -> TypeId {
        let expr = skip_parentheses(node);
        if is_await_expression(expr) {
            let t = self.get_quick_type_of_expression(expr.expression());
            if t.is_some() {
                return self.get_awaited_type(t);
            }
            return TypeId::NIL;
        }
        // Optimize for the common case of a call to a function with a single non-generic call
        // signature where we can just fetch the return type without checking the arguments.
        if is_call_expression(expr)
            && expr.expression().kind() != SyntaxKind::SuperKeyword
            && !is_require_call(expr, true /*requireStringLiteralLikeArgument*/)
            && !self.is_symbol_or_symbol_for_call(expr)
            && !is_import_call(expr)
        {
            if is_call_chain(expr) {
                return self.get_return_type_of_single_non_generic_signature_of_call_chain(expr);
            }
            let func_type = self.check_non_null_expression(expr.expression());
            return self
                .get_return_type_of_single_non_generic_signature(func_type, SignatureKind::CALL);
        }
        if is_new_expression(expr) {
            let func_type = self.check_non_null_expression(expr.expression());
            return self.get_return_type_of_single_non_generic_signature(
                func_type,
                SignatureKind::CONSTRUCT,
            );
        }
        if is_assertion_expression(expr) && !is_const_type_reference(expr.type_()) {
            return self.get_type_from_type_node(expr.type_());
        }
        if is_literal_expression(node) || is_boolean_literal(node) {
            return self.check_expression(node);
        }
        TypeId::NIL
    }

    // Go: checker/checker.go:7559 getReturnTypeOfSingleNonGenericSignature
    pub fn get_return_type_of_single_non_generic_signature(
        &mut self,
        func_type: TypeId,
        kind: SignatureKind,
    ) -> TypeId {
        let signature = self.get_single_signature(func_type, kind, true /*allowMembers*/);
        if signature.is_some() && self.sig(signature).type_parameters.is_empty() {
            return self.get_return_type_of_signature(signature);
        }
        TypeId::NIL
    }

    // Go: checker/checker.go:7567 getReturnTypeOfSingleNonGenericSignatureOfCallChain
    pub fn get_return_type_of_single_non_generic_signature_of_call_chain(
        &mut self,
        expr: Node,
    ) -> TypeId {
        let func_type = self.check_expression(expr.expression());
        let non_optional_type = self.get_optional_expression_type(func_type, expr.expression());
        let return_type =
            self.get_return_type_of_single_non_generic_signature(func_type, SignatureKind::CALL);
        if return_type.is_some() {
            return self.propagate_optional_type_marker(
                return_type,
                expr,
                non_optional_type != func_type,
            );
        }
        TypeId::NIL
    }

    // Go: checker/checker.go:7577 checkNonNullExpression
    pub fn check_non_null_expression(&mut self, node: Node) -> TypeId {
        let t = self.check_expression(node);
        self.check_non_null_type(t, node)
    }

    // Go: checker/checker.go:7581 checkNonNullType
    pub fn check_non_null_type(&mut self, t: TypeId, node: Node) -> TypeId {
        self.check_non_null_type_with_reporter(
            t,
            node,
            &mut |c: &mut Checker, n: Node, facts: TypeFacts| {
                c.report_object_possibly_null_or_undefined_error(n, facts)
            },
        )
    }

    // Go: checker/checker.go:7585 checkNonNullTypeWithReporter
    pub fn check_non_null_type_with_reporter(
        &mut self,
        t: TypeId,
        node: Node,
        report_error: &mut dyn FnMut(&mut Checker, Node, TypeFacts),
    ) -> TypeId {
        if self.strict_null_checks && self.ty(t).flags.intersects(TypeFlags::UNKNOWN) {
            if is_entity_name_expression(node) {
                // PORT: checker `entityNameToString(node)` is
                // `ast.EntityNameToString(node, scanner.GetTextOfNode)`.
                let node_text = entity_name_to_string(node, Some(&get_text_of_node));
                if node_text.len() < 100 {
                    self.error(node, diag::X_0_is_of_type_unknown, args![node_text]);
                    return self.error_type;
                }
            }
            self.error(node, diag::Object_is_of_type_unknown, args![]);
            return self.error_type;
        }
        let facts = self.get_type_facts(t, TypeFacts::IS_UNDEFINED_OR_NULL);
        if facts.intersects(TypeFacts::IS_UNDEFINED_OR_NULL) {
            report_error(self, node, facts);
            let non_nullable = self.get_non_nullable_type(t);
            if self
                .ty(non_nullable)
                .flags
                .intersects(TypeFlags::NULLABLE | TypeFlags::NEVER)
            {
                return self.error_type;
            }
            return non_nullable;
        }
        t
    }

    // Go: checker/checker.go:7609 checkNonNullNonVoidType
    pub fn check_non_null_non_void_type(&mut self, t: TypeId, node: Node) -> TypeId {
        let non_null_type = self.check_non_null_type(t, node);
        if self.ty(non_null_type).flags.intersects(TypeFlags::VOID) {
            if is_entity_name_expression(node) {
                // PORT: checker `entityNameToString(node)`, see above.
                let node_text = entity_name_to_string(node, Some(&get_text_of_node));
                if is_identifier(node) && node_text == "undefined" {
                    self.error(
                        node,
                        diag::The_value_0_cannot_be_used_here,
                        args![node_text],
                    );
                    return non_null_type;
                }
                if node_text.len() < 100 {
                    self.error(node, diag::X_0_is_possibly_undefined, args![node_text]);
                    return non_null_type;
                }
            }
            self.error(node, diag::Object_is_possibly_undefined, args![]);
        }
        non_null_type
    }

    // Go: checker/checker.go:7628 reportObjectPossiblyNullOrUndefinedError
    pub fn report_object_possibly_null_or_undefined_error(&mut self, node: Node, facts: TypeFacts) {
        let mut node_text = String::new();
        if is_entity_name_expression(node) {
            // PORT: checker `entityNameToString(node)`, see above.
            node_text = entity_name_to_string(node, Some(&get_text_of_node));
        }
        if node.kind() == SyntaxKind::NullKeyword {
            self.error(node, diag::The_value_0_cannot_be_used_here, args!["null"]);
            return;
        }
        if !node_text.is_empty() && node_text.len() < 100 {
            if is_identifier(node) && node_text == "undefined" {
                self.error(
                    node,
                    diag::The_value_0_cannot_be_used_here,
                    args!["undefined"],
                );
                return;
            }
            let message = if facts.intersects(TypeFacts::IS_UNDEFINED) {
                if facts.intersects(TypeFacts::IS_NULL) {
                    diag::X_0_is_possibly_null_or_undefined
                } else {
                    diag::X_0_is_possibly_undefined
                }
            } else {
                diag::X_0_is_possibly_null
            };
            self.error(node, message, args![node_text]);
        } else {
            let message = if facts.intersects(TypeFacts::IS_UNDEFINED) {
                if facts.intersects(TypeFacts::IS_NULL) {
                    diag::Object_is_possibly_null_or_undefined
                } else {
                    diag::Object_is_possibly_undefined
                }
            } else {
                diag::Object_is_possibly_null
            };
            self.error(node, message, args![]);
        }
    }

    // Go: checker/checker.go:7656 checkExpressionWithContextualType
    pub fn check_expression_with_contextual_type(
        &mut self,
        node: Node,
        contextual_type: TypeId,
        inference_context: InferenceContextId,
        check_mode: CheckMode,
    ) -> TypeId {
        let context_node = self.get_context_node(node);
        self.push_contextual_type(context_node, contextual_type, false /*isCache*/);
        self.push_inference_context(context_node, inference_context);
        let inferential = if inference_context.is_some() {
            CheckMode::INFERENTIAL
        } else {
            CheckMode::NORMAL
        };
        let mut t =
            self.check_expression_ex(node, check_mode | CheckMode::CONTEXTUAL | inferential);
        // In CheckMode.Inferential we collect intra-expression inference sites to process before fixing any type
        // parameters. This information is no longer needed after the call to checkExpression.
        // PORT: Go sets the slice to nil; an empty Vec is the Rust nil.
        if inference_context.is_some()
            && !self
                .inference_context(inference_context)
                .intra_expression_inference_sites()
                .is_empty()
        {
            self.inference_context_mut(inference_context)
                .rare_mut()
                .intra_expression_inference_sites = Vec::new();
        }
        // We strip literal freshness when an appropriate contextual type is present such that contextually typed
        // literals always preserve their literal types (otherwise they might widen during type inference). An alternative
        // here would be to not mark contextually typed literals as fresh in the first place.
        if self.maybe_type_of_kind(t, TypeFlags::LITERAL) && {
            let instantiated =
                self.instantiate_contextual_type(contextual_type, node, ContextFlags::NONE);
            self.is_literal_of_contextual_type(t, instantiated)
        } {
            t = self.get_regular_type_of_literal_type(t);
        }
        self.pop_inference_context();
        self.pop_contextual_type();
        t
    }

    // Go: checker/checker.go:7677 getContextNode
    pub fn get_context_node(&self, node: Node) -> Node {
        if is_jsx_attributes(node) && !is_jsx_self_closing_element(node.parent()) {
            // Needs to be the root JsxElement, so it encompasses the attributes _and_ the children (which are essentially part of the attributes)
            return node.parent().parent();
        }
        node
    }

    // Go: checker/checker.go:7685 checkExpressionCached
    pub fn check_expression_cached(&mut self, node: Node) -> TypeId {
        self.check_expression_cached_ex(node, CheckMode::NORMAL)
    }

    // Go: checker/checker.go:7689 checkExpressionCachedEx
    pub fn check_expression_cached_ex(&mut self, node: Node, check_mode: CheckMode) -> TypeId {
        if check_mode != CheckMode::NORMAL {
            return self.check_expression_ex(node, check_mode);
        }
        if self.type_node_links.get(node).resolved_type.is_nil() {
            // When computing a type that we're going to cache, we need to ignore any ongoing control flow
            // analysis because variables may have transient types in indeterminable states. Moving flowLoopStart
            // to the top of the stack ensures all transient types are computed from a known point.
            // PORT: Go nil-assigns the stack and cache; `mem::take` leaves them empty.
            let save_flow_loop_stack = std::mem::take(&mut self.flow_loop_stack);
            let save_flow_type_cache = std::mem::take(&mut self.flow_type_cache);
            let t = self.check_expression_ex(node, check_mode);
            self.type_node_links.get(node).resolved_type = t;
            self.flow_type_cache = save_flow_type_cache;
            self.flow_loop_stack = save_flow_loop_stack;
        }
        self.type_node_links.get(node).resolved_type
    }

    // Returns the type of an expression. Unlike checkExpression, this function is simply concerned
    // with computing the type and may not fully check all contained sub-expressions for errors.
    // It is intended for uses where you know there is no contextual type,
    // and requesting the contextual type might cause a circularity or other bad behaviour.
    // It sets the contextual type of the node to any before calling getTypeOfExpression.
    // Go: checker/checker.go:7714 getContextFreeTypeOfExpression
    pub fn get_context_free_type_of_expression(&mut self, node: Node) -> TypeId {
        if let Some(&cached) = self.context_free_types.get(&node) {
            if cached.is_some() {
                return cached;
            }
        }
        let any_type = self.any_type;
        self.push_contextual_type(node, any_type, false /*isCache*/);
        let t = self.check_expression_ex(node, CheckMode::SKIP_CONTEXT_SENSITIVE);
        self.context_free_types.insert(node, t);
        self.pop_contextual_type();
        t
    }
}
