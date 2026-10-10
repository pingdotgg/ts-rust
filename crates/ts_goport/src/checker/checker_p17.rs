//! Port of Go `checker/checker.go` lines 14829-15734 (alias targets,
//! external module resolution, ES module symbol wrapping, entity names).
//!
//! PORT: Go `c.error(...)` returns the `*ast.Diagnostic` it already added, and
//! some callers mutate it afterwards (`AddRelatedInfo`). Diagnostics are owned
//! values here, so those call sites build the diagnostic with
//! `new_diagnostic_for_node`, finish it, then call `self.add_diagnostic` (the
//! same two steps Go `error` does).
//!
//! PORT: Go `tspath`, `module.GetResolutionDiagnostic` and a few `core`
//! helpers have no port in this crate. Private copies that follow the Go code
//! exactly live in the `tspath_p17`, `module_p17` and `core_p17` modules at
//! the end of this file.
//!
//! PORT: Go `c.program.X(...)` methods are free functions with the Go snake
//! names (see `PORTING.md`, Program section). `*module.ResolvedModule` is
//! `Option<&ResolvedModule>`.

use crate::diagnostics::Message;
use crate::prelude::*;

// PORT: Go compares `*diagnostics.Message` pointers. A nil message is `None`.
fn message_is_p17(message: Option<&'static Message>, target: &'static Message) -> bool {
    message.is_some_and(|m| std::ptr::eq(m, target))
}

// PORT: Go `resolvedModule.IsResolved()` (nil-safe method on `*ResolvedModule`).
fn is_resolved_p17(resolved_module: Option<&ResolvedModule>) -> bool {
    resolved_module.is_some_and(|r| !r.resolved_file_name.is_empty())
}

impl Checker {
    // Go: checker/checker.go:15151 reportNonExportedMember
    pub fn report_non_exported_member(
        &mut self,
        name: Node,
        declaration_name: &str,
        module_symbol: SymbolId,
        module_name: &str,
    ) {
        let mut local_symbol = SymbolId::NIL;
        let locals = self.sym(module_symbol).value_declaration.locals();
        if locals.is_some() {
            local_symbol = self.symbols.get(locals, name.text());
        }
        let exports = self.sym(module_symbol).exports;
        if local_symbol.is_some() {
            let exported_equals_symbol = self
                .symbols
                .get(exports, INTERNAL_SYMBOL_NAME_EXPORT_EQUALS);
            if exported_equals_symbol.is_some() {
                if self
                    .get_symbol_if_same_reference(exported_equals_symbol, local_symbol)
                    .is_some()
                {
                    self.report_invalid_import_equals_export_member(
                        name,
                        declaration_name,
                        module_name,
                    );
                } else {
                    self.error(
                        name,
                        diag::Module_0_has_no_exported_member_1,
                        args![module_name, declaration_name],
                    );
                }
            } else {
                // PORT: Go `findInMap(exports, ...)` over the symbol table values.
                let mut exported_symbol = SymbolId::NIL;
                for symbol in self.symbols.values(exports) {
                    if self
                        .get_symbol_if_same_reference(symbol, local_symbol)
                        .is_some()
                    {
                        exported_symbol = symbol;
                        break;
                    }
                }
                let mut diagnostic = if exported_symbol.is_some() {
                    let exported_name = self.symbol_to_string(exported_symbol);
                    new_diagnostic_for_node(
                        name,
                        diag::Module_0_declares_1_locally_but_it_is_exported_as_2,
                        args![module_name, declaration_name, exported_name],
                    )
                } else {
                    new_diagnostic_for_node(
                        name,
                        diag::Module_0_declares_1_locally_but_it_is_not_exported,
                        args![module_name, declaration_name],
                    )
                };
                let declarations = self.sym(local_symbol).declarations.clone();
                for (i, decl) in declarations.iter().enumerate() {
                    let message = if i == 0 {
                        diag::X_0_is_declared_here
                    } else {
                        diag::X_and_here
                    };
                    diagnostic.add_related_info(Some(create_diagnostic_for_node(
                        *decl,
                        message,
                        args![declaration_name],
                    )));
                }
                self.add_diagnostic(diagnostic);
            }
        } else {
            self.error(
                name,
                diag::Module_0_has_no_exported_member_1,
                args![module_name, declaration_name],
            );
        }
    }

    // Go: checker/checker.go:15183 reportInvalidImportEqualsExportMember
    pub fn report_invalid_import_equals_export_member(
        &mut self,
        name: Node,
        declaration_name: &str,
        module_name: &str,
    ) {
        if self.module_kind >= ModuleKind::ES2015 {
            self.error(
                name,
                diag::X_0_can_only_be_imported_by_using_a_default_import,
                args![declaration_name],
            );
        } else if is_in_js_file(name) {
            self.error(
                name,
                diag::X_0_can_only_be_imported_by_using_a_require_call_or_by_using_a_default_import,
                args![declaration_name],
            );
        } else {
            self.error(
                name,
                diag::X_0_can_only_be_imported_by_using_import_1_require_2_or_a_default_import,
                args![declaration_name, declaration_name, module_name],
            );
        }
    }

    // Go: checker/checker.go:15193 getTargetOfExportSpecifier
    pub fn get_target_of_export_specifier(
        &mut self,
        node: Node,
        meaning: SymbolFlags,
        dont_resolve_alias: bool,
    ) -> SymbolId {
        let name = node.property_name_or_name();
        if module_export_name_is_default(name) {
            let specifier = self.get_module_specifier_for_import_or_export(node);
            if specifier.is_some() {
                let import_attributes_type = self
                    .get_type_from_import_attributes(get_import_attributes(node.parent().parent()));
                let module_symbol = self.resolve_external_module_name(
                    node,
                    specifier,
                    false, /*ignoreErrors*/
                    import_attributes_type,
                );
                if module_symbol.is_some() {
                    return self.get_target_of_module_default(
                        module_symbol,
                        node,
                        dont_resolve_alias,
                    );
                }
            }
        }
        let export_declaration = node.parent().parent();
        let resolved = if export_declaration.module_specifier().is_some() {
            self.get_external_module_member(export_declaration, node, dont_resolve_alias)
        } else if is_string_literal(name) {
            SymbolId::NIL
        } else {
            self.resolve_entity_name(
                name,
                meaning,
                false, /*ignoreErrors*/
                dont_resolve_alias,
                Node::NIL, /*location*/
            )
        };
        self.mark_symbol_of_alias_declaration_if_type_only(node, Node::NIL);
        resolved
    }

    // Go: checker/checker.go:15218 getTargetOfExportAssignment
    pub fn get_target_of_export_assignment(&mut self, node: Node) -> SymbolId {
        // An `export =` / `export default` inside a namespace/module block is a grammar error;
        // checkExportAssignment reports it and returns without resolving the expression. Mirror that
        // bail-out here (using the same container computation) so that alias resolution triggered by
        // the emit resolver does not resolve — and report "Cannot find name" diagnostics on — the
        // expression, which would produce diagnostics inconsistent with checking.
        if is_contained_by_namespace(node) {
            return SymbolId::NIL;
        }
        let resolved = self.get_target_of_alias_like_expression(node.expression());
        self.mark_symbol_of_alias_declaration_if_type_only(node, Node::NIL);
        resolved
    }

    // Go: checker/checker.go:15232 getTargetOfBinaryExpression
    pub fn get_target_of_binary_expression(&mut self, node: Node) -> SymbolId {
        let resolved = self.get_target_of_alias_like_expression(node.right());
        self.mark_symbol_of_alias_declaration_if_type_only(node, Node::NIL);
        resolved
    }

    // Go: checker/checker.go:15238 getTargetOfAliasLikeExpression
    pub fn get_target_of_alias_like_expression(&mut self, expression: Node) -> SymbolId {
        if is_class_expression(expression) {
            let t = self.check_expression_cached(expression);
            return self.ty(t).symbol;
        }
        if !is_entity_name(expression) && !is_entity_name_expression(expression) {
            return SymbolId::NIL;
        }
        let alias_like = self.resolve_entity_name(
            expression,
            SymbolFlags::VALUE | SymbolFlags::TYPE | SymbolFlags::NAMESPACE,
            true,      /*ignoreErrors*/
            true,      /*dontResolveAlias*/
            Node::NIL, /*location*/
        );
        if alias_like.is_some() {
            return alias_like;
        }
        self.check_expression_cached(expression);
        self.get_resolved_symbol_or_nil(expression)
    }

    // Go: checker/checker.go:15253 getTargetOfNamespaceExportDeclaration
    pub fn get_target_of_namespace_export_declaration(&mut self, node: Node) -> SymbolId {
        if can_have_symbol(node.parent()) {
            let resolved = self.resolve_external_module_symbol(
                node.parent().symbol(),
                true, /*dontResolveAlias*/
            );
            self.mark_symbol_of_alias_declaration_if_type_only(node, Node::NIL);
            return resolved;
        }
        SymbolId::NIL
    }

    // Go: checker/checker.go:15262 getTargetOfAccessExpression
    pub fn get_target_of_access_expression(&mut self, node: Node) -> SymbolId {
        if is_binary_expression(node.parent()) {
            let expr = node.parent();
            if expr.left() == node && expr.operator_token().kind() == SyntaxKind::EqualsToken {
                return self.get_target_of_alias_like_expression(expr.right());
            }
        }
        SymbolId::NIL
    }

    // Go: checker/checker.go:15272 getModuleSpecifierForImportOrExport
    pub fn get_module_specifier_for_import_or_export(&mut self, node: Node) -> Node {
        match node.kind() {
            SyntaxKind::ImportClause => return get_module_specifier_from_node(node.parent()),
            SyntaxKind::ImportEqualsDeclaration => {
                if is_external_module_reference(node.module_reference()) {
                    return node.module_reference().expression();
                } else {
                    return Node::NIL;
                }
            }
            SyntaxKind::NamespaceImport => {
                return get_module_specifier_from_node(node.parent().parent());
            }
            SyntaxKind::ImportSpecifier => {
                return get_module_specifier_from_node(node.parent().parent().parent());
            }
            SyntaxKind::NamespaceExport => return get_module_specifier_from_node(node.parent()),
            SyntaxKind::ExportSpecifier => {
                return get_module_specifier_from_node(node.parent().parent());
            }
            _ => {}
        }
        panic!("Unhandled case in getModuleSpecifierForImportOrExport");
    }
}

// Go: checker/checker.go:15294 getModuleSpecifierFromNode
pub fn get_module_specifier_from_node(node: Node) -> Node {
    match node.kind() {
        SyntaxKind::ImportDeclaration | SyntaxKind::JsImportDeclaration => {
            return node.module_specifier();
        }
        SyntaxKind::ExportDeclaration => return node.module_specifier(),
        _ => {}
    }
    panic!(
        "Unhandled case in getModuleSpecifierFromNode: {:?}",
        node.kind()
    );
}

impl Checker {
    // Go: checker/checker.go:15325 markSymbolOfAliasDeclarationIfTypeOnly
    //
    // Marks a symbol as type-only if its declaration is syntactically type-only.
    // If it is not itself marked type-only, but resolves to a type-only alias
    // somewhere in its resolution chain, save a reference to the type-only alias declaration
    // so the alias _not_ marked type-only can be identified as _transitively_ type-only.
    //
    // This function is called on each alias declaration that could be type-only or resolve to
    // another type-only alias during `resolveAlias`, so that later, when an alias is used in a
    // JS-emitting expression, we can quickly determine if that symbol is effectively type-only
    // and issue an error if so.
    pub fn mark_symbol_of_alias_declaration_if_type_only(
        &mut self,
        alias_declaration: Node,
        export_star_declaration: Node,
    ) -> bool {
        if alias_declaration.is_nil() || !is_declaration_node(alias_declaration) {
            return false;
        }
        // If the declaration itself is type-only, mark it and return. No need to check what it resolves to.
        let source_symbol = self.get_symbol_of_declaration(alias_declaration);
        let links = self.alias_symbol_links.get(source_symbol);
        if links.type_only_declaration.is_nil()
            && is_type_only_import_or_export_declaration(alias_declaration)
        {
            links.type_only_declaration = alias_declaration;
            return true;
        }
        if links.type_only_declaration.is_nil() && export_star_declaration.is_some() {
            links.type_only_declaration = export_star_declaration;
            return true;
        }
        links.type_only_declaration.is_some()
    }

    // Go: checker/checker.go:15343 resolveExternalModuleName
    pub fn resolve_external_module_name(
        &mut self,
        location: Node,
        module_reference_expression: Node,
        ignore_errors: bool,
        import_attributes_type: TypeId,
    ) -> SymbolId {
        let mut error_message = self
            .get_cannot_resolve_module_name_error_for_specific_module(module_reference_expression);
        if error_message.is_none() {
            error_message = Some(diag::Cannot_find_module_0_or_its_corresponding_type_declarations);
        }
        let ignore_errors = ignore_errors || self.compiler_options.no_check.is_true();
        self.resolve_external_module_name_worker(
            location,
            module_reference_expression,
            if ignore_errors { None } else { error_message },
            ignore_errors,
            false, /*isForAugmentation*/
            import_attributes_type,
        )
    }

