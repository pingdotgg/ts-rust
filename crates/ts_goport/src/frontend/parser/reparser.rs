//! Port of typescript-go `internal/parser/reparser.go`: JSDoc tags in JS
//! files become synthetic TypeScript nodes (unit U9).
//!
//! Hosted tags find a host node and add their children to it. Unhosted tags
//! add new statements to the reparse list.
//!
//! PORT: Go writes node fields in place (`node.AsX().Field = v`,
//! `AsMutable().SetType(..)`). Here each write clones the node data, changes
//! the field and calls `replace_node_data` (see `mutate`). Go list appends
//! (`list.Nodes = append(list.Nodes, n)`) make a new list with the same
//! `Loc`, because Rust lists cannot change.
//!
//! PORT: Go `p.nodeSliceArena` only saves allocations. Here each call uses a
//! new `Vec`.

use crate::astdata::BoxedData as D;
use crate::frontend::prelude::*;

/// Go `node.AsX().Field = v`: clones the data of `n`, lets `f` change it and
/// stores it back. The ids and lists that `f` stores come from the `node_*`
/// value helpers of `ast::synthetic` for `n` (`node_child_id`,
/// `node_list_value`, ...), not from the parser's factory: they must be in
/// the space and owner of `n`, as its new data is (astshrink2).
fn mutate(n: Node, f: impl FnOnce(&mut D)) {
    let mut data = with_ast_data(n, |d| d.to_boxed());
    f(&mut data);
    replace_node_data(n, data);
}

/// Runs `$body` with `$d` bound to the data of a function-like node (Go
/// `FunctionLikeData()`). `$index_body` is for IndexSignatureDeclaration,
/// whose `type_` field is required in astdata. Other kinds panic, like a Go
/// nil `FunctionLikeData()` dereference.
macro_rules! with_function_like_data {
    ($data:expr, $d:ident => $body:expr, index $i:ident => $index_body:expr) => {
        match $data {
            D::ArrowFunction($d) => $body,
            D::CallSignatureDeclaration($d) => $body,
            D::ConstructSignatureDeclaration($d) => $body,
            D::ConstructorDeclaration($d) => $body,
            D::ConstructorTypeNode($d) => $body,
            D::FunctionDeclaration($d) => $body,
            D::FunctionExpression($d) => $body,
            D::FunctionTypeNode($d) => $body,
            D::GetAccessorDeclaration($d) => $body,
            D::JsDocSignature($d) => $body,
            D::MethodDeclaration($d) => $body,
            D::MethodSignatureDeclaration($d) => $body,
            D::SetAccessorDeclaration($d) => $body,
            D::IndexSignatureDeclaration($i) => $index_body,
            other => panic!(
                "node data is not function-like: {}",
                other.view().schema_name()
            ),
        }
    };
}

// Go: ast/ast.go:663 (m *MutableNode) SetType
// PORT: only the kinds that the reparser writes. Go panics on other kinds.
fn set_type(n: Node, t: Node) {
    let id = node_opt_child_id(n, t);
    mutate(n, |data| match data {
        D::VariableDeclaration(d) => d.type_ = id,
        D::ParameterDeclaration(d) => d.type_ = id,
        D::PropertyDeclaration(d) => d.type_ = id,
        D::PropertyAssignment(d) => d.type_ = id,
        D::ShorthandPropertyAssignment(d) => d.type_ = id,
        D::ExportAssignment(d) => d.type_ = id,
        D::BinaryExpression(d) => d.type_ = id,
        D::GetAccessorDeclaration(d) => d.type_ = id,
        other => panic!(
            "Unhandled case in mutableNode.SetType: {}",
            other.view().schema_name()
        ),
    });
}

// Go: ast/ast.go:373 (m *MutableNode) SetExpression
// PORT: only the kinds that the reparser writes.
fn set_expression(n: Node, expr: Node) {
    let id = node_child_id(n, expr);
    mutate(n, |data| match data {
        D::ReturnStatement(d) => d.expression = Some(id),
        D::ParenthesizedExpression(d) => d.expression = id,
        D::ExportAssignment(d) => d.expression = id,
        other => panic!(
            "Unhandled case in mutableNode.SetExpression: {}",
            other.view().schema_name()
        ),
    });
}

// Go: ast/ast.go:751 (m *MutableNode) SetInitializer
// PORT: only the kinds that the reparser writes.
fn set_initializer(n: Node, initializer: Node) {
    let id = node_child_id(n, initializer);
    mutate(n, |data| match data {
        D::VariableDeclaration(d) => d.initializer = Some(id),
        D::PropertyDeclaration(d) => d.initializer = Some(id),
        D::PropertyAssignment(d) => d.initializer = id,
        _ => panic!("Unhandled case in mutableNode.SetInitializer"),
    });
}

// Go: ast/ast_generated.go:11904 (n *MutableNode) SetModifiers
// PORT: only the kinds that the reparser writes.
fn set_modifiers(n: Node, modifiers: ModifierList) {
    let mods = node_modifiers_value(n, modifiers);
    mutate(n, |data| match data {
        D::PropertyDeclaration(d) => d.modifiers = mods,
        D::MethodDeclaration(d) => d.modifiers = mods,
        D::ConstructorDeclaration(d) => d.modifiers = mods,
        D::GetAccessorDeclaration(d) => d.modifiers = mods,
        D::SetAccessorDeclaration(d) => d.modifiers = mods,
        D::BinaryExpression(d) => d.modifiers = mods,
        other => panic!(
            "Unhandled case in setModifiers: {}",
            other.view().schema_name()
        ),
    });
}

/// Go `fun.FunctionLikeData().TypeParameters = list`.
fn set_function_type_parameters(fun: Node, list: NodeList) {
    let v = node_list_value(fun, list);
    mutate(
        fun,
        |data| with_function_like_data!(data, d => d.type_parameters = v, index d => d.type_parameters = v),
    );
}

/// Go `fun.FunctionLikeData().Parameters = list`.
fn set_function_parameters(fun: Node, list: NodeList) {
    let v = node_req_list_value(fun, list);
    mutate(
        fun,
        |data| with_function_like_data!(data, d => d.parameters = v, index d => d.parameters = v),
    );
}

/// Go `fun.FunctionLikeData().Type = t`.
fn set_function_type(fun: Node, t: Node) {
    let v = node_opt_child_id(fun, t);
    let required = node_child_id(fun, t);
    mutate(
        fun,
        |data| with_function_like_data!(data, d => d.type_ = v, index d => d.type_ = required),
    );
}

