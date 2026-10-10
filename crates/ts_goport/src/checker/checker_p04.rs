//! Port of Go `checker/checker.go` lines 2968-3873 (unit checker-04):
//! type node checks, function declaration checks, overload agreement checks,
//! block and if statement checks, and known-truthy condition checks.

use crate::diagnostics::Message;
use crate::prelude::*;

// PORT: Go `core.OrElse(a, b)` for node handles.
fn or_else_node_p04(a: Node, b: Node) -> Node {
    if a.is_some() { a } else { b }
}

// PORT: Go `core.OrElse(a, b)` for symbol handles.
fn or_else_symbol_p04(a: SymbolId, b: SymbolId) -> SymbolId {
    if a.is_some() { a } else { b }
}

// PORT: Go `core.ElementOrNil(slice, i)`.
fn element_or_nil_p04(nodes: &[Node], i: usize) -> Node {
    if i < nodes.len() { nodes[i] } else { Node::NIL }
}

impl Checker {
    // Go: checker/checker.go:3028 checkTypeReferenceNode
    pub fn check_type_reference_node(&mut self, node: Node) {
        self.check_grammar_type_arguments(node, node.type_argument_list());
        if is_type_reference_node(node) && !node.flags().intersects(NodeFlags::JS_DOC) {
            let type_arguments = node.type_argument_list();
            let type_name = node.type_name();
            if !type_arguments.is_nil() && type_name.end() != type_arguments.pos() {
                // If there was a token between the type name and the type arguments, check if it was a DotToken
                let source_file = get_source_file_of_node(node);
                if scan_token_at_position(source_file, type_name.end()) == SyntaxKind::DotToken {
                    self.grammar_error_at_pos(
                        node,
                        skip_trivia(&source_file_text(source_file), type_name.end()),
                        1,
                        diag::JSDoc_types_can_only_be_used_inside_documentation_comments,
                        args![],
                    );
                }
            }
        }
        self.check_source_elements(node.type_arguments());
        if !(is_const_type_reference(node) && is_assertion_expression(node.parent())) {
            self.check_type_reference_or_import(node);
        }
    }

    // Go: checker/checker.go:3046 checkTypeReferenceOrImport
    pub fn check_type_reference_or_import(&mut self, node: Node) {
        let t = self.get_type_from_type_node(node);
        if !self.is_error_type(t) {
            if node.type_arguments().len() != 0 {
                let type_parameters = self.get_type_parameters_for_type_reference_or_import(node);
                if !type_parameters.is_empty() {
                    self.check_type_argument_constraints(node, &type_parameters);
                }
            }
            let symbol = self.get_resolved_symbol_or_nil(node);
            if symbol.is_some() {
                let declarations = self.sym(symbol).declarations.clone();
                let mut some = false;
                for &d in &declarations {
                    if is_type_declaration(d) && self.is_deprecated_declaration(d) {
                        some = true;
                        break;
                    }
                }
                if some {
                    let name = self.sym(symbol).name.clone();
                    let suggestion_node = self.get_deprecated_suggestion_node(node);
                    self.add_deprecated_suggestion(suggestion_node, &declarations, &name);
                }
            }
        }
    }

    // Go: checker/checker.go:3064 checkTypeArgumentConstraints
    pub fn check_type_argument_constraints(
        &mut self,
        node: Node,
        type_parameters: &[TypeId],
    ) -> bool {
        // PORT: Go nil slice `typeArguments` is `None` until first computed.
        let mut type_arguments: Option<Vec<TypeId>> = None;
        let mut mapper = MapperId::NIL;
        let mut result = true;
        for (i, &type_parameter) in type_parameters.iter().enumerate() {
            let constraint = self.get_constraint_of_type_parameter(type_parameter);
            if constraint.is_some() {
                if type_arguments.is_none() {
                    let args = self.get_effective_type_arguments(node, type_parameters);
                    mapper = self.new_type_mapper(type_parameters, &args);
                    type_arguments = Some(args);
                }
                // Go short-circuits: `result = result && check(...)`.
                if result {
                    let type_argument = type_arguments.as_ref().unwrap()[i];
                    let instantiated = self.instantiate_type(constraint, mapper);
                    let error_node = element_or_nil_p04(&node.type_arguments().to_vec(), i);
                    result = self.check_type_assignable_to(
                        type_argument,
                        instantiated,
                        error_node,
                        Some(diag::Type_0_does_not_satisfy_the_constraint_1),
                    );
                }
            }
        }
        result
    }

    // Go: checker/checker.go:3081 getDeprecatedSuggestionNode
    pub fn get_deprecated_suggestion_node(&mut self, node: Node) -> Node {
        let node = skip_parentheses(node);
        match node.kind() {
            SyntaxKind::CallExpression | SyntaxKind::Decorator | SyntaxKind::NewExpression => {
                return self.get_deprecated_suggestion_node(node.expression());
            }
            SyntaxKind::TaggedTemplateExpression => {
                return self.get_deprecated_suggestion_node(node.tag());
            }
            SyntaxKind::JsxOpeningElement | SyntaxKind::JsxSelfClosingElement => {
                return self.get_deprecated_suggestion_node(node.tag_name());
            }
            SyntaxKind::ElementAccessExpression => {
                return node.argument_expression();
            }
            SyntaxKind::PropertyAccessExpression => {
                return node.name();
            }
            SyntaxKind::TypeReference => {
                let type_name = node.type_name();
                if is_qualified_name(type_name) {
                    return type_name.right();
                }
            }
            _ => {}
        }
        node
    }

    // Go: checker/checker.go:3103 checkTypePredicate
    pub fn check_type_predicate(&mut self, node: Node) {
        // Always check the predicate's type so nested type errors are reported even when the
        // predicate is in an invalid position, keeping diagnostics stable.
        self.check_source_element(node.type_());
        let parent = self.get_type_predicate_parent(node);
        if parent.is_nil() {
            // The parent must not be valid.
            self.error(node, diag::A_type_predicate_is_only_allowed_in_return_type_position_for_functions_and_methods, args![]);
            return;
        }
        let signature = self.get_signature_from_declaration(parent);
        let type_predicate = self.get_type_predicate_of_signature(signature);
        if type_predicate.is_nil() {
            return;
        }
        let parameter_name = node.parameter_name();
        let pred_kind = self.pred(type_predicate).kind;
        if pred_kind != TypePredicateKind::THIS && pred_kind != TypePredicateKind::ASSERTS_THIS {
            let parameter_index = self.pred(type_predicate).parameter_index;
            if parameter_index >= 0 {
                if self.signature_has_rest_parameter(signature)
                    && parameter_index as i32 == self.sig(signature).parameters.len() as i32 - 1
                {
                    self.error(
                        parameter_name,
                        diag::A_type_predicate_cannot_reference_a_rest_parameter,
                        args![],
                    );
                } else {
                    let pred_type = self.pred(type_predicate).t;
                    if pred_type.is_some() {
                        let mut diags: Vec<Diagnostic> = Vec::new();
                        let param = self.sig(signature).parameters[parameter_index as usize];
                        let param_type = self.get_type_of_symbol(param);
                        if !self.check_type_assignable_to_ex(
                            pred_type,
                            param_type,
                            node.type_(),
                            None, /*headMessage*/
                            Some(&mut diags),
                        ) {
                            self.add_diagnostic(new_diagnostic_chain(
                                Some(diags[0].clone()),
                                diag::A_type_predicate_s_type_must_be_assignable_to_its_parameter_s_type,
                                args![],
                            ));
                        }
                    }
                }
            } else if parameter_name.is_some() {
                let mut has_reported_error = false;
                let predicate_parameter_name = self.pred(type_predicate).parameter_name.clone();
                for param in parent.parameters() {
                    let name = param.name();
                    if is_binding_pattern(name)
                        && self.check_if_type_predicate_variable_is_declared_in_binding_pattern(
                            name,
                            parameter_name,
                            &predicate_parameter_name,
                        )
                    {
                        has_reported_error = true;
                        break;
                    }
                }
                if !has_reported_error {
                    self.error(
                        parameter_name,
                        diag::Cannot_find_parameter_0,
                        args![predicate_parameter_name],
                    );
                }
            }
        }
    }

