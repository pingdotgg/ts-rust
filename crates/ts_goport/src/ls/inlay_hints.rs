use crate::ls::prelude::*;

// Go `internal/ls/inlay_hints.go`: textDocument/inlayHint.

use crate::frontend::stringutil_ls;
use crate::spanmap::Feature;

impl LanguageService {
    // Go: ls/inlay_hints.go:25 ProvideInlayHint
    pub fn provide_inlay_hint(
        &self,
        ctx: &Context,
        params: &lsproto::InlayHintParams,
    ) -> Result<lsproto::InlayHintResponse, GoError> {
        let user_preferences = self.user_preferences();
        let inlay_hint_preferences = user_preferences.inlay_hints;
        if !is_any_inlay_hint_enabled(&inlay_hint_preferences) {
            return Ok(lsproto::InlayHintsOrNull { inlay_hints: None });
        }

        let (program, file) = self.get_program_and_file(&params.text_document.uri);
        let quote_preference = lsutil::get_quote_preference(file, &user_preferences);

        let mapped_ranges = lsconv::from_lsp_range_intersecting_for_source_file(
            &self.converters,
            file,
            params.range,
            Feature::INLAY_HINTS,
        );
        let mut result: Vec<lsproto::InlayHint> = Vec::with_capacity(mapped_ranges.len());
        // ts#64543: each range releases its checker before the next range
        // acquires one (Go wraps the loop body in a func with `defer done()`),
        // because acquisitions are not reentrant. `_done` drops at the end of
        // each pass.
        for mapped in mapped_ranges {
            let projection = mapped.script;
            let (checker, _done) = ls_program::get_type_checker_for_file(program, ctx, projection);
            let c = &mut *checker.borrow_mut();
            let mut inlay_hint_state = InlayHintState {
                ctx,
                span: mapped.span,
                preferences: inlay_hint_preferences,
                quote_preference,
                file: projection,
                checker: c,
                converters: self.converters.clone(),
                result: Vec::new(),
            };
            inlay_hint_state.visit(projection);
            result.extend(inlay_hint_state.result);
        }
        Ok(lsproto::InlayHintsOrNull {
            inlay_hints: Some(result),
        })
    }
}

// Go: ls/inlay_hints.go:61 inlayHintState
// PORT: Go `checker *checker.Checker` is the leased checker, borrowed for the
// request (`&mut Checker`).
struct InlayHintState<'a> {
    ctx: &'a Context,
    span: TextRange,
    preferences: lsutil::InlayHintsPreferences,
    quote_preference: lsutil::QuotePreference,
    file: Node,
    checker: &'a mut Checker,
    converters: Rc<lsconv::Converters>,
    result: Vec<lsproto::InlayHint>,
}

// Go: ls/inlay_hints.go:830 parameterInfo
struct ParameterInfo {
    parameter: Node,
    name: String,
    is_rest_parameter: bool,
}

