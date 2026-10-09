//! Port of `transformers/declarations/transform.go` lines 753 to 1486:
//! `transformImportTypeNode` through `transformCommonJSExportWorker`.
//!
//! PORT: Go `tx.Visitor()` is the root visitor that `NewTransformer` builds
//! with `EmitContext.NewNodeVisitor(tx.visit)`. Its `Visit` field is
//! `tx.visit`, so Go `tx.Visitor().Visit(n)` is `self.visit(n)` here. The
//! other visitor methods (`VisitNode`, `VisitNodes`, `VisitEachChild`) go
//! through `with_visitor`, which builds an `ast` visitor over the transformer.
//! `with_visitor` uses `EmitContext::new_node_visitor`, so the emit context
//! hooks (`VisitParameters`, `VisitFunctionBody`, ...) are attached as in Go.
//!
//! Methods ported in other files of this module (`rewriteModuleSpecifier`,
//! `ensureType`, `ensureModifiers`, `preserveJsDoc`, ...) are called with their
//! Go snake names.

use super::diagnostics::{SymbolAccessibilityDiagnostic, bound_symbol_declarations};
use super::tracker::SymbolTrackerImpl;
use super::transform::DeclarationTransformer;
use super::transform_p3::is_common_js_alias_export;
use super::util::{
    get_binding_name_visible, is_private_method_type_parameter, unwrap_parenthesized_expression,
};
use crate::ast::visitor::{NodeVisitor, syntax_list_children};
use crate::prelude::*;
use crate::printer::{
    AutoGenerateOptions, EmitSymbolTracker, GeneratedIdentifierFlags, SymbolAccessibilityResult,
};

/// Go `printer.AutoGenerateOptions{Flags: printer.GeneratedIdentifierFlagsOptimistic}`.
fn optimistic() -> AutoGenerateOptions {
    AutoGenerateOptions {
        flags: GeneratedIdentifierFlags::OPTIMISTIC,
        ..Default::default()
    }
}

/// Go `func(_ printer.SymbolAccessibilityResult) *SymbolAccessibilityDiagnostic`
/// that reports `Default_export_of_the_module_has_or_is_using_private_name_0`
/// on `input`. Go writes this closure inline three times.
fn default_export_diagnostic(input: Node) -> super::diagnostics::GetSymbolAccessibilityDiagnostic {
    Rc::new(move |_: &SymbolAccessibilityResult| {
        Some(SymbolAccessibilityDiagnostic {
            diagnostic_message: diag::Default_export_of_the_module_has_or_is_using_private_name_0,
            error_node: input,
            type_name: Node::NIL,
        })
    })
}

/// Go `defer func() { tx.tracker.watchedClassSymbol = nil; tx.tracker.classSymbolTracked = false }()`.
struct ResetWatchedClassSymbol(Rc<SymbolTrackerImpl>);

impl Drop for ResetWatchedClassSymbol {
    fn drop(&mut self) {
        let tracker = &self.0;
        tracker.watched_class_symbol.set(SymbolId::NIL);
        tracker.class_symbol_tracked.set(false);
    }
}

