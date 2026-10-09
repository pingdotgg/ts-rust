//! Port of `transformers/declarations/transform.go` lines 1487 to 2152
//! (`wrapInCJSExportNamespace` to `collectThisPropertyAssignments`).
//!
//! PORT: this file is a child module of `declarations::transform`, so it can
//! read the private `DeclarationTransformer` fields and helpers.
//!
//! PORT: Go keeps six `*ast.NodeVisitor` fields made by
//! `EmitContext.NewNodeVisitor`. Each use here builds one on demand with
//! `EmitContext::new_node_visitor` and the same callback (`with_tx_visitor`),
//! so the emit context hooks are attached as in Go. Go
//! `tx.Visitor().Visit(n)` calls the callback directly, so it is
//! `self.visit(n)`.

use crate::ast::visitor::{NodeVisitor, syntax_list_children};
use crate::checker::nodebuilder_types::SymbolTracker;
use crate::declarations::SymbolTrackerSharedState;
use crate::declarations::diagnostics::{
    GetSymbolAccessibilityDiagnostic, SymbolAccessibilityDiagnostic, bound_symbol_declarations,
    create_get_symbol_accessibility_diagnostic_for_node,
};
use crate::declarations::transform::{
    DECLARATION_EMIT_INTERNAL_NODE_BUILDER_FLAGS, DECLARATION_EMIT_NODE_BUILDER_FLAGS,
    DeclarationTransformer, ThisPropertyAssignmentKey, get_this_property_assignment_key,
};
use crate::declarations::util::{
    can_have_literal_initializer, can_produce_diagnostics, get_effective_base_type_node,
    has_scope_marker, is_declaration_and_not_visible, is_enclosing_declaration,
};
use crate::prelude::*;
use crate::printer::{
    AutoGenerateOptions, EmitFlags, EmitSymbolTracker, GeneratedIdentifierFlags,
    SymbolAccessibilityResult,
};
use crate::transformers::utilities::is_simple_inlineable_expression;

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

/// Go `tx.Visitor().VisitNodes(list)`.
fn root_visit_nodes(tx: &mut DeclarationTransformer, list: NodeList) -> NodeList {
    with_tx_visitor(tx, DeclarationTransformer::visit, |v| v.visit_nodes(list))
}

/// Go `tx.tracker` passed to a resolver call.
fn emit_tracker(tx: &DeclarationTransformer) -> EmitSymbolTracker {
    let tracker: Rc<dyn SymbolTracker> = tx.tracker.clone();
    Some(tracker)
}

impl DeclarationTransformer {
    // Go: transformers/declarations/transform.go:1536 DeclarationTransformer.wrapInCJSExportNamespace
    pub(crate) fn wrap_in_cjs_export_namespace(&mut self, content: Node) -> Node {
        if self.cjs_export_assignment_name.is_nil() {
            return content;
        }
        // Reuse the same name node so unique names resolve consistently with the class/export
        let ns_name = self.cjs_export_assignment_name;
        let mut members: Vec<Node> = if content.kind() == SyntaxKind::SyntaxList {
            syntax_list_children(content)
        } else {
            vec![content]
        };
        let ec = self.emit_context.clone();
        let mut ns_mods: Vec<Node> = Vec::new();
        if self.needs_declare {
            ns_mods.push(ec.factory().new_modifier(SyntaxKind::DeclareKeyword));
        }
        // PORT: `strip_declare_modifiers` is Go `stripDeclareModifiers` (transform.go:2624).
        members = with_tx_visitor(self, DeclarationTransformer::strip_declare_modifiers, |v| {
            v.visit_slice(&members).0
        });
        let f = ec.factory();
        f.new_module_declaration(
            f.new_modifier_list(&ns_mods),
            SyntaxKind::NamespaceKeyword,
            ns_name,
            Node::NIL,
            f.new_module_block(f.new_node_list(&members)),
        )
    }

    // Go: transformers/declarations/transform.go:1574 DeclarationTransformer.transformClassExpressionToDeclaration
    // transformClassExpressionToDeclaration converts a class expression into a class declaration
    // for use in CJS export declarations (e.g., exports.K = class K {} or module.exports = class Thing {}).
    // This delegates to the shared buildClassMembers helper to stay in sync with transformClassDeclaration.
    pub(crate) fn transform_class_expression_to_declaration(
        &mut self,
        class_expr: Node,
        class_name: Node,
        modifiers: ModifierList,
    ) -> Node {
        let previous_enclosing_declaration = self.enclosing_declaration;
        self.enclosing_declaration = class_expr;
        let previous_in_class_expression_declaration = self.in_class_expression_declaration;
        self.in_class_expression_declaration = true;

        let mut extra_members: Vec<Node> = Vec::new();
        if is_in_js_file(class_expr) {
            extra_members = self.collect_this_property_assignments(class_expr);
        }
        let members = self.build_class_members(class_expr, &extra_members);
        let type_parameters = self.ensure_type_params(class_expr, class_expr.type_parameter_list());
        let heritage_clauses = root_visit_nodes(self, class_expr.heritage_clauses());

        let ec = self.emit_context.clone();
        let result = ec.factory().new_class_declaration(
            modifiers,
            class_name,
            type_parameters,
            heritage_clauses,
            members,
        );
        // PORT: Go restores these in a defer.
        self.enclosing_declaration = previous_enclosing_declaration;
        self.in_class_expression_declaration = previous_in_class_expression_declaration;
        result
    }

