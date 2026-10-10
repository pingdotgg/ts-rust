//! Port of Go `checker/checker.go` lines 5715-6625 (checker-07):
//! `hasExportedMembersOfKind` through `getIterationTypesOfMethod`.
//!
//! PORT: cross-file choices this file makes (the contract does not settle them):
//! - Go stored `func` fields on `Checker` (`getGlobalIterableType`,
//!   `getGlobalDisposableType`, `resolveName`, ...) are
//!   `Rc<dyn Fn(&mut Checker, ...) -> ...>` fields and are called as
//!   `(self.field.clone())(self, ...)`.
//! - `c.syncIterationTypesResolver` / `c.asyncIterationTypesResolver` are
//!   `Rc<IterationTypesResolver>`. Functions that take a Go
//!   `*IterationTypesResolver` take `&IterationTypesResolver`.
//! - Nilable `*diagnostics.Message` parameters are `Option<&'static Message>`.
//! - Go `diagnosticOutput *[]*ast.Diagnostic` / `candidatesOutArray
//!   *[]*Signature` are `Option<&mut Vec<Diagnostic>>` /
//!   `Option<&mut Vec<SignatureId>>`.
//! - Go `c.addDeferredDiagnostic(func())` takes `Rc<dyn Fn(&mut Checker)>`.

use crate::diagnostics::Message;
use crate::prelude::*;

impl Checker {
    // Go: checker/checker.go:5903 hasExportedMembersOfKind
    pub fn has_exported_members_of_kind(
        &mut self,
        module_symbol: SymbolId,
        kind: SymbolFlags,
    ) -> bool {
        let exports = self.sym(module_symbol).exports;
        for symbol in self.symbols.values(exports) {
            if self.sym(symbol).name != INTERNAL_SYMBOL_NAME_EXPORT_EQUALS
                && self.get_symbol_flags(symbol).intersects(kind)
            {
                return true;
            }
        }
        false
    }

    // Go: checker/checker.go:5912 hasShadowedNamespace
    pub fn has_shadowed_namespace(&mut self, symbol: SymbolId) -> bool {
        let flags = self.sym(symbol).flags;
        if flags.intersects(SymbolFlags::NAMESPACE_MODULE) && flags.intersects(SymbolFlags::ALIAS) {
            let target = self.resolve_alias(symbol);
            if self.sym(target).flags.intersects(SymbolFlags::NAMESPACE)
                && self.has_exported_members_of_kind(
                    target,
                    SymbolFlags::TYPE | SymbolFlags::NAMESPACE,
                )
            {
                return true;
            }
        }
        false
    }
}

// Go: checker/checker.go:5921 isNotOverload
pub fn is_not_overload(node: Node) -> bool {
    !is_function_declaration(node) && !is_method_declaration(node) || node.body().is_some()
}

impl Checker {
    // Go: checker/checker.go:5925 checkMissingDeclaration
    pub fn check_missing_declaration(&mut self, node: Node) {
        self.check_decorators(node);
    }

    // Go: checker/checker.go:5929 checkVariableStatement
    pub fn check_variable_statement(&mut self, node: Node) {
        let declaration_list = node.declaration_list();
        if !self.check_grammar_modifiers(node)
            && !self.check_grammar_variable_declaration_list(declaration_list)
        {
            self.check_grammar_for_disallowed_block_scoped_variable_statement(node);
        }
        self.check_variable_declaration_list(declaration_list);
    }

    // Go: checker/checker.go:5938 checkVariableDeclarationList
    pub fn check_variable_declaration_list(&mut self, node: Node) {
        let block_scope_kind = get_combined_node_flags(node) & NodeFlags::BLOCK_SCOPED;
        if (block_scope_kind == NodeFlags::USING || block_scope_kind == NodeFlags::AWAIT_USING)
            && self.language_version < LANGUAGE_FEATURE_MINIMUM_TARGET.using_and_await_using
        {
            self.check_external_emit_helpers(
                node,
                ExternalEmitHelpers::ADD_DISPOSABLE_RESOURCE_AND_DISPOSE_RESOURCES,
            );
        }
        self.check_source_elements(node.declarations().nodes());
    }

    // Go: checker/checker.go:5946 checkVariableDeclaration
    pub fn check_variable_declaration(&mut self, node: Node) {
        let _trace = self.tracer.map(|tr| {
            tr.push(
                crate::tracing::Phase::Check,
                "checkVariableDeclaration",
                crate::tracing::node_args(node),
                false,
            )
        });
        self.check_grammar_variable_declaration(node);
        self.check_variable_like_declaration(node);
    }

