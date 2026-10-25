//! Port of `transformers/tstransforms/runtimesyntax.go`.
//!
//! Transforms TypeScript-specific runtime syntax (enums, namespaces,
//! parameter properties, `import x = N.y`) into JavaScript-compatible syntax.

// !!! SourceMaps and Comments need to be validated

use super::TxVisit;
use super::utilities::constant_expression;
use crate::ast::visitor::NodeVisitor;
use crate::prelude::*;
use crate::printer::factory::{AssignedNameOptions, NameOptions};
use crate::transformers::destructuring::{FlattenLevel, flatten_destructuring_assignment};
use crate::transformers::modifier_visitor::extract_modifiers;
use crate::transformers::transformer::{
    TransformOptions, TransformReferenceResolver, Transformer, TransformerBox, TransformerVisit,
};
use crate::transformers::utilities::{
    convert_variable_declaration_to_assignment_expression, find_super_statement_index_path,
    is_generated_identifier, is_identifier_reference, is_local_name,
};

/// Go `map[string]*ast.Node` held by pointer: saved and restored by
/// reference, and `nil` until the first record.
type FirstDeclarationsOfName = Option<Rc<RefCell<FxHashMap<String, Node>>>>;

// Go: transformers/tstransforms/runtimesyntax.go:17 RuntimeSyntaxTransformer
/// Transforms TypeScript-specific runtime syntax into JavaScript-compatible syntax.
pub struct RuntimeSyntaxTransformer {
    emit_context: Rc<EmitContext>,
    compiler_options: &'static CompilerOptions,
    parent_node: Node,
    current_node: Node,
    current_source_file: Node,
    current_scope: Node, // SourceFile | Block | ModuleBlock | CaseBlock
    current_scope_first_declarations_of_name: FirstDeclarationsOfName,
    current_enum: Node,
    current_namespace: Node,
    resolver: Rc<dyn TransformReferenceResolver>,
    emit_resolver: Rc<dyn EmitResolver>,
}

// Go: transformers/tstransforms/runtimesyntax.go:31 NewRuntimeSyntaxTransformer
// PORT: Go never returns nil here. The result is an `Option` so the
// constructor has the `TransformerFactory` shape.
pub fn new_runtime_syntax_transformer(opt: &TransformOptions) -> Option<TransformerBox> {
    let compiler_options = opt.compiler_options;
    let emit_context = opt.context.clone();
    let tx = RuntimeSyntaxTransformer {
        emit_context,
        compiler_options,
        parent_node: Node::NIL,
        current_node: Node::NIL,
        current_source_file: Node::NIL,
        current_scope: Node::NIL,
        current_scope_first_declarations_of_name: None,
        current_enum: Node::NIL,
        current_namespace: Node::NIL,
        resolver: opt.resolver.clone(),
        emit_resolver: opt.emit_resolver.clone(),
    };
    Some(Box::new(tx))
}

impl Transformer for RuntimeSyntaxTransformer {
    fn emit_context(&self) -> &Rc<EmitContext> {
        &self.emit_context
    }

    fn transform_source_file(&mut self, file: Node) -> Node {
        self.visit_source_file_root(file)
    }
}

impl TransformerVisit for RuntimeSyntaxTransformer {
    fn emit_context_rc(&self) -> Rc<EmitContext> {
        self.emit_context.clone()
    }

    // Go: transformers/tstransforms/runtimesyntax.go:79 RuntimeSyntaxTransformer.visit
    /// Visits each node in the AST
    fn visit(&mut self, node: Node) -> Node {
        let grandparent_node = self.push_node(node);
        let (saved_current_scope, saved_current_scope_first_declarations_of_name) =
            self.push_scope(node);

        let result = self.visit_worker(node);

        // PORT: Go pops both with `defer`, so `popScope` runs first.
        self.pop_scope(
            saved_current_scope,
            saved_current_scope_first_declarations_of_name,
        );
        self.pop_node(grandparent_node);
        result
    }
}

impl RuntimeSyntaxTransformer {
    // Go: transformers/tstransforms/runtimesyntax.go:39 RuntimeSyntaxTransformer.pushNode
    /// Pushes a new child node onto the ancestor tracking stack, returning the grandparent node to be restored later via `popNode`.
    fn push_node(&mut self, node: Node) -> Node {
        let grandparent_node = self.parent_node;
        self.parent_node = self.current_node;
        self.current_node = node;
        grandparent_node
    }

    // Go: transformers/tstransforms/runtimesyntax.go:47 RuntimeSyntaxTransformer.popNode
    /// Pops the last child node off the ancestor tracking stack, restoring the grandparent node.
    fn pop_node(&mut self, grandparent_node: Node) {
        self.current_node = self.parent_node;
        self.parent_node = grandparent_node;
    }

    // Go: transformers/tstransforms/runtimesyntax.go:52 RuntimeSyntaxTransformer.pushScope
    fn push_scope(&mut self, node: Node) -> (Node, FirstDeclarationsOfName) {
        let saved_current_scope = self.current_scope;
        let saved_current_scope_first_declarations_of_name =
            self.current_scope_first_declarations_of_name.clone();
        match node.kind() {
            SyntaxKind::SourceFile => {
                self.current_scope = node;
                self.current_source_file = node;
                self.current_scope_first_declarations_of_name = None;
            }
            SyntaxKind::CaseBlock | SyntaxKind::ModuleBlock | SyntaxKind::Block => {
                self.current_scope = node;
                self.current_scope_first_declarations_of_name = None;
            }
            SyntaxKind::FunctionDeclaration
            | SyntaxKind::ClassDeclaration
            | SyntaxKind::VariableStatement => {
                self.record_declaration_in_scope(node);
            }
            _ => {}
        }
        (
            saved_current_scope,
            saved_current_scope_first_declarations_of_name,
        )
    }

    // Go: transformers/tstransforms/runtimesyntax.go:69 RuntimeSyntaxTransformer.popScope
    fn pop_scope(
        &mut self,
        saved_current_scope: Node,
        saved_current_scope_first_declarations_of_name: FirstDeclarationsOfName,
    ) {
        if self.current_scope != saved_current_scope {
            // only reset the first declaration for a name if we are exiting the scope in which it was declared
            self.current_scope_first_declarations_of_name =
                saved_current_scope_first_declarations_of_name;
        }

        self.current_scope = saved_current_scope;
    }

