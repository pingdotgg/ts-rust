//! Go `checker/grammarchecks.go` lines 1-951.

use crate::diagnostics::Message;
use crate::prelude::*;

impl Checker {
    // Go: checker/grammarchecks.go:19 grammarErrorOnFirstToken
    pub fn grammar_error_on_first_token(
        &mut self,
        node: Node,
        message: &'static Message,
        args: Vec<String>,
    ) -> bool {
        let source_file = get_source_file_of_node(node);
        if !self.has_parse_diagnostics(source_file) {
            let span = get_range_of_token_at_position(source_file, node.pos());
            self.add_diagnostic(new_diagnostic(source_file, span, message, args));
            return true;
        }
        false
    }

    // Go: checker/grammarchecks.go:29 grammarErrorAtPos
    pub fn grammar_error_at_pos(
        &mut self,
        node_for_source_file: Node,
        start: i32,
        length: i32,
        message: &'static Message,
        args: Vec<String>,
    ) -> bool {
        let source_file = get_source_file_of_node(node_for_source_file);
        if !self.has_parse_diagnostics(source_file) {
            self.add_diagnostic(new_diagnostic(
                source_file,
                TextRange::new(start, start + length),
                message,
                args,
            ));
            return true;
        }
        false
    }

    // Go: checker/grammarchecks.go:38 grammarErrorOnNode
    pub fn grammar_error_on_node(
        &mut self,
        node: Node,
        message: &'static Message,
        args: Vec<String>,
    ) -> bool {
        let source_file = get_source_file_of_node(node);
        if !self.has_parse_diagnostics(source_file) {
            self.error(node, message, args);
            return true;
        }
        false
    }

    // Go: checker/grammarchecks.go:47 grammarErrorOnNodeSkippedOnNoEmit
    pub fn grammar_error_on_node_skipped_on_no_emit(
        &mut self,
        node: Node,
        message: &'static Message,
        args: Vec<String>,
    ) -> bool {
        let source_file = get_source_file_of_node(node);
        if !self.has_parse_diagnostics(source_file) {
            let mut d = new_diagnostic_for_node(node, message, args);
            d.set_skipped_on_no_emit();
            self.add_diagnostic(d);
            return true;
        }
        false
    }
}

// Go: checker/grammarchecks.go:58 getIdentifierFromEntityNameExpression
pub fn get_identifier_from_entity_name_expression(node: Node) -> Node {
    match node.kind() {
        SyntaxKind::Identifier => node,
        SyntaxKind::PropertyAccessExpression => node.name(),
        _ => Node::NIL,
    }
}

impl Checker {
    // Go: checker/grammarchecks.go:69 checkGrammarRegularExpressionLiteral
    // PORT: Go caches the scanner in `c.regExpScanner`; that field is not part
    // of the Rust `Checker` (see checker_p01), so a fresh scanner is used here.
    // The scanner is reset to the same state either way. The Go error callback
    // adds diagnostics while scanning. Rust callbacks are `'static` and cannot
    // reach the checker, so the callback records each report in order and the
    // same `lastError` logic runs on the recorded reports after the scan. No
    // other diagnostic is added during the scan, so the order is the same.
    // Since #4825 Go `lastError` is the stored diagnostic that `addDiagnostic`
    // returns, and a spelling suggestion goes on that stored diagnostic.
    pub fn check_grammar_regular_expression_literal(&mut self, node: Node) -> bool {
        let source_file = get_source_file_of_node(node);
        if !self.has_parse_diagnostics(source_file) {
            let reports: Rc<RefCell<Vec<(&'static Message, i32, i32, Vec<String>)>>> =
                Rc::new(RefCell::new(Vec::new()));
            let text = source_file_text(source_file);
            let mut reg_exp_scanner = crate::frontend::scanner::new_scanner();
            reg_exp_scanner.set_script_target(self.language_version);
            reg_exp_scanner.set_language_variant(with_source_file_info(source_file, |info| {
                info.language_variant
            }));
            let sink = reports.clone();
            reg_exp_scanner.set_on_error(Some(Box::new(
                move |message: &'static Message, start: i32, length: i32, args: Vec<String>| {
                    sink.borrow_mut().push((message, start, length, args));
                },
            )));
            reg_exp_scanner.set_text(&text);
            reg_exp_scanner.reset_token_state(node.pos());
            reg_exp_scanner.scan();
            let token_is_regular_expression_literal =
                reg_exp_scanner.re_scan_slash_token(true) == SyntaxKind::RegularExpressionLiteral;
            reg_exp_scanner.set_text("");
            reg_exp_scanner.set_on_error(None);
            debug_assert!(token_is_regular_expression_literal);

            // Replay of the Go `SetOnError` callback. `last_error` is Go
            // `lastError`: its position, its length, and the stored diagnostic
            // (`None` when `addDiagnostic` discarded it).
            let mut last_error: Option<(i32, i32, Option<&mut Diagnostic>)> = None;
            for (message, start, length, args) in reports.take() {
                let matches_last = last_error
                    .as_ref()
                    .is_some_and(|&(pos, len, _)| start == pos && length == len);
                if message.category() == crate::diagnostics::Category::Message && matches_last {
                    // For providing spelling suggestions.
                    let err = new_diagnostic(
                        Node::NIL,
                        TextRange::new(start, start + length),
                        message,
                        args,
                    );
                    if let Some((_, _, Some(stored))) = &mut last_error {
                        stored.add_related_info(Some(err));
                    }
                } else if last_error.as_ref().is_none_or(|&(pos, _, _)| start != pos) {
                    let diagnostic = new_diagnostic(
                        source_file,
                        TextRange::new(start, start + length),
                        message,
                        args,
                    );
                    // Go (#4825): `lastError = c.addDiagnostic(lastError)`.
                    last_error = Some((start, length, self.add_diagnostic(diagnostic)));
                }
            }
            return last_error.is_some();
        }
        false
    }

    // Go: checker/grammarchecks.go:100 checkGrammarPrivateIdentifierExpression
    pub fn check_grammar_private_identifier_expression(&mut self, priv_id: Node) -> bool {
        let priv_id_as_node = priv_id;
        if get_containing_class(priv_id).is_nil() {
            return self.grammar_error_on_node(
                priv_id,
                diag::Private_identifiers_are_not_allowed_outside_class_bodies,
                args![],
            );
        }

        if !is_for_in_statement(priv_id.parent()) {
            if !is_expression_node(priv_id_as_node) {
                return self.grammar_error_on_node(
                    priv_id_as_node,
                    diag::Private_identifiers_are_only_allowed_in_class_bodies_and_may_only_be_used_as_part_of_a_class_member_declaration_property_access_or_on_the_left_hand_side_of_an_in_expression,
                    args![],
                );
            }

            let is_in_operation = is_binary_expression(priv_id.parent())
                && priv_id.parent().operator_token().kind() == SyntaxKind::InKeyword;
            if self
                .get_symbol_for_private_identifier_expression(priv_id_as_node)
                .is_nil()
                && !is_in_operation
            {
                return self.grammar_error_on_node(
                    priv_id_as_node,
                    diag::Cannot_find_name_0,
                    args![priv_id.text()],
                );
            }
        }

        false
    }

    // Go: checker/grammarchecks.go:120 checkGrammarMappedType
    pub fn check_grammar_mapped_type(&mut self, node: Node) -> bool {
        // PORT: Go reads the `Members` field (a `*NodeList`); the Go
        // `Members()` method returns the same nodes for a mapped type.
        let members = node.members();
        if members.len() > 0 {
            return self.grammar_error_on_node(
                members.get(0),
                diag::A_mapped_type_may_not_declare_properties_or_methods,
                args![],
            );
        }
        false
    }

