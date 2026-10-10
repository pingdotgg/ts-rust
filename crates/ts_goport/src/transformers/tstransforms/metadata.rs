//! Port of `transformers/tstransforms/metadata.go`.

use super::TxVisit;
use super::legacy_decorators::get_decorators_of_parameters;
use super::type_serializer::{
    MetadataSerializer, MetadataSerializerContext, new_metadata_serializer,
};
use crate::prelude::*;
use crate::transformers::transformer::{
    TransformOptions, Transformer, TransformerBox, TransformerVisit,
};

// Go: transformers/tstransforms/metadata.go:10 USE_NEW_TYPE_METADATA_FORMAT
const USE_NEW_TYPE_METADATA_FORMAT: bool = false;

// Go: transformers/tstransforms/metadata.go:12 MetadataTransformer
pub struct MetadataTransformer {
    emit_context: Rc<EmitContext>,
    legacy_decorators: bool,
    resolver: Rc<dyn EmitResolver>,

    serializer: Option<MetadataSerializer>,
    language_version: ScriptTarget,
    strict_null_checks: bool,
    parent: Node,
    current_lexical_scope: Node,
}

// Go: transformers/tstransforms/metadata.go:24 NewMetadataTransformer
// PORT: Go never returns nil here. The result is an `Option` so the
// constructor has the `TransformerFactory` shape.
pub fn new_metadata_transformer(opt: &TransformOptions) -> Option<TransformerBox> {
    let compiler_options = opt.compiler_options;
    let tx = MetadataTransformer {
        emit_context: opt.context.clone(),
        legacy_decorators: compiler_options.experimental_decorators.is_true(),
        resolver: opt.emit_resolver.clone(),
        serializer: None,
        language_version: compiler_options.get_emit_script_target(),
        strict_null_checks: compiler_options
            .get_strict_option_value(compiler_options.strict_null_checks),
        parent: Node::NIL,
        current_lexical_scope: Node::NIL,
    };
    Some(Box::new(tx))
}

impl Transformer for MetadataTransformer {
    fn emit_context(&self) -> &Rc<EmitContext> {
        &self.emit_context
    }

    fn transform_source_file(&mut self, file: Node) -> Node {
        self.visit_source_file_root(file)
    }
}

impl TransformerVisit for MetadataTransformer {
    fn emit_context_rc(&self) -> Rc<EmitContext> {
        self.emit_context.clone()
    }

    // Go: transformers/tstransforms/metadata.go:34 MetadataTransformer.visit
    fn visit(&mut self, node: Node) -> Node {
        if !node
            .subtree_facts()
            .intersects(SubtreeFacts::SUBTREE_CONTAINS_DECORATORS)
        {
            return node;
        }

        match node.kind() {
            SyntaxKind::ClassDeclaration => self.visit_class_declaration(node),
            SyntaxKind::ClassExpression => self.visit_class_expression(node),
            SyntaxKind::ObjectLiteralExpression => self.visit_object_literal_expression(node),
            SyntaxKind::PropertyDeclaration => self.visit_property_declaration(node),
            SyntaxKind::MethodDeclaration => self.visit_method_declaration(node),
            SyntaxKind::SetAccessor => self.visit_set_accessor(node),
            SyntaxKind::GetAccessor => self.visit_get_accessor(node),
            SyntaxKind::SourceFile => {
                self.parent = Node::NIL;
                self.current_lexical_scope = node;
                self.serializer = Some(new_metadata_serializer(
                    self.resolver.clone(),
                    self.emit_context.clone(),
                    self.language_version,
                    self.strict_null_checks,
                ));
                let updated = self.visit_each_child(node);
                let ec = self.emit_context.clone();
                ec.add_emit_helper(updated, &ec.read_emit_helpers());
                // PORT: Go resets these with `defer` (scope first, then parent).
                self.set_current_lexical_scope(Node::NIL);
                self.set_parent(Node::NIL);
                updated
            }
            SyntaxKind::ModuleBlock | SyntaxKind::Block | SyntaxKind::CaseBlock => {
                let old_scope = self.current_lexical_scope;
                self.current_lexical_scope = node;
                let result = self.visit_each_child(node);
                self.set_current_lexical_scope(old_scope);
                result
            }
            _ => self.visit_each_child(node),
        }
    }
}