    // Go: transformers/declarations/transform.go:1601 DeclarationTransformer.rewriteModuleSpecifier
    pub(crate) fn rewrite_module_specifier(&mut self, parent: Node, input: Node) -> Node {
        if input.is_nil() {
            return Node::NIL;
        }
        self.result_has_external_module_indicator = self.result_has_external_module_indicator
            || (parent.kind() != SyntaxKind::ModuleDeclaration
                && parent.kind() != SyntaxKind::ImportType);
        input
    }

    // Go: transformers/declarations/transform.go:1609 DeclarationTransformer.preserveJsDoc
    pub(crate) fn preserve_js_doc(&mut self, updated: Node, original: Node) {
        // Copy comment range from original to updated node so JSDoc comments are preserved
        self.emit_context.assign_comment_range(updated, original);
    }

    // Go: transformers/declarations/transform.go:1614 DeclarationTransformer.preservePartialJsDoc
    pub(crate) fn preserve_partial_js_doc(&mut self, updated: Node, original: Node) {
        if !original.flags().intersects(NodeFlags::REPARSED) {
            return;
        }
        let jsdoc = original
            .eager_js_doc(get_source_file_of_node(original))
            .first()
            .unwrap_or(Node::NIL);
        if jsdoc.is_nil() {
            return;
        }
        let description = get_text_of_js_doc_comment(jsdoc.comment());
        if description.is_empty() {
            return;
        }
        let comment = "*\n * ".to_string() + &description.replace('\n', "\n * ") + "\n ";
        self.emit_context.add_synthetic_leading_comment(
            updated,
            SyntaxKind::MultiLineCommentTrivia,
            &comment,
            true, /*hasTrailingNewLine*/
        );
    }

    // Go: transformers/declarations/transform.go:1630 DeclarationTransformer.removeAllComments
    pub(crate) fn remove_all_comments(&mut self, node: Node) {
        self.emit_context
            .add_emit_flags(node, EmitFlags::NO_COMMENTS);
        // !!! TODO: Also remove synthetic trailing/leading comments added by transforms
        // emitNode.leadingComments = undefined;
        // emitNode.trailingComments = undefined;
    }