    // Go: checker/checker.go:15352 getCannotResolveModuleNameErrorForSpecificModule
    pub fn get_cannot_resolve_module_name_error_for_specific_module(
        &mut self,
        module_name: Node,
    ) -> Option<&'static Message> {
        if is_string_literal(module_name) {
            if core_p17::is_node_core_module(module_name.text()) {
                if self.compiler_options.uses_wildcard_types() {
                    return Some(diag::Cannot_find_name_0_Do_you_need_to_install_type_definitions_for_node_Try_npm_i_save_dev_types_Slashnode);
                }
                return Some(
                    diag::Cannot_find_name_0_Do_you_need_to_install_type_definitions_for_node_Try_npm_i_save_dev_types_Slashnode_and_then_add_node_to_the_types_field_in_your_tsconfig,
                );
            }
        }
        None
    }

    // Go: checker/checker.go:15364 resolveExternalModuleNameWorker
    pub fn resolve_external_module_name_worker(
        &mut self,
        location: Node,
        module_reference_expression: Node,
        module_not_found_error: Option<&'static Message>,
        ignore_errors: bool,
        is_for_augmentation: bool,
        import_attributes_type: TypeId,
    ) -> SymbolId {
        if is_string_literal_like(module_reference_expression) {
            // ts#63915, Go N' checker.go:15418: a source phase import has no
            // module symbol.
            if is_source_phase_import(module_reference_expression.parent()) {
                return SymbolId::NIL;
            }
            return self.resolve_external_module(
                location,
                module_reference_expression.text(),
                module_not_found_error,
                if !ignore_errors {
                    module_reference_expression
                } else {
                    Node::NIL
                },
                is_for_augmentation,
                import_attributes_type,
            );
        }
        SymbolId::NIL
    }

    // Go: checker/checker.go:15371 getExternalModuleFileFromDeclaration
    pub fn get_external_module_file_from_declaration(&mut self, declaration: Node) -> Node {
        let mut specifier = Node::NIL;
        if declaration.kind() == SyntaxKind::ModuleDeclaration {
            if is_string_literal(declaration.name()) {
                specifier = declaration.name();
            }
        } else {
            specifier = get_external_module_name(declaration);
        }
        let mut import_attributes_type = TypeId::NIL;
        if has_import_attributes(declaration) {
            import_attributes_type =
                self.get_type_from_import_attributes(get_import_attributes(declaration));
        }
        // This is only used by emit and type printing, after checking has already reported any
        // resolution errors for this specifier. Resolve with ignoreErrors so that these queries
        // don't add new diagnostics (e.g. an implicit-any-module suggestion) as a side effect.
        // (ts#64479, Go N' checker.go:15385)
        let module_symbol = self.resolve_external_module_name_worker(
            specifier,
            specifier,
            None,  /*moduleNotFoundError*/
            true,  /*ignoreErrors*/
            false, /*isForAugmentation*/
            import_attributes_type,
        );
        if module_symbol.is_nil() {
            return Node::NIL;
        }
        let decl = get_declaration_of_kind(&self.symbols, module_symbol, SyntaxKind::SourceFile);
        if decl.is_nil() {
            return Node::NIL;
        }
        decl
    }

    // Go: checker/checker.go:15395 resolveExternalModule
    pub fn resolve_external_module(
        &mut self,
        location: Node,
        module_reference: &str,
        module_not_found_error: Option<&'static Message>,
        error_node: Node,
        is_for_augmentation: bool,
        import_attributes_type: TypeId,
    ) -> SymbolId {
        if error_node.is_some() && module_reference.starts_with("@types/") {
            let without_at_type_prefix = &module_reference["@types/".len()..];
            self.error(
                error_node,
                diag::Cannot_import_type_declaration_files_Consider_importing_0_instead_of_1,
                args![without_at_type_prefix, module_reference],
            );
        }
        let import_attributes_type = if import_attributes_type.is_nil() {
            self.empty_object_type
        } else {
            import_attributes_type
        };

        let ambient_module =
            self.try_find_ambient_module(module_reference, true /*withAugmentations*/);
        if ambient_module.is_some() {
            return self.try_resolve_pattern_ambient_module(
                ambient_module,
                module_reference,
                import_attributes_type,
            );
        }

        let importing_source_file = get_source_file_of_node(location);
        let mut context_specifier = Node::NIL;
        let mode: ResolutionMode;

        if is_string_literal_like(location)
            || location.parent().is_some()
                && is_module_declaration(location.parent())
                && location.parent().name() == location
        {
            context_specifier = location;
        } else if is_module_declaration(location) {
            context_specifier = location.name();
        } else if is_literal_import_type_node(location) {
            context_specifier = location.argument().literal();
        } else if is_variable_declaration_initialized_to_bare_or_accessed_require(location) {
            context_specifier = get_module_specifier_of_bare_or_accessed_require(location);
        } else {
            let mut ancestor = find_ancestor(location, is_import_call);
            if ancestor.is_some() {
                context_specifier = ancestor.arguments().get(0);
            }

            if ancestor.is_nil() {
                ancestor = find_ancestor(location, is_import_declaration_or_js_import_declaration);
                if ancestor.is_some() {
                    context_specifier = ancestor.module_specifier();
                }
            }
            if ancestor.is_nil() {
                ancestor = find_ancestor(location, is_export_declaration);
                if ancestor.is_some() {
                    context_specifier = ancestor.module_specifier();
                }
            }
            if ancestor.is_nil() {
                ancestor = find_ancestor(location, is_import_equals_declaration);
                if ancestor.is_some() {
                    let module_refrence = ancestor.module_reference();
                    if module_refrence.kind() == SyntaxKind::ExternalModuleReference {
                        context_specifier = module_refrence.expression();
                    }
                }
            }
        }

        if context_specifier.is_some() && is_string_literal_like(context_specifier) {
            mode = get_mode_for_usage_location(importing_source_file, context_specifier);
        } else {
            mode = get_default_resolution_mode_for_file(importing_source_file);
        }

        // PERF: borrow the program's resolution instead of cloning it.
        let resolved_module_owner =
            get_resolved_module(importing_source_file, module_reference, mode);
        let resolved_module = resolved_module_owner.as_deref();

        let mut resolution_diagnostic: Option<&'static Message> = None;
        if error_node.is_some() && is_resolved_p17(resolved_module) {
            resolution_diagnostic = module_p17::get_resolution_diagnostic(
                self.compiler_options,
                resolved_module.unwrap(),
                importing_source_file,
            );
        }

        let mut source_file = Node::NIL;
        if is_resolved_p17(resolved_module)
            && (resolution_diagnostic.is_none()
                || message_is_p17(
                    resolution_diagnostic,
                    diag::Module_0_was_resolved_to_1_but_jsx_is_not_set,
                ))
        {
            source_file =
                get_source_file_for_resolved_module(&resolved_module.unwrap().resolved_file_name);
        }

        if source_file.is_some() {
            // PORT: `resolvedModule` is resolved here, so the Go dereferences never see nil.
            let rm = resolved_module.unwrap();
            // If there's a resolutionDiagnostic we need to report it even if a sourceFile is found.
            if let Some(resolution_diagnostic) = resolution_diagnostic {
                self.error(
                    error_node,
                    resolution_diagnostic,
                    args![module_reference, rm.resolved_file_name],
                );
            }

            if error_node.is_some() {
                if rm.resolved_using_ts_extension
                    && tspath_p17::is_declaration_file_name(module_reference)
                {
                    if find_ancestor(location, is_emittable_import).is_some() {
                        let ts_extension = tspath_p17::try_extract_ts_extension(module_reference);
                        if ts_extension.is_empty() {
                            panic!(
                                "should be able to extract TS extension from string that passes IsDeclarationFileName"
                            );
                        }
                        let suggested =
                            self.get_suggested_import_source(module_reference, ts_extension, mode);
                        self.error(
                            error_node,
                            diag::A_declaration_file_cannot_be_imported_without_import_type_Did_you_mean_to_import_an_implementation_file_0_instead,
                            args![suggested],
                        );
                    }
                } else if rm.resolved_using_ts_extension
                    && !self.compiler_options.allow_importing_ts_extensions_from(
                        source_file_file_name(importing_source_file),
                    )
                {
                    if find_ancestor(location, is_emittable_import).is_some() {
                        let mut ts_extension =
                            tspath_p17::try_extract_ts_extension(module_reference);
                        if ts_extension.is_empty() {
                            // Fallback: do a best-effort extraction using strings.Contains.
                            // This handles cases where a wildcard pattern matches a TS extension that's
                            // not at the end of the module specifier, e.g., "#/foo.ts.omg" through "#/*.omg": "./src/*"
                            for ext in tspath_p17::SUPPORTED_TS_EXTENSIONS_FLAT {
                                if module_reference.contains(ext) {
                                    ts_extension = ext;
                                    break;
                                }
                            }
                        }
                        if ts_extension.is_empty() {
                            panic!(
                                "should be able to extract TS extension from string when resolvedUsingTsExtension is true"
                            );
                        }
                        self.error(
                            error_node,
                            diag::An_import_path_can_only_end_with_a_0_extension_when_allowImportingTsExtensions_is_enabled,
                            args![ts_extension],
                        );
                    }
                } else if self
                    .compiler_options
                    .rewrite_relative_import_extensions
                    .is_true()
                    && !location.flags().intersects(NodeFlags::AMBIENT)
                    && !tspath_p17::is_declaration_file_name(module_reference)
                    && !is_literal_import_type_node(location)
                    && !is_part_of_type_only_import_or_export_declaration(location)
                {
                    let should_rewrite = core_p17::should_rewrite_module_specifier(
                        module_reference,
                        self.compiler_options,
                    );
                    if !rm.resolved_using_ts_extension && should_rewrite {
                        let current_directory = get_current_directory().to_string();
                        let relative_to_source_file = tspath_p17::get_relative_path_from_file(
                            &tspath_p17::get_normalized_absolute_path(
                                source_file_file_name(importing_source_file),
                                &current_directory,
                            ),
                            &rm.resolved_file_name,
                            &tspath_p17::ComparePathsOptions {
                                use_case_sensitive_file_names: use_case_sensitive_file_names(),
                                current_directory: current_directory.clone(),
                            },
                        );
                        self.error(
                            error_node,
                            diag::This_relative_import_path_is_unsafe_to_rewrite_because_it_looks_like_a_file_name_but_actually_resolves_to_0,
                            args![relative_to_source_file],
                        );
                    } else if rm.resolved_using_ts_extension
                        && !should_rewrite
                        && source_file_may_be_emitted(source_file, false)
                    {
                        self.error(
                            error_node,
                            diag::This_import_uses_a_0_extension_to_resolve_to_an_input_TypeScript_file_but_will_not_be_rewritten_during_emit_because_it_is_not_a_relative_path,
                            args![tspath_p17::get_any_extension_from_path(module_reference, &[], false)],
                        );
                    } else if rm.resolved_using_ts_extension && should_rewrite {
                        if let Some(redirect) = get_redirect_for_resolution(source_file) {
                            let own_root_dir = common_source_directory().to_string();
                            let other_root_dir = redirect.common_source_directory().to_string();

                            let compare_options = tspath_p17::ComparePathsOptions {
                                use_case_sensitive_file_names: use_case_sensitive_file_names(),
                                current_directory: get_current_directory().to_string(),
                            };

                            let root_dir_path = tspath_p17::get_relative_path_from_directory(
                                &own_root_dir,
                                &other_root_dir,
                                &compare_options,
                            );

                            // Get outDir paths, defaulting to root directories if not specified
                            let mut own_out_dir = self.compiler_options.out_dir.clone();
                            if own_out_dir.is_empty() {
                                own_out_dir = own_root_dir;
                            }
                            let mut other_out_dir = redirect.compiler_options().out_dir.clone();
                            if other_out_dir.is_empty() {
                                other_out_dir = other_root_dir;
                            }
                            let out_dir_path = tspath_p17::get_relative_path_from_directory(
                                &own_out_dir,
                                &other_out_dir,
                                &compare_options,
                            );

                            if root_dir_path != out_dir_path {
                                self.error(
                                    error_node,
                                    diag::This_import_path_is_unsafe_to_rewrite_because_it_resolves_to_another_project_and_the_relative_path_between_the_projects_output_files_is_not_the_same_as_the_relative_path_between_its_input_files,
                                    args![],
                                );
                            }
                        }
                    }
                }
            }

            let source_file_symbol = source_file.symbol();
            if source_file_symbol.is_some() {
                if error_node.is_some() {
                    if rm.is_external_library_import
                        && !resolution_extension_is_ts_or_json(&rm.extension)
                    {
                        self.error_on_implicit_any_module(
                            false, /*isError*/
                            error_node,
                            mode,
                            rm,
                            module_reference,
                        );
                    }
                    if self.module_kind == ModuleKind::NODE16
                        || self.module_kind == ModuleKind::NODE18
                    {
                        let is_sync_import =
                            get_default_resolution_mode_for_file(importing_source_file)
                                == ModuleKind::COMMON_JS
                                && find_ancestor(location, is_import_call).is_nil()
                                || find_ancestor(location, is_import_equals_declaration).is_some();
                        let override_host =
                            find_ancestor(location, is_resolution_mode_override_host);
                        if is_sync_import
                            && get_default_resolution_mode_for_file(source_file)
                                == ModuleKind::ES_NEXT
                            && !has_resolution_mode_override(override_host)
                        {
                            if find_ancestor_kind(location, SyntaxKind::ImportEqualsDeclaration)
                                .is_some()
                            {
                                // ImportEquals in an ESM file resolving to another ESM file
                                self.error(
                                    error_node,
                                    diag::Module_0_cannot_be_imported_using_this_construct_The_specifier_only_resolves_to_an_ES_module_which_cannot_be_imported_with_require_Use_an_ECMAScript_import_instead,
                                    args![module_reference],
                                );
                            } else {
                                // CJS file resolving to an ESM file
                                let mut diagnostic_details: Option<Diagnostic> = None;
                                let ext = tspath_p17::try_get_extension_from_path(
                                    source_file_file_name(importing_source_file),
                                );
                                if ext == tspath_p17::EXTENSION_TS
                                    || ext == tspath_p17::EXTENSION_JS
                                    || ext == tspath_p17::EXTENSION_TSX
                                    || ext == tspath_p17::EXTENSION_JSX
                                {
                                    diagnostic_details = Some(self.create_mode_mismatch_details(
                                        importing_source_file,
                                        error_node,
                                    ));
                                }

                                let message = if override_host.is_some()
                                    && override_host.kind() == SyntaxKind::ImportDeclaration
                                    && override_host.import_clause().is_some()
                                    && override_host.import_clause().is_type_only()
                                {
                                    diag::Type_only_import_of_an_ECMAScript_module_from_a_CommonJS_module_must_have_a_resolution_mode_attribute
                                } else if override_host.is_some()
                                    && override_host.kind() == SyntaxKind::ImportType
                                {
                                    diag::Type_import_of_an_ECMAScript_module_from_a_CommonJS_module_must_have_a_resolution_mode_attribute
                                } else {
                                    diag::The_current_file_is_a_CommonJS_module_whose_imports_will_produce_require_calls_however_the_referenced_file_is_an_ECMAScript_module_and_cannot_be_imported_with_require_Consider_writing_a_dynamic_import_0_call_instead
                                };

                                self.add_diagnostic(new_diagnostic_chain_for_node(
                                    diagnostic_details,
                                    error_node,
                                    message,
                                    args![module_reference],
                                ));
                            }
                        }
                    }
                }
                let merged = self.get_merged_symbol(source_file_symbol);
                return self.try_resolve_pattern_ambient_module(
                    merged,
                    module_reference,
                    import_attributes_type,
                );
            }
            let pattern_ambient_module = self.try_resolve_pattern_ambient_module(
                SymbolId::NIL, /*resolvedSymbol*/
                module_reference,
                import_attributes_type,
            );
            if pattern_ambient_module.is_some() {
                return pattern_ambient_module;
            }
            if error_node.is_some()
                && module_not_found_error.is_some()
                && !is_side_effect_import(error_node)
            {
                self.error(
                    error_node,
                    diag::File_0_is_not_a_module,
                    args![rm.resolved_file_name],
                );
            }
            return SymbolId::NIL;
        }

        let pattern_ambient_module = self.try_resolve_pattern_ambient_module(
            SymbolId::NIL, /*resolvedSymbol*/
            module_reference,
            import_attributes_type,
        );
        if pattern_ambient_module.is_some() {
            return pattern_ambient_module;
        }

        if error_node.is_nil() {
            return SymbolId::NIL;
        }

        if is_resolved_p17(resolved_module)
            && !resolution_extension_is_ts_or_json(&resolved_module.unwrap().extension)
            && resolution_diagnostic.is_none()
            || message_is_p17(
                resolution_diagnostic,
                diag::Could_not_find_a_declaration_file_for_module_0_1_implicitly_has_an_any_type,
            )
        {
            // PORT: both sides of the condition imply `resolvedModule` is resolved.
            let rm = resolved_module.unwrap();
            if is_for_augmentation {
                self.error(
                    error_node,
                    diag::Invalid_module_name_in_augmentation_Module_0_resolves_to_an_untyped_module_at_1_which_cannot_be_augmented,
                    args![module_reference, rm.resolved_file_name],
                );
            } else {
                self.error_on_implicit_any_module(
                    self.no_implicit_any && module_not_found_error.is_some(),
                    error_node,
                    mode,
                    rm,
                    module_reference,
                );
            }
            return SymbolId::NIL;
        }

        if let Some(module_not_found_error) = module_not_found_error {
            // See if this was possibly a projectReference redirect
            if is_resolved_p17(resolved_module) {
                let rm = resolved_module.unwrap();
                let redirect = get_project_reference_from_source(&tspath_p17::to_path(
                    &rm.resolved_file_name,
                    &get_current_directory().to_string(),
                    use_case_sensitive_file_names(),
                ));
                if let Some(redirect) = redirect.filter(|r| !r.output_dts.is_empty()) {
                    self.error(
                        error_node,
                        diag::Output_file_0_has_not_been_built_from_source_file_1,
                        args![redirect.output_dts, rm.resolved_file_name],
                    );
                    return SymbolId::NIL;
                }
            }

            if let Some(resolution_diagnostic) = resolution_diagnostic {
                // PORT: a non-nil `resolutionDiagnostic` implies `resolvedModule` is resolved.
                let rm = resolved_module.unwrap();
                self.error(
                    error_node,
                    resolution_diagnostic,
                    args![module_reference, rm.resolved_file_name],
                );
            } else {
                let is_extensionless_relative_path_import =
                    tspath_p17::path_is_relative(module_reference)
                        && !tspath_p17::has_extension(module_reference);
                let resolution_is_node16_or_next = self.module_resolution_kind
                    == ModuleResolutionKind::NODE16
                    || self.module_resolution_kind == ModuleResolutionKind::NODE_NEXT;
                if !self.compiler_options.get_resolve_json_module()
                    && tspath_p17::file_extension_is(module_reference, tspath_p17::EXTENSION_JSON)
                {
                    self.error(
                        error_node,
                        diag::Cannot_find_module_0_Consider_using_resolveJsonModule_to_import_module_with_json_extension,
                        args![module_reference],
                    );
                } else if mode == RESOLUTION_MODE_ESM
                    && resolution_is_node16_or_next
                    && is_extensionless_relative_path_import
                {
                    let absolute_ref = tspath_p17::get_normalized_absolute_path(
                        module_reference,
                        &tspath_p17::get_directory_path(source_file_file_name(
                            importing_source_file,
                        )),
                    );
                    let suggested_ext = self.get_suggested_import_extension(&absolute_ref);
                    if !suggested_ext.is_empty() {
                        self.error(
                            error_node,
                            diag::Relative_import_paths_need_explicit_file_extensions_in_ECMAScript_imports_when_moduleResolution_is_node16_or_nodenext_Did_you_mean_0,
                            args![format!("{module_reference}{suggested_ext}")],
                        );
                    } else {
                        self.error(
                            error_node,
                            diag::Relative_import_paths_need_explicit_file_extensions_in_ECMAScript_imports_when_moduleResolution_is_node16_or_nodenext_Consider_adding_an_extension_to_the_import_path,
                            args![],
                        );
                    }
                } else if let Some(rm) = resolved_module.filter(|r| !r.alternate_result.is_empty())
                {
                    let error_info = self.create_module_not_found_chain(
                        rm,
                        error_node,
                        module_reference,
                        mode,
                        module_reference,
                    );
                    self.add_diagnostic(new_diagnostic_chain_for_node(
                        Some(error_info),
                        error_node,
                        module_not_found_error,
                        args![module_reference],
                    ));
                } else {
                    self.error(error_node, module_not_found_error, args![module_reference]);
                }
            }
        }

        SymbolId::NIL
    }

    // Go: checker/checker.go:15692 tryResolvePatternAmbientModule
    // Resolves the module reference to a pattern ambient module, if one exists.
    // If a resolved symbol from regular module resolution exists and we have an empty import attributes type,
    // we prefer the resolved symbol.
    pub fn try_resolve_pattern_ambient_module(
        &mut self,
        resolved_symbol: SymbolId,
        module_reference: &str,
        import_attributes_type: TypeId,
    ) -> SymbolId {
        if self.is_empty_object_type(import_attributes_type) && resolved_symbol.is_some() {
            return resolved_symbol;
        }
        if !self.pattern_ambient_modules.is_empty() {
            // PERF: chkport1 item 3. Go filters a slice of pointers. A clone
            // of the whole list copied two strings per pattern on each call,
            // so the list is read by index and only candidates are cloned.
            // `initialize_checker` sets the list before any check.
            // PERF: the Go bytes of the module name are made once per call,
            // not three times per pattern (midway tests every import that
            // its paths do not resolve against many patterns).
            let module_reference_bytes = go_string_bytes(module_reference);
            let mut candidates: Vec<PatternAmbientModule> = Vec::new();
            for i in 0..self.pattern_ambient_modules.len() {
                let symbol = self.pattern_ambient_modules[i].symbol;
                let module_attributes_type = self.get_type_of_module_import_attributes(symbol);
                if core_p17::pattern_matches_go_bytes(
                    &self.pattern_ambient_modules[i],
                    &module_reference_bytes,
                ) && self.is_type_assignable_to(import_attributes_type, module_attributes_type)
                {
                    candidates.push(self.pattern_ambient_modules[i].clone());
                }
            }

            if !candidates.is_empty() {
                let augmentation = self
                    .symbols
                    .get(self.pattern_ambient_module_augmentations, module_reference);
                let augmentation_target = self.symbols.get(
                    self.pattern_ambient_module_augmentation_targets,
                    module_reference,
                );

                if candidates.len() == 1 {
                    let merged_candidate = self.get_merged_symbol(candidates[0].symbol);
                    if augmentation.is_some() && augmentation_target == merged_candidate {
                        return self.get_merged_symbol(augmentation);
                    }
                    return merged_candidate;
                }

                let mut best_type_candidates: Vec<PatternAmbientModule> = Vec::new();
                'outer: for (i, candidate) in candidates.iter().enumerate() {
                    let candidate_type =
                        self.get_type_of_module_import_attributes(candidate.symbol);
                    for (j, other) in candidates.iter().enumerate() {
                        let other_type = self.get_type_of_module_import_attributes(other.symbol);
                        if i != j
                            && self.is_type_strict_subtype_of(other_type, candidate_type)
                            && !self.is_type_identical_to(other_type, candidate_type)
                        {
                            continue 'outer;
                        }
                    }
                    best_type_candidates.push(candidate.clone());
                }
                if best_type_candidates.len() == 1 {
                    let merged_candidate = self.get_merged_symbol(best_type_candidates[0].symbol);
                    if augmentation.is_some() && augmentation_target == merged_candidate {
                        return self.get_merged_symbol(augmentation);
                    }
                    return merged_candidate;
                }
                let pattern_symbol =
                    core_p17::find_best_pattern_match(&best_type_candidates, module_reference)
                        .map(|p| p.symbol)
                        .unwrap_or(SymbolId::NIL);
                let merged_candidate = self.get_merged_symbol(pattern_symbol);
                if augmentation.is_some() && augmentation_target == merged_candidate {
                    return self.get_merged_symbol(augmentation);
                }
                return merged_candidate;
            }
        }
        resolved_symbol
    }
}