    // Go: checker/checker.go:3147 getTypePredicateParent
    pub fn get_type_predicate_parent(&self, node: Node) -> Node {
        let parent = node.parent();
        match parent.kind() {
            SyntaxKind::ArrowFunction
            | SyntaxKind::CallSignature
            | SyntaxKind::FunctionDeclaration
            | SyntaxKind::FunctionExpression
            | SyntaxKind::FunctionType
            | SyntaxKind::MethodDeclaration
            | SyntaxKind::MethodSignature => {
                if node == parent.type_() {
                    return parent;
                }
            }
            _ => {}
        }
        Node::NIL
    }

    // Go: checker/checker.go:3159 checkIfTypePredicateVariableIsDeclaredInBindingPattern
    pub fn check_if_type_predicate_variable_is_declared_in_binding_pattern(
        &mut self,
        pattern: Node,
        predicate_variable_node: Node,
        predicate_variable_name: &str,
    ) -> bool {
        for element in pattern.elements() {
            let name = element.name();
            if name.is_nil() {
                continue;
            }
            if is_identifier(name) && name.text() == predicate_variable_name {
                self.error(
                    predicate_variable_node,
                    diag::A_type_predicate_cannot_reference_element_0_in_a_binding_pattern,
                    args![predicate_variable_name],
                );
                return true;
            }
            if is_array_binding_pattern(name) || is_object_binding_pattern(name) {
                if self.check_if_type_predicate_variable_is_declared_in_binding_pattern(
                    name,
                    predicate_variable_node,
                    predicate_variable_name,
                ) {
                    return true;
                }
            }
        }
        false
    }

    // Go: checker/checker.go:3178 checkTypeQuery
    pub fn check_type_query(&mut self, node: Node) {
        self.get_type_from_type_query_node(node);
    }

    // Go: checker/checker.go:3182 checkTypeLiteral
    pub fn check_type_literal(&mut self, node: Node) {
        self.check_source_elements(node.members());
        let t = self.get_type_from_type_literal_or_function_or_constructor_type_node(node);
        let symbol = self.ty(t).symbol;
        self.check_index_constraints(t, symbol, false /*isStaticIndex*/);
        self.check_type_for_duplicate_index_signatures(node);
        self.check_object_type_for_duplicate_declarations(node, false /*checkPrivateNames*/);
    }

    // Go: checker/checker.go:3190 checkObjectTypeForDuplicateDeclarations
    pub fn check_object_type_for_duplicate_declarations(
        &mut self,
        node: Node,
        check_private_names: bool,
    ) {
        // PORT: Go nil maps are `None` until first written.
        let mut instance_names: Option<FxHashMap<String, i32>> = None;
        let mut static_names: Option<FxHashMap<String, i32>> = None;
        let mut private_names: Option<FxHashMap<String, i32>> = None;
        let node_in_ambient_context = node.flags().intersects(NodeFlags::AMBIENT);
        // PORT: Go closure `checkPropertyOrAccessor` captures `c`, `node` and the
        // two name maps; here they are passed explicitly.
        fn check_property_or_accessor(
            c: &mut Checker,
            node: Node,
            instance_names: &mut Option<FxHashMap<String, i32>>,
            static_names: &mut Option<FxHashMap<String, i32>>,
            symbol: SymbolId,
            kind: i32,
            is_static: bool,
        ) {
            if c.sym(symbol).declarations.len() > 1 {
                let names = if is_static {
                    static_names.get_or_insert_with(FxHashMap::default)
                } else {
                    instance_names.get_or_insert_with(FxHashMap::default)
                };
                let symbol_name = c.sym(symbol).name.to_string();
                let state = names.get(&symbol_name).copied().unwrap_or(0);
                if state == 0 {
                    // On first occurrence just record the kind
                    names.insert(symbol_name, kind);
                } else if state == 1 || state == 2 && kind != 2 {
                    // Error on second property or combination of property and accessor
                    c.report_duplicate_member_errors(
                        node,
                        &symbol_name,
                        true,
                        is_static,
                        diag::Duplicate_identifier_0,
                    );
                    // Record that errors have been reported
                    names.insert(symbol_name, 3);
                }
            }
        }
        for member in node.members() {
            if is_constructor_declaration(member) {
                for param in member.parameters() {
                    if is_parameter_property_declaration(param, member)
                        && !is_binding_pattern(param.name())
                    {
                        let param_symbol = self.get_symbol_of_declaration(param);
                        check_property_or_accessor(
                            self,
                            node,
                            &mut instance_names,
                            &mut static_names,
                            param_symbol,
                            1,
                            false, /*isStatic*/
                        );
                    }
                }
            } else {
                let symbol = self.get_symbol_of_declaration(member);
                let is_static_member = has_static_modifier(member);
                // In non-ambient contexts, check that static members are not named 'prototype'.
                if !node_in_ambient_context
                    && is_static_member
                    && symbol.is_some()
                    && self.sym(symbol).name == "prototype"
                {
                    let symbol_name = self.sym(symbol).name.to_string();
                    let node_symbol = self.get_symbol_of_declaration(node);
                    let node_symbol_string = self.symbol_to_string(node_symbol);
                    self.error(
                        member.name(),
                        diag::Static_property_0_conflicts_with_built_in_property_Function_0_of_constructor_function_1,
                        args![symbol_name, node_symbol_string],
                    );
                }
                // Check that this object type declaration doesn't contain multiple declarations of the same property,
                // or accessor and property declarations with the same name.
                if is_property_declaration(member) && !has_accessor_modifier(member)
                    || is_property_signature_declaration(member)
                {
                    check_property_or_accessor(
                        self,
                        node,
                        &mut instance_names,
                        &mut static_names,
                        symbol,
                        1,
                        is_static_member,
                    );
                } else if is_accessor(member)
                    || is_property_declaration(member) && has_accessor_modifier(member)
                {
                    check_property_or_accessor(
                        self,
                        node,
                        &mut instance_names,
                        &mut static_names,
                        symbol,
                        2,
                        is_static_member,
                    );
                }
                // Check that each private identifier is used only for instance members or only for static members. It is an
                // error for an instance and a static member to have the same private identifier.
                if check_private_names
                    && member.name().is_some()
                    && is_private_identifier(member.name())
                {
                    let symbol_name = self.sym(symbol).name.to_string();
                    let mut flags = private_names
                        .as_ref()
                        .and_then(|m| m.get(&symbol_name).copied())
                        .unwrap_or(0);
                    if flags != 3 {
                        flags |= if is_static(member) { 2 } else { 1 };
                        private_names
                            .get_or_insert_with(FxHashMap::default)
                            .insert(symbol_name.clone(), flags);
                        if flags == 3 {
                            self.report_duplicate_member_errors(
                                node,
                                &symbol_name,
                                false,
                                false,
                                diag::Duplicate_identifier_0_Static_and_instance_elements_cannot_share_the_same_private_name,
                            );
                        }
                    }
                }
            }
        }
    }

