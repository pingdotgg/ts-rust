//! Port of Go `printer/factory.go`: the printer `NodeFactory`, which wraps
//! `ast.NodeFactory` with `EmitContext` hooks and adds emit-time helpers.

use crate::prelude::*;

use super::emit_context::{
    AutoGenerateInfo, AutoGenerateOptions, EmitContext, next_auto_generate_id,
};
use super::helpers::*;
use super::types::{EmitFlags, GeneratedIdentifierFlags};
use super::utilities::format_generated_name;
use std::rc::Weak;

// Go: printer/factory.go:13 NodeFactory
/// Go `printer.NodeFactory`. It derefs to `ast::NodeFactory`, as Go embeds it.
// PORT: Go `EmitContext.Factory` points back to this factory, and this factory
// points to its `EmitContext`. The factory keeps a `Weak`, so the pair is no
// `Rc` cycle and a context is freed with its last `Rc` (each declaration
// diagnostic and emit makes one, so a cycle leaked one per file and program
// version). Only `new_emit_context` makes a factory, and the context owns it,
// so the context is alive while its factory is used.
pub struct NodeFactory {
    pub ast: crate::ast::NodeFactory,
    context: Weak<EmitContext>,
}

impl std::ops::Deref for NodeFactory {
    type Target = crate::ast::NodeFactory;

    fn deref(&self) -> &crate::ast::NodeFactory {
        &self.ast
    }
}

impl NodeFactory {
    // Go: printer/factory.go:18 NewNodeFactory
    // PORT: an associated fn, so the name does not clash with Go
    // `ast.NewNodeFactory`. `new_emit_context` (U3) calls it. The hooks hold a
    // `Weak` to the context, so they add no second `Rc` cycle; Go closes over
    // the pointer.
    #[must_use]
    pub fn new(context: &Rc<EmitContext>) -> Self {
        let on_create: Weak<EmitContext> = Rc::downgrade(context);
        let on_update: Weak<EmitContext> = Rc::downgrade(context);
        let on_clone: Weak<EmitContext> = Rc::downgrade(context);
        Self {
            ast: crate::ast::NodeFactory::new_with_hooks(NodeFactoryHooks {
                on_create: Some(Rc::new(move |node: Node| {
                    if let Some(c) = on_create.upgrade() {
                        c.on_create(node);
                    }
                })),
                on_update: Some(Rc::new(move |updated: Node, original: Node| {
                    if let Some(c) = on_update.upgrade() {
                        c.on_update(updated, original);
                    }
                })),
                on_clone: Some(Rc::new(move |updated: Node, original: Node| {
                    if let Some(c) = on_clone.upgrade() {
                        c.on_clone(updated, original);
                    }
                })),
            }),
            context: Rc::downgrade(context),
        }
    }

    /// Go `f.emitContext`.
    #[must_use]
    pub fn emit_context(&self) -> Rc<EmitContext> {
        self.context
            .upgrade()
            .expect("a printer factory outlived its EmitContext")
    }

    // Go: ast/ast.go:99 AsNodeFactory (promoted from the embedded ast.NodeFactory)
    #[must_use]
    pub fn as_node_factory(&self) -> &crate::ast::NodeFactory {
        &self.ast
    }

    // Go: printer/factory.go:29 newGeneratedIdentifier
    fn new_generated_identifier(
        &self,
        kind: GeneratedIdentifierFlags,
        text: &str,
        node: Node,
        options: AutoGenerateOptions,
    ) -> Node {
        let id = next_auto_generate_id();

        let mut text = text.to_string();
        if text.is_empty() {
            if node.is_nil() {
                text = format!("(auto@{})", id.0);
            } else if is_member_name(node) {
                text = node.text().to_string();
            } else {
                text = format!(
                    "(generated@{})",
                    get_node_id(
                        self.emit_context()
                            .get_node_for_generated_name_worker(node, id)
                    )
                );
            }
            text = format_generated_name(
                false, /*privateName*/
                &options.prefix,
                &text,
                &options.suffix,
            );
        }

        let name = self.new_identifier(text);
        let auto_generate = AutoGenerateInfo {
            id,
            flags: kind | options.flags.without(GeneratedIdentifierFlags::KIND_MASK),
            prefix: options.prefix.clone(),
            suffix: options.suffix,
            node,
        };
        // PORT: Go allocates the nil map here; the Rust map always exists.
        self.emit_context()
            .auto_generate
            .borrow_mut()
            .insert(name, auto_generate);
        name
    }

    // Go: printer/factory.go:62 NewTempVariable
    /// Allocates a new temp variable name, but does not record it in the environment. It is recommended to pass this to either
    /// `AddVariableDeclaration` or `AddLexicalDeclaration` to ensure it is properly tracked, if you are not otherwise handling
    /// it yourself.
    pub fn new_temp_variable(&self) -> Node {
        self.new_temp_variable_ex(AutoGenerateOptions::default())
    }

    // Go: printer/factory.go:69 NewTempVariableEx
    /// Allocates a new temp variable name, but does not record it in the environment. It is recommended to pass this to either
    /// `AddVariableDeclaration` or `AddLexicalDeclaration` to ensure it is properly tracked, if you are not otherwise handling
    /// it yourself.
    pub fn new_temp_variable_ex(&self, options: AutoGenerateOptions) -> Node {
        self.new_generated_identifier(
            GeneratedIdentifierFlags::AUTO,
            "",
            Node::NIL, /*node*/
            options,
        )
    }

    // Go: printer/factory.go:74 NewLoopVariable
    /// Allocates a new loop variable name.
    pub fn new_loop_variable(&self) -> Node {
        self.new_loop_variable_ex(AutoGenerateOptions::default())
    }

    // Go: printer/factory.go:79 NewLoopVariableEx
    /// Allocates a new loop variable name.
    pub fn new_loop_variable_ex(&self, options: AutoGenerateOptions) -> Node {
        self.new_generated_identifier(
            GeneratedIdentifierFlags::LOOP,
            "",
            Node::NIL, /*node*/
            options,
        )
    }

    // Go: printer/factory.go:84 NewUniqueName
    /// Allocates a new unique name based on the provided text.
    pub fn new_unique_name(&self, text: &str) -> Node {
        self.new_unique_name_ex(text, AutoGenerateOptions::default())
    }

    // Go: printer/factory.go:89 NewUniqueNameEx
    /// Allocates a new unique name based on the provided text.
    pub fn new_unique_name_ex(&self, text: &str, options: AutoGenerateOptions) -> Node {
        self.new_generated_identifier(
            GeneratedIdentifierFlags::UNIQUE,
            text,
            Node::NIL, /*node*/
            options,
        )
    }

    // Go: printer/factory.go:94 NewGeneratedNameForNode
    /// Allocates a new unique name based on the provided node.
    pub fn new_generated_name_for_node(&self, node: Node) -> Node {
        self.new_generated_name_for_node_ex(node, AutoGenerateOptions::default())
    }

    // Go: printer/factory.go:99 NewGeneratedNameForNodeEx
    /// Allocates a new unique name based on the provided node.
    pub fn new_generated_name_for_node_ex(
        &self,
        node: Node,
        mut options: AutoGenerateOptions,
    ) -> Node {
        if !options.prefix.is_empty() || !options.suffix.is_empty() {
            options.flags |= GeneratedIdentifierFlags::OPTIMISTIC;
        }

        self.new_generated_identifier(GeneratedIdentifierFlags::NODE, "", node, options)
    }

    // Go: printer/factory.go:107 newGeneratedPrivateIdentifier
    fn new_generated_private_identifier(
        &self,
        kind: GeneratedIdentifierFlags,
        text: &str,
        node: Node,
        options: AutoGenerateOptions,
    ) -> Node {
        let id = next_auto_generate_id();

        let mut text = text.to_string();
        if text.is_empty() {
            if node.is_nil() {
                text = format!("(auto@{})", id.0);
            } else if is_member_name(node) {
                text = node.text().to_string();
            } else {
                text = format!(
                    "(generated@{})",
                    get_node_id(
                        self.emit_context()
                            .get_node_for_generated_name_worker(node, id)
                    )
                );
            }
            text = format_generated_name(
                true, /*privateName*/
                &options.prefix,
                &text,
                &options.suffix,
            );
        } else if !text.starts_with('#') {
            panic!("First character of private identifier must be #: {text}");
        }

        let name = self.new_private_identifier(text);
        let auto_generate = AutoGenerateInfo {
            id,
            flags: kind | options.flags.without(GeneratedIdentifierFlags::KIND_MASK),
            prefix: options.prefix.clone(),
            suffix: options.suffix,
            node,
        };
        // PORT: Go allocates the nil map here; the Rust map always exists.
        self.emit_context()
            .auto_generate
            .borrow_mut()
            .insert(name, auto_generate);
        name
    }