    // Check variable, parameter, or property declaration
    // Go: checker/checker.go:5955 checkVariableLikeDeclaration
    pub fn check_variable_like_declaration(&mut self, node: Node) {
        self.check_decorators(node);
        let name = node.name();
        if name.is_nil() {
            return; // Missing array binding elements have no name
        }
        let type_node = node.type_();
        let initializer = node.initializer();
        if !is_binding_element(node) {
            self.check_source_element(type_node);
        }
        // For a computed property, just check the initializer and exit
        // Do not use hasDynamicName here, because that returns false for well known symbols.
        // We want to perform checkComputedPropertyName for all computed properties, including
        // well known symbols.
        if is_computed_property_name(name) {
            self.check_computed_property_name(name);
            if initializer.is_some() {
                self.check_expression_cached(initializer);
            }
        }
        if is_binding_element(node) {
            let prop_name = node.property_name();

            if prop_name.is_some() && is_private_identifier(prop_name) {
                self.grammar_error_on_node(
                    prop_name,
                    diag::Private_identifiers_cannot_be_used_in_destructuring_patterns,
                    args![],
                );
            }

            if prop_name.is_some()
                && is_identifier(node.name())
                && is_part_of_parameter_declaration(node)
                && node_is_missing(get_containing_function(node).body())
            {
                // type F = ({a: string}) => void;
                //               ^^^^^^
                // variable renaming in function type notation is confusing,
                // so we forbid it even if noUnusedLocals is not enabled
                self.renamed_binding_elements_in_types.push(node);
                return;
            }
            if is_object_binding_pattern(node.parent())
                && has_dot_dot_dot_token(node)
                && self.language_version < LANGUAGE_FEATURE_MINIMUM_TARGET.object_spread_rest
            {
                self.check_external_emit_helpers(node, ExternalEmitHelpers::REST);
            }
            // check computed properties inside property names of binding elements
            if prop_name.is_some() && is_computed_property_name(prop_name) {
                self.check_computed_property_name(prop_name);
            }
            // check private/protected variable access
            let parent = node.parent().parent();
            let parent_check_mode = if has_dot_dot_dot_token(node) {
                CheckMode::REST_BINDING_ELEMENT
            } else {
                CheckMode::NORMAL
            };
            let parent_type = self.get_type_for_binding_element_parent(parent, parent_check_mode);
            let prop_name_name = node.property_name_or_name();
            if parent_type.is_some() && !is_binding_pattern(prop_name_name) {
                let expr_type = self.get_literal_type_from_property_name(prop_name_name);
                if self.is_type_usable_as_property_name(expr_type) {
                    let name_text = self.get_property_name_from_type(expr_type);
                    let property = self.get_property_of_type(parent_type, &name_text);
                    if property.is_some() {
                        self.mark_property_as_referenced(
                            property,
                            Node::NIL, /*nodeForCheckWriteOnly*/
                            false,     /*isSelfTypeAccess*/
                        );
                        // A destructuring is never a write-only reference.
                        let is_super = parent.initializer().is_some()
                            && parent.initializer().kind() == SyntaxKind::SuperKeyword;
                        self.check_property_accessibility(
                            node,
                            is_super,
                            false, /*writing*/
                            parent_type,
                            property,
                        );
                    }
                }
            }
        }
        // For a binding pattern, check contained binding elements
        if is_binding_pattern(name) {
            self.check_source_elements(name.elements());
        }
        // For a parameter declaration with an initializer, error and exit if the containing function doesn't have a body
        if initializer.is_some()
            && is_part_of_parameter_declaration(node)
            && node_is_missing(get_containing_function(node).body())
        {
            self.error(node, diag::A_parameter_initializer_is_only_allowed_in_a_function_or_constructor_implementation, vec![]);
            return;
        }
        // For a binding pattern, validate the initializer and exit
        if is_binding_pattern(name) {
            if is_in_ambient_or_type_node(node) {
                return;
            }
            let need_check_initializer = initializer.is_some()
                && node.parent().parent().kind() != SyntaxKind::ForInStatement;
            let need_check_widened_type = !name.elements().iter().any(|n| n.name().is_some());
            if need_check_initializer || need_check_widened_type {
                // Don't validate for-in initializer as it is already an error
                let widened_type = self.get_widened_type_for_variable_like_declaration(
                    node, false, /*reportErrors*/
                );
                if need_check_initializer {
                    let initializer_type = self.check_expression_cached(initializer);
                    if self.strict_null_checks && need_check_widened_type {
                        self.check_non_null_non_void_type(initializer_type, node);
                    } else {
                        let target =
                            self.get_widened_type_for_variable_like_declaration(node, false);
                        self.check_type_assignable_to_and_optionally_elaborate(
                            initializer_type,
                            target,
                            node,
                            initializer,
                            None,
                            None,
                        );
                    }
                }
                // check the binding pattern with empty elements
                if need_check_widened_type {
                    if is_array_binding_pattern(name) {
                        let undefined_type = self.undefined_type;
                        self.check_iterated_type_or_element_type(
                            IterationUse::DESTRUCTURING,
                            widened_type,
                            undefined_type,
                            node,
                        );
                    } else if self.strict_null_checks {
                        self.check_non_null_non_void_type(widened_type, node);
                    }
                }
            }
            return;
        }
        // For a commonjs `const x = require`, validate the alias and exit
        let symbol = self.get_symbol_of_declaration(node);
        if self.sym(symbol).flags.intersects(SymbolFlags::ALIAS)
            && is_variable_declaration_initialized_to_require(node)
        {
            self.check_alias_symbol(node);
            return;
        }
        if is_big_int_literal(name) {
            self.error(
                name,
                diag::A_bigint_literal_cannot_be_used_as_a_property_name,
                vec![],
            );
        }
        let type_of_symbol = self.get_type_of_symbol(symbol);
        let t = self.convert_auto_to_any(type_of_symbol);
        if node == self.sym(symbol).value_declaration {
            // Node is the primary declaration of the symbol, just validate the initializer
            // Don't validate for-in initializer as it is already an error
            if initializer.is_some() && !is_for_in_statement(node.parent().parent()) {
                let initializer_type = self.check_expression_cached(initializer);
                self.check_type_assignable_to_and_optionally_elaborate(
                    initializer_type,
                    t,
                    node,
                    initializer,
                    None, /*headMessage*/
                    None,
                );
                let block_scope_kind =
                    self.get_combined_node_flags_cached(node) & NodeFlags::BLOCK_SCOPED;
                if block_scope_kind == NodeFlags::AWAIT_USING {
                    let global_async_disposable_type =
                        (self.get_global_async_disposable_type.clone())(self);
                    let global_disposable_type = (self.get_global_disposable_type.clone())(self);
                    if global_async_disposable_type != self.empty_object_type
                        && global_disposable_type != self.empty_object_type
                    {
                        let null_type = self.null_type;
                        let undefined_type = self.undefined_type;
                        let optional_disposable_type = self.get_union_type(&[
                            global_async_disposable_type,
                            global_disposable_type,
                            null_type,
                            undefined_type,
                        ]);
                        let widened = self.widen_type_for_variable_like_declaration(
                            initializer_type,
                            node,
                            false,
                        );
                        self.check_type_assignable_to(
                            widened,
                            optional_disposable_type,
                            initializer,
                            Some(diag::The_initializer_of_an_await_using_declaration_must_be_either_an_object_with_a_Symbol_asyncDispose_or_Symbol_dispose_method_or_be_null_or_undefined),
                        );
                    }
                } else if block_scope_kind == NodeFlags::USING {
                    let global_disposable_type = (self.get_global_disposable_type.clone())(self);
                    if global_disposable_type != self.empty_object_type {
                        let null_type = self.null_type;
                        let undefined_type = self.undefined_type;
                        let optional_disposable_type = self.get_union_type(&[
                            global_disposable_type,
                            null_type,
                            undefined_type,
                        ]);
                        let widened = self.widen_type_for_variable_like_declaration(
                            initializer_type,
                            node,
                            false,
                        );
                        self.check_type_assignable_to(
                            widened,
                            optional_disposable_type,
                            initializer,
                            Some(diag::The_initializer_of_a_using_declaration_must_be_either_an_object_with_a_Symbol_dispose_method_or_be_null_or_undefined),
                        );
                    }
                }
            }
            let declarations = self.sym(symbol).declarations.clone();
            if declarations.len() > 1 {
                let mut some = false;
                for &d in &declarations {
                    if d != node
                        && is_variable_like(d)
                        && !self.are_declaration_flags_identical(d, node)
                    {
                        some = true;
                        break;
                    }
                }
                if some {
                    self.error(
                        name,
                        diag::All_declarations_of_0_must_have_identical_modifiers,
                        args![declaration_name_to_string(name)],
                    );
                }
            }
        } else {
            // Node is a secondary declaration, check that type is identical to primary declaration and check that
            // initializer is consistent with type associated with the node
            let widened = self.get_widened_type_for_variable_like_declaration(node, false);
            let declaration_type = self.convert_auto_to_any(widened);
            if !self.is_error_type(t)
                && !self.is_error_type(declaration_type)
                && !self.is_type_identical_to(t, declaration_type)
                && !self.sym(symbol).flags.intersects(SymbolFlags::ASSIGNMENT)
            {
                let value_declaration = self.sym(symbol).value_declaration;
                self.error_next_variable_or_property_declaration_must_have_same_type(
                    value_declaration,
                    t,
                    node,
                    declaration_type,
                );
            }
            if initializer.is_some() {
                let initializer_type = self.check_expression_cached(initializer);
                self.check_type_assignable_to_and_optionally_elaborate(
                    initializer_type,
                    declaration_type,
                    node,
                    initializer,
                    None, /*headMessage*/
                    None,
                );
            }
            let value_declaration = self.sym(symbol).value_declaration;
            if value_declaration.is_some()
                && !self.are_declaration_flags_identical(node, value_declaration)
            {
                self.error(
                    name,
                    diag::All_declarations_of_0_must_have_identical_modifiers,
                    args![declaration_name_to_string(name)],
                );
            }
        }
        if !is_property_declaration(node) && !is_property_signature_declaration(node) {
            // We know we don't have a binding pattern or computed name here
            self.check_exports_on_merged_declarations(node);
            if is_variable_declaration(node) || is_binding_element(node) {
                self.check_var_declared_names_not_shadowed(node);
            }
            self.check_collisions_for_declaration_name(node, node.name());
        }
    }