impl InlayHintState<'_> {
    // Go: ls/inlay_hints.go:72 inlayHintState.visit
    fn visit(&mut self, node: Node) -> bool {
        if node.is_nil()
            || node.end() - node.pos() == 0
            || node.flags().intersects(NodeFlags::REPARSED)
        {
            return false;
        }

        match node.kind() {
            SyntaxKind::ModuleDeclaration
            | SyntaxKind::ClassDeclaration
            | SyntaxKind::InterfaceDeclaration
            | SyntaxKind::FunctionDeclaration
            | SyntaxKind::ClassExpression
            | SyntaxKind::FunctionExpression
            | SyntaxKind::MethodDeclaration
            | SyntaxKind::ArrowFunction => {
                if self.ctx.err().is_some() {
                    return true;
                }
            }
            _ => {}
        }

        if !self.span.intersects(node.loc()) {
            return false;
        }

        if is_type_node(node) && !is_expression_with_type_arguments(node) {
            return false;
        }

        if self.preferences.include_inlay_variable_type_hints.is_true()
            && is_variable_declaration(node)
        {
            self.visit_variable_like_declaration(node);
        } else if self
            .preferences
            .include_inlay_property_declaration_type_hints
            .is_true()
            && is_property_declaration(node)
        {
            self.visit_variable_like_declaration(node);
        } else if self
            .preferences
            .include_inlay_enum_member_value_hints
            .is_true()
            && is_enum_member(node)
        {
            self.visit_enum_member(node);
        } else if should_show_parameter_name_hints(&self.preferences)
            && (is_call_expression(node) || is_new_expression(node))
        {
            self.visit_call_or_new_expression(node);
        } else {
            if self
                .preferences
                .include_inlay_function_parameter_type_hints
                .is_true()
                && is_function_like_declaration(node)
                && has_context_sensitive_parameters(node)
            {
                self.visit_function_like_for_parameter_type(node);
            }
            if self
                .preferences
                .include_inlay_function_like_return_type_hints
                .is_true()
                && is_signature_supporting_return_annotation(node)
            {
                self.visit_function_declaration_like_for_return_type(node);
            }
        }
        node.for_each_child(|child| self.visit(child))
    }

    // Go: ls/inlay_hints.go:117 inlayHintState.visitFunctionDeclarationLikeForReturnType
    // FunctionDeclaration | MethodDeclaration | GetAccessor | FunctionExpression | ArrowFunction
    fn visit_function_declaration_like_for_return_type(&mut self, decl: Node) {
        if is_arrow_function(decl) {
            if astnav::find_child_of_kind(decl, SyntaxKind::OpenParenToken, self.file).is_nil() {
                return;
            }
        }

        let type_annotation = decl.type_();
        if type_annotation.is_some() || decl.body().is_nil() {
            return;
        }

        let signature = self.checker.get_signature_from_declaration_exported(decl);
        if signature.is_nil() {
            return;
        }

        let type_predicate = self
            .checker
            .get_type_predicate_of_signature_exported(signature);

        if type_predicate.is_some() && self.checker.pred(type_predicate).type_().is_some() {
            let hint_parts = self.type_predicate_to_inlay_hint_parts(type_predicate);
            let position = self.get_type_annotation_position(decl);
            self.add_type_hints(hint_parts, position);
            return;
        }

        let return_type = self
            .checker
            .get_return_type_of_signature_exported(signature);
        if is_module_reference_type(&*self.checker, return_type) {
            return;
        }

        let hint_parts = self.type_to_inlay_hint_parts(return_type);
        let position = self.get_type_annotation_position(decl);
        self.add_type_hints(hint_parts, position);
    }

    // Go: ls/inlay_hints.go:151 inlayHintState.visitCallOrNewExpression
    fn visit_call_or_new_expression(&mut self, expr: Node) {
        let args = expr.arguments();
        if args.is_empty() {
            return;
        }

        let signature = self.checker.get_resolved_signature_exported(expr);
        if signature.is_nil() {
            return;
        }

        let mut signature_param_pos: i32 = 0;
        for original_arg in args {
            let arg = skip_parentheses(original_arg);
            if should_show_literal_parameter_name_hints_only(&self.preferences)
                && !is_hintable_literal(arg)
            {
                signature_param_pos += 1;
                continue;
            }

            let mut spread_args: i32 = 0;
            if is_spread_element(arg) {
                let spread_type = self.checker.get_type_at_location(arg.expression());
                if self.checker.is_tuple_type(spread_type) {
                    let target = self.checker.ty(spread_type).target();
                    let element_flags = self.checker.ty(target).as_tuple_type().element_flags();
                    let fixed_length = self.checker.ty(target).as_tuple_type().fixed_length();
                    if fixed_length == 0 {
                        continue;
                    }
                    // PORT: Go `slices.IndexFunc`, -1 when no element matches.
                    let first_optional_index = element_flags
                        .iter()
                        .position(|f| !f.intersects(ElementFlags::REQUIRED))
                        .map_or(-1, |i| i as i32);
                    let required_args = if first_optional_index < 0 {
                        fixed_length
                    } else {
                        first_optional_index
                    };
                    if required_args > 0 {
                        spread_args = required_args;
                    }
                }
            }

            let identifier_info =
                self.get_parameter_identifier_info_at_position(signature, signature_param_pos);
            signature_param_pos += if spread_args > 0 { spread_args } else { 1 };
            let Some(identifier_info) = identifier_info else {
                return;
            };

            let parameter = identifier_info.parameter;
            let parameter_name = identifier_info.name;
            let is_first_variadic_argument = identifier_info.is_rest_parameter;
            let parameter_name_not_same_as_argument = self
                .preferences
                .include_inlay_parameter_name_hints_when_argument_matches_name
                .is_true()
                || !identifier_or_access_expression_postfix_matches_parameter_name(
                    arg,
                    &parameter_name,
                );
            if !parameter_name_not_same_as_argument && !is_first_variadic_argument {
                continue;
            }

            if self.leading_comments_contains_parameter_name(arg, &parameter_name) {
                continue;
            }

            self.add_parameter_hints(
                &parameter_name,
                parameter,
                astnav::get_start_of_node(original_arg, self.file, false /*includeJSDoc*/),
                is_first_variadic_argument,
            );
        }
    }

    // Go: ls/inlay_hints.go:217 inlayHintState.visitEnumMember
    fn visit_enum_member(&mut self, member: Node) {
        if member.initializer().is_some() {
            return;
        }

        let enum_value = self.checker.get_constant_value(member);
        if let Some(enum_value) = enum_value {
            self.add_enum_member_value_hints(&any_to_string(&enum_value), member.end());
        }
    }

    // Go: ls/inlay_hints.go:228 inlayHintState.visitVariableLikeDeclaration
    fn visit_variable_like_declaration(&mut self, decl: Node) {
        if decl.initializer().is_nil()
            && !(is_property_declaration(decl) && {
                let t = self.checker.get_type_at_location(decl);
                !self.checker.ty(t).flags().intersects(TypeFlags::ANY)
            })
            || is_binding_pattern(decl.name())
            || (is_variable_declaration(decl) && !is_hintable_declaration(decl))
        {
            return;
        }

        let type_annotation = decl.type_();
        if type_annotation.is_some() {
            return;
        }

        let declaration_type = self.checker.get_type_at_location(decl);
        if is_module_reference_type(&*self.checker, declaration_type) {
            return;
        }

        let hint_parts = self.type_to_inlay_hint_parts(declaration_type);
        let mut hint_text = String::new();
        if let Some(string) = &hint_parts.string {
            hint_text = string.clone();
        } else if let Some(inlay_hint_label_parts) = &hint_parts.inlay_hint_label_parts {
            let mut b = String::new();
            for part in inlay_hint_label_parts {
                // Go `part.Value` (a nil part panics).
                let part = part
                    .as_ref()
                    .unwrap_or_else(|| crate::core::go_nil_dereference());
                b.push_str(&part.value);
            }
            hint_text = b;
        }
        if !self
            .preferences
            .include_inlay_variable_type_hints_when_type_matches_name
            .is_true()
            && !is_computed_property_name(decl.name())
            && stringutil_ls::equate_string_case_insensitive(decl.name().text(), &hint_text)
        {
            return;
        }
        self.add_type_hints(hint_parts, decl.name().end());
    }

    // Go: ls/inlay_hints.go:264 inlayHintState.visitFunctionLikeForParameterType
    fn visit_function_like_for_parameter_type(&mut self, node: Node) {
        let signature = self.checker.get_signature_from_declaration_exported(node);
        if signature.is_nil() {
            return;
        }

        let mut pos = 0;
        for param in node.parameters() {
            if is_hintable_declaration(param) {
                let symbol;
                if is_this_parameter(param) {
                    symbol = self.checker.sig(signature).this_parameter();
                } else {
                    symbol = self.checker.sig(signature).parameters()[pos];
                }
                self.add_parameter_type_hint(param, symbol);
            }
            if is_this_parameter(param) {
                continue;
            }
            pos += 1;
        }
    }

    // Go: ls/inlay_hints.go:288 inlayHintState.addParameterTypeHint
    fn add_parameter_type_hint(&mut self, node: Node, symbol: SymbolId) {
        let type_annotation = node.type_();
        if type_annotation.is_some() || symbol.is_nil() {
            return;
        }
        let type_hints = self.get_parameter_declaration_type_hints(symbol);
        let Some(type_hints) = type_hints else {
            return;
        };
        let pos;
        if node.question_token().is_some() {
            pos = node.question_token().end();
        } else {
            pos = node.name().end();
        }
        self.add_type_hints(type_hints, pos);
    }

    // Go: ls/inlay_hints.go:306 inlayHintState.getParameterDeclarationTypeHints
    fn get_parameter_declaration_type_hints(
        &mut self,
        symbol: SymbolId,
    ) -> Option<lsproto::StringOrInlayHintLabelParts> {
        let value_declaration = self.checker.sym(symbol).value_declaration;
        if value_declaration.is_nil() || !is_parameter_declaration(value_declaration) {
            return None;
        }

        let signature_param_type = self
            .checker
            .get_type_of_symbol_at_location(symbol, value_declaration);
        if is_module_reference_type(&*self.checker, signature_param_type) {
            return None;
        }

        Some(self.type_to_inlay_hint_parts(signature_param_type))
    }

    // Go: ls/inlay_hints.go:320 inlayHintState.typeToInlayHintParts
    // PORT: Go `c.TypeToTypeNode(t, nil, flags, idToSymbol)` shares
    // `idToSymbol` with the node builder, which fills it. The Rust builder owns
    // its map, so this inlines the wrapper body (`getNodeBuilderEx` plus
    // `nodeBuilder.TypeToTypeNode`) and reads the filled map back from
    // `impl_.id_to_symbol`.
    fn type_to_inlay_hint_parts(&mut self, t: TypeId) -> lsproto::StringOrInlayHintLabelParts {
        let flags = NodeBuilderFlags::IGNORE_ERRORS
            | NodeBuilderFlags::ALLOW_UNIQUE_ES_SYMBOL_TYPE
            | NodeBuilderFlags::USE_ALIAS_DEFINED_OUTSIDE_CURRENT_SCOPE;
        let id_to_symbol: FxHashMap<Node, SymbolId> = FxHashMap::default();
        // !!! Avoid type node reuse so we collect identifier symbols.
        let node_builder = self.checker.get_node_builder_ex(Some(id_to_symbol));
        let type_node = self.checker.node_builder_type_to_type_node(
            &node_builder,
            t,
            Node::NIL, /*enclosingDeclaration*/
            flags,
            InternalNodeBuilderFlags::NONE,
            None,
        );
        let id_to_symbol = node_builder.borrow().impl_.borrow().id_to_symbol.clone();
        crate::go_assert!(type_node.is_some(), "should always get typenode");
        lsproto::StringOrInlayHintLabelParts {
            inlay_hint_label_parts: Some(self.get_inlay_hint_label_parts(type_node, &id_to_symbol)),
            ..Default::default()
        }
    }

    // Go: ls/inlay_hints.go:332 inlayHintState.typePredicateToInlayHintParts
    // PORT: see type_to_inlay_hint_parts about `idToSymbol`.
    fn type_predicate_to_inlay_hint_parts(
        &mut self,
        type_predicate: TypePredicateId,
    ) -> lsproto::StringOrInlayHintLabelParts {
        let flags = NodeBuilderFlags::IGNORE_ERRORS
            | NodeBuilderFlags::ALLOW_UNIQUE_ES_SYMBOL_TYPE
            | NodeBuilderFlags::USE_ALIAS_DEFINED_OUTSIDE_CURRENT_SCOPE;
        let id_to_symbol: FxHashMap<Node, SymbolId> = FxHashMap::default();
        // !!! Avoid type node reuse so we collect identifier symbols.
        let node_builder = self.checker.get_node_builder_ex(Some(id_to_symbol));
        let type_node = self
            .checker
            .node_builder_type_predicate_to_type_predicate_node(
                &node_builder,
                type_predicate,
                Node::NIL, /*enclosingDeclaration*/
                flags,
                InternalNodeBuilderFlags::NONE,
                None,
            );
        let id_to_symbol = node_builder.borrow().impl_.borrow().id_to_symbol.clone();
        crate::go_assert!(type_node.is_some(), "should always get typePredicateNode");
        lsproto::StringOrInlayHintLabelParts {
            inlay_hint_label_parts: Some(self.get_inlay_hint_label_parts(type_node, &id_to_symbol)),
            ..Default::default()
        }
    }

    // Go: ls/inlay_hints.go:344 inlayHintState.addTypeHints
    fn add_type_hints(&mut self, hint: lsproto::StringOrInlayHintLabelParts, position: i32) {
        let (lsp_position, fidelity) =
            self.converters
                .to_lsp_position_for_feature(&self.file, position, Feature::INLAY_HINTS);
        if fidelity.is_none() {
            return;
        }
        let mut hint = hint;
        if hint.string.is_some() {
            hint.string = Some(format!(": {}", hint.string.as_ref().unwrap()));
        } else {
            let mut parts = vec![Some(lsproto::InlayHintLabelPart {
                value: ": ".to_string(),
                ..Default::default()
            })];
            // PORT: Go dereferences `*hint.InlayHintLabelParts`; nil panics.
            parts.extend(
                hint.inlay_hint_label_parts
                    .take()
                    .unwrap_or_else(|| crate::core::go_nil_dereference()),
            );
            hint.inlay_hint_label_parts = Some(parts);
        }
        self.result.push(lsproto::InlayHint {
            label: hint,
            position: lsp_position,
            kind: Some(lsproto::InlayHintKind::TYPE),
            padding_left: Some(true),
            ..Default::default()
        });
    }

    // Go: ls/inlay_hints.go:362 inlayHintState.addEnumMemberValueHints
    fn add_enum_member_value_hints(&mut self, text: &str, position: i32) {
        let (lsp_position, fidelity) =
            self.converters
                .to_lsp_position_for_feature(&self.file, position, Feature::INLAY_HINTS);
        if fidelity.is_none() {
            return;
        }
        self.result.push(lsproto::InlayHint {
            label: lsproto::StringOrInlayHintLabelParts {
                string: Some(format!("= {text}")),
                ..Default::default()
            },
            position: lsp_position,
            padding_left: Some(true),
            ..Default::default()
        });
    }

    // Go: ls/inlay_hints.go:376 inlayHintState.addParameterHints
    fn add_parameter_hints(
        &mut self,
        text: &str,
        parameter: Node,
        position: i32,
        is_first_variadic_argument: bool,
    ) {
        let (lsp_position, fidelity) =
            self.converters
                .to_lsp_position_for_feature(&self.file, position, Feature::INLAY_HINTS);
        if fidelity.is_none() {
            return;
        }
        let hint_text = format!(
            "{}{}",
            if is_first_variadic_argument {
                "..."
            } else {
                ""
            },
            text
        );
        let display_parts = vec![
            Some(self.get_node_display_part(&hint_text, parameter)),
            Some(lsproto::InlayHintLabelPart {
                value: ":".to_string(),
                ..Default::default()
            }),
        ];
        let label_parts = lsproto::StringOrInlayHintLabelParts {
            inlay_hint_label_parts: Some(display_parts),
            ..Default::default()
        };

        self.result.push(lsproto::InlayHint {
            label: label_parts,
            position: lsp_position,
            kind: Some(lsproto::InlayHintKind::PARAMETER),
            padding_right: Some(true),
            ..Default::default()
        });
    }

    // Go: ls/inlay_hints.go:443 inlayHintState.getInlayHintLabelParts
    // PORT: the Go closures `visitForDisplayParts`, `visitDisplayPartList` and
    // `visitParametersAndTypeParameters` are methods below. They take the
    // captured `parts` and `idToSymbol` as parameters. Go returns
    // `[]*lsproto.InlayHintLabelPart`; the visitors build values, which are
    // never nil.
    fn get_inlay_hint_label_parts(
        &self,
        node: Node,
        id_to_symbol: &FxHashMap<Node, SymbolId>,
    ) -> Vec<Option<lsproto::InlayHintLabelPart>> {
        let mut parts: Vec<lsproto::InlayHintLabelPart> = Vec::new();
        self.visit_for_display_parts(node, id_to_symbol, &mut parts);
        parts.into_iter().map(Some).collect()
    }

    // Go: ls/inlay_hints.go:450 visitForDisplayParts (closure in getInlayHintLabelParts)
    fn visit_for_display_parts(
        &self,
        node: Node,
        id_to_symbol: &FxHashMap<Node, SymbolId>,
        parts: &mut Vec<lsproto::InlayHintLabelPart>,
    ) {
        if node.is_nil() {
            return;
        }

        let token_string = token_to_string(node.kind());
        if !token_string.is_empty() {
            parts.push(label_part(token_string));
            return;
        }

        if is_literal_expression(node) {
            parts.push(label_part(&self.get_literal_text(node)));
            return;
        }

        match node.kind() {
            SyntaxKind::Identifier => {
                let identifier_text = node.text();
                let mut name = Node::NIL;
                let symbol = id_to_symbol.get(&node).copied().unwrap_or(SymbolId::NIL);
                if symbol.is_some() && !self.checker.sym(symbol).declarations.is_empty() {
                    name = get_name_of_declaration(self.checker.sym(symbol).declarations[0]);
                }
                if name.is_some() {
                    parts.push(self.get_node_display_part(identifier_text, name));
                } else {
                    parts.push(label_part(identifier_text));
                }
            }
            SyntaxKind::QualifiedName => {
                self.visit_for_display_parts(node.left(), id_to_symbol, parts);
                parts.push(label_part("."));
                self.visit_for_display_parts(node.right(), id_to_symbol, parts);
            }
            SyntaxKind::TypePredicate => {
                if node.asserts_modifier().is_some() {
                    parts.push(label_part("asserts "));
                }
                self.visit_for_display_parts(node.parameter_name(), id_to_symbol, parts);
                if node.type_().is_some() {
                    parts.push(label_part(" is "));
                    self.visit_for_display_parts(node.type_(), id_to_symbol, parts);
                }
            }
            SyntaxKind::TypeReference => {
                self.visit_for_display_parts(node.type_name(), id_to_symbol, parts);
                if !node.type_arguments().is_empty() {
                    parts.push(label_part("<"));
                    self.visit_display_part_list(node.type_arguments(), ",", id_to_symbol, parts);
                    parts.push(label_part(">"));
                }
            }
            SyntaxKind::TypeParameter => {
                if !node.modifier_nodes().is_empty() {
                    self.visit_display_part_list(node.modifier_nodes(), "", id_to_symbol, parts);
                }
                self.visit_for_display_parts(node.name(), id_to_symbol, parts);
                if node.constraint().is_some() {
                    parts.push(label_part(" extends "));
                    self.visit_for_display_parts(node.constraint(), id_to_symbol, parts);
                }
                if node.default_type().is_some() {
                    parts.push(label_part(" = "));
                    self.visit_for_display_parts(node.default_type(), id_to_symbol, parts);
                }
            }
            SyntaxKind::Parameter => {
                if !node.modifier_nodes().is_empty() {
                    self.visit_display_part_list(node.modifier_nodes(), " ", id_to_symbol, parts);
                }
                if node.dot_dot_dot_token().is_some() {
                    parts.push(label_part("..."));
                }
                self.visit_for_display_parts(node.name(), id_to_symbol, parts);
                if node.question_token().is_some() {
                    parts.push(label_part("?"));
                }
                if node.type_().is_some() {
                    parts.push(label_part(": "));
                    self.visit_for_display_parts(node.type_(), id_to_symbol, parts);
                }
            }
            SyntaxKind::ConstructorType => {
                parts.push(label_part("new "));
                self.visit_parameters_and_type_parameters(node, id_to_symbol, parts);
                parts.push(label_part(" => "));
                self.visit_for_display_parts(node.type_(), id_to_symbol, parts);
            }
            SyntaxKind::TypeQuery => {
                parts.push(label_part("typeof "));
                self.visit_for_display_parts(node.expr_name(), id_to_symbol, parts);
                if !node.type_arguments().is_empty() {
                    parts.push(label_part("<"));
                    self.visit_display_part_list(node.type_arguments(), ", ", id_to_symbol, parts);
                    parts.push(label_part(">"));
                }
            }
            SyntaxKind::TypeLiteral => {
                parts.push(label_part("{"));
                if !node.members().is_empty() {
                    parts.push(label_part(" "));
                    self.visit_display_part_list(node.members(), "; ", id_to_symbol, parts);
                    parts.push(label_part(" "));
                }
                parts.push(label_part("}"));
            }
            SyntaxKind::ArrayType => {
                self.visit_for_display_parts(node.element_type(), id_to_symbol, parts);
                parts.push(label_part("[]"));
            }
            SyntaxKind::TupleType => {
                parts.push(label_part("["));
                self.visit_display_part_list(node.elements(), ", ", id_to_symbol, parts);
                parts.push(label_part("]"));
            }
            SyntaxKind::NamedTupleMember => {
                if node.dot_dot_dot_token().is_some() {
                    parts.push(label_part("..."));
                }
                self.visit_for_display_parts(node.name(), id_to_symbol, parts);
                if node.question_token().is_some() {
                    parts.push(label_part("?"));
                }
                parts.push(label_part(": "));
                self.visit_for_display_parts(node.type_(), id_to_symbol, parts);
            }
            SyntaxKind::OptionalType => {
                self.visit_for_display_parts(node.type_(), id_to_symbol, parts);
                parts.push(label_part("?"));
            }
            SyntaxKind::RestType => {
                parts.push(label_part("..."));
                self.visit_for_display_parts(node.type_(), id_to_symbol, parts);
            }
            SyntaxKind::UnionType => {
                if node.types().is_some() {
                    self.visit_display_part_list(node.types().nodes(), " | ", id_to_symbol, parts);
                }
            }
            SyntaxKind::IntersectionType => {
                if node.types().is_some() {
                    self.visit_display_part_list(node.types().nodes(), " & ", id_to_symbol, parts);
                }
            }
            SyntaxKind::ConditionalType => {
                self.visit_for_display_parts(node.check_type(), id_to_symbol, parts);
                parts.push(label_part(" extends "));
                self.visit_for_display_parts(node.extends_type(), id_to_symbol, parts);
                parts.push(label_part(" ? "));
                self.visit_for_display_parts(node.true_type(), id_to_symbol, parts);
                parts.push(label_part(" : "));
                self.visit_for_display_parts(node.false_type(), id_to_symbol, parts);
            }
            SyntaxKind::InferType => {
                parts.push(label_part("infer "));
                self.visit_for_display_parts(node.type_parameter(), id_to_symbol, parts);
            }
            SyntaxKind::ParenthesizedType => {
                parts.push(label_part("("));
                self.visit_for_display_parts(node.type_(), id_to_symbol, parts);
                parts.push(label_part(")"));
            }
            SyntaxKind::TypeOperator => {
                parts.push(label_part(token_to_string(node.operator())));
                self.visit_for_display_parts(node.type_(), id_to_symbol, parts);
            }
            SyntaxKind::IndexedAccessType => {
                self.visit_for_display_parts(node.object_type(), id_to_symbol, parts);
                parts.push(label_part("["));
                self.visit_for_display_parts(node.index_type(), id_to_symbol, parts);
                parts.push(label_part("]"));
            }
            SyntaxKind::MappedType => {
                parts.push(label_part("{ "));
                if node.readonly_token().is_some() {
                    if node.readonly_token().kind() == SyntaxKind::PlusToken {
                        parts.push(label_part("+"));
                    } else if node.readonly_token().kind() == SyntaxKind::MinusToken {
                        parts.push(label_part("-"));
                    }
                    parts.push(label_part("readonly "));
                }
                parts.push(label_part("["));
                self.visit_for_display_parts(node.type_parameter(), id_to_symbol, parts);
                if node.name_type().is_some() {
                    parts.push(label_part(" as "));
                    self.visit_for_display_parts(node.name_type(), id_to_symbol, parts);
                }
                parts.push(label_part("]"));
                if node.question_token().is_some() {
                    if node.question_token().kind() == SyntaxKind::PlusToken {
                        parts.push(label_part("+"));
                    } else if node.question_token().kind() == SyntaxKind::MinusToken {
                        parts.push(label_part("-"));
                    }
                    parts.push(label_part("?"));
                }
                parts.push(label_part(": "));
                if node.type_().is_some() {
                    self.visit_for_display_parts(node.type_(), id_to_symbol, parts);
                }
                parts.push(label_part("; }"));
            }
            SyntaxKind::LiteralType => {
                self.visit_for_display_parts(node.literal(), id_to_symbol, parts);
            }
            SyntaxKind::FunctionType => {
                self.visit_parameters_and_type_parameters(node, id_to_symbol, parts);
                parts.push(label_part(" => "));
                self.visit_for_display_parts(node.type_(), id_to_symbol, parts);
            }
            SyntaxKind::ImportType => {
                if node.is_type_of() {
                    parts.push(label_part("typeof "));
                }
                parts.push(label_part("import("));
                self.visit_for_display_parts(node.argument(), id_to_symbol, parts);
                parts.push(label_part(")"));
                if node.qualifier().is_some() {
                    parts.push(label_part("."));
                    self.visit_for_display_parts(node.qualifier(), id_to_symbol, parts);
                }
                if !node.type_arguments().is_empty() {
                    parts.push(label_part("<"));
                    self.visit_display_part_list(node.type_arguments(), ", ", id_to_symbol, parts);
                    parts.push(label_part(">"));
                }
            }
            SyntaxKind::PropertySignature => {
                if !node.modifier_nodes().is_empty() {
                    self.visit_display_part_list(node.modifier_nodes(), " ", id_to_symbol, parts);
                    parts.push(label_part(" "));
                }
                self.visit_for_display_parts(node.name(), id_to_symbol, parts);
                if node.postfix_token().is_some() {
                    parts.push(label_part(token_to_string(node.postfix_token().kind())));
                }
                if node.type_().is_some() {
                    parts.push(label_part(": "));
                    self.visit_for_display_parts(node.type_(), id_to_symbol, parts);
                }
            }
            SyntaxKind::IndexSignature => {
                parts.push(label_part("["));
                self.visit_display_part_list(node.parameters(), ", ", id_to_symbol, parts);
                parts.push(label_part("]"));
                if node.type_().is_some() {
                    parts.push(label_part(": "));
                    self.visit_for_display_parts(node.type_(), id_to_symbol, parts);
                }
            }
            SyntaxKind::MethodSignature => {
                if !node.modifier_nodes().is_empty() {
                    self.visit_display_part_list(node.modifier_nodes(), " ", id_to_symbol, parts);
                    parts.push(label_part(" "));
                }
                self.visit_for_display_parts(node.name(), id_to_symbol, parts);
                if node.postfix_token().is_some() {
                    parts.push(label_part(token_to_string(node.postfix_token().kind())));
                }
                self.visit_parameters_and_type_parameters(node, id_to_symbol, parts);
                if node.type_().is_some() {
                    parts.push(label_part(": "));
                    self.visit_for_display_parts(node.type_(), id_to_symbol, parts);
                }
            }
            SyntaxKind::CallSignature => {
                self.visit_parameters_and_type_parameters(node, id_to_symbol, parts);
                if node.type_().is_some() {
                    parts.push(label_part(": "));
                    self.visit_for_display_parts(node.type_(), id_to_symbol, parts);
                }
            }
            SyntaxKind::ConstructSignature => {
                parts.push(label_part("new "));
                self.visit_parameters_and_type_parameters(node, id_to_symbol, parts);
                if node.type_().is_some() {
                    parts.push(label_part(": "));
                    self.visit_for_display_parts(node.type_(), id_to_symbol, parts);
                }
            }
            SyntaxKind::ArrayBindingPattern => {
                parts.push(label_part("["));
                self.visit_display_part_list(node.elements(), ", ", id_to_symbol, parts);
                parts.push(label_part("]"));
            }
            SyntaxKind::ObjectBindingPattern => {
                parts.push(label_part("{"));
                if !node.elements().is_empty() {
                    parts.push(label_part(" "));
                    self.visit_display_part_list(node.elements(), ", ", id_to_symbol, parts);
                    parts.push(label_part(" "));
                }
                parts.push(label_part("}"));
            }
            SyntaxKind::BindingElement => {
                self.visit_for_display_parts(node.name(), id_to_symbol, parts);
            }
            SyntaxKind::PrefixUnaryExpression => {
                parts.push(label_part(token_to_string(node.operator())));
                self.visit_for_display_parts(node.operand(), id_to_symbol, parts);
            }
            SyntaxKind::TemplateLiteralType => {
                self.visit_for_display_parts(node.head(), id_to_symbol, parts);
                for span in node.template_spans().nodes() {
                    self.visit_for_display_parts(span, id_to_symbol, parts);
                }
            }
            SyntaxKind::TemplateHead => {
                parts.push(label_part(&self.get_literal_text(node)));
            }
            SyntaxKind::TemplateLiteralTypeSpan => {
                self.visit_for_display_parts(node.type_(), id_to_symbol, parts);
                self.visit_for_display_parts(node.literal(), id_to_symbol, parts);
            }
            SyntaxKind::TemplateMiddle | SyntaxKind::TemplateTail => {
                parts.push(label_part(&self.get_literal_text(node)));
            }
            SyntaxKind::ThisType => {
                parts.push(label_part("this"));
            }
            SyntaxKind::ComputedPropertyName => {
                parts.push(label_part("["));
                self.visit_for_display_parts(node.expression(), id_to_symbol, parts);
                parts.push(label_part("]"));
            }
            SyntaxKind::PropertyAccessExpression => {
                self.visit_for_display_parts(node.expression(), id_to_symbol, parts);
                parts.push(label_part("."));
                self.visit_for_display_parts(node.name(), id_to_symbol, parts);
            }
            SyntaxKind::ElementAccessExpression => {
                self.visit_for_display_parts(node.expression(), id_to_symbol, parts);
                parts.push(label_part("["));
                self.visit_for_display_parts(node.argument_expression(), id_to_symbol, parts);
                parts.push(label_part("]"));
            }
            _ => crate::gostd::debug::fail_bad_syntax_kind(node.kind(), None),
        }
    }

    // Go: ls/inlay_hints.go:765 visitDisplayPartList (closure in getInlayHintLabelParts)
    fn visit_display_part_list(
        &self,
        nodes: NodeSlice,
        separator: &str,
        id_to_symbol: &FxHashMap<Node, SymbolId>,
        parts: &mut Vec<lsproto::InlayHintLabelPart>,
    ) {
        for (i, n) in nodes.iter().enumerate() {
            if i > 0 {
                parts.push(label_part(separator));
            }
            self.visit_for_display_parts(n, id_to_symbol, parts);
        }
    }

    // Go: ls/inlay_hints.go:774 visitParametersAndTypeParameters (closure in getInlayHintLabelParts)
    fn visit_parameters_and_type_parameters(
        &self,
        node: Node,
        id_to_symbol: &FxHashMap<Node, SymbolId>,
        parts: &mut Vec<lsproto::InlayHintLabelPart>,
    ) {
        if !node.type_parameters().is_empty() {
            parts.push(label_part("<"));
            self.visit_display_part_list(node.type_parameters(), ", ", id_to_symbol, parts);
            parts.push(label_part(">"));
        }
        parts.push(label_part("("));
        self.visit_display_part_list(node.parameters(), ", ", id_to_symbol, parts);
        parts.push(label_part(")"));
    }

    // Go: ls/inlay_hints.go:789 inlayHintState.getNodeDisplayPart
    fn get_node_display_part(&self, text: &str, node: Node) -> lsproto::InlayHintLabelPart {
        let file = get_source_file_of_node(node);
        let pos = astnav::get_start_of_node(node, file, false /*includeJSDoc*/);
        let end = node.end();
        let mut part = lsproto::InlayHintLabelPart {
            value: text.to_string(),
            ..Default::default()
        };
        // The location is an optional go-to target for the name. Only attach it when the name maps back to a
        // single concrete span in the original text; an approximate or synthesized mapping would point the
        // user somewhere wrong, so it is better to omit the target than to fabricate one.
        let (lsp_range, fidelity) = self.converters.to_lsp_range_for_feature(
            &file,
            TextRange::new(pos, end),
            Feature::INLAY_HINTS,
        );
        if fidelity.is_single_segment() {
            part.location = Some(lsproto::Location {
                uri: lsconv::file_name_to_document_uri(source_file_original_file_name(file)),
                range: lsp_range,
            });
        }
        part
    }

    // Go: ls/inlay_hints.go:806 inlayHintState.getLiteralText
    fn get_literal_text(&self, node: Node) -> String {
        match node.kind() {
            SyntaxKind::StringLiteral => {
                if self.quote_preference == lsutil::QuotePreference::SINGLE {
                    return format!("'{}'", escape_string(node.text(), QuoteChar::SINGLE_QUOTE));
                }
                return format!(
                    "\"{}\"",
                    escape_string(node.text(), QuoteChar::DOUBLE_QUOTE)
                );
            }
            SyntaxKind::TemplateHead | SyntaxKind::TemplateMiddle | SyntaxKind::TemplateTail => {
                let mut raw_text = node.raw_text().to_string();
                if raw_text.is_empty() {
                    raw_text = escape_string(node.text(), QuoteChar::BACKTICK);
                }
                match node.kind() {
                    SyntaxKind::TemplateHead => return format!("`{raw_text}${{"),
                    SyntaxKind::TemplateMiddle => return format!("}}{raw_text}${{"),
                    SyntaxKind::TemplateTail => return format!("}}{raw_text}`"),
                    _ => {}
                }
            }
            _ => {}
        }
        node.text().to_string()
    }

    // Go: ls/inlay_hints.go:836 inlayHintState.getParameterIdentifierInfoAtPosition
    fn get_parameter_identifier_info_at_position(
        &mut self,
        signature: SignatureId,
        pos: i32,
    ) -> Option<ParameterInfo> {
        let parameters = self.checker.sig(signature).parameters().to_vec();
        let param_count = parameters.len() as i32
            - if self.checker.sig(signature).has_rest_parameter() {
                1
            } else {
                0
            };
        if pos < param_count {
            let param = parameters[pos as usize];
            let param_id = get_parameter_declaration_identifier(&self.checker.symbols, param);
            if param_id.is_nil() {
                return None;
            }
            return Some(ParameterInfo {
                parameter: param_id,
                name: param_id.text().to_string(),
                is_rest_parameter: false,
            });
        }

        let mut rest_parameter = SymbolId::NIL;
        let mut rest_id = Node::NIL;
        if param_count < parameters.len() as i32 {
            rest_parameter = parameters[param_count as usize];
            rest_id = get_parameter_declaration_identifier(&self.checker.symbols, rest_parameter);
        }
        if rest_id.is_nil() {
            return None;
        }

        let rest_type = self.checker.get_type_of_symbol_exported(rest_parameter);
        if self.checker.is_tuple_type(rest_type) {
            let target = self.checker.ty(rest_type).target();
            let element_infos = self
                .checker
                .ty(target)
                .as_tuple_type()
                .element_infos()
                .to_vec();
            let mut associated_names: Vec<Node> = Vec::with_capacity(element_infos.len());
            for element_info in &element_infos {
                let labeled_element = element_info.labeled_declaration();
                associated_names.push(labeled_element);
            }
            let index = pos - param_count;
            if index < associated_names.len() as i32 {
                let associated_name = associated_names[index as usize];
                if associated_name.is_some() {
                    crate::go_assert!(is_identifier(associated_name.name()));
                    let is_rest_tuple_element;
                    if is_named_tuple_member(associated_name) {
                        is_rest_tuple_element = associated_name.dot_dot_dot_token().is_some();
                    } else {
                        is_rest_tuple_element = associated_name.dot_dot_dot_token().is_some();
                    }
                    return Some(ParameterInfo {
                        parameter: associated_name.name(),
                        name: associated_name.name().text().to_string(),
                        is_rest_parameter: is_rest_tuple_element,
                    });
                }
            }

            return None;
        }

        if pos == param_count {
            return Some(ParameterInfo {
                parameter: rest_id,
                name: self.checker.sym(rest_parameter).name.as_str().to_string(),
                is_rest_parameter: true,
            });
        }
        None
    }

    // Go: ls/inlay_hints.go:918 inlayHintState.leadingCommentsContainsParameterName
    fn leading_comments_contains_parameter_name(&self, node: Node, name: &str) -> bool {
        if !is_identifier_text(name, source_file_info(self.file).language_variant) {
            return false;
        }

        let ranges = get_leading_comment_ranges_of_node(node, self.file);
        let file_text = source_file_text(self.file);
        for r in ranges {
            // PORT: Go `strings.TrimFunc` with `unicode.IsSpace`; Rust
            // `char::is_whitespace` is the same Unicode White_Space set.
            let comment_text = file_text[r.pos() as usize..r.end() as usize]
                .trim_matches(|r: char| r.is_whitespace() || r == '/' || r == '*');
            if comment_text == name {
                return true;
            }
        }

        false
    }

    // Go: ls/inlay_hints.go:937 inlayHintState.getTypeAnnotationPosition
    fn get_type_annotation_position(&self, decl: Node) -> i32 {
        let close_paren_token =
            astnav::find_child_of_kind(decl, SyntaxKind::CloseParenToken, self.file);
        if close_paren_token.is_some() {
            return close_paren_token.end();
        }
        decl.parameter_list().end()
    }
}