    // Go: printer/factory.go:140 NewUniquePrivateName
    /// Allocates a new unique private name based on the provided text.
    pub fn new_unique_private_name(&self, text: &str) -> Node {
        self.new_unique_private_name_ex(text, AutoGenerateOptions::default())
    }

    // Go: printer/factory.go:145 NewUniquePrivateNameEx
    /// Allocates a new unique private name based on the provided text.
    pub fn new_unique_private_name_ex(&self, text: &str, options: AutoGenerateOptions) -> Node {
        self.new_generated_private_identifier(
            GeneratedIdentifierFlags::UNIQUE,
            text,
            Node::NIL, /*node*/
            options,
        )
    }

    // Go: printer/factory.go:150 NewGeneratedPrivateNameForNode
    /// Allocates a new unique private name based on the provided node.
    pub fn new_generated_private_name_for_node(&self, node: Node) -> Node {
        self.new_generated_private_name_for_node_ex(node, AutoGenerateOptions::default())
    }

    // Go: printer/factory.go:155 NewGeneratedPrivateNameForNodeEx
    /// Allocates a new unique private name based on the provided node.
    pub fn new_generated_private_name_for_node_ex(
        &self,
        node: Node,
        mut options: AutoGenerateOptions,
    ) -> Node {
        if !options.prefix.is_empty() || !options.suffix.is_empty() {
            options.flags |= GeneratedIdentifierFlags::OPTIMISTIC;
        }

        self.new_generated_private_identifier(GeneratedIdentifierFlags::NODE, "", node, options)
    }

    // Go: printer/factory.go:165 NewStringLiteralFromNode
    /// Allocates a new StringLiteral whose source text is derived from the provided node. This is often used to create a
    /// string representation of an Identifier or NumericLiteral.
    pub fn new_string_literal_from_node(&self, text_source_node: Node) -> Node {
        let mut text = "";
        match text_source_node.kind() {
            SyntaxKind::Identifier
            | SyntaxKind::PrivateIdentifier
            | SyntaxKind::JsxNamespacedName
            | SyntaxKind::StringLiteral
            | SyntaxKind::NumericLiteral
            | SyntaxKind::BigIntLiteral
            | SyntaxKind::NoSubstitutionTemplateLiteral
            | SyntaxKind::TemplateHead
            | SyntaxKind::TemplateMiddle
            | SyntaxKind::TemplateTail
            | SyntaxKind::RegularExpressionLiteral => {
                text = text_source_node.text();
            }
            _ => {}
        }
        let node = self.new_string_literal(text, TokenFlags::NONE);
        // PORT: Go allocates the nil map here; the Rust map always exists.
        self.emit_context()
            .text_source
            .borrow_mut()
            .insert(node, text_source_node);
        node
    }

    //
    // Common Tokens
    //

    // Go: printer/factory.go:193 NewThisExpression
    pub fn new_this_expression(&self) -> Node {
        self.new_keyword_expression(SyntaxKind::ThisKeyword)
    }

    // Go: printer/factory.go:197 NewTrueExpression
    pub fn new_true_expression(&self) -> Node {
        self.new_keyword_expression(SyntaxKind::TrueKeyword)
    }

    // Go: printer/factory.go:201 NewFalseExpression
    pub fn new_false_expression(&self) -> Node {
        self.new_keyword_expression(SyntaxKind::FalseKeyword)
    }

    //
    // Common Operators
    //

    // Go: printer/factory.go:209 NewCommaExpression
    pub fn new_comma_expression(&self, left: Node, right: Node) -> Node {
        self.new_binary_expression(
            ModifierList::NIL, /*modifiers*/
            left,
            Node::NIL, /*typeNode*/
            self.new_token(SyntaxKind::CommaToken),
            right,
        )
    }

    // Go: printer/factory.go:213 NewAssignmentExpression
    pub fn new_assignment_expression(&self, left: Node, right: Node) -> Node {
        self.new_binary_expression(
            ModifierList::NIL, /*modifiers*/
            left,
            Node::NIL, /*typeNode*/
            self.new_token(SyntaxKind::EqualsToken),
            right,
        )
    }

    // Go: printer/factory.go:217 NewLogicalORExpression
    pub fn new_logical_or_expression(&self, left: Node, right: Node) -> Node {
        self.new_binary_expression(
            ModifierList::NIL, /*modifiers*/
            left,
            Node::NIL, /*typeNode*/
            self.new_token(SyntaxKind::BarBarToken),
            right,
        )
    }

    // Go: printer/factory.go:221 NewLogicalANDExpression
    pub fn new_logical_and_expression(&self, left: Node, right: Node) -> Node {
        self.new_binary_expression(
            ModifierList::NIL, /*modifiers*/
            left,
            Node::NIL, /*typeNode*/
            self.new_token(SyntaxKind::AmpersandAmpersandToken),
            right,
        )
    }

    // func (f *NodeFactory) NewLogicalANDExpression(left *ast.Expression, right *ast.Expression) *ast.Expression
    // func (f *NodeFactory) NewBitwiseORExpression(left *ast.Expression, right *ast.Expression) *ast.Expression
    // func (f *NodeFactory) NewBitwiseXORExpression(left *ast.Expression, right *ast.Expression) *ast.Expression
    // func (f *NodeFactory) NewBitwiseANDExpression(left *ast.Expression, right *ast.Expression) *ast.Expression

    // Go: printer/factory.go:229 NewStrictEqualityExpression
    pub fn new_strict_equality_expression(&self, left: Node, right: Node) -> Node {
        self.new_binary_expression(
            ModifierList::NIL, /*modifiers*/
            left,
            Node::NIL, /*typeNode*/
            self.new_token(SyntaxKind::EqualsEqualsEqualsToken),
            right,
        )
    }

    // Go: printer/factory.go:233 NewStrictInequalityExpression
    pub fn new_strict_inequality_expression(&self, left: Node, right: Node) -> Node {
        self.new_binary_expression(
            ModifierList::NIL, /*modifiers*/
            left,
            Node::NIL, /*typeNode*/
            self.new_token(SyntaxKind::ExclamationEqualsEqualsToken),
            right,
        )
    }

    //
    // Compound Nodes
    //

    // Go: printer/factory.go:241 NewVoidZeroExpression
    pub fn new_void_zero_expression(&self) -> Node {
        self.new_void_expression(self.new_numeric_literal("0", TokenFlags::NONE))
    }
}

// Go: printer/factory.go:245 flattenCommaElement
fn flatten_comma_element(node: Node, expressions: &mut Vec<Node>) {
    if is_binary_expression(node)
        && node_is_synthesized(node)
        && node.operator_token().kind() == SyntaxKind::CommaToken
    {
        flatten_comma_element(node.left(), expressions);
        flatten_comma_element(node.right(), expressions);
    } else {
        expressions.push(node);
    }
}

// Go: printer/factory.go:255 flattenCommaElements
fn flatten_comma_elements(expressions: &[Node]) -> Vec<Node> {
    let mut result: Vec<Node> = Vec::new();
    for &expression in expressions {
        flatten_comma_element(expression, &mut result);
    }
    result
}

impl NodeFactory {
    // Go: printer/factory.go:264 InlineExpressions
    /// Converts a slice of expressions into a single comma-delimited expression. Returns nil if expressions is nil or empty.
    pub fn inline_expressions(&self, expressions: &[Node]) -> Node {
        if expressions.is_empty() {
            return Node::NIL;
        }
        if expressions.len() == 1 {
            return expressions[0];
        }
        let expressions = flatten_comma_elements(expressions);
        let mut expression = expressions[0];
        for &next in &expressions[1..] {
            expression = self.new_comma_expression(expression, next);
        }
        expression
    }

    //
    // Utilities
    //

    // Go: printer/factory.go:283 CreateExpressionFromEntityName
    pub fn create_expression_from_entity_name(&self, node: Node) -> Node {
        if is_qualified_name(node) {
            let left = self.create_expression_from_entity_name(node.left());
            let right = self.as_node_factory().clone_node(node.right());
            set_node_loc(right, node.right().loc());
            // TODO(rbuckton): Does this need to be parented?
            set_node_parent(right, node.right().parent());
            let prop_access =
                self.new_property_access_expression(left, Node::NIL, right, NodeFlags::NONE);
            set_node_loc(prop_access, node.loc());
            return prop_access;
        }
        let res = self.as_node_factory().clone_node(node);
        set_node_loc(res, node.loc());
        // TODO(rbuckton): Does this need to be parented?
        set_node_parent(res, node.parent());
        res
    }