    // Go: checker/checker.go:6119 errorNextVariableOrPropertyDeclarationMustHaveSameType
    pub fn error_next_variable_or_property_declaration_must_have_same_type(
        &mut self,
        first_declaration: Node,
        first_type: TypeId,
        next_declaration: Node,
        next_type: TypeId,
    ) {
        let next_declaration_name = get_name_of_declaration(next_declaration);
        let message = if is_property_declaration(next_declaration)
            || is_property_signature_declaration(next_declaration)
        {
            diag::Subsequent_property_declarations_must_have_the_same_type_Property_0_must_be_of_type_1_but_here_has_type_2
        } else {
            diag::Subsequent_variable_declarations_must_have_the_same_type_Variable_0_must_be_of_type_1_but_here_has_type_2
        };
        let decl_name = declaration_name_to_string(next_declaration_name);
        let first_type_string = self.type_to_string(first_type);
        let next_type_string = self.type_to_string(next_type);
        // PORT: Go calls `c.error` (which adds the diagnostic) and then mutates
        // the returned `*ast.Diagnostic` with `AddRelatedInfo`. Diagnostics are
        // owned values here, so the related info is attached first and the
        // diagnostic is added afterwards. `c.error` is exactly
        // `NewDiagnosticForNode` + `addDiagnostic`, so the result is the same.
        let mut err = new_diagnostic_for_node(
            next_declaration_name,
            message,
            args![decl_name, first_type_string, next_type_string],
        );
        if first_declaration.is_some() {
            err.add_related_info(Some(create_diagnostic_for_node(
                first_declaration,
                diag::X_0_was_also_declared_here,
                args![decl_name],
            )));
        }
        self.add_diagnostic(err);
    }

    // Go: checker/checker.go:6131 checkVarDeclaredNamesNotShadowed
    pub fn check_var_declared_names_not_shadowed(&mut self, node: Node) {
        // - ScriptBody : StatementList
        // It is a Syntax Error if any element of the LexicallyDeclaredNames of StatementList
        // also occurs in the VarDeclaredNames of StatementList.

        // - Block : { StatementList }
        // It is a Syntax Error if any element of the LexicallyDeclaredNames of StatementList
        // also occurs in the VarDeclaredNames of StatementList.

        // Variable declarations are hoisted to the top of their function scope. They can shadow
        // block scoped declarations, which bind tighter. this will not be flagged as duplicate definition
        // by the binder as the declaration scope is different.
        // A non-initialized declaration is a no-op as the block declaration will resolve before the var
        // declaration. the problem is if the declaration has an initializer. this will act as a write to the
        // block declared value. this is fine for let, but not const.
        // Only consider declarations with initializers, uninitialized const declarations will not
        // step on a let/const variable.
        // Do not consider const and const declarations, as duplicate block-scoped declarations
        // are handled by the binder.
        // We are only looking for const declarations that step on let\const declarations from a
        // different scope. e.g.:
        //      {
        //          const x = 0; // localDeclarationSymbol obtained after name resolution will correspond to this declaration
        //          const x = 0; // symbol for this declaration will be 'symbol'
        //      }

        // skip block-scoped variables and parameters
        if self
            .get_combined_node_flags_cached(node)
            .intersects(NodeFlags::BLOCK_SCOPED)
            || is_part_of_parameter_declaration(node)
        {
            return;
        }
        // NOTE: in ES6 spec initializer is required in variable declarations where name is binding pattern
        // so we'll always treat binding elements as initialized
        let symbol = self.get_symbol_of_declaration(node);
        let name = node.name();
        if self
            .sym(symbol)
            .flags
            .intersects(SymbolFlags::FUNCTION_SCOPED_VARIABLE)
        {
            if !is_identifier(name) {
                panic!("Identifier expected");
            }
            let local_declaration_symbol = (self.resolve_name.clone())(
                self,
                node,
                name.text(),
                SymbolFlags::VARIABLE,
                None,  /*nameNotFoundMessage*/
                false, /*isUse*/
                false,
            );
            if local_declaration_symbol.is_some()
                && local_declaration_symbol != symbol
                && self
                    .sym(local_declaration_symbol)
                    .flags
                    .intersects(SymbolFlags::BLOCK_SCOPED_VARIABLE)
            {
                if self
                    .get_declaration_node_flags_from_symbol(local_declaration_symbol)
                    .intersects(NodeFlags::BLOCK_SCOPED)
                {
                    let var_decl_list = find_ancestor_kind(
                        self.sym(local_declaration_symbol).value_declaration,
                        SyntaxKind::VariableDeclarationList,
                    );
                    let mut container = Node::NIL;
                    if is_variable_statement(var_decl_list.parent())
                        && var_decl_list.parent().parent().is_some()
                    {
                        container = var_decl_list.parent().parent();
                    }
                    // names of block-scoped and function scoped variables can collide only
                    // if block scoped variable is defined in the function\module\source file scope (because of variable hoisting)
                    let names_share_scope = container.is_some()
                        && (is_block(container) && is_function_like(container.parent())
                            || is_module_block(container)
                            || is_module_declaration(container)
                            || is_source_file(container));
                    // here we know that function scoped variable is "shadowed" by block scoped one
                    // a var declaration can't hoist past a lexical declaration and it results in a SyntaxError at runtime
                    if !names_share_scope {
                        let name = self.symbol_to_string(local_declaration_symbol);
                        self.error(
                            node,
                            diag::Cannot_initialize_outer_scoped_variable_0_in_the_same_scope_as_block_scoped_declaration_1,
                            args![name.clone(), name],
                        );
                    }
                }
            }
        }
    }

    // Go: checker/checker.go:6192 checkDecorators
    pub fn check_decorators(&mut self, node: Node) {
        // skip this check for nodes that cannot have decorators. These should have already had an error reported by
        // checkGrammarModifiers.
        if !can_have_decorators(node)
            || !has_decorators(node)
            || !node_can_be_decorated(
                self.legacy_decorators,
                node,
                node.parent(),
                node.parent().parent(),
            )
        {
            return;
        }
        let first_decorator = node
            .modifier_nodes()
            .iter()
            .find(|&m| is_decorator(m))
            .unwrap_or(Node::NIL);
        if first_decorator.is_nil() {
            return;
        }
        if self.legacy_decorators {
            self.check_external_emit_helpers(first_decorator, ExternalEmitHelpers::DECORATE);
            if is_parameter_declaration(node) {
                self.check_external_emit_helpers(first_decorator, ExternalEmitHelpers::PARAM);
            }
        } else if self.language_version
            < LANGUAGE_FEATURE_MINIMUM_TARGET.class_and_class_element_decorators
        {
            self.check_external_emit_helpers(
                first_decorator,
                ExternalEmitHelpers::ES_DECORATE_AND_RUN_INITIALIZERS,
            );
            if is_class_declaration(node) {
                if node.name().is_nil()
                    || self
                        .get_first_transformable_static_class_element(node)
                        .is_some()
                {
                    self.check_external_emit_helpers(
                        first_decorator,
                        ExternalEmitHelpers::SET_FUNCTION_NAME,
                    );
                }
            } else if !is_class_expression(node) {
                let name = node.name();
                if is_private_identifier(name)
                    && (is_method_declaration(node)
                        || is_accessor(node)
                        || is_auto_accessor_property_declaration(node))
                {
                    self.check_external_emit_helpers(
                        first_decorator,
                        ExternalEmitHelpers::SET_FUNCTION_NAME,
                    );
                }
                if is_computed_property_name(name) {
                    self.check_external_emit_helpers(
                        first_decorator,
                        ExternalEmitHelpers::PROP_KEY,
                    );
                }
            }
        }
        self.mark_linked_references(node, ReferenceHint::DECORATOR, SymbolId::NIL, TypeId::NIL);
        for modifier in node.modifier_nodes() {
            if is_decorator(modifier) {
                self.check_decorator(modifier);
            }
        }
    }

