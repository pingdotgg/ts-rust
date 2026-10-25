//! Port of Go `ast.NodeVisitor` (`ast/visitor.go`) and the `VisitEachChild`
//! methods (`ast/ast_generated.go`, plus `SourceFile` and
//! `visitEachChild_JSDocParameterOrPropertyTag` from `ast/ast.go`).
//!
//! PORT: Go visitor callbacks are closures that capture the visitor and any
//! state they need. Here:
//! - `NodeVisitor` owns a context value `ctx: C` (for example
//!   `&mut Checker`), so a callback can reach mutable state through the
//!   visitor it gets. `C` defaults to `()`.
//! - The `visit` callback and every hook get `(node, &mut NodeVisitor)`. Go
//!   `Visit(node)` gets only the node; the visitor argument replaces the Go
//!   closure capture of the visitor.
//! - Callbacks are `Rc<dyn Fn>`, so a callback can call back into the visitor
//!   (Go recursion through `v.VisitEachChild`). Keep callback state in
//!   `Cell`/`RefCell` or in `ctx`.
//! - Go has exported and unexported methods with the same name (`VisitNode`
//!   and `visitNode`). The exported ones keep the plain snake names
//!   (`visit_node`, `visit_nodes`, `visit_modifiers`,
//!   `visit_embedded_statement`), as the printer plan's contract gives them.
//!   The unexported hook-aware ones get a `_hooked` suffix.

use crate::astdata::NodeData;
use crate::prelude::*;

/// Go `NodeVisitor.Visit`.
pub type VisitFn<'a, C> = Rc<dyn Fn(Node, &mut NodeVisitor<'a, C>) -> Node + 'a>;
/// A Go hook over one node.
pub type VisitNodeHook<'a, C> = Rc<dyn Fn(Node, &mut NodeVisitor<'a, C>) -> Node + 'a>;
/// A Go hook over a `NodeList`.
pub type VisitNodesHook<'a, C> = Rc<dyn Fn(NodeList, &mut NodeVisitor<'a, C>) -> NodeList + 'a>;
/// A Go hook over a `ModifierList`.
pub type VisitModifiersHook<'a, C> =
    Rc<dyn Fn(ModifierList, &mut NodeVisitor<'a, C>) -> ModifierList + 'a>;

// Go: ast/visitor.go:16 NodeVisitorHooks
/// These hooks are used to intercept the default behavior of the visitor.
pub struct NodeVisitorHooks<'a, C> {
    /// Overrides visiting a Node. Only invoked by the VisitEachChild method on a given Node subtype.
    pub visit_node: Option<VisitNodeHook<'a, C>>,
    /// Overrides visiting a TokenNode. Only invoked by the VisitEachChild method on a given Node subtype.
    pub visit_token: Option<VisitNodeHook<'a, C>>,
    /// Overrides visiting a NodeList. Only invoked by the VisitEachChild method on a given Node subtype.
    pub visit_nodes: Option<VisitNodesHook<'a, C>>,
    /// Overrides visiting a ModifierList. Only invoked by the VisitEachChild method on a given Node subtype.
    pub visit_modifiers: Option<VisitModifiersHook<'a, C>>,
    /// Overrides visiting a Node when it is the embedded statement body of an iteration statement, `if` statement, or `with` statement. Only invoked by the VisitEachChild method on a given Node subtype.
    pub visit_embedded_statement: Option<VisitNodeHook<'a, C>>,
    /// Overrides visiting a Node when it is the embedded statement body of an iteration statement. Only invoked by the VisitEachChild method on a given Node subtype.
    pub visit_iteration_body: Option<VisitNodeHook<'a, C>>,
    /// Overrides visiting a ParameterList. Only invoked by the VisitEachChild method on a given Node subtype.
    pub visit_parameters: Option<VisitNodesHook<'a, C>>,
    /// Overrides visiting a function body. Only invoked by the VisitEachChild method on a given Node subtype.
    pub visit_function_body: Option<VisitNodeHook<'a, C>>,
    /// Overrides visiting a variable environment. Only invoked by the VisitEachChild method on a given Node subtype.
    pub visit_top_level_statements: Option<VisitNodesHook<'a, C>>,
}

impl<C> Default for NodeVisitorHooks<'_, C> {
    fn default() -> Self {
        Self {
            visit_node: None,
            visit_token: None,
            visit_nodes: None,
            visit_modifiers: None,
            visit_embedded_statement: None,
            visit_iteration_body: None,
            visit_parameters: None,
            visit_function_body: None,
            visit_top_level_statements: None,
        }
    }
}

impl<C> Clone for NodeVisitorHooks<'_, C> {
    fn clone(&self) -> Self {
        Self {
            visit_node: self.visit_node.clone(),
            visit_token: self.visit_token.clone(),
            visit_nodes: self.visit_nodes.clone(),
            visit_modifiers: self.visit_modifiers.clone(),
            visit_embedded_statement: self.visit_embedded_statement.clone(),
            visit_iteration_body: self.visit_iteration_body.clone(),
            visit_parameters: self.visit_parameters.clone(),
            visit_function_body: self.visit_function_body.clone(),
            visit_top_level_statements: self.visit_top_level_statements.clone(),
        }
    }
}

// Go: ast/visitor.go:9 NodeVisitor
/// Go `ast.NodeVisitor`.
pub struct NodeVisitor<'a, C = ()> {
    /// Required. The callback used to visit a node. `None` is Go `nil`.
    pub visit: Option<VisitFn<'a, C>>,
    /// Required. The NodeFactory used to produce new nodes when passed to VisitEachChild.
    // PORT: Go stores `*NodeFactory`; `None` means the visitor's own default
    // factory (Go `&NodeFactory{}` in `NewNodeVisitor`). Read it with `factory()`.
    pub factory: Option<&'a NodeFactory>,
    default_factory: NodeFactory,
    /// Hooks to be invoked when visiting a node.
    pub hooks: NodeVisitorHooks<'a, C>,
    /// The emit context whose visit methods stand in for the parameters,
    /// function body, iteration body, top-level statements and embedded
    /// statement hooks when those hooks are unset. It replaces five `Rc`
    /// closures that `EmitContext::new_node_visitor` allocated per visitor.
    pub emit_context: Option<&'a crate::printer::EmitContext>,
    /// PORT: state that Go callbacks capture. See the module comment.
    pub ctx: C,
}

/// The plan name for `NodeVisitor`.
pub type Visitor<'a, C = ()> = NodeVisitor<'a, C>;

// Go: ast/visitor.go:28 NewNodeVisitor
// PORT: Go takes a nilable func; set `visit` to `None` after creation for a
// nil callback. `ctx` is the callback state (see the module comment).
pub fn new_node_visitor<'a, C>(
    visit: impl Fn(Node, &mut NodeVisitor<'a, C>) -> Node + 'a,
    factory: Option<&'a NodeFactory>,
    hooks: NodeVisitorHooks<'a, C>,
    ctx: C,
) -> NodeVisitor<'a, C> {
    NodeVisitor {
        visit: Some(Rc::new(visit)),
        factory,
        default_factory: NodeFactory::default(),
        hooks,
        emit_context: None,
        ctx,
    }
}

/// Go `node.AsSyntaxList().Children`.
// PORT: node.rs has no accessor for the `SyntaxList` children.
pub fn syntax_list_children(node: Node) -> Vec<Node> {
    let file = node.file_index();
    with_ast_data(node, |d| match d {
        NodeData::SyntaxList(d) => d.children.iter().map(|&id| Node::new(file, id)).collect(),
        _ => panic!(
            "ast field Children does not exist on node kind {:?}",
            node.kind()
        ),
    })
}

impl<'a, C> NodeVisitor<'a, C> {
    /// Go `v.Factory`.
    #[must_use]
    pub fn factory(&self) -> &NodeFactory {
        self.factory.unwrap_or(&self.default_factory)
    }

    /// Calls `v.Visit(node)`. The caller checks that `visit` is set.
    fn call_visit(&mut self, node: Node) -> Node {
        let visit = self.visit.clone().expect("NodeVisitor.Visit is nil");
        visit(node, self)
    }

    // Go: ast/visitor.go:35 VisitSourceFile
    pub fn visit_source_file(&mut self, node: Node) -> Node {
        let visited = self.visit_node(node);
        // PORT: Go `.AsSourceFile()` is a type assertion that panics on nil or another kind.
        assert!(
            visited.is_some() && visited.kind() == SyntaxKind::SourceFile,
            "VisitSourceFile: the result is not a SourceFile"
        );
        visited
    }

    // Go: ast/visitor.go:45 VisitNode
    /// Visits a Node, possibly returning a new Node in its place.
    ///
    ///   - If the input node is nil, then the output is nil.
    ///   - If v.Visit is nil, then the output is the input.
    ///   - If v.Visit returns nil, then the output is nil.
    ///   - If v.Visit returns a SyntaxList Node, then the output is the only child of the SyntaxList Node.
    pub fn visit_node(&mut self, node: Node) -> Node {
        if node.is_nil() || self.visit.is_none() {
            return node;
        }

        let mut visited = self.call_visit(node);
        if visited.is_some() && visited.kind() == SyntaxKind::SyntaxList {
            let nodes = syntax_list_children(visited);
            if nodes.len() != 1 {
                panic!("Expected only a single node to be written to output");
            }
            visited = nodes[0];
            if visited.is_some() && visited.kind() == SyntaxKind::SyntaxList {
                panic!("The result of visiting and lifting a Node may not be SyntaxList");
            }
        }
        visited
    }

    // Go: ast/visitor.go:74 VisitEmbeddedStatement
    /// Visits an embedded Statement (i.e., the single statement body of a loop, `if..else` branch, etc.), possibly returning a new Statement in its place.
    ///
    ///   - If the input node is nil, then the output is nil.
    ///   - If v.Visit is nil, then the output is the input.
    ///   - If v.Visit returns nil, then the output is nil.
    ///   - If v.Visit returns a SyntaxList Node, then the output is either the only child of the SyntaxList Node, or a Block containing the nodes in the list.
    pub fn visit_embedded_statement(&mut self, node: Node) -> Node {
        if node.is_nil() || self.visit.is_none() {
            return node;
        }

        let visited = self.call_visit(node);
        if visited.is_nil() {
            return Node::NIL;
        }
        self.lift_to_block(visited)
    }

    // Go: ast/visitor.go:94 VisitNodes
    /// Visits a NodeList, possibly returning a new NodeList in its place.
    ///
    ///   - If the input NodeList is nil, the output is nil.
    ///   - If v.Visit is nil, then the output is the input.
    ///   - If v.Visit returns nil, the visited Node will be absent in the output.
    ///   - If v.Visit returns a different Node than the input, a new NodeList will be generated and returned.
    ///   - If v.Visit returns a SyntaxList Node, then the children of that node will be merged into the output and a new NodeList will be returned.
    ///   - If this method returns a new NodeList for any reason, it will have the same Loc as the input NodeList.
    pub fn visit_nodes(&mut self, nodes: NodeList) -> NodeList {
        if nodes.is_nil() || self.visit.is_none() {
            return nodes;
        }

        // PERF: read the list in place. A new Vec is made only when a node
        // changes (Go `VisitSlice` returns the input slice otherwise).
        if let Some(result) = self.visit_slice_changed(nodes.nodes().iter()) {
            // PORT: Go `list := v.Factory.NewNodeList(result); list.Loc = nodes.Loc`.
            // A factory list fixes its `Loc` at creation, so it is passed in.
            return self.factory().new_node_list_with_loc(&result, nodes.loc());
        }

        nodes
    }

    // Go: ast/visitor.go:116 VisitModifiers
    /// Visits a ModifierList, possibly returning a new ModifierList in its place.
    ///
    ///   - If the input ModifierList is nil, the output is nil.
    ///   - If v.Visit is nil, then the output is the input.
    ///   - If v.Visit returns nil, the visited Node will be absent in the output.
    ///   - If v.Visit returns a different Node than the input, a new ModifierList will be generated and returned.
    ///   - If v.Visit returns a SyntaxList Node, then the children of that node will be merged into the output and a new NodeList will be returned.
    ///   - If this method returns a new NodeList for any reason, it will have the same Loc as the input NodeList.
    pub fn visit_modifiers(&mut self, nodes: ModifierList) -> ModifierList {
        if nodes.is_nil() || self.visit.is_none() {
            return nodes;
        }

        // PERF: read the list in place, as in `visit_nodes`.
        if let Some(result) = self.visit_slice_changed(nodes.nodes().iter()) {
            // PORT: Go `list := v.Factory.NewModifierList(result); list.Loc = nodes.Loc`.
            // A factory list fixes its `Loc` at creation, so it is passed in.
            return self
                .factory()
                .new_modifier_list_with_loc(&result, nodes.node_list().loc());
        }

        nodes
    }

    // Go: ast/visitor.go:137 VisitSlice
    /// Visits a slice of Nodes, returning the resulting slice and a value indicating whether the slice was changed.
    ///
    ///   - If the input slice is nil, the output is nil.
    ///   - If v.Visit is nil, then the output is the input.
    ///   - If v.Visit returns nil, the visited Node will be absent in the output.
    ///   - If v.Visit returns a different Node than the input, a new slice will be generated and returned.
    ///   - If v.Visit returns a SyntaxList Node, then the children of that node will be merged into the output and a new slice will be returned.
    // PORT: an unchanged result is a copy of the input, not the same slice.
    // `visit_slice_changed` gives `None` instead of that copy.
    pub fn visit_slice(&mut self, nodes: &[Node]) -> (Vec<Node>, bool) {
        match self.visit_slice_changed(nodes.iter().copied()) {
            Some(updated) => (updated, true),
            None => (nodes.to_vec(), false),
        }
    }