    // Go: printer/factory.go:301 RestoreEnclosingLabel
    pub fn restore_enclosing_label(&self, node: Node, outermost_labeled_statement: Node) -> Node {
        if outermost_labeled_statement.is_nil() {
            return node;
        }
        let mut inner_label = node;
        if is_labeled_statement(outermost_labeled_statement.statement()) {
            inner_label =
                self.restore_enclosing_label(node, outermost_labeled_statement.statement());
        }
        self.update_labeled_statement(
            outermost_labeled_statement,
            outermost_labeled_statement.label(),
            inner_label,
        )
    }

    // Go: printer/factory.go:317 CreateForOfBindingStatement
    /// CreateForOfBindingStatement creates a statement to bind the iteration value.
    pub fn create_for_of_binding_statement(&self, node: Node, bound_value: Node) -> Node {
        if is_variable_declaration_list(node) {
            let first_declaration = node.declarations().nodes().get(0);
            let updated_declaration = self.update_variable_declaration(
                first_declaration,
                first_declaration.name(),
                Node::NIL, /*exclamationToken*/
                Node::NIL, /*type*/
                bound_value,
            );
            let statement = self.new_variable_statement(
                ModifierList::NIL,
                self.update_variable_declaration_list(
                    node,
                    self.new_node_list(&[updated_declaration]),
                    node.flags(),
                ),
            );
            set_node_loc(statement, node.loc());
            return statement;
        }
        let updated_expression = self.new_assignment_expression(node, bound_value);
        set_node_loc(updated_expression, node.loc());
        let statement = self.new_expression_statement(updated_expression);
        set_node_loc(statement, node.loc());
        statement
    }

    // Go: printer/factory.go:345 NewTypeCheck
    pub fn new_type_check(&self, value: Node, tag: &str) -> Node {
        if tag == "null" {
            self.new_strict_equality_expression(
                value,
                self.new_keyword_expression(SyntaxKind::NullKeyword),
            )
        } else if tag == "undefined" {
            self.new_strict_equality_expression(value, self.new_void_zero_expression())
        } else {
            self.new_strict_equality_expression(
                self.new_type_of_expression(value),
                self.new_string_literal(tag, TokenFlags::NONE),
            )
        }
    }

    // Go: printer/factory.go:355 NewMethodCall
    pub fn new_method_call(
        &self,
        object: Node,
        method_name: Node,
        arguments_list: &[Node],
    ) -> Node {
        // Preserve the optionality of `object`.
        if is_call_expression(object) && object.flags().intersects(NodeFlags::OPTIONAL_CHAIN) {
            return self.new_call_expression(
                self.new_property_access_expression(
                    object,
                    Node::NIL,
                    method_name,
                    NodeFlags::NONE,
                ),
                Node::NIL,
                NodeList::NIL,
                self.new_node_list(arguments_list),
                NodeFlags::OPTIONAL_CHAIN,
            );
        }
        self.new_call_expression(
            self.new_property_access_expression(object, Node::NIL, method_name, NodeFlags::NONE),
            Node::NIL,
            NodeList::NIL,
            self.new_node_list(arguments_list),
            NodeFlags::NONE,
        )
    }

    // Go: printer/factory.go:375 NewGlobalMethodCall
    pub fn new_global_method_call(
        &self,
        global_object_name: &str,
        method_name: &str,
        arguments_list: &[Node],
    ) -> Node {
        self.new_method_call(
            self.new_identifier(global_object_name),
            self.new_identifier(method_name),
            arguments_list,
        )
    }

    // Go: printer/factory.go:379 NewFunctionCallCall
    pub fn new_function_call_call(
        &self,
        target: Node,
        this_arg: Node,
        arguments_list: &[Node],
    ) -> Node {
        if this_arg.is_nil() {
            panic!("Attempted to construct function call call without this argument expression");
        }
        let mut args = vec![this_arg];
        args.extend_from_slice(arguments_list);
        self.new_method_call(target, self.new_identifier("call"), &args)
    }

    // Go: printer/factory.go:387 NewArraySliceCall
    pub fn new_array_slice_call(&self, array: Node, start: i32) -> Node {
        let mut args: Vec<Node> = Vec::new();
        if start != 0 {
            args.push(self.new_numeric_literal(start.to_string(), TokenFlags::NONE));
        }
        self.new_method_call(array, self.new_identifier("slice"), &args)
    }

    // Go: printer/factory.go:407 isIgnorableParen
    /// Determines whether a node is a parenthesized expression that can be ignored when recreating outer expressions.
    ///
    /// A parenthesized expression can be ignored when all of the following are true:
    ///
    /// - It's `pos` and `end` are not -1
    /// - It does not have a custom source map range
    /// - It does not have a custom comment range
    /// - It does not have synthetic leading or trailing comments
    ///
    /// If an outermost parenthesized expression is ignored, but the containing expression requires a parentheses around
    /// the expression to maintain precedence, a new parenthesized expression should be created automatically when
    /// the containing expression is created/updated.
    fn is_ignorable_paren(&self, node: Node) -> bool {
        is_parenthesized_expression(node)
            && node_is_synthesized(node)
            && range_is_synthesized(self.emit_context().source_map_range(node))
            && range_is_synthesized(self.emit_context().comment_range(node)) // &&
        // len(emitContext.SyntheticLeadingComments(node)) == 0 &&
        // len(emitContext.SyntheticTrailingComments(node)) == 0
    }

    // Go: printer/factory.go:416 updateOuterExpression
    fn update_outer_expression(
        &self,
        outer_expression: Node, /*OuterExpression*/
        expression: Node,
    ) -> Node {
        match outer_expression.kind() {
            SyntaxKind::ParenthesizedExpression => {
                self.update_parenthesized_expression(outer_expression, expression)
            }
            SyntaxKind::TypeAssertionExpression => {
                self.update_type_assertion(outer_expression, outer_expression.type_(), expression)
            }
            SyntaxKind::AsExpression => {
                self.update_as_expression(outer_expression, expression, outer_expression.type_())
            }
            SyntaxKind::SatisfiesExpression => self.update_satisfies_expression(
                outer_expression,
                expression,
                outer_expression.type_(),
            ),
            SyntaxKind::NonNullExpression => self.update_non_null_expression(
                outer_expression,
                expression,
                outer_expression.flags(),
            ),
            SyntaxKind::ExpressionWithTypeArguments => self.update_expression_with_type_arguments(
                outer_expression,
                expression,
                outer_expression.type_argument_list(),
            ),
            SyntaxKind::PartiallyEmittedExpression => {
                self.update_partially_emitted_expression(outer_expression, expression)
            }
            kind => panic!(
                "Unexpected outer expression kind: {}",
                crate::gostd::debug::kind_string(kind)
            ),
        }
    }

    // Go: printer/factory.go:437 RestoreOuterExpressions
    pub fn restore_outer_expressions(
        &self,
        outer_expression: Node,
        inner_expression: Node,
        kinds: OuterExpressionKinds,
    ) -> Node {
        if outer_expression.is_some()
            && is_outer_expression(outer_expression, kinds)
            && !self.is_ignorable_paren(outer_expression)
        {
            return self.update_outer_expression(
                outer_expression,
                self.restore_outer_expressions(
                    outer_expression.expression(),
                    inner_expression,
                    OuterExpressionKinds::OEK_ALL,
                ),
            );
        }
        inner_expression
    }

    // Go: printer/factory.go:448 EnsureUseStrict
    /// Ensures `"use strict"` is the first statement of a slice of statements.
    pub fn ensure_use_strict(&self, statements: &[Node]) -> Vec<Node> {
        for &statement in statements {
            if is_prologue_directive(statement) && statement.expression().text() == "use strict" {
                return statements.to_vec();
            } else {
                break;
            }
        }
        let use_strict_prologue =
            self.new_expression_statement(self.new_string_literal("use strict", TokenFlags::NONE));
        let mut result = Vec::with_capacity(statements.len() + 1);
        result.push(use_strict_prologue);
        result.extend_from_slice(statements);
        result
    }

