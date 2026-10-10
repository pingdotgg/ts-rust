use crate::prelude::*;

impl Checker {
    // Go: checker/grammarchecks.go:1841 checkGrammarConstructorTypeParameters
    pub fn check_grammar_constructor_type_parameters(&mut self, node: Node) -> bool {
        // PORT: Go reads the `TypeParameters` *NodeList field; the Node method
        // `TypeParameterList()` returns that same list.
        let range_ = node.type_parameter_list();
        if !range_.is_nil() {
            let pos: i32;
            if range_.pos() == range_.end() {
                pos = range_.pos();
            } else {
                pos = skip_trivia(
                    &source_file_text(get_source_file_of_node(node)),
                    range_.pos(),
                );
            }
            return self.grammar_error_at_pos(
                node,
                pos,
                range_.end() - pos,
                diag::Type_parameters_cannot_appear_on_a_constructor_declaration,
                args![],
            );
        }

        false
    }

    // Go: checker/grammarchecks.go:1856 checkGrammarConstructorTypeAnnotation
    pub fn check_grammar_constructor_type_annotation(&mut self, node: Node) -> bool {
        let t = node.type_();
        if t.is_some() {
            return self.grammar_error_on_node(
                t,
                diag::Type_annotation_cannot_appear_on_a_constructor_declaration,
                args![],
            );
        }
        false
    }

    // Go: checker/grammarchecks.go:1864 checkGrammarProperty
    pub fn check_grammar_property(
        &mut self,
        node: Node, /*Union[PropertyDeclaration, PropertySignature]*/
    ) -> bool {
        let property_name = node.name();
        if is_computed_property_name(property_name)
            && is_binary_expression(property_name.expression())
            && property_name.expression().operator_token().kind() == SyntaxKind::InKeyword
        {
            return self.grammar_error_on_node(
                node.parent().members().get(0),
                diag::A_mapped_type_may_not_declare_properties_or_methods,
                args![],
            );
        }
        if is_class_like(node.parent()) {
            if is_string_literal(property_name) && property_name.text() == "constructor" {
                return self.grammar_error_on_node(
                    property_name,
                    diag::Classes_may_not_have_a_field_named_constructor,
                    args![],
                );
            }
            if self.check_grammar_for_invalid_dynamic_name(
                property_name,
                diag::A_computed_property_name_in_a_class_property_declaration_must_have_a_simple_literal_type_or_a_unique_symbol_type,
            ) {
                return true;
            }
            if is_auto_accessor_property_declaration(node)
                && self.check_grammar_for_invalid_question_mark(
                    node.postfix_token(),
                    diag::An_accessor_property_cannot_be_declared_optional,
                )
            {
                return true;
            }
        } else if is_interface_declaration(node.parent()) {
            if self.check_grammar_for_invalid_dynamic_name(
                property_name,
                diag::A_computed_property_name_in_an_interface_must_refer_to_an_expression_whose_type_is_a_literal_type_or_a_unique_symbol_type,
            ) {
                return true;
            }
            if !is_property_signature_declaration(node) {
                // Interfaces cannot contain property declarations
                panic!("Unexpected node kind {:?}", node.kind());
            }
            let initializer = node.initializer();
            if initializer.is_some() {
                return self.grammar_error_on_node(
                    initializer,
                    diag::An_interface_property_cannot_have_an_initializer,
                    args![],
                );
            }
        } else if is_type_literal_node(node.parent()) {
            if self.check_grammar_for_invalid_dynamic_name(
                node.name(),
                diag::A_computed_property_name_in_a_type_literal_must_refer_to_an_expression_whose_type_is_a_literal_type_or_a_unique_symbol_type,
            ) {
                return true;
            }
            if !is_property_signature_declaration(node) {
                // Type literals cannot contain property declarations
                panic!("Unexpected node kind {:?}", node.kind());
            }
            let initializer = node.initializer();
            if initializer.is_some() {
                return self.grammar_error_on_node(
                    initializer,
                    diag::A_type_literal_property_cannot_have_an_initializer,
                    args![],
                );
            }
        }

        if node.flags().intersects(NodeFlags::AMBIENT) {
            self.check_ambient_initializer(node);
        }

        if is_property_declaration(node) {
            let postfix_token = node.postfix_token();
            if postfix_token.is_some() && postfix_token.kind() == SyntaxKind::ExclamationToken {
                if node.initializer().is_some() {
                    return self.grammar_error_on_node(
                        postfix_token,
                        diag::Declarations_with_initializers_cannot_also_have_definite_assignment_assertions,
                        args![],
                    );
                } else if node.type_().is_nil() {
                    return self.grammar_error_on_node(
                        postfix_token,
                        diag::Declarations_with_definite_assignment_assertions_must_also_have_type_annotations,
                        args![],
                    );
                } else if !is_class_like(node.parent())
                    || node.flags().intersects(NodeFlags::AMBIENT)
                    || is_static(node)
                    || has_abstract_modifier(node)
                {
                    return self.grammar_error_on_node(
                        postfix_token,
                        diag::A_definite_assignment_assertion_is_not_permitted_in_this_context,
                        args![],
                    );
                }
            }
        }

        false
    }

