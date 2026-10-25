//! Port of Go `ast.NodeFactory` (`ast/ast.go`, `ast/ast_generated.go`).
//!
//! Every `New*` constructor that the checker, the node builder and the printer
//! use. A factory allocates in the synthetic arena (`ast/synthetic.rs`), or,
//! when made with `NodeFactory::for_file`, in the node store of one parsed
//! file (`ast/store.rs`).
//!
//! Argument rules:
//! - Go `*Node` is `Node`; Go `nil` is `Node::NIL`.
//! - Go `*NodeList` is `NodeList`; Go `*ModifierList` is `ModifierList`.
//!   `NodeList::NIL` / `ModifierList::NIL` is Go `nil`.
//! - A node of another file (or a parsed node for a synthetic target) may be
//!   passed as a child. The new node then refers to that same node (Go
//!   shares the pointer) through an alias slot; see `synthetic.rs` and
//!   `store.rs`.
//!
//! Go `newNode` does not set parents. Callers set `Parent` when Go does, with
//! `set_node_parent`.

use crate::astdata::AstData;
use crate::prelude::*;

/// Go `*ast.NodeFactory`.
// PORT: Go `NodeFactoryHooks` (OnCreate/OnUpdate/OnClone) live in
// `ast/update.rs`; only the printer's emit context sets them. The counters use
// `Cell` so a shared `&NodeFactory` can create nodes while the checker is
// borrowed.
#[derive(Debug, Default)]
pub struct NodeFactory {
    hooks: NodeFactoryHooks,
    node_count: std::cell::Cell<usize>,
    text_count: std::cell::Cell<usize>,
    target: NodeFactoryTarget,
    /// Set on the factory of the checker's to-string node builder
    /// (`Checker::get_node_builder`). In a print scope
    /// (`ast::synthetic::enter_print_scope`) its new nodes and lists belong
    /// to the print owner, which frees them when the to-string call ends.
    print_owned: std::cell::Cell<bool>,
}

/// Where a `NodeFactory` puts new nodes.
// PORT: Go allocates every node on the heap. Here the parser's factory
// writes into the store of the file it parses, and every other factory into
// the synthetic arena.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NodeFactoryTarget {
    /// The synthetic arena (checker, node builder, printer, transforms).
    #[default]
    Synthetic,
    /// The node store with this id (the ported parser).
    File(usize),
}

/// ts_ast token flags from Go token flags.
fn token_flags(flags: TokenFlags) -> crate::astdata::TokenFlags {
    crate::astdata::TokenFlags(flags.bits() as u32)
}

const NO_TOKEN_FLAGS: crate::astdata::TokenFlags = crate::astdata::TokenFlags(0);

impl NodeFactory {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The parser's factory: new nodes and lists go into node store `store`
    /// (`new_file_store`).
    #[must_use]
    pub fn for_file(store: usize) -> Self {
        Self {
            target: NodeFactoryTarget::File(store),
            ..Self::default()
        }
    }

    /// Makes this the factory of a to-string node builder (see
    /// `print_owned`).
    pub fn set_print_owned(&self) {
        self.print_owned.set(true);
    }

    /// A synthetic list of `nodes` with `loc`, for code that builds lists
    /// beside this factory (deep clone, node copy). It goes where this
    /// factory's own lists go: the print owner for a to-string factory in a
    /// print scope. Other factories (and a parser factory) give it to the
    /// current owner, like `ast::synthetic::new_synthetic_node_list`.
    #[must_use]
    pub fn new_synthetic_node_list(&self, nodes: &[Node], loc: TextRange) -> NodeList {
        crate::ast::synthetic::new_synthetic_node_list_in(self.print_owned.get(), nodes, loc)
    }

    /// `new_synthetic_node_list` for a modifier list.
    #[must_use]
    pub fn new_synthetic_modifier_list(&self, nodes: &[Node], loc: TextRange) -> ModifierList {
        crate::ast::synthetic::new_synthetic_modifier_list_in(self.print_owned.get(), nodes, loc)
    }

    /// Where this factory puts new nodes.
    #[must_use]
    pub fn target(&self) -> NodeFactoryTarget {
        self.target
    }

    /// Target-space id of a required child.
    pub(crate) fn id(&self, n: Node) -> crate::astdata::NodeId {
        match self.target {
            NodeFactoryTarget::Synthetic => synthetic_child_id(n),
            NodeFactoryTarget::File(store) => store_child_id(store, n),
        }
    }

    /// Target-space id of an optional child.
    pub(crate) fn oid(&self, n: Node) -> Option<crate::astdata::NodeId> {
        match self.target {
            NodeFactoryTarget::Synthetic => synthetic_opt_child_id(n),
            NodeFactoryTarget::File(store) => store_opt_child_id(store, n),
        }
    }

    /// A required list field.
    pub(crate) fn req_list(&self, l: NodeList) -> crate::astdata::NodeList {
        match self.target {
            NodeFactoryTarget::Synthetic => synthetic_req_list_value(self.print_owned.get(), l),
            NodeFactoryTarget::File(store) => store_req_list_value(store, l),
        }
    }

    /// An optional list field.
    pub(crate) fn opt_list(&self, l: NodeList) -> Option<crate::astdata::NodeList> {
        match self.target {
            NodeFactoryTarget::Synthetic => synthetic_list_value(self.print_owned.get(), l),
            NodeFactoryTarget::File(store) => store_list_value(store, l),
        }
    }

    /// A modifiers field.
    pub(crate) fn mods(&self, m: ModifierList) -> Option<crate::astdata::ModifierList> {
        match self.target {
            NodeFactoryTarget::Synthetic => synthetic_modifiers_value(self.print_owned.get(), m),
            NodeFactoryTarget::File(store) => store_modifiers_value(store, m),
        }
    }

    // Go: ast/ast.go:72 NewNodeFactory
    #[must_use]
    pub fn new_with_hooks(hooks: NodeFactoryHooks) -> Self {
        Self {
            hooks,
            ..Self::default()
        }
    }

    /// Go `f.hooks`.
    pub(crate) fn hooks(&self) -> &NodeFactoryHooks {
        &self.hooks
    }

    // Go: ast/ast.go:85 (f *NodeFactory) newNode
    // PERF: astchunks2. `data` comes by value: a store node of a static
    // parse moves it into the AST arena (no box), as Go's typed arenas.
    // Synthetic nodes box it.
    pub(crate) fn new_node<T: AstData>(&self, kind: SyntaxKind, data: T) -> Node {
        self.node_count.set(self.node_count.get() + 1);
        let node = match self.target {
            NodeFactoryTarget::Synthetic if self.print_owned.get() => {
                crate::ast::synthetic::alloc_synthetic_print_node(kind, data.into_boxed())
            }
            NodeFactoryTarget::Synthetic => alloc_synthetic_node(kind, data.into_boxed()),
            NodeFactoryTarget::File(store) => alloc_store_node(store, kind, data),
        };
        // Go: ast.go:79
        if let Some(h) = &self.hooks.on_create {
            h(node);
        }
        node
    }

    /// `newNode` plus Go `f.textCount++`, for nodes that carry text.
    pub(crate) fn new_text_node<T: AstData>(&self, kind: SyntaxKind, data: T) -> Node {
        self.text_count.set(self.text_count.get() + 1);
        self.new_node(kind, data)
    }

    /// `new_text_node` for an Identifier or PrivateIdentifier with Go text
    /// `text`. `data` makes the node data from the text it holds.
    // PERF: U1 (d). A store node gets an empty data text, which needs no
    // allocation. The store keeps `text` in the name word of the slot,
    // and `Node::text` reads it there. A synthetic node keeps the text in
    // its data.
    // PERF: S1. With an empty text the store payload is always the same
    // (no flow node either), so the slot points at one shared ts_ast node
    // (`alloc_store_shared_name_node`) and `data` is not called. Debug
    // builds call it and check that it equals the shared payload.
    fn new_name_node<T: AstData>(
        &self,
        kind: SyntaxKind,
        text: impl AsRef<str> + Into<String>,
        data: impl FnOnce(String) -> T,
    ) -> Node {
        let NodeFactoryTarget::File(store) = self.target else {
            return self.new_text_node(kind, data(text.into()));
        };
        self.text_count.set(self.text_count.get() + 1);
        self.node_count.set(self.node_count.get() + 1);
        #[cfg(debug_assertions)]
        debug_assert_shared_name_data(kind, data(String::new()).view());
        let node = alloc_store_shared_name_node(store, kind, text.as_ref());
        // Go: ast.go:79
        if let Some(h) = &self.hooks.on_create {
            h(node);
        }
        node
    }