    // Go: checker/grammarchecks.go:127 checkGrammarDecorator
    pub fn check_grammar_decorator(&mut self, decorator: Node) -> bool {
        let source_file = get_source_file_of_node(decorator);
        if !self.has_parse_diagnostics(source_file) {
            let mut node = decorator.expression();

            // DecoratorParenthesizedExpression :
            //   `(` Expression `)`

            if is_parenthesized_expression(node) {
                return false;
            }

            let mut can_have_call_expression = true;
            let mut error_node = Node::NIL;
            loop {
                // Allow TS syntax such as non-null assertions and instantiation expressions
                if is_expression_with_type_arguments(node) || is_non_null_expression(node) {
                    node = node.expression();
                    continue;
                }

                // DecoratorCallExpression :
                //   DecoratorMemberExpression Arguments

                if is_call_expression(node) {
                    if !can_have_call_expression {
                        error_node = node;
                    }
                    if node.question_dot_token().is_some() {
                        // Even if we already have an error node, error at the `?.` token since it appears earlier.
                        error_node = node.question_dot_token();
                    }
                    node = node.expression();
                    can_have_call_expression = false;
                    continue;
                }

                // DecoratorMemberExpression :
                //   IdentifierReference
                //   DecoratorMemberExpression `.` IdentifierName
                //   DecoratorMemberExpression `.` PrivateIdentifier

                if is_property_access_expression(node) {
                    if node.question_dot_token().is_some() {
                        // Even if we already have an error node, error at the `?.` token since it appears earlier.
                        error_node = node.question_dot_token();
                    }
                    node = node.expression();
                    can_have_call_expression = false;
                    continue;
                }

                if !is_identifier(node) {
                    // Even if we already have an error node, error at this node since it appears earlier.
                    error_node = node;
                }

                break;
            }

            if error_node.is_some() {
                // PORT: Go `c.error(...)` then mutates the stored diagnostic.
                // Here the related info is attached before the add, which
                // gives the same stored diagnostic.
                let mut err = new_diagnostic_for_node(
                    decorator.expression(),
                    diag::Expression_must_be_enclosed_in_parentheses_to_be_used_as_a_decorator,
                    args![],
                );
                err.add_related_info(Some(create_diagnostic_for_node(
                    error_node,
                    diag::Invalid_syntax_in_decorator,
                    args![],
                )));
                self.add_diagnostic(err);
                return true;
            }
        }

        false
    }

    // Go: checker/grammarchecks.go:199 checkGrammarExportDeclaration
    pub fn check_grammar_export_declaration(&mut self, node: Node) -> bool {
        // PORT: Go reads the `IsTypeOnly` field; the Go `IsTypeOnly()` method
        // returns it for an export declaration.
        let export_clause = node.export_clause();
        if node.is_type_only()
            && export_clause.is_some()
            && export_clause.kind() == SyntaxKind::NamedExports
        {
            return self.check_grammar_type_only_named_imports_or_exports(export_clause);
        }
        false
    }

    // Go: checker/grammarchecks.go:206 checkGrammarModuleElementContext
    pub fn check_grammar_module_element_context(
        &mut self,
        node: Node,
        error_message: &'static Message,
    ) -> bool {
        let parent_kind = node.parent().kind();
        let is_in_appropriate_context = parent_kind == SyntaxKind::SourceFile
            || parent_kind == SyntaxKind::ModuleBlock
            || parent_kind == SyntaxKind::ModuleDeclaration;
        if !is_in_appropriate_context {
            self.grammar_error_on_first_token(node, error_message, args![]);
        }
        !is_in_appropriate_context
    }