    // Go: checker/grammarchecks.go:1925 checkAmbientInitializer
    pub fn check_ambient_initializer(&mut self, node: Node) -> bool {
        let initializer: Node;
        let type_node: Node;
        match node.kind() {
            SyntaxKind::VariableDeclaration => {
                initializer = node.initializer();
                type_node = node.type_();
            }
            SyntaxKind::PropertyDeclaration => {
                initializer = node.initializer();
                type_node = node.type_();
            }
            SyntaxKind::PropertySignature => {
                initializer = node.initializer();
                type_node = node.type_();
            }
            _ => panic!("Unexpected node kind {:?}", node.kind()),
        }

        if initializer.is_some() {
            let is_invalid_initializer =
                !(is_initializer_string_or_number_literal_expression(initializer)
                    || self.is_initializer_simple_literal_enum_reference(initializer)
                    || initializer.kind() == SyntaxKind::TrueKeyword
                    || initializer.kind() == SyntaxKind::FalseKeyword
                    || is_initializer_big_int_literal_expression(initializer));
            let is_const_or_readonly = is_declaration_readonly(node)
                || is_variable_declaration(node) && self.is_var_const_like(node);
            if is_const_or_readonly && type_node.is_nil() {
                if is_invalid_initializer {
                    return self.grammar_error_on_node(
                        initializer,
                        diag::A_const_initializer_in_an_ambient_context_must_be_a_string_or_numeric_literal_or_literal_enum_reference,
                        args![],
                    );
                }
            } else {
                return self.grammar_error_on_node(
                    initializer,
                    diag::Initializers_are_not_allowed_in_ambient_contexts,
                    args![],
                );
            }
        }

        false
    }
}

// Go: checker/grammarchecks.go:1960 isInitializerStringOrNumberLiteralExpression
pub fn is_initializer_string_or_number_literal_expression(expr: Node) -> bool {
    is_string_or_numeric_literal_like(expr)
        || expr.kind() == SyntaxKind::PrefixUnaryExpression
            && expr.operator() == SyntaxKind::MinusToken
            && expr.operand().kind() == SyntaxKind::NumericLiteral
}

// Go: checker/grammarchecks.go:1965 isInitializerBigIntLiteralExpression
pub fn is_initializer_big_int_literal_expression(expr: Node) -> bool {
    if expr.kind() == SyntaxKind::BigIntLiteral {
        return true;
    }

    if expr.kind() == SyntaxKind::PrefixUnaryExpression {
        return expr.operator() == SyntaxKind::MinusToken
            && expr.operand().kind() == SyntaxKind::BigIntLiteral;
    }

    false
}