impl DeclarationTransformer {
    /// Runs `f` with Go `tx.Visitor()`.
    // PORT: see the module comment.
    pub(super) fn with_visitor<R>(
        &mut self,
        f: impl FnOnce(&mut NodeVisitor<'_, &mut DeclarationTransformer>) -> R,
    ) -> R {
        let emit_context = self.emit_context.clone();
        let mut visitor = emit_context.new_node_visitor(
            |node, v: &mut NodeVisitor<'_, &mut DeclarationTransformer>| v.ctx.visit(node),
            self,
        );
        f(&mut visitor)
    }

    // Go: transformers/declarations/transform.go:761 DeclarationTransformer.transformImportTypeNode
    pub(super) fn transform_import_type_node(&mut self, input: Node) -> Node {
        if !is_literal_import_type_node(input) {
            return input;
        }
        let ec = self.emit_context.clone();
        let f = ec.factory();
        let argument = input.argument();
        let literal = self.rewrite_module_specifier(input, argument.literal());
        let argument = f.update_literal_type_node(argument, literal);
        let type_arguments = self.with_visitor(|v| v.visit_nodes(input.type_argument_list()));
        f.update_import_type_node(
            input,
            input.is_type_of(),
            argument,
            input.attributes(),
            input.qualifier(),
            type_arguments,
        )
    }

    // Go: transformers/declarations/transform.go:778 DeclarationTransformer.transformConstructorTypeNode
    pub(super) fn transform_constructor_type_node(&mut self, input: Node) -> Node {
        let ec = self.emit_context.clone();
        let modifiers = self.ensure_modifiers(input);
        let type_parameters = self.with_visitor(|v| v.visit_nodes(input.type_parameter_list()));
        let parameters = self.update_param_list(input, input.parameter_list());
        let type_node = self.visit(input.type_());
        ec.factory().update_constructor_type_node(
            input,
            modifiers,
            type_parameters,
            parameters,
            type_node,
        )
    }

    // Go: transformers/declarations/transform.go:788 DeclarationTransformer.transformFunctionTypeNode
    pub(super) fn transform_function_type_node(&mut self, input: Node) -> Node {
        let ec = self.emit_context.clone();
        let type_parameters = self.with_visitor(|v| v.visit_nodes(input.type_parameter_list()));
        let parameters = self.update_param_list(input, input.parameter_list());
        let type_node = self.visit(input.type_());
        ec.factory()
            .update_function_type_node(input, type_parameters, parameters, type_node)
    }

    // Go: transformers/declarations/transform.go:797 DeclarationTransformer.transformConditionalTypeNode
    pub(super) fn transform_conditional_type_node(&mut self, input: Node) -> Node {
        let check_type = self.visit(input.check_type());
        let extends_type = self.visit(input.extends_type());
        let old_enclosing_decl = self.enclosing_declaration;
        self.enclosing_declaration = input.true_type();
        let true_type = self.visit(input.true_type());
        self.enclosing_declaration = old_enclosing_decl;
        let false_type = self.visit(input.false_type());

        let ec = self.emit_context.clone();
        ec.factory().update_conditional_type_node(
            input,
            check_type,
            extends_type,
            true_type,
            false_type,
        )
    }

    // Go: transformers/declarations/transform.go:815 DeclarationTransformer.transformTypeReference
    pub(super) fn transform_type_reference(&mut self, input: Node) -> Node {
        self.check_entity_name_visibility(input.type_name(), self.enclosing_declaration);
        self.with_visitor(|v| v.visit_each_child(input))
    }

    // Go: transformers/declarations/transform.go:820 DeclarationTransformer.transformExpressionWithTypeArguments
    pub(super) fn transform_expression_with_type_arguments(&mut self, input: Node) -> Node {
        if is_entity_name(input.expression()) || is_entity_name_expression(input.expression()) {
            self.check_entity_name_visibility(input.expression(), self.enclosing_declaration);
        }
        self.with_visitor(|v| v.visit_each_child(input))
    }

    // Go: transformers/declarations/transform.go:827 DeclarationTransformer.transformTypeParameterDeclaration
    pub(super) fn transform_type_parameter_declaration(&mut self, input: Node) -> Node {
        if is_private_method_type_parameter(&*self.resolver, input)
            && (input.default_type().is_some() || input.constraint().is_some())
        {
            let ec = self.emit_context.clone();
            return ec.factory().update_type_parameter_declaration(
                input,
                input.modifiers(),
                input.name(),
                Node::NIL,
                input.expression(),
                Node::NIL,
            );
        }
        self.with_visitor(|v| v.visit_each_child(input))
    }

    // Go: transformers/declarations/transform.go:841 DeclarationTransformer.transformVariableDeclaration
    pub(super) fn transform_variable_declaration(&mut self, input: Node) -> Node {
        let current_source_file = self.state.borrow().current_source_file;
        if source_file_info(current_source_file)
            .common_js_module_indicator
            .is_some()
            && is_variable_declaration_initialized_to_require(input)
        {
            return self.transform_cjs_require_variable_declaration(input);
        }
        if is_binding_pattern(input.name()) && has_any_binding_initializers(input.name()) {
            return self.recreate_binding_pattern(input.name());
        }
        // Variable declaration types also suppress new diagnostic contexts, provided the contexts wouldn't be made for binding pattern types
        self.suppress_new_diagnostic_contexts = true;
        let ec = self.emit_context.clone();
        // PORT: Go `tx.bindingNameVisitor.VisitNode`; the visitor is built on
        // demand with the `visitBindingName` callback (see `with_visitor`).
        let name = {
            let mut v = ec.new_node_visitor(
                |node, v: &mut NodeVisitor<'_, &mut DeclarationTransformer>| {
                    v.ctx.visit_binding_name(node)
                },
                &mut *self,
            );
            v.visit_node(input.name())
        };
        let type_node = self.ensure_type(input, false);
        let initializer = self.ensure_no_initializer(input);
        ec.factory()
            .update_variable_declaration(input, name, Node::NIL, type_node, initializer)
    }

    // Go: transformers/declarations/transform.go:875 DeclarationTransformer.transformCjsRequireVariableDeclaration
    pub(super) fn transform_cjs_require_variable_declaration(&mut self, input: Node) -> Node {
        let ec = self.emit_context.clone();
        let f = ec.factory();
        let specifier =
            self.rewrite_module_specifier(input, input.initializer().arguments().get(0));
        if is_identifier(input.name()) {
            // `const x = require("something")` -> `import x = require("something")`
            f.new_import_equals_declaration(
                ModifierList::NIL,
                false,
                input.name(),
                f.new_external_module_reference(specifier),
            )
        } else if is_array_binding_pattern(input.name()) {
            // TODO: Is this actually reachable? should we error on this?
            Node::NIL
        } else {
            // object binding pattern

            // `const {x, y: z} = require("something")` -> `import {x, y as z} from "something"`
            let b = input.name();
            let mut import_specifiers: Vec<Node> = Vec::new();
            for elem in b.elements().iter() {
                if !is_identifier(elem.name()) {
                    continue; // nested destructuring, bail
                }
                import_specifiers.push(f.new_import_specifier(
                    false,
                    elem.property_name(),
                    elem.name(),
                ));
            }
            f.new_import_declaration(
                ModifierList::NIL,
                f.new_import_clause(
                    SyntaxKind::Unknown,
                    Node::NIL,
                    f.new_named_imports(f.new_node_list(&import_specifiers)),
                ),
                specifier,
                Node::NIL,
            )
        }
    }