    // Go: checker/grammarchecks.go:214 checkGrammarModifiers
    pub fn check_grammar_modifiers(
        &mut self,
        node: Node, /*Union[HasModifiers, HasDecorators, HasIllegalModifiers, HasIllegalDecorators]*/
    ) -> bool {
        let modifier_list = node.modifiers();
        if modifier_list.is_nil() {
            return false;
        }
        // PERF: chkport1 item 4. Go reads `node.Modifiers()`, `node.Kind`
        // and `node.Parent` as plain fields. Here each one is an AST store
        // read, so they are read once and passed on.
        let modifiers = modifier_list.nodes();
        let node_kind = node.kind();
        let parent = node.parent();
        if self.report_obvious_decorator_errors(node, modifiers)
            || self.report_obvious_modifier_errors(node, node_kind, parent, modifiers)
        {
            return true;
        }
        if is_this_parameter(node) {
            return self.grammar_error_on_first_token(
                node,
                diag::Neither_decorators_nor_modifiers_may_be_applied_to_this_parameters,
                args![],
            );
        }
        let mut block_scope_kind = NodeFlags::NONE;
        if is_variable_statement(node) {
            block_scope_kind = node.declaration_list().flags() & NodeFlags::BLOCK_SCOPED;
        }
        let mut last_static = Node::NIL;
        let mut last_declare = Node::NIL;
        let mut last_async = Node::NIL;
        let mut last_override = Node::NIL;
        let mut first_decorator = Node::NIL;
        let mut flags = ModifierFlags::NONE;
        let mut saw_export_before_decorators = false;
        // We parse decorators and modifiers in four contiguous chunks:
        // [...leadingDecorators, ...leadingModifiers, ...trailingDecorators, ...trailingModifiers]. It is an error to
        // have both leading and trailing decorators.
        let mut has_leading_decorators = false;
        for modifier in modifiers {
            if is_decorator(modifier) {
                if !node_can_be_decorated(self.legacy_decorators, node, parent, parent.parent()) {
                    if node_kind == SyntaxKind::MethodDeclaration && !node_is_present(node.body()) {
                        return self.grammar_error_on_first_token(
                            node,
                            diag::A_decorator_can_only_decorate_a_method_implementation_not_an_overload,
                            args![],
                        );
                    } else {
                        return self.grammar_error_on_first_token(
                            node,
                            diag::Decorators_are_not_valid_here,
                            args![],
                        );
                    }
                } else if self.legacy_decorators
                    && (node_kind == SyntaxKind::GetAccessor
                        || node_kind == SyntaxKind::SetAccessor)
                {
                    let symbol = self.get_symbol_of_declaration(node);
                    let declarations = self.sym(symbol).declarations.clone();
                    let accessors =
                        get_all_accessor_declarations_for_declaration(node, &declarations);
                    if has_decorators(accessors.first_accessor) && node == accessors.second_accessor
                    {
                        return self.grammar_error_on_first_token(
                            node,
                            diag::Decorators_cannot_be_applied_to_multiple_get_Slashset_accessors_of_the_same_name,
                            args![],
                        );
                    }
                }

                // if we've seen any modifiers aside from `export`, `default`, or another decorator, then this is an invalid position
                if !flags
                    .without(ModifierFlags::EXPORT_DEFAULT | ModifierFlags::DECORATOR)
                    .is_empty()
                {
                    return self.grammar_error_on_node(
                        modifier,
                        diag::Decorators_are_not_valid_here,
                        args![],
                    );
                }

                // if we've already seen leading decorators and leading modifiers, then trailing decorators are an invalid position
                if has_leading_decorators && flags.intersects(ModifierFlags::MODIFIER) {
                    if first_decorator.is_nil() {
                        panic!("Expected firstDecorator to be set");
                    }
                    let source_file = get_source_file_of_node(modifier);
                    if !self.has_parse_diagnostics(source_file) {
                        // PORT: Go `c.error(...)` then mutates the stored
                        // diagnostic; the related info is attached before the add.
                        let mut err = new_diagnostic_for_node(
                            modifier,
                            diag::Decorators_may_not_appear_after_export_or_export_default_if_they_also_appear_before_export,
                            args![],
                        );
                        err.add_related_info(Some(create_diagnostic_for_node(
                            first_decorator,
                            diag::Decorator_used_before_export_here,
                            args![],
                        )));
                        self.add_diagnostic(err);
                        return true;
                    }
                    return false;
                }

                flags |= ModifierFlags::DECORATOR;

                // if we have not yet seen a modifier, then these are leading decorators
                if !flags.intersects(ModifierFlags::MODIFIER) {
                    has_leading_decorators = true;
                } else if flags.intersects(ModifierFlags::EXPORT) {
                    saw_export_before_decorators = true;
                }

                if first_decorator.is_nil() {
                    first_decorator = modifier;
                }
            } else {
                let modifier_kind = modifier.kind();
                let not_reparsed = !modifier.flags().intersects(NodeFlags::REPARSED);
                if modifier_kind != SyntaxKind::ReadonlyKeyword {
                    if node_kind == SyntaxKind::PropertySignature
                        || node_kind == SyntaxKind::MethodSignature
                    {
                        return self.grammar_error_on_node(
                            modifier,
                            diag::X_0_modifier_cannot_appear_on_a_type_member,
                            args![token_to_string(modifier_kind)],
                        );
                    }
                    if node_kind == SyntaxKind::IndexSignature
                        && (modifier_kind != SyntaxKind::StaticKeyword || !is_class_like(parent))
                    {
                        return self.grammar_error_on_node(
                            modifier,
                            diag::X_0_modifier_cannot_appear_on_an_index_signature,
                            args![token_to_string(modifier_kind)],
                        );
                    }
                }
                if modifier_kind != SyntaxKind::InKeyword
                    && modifier_kind != SyntaxKind::OutKeyword
                    && modifier_kind != SyntaxKind::ConstKeyword
                {
                    if node_kind == SyntaxKind::TypeParameter {
                        return self.grammar_error_on_node(
                            modifier,
                            diag::X_0_modifier_cannot_appear_on_a_type_parameter,
                            args![token_to_string(modifier_kind)],
                        );
                    }
                }
                match modifier_kind {
                    SyntaxKind::ConstKeyword => {
                        if node_kind != SyntaxKind::EnumDeclaration
                            && node_kind != SyntaxKind::TypeParameter
                        {
                            return self.grammar_error_on_node(
                                node,
                                diag::A_class_member_cannot_have_the_0_keyword,
                                args![token_to_string(SyntaxKind::ConstKeyword)],
                            );
                        }
                        if node_kind == SyntaxKind::TypeParameter {
                            if !(is_function_like_declaration(parent)
                                || is_class_like(parent)
                                || is_function_type_node(parent)
                                || is_constructor_type_node(parent)
                                || is_call_signature_declaration(parent)
                                || is_construct_signature_declaration(parent)
                                || is_method_signature_declaration(parent))
                            {
                                return self.grammar_error_on_node(
                                    modifier,
                                    diag::X_0_modifier_can_only_appear_on_a_type_parameter_of_a_function_method_or_class,
                                    args![token_to_string(modifier_kind)],
                                );
                            }
                        }
                    }
                    SyntaxKind::OverrideKeyword => {
                        // If node.kind === SyntaxKind.Parameter, checkParameter reports an error if it's not a parameter property.
                        if flags.intersects(ModifierFlags::OVERRIDE) {
                            return self.grammar_error_on_node(
                                modifier,
                                diag::X_0_modifier_already_seen,
                                args!["override"],
                            );
                        } else if flags.intersects(ModifierFlags::AMBIENT) {
                            return self.grammar_error_on_node(
                                modifier,
                                diag::X_0_modifier_cannot_be_used_with_1_modifier,
                                args!["override", "declare"],
                            );
                        } else if flags.intersects(ModifierFlags::READONLY) && not_reparsed {
                            return self.grammar_error_on_node(
                                modifier,
                                diag::X_0_modifier_must_precede_1_modifier,
                                args!["override", "readonly"],
                            );
                        } else if flags.intersects(ModifierFlags::ACCESSOR) && not_reparsed {
                            return self.grammar_error_on_node(
                                modifier,
                                diag::X_0_modifier_must_precede_1_modifier,
                                args!["override", "accessor"],
                            );
                        } else if flags.intersects(ModifierFlags::ASYNC) && not_reparsed {
                            return self.grammar_error_on_node(
                                modifier,
                                diag::X_0_modifier_must_precede_1_modifier,
                                args!["override", "async"],
                            );
                        }
                        flags |= ModifierFlags::OVERRIDE;
                        last_override = modifier;
                    }

                    SyntaxKind::PublicKeyword
                    | SyntaxKind::ProtectedKeyword
                    | SyntaxKind::PrivateKeyword => {
                        let text = visibility_to_string(modifier_to_flag(modifier_kind));

                        if flags.intersects(ModifierFlags::ACCESSIBILITY_MODIFIER) {
                            return self.grammar_error_on_node(
                                modifier,
                                diag::Accessibility_modifier_already_seen,
                                args![],
                            );
                        } else if flags.intersects(ModifierFlags::OVERRIDE) && not_reparsed {
                            return self.grammar_error_on_node(
                                modifier,
                                diag::X_0_modifier_must_precede_1_modifier,
                                args![text, "override"],
                            );
                        } else if flags.intersects(ModifierFlags::STATIC) && not_reparsed {
                            return self.grammar_error_on_node(
                                modifier,
                                diag::X_0_modifier_must_precede_1_modifier,
                                args![text, "static"],
                            );
                        } else if flags.intersects(ModifierFlags::ACCESSOR) && not_reparsed {
                            return self.grammar_error_on_node(
                                modifier,
                                diag::X_0_modifier_must_precede_1_modifier,
                                args![text, "accessor"],
                            );
                        } else if flags.intersects(ModifierFlags::READONLY) && not_reparsed {
                            return self.grammar_error_on_node(
                                modifier,
                                diag::X_0_modifier_must_precede_1_modifier,
                                args![text, "readonly"],
                            );
                        } else if flags.intersects(ModifierFlags::ASYNC) && not_reparsed {
                            return self.grammar_error_on_node(
                                modifier,
                                diag::X_0_modifier_must_precede_1_modifier,
                                args![text, "async"],
                            );
                        } else if parent.kind() == SyntaxKind::ModuleBlock
                            || parent.kind() == SyntaxKind::SourceFile
                        {
                            return self.grammar_error_on_node(
                                modifier,
                                diag::X_0_modifier_cannot_appear_on_a_module_or_namespace_element,
                                args![text],
                            );
                        } else if flags.intersects(ModifierFlags::ABSTRACT) {
                            if modifier_kind == SyntaxKind::PrivateKeyword {
                                return self.grammar_error_on_node(
                                    modifier,
                                    diag::X_0_modifier_cannot_be_used_with_1_modifier,
                                    args![text, "abstract"],
                                );
                            } else if not_reparsed {
                                return self.grammar_error_on_node(
                                    modifier,
                                    diag::X_0_modifier_must_precede_1_modifier,
                                    args![text, "abstract"],
                                );
                            }
                        } else if is_private_identifier_class_element_declaration(node) {
                            return self.grammar_error_on_node(
                                modifier,
                                diag::An_accessibility_modifier_cannot_be_used_with_a_private_identifier,
                                args![],
                            );
                        }
                        flags |= modifier_to_flag(modifier_kind);
                    }
                    SyntaxKind::StaticKeyword => {
                        if flags.intersects(ModifierFlags::STATIC) {
                            return self.grammar_error_on_node(
                                modifier,
                                diag::X_0_modifier_already_seen,
                                args!["static"],
                            );
                        } else if flags.intersects(ModifierFlags::READONLY) && not_reparsed {
                            return self.grammar_error_on_node(
                                modifier,
                                diag::X_0_modifier_must_precede_1_modifier,
                                args!["static", "readonly"],
                            );
                        } else if flags.intersects(ModifierFlags::ASYNC) && not_reparsed {
                            return self.grammar_error_on_node(
                                modifier,
                                diag::X_0_modifier_must_precede_1_modifier,
                                args!["static", "async"],
                            );
                        } else if flags.intersects(ModifierFlags::ACCESSOR) && not_reparsed {
                            return self.grammar_error_on_node(
                                modifier,
                                diag::X_0_modifier_must_precede_1_modifier,
                                args!["static", "accessor"],
                            );
                        } else if parent.kind() == SyntaxKind::ModuleBlock
                            || parent.kind() == SyntaxKind::SourceFile
                        {
                            return self.grammar_error_on_node(
                                modifier,
                                diag::X_0_modifier_cannot_appear_on_a_module_or_namespace_element,
                                args!["static"],
                            );
                        } else if node_kind == SyntaxKind::Parameter {
                            return self.grammar_error_on_node(
                                modifier,
                                diag::X_0_modifier_cannot_appear_on_a_parameter,
                                args!["static"],
                            );
                        } else if flags.intersects(ModifierFlags::ABSTRACT) {
                            return self.grammar_error_on_node(
                                modifier,
                                diag::X_0_modifier_cannot_be_used_with_1_modifier,
                                args!["static", "abstract"],
                            );
                        } else if flags.intersects(ModifierFlags::OVERRIDE) && not_reparsed {
                            return self.grammar_error_on_node(
                                modifier,
                                diag::X_0_modifier_must_precede_1_modifier,
                                args!["static", "override"],
                            );
                        }
                        flags |= ModifierFlags::STATIC;
                        last_static = modifier;
                    }
                    SyntaxKind::AccessorKeyword => {
                        if flags.intersects(ModifierFlags::ACCESSOR) {
                            return self.grammar_error_on_node(
                                modifier,
                                diag::X_0_modifier_already_seen,
                                args!["accessor"],
                            );
                        } else if flags.intersects(ModifierFlags::READONLY) {
                            return self.grammar_error_on_node(
                                modifier,
                                diag::X_0_modifier_cannot_be_used_with_1_modifier,
                                args!["accessor", "readonly"],
                            );
                        } else if flags.intersects(ModifierFlags::AMBIENT) {
                            return self.grammar_error_on_node(
                                modifier,
                                diag::X_0_modifier_cannot_be_used_with_1_modifier,
                                args!["accessor", "declare"],
                            );
                        } else if node_kind != SyntaxKind::PropertyDeclaration {
                            return self.grammar_error_on_node(
                                modifier,
                                diag::X_accessor_modifier_can_only_appear_on_a_property_declaration,
                                args![],
                            );
                        }

                        flags |= ModifierFlags::ACCESSOR;
                    }
                    SyntaxKind::ReadonlyKeyword => {
                        if flags.intersects(ModifierFlags::READONLY) {
                            return self.grammar_error_on_node(
                                modifier,
                                diag::X_0_modifier_already_seen,
                                args!["readonly"],
                            );
                        } else if node_kind != SyntaxKind::PropertyDeclaration
                            && node_kind != SyntaxKind::PropertySignature
                            && node_kind != SyntaxKind::IndexSignature
                            && node_kind != SyntaxKind::Parameter
                        {
                            // If node.kind === SyntaxKind.Parameter, checkParameter reports an error if it's not a parameter property.
                            return self.grammar_error_on_node(
                                modifier,
                                diag::X_readonly_modifier_can_only_appear_on_a_property_declaration_or_index_signature,
                                args![],
                            );
                        } else if flags.intersects(ModifierFlags::ACCESSOR) {
                            return self.grammar_error_on_node(
                                modifier,
                                diag::X_0_modifier_cannot_be_used_with_1_modifier,
                                args!["readonly", "accessor"],
                            );
                        }
                        flags |= ModifierFlags::READONLY;
                    }
                    SyntaxKind::ExportKeyword => {
                        if self.compiler_options.verbatim_module_syntax == Tristate::True
                            && !node.flags().intersects(NodeFlags::AMBIENT)
                            && node_kind != SyntaxKind::TypeAliasDeclaration
                            && node_kind != SyntaxKind::InterfaceDeclaration
                            && node_kind != SyntaxKind::ModuleDeclaration
                            && parent.kind() == SyntaxKind::SourceFile
                            && get_emit_module_format_of_file(get_source_file_of_node(node))
                                == ModuleKind::COMMON_JS
                        {
                            return self.grammar_error_on_node(
                                modifier,
                                diag::A_top_level_export_modifier_cannot_be_used_on_value_declarations_in_a_CommonJS_module_when_verbatimModuleSyntax_is_enabled,
                                args![],
                            );
                        }
                        if flags.intersects(ModifierFlags::EXPORT) {
                            return self.grammar_error_on_node(
                                modifier,
                                diag::X_0_modifier_already_seen,
                                args!["export"],
                            );
                        } else if flags.intersects(ModifierFlags::AMBIENT) && not_reparsed {
                            return self.grammar_error_on_node(
                                modifier,
                                diag::X_0_modifier_must_precede_1_modifier,
                                args!["export", "declare"],
                            );
                        } else if flags.intersects(ModifierFlags::ABSTRACT) && not_reparsed {
                            return self.grammar_error_on_node(
                                modifier,
                                diag::X_0_modifier_must_precede_1_modifier,
                                args!["export", "abstract"],
                            );
                        } else if flags.intersects(ModifierFlags::ASYNC) && not_reparsed {
                            return self.grammar_error_on_node(
                                modifier,
                                diag::X_0_modifier_must_precede_1_modifier,
                                args!["export", "async"],
                            );
                        } else if is_class_like(parent) && !is_js_type_alias_declaration(node) {
                            return self.grammar_error_on_node(
                                modifier,
                                diag::X_0_modifier_cannot_appear_on_class_elements_of_this_kind,
                                args!["export"],
                            );
                        } else if node_kind == SyntaxKind::Parameter {
                            return self.grammar_error_on_node(
                                modifier,
                                diag::X_0_modifier_cannot_appear_on_a_parameter,
                                args!["export"],
                            );
                        } else if block_scope_kind == NodeFlags::USING {
                            return self.grammar_error_on_node(
                                modifier,
                                diag::X_0_modifier_cannot_appear_on_a_using_declaration,
                                args!["export"],
                            );
                        } else if block_scope_kind == NodeFlags::AWAIT_USING {
                            return self.grammar_error_on_node(
                                modifier,
                                diag::X_0_modifier_cannot_appear_on_an_await_using_declaration,
                                args!["export"],
                            );
                        }
                        flags |= ModifierFlags::EXPORT;
                    }
                    SyntaxKind::DefaultKeyword => {
                        let container = if parent.kind() == SyntaxKind::SourceFile {
                            parent
                        } else {
                            parent.parent()
                        };
                        if container.kind() == SyntaxKind::ModuleDeclaration
                            && !is_ambient_module(container)
                        {
                            return self.grammar_error_on_node(
                                modifier,
                                diag::A_default_export_can_only_be_used_in_an_ECMAScript_style_module,
                                args![],
                            );
                        } else if block_scope_kind == NodeFlags::USING {
                            return self.grammar_error_on_node(
                                modifier,
                                diag::X_0_modifier_cannot_appear_on_a_using_declaration,
                                args!["default"],
                            );
                        } else if block_scope_kind == NodeFlags::AWAIT_USING {
                            return self.grammar_error_on_node(
                                modifier,
                                diag::X_0_modifier_cannot_appear_on_an_await_using_declaration,
                                args!["default"],
                            );
                        } else if !flags.intersects(ModifierFlags::EXPORT) && not_reparsed {
                            return self.grammar_error_on_node(
                                modifier,
                                diag::X_0_modifier_must_precede_1_modifier,
                                args!["export", "default"],
                            );
                        } else if saw_export_before_decorators {
                            return self.grammar_error_on_node(
                                first_decorator,
                                diag::Decorators_are_not_valid_here,
                                args![],
                            );
                        }

                        flags |= ModifierFlags::DEFAULT;
                    }
                    SyntaxKind::DeclareKeyword => {
                        if flags.intersects(ModifierFlags::AMBIENT) {
                            return self.grammar_error_on_node(
                                modifier,
                                diag::X_0_modifier_already_seen,
                                args!["declare"],
                            );
                        } else if flags.intersects(ModifierFlags::ASYNC) {
                            return self.grammar_error_on_node(
                                modifier,
                                diag::X_0_modifier_cannot_be_used_in_an_ambient_context,
                                args!["async"],
                            );
                        } else if flags.intersects(ModifierFlags::OVERRIDE) {
                            return self.grammar_error_on_node(
                                modifier,
                                diag::X_0_modifier_cannot_be_used_in_an_ambient_context,
                                args!["override"],
                            );
                        } else if is_class_like(parent) && !is_property_declaration(node) {
                            return self.grammar_error_on_node(
                                modifier,
                                diag::X_0_modifier_cannot_appear_on_class_elements_of_this_kind,
                                args!["declare"],
                            );
                        } else if node_kind == SyntaxKind::Parameter {
                            return self.grammar_error_on_node(
                                modifier,
                                diag::X_0_modifier_cannot_appear_on_a_parameter,
                                args!["declare"],
                            );
                        } else if block_scope_kind == NodeFlags::USING {
                            return self.grammar_error_on_node(
                                modifier,
                                diag::X_0_modifier_cannot_appear_on_a_using_declaration,
                                args!["declare"],
                            );
                        } else if block_scope_kind == NodeFlags::AWAIT_USING {
                            return self.grammar_error_on_node(
                                modifier,
                                diag::X_0_modifier_cannot_appear_on_an_await_using_declaration,
                                args!["declare"],
                            );
                        } else if parent.flags().intersects(NodeFlags::AMBIENT)
                            && parent.kind() == SyntaxKind::ModuleBlock
                        {
                            return self.grammar_error_on_node(
                                modifier,
                                diag::A_declare_modifier_cannot_be_used_in_an_already_ambient_context,
                                args![],
                            );
                        } else if is_private_identifier_class_element_declaration(node) {
                            return self.grammar_error_on_node(
                                modifier,
                                diag::X_0_modifier_cannot_be_used_with_a_private_identifier,
                                args!["declare"],
                            );
                        } else if flags.intersects(ModifierFlags::ACCESSOR) {
                            return self.grammar_error_on_node(
                                modifier,
                                diag::X_0_modifier_cannot_be_used_with_1_modifier,
                                args!["declare", "accessor"],
                            );
                        }
                        flags |= ModifierFlags::AMBIENT;
                        last_declare = modifier;
                    }
                    SyntaxKind::AbstractKeyword => {
                        if flags.intersects(ModifierFlags::ABSTRACT) {
                            return self.grammar_error_on_node(
                                modifier,
                                diag::X_0_modifier_already_seen,
                                args!["abstract"],
                            );
                        }
                        if node_kind != SyntaxKind::ClassDeclaration
                            && node_kind != SyntaxKind::ConstructorType
                        {
                            if node_kind != SyntaxKind::MethodDeclaration
                                && node_kind != SyntaxKind::PropertyDeclaration
                                && node_kind != SyntaxKind::GetAccessor
                                && node_kind != SyntaxKind::SetAccessor
                            {
                                return self.grammar_error_on_node(
                                    modifier,
                                    diag::X_abstract_modifier_can_only_appear_on_a_class_method_or_property_declaration,
                                    args![],
                                );
                            }
                            if !(parent.kind() == SyntaxKind::ClassDeclaration
                                && has_syntactic_modifier(parent, ModifierFlags::ABSTRACT))
                            {
                                let message = if node_kind == SyntaxKind::PropertyDeclaration {
                                    diag::Abstract_properties_can_only_appear_within_an_abstract_class
                                } else {
                                    diag::Abstract_methods_can_only_appear_within_an_abstract_class
                                };
                                return self.grammar_error_on_node(modifier, message, args![]);
                            }
                            if flags.intersects(ModifierFlags::STATIC) {
                                return self.grammar_error_on_node(
                                    modifier,
                                    diag::X_0_modifier_cannot_be_used_with_1_modifier,
                                    args!["static", "abstract"],
                                );
                            }
                            if flags.intersects(ModifierFlags::PRIVATE) {
                                return self.grammar_error_on_node(
                                    modifier,
                                    diag::X_0_modifier_cannot_be_used_with_1_modifier,
                                    args!["private", "abstract"],
                                );
                            }
                            if flags.intersects(ModifierFlags::ASYNC) && last_async.is_some() {
                                return self.grammar_error_on_node(
                                    last_async,
                                    diag::X_0_modifier_cannot_be_used_with_1_modifier,
                                    args!["async", "abstract"],
                                );
                            }
                            if flags.intersects(ModifierFlags::OVERRIDE) && not_reparsed {
                                return self.grammar_error_on_node(
                                    modifier,
                                    diag::X_0_modifier_must_precede_1_modifier,
                                    args!["abstract", "override"],
                                );
                            }
                            if flags.intersects(ModifierFlags::ACCESSOR) && not_reparsed {
                                return self.grammar_error_on_node(
                                    modifier,
                                    diag::X_0_modifier_must_precede_1_modifier,
                                    args!["abstract", "accessor"],
                                );
                            }
                        }
                        let name = node.name();
                        if name.is_some() && name.kind() == SyntaxKind::PrivateIdentifier {
                            return self.grammar_error_on_node(
                                modifier,
                                diag::X_0_modifier_cannot_be_used_with_a_private_identifier,
                                args!["abstract"],
                            );
                        }

                        flags |= ModifierFlags::ABSTRACT;
                    }
                    SyntaxKind::AsyncKeyword => {
                        if flags.intersects(ModifierFlags::ASYNC) {
                            return self.grammar_error_on_node(
                                modifier,
                                diag::X_0_modifier_already_seen,
                                args!["async"],
                            );
                        } else if flags.intersects(ModifierFlags::AMBIENT)
                            || parent.flags().intersects(NodeFlags::AMBIENT)
                        {
                            return self.grammar_error_on_node(
                                modifier,
                                diag::X_0_modifier_cannot_be_used_in_an_ambient_context,
                                args!["async"],
                            );
                        } else if node_kind == SyntaxKind::Parameter {
                            return self.grammar_error_on_node(
                                modifier,
                                diag::X_0_modifier_cannot_appear_on_a_parameter,
                                args!["async"],
                            );
                        }
                        if flags.intersects(ModifierFlags::ABSTRACT) {
                            return self.grammar_error_on_node(
                                modifier,
                                diag::X_0_modifier_cannot_be_used_with_1_modifier,
                                args!["async", "abstract"],
                            );
                        }
                        flags |= ModifierFlags::ASYNC;
                        last_async = modifier;
                    }
                    SyntaxKind::InKeyword | SyntaxKind::OutKeyword => {
                        let in_out_flag = if modifier_kind == SyntaxKind::InKeyword {
                            ModifierFlags::IN
                        } else {
                            ModifierFlags::OUT
                        };
                        let in_out_text = if modifier_kind == SyntaxKind::InKeyword {
                            "in"
                        } else {
                            "out"
                        };
                        if node_kind != SyntaxKind::TypeParameter
                            || parent.is_some()
                                && !(is_interface_declaration(parent)
                                    || is_class_like(parent)
                                    || is_type_or_js_type_alias_declaration(parent))
                        {
                            return self.grammar_error_on_node(
                                modifier,
                                diag::X_0_modifier_can_only_appear_on_a_type_parameter_of_a_class_interface_or_type_alias,
                                args![in_out_text],
                            );
                        }
                        if flags.intersects(in_out_flag) {
                            return self.grammar_error_on_node(
                                modifier,
                                diag::X_0_modifier_already_seen,
                                args![in_out_text],
                            );
                        }
                        if in_out_flag.intersects(ModifierFlags::IN)
                            && flags.intersects(ModifierFlags::OUT)
                        {
                            return self.grammar_error_on_node(
                                modifier,
                                diag::X_0_modifier_must_precede_1_modifier,
                                args!["in", "out"],
                            );
                        }
                        flags |= in_out_flag;
                    }
                    _ => {}
                }
            }
        }

        if node_kind == SyntaxKind::Constructor {
            if flags.intersects(ModifierFlags::STATIC) {
                return self.grammar_error_on_node(
                    last_static,
                    diag::X_0_modifier_cannot_appear_on_a_constructor_declaration,
                    args!["static"],
                );
            }
            if flags.intersects(ModifierFlags::OVERRIDE) {
                return self.grammar_error_on_node(
                    last_override,
                    diag::X_0_modifier_cannot_appear_on_a_constructor_declaration,
                    args!["override"],
                );
            }
            if flags.intersects(ModifierFlags::ASYNC) {
                return self.grammar_error_on_node(
                    last_async,
                    diag::X_0_modifier_cannot_appear_on_a_constructor_declaration,
                    args!["async"],
                );
            }
            return false;
        } else if (node_kind == SyntaxKind::ImportDeclaration
            || node_kind == SyntaxKind::JsImportDeclaration
            || node_kind == SyntaxKind::ImportEqualsDeclaration)
            && flags.intersects(ModifierFlags::AMBIENT)
        {
            return self.grammar_error_on_node(
                last_declare,
                diag::A_0_modifier_cannot_be_used_with_an_import_declaration,
                args!["declare"],
            );
        } else if node_kind == SyntaxKind::Parameter
            && flags.intersects(ModifierFlags::PARAMETER_PROPERTY_MODIFIER)
            && is_binding_pattern(node.name())
        {
            return self.grammar_error_on_node(
                node,
                diag::A_parameter_property_may_not_be_declared_using_a_binding_pattern,
                args![],
            );
        } else if node_kind == SyntaxKind::Parameter
            && flags.intersects(ModifierFlags::PARAMETER_PROPERTY_MODIFIER)
            && node.dot_dot_dot_token().is_some()
        {
            return self.grammar_error_on_node(
                node,
                diag::A_parameter_property_cannot_be_declared_using_a_rest_parameter,
                args![],
            );
        }
        if flags.intersects(ModifierFlags::ASYNC) {
            return self.check_grammar_async_modifier(node, last_async);
        }
        false
    }