    /// Go `VisitSlice` over any node sequence (a `&[Node]` or a
    /// `NodeSlice`). `None` means "not changed": Go returns the input slice,
    /// so the caller keeps its own input and nothing is copied.
    // PORT: the body of Go `VisitSlice` (ast/visitor.go:138).
    // PERF: `nodes` is read in place. The clone of the iterator rereads the
    // unchanged prefix only when a node changes (Go `slices.Clone(nodes[:i])`).
    pub fn visit_slice_changed<I>(&mut self, nodes: I) -> Option<Vec<Node>>
    where
        I: Iterator<Item = Node> + Clone,
    {
        if self.visit.is_none() {
            return None;
        }

        let prefix = nodes.clone();
        let mut rest = nodes;
        let mut i = 0;
        while let Some(node) = rest.next() {
            if self.visit.is_none() {
                break;
            }

            let mut visited = self.call_visit(node);
            if visited.is_nil() || visited != node {
                let mut updated: Vec<Node> = Vec::with_capacity(prefix.size_hint().0);
                updated.extend(prefix.take(i));

                loop {
                    // finish prior loop
                    if visited.is_nil() {
                        // do nothing
                    } else if visited.kind() == SyntaxKind::SyntaxList {
                        updated.extend(syntax_list_children(visited));
                    } else {
                        updated.push(visited);
                    }

                    // loop over remaining elements
                    let Some(next) = rest.next() else {
                        break;
                    };

                    if self.visit.is_some() {
                        visited = self.call_visit(next);
                    } else {
                        updated.push(next);
                        updated.extend(rest);
                        break;
                    }
                }

                return Some(updated);
            }
            i += 1;
        }

        None
    }

    // Go: ast/visitor.go:186 VisitEachChild
    /// Visits each child of a Node, possibly returning a new Node of the same kind in its place.
    pub fn visit_each_child(&mut self, node: Node) -> Node {
        if node.is_nil() || self.visit.is_none() {
            return node;
        }

        node.visit_each_child(self)
    }

    // Go: ast/visitor.go:260 liftToBlock
    fn lift_to_block(&mut self, node: Node) -> Node {
        let mut nodes: Vec<Node> = Vec::new();
        if node.is_some() {
            if node.kind() == SyntaxKind::SyntaxList {
                nodes = syntax_list_children(node);
            } else {
                nodes = vec![node];
            }
        }
        let node = if nodes.len() == 1 {
            nodes[0]
        } else {
            let list = self.factory().new_node_list(&nodes);
            self.factory().new_block(list, true /*multiLine*/)
        };
        if node.kind() == SyntaxKind::SyntaxList {
            panic!("The result of visiting and lifting a Node may not be SyntaxList");
        }
        node
    }
}

/// The `NodeVisitor` methods that the `visit_each_child_*` functions call:
/// `factory` and the Go unexported, hook-aware methods. On wasm the
/// functions get a `&mut dyn ChildVisitor` (see `Node::visit_each_child`).
trait ChildVisitor {
    fn factory(&self) -> &NodeFactory;
    fn visit_node_hooked(&mut self, node: Node) -> Node;
    fn visit_embedded_statement_hooked(&mut self, node: Node) -> Node;
    fn visit_iteration_body(&mut self, node: Node) -> Node;
    fn visit_function_body(&mut self, node: Node) -> Node;
    fn visit_token(&mut self, node: Node) -> Node;
    fn visit_nodes_hooked(&mut self, nodes: NodeList) -> NodeList;
    fn visit_modifiers_hooked(&mut self, nodes: ModifierList) -> ModifierList;
    fn visit_parameters(&mut self, nodes: NodeList) -> NodeList;
    fn visit_top_level_statements(&mut self, nodes: NodeList) -> NodeList;
}

impl<C> ChildVisitor for NodeVisitor<'_, C> {
    fn factory(&self) -> &NodeFactory {
        NodeVisitor::factory(self)
    }

    // Go: ast/visitor.go:194 visitNode
    fn visit_node_hooked(&mut self, node: Node) -> Node {
        if let Some(hook) = self.hooks.visit_node.clone() {
            return hook(node, self);
        }
        self.visit_node(node)
    }

    // Go: ast/visitor.go:201 visitEmbeddedStatement
    fn visit_embedded_statement_hooked(&mut self, node: Node) -> Node {
        if let Some(hook) = self.hooks.visit_embedded_statement.clone() {
            return hook(node, self);
        }
        if let Some(ec) = self.emit_context {
            return ec.visit_embedded_statement(node, self);
        }
        if let Some(hook) = self.hooks.visit_node.clone() {
            let visited = hook(node, self);
            return self.lift_to_block(visited);
        }
        self.visit_embedded_statement(node)
    }

    // Go: ast/visitor.go:211 visitIterationBody
    fn visit_iteration_body(&mut self, node: Node) -> Node {
        if let Some(hook) = self.hooks.visit_iteration_body.clone() {
            return hook(node, self);
        }
        if let Some(ec) = self.emit_context {
            return ec.visit_iteration_body(node, self);
        }
        self.visit_embedded_statement_hooked(node)
    }

    // Go: ast/visitor.go:218 visitFunctionBody
    fn visit_function_body(&mut self, node: Node) -> Node {
        if let Some(hook) = self.hooks.visit_function_body.clone() {
            return hook(node, self);
        }
        if let Some(ec) = self.emit_context {
            return ec.visit_function_body(node, self);
        }
        self.visit_node_hooked(node)
    }

    // Go: ast/visitor.go:225 visitToken
    fn visit_token(&mut self, node: Node) -> Node {
        if let Some(hook) = self.hooks.visit_token.clone() {
            return hook(node, self);
        }
        self.visit_node(node)
    }

    // Go: ast/visitor.go:232 visitNodes
    fn visit_nodes_hooked(&mut self, nodes: NodeList) -> NodeList {
        if let Some(hook) = self.hooks.visit_nodes.clone() {
            return hook(nodes, self);
        }
        self.visit_nodes(nodes)
    }

    // Go: ast/visitor.go:239 visitModifiers
    fn visit_modifiers_hooked(&mut self, nodes: ModifierList) -> ModifierList {
        if let Some(hook) = self.hooks.visit_modifiers.clone() {
            return hook(nodes, self);
        }
        self.visit_modifiers(nodes)
    }

    // Go: ast/visitor.go:246 visitParameters
    fn visit_parameters(&mut self, nodes: NodeList) -> NodeList {
        if let Some(hook) = self.hooks.visit_parameters.clone() {
            return hook(nodes, self);
        }
        if let Some(ec) = self.emit_context {
            return ec.visit_parameters(nodes, self);
        }
        self.visit_nodes_hooked(nodes)
    }

    // Go: ast/visitor.go:253 visitTopLevelStatements
    fn visit_top_level_statements(&mut self, nodes: NodeList) -> NodeList {
        if let Some(hook) = self.hooks.visit_top_level_statements.clone() {
            return hook(nodes, self);
        }
        if let Some(ec) = self.emit_context {
            return ec.visit_variable_environment(nodes, self);
        }
        self.visit_nodes_hooked(nodes)
    }
}

impl Node {
    // Go: ast/ast_generated.go:9114 (n *Node) VisitEachChild
    /// Go `node.VisitEachChild(v)`: dispatches on the Go node data type.
    pub fn visit_each_child<C>(self, v: &mut NodeVisitor<'_, C>) -> Node {
        // PERF(wasm): the dispatch and the `visit_each_child_*` functions are
        // generic over the visitor, so the module had one copy of them per
        // context type C. On wasm they get a `&mut dyn ChildVisitor`, so there
        // is one copy: the module is 94 KB smaller raw, 6.1 KB gzip and 1.4 KB
        // brotli. Native builds keep the static calls.
        #[cfg(target_family = "wasm")]
        let v: &mut dyn ChildVisitor = v;
        self.visit_each_child_with(v)
    }