    // Go: printer/factory.go:462 SplitStandardPrologue
    /// Splits a slice of statements into two parts: standard prologue statements and the rest of the statements
    // PORT: Go returns a nil `rest` slice when every statement is a prologue;
    // here it is an empty slice.
    #[must_use]
    pub fn split_standard_prologue<'a>(&self, source: &'a [Node]) -> (&'a [Node], &'a [Node]) {
        for (i, &statement) in source.iter().enumerate() {
            if !is_prologue_directive(statement) {
                return (&source[..i], &source[i..]);
            }
        }
        (source, &[])
    }

    // Go: printer/factory.go:472 SplitCustomPrologue
    /// Splits a slice of statements into two parts: custom prologue statements (e.g., with `EFCustomPrologue` set) and the rest of the statements
    // PORT: Go returns a nil `prologue` slice when no statement stops the
    // scan; here it is an empty slice.
    #[must_use]
    pub fn split_custom_prologue<'a>(&self, source: &'a [Node]) -> (&'a [Node], &'a [Node]) {
        for (i, &statement) in source.iter().enumerate() {
            if is_prologue_directive(statement)
                || !self
                    .emit_context()
                    .emit_flags(statement)
                    .intersects(EmitFlags::CUSTOM_PROLOGUE)
            {
                return (&source[..i], &source[i..]);
            }
        }
        (&[], source)
    }
}

//
// Declaration Names
//

// Go: printer/factory.go:485 NameOptions
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NameOptions {
    /// indicates whether comments may be emitted for the name.
    pub allow_comments: bool,
    /// indicates whether source maps may be emitted for the name.
    pub allow_source_maps: bool,
}

// Go: printer/factory.go:490 AssignedNameOptions
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AssignedNameOptions {
    /// indicates whether comments may be emitted for the name.
    pub allow_comments: bool,
    /// indicates whether source maps may be emitted for the name.
    pub allow_source_maps: bool,
    /// indicates whether the assigned name of a declaration shouldn't be considered.
    pub ignore_assigned_name: bool,
}

impl NodeFactory {
    // Go: printer/factory.go:496 getName
    fn get_name(&self, node: Node, mut emit_flags: EmitFlags, opts: AssignedNameOptions) -> Node {
        let mut node_name = Node::NIL;
        if node.is_some() {
            if opts.ignore_assigned_name {
                node_name = get_non_assigned_name_of_declaration(node);
            } else {
                node_name = get_name_of_declaration(node);
            }
        }

        if node_name.is_some() {
            let name = self.as_node_factory().clone_node(node_name);
            if !opts.allow_comments {
                emit_flags |= EmitFlags::NO_COMMENTS;
            }
            if !opts.allow_source_maps {
                emit_flags |= EmitFlags::NO_SOURCE_MAP;
            }
            self.emit_context().add_emit_flags(name, emit_flags);
            return name;
        }

        self.new_generated_name_for_node(node)
    }

    // Go: printer/factory.go:524 GetLocalName
    /// Gets the local name of a declaration. This is primarily used for declarations that can be referred to by name in the
    /// declaration's immediate scope (classes, enums, namespaces). A local name will *never* be prefixed with a module or
    /// namespace export modifier like "exports." when emitted as an expression.
    pub fn get_local_name(&self, node: Node) -> Node {
        self.get_local_name_ex(node, AssignedNameOptions::default())
    }

    // Go: printer/factory.go:531 GetLocalNameEx
    /// Gets the local name of a declaration. This is primarily used for declarations that can be referred to by name in the
    /// declaration's immediate scope (classes, enums, namespaces). A local name will *never* be prefixed with a module or
    /// namespace export modifier like "exports." when emitted as an expression.
    pub fn get_local_name_ex(&self, node: Node, opts: AssignedNameOptions) -> Node {
        self.get_name(node, EmitFlags::LOCAL_NAME, opts)
    }

    // Go: printer/factory.go:539 GetExportName
    /// Gets the export name of a declaration. This is primarily used for declarations that can be
    /// referred to by name in the declaration's immediate scope (classes, enums, namespaces). An
    /// export name will *always* be prefixed with an module or namespace export modifier like
    /// `"exports."` when emitted as an expression if the name points to an exported symbol.
    pub fn get_export_name(&self, node: Node) -> Node {
        self.get_export_name_ex(node, AssignedNameOptions::default())
    }

    // Go: printer/factory.go:547 GetExportNameEx
    /// Gets the export name of a declaration. This is primarily used for declarations that can be
    /// referred to by name in the declaration's immediate scope (classes, enums, namespaces). An
    /// export name will *always* be prefixed with an module or namespace export modifier like
    /// `"exports."` when emitted as an expression if the name points to an exported symbol.
    pub fn get_export_name_ex(&self, node: Node, opts: AssignedNameOptions) -> Node {
        self.get_name(node, EmitFlags::EXPORT_NAME, opts)
    }

    // Go: printer/factory.go:552 GetDeclarationName
    /// Gets the name of a declaration to use during emit.
    pub fn get_declaration_name(&self, node: Node) -> Node {
        self.get_declaration_name_ex(node, NameOptions::default())
    }

    // Go: printer/factory.go:557 GetDeclarationNameEx
    /// Gets the name of a declaration to use during emit.
    pub fn get_declaration_name_ex(&self, node: Node, opts: NameOptions) -> Node {
        self.get_name(
            node,
            EmitFlags::NONE,
            AssignedNameOptions {
                allow_comments: opts.allow_comments,
                allow_source_maps: opts.allow_source_maps,
                ..Default::default()
            },
        )
    }

    // Go: printer/factory.go:561 GetNamespaceMemberName
    pub fn get_namespace_member_name(&self, ns: Node, mut name: Node, opts: NameOptions) -> Node {
        if !self.emit_context().has_auto_generate_info(name) {
            name = self.as_node_factory().clone_node(name);
        }
        let qualified_name = self.new_property_access_expression(
            ns,
            Node::NIL, /*questionDotToken*/
            name,
            NodeFlags::NONE,
        );
        self.emit_context()
            .assign_comment_and_source_map_ranges(qualified_name, name);
        if !opts.allow_comments {
            self.emit_context()
                .add_emit_flags(qualified_name, EmitFlags::NO_COMMENTS);
        }
        if !opts.allow_source_maps {
            self.emit_context()
                .add_emit_flags(qualified_name, EmitFlags::NO_SOURCE_MAP);
        }
        qualified_name
    }

    // Go: printer/factory.go:580 GetExternalModuleOrNamespaceExportName
    /// Gets the export name of a declaration for use in expressions.
    ///
    /// An export name will *always* be prefixed with a module or namespace export modifier like
    /// `"exports."` when emitted as an expression if the name points to an exported symbol.
    pub fn get_external_module_or_namespace_export_name(
        &self,
        ns: Node,
        node: Node,
        allow_comments: bool,
        allow_source_maps: bool,
    ) -> Node {
        if ns.is_some() && has_syntactic_modifier(node, ModifierFlags::EXPORT) {
            let name_opts = NameOptions {
                allow_comments,
                allow_source_maps,
            };
            return self.get_namespace_member_name(
                ns,
                self.get_declaration_name_ex(node, name_opts),
                name_opts,
            );
        }
        self.get_export_name_ex(
            node,
            AssignedNameOptions {
                allow_comments,
                allow_source_maps,
                ..Default::default()
            },
        )
    }

    //
    // Emit Helpers
    //

    // Go: printer/factory.go:593 NewUnscopedHelperName
    /// Allocates a new Identifier representing a reference to a helper function.
    pub fn new_unscoped_helper_name(&self, name: &str) -> Node {
        let node = self.new_identifier(name);
        self.emit_context()
            .set_emit_flags(node, EmitFlags::HELPER_NAME);
        node
    }

    // TypeScript Helpers

    // Go: printer/factory.go:601 NewDecorateHelper
    pub fn new_decorate_helper(
        &self,
        decorator_expressions: &[Node],
        target: Node,
        member_name: Node,
        descriptor: Node,
    ) -> Node {
        self.emit_context().request_emit_helper(&DECORATE_HELPER);

        let mut arguments_array: Vec<Node> = Vec::new();
        arguments_array.push(
            self.new_array_literal_expression(self.new_node_list(decorator_expressions), true),
        );
        arguments_array.push(target);
        if member_name.is_some() {
            arguments_array.push(member_name);
            if descriptor.is_some() {
                arguments_array.push(descriptor);
            }
        }

        self.new_call_expression(
            self.new_unscoped_helper_name("__decorate"),
            Node::NIL,     /*questionDotToken*/
            NodeList::NIL, /*typeArguments*/
            self.new_node_list(&arguments_array),
            NodeFlags::NONE,
        )
    }