/// Go `fun.FunctionLikeData().FullSignature = t`.
fn set_function_full_signature(fun: Node, t: Node) {
    let v = node_opt_child_id(fun, t);
    mutate(
        fun,
        |data| with_function_like_data!(data, d => d.full_signature = v, index d => d.full_signature = v),
    );
}

impl<'a> Parser<'a> {
    // Go: parser/reparser.go:13 finishReparsedNode
    fn finish_reparsed_node(&mut self, node: Node, location_node: Node) {
        set_node_flags(node, self.context_flags | NodeFlags::REPARSED);
        set_node_loc(node, location_node.loc());
        self.override_parent_in_immediate_children(node);
    }

    // Go: parser/reparser.go:19 finishMutatedNode
    fn finish_mutated_node(&mut self, node: Node) {
        self.override_parent_in_immediate_children(node);
    }

    // Go: parser/reparser.go:26 addDeepCloneReparse
    // Deep-clone the given node and add the clone to the reparsed clone list. The list is used by ast.GetReparsedNodeForNode
    // to locate reparsed clones of JSDoc nodes. Since the binder attaches symbols to reparsed nodes and not to JSDoc nodes, we
    // need the mapping when obtaining symbols and types from JSDoc nodes.
    fn add_deep_clone_reparse(&mut self, node: Node) -> Node {
        let clone = self.factory.deep_clone_reparse(node);
        if clone.is_some() {
            self.reparsed_clones.push(clone);
        }
        clone
    }

    // Go: parser/reparser.go:34 addTransformedReparse
    fn add_transformed_reparse(&mut self, new_node: Node, old: Node) -> Node {
        self.finish_reparsed_node(new_node, old);
        set_node_flags(
            new_node,
            new_node.flags() | NodeFlags::REPARSER_TRANSFORMED_LITERAL,
        );
        self.reparsed_clones.push(new_node);
        new_node
    }

    // Go: parser/reparser.go:41 checkNonIdentifierName
    fn check_non_identifier_name(&mut self, name: Node) -> Node {
        // Handles the case of anonymous functions
        if name.is_nil() {
            return Node::NIL;
        }
        if is_identifier(name) && !is_valid_identifier(name.text()) {
            let mut err_loc = name.loc();
            if err_loc.len() == 0 {
                // missing name, emit error on the character before the missing name node
                // PORT: Go `pos-1` is one Go byte back (`go_offset_before`).
                let pos = name.loc().pos();
                err_loc = TextRange::new(
                    crate::scanner_util::go_offset_before(self.source_text, pos),
                    pos,
                );
            }
            self.parse_error_at_range(err_loc, diag::Identifier_expected, args![]);
        }
        name
    }

    // Go: parser/reparser.go:58 reparseTags
    /// Hosted tags find a host and add their children to the correct location under the host.
    /// Unhosted tags add synthetic nodes to the reparse list.
    pub fn reparse_tags(&mut self, parent: Node, js_doc: &[Node]) {
        for &j in js_doc {
            let is_last = j == js_doc[js_doc.len() - 1];
            let tags = j.tags();
            if tags.is_nil() {
                continue;
            }
            for tag in tags.nodes().iter() {
                self.reparse_unhosted(tag, parent, j);
                if is_last {
                    self.reparse_hosted(tag, parent, j);
                }
            }
        }
    }

    // Go: parser/reparser.go:74 reparseUnhosted
    fn reparse_unhosted(&mut self, tag: Node, parent: Node, js_doc: Node) {
        match tag.kind() {
            SyntaxKind::JsDocTypedefTag => {
                let type_expression = tag.type_expression();
                if type_expression.is_nil() {
                    return;
                }
                let full_name = tag.name();
                let is_namespace = full_name.is_some() && is_module_declaration(full_name);
                let modifiers = if is_namespace {
                    self.create_export_modifier(tag)
                } else {
                    ModifierList::NIL
                };
                let innermost = self.get_innermost_name_of_js_doc_namespace(full_name);
                let checked = self.check_non_identifier_name(innermost);
                let name = self.add_deep_clone_reparse(checked);
                let type_alias = self.factory.new_js_type_alias_declaration(
                    modifiers,
                    name,
                    NodeList::NIL,
                    Node::NIL,
                );
                let type_parameters =
                    self.gather_type_parameters(js_doc, true /*typedefOrCallback*/);
                let tp = node_list_value(type_alias, type_parameters);
                mutate(type_alias, |data| {
                    if let D::TypeAliasDeclaration(d) = data {
                        d.type_parameters = tp;
                    }
                });
                let t = match type_expression.kind() {
                    SyntaxKind::JsDocTypeExpression => {
                        self.add_deep_clone_reparse(type_expression.type_())
                    }
                    SyntaxKind::JsDocTypeLiteral => {
                        self.reparse_js_doc_type_literal(type_expression)
                    }
                    kind => panic!(
                        "typedef tag type expression should be a name reference or a type expression{kind:?}"
                    ),
                };
                let t = node_child_id(type_alias, t);
                mutate(type_alias, |data| {
                    if let D::TypeAliasDeclaration(d) = data {
                        d.type_ = t;
                    }
                });
                self.finish_reparsed_node(type_alias, tag);
                self.jsdoc_infos.push(JsDocInfo {
                    parent: type_alias,
                    js_docs: vec![js_doc],
                });
                set_node_flags(type_alias, type_alias.flags() | NodeFlags::HAS_JS_DOC);
                let result =
                    self.wrap_in_js_doc_namespace(full_name, type_alias, false /*nested*/);
                self.reparse_list.push(result);
            }
            SyntaxKind::JsDocCallbackTag => {
                let type_expression = tag.type_expression();
                if type_expression.is_nil() {
                    return;
                }
                let full_name = tag.name();
                let is_namespace = full_name.is_some() && is_module_declaration(full_name);
                let modifiers = if is_namespace {
                    self.create_export_modifier(tag)
                } else {
                    ModifierList::NIL
                };
                let function_type = self.reparse_js_doc_signature(
                    type_expression,
                    tag,
                    js_doc,
                    tag,
                    ModifierList::NIL,
                );
                let innermost = self.get_innermost_name_of_js_doc_namespace(full_name);
                let name = self.add_deep_clone_reparse(innermost);
                let type_alias = self.factory.new_js_type_alias_declaration(
                    modifiers,
                    name,
                    NodeList::NIL,
                    function_type,
                );
                let type_parameters =
                    self.gather_type_parameters(js_doc, true /*typedefOrCallback*/);
                let tp = node_list_value(type_alias, type_parameters);
                mutate(type_alias, |data| {
                    if let D::TypeAliasDeclaration(d) = data {
                        d.type_parameters = tp;
                    }
                });
                self.finish_reparsed_node(type_alias, tag);
                self.jsdoc_infos.push(JsDocInfo {
                    parent: type_alias,
                    js_docs: vec![js_doc],
                });
                set_node_flags(type_alias, type_alias.flags() | NodeFlags::HAS_JS_DOC);
                let result =
                    self.wrap_in_js_doc_namespace(full_name, type_alias, false /*nested*/);
                self.reparse_list.push(result);
            }
            SyntaxKind::JsDocImportTag => {
                if tag.import_clause().is_nil() {
                    return;
                }
                let import_clause = self.add_deep_clone_reparse(tag.import_clause());
                mutate(import_clause, |data| {
                    if let D::ImportClause(d) = data {
                        d.phase_modifier = Some(SyntaxKind::TypeKeyword);
                    }
                });
                let modifiers = self.factory.deep_clone_reparse_modifiers(tag.modifiers());
                let module_specifier = self.add_deep_clone_reparse(tag.module_specifier());
                let attributes = self.add_deep_clone_reparse(tag.attributes());
                let import_declaration = self.factory.new_js_import_declaration(
                    modifiers,
                    import_clause,
                    module_specifier,
                    attributes,
                );
                self.finish_reparsed_node(import_declaration, tag);
                self.reparse_list.push(import_declaration);
            }
            SyntaxKind::JsDocOverloadTag => {
                // Create overload signatures only for function, method, and constructor declarations outside object literals
                if (is_function_declaration(parent)
                    || is_method_declaration(parent)
                    || is_constructor_declaration(parent))
                    && self.parsing_contexts & (1 << (ParsingContext::ObjectLiteralMembers as i32))
                        == 0
                {
                    let signature = self.reparse_js_doc_signature(
                        tag.type_expression(),
                        parent,
                        js_doc,
                        tag,
                        parent.modifiers(),
                    );
                    self.reparse_list.push(signature);
                }
            }
            _ => {}
        }
    }