    // Go: transformers/declarations/transform.go:1637 DeclarationTransformer.ensureType
    pub(crate) fn ensure_type(&mut self, node: Node, ignore_private: bool) -> Node {
        let ec = self.emit_context.clone();
        if !ignore_private
            && !self
                .resolver
                .get_effective_declaration_flags(ec.parse_node(node), ModifierFlags::PRIVATE)
                .is_empty()
        {
            // Private nodes emit no types (except private parameter properties, whose parameter types are actually visible)
            return Node::NIL;
        }

        if self.should_print_with_initializer(node) {
            // Literal const declarations will have an initializer ensured rather than a type
            return Node::NIL;
        }

        // Should be removed createTypeOfDeclaration will actually now reuse the existing annotation so there is no real need to duplicate type walking
        // Left in for now to minimize diff during syntactic type node builder refactor
        if !is_export_assignment(node)
            && !is_binding_element(node)
            && node.type_().is_some()
            && (!is_parameter_declaration(node)
                || !self.resolver.requires_adding_implicit_undefined(
                    node,
                    SymbolId::NIL,
                    self.enclosing_declaration,
                ))
        {
            let current_source_file = self.state.borrow().current_source_file;
            if is_source_file_js(current_source_file) {
                // JS types have a heap of constructs we can't directly emit into .d.ts files; the node builder contains logic to remap those where possible, so we invoke it here
                // In strada we always built js declarations symbolically, so all js type nodes went through this postprocessing
                let mut js_flags = DECLARATION_EMIT_NODE_BUILDER_FLAGS;
                if self.in_class_expression_declaration {
                    js_flags =
                        js_flags.without(NodeBuilderFlags::WRITE_CLASS_EXPRESSION_AS_TYPE_LITERAL);
                }
                let res = self.resolver.try_js_type_node_to_type_node(
                    node.type_(),
                    self.enclosing_declaration,
                    js_flags,
                    DECLARATION_EMIT_INTERNAL_NODE_BUILDER_FLAGS,
                    emit_tracker(self),
                );
                if res.is_some() {
                    return res;
                }
                // otherwise, fall back to full serialization
            } else {
                return self.visit(node.type_());
            }
        }

        let old_error_name_node = self.state.borrow().error_name_node;
        self.state.borrow_mut().error_name_node = node.name();
        let mut old_diag: Option<GetSymbolAccessibilityDiagnostic> = None;
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
        let mut flags = DECLARATION_EMIT_NODE_BUILDER_FLAGS;
        if self.in_class_expression_declaration {
            flags = flags.without(NodeBuilderFlags::WRITE_CLASS_EXPRESSION_AS_TYPE_LITERAL);
        }
        // PORT: Go starts `typeNode` at nil and assigns it in each branch.
        let type_node = if has_inferred_type(node) {
            self.resolver.create_type_of_declaration(
                node,
                self.enclosing_declaration,
                flags,
                DECLARATION_EMIT_INTERNAL_NODE_BUILDER_FLAGS,
                emit_tracker(self),
            )
        } else if is_function_like(node) {
            self.resolver.create_return_type_of_signature_declaration(
                node,
                self.enclosing_declaration,
                flags,
                DECLARATION_EMIT_INTERNAL_NODE_BUILDER_FLAGS,
                emit_tracker(self),
            )
        } else {
            crate::gostd::debug::assert_never(&crate::gostd::debug::kind_string(node.kind()), None)
        };

        self.state.borrow_mut().error_name_node = old_error_name_node;
        if !self.suppress_new_diagnostic_contexts {
            self.state.borrow_mut().get_symbol_accessibility_diagnostic = old_diag;
        }
        if type_node.is_nil() {
            return ec.factory().new_keyword_type_node(SyntaxKind::AnyKeyword);
        }
        type_node
    }

    // Go: transformers/declarations/transform.go:1701 DeclarationTransformer.shouldPrintWithInitializer
    pub(crate) fn should_print_with_initializer(&mut self, node: Node) -> bool {
        can_have_literal_initializer(&*self.resolver, node)
            && node.initializer().is_some()
            && self
                .resolver
                .is_literal_const_declaration(self.emit_context.most_original(node))
    }

    // Go: transformers/declarations/transform.go:1705 DeclarationTransformer.checkEntityNameVisibility
    pub(crate) fn check_entity_name_visibility(
        &mut self,
        entity_name: Node,
        enclosing_declaration: Node,
    ) {
        let visibility_result = self
            .resolver
            .is_entity_name_visible(entity_name, enclosing_declaration);
        self.tracker
            .handle_symbol_accessibility_error(visibility_result);
    }

    // Go: transformers/declarations/transform.go:1711 DeclarationTransformer.transformTopLevelDeclaration
    // Transforms the direct child of a source file into zero or more replacement statements
    pub(crate) fn transform_top_level_declaration(&mut self, input: Node) -> Node {
        {
            let mut state = self.state.borrow_mut();
            if !state.late_marked_statements.is_empty() {
                // Remove duplicates of the current statement from the deferred work queue (this was done via orderedRemoveItem in strada - why? to ensure the same backing array? microop?)
                state.late_marked_statements.retain(|&node| node != input);
            }
        }
        if self.should_strip_internal(input) {
            return Node::NIL;
        }
        if input.kind() == SyntaxKind::ImportEqualsDeclaration {
            return self.transform_import_equals_declaration(input);
        }
        if input.kind() == SyntaxKind::ImportDeclaration
            || input.kind() == SyntaxKind::JsImportDeclaration
        {
            let res = self.transform_import_declaration(input);
            if res.is_some() && res.kind() != SyntaxKind::ImportDeclaration {
                let res = self
                    .emit_context
                    .factory()
                    .as_node_factory()
                    .clone_node(res);
                set_node_kind(res, SyntaxKind::ImportDeclaration);
                return res;
            }
            return res;
        }
        let ec = self.emit_context.clone();
        if is_declaration(input) && is_declaration_and_not_visible(&ec, &*self.resolver, input) {
            return Node::NIL;
        }

        // !!! TODO: JSDoc support
        // if (isJSDocImportTag(input)) return;

        // Elide implementation signatures from overload sets
        if is_function_like(input) && self.resolver.is_implementation_of_overload(input) {
            return Node::NIL;
        }
        // PORT: Go keys `expandoHosts` by `ast.NodeId`; it is keyed by `Node`.
        let original = ec.most_original(input);
        let is_expando_host = self.expando_hosts.contains_key(&original);
        let has_deferred_expando_assignments =
            self.deferred_expando_assignments.contains_key(&original);
        if is_expando_host || has_deferred_expando_assignments {
            return self.create_full_expando_block(original);
        }

        let previous_enclosing_declaration = self.enclosing_declaration;
        if is_enclosing_declaration(input) {
            self.enclosing_declaration = input;
        }

        let can_produce_diagnostic = can_produce_diagnostics(input);
        let (old_diag, old_name) = {
            let state = self.state.borrow();
            (
                state.get_symbol_accessibility_diagnostic.clone(),
                state.error_name_node,
            )
        };
        if can_produce_diagnostic {
            self.state.borrow_mut().get_symbol_accessibility_diagnostic =
                Some(create_get_symbol_accessibility_diagnostic_for_node(input));
        }
        let save_needs_declare = self.needs_declare;

        let result = match input.kind() {
            SyntaxKind::TypeAliasDeclaration | SyntaxKind::JsTypeAliasDeclaration => {
                self.transform_type_alias_declaration(input)
            }
            SyntaxKind::InterfaceDeclaration => self.transform_interface_declaration(input),
            SyntaxKind::FunctionDeclaration => self.transform_function_declaration(input),
            SyntaxKind::ModuleDeclaration => self.transform_module_declaration(input),
            SyntaxKind::ClassDeclaration => self.transform_class_declaration(input),
            SyntaxKind::VariableStatement => self.transform_variable_statement(input),
            SyntaxKind::EnumDeclaration => self.transform_enum_declaration(input),
            // Anything left unhandled is an error, so this should be unreachable
            kind => panic!("Unhandled top-level node in declaration emit: {:?}", kind),
        };

        self.enclosing_declaration = previous_enclosing_declaration;
        {
            let mut state = self.state.borrow_mut();
            state.get_symbol_accessibility_diagnostic = old_diag;
            state.error_name_node = old_name;
        }
        self.needs_declare = save_needs_declare;
        result
    }