    /// `newNode` plus Go `node.Flags |= flags & NodeFlagsOptionalChain`.
    // PERF: parsefast2. A node flag read and write is a store access, not a
    // field write. Without the bit the Go write keeps the flags as they
    // are, so it is skipped (most property accesses and calls).
    fn new_chain_node<T: AstData>(&self, kind: SyntaxKind, data: T, flags: NodeFlags) -> Node {
        let node = self.new_node(kind, data);
        if flags.intersects(NodeFlags::OPTIONAL_CHAIN) {
            set_node_flags(node, node.flags() | NodeFlags::OPTIONAL_CHAIN);
        }
        node
    }

    // Go: ast/ast.go:90 NodeCount
    #[must_use]
    pub fn node_count(&self) -> usize {
        self.node_count.get()
    }

    // Go: ast/ast.go:94 TextCount
    #[must_use]
    pub fn text_count(&self) -> usize {
        self.text_count.get()
    }

    /// Adds `nodes` to `node_count` and `texts` to `text_count`: the counts
    /// of the nodes that a replayed speculative JSDoc parse made in Go
    /// (`Parser::speculate_js_doc_tag`).
    pub(crate) fn add_counts(&self, nodes: usize, texts: usize) {
        self.node_count.set(self.node_count.get() + nodes);
        self.text_count.set(self.text_count.get() + texts);
    }

    // ── Source files ───────────────────────────────────────────────────

    // Go: ast/ast.go:2451 NewSourceFile
    // PORT: Go takes `opts SourceFileParseOptions`; only its `FileName` and
    // `Path` are kept (see `SyntheticSourceFileData`). A synthetic
    // SourceFile keeps the Go fields in its slot. A store SourceFile (the
    // ported parser) keeps file name and text in its node store.
    // ts#64159 removes the panic on a file name that is not normalized and
    // absolute: Go `opts.FileName` is a `RootedFilePath` now.
    pub fn new_source_file(
        &self,
        file_name: &'static str,
        path: &str,
        text: impl Into<FileText>,
        statements: NodeList,
        end_of_file_token: Node,
    ) -> Node {
        let node = self.new_node(
            SyntaxKind::SourceFile,
            crate::astdata::SourceFileData {
                end_of_file_token: self.id(end_of_file_token),
                statements: self.req_list(statements),
            },
        );
        if self.target == NodeFactoryTarget::Synthetic {
            set_synthetic_source_file_data(
                node,
                SyntheticSourceFileData {
                    file_name,
                    path: path.to_string(),
                    text: text.into(),
                    ..Default::default()
                },
            );
        }
        node
    }

    /// Go `f.NewSourceFile(node.parseOptions, node.text, statements,
    /// endOfFileToken)` followed by `updated.copyFrom(node)`, the shared start
    /// of Go `UpdateSourceFile` and `SourceFile.Clone`. `node` is a parsed or
    /// factory SourceFile.
    pub fn new_source_file_from(
        &self,
        node: Node,
        statements: NodeList,
        end_of_file_token: Node,
    ) -> Node {
        let (file_name, path, text) = if is_synthetic_node(node) {
            with_synthetic_source_file(node, |d| (d.file_name, d.path.clone(), d.text.clone()))
        } else {
            (
                source_file_file_name(node),
                source_file_info(node).path.clone(),
                source_file_text(node),
            )
        };
        let updated = self.new_source_file(file_name, &path, text, statements, end_of_file_token);
        source_file_copy_from(updated, node);
        updated
    }

    // ── Lists ──────────────────────────────────────────────────────────

    // Go: ast/ast.go:128 NewNodeList
    #[must_use]
    pub fn new_node_list(&self, nodes: &[Node]) -> NodeList {
        self.new_node_list_with_loc(nodes, TextRange::undefined())
    }

    /// Go `list := f.NewNodeList(nodes); list.Loc = loc` (parser.go
    /// `newNodeList`).
    // PORT: a list `Loc` is fixed when the list is made (see
    // `new_synthetic_node_list`).
    #[must_use]
    pub fn new_node_list_with_loc(&self, nodes: &[Node], loc: TextRange) -> NodeList {
        match self.target {
            NodeFactoryTarget::Synthetic => crate::ast::synthetic::new_synthetic_node_list_in(
                self.print_owned.get(),
                nodes,
                loc,
            ),
            NodeFactoryTarget::File(store) => new_store_node_list(store, nodes, loc),
        }
    }

    // Go: ast/ast.go:159 NewModifierList
    #[must_use]
    pub fn new_modifier_list(&self, nodes: &[Node]) -> ModifierList {
        self.new_modifier_list_with_loc(nodes, TextRange::undefined())
    }

    /// Go `list := f.NewModifierList(nodes); list.Loc = loc` (parser.go
    /// `newModifierList`).
    #[must_use]
    pub fn new_modifier_list_with_loc(&self, nodes: &[Node], loc: TextRange) -> ModifierList {
        match self.target {
            NodeFactoryTarget::Synthetic => crate::ast::synthetic::new_synthetic_modifier_list_in(
                self.print_owned.get(),
                nodes,
                loc,
            ),
            NodeFactoryTarget::File(store) => new_store_modifier_list(store, nodes, loc),
        }
    }

    // ── Tokens, names and literals ─────────────────────────────────────

    // Go: ast/ast_generated.go:601 NewToken
    pub fn new_token(&self, kind: SyntaxKind) -> Node {
        self.new_node(kind, crate::astdata::TokenData)
    }

    // Go: ast/ast.go:1599 NewModifier
    pub fn new_modifier(&self, kind: SyntaxKind) -> Node {
        self.new_token(kind)
    }

    // Go: ast/ast_generated.go:673 NewIdentifier
    pub fn new_identifier(&self, text: impl AsRef<str> + Into<String>) -> Node {
        self.new_name_node(SyntaxKind::Identifier, text, |text| {
            crate::astdata::IdentifierData { text }
        })
    }

    // Go: ast/ast_generated.go:697 NewPrivateIdentifier
    pub fn new_private_identifier(&self, text: impl AsRef<str> + Into<String>) -> Node {
        self.new_name_node(SyntaxKind::PrivateIdentifier, text, |text| {
            crate::astdata::PrivateIdentifierData { text }
        })
    }

    // Go: ast/ast_generated.go:724 NewQualifiedName
    pub fn new_qualified_name(&self, left: Node, right: Node) -> Node {
        self.new_node(
            SyntaxKind::QualifiedName,
            crate::astdata::QualifiedNameData {
                left: self.id(left),
                right: self.id(right),
            },
        )
    }

    // Go: ast/ast_generated.go:769 NewComputedPropertyName
    pub fn new_computed_property_name(&self, expression: Node) -> Node {
        self.new_node(
            SyntaxKind::ComputedPropertyName,
            crate::astdata::ComputedPropertyNameData {
                expression: self.id(expression),
            },
        )
    }

    // Go: ast/ast_generated.go:3667 NewStringLiteral
    pub fn new_string_literal(&self, text: impl Into<String>, flags: TokenFlags) -> Node {
        self.new_text_node(
            SyntaxKind::StringLiteral,
            crate::astdata::StringLiteralData {
                text: text.into(),
                token_flags: token_flags(flags & TokenFlags::STRING_LITERAL_FLAGS),
            },
        )
    }

    // Go: ast/ast_generated.go:3691 NewNumericLiteral
    pub fn new_numeric_literal(&self, text: impl Into<String>, flags: TokenFlags) -> Node {
        self.new_text_node(
            SyntaxKind::NumericLiteral,
            crate::astdata::NumericLiteralData {
                text: text.into(),
                token_flags: token_flags(flags & TokenFlags::NUMERIC_LITERAL_FLAGS),
            },
        )
    }

    // Go: ast/ast_generated.go:3715 NewBigIntLiteral
    pub fn new_big_int_literal(&self, text: impl Into<String>, flags: TokenFlags) -> Node {
        self.new_text_node(
            SyntaxKind::BigIntLiteral,
            crate::astdata::BigIntLiteralData {
                text: text.into(),
                token_flags: token_flags(flags & TokenFlags::NUMERIC_LITERAL_FLAGS),
            },
        )
    }