    /// `visit_each_child` for any `ChildVisitor`.
    fn visit_each_child_with<V: ChildVisitor + ?Sized>(self, v: &mut V) -> Node {
        let node = self;
        with_ast_data(node, |d| match d {
            NodeData::SourceFile(_) => visit_each_child_source_file(node, v),
            NodeData::QualifiedName(_) => visit_each_child_qualified_name(node, v),
            NodeData::ComputedPropertyName(_) => visit_each_child_computed_property_name(node, v),
            NodeData::Decorator(_) => visit_each_child_decorator(node, v),
            NodeData::IfStatement(_) => visit_each_child_if_statement(node, v),
            NodeData::DoStatement(_) => visit_each_child_do_statement(node, v),
            NodeData::WhileStatement(_) => visit_each_child_while_statement(node, v),
            NodeData::ForStatement(_) => visit_each_child_for_statement(node, v),
            NodeData::ForInOrOfStatement(_) => visit_each_child_for_in_or_of_statement(node, v),
            NodeData::BreakStatement(_) => visit_each_child_break_statement(node, v),
            NodeData::ContinueStatement(_) => visit_each_child_continue_statement(node, v),
            NodeData::ReturnStatement(_) => visit_each_child_return_statement(node, v),
            NodeData::WithStatement(_) => visit_each_child_with_statement(node, v),
            NodeData::SwitchStatement(_) => visit_each_child_switch_statement(node, v),
            NodeData::CaseBlock(_) => visit_each_child_case_block(node, v),
            NodeData::CaseOrDefaultClause(_) => visit_each_child_case_or_default_clause(node, v),
            NodeData::ThrowStatement(_) => visit_each_child_throw_statement(node, v),
            NodeData::TryStatement(_) => visit_each_child_try_statement(node, v),
            NodeData::CatchClause(_) => visit_each_child_catch_clause(node, v),
            NodeData::LabeledStatement(_) => visit_each_child_labeled_statement(node, v),
            NodeData::ExpressionStatement(_) => visit_each_child_expression_statement(node, v),
            NodeData::Block(_) => visit_each_child_block(node, v),
            NodeData::VariableStatement(_) => visit_each_child_variable_statement(node, v),
            NodeData::VariableDeclaration(_) => visit_each_child_variable_declaration(node, v),
            NodeData::VariableDeclarationList(_) => {
                visit_each_child_variable_declaration_list(node, v)
            }
            NodeData::BindingPattern(_) => visit_each_child_binding_pattern(node, v),
            NodeData::ParameterDeclaration(_) => visit_each_child_parameter_declaration(node, v),
            NodeData::BindingElement(_) => visit_each_child_binding_element(node, v),
            NodeData::MissingDeclaration(_) => visit_each_child_missing_declaration(node, v),
            NodeData::FunctionDeclaration(_) => visit_each_child_function_declaration(node, v),
            NodeData::ClassDeclaration(_) => visit_each_child_class_declaration(node, v),
            NodeData::ClassExpression(_) => visit_each_child_class_expression(node, v),
            NodeData::HeritageClause(_) => visit_each_child_heritage_clause(node, v),
            NodeData::InterfaceDeclaration(_) => visit_each_child_interface_declaration(node, v),
            NodeData::TypeAliasDeclaration(_) => visit_each_child_type_alias_declaration(node, v),
            NodeData::EnumMember(_) => visit_each_child_enum_member(node, v),
            NodeData::EnumDeclaration(_) => visit_each_child_enum_declaration(node, v),
            NodeData::ModuleBlock(_) => visit_each_child_module_block(node, v),
            NodeData::ImportDeclaration(_) => visit_each_child_import_declaration(node, v),
            NodeData::ExternalModuleReference(_) => {
                visit_each_child_external_module_reference(node, v)
            }
            NodeData::NamespaceImport(_) => visit_each_child_namespace_import(node, v),
            NodeData::NamedImports(_) => visit_each_child_named_imports(node, v),
            NodeData::ExportAssignment(_) => visit_each_child_export_assignment(node, v),
            NodeData::NamespaceExportDeclaration(_) => {
                visit_each_child_namespace_export_declaration(node, v)
            }
            NodeData::NamespaceExport(_) => visit_each_child_namespace_export(node, v),
            NodeData::NamedExports(_) => visit_each_child_named_exports(node, v),
            NodeData::ExportSpecifier(_) => visit_each_child_export_specifier(node, v),
            NodeData::CallSignatureDeclaration(_) => {
                visit_each_child_call_signature_declaration(node, v)
            }
            NodeData::ConstructSignatureDeclaration(_) => {
                visit_each_child_construct_signature_declaration(node, v)
            }
            NodeData::ConstructorDeclaration(_) => {
                visit_each_child_constructor_declaration(node, v)
            }
            NodeData::GetAccessorDeclaration(_) => {
                visit_each_child_get_accessor_declaration(node, v)
            }
            NodeData::SetAccessorDeclaration(_) => {
                visit_each_child_set_accessor_declaration(node, v)
            }
            NodeData::IndexSignatureDeclaration(_) => {
                visit_each_child_index_signature_declaration(node, v)
            }
            NodeData::MethodSignatureDeclaration(_) => {
                visit_each_child_method_signature_declaration(node, v)
            }
            NodeData::MethodDeclaration(_) => visit_each_child_method_declaration(node, v),
            NodeData::PropertySignatureDeclaration(_) => {
                visit_each_child_property_signature_declaration(node, v)
            }
            NodeData::PropertyDeclaration(_) => visit_each_child_property_declaration(node, v),
            NodeData::ClassStaticBlockDeclaration(_) => {
                visit_each_child_class_static_block_declaration(node, v)
            }
            NodeData::BinaryExpression(_) => visit_each_child_binary_expression(node, v),
            NodeData::PrefixUnaryExpression(_) => visit_each_child_prefix_unary_expression(node, v),
            NodeData::PostfixUnaryExpression(_) => {
                visit_each_child_postfix_unary_expression(node, v)
            }
            NodeData::YieldExpression(_) => visit_each_child_yield_expression(node, v),
            NodeData::ArrowFunction(_) => visit_each_child_arrow_function(node, v),
            NodeData::FunctionExpression(_) => visit_each_child_function_expression(node, v),
            NodeData::AsExpression(_) => visit_each_child_as_expression(node, v),
            NodeData::SatisfiesExpression(_) => visit_each_child_satisfies_expression(node, v),
            NodeData::ConditionalExpression(_) => visit_each_child_conditional_expression(node, v),
            NodeData::PropertyAccessExpression(_) => {
                visit_each_child_property_access_expression(node, v)
            }
            NodeData::ElementAccessExpression(_) => {
                visit_each_child_element_access_expression(node, v)
            }
            NodeData::CallExpression(_) => visit_each_child_call_expression(node, v),
            NodeData::NewExpression(_) => visit_each_child_new_expression(node, v),
            NodeData::MetaProperty(_) => visit_each_child_meta_property(node, v),
            NodeData::NonNullExpression(_) => visit_each_child_non_null_expression(node, v),
            NodeData::SpreadElement(_) => visit_each_child_spread_element(node, v),
            NodeData::TemplateExpression(_) => visit_each_child_template_expression(node, v),
            NodeData::TemplateSpan(_) => visit_each_child_template_span(node, v),
            NodeData::TaggedTemplateExpression(_) => {
                visit_each_child_tagged_template_expression(node, v)
            }
            NodeData::ParenthesizedExpression(_) => {
                visit_each_child_parenthesized_expression(node, v)
            }
            NodeData::ArrayLiteralExpression(_) => {
                visit_each_child_array_literal_expression(node, v)
            }
            NodeData::ObjectLiteralExpression(_) => {
                visit_each_child_object_literal_expression(node, v)
            }
            NodeData::SpreadAssignment(_) => visit_each_child_spread_assignment(node, v),
            NodeData::PropertyAssignment(_) => visit_each_child_property_assignment(node, v),
            NodeData::ShorthandPropertyAssignment(_) => {
                visit_each_child_shorthand_property_assignment(node, v)
            }
            NodeData::DeleteExpression(_) => visit_each_child_delete_expression(node, v),
            NodeData::TypeOfExpression(_) => visit_each_child_type_of_expression(node, v),
            NodeData::VoidExpression(_) => visit_each_child_void_expression(node, v),
            NodeData::AwaitExpression(_) => visit_each_child_await_expression(node, v),
            NodeData::TypeAssertion(_) => visit_each_child_type_assertion(node, v),
            NodeData::UnionTypeNode(_) => visit_each_child_union_type_node(node, v),
            NodeData::IntersectionTypeNode(_) => visit_each_child_intersection_type_node(node, v),
            NodeData::ConditionalTypeNode(_) => visit_each_child_conditional_type_node(node, v),
            NodeData::TypeOperatorNode(_) => visit_each_child_type_operator_node(node, v),
            NodeData::InferTypeNode(_) => visit_each_child_infer_type_node(node, v),
            NodeData::ArrayTypeNode(_) => visit_each_child_array_type_node(node, v),
            NodeData::IndexedAccessTypeNode(_) => {
                visit_each_child_indexed_access_type_node(node, v)
            }
            NodeData::TypeReferenceNode(_) => visit_each_child_type_reference_node(node, v),
            NodeData::ExpressionWithTypeArguments(_) => {
                visit_each_child_expression_with_type_arguments(node, v)
            }
            NodeData::LiteralTypeNode(_) => visit_each_child_literal_type_node(node, v),
            NodeData::TypePredicateNode(_) => visit_each_child_type_predicate_node(node, v),
            NodeData::ImportAttribute(_) => visit_each_child_import_attribute(node, v),
            NodeData::ImportAttributes(_) => visit_each_child_import_attributes(node, v),
            NodeData::TypeQueryNode(_) => visit_each_child_type_query_node(node, v),
            NodeData::MappedTypeNode(_) => visit_each_child_mapped_type_node(node, v),
            NodeData::TypeLiteralNode(_) => visit_each_child_type_literal_node(node, v),
            NodeData::TupleTypeNode(_) => visit_each_child_tuple_type_node(node, v),
            NodeData::NamedTupleMember(_) => visit_each_child_named_tuple_member(node, v),
            NodeData::OptionalTypeNode(_) => visit_each_child_optional_type_node(node, v),
            NodeData::RestTypeNode(_) => visit_each_child_rest_type_node(node, v),
            NodeData::ParenthesizedTypeNode(_) => visit_each_child_parenthesized_type_node(node, v),
            NodeData::FunctionTypeNode(_) => visit_each_child_function_type_node(node, v),
            NodeData::ConstructorTypeNode(_) => visit_each_child_constructor_type_node(node, v),
            NodeData::TemplateLiteralTypeNode(_) => {
                visit_each_child_template_literal_type_node(node, v)
            }
            NodeData::TemplateLiteralTypeSpan(_) => {
                visit_each_child_template_literal_type_span(node, v)
            }
            NodeData::SyntheticExpression(_) => visit_each_child_synthetic_expression(node, v),
            NodeData::PartiallyEmittedExpression(_) => {
                visit_each_child_partially_emitted_expression(node, v)
            }
            NodeData::JsxElement(_) => visit_each_child_jsx_element(node, v),
            NodeData::JsxAttributes(_) => visit_each_child_jsx_attributes(node, v),
            NodeData::JsxNamespacedName(_) => visit_each_child_jsx_namespaced_name(node, v),
            NodeData::JsxOpeningElement(_) => visit_each_child_jsx_opening_element(node, v),
            NodeData::JsxSelfClosingElement(_) => {
                visit_each_child_jsx_self_closing_element(node, v)
            }
            NodeData::JsxFragment(_) => visit_each_child_jsx_fragment(node, v),
            NodeData::JsxAttribute(_) => visit_each_child_jsx_attribute(node, v),
            NodeData::JsxSpreadAttribute(_) => visit_each_child_jsx_spread_attribute(node, v),
            NodeData::JsxClosingElement(_) => visit_each_child_jsx_closing_element(node, v),
            NodeData::JsxExpression(_) => visit_each_child_jsx_expression(node, v),
            NodeData::SyntaxList(_) => visit_each_child_syntax_list(node, v),
            NodeData::JsDoc(_) => visit_each_child_js_doc(node, v),
            NodeData::JsDocTypeExpression(_) => visit_each_child_js_doc_type_expression(node, v),
            NodeData::JsDocNonNullableType(_) => visit_each_child_js_doc_non_nullable_type(node, v),
            NodeData::JsDocNullableType(_) => visit_each_child_js_doc_nullable_type(node, v),
            NodeData::JsDocVariadicType(_) => visit_each_child_js_doc_variadic_type(node, v),
            NodeData::JsDocOptionalType(_) => visit_each_child_js_doc_optional_type(node, v),
            NodeData::JsDocTypeTag(_) => visit_each_child_js_doc_type_tag(node, v),
            NodeData::JsDocUnknownTag(_) => visit_each_child_js_doc_unknown_tag(node, v),
            NodeData::JsDocTemplateTag(_) => visit_each_child_js_doc_template_tag(node, v),
            NodeData::JsDocReturnTag(_) => visit_each_child_js_doc_return_tag(node, v),
            NodeData::JsDocPublicTag(_) => visit_each_child_js_doc_public_tag(node, v),
            NodeData::JsDocPrivateTag(_) => visit_each_child_js_doc_private_tag(node, v),
            NodeData::JsDocProtectedTag(_) => visit_each_child_js_doc_protected_tag(node, v),
            NodeData::JsDocReadonlyTag(_) => visit_each_child_js_doc_readonly_tag(node, v),
            NodeData::JsDocOverrideTag(_) => visit_each_child_js_doc_override_tag(node, v),
            NodeData::JsDocDeprecatedTag(_) => visit_each_child_js_doc_deprecated_tag(node, v),
            NodeData::JsDocSeeTag(_) => visit_each_child_js_doc_see_tag(node, v),
            NodeData::JsDocImplementsTag(_) => visit_each_child_js_doc_implements_tag(node, v),
            NodeData::JsDocAugmentsTag(_) => visit_each_child_js_doc_augments_tag(node, v),
            NodeData::JsDocSatisfiesTag(_) => visit_each_child_js_doc_satisfies_tag(node, v),
            NodeData::JsDocThrowsTag(_) => visit_each_child_js_doc_throws_tag(node, v),
            NodeData::JsDocThisTag(_) => visit_each_child_js_doc_this_tag(node, v),
            NodeData::JsDocImportTag(_) => visit_each_child_js_doc_import_tag(node, v),
            NodeData::JsDocCallbackTag(_) => visit_each_child_js_doc_callback_tag(node, v),
            NodeData::JsDocOverloadTag(_) => visit_each_child_js_doc_overload_tag(node, v),
            NodeData::JsDocTypedefTag(_) => visit_each_child_js_doc_typedef_tag(node, v),
            NodeData::JsDocSignature(_) => visit_each_child_js_doc_signature(node, v),
            NodeData::JsDocNameReference(_) => visit_each_child_js_doc_name_reference(node, v),
            NodeData::ModuleDeclaration(_) => visit_each_child_module_declaration(node, v),
            NodeData::ImportEqualsDeclaration(_) => {
                visit_each_child_import_equals_declaration(node, v)
            }
            NodeData::ExportDeclaration(_) => visit_each_child_export_declaration(node, v),
            NodeData::ImportTypeNode(_) => visit_each_child_import_type_node(node, v),
            NodeData::ImportClause(_) => visit_each_child_import_clause(node, v),
            NodeData::ImportSpecifier(_) => visit_each_child_import_specifier(node, v),
            NodeData::JsDocLink(_) => visit_each_child_js_doc_link(node, v),
            NodeData::JsDocLinkPlain(_) => visit_each_child_js_doc_link_plain(node, v),
            NodeData::JsDocLinkCode(_) => visit_each_child_js_doc_link_code(node, v),
            NodeData::TypeParameterDeclaration(_) => {
                visit_each_child_type_parameter_declaration(node, v)
            }
            NodeData::SyntheticReferenceExpression(_) => {
                visit_each_child_synthetic_reference_expression(node, v)
            }
            NodeData::JsDocTypeLiteral(_) => visit_each_child_js_doc_type_literal(node, v),
            NodeData::JsDocParameterOrPropertyTag(_) => {
                visit_each_child_js_doc_parameter_or_property_tag(node, v)
            }
            // Go: ast/ast.go:1216 (node *NodeDefault) VisitEachChild (at fed0bf24149f; ts#64773 makes it the default case of (*Node).VisitEachChild, ast/ast_generated.go:9450)
            _ => node,
        })
    }
}

// Go: ast/ast.go:2711 (node *SourceFile) VisitEachChild
fn visit_each_child_source_file<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let statements = v.visit_top_level_statements(node.statement_list());
    let end_of_file_token = v.visit_token(node.end_of_file_token());
    v.factory()
        .update_source_file(node, statements, end_of_file_token)
}

// Go: ast/ast.go:3106 visitEachChild_JSDocParameterOrPropertyTag
fn visit_each_child_js_doc_parameter_or_property_tag_impl<V: ChildVisitor + ?Sized>(
    node: Node,
    v: &mut V,
) -> Node {
    let tag_name = v.visit_node_hooked(node.tag_name());
    let name = v.visit_node_hooked(node.name());
    let type_expression = v.visit_node_hooked(node.type_expression());
    let comment = v.visit_nodes_hooked(node.comment());
    v.factory().update_js_doc_parameter_or_property_tag(
        node,
        tag_name,
        name,
        node.is_bracketed(),
        type_expression,
        node.is_name_first(),
        comment,
    )
}

// Go: ast/ast_generated.go:742 (node *QualifiedName) VisitEachChild
fn visit_each_child_qualified_name<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let left = v.visit_node_hooked(node.left());
    let right = v.visit_node_hooked(node.right());
    v.factory().update_qualified_name(node, left, right)
}

// Go: ast/ast_generated.go:786 (node *ComputedPropertyName) VisitEachChild
fn visit_each_child_computed_property_name<V: ChildVisitor + ?Sized>(
    node: Node,
    v: &mut V,
) -> Node {
    let expression = v.visit_node_hooked(node.expression());
    v.factory().update_computed_property_name(node, expression)
}

// Go: ast/ast_generated.go:829 (node *Decorator) VisitEachChild
fn visit_each_child_decorator<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let expression = v.visit_node_hooked(node.expression());
    v.factory().update_decorator(node, expression)
}