    // Go: transformers/declarations/transform.go:1791 DeclarationTransformer.transformTypeAliasDeclaration
    pub(crate) fn transform_type_alias_declaration(&mut self, input: Node) -> Node {
        self.needs_declare = false;
        let modifiers = self.ensure_modifiers(input);
        let name = input.name();
        let type_parameters = root_visit_nodes(self, input.type_parameter_list());
        let type_node = self.visit(input.type_());
        let ec = self.emit_context.clone();
        ec.factory().update_type_alias_declaration(
            input,
            modifiers,
            name,
            type_parameters,
            type_node,
        )
    }

    // Go: transformers/declarations/transform.go:1802 DeclarationTransformer.transformInterfaceDeclaration
    pub(crate) fn transform_interface_declaration(&mut self, input: Node) -> Node {
        let modifiers = self.ensure_modifiers(input);
        let name = input.name();
        let type_parameters = root_visit_nodes(self, input.type_parameter_list());
        let heritage_clauses = root_visit_nodes(self, input.heritage_clauses());
        let members = root_visit_nodes(self, input.member_list());
        let ec = self.emit_context.clone();
        ec.factory().update_interface_declaration(
            input,
            modifiers,
            name,
            type_parameters,
            heritage_clauses,
            members,
        )
    }

    // Go: transformers/declarations/transform.go:1813 DeclarationTransformer.transformFunctionDeclaration
    pub(crate) fn transform_function_declaration(&mut self, input: Node) -> Node {
        if self.resolver.is_expando_function_declaration(input) {
            // PORT: Go `p.ValueDeclaration` goes through the resolver, which
            // holds the checker arena (see `report_expando_function_errors`).
            let resolver = self.resolver.clone();
            SymbolTrackerSharedState::report_expando_function_errors(
                &self.state,
                input,
                &mut |n| {
                    let props = resolver.get_properties_of_container_function(n);
                    props
                        .into_iter()
                        .map(|p| resolver.symbol_value_declaration(p))
                        .collect()
                },
            );
        }
        let modifiers = self.ensure_modifiers(input);
        let name = input.name();
        let type_parameters = self.ensure_type_params(input, input.type_parameter_list());
        let parameters = self.update_param_list(input, input.parameter_list());
        let type_node = self.ensure_type(input, false);
        let ec = self.emit_context.clone();
        ec.factory().update_function_declaration(
            input,
            modifiers,
            Node::NIL,
            name,
            type_parameters,
            parameters,
            type_node,
            Node::NIL, /*fullSignature*/
            Node::NIL,
        )
    }