impl Checker {
    // Go: checker/grammarchecks.go:1978 isInitializerSimpleLiteralEnumReference
    pub fn is_initializer_simple_literal_enum_reference(&mut self, expr: Node) -> bool {
        if is_property_access_expression(expr) {
            let t = self.check_expression_cached(expr);
            return self.ty(t).flags.intersects(TypeFlags::ENUM_LIKE);
        }

        if is_element_access_expression(expr) {
            return is_initializer_string_or_number_literal_expression(expr.argument_expression())
                && is_entity_name_expression(expr.expression())
                && {
                    let t = self.check_expression_cached(expr);
                    self.ty(t).flags.intersects(TypeFlags::ENUM_LIKE)
                };
        }

        false
    }

    // Go: checker/grammarchecks.go:1994 checkGrammarTopLevelElementForRequiredDeclareModifier
    pub fn check_grammar_top_level_element_for_required_declare_modifier(
        &mut self,
        node: Node,
    ) -> bool {
        // A declare modifier is required for any top level .d.ts declaration except export=, export default, export as namespace
        // interfaces and imports categories:
        //
        //  DeclarationElement:
        //     ExportAssignment
        //     export_opt   InterfaceDeclaration
        //     export_opt   TypeAliasDeclaration
        //     export_opt   ImportDeclaration
        //     export_opt   ExternalImportDeclaration
        //     export_opt   AmbientDeclaration
        //
        // TODO: The spec needs to be amended to reflect this grammar.
        let kind = node.kind();
        if kind == SyntaxKind::InterfaceDeclaration
            || kind == SyntaxKind::TypeAliasDeclaration
            || kind == SyntaxKind::ImportDeclaration
            || kind == SyntaxKind::JsImportDeclaration
            || kind == SyntaxKind::ImportEqualsDeclaration
            || kind == SyntaxKind::ExportDeclaration
            || kind == SyntaxKind::ExportAssignment
            || kind == SyntaxKind::NamespaceExportDeclaration
            || has_syntactic_modifier(
                node,
                ModifierFlags::AMBIENT | ModifierFlags::EXPORT | ModifierFlags::DEFAULT,
            )
        {
            return false;
        }

        self.grammar_error_on_first_token(
            node,
            diag::Top_level_declarations_in_d_ts_files_must_start_with_either_a_declare_or_export_modifier,
            args![],
        )
    }

    // Go: checker/grammarchecks.go:2014 checkGrammarTopLevelElementsForRequiredDeclareModifier
    pub fn check_grammar_top_level_elements_for_required_declare_modifier(
        &mut self,
        file: Node,
    ) -> bool {
        for decl in file.statements().iter() {
            if is_declaration_node(decl) || decl.kind() == SyntaxKind::VariableStatement {
                if self.check_grammar_top_level_element_for_required_declare_modifier(decl) {
                    return true;
                }
            }
        }
        false
    }

    // Go: checker/grammarchecks.go:2025 checkGrammarSourceFile
    pub fn check_grammar_source_file(&mut self, node: Node) -> bool {
        node.flags().intersects(NodeFlags::AMBIENT)
            && self.check_grammar_top_level_elements_for_required_declare_modifier(node)
    }

    // Go: checker/grammarchecks.go:2029 checkGrammarStatementInAmbientContext
    pub fn check_grammar_statement_in_ambient_context(&mut self, node: Node) -> bool {
        if node.flags().intersects(NodeFlags::AMBIENT) {
            // Find containing block which is either Block, ModuleBlock, SourceFile
            let has_reported = self
                .node_links
                .get(node)
                .has_reported_statement_in_ambient_context;
            if !has_reported && (is_function_like(node.parent()) || is_accessor(node.parent())) {
                let reported = self.grammar_error_on_first_token(
                    node,
                    diag::An_implementation_cannot_be_declared_in_ambient_contexts,
                    args![],
                );
                self.node_links
                    .get(node)
                    .has_reported_statement_in_ambient_context = reported;
                return reported;
            }

            // We are either parented by another statement, or some sort of block.
            // If we're in a block, we only want to really report an error once
            // to prevent noisiness.  So use a bit on the block to indicate if
            // this has already been reported, and don't report if it has.
            //
            let parent = node.parent();
            if parent.kind() == SyntaxKind::Block
                || parent.kind() == SyntaxKind::ModuleBlock
                || parent.kind() == SyntaxKind::SourceFile
            {
                // Check if the containing block ever report this error
                if !self
                    .node_links
                    .get(parent)
                    .has_reported_statement_in_ambient_context
                {
                    let reported = self.grammar_error_on_first_token(
                        node,
                        diag::Statements_are_not_allowed_in_ambient_contexts,
                        args![],
                    );
                    self.node_links
                        .get(parent)
                        .has_reported_statement_in_ambient_context = reported;
                    return reported;
                }
            } else {
                // We must be parented by a statement.  If so, there's no need
                // to report the error as our parent will have already done it.
                // debug.Assert(ast.IsStatement(node.Parent)) // !!! commented out in strada - fails if uncommented
            }
        }
        false
    }