    /// The body of Go `visit` after the pushes (Go pops with `defer`).
    fn visit_worker(&mut self, node: Node) -> Node {
        let facts = node.subtree_facts();
        if !facts.intersects(SubtreeFacts::SUBTREE_CONTAINS_TYPE_SCRIPT)
            && (self.current_namespace.is_nil() && self.current_enum.is_nil()
                || !facts.intersects(SubtreeFacts::SUBTREE_CONTAINS_IDENTIFIER))
        {
            return node;
        }

        match node.kind() {
            // TypeScript parameter property modifiers are elided
            SyntaxKind::PublicKeyword
            | SyntaxKind::PrivateKeyword
            | SyntaxKind::ProtectedKeyword
            | SyntaxKind::ReadonlyKeyword
            | SyntaxKind::OverrideKeyword => Node::NIL,
            SyntaxKind::EnumDeclaration => self.visit_enum_declaration(node),
            SyntaxKind::ModuleDeclaration => self.visit_module_declaration(node),
            SyntaxKind::ClassDeclaration => self.visit_class_declaration(node),
            SyntaxKind::ClassExpression => self.visit_class_expression(node),
            SyntaxKind::Constructor => self.visit_constructor_declaration(node),
            SyntaxKind::FunctionDeclaration => self.visit_function_declaration(node),
            SyntaxKind::VariableStatement => self.visit_variable_statement(node),
            SyntaxKind::ExportDeclaration
            | SyntaxKind::ImportDeclaration
            | SyntaxKind::ImportClause => {
                if self.current_namespace.is_some()
                    && self.current_scope.is_some()
                    && self.current_scope.kind() != SyntaxKind::Block
                {
                    // do not emit ES6 imports and exports since they are illegal inside a namespace
                    Node::NIL
                } else {
                    self.visit_each_child(node)
                }
            }
            SyntaxKind::ImportEqualsDeclaration => {
                if self.current_namespace.is_some()
                    && self.current_scope.is_some()
                    && self.current_scope.kind() != SyntaxKind::Block
                    && node.module_reference().kind() == SyntaxKind::ExternalModuleReference
                {
                    // do not emit ES6 imports and exports since they are illegal inside a namespace
                    Node::NIL
                } else if self.current_namespace.is_some()
                    && self.current_scope.is_some()
                    && self.current_scope.kind() == SyntaxKind::Block
                    && node.module_reference().kind() != SyntaxKind::ExternalModuleReference
                {
                    // inside a block within a namespace, elide internal import aliases
                    Node::NIL
                } else {
                    self.visit_import_equals_declaration(node)
                }
            }
            SyntaxKind::Identifier => self.visit_identifier(node),
            SyntaxKind::ShorthandPropertyAssignment => {
                self.visit_shorthand_property_assignment(node)
            }
            _ => self.visit_each_child(node),
        }
    }

    // Go: transformers/tstransforms/runtimesyntax.go:140 RuntimeSyntaxTransformer.recordDeclarationInScope
    /// Records that a declaration was emitted in the current scope, if it was the first declaration for the provided symbol.
    fn record_declaration_in_scope(&mut self, node: Node) {
        match node.kind() {
            SyntaxKind::VariableStatement => {
                self.record_declaration_in_scope(node.declaration_list());
                return;
            }
            SyntaxKind::VariableDeclarationList => {
                for decl in node.declarations().nodes().iter() {
                    self.record_declaration_in_scope(decl);
                }
                return;
            }
            SyntaxKind::ArrayBindingPattern | SyntaxKind::ObjectBindingPattern => {
                for element in node.elements().iter() {
                    self.record_declaration_in_scope(element);
                }
                return;
            }
            _ => {}
        }
        let name = node.name();
        if name.is_some() {
            if is_identifier(name) {
                let map = self
                    .current_scope_first_declarations_of_name
                    .get_or_insert_with(|| Rc::new(RefCell::new(FxHashMap::default())))
                    .clone();
                let text = name.text();
                map.borrow_mut().entry(text.to_string()).or_insert(node);
            } else if is_binding_pattern(name) {
                self.record_declaration_in_scope(name);
            }
        }
    }

    // Go: transformers/tstransforms/runtimesyntax.go:173 RuntimeSyntaxTransformer.isFirstDeclarationInScope
    /// Determines whether a declaration is the first declaration with the same name emitted in the current scope.
    fn is_first_declaration_in_scope(&self, node: Node) -> bool {
        let name = node.name();
        if name.is_some() && is_identifier(name) {
            let text = name.text();
            if let Some(map) = &self.current_scope_first_declarations_of_name {
                if let Some(&first_declaration) = map.borrow().get(text) {
                    return first_declaration == node;
                }
            }
        }
        false
    }

    // Go: transformers/tstransforms/runtimesyntax.go:184 RuntimeSyntaxTransformer.isExportOfNamespace
    fn is_export_of_namespace(&self, node: Node) -> bool {
        self.current_namespace.is_some()
            && (self.current_scope.is_nil() || self.current_scope.kind() != SyntaxKind::Block)
            && node.modifier_flags().intersects(ModifierFlags::EXPORT)
    }

    // Go: transformers/tstransforms/runtimesyntax.go:189 RuntimeSyntaxTransformer.getExpressionForPropertyName
    /// Gets an expression that represents a property name, such as `"foo"` for the identifier `foo`.
    fn get_expression_for_property_name(&mut self, member: Node) -> Node {
        let ec = self.emit_context.clone();
        let f = ec.factory();
        let name = member.name();
        match name.kind() {
            SyntaxKind::PrivateIdentifier => f.new_identifier(""),
            SyntaxKind::ComputedPropertyName => {
                // enums don't support computed properties so we always generate the 'expression' part of the name as-is.
                self.visit_node(name.expression())
            }
            SyntaxKind::Identifier => f.new_string_literal(name.text(), TokenFlags::NONE),
            // !!! propagate token flags (will produce new diffs)
            SyntaxKind::StringLiteral => f.new_string_literal(name.text(), TokenFlags::NONE),
            SyntaxKind::NumericLiteral => f.new_numeric_literal(name.text(), TokenFlags::NONE),
            _ => name,
        }
    }

    // Go: transformers/tstransforms/runtimesyntax.go:210 RuntimeSyntaxTransformer.getEnumQualifiedElement
    /// Gets an expression like `E["A"]` that references an enum member.
    fn get_enum_qualified_element(&mut self, enum_: Node, member: Node) -> Node {
        let container_name = self.get_namespace_container_name(enum_);
        let property_name = self.get_expression_for_property_name(member);
        let prop = self.get_namespace_qualified_element(container_name, property_name);
        self.emit_context.add_emit_flags(
            prop,
            EmitFlags::NO_COMMENTS
                | EmitFlags::NO_NESTED_COMMENTS
                | EmitFlags::NO_SOURCE_MAP
                | EmitFlags::NO_NESTED_SOURCE_MAPS,
        );
        prop
    }