    // Go: transformers/declarations/transform.go:1830 DeclarationTransformer.transformModuleDeclaration
    pub(crate) fn transform_module_declaration(&mut self, input: Node) -> Node {
        // !!! TODO: module declarations are now parsed into nested module objects with export modifiers
        // It'd be good to collapse those back in the declaration output, but the AST can't represent the
        // `namespace a.b.c` shape for the printer (without using invalid identifier names).
        let mods = self.ensure_modifiers(input);
        let save_needs_declare = self.needs_declare;
        self.needs_declare = false;
        let inner = input.body();
        let mut keyword = input.keyword();
        if keyword != SyntaxKind::GlobalKeyword
            && (input.name().is_nil() || !is_string_literal(input.name()))
        {
            keyword = SyntaxKind::NamespaceKeyword;
        }
        let attributes = self.visit(input.attributes());
        let ec = self.emit_context.clone();

        if inner.is_some() && inner.kind() == SyntaxKind::ModuleBlock {
            let old_needs_scope_fix = self.needs_scope_fix_marker;
            let old_has_scope_fix = self.result_has_scope_marker;
            self.result_has_scope_marker = false;
            self.needs_scope_fix_marker = false;
            let statements = root_visit_nodes(self, inner.statement_list());
            let mut late_statements =
                self.transform_and_replace_late_painted_statements(statements);
            if input.flags().intersects(NodeFlags::AMBIENT) {
                self.needs_scope_fix_marker = false; // If it was `declare`'d everything is implicitly exported already, ignore late printed "privates"
            }
            // With the final list of statements, there are 3 possibilities:
            // 1. There's an export assignment or export declaration in the namespace - do nothing
            // 2. Everything is exported and there are no export assignments or export declarations - strip all export modifiers
            // 3. Some things are exported, some are not, and there's no marker - add an empty marker
            let late_nodes = late_statements.nodes().to_vec();
            if !is_global_scope_augmentation(input)
                && !self.result_has_scope_marker
                && !has_scope_marker(if late_statements.is_some() {
                    Some(&late_nodes)
                } else {
                    None
                })
            {
                if self.needs_scope_fix_marker {
                    let mut nodes = late_nodes;
                    // PORT: Go `createEmptyExports` (transform.go:380).
                    nodes.push(crate::declarations::transform::create_empty_exports(
                        &ec.factory().ast,
                    ));
                    late_statements = ec.factory().new_node_list(&nodes);
                } else {
                    late_statements = with_tx_visitor(
                        self,
                        DeclarationTransformer::strip_export_modifiers,
                        |v| v.visit_nodes(late_statements),
                    );
                }
            }

            let body = ec.factory().update_module_block(inner, late_statements);
            self.needs_declare = save_needs_declare;
            self.needs_scope_fix_marker = old_needs_scope_fix;
            self.result_has_scope_marker = old_has_scope_fix;

            return ec.factory().update_module_declaration(
                input,
                mods,
                keyword,
                input.name(),
                attributes,
                body,
            );
        }
        if inner.is_some() {
            // trigger visit. ignore result (is deferred, so is just inner unless elided)
            self.visit(inner);
            // eagerly transform nested namespaces (the nesting doesn't need any elision or painting done)
            // PORT: Go keys `lateStatementReplacementMap` by `ast.NodeId`; it is keyed by `Node`.
            let original = ec.most_original(inner);
            let body = self
                .late_statement_replacement_map
                .remove(&original)
                .unwrap_or(Node::NIL);
            return ec.factory().update_module_declaration(
                input,
                mods,
                keyword,
                input.name(),
                attributes,
                body,
            );
        }
        ec.factory().update_module_declaration(
            input,
            mods,
            keyword,
            input.name(),
            attributes,
            Node::NIL,
        )
    }

    // Go: transformers/declarations/transform.go:1907 DeclarationTransformer.stripExportModifiers
    pub(crate) fn strip_export_modifiers(&mut self, statement: Node) -> Node {
        if statement.is_nil() {
            return Node::NIL;
        }
        let ec = self.emit_context.clone();
        let parse_node = ec.parse_node(statement);
        if is_import_equals_declaration(statement)
            || (parse_node.is_some()
                && !self
                    .resolver
                    .get_effective_declaration_flags(parse_node, ModifierFlags::DEFAULT)
                    .is_empty())
            || !can_have_modifiers(statement)
        {
            // `export import` statements should remain as-is, as imports are _not_ implicitly exported in an ambient namespace
            // Likewise, `export default` classes and the like and just be `default`, so we preserve their `export` modifiers, too
            return statement;
        }

        let old_flags = get_combined_modifier_flags(statement);
        if !old_flags.intersects(ModifierFlags::EXPORT) {
            return statement;
        }
        let new_flags = old_flags & ModifierFlags(ModifierFlags::ALL.0 ^ ModifierFlags::EXPORT.0);
        let modifiers = create_modifiers_from_modifier_flags(new_flags, &mut |kind| {
            ec.factory().new_modifier(kind)
        });
        replace_modifiers(
            ec.factory().as_node_factory(),
            statement,
            ec.factory().new_modifier_list(&modifiers),
        )
    }