    // Go: ast/ast_generated.go:3739 NewRegularExpressionLiteral
    pub fn new_regular_expression_literal(
        &self,
        text: impl Into<String>,
        flags: TokenFlags,
    ) -> Node {
        self.new_text_node(
            SyntaxKind::RegularExpressionLiteral,
            crate::astdata::RegularExpressionLiteralData {
                text: text.into(),
                token_flags: token_flags(flags & TokenFlags::REGULAR_EXPRESSION_LITERAL_FLAGS),
            },
        )
    }

    // Go: ast/ast_generated.go:3765 NewNoSubstitutionTemplateLiteral
    // PORT: Go sets no `RawText`; ts_ast stores an empty string for it.
    pub fn new_no_substitution_template_literal(
        &self,
        text: impl Into<String>,
        template_flags: TokenFlags,
    ) -> Node {
        self.new_text_node(
            SyntaxKind::NoSubstitutionTemplateLiteral,
            crate::astdata::NoSubstitutionTemplateLiteralData {
                raw_text: String::new(),
                template_flags: token_flags(
                    template_flags & TokenFlags::TEMPLATE_LITERAL_LIKE_FLAGS,
                ),
                text: text.into(),
                token_flags: NO_TOKEN_FLAGS,
            },
        )
    }

    // Go: ast/ast_generated.go:6182 NewTemplateHead
    pub fn new_template_head(
        &self,
        text: impl Into<String>,
        raw_text: impl Into<String>,
        template_flags: TokenFlags,
    ) -> Node {
        self.new_text_node(
            SyntaxKind::TemplateHead,
            crate::astdata::TemplateHeadData {
                raw_text: raw_text.into(),
                template_flags: token_flags(
                    template_flags & TokenFlags::TEMPLATE_LITERAL_LIKE_FLAGS,
                ),
                text: text.into(),
                token_flags: NO_TOKEN_FLAGS,
            },
        )
    }

    // Go: ast/ast_generated.go:6208 NewTemplateMiddle
    pub fn new_template_middle(
        &self,
        text: impl Into<String>,
        raw_text: impl Into<String>,
        template_flags: TokenFlags,
    ) -> Node {
        self.new_text_node(
            SyntaxKind::TemplateMiddle,
            crate::astdata::TemplateMiddleData {
                raw_text: raw_text.into(),
                template_flags: token_flags(
                    template_flags & TokenFlags::TEMPLATE_LITERAL_LIKE_FLAGS,
                ),
                text: text.into(),
                token_flags: NO_TOKEN_FLAGS,
            },
        )
    }

    // Go: ast/ast_generated.go:6234 NewTemplateTail
    pub fn new_template_tail(
        &self,
        text: impl Into<String>,
        raw_text: impl Into<String>,
        template_flags: TokenFlags,
    ) -> Node {
        self.new_text_node(
            SyntaxKind::TemplateTail,
            crate::astdata::TemplateTailData {
                raw_text: raw_text.into(),
                template_flags: token_flags(
                    template_flags & TokenFlags::TEMPLATE_LITERAL_LIKE_FLAGS,
                ),
                text: text.into(),
                token_flags: NO_TOKEN_FLAGS,
            },
        )
    }

    // ── Type nodes ─────────────────────────────────────────────────────

    // Go: ast/ast_generated.go:5176 NewKeywordTypeNode
    pub fn new_keyword_type_node(&self, kind: SyntaxKind) -> Node {
        self.new_node(kind, crate::astdata::KeywordTypeNodeData)
    }

    // Go: ast/ast_generated.go:5206 NewUnionTypeNode
    pub fn new_union_type_node(&self, types: NodeList) -> Node {
        self.new_node(
            SyntaxKind::UnionType,
            crate::astdata::UnionTypeNodeData {
                types: self.req_list(types),
            },
        )
    }

    // Go: ast/ast_generated.go:5243 NewIntersectionTypeNode
    pub fn new_intersection_type_node(&self, types: NodeList) -> Node {
        self.new_node(
            SyntaxKind::IntersectionType,
            crate::astdata::IntersectionTypeNodeData {
                types: self.req_list(types),
            },
        )
    }

    // Go: ast/ast_generated.go:5285 NewConditionalTypeNode
    pub fn new_conditional_type_node(
        &self,
        check_type: Node,
        extends_type: Node,
        true_type: Node,
        false_type: Node,
    ) -> Node {
        self.new_node(
            SyntaxKind::ConditionalType,
            crate::astdata::ConditionalTypeNodeData {
                check_type: self.id(check_type),
                extends_type: self.id(extends_type),
                false_type: self.id(false_type),
                true_type: self.id(true_type),
            },
        )
    }

    // Go: ast/ast_generated.go:5330 NewTypeOperatorNode
    pub fn new_type_operator_node(&self, operator: SyntaxKind, type_node: Node) -> Node {
        self.new_node(
            SyntaxKind::TypeOperator,
            crate::astdata::TypeOperatorNodeData {
                operator,
                type_: self.id(type_node),
            },
        )
    }

    // Go: ast/ast_generated.go:5369 NewInferTypeNode
    pub fn new_infer_type_node(&self, type_parameter: Node) -> Node {
        self.new_node(
            SyntaxKind::InferType,
            crate::astdata::InferTypeNodeData {
                type_parameter: self.id(type_parameter),
            },
        )
    }

    // Go: ast/ast_generated.go:5407 NewArrayTypeNode
    pub fn new_array_type_node(&self, element_type: Node) -> Node {
        self.new_node(
            SyntaxKind::ArrayType,
            crate::astdata::ArrayTypeNodeData {
                element_type: self.id(element_type),
            },
        )
    }

    // Go: ast/ast_generated.go:5446 NewIndexedAccessTypeNode
    pub fn new_indexed_access_type_node(&self, object_type: Node, index_type: Node) -> Node {
        self.new_node(
            SyntaxKind::IndexedAccessType,
            crate::astdata::IndexedAccessTypeNodeData {
                index_type: self.id(index_type),
                object_type: self.id(object_type),
            },
        )
    }

    // Go: ast/ast_generated.go:5485 NewTypeReferenceNode
    pub fn new_type_reference_node(&self, type_name: Node, type_arguments: NodeList) -> Node {
        self.new_node(
            SyntaxKind::TypeReference,
            crate::astdata::TypeReferenceNodeData {
                type_arguments: self.opt_list(type_arguments),
                type_name: self.id(type_name),
            },
        )
    }

    // Go: ast/ast_generated.go:5526 NewExpressionWithTypeArguments
    pub fn new_expression_with_type_arguments(
        &self,
        expression: Node,
        type_arguments: NodeList,
    ) -> Node {
        self.new_node(
            SyntaxKind::ExpressionWithTypeArguments,
            crate::astdata::ExpressionWithTypeArgumentsData {
                expression: self.id(expression),
                type_arguments: self.opt_list(type_arguments),
            },
        )
    }

    // Go: ast/ast_generated.go:5565 NewLiteralTypeNode
    pub fn new_literal_type_node(&self, literal: Node) -> Node {
        self.new_node(
            SyntaxKind::LiteralType,
            crate::astdata::LiteralTypeNodeData {
                literal: self.id(literal),
            },
        )
    }

    // Go: ast/ast_generated.go:5602 NewThisTypeNode
    pub fn new_this_type_node(&self) -> Node {
        self.new_node(SyntaxKind::ThisType, crate::astdata::ThisTypeNodeData)
    }

    // Go: ast/ast_generated.go:5626 NewTypePredicateNode
    pub fn new_type_predicate_node(
        &self,
        asserts_modifier: Node,
        parameter_name: Node,
        type_node: Node,
    ) -> Node {
        self.new_node(
            SyntaxKind::TypePredicate,
            crate::astdata::TypePredicateNodeData {
                asserts_modifier: self.oid(asserts_modifier),
                parameter_name: self.id(parameter_name),
                type_: self.oid(type_node),
            },
        )
    }

    // Go: ast/ast_generated.go:5763 NewTypeQueryNode
    pub fn new_type_query_node(&self, expr_name: Node, type_arguments: NodeList) -> Node {
        self.new_node(
            SyntaxKind::TypeQuery,
            crate::astdata::TypeQueryNodeData {
                expr_name: self.id(expr_name),
                type_arguments: self.opt_list(type_arguments),
            },
        )
    }

    // Go: ast/ast_generated.go:5809 NewMappedTypeNode
    pub fn new_mapped_type_node(
        &self,
        readonly_token: Node,
        type_parameter: Node,
        name_type: Node,
        question_token: Node,
        type_node: Node,
        members: NodeList,
    ) -> Node {
        self.new_node(
            SyntaxKind::MappedType,
            crate::astdata::MappedTypeNodeData {
                members: self.opt_list(members),
                name_type: self.oid(name_type),
                question_token: self.oid(question_token),
                readonly_token: self.oid(readonly_token),
                type_: self.oid(type_node),
                type_parameter: self.id(type_parameter),
            },
        )
    }