    // Go: checker/checker.go:3261 reportDuplicateMemberErrors
    pub fn report_duplicate_member_errors(
        &mut self,
        node: Node,
        name: &str,
        check_static: bool,
        is_static_: bool,
        message: &'static Message,
    ) {
        for member in node.members() {
            if is_constructor_declaration(member) {
                for param in member.parameters() {
                    if is_parameter_property_declaration(param, member)
                        && !is_binding_pattern(param.name())
                    {
                        let symbol = self.get_symbol_of_declaration(param);
                        if self.sym(symbol).name == name {
                            let symbol_string = self.symbol_to_string(symbol);
                            self.error(param.name(), message, args![symbol_string]);
                        }
                    }
                }
            } else {
                let symbol = self.get_symbol_of_declaration(member);
                if symbol.is_some()
                    && self.sym(symbol).name == name
                    && (!check_static || is_static_ == is_static(member))
                {
                    let symbol_string = self.symbol_to_string(symbol);
                    self.error(member.name(), message, args![symbol_string]);
                }
            }
        }
    }

    // Go: checker/checker.go:3277 checkArrayType
    pub fn check_array_type(&mut self, node: Node) {
        self.check_source_element(node.element_type());
    }

    // Go: checker/checker.go:3281 checkTupleType
    pub fn check_tuple_type(&mut self, node: Node) {
        let mut seen_optional_element = false;
        let mut seen_rest_element = false;
        let elements = node.elements();
        for e in elements {
            let mut flags = self.get_tuple_element_flags(e);
            if flags.intersects(ElementFlags::VARIADIC) {
                let t = self.get_type_from_type_node(e.type_());
                if !self.is_array_like_type(t) {
                    self.error(e, diag::A_rest_element_type_must_be_an_array_type, args![]);
                    break;
                }
                // PORT: Go `t.TargetTupleType().combinedFlags` reads the tuple
                // data of the reference target; spelled out through the arena.
                let is_rest_tuple = self.is_tuple_type(t) && {
                    let target = self.ty(t).target();
                    self.ty(target)
                        .as_tuple_type()
                        .combined_flags
                        .intersects(ElementFlags::REST)
                };
                if self.is_array_type(t) || is_rest_tuple {
                    flags |= ElementFlags::REST;
                }
            }
            if flags.intersects(ElementFlags::REST) {
                if seen_rest_element {
                    self.grammar_error_on_node(
                        e,
                        diag::A_rest_element_cannot_follow_another_rest_element,
                        args![],
                    );
                    break;
                }
                seen_rest_element = true;
            } else if flags.intersects(ElementFlags::OPTIONAL) {
                if seen_rest_element {
                    self.grammar_error_on_node(
                        e,
                        diag::An_optional_element_cannot_follow_a_rest_element,
                        args![],
                    );
                    break;
                }
                seen_optional_element = true;
            } else if flags.intersects(ElementFlags::REQUIRED) && seen_optional_element {
                self.grammar_error_on_node(
                    e,
                    diag::A_required_element_cannot_follow_an_optional_element,
                    args![],
                );
                break;
            }
        }
        self.check_source_elements(elements);
        self.get_type_from_type_node(node);
    }

    // Go: checker/checker.go:3318 checkUnionOrIntersectionType
    pub fn check_union_or_intersection_type(&mut self, node: Node) {
        node.for_each_child(&mut |child: Node| self.check_source_element(child));
        self.get_type_from_type_node(node);
    }

    // Go: checker/checker.go:3323 checkThisType
    pub fn check_this_type(&mut self, node: Node) {
        self.get_type_from_this_type_node(node);
    }

    // Go: checker/checker.go:3327 checkTypeOperator
    pub fn check_type_operator(&mut self, node: Node) {
        self.check_grammar_type_operator_node(node);
        self.check_source_element(node.type_());
    }

    // Go: checker/checker.go:3332 checkConditionalType
    pub fn check_conditional_type(&mut self, node: Node) {
        node.for_each_child(&mut |child: Node| self.check_source_element(child));
    }

    // Go: checker/checker.go:3336 checkInferType
    pub fn check_infer_type(&mut self, node: Node) {
        if find_ancestor(node, |n: Node| {
            n.parent().is_some()
                && n.parent().kind() == SyntaxKind::ConditionalType
                && n.parent().extends_type() == n
        })
        .is_nil()
        {
            self.grammar_error_on_node(
                node,
                diag::X_infer_declarations_are_only_permitted_in_the_extends_clause_of_a_conditional_type,
                args![],
            );
        }
        let type_parameter_declaration_node = node.type_parameter();
        self.check_source_element(type_parameter_declaration_node);
        let symbol = self.get_symbol_of_declaration(type_parameter_declaration_node);
        if self.sym(symbol).declarations.len() > 1 {
            if !self.declared_type_links.get(symbol).type_parameters_checked {
                self.declared_type_links.get(symbol).type_parameters_checked = true;
                let type_parameter = self.get_declared_type_of_type_parameter(symbol);
                let declarations = self.get_declarations_of_kind(symbol, SyntaxKind::TypeParameter);
                if !self.are_type_parameters_identical(
                    &declarations,
                    &[type_parameter],
                    &mut |decl: Node| vec![decl],
                ) {
                    // Report an error on every conflicting declaration.
                    let name = self.symbol_to_string(symbol);
                    for &declaration in &declarations {
                        self.error(
                            declaration.name(),
                            diag::All_declarations_of_0_must_have_identical_constraints,
                            args![name],
                        );
                    }
                }
            }
        }
        self.register_for_unused_identifiers_check(node);
    }