    // Go: transformers/declarations/transform.go:907 DeclarationTransformer.recreateBindingPattern
    pub(super) fn recreate_binding_pattern(&mut self, input: Node) -> Node {
        let mut results: Vec<Node> = Vec::new();
        for elem in input.elements().iter() {
            let result = self.recreate_binding_element(elem);
            if result.is_nil() {
                continue;
            }
            if result.kind() == SyntaxKind::SyntaxList {
                results.extend(syntax_list_children(result));
            } else {
                results.push(result);
            }
        }
        if results.is_empty() {
            return Node::NIL;
        }
        if results.len() == 1 {
            return results[0];
        }
        let ec = self.emit_context.clone();
        ec.factory().new_syntax_list(&results)
    }

    // Go: transformers/declarations/transform.go:929 DeclarationTransformer.recreateBindingElement
    pub(super) fn recreate_binding_element(&mut self, e: Node) -> Node {
        if e.name().is_nil() {
            return Node::NIL;
        }
        if !get_binding_name_visible(&*self.resolver, e) {
            return Node::NIL;
        }
        if is_binding_pattern(e.name()) {
            return self.recreate_binding_pattern(e.name());
        }
        let ec = self.emit_context.clone();
        let type_node = self.ensure_type(e, false);
        ec.factory().new_variable_declaration(
            e.name(),
            Node::NIL,
            type_node,
            Node::NIL, // TODO: possible strada bug - not emitting const initialized binding pattern elements?
        )
    }

    // Go: transformers/declarations/transform.go:947 DeclarationTransformer.transformIndexSignatureDeclaration
    pub(super) fn transform_index_signature_declaration(&mut self, input: Node) -> Node {
        let ec = self.emit_context.clone();
        let f = ec.factory();
        let mut t = self.visit(input.type_());
        if t.is_nil() {
            t = f.new_keyword_type_node(SyntaxKind::AnyKeyword);
        }
        let modifiers = self.ensure_modifiers(input);
        let parameters = self.update_param_list(input, input.parameter_list());
        f.update_index_signature_declaration(input, modifiers, parameters, t)
    }

    // Go: transformers/declarations/transform.go:960 DeclarationTransformer.transformCallSignatureDeclaration
    pub(super) fn transform_call_signature_declaration(&mut self, input: Node) -> Node {
        let ec = self.emit_context.clone();
        let type_parameters = self.ensure_type_params(input, input.type_parameter_list());
        let parameters = self.update_param_list(input, input.parameter_list());
        let type_node = self.ensure_type(input, false);
        ec.factory().update_call_signature_declaration(
            input,
            type_parameters,
            parameters,
            type_node,
        )
    }

    // Go: transformers/declarations/transform.go:969 DeclarationTransformer.transformPropertySignatureDeclaration
    pub(super) fn transform_property_signature_declaration(&mut self, input: Node) -> Node {
        if is_private_identifier(input.name()) {
            return Node::NIL;
        }
        let ec = self.emit_context.clone();
        let modifiers = self.ensure_modifiers(input);
        let type_node = self.ensure_type(input, false);
        let initializer = self.ensure_no_initializer(input); // TODO: possible strada bug (fixed here) - const property signatures never initialized
        let result = ec.factory().update_property_signature_declaration(
            input,
            modifiers,
            input.name(),
            input.postfix_token(),
            type_node,
            initializer,
        );
        self.preserve_partial_js_doc(result, input);
        result
    }

    // Go: transformers/declarations/transform.go:985 DeclarationTransformer.transformPropertyDeclaration
    pub(super) fn transform_property_declaration(&mut self, input: Node) -> Node {
        if is_private_identifier(input.name()) {
            return Node::NIL;
        }
        // Remove definite assignment assertion (!) from declaration files
        let mut postfix_token = input.postfix_token();
        if postfix_token.is_some() && postfix_token.kind() == SyntaxKind::ExclamationToken {
            postfix_token = Node::NIL;
        }
        let ec = self.emit_context.clone();
        let modifiers = self.ensure_modifiers(input);
        let type_node = self.ensure_type(input, false);
        let initializer = self.ensure_no_initializer(input);
        ec.factory().update_property_declaration(
            input,
            modifiers,
            input.name(),
            postfix_token,
            type_node,
            initializer,
        )
    }

    // Go: transformers/declarations/transform.go:1004 DeclarationTransformer.transformSetAccessorDeclaration
    pub(super) fn transform_set_accessor_declaration(&mut self, input: Node) -> Node {
        if is_private_identifier(input.name()) {
            return Node::NIL;
        }

        let ec = self.emit_context.clone();
        let modifiers = self.ensure_modifiers(input);
        let is_private = !self
            .resolver
            .get_effective_declaration_flags(ec.parse_node(input), ModifierFlags::PRIVATE)
            .is_empty();
        let parameters = self.update_accessor_param_list(input, is_private);
        ec.factory().update_set_accessor_declaration(
            input,
            modifiers,
            input.name(),
            NodeList::NIL, // accessors shouldn't have type params
            parameters,
            Node::NIL,
            Node::NIL,
            Node::NIL,
        )
    }

    // Go: transformers/declarations/transform.go:1021 DeclarationTransformer.transformGetAccesorDeclaration
    pub(super) fn transform_get_accesor_declaration(&mut self, input: Node) -> Node {
        if is_private_identifier(input.name()) {
            return Node::NIL;
        }
        let ec = self.emit_context.clone();
        let modifiers = self.ensure_modifiers(input);
        let is_private = !self
            .resolver
            .get_effective_declaration_flags(ec.parse_node(input), ModifierFlags::PRIVATE)
            .is_empty();
        let parameters = self.update_accessor_param_list(input, is_private);
        let type_node = self.ensure_type(input, false);
        ec.factory().update_get_accessor_declaration(
            input,
            modifiers,
            input.name(),
            NodeList::NIL, // accessors shouldn't have type params
            parameters,
            type_node,
            Node::NIL,
            Node::NIL,
        )
    }