    // Go: parser/reparser.go:146 reparseJSDocSignature
    fn reparse_js_doc_signature(
        &mut self,
        js_signature: Node,
        fun: Node,
        js_doc: Node,
        tag: Node,
        modifiers: ModifierList,
    ) -> Node {
        let cloned_modifiers = self.factory.deep_clone_reparse_modifiers(modifiers);
        let signature = match fun.kind() {
            SyntaxKind::FunctionDeclaration => {
                let checked = self.check_non_identifier_name(fun.name());
                let name = self.factory.deep_clone_reparse(checked);
                self.factory.new_function_declaration(
                    cloned_modifiers,
                    Node::NIL,
                    name,
                    NodeList::NIL,
                    NodeList::NIL,
                    Node::NIL,
                    Node::NIL,
                    Node::NIL,
                )
            }
            SyntaxKind::MethodDeclaration => {
                let checked = self.check_non_identifier_name(fun.name());
                let name = self.factory.deep_clone_reparse(checked);
                self.factory.new_method_declaration(
                    cloned_modifiers,
                    Node::NIL,
                    name,
                    Node::NIL,
                    NodeList::NIL,
                    NodeList::NIL,
                    Node::NIL,
                    Node::NIL,
                    Node::NIL,
                )
            }
            SyntaxKind::Constructor => self.factory.new_constructor_declaration(
                cloned_modifiers,
                NodeList::NIL,
                NodeList::NIL,
                Node::NIL,
                Node::NIL,
                Node::NIL,
            ),
            SyntaxKind::JsDocCallbackTag => {
                let any = self.factory.new_keyword_type_node(SyntaxKind::AnyKeyword);
                self.factory
                    .new_function_type_node(NodeList::NIL, NodeList::NIL, any)
            }
            kind => panic!("Unexpected kind {kind:?}"),
        };

        if tag.kind() != SyntaxKind::JsDocCallbackTag {
            let type_parameters =
                self.gather_type_parameters(js_doc, false /*typedefOrCallback*/);
            set_function_type_parameters(signature, type_parameters);
        }
        let mut parameters: Vec<Node> = Vec::new();
        for (pi, param) in js_signature.parameters().iter().enumerate() {
            let mut parameter = Node::NIL;
            if param.kind() == SyntaxKind::JsDocThisTag {
                let this_ident = self.factory.new_identifier("this");
                set_node_loc(this_ident, param.loc());
                set_node_flags(this_ident, self.context_flags | NodeFlags::REPARSED);
                parameter = self.factory.new_parameter_declaration(
                    ModifierList::NIL,
                    Node::NIL,
                    this_ident,
                    Node::NIL,
                    Node::NIL,
                    Node::NIL,
                );
                if param.type_expression().is_some() {
                    let t = self.add_deep_clone_reparse(param.type_expression().type_());
                    set_type(parameter, t);
                }
            } else if param.kind() == SyntaxKind::JsDocParameterTag
                || param.kind() == SyntaxKind::JsDocPropertyTag
            {
                // Skip sub-property parameters (e.g., @param x.y) - these have QualifiedNames
                // and describe properties of a parent parameter, not standalone parameters.
                if is_qualified_name(param.name()) {
                    continue;
                }
                let mut dot_dot_dot_token = Node::NIL;
                let mut param_type = Node::NIL;

                let type_expression = param.type_expression();
                if type_expression.is_some() {
                    if type_expression.type_().kind() == SyntaxKind::JsDocVariadicType {
                        dot_dot_dot_token = self.factory.new_token(SyntaxKind::DotDotDotToken);
                        set_node_loc(dot_dot_dot_token, param.loc());
                        set_node_flags(dot_dot_dot_token, self.context_flags | NodeFlags::REPARSED);

                        let variadic_type = type_expression.type_();
                        param_type = self.reparse_js_doc_type_literal(variadic_type.type_());
                    } else {
                        param_type = self.reparse_js_doc_type_literal(type_expression.type_());
                    }
                }
                let mut name = param.name();
                if is_identifier(name) && !is_valid_identifier(name.text()) {
                    // drop invalid chars for _, if empty, write _0, etc., so we have a valid param name to emit later
                    let mut result = String::new();
                    for (i, ch) in name.text().char_indices() {
                        if i == 0 {
                            if !is_identifier_start(ch) {
                                result.push('_');
                            } else {
                                result.push(ch);
                            }
                            continue;
                        } else if !is_identifier_part(ch) {
                            result.push('_');
                        } else {
                            result.push(ch);
                        }
                    }
                    if result.is_empty() {
                        result.push('_');
                        result.push_str(&pi.to_string());
                    }
                    let ident = self.factory.new_identifier(result);
                    name = self.add_transformed_reparse(ident, name);
                } else {
                    name = self.add_deep_clone_reparse(name);
                }
                let question = self.make_question_if_optional(param);
                parameter = self.factory.new_parameter_declaration(
                    ModifierList::NIL,
                    dot_dot_dot_token,
                    name,
                    question,
                    param_type,
                    Node::NIL,
                );
            }
            self.finish_reparsed_node(parameter, param);
            parameters.push(parameter);
            self.reparse_js_doc_comment(parameter, param);
        }
        let parameter_list = self.new_node_list(js_signature.parameter_list().loc(), &parameters);
        set_function_parameters(signature, parameter_list);

        if js_signature.type_().is_some() && js_signature.type_().type_expression().is_some() {
            let t = self.add_deep_clone_reparse(js_signature.type_().type_expression().type_());
            set_function_type(signature, t);
        }
        let loc = if tag.kind() == SyntaxKind::JsDocOverloadTag {
            tag.tag_name()
        } else {
            js_signature
        };
        self.finish_reparsed_node(signature, loc);
        signature
    }