    // Go: checker/checker.go:3363 checkTemplateLiteralType
    pub fn check_template_literal_type(&mut self, node: Node) {
        for span in node.template_spans().nodes() {
            self.check_source_element(span.type_());
            let t = self.get_type_from_type_node(span.type_());
            let template_constraint_type = self.template_constraint_type;
            self.check_type_assignable_to(t, template_constraint_type, span.type_(), None);
        }
        self.get_type_from_type_node(node);
    }

    // Go: checker/checker.go:3372 checkImportType
    pub fn check_import_type(&mut self, node: Node) {
        self.check_source_element(node.argument());
        let attributes = node.attributes();
        if attributes.is_some() {
            let import_attributes = attributes;
            self.check_grammar_import_attribute_values(import_attributes);
            self.get_resolution_mode_override(import_attributes, true /*reportErrors*/);
        }
        self.check_type_reference_or_import(node);
        self.check_import_attributes(node);
    }

    // Go: checker/checker.go:3383 getResolutionModeOverride
    // PORT: Go passes `c.grammarErrorOnNode` as the callback, or nil.
    pub fn get_resolution_mode_override(
        &mut self,
        node: Node,
        report_errors: bool,
    ) -> ResolutionMode {
        let (mode, _) = if report_errors {
            node.get_resolution_mode_override(Some(&mut |n: Node,
                                                         message: &'static Message,
                                                         args: Vec<String>|
             -> bool {
                self.grammar_error_on_node(n, message, args)
            }))
        } else {
            node.get_resolution_mode_override(None)
        };
        mode
    }

    // Go: checker/checker.go:3392 checkNamedTupleMember
    pub fn check_named_tuple_member(&mut self, node: Node) {
        let member_type = node.type_();
        if node.dot_dot_dot_token().is_some() && node.question_token().is_some() {
            self.grammar_error_on_node(
                node,
                diag::A_tuple_member_cannot_be_both_optional_and_rest,
                args![],
            );
        }
        if member_type.kind() == SyntaxKind::OptionalType {
            self.grammar_error_on_node(
                member_type,
                diag::A_labeled_tuple_element_is_declared_as_optional_with_a_question_mark_after_the_name_and_before_the_colon_rather_than_after_the_type,
                args![],
            );
        }
        if member_type.kind() == SyntaxKind::RestType {
            self.grammar_error_on_node(
                member_type,
                diag::A_labeled_tuple_element_is_declared_as_rest_with_a_before_the_name_rather_than_before_the_type,
                args![],
            );
        }
        self.check_source_element(node.type_());
        self.get_type_from_type_node(node);
    }

    // Go: checker/checker.go:3407 checkIndexedAccessType
    pub fn check_indexed_access_type(&mut self, node: Node) {
        node.for_each_child(&mut |child: Node| self.check_source_element(child));
        let t = self.get_type_from_indexed_access_type_node(node);
        self.check_indexed_access_index_type(t, node);
    }

    // Go: checker/checker.go:3412 checkMappedType
    pub fn check_mapped_type(&mut self, node: Node) {
        self.check_grammar_mapped_type(node);
        self.check_source_element(node.type_parameter());
        self.check_source_element(node.name_type());
        self.check_source_element(node.type_());
        if node.type_().is_nil() {
            let any_type = self.any_type;
            self.report_implicit_any(node, any_type, WideningKind::NORMAL);
        }
        let t = self.get_type_from_mapped_type_node(node);
        let name_type = self.get_name_type_from_mapped_type(t);
        let string_number_symbol_type = self.string_number_symbol_type;
        if name_type.is_some() {
            self.check_type_assignable_to(
                name_type,
                string_number_symbol_type,
                node.name_type(),
                None,
            );
        } else {
            let constraint_type = self.get_constraint_type_from_mapped_type(t);
            self.check_type_assignable_to(
                constraint_type,
                string_number_symbol_type,
                node.type_parameter().constraint(),
                None,
            );
        }
    }

    // Go: checker/checker.go:3431 checkFunctionDeclaration
    pub fn check_function_declaration(&mut self, node: Node) {
        self.check_function_or_method_declaration(node);
        self.check_grammar_for_generator(node);
        self.check_collisions_for_declaration_name(node, node.name());
    }

    // Go: checker/checker.go:3437 checkFunctionOrMethodDeclaration
    pub fn check_function_or_method_declaration(&mut self, node: Node) {
        self.check_decorators(node);
        self.check_signature_declaration(node);
        let function_flags = get_function_flags(node);
        // Do not use hasDynamicName here, because that returns false for well known symbols.
        // We want to perform checkComputedPropertyName for all computed properties, including
        // well known symbols.
        if node.name().is_some() && is_computed_property_name(node.name()) {
            // This check will account for methods in class/interface declarations,
            // as well as accessors in classes/object literals
            self.check_computed_property_name(node.name());
        }
        if self.has_bindable_name(node) {
            // first we want to check the local symbol that contain this declaration
            // - if node.localSymbol !== undefined - this is current declaration is exported and localSymbol points to the local symbol
            // - if node.localSymbol === undefined - this node is non-exported so we can just pick the result of getSymbolOfNode
            let symbol = self.get_symbol_of_declaration(node);
            let local_symbol = or_else_symbol_p04(node.local_symbol(), symbol);
            // Since the javascript won't do semantic analysis like typescript, ignore javascript function
            // declarations so that redeclaring a function in a JS file is not reported as a duplicate.
            if !node.flags().intersects(NodeFlags::JAVA_SCRIPT_FILE) {
                self.check_function_or_constructor_symbol(local_symbol);
            }
            if self.sym(symbol).parent.is_some() {
                // run check on export symbol to check that modifiers agree across all exported declarations
                self.check_function_or_constructor_symbol(symbol);
            }
        }
        let body = node.body();
        self.check_source_element(body);
        let return_type = self.get_return_type_from_annotation(node);
        self.check_all_code_paths_in_non_void_function_return_or_throw(node, return_type);
        // PORT: Go `node.FunctionLikeData().FullSignature`; every node that
        // reaches here embeds FunctionLikeBase, which `full_signature()` covers.
        let full_signature = node.full_signature();
        if full_signature.is_some() {
            self.check_source_element(full_signature);
            let full_signature_type = self.get_type_from_type_node(full_signature);
            if self
                .get_contextual_call_signature(full_signature_type, node)
                .is_nil()
            {
                self.error(
                    full_signature,
                    diag::A_JSDoc_type_tag_on_a_function_must_have_a_signature_with_the_correct_number_of_arguments,
                    args![],
                );
            }
        }
        if node.type_().is_nil() {
            // Report an implicit any error if there is no body, no explicit return type, and node is not a private method
            // in an ambient context
            if node_is_missing(body) && !is_private_within_ambient(node) {
                let any_type = self.any_type;
                self.report_implicit_any(node, any_type, WideningKind::NORMAL);
            }
            if function_flags.intersects(FunctionFlags::GENERATOR) && node_is_present(body) {
                // A generator with a body and no type annotation can still cause errors. It can error if the
                // yielded values have no common supertype, or it can give an implicit any error if it has no
                // yielded values. The only way to trigger these errors is to try checking its return type.
                let signature = self.get_signature_from_declaration(node);
                self.get_return_type_of_signature(signature);
            }
        }
    }