    // Go: transformers/declarations/transform.go:1037 DeclarationTransformer.updateAccessorParamList
    pub(super) fn update_accessor_param_list(&mut self, input: Node, is_private: bool) -> NodeList {
        let ec = self.emit_context.clone();
        let f = ec.factory();
        let mut new_params: Vec<Node> = Vec::new();
        if !is_private {
            let this_param = get_this_parameter(input);
            if this_param.is_some() {
                new_params.push(self.ensure_parameter(this_param));
            }
        }
        if is_set_accessor_declaration(input) {
            let mut value_param = Node::NIL;
            if !is_private {
                let parameters = input.parameters();
                if new_params.len() == 1 && parameters.len() >= 2 {
                    value_param = self.ensure_parameter(parameters.get(1));
                } else if new_params.is_empty() && !parameters.is_empty() {
                    value_param = self.ensure_parameter(parameters.get(0));
                }
            }
            if value_param.is_nil() {
                // When synthesizing a missing value parameter, emit `value: any` for non-private accessors to match TypeScript's declaration emit behavior.
                let mut t = Node::NIL;
                if !is_private {
                    t = f.new_keyword_type_node(SyntaxKind::AnyKeyword);
                }
                value_param = f.new_parameter_declaration(
                    ModifierList::NIL,
                    Node::NIL,
                    f.new_identifier("value"),
                    Node::NIL,
                    t,
                    Node::NIL,
                );
            }
            new_params.push(value_param);
        }
        f.new_node_list(&new_params)
    }

    // Go: transformers/declarations/transform.go:1074 DeclarationTransformer.transformConstructorDeclaration
    pub(super) fn transform_constructor_declaration(&mut self, input: Node) -> Node {
        let ec = self.emit_context.clone();
        let modifiers = self.ensure_modifiers(input);
        let parameters = self.update_param_list(input, input.parameter_list());
        // A constructor declaration may not have a type annotation
        ec.factory().update_constructor_declaration(
            input,
            modifiers,
            NodeList::NIL, // no type params
            parameters,
            Node::NIL, // no return type
            Node::NIL,
            Node::NIL,
        )
    }

    // Go: transformers/declarations/transform.go:1087 DeclarationTransformer.transformConstructSignatureDeclaration
    pub(super) fn transform_construct_signature_declaration(&mut self, input: Node) -> Node {
        let ec = self.emit_context.clone();
        let type_parameters = self.ensure_type_params(input, input.type_parameter_list());
        let parameters = self.update_param_list(input, input.parameter_list());
        let type_node = self.ensure_type(input, false);
        ec.factory().update_construct_signature_declaration(
            input,
            type_parameters,
            parameters,
            type_node,
        )
    }

    // Go: transformers/declarations/transform.go:1096 DeclarationTransformer.omitPrivateMethodType
    pub(super) fn omit_private_method_type(&mut self, input: Node) -> Node {
        let symbol = input.symbol();
        if symbol.is_some() {
            let declarations = bound_symbol_declarations(symbol);
            if !declarations.is_empty() && declarations[0] != input {
                return Node::NIL;
            }
        }
        let ec = self.emit_context.clone();
        let result;
        if is_method_signature_declaration(input) {
            let modifiers = self.ensure_modifiers(input);
            result = ec.factory().new_property_signature_declaration(
                modifiers,
                input.name(),
                Node::NIL, /*postfixToken*/
                Node::NIL, /*typeNode*/
                Node::NIL, /*initializer*/
            );
        } else {
            let modifiers = self.ensure_modifiers(input);
            result = ec.factory().new_property_declaration(
                modifiers,
                input.name(),
                Node::NIL, /*postfixToken*/
                Node::NIL, /*typeNode*/
                Node::NIL, /*initializer*/
            );
        }
        self.preserve_js_doc(result, input);
        result
    }

    // Go: transformers/declarations/transform.go:1122 DeclarationTransformer.transformMethodSignatureDeclaration
    pub(super) fn transform_method_signature_declaration(&mut self, input: Node) -> Node {
        let ec = self.emit_context.clone();
        if !self
            .resolver
            .get_effective_declaration_flags(ec.parse_node(input), ModifierFlags::PRIVATE)
            .is_empty()
        {
            self.omit_private_method_type(input)
        } else if is_private_identifier(input.name()) {
            Node::NIL
        } else {
            let modifiers = self.ensure_modifiers(input);
            let type_parameters = self.ensure_type_params(input, input.type_parameter_list());
            let parameters = self.update_param_list(input, input.parameter_list());
            let type_node = self.ensure_type(input, false);
            ec.factory().update_method_signature_declaration(
                input,
                modifiers,
                input.name(),
                input.postfix_token(),
                type_parameters,
                parameters,
                type_node,
            )
        }
    }