    // Go: printer/factory.go:623 NewMetadataHelper
    pub fn new_metadata_helper(&self, metadata_key: &str, metadata_value: Node) -> Node {
        self.emit_context().request_emit_helper(&METADATA_HELPER);

        self.new_call_expression(
            self.new_unscoped_helper_name("__metadata"),
            Node::NIL,     /*questionDotToken*/
            NodeList::NIL, /*typeArguments*/
            self.new_node_list(&[
                self.new_string_literal(metadata_key, TokenFlags::NONE),
                metadata_value,
            ]),
            NodeFlags::NONE,
        )
    }

    // Go: printer/factory.go:638 NewParamHelper
    pub fn new_param_helper(
        &self,
        expression: Node,
        parameter_offset: i32,
        location: TextRange,
    ) -> Node {
        self.emit_context().request_emit_helper(&PARAM_HELPER);
        let helper = self.new_call_expression(
            self.new_unscoped_helper_name("__param"),
            Node::NIL,     /*questionDotToken*/
            NodeList::NIL, /*typeArguments*/
            self.new_node_list(&[
                self.new_numeric_literal(parameter_offset.to_string(), TokenFlags::NONE),
                expression,
            ]),
            NodeFlags::NONE,
        );
        set_node_loc(helper, location);
        helper
    }

    // ESNext Helpers

    // Go: printer/factory.go:653 NewAddDisposableResourceHelper
    pub fn new_add_disposable_resource_helper(
        &self,
        env_binding: Node,
        value: Node,
        async_: bool,
    ) -> Node {
        self.emit_context()
            .request_emit_helper(&ADD_DISPOSABLE_RESOURCE_HELPER);
        self.new_call_expression(
            self.new_unscoped_helper_name("__addDisposableResource"),
            Node::NIL,     /*questionDotToken*/
            NodeList::NIL, /*typeArguments*/
            self.new_node_list(&[
                env_binding,
                value,
                self.new_keyword_expression(if async_ {
                    SyntaxKind::TrueKeyword
                } else {
                    SyntaxKind::FalseKeyword
                }),
            ]),
            NodeFlags::NONE,
        )
    }

    // Go: printer/factory.go:664 NewDisposeResourcesHelper
    pub fn new_dispose_resources_helper(&self, env_binding: Node) -> Node {
        self.emit_context()
            .request_emit_helper(&DISPOSE_RESOURCES_HELPER);
        self.new_call_expression(
            self.new_unscoped_helper_name("__disposeResources"),
            Node::NIL,     /*questionDotToken*/
            NodeList::NIL, /*typeArguments*/
            self.new_node_list(&[env_binding]),
            NodeFlags::NONE,
        )
    }
}

// Class Fields Helpers

// Go: printer/factory.go:677 PrivateIdentifierKind
/// Go `type PrivateIdentifierKind string`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PrivateIdentifierKind(pub &'static str);

impl PrivateIdentifierKind {
    pub const FIELD: Self = Self("f");
    pub const METHOD: Self = Self("m");
    pub const ACCESSOR: Self = Self("a");
    pub const UNTRANSFORMED: Self = Self("untransformed");
}

impl NodeFactory {
    // Go: printer/factory.go:686 NewClassPrivateFieldGetHelper
    pub fn new_class_private_field_get_helper(
        &self,
        receiver: Node,
        state: Node,
        kind: PrivateIdentifierKind,
        fn_: Node,
    ) -> Node {
        self.emit_context()
            .request_emit_helper(&CLASS_PRIVATE_FIELD_GET_HELPER);
        let args: Vec<Node> = if fn_.is_nil() {
            vec![
                receiver,
                state,
                self.new_string_literal(kind.0, TokenFlags::NONE),
            ]
        } else {
            vec![
                receiver,
                state,
                self.new_string_literal(kind.0, TokenFlags::NONE),
                fn_,
            ]
        };
        self.new_call_expression(
            self.new_unscoped_helper_name("__classPrivateFieldGet"),
            Node::NIL,     /*questionDotToken*/
            NodeList::NIL, /*typeArguments*/
            self.new_node_list(&args),
            NodeFlags::NONE,
        )
    }

    // Go: printer/factory.go:703 NewClassPrivateFieldSetHelper
    pub fn new_class_private_field_set_helper(
        &self,
        receiver: Node,
        state: Node,
        value: Node,
        kind: PrivateIdentifierKind,
        fn_: Node,
    ) -> Node {
        self.emit_context()
            .request_emit_helper(&CLASS_PRIVATE_FIELD_SET_HELPER);
        let args: Vec<Node> = if fn_.is_nil() {
            vec![
                receiver,
                state,
                value,
                self.new_string_literal(kind.0, TokenFlags::NONE),
            ]
        } else {
            vec![
                receiver,
                state,
                value,
                self.new_string_literal(kind.0, TokenFlags::NONE),
                fn_,
            ]
        };
        self.new_call_expression(
            self.new_unscoped_helper_name("__classPrivateFieldSet"),
            Node::NIL,     /*questionDotToken*/
            NodeList::NIL, /*typeArguments*/
            self.new_node_list(&args),
            NodeFlags::NONE,
        )
    }

    // Go: printer/factory.go:720 NewClassPrivateFieldInHelper
    pub fn new_class_private_field_in_helper(&self, state: Node, receiver: Node) -> Node {
        self.emit_context()
            .request_emit_helper(&CLASS_PRIVATE_FIELD_IN_HELPER);
        self.new_call_expression(
            self.new_unscoped_helper_name("__classPrivateFieldIn"),
            Node::NIL,     /*questionDotToken*/
            NodeList::NIL, /*typeArguments*/
            self.new_node_list(&[state, receiver]),
            NodeFlags::NONE,
        )
    }

    // Go: printer/factory.go:732 NewObjectDefinePropertyCall
    /// Creates `Object.defineProperty(target, name, descriptor)`.
    pub fn new_object_define_property_call(
        &self,
        target: Node,
        name: Node,
        descriptor: Node,
    ) -> Node {
        self.new_call_expression(
            self.new_property_access_expression(
                self.new_identifier("Object"),
                Node::NIL,
                self.new_identifier("defineProperty"),
                NodeFlags::NONE,
            ),
            Node::NIL,     /*questionDotToken*/
            NodeList::NIL, /*typeArguments*/
            self.new_node_list(&[target, name, descriptor]),
            NodeFlags::NONE,
        )
    }

    // Go: printer/factory.go:748 NewReflectGetCall
    /// Creates `Reflect.get(target, propertyKey, receiver)`.
    pub fn new_reflect_get_call(&self, target: Node, property_key: Node, receiver: Node) -> Node {
        self.new_call_expression(
            self.new_property_access_expression(
                self.new_identifier("Reflect"),
                Node::NIL,
                self.new_identifier("get"),
                NodeFlags::NONE,
            ),
            Node::NIL,     /*questionDotToken*/
            NodeList::NIL, /*typeArguments*/
            self.new_node_list(&[target, property_key, receiver]),
            NodeFlags::NONE,
        )
    }

    // Go: printer/factory.go:764 NewReflectSetCall
    /// Creates `Reflect.set(target, propertyKey, value, receiver)`.
    pub fn new_reflect_set_call(
        &self,
        target: Node,
        property_key: Node,
        value: Node,
        receiver: Node,
    ) -> Node {
        self.new_call_expression(
            self.new_property_access_expression(
                self.new_identifier("Reflect"),
                Node::NIL,
                self.new_identifier("set"),
                NodeFlags::NONE,
            ),
            Node::NIL,     /*questionDotToken*/
            NodeList::NIL, /*typeArguments*/
            self.new_node_list(&[target, property_key, value, receiver]),
            NodeFlags::NONE,
        )
    }

    // Go: printer/factory.go:780 NewFunctionBindCall
    /// Creates `target.bind(thisArg, ...args)`.
    pub fn new_function_bind_call(
        &self,
        target: Node,
        this_arg: Node,
        arguments_list: &[Node],
    ) -> Node {
        let mut args: Vec<Node> = Vec::with_capacity(1 + arguments_list.len());
        args.push(this_arg);
        args.extend_from_slice(arguments_list);
        self.new_method_call(target, self.new_identifier("bind"), &args)
    }

    // Go: printer/factory.go:788 NewImmediatelyInvokedArrowFunction
    /// Creates `(() => { ...statements })()` — an immediately invoked arrow function.
    pub fn new_immediately_invoked_arrow_function(&self, statements: &[Node]) -> Node {
        let arrow = self.new_arrow_function(
            ModifierList::NIL,                                  /*modifiers*/
            NodeList::NIL,                                      /*typeParameters*/
            self.new_node_list(&[]),                            /*parameters*/
            Node::NIL,                                          /*returnType*/
            Node::NIL,                                          /*fullSignature*/
            self.new_token(SyntaxKind::EqualsGreaterThanToken), /*equalsGreaterThanToken*/
            self.new_block(self.new_node_list(statements), true),
        );
        self.new_call_expression(
            self.new_parenthesized_expression(arrow),
            Node::NIL,     /*questionDotToken*/
            NodeList::NIL, /*typeArguments*/
            self.new_node_list(&[]),
            NodeFlags::NONE,
        )
    }

