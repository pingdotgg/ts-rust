//! Port of typescript-go `internal/checker/grammarchecks.go` lines 952-1857.

use crate::prelude::*;
use std::borrow::Cow;

use crate::diagnostics::Message;

impl Checker {
    // Go: checker/grammarchecks.go:935 checkGrammarInterfaceDeclaration
    pub fn check_grammar_interface_declaration(&mut self, node: Node) -> bool {
        let heritage_clauses = node.heritage_clauses();
        if !heritage_clauses.is_nil() {
            let mut seen_extends_clause = false;
            for heritage_clause_node in heritage_clauses.nodes().iter() {
                let heritage_clause = heritage_clause_node;

                match heritage_clause.token() {
                    SyntaxKind::ExtendsKeyword => {
                        if seen_extends_clause {
                            return self.grammar_error_on_first_token(
                                heritage_clause_node,
                                diag::X_extends_clause_already_seen,
                                args![],
                            );
                        }
                        seen_extends_clause = true;
                    }
                    SyntaxKind::ImplementsKeyword => {
                        return self.grammar_error_on_first_token(
                            heritage_clause_node,
                            diag::Interface_declaration_cannot_have_implements_clause,
                            args![],
                        );
                    }
                    token => panic!("Unexpected token {:?}", token),
                }

                // Grammar checking heritageClause inside class declaration
                self.check_grammar_heritage_clause(heritage_clause);
            }
        }

        false
    }

    // Go: checker/grammarchecks.go:961 checkGrammarComputedPropertyName
    pub fn check_grammar_computed_property_name(&mut self, node: Node) -> bool {
        // If node is not a computedPropertyName, just skip the grammar checking
        if node.kind() != SyntaxKind::ComputedPropertyName {
            return false;
        }

        let expression = node.expression();
        if expression.kind() == SyntaxKind::BinaryExpression
            && expression.operator_token().kind() == SyntaxKind::CommaToken
        {
            return self.grammar_error_on_node(
                expression,
                diag::A_comma_expression_is_not_allowed_in_a_computed_property_name,
                args![],
            );
        }
        false
    }

    // Go: checker/grammarchecks.go:974 checkGrammarForGenerator
    pub fn check_grammar_for_generator(&mut self, node: Node) -> bool {
        // PORT: Go `node.BodyData()` is non-nil exactly for the kinds that embed
        // `BodyBase` (directly or through `FunctionLikeWithBodyBase`). The body
        // data fields are read through the per-field accessors.
        let has_body_data = matches!(
            node.kind(),
            SyntaxKind::FunctionDeclaration
                | SyntaxKind::Constructor
                | SyntaxKind::GetAccessor
                | SyntaxKind::SetAccessor
                | SyntaxKind::MethodDeclaration
                | SyntaxKind::ArrowFunction
                | SyntaxKind::FunctionExpression
                | SyntaxKind::ModuleDeclaration
        );
        if has_body_data && node.asterisk_token().is_some() {
            let asterisk_token = node.asterisk_token();
            if node.kind() != SyntaxKind::FunctionDeclaration
                && node.kind() != SyntaxKind::FunctionExpression
                && node.kind() != SyntaxKind::MethodDeclaration
            {
                panic!("Unexpected node kind {:?}", node.kind());
            }
            if node.flags().intersects(NodeFlags::AMBIENT) {
                return self.grammar_error_on_node(
                    asterisk_token,
                    diag::Generators_are_not_allowed_in_an_ambient_context,
                    args![],
                );
            }
            if node.body().is_nil() {
                return self.grammar_error_on_node(
                    asterisk_token,
                    diag::An_overload_signature_cannot_be_declared_as_a_generator,
                    args![],
                );
            }
        }

        false
    }

    // Go: checker/grammarchecks.go:990 checkGrammarForInvalidQuestionMark
    pub fn check_grammar_for_invalid_question_mark(
        &mut self,
        postfix_token: Node,
        message: &'static Message,
    ) -> bool {
        postfix_token.is_some()
            && postfix_token.kind() == SyntaxKind::QuestionToken
            && self.grammar_error_on_node(postfix_token, message, args![])
    }

    // Go: checker/grammarchecks.go:994 checkGrammarForInvalidExclamationToken
    pub fn check_grammar_for_invalid_exclamation_token(
        &mut self,
        postfix_token: Node,
        message: &'static Message,
    ) -> bool {
        postfix_token.is_some()
            && postfix_token.kind() == SyntaxKind::ExclamationToken
            && self.grammar_error_on_node(postfix_token, message, args![])
    }