    // Go: transformers/declarations/transform.go:1140 DeclarationTransformer.transformMethodDeclaration
    pub(super) fn transform_method_declaration(&mut self, input: Node) -> Node {
        let ec = self.emit_context.clone();
        if !self
            .resolver
            .get_effective_declaration_flags(ec.parse_node(input), ModifierFlags::PRIVATE)
            .is_empty()
        {
            self.omit_private_method_type(input)
        } else if is_private_identifier(input.name()) {
            Node::NIL
        } else {
            let modifiers = self.ensure_modifiers(input);
            let type_parameters = self.ensure_type_params(input, input.type_parameter_list());
            let parameters = self.update_param_list(input, input.parameter_list());
            let type_node = self.ensure_type(input, false);
            ec.factory().update_method_declaration(
                input,
                modifiers,
                Node::NIL,
                input.name(),
                input.postfix_token(),
                type_parameters,
                parameters,
                type_node,
                Node::NIL,
                Node::NIL,
            )
        }
    }

    // Go: transformers/declarations/transform.go:1161 DeclarationTransformer.visitDeclarationStatements
    pub(super) fn visit_declaration_statements(&mut self, input: Node) -> Node {
        if self.should_strip_internal(input) {
            return Node::NIL;
        }
        match input.kind() {
            SyntaxKind::ExportDeclaration => {
                if is_source_file(input.parent()) {
                    self.result_has_external_module_indicator = true;
                }
                self.result_has_scope_marker = true;
                // Rewrite external module names if necessary
                let ec = self.emit_context.clone();
                let module_specifier =
                    self.rewrite_module_specifier(input, input.module_specifier());
                ec.factory().update_export_declaration(
                    input,
                    input.modifiers(),
                    input.is_type_only(),
                    input.export_clause(),
                    module_specifier,
                    input.attributes(),
                )
            }
            SyntaxKind::ExportAssignment => self.transform_export_assignment(
                input,
                input,
                input.expression(),
                input.is_export_equals(),
            ),
            _ => {
                // PORT: Go keys the map by `ast.GetNodeId`; this map is keyed by `Node`.
                let id = self.emit_context.most_original(input);
                if self
                    .late_statement_replacement_map
                    .get(&id)
                    .is_none_or(|n| n.is_nil())
                {
                    // Don't actually transform yet; just leave as original node - will be elided/swapped by late pass
                    let transformed = self.transform_top_level_declaration(input);
                    self.late_statement_replacement_map.insert(id, transformed);
                }
                input
            }
        }
    }

    // Go: transformers/declarations/transform.go:1192 DeclarationTransformer.tryGetNameOfAssignedExpression
    pub(super) fn try_get_name_of_assigned_expression(&mut self, unwrapped: Node) -> Node {
        let mut name_node = Node::NIL;
        let mut name_text = "";
        if !is_property_access_expression(unwrapped) && unwrapped.name().is_some() {
            name_text = unwrapped.name().text();
        } else if is_identifier(unwrapped) {
            name_text = unwrapped.text();
        }
        if !name_text.is_empty() && name_text != "default" {
            let ec = self.emit_context.clone();
            if self
                .resolver
                .is_name_resolvable(self.enclosing_declaration, name_text)
            {
                // create a unique name that shares the same text as its' base
                name_node = ec.factory().new_unique_name_ex(name_text, optimistic());
            } else {
                // use the node's name as-is, since it's not otherwise in-scope
                name_node = ec.factory().new_identifier(name_text);
            }
        }
        name_node
    }

    // Go: transformers/declarations/transform.go:1212 DeclarationTransformer.getNameOfExportedAssignedExpression
    pub(super) fn get_name_of_exported_assigned_expression(
        &mut self,
        unwrapped: Node,
        is_export_equals: bool,
    ) -> Node {
        let mut name_node = self.try_get_name_of_assigned_expression(unwrapped);
        if name_node.is_nil() {
            let ec = self.emit_context.clone();
            let current_source_file = self.state.borrow().current_source_file;
            // fallback to a default name
            if is_export_equals && is_source_file_js(current_source_file) {
                // only JS files prefer to use `_exports` for export assignments - TS has always used `_default` for both `export=` and `export default`
                name_node = ec.factory().new_unique_name_ex("_exports", optimistic());
            } else {
                name_node = ec.factory().new_unique_name_ex("_default", optimistic());
            }
        }
        self.cjs_export_assignment_name = name_node;
        name_node
    }