    // Go: checker/grammarchecks.go:571 reportObviousModifierErrors
    // PERF: chkport1 item 4. The caller passes the node kind, the parent and
    // the modifier nodes that it read once (`check_grammar_modifiers`).
    pub fn report_obvious_modifier_errors(
        &mut self,
        node: Node,
        node_kind: SyntaxKind,
        parent: Node,
        modifiers: NodeSlice,
    ) -> bool {
        let modifier = self.find_first_illegal_modifier(node, node_kind, parent, modifiers);
        if modifier.is_nil() {
            return false;
        }
        self.grammar_error_on_first_token(modifier, diag::Modifiers_cannot_appear_here, args![])
    }

    // Go: checker/grammarchecks.go:579 findFirstModifierExcept
    // PERF: chkport1 item 4. Takes `node.ModifierNodes()`, read once.
    pub fn find_first_modifier_except(
        &self,
        modifiers: NodeSlice,
        allowed_modifier: SyntaxKind,
    ) -> Node {
        let modifier = modifiers
            .iter()
            .find(|&m| is_modifier(m))
            .unwrap_or(Node::NIL);
        if modifier.is_some() && modifier.kind() != allowed_modifier {
            return modifier;
        }
        Node::NIL
    }

    // Go: checker/grammarchecks.go:587 findFirstIllegalModifier
    // PERF: chkport1 item 4. `node_kind`, `parent` and `modifiers` are
    // `node.Kind`, `node.Parent` and `node.ModifierNodes()`, read once.
    pub fn find_first_illegal_modifier(
        &self,
        node: Node,
        node_kind: SyntaxKind,
        parent: Node,
        modifiers: NodeSlice,
    ) -> Node {
        match node_kind {
            SyntaxKind::GetAccessor
            | SyntaxKind::SetAccessor
            | SyntaxKind::Constructor
            | SyntaxKind::PropertyDeclaration
            | SyntaxKind::PropertySignature
            | SyntaxKind::MethodDeclaration
            | SyntaxKind::MethodSignature
            | SyntaxKind::IndexSignature
            | SyntaxKind::ModuleDeclaration
            | SyntaxKind::ImportDeclaration
            | SyntaxKind::JsImportDeclaration
            | SyntaxKind::ImportEqualsDeclaration
            | SyntaxKind::ExportDeclaration
            | SyntaxKind::ExportAssignment
            | SyntaxKind::FunctionExpression
            | SyntaxKind::ArrowFunction
            | SyntaxKind::Parameter
            | SyntaxKind::TypeParameter
            | SyntaxKind::JsTypeAliasDeclaration => Node::NIL,
            SyntaxKind::ClassStaticBlockDeclaration
            | SyntaxKind::PropertyAssignment
            | SyntaxKind::ShorthandPropertyAssignment
            | SyntaxKind::NamespaceExportDeclaration
            | SyntaxKind::MissingDeclaration => modifiers
                .iter()
                .find(|&m| is_modifier(m))
                .unwrap_or(Node::NIL),
            _ => {
                let parent_kind = parent.kind();
                if parent_kind == SyntaxKind::ModuleBlock || parent_kind == SyntaxKind::SourceFile {
                    return Node::NIL;
                }
                match node_kind {
                    SyntaxKind::FunctionDeclaration => {
                        self.find_first_modifier_except(modifiers, SyntaxKind::AsyncKeyword)
                    }
                    SyntaxKind::ClassDeclaration | SyntaxKind::ConstructorType => {
                        self.find_first_modifier_except(modifiers, SyntaxKind::AbstractKeyword)
                    }
                    SyntaxKind::ClassExpression
                    | SyntaxKind::InterfaceDeclaration
                    | SyntaxKind::TypeAliasDeclaration => modifiers
                        .iter()
                        .find(|&m| is_modifier(m))
                        .unwrap_or(Node::NIL),
                    SyntaxKind::VariableStatement => {
                        if node.declaration_list().flags().intersects(NodeFlags::USING) {
                            return self
                                .find_first_modifier_except(modifiers, SyntaxKind::AwaitKeyword);
                        }
                        modifiers
                            .iter()
                            .find(|&m| is_modifier(m))
                            .unwrap_or(Node::NIL)
                    }
                    SyntaxKind::EnumDeclaration => {
                        self.find_first_modifier_except(modifiers, SyntaxKind::ConstKeyword)
                    }
                    _ => panic!("Unhandled case in findFirstIllegalModifier."),
                }
            }
        }
    }

