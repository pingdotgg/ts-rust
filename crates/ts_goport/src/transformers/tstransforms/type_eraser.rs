//! Port of `transformers/tstransforms/typeeraser.go`.

use super::TxVisit;
use super::runtime_syntax::get_innermost_module_declaration_from_dotted_module;
use crate::prelude::*;
use crate::transformers::modifier_visitor::extract_modifiers;
use crate::transformers::transformer::{
    TransformOptions, Transformer, TransformerBox, TransformerVisit,
};

// Go: transformers/tstransforms/typeeraser.go:11 TypeEraserTransformer
pub struct TypeEraserTransformer {
    emit_context: Rc<EmitContext>,
    compiler_options: &'static CompilerOptions,
    parent_node: Node,
    current_node: Node,
}

// Go: transformers/tstransforms/typeeraser.go:18 NewTypeEraserTransformer
// PORT: Go never returns nil here. The result is an `Option` so the
// constructor has the `TransformerFactory` shape.
pub fn new_type_eraser_transformer(opt: &TransformOptions) -> Option<TransformerBox> {
    let compiler_options = opt.compiler_options;
    let emit_context = opt.context.clone();
    let tx = TypeEraserTransformer {
        emit_context,
        compiler_options,
        parent_node: Node::NIL,
        current_node: Node::NIL,
    };
    Some(Box::new(tx))
}

impl Transformer for TypeEraserTransformer {
    fn emit_context(&self) -> &Rc<EmitContext> {
        &self.emit_context
    }

    fn transform_source_file(&mut self, file: Node) -> Node {
        self.visit_source_file_root(file)
    }
}

impl TransformerVisit for TypeEraserTransformer {
    fn emit_context_rc(&self) -> Rc<EmitContext> {
        self.emit_context.clone()
    }

    // Go: transformers/tstransforms/typeeraser.go:43 TypeEraserTransformer.visit
    fn visit(&mut self, node: Node) -> Node {
        if !node
            .subtree_facts()
            .intersects(SubtreeFacts::SUBTREE_CONTAINS_TYPE_SCRIPT)
        {
            return node;
        }

        if is_statement(node) && has_syntactic_modifier(node, ModifierFlags::AMBIENT) {
            return self.elide(node);
        }

        let grandparent_node = self.push_node(node);
        let result = self.visit_worker(node);
        self.pop_node(grandparent_node);
        result
    }
}

impl TypeEraserTransformer {
    // Go: transformers/tstransforms/typeeraser.go:26 TypeEraserTransformer.pushNode
    /// Pushes a new child node onto the ancestor tracking stack, returning the grandparent node to be restored later via `popNode`.
    fn push_node(&mut self, node: Node) -> Node {
        let grandparent_node = self.parent_node;
        self.parent_node = self.current_node;
        self.current_node = node;
        grandparent_node
    }

    // Go: transformers/tstransforms/typeeraser.go:34 TypeEraserTransformer.popNode
    /// Pops the last child node off the ancestor tracking stack, restoring the grandparent node.
    fn pop_node(&mut self, grandparent_node: Node) {
        self.current_node = self.parent_node;
        self.parent_node = grandparent_node;
    }

    // Go: transformers/tstransforms/typeeraser.go:39 TypeEraserTransformer.elide
    fn elide(&self, node: Node) -> Node {
        self.emit_context.new_not_emitted_statement(node)
    }