    // Go: checker/grammarchecks.go:2059 checkGrammarNumericLiteral
    pub fn check_grammar_numeric_literal(&mut self, node: Node) {
        let node_text = get_text_of_node(node);

        // Realism (size) checking
        // We should test against `getTextOfNode(node)` rather than `node.text`, because `node.text` for large numeric literals can contain "."
        // e.g. `node.text` for numeric literal `1100000000000000000000` is `1.1e21`.
        let is_fractional = node_text.contains('.');
        let is_scientific = node.token_flags().intersects(TokenFlags::SCIENTIFIC);

        // Scientific notation (e.g. 2e54 and 1e00000000010) can't be converted to bigint
        // Fractional numbers (e.g. 9000000000000000.001) are inherently imprecise anyway
        if is_fractional || is_scientific {
            return;
        }

        // Here `node` is guaranteed to be a numeric literal representing an integer.
        // We need to judge whether the integer `node` represents is <= 2 ** 53 - 1, which can be accomplished by comparing to `value` defined below because:
        // 1) when `node` represents an integer <= 2 ** 53 - 1, `node.text` is its exact string representation and thus `value` precisely represents the integer.
        // 2) otherwise, although `node.text` may be imprecise string representation, its mathematical value and consequently `value` cannot be less than 2 ** 53,
        //    thus the result of the predicate won't be affected.
        let value = crate::jsnum::from_string(&node.text());
        if value <= crate::jsnum::MAX_SAFE_INTEGER {
            return;
        }

        self.add_error_or_suggestion(
            false,
            create_diagnostic_for_node(
                node,
                diag::Numeric_literals_with_absolute_values_equal_to_2_53_or_greater_are_too_large_to_be_represented_accurately_as_integers,
                args![],
            ),
        );
    }

    // Go: checker/grammarchecks.go:2087 checkGrammarBigIntLiteral
    pub fn check_grammar_big_int_literal(&mut self, node: Node) -> bool {
        let literal_type = is_literal_type_node(node.parent())
            || is_prefix_unary_expression(node.parent())
                && is_literal_type_node(node.parent().parent());
        if !literal_type {
            // Don't error on BigInt literals in ambient contexts
            if !node.flags().intersects(NodeFlags::AMBIENT)
                && self.language_version < ScriptTarget::ES2020
            {
                if self.grammar_error_on_node(
                    node,
                    diag::BigInt_literals_are_not_available_when_targeting_lower_than_ES2020,
                    args![],
                ) {
                    return true;
                }
            }
        }
        false
    }