    // Go: parser/reparser.go:244 reparseJSDocTypeLiteral
    fn reparse_js_doc_type_literal(&mut self, t: Node) -> Node {
        if t.is_nil() {
            return Node::NIL;
        }
        if t.kind() == SyntaxKind::JsDocTypeLiteral {
            let js_type_literal = t;
            let is_array_type = js_type_literal.is_array_type();
            let mut properties: Vec<Node> = Vec::new();
            for prop in js_type_literal.js_doc_property_tags() {
                if prop.kind() != SyntaxKind::JsDocPropertyTag
                    && prop.kind() != SyntaxKind::JsDocParameterTag
                {
                    continue;
                }
                let mut name = prop.name();
                if name.kind() == SyntaxKind::QualifiedName {
                    name = name.right();
                }
                if is_identifier(name) && !is_valid_identifier(name.text()) {
                    let literal = self
                        .factory
                        .new_string_literal(name.text(), TokenFlags::NONE);
                    name = self.add_transformed_reparse(literal, name);
                } else {
                    name = self.add_deep_clone_reparse(name);
                }
                let question = self.make_question_if_optional(prop);
                let property = self.factory.new_property_signature_declaration(
                    ModifierList::NIL,
                    name,
                    question,
                    Node::NIL,
                    Node::NIL,
                );
                if prop.type_expression().is_some() {
                    let property_type =
                        self.reparse_js_doc_type_literal(prop.type_expression().type_());
                    let id = node_child_id(property, property_type);
                    mutate(property, |data| {
                        if let D::PropertySignatureDeclaration(d) = data {
                            d.type_ = id;
                        }
                    });
                }
                self.finish_reparsed_node(property, prop);
                properties.push(property);
                self.reparse_js_doc_comment(property, prop);
            }
            let members = self.new_node_list(js_type_literal.loc(), &properties);
            let mut t = self.factory.new_type_literal_node(members);
            if is_array_type {
                self.finish_reparsed_node(t, js_type_literal);
                t = self.factory.new_array_type_node(t);
            }
            self.finish_reparsed_node(t, js_type_literal);
            return t;
        }
        self.add_deep_clone_reparse(t)
    }

    // Go: parser/reparser.go:285 reparseJSDocComment
    fn reparse_js_doc_comment(&mut self, node: Node, tag: Node) {
        let comment = tag.comment_list();
        if comment.is_some() {
            let nodes: Vec<Node> = comment
                .nodes()
                .iter()
                .map(|n| self.factory.deep_clone_reparse(n))
                .collect();
            let new_comment = self.factory.new_node_list_with_loc(&nodes, comment.loc());
            let prop_js_doc = self.factory.new_js_doc(new_comment, NodeList::NIL);
            self.finish_reparsed_node(prop_js_doc, tag);
            set_node_parent(prop_js_doc, node);
            self.jsdoc_infos.push(JsDocInfo {
                parent: node,
                js_docs: vec![prop_js_doc],
            });
            set_node_flags(node, node.flags() | NodeFlags::HAS_JS_DOC);
        }
    }

    // Go: parser/reparser.go:297 gatherTypeParameters
    fn gather_type_parameters(&mut self, j: Node, typedef_or_callback: bool) -> NodeList {
        let mut type_parameters: Vec<Node> = Vec::new();
        let mut pos = -1;
        let mut end_pos = -1;
        let mut first_template = true;
        for tag in j.tags().nodes().iter() {
            // When a JSDoc comment contains an `@typedef` or `@callback` tag, `@template` type parameter
            // declarations apply to the type being defined.
            if !typedef_or_callback && (is_js_doc_typedef_tag(tag) || is_js_doc_callback_tag(tag)) {
                return NodeList::NIL;
            }
            if !is_js_doc_template_tag(tag) {
                continue;
            }
            if first_template {
                pos = tag.pos();
                first_template = false;
            }
            end_pos = tag.end();
            let constraint = tag.constraint();
            let mut first_type_parameter = true;
            for tp in tag.type_parameters().iter() {
                let reparse;
                if constraint.is_some() && first_type_parameter {
                    let modifiers = self.factory.deep_clone_reparse_modifiers(tp.modifiers());
                    let checked = self.check_non_identifier_name(tp.name());
                    let name = self.add_deep_clone_reparse(checked);
                    let constraint_type = self.add_deep_clone_reparse(constraint.type_());
                    let default_type = self.add_deep_clone_reparse(tp.default_type());
                    reparse = self.factory.new_type_parameter_declaration(
                        modifiers,
                        name,
                        constraint_type,
                        Node::NIL, // expression
                        default_type,
                    );
                    self.finish_reparsed_node(reparse, tp);
                } else {
                    reparse = self.add_deep_clone_reparse(tp);
                }
                type_parameters.push(reparse);
                first_type_parameter = false;
            }
        }
        if type_parameters.is_empty() {
            NodeList::NIL
        } else {
            self.new_node_list(TextRange::new(pos, end_pos), &type_parameters)
        }
    }