    // Go: transformers/declarations/transform.go:1221 DeclarationTransformer.transformExportAssignment
    pub(super) fn transform_export_assignment(
        &mut self,
        input: Node,
        assignment: Node,
        expression: Node,
        is_export_equals: bool,
    ) -> Node {
        let ec = self.emit_context.clone();
        let f = ec.factory();
        if is_source_file(input.parent()) {
            self.result_has_external_module_indicator = true;
        }
        self.result_has_scope_marker = true;
        if is_identifier(expression)
            && (is_source_file(input.parent()) || is_module_block(input.parent()))
        {
            let export_assignment =
                f.new_export_assignment(ModifierList::NIL, is_export_equals, Node::NIL, expression);
            ec.assign_source_map_range(export_assignment, input);
            self.preserve_js_doc(export_assignment, input);
            return export_assignment;
        }

        self.state.borrow_mut().get_symbol_accessibility_diagnostic =
            Some(default_export_diagnostic(input));
        self.tracker.push_error_fallback_node(assignment);

        // Check if the expression is a class expression - emit as a class declaration + export assignment
        let unwrapped = skip_outer_expressions(
            expression,
            OuterExpressionKinds::OEK_EXPRESSION_TYPE_PASSTHROUGH,
        );
        let new_id = self.get_name_of_exported_assigned_expression(unwrapped, is_export_equals);
        if is_class_expression(unwrapped) {
            let mut mods: Vec<Node> = Vec::new();
            if self.needs_declare {
                mods.push(f.new_modifier(SyntaxKind::DeclareKeyword));
            }
            let class_decl = self.transform_class_expression_to_declaration(
                unwrapped,
                new_id,
                f.new_modifier_list(&mods),
            );
            self.tracker.pop_error_fallback_node();
            self.preserve_js_doc(class_decl, input);
            // Reuse the same name node for the export so unique names resolve consistently
            let export_assignment =
                f.new_export_assignment(ModifierList::NIL, is_export_equals, Node::NIL, new_id);
            ec.assign_source_map_range(export_assignment, input);
            self.remove_all_comments(export_assignment);
            return f.new_syntax_list(&[export_assignment, class_decl]);
        } else if is_function_like(unwrapped) {
            // Promote function or arrow function expressions to a function declaration
            let mut mods: Vec<Node> = Vec::new();
            if self.needs_declare {
                mods.push(f.new_modifier(SyntaxKind::DeclareKeyword));
            }
            let full_signature_type = assignment.type_();
            let func_decl = self.transform_function_like_to_declaration(
                unwrapped,
                new_id,
                f.new_modifier_list(&mods),
                full_signature_type,
            );
            self.tracker.pop_error_fallback_node();
            self.preserve_js_doc(func_decl, input);
            // Reuse the same name node for the export so unique names resolve consistently
            let export_assignment =
                f.new_export_assignment(ModifierList::NIL, is_export_equals, Node::NIL, new_id);
            ec.assign_source_map_range(export_assignment, input);
            self.remove_all_comments(export_assignment);
            return f.new_syntax_list(&[export_assignment, func_decl]);
        }

        // expression is non-identifier, create _default typed variable to reference
        self.cjs_export_assignment_name = new_id;
        let mut type_ = Node::NIL;
        let mut initializer = Node::NIL;
        if is_primitive_literal_value(unwrap_parenthesized_expression(expression), true) {
            let tracker: EmitSymbolTracker = Some(self.tracker.clone());
            initializer = self
                .resolver
                .create_literal_const_value(ec.parse_node(assignment), tracker);
        }
        if initializer.is_nil() {
            type_ = self.ensure_type(assignment, false);
        }
        let var_decl = f.new_variable_declaration(new_id, Node::NIL, type_, initializer);
        self.tracker.pop_error_fallback_node();
        let mod_list = if self.needs_declare {
            f.new_modifier_list(&[f.new_modifier(SyntaxKind::DeclareKeyword)])
        } else {
            f.new_modifier_list(&[])
        };
        let statement = f.new_variable_statement(
            mod_list,
            f.new_variable_declaration_list(f.new_node_list(&[var_decl]), NodeFlags::CONST),
        );
        let export_assignment =
            f.new_export_assignment(ModifierList::NIL, is_export_equals, Node::NIL, new_id);
        ec.assign_source_map_range(export_assignment, input);
        // Remove comments from the export declaration and copy them onto the synthetic _default declaration
        self.preserve_js_doc(statement, input);
        f.new_syntax_list(&[statement, export_assignment])
    }

    // Go: transformers/declarations/transform.go:1298 DeclarationTransformer.transformFunctionLikeToDeclaration
    pub(super) fn transform_function_like_to_declaration(
        &mut self,
        unwrapped: Node,
        func_name: Node,
        mods: ModifierList,
        full_signature_type: Node,
    ) -> Node {
        let ec = self.emit_context.clone();
        let f = ec.factory();
        // PORT: Go reads `unwrapped.FunctionLikeData()`; these are the same fields.
        let mut sig = unwrapped.full_signature();
        if sig.is_nil() {
            sig = full_signature_type;
        }
        if sig.is_nil() {
            let type_parameters =
                self.ensure_type_params(unwrapped, unwrapped.type_parameter_list());
            let parameters = self.update_param_list(unwrapped, unwrapped.parameter_list());
            let type_node = self.ensure_type(unwrapped, false);
            let full_signature = self.with_visitor(|v| v.visit_node(sig));
            f.new_function_declaration(
                mods,
                Node::NIL,
                func_name,
                type_parameters,
                parameters,
                type_node,
                full_signature,
                Node::NIL,
            )
        } else {
            // If a full signature type node is present, emit as a variable statement to reuse it
            let type_node = self.with_visitor(|v| v.visit_node(sig));
            f.new_variable_statement(
                mods,
                f.new_variable_declaration_list(
                    f.new_node_list(&[f.new_variable_declaration(
                        func_name,
                        Node::NIL,
                        type_node,
                        Node::NIL,
                    )]),
                    NodeFlags::CONST,
                ),
            )
        }
    }

    // Go: transformers/declarations/transform.go:1324 DeclarationTransformer.transformBinaryExpressionToExportDeclaration
    pub(super) fn transform_binary_expression_to_export_declaration(
        &mut self,
        input: Node,
        name: Node,
    ) -> Node {
        let mut property_name = input.right();

        // track alias target so referenced declarations are included in the output
        let result = self
            .resolver
            .is_entity_name_visible(property_name, self.enclosing_declaration);
        self.tracker.handle_symbol_accessibility_error(result);

        if is_identifier(name) && property_name.text() == name.text() {
            property_name = Node::NIL;
        }

        let ec = self.emit_context.clone();
        let f = ec.factory();
        f.new_export_declaration(
            ModifierList::NIL,
            false,
            f.new_named_exports(f.new_node_list(&[f.new_export_specifier(
                false,
                property_name,
                name,
            )])),
            Node::NIL,
            Node::NIL,
        )
    }