    // Go: checker/grammarchecks.go:2100 checkGrammarImportClause
    pub fn check_grammar_import_clause(&mut self, node: Node) -> bool {
        match node.phase_modifier() {
            SyntaxKind::TypeKeyword => {
                if !node.flags().intersects(NodeFlags::JS_DOC)
                    && node.name().is_some()
                    && node.named_bindings().is_some()
                {
                    return self.grammar_error_on_node(
                        node,
                        diag::A_type_only_import_can_specify_a_default_import_or_named_bindings_but_not_both,
                        args![],
                    );
                }
                if node.named_bindings().is_some()
                    && node.named_bindings().kind() == SyntaxKind::NamedImports
                {
                    return self
                        .check_grammar_type_only_named_imports_or_exports(node.named_bindings());
                }
            }
            SyntaxKind::DeferKeyword => {
                if node.name().is_some() {
                    return self.grammar_error_on_node(
                        node,
                        diag::Default_imports_are_not_allowed_in_a_deferred_import,
                        args![],
                    );
                }
                // ts#64640, Go N' grammarchecks.go:2113
                if node.named_bindings().is_nil() {
                    return self.grammar_error_on_node(
                        node,
                        diag::A_deferred_import_must_specify_a_namespace_binding,
                        args![],
                    );
                }
                if node.named_bindings().kind() == SyntaxKind::NamedImports {
                    return self.grammar_error_on_node(
                        node,
                        diag::Named_imports_are_not_allowed_in_a_deferred_import,
                        args![],
                    );
                }
                // ts#63915, Go N' grammarchecks.go:2119
                if module_kind_supports_deferred_imports(self.module_kind) {
                    return false;
                }
                return self.grammar_error_on_node(
                    node,
                    diag::Deferred_imports_are_only_supported_when_the_module_flag_is_set_to_esnext_or_preserve,
                    args![],
                );
            }
            // ts#63915, Go N' grammarchecks.go:2123
            SyntaxKind::SourceKeyword => {
                if node.named_bindings().is_some() {
                    return self.grammar_error_on_node(
                        node,
                        diag::Named_and_namespace_imports_are_not_allowed_in_a_source_phase_import,
                        args![],
                    );
                }
                if node.name().is_nil() {
                    return self.grammar_error_on_node(
                        node,
                        diag::A_source_phase_import_must_specify_a_local_binding,
                        args![],
                    );
                }
                if module_kind_supports_source_phase_imports(self.module_kind) {
                    let module_specifier = get_module_specifier_from_node(node.parent());
                    if self.get_emit_syntax_for_module_specifier_expression(module_specifier)
                        == ModuleKind::COMMON_JS
                    {
                        return self.grammar_error_on_node(
                            node,
                            diag::Source_phase_imports_are_not_allowed_on_statements_that_compile_to_CommonJS_require_calls,
                            args![],
                        );
                    }
                    return false;
                }
                return self.grammar_error_on_node(
                    node,
                    diag::Source_phase_imports_are_only_supported_when_the_module_option_is_set_to_esnext_nodenext_or_preserve,
                    args![],
                );
            }
            _ => {}
        }
        false
    }

    // Go: checker/grammarchecks.go:2136 checkGrammarTypeOnlyNamedImportsOrExports
    pub fn check_grammar_type_only_named_imports_or_exports(
        &mut self,
        named_bindings: Node,
    ) -> bool {
        let node_list = named_bindings.element_list();
        for specifier in node_list.nodes().iter() {
            let specifier_is_type_only: bool;
            let message: &'static crate::diagnostics::Message;
            if specifier.kind() == SyntaxKind::ImportSpecifier {
                specifier_is_type_only = specifier.is_type_only();
                message = diag::The_type_modifier_cannot_be_used_on_a_named_import_when_import_type_is_used_on_its_import_statement;
            } else {
                specifier_is_type_only = specifier.is_type_only();
                message = diag::The_type_modifier_cannot_be_used_on_a_named_export_when_export_type_is_used_on_its_export_statement;
            }

            if specifier_is_type_only {
                return self.grammar_error_on_first_token(specifier, message, args![]);
            }
        }

        false
    }