    // Go: ast/ast_generated.go:5858 NewTypeLiteralNode
    pub fn new_type_literal_node(&self, members: NodeList) -> Node {
        self.new_node(
            SyntaxKind::TypeLiteral,
            crate::astdata::TypeLiteralNodeData {
                members: self.req_list(members),
            },
        )
    }

    // Go: ast/ast_generated.go:5896 NewTupleTypeNode
    pub fn new_tuple_type_node(&self, elements: NodeList) -> Node {
        self.new_node(
            SyntaxKind::TupleType,
            crate::astdata::TupleTypeNodeData {
                elements: self.req_list(elements),
            },
        )
    }

    // Go: ast/ast_generated.go:5938 NewNamedTupleMember
    pub fn new_named_tuple_member(
        &self,
        dot_dot_dot_token: Node,
        name: Node,
        question_token: Node,
        type_node: Node,
    ) -> Node {
        self.new_node(
            SyntaxKind::NamedTupleMember,
            crate::astdata::NamedTupleMemberData {
                dot_dot_dot_token: self.oid(dot_dot_dot_token),
                question_token: self.oid(question_token),
                type_: self.id(type_node),
                name: self.id(name),
            },
        )
    }

    // Go: ast/ast_generated.go:5986 NewOptionalTypeNode
    pub fn new_optional_type_node(&self, type_node: Node) -> Node {
        self.new_node(
            SyntaxKind::OptionalType,
            crate::astdata::OptionalTypeNodeData {
                type_: self.id(type_node),
            },
        )
    }

    // Go: ast/ast_generated.go:6024 NewRestTypeNode
    pub fn new_rest_type_node(&self, type_node: Node) -> Node {
        self.new_node(
            SyntaxKind::RestType,
            crate::astdata::RestTypeNodeData {
                type_: self.id(type_node),
            },
        )
    }

    // Go: ast/ast_generated.go:6062 NewParenthesizedTypeNode
    pub fn new_parenthesized_type_node(&self, type_node: Node) -> Node {
        self.new_node(
            SyntaxKind::ParenthesizedType,
            crate::astdata::ParenthesizedTypeNodeData {
                type_: self.id(type_node),
            },
        )
    }

    // Go: ast/ast_generated.go:6099 NewFunctionTypeNode
    pub fn new_function_type_node(
        &self,
        type_parameters: NodeList,
        parameters: NodeList,
        type_node: Node,
    ) -> Node {
        self.new_node(
            SyntaxKind::FunctionType,
            crate::astdata::FunctionTypeNodeData {
                full_signature: None,
                parameters: self.req_list(parameters),
                type_: self.oid(type_node),
                type_parameters: self.opt_list(type_parameters),
                modifiers: None,
            },
        )
    }

    // Go: ast/ast_generated.go:6138 NewConstructorTypeNode
    pub fn new_constructor_type_node(
        &self,
        modifiers: ModifierList,
        type_parameters: NodeList,
        parameters: NodeList,
        type_node: Node,
    ) -> Node {
        self.new_node(
            SyntaxKind::ConstructorType,
            crate::astdata::ConstructorTypeNodeData {
                full_signature: None,
                parameters: self.req_list(parameters),
                type_: self.oid(type_node),
                type_parameters: self.opt_list(type_parameters),
                modifiers: self.mods(modifiers),
            },
        )
    }

    // Go: ast/ast_generated.go:6261 NewTemplateLiteralTypeNode
    pub fn new_template_literal_type_node(&self, head: Node, template_spans: NodeList) -> Node {
        self.new_node(
            SyntaxKind::TemplateLiteralType,
            crate::astdata::TemplateLiteralTypeNodeData {
                head: self.id(head),
                template_spans: self.req_list(template_spans),
            },
        )
    }

    // Go: ast/ast_generated.go:6301 NewTemplateLiteralTypeSpan
    pub fn new_template_literal_type_span(&self, type_node: Node, literal: Node) -> Node {
        self.new_node(
            SyntaxKind::TemplateLiteralTypeSpan,
            crate::astdata::TemplateLiteralTypeSpanData {
                literal: self.id(literal),
                type_: self.id(type_node),
            },
        )
    }

    // Go: ast/ast_generated.go:8274 NewImportTypeNode
    pub fn new_import_type_node(
        &self,
        is_type_of: bool,
        argument: Node,
        attributes: Node,
        qualifier: Node,
        type_arguments: NodeList,
    ) -> Node {
        self.new_node(
            SyntaxKind::ImportType,
            crate::astdata::ImportTypeNodeData {
                argument: self.id(argument),
                attributes: self.oid(attributes),
                is_type_of,
                qualifier: self.oid(qualifier),
                type_arguments: self.opt_list(type_arguments),
            },
        )
    }

    // ── Signatures and type elements ───────────────────────────────────

    // Go: ast/ast_generated.go:8578 NewTypeParameterDeclaration
    pub fn new_type_parameter_declaration(
        &self,
        modifiers: ModifierList,
        name: Node,
        constraint: Node,
        expression: Node,
        default_type: Node,
    ) -> Node {
        self.new_node(
            SyntaxKind::TypeParameter,
            crate::astdata::TypeParameterDeclarationData {
                constraint: self.oid(constraint),
                default_type: self.oid(default_type),
                expression: self.oid(expression),
                modifiers: self.mods(modifiers),
                name: self.id(name),
            },
        )
    }

    // Go: ast/ast_generated.go:1909 NewParameterDeclaration
    pub fn new_parameter_declaration(
        &self,
        modifiers: ModifierList,
        dot_dot_dot_token: Node,
        name: Node,
        question_token: Node,
        type_node: Node,
        initializer: Node,
    ) -> Node {
        self.new_node(
            SyntaxKind::Parameter,
            crate::astdata::ParameterDeclarationData {
                dot_dot_dot_token: self.oid(dot_dot_dot_token),
                initializer: self.oid(initializer),
                question_token: self.oid(question_token),
                type_: self.oid(type_node),
                modifiers: self.mods(modifiers),
                name: self.id(name),
            },
        )
    }

    // Go: ast/ast_generated.go:3031 NewCallSignatureDeclaration
    pub fn new_call_signature_declaration(
        &self,
        type_parameters: NodeList,
        parameters: NodeList,
        type_node: Node,
    ) -> Node {
        self.new_node(
            SyntaxKind::CallSignature,
            crate::astdata::CallSignatureDeclarationData {
                full_signature: None,
                parameters: self.req_list(parameters),
                type_: self.oid(type_node),
                type_parameters: self.opt_list(type_parameters),
            },
        )
    }

    // Go: ast/ast_generated.go:3074 NewConstructSignatureDeclaration
    pub fn new_construct_signature_declaration(
        &self,
        type_parameters: NodeList,
        parameters: NodeList,
        type_node: Node,
    ) -> Node {
        self.new_node(
            SyntaxKind::ConstructSignature,
            crate::astdata::ConstructSignatureDeclarationData {
                full_signature: None,
                parameters: self.req_list(parameters),
                type_: self.oid(type_node),
                type_parameters: self.opt_list(type_parameters),
            },
        )
    }

    // Go: ast/ast_generated.go:3119 NewConstructorDeclaration
    pub fn new_constructor_declaration(
        &self,
        modifiers: ModifierList,
        type_parameters: NodeList,
        parameters: NodeList,
        type_node: Node,
        full_signature: Node,
        body: Node,
    ) -> Node {
        self.new_node(
            SyntaxKind::Constructor,
            crate::astdata::ConstructorDeclarationData {
                asterisk_token: None,
                body: self.oid(body),
                full_signature: self.oid(full_signature),
                parameters: self.req_list(parameters),
                type_: self.oid(type_node),
                type_parameters: self.opt_list(type_parameters),
                modifiers: self.mods(modifiers),
            },
        )
    }

    // Go: ast/ast_generated.go:3166 NewGetAccessorDeclaration
    #[allow(clippy::too_many_arguments)]
    pub fn new_get_accessor_declaration(
        &self,
        modifiers: ModifierList,
        name: Node,
        type_parameters: NodeList,
        parameters: NodeList,
        type_node: Node,
        full_signature: Node,
        body: Node,
    ) -> Node {
        self.new_node(
            SyntaxKind::GetAccessor,
            crate::astdata::GetAccessorDeclarationData {
                asterisk_token: None,
                body: self.oid(body),
                full_signature: self.oid(full_signature),
                parameters: self.req_list(parameters),
                postfix_token: None,
                type_: self.oid(type_node),
                type_parameters: self.opt_list(type_parameters),
                modifiers: self.mods(modifiers),
                name: self.id(name),
            },
        )
    }