    // Go: transformers/declarations/transform.go:1930 DeclarationTransformer.buildClassMembers
    // buildClassMembers builds the member list for a class-like node (ClassDeclaration or ClassExpression).
    // It handles parameter properties, private identifiers, late-bound index signatures, and visited members.
    // Extra members (e.g., this-property assignments from JS files) can be passed via extraMembers.
    pub(crate) fn build_class_members(
        &mut self,
        class_node: Node,
        extra_members: &[Node],
    ) -> NodeList {
        let ec = self.emit_context.clone();
        let ctor = get_first_constructor_with_body(class_node);
        let mut parameter_properties: Vec<Node> = Vec::new();
        if ctor.is_some() {
            let old_diag = self
                .state
                .borrow()
                .get_symbol_accessibility_diagnostic
                .clone();
            for param in ctor.parameters().iter() {
                if !has_syntactic_modifier(param, ModifierFlags::PARAMETER_PROPERTY_MODIFIER)
                    || self.should_strip_internal(param)
                {
                    continue;
                }
                self.state.borrow_mut().get_symbol_accessibility_diagnostic =
                    Some(create_get_symbol_accessibility_diagnostic_for_node(param));
                if param.name().kind() == SyntaxKind::Identifier {
                    let modifiers = self.ensure_modifiers(param);
                    let name = param.name();
                    let question_token = param.question_token();
                    let type_node = self.ensure_type(param, false);
                    let initializer = self.ensure_no_initializer(param);
                    let updated = ec.factory().new_property_declaration(
                        modifiers,
                        name,
                        question_token,
                        type_node,
                        initializer,
                    );
                    self.preserve_js_doc(updated, param);
                    parameter_properties.push(updated);
                } else {
                    // Pattern - this is currently an error, but we emit declarations for it somewhat correctly
                    let elems = self.walk_binding_pattern(param.name(), param);
                    parameter_properties.extend(elems);
                }
            }
            self.state.borrow_mut().get_symbol_accessibility_diagnostic = old_diag;
        }

        // When the class has at least one private identifier, create a unique constant identifier to retain the nominal typing behavior
        // Prevents other classes with the same public members from being used in place of the current class
        let mut private_identifier = Node::NIL;
        if class_node
            .members()
            .iter()
            .any(|member| member.name().is_some() && is_private_identifier(member.name()))
        {
            private_identifier = ec.factory().new_property_declaration(
                ModifierList::NIL,
                ec.factory().new_private_identifier("#private"),
                Node::NIL,
                Node::NIL,
                Node::NIL,
            );
        }

        let late_indexes = self.resolver.create_late_bound_index_signatures(
            class_node,
            self.enclosing_declaration,
            DECLARATION_EMIT_NODE_BUILDER_FLAGS,
            DECLARATION_EMIT_INTERNAL_NODE_BUILDER_FLAGS,
            emit_tracker(self),
        );

        let mut member_nodes: Vec<Node> = Vec::with_capacity(class_node.members().len());
        if private_identifier.is_some() {
            member_nodes.push(private_identifier);
        }
        member_nodes.extend(late_indexes);
        member_nodes.extend(parameter_properties);
        member_nodes.extend_from_slice(extra_members);
        let visit_result = root_visit_nodes(self, class_node.member_list());
        if visit_result.is_some() && !visit_result.nodes().is_empty() {
            member_nodes.extend(visit_result.nodes().iter());
        }
        ec.factory().new_node_list(&member_nodes)
    }