    // Go: parser/reparser.go:346 reparseHosted
    fn reparse_hosted(&mut self, tag: Node, parent: Node, js_doc: Node) {
        let mut parent = parent;
        match tag.kind() {
            SyntaxKind::JsDocTypeTag => {
                match parent.kind() {
                    SyntaxKind::VariableStatement => {
                        if parent.declaration_list().is_some() {
                            for declaration in
                                parent.declaration_list().declarations().nodes().iter()
                            {
                                if declaration.type_().is_nil() && tag.type_expression().is_some() {
                                    let t =
                                        self.add_deep_clone_reparse(tag.type_expression().type_());
                                    set_type(declaration, t);
                                    self.finish_mutated_node(declaration);
                                    return;
                                }
                            }
                        }
                    }
                    SyntaxKind::VariableDeclaration
                    | SyntaxKind::ExportAssignment
                    | SyntaxKind::PropertyDeclaration
                    | SyntaxKind::PropertyAssignment
                    | SyntaxKind::ShorthandPropertyAssignment
                    | SyntaxKind::GetAccessor => {
                        if parent.type_().is_nil() && tag.type_expression().is_some() {
                            let t = self.add_deep_clone_reparse(tag.type_expression().type_());
                            set_type(parent, t);
                            self.finish_mutated_node(parent);
                            return;
                        }
                    }
                    SyntaxKind::Parameter => {
                        if parent.type_().is_nil() && tag.type_expression().is_some() {
                            let t = self.reparse_js_doc_type_literal(tag.type_expression().type_());
                            set_type(parent, t);
                            self.finish_mutated_node(parent);
                            return;
                        }
                    }
                    SyntaxKind::ExpressionStatement => {
                        if parent.expression().kind() == SyntaxKind::BinaryExpression {
                            let bin = parent.expression();
                            let kind = get_assignment_declaration_kind(bin);
                            if kind != JSDeclarationKind::NONE && tag.type_expression().is_some() {
                                let t = self.add_deep_clone_reparse(tag.type_expression().type_());
                                set_type(bin, t);
                                self.finish_mutated_node(bin);
                                return;
                            }
                        }
                    }
                    SyntaxKind::ReturnStatement | SyntaxKind::ParenthesizedExpression => {
                        if parent.expression().is_some() && tag.type_expression().is_some() {
                            let t = self.add_deep_clone_reparse(tag.type_expression().type_());
                            let cast = self.make_new_cast(
                                t,
                                parent.expression(),
                                true, /*isAssertion*/
                            );
                            set_expression(parent, cast);
                            self.finish_mutated_node(parent);
                            return;
                        }
                    }
                    _ => {}
                }
                let fun = get_function_like_host(parent);
                if fun.is_some() {
                    let no_typed_params =
                        fun.parameters().iter().all(|param| param.type_().is_nil());
                    if fun.type_parameter_list().is_nil()
                        && fun.type_().is_nil()
                        && no_typed_params
                        && tag.type_expression().is_some()
                    {
                        let t = self.add_deep_clone_reparse(tag.type_expression().type_());
                        set_function_full_signature(fun, t);
                        self.finish_mutated_node(fun);
                    }
                }
            }
            SyntaxKind::JsDocSatisfiesTag => match parent.kind() {
                SyntaxKind::VariableStatement => {
                    if parent.declaration_list().is_some() {
                        for declaration in parent.declaration_list().declarations().nodes().iter() {
                            if declaration.initializer().is_some()
                                && tag.type_expression().is_some()
                            {
                                let t = self.add_deep_clone_reparse(tag.type_expression().type_());
                                let cast = self.make_new_cast(
                                    t,
                                    declaration.initializer(),
                                    false, /*isAssertion*/
                                );
                                set_initializer(declaration, cast);
                                self.finish_mutated_node(declaration);
                                break;
                            }
                        }
                    }
                }
                SyntaxKind::VariableDeclaration
                | SyntaxKind::PropertyDeclaration
                | SyntaxKind::PropertyAssignment => {
                    if parent.initializer().is_some() && tag.type_expression().is_some() {
                        let t = self.add_deep_clone_reparse(tag.type_expression().type_());
                        let cast =
                            self.make_new_cast(t, parent.initializer(), false /*isAssertion*/);
                        set_initializer(parent, cast);
                        self.finish_mutated_node(parent);
                    }
                }
                SyntaxKind::ShorthandPropertyAssignment => {
                    let initializer = parent.object_assignment_initializer();
                    if initializer.is_some() && tag.type_expression().is_some() {
                        let t = self.add_deep_clone_reparse(tag.type_expression().type_());
                        let cast = self.make_new_cast(t, initializer, false /*isAssertion*/);
                        let id = node_opt_child_id(parent, cast);
                        mutate(parent, |data| {
                            if let D::ShorthandPropertyAssignment(d) = data {
                                d.object_assignment_initializer = id;
                            }
                        });
                        self.finish_mutated_node(parent);
                    }
                }
                SyntaxKind::ReturnStatement
                | SyntaxKind::ParenthesizedExpression
                | SyntaxKind::ExportAssignment => {
                    if parent.expression().is_some() && tag.type_expression().is_some() {
                        let t = self.add_deep_clone_reparse(tag.type_expression().type_());
                        let cast =
                            self.make_new_cast(t, parent.expression(), false /*isAssertion*/);
                        set_expression(parent, cast);
                        self.finish_mutated_node(parent);
                    }
                }
                SyntaxKind::ExpressionStatement => {
                    if parent.expression().kind() == SyntaxKind::BinaryExpression {
                        let bin = parent.expression();
                        let kind = get_assignment_declaration_kind(bin);
                        if kind != JSDeclarationKind::NONE && tag.type_expression().is_some() {
                            let t = self.add_deep_clone_reparse(tag.type_expression().type_());
                            let cast =
                                self.make_new_cast(t, bin.right(), false /*isAssertion*/);
                            let id = node_child_id(bin, cast);
                            mutate(bin, |data| {
                                if let D::BinaryExpression(d) = data {
                                    d.right = id;
                                }
                            });
                            self.finish_mutated_node(bin);
                        }
                    }
                }
                _ => {}
            },
            SyntaxKind::JsDocTemplateTag => {
                let fun = get_function_like_host(parent);
                if fun.is_some() {
                    // PORT: Go `fun.TypeParameters() == nil` tests the list's Nodes, which
                    // the Go parser leaves nil for an empty `<>` (Arena.Clone of an empty
                    // slice is nil), so an empty list is replaced too.
                    if fun.type_parameters().is_empty() && fun.full_signature().is_nil() {
                        let type_parameters =
                            self.gather_type_parameters(js_doc, false /*typedefOrCallback*/);
                        set_function_type_parameters(fun, type_parameters);
                        self.finish_mutated_node(fun);
                    }
                } else if parent.kind() == SyntaxKind::ClassDeclaration
                    || parent.kind() == SyntaxKind::ClassExpression
                {
                    // PORT: Go has one branch per class kind with the same body.
                    if parent.type_parameter_list().is_nil() {
                        let type_parameters =
                            self.gather_type_parameters(js_doc, false /*typedefOrCallback*/);
                        let v = node_list_value(parent, type_parameters);
                        mutate(parent, |data| match data {
                            D::ClassDeclaration(d) => d.type_parameters = v,
                            D::ClassExpression(d) => d.type_parameters = v,
                            _ => unreachable!(),
                        });
                        self.finish_mutated_node(parent);
                    }
                }
            }
            SyntaxKind::JsDocParameterTag => {
                let fun = get_function_like_host(parent);
                if fun.is_some() && fun.full_signature().is_nil() {
                    let parameter_tag = tag;
                    let param = find_matching_parameter(fun, parameter_tag, js_doc);
                    if param.is_some() {
                        let mut new_type = None;
                        if param.type_().is_nil() && parameter_tag.type_expression().is_some() {
                            let t = self.reparse_js_doc_type_literal(
                                parameter_tag.type_expression().type_(),
                            );
                            new_type = Some(node_opt_child_id(param, t));
                        }
                        let mut new_question = None;
                        if param.question_token().is_nil() {
                            let question = self.make_question_if_optional(parameter_tag);
                            if question.is_some() {
                                new_question = Some(node_opt_child_id(param, question));
                            }
                        }
                        mutate(param, |data| {
                            if let D::ParameterDeclaration(d) = data {
                                if let Some(t) = new_type {
                                    d.type_ = t;
                                }
                                if let Some(q) = new_question {
                                    d.question_token = q;
                                }
                            }
                        });
                        self.finish_mutated_node(param);
                    }
                }
            }
            SyntaxKind::JsDocThisTag => {
                let fun = get_function_like_host(parent);
                if fun.is_some() {
                    let params = fun.parameters();
                    if params.is_empty()
                        || (params.get(0).name().kind() != SyntaxKind::ThisKeyword
                            && !is_this_identifier(params.get(0).name()))
                    {
                        let this_ident = self.factory.new_identifier("this");
                        let this_param = self.factory.new_parameter_declaration(
                            ModifierList::NIL, /* modifiers */
                            Node::NIL,         /* dotDotDotToken */
                            this_ident,
                            Node::NIL, /* questionToken */
                            Node::NIL, /* type */
                            Node::NIL, /* initializer */
                        );
                        if tag.type_expression().is_some() {
                            let t = self.add_deep_clone_reparse(tag.type_expression().type_());
                            set_type(this_param, t);
                        }
                        self.finish_reparsed_node(this_param, tag.tag_name());

                        let mut new_params: Vec<Node> = Vec::with_capacity(params.len() + 1);
                        new_params.push(this_param);
                        new_params.extend(params.iter());

                        let list = self.new_node_list(fun.parameter_list().loc(), &new_params);
                        set_function_parameters(fun, list);
                        self.finish_mutated_node(fun);
                    }
                }
            }
            SyntaxKind::JsDocReturnTag => {
                let fun = get_function_like_host(parent);
                if fun.is_some()
                    && fun.full_signature().is_nil()
                    && fun.type_().is_nil()
                    && tag.type_expression().is_some()
                {
                    let t = self.add_deep_clone_reparse(tag.type_expression().type_());
                    set_function_type(fun, t);
                    self.finish_mutated_node(fun);
                }
            }
            SyntaxKind::JsDocReadonlyTag
            | SyntaxKind::JsDocPrivateTag
            | SyntaxKind::JsDocPublicTag
            | SyntaxKind::JsDocProtectedTag
            | SyntaxKind::JsDocOverrideTag => {
                if parent.kind() == SyntaxKind::ExpressionStatement {
                    parent = parent.expression();
                }
                // In object literals these aren't class-like members, so JSDoc modifiers like @override
                // or @readonly aren't real modifiers there; reparsing them produces spurious grammar errors (#4437).
                // PORT: Go returns in the MethodDeclaration, GetAccessor and
                // SetAccessor case and falls through to the shared body otherwise.
                if matches!(
                    parent.kind(),
                    SyntaxKind::MethodDeclaration
                        | SyntaxKind::GetAccessor
                        | SyntaxKind::SetAccessor
                ) && self.parsing_contexts & (1 << (ParsingContext::ObjectLiteralMembers as i32))
                    != 0
                {
                    return;
                }
                match parent.kind() {
                    SyntaxKind::PropertyDeclaration
                    | SyntaxKind::MethodDeclaration
                    | SyntaxKind::Constructor
                    | SyntaxKind::GetAccessor
                    | SyntaxKind::SetAccessor
                    | SyntaxKind::BinaryExpression => {
                        let keyword = match tag.kind() {
                            SyntaxKind::JsDocReadonlyTag => SyntaxKind::ReadonlyKeyword,
                            SyntaxKind::JsDocPrivateTag => SyntaxKind::PrivateKeyword,
                            SyntaxKind::JsDocPublicTag => SyntaxKind::PublicKeyword,
                            SyntaxKind::JsDocProtectedTag => SyntaxKind::ProtectedKeyword,
                            _ => SyntaxKind::OverrideKeyword,
                        };
                        let modifier = self.factory.new_modifier(keyword);
                        set_node_loc(modifier, tag.loc());
                        set_node_flags(modifier, self.context_flags | NodeFlags::REPARSED);
                        let nodes: Vec<Node>;
                        let loc;
                        if parent.modifiers().is_nil() {
                            nodes = vec![modifier];
                            loc = tag.loc();
                        } else {
                            let mut v = parent.modifier_nodes().to_vec();
                            v.push(modifier);
                            nodes = v;
                            loc = parent.modifiers().loc();
                        }
                        let list = self.new_modifier_list(loc, &nodes);
                        set_modifiers(parent, list);
                        self.finish_mutated_node(parent);
                    }
                    _ => {}
                }
            }
            SyntaxKind::JsDocImplementsTag => {
                let class = get_class_like_data(parent);
                if class.is_some() {
                    let class_name = tag.class_name();

                    let heritage_clauses = class.heritage_clauses();
                    if heritage_clauses.is_some() {
                        let implements_clause = heritage_clauses
                            .nodes()
                            .iter()
                            .find(|node| node.token() == SyntaxKind::ImplementsKeyword)
                            .unwrap_or(Node::NIL);
                        if implements_clause.is_some() {
                            // PORT: Go appends to `Types.Nodes` in place. Here a new list with the same Loc.
                            let old_types = implements_clause.types();
                            let mut types = old_types.nodes().to_vec();
                            types.push(self.add_deep_clone_reparse(class_name));
                            let list = self.factory.new_node_list_with_loc(&types, old_types.loc());
                            let v = node_req_list_value(implements_clause, list);
                            mutate(implements_clause, |data| {
                                if let D::HeritageClause(d) = data {
                                    d.types = v;
                                }
                            });
                            self.finish_mutated_node(implements_clause);
                            return;
                        }
                    }
                    let clone = self.add_deep_clone_reparse(class_name);
                    let types_list = self.new_node_list(class_name.loc(), &[clone]);

                    let heritage_clause = self
                        .factory
                        .new_heritage_clause(SyntaxKind::ImplementsKeyword, types_list);
                    self.finish_reparsed_node(heritage_clause, class_name);

                    let new_heritage_clauses = if heritage_clauses.is_nil() {
                        self.new_node_list(class_name.loc(), &[heritage_clause])
                    } else {
                        // PORT: Go appends to `HeritageClauses.Nodes` in place. Here a new list with the same Loc.
                        let mut clauses = heritage_clauses.nodes().to_vec();
                        clauses.push(heritage_clause);
                        self.factory
                            .new_node_list_with_loc(&clauses, heritage_clauses.loc())
                    };
                    let v = node_list_value(class, new_heritage_clauses);
                    mutate(class, |data| match data {
                        D::ClassDeclaration(d) => d.heritage_clauses = v,
                        D::ClassExpression(d) => d.heritage_clauses = v,
                        _ => unreachable!(),
                    });
                    self.finish_mutated_node(parent);
                }
            }
            SyntaxKind::JsDocAugmentsTag => {
                let class = get_class_like_data(parent);
                if class.is_some() && class.heritage_clauses().is_some() {
                    let extends_clause = class
                        .heritage_clauses()
                        .nodes()
                        .iter()
                        .find(|node| node.token() == SyntaxKind::ExtendsKeyword)
                        .unwrap_or(Node::NIL);
                    if extends_clause.is_some() && extends_clause.types().nodes().len() == 1 {
                        let target = extends_clause.types().nodes().get(0);
                        let source = tag.class_name();
                        if target.type_argument_list().is_nil()
                            && source.type_argument_list().is_some()
                            && target.expression().kind() != SyntaxKind::NullKeyword
                            && (!is_entity_name_expression(target.expression())
                                || has_same_property_access_name(
                                    target.expression(),
                                    source.expression(),
                                ))
                        {
                            self.set_reparsed_type_arguments(target, source.type_argument_list());
                        }
                    }
                }
            }
            _ => {}
        }
    }