    // Go: checker/grammarchecks.go:998 checkGrammarObjectLiteralExpression
    pub fn check_grammar_object_literal_expression(
        &mut self,
        node: Node,
        in_destructuring: bool,
    ) -> bool {
        let mut seen: FxHashMap<Cow<'static, str>, DeclarationMeaning> = FxHashMap::default();

        // PORT: Go reads `node.Properties.Nodes` with a nil check; `properties()`
        // is empty for a nil list.
        let properties = node.properties();
        for prop in properties {
            if prop.kind() == SyntaxKind::SpreadAssignment {
                let spread_expression = prop.expression();
                if in_destructuring {
                    // a rest property cannot be destructured any further
                    let expression = skip_parentheses(spread_expression);
                    if is_array_literal_expression(expression)
                        || is_object_literal_expression(expression)
                    {
                        return self.grammar_error_on_node(
                            spread_expression,
                            diag::A_rest_element_cannot_contain_a_binding_pattern,
                            args![],
                        );
                    }
                }
                continue;
            }
            let name = prop.name();
            if name.kind() == SyntaxKind::ComputedPropertyName {
                // If the name is not a ComputedPropertyName, the grammar checking will skip it
                self.check_grammar_computed_property_name(name);
            }

            if prop.kind() == SyntaxKind::ShorthandPropertyAssignment && !in_destructuring {
                let object_assignment_initializer = prop.object_assignment_initializer();
                if object_assignment_initializer.is_some() {
                    // having objectAssignmentInitializer is only valid in an ObjectAssignmentPattern.
                    // Outside of destructuring, it is a syntax error.

                    // Try to grab the last node prior to the initializer,
                    // then error on the first token following (which should be the `=` token).
                    let mut last_node_before_initializer = Node::NIL;
                    prop.for_each_child(&mut |child: Node| -> bool {
                        if child != object_assignment_initializer {
                            last_node_before_initializer = child;
                            return false;
                        }
                        true
                    });

                    self.grammar_error_on_first_token(
                        last_node_before_initializer,
                        diag::Did_you_mean_to_use_a_Colon_An_can_only_follow_a_property_name_when_the_containing_object_literal_is_part_of_a_destructuring_pattern,
                        args![],
                    );
                }
            }

            if name.kind() == SyntaxKind::PrivateIdentifier {
                self.grammar_error_on_node(
                    name,
                    diag::Private_identifiers_are_not_allowed_outside_class_bodies,
                    args![],
                );
            }

            // Modifiers are never allowed on properties except for 'async' on a method declaration
            let modifiers = prop.modifier_nodes();
            if modifiers.len() != 0 {
                if can_have_modifiers(prop) {
                    for m in modifiers.iter() {
                        if is_modifier(m)
                            && (m.kind() != SyntaxKind::AsyncKeyword
                                || prop.kind() != SyntaxKind::MethodDeclaration)
                        {
                            self.grammar_error_on_node(
                                m,
                                diag::X_0_modifier_cannot_be_used_here,
                                args![get_text_of_node(m)],
                            );
                        }
                    }
                } else if can_have_illegal_modifiers(prop) {
                    for m in modifiers.iter() {
                        if is_modifier(m) {
                            self.grammar_error_on_node(
                                m,
                                diag::X_0_modifier_cannot_be_used_here,
                                args![get_text_of_node(m)],
                            );
                        }
                    }
                }
            }

            // ECMA-262 11.1.5 Object Initializer
            // If previous is not undefined then throw a SyntaxError exception if any of the following conditions are true
            // a.This production is contained in strict code and IsDataDescriptor(previous) is true and
            // IsDataDescriptor(propId.descriptor) is true.
            //    b.IsDataDescriptor(previous) is true and IsAccessorDescriptor(propId.descriptor) is true.
            //    c.IsAccessorDescriptor(previous) is true and IsDataDescriptor(propId.descriptor) is true.
            //    d.IsAccessorDescriptor(previous) is true and IsAccessorDescriptor(propId.descriptor) is true
            // and either both previous and propId.descriptor have[[Get]] fields or both previous and propId.descriptor have[[Set]] fields
            let current_kind: DeclarationMeaning;
            match prop.kind() {
                SyntaxKind::ShorthandPropertyAssignment | SyntaxKind::PropertyAssignment => {
                    // PORT: Go reads `NamedMemberBase.PostfixToken` from the concrete
                    // node; `postfix_token()` (Go `Node.PostfixToken()`) reads the same
                    // field for both kinds. Go also calls `prop.ClassLikeData()` and
                    // drops the result; that call has no effect and is omitted.
                    let postfix_token = prop.postfix_token();

                    // Grammar checking for computedPropertyName and shorthandPropertyAssignment
                    self.check_grammar_for_invalid_exclamation_token(
                        postfix_token,
                        diag::A_definite_assignment_assertion_is_not_permitted_in_this_context,
                    );
                    self.check_grammar_for_invalid_question_mark(
                        postfix_token,
                        diag::An_object_member_cannot_be_declared_optional,
                    );

                    if name.kind() == SyntaxKind::NumericLiteral {
                        self.check_grammar_numeric_literal(name);
                    }

                    if name.kind() == SyntaxKind::BigIntLiteral {
                        self.add_error_or_suggestion(
                            true,
                            create_diagnostic_for_node(
                                name,
                                diag::A_bigint_literal_cannot_be_used_as_a_property_name,
                                args![],
                            ),
                        );
                    }

                    current_kind = DeclarationMeaning::PROPERTY_ASSIGNMENT;
                }
                SyntaxKind::MethodDeclaration => {
                    current_kind = DeclarationMeaning::METHOD;
                }
                SyntaxKind::GetAccessor => {
                    current_kind = DeclarationMeaning::GET_ACCESSOR;
                }
                SyntaxKind::SetAccessor => {
                    current_kind = DeclarationMeaning::SET_ACCESSOR;
                }
                kind => panic!("Unexpected node kind {:?}", kind),
            }

            if !in_destructuring {
                let (effective_name, ok) =
                    self.get_effective_property_name_for_property_name_node(name);
                if !ok {
                    continue;
                }

                let existing_kind = seen.get(&effective_name).copied().unwrap_or_default();
                if existing_kind.is_empty() {
                    seen.insert(effective_name, current_kind);
                } else if current_kind.intersects(DeclarationMeaning::METHOD)
                    && existing_kind.intersects(DeclarationMeaning::METHOD)
                {
                    self.grammar_error_on_node(
                        name,
                        diag::Duplicate_identifier_0,
                        args![get_text_of_node(name)],
                    );
                } else if current_kind.intersects(DeclarationMeaning::PROPERTY_ASSIGNMENT)
                    && existing_kind.intersects(DeclarationMeaning::PROPERTY_ASSIGNMENT)
                {
                    self.grammar_error_on_node(
                        name,
                        diag::An_object_literal_cannot_have_multiple_properties_with_the_same_name,
                        args![get_text_of_node(name)],
                    );
                } else if current_kind.intersects(DeclarationMeaning::GET_OR_SET_ACCESSOR)
                    && existing_kind.intersects(DeclarationMeaning::GET_OR_SET_ACCESSOR)
                {
                    if existing_kind != DeclarationMeaning::GET_OR_SET_ACCESSOR
                        && current_kind != existing_kind
                    {
                        seen.insert(effective_name, current_kind | existing_kind);
                    } else {
                        return self.grammar_error_on_node(
                            name,
                            diag::An_object_literal_cannot_have_multiple_get_Slashset_accessors_with_the_same_name,
                            args![],
                        );
                    }
                } else {
                    return self.grammar_error_on_node(
                        name,
                        diag::An_object_literal_cannot_have_property_and_accessor_with_the_same_name,
                        args![],
                    );
                }
            }
        }

        false
    }

    // Go: checker/grammarchecks.go:1138 checkGrammarJsxElement
    pub fn check_grammar_jsx_element(&mut self, node: Node) -> bool {
        self.check_grammar_jsx_name(node.tag_name());
        self.check_grammar_type_arguments(node, node.type_argument_list());
        let mut seen: FxHashSet<String> = FxHashSet::default();
        for attr_node in node.attributes().properties().iter() {
            if attr_node.kind() == SyntaxKind::JsxSpreadAttribute {
                continue;
            }
            let attr = attr_node;
            let name = attr.name();
            let initializer = attr.initializer();
            let text_of_name = name.text().to_string();
            if !seen.contains(&text_of_name) {
                seen.insert(text_of_name);
            } else {
                return self.grammar_error_on_node(
                    name,
                    diag::JSX_elements_cannot_have_multiple_attributes_with_the_same_name,
                    args![],
                );
            }
            if initializer.is_some()
                && initializer.kind() == SyntaxKind::JsxExpression
                && initializer.expression().is_nil()
            {
                return self.grammar_error_on_node(
                    initializer,
                    diag::JSX_attributes_must_only_be_assigned_a_non_empty_expression,
                    args![],
                );
            }
        }
        false
    }