    // Go: transformers/tstransforms/runtimesyntax.go:217 RuntimeSyntaxTransformer.getNamespaceContainerName
    /// Gets an expression used to refer to a namespace or enum from within the body of its declaration.
    fn get_namespace_container_name(&self, node: Node) -> Node {
        self.emit_context
            .factory()
            .new_generated_name_for_node(node)
    }

    // Go: transformers/tstransforms/runtimesyntax.go:222 RuntimeSyntaxTransformer.getNamespaceQualifiedProperty
    /// Gets an expression used to refer to an export of a namespace or a member of an enum by property name.
    fn get_namespace_qualified_property(&self, ns: Node, name: Node) -> Node {
        self.emit_context.factory().get_namespace_member_name(
            ns,
            name,
            NameOptions {
                allow_source_maps: true,
                ..Default::default()
            },
        )
    }

    // Go: transformers/tstransforms/runtimesyntax.go:227 RuntimeSyntaxTransformer.getNamespaceQualifiedElement
    /// Gets an expression used to refer to an export of a namespace or a member of an enum by indexed access.
    fn get_namespace_qualified_element(&self, ns: Node, expression: Node) -> Node {
        let ec = &self.emit_context;
        let qualified_name = ec.factory().new_element_access_expression(
            ns,
            Node::NIL, /*questionDotToken*/
            expression,
            NodeFlags::NONE,
        );
        ec.assign_comment_and_source_map_ranges(qualified_name, expression);
        qualified_name
    }

    // Go: transformers/tstransforms/runtimesyntax.go:234 RuntimeSyntaxTransformer.getExportQualifiedReferenceToDeclaration
    /// Gets an expression used within the provided node's container for any exported references.
    fn get_export_qualified_reference_to_declaration(&self, node: Node) -> Node {
        let f = self.emit_context.factory();
        if self.is_export_of_namespace(node) {
            return f.get_external_module_or_namespace_export_name(
                self.get_namespace_container_name(self.current_namespace),
                node,
                false, /*allowComments*/
                true,  /*allowSourceMaps*/
            );
        }
        f.get_declaration_name_ex(
            node,
            NameOptions {
                allow_source_maps: true,
                ..Default::default()
            },
        )
    }

    // Go: transformers/tstransforms/runtimesyntax.go:241 RuntimeSyntaxTransformer.addVarForDeclaration
    fn add_var_for_declaration(&mut self, statements: &mut Vec<Node>, node: Node) -> bool {
        self.record_declaration_in_scope(node);
        if !self.is_first_declaration_in_scope(node) {
            return false;
        }

        let ec = self.emit_context.clone();
        let f = ec.factory();

        // var name;
        let name = f.get_local_name_ex(
            node,
            AssignedNameOptions {
                allow_source_maps: true,
                ..Default::default()
            },
        );
        let var_decl = f.new_variable_declaration(name, Node::NIL, Node::NIL, Node::NIL);
        let var_flags = if self.current_scope == self.current_source_file {
            NodeFlags::NONE
        } else {
            NodeFlags::LET
        };
        let var_decls = f.new_variable_declaration_list(f.new_node_list(&[var_decl]), var_flags);
        // Replicate modifierVisitor: strip decorators, TypeScript modifiers, and export when in namespace.
        let mut modifier_mask = !(ModifierFlags::TYPE_SCRIPT_MODIFIER | ModifierFlags::DECORATOR);
        if self.current_namespace.is_some() {
            modifier_mask = modifier_mask.without(ModifierFlags::EXPORT);
        }
        let modifiers = extract_modifiers(&ec, node.modifiers(), modifier_mask);
        let var_statement = f.new_variable_statement(modifiers, var_decls);

        ec.set_original(var_decl, node);
        // !!! synthetic comments
        ec.set_original(var_statement, node);

        // Adjust the source map emit to match the old emitter.
        if is_enum_declaration(node) {
            ec.set_source_map_range(var_decls, node.loc());
        } else {
            ec.set_source_map_range(var_statement, node.loc());
        }

        // Trailing comments for enum declaration should be emitted after the function closure
        // instead of the variable statement:
        //
        //     /** Leading comment*/
        //     enum E {
        //         A
        //     } // trailing comment
        //
        // Should emit:
        //
        //     /** Leading comment*/
        //     var E;
        //     (function (E) {
        //         E[E["A"] = 0] = "A";
        //     })(E || (E = {})); // trailing comment
        //
        ec.set_comment_range(var_statement, node.loc());
        ec.add_emit_flags(var_statement, EmitFlags::NO_TRAILING_COMMENTS);
        statements.push(var_statement);

        true
    }

    // Go: transformers/tstransforms/runtimesyntax.go:294 RuntimeSyntaxTransformer.visitEnumDeclaration
    fn visit_enum_declaration(&mut self, node: Node) -> Node {
        let ec = self.emit_context.clone();
        let f = ec.factory();
        if !self.should_emit_enum_declaration(node) {
            return ec.new_not_emitted_statement(node);
        }

        let mut statements: Vec<Node> = Vec::new();

        // If needed, we should emit a variable declaration for the enum:
        //  var name;
        let var_added = self.add_var_for_declaration(&mut statements, node);

        // If we emit a leading variable declaration, we should not emit leading comments for the enum body, but we should
        // still emit the comments if we are emitting to a System module.
        let mut emit_flags = EmitFlags::NONE;
        if var_added
            && (self.compiler_options.get_emit_module_kind() != ModuleKind::SYSTEM
                || self.current_scope != self.current_source_file)
        {
            emit_flags |= EmitFlags::NO_LEADING_COMMENTS;
        }

        //  x || (x = {})
        //  exports.x || (exports.x = {})
        let mut enum_arg = f.new_logical_or_expression(
            self.get_export_qualified_reference_to_declaration(node),
            f.new_assignment_expression(
                self.get_export_qualified_reference_to_declaration(node),
                f.new_object_literal_expression(f.new_node_list(&[]), false),
            ),
        );

        if self.is_export_of_namespace(node) {
            // `localName` is the expression used within this node's containing scope for any local references.
            let local_name = f.get_local_name_ex(
                node,
                AssignedNameOptions {
                    allow_source_maps: true,
                    ..Default::default()
                },
            );

            //  x = (exports.x || (exports.x = {}))
            enum_arg = f.new_assignment_expression(local_name, enum_arg);
        }

        // (function (name) { ... })(name || (name = {}))
        let enum_param_name = f.new_generated_name_for_node(node);
        ec.set_source_map_range(enum_param_name, node.name().loc());

        let enum_param = f.new_parameter_declaration(
            ModifierList::NIL,
            Node::NIL,
            enum_param_name,
            Node::NIL,
            Node::NIL,
            Node::NIL,
        );
        let enum_body = self.transform_enum_body(node);
        let enum_func = f.new_function_expression(
            ModifierList::NIL,
            Node::NIL,
            Node::NIL,
            NodeList::NIL,
            f.new_node_list(&[enum_param]),
            Node::NIL,
            Node::NIL,
            enum_body,
        );
        let enum_call = f.new_call_expression(
            f.new_parenthesized_expression(enum_func),
            Node::NIL,
            NodeList::NIL,
            f.new_node_list(&[enum_arg]),
            NodeFlags::NONE,
        );
        let enum_statement = f.new_expression_statement(enum_call);
        ec.set_original(enum_statement, node);
        ec.assign_comment_and_source_map_ranges(enum_statement, node);
        ec.add_emit_flags(enum_statement, emit_flags);
        statements.push(enum_statement);
        f.new_syntax_list(&statements)
    }