    // Go: checker/checker.go:6231 checkDecorator
    pub fn check_decorator(&mut self, node: Node) {
        self.check_grammar_decorator(node);
        let signature = self.get_resolved_signature(node, None, CheckMode::NORMAL);
        self.check_deprecated_signature(signature, node);
        let return_type = self.get_return_type_of_signature(signature);
        if self.ty(return_type).flags.intersects(TypeFlags::ANY) {
            return;
        }
        // if we fail to get a signature and return type here, we will have already reported a grammar error in `checkDecorators`.
        let decorator_signature = self.get_decorator_call_signature(node);
        if decorator_signature.is_nil()
            || self.sig(decorator_signature).resolved_return_type.is_nil()
        {
            return;
        }
        let expected_return_type = self.sig(decorator_signature).resolved_return_type;
        let head_message: &'static Message = match node.parent().kind() {
            SyntaxKind::ClassDeclaration | SyntaxKind::ClassExpression => {
                diag::Decorator_function_return_type_0_is_not_assignable_to_type_1
            }
            SyntaxKind::PropertyDeclaration if !self.legacy_decorators => {
                diag::Decorator_function_return_type_0_is_not_assignable_to_type_1
            }
            // Go: `case ast.KindPropertyDeclaration: ... fallthrough` into `case ast.KindParameter`.
            SyntaxKind::PropertyDeclaration | SyntaxKind::Parameter => {
                diag::Decorator_function_return_type_is_0_but_is_expected_to_be_void_or_any
            }
            SyntaxKind::MethodDeclaration | SyntaxKind::GetAccessor | SyntaxKind::SetAccessor => {
                diag::Decorator_function_return_type_0_is_not_assignable_to_type_1
            }
            _ => panic!("Unhandled case in checkDecorator"),
        };
        self.check_type_assignable_to(
            return_type,
            expected_return_type,
            node.expression(),
            Some(head_message),
        );
    }

    // Go: checker/checker.go:6265 checkIteratedTypeOrElementType
    pub fn check_iterated_type_or_element_type(
        &mut self,
        use_: IterationUse,
        input_type: TypeId,
        sent_type: TypeId,
        error_node: Node,
    ) -> TypeId {
        if self.is_type_any(input_type) {
            return input_type;
        }
        let t = self.get_iterated_type_or_element_type(
            use_, input_type, sent_type, error_node, true, /*checkAssignability*/
        );
        if t.is_some() {
            return t;
        }
        self.any_type
    }

    // Go: checker/checker.go:6276 getIteratedTypeOrElementType
    pub fn get_iterated_type_or_element_type(
        &mut self,
        use_: IterationUse,
        input_type: TypeId,
        sent_type: TypeId,
        error_node: Node,
        check_assignability: bool,
    ) -> TypeId {
        let allow_async_iterables = use_.intersects(IterationUse::ALLOWS_ASYNC_ITERABLES_FLAG);
        if input_type == self.never_type {
            if error_node.is_some() {
                self.report_type_not_iterable_error(error_node, input_type, allow_async_iterables);
            }
            return TypeId::NIL;
        }
        let iterable_exists =
            (self.get_global_iterable_type.clone())(self) != self.empty_generic_type;
        let possible_out_of_bounds = self.compiler_options.no_unchecked_indexed_access
            == Tristate::True
            && use_.intersects(IterationUse::POSSIBLY_OUT_OF_BOUNDS);
        if iterable_exists || allow_async_iterables {
            let iteration_types = self.get_iteration_types_of_iterable(
                input_type,
                use_,
                if iterable_exists {
                    error_node
                } else {
                    Node::NIL
                },
            );
            if check_assignability {
                if iteration_types.next_type.is_some() {
                    let mut diagnostic: Option<&'static Message> = None;
                    if use_.intersects(IterationUse::FOR_OF_FLAG) {
                        diagnostic = Some(diag::Cannot_iterate_value_because_the_next_method_of_its_iterator_expects_type_1_but_for_of_will_always_send_0);
                    } else if use_.intersects(IterationUse::SPREAD_FLAG) {
                        diagnostic = Some(diag::Cannot_iterate_value_because_the_next_method_of_its_iterator_expects_type_1_but_array_spread_will_always_send_0);
                    } else if use_.intersects(IterationUse::DESTRUCTURING_FLAG) {
                        diagnostic = Some(diag::Cannot_iterate_value_because_the_next_method_of_its_iterator_expects_type_1_but_array_destructuring_will_always_send_0);
                    } else if use_.intersects(IterationUse::YIELD_STAR_FLAG) {
                        diagnostic = Some(diag::Cannot_delegate_iteration_to_value_because_the_next_method_of_its_iterator_expects_type_1_but_the_containing_generator_will_always_send_0);
                    }
                    if diagnostic.is_some() {
                        self.check_type_assignable_to(
                            sent_type,
                            iteration_types.next_type,
                            error_node,
                            diagnostic,
                        );
                    }
                }
            }
            if iteration_types.yield_type.is_some() || iterable_exists {
                if iteration_types.yield_type.is_nil() {
                    return TypeId::NIL;
                }
                if possible_out_of_bounds {
                    return self.include_undefined_in_index_signature(iteration_types.yield_type);
                }
                return iteration_types.yield_type;
            }
        }
        let mut array_type = input_type;
        let mut has_string_constituent = false;
        // If strings are permitted, remove any string-like constituents from the array type.
        // This allows us to find other non-string element types from an array unioned with
        // a string.
        if use_.intersects(IterationUse::ALLOWS_STRING_INPUT_FLAG) {
            if self.ty(array_type).flags.intersects(TypeFlags::UNION) {
                // After we remove all types that are StringLike, we will know if there was a string constituent
                // based on whether the result of filter is a new array.
                let array_types = self.ty(input_type).types_list();
                let filtered_types: Vec<TypeId> = array_types
                    .iter()
                    .copied()
                    .filter(|&t| !self.ty(t).flags.intersects(TypeFlags::STRING_LIKE))
                    .collect();
                // PORT: Go `core.Same(filteredTypes, arrayTypes)` is true exactly when
                // `core.Filter` kept every element (it then returns the input slice).
                if filtered_types.len() != array_types.len() {
                    array_type = self.get_union_type_ex(
                        &filtered_types,
                        UnionReduction::SUBTYPE,
                        None,
                        TypeId::NIL,
                    );
                }
            } else if self.ty(array_type).flags.intersects(TypeFlags::STRING_LIKE) {
                array_type = self.never_type;
            }
            has_string_constituent = array_type != input_type;
            if has_string_constituent {
                // Now that we've removed all the StringLike types, if no constituents remain, then the entire
                // arrayOrStringType was a string.
                if self.ty(array_type).flags.intersects(TypeFlags::NEVER) {
                    if possible_out_of_bounds {
                        let string_type = self.string_type;
                        return self.include_undefined_in_index_signature(string_type);
                    }
                    return self.string_type;
                }
            }
        }
        if !self.is_array_like_type(array_type) {
            if error_node.is_some() {
                // Which error we report depends on whether we allow strings or if there was a
                // string constituent. For example, if the input type is number | string, we
                // want to say that number is not an array type. But if the input was just
                // number and string input is allowed, we want to say that number is not an
                // array type or a string type.
                let allows_strings = use_.intersects(IterationUse::ALLOWS_STRING_INPUT_FLAG)
                    && !has_string_constituent;
                let (default_diagnostic, maybe_missing_await) =
                    self.get_iteration_diagnostic_details(use_, input_type, allows_strings);
                let maybe_missing_await =
                    maybe_missing_await && self.get_awaited_type_of_promise(array_type).is_some();
                let array_type_string = self.type_to_string(array_type);
                self.error_and_maybe_suggest_await(
                    error_node,
                    maybe_missing_await,
                    default_diagnostic,
                    args![array_type_string],
                );
            }
            if has_string_constituent {
                if possible_out_of_bounds {
                    let string_type = self.string_type;
                    return self.include_undefined_in_index_signature(string_type);
                }
                return self.string_type;
            }
            return TypeId::NIL;
        }
        let number_type = self.number_type;
        let array_element_type = self.get_index_type_of_type(array_type, number_type);
        if has_string_constituent && array_element_type.is_some() {
            // This is just an optimization for the case where arrayOrStringType is string | string[]
            if self
                .ty(array_element_type)
                .flags
                .intersects(TypeFlags::STRING_LIKE)
                && self.compiler_options.no_unchecked_indexed_access != Tristate::True
            {
                return self.string_type;
            }
            let string_type = self.string_type;
            if possible_out_of_bounds {
                let undefined_type = self.undefined_type;
                return self.get_union_type_ex(
                    &[array_element_type, string_type, undefined_type],
                    UnionReduction::SUBTYPE,
                    None,
                    TypeId::NIL,
                );
            }
            return self.get_union_type_ex(
                &[array_element_type, string_type],
                UnionReduction::SUBTYPE,
                None,
                TypeId::NIL,
            );
        }
        if use_.intersects(IterationUse::POSSIBLY_OUT_OF_BOUNDS) {
            return self.include_undefined_in_index_signature(array_element_type);
        }
        array_element_type
    }

    // Gets the requested "iteration type" from a type that is either `Iterable`-like, `Iterator`-like,
    // `IterableIterator`-like, or `Generator`-like (for a non-async generator); or `AsyncIterable`-like,
    // `AsyncIterator`-like, `AsyncIterableIterator`-like, or `AsyncGenerator`-like (for an async generator).
    // Go: checker/checker.go:6386 getIterationTypeOfGeneratorFunctionReturnType
    pub fn get_iteration_type_of_generator_function_return_type(
        &mut self,
        type_kind: IterationTypeKind,
        return_type: TypeId,
        is_async_generator: bool,
    ) -> TypeId {
        if self.is_type_any(return_type) {
            return TypeId::NIL;
        }
        let iteration_types = self
            .get_iteration_types_of_generator_function_return_type(return_type, is_async_generator);
        iteration_types.get_type(type_kind)
    }

    // Go: checker/checker.go:6394 getIterationTypesOfGeneratorFunctionReturnType
    pub fn get_iteration_types_of_generator_function_return_type(
        &mut self,
        t: TypeId,
        is_async_generator: bool,
    ) -> IterationTypes {
        if self.is_type_any(t) {
            return IterationTypes {
                yield_type: self.any_type,
                return_type: self.any_type,
                next_type: self.any_type,
            };
        }
        let use_ = if is_async_generator {
            IterationUse::ASYNC_GENERATOR_RETURN_TYPE
        } else {
            IterationUse::GENERATOR_RETURN_TYPE
        };
        let resolver = if is_async_generator {
            self.async_iteration_types_resolver.clone()
        } else {
            self.sync_iteration_types_resolver.clone()
        };
        let result = self.get_iteration_types_of_iterable(t, use_, Node::NIL /*errorNode*/);
        if result.has_types() {
            return result;
        }
        self.get_iteration_types_of_iterator(
            t,
            &resolver,
            Node::NIL, /*errorNode*/
            None,      /*diagnosticOutput*/
        )
    }

    // Gets the requested "iteration type" from an `Iterable`-like or `AsyncIterable`-like type.
    // Go: checker/checker.go:6408 getIterationTypeOfIterable
    pub fn get_iteration_type_of_iterable(
        &mut self,
        use_: IterationUse,
        type_kind: IterationTypeKind,
        input_type: TypeId,
        error_node: Node,
    ) -> TypeId {
        if self.is_type_any(input_type) {
            return TypeId::NIL;
        }
        let iteration_types = self.get_iteration_types_of_iterable(input_type, use_, error_node);
        iteration_types.get_type(type_kind)
    }

    // Gets the *yield*, *return*, and *next* types from an `Iterable`-like or `AsyncIterable`-like type.
    //
    // At every level that involves analyzing return types of signatures, we union the return types of all the signatures.
    //
    // Another thing to note is that at any step of this process, we could run into a dead end,
    // meaning either the property is missing, or we run into the anyType. If either of these things
    // happens, we return a default `IterationTypes{}` to signal that we could not find the iteration type.
    // If a property is missing, and the previous step did not result in `any`, then we also give an error
    // if the caller requested it. Then the caller can decide what to do in the case where there is no
    // iterated type.
    //
    // For a **for-of** statement, `yield*` (in a normal generator), spread, array
    // destructuring, or normal generator we will only ever look for a `[Symbol.iterator]()`
    // method.
    //
    // For an async generator we will only ever look at the `[Symbol.asyncIterator]()` method.
    //
    // For a **for-await-of** statement or a `yield*` in an async generator we will look for
    // the `[Symbol.asyncIterator]()` method first, and then the `[Symbol.iterator]()` method.
    // Go: checker/checker.go:6435 getIterationTypesOfIterable
    pub fn get_iteration_types_of_iterable(
        &mut self,
        t: TypeId,
        use_: IterationUse,
        error_node: Node,
    ) -> IterationTypes {
        let t = self.get_reduced_type(t);
        if self.is_type_any(t) {
            return IterationTypes {
                yield_type: self.any_type,
                return_type: self.any_type,
                next_type: self.any_type,
            };
        }
        let key = IterationTypesKey {
            type_id: self.ty(t).id,
            use_: use_ & IterationUse::CACHE_FLAGS,
        };
        // If we are reporting errors and encounter a cached `noIterationTypes`, we should ignore the cached value and continue as if nothing was cached.
        // In addition, we should not cache any new results for this call.
        let mut no_cache = false;
        if let Some(cached) = self.iteration_types_cache.get(&key).cloned() {
            if error_node.is_nil() || cached.has_types() {
                return cached;
            }
            no_cache = true;
        }
        let result = self.get_iteration_types_of_iterable_worker(t, use_, error_node, no_cache);
        if !no_cache {
            self.iteration_types_cache.insert(key, result.clone());
        }
        result
    }

    // Go: checker/checker.go:6457 getIterationTypesOfIterableWorker
    pub fn get_iteration_types_of_iterable_worker(
        &mut self,
        t: TypeId,
        use_: IterationUse,
        error_node: Node,
        no_cache: bool,
    ) -> IterationTypes {
        if self.ty(t).flags.intersects(TypeFlags::UNION) {
            let types = self.ty(t).types_list();
            let mut all_iteration_types: Vec<IterationTypes> = Vec::with_capacity(types.len());
            for constituent in types {
                let iteration_types = self.get_iteration_types_of_iterable_worker(
                    constituent,
                    use_,
                    Node::NIL,
                    no_cache,
                );
                if !iteration_types.has_types() {
                    if error_node.is_some() {
                        self.add_deferred_diagnostic(Rc::new(move |c: &mut Checker| {
                            c.report_type_not_iterable_error(
                                error_node,
                                t,
                                use_.intersects(IterationUse::ALLOWS_ASYNC_ITERABLES_FLAG),
                            );
                        }));
                    }
                    return IterationTypes::default();
                }
                all_iteration_types.push(iteration_types);
            }
            return self.combine_iteration_types(&all_iteration_types);
        }
        let mut diags: Vec<Diagnostic> = Vec::new();
        if use_.intersects(IterationUse::ALLOWS_ASYNC_ITERABLES_FLAG) {
            let resolver = self.async_iteration_types_resolver.clone();
            let iteration_types = self.get_iteration_types_of_iterable_fast(t, &resolver);
            if iteration_types.has_types() {
                if use_.intersects(IterationUse::FOR_OF_FLAG) {
                    return self.get_async_from_sync_iteration_types(iteration_types, error_node);
                }
                return iteration_types;
            }
            let iteration_types = self.get_iteration_types_of_iterable_slow(
                t,
                &resolver,
                error_node,
                Some(&mut diags),
            );
            if iteration_types.has_types() {
                if !diags.is_empty() {
                    for d in diags {
                        self.add_diagnostic(d);
                    }
                }
                return iteration_types;
            }
        }
        if use_.intersects(IterationUse::ALLOWS_SYNC_ITERABLES_FLAG) {
            let resolver = self.sync_iteration_types_resolver.clone();
            let iteration_types = self.get_iteration_types_of_iterable_fast(t, &resolver);
            if iteration_types.has_types() {
                if use_.intersects(IterationUse::ALLOWS_ASYNC_ITERABLES_FLAG) {
                    return self.get_async_from_sync_iteration_types(iteration_types, error_node);
                }
                return iteration_types;
            }
            let iteration_types = self.get_iteration_types_of_iterable_slow(
                t,
                &resolver,
                error_node,
                Some(&mut diags),
            );
            if iteration_types.has_types() {
                if !diags.is_empty() {
                    for d in diags {
                        self.add_diagnostic(d);
                    }
                }
                if use_.intersects(IterationUse::ALLOWS_ASYNC_ITERABLES_FLAG) {
                    return self.get_async_from_sync_iteration_types(iteration_types, error_node);
                }
                return iteration_types;
            }
        }
        if error_node.is_some() {
            // We defer the diagnostic because TypeToString may attempt to resolve symbols that are already being
            // resolved, possibly causing circularities.
            self.add_deferred_diagnostic(Rc::new(move |c: &mut Checker| {
                let diagnostic = c.report_type_not_iterable_error(
                    error_node,
                    t,
                    use_.intersects(IterationUse::ALLOWS_ASYNC_ITERABLES_FLAG),
                );
                c.add_related_info_to_reported_diagnostic(&diagnostic, diags.clone());
            }));
        }
        IterationTypes::default()
    }

    // PORT: Go appends related info to the `*ast.Diagnostic` that
    // `reportTypeNotIterableError` returned: the stored one (#4825: it can be an
    // equal one added before). `reported` is a clone of it, so `lookup` finds
    // the stored diagnostic (or an identical one). If the diagnostic was
    // discarded (`addDiagnostic` drops it at the maximum serialization level,
    // which is the same here as when it was added), Go changes a diagnostic that
    // is not stored, so nothing is done.
    fn add_related_info_to_reported_diagnostic(
        &mut self,
        reported: &Diagnostic,
        related: Vec<Diagnostic>,
    ) {
        if self.serialization_level >= MAX_SERIALIZATION_LEVEL {
            return;
        }
        if let Some(stored) = self.diagnostics.lookup(reported) {
            for d in related {
                stored.add_related_info(Some(d));
            }
        }
    }

    // Go: checker/checker.go:6527 getIterationTypesOfIterableFast
    pub fn get_iteration_types_of_iterable_fast(
        &mut self,
        t: TypeId,
        r: &IterationTypesResolver,
    ) -> IterationTypes {
        // As an optimization, if the type is an instantiation of the following global type, then
        // just grab its related type arguments:
        // - `Iterable<T, TReturn, TNext>` or `AsyncIterable<T, TReturn, TNext>`
        // - `IteratorObject<T, TReturn, TNext>` or `AsyncIteratorObject<T, TReturn, TNext>`
        // - `IterableIterator<T, TReturn, TNext>` or `AsyncIterableIterator<T, TReturn, TNext>`
        // - `Generator<T, TReturn, TNext>` or `AsyncGenerator<T, TReturn, TNext>`
        let global_iterable_type = (r.get_global_iterable_type)(self);
        let mut matches = self.is_reference_to_type(t, global_iterable_type);
        if !matches {
            let global_iterator_object_type = (r.get_global_iterator_object_type)(self);
            matches = self.is_reference_to_type(t, global_iterator_object_type);
        }
        if !matches {
            let global_iterable_iterator_type = (r.get_global_iterable_iterator_type)(self);
            matches = self.is_reference_to_type(t, global_iterable_iterator_type);
        }
        if !matches {
            let global_generator_type = (r.get_global_generator_type)(self);
            matches = self.is_reference_to_type(t, global_generator_type);
        }
        if matches {
            let type_arguments = self.get_type_arguments(t);
            let (yield_type, return_type, next_type) =
                (type_arguments[0], type_arguments[1], type_arguments[2]);
            return r.get_resolved_iteration_types(self, yield_type, return_type, next_type);
        }
        // As an optimization, if the type is an instantiation of one of the following global types, then
        // just grab the related type argument:
        // - `ArrayIterator<T>`
        // - `MapIterator<T>`
        // - `SetIterator<T>`
        // - `StringIterator<T>`
        // - `ReadableStreamAsyncIterator<T>`
        let builtin_iterator_types = (r.get_global_builtin_iterator_types)(self);
        if self.is_reference_to_some_type(t, &builtin_iterator_types) {
            let yield_type = self.get_type_arguments(t)[0];
            let return_type = self.get_builtin_iterator_return_type();
            let unknown_type = self.unknown_type;
            return r.get_resolved_iteration_types(self, yield_type, return_type, unknown_type);
        }
        IterationTypes::default()
    }
}