    // Go: checker/grammarchecks.go:1162 checkGrammarJsxName
    pub fn check_grammar_jsx_name(&mut self, node: Node) -> bool {
        if is_property_access_expression(node) && is_jsx_namespaced_name(node.expression()) {
            return self.grammar_error_on_node(
                node.expression(),
                diag::JSX_property_access_expressions_cannot_include_JSX_namespace_names,
                args![],
            );
        }

        if is_jsx_namespaced_name(node)
            && self.compiler_options.get_jsx_transform_enabled()
            && !is_intrinsic_jsx_name(&node.namespace().text())
        {
            return self.grammar_error_on_node(
                node,
                diag::React_components_cannot_include_JSX_namespace_names,
                args![],
            );
        }

        false
    }

    // Go: checker/grammarchecks.go:1174 checkGrammarJsxExpression
    pub fn check_grammar_jsx_expression(&mut self, node: Node) -> bool {
        let expression = node.expression();
        if expression.is_some() && is_comma_sequence(expression) {
            return self.grammar_error_on_node(
                expression,
                diag::JSX_expressions_may_not_use_the_comma_operator_Did_you_mean_to_write_an_array,
                args![],
            );
        }

        false
    }

    // Go: checker/grammarchecks.go:1182 checkGrammarForInOrForOfStatement
    pub fn check_grammar_for_in_or_for_of_statement(
        &mut self,
        for_in_or_of_statement: Node,
    ) -> bool {
        let as_node = for_in_or_of_statement;
        if self.check_grammar_statement_in_ambient_context(as_node) {
            return true;
        }

        if for_in_or_of_statement.kind() == SyntaxKind::ForOfStatement
            && for_in_or_of_statement.await_modifier().is_some()
        {
            let await_modifier = for_in_or_of_statement.await_modifier();
            if !for_in_or_of_statement
                .flags()
                .intersects(NodeFlags::AWAIT_CONTEXT)
            {
                let source_file = get_source_file_of_node(as_node);
                if is_in_top_level_context(as_node) {
                    if !self.has_parse_diagnostics(source_file) {
                        if !is_effective_external_module(source_file, self.compiler_options) {
                            self.add_diagnostic(create_diagnostic_for_node(
                                await_modifier,
                                diag::X_for_await_loops_are_only_allowed_at_the_top_level_of_a_file_when_that_file_is_a_module_but_this_file_has_no_imports_or_exports_Consider_adding_an_empty_export_to_make_this_file_a_module,
                                args![],
                            ));
                        }
                        // PORT: Go `switch` with `fallthrough`. `report_top_level` is
                        // true when control reaches the `default` case.
                        let mut report_top_level = false;
                        match self.module_kind {
                            ModuleKind::NODE16
                            | ModuleKind::NODE18
                            | ModuleKind::NODE20
                            | ModuleKind::NODE_NEXT => {
                                let source_file_meta_data =
                                    get_source_file_meta_data(&source_file_info(source_file).path);
                                if source_file_meta_data.implied_node_format
                                    == ModuleKind::COMMON_JS
                                {
                                    self.add_diagnostic(create_diagnostic_for_node(
                                        await_modifier,
                                        diag::The_current_file_is_a_CommonJS_module_and_cannot_use_await_at_the_top_level,
                                        args![],
                                    ));
                                } else if self.language_version < ScriptTarget::ES2017 {
                                    // fallthrough, fallthrough
                                    report_top_level = true;
                                }
                            }
                            ModuleKind::ES2022
                            | ModuleKind::ES_NEXT
                            | ModuleKind::PRESERVE
                            | ModuleKind::SYSTEM => {
                                if self.language_version < ScriptTarget::ES2017 {
                                    // fallthrough
                                    report_top_level = true;
                                }
                            }
                            _ => {
                                report_top_level = true;
                            }
                        }
                        if report_top_level {
                            self.add_diagnostic(create_diagnostic_for_node(
                                await_modifier,
                                diag::Top_level_for_await_loops_are_only_allowed_when_the_module_option_is_set_to_es2022_esnext_system_node16_node18_node20_nodenext_or_preserve_and_the_target_option_is_set_to_es2017_or_higher,
                                args![],
                            ));
                        }
                    }
                } else {
                    // use of 'for-await-of' in non-async function
                    if !self.has_parse_diagnostics(source_file) {
                        let mut diagnostic = create_diagnostic_for_node(
                            await_modifier,
                            diag::X_for_await_loops_are_only_allowed_within_async_functions_and_at_the_top_levels_of_modules,
                            args![],
                        );
                        let containing_func = get_containing_function(for_in_or_of_statement);
                        if containing_func.is_some()
                            && containing_func.kind() != SyntaxKind::Constructor
                        {
                            go_assert!(
                                !get_function_flags(containing_func)
                                    .intersects(FunctionFlags::ASYNC),
                                "Enclosing function should never be an async function."
                            );
                            let related_info = create_diagnostic_for_node(
                                containing_func,
                                diag::Did_you_mean_to_mark_this_function_as_async,
                                args![],
                            );
                            diagnostic.add_related_info(Some(related_info));
                        }
                        self.add_diagnostic(diagnostic);
                        return true;
                    }
                }
            }
        }

        let initializer = for_in_or_of_statement.initializer();
        if is_for_of_statement(as_node)
            && !for_in_or_of_statement
                .flags()
                .intersects(NodeFlags::AWAIT_CONTEXT)
            && is_identifier(initializer)
            && initializer.text() == "async"
        {
            self.grammar_error_on_node(
                initializer,
                diag::The_left_hand_side_of_a_for_of_statement_may_not_be_async,
                args![],
            );
            return false;
        }

        if initializer.kind() == SyntaxKind::VariableDeclarationList {
            let variable_list = initializer;
            if !self.check_grammar_variable_declaration_list(variable_list) {
                let declarations = variable_list.declarations().nodes();

                // declarations.length can be zero if there is an error in variable declaration in for-of or for-in
                // See http://www.ecma-international.org/ecma-262/6.0/#sec-for-in-and-for-of-statements for details
                // For example:
                //      var let = 10;
                //      for (let of [1,2,3]) {} // this is invalid ES6 syntax
                //      for (let in [1,2,3]) {} // this is invalid ES6 syntax
                // We will then want to skip on grammar checking on variableList declaration
                if declarations.len() == 0 {
                    return false;
                }

                if declarations.len() > 1 {
                    let diagnostic = if for_in_or_of_statement.kind() == SyntaxKind::ForInStatement
                    {
                        diag::Only_a_single_variable_declaration_is_allowed_in_a_for_in_statement
                    } else {
                        diag::Only_a_single_variable_declaration_is_allowed_in_a_for_of_statement
                    };
                    return self.grammar_error_on_first_token(
                        declarations.get(1),
                        diagnostic,
                        args![],
                    );
                }

                let first_variable_declaration = declarations.get(0);
                if first_variable_declaration.initializer().is_some() {
                    let diagnostic = if for_in_or_of_statement.kind() == SyntaxKind::ForInStatement
                    {
                        diag::The_variable_declaration_of_a_for_in_statement_cannot_have_an_initializer
                    } else {
                        diag::The_variable_declaration_of_a_for_of_statement_cannot_have_an_initializer
                    };
                    return self.grammar_error_on_node(
                        first_variable_declaration.name(),
                        diagnostic,
                        args![],
                    );
                }
                if first_variable_declaration.type_().is_some() {
                    let diagnostic = if for_in_or_of_statement.kind() == SyntaxKind::ForInStatement
                    {
                        diag::The_left_hand_side_of_a_for_in_statement_cannot_use_a_type_annotation
                    } else {
                        diag::The_left_hand_side_of_a_for_of_statement_cannot_use_a_type_annotation
                    };
                    return self.grammar_error_on_node(
                        first_variable_declaration,
                        diagnostic,
                        args![],
                    );
                }
            }
        }

        false
    }