impl MetadataTransformer {
    // Go: transformers/tstransforms/metadata.go:71 MetadataTransformer.setParent
    fn set_parent(&mut self, node: Node) {
        self.parent = node;
    }

    // Go: transformers/tstransforms/metadata.go:75 MetadataTransformer.setCurrentLexicalScope
    fn set_current_lexical_scope(&mut self, node: Node) {
        self.current_lexical_scope = node;
    }

    // Go: transformers/tstransforms/metadata.go:81 MetadataTransformer.visitObjectLiteralExpression
    fn visit_object_literal_expression(&mut self, node: Node) -> Node {
        let old_parent = self.parent;
        self.parent = node;
        let result = self.visit_each_child(node);
        // PORT: Go restores this with `defer`.
        self.set_parent(old_parent);
        result
    }

    // Go: transformers/tstransforms/metadata.go:89 MetadataTransformer.visitClassExpression
    fn visit_class_expression(&mut self, node: Node) -> Node {
        let old_parent = self.parent;
        self.parent = node;
        let result = self.visit_class_expression_worker(node);
        // PORT: Go restores this with `defer`.
        self.set_parent(old_parent);
        result
    }

    fn visit_class_expression_worker(&mut self, node: Node) -> Node {
        if !class_or_constructor_parameter_is_decorated(self.legacy_decorators, node) {
            return self.visit_each_child(node);
        }
        let visited_modifiers = self.visit_modifiers(node.modifiers());
        let modifiers = self.inject_class_type_metadata(visited_modifiers, node);
        let name = self.visit_node(node.name());
        let type_parameters = self.visit_nodes(node.type_parameter_list());
        let heritage_clauses = self.visit_nodes(node.heritage_clauses());
        let members = self.visit_nodes(node.member_list());
        self.emit_context.factory().update_class_expression(
            node,
            modifiers,
            name,
            type_parameters,
            heritage_clauses,
            members,
        )
    }

    // Go: transformers/tstransforms/metadata.go:98 MetadataTransformer.visitClassDeclaration
    fn visit_class_declaration(&mut self, node: Node) -> Node {
        let old_parent = self.parent;
        self.parent = node;
        let result = self.visit_class_declaration_worker(node);
        // PORT: Go restores this with `defer`.
        self.set_parent(old_parent);
        result
    }

    fn visit_class_declaration_worker(&mut self, node: Node) -> Node {
        if !class_or_constructor_parameter_is_decorated(self.legacy_decorators, node) {
            return self.visit_each_child(node);
        }
        let visited_modifiers = self.visit_modifiers(node.modifiers());
        let modifiers = self.inject_class_type_metadata(visited_modifiers, node);
        let name = self.visit_node(node.name());
        let type_parameters = self.visit_nodes(node.type_parameter_list());
        let heritage_clauses = self.visit_nodes(node.heritage_clauses());
        let members = self.visit_nodes(node.member_list());
        self.emit_context.factory().update_class_declaration(
            node,
            modifiers,
            name,
            type_parameters,
            heritage_clauses,
            members,
        )
    }

    // Go: transformers/tstransforms/metadata.go:117 MetadataTransformer.visitPropertyDeclaration
    fn visit_property_declaration(&mut self, node: Node) -> Node {
        if !has_decorators(node) {
            return self.visit_each_child(node);
        }

        let visited_modifiers = self.visit_modifiers(node.modifiers());
        let modifiers =
            self.inject_class_element_type_metadata(visited_modifiers, node, self.parent);
        let name = self.visit_node(node.name());
        let postfix_token = self.visit_node(node.postfix_token());
        let type_node = self.visit_node(node.type_());
        let initializer = self.visit_node(node.initializer());
        self.emit_context.factory().update_property_declaration(
            node,
            modifiers,
            name,
            postfix_token,
            type_node,
            initializer,
        )
    }