impl IterationTypesResolver {
    // Go: checker/checker.go:6554 IterationTypesResolver.getResolvedIterationTypes
    pub fn get_resolved_iteration_types(
        &self,
        c: &mut Checker,
        yield_type: TypeId,
        return_type: TypeId,
        next_type: TypeId,
    ) -> IterationTypes {
        let resolved_yield =
            (self.resolve_iteration_type)(c, yield_type, Node::NIL /*errorNode*/);
        let resolved_return =
            (self.resolve_iteration_type)(c, return_type, Node::NIL /*errorNode*/);
        IterationTypes {
            yield_type: if resolved_yield.is_some() {
                resolved_yield
            } else {
                yield_type
            },
            return_type: if resolved_return.is_some() {
                resolved_return
            } else {
                return_type
            },
            next_type,
        }
    }
}

impl Checker {
    // Go: checker/checker.go:6562 isReferenceToType
    pub fn is_reference_to_type(&self, t: TypeId, target: TypeId) -> bool {
        t.is_some()
            && self.ty(t).object_flags.intersects(ObjectFlags::REFERENCE)
            && self.ty(t).target() == target
    }

    // Go: checker/checker.go:6566 isReferenceToSomeType
    pub fn is_reference_to_some_type(&self, t: TypeId, targets: &[TypeId]) -> bool {
        t.is_some()
            && self.ty(t).object_flags.intersects(ObjectFlags::REFERENCE)
            && targets.contains(&self.ty(t).target())
    }