    // Go: parser/reparser.go:608 setReparsedTypeArguments
    fn set_reparsed_type_arguments(&mut self, target: Node, source: NodeList) {
        let mut type_arguments: Vec<Node> = Vec::with_capacity(source.nodes().len());
        for type_argument in source.nodes().iter() {
            type_arguments.push(self.add_deep_clone_reparse(type_argument));
        }
        let list = self.new_node_list(source.loc(), &type_arguments);
        let v = node_list_value(target, list);
        mutate(target, |data| {
            if let D::ExpressionWithTypeArguments(d) = data {
                d.type_arguments = v;
            }
        });
        self.finish_mutated_node(target);
    }

    // Go: parser/reparser.go:617 makeQuestionIfOptional
    fn make_question_if_optional(&mut self, parameter: Node) -> Node {
        let mut question_token = Node::NIL;
        if parameter.is_bracketed()
            || parameter.type_expression().is_some()
                && parameter.type_expression().type_().kind() == SyntaxKind::JsDocOptionalType
        {
            question_token = self.factory.new_token(SyntaxKind::QuestionToken);
            set_node_loc(question_token, parameter.loc());
            set_node_flags(question_token, self.context_flags | NodeFlags::REPARSED);
        }
        question_token
    }

    // Go: parser/reparser.go:680 makeNewCast
    fn make_new_cast(&mut self, t: Node, e: Node, is_assertion: bool) -> Node {
        let assert = if is_assertion {
            self.factory.new_as_expression(e, t)
        } else {
            self.factory.new_satisfies_expression(e, t)
        };
        self.finish_node_with_end(assert, e.pos(), e.end());
        assert
    }

