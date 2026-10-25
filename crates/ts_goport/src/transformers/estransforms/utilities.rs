//! Port of Go `transformers/estransforms/utilities.go`, plus the visitor
//! plumbing that every transformer in this package shares.

use super::async_::{assignment_target_contains_super_property, is_update_expression};
use crate::ast::visitor::NodeVisitor;
use crate::prelude::*;
use crate::printer::factory::NodeFactory;
use crate::printer::{AutoGenerateOptions, EmitContext};
use crate::transformers::modifier_visitor::extract_modifiers;

/// Go `transformers.Transformer` visitor plumbing.
///
/// PORT: Go builds each `*ast.NodeVisitor` once, closing over the
/// transformer. Here a visitor holds `&mut` to the transformer in its `ctx`,
/// so it is built for each call. `root_visit` is Go `tx.visit`, the callback
/// of the root visitor `tx.Visitor()`. The default methods are Go
/// `tx.Visitor().VisitX(..)` and the `EmitContext` visitor hooks called with
/// `tx.Visitor()`.
pub(crate) trait TxVisitors: Sized {
    /// Go `tx.EmitContext()`.
    fn ec(&self) -> Rc<EmitContext>;

    /// Go `tx.visit`, the callback of `tx.Visitor()`.
    fn root_visit(&mut self, node: Node) -> Node;