    // Go: transformers/tstransforms/runtimesyntax.go:346 RuntimeSyntaxTransformer.transformEnumBody
    /// Transforms the body of an enum declaration.
    fn transform_enum_body(&mut self, node: Node) -> Node {
        let saved_current_enum = self.current_enum;
        self.current_enum = node;

        // visit the children of `node` in advance to capture any references to enum members
        let node = self.visit_each_child(node);

        let mut statements: Vec<Node> = Vec::new();
        for i in 0..node.member_list().nodes().len() {
            //  E[E["A"] = 0] = "A";
            self.transform_enum_member(&mut statements, node, i);
        }

        let ec = self.emit_context.clone();
        let f = ec.factory();
        let statement_list = f.new_node_list_with_loc(&statements, node.member_list().loc());

        self.current_enum = saved_current_enum;
        f.new_block(statement_list, true /*multiline*/)
    }

    // Go: transformers/tstransforms/runtimesyntax.go:371 RuntimeSyntaxTransformer.transformEnumMember
    /// Transforms an enum member into a statement. It is expected that `enum` has already been visited.
    fn transform_enum_member(&mut self, statements: &mut Vec<Node>, enum_: Node, index: usize) {
        let ec = self.emit_context.clone();
        let f = ec.factory();
        let member = enum_.member_list().nodes().get(index);

        let saved_parent = self.parent_node;
        self.parent_node = self.current_node;
        self.current_node = member;

        //  E[E["A"] = x] = "A";
        //             ^
        let mut expression = member.initializer(); // NOTE: already visited

        let use_explicit_reverse_mapping;

        let parse_node = ec.parse_node(member);
        let result = self.emit_resolver.get_enum_member_value(parse_node);
        match &result.value {
            Some(value @ LiteralValue::Number(_)) => {
                expression = coalesce(constant_expression(value, f), expression);
                use_explicit_reverse_mapping = true;
            }
            Some(value @ LiteralValue::String(_)) => {
                expression = coalesce(constant_expression(value, f), expression);
                use_explicit_reverse_mapping = false;
            }
            _ => {
                if expression.is_nil() {
                    expression = f.new_void_zero_expression();
                }
                use_explicit_reverse_mapping = !result.is_syntactically_string;
            }
        }

        // Define the enum member property:
        //  E[E["A"] = 0] = "A";
        //    ^^^^^^^^--_____
        expression =
            f.new_assignment_expression(self.get_enum_qualified_element(enum_, member), expression);

        if use_explicit_reverse_mapping {
            //  E[E["A"] = 0] = "A";
            //  ^^--------------^^^^^
            expression = f.new_assignment_expression(
                f.new_element_access_expression(
                    self.get_namespace_container_name(enum_),
                    Node::NIL, /*questionDotToken*/
                    expression,
                    NodeFlags::NONE,
                ),
                self.get_expression_for_property_name(member),
            );
        }

        let member_statement = f.new_expression_statement(expression);
        ec.assign_comment_and_source_map_ranges(expression, member);
        ec.assign_comment_and_source_map_ranges(member_statement, member);
        statements.push(member_statement);

        self.current_node = self.parent_node;
        self.parent_node = saved_parent;
    }

    // Go: transformers/tstransforms/runtimesyntax.go:436 RuntimeSyntaxTransformer.visitModuleDeclaration
    fn visit_module_declaration(&mut self, node: Node) -> Node {
        let ec = self.emit_context.clone();
        let f = ec.factory();
        if !self.should_emit_module_declaration(node) {
            return ec.new_not_emitted_statement(node);
        }

        let mut statements: Vec<Node> = Vec::new();

        // If needed, we should emit a variable declaration for the module:
        //  var name;
        let var_added = self.add_var_for_declaration(&mut statements, node);

        // If we emit a leading variable declaration, we should not emit leading comments for the module body, but we should
        // still emit the comments if we are emitting to a System module.
        let mut emit_flags = EmitFlags::NONE;
        if var_added
            && (self.compiler_options.get_emit_module_kind() != ModuleKind::SYSTEM
                || self.current_scope != self.current_source_file)
        {
            emit_flags |= EmitFlags::NO_LEADING_COMMENTS;
        }

        //  x || (x = {})
        //  exports.x || (exports.x = {})
        let mut module_arg = f.new_logical_or_expression(
            self.get_export_qualified_reference_to_declaration(node),
            f.new_assignment_expression(
                self.get_export_qualified_reference_to_declaration(node),
                f.new_object_literal_expression(f.new_node_list(&[]), false),
            ),
        );

        if self.is_export_of_namespace(node) {
            // `localName` is the expression used within this node's containing scope for any local references.
            let local_name = f.get_local_name_ex(
                node,
                AssignedNameOptions {
                    allow_source_maps: true,
                    ..Default::default()
                },
            );

            //  x = (exports.x || (exports.x = {}))
            module_arg = f.new_assignment_expression(local_name, module_arg);
        }

        // (function (name) { ... })(name || (name = {}))
        let module_param_name = f.new_generated_name_for_node(node);
        ec.set_source_map_range(module_param_name, node.name().loc());

        let module_param = f.new_parameter_declaration(
            ModifierList::NIL,
            Node::NIL,
            module_param_name,
            Node::NIL,
            Node::NIL,
            Node::NIL,
        );
        let namespace_local_name = self.get_namespace_container_name(node);
        let module_body = self.transform_module_body(node, namespace_local_name);
        let module_func = f.new_function_expression(
            ModifierList::NIL,
            Node::NIL,
            Node::NIL,
            NodeList::NIL,
            f.new_node_list(&[module_param]),
            Node::NIL,
            Node::NIL,
            module_body,
        );
        let module_call = f.new_call_expression(
            f.new_parenthesized_expression(module_func),
            Node::NIL,
            NodeList::NIL,
            f.new_node_list(&[module_arg]),
            NodeFlags::NONE,
        );
        let module_statement = f.new_expression_statement(module_call);
        ec.set_original(module_statement, node);
        ec.assign_comment_and_source_map_ranges(module_statement, node);
        ec.add_emit_flags(module_statement, emit_flags);
        statements.push(module_statement);
        f.new_syntax_list(&statements)
    }