    // Go: checker/grammarchecks.go:1289 checkGrammarAccessor
    pub fn check_grammar_accessor(&mut self, accessor: Node) -> bool {
        let body = accessor.body();
        if !accessor.flags().intersects(NodeFlags::AMBIENT)
            && (accessor.parent().kind() != SyntaxKind::TypeLiteral)
            && (accessor.parent().kind() != SyntaxKind::InterfaceDeclaration)
        {
            if body.is_nil() && !has_syntactic_modifier(accessor, ModifierFlags::ABSTRACT) {
                return self.grammar_error_at_pos(
                    accessor,
                    accessor.end() - 1,
                    ";".len() as i32,
                    diag::X_0_expected,
                    args!["{"],
                );
            }
        }
        if body.is_some() {
            if has_syntactic_modifier(accessor, ModifierFlags::ABSTRACT) {
                return self.grammar_error_on_node(
                    accessor,
                    diag::An_abstract_accessor_cannot_have_an_implementation,
                    args![],
                );
            }
            if accessor.parent().kind() == SyntaxKind::TypeLiteral
                || accessor.parent().kind() == SyntaxKind::InterfaceDeclaration
            {
                return self.grammar_error_on_node(
                    body,
                    diag::An_implementation_cannot_be_declared_in_ambient_contexts,
                    args![],
                );
            }
        }

        // PORT: Go reads `accessor.FunctionLikeData().TypeParameters`; accessors
        // always have function-like data, and `type_parameter_list()` (Go
        // `Node.TypeParameterList()`) returns that same list.
        let type_parameters = accessor.type_parameter_list();

        if !type_parameters.is_nil() {
            return self.grammar_error_on_node(
                accessor.name(),
                diag::An_accessor_cannot_have_type_parameters,
                args![],
            );
        }
        if !self.does_accessor_have_correct_parameter_count(accessor) {
            return self.grammar_error_on_node(
                accessor.name(),
                if accessor.kind() == SyntaxKind::GetAccessor {
                    diag::A_get_accessor_cannot_have_parameters
                } else {
                    diag::A_set_accessor_must_have_exactly_one_parameter
                },
                args![],
            );
        }
        if accessor.kind() == SyntaxKind::SetAccessor {
            // PORT: Go `funcData.Type` is the accessor's `Type` field (`type_()`).
            if accessor.type_().is_some() {
                return self.grammar_error_on_node(
                    accessor.name(),
                    diag::A_set_accessor_cannot_have_a_return_type_annotation,
                    args![],
                );
            }

            let parameter_node = get_set_accessor_value_parameter(accessor);
            if parameter_node.is_nil() {
                panic!("Return value does not match parameter count assertion.");
            }
            let parameter = parameter_node;
            if parameter.dot_dot_dot_token().is_some() {
                return self.grammar_error_on_node(
                    parameter.dot_dot_dot_token(),
                    diag::A_set_accessor_cannot_have_rest_parameter,
                    args![],
                );
            }
            if parameter.question_token().is_some() {
                return self.grammar_error_on_node(
                    parameter.question_token(),
                    diag::A_set_accessor_cannot_have_an_optional_parameter,
                    args![],
                );
            }
            if parameter.initializer().is_some() {
                return self.grammar_error_on_node(
                    accessor.name(),
                    diag::A_set_accessor_parameter_cannot_have_an_initializer,
                    args![],
                );
            }
        }

        false
    }

    // Go: checker/grammarchecks.go:1345 doesAccessorHaveCorrectParameterCount
    // Does the accessor have the right number of parameters?
    //
    //	A `get` accessor has no parameters or a single `this` parameter.
    //	A `set` accessor has one parameter or a `this` parameter and one more parameter.
    pub fn does_accessor_have_correct_parameter_count(&self, accessor: Node) -> bool {
        // `getAccessorThisParameter` returns `nil` if the accessor's arity is incorrect,
        // even if there is a `this` parameter declared.
        self.get_accessor_this_parameter(accessor).is_some()
            || accessor.parameters().len()
                == if accessor.kind() == SyntaxKind::GetAccessor {
                    0
                } else {
                    1
                }
    }