    // Go: ast/ast_generated.go:3219 NewSetAccessorDeclaration
    #[allow(clippy::too_many_arguments)]
    pub fn new_set_accessor_declaration(
        &self,
        modifiers: ModifierList,
        name: Node,
        type_parameters: NodeList,
        parameters: NodeList,
        type_node: Node,
        full_signature: Node,
        body: Node,
    ) -> Node {
        self.new_node(
            SyntaxKind::SetAccessor,
            crate::astdata::SetAccessorDeclarationData {
                asterisk_token: None,
                body: self.oid(body),
                full_signature: self.oid(full_signature),
                parameters: self.req_list(parameters),
                postfix_token: None,
                type_: self.oid(type_node),
                type_parameters: self.opt_list(type_parameters),
                modifiers: self.mods(modifiers),
                name: self.id(name),
            },
        )
    }

    // Go: ast/ast_generated.go:3278 NewIndexSignatureDeclaration
    // PORT: Go keeps a nil `Type`; ts_ast requires one, so nil is stored in
    // the nil slot and `type_node()` still reads nil.
    pub fn new_index_signature_declaration(
        &self,
        modifiers: ModifierList,
        parameters: NodeList,
        type_node: Node,
    ) -> Node {
        self.new_node(
            SyntaxKind::IndexSignature,
            crate::astdata::IndexSignatureDeclarationData {
                full_signature: None,
                parameters: self.req_list(parameters),
                type_: self.id(type_node),
                type_parameters: None,
                modifiers: self.mods(modifiers),
            },
        )
    }

    // Go: ast/ast_generated.go:3322 NewMethodSignatureDeclaration
    pub fn new_method_signature_declaration(
        &self,
        modifiers: ModifierList,
        name: Node,
        postfix_token: Node,
        type_parameters: NodeList,
        parameters: NodeList,
        type_node: Node,
    ) -> Node {
        self.new_node(
            SyntaxKind::MethodSignature,
            crate::astdata::MethodSignatureDeclarationData {
                full_signature: None,
                parameters: self.req_list(parameters),
                postfix_token: self.oid(postfix_token),
                type_: self.oid(type_node),
                type_parameters: self.opt_list(type_parameters),
                modifiers: self.mods(modifiers),
                name: self.id(name),
            },
        )
    }

    // Go: ast/ast_generated.go:3380 NewMethodDeclaration
    #[allow(clippy::too_many_arguments)]
    pub fn new_method_declaration(
        &self,
        modifiers: ModifierList,
        asterisk_token: Node,
        name: Node,
        postfix_token: Node,
        type_parameters: NodeList,
        parameters: NodeList,
        type_node: Node,
        full_signature: Node,
        body: Node,
    ) -> Node {
        self.new_node(
            SyntaxKind::MethodDeclaration,
            crate::astdata::MethodDeclarationData {
                asterisk_token: self.oid(asterisk_token),
                body: self.oid(body),
                full_signature: self.oid(full_signature),
                parameters: self.req_list(parameters),
                postfix_token: self.oid(postfix_token),
                type_: self.oid(type_node),
                type_parameters: self.opt_list(type_parameters),
                modifiers: self.mods(modifiers),
                name: self.id(name),
            },
        )
    }

    // Go: ast/ast_generated.go:3443 NewPropertySignatureDeclaration
    // PORT: ts_ast requires `Type` and `Initializer`; Go nil goes to the nil
    // slot, so `type_node()` and `initializer()` still read nil.
    pub fn new_property_signature_declaration(
        &self,
        modifiers: ModifierList,
        name: Node,
        postfix_token: Node,
        type_node: Node,
        initializer: Node,
    ) -> Node {
        self.new_node(
            SyntaxKind::PropertySignature,
            crate::astdata::PropertySignatureDeclarationData {
                initializer: self.id(initializer),
                postfix_token: self.oid(postfix_token),
                type_: self.id(type_node),
                modifiers: self.mods(modifiers),
                name: self.id(name),
            },
        )
    }

    // Go: ast/ast_generated.go:3498 NewPropertyDeclaration
    pub fn new_property_declaration(
        &self,
        modifiers: ModifierList,
        name: Node,
        postfix_token: Node,
        type_node: Node,
        initializer: Node,
    ) -> Node {
        self.new_node(
            SyntaxKind::PropertyDeclaration,
            crate::astdata::PropertyDeclarationData {
                initializer: self.oid(initializer),
                postfix_token: self.oid(postfix_token),
                type_: self.oid(type_node),
                modifiers: self.mods(modifiers),
                name: self.id(name),
            },
        )
    }

    // Go: ast/ast_generated.go:2559 NewNotEmittedTypeElement
    pub fn new_not_emitted_type_element(&self) -> Node {
        self.new_node(
            SyntaxKind::NotEmittedTypeElement,
            crate::astdata::NotEmittedTypeElementData,
        )
    }

    // ── Declarations ───────────────────────────────────────────────────

    // Go: ast/ast_generated.go:2402 NewEnumMember
    pub fn new_enum_member(&self, name: Node, initializer: Node) -> Node {
        self.new_node(
            SyntaxKind::EnumMember,
            crate::astdata::EnumMemberData {
                initializer: self.oid(initializer),
                postfix_token: None,
                modifiers: None,
                name: self.id(name),
            },
        )
    }

    // Go: ast/ast_generated.go:2450 NewEnumDeclaration
    pub fn new_enum_declaration(
        &self,
        modifiers: ModifierList,
        name: Node,
        members: NodeList,
    ) -> Node {
        self.new_node(
            SyntaxKind::EnumDeclaration,
            crate::astdata::EnumDeclarationData {
                members: self.req_list(members),
                modifiers: self.mods(modifiers),
                name: self.id(name),
            },
        )
    }

    // Go: ast/ast_generated.go:2061 NewFunctionDeclaration
    #[allow(clippy::too_many_arguments)]
    pub fn new_function_declaration(
        &self,
        modifiers: ModifierList,
        asterisk_token: Node,
        name: Node,
        type_parameters: NodeList,
        parameters: NodeList,
        type_node: Node,
        full_signature: Node,
        body: Node,
    ) -> Node {
        self.new_node(
            SyntaxKind::FunctionDeclaration,
            crate::astdata::FunctionDeclarationData {
                asterisk_token: self.oid(asterisk_token),
                body: self.oid(body),
                full_signature: self.oid(full_signature),
                parameters: self.req_list(parameters),
                type_: self.oid(type_node),
                type_parameters: self.opt_list(type_parameters),
                modifiers: self.mods(modifiers),
                name: self.oid(name),
            },
        )
    }

    // Go: ast/ast_generated.go:2118 NewClassDeclaration
    pub fn new_class_declaration(
        &self,
        modifiers: ModifierList,
        name: Node,
        type_parameters: NodeList,
        heritage_clauses: NodeList,
        members: NodeList,
    ) -> Node {
        self.new_node(
            SyntaxKind::ClassDeclaration,
            crate::astdata::ClassDeclarationData {
                heritage_clauses: self.opt_list(heritage_clauses),
                members: self.req_list(members),
                type_parameters: self.opt_list(type_parameters),
                modifiers: self.mods(modifiers),
                name: self.oid(name),
            },
        )
    }

    // Go: ast/ast_generated.go:2169 NewClassExpression
    pub fn new_class_expression(
        &self,
        modifiers: ModifierList,
        name: Node,
        type_parameters: NodeList,
        heritage_clauses: NodeList,
        members: NodeList,
    ) -> Node {
        self.new_node(
            SyntaxKind::ClassExpression,
            crate::astdata::ClassExpressionData {
                heritage_clauses: self.opt_list(heritage_clauses),
                members: self.req_list(members),
                type_parameters: self.opt_list(type_parameters),
                modifiers: self.mods(modifiers),
                name: self.oid(name),
            },
        )
    }

    // Go: ast/ast_generated.go:2221 NewHeritageClause
    pub fn new_heritage_clause(&self, token: SyntaxKind, types: NodeList) -> Node {
        self.new_node(
            SyntaxKind::HeritageClause,
            crate::astdata::HeritageClauseData {
                token,
                types: self.req_list(types),
            },
        )
    }