// Go: checker/checker.go:15744 resolutionExtensionIsTSOrJson
pub fn resolution_extension_is_ts_or_json(ext: &str) -> bool {
    tspath_p17::extension_is_ts(ext) || ext == tspath_p17::EXTENSION_JSON
}

impl Checker {
    // Go: checker/checker.go:15748 getSuggestedImportSource
    pub fn get_suggested_import_source(
        &mut self,
        module_reference: &str,
        ts_extension: &str,
        mode: ResolutionMode,
    ) -> String {
        let import_source_without_extension =
            tspath_p17::remove_extension(module_reference, ts_extension);

        // Direct users to import source with .js extension if outputting an ES module.
        // @see https://github.com/microsoft/TypeScript/issues/42151
        if self.module_kind.is_non_node_esm() || mode == ModuleKind::ES_NEXT {
            let prefer_ts = tspath_p17::is_declaration_file_name(module_reference)
                && self.compiler_options.get_allow_importing_ts_extensions();
            let ext = if ts_extension == tspath_p17::EXTENSION_MTS
                || ts_extension == tspath_p17::EXTENSION_DMTS
            {
                if prefer_ts { ".mts" } else { ".mjs" }
            } else if ts_extension == tspath_p17::EXTENSION_CTS
                || ts_extension == tspath_p17::EXTENSION_DCTS
            {
                if prefer_ts { ".cts" } else { ".cjs" }
            } else if prefer_ts {
                ".ts"
            } else {
                ".js"
            };

            return format!("{import_source_without_extension}{ext}");
        }

        import_source_without_extension.to_string()
    }