    // Go: checker/grammarchecks.go:1351 checkGrammarTypeOperatorNode
    pub fn check_grammar_type_operator_node(&mut self, node: Node) -> bool {
        if node.operator() == SyntaxKind::UniqueKeyword {
            let inner_type = node.type_();
            if inner_type.kind() != SyntaxKind::SymbolKeyword {
                return self.grammar_error_on_node(
                    inner_type,
                    diag::X_0_expected,
                    args![token_to_string(SyntaxKind::SymbolKeyword)],
                );
            }
            let parent = walk_up_parenthesized_types(node.parent());
            match parent.kind() {
                SyntaxKind::VariableDeclaration => {
                    let decl = parent;
                    if decl.name().kind() != SyntaxKind::Identifier {
                        return self.grammar_error_on_node(
                            node,
                            diag::X_unique_symbol_types_may_not_be_used_on_a_variable_declaration_with_a_binding_name,
                            args![],
                        );
                    }
                    if !is_variable_declaration_in_variable_statement(decl) {
                        return self.grammar_error_on_node(
                            node,
                            diag::X_unique_symbol_types_are_only_allowed_on_variables_in_a_variable_statement,
                            args![],
                        );
                    }
                    if !decl.parent().flags().intersects(NodeFlags::CONST) {
                        return self.grammar_error_on_node(
                            parent.name(),
                            diag::A_variable_whose_type_is_a_unique_symbol_type_must_be_const,
                            args![],
                        );
                    }
                }
                SyntaxKind::PropertyDeclaration => {
                    if !is_static(parent) || !has_readonly_modifier(parent) {
                        return self.grammar_error_on_node(
                            parent.name(),
                            diag::A_property_of_a_class_whose_type_is_a_unique_symbol_type_must_be_both_static_and_readonly,
                            args![],
                        );
                    }
                }
                SyntaxKind::PropertySignature => {
                    if !has_syntactic_modifier(parent, ModifierFlags::READONLY) {
                        return self.grammar_error_on_node(
                            parent.name(),
                            diag::A_property_of_an_interface_or_type_literal_whose_type_is_a_unique_symbol_type_must_be_readonly,
                            args![],
                        );
                    }
                }
                _ => {
                    return self.grammar_error_on_node(
                        node,
                        diag::X_unique_symbol_types_are_not_allowed_here,
                        args![],
                    );
                }
            }
        } else if node.operator() == SyntaxKind::ReadonlyKeyword {
            let inner_type = node.type_();
            if inner_type.kind() != SyntaxKind::ArrayType
                && inner_type.kind() != SyntaxKind::TupleType
            {
                return self.grammar_error_on_first_token(
                    node,
                    diag::X_readonly_type_modifier_is_only_permitted_on_array_and_tuple_literal_types,
                    args![token_to_string(SyntaxKind::SymbolKeyword)],
                );
            }
        }

        false
    }

    // Go: checker/grammarchecks.go:1391 checkGrammarForInvalidDynamicName
    pub fn check_grammar_for_invalid_dynamic_name(
        &mut self,
        node: Node,
        message: &'static Message,
    ) -> bool {
        if !self.is_non_bindable_dynamic_name(node) {
            return false;
        }
        let expression = if is_element_access_expression(node) {
            skip_parentheses(node.argument_expression())
        } else {
            node.expression()
        };

        if !is_entity_name_expression(expression) {
            return self.grammar_error_on_node(node, message, args![]);
        }

        false
    }

    // Go: checker/grammarchecks.go:1410 isNonBindableDynamicName
    // Indicates whether a declaration name is a dynamic name that cannot be late-bound.
    pub fn is_non_bindable_dynamic_name(&mut self, node: Node) -> bool {
        is_dynamic_name(node) && !self.is_late_bindable_name(node)
    }

    // Go: checker/grammarchecks.go:1414 checkGrammarMethod
    pub fn check_grammar_method(
        &mut self,
        node: Node, /*Union[MethodDeclaration, MethodSignature]*/
    ) -> bool {
        if self.check_grammar_function_like_declaration(node) {
            return true;
        }

        if node.kind() == SyntaxKind::MethodDeclaration {
            if node.parent().kind() == SyntaxKind::ObjectLiteralExpression {
                // We only disallow modifier on a method declaration if it is a property of object-literal-expression
                let modifiers = node.modifiers();
                if !modifiers.is_nil()
                    && !(modifiers.nodes().len() == 1
                        && modifiers.nodes().get(0).kind() == SyntaxKind::AsyncKeyword)
                {
                    return self.grammar_error_on_first_token(
                        node,
                        diag::Modifiers_cannot_appear_here,
                        args![],
                    );
                }

                let postfix_token = node.postfix_token();
                if self.check_grammar_for_invalid_question_mark(
                    postfix_token,
                    diag::An_object_member_cannot_be_declared_optional,
                ) {
                    return true;
                }
                if self.check_grammar_for_invalid_exclamation_token(
                    postfix_token,
                    diag::A_definite_assignment_assertion_is_not_permitted_in_this_context,
                ) {
                    return true;
                }
                if node.body().is_nil() {
                    return self.grammar_error_at_pos(
                        node,
                        node.end() - 1,
                        ";".len() as i32,
                        diag::X_0_expected,
                        args!["{"],
                    );
                }
            }
            if self.check_grammar_for_generator(node) {
                return true;
            }
        }

        if is_class_like(node.parent()) {
            // Technically, computed properties in ambient contexts is disallowed
            // for property declarations and accessors too, not just methods.
            // However, property declarations disallow computed names in general,
            // and accessors are not allowed in ambient contexts in general,
            // so this error only really matters for methods.
            if node.flags().intersects(NodeFlags::AMBIENT) {
                return self.check_grammar_for_invalid_dynamic_name(
                    node.name(),
                    diag::A_computed_property_name_in_an_ambient_context_must_refer_to_an_expression_whose_type_is_a_literal_type_or_a_unique_symbol_type,
                );
            } else if node.kind() == SyntaxKind::MethodDeclaration && node.body().is_nil() {
                return self.check_grammar_for_invalid_dynamic_name(
                    node.name(),
                    diag::A_computed_property_name_in_a_method_overload_must_refer_to_an_expression_whose_type_is_a_literal_type_or_a_unique_symbol_type,
                );
            }
        } else if node.parent().kind() == SyntaxKind::InterfaceDeclaration {
            return self.check_grammar_for_invalid_dynamic_name(
                node.name(),
                diag::A_computed_property_name_in_an_interface_must_refer_to_an_expression_whose_type_is_a_literal_type_or_a_unique_symbol_type,
            );
        } else if node.parent().kind() == SyntaxKind::TypeLiteral {
            return self.check_grammar_for_invalid_dynamic_name(
                node.name(),
                diag::A_computed_property_name_in_a_type_literal_must_refer_to_an_expression_whose_type_is_a_literal_type_or_a_unique_symbol_type,
            );
        }

        false
    }