// Go: ast/ast_generated.go:893 (node *IfStatement) VisitEachChild
fn visit_each_child_if_statement<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let expression = v.visit_node_hooked(node.expression());
    let then_statement = v.visit_embedded_statement_hooked(node.then_statement());
    let else_statement = v.visit_embedded_statement_hooked(node.else_statement());
    v.factory()
        .update_if_statement(node, expression, then_statement, else_statement)
}

// Go: ast/ast_generated.go:939 (node *DoStatement) VisitEachChild
fn visit_each_child_do_statement<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let statement = v.visit_iteration_body(node.statement());
    let expression = v.visit_node_hooked(node.expression());
    v.factory().update_do_statement(node, statement, expression)
}

// Go: ast/ast_generated.go:984 (node *WhileStatement) VisitEachChild
fn visit_each_child_while_statement<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let expression = v.visit_node_hooked(node.expression());
    let statement = v.visit_iteration_body(node.statement());
    v.factory()
        .update_while_statement(node, expression, statement)
}

// Go: ast/ast_generated.go:1037 (node *ForStatement) VisitEachChild
fn visit_each_child_for_statement<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let initializer = v.visit_node_hooked(node.initializer());
    let condition = v.visit_node_hooked(node.condition());
    let incrementor = v.visit_node_hooked(node.incrementor());
    let statement = v.visit_iteration_body(node.statement());
    v.factory()
        .update_for_statement(node, initializer, condition, incrementor, statement)
}

// Go: ast/ast_generated.go:1098 (node *ForInOrOfStatement) VisitEachChild
fn visit_each_child_for_in_or_of_statement<V: ChildVisitor + ?Sized>(
    node: Node,
    v: &mut V,
) -> Node {
    let await_modifier = v.visit_node_hooked(node.await_modifier());
    let initializer = v.visit_node_hooked(node.initializer());
    let expression = v.visit_node_hooked(node.expression());
    let statement = v.visit_iteration_body(node.statement());
    v.factory().update_for_in_or_of_statement(
        node,
        await_modifier,
        initializer,
        expression,
        statement,
    )
}

// Go: ast/ast_generated.go:1140 (node *BreakStatement) VisitEachChild
fn visit_each_child_break_statement<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let label = v.visit_node_hooked(node.label());
    v.factory().update_break_statement(node, label)
}

// Go: ast/ast_generated.go:1178 (node *ContinueStatement) VisitEachChild
fn visit_each_child_continue_statement<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let label = v.visit_node_hooked(node.label());
    v.factory().update_continue_statement(node, label)
}

// Go: ast/ast_generated.go:1217 (node *ReturnStatement) VisitEachChild
fn visit_each_child_return_statement<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let expression = v.visit_node_hooked(node.expression());
    v.factory().update_return_statement(node, expression)
}

// Go: ast/ast_generated.go:1258 (node *WithStatement) VisitEachChild
fn visit_each_child_with_statement<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let expression = v.visit_node_hooked(node.expression());
    let statement = v.visit_embedded_statement_hooked(node.statement());
    v.factory()
        .update_with_statement(node, expression, statement)
}

// Go: ast/ast_generated.go:1304 (node *SwitchStatement) VisitEachChild
fn visit_each_child_switch_statement<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let expression = v.visit_node_hooked(node.expression());
    let case_block = v.visit_node_hooked(node.case_block());
    v.factory()
        .update_switch_statement(node, expression, case_block)
}

// Go: ast/ast_generated.go:1349 (node *CaseBlock) VisitEachChild
fn visit_each_child_case_block<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let clauses = v.visit_nodes_hooked(node.clauses());
    v.factory().update_case_block(node, clauses)
}

// Go: ast/ast_generated.go:1400 (node *CaseOrDefaultClause) VisitEachChild
fn visit_each_child_case_or_default_clause<V: ChildVisitor + ?Sized>(
    node: Node,
    v: &mut V,
) -> Node {
    let expression = v.visit_node_hooked(node.expression());
    let statements = v.visit_nodes_hooked(node.statement_list());
    v.factory()
        .update_case_or_default_clause(node, expression, statements)
}

// Go: ast/ast_generated.go:1448 (node *ThrowStatement) VisitEachChild
fn visit_each_child_throw_statement<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let expression = v.visit_node_hooked(node.expression());
    v.factory().update_throw_statement(node, expression)
}

// Go: ast/ast_generated.go:1495 (node *TryStatement) VisitEachChild
fn visit_each_child_try_statement<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let try_block = v.visit_node_hooked(node.try_block());
    let catch_clause = v.visit_node_hooked(node.catch_clause());
    let finally_block = v.visit_node_hooked(node.finally_block());
    v.factory()
        .update_try_statement(node, try_block, catch_clause, finally_block)
}

// Go: ast/ast_generated.go:1543 (node *CatchClause) VisitEachChild
fn visit_each_child_catch_clause<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let variable_declaration = v.visit_node_hooked(node.variable_declaration());
    let block = v.visit_node_hooked(node.block());
    v.factory()
        .update_catch_clause(node, variable_declaration, block)
}

// Go: ast/ast_generated.go:1604 (node *LabeledStatement) VisitEachChild
fn visit_each_child_labeled_statement<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let label = v.visit_node_hooked(node.label());
    let statement = v.visit_embedded_statement_hooked(node.statement());
    v.factory().update_labeled_statement(node, label, statement)
}

// Go: ast/ast_generated.go:1647 (node *ExpressionStatement) VisitEachChild
fn visit_each_child_expression_statement<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let expression = v.visit_node_hooked(node.expression());
    v.factory().update_expression_statement(node, expression)
}

// Go: ast/ast_generated.go:1693 (node *Block) VisitEachChild
fn visit_each_child_block<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let statements = v.visit_nodes_hooked(node.statement_list());
    v.factory()
        .update_block(node, statements, node.multi_line())
}

// Go: ast/ast_generated.go:1738 (node *VariableStatement) VisitEachChild
fn visit_each_child_variable_statement<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let modifiers = v.visit_modifiers_hooked(node.modifiers());
    let declaration_list = v.visit_node_hooked(node.declaration_list());
    v.factory()
        .update_variable_statement(node, modifiers, declaration_list)
}

// Go: ast/ast_generated.go:1788 (node *VariableDeclaration) VisitEachChild
fn visit_each_child_variable_declaration<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let name = v.visit_node_hooked(node.name());
    let exclamation_token = v.visit_node_hooked(node.exclamation_token());
    let type_node = v.visit_node_hooked(node.type_());
    let initializer = v.visit_node_hooked(node.initializer());
    v.factory()
        .update_variable_declaration(node, name, exclamation_token, type_node, initializer)
}

// Go: ast/ast_generated.go:1833 (node *VariableDeclarationList) VisitEachChild
fn visit_each_child_variable_declaration_list<V: ChildVisitor + ?Sized>(
    node: Node,
    v: &mut V,
) -> Node {
    let declarations = v.visit_nodes_hooked(node.declarations());
    v.factory()
        .update_variable_declaration_list(node, declarations, node.flags())
}

// Go: ast/ast_generated.go:1877 (node *BindingPattern) VisitEachChild
fn visit_each_child_binding_pattern<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let elements = v.visit_nodes_hooked(node.element_list());
    v.factory().update_binding_pattern(node, elements)
}

// Go: ast/ast_generated.go:1936 (node *ParameterDeclaration) VisitEachChild
fn visit_each_child_parameter_declaration<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let modifiers = v.visit_modifiers_hooked(node.modifiers());
    let dot_dot_dot_token = v.visit_node_hooked(node.dot_dot_dot_token());
    let name = v.visit_node_hooked(node.name());
    let question_token = v.visit_node_hooked(node.question_token());
    let type_node = v.visit_node_hooked(node.type_());
    let initializer = v.visit_node_hooked(node.initializer());
    v.factory().update_parameter_declaration(
        node,
        modifiers,
        dot_dot_dot_token,
        name,
        question_token,
        type_node,
        initializer,
    )
}

// Go: ast/ast_generated.go:1991 (node *BindingElement) VisitEachChild
fn visit_each_child_binding_element<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let dot_dot_dot_token = v.visit_node_hooked(node.dot_dot_dot_token());
    let property_name = v.visit_node_hooked(node.property_name());
    let name = v.visit_node_hooked(node.name());
    let initializer = v.visit_node_hooked(node.initializer());
    v.factory()
        .update_binding_element(node, dot_dot_dot_token, property_name, name, initializer)
}

// Go: ast/ast_generated.go:2034 (node *MissingDeclaration) VisitEachChild
fn visit_each_child_missing_declaration<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let modifiers = v.visit_modifiers_hooked(node.modifiers());
    v.factory().update_missing_declaration(node, modifiers)
}

// Go: ast/ast_generated.go:2092 (node *FunctionDeclaration) VisitEachChild
fn visit_each_child_function_declaration<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let modifiers = v.visit_modifiers_hooked(node.modifiers());
    let asterisk_token = v.visit_node_hooked(node.asterisk_token());
    let name = v.visit_node_hooked(node.name());
    let type_parameters = v.visit_nodes_hooked(node.type_parameter_list());
    let parameters = v.visit_parameters(node.parameter_list());
    let type_node = v.visit_node_hooked(node.type_());
    let full_signature = v.visit_node_hooked(node.full_signature());
    let body = v.visit_function_body(node.body());
    v.factory().update_function_declaration(
        node,
        modifiers,
        asterisk_token,
        name,
        type_parameters,
        parameters,
        type_node,
        full_signature,
        body,
    )
}

// Go: ast/ast_generated.go:2143 (node *ClassDeclaration) VisitEachChild
fn visit_each_child_class_declaration<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let modifiers = v.visit_modifiers_hooked(node.modifiers());
    let name = v.visit_node_hooked(node.name());
    let type_parameters = v.visit_nodes_hooked(node.type_parameter_list());
    let heritage_clauses = v.visit_nodes_hooked(node.heritage_clauses());
    let members = v.visit_nodes_hooked(node.member_list());
    v.factory().update_class_declaration(
        node,
        modifiers,
        name,
        type_parameters,
        heritage_clauses,
        members,
    )
}

// Go: ast/ast_generated.go:2194 (node *ClassExpression) VisitEachChild
fn visit_each_child_class_expression<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let modifiers = v.visit_modifiers_hooked(node.modifiers());
    let name = v.visit_node_hooked(node.name());
    let type_parameters = v.visit_nodes_hooked(node.type_parameter_list());
    let heritage_clauses = v.visit_nodes_hooked(node.heritage_clauses());
    let members = v.visit_nodes_hooked(node.member_list());
    v.factory().update_class_expression(
        node,
        modifiers,
        name,
        type_parameters,
        heritage_clauses,
        members,
    )
}

// Go: ast/ast_generated.go:2239 (node *HeritageClause) VisitEachChild
fn visit_each_child_heritage_clause<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let types = v.visit_nodes_hooked(node.types());
    v.factory()
        .update_heritage_clause(node, node.token(), types)
}

// Go: ast/ast_generated.go:2292 (node *InterfaceDeclaration) VisitEachChild
fn visit_each_child_interface_declaration<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let modifiers = v.visit_modifiers_hooked(node.modifiers());
    let name = v.visit_node_hooked(node.name());
    let type_parameters = v.visit_nodes_hooked(node.type_parameter_list());
    let heritage_clauses = v.visit_nodes_hooked(node.heritage_clauses());
    let members = v.visit_nodes_hooked(node.member_list());
    v.factory().update_interface_declaration(
        node,
        modifiers,
        name,
        type_parameters,
        heritage_clauses,
        members,
    )
}

// Go: ast/ast_generated.go:2363 (node *TypeAliasDeclaration) VisitEachChild
fn visit_each_child_type_alias_declaration<V: ChildVisitor + ?Sized>(
    node: Node,
    v: &mut V,
) -> Node {
    let modifiers = v.visit_modifiers_hooked(node.modifiers());
    let name = v.visit_node_hooked(node.name());
    let type_parameters = v.visit_nodes_hooked(node.type_parameter_list());
    let type_node = v.visit_node_hooked(node.type_());
    v.factory()
        .update_type_alias_declaration(node, modifiers, name, type_parameters, type_node)
}

// Go: ast/ast_generated.go:2420 (node *EnumMember) VisitEachChild
fn visit_each_child_enum_member<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let name = v.visit_node_hooked(node.name());
    let initializer = v.visit_node_hooked(node.initializer());
    v.factory().update_enum_member(node, name, initializer)
}

// Go: ast/ast_generated.go:2469 (node *EnumDeclaration) VisitEachChild
fn visit_each_child_enum_declaration<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let modifiers = v.visit_modifiers_hooked(node.modifiers());
    let name = v.visit_node_hooked(node.name());
    let members = v.visit_nodes_hooked(node.member_list());
    v.factory()
        .update_enum_declaration(node, modifiers, name, members)
}

// Go: ast/ast_generated.go:2512 (node *ModuleBlock) VisitEachChild
fn visit_each_child_module_block<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let statements = v.visit_nodes_hooked(node.statement_list());
    v.factory().update_module_block(node, statements)
}

// Go: ast/ast_generated.go:2625 (node *ImportDeclaration) VisitEachChild
fn visit_each_child_import_declaration<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let modifiers = v.visit_modifiers_hooked(node.modifiers());
    let import_clause = v.visit_node_hooked(node.import_clause());
    let module_specifier = v.visit_node_hooked(node.module_specifier());
    let attributes = v.visit_node_hooked(node.attributes());
    v.factory().update_import_declaration(
        node,
        modifiers,
        import_clause,
        module_specifier,
        attributes,
    )
}