    // Go: checker/checker.go:15771 getSuggestedImportExtension
    pub fn get_suggested_import_extension(
        &mut self,
        extensionless_import_path: &str,
    ) -> &'static str {
        let exists = |ext: &str| file_exists(&format!("{extensionless_import_path}{ext}"));
        if exists(".mts") {
            return ".mjs";
        }
        if exists(".ts") {
            return ".js";
        }
        if exists(".cts") {
            return ".cjs";
        }
        if exists(".mjs") {
            return ".mjs";
        }
        if exists(".js") {
            return ".js";
        }
        if exists(".cjs") {
            return ".cjs";
        }
        if exists(".tsx") {
            return if self.compiler_options.jsx == JsxEmit::PRESERVE {
                ".jsx"
            } else {
                ".js"
            };
        }
        if exists(".jsx") {
            return ".jsx";
        }
        if exists(".json") {
            return ".json";
        }
        ""
    }

    // Go: checker/checker.go:15795 errorOnImplicitAnyModule
    // PORT: Go takes `*module.ResolvedModule` and dereferences it without a nil
    // check. Every caller passes a resolved module, so this takes a reference.
    pub fn error_on_implicit_any_module(
        &mut self,
        is_error: bool,
        error_node: Node,
        mode: ResolutionMode,
        resolved_module: &ResolvedModule,
        module_reference: &str,
    ) {
        if is_side_effect_import(error_node) {
            return;
        }

        let mut error_info: Option<Diagnostic> = None;
        if !tspath_p17::is_external_module_name_relative(module_reference)
            && !resolved_module.package_id.name.is_empty()
        {
            error_info = Some(self.create_module_not_found_chain(
                resolved_module,
                error_node,
                module_reference,
                mode,
                &resolved_module.package_id.name,
            ));
        }
        self.add_error_or_suggestion(
            is_error,
            new_diagnostic_chain_for_node(
                error_info,
                error_node,
                diag::Could_not_find_a_declaration_file_for_module_0_1_implicitly_has_an_any_type,
                args![module_reference, resolved_module.resolved_file_name],
            ),
        );
    }

    // Go: checker/checker.go:15816 createModuleNotFoundChain
    pub fn create_module_not_found_chain(
        &mut self,
        _resolved_module: &ResolvedModule,
        error_node: Node,
        module_reference: &str,
        mode: ResolutionMode,
        package_name: &str,
    ) -> Diagnostic {
        // Store the original packageName for repopulateInfo before any modifications
        let mut stored_package_name = package_name;
        if stored_package_name == module_reference {
            stored_package_name = "";
        }

        // PERF: chkport1 item 3. Go `program.GetPackagesMap()` builds the
        // map once per program; the port's builds it on each call, so the
        // checker's memo (Go `c.packagesMap`) is passed in.
        let program = self.program;
        let details = create_module_not_found_chain_with(
            program,
            get_source_file_of_node(error_node),
            module_reference,
            mode,
            package_name,
            || self.get_packages_map(),
        );
        let mut result = new_diagnostic_for_node(error_node, details.message, details.args);
        result.set_repopulate_info(Some(std::sync::Arc::new(RepopulateDiagnosticInfo {
            kind: RepopulateDiagnosticKind::MODULE_NOT_FOUND,
            module_reference: module_reference.to_string(),
            mode,
            package_name: stored_package_name.to_string(),
        })));
        result
    }

    // Go: checker/checker.go:15834 createModeMismatchDetails
    pub fn create_mode_mismatch_details(
        &mut self,
        source_file: Node,
        error_node: Node,
    ) -> Diagnostic {
        let details = create_mode_mismatch_details(self.program, source_file);
        let mut result = new_diagnostic_for_node(error_node, details.message, details.args);
        result.set_repopulate_info(Some(std::sync::Arc::new(RepopulateDiagnosticInfo {
            kind: RepopulateDiagnosticKind::MODE_MISMATCH,
            ..RepopulateDiagnosticInfo::default()
        })));
        result
    }

    // Go: checker/checker.go:15843 tryFindAmbientModule
    pub fn try_find_ambient_module(
        &mut self,
        module_name: &str,
        with_augmentations: bool,
    ) -> SymbolId {
        if tspath_p17::is_external_module_name_relative(module_name) {
            return SymbolId::NIL;
        }
        // PERF: chkport1 item 3. Go concatenates the three strings; `format!`
        // runs the formatting machinery for each lookup.
        let mut quoted = String::with_capacity(module_name.len() + 2);
        quoted.push('"');
        quoted.push_str(module_name);
        quoted.push('"');
        let symbol = self.get_symbol(self.globals, &quoted, SymbolFlags::VALUE_MODULE);
        // merged symbol is module declaration symbol combined with all augmentations
        if with_augmentations {
            return self.get_merged_symbol(symbol);
        }
        symbol
    }

    // Go: checker/checker.go:15855 GetAmbientModules
    // PORT: Go `sync.Once` becomes the `ambient_modules_once` flag.
    pub fn get_ambient_modules(&mut self) -> Vec<SymbolId> {
        if !self.ambient_modules_once {
            self.ambient_modules_once = true;
            let mut seen: FxHashSet<SymbolId> = FxHashSet::default();
            for (sym, global) in self.symbols.iter(self.globals) {
                if is_ambient_module_symbol_name(sym) {
                    self.ambient_modules.push(global);
                    seen.insert(global);
                }
            }
            for i in 0..self.pattern_ambient_modules.len() {
                let symbol = self.get_merged_symbol(self.pattern_ambient_modules[i].symbol);
                if seen.insert(symbol) {
                    self.ambient_modules.push(symbol);
                }
            }
        }
        self.ambient_modules.clone()
    }

    // Go: checker/checker.go:15875 resolveExternalModuleSymbol
    pub fn resolve_external_module_symbol(
        &mut self,
        module_symbol: SymbolId,
        dont_resolve_alias: bool,
    ) -> SymbolId {
        if module_symbol.is_some() {
            let export_equals_symbol = self.symbols.get(
                self.sym(module_symbol).exports,
                INTERNAL_SYMBOL_NAME_EXPORT_EQUALS,
            );
            let export_equals = self.resolve_symbol_ex(export_equals_symbol, dont_resolve_alias);
            if export_equals.is_some() {
                return self.get_merged_symbol(export_equals);
            }
        }
        module_symbol
    }

    // Go: checker/checker.go:15887 resolveESModuleSymbol
    //
    // Resolves the given external module symbol, possibly removing call and construct signatures or creating a
    // wrapper module with a synthetic default.
    pub fn resolve_es_module_symbol(
        &mut self,
        module_symbol: SymbolId,
        node: Node,
        module_specifier: Node,
    ) -> SymbolId {
        let mut symbol =
            self.resolve_external_module_symbol(module_symbol, true /*dontResolveAlias*/);
        if is_non_local_alias(
            &self.symbols,
            symbol,
            SymbolFlags::VALUE | SymbolFlags::TYPE | SymbolFlags::NAMESPACE,
        ) {
            // When the module has an export= with a pure alias, we transitively resolve and propagate any typeOnlyDeclaration
            let source = self.get_symbol_of_declaration(node);
            let indirection = self.resolve_indirection_alias(source, symbol);
            symbol = self.get_merged_symbol(indirection);
        }
        if symbol.is_some() {
            let reference_parent = module_specifier.parent();
            let mut namespace_import = Node::NIL;
            if is_import_declaration(reference_parent) {
                namespace_import = get_namespace_declaration_node(reference_parent);
            }
            if namespace_import.is_some() || is_import_call(reference_parent) {
                let reference = if is_import_call(reference_parent) {
                    reference_parent.arguments().get(0)
                } else {
                    reference_parent.module_specifier()
                };
                let typ = self.get_type_of_symbol(symbol);
                let import_attributes_type =
                    self.get_import_attributes_type_for_module_specifier(reference);
                let default_only_type = self.get_type_with_synthetic_default_only(
                    typ,
                    symbol,
                    module_symbol,
                    reference,
                    import_attributes_type,
                );
                if default_only_type.is_some() {
                    return self.clone_type_as_module_type(
                        symbol,
                        default_only_type,
                        reference_parent,
                    );
                }

                let target_file = self
                    .sym(module_symbol)
                    .declarations
                    .iter()
                    .copied()
                    .find(|d| is_source_file(*d))
                    .unwrap_or_default();
                let usage_mode = self.get_emit_syntax_for_module_specifier_expression(reference);
                let mut export_module_dot_exports_symbol = SymbolId::NIL;
                if namespace_import.is_some()
                    && target_file.is_some()
                    && ModuleKind::NODE20 <= self.module_kind
                    && self.module_kind <= ModuleKind::NODE_NEXT
                    && usage_mode == ModuleKind::COMMON_JS
                    && get_implied_node_format_for_emit(target_file) == ModuleKind::ES_NEXT
                {
                    export_module_dot_exports_symbol = self.get_export_of_module(
                        symbol,
                        INTERNAL_SYMBOL_NAME_MODULE_EXPORTS,
                        namespace_import,
                        true, /*dontResolveAlias*/
                    );
                }
                if export_module_dot_exports_symbol.is_some() {
                    if self.has_signatures(typ) {
                        return self.clone_type_as_module_type(
                            export_module_dot_exports_symbol,
                            typ,
                            reference_parent,
                        );
                    }
                    return export_module_dot_exports_symbol;
                }

                let is_esm_cjs_ref = target_file.is_some()
                    && is_esm_format_import_importing_commonjs_format_file(
                        usage_mode,
                        get_implied_node_format_for_emit(target_file),
                    );
                if self.has_signatures(typ)
                    || self
                        .get_property_of_type_ex(
                            typ,
                            INTERNAL_SYMBOL_NAME_DEFAULT,
                            true,  /*skipObjectFunctionPropertyAugment*/
                            false, /*includeTypeOnlyMembers*/
                        )
                        .is_some()
                    || is_esm_cjs_ref
                {
                    let module_type = if self.ty(typ).flags.intersects(TypeFlags::STRUCTURED_TYPE) {
                        self.get_type_with_synthetic_default_import_type(
                            typ,
                            symbol,
                            module_symbol,
                            reference,
                        )
                    } else {
                        let parent = self.sym(symbol).parent;
                        self.create_default_property_wrapper_for_module(
                            symbol,
                            parent,
                            SymbolId::NIL,
                        )
                    };
                    return self.clone_type_as_module_type(symbol, module_type, reference_parent);
                }
            }
        }
        symbol
    }

    // Go: checker/checker.go:15943 hasSignatures
    pub fn has_signatures(&mut self, t: TypeId) -> bool {
        !self
            .get_signatures_of_structured_type(t, SignatureKind::CALL)
            .is_empty()
            || !self
                .get_signatures_of_structured_type(t, SignatureKind::CONSTRUCT)
                .is_empty()
    }
}

// Go: checker/checker.go:15947 isESMFormatImportImportingCommonjsFormatFile
pub fn is_esm_format_import_importing_commonjs_format_file(
    usage_mode: ResolutionMode,
    target_mode: ResolutionMode,
) -> bool {
    usage_mode == ModuleKind::ES_NEXT && target_mode == ModuleKind::COMMON_JS
}

impl Checker {
    // Go: checker/checker.go:15951 getTypeWithSyntheticDefaultOnly
    pub fn get_type_with_synthetic_default_only(
        &mut self,
        t: TypeId,
        symbol: SymbolId,
        original_symbol: SymbolId,
        module_specifier: Node,
        import_attributes_type: TypeId,
    ) -> TypeId {
        let has_default_only = self.is_only_importable_as_default(
            module_specifier,
            SymbolId::NIL,
            import_attributes_type,
        );
        if has_default_only && t.is_some() && !self.is_error_type(t) {
            let key = CachedTypeKey {
                kind: CachedTypeKind::DEFAULT_ONLY_TYPE,
                type_id: t,
            };
            if let Some(&cached) = self.cached_types.get(&key) {
                if cached.is_some() {
                    return cached;
                }
            }
            let result = self.create_default_property_wrapper_for_module(
                symbol,
                original_symbol,
                SymbolId::NIL,
            );
            self.cached_types.insert(key, result);
            return result;
        }
        TypeId::NIL
    }