    // Go: checker/grammarchecks.go:1462 checkGrammarBreakOrContinueStatement
    pub fn check_grammar_break_or_continue_statement(&mut self, node: Node) -> bool {
        let target_label = node.label();
        let mut current = node;
        while current.is_some() {
            if is_function_like_or_class_static_block_declaration(current) {
                return self.grammar_error_on_node(
                    node,
                    diag::Jump_target_cannot_cross_function_boundary,
                    args![],
                );
            }

            match current.kind() {
                SyntaxKind::LabeledStatement => {
                    if target_label.is_some() && current.label().text() == target_label.text() {
                        // found matching label - verify that label usage is correct
                        // continue can only target labels that are on iteration statements
                        let is_misplaced_continue_label = node.kind()
                            == SyntaxKind::ContinueStatement
                            && !is_iteration_statement(
                                current.statement(),
                                true, /*lookInLabeledStatements*/
                            );

                        if is_misplaced_continue_label {
                            return self.grammar_error_on_node(
                                node,
                                diag::A_continue_statement_can_only_jump_to_a_label_of_an_enclosing_iteration_statement,
                                args![],
                            );
                        }

                        return false;
                    }
                }
                SyntaxKind::SwitchStatement => {
                    if node.kind() == SyntaxKind::BreakStatement && target_label.is_nil() {
                        // unlabeled break within switch statement - ok
                        return false;
                    }
                }
                _ => {
                    if is_iteration_statement(current, false /*lookInLabeledStatements*/)
                        && target_label.is_nil()
                    {
                        // unlabeled break or continue within iteration statement - ok
                        return false;
                    }
                }
            }

            current = current.parent();
        }

        if target_label.is_some() {
            let message = if node.kind() == SyntaxKind::BreakStatement {
                diag::A_break_statement_can_only_jump_to_a_label_of_an_enclosing_statement
            } else {
                diag::A_continue_statement_can_only_jump_to_a_label_of_an_enclosing_iteration_statement
            };

            self.grammar_error_on_node(node, message, args![])
        } else {
            let message = if node.kind() == SyntaxKind::BreakStatement {
                diag::A_break_statement_can_only_be_used_within_an_enclosing_iteration_or_switch_statement
            } else {
                diag::A_continue_statement_can_only_be_used_within_an_enclosing_iteration_statement
            };
            self.grammar_error_on_node(node, message, args![])
        }
    }

    // Go: checker/grammarchecks.go:1518 checkGrammarBindingElement
    pub fn check_grammar_binding_element(&mut self, node: Node) -> bool {
        if node.dot_dot_dot_token().is_some() {
            let elements = node.parent().element_list();
            // PORT: Go `core.LastOrNil(elements.Nodes)`.
            let element_nodes = elements.nodes();
            let last = if element_nodes.is_empty() {
                Node::NIL
            } else {
                element_nodes.get(element_nodes.len() - 1)
            };
            if node != last {
                return self.grammar_error_on_node(
                    node,
                    diag::A_rest_element_must_be_last_in_a_destructuring_pattern,
                    args![],
                );
            }
            self.check_grammar_for_disallowed_trailing_comma(
                elements,
                diag::A_rest_parameter_or_binding_pattern_may_not_have_a_trailing_comma,
            );

            if node.property_name().is_some() {
                return self.grammar_error_on_node(
                    node.name(),
                    diag::A_rest_element_cannot_have_a_property_name,
                    args![],
                );
            }
        }

        if node.dot_dot_dot_token().is_some() && node.initializer().is_some() {
            // Error on equals token which immediately precedes the initializer
            return self.grammar_error_at_pos(
                node,
                node.initializer().pos() - 1,
                1,
                diag::A_rest_element_cannot_have_an_initializer,
                args![],
            );
        }

        false
    }

    // Go: checker/grammarchecks.go:1539 checkGrammarVariableDeclaration
    pub fn check_grammar_variable_declaration(&mut self, node: Node) -> bool {
        let node_flags = self.get_combined_node_flags_cached(node);
        let block_scope_kind = node_flags & NodeFlags::BLOCK_SCOPED;
        if is_binding_pattern(node.name()) {
            if block_scope_kind == NodeFlags::AWAIT_USING {
                return self.grammar_error_on_node(
                    node,
                    diag::X_0_declarations_may_not_have_binding_patterns,
                    args!["await using"],
                );
            } else if block_scope_kind == NodeFlags::USING {
                return self.grammar_error_on_node(
                    node,
                    diag::X_0_declarations_may_not_have_binding_patterns,
                    args!["using"],
                );
            }
        }

        if node.parent().parent().kind() != SyntaxKind::ForInStatement
            && node.parent().parent().kind() != SyntaxKind::ForOfStatement
        {
            if node_flags.intersects(NodeFlags::AMBIENT) {
                self.check_ambient_initializer(node);
            } else if node.initializer().is_nil() {
                if is_binding_pattern(node.name()) && !is_binding_pattern(node.parent()) {
                    return self.grammar_error_on_node(
                        node,
                        diag::A_destructuring_declaration_must_have_an_initializer,
                        args![],
                    );
                }
                if block_scope_kind == NodeFlags::AWAIT_USING {
                    return self.grammar_error_on_node(
                        node,
                        diag::X_0_declarations_must_be_initialized,
                        args!["await using"],
                    );
                } else if block_scope_kind == NodeFlags::USING {
                    return self.grammar_error_on_node(
                        node,
                        diag::X_0_declarations_must_be_initialized,
                        args!["using"],
                    );
                } else if block_scope_kind == NodeFlags::CONST {
                    return self.grammar_error_on_node(
                        node,
                        diag::X_0_declarations_must_be_initialized,
                        args!["const"],
                    );
                }
            }
        }

        let exclamation_token = node.exclamation_token();
        if exclamation_token.is_some()
            && (node.parent().parent().kind() != SyntaxKind::VariableStatement
                || node.type_().is_nil()
                || node.initializer().is_some()
                || node_flags.intersects(NodeFlags::AMBIENT))
        {
            let message = if node.initializer().is_some() {
                diag::Declarations_with_initializers_cannot_also_have_definite_assignment_assertions
            } else if node.type_().is_nil() {
                diag::Declarations_with_definite_assignment_assertions_must_also_have_type_annotations
            } else {
                diag::A_definite_assignment_assertion_is_not_permitted_in_this_context
            };
            return self.grammar_error_on_node(exclamation_token, message, args![]);
        }

        if get_emit_module_format_of_file(get_source_file_of_node(node)) < ModuleKind::SYSTEM
            && !node
                .parent()
                .parent()
                .flags()
                .intersects(NodeFlags::AMBIENT)
            && has_syntactic_modifier(node.parent().parent(), ModifierFlags::EXPORT)
        {
            self.check_grammar_for_es_module_marker_in_binding_name(node.name());
        }

        // 1. LexicalDeclaration : LetOrConst BindingList ;
        // It is a Syntax Error if the BoundNames of BindingList contains "let".
        // 2. ForDeclaration: ForDeclaration : LetOrConst ForBinding
        // It is a Syntax Error if the BoundNames of ForDeclaration contains "let".

        // It is a SyntaxError if a VariableDeclaration or VariableDeclarationNoIn occurs within strict code
        // and its Identifier is eval or arguments
        !block_scope_kind.is_empty()
            && self.check_grammar_name_in_let_or_const_declarations(node.name())
    }