    // Go: parser/reparser.go:702 createExportModifier
    fn create_export_modifier(&mut self, location_node: Node) -> ModifierList {
        let export_modifier = self.factory.new_modifier(SyntaxKind::ExportKeyword);
        set_node_loc(export_modifier, location_node.loc());
        set_node_flags(export_modifier, self.context_flags | NodeFlags::REPARSED);
        self.new_modifier_list(location_node.loc(), &[export_modifier])
    }

    // Go: parser/reparser.go:713 getInnermostNameOfJSDocNamespace
    /// Returns the innermost identifier from a JSDoc namespace chain
    /// (ModuleDeclaration). For a simple identifier, it returns the identifier
    /// itself. For "A.B.C", it returns the identifier "C".
    fn get_innermost_name_of_js_doc_namespace(&self, full_name: Node) -> Node {
        let mut full_name = full_name;
        if full_name.is_nil() {
            return Node::NIL;
        }
        while full_name.kind() == SyntaxKind::ModuleDeclaration {
            let body = full_name.body();
            if body.is_nil() {
                return full_name.name();
            }
            full_name = body;
        }
        full_name
    }

    // Go: parser/reparser.go:735 wrapInJSDocNamespace
    /// Wraps a statement (typically a type alias) in namespace declarations
    /// for a JSDoc dotted name. For name "A.B.C" and a type alias for C, this
    /// makes `namespace A { namespace B { type C = ... } }`. If the name is a
    /// simple identifier (not a ModuleDeclaration), it returns the statement.
    fn wrap_in_js_doc_namespace(&mut self, full_name: Node, statement: Node, nested: bool) -> Node {
        if full_name.is_nil() || !is_module_declaration(full_name) {
            return statement;
        }
        // Recursively wrap from outermost to innermost. Inner namespaces always get an export modifier
        // so members are accessible via dotted access from outside. The outermost namespace is treated as
        // exported only in module files via IsImplicitlyExportedJSDocDeclaration (in the binder), so it
        // does not get an explicit export modifier here.
        let wrapped =
            self.wrap_in_js_doc_namespace(full_name.body(), statement, true /*nested*/);
        let statements = self.new_node_list(full_name.loc(), &[wrapped]);
        let block = self.factory.new_module_block(statements);
        self.finish_reparsed_node(block, full_name);
        let modifiers = if nested {
            self.create_export_modifier(full_name)
        } else {
            ModifierList::NIL
        };
        let name = self.add_deep_clone_reparse(full_name.name());
        let result = self.factory.new_module_declaration(
            modifiers,
            SyntaxKind::NamespaceKeyword,
            name,
            Node::NIL,
            block,
        );
        self.finish_reparsed_node(result, full_name);
        self.reparsed_clones.push(result);
        result
    }
}