    // Go: checker/grammarchecks.go:2157 checkGrammarImportCallExpression
    pub fn check_grammar_import_call_expression(&mut self, node: Node) -> bool {
        if self.compiler_options.verbatim_module_syntax == Tristate::True
            && self.module_kind == ModuleKind::COMMON_JS
        {
            return self.grammar_error_on_node(
                node,
                get_verbatim_module_syntax_error_message(node),
                args![],
            );
        }

        // ts#63915, Go N' grammarchecks.go:2181
        if is_import_source_meta_property(node.expression()) {
            if !module_kind_supports_source_phase_imports(self.module_kind) {
                return self.grammar_error_on_node(
                    node,
                    diag::Source_phase_imports_are_only_supported_when_the_module_option_is_set_to_esnext_nodenext_or_preserve,
                    args![],
                );
            }
        } else if is_import_defer_meta_property(node.expression()) {
            if !module_kind_supports_deferred_imports(self.module_kind) {
                return self.grammar_error_on_node(
                    node,
                    diag::Deferred_imports_are_only_supported_when_the_module_flag_is_set_to_esnext_or_preserve,
                    args![],
                );
            }
        } else if self.module_kind == ModuleKind::ES2015 {
            return self.grammar_error_on_node(
                node,
                diag::Dynamic_imports_are_only_supported_when_the_module_flag_is_set_to_es2020_es2022_esnext_commonjs_amd_system_umd_node16_node18_node20_or_nodenext,
                args![],
            );
        }

        // ts#63915, Go N' grammarchecks.go:2197
        if is_source_phase_import_call(node) && node.question_dot_token().is_some() {
            return self.grammar_error_on_node(
                node.question_dot_token(),
                diag::Optional_chaining_cannot_be_used_with_import_source,
                args![],
            );
        }

        // PORT: Go reads the `TypeArguments` and `Arguments` *NodeList fields;
        // the Node methods `TypeArgumentList()`/`ArgumentList()` return them.
        if node.type_argument_list().is_some() {
            return self.grammar_error_on_node(
                node,
                diag::This_use_of_import_is_invalid_import_calls_can_be_written_but_they_must_have_parentheses_and_cannot_have_type_arguments,
                args![],
            );
        }

        let node_arguments = node.argument_list();
        let argument_nodes = node_arguments.nodes();
        if !(ModuleKind::NODE16 <= self.module_kind && self.module_kind <= ModuleKind::NODE_NEXT)
            && self.module_kind != ModuleKind::ES_NEXT
            && self.module_kind != ModuleKind::PRESERVE
        {
            // We are allowed trailing comma after proposal-import-assertions.
            self.check_grammar_for_disallowed_trailing_comma(
                node_arguments,
                diag::Trailing_comma_not_allowed,
            );

            if argument_nodes.len() > 1 {
                let import_attributes_argument = argument_nodes.get(1);
                return self.grammar_error_on_node(
                    import_attributes_argument,
                    diag::Dynamic_imports_only_support_a_second_argument_when_the_module_option_is_set_to_esnext_node16_node18_node20_nodenext_or_preserve,
                    args![],
                );
            }
        }

        if argument_nodes.len() == 0 || argument_nodes.len() > 2 {
            return self.grammar_error_on_node(
                node,
                diag::Dynamic_imports_can_only_accept_a_module_specifier_and_an_optional_set_of_attributes_as_arguments,
                args![],
            );
        }

        // see: parseArgumentOrArrayLiteralElement...we use this function which parse arguments of callExpression to parse specifier for dynamic import.
        // parseArgumentOrArrayLiteralElement allows spread element to be in an argument list which is not allowed as specifier in dynamic import.
        let spread_element = argument_nodes
            .iter()
            .find(|n| is_spread_element(*n))
            .unwrap_or(Node::NIL);
        if spread_element.is_some() {
            return self.grammar_error_on_node(
                spread_element,
                diag::Argument_of_dynamic_import_cannot_be_spread_element,
                args![],
            );
        }
        false
    }
}

// Go: core/compileroptions.go:217 ModuleKind.SupportsDeferredImports (ts#63915)
// PORT: Go's method is in `core`; the port of `core/compileroptions.go`
// (`src/options.rs`) belongs to the program lane. This local copy is used
// until that lane adds the method.
fn module_kind_supports_deferred_imports(module_kind: ModuleKind) -> bool {
    module_kind == ModuleKind::ES_NEXT || module_kind == ModuleKind::PRESERVE
}

// Go: core/compileroptions.go:221 ModuleKind.SupportsSourcePhaseImports (ts#63915)
// PORT: see `module_kind_supports_deferred_imports`.
fn module_kind_supports_source_phase_imports(module_kind: ModuleKind) -> bool {
    module_kind == ModuleKind::ES_NEXT
        || module_kind == ModuleKind::NODE_NEXT
        || module_kind == ModuleKind::PRESERVE
}