    // Go: checker/checker.go:15965 getTypeWithSyntheticDefaultImportType
    pub fn get_type_with_synthetic_default_import_type(
        &mut self,
        t: TypeId,
        symbol: SymbolId,
        original_symbol: SymbolId,
        module_specifier: Node,
    ) -> TypeId {
        if t.is_some() && !self.is_error_type(t) {
            let key = CachedTypeKey {
                kind: CachedTypeKind::SYNTHETIC_TYPE,
                type_id: t,
            };
            if let Some(&cached) = self.cached_types.get(&key) {
                if cached.is_some() {
                    return cached;
                }
            }
            let file = self
                .sym(original_symbol)
                .declarations
                .iter()
                .copied()
                .find(|d| is_source_file(*d))
                .unwrap_or_default();
            let has_synthetic_default = self.can_have_synthetic_default(
                file,
                original_symbol,
                false, /*dontResolveAlias*/
                module_specifier,
            );
            let synthetic_type;
            if has_synthetic_default {
                let anonymous_symbol =
                    self.new_symbol(SymbolFlags::TYPE_LITERAL, INTERNAL_SYMBOL_NAME_TYPE);
                let declarations = self.sym(original_symbol).declarations.clone();
                self.sym_mut(anonymous_symbol).declarations = declarations;
                let default_containing_object = self.create_default_property_wrapper_for_module(
                    symbol,
                    original_symbol,
                    anonymous_symbol,
                );
                self.value_symbol_links
                    .get_by_id(&self.symbols, anonymous_symbol)
                    .resolved_type = default_containing_object;
                if self.is_valid_spread_type(t) {
                    synthetic_type = self.get_spread_type(
                        t,
                        default_containing_object,
                        anonymous_symbol,
                        ObjectFlags(0), /*objectFlags*/
                        false,          /*readonly*/
                    );
                } else {
                    synthetic_type = default_containing_object;
                }
            } else {
                synthetic_type = t;
            }
            self.cached_types.insert(key, synthetic_type);
            return synthetic_type;
        }
        t
    }

    // Go: checker/checker.go:15993 isCommonJSRequire
    pub fn is_common_js_require(&mut self, node: Node) -> bool {
        if !is_require_call(node, true /*requireStringLiteralLikeArgument*/) {
            return false;
        }
        if !is_identifier(node.expression()) {
            panic!("Expected identifier for require call");
        }
        // Make sure require is not a local function
        let resolved_require = self.resolve_name(
            node.expression(),
            node.expression().text(),
            SymbolFlags::VALUE,
            None,  /*nameNotFoundMessage*/
            true,  /*isUse*/
            false, /*excludeGlobals*/
        );
        if resolved_require == self.require_symbol {
            return true;
        }
        // project includes symbol named 'require' - make sure that it is ambient and local non-alias
        if resolved_require.is_nil()
            || self
                .sym(resolved_require)
                .flags
                .intersects(SymbolFlags::ALIAS)
        {
            return false;
        }

        let resolved_flags = self.sym(resolved_require).flags;
        let target_declaration_kind = if resolved_flags.intersects(SymbolFlags::FUNCTION) {
            SyntaxKind::FunctionDeclaration
        } else if resolved_flags.intersects(SymbolFlags::VARIABLE) {
            SyntaxKind::VariableDeclaration
        } else {
            SyntaxKind::Unknown
        };
        if target_declaration_kind != SyntaxKind::Unknown {
            let decl =
                get_declaration_of_kind(&self.symbols, resolved_require, target_declaration_kind);
            // function/variable declaration should be ambient
            return decl.is_some() && decl.flags().intersects(NodeFlags::AMBIENT);
        }
        false
    }

    // Go: checker/checker.go:16026 createDefaultPropertyWrapperForModule
    pub fn create_default_property_wrapper_for_module(
        &mut self,
        symbol: SymbolId,
        original_symbol: SymbolId,
        anonymous_symbol: SymbolId,
    ) -> TypeId {
        let member_table = self.symbols.new_table();
        let new_symbol = self.new_symbol(SymbolFlags::ALIAS, INTERNAL_SYMBOL_NAME_DEFAULT);
        self.sym_mut(new_symbol).parent = original_symbol;
        let name_type = self.get_string_literal_type("default");
        self.value_symbol_links
            .get_by_id(&self.symbols, new_symbol)
            .name_type = name_type;
        let alias_target = self.resolve_symbol(symbol);
        self.alias_symbol_links.get(new_symbol).alias_target = alias_target;
        self.symbols
            .set(member_table, INTERNAL_SYMBOL_NAME_DEFAULT, new_symbol);
        let mut anonymous_symbol = anonymous_symbol;
        if anonymous_symbol.is_nil() && original_symbol.is_some() {
            anonymous_symbol =
                self.new_symbol(SymbolFlags::OBJECT_LITERAL, INTERNAL_SYMBOL_NAME_OBJECT);
            let declarations = self.sym(original_symbol).declarations.clone();
            self.sym_mut(anonymous_symbol).declarations = declarations;
        }
        self.new_anonymous_type(anonymous_symbol, member_table, &[], &[], &[])
    }

    // Go: checker/checker.go:16040 cloneTypeAsModuleType
    pub fn clone_type_as_module_type(
        &mut self,
        symbol: SymbolId,
        module_type: TypeId,
        reference_parent: Node,
    ) -> SymbolId {
        let (flags, name) = {
            let s = self.sym(symbol);
            (s.flags, s.name.clone())
        };
        let result = self.new_symbol(flags, &name);
        let declarations = self.sym(symbol).declarations.clone();
        let value_declaration = self.sym(symbol).value_declaration;
        let (symbol_members, symbol_exports) = (self.sym(symbol).members, self.sym(symbol).exports);
        let members = maps_clone_p17(&mut self.symbols, symbol_members);
        let exports = maps_clone_p17(&mut self.symbols, symbol_exports);
        let parent = self.sym(symbol).parent;
        {
            let r = self.sym_mut(result);
            r.declarations = declarations;
            r.value_declaration = value_declaration;
            r.members = members;
            r.exports = exports;
            r.parent = parent;
        }
        let links = self.export_type_links.get(result);
        links.target = symbol;
        links.originating_import = reference_parent;
        self.resolve_structured_type_members(module_type);
        let (resolved_members, resolved_index_infos) = {
            let st = self.ty(module_type).as_structured_type();
            (st.members, st.index_infos_list())
        };
        let resolved_type =
            self.new_anonymous_type(result, resolved_members, &[], &[], &resolved_index_infos);
        self.value_symbol_links
            .get_by_id(&self.symbols, result)
            .resolved_type = resolved_type;
        result
    }

    // Go: checker/checker.go:16055 getTargetOfAliasDeclaration
    pub fn get_target_of_alias_declaration(&mut self, node: Node) -> SymbolId {
        if node.is_nil() {
            return SymbolId::NIL;
        }
        match node.kind() {
            SyntaxKind::ImportEqualsDeclaration | SyntaxKind::VariableDeclaration => {
                return self.get_target_of_import_equals_declaration(node);
            }
            SyntaxKind::ImportClause => return self.get_target_of_import_clause(node),
            SyntaxKind::NamespaceImport => return self.get_target_of_namespace_import(node),
            SyntaxKind::NamespaceExport => return self.get_target_of_namespace_export(node),
            SyntaxKind::ImportSpecifier | SyntaxKind::BindingElement => {
                return self.get_target_of_import_specifier(node);
            }
            SyntaxKind::ExportSpecifier => {
                return self.get_target_of_export_specifier(
                    node,
                    SymbolFlags::VALUE | SymbolFlags::TYPE | SymbolFlags::NAMESPACE,
                    true, /*dontRecursivelyResolve*/
                );
            }
            SyntaxKind::ExportAssignment => return self.get_target_of_export_assignment(node),
            SyntaxKind::BinaryExpression => return self.get_target_of_binary_expression(node),
            SyntaxKind::NamespaceExportDeclaration => {
                return self.get_target_of_namespace_export_declaration(node);
            }
            SyntaxKind::ShorthandPropertyAssignment => {
                return self.resolve_entity_name(
                    node.name(),
                    SymbolFlags::VALUE | SymbolFlags::TYPE | SymbolFlags::NAMESPACE,
                    true,      /*ignoreErrors*/
                    true,      /*dontRecursivelyResolve*/
                    Node::NIL, /*location*/
                );
            }
            SyntaxKind::PropertyAssignment => {
                return self.get_target_of_alias_like_expression(node.initializer());
            }
            SyntaxKind::ElementAccessExpression | SyntaxKind::PropertyAccessExpression => {
                return self.get_target_of_access_expression(node);
            }
            _ => {}
        }
        panic!(
            "Unhandled case in getTargetOfAliasDeclaration: {:?}",
            node.kind()
        );
    }

    // Go: checker/checker.go:16091 resolveEntityName
    //
    // Resolves a qualified name and any involved aliases.
    pub fn resolve_entity_name(
        &mut self,
        name: Node,
        meaning: SymbolFlags,
        ignore_errors: bool,
        dont_resolve_alias: bool,
        location: Node,
    ) -> SymbolId {
        if node_is_missing(name) {
            return SymbolId::NIL;
        }
        let mut symbol;
        match name.kind() {
            SyntaxKind::Identifier => {
                let mut message: Option<NameNotFound> = None;
                if !ignore_errors {
                    if meaning == SymbolFlags::NAMESPACE || node_is_synthesized(name) {
                        message = Some(NameNotFound::Message(diag::Cannot_find_namespace_0));
                    } else {
                        // PERF: the resolver builds this message only when
                        // the name is not found (see `NameNotFound`).
                        message = Some(NameNotFound::CannotFindName(get_first_identifier(name)));
                    }
                }
                let mut resolve_location = location;
                if resolve_location.is_nil() {
                    resolve_location = name;
                }
                // PERF: U1 (a). The name interned at parse
                // (`Node::text_name`) goes to each resolve call with its
                // text (`resolver_name_text`), so neither the node data nor
                // an intern is needed.
                let name_key = name.text_name();
                if meaning == SymbolFlags::NAMESPACE {
                    let resolved = self.resolve_name(
                        resolve_location,
                        resolver_name_text(name_key.clone()),
                        meaning,
                        None,
                        true,  /*isUse*/
                        false, /*excludeGlobals*/
                    );
                    symbol = self.get_merged_symbol(resolved);
                    if symbol.is_nil() {
                        let resolved_alias = self.resolve_name(
                            resolve_location,
                            resolver_name_text(name_key.clone()),
                            SymbolFlags::ALIAS,
                            None,
                            true,  /*isUse*/
                            false, /*excludeGlobals*/
                        );
                        let alias = self.get_merged_symbol(resolved_alias);
                        if alias.is_some()
                            && self.sym(alias).name == INTERNAL_SYMBOL_NAME_EXPORT_EQUALS
                        {
                            // resolve typedefs exported from commonjs, stored on the module symbol
                            symbol = self.sym(alias).parent;
                        }
                    }
                    if symbol.is_nil() && message.is_some() {
                        let resolve_name = self.resolve_name.clone();
                        resolve_name(
                            self,
                            resolve_location,
                            resolver_name_text(name_key),
                            meaning,
                            message,
                            true,  /*isUse*/
                            false, /*excludeGlobals*/
                        );
                    }
                } else {
                    let resolve_name = self.resolve_name.clone();
                    let resolved = resolve_name(
                        self,
                        resolve_location,
                        resolver_name_text(name_key),
                        meaning,
                        message,
                        true,  /*isUse*/
                        false, /*excludeGlobals*/
                    );
                    symbol = self.get_merged_symbol(resolved);
                }
            }
            SyntaxKind::QualifiedName => {
                symbol = self.resolve_qualified_name(
                    name,
                    name.left(),
                    name.right(),
                    meaning,
                    ignore_errors,
                    location,
                );
            }
            SyntaxKind::PropertyAccessExpression => {
                symbol = self.resolve_qualified_name(
                    name,
                    name.expression(),
                    name.name(),
                    meaning,
                    ignore_errors,
                    location,
                );
            }
            _ => panic!("Unknown entity name kind"),
        }
        if symbol.is_some() && symbol != self.unknown_symbol {
            if !node_is_synthesized(name)
                && is_entity_name(name)
                && (self.sym(symbol).flags.intersects(SymbolFlags::ALIAS)
                    || name.parent().is_some()
                        && name.parent().kind() == SyntaxKind::ExportAssignment)
            {
                self.mark_symbol_of_alias_declaration_if_type_only(
                    get_alias_declaration_from_name(name),
                    Node::NIL,
                );
            }
            // We know a symbol with the given meaning exists along the alias chain, so resolve until we find it.
            while !self.sym(symbol).flags.intersects(meaning)
                && !dont_resolve_alias
                && self.sym(symbol).flags.intersects(SymbolFlags::ALIAS)
            {
                symbol = self.resolve_alias(symbol);
            }
        }
        symbol
    }
}

// PORT: Go `maps.Clone(table)` on an `ast.SymbolTable`. A nil map clones to
// nil; otherwise a new table with the same entries.
fn maps_clone_p17(symbols: &mut SymbolArena, table: SymbolTable) -> SymbolTable {
    symbols.clone_table(table)
}