    // Go: transformers/tstransforms/runtimesyntax.go:487 RuntimeSyntaxTransformer.transformModuleBody
    fn transform_module_body(&mut self, node: Node, _namespace_local_name: Node) -> Node {
        let ec = self.emit_context.clone();
        let f = ec.factory();
        let saved_current_namespace = self.current_namespace;
        let saved_current_scope = self.current_scope;
        let saved_current_scope_first_declarations_of_name =
            self.current_scope_first_declarations_of_name.clone();

        self.current_namespace = node;
        self.current_scope_first_declarations_of_name = None;

        let mut statements: Vec<Node> = Vec::new();
        ec.start_variable_environment();

        let mut statements_location = TextRange::default();
        let mut block_location = TextRange::default();
        let mut node = node;
        if node.body().is_some() {
            if node.body().kind() == SyntaxKind::ModuleBlock {
                // visit the children of `node` in advance to capture any references to namespace members
                node = self.visit_each_child(node);
                let body = node.body();
                statements = body.statements().to_vec();
                statements_location = body.statement_list().loc();
                block_location = body.loc();
            } else {
                // node.Body.Kind == ast.KindModuleDeclaration
                // !!! Strada didn't do this; why?
                // tx.currentScope = node.AsNode()
                let (visited, _) = self.visit_slice(&[node.body()]);
                statements = visited;
                let module_block = get_innermost_module_declaration_from_dotted_module(node).body();
                // Go: ast/ast_generated.go:12468 (*Node).AsModuleBlock. The
                // type eraser can elide an inner part of a dotted name to a
                // NotEmittedStatement (typeeraser.go:124 calls
                // IsInstantiatedModule for each part), and Go panics here.
                if module_block.kind() != SyntaxKind::ModuleBlock {
                    crate::core::go_panic("Invalid node cast to ModuleBlock".to_string());
                }
                statements_location = module_block.statement_list().loc().with_pos(-1);
            }
        }

        self.current_namespace = saved_current_namespace;
        self.current_scope = saved_current_scope;
        self.current_scope_first_declarations_of_name =
            saved_current_scope_first_declarations_of_name;

        let statements = ec.end_and_merge_variable_environment(&statements);
        let statement_list = f.new_node_list_with_loc(&statements, statements_location);
        let block = f.new_block(statement_list, true /*multiline*/);
        set_node_loc(block, block_location);

        //  namespace hello.hi.world {
        //       function foo() {}
        //
        //       // TODO, blah
        //  }
        //
        // should be emitted as
        //
        //  var hello;
        //  (function (hello) {
        //      var hi;
        //      (function (hi) {
        //          var world;
        //          (function (world) {
        //              function foo() { }
        //              // TODO, blah
        //          })(world = hi.world || (hi.world = {}));
        //      })(hi = hello.hi || (hello.hi = {}));
        //  })(hello || (hello = {}));
        //
        // We only want to emit comment on the namespace which contains block body itself, not the containing namespaces.
        if node.body().is_nil() || node.body().kind() != SyntaxKind::ModuleBlock {
            ec.add_emit_flags(block, EmitFlags::NO_COMMENTS);
        }
        block
    }

    // Go: transformers/tstransforms/runtimesyntax.go:554 RuntimeSyntaxTransformer.visitImportEqualsDeclaration
    fn visit_import_equals_declaration(&mut self, node: Node) -> Node {
        if node.module_reference().kind() == SyntaxKind::ExternalModuleReference {
            return self.visit_each_child(node);
        }

        let ec = self.emit_context.clone();
        let f = ec.factory();
        let module_reference = f.create_expression_from_entity_name(node.module_reference());
        ec.set_emit_flags(
            module_reference,
            EmitFlags::NO_COMMENTS | EmitFlags::NO_NESTED_COMMENTS,
        );
        if !self.is_export_of_namespace(node) {
            //  export var ${name} = ${moduleReference};
            //  var ${name} = ${moduleReference};
            let var_decl = f.new_variable_declaration(
                node.name(),
                Node::NIL, /*exclamationToken*/
                Node::NIL, /*type*/
                module_reference,
            );
            ec.set_original(var_decl, node);
            let var_list =
                f.new_variable_declaration_list(f.new_node_list(&[var_decl]), NodeFlags::NONE);
            let var_modifiers = extract_modifiers(&ec, node.modifiers(), ModifierFlags::EXPORT);
            let var_statement = f.new_variable_statement(var_modifiers, var_list);
            ec.set_original(var_statement, node);
            ec.assign_comment_and_source_map_ranges(var_statement, node);
            var_statement
        } else {
            // exports.${name} = ${moduleReference};
            let statement = self.create_export_statement(
                node.name(),
                module_reference,
                node.loc(),
                node.loc(),
                node,
            );
            set_node_loc(statement, node.loc());
            statement
        }
    }

    // Go: transformers/tstransforms/runtimesyntax.go:580 RuntimeSyntaxTransformer.visitVariableStatement
    fn visit_variable_statement(&mut self, node: Node) -> Node {
        if self.is_export_of_namespace(node) {
            let ec = self.emit_context.clone();
            let f = ec.factory();
            let mut expressions: Vec<Node> = Vec::new();
            for declaration in node.declaration_list().declarations().nodes().iter() {
                if declaration.initializer().is_nil() {
                    continue;
                }
                if is_binding_pattern(declaration.name()) {
                    let flatten_context = ec.clone();
                    let expression = self.with_visitor(|v| {
                        let visited = v.visit_node(declaration);
                        flatten_destructuring_assignment(
                            &flatten_context,
                            v,
                            visited,
                            false, /*needsValue*/
                            FlattenLevel::All,
                            Some(&mut |v: &mut NodeVisitor<'_, &mut Self>,
                                       export_name: Node,
                                       export_value: Node,
                                       location: Option<TextRange>| {
                                v.ctx.create_namespace_export_expression(
                                    export_name,
                                    export_value,
                                    location,
                                )
                            }),
                        )
                    });
                    if expression.is_some() {
                        expressions.push(expression);
                    }
                } else {
                    let expression =
                        convert_variable_declaration_to_assignment_expression(&ec, declaration);
                    if expression.is_some() {
                        expressions.push(expression);
                    }
                }
            }
            if expressions.is_empty() {
                return Node::NIL;
            }
            let expression = f.inline_expressions(&expressions);
            let statement = f.new_expression_statement(expression);
            ec.set_original(statement, node);
            ec.assign_comment_and_source_map_ranges(statement, node);

            // re-visit as the new node
            let saved_current = self.current_node;
            self.current_node = statement;
            let statement = self.visit_each_child(statement);
            self.current_node = saved_current;
            return statement;
        }
        self.visit_each_child(node)
    }