// Go: ast/ast_generated.go:2681 (node *ExternalModuleReference) VisitEachChild
fn visit_each_child_external_module_reference<V: ChildVisitor + ?Sized>(
    node: Node,
    v: &mut V,
) -> Node {
    let expression = v.visit_node_hooked(node.expression());
    v.factory()
        .update_external_module_reference(node, expression)
}

// Go: ast/ast_generated.go:2725 (node *NamespaceImport) VisitEachChild
fn visit_each_child_namespace_import<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let name = v.visit_node_hooked(node.name());
    v.factory().update_namespace_import(node, name)
}

// Go: ast/ast_generated.go:2772 (node *NamedImports) VisitEachChild
fn visit_each_child_named_imports<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let elements = v.visit_nodes_hooked(node.element_list());
    v.factory().update_named_imports(node, elements)
}

// Go: ast/ast_generated.go:2822 (node *ExportAssignment) VisitEachChild
fn visit_each_child_export_assignment<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let modifiers = v.visit_modifiers_hooked(node.modifiers());
    let type_node = v.visit_node_hooked(node.type_());
    let expression = v.visit_node_hooked(node.expression());
    v.factory().update_export_assignment(
        node,
        modifiers,
        node.is_export_equals(),
        type_node,
        expression,
    )
}

// Go: ast/ast_generated.go:2864 (node *NamespaceExportDeclaration) VisitEachChild
fn visit_each_child_namespace_export_declaration<V: ChildVisitor + ?Sized>(
    node: Node,
    v: &mut V,
) -> Node {
    let modifiers = v.visit_modifiers_hooked(node.modifiers());
    let name = v.visit_node_hooked(node.name());
    v.factory()
        .update_namespace_export_declaration(node, modifiers, name)
}

// Go: ast/ast_generated.go:2907 (node *NamespaceExport) VisitEachChild
fn visit_each_child_namespace_export<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let name = v.visit_node_hooked(node.name());
    v.factory().update_namespace_export(node, name)
}

// Go: ast/ast_generated.go:2954 (node *NamedExports) VisitEachChild
fn visit_each_child_named_exports<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let elements = v.visit_nodes_hooked(node.element_list());
    v.factory().update_named_exports(node, elements)
}

// Go: ast/ast_generated.go:3003 (node *ExportSpecifier) VisitEachChild
fn visit_each_child_export_specifier<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let property_name = v.visit_node_hooked(node.property_name());
    let name = v.visit_node_hooked(node.name());
    v.factory()
        .update_export_specifier(node, node.is_type_only(), property_name, name)
}

// Go: ast/ast_generated.go:3050 (node *CallSignatureDeclaration) VisitEachChild
fn visit_each_child_call_signature_declaration<V: ChildVisitor + ?Sized>(
    node: Node,
    v: &mut V,
) -> Node {
    let type_parameters = v.visit_nodes_hooked(node.type_parameter_list());
    let parameters = v.visit_nodes_hooked(node.parameter_list());
    let type_node = v.visit_node_hooked(node.type_());
    v.factory()
        .update_call_signature_declaration(node, type_parameters, parameters, type_node)
}

// Go: ast/ast_generated.go:3093 (node *ConstructSignatureDeclaration) VisitEachChild
fn visit_each_child_construct_signature_declaration<V: ChildVisitor + ?Sized>(
    node: Node,
    v: &mut V,
) -> Node {
    let type_parameters = v.visit_nodes_hooked(node.type_parameter_list());
    let parameters = v.visit_nodes_hooked(node.parameter_list());
    let type_node = v.visit_node_hooked(node.type_());
    v.factory()
        .update_construct_signature_declaration(node, type_parameters, parameters, type_node)
}

// Go: ast/ast_generated.go:3146 (node *ConstructorDeclaration) VisitEachChild
fn visit_each_child_constructor_declaration<V: ChildVisitor + ?Sized>(
    node: Node,
    v: &mut V,
) -> Node {
    let modifiers = v.visit_modifiers_hooked(node.modifiers());
    let type_parameters = v.visit_nodes_hooked(node.type_parameter_list());
    let parameters = v.visit_parameters(node.parameter_list());
    let type_node = v.visit_node_hooked(node.type_());
    let full_signature = v.visit_node_hooked(node.full_signature());
    let body = v.visit_function_body(node.body());
    v.factory().update_constructor_declaration(
        node,
        modifiers,
        type_parameters,
        parameters,
        type_node,
        full_signature,
        body,
    )
}

// Go: ast/ast_generated.go:3195 (node *GetAccessorDeclaration) VisitEachChild
fn visit_each_child_get_accessor_declaration<V: ChildVisitor + ?Sized>(
    node: Node,
    v: &mut V,
) -> Node {
    let modifiers = v.visit_modifiers_hooked(node.modifiers());
    let name = v.visit_node_hooked(node.name());
    let type_parameters = v.visit_nodes_hooked(node.type_parameter_list());
    let parameters = v.visit_parameters(node.parameter_list());
    let type_node = v.visit_node_hooked(node.type_());
    let full_signature = v.visit_node_hooked(node.full_signature());
    let body = v.visit_function_body(node.body());
    v.factory().update_get_accessor_declaration(
        node,
        modifiers,
        name,
        type_parameters,
        parameters,
        type_node,
        full_signature,
        body,
    )
}

// Go: ast/ast_generated.go:3248 (node *SetAccessorDeclaration) VisitEachChild
fn visit_each_child_set_accessor_declaration<V: ChildVisitor + ?Sized>(
    node: Node,
    v: &mut V,
) -> Node {
    let modifiers = v.visit_modifiers_hooked(node.modifiers());
    let name = v.visit_node_hooked(node.name());
    let type_parameters = v.visit_nodes_hooked(node.type_parameter_list());
    let parameters = v.visit_parameters(node.parameter_list());
    let type_node = v.visit_node_hooked(node.type_());
    let full_signature = v.visit_node_hooked(node.full_signature());
    let body = v.visit_function_body(node.body());
    v.factory().update_set_accessor_declaration(
        node,
        modifiers,
        name,
        type_parameters,
        parameters,
        type_node,
        full_signature,
        body,
    )
}

// Go: ast/ast_generated.go:3297 (node *IndexSignatureDeclaration) VisitEachChild
fn visit_each_child_index_signature_declaration<V: ChildVisitor + ?Sized>(
    node: Node,
    v: &mut V,
) -> Node {
    let modifiers = v.visit_modifiers_hooked(node.modifiers());
    let parameters = v.visit_nodes_hooked(node.parameter_list());
    let type_node = v.visit_node_hooked(node.type_());
    v.factory()
        .update_index_signature_declaration(node, modifiers, parameters, type_node)
}

// Go: ast/ast_generated.go:3349 (node *MethodSignatureDeclaration) VisitEachChild
fn visit_each_child_method_signature_declaration<V: ChildVisitor + ?Sized>(
    node: Node,
    v: &mut V,
) -> Node {
    let modifiers = v.visit_modifiers_hooked(node.modifiers());
    let name = v.visit_node_hooked(node.name());
    let postfix_token = v.visit_node_hooked(node.postfix_token());
    let type_parameters = v.visit_nodes_hooked(node.type_parameter_list());
    let parameters = v.visit_nodes_hooked(node.parameter_list());
    let type_node = v.visit_node_hooked(node.type_());
    v.factory().update_method_signature_declaration(
        node,
        modifiers,
        name,
        postfix_token,
        type_parameters,
        parameters,
        type_node,
    )
}

// Go: ast/ast_generated.go:3413 (node *MethodDeclaration) VisitEachChild
fn visit_each_child_method_declaration<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let modifiers = v.visit_modifiers_hooked(node.modifiers());
    let asterisk_token = v.visit_node_hooked(node.asterisk_token());
    let name = v.visit_node_hooked(node.name());
    let postfix_token = v.visit_node_hooked(node.postfix_token());
    let type_parameters = v.visit_nodes_hooked(node.type_parameter_list());
    let parameters = v.visit_parameters(node.parameter_list());
    let type_node = v.visit_node_hooked(node.type_());
    let full_signature = v.visit_node_hooked(node.full_signature());
    let body = v.visit_function_body(node.body());
    v.factory().update_method_declaration(
        node,
        modifiers,
        asterisk_token,
        name,
        postfix_token,
        type_parameters,
        parameters,
        type_node,
        full_signature,
        body,
    )
}

// Go: ast/ast_generated.go:3468 (node *PropertySignatureDeclaration) VisitEachChild
fn visit_each_child_property_signature_declaration<V: ChildVisitor + ?Sized>(
    node: Node,
    v: &mut V,
) -> Node {
    let modifiers = v.visit_modifiers_hooked(node.modifiers());
    let name = v.visit_node_hooked(node.name());
    let postfix_token = v.visit_node_hooked(node.postfix_token());
    let type_node = v.visit_node_hooked(node.type_());
    let initializer = v.visit_node_hooked(node.initializer());
    v.factory().update_property_signature_declaration(
        node,
        modifiers,
        name,
        postfix_token,
        type_node,
        initializer,
    )
}

// Go: ast/ast_generated.go:3523 (node *PropertyDeclaration) VisitEachChild
fn visit_each_child_property_declaration<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let modifiers = v.visit_modifiers_hooked(node.modifiers());
    let name = v.visit_node_hooked(node.name());
    let postfix_token = v.visit_node_hooked(node.postfix_token());
    let type_node = v.visit_node_hooked(node.type_());
    let initializer = v.visit_node_hooked(node.initializer());
    v.factory().update_property_declaration(
        node,
        modifiers,
        name,
        postfix_token,
        type_node,
        initializer,
    )
}

// Go: ast/ast_generated.go:3595 (node *ClassStaticBlockDeclaration) VisitEachChild
fn visit_each_child_class_static_block_declaration<V: ChildVisitor + ?Sized>(
    node: Node,
    v: &mut V,
) -> Node {
    let modifiers = v.visit_modifiers_hooked(node.modifiers());
    let body = v.visit_node_hooked(node.body());
    v.factory()
        .update_class_static_block_declaration(node, modifiers, body)
}

// Go: ast/ast_generated.go:3821 (node *BinaryExpression) VisitEachChild
fn visit_each_child_binary_expression<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let modifiers = v.visit_modifiers_hooked(node.modifiers());
    let left = v.visit_node_hooked(node.left());
    let type_node = v.visit_node_hooked(node.type_());
    let operator_token = v.visit_node_hooked(node.operator_token());
    let right = v.visit_node_hooked(node.right());
    v.factory()
        .update_binary_expression(node, modifiers, left, type_node, operator_token, right)
}

// Go: ast/ast_generated.go:3861 (node *PrefixUnaryExpression) VisitEachChild
fn visit_each_child_prefix_unary_expression<V: ChildVisitor + ?Sized>(
    node: Node,
    v: &mut V,
) -> Node {
    let operand = v.visit_node_hooked(node.operand());
    v.factory()
        .update_prefix_unary_expression(node, node.operator(), operand)
}

// Go: ast/ast_generated.go:3905 (node *PostfixUnaryExpression) VisitEachChild
fn visit_each_child_postfix_unary_expression<V: ChildVisitor + ?Sized>(
    node: Node,
    v: &mut V,
) -> Node {
    let operand = v.visit_node_hooked(node.operand());
    v.factory()
        .update_postfix_unary_expression(node, operand, node.operator())
}

// Go: ast/ast_generated.go:3949 (node *YieldExpression) VisitEachChild
fn visit_each_child_yield_expression<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let asterisk_token = v.visit_node_hooked(node.asterisk_token());
    let expression = v.visit_node_hooked(node.expression());
    v.factory()
        .update_yield_expression(node, asterisk_token, expression)
}

// Go: ast/ast_generated.go:4004 (node *ArrowFunction) VisitEachChild
fn visit_each_child_arrow_function<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let modifiers = v.visit_modifiers_hooked(node.modifiers());
    let type_parameters = v.visit_nodes_hooked(node.type_parameter_list());
    let parameters = v.visit_parameters(node.parameter_list());
    let type_node = v.visit_node_hooked(node.type_());
    let full_signature = v.visit_node_hooked(node.full_signature());
    let equals_greater_than_token = v.visit_node_hooked(node.equals_greater_than_token());
    let body = v.visit_function_body(node.body());
    v.factory().update_arrow_function(
        node,
        modifiers,
        type_parameters,
        parameters,
        type_node,
        full_signature,
        equals_greater_than_token,
        body,
    )
}

// Go: ast/ast_generated.go:4062 (node *FunctionExpression) VisitEachChild
fn visit_each_child_function_expression<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let modifiers = v.visit_modifiers_hooked(node.modifiers());
    let asterisk_token = v.visit_node_hooked(node.asterisk_token());
    let name = v.visit_node_hooked(node.name());
    let type_parameters = v.visit_nodes_hooked(node.type_parameter_list());
    let parameters = v.visit_parameters(node.parameter_list());
    let type_node = v.visit_node_hooked(node.type_());
    let full_signature = v.visit_node_hooked(node.full_signature());
    let body = v.visit_function_body(node.body());
    v.factory().update_function_expression(
        node,
        modifiers,
        asterisk_token,
        name,
        type_parameters,
        parameters,
        type_node,
        full_signature,
        body,
    )
}