/// Go `&lsproto.InlayHintLabelPart{Value: v}`.
// PORT: a local helper for the Go composite literal used in every case above.
fn label_part(value: &str) -> lsproto::InlayHintLabelPart {
    lsproto::InlayHintLabelPart {
        value: value.to_string(),
        ..Default::default()
    }
}

// Go: ls/inlay_hints.go:398 shouldShowParameterNameHints
fn should_show_parameter_name_hints(preferences: &lsutil::InlayHintsPreferences) -> bool {
    preferences.include_inlay_parameter_name_hints
        == lsutil::IncludeInlayParameterNameHints::LITERALS
        || preferences.include_inlay_parameter_name_hints
            == lsutil::IncludeInlayParameterNameHints::ALL
}

// Go: ls/inlay_hints.go:403 shouldShowLiteralParameterNameHintsOnly
fn should_show_literal_parameter_name_hints_only(
    preferences: &lsutil::InlayHintsPreferences,
) -> bool {
    preferences.include_inlay_parameter_name_hints
        == lsutil::IncludeInlayParameterNameHints::LITERALS
}

// Go: ls/inlay_hints.go:408 isSignatureSupportingReturnAnnotation
// node is FunctionDeclaration | ArrowFunction | FunctionExpression | MethodDeclaration | GetAccessor
fn is_signature_supporting_return_annotation(node: Node) -> bool {
    is_arrow_function(node)
        || is_function_expression(node)
        || is_function_declaration(node)
        || is_method_declaration(node)
        || is_get_accessor_declaration(node)
}