    // Go: transformers/tstransforms/runtimesyntax.go:626 RuntimeSyntaxTransformer.createNamespaceExportExpression
    /// createNamespaceExportExpression creates an assignment to a namespace member for use as a
    /// callback during destructuring flattening.
    fn create_namespace_export_expression(
        &self,
        export_name: Node,
        export_value: Node,
        location: Option<TextRange>,
    ) -> Node {
        let member_name = self.get_namespace_qualified_property(
            self.get_namespace_container_name(self.current_namespace),
            export_name,
        );
        let expression = self
            .emit_context
            .factory()
            .new_assignment_expression(member_name, export_value);
        if let Some(location) = location {
            set_node_loc(expression, location);
        }
        expression
    }

    // Go: transformers/tstransforms/runtimesyntax.go:635 RuntimeSyntaxTransformer.visitFunctionDeclaration
    fn visit_function_declaration(&mut self, node: Node) -> Node {
        if self.is_export_of_namespace(node) {
            let ec = self.emit_context.clone();
            let f = ec.factory();
            let modifiers = self.visit_modifiers(extract_modifiers(
                &ec,
                node.modifiers(),
                !ModifierFlags::EXPORT,
            ));
            let name = self.visit_node(node.name());
            let parameters = self.visit_nodes(node.parameter_list());
            let body = self.visit_node(node.body());
            let updated = f.update_function_declaration(
                node,
                modifiers,
                node.asterisk_token(),
                name,
                NodeList::NIL, /*typeParameters*/
                parameters,
                Node::NIL, /*returnType*/
                Node::NIL, /*fullSignature*/
                body,
            );
            let export = self.create_export_statement_for_declaration(node);
            if export.is_some() {
                return f.new_syntax_list(&[updated, export]);
            }
            return updated;
        }
        self.visit_each_child(node)
    }

    // Go: transformers/tstransforms/runtimesyntax.go:657 RuntimeSyntaxTransformer.getParameterProperties
    fn get_parameter_properties(&self, constructor: Node) -> Vec<Node> {
        let mut parameter_properties = Vec::new();
        if constructor.is_some() {
            for parameter in constructor.parameters().iter() {
                if is_parameter_property_declaration(parameter, constructor) {
                    parameter_properties.push(parameter);
                }
            }
        }
        parameter_properties
    }

    /// The shared parameter property members of Go `visitClassDeclaration`
    /// and `visitClassExpression`.
    fn add_parameter_property_members(&self, node: Node, members: NodeList) -> NodeList {
        let ec = &self.emit_context;
        let f = ec.factory();
        let constructor = node
            .members()
            .iter()
            .find(|&member| is_constructor_declaration(member))
            .unwrap_or(Node::NIL);
        let parameter_properties = self.get_parameter_properties(constructor);

        let mut members = members;
        if !parameter_properties.is_empty() {
            let mut new_members: Vec<Node> = Vec::new();
            for parameter in parameter_properties {
                if is_identifier(parameter.name()) {
                    let parameter_property = f.new_property_declaration(
                        ModifierList::NIL, /*modifiers*/
                        f.clone_node(parameter.name()),
                        Node::NIL, /*questionOrExclamationToken*/
                        Node::NIL, /*type*/
                        Node::NIL, /*initializer*/
                    );
                    ec.set_original(parameter_property, parameter);
                    new_members.push(parameter_property);
                }
            }
            if !new_members.is_empty() {
                new_members.extend(members.nodes().iter());
                members = f.new_node_list_with_loc(&new_members, node.member_list().loc());
            }
        }
        members
    }

    // Go: transformers/tstransforms/runtimesyntax.go:669 RuntimeSyntaxTransformer.visitClassDeclaration
    fn visit_class_declaration(&mut self, node: Node) -> Node {
        let ec = self.emit_context.clone();
        let f = ec.factory();
        let exported = self.is_export_of_namespace(node);
        let modifiers = if exported {
            self.visit_modifiers(extract_modifiers(
                &ec,
                node.modifiers(),
                !ModifierFlags::EXPORT_DEFAULT,
            ))
        } else {
            self.visit_modifiers(node.modifiers())
        };

        let mut name = self.visit_node(node.name());
        if name.is_nil()
            && (exported
                || child_is_decorated(
                    self.compiler_options.experimental_decorators.is_true(),
                    node,
                    Node::NIL,
                )
                || (node.modifier_flags() & ModifierFlags::EXPORT_DEFAULT
                    != ModifierFlags::EXPORT_DEFAULT
                    && node_is_decorated(
                        self.compiler_options.experimental_decorators.is_true(),
                        node,
                        Node::NIL,
                        Node::NIL,
                    )))
        {
            name = f.new_generated_name_for_node(node);
        }
        let heritage_clauses = self.visit_nodes(node.heritage_clauses());
        let members = self.visit_nodes(node.member_list());
        // PORT: Go inlines the parameter property loop here and in `visitClassExpression`.
        let members = self.add_parameter_property_members(node, members);

        let updated = f.update_class_declaration(
            node,
            modifiers,
            name,
            NodeList::NIL, /*typeParameters*/
            heritage_clauses,
            members,
        );
        if exported {
            let export = self.create_export_statement_for_declaration(node);
            if export.is_some() {
                return f.new_syntax_list(&[updated, export]);
            }
        }
        updated
    }

    // Go: transformers/tstransforms/runtimesyntax.go:721 RuntimeSyntaxTransformer.visitClassExpression
    fn visit_class_expression(&mut self, node: Node) -> Node {
        let ec = self.emit_context.clone();
        let f = ec.factory();
        let modifiers = self.visit_modifiers(extract_modifiers(
            &ec,
            node.modifiers(),
            !ModifierFlags::EXPORT_DEFAULT,
        ));
        let name = self.visit_node(node.name());
        let heritage_clauses = self.visit_nodes(node.heritage_clauses());
        let members = self.visit_nodes(node.member_list());
        let members = self.add_parameter_property_members(node, members);

        f.update_class_expression(
            node,
            modifiers,
            name,
            NodeList::NIL, /*typeParameters*/
            heritage_clauses,
            members,
        )
    }