/// Private copies of Go `tspath` functions used by this unit. tspath has no
/// Go-shaped port in this crate, so these follow the Go code exactly.
pub(crate) mod tspath_p17 {
    // Go: tspath/extension.go constants
    pub const EXTENSION_TS: &str = ".ts";
    pub const EXTENSION_TSX: &str = ".tsx";
    pub const EXTENSION_DTS: &str = ".d.ts";
    pub const EXTENSION_JS: &str = ".js";
    pub const EXTENSION_JSX: &str = ".jsx";
    pub const EXTENSION_JSON: &str = ".json";
    pub const EXTENSION_MJS: &str = ".mjs";
    pub const EXTENSION_MTS: &str = ".mts";
    pub const EXTENSION_DMTS: &str = ".d.mts";
    pub const EXTENSION_CJS: &str = ".cjs";
    pub const EXTENSION_CTS: &str = ".cts";
    pub const EXTENSION_DCTS: &str = ".d.cts";

    const SUPPORTED_DECLARATION_EXTENSIONS: [&str; 3] =
        [EXTENSION_DTS, EXTENSION_DCTS, EXTENSION_DMTS];
    const SUPPORTED_TS_EXTENSIONS_FOR_EXTRACT_EXTENSION: [&str; 7] = [
        EXTENSION_DTS,
        EXTENSION_DCTS,
        EXTENSION_DMTS,
        EXTENSION_TS,
        EXTENSION_TSX,
        EXTENSION_MTS,
        EXTENSION_CTS,
    ];
    pub const SUPPORTED_TS_EXTENSIONS_FLAT: [&str; 7] = [
        EXTENSION_TS,
        EXTENSION_TSX,
        EXTENSION_DTS,
        EXTENSION_CTS,
        EXTENSION_DCTS,
        EXTENSION_MTS,
        EXTENSION_DMTS,
    ];
    const EXTENSIONS_TO_REMOVE: [&str; 12] = [
        EXTENSION_DTS,
        EXTENSION_DMTS,
        EXTENSION_DCTS,
        EXTENSION_MJS,
        EXTENSION_MTS,
        EXTENSION_CJS,
        EXTENSION_CTS,
        EXTENSION_TS,
        EXTENSION_JS,
        EXTENSION_TSX,
        EXTENSION_JSX,
        EXTENSION_JSON,
    ];

    // Go: tspath/path.go:988 ComparePathsOptions
    pub struct ComparePathsOptions {
        pub use_case_sensitive_file_names: bool,
        pub current_directory: String,
    }

    // Go: tspath/path.go:997 ComparePathsOptions.getEqualityComparer
    // PORT: `stringutil.GetStringEqualityComparer(!o.UseCaseSensitiveFileNames)`.
    fn equal_with_options(options: &ComparePathsOptions, a: &str, b: &str) -> bool {
        if !options.use_case_sensitive_file_names {
            equate_string_case_insensitive(a, b)
        } else {
            a == b
        }
    }

    // Go: stringutil/compare.go:9 EquateStringCaseInsensitive
    // PORT: Go `strings.EqualFold` uses Unicode simple case folding. This
    // compares each rune by its simple lowercase and uppercase forms, which
    // gives the same result for every rune with a one-to-one case mapping.
    fn equate_string_case_insensitive(a: &str, b: &str) -> bool {
        let mut ai = a.chars();
        let mut bi = b.chars();
        loop {
            match (ai.next(), bi.next()) {
                (None, None) => return true,
                (Some(x), Some(y)) => {
                    if x == y {
                        continue;
                    }
                    let fold = |c: char| {
                        let mut l = c.to_lowercase();
                        match (l.next(), l.next()) {
                            (Some(single), None) => single,
                            _ => c,
                        }
                    };
                    let upper = |c: char| {
                        let mut u = c.to_uppercase();
                        match (u.next(), u.next()) {
                            (Some(single), None) => single,
                            _ => c,
                        }
                    };
                    if fold(x) != fold(y) && upper(x) != upper(y) {
                        return false;
                    }
                }
                _ => return false,
            }
        }
    }

    // Go: tspath/path.go:27 isAnyDirectorySeparator
    fn is_any_directory_separator(ch: u8) -> bool {
        ch == b'/' || ch == b'\\'
    }

    // Go: tspath/path.go:38 IsRootedDiskPath
    pub fn is_rooted_disk_path(path: &str) -> bool {
        get_encoded_root_length(path) > 0
    }

    // Go: tspath/path.go:67 PathIsAbsolute
    fn path_is_absolute(path: &str) -> bool {
        get_encoded_root_length(path) != 0
    }

    // Go: tspath/path.go:71 HasTrailingDirectorySeparator
    fn has_trailing_directory_separator(path: &str) -> bool {
        !path.is_empty() && is_any_directory_separator(path.as_bytes()[path.len() - 1])
    }

    // Go: tspath/path.go:91 CombinePaths
    // PORT: Go builds into one `strings.Builder` and slices from `start`; the
    // result string is the same as building the current result directly.
    fn combine_paths(first_path: &str, paths: &[&str]) -> String {
        let mut result = normalize_slashes(first_path);
        for trailing_path in paths {
            if trailing_path.is_empty() {
                continue;
            }
            let trailing_path = normalize_slashes(trailing_path);
            if result.is_empty() || get_root_length(&trailing_path) != 0 {
                // `trailingPath` is absolute.
                result = trailing_path;
            } else {
                if !has_trailing_directory_separator(&result) {
                    result.push('/');
                }
                result.push_str(&trailing_path);
            }
        }
        result
    }

    // Go: tspath/path.go:134 GetPathComponents
    fn get_path_components(path: &str, current_directory: &str) -> Vec<String> {
        let path = combine_paths(current_directory, &[path]);
        let root_length = get_root_length(&path);
        path_components(&path, root_length)
    }

    // Go: tspath/path.go:139 pathComponents
    fn path_components(path: &str, root_length: usize) -> Vec<String> {
        let root = &path[..root_length];
        let mut rest: Vec<String> = path[root_length..].split('/').map(str::to_string).collect();
        if rest.last().is_some_and(String::is_empty) {
            rest.pop();
        }
        let mut result = vec![root.to_string()];
        result.extend(rest);
        result
    }

    // Go: tspath/path.go:148 IsVolumeCharacter
    fn is_volume_character(ch: u8) -> bool {
        ch.is_ascii_lowercase() || ch.is_ascii_uppercase()
    }

    // Go: tspath/path.go:152 getFileUrlVolumeSeparatorEnd
    fn get_file_url_volume_separator_end(url: &[u8], start: usize) -> i32 {
        if url.len() <= start {
            return -1;
        }
        let ch0 = url[start];
        if ch0 == b':' {
            return (start + 1) as i32;
        }
        if ch0 == b'%' && url.len() > start + 2 && url[start + 1] == b'3' {
            let ch2 = url[start + 2];
            if ch2 == b'a' || ch2 == b'A' {
                return (start + 3) as i32;
            }
        }
        -1
    }

    // Go: tspath/path.go:169 GetEncodedRootLength
    fn get_encoded_root_length(path: &str) -> i32 {
        let b = path.as_bytes();
        let ln = b.len();
        if ln == 0 {
            return 0;
        }
        let ch0 = b[0];

        // POSIX or UNC
        if ch0 == b'/' || ch0 == b'\\' {
            if ln == 1 || b[1] != ch0 {
                return 1; // POSIX: "/" (or non-normalized "\")
            }
            let offset = 2;
            return match b[offset..].iter().position(|&c| c == ch0) {
                None => ln as i32,                    // UNC: "//server" or "\\server"
                Some(p1) => (p1 + offset + 1) as i32, // UNC: "//server/" or "\\server\"
            };
        }

        // DOS
        if is_volume_character(ch0) && ln > 1 && b[1] == b':' {
            if ln == 2 {
                return 2; // DOS: "c:" (but not "c:d")
            }
            let ch2 = b[2];
            if ch2 == b'/' || ch2 == b'\\' {
                return 3; // DOS: "c:/" or "c:\"
            }
        }

        // Untitled paths (e.g., "^/untitled/ts-nul-authority/Untitled-1")
        if ch0 == b'^' && ln > 1 && b[1] == b'/' {
            return 2; // Untitled: "^/"
        }

        // URL
        const URL_SCHEME_SEPARATOR: &str = "://";
        if let Some(scheme_end) = path.find(URL_SCHEME_SEPARATOR) {
            let authority_start = scheme_end + URL_SCHEME_SEPARATOR.len();
            if let Some(authority_length) = path[authority_start..].find('/') {
                // URL: "file:///", "file://server/", "file://server/path"
                let authority_end = authority_start + authority_length;

                // For local "file" URLs, include the leading DOS volume (if present).
                let scheme = &path[..scheme_end];
                let authority = &path[authority_start..authority_end];
                if scheme == "file"
                    && (authority.is_empty() || authority == "localhost")
                    && (ln > authority_end + 2)
                    && is_volume_character(b[authority_end + 1])
                {
                    let volume_separator_end =
                        get_file_url_volume_separator_end(b, authority_end + 2);
                    if volume_separator_end != -1 {
                        if volume_separator_end as usize == ln {
                            return !volume_separator_end;
                        }
                        if b[volume_separator_end as usize] == b'/' {
                            return !(volume_separator_end + 1);
                        }
                    }
                }
                return !((authority_end + 1) as i32); // URL: "file://server/", "http://server/"
            }
            return !(ln as i32); // URL: "file://server", "http://server"
        }

        // relative
        0
    }

    // Go: tspath/path.go:243 GetRootLength
    fn get_root_length(path: &str) -> usize {
        let root_length = get_encoded_root_length(path);
        if root_length < 0 {
            return (!root_length) as usize;
        }
        root_length as usize
    }

    // Go: tspath/path.go:251 GetDirectoryPath
    pub fn get_directory_path(path: &str) -> String {
        let path = normalize_slashes(path);

        // If the path provided is itself a root, then return it.
        let root_length = get_root_length(&path);
        if root_length == path.len() {
            return path;
        }

        // return the leading portion of the path up to the last (non-terminal) directory separator
        // but not including any trailing directory separator.
        let path = remove_trailing_directory_separator(&path);
        let last = path.rfind('/').map_or(-1, |i| i as isize);
        path[..(root_length as isize).max(last) as usize].to_string()
    }

    // Go: tspath/path.go:270 GetPathFromPathComponents
    pub fn get_path_from_path_components(path_components: &[String]) -> String {
        if path_components.is_empty() {
            return String::new();
        }

        let mut root = path_components[0].clone();
        if !root.is_empty() {
            root = ensure_trailing_directory_separator(&root);
        }

        root + &path_components[1..].join("/")
    }

    // Go: tspath/path.go:283 NormalizeSlashes
    pub fn normalize_slashes(path: &str) -> String {
        path.replace('\\', "/")
    }

    // Go: tspath/path.go:287 reducePathComponents
    fn reduce_path_components(components: Vec<String>) -> Vec<String> {
        if components.is_empty() {
            return Vec::new();
        }
        let mut reduced = vec![components[0].clone()];
        for component in components.into_iter().skip(1) {
            if component.is_empty() {
                continue;
            }
            if component == "." {
                continue;
            }
            if component == ".." {
                if reduced.len() > 1 {
                    if reduced[reduced.len() - 1] != ".." {
                        reduced.pop();
                        continue;
                    }
                } else if !reduced[0].is_empty() {
                    continue;
                }
            }
            reduced.push(component);
        }
        reduced
    }

