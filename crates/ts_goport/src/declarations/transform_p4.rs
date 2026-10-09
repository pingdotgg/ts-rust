//! Port of `transformers/declarations/transform.go` lines 2153 to 2939
//! (`walkBindingPattern` to the end of the file).
//!
//! PORT: this file is a child module of `declarations::transform` (like
//! `transform_p3.rs`), so it can read the private `DeclarationTransformer`
//! fields and helpers.
//!
//! PORT: Go keeps `*ast.NodeVisitor` fields made by
//! `EmitContext.NewNodeVisitor` (bindingNameVisitor, expressionVisitor,
//! cjsExportAssignmentVisitor). Each use here builds one on demand with
//! `EmitContext::new_node_visitor` and the same callback (`with_tx_visitor`),
//! so the emit context hooks are attached as in Go. The two whole-tree walks
//! (`visit_cjs_export_assignments`, `visit_nested_expression`) build one
//! visitor per walk and recurse through it. Go
//! `tx.Visitor().Visit(n)` calls the callback directly, so it is `self.visit(n)`.
//!
//! PORT: Go `setupDiagnosticContext` returns a cleanup closure. Here it
//! returns `CleanupDiagnosticContext`, and `cleanup.run(self)` restores it. Go
//! runs the cleanup in a `defer`, so every return path below runs it.

use crate::ast::visitor::{NodeVisitor, syntax_list_children};
use crate::checker::nodebuilder_types::SymbolTracker;
use crate::declarations::SymbolTrackerSharedState;
use crate::declarations::diagnostics::{
    create_diagnostic_for_node, create_get_symbol_accessibility_diagnostic_for_node,
};
use crate::declarations::transform::{
    DECLARATION_EMIT_INTERNAL_NODE_BUILDER_FLAGS, DECLARATION_EMIT_NODE_BUILDER_FLAGS,
    DeclarationTransformer,
};
use crate::declarations::util::{
    can_produce_diagnostics, can_reuse_modifier_nodes, get_binding_name_visible, is_always_type,
    is_declaration_and_not_visible, mask_modifier_flags, should_emit_function_properties,
    unwrap_parenthesized_expression,
};
use crate::prelude::*;
use crate::printer::EmitSymbolTracker;

/// A Go visitor callback on the transformer.
type TxVisitFn = fn(&mut DeclarationTransformer, Node) -> Node;