    // Go: printer/factory.go:808 NewExportDefault
    /// Creates `export default <expression>;`.
    pub fn new_export_default(&self, expression: Node) -> Node {
        self.new_export_assignment(ModifierList::NIL, false, Node::NIL, expression)
    }

    // Go: printer/factory.go:813 NewExternalModuleExport
    /// Creates `export { <name> };`.
    pub fn new_external_module_export(&self, name: Node) -> Node {
        let specifier = self.new_export_specifier(false, Node::NIL, name);
        let named_exports = self.new_named_exports(self.new_node_list(&[specifier]));
        self.new_export_declaration(
            ModifierList::NIL,
            false,
            named_exports,
            Node::NIL,
            Node::NIL,
        )
    }

    // ES2018 Helpers

    // Go: printer/factory.go:821 NewAssignHelper
    /// Chains a sequence of expressions using the __assign helper or Object.assign if available in the target
    pub fn new_assign_helper(
        &self,
        attributes_segments: &[Node],
        _script_target: ScriptTarget,
    ) -> Node {
        self.new_call_expression(
            self.new_property_access_expression(
                self.new_identifier("Object"),
                Node::NIL,
                self.new_identifier("assign"),
                NodeFlags::NONE,
            ),
            Node::NIL,
            NodeList::NIL,
            self.new_node_list(attributes_segments),
            NodeFlags::NONE,
        )
    }

    // ES2018 Destructuring Helpers

    // Go: printer/factory.go:827 NewRestHelper
    // PORT: Go `computedTempVariables == nil` is `None`.
    pub fn new_rest_helper(
        &self,
        value: Node,
        elements: &[Node],
        computed_temp_variables: Option<&[Node]>,
        location: TextRange,
    ) -> Node {
        self.emit_context().request_emit_helper(&REST_HELPER);
        let mut property_names: Vec<Node> = Vec::new();
        let mut computed_temp_variable_offset = 0usize;
        for (i, &element) in elements.iter().enumerate() {
            if i == elements.len() - 1 {
                break;
            }
            let property_name = try_get_property_name_of_binding_or_assignment_element(element);
            if property_name.is_some() {
                if is_computed_property_name(property_name) {
                    go_assert!(
                        computed_temp_variables.is_some(),
                        "Encountered computed property name but 'computedTempVariables' argument was not provided."
                    );
                    let temp =
                        computed_temp_variables.unwrap_or(&[])[computed_temp_variable_offset];
                    computed_temp_variable_offset += 1;
                    // typeof _tmp === "symbol" ? _tmp : _tmp + ""
                    property_names.push(self.new_conditional_expression(
                        self.new_type_check(temp, "symbol"),
                        self.new_token(SyntaxKind::QuestionToken),
                        temp,
                        self.new_token(SyntaxKind::ColonToken),
                        self.new_binary_expression(
                            ModifierList::NIL,
                            temp,
                            Node::NIL,
                            self.new_token(SyntaxKind::PlusToken),
                            self.new_string_literal("", TokenFlags::NONE),
                        ),
                    ));
                } else {
                    property_names.push(self.new_string_literal_from_node(property_name));
                }
            }
        }
        let prop_names =
            self.new_array_literal_expression(self.new_node_list(&property_names), false);
        set_node_loc(prop_names, location);
        self.new_call_expression(
            self.new_unscoped_helper_name("__rest"),
            Node::NIL,
            NodeList::NIL,
            self.new_node_list(&[value, prop_names]),
            NodeFlags::NONE,
        )
    }

    // ES2018 Helpers

    // Go: printer/factory.go:871 NewAwaitHelper
    /// Allocates a new Call expression to the `__await` helper.
    pub fn new_await_helper(&self, expression: Node) -> Node {
        self.emit_context().request_emit_helper(&AWAIT_HELPER);
        self.new_call_expression(
            self.new_unscoped_helper_name("__await"),
            Node::NIL,     /*questionDotToken*/
            NodeList::NIL, /*typeArguments*/
            self.new_node_list(&[expression]),
            NodeFlags::NONE,
        )
    }

    // Go: printer/factory.go:883 NewAsyncGeneratorHelper
    /// Allocates a new Call expression to the `__asyncGenerator` helper.
    pub fn new_async_generator_helper(&self, generator_func: Node, has_lexical_this: bool) -> Node {
        self.emit_context().request_emit_helper(&AWAIT_HELPER);
        self.emit_context()
            .request_emit_helper(&ASYNC_GENERATOR_HELPER);

        // Mark this node as originally an async function body
        self.emit_context().add_emit_flags(
            generator_func,
            EmitFlags::ASYNC_FUNCTION_BODY | EmitFlags::REUSE_TEMP_VARIABLE_SCOPE,
        );

        let this_arg = if has_lexical_this {
            self.new_keyword_expression(SyntaxKind::ThisKeyword)
        } else {
            self.new_void_zero_expression()
        };

        self.new_call_expression(
            self.new_unscoped_helper_name("__asyncGenerator"),
            Node::NIL,     /*questionDotToken*/
            NodeList::NIL, /*typeArguments*/
            self.new_node_list(&[this_arg, self.new_identifier("arguments"), generator_func]),
            NodeFlags::NONE,
        )
    }

    // Go: printer/factory.go:914 NewAsyncDelegatorHelper
    /// Allocates a new Call expression to the `__asyncDelegator` helper.
    pub fn new_async_delegator_helper(&self, expression: Node) -> Node {
        self.emit_context().request_emit_helper(&AWAIT_HELPER);
        self.emit_context()
            .request_emit_helper(&ASYNC_DELEGATOR_HELPER);
        self.new_call_expression(
            self.new_unscoped_helper_name("__asyncDelegator"),
            Node::NIL,     /*questionDotToken*/
            NodeList::NIL, /*typeArguments*/
            self.new_node_list(&[expression]),
            NodeFlags::NONE,
        )
    }

    // Go: printer/factory.go:927 NewAsyncValuesHelper
    /// Allocates a new Call expression to the `__asyncValues` helper.
    pub fn new_async_values_helper(&self, expression: Node) -> Node {
        self.emit_context()
            .request_emit_helper(&ASYNC_VALUES_HELPER);
        self.new_call_expression(
            self.new_unscoped_helper_name("__asyncValues"),
            Node::NIL,     /*questionDotToken*/
            NodeList::NIL, /*typeArguments*/
            self.new_node_list(&[expression]),
            NodeFlags::NONE,
        )
    }

    // !!! ES2017 Helpers

    // Go: printer/factory.go:941 NewAwaiterHelper
    /// Allocates a new Call expression to the `__awaiter` helper.
    pub fn new_awaiter_helper(
        &self,
        has_lexical_this: bool,
        arguments_expression: Node,
        parameters: NodeList,
        body: Node,
    ) -> Node {
        self.emit_context().request_emit_helper(&AWAITER_HELPER);

        let params = if parameters.is_nil() {
            self.new_node_list(&[])
        } else {
            parameters
        };

        let generator_func = self.new_function_expression(
            ModifierList::NIL, /*modifiers*/
            self.new_token(SyntaxKind::AsteriskToken),
            Node::NIL,     /*name*/
            NodeList::NIL, /*typeParameters*/
            params,
            Node::NIL, /*returnType*/
            Node::NIL, /*fullSignature*/
            body,
        );

        // Mark this node as originally an async function body
        self.emit_context().add_emit_flags(
            generator_func,
            EmitFlags::ASYNC_FUNCTION_BODY | EmitFlags::REUSE_TEMP_VARIABLE_SCOPE,
        );

        let this_arg = if has_lexical_this {
            self.new_keyword_expression(SyntaxKind::ThisKeyword)
        } else {
            self.new_void_zero_expression()
        };

        let args_arg = if arguments_expression.is_some() {
            arguments_expression
        } else {
            self.new_void_zero_expression()
        };

        self.new_call_expression(
            self.new_unscoped_helper_name("__awaiter"),
            Node::NIL,     /*questionDotToken*/
            NodeList::NIL, /*typeArguments*/
            self.new_node_list(&[
                this_arg,
                args_arg,
                self.new_void_zero_expression(),
                generator_func,
            ]),
            NodeFlags::NONE,
        )
    }