    // Go: tspath/path.go:394 GetNormalizedAbsolutePath
    pub fn get_normalized_absolute_path(file_name: &str, current_directory: &str) -> String {
        let mut root_length = get_root_length(file_name);
        let file_name = if root_length == 0 && !current_directory.is_empty() {
            combine_paths(current_directory, &[file_name])
        } else {
            // CombinePaths normalizes slashes, so not necessary in other branch
            normalize_slashes(file_name)
        };
        root_length = get_root_length(&file_name);

        if let Some(simple_normalized) = simple_normalize_path(&file_name) {
            let length = simple_normalized.len();
            if length > root_length {
                return remove_trailing_directory_separator(&simple_normalized).to_string();
            }
            if length == root_length && root_length != 0 {
                return ensure_trailing_directory_separator(&simple_normalized);
            }
            return simple_normalized;
        }

        let fb = file_name.as_bytes();
        let length = fb.len();
        let root = &file_name[..root_length];
        // `normalized` is only initialized once `fileName` is determined to be non-normalized.
        // `changed` is set at the same time.
        let mut changed = false;
        let mut normalized = String::new();
        let mut segment_start;
        let mut index = root_length;
        let mut normalized_up_to = index;
        let mut seen_non_dot_dot_segment = root_length != 0;
        while index < length {
            // At beginning of segment
            segment_start = index;
            let mut ch = fb[index];
            while ch == b'/' {
                index += 1;
                if index < length {
                    ch = fb[index];
                } else {
                    break;
                }
            }
            if index > segment_start {
                // Seen superfluous separator
                if !changed {
                    normalized = file_name[..root_length.max(segment_start - 1)].to_string();
                    changed = true;
                }
                if index == length {
                    break;
                }
                segment_start = index;
            }
            // Past any superfluous separators
            let segment_end = match file_name[index + 1..].find('/') {
                None => length,
                Some(i) => i + index + 1,
            };
            let segment_length = segment_end - segment_start;
            if segment_length == 1 && fb[index] == b'.' {
                // "." segment (skip)
                if !changed {
                    normalized = file_name[..normalized_up_to].to_string();
                    changed = true;
                }
            } else if segment_length == 2 && fb[index] == b'.' && fb[index + 1] == b'.' {
                // ".." segment
                if !seen_non_dot_dot_segment {
                    if changed {
                        if normalized.len() == root_length {
                            normalized.push_str("..");
                        } else {
                            normalized.push_str("/..");
                        }
                    } else {
                        normalized_up_to = index + 2;
                    }
                } else if !changed {
                    if normalized_up_to as isize - 1 >= 0 {
                        let last = file_name[..normalized_up_to - 1]
                            .rfind('/')
                            .map_or(-1, |i| i as isize);
                        normalized =
                            file_name[..(root_length as isize).max(last) as usize].to_string();
                    } else {
                        normalized = file_name[..normalized_up_to].to_string();
                    }
                    changed = true;
                    seen_non_dot_dot_segment = (normalized.len() != root_length
                        || root_length != 0)
                        && normalized != ".."
                        && !normalized.ends_with("/..");
                } else {
                    match normalized.rfind('/') {
                        Some(last_slash) => {
                            normalized = normalized[..root_length.max(last_slash)].to_string();
                        }
                        None => {
                            normalized = root.to_string();
                        }
                    }
                    seen_non_dot_dot_segment = (normalized.len() != root_length
                        || root_length != 0)
                        && normalized != ".."
                        && !normalized.ends_with("/..");
                }
            } else if changed {
                if normalized.len() != root_length {
                    normalized.push('/');
                }
                seen_non_dot_dot_segment = true;
                normalized.push_str(&file_name[segment_start..segment_end]);
            } else {
                seen_non_dot_dot_segment = true;
                normalized_up_to = segment_end;
            }
            index = segment_end + 1;
        }
        if changed {
            return normalized;
        }
        if length > root_length {
            return remove_trailing_directory_separators(&file_name).to_string();
        }
        if length == root_length {
            return ensure_trailing_directory_separator(&file_name);
        }
        file_name
    }

    // Go: tspath/path.go:515 simpleNormalizePath
    // PORT: Go returns `(string, bool)`; `None` is `("", false)`.
    fn simple_normalize_path(path: &str) -> Option<String> {
        // Most paths don't require normalization
        if !has_relative_path_segment(path) {
            return Some(path.to_string());
        }
        // Some paths only require cleanup of `/./` or leading `./`
        let simplified = path.replace("/./", "/");
        let trimmed = simplified.strip_prefix("./").unwrap_or(&simplified);
        if trimmed != path
            && !has_relative_path_segment(trimmed)
            && !(trimmed != simplified && trimmed.starts_with('/'))
        {
            // If we trimmed a leading "./" and the path now starts with "/", we changed the meaning
            return Some(trimmed.to_string());
        }
        None
    }

    // Go: tspath/path.go:532 hasRelativePathSegment
    // hasRelativePathSegment reports whether p contains ".", "..", "./", "../", "/.", "/..", "//", "/./", or "/../".
    fn has_relative_path_segment(p: &str) -> bool {
        let b = p.as_bytes();
        let n = b.len();
        if n == 0 {
            return false;
        }

        if p == "." || p == ".." {
            return true;
        }

        // Leading "./" OR "../"
        if b[0] == b'.' {
            if n >= 2 && b[1] == b'/' {
                return true;
            }
            // Leading "../"
            if n >= 3 && b[1] == b'.' && b[2] == b'/' {
                return true;
            }
        }
        // Trailing "/." OR "/.."
        if b[n - 1] == b'.' {
            if n >= 2 && b[n - 2] == b'/' {
                return true;
            }
            if n >= 3 && b[n - 2] == b'.' && b[n - 3] == b'/' {
                return true;
            }
        }

        // Now look for any `//` or `/./` or `/../`

        let mut prev_slash = false;
        let mut seg_len = 0; // length of current segment since last slash
        let mut dot_count: i32 = 0; // consecutive dots at start of the current segment; -1 => not only dots

        for &c in b {
            if c == b'/' {
                // "//"
                if prev_slash {
                    return true;
                }
                // "/./" or "/../"
                if (seg_len == 1 && dot_count == 1) || (seg_len == 2 && dot_count == 2) {
                    return true;
                }
                prev_slash = true;
                seg_len = 0;
                dot_count = 0;
                continue;
            }

            if c == b'.' {
                if dot_count >= 0 {
                    dot_count += 1;
                }
            } else {
                dot_count = -1;
            }
            seg_len += 1;
            prev_slash = false;
        }

        // Trailing "/." or "/.."
        (seg_len == 1 && dot_count == 1) || (seg_len == 2 && dot_count == 2)
    }

    // Go: tspath/path.go:600 NormalizePath
    fn normalize_path(path: &str) -> String {
        let path = normalize_slashes(path);
        if let Some(normalized) = simple_normalize_path(&path) {
            return normalized;
        }
        let mut normalized = get_normalized_absolute_path(&path, "");
        if !normalized.is_empty() && has_trailing_directory_separator(&path) {
            normalized = ensure_trailing_directory_separator(&normalized);
        }
        normalized
    }

    // Go: tspath/path.go:612 GetCanonicalFileName
    fn get_canonical_file_name(file_name: &str, use_case_sensitive_file_names: bool) -> String {
        if use_case_sensitive_file_names {
            return file_name.to_string();
        }
        to_file_name_lower_case(file_name)
    }

    // Go: tspath/path.go:674 ToFileNameLowerCase
    // PORT: Go `unicode.ToLower` maps one rune to one rune. Rust
    // `char::to_lowercase` can yield several; a multi-rune result keeps the
    // original rune, which matches Go for those runes. Go `strings.Map`
    // writes each byte that is not valid UTF-8 as U+FFFD (`go_map_runes`).
    fn to_file_name_lower_case(file_name: &str) -> String {
        const I_WITH_DOT: char = '\u{0130}';

        if file_name.is_ascii() {
            return file_name.to_ascii_lowercase();
        }

        crate::scanner_util::go_map_runes(file_name, |r| {
            if r == I_WITH_DOT {
                return r;
            }
            let mut l = r.to_lowercase();
            match (l.next(), l.next()) {
                (Some(single), None) => single,
                _ => r,
            }
        })
    }

    // Go: tspath/path.go:723 ToPath
    // PORT: Go `tspath.Path` is a string type; this returns the string.
    pub fn to_path(
        file_name: &str,
        base_path: &str,
        use_case_sensitive_file_names: bool,
    ) -> String {
        let non_canonicalized_path = if is_rooted_disk_path(file_name) {
            normalize_path(file_name)
        } else {
            get_normalized_absolute_path(file_name, base_path)
        };
        get_canonical_file_name(&non_canonicalized_path, use_case_sensitive_file_names)
    }

    // Go: tspath/path.go:733 RemoveTrailingDirectorySeparator
    fn remove_trailing_directory_separator(path: &str) -> &str {
        if has_trailing_directory_separator(path) {
            return &path[..path.len() - 1];
        }
        path
    }

    // Go: tspath/path.go:744 RemoveTrailingDirectorySeparators
    fn remove_trailing_directory_separators(path: &str) -> &str {
        let mut path = path;
        while has_trailing_directory_separator(path) {
            path = remove_trailing_directory_separator(path);
        }
        path
    }

    // Go: tspath/path.go:751 EnsureTrailingDirectorySeparator
    fn ensure_trailing_directory_separator(path: &str) -> String {
        if !has_trailing_directory_separator(path) {
            return format!("{path}/");
        }
        path.to_string()
    }

    // Go: tspath/path.go:765 GetPathComponentsRelativeTo
    pub fn get_path_components_relative_to(
        from: &str,
        to: &str,
        options: &ComparePathsOptions,
    ) -> Vec<String> {
        let from_components =
            reduce_path_components(get_path_components(from, &options.current_directory));
        let to_components =
            reduce_path_components(get_path_components(to, &options.current_directory));

        let mut start = 0;
        let max_common_components = from_components.len().min(to_components.len());
        while start < max_common_components {
            let from_component = &from_components[start];
            let to_component = &to_components[start];
            if start == 0 {
                if !equate_string_case_insensitive(from_component, to_component) {
                    break;
                }
            } else if !equal_with_options(options, from_component, to_component) {
                break;
            }
            start += 1;
        }

        if start == 0 {
            return to_components;
        }

        let num_dot_dot_slashes = from_components.len() - start;
        let mut result = Vec::with_capacity(1 + num_dot_dot_slashes + to_components.len() - start);

        result.push(String::new());
        // Add all the relative components until we hit a common directory.
        for _ in 0..num_dot_dot_slashes {
            result.push("..".to_string());
        }
        // Now add all the remaining components of the "to" path.
        for component in &to_components[start..] {
            result.push(component.clone());
        }

        result
    }

    // Go: tspath/path.go:809 GetRelativePathFromDirectory
    pub fn get_relative_path_from_directory(
        from_directory: &str,
        to: &str,
        options: &ComparePathsOptions,
    ) -> String {
        if (get_root_length(from_directory) > 0) != (get_root_length(to) > 0) {
            panic!("paths must either both be absolute or both be relative");
        }
        let path_components = get_path_components_relative_to(from_directory, to, options);
        get_path_from_path_components(&path_components)
    }

    // Go: tspath/path.go:817 GetRelativePathFromFile
    pub fn get_relative_path_from_file(
        from: &str,
        to: &str,
        options: &ComparePathsOptions,
    ) -> String {
        ensure_path_is_non_module_name(&get_relative_path_from_directory(
            &get_directory_path(from),
            to,
            options,
        ))
    }

    // Go: tspath/path.go:876 GetBaseFileName
    // PERF: returns a slice of `path`. NormalizeSlashes swaps one byte for
    // one byte, and the base name has no separator, so the same byte range
    // of `path` holds the same text. Only a path with a backslash builds the
    // normalized copy.
    fn get_base_file_name(path: &str) -> &str {
        let range = if path.contains('\\') {
            base_file_name_range(&normalize_slashes(path))
        } else {
            base_file_name_range(path)
        };
        &path[range]
    }

    // The byte range of the Go GetBaseFileName result in the normalized `path`.
    fn base_file_name_range(path: &str) -> std::ops::Range<usize> {
        // if the path provided is itself the root, then it has no file name.
        let root_length = get_root_length(path);
        if root_length == path.len() {
            return 0..0;
        }

        // return the trailing portion of the path starting after the last (non-terminal) directory
        // separator but not including any trailing directory separator.
        let path = remove_trailing_directory_separator(path);
        let after_last = path.rfind('/').map_or(0, |i| i + 1);
        get_root_length(path).max(after_last)..path.len()
    }

    // Go: tspath/path.go:902 GetAnyExtensionFromPath
    pub fn get_any_extension_from_path(
        path: &str,
        extensions: &[&str],
        ignore_case: bool,
    ) -> String {
        // Retrieves any string from the final "." onwards from a base file name.
        // Unlike extensionFromPath, which throws an exception on unrecognized extensions.
        if !extensions.is_empty() {
            return get_any_extension_from_path_worker(
                remove_trailing_directory_separator(path),
                extensions,
                ignore_case,
            );
        }

        let base_file_name = get_base_file_name(path);
        match base_file_name.rfind('.') {
            Some(extension_index) => base_file_name[extension_index..].to_string(),
            None => String::new(),
        }
    }

    // Go: tspath/path.go:931 getAnyExtensionFromPathWorker
    // PORT: the Go equality comparer is `stringutil.GetStringEqualityComparer(ignoreCase)`.
    fn get_any_extension_from_path_worker(
        path: &str,
        extensions: &[&str],
        ignore_case: bool,
    ) -> String {
        for extension in extensions {
            let result = try_get_extension_from_path_with(path, extension, ignore_case);
            if !result.is_empty() {
                return result;
            }
        }
        String::new()
    }

    // Go: tspath/path.go:941 tryGetExtensionFromPath
    fn try_get_extension_from_path_with(path: &str, extension: &str, ignore_case: bool) -> String {
        let extension = if !extension.starts_with('.') {
            format!(".{extension}")
        } else {
            extension.to_string()
        };
        if path.len() >= extension.len() && path.as_bytes()[path.len() - extension.len()] == b'.' {
            let path_extension = &path[path.len() - extension.len()..];
            let equal = if ignore_case {
                equate_string_case_insensitive(path_extension, &extension)
            } else {
                path_extension == extension
            };
            if equal {
                return path_extension.to_string();
            }
        }
        String::new()
    }

    // Go: tspath/path.go:954 PathIsRelative
    pub fn path_is_relative(path: &str) -> bool {
        // True if path is ".", "..", or starts with "./", "../", ".\\", or "..\\".
        if path == "." || path == ".." {
            return true;
        }
        let b = path.as_bytes();
        if b.len() >= 2 && b[0] == b'.' && (b[1] == b'/' || b[1] == b'\\') {
            return true;
        }
        if b.len() >= 3 && b[0] == b'.' && b[1] == b'.' && (b[2] == b'/' || b[2] == b'\\') {
            return true;
        }
        false
    }