    // Go: transformers/tstransforms/runtimesyntax.go:753 RuntimeSyntaxTransformer.visitConstructorDeclaration
    fn visit_constructor_declaration(&mut self, node: Node) -> Node {
        let modifiers = self.visit_modifiers(node.modifiers());
        let parameters = self.visit_parameters(node.parameter_list());
        let body = self.visit_constructor_body(node.body(), node);
        self.emit_context.factory().update_constructor_declaration(
            node,
            modifiers,
            NodeList::NIL, /*typeParameters*/
            parameters,
            Node::NIL, /*returnType*/
            Node::NIL, /*fullSignature*/
            body,
        )
    }

    // Go: transformers/tstransforms/runtimesyntax.go:760 RuntimeSyntaxTransformer.visitConstructorBody
    fn visit_constructor_body(&mut self, body: Node, constructor: Node) -> Node {
        let parameter_properties = self.get_parameter_properties(constructor);
        if parameter_properties.is_empty() {
            return self.visit_function_body(body);
        }

        let ec = self.emit_context.clone();
        let f = ec.factory();

        let grandparent_of_body = self.push_node(body);
        let (saved_current_scope, saved_current_scope_first_declarations_of_name) =
            self.push_scope(body);

        ec.start_variable_environment();
        let body_statements = body.statements().to_vec();
        let (prologue, rest) = f.split_standard_prologue(&body_statements);
        let mut statements: Vec<Node> = prologue.to_vec();

        // Transform parameters into property assignments. Transforms this:
        //
        //  constructor (public x, public y) {
        //  }
        //
        // Into this:
        //
        //  constructor (x, y) {
        //      this.x = x;
        //      this.y = y;
        //  }
        //

        let mut parameter_property_assignments: Vec<Node> = Vec::new();
        for parameter in parameter_properties {
            if is_identifier(parameter.name()) {
                let property_name = f.clone_node(parameter.name());
                // .Parent set to get node to printback using text from original file instead of processed text; TODO: this should be achievable via EmitFlags instead
                set_node_parent(property_name, parameter.name().parent());
                ec.add_emit_flags(
                    property_name,
                    EmitFlags::NO_COMMENTS | EmitFlags::NO_SOURCE_MAP,
                );

                let local_name = f.clone_node(parameter.name());
                // .Parent set to get node to printback using text from original file instead of processed text; TODO: this should be achievable via EmitFlags instead
                set_node_parent(local_name, parameter.name().parent());
                ec.add_emit_flags(local_name, EmitFlags::NO_COMMENTS);

                let parameter_property = f.new_expression_statement(f.new_assignment_expression(
                    f.new_property_access_expression(
                        f.new_this_expression(),
                        Node::NIL, /*questionDotToken*/
                        property_name,
                        NodeFlags::NONE,
                    ),
                    local_name,
                ));
                ec.set_original(parameter_property, parameter);
                ec.add_emit_flags(parameter_property, EmitFlags::START_ON_NEW_LINE);
                parameter_property_assignments.push(parameter_property);
            }
        }

        let super_path = find_super_statement_index_path(rest, 0);

        if !super_path.is_empty() {
            let transformed = self.transform_constructor_body_worker(
                rest,
                &super_path,
                &parameter_property_assignments,
            );
            statements.extend(transformed);
        } else {
            statements.extend(parameter_property_assignments);
            let (visited, _) = self.visit_slice(rest);
            statements.extend(visited);
        }

        let statements = ec.end_and_merge_variable_environment(&statements);
        let statement_list = f.new_node_list_with_loc(&statements, body.statement_list().loc());

        self.pop_scope(
            saved_current_scope,
            saved_current_scope_first_declarations_of_name,
        );
        self.pop_node(grandparent_of_body);
        let updated = f.new_block(statement_list /*multiline*/, true);
        ec.set_original(updated, body);
        set_node_loc(updated, body.loc());
        updated
    }

    // Go: transformers/tstransforms/runtimesyntax.go:835 RuntimeSyntaxTransformer.transformConstructorBodyWorker
    fn transform_constructor_body_worker(
        &mut self,
        statements_in: &[Node],
        super_path: &[usize],
        initializer_statements: &[Node],
    ) -> Vec<Node> {
        let ec = self.emit_context.clone();
        let f = ec.factory();
        let mut statements_out: Vec<Node> = Vec::new();
        let super_statement_index = super_path[0];
        let super_statement = statements_in[super_statement_index];

        // visit up to the statement containing `super`
        let (visited, _) = self.visit_slice(&statements_in[..super_statement_index]);
        statements_out.extend(visited);

        // if the statement containing `super` is a `try` statement, transform the body of the `try` block
        if is_try_statement(super_statement) {
            let try_statement = super_statement;
            let try_block = try_statement.try_block();

            // keep track of hierarchy as we descend
            let grandparent_of_try_statement = self.push_node(try_statement);
            let grandparent_of_try_block = self.push_node(try_block);
            let (saved_current_scope, saved_current_scope_first_declarations_of_name) =
                self.push_scope(try_block);

            // visit the `try` block
            let try_block_statements_in = try_block.statements().to_vec();
            let try_block_statements = self.transform_constructor_body_worker(
                &try_block_statements_in,
                &super_path[1..],
                initializer_statements,
            );

            // restore hierarchy as we ascend to the `try` statement
            self.pop_scope(
                saved_current_scope,
                saved_current_scope_first_declarations_of_name,
            );
            self.pop_node(grandparent_of_try_block);

            let try_block_statement_list =
                f.new_node_list_with_loc(&try_block_statements, try_block.statement_list().loc());
            let updated_try_block =
                f.update_block(try_block, try_block_statement_list, try_block.multi_line());
            let catch_clause = self.visit_node(try_statement.catch_clause());
            let finally_block = self.visit_node(try_statement.finally_block());
            statements_out.push(f.update_try_statement(
                try_statement,
                updated_try_block,
                catch_clause,
                finally_block,
            ));

            // restore hierarchy as we ascend to the parent of the `try` statement
            self.pop_node(grandparent_of_try_statement);
        } else {
            // visit the statement containing `super`
            let (visited, _) =
                self.visit_slice(&statements_in[super_statement_index..super_statement_index + 1]);
            statements_out.extend(visited);

            // insert the initializer statements
            statements_out.extend_from_slice(initializer_statements);
        }

        // visit the statements after `super`
        let (visited, _) = self.visit_slice(&statements_in[super_statement_index + 1..]);
        statements_out.extend(visited);
        statements_out
    }