    // Go: ast/ast_generated.go:2267 NewInterfaceDeclaration
    pub fn new_interface_declaration(
        &self,
        modifiers: ModifierList,
        name: Node,
        type_parameters: NodeList,
        heritage_clauses: NodeList,
        members: NodeList,
    ) -> Node {
        self.new_node(
            SyntaxKind::InterfaceDeclaration,
            crate::astdata::InterfaceDeclarationData {
                heritage_clauses: self.opt_list(heritage_clauses),
                members: self.req_list(members),
                type_parameters: self.opt_list(type_parameters),
                modifiers: self.mods(modifiers),
                name: self.id(name),
            },
        )
    }

    // Go: ast/ast_generated.go:2324 NewTypeAliasDeclaration
    pub fn new_type_alias_declaration(
        &self,
        modifiers: ModifierList,
        name: Node,
        type_parameters: NodeList,
        type_node: Node,
    ) -> Node {
        self.new_node(
            SyntaxKind::TypeAliasDeclaration,
            crate::astdata::TypeAliasDeclarationData {
                type_: self.id(type_node),
                type_parameters: self.opt_list(type_parameters),
                modifiers: self.mods(modifiers),
                name: self.id(name),
            },
        )
    }

    // Go: ast/ast_generated.go:8120 NewModuleDeclaration
    pub fn new_module_declaration(
        &self,
        modifiers: ModifierList,
        keyword: SyntaxKind,
        name: Node,
        attributes: Node,
        body: Node,
    ) -> Node {
        self.new_node(
            SyntaxKind::ModuleDeclaration,
            crate::astdata::ModuleDeclarationData {
                asterisk_token: None,
                attributes: self.oid(attributes),
                body: self.oid(body),
                keyword,
                modifiers: self.mods(modifiers),
                name: self.id(name),
            },
        )
    }

    // Go: ast/ast_generated.go:2495 NewModuleBlock
    pub fn new_module_block(&self, statements: NodeList) -> Node {
        self.new_node(
            SyntaxKind::ModuleBlock,
            crate::astdata::ModuleBlockData {
                statements: self.req_list(statements),
            },
        )
    }

    // Go: ast/ast_generated.go:5668 NewImportAttribute
    pub fn new_import_attribute(&self, name: Node, value: Node) -> Node {
        self.new_node(
            SyntaxKind::ImportAttribute,
            crate::astdata::ImportAttributeData {
                value: self.id(value),
                name: self.id(name),
            },
        )
    }

    // Go: ast/ast_generated.go:5719 NewImportAttributes
    pub fn new_import_attributes(
        &self,
        token: SyntaxKind,
        attributes: NodeList,
        multi_line: bool,
    ) -> Node {
        self.new_node(
            SyntaxKind::ImportAttributes,
            crate::astdata::ImportAttributesData {
                attributes: self.req_list(attributes),
                multi_line,
                token,
            },
        )
    }

    // Go: ast/ast_generated.go:2802 NewExportAssignment
    pub fn new_export_assignment(
        &self,
        modifiers: ModifierList,
        is_export_equals: bool,
        type_node: Node,
        expression: Node,
    ) -> Node {
        self.new_node(
            SyntaxKind::ExportAssignment,
            crate::astdata::ExportAssignmentData {
                expression: self.id(expression),
                is_export_equals,
                type_: self.oid(type_node),
                modifiers: self.mods(modifiers),
            },
        )
    }

    // Go: ast/ast_generated.go:2937 NewNamedExports
    pub fn new_named_exports(&self, elements: NodeList) -> Node {
        self.new_node(
            SyntaxKind::NamedExports,
            crate::astdata::NamedExportsData {
                elements: self.req_list(elements),
            },
        )
    }

    // Go: ast/ast_generated.go:2984 NewExportSpecifier
    pub fn new_export_specifier(
        &self,
        is_type_only: bool,
        property_name: Node,
        name: Node,
    ) -> Node {
        self.new_node(
            SyntaxKind::ExportSpecifier,
            crate::astdata::ExportSpecifierData {
                is_type_only,
                property_name: self.oid(property_name),
                name: self.id(name),
            },
        )
    }

    // Go: ast/ast_generated.go:8226 NewExportDeclaration
    pub fn new_export_declaration(
        &self,
        modifiers: ModifierList,
        is_type_only: bool,
        export_clause: Node,
        module_specifier: Node,
        attributes: Node,
    ) -> Node {
        self.new_node(
            SyntaxKind::ExportDeclaration,
            crate::astdata::ExportDeclarationData {
                attributes: self.oid(attributes),
                export_clause: self.oid(export_clause),
                is_type_only,
                module_specifier: self.oid(module_specifier),
                modifiers: self.mods(modifiers),
            },
        )
    }

    // Go: ast/ast_generated.go:2586 NewImportDeclaration
    pub fn new_import_declaration(
        &self,
        modifiers: ModifierList,
        import_clause: Node,
        module_specifier: Node,
        attributes: Node,
    ) -> Node {
        self.new_node(
            SyntaxKind::ImportDeclaration,
            crate::astdata::ImportDeclarationData {
                attributes: self.oid(attributes),
                import_clause: self.oid(import_clause),
                module_specifier: self.id(module_specifier),
                modifiers: self.mods(modifiers),
            },
        )
    }

    // Go: ast/ast_generated.go:2664 NewExternalModuleReference
    pub fn new_external_module_reference(&self, expression: Node) -> Node {
        self.new_node(
            SyntaxKind::ExternalModuleReference,
            crate::astdata::ExternalModuleReferenceData {
                expression: self.id(expression),
            },
        )
    }

    // Go: ast/ast_generated.go:2755 NewNamedImports
    pub fn new_named_imports(&self, elements: NodeList) -> Node {
        self.new_node(
            SyntaxKind::NamedImports,
            crate::astdata::NamedImportsData {
                elements: self.req_list(elements),
            },
        )
    }

    // Go: ast/ast_generated.go:6913 NewSyntaxList
    pub fn new_syntax_list(&self, children: &[Node]) -> Node {
        let children = children.iter().map(|&n| self.id(n)).collect();
        self.new_node(
            SyntaxKind::SyntaxList,
            crate::astdata::SyntaxListData { children },
        )
    }

    // Go: ast/ast_generated.go:8175 NewImportEqualsDeclaration
    pub fn new_import_equals_declaration(
        &self,
        modifiers: ModifierList,
        is_type_only: bool,
        name: Node,
        module_reference: Node,
    ) -> Node {
        self.new_node(
            SyntaxKind::ImportEqualsDeclaration,
            crate::astdata::ImportEqualsDeclarationData {
                is_type_only,
                module_reference: self.id(module_reference),
                modifiers: self.mods(modifiers),
                name: self.id(name),
            },
        )
    }

    // Go: ast/ast_generated.go:8324 NewImportClause
    // PORT: Go KindUnknown for `phaseModifier` is `None` in the data.
    pub fn new_import_clause(
        &self,
        phase_modifier: SyntaxKind,
        name: Node,
        named_bindings: Node,
    ) -> Node {
        self.new_node(
            SyntaxKind::ImportClause,
            crate::astdata::ImportClauseData {
                named_bindings: self.oid(named_bindings),
                phase_modifier: if phase_modifier == SyntaxKind::Unknown {
                    None
                } else {
                    Some(phase_modifier)
                },
                name: self.oid(name),
            },
        )
    }

    // Go: ast/ast_generated.go:8373 NewImportSpecifier
    pub fn new_import_specifier(
        &self,
        is_type_only: bool,
        property_name: Node,
        name: Node,
    ) -> Node {
        self.new_node(
            SyntaxKind::ImportSpecifier,
            crate::astdata::ImportSpecifierData {
                is_type_only,
                property_name: self.oid(property_name),
                name: self.id(name),
            },
        )
    }

    // ── Expressions ────────────────────────────────────────────────────

    // Go: ast/ast_generated.go:3637 NewKeywordExpression
    pub fn new_keyword_expression(&self, kind: SyntaxKind) -> Node {
        self.new_node(kind, crate::astdata::KeywordExpressionData)
    }

    // Go: ast/ast_generated.go:4230 NewPropertyAccessExpression
    pub fn new_property_access_expression(
        &self,
        expression: Node,
        question_dot_token: Node,
        name: Node,
        flags: NodeFlags,
    ) -> Node {
        self.new_chain_node(
            SyntaxKind::PropertyAccessExpression,
            crate::astdata::PropertyAccessExpressionData {
                expression: self.id(expression),
                question_dot_token: self.oid(question_dot_token),
                name: self.id(name),
            },
            flags,
        )
    }