    // ES Decorator Helpers

    // Go: printer/factory.go:1000 NewESDecorateClassContextObject
    pub fn new_es_decorate_class_context_object(&self, name_expr: Node, metadata: Node) -> Node {
        let props = [
            self.new_property_assignment(
                ModifierList::NIL,
                self.new_identifier("kind"),
                Node::NIL,
                Node::NIL,
                self.new_string_literal("class", TokenFlags::NONE),
            ),
            self.new_property_assignment(
                ModifierList::NIL,
                self.new_identifier("name"),
                Node::NIL,
                Node::NIL,
                name_expr,
            ),
            self.new_property_assignment(
                ModifierList::NIL,
                self.new_identifier("metadata"),
                Node::NIL,
                Node::NIL,
                metadata,
            ),
        ];
        self.new_object_literal_expression(self.new_node_list(&props), false)
    }

    // Go: printer/factory.go:1009 NewESDecorateClassElementAccessGetMethod
    pub fn new_es_decorate_class_element_access_get_method(
        &self,
        name_computed: bool,
        name_expr: Node,
    ) -> Node {
        let accessor = if name_computed {
            self.new_element_access_expression(
                self.new_identifier("obj"),
                Node::NIL,
                name_expr,
                NodeFlags::NONE,
            )
        } else {
            self.new_property_access_expression(
                self.new_identifier("obj"),
                Node::NIL,
                name_expr,
                NodeFlags::NONE,
            )
        };

        let obj_param = self.new_parameter_declaration(
            ModifierList::NIL,
            Node::NIL,
            self.new_identifier("obj"),
            Node::NIL,
            Node::NIL,
            Node::NIL,
        );

        let arrow = self.new_arrow_function(
            ModifierList::NIL,
            NodeList::NIL,
            self.new_node_list(&[obj_param]),
            Node::NIL,
            Node::NIL,
            self.new_token(SyntaxKind::EqualsGreaterThanToken),
            accessor,
        );

        self.new_property_assignment(
            ModifierList::NIL,
            self.new_identifier("get"),
            Node::NIL,
            Node::NIL,
            arrow,
        )
    }

    // Go: printer/factory.go:1033 NewESDecorateClassElementAccessSetMethod
    pub fn new_es_decorate_class_element_access_set_method(
        &self,
        name_computed: bool,
        name_expr: Node,
    ) -> Node {
        let accessor = if name_computed {
            self.new_element_access_expression(
                self.new_identifier("obj"),
                Node::NIL,
                name_expr,
                NodeFlags::NONE,
            )
        } else {
            self.new_property_access_expression(
                self.new_identifier("obj"),
                Node::NIL,
                name_expr,
                NodeFlags::NONE,
            )
        };

        let assignment = self.new_assignment_expression(accessor, self.new_identifier("value"));
        let stmt = self.new_expression_statement(assignment);
        let body = self.new_block(self.new_node_list(&[stmt]), false);

        let obj_param = self.new_parameter_declaration(
            ModifierList::NIL,
            Node::NIL,
            self.new_identifier("obj"),
            Node::NIL,
            Node::NIL,
            Node::NIL,
        );
        let value_param = self.new_parameter_declaration(
            ModifierList::NIL,
            Node::NIL,
            self.new_identifier("value"),
            Node::NIL,
            Node::NIL,
            Node::NIL,
        );

        let arrow = self.new_arrow_function(
            ModifierList::NIL,
            NodeList::NIL,
            self.new_node_list(&[obj_param, value_param]),
            Node::NIL,
            Node::NIL,
            self.new_token(SyntaxKind::EqualsGreaterThanToken),
            body,
        );

        self.new_property_assignment(
            ModifierList::NIL,
            self.new_identifier("set"),
            Node::NIL,
            Node::NIL,
            arrow,
        )
    }

    // Go: printer/factory.go:1062 NewESDecorateClassElementAccessHasMethod
    pub fn new_es_decorate_class_element_access_has_method(
        &self,
        name_computed: bool,
        name_expr: Node,
    ) -> Node {
        // The property name for the "in" expression
        let property_name = if !name_computed && name_expr.is_some() && is_identifier(name_expr) {
            self.new_string_literal_from_node(name_expr)
        } else {
            name_expr
        };

        let obj_param = self.new_parameter_declaration(
            ModifierList::NIL,
            Node::NIL,
            self.new_identifier("obj"),
            Node::NIL,
            Node::NIL,
            Node::NIL,
        );
        let in_expr = self.new_binary_expression(
            ModifierList::NIL,
            property_name,
            Node::NIL,
            self.new_token(SyntaxKind::InKeyword),
            self.new_identifier("obj"),
        );

        let arrow = self.new_arrow_function(
            ModifierList::NIL,
            NodeList::NIL,
            self.new_node_list(&[obj_param]),
            Node::NIL,
            Node::NIL,
            self.new_token(SyntaxKind::EqualsGreaterThanToken),
            in_expr,
        );

        self.new_property_assignment(
            ModifierList::NIL,
            self.new_identifier("has"),
            Node::NIL,
            Node::NIL,
            arrow,
        )
    }

    // Go: printer/factory.go:1098 NewESDecorateClassElementAccessObject
    /// Creates the "access" object for a class element decorator context.
    ///
    /// 15.7.3 CreateDecoratorAccessObject (kind, name)
    ///
    ///  2. If _kind_ is ~field~, ~method~, ~accessor~, or ~getter~, then
    ///     a. Let _getAccess_ be a new Abstract Closure with parameters (_object_) that captures _kind_ and _name_ ...
    ///     b. Perform ! CreateDataPropertyOrThrow(_access_, "get", _getAccess_).
    ///  3. If _kind_ is ~field~, ~accessor~, or ~setter~, then
    ///     a. Let _setAccess_ be a new Abstract Closure with parameters (_object_, _value_) that captures _kind_ and _name_ ...
    ///     b. Perform ! CreateDataPropertyOrThrow(_access_, "set", _setAccess_).
    pub fn new_es_decorate_class_element_access_object(
        &self,
        name_computed: bool,
        name_expr: Node,
        has_get: bool,
        has_set: bool,
    ) -> Node {
        let mut access_props: Vec<Node> = Vec::new();

        // "has" method: obj => name in obj
        access_props
            .push(self.new_es_decorate_class_element_access_has_method(name_computed, name_expr));

        // "get" method: obj => obj.name or obj => obj[name]
        if has_get {
            access_props.push(
                self.new_es_decorate_class_element_access_get_method(name_computed, name_expr),
            );
        }

        // "set" method: (obj, value) => { obj.name = value; } or (obj, value) => { obj[name] = value; }
        if has_set {
            access_props.push(
                self.new_es_decorate_class_element_access_set_method(name_computed, name_expr),
            );
        }

        self.new_object_literal_expression(self.new_node_list(&access_props), false)
    }

    // Go: printer/factory.go:1122 NewESDecorateClassElementContextObject
    #[allow(clippy::too_many_arguments)]
    pub fn new_es_decorate_class_element_context_object(
        &self,
        kind: &str,
        name_computed: bool,
        name_expr: Node,
        is_static: bool,
        is_private: bool,
        has_get: bool,
        has_set: bool,
        metadata: Node,
    ) -> Node {
        // Build the name value for the context's "name" property
        let name_value = if !name_computed
            && name_expr.is_some()
            && (is_private_identifier(name_expr) || is_identifier(name_expr))
        {
            self.new_string_literal_from_node(name_expr)
        } else {
            name_expr
        };

        // Build the access object with has/get/set arrow functions
        let access_obj = self.new_es_decorate_class_element_access_object(
            name_computed,
            name_expr,
            has_get,
            has_set,
        );

        let static_expr = if is_static {
            self.new_true_expression()
        } else {
            self.new_false_expression()
        };

        let private_expr = if is_private {
            self.new_true_expression()
        } else {
            self.new_false_expression()
        };

        let props = [
            self.new_property_assignment(
                ModifierList::NIL,
                self.new_identifier("kind"),
                Node::NIL,
                Node::NIL,
                self.new_string_literal(kind, TokenFlags::NONE),
            ),
            self.new_property_assignment(
                ModifierList::NIL,
                self.new_identifier("name"),
                Node::NIL,
                Node::NIL,
                name_value,
            ),
            self.new_property_assignment(
                ModifierList::NIL,
                self.new_identifier("static"),
                Node::NIL,
                Node::NIL,
                static_expr,
            ),
            self.new_property_assignment(
                ModifierList::NIL,
                self.new_identifier("private"),
                Node::NIL,
                Node::NIL,
                private_expr,
            ),
            self.new_property_assignment(
                ModifierList::NIL,
                self.new_identifier("access"),
                Node::NIL,
                Node::NIL,
                access_obj,
            ),
            self.new_property_assignment(
                ModifierList::NIL,
                self.new_identifier("metadata"),
                Node::NIL,
                Node::NIL,
                metadata,
            ),
        ];
        self.new_object_literal_expression(self.new_node_list(&props), false)
    }