    // Go: checker/checker.go:6570 getBuiltinIteratorReturnType
    pub fn get_builtin_iterator_return_type(&self) -> TypeId {
        if self.strict_builtin_iterator_return {
            self.undefined_type
        } else {
            self.any_type
        }
    }
}

impl IterationTypes {
    // Go: checker/checker.go:6574 IterationTypes.hasTypes
    pub fn has_types(&self) -> bool {
        self.yield_type.is_some() || self.return_type.is_some() || self.next_type.is_some()
    }

    // Go: checker/checker.go:6578 IterationTypes.getType
    pub fn get_type(&self, type_kind: IterationTypeKind) -> TypeId {
        match type_kind {
            IterationTypeKind::YIELD => self.yield_type,
            IterationTypeKind::RETURN => self.return_type,
            IterationTypeKind::NEXT => self.next_type,
            _ => panic!("Unhandled case in getType(IterationTypeKind)"),
        }
    }
}

impl Checker {
    // Go: checker/checker.go:6590 combineIterationTypes
    pub fn combine_iteration_types(
        &mut self,
        iteration_types: &[IterationTypes],
    ) -> IterationTypes {
        IterationTypes {
            yield_type: self.get_iteration_type_union(iteration_types, |t| t.yield_type),
            return_type: self.get_iteration_type_union(iteration_types, |t| t.return_type),
            next_type: self.get_iteration_type_union(iteration_types, |t| t.next_type),
        }
    }