// Go: ast/ast_generated.go:4106 (node *AsExpression) VisitEachChild
fn visit_each_child_as_expression<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let expression = v.visit_node_hooked(node.expression());
    let type_node = v.visit_node_hooked(node.type_());
    v.factory()
        .update_as_expression(node, expression, type_node)
}

// Go: ast/ast_generated.go:4146 (node *SatisfiesExpression) VisitEachChild
fn visit_each_child_satisfies_expression<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let expression = v.visit_node_hooked(node.expression());
    let type_node = v.visit_node_hooked(node.type_());
    v.factory()
        .update_satisfies_expression(node, expression, type_node)
}

// Go: ast/ast_generated.go:4197 (node *ConditionalExpression) VisitEachChild
fn visit_each_child_conditional_expression<V: ChildVisitor + ?Sized>(
    node: Node,
    v: &mut V,
) -> Node {
    let condition = v.visit_node_hooked(node.condition());
    let question_token = v.visit_node_hooked(node.question_token());
    let when_true = v.visit_node_hooked(node.when_true());
    let colon_token = v.visit_node_hooked(node.colon_token());
    let when_false = v.visit_node_hooked(node.when_false());
    v.factory().update_conditional_expression(
        node,
        condition,
        question_token,
        when_true,
        colon_token,
        when_false,
    )
}

// Go: ast/ast_generated.go:4251 (node *PropertyAccessExpression) VisitEachChild
fn visit_each_child_property_access_expression<V: ChildVisitor + ?Sized>(
    node: Node,
    v: &mut V,
) -> Node {
    let expression = v.visit_node_hooked(node.expression());
    let question_dot_token = v.visit_node_hooked(node.question_dot_token());
    let name = v.visit_node_hooked(node.name());
    v.factory().update_property_access_expression(
        node,
        expression,
        question_dot_token,
        name,
        node.flags(),
    )
}

// Go: ast/ast_generated.go:4301 (node *ElementAccessExpression) VisitEachChild
fn visit_each_child_element_access_expression<V: ChildVisitor + ?Sized>(
    node: Node,
    v: &mut V,
) -> Node {
    let expression = v.visit_node_hooked(node.expression());
    let question_dot_token = v.visit_node_hooked(node.question_dot_token());
    let argument_expression = v.visit_node_hooked(node.argument_expression());
    v.factory().update_element_access_expression(
        node,
        expression,
        question_dot_token,
        argument_expression,
        node.flags(),
    )
}

// Go: ast/ast_generated.go:4358 (node *CallExpression) VisitEachChild
fn visit_each_child_call_expression<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let expression = v.visit_node_hooked(node.expression());
    let question_dot_token = v.visit_node_hooked(node.question_dot_token());
    let type_arguments = v.visit_nodes_hooked(node.type_argument_list());
    let arguments = v.visit_nodes_hooked(node.argument_list());
    v.factory().update_call_expression(
        node,
        expression,
        question_dot_token,
        type_arguments,
        arguments,
        node.flags(),
    )
}

// Go: ast/ast_generated.go:4401 (node *NewExpression) VisitEachChild
fn visit_each_child_new_expression<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let expression = v.visit_node_hooked(node.expression());
    let type_arguments = v.visit_nodes_hooked(node.type_argument_list());
    let arguments = v.visit_nodes_hooked(node.argument_list());
    v.factory()
        .update_new_expression(node, expression, type_arguments, arguments)
}

// Go: ast/ast_generated.go:4443 (node *MetaProperty) VisitEachChild
fn visit_each_child_meta_property<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let name = v.visit_node_hooked(node.name());
    v.factory()
        .update_meta_property(node, node.keyword_token(), name)
}

// Go: ast/ast_generated.go:4487 (node *NonNullExpression) VisitEachChild
fn visit_each_child_non_null_expression<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let expression = v.visit_node_hooked(node.expression());
    v.factory()
        .update_non_null_expression(node, expression, node.flags())
}

// Go: ast/ast_generated.go:4525 (node *SpreadElement) VisitEachChild
fn visit_each_child_spread_element<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let expression = v.visit_node_hooked(node.expression());
    v.factory().update_spread_element(node, expression)
}

// Go: ast/ast_generated.go:4566 (node *TemplateExpression) VisitEachChild
fn visit_each_child_template_expression<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let head = v.visit_node_hooked(node.head());
    let template_spans = v.visit_nodes_hooked(node.template_spans());
    v.factory()
        .update_template_expression(node, head, template_spans)
}

// Go: ast/ast_generated.go:4611 (node *TemplateSpan) VisitEachChild
fn visit_each_child_template_span<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let expression = v.visit_node_hooked(node.expression());
    let literal = v.visit_node_hooked(node.literal());
    v.factory().update_template_span(node, expression, literal)
}

// Go: ast/ast_generated.go:4666 (node *TaggedTemplateExpression) VisitEachChild
fn visit_each_child_tagged_template_expression<V: ChildVisitor + ?Sized>(
    node: Node,
    v: &mut V,
) -> Node {
    let tag = v.visit_node_hooked(node.tag());
    let question_dot_token = v.visit_node_hooked(node.question_dot_token());
    let type_arguments = v.visit_nodes_hooked(node.type_argument_list());
    let template = v.visit_node_hooked(node.template());
    v.factory().update_tagged_template_expression(
        node,
        tag,
        question_dot_token,
        type_arguments,
        template,
        node.flags(),
    )
}

// Go: ast/ast_generated.go:4704 (node *ParenthesizedExpression) VisitEachChild
fn visit_each_child_parenthesized_expression<V: ChildVisitor + ?Sized>(
    node: Node,
    v: &mut V,
) -> Node {
    let expression = v.visit_node_hooked(node.expression());
    v.factory()
        .update_parenthesized_expression(node, expression)
}

// Go: ast/ast_generated.go:4749 (node *ArrayLiteralExpression) VisitEachChild
fn visit_each_child_array_literal_expression<V: ChildVisitor + ?Sized>(
    node: Node,
    v: &mut V,
) -> Node {
    let elements = v.visit_nodes_hooked(node.element_list());
    v.factory()
        .update_array_literal_expression(node, elements, node.multi_line())
}

// Go: ast/ast_generated.go:4795 (node *ObjectLiteralExpression) VisitEachChild
fn visit_each_child_object_literal_expression<V: ChildVisitor + ?Sized>(
    node: Node,
    v: &mut V,
) -> Node {
    let properties = v.visit_nodes_hooked(node.property_list());
    v.factory()
        .update_object_literal_expression(node, properties, node.multi_line())
}

// Go: ast/ast_generated.go:4839 (node *SpreadAssignment) VisitEachChild
fn visit_each_child_spread_assignment<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let expression = v.visit_node_hooked(node.expression());
    v.factory().update_spread_assignment(node, expression)
}

// Go: ast/ast_generated.go:4890 (node *PropertyAssignment) VisitEachChild
fn visit_each_child_property_assignment<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let modifiers = v.visit_modifiers_hooked(node.modifiers());
    let name = v.visit_node_hooked(node.name());
    let postfix_token = v.visit_node_hooked(node.postfix_token());
    let type_node = v.visit_node_hooked(node.type_());
    let initializer = v.visit_node_hooked(node.initializer());
    v.factory().update_property_assignment(
        node,
        modifiers,
        name,
        postfix_token,
        type_node,
        initializer,
    )
}

// Go: ast/ast_generated.go:4948 (node *ShorthandPropertyAssignment) VisitEachChild
fn visit_each_child_shorthand_property_assignment<V: ChildVisitor + ?Sized>(
    node: Node,
    v: &mut V,
) -> Node {
    let modifiers = v.visit_modifiers_hooked(node.modifiers());
    let name = v.visit_node_hooked(node.name());
    let postfix_token = v.visit_node_hooked(node.postfix_token());
    let type_node = v.visit_node_hooked(node.type_());
    let equals_token = v.visit_node_hooked(node.equals_token());
    let object_assignment_initializer = v.visit_node_hooked(node.object_assignment_initializer());
    v.factory().update_shorthand_property_assignment(
        node,
        modifiers,
        name,
        postfix_token,
        type_node,
        equals_token,
        object_assignment_initializer,
    )
}

// Go: ast/ast_generated.go:4990 (node *DeleteExpression) VisitEachChild
fn visit_each_child_delete_expression<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let expression = v.visit_node_hooked(node.expression());
    v.factory().update_delete_expression(node, expression)
}

// Go: ast/ast_generated.go:5032 (node *TypeOfExpression) VisitEachChild
fn visit_each_child_type_of_expression<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let expression = v.visit_node_hooked(node.expression());
    v.factory().update_type_of_expression(node, expression)
}

// Go: ast/ast_generated.go:5074 (node *VoidExpression) VisitEachChild
fn visit_each_child_void_expression<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let expression = v.visit_node_hooked(node.expression());
    v.factory().update_void_expression(node, expression)
}

// Go: ast/ast_generated.go:5116 (node *AwaitExpression) VisitEachChild
fn visit_each_child_await_expression<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let expression = v.visit_node_hooked(node.expression());
    v.factory().update_await_expression(node, expression)
}

// Go: ast/ast_generated.go:5156 (node *TypeAssertion) VisitEachChild
fn visit_each_child_type_assertion<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let type_node = v.visit_node_hooked(node.type_());
    let expression = v.visit_node_hooked(node.expression());
    v.factory()
        .update_type_assertion(node, type_node, expression)
}

// Go: ast/ast_generated.go:5223 (node *UnionTypeNode) VisitEachChild
fn visit_each_child_union_type_node<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let types = v.visit_nodes_hooked(node.types());
    v.factory().update_union_type_node(node, types)
}

// Go: ast/ast_generated.go:5260 (node *IntersectionTypeNode) VisitEachChild
fn visit_each_child_intersection_type_node<V: ChildVisitor + ?Sized>(
    node: Node,
    v: &mut V,
) -> Node {
    let types = v.visit_nodes_hooked(node.types());
    v.factory().update_intersection_type_node(node, types)
}

// Go: ast/ast_generated.go:5308 (node *ConditionalTypeNode) VisitEachChild
fn visit_each_child_conditional_type_node<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let check_type = v.visit_node_hooked(node.check_type());
    let extends_type = v.visit_node_hooked(node.extends_type());
    let true_type = v.visit_node_hooked(node.true_type());
    let false_type = v.visit_node_hooked(node.false_type());
    v.factory()
        .update_conditional_type_node(node, check_type, extends_type, true_type, false_type)
}

// Go: ast/ast_generated.go:5348 (node *TypeOperatorNode) VisitEachChild
fn visit_each_child_type_operator_node<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let type_node = v.visit_node_hooked(node.type_());
    v.factory()
        .update_type_operator_node(node, node.operator(), type_node)
}

// Go: ast/ast_generated.go:5386 (node *InferTypeNode) VisitEachChild
fn visit_each_child_infer_type_node<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let type_parameter = v.visit_node_hooked(node.type_parameter());
    v.factory().update_infer_type_node(node, type_parameter)
}

// Go: ast/ast_generated.go:5424 (node *ArrayTypeNode) VisitEachChild
fn visit_each_child_array_type_node<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let element_type = v.visit_node_hooked(node.element_type());
    v.factory().update_array_type_node(node, element_type)
}

// Go: ast/ast_generated.go:5464 (node *IndexedAccessTypeNode) VisitEachChild
fn visit_each_child_indexed_access_type_node<V: ChildVisitor + ?Sized>(
    node: Node,
    v: &mut V,
) -> Node {
    let object_type = v.visit_node_hooked(node.object_type());
    let index_type = v.visit_node_hooked(node.index_type());
    v.factory()
        .update_indexed_access_type_node(node, object_type, index_type)
}

// Go: ast/ast_generated.go:5503 (node *TypeReferenceNode) VisitEachChild
fn visit_each_child_type_reference_node<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let type_name = v.visit_node_hooked(node.type_name());
    let type_arguments = v.visit_nodes_hooked(node.type_argument_list());
    v.factory()
        .update_type_reference_node(node, type_name, type_arguments)
}

// Go: ast/ast_generated.go:5544 (node *ExpressionWithTypeArguments) VisitEachChild
fn visit_each_child_expression_with_type_arguments<V: ChildVisitor + ?Sized>(
    node: Node,
    v: &mut V,
) -> Node {
    let expression = v.visit_node_hooked(node.expression());
    let type_arguments = v.visit_nodes_hooked(node.type_argument_list());
    v.factory()
        .update_expression_with_type_arguments(node, expression, type_arguments)
}

// Go: ast/ast_generated.go:5582 (node *LiteralTypeNode) VisitEachChild
fn visit_each_child_literal_type_node<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let literal = v.visit_node_hooked(node.literal());
    v.factory().update_literal_type_node(node, literal)
}

// Go: ast/ast_generated.go:5645 (node *TypePredicateNode) VisitEachChild
fn visit_each_child_type_predicate_node<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let asserts_modifier = v.visit_node_hooked(node.asserts_modifier());
    let parameter_name = v.visit_node_hooked(node.parameter_name());
    let type_node = v.visit_node_hooked(node.type_());
    v.factory()
        .update_type_predicate_node(node, asserts_modifier, parameter_name, type_node)
}