    /// Runs `f` with a visitor whose callback is `visit` (Go
    /// `tx.EmitContext().NewNodeVisitor(visit)`).
    fn with_visitor<R>(
        &mut self,
        visit: fn(&mut Self, Node) -> Node,
        f: impl FnOnce(&mut NodeVisitor<'_, &mut Self>) -> R,
    ) -> R {
        let ec = self.ec();
        let mut visitor = ec.new_node_visitor(
            move |node, v: &mut NodeVisitor<'_, &mut Self>| visit(v.ctx, node),
            self,
        );
        f(&mut visitor)
    }

    /// Go `tx.Visitor().VisitNode(node)`.
    fn visit_node(&mut self, node: Node) -> Node {
        self.with_visitor(Self::root_visit, |v| v.visit_node(node))
    }

    /// Go `tx.Visitor().VisitNodes(nodes)`.
    fn visit_nodes(&mut self, nodes: NodeList) -> NodeList {
        self.with_visitor(Self::root_visit, |v| v.visit_nodes(nodes))
    }

    /// Go `tx.Visitor().VisitModifiers(nodes)`.
    fn visit_modifiers(&mut self, nodes: ModifierList) -> ModifierList {
        self.with_visitor(Self::root_visit, |v| v.visit_modifiers(nodes))
    }

    /// Go `tx.Visitor().VisitEachChild(node)`.
    fn visit_each_child(&mut self, node: Node) -> Node {
        self.with_visitor(Self::root_visit, |v| v.visit_each_child(node))
    }

    /// Go `tx.Visitor().VisitSlice(nodes)`.
    fn visit_slice(&mut self, nodes: &[Node]) -> (Vec<Node>, bool) {
        self.with_visitor(Self::root_visit, |v| v.visit_slice(nodes))
    }

    /// Go `tx.Visitor().VisitEmbeddedStatement(node)`.
    fn visit_embedded_statement(&mut self, node: Node) -> Node {
        self.with_visitor(Self::root_visit, |v| v.visit_embedded_statement(node))
    }

    /// Go `tx.EmitContext().VisitFunctionBody(node, tx.Visitor())`.
    fn visit_function_body(&mut self, node: Node) -> Node {
        let ec = self.ec();
        self.with_visitor(Self::root_visit, |v| ec.visit_function_body(node, v))
    }

    /// Go `tx.EmitContext().VisitIterationBody(node, tx.Visitor())`.
    fn visit_iteration_body(&mut self, node: Node) -> Node {
        let ec = self.ec();
        self.with_visitor(Self::root_visit, |v| ec.visit_iteration_body(node, v))
    }

    /// Go `tx.EmitContext().VisitParameters(nodes, tx.Visitor())`.
    fn visit_parameters(&mut self, nodes: NodeList) -> NodeList {
        let ec = self.ec();
        self.with_visitor(Self::root_visit, |v| ec.visit_parameters(nodes, v))
    }

    /// Go `tx.EmitContext().VisitVariableEnvironment(nodes, tx.Visitor())`.
    fn visit_variable_environment(&mut self, nodes: NodeList) -> NodeList {
        let ec = self.ec();
        self.with_visitor(Self::root_visit, |v| {
            ec.visit_variable_environment(nodes, v)
        })
    }

    // Go: transformers/transformer.go:39 Transformer.TransformSourceFile
    fn transform_source_file_impl(&mut self, file: Node) -> Node {
        self.with_visitor(Self::root_visit, |v| v.visit_source_file(file))
    }
}

/// Implements `TxVisitors` and the contract `Transformer` for a transformer
/// struct with an `emit_context: Rc<EmitContext>` field and an inherent
/// `fn visit(&mut self, node: Node) -> Node` (Go `tx.visit`).
macro_rules! impl_es_transformer {
    ($t:ty) => {
        impl $crate::transformers::estransforms::utilities::TxVisitors for $t {
            fn ec(&self) -> Rc<$crate::printer::EmitContext> {
                self.emit_context.clone()
            }

            fn root_visit(&mut self, node: Node) -> Node {
                <$t>::visit(self, node)
            }
        }

        impl $crate::transformers::estransforms::contract::Transformer for $t {
            fn emit_context(&self) -> &Rc<$crate::printer::EmitContext> {
                &self.emit_context
            }

            fn transform_source_file(&mut self, file: Node) -> Node {
                $crate::transformers::estransforms::utilities::TxVisitors::transform_source_file_impl(
                    self, file,
                )
            }
        }
    };
}
pub(crate) use impl_es_transformer;

/// Go `node.AsSourceFile().ScriptKind` for a parsed or factory-made file.
pub(crate) fn source_file_script_kind(file: Node) -> ScriptKind {
    if is_synthetic_node(file) {
        return with_synthetic_source_file(file, |d| d.script_kind);
    }
    source_file_info(file).script_kind
}

/// Go `node.AsSourceFile().IsDeclarationFile` for a parsed or factory-made file.
pub(crate) fn source_file_is_declaration_file(file: Node) -> bool {
    if is_synthetic_node(file) {
        return with_synthetic_source_file(file, |d| d.is_declaration_file);
    }
    source_file_info(file).is_declaration_file
}

/// Go `ast.IsExternalModule(file)` for a parsed or factory-made file.
// PORT: `ast::is_external_module` reads `SourceFileInfo`, which exists only
// for parsed files. An earlier transformer may have updated the file.
pub(crate) fn source_file_is_external_module(file: Node) -> bool {
    if is_synthetic_node(file) {
        return with_synthetic_source_file(file, |d| d.external_module_indicator.is_some());
    }
    is_external_module(file)
}

/// Go `^flags` for a `ModifierFlags` mask.
pub(crate) fn all_modifiers_except(flags: ModifierFlags) -> ModifierFlags {
    !flags
}

// Go: transformers/estransforms/utilities.go:10 convertClassDeclarationToClassExpression
pub(crate) fn convert_class_declaration_to_class_expression(
    emit_context: &EmitContext,
    node: Node,
) -> Node {
    let f = emit_context.factory();
    let updated = f.new_class_expression(
        extract_modifiers(
            emit_context,
            node.modifiers(),
            all_modifiers_except(ModifierFlags::EXPORT_DEFAULT),
        ),
        node.name(),
        node.type_parameter_list(),
        node.heritage_clauses(),
        node.member_list(),
    );
    emit_context.set_original(updated, node);
    set_node_loc(updated, node.loc());
    updated
}

// Go: transformers/estransforms/utilities.go:23 createNotNullCondition
pub(crate) fn create_not_null_condition(
    emit_context: &EmitContext,
    left: Node,
    right: Node,
    invert: bool,
) -> Node {
    let mut token = SyntaxKind::ExclamationEqualsEqualsToken;
    let mut op = SyntaxKind::AmpersandAmpersandToken;
    if invert {
        token = SyntaxKind::EqualsEqualsEqualsToken;
        op = SyntaxKind::BarBarToken;
    }

    let f = emit_context.factory();
    f.new_binary_expression(
        ModifierList::NIL,
        f.new_binary_expression(
            ModifierList::NIL,
            left,
            Node::NIL,
            f.new_token(token),
            f.new_keyword_expression(SyntaxKind::NullKeyword),
        ),
        Node::NIL,
        f.new_token(op),
        f.new_binary_expression(
            ModifierList::NIL,
            right,
            Node::NIL,
            f.new_token(token),
            f.new_void_zero_expression(),
        ),
    )
}

// Go: transformers/estransforms/utilities.go:55 superAccessState
/// superAccessState tracks super property/element accesses and super property assignments
/// within async function or async generator bodies. It is embedded by both asyncTransformer
/// and forawaitTransformer to share the tracking logic.
// PORT: Go stores the factory and a visitor. Here the state keeps the emit
// context (its factory is the Go factory) and builds the visitor on demand.
#[derive(Default)]
pub(crate) struct SuperAccessState {
    pub(crate) emit_context: Option<Rc<EmitContext>>,