    // Go: checker/checker.go:6598 getIterationTypeUnion
    pub fn get_iteration_type_union(
        &mut self,
        iteration_types: &[IterationTypes],
        f: impl Fn(&IterationTypes) -> TypeId,
    ) -> TypeId {
        let types: Vec<TypeId> = iteration_types
            .iter()
            .map(|t| f(t))
            .filter(|t| t.is_some())
            .collect();
        if types.is_empty() {
            return TypeId::NIL;
        }
        self.get_union_type(&types)
    }

    // Go: checker/checker.go:6606 getAsyncFromSyncIterationTypes
    pub fn get_async_from_sync_iteration_types(
        &mut self,
        iteration_types: IterationTypes,
        error_node: Node,
    ) -> IterationTypes {
        if !iteration_types.has_types()
            || iteration_types.yield_type == self.any_type
                && iteration_types.return_type == self.any_type
                && iteration_types.next_type == self.any_type
        {
            return iteration_types;
        }
        // if we're requesting diagnostics, report errors for a missing `Awaited<T>`.
        if error_node.is_some() {
            (self.get_global_awaited_symbol.clone())(self);
        }
        let awaited_yield =
            self.get_awaited_type_ex(iteration_types.yield_type, error_node, None, args![]);
        let awaited_return =
            self.get_awaited_type_ex(iteration_types.return_type, error_node, None, args![]);
        IterationTypes {
            yield_type: if awaited_yield.is_some() {
                awaited_yield
            } else {
                self.any_type
            },
            return_type: if awaited_return.is_some() {
                awaited_return
            } else {
                self.any_type
            },
            next_type: iteration_types.next_type,
        }
    }

    // Gets the *yield*, *return*, and *next* types of an `Iterable`-like or `AsyncIterable`-like
    // type from its members.
    //
    // If we successfully found the *yield*, *return*, and *next* types, an `IterationTypes` with non-nil
    // members is returned. Otherwise, a default `IterationTypes{}` is returned.
    //
    // NOTE: You probably don't want to call this directly and should be calling
    // `getIterationTypesOfIterable` instead.
    // Go: checker/checker.go:6630 getIterationTypesOfIterableSlow
    pub fn get_iteration_types_of_iterable_slow(
        &mut self,
        t: TypeId,
        r: &IterationTypesResolver,
        error_node: Node,
        diagnostic_output: Option<&mut Vec<Diagnostic>>,
    ) -> IterationTypes {
        let property_name = self.get_property_name_for_known_symbol_name(&r.iterator_symbol_name);
        let method = self.get_property_of_type(t, &property_name);
        if method.is_some() && !self.sym(method).flags.intersects(SymbolFlags::OPTIONAL) {
            let method_type = self.get_type_of_symbol(method);
            if self.is_type_any(method_type) {
                return IterationTypes {
                    yield_type: self.any_type,
                    return_type: self.any_type,
                    next_type: self.any_type,
                };
            }
            let all_signatures = self
                .get_signatures_of_type(method_type, SignatureKind::CALL)
                .to_vec();
            let valid_signatures: Vec<SignatureId> = all_signatures
                .iter()
                .copied()
                .filter(|&sig| self.get_min_argument_count(sig) == 0)
                .collect();
            if !valid_signatures.is_empty() {
                let return_types: Vec<TypeId> = valid_signatures
                    .iter()
                    .map(|&sig| self.get_return_type_of_signature(sig))
                    .collect();
                let iterator_type = self.get_intersection_type(&return_types);
                return self.get_iteration_types_of_iterator_worker(
                    iterator_type,
                    r,
                    error_node,
                    diagnostic_output,
                );
            }
            if error_node.is_some() && !all_signatures.is_empty() {
                let global_iterable_type_checked = (r.get_global_iterable_type_checked)(self);
                self.check_type_assignable_to_ex(
                    t,
                    global_iterable_type_checked,
                    error_node,
                    None,
                    diagnostic_output,
                );
            }
        }
        IterationTypes::default()
    }

    // Gets the *yield*, *return*, and *next* types from an `Iterator`-like or `AsyncIterator`-like type.
    //
    // If we successfully found the *yield*, *return*, and *next* types, an `IterationTypes` with non-nil
    // members is returned. Otherwise, a default `IterationTypes{}` is returned.
    // Go: checker/checker.go:6655 getIterationTypesOfIterator
    pub fn get_iteration_types_of_iterator(
        &mut self,
        t: TypeId,
        r: &IterationTypesResolver,
        error_node: Node,
        diagnostic_output: Option<&mut Vec<Diagnostic>>,
    ) -> IterationTypes {
        self.get_iteration_types_of_iterator_worker(t, r, error_node, diagnostic_output)
    }

    // Gets the *yield*, *return*, and *next* types from an `Iterator`-like or `AsyncIterator`-like type.
    //
    // If we successfully found the *yield*, *return*, and *next* types, an `IterationTypes` with non-nil
    // members is returned. Otherwise, a default `IterationTypes{}` is returned.
    //
    // NOTE: You probably don't want to call this directly and should be calling `getIterationTypesOfIterator` instead.
    // Go: checker/checker.go:6665 getIterationTypesOfIteratorWorker
    pub fn get_iteration_types_of_iterator_worker(
        &mut self,
        t: TypeId,
        r: &IterationTypesResolver,
        error_node: Node,
        diagnostic_output: Option<&mut Vec<Diagnostic>>,
    ) -> IterationTypes {
        if self.is_type_any(t) {
            return IterationTypes {
                yield_type: self.any_type,
                return_type: self.any_type,
                next_type: self.any_type,
            };
        }
        let iteration_types = self.get_iteration_types_of_iterator_fast(t, r);
        if iteration_types.has_types() {
            return iteration_types;
        }
        self.get_iteration_types_of_iterator_slow(t, r, error_node, diagnostic_output)
    }

    // Go: checker/checker.go:6676 getIterationTypesOfIteratorFast
    pub fn get_iteration_types_of_iterator_fast(
        &mut self,
        t: TypeId,
        r: &IterationTypesResolver,
    ) -> IterationTypes {
        // As an optimization, if the type is an instantiation of the following global type, then
        // just grab its related type arguments:
        // - `Iterable<T, TReturn, TNext>` or `AsyncIterable<T, TReturn, TNext>`
        // - `IteratorObject<T, TReturn, TNext>` or `AsyncIteratorObject<T, TReturn, TNext>`
        // - `IterableIterator<T, TReturn, TNext>` or `AsyncIterableIterator<T, TReturn, TNext>`
        // - `Generator<T, TReturn, TNext>` or `AsyncGenerator<T, TReturn, TNext>`
        let global_iterator_type = (r.get_global_iterator_type)(self);
        let mut matches = self.is_reference_to_type(t, global_iterator_type);
        if !matches {
            let global_iterator_object_type = (r.get_global_iterator_object_type)(self);
            matches = self.is_reference_to_type(t, global_iterator_object_type);
        }
        if !matches {
            let global_iterable_iterator_type = (r.get_global_iterable_iterator_type)(self);
            matches = self.is_reference_to_type(t, global_iterable_iterator_type);
        }
        if !matches {
            let global_generator_type = (r.get_global_generator_type)(self);
            matches = self.is_reference_to_type(t, global_generator_type);
        }
        if matches {
            let type_arguments = self.get_type_arguments(t);
            let (yield_type, return_type, next_type) =
                (type_arguments[0], type_arguments[1], type_arguments[2]);
            return r.get_resolved_iteration_types(self, yield_type, return_type, next_type);
        }
        // As an optimization, if the type is an instantiation of one of the following global types, then
        // just grab the related type argument:
        // - `ArrayIterator<T>`
        // - `MapIterator<T>`
        // - `SetIterator<T>`
        // - `StringIterator<T>`
        // - `ReadableStreamAsyncIterator<T>`
        let builtin_iterator_types = (r.get_global_builtin_iterator_types)(self);
        if self.is_reference_to_some_type(t, &builtin_iterator_types) {
            let yield_type = self.get_type_arguments(t)[0];
            let return_type = self.get_builtin_iterator_return_type();
            let unknown_type = self.unknown_type;
            return r.get_resolved_iteration_types(self, yield_type, return_type, unknown_type);
        }
        IterationTypes::default()
    }