    // Go: checker/grammarchecks.go:1596 checkGrammarForEsModuleMarkerInBindingName
    pub fn check_grammar_for_es_module_marker_in_binding_name(&mut self, name: Node) -> bool {
        if is_identifier(name) {
            if name.text() == "__esModule" {
                return self.grammar_error_on_node_skipped_on_no_emit(
                    name,
                    diag::Identifier_expected_esModule_is_reserved_as_an_exported_marker_when_transforming_ECMAScript_modules,
                    args![],
                );
            }
        } else {
            for element in name.elements().iter() {
                if element.name().is_some() {
                    return self.check_grammar_for_es_module_marker_in_binding_name(element.name());
                }
            }
        }
        false
    }

    // Go: checker/grammarchecks.go:1611 checkGrammarNameInLetOrConstDeclarations
    pub fn check_grammar_name_in_let_or_const_declarations(
        &mut self,
        name: Node, /*Union[Identifier, BindingPattern]*/
    ) -> bool {
        if name.kind() == SyntaxKind::Identifier {
            if name.text() == "let" {
                return self.grammar_error_on_node(
                    name,
                    diag::X_let_is_not_allowed_to_be_used_as_a_name_in_let_or_const_declarations,
                    args![],
                );
            }
        } else {
            let elements = name.elements();
            for element in elements.iter() {
                let binding_element = element;
                if binding_element.name().is_some() {
                    self.check_grammar_name_in_let_or_const_declarations(binding_element.name());
                }
            }
        }
        false
    }

    // Go: checker/grammarchecks.go:1628 checkGrammarVariableDeclarationList
    pub fn check_grammar_variable_declaration_list(&mut self, declaration_list: Node) -> bool {
        let declarations = declaration_list.declarations();
        if self.check_grammar_for_disallowed_trailing_comma(
            declarations,
            diag::Trailing_comma_not_allowed,
        ) {
            return true;
        }

        if declarations.nodes().len() == 0 {
            return self.grammar_error_at_pos(
                declaration_list,
                declarations.pos(),
                declarations.end() - declarations.pos(),
                diag::Variable_declaration_list_cannot_be_empty,
                args![],
            );
        }

        let block_scope_flags = declaration_list.flags() & NodeFlags::BLOCK_SCOPED;
        if block_scope_flags == NodeFlags::USING || block_scope_flags == NodeFlags::AWAIT_USING {
            if is_for_in_statement(declaration_list.parent()) {
                return self.grammar_error_on_node(
                    declaration_list,
                    if block_scope_flags == NodeFlags::USING {
                        diag::The_left_hand_side_of_a_for_in_statement_cannot_be_a_using_declaration
                    } else {
                        diag::The_left_hand_side_of_a_for_in_statement_cannot_be_an_await_using_declaration
                    },
                    args![],
                );
            }
            if declaration_list.flags().intersects(NodeFlags::AMBIENT) {
                return self.grammar_error_on_node(
                    declaration_list,
                    if block_scope_flags == NodeFlags::USING {
                        diag::X_using_declarations_are_not_allowed_in_ambient_contexts
                    } else {
                        diag::X_await_using_declarations_are_not_allowed_in_ambient_contexts
                    },
                    args![],
                );
            }
            if is_variable_statement(declaration_list.parent())
                && (is_case_clause(declaration_list.parent().parent())
                    || is_default_clause(declaration_list.parent().parent()))
            {
                return self.grammar_error_on_node(
                    declaration_list,
                    if block_scope_flags == NodeFlags::USING {
                        diag::X_using_declarations_are_not_allowed_in_case_or_default_clauses_unless_contained_within_a_block
                    } else {
                        diag::X_await_using_declarations_are_not_allowed_in_case_or_default_clauses_unless_contained_within_a_block
                    },
                    args![],
                );
            }
        }

        if block_scope_flags == NodeFlags::AWAIT_USING {
            return self.check_grammar_await_or_await_using(declaration_list);
        }

        false
    }

    // Go: checker/grammarchecks.go:1658 checkGrammarAwaitOrAwaitUsing
    pub fn check_grammar_await_or_await_using(&mut self, node: Node) -> bool {
        // Grammar checking
        let mut has_error = false;
        let container = get_containing_function_or_class_static_block(node);
        if container.is_some() && is_class_static_block_declaration(container) {
            // NOTE: We report this regardless as to whether there are parse diagnostics.
            let message = if is_await_expression(node) {
                diag::X_await_expression_cannot_be_used_inside_a_class_static_block
            } else {
                diag::X_await_using_statements_cannot_be_used_inside_a_class_static_block
            };
            self.error(node, message, args![]);
            has_error = true;
        } else if !node.flags().intersects(NodeFlags::AWAIT_CONTEXT) {
            if is_in_top_level_context(node) {
                let source_file = get_source_file_of_node(node);
                if !self.has_parse_diagnostics(source_file) {
                    // PORT: Go zero value `core.TextRange{}`.
                    let mut span = TextRange::new(0, 0);
                    let mut span_calculated = false;
                    if !is_effective_external_module(source_file, self.compiler_options) {
                        span = get_range_of_token_at_position(source_file, node.pos());
                        span_calculated = true;
                        let message = if is_await_expression(node) {
                            diag::X_await_expressions_are_only_allowed_at_the_top_level_of_a_file_when_that_file_is_a_module_but_this_file_has_no_imports_or_exports_Consider_adding_an_empty_export_to_make_this_file_a_module
                        } else {
                            diag::X_await_using_statements_are_only_allowed_at_the_top_level_of_a_file_when_that_file_is_a_module_but_this_file_has_no_imports_or_exports_Consider_adding_an_empty_export_to_make_this_file_a_module
                        };
                        let diagnostic = new_diagnostic(source_file, span, message, args![]);
                        self.add_diagnostic(diagnostic);
                        has_error = true;
                    }
                    // PORT: Go `switch` with `fallthrough`. `report_default` is true
                    // when control reaches the `default` case.
                    let mut report_default = false;
                    match self.module_kind {
                        ModuleKind::NODE16
                        | ModuleKind::NODE18
                        | ModuleKind::NODE20
                        | ModuleKind::NODE_NEXT => {
                            let source_file_meta_data =
                                get_source_file_meta_data(&source_file_info(source_file).path);
                            if source_file_meta_data.implied_node_format == ModuleKind::COMMON_JS {
                                if !span_calculated {
                                    span = get_range_of_token_at_position(source_file, node.pos());
                                }
                                self.add_diagnostic(new_diagnostic(
                                    source_file,
                                    span,
                                    diag::The_current_file_is_a_CommonJS_module_and_cannot_use_await_at_the_top_level,
                                    args![],
                                ));
                                has_error = true;
                            } else if self.language_version < ScriptTarget::ES2017 {
                                // fallthrough, fallthrough
                                report_default = true;
                            }
                        }
                        ModuleKind::ES2022
                        | ModuleKind::ES_NEXT
                        | ModuleKind::PRESERVE
                        | ModuleKind::SYSTEM => {
                            if self.language_version < ScriptTarget::ES2017 {
                                // fallthrough
                                report_default = true;
                            }
                        }
                        _ => {
                            report_default = true;
                        }
                    }
                    if report_default {
                        if !span_calculated {
                            span = get_range_of_token_at_position(source_file, node.pos());
                        }
                        let message = if is_await_expression(node) {
                            diag::Top_level_await_expressions_are_only_allowed_when_the_module_option_is_set_to_es2022_esnext_system_node16_node18_node20_nodenext_or_preserve_and_the_target_option_is_set_to_es2017_or_higher
                        } else {
                            diag::Top_level_await_using_statements_are_only_allowed_when_the_module_option_is_set_to_es2022_esnext_system_node16_node18_node20_nodenext_or_preserve_and_the_target_option_is_set_to_es2017_or_higher
                        };
                        self.add_diagnostic(new_diagnostic(source_file, span, message, args![]));
                        has_error = true;
                    }
                }
            } else {
                // use of 'await' in non-async function
                let source_file = get_source_file_of_node(node);
                if !self.has_parse_diagnostics(source_file) {
                    let span = get_range_of_token_at_position(source_file, node.pos());
                    let message = if is_await_expression(node) {
                        diag::X_await_expressions_are_only_allowed_within_async_functions_and_at_the_top_levels_of_modules
                    } else {
                        diag::X_await_using_statements_are_only_allowed_within_async_functions_and_at_the_top_levels_of_modules
                    };
                    let mut diagnostic = new_diagnostic(source_file, span, message, args![]);
                    if container.is_some()
                        && container.kind() != SyntaxKind::Constructor
                        && !has_async_modifier(container)
                    {
                        let related_info = new_diagnostic_for_node(
                            container,
                            diag::Did_you_mean_to_mark_this_function_as_async,
                            args![],
                        );
                        diagnostic.add_related_info(Some(related_info));
                    }
                    self.add_diagnostic(diagnostic);
                    has_error = true;
                }
            }
        }

        if is_await_expression(node)
            && self.is_in_parameter_initializer_before_containing_function(node)
        {
            // NOTE: We report this regardless as to whether there are parse diagnostics.
            self.error(
                node,
                diag::X_await_expressions_cannot_be_used_in_a_parameter_initializer,
                args![],
            );
            has_error = true;
        }

        has_error
    }