    /// Keeps track of property names accessed on super (`super.x`) within async functions.
    pub(crate) captured_super_properties: Option<IndexSet<String>>,
    /// Whether the async function contains an element access on super (`super[x]`).
    pub(crate) has_super_element_access: bool,
    pub(crate) has_super_property_assignment: bool,

    pub(crate) super_binding: Node,
    pub(crate) super_index_binding: Node,
}

impl SuperAccessState {
    fn factory(&self) -> &NodeFactory {
        self.emit_context
            .as_ref()
            .expect("super access visitor is not initialized")
            .factory()
    }

    // Go: transformers/estransforms/utilities.go:69 superAccessState.initSuperAccessVisitor
    pub(crate) fn init_super_access_visitor(&mut self, emit_context: &Rc<EmitContext>) {
        self.emit_context = Some(emit_context.clone());
    }

    /// Runs `f` with Go `s.superAccessVisitor`.
    fn with_super_access_visitor<R>(
        &mut self,
        f: impl FnOnce(&mut NodeVisitor<'_, &mut SuperAccessState>) -> R,
    ) -> R {
        let ec = self
            .emit_context
            .clone()
            .expect("super access visitor is not initialized");
        let mut visitor = ec.new_node_visitor(
            |node, v: &mut NodeVisitor<'_, &mut SuperAccessState>| {
                v.ctx.visit_super_access_node(node)
            },
            self,
        );
        f(&mut visitor)
    }

    // Go: transformers/estransforms/utilities.go:77 superAccessState.visitSuperAccessNode
    /// visitSuperAccessNode walks the async/generator body and replaces super property/element
    /// accesses with _super/_superIndex references. This is necessary because the async body
    /// ends up inside a generator function where `super` is not valid.
    fn visit_super_access_node(&mut self, node: Node) -> Node {
        match node.kind() {
            SyntaxKind::CallExpression => {
                if is_super_property(node.expression()) {
                    return self.substitute_call_expression_with_super_access(node);
                }
                self.with_super_access_visitor(|v| v.visit_each_child(node))
            }
            SyntaxKind::PropertyAccessExpression => {
                if node.expression().kind() == SyntaxKind::SuperKeyword {
                    // super.x → _super.x
                    let f = self.factory();
                    return f.new_property_access_expression(
                        self.super_binding,
                        Node::NIL,
                        node.name(),
                        NodeFlags::NONE,
                    );
                }
                self.with_super_access_visitor(|v| v.visit_each_child(node))
            }
            SyntaxKind::ElementAccessExpression => {
                if node.expression().kind() == SyntaxKind::SuperKeyword {
                    // super[x] → _superIndex(x) or _superIndex(x).value
                    return self
                        .create_super_element_access_in_async_method(node.argument_expression());
                }
                self.with_super_access_visitor(|v| v.visit_each_child(node))
            }
            // Don't recurse into non-arrow function scopes or classes
            SyntaxKind::FunctionExpression
            | SyntaxKind::FunctionDeclaration
            | SyntaxKind::MethodDeclaration
            | SyntaxKind::GetAccessor
            | SyntaxKind::SetAccessor
            | SyntaxKind::Constructor
            | SyntaxKind::ClassDeclaration
            | SyntaxKind::ClassExpression => node,
            _ => self.with_super_access_visitor(|v| v.visit_each_child(node)),
        }
    }

    // Go: transformers/estransforms/utilities.go:111 superAccessState.substituteSuperAccessesInBody
    pub(crate) fn substitute_super_accesses_in_body(&mut self, body: Node) -> Node {
        self.with_super_access_visitor(|v| v.visit_node(body))
    }

    /// Go `s.superAccessVisitor.VisitNodes(nodes)`.
    pub(crate) fn super_access_visit_nodes(&mut self, nodes: NodeList) -> NodeList {
        self.with_super_access_visitor(|v| v.visit_nodes(nodes))
    }