    // Go: checker/grammarchecks.go:642 reportObviousDecoratorErrors
    // PERF: chkport1 item 4. Takes `node.ModifierNodes()`, read once.
    pub fn report_obvious_decorator_errors(&mut self, node: Node, modifiers: NodeSlice) -> bool {
        let decorator = self.find_first_illegal_decorator(node, modifiers);
        if decorator.is_nil() {
            return false;
        }
        self.grammar_error_on_first_token(decorator, diag::Decorators_are_not_valid_here, args![])
    }

    // Go: checker/grammarchecks.go:650 findFirstIllegalDecorator
    // PERF: chkport1 item 4. Takes `node.ModifierNodes()`, read once.
    pub fn find_first_illegal_decorator(&self, node: Node, modifiers: NodeSlice) -> Node {
        if can_have_illegal_decorators(node) {
            let decorator = modifiers
                .iter()
                .find(|&m| is_decorator(m))
                .unwrap_or(Node::NIL);
            decorator
        } else {
            Node::NIL
        }
    }

    // Go: checker/grammarchecks.go:659 checkGrammarAsyncModifier
    pub fn check_grammar_async_modifier(&mut self, node: Node, async_modifier: Node) -> bool {
        match node.kind() {
            SyntaxKind::MethodDeclaration
            | SyntaxKind::FunctionDeclaration
            | SyntaxKind::FunctionExpression
            | SyntaxKind::ArrowFunction => {
                return false;
            }
            _ => {}
        }

        self.grammar_error_on_node(
            async_modifier,
            diag::X_0_modifier_cannot_be_used_here,
            args!["async"],
        )
    }