// Go: ast/ast_generated.go:5686 (node *ImportAttribute) VisitEachChild
fn visit_each_child_import_attribute<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let name = v.visit_node_hooked(node.name());
    let value = v.visit_node_hooked(node.value());
    v.factory().update_import_attribute(node, name, value)
}

// Go: ast/ast_generated.go:5738 (node *ImportAttributes) VisitEachChild
fn visit_each_child_import_attributes<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let attributes = v.visit_nodes_hooked(import_attributes_list(node));
    v.factory()
        .update_import_attributes(node, node.token(), attributes, node.multi_line())
}

// Go: ast/ast_generated.go:5781 (node *TypeQueryNode) VisitEachChild
fn visit_each_child_type_query_node<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let expr_name = v.visit_node_hooked(node.expr_name());
    let type_arguments = v.visit_nodes_hooked(node.type_argument_list());
    v.factory()
        .update_type_query_node(node, expr_name, type_arguments)
}

// Go: ast/ast_generated.go:5836 (node *MappedTypeNode) VisitEachChild
fn visit_each_child_mapped_type_node<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let readonly_token = v.visit_node_hooked(node.readonly_token());
    let type_parameter = v.visit_node_hooked(node.type_parameter());
    let name_type = v.visit_node_hooked(node.name_type());
    let question_token = v.visit_node_hooked(node.question_token());
    let type_node = v.visit_node_hooked(node.type_());
    let members = v.visit_nodes_hooked(node.member_list());
    v.factory().update_mapped_type_node(
        node,
        readonly_token,
        type_parameter,
        name_type,
        question_token,
        type_node,
        members,
    )
}

// Go: ast/ast_generated.go:5875 (node *TypeLiteralNode) VisitEachChild
fn visit_each_child_type_literal_node<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let members = v.visit_nodes_hooked(node.member_list());
    v.factory().update_type_literal_node(node, members)
}

// Go: ast/ast_generated.go:5913 (node *TupleTypeNode) VisitEachChild
fn visit_each_child_tuple_type_node<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let elements = v.visit_nodes_hooked(node.element_list());
    v.factory().update_tuple_type_node(node, elements)
}

// Go: ast/ast_generated.go:5961 (node *NamedTupleMember) VisitEachChild
fn visit_each_child_named_tuple_member<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let dot_dot_dot_token = v.visit_node_hooked(node.dot_dot_dot_token());
    let name = v.visit_node_hooked(node.name());
    let question_token = v.visit_node_hooked(node.question_token());
    let type_node = v.visit_node_hooked(node.type_());
    v.factory()
        .update_named_tuple_member(node, dot_dot_dot_token, name, question_token, type_node)
}

// Go: ast/ast_generated.go:6003 (node *OptionalTypeNode) VisitEachChild
fn visit_each_child_optional_type_node<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let type_node = v.visit_node_hooked(node.type_());
    v.factory().update_optional_type_node(node, type_node)
}

// Go: ast/ast_generated.go:6041 (node *RestTypeNode) VisitEachChild
fn visit_each_child_rest_type_node<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let type_node = v.visit_node_hooked(node.type_());
    v.factory().update_rest_type_node(node, type_node)
}

// Go: ast/ast_generated.go:6079 (node *ParenthesizedTypeNode) VisitEachChild
fn visit_each_child_parenthesized_type_node<V: ChildVisitor + ?Sized>(
    node: Node,
    v: &mut V,
) -> Node {
    let type_node = v.visit_node_hooked(node.type_());
    v.factory().update_parenthesized_type_node(node, type_node)
}

// Go: ast/ast_generated.go:6118 (node *FunctionTypeNode) VisitEachChild
fn visit_each_child_function_type_node<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let type_parameters = v.visit_nodes_hooked(node.type_parameter_list());
    let parameters = v.visit_nodes_hooked(node.parameter_list());
    let type_node = v.visit_node_hooked(node.type_());
    v.factory()
        .update_function_type_node(node, type_parameters, parameters, type_node)
}

// Go: ast/ast_generated.go:6161 (node *ConstructorTypeNode) VisitEachChild
fn visit_each_child_constructor_type_node<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let modifiers = v.visit_modifiers_hooked(node.modifiers());
    let type_parameters = v.visit_nodes_hooked(node.type_parameter_list());
    let parameters = v.visit_nodes_hooked(node.parameter_list());
    let type_node = v.visit_node_hooked(node.type_());
    v.factory().update_constructor_type_node(
        node,
        modifiers,
        type_parameters,
        parameters,
        type_node,
    )
}

// Go: ast/ast_generated.go:6279 (node *TemplateLiteralTypeNode) VisitEachChild
fn visit_each_child_template_literal_type_node<V: ChildVisitor + ?Sized>(
    node: Node,
    v: &mut V,
) -> Node {
    let head = v.visit_node_hooked(node.head());
    let template_spans = v.visit_nodes_hooked(node.template_spans());
    v.factory()
        .update_template_literal_type_node(node, head, template_spans)
}

// Go: ast/ast_generated.go:6319 (node *TemplateLiteralTypeSpan) VisitEachChild
fn visit_each_child_template_literal_type_span<V: ChildVisitor + ?Sized>(
    node: Node,
    v: &mut V,
) -> Node {
    let type_node = v.visit_node_hooked(node.type_());
    let literal = v.visit_node_hooked(node.literal());
    v.factory()
        .update_template_literal_type_span(node, type_node, literal)
}

// Go: ast/ast_generated.go:6361 (node *SyntheticExpression) VisitEachChild
fn visit_each_child_synthetic_expression<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let tuple_name_source = v.visit_node_hooked(node.tuple_name_source());
    v.factory().update_synthetic_expression(
        node,
        synthetic_expression_type(node),
        node.is_spread(),
        tuple_name_source,
    )
}

// Go: ast/ast_generated.go:6399 (node *PartiallyEmittedExpression) VisitEachChild
fn visit_each_child_partially_emitted_expression<V: ChildVisitor + ?Sized>(
    node: Node,
    v: &mut V,
) -> Node {
    let expression = v.visit_node_hooked(node.expression());
    v.factory()
        .update_partially_emitted_expression(node, expression)
}

// Go: ast/ast_generated.go:6446 (node *JsxElement) VisitEachChild
fn visit_each_child_jsx_element<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let opening_element = v.visit_node_hooked(node.opening_element());
    let children = v.visit_nodes_hooked(node.children());
    let closing_element = v.visit_node_hooked(node.closing_element());
    v.factory()
        .update_jsx_element(node, opening_element, children, closing_element)
}

// Go: ast/ast_generated.go:6486 (node *JsxAttributes) VisitEachChild
fn visit_each_child_jsx_attributes<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let properties = v.visit_nodes_hooked(node.property_list());
    v.factory().update_jsx_attributes(node, properties)
}

// Go: ast/ast_generated.go:6527 (node *JsxNamespacedName) VisitEachChild
fn visit_each_child_jsx_namespaced_name<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let namespace = v.visit_node_hooked(node.namespace());
    let name = v.visit_node_hooked(node.name());
    v.factory()
        .update_jsx_namespaced_name(node, namespace, name)
}

// Go: ast/ast_generated.go:6574 (node *JsxOpeningElement) VisitEachChild
fn visit_each_child_jsx_opening_element<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let tag_name = v.visit_node_hooked(node.tag_name());
    let type_arguments = v.visit_nodes_hooked(node.type_argument_list());
    let attributes = v.visit_node_hooked(node.attributes());
    v.factory()
        .update_jsx_opening_element(node, tag_name, type_arguments, attributes)
}

// Go: ast/ast_generated.go:6617 (node *JsxSelfClosingElement) VisitEachChild
fn visit_each_child_jsx_self_closing_element<V: ChildVisitor + ?Sized>(
    node: Node,
    v: &mut V,
) -> Node {
    let tag_name = v.visit_node_hooked(node.tag_name());
    let type_arguments = v.visit_nodes_hooked(node.type_argument_list());
    let attributes = v.visit_node_hooked(node.attributes());
    v.factory()
        .update_jsx_self_closing_element(node, tag_name, type_arguments, attributes)
}

// Go: ast/ast_generated.go:6660 (node *JsxFragment) VisitEachChild
fn visit_each_child_jsx_fragment<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let opening_fragment = v.visit_node_hooked(node.opening_fragment());
    let children = v.visit_nodes_hooked(node.children());
    let closing_fragment = v.visit_node_hooked(node.closing_fragment());
    v.factory()
        .update_jsx_fragment(node, opening_fragment, children, closing_fragment)
}

// Go: ast/ast_generated.go:6744 (node *JsxAttribute) VisitEachChild
fn visit_each_child_jsx_attribute<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let name = v.visit_node_hooked(node.name());
    let initializer = v.visit_node_hooked(node.initializer());
    v.factory().update_jsx_attribute(node, name, initializer)
}

// Go: ast/ast_generated.go:6788 (node *JsxSpreadAttribute) VisitEachChild
fn visit_each_child_jsx_spread_attribute<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let expression = v.visit_node_hooked(node.expression());
    v.factory().update_jsx_spread_attribute(node, expression)
}

// Go: ast/ast_generated.go:6826 (node *JsxClosingElement) VisitEachChild
fn visit_each_child_jsx_closing_element<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let tag_name = v.visit_node_hooked(node.tag_name());
    v.factory().update_jsx_closing_element(node, tag_name)
}

// Go: ast/ast_generated.go:6866 (node *JsxExpression) VisitEachChild
fn visit_each_child_jsx_expression<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let dot_dot_dot_token = v.visit_node_hooked(node.dot_dot_dot_token());
    let expression = v.visit_node_hooked(node.expression());
    v.factory()
        .update_jsx_expression(node, dot_dot_dot_token, expression)
}

// Go: ast/ast_generated.go:6930 (node *SyntaxList) VisitEachChild
fn visit_each_child_syntax_list<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    // PORT: Go `core.SameMap` returns the original slice when no element changed;
    // the update compares elements, so a mapped copy with equal elements is the same.
    let children: Vec<Node> = syntax_list_children(node)
        .into_iter()
        .map(|n| v.visit_node_hooked(n))
        .collect();
    v.factory().update_syntax_list(node, &children)
}

// Go: ast/ast_generated.go:6971 (node *JSDoc) VisitEachChild
fn visit_each_child_js_doc<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let comment = v.visit_nodes_hooked(node.comment());
    let tags = v.visit_nodes_hooked(node.tags());
    v.factory().update_js_doc(node, comment, tags)
}

// Go: ast/ast_generated.go:7009 (node *JSDocTypeExpression) VisitEachChild
fn visit_each_child_js_doc_type_expression<V: ChildVisitor + ?Sized>(
    node: Node,
    v: &mut V,
) -> Node {
    let type_node = v.visit_node_hooked(node.type_());
    v.factory().update_js_doc_type_expression(node, type_node)
}

// Go: ast/ast_generated.go:7047 (node *JSDocNonNullableType) VisitEachChild
fn visit_each_child_js_doc_non_nullable_type<V: ChildVisitor + ?Sized>(
    node: Node,
    v: &mut V,
) -> Node {
    let type_node = v.visit_node_hooked(node.type_());
    v.factory().update_js_doc_non_nullable_type(node, type_node)
}

// Go: ast/ast_generated.go:7085 (node *JSDocNullableType) VisitEachChild
fn visit_each_child_js_doc_nullable_type<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let type_node = v.visit_node_hooked(node.type_());
    v.factory().update_js_doc_nullable_type(node, type_node)
}

// Go: ast/ast_generated.go:7144 (node *JSDocVariadicType) VisitEachChild
fn visit_each_child_js_doc_variadic_type<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let type_node = v.visit_node_hooked(node.type_());
    v.factory().update_js_doc_variadic_type(node, type_node)
}

// Go: ast/ast_generated.go:7182 (node *JSDocOptionalType) VisitEachChild
fn visit_each_child_js_doc_optional_type<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let type_node = v.visit_node_hooked(node.type_());
    v.factory().update_js_doc_optional_type(node, type_node)
}

// Go: ast/ast_generated.go:7222 (node *JSDocTypeTag) VisitEachChild
fn visit_each_child_js_doc_type_tag<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let tag_name = v.visit_node_hooked(node.tag_name());
    let type_expression = v.visit_node_hooked(node.type_expression());
    let comment = v.visit_nodes_hooked(node.comment());
    v.factory()
        .update_js_doc_type_tag(node, tag_name, type_expression, comment)
}

// Go: ast/ast_generated.go:7260 (node *JSDocUnknownTag) VisitEachChild
fn visit_each_child_js_doc_unknown_tag<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let tag_name = v.visit_node_hooked(node.tag_name());
    let comment = v.visit_nodes_hooked(node.comment());
    v.factory()
        .update_js_doc_unknown_tag(node, tag_name, comment)
}

// Go: ast/ast_generated.go:7305 (node *JSDocTemplateTag) VisitEachChild
fn visit_each_child_js_doc_template_tag<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let tag_name = v.visit_node_hooked(node.tag_name());
    let constraint = v.visit_node_hooked(node.constraint());
    let type_parameters = v.visit_nodes_hooked(node.type_parameter_list());
    let comment = v.visit_nodes_hooked(node.comment());
    v.factory()
        .update_js_doc_template_tag(node, tag_name, constraint, type_parameters, comment)
}