    // Go: checker/grammarchecks.go:1759 checkGrammarYieldExpression
    pub fn check_grammar_yield_expression(&mut self, node: Node) -> bool {
        let mut has_error = false;
        if !node.flags().intersects(NodeFlags::YIELD_CONTEXT) {
            self.grammar_error_on_first_token(
                node,
                diag::A_yield_expression_is_only_allowed_in_a_generator_body,
                args![],
            );
            has_error = true;
        }
        if self.is_in_parameter_initializer_before_containing_function(node) {
            self.error(
                node,
                diag::X_yield_expressions_cannot_be_used_in_a_parameter_initializer,
                args![],
            );
            has_error = true;
        }
        has_error
    }

    // Go: checker/grammarchecks.go:1772 checkGrammarForDisallowedBlockScopedVariableStatement
    pub fn check_grammar_for_disallowed_block_scoped_variable_statement(
        &mut self,
        node: Node,
    ) -> bool {
        if !self.container_allows_block_scoped_variable(node.parent()) {
            let block_scope_kind = self.get_combined_node_flags_cached(node.declaration_list())
                & NodeFlags::BLOCK_SCOPED;
            if !block_scope_kind.is_empty() {
                let keyword = if block_scope_kind == NodeFlags::LET {
                    "let"
                } else if block_scope_kind == NodeFlags::CONST {
                    "const"
                } else if block_scope_kind == NodeFlags::USING {
                    "using"
                } else if block_scope_kind == NodeFlags::AWAIT_USING {
                    "await using"
                } else {
                    panic!("Unknown BlockScope flag")
                };
                self.error(
                    node,
                    diag::X_0_declarations_can_only_be_declared_inside_a_block,
                    args![keyword],
                );
            }
        }

        false
    }

    // Go: checker/grammarchecks.go:1796 containerAllowsBlockScopedVariable
    pub fn container_allows_block_scoped_variable(&self, parent: Node) -> bool {
        match parent.kind() {
            SyntaxKind::IfStatement
            | SyntaxKind::DoStatement
            | SyntaxKind::WhileStatement
            | SyntaxKind::WithStatement
            | SyntaxKind::ForStatement
            | SyntaxKind::ForInStatement
            | SyntaxKind::ForOfStatement => return false,
            SyntaxKind::LabeledStatement => {
                return self.container_allows_block_scoped_variable(parent.parent());
            }
            _ => {}
        }

        true
    }

    // Go: checker/grammarchecks.go:1813 checkGrammarMetaProperty
    pub fn check_grammar_meta_property(&mut self, node: Node) -> bool {
        let node_name = node.name();
        let name_text = node_name.text().to_string();

        match node.keyword_token() {
            SyntaxKind::NewKeyword => {
                if name_text != "target" {
                    return self.grammar_error_on_node(
                        node_name,
                        diag::X_0_is_not_a_valid_meta_property_for_keyword_1_Did_you_mean_2,
                        args![name_text, token_to_string(node.keyword_token()), "target"],
                    );
                }
            }
            SyntaxKind::ImportKeyword => {
                if name_text != "meta" {
                    let is_callee =
                        is_call_expression(node.parent()) && node.parent().expression() == node;
                    // ts#63915, Go N' grammarchecks.go:1825: `defer` and `source`
                    if is_import_phase_meta_property(node) {
                        if !is_callee {
                            return self.grammar_error_at_pos(
                                node,
                                node.end(),
                                0,
                                diag::X_0_expected,
                                args!["("],
                            );
                        }
                    } else {
                        if is_callee {
                            return self.grammar_error_on_node(
                                node_name,
                                diag::X_0_is_not_a_valid_meta_property_for_keyword_import_Did_you_mean_meta_defer_or_source,
                                args![name_text],
                            );
                        }
                        return self.grammar_error_on_node(
                            node_name,
                            diag::X_0_is_not_a_valid_meta_property_for_keyword_1_Did_you_mean_2,
                            args![name_text, token_to_string(node.keyword_token()), "meta"],
                        );
                    }
                }
            }
            _ => {}
        }

        false
    }
}