    // Go: checker/grammarchecks.go:671 checkGrammarForDisallowedTrailingComma
    pub fn check_grammar_for_disallowed_trailing_comma(
        &mut self,
        list: NodeList,
        diag: &'static Message,
    ) -> bool {
        if list.is_some() && list.has_trailing_comma() {
            let len_comma = ",".len() as i32;
            return self.grammar_error_at_pos(
                list.nodes().get(0),
                list.end() - len_comma,
                len_comma,
                diag,
                args![],
            );
        }
        false
    }

    // Go: checker/grammarchecks.go:678 checkGrammarTypeParameterList
    pub fn check_grammar_type_parameter_list(
        &mut self,
        type_parameters: NodeList,
        file: Node,
    ) -> bool {
        if type_parameters.is_some() && type_parameters.nodes().len() == 0 {
            let start = type_parameters.pos() - "<".len() as i32;
            let end =
                skip_trivia(&source_file_text(file), type_parameters.end()) + ">".len() as i32;
            return self.grammar_error_at_pos(
                file,
                start,
                end - start,
                diag::Type_parameter_list_cannot_be_empty,
                args![],
            );
        }
        false
    }

    // Go: checker/grammarchecks.go:687 checkGrammarParameterList
    pub fn check_grammar_parameter_list(&mut self, parameters: NodeList) -> bool {
        let mut seen_optional_parameter = false;
        let parameter_nodes = parameters.nodes();
        let parameter_count = parameter_nodes.len();

        for i in 0..parameter_count {
            let parameter = parameter_nodes.get(i);
            if parameter.dot_dot_dot_token().is_some() {
                if i != parameter_count - 1 {
                    return self.grammar_error_on_node(
                        parameter.dot_dot_dot_token(),
                        diag::A_rest_parameter_must_be_last_in_a_parameter_list,
                        args![],
                    );
                }
                if !parameter.flags().intersects(NodeFlags::AMBIENT) {
                    self.check_grammar_for_disallowed_trailing_comma(
                        parameters,
                        diag::A_rest_parameter_or_binding_pattern_may_not_have_a_trailing_comma,
                    );
                }

                if parameter.question_token().is_some() {
                    return self.grammar_error_on_node(
                        parameter.question_token(),
                        diag::A_rest_parameter_cannot_be_optional,
                        args![],
                    );
                }

                if parameter.initializer().is_some() {
                    return self.grammar_error_on_node(
                        parameter.name(),
                        diag::A_rest_parameter_cannot_have_an_initializer,
                        args![],
                    );
                }
            } else if is_optional_declaration(parameter) {
                seen_optional_parameter = true;
                // A reparsed '?' token indicates a bracketed name in @param tag
                let question_token = parameter.question_token();
                if question_token.is_some()
                    && !question_token.flags().intersects(NodeFlags::REPARSED)
                    && parameter.initializer().is_some()
                {
                    return self.grammar_error_on_node(
                        parameter.name(),
                        diag::Parameter_cannot_have_question_mark_and_initializer,
                        args![],
                    );
                }
            } else if seen_optional_parameter && parameter.initializer().is_nil() {
                return self.grammar_error_on_node(
                    parameter.name(),
                    diag::A_required_parameter_cannot_follow_an_optional_parameter,
                    args![],
                );
            }
        }

        false
    }