// Go: parser/reparser.go:627 findMatchingParameter
// PORT: returns the parameter node, or nil for Go `(nil, false)`.
fn find_matching_parameter(fun: Node, parameter_tag: Node, js_doc: Node) -> Node {
    let mut tag_index: i64 = -1;
    let mut param_count: i64 = -1;
    for tag in js_doc.tags().nodes().iter() {
        if tag.kind() == SyntaxKind::JsDocParameterTag {
            param_count += 1;
            if tag == parameter_tag {
                tag_index = param_count;
                break;
            }
        }
    }
    for (parameter_index, parameter) in fun.parameters().iter().enumerate() {
        let parameter_index = parameter_index as i64;
        if parameter.name().kind() == SyntaxKind::Identifier {
            if parameter_tag.name().kind() == SyntaxKind::Identifier
                && ((parameter.name().text() == parameter_tag.name().text())
                    || (parameter_index == tag_index && parameter_tag.name().text().is_empty()))
            {
                return parameter;
            }
        } else if parameter_index == tag_index {
            return parameter;
        }
    }
    Node::NIL
}

// Go: parser/reparser.go:652 skipSatisfiesExpressions
fn skip_satisfies_expressions(node: Node) -> Node {
    let mut node = node;
    while node.is_some() && node.kind() == SyntaxKind::SatisfiesExpression {
        node = node.expression();
    }
    node
}

// Go: parser/reparser.go:659 getFunctionLikeHost
fn get_function_like_host(host: Node) -> Node {
    let mut fun = host;
    match host.kind() {
        SyntaxKind::VariableStatement => {
            let nodes = host.declaration_list().declarations().nodes();
            if !nodes.is_empty() {
                fun = nodes.get(0).initializer();
            }
        }
        SyntaxKind::PropertyAssignment | SyntaxKind::PropertyDeclaration => {
            fun = host.initializer();
        }
        SyntaxKind::ExportAssignment | SyntaxKind::ReturnStatement => {
            fun = host.expression();
        }
        SyntaxKind::ExpressionStatement => {
            fun = get_right_most_assigned_expression(host.expression());
        }
        _ => {}
    }
    fun = skip_satisfies_expressions(fun);
    if is_function_like(fun) {
        return fun;
    }
    Node::NIL
}

// Go: parser/reparser.go:691 getClassLikeData
// PORT: Go returns the `*ClassLikeBase` data. Here the class node (or nil);
// callers read and write its fields through the node.
fn get_class_like_data(parent: Node) -> Node {
    match parent.kind() {
        SyntaxKind::ClassDeclaration | SyntaxKind::ClassExpression => parent,
        _ => Node::NIL,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::synthetic::{free_synthetic_nodes, free_synthetic_owner, open_synthetic_owner};
    use crate::core::{GoProgram, enter_program, next_program_id};

    // A data write (`mutate`) on a synthetic node puts the lists of its new
    // values in the owner of the node, with its new data, not in the current
    // owner: a base node changed while a program version is current keeps
    // them after that version is freed. It is the astshrink2 shape of
    // `set_node_modifiers`; the reparser writes store nodes today, so no run
    // reaches it.
    #[test]
    fn data_writes_put_their_lists_in_the_node_owner() {
        std::thread::spawn(|| {
            let program: &'static GoProgram = Box::leak(Box::new(GoProgram {
                id: next_program_id(),
                options: Box::leak(Box::default()),
                state: std::sync::OnceLock::new(),
            }));
            let f = NodeFactory::new();
            let a = f.new_identifier("a");
            let export = f.new_modifier(SyntaxKind::ExportKeyword);
            let fun = f.new_function_type_node(NodeList::NIL, f.new_node_list(&[]), Node::NIL);
            let bin = f.new_binary_expression(
                ModifierList::NIL,
                f.new_identifier("x"),
                Node::NIL,
                f.new_token(SyntaxKind::EqualsToken),
                f.new_identifier("y"),
            );
            let (parameters, type_parameters) = (f.new_node_list(&[a]), f.new_node_list(&[a]));
            let modifiers = f.new_modifier_list(&[export]);

            open_synthetic_owner(program.id);
            {
                let _program = enter_program(Some(program));
                set_function_parameters(fun, parameters);
                set_function_type_parameters(fun, type_parameters);
                set_modifiers(bin, modifiers);
            }
            free_synthetic_owner(program.id);

            assert_eq!(fun.parameter_list().nodes().to_vec(), vec![a]);
            assert_eq!(fun.type_parameter_list().nodes().to_vec(), vec![a]);
            assert_eq!(bin.modifiers().nodes().to_vec(), vec![export]);
            free_synthetic_nodes();
        })
        .join()
        .expect("test thread panicked");
    }
}
