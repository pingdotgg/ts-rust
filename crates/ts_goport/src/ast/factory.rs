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

use crate::astdata::NodeData as D;
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
            NodeFactoryTarget::Synthetic => synthetic_req_list_value(l),
            NodeFactoryTarget::File(store) => store_req_list_value(store, l),
        }
    }

    /// An optional list field.
    pub(crate) fn opt_list(&self, l: NodeList) -> Option<crate::astdata::NodeList> {
        match self.target {
            NodeFactoryTarget::Synthetic => synthetic_list_value(l),
            NodeFactoryTarget::File(store) => store_list_value(store, l),
        }
    }

    /// A modifiers field.
    pub(crate) fn mods(&self, m: ModifierList) -> Option<crate::astdata::ModifierList> {
        match self.target {
            NodeFactoryTarget::Synthetic => synthetic_modifiers_value(m),
            NodeFactoryTarget::File(store) => store_modifiers_value(store, m),
        }
    }

    // Go: ast/ast.go:71 NewNodeFactory
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

    // Go: ast/ast.go:84 (f *NodeFactory) newNode
    pub(crate) fn new_node(&self, kind: SyntaxKind, data: D) -> Node {
        self.node_count.set(self.node_count.get() + 1);
        let node = match self.target {
            NodeFactoryTarget::Synthetic => alloc_synthetic_node(kind, data),
            NodeFactoryTarget::File(store) => alloc_store_node(store, kind, data),
        };
        // Go: ast.go:73
        if let Some(h) = &self.hooks.on_create {
            h(node);
        }
        node
    }

    /// `newNode` plus Go `f.textCount++`, for nodes that carry text.
    pub(crate) fn new_text_node(&self, kind: SyntaxKind, data: D) -> Node {
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
    fn new_name_node(
        &self,
        kind: SyntaxKind,
        text: impl AsRef<str> + Into<String>,
        data: impl FnOnce(String) -> D,
    ) -> Node {
        let NodeFactoryTarget::File(store) = self.target else {
            return self.new_text_node(kind, data(text.into()));
        };
        self.text_count.set(self.text_count.get() + 1);
        self.node_count.set(self.node_count.get() + 1);
        #[cfg(debug_assertions)]
        debug_assert_shared_name_data(kind, &data(String::new()));
        let node = alloc_store_shared_name_node(store, kind, text.as_ref());
        // Go: ast.go:73
        if let Some(h) = &self.hooks.on_create {
            h(node);
        }
        node
    }

    /// `newNode` plus Go `node.Flags |= flags & NodeFlagsOptionalChain`.
    fn new_chain_node(&self, kind: SyntaxKind, data: D, flags: NodeFlags) -> Node {
        let node = self.new_node(kind, data);
        set_node_flags(node, node.flags() | (flags & NodeFlags::OPTIONAL_CHAIN));
        node
    }

    // Go: ast/ast.go:91 NodeCount
    #[must_use]
    pub fn node_count(&self) -> usize {
        self.node_count.get()
    }

    // Go: ast/ast.go:95 TextCount
    #[must_use]
    pub fn text_count(&self) -> usize {
        self.text_count.get()
    }

    // ── Source files ───────────────────────────────────────────────────

    // Go: ast/ast.go:2526 NewSourceFile
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
            D::SourceFile(Box::new(crate::astdata::SourceFileData {
                end_of_file_token: self.id(end_of_file_token),
                locals: crate::astdata::SymbolTable,
                next_container: None,
                statements: self.req_list(statements),
                symbol: None,
                facts: 0,
            })),
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

    // Go: ast/ast.go:129 NewNodeList
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
            NodeFactoryTarget::Synthetic => new_synthetic_node_list(nodes, loc),
            NodeFactoryTarget::File(store) => new_store_node_list(store, nodes, loc),
        }
    }

    // Go: ast/ast.go:160 NewModifierList
    #[must_use]
    pub fn new_modifier_list(&self, nodes: &[Node]) -> ModifierList {
        self.new_modifier_list_with_loc(nodes, TextRange::undefined())
    }

    /// Go `list := f.NewModifierList(nodes); list.Loc = loc` (parser.go
    /// `newModifierList`).
    #[must_use]
    pub fn new_modifier_list_with_loc(&self, nodes: &[Node], loc: TextRange) -> ModifierList {
        match self.target {
            NodeFactoryTarget::Synthetic => new_synthetic_modifier_list(nodes, loc),
            NodeFactoryTarget::File(store) => new_store_modifier_list(store, nodes, loc),
        }
    }

    // ── Tokens, names and literals ─────────────────────────────────────

    // Go: ast/ast_generated.go:600 NewToken
    pub fn new_token(&self, kind: SyntaxKind) -> Node {
        self.new_node(kind, D::Token(Box::new(crate::astdata::TokenData)))
    }

    // Go: ast/ast.go:1674 NewModifier
    pub fn new_modifier(&self, kind: SyntaxKind) -> Node {
        self.new_token(kind)
    }

    // Go: ast/ast_generated.go:627 NewIdentifier
    pub fn new_identifier(&self, text: impl AsRef<str> + Into<String>) -> Node {
        self.new_name_node(SyntaxKind::Identifier, text, |text| {
            D::Identifier(Box::new(crate::astdata::IdentifierData {
                flow_node: None,
                text,
            }))
        })
    }

    // Go: ast/ast_generated.go:651 NewPrivateIdentifier
    pub fn new_private_identifier(&self, text: impl AsRef<str> + Into<String>) -> Node {
        self.new_name_node(SyntaxKind::PrivateIdentifier, text, |text| {
            D::PrivateIdentifier(Box::new(crate::astdata::PrivateIdentifierData { text }))
        })
    }

    // Go: ast/ast_generated.go:678 NewQualifiedName
    pub fn new_qualified_name(&self, left: Node, right: Node) -> Node {
        self.new_node(
            SyntaxKind::QualifiedName,
            D::QualifiedName(Box::new(crate::astdata::QualifiedNameData {
                flow_node: None,
                left: self.id(left),
                right: self.id(right),
                facts: 0,
            })),
        )
    }

    // Go: ast/ast_generated.go:723 NewComputedPropertyName
    pub fn new_computed_property_name(&self, expression: Node) -> Node {
        self.new_node(
            SyntaxKind::ComputedPropertyName,
            D::ComputedPropertyName(Box::new(crate::astdata::ComputedPropertyNameData {
                expression: self.id(expression),
                facts: 0,
            })),
        )
    }

    // Go: ast/ast_generated.go:3601 NewStringLiteral
    pub fn new_string_literal(&self, text: impl Into<String>, flags: TokenFlags) -> Node {
        self.new_text_node(
            SyntaxKind::StringLiteral,
            D::StringLiteral(Box::new(crate::astdata::StringLiteralData {
                text: text.into(),
                token_flags: token_flags(flags & TokenFlags::STRING_LITERAL_FLAGS),
            })),
        )
    }

    // Go: ast/ast_generated.go:3625 NewNumericLiteral
    pub fn new_numeric_literal(&self, text: impl Into<String>, flags: TokenFlags) -> Node {
        self.new_text_node(
            SyntaxKind::NumericLiteral,
            D::NumericLiteral(Box::new(crate::astdata::NumericLiteralData {
                text: text.into(),
                token_flags: token_flags(flags & TokenFlags::NUMERIC_LITERAL_FLAGS),
            })),
        )
    }

    // Go: ast/ast_generated.go:3649 NewBigIntLiteral
    pub fn new_big_int_literal(&self, text: impl Into<String>, flags: TokenFlags) -> Node {
        self.new_text_node(
            SyntaxKind::BigIntLiteral,
            D::BigIntLiteral(Box::new(crate::astdata::BigIntLiteralData {
                text: text.into(),
                token_flags: token_flags(flags & TokenFlags::NUMERIC_LITERAL_FLAGS),
            })),
        )
    }

    // Go: ast/ast_generated.go:3673 NewRegularExpressionLiteral
    pub fn new_regular_expression_literal(
        &self,
        text: impl Into<String>,
        flags: TokenFlags,
    ) -> Node {
        self.new_text_node(
            SyntaxKind::RegularExpressionLiteral,
            D::RegularExpressionLiteral(Box::new(crate::astdata::RegularExpressionLiteralData {
                text: text.into(),
                token_flags: token_flags(flags & TokenFlags::REGULAR_EXPRESSION_LITERAL_FLAGS),
            })),
        )
    }

    // Go: ast/ast_generated.go:3699 NewNoSubstitutionTemplateLiteral
    // PORT: Go sets no `RawText`; ts_ast stores an empty string for it.
    pub fn new_no_substitution_template_literal(
        &self,
        text: impl Into<String>,
        template_flags: TokenFlags,
    ) -> Node {
        self.new_text_node(
            SyntaxKind::NoSubstitutionTemplateLiteral,
            D::NoSubstitutionTemplateLiteral(Box::new(
                crate::astdata::NoSubstitutionTemplateLiteralData {
                    raw_text: String::new(),
                    symbol: None,
                    template_flags: token_flags(
                        template_flags & TokenFlags::TEMPLATE_LITERAL_LIKE_FLAGS,
                    ),
                    text: text.into(),
                    token_flags: NO_TOKEN_FLAGS,
                },
            )),
        )
    }

    // Go: ast/ast_generated.go:6111 NewTemplateHead
    pub fn new_template_head(
        &self,
        text: impl Into<String>,
        raw_text: impl Into<String>,
        template_flags: TokenFlags,
    ) -> Node {
        self.new_text_node(
            SyntaxKind::TemplateHead,
            D::TemplateHead(Box::new(crate::astdata::TemplateHeadData {
                raw_text: raw_text.into(),
                template_flags: token_flags(
                    template_flags & TokenFlags::TEMPLATE_LITERAL_LIKE_FLAGS,
                ),
                text: text.into(),
                token_flags: NO_TOKEN_FLAGS,
            })),
        )
    }

    // Go: ast/ast_generated.go:6137 NewTemplateMiddle
    pub fn new_template_middle(
        &self,
        text: impl Into<String>,
        raw_text: impl Into<String>,
        template_flags: TokenFlags,
    ) -> Node {
        self.new_text_node(
            SyntaxKind::TemplateMiddle,
            D::TemplateMiddle(Box::new(crate::astdata::TemplateMiddleData {
                raw_text: raw_text.into(),
                template_flags: token_flags(
                    template_flags & TokenFlags::TEMPLATE_LITERAL_LIKE_FLAGS,
                ),
                text: text.into(),
                token_flags: NO_TOKEN_FLAGS,
            })),
        )
    }

    // Go: ast/ast_generated.go:6163 NewTemplateTail
    pub fn new_template_tail(
        &self,
        text: impl Into<String>,
        raw_text: impl Into<String>,
        template_flags: TokenFlags,
    ) -> Node {
        self.new_text_node(
            SyntaxKind::TemplateTail,
            D::TemplateTail(Box::new(crate::astdata::TemplateTailData {
                raw_text: raw_text.into(),
                template_flags: token_flags(
                    template_flags & TokenFlags::TEMPLATE_LITERAL_LIKE_FLAGS,
                ),
                text: text.into(),
                token_flags: NO_TOKEN_FLAGS,
            })),
        )
    }

    // ── Type nodes ─────────────────────────────────────────────────────

    // Go: ast/ast_generated.go:5110 NewKeywordTypeNode
    pub fn new_keyword_type_node(&self, kind: SyntaxKind) -> Node {
        self.new_node(
            kind,
            D::KeywordTypeNode(Box::new(crate::astdata::KeywordTypeNodeData)),
        )
    }

    // Go: ast/ast_generated.go:5135 NewUnionTypeNode
    pub fn new_union_type_node(&self, types: NodeList) -> Node {
        self.new_node(
            SyntaxKind::UnionType,
            D::UnionTypeNode(Box::new(crate::astdata::UnionTypeNodeData {
                types: self.req_list(types),
            })),
        )
    }

    // Go: ast/ast_generated.go:5172 NewIntersectionTypeNode
    pub fn new_intersection_type_node(&self, types: NodeList) -> Node {
        self.new_node(
            SyntaxKind::IntersectionType,
            D::IntersectionTypeNode(Box::new(crate::astdata::IntersectionTypeNodeData {
                types: self.req_list(types),
            })),
        )
    }

    // Go: ast/ast_generated.go:5214 NewConditionalTypeNode
    pub fn new_conditional_type_node(
        &self,
        check_type: Node,
        extends_type: Node,
        true_type: Node,
        false_type: Node,
    ) -> Node {
        self.new_node(
            SyntaxKind::ConditionalType,
            D::ConditionalTypeNode(Box::new(crate::astdata::ConditionalTypeNodeData {
                check_type: self.id(check_type),
                extends_type: self.id(extends_type),
                false_type: self.id(false_type),
                locals: crate::astdata::SymbolTable,
                next_container: None,
                true_type: self.id(true_type),
            })),
        )
    }

    // Go: ast/ast_generated.go:5259 NewTypeOperatorNode
    pub fn new_type_operator_node(&self, operator: SyntaxKind, type_node: Node) -> Node {
        self.new_node(
            SyntaxKind::TypeOperator,
            D::TypeOperatorNode(Box::new(crate::astdata::TypeOperatorNodeData {
                operator,
                type_: self.id(type_node),
            })),
        )
    }

    // Go: ast/ast_generated.go:5298 NewInferTypeNode
    pub fn new_infer_type_node(&self, type_parameter: Node) -> Node {
        self.new_node(
            SyntaxKind::InferType,
            D::InferTypeNode(Box::new(crate::astdata::InferTypeNodeData {
                type_parameter: self.id(type_parameter),
            })),
        )
    }

    // Go: ast/ast_generated.go:5336 NewArrayTypeNode
    pub fn new_array_type_node(&self, element_type: Node) -> Node {
        self.new_node(
            SyntaxKind::ArrayType,
            D::ArrayTypeNode(Box::new(crate::astdata::ArrayTypeNodeData {
                element_type: self.id(element_type),
            })),
        )
    }

    // Go: ast/ast_generated.go:5375 NewIndexedAccessTypeNode
    pub fn new_indexed_access_type_node(&self, object_type: Node, index_type: Node) -> Node {
        self.new_node(
            SyntaxKind::IndexedAccessType,
            D::IndexedAccessTypeNode(Box::new(crate::astdata::IndexedAccessTypeNodeData {
                index_type: self.id(index_type),
                object_type: self.id(object_type),
            })),
        )
    }

    // Go: ast/ast_generated.go:5414 NewTypeReferenceNode
    pub fn new_type_reference_node(&self, type_name: Node, type_arguments: NodeList) -> Node {
        self.new_node(
            SyntaxKind::TypeReference,
            D::TypeReferenceNode(Box::new(crate::astdata::TypeReferenceNodeData {
                type_arguments: self.opt_list(type_arguments),
                type_name: self.id(type_name),
            })),
        )
    }

    // Go: ast/ast_generated.go:5455 NewExpressionWithTypeArguments
    pub fn new_expression_with_type_arguments(
        &self,
        expression: Node,
        type_arguments: NodeList,
    ) -> Node {
        self.new_node(
            SyntaxKind::ExpressionWithTypeArguments,
            D::ExpressionWithTypeArguments(Box::new(
                crate::astdata::ExpressionWithTypeArgumentsData {
                    expression: self.id(expression),
                    type_arguments: self.opt_list(type_arguments),
                    facts: 0,
                },
            )),
        )
    }

    // Go: ast/ast_generated.go:5494 NewLiteralTypeNode
    pub fn new_literal_type_node(&self, literal: Node) -> Node {
        self.new_node(
            SyntaxKind::LiteralType,
            D::LiteralTypeNode(Box::new(crate::astdata::LiteralTypeNodeData {
                literal: self.id(literal),
            })),
        )
    }

    // Go: ast/ast_generated.go:5531 NewThisTypeNode
    pub fn new_this_type_node(&self) -> Node {
        self.new_node(
            SyntaxKind::ThisType,
            D::ThisTypeNode(Box::new(crate::astdata::ThisTypeNodeData)),
        )
    }

    // Go: ast/ast_generated.go:5555 NewTypePredicateNode
    pub fn new_type_predicate_node(
        &self,
        asserts_modifier: Node,
        parameter_name: Node,
        type_node: Node,
    ) -> Node {
        self.new_node(
            SyntaxKind::TypePredicate,
            D::TypePredicateNode(Box::new(crate::astdata::TypePredicateNodeData {
                asserts_modifier: self.oid(asserts_modifier),
                parameter_name: self.id(parameter_name),
                type_: self.oid(type_node),
            })),
        )
    }

    // Go: ast/ast_generated.go:5692 NewTypeQueryNode
    pub fn new_type_query_node(&self, expr_name: Node, type_arguments: NodeList) -> Node {
        self.new_node(
            SyntaxKind::TypeQuery,
            D::TypeQueryNode(Box::new(crate::astdata::TypeQueryNodeData {
                expr_name: self.id(expr_name),
                type_arguments: self.opt_list(type_arguments),
            })),
        )
    }

    // Go: ast/ast_generated.go:5738 NewMappedTypeNode
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
            D::MappedTypeNode(Box::new(crate::astdata::MappedTypeNodeData {
                locals: crate::astdata::SymbolTable,
                members: self.opt_list(members),
                name_type: self.oid(name_type),
                next_container: None,
                question_token: self.oid(question_token),
                readonly_token: self.oid(readonly_token),
                symbol: None,
                type_: self.oid(type_node),
                type_parameter: self.id(type_parameter),
            })),
        )
    }

    // Go: ast/ast_generated.go:5787 NewTypeLiteralNode
    pub fn new_type_literal_node(&self, members: NodeList) -> Node {
        self.new_node(
            SyntaxKind::TypeLiteral,
            D::TypeLiteralNode(Box::new(crate::astdata::TypeLiteralNodeData {
                members: self.req_list(members),
                symbol: None,
            })),
        )
    }

    // Go: ast/ast_generated.go:5825 NewTupleTypeNode
    pub fn new_tuple_type_node(&self, elements: NodeList) -> Node {
        self.new_node(
            SyntaxKind::TupleType,
            D::TupleTypeNode(Box::new(crate::astdata::TupleTypeNodeData {
                elements: self.req_list(elements),
            })),
        )
    }

    // Go: ast/ast_generated.go:5867 NewNamedTupleMember
    pub fn new_named_tuple_member(
        &self,
        dot_dot_dot_token: Node,
        name: Node,
        question_token: Node,
        type_node: Node,
    ) -> Node {
        self.new_node(
            SyntaxKind::NamedTupleMember,
            D::NamedTupleMember(Box::new(crate::astdata::NamedTupleMemberData {
                dot_dot_dot_token: self.oid(dot_dot_dot_token),
                question_token: self.oid(question_token),
                symbol: None,
                type_: self.id(type_node),
                name: self.id(name),
            })),
        )
    }

    // Go: ast/ast_generated.go:5915 NewOptionalTypeNode
    pub fn new_optional_type_node(&self, type_node: Node) -> Node {
        self.new_node(
            SyntaxKind::OptionalType,
            D::OptionalTypeNode(Box::new(crate::astdata::OptionalTypeNodeData {
                type_: self.id(type_node),
            })),
        )
    }

    // Go: ast/ast_generated.go:5953 NewRestTypeNode
    pub fn new_rest_type_node(&self, type_node: Node) -> Node {
        self.new_node(
            SyntaxKind::RestType,
            D::RestTypeNode(Box::new(crate::astdata::RestTypeNodeData {
                type_: self.id(type_node),
            })),
        )
    }

    // Go: ast/ast_generated.go:5991 NewParenthesizedTypeNode
    pub fn new_parenthesized_type_node(&self, type_node: Node) -> Node {
        self.new_node(
            SyntaxKind::ParenthesizedType,
            D::ParenthesizedTypeNode(Box::new(crate::astdata::ParenthesizedTypeNodeData {
                type_: self.id(type_node),
            })),
        )
    }

    // Go: ast/ast_generated.go:6028 NewFunctionTypeNode
    pub fn new_function_type_node(
        &self,
        type_parameters: NodeList,
        parameters: NodeList,
        type_node: Node,
    ) -> Node {
        self.new_node(
            SyntaxKind::FunctionType,
            D::FunctionTypeNode(Box::new(crate::astdata::FunctionTypeNodeData {
                full_signature: None,
                locals: crate::astdata::SymbolTable,
                next_container: None,
                parameters: self.req_list(parameters),
                symbol: None,
                type_: self.oid(type_node),
                type_parameters: self.opt_list(type_parameters),
                modifiers: None,
            })),
        )
    }

    // Go: ast/ast_generated.go:6067 NewConstructorTypeNode
    pub fn new_constructor_type_node(
        &self,
        modifiers: ModifierList,
        type_parameters: NodeList,
        parameters: NodeList,
        type_node: Node,
    ) -> Node {
        self.new_node(
            SyntaxKind::ConstructorType,
            D::ConstructorTypeNode(Box::new(crate::astdata::ConstructorTypeNodeData {
                full_signature: None,
                locals: crate::astdata::SymbolTable,
                next_container: None,
                parameters: self.req_list(parameters),
                symbol: None,
                type_: self.oid(type_node),
                type_parameters: self.opt_list(type_parameters),
                modifiers: self.mods(modifiers),
            })),
        )
    }

    // Go: ast/ast_generated.go:6190 NewTemplateLiteralTypeNode
    pub fn new_template_literal_type_node(&self, head: Node, template_spans: NodeList) -> Node {
        self.new_node(
            SyntaxKind::TemplateLiteralType,
            D::TemplateLiteralTypeNode(Box::new(crate::astdata::TemplateLiteralTypeNodeData {
                head: self.id(head),
                template_spans: self.req_list(template_spans),
            })),
        )
    }

    // Go: ast/ast_generated.go:6230 NewTemplateLiteralTypeSpan
    pub fn new_template_literal_type_span(&self, type_node: Node, literal: Node) -> Node {
        self.new_node(
            SyntaxKind::TemplateLiteralTypeSpan,
            D::TemplateLiteralTypeSpan(Box::new(crate::astdata::TemplateLiteralTypeSpanData {
                literal: self.id(literal),
                type_: self.id(type_node),
            })),
        )
    }

    // Go: ast/ast_generated.go:8203 NewImportTypeNode
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
            D::ImportTypeNode(Box::new(crate::astdata::ImportTypeNodeData {
                argument: self.id(argument),
                attributes: self.oid(attributes),
                is_type_of,
                qualifier: self.oid(qualifier),
                type_arguments: self.opt_list(type_arguments),
            })),
        )
    }

    // ── Signatures and type elements ───────────────────────────────────

    // Go: ast/ast_generated.go:8507 NewTypeParameterDeclaration
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
            D::TypeParameterDeclaration(Box::new(crate::astdata::TypeParameterDeclarationData {
                constraint: self.oid(constraint),
                default_type: self.oid(default_type),
                expression: self.oid(expression),
                symbol: None,
                modifiers: self.mods(modifiers),
                name: self.id(name),
            })),
        )
    }

    // Go: ast/ast_generated.go:1848 NewParameterDeclaration
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
            D::ParameterDeclaration(Box::new(crate::astdata::ParameterDeclarationData {
                dot_dot_dot_token: self.oid(dot_dot_dot_token),
                initializer: self.oid(initializer),
                question_token: self.oid(question_token),
                symbol: None,
                type_: self.oid(type_node),
                facts: 0,
                modifiers: self.mods(modifiers),
                name: self.id(name),
            })),
        )
    }

    // Go: ast/ast_generated.go:2970 NewCallSignatureDeclaration
    pub fn new_call_signature_declaration(
        &self,
        type_parameters: NodeList,
        parameters: NodeList,
        type_node: Node,
    ) -> Node {
        self.new_node(
            SyntaxKind::CallSignature,
            D::CallSignatureDeclaration(Box::new(crate::astdata::CallSignatureDeclarationData {
                full_signature: None,
                locals: crate::astdata::SymbolTable,
                next_container: None,
                parameters: self.req_list(parameters),
                symbol: None,
                type_: self.oid(type_node),
                type_parameters: self.opt_list(type_parameters),
            })),
        )
    }

    // Go: ast/ast_generated.go:3013 NewConstructSignatureDeclaration
    pub fn new_construct_signature_declaration(
        &self,
        type_parameters: NodeList,
        parameters: NodeList,
        type_node: Node,
    ) -> Node {
        self.new_node(
            SyntaxKind::ConstructSignature,
            D::ConstructSignatureDeclaration(Box::new(
                crate::astdata::ConstructSignatureDeclarationData {
                    full_signature: None,
                    locals: crate::astdata::SymbolTable,
                    next_container: None,
                    parameters: self.req_list(parameters),
                    symbol: None,
                    type_: self.oid(type_node),
                    type_parameters: self.opt_list(type_parameters),
                },
            )),
        )
    }

    // Go: ast/ast_generated.go:3058 NewConstructorDeclaration
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
            D::ConstructorDeclaration(Box::new(crate::astdata::ConstructorDeclarationData {
                asterisk_token: None,
                body: self.oid(body),
                end_flow_node: None,
                full_signature: self.oid(full_signature),
                locals: crate::astdata::SymbolTable,
                next_container: None,
                parameters: self.req_list(parameters),
                return_flow_node: None,
                symbol: None,
                type_: self.oid(type_node),
                type_parameters: self.opt_list(type_parameters),
                facts: 0,
                modifiers: self.mods(modifiers),
            })),
        )
    }

    // Go: ast/ast_generated.go:3105 NewGetAccessorDeclaration
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
            D::GetAccessorDeclaration(Box::new(crate::astdata::GetAccessorDeclarationData {
                asterisk_token: None,
                body: self.oid(body),
                end_flow_node: None,
                flow_node: None,
                full_signature: self.oid(full_signature),
                locals: crate::astdata::SymbolTable,
                next_container: None,
                parameters: self.req_list(parameters),
                postfix_token: None,
                symbol: None,
                type_: self.oid(type_node),
                type_parameters: self.opt_list(type_parameters),
                facts: 0,
                modifiers: self.mods(modifiers),
                name: self.id(name),
            })),
        )
    }

    // Go: ast/ast_generated.go:3158 NewSetAccessorDeclaration
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
            D::SetAccessorDeclaration(Box::new(crate::astdata::SetAccessorDeclarationData {
                asterisk_token: None,
                body: self.oid(body),
                end_flow_node: None,
                flow_node: None,
                full_signature: self.oid(full_signature),
                locals: crate::astdata::SymbolTable,
                next_container: None,
                parameters: self.req_list(parameters),
                postfix_token: None,
                symbol: None,
                type_: self.oid(type_node),
                type_parameters: self.opt_list(type_parameters),
                facts: 0,
                modifiers: self.mods(modifiers),
                name: self.id(name),
            })),
        )
    }

    // Go: ast/ast_generated.go:3217 NewIndexSignatureDeclaration
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
            D::IndexSignatureDeclaration(Box::new(crate::astdata::IndexSignatureDeclarationData {
                full_signature: None,
                locals: crate::astdata::SymbolTable,
                next_container: None,
                parameters: self.req_list(parameters),
                symbol: None,
                type_: self.id(type_node),
                type_parameters: None,
                modifiers: self.mods(modifiers),
            })),
        )
    }

    // Go: ast/ast_generated.go:3261 NewMethodSignatureDeclaration
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
            D::MethodSignatureDeclaration(Box::new(
                crate::astdata::MethodSignatureDeclarationData {
                    full_signature: None,
                    locals: crate::astdata::SymbolTable,
                    next_container: None,
                    parameters: self.req_list(parameters),
                    postfix_token: self.oid(postfix_token),
                    symbol: None,
                    type_: self.oid(type_node),
                    type_parameters: self.opt_list(type_parameters),
                    modifiers: self.mods(modifiers),
                    name: self.id(name),
                },
            )),
        )
    }

    // Go: ast/ast_generated.go:3319 NewMethodDeclaration
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
            D::MethodDeclaration(Box::new(crate::astdata::MethodDeclarationData {
                asterisk_token: self.oid(asterisk_token),
                body: self.oid(body),
                end_flow_node: None,
                flow_node: None,
                full_signature: self.oid(full_signature),
                locals: crate::astdata::SymbolTable,
                next_container: None,
                parameters: self.req_list(parameters),
                postfix_token: self.oid(postfix_token),
                symbol: None,
                type_: self.oid(type_node),
                type_parameters: self.opt_list(type_parameters),
                facts: 0,
                modifiers: self.mods(modifiers),
                name: self.id(name),
            })),
        )
    }

    // Go: ast/ast_generated.go:3382 NewPropertySignatureDeclaration
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
            D::PropertySignatureDeclaration(Box::new(
                crate::astdata::PropertySignatureDeclarationData {
                    initializer: self.id(initializer),
                    postfix_token: self.oid(postfix_token),
                    symbol: None,
                    type_: self.id(type_node),
                    modifiers: self.mods(modifiers),
                    name: self.id(name),
                },
            )),
        )
    }

    // Go: ast/ast_generated.go:3437 NewPropertyDeclaration
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
            D::PropertyDeclaration(Box::new(crate::astdata::PropertyDeclarationData {
                initializer: self.oid(initializer),
                postfix_token: self.oid(postfix_token),
                symbol: None,
                type_: self.oid(type_node),
                facts: 0,
                modifiers: self.mods(modifiers),
                name: self.id(name),
            })),
        )
    }

    // Go: ast/ast_generated.go:2498 NewNotEmittedTypeElement
    pub fn new_not_emitted_type_element(&self) -> Node {
        self.new_node(
            SyntaxKind::NotEmittedTypeElement,
            D::NotEmittedTypeElement(Box::new(crate::astdata::NotEmittedTypeElementData)),
        )
    }

    // ── Declarations ───────────────────────────────────────────────────

    // Go: ast/ast_generated.go:2341 NewEnumMember
    pub fn new_enum_member(&self, name: Node, initializer: Node) -> Node {
        self.new_node(
            SyntaxKind::EnumMember,
            D::EnumMember(Box::new(crate::astdata::EnumMemberData {
                initializer: self.oid(initializer),
                postfix_token: None,
                symbol: None,
                facts: 0,
                modifiers: None,
                name: self.id(name),
            })),
        )
    }

    // Go: ast/ast_generated.go:2389 NewEnumDeclaration
    pub fn new_enum_declaration(
        &self,
        modifiers: ModifierList,
        name: Node,
        members: NodeList,
    ) -> Node {
        self.new_node(
            SyntaxKind::EnumDeclaration,
            D::EnumDeclaration(Box::new(crate::astdata::EnumDeclarationData {
                flow_node: None,
                local_symbol: None,
                members: self.req_list(members),
                symbol: None,
                facts: 0,
                modifiers: self.mods(modifiers),
                name: self.id(name),
            })),
        )
    }

    // Go: ast/ast_generated.go:2000 NewFunctionDeclaration
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
            D::FunctionDeclaration(Box::new(crate::astdata::FunctionDeclarationData {
                asterisk_token: self.oid(asterisk_token),
                body: self.oid(body),
                end_flow_node: None,
                flow_node: None,
                full_signature: self.oid(full_signature),
                local_symbol: None,
                locals: crate::astdata::SymbolTable,
                next_container: None,
                parameters: self.req_list(parameters),
                return_flow_node: None,
                symbol: None,
                type_: self.oid(type_node),
                type_parameters: self.opt_list(type_parameters),
                facts: 0,
                modifiers: self.mods(modifiers),
                name: self.oid(name),
            })),
        )
    }

    // Go: ast/ast_generated.go:2057 NewClassDeclaration
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
            D::ClassDeclaration(Box::new(crate::astdata::ClassDeclarationData {
                flow_node: None,
                heritage_clauses: self.opt_list(heritage_clauses),
                local_symbol: None,
                locals: crate::astdata::SymbolTable,
                members: self.req_list(members),
                next_container: None,
                symbol: None,
                type_parameters: self.opt_list(type_parameters),
                facts: 0,
                modifiers: self.mods(modifiers),
                name: self.oid(name),
            })),
        )
    }

    // Go: ast/ast_generated.go:2108 NewClassExpression
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
            D::ClassExpression(Box::new(crate::astdata::ClassExpressionData {
                heritage_clauses: self.opt_list(heritage_clauses),
                local_symbol: None,
                locals: crate::astdata::SymbolTable,
                members: self.req_list(members),
                next_container: None,
                symbol: None,
                type_parameters: self.opt_list(type_parameters),
                facts: 0,
                modifiers: self.mods(modifiers),
                name: self.oid(name),
            })),
        )
    }

    // Go: ast/ast_generated.go:2160 NewHeritageClause
    pub fn new_heritage_clause(&self, token: SyntaxKind, types: NodeList) -> Node {
        self.new_node(
            SyntaxKind::HeritageClause,
            D::HeritageClause(Box::new(crate::astdata::HeritageClauseData {
                token,
                types: self.req_list(types),
                facts: 0,
            })),
        )
    }

    // Go: ast/ast_generated.go:2206 NewInterfaceDeclaration
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
            D::InterfaceDeclaration(Box::new(crate::astdata::InterfaceDeclarationData {
                flow_node: None,
                heritage_clauses: self.opt_list(heritage_clauses),
                local_symbol: None,
                members: self.req_list(members),
                symbol: None,
                type_parameters: self.opt_list(type_parameters),
                modifiers: self.mods(modifiers),
                name: self.id(name),
            })),
        )
    }

    // Go: ast/ast_generated.go:2263 NewTypeAliasDeclaration
    pub fn new_type_alias_declaration(
        &self,
        modifiers: ModifierList,
        name: Node,
        type_parameters: NodeList,
        type_node: Node,
    ) -> Node {
        self.new_node(
            SyntaxKind::TypeAliasDeclaration,
            D::TypeAliasDeclaration(Box::new(crate::astdata::TypeAliasDeclarationData {
                flow_node: None,
                local_symbol: None,
                locals: crate::astdata::SymbolTable,
                next_container: None,
                symbol: None,
                type_: self.id(type_node),
                type_parameters: self.opt_list(type_parameters),
                modifiers: self.mods(modifiers),
                name: self.id(name),
            })),
        )
    }

    // Go: ast/ast_generated.go:8049 NewModuleDeclaration
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
            D::ModuleDeclaration(Box::new(crate::astdata::ModuleDeclarationData {
                asterisk_token: None,
                attributes: self.oid(attributes),
                body: self.oid(body),
                end_flow_node: None,
                flow_node: None,
                keyword,
                local_symbol: None,
                locals: crate::astdata::SymbolTable,
                next_container: None,
                symbol: None,
                facts: 0,
                modifiers: self.mods(modifiers),
                name: self.id(name),
            })),
        )
    }

    // Go: ast/ast_generated.go:2434 NewModuleBlock
    pub fn new_module_block(&self, statements: NodeList) -> Node {
        self.new_node(
            SyntaxKind::ModuleBlock,
            D::ModuleBlock(Box::new(crate::astdata::ModuleBlockData {
                flow_node: None,
                statements: self.req_list(statements),
                facts: 0,
            })),
        )
    }

    // Go: ast/ast_generated.go:5597 NewImportAttribute
    pub fn new_import_attribute(&self, name: Node, value: Node) -> Node {
        self.new_node(
            SyntaxKind::ImportAttribute,
            D::ImportAttribute(Box::new(crate::astdata::ImportAttributeData {
                value: self.id(value),
                facts: 0,
                name: self.id(name),
            })),
        )
    }

    // Go: ast/ast_generated.go:5648 NewImportAttributes
    pub fn new_import_attributes(
        &self,
        token: SyntaxKind,
        attributes: NodeList,
        multi_line: bool,
    ) -> Node {
        self.new_node(
            SyntaxKind::ImportAttributes,
            D::ImportAttributes(Box::new(crate::astdata::ImportAttributesData {
                attributes: self.req_list(attributes),
                multi_line,
                token,
                facts: 0,
            })),
        )
    }

    // Go: ast/ast_generated.go:2741 NewExportAssignment
    pub fn new_export_assignment(
        &self,
        modifiers: ModifierList,
        is_export_equals: bool,
        type_node: Node,
        expression: Node,
    ) -> Node {
        self.new_node(
            SyntaxKind::ExportAssignment,
            D::ExportAssignment(Box::new(crate::astdata::ExportAssignmentData {
                expression: self.id(expression),
                flow_node: None,
                is_export_equals,
                symbol: None,
                type_: self.oid(type_node),
                facts: 0,
                modifiers: self.mods(modifiers),
            })),
        )
    }

    // Go: ast/ast_generated.go:2876 NewNamedExports
    pub fn new_named_exports(&self, elements: NodeList) -> Node {
        self.new_node(
            SyntaxKind::NamedExports,
            D::NamedExports(Box::new(crate::astdata::NamedExportsData {
                elements: self.req_list(elements),
                facts: 0,
            })),
        )
    }

    // Go: ast/ast_generated.go:2923 NewExportSpecifier
    pub fn new_export_specifier(
        &self,
        is_type_only: bool,
        property_name: Node,
        name: Node,
    ) -> Node {
        self.new_node(
            SyntaxKind::ExportSpecifier,
            D::ExportSpecifier(Box::new(crate::astdata::ExportSpecifierData {
                is_type_only,
                local_symbol: None,
                property_name: self.oid(property_name),
                symbol: None,
                facts: 0,
                name: self.id(name),
            })),
        )
    }

    // Go: ast/ast_generated.go:8155 NewExportDeclaration
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
            D::ExportDeclaration(Box::new(crate::astdata::ExportDeclarationData {
                attributes: self.oid(attributes),
                export_clause: self.oid(export_clause),
                flow_node: None,
                is_type_only,
                module_specifier: self.oid(module_specifier),
                symbol: None,
                facts: 0,
                modifiers: self.mods(modifiers),
            })),
        )
    }

    // Go: ast/ast_generated.go:2525 NewImportDeclaration
    pub fn new_import_declaration(
        &self,
        modifiers: ModifierList,
        import_clause: Node,
        module_specifier: Node,
        attributes: Node,
    ) -> Node {
        self.new_node(
            SyntaxKind::ImportDeclaration,
            D::ImportDeclaration(Box::new(crate::astdata::ImportDeclarationData {
                attributes: self.oid(attributes),
                flow_node: None,
                import_clause: self.oid(import_clause),
                module_specifier: self.id(module_specifier),
                symbol: None,
                facts: 0,
                modifiers: self.mods(modifiers),
            })),
        )
    }

    // Go: ast/ast_generated.go:2603 NewExternalModuleReference
    pub fn new_external_module_reference(&self, expression: Node) -> Node {
        self.new_node(
            SyntaxKind::ExternalModuleReference,
            D::ExternalModuleReference(Box::new(crate::astdata::ExternalModuleReferenceData {
                expression: self.id(expression),
            })),
        )
    }

    // Go: ast/ast_generated.go:2694 NewNamedImports
    pub fn new_named_imports(&self, elements: NodeList) -> Node {
        self.new_node(
            SyntaxKind::NamedImports,
            D::NamedImports(Box::new(crate::astdata::NamedImportsData {
                elements: self.req_list(elements),
                facts: 0,
            })),
        )
    }

    // Go: ast/ast_generated.go:6842 NewSyntaxList
    pub fn new_syntax_list(&self, children: &[Node]) -> Node {
        let children = children.iter().map(|&n| self.id(n)).collect();
        self.new_node(
            SyntaxKind::SyntaxList,
            D::SyntaxList(Box::new(crate::astdata::SyntaxListData { children })),
        )
    }

    // Go: ast/ast_generated.go:8104 NewImportEqualsDeclaration
    pub fn new_import_equals_declaration(
        &self,
        modifiers: ModifierList,
        is_type_only: bool,
        name: Node,
        module_reference: Node,
    ) -> Node {
        self.new_node(
            SyntaxKind::ImportEqualsDeclaration,
            D::ImportEqualsDeclaration(Box::new(crate::astdata::ImportEqualsDeclarationData {
                flow_node: None,
                is_type_only,
                local_symbol: None,
                module_reference: self.id(module_reference),
                symbol: None,
                facts: 0,
                modifiers: self.mods(modifiers),
                name: self.id(name),
            })),
        )
    }

    // Go: ast/ast_generated.go:8253 NewImportClause
    // PORT: Go KindUnknown for `phaseModifier` is `None` in the data.
    pub fn new_import_clause(
        &self,
        phase_modifier: SyntaxKind,
        name: Node,
        named_bindings: Node,
    ) -> Node {
        self.new_node(
            SyntaxKind::ImportClause,
            D::ImportClause(Box::new(crate::astdata::ImportClauseData {
                local_symbol: None,
                named_bindings: self.oid(named_bindings),
                phase_modifier: if phase_modifier == SyntaxKind::Unknown {
                    None
                } else {
                    Some(phase_modifier)
                },
                symbol: None,
                facts: 0,
                name: self.oid(name),
            })),
        )
    }

    // Go: ast/ast_generated.go:8302 NewImportSpecifier
    pub fn new_import_specifier(
        &self,
        is_type_only: bool,
        property_name: Node,
        name: Node,
    ) -> Node {
        self.new_node(
            SyntaxKind::ImportSpecifier,
            D::ImportSpecifier(Box::new(crate::astdata::ImportSpecifierData {
                is_type_only,
                local_symbol: None,
                property_name: self.oid(property_name),
                symbol: None,
                facts: 0,
                name: self.id(name),
            })),
        )
    }

    // ── Expressions ────────────────────────────────────────────────────

    // Go: ast/ast_generated.go:3576 NewKeywordExpression
    pub fn new_keyword_expression(&self, kind: SyntaxKind) -> Node {
        self.new_node(
            kind,
            D::KeywordExpression(Box::new(crate::astdata::KeywordExpressionData {
                flow_node: None,
            })),
        )
    }

    // Go: ast/ast_generated.go:4164 NewPropertyAccessExpression
    pub fn new_property_access_expression(
        &self,
        expression: Node,
        question_dot_token: Node,
        name: Node,
        flags: NodeFlags,
    ) -> Node {
        self.new_chain_node(
            SyntaxKind::PropertyAccessExpression,
            D::PropertyAccessExpression(Box::new(crate::astdata::PropertyAccessExpressionData {
                expression: self.id(expression),
                flow_node: None,
                question_dot_token: self.oid(question_dot_token),
                facts: 0,
                name: self.id(name),
            })),
            flags,
        )
    }

    // Go: ast/ast_generated.go:4214 NewElementAccessExpression
    pub fn new_element_access_expression(
        &self,
        expression: Node,
        question_dot_token: Node,
        argument_expression: Node,
        flags: NodeFlags,
    ) -> Node {
        self.new_chain_node(
            SyntaxKind::ElementAccessExpression,
            D::ElementAccessExpression(Box::new(crate::astdata::ElementAccessExpressionData {
                argument_expression: self.id(argument_expression),
                expression: self.id(expression),
                flow_node: None,
                question_dot_token: self.oid(question_dot_token),
                facts: 0,
            })),
            flags,
        )
    }

    // Go: ast/ast_generated.go:4267 NewCallExpression
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
            D::CallExpression(Box::new(crate::astdata::CallExpressionData {
                arguments: self.req_list(arguments),
                expression: self.id(expression),
                question_dot_token: self.oid(question_dot_token),
                symbol: None,
                type_arguments: self.opt_list(type_arguments),
                facts: 0,
            })),
            flags,
        )
    }

    // Go: ast/ast_generated.go:4316 NewNewExpression
    pub fn new_new_expression(
        &self,
        expression: Node,
        type_arguments: NodeList,
        arguments: NodeList,
    ) -> Node {
        self.new_node(
            SyntaxKind::NewExpression,
            D::NewExpression(Box::new(crate::astdata::NewExpressionData {
                arguments: self.opt_list(arguments),
                expression: self.id(expression),
                type_arguments: self.opt_list(type_arguments),
                facts: 0,
            })),
        )
    }

    // Go: ast/ast_generated.go:4402 NewNonNullExpression
    pub fn new_non_null_expression(&self, expression: Node, flags: NodeFlags) -> Node {
        self.new_chain_node(
            SyntaxKind::NonNullExpression,
            D::NonNullExpression(Box::new(crate::astdata::NonNullExpressionData {
                expression: self.id(expression),
            })),
            flags,
        )
    }

    // Go: ast/ast_generated.go:4575 NewTaggedTemplateExpression
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
            D::TaggedTemplateExpression(Box::new(crate::astdata::TaggedTemplateExpressionData {
                question_dot_token: self.oid(question_dot_token),
                tag: self.id(tag),
                template: self.id(template),
                type_arguments: self.opt_list(type_arguments),
                facts: 0,
            })),
            flags,
        )
    }

    // Go: ast/ast_generated.go:3777 NewPrefixUnaryExpression
    pub fn new_prefix_unary_expression(&self, operator: SyntaxKind, operand: Node) -> Node {
        self.new_node(
            SyntaxKind::PrefixUnaryExpression,
            D::PrefixUnaryExpression(Box::new(crate::astdata::PrefixUnaryExpressionData {
                operand: self.id(operand),
                operator,
            })),
        )
    }

    // Go: ast/ast_generated.go:3730 NewBinaryExpression
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
            D::BinaryExpression(Box::new(crate::astdata::BinaryExpressionData {
                left: self.id(left),
                operator_token: self.id(operator_token),
                right: self.id(right),
                symbol: None,
                type_: self.oid(type_node),
                facts: 0,
                modifiers: self.mods(modifiers),
            })),
        )
    }

    // Go: ast/ast_generated.go:4106 NewConditionalExpression
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
            D::ConditionalExpression(Box::new(crate::astdata::ConditionalExpressionData {
                colon_token: self.id(colon_token),
                condition: self.id(condition),
                question_token: self.id(question_token),
                when_false: self.id(when_false),
                when_true: self.id(when_true),
                facts: 0,
            })),
        )
    }

    // Go: ast/ast_generated.go:4621 NewParenthesizedExpression
    pub fn new_parenthesized_expression(&self, expression: Node) -> Node {
        self.new_node(
            SyntaxKind::ParenthesizedExpression,
            D::ParenthesizedExpression(Box::new(crate::astdata::ParenthesizedExpressionData {
                expression: self.id(expression),
            })),
        )
    }

    // Go: ast/ast_generated.go:3965 NewFunctionExpression
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
            D::FunctionExpression(Box::new(crate::astdata::FunctionExpressionData {
                asterisk_token: self.oid(asterisk_token),
                body: self.id(body),
                end_flow_node: None,
                flow_node: None,
                full_signature: self.oid(full_signature),
                locals: crate::astdata::SymbolTable,
                next_container: None,
                parameters: self.req_list(parameters),
                return_flow_node: None,
                symbol: None,
                type_: self.oid(type_node),
                type_parameters: self.opt_list(type_parameters),
                facts: 0,
                modifiers: self.mods(modifiers),
                name: self.oid(name),
            })),
        )
    }

    // Go: ast/ast_generated.go:3909 NewArrowFunction
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
            D::ArrowFunction(Box::new(crate::astdata::ArrowFunctionData {
                asterisk_token: None,
                body: self.id(body),
                end_flow_node: None,
                equals_greater_than_token: self.id(equals_greater_than_token),
                flow_node: None,
                full_signature: self.oid(full_signature),
                locals: crate::astdata::SymbolTable,
                next_container: None,
                parameters: self.req_list(parameters),
                symbol: None,
                type_: self.oid(type_node),
                type_parameters: self.opt_list(type_parameters),
                facts: 0,
                modifiers: self.mods(modifiers),
            })),
        )
    }

    // Go: ast/ast_generated.go:6271 NewSyntheticExpression
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
            D::SyntheticExpression(Box::new(crate::astdata::SyntheticExpressionData {
                is_spread,
                tuple_name_source: self.oid(tuple_name_source),
                type_: crate::astdata::OpaqueValue,
            })),
        );
        set_synthetic_expression_type(node, type_);
        node
    }

    // Go: ast/ast_generated.go:4442 NewSpreadElement
    pub fn new_spread_element(&self, expression: Node) -> Node {
        self.new_node(
            SyntaxKind::SpreadElement,
            D::SpreadElement(Box::new(crate::astdata::SpreadElementData {
                expression: self.id(expression),
            })),
        )
    }

    // Go: ast/ast_generated.go:4665 NewArrayLiteralExpression
    pub fn new_array_literal_expression(&self, elements: NodeList, multi_line: bool) -> Node {
        self.new_node(
            SyntaxKind::ArrayLiteralExpression,
            D::ArrayLiteralExpression(Box::new(crate::astdata::ArrayLiteralExpressionData {
                elements: self.req_list(elements),
                multi_line,
                facts: 0,
            })),
        )
    }

    // Go: ast/ast_generated.go:4711 NewObjectLiteralExpression
    pub fn new_object_literal_expression(&self, properties: NodeList, multi_line: bool) -> Node {
        self.new_node(
            SyntaxKind::ObjectLiteralExpression,
            D::ObjectLiteralExpression(Box::new(crate::astdata::ObjectLiteralExpressionData {
                multi_line,
                properties: self.req_list(properties),
                symbol: None,
                facts: 0,
            })),
        )
    }

    // Go: ast/ast_generated.go:4799 NewPropertyAssignment
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
            D::PropertyAssignment(Box::new(crate::astdata::PropertyAssignmentData {
                initializer: self.id(initializer),
                postfix_token: self.oid(postfix_token),
                symbol: None,
                type_: self.oid(type_node),
                facts: 0,
                modifiers: self.mods(modifiers),
                name: self.id(name),
            })),
        )
    }

    // Go: ast/ast_generated.go:4022 NewAsExpression
    pub fn new_as_expression(&self, expression: Node, type_node: Node) -> Node {
        self.new_node(
            SyntaxKind::AsExpression,
            D::AsExpression(Box::new(crate::astdata::AsExpressionData {
                expression: self.id(expression),
                type_: self.id(type_node),
            })),
        )
    }

    // Go: ast/ast_generated.go:4062 NewSatisfiesExpression
    pub fn new_satisfies_expression(&self, expression: Node, type_node: Node) -> Node {
        self.new_node(
            SyntaxKind::SatisfiesExpression,
            D::SatisfiesExpression(Box::new(crate::astdata::SatisfiesExpressionData {
                expression: self.id(expression),
                type_: self.id(type_node),
            })),
        )
    }

    // Go: ast/ast_generated.go:5072 NewTypeAssertion
    pub fn new_type_assertion(&self, type_node: Node, expression: Node) -> Node {
        self.new_node(
            SyntaxKind::TypeAssertionExpression,
            D::TypeAssertion(Box::new(crate::astdata::TypeAssertionData {
                expression: self.id(expression),
                type_: self.id(type_node),
            })),
        )
    }

    // Go: ast/ast_generated.go:4907 NewDeleteExpression
    pub fn new_delete_expression(&self, expression: Node) -> Node {
        self.new_node(
            SyntaxKind::DeleteExpression,
            D::DeleteExpression(Box::new(crate::astdata::DeleteExpressionData {
                expression: self.id(expression),
            })),
        )
    }

    // Go: ast/ast_generated.go:4949 NewTypeOfExpression
    pub fn new_type_of_expression(&self, expression: Node) -> Node {
        self.new_node(
            SyntaxKind::TypeOfExpression,
            D::TypeOfExpression(Box::new(crate::astdata::TypeOfExpressionData {
                expression: self.id(expression),
            })),
        )
    }

    // Go: ast/ast_generated.go:4991 NewVoidExpression
    pub fn new_void_expression(&self, expression: Node) -> Node {
        self.new_node(
            SyntaxKind::VoidExpression,
            D::VoidExpression(Box::new(crate::astdata::VoidExpressionData {
                expression: self.id(expression),
            })),
        )
    }

    // Go: ast/ast_generated.go:5033 NewAwaitExpression
    pub fn new_await_expression(&self, expression: Node) -> Node {
        self.new_node(
            SyntaxKind::AwaitExpression,
            D::AwaitExpression(Box::new(crate::astdata::AwaitExpressionData {
                expression: self.id(expression),
            })),
        )
    }

    // Go: ast/ast_generated.go:3865 NewYieldExpression
    pub fn new_yield_expression(&self, asterisk_token: Node, expression: Node) -> Node {
        self.new_node(
            SyntaxKind::YieldExpression,
            D::YieldExpression(Box::new(crate::astdata::YieldExpressionData {
                asterisk_token: self.oid(asterisk_token),
                expression: self.oid(expression),
            })),
        )
    }

    // Go: ast/ast_generated.go:766 NewDecorator
    pub fn new_decorator(&self, expression: Node) -> Node {
        self.new_node(
            SyntaxKind::Decorator,
            D::Decorator(Box::new(crate::astdata::DecoratorData {
                expression: self.id(expression),
                facts: 0,
            })),
        )
    }

    // ── Statements ─────────────────────────────────────────────────────

    // Go: ast/ast_generated.go:1619 NewBlock
    pub fn new_block(&self, statements: NodeList, multi_line: bool) -> Node {
        self.new_node(
            SyntaxKind::Block,
            D::Block(Box::new(crate::astdata::BlockData {
                flow_node: None,
                locals: crate::astdata::SymbolTable,
                multi_line,
                next_container: None,
                statements: self.req_list(statements),
                facts: 0,
            })),
        )
    }

    // Go: ast/ast_generated.go:1664 NewVariableStatement
    pub fn new_variable_statement(&self, modifiers: ModifierList, declaration_list: Node) -> Node {
        self.new_node(
            SyntaxKind::VariableStatement,
            D::VariableStatement(Box::new(crate::astdata::VariableStatementData {
                declaration_list: self.id(declaration_list),
                flow_node: None,
                facts: 0,
                modifiers: self.mods(modifiers),
            })),
        )
    }

    // Go: ast/ast_generated.go:1709 NewVariableDeclaration
    pub fn new_variable_declaration(
        &self,
        name: Node,
        exclamation_token: Node,
        type_node: Node,
        initializer: Node,
    ) -> Node {
        self.new_node(
            SyntaxKind::VariableDeclaration,
            D::VariableDeclaration(Box::new(crate::astdata::VariableDeclarationData {
                exclamation_token: self.oid(exclamation_token),
                initializer: self.oid(initializer),
                local_symbol: None,
                symbol: None,
                type_: self.oid(type_node),
                facts: 0,
                name: self.id(name),
            })),
        )
    }

    // Go: ast/ast_generated.go:1758 NewVariableDeclarationList
    pub fn new_variable_declaration_list(&self, declarations: NodeList, flags: NodeFlags) -> Node {
        let node = self.new_node(
            SyntaxKind::VariableDeclarationList,
            D::VariableDeclarationList(Box::new(crate::astdata::VariableDeclarationListData {
                declarations: self.req_list(declarations),
                facts: 0,
            })),
        );
        set_node_flags(node, flags);
        node
    }

    // Go: ast/ast_generated.go:1574 NewExpressionStatement
    pub fn new_expression_statement(&self, expression: Node) -> Node {
        self.new_node(
            SyntaxKind::ExpressionStatement,
            D::ExpressionStatement(Box::new(crate::astdata::ExpressionStatementData {
                expression: self.id(expression),
                flow_node: None,
            })),
        )
    }

    // Go: ast/ast_generated.go:1149 NewReturnStatement
    pub fn new_return_statement(&self, expression: Node) -> Node {
        self.new_node(
            SyntaxKind::ReturnStatement,
            D::ReturnStatement(Box::new(crate::astdata::ReturnStatementData {
                expression: self.oid(expression),
                flow_node: None,
                facts: 0,
            })),
        )
    }

    // Go: ast/ast_generated.go:828 NewIfStatement
    pub fn new_if_statement(
        &self,
        expression: Node,
        then_statement: Node,
        else_statement: Node,
    ) -> Node {
        self.new_node(
            SyntaxKind::IfStatement,
            D::IfStatement(Box::new(crate::astdata::IfStatementData {
                else_statement: self.oid(else_statement),
                expression: self.id(expression),
                flow_node: None,
                then_statement: self.id(then_statement),
                facts: 0,
            })),
        )
    }

    // Go: ast/ast_generated.go:803 NewEmptyStatement
    pub fn new_empty_statement(&self) -> Node {
        self.new_node(
            SyntaxKind::EmptyStatement,
            D::EmptyStatement(Box::new(crate::astdata::EmptyStatementData {
                flow_node: None,
            })),
        )
    }

    // Go: ast/ast_generated.go:2475 NewNotEmittedStatement
    pub fn new_not_emitted_statement(&self) -> Node {
        self.new_node(
            SyntaxKind::NotEmittedStatement,
            D::NotEmittedStatement(Box::new(crate::astdata::NotEmittedStatementData {
                flow_node: None,
            })),
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