    // Go: transformers/estransforms/utilities.go:116 superAccessState.substituteCallExpressionWithSuperAccess
    /// substituteCallExpressionWithSuperAccess handles super.x(args) and super[x](args).
    // PORT: Go passes `s.superAccessVisitor` as `visitor`; it is built here.
    fn substitute_call_expression_with_super_access(&mut self, call: Node) -> Node {
        let expression = call.expression();
        let target;

        if is_property_access_expression(expression) {
            // super.x(args) → _super.x.call(this, args)
            target = self.factory().new_property_access_expression(
                self.super_binding,
                Node::NIL,
                expression.name(),
                NodeFlags::NONE,
            );
        } else if is_element_access_expression(expression) {
            // super[x](args) → _superIndex(x).call(this, args) or _superIndex(x).value.call(this, args)
            target =
                self.create_super_element_access_in_async_method(expression.argument_expression());
        } else {
            return self.with_super_access_visitor(|v| v.visit_each_child(call));
        }

        let ec = self
            .emit_context
            .clone()
            .expect("super access visitor is not initialized");
        let f = ec.factory();
        let call_target = f.new_property_access_expression(
            target,
            Node::NIL,
            f.new_identifier("call"),
            NodeFlags::NONE,
        );

        let mut all_args: Vec<Node> = vec![f.new_this_expression()];
        if call.argument_list().is_some() {
            let visited_args =
                self.with_super_access_visitor(|v| v.visit_nodes(call.argument_list()));
            if visited_args.is_some() {
                all_args.extend(visited_args.nodes().iter());
            }
        }

        let result = f.new_call_expression(
            call_target,
            Node::NIL,
            NodeList::NIL,
            f.new_node_list(&all_args),
            NodeFlags::NONE,
        );
        set_node_loc(result, call.loc());
        result
    }

    // Go: transformers/estransforms/utilities.go:158 superAccessState.createSuperElementAccessInAsyncMethod
    /// createSuperElementAccessInAsyncMethod creates _superIndex(x) or _superIndex(x).value.
    pub(crate) fn create_super_element_access_in_async_method(
        &self,
        argument_expression: Node,
    ) -> Node {
        let f = self.factory();
        let super_index_call = f.new_call_expression(
            self.super_index_binding,
            Node::NIL,
            NodeList::NIL,
            f.new_node_list(&[argument_expression]),
            NodeFlags::NONE,
        );
        if self.has_super_property_assignment {
            return f.new_property_access_expression(
                super_index_call,
                Node::NIL,
                f.new_identifier("value"),
                NodeFlags::NONE,
            );
        }
        super_index_call
    }