    // Go: transformers/tstransforms/metadata.go:133 MetadataTransformer.visitMethodDeclaration
    fn visit_method_declaration(&mut self, node: Node) -> Node {
        if !has_decorators(node) && get_decorators_of_parameters(node).is_empty() {
            return self.visit_each_child(node);
        }

        let visited_modifiers = self.visit_modifiers(node.modifiers());
        let modifiers =
            self.inject_class_element_type_metadata(visited_modifiers, node, self.parent);
        let asterisk_token = self.visit_node(node.asterisk_token());
        let name = self.visit_node(node.name());
        let postfix_token = self.visit_node(node.postfix_token());
        let type_parameters = self.visit_nodes(node.type_parameter_list());
        let parameters = self.visit_nodes(node.parameter_list());
        let type_node = self.visit_node(node.type_());
        let full_signature = self.visit_node(node.full_signature());
        let body = self.visit_node(node.body());
        self.emit_context.factory().update_method_declaration(
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

    // Go: transformers/tstransforms/metadata.go:153 MetadataTransformer.visitSetAccessor
    fn visit_set_accessor(&mut self, node: Node) -> Node {
        if !has_decorators(node) && get_decorators_of_parameters(node).is_empty() {
            return self.visit_each_child(node);
        }

        let visited_modifiers = self.visit_modifiers(node.modifiers());
        let modifiers =
            self.inject_class_element_type_metadata(visited_modifiers, node, self.parent);
        let name = self.visit_node(node.name());
        let type_parameters = self.visit_nodes(node.type_parameter_list());
        let parameters = self.visit_nodes(node.parameter_list());
        let type_node = self.visit_node(node.type_());
        let full_signature = self.visit_node(node.full_signature());
        let body = self.visit_node(node.body());
        self.emit_context.factory().update_set_accessor_declaration(
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

    // Go: transformers/tstransforms/metadata.go:171 MetadataTransformer.visitGetAccessor
    fn visit_get_accessor(&mut self, node: Node) -> Node {
        if !has_decorators(node) {
            return self.visit_each_child(node);
        }

        let visited_modifiers = self.visit_modifiers(node.modifiers());
        let modifiers =
            self.inject_class_element_type_metadata(visited_modifiers, node, self.parent);
        let name = self.visit_node(node.name());
        let type_parameters = self.visit_nodes(node.type_parameter_list());
        let parameters = self.visit_nodes(node.parameter_list());
        let type_node = self.visit_node(node.type_());
        let full_signature = self.visit_node(node.full_signature());
        let body = self.visit_node(node.body());
        self.emit_context.factory().update_get_accessor_declaration(
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

    // Go: transformers/tstransforms/metadata.go:189 MetadataTransformer.injectClassTypeMetadata
    fn inject_class_type_metadata(&mut self, list: ModifierList, node: Node) -> ModifierList {
        let metadata = self.get_type_metadata(node, node);
        if !metadata.is_empty() {
            let f = self.emit_context.factory();
            let mut original_nodes: Vec<Node> = Vec::new();
            if list.is_some() {
                original_nodes = list.nodes().to_vec();
            }
            if original_nodes.is_empty() {
                if list.is_some() {
                    return f.new_modifier_list_with_loc(&metadata, list.node_list().loc());
                }
                return f.new_modifier_list(&metadata);
            }
            let mut modifiers_array: Vec<Node> = Vec::new();
            if is_modifier(original_nodes[0])
                && (original_nodes[0].kind() == SyntaxKind::DefaultKeyword
                    || original_nodes[0].kind() == SyntaxKind::ExportKeyword)
            {
                modifiers_array.push(original_nodes[0]);
                if original_nodes.len() > 1
                    && (original_nodes[1].kind() == SyntaxKind::DefaultKeyword
                        || original_nodes[1].kind() == SyntaxKind::ExportKeyword)
                {
                    modifiers_array.push(original_nodes[1]);
                }
            }
            let rest_start = modifiers_array.len();
            modifiers_array.extend(original_nodes.iter().copied().filter(|&n| is_decorator(n)));
            modifiers_array.extend(metadata);
            modifiers_array.extend(
                original_nodes[rest_start..]
                    .iter()
                    .copied()
                    .filter(|&n| is_modifier(n)),
            );
            return f.new_modifier_list_with_loc(&modifiers_array, list.node_list().loc());
        }
        list
    }

    // Go: transformers/tstransforms/metadata.go:223 MetadataTransformer.injectClassElementTypeMetadata
    fn inject_class_element_type_metadata(
        &mut self,
        list: ModifierList,
        node: Node,
        container: Node,
    ) -> ModifierList {
        if !is_class_like(container) {
            return list;
        }
        if !class_element_or_class_element_parameter_is_decorated(
            self.legacy_decorators,
            node,
            container,
        ) {
            return list;
        }
        let metadata = self.get_type_metadata(node, container);
        if !metadata.is_empty() {
            let f = self.emit_context.factory();
            let mut original_nodes: Vec<Node> = Vec::new();
            if list.is_some() {
                original_nodes = list.nodes().to_vec();
            }
            if original_nodes.is_empty() {
                if list.is_some() {
                    return f.new_modifier_list_with_loc(&metadata, list.node_list().loc());
                }
                return f.new_modifier_list(&metadata);
            }
            let mut modifiers_array: Vec<Node> = Vec::new();
            modifiers_array.extend(original_nodes.iter().copied().filter(|&n| is_decorator(n)));
            modifiers_array.extend(metadata);
            modifiers_array.extend(original_nodes.iter().copied().filter(|&n| is_modifier(n)));
            return f.new_modifier_list_with_loc(&modifiers_array, list.node_list().loc());
        }
        list
    }

    // Go: transformers/tstransforms/metadata.go:261 MetadataTransformer.getTypeMetadata
    /// Gets optional type metadata for a declaration.
    ///
    /// @param node The declaration node.
    fn get_type_metadata(&mut self, node: Node, container: Node) -> Vec<Node> {
        // Decorator metadata is not yet supported for ES decorators.
        if !self.legacy_decorators {
            return Vec::new();
        }
        if USE_NEW_TYPE_METADATA_FORMAT {
            return self.get_new_type_metadata(node, container);
        }
        self.get_old_type_metadata(node, container)
    }

    /// Go `metadataSerializerContext{currentLexicalScope: tx.currentLexicalScope, currentNameScope: container}`.
    fn serializer_context(&self, container: Node) -> MetadataSerializerContext {
        MetadataSerializerContext {
            current_lexical_scope: self.current_lexical_scope,
            current_name_scope: container,
            serializing_conditional_type_branch: false,
        }
    }

    /// Go `tx.serializer` (set when the source file is visited).
    fn serializer(&mut self) -> &mut MetadataSerializer {
        self.serializer
            .as_mut()
            .expect("MetadataTransformer.serializer is nil")
    }

    // Go: transformers/tstransforms/metadata.go:272 MetadataTransformer.getOldTypeMetadata
    fn get_old_type_metadata(&mut self, node: Node, container: Node) -> Vec<Node> {
        let ec = self.emit_context.clone();
        let f = ec.factory();
        let mut decorators = Vec::new();
        if self.should_add_type_metadata(node) {
            let ctx = self.serializer_context(container);
            let value = self
                .serializer()
                .serialize_type_of_node_exported(ctx, node, container);
            let type_metadata = f.new_metadata_helper("design:type", value);
            decorators.push(f.new_decorator(type_metadata));
        }
        if self.should_add_param_types_metadata(node) {
            let ctx = self.serializer_context(container);
            let value = self
                .serializer()
                .serialize_parameter_types_of_node_exported(ctx, node, container);
            let param_types_metadata = f.new_metadata_helper("design:paramtypes", value);
            decorators.push(f.new_decorator(param_types_metadata));
        }
        if self.should_add_return_type_metadata(node) {
            let ctx = self.serializer_context(container);
            let value = self
                .serializer()
                .serialize_return_type_of_node_exported(ctx, node);
            let return_type_metadata = f.new_metadata_helper("design:returntype", value);
            decorators.push(f.new_decorator(return_type_metadata));
        }
        decorators
    }

    // Go: transformers/tstransforms/metadata.go:289 MetadataTransformer.getNewTypeMetadata
    fn get_new_type_metadata(&mut self, node: Node, container: Node) -> Vec<Node> {
        let ec = self.emit_context.clone();
        let f = ec.factory();
        let mut properties = Vec::new();
        if self.should_add_type_metadata(node) {
            let ctx = self.serializer_context(container);
            let value = self
                .serializer()
                .serialize_type_of_node_exported(ctx, node, container);
            properties.push(f.new_property_assignment(
                ModifierList::NIL,
                f.new_identifier("type"),
                Node::NIL,
                Node::NIL,
                f.new_arrow_function(
                    ModifierList::NIL,
                    NodeList::NIL,
                    f.new_node_list(&[]),
                    Node::NIL,
                    Node::NIL,
                    f.new_token(SyntaxKind::EqualsGreaterThanToken),
                    value,
                ),
            ));
        }
        if self.should_add_param_types_metadata(node) {
            let ctx = self.serializer_context(container);
            let value = self
                .serializer()
                .serialize_parameter_types_of_node_exported(ctx, node, container);
            properties.push(f.new_property_assignment(
                ModifierList::NIL,
                f.new_identifier("paramTypes"),
                Node::NIL,
                Node::NIL,
                f.new_arrow_function(
                    ModifierList::NIL,
                    NodeList::NIL,
                    f.new_node_list(&[]),
                    Node::NIL,
                    Node::NIL,
                    f.new_token(SyntaxKind::EqualsGreaterThanToken),
                    value,
                ),
            ));
        }
        if self.should_add_return_type_metadata(node) {
            let ctx = self.serializer_context(container);
            let value = self
                .serializer()
                .serialize_return_type_of_node_exported(ctx, node);
            properties.push(f.new_property_assignment(
                ModifierList::NIL,
                f.new_identifier("returnType"),
                Node::NIL,
                Node::NIL,
                f.new_arrow_function(
                    ModifierList::NIL,
                    NodeList::NIL,
                    f.new_node_list(&[]),
                    Node::NIL,
                    Node::NIL,
                    f.new_token(SyntaxKind::EqualsGreaterThanToken),
                    value,
                ),
            ));
        }
        if !properties.is_empty() {
            let type_info_metadata = f.new_metadata_helper(
                "design:typeinfo",
                f.new_object_literal_expression(f.new_node_list(&properties), true),
            );
            return vec![f.new_decorator(type_info_metadata)];
        }
        Vec::new()
    }

    // Go: transformers/tstransforms/metadata.go:356 MetadataTransformer.shouldAddTypeMetadata
    /// Determines whether to emit the "design:type" metadata based on the node's kind.
    /// The caller should have already tested whether the node has decorators and whether the
    /// emitDecoratorMetadata compiler option is set.
    ///
    /// @param node The node to test.
    fn should_add_type_metadata(&self, node: Node) -> bool {
        matches!(
            node.kind(),
            SyntaxKind::MethodDeclaration
                | SyntaxKind::GetAccessor
                | SyntaxKind::SetAccessor
                | SyntaxKind::PropertyDeclaration
        )
    }

    // Go: transformers/tstransforms/metadata.go:371 MetadataTransformer.shouldAddReturnTypeMetadata
    /// Determines whether to emit the "design:returntype" metadata based on the node's kind.
    /// The caller should have already tested whether the node has decorators and whether the
    /// emitDecoratorMetadata compiler option is set.
    ///
    /// @param node The node to test.
    fn should_add_return_type_metadata(&self, node: Node) -> bool {
        node.kind() == SyntaxKind::MethodDeclaration
    }

    // Go: transformers/tstransforms/metadata.go:382 MetadataTransformer.shouldAddParamTypesMetadata
    /// Determines whether to emit the "design:paramtypes" metadata based on the node's kind.
    /// The caller should have already tested whether the node has decorators and whether the
    /// emitDecoratorMetadata compiler option is set.
    ///
    /// @param node The node to test.
    fn should_add_param_types_metadata(&self, node: Node) -> bool {
        match node.kind() {
            SyntaxKind::ClassDeclaration | SyntaxKind::ClassExpression => {
                get_first_constructor_with_body(node).is_some()
            }
            SyntaxKind::MethodDeclaration | SyntaxKind::GetAccessor | SyntaxKind::SetAccessor => {
                true
            }
            _ => false,
        }
    }
}