    // Go: checker/checker.go:3489 checkFunctionOrConstructorSymbol
    pub fn check_function_or_constructor_symbol(&mut self, symbol: SymbolId) {
        // Only check the symbol once
        if !self
            .value_symbol_links
            .get_by_id(&self.symbols, symbol)
            .function_or_constructor_checked
        {
            self.value_symbol_links
                .get_by_id(&self.symbols, symbol)
                .function_or_constructor_checked = true;
            self.check_function_or_constructor_symbol_worker(symbol);
        }
    }

    // Go: checker/checker.go:3497 checkFunctionOrConstructorSymbolWorker
    pub fn check_function_or_constructor_symbol_worker(&mut self, symbol: SymbolId) {
        let flags_to_check = ModifierFlags::EXPORT
            | ModifierFlags::AMBIENT
            | ModifierFlags::PRIVATE
            | ModifierFlags::PROTECTED
            | ModifierFlags::ABSTRACT;
        let mut some_node_flags = ModifierFlags::NONE;
        let mut all_node_flags = flags_to_check;
        let mut some_have_question_token = false;
        let mut all_have_question_token = true;
        let mut has_overloads = false;
        let mut body_declaration = Node::NIL;
        let mut last_seen_non_ambient_declaration = Node::NIL;
        let mut previous_declaration = Node::NIL;
        let declarations = self.sym(symbol).declarations.clone();
        let is_constructor = self.sym(symbol).flags.intersects(SymbolFlags::CONSTRUCTOR);
        let mut duplicate_function_declaration = false;
        let mut multiple_constructor_implementation = false;
        let mut has_non_ambient_class = false;
        let mut function_declarations: Vec<Node> = Vec::new();
        fn get_canonical_overload(overloads: &[Node], implementation: Node) -> Node {
            // Consider the canonical set of flags to be the flags of the bodyDeclaration or the first declaration
            // Error on all deviations from this canonical set of flags
            // The caveat is that if some overloads are defined in lib.d.ts, we don't want to
            // report the errors on those. To achieve this, we will say that the implementation is
            // the canonical signature only if it is in the same container as the first overload
            let implementation_shares_container_with_first_overload =
                implementation.is_some() && implementation.parent() == overloads[0].parent();
            if implementation_shares_container_with_first_overload {
                return implementation;
            }
            overloads[0]
        }
        let check_flag_agreement_between_overloads =
            |c: &mut Checker,
             overloads: &[Node],
             implementation: Node,
             flags_to_check: ModifierFlags,
             some_overload_flags: ModifierFlags,
             all_overload_flags: ModifierFlags| {
                // Error if some overloads have a flag that is not shared by all overloads. To find the
                // deviations, we XOR someOverloadFlags with allOverloadFlags
                let some_but_not_all_overload_flags =
                    ModifierFlags(some_overload_flags.0 ^ all_overload_flags.0);
                if !some_but_not_all_overload_flags.is_empty() {
                    let canonical_flags = c.get_effective_declaration_flags(
                        get_canonical_overload(overloads, implementation),
                        flags_to_check,
                    );
                    // PORT: Go map iteration order is random; `IndexMap` keeps file
                    // insertion order. Diagnostics are sorted later.
                    let mut groups: IndexMap<Node, Vec<Node>> = IndexMap::new();
                    for &overload in overloads {
                        let source_file = get_source_file_of_node(overload);
                        groups.entry(source_file).or_default().push(overload);
                    }
                    for (_, overloads_in_file) in groups {
                        let canonical_flags_for_file = c.get_effective_declaration_flags(
                            get_canonical_overload(&overloads_in_file, implementation),
                            flags_to_check,
                        );
                        for &overload in &overloads_in_file {
                            let deviation = ModifierFlags(
                                c.get_effective_declaration_flags(overload, flags_to_check)
                                    .0
                                    ^ canonical_flags.0,
                            );
                            let deviation_in_file = ModifierFlags(
                                c.get_effective_declaration_flags(overload, flags_to_check)
                                    .0
                                    ^ canonical_flags_for_file.0,
                            );
                            if deviation_in_file.intersects(ModifierFlags::EXPORT) {
                                // Overloads in different files need not all have export modifiers. This is ok:
                                //   // lib.d.ts
                                //   declare function foo(s: number): string;
                                //   declare function foo(s: string): number;
                                //   export { foo };
                                //
                                //   // app.ts
                                //   declare module "lib" {
                                //     export function foo(s: boolean): boolean;
                                //   }
                                c.error(
                                    get_name_of_declaration(overload),
                                    diag::Overload_signatures_must_all_be_exported_or_non_exported,
                                    args![],
                                );
                            } else if deviation_in_file.intersects(ModifierFlags::AMBIENT) {
                                // Though rare, a module augmentation (necessarily ambient) is allowed to add overloads
                                // to a non-ambient function in an implementation file.
                                c.error(
                                    get_name_of_declaration(overload),
                                    diag::Overload_signatures_must_all_be_ambient_or_non_ambient,
                                    args![],
                                );
                            } else if deviation
                                .intersects(ModifierFlags::PRIVATE | ModifierFlags::PROTECTED)
                            {
                                c.error(
                                or_else_node_p04(get_name_of_declaration(overload), overload),
                                diag::Overload_signatures_must_all_be_public_private_or_protected,
                                args![],
                            );
                            } else if deviation.intersects(ModifierFlags::ABSTRACT) {
                                c.error(
                                    get_name_of_declaration(overload),
                                    diag::Overload_signatures_must_all_be_abstract_or_non_abstract,
                                    args![],
                                );
                            }
                        }
                    }
                }
            };
        let check_question_token_agreement_between_overloads =
            |c: &mut Checker,
             overloads: &[Node],
             implementation: Node,
             some_have_question_token: bool,
             all_have_question_token: bool| {
                if some_have_question_token != all_have_question_token {
                    let canonical_has_question_token =
                        is_optional_declaration(get_canonical_overload(overloads, implementation));
                    for &o in overloads {
                        if is_optional_declaration(o) != canonical_has_question_token {
                            c.error(
                                get_name_of_declaration(o),
                                diag::Overload_signatures_must_all_be_optional_or_required,
                                args![],
                            );
                        }
                    }
                }
            };
        let report_implementation_expected_error = |c: &mut Checker, node: Node| {
            let name = node.name();
            if name.is_some() && node_is_missing(name) {
                return;
            }
            let mut seen = false;
            let mut subsequent_node = Node::NIL;
            node.parent().for_each_child(&mut |child: Node| {
                if seen {
                    subsequent_node = child;
                    return true;
                }
                seen = child == node;
                false
            });
            // We may be here because of some extra nodes between overloads that could not be parsed into a valid node.
            // In this case the subsequent node is not really consecutive (.pos !== node.end), and we must ignore it here.
            if subsequent_node.is_some() && subsequent_node.pos() == node.end() {
                if subsequent_node.kind() == node.kind() {
                    let subsequent_name = subsequent_node.name();
                    let error_node = or_else_node_p04(subsequent_name, subsequent_node);
                    if name.is_some()
                        && subsequent_name.is_some()
                        && (is_private_identifier(name)
                            && is_private_identifier(subsequent_name)
                            && name.text() == subsequent_name.text()
                            || is_computed_property_name(name)
                                && is_computed_property_name(subsequent_name)
                                && {
                                    let t1 = c.check_computed_property_name(name);
                                    let t2 = c.check_computed_property_name(subsequent_name);
                                    c.is_type_identical_to(t1, t2)
                                }
                            || is_property_name_literal(name)
                                && is_property_name_literal(subsequent_name)
                                && name.text() == subsequent_name.text())
                    {
                        let report_error = (is_method_declaration(node)
                            || is_method_signature_declaration(node))
                            && is_static(node) != is_static(subsequent_node);
                        // we can get here in two cases
                        // 1. mixed static and instance class members
                        // 2. something with the same name was defined before the set of overloads that prevents them from merging
                        // here we'll report error only for the first case since for second we should already report error in binder
                        if report_error {
                            let diagnostic = if is_static(node) {
                                diag::Function_overload_must_be_static
                            } else {
                                diag::Function_overload_must_not_be_static
                            };
                            c.error(error_node, diagnostic, args![]);
                        }
                        return;
                    }
                    if node_is_present(subsequent_node.body()) {
                        c.error(
                            error_node,
                            diag::Function_implementation_name_must_be_0,
                            args![declaration_name_to_string(name)],
                        );
                        return;
                    }
                }
            }
            let error_node = or_else_node_p04(name, node);
            if is_constructor {
                c.error(
                    error_node,
                    diag::Constructor_implementation_is_missing,
                    args![],
                );
            } else {
                // Report different errors regarding non-consecutive blocks of declarations depending on whether
                // the node in question is abstract.
                if has_syntactic_modifier(node, ModifierFlags::ABSTRACT) {
                    c.error(
                        error_node,
                        diag::All_declarations_of_an_abstract_method_must_be_consecutive,
                        args![],
                    );
                } else {
                    c.error(error_node, diag::Function_implementation_is_missing_or_not_immediately_following_the_declaration, args![]);
                }
            }
        };
        for &node in &declarations {
            let in_ambient_context = node.flags().intersects(NodeFlags::AMBIENT);
            let in_ambient_context_or_interface = in_ambient_context
                || node.parent().is_some()
                    && (is_interface_declaration(node.parent())
                        || is_type_literal_node(node.parent()));
            if in_ambient_context_or_interface {
                // check if declarations are consecutive only if they are non-ambient
                // 1. ambient declarations can be interleaved
                // i.e. this is legal
                //     declare function foo();
                //     declare function bar();
                //     declare function foo();
                // 2. mixing ambient and non-ambient declarations is a separate error that will be reported - do not want to report an extra one
                previous_declaration = Node::NIL;
            }
            if is_class_like(node) && !in_ambient_context {
                has_non_ambient_class = true;
            }
            if is_function_declaration(node)
                || is_method_declaration(node)
                || is_method_signature_declaration(node)
                || is_constructor_declaration(node)
            {
                function_declarations.push(node);
                let current_node_flags = self.get_effective_declaration_flags(node, flags_to_check);
                some_node_flags |= current_node_flags;
                all_node_flags &= current_node_flags;
                some_have_question_token =
                    some_have_question_token || is_optional_declaration(node);
                all_have_question_token = all_have_question_token && is_optional_declaration(node);
                let body_is_present = node_is_present(node.body());
                if body_is_present && body_declaration.is_some() {
                    if is_constructor {
                        multiple_constructor_implementation = true;
                    } else {
                        duplicate_function_declaration = true;
                    }
                } else if previous_declaration.is_some()
                    && previous_declaration.parent() == node.parent()
                    && previous_declaration.end() != node.pos()
                    && !previous_declaration.flags().intersects(NodeFlags::REPARSED)
                {
                    report_implementation_expected_error(self, previous_declaration);
                }
                if body_is_present {
                    if body_declaration.is_nil() {
                        body_declaration = node;
                    }
                } else {
                    has_overloads = true;
                }
                previous_declaration = node;
                if !in_ambient_context_or_interface {
                    last_seen_non_ambient_declaration = node;
                }
            }
        }
        if multiple_constructor_implementation {
            for &declaration in &function_declarations {
                self.error(
                    declaration,
                    diag::Multiple_constructor_implementations_are_not_allowed,
                    args![],
                );
            }
        }
        if duplicate_function_declaration {
            for &declaration in &function_declarations {
                self.error(
                    or_else_node_p04(get_name_of_declaration(declaration), declaration),
                    diag::Duplicate_function_implementation,
                    args![],
                );
            }
        }
        if has_non_ambient_class
            && !is_constructor
            && self.sym(symbol).flags.intersects(SymbolFlags::FUNCTION)
            && !declarations.is_empty()
        {
            let mut related_diagnostics: Vec<Diagnostic> = Vec::new();
            for &declaration in &declarations {
                if is_class_declaration(declaration) {
                    related_diagnostics.push(create_diagnostic_for_node(
                        declaration,
                        diag::Consider_adding_a_declare_modifier_to_this_class,
                        args![],
                    ));
                }
            }
            let symbol_name = self.sym(symbol).name.clone();
            for &declaration in &declarations {
                let diagnostic: Option<&'static Message> = match declaration.kind() {
                    SyntaxKind::ClassDeclaration => {
                        Some(diag::Class_declaration_cannot_implement_overload_list_for_0)
                    }
                    SyntaxKind::FunctionDeclaration => Some(
                        diag::Function_with_bodies_can_only_merge_with_classes_that_are_ambient,
                    ),
                    _ => None,
                };
                if let Some(diagnostic) = diagnostic {
                    // PORT: Go `c.error(...).SetRelatedInfo(...)` mutates the
                    // diagnostic after it was added. `error` returns an owned
                    // copy here, so build it, set the related info, then add it
                    // (same as Go `c.error` body: NewDiagnosticForNode + addDiagnostic).
                    let mut d = new_diagnostic_for_node(
                        or_else_node_p04(get_name_of_declaration(declaration), declaration),
                        diagnostic,
                        args![symbol_name],
                    );
                    d.set_related_info(related_diagnostics.clone());
                    self.add_diagnostic(d);
                }
            }
        }
        // Abstract methods can't have an implementation -- in particular, they don't need one.
        if last_seen_non_ambient_declaration.is_some()
            && last_seen_non_ambient_declaration.body().is_nil()
            && !has_syntactic_modifier(last_seen_non_ambient_declaration, ModifierFlags::ABSTRACT)
            && !is_optional_declaration(last_seen_non_ambient_declaration)
        {
            report_implementation_expected_error(self, last_seen_non_ambient_declaration);
        }
        if has_overloads {
            check_flag_agreement_between_overloads(
                self,
                &declarations,
                body_declaration,
                flags_to_check,
                some_node_flags,
                all_node_flags,
            );
            check_question_token_agreement_between_overloads(
                self,
                &declarations,
                body_declaration,
                some_have_question_token,
                all_have_question_token,
            );
            if body_declaration.is_some() {
                let signatures = self.get_signatures_of_symbol(symbol);
                let body_signature = self.get_signature_from_declaration(body_declaration);
                for &signature in &signatures {
                    if !self.is_implementation_compatible_with_overload(body_signature, signature) {
                        let error_node = self.sig(signature).declaration;
                        // PORT: Go `c.error(...).AddRelatedInfo(...)`; see the
                        // SetRelatedInfo note above.
                        let mut d = new_diagnostic_for_node(
                            error_node,
                            diag::This_overload_signature_is_not_compatible_with_its_implementation_signature,
                            args![],
                        );
                        d.add_related_info(Some(create_diagnostic_for_node(
                            body_declaration,
                            diag::The_implementation_signature_is_declared_here,
                            args![],
                        )));
                        self.add_diagnostic(d);
                        break;
                    }
                }
            }
        }
    }

    // Go: checker/checker.go:3729 getEffectiveDeclarationFlags
    pub fn get_effective_declaration_flags(
        &mut self,
        n: Node,
        flags_to_check: ModifierFlags,
    ) -> ModifierFlags {
        let mut flags = self.get_combined_modifier_flags_cached(n);
        // children of classes (even ambient classes) should not be marked as ambient or export
        // because those flags have no useful semantics there.
        if !is_interface_declaration(n.parent())
            && !is_class_declaration(n.parent())
            && !is_class_expression(n.parent())
            && n.flags().intersects(NodeFlags::AMBIENT)
        {
            let container = get_enclosing_container(n);
            if container.is_some()
                && container.flags().intersects(NodeFlags::EXPORT_CONTEXT)
                && !flags.intersects(ModifierFlags::AMBIENT)
                && !(is_module_block(n.parent())
                    && is_global_scope_augmentation(n.parent().parent()))
            {
                // It is nested in an ambient export context, which means it is automatically exported
                flags |= ModifierFlags::EXPORT;
            }
            flags |= ModifierFlags::AMBIENT;
        }
        flags & flags_to_check
    }

    // Go: checker/checker.go:3744 isImplementationCompatibleWithOverload
    pub fn is_implementation_compatible_with_overload(
        &mut self,
        implementation: SignatureId,
        overload: SignatureId,
    ) -> bool {
        let erased_source = self.get_erased_signature(implementation);
        let erased_target = self.get_erased_signature(overload);
        // First see if the return types are compatible in either direction.
        let source_return_type = self.get_return_type_of_signature(erased_source);
        let target_return_type = self.get_return_type_of_signature(erased_target);
        if target_return_type == self.void_type
            || {
                let relation = self.assignable_relation.clone();
                self.is_type_related_to(target_return_type, source_return_type, &relation)
            }
            || {
                let relation = self.assignable_relation.clone();
                self.is_type_related_to(source_return_type, target_return_type, &relation)
            }
        {
            return self.is_signature_assignable_to(
                erased_source,
                erased_target,
                true, /*ignoreReturnTypes*/
            );
        }
        false
    }

    // Go: checker/checker.go:3756 checkAllCodePathsInNonVoidFunctionReturnOrThrow
    pub fn check_all_code_paths_in_non_void_function_return_or_throw(
        &mut self,
        fn_: Node,
        return_type: TypeId,
    ) {
        let function_flags = get_function_flags(fn_);
        let mut t = TypeId::NIL;
        if return_type.is_some() {
            t = self.unwrap_return_type(return_type, function_flags);
        }
        // Functions with an explicitly specified return type that includes `void` or is exactly `any` or `undefined` don't
        // need any return statements.
        if t.is_some()
            && (self.maybe_type_of_kind(t, TypeFlags::VOID)
                || self
                    .ty(t)
                    .flags
                    .intersects(TypeFlags::ANY | TypeFlags::UNDEFINED))
        {
            return;
        }
        // If all we have is a function signature, or an arrow function with an expression body, then there is nothing to check.
        // also if HasImplicitReturn flag is not set this means that all codepaths in function body end with return or throw
        if is_method_signature_declaration(fn_)
            || node_is_missing(fn_.body())
            || !is_block(fn_.body())
            || !self.function_has_implicit_return(fn_)
        {
            return;
        }
        let has_explicit_return = fn_.flags().intersects(NodeFlags::HAS_EXPLICIT_RETURN);
        let mut error_node = fn_.type_();
        if error_node.is_nil() {
            // PORT: Go `fn.FunctionLikeData()` is non-nil for every function-like
            // node that reaches here (it has a block body); `full_signature()`
            // covers the same kinds.
            let full_signature = fn_.full_signature();
            if full_signature.is_some() {
                error_node = full_signature;
            }
        }
        if error_node.is_nil() {
            error_node = fn_;
        }
        if t.is_some() && self.ty(t).flags.intersects(TypeFlags::NEVER) {
            self.error(
                error_node,
                diag::A_function_returning_never_cannot_have_a_reachable_end_point,
                args![],
            );
        } else if t.is_some() && !has_explicit_return {
            // minimal check: function has syntactic return type annotation and no explicit return statements in the body
            // this function does not conform to the specification.
            self.error(error_node, diag::A_function_whose_declared_type_is_neither_undefined_void_nor_any_must_return_a_value, args![]);
        } else if t.is_some() && self.strict_null_checks && {
            let undefined_type = self.undefined_type;
            !self.is_type_assignable_to(undefined_type, t)
        } {
            self.error(error_node, diag::Function_lacks_ending_return_statement_and_return_type_does_not_include_undefined, args![]);
        } else if self.compiler_options.no_implicit_returns == Tristate::True {
            if t.is_nil() {
                // If return type annotation is omitted check if function has any explicit return statements.
                // If it does not have any - its inferred return type is void - don't do any checks.
                // Otherwise get inferred return type from function body and report error only if it is not void / anytype
                if !has_explicit_return {
                    return;
                }
                let signature = self.get_signature_from_declaration(fn_);
                let inferred_return_type = self.get_return_type_of_signature(signature);
                if self.is_unwrapped_return_type_undefined_void_or_any(fn_, inferred_return_type) {
                    return;
                }
            }
            self.error(error_node, diag::Not_all_code_paths_return_a_value, args![]);
        }
    }

    // Go: checker/checker.go:3808 isUnwrappedReturnTypeUndefinedVoidOrAny
    pub fn is_unwrapped_return_type_undefined_void_or_any(
        &mut self,
        fn_: Node,
        return_type: TypeId,
    ) -> bool {
        let t = self.unwrap_return_type(return_type, get_function_flags(fn_));
        t.is_some()
            && (self.maybe_type_of_kind(t, TypeFlags::VOID)
                || self
                    .ty(t)
                    .flags
                    .intersects(TypeFlags::ANY | TypeFlags::UNDEFINED))
    }

    // Go: checker/checker.go:3813 checkBlock
    pub fn check_block(&mut self, node: Node) {
        // Grammar checking for SyntaxKind.Block
        if node.kind() == SyntaxKind::Block {
            self.check_grammar_statement_in_ambient_context(node);
        }
        if is_function_or_module_block(node) {
            let save_flow_analysis_disabled = self.flow_analysis_disabled;
            self.check_source_elements(node.statements());
            self.flow_analysis_disabled = save_flow_analysis_disabled;
        } else {
            self.check_source_elements(node.statements());
        }
        if self.symbols.len(node.locals()) != 0 {
            self.register_for_unused_identifiers_check(node);
        }
    }

    // Go: checker/checker.go:3830 checkIfStatement
    pub fn check_if_statement(&mut self, node: Node) {
        self.check_grammar_statement_in_ambient_context(node);
        let t = self.check_truthiness_expression(node.expression(), CheckMode::NORMAL);
        let then_statement = node.then_statement();
        self.check_testing_known_truthy_callable_or_awaitable_or_enum_member_type(
            node.expression(),
            t,
            then_statement,
        );
        self.check_source_element(then_statement);
        if is_empty_statement(then_statement) {
            self.error(
                then_statement,
                diag::The_body_of_an_if_statement_cannot_be_the_empty_statement,
                args![],
            );
        }
        self.check_source_element(node.else_statement());
    }

    // Go: checker/checker.go:3842 checkTestingKnownTruthyCallableOrAwaitableOrEnumMemberType
    pub fn check_testing_known_truthy_callable_or_awaitable_or_enum_member_type(
        &mut self,
        cond_expr: Node,
        cond_type: TypeId,
        body: Node,
    ) {
        if !self.strict_null_checks {
            return;
        }
        self.check_testing_known_truthy_types(cond_expr, cond_type, body);
    }

    // Go: checker/checker.go:3849 checkTestingKnownTruthyTypes
    pub fn check_testing_known_truthy_types(
        &mut self,
        cond_expr: Node,
        cond_type: TypeId,
        body: Node,
    ) {
        let mut cond_expr = skip_parentheses(cond_expr);
        self.check_testing_known_truthy_type(cond_expr, cond_type, body);
        while is_binary_expression(cond_expr)
            && (cond_expr.operator_token().kind() == SyntaxKind::BarBarToken
                || cond_expr.operator_token().kind() == SyntaxKind::QuestionQuestionToken)
        {
            cond_expr = skip_parentheses(cond_expr.left());
            self.check_testing_known_truthy_type(cond_expr, cond_type, body);
        }
    }

    // Go: checker/checker.go:3858 checkTestingKnownTruthyType
    pub fn check_testing_known_truthy_type(
        &mut self,
        cond_expr: Node,
        cond_type: TypeId,
        body: Node,
    ) {
        let mut location = cond_expr;
        if is_logical_or_coalescing_binary_expression(cond_expr) {
            location = skip_parentheses(cond_expr.right());
        }
        if is_module_exports_access_expression(location) {
            return;
        }
        if is_logical_or_coalescing_binary_expression(location) {
            self.check_testing_known_truthy_types(location, cond_type, body);
            return;
        }
        let mut t = cond_type;
        if location != cond_expr {
            t = self.check_expression(location);
        }
        if self.ty(t).flags.intersects(TypeFlags::ENUM_LITERAL)
            && is_property_access_expression(location)
            && {
                let resolved = self.get_resolved_symbol_or_nil(location.expression());
                let unknown_symbol = self.unknown_symbol;
                self.sym(or_else_symbol_p04(resolved, unknown_symbol))
                    .flags
                    .intersects(SymbolFlags::ENUM)
            }
        {
            // EnumLiteral type at condition with known value is always truthy or always falsy, likely an error
            let truthy = crate::evaluator::is_truthy(
                self.ty(t)
                    .as_literal_type()
                    .value
                    .as_ref()
                    .expect("Unhandled case in IsTruthy"),
            );
            self.error(
                location,
                diag::This_condition_will_always_return_0,
                args![if truthy { "true" } else { "false" }],
            );
            return;
        }
        // PORT: Go checker `isTypeAssertion(node)` is
        // `ast.IsAssertionExpression(ast.SkipParentheses(node))`. It is inlined
        // because its Rust name collides with the generated `ast.IsTypeAssertion`.
        let is_property_expression_cast = is_property_access_expression(location)
            && is_assertion_expression(skip_parentheses(location.expression()));
        if !self.has_type_facts(t, TypeFacts::TRUTHY) || is_property_expression_cast {
            return;
        }
        // While it technically should be invalid for any known-truthy value
        // to be tested, we de-scope to functions and Promises unreferenced in
        // the block as a heuristic to identify the most common bugs. There
        // are too many false positives for values sourced from type
        // definitions without strictNullChecks otherwise.
        let call_signatures = self.get_signatures_of_type(t, SignatureKind::CALL);
        let is_promise = self.get_awaited_type_of_promise(t).is_some();
        if call_signatures.is_empty() && !is_promise {
            return;
        }
        let mut tested_node = Node::NIL;
        if is_identifier(location) {
            tested_node = location;
        } else if is_property_access_expression(location) {
            tested_node = location.name();
        }
        let mut tested_symbol = SymbolId::NIL;
        if tested_node.is_some() {
            tested_symbol = self.get_symbol_at_location(tested_node, false);
        }
        if tested_symbol.is_nil() && !is_promise {
            return;
        }
        let is_used = tested_symbol.is_some()
            && is_binary_expression(cond_expr.parent())
            && self.is_symbol_used_in_binary_expression_chain(cond_expr.parent(), tested_symbol)
            || tested_symbol.is_some()
                && body.is_some()
                && self.is_symbol_used_in_condition_body(
                    cond_expr,
                    body,
                    tested_node,
                    tested_symbol,
                );
        if !is_used {
            if is_promise {
                let type_name = self.get_type_name_for_error_display(t);
                self.error_and_maybe_suggest_await(
                    location,
                    true,
                    diag::This_condition_will_always_return_true_since_this_0_is_always_defined,
                    args![type_name],
                );
            } else {
                self.error(
                    location,
                    diag::This_condition_will_always_return_true_since_this_function_is_always_defined_Did_you_mean_to_call_it_instead,
                    args![],
                );
            }
        }
    }
}