    // Go: transformers/estransforms/utilities.go:182 superAccessState.createSuperAccessVariableStatement
    /// createSuperAccessVariableStatement creates a variable named `_super` with accessor
    /// properties for the given property names.
    ///
    /// Create a variable declaration with a getter/setter (if binding) definition for each name:
    ///
    /// ```text
    /// const _super = Object.create(null, {
    ///     x: { get: () => super.x },                           // read-only
    ///     x: { get: () => super.x, set: (v) => super.x = v }, // read-write
    /// });
    /// ```
    pub(crate) fn create_super_access_variable_statement(&self) -> Node {
        let f = self.factory();
        let mut accessors: Vec<Node> = Vec::new();

        let names: Vec<String> = self
            .captured_super_properties
            .as_ref()
            .map(|s| s.iter().cloned().collect())
            .unwrap_or_default();
        for name in &names {
            let mut descriptor_properties: Vec<Node> = Vec::new();

            // getter: get: () => super.name
            let getter_body = f.new_property_access_expression(
                f.new_keyword_expression(SyntaxKind::SuperKeyword),
                Node::NIL,
                f.new_identifier(name.as_str()),
                NodeFlags::NONE,
            );
            let getter_arrow = f.new_arrow_function(
                ModifierList::NIL,
                NodeList::NIL,
                f.new_node_list(&[]),
                Node::NIL,
                Node::NIL,
                f.new_token(SyntaxKind::EqualsGreaterThanToken),
                getter_body,
            );
            let getter = f.new_property_assignment(
                ModifierList::NIL,
                f.new_identifier("get"),
                Node::NIL,
                Node::NIL,
                getter_arrow,
            );
            descriptor_properties.push(getter);

            if self.has_super_property_assignment {
                // setter: set: v => super.name = v
                let v_param = f.new_parameter_declaration(
                    ModifierList::NIL,
                    Node::NIL,
                    f.new_identifier("v"),
                    Node::NIL,
                    Node::NIL,
                    Node::NIL,
                );
                let super_prop = f.new_property_access_expression(
                    f.new_keyword_expression(SyntaxKind::SuperKeyword),
                    Node::NIL,
                    f.new_identifier(name.as_str()),
                    NodeFlags::NONE,
                );
                let assign_expr = f.new_assignment_expression(super_prop, f.new_identifier("v"));
                let setter_arrow = f.new_arrow_function(
                    ModifierList::NIL,
                    NodeList::NIL,
                    f.new_node_list(&[v_param]),
                    Node::NIL,
                    Node::NIL,
                    f.new_token(SyntaxKind::EqualsGreaterThanToken),
                    assign_expr,
                );
                let setter = f.new_property_assignment(
                    ModifierList::NIL,
                    f.new_identifier("set"),
                    Node::NIL,
                    Node::NIL,
                    setter_arrow,
                );
                descriptor_properties.push(setter);
            }

            let descriptor =
                f.new_object_literal_expression(f.new_node_list(&descriptor_properties), false);
            let accessor = f.new_property_assignment(
                ModifierList::NIL,
                f.new_identifier(name.as_str()),
                Node::NIL,
                Node::NIL,
                descriptor,
            );
            accessors.push(accessor);
        }

        let descriptors_object = f.new_object_literal_expression(f.new_node_list(&accessors), true);

        let object_create_call = f.new_call_expression(
            f.new_property_access_expression(
                f.new_identifier("Object"),
                Node::NIL,
                f.new_identifier("create"),
                NodeFlags::NONE,
            ),
            Node::NIL,
            NodeList::NIL,
            f.new_node_list(&[
                f.new_keyword_expression(SyntaxKind::NullKeyword),
                descriptors_object,
            ]),
            NodeFlags::NONE,
        );

        let decl = f.new_variable_declaration(
            self.super_binding,
            Node::NIL,
            Node::NIL,
            object_create_call,
        );
        let decl_list = f.new_variable_declaration_list(f.new_node_list(&[decl]), NodeFlags::CONST);
        f.new_variable_statement(ModifierList::NIL, decl_list)
    }

    // Go: transformers/estransforms/utilities.go:251 superAccessState.trackSuperAccess
    /// trackSuperAccess records super property/element accesses and super property assignments
    /// for the enclosing async method body. Called from both the main visitor and auxiliary
    /// visitors to ensure super accesses are tracked regardless of whether the node has
    /// transform flags.
    pub(crate) fn track_super_access(&mut self, node: Node) {
        let Some(captured) = self.captured_super_properties.as_mut() else {
            return;
        };
        match node.kind() {
            SyntaxKind::PropertyAccessExpression => {
                if node.expression().kind() == SyntaxKind::SuperKeyword {
                    captured.insert(node.name().text().to_string());
                }
            }
            SyntaxKind::ElementAccessExpression => {
                if node.expression().kind() == SyntaxKind::SuperKeyword {
                    self.has_super_element_access = true;
                }
            }
            SyntaxKind::BinaryExpression => {
                if is_assignment_operator(node.operator_token().kind())
                    && assignment_target_contains_super_property(node.left())
                {
                    self.has_super_property_assignment = true;
                }
            }
            SyntaxKind::PrefixUnaryExpression | SyntaxKind::PostfixUnaryExpression => {
                if is_update_expression(node)
                    && assignment_target_contains_super_property(node.operand())
                {
                    self.has_super_property_assignment = true;
                }
            }
            _ => {}
        }
    }
}

// Go: transformers/estransforms/utilities.go:280 createAccessorPropertyBackingField
/// createAccessorPropertyBackingField creates a private backing field for an `accessor` PropertyDeclaration.
pub(crate) fn create_accessor_property_backing_field(
    f: &NodeFactory,
    node: Node,
    modifiers: ModifierList,
    initializer: Node,
) -> Node {
    f.update_property_declaration(
        node,
        modifiers,
        f.new_generated_private_name_for_node_ex(
            node.name(),
            AutoGenerateOptions {
                suffix: "_accessor_storage".to_string(),
                ..Default::default()
            },
        ),
        Node::NIL, /*postfixToken*/
        Node::NIL, /*typeNode*/
        initializer,
    )
}