    // Go: transformers/declarations/transform.go:1990 DeclarationTransformer.transformClassDeclaration
    pub(crate) fn transform_class_declaration(&mut self, input: Node) -> Node {
        let previous_enclosing_declaration = self.enclosing_declaration;
        self.enclosing_declaration = input;

        self.state.borrow_mut().error_name_node = input.name();
        self.tracker.push_error_fallback_node(input);

        // PORT: Go restores `enclosingDeclaration` and pops the error fallback
        // node in defers; the labeled block computes the result first.
        let result = 'body: {
            let ec = self.emit_context.clone();
            let modifiers = self.ensure_modifiers(input);
            let type_parameters = self.ensure_type_params(input, input.type_parameter_list());

            // Collect this.x property assignments from constructors and static blocks in JS files
            let mut extra_members: Vec<Node> = Vec::new();
            if is_in_js_file(input) {
                extra_members = self.collect_this_property_assignments(input);
            }

            let members = self.build_class_members(input, &extra_members);

            let extends_clause = get_effective_base_type_node(input);

            if extends_clause.is_some()
                && !is_entity_name_expression(extends_clause.expression())
                && extends_clause.expression().kind() != SyntaxKind::NullKeyword
            {
                self.tracker
                    .report_inference_fallback(extends_clause.expression()); // Add an isolated declarations error on this extends clause
                let mut old_id = "default".to_string();
                if node_is_present(input.name())
                    && is_identifier(input.name())
                    && !input.name().text().is_empty()
                {
                    old_id = input.name().text().to_string();
                }
                let new_id = ec.factory().new_unique_name_ex(
                    &(old_id + "_base"),
                    AutoGenerateOptions {
                        flags: GeneratedIdentifierFlags::OPTIMISTIC,
                        ..Default::default()
                    },
                );
                let input_name = input.name();
                self.state.borrow_mut().get_symbol_accessibility_diagnostic = Some(Rc::new(
                    move |_: &SymbolAccessibilityResult| {
                        Some(SymbolAccessibilityDiagnostic {
                            diagnostic_message: diag::X_extends_clause_of_exported_class_0_has_or_is_using_private_name_1,
                            error_node: extends_clause,
                            type_name: input_name,
                        })
                    },
                ));

                let type_of_expression = self.resolver.create_type_of_expression(
                    extends_clause.expression(),
                    input,
                    DECLARATION_EMIT_NODE_BUILDER_FLAGS,
                    DECLARATION_EMIT_INTERNAL_NODE_BUILDER_FLAGS,
                    emit_tracker(self),
                );
                let f = ec.factory();
                let var_decl =
                    f.new_variable_declaration(new_id, Node::NIL, type_of_expression, Node::NIL);
                let mut mods = ModifierList::NIL;
                if self.needs_declare {
                    mods = f.new_modifier_list(&[f.new_modifier(SyntaxKind::DeclareKeyword)]);
                }
                let statement = f.new_variable_statement(
                    mods,
                    f.new_variable_declaration_list(f.new_node_list(&[var_decl]), NodeFlags::CONST),
                );
                let type_arguments = root_visit_nodes(self, extends_clause.type_argument_list());
                let f = ec.factory();
                let new_heritage_clause = f.update_heritage_clause(
                    extends_clause.parent(),
                    extends_clause.parent().token(),
                    f.new_node_list(&[f.update_expression_with_type_arguments(
                        extends_clause,
                        new_id,
                        type_arguments,
                    )]),
                );
                let retained_heritage_clauses = root_visit_nodes(self, input.heritage_clauses()); // should just be `implements`
                let mut heritage_list = vec![new_heritage_clause];
                if retained_heritage_clauses.is_some()
                    && !retained_heritage_clauses.nodes().is_empty()
                {
                    heritage_list.extend(retained_heritage_clauses.nodes().iter());
                }
                let f = ec.factory();
                let heritage_clauses = f.new_node_list(&heritage_list);

                let class_declaration = f.update_class_declaration(
                    input,
                    modifiers,
                    input.name(),
                    type_parameters,
                    heritage_clauses,
                    members,
                );
                break 'body f.new_syntax_list(&[statement, class_declaration]);
            }

            let heritage_clauses = root_visit_nodes(self, input.heritage_clauses());
            ec.factory().update_class_declaration(
                input,
                modifiers,
                input.name(),
                type_parameters,
                heritage_clauses,
                members,
            )
        };