    // Go: printer/factory.go:1168 NewESDecorateHelper
    pub fn new_es_decorate_helper(
        &self,
        ctor: Node,
        descriptor_in: Node,
        decorators: Node,
        context_in: Node,
        initializers: Node,
        extra_initializers: Node,
    ) -> Node {
        self.emit_context().request_emit_helper(&ES_DECORATE_HELPER);
        self.new_call_expression(
            self.new_unscoped_helper_name("__esDecorate"),
            Node::NIL,     /*questionDotToken*/
            NodeList::NIL, /*typeArguments*/
            self.new_node_list(&[
                ctor,
                descriptor_in,
                decorators,
                context_in,
                initializers,
                extra_initializers,
            ]),
            NodeFlags::NONE,
        )
    }

    // Go: printer/factory.go:1179 NewRunInitializersHelper
    pub fn new_run_initializers_helper(
        &self,
        this_arg: Node,
        initializers: Node,
        value: Node,
    ) -> Node {
        self.emit_context()
            .request_emit_helper(&RUN_INITIALIZERS_HELPER);
        let arguments: Vec<Node> = if value.is_some() {
            vec![this_arg, initializers, value]
        } else {
            vec![this_arg, initializers]
        };
        self.new_call_expression(
            self.new_unscoped_helper_name("__runInitializers"),
            Node::NIL,     /*questionDotToken*/
            NodeList::NIL, /*typeArguments*/
            self.new_node_list(&arguments),
            NodeFlags::NONE,
        )
    }

    // ES2015 Helpers

    // Go: printer/factory.go:1198 NewTemplateObjectHelper
    pub fn new_template_object_helper(&self, cooked_array: Node, raw_array: Node) -> Node {
        self.emit_context()
            .request_emit_helper(&MAKE_TEMPLATE_OBJECT_HELPER);
        self.new_call_expression(
            self.new_unscoped_helper_name("__makeTemplateObject"),
            Node::NIL,     /*questionDotToken*/
            NodeList::NIL, /*typeArguments*/
            self.new_node_list(&[cooked_array, raw_array]),
            NodeFlags::NONE,
        )
    }

    // Go: printer/factory.go:1209 NewPropKeyHelper
    pub fn new_prop_key_helper(&self, expr: Node) -> Node {
        self.emit_context().request_emit_helper(&PROP_KEY_HELPER);
        self.new_call_expression(
            self.new_unscoped_helper_name("__propKey"),
            Node::NIL,     /*questionDotToken*/
            NodeList::NIL, /*typeArguments*/
            self.new_node_list(&[expr]),
            NodeFlags::NONE,
        )
    }

    // Go: printer/factory.go:1220 NewSetFunctionNameHelper
    pub fn new_set_function_name_helper(&self, fn_: Node, name: Node, prefix: &str) -> Node {
        self.emit_context()
            .request_emit_helper(&SET_FUNCTION_NAME_HELPER);
        let arguments: Vec<Node> = if !prefix.is_empty() {
            vec![fn_, name, self.new_string_literal(prefix, TokenFlags::NONE)]
        } else {
            vec![fn_, name]
        };
        self.new_call_expression(
            self.new_unscoped_helper_name("__setFunctionName"),
            Node::NIL,     /*questionDotToken*/
            NodeList::NIL, /*typeArguments*/
            self.new_node_list(&arguments),
            NodeFlags::NONE,
        )
    }

    // ES Module Helpers

    // Go: printer/factory.go:1240 NewImportDefaultHelper
    /// Allocates a new Call expression to the `__importDefault` helper.
    pub fn new_import_default_helper(&self, expression: Node) -> Node {
        self.emit_context()
            .request_emit_helper(&IMPORT_DEFAULT_HELPER);
        self.new_call_expression(
            self.new_unscoped_helper_name("__importDefault"),
            Node::NIL,     /*questionDotToken*/
            NodeList::NIL, /*typeArguments*/
            self.new_node_list(&[expression]),
            NodeFlags::NONE,
        )
    }

    // Go: printer/factory.go:1252 NewImportStarHelper
    /// Allocates a new Call expression to the `__importStar` helper.
    pub fn new_import_star_helper(&self, expression: Node) -> Node {
        self.emit_context().request_emit_helper(&IMPORT_STAR_HELPER);
        self.new_call_expression(
            self.new_unscoped_helper_name("__importStar"),
            Node::NIL,     /*questionDotToken*/
            NodeList::NIL, /*typeArguments*/
            self.new_node_list(&[expression]),
            NodeFlags::NONE,
        )
    }

    // Go: printer/factory.go:1264 NewExportStarHelper
    /// Allocates a new Call expression to the `__exportStar` helper.
    pub fn new_export_star_helper(
        &self,
        module_expression: Node,
        exports_expression: Node,
    ) -> Node {
        self.emit_context().request_emit_helper(&EXPORT_STAR_HELPER);
        self.new_call_expression(
            self.new_unscoped_helper_name("__exportStar"),
            Node::NIL,     /*questionDotToken*/
            NodeList::NIL, /*typeArguments*/
            self.new_node_list(&[module_expression, exports_expression]),
            NodeFlags::NONE,
        )
    }

    // Go: printer/factory.go:1275 NewAssignmentTargetWrapper
    pub fn new_assignment_target_wrapper(&self, param_name: Node, expression: Node) -> Node {
        let set_accessor = self.new_set_accessor_declaration(
            ModifierList::NIL, /*modifiers*/
            self.new_identifier("value"),
            NodeList::NIL, /*typeParameters*/
            self.new_node_list(&[self.new_parameter_declaration(
                ModifierList::NIL,
                Node::NIL,
                param_name,
                Node::NIL,
                Node::NIL,
                Node::NIL,
            )]),
            Node::NIL, /*returnType*/
            Node::NIL, /*fullSignature*/
            self.new_block(
                self.new_node_list(&[self.new_expression_statement(expression)]),
                false,
            ),
        );
        let obj_literal =
            self.new_object_literal_expression(self.new_node_list(&[set_accessor]), false);
        // Explicit parens required because of v8 regression (https://bugs.chromium.org/p/v8/issues/detail?id=9560)
        self.new_property_access_expression(
            self.new_parenthesized_expression(obj_literal),
            Node::NIL, /*questionDotToken*/
            self.new_identifier("value"),
            NodeFlags::NONE,
        )
    }

    // Go: printer/factory.go:1300 NewRewriteRelativeImportExtensionsHelper
    /// Allocates a new Call expression to the `__rewriteRelativeImportExtension` helper.
    pub fn new_rewrite_relative_import_extensions_helper(
        &self,
        first_argument: Node,
        preserve_jsx: bool,
    ) -> Node {
        self.emit_context()
            .request_emit_helper(&REWRITE_RELATIVE_IMPORT_EXTENSIONS_HELPER);
        let arguments: Vec<Node> = if preserve_jsx {
            vec![first_argument, self.new_token(SyntaxKind::TrueKeyword)]
        } else {
            vec![first_argument]
        };
        self.new_call_expression(
            self.new_unscoped_helper_name("__rewriteRelativeImportExtension"),
            Node::NIL,     /*questionDotToken*/
            NodeList::NIL, /*typeArguments*/
            self.new_node_list(&arguments),
            NodeFlags::NONE,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::panic::AssertUnwindSafe;

    // Go prints `%s` of the Kind in this panic (printer/factory.go:433
    // `panic(fmt.Sprintf("Unexpected outer expression kind: %s",
    // outerExpression.Kind))`), which is `Kind.String()`.
    #[test]
    fn unexpected_outer_expression_panic_names_the_go_kind() {
        let context = super::super::emit_context::new_emit_context();
        let f = NodeFactory::new(&context);
        let (outer, inner) = (f.new_identifier("x"), f.new_identifier("y"));
        let payload =
            std::panic::catch_unwind(AssertUnwindSafe(|| f.update_outer_expression(outer, inner)))
                .expect_err("no panic");
        assert_eq!(
            payload.downcast_ref::<String>().map(String::as_str),
            Some("Unexpected outer expression kind: KindIdentifier")
        );
    }
}