// Go: ls/inlay_hints.go:413 isHintableDeclaration
fn is_hintable_declaration(node: Node) -> bool {
    if (is_part_of_parameter_declaration(node)
        || is_variable_declaration(node) && is_var_const(node))
        && node.initializer().is_some()
    {
        let initializer = skip_parentheses(node.initializer());
        return !(is_hintable_literal(initializer)
            || is_new_expression(initializer)
            || is_object_literal_expression(initializer)
            || is_assertion_expression(initializer));
    }
    true
}

// Go: ls/inlay_hints.go:423 isHintableLiteral
// PORT: Go calls `ast.IsInfinityOrNaNString`, which has no ast port; the
// checker port `is_infinity_or_nan_string` has the same body.
fn is_hintable_literal(node: Node) -> bool {
    match node.kind() {
        SyntaxKind::PrefixUnaryExpression => {
            let operand = node.operand();
            return is_literal_expression(operand)
                || is_identifier(operand)
                    && crate::checker::is_infinity_or_nan_string(operand.text());
        }
        SyntaxKind::TrueKeyword
        | SyntaxKind::FalseKeyword
        | SyntaxKind::NullKeyword
        | SyntaxKind::NoSubstitutionTemplateLiteral
        | SyntaxKind::TemplateExpression => {
            return true;
        }
        SyntaxKind::Identifier => {
            let name = node.text();
            return name == "undefined" || crate::checker::is_infinity_or_nan_string(name);
        }
        _ => {}
    }
    is_literal_expression(node)
}