    // Go: transformers/tstransforms/runtimesyntax.go:888 RuntimeSyntaxTransformer.visitShorthandPropertyAssignment
    fn visit_shorthand_property_assignment(&mut self, node: Node) -> Node {
        let ec = self.emit_context.clone();
        let f = ec.factory();
        let name = node.name();
        let exported_or_imported_name = self.visit_expression_identifier(name);
        if exported_or_imported_name != name {
            let mut expression = exported_or_imported_name;
            if node.object_assignment_initializer().is_some() {
                let mut equals_token = node.equals_token();
                if equals_token.is_nil() {
                    equals_token = f.new_token(SyntaxKind::EqualsToken);
                }
                let right = self.visit_node(node.object_assignment_initializer());
                expression = f.new_binary_expression(
                    ModifierList::NIL, /*modifiers*/
                    expression,
                    Node::NIL, /*typeNode*/
                    equals_token,
                    right,
                );
            }

            let updated = f.new_property_assignment(
                ModifierList::NIL, /*modifiers*/
                node.name(),
                Node::NIL, /*postfixToken*/
                Node::NIL, /*typeNode*/
                expression,
            );
            set_node_loc(updated, node.loc());
            ec.set_original(updated, node);
            ec.assign_comment_and_source_map_ranges(updated, node);
            return updated;
        }
        let object_assignment_initializer = self.visit_node(node.object_assignment_initializer());
        f.update_shorthand_property_assignment(
            node,
            ModifierList::NIL, /*modifiers*/
            exported_or_imported_name,
            Node::NIL, /*postfixToken*/
            Node::NIL, /*typeNode*/
            node.equals_token(),
            object_assignment_initializer,
        )
    }

    // Go: transformers/tstransforms/runtimesyntax.go:924 RuntimeSyntaxTransformer.visitIdentifier
    fn visit_identifier(&mut self, node: Node) -> Node {
        if is_identifier_reference(node, self.parent_node) {
            return self.visit_expression_identifier(node);
        }
        node
    }

    // Go: transformers/tstransforms/runtimesyntax.go:931 RuntimeSyntaxTransformer.visitExpressionIdentifier
    fn visit_expression_identifier(&mut self, node: Node) -> Node {
        let ec = self.emit_context.clone();
        if (self.current_enum.is_some() || self.current_namespace.is_some())
            && !is_generated_identifier(&ec, node)
            && !is_local_name(&ec, node)
        {
            let location = ec.most_original(node);
            let container = self
                .resolver
                .get_referenced_export_container(location, false /*prefixLocals*/);
            if container.is_some()
                && (is_enum_declaration(container) || is_module_declaration(container))
            {
                let f = ec.factory();
                let container_name = self.get_namespace_container_name(container);

                let member_name = f.clone_node(node);
                ec.set_emit_flags(
                    member_name,
                    EmitFlags::NO_COMMENTS | EmitFlags::NO_SOURCE_MAP,
                );

                let expression = f.get_namespace_member_name(
                    container_name,
                    member_name,
                    NameOptions {
                        allow_source_maps: true,
                        ..Default::default()
                    },
                );
                ec.assign_comment_and_source_map_ranges(expression, node);
                return expression;
            }
        }
        node
    }

    // Go: transformers/tstransforms/runtimesyntax.go:949 RuntimeSyntaxTransformer.createExportStatementForDeclaration
    fn create_export_statement_for_declaration(&self, node: Node) -> Node {
        let ec = &self.emit_context;
        let f = ec.factory();
        let export_name = f.get_external_module_or_namespace_export_name(
            self.get_namespace_container_name(self.current_namespace),
            node,
            false, /*allowComments*/
            true,  /*allowSourceMaps*/
        );
        let local_name = f.get_local_name(node);
        let expression = f.new_assignment_expression(export_name, local_name);
        let mut export_assignment_source_map_range = node.loc();
        if node.name().is_some() {
            export_assignment_source_map_range =
                export_assignment_source_map_range.with_pos(node.name().pos());
        }
        ec.set_source_map_range(expression, export_assignment_source_map_range);

        let statement = f.new_expression_statement(expression);
        let export_statement_source_map_range = node.loc().with_pos(-1);
        ec.set_source_map_range(statement, export_statement_source_map_range);
        statement
    }

    // Go: transformers/tstransforms/runtimesyntax.go:965 RuntimeSyntaxTransformer.createExportAssignment
    fn create_export_assignment(
        &self,
        name: Node,
        expression: Node,
        export_assignment_source_map_range: TextRange,
        original: Node,
    ) -> Node {
        let ec = &self.emit_context;
        let export_name = self.get_namespace_qualified_property(
            self.get_namespace_container_name(self.current_namespace),
            name,
        );
        let export_assignment = ec
            .factory()
            .new_assignment_expression(export_name, expression);
        ec.set_original(export_assignment, original);
        ec.set_source_map_range(export_assignment, export_assignment_source_map_range);
        export_assignment
    }

    // Go: transformers/tstransforms/runtimesyntax.go:973 RuntimeSyntaxTransformer.createExportStatement
    fn create_export_statement(
        &self,
        name: Node,
        expression: Node,
        export_assignment_source_map_range: TextRange,
        export_statement_source_map_range: TextRange,
        original: Node,
    ) -> Node {
        let ec = &self.emit_context;
        let export_statement =
            ec.factory()
                .new_expression_statement(self.create_export_assignment(
                    name,
                    expression,
                    export_assignment_source_map_range,
                    original,
                ));
        ec.set_original(export_statement, original);
        ec.set_source_map_range(export_statement, export_statement_source_map_range);
        export_statement
    }

    // Go: transformers/tstransforms/runtimesyntax.go:980 RuntimeSyntaxTransformer.shouldEmitEnumDeclaration
    fn should_emit_enum_declaration(&self, node: Node) -> bool {
        !is_enum_const(node) || self.compiler_options.should_preserve_const_enums()
    }

    // Go: transformers/tstransforms/runtimesyntax.go:984 RuntimeSyntaxTransformer.shouldEmitModuleDeclaration
    fn should_emit_module_declaration(&self, node: Node) -> bool {
        let pn = self.emit_context.parse_node(node);
        if pn.is_nil() {
            // If we can't find a parse tree node, assume the node is instantiated.
            return true;
        }
        is_instantiated_module(pn, self.compiler_options.should_preserve_const_enums())
    }
}

/// Go `core.Coalesce(a, b)` for nodes: `a` unless it is nil.
fn coalesce(a: Node, b: Node) -> Node {
    if a.is_some() { a } else { b }
}

// Go: transformers/tstransforms/runtimesyntax.go:993 getInnermostModuleDeclarationFromDottedModule
pub(crate) fn get_innermost_module_declaration_from_dotted_module(
    module_declaration: Node,
) -> Node {
    let mut module_declaration = module_declaration;
    while module_declaration.body().is_some()
        && module_declaration.body().kind() == SyntaxKind::ModuleDeclaration
    {
        module_declaration = module_declaration.body();
    }
    module_declaration
}