    // Go: checker/grammarchecks.go:722 checkGrammarForUseStrictSimpleParameterList
    pub fn check_grammar_for_use_strict_simple_parameter_list(&mut self, node: Node) -> bool {
        if self.language_version >= ScriptTarget::ES2016 {
            let body = node.body();
            let mut use_strict_directive = Node::NIL;
            if body.is_some() && is_block(body) {
                use_strict_directive = find_use_strict_prologue(
                    get_source_file_of_node(node),
                    &body.statements().to_vec(),
                );
            }
            if use_strict_directive.is_some() {
                let non_simple_parameters: Vec<Node> = node
                    .parameters()
                    .iter()
                    .filter(|&parameter| {
                        parameter.initializer().is_some()
                            || is_binding_pattern(parameter.name())
                            || is_rest_parameter(parameter)
                    })
                    .collect();
                if !non_simple_parameters.is_empty() {
                    for &parameter in &non_simple_parameters {
                        // PORT: Go `c.error(...)` then mutates the stored
                        // diagnostic; the related info is attached before the add.
                        let mut err = new_diagnostic_for_node(
                            parameter,
                            diag::This_parameter_is_not_allowed_with_use_strict_directive,
                            args![],
                        );
                        err.add_related_info(Some(create_diagnostic_for_node(
                            use_strict_directive,
                            diag::X_use_strict_directive_used_here,
                            args![],
                        )));
                        self.add_diagnostic(err);
                    }

                    let mut err = new_diagnostic_for_node(
                        use_strict_directive,
                        diag::X_use_strict_directive_cannot_be_used_with_non_simple_parameter_list,
                        args![],
                    );
                    for (index, &parameter) in non_simple_parameters.iter().enumerate() {
                        let related_message = if index == 0 {
                            diag::Non_simple_parameter_declared_here
                        } else {
                            diag::X_and_here
                        };
                        err.add_related_info(Some(create_diagnostic_for_node(
                            parameter,
                            related_message,
                            args![],
                        )));
                    }
                    self.add_diagnostic(err);

                    return true;
                }
            }
        }
        false
    }

    // Go: checker/grammarchecks.go:758 checkGrammarFunctionLikeDeclaration
    pub fn check_grammar_function_like_declaration(&mut self, node: Node) -> bool {
        // Prevent cascading error by short-circuit
        let file = get_source_file_of_node(node);
        // PORT: Go reads `node.FunctionLikeData().TypeParameters` and
        // `.Parameters`. For function-like nodes the Go methods
        // `TypeParameterList()` and `ParameterList()` return those same lists.
        let type_parameters = node.type_parameter_list();
        let parameters = node.parameter_list();
        self.check_grammar_modifiers(node)
            || self.check_grammar_type_parameter_list(type_parameters, file)
            || self.check_grammar_parameter_list(parameters)
            || self.check_grammar_arrow_function(node, file)
            || (is_function_like_declaration(node)
                && self.check_grammar_for_use_strict_simple_parameter_list(node))
    }

    // Go: checker/grammarchecks.go:767 checkGrammarClassLikeDeclaration
    pub fn check_grammar_class_like_declaration(&mut self, node: Node) -> bool {
        let file = get_source_file_of_node(node);
        self.check_grammar_class_declaration_heritage_clauses(node, file)
            || self.check_grammar_type_parameter_list(node.type_parameter_list(), file)
    }

    // Go: checker/grammarchecks.go:772 checkGrammarArrowFunction
    pub fn check_grammar_arrow_function(&mut self, node: Node, file: Node) -> bool {
        if !is_arrow_function(node) {
            return false;
        }

        let type_parameters = node.type_parameter_list();
        if type_parameters.is_some() {
            let type_param_nodes = type_parameters.nodes();
            let has_constraint =
                type_param_nodes.len() > 0 && type_param_nodes.get(0).constraint().is_some();
            if !(type_param_nodes.len() > 1
                || type_parameters.has_trailing_comma()
                || has_constraint)
            {
                if file_extension_is_one_of_gc1(source_file_file_name(file), &[".mts", ".cts"]) {
                    // TODO(danielr): should we return early here?
                    self.grammar_error_on_node(
                        type_param_nodes.get(0),
                        diag::This_syntax_is_reserved_in_files_with_the_mts_or_cts_extension_Add_a_trailing_comma_or_explicit_constraint,
                        args![],
                    );
                }
            }
        }

        let equals_greater_than_token = node.equals_greater_than_token();
        let arrow_full_text = &source_file_text(file)
            [equals_greater_than_token.pos() as usize..equals_greater_than_token.end() as usize];
        arrow_full_text.chars().any(is_line_break)
            && self.grammar_error_on_node(
                equals_greater_than_token,
                diag::Line_terminator_not_permitted_before_arrow,
                args![],
            )
    }

    // Go: checker/grammarchecks.go:796 checkGrammarIndexSignatureParameters
    pub fn check_grammar_index_signature_parameters(&mut self, node: Node) -> bool {
        let parameters = node.parameter_list();
        let param_nodes = parameters.nodes();

        if param_nodes.len() == 0 {
            return self.grammar_error_on_node(
                node,
                diag::An_index_signature_must_have_exactly_one_parameter,
                args![],
            );
        }

        let parameter = param_nodes.get(0);
        if param_nodes.len() != 1 {
            return self.grammar_error_on_node(
                parameter.name(),
                diag::An_index_signature_must_have_exactly_one_parameter,
                args![],
            );
        }

        self.check_grammar_for_disallowed_trailing_comma(
            parameters,
            diag::An_index_signature_cannot_have_a_trailing_comma,
        );
        if parameter.dot_dot_dot_token().is_some() {
            return self.grammar_error_on_node(
                parameter.dot_dot_dot_token(),
                diag::An_index_signature_cannot_have_a_rest_parameter,
                args![],
            );
        }
        if parameter.modifiers().is_some() {
            return self.grammar_error_on_node(
                parameter.name(),
                diag::An_index_signature_parameter_cannot_have_an_accessibility_modifier,
                args![],
            );
        }
        if parameter.question_token().is_some() {
            return self.grammar_error_on_node(
                parameter.question_token(),
                diag::An_index_signature_parameter_cannot_have_a_question_mark,
                args![],
            );
        }
        if parameter.initializer().is_some() {
            return self.grammar_error_on_node(
                parameter.name(),
                diag::An_index_signature_parameter_cannot_have_an_initializer,
                args![],
            );
        }
        let type_node = parameter.type_();
        if type_node.is_nil() {
            return self.grammar_error_on_node(
                parameter.name(),
                diag::An_index_signature_parameter_must_have_a_type_annotation,
                args![],
            );
        }
        let t = self.get_type_from_type_node(type_node);
        if self.some_type(t, &mut |c: &mut Checker, t: TypeId| {
            c.ty(t)
                .flags
                .intersects(TypeFlags::STRING_OR_NUMBER_LITERAL_OR_UNIQUE)
        }) || self.is_generic_type(t)
        {
            return self.grammar_error_on_node(
                parameter.name(),
                diag::An_index_signature_parameter_type_cannot_be_a_literal_type_or_generic_type_Consider_using_a_mapped_object_type_instead,
                args![],
            );
        }
        if !self.every_type(t, &mut |c: &mut Checker, t: TypeId| {
            c.is_valid_index_key_type(t)
        }) {
            return self.grammar_error_on_node(
                parameter.name(),
                diag::An_index_signature_parameter_type_must_be_string_number_symbol_or_a_template_literal_type,
                args![],
            );
        }
        if node.type_().is_nil() {
            return self.grammar_error_on_node(
                node,
                diag::An_index_signature_must_have_a_type_annotation,
                args![],
            );
        }
        false
    }

    // Go: checker/grammarchecks.go:840 checkGrammarIndexSignature
    pub fn check_grammar_index_signature(&mut self, node: Node) -> bool {
        // Prevent cascading error by short-circuit
        self.check_grammar_modifiers(node) || self.check_grammar_index_signature_parameters(node)
    }