/// Builds the visitor that Go keeps as a transformer field, with `visit` as
/// its callback, and runs `f` with it. See the module comment.
fn with_tx_visitor<R>(
    tx: &mut DeclarationTransformer,
    visit: TxVisitFn,
    f: impl FnOnce(&mut NodeVisitor<'_, &mut DeclarationTransformer>) -> R,
) -> R {
    let ec = tx.emit_context.clone();
    let mut v = ec.new_node_visitor(
        move |node, v: &mut NodeVisitor<'_, &mut DeclarationTransformer>| visit(&mut *v.ctx, node),
        tx,
    );
    f(&mut v)
}

/// Go `tx.tracker` passed to a resolver call.
fn emit_tracker(tx: &DeclarationTransformer) -> EmitSymbolTracker {
    let tracker: Rc<dyn SymbolTracker> = tx.tracker.clone();
    Some(tracker)
}

// Go: transformers/declarations/transform.go:2680 DeclarationTransformer.visitCJSExportAssignments
/// The body of `visit_cjs_export_assignments` and the callback of its walk
/// visitor `v` (Go `tx.cjsExportAssignmentVisitor`). `v.ctx` is `tx`.
fn visit_cjs_export_assignments_in(
    expression: Node,
    v: &mut NodeVisitor<'_, &mut DeclarationTransformer>,
) -> Node {
    if expression.is_some() {
        let tx = &mut *v.ctx;
        let (_, cleanup) = tx.setup_diagnostic_context(expression);
        if get_assignment_declaration_kind(expression) == JSDeclarationKind::MODULE_EXPORTS {
            let current_source_file = tx.state.borrow().current_source_file;
            if source_file_info(current_source_file)
                .common_js_module_indicator
                .is_some()
            {
                let result = tx.transform_export_assignment(
                    expression.parent(),
                    expression,
                    expression.right(),
                    true, /*isExportEquals*/
                );
                if result.is_some() {
                    tx.cjs_export_assignment = result;
                    tx.result_has_scope_marker = true;
                    tx.result_has_external_module_indicator = true;
                }
            }
        }
        // recur through the whole tree, looking for module.exports=
        let result = v.visit_each_child(expression);
        cleanup.run(&mut *v.ctx);
        return result;
    }
    Node::NIL
}

// Go: transformers/declarations/transform.go:2700 DeclarationTransformer.visitNestedExpression
/// The body of `visit_nested_expression` and the callback of its walk
/// visitor `v` (Go `tx.expressionVisitor`). `v.ctx` is `tx`.
fn visit_nested_expression_in(
    expression: Node,
    v: &mut NodeVisitor<'_, &mut DeclarationTransformer>,
) -> Node {
    if expression.is_some() {
        let tx = &mut *v.ctx;
        let (_, cleanup) = tx.setup_diagnostic_context(expression);
        let kind = get_assignment_declaration_kind(expression);
        if kind == JSDeclarationKind::PROPERTY {
            tx.transform_expando_assignment(expression);
        } else if kind == JSDeclarationKind::EXPORTS_PROPERTY {
            let current_source_file = tx.state.borrow().current_source_file;
            if source_file_info(current_source_file)
                .common_js_module_indicator
                .is_some()
            {
                let name = tx.get_name_expression_preferring_identifier(
                    get_element_or_property_access_name(expression.left()),
                );
                let result = tx.transform_common_js_export(expression, name);
                if result.is_some() {
                    tx.cjs_export_members.push(result);
                }
            }
        } else if kind == JSDeclarationKind::OBJECT_DEFINE_PROPERTY_EXPORTS {
            let current_source_file = tx.state.borrow().current_source_file;
            if source_file_info(current_source_file)
                .common_js_module_indicator
                .is_some()
            {
                let name =
                    tx.get_name_expression_preferring_identifier(expression.arguments().get(1));
                let result = tx.transform_common_js_export(expression, name);
                if result.is_some() {
                    tx.cjs_export_members.push(result);
                }
            }
        }
        // recur through the whole tree, looking for special assignments
        let result = v.visit_each_child(expression);
        cleanup.run(&mut *v.ctx);
        return result;
    }
    Node::NIL
}

/// Go `node.FunctionLikeData() != nil && node.FunctionLikeData().FullSignature != nil`.
// PORT: `Node::full_signature` panics on kinds without FunctionLikeData, where
// Go `FunctionLikeData()` returns nil.
fn has_full_signature(node: Node) -> bool {
    matches!(
        node.kind(),
        SyntaxKind::FunctionDeclaration
            | SyntaxKind::CallSignature
            | SyntaxKind::ConstructSignature
            | SyntaxKind::Constructor
            | SyntaxKind::GetAccessor
            | SyntaxKind::SetAccessor
            | SyntaxKind::IndexSignature
            | SyntaxKind::MethodSignature
            | SyntaxKind::MethodDeclaration
            | SyntaxKind::ArrowFunction
            | SyntaxKind::FunctionExpression
            | SyntaxKind::FunctionType
            | SyntaxKind::ConstructorType
            | SyntaxKind::JsDocSignature
    ) && node.full_signature().is_some()
}

impl DeclarationTransformer {
    // Go: transformers/declarations/transform.go:2194 DeclarationTransformer.walkBindingPattern
    pub(crate) fn walk_binding_pattern(&mut self, pattern: Node, param: Node) -> Vec<Node> {
        let mut elems: Vec<Node> = Vec::new();
        for elem in pattern.elements().iter() {
            if is_omitted_expression(elem) {
                continue;
            }
            if is_binding_pattern(elem.name()) {
                let nested = self.walk_binding_pattern(elem.name(), param);
                elems.extend(nested);
                continue;
            }
            let ec = self.emit_context.clone();
            let modifiers = self.ensure_modifiers(param);
            let type_node = self.ensure_type(elem, false);
            elems.push(ec.factory().new_property_declaration(
                modifiers,
                elem.name(),
                Node::NIL, /*questionOrExclamationToken*/
                type_node,
                Node::NIL, /*initializer*/
            ));
        }
        elems
    }

    // Go: transformers/declarations/transform.go:2215 DeclarationTransformer.transformVariableStatement
    pub(crate) fn transform_variable_statement(&mut self, input: Node) -> Node {
        let mut visible = false;
        for decl in input.declaration_list().declarations().nodes().iter() {
            visible = get_binding_name_visible(&*self.resolver, decl);
            if visible {
                break;
            }
        }
        if !visible {
            return Node::NIL;
        }

        let mut input_nodes: Vec<Node> = input.declaration_list().declarations().nodes().to_vec();
        let mut extra_imports: Vec<Node> = Vec::new();
        let current_source_file = self.state.borrow().current_source_file;
        if source_file_info(current_source_file)
            .common_js_module_indicator
            .is_some()
        {
            let mut normal_declarations: Vec<Node> = Vec::new();
            let mut imports: Vec<Node> = Vec::new();
            for &n in &input_nodes {
                if is_variable_declaration_initialized_to_require(n) {
                    imports.push(n);
                } else {
                    normal_declarations.push(n);
                }
            }
            input_nodes = normal_declarations;
            extra_imports = with_tx_visitor(self, DeclarationTransformer::visit, |v| {
                v.visit_slice_changed(imports.iter().copied())
            })
            .unwrap_or(imports);
        }

        let nodes = with_tx_visitor(self, DeclarationTransformer::visit, |v| {
            v.visit_slice_changed(input_nodes.iter().copied())
        })
        .unwrap_or(input_nodes);
        if nodes.is_empty() {
            if !extra_imports.is_empty() {
                return self.emit_context.factory().new_syntax_list(&extra_imports);
            }
            return Node::NIL;
        }
        let ec = self.emit_context.clone();
        let f = ec.factory();
        let node_list = f.new_node_list(&nodes);

        let modifiers = self.ensure_modifiers(input);

        let decl_list;
        if is_var_using(input.declaration_list()) || is_var_await_using(input.declaration_list()) {
            decl_list = f.new_variable_declaration_list(node_list, NodeFlags::CONST);
            ec.set_original(decl_list, input.declaration_list());
            ec.set_comment_range(decl_list, input.declaration_list().loc());
            set_node_loc(decl_list, input.declaration_list().loc());
        } else {
            decl_list = f.update_variable_declaration_list(
                input.declaration_list(),
                node_list,
                input.declaration_list().flags(),
            );
        }
        let res = f.update_variable_statement(input, modifiers, decl_list);
        if !extra_imports.is_empty() {
            extra_imports.push(res);
            return f.new_syntax_list(&extra_imports);
        }
        res
    }

    // Go: transformers/declarations/transform.go:2270 DeclarationTransformer.transformEnumDeclaration
    pub(crate) fn transform_enum_declaration(&mut self, input: Node) -> Node {
        let ec = self.emit_context.clone();
        let f = ec.factory();
        let modifiers = self.ensure_modifiers(input);
        let name = input.name();
        // Go core.MapNonNil over input.Members.Nodes
        let mut members: Vec<Node> = Vec::new();
        for m in input.members().iter() {
            if self.should_strip_internal(m) {
                continue;
            }

            // Rewrite enum values to their constants, if available
            let enum_value = self.resolver.get_enum_member_value(m);

            let isolated_declarations = self.state.borrow().isolated_declarations;
            if isolated_declarations && m.initializer().is_some() && enum_value.has_external_references &&
                // This will be its own compiler error instead, so don't report.
                !is_computed_property_name(m.name())
            {
                self.state.borrow_mut().add_diagnostic(create_diagnostic_for_node(
                    m,
                    diag::Enum_member_initializers_must_be_computable_without_references_to_external_symbols_with_isolatedDeclarations,
                    args![],
                ));
            }

            let new_initializer = match &enum_value.value {
                Some(LiteralValue::Number(value)) => {
                    let value = *value;
                    if value.is_infinite() {
                        if value.0 > 0.0 {
                            f.new_identifier("Infinity")
                        } else {
                            f.new_prefix_unary_expression(
                                SyntaxKind::MinusToken,
                                f.new_identifier("Infinity"),
                            )
                        }
                    } else if value.is_nan() {
                        f.new_identifier("NaN")
                    } else if value.0 >= 0.0 {
                        f.new_numeric_literal(value.to_string(), TokenFlags::NONE)
                    } else {
                        f.new_prefix_unary_expression(
                            SyntaxKind::MinusToken,
                            f.new_numeric_literal((-value).to_string(), TokenFlags::NONE),
                        )
                    }
                }
                Some(LiteralValue::String(value)) => {
                    f.new_string_literal(value.clone(), TokenFlags::NONE)
                }
                // nil
                _ => Node::NIL,
            };
            let result = f.update_enum_member(m, m.name(), new_initializer);
            self.preserve_js_doc(result, m);
            members.push(result);
        }
        f.update_enum_declaration(input, modifiers, name, f.new_node_list(&members))
    }

    // Go: transformers/declarations/transform.go:2321 DeclarationTransformer.ensureModifiers
    pub(crate) fn ensure_modifiers(&mut self, node: Node) -> ModifierList {
        let ec = self.emit_context.clone();
        let f = ec.factory();
        let current_flags = get_combined_modifier_flags(ec.parse_node(node)) & ModifierFlags::ALL;
        let new_flags = self.ensure_modifier_flags(node);
        if current_flags == new_flags {
            // Elide decorators
            let mods = node.modifiers();
            if mods.is_nil() {
                return mods;
            }
            let mod_nodes = mods.nodes().to_vec();
            if can_reuse_modifier_nodes(&mod_nodes) {
                let filtered: Vec<Node> =
                    mod_nodes.into_iter().filter(|&m| is_modifier(m)).collect();
                return f.new_modifier_list(&filtered);
            }
        }
        let result = create_modifiers_from_modifier_flags(new_flags, &mut |k| f.new_modifier(k));
        if result.is_empty() {
            return ModifierList::NIL;
        }
        f.new_modifier_list(&result)
    }

    // Go: transformers/declarations/transform.go:2341 DeclarationTransformer.ensureModifierFlags
    pub(crate) fn ensure_modifier_flags(&mut self, node: Node) -> ModifierFlags {
        // PORT: Go `All ^ (Public | Async | Override)`; those bits are in All, so XOR clears them.
        let mut mask = ModifierFlags::ALL
            .without(ModifierFlags::PUBLIC | ModifierFlags::ASYNC | ModifierFlags::OVERRIDE); // No async and override modifiers in declaration files
        let mut additions = ModifierFlags::NONE;
        if self.needs_declare && !is_always_type(node) {
            additions = ModifierFlags::AMBIENT;
        }
        let parent_is_file = node.parent().kind() == SyntaxKind::SourceFile;
        if !parent_is_file {
            // PORT: Go `mask ^= Ambient`; Ambient is still set in mask, so XOR clears it.
            mask = mask.without(ModifierFlags::AMBIENT);
            additions = ModifierFlags::NONE;
        }
        if is_implicitly_exported_js_doc_declaration(node) {
            additions |= ModifierFlags::EXPORT;
        }
        mask_modifier_flags(node, mask, additions)
    }

    // Go: transformers/declarations/transform.go:2358 DeclarationTransformer.ensureTypeParams
    pub(crate) fn ensure_type_params(&mut self, node: Node, params: NodeList) -> NodeList {
        let ec = self.emit_context.clone();
        if !self
            .resolver
            .get_effective_declaration_flags(ec.parse_node(node), ModifierFlags::PRIVATE)
            .is_empty()
        {
            return NodeList::NIL;
        }
        let mut type_parameters = with_tx_visitor(self, DeclarationTransformer::visit, |v| {
            v.visit_nodes(params)
        });
        if type_parameters.is_some() {
            return type_parameters;
        }
        let old_error_name_node = self.state.borrow().error_name_node;
        self.state.borrow_mut().error_name_node = node.name();
        let mut old_diag = None;
        if !self.suppress_new_diagnostic_contexts {
            old_diag = self
                .state
                .borrow()
                .get_symbol_accessibility_diagnostic
                .clone();
            if can_produce_diagnostics(node) {
                self.state.borrow_mut().get_symbol_accessibility_diagnostic =
                    Some(create_get_symbol_accessibility_diagnostic_for_node(node));
            }
        }

        if has_full_signature(node) {
            let nodes = self
                .resolver
                .create_type_parameters_of_signature_declaration(
                    node,
                    self.enclosing_declaration,
                    DECLARATION_EMIT_NODE_BUILDER_FLAGS,
                    DECLARATION_EMIT_INTERNAL_NODE_BUILDER_FLAGS,
                    emit_tracker(self),
                );
            // PORT: Go tests `nodes != nil`. The Rust resolver returns a Vec,
            // and the Go node builder returns a nil slice when there are no
            // type parameters, so an empty Vec is Go nil.
            if !nodes.is_empty() {
                type_parameters = new_synthetic_node_list(&nodes, node.loc());
            }
        }

        self.state.borrow_mut().error_name_node = old_error_name_node;
        if !self.suppress_new_diagnostic_contexts {
            self.state.borrow_mut().get_symbol_accessibility_diagnostic = old_diag;
        }
        type_parameters
    }

    // Go: transformers/declarations/transform.go:2392 DeclarationTransformer.updateParamList
    pub(crate) fn update_param_list(&mut self, node: Node, params: NodeList) -> NodeList {
        let ec = self.emit_context.clone();
        if !self
            .resolver
            .get_effective_declaration_flags(ec.parse_node(node), ModifierFlags::PRIVATE)
            .is_empty()
            || params.nodes().is_empty()
        {
            return ec.factory().new_node_list(&[]);
        }
        let mut results: Vec<Node> = Vec::with_capacity(params.nodes().len());
        for p in params.nodes().iter() {
            results.push(self.ensure_parameter(p));
        }
        ec.factory().new_node_list(&results)
    }

    // Go: transformers/declarations/transform.go:2403 DeclarationTransformer.ensureParameter
    pub(crate) fn ensure_parameter(&mut self, p: Node) -> Node {
        let ec = self.emit_context.clone();
        let f = ec.factory();
        let old_diag = self
            .state
            .borrow()
            .get_symbol_accessibility_diagnostic
            .clone();
        if !self.suppress_new_diagnostic_contexts {
            self.state.borrow_mut().get_symbol_accessibility_diagnostic =
                Some(create_get_symbol_accessibility_diagnostic_for_node(p));
        }
        let mut question_token = Node::NIL;
        if self.resolver.is_optional_parameter(p) {
            if p.question_token().is_some() {
                question_token = p.question_token();
            } else {
                question_token = f.new_token(SyntaxKind::QuestionToken);
            }
        }
        let name = with_tx_visitor(self, DeclarationTransformer::visit_binding_name, |v| {
            v.visit_node(p.name())
        });
        let type_node = self.ensure_type(p, true);
        let initializer = self.ensure_no_initializer(p);
        let result = f.update_parameter_declaration(
            p,
            ModifierList::NIL,
            p.dot_dot_dot_token(),
            name,
            question_token,
            type_node,
            initializer,
        );
        self.state.borrow_mut().get_symbol_accessibility_diagnostic = old_diag;
        result
    }

    // Go: transformers/declarations/transform.go:2429 DeclarationTransformer.ensureNoInitializer
    pub(crate) fn ensure_no_initializer(&mut self, node: Node) -> Node {
        if self.should_print_with_initializer(node) {
            let unwrapped_initializer = unwrap_parenthesized_expression(node.initializer());
            if !is_primitive_literal_value(unwrapped_initializer, true) {
                self.tracker.report_inference_fallback(node);
            }
            let ec = self.emit_context.clone();
            return self
                .resolver
                .create_literal_const_value(ec.parse_node(node), emit_tracker(self));
        }
        Node::NIL
    }

    // Go: transformers/declarations/transform.go:2440 DeclarationTransformer.visitBindingName
    pub(crate) fn visit_binding_name(&mut self, node: Node) -> Node {
        match node.kind() {
            SyntaxKind::Identifier | SyntaxKind::OmittedExpression => node,
            SyntaxKind::ArrayBindingPattern | SyntaxKind::ObjectBindingPattern => {
                with_tx_visitor(self, DeclarationTransformer::visit_binding_name, |v| {
                    v.visit_each_child(node)
                })
            }
            SyntaxKind::BindingElement => {
                if node.property_name().is_some()
                    && is_computed_property_name(node.property_name())
                    && is_entity_name_expression(node.property_name().expression())
                {
                    self.check_entity_name_visibility(
                        node.property_name().expression(),
                        self.enclosing_declaration,
                    );
                }
                let name = with_tx_visitor(self, DeclarationTransformer::visit_binding_name, |v| {
                    v.visit_node(node.name())
                });
                let ec = self.emit_context.clone();
                ec.factory().update_binding_element(
                    node,
                    node.dot_dot_dot_token(),
                    node.property_name(),
                    name,
                    Node::NIL, /*initializer*/
                )
            }
            _ => node,
        }
    }

    // Go: transformers/declarations/transform.go:2456 DeclarationTransformer.transformImportEqualsDeclaration
    pub(crate) fn transform_import_equals_declaration(&mut self, decl: Node) -> Node {
        if !self.resolver.is_declaration_visible(decl) {
            return Node::NIL;
        }
        if decl.module_reference().kind() == SyntaxKind::ExternalModuleReference {
            // Rewrite external module names if necessary
            let specifier = get_external_module_import_equals_declaration_expression(decl);
            let rewritten = self.rewrite_module_specifier(decl, specifier);
            let ec = self.emit_context.clone();
            let f = ec.factory();
            f.update_import_equals_declaration(
                decl,
                decl.modifiers(),
                decl.is_type_only(),
                decl.name(),
                f.update_external_module_reference(decl.module_reference(), rewritten),
            )
        } else {
            let old_diag = self
                .state
                .borrow()
                .get_symbol_accessibility_diagnostic
                .clone();
            self.state.borrow_mut().get_symbol_accessibility_diagnostic =
                Some(create_get_symbol_accessibility_diagnostic_for_node(decl));
            self.check_entity_name_visibility(decl.module_reference(), self.enclosing_declaration);
            self.state.borrow_mut().get_symbol_accessibility_diagnostic = old_diag;
            decl
        }
    }

    // Go: transformers/declarations/transform.go:2479 DeclarationTransformer.transformImportDeclaration
    pub(crate) fn transform_import_declaration(&mut self, decl: Node) -> Node {
        let ec = self.emit_context.clone();
        let f = ec.factory();
        if decl.import_clause().is_nil() {
            // import "mod" - possibly needed for side effects? (global interface patches, module augmentations, etc)
            let module_specifier = self.rewrite_module_specifier(decl, decl.module_specifier());
            let attributes = decl.attributes();
            return f.update_import_declaration(
                decl,
                decl.modifiers(),
                decl.import_clause(),
                module_specifier,
                attributes,
            );
        }
        let mut phase_modifier = decl.import_clause().phase_modifier();
        if phase_modifier == SyntaxKind::DeferKeyword {
            phase_modifier = SyntaxKind::Unknown;
        }
        // The `importClause` visibility corresponds to the default's visibility.
        let mut visible_default_binding = Node::NIL;
        if decl.import_clause().is_some()
            && decl.import_clause().name().is_some()
            && self.resolver.is_declaration_visible(decl.import_clause())
        {
            visible_default_binding = decl.import_clause().name();
        }
        if decl.import_clause().named_bindings().is_nil() {
            // No named bindings (either namespace or list), meaning the import is just default or should be elided
            if visible_default_binding.is_nil() {
                return Node::NIL;
            }
            let import_clause = f.update_import_clause(
                decl.import_clause(),
                phase_modifier,
                visible_default_binding,
                Node::NIL, /*namedBindings*/
            );
            let module_specifier = self.rewrite_module_specifier(decl, decl.module_specifier());
            let attributes = decl.attributes();
            return f.update_import_declaration(
                decl,
                decl.modifiers(),
                import_clause,
                module_specifier,
                attributes,
            );
        }
        if decl.import_clause().named_bindings().kind() == SyntaxKind::NamespaceImport {
            // Namespace import (optionally with visible default)
            let mut named_bindings = Node::NIL;
            if self
                .resolver
                .is_declaration_visible(decl.import_clause().named_bindings())
            {
                named_bindings = decl.import_clause().named_bindings();
            }
            if visible_default_binding.is_nil() && named_bindings.is_nil() {
                return Node::NIL;
            }
            let import_clause = f.update_import_clause(
                decl.import_clause(),
                phase_modifier,
                visible_default_binding,
                named_bindings,
            );
            let module_specifier = self.rewrite_module_specifier(decl, decl.module_specifier());
            let attributes = decl.attributes();
            return f.update_import_declaration(
                decl,
                decl.modifiers(),
                import_clause,
                module_specifier,
                attributes,
            );
        }
        // Named imports (optionally with visible default)
        let binding_list: Vec<Node> = decl
            .import_clause()
            .named_bindings()
            .elements()
            .iter()
            .filter(|&b| self.resolver.is_declaration_visible(b))
            .collect();
        if !binding_list.is_empty() || visible_default_binding.is_some() {
            let mut named_imports = Node::NIL;
            if !binding_list.is_empty() {
                named_imports = f.update_named_imports(
                    decl.import_clause().named_bindings(),
                    f.new_node_list(&binding_list),
                );
            }
            let import_clause = f.update_import_clause(
                decl.import_clause(),
                phase_modifier,
                visible_default_binding,
                named_imports,
            );
            let module_specifier = self.rewrite_module_specifier(decl, decl.module_specifier());
            let attributes = decl.attributes();
            return f.update_import_declaration(
                decl,
                decl.modifiers(),
                import_clause,
                module_specifier,
                attributes,
            );
        }
        // Augmentation of export depends on import
        if self.resolver.is_import_required_by_augmentation(decl) {
            let isolated_declarations = self.state.borrow().isolated_declarations;
            if isolated_declarations {
                self.state.borrow_mut().add_diagnostic(create_diagnostic_for_node(
                    decl,
                    diag::Declaration_emit_for_this_file_requires_preserving_this_import_for_augmentations_This_is_not_supported_with_isolatedDeclarations,
                    args![],
                ));
            }
            let module_specifier = self.rewrite_module_specifier(decl, decl.module_specifier());
            let attributes = decl.attributes();
            return f.update_import_declaration(
                decl,
                decl.modifiers(),
                Node::NIL, /*importClause*/
                module_specifier,
                attributes,
            );
        }
        // Nothing visible
        Node::NIL
    }

    // Go: transformers/declarations/transform.go:2584 DeclarationTransformer.transformJSDocTypeExpression
    pub(crate) fn transform_js_doc_type_expression(&mut self, input: Node) -> Node {
        self.visit(input.type_())
    }

    // Go: transformers/declarations/transform.go:2588 DeclarationTransformer.transformJSDocTypeLiteral
    pub(crate) fn transform_js_doc_type_literal(&mut self, input: Node) -> Node {
        let tags = input.js_doc_property_tags();
        let members = with_tx_visitor(self, DeclarationTransformer::visit, |v| {
            v.visit_slice_changed(tags.iter().copied())
        })
        .unwrap_or(tags);
        let ec = self.emit_context.clone();
        let f = ec.factory();
        let replacement = f.new_type_literal_node(f.new_node_list(&members));
        ec.set_original(replacement, input);
        replacement
    }

    // Go: transformers/declarations/transform.go:2595 DeclarationTransformer.transformJSDocPropertyTag
    pub(crate) fn transform_js_doc_property_tag(&mut self, input: Node) -> Node {
        let name = self.visit(input.tag_name());
        let type_node = self.visit(input.type_expression());
        let ec = self.emit_context.clone();
        let replacement = ec.factory().new_property_signature_declaration(
            ModifierList::NIL,
            name,
            Node::NIL,
            type_node,
            Node::NIL,
        );
        ec.set_original(replacement, input);
        replacement
    }

    // Go: transformers/declarations/transform.go:2607 DeclarationTransformer.transformJSDocAllType
    pub(crate) fn transform_js_doc_all_type(&mut self, input: Node) -> Node {
        let ec = self.emit_context.clone();
        let replacement = ec.factory().new_keyword_type_node(SyntaxKind::AnyKeyword);
        ec.set_original(replacement, input);
        replacement
    }

    // Go: transformers/declarations/transform.go:2613 DeclarationTransformer.transformJSDocNullableType
    pub(crate) fn transform_js_doc_nullable_type(&mut self, input: Node) -> Node {
        let type_node = self.visit(input.type_());
        let ec = self.emit_context.clone();
        let f = ec.factory();
        let replacement = f.new_union_type_node(f.new_node_list(&[
            type_node,
            f.new_literal_type_node(f.new_keyword_expression(SyntaxKind::NullKeyword)),
        ]));
        ec.set_original(replacement, input);
        replacement
    }

    // Go: transformers/declarations/transform.go:2622 DeclarationTransformer.transformJSDocNonNullableType
    pub(crate) fn transform_js_doc_non_nullable_type(&mut self, input: Node) -> Node {
        self.visit(input.type_())
    }

    // Go: transformers/declarations/transform.go:2626 DeclarationTransformer.transformJSDocVariadicType
    pub(crate) fn transform_js_doc_variadic_type(&mut self, input: Node) -> Node {
        let type_node = self.visit(input.type_());
        let ec = self.emit_context.clone();
        let replacement = ec.factory().new_array_type_node(type_node);
        ec.set_original(replacement, input);
        replacement
    }

    // Go: transformers/declarations/transform.go:2632 DeclarationTransformer.transformJSDocOptionalType
    pub(crate) fn transform_js_doc_optional_type(&mut self, input: Node) -> Node {
        let type_node = self.visit(input.type_());
        let ec = self.emit_context.clone();
        let f = ec.factory();
        let replacement = f.new_union_type_node(f.new_node_list(&[
            type_node,
            f.new_keyword_type_node(SyntaxKind::UndefinedKeyword),
        ]));
        ec.set_original(replacement, input);
        replacement
    }

    // Go: transformers/declarations/transform.go:2641 DeclarationTransformer.getNameExpressionPreferringIdentifier
    pub(crate) fn get_name_expression_preferring_identifier(
        &mut self,
        mut name_expr: Node,
    ) -> Node {
        let ec = self.emit_context.clone();
        let f = ec.factory();
        if is_numeric_literal(name_expr) {
            // Numeric property names are string properties in JS; convert to string literal
            name_expr = f.new_string_literal(name_expr.text(), TokenFlags::NONE);
        }
        if is_string_literal_like(name_expr)
            && is_identifier_text(&name_expr.text(), LanguageVariant::STANDARD)
        {
            let result = f.new_identifier(name_expr.text()); // prefer non-string literal names where possible
            let kw_kind = identifier_to_keyword_kind(result);
            // keep keywords as strings, except `default`, which has special reformulations in the transformer
            if kw_kind == SyntaxKind::Unknown || kw_kind == SyntaxKind::DefaultKeyword {
                // fake this into a parse tree node so the reference resolver resolves the node via `resolveName`
                set_node_parent(result, name_expr.parent());
                set_node_flags(result, result.flags().without(NodeFlags::SYNTHESIZED));
                // intentionally leave Loc unset so the string isn't used as the text source of the identifier
                return result;
            }
        }
        name_expr
    }

    // Go: transformers/declarations/transform.go:2665 DeclarationTransformer.stripDeclareModifiers
    pub(crate) fn strip_declare_modifiers(&mut self, node: Node) -> Node {
        if node.is_nil() {
            return Node::NIL;
        }
        let mods = node.modifiers();
        if mods.is_some() {
            let flags = node.modifier_flags();
            if flags.intersects(ModifierFlags::AMBIENT) {
                let filtered: Vec<Node> = mods
                    .nodes()
                    .iter()
                    .filter(|&m| is_not_declare_modifier(m))
                    .collect();
                set_node_modifiers(
                    node,
                    self.emit_context.factory().new_modifier_list(&filtered),
                );
            }
        }
        node // no need to recur into children, only strip at top-level
    }

    // Go: transformers/declarations/transform.go:2680 DeclarationTransformer.visitCJSExportAssignments
    // PERF: Go recurses through the one `tx.cjsExportAssignmentVisitor`. This
    // entry makes one visitor for the whole walk; its callback
    // `visit_cjs_export_assignments_in` recurses through the visitor it gets,
    // so no visitor (an `Rc` closure) is made per node.
    pub(crate) fn visit_cjs_export_assignments(&mut self, expression: Node) -> Node {
        let ec = self.emit_context.clone();
        let mut v = ec.new_node_visitor(visit_cjs_export_assignments_in, self);
        visit_cjs_export_assignments_in(expression, &mut v)
    }

    // Go: transformers/declarations/transform.go:2700 DeclarationTransformer.visitNestedExpression
    // PERF: one visitor per walk, like `visit_cjs_export_assignments`. Go
    // keeps it in `tx.expressionVisitor`.
    pub(crate) fn visit_nested_expression(&mut self, expression: Node) -> Node {
        let ec = self.emit_context.clone();
        let mut v = ec.new_node_visitor(visit_nested_expression_in, self);
        visit_nested_expression_in(expression, &mut v)
    }

    // Go: transformers/declarations/transform.go:2727 DeclarationTransformer.transformExpandoAssignment
    pub(crate) fn transform_expando_assignment(&mut self, node: Node) {
        let left = node.left();

        let symbol = node.symbol();
        // PORT: `symbol.Flags` of a binder symbol, read from the program's
        // binder symbols (`program::bound_symbols`).
        if symbol.is_nil()
            || !crate::program::bound_symbols()
                .sym(symbol)
                .flags
                .intersects(SymbolFlags::ASSIGNMENT)
        {
            return;
        }

        let ns = get_leftmost_access_expression(left);
        if ns.is_nil() || ns.kind() != SyntaxKind::Identifier {
            return;
        }

        let declaration = self.resolver.get_referenced_value_declaration(ns);
        if declaration.is_nil() {
            return;
        }

        if self.should_strip_internal(declaration) {
            return;
        }

        if is_variable_declaration(declaration) && declaration.type_().is_some() {
            return;
        }

        if is_function_declaration(declaration) && declaration.full_signature().is_some() {
            return;
        }

        if is_variable_declaration(declaration) && !is_function_like(declaration.initializer()) {
            return; // We're going to add a type, no need to dupe members with a namespace
        }

        let host = declaration.symbol();
        if host.is_nil() {
            return;
        }

        let ec = self.emit_context.clone();
        let f = ec.factory();
        let name = f.new_identifier(ns.text());
        let property = self.try_get_property_name(left);
        if property.is_empty() || !is_identifier_text(&property, LanguageVariant::STANDARD) {
            return;
        }

        let host_id = self.get_expando_host_id(declaration);

        if is_declaration(declaration)
            && is_declaration_and_not_visible(&ec, &*self.resolver, declaration)
        {
            // The host isn't visible (yet) - printing the type of a visible declaration may still
            // late-mark it as visible (e.g. an exported variable whose type prints as `typeof host`),
            // so defer the assignment to be processed if and when that happens.
            self.deferred_expando_assignments
                .entry(host_id)
                .or_default()
                .push(node);
            return;
        }

        if is_function_declaration(declaration) && !should_emit_function_properties(declaration) {
            return;
        }

        self.transform_expando_host(name, declaration);

        let export_name = f.new_identifier(property.clone());
        let mut local_name = self.try_get_name_of_assigned_expression(node);
        if local_name.is_nil()
            && !self
                .resolver
                .is_name_resolvable(self.enclosing_declaration, &property)
            && !is_non_contextual_keyword(string_to_token(&export_name.text()))
        {
            // use exportName as localName if there won't be any conflicts or keyword issues
            local_name = export_name;
        }
        if local_name.is_nil() || is_non_contextual_keyword(string_to_token(&local_name.text())) {
            // fallback to a generated name if the localName doesn't exist or is a keyword
            local_name = f.new_generated_name_for_node(node);
        }

        let (_, cleanup) = self.setup_diagnostic_context(node);

        let preexisting_expando_has_export = self
            .expando_members
            .get(&host_id)
            .is_some_and(|members| members.iter().any(|&m| is_export_declaration(m)));

        if is_identifier(node.right()) {
            if !preexisting_expando_has_export {
                self.add_export_modifier_to_expando_members(host_id);
            }
            // alias-like, emit an `export {name}` or `export {name as alias}`
            let result = self.transform_binary_expression_to_export_declaration(node, export_name);
            self.expando_members
                .entry(host_id)
                .or_default()
                .push(result);
            cleanup.run(self);
            return;
        }

        let mut var_modifiers = ModifierList::NIL;

        if preexisting_expando_has_export {
            var_modifiers = f.new_modifier_list(&create_modifiers_from_modifier_flags(
                ModifierFlags::EXPORT,
                &mut |k| f.new_modifier(k),
            ));
        }

        let synthesized_namespace = f.new_module_declaration(
            ModifierList::NIL, /*modifiers*/
            SyntaxKind::NamespaceKeyword,
            name,
            Node::NIL,
            f.new_module_block(f.new_node_list(&[])),
        );
        set_node_parent(synthesized_namespace, self.enclosing_declaration);
        set_node_symbol(synthesized_namespace, host);
        // Go: containerData.Locals = make(ast.SymbolTable, 0); Locals[localName.Text()] = symbol
        let locals = self
            .resolver
            .make_symbol_table(&[(local_name.text(), symbol)]);
        set_node_locals(synthesized_namespace, locals);

        let old_enclosing = self.enclosing_declaration;
        self.enclosing_declaration = synthesized_namespace;

        let type_node = self.ensure_type(node, false);
        let mut statements: Vec<Node> = vec![f.new_variable_statement(
            var_modifiers,
            f.new_variable_declaration_list(
                f.new_node_list(&[f.new_variable_declaration(
                    local_name,
                    Node::NIL, /*exclamationToken*/
                    type_node,
                    Node::NIL, /*initializer*/
                )]),
                NodeFlags::NONE,
            ),
        )];

        if local_name.text() != export_name.text() {
            let named_exports = f.new_named_exports(f.new_node_list(&[f.new_export_specifier(
                false, /*isTypeOnly*/
                local_name,
                export_name,
            )]));
            statements.push(f.new_export_declaration(
                ModifierList::NIL, /*modifiers*/
                false,             /*isTypeOnly*/
                named_exports,
                Node::NIL, /*moduleSpecifier*/
                Node::NIL, /*attributes*/
            ));
            if !preexisting_expando_has_export {
                // Done before adding statements to expando members to keep the initial variable statement, before we rename anything, private
                self.add_export_modifier_to_expando_members(host_id);
            }
        }

        self.expando_members
            .entry(host_id)
            .or_default()
            .extend(statements);

        // Go defers run last-in first-out.
        self.enclosing_declaration = old_enclosing;
        cleanup.run(self);
    }

    // Go: transformers/declarations/transform.go:2862 DeclarationTransformer.addExportModifierToExpandoMembers
    // PORT: Go keys the members by `ast.NodeId`; the Rust maps are keyed by
    // the host node (`get_expando_host_id`).
    pub(crate) fn add_export_modifier_to_expando_members(&mut self, host_id: Node) {
        let ec = self.emit_context.clone();
        let f = ec.factory();
        // Add an `export` modifier to all existing expando members so they remain exported after the `export {}` is added
        let existing = self
            .expando_members
            .get(&host_id)
            .cloned()
            .unwrap_or_default();
        for decl in existing {
            // only invoked when `tx.expandoMembers` does not *yet* contain an `export` declaration, so no need to skip one here to prevent `export export {}`
            let modifier_flags = ModifierFlags::EXPORT | get_combined_modifier_flags(decl);
            let modifiers = f.new_modifier_list(&create_modifiers_from_modifier_flags(
                modifier_flags,
                &mut |k| f.new_modifier(k),
            ));
            set_node_modifiers(decl, modifiers);
        }
    }

    // Go: transformers/declarations/transform.go:2871 DeclarationTransformer.getExpandoHostId
    // PORT: Go returns `ast.GetNodeId(mostOriginal)`. The Rust maps are keyed
    // by `Node`, so this returns the node. `get_node_id` still runs for its
    // id assignment side effect.
    pub(crate) fn get_expando_host_id(&mut self, declaration: Node) -> Node {
        let root = if is_variable_declaration(declaration) {
            declaration.parent().parent()
        } else {
            declaration
        };
        let original = self.emit_context.most_original(root);
        let _ = get_node_id(original);
        original
    }

    // Go: transformers/declarations/transform.go:2877 DeclarationTransformer.transformExpandoHost
    pub(crate) fn transform_expando_host(&mut self, name: Node, declaration: Node) {
        let root = if is_variable_declaration(declaration) {
            declaration.parent().parent()
        } else {
            declaration
        };
        let id = self.get_expando_host_id(declaration);

        if self.expando_hosts.contains_key(&id) {
            return;
        }

        let save_needs_declare = self.needs_declare;
        self.needs_declare = true;

        let mut modifier_flags = self.ensure_modifier_flags(root);
        let default_export = modifier_flags.intersects(ModifierFlags::EXPORT)
            && modifier_flags.intersects(ModifierFlags::DEFAULT);

        self.needs_declare = save_needs_declare;

        if default_export {
            modifier_flags |= ModifierFlags::AMBIENT;
            // PORT: Go `^= Default` and `^= Export`; both are set here, so XOR clears them.
            modifier_flags = modifier_flags.without(ModifierFlags::DEFAULT);
            modifier_flags = modifier_flags.without(ModifierFlags::EXPORT);
        }

        let (_, cleanup) = self.setup_diagnostic_context(declaration);

        let ec = self.emit_context.clone();
        let f = ec.factory();
        let modifiers = f.new_modifier_list(&create_modifiers_from_modifier_flags(
            modifier_flags,
            &mut |k| f.new_modifier(k),
        ));
        let mut replacement: Vec<Node> = Vec::new();

        if is_function_declaration(declaration) {
            let (type_parameters, parameters, asterisk_token) =
                extract_expando_host_params(declaration);
            let type_params = self.ensure_type_params(declaration, type_parameters);
            let params = self.update_param_list(declaration, parameters);
            let type_node = self.ensure_type(declaration, false);
            replacement.push(f.update_function_declaration(
                declaration,
                modifiers,
                asterisk_token,
                declaration.name(),
                type_params,
                params,
                type_node,
                Node::NIL, /*fullSignature*/
                Node::NIL, /*body*/
            ));
        } else if is_variable_declaration(declaration)
            && is_function_expression_or_arrow_function(declaration.initializer())
        {
            let func = declaration.initializer();
            let (type_parameters, parameters, asterisk_token) = extract_expando_host_params(func);
            let fn_name = f.new_identifier(name.text());
            let type_params = self.ensure_type_params(func, type_parameters);
            let params = self.update_param_list(func, parameters);
            let type_node = self.ensure_type(func, false);
            replacement.push(f.new_function_declaration(
                modifiers,
                asterisk_token,
                fn_name,
                type_params,
                params,
                type_node,
                Node::NIL, /*fullSignature*/
                Node::NIL, /*body*/
            ));
        } else {
            let result = self.transform_top_level_declaration(declaration);
            self.expando_hosts.insert(id, result);
            cleanup.run(self);
            return;
        }

        // PORT: Go `p.ValueDeclaration` goes through the resolver, which
        // holds the checker arena (see `report_expando_function_errors`).
        let resolver = self.resolver.clone();
        SymbolTrackerSharedState::report_expando_function_errors(
            &self.state,
            declaration,
            &mut |n| {
                let props = resolver.get_properties_of_container_function(n);
                props
                    .into_iter()
                    .map(|p| resolver.symbol_value_declaration(p))
                    .collect()
            },
        );

        if default_export {
            if is_source_file(declaration.parent()) {
                self.result_has_external_module_indicator = true;
            }
            self.result_has_scope_marker = true;
            replacement.push(f.new_export_assignment(
                ModifierList::NIL, /*modifiers*/
                false,             /*isExportEquals*/
                Node::NIL,         /*typeNode*/
                name,
            ));
        }

        // store host result to be added to the output when it's actually visited
        self.expando_hosts
            .insert(id, f.new_syntax_list(&replacement));
        if self.late_statement_replacement_map.contains_key(&id) {
            let block = self.create_full_expando_block(id);
            self.late_statement_replacement_map.insert(id, block);
        }
        cleanup.run(self);
    }

    // Go: transformers/declarations/transform.go:2934 DeclarationTransformer.createFullExpandoBlock
    pub(crate) fn create_full_expando_block(&mut self, id: Node) -> Node {
        // Process any expando assignments on this host that were skipped because it wasn't
        // visible when they were collected - if it's still not visible, they simply get
        // re-deferred, and are dropped if the host is never late-marked visible.
        if let Some(deferred) = self.deferred_expando_assignments.remove(&id) {
            for assignment in deferred {
                self.transform_expando_assignment(assignment);
            }
        }
        let n = self.expando_hosts.get(&id).copied().unwrap_or(Node::NIL);
        if let Some(add_ons) = self.expando_members.get(&id).cloned() {
            let ec = self.emit_context.clone();
            let f = ec.factory();
            let mut modifiers = ModifierList::NIL;
            let mut name = Node::NIL;
            let mut host: Vec<Node> = Vec::new();
            if n.is_some() && n.kind() == SyntaxKind::SyntaxList {
                // find the first named syntax list element and use its' name & modifiers
                for c in syntax_list_children(n) {
                    if c.name().is_some() {
                        name = f.clone_node(c.name());
                        if c.modifiers().is_some() {
                            modifiers = f.clone_modifier_list(c.modifiers());
                        }
                        break;
                    }
                }
                host = syntax_list_children(n);
            } else if n.is_some() {
                name = f.clone_node(n.name());
                if n.modifiers().is_some() {
                    modifiers = f.clone_modifier_list(n.modifiers());
                }
                host = vec![n];
            }
            if name.is_some() {
                let module_decl = f.new_module_declaration(
                    modifiers,
                    SyntaxKind::NamespaceKeyword,
                    name,
                    Node::NIL,
                    f.new_module_block(f.new_node_list(&add_ons)),
                );
                let mut members = host;
                members.push(module_decl);
                return f.new_syntax_list(&members);
            }
        }
        n
    }

    // Go: transformers/declarations/transform.go:2997 DeclarationTransformer.tryGetPropertyName
    pub(crate) fn try_get_property_name(&mut self, node: Node) -> String {
        if is_element_access_expression(node) {
            return self.resolver.get_element_access_expression_name(node);
        }
        if is_property_access_expression(node) {
            return node.name().text().to_string();
        }
        String::new()
    }
}

// Go: transformers/declarations/transform.go:2661 isNotDeclareModifier
fn is_not_declare_modifier(m: Node) -> bool {
    m.kind() != SyntaxKind::DeclareKeyword
}

// Go: transformers/declarations/transform.go:2983 extractExpandoHostParams
// Returns (typeParameters, parameters, asteriskToken).
fn extract_expando_host_params(node: Node) -> (NodeList, NodeList, Node) {
    // PORT: Go switches on FunctionExpression, ArrowFunction and (default)
    // FunctionDeclaration; the three read the same fields.
    (
        node.type_parameter_list(),
        node.parameter_list(),
        node.asterisk_token(),
    )
}