    // Go: checker/checker.go:6703 getIterationTypesOfIteratorSlow
    pub fn get_iteration_types_of_iterator_slow(
        &mut self,
        t: TypeId,
        r: &IterationTypesResolver,
        error_node: Node,
        mut diagnostic_output: Option<&mut Vec<Diagnostic>>,
    ) -> IterationTypes {
        let next = self.get_iteration_types_of_method(
            t,
            r,
            "next",
            error_node,
            diagnostic_output.as_deref_mut(),
        );
        let return_ = self.get_iteration_types_of_method(
            t,
            r,
            "return",
            error_node,
            diagnostic_output.as_deref_mut(),
        );
        let throw = self.get_iteration_types_of_method(
            t,
            r,
            "throw",
            error_node,
            diagnostic_output.as_deref_mut(),
        );
        self.combine_iteration_types(&[next, return_, throw])
    }

    // Go: checker/checker.go:6711 getIterationTypesOfMethod
    pub fn get_iteration_types_of_method(
        &mut self,
        t: TypeId,
        resolver: &IterationTypesResolver,
        method_name: &str,
        error_node: Node,
        mut diagnostic_output: Option<&mut Vec<Diagnostic>>,
    ) -> IterationTypes {
        let method = self.get_property_of_type(t, method_name);
        // Ignore 'return' or 'throw' if they are missing.
        if method.is_nil() && method_name != "next" {
            return IterationTypes::default();
        }
        let mut method_type = TypeId::NIL;
        if method.is_some()
            && !(method_name == "next" && self.sym(method).flags.intersects(SymbolFlags::OPTIONAL))
        {
            if method_name == "next" {
                method_type = self.get_type_of_symbol(method);
            } else {
                let method_symbol_type = self.get_type_of_symbol(method);
                method_type =
                    self.get_type_with_facts(method_symbol_type, TypeFacts::NE_UNDEFINED_OR_NULL);
            }
        }
        if self.is_type_any(method_type) {
            return IterationTypes {
                yield_type: self.any_type,
                return_type: self.any_type,
                next_type: self.any_type,
            };
        }
        // Both async and non-async iterators *must* have a `next` method.
        let mut method_signatures: Vec<SignatureId> = Vec::new();
        if method_type.is_some() {
            method_signatures = self
                .get_signatures_of_type(method_type, SignatureKind::CALL)
                .to_vec();
        }
        if method_signatures.is_empty() {
            if error_node.is_some() {
                let diagnostic = if method_name == "next" {
                    resolver.must_have_a_next_method_diagnostic
                } else {
                    resolver.must_be_a_method_diagnostic
                };
                self.report_diagnostic(
                    new_diagnostic_for_node(error_node, diagnostic, args![method_name]),
                    diagnostic_output,
                );
            }
            return IterationTypes::default();
        }
        // If the method signature comes exclusively from the global iterator or generator type,
        // create iteration types from its type arguments like `getIterationTypesOfIteratorFast`
        // does (so as to remove `undefined` from the next and return types). We arrive here when
        // a contextual type for a generator was not a direct reference to one of those global types,
        // but looking up `methodType` referred to one of them (and nothing else). E.g., in
        // `interface SpecialIterator extends Iterator<number> {}`, `SpecialIterator` is not a
        // reference to `Iterator`, but its `next` member derives exclusively from `Iterator`.
        if method_signatures.len() == 1 && self.ty(method_type).symbol.is_some() {
            let global_generator_type = (resolver.get_global_generator_type)(self);
            let global_iterator_type = (resolver.get_global_iterator_type)(self);
            let method_type_symbol = self.ty(method_type).symbol;
            let global_generator_symbol = self.ty(global_generator_type).symbol;
            let is_generator_method = global_generator_symbol.is_some()
                && self
                    .symbols
                    .get(self.sym(global_generator_symbol).members, method_name)
                    == method_type_symbol;
            let global_iterator_symbol = self.ty(global_iterator_type).symbol;
            let is_iterator_method = !is_generator_method
                && global_iterator_symbol.is_some()
                && self
                    .symbols
                    .get(self.sym(global_iterator_symbol).members, method_name)
                    == method_type_symbol;
            if is_generator_method || is_iterator_method {
                let global_type = if is_generator_method {
                    global_generator_type
                } else {
                    global_iterator_type
                };
                let type_parameters = self
                    .ty(global_type)
                    .as_interface_type()
                    .type_parameters()
                    .to_vec();
                let mapper = self.ty(method_type).mapper();
                let mut next_type = TypeId::NIL;
                if method_name == "next" {
                    next_type = self.get_mapped_type(type_parameters[2], mapper);
                }
                let yield_type = self.get_mapped_type(type_parameters[0], mapper);
                let return_type = self.get_mapped_type(type_parameters[1], mapper);
                return IterationTypes {
                    yield_type,
                    return_type,
                    next_type,
                };
            }
        }
        // Extract the first parameter and return type of each signature.
        let mut method_parameter_types: Vec<TypeId> = Vec::new();
        let mut method_return_types: Vec<TypeId> = Vec::new();
        for &signature in &method_signatures {
            if method_name != "throw" && !self.sig(signature).parameters.is_empty() {
                let parameter_type = self.get_type_at_position(signature, 0);
                method_parameter_types.push(parameter_type);
            }
            let return_type = self.get_return_type_of_signature(signature);
            method_return_types.push(return_type);
        }
        // Resolve the *next* or *return* type from the first parameter of a `next()` or
        // `return()` method, respectively.
        let mut return_types: Vec<TypeId> = Vec::new();
        let mut next_type = TypeId::NIL;
        if method_name != "throw" {
            // PORT: Go `methodParameterTypes != nil` is true exactly when at least one
            // element was appended.
            let method_parameter_type = if !method_parameter_types.is_empty() {
                self.get_union_type(&method_parameter_types)
            } else {
                self.unknown_type
            };
            if method_name == "next" {
                // The value of `next(value)` is *not* awaited by async generators
                next_type = method_parameter_type;
            } else if method_name == "return" {
                // The value of `return(value)` *is* awaited by async generators
                let resolved =
                    (resolver.resolve_iteration_type)(self, method_parameter_type, error_node);
                let resolved_method_parameter_type = if resolved.is_some() {
                    resolved
                } else {
                    self.any_type
                };
                return_types.push(resolved_method_parameter_type);
            }
        }
        // Resolve the *yield* and *return* types from the return type of the method (i.e. `IteratorResult`)
        let yield_type;
        // PORT: Go `methodReturnTypes != nil` is true exactly when at least one
        // element was appended.
        let method_return_type = if !method_return_types.is_empty() {
            self.get_intersection_type(&method_return_types)
        } else {
            self.never_type
        };
        let resolved = (resolver.resolve_iteration_type)(self, method_return_type, error_node);
        let resolved_method_return_type = if resolved.is_some() {
            resolved
        } else {
            self.any_type
        };
        let iteration_types =
            self.get_iteration_types_of_iterator_result(resolved_method_return_type);
        if !iteration_types.has_types() {
            if error_node.is_some() {
                self.report_diagnostic(
                    new_diagnostic_for_node(
                        error_node,
                        resolver.must_have_a_value_diagnostic,
                        args![method_name],
                    ),
                    diagnostic_output.as_deref_mut(),
                );
            }
            yield_type = self.any_type;
            return_types.push(self.any_type);
        } else {
            yield_type = iteration_types.yield_type;
            return_types.push(iteration_types.return_type);
        }
        let union_return_type = self.get_union_type(&return_types);
        IterationTypes {
            yield_type,
            return_type: union_return_type,
            next_type,
        }
    }
}