    // Go: checker/grammarchecks.go:845 checkGrammarForAtLeastOneTypeArgument
    pub fn check_grammar_for_at_least_one_type_argument(
        &mut self,
        node: Node,
        type_arguments: NodeList,
    ) -> bool {
        if type_arguments.is_some() && type_arguments.nodes().len() == 0 {
            let source_file = get_source_file_of_node(node);
            let start = type_arguments.pos() - "<".len() as i32;
            let end = skip_trivia(&source_file_text(source_file), type_arguments.end())
                + ">".len() as i32;
            return self.grammar_error_at_pos(
                source_file,
                start,
                end - start,
                diag::Type_argument_list_cannot_be_empty,
                args![],
            );
        }
        false
    }

    // Go: checker/grammarchecks.go:855 checkGrammarTypeArguments
    pub fn check_grammar_type_arguments(&mut self, node: Node, type_arguments: NodeList) -> bool {
        self.check_grammar_for_disallowed_trailing_comma(
            type_arguments,
            diag::Trailing_comma_not_allowed,
        ) || self.check_grammar_for_at_least_one_type_argument(node, type_arguments)
    }

    // Go: checker/grammarchecks.go:859 checkGrammarTaggedTemplateChain
    pub fn check_grammar_tagged_template_chain(&mut self, node: Node) -> bool {
        if node.question_dot_token().is_some() || node.flags().intersects(NodeFlags::OPTIONAL_CHAIN)
        {
            return self.grammar_error_on_node(
                node.template(),
                diag::Tagged_template_expressions_are_not_permitted_in_an_optional_chain,
                args![],
            );
        }
        false
    }

    // Go: checker/grammarchecks.go:866 checkGrammarHeritageClause
    pub fn check_grammar_heritage_clause(&mut self, node: Node) -> bool {
        let types = node.types();
        if self.check_grammar_for_disallowed_trailing_comma(types, diag::Trailing_comma_not_allowed)
        {
            return true;
        }
        if types.is_some() && types.nodes().len() == 0 {
            let list_type = token_to_string(node.token());
            // TODO(danielr): why not error on the token?
            return self.grammar_error_at_pos(
                node,
                types.pos(),
                0,
                diag::X_0_list_cannot_be_empty,
                args![list_type],
            );
        }

        for node in types.nodes() {
            if self.check_grammar_expression_with_type_arguments(node) {
                return true;
            }
        }
        false
    }

    // Go: checker/grammarchecks.go:885 checkGrammarExpressionWithTypeArguments
    pub fn check_grammar_expression_with_type_arguments(
        &mut self,
        node: Node, /*Union[ExpressionWithTypeArguments, TypeQuery]*/
    ) -> bool {
        if is_expression_with_type_arguments(node)
            && node.expression().kind() == SyntaxKind::ImportKeyword
            && node.type_argument_list().is_some()
        {
            return self.grammar_error_on_node(
                node,
                diag::This_use_of_import_is_invalid_import_calls_can_be_written_but_they_must_have_parentheses_and_cannot_have_type_arguments,
                args![],
            );
        }
        self.check_grammar_type_arguments(node, node.type_argument_list())
    }

    // Go: checker/grammarchecks.go:892 checkGrammarClassDeclarationHeritageClauses
    pub fn check_grammar_class_declaration_heritage_clauses(
        &mut self,
        node: Node,
        _file: Node,
    ) -> bool {
        let mut seen_extends_clause = false;
        let mut seen_implements_clause = false;

        // PORT: Go reads `node.ClassLikeData().HeritageClauses`; the generated
        // `heritage_clauses()` field accessor reads the same field.
        let heritage_clauses = node.heritage_clauses();

        if !self.check_grammar_modifiers(node) && heritage_clauses.is_some() {
            for heritage_clause_node in heritage_clauses.nodes() {
                let heritage_clause = heritage_clause_node;
                if heritage_clause.token() == SyntaxKind::ExtendsKeyword {
                    if seen_extends_clause {
                        return self.grammar_error_on_first_token(
                            heritage_clause_node,
                            diag::X_extends_clause_already_seen,
                            args![],
                        );
                    }

                    if seen_implements_clause {
                        return self.grammar_error_on_first_token(
                            heritage_clause_node,
                            diag::X_extends_clause_must_precede_implements_clause,
                            args![],
                        );
                    }

                    let type_nodes = heritage_clause.types().nodes();
                    if type_nodes.len() > 1 {
                        return self.grammar_error_on_first_token(
                            type_nodes.get(1),
                            diag::Classes_can_only_extend_a_single_class,
                            args![],
                        );
                    }

                    seen_extends_clause = true;
                } else {
                    if heritage_clause.token() != SyntaxKind::ImplementsKeyword {
                        panic!("Unexpected token {:?}", heritage_clause.token());
                    }
                    if seen_implements_clause {
                        return self.grammar_error_on_first_token(
                            heritage_clause_node,
                            diag::X_implements_clause_already_seen,
                            args![],
                        );
                    }

                    seen_implements_clause = true;
                }

                // Grammar checking heritageClause inside class declaration
                self.check_grammar_heritage_clause(heritage_clause);
            }
        }

        false
    }
}

// PORT: Go `tspath.FileExtensionIsOneOf`. The `tspath` package has no shared
// Rust port, so this file keeps a private copy (like checker_p06/p17).
fn file_extension_is_one_of_gc1(path: &str, extensions: &[&str]) -> bool {
    for ext in extensions {
        // Go: tspath/path.go:1095 FileExtensionIs
        if path.len() > ext.len() && path.ends_with(ext) {
            return true;
        }
    }
    false
}

impl Checker {
    // Go: checker/grammarchecks.go:2123 checkGrammarImportAttributeValues
    // PORT: in Go this follows checkGrammarImportClause (grammarchecks_p3.rs);
    // it is here because the checker lane owns this file. The children of an
    // ImportAttributes node are its `Attributes` list.
    pub fn check_grammar_import_attribute_values(&mut self, node: Node) -> bool {
        let mut has_error = false;
        let mut attributes = Vec::new();
        node.for_each_child(&mut |child: Node| {
            if child.kind() == SyntaxKind::ImportAttribute {
                attributes.push(child);
            }
            false
        });
        for attribute in attributes {
            let value = attribute.value();
            if is_string_literal(value) {
                continue;
            }
            has_error = true;
            self.error(
                value,
                diag::Import_attribute_values_must_be_string_literal_expressions,
                args![],
            );
        }
        has_error
    }

    // Go: checker/grammarchecks.go:2200 checkGrammarImportAttributesType
    // PORT: the Go function is last in grammarchecks.go (grammarchecks_p3.rs);
    // it is here because the checker lane owns this file.
    pub fn check_grammar_import_attributes_type(&mut self, attributes: Node) -> bool {
        let members = attributes.members();
        for member in members {
            if member.kind() != SyntaxKind::PropertySignature {
                return self.grammar_error_on_node(
                    member,
                    diag::An_import_attributes_type_may_only_contain_property_signatures,
                    args![],
                );
            }
            let modifiers = member.modifiers();
            if modifiers.is_some() {
                for modifier in modifiers.nodes() {
                    if modifier.kind() == SyntaxKind::ReadonlyKeyword {
                        return self.grammar_error_on_node(
                            modifier,
                            diag::An_import_attributes_property_cannot_have_a_readonly_modifier,
                            args![],
                        );
                    }
                }
            }
            if member.type_().is_nil() {
                return self.grammar_error_on_node(
                    member,
                    diag::An_import_attributes_property_must_have_a_type_annotation,
                    args![],
                );
            }
            if member.question_token().is_some() {
                return self.grammar_error_on_node(
                    member,
                    diag::An_import_attributes_property_cannot_be_optional,
                    args![],
                );
            }
            let name = member.name();
            if !(is_string_literal_like(name) || is_identifier(name)) {
                return self.grammar_error_on_node(
                    name,
                    diag::An_import_attributes_property_must_have_a_string_literal_or_identifier_name,
                    args![],
                );
            }
            if name.text() == "resolution-mode" {
                return self.grammar_error_on_node(
                    name,
                    diag::X_0_is_not_a_valid_key_for_an_import_attributes_type,
                    args![name.text()],
                );
            }

            let type_node = member.type_();
            // ts#64243 (Go N' grammarchecks.go:2261): a no-substitution template
            // literal type is no string literal type here.
            if !is_literal_type_node(type_node) || !is_string_literal(type_node.literal()) {
                return self.grammar_error_on_node(
                    type_node,
                    diag::An_import_attributes_property_must_have_a_string_literal_type_annotation,
                    args![],
                );
            }
        }
        false
    }
}