    // Go: ast/ast_generated.go:4280 NewElementAccessExpression
    pub fn new_element_access_expression(
        &self,
        expression: Node,
        question_dot_token: Node,
        argument_expression: Node,
        flags: NodeFlags,
    ) -> Node {
        self.new_chain_node(
            SyntaxKind::ElementAccessExpression,
            crate::astdata::ElementAccessExpressionData {
                argument_expression: self.id(argument_expression),
                expression: self.id(expression),
                question_dot_token: self.oid(question_dot_token),
            },
            flags,
        )
    }

    // Go: ast/ast_generated.go:4333 NewCallExpression
    pub fn new_call_expression(
        &self,
        expression: Node,
        question_dot_token: Node,
        type_arguments: NodeList,
        arguments: NodeList,
        flags: NodeFlags,
    ) -> Node {
        self.new_chain_node(
            SyntaxKind::CallExpression,
            crate::astdata::CallExpressionData {
                arguments: self.req_list(arguments),
                expression: self.id(expression),
                question_dot_token: self.oid(question_dot_token),
                type_arguments: self.opt_list(type_arguments),
            },
            flags,
        )
    }

    // Go: ast/ast_generated.go:4382 NewNewExpression
    pub fn new_new_expression(
        &self,
        expression: Node,
        type_arguments: NodeList,
        arguments: NodeList,
    ) -> Node {
        self.new_node(
            SyntaxKind::NewExpression,
            crate::astdata::NewExpressionData {
                arguments: self.opt_list(arguments),
                expression: self.id(expression),
                type_arguments: self.opt_list(type_arguments),
            },
        )
    }

    // Go: ast/ast_generated.go:4468 NewNonNullExpression
    pub fn new_non_null_expression(&self, expression: Node, flags: NodeFlags) -> Node {
        self.new_chain_node(
            SyntaxKind::NonNullExpression,
            crate::astdata::NonNullExpressionData {
                expression: self.id(expression),
            },
            flags,
        )
    }

    // Go: ast/ast_generated.go:4641 NewTaggedTemplateExpression
    pub fn new_tagged_template_expression(
        &self,
        tag: Node,
        question_dot_token: Node,
        type_arguments: NodeList,
        template: Node,
        flags: NodeFlags,
    ) -> Node {
        self.new_chain_node(
            SyntaxKind::TaggedTemplateExpression,
            crate::astdata::TaggedTemplateExpressionData {
                question_dot_token: self.oid(question_dot_token),
                tag: self.id(tag),
                template: self.id(template),
                type_arguments: self.opt_list(type_arguments),
            },
            flags,
        )
    }

    // Go: ast/ast_generated.go:3843 NewPrefixUnaryExpression
    pub fn new_prefix_unary_expression(&self, operator: SyntaxKind, operand: Node) -> Node {
        self.new_node(
            SyntaxKind::PrefixUnaryExpression,
            crate::astdata::PrefixUnaryExpressionData {
                operand: self.id(operand),
                operator,
            },
        )
    }

    // Go: ast/ast_generated.go:3796 NewBinaryExpression
    pub fn new_binary_expression(
        &self,
        modifiers: ModifierList,
        left: Node,
        type_node: Node,
        operator_token: Node,
        right: Node,
    ) -> Node {
        self.new_node(
            SyntaxKind::BinaryExpression,
            crate::astdata::BinaryExpressionData {
                left: self.id(left),
                operator_token: self.id(operator_token),
                right: self.id(right),
                type_: self.oid(type_node),
                modifiers: self.mods(modifiers),
            },
        )
    }

    // Go: ast/ast_generated.go:4172 NewConditionalExpression
    pub fn new_conditional_expression(
        &self,
        condition: Node,
        question_token: Node,
        when_true: Node,
        colon_token: Node,
        when_false: Node,
    ) -> Node {
        self.new_node(
            SyntaxKind::ConditionalExpression,
            crate::astdata::ConditionalExpressionData {
                colon_token: self.id(colon_token),
                condition: self.id(condition),
                question_token: self.id(question_token),
                when_false: self.id(when_false),
                when_true: self.id(when_true),
            },
        )
    }

    // Go: ast/ast_generated.go:4687 NewParenthesizedExpression
    pub fn new_parenthesized_expression(&self, expression: Node) -> Node {
        self.new_node(
            SyntaxKind::ParenthesizedExpression,
            crate::astdata::ParenthesizedExpressionData {
                expression: self.id(expression),
            },
        )
    }

    // Go: ast/ast_generated.go:4031 NewFunctionExpression
    #[allow(clippy::too_many_arguments)]
    pub fn new_function_expression(
        &self,
        modifiers: ModifierList,
        asterisk_token: Node,
        name: Node,
        type_parameters: NodeList,
        parameters: NodeList,
        type_node: Node,
        full_signature: Node,
        body: Node,
    ) -> Node {
        self.new_node(
            SyntaxKind::FunctionExpression,
            crate::astdata::FunctionExpressionData {
                asterisk_token: self.oid(asterisk_token),
                body: self.id(body),
                full_signature: self.oid(full_signature),
                parameters: self.req_list(parameters),
                type_: self.oid(type_node),
                type_parameters: self.opt_list(type_parameters),
                modifiers: self.mods(modifiers),
                name: self.oid(name),
            },
        )
    }

    // Go: ast/ast_generated.go:3975 NewArrowFunction
    #[allow(clippy::too_many_arguments)]
    pub fn new_arrow_function(
        &self,
        modifiers: ModifierList,
        type_parameters: NodeList,
        parameters: NodeList,
        type_node: Node,
        full_signature: Node,
        equals_greater_than_token: Node,
        body: Node,
    ) -> Node {
        self.new_node(
            SyntaxKind::ArrowFunction,
            crate::astdata::ArrowFunctionData {
                asterisk_token: None,
                body: self.id(body),
                equals_greater_than_token: self.id(equals_greater_than_token),
                full_signature: self.oid(full_signature),
                parameters: self.req_list(parameters),
                type_: self.oid(type_node),
                type_parameters: self.opt_list(type_parameters),
                modifiers: self.mods(modifiers),
            },
        )
    }

    // Go: ast/ast_generated.go:6342 NewSyntheticExpression
    // PORT: Go `Type any` holds a `*checker.Type`; here it is a `TypeId`,
    // read back with `synthetic_expression_type`.
    pub fn new_synthetic_expression(
        &self,
        type_: TypeId,
        is_spread: bool,
        tuple_name_source: Node,
    ) -> Node {
        let node = self.new_node(
            SyntaxKind::SyntheticExpression,
            crate::astdata::SyntheticExpressionData {
                is_spread,
                tuple_name_source: self.oid(tuple_name_source),
                type_: crate::astdata::OpaqueValue,
            },
        );
        set_synthetic_expression_type(node, type_);
        node
    }

    // Go: ast/ast_generated.go:4508 NewSpreadElement
    pub fn new_spread_element(&self, expression: Node) -> Node {
        self.new_node(
            SyntaxKind::SpreadElement,
            crate::astdata::SpreadElementData {
                expression: self.id(expression),
            },
        )
    }

    // Go: ast/ast_generated.go:4731 NewArrayLiteralExpression
    pub fn new_array_literal_expression(&self, elements: NodeList, multi_line: bool) -> Node {
        self.new_node(
            SyntaxKind::ArrayLiteralExpression,
            crate::astdata::ArrayLiteralExpressionData {
                elements: self.req_list(elements),
                multi_line,
            },
        )
    }

    // Go: ast/ast_generated.go:4777 NewObjectLiteralExpression
    pub fn new_object_literal_expression(&self, properties: NodeList, multi_line: bool) -> Node {
        self.new_node(
            SyntaxKind::ObjectLiteralExpression,
            crate::astdata::ObjectLiteralExpressionData {
                multi_line,
                properties: self.req_list(properties),
            },
        )
    }

    // Go: ast/ast_generated.go:4865 NewPropertyAssignment
    pub fn new_property_assignment(
        &self,
        modifiers: ModifierList,
        name: Node,
        postfix_token: Node,
        type_node: Node,
        initializer: Node,
    ) -> Node {
        self.new_node(
            SyntaxKind::PropertyAssignment,
            crate::astdata::PropertyAssignmentData {
                initializer: self.id(initializer),
                postfix_token: self.oid(postfix_token),
                type_: self.oid(type_node),
                modifiers: self.mods(modifiers),
                name: self.id(name),
            },
        )
    }