// Go: ast/ast_generated.go:7345 (node *JSDocReturnTag) VisitEachChild
fn visit_each_child_js_doc_return_tag<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let tag_name = v.visit_node_hooked(node.tag_name());
    let type_expression = v.visit_node_hooked(node.type_expression());
    let comment = v.visit_nodes_hooked(node.comment());
    v.factory()
        .update_js_doc_return_tag(node, tag_name, type_expression, comment)
}

// Go: ast/ast_generated.go:7383 (node *JSDocPublicTag) VisitEachChild
fn visit_each_child_js_doc_public_tag<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let tag_name = v.visit_node_hooked(node.tag_name());
    let comment = v.visit_nodes_hooked(node.comment());
    v.factory()
        .update_js_doc_public_tag(node, tag_name, comment)
}

// Go: ast/ast_generated.go:7421 (node *JSDocPrivateTag) VisitEachChild
fn visit_each_child_js_doc_private_tag<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let tag_name = v.visit_node_hooked(node.tag_name());
    let comment = v.visit_nodes_hooked(node.comment());
    v.factory()
        .update_js_doc_private_tag(node, tag_name, comment)
}

// Go: ast/ast_generated.go:7459 (node *JSDocProtectedTag) VisitEachChild
fn visit_each_child_js_doc_protected_tag<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let tag_name = v.visit_node_hooked(node.tag_name());
    let comment = v.visit_nodes_hooked(node.comment());
    v.factory()
        .update_js_doc_protected_tag(node, tag_name, comment)
}

// Go: ast/ast_generated.go:7497 (node *JSDocReadonlyTag) VisitEachChild
fn visit_each_child_js_doc_readonly_tag<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let tag_name = v.visit_node_hooked(node.tag_name());
    let comment = v.visit_nodes_hooked(node.comment());
    v.factory()
        .update_js_doc_readonly_tag(node, tag_name, comment)
}

// Go: ast/ast_generated.go:7535 (node *JSDocOverrideTag) VisitEachChild
fn visit_each_child_js_doc_override_tag<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let tag_name = v.visit_node_hooked(node.tag_name());
    let comment = v.visit_nodes_hooked(node.comment());
    v.factory()
        .update_js_doc_override_tag(node, tag_name, comment)
}

// Go: ast/ast_generated.go:7573 (node *JSDocDeprecatedTag) VisitEachChild
fn visit_each_child_js_doc_deprecated_tag<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let tag_name = v.visit_node_hooked(node.tag_name());
    let comment = v.visit_nodes_hooked(node.comment());
    v.factory()
        .update_js_doc_deprecated_tag(node, tag_name, comment)
}

// Go: ast/ast_generated.go:7613 (node *JSDocSeeTag) VisitEachChild
fn visit_each_child_js_doc_see_tag<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let tag_name = v.visit_node_hooked(node.tag_name());
    let name_expression = v.visit_node_hooked(node.name_expression());
    let comment = v.visit_nodes_hooked(node.comment());
    v.factory()
        .update_js_doc_see_tag(node, tag_name, name_expression, comment)
}

// Go: ast/ast_generated.go:7653 (node *JSDocImplementsTag) VisitEachChild
fn visit_each_child_js_doc_implements_tag<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let tag_name = v.visit_node_hooked(node.tag_name());
    let class_name = v.visit_node_hooked(node.class_name());
    let comment = v.visit_nodes_hooked(node.comment());
    v.factory()
        .update_js_doc_implements_tag(node, tag_name, class_name, comment)
}

// Go: ast/ast_generated.go:7693 (node *JSDocAugmentsTag) VisitEachChild
fn visit_each_child_js_doc_augments_tag<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let tag_name = v.visit_node_hooked(node.tag_name());
    let class_name = v.visit_node_hooked(node.class_name());
    let comment = v.visit_nodes_hooked(node.comment());
    v.factory()
        .update_js_doc_augments_tag(node, tag_name, class_name, comment)
}

// Go: ast/ast_generated.go:7733 (node *JSDocSatisfiesTag) VisitEachChild
fn visit_each_child_js_doc_satisfies_tag<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let tag_name = v.visit_node_hooked(node.tag_name());
    let type_expression = v.visit_node_hooked(node.type_expression());
    let comment = v.visit_nodes_hooked(node.comment());
    v.factory()
        .update_js_doc_satisfies_tag(node, tag_name, type_expression, comment)
}

// Go: ast/ast_generated.go:7773 (node *JSDocThrowsTag) VisitEachChild
fn visit_each_child_js_doc_throws_tag<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let tag_name = v.visit_node_hooked(node.tag_name());
    let type_expression = v.visit_node_hooked(node.type_expression());
    let comment = v.visit_nodes_hooked(node.comment());
    v.factory()
        .update_js_doc_throws_tag(node, tag_name, type_expression, comment)
}

// Go: ast/ast_generated.go:7813 (node *JSDocThisTag) VisitEachChild
fn visit_each_child_js_doc_this_tag<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let tag_name = v.visit_node_hooked(node.tag_name());
    let type_expression = v.visit_node_hooked(node.type_expression());
    let comment = v.visit_nodes_hooked(node.comment());
    v.factory()
        .update_js_doc_this_tag(node, tag_name, type_expression, comment)
}

// Go: ast/ast_generated.go:7861 (node *JSDocImportTag) VisitEachChild
fn visit_each_child_js_doc_import_tag<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let tag_name = v.visit_node_hooked(node.tag_name());
    let import_clause = v.visit_node_hooked(node.import_clause());
    let module_specifier = v.visit_node_hooked(node.module_specifier());
    let attributes = v.visit_node_hooked(node.attributes());
    let comment = v.visit_nodes_hooked(node.comment());
    v.factory().update_js_doc_import_tag(
        node,
        tag_name,
        import_clause,
        module_specifier,
        attributes,
        comment,
    )
}

// Go: ast/ast_generated.go:7906 (node *JSDocCallbackTag) VisitEachChild
fn visit_each_child_js_doc_callback_tag<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let tag_name = v.visit_node_hooked(node.tag_name());
    let type_expression = v.visit_node_hooked(node.type_expression());
    let name = v.visit_node_hooked(node.name());
    let comment = v.visit_nodes_hooked(node.comment());
    v.factory()
        .update_js_doc_callback_tag(node, tag_name, type_expression, name, comment)
}

// Go: ast/ast_generated.go:7950 (node *JSDocOverloadTag) VisitEachChild
fn visit_each_child_js_doc_overload_tag<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let tag_name = v.visit_node_hooked(node.tag_name());
    let type_expression = v.visit_node_hooked(node.type_expression());
    let comment = v.visit_nodes_hooked(node.comment());
    v.factory()
        .update_js_doc_overload_tag(node, tag_name, type_expression, comment)
}

// Go: ast/ast_generated.go:7995 (node *JSDocTypedefTag) VisitEachChild
fn visit_each_child_js_doc_typedef_tag<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let tag_name = v.visit_node_hooked(node.tag_name());
    let type_expression = v.visit_node_hooked(node.type_expression());
    let name = v.visit_node_hooked(node.name());
    let comment = v.visit_nodes_hooked(node.comment());
    v.factory()
        .update_js_doc_typedef_tag(node, tag_name, type_expression, name, comment)
}

// Go: ast/ast_generated.go:8040 (node *JSDocSignature) VisitEachChild
fn visit_each_child_js_doc_signature<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let type_parameters = v.visit_nodes_hooked(node.type_parameter_list());
    let parameters = v.visit_nodes_hooked(node.parameter_list());
    let type_node = v.visit_node_hooked(node.type_());
    v.factory()
        .update_js_doc_signature(node, type_parameters, parameters, type_node)
}

// Go: ast/ast_generated.go:8078 (node *JSDocNameReference) VisitEachChild
fn visit_each_child_js_doc_name_reference<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let name = v.visit_node_hooked(node.name());
    v.factory().update_js_doc_name_reference(node, name)
}

// Go: ast/ast_generated.go:8144 (node *ModuleDeclaration) VisitEachChild
fn visit_each_child_module_declaration<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let modifiers = v.visit_modifiers_hooked(node.modifiers());
    let name = v.visit_node_hooked(node.name());
    let attributes = v.visit_node_hooked(node.attributes());
    let body = v.visit_node_hooked(node.body());
    v.factory()
        .update_module_declaration(node, modifiers, node.keyword(), name, attributes, body)
}

// Go: ast/ast_generated.go:8195 (node *ImportEqualsDeclaration) VisitEachChild
fn visit_each_child_import_equals_declaration<V: ChildVisitor + ?Sized>(
    node: Node,
    v: &mut V,
) -> Node {
    let modifiers = v.visit_modifiers_hooked(node.modifiers());
    let name = v.visit_node_hooked(node.name());
    let module_reference = v.visit_node_hooked(node.module_reference());
    v.factory().update_import_equals_declaration(
        node,
        modifiers,
        node.is_type_only(),
        name,
        module_reference,
    )
}

// Go: ast/ast_generated.go:8250 (node *ExportDeclaration) VisitEachChild
fn visit_each_child_export_declaration<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let modifiers = v.visit_modifiers_hooked(node.modifiers());
    let export_clause = v.visit_node_hooked(node.export_clause());
    let module_specifier = v.visit_node_hooked(node.module_specifier());
    let attributes = v.visit_node_hooked(node.attributes());
    v.factory().update_export_declaration(
        node,
        modifiers,
        node.is_type_only(),
        export_clause,
        module_specifier,
        attributes,
    )
}

// Go: ast/ast_generated.go:8298 (node *ImportTypeNode) VisitEachChild
fn visit_each_child_import_type_node<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let argument = v.visit_node_hooked(node.argument());
    let attributes = v.visit_node_hooked(node.attributes());
    let qualifier = v.visit_node_hooked(node.qualifier());
    let type_arguments = v.visit_nodes_hooked(node.type_argument_list());
    v.factory().update_import_type_node(
        node,
        node.is_type_of(),
        argument,
        attributes,
        qualifier,
        type_arguments,
    )
}

// Go: ast/ast_generated.go:8343 (node *ImportClause) VisitEachChild
fn visit_each_child_import_clause<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let name = v.visit_node_hooked(node.name());
    let named_bindings = v.visit_node_hooked(node.named_bindings());
    v.factory()
        .update_import_clause(node, node.phase_modifier(), name, named_bindings)
}

// Go: ast/ast_generated.go:8392 (node *ImportSpecifier) VisitEachChild
fn visit_each_child_import_specifier<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let property_name = v.visit_node_hooked(node.property_name());
    let name = v.visit_node_hooked(node.name());
    v.factory()
        .update_import_specifier(node, node.is_type_only(), property_name, name)
}

// Go: ast/ast_generated.go:8459 (node *JSDocLink) VisitEachChild
fn visit_each_child_js_doc_link<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let name = v.visit_node_hooked(node.name());
    v.factory().update_js_doc_link(node, name, node.text())
}

// Go: ast/ast_generated.go:8503 (node *JSDocLinkPlain) VisitEachChild
fn visit_each_child_js_doc_link_plain<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let name = v.visit_node_hooked(node.name());
    v.factory()
        .update_js_doc_link_plain(node, name, node.text())
}

// Go: ast/ast_generated.go:8547 (node *JSDocLinkCode) VisitEachChild
fn visit_each_child_js_doc_link_code<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    let name = v.visit_node_hooked(node.name());
    v.factory().update_js_doc_link_code(node, name, node.text())
}

// Go: ast/ast_generated.go:8603 (node *TypeParameterDeclaration) VisitEachChild
fn visit_each_child_type_parameter_declaration<V: ChildVisitor + ?Sized>(
    node: Node,
    v: &mut V,
) -> Node {
    let modifiers = v.visit_modifiers_hooked(node.modifiers());
    let name = v.visit_node_hooked(node.name());
    let constraint = v.visit_node_hooked(node.constraint());
    let expression = v.visit_node_hooked(node.expression());
    let default_type = v.visit_node_hooked(node.default_type());
    v.factory().update_type_parameter_declaration(
        node,
        modifiers,
        name,
        constraint,
        expression,
        default_type,
    )
}

// Go: ast/ast_generated.go:8647 (node *SyntheticReferenceExpression) VisitEachChild
fn visit_each_child_synthetic_reference_expression<V: ChildVisitor + ?Sized>(
    node: Node,
    v: &mut V,
) -> Node {
    let expression = v.visit_node_hooked(node.expression());
    let this_arg = v.visit_node_hooked(node.this_arg());
    v.factory()
        .update_synthetic_reference_expression(node, expression, this_arg)
}

// Go: ast/ast_generated.go:8693 (node *JSDocTypeLiteral) VisitEachChild
fn visit_each_child_js_doc_type_literal<V: ChildVisitor + ?Sized>(node: Node, v: &mut V) -> Node {
    // PORT: Go `core.SameMap` returns the original slice when no element changed;
    // the update compares elements, so a mapped copy with equal elements is the same.
    let jsdoc_property_tags: Vec<Node> = node
        .js_doc_property_tags()
        .into_iter()
        .map(|n| v.visit_node_hooked(n))
        .collect();
    v.factory()
        .update_js_doc_type_literal(node, &jsdoc_property_tags, node.is_array_type())
}

// Go: ast/ast_generated.go:8745 (node *JSDocParameterOrPropertyTag) VisitEachChild
fn visit_each_child_js_doc_parameter_or_property_tag<V: ChildVisitor + ?Sized>(
    node: Node,
    v: &mut V,
) -> Node {
    visit_each_child_js_doc_parameter_or_property_tag_impl(node, v)
}