    // Go: tspath/path.go:974 EnsurePathIsNonModuleName
    fn ensure_path_is_non_module_name(path: &str) -> String {
        if !path_is_absolute(path) && !path_is_relative(path) {
            return format!("./{path}");
        }
        path.to_string()
    }

    // Go: tspath/path.go:981 IsExternalModuleNameRelative
    pub fn is_external_module_name_relative(module_name: &str) -> bool {
        // TypeScript 1.0 spec (April 2014): 11.2.1
        // An external module name is "relative" if the first term is "." or "..".
        // Update: We also consider a path like `C:\foo.ts` "relative" because we do not search for it in `node_modules` or treat it as an ambient module.
        path_is_relative(module_name) || is_rooted_disk_path(module_name)
    }

    // Go: tspath/path.go:1095 FileExtensionIs
    pub fn file_extension_is(path: &str, extension: &str) -> bool {
        path.len() > extension.len() && path.ends_with(extension)
    }

    // Go: tspath/path.go:1139 HasExtension
    pub fn has_extension(file_name: &str) -> bool {
        get_base_file_name(file_name).contains('.')
    }

    // Go: tspath/extension.go:39 ExtensionIsTs
    pub fn extension_is_ts(ext: &str) -> bool {
        ext == EXTENSION_TS
            || ext == EXTENSION_TSX
            || ext == EXTENSION_DTS
            || ext == EXTENSION_MTS
            || ext == EXTENSION_DMTS
            || ext == EXTENSION_CTS
            || ext == EXTENSION_DCTS
            || ext.len() >= 7 && &ext[..3] == ".d." && &ext[ext.len() - 3..] == ".ts"
    }

    // Go: tspath/extension.go TryGetExtensionFromPath
    pub fn try_get_extension_from_path(p: &str) -> &'static str {
        for ext in EXTENSIONS_TO_REMOVE {
            if file_extension_is(p, ext) {
                return ext;
            }
        }
        ""
    }

    // Go: tspath/extension.go RemoveExtension
    pub fn remove_extension<'a>(path: &'a str, extension: &str) -> &'a str {
        &path[..path.len() - extension.len()]
    }

    // Go: tspath/extension.go FileExtensionIsOneOf
    fn file_extension_is_one_of(path: &str, extensions: &[&str]) -> bool {
        extensions.iter().any(|ext| file_extension_is(path, ext))
    }

    // Go: tspath/extension.go TryExtractTSExtension
    pub fn try_extract_ts_extension(file_name: &str) -> &'static str {
        for ext in SUPPORTED_TS_EXTENSIONS_FOR_EXTRACT_EXTENSION {
            if file_extension_is(file_name, ext) {
                return ext;
            }
        }
        ""
    }

    // Go: tspath/extension.go HasTSFileExtension
    pub fn has_ts_file_extension(path: &str) -> bool {
        file_extension_is_one_of(path, &SUPPORTED_TS_EXTENSIONS_FLAT)
    }

    // Go: tspath/extension.go IsDeclarationFileName
    pub fn is_declaration_file_name(file_name: &str) -> bool {
        !get_declaration_file_extension(file_name).is_empty()
    }

    // Go: tspath/extension.go GetDeclarationFileExtension
    // PERF: returns a slice of `file_name` or a constant, not a copy.
    fn get_declaration_file_extension(file_name: &str) -> &str {
        let base = get_base_file_name(file_name);
        for ext in SUPPORTED_DECLARATION_EXTENSIONS {
            if base.ends_with(ext) {
                return ext;
            }
        }
        if base.ends_with(EXTENSION_TS) {
            if let Some(index) = base.find(".d.") {
                return &base[index..];
            }
        }
        ""
    }
}

/// Private copies of Go `core` helpers used by this unit.
mod core_p17 {
    use crate::prelude::*;

    // Go: core/nodemodules.go UnprefixedNodeCoreModules
    const UNPREFIXED_NODE_CORE_MODULES: [&str; 54] = [
        "assert",
        "assert/strict",
        "async_hooks",
        "buffer",
        "child_process",
        "cluster",
        "console",
        "constants",
        "crypto",
        "dgram",
        "diagnostics_channel",
        "dns",
        "dns/promises",
        "domain",
        "events",
        "fs",
        "fs/promises",
        "http",
        "http2",
        "https",
        "inspector",
        "inspector/promises",
        "module",
        "net",
        "os",
        "path",
        "path/posix",
        "path/win32",
        "perf_hooks",
        "process",
        "punycode",
        "querystring",
        "readline",
        "readline/promises",
        "repl",
        "stream",
        "stream/consumers",
        "stream/promises",
        "stream/web",
        "string_decoder",
        "sys",
        "timers",
        "timers/promises",
        "tls",
        "trace_events",
        "tty",
        "url",
        "util",
        "util/types",
        "v8",
        "vm",
        "wasi",
        "worker_threads",
        "zlib",
    ];

    // Go: core/nodemodules.go ExclusivelyPrefixedNodeCoreModules
    const EXCLUSIVELY_PREFIXED_NODE_CORE_MODULES: [&str; 5] = [
        "node:quic",
        "node:sea",
        "node:sqlite",
        "node:test",
        "node:test/reporters",
    ];

    // Go: core/nodemodules.go NodeCoreModules
    // PORT: Go builds a map once (each unprefixed name plus its "node:" form,
    // plus the exclusively prefixed names). This checks membership directly.
    pub fn is_node_core_module(name: &str) -> bool {
        let unprefixed = name.strip_prefix("node:").unwrap_or(name);
        UNPREFIXED_NODE_CORE_MODULES.contains(&unprefixed)
            || EXCLUSIVELY_PREFIXED_NODE_CORE_MODULES.contains(&name)
    }

    // Go: core/core.go:724 ShouldRewriteModuleSpecifier
    pub fn should_rewrite_module_specifier(
        specifier: &str,
        compiler_options: &CompilerOptions,
    ) -> bool {
        compiler_options
            .rewrite_relative_import_extensions
            .is_true()
            && super::tspath_p17::path_is_relative(specifier)
            && !super::tspath_p17::is_declaration_file_name(specifier)
            && super::tspath_p17::has_ts_file_extension(specifier)
    }

    // Go: core/pattern.go FindBestPatternMatch
    // PORT: `ast.PatternAmbientModule.Pattern` is stored as a prefix and a
    // suffix. Ambient module patterns always contain a star, so Go
    // `StarIndex` is the prefix length and `Pattern.Matches` checks
    // `len(candidate) >= len(Text)-1` plus the prefix and suffix.
    pub fn find_best_pattern_match<'a>(
        values: &'a [PatternAmbientModule],
        candidate: &str,
    ) -> Option<&'a PatternAmbientModule> {
        let mut best_pattern = None;
        let mut longest_match_prefix_length: isize = -1;
        // PORT: Go compares bytes. The texts are port forms, so this
        // compares their Go bytes (see `scanner_util::GO_STRING_MARKER`).
        let candidate = go_string_bytes(candidate);
        for value in values {
            let star_index = go_len(&value.pattern_prefix) as isize;
            let matches = pattern_matches_go_bytes(value, &candidate);
            if star_index > longest_match_prefix_length && matches {
                best_pattern = Some(value);
                longest_match_prefix_length = star_index;
            }
        }
        best_pattern
    }

    // Go: core/pattern.go:22 Pattern.Matches
    // PORT: for `ast.PatternAmbientModule.Pattern`, which always has a star
    // (see `find_best_pattern_match`). `candidate` is the Go bytes of the
    // candidate (`go_string_bytes`), so a caller that tests many patterns
    // makes them once.
    // PERF: a first or last byte that differs rejects the pattern before
    // the `memcmp` call of `starts_with` or `ends_with`.
    pub fn pattern_matches_go_bytes(value: &PatternAmbientModule, candidate: &[u8]) -> bool {
        let prefix = go_string_bytes(&value.pattern_prefix);
        let suffix = go_string_bytes(&value.pattern_suffix);
        candidate.len() >= prefix.len() + suffix.len()
            && prefix.first().is_none_or(|&b| candidate[0] == b)
            && suffix
                .last()
                .is_none_or(|&b| candidate[candidate.len() - 1] == b)
            && candidate.starts_with(&prefix)
            && candidate.ends_with(&suffix)
    }
}

/// Private copy of Go `module.GetResolutionDiagnostic`.
mod module_p17 {
    use super::tspath_p17::*;
    use crate::diagnostics::Message;
    use crate::prelude::*;

    // Go: module/util.go:125 GetResolutionDiagnostic
    pub fn get_resolution_diagnostic(
        options: &CompilerOptions,
        resolved_module: &ResolvedModule,
        file: Node,
    ) -> Option<&'static Message> {
        let need_jsx = || -> Option<&'static Message> {
            if options.jsx != JsxEmit::NONE {
                return None;
            }
            Some(diag::Module_0_was_resolved_to_1_but_jsx_is_not_set)
        };

        let need_allow_js = || -> Option<&'static Message> {
            if options.get_allow_js()
                || !options
                    .no_implicit_any
                    .default_if_unknown(options.strict)
                    .is_true()
            {
                return None;
            }
            Some(diag::Could_not_find_a_declaration_file_for_module_0_1_implicitly_has_an_any_type)
        };

        let need_resolve_json_module = || -> Option<&'static Message> {
            if options.get_resolve_json_module() {
                return None;
            }
            Some(diag::Module_0_was_resolved_to_1_but_resolveJsonModule_is_not_used)
        };

        let need_allow_arbitrary_extensions = || -> Option<&'static Message> {
            if with_source_file_info(file, |info| info.is_declaration_file)
                || options.allow_arbitrary_extensions.is_true()
            {
                return None;
            }
            Some(diag::Module_0_was_resolved_to_1_but_allowArbitraryExtensions_is_not_set)
        };

        // tsgo#4712: a content mapper extension is always allowed.
        if resolved_module.resolved_using_extra_extensions {
            return None;
        }

        let ext = resolved_module.extension.as_str();
        if ext == EXTENSION_TS
            || ext == EXTENSION_DTS
            || ext == EXTENSION_MTS
            || ext == EXTENSION_DMTS
            || ext == EXTENSION_CTS
            || ext == EXTENSION_DCTS
        {
            // These are always allowed.
            None
        } else if ext == EXTENSION_TSX {
            need_jsx()
        } else if ext == EXTENSION_JSX {
            if let Some(message) = need_jsx() {
                return Some(message);
            }
            need_allow_js()
        } else if ext == EXTENSION_JS || ext == EXTENSION_MJS || ext == EXTENSION_CJS {
            need_allow_js()
        } else if ext == EXTENSION_JSON {
            need_resolve_json_module()
        } else {
            need_allow_arbitrary_extensions()
        }
    }
}

#[cfg(test)]
mod pattern_match_tests {
    use super::core_p17::pattern_matches_go_bytes;
    use crate::prelude::*;

    /// `pattern_matches_go_bytes` gives Go `Pattern.Matches`
    /// (core/pattern.go:22) on the Go bytes: `len(candidate) >= len(Text)-1`,
    /// `strings.HasPrefix` and `strings.HasSuffix`. The texts include empty
    /// affixes, affixes that overlap in a short candidate, and port forms
    /// with invalid byte units, where a byte suffix can match inside a char.
    #[test]
    fn pattern_matches_as_go_on_go_bytes() {
        let invalid = |bytes: &[u8]| go_string_from_bytes(bytes.to_vec());
        let patterns = [
            ("", ".svg"),
            ("virtual:", ""),
            ("ab", "ba"),
            ("", ""),
            ("", &invalid(&[0xA9])),
            (&invalid(&[0xC3]), ""),
            ("x", &invalid(&[0xFF, b'z'])),
        ];
        let candidates = [
            "a.svg",
            ".svg",
            "svg",
            "virtual:x",
            "virtual",
            "aba",
            "abba",
            "ab-ba",
            "",
            "é",
            "xé",
            &invalid(&[b'x', 0xFF, b'z']),
            &invalid(&[0xC3]),
        ];
        let mut got = Vec::new();
        let mut want = Vec::new();
        for (prefix, suffix) in &patterns {
            let value = PatternAmbientModule {
                pattern_prefix: prefix.to_string(),
                pattern_suffix: suffix.to_string(),
                symbol: SymbolId::NIL,
            };
            for candidate in &candidates {
                let (c, p, s) = (
                    go_string_bytes(candidate),
                    go_string_bytes(prefix),
                    go_string_bytes(suffix),
                );
                want.push(c.len() >= p.len() + s.len() && c.starts_with(&p) && c.ends_with(&s));
                got.push(pattern_matches_go_bytes(&value, &c));
            }
        }
        assert_eq!(got, want);
        // `é` (C3 A9) ends with the byte A9, as in Go.
        assert!(got[4 * candidates.len() + 9]);
        assert_eq!(got.iter().filter(|&&m| m).count(), 23);
    }
}