    // Go: ast/ast_generated.go:4088 NewAsExpression
    pub fn new_as_expression(&self, expression: Node, type_node: Node) -> Node {
        self.new_node(
            SyntaxKind::AsExpression,
            crate::astdata::AsExpressionData {
                expression: self.id(expression),
                type_: self.id(type_node),
            },
        )
    }

    // Go: ast/ast_generated.go:4128 NewSatisfiesExpression
    pub fn new_satisfies_expression(&self, expression: Node, type_node: Node) -> Node {
        self.new_node(
            SyntaxKind::SatisfiesExpression,
            crate::astdata::SatisfiesExpressionData {
                expression: self.id(expression),
                type_: self.id(type_node),
            },
        )
    }

    // Go: ast/ast_generated.go:5138 NewTypeAssertion
    pub fn new_type_assertion(&self, type_node: Node, expression: Node) -> Node {
        self.new_node(
            SyntaxKind::TypeAssertionExpression,
            crate::astdata::TypeAssertionData {
                expression: self.id(expression),
                type_: self.id(type_node),
            },
        )
    }

    // Go: ast/ast_generated.go:4973 NewDeleteExpression
    pub fn new_delete_expression(&self, expression: Node) -> Node {
        self.new_node(
            SyntaxKind::DeleteExpression,
            crate::astdata::DeleteExpressionData {
                expression: self.id(expression),
            },
        )
    }

    // Go: ast/ast_generated.go:5015 NewTypeOfExpression
    pub fn new_type_of_expression(&self, expression: Node) -> Node {
        self.new_node(
            SyntaxKind::TypeOfExpression,
            crate::astdata::TypeOfExpressionData {
                expression: self.id(expression),
            },
        )
    }

    // Go: ast/ast_generated.go:5057 NewVoidExpression
    pub fn new_void_expression(&self, expression: Node) -> Node {
        self.new_node(
            SyntaxKind::VoidExpression,
            crate::astdata::VoidExpressionData {
                expression: self.id(expression),
            },
        )
    }

    // Go: ast/ast_generated.go:5099 NewAwaitExpression
    pub fn new_await_expression(&self, expression: Node) -> Node {
        self.new_node(
            SyntaxKind::AwaitExpression,
            crate::astdata::AwaitExpressionData {
                expression: self.id(expression),
            },
        )
    }

    // Go: ast/ast_generated.go:3931 NewYieldExpression
    pub fn new_yield_expression(&self, asterisk_token: Node, expression: Node) -> Node {
        self.new_node(
            SyntaxKind::YieldExpression,
            crate::astdata::YieldExpressionData {
                asterisk_token: self.oid(asterisk_token),
                expression: self.oid(expression),
            },
        )
    }

    // Go: ast/ast_generated.go:812 NewDecorator
    pub fn new_decorator(&self, expression: Node) -> Node {
        self.new_node(
            SyntaxKind::Decorator,
            crate::astdata::DecoratorData {
                expression: self.id(expression),
            },
        )
    }

    // ── Statements ─────────────────────────────────────────────────────

    // Go: ast/ast_generated.go:1675 NewBlock
    pub fn new_block(&self, statements: NodeList, multi_line: bool) -> Node {
        self.new_node(
            SyntaxKind::Block,
            crate::astdata::BlockData {
                multi_line,
                statements: self.req_list(statements),
            },
        )
    }

    // Go: ast/ast_generated.go:1720 NewVariableStatement
    pub fn new_variable_statement(&self, modifiers: ModifierList, declaration_list: Node) -> Node {
        self.new_node(
            SyntaxKind::VariableStatement,
            crate::astdata::VariableStatementData {
                declaration_list: self.id(declaration_list),
                modifiers: self.mods(modifiers),
            },
        )
    }

    // Go: ast/ast_generated.go:1765 NewVariableDeclaration
    pub fn new_variable_declaration(
        &self,
        name: Node,
        exclamation_token: Node,
        type_node: Node,
        initializer: Node,
    ) -> Node {
        self.new_node(
            SyntaxKind::VariableDeclaration,
            crate::astdata::VariableDeclarationData {
                exclamation_token: self.oid(exclamation_token),
                initializer: self.oid(initializer),
                type_: self.oid(type_node),
                name: self.id(name),
            },
        )
    }

    // Go: ast/ast_generated.go:1814 NewVariableDeclarationList
    pub fn new_variable_declaration_list(&self, declarations: NodeList, flags: NodeFlags) -> Node {
        let node = self.new_node(
            SyntaxKind::VariableDeclarationList,
            crate::astdata::VariableDeclarationListData {
                declarations: self.req_list(declarations),
            },
        );
        set_node_flags(node, flags);
        node
    }

    // Go: ast/ast_generated.go:1630 NewExpressionStatement
    pub fn new_expression_statement(&self, expression: Node) -> Node {
        self.new_node(
            SyntaxKind::ExpressionStatement,
            crate::astdata::ExpressionStatementData {
                expression: self.id(expression),
            },
        )
    }

    // Go: ast/ast_generated.go:1200 NewReturnStatement
    pub fn new_return_statement(&self, expression: Node) -> Node {
        self.new_node(
            SyntaxKind::ReturnStatement,
            crate::astdata::ReturnStatementData {
                expression: self.oid(expression),
            },
        )
    }

    // Go: ast/ast_generated.go:874 NewIfStatement
    pub fn new_if_statement(
        &self,
        expression: Node,
        then_statement: Node,
        else_statement: Node,
    ) -> Node {
        self.new_node(
            SyntaxKind::IfStatement,
            crate::astdata::IfStatementData {
                else_statement: self.oid(else_statement),
                expression: self.id(expression),
                then_statement: self.id(then_statement),
            },
        )
    }

    // Go: ast/ast_generated.go:849 NewEmptyStatement
    pub fn new_empty_statement(&self) -> Node {
        self.new_node(
            SyntaxKind::EmptyStatement,
            crate::astdata::EmptyStatementData,
        )
    }

    // Go: ast/ast_generated.go:2536 NewNotEmittedStatement
    pub fn new_not_emitted_statement(&self) -> Node {
        self.new_node(
            SyntaxKind::NotEmittedStatement,
            crate::astdata::NotEmittedStatementData,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn factory_nodes_read_like_go_nodes() {
        let f = NodeFactory::new();
        let this = f.new_keyword_expression(SyntaxKind::ThisKeyword);
        let name = f.new_identifier("x");
        let access = f.new_property_access_expression(
            this,
            Node::NIL,
            name,
            NodeFlags::OPTIONAL_CHAIN | NodeFlags::SYNTHESIZED,
        );

        // Go `newNode`: undefined loc, nil parent, and only OptionalChain kept.
        assert_eq!(access.kind(), SyntaxKind::PropertyAccessExpression);
        assert_eq!(access.flags(), NodeFlags::OPTIONAL_CHAIN);
        assert_eq!(access.loc(), TextRange::undefined());
        assert!(access.parent().is_nil());
        // Children keep their identity, like Go pointers.
        assert_eq!(access.expression(), this);
        assert_eq!(access.name(), name);
        assert_eq!(name.text(), "x");

        // Go field writes after creation.
        set_node_parent(this, access);
        assert_eq!(this.parent(), access);
        set_node_loc(access, TextRange::new(3, 7));
        assert_eq!(access.pos(), 3);

        // Go nil in a field that ts_ast requires still reads as nil.
        let sig =
            f.new_index_signature_declaration(ModifierList::NIL, f.new_node_list(&[]), Node::NIL);
        assert!(sig.type_().is_nil());
        assert_eq!(f.node_count(), 4);
        assert_eq!(f.text_count(), 1);
    }

    #[test]
    fn literal_flags_are_masked_like_go() {
        let f = NodeFactory::new();
        let s = f.new_string_literal("a", TokenFlags(-1));
        assert_eq!(s.token_flags(), TokenFlags::STRING_LITERAL_FLAGS);
        let list = f.new_node_list(&[s]);
        assert_eq!(list.loc(), TextRange::undefined());
        assert_eq!(list.nodes().get(0), s);
    }

    #[test]
    fn synthetic_expression_keeps_its_type() {
        let f = NodeFactory::new();
        let e = f.new_synthetic_expression(TypeId(42), true, Node::NIL);
        assert_eq!(synthetic_expression_type(e), TypeId(42));
        assert!(e.is_spread());
        assert!(e.tuple_name_source().is_nil());
    }
}