        self.tracker.pop_error_fallback_node();
        self.enclosing_declaration = previous_enclosing_declaration;
        result
    }

    // Go: transformers/declarations/transform.go:2084 DeclarationTransformer.visitThisPropertyAssignments
    pub(crate) fn visit_this_property_assignments(&mut self, node: Node) -> Node {
        let mut is_static = false;
        let this_container = get_this_container(node, false, false);
        let this_target = this_container.parent();
        if this_target.is_nil() {
            return Node::NIL; // thisContainer was source file, can't have expando-this
        }
        if has_static_modifier(this_container) || is_class_static_block_declaration(this_container)
        {
            is_static = true;
        }
        if this_target != self.enclosing_declaration {
            return Node::NIL; // stop searching within new `this` contexts
        }
        'case_block: {
            if get_assignment_declaration_kind(node) == JSDeclarationKind::THIS_PROPERTY {
                let mut name = get_name_of_declaration(node);
                let base = self.resolver.get_referenced_member_value_declaration(node);
                let key: ThisPropertyAssignmentKey =
                    get_this_property_assignment_key(name, node, is_static);
                if base.is_nil() || self.seen_properties.contains(&key) {
                    break 'case_block;
                }
                self.seen_properties.insert(key);

                // problem: this prop might be overriding a prop from a base type. The checker has special bails for override compat comparisons for binary expression properties,
                // but what we transform to won't - so we either need to match the base type (for example, if it's a getter/setter) or emit nothing
                // See `checkKindsOfPropertyMemberOverrides` in the checker for what we're trying to satisfy here
                let heritage_clauses = this_target.heritage_clauses();
                if heritage_clauses.is_some()
                    && !heritage_clauses.nodes().is_empty()
                    && !is_class_extending_null(this_target)
                {
                    // there is a base type any assignments might be "from"
                    self.tracker.report_inference_fallback(this_target); // Add an isolated declarations error on this class - we can't know how to transform this prop into an assignment without referring to type information
                    if self
                        .resolver
                        .is_this_property_assignment_declaration_redundant(node)
                    {
                        break 'case_block; // skip assignments whose member is already provided by an `extends` base type (an inherited accessor/method, or an identical inherited property)
                        // TODO: If the property has an explicit `@type` annotation, we should probably emit it (maybe with an `override` modifier) instead of skipping it
                    }
                }

                let ec = self.emit_context.clone();
                let mut mods = ModifierList::NIL;
                if is_static {
                    mods = ec
                        .factory()
                        .new_modifier_list(&[ec.factory().new_modifier(SyntaxKind::StaticKeyword)]);
                }
                if has_dynamic_name(node) {
                    if !is_simple_inlineable_expression(name) {
                        break 'case_block; // Member either becomes an index signature or is a reassignment
                    }
                    self.check_name(node);
                    name = ec.factory().new_computed_property_name(name); // Convert `this[foo] = expr` to `[foo]: Type`
                }
                if get_text_of_property_name(name) == "constructor" {
                    break 'case_block; // `constructor` is a builtin class member, not allowed to redeclare it
                }
                if is_identifier(name)
                    && !is_identifier_text(name.text(), LanguageVariant::STANDARD)
                {
                    name = ec.factory().new_string_literal_from_node(name);
                }
                let type_node = self.ensure_type(node, false);
                let prop = ec.factory().new_property_declaration(
                    mods,
                    name,
                    Node::NIL,
                    type_node,
                    Node::NIL,
                );
                if is_expression_statement(node.parent()) {
                    self.preserve_js_doc(prop, node.parent());
                }
                self.this_property_assignments_collected.push(prop);
            }
        }
        with_tx_visitor(
            self,
            DeclarationTransformer::visit_this_property_assignments,
            |v| v.visit_each_child(node),
        )
    }

    // Go: transformers/declarations/transform.go:2171 DeclarationTransformer.collectThisPropertyAssignments
    // collectThisPropertyAssignments finds `this.x = expr` assignments in constructors, methods, and static blocks
    // of JS classes and synthesizes PropertyDeclaration nodes for each unique property name.
    pub(crate) fn collect_this_property_assignments(&mut self, class_node: Node) -> Vec<Node> {
        let members = class_node.members();
        let mut seen: FxHashSet<ThisPropertyAssignmentKey> = FxHashSet::default();
        // Pre-populate seen with existing direct member nodes to avoid duplicates
        for member in members.iter() {
            if member.name().is_some() {
                let is_static = is_static(member);
                seen.insert(get_this_property_assignment_key(
                    member.name(),
                    member,
                    is_static,
                ));
            }
        }
        self.seen_properties = seen;
        self.this_property_assignments_collected = Vec::new();

        for n in members.iter() {
            with_tx_visitor(
                self,
                DeclarationTransformer::visit_this_property_assignments,
                |v| v.visit_each_child(n),
            );
        }
        // PORT: Go clears `seenProperties` and sets the collected slice to nil in
        // defers, after the return value is read.
        let result = std::mem::take(&mut self.this_property_assignments_collected);
        self.seen_properties.clear();
        result
    }
}

// Go: transformers/declarations/transform.go:1562 isCommonJSAliasExport
pub(crate) fn is_common_js_alias_export(node: Node) -> bool {
    if is_binary_expression(node) && is_identifier(node.right()) {
        let symbol = node.symbol();
        // PORT: Go reads `symbol.Declarations` of the bound symbol.
        if symbol != SymbolId::NIL && bound_symbol_declarations(symbol).len() == 1 {
            return true;
        }
    }
    false
}

// Go: transformers/declarations/transform.go:2153 isClassExtendingNull
pub(crate) fn is_class_extending_null(node: Node) -> bool {
    if node.is_nil() {
        return false;
    }
    let extends_clause = get_heritage_clause(node, SyntaxKind::ExtendsKeyword);
    if extends_clause.is_nil() {
        return false;
    }
    let types = extends_clause.types();
    if types.is_nil() || types.nodes().len() != 1 {
        return false;
    }
    let expr = types.nodes().get(0).expression();
    expr.is_some() && expr.kind() == SyntaxKind::NullKeyword
}