    /// The body of Go `visit` after `pushNode` (Go pops with `defer`).
    fn visit_worker(&mut self, node: Node) -> Node {
        let ec = self.emit_context.clone();
        let f = ec.factory();
        match node.kind() {
            // TypeScript accessibility and readonly modifiers are elided
            SyntaxKind::PublicKeyword
            | SyntaxKind::PrivateKeyword
            | SyntaxKind::ProtectedKeyword
            | SyntaxKind::AbstractKeyword
            | SyntaxKind::OverrideKeyword
            | SyntaxKind::ConstKeyword
            | SyntaxKind::DeclareKeyword
            | SyntaxKind::ReadonlyKeyword
            // TypeScript type nodes are elided.
            | SyntaxKind::ArrayType
            | SyntaxKind::TupleType
            | SyntaxKind::OptionalType
            | SyntaxKind::RestType
            | SyntaxKind::TypeLiteral
            | SyntaxKind::TypePredicate
            | SyntaxKind::TypeParameter
            | SyntaxKind::AnyKeyword
            | SyntaxKind::UnknownKeyword
            | SyntaxKind::BooleanKeyword
            | SyntaxKind::StringKeyword
            | SyntaxKind::NumberKeyword
            | SyntaxKind::NeverKeyword
            | SyntaxKind::VoidKeyword
            | SyntaxKind::SymbolKeyword
            | SyntaxKind::ConstructorType
            | SyntaxKind::FunctionType
            | SyntaxKind::TypeQuery
            | SyntaxKind::TypeReference
            | SyntaxKind::UnionType
            | SyntaxKind::IntersectionType
            | SyntaxKind::ConditionalType
            | SyntaxKind::ParenthesizedType
            | SyntaxKind::ThisType
            | SyntaxKind::TypeOperator
            | SyntaxKind::IndexedAccessType
            | SyntaxKind::MappedType
            | SyntaxKind::LiteralType
            // TypeScript index signatures are elided.
            | SyntaxKind::IndexSignature => Node::NIL,

            SyntaxKind::InKeyword | SyntaxKind::OutKeyword => {
                // TypeScript `in`/`out` variance modifiers are elided. These keywords are only
                // meaningful as modifiers on type parameters (which are themselves elided), but they may
                // appear as a grammar error on other declarations and must not leak into the emitted JS.
                // The `in` binary operator shares this token kind, so only elide when used as a modifier.
                if self.parent_node.is_nil() || !is_binary_expression(self.parent_node) {
                    return Node::NIL;
                }
                self.visit_each_child(node)
            }

            // reparsed commonjs are elided
            SyntaxKind::JsImportDeclaration => Node::NIL,

            SyntaxKind::TypeAliasDeclaration
            | SyntaxKind::JsTypeAliasDeclaration
            | SyntaxKind::InterfaceDeclaration => {
                // TypeScript type-only declarations are elided.
                self.elide(node)
            }

            // TypeScript namespace export declarations are elided.
            SyntaxKind::NamespaceExportDeclaration => Node::NIL,

            SyntaxKind::ModuleDeclaration => {
                if !is_identifier(node.name())
                    || !is_instantiated_module(
                        node,
                        self.compiler_options.should_preserve_const_enums(),
                    )
                    || get_innermost_module_declaration_from_dotted_module(node)
                        .body()
                        .is_nil()
                {
                    // TypeScript module declarations are elided if they are not instantiated or have no body
                    return self.elide(node);
                }
                self.visit_each_child(node)
            }

            SyntaxKind::ExpressionWithTypeArguments => {
                let expression = self.visit_node(node.expression());
                f.update_expression_with_type_arguments(node, expression, NodeList::NIL)
            }

            SyntaxKind::PropertyDeclaration => {
                if self.compiler_options.experimental_decorators.is_true()
                    && has_syntactic_modifier(
                        node,
                        ModifierFlags::AMBIENT | ModifierFlags::ABSTRACT,
                    )
                    && has_decorators(node)
                {
                    // declare/abstract props with decorators must be preserved until the decorator transform can process them and remove them
                    let modifiers = self.visit_modifiers(node.modifiers());
                    let name = self.visit_node(node.name());
                    let initializer = self.visit_node(node.initializer());
                    return f.update_property_declaration(
                        node,
                        modifiers,
                        name,
                        Node::NIL,
                        Node::NIL,
                        initializer,
                    );
                }
                if has_syntactic_modifier(node, ModifierFlags::AMBIENT | ModifierFlags::ABSTRACT) {
                    // TypeScript `declare` fields are elided
                    return Node::NIL;
                }
                let modifiers = self.visit_modifiers(node.modifiers());
                let name = self.visit_node(node.name());
                let initializer = self.visit_node(node.initializer());
                f.update_property_declaration(
                    node,
                    modifiers,
                    name,
                    Node::NIL,
                    Node::NIL,
                    initializer,
                )
            }

            SyntaxKind::Constructor => {
                if node_is_missing(node.body()) {
                    // TypeScript overloads are elided
                    return Node::NIL;
                }
                let parameters = self.visit_nodes(node.parameter_list());
                let body = self.visit_node(node.body());
                f.update_constructor_declaration(
                    node,
                    ModifierList::NIL,
                    NodeList::NIL,
                    parameters,
                    Node::NIL,
                    Node::NIL,
                    body,
                )
            }

            SyntaxKind::MethodDeclaration => {
                if node_is_missing(node.body()) {
                    // TypeScript overloads are elided
                    return Node::NIL;
                }
                let modifiers = self.visit_modifiers(node.modifiers());
                let name = self.visit_node(node.name());
                let parameters = self.visit_nodes(node.parameter_list());
                let body = self.visit_node(node.body());
                f.update_method_declaration(
                    node,
                    modifiers,
                    node.asterisk_token(),
                    name,
                    Node::NIL,
                    NodeList::NIL,
                    parameters,
                    Node::NIL,
                    Node::NIL,
                    body,
                )
            }

            SyntaxKind::GetAccessor => {
                if node_is_missing(node.body())
                    && has_syntactic_modifier(node, ModifierFlags::ABSTRACT)
                {
                    // Abstract accessors are elided
                    return Node::NIL;
                }
                let mut body = self.visit_node(node.body());
                if body.is_nil() {
                    body = f.new_block(f.new_node_list(&[]), false);
                }
                let modifiers = self.visit_modifiers(node.modifiers());
                let name = self.visit_node(node.name());
                let parameters = self.visit_nodes(node.parameter_list());
                f.update_get_accessor_declaration(
                    node,
                    modifiers,
                    name,
                    NodeList::NIL,
                    parameters,
                    Node::NIL,
                    Node::NIL,
                    body,
                )
            }

            SyntaxKind::SetAccessor => {
                if node_is_missing(node.body())
                    && has_syntactic_modifier(node, ModifierFlags::ABSTRACT)
                {
                    // Abstract accessors are elided
                    return Node::NIL;
                }
                let mut body = self.visit_node(node.body());
                if body.is_nil() {
                    body = f.new_block(f.new_node_list(&[]), false);
                }
                let modifiers = self.visit_modifiers(node.modifiers());
                let name = self.visit_node(node.name());
                let parameters = self.visit_nodes(node.parameter_list());
                f.update_set_accessor_declaration(
                    node,
                    modifiers,
                    name,
                    NodeList::NIL,
                    parameters,
                    Node::NIL,
                    Node::NIL,
                    body,
                )
            }

            SyntaxKind::VariableDeclaration => {
                let name = self.visit_node(node.name());
                let initializer = self.visit_node(node.initializer());
                let updated =
                    f.update_variable_declaration(node, name, Node::NIL, Node::NIL, initializer);
                if node.type_().is_some() {
                    ec.set_type_node(updated.name(), node.type_());
                }
                updated
            }

            SyntaxKind::HeritageClause => {
                if node.token() == SyntaxKind::ImplementsKeyword {
                    // TypeScript `implements` clauses are elided
                    return Node::NIL;
                }
                let types = self.visit_nodes(node.types());
                f.update_heritage_clause(node, node.token(), types)
            }

            SyntaxKind::ClassDeclaration => {
                let modifiers = self.visit_modifiers(node.modifiers());
                let name = self.visit_node(node.name());
                let heritage_clauses = self.visit_nodes(node.heritage_clauses());
                let members = self.visit_nodes(node.member_list());
                f.update_class_declaration(
                    node,
                    modifiers,
                    name,
                    NodeList::NIL,
                    heritage_clauses,
                    members,
                )
            }

            SyntaxKind::ClassExpression => {
                let modifiers = self.visit_modifiers(node.modifiers());
                let name = self.visit_node(node.name());
                let heritage_clauses = self.visit_nodes(node.heritage_clauses());
                let members = self.visit_nodes(node.member_list());
                f.update_class_expression(
                    node,
                    modifiers,
                    name,
                    NodeList::NIL,
                    heritage_clauses,
                    members,
                )
            }

            SyntaxKind::FunctionDeclaration => {
                if node_is_missing(node.body()) {
                    // TypeScript overloads are elided
                    return self.elide(node);
                }
                let modifiers = self.visit_modifiers(node.modifiers());
                let name = self.visit_node(node.name());
                let parameters = self.visit_nodes(node.parameter_list());
                let body = self.visit_node(node.body());
                f.update_function_declaration(
                    node,
                    modifiers,
                    node.asterisk_token(),
                    name,
                    NodeList::NIL,
                    parameters,
                    Node::NIL,
                    Node::NIL,
                    body,
                )
            }

            SyntaxKind::FunctionExpression => {
                let modifiers = self.visit_modifiers(node.modifiers());
                let name = self.visit_node(node.name());
                let parameters = self.visit_nodes(node.parameter_list());
                let body = self.visit_node(node.body());
                f.update_function_expression(
                    node,
                    modifiers,
                    node.asterisk_token(),
                    name,
                    NodeList::NIL,
                    parameters,
                    Node::NIL,
                    Node::NIL,
                    body,
                )
            }

            SyntaxKind::ArrowFunction => {
                let modifiers = self.visit_modifiers(node.modifiers());
                let parameters = self.visit_nodes(node.parameter_list());
                let body = self.visit_node(node.body());
                f.update_arrow_function(
                    node,
                    modifiers,
                    NodeList::NIL,
                    parameters,
                    Node::NIL,
                    Node::NIL,
                    node.equals_greater_than_token(),
                    body,
                )
            }

            SyntaxKind::Parameter => {
                if is_this_parameter(node) {
                    // TypeScript `this` parameters are elided
                    return Node::NIL;
                }
                // preserve parameter property modifiers to be handled by the runtime transformer
                let mut modifiers = ModifierList::NIL;
                if is_parameter_property_declaration(node, self.parent_node) {
                    modifiers = extract_modifiers(
                        &ec,
                        node.modifiers(),
                        ModifierFlags::PARAMETER_PROPERTY_MODIFIER,
                    );
                }
                // preserve decorators for the decorator transforms
                if has_decorators(node) {
                    let decorators = node.decorators().to_vec();
                    let (visited, _) = self.visit_slice(&decorators);
                    if modifiers.is_nil() {
                        modifiers = f.new_modifier_list(&visited);
                    } else {
                        let mut nodes = modifiers.nodes().to_vec();
                        nodes.extend(visited);
                        modifiers = f.new_modifier_list(&nodes);
                    }
                }
                let name = self.visit_node(node.name());
                let initializer = self.visit_node(node.initializer());
                f.update_parameter_declaration(
                    node,
                    modifiers,
                    node.dot_dot_dot_token(),
                    name,
                    Node::NIL,
                    Node::NIL,
                    initializer,
                )
            }

            SyntaxKind::CallExpression => {
                let expression = self.visit_node(node.expression());
                let arguments = self.visit_nodes(node.argument_list());
                f.update_call_expression(
                    node,
                    expression,
                    node.question_dot_token(),
                    NodeList::NIL,
                    arguments,
                    node.flags(),
                )
            }

            SyntaxKind::NewExpression => {
                let expression = self.visit_node(node.expression());
                let arguments = self.visit_nodes(node.argument_list());
                f.update_new_expression(node, expression, NodeList::NIL, arguments)
            }

            SyntaxKind::TaggedTemplateExpression => {
                let tag = self.visit_node(node.tag());
                let template = self.visit_node(node.template());
                f.update_tagged_template_expression(
                    node,
                    tag,
                    node.question_dot_token(),
                    NodeList::NIL,
                    template,
                    node.flags(),
                )
            }

            SyntaxKind::NonNullExpression
            | SyntaxKind::TypeAssertionExpression
            | SyntaxKind::AsExpression
            | SyntaxKind::SatisfiesExpression => {
                let expression = self.visit_node(node.expression());
                let partial = f.new_partially_emitted_expression(expression);
                ec.set_original(partial, node);
                set_node_loc(partial, node.loc());
                partial
            }

            SyntaxKind::ParenthesizedExpression => {
                if !is_js_doc_type_assertion(node) {
                    let expression = skip_outer_expressions(
                        node.expression(),
                        OuterExpressionKinds::OEK_ALL_EXCEPT_ASSERTIONS_OR_EXPRESSIONS_WITH_TYPE_ARGUMENTS,
                    );
                    if is_assertion_expression(expression) || is_satisfies_expression(expression) {
                        let visited = self.visit_node(node.expression());
                        let partial = f.new_partially_emitted_expression(visited);
                        ec.set_original(partial, node);
                        set_node_loc(partial, node.loc());
                        return partial;
                    }
                }
                self.visit_each_child(node)
            }

            SyntaxKind::JsxSelfClosingElement => {
                let tag_name = self.visit_node(node.tag_name());
                let attributes = self.visit_node(node.attributes());
                f.update_jsx_self_closing_element(node, tag_name, NodeList::NIL, attributes)
            }

            SyntaxKind::JsxOpeningElement => {
                let tag_name = self.visit_node(node.tag_name());
                let attributes = self.visit_node(node.attributes());
                f.update_jsx_opening_element(node, tag_name, NodeList::NIL, attributes)
            }

            SyntaxKind::ImportEqualsDeclaration => {
                if node.is_type_only() {
                    // elide type-only imports
                    return Node::NIL;
                }
                self.visit_each_child(node)
            }

            SyntaxKind::ImportDeclaration => {
                if node.import_clause().is_nil() {
                    // Do not elide a side-effect only import declaration.
                    //  import "foo";
                    return node;
                }
                let import_clause = self.visit_node(node.import_clause());
                if import_clause.is_nil() {
                    return Node::NIL;
                }
                f.update_import_declaration(
                    node,
                    node.modifiers(),
                    import_clause,
                    node.module_specifier(),
                    node.attributes(),
                )
            }

            SyntaxKind::ImportClause => {
                if node.is_type_only() {
                    // Always elide type-only imports
                    return Node::NIL;
                }
                let name = node.name();
                let mut named_bindings = self.visit_node(node.named_bindings());
                // Empty {} due to type-only import erasure can be skipped if there is also a default import
                if name.is_some()
                    && named_bindings.is_some()
                    && is_named_imports(named_bindings)
                    && named_bindings.element_list().nodes().is_empty()
                    && !node.named_bindings().element_list().nodes().is_empty()
                {
                    // the default binding keeps the import; a source-written {} is left as is
                    named_bindings = Node::NIL;
                }
                if name.is_nil() && named_bindings.is_nil() {
                    // all import bindings were elided
                    return Node::NIL;
                }
                f.update_import_clause(node, node.phase_modifier(), name, named_bindings)
            }

            SyntaxKind::NamedImports => {
                if node.element_list().nodes().is_empty() {
                    // Do not elide a side-effect only import declaration.
                    return node;
                }
                let elements = self.visit_nodes(node.element_list());
                if !self.compiler_options.verbatim_module_syntax.is_true()
                    && elements.nodes().is_empty()
                {
                    // all import specifiers were elided
                    return Node::NIL;
                }
                f.update_named_imports(node, elements)
            }

            SyntaxKind::ImportSpecifier => {
                if node.is_type_only() {
                    // elide type-only or unused imports
                    return Node::NIL;
                }
                node
            }

            SyntaxKind::ExportDeclaration => {
                if node.is_type_only() {
                    // elide type-only exports
                    return Node::NIL;
                }
                let mut export_clause = Node::NIL;
                if node.export_clause().is_some() {
                    export_clause = self.visit_node(node.export_clause());
                    if export_clause.is_nil() {
                        // all export bindings were elided
                        return Node::NIL;
                    }
                }
                let module_specifier = self.visit_node(node.module_specifier());
                let attributes = self.visit_node(node.attributes());
                f.update_export_declaration(
                    node,
                    ModifierList::NIL, /*modifiers*/
                    false,             /*isTypeOnly*/
                    export_clause,
                    module_specifier,
                    attributes,
                )
            }

            SyntaxKind::NamedExports => {
                if node.element_list().nodes().is_empty() {
                    // Do not elide an empty export declaration.
                    return node;
                }

                let elements = self.visit_nodes(node.element_list());
                if !self.compiler_options.verbatim_module_syntax.is_true()
                    && elements.nodes().is_empty()
                {
                    // all export specifiers were elided
                    return Node::NIL;
                }
                f.update_named_exports(node, elements)
            }

            SyntaxKind::ExportSpecifier => {
                if node.is_type_only() {
                    // elide unused export
                    return Node::NIL;
                }
                node
            }

            SyntaxKind::EnumDeclaration => {
                if is_enum_const(node) {
                    return node;
                }
                self.visit_each_child(node)
            }

            _ => self.visit_each_child(node),
        }
    }
}