// Go: ls/inlay_hints.go:438 isModuleReferenceType
// PORT: types and symbols live in the checker arenas, so the checker is a parameter.
fn is_module_reference_type(c: &Checker, t: TypeId) -> bool {
    let symbol = c.ty(t).symbol();
    symbol.is_some() && c.sym(symbol).flags.intersects(SymbolFlags::MODULE)
}

// Go: ls/inlay_hints.go:901 getParameterDeclarationIdentifier
// PORT: symbols live in the checker arena, so it is a parameter (PORTING
// "Names": functions that take a symbol get `symbols: &SymbolArena`).
fn get_parameter_declaration_identifier(symbols: &SymbolArena, symbol: SymbolId) -> Node {
    let value_declaration = symbols.sym(symbol).value_declaration;
    if value_declaration.is_some()
        && is_parameter_declaration(value_declaration)
        && is_identifier(value_declaration.name())
    {
        return value_declaration.name();
    }
    Node::NIL
}

// Go: ls/inlay_hints.go:908 identifierOrAccessExpressionPostfixMatchesParameterName
fn identifier_or_access_expression_postfix_matches_parameter_name(
    expr: Node,
    parameter_name: &str,
) -> bool {
    if is_identifier(expr) {
        return expr.text() == parameter_name;
    }
    if is_property_access_expression(expr) {
        return expr.name().text() == parameter_name;
    }
    false
}

// Go: ls/inlay_hints.go:945 isAnyInlayHintEnabled
fn is_any_inlay_hint_enabled(preferences: &lsutil::InlayHintsPreferences) -> bool {
    preferences.include_inlay_parameter_name_hints != lsutil::IncludeInlayParameterNameHints::NONE
        || preferences
            .include_inlay_function_parameter_type_hints
            .is_true()
        || preferences.include_inlay_variable_type_hints.is_true()
        || preferences
            .include_inlay_property_declaration_type_hints
            .is_true()
        || preferences
            .include_inlay_function_like_return_type_hints
            .is_true()
        || preferences.include_inlay_enum_member_value_hints.is_true()
}