    // Go: transformers/declarations/transform.go:1343 DeclarationTransformer.transformCommonJSExport
    pub(super) fn transform_common_js_export(&mut self, input: Node, name: Node) -> Node {
        let res = self.transform_common_js_export_worker(input, name);
        if res.is_nil() {
            return res;
        }
        self.wrap_in_cjs_export_namespace(res)
    }

    // Go: transformers/declarations/transform.go:1351 DeclarationTransformer.transformCommonJSExportWorker
    pub(super) fn transform_common_js_export_worker(&mut self, input: Node, name: Node) -> Node {
        let ec = self.emit_context.clone();
        let f = ec.factory();
        let mut name_text = "";
        if is_identifier(name) || is_string_literal(name) {
            name_text = name.text();
        }
        if self.witnessed_cjs_exports.contains(name_text) && !name_text.is_empty() {
            return Node::NIL; // Already emitted this export name
        }
        self.witnessed_cjs_exports.insert(name_text.to_string());
        self.result_has_external_module_indicator = true;
        self.result_has_scope_marker = true;
        // only transform cjs exports to shorthand at the top-level of a source file, otherwise we uniformly emit nested exports with a type annotation
        if is_common_js_alias_export(input)
            && is_expression_statement(input.parent())
            && is_source_file(input.parent().parent())
        {
            // export { name }
            // export { source as name }
            return self.transform_binary_expression_to_export_declaration(input, name);
        }

        // Check if the RHS is a class expression - emit as a class declaration instead of a typed variable
        if is_binary_expression(input) {
            let rhs = unwrap_parenthesized_expression(input.right());
            if is_class_expression(rhs) {
                let class_expr_name = rhs.name();
                let has_expr_name = class_expr_name.is_some() && !class_expr_name.text().is_empty();

                if has_expr_name {
                    // Set up TrackSymbol watch to detect if the class expression's own
                    // symbol is referenced during member type serialization.
                    {
                        let tracker = self.tracker.clone();
                        tracker.watched_class_symbol.set(rhs.symbol());
                        tracker.class_symbol_tracked.set(false);
                    }
                    // PORT: Go `defer`; every path out of this block returns.
                    let _reset = ResetWatchedClassSymbol(self.tracker.clone());

                    // Serialize class members using the class expression name, which
                    // triggers TrackSymbol for any self-referential member types.
                    let class_name = f.new_identifier(class_expr_name.text());
                    let class_mods = [f.new_modifier(SyntaxKind::ExportKeyword)];
                    let mut class_decl = self.transform_class_expression_to_declaration(
                        rhs,
                        class_name,
                        f.new_modifier_list(&class_mods),
                    );
                    self.preserve_js_doc(class_decl, input);

                    // Determine if namespace isolation is needed:
                    // - The class expression name differs from the export name, OR
                    // - The class's own symbol was used in a member's serialized type
                    let names_differ =
                        !is_identifier(name) || class_expr_name.text() != name.text();
                    let needs_isolation = names_differ || self.tracker.class_symbol_tracked.get();

                    if needs_isolation {
                        let ns_name = f.new_unique_name_ex("_ns", optimistic());
                        let mut ns_mods: Vec<Node> = Vec::new();
                        if self.needs_declare {
                            ns_mods.push(f.new_modifier(SyntaxKind::DeclareKeyword));
                        }
                        let ns_decl = f.new_module_declaration(
                            f.new_modifier_list(&ns_mods),
                            SyntaxKind::NamespaceKeyword,
                            ns_name,
                            Node::NIL,
                            f.new_module_block(f.new_node_list(&[class_decl])),
                        );

                        let mut alias_base = String::from("_exported");
                        let name_text = name.text();
                        if is_identifier(name)
                            && is_identifier_text(
                                &format!("_{name_text}"),
                                LanguageVariant::STANDARD,
                            )
                        {
                            alias_base = format!("_{name_text}");
                        }
                        let import_alias = f.new_unique_name_ex(&alias_base, optimistic());
                        let qualified_name = f.new_qualified_name(ns_name, class_name);
                        let import_decl = f.new_import_equals_declaration(
                            ModifierList::NIL,
                            false,
                            import_alias,
                            qualified_name,
                        );

                        let export_specifier = f.new_export_specifier(false, import_alias, name);
                        let export_decl = f.new_export_declaration(
                            ModifierList::NIL,
                            false,
                            f.new_named_exports(f.new_node_list(&[export_specifier])),
                            Node::NIL,
                            Node::NIL,
                        );
                        self.remove_all_comments(export_decl);

                        return f.new_syntax_list(&[ns_decl, import_decl, export_decl]);
                    }

                    // No isolation needed: names match and no self-references.
                    // Update modifiers to include declare if needed.
                    let mut mods: Vec<Node> = Vec::new();
                    mods.push(f.new_modifier(SyntaxKind::ExportKeyword));
                    if self.needs_declare {
                        mods.push(f.new_modifier(SyntaxKind::DeclareKeyword));
                    }
                    class_decl = f.update_class_declaration(
                        class_decl,
                        f.new_modifier_list(&mods),
                        class_decl.name(),
                        class_decl.type_parameter_list(),
                        class_decl.heritage_clauses(),
                        class_decl.member_list(),
                    );
                    return class_decl;
                }
                let mut mods: Vec<Node> = Vec::new();
                mods.push(f.new_modifier(SyntaxKind::ExportKeyword));
                if self.needs_declare {
                    mods.push(f.new_modifier(SyntaxKind::DeclareKeyword));
                }
                let mut class_name = name;
                if !is_identifier(class_name) {
                    class_name = f.new_unique_name_ex("_class", optimistic());
                }
                let class_decl = self.transform_class_expression_to_declaration(
                    rhs,
                    class_name,
                    f.new_modifier_list(&mods),
                );
                self.preserve_js_doc(class_decl, input);
                if !is_identifier(name) {
                    // Non-identifier name: emit class declaration + named export
                    let export_decl = f.new_export_declaration(
                        ModifierList::NIL,
                        false,
                        f.new_named_exports(
                            f.new_node_list(&[f.new_export_specifier(false, class_name, name)]),
                        ),
                        Node::NIL,
                        Node::NIL,
                    );
                    self.remove_all_comments(export_decl);
                    return f.new_syntax_list(&[class_decl, export_decl]);
                }
                return class_decl;
            }
        }

        if is_identifier(name) {
            if name.text() == "default" {
                // const _default: Type; export default _default;
                let new_id = f.new_unique_name_ex("_default", optimistic());
                self.state.borrow_mut().get_symbol_accessibility_diagnostic =
                    Some(default_export_diagnostic(input));
                self.tracker.push_error_fallback_node(input);
                let type_ = self.ensure_type(input, false);
                let var_decl = f.new_variable_declaration(new_id, Node::NIL, type_, Node::NIL);
                self.tracker.pop_error_fallback_node();
                let mod_list = if self.needs_declare {
                    f.new_modifier_list(&[f.new_modifier(SyntaxKind::DeclareKeyword)])
                } else {
                    f.new_modifier_list(&[])
                };
                let statement = f.new_variable_statement(
                    mod_list,
                    f.new_variable_declaration_list(f.new_node_list(&[var_decl]), NodeFlags::CONST),
                );

                let assignment =
                    f.new_export_assignment(input.modifiers(), false, Node::NIL, new_id);
                // Remove comments from the export declaration and copy them onto the synthetic _default declaration
                self.preserve_js_doc(statement, input);
                self.remove_all_comments(assignment);
                return f.new_syntax_list(&[statement, assignment]);
            } else if self.resolver.get_referenced_value_declaration(name) == input
                || self
                    .resolver
                    .get_referenced_value_declaration(name)
                    .is_nil()
            {
                // only inline to a export var if the `name` lookup points at this assignment or nothing - if it points at something else, we must use a temp name
                // export var name: Type
                self.tracker.push_error_fallback_node(input);
                let type_ = self.ensure_type(input, false);
                let var_decl = f.new_variable_declaration(name, Node::NIL, type_, Node::NIL);
                self.tracker.pop_error_fallback_node();
                let mod_list = if self.needs_declare {
                    f.new_modifier_list(&[
                        f.new_modifier(SyntaxKind::ExportKeyword),
                        f.new_modifier(SyntaxKind::DeclareKeyword),
                    ])
                } else {
                    f.new_modifier_list(&[f.new_modifier(SyntaxKind::ExportKeyword)])
                };
                return f.new_variable_statement(
                    mod_list,
                    f.new_variable_declaration_list(f.new_node_list(&[var_decl]), NodeFlags::NONE),
                );
            }
        }
        // const _exported: Type; export {_exported as "name"};
        let new_id = f.new_unique_name_ex("_exported", optimistic());
        self.state.borrow_mut().get_symbol_accessibility_diagnostic =
            Some(default_export_diagnostic(input));
        self.tracker.push_error_fallback_node(input);
        let type_ = self.ensure_type(input, false);
        let var_decl = f.new_variable_declaration(new_id, Node::NIL, type_, Node::NIL);
        self.tracker.pop_error_fallback_node();
        let mod_list = if self.needs_declare {
            f.new_modifier_list(&[f.new_modifier(SyntaxKind::DeclareKeyword)])
        } else {
            f.new_modifier_list(&[])
        };
        let statement = f.new_variable_statement(
            mod_list,
            f.new_variable_declaration_list(f.new_node_list(&[var_decl]), NodeFlags::CONST),
        );

        let assignment = f.new_export_declaration(
            ModifierList::NIL,
            false,
            f.new_named_exports(f.new_node_list(&[f.new_export_specifier(false, new_id, name)])),
            Node::NIL,
            Node::NIL,
        );
        // Remove comments from the export declaration and copy them onto the synthetic _default declaration
        self.preserve_js_doc(statement, input);
        self.remove_all_comments(assignment);
        f.new_syntax_list(&[statement, assignment])
    }
}

// Go: transformers/declarations/transform.go:859 hasAnyBindingInitializers
fn has_any_binding_initializers(binding_pattern: Node) -> bool {
    for elem in binding_pattern.elements().iter() {
        if !is_binding_element(elem) {
            continue;
        }
        if elem.initializer().is_some() {
            return true;
        }
        if elem.name().is_some()
            && is_binding_pattern(elem.name())
            && has_any_binding_initializers(elem.name())
        {
            return true;
        }
    }
    false
}
