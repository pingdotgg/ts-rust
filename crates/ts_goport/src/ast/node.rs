//! Go `*ast.Node` methods, `NodeList`, `ModifierList`, `core.TextRange`,
//! subtree facts, access kinds and the `ast.SourceFile` accessors from pinned
//! typescript-go `internal/ast/ast.go` (and `internal/core/text.go` for
//! `TextRange`).
//!
//! Go reads the AST without a context. So do we: every method reaches the
//! parsed astdata arena through the file registry (`ast/store.rs`
//! `go_file`). Nodes of a ported-parser file are
//! read from its node store (`ast/store.rs`), and factory nodes from the
//! synthetic arena (`ast/synthetic.rs`).
//!
//! PORT: Go methods that panic with "Unhandled case in Node.X" return nil,
//! an empty list or "" here (PORTING.md "AST"). They also cover every node
//! struct that has the Go field of the same name, because `fields.rs` does not
//! generate accessors for names that clash with Go `Node` methods.
//!
//! PORT: the factories (`New*`), `Clone`, `VisitEachChild`, the printer, the
//! `MutableNode` setters and the `AsFlow*` casts are out of scope.

use crate::ast::synthetic::{list_of, modifiers_of, with_data};
use crate::astdata::NodeData;
use crate::prelude::*;
use std::sync::atomic::Ordering;

// ──────────────────────────────────────────────────────────────────────
// Arena helpers
// ──────────────────────────────────────────────────────────────────────

/// The astdata id of `n`.
// PORT: `Node::node_id` in core.rs passes a usize to `NodeId::new(u32)`, so
// this file computes the id itself.
fn nid(n: Node) -> crate::astdata::NodeId {
    crate::astdata::NodeId::new(((n.0 & 0xffff_ffff) - 1) as u32)
}

/// The selector result for a list field (see `list_of!`): `req` for a
/// required list, `opt` for an optional one, `mods` for an optional modifier
/// list.
macro_rules! sel_field {
    (req, $x:expr) => {
        Some(AnyList::Nodes(&$x))
    };
    (opt, $x:expr) => {
        $x.as_ref().map(AnyList::Nodes)
    };
    (mods, $x:expr) => {
        $x.as_ref().map(AnyList::Modifiers)
    };
}

/// Go `nil` for an optional child.
fn opt(file: usize, id: Option<crate::astdata::NodeId>) -> Node {
    match id {
        Some(id) => Node::new(file, id),
        None => Node::NIL,
    }
}

/// A required child.
fn req(file: usize, id: crate::astdata::NodeId) -> Node {
    Node::new(file, id)
}

/// A list read from the data of node `n`, parsed or synthetic, as a
/// `NodeList` handle (`data_list`). Go `nil` when `None`. Code that walks
/// the lists of node data it reads in a scope (subtree facts, the child walk
/// of a synthetic node) reads them this way; accessors read them with
/// `list_of!` instead.
#[derive(Clone, Copy)]
struct DataList {
    list: NodeList,
}

impl DataList {
    /// A required list.
    fn req(n: Node, l: &crate::astdata::NodeList) -> Self {
        Self {
            list: data_list(n, *l),
        }
    }

    /// An optional list.
    fn opt(n: Node, l: &Option<crate::astdata::NodeList>) -> Self {
        Self {
            list: l.map_or(NodeList::NIL, |l| data_list(n, l)),
        }
    }

    /// The list of an optional modifier list.
    fn mods(n: Node, m: &Option<crate::astdata::ModifierList>) -> Self {
        Self {
            list: m.map_or(NodeList::NIL, |m| data_list(n, m.list)),
        }
    }

    /// Go `list != nil` (see `NodeList::is_nil`).
    fn is_some(self) -> bool {
        self.list.is_some()
    }

    /// The nodes of the list, in order, as `NodeList::nodes` gives them.
    /// Empty when nil.
    fn nodes(self) -> NodeSliceIter {
        self.list.nodes().iter()
    }
}

/// `ModifierList::modifier_flags` of modifier list field `m` of the data of
/// node `n` (store or synthetic).
fn in_place_modifier_flags(n: Node, m: &Option<crate::astdata::ModifierList>) -> ModifierFlags {
    m.map_or(ModifierFlags::NONE, |m| {
        data_modifiers(n, m).modifier_flags()
    })
}

/// True when data variant `$v` fits a node of kind `$k`. This mirrors
/// `crate::astdata::NodeData::matches_syntax_kind`: a variant named like a kind fits
/// that kind, and the other variants are listed here. A variant that is not
/// listed and is not named like a kind does not compile.
macro_rules! variant_has_kind {
    (Token, $k:expr) => {
        $k.is_token()
    };
    (KeywordExpression, $k:expr) => {
        $k.is_keyword_expression()
    };
    (KeywordTypeNode, $k:expr) => {
        $k.is_keyword_type()
    };
    (ForInOrOfStatement, $k:expr) => {
        matches!($k, SyntaxKind::ForInStatement | SyntaxKind::ForOfStatement)
    };
    (CaseOrDefaultClause, $k:expr) => {
        matches!($k, SyntaxKind::CaseClause | SyntaxKind::DefaultClause)
    };
    (BindingPattern, $k:expr) => {
        matches!(
            $k,
            SyntaxKind::ObjectBindingPattern | SyntaxKind::ArrayBindingPattern
        )
    };
    (JsDocParameterOrPropertyTag, $k:expr) => {
        matches!(
            $k,
            SyntaxKind::JsDocParameterTag | SyntaxKind::JsDocPropertyTag
        )
    };
    // These three are named like a kind but also fit a second kind.
    (TypeAliasDeclaration, $k:expr) => {
        matches!(
            $k,
            SyntaxKind::TypeAliasDeclaration | SyntaxKind::JsTypeAliasDeclaration
        )
    };
    (ImportDeclaration, $k:expr) => {
        matches!(
            $k,
            SyntaxKind::ImportDeclaration | SyntaxKind::JsImportDeclaration
        )
    };
    (JsxText, $k:expr) => {
        matches!($k, SyntaxKind::JsxText | SyntaxKind::JsxTextAllWhiteSpaces)
    };
    (ParameterDeclaration, $k:expr) => {
        $k == SyntaxKind::Parameter
    };
    (CallSignatureDeclaration, $k:expr) => {
        $k == SyntaxKind::CallSignature
    };
    (ConstructSignatureDeclaration, $k:expr) => {
        $k == SyntaxKind::ConstructSignature
    };
    (ConstructorDeclaration, $k:expr) => {
        $k == SyntaxKind::Constructor
    };
    (GetAccessorDeclaration, $k:expr) => {
        $k == SyntaxKind::GetAccessor
    };
    (SetAccessorDeclaration, $k:expr) => {
        $k == SyntaxKind::SetAccessor
    };
    (IndexSignatureDeclaration, $k:expr) => {
        $k == SyntaxKind::IndexSignature
    };
    (MethodSignatureDeclaration, $k:expr) => {
        $k == SyntaxKind::MethodSignature
    };
    (PropertySignatureDeclaration, $k:expr) => {
        $k == SyntaxKind::PropertySignature
    };
    (TypeParameterDeclaration, $k:expr) => {
        $k == SyntaxKind::TypeParameter
    };
    (TypeAssertion, $k:expr) => {
        $k == SyntaxKind::TypeAssertionExpression
    };
    (UnionTypeNode, $k:expr) => {
        $k == SyntaxKind::UnionType
    };
    (IntersectionTypeNode, $k:expr) => {
        $k == SyntaxKind::IntersectionType
    };
    (ConditionalTypeNode, $k:expr) => {
        $k == SyntaxKind::ConditionalType
    };
    (TypeOperatorNode, $k:expr) => {
        $k == SyntaxKind::TypeOperator
    };
    (InferTypeNode, $k:expr) => {
        $k == SyntaxKind::InferType
    };
    (ArrayTypeNode, $k:expr) => {
        $k == SyntaxKind::ArrayType
    };
    (IndexedAccessTypeNode, $k:expr) => {
        $k == SyntaxKind::IndexedAccessType
    };
    (TypeReferenceNode, $k:expr) => {
        $k == SyntaxKind::TypeReference
    };
    (LiteralTypeNode, $k:expr) => {
        $k == SyntaxKind::LiteralType
    };
    (ThisTypeNode, $k:expr) => {
        $k == SyntaxKind::ThisType
    };
    (TypePredicateNode, $k:expr) => {
        $k == SyntaxKind::TypePredicate
    };
    (TypeQueryNode, $k:expr) => {
        $k == SyntaxKind::TypeQuery
    };
    (MappedTypeNode, $k:expr) => {
        $k == SyntaxKind::MappedType
    };
    (TypeLiteralNode, $k:expr) => {
        $k == SyntaxKind::TypeLiteral
    };
    (TupleTypeNode, $k:expr) => {
        $k == SyntaxKind::TupleType
    };
    (OptionalTypeNode, $k:expr) => {
        $k == SyntaxKind::OptionalType
    };
    (RestTypeNode, $k:expr) => {
        $k == SyntaxKind::RestType
    };
    (ParenthesizedTypeNode, $k:expr) => {
        $k == SyntaxKind::ParenthesizedType
    };
    (FunctionTypeNode, $k:expr) => {
        $k == SyntaxKind::FunctionType
    };
    (ConstructorTypeNode, $k:expr) => {
        $k == SyntaxKind::ConstructorType
    };
    (TemplateLiteralTypeNode, $k:expr) => {
        $k == SyntaxKind::TemplateLiteralType
    };
    (ImportTypeNode, $k:expr) => {
        $k == SyntaxKind::ImportType
    };
    ($v:ident, $k:expr) => {
        $k == SyntaxKind::$v
    };
}

/// True when `n` is a published store node whose kind fits none of the listed
/// data variants. Such a node cannot hold the field, so the accessor gives
/// its default without loading the node data.
// Store data always fits the header kind (`alloc_store_node`). `Unknown` is
// also the kind of nil and alias slots, so it takes the data path, which
// keeps its panics.
macro_rules! kind_lacks_data {
    ($n:expr, [$($v:ident),+]) => {
        match frozen_store_kind($n) {
            Some(kind) => kind != SyntaxKind::Unknown $(&& !variant_has_kind!($v, kind))+,
            None => false,
        }
    };
}

/// The data match of `by_data!`: `$data` is the loaded data of a node of
/// file `$file_in`. The arms are those of `by_data!`.
macro_rules! match_data {
    ($file_in:expr, $data:expr, $def:expr, $([$($v:ident),+ $(,)?] => |$file:ident, $d:ident| $e:expr),+ $(,)?) => {{
        let file: usize = $file_in;
        match $data {
            $($(NodeData::$v($d) => {
                let $file = file;
                $e
            })+)+
            _ => $def,
        }
    }};
}

/// True when `$k`, the kind of a published store node, fits none of the
/// data variants `$v`: `kind_lacks_data!` on a kind that the caller read.
macro_rules! kind_lacks_variants {
    ($k:expr, [$($v:ident),+]) => {{
        let kind: SyntaxKind = $k;
        kind != SyntaxKind::Unknown $(&& !variant_has_kind!($v, kind))+
    }};
}

/// `read_ast_node_miss` with the nil check of `with_data!`: the node data
/// read of a node that is not a published store node with node data
/// (`FrozenNode::data`).
#[inline]
pub(crate) fn read_data_miss<R>(n: Node, read: impl FnOnce(NodeData<'_>) -> R) -> R {
    assert!(n.is_some(), "nil node dereference");
    crate::ast::synthetic::read_ast_node_miss(n, read)
}

/// Go field read by node data variant. Each arm lists `NodeData` variants,
/// binds the variant data to `$d` and the node's file to `$file`, and
/// evaluates `$e`. Other variants give `$def`.
///
/// PERF: a published store node whose kind fits no listed variant gives `$def`
/// from its node record (`kind_lacks_data!`), without the pointer
/// chase to its astdata node and data.
// PERF: noderead2. The kind test and the node data read share one registry
// lookup (`FrozenNode`). They were two (`frozen_store_kind`, then
// `with_data!`), and the `OnceLock` acquire load of each stops LLVM from
// merging them. Go reads `n.Kind` and `n.data` from one struct.
macro_rules! by_data {
    ($n:expr, $def:expr, $([$($v:ident),+ $(,)?] => |$file:ident, $d:ident| $e:expr),+ $(,)?) => {{
        let node: Node = $n;
        let frozen = frozen_store_node(node);
        match frozen {
            Some(f) if kind_lacks_variants!(f.kind(), [$($($v),+),+]) => {
                debug_assert!(!with_data!(node, |d| matches!(d, $($(NodeData::$v(_))|+)|+)));
                $def
            }
            _ => match frozen.and_then(FrozenNode::data) {
                Some(d) => match_data!(node.file_index(), d, $def, $([$($v),+] => |$file, $d| $e),+),
                None => read_data_miss(node, |d| {
                    match_data!(node.file_index(), d, $def, $([$($v),+] => |$file, $d| $e),+)
                }),
            },
        }
    }};
}

/// `by_data!` for arms that read child nodes: each arm binds `$ids`, the
/// `ChildIds` of the node, in place of its file. A published store node
/// gives its kind, node data and child ids from one registry lookup.
// PERF: noderead2. `by_data!` and then `Node::new` for the child were two
// registry lookups (three before `by_data!` shared one).
macro_rules! by_child_ids {
    ($n:expr, $def:expr, $([$($v:ident),+ $(,)?] => |$ids:ident, $d:ident| $e:expr),+ $(,)?) => {{
        let node: Node = $n;
        let frozen = frozen_store_node(node);
        match frozen {
            Some(f) if kind_lacks_variants!(f.kind(), [$($($v),+),+]) => {
                debug_assert!(!with_data!(node, |d| matches!(d, $($(NodeData::$v(_))|+)|+)));
                $def
            }
            _ => match frozen.and_then(|f| Some((f.data()?, f.ids()))) {
                Some((d, ids)) => {
                    match_ids!(ChildIds::Frozen(ids), d, $def, $([$($v),+] => |$ids, $d| $e),+)
                }
                None => read_data_miss(node, |d| {
                    let ids = ChildIds::File(node.file_index());
                    match_ids!(ids, d, $def, $([$($v),+] => |$ids, $d| $e),+)
                }),
            },
        }
    }};
}

/// The data match of `by_child_ids!`: `match_data!` with the `ChildIds`
/// `$ids_in` in place of the file.
macro_rules! match_ids {
    ($ids_in:expr, $data:expr, $def:expr, $([$($v:ident),+ $(,)?] => |$ids:ident, $d:ident| $e:expr),+ $(,)?) => {{
        let ids: ChildIds = $ids_in;
        match $data {
            $($(NodeData::$v($d) => {
                let $ids = ids;
                $e
            })+)+
            _ => $def,
        }
    }};
}

/// Go list field read by node data variant: each arm lists `NodeData`
/// variants and the field (`req`, `opt` or `mods`, see `sel_field!`).
/// `$of` is `list_of` or `modifiers_of`. Other variants give `$def`. The
/// `kind_lacks_data!` shortcut is the same as in `by_data!`.
// PERF: noderead2. One registry lookup for the kind test, the node data and
// the list table, as in `by_data!`.
macro_rules! list_by_data {
    ($n:expr, $of:ident, $def:expr, $([$($v:ident),+ $(,)?] => $how:ident $field:ident),+ $(,)?) => {{
        let node: Node = $n;
        let frozen = frozen_store_node(node);
        match frozen {
            Some(f) if kind_lacks_variants!(f.kind(), [$($($v),+),+]) => $def,
            _ => frozen_list_of!($of, node, frozen, |d| match d {
                $($(NodeData::$v(d) => Some(sel_field!($how, d.$field)),)+)+
                _ => None,
            })
            .unwrap_or($def),
        }
    }};
}

/// `list_of!` (or `modifiers_of!`, `$of`) on node `$n`, whose
/// `frozen_store_node` the caller already read as `$frozen`: the node data
/// and the list table of a published store node come from that lookup. Any
/// other node reads through `$of!`.
macro_rules! frozen_list_of {
    (list_of, $n:expr, $frozen:expr, |$d:ident| $body:expr) => {
        frozen_list_of!(@read list_of, $n, $frozen, |$d| $body, NodeList, nodes)
    };
    (modifiers_of, $n:expr, $frozen:expr, |$d:ident| $body:expr) => {
        frozen_list_of!(@read modifiers_of, $n, $frozen, |$d| $body, ModifierList, modifiers)
    };
    (@read $of:ident, $n:expr, $frozen:expr, |$d:ident| $body:expr, $list:ident, $as:ident) => {{
        let n__: Node = $n;
        let frozen__: Option<FrozenNode> = $frozen;
        match frozen__.and_then(|f| Some((f.data()?, f.lists()))) {
            Some((data__, lists__)) => {
                let $d: NodeData<'static> = data__;
                let found: Option<Option<crate::ast::synthetic::AnyList<'static>>> = $body;
                found.map(|l| {
                    $list::from_table(
                        n__.file_index(),
                        lists__,
                        l.map(crate::ast::synthetic::AnyList::$as),
                    )
                })
            }
            None => $of!(n__, |$d| $body),
        }
    }};
}

/// How the child ids in the node data of one node become handles
/// (`Node::new(file, id)`): from the registry block that the data read
/// already looked up (`FrozenIds`), or through `Node::new` for a node that
/// is not a published store node with node data.
#[derive(Clone, Copy)]
pub(crate) enum ChildIds {
    Frozen(FrozenIds),
    File(usize),
}

impl ChildIds {
    /// `Node::new(file, id)`.
    #[inline(always)]
    pub(crate) fn node(self, id: crate::astdata::NodeId) -> Node {
        match self {
            ChildIds::Frozen(ids) => {
                let n = ids.node(id);
                #[cfg(debug_assertions)]
                {
                    let (FrozenIds::Direct { base } | FrozenIds::Records { base, .. }) = ids;
                    debug_assert_eq!(n, Node::new((base >> 32) as usize, id), "ChildIds");
                }
                n
            }
            ChildIds::File(file) => Node::new(file, id),
        }
    }

    /// Go `nil` for an optional child, else `node`.
    #[inline(always)]
    pub(crate) fn opt(self, id: Option<crate::astdata::NodeId>) -> Node {
        match id {
            Some(id) => self.node(id),
            None => Node::NIL,
        }
    }
}

/// The node data and the child ids of published store node `n`, from one
/// registry lookup, or `None` for any other node (read it with the
/// accessors). For a hot site that reads a few fields of one node.
#[inline]
#[must_use]
pub fn frozen_data_ids(n: Node) -> Option<(NodeData<'static>, FrozenIds)> {
    let f = frozen_store_node(n)?;
    Some((f.data()?, f.ids()))
}

/// `with_data!` that also binds `$ids`, the `ChildIds` of node `$n`: a
/// published store node gives its node data and child ids from one registry
/// lookup. The Go field accessors of `fields.rs` read through it.
// PERF: noderead2. `with_data!` and then `Node::new` for the child were
// two registry lookups for a Go `n.AsX().Field` read.
macro_rules! with_child_ids {
    ($n:expr, |$d:ident, $ids:ident| $body:expr) => {{
        let n__: $crate::core::Node = $n;
        match $crate::ast::frozen_store_node(n__).and_then(|f| Some((f.data()?, f.ids()))) {
            Some((data__, ids__)) => {
                let $d: $crate::astdata::NodeData<'_> = data__;
                let $ids = $crate::ast::ChildIds::Frozen(ids__);
                $body
            }
            None => $crate::ast::read_data_miss(n__, |data__| {
                let $d: $crate::astdata::NodeData<'_> = data__;
                let $ids = $crate::ast::ChildIds::File(n__.file_index());
                $body
            }),
        }
    }};
}
pub(crate) use with_child_ids;

/// True when `d` is the data of parsed node `n` (the `_in` debug checks).
fn is_data_of(d: NodeData<'_>, n: Node) -> bool {
    static_ast_node(n).is_some_and(|data| d.same(data))
}

/// Defines one Go field accessor as two `Node` methods from one arm list:
/// `$name(self)` reads the node like `by_data!`, and `$name_in(self, d)`
/// reads `d`, the data of the same node, which the caller already loaded
/// with `parsed_node_data`. `$n` names the node in the arms. The arms are
/// those of `by_data!`. With `d` `None` (a node that a freeable parse owns,
/// lsshells M3c, `LoadedData`) `$name_in` is `$name`.
///
/// PERF: query Q7-3. Each `$name` call repeats the `FROZEN` lookup, the
/// kind-table test and the data load. A caller that reads several fields of
/// one node (the binder) loads the data once and calls the `_in` methods.
/// Both methods give the same result.
macro_rules! data_accessor {
    (
        $(#[$attr:meta])*
        $vis:vis fn $name:ident, $name_in:ident($n:ident) -> $ty:ty {
            $def:expr,
            $([$($v:ident),+ $(,)?] => |$file:ident, $d:ident| $e:expr),+ $(,)?
        }
    ) => {
        $(#[$attr])*
        #[must_use]
        $vis fn $name(self) -> $ty {
            let $n: Node = self;
            by_data!($n, $def, $([$($v),+] => |$file, $d| $e),+)
        }

        #[doc = concat!("`Node::", stringify!($name), "` on `d`, the data of this node that the caller already loaded with `parsed_node_data` (see `data_accessor!`).")]
        #[must_use]
        $vis fn $name_in(self, d: LoadedData) -> $ty {
            let Some(d) = d else {
                return self.$name();
            };
            let $n: Node = self;
            debug_assert!(is_data_of(d, $n), "data of another node");
            match_data!($n.file_index(), d, $def, $([$($v),+] => |$file, $d| $e),+)
        }
    };
}

/// Expands `$mac! { $($args)* [variants] }` with the `NodeData` variants
/// that hold a Go `Modifiers()` list. `Node::modifiers` and the U1 (b) store
/// column (`store_node_modifier_list`) both take the list from here, so they
/// agree by construction.
macro_rules! modifiers_variants {
    ($mac:ident! { $($args:tt)* }) => {
        $mac! {
            $($args)*
            [
                ArrowFunction,
                BinaryExpression,
                ClassDeclaration,
                ClassExpression,
                ClassStaticBlockDeclaration,
                ConstructorDeclaration,
                ConstructorTypeNode,
                EnumDeclaration,
                EnumMember,
                ExportAssignment,
                ExportDeclaration,
                FunctionDeclaration,
                FunctionExpression,
                FunctionTypeNode,
                GetAccessorDeclaration,
                SetAccessorDeclaration,
                ImportDeclaration,
                ImportEqualsDeclaration,
                IndexSignatureDeclaration,
                InterfaceDeclaration,
                MethodDeclaration,
                MethodSignatureDeclaration,
                MissingDeclaration,
                ModuleDeclaration,
                NamespaceExportDeclaration,
                ParameterDeclaration,
                PropertyAssignment,
                PropertyDeclaration,
                PropertySignatureDeclaration,
                ShorthandPropertyAssignment,
                TypeAliasDeclaration,
                TypeParameterDeclaration,
                VariableStatement
            ]
        }
    };
}

/// `Node::modifiers` and `Node::modifiers_in` over the `modifiers_variants!`
/// list, as `data_accessor!` does for other fields. The list is a handle, so
/// `modifiers` reads it with `modifiers_of!` (`list_by_data!`), not
/// `by_data!`.
macro_rules! modifiers_accessor {
    ([$($v:ident),+ $(,)?]) => {
        #[must_use]
        pub fn modifiers(self) -> ModifierList {
            list_by_data!(self, modifiers_of, ModifierList::NIL, [$($v),+] => mods modifiers)
        }

        /// `Node::modifiers` on `d`, the data of this node that the caller
        /// already loaded with `parsed_node_data` (see `data_accessor!`).
        #[must_use]
        pub fn modifiers_in(self, d: LoadedData) -> ModifierList {
            let Some(d) = d else {
                return self.modifiers();
            };
            debug_assert!(is_data_of(d, self), "data of another node");
            match_data!(
                self.file_index(),
                d,
                ModifierList::NIL,
                [$($v),+] => |_f, d| d.modifiers.map_or(ModifierList::NIL, |m| data_modifiers(self, m))
            )
        }
    };
}

/// `store_node_modifier_list` over the `modifiers_variants!` list.
macro_rules! store_node_modifier_list_fn {
    ([$($v:ident),+ $(,)?]) => {
        /// U1 (b): the own modifier list field of `data`, the data of a store
        /// node of kind `kind`, or `None` when the node has none. The kind
        /// test comes first, so most nodes are not loaded (the `NodeKids`
        /// modifier word).
        #[inline]
        pub(crate) fn store_node_modifier_list(
            kind: SyntaxKind,
            data: NodeData<'_>,
        ) -> Option<crate::astdata::ModifierList> {
            let mut has_list = false;
            $(has_list |= variant_has_kind!($v, kind);)+
            if !has_list {
                return None;
            }
            match data {
                $(NodeData::$v(d) => d.modifiers,)+
                _ => None,
            }
        }
    };
}

modifiers_variants!(store_node_modifier_list_fn! {});

/// U1 (b): `ModifierList::modifier_flags` of the own modifier list of
/// `data`, the data of a store node of kind `kind`, as bits: the value
/// `Node::modifiers().modifier_flags()` gives for it (store lists hold their
/// Go flags). `flags` gives the flags of a list field (from the list table
/// of the store). 0 when the node has no list.
pub(crate) fn store_node_modifier_bits(
    kind: SyntaxKind,
    data: NodeData<'_>,
    flags: impl FnOnce(crate::astdata::ModifierList) -> u32,
) -> u32 {
    store_node_modifier_list(kind, data).map_or(0, flags)
}

// U4 (CH6, bind A): the plain field arms of Go `Name()`, `Expression()`,
// `PostfixToken()` and the own `QuestionToken` field. Each macro takes
// `$mac!($($pre)*)` and calls `$mac! { $($pre)* arms }`: it appends its arms
// to the tokens `$pre`, which end with a comma. The braces let the call
// expand to items (in `impl Node`) as well as to an expression. Each arm
// lists `NodeData` variants and reads the field with `$opt` (a field that
// can be nil) or `$req` (a required field). The accessors and the store
// column (`store_node_children`) both take the arms from here, so they
// agree by construction. The special arms (QualifiedName,
// CaseOrDefaultClause) stay with the accessors; the column marks those
// kinds unknown.

/// The plain arms of Go `Name()` (see above).
macro_rules! name_arms {
    ($mac:ident!($($pre:tt)*), $opt:ident, $req:ident) => {
        $mac! {
            $($pre)*
            [
                BindingElement,
                ClassDeclaration,
                ClassExpression,
                FunctionDeclaration,
                FunctionExpression,
                ImportClause,
                JsDocCallbackTag,
                JsDocLink,
                JsDocLinkCode,
                JsDocLinkPlain,
                JsDocTypedefTag,
            ] => |f, d| $opt(f, d.name),
            [
                EnumDeclaration,
                EnumMember,
                ExportSpecifier,
                GetAccessorDeclaration,
                SetAccessorDeclaration,
                ImportAttribute,
                ImportEqualsDeclaration,
                ImportSpecifier,
                InterfaceDeclaration,
                JsDocNameReference,
                JsDocParameterOrPropertyTag,
                JsxAttribute,
                JsxNamespacedName,
                MetaProperty,
                MethodDeclaration,
                MethodSignatureDeclaration,
                ModuleDeclaration,
                NamedTupleMember,
                NamespaceExport,
                NamespaceExportDeclaration,
                NamespaceImport,
                ParameterDeclaration,
                PropertyAccessExpression,
                PropertyAssignment,
                PropertyDeclaration,
                PropertySignatureDeclaration,
                ShorthandPropertyAssignment,
                TypeAliasDeclaration,
                TypeParameterDeclaration,
                VariableDeclaration,
            ] => |f, d| $req(f, d.name),
        }
    };
}

/// The plain arms of Go `Expression()` (see above).
// PORT: also covers TypeParameterDeclaration and SyntheticReferenceExpression,
// whose Go `Expression` fields have no generated accessor.
macro_rules! expression_arms {
    ($mac:ident!($($pre:tt)*), $opt:ident, $req:ident) => {
        $mac! {
            $($pre)*
            [
                PropertyAccessExpression,
                ElementAccessExpression,
                ParenthesizedExpression,
                CallExpression,
                NewExpression,
                ExpressionWithTypeArguments,
                ComputedPropertyName,
                NonNullExpression,
                TypeAssertion,
                AsExpression,
                SatisfiesExpression,
                TypeOfExpression,
                SpreadAssignment,
                SpreadElement,
                TemplateSpan,
                DeleteExpression,
                VoidExpression,
                AwaitExpression,
                PartiallyEmittedExpression,
                IfStatement,
                DoStatement,
                WhileStatement,
                WithStatement,
                ForInOrOfStatement,
                SwitchStatement,
                ExpressionStatement,
                ThrowStatement,
                ExternalModuleReference,
                ExportAssignment,
                Decorator,
                JsxSpreadAttribute,
                SyntheticReferenceExpression,
            ] => |f, d| $req(f, d.expression),
            [
                YieldExpression,
                ReturnStatement,
                JsxExpression,
                TypeParameterDeclaration,
            ] => |f, d| $opt(f, d.expression),
        }
    };
}

/// The arms of Go `PostfixToken()` (see above).
macro_rules! postfix_arms {
    ($mac:ident!($($pre:tt)*), $opt:ident) => {
        $mac! {
            $($pre)*
            [
                MethodDeclaration,
                ShorthandPropertyAssignment,
                MethodSignatureDeclaration,
                PropertySignatureDeclaration,
                PropertyAssignment,
                PropertyDeclaration,
                EnumMember,
                GetAccessorDeclaration,
                SetAccessorDeclaration,
            ] => |f, d| $opt(f, d.postfix_token),
        }
    };
}

/// The arms of the node's own Go `QuestionToken` field, the first step of
/// Go `QuestionToken()` (see above).
macro_rules! question_arms {
    ($mac:ident!($($pre:tt)*), $opt:ident, $req:ident) => {
        $mac! {
            $($pre)*
            [
                ParameterDeclaration,
                MappedTypeNode,
                NamedTupleMember,
            ] => |f, d| $opt(f, d.question_token),
            [ConditionalExpression] => |f, d| $req(f, d.question_token),
        }
    };
}

// C2: the arms of Go `Type()`, `Initializer()` and `TypeArgumentList()`,
// shared by the accessors and the `typed` field of the store column
// (`store_node_children`) in the same way as the U4 arms above. Every arm
// is a plain field read.

/// The arms of Go `Type()` (see above).
// PORT: also covers JsDocVariadicType, whose Go `Type` field has no
// generated accessor.
macro_rules! type_arms {
    ($mac:ident!($($pre:tt)*), $opt:ident, $req:ident) => {
        $mac! {
            $($pre)*
            [JsDocParameterOrPropertyTag] => |f, d| $opt(f, d.type_expression),
            [IndexSignatureDeclaration] => |f, d| $req(f, d.type_),
            [
                VariableDeclaration,
                ParameterDeclaration,
                PropertyDeclaration,
                PropertyAssignment,
                ShorthandPropertyAssignment,
                TypePredicateNode,
                MappedTypeNode,
                ExportAssignment,
                BinaryExpression,
                ArrowFunction,
                CallSignatureDeclaration,
                ConstructSignatureDeclaration,
                ConstructorDeclaration,
                FunctionDeclaration,
                FunctionExpression,
                GetAccessorDeclaration,
                SetAccessorDeclaration,
                JsDocSignature,
                MethodDeclaration,
                MethodSignatureDeclaration,
                FunctionTypeNode,
                ConstructorTypeNode,
            ] => |f, d| $opt(f, d.type_),
            [
                PropertySignatureDeclaration,
                ParenthesizedTypeNode,
                TypeOperatorNode,
                TypeAssertion,
                AsExpression,
                SatisfiesExpression,
                TypeAliasDeclaration,
                NamedTupleMember,
                OptionalTypeNode,
                RestTypeNode,
                TemplateLiteralTypeSpan,
                JsDocTypeExpression,
                JsDocNullableType,
                JsDocNonNullableType,
                JsDocOptionalType,
                JsDocVariadicType,
            ] => |f, d| $req(f, d.type_),
        }
    };
}

/// The arms of Go `Initializer()` (see above).
macro_rules! initializer_arms {
    ($mac:ident!($($pre:tt)*), $opt:ident, $req:ident) => {
        $mac! {
            $($pre)*
            [
                VariableDeclaration,
                ParameterDeclaration,
                BindingElement,
                PropertyDeclaration,
                EnumMember,
                ForStatement,
                JsxAttribute,
            ] => |f, d| $opt(f, d.initializer),
            [
                PropertySignatureDeclaration,
                PropertyAssignment,
                ForInOrOfStatement,
            ] => |f, d| $req(f, d.initializer),
        }
    };
}

/// The arms of Go `TypeArgumentList()` (see above). `$list` reads the
/// optional list field.
macro_rules! type_arguments_arms {
    ($mac:ident!($($pre:tt)*), $list:ident) => {
        $mac! {
            $($pre)*
            [
                CallExpression,
                NewExpression,
                TaggedTemplateExpression,
                TypeReferenceNode,
                ExpressionWithTypeArguments,
                ImportTypeNode,
                TypeQueryNode,
                JsxOpeningElement,
                JsxSelfClosingElement,
            ] => |f, d| $list(f, &d.type_arguments),
        }
    };
}

/// `Node::initializer_data` and `Node::initializer_data_in`: Go
/// `Initializer()` from the node data, over the `initializer_arms!` arms.
macro_rules! initializer_data_accessor {
    ($($arms:tt)*) => {
        data_accessor! {
            fn initializer_data, initializer_data_in(n) -> Node {
                Node::NIL,
                $($arms)*
            }
        }
    };
}

/// One `match_data!` over the expression, postfix and own question arms for
/// the `other` entry of `store_node_children`: `Some((tag, id))`, or `None`
/// for unknown. The three field sets have disjoint variants, so one match
/// covers them (a variant in two sets would be an unreachable pattern). The
/// `@` steps collect the arms of each list macro in turn.
macro_rules! other_column_match {
    (@expression $data:expr, $expr_opt:ident, $expr_req:ident, $postfix:ident, $q_opt:ident, $q_req:ident) => {
        expression_arms!(
            other_column_match!(@postfix $data, $postfix, $q_opt, $q_req,),
            $expr_opt,
            $expr_req
        )
    };
    (@postfix $data:expr, $postfix:ident, $q_opt:ident, $q_req:ident, $($arms:tt)*) => {
        postfix_arms!(other_column_match!(@question $data, $q_opt, $q_req, $($arms)*), $postfix)
    };
    (@question $data:expr, $q_opt:ident, $q_req:ident, $($arms:tt)*) => {
        question_arms!(other_column_match!(@match $data, $($arms)*), $q_opt, $q_req)
    };
    (@match $data:expr, $($arms:tt)*) => {
        match_data!(
            0,
            $data,
            // A kind with none of the fields: every read gives nil.
            Some((SlotChildren::TAG_EXPRESSION, 0)),
            // The special arms of `Node::expression`.
            [CaseOrDefaultClause, QualifiedName] => |_f, _d| None,
            $($arms)*
        )
    };
}

/// `Node::name_data` and `Node::name_data_in`: Go `Name()` from the node
/// data, over the `name_arms!` arms. `name_data` is not inlined, so
/// `Node::name` stays small enough to inline.
// PERF (followups57): noderead2 made `by_data!` larger, and the R207 PGO
// builds inlined `name_data` into `Node::name` (1,067 bytes), which then no
// longer inlined into its callers. The Effect plugin calls it often: effect
// st instructions:u +0.29%, 56M in `Node::name` alone. The R206 builds
// called `name_data` out of line too. A published store node takes the
// store column and never calls it.
macro_rules! name_data_accessor {
    ($($arms:tt)*) => {
        data_accessor! {
            #[inline(never)]
            fn name_data, name_data_in(n) -> Node {
                Node::NIL,
                // PORT: a QualifiedName that Go parses as a PropertyAccessExpression.
                [QualifiedName] => |f, d| if n.kind() == SyntaxKind::PropertyAccessExpression {
                    req(f, d.right)
                } else {
                    Node::NIL
                },
                $($arms)*
            }
        }
    };
}

/// `Node::postfix_token_data` and `Node::postfix_token_data_in`: Go
/// `PostfixToken()` from the node data, over the `postfix_arms!` arms.
macro_rules! postfix_token_data_accessor {
    ($($arms:tt)*) => {
        data_accessor! {
            fn postfix_token_data, postfix_token_data_in(n) -> Node {
                Node::NIL,
                $($arms)*
            }
        }
    };
}

/// `Node::own_question_token` and `Node::own_question_token_in` over the
/// `question_arms!` arms: `Some` of the node's own question token field,
/// `None` for kinds without that field (the first step of Go
/// `QuestionToken`).
macro_rules! own_question_token_accessor {
    ($($arms:tt)*) => {
        data_accessor! {
            fn own_question_token, own_question_token_in(n) -> Option<Node> {
                None,
                $($arms)*
            }
        }
    };
}

/// `Some(opt(file, id))`, for `own_question_token`.
fn some_opt(file: usize, id: Option<crate::astdata::NodeId>) -> Option<Node> {
    Some(opt(file, id))
}

/// `Some(req(file, id))`, for `own_question_token`.
fn some_req(file: usize, id: crate::astdata::NodeId) -> Option<Node> {
    Some(req(file, id))
}

/// The `list_of!` selector result of an optional type argument list field,
/// for `Node::type_argument_list` over the `type_arguments_arms!` arms.
fn sel_type_arguments(
    _file: usize,
    l: &Option<crate::astdata::NodeList>,
) -> Option<Option<AnyList<'_>>> {
    Some(l.as_ref().map(AnyList::Nodes))
}

/// A `SlotChildren` id of an optional child field: 0 (the nil slot) for Go
/// nil.
fn column_opt_id(id: Option<crate::astdata::NodeId>) -> u32 {
    id.map_or(0, column_req_id)
}

/// A `SlotChildren` id of a required child field.
fn column_req_id(id: crate::astdata::NodeId) -> u32 {
    // Child ids are `u32` slot indexes.
    id.index() as u32
}

/// U4 (CH6, bind A): the `SlotChildren` entry of a store node of kind `kind`
/// with astdata node `node`, from the arm lists of `Node::name`,
/// `Node::expression`, `Node::postfix_token` and `Node::question_token`,
/// and (C2) of `Node::type_`, `Node::initializer`, `Node::type_name` and
/// `Node::type_argument_list`. Store data always fits the header kind
/// (`alloc_store_node`). A kind with a special arm (QualifiedName,
/// CaseOrDefaultClause) gets an unknown field, whose reads take the node
/// data.
// PERF: called when the slot is made, while its data is hot (a few data
// matches), so no pass over the node data is needed later.
pub(crate) fn store_node_children(kind: SyntaxKind, data: NodeData<'_>) -> SlotChildren {
    debug_assert!(
        data.matches_syntax_kind(kind),
        "{kind:?} does not fit its NodeData"
    );
    // PERF: about a third of the slots are identifiers, which have none of
    // the fields. The early exit skips the data matches.
    if kind == SyntaxKind::Identifier {
        let none = SlotChildren::new(Some(0), Some((SlotChildren::TAG_EXPRESSION, 0)))
            .with_typed(Some((SlotChildren::TYPED_TYPE, 0)), true);
        debug_assert_eq!(none, store_node_children_from_data(data));
        return none;
    }
    store_node_children_from_data(data)
}

/// `store_node_children` for node data `data`.
fn store_node_children_from_data(data: NodeData<'_>) -> SlotChildren {
    let id_opt = |_: usize, id: Option<crate::astdata::NodeId>| Some(column_opt_id(id));
    let id_req = |_: usize, id: crate::astdata::NodeId| Some(column_req_id(id));
    let name = name_arms!(
        match_data!(0, data, Some(0), [QualifiedName] => |_f, _d| None,),
        id_opt,
        id_req
    );
    let tagged_opt = |tag: u32| {
        move |_: usize, id: Option<crate::astdata::NodeId>| Some((tag, column_opt_id(id)))
    };
    let tagged_req =
        |tag: u32| move |_: usize, id: crate::astdata::NodeId| Some((tag, column_req_id(id)));
    let (expr_opt, expr_req) = (
        tagged_opt(SlotChildren::TAG_EXPRESSION),
        tagged_req(SlotChildren::TAG_EXPRESSION),
    );
    let postfix = tagged_opt(SlotChildren::TAG_POSTFIX);
    let (q_opt, q_req) = (
        tagged_opt(SlotChildren::TAG_QUESTION),
        tagged_req(SlotChildren::TAG_QUESTION),
    );
    let other = other_column_match!(@expression data, expr_opt, expr_req, postfix, q_opt, q_req);
    // C2: `None` for a kind without the field, `Some(0)` for nil. A kind can
    // have both a type and an initializer, so each field has its own match.
    // The column holds the one that is set; only a node where both are set
    // keeps its initializer in the data (`TYPED_TYPE_WITH_INITIALIZER`).
    let type_id = type_arms!(match_data!(0, data, None,), id_opt, id_req);
    let initializer_id = initializer_arms!(match_data!(0, data, None,), id_opt, id_req);
    let typed = match (type_id, initializer_id) {
        (Some(ty), Some(init)) if ty != 0 && init != 0 => {
            (SlotChildren::TYPED_TYPE_WITH_INITIALIZER, ty)
        }
        (Some(ty), _) if ty != 0 => (SlotChildren::TYPED_TYPE, ty),
        (_, Some(init)) => (SlotChildren::TYPED_INITIALIZER, init),
        (Some(_), None) => (SlotChildren::TYPED_TYPE, 0),
        (None, None) => match data {
            // Go `AsTypeReference().TypeName` (`Node::type_name`).
            NodeData::TypeReferenceNode(d) => {
                (SlotChildren::TYPED_TYPE_NAME, column_req_id(d.type_name))
            }
            _ => (SlotChildren::TYPED_TYPE, 0),
        },
    };
    // Only a missing list gives `NodeList::NIL` itself.
    let list_is_none = |_: usize, list: &Option<crate::astdata::NodeList>| list.is_none();
    let no_type_arguments = type_arguments_arms!(match_data!(0, data, true,), list_is_none);
    SlotChildren::new(name, other).with_typed(Some(typed), no_type_arguments)
}

/// Expands `$mac! { $($args)* [variants] }` with the `NodeData` variants of
/// the Go node types that embed `LocalsContainerBase`. `is_locals_container`
/// and the U1 (d) kind test of `Node::locals` both take the list from here,
/// so they agree by construction.
macro_rules! locals_container_variants {
    ($mac:ident! { $($args:tt)* }) => {
        $mac! {
            $($args)*
            [
                ArrowFunction,
                Block,
                CallSignatureDeclaration,
                CaseBlock,
                CatchClause,
                ClassDeclaration,
                ClassExpression,
                ClassStaticBlockDeclaration,
                ConditionalTypeNode,
                ConstructSignatureDeclaration,
                ConstructorDeclaration,
                ConstructorTypeNode,
                ForInOrOfStatement,
                ForStatement,
                FunctionDeclaration,
                FunctionExpression,
                FunctionTypeNode,
                GetAccessorDeclaration,
                IndexSignatureDeclaration,
                JsDocSignature,
                MappedTypeNode,
                MethodDeclaration,
                MethodSignatureDeclaration,
                ModuleDeclaration,
                SetAccessorDeclaration,
                SourceFile,
                TypeAliasDeclaration
            ]
        }
    };
}

/// True when `$d` (a `NodeData<'_>`) is one of the listed variants.
macro_rules! data_is_variant {
    ($d:expr, [$($v:ident),+ $(,)?]) => {
        matches!($d, $(NodeData::$v(_))|+)
    };
}

/// U4 (CH7): every bit the binder adds to or removes from Go `node.Flags`
/// (binder.go: `ExportContext`, `ContainsThis`, `ReachabilityAndEmitFlags`
/// = `HasImplicitReturn | HasExplicitReturn | HasAsyncFunctions`,
/// `Unreachable`, `ThisNodeOrAnySubNodesHasError`). `NodeBindData::
/// added_flags` holds only these (checked, in release builds too, by
/// `ast::bind_store_records`, which ORs them into the node record), so for
/// a mask without them Go `node.Flags & mask` is the parser flags `& mask`
/// (`Node::parser_flags`).
pub const BINDER_ADDED_FLAGS: NodeFlags = NodeFlags::EXPORT_CONTEXT
    .union(NodeFlags::CONTAINS_THIS)
    .union(NodeFlags::REACHABILITY_AND_EMIT_FLAGS)
    .union(NodeFlags::UNREACHABLE)
    .union(NodeFlags::THIS_NODE_OR_ANY_SUB_NODES_HAS_ERROR);

/// Every `NodeFlags` bit that the binder does not add: the mask of
/// `Node::parser_flags` for "all the parser flags".
pub const PARSER_ONLY_FLAGS: NodeFlags = NodeFlags(!BINDER_ADDED_FLAGS.0);

/// Binder data for nodes of a file that is not bound yet.
static NO_BIND: NodeBindData = NodeBindData {
    symbol: SymbolId::NIL,
    local_symbol: SymbolId::NIL,
    locals: SymbolTable::NIL,
    next_container: Node::NIL,
    flow_node: FlowNodeId::NIL,
    end_flow_node: FlowNodeId::NIL,
    return_flow_node: FlowNodeId::NIL,
    added_flags: NodeFlags::NONE,
};

/// The flow nodes of `go_file`. Panics before its binder built them.
#[inline]
fn flow_nodes_of(go_file: &GoFile) -> &[FlowNode] {
    go_file
        .flow_nodes
        .get()
        .expect("flow nodes are not built for this file")
}

// ──────────────────────────────────────────────────────────────────────
// core.TextRange (internal/core/text.go)
// ──────────────────────────────────────────────────────────────────────

/// Go `core.TextRange`. Positions are Go positions (UTF-8 byte offsets).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct TextRange {
    pos: i32,
    end: i32,
}

impl TextRange {
    // Go: core/text.go:14 NewTextRange
    #[must_use]
    pub const fn new(pos: i32, end: i32) -> Self {
        Self { pos, end }
    }

    // Go: core/text.go:18 UndefinedTextRange
    #[must_use]
    pub const fn undefined() -> Self {
        Self { pos: -1, end: -1 }
    }

    // Go: core/text.go:22 Pos
    #[must_use]
    pub const fn pos(self) -> i32 {
        self.pos
    }

    // Go: core/text.go:26 End
    #[must_use]
    pub const fn end(self) -> i32 {
        self.end
    }

    // Go: core/text.go:30 Len
    #[must_use]
    pub const fn len(self) -> i32 {
        self.end - self.pos
    }

    // Go: core/text.go:34 IsValid
    #[must_use]
    pub const fn is_valid(self) -> bool {
        self.pos >= 0 || self.end >= 0
    }

    // Go: core/text.go:38 Contains
    #[must_use]
    pub const fn contains(self, pos: i32) -> bool {
        pos >= self.pos && pos < self.end
    }

    // Go: core/text.go:42 ContainsInclusive
    #[must_use]
    pub const fn contains_inclusive(self, pos: i32) -> bool {
        pos >= self.pos && pos <= self.end
    }

    // Go: core/text.go:46 ContainsExclusive
    #[must_use]
    pub const fn contains_exclusive(self, pos: i32) -> bool {
        self.pos < pos && pos < self.end
    }

    // Go: core/text.go:50 WithPos
    #[must_use]
    pub const fn with_pos(self, pos: i32) -> Self {
        Self { pos, end: self.end }
    }

    // Go: core/text.go:54 WithEnd
    #[must_use]
    pub const fn with_end(self, end: i32) -> Self {
        Self { pos: self.pos, end }
    }

    // Go: core/text.go:58 ContainedBy
    #[must_use]
    pub const fn contained_by(self, t2: TextRange) -> bool {
        t2.pos <= self.pos && t2.end >= self.end
    }

    // Go: core/text.go:62 Overlaps
    #[must_use]
    pub fn overlaps(self, t2: TextRange) -> bool {
        let start = self.pos.max(t2.pos);
        let end = self.end.min(t2.end);
        start < end
    }

    // Go: core/text.go:70 Intersects
    /// Like `overlaps`, but touching ranges intersect.
    #[must_use]
    pub fn intersects(self, t2: TextRange) -> bool {
        let start = self.pos.max(t2.pos);
        let end = self.end.min(t2.end);
        start <= end
    }
}

// Go: core/text.go:76 CompareTextRanges
#[must_use]
pub fn compare_text_ranges(r1: TextRange, r2: TextRange) -> i32 {
    let c = r1.pos - r2.pos;
    if c != 0 {
        return c;
    }
    r1.end - r2.end
}

/// A astdata range as a Go `core.TextRange`.
fn text_range_of(r: &crate::astdata::text::TextRange) -> TextRange {
    TextRange::new(r.start.get() as i32, r.end.get() as i32)
}

/// Go `node.Text()` of Identifier or PrivateIdentifier `n`, whose data text
/// is `text`.
// PERF: U1 (d). A store node made by `alloc_store_name_node` or
// `alloc_store_shared_name_node` (S1) has an empty data text; its text is
// the name of its slot (`store_identifier_name`). A
// data text that is not empty is the text: synthetic and cloned nodes
// keep it, and a store slot whose data has one has that name.
#[inline]
fn name_node_text(n: Node, text: &'static str) -> &'static str {
    if !text.is_empty() {
        return text;
    }
    store_identifier_name(n).map_or("", |name| name.as_str())
}

/// `name_node_text` for data text `text` that is not `'static` (a node that
/// a freeable parse owns, lsshells M3c, or a factory node): a text that is
/// not empty is interned.
fn scoped_name_text(n: Node, text: &str) -> &'static str {
    if !text.is_empty() {
        return Name::from(text).as_str();
    }
    store_identifier_name(n).map_or("", |name| name.as_str())
}

/// The last step of Go `QuestionToken`: the postfix token when it is `?`.
fn question_of_postfix(postfix: Node) -> Node {
    if postfix.is_some() && postfix.kind() == SyntaxKind::QuestionToken {
        return postfix;
    }
    Node::NIL
}

// ──────────────────────────────────────────────────────────────────────
// NodeSlice: Go `[]*Node`
// ──────────────────────────────────────────────────────────────────────

/// Go `[]*Node`. It points at a astdata id list of one file, at a slice of
/// `Node`s that lives for the process, or at a list of this thread's
/// synthetic arena. The default value is Go `nil`.
#[derive(Clone, Copy, Debug)]
pub struct NodeSlice(SliceRepr);

#[derive(Clone, Copy, Debug)]
enum SliceRepr {
    /// Ts_ast ids in `file`.
    Ids {
        file: u32,
        ids: &'static [crate::astdata::NodeId],
    },
    /// Nodes that live for the process.
    Nodes(&'static [Node]),
    /// The `len` nodes of a list of this thread's synthetic arena.
    Synthetic { list: SyntheticList, len: u32 },
    /// A list of `len` nodes of freeable file version `file` (lsshells M3b),
    /// read at each use (`file_list_node`): Go `SourceFile.Imports()` when
    /// `host` is nil (`NodeSlice::from_file_imports`), else the eager JSDoc
    /// of `host` in its JSDoc cache (`NodeSlice::from_file_js_doc`; before
    /// the publish, the cache of its store, lsshells M3c). A published list
    /// does not change, so its length is kept here.
    FileList { file: u32, len: u32, host: Node },
    /// The `len` nodes of a list of a store that owns its nodes (a freeable
    /// parse, lsshells M3c), read at each use (`with_store_list`).
    Store { list: StoreList, len: u32 },
}

impl Default for NodeSlice {
    fn default() -> Self {
        Self::NIL
    }
}

impl NodeSlice {
    /// Go `nil`.
    pub const NIL: Self = Self(SliceRepr::Nodes(&[]));

    /// A slice over astdata ids in `file`.
    #[must_use]
    pub fn from_ids(file: usize, ids: &'static [crate::astdata::NodeId]) -> Self {
        Self(SliceRepr::Ids {
            file: file as u32,
            ids,
        })
    }

    /// A slice over nodes that live for the program.
    #[must_use]
    pub fn from_nodes(nodes: &'static [Node]) -> Self {
        Self(SliceRepr::Nodes(nodes))
    }

    /// Go `SourceFile.Imports()` of published file `file` (a file id), read
    /// at each use, for a file whose info is not `'static` (a freeable file
    /// version). A use after the version dies panics.
    #[must_use]
    pub fn from_file_imports(file: usize) -> Self {
        Self::from_file_list(file, Node::NIL)
    }

    /// The JSDoc cache entry of `host` in published file `file`, read at
    /// each use (see `from_file_imports`). `host` must be in the cache.
    #[must_use]
    pub fn from_file_js_doc(file: usize, host: Node) -> Self {
        debug_assert!(host.is_some());
        Self::from_file_list(file, host)
    }

    /// `SliceRepr::FileList` of `file` and `host`.
    fn from_file_list(file: usize, host: Node) -> Self {
        Self(SliceRepr::FileList {
            file: file as u32,
            len: file_list_len(file, host) as u32,
            host,
        })
    }

    /// The nodes of list `list` of a store that owns its nodes (lsshells
    /// M3c).
    #[must_use]
    pub fn from_store(list: StoreList) -> Self {
        Self(SliceRepr::Store {
            list,
            len: list.len() as u32,
        })
    }

    /// The nodes of a list of this thread's synthetic arena.
    #[must_use]
    pub fn from_synthetic(list: SyntheticList) -> Self {
        Self(SliceRepr::Synthetic {
            list,
            len: synthetic_list_len(list) as u32,
        })
    }

    #[inline]
    #[must_use]
    pub fn len(self) -> usize {
        match self.0 {
            SliceRepr::Ids { ids, .. } => ids.len(),
            SliceRepr::Nodes(nodes) => nodes.len(),
            SliceRepr::Synthetic { len, .. } => len as usize,
            SliceRepr::FileList { len, .. } => len as usize,
            SliceRepr::Store { len, .. } => len as usize,
        }
    }

    #[must_use]
    pub fn is_empty(self) -> bool {
        self.len() == 0
    }

    /// The file and the astdata ids of a slice over the ids of one store
    /// (`from_ids`), for `BuildStoreReads::node`. `None` for other slices.
    #[must_use]
    pub fn store_ids(self) -> Option<(usize, &'static [crate::astdata::NodeId])> {
        match self.0 {
            SliceRepr::Ids { file, ids } => Some((file as usize, ids)),
            _ => None,
        }
    }

    /// Go `nodes[i]`. Panics when `i` is out of range, like Go.
    #[inline]
    #[must_use]
    pub fn get(self, i: usize) -> Node {
        match self.0 {
            SliceRepr::Ids { file, ids } => Node::new(file as usize, ids[i]),
            SliceRepr::Nodes(nodes) => nodes[i],
            SliceRepr::Synthetic { list, len } => {
                assert!(
                    i < len as usize,
                    "index {i} out of range for a slice of {len}"
                );
                synthetic_list_node(list, i)
            }
            SliceRepr::FileList { file, host, .. } => file_list_node(file as usize, host, i),
            SliceRepr::Store { list, .. } => store_list_node(list, i),
        }
    }

    #[must_use]
    pub fn first(self) -> Option<Node> {
        if self.is_empty() {
            None
        } else {
            Some(self.get(0))
        }
    }

    #[must_use]
    pub fn last(self) -> Option<Node> {
        if self.is_empty() {
            None
        } else {
            Some(self.get(self.len() - 1))
        }
    }

    #[must_use]
    #[inline]
    pub fn iter(self) -> NodeSliceIter {
        let ids = match self.0 {
            SliceRepr::Nodes(nodes) => SliceIds::Nodes(nodes),
            // An empty slice reads no node, so it needs no store lookup.
            SliceRepr::Ids { ids, .. } if ids.is_empty() => SliceIds::Nodes(&[]),
            SliceRepr::Ids { file, ids } => match frozen_store_ids(file as usize) {
                Some(frozen) => SliceIds::Frozen(ids, frozen),
                None => SliceIds::Slow,
            },
            SliceRepr::Synthetic { .. } | SliceRepr::FileList { .. } => SliceIds::Slow,
            // An empty slice reads no node.
            SliceRepr::Store { len: 0, .. } => SliceIds::Nodes(&[]),
            // lsshells M3f: a short list is read once into the iterator; a
            // longer one is read at each step. Neither copies to the heap.
            SliceRepr::Store { list, len } if len as usize <= INLINE_NODES => {
                SliceIds::Inline(store_list_first_nodes(list))
            }
            SliceRepr::Store { list, .. } => SliceIds::Store(list),
        };
        NodeSliceIter {
            slice: self,
            ids,
            front: 0,
            back: self.len(),
        }
    }

    #[must_use]
    pub fn to_vec(self) -> Vec<Node> {
        self.iter().collect()
    }
}

/// The length of the list of `SliceRepr::FileList { file, host }`.
fn file_list_len(file: usize, host: Node) -> usize {
    let published = crate::ast::try_with_go_file(file, |g| {
        if host.is_nil() {
            g.info.imports.len()
        } else {
            g.info.jsdoc_cache[&host].len()
        }
    });
    // lsshells M3c: the JSDoc of a store that owns its nodes, before the
    // publish.
    published.unwrap_or_else(|| owned_store_js_doc(file, host, <[Node]>::len))
}

/// `read` on the JSDoc cache entry of `host` in unpublished store `file`,
/// which owns its nodes (lsshells M3c). Panics as `with_go_file` for any
/// other file.
#[cold]
#[inline(never)]
fn owned_store_js_doc<R>(file: usize, host: Node, read: impl FnOnce(&[Node]) -> R) -> R {
    match with_owned_store_js_doc(file, host, read) {
        Some(result) => result,
        None => panic!("file {file} is not published"),
    }
}

/// Node `i` of `SliceRepr::FileList { file, host }`. Panics when `i` is
/// out of range, like Go.
// PERF: lsshells M3b. Out of line, so `NodeSlice::get` keeps the R134
// inline code for the other variants.
#[cold]
#[inline(never)]
fn file_list_node(file: usize, host: Node, i: usize) -> Node {
    let published = crate::ast::try_with_go_file(file, |g| {
        if host.is_nil() {
            g.info.imports[i]
        } else {
            g.info.jsdoc_cache[&host][i]
        }
    });
    published.unwrap_or_else(|| owned_store_js_doc(file, host, |jsdocs| jsdocs[i]))
}

/// The most nodes of a store list that `NodeSliceIter` reads once
/// (`SliceIds::Inline`).
const INLINE_NODES: usize = 4;

/// Entries of `HOT_LIST_NODES`.
const HOT_LIST_NODE_SLOTS: usize = 256;

thread_local! {
    /// The first nodes of short lists of hot file versions
    /// (`store_list_first_nodes`), by list handle, direct mapped (lsshells
    /// M3f). A published list never changes, and a handle names its file
    /// id, which is never reused; only a list of a hot version reads it.
    // PERF: the checker loops over the same short lists of the edited file
    // many times (parameters, type arguments).
    static HOT_LIST_NODES: RefCell<[(Option<StoreList>, [Node; INLINE_NODES]); HOT_LIST_NODE_SLOTS]> =
        const { RefCell::new([(None, [Node::NIL; INLINE_NODES]); HOT_LIST_NODE_SLOTS]) };
}

/// The first `INLINE_NODES` nodes of `SliceRepr::Store { list }` (nil after
/// its end).
#[cold]
#[inline(never)]
fn store_list_first_nodes(list: StoreList) -> [Node; INLINE_NODES] {
    if !crate::ast::is_hot_file(list.file()) {
        return read_store_list_first_nodes(list);
    }
    let slot = list.hash_slot(HOT_LIST_NODE_SLOTS);
    let hit = HOT_LIST_NODES
        .try_with(|lists| {
            let (key, nodes) = lists.borrow()[slot];
            (key == Some(list)).then_some(nodes)
        })
        .ok()
        .flatten();
    if let Some(nodes) = hit {
        return nodes;
    }
    let nodes = read_store_list_first_nodes(list);
    let _ = HOT_LIST_NODES.try_with(|lists| {
        if let Ok(mut lists) = lists.try_borrow_mut() {
            lists[slot] = (Some(list), nodes);
        }
    });
    nodes
}

/// `store_list_first_nodes`, read from the store.
fn read_store_list_first_nodes(list: StoreList) -> [Node; INLINE_NODES] {
    let file = list.file();
    let mut nodes = [Node::NIL; INLINE_NODES];
    with_store_list(list, |l| {
        for (node, &id) in nodes.iter_mut().zip(l.ids()) {
            *node = Node::new(file, id);
        }
    });
    nodes
}

/// Node `i` of `SliceRepr::Store { list }`. Panics when `i` is out of
/// range, like Go.
#[cold]
#[inline(never)]
fn store_list_node(list: StoreList, i: usize) -> Node {
    let id = with_store_list(list, |l| l.ids()[i]);
    Node::new(list.file(), id)
}

// PERF: 24 bytes. The synthetic list is a variant of `SyntheticList`, whose
// tag and index fit next to the length.
const _: () = assert!(std::mem::size_of::<NodeSlice>() == 24);

/// How a `NodeSliceIter` turns position `i` into a node, read once for the
/// whole loop.
#[derive(Clone, Copy, Debug)]
enum SliceIds {
    /// The nodes of the slice (`NodeSlice::from_nodes`).
    Nodes(&'static [Node]),
    /// The ids of the slice, of a frozen store (`frozen_store_ids`).
    Frozen(&'static [crate::astdata::NodeId], FrozenIds),
    /// A list of a store that owns its nodes, read at each step
    /// (`store_list_node`, lsshells M3f).
    Store(StoreList),
    /// The nodes of a short list of a store that owns its nodes, read once
    /// (`store_list_first_nodes`, lsshells M3f).
    Inline([Node; INLINE_NODES]),
    /// `NodeSlice::get` per node: ids before freeze, synthetic lists and
    /// the file lists of freeable file versions.
    Slow,
}

/// Iterator over a `NodeSlice`. Yields `Node` by value.
#[derive(Clone, Debug)]
pub struct NodeSliceIter {
    slice: NodeSlice,
    ids: SliceIds,
    front: usize,
    back: usize,
}

impl NodeSliceIter {
    /// `self.slice.get(i)`, without the per-node store lookup of a published store.
    // PERF: U1 (c). The store facts are read once in `NodeSlice::iter`. For
    // an alias-free store (most stores) each id then becomes its handle with
    // no table load (`FrozenIds::Direct`).
    #[inline]
    fn at(&self, i: usize) -> Node {
        match &self.ids {
            SliceIds::Nodes(nodes) => nodes[i],
            &SliceIds::Frozen(slice_ids, ids) => {
                let n = ids.node(slice_ids[i]);
                debug_assert_eq!(n, self.slice.get(i));
                n
            }
            &SliceIds::Store(list) => store_list_node(list, i),
            SliceIds::Inline(nodes) => nodes[i],
            SliceIds::Slow => self.slice.get(i),
        }
    }
}

impl Iterator for NodeSliceIter {
    type Item = Node;

    fn next(&mut self) -> Option<Node> {
        if self.front >= self.back {
            return None;
        }
        let n = self.at(self.front);
        self.front += 1;
        Some(n)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let n = self.back - self.front;
        (n, Some(n))
    }
}

impl DoubleEndedIterator for NodeSliceIter {
    fn next_back(&mut self) -> Option<Node> {
        if self.front >= self.back {
            return None;
        }
        self.back -= 1;
        Some(self.at(self.back))
    }
}

impl ExactSizeIterator for NodeSliceIter {}

impl IntoIterator for NodeSlice {
    type Item = Node;
    type IntoIter = NodeSliceIter;

    fn into_iter(self) -> NodeSliceIter {
        self.iter()
    }
}

impl IntoIterator for &NodeSlice {
    type Item = Node;
    type IntoIter = NodeSliceIter;

    fn into_iter(self) -> NodeSliceIter {
        self.iter()
    }
}

// ──────────────────────────────────────────────────────────────────────
// NodeList
// ──────────────────────────────────────────────────────────────────────

/// Go `*ast.NodeList`. `NodeList::NIL` is Go `nil`. Equality is pointer
/// equality, like Go.
// PERF: astshrink1. Node data holds a list field as a 4-byte index into the
// list table of its owner (`crate::astdata::NodeList`); the handle names the
// table entry, so a list that the parser made and the same list read from
// node data are one list, like the Go pointer. The handle stays 16 bytes
// (tag, file or index, pointer), so it returns in registers.
#[derive(Clone, Copy, Debug, Default)]
pub struct NodeList(ListRef);

/// The list that a `NodeList` names.
#[derive(Clone, Copy, Debug, Default)]
enum ListRef {
    /// Go `nil`.
    #[default]
    Nil,
    /// A list of static store `file` (`StaticList`). It lives for the
    /// process.
    Ts {
        file: u32,
        list: &'static StaticList,
    },
    /// A list of this thread's synthetic arena (`SyntheticList::Own`): a
    /// factory list or a list field of synthetic node data.
    Own { index: u32 },
    /// A list of a store that owns its nodes (a freeable parse, lsshells
    /// M3c), read at each use (`with_store_list`).
    Store(StoreList),
}

const _: () = assert!(std::mem::size_of::<NodeList>() == 16);

impl ListRef {
    fn synthetic(list: SyntheticList) -> Self {
        match list {
            SyntheticList::Own { index } => Self::Own { index },
            SyntheticList::Slice { .. } => panic!("a node slice is not a NodeList"),
        }
    }

    fn synthetic_list(self) -> Option<SyntheticList> {
        match self {
            Self::Own { index } => Some(SyntheticList::Own { index }),
            Self::Nil | Self::Ts { .. } | Self::Store(_) => None,
        }
    }
}

impl PartialEq for NodeList {
    fn eq(&self, other: &Self) -> bool {
        match (self.is_nil(), other.is_nil()) {
            (true, true) => true,
            (false, false) => match (self.0, other.0) {
                (ListRef::Ts { list: a, .. }, ListRef::Ts { list: b, .. }) => std::ptr::eq(a, b),
                (ListRef::Store(a), ListRef::Store(b)) => same_store_list(a, b),
                (ListRef::Own { index: a }, ListRef::Own { index: b }) => a == b,
                _ => false,
            },
            _ => false,
        }
    }
}

impl Eq for NodeList {}

impl NodeList {
    /// Go `nil`.
    pub const NIL: Self = Self(ListRef::Nil);

    /// The handle of list `list` of static store `file`.
    #[inline]
    #[must_use]
    pub(crate) fn from_static(file: usize, list: &'static StaticList) -> Self {
        Self(ListRef::Ts {
            file: file as u32,
            list,
        })
    }

    /// The handle of list field `list` (`None` is Go `nil`) of node data of
    /// static store `file`, whose list table is `lists`.
    #[inline]
    #[must_use]
    pub(crate) fn from_table(
        file: usize,
        lists: &'static [&'static StaticList],
        list: Option<&crate::astdata::NodeList>,
    ) -> Self {
        match list {
            Some(list) => Self::from_static(file, lists[list.index()]),
            None => Self::NIL,
        }
    }

    /// The handle of a list of this thread's synthetic arena.
    #[must_use]
    pub fn synthetic(list: SyntheticList) -> Self {
        Self(ListRef::synthetic(list))
    }

    /// The handle of list `list` of a store that owns its nodes (lsshells
    /// M3c).
    #[inline]
    #[must_use]
    pub fn store(list: StoreList) -> Self {
        Self(ListRef::Store(list))
    }

    /// The file of the list: a store id or `SYNTHETIC_NODE_FILE`. 0 for
    /// `NodeList::NIL`.
    #[inline]
    #[must_use]
    pub fn file(self) -> usize {
        match self.0 {
            ListRef::Nil => 0,
            ListRef::Ts { file, .. } => file as usize,
            ListRef::Store(list) => list.file(),
            ListRef::Own { .. } => SYNTHETIC_NODE_FILE,
        }
    }

    /// The list of a static store, else `None`.
    #[inline]
    #[must_use]
    pub fn static_list(self) -> Option<&'static StaticList> {
        match self.0 {
            ListRef::Ts { list, .. } => Some(list),
            _ => None,
        }
    }

    /// True for the Go `nil` handle (`NodeList::NIL`). Unlike `is_nil`,
    /// false for a nil marker list (`store::NIL_LIST_POS`).
    #[inline]
    #[must_use]
    pub(crate) const fn is_nil_handle(self) -> bool {
        matches!(self.0, ListRef::Nil)
    }

    /// The store id and the list of a list of a static store, else `None`
    /// (`store::store_list_value`).
    #[inline]
    #[must_use]
    pub(crate) fn static_parts(self) -> Option<(usize, &'static StaticList)> {
        match self.0 {
            ListRef::Ts { file, list } => Some((file as usize, list)),
            _ => None,
        }
    }

    /// The synthetic list of the handle, else `None`.
    #[inline]
    #[must_use]
    pub fn synthetic_list(self) -> Option<SyntheticList> {
        self.0.synthetic_list()
    }

    /// The list of a store that owns its nodes (lsshells M3c), else `None`.
    #[inline]
    #[must_use]
    pub fn store_list(self) -> Option<StoreList> {
        match self.0 {
            ListRef::Store(list) => Some(list),
            _ => None,
        }
    }

    /// The address of the list, for Go pointer compares and keys. `None` for
    /// `NodeList::NIL`. A Go `nil` marker list (`store::NIL_LIST_POS`) has an
    /// address. A synthetic list keeps its address while its thread's arena
    /// lives (`synthetic_list_ptr`), and a list of a store that owns its
    /// nodes while that store lives (lsshells M3c): an address can repeat
    /// after the store is freed, so keep it only for keys of one request.
    #[must_use]
    pub fn list_ptr(self) -> Option<*const ()> {
        match self.0 {
            ListRef::Nil => None,
            ListRef::Ts { list, .. } => Some(std::ptr::from_ref(list).cast()),
            ListRef::Store(list) => Some(with_store_list(list, |l| l.ptr())),
            ListRef::Own { index } => Some(synthetic_list_ptr(SyntheticList::Own { index })),
        }
    }

    /// The astdata `has_trailing_comma` bit of the list, false for
    /// `NodeList::NIL`. Go computes `HasTrailingComma` from the list ends
    /// (`has_trailing_comma`). The parser sets this bit on an empty list as
    /// the missing-list marker (`with_missing_marker`), and synthetic copies
    /// keep it.
    #[must_use]
    pub fn stored_trailing_comma(self) -> bool {
        match self.0 {
            ListRef::Nil => false,
            ListRef::Ts { list, .. } => list.has_trailing_comma(),
            ListRef::Store(list) => list.stored_trailing_comma(),
            ListRef::Own { index } => {
                with_synthetic_list(SyntheticList::Own { index }, |l| l.has_trailing_comma)
            }
        }
    }

    /// Parser `createMissingList`: a new empty list at the `Loc` of this
    /// (empty) list, with the astdata `has_trailing_comma` bit set, the marker
    /// that parser.rs `is_missing_node_list` reads. Go `nil` stays nil.
    #[must_use]
    pub fn with_missing_marker(self) -> NodeList {
        match self.0 {
            ListRef::Nil => self,
            ListRef::Ts { file, list } => Self::from_static(
                file as usize,
                new_static_missing_list(file as usize, list.range),
            ),
            // lsshells M3c: the store that owns its nodes keeps the list.
            ListRef::Store(list) => {
                let range = with_store_list(list, |l| l.range());
                Self(ListRef::Store(new_store_missing_list(list.file(), range)))
            }
            // A factory parse (lazy JSDoc): the thread's arena owns the list.
            ListRef::Own { index } => {
                Self::synthetic(synthetic_missing_list(SyntheticList::Own { index }))
            }
        }
    }

    /// Go `list == nil`. A required astdata list field of a store node holds
    /// Go `nil` as a marker list (`store::NIL_LIST_POS`). A synthetic list is
    /// never a marker: synthetic data holds Go `nil` in a required list field
    /// as an empty list with an undefined `Loc` (`synthetic_req_list_value`).
    #[must_use]
    pub fn is_nil(self) -> bool {
        match self.0 {
            ListRef::Nil => true,
            ListRef::Ts { list, .. } => is_nil_list_range(&list.range),
            ListRef::Store(list) => list.is_nil_marker(),
            ListRef::Own { .. } => false,
        }
    }

    #[must_use]
    pub fn is_some(self) -> bool {
        !self.is_nil()
    }

    /// Go `list.Nodes`. Empty when nil.
    #[must_use]
    pub fn nodes(self) -> NodeSlice {
        match self.0 {
            ListRef::Ts { file, list } => NodeSlice::from_ids(file as usize, list.nodes),
            ListRef::Nil => NodeSlice::NIL,
            ListRef::Store(list) => NodeSlice::from_store(list),
            ListRef::Own { index } => NodeSlice::from_synthetic(SyntheticList::Own { index }),
        }
    }

    /// Go `list.Loc`. Panics when nil, like Go.
    #[must_use]
    pub fn loc(self) -> TextRange {
        match self.0 {
            ListRef::Nil => panic!("nil NodeList dereference"),
            // Store lists hold the Go `Loc`.
            ListRef::Ts { list, .. } => {
                assert!(!is_nil_list_range(&list.range), "nil NodeList dereference");
                text_range_of(&list.range)
            }
            ListRef::Store(list) => {
                let range = with_store_list(list, |l| l.range());
                assert!(!is_nil_list_range(&range), "nil NodeList dereference");
                text_range_of(&range)
            }
            // A synthetic list holds the Go `Loc`.
            ListRef::Own { index } => {
                with_synthetic_list(SyntheticList::Own { index }, |l| text_range_of(&l.range))
            }
        }
    }

    // Go: ast.go:135 Pos
    #[must_use]
    pub fn pos(self) -> i32 {
        self.loc().pos()
    }

    // Go: ast.go:136 End
    #[must_use]
    pub fn end(self) -> i32 {
        self.loc().end()
    }

    // Go: ast.go:138 HasTrailingComma
    #[must_use]
    pub fn has_trailing_comma(self) -> bool {
        let nodes = self.nodes();
        match nodes.last() {
            None => false,
            Some(last) => last.end() < self.end(),
        }
    }
}

// ──────────────────────────────────────────────────────────────────────
// ModifierList
// ──────────────────────────────────────────────────────────────────────

/// Go `*ast.ModifierList`. `ModifierList::NIL` is Go `nil`. Equality is
/// pointer equality, like Go. It names a list table entry like `NodeList`
/// (astshrink1); the entry holds Go `ModifiersToFlags`. 16 bytes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ModifierList(NodeList);

const _: () = assert!(std::mem::size_of::<ModifierList>() == 16);

impl ModifierList {
    /// Go `nil`.
    pub const NIL: Self = Self(NodeList::NIL);

    /// The handle of modifier list `list` of static store `file`.
    #[inline]
    #[must_use]
    pub(crate) fn from_static(file: usize, list: &'static StaticList) -> Self {
        Self(NodeList::from_static(file, list))
    }

    /// `NodeList::from_table` for a modifier list field.
    #[inline]
    #[must_use]
    pub(crate) fn from_table(
        file: usize,
        lists: &'static [&'static StaticList],
        list: Option<&crate::astdata::ModifierList>,
    ) -> Self {
        Self(NodeList::from_table(file, lists, list.map(|m| &m.list)))
    }

    /// The handle of a modifier list of this thread's synthetic arena.
    #[must_use]
    pub fn synthetic(list: SyntheticList) -> Self {
        Self(NodeList::synthetic(list))
    }

    /// The handle of modifier list `list` of a store that owns its nodes
    /// (lsshells M3c).
    #[inline]
    #[must_use]
    pub fn store(list: StoreList) -> Self {
        Self(NodeList::store(list))
    }

    /// The file of the list (see `NodeList::file`). 0 for
    /// `ModifierList::NIL`.
    #[inline]
    #[must_use]
    pub fn file(self) -> usize {
        self.0.file()
    }

    /// The list of a static store, else `None`.
    #[inline]
    #[must_use]
    pub fn static_list(self) -> Option<&'static StaticList> {
        self.0.static_list()
    }

    /// The synthetic list of the handle, else `None`.
    #[inline]
    #[must_use]
    pub fn synthetic_list(self) -> Option<SyntheticList> {
        self.0.synthetic_list()
    }

    /// The list of a store that owns its nodes (lsshells M3c), else `None`.
    #[inline]
    #[must_use]
    pub fn store_list(self) -> Option<StoreList> {
        self.0.store_list()
    }

    #[must_use]
    pub const fn is_nil(self) -> bool {
        matches!(self.0.0, ListRef::Nil)
    }

    #[must_use]
    pub const fn is_some(self) -> bool {
        !self.is_nil()
    }

    /// Go `modifiers.NodeList`.
    #[must_use]
    pub fn node_list(self) -> NodeList {
        self.0
    }

    /// Go `modifiers.Nodes`. Empty when nil.
    #[must_use]
    pub fn nodes(self) -> NodeSlice {
        self.0.nodes()
    }

    #[must_use]
    pub fn pos(self) -> i32 {
        self.0.pos()
    }

    #[must_use]
    pub fn end(self) -> i32 {
        self.0.end()
    }

    #[must_use]
    pub fn loc(self) -> TextRange {
        self.0.loc()
    }

    // Go: ast.go:156 ModifierList.ModifierFlags (set at ast.go:163)
    /// Go `modifiers.ModifierFlags`. NONE when nil.
    // PERF: store and synthetic lists hold Go `ModifiersToFlags(nodes)`,
    // stored when the factory made the list (`new_store_modifier_list`,
    // `new_synthetic_modifier_list` and the `*_modifiers_value` copies), so
    // they read it like Go does instead of walking the modifier nodes.
    #[must_use]
    pub fn modifier_flags(self) -> ModifierFlags {
        match self.0.0 {
            ListRef::Nil => ModifierFlags::NONE,
            ListRef::Ts { list, .. } => ModifierFlags(list.modifier_flags().0),
            ListRef::Store(list) => with_store_list(list, |l| l.modifier_flags()),
            ListRef::Own { index } => {
                with_synthetic_list(SyntheticList::Own { index }, |l| ModifierFlags(l.flags.0))
            }
        }
    }
}

// ──────────────────────────────────────────────────────────────────────
// Visitor helpers
// ──────────────────────────────────────────────────────────────────────

// Go: ast.go:32 visit
fn visit(v: &mut dyn FnMut(Node) -> bool, node: Node) -> bool {
    if node.is_some() {
        return v(node);
    }
    false
}

// Go: ast.go:39 visitNodes
fn visit_nodes(v: &mut dyn FnMut(Node) -> bool, nodes: NodeSlice) -> bool {
    for node in nodes {
        if v(node) {
            return true;
        }
    }
    false
}

// Go: ast.go:48 visitNodeList
fn visit_node_list(v: &mut dyn FnMut(Node) -> bool, node_list: NodeList) -> bool {
    if node_list.is_some() {
        return visit_nodes(v, node_list.nodes());
    }
    false
}

// Go: ast.go:55 visitModifiers
fn visit_modifiers(v: &mut dyn FnMut(Node) -> bool, modifiers: ModifierList) -> bool {
    if modifiers.is_some() {
        return visit_nodes(v, modifiers.nodes());
    }
    false
}

// ──────────────────────────────────────────────────────────────────────
// Node core accessors
// ──────────────────────────────────────────────────────────────────────

thread_local! {
    /// Go `Node.Decorators()` results. Go allocates a new slice on each call;
    /// we keep one per node: a leaked slice for a parsed node, a slice of the
    /// thread's synthetic arena for a factory node (see `Node::decorators`).
    /// It forgets the nodes of dead file versions (`PerFileMap`, lsshells
    /// M3c); their small slices stay leaked.
    static DECORATORS: RefCell<PerFileMap<NodeSlice>> = const { RefCell::new(PerFileMap::new()) };
    /// Go `CompositeBase.facts` of the parsed nodes that have no facts
    /// column (`static_facts_word`): nodes of freeable file versions and of
    /// unpublished stores. Per file (keyed by `facts_file_key`), the cached
    /// `SubtreeFacts` of each node of a `CompositeBase` kind
    /// (`is_composite_data`) by its index, with `COMPUTED` set once cached.
    /// It forgets the files of dead file versions (`PerFileMap`, lsshells
    /// M3a). A factory node keeps its facts in its arena slot
    /// (`synthetic_subtree_facts`).
    // PERF: emitast1 F1. A column per file, not a map entry per node: no
    // hash per read and no growth rehash of a map with one entry per node.
    static SUBTREE_FACTS: RefCell<PerFileMap<Vec<SubtreeFacts>>> = const { RefCell::new(PerFileMap::new()) };
    /// `SUBTREE_FACTS` for a node whose index is too large for a column
    /// (`FACTS_COLUMN_LIMIT`).
    static SUBTREE_FACTS_SPARSE: RefCell<PerFileMap<SubtreeFacts>> = const { RefCell::new(PerFileMap::new()) };
    /// Go `Node.Text()` results that Go builds with string concatenation. It
    /// forgets the nodes of dead file versions (`PerFileMap`).
    static JOINED_TEXT: RefCell<PerFileMap<&'static str>> = const { RefCell::new(PerFileMap::new()) };
}

/// The node indexes that a facts column holds (`SUBTREE_FACTS`). A larger
/// index (a file of over 4 million nodes) goes in `SUBTREE_FACTS_SPARSE`,
/// so a column never holds more than 16 MiB.
const FACTS_COLUMN_LIMIT: usize = 1 << 22;

/// The index of non-nil node `n` in its file (`SUBTREE_FACTS`).
#[inline]
fn facts_index(n: Node) -> usize {
    (n.0 & 0xffff_ffff).wrapping_sub(1) as usize
}

/// The `SUBTREE_FACTS` key of the file of `n`: the handle of index 0 of
/// that file, so `PerFileMap` forgets it with the file version.
#[inline]
fn facts_file_key(n: Node) -> Node {
    Node((n.0 & !0xffff_ffff) | 1)
}

/// The cached `Node::subtree_facts` of `n`, a node of a `CompositeBase`
/// kind, if any. A node of a static publish keeps them in the facts column
/// of its file (`static_facts_word`, factscol1b), a factory node in its
/// arena slot (emitast1 F1), any other node in `SUBTREE_FACTS`.
#[cfg(test)]
fn cached_subtree_facts(n: Node) -> Option<SubtreeFacts> {
    if let Some(word) = super::store::static_facts_word(n) {
        return computed_facts(SubtreeFacts(word.load(Ordering::Relaxed)));
    }
    cached_unpublished_subtree_facts(n)
}

/// `cached_subtree_facts` for a node with no facts column.
#[inline(never)]
fn cached_unpublished_subtree_facts(n: Node) -> Option<SubtreeFacts> {
    if is_synthetic_node(n) {
        return synthetic_subtree_facts(n);
    }
    let index = facts_index(n);
    if index >= FACTS_COLUMN_LIMIT {
        return SUBTREE_FACTS_SPARSE.with(|c| c.borrow().get(&n).copied());
    }
    SUBTREE_FACTS.with(|c| computed_facts(*c.borrow().get(&facts_file_key(n))?.get(index)?))
}

/// The facts of a cache entry `facts`, or `None` when it has no `COMPUTED`
/// bit (no facts cached).
#[inline]
fn computed_facts(facts: SubtreeFacts) -> Option<SubtreeFacts> {
    facts
        .intersects(SubtreeFacts::COMPUTED)
        .then(|| facts.without(SubtreeFacts::COMPUTED))
}

/// Caches `facts` as the `Node::subtree_facts` of `n`, a node with no facts
/// column (`cached_unpublished_subtree_facts`).
#[inline(never)]
fn cache_unpublished_subtree_facts(n: Node, facts: SubtreeFacts) {
    if is_synthetic_node(n) {
        return set_synthetic_subtree_facts(n, facts);
    }
    let index = facts_index(n);
    if index >= FACTS_COLUMN_LIMIT {
        return SUBTREE_FACTS_SPARSE.with(|c| {
            c.borrow_mut().write().insert(n, facts);
        });
    }
    SUBTREE_FACTS.with(|c| {
        let mut c = c.borrow_mut();
        let column = c.write().entry(facts_file_key(n)).or_default();
        if column.len() <= index {
            // `resize` reserves with doubling, so a column grows in
            // amortized steps.
            column.resize(index + 1, SubtreeFacts::NONE);
        }
        column[index] = facts | SubtreeFacts::COMPUTED;
    });
}

impl Node {
    /// Go `node.Kind`. Published store nodes read their node record inline;
    /// other nodes take `kind_slow`.
    #[inline]
    #[must_use]
    pub fn kind(self) -> SyntaxKind {
        match frozen_store_kind(self) {
            Some(kind) => kind,
            None => self.kind_slow(),
        }
    }

    #[inline(never)]
    fn kind_slow(self) -> SyntaxKind {
        // PERF: perf15 S0. A synthetic node reads its kind in the arena, with
        // no store lookup and no data chunk clone (`synthetic_kind`).
        if is_synthetic_node(self) {
            return synthetic_kind(self);
        }
        // PERF: storefast1. A node of the file this thread parses reads its
        // kind column, with no whole header (as `flags_slow`).
        if let Some(kind) = active_store_kind(self) {
            return kind;
        }
        // A store node holds the Go kind in its header.
        if let Some(h) = try_store_header(self) {
            return h.kind;
        }
        // Go reads the kind of a nil node.
        if self.is_nil() {
            // Go N' loads `node.Kind` (offset 0): addr=0x0.
            crate::core::go_nil_dereference_at(0x0);
        }
        unreachable!("node {self:?} is not synthetic and has no store")
    }

    /// Go `node.Flags`: parser flags plus the flags the binder adds.
    // PERF: AST node records, step 2. The bind ORs its flags into the record
    // (`bind_store_records`), so a published store node reads one word.
    #[inline]
    #[must_use]
    pub fn flags(self) -> NodeFlags {
        match frozen_store_flags(self) {
            Some(flags) => flags,
            None => self.flags_slow(),
        }
    }

    /// Go `node.Flags & mask` for a `mask` without a binder-added bit
    /// (`BINDER_ADDED_FLAGS`). For such a mask the binder bits do not
    /// matter, so a published store node reads only its record flags.
    // PERF: U4 (CH7). When this was added, `Node::flags` loaded the binder
    // data of the file (`Node::bind`) for every test, also of a parser-only
    // bit such as `AMBIENT`, `OPTIONAL_CHAIN` or `HAS_JS_DOC`. Since AST
    // node records step 2a the bind ORs its flags into the record, so this
    // is now the same read as `Node::flags() & mask`.
    #[inline]
    #[must_use]
    pub fn parser_flags(self, mask: NodeFlags) -> NodeFlags {
        debug_assert!(
            !mask.intersects(BINDER_ADDED_FLAGS),
            "parser_flags mask {:#x} has binder bits",
            mask.bits()
        );
        match frozen_store_flags(self) {
            Some(flags) => {
                debug_assert_eq!(flags & mask, self.flags() & mask);
                flags & mask
            }
            // `Node::flags` after its record missed.
            None => self.flags_slow() & mask,
        }
    }

    #[inline(never)]
    fn flags_slow(self) -> NodeFlags {
        // PERF: a node of the file this thread parses (query Q8). No
        // program holds that file yet, so there are no binder flags and no
        // `FROZEN` or program check is needed.
        // PERF: storefast1. Only the flags word of its record.
        if let Some(flags) = active_store_flags(self) {
            return flags;
        }
        if is_synthetic_node(self) {
            return synthetic_flags(self);
        }
        if let Some(h) = try_store_header(self) {
            // The parser reads flags before the file is published. The bind
            // of a published file ORs its flags into the record.
            return h.flags;
        }
        unreachable!("node {self:?} is not synthetic and has no store")
    }

    /// Go `node.Parent`.
    #[inline]
    #[must_use]
    pub fn parent(self) -> Node {
        match frozen_store_parent(self) {
            Some(parent) => parent,
            None => self.parent_slow(),
        }
    }

    #[inline(never)]
    fn parent_slow(self) -> Node {
        if is_synthetic_node(self) {
            return synthetic_parent(self);
        }
        if let Some(h) = try_store_header(self) {
            return h.parent;
        }
        unreachable!("node {self:?} is not synthetic and has no store")
    }

    /// Go `node.Loc`.
    #[inline]
    #[must_use]
    pub fn loc(self) -> TextRange {
        match frozen_store_loc(self) {
            Some(loc) => loc,
            None => self.loc_slow(),
        }
    }

    #[inline(never)]
    fn loc_slow(self) -> TextRange {
        if is_synthetic_node(self) {
            return synthetic_loc(self);
        }
        if let Some(h) = try_store_header(self) {
            return h.loc;
        }
        unreachable!("node {self:?} is not synthetic and has no store")
    }

    // Go: ast.go:189 Pos
    #[must_use]
    pub fn pos(self) -> i32 {
        self.loc().pos()
    }

    // Go: ast.go:190 End
    #[must_use]
    pub fn end(self) -> i32 {
        self.loc().end()
    }

    /// The published file that holds this node. The guard pins a freeable
    /// file version while it lives (see `ast::go_file`).
    #[must_use]
    pub fn go_file(self) -> FileRef<GoFile> {
        crate::ast::go_file(self.file_index())
    }

    // Go: ast_generated.go:11710 Name
    // PERF: U4 (CH6). A published store node reads its name child from the
    // store column (`frozen_store_child`), not from its node data. Debug
    // builds compare it with the data read.
    #[must_use]
    pub fn name(self) -> Node {
        match frozen_store_child(self, StoreChild::Name) {
            Some(name) => {
                debug_assert_eq!(name, self.name_data(), "U4 name column");
                name
            }
            None => self.name_data(),
        }
    }

    /// `Node::name` on `d`, the data of this node that the caller already
    /// loaded with `parsed_node_data` (see `data_accessor!`).
    #[must_use]
    pub fn name_in(self, d: LoadedData) -> Node {
        match frozen_store_child(self, StoreChild::Name) {
            Some(name) => {
                debug_assert_eq!(name, self.name_data_in(d), "U4 name column");
                name
            }
            None => self.name_data_in(d),
        }
    }

    // `name_data` and `name_data_in`: Go `Name()` from the node data.
    name_arms!(name_data_accessor!(), opt, req);

    // Go: ast_generated.go:11835 Modifiers
    modifiers_variants!(modifiers_accessor! {});

    // Go: ast.go:200 ParameterList
    /// Go `FunctionLikeData().Parameters`. Nil for other nodes.
    // PORT: Go dereferences a nil FunctionLikeData here; we return nil.
    #[must_use]
    pub fn parameter_list(self) -> NodeList {
        list_by_data!(
            self,
            list_of,
            NodeList::NIL,
            [
                ArrowFunction,
                CallSignatureDeclaration,
                ConstructSignatureDeclaration,
                ConstructorDeclaration,
                FunctionDeclaration,
                FunctionExpression,
                GetAccessorDeclaration,
                SetAccessorDeclaration,
                IndexSignatureDeclaration,
                JsDocSignature,
                MethodDeclaration,
                MethodSignatureDeclaration,
                FunctionTypeNode,
                ConstructorTypeNode,
            ] => req parameters,
        )
    }

    // Go: ast.go:201 Parameters
    #[must_use]
    pub fn parameters(self) -> NodeSlice {
        self.parameter_list().nodes()
    }

    // Go: ast_generated.go:9856 SubtreeFacts
    /// Go `node.SubtreeFacts()`: cached for a `CompositeBase` kind, computed
    /// on each call for any other kind (`subtree_facts_with_data`).
    #[must_use]
    pub fn subtree_facts(self) -> SubtreeFacts {
        with_data!(self, |d| subtree_facts_with_data(self, d))
    }

    // Go: ast.go:204 Decorators
    #[must_use]
    pub fn decorators(self) -> NodeSlice {
        let modifiers = self.modifiers();
        if modifiers.is_nil() {
            return NodeSlice::NIL;
        }
        if let Some(cached) = DECORATORS.with(|c| c.borrow().get(&self).copied()) {
            return cached;
        }
        let filtered: Vec<Node> = modifiers
            .nodes()
            .iter()
            .filter(|&m| is_decorator(m))
            .collect();
        // The slice of a factory node belongs to the thread's synthetic
        // arena, which frees it with the node.
        let slice = if !is_synthetic_node(self) {
            NodeSlice::from_nodes(Box::leak(filtered.into_boxed_slice()))
        } else if filtered.is_empty() {
            NodeSlice::NIL
        } else {
            NodeSlice::from_synthetic(new_synthetic_slice(filtered))
        };
        DECORATORS.with(|c| c.borrow_mut().write().insert(self, slice));
        slice
    }

    // Go: ast.go:215 Symbol
    // PORT: Go reads `DeclarationData().Symbol`. The binder only sets the
    // symbol on declaration nodes, so reading the bind data directly is the
    // same.
    // PERF: AST node records, step 2. The symbol of a published store node
    // is in its record (`frozen_store_symbol`).
    #[must_use]
    pub fn symbol(self) -> SymbolId {
        match frozen_store_symbol(self) {
            Some(symbol) => symbol,
            None => self.bind_miss().symbol,
        }
    }

    // Go: ast.go:223 LocalSymbol
    #[must_use]
    pub fn local_symbol(self) -> SymbolId {
        self.bind_extra(|e| e.local_symbol)
    }

    // Go: ast.go:231 Locals
    // PERF: U1 (d). Go `Locals()` is nil when `LocalsContainerData()` is nil.
    // A frozen store node whose kind is no locals container gives nil from
    // the kind table, without the binder data lookup. The binder sets locals
    // only on locals containers (debug_asserts in binder_p1 and binder_p2).
    #[must_use]
    pub fn locals(self) -> SymbolTable {
        if locals_container_variants!(kind_lacks_data! { self, }) {
            debug_assert!(self.bind_extra(|e| e.locals.is_nil()));
            return SymbolTable::NIL;
        }
        self.bind_extra(|e| e.locals)
    }

    /// Go `LocalsContainerData().NextContainer`.
    // PERF: U1 (d), as `locals`.
    #[must_use]
    pub fn next_container(self) -> Node {
        if locals_container_variants!(kind_lacks_data! { self, }) {
            debug_assert!(self.bind_extra(|e| e.next_container.is_nil()));
            return Node::NIL;
        }
        self.bind_extra(|e| e.next_container)
    }

    /// Go `FlowNodeData().FlowNode`.
    // PERF: AST node records, step 2. The flow node of a published store
    // node is its `bind` word, the low half of the id (the flow node is in
    // the same file). astmem1 P3: a node with an extras entry
    // (`BIND_EXTRA`) has it there.
    #[must_use]
    pub fn flow_node(self) -> FlowNodeId {
        match frozen_store_bind_word(self) {
            Some(0) => FlowNodeId::NIL,
            Some(word) if word & BIND_EXTRA == 0 => {
                FlowNodeId((self.0 & !0xffff_ffff) | u64::from(word))
            }
            Some(_) => self.bind_extra(|e| e.flow_node),
            None => self.bind_miss().flow_node,
        }
    }

    /// Go `EndFlowNode` of a function-like or module node.
    #[must_use]
    pub fn end_flow_node(self) -> FlowNodeId {
        self.bind_extra(|e| e.end_flow_node)
    }

    /// Go `ReturnFlowNode` of a function-like node or class static block.
    #[must_use]
    pub fn return_flow_node(self) -> FlowNodeId {
        self.bind_extra(|e| e.return_flow_node)
    }

    /// AST node records, step 2: reads one field of the binder fields of
    /// this node that are not in its record (`NodeBindExtra`). A published
    /// store node reads the index in its `bind` word, and only a node that
    /// has an entry reads its `GoFile` (`FileNodeBind::extra`).
    // PERF: AST node records, step 3. The record and the `GoFile` of a
    // static file come from one block lookup (`frozen_store_bind_and_file`).
    #[inline]
    fn bind_extra<T>(self, field: impl FnOnce(&NodeBindExtra) -> T) -> T {
        match frozen_store_bind_and_file(self) {
            Some((word, go_file)) if word & BIND_EXTRA != 0 => {
                let extra = word & !BIND_EXTRA;
                match go_file {
                    Some(go_file) => field(node_extra_in(go_file, extra)),
                    None => self.bind_extra_slow(extra, field),
                }
            }
            Some(_) => field(&NodeBindExtra::NONE),
            None => field(&NodeBindExtra::of(&self.bind_miss())),
        }
    }

    /// `bind_extra` for a node of a freeable file version, which owns its
    /// `GoFile` (the hot file version first).
    #[cold]
    #[inline(never)]
    fn bind_extra_slow<T>(self, extra: u32, field: impl FnOnce(&NodeBindExtra) -> T) -> T {
        let file = self.file_index();
        if crate::ast::is_hot_file(file) {
            return crate::ast::with_hot_go_file(|go_file| field(node_extra_in(go_file, extra)));
        }
        crate::ast::with_go_file(file, |go_file| field(node_extra_in(go_file, extra)))
    }

    /// AST node records, step 2: the binder data of a node that has no
    /// published record: a factory node (its thread's data), or a store node
    /// of a file that is not bound yet (nil values). Panics as `go_file`
    /// for a store node of a file that is not published.
    #[cold]
    #[inline(never)]
    fn bind_miss(self) -> NodeBindData {
        if is_synthetic_node(self) {
            return synthetic_bind(self);
        }
        crate::ast::with_go_file(self.file_index(), |go_file| {
            debug_assert!(
                go_file.node_bind.get().is_none(),
                "a bound file with no published records"
            );
        });
        NO_BIND
    }
}

/// AST node records, step 2: extras entry `extra` (index + 1) of the bound
/// file `go_file`.
#[inline]
fn node_extra_in(go_file: &GoFile, extra: u32) -> &NodeBindExtra {
    go_file
        .node_bind
        .get()
        .expect("a record with binder extras is in a bound file")
        .extra(extra)
}

impl FlowNodeId {
    /// Go `*FlowNode` dereference. The guard pins a freeable file version
    /// while it lives; copy the fields out rather than keep it.
    // PERF: lsshells M3b. A static file (a static publish) returns its
    // `FileRef::Static` inline, as R134 returned the `&'static FlowNode`;
    // the synthetic flow file and a freeable file version are out of line
    // (`get_flow_slow`). The synthetic id is above every publish, so the
    // static test misses it.
    #[must_use]
    pub fn get_flow(self) -> FileRef<FlowNode> {
        match self.static_flow() {
            Some(data) => FileRef::Static(data),
            None => self.get_flow_slow(),
        }
    }

    /// Go `*FlowNode` dereference for a hot walk: the `'static` borrow of a
    /// static file (a static publish), with no guard, or the borrow of a guard
    /// that `guard` keeps (a synthetic flow node or a freeable file
    /// version). Use: `let mut guard = None; let data = flow.get_flow_in(&mut
    /// guard);`.
    // PERF: lsshells M3b. The checker flow walks read one flow node per
    // step. A `FileRef` there is returned through memory, matched at each
    // read and dropped at each step: `goport -p` ran about 0.3% more
    // instructions in them (lsshells/m3/M3b/prof p7). This gives R134's
    // `&'static FlowNode` for a static file.
    #[inline]
    pub fn get_flow_in(self, guard: &mut Option<FileRef<FlowNode>>) -> &FlowNode {
        match self.static_flow() {
            Some(data) => data,
            None => self.get_flow_in_slow(guard),
        }
    }

    /// `get_flow_in` for a synthetic flow node or a node of a freeable file
    /// version. A guard that pins the version of this flow node already is
    /// pointed at it, with no new `Arc`.
    // PERF: lsshells M3 repair. A flow walk of the edited file reads one flow
    // node per step; a new `FileRef` per step cloned and dropped the `Arc` of
    // the version (two atomic writes per step).
    #[cold]
    #[inline(never)]
    fn get_flow_in_slow(self, guard: &mut Option<FileRef<FlowNode>>) -> &FlowNode {
        let file = self.file_index();
        match guard {
            Some(FileRef::Pinned { version, key, get }) if version.file() == file => {
                *key = self.local_index();
                *get = flow_node_in;
            }
            _ => *guard = Some(self.get_flow_slow()),
        }
        guard.as_deref().expect("the guard is set")
    }

    /// The flow node of a static file (a static publish), or `None` for any
    /// other flow node (synthetic, a freeable file version).
    #[inline]
    fn static_flow(self) -> Option<&'static FlowNode> {
        assert!(self.is_some(), "nil flow node dereference");
        let g = crate::ast::static_go_file(self.file_index())?;
        Some(&flow_nodes_of(g)[self.local_index()])
    }

    /// `get_flow` for a synthetic flow node or a node of a freeable file
    /// version.
    #[cold]
    #[inline(never)]
    fn get_flow_slow(self) -> FileRef<FlowNode> {
        if self.file_index() == crate::checker::SYNTHETIC_FLOW_FILE {
            return FileRef::Static(crate::checker::synthetic_flow(self));
        }
        match crate::ast::go_file(self.file_index()) {
            FileRef::Static(g) => FileRef::Static(&flow_nodes_of(g)[self.local_index()]),
            FileRef::Pinned { version, .. } => FileRef::Pinned {
                version,
                key: self.local_index(),
                get: flow_node_in,
            },
        }
    }
}

/// Flow node `index` of freeable file version `version` (the getter of a
/// pinned flow node guard).
fn flow_node_in(version: &FileVersion, index: usize) -> &FlowNode {
    &flow_nodes_of(version.go_file())[index]
}

/// Go `file.AsSourceFile()` fields that the parser and program set. The
/// guard pins a freeable file version while it lives (see `ast::go_file`).
#[must_use]
pub fn source_file_info(file: Node) -> FileRef<crate::program::SourceFileInfo> {
    let file = published_source_file(file);
    crate::ast::file_version::go_file_ref!(file.file_index(), 0, |g, _key| g.info)
}

/// Runs `read` on `source_file_info(file)` with no guard: a freeable file
/// version is pinned only while `read` runs (see `ast::with_go_file`).
// PERF: lsshells M3 repair. A `FileRef` guard of a freeable file version
// clones and drops its `Arc` (two atomic writes). The checker reads one
// field of the info of the edited file many times per edit
// (`is_external_module`, `is_declaration_file`, ...), so those readers use
// this.
#[inline]
pub fn with_source_file_info<R>(
    file: Node,
    read: impl FnOnce(&crate::program::SourceFileInfo) -> R,
) -> R {
    let file = published_source_file(file);
    crate::ast::with_go_file(file.file_index(), |g| read(&g.info))
}

/// The published SourceFile whose info `source_file_info(file)` reads:
/// `file`, or for a factory SourceFile the parsed file with its path.
// PORT: Go SourceFile.copyFrom copies the parsed fields onto a factory
// SourceFile. Here a factory SourceFile reads the parsed file with its path.
fn published_source_file(file: Node) -> Node {
    if is_synthetic_node(file) {
        let path = with_synthetic_source_file(file, |d| d.path.clone());
        return crate::program::get_source_file_by_path(&path);
    }
    file
}

/// Go `file.LanguageVariant`.
// PORT: a file that is not published yet (a parse outside a program, as the
// format tests make) has no `GoFile`; its store keeps the parser value.
#[must_use]
pub fn source_file_language_variant(file: Node) -> LanguageVariant {
    if !is_synthetic_node(file) && is_file_store_before_program(file.file_index()) {
        return file_store_language_variant(file.file_index());
    }
    with_source_file_info(file, |info| info.language_variant)
}

/// Go `file.AsSourceFile()` fields that the binder sets. The guard pins a
/// freeable file version while it lives (see `ast::go_file`).
#[must_use]
pub fn file_bind_data(file: Node) -> FileRef<FileBindData> {
    crate::ast::file_version::go_file_ref!(file.file_index(), 0, |g, _key| *g
        .file_bind
        .get()
        .expect("source file is not bound"))
}

// ──────────────────────────────────────────────────────────────────────
// Node field accessors (Go methods on *Node)
// ──────────────────────────────────────────────────────────────────────

/// Go `AsCaseOrDefaultClause().Expression`. Nil for a default clause.
// PORT: astdata stores a required expression id on both clause kinds. Go
// leaves it nil for `default:`, so the kind decides.
fn case_expression(n: Node, file: usize, id: crate::astdata::NodeId) -> Node {
    if n.kind() == SyntaxKind::CaseClause {
        req(file, id)
    } else {
        Node::NIL
    }
}

/// Caches a string that Go builds on each call, so `text()` can return
/// `&'static str`. The text is interned (`Name`), so the texts of the
/// factory nodes of released programs are kept once, not per node.
fn joined_text(n: Node, build: impl FnOnce() -> String) -> &'static str {
    if let Some(s) = JOINED_TEXT.with(|c| c.borrow().get(&n).copied()) {
        return s;
    }
    let s: &'static str = Name::from(build()).as_str();
    JOINED_TEXT.with(|c| c.borrow_mut().write().insert(n, s));
    s
}

impl Node {
    // Go: ast.go:239 Body
    // PORT: also covers ClassStaticBlockDeclaration, whose Go `Body` field
    // has no generated accessor.
    #[must_use]
    pub fn body(self) -> Node {
        by_child_ids!(
            self,
            Node::NIL,
            [
                FunctionDeclaration,
                MethodDeclaration,
                GetAccessorDeclaration,
                SetAccessorDeclaration,
                ConstructorDeclaration,
                ModuleDeclaration,
            ] => |ids, d| ids.opt(d.body),
            [
                FunctionExpression,
                ArrowFunction,
                ClassStaticBlockDeclaration,
            ] => |ids, d| ids.node(d.body),
        )
    }

    // Go: ast.go:247 Text
    // PORT: also covers JsxText, whose Go `Text` field has no generated
    // accessor. Other kinds give "" instead of a panic.
    #[must_use]
    pub fn text(self) -> &'static str {
        let Some(data) = static_ast_node(self) else {
            return self.scoped_node_text();
        };
        match data {
            NodeData::Identifier(d) => name_node_text(self, &d.text),
            NodeData::PrivateIdentifier(d) => name_node_text(self, &d.text),
            NodeData::StringLiteral(d) => &d.text,
            NodeData::NumericLiteral(d) => &d.text,
            NodeData::BigIntLiteral(d) => &d.text,
            NodeData::MetaProperty(_) => self.name().text(),
            NodeData::NoSubstitutionTemplateLiteral(d) => &d.text,
            NodeData::TemplateHead(d) => &d.text,
            NodeData::TemplateMiddle(d) => &d.text,
            NodeData::TemplateTail(d) => &d.text,
            NodeData::JsxNamespacedName(d) => {
                let file = self.file_index();
                joined_text(self, || {
                    format!(
                        "{}:{}",
                        req(file, d.namespace).text(),
                        req(file, d.name).text()
                    )
                })
            }
            NodeData::RegularExpressionLiteral(d) => &d.text,
            NodeData::JsDocText(d) => joined_text(self, || d.text.concat()),
            NodeData::JsDocLink(d) => joined_text(self, || d.text.concat()),
            NodeData::JsDocLinkCode(d) => joined_text(self, || d.text.concat()),
            NodeData::JsDocLinkPlain(d) => joined_text(self, || d.text.concat()),
            NodeData::JsxText(d) => &d.text,
            _ => "",
        }
    }

    /// `text` of a node with no `'static` node, whose data is read in a
    /// scope (see `synthetic_text`): a factory node, or a node that a
    /// freeable parse owns (lsshells M3c).
    // PERF: lsshells M3c. The text of a node of a published freeable file
    // version is interned once per thread and node (`JOINED_TEXT`, which
    // forgets the nodes of dead versions), not at each read: the checker
    // reads the texts of the edited file many times.
    #[cold]
    #[inline(never)]
    fn scoped_node_text(self) -> &'static str {
        // An Identifier or PrivateIdentifier of a published store has its
        // text in the kids of its node shell (`name_node_text`), with
        // no data read.
        if let Some(name) = frozen_store_text_name(self) {
            return name.as_str();
        }
        let synthetic = is_synthetic_node(self);
        if !synthetic && let Some(text) = JOINED_TEXT.with(|c| c.borrow().get(&self).copied()) {
            return text;
        }
        let file = self.file_index();
        let interned = |text: &str| -> &'static str { Name::from(text).as_str() };
        let text = read_scoped_ast_node(self, |data| match data {
            // A store Identifier keeps its text in its name word
            // (`name_node_text`); a synthetic one in its data.
            NodeData::Identifier(d) => scoped_name_text(self, &d.text),
            NodeData::PrivateIdentifier(d) => scoped_name_text(self, &d.text),
            NodeData::StringLiteral(d) => interned(d.text.as_str()),
            NodeData::NumericLiteral(d) => interned(d.text.as_str()),
            NodeData::BigIntLiteral(d) => interned(d.text.as_str()),
            NodeData::MetaProperty(_) => self.name().text(),
            NodeData::NoSubstitutionTemplateLiteral(d) => interned(d.text.as_str()),
            NodeData::TemplateHead(d) => interned(d.text.as_str()),
            NodeData::TemplateMiddle(d) => interned(d.text.as_str()),
            NodeData::TemplateTail(d) => interned(d.text.as_str()),
            NodeData::JsxNamespacedName(d) => joined_text(self, || {
                format!(
                    "{}:{}",
                    req(file, d.namespace).text(),
                    req(file, d.name).text()
                )
            }),
            NodeData::RegularExpressionLiteral(d) => interned(d.text.as_str()),
            NodeData::JsDocText(d) => joined_text(self, || d.text.concat()),
            NodeData::JsDocLink(d) => joined_text(self, || d.text.concat()),
            NodeData::JsDocLinkCode(d) => joined_text(self, || d.text.concat()),
            NodeData::JsDocLinkPlain(d) => joined_text(self, || d.text.concat()),
            NodeData::JsxText(d) => interned(d.text.as_str()),
            _ => "",
        });
        // A published file does not change, so its text stays. A store that
        // is still parsed can get new node data (`replace_node_data`).
        if !synthetic && is_published(file) {
            JOINED_TEXT.with(|c| c.borrow_mut().write().insert(self, text));
        }
        text
    }

    /// `Name::from(self.text())`: the interned Go `node.Text()`.
    // PERF: U1 (a). A frozen Identifier or PrivateIdentifier store node reads
    // the name interned when its slot was made (`frozen_store_text_name`),
    // without its node data and without an intern.
    #[inline]
    #[must_use]
    pub fn text_name(self) -> Name {
        if let Some(name) = frozen_store_text_name(self) {
            debug_assert_eq!(name, Name::from(self.text()));
            return name;
        }
        Name::from(self.text())
    }

    /// Go `node.Text() == name`.
    // PERF: U1 (a). A frozen Identifier or PrivateIdentifier store node
    // compares name ids (`frozen_store_text_name`). Other nodes compare the
    // text, so they do not intern it.
    #[inline]
    #[must_use]
    pub fn text_is(self, name: &Name) -> bool {
        match frozen_store_text_name(self) {
            Some(own) => {
                debug_assert_eq!(own == *name, self.text() == name.as_str());
                own == *name
            }
            None => self.text() == name.as_str(),
        }
    }

    /// Go `scanner.GetIdentifierToken(node.Text()) != ast.KindIdentifier`:
    /// the text is a keyword.
    // PERF: U1 (a). A frozen Identifier or PrivateIdentifier store node reads
    // a record bit set when its slot was made
    // (`frozen_store_text_is_keyword`).
    #[inline]
    #[must_use]
    pub fn text_is_keyword(self) -> bool {
        if let Some(is_keyword) = frozen_store_text_is_keyword(self) {
            debug_assert_eq!(
                is_keyword,
                get_identifier_token(self.text()) != SyntaxKind::Identifier
            );
            return is_keyword;
        }
        get_identifier_token(self.text()) != SyntaxKind::Identifier
    }

    // Go: ast.go:285 Expression
    // PERF: U4 (CH6). A published store node reads its expression child from
    // the store column (`frozen_store_child`), as `Node::name` does.
    #[must_use]
    pub fn expression(self) -> Node {
        match frozen_store_child(self, StoreChild::Expression) {
            Some(expression) => {
                debug_assert_eq!(expression, self.expression_data(), "U4 expression column");
                expression
            }
            None => self.expression_data(),
        }
    }

    /// Go `Expression()` from the node data (`expression_arms!`).
    fn expression_data(self) -> Node {
        expression_arms!(
            by_data!(
                self,
                Node::NIL,
                [CaseOrDefaultClause] => |f, d| case_expression(self, f, d.expression),
                // PORT: a QualifiedName that Go parses as a PropertyAccessExpression.
                [QualifiedName] => |f, d| if self.kind() == SyntaxKind::PropertyAccessExpression {
                    req(f, d.left)
                } else {
                    Node::NIL
                },
            ),
            opt,
            req
        )
    }

    // Go: ast.go:361 RawText
    // PORT: also covers NoSubstitutionTemplateLiteral, whose Go `RawText`
    // field has no generated accessor.
    #[must_use]
    pub fn raw_text(self) -> &'static str {
        fn raw_text_of(d: NodeData<'_>) -> Option<&str> {
            match d {
                NodeData::TemplateHead(d) => Some(&d.raw_text),
                NodeData::TemplateMiddle(d) => Some(&d.raw_text),
                NodeData::TemplateTail(d) => Some(&d.raw_text),
                NodeData::NoSubstitutionTemplateLiteral(d) => Some(&d.raw_text),
                _ => None,
            }
        }
        match static_ast_node(self) {
            Some(data) => raw_text_of(data).unwrap_or(""),
            None => synthetic_text(self, raw_text_of),
        }
    }

    // Go: ast.go:451 ArgumentList
    #[must_use]
    pub fn argument_list(self) -> NodeList {
        list_of!(self, |d| match d {
            NodeData::CallExpression(d) => Some(sel_field!(req, d.arguments)),
            NodeData::NewExpression(d) => Some(sel_field!(opt, d.arguments)),
            _ => None,
        })
        .unwrap_or(NodeList::NIL)
    }

    // Go: ast.go:461 Arguments
    #[must_use]
    pub fn arguments(self) -> NodeSlice {
        self.argument_list().nodes()
    }

    // Go: ast.go:469 TypeArgumentList
    // PERF: C2. A published store node whose data has no list gives nil from
    // the store column (`frozen_store_lacks_type_arguments`), without its
    // node data. A node with a list reads it from the data.
    #[must_use]
    pub fn type_argument_list(self) -> NodeList {
        if frozen_store_lacks_type_arguments(self) {
            debug_assert!(
                matches!(self.type_argument_list_data().0, ListRef::Nil),
                "C2 type arguments column"
            );
            return NodeList::NIL;
        }
        self.type_argument_list_data()
    }

    /// Go `TypeArgumentList()` from the node data (`type_arguments_arms!`).
    // The C2 column above already answers for a kind without the field, so
    // this has no `kind_lacks_data!` test.
    fn type_argument_list_data(self) -> NodeList {
        list_of!(self, |d| type_arguments_arms!(
            match_data!(0, d, None,),
            sel_type_arguments
        ))
        .unwrap_or(NodeList::NIL)
    }

    // Go: ast.go:493 TypeArguments
    #[must_use]
    pub fn type_arguments(self) -> NodeSlice {
        self.type_argument_list().nodes()
    }

    // Go: ast.go:501 TypeParameterList
    #[must_use]
    pub fn type_parameter_list(self) -> NodeList {
        list_by_data!(
            self,
            list_of,
            NodeList::NIL,
            [JsDocTemplateTag] => req type_parameters,
            [
                ClassDeclaration,
                ClassExpression,
                InterfaceDeclaration,
                TypeAliasDeclaration,
                ArrowFunction,
                CallSignatureDeclaration,
                ConstructSignatureDeclaration,
                ConstructorDeclaration,
                FunctionDeclaration,
                FunctionExpression,
                GetAccessorDeclaration,
                SetAccessorDeclaration,
                IndexSignatureDeclaration,
                JsDocSignature,
                MethodDeclaration,
                MethodSignatureDeclaration,
                FunctionTypeNode,
                ConstructorTypeNode,
            ] => opt type_parameters,
        )
    }

    // Go: ast.go:522 TypeParameters
    #[must_use]
    pub fn type_parameters(self) -> NodeSlice {
        self.type_parameter_list().nodes()
    }

    // Go: ast.go:530 MemberList
    #[must_use]
    pub fn member_list(self) -> NodeList {
        let list = list_by_data!(
            self,
            list_of,
            NodeList::NIL,
            [MappedTypeNode] => opt members,
            [
                ClassDeclaration,
                ClassExpression,
                InterfaceDeclaration,
                EnumDeclaration,
                TypeLiteralNode,
            ] => req members,
        );
        list
    }

    // Go: ast.go:548 Members
    #[must_use]
    pub fn members(self) -> NodeSlice {
        self.member_list().nodes()
    }

    // Go: ast.go:556 StatementList
    #[must_use]
    pub fn statement_list(self) -> NodeList {
        list_by_data!(
            self,
            list_of,
            NodeList::NIL,
            [SourceFile, Block, ModuleBlock, CaseOrDefaultClause] => req statements,
        )
    }

    // Go: ast.go:570 Statements
    #[must_use]
    pub fn statements(self) -> NodeSlice {
        self.statement_list().nodes()
    }

    // Go: ast.go:578 CanHaveStatements
    #[must_use]
    pub fn can_have_statements(self) -> bool {
        matches!(
            self.kind(),
            SyntaxKind::SourceFile
                | SyntaxKind::Block
                | SyntaxKind::ModuleBlock
                | SyntaxKind::CaseClause
                | SyntaxKind::DefaultClause
        )
    }

    // Go: ast.go:587 ModifierFlags
    // PERF: U1 (b). A frozen store node reads its flags from the store column
    // (`frozen_store_modifier_flags`), without its node data and list.
    #[must_use]
    pub fn modifier_flags(self) -> ModifierFlags {
        if let Some(flags) = frozen_store_modifier_flags(self) {
            debug_assert_eq!(flags, self.modifiers().modifier_flags());
            return flags;
        }
        self.modifiers().modifier_flags()
    }

    // Go: ast.go:595 ModifierNodes
    #[must_use]
    pub fn modifier_nodes(self) -> NodeSlice {
        self.modifiers().nodes()
    }

    // Go: ast.go:603 Type
    // PORT: also covers JsDocVariadicType, whose Go `Type` field has no
    // generated accessor.
    // PERF: C2. A published store node reads its type child from the store
    // column (`frozen_store_child`), as `Node::name` does.
    #[must_use]
    pub fn type_(self) -> Node {
        match frozen_store_child(self, StoreChild::Type) {
            Some(type_) => {
                debug_assert_eq!(type_, self.type_data(), "C2 type column");
                type_
            }
            None => self.type_data(),
        }
    }

    /// Go `Type()` from the node data (`type_arms!`).
    fn type_data(self) -> Node {
        type_arms!(by_data!(self, Node::NIL,), opt, req)
    }

    // Go: ast.go:725 Initializer
    // PERF: C2, as `Node::type_`. A node with both a type and an
    // initializer keeps only the type in the column; its initializer reads
    // take the node data.
    #[must_use]
    pub fn initializer(self) -> Node {
        match frozen_store_child(self, StoreChild::Initializer) {
            Some(initializer) => {
                debug_assert_eq!(
                    initializer,
                    self.initializer_data(),
                    "C2 initializer column"
                );
                initializer
            }
            None => self.initializer_data(),
        }
    }

    /// `Node::initializer` on `d`, the data of this node that the caller
    /// already loaded with `parsed_node_data` (see `data_accessor!`).
    #[must_use]
    pub fn initializer_in(self, d: LoadedData) -> Node {
        match frozen_store_child(self, StoreChild::Initializer) {
            Some(initializer) => {
                debug_assert_eq!(
                    initializer,
                    self.initializer_data_in(d),
                    "C2 initializer column"
                );
                initializer
            }
            None => self.initializer_data_in(d),
        }
    }

    // `initializer_data` and `initializer_data_in`: Go `Initializer()` from
    // the node data.
    initializer_arms!(initializer_data_accessor!(), opt, req);

    // Go: ast.go:779 TagName
    #[must_use]
    pub fn tag_name(self) -> Node {
        by_child_ids!(
            self,
            Node::NIL,
            [
                JsxOpeningElement,
                JsxClosingElement,
                JsxSelfClosingElement,
                JsDocUnknownTag,
                JsDocAugmentsTag,
                JsDocImplementsTag,
                JsDocDeprecatedTag,
                JsDocPublicTag,
                JsDocPrivateTag,
                JsDocProtectedTag,
                JsDocReadonlyTag,
                JsDocOverrideTag,
                JsDocCallbackTag,
                JsDocOverloadTag,
                JsDocParameterOrPropertyTag,
                JsDocReturnTag,
                JsDocThisTag,
                JsDocTypeTag,
                JsDocTemplateTag,
                JsDocTypedefTag,
                JsDocSeeTag,
                JsDocSatisfiesTag,
                JsDocThrowsTag,
                JsDocImportTag,
            ] => |ids, d| ids.node(d.tag_name),
        )
    }

    // Go: ast.go:833 PropertyName
    #[must_use]
    pub fn property_name(self) -> Node {
        by_child_ids!(
            self,
            Node::NIL,
            [ImportSpecifier, ExportSpecifier, BindingElement] => |ids, d| ids.opt(d.property_name),
        )
    }

    // Go: ast.go:845 PropertyNameOrName
    #[must_use]
    pub fn property_name_or_name(self) -> Node {
        let name = self.property_name();
        if name.is_nil() {
            return self.name();
        }
        name
    }

    // Go: ast.go:853 IsTypeOnly
    #[must_use]
    pub fn is_type_only(self) -> bool {
        with_data!(self, |d| match d {
            NodeData::ImportEqualsDeclaration(d) => d.is_type_only,
            NodeData::ImportSpecifier(d) => d.is_type_only,
            NodeData::ImportClause(d) => d.phase_modifier == Some(SyntaxKind::TypeKeyword),
            NodeData::ExportDeclaration(d) => d.is_type_only,
            NodeData::ExportSpecifier(d) => d.is_type_only,
            _ => false,
        })
    }

    // Go: ast.go:870 CommentList
    #[must_use]
    pub fn comment_list(self) -> NodeList {
        list_by_data!(
            self,
            list_of,
            NodeList::NIL,
            [JsDoc] => req comment,
            [
                JsDocUnknownTag,
                JsDocAugmentsTag,
                JsDocImplementsTag,
                JsDocDeprecatedTag,
                JsDocPublicTag,
                JsDocPrivateTag,
                JsDocProtectedTag,
                JsDocReadonlyTag,
                JsDocOverrideTag,
                JsDocCallbackTag,
                JsDocOverloadTag,
                JsDocParameterOrPropertyTag,
                JsDocReturnTag,
                JsDocThisTag,
                JsDocTypeTag,
                JsDocTemplateTag,
                JsDocTypedefTag,
                JsDocSeeTag,
                JsDocSatisfiesTag,
                JsDocThrowsTag,
                JsDocImportTag,
            ] => opt comment,
        )
    }

    // Go: ast.go:920 Comments
    #[must_use]
    pub fn comments(self) -> NodeSlice {
        self.comment_list().nodes()
    }

    // Go: ast.go:928 Label
    #[must_use]
    pub fn label(self) -> Node {
        let f = self.file_index();
        with_data!(self, |d| match d {
            NodeData::LabeledStatement(d) => req(f, d.label),
            NodeData::BreakStatement(d) => opt(f, d.label),
            NodeData::ContinueStatement(d) => opt(f, d.label),
            _ => Node::NIL,
        })
    }

    // Go: ast.go:940 Attributes
    // PORT: also covers the `Attributes` fields of ImportDeclaration,
    // ExportDeclaration, ImportTypeNode and JSDocImportTag, which have no
    // generated accessor.
    #[must_use]
    pub fn attributes(self) -> Node {
        by_child_ids!(
            self,
            Node::NIL,
            [JsxOpeningElement, JsxSelfClosingElement] => |ids, d| ids.node(d.attributes),
            [
                ModuleDeclaration,
                ImportDeclaration,
                ExportDeclaration,
                ImportTypeNode,
                JsDocImportTag,
            ] => |ids, d| ids.opt(d.attributes),
        )
    }

    // Go: ast.go:952 Children
    #[must_use]
    pub fn children(self) -> NodeList {
        list_by_data!(
            self,
            list_of,
            NodeList::NIL,
            [JsxElement, JsxFragment] => req children,
        )
    }

    // Go: ast.go:962 ModuleSpecifier
    #[must_use]
    pub fn module_specifier(self) -> Node {
        let f = self.file_index();
        with_data!(self, |d| match d {
            NodeData::ImportDeclaration(d) => req(f, d.module_specifier),
            NodeData::ExportDeclaration(d) => opt(f, d.module_specifier),
            NodeData::JsDocImportTag(d) => req(f, d.module_specifier),
            _ => Node::NIL,
        })
    }

    // Go: ast.go:974 ImportClause
    #[must_use]
    pub fn import_clause(self) -> Node {
        by_child_ids!(
            self,
            Node::NIL,
            [ImportDeclaration, JsDocImportTag] => |ids, d| ids.opt(d.import_clause),
        )
    }

    // Go: ast.go:984 Statement
    #[must_use]
    pub fn statement(self) -> Node {
        by_child_ids!(
            self,
            Node::NIL,
            [
                DoStatement,
                WhileStatement,
                ForStatement,
                ForInOrOfStatement,
                WithStatement,
                LabeledStatement,
            ] => |ids, d| ids.node(d.statement),
        )
    }

    // Go: ast.go:1002 PropertyList
    #[must_use]
    pub fn property_list(self) -> NodeList {
        list_by_data!(
            self,
            list_of,
            NodeList::NIL,
            [ObjectLiteralExpression, JsxAttributes] => req properties,
        )
    }

    // Go: ast_generated.go ImportAttributes.Attributes
    #[must_use]
    pub fn attribute_list(self) -> NodeList {
        list_by_data!(
            self,
            list_of,
            NodeList::NIL,
            [ImportAttributes] => req attributes,
        )
    }

    // Go: ast.go:1012 Properties
    #[must_use]
    pub fn properties(self) -> NodeSlice {
        self.property_list().nodes()
    }

    // Go: ast.go:1020 ElementList
    #[must_use]
    pub fn element_list(self) -> NodeList {
        list_by_data!(
            self,
            list_of,
            NodeList::NIL,
            [
                NamedImports,
                NamedExports,
                BindingPattern,
                ArrayLiteralExpression,
                TupleTypeNode,
            ] => req elements,
        )
    }

    // Go: ast.go:1036 Elements
    #[must_use]
    pub fn elements(self) -> NodeSlice {
        self.element_list().nodes()
    }

    // Go: ast.go:1044 PostfixToken
    // PERF: U4 (bind A). A published store node reads the token from the store
    // column (`frozen_store_child`), as `Node::name` does.
    #[must_use]
    pub fn postfix_token(self) -> Node {
        match frozen_store_child(self, StoreChild::PostfixToken) {
            Some(token) => {
                debug_assert_eq!(token, self.postfix_token_data(), "U4 postfix column");
                token
            }
            None => self.postfix_token_data(),
        }
    }

    /// `Node::postfix_token` on `d`, the data of this node that the caller
    /// already loaded with `parsed_node_data` (see `data_accessor!`).
    #[must_use]
    pub fn postfix_token_in(self, d: LoadedData) -> Node {
        match frozen_store_child(self, StoreChild::PostfixToken) {
            Some(token) => {
                debug_assert_eq!(token, self.postfix_token_data_in(d), "U4 postfix column");
                token
            }
            None => self.postfix_token_data_in(d),
        }
    }

    // `postfix_token_data` and `postfix_token_data_in`: Go `PostfixToken()`
    // from the node data.
    postfix_arms!(postfix_token_data_accessor!(), opt);

    // Go: ast.go:1068 QuestionToken
    // PERF: U4 (bind A), as `Node::postfix_token`.
    #[must_use]
    pub fn question_token(self) -> Node {
        match frozen_store_child(self, StoreChild::QuestionToken) {
            Some(token) => {
                debug_assert_eq!(token, self.question_token_data(), "U4 question column");
                token
            }
            None => self.question_token_data(),
        }
    }

    /// `Node::question_token` on `d`, the data of this node that the caller
    /// already loaded with `parsed_node_data` (see `data_accessor!`).
    #[must_use]
    pub fn question_token_in(self, d: LoadedData) -> Node {
        match frozen_store_child(self, StoreChild::QuestionToken) {
            Some(token) => {
                debug_assert_eq!(token, self.question_token_data_in(d), "U4 question column");
                token
            }
            None => self.question_token_data_in(d),
        }
    }

    /// Go `QuestionToken()` from the node data.
    fn question_token_data(self) -> Node {
        match self.own_question_token() {
            Some(token) => token,
            None => question_of_postfix(self.postfix_token_data()),
        }
    }

    /// `question_token_data` on `d` (see `data_accessor!`).
    fn question_token_data_in(self, d: LoadedData) -> Node {
        match self.own_question_token_in(d) {
            Some(token) => token,
            None => question_of_postfix(self.postfix_token_data_in(d)),
        }
    }

    // `own_question_token` and `own_question_token_in`: the first step of Go
    // `QuestionToken`.
    question_arms!(own_question_token_accessor!(), some_opt, some_req);

    // Go: ast.go:1086 QuestionDotToken
    #[must_use]
    pub fn question_dot_token(self) -> Node {
        by_child_ids!(
            self,
            Node::NIL,
            [
                ElementAccessExpression,
                PropertyAccessExpression,
                CallExpression,
                TaggedTemplateExpression,
            ] => |ids, d| ids.opt(d.question_dot_token),
        )
    }

    // Go: ast.go:1100 TypeExpression
    // PORT: also covers the JSDoc `@this` and `@overload` tags, whose Go
    // `TypeExpression` fields have no generated accessor.
    #[must_use]
    pub fn type_expression(self) -> Node {
        by_child_ids!(
            self,
            Node::NIL,
            [
                JsDocParameterOrPropertyTag,
                JsDocReturnTag,
                JsDocTypedefTag,
                JsDocThrowsTag,
            ] => |ids, d| ids.opt(d.type_expression),
            [
                JsDocTypeTag,
                JsDocCallbackTag,
                JsDocSatisfiesTag,
                JsDocThisTag,
                JsDocOverloadTag,
            ] => |ids, d| ids.node(d.type_expression),
        )
    }

    // Go: ast.go:1120 ClassName
    #[must_use]
    pub fn class_name(self) -> Node {
        by_child_ids!(
            self,
            Node::NIL,
            [JsDocAugmentsTag, JsDocImplementsTag] => |ids, d| ids.node(d.class_name),
        )
    }

    // Go: ast.go:1132 Contains
    /// Reports whether `self` contains `descendant` by walking up the parent
    /// links. Panics if a non-SourceFile ancestor has no parent.
    #[must_use]
    pub fn contains(self, mut descendant: Node) -> bool {
        while descendant.is_some() {
            if descendant == self {
                return true;
            }
            let parent = descendant.parent();
            if parent.is_nil() && !is_source_file(descendant) {
                panic!("descendant is not parented");
            }
            descendant = parent;
        }
        false
    }
}

// ──────────────────────────────────────────────────────────────────────
// ForEachChild / IterChildren
// ──────────────────────────────────────────────────────────────────────

impl Node {
    // Go: ast_generated.go:8769 ForEachChild
    /// Calls `v` on each child in Go source order. Stops and returns true
    /// when `v` returns true.
    pub fn for_each_child(self, mut v: impl FnMut(Node) -> bool) -> bool {
        for_each_child_dyn(self, &mut v)
    }

    // Go: ast.go:191 IterChildren
    /// The children that `for_each_child` visits, in the same order.
    #[must_use]
    pub fn iter_children(self) -> std::vec::IntoIter<Node> {
        let mut out = Vec::new();
        for_each_child_dyn(self, &mut |c| {
            out.push(c);
            false
        });
        out.into_iter()
    }
}

// Go: ast_generated.go ForEachChild (one method per node struct)
// PORT: the per-struct Go methods are merged into one match. The order of
// each arm follows the generated Go code.
fn for_each_child_dyn(n: Node, v: &mut dyn FnMut(Node) -> bool) -> bool {
    // Go `switch n.Kind`: a kind with no case returns false and never reads
    // `n.data`.
    // PERF: noderef1 W1. 35 to 48% of the walk entries are tokens and
    // identifiers (getsym1 report). Their kind is one load from the block
    // that the node data read needs anyway.
    if let Some(f) = frozen_store_node(n) {
        let kind = f.kind();
        if has_no_child_fields(kind) {
            debug_assert!(
                f.data().is_none_or(|d| {
                    let mut fields = 0;
                    for_each_child_field(kind, n, d, |_| fields += 1);
                    fields == 0
                }),
                "{kind:?} has child fields"
            );
            return false;
        }
        // PERF: perfP1 item 1 (perfscan2). A node whose child chain is
        // known (R2-5 `SlotLinks`, Go `ForEachChild` order; the binder walks
        // it) walks the chain: two loads per child from one column, no
        // node data match. Debug builds compare it with the data walk.
        if let Some(children) = f.children() {
            debug_assert!(
                children.eq(children_by_data(n)),
                "child links differ from the data walk of {kind:?}"
            );
            for child in children {
                if v(child) {
                    return true;
                }
            }
            return false;
        }
        if let Some(data) = f.data() {
            let mut visit = FrozenChildVisit {
                kind,
                id_map: f.ids(),
                lists: f.lists(),
                file: n.file_index(),
                v,
            };
            return walk_children(data, &mut visit);
        }
    }
    for_each_child_impl(n, v, None)
}

/// The children of `n` from its node data (`for_each_child_impl`), not
/// from its child chain: the debug check of the chain walk.
fn children_by_data(n: Node) -> std::vec::IntoIter<Node> {
    let mut out = Vec::new();
    for_each_child_impl(
        n,
        &mut |c| {
            out.push(c);
            false
        },
        None,
    );
    out.into_iter()
}

/// A list slot seen by `for_each_child_and_lists`: the list and whether it
/// is a modifier list.
pub type ListHook<'a> = &'a mut dyn FnMut(NodeList, bool);

impl Node {
    /// `for_each_child` that also reports each non-nil NodeList and
    /// ModifierList slot to `lists` before its nodes are visited. Used by the
    /// `astdump` parity tool.
    pub fn for_each_child_and_lists(
        self,
        v: &mut dyn FnMut(Node) -> bool,
        lists: ListHook,
    ) -> bool {
        for_each_child_impl(self, v, Some(lists))
    }
}

/// The walk of `for_each_child_dyn` for a node that is not a published
/// store node with node data, and the walk of `for_each_child_and_lists`.
fn for_each_child_impl(n: Node, v: &mut dyn FnMut(Node) -> bool, lists: Option<ListHook>) -> bool {
    let Some(data) = static_ast_node(n) else {
        return for_each_scoped_child(n, v, lists);
    };
    let mut visit = NodeChildVisit {
        n,
        file: n.file_index(),
        v,
        lists,
    };
    walk_children(data, &mut visit)
}

/// `for_each_child_impl` for a node with no `'static` node (a factory node,
/// or a node that a freeable parse owns, lsshells M3c), whose data is read
/// in a scope (`ScopedChildVisit`). The node is held, so `v` can make and
/// change nodes.
#[cold]
#[inline(never)]
fn for_each_scoped_child(
    n: Node,
    v: &mut dyn FnMut(Node) -> bool,
    lists: Option<ListHook>,
) -> bool {
    with_scoped_ast_node(n, |data| {
        walk_children(
            data,
            &mut ScopedChildVisit {
                n,
                file: n.file_index(),
                v,
                lists,
            },
        )
    })
}

/// R3-2: calls `v` with the slot index of each child id in `data`, the
/// data of a store node of Go kind `kind`, in Go `ForEachChild` order, and
/// stops when `v` returns true. It reads only the node data: no store and
/// no node read. A nil single child field is skipped, as Go `visit` skips
/// nil; list entries are all passed, so `v` sees slot 0 (nil) for a nil
/// list entry. `v` must resolve each index in the store of `node`: a node
/// slot is that node, and any other slot (nil, alias) is not.
// PERF: R3-2. The parser sets the parents of the children of each node it
// finishes (`set_parent_in_store_children`). The generic walk read the
// data and every child through the node reads (`static_ast_node_slow`,
// `Node::new_slow`), a store lookup each.
pub(crate) fn for_each_store_child_id(
    kind: SyntaxKind,
    data: NodeData<'_>,
    lists: ListIds<'_>,
    v: impl StoreChildSink,
) -> bool {
    walk_children(data, &mut StoreChildIds { kind, lists, v })
}

/// R3-2: what `for_each_store_child_id` calls with the slot index of each
/// child (`child` returns true to stop the walk). A closure is one.
// PERF: storefast1. A sink whose `child` is `#[inline(always)]` (the
// parser's parent writes, `ParentLinks` in store.rs) is inlined into each
// arm of `walk_children`, as the generic walk is (`NodeChildVisit`). A
// closure stays a call per child.
pub(crate) trait StoreChildSink {
    fn child(&mut self, index: u32) -> bool;
}

impl<F: FnMut(u32) -> bool> StoreChildSink for F {
    #[inline(always)]
    fn child(&mut self, index: u32) -> bool {
        self(index)
    }
}

/// R3-2: the fields that one arm of `walk_children` visits, in Go
/// `ForEachChild` order. Each method returns true to stop the walk. The
/// generic walk (`NodeChildVisit`) turns the ids into `Node` handles; the
/// parser's walk over a store node (`StoreChildIds`) keeps the slot
/// indexes. One match serves both, so they visit the same fields. `'d` is
/// the lifetime of the node data: `'static` for a parsed node of a static
/// parse, a read scope for a factory node and for a node that a freeable
/// parse owns (`ScopedChildVisit`, `StoreChildIds`).
trait ChildVisit<'d> {
    /// A required child field (Go `visit`).
    fn node(&mut self, id: crate::astdata::NodeId) -> bool;
    /// An optional child field (Go `visit`).
    fn opt(&mut self, id: Option<crate::astdata::NodeId>) -> bool;
    /// A required list field (Go `visitNodeList`).
    fn list(&mut self, l: &'d crate::astdata::NodeList) -> bool;
    /// An optional list field (Go `visitNodeList`).
    fn opt_list(&mut self, l: &'d Option<crate::astdata::NodeList>) -> bool;
    /// A modifier list field (Go `visitModifiers`).
    fn mods(&mut self, m: &'d Option<crate::astdata::ModifierList>) -> bool;
    /// A Go `[]*Node` field with no list (Go `visitNodes`).
    fn ids(&mut self, ids: &'d [crate::astdata::NodeId]) -> bool;
    /// Go `CaseOrDefaultClause.Expression` (see `case_expression`).
    fn case_expression(&mut self, id: crate::astdata::NodeId) -> bool;
    /// The Go `FullSignature` field of a function-like node: an optional
    /// child field (Go `visit`). Only `ChildFields` tells it apart.
    #[inline(always)]
    fn full_signature(&mut self, id: Option<crate::astdata::NodeId>) -> bool {
        self.opt(id)
    }
}

/// The generic walk of `for_each_child_impl`: the nodes that are not
/// published store nodes, and the list hook of `for_each_child_and_lists`.
// PERF: the methods are always inlined, so each arm of `walk_children`
// compiles to the same code as the macros of the old single match.
struct NodeChildVisit<'a, 'b> {
    n: Node,
    file: usize,
    v: &'a mut dyn FnMut(Node) -> bool,
    lists: Option<ListHook<'b>>,
}

impl NodeChildVisit<'_, '_> {
    #[inline(always)]
    fn report(&mut self, l: NodeList, is_mod: bool) {
        if let Some(h) = self.lists.as_mut() {
            if l.is_some() {
                h(l, is_mod);
            }
        }
    }

    #[inline(always)]
    fn visit_list(&mut self, l: NodeList) -> bool {
        self.report(l, false);
        visit_node_list(self.v, l)
    }
}

impl ChildVisit<'static> for NodeChildVisit<'_, '_> {
    #[inline(always)]
    fn node(&mut self, id: crate::astdata::NodeId) -> bool {
        visit(self.v, req(self.file, id))
    }

    #[inline(always)]
    fn opt(&mut self, id: Option<crate::astdata::NodeId>) -> bool {
        visit(self.v, opt(self.file, id))
    }

    #[inline(always)]
    fn list(&mut self, l: &'static crate::astdata::NodeList) -> bool {
        self.visit_list(data_list(self.n, *l))
    }

    #[inline(always)]
    fn opt_list(&mut self, l: &'static Option<crate::astdata::NodeList>) -> bool {
        self.visit_list(l.map_or(NodeList::NIL, |l| data_list(self.n, l)))
    }

    #[inline(always)]
    fn mods(&mut self, m: &'static Option<crate::astdata::ModifierList>) -> bool {
        let m = m.map_or(ModifierList::NIL, |m| data_modifiers(self.n, m));
        self.report(m.node_list(), true);
        visit_modifiers(self.v, m)
    }

    #[inline(always)]
    fn ids(&mut self, ids: &'static [crate::astdata::NodeId]) -> bool {
        visit_nodes(self.v, NodeSlice::from_ids(self.file, ids))
    }

    #[inline(always)]
    fn case_expression(&mut self, id: crate::astdata::NodeId) -> bool {
        visit(self.v, case_expression(self.n, self.file, id))
    }
}

/// The walk of `for_each_child_dyn` over the data of a published store
/// node (`FrozenNode`). It visits like `NodeChildVisit`, and makes each
/// child handle from the id map of the store (`FrozenIds`) that the node
/// lookup read. It has no list hook: `for_each_child_and_lists` walks with
/// `NodeChildVisit`.
// PERF: noderef1 W2. `NodeChildVisit` makes each child with an out-of-line
// `req` or `opt` call, a registry lookup each (about 25 instructions per
// field, Go 7, getsym1 report), and each list with a third lookup
// (`NodeSlice::iter`).
// PERF: noderef1 W3. Without the hook, the walk has no run-time hook test
// and no `NodeList` handles on the stack (Go: one `ForEachChild`, no hook).
struct FrozenChildVisit<'a> {
    /// Go `n.Kind` (`case_expression`).
    kind: SyntaxKind,
    id_map: FrozenIds,
    /// The list table of the node data (`BlockFile::lists`).
    lists: &'static [&'static StaticList],
    file: usize,
    v: &'a mut dyn FnMut(Node) -> bool,
}

impl FrozenChildVisit<'_> {
    /// Go `visitNodeList` of a list field.
    #[inline(always)]
    fn visit_list(&mut self, l: &'static crate::astdata::NodeList) -> bool {
        let l = self.lists[l.index()];
        !is_nil_list_range(&l.range) && self.ids(l.nodes)
    }
}

impl ChildVisit<'static> for FrozenChildVisit<'_> {
    #[inline(always)]
    fn node(&mut self, id: crate::astdata::NodeId) -> bool {
        let child = self.id_map.node(id);
        debug_assert_eq!(child, Node::new(self.file, id));
        visit(self.v, child)
    }

    #[inline(always)]
    fn opt(&mut self, id: Option<crate::astdata::NodeId>) -> bool {
        id.is_some_and(|id| self.node(id))
    }

    #[inline(always)]
    fn list(&mut self, l: &'static crate::astdata::NodeList) -> bool {
        self.visit_list(l)
    }

    #[inline(always)]
    fn opt_list(&mut self, l: &'static Option<crate::astdata::NodeList>) -> bool {
        l.as_ref().is_some_and(|l| self.visit_list(l))
    }

    // Go `visitModifiers`: `ModifierList::is_some` has no nil marker.
    #[inline(always)]
    fn mods(&mut self, m: &'static Option<crate::astdata::ModifierList>) -> bool {
        m.as_ref()
            .is_some_and(|m| self.ids(self.lists[m.list.index()].nodes))
    }

    /// Go `visitNodes`: no nil test.
    #[inline(always)]
    fn ids(&mut self, ids: &'static [crate::astdata::NodeId]) -> bool {
        let (map, file) = (self.id_map, self.file);
        ids.iter().any(|&id| {
            let child = map.node(id);
            debug_assert_eq!(child, Node::new(file, id));
            (self.v)(child)
        })
    }

    #[inline(always)]
    fn case_expression(&mut self, id: crate::astdata::NodeId) -> bool {
        self.kind == SyntaxKind::CaseClause && self.node(id)
    }
}

/// The walk of `for_each_child_impl` over the data of a node of `file`
/// with no `'static` node: a factory node, or a node that a freeable parse
/// owns (lsshells M3c). The data is read in a scope, so each list is read
/// through its handle (`DataList`). Otherwise it visits like
/// `NodeChildVisit`.
struct ScopedChildVisit<'a, 'b> {
    n: Node,
    /// `SYNTHETIC_NODE_FILE` or the store id of `n`.
    file: usize,
    v: &'a mut dyn FnMut(Node) -> bool,
    lists: Option<ListHook<'b>>,
}

impl ScopedChildVisit<'_, '_> {
    /// `NodeChildVisit::report` for a list read in place.
    fn report(&mut self, l: DataList, is_mod: bool) {
        if let Some(h) = self.lists.as_mut()
            && l.is_some()
        {
            h(l.list, is_mod);
        }
    }

    /// Go `visitNodes` over the nodes of `l`.
    fn visit_nodes(&mut self, l: DataList) -> bool {
        for node in l.nodes() {
            if (self.v)(node) {
                return true;
            }
        }
        false
    }

    /// Go `visitNodeList` for a list read in place.
    fn visit_list(&mut self, l: DataList) -> bool {
        self.report(l, false);
        l.is_some() && self.visit_nodes(l)
    }
}

impl<'d> ChildVisit<'d> for ScopedChildVisit<'_, '_> {
    fn node(&mut self, id: crate::astdata::NodeId) -> bool {
        visit(self.v, req(self.file, id))
    }

    fn opt(&mut self, id: Option<crate::astdata::NodeId>) -> bool {
        visit(self.v, opt(self.file, id))
    }

    fn list(&mut self, l: &'d crate::astdata::NodeList) -> bool {
        self.visit_list(DataList::req(self.n, l))
    }

    fn opt_list(&mut self, l: &'d Option<crate::astdata::NodeList>) -> bool {
        self.visit_list(DataList::opt(self.n, l))
    }

    // Go `visitModifiers`: `ModifierList::is_some` has no nil marker.
    fn mods(&mut self, m: &'d Option<crate::astdata::ModifierList>) -> bool {
        let l = DataList::mods(self.n, m);
        self.report(l, true);
        l.is_some() && self.visit_nodes(l)
    }

    fn ids(&mut self, ids: &'d [crate::astdata::NodeId]) -> bool {
        let file = self.file;
        ids.iter().any(|&id| (self.v)(Node::new(file, id)))
    }

    fn case_expression(&mut self, id: crate::astdata::NodeId) -> bool {
        visit(self.v, case_expression(self.n, self.file, id))
    }
}

/// One child field of node data, in Go `ForEachChild` order
/// (`for_each_child_field`).
pub enum ChildField<'d> {
    /// A single child (Go `visit`). `Node::NIL` for Go `nil`.
    Node(Node),
    /// The `FullSignature` child of a function-like node (Go `visit`).
    /// `Node::NIL` for Go `nil`. The API encoder's property mask has no bit
    /// for it.
    FullSignature(Node),
    /// A list (Go `visitNodeList`). `NodeList::NIL` for Go `nil`: no list,
    /// or the nil marker of a required list field.
    List(NodeList),
    /// A modifier list (Go `visitModifiers`). `ModifierList::NIL` for Go
    /// `nil`.
    Modifiers(ModifierList),
    /// A Go `[]*Node` field with no list (Go `visitNodes`): the children of
    /// a SyntaxList and the property tags of a JSDocTypeLiteral.
    Ids(&'d [crate::astdata::NodeId]),
}

/// Calls `f` with each child field of `data`, the node data of node `n` of
/// Go kind `kind`, in Go `ForEachChild` order. Each child id becomes a
/// `Node` as the field accessors make it (`Node::new`), and each list a
/// handle (`data_list`).
/// Not in Go (perf, apiperf2): the API encoder (`encode_tree`) reads the
/// fields of a node of the encoded file once for its property mask and its
/// children. Go `VisitEachChild` visits the same fields in the same order,
/// except in a SyntaxList, a JSDocTypeLiteral and a JSDoc parameter or
/// property tag.
pub fn for_each_child_field<'d>(
    kind: SyntaxKind,
    n: Node,
    data: NodeData<'d>,
    f: impl FnMut(ChildField<'d>),
) {
    let file = n.file_index();
    walk_children(data, &mut ChildFields { kind, n, file, f });
}

/// The walk of `for_each_child_field`. It never stops early.
struct ChildFields<F> {
    /// The Go kind of the node (`case_expression`).
    kind: SyntaxKind,
    n: Node,
    file: usize,
    f: F,
}

impl<'d, F: FnMut(ChildField<'d>)> ChildVisit<'d> for ChildFields<F> {
    #[inline(always)]
    fn node(&mut self, id: crate::astdata::NodeId) -> bool {
        (self.f)(ChildField::Node(Node::new(self.file, id)));
        false
    }

    #[inline(always)]
    fn opt(&mut self, id: Option<crate::astdata::NodeId>) -> bool {
        (self.f)(ChildField::Node(
            id.map_or(Node::NIL, |id| Node::new(self.file, id)),
        ));
        false
    }

    #[inline(always)]
    fn list(&mut self, l: &'d crate::astdata::NodeList) -> bool {
        let l = data_list(self.n, *l);
        (self.f)(ChildField::List(if l.is_nil() { NodeList::NIL } else { l }));
        false
    }

    #[inline(always)]
    fn opt_list(&mut self, l: &'d Option<crate::astdata::NodeList>) -> bool {
        let l = l.map_or(NodeList::NIL, |l| data_list(self.n, l));
        (self.f)(ChildField::List(if l.is_nil() { NodeList::NIL } else { l }));
        false
    }

    #[inline(always)]
    fn mods(&mut self, m: &'d Option<crate::astdata::ModifierList>) -> bool {
        let m = m.map_or(ModifierList::NIL, |m| data_modifiers(self.n, m));
        (self.f)(ChildField::Modifiers(m));
        false
    }

    #[inline(always)]
    fn ids(&mut self, ids: &'d [crate::astdata::NodeId]) -> bool {
        (self.f)(ChildField::Ids(ids));
        false
    }

    // Go `CaseOrDefaultClause.Expression`: nil for `default:`.
    #[inline(always)]
    fn case_expression(&mut self, id: crate::astdata::NodeId) -> bool {
        let n = if self.kind == SyntaxKind::CaseClause {
            Node::new(self.file, id)
        } else {
            Node::NIL
        };
        (self.f)(ChildField::Node(n));
        false
    }

    #[inline(always)]
    fn full_signature(&mut self, id: Option<crate::astdata::NodeId>) -> bool {
        (self.f)(ChildField::FullSignature(
            id.map_or(Node::NIL, |id| Node::new(self.file, id)),
        ));
        false
    }
}

/// R3-2: the walk of `for_each_store_child_id`. The ids are slot indexes of
/// the store of the node. Slot 0 resolves to Go `nil`
/// (`resolve_store_id`), so a single child field with id 0 is skipped.
struct StoreChildIds<'l, F> {
    /// The Go kind of the node (`case_expression` reads it).
    kind: SyntaxKind,
    /// The list table of the node data.
    lists: ListIds<'l>,
    v: F,
}

impl<'d, F: StoreChildSink> ChildVisit<'d> for StoreChildIds<'_, F> {
    #[inline(always)]
    fn node(&mut self, id: crate::astdata::NodeId) -> bool {
        id.index() != 0 && self.v.child(id.index() as u32)
    }

    #[inline(always)]
    fn opt(&mut self, id: Option<crate::astdata::NodeId>) -> bool {
        id.is_some_and(|id| self.node(id))
    }

    // `NodeList::is_nil` of a store list: the nil marker list.
    #[inline(always)]
    fn list(&mut self, l: &'d crate::astdata::NodeList) -> bool {
        self.lists.ids(*l).is_some_and(|ids| self.slots(ids))
    }

    #[inline(always)]
    fn opt_list(&mut self, l: &'d Option<crate::astdata::NodeList>) -> bool {
        l.as_ref().is_some_and(|l| self.list(l))
    }

    // `ModifierList::is_nil` is only `None`.
    #[inline(always)]
    fn mods(&mut self, m: &'d Option<crate::astdata::ModifierList>) -> bool {
        m.as_ref()
            .is_some_and(|m| self.lists.ids(m.list).is_some_and(|ids| self.slots(ids)))
    }

    #[inline(always)]
    fn ids(&mut self, ids: &'d [crate::astdata::NodeId]) -> bool {
        self.slots(ids)
    }

    #[inline(always)]
    fn case_expression(&mut self, id: crate::astdata::NodeId) -> bool {
        self.kind == SyntaxKind::CaseClause && self.node(id)
    }
}

impl<F: StoreChildSink> StoreChildIds<'_, F> {
    /// `v` on each slot index of `ids`, until it returns true.
    #[inline(always)]
    fn slots(&mut self, ids: &[crate::astdata::NodeId]) -> bool {
        ids.iter().any(|id| self.v.child(id.index() as u32))
    }
}

/// The Go `ForEachChild` fields of node data `data`, in Go order, for `w`.
// Go: ast_generated.go ForEachChild (one method per node struct)
// PORT: the per-struct Go methods are merged into one match. The order of
// each arm follows the generated Go code.
fn walk_children<'d>(data: NodeData<'d>, w: &mut impl ChildVisit<'d>) -> bool {
    macro_rules! n {
        ($x:expr) => {
            w.node($x)
        };
    }
    macro_rules! o {
        ($x:expr) => {
            w.opt($x)
        };
    }
    macro_rules! l {
        ($x:expr) => {
            w.list(&$x)
        };
    }
    macro_rules! ol {
        ($x:expr) => {
            w.opt_list(&$x)
        };
    }
    macro_rules! m {
        ($x:expr) => {
            w.mods(&$x)
        };
    }
    match data {
        NodeData::QualifiedName(d) => n!(d.left) || n!(d.right),
        NodeData::ComputedPropertyName(d) => n!(d.expression),
        NodeData::Decorator(d) => n!(d.expression),
        NodeData::IfStatement(d) => {
            n!(d.expression) || n!(d.then_statement) || o!(d.else_statement)
        }
        NodeData::DoStatement(d) => n!(d.statement) || n!(d.expression),
        NodeData::WhileStatement(d) => n!(d.expression) || n!(d.statement),
        NodeData::ForStatement(d) => {
            o!(d.initializer) || o!(d.condition) || o!(d.incrementor) || n!(d.statement)
        }
        NodeData::ForInOrOfStatement(d) => {
            o!(d.await_modifier) || n!(d.initializer) || n!(d.expression) || n!(d.statement)
        }
        NodeData::BreakStatement(d) => o!(d.label),
        NodeData::ContinueStatement(d) => o!(d.label),
        NodeData::ReturnStatement(d) => o!(d.expression),
        NodeData::WithStatement(d) => n!(d.expression) || n!(d.statement),
        NodeData::SwitchStatement(d) => n!(d.expression) || n!(d.case_block),
        NodeData::CaseBlock(d) => l!(d.clauses),
        NodeData::CaseOrDefaultClause(d) => w.case_expression(d.expression) || l!(d.statements),
        NodeData::ThrowStatement(d) => n!(d.expression),
        NodeData::TryStatement(d) => n!(d.try_block) || o!(d.catch_clause) || o!(d.finally_block),
        NodeData::CatchClause(d) => o!(d.variable_declaration) || n!(d.block),
        NodeData::LabeledStatement(d) => n!(d.label) || n!(d.statement),
        NodeData::ExpressionStatement(d) => n!(d.expression),
        NodeData::Block(d) => l!(d.statements),
        NodeData::VariableStatement(d) => m!(d.modifiers) || n!(d.declaration_list),
        NodeData::VariableDeclaration(d) => {
            n!(d.name) || o!(d.exclamation_token) || o!(d.type_) || o!(d.initializer)
        }
        NodeData::VariableDeclarationList(d) => l!(d.declarations),
        NodeData::BindingPattern(d) => l!(d.elements),
        NodeData::ParameterDeclaration(d) => {
            m!(d.modifiers)
                || o!(d.dot_dot_dot_token)
                || n!(d.name)
                || o!(d.question_token)
                || o!(d.type_)
                || o!(d.initializer)
        }
        NodeData::BindingElement(d) => {
            o!(d.dot_dot_dot_token) || o!(d.property_name) || o!(d.name) || o!(d.initializer)
        }
        NodeData::MissingDeclaration(d) => m!(d.modifiers),
        NodeData::FunctionDeclaration(d) => {
            m!(d.modifiers)
                || o!(d.asterisk_token)
                || o!(d.name)
                || ol!(d.type_parameters)
                || l!(d.parameters)
                || o!(d.type_)
                || w.full_signature(d.full_signature)
                || o!(d.body)
        }
        NodeData::ClassDeclaration(d) => {
            m!(d.modifiers)
                || o!(d.name)
                || ol!(d.type_parameters)
                || ol!(d.heritage_clauses)
                || l!(d.members)
        }
        NodeData::ClassExpression(d) => {
            m!(d.modifiers)
                || o!(d.name)
                || ol!(d.type_parameters)
                || ol!(d.heritage_clauses)
                || l!(d.members)
        }
        NodeData::InterfaceDeclaration(d) => {
            m!(d.modifiers)
                || n!(d.name)
                || ol!(d.type_parameters)
                || ol!(d.heritage_clauses)
                || l!(d.members)
        }
        NodeData::HeritageClause(d) => l!(d.types),
        NodeData::TypeAliasDeclaration(d) => {
            m!(d.modifiers) || n!(d.name) || ol!(d.type_parameters) || n!(d.type_)
        }
        NodeData::EnumMember(d) => n!(d.name) || o!(d.initializer),
        NodeData::EnumDeclaration(d) => m!(d.modifiers) || n!(d.name) || l!(d.members),
        NodeData::ModuleBlock(d) => l!(d.statements),
        NodeData::ImportDeclaration(d) => {
            m!(d.modifiers) || o!(d.import_clause) || n!(d.module_specifier) || o!(d.attributes)
        }
        NodeData::ExternalModuleReference(d) => n!(d.expression),
        NodeData::NamespaceImport(d) => n!(d.name),
        NodeData::NamedImports(d) => l!(d.elements),
        NodeData::ExportAssignment(d) => m!(d.modifiers) || o!(d.type_) || n!(d.expression),
        NodeData::NamespaceExportDeclaration(d) => m!(d.modifiers) || n!(d.name),
        NodeData::NamespaceExport(d) => n!(d.name),
        NodeData::NamedExports(d) => l!(d.elements),
        NodeData::ExportSpecifier(d) => o!(d.property_name) || n!(d.name),
        NodeData::CallSignatureDeclaration(d) => {
            ol!(d.type_parameters) || l!(d.parameters) || o!(d.type_)
        }
        NodeData::ConstructSignatureDeclaration(d) => {
            ol!(d.type_parameters) || l!(d.parameters) || o!(d.type_)
        }
        NodeData::ConstructorDeclaration(d) => {
            m!(d.modifiers)
                || ol!(d.type_parameters)
                || l!(d.parameters)
                || o!(d.type_)
                || w.full_signature(d.full_signature)
                || o!(d.body)
        }
        NodeData::GetAccessorDeclaration(d) => {
            m!(d.modifiers)
                || n!(d.name)
                || ol!(d.type_parameters)
                || l!(d.parameters)
                || o!(d.type_)
                || w.full_signature(d.full_signature)
                || o!(d.body)
        }
        NodeData::SetAccessorDeclaration(d) => {
            m!(d.modifiers)
                || n!(d.name)
                || ol!(d.type_parameters)
                || l!(d.parameters)
                || o!(d.type_)
                || w.full_signature(d.full_signature)
                || o!(d.body)
        }
        NodeData::IndexSignatureDeclaration(d) => {
            m!(d.modifiers) || l!(d.parameters) || n!(d.type_)
        }
        NodeData::MethodSignatureDeclaration(d) => {
            m!(d.modifiers)
                || n!(d.name)
                || o!(d.postfix_token)
                || ol!(d.type_parameters)
                || l!(d.parameters)
                || o!(d.type_)
        }
        NodeData::MethodDeclaration(d) => {
            m!(d.modifiers)
                || o!(d.asterisk_token)
                || n!(d.name)
                || o!(d.postfix_token)
                || ol!(d.type_parameters)
                || l!(d.parameters)
                || o!(d.type_)
                || w.full_signature(d.full_signature)
                || o!(d.body)
        }
        NodeData::PropertySignatureDeclaration(d) => {
            m!(d.modifiers) || n!(d.name) || o!(d.postfix_token) || n!(d.type_) || n!(d.initializer)
        }
        NodeData::PropertyDeclaration(d) => {
            m!(d.modifiers) || n!(d.name) || o!(d.postfix_token) || o!(d.type_) || o!(d.initializer)
        }
        NodeData::ClassStaticBlockDeclaration(d) => m!(d.modifiers) || n!(d.body),
        NodeData::BinaryExpression(d) => {
            m!(d.modifiers) || n!(d.left) || o!(d.type_) || n!(d.operator_token) || n!(d.right)
        }
        NodeData::PrefixUnaryExpression(d) => n!(d.operand),
        NodeData::PostfixUnaryExpression(d) => n!(d.operand),
        NodeData::YieldExpression(d) => o!(d.asterisk_token) || o!(d.expression),
        NodeData::ArrowFunction(d) => {
            m!(d.modifiers)
                || ol!(d.type_parameters)
                || l!(d.parameters)
                || o!(d.type_)
                || w.full_signature(d.full_signature)
                || n!(d.equals_greater_than_token)
                || n!(d.body)
        }
        NodeData::FunctionExpression(d) => {
            m!(d.modifiers)
                || o!(d.asterisk_token)
                || o!(d.name)
                || ol!(d.type_parameters)
                || l!(d.parameters)
                || o!(d.type_)
                || w.full_signature(d.full_signature)
                || n!(d.body)
        }
        NodeData::AsExpression(d) => n!(d.expression) || n!(d.type_),
        NodeData::SatisfiesExpression(d) => n!(d.expression) || n!(d.type_),
        NodeData::ConditionalExpression(d) => {
            n!(d.condition)
                || n!(d.question_token)
                || n!(d.when_true)
                || n!(d.colon_token)
                || n!(d.when_false)
        }
        NodeData::PropertyAccessExpression(d) => {
            n!(d.expression) || o!(d.question_dot_token) || n!(d.name)
        }
        NodeData::ElementAccessExpression(d) => {
            n!(d.expression) || o!(d.question_dot_token) || n!(d.argument_expression)
        }
        NodeData::CallExpression(d) => {
            n!(d.expression) || o!(d.question_dot_token) || ol!(d.type_arguments) || l!(d.arguments)
        }
        NodeData::NewExpression(d) => n!(d.expression) || ol!(d.type_arguments) || ol!(d.arguments),
        NodeData::MetaProperty(d) => n!(d.name),
        NodeData::NonNullExpression(d) => n!(d.expression),
        NodeData::SpreadElement(d) => n!(d.expression),
        NodeData::TemplateExpression(d) => n!(d.head) || l!(d.template_spans),
        NodeData::TemplateSpan(d) => n!(d.expression) || n!(d.literal),
        NodeData::TaggedTemplateExpression(d) => {
            n!(d.tag) || o!(d.question_dot_token) || ol!(d.type_arguments) || n!(d.template)
        }
        NodeData::ParenthesizedExpression(d) => n!(d.expression),
        NodeData::ArrayLiteralExpression(d) => l!(d.elements),
        NodeData::ObjectLiteralExpression(d) => l!(d.properties),
        NodeData::SpreadAssignment(d) => n!(d.expression),
        NodeData::PropertyAssignment(d) => {
            m!(d.modifiers) || n!(d.name) || o!(d.postfix_token) || o!(d.type_) || n!(d.initializer)
        }
        NodeData::ShorthandPropertyAssignment(d) => {
            m!(d.modifiers)
                || n!(d.name)
                || o!(d.postfix_token)
                || o!(d.type_)
                || o!(d.equals_token)
                || o!(d.object_assignment_initializer)
        }
        NodeData::DeleteExpression(d) => n!(d.expression),
        NodeData::TypeOfExpression(d) => n!(d.expression),
        NodeData::VoidExpression(d) => n!(d.expression),
        NodeData::AwaitExpression(d) => n!(d.expression),
        NodeData::TypeAssertion(d) => n!(d.type_) || n!(d.expression),
        NodeData::UnionTypeNode(d) => l!(d.types),
        NodeData::IntersectionTypeNode(d) => l!(d.types),
        NodeData::ConditionalTypeNode(d) => {
            n!(d.check_type) || n!(d.extends_type) || n!(d.true_type) || n!(d.false_type)
        }
        NodeData::TypeOperatorNode(d) => n!(d.type_),
        NodeData::InferTypeNode(d) => n!(d.type_parameter),
        NodeData::ArrayTypeNode(d) => n!(d.element_type),
        NodeData::IndexedAccessTypeNode(d) => n!(d.object_type) || n!(d.index_type),
        NodeData::TypeReferenceNode(d) => n!(d.type_name) || ol!(d.type_arguments),
        NodeData::ExpressionWithTypeArguments(d) => n!(d.expression) || ol!(d.type_arguments),
        NodeData::LiteralTypeNode(d) => n!(d.literal),
        NodeData::TypePredicateNode(d) => {
            o!(d.asserts_modifier) || n!(d.parameter_name) || o!(d.type_)
        }
        NodeData::ImportAttribute(d) => n!(d.name) || n!(d.value),
        NodeData::ImportAttributes(d) => l!(d.attributes),
        NodeData::TypeQueryNode(d) => n!(d.expr_name) || ol!(d.type_arguments),
        NodeData::MappedTypeNode(d) => {
            o!(d.readonly_token)
                || n!(d.type_parameter)
                || o!(d.name_type)
                || o!(d.question_token)
                || o!(d.type_)
                || ol!(d.members)
        }
        NodeData::TypeLiteralNode(d) => l!(d.members),
        NodeData::TupleTypeNode(d) => l!(d.elements),
        NodeData::NamedTupleMember(d) => {
            o!(d.dot_dot_dot_token) || n!(d.name) || o!(d.question_token) || n!(d.type_)
        }
        NodeData::OptionalTypeNode(d) => n!(d.type_),
        NodeData::RestTypeNode(d) => n!(d.type_),
        NodeData::ParenthesizedTypeNode(d) => n!(d.type_),
        NodeData::FunctionTypeNode(d) => ol!(d.type_parameters) || l!(d.parameters) || o!(d.type_),
        NodeData::ConstructorTypeNode(d) => {
            m!(d.modifiers) || ol!(d.type_parameters) || l!(d.parameters) || o!(d.type_)
        }
        NodeData::TemplateLiteralTypeNode(d) => n!(d.head) || l!(d.template_spans),
        NodeData::TemplateLiteralTypeSpan(d) => n!(d.type_) || n!(d.literal),
        NodeData::SyntheticExpression(d) => o!(d.tuple_name_source),
        NodeData::PartiallyEmittedExpression(d) => n!(d.expression),
        NodeData::JsxElement(d) => n!(d.opening_element) || l!(d.children) || n!(d.closing_element),
        NodeData::JsxAttributes(d) => l!(d.properties),
        NodeData::JsxNamespacedName(d) => n!(d.namespace) || n!(d.name),
        NodeData::JsxOpeningElement(d) => {
            n!(d.tag_name) || ol!(d.type_arguments) || n!(d.attributes)
        }
        NodeData::JsxSelfClosingElement(d) => {
            n!(d.tag_name) || ol!(d.type_arguments) || n!(d.attributes)
        }
        NodeData::JsxFragment(d) => {
            n!(d.opening_fragment) || l!(d.children) || n!(d.closing_fragment)
        }
        NodeData::JsxAttribute(d) => n!(d.name) || o!(d.initializer),
        NodeData::JsxSpreadAttribute(d) => n!(d.expression),
        NodeData::JsxClosingElement(d) => n!(d.tag_name),
        NodeData::JsxExpression(d) => o!(d.dot_dot_dot_token) || o!(d.expression),
        NodeData::SyntaxList(d) => w.ids(&d.children),
        NodeData::JsDoc(d) => l!(d.comment) || ol!(d.tags),
        NodeData::JsDocTypeExpression(d) => n!(d.type_),
        NodeData::JsDocNonNullableType(d) => n!(d.type_),
        NodeData::JsDocNullableType(d) => n!(d.type_),
        NodeData::JsDocVariadicType(d) => n!(d.type_),
        NodeData::JsDocOptionalType(d) => n!(d.type_),
        NodeData::JsDocTypeTag(d) => n!(d.tag_name) || n!(d.type_expression) || ol!(d.comment),
        NodeData::JsDocReturnTag(d) => n!(d.tag_name) || o!(d.type_expression) || ol!(d.comment),
        NodeData::JsDocSatisfiesTag(d) => n!(d.tag_name) || n!(d.type_expression) || ol!(d.comment),
        NodeData::JsDocThrowsTag(d) => n!(d.tag_name) || o!(d.type_expression) || ol!(d.comment),
        NodeData::JsDocThisTag(d) => n!(d.tag_name) || n!(d.type_expression) || ol!(d.comment),
        NodeData::JsDocOverloadTag(d) => n!(d.tag_name) || n!(d.type_expression) || ol!(d.comment),
        NodeData::JsDocUnknownTag(d) => n!(d.tag_name) || ol!(d.comment),
        NodeData::JsDocPublicTag(d) => n!(d.tag_name) || ol!(d.comment),
        NodeData::JsDocPrivateTag(d) => n!(d.tag_name) || ol!(d.comment),
        NodeData::JsDocProtectedTag(d) => n!(d.tag_name) || ol!(d.comment),
        NodeData::JsDocReadonlyTag(d) => n!(d.tag_name) || ol!(d.comment),
        NodeData::JsDocOverrideTag(d) => n!(d.tag_name) || ol!(d.comment),
        NodeData::JsDocDeprecatedTag(d) => n!(d.tag_name) || ol!(d.comment),
        NodeData::JsDocTemplateTag(d) => {
            n!(d.tag_name) || n!(d.constraint) || l!(d.type_parameters) || ol!(d.comment)
        }
        NodeData::JsDocSeeTag(d) => n!(d.tag_name) || n!(d.name_expression) || ol!(d.comment),
        NodeData::JsDocImplementsTag(d) => n!(d.tag_name) || n!(d.class_name) || ol!(d.comment),
        NodeData::JsDocAugmentsTag(d) => n!(d.tag_name) || n!(d.class_name) || ol!(d.comment),
        NodeData::JsDocImportTag(d) => {
            n!(d.tag_name)
                || o!(d.import_clause)
                || n!(d.module_specifier)
                || o!(d.attributes)
                || ol!(d.comment)
        }
        NodeData::JsDocCallbackTag(d) => {
            n!(d.tag_name) || n!(d.type_expression) || o!(d.name) || ol!(d.comment)
        }
        NodeData::JsDocTypedefTag(d) => {
            n!(d.tag_name) || o!(d.type_expression) || o!(d.name) || ol!(d.comment)
        }
        NodeData::JsDocSignature(d) => ol!(d.type_parameters) || l!(d.parameters) || o!(d.type_),
        NodeData::JsDocNameReference(d) => n!(d.name),
        // Go: ast.go:3097 forEachChild_JSDocParameterOrPropertyTag
        NodeData::JsDocParameterOrPropertyTag(d) => {
            n!(d.tag_name)
                || (d.is_name_first && (n!(d.name) || o!(d.type_expression)))
                || (!d.is_name_first && (o!(d.type_expression) || n!(d.name)))
                || ol!(d.comment)
        }
        NodeData::ModuleDeclaration(d) => {
            m!(d.modifiers) || n!(d.name) || o!(d.attributes) || o!(d.body)
        }
        NodeData::ImportEqualsDeclaration(d) => {
            m!(d.modifiers) || n!(d.name) || n!(d.module_reference)
        }
        NodeData::ExportDeclaration(d) => {
            m!(d.modifiers) || o!(d.export_clause) || o!(d.module_specifier) || o!(d.attributes)
        }
        NodeData::ImportTypeNode(d) => {
            n!(d.argument) || o!(d.attributes) || o!(d.qualifier) || ol!(d.type_arguments)
        }
        NodeData::ImportClause(d) => o!(d.name) || o!(d.named_bindings),
        NodeData::ImportSpecifier(d) => o!(d.property_name) || n!(d.name),
        NodeData::JsDocLink(d) => o!(d.name),
        NodeData::JsDocLinkCode(d) => o!(d.name),
        NodeData::JsDocLinkPlain(d) => o!(d.name),
        NodeData::TypeParameterDeclaration(d) => {
            m!(d.modifiers)
                || n!(d.name)
                || o!(d.constraint)
                || o!(d.expression)
                || o!(d.default_type)
        }
        NodeData::SyntheticReferenceExpression(d) => n!(d.expression) || n!(d.this_arg),
        NodeData::JsDocTypeLiteral(d) => match &d.js_doc_property_tags {
            Some(tags) => w.ids(tags),
            None => false,
        },
        NodeData::SourceFile(d) => l!(d.statements) || n!(d.end_of_file_token),
        // The data of the kinds of `has_no_child_fields`. Every variant is
        // listed, so a new variant must pick an arm.
        NodeData::Token(_)
        | NodeData::Identifier(_)
        | NodeData::PrivateIdentifier(_)
        | NodeData::KeywordExpression(_)
        | NodeData::KeywordTypeNode(_)
        | NodeData::NumericLiteral(_)
        | NodeData::BigIntLiteral(_)
        | NodeData::StringLiteral(_)
        | NodeData::RegularExpressionLiteral(_)
        | NodeData::NoSubstitutionTemplateLiteral(_)
        | NodeData::TemplateHead(_)
        | NodeData::TemplateMiddle(_)
        | NodeData::TemplateTail(_)
        | NodeData::JsxText(_)
        | NodeData::ThisTypeNode(_)
        | NodeData::OmittedExpression(_)
        | NodeData::SemicolonClassElement(_)
        | NodeData::EmptyStatement(_)
        | NodeData::DebuggerStatement(_)
        | NodeData::JsxOpeningFragment(_)
        | NodeData::JsxClosingFragment(_)
        | NodeData::JsDocAllType(_)
        | NodeData::JsDocText(_)
        | NodeData::NotEmittedStatement(_)
        | NodeData::NotEmittedTypeElement(_) => false,
    }
}

/// Go `(*Node).ForEachChild` (ast_generated.go:8693) has no case for these
/// kinds and returns false: every token kind (identifiers, literals,
/// punctuation, keywords) and the node kinds with no child field. Their
/// data is a variant of the last arm of `walk_children`, which visits
/// nothing.
#[inline]
pub(crate) const fn has_no_child_fields(kind: SyntaxKind) -> bool {
    kind.is_token()
        || matches!(
            kind,
            SyntaxKind::ThisType
                | SyntaxKind::OmittedExpression
                | SyntaxKind::SemicolonClassElement
                | SyntaxKind::EmptyStatement
                | SyntaxKind::DebuggerStatement
                | SyntaxKind::JsxOpeningFragment
                | SyntaxKind::JsxClosingFragment
                | SyntaxKind::JsDocAllType
                | SyntaxKind::JsDocText
                | SyntaxKind::NotEmittedStatement
                | SyntaxKind::NotEmittedTypeElement
        )
}

// ──────────────────────────────────────────────────────────────────────
// Subtree facts (ast.go computeSubtreeFacts / propagateSubtreeFacts)
// ──────────────────────────────────────────────────────────────────────

// Go: subtreefacts.go SubtreeExclusions*
const EXCL_NODE: SubtreeFacts = SubtreeFacts::COMPUTED;
const EXCL_ARROW_FUNCTION: SubtreeFacts = EXCL_NODE
    .union(SubtreeFacts::SUBTREE_CONTAINS_AWAIT)
    .union(SubtreeFacts::SUBTREE_CONTAINS_OBJECT_REST_OR_SPREAD);
const EXCL_FUNCTION: SubtreeFacts = EXCL_NODE
    .union(SubtreeFacts::SUBTREE_CONTAINS_LEXICAL_THIS)
    .union(SubtreeFacts::SUBTREE_CONTAINS_LEXICAL_SUPER)
    .union(SubtreeFacts::SUBTREE_CONTAINS_AWAIT)
    .union(SubtreeFacts::SUBTREE_CONTAINS_OBJECT_REST_OR_SPREAD);
const EXCL_CONSTRUCTOR: SubtreeFacts = EXCL_FUNCTION;
const EXCL_METHOD: SubtreeFacts = EXCL_FUNCTION;
const EXCL_ACCESSOR: SubtreeFacts = EXCL_FUNCTION;
const EXCL_PROPERTY: SubtreeFacts = EXCL_NODE
    .union(SubtreeFacts::SUBTREE_CONTAINS_LEXICAL_THIS)
    .union(SubtreeFacts::SUBTREE_CONTAINS_LEXICAL_SUPER);
const EXCL_MODULE: SubtreeFacts = EXCL_PROPERTY;
const EXCL_OBJECT_LITERAL: SubtreeFacts =
    EXCL_NODE.union(SubtreeFacts::SUBTREE_CONTAINS_OBJECT_REST_OR_SPREAD);
const EXCL_VARIABLE_DECLARATION_LIST: SubtreeFacts = EXCL_OBJECT_LITERAL;
const EXCL_CATCH_CLAUSE: SubtreeFacts = EXCL_OBJECT_LITERAL;
const EXCL_BINDING_PATTERN: SubtreeFacts =
    EXCL_NODE.union(SubtreeFacts::SUBTREE_CONTAINS_REST_OR_SPREAD);

const TS: SubtreeFacts = SubtreeFacts::SUBTREE_CONTAINS_TYPE_SCRIPT;
const NONE_FACTS: SubtreeFacts = SubtreeFacts::NONE;

/// Go `core.IfElse(cond, facts, SubtreeFactsNone)`.
fn facts_if(cond: bool, facts: SubtreeFacts) -> SubtreeFacts {
    if cond { facts } else { NONE_FACTS }
}

// Go: subtreefacts.go:89 propagateEraseableSyntaxListSubtreeFacts
fn propagate_eraseable_list(children: DataList) -> SubtreeFacts {
    facts_if(children.is_some(), TS)
}

// Go: subtreefacts.go:93 propagateEraseableSyntaxSubtreeFacts
fn propagate_eraseable(child: Node) -> SubtreeFacts {
    facts_if(child.is_some(), TS)
}

// Go: subtreefacts.go:97 propagateObjectBindingElementSubtreeFacts
fn propagate_object_binding_element(child: Node) -> SubtreeFacts {
    let mut facts = propagate_subtree_facts(child);
    if facts.intersects(SubtreeFacts::SUBTREE_CONTAINS_REST_OR_SPREAD) {
        facts = facts.without(SubtreeFacts::SUBTREE_CONTAINS_REST_OR_SPREAD);
        facts |= SubtreeFacts::SUBTREE_CONTAINS_OBJECT_REST_OR_SPREAD
            | SubtreeFacts::SUBTREE_CONTAINS_ES_OBJECT_REST_OR_SPREAD;
    }
    facts
}

// Go: subtreefacts.go:106 propagateBindingElementSubtreeFacts
fn propagate_binding_element(child: Node) -> SubtreeFacts {
    propagate_subtree_facts(child).without(SubtreeFacts::SUBTREE_CONTAINS_REST_OR_SPREAD)
}

// Go: subtreefacts.go:110 propagateSubtreeFacts
/// The facts a child adds to its parent. None for a nil child.
fn propagate_subtree_facts(child: Node) -> SubtreeFacts {
    if child.is_nil() {
        return NONE_FACTS;
    }
    propagate_node_facts(child)
}

// Go: subtreefacts.go:117 propagateNodeListSubtreeFacts
// Go: subtreefacts.go:128 propagateModifierListSubtreeFacts (with
// `propagate_subtree_facts`)
// PORT: the list is read from the node data as a handle (`DataList`).
fn propagate_node_list(children: DataList, propagate: fn(Node) -> SubtreeFacts) -> SubtreeFacts {
    let mut facts = NONE_FACTS;
    for child in children.nodes() {
        facts |= propagate(child);
    }
    facts
}

/// True for Go node types that embed `CompositeBase` (directly, or through
/// `ClassLikeBase` or `AccessorDeclarationBase`), on the data `d` of node
/// `n`. Go caches `SubtreeFacts()` only for these types (`ast.go:1602`).
#[inline]
fn is_composite_data(n: Node, d: NodeData<'_>) -> bool {
    match d {
        // PORT: a PropertyDeclaration that Go parses as a PropertySignature
        // (`is_type_syntax_data`).
        NodeData::PropertyDeclaration(_) => n.kind() != SyntaxKind::PropertySignature,
        NodeData::ClassDeclaration(_)
        | NodeData::ClassExpression(_)
        | NodeData::GetAccessorDeclaration(_)
        | NodeData::SetAccessorDeclaration(_)
        | NodeData::QualifiedName(_)
        | NodeData::ComputedPropertyName(_)
        | NodeData::Decorator(_)
        | NodeData::IfStatement(_)
        | NodeData::DoStatement(_)
        | NodeData::WhileStatement(_)
        | NodeData::ForStatement(_)
        | NodeData::ForInOrOfStatement(_)
        | NodeData::ReturnStatement(_)
        | NodeData::WithStatement(_)
        | NodeData::SwitchStatement(_)
        | NodeData::CaseBlock(_)
        | NodeData::CaseOrDefaultClause(_)
        | NodeData::ThrowStatement(_)
        | NodeData::TryStatement(_)
        | NodeData::CatchClause(_)
        | NodeData::Block(_)
        | NodeData::VariableStatement(_)
        | NodeData::VariableDeclaration(_)
        | NodeData::VariableDeclarationList(_)
        | NodeData::BindingPattern(_)
        | NodeData::ParameterDeclaration(_)
        | NodeData::BindingElement(_)
        | NodeData::FunctionDeclaration(_)
        | NodeData::HeritageClause(_)
        | NodeData::EnumMember(_)
        | NodeData::EnumDeclaration(_)
        | NodeData::ModuleBlock(_)
        | NodeData::ImportDeclaration(_)
        | NodeData::NamedImports(_)
        | NodeData::ExportAssignment(_)
        | NodeData::NamedExports(_)
        | NodeData::ExportSpecifier(_)
        | NodeData::ConstructorDeclaration(_)
        | NodeData::MethodDeclaration(_)
        | NodeData::ClassStaticBlockDeclaration(_)
        | NodeData::BinaryExpression(_)
        | NodeData::ArrowFunction(_)
        | NodeData::FunctionExpression(_)
        | NodeData::ConditionalExpression(_)
        | NodeData::PropertyAccessExpression(_)
        | NodeData::ElementAccessExpression(_)
        | NodeData::CallExpression(_)
        | NodeData::NewExpression(_)
        | NodeData::MetaProperty(_)
        | NodeData::TemplateExpression(_)
        | NodeData::TaggedTemplateExpression(_)
        | NodeData::ArrayLiteralExpression(_)
        | NodeData::ObjectLiteralExpression(_)
        | NodeData::PropertyAssignment(_)
        | NodeData::ShorthandPropertyAssignment(_)
        | NodeData::ExpressionWithTypeArguments(_)
        | NodeData::ImportAttribute(_)
        | NodeData::ImportAttributes(_)
        | NodeData::JsxElement(_)
        | NodeData::JsxAttributes(_)
        | NodeData::JsxNamespacedName(_)
        | NodeData::JsxOpeningElement(_)
        | NodeData::JsxSelfClosingElement(_)
        | NodeData::JsxFragment(_)
        | NodeData::JsxAttribute(_)
        | NodeData::ModuleDeclaration(_)
        | NodeData::ImportEqualsDeclaration(_)
        | NodeData::ExportDeclaration(_)
        | NodeData::ImportClause(_)
        | NodeData::ImportSpecifier(_)
        | NodeData::SourceFile(_) => true,
        _ => false,
    }
}

/// True for Go node types that embed `TypeSyntaxBase`, on the data `d` of
/// node `n`.
// PERF: emitast1 F2. The callers load the data once and pass it in; this
// was two data reads of its own for each node (`with_data!` twice).
#[inline]
fn is_type_syntax_data(n: Node, d: NodeData<'_>) -> bool {
    match d {
        // PORT: a PropertyDeclaration that Go parses as a PropertySignature.
        NodeData::PropertyDeclaration(_) => n.kind() == SyntaxKind::PropertySignature,
        NodeData::InterfaceDeclaration(_)
        | NodeData::TypeAliasDeclaration(_)
        | NodeData::NamespaceExportDeclaration(_)
        | NodeData::CallSignatureDeclaration(_)
        | NodeData::ConstructSignatureDeclaration(_)
        | NodeData::IndexSignatureDeclaration(_)
        | NodeData::MethodSignatureDeclaration(_)
        | NodeData::PropertySignatureDeclaration(_)
        | NodeData::TypeParameterDeclaration(_)
        | NodeData::KeywordTypeNode(_)
        | NodeData::UnionTypeNode(_)
        | NodeData::IntersectionTypeNode(_)
        | NodeData::ConditionalTypeNode(_)
        | NodeData::TypeOperatorNode(_)
        | NodeData::InferTypeNode(_)
        | NodeData::ArrayTypeNode(_)
        | NodeData::IndexedAccessTypeNode(_)
        | NodeData::TypeReferenceNode(_)
        | NodeData::LiteralTypeNode(_)
        | NodeData::ThisTypeNode(_)
        | NodeData::TypePredicateNode(_)
        | NodeData::TypeQueryNode(_)
        | NodeData::MappedTypeNode(_)
        | NodeData::TypeLiteralNode(_)
        | NodeData::TupleTypeNode(_)
        | NodeData::NamedTupleMember(_)
        | NodeData::OptionalTypeNode(_)
        | NodeData::RestTypeNode(_)
        | NodeData::ParenthesizedTypeNode(_)
        | NodeData::FunctionTypeNode(_)
        | NodeData::ConstructorTypeNode(_)
        | NodeData::TemplateLiteralTypeNode(_)
        | NodeData::TemplateLiteralTypeSpan(_)
        | NodeData::ImportTypeNode(_)
        | NodeData::JsDocTypeExpression(_)
        | NodeData::JsDocNonNullableType(_)
        | NodeData::JsDocNullableType(_)
        | NodeData::JsDocAllType(_)
        | NodeData::JsDocVariadicType(_)
        | NodeData::JsDocOptionalType(_)
        | NodeData::JsDocSignature(_)
        | NodeData::JsDocNameReference(_)
        | NodeData::JsDocTypeLiteral(_) => true,
        _ => false,
    }
}

// Go: ast.go:1173 (*NodeDefault).propagateSubtreeFacts and the per-type
// propagateSubtreeFacts overrides in ast.go.
// PORT: the Go overrides are merged into one match on the node data.
// PERF: emitast1 F2. One data read per child: the type syntax test, the
// facts of a child that has none cached yet and the exclusion all use it.
fn propagate_node_facts(n: Node) -> SubtreeFacts {
    with_data!(n, |d| propagate_node_facts_of_data(n, d))
}

/// `propagate_node_facts` over the data `d` of `n`, read in place.
fn propagate_node_facts_of_data(n: Node, d: NodeData<'_>) -> SubtreeFacts {
    // Go: ast.go:1546 (*TypeSyntaxBase).propagateSubtreeFacts
    if is_type_syntax_data(n, d) {
        return TS;
    }
    let facts = non_type_subtree_facts(n, d);
    let f = n.file_index();
    match d {
        NodeData::CatchClause(_) => facts.without(EXCL_CATCH_CLAUSE),
        NodeData::VariableDeclarationList(_) => facts.without(EXCL_VARIABLE_DECLARATION_LIST),
        NodeData::BindingPattern(_) => facts.without(EXCL_BINDING_PATTERN),
        NodeData::FunctionDeclaration(_) | NodeData::FunctionExpression(_) => {
            facts.without(EXCL_FUNCTION)
        }
        NodeData::ModuleDeclaration(_) => facts.without(EXCL_MODULE),
        NodeData::ConstructorDeclaration(_) => facts.without(EXCL_CONSTRUCTOR),
        // `req(f, d.name)` is `n.name()` (the name column of a store node
        // holds the same child).
        NodeData::GetAccessorDeclaration(d) => {
            facts.without(EXCL_ACCESSOR) | propagate_subtree_facts(req(f, d.name))
        }
        NodeData::SetAccessorDeclaration(d) => {
            facts.without(EXCL_ACCESSOR) | propagate_subtree_facts(req(f, d.name))
        }
        NodeData::MethodDeclaration(d) => {
            facts.without(EXCL_METHOD) | propagate_subtree_facts(req(f, d.name))
        }
        NodeData::PropertyDeclaration(d) => {
            facts.without(EXCL_PROPERTY) | propagate_subtree_facts(req(f, d.name))
        }
        NodeData::ArrowFunction(_) => facts.without(EXCL_ARROW_FUNCTION),
        NodeData::ObjectLiteralExpression(_) => facts.without(EXCL_OBJECT_LITERAL),
        // Parameter, Class, OuterExpression, PropertyAccess, ElementAccess,
        // Call, New and ArrayLiteral exclusions all equal the Node exclusion.
        _ => facts.without(EXCL_NODE),
    }
}

// Go: ast.go:1527 (*CompositeBase).subtreeFactsWorker and ast.go:1234 (at fed0bf24149f; removed by ts#64773)
// (*NodeDefault).subtreeFactsWorker
/// Go `n.SubtreeFacts()` for node `n` whose data `d` the caller loaded. A
/// node of a `CompositeBase` kind gets the cached facts, or the facts
/// computed from `d` (and cached). Any other node (a token, an identifier,
/// a literal, type syntax) gets the facts computed from `d`, not cached.
fn subtree_facts_with_data(n: Node, d: NodeData<'_>) -> SubtreeFacts {
    if is_type_syntax_data(n, d) {
        // Go: ast.go:1544 (*TypeSyntaxBase).computeSubtreeFacts
        return TS;
    }
    non_type_subtree_facts(n, d)
}

/// `subtree_facts_with_data` for a node that is not type syntax
/// (`is_type_syntax_data`), which the caller checked.
// PERF: factscol1a. Caching every node cost 2 thread-local map accesses per
// node (4.9x Go's instructions in the facts walk of a 5 MB JS file).
// PERF: factscol1b. A node of a static publish reads and writes one atomic
// word of the facts column of its file, as Go does. Two threads can both
// compute the facts of a node; Go's `computeSubtreeFacts` is idempotent, so
// they write the same word. The type syntax test is made once per node,
// not once more here: in a deep chain of nodes that Go does not cache
// (`await await ... x`), each step was 2 tests.
#[inline]
fn non_type_subtree_facts(n: Node, d: NodeData<'_>) -> SubtreeFacts {
    debug_assert!(!is_type_syntax_data(n, d), "{:?} is type syntax", n.kind());
    if !is_composite_data(n, d) {
        return subtree_facts_of_data(n, d);
    }
    composite_subtree_facts(n, d)
}

/// `non_type_subtree_facts` for a node of a `CompositeBase` kind: the
/// cached facts, or the facts computed from `d`, cached.
#[inline(never)]
fn composite_subtree_facts(n: Node, d: NodeData<'_>) -> SubtreeFacts {
    if let Some(word) = super::store::static_facts_word(n) {
        if let Some(facts) = computed_facts(SubtreeFacts(word.load(Ordering::Relaxed))) {
            return facts;
        }
        let facts = subtree_facts_of_data(n, d).without(SubtreeFacts::COMPUTED);
        word.store((facts | SubtreeFacts::COMPUTED).0, Ordering::Relaxed);
        return facts;
    }
    if let Some(facts) = cached_unpublished_subtree_facts(n) {
        return facts;
    }
    let facts = subtree_facts_of_data(n, d).without(SubtreeFacts::COMPUTED);
    cache_unpublished_subtree_facts(n, facts);
    facts
}

// Go: ast.go computeSubtreeFacts overrides (ast.go:1544 to ast.go:2748) and
// ast_generated.go computeSubtreeFacts methods.
// PORT: the Go per-type methods are merged into one match. Types without an
// override use Go `(*NodeDefault).computeSubtreeFacts`, which is None.
// `(*TypeSyntaxBase).computeSubtreeFacts` is in `subtree_facts_with_data`.
/// Go `computeSubtreeFacts` over the data `d` of `n`, read in place, for a
/// node that is not type syntax (`non_type_subtree_facts`).
fn subtree_facts_of_data(n: Node, d: NodeData<'_>) -> SubtreeFacts {
    let f = n.file_index();
    macro_rules! p {
        ($x:expr) => {
            propagate_subtree_facts(req(f, $x))
        };
    }
    macro_rules! po {
        ($x:expr) => {
            propagate_subtree_facts(opt(f, $x))
        };
    }
    macro_rules! pl {
        ($x:expr) => {
            propagate_node_list(DataList::req(n, &$x), propagate_subtree_facts)
        };
    }
    macro_rules! pol {
        ($x:expr) => {
            propagate_node_list(DataList::opt(n, &$x), propagate_subtree_facts)
        };
    }
    macro_rules! pm {
        ($x:expr) => {
            propagate_node_list(DataList::mods(n, &$x), propagate_subtree_facts)
        };
    }
    macro_rules! el {
        ($x:expr) => {
            propagate_eraseable_list(DataList::opt(n, &$x))
        };
    }
    macro_rules! e {
        ($x:expr) => {
            propagate_eraseable(opt(f, $x))
        };
    }
    let ambient = |m: &Option<crate::astdata::ModifierList>| {
        in_place_modifier_flags(n, m).intersects(ModifierFlags::AMBIENT)
    };
    let jsx = SubtreeFacts::SUBTREE_CONTAINS_JSX;
    match d {
        // Go: ast.go:1548 (*Token).computeSubtreeFacts
        NodeData::Token(_) => match n.kind() {
            SyntaxKind::UsingKeyword => SubtreeFacts::SUBTREE_CONTAINS_USING,
            SyntaxKind::PublicKeyword
            | SyntaxKind::PrivateKeyword
            | SyntaxKind::ProtectedKeyword
            | SyntaxKind::ReadonlyKeyword
            | SyntaxKind::AbstractKeyword
            | SyntaxKind::DeclareKeyword
            | SyntaxKind::ConstKeyword
            | SyntaxKind::AnyKeyword
            | SyntaxKind::NumberKeyword
            | SyntaxKind::BigIntKeyword
            | SyntaxKind::NeverKeyword
            | SyntaxKind::ObjectKeyword
            | SyntaxKind::InKeyword
            | SyntaxKind::OutKeyword
            | SyntaxKind::OverrideKeyword
            | SyntaxKind::StringKeyword
            | SyntaxKind::BooleanKeyword
            | SyntaxKind::SymbolKeyword
            | SyntaxKind::VoidKeyword
            | SyntaxKind::UnknownKeyword
            | SyntaxKind::UndefinedKeyword
            | SyntaxKind::ExportKeyword => TS,
            SyntaxKind::AccessorKeyword => SubtreeFacts::SUBTREE_CONTAINS_CLASS_FIELDS,
            SyntaxKind::AsyncKeyword => SubtreeFacts::SUBTREE_CONTAINS_ANY_AWAIT,
            SyntaxKind::SuperKeyword => SubtreeFacts::SUBTREE_CONTAINS_LEXICAL_SUPER,
            SyntaxKind::ThisKeyword => SubtreeFacts::SUBTREE_CONTAINS_LEXICAL_THIS,
            SyntaxKind::AsteriskAsteriskToken | SyntaxKind::AsteriskAsteriskEqualsToken => {
                SubtreeFacts::SUBTREE_CONTAINS_EXPONENTIATION_OPERATOR
            }
            SyntaxKind::QuestionQuestionToken => SubtreeFacts::SUBTREE_CONTAINS_NULLISH_COALESCING,
            SyntaxKind::QuestionDotToken => SubtreeFacts::SUBTREE_CONTAINS_OPTIONAL_CHAINING,
            SyntaxKind::QuestionQuestionEqualsToken
            | SyntaxKind::BarBarEqualsToken
            | SyntaxKind::AmpersandAmpersandEqualsToken => {
                SubtreeFacts::SUBTREE_CONTAINS_LOGICAL_ASSIGNMENTS
            }
            _ => NONE_FACTS,
        },
        // Go: ast.go:1595 (*PrivateIdentifier).computeSubtreeFacts
        NodeData::PrivateIdentifier(_) => SubtreeFacts::SUBTREE_CONTAINS_CLASS_FIELDS,
        // Go: ast.go:1603 (*Decorator).computeSubtreeFacts
        NodeData::Decorator(d) => p!(d.expression) | TS | SubtreeFacts::SUBTREE_CONTAINS_DECORATORS,
        // Go: ast.go:1609 (*ForInOrOfStatement).computeSubtreeFacts
        NodeData::ForInOrOfStatement(d) => {
            p!(d.initializer)
                | p!(d.expression)
                | p!(d.statement)
                | facts_if(
                    d.await_modifier.is_some(),
                    SubtreeFacts::SUBTREE_CONTAINS_FOR_AWAIT_OR_ASYNC_GENERATOR,
                )
        }
        // Go: ast.go:1616 (*ReturnStatement).computeSubtreeFacts
        NodeData::ReturnStatement(d) => {
            po!(d.expression) | SubtreeFacts::SUBTREE_CONTAINS_FOR_AWAIT_OR_ASYNC_GENERATOR
        }
        // Go: ast.go:1621 (*CatchClause).computeSubtreeFacts
        NodeData::CatchClause(d) => {
            po!(d.variable_declaration)
                | p!(d.block)
                | facts_if(
                    d.variable_declaration.is_none(),
                    SubtreeFacts::SUBTREE_CONTAINS_MISSING_CATCH_CLAUSE_VARIABLE,
                )
        }
        // Go: ast.go:1634 (*VariableStatement).computeSubtreeFacts
        NodeData::VariableStatement(d) => {
            if ambient(&d.modifiers) {
                TS
            } else {
                pm!(d.modifiers) | p!(d.declaration_list)
            }
        }
        // Go: ast.go:1643 (*VariableDeclaration).computeSubtreeFacts
        NodeData::VariableDeclaration(d) => {
            p!(d.name) | e!(d.exclamation_token) | e!(d.type_) | po!(d.initializer)
        }
        // Go: ast.go:1650 (*VariableDeclarationList).computeSubtreeFacts
        NodeData::VariableDeclarationList(d) => {
            pl!(d.declarations)
                | facts_if(
                    n.flags().intersects(NodeFlags::USING),
                    SubtreeFacts::SUBTREE_CONTAINS_USING,
                )
        }
        // Go: ast.go:1659 (*BindingPattern).computeSubtreeFacts
        NodeData::BindingPattern(d) => match n.kind() {
            SyntaxKind::ObjectBindingPattern => propagate_node_list(
                DataList::req(n, &d.elements),
                propagate_object_binding_element,
            ),
            SyntaxKind::ArrayBindingPattern => {
                propagate_node_list(DataList::req(n, &d.elements), propagate_binding_element)
            }
            _ => NONE_FACTS,
        },
        // Go: ast.go:1674 (*ParameterDeclaration).computeSubtreeFacts
        NodeData::ParameterDeclaration(d) => {
            let name = req(f, d.name);
            if name.is_some() && is_this_identifier(name) {
                TS
            } else {
                pm!(d.modifiers)
                    | propagate_subtree_facts(name)
                    | e!(d.question_token)
                    | e!(d.type_)
                    | po!(d.initializer)
            }
        }
        // Go: ast.go:1690 (*BindingElement).computeSubtreeFacts
        NodeData::BindingElement(d) => {
            po!(d.property_name)
                | po!(d.name)
                | po!(d.initializer)
                | facts_if(
                    d.dot_dot_dot_token.is_some(),
                    SubtreeFacts::SUBTREE_CONTAINS_REST_OR_SPREAD,
                )
        }
        // Go: ast.go:1697 (*FunctionDeclaration).computeSubtreeFacts
        NodeData::FunctionDeclaration(d) => {
            let flags = n.modifier_flags();
            if d.body.is_none() || flags.intersects(ModifierFlags::AMBIENT) {
                TS
            } else {
                let is_async = flags.intersects(ModifierFlags::ASYNC);
                let is_generator = d.asterisk_token.is_some();
                pm!(d.modifiers)
                    | po!(d.asterisk_token)
                    | po!(d.name)
                    | el!(d.type_parameters)
                    | pl!(d.parameters)
                    | e!(d.type_)
                    | e!(d.full_signature)
                    | po!(d.body)
                    | facts_if(
                        is_async && is_generator,
                        SubtreeFacts::SUBTREE_CONTAINS_FOR_AWAIT_OR_ASYNC_GENERATOR,
                    )
                    | facts_if(
                        is_async && !is_generator,
                        SubtreeFacts::SUBTREE_CONTAINS_ANY_AWAIT,
                    )
            }
        }
        // Go: ast.go:1726 (*ClassLikeBase).computeSubtreeFacts
        NodeData::ClassDeclaration(d) => {
            if ambient(&d.modifiers) {
                TS
            } else {
                pm!(d.modifiers)
                    | po!(d.name)
                    | el!(d.type_parameters)
                    | pol!(d.heritage_clauses)
                    | pl!(d.members)
            }
        }
        NodeData::ClassExpression(d) => {
            if ambient(&d.modifiers) {
                TS
            } else {
                pm!(d.modifiers)
                    | po!(d.name)
                    | el!(d.type_parameters)
                    | pol!(d.heritage_clauses)
                    | pl!(d.members)
            }
        }
        // Go: ast.go:1746 (*HeritageClause).computeSubtreeFacts
        NodeData::HeritageClause(d) => match d.token {
            SyntaxKind::ExtendsKeyword => pl!(d.types),
            SyntaxKind::ImplementsKeyword => TS,
            _ => NONE_FACTS,
        },
        // Go: ast.go:1761 (*EnumMember).computeSubtreeFacts
        NodeData::EnumMember(d) => p!(d.name) | po!(d.initializer) | TS,
        // Go: ast.go:1767 (*EnumDeclaration).computeSubtreeFacts
        NodeData::EnumDeclaration(d) => {
            if ambient(&d.modifiers) {
                TS
            } else {
                pm!(d.modifiers) | p!(d.name) | pl!(d.members) | TS
            }
        }
        // Go: ast.go:1778 (*ModuleDeclaration).computeSubtreeFacts
        NodeData::ModuleDeclaration(d) => {
            if n.modifier_flags().intersects(ModifierFlags::AMBIENT) {
                TS
            } else {
                pm!(d.modifiers) | p!(d.name) | po!(d.body) | TS
            }
        }
        // Go: ast.go:1793 (*ImportEqualsDeclaration).computeSubtreeFacts
        NodeData::ImportEqualsDeclaration(d) => {
            if d.is_type_only || !is_external_module_reference(req(f, d.module_reference)) {
                TS
            } else {
                pm!(d.modifiers) | p!(d.name) | p!(d.module_reference)
            }
        }
        // Go: ast.go:1807 (*ImportSpecifier).computeSubtreeFacts
        NodeData::ImportSpecifier(d) => {
            if d.is_type_only {
                TS
            } else {
                po!(d.property_name) | p!(d.name)
            }
        }
        // Go: ast.go:1816 (*ImportClause).computeSubtreeFacts
        NodeData::ImportClause(d) => {
            if d.phase_modifier == Some(SyntaxKind::TypeKeyword) {
                TS
            } else {
                po!(d.name) | po!(d.named_bindings)
            }
        }
        // Go: ast.go:1825 (*ExportAssignment).computeSubtreeFacts
        NodeData::ExportAssignment(d) => {
            pm!(d.modifiers) | po!(d.type_) | p!(d.expression) | facts_if(d.is_export_equals, TS)
        }
        // Go: ast.go:1833 (*ExportDeclaration).computeSubtreeFacts
        NodeData::ExportDeclaration(d) => {
            pm!(d.modifiers)
                | po!(d.export_clause)
                | po!(d.module_specifier)
                | po!(d.attributes)
                | facts_if(d.is_type_only, TS)
        }
        // Go: ast.go:1841 (*ExportSpecifier).computeSubtreeFacts
        NodeData::ExportSpecifier(d) => {
            if d.is_type_only {
                TS
            } else {
                po!(d.property_name) | p!(d.name)
            }
        }
        // Go: ast.go:1856 (*ConstructorDeclaration).computeSubtreeFacts
        NodeData::ConstructorDeclaration(d) => {
            if d.body.is_none() {
                TS
            } else {
                pm!(d.modifiers)
                    | el!(d.type_parameters)
                    | pl!(d.parameters)
                    | e!(d.type_)
                    | e!(d.full_signature)
                    | po!(d.body)
            }
        }
        // Go: ast.go:1875 (*AccessorDeclarationBase).computeSubtreeFacts
        NodeData::GetAccessorDeclaration(d) => {
            if d.body.is_none() {
                TS
            } else {
                pm!(d.modifiers)
                    | p!(d.name)
                    | el!(d.type_parameters)
                    | pl!(d.parameters)
                    | e!(d.type_)
                    | e!(d.full_signature)
                    | po!(d.body)
            }
        }
        NodeData::SetAccessorDeclaration(d) => {
            if d.body.is_none() {
                TS
            } else {
                pm!(d.modifiers)
                    | p!(d.name)
                    | el!(d.type_parameters)
                    | pl!(d.parameters)
                    | e!(d.type_)
                    | e!(d.full_signature)
                    | po!(d.body)
            }
        }
        // Go: ast.go:1894 (*MethodDeclaration).computeSubtreeFacts
        NodeData::MethodDeclaration(d) => {
            if d.body.is_none() {
                TS
            } else {
                let is_async = modifiers_have_async(n, &d.modifiers);
                let is_generator = d.asterisk_token.is_some();
                pm!(d.modifiers)
                    | po!(d.asterisk_token)
                    | p!(d.name)
                    | e!(d.postfix_token)
                    | el!(d.type_parameters)
                    | pl!(d.parameters)
                    | po!(d.body)
                    | e!(d.type_)
                    | e!(d.full_signature)
                    | facts_if(
                        is_async && is_generator,
                        SubtreeFacts::SUBTREE_CONTAINS_FOR_AWAIT_OR_ASYNC_GENERATOR,
                    )
                    | facts_if(
                        is_async && !is_generator,
                        SubtreeFacts::SUBTREE_CONTAINS_ANY_AWAIT,
                    )
            }
        }
        // Go: ast.go:1919 (*PropertyDeclaration).computeSubtreeFacts
        NodeData::PropertyDeclaration(d) => {
            pm!(d.modifiers)
                | p!(d.name)
                | e!(d.postfix_token)
                | e!(d.type_)
                | po!(d.initializer)
                | SubtreeFacts::SUBTREE_CONTAINS_CLASS_FIELDS
        }
        // Go: ast.go:1933 (*ClassStaticBlockDeclaration).computeSubtreeFacts
        NodeData::ClassStaticBlockDeclaration(d) => {
            pm!(d.modifiers) | p!(d.body) | SubtreeFacts::SUBTREE_CONTAINS_CLASS_FIELDS
        }
        // Go: ast.go:1939 (*KeywordExpression).computeSubtreeFacts
        NodeData::KeywordExpression(_) => match n.kind() {
            SyntaxKind::ThisKeyword => SubtreeFacts::SUBTREE_CONTAINS_LEXICAL_THIS,
            SyntaxKind::SuperKeyword => SubtreeFacts::SUBTREE_CONTAINS_LEXICAL_SUPER,
            _ => NONE_FACTS,
        },
        // Go: ast.go:1953 (*BigIntLiteral).computeSubtreeFacts
        NodeData::BigIntLiteral(_) => NONE_FACTS,
        // Go: ast.go:1957 (*Identifier).computeSubtreeFacts
        NodeData::Identifier(_) => SubtreeFacts::SUBTREE_CONTAINS_IDENTIFIER,
        // Go: ast.go:1961 (*NoSubstitutionTemplateLiteral).computeSubtreeFacts
        NodeData::NoSubstitutionTemplateLiteral(_) => facts_if(
            n.template_flags()
                .intersects(TokenFlags::CONTAINS_INVALID_ESCAPE),
            SubtreeFacts::SUBTREE_CONTAINS_INVALID_TEMPLATE_ESCAPE,
        ),
        // Go: ast.go:1968 (*BinaryExpression).computeSubtreeFacts
        NodeData::BinaryExpression(d) => {
            let left = req(f, d.left);
            let operator_kind = req(f, d.operator_token).kind();
            let mut facts = pm!(d.modifiers)
                | propagate_subtree_facts(left)
                | po!(d.type_)
                | p!(d.operator_token)
                | p!(d.right)
                | facts_if(
                    operator_kind == SyntaxKind::InKeyword && is_private_identifier(left),
                    SubtreeFacts::SUBTREE_CONTAINS_CLASS_FIELDS
                        | SubtreeFacts::SUBTREE_CONTAINS_PRIVATE_IDENTIFIER_IN_EXPRESSION,
                );
            if operator_kind == SyntaxKind::EqualsToken
                && (is_object_literal_expression(left) || is_array_literal_expression(left))
                && contains_object_rest_or_spread(left)
            {
                facts |= SubtreeFacts::SUBTREE_CONTAINS_OBJECT_REST_OR_SPREAD;
            }
            facts
        }
        // Go: ast.go:1985 (*YieldExpression).computeSubtreeFacts
        NodeData::YieldExpression(d) => {
            po!(d.expression) | SubtreeFacts::SUBTREE_CONTAINS_FOR_AWAIT_OR_ASYNC_GENERATOR
        }
        // Go: ast.go:1989 (*ArrowFunction).computeSubtreeFacts
        NodeData::ArrowFunction(d) => {
            pm!(d.modifiers)
                | el!(d.type_parameters)
                | pl!(d.parameters)
                | e!(d.type_)
                | e!(d.full_signature)
                | p!(d.body)
                | facts_if(
                    n.modifier_flags().intersects(ModifierFlags::ASYNC),
                    SubtreeFacts::SUBTREE_CONTAINS_ANY_AWAIT,
                )
        }
        // Go: ast.go:2003 (*FunctionExpression).computeSubtreeFacts
        NodeData::FunctionExpression(d) => {
            let is_async = modifiers_have_async(n, &d.modifiers);
            let is_generator = d.asterisk_token.is_some();
            pm!(d.modifiers)
                | po!(d.asterisk_token)
                | po!(d.name)
                | el!(d.type_parameters)
                | pl!(d.parameters)
                | e!(d.type_)
                | e!(d.full_signature)
                | p!(d.body)
                | facts_if(
                    is_async && is_generator,
                    SubtreeFacts::SUBTREE_CONTAINS_FOR_AWAIT_OR_ASYNC_GENERATOR,
                )
                | facts_if(
                    is_async && !is_generator,
                    SubtreeFacts::SUBTREE_CONTAINS_ANY_AWAIT,
                )
        }
        // Go: ast.go:2022 (*AsExpression).computeSubtreeFacts
        NodeData::AsExpression(d) => p!(d.expression) | TS,
        // Go: ast.go:2030 (*SatisfiesExpression).computeSubtreeFacts
        NodeData::SatisfiesExpression(d) => p!(d.expression) | TS,
        // Go: ast.go:2038 (*PropertyAccessExpression).computeSubtreeFacts
        NodeData::PropertyAccessExpression(d) => {
            let name = req(f, d.name);
            p!(d.expression)
                | po!(d.question_dot_token)
                | propagate_subtree_facts(name)
                | facts_if(
                    !is_identifier(name),
                    SubtreeFacts::SUBTREE_CONTAINS_PRIVATE_IDENTIFIER_IN_EXPRESSION,
                )
        }
        // Go: ast.go:2056 (*CallExpression).computeSubtreeFacts
        NodeData::CallExpression(d) => {
            p!(d.expression)
                | po!(d.question_dot_token)
                | el!(d.type_arguments)
                | pl!(d.arguments)
                | facts_if(
                    is_import_call(n),
                    SubtreeFacts::SUBTREE_CONTAINS_DYNAMIC_IMPORT,
                )
        }
        // Go: ast.go:2068 (*NewExpression).computeSubtreeFacts
        NodeData::NewExpression(d) => p!(d.expression) | el!(d.type_arguments) | pol!(d.arguments),
        // Go: ast.go:2078 (*MetaProperty).computeSubtreeFacts
        NodeData::MetaProperty(d) => p!(d.name).without(SubtreeFacts::SUBTREE_CONTAINS_IDENTIFIER),
        // Go: ast.go:2082 (*NonNullExpression).computeSubtreeFacts
        NodeData::NonNullExpression(d) => p!(d.expression) | TS,
        // Go: ast.go:2086 (*SpreadElement).computeSubtreeFacts
        NodeData::SpreadElement(d) => {
            p!(d.expression) | SubtreeFacts::SUBTREE_CONTAINS_REST_OR_SPREAD
        }
        // Go: ast.go:2090 (*TaggedTemplateExpression).computeSubtreeFacts
        NodeData::TaggedTemplateExpression(d) => {
            p!(d.tag) | po!(d.question_dot_token) | el!(d.type_arguments) | p!(d.template)
        }
        // Go: ast.go:2107 (*SpreadAssignment).computeSubtreeFacts
        NodeData::SpreadAssignment(d) => {
            p!(d.expression)
                | SubtreeFacts::SUBTREE_CONTAINS_ES_OBJECT_REST_OR_SPREAD
                | SubtreeFacts::SUBTREE_CONTAINS_OBJECT_REST_OR_SPREAD
        }
        // Go: ast.go:2111 (*PropertyAssignment).computeSubtreeFacts
        NodeData::PropertyAssignment(d) => p!(d.name) | po!(d.type_) | p!(d.initializer),
        // Go: ast.go:2117 (*ShorthandPropertyAssignment).computeSubtreeFacts
        NodeData::ShorthandPropertyAssignment(d) => {
            p!(d.name) | po!(d.type_) | po!(d.object_assignment_initializer) | TS
        }
        // Go: ast.go:2124 (*AwaitExpression).computeSubtreeFacts
        NodeData::AwaitExpression(d) => {
            p!(d.expression)
                | SubtreeFacts::SUBTREE_CONTAINS_AWAIT
                | SubtreeFacts::SUBTREE_CONTAINS_ANY_AWAIT
                | SubtreeFacts::SUBTREE_CONTAINS_FOR_AWAIT_OR_ASYNC_GENERATOR
        }
        // Go: ast.go:2129 (*TypeAssertion).computeSubtreeFacts
        NodeData::TypeAssertion(d) => p!(d.expression) | TS,
        // Go: ast.go:2137 (*ExpressionWithTypeArguments).computeSubtreeFacts
        NodeData::ExpressionWithTypeArguments(d) => p!(d.expression) | el!(d.type_arguments),
        // Go: ast.go:2183 (*TemplateHead).computeSubtreeFacts
        NodeData::TemplateHead(_) => facts_if(
            n.template_flags()
                .intersects(TokenFlags::CONTAINS_INVALID_ESCAPE),
            SubtreeFacts::SUBTREE_CONTAINS_INVALID_TEMPLATE_ESCAPE,
        ),
        // Go: ast.go:2190 (*TemplateMiddle).computeSubtreeFacts
        NodeData::TemplateMiddle(_) => facts_if(
            n.template_flags()
                .intersects(TokenFlags::CONTAINS_INVALID_ESCAPE),
            SubtreeFacts::SUBTREE_CONTAINS_INVALID_TEMPLATE_ESCAPE,
        ),
        // Go: ast.go:2197 (*TemplateTail).computeSubtreeFacts
        NodeData::TemplateTail(_) => facts_if(
            n.template_flags()
                .intersects(TokenFlags::CONTAINS_INVALID_ESCAPE),
            SubtreeFacts::SUBTREE_CONTAINS_INVALID_TEMPLATE_ESCAPE,
        ),
        // Go: ast.go:2204 (*JsxElement).computeSubtreeFacts
        NodeData::JsxElement(d) => {
            p!(d.opening_element) | pl!(d.children) | p!(d.closing_element) | jsx
        }
        // Go: ast.go:2211 (*JsxAttributes).computeSubtreeFacts
        NodeData::JsxAttributes(d) => pl!(d.properties) | jsx,
        // Go: ast.go:2216 (*JsxNamespacedName).computeSubtreeFacts
        NodeData::JsxNamespacedName(d) => p!(d.namespace) | p!(d.name) | jsx,
        // Go: ast.go:2222 (*JsxOpeningElement).computeSubtreeFacts
        NodeData::JsxOpeningElement(d) => {
            p!(d.tag_name) | el!(d.type_arguments) | p!(d.attributes) | jsx
        }
        // Go: ast.go:2229 (*JsxSelfClosingElement).computeSubtreeFacts
        NodeData::JsxSelfClosingElement(d) => {
            p!(d.tag_name) | el!(d.type_arguments) | p!(d.attributes) | jsx
        }
        // Go: ast.go:2236 (*JsxFragment).computeSubtreeFacts
        NodeData::JsxFragment(d) => pl!(d.children) | jsx,
        // Go: ast.go:2241 and ast.go:2245 Jsx fragment tokens
        NodeData::JsxOpeningFragment(_) | NodeData::JsxClosingFragment(_) => jsx,
        // Go: ast.go:2249 (*JsxAttribute).computeSubtreeFacts
        NodeData::JsxAttribute(d) => p!(d.name) | po!(d.initializer) | jsx,
        // Go: ast.go:2255 (*JsxSpreadAttribute).computeSubtreeFacts
        NodeData::JsxSpreadAttribute(d) => p!(d.expression) | jsx,
        // Go: ast.go:2259 (*JsxClosingElement).computeSubtreeFacts
        NodeData::JsxClosingElement(d) => p!(d.tag_name) | jsx,
        // Go: ast.go:2263 (*JsxExpression).computeSubtreeFacts
        NodeData::JsxExpression(d) => po!(d.expression) | jsx,
        // Go: ast.go:2267 (*JsxText).computeSubtreeFacts
        NodeData::JsxText(_) => jsx,
        // Go: ast.go:2748 (*SourceFile).computeSubtreeFacts
        NodeData::SourceFile(d) => pl!(d.statements),
        // Go: ast_generated.go computeSubtreeFacts methods
        NodeData::QualifiedName(d) => p!(d.left) | p!(d.right),
        NodeData::ComputedPropertyName(d) => p!(d.expression),
        NodeData::IfStatement(d) => p!(d.expression) | p!(d.then_statement) | po!(d.else_statement),
        NodeData::DoStatement(d) => p!(d.statement) | p!(d.expression),
        NodeData::WhileStatement(d) => p!(d.expression) | p!(d.statement),
        NodeData::ForStatement(d) => {
            po!(d.initializer) | po!(d.condition) | po!(d.incrementor) | p!(d.statement)
        }
        NodeData::WithStatement(d) => p!(d.expression) | p!(d.statement),
        NodeData::SwitchStatement(d) => p!(d.expression) | p!(d.case_block),
        NodeData::CaseBlock(d) => pl!(d.clauses),
        NodeData::CaseOrDefaultClause(d) => {
            propagate_subtree_facts(case_expression(n, f, d.expression)) | pl!(d.statements)
        }
        NodeData::ThrowStatement(d) => p!(d.expression),
        NodeData::TryStatement(d) => p!(d.try_block) | po!(d.catch_clause) | po!(d.finally_block),
        NodeData::LabeledStatement(d) => p!(d.label) | p!(d.statement),
        NodeData::ExpressionStatement(d) => p!(d.expression),
        NodeData::Block(d) => pl!(d.statements),
        NodeData::ModuleBlock(d) => pl!(d.statements),
        NodeData::ImportDeclaration(d) => {
            pm!(d.modifiers) | po!(d.import_clause) | p!(d.module_specifier) | po!(d.attributes)
        }
        NodeData::ExternalModuleReference(d) => p!(d.expression),
        NodeData::NamespaceImport(d) => p!(d.name),
        NodeData::NamedImports(d) => pl!(d.elements),
        NodeData::NamespaceExport(d) => p!(d.name),
        NodeData::NamedExports(d) => pl!(d.elements),
        NodeData::PrefixUnaryExpression(d) => p!(d.operand),
        NodeData::PostfixUnaryExpression(d) => p!(d.operand),
        NodeData::ConditionalExpression(d) => {
            p!(d.condition)
                | p!(d.question_token)
                | p!(d.when_true)
                | p!(d.colon_token)
                | p!(d.when_false)
        }
        NodeData::ElementAccessExpression(d) => {
            p!(d.expression) | po!(d.question_dot_token) | p!(d.argument_expression)
        }
        NodeData::TemplateExpression(d) => p!(d.head) | pl!(d.template_spans),
        NodeData::TemplateSpan(d) => p!(d.expression) | p!(d.literal),
        NodeData::ParenthesizedExpression(d) => p!(d.expression),
        NodeData::ArrayLiteralExpression(d) => pl!(d.elements),
        NodeData::ObjectLiteralExpression(d) => pl!(d.properties),
        NodeData::DeleteExpression(d) => p!(d.expression),
        NodeData::TypeOfExpression(d) => p!(d.expression),
        NodeData::VoidExpression(d) => p!(d.expression),
        NodeData::ImportAttribute(d) => p!(d.name) | p!(d.value),
        NodeData::ImportAttributes(d) => pl!(d.attributes),
        NodeData::PartiallyEmittedExpression(d) => p!(d.expression),
        NodeData::SyntheticReferenceExpression(d) => p!(d.expression) | p!(d.this_arg),
        // Go: ast.go:1169 (*NodeDefault).computeSubtreeFacts
        _ => NONE_FACTS,
    }
}

/// Go `node.modifiers != nil && node.modifiers.ModifierFlags&ModifierFlagsAsync != 0`
/// for the modifier list field `m` of the data of node `n`.
fn modifiers_have_async(n: Node, m: &Option<crate::astdata::ModifierList>) -> bool {
    in_place_modifier_flags(n, m).intersects(ModifierFlags::ASYNC)
}

// ──────────────────────────────────────────────────────────────────────
// Access kinds
// ──────────────────────────────────────────────────────────────────────

// Go: ast.go:1193 IsWriteOnlyAccess
#[must_use]
pub fn is_write_only_access(node: Node) -> bool {
    access_kind(node) == AccessKind::WRITE
}

// Go: ast.go:1197 IsWriteAccess
#[must_use]
pub fn is_write_access(node: Node) -> bool {
    access_kind(node) != AccessKind::READ
}

// Go: ast.go:1201 IsWriteAccessForReference
#[must_use]
pub fn is_write_access_for_reference(node: Node) -> bool {
    let decl = get_declaration_from_name(node);
    (decl.is_some() && declaration_is_write_access(decl))
        || node.kind() == SyntaxKind::DefaultKeyword
        || is_write_access(node)
}

// Go: ast.go:1206 GetDeclarationFromName
/// The declaration that `name` names, or nil.
#[must_use]
pub fn get_declaration_from_name(name: Node) -> Node {
    if name.is_nil() || name.parent().is_nil() {
        return Node::NIL;
    }
    let parent = name.parent();
    match name.kind() {
        SyntaxKind::StringLiteral
        | SyntaxKind::NoSubstitutionTemplateLiteral
        | SyntaxKind::NumericLiteral
        | SyntaxKind::Identifier => {
            // Go: the literal kinds check for a computed property name, then
            // fall through to the Identifier case.
            if name.kind() != SyntaxKind::Identifier && is_computed_property_name(parent) {
                return parent.parent();
            }
            if is_declaration(parent) {
                if parent.name() == name {
                    return parent;
                }
                return Node::NIL;
            }
            if is_qualified_name(parent) {
                let tag = parent.parent();
                if is_js_doc_parameter_tag(tag) && tag.name() == parent {
                    return tag;
                }
                return Node::NIL;
            }
            let bin_exp = parent.parent();
            if is_binary_expression(bin_exp)
                && get_assignment_declaration_kind(bin_exp) != JSDeclarationKind::NONE
            {
                // (binExp.left as BindableStaticNameExpression).symbol || binExp.symbol
                let left = bin_exp.left();
                let left_has_symbol = left.is_some() && left.symbol().is_some();
                if (left_has_symbol || bin_exp.symbol().is_some())
                    && get_name_of_declaration(bin_exp) == name
                {
                    return bin_exp;
                }
            }
        }
        SyntaxKind::PrivateIdentifier => {
            if is_declaration(parent) && parent.name() == name {
                return parent;
            }
        }
        _ => {}
    }
    Node::NIL
}

// Go: ast.go:1252 declarationIsWriteAccess
fn declaration_is_write_access(decl: Node) -> bool {
    if decl.is_nil() {
        return false;
    }
    // Consider anything in an ambient declaration to be a write access since it may be coming from JS.
    if !decl.parser_flags(NodeFlags::AMBIENT).is_empty() {
        return true;
    }
    match decl.kind() {
        SyntaxKind::BinaryExpression
        | SyntaxKind::BindingElement
        | SyntaxKind::ClassDeclaration
        | SyntaxKind::ClassExpression
        | SyntaxKind::DefaultKeyword
        | SyntaxKind::EnumDeclaration
        | SyntaxKind::EnumMember
        | SyntaxKind::ExportSpecifier
        | SyntaxKind::ImportClause
        | SyntaxKind::ImportEqualsDeclaration
        | SyntaxKind::ImportSpecifier
        | SyntaxKind::InterfaceDeclaration
        | SyntaxKind::JsDocCallbackTag
        | SyntaxKind::JsDocTypedefTag
        | SyntaxKind::JsxAttribute
        | SyntaxKind::ModuleDeclaration
        | SyntaxKind::NamespaceExportDeclaration
        | SyntaxKind::NamespaceImport
        | SyntaxKind::NamespaceExport
        | SyntaxKind::Parameter
        | SyntaxKind::ShorthandPropertyAssignment
        | SyntaxKind::TypeAliasDeclaration
        | SyntaxKind::JsTypeAliasDeclaration
        | SyntaxKind::TypeParameter => true,
        // In `({ x: y } = 0);`, `x` is not a write access.
        SyntaxKind::PropertyAssignment => {
            !is_array_literal_or_object_literal_destructuring_pattern(decl.parent())
        }
        // Functions are writes if they provide a value (have a body).
        SyntaxKind::FunctionDeclaration
        | SyntaxKind::FunctionExpression
        | SyntaxKind::Constructor
        | SyntaxKind::MethodDeclaration
        | SyntaxKind::GetAccessor
        | SyntaxKind::SetAccessor => decl.body().is_some(),
        // Variables and properties are writes if they have an initializer or are in a catch clause.
        SyntaxKind::VariableDeclaration | SyntaxKind::PropertyDeclaration => {
            decl.initializer().is_some() || is_catch_clause(decl.parent())
        }
        SyntaxKind::MethodSignature
        | SyntaxKind::PropertySignature
        | SyntaxKind::JsDocPropertyTag
        | SyntaxKind::JsDocParameterTag => false,
        // Preserve TS behavior: crash on unexpected kinds.
        _ => panic!("Unhandled case in declarationIsWriteAccess"),
    }
}

// Go: ast.go:1331 IsArrayLiteralOrObjectLiteralDestructuringPattern
#[must_use]
pub fn is_array_literal_or_object_literal_destructuring_pattern(node: Node) -> bool {
    if !(is_array_literal_expression(node) || is_object_literal_expression(node)) {
        return false;
    }
    let parent = node.parent();
    // [a, b, c] = someExpression;
    if is_binary_expression(parent)
        && parent.left() == node
        && parent.operator_token().kind() == SyntaxKind::EqualsToken
    {
        return true;
    }
    // for ([a, b, c] of expression)
    if is_for_of_statement(parent) && parent.initializer() == node {
        return true;
    }
    // {x, a: {a, b, c} } = someExpression
    if is_property_assignment(parent) {
        return is_array_literal_or_object_literal_destructuring_pattern(parent.parent());
    }
    // [x, [a, b, c] ] = someExpression
    is_array_literal_or_object_literal_destructuring_pattern(parent)
}

// Go: ast.go:1355 accessKind
fn access_kind(node: Node) -> AccessKind {
    access_kind_ref(NodeRef::new(node))
}

/// `access_kind` of the view of a node.
// PERF: checkid1. The parent, its kind and the parents of the recursive
// steps are read through views (`NodeRef`): a parent in the same store
// costs no registry lookup. Go reads struct fields. Same tests, same order.
fn access_kind_ref(node: NodeRef) -> AccessKind {
    let parent = node.parent();
    if parent.is_nil() {
        return AccessKind::READ;
    }
    match parent.kind() {
        SyntaxKind::ParenthesizedExpression | SyntaxKind::ArrayLiteralExpression => {
            access_kind_ref(parent)
        }
        // PERF: noderead2. The operator and the left operand come from one
        // node data read of the parent (`NodeRef::data`, `frozen_data_ids`),
        // and the operator token kind is read once. Go reads the fields.
        SyntaxKind::PrefixUnaryExpression | SyntaxKind::PostfixUnaryExpression => {
            let operator = match parent.data() {
                Some(NodeData::PrefixUnaryExpression(d)) => d.operator,
                Some(NodeData::PostfixUnaryExpression(d)) => d.operator,
                _ => parent.node().operator(),
            };
            debug_assert_eq!(operator, parent.node().operator());
            if operator == SyntaxKind::PlusPlusToken || operator == SyntaxKind::MinusMinusToken {
                AccessKind::READ_WRITE
            } else {
                AccessKind::READ
            }
        }
        SyntaxKind::BinaryExpression => {
            let (left, operator_token) = match frozen_data_ids(parent.node()) {
                Some((NodeData::BinaryExpression(d), ids)) => {
                    (ids.node(d.left), ids.node(d.operator_token))
                }
                _ => (parent.node().left(), parent.node().operator_token()),
            };
            if left == node.node() {
                let operator = operator_token.kind();
                if is_assignment_operator(operator) {
                    if operator == SyntaxKind::EqualsToken {
                        return AccessKind::WRITE;
                    }
                    return AccessKind::READ_WRITE;
                }
            }
            AccessKind::READ
        }
        SyntaxKind::PropertyAccessExpression => {
            if parent.name() != node.node() {
                return AccessKind::READ;
            }
            access_kind_ref(parent)
        }
        SyntaxKind::PropertyAssignment => {
            let parent_access = access_kind_ref(parent.parent());
            // In `({ x: varname }) = { x: 1 }`, the left `x` is a read, the right `x` is a write.
            if node.node() == parent.name() {
                return reverse_access_kind(parent_access);
            }
            parent_access
        }
        SyntaxKind::ShorthandPropertyAssignment => {
            // Assume it's the local variable being accessed, since we don't check public properties for --noUnusedLocals.
            if node.node() == parent.node().object_assignment_initializer() {
                return AccessKind::READ;
            }
            access_kind_ref(parent.parent())
        }
        SyntaxKind::ForInStatement | SyntaxKind::ForOfStatement => {
            if node.node() == parent.initializer() {
                AccessKind::WRITE
            } else {
                AccessKind::READ
            }
        }
        _ => AccessKind::READ,
    }
}

// Go: ast.go:1416 reverseAccessKind
fn reverse_access_kind(a: AccessKind) -> AccessKind {
    if a == AccessKind::READ {
        AccessKind::WRITE
    } else if a == AccessKind::WRITE {
        AccessKind::READ
    } else if a == AccessKind::READ_WRITE {
        AccessKind::READ_WRITE
    } else {
        panic!("Unhandled case in reverseAccessKind")
    }
}

// Go: ast.go:1440 IsDeclarationNode
/// Go `node.DeclarationData() != nil`: the node types that embed
/// `DeclarationBase`.
#[must_use]
pub fn is_declaration_node(node: Node) -> bool {
    with_data!(node, |d| is_declaration_data(d))
}

/// `is_declaration_node` on the view of a node (`NodeRef`): a published
/// store node reads its data with no second registry lookup.
#[inline]
#[must_use]
pub fn is_declaration_node_ref(node: NodeRef) -> bool {
    match node.data() {
        Some(d) => is_declaration_data(d),
        None => is_declaration_node(node.node()),
    }
}

/// `is_declaration_node` on the data of the node.
fn is_declaration_data(d: NodeData<'_>) -> bool {
    matches!(
        d,
        NodeData::ArrowFunction(_)
            | NodeData::BinaryExpression(_)
            | NodeData::BindingElement(_)
            | NodeData::CallExpression(_)
            | NodeData::CallSignatureDeclaration(_)
            | NodeData::ClassDeclaration(_)
            | NodeData::ClassExpression(_)
            | NodeData::ClassStaticBlockDeclaration(_)
            | NodeData::ConstructSignatureDeclaration(_)
            | NodeData::ConstructorDeclaration(_)
            | NodeData::ConstructorTypeNode(_)
            | NodeData::EnumDeclaration(_)
            | NodeData::EnumMember(_)
            | NodeData::ExportAssignment(_)
            | NodeData::ExportDeclaration(_)
            | NodeData::ExportSpecifier(_)
            | NodeData::FunctionDeclaration(_)
            | NodeData::FunctionExpression(_)
            | NodeData::FunctionTypeNode(_)
            | NodeData::GetAccessorDeclaration(_)
            | NodeData::ImportClause(_)
            | NodeData::ImportDeclaration(_)
            | NodeData::ImportEqualsDeclaration(_)
            | NodeData::ImportSpecifier(_)
            | NodeData::IndexSignatureDeclaration(_)
            | NodeData::InterfaceDeclaration(_)
            | NodeData::JsDocSignature(_)
            | NodeData::JsDocTypeLiteral(_)
            | NodeData::JsxAttribute(_)
            | NodeData::JsxAttributes(_)
            | NodeData::JsxSpreadAttribute(_)
            | NodeData::MappedTypeNode(_)
            | NodeData::MethodDeclaration(_)
            | NodeData::MethodSignatureDeclaration(_)
            | NodeData::MissingDeclaration(_)
            | NodeData::ModuleDeclaration(_)
            | NodeData::NamedTupleMember(_)
            | NodeData::NamespaceExport(_)
            | NodeData::NamespaceExportDeclaration(_)
            | NodeData::NamespaceImport(_)
            | NodeData::NoSubstitutionTemplateLiteral(_)
            | NodeData::NotEmittedTypeElement(_)
            | NodeData::ObjectLiteralExpression(_)
            | NodeData::ParameterDeclaration(_)
            | NodeData::PropertyAssignment(_)
            | NodeData::PropertyDeclaration(_)
            | NodeData::PropertySignatureDeclaration(_)
            | NodeData::SemicolonClassElement(_)
            | NodeData::SetAccessorDeclaration(_)
            | NodeData::ShorthandPropertyAssignment(_)
            | NodeData::SourceFile(_)
            | NodeData::SpreadAssignment(_)
            | NodeData::TypeAliasDeclaration(_)
            | NodeData::TypeLiteralNode(_)
            | NodeData::TypeParameterDeclaration(_)
            | NodeData::VariableDeclaration(_)
    )
}

// Go: ast.go:1457 IsLocalsContainer
/// Go `node.LocalsContainerData() != nil`: the node types that embed
/// `LocalsContainerBase`.
// PORT: the variant list is `locals_container_variants!`, shared with the
// kind test of `Node::locals`.
#[must_use]
pub fn is_locals_container(node: Node) -> bool {
    with_data!(node, |d| locals_container_variants!(
        data_is_variant! { d, }
    ))
}

impl Node {
    // Go: ast.go:1488 JSDoc
    /// The JSDoc nodes of this node. Pass `Node::NIL` for `file` to walk up
    /// to the source file.
    // PORT: Go resolves a lazy cache miss with the parser hook
    // `parseJSDocForNode`. A file that is not published yet runs it through
    // `resolve_file_store_js_doc`; a published file runs it through
    // `program::resolve_lazy_js_doc`, with the inputs of the file itself.
    // PERF: U4 (CH7). `HAS_JS_DOC` is a parser bit (`Node::parser_flags`).
    #[must_use]
    pub fn js_doc(self, file: Node) -> NodeSlice {
        if self.parser_flags(NodeFlags::HAS_JS_DOC).is_empty() {
            return NodeSlice::NIL;
        }
        let file = if file.is_nil() {
            get_source_file_of_node(self)
        } else {
            file
        };
        if file.is_nil() {
            return NodeSlice::NIL;
        }
        // Go: a factory SourceFile (`NewSourceFile`, also after `copyFrom`)
        // has `hasLazyJSDoc` false and a nil `jsdocCache`, so the result is
        // nil. `source_file_info` would read the parsed file with the same
        // path, whose cache and lazy flag Go does not copy.
        if is_synthetic_node(file) {
            return NodeSlice::NIL;
        }
        // PORT: during the parse (Go `collectExternalModuleReferences`) or
        // after a parse outside a program (Go `parser.ParseSourceFile`, as
        // the astnav tests do) the store file is not in a program; its cache
        // and lazy JSDoc inputs are in the store.
        if is_file_store_before_program(file.file_index()) {
            return resolve_file_store_js_doc(file.file_index(), self).unwrap_or(NodeSlice::NIL);
        }
        let info = source_file_info(file);
        match cached_js_doc(file, &info, self) {
            Some(jsdocs) => jsdocs,
            None if info.has_lazy_js_doc => {
                NodeSlice::from_nodes(crate::program::resolve_lazy_js_doc(file, &info, self))
            }
            None => NodeSlice::NIL,
        }
    }

    // Go: ast.go:1506 EagerJSDoc
    /// JSDoc nodes that are already parsed and cached. It never parses.
    // PORT: Go reads `jsdocCache`, which also holds the entries that a lazy
    // parse (`resolveJSDoc`) added. The port keeps those in `LAZY_JSDOC`
    // (`program::cached_lazy_js_doc`), so a miss in the parse cache of a
    // lazy file reads them. Without that read, `checkSourceElement` missed
    // a `{@link}` that the comment prefilter does not see (a form feed
    // after `@link`) when an earlier `@deprecated` lookup parsed the
    // comment, and the import that the link names was reported unused.
    // Go shares the cache between its checkers; each port thread has its
    // own (`LAZY_JSDOC`).
    #[must_use]
    pub fn eager_js_doc(self, file: Node) -> NodeSlice {
        if self.parser_flags(NodeFlags::HAS_JS_DOC).is_empty() {
            return NodeSlice::NIL;
        }
        let file = if file.is_nil() {
            get_source_file_of_node(self)
        } else {
            file
        };
        if file.is_nil() {
            return NodeSlice::NIL;
        }
        // Go: a factory SourceFile has a nil `jsdocCache` (see `js_doc`).
        if is_synthetic_node(file) {
            return NodeSlice::NIL;
        }
        if is_file_store_before_program(file.file_index()) {
            return file_store_js_doc(file.file_index(), self).unwrap_or(NodeSlice::NIL);
        }
        let info = source_file_info(file);
        match cached_js_doc(file, &info, self) {
            Some(jsdocs) => jsdocs,
            None if info.has_lazy_js_doc => crate::program::cached_lazy_js_doc(self)
                .map_or(NodeSlice::NIL, NodeSlice::from_nodes),
            None => NodeSlice::NIL,
        }
    }
}

/// The JSDoc cache entry of `host` in published file `file`, whose info is
/// `info`, or `None` on a miss. The slice of a freeable file version reads
/// the cache at each use (`NodeSlice::from_file_js_doc`).
fn cached_js_doc(
    file: Node,
    info: &FileRef<crate::program::SourceFileInfo>,
    host: Node,
) -> Option<NodeSlice> {
    match info.as_static() {
        Some(info) => info
            .jsdoc_cache
            .get(&host)
            .map(|jsdocs| NodeSlice::from_nodes(jsdocs)),
        None => info
            .jsdoc_cache
            .contains_key(&host)
            .then(|| NodeSlice::from_file_js_doc(file.file_index(), host)),
    }
}

// Go: ast.go:1757 IsTypeOrJSTypeAliasDeclaration
#[must_use]
pub fn is_type_or_js_type_alias_declaration(node: Node) -> bool {
    node.kind() == SyntaxKind::TypeAliasDeclaration
        || node.kind() == SyntaxKind::JsTypeAliasDeclaration
}

// Go: ast.go:1803 IsImportDeclarationOrJSImportDeclaration
#[must_use]
pub fn is_import_declaration_or_js_import_declaration(node: Node) -> bool {
    node.kind() == SyntaxKind::ImportDeclaration || node.kind() == SyntaxKind::JsImportDeclaration
}

// Go: ast.go:1829 IsAnyExportAssignment
#[must_use]
pub fn is_any_export_assignment(node: Node) -> bool {
    node.kind() == SyntaxKind::ExportAssignment
}

impl Node {
    // Go: ast.go:2142 (*ImportAttributesNode).GetResolutionModeOverride
    /// The `resolution-mode` from an import attributes node, and whether
    /// one was given. The node can be nil.
    // PORT: Go `grammarErrorOnNode` is a nil-able func; `None` is Go nil.
    #[must_use]
    pub fn get_resolution_mode_override(
        self,
        grammar_error_on_node: Option<
            &mut dyn FnMut(Node, &'static crate::diagnostics::Message, Vec<String>) -> bool,
        >,
    ) -> (ResolutionMode, bool) {
        if self.is_nil() {
            return (RESOLUTION_MODE_NONE, false);
        }
        let attributes = list_of!(self, |d| match d {
            NodeData::ImportAttributes(d) => Some(sel_field!(req, d.attributes)),
            _ => None,
        })
        .unwrap_or_else(|| panic!("AsImportAttributes called on {:?}", self.kind()))
        .nodes();
        let attribute = attributes
            .iter()
            .find(|attribute| attribute.name().text() == "resolution-mode");
        let Some(elem) = attribute else {
            return (RESOLUTION_MODE_NONE, false);
        };
        let value = elem.value();
        if !is_string_literal_like(value) {
            return (RESOLUTION_MODE_NONE, false);
        }
        if value.text() != "import" && value.text() != "require" {
            if let Some(grammar_error_on_node) = grammar_error_on_node {
                grammar_error_on_node(
                    value,
                    diag::X_resolution_mode_should_be_either_require_or_import,
                    Vec::new(),
                );
            }
            return (RESOLUTION_MODE_NONE, false);
        }
        if value.text() == "import" {
            (RESOLUTION_MODE_ESM, true)
        } else {
            (RESOLUTION_MODE_COMMON_JS, true)
        }
    }
}

// ──────────────────────────────────────────────────────────────────────
// SourceFile methods. `file` is a SourceFile node.
// ──────────────────────────────────────────────────────────────────────

// PORT: the caches below are per thread and leaked, for static files
// (static publishes, synthetic) and files that are not published yet. A
// published freeable file version (lsshells M3b) keeps them on its
// `FileVersion` (`name_table`, `position_map`, `declaration_map`; see
// `file_version_of`) and its ECMA line map in its store
// (`frozen_file_ecma_line_starts`), as Go keeps them on the `SourceFile`,
// so they are freed with it.
thread_local! {
    /// Go `SourceFile.ecmaLineMap`, computed once per file.
    static ECMA_LINE_MAPS: RefCell<FxHashMap<Node, &'static [i32]>> = RefCell::new(FxHashMap::default());
    /// Go `SourceFile.nameTable`, computed once per file.
    static NAME_TABLES: RefCell<FxHashMap<Node, &'static FxHashMap<String, i32>>> = RefCell::new(FxHashMap::default());
    /// Go `SourceFile.positionMap`, computed once per file.
    static POSITION_MAPS: RefCell<FxHashMap<Node, &'static PositionMap>> = RefCell::new(FxHashMap::default());
    /// Go `SourceFile.declarationMap`, computed once per file.
    static DECLARATION_MAPS: RefCell<FxHashMap<Node, &'static FxHashMap<String, Vec<Node>>>> =
        RefCell::new(FxHashMap::default());
    /// Go `SourceFile.identifiers`, collected once per parsed file.
    static IDENTIFIER_SETS: RefCell<FxHashMap<Node, &'static FxHashSet<&'static str>>> =
        RefCell::new(FxHashMap::default());
}

/// The live freeable file version of `file` (a SourceFile node), or `None`
/// for a static, unpublished or synthetic file.
fn file_version_of(file: Node) -> Option<crate::ast::VersionPin> {
    if is_synthetic_node(file) {
        return None;
    }
    crate::ast::try_go_file(file.file_index())?
        .version()
        .cloned()
}

// Go: ast.go:2465 (*SourceFile).Text
/// The text of a SourceFile node. Hold the result only as long as it is
/// read: the text of a freeable file version goes when the last holder
/// lets go (`FileText`).
#[must_use]
pub fn source_file_text(file: Node) -> FileText {
    if is_synthetic_node(file) {
        return synthetic_source_file_text(file);
    }
    file_store_text(file.file_index())
}

// ---------------------------------------------------------------------------
// Content mapper info of a SourceFile (tsgo#4712)
// ---------------------------------------------------------------------------

// Go: ast/ast.go:2529 MappedDiagnosticDirectivePolicy
crate::flags_macros::go_enum!(MappedDiagnosticDirectivePolicy, u8 {
    IGNORE = 0; // MappedDiagnosticDirectivePolicyIgnore
    EXPECT = 1; // MappedDiagnosticDirectivePolicyExpect
});

// Go: ast/ast.go:2536 MappedDiagnosticDirective
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MappedDiagnosticDirective {
    pub original_range: TextRange,
    pub virtual_range: TextRange,
    pub policy: MappedDiagnosticDirectivePolicy,
    pub unused_code: i32,
    pub unused_message_text: String,
    pub source: String,
}

// Go: ast/ast.go:2545 ContentMapperSourceFileInfo
// PORT: Go `*SourceFile` is `Rc<ParsedSourceFile>` (the compiler host
// type), and Go `*spanmap.SpanMap` is `Option<Arc<SpanMap>>`. Pass it to
// `ParsedSourceFile::set_content_mapper_info`, which keeps it in two parts:
// `ContentMapperFileInfo` for readers of the SourceFile node on any thread,
// and the `Rc` links on this thread.
#[derive(Clone, Debug, Default)]
pub struct ContentMapperSourceFileInfo {
    pub content_mapper: String,
    pub transform_identity: String,
    pub parse_options: crate::frontend::parser::SourceFileParseOptions,
    pub virtual_file_name: String,
    pub original_text: String,
    pub span_map: Option<std::sync::Arc<crate::spanmap::SpanMap>>,
    pub diagnostic_directives: Vec<MappedDiagnosticDirective>,
    pub supplemental_source_files: Vec<Rc<crate::frontend::parser::source_file::ParsedSourceFile>>,
    pub canonical_source_file: Option<Rc<crate::frontend::parser::source_file::ParsedSourceFile>>,
}

/// Go `SourceFile.contentMapperInfo` as the readers of a SourceFile node
/// see it: `ContentMapperSourceFileInfo` with each linked file as its
/// SourceFile node (`Node::NIL` for no canonical file).
// PORT: Go keeps the info on the SourceFile and reads it on any goroutine
// (the emitter, the diagnostic writer, the language service). A parsed
// file's node data cannot hold it and `ParsedSourceFile` must stay `Send`
// (a parse worker makes it), so the info lives in a process table by file
// (`content_mapper_key`), like the file stores. It is set once and never
// freed, as a published file is never freed.
#[derive(Debug)]
pub struct ContentMapperFileInfo {
    pub content_mapper: String,
    pub transform_identity: String,
    pub parse_options: crate::frontend::parser::SourceFileParseOptions,
    pub virtual_file_name: String,
    pub original_text: String,
    pub span_map: Option<std::sync::Arc<crate::spanmap::SpanMap>>,
    pub diagnostic_directives: Vec<MappedDiagnosticDirective>,
    pub supplemental_source_files: Vec<Node>,
    pub canonical_source_file: Node,
}

/// The content mapper info of each parsed file, by `content_mapper_key`.
static CONTENT_MAPPER_INFOS: std::sync::RwLock<
    FxHashMap<(usize, usize), &'static ContentMapperFileInfo>,
> = std::sync::RwLock::new(FxHashMap::with_hasher(rustc_hash::FxBuildHasher));

/// The key of the parsed file with id `file` in `CONTENT_MAPPER_INFOS`: the
/// id and the address of the file name that its store keeps. Panics when
/// this thread sees no store `file`.
// PORT: Go keeps the info on the `*SourceFile`, so each parse has its own.
// A file id alone does not name one parse: the build stores of every thread
// take their ids from `PUBLISHED` (`store.rs`), so a store that is not
// published on another thread, or on a thread that ended without a publish
// (a test), can have the same id. `parse_source_file` leaks the file name
// of each parse (a path, never empty), and a leak is never freed, so its
// address names the parse. A clone of the `ParsedSourceFile` and a later
// program that shares the published file read the same store, so they get
// the same key.
fn content_mapper_key(file: usize) -> (usize, usize) {
    (file, file_store_file_name(file).as_ptr().addr())
}

/// True once any file has content mapper info, so a program with no content
/// mapper reads no table.
static HAS_CONTENT_MAPPER_INFO: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Go `SourceFileParseOptions{}`, which `ContentMapperParseOptions` returns
/// for a file with no content mapper info.
static EMPTY_PARSE_OPTIONS: crate::frontend::parser::SourceFileParseOptions =
    crate::frontend::parser::SourceFileParseOptions {
        file_name: String::new(),
        path: crate::frontend::tspath::Path(String::new()),
        external_module_indicator_options:
            crate::frontend::parser::ExternalModuleIndicatorOptions {
                jsx: false,
                force: false,
            },
    };

// Go: ast/ast.go:2566 (*SourceFile).SetContentMapperInfo, for the node side.
/// Stores the content mapper info of the parsed SourceFile `file`.
/// `ParsedSourceFile::set_content_mapper_info` calls it.
// PORT: panics when the info is already set, as Go does.
pub fn set_source_file_content_mapper_info(file: Node, info: ContentMapperFileInfo) {
    let key = content_mapper_key(file.file_index());
    let mut infos = CONTENT_MAPPER_INFOS
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if infos.contains_key(&key) {
        panic!("content mapper source file info already set");
    }
    infos.insert(key, Box::leak(Box::new(info)));
    HAS_CONTENT_MAPPER_INFO.store(true, std::sync::atomic::Ordering::Release);
}

/// Go `node.contentMapperInfo` of the SourceFile `file`: `None` for nil.
/// A factory SourceFile has the info that `source_file_copy_from` copied.
#[must_use]
pub fn source_file_content_mapper_info(file: Node) -> Option<&'static ContentMapperFileInfo> {
    if file.is_nil() || !HAS_CONTENT_MAPPER_INFO.load(std::sync::atomic::Ordering::Acquire) {
        return None;
    }
    if is_synthetic_node(file) {
        return with_synthetic_source_file(file, |d| d.content_mapper_info);
    }
    if !has_file_store(file.file_index()) {
        return None;
    }
    let key = content_mapper_key(file.file_index());
    CONTENT_MAPPER_INFOS
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(&key)
        .copied()
}

// Go: ast/ast.go:2470 (*SourceFile).OriginalText
// OriginalText returns the untransformed source text for content-mapped files, or Text() otherwise.
#[must_use]
pub fn source_file_original_text(file: Node) -> FileText {
    match source_file_content_mapper_info(file) {
        Some(info) if !info.content_mapper.is_empty() => FileText::Static(&info.original_text),
        _ => source_file_text(file),
    }
}

// Go: ast/ast.go:2478 (*SourceFile).OriginalFileName
// OriginalFileName returns the canonical filename associated with a supplemental source file, or FileName() otherwise.
#[must_use]
pub fn source_file_original_file_name(file: Node) -> &'static str {
    let canonical = source_file_canonical_source_file(file);
    if canonical.is_some() {
        return source_file_file_name(canonical);
    }
    source_file_file_name(file)
}

// Go: ast/ast.go:2488 (*SourceFile).SpanMap
// SpanMap returns the span map that maps positions in this file's transformed Text() back to its
// original, untransformed content, or nil if the file is not content-mapped (or is a failure stub).
// The returned map is nil-safe: a nil map maps positions identically.
// PORT: nil is `None`; the `SpanMap` methods take `Option<&SpanMap>`.
#[must_use]
pub fn source_file_span_map(file: Node) -> Option<&'static crate::spanmap::SpanMap> {
    source_file_content_mapper_info(file)?.span_map.as_deref()
}

// Go: ast/ast.go:2496 (*SourceFile).IsContentMapped (ts#64518)
// IsContentMapped reports whether this file was produced by a content mapper.
#[must_use]
pub fn source_file_is_content_mapped(file: Node) -> bool {
    source_file_content_mapper_info(file).is_some()
}

// Go: ast/ast.go:2502 (*SourceFile).ContentMapper
// ContentMapper returns the identity of the content mapper that produced this file, or "" if the file
// was not produced by a content mapper (or the mapper did not identify itself).
#[must_use]
pub fn source_file_content_mapper(file: Node) -> &'static str {
    source_file_content_mapper_info(file).map_or("", |info| &info.content_mapper)
}

// Go: ast/ast.go:2511 (*SourceFile).IsContentMapperFailureStub
// IsContentMapperFailureStub reports whether this file is the empty placeholder produced when a content
// mapper's transform failed.
#[must_use]
pub fn source_file_is_content_mapper_failure_stub(file: Node) -> bool {
    !source_file_content_mapper(file).is_empty() && source_file_span_map(file).is_none()
}

// Go: ast/ast.go:2515 (*SourceFile).ContentMapperTransformIdentity
#[must_use]
pub fn source_file_content_mapper_transform_identity(file: Node) -> &'static str {
    source_file_content_mapper_info(file).map_or("", |info| &info.transform_identity)
}

// Go: ast/ast.go:2522 (*SourceFile).VirtualFileName
#[must_use]
pub fn source_file_virtual_file_name(file: Node) -> &'static str {
    source_file_content_mapper_info(file).map_or("", |info| &info.virtual_file_name)
}

// Go: ast/ast.go:2558 (*SourceFile).ContentMapperParseOptions
// ContentMapperParseOptions returns the parse options used to acquire this file from the mapped parse cache.
// PORT: returns a reference; the zero value is `EMPTY_PARSE_OPTIONS`.
#[must_use]
pub fn source_file_content_mapper_parse_options(
    file: Node,
) -> &'static crate::frontend::parser::SourceFileParseOptions {
    source_file_content_mapper_info(file).map_or(&EMPTY_PARSE_OPTIONS, |info| &info.parse_options)
}

// Go: ast/ast.go:2573 (*SourceFile).DiagnosticDirectives
#[must_use]
pub fn source_file_diagnostic_directives(file: Node) -> &'static [MappedDiagnosticDirective] {
    match source_file_content_mapper_info(file) {
        Some(info) => &info.diagnostic_directives,
        None => &[],
    }
}

// Go: ast/ast.go:2581 (*SourceFile).SupplementalSourceFiles
// SupplementalSourceFiles returns the additional outputs produced from this canonical source file.
// PORT: the files are their SourceFile nodes. `ParsedSourceFile` has the
// `Rc` form.
#[must_use]
pub fn source_file_supplemental_source_files(file: Node) -> &'static [Node] {
    match source_file_content_mapper_info(file) {
        Some(info) => &info.supplemental_source_files,
        None => &[],
    }
}

// Go: ast/ast.go:2589 (*SourceFile).CanonicalSourceFile
// CanonicalSourceFile returns the canonical output associated with this supplemental source file.
// PORT: the file is its SourceFile node, or `Node::NIL`. `ParsedSourceFile`
// has the `Rc` form.
#[must_use]
pub fn source_file_canonical_source_file(file: Node) -> Node {
    source_file_content_mapper_info(file).map_or(Node::NIL, |info| info.canonical_source_file)
}

// Go: ast/ast.go:2597 (*SourceFile).IsContentMapperSupplemental
// IsContentMapperSupplemental reports whether this is an unnamed supplemental mapper output.
#[must_use]
pub fn source_file_is_content_mapper_supplemental(file: Node) -> bool {
    source_file_canonical_source_file(file).is_some()
}

// Go: ast.go:2601 (*SourceFile).HasIdentifier
// PORT: Go `identifiersOnce` is a per-thread cache (`IDENTIFIER_SETS`) for a
// parsed file, whose id is never reused. A freeable file version keeps its
// set, which is freed with it (lsshells M3). A factory SourceFile collects
// its set on each call, because a released program frees its synthetic
// nodes and a later node can get the same handle.
#[must_use]
pub fn source_file_has_identifier(file: Node, name: &str) -> bool {
    if is_synthetic_node(file) {
        return collect_identifiers_for_source_file(file).contains(name);
    }
    if let Some(version) = file_version_of(file) {
        return version
            .identifiers
            .get_or_init(|| collect_identifiers_for_source_file(file))
            .contains(name);
    }
    if let Some(identifiers) = IDENTIFIER_SETS.with(|c| c.borrow().get(&file).copied()) {
        return identifiers.contains(name);
    }
    let identifiers: &'static FxHashSet<&'static str> =
        Box::leak(Box::new(collect_identifiers_for_source_file(file)));
    IDENTIFIER_SETS.with(|c| c.borrow_mut().insert(file, identifiers));
    identifiers.contains(name)
}

// Go: ast.go:2608 collectIdentifiersForSourceFile
fn collect_identifiers_for_source_file(source_file: Node) -> FxHashSet<&'static str> {
    fn collect(node: Node, identifiers: &mut FxHashSet<&'static str>) -> bool {
        match node.kind() {
            SyntaxKind::Identifier
            | SyntaxKind::PrivateIdentifier
            | SyntaxKind::StringLiteral
            | SyntaxKind::NumericLiteral
            | SyntaxKind::BigIntLiteral
            | SyntaxKind::NoSubstitutionTemplateLiteral => {
                identifiers.insert(node.text());
            }
            _ => {}
        }
        node.for_each_child(|child| collect(child, identifiers));
        false
    }
    let mut identifiers = FxHashSet::default();
    collect(source_file, &mut identifiers);
    identifiers
}

// Go: ast.go:2628 (*SourceFile).FileName
#[must_use]
pub fn source_file_file_name(file: Node) -> &'static str {
    if is_synthetic_node(file) {
        return synthetic_source_file_file_name(file);
    }
    file_store_file_name(file.file_index())
}

// Go: ast.go:2636 (*SourceFile).Imports
// PORT: a freeable file version (lsshells M3b) gives a handle slice
// (`NodeSlice::from_file_imports`), which reads the imports at each use.
#[must_use]
pub fn source_file_imports(file: Node) -> NodeSlice {
    let file = published_source_file(file);
    match source_file_info(file).as_static() {
        Some(info) => NodeSlice::from_nodes(&info.imports),
        None => NodeSlice::from_file_imports(file.file_index()),
    }
}

/// A diagnostic list of `SourceFileInfo`, as the key of a `FileRef`.
#[derive(Clone, Copy)]
enum DiagnosticList {
    Parse,
    Js,
    JsDoc,
}

/// The list of `info` that `key` (a `DiagnosticList`) names.
fn diagnostic_list(info: &crate::program::SourceFileInfo, key: usize) -> &[Diagnostic] {
    match key {
        k if k == DiagnosticList::Parse as usize => &*info.diagnostics,
        k if k == DiagnosticList::Js as usize => &*info.js_diagnostics,
        _ => &*info.jsdoc_diagnostics,
    }
}

/// Diagnostic list `list` of the info of `file` (see `source_file_info`).
// PORT: the part must stay a place expression, with no braces around it. A
// block is a value, so `&{ .. }` would move the list into a temporary.
fn source_file_list(file: Node, list: DiagnosticList) -> FileRef<[Diagnostic]> {
    use crate::ast::file_version::go_file_ref;
    let file = published_source_file(file);
    go_file_ref!(file.file_index(), list as usize, |g, key| *diagnostic_list(
        &g.info, key
    ))
}

// Go: ast.go:2640 (*SourceFile).Diagnostics
// PORT: a file that is not published yet reads its store (see
// `source_file_language_variant`).
#[must_use]
pub fn source_file_diagnostics(file: Node) -> FileRef<[Diagnostic]> {
    if !is_synthetic_node(file) && is_file_store_before_program(file.file_index()) {
        return FileRef::Static(file_store_diagnostics(file.file_index()));
    }
    source_file_list(file, DiagnosticList::Parse)
}

// Go: ast.go:2648 (*SourceFile).JSDiagnostics
#[must_use]
pub fn source_file_js_diagnostics(file: Node) -> FileRef<[Diagnostic]> {
    source_file_list(file, DiagnosticList::Js)
}

// Go: ast.go:2656 (*SourceFile).JSDocDiagnostics
#[must_use]
pub fn source_file_jsdoc_diagnostics(file: Node) -> FileRef<[Diagnostic]> {
    source_file_list(file, DiagnosticList::JsDoc)
}

// Go: ast.go:2699 (*SourceFile).BindDiagnostics
// PORT: panics before the file is bound. Go returns nil there.
#[must_use]
pub fn source_file_bind_diagnostics(file: Node) -> FileRef<[Diagnostic]> {
    crate::ast::file_version::go_file_ref!(file.file_index(), 0, |g, _key| *g
        .file_bind
        .get()
        .expect("source file is not bound")
        .bind_diagnostics)
}

// Go: ast.go:2715 (*SourceFile).IsJS
#[must_use]
pub fn source_file_is_js(file: Node) -> bool {
    is_source_file_js(file)
}

// Go: ast.go:2761 (*SourceFile).ECMALineMap
#[must_use]
pub fn source_file_ecma_line_map(file: Node) -> FileRef<[i32]> {
    if !is_synthetic_node(file)
        && let Some(line_map) = crate::ast::store::frozen_file_ecma_line_starts(file.file_index())
    {
        return line_map;
    }
    if let Some(line_map) = ECMA_LINE_MAPS.with(|c| c.borrow().get(&file).copied()) {
        return FileRef::Static(line_map);
    }
    let line_map: &'static [i32] = Vec::leak(compute_ecma_line_starts(&source_file_text(file)));
    ECMA_LINE_MAPS.with(|c| c.borrow_mut().insert(file, line_map));
    FileRef::Static(line_map)
}

// Go: ast.go:2779 (*SourceFile).GetNameTable
/// All names in the file mapped to their position. A name that appears
/// more than once maps to -1.
#[must_use]
pub fn source_file_get_name_table(file: Node) -> FileRef<FxHashMap<String, i32>> {
    if let Some(version) = file_version_of(file) {
        version.name_table.get_or_init(|| compute_name_table(file));
        return FileRef::Pinned {
            version,
            key: 0,
            get: |version, _| version.name_table.get().expect("the name table is made"),
        };
    }
    if let Some(table) = NAME_TABLES.with(|c| c.borrow().get(&file).copied()) {
        return FileRef::Static(table);
    }
    let table: &'static FxHashMap<String, i32> = Box::leak(Box::new(compute_name_table(file)));
    NAME_TABLES.with(|c| c.borrow_mut().insert(file, table));
    FileRef::Static(table)
}

/// Go `(*SourceFile).GetNameTable` without the cache.
fn compute_name_table(file: Node) -> FxHashMap<String, i32> {
    fn walk(file: Node, node: Node, name_table: &mut FxHashMap<String, i32>) -> bool {
        if is_identifier(node) && !is_tag_name(node) && !node.text().is_empty()
            || is_string_or_numeric_literal_like(node) && literal_is_name(node)
            || is_private_identifier(node)
        {
            let text = node.text();
            if let Some(pos) = name_table.get_mut(text) {
                *pos = -1;
            } else {
                name_table.insert(text.to_string(), node.pos());
            }
        }
        node.for_each_child(|child| walk(file, child, name_table));
        for jsdoc in node.js_doc(file) {
            jsdoc.for_each_child(|child| walk(file, child, name_table));
        }
        false
    }
    let mut name_table = FxHashMap::default();
    file.for_each_child(|child| walk(file, child, &mut name_table));
    name_table
}

// Go: ast.go:2810 (*SourceFile).IsBound
#[must_use]
pub fn source_file_is_bound(file: Node) -> bool {
    crate::ast::with_go_file(file.file_index(), |g| g.file_bind.get().is_some())
}

// Go: ast.go:2815 (*SourceFile).GetPositionMap
// PORT: Go `positionMapOnce` is a per-thread cache (`POSITION_MAPS`).
#[must_use]
pub fn source_file_get_position_map(file: Node) -> FileRef<PositionMap> {
    if let Some(version) = file_version_of(file) {
        version
            .position_map
            .get_or_init(|| compute_source_file_position_map(file));
        return FileRef::Pinned {
            version,
            key: 0,
            get: |version, _| {
                version
                    .position_map
                    .get()
                    .expect("the position map is made")
            },
        };
    }
    if let Some(map) = POSITION_MAPS.with(|c| c.borrow().get(&file).copied()) {
        return FileRef::Static(map);
    }
    let map: &'static PositionMap = Box::leak(Box::new(compute_source_file_position_map(file)));
    POSITION_MAPS.with(|c| c.borrow_mut().insert(file, map));
    FileRef::Static(map)
}

/// Go `(*SourceFile).GetPositionMap` without the cache.
fn compute_source_file_position_map(file: Node) -> PositionMap {
    compute_position_map(&source_file_text(file))
}

// Go: ast.go:2895 (*SourceFile).GetDeclarationMap
#[must_use]
pub fn source_file_get_declaration_map(file: Node) -> FileRef<FxHashMap<String, Vec<Node>>> {
    if let Some(version) = file_version_of(file) {
        version
            .declaration_map
            .get_or_init(|| compute_declaration_map(file));
        return FileRef::Pinned {
            version,
            key: 0,
            get: |version, _| {
                version
                    .declaration_map
                    .get()
                    .expect("the declaration map is made")
            },
        };
    }
    if let Some(map) = DECLARATION_MAPS.with(|c| c.borrow().get(&file).copied()) {
        return FileRef::Static(map);
    }
    let map: &'static FxHashMap<String, Vec<Node>> =
        Box::leak(Box::new(compute_declaration_map(file)));
    DECLARATION_MAPS.with(|c| c.borrow_mut().insert(file, map));
    FileRef::Static(map)
}

// Go: ast.go:2904 (*SourceFile).computeDeclarationMap
fn compute_declaration_map(file: Node) -> FxHashMap<String, Vec<Node>> {
    fn add_declaration(result: &mut FxHashMap<String, Vec<Node>>, declaration: Node) {
        let name = get_declaration_name(declaration);
        if !name.is_empty() {
            result.entry(name).or_default().push(declaration);
        }
    }
    fn visit(result: &mut FxHashMap<String, Vec<Node>>, node: Node) -> bool {
        match node.kind() {
            SyntaxKind::FunctionDeclaration
            | SyntaxKind::FunctionExpression
            | SyntaxKind::MethodDeclaration
            | SyntaxKind::MethodSignature => {
                let declaration_name = get_declaration_name(node);
                if !declaration_name.is_empty() {
                    let declarations = result.entry(declaration_name).or_default();
                    let last_declaration = declarations.last().copied().unwrap_or(Node::NIL);
                    // Check whether this declaration belongs to an "overload group".
                    if last_declaration.is_some()
                        && node.parent() == last_declaration.parent()
                        && node.symbol() == last_declaration.symbol()
                    {
                        // Overwrite the last declaration if it was an overload and this one is an implementation.
                        if node.body().is_some() && last_declaration.body().is_nil() {
                            let last = declarations.len() - 1;
                            declarations[last] = node;
                        }
                    } else {
                        declarations.push(node);
                    }
                }
                node.for_each_child(|child| visit(result, child));
            }
            SyntaxKind::ClassDeclaration
            | SyntaxKind::ClassExpression
            | SyntaxKind::InterfaceDeclaration
            | SyntaxKind::TypeAliasDeclaration
            | SyntaxKind::EnumDeclaration
            | SyntaxKind::ModuleDeclaration
            | SyntaxKind::ImportEqualsDeclaration
            | SyntaxKind::ImportClause
            | SyntaxKind::NamespaceImport
            | SyntaxKind::GetAccessor
            | SyntaxKind::SetAccessor
            | SyntaxKind::TypeLiteral => {
                add_declaration(result, node);
                node.for_each_child(|child| visit(result, child));
            }
            SyntaxKind::ImportSpecifier | SyntaxKind::ExportSpecifier => {
                if node.property_name().is_some() {
                    add_declaration(result, node);
                }
            }
            SyntaxKind::Parameter
            | SyntaxKind::VariableDeclaration
            | SyntaxKind::BindingElement => {
                // Only consider parameter properties.
                if node.kind() == SyntaxKind::Parameter
                    && !has_syntactic_modifier(node, ModifierFlags::PARAMETER_PROPERTY_MODIFIER)
                {
                    return false;
                }
                let name = node.name();
                if name.is_some() {
                    if is_binding_pattern(name) {
                        name.for_each_child(|child| visit(result, child));
                    } else {
                        if node.initializer().is_some() {
                            visit(result, node.initializer());
                        }
                        add_declaration(result, node);
                    }
                }
            }
            SyntaxKind::EnumMember
            | SyntaxKind::PropertyDeclaration
            | SyntaxKind::PropertySignature => {
                add_declaration(result, node);
            }
            SyntaxKind::ExportDeclaration => {
                // Handle named exports case e.g.:
                //    export {a, b as B} from "mod";
                let export_clause = node.export_clause();
                if export_clause.is_some() {
                    if is_named_exports(export_clause) {
                        for element in export_clause.elements() {
                            visit(result, element);
                        }
                    } else {
                        visit(result, export_clause.name());
                    }
                }
            }
            SyntaxKind::ImportDeclaration => {
                let import_clause = node.import_clause();
                if import_clause.is_some() {
                    // Handle default import case e.g.:
                    //    import d from "mod";
                    if import_clause.name().is_some() {
                        add_declaration(result, import_clause.name());
                    }
                    // Handle named bindings in imports e.g.:
                    //    import * as NS from "mod";
                    //    import {a, b as B} from "mod";
                    let named_bindings = import_clause.named_bindings();
                    if named_bindings.is_some() {
                        if named_bindings.kind() == SyntaxKind::NamespaceImport {
                            add_declaration(result, named_bindings);
                        } else {
                            for element in named_bindings.elements() {
                                visit(result, element);
                            }
                        }
                    }
                }
            }
            SyntaxKind::BinaryExpression => {
                let kind = get_assignment_declaration_kind(node);
                if kind == JSDeclarationKind::EXPORTS_PROPERTY
                    || kind == JSDeclarationKind::THIS_PROPERTY
                    || kind == JSDeclarationKind::PROPERTY
                {
                    add_declaration(result, node);
                }
                node.for_each_child(|child| visit(result, child));
            }
            _ => {
                node.for_each_child(|child| visit(result, child));
            }
        }
        false
    }
    let mut result = FxHashMap::default();
    file.for_each_child(|child| visit(&mut result, child));
    result
}

// Go: ast.go:3014 GetDeclarationName
/// The plain text name of a declaration, or "" when it has none.
#[must_use]
pub fn get_declaration_name(declaration: Node) -> String {
    let name = get_non_assigned_name_of_declaration(declaration);
    if name.is_some() {
        if is_computed_property_name(name) {
            if is_string_or_numeric_literal_like(name.expression()) {
                return name.expression().text().to_string();
            }
            if is_property_access_expression(name.expression()) {
                return name.expression().name().text().to_string();
            }
        } else if is_property_name(name) {
            return name.text().to_string();
        }
    }
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::astdata::BoxedData;

    /// Go `(*Node)(nil).SubtreeFacts()` dereferences nil. The data read of
    /// nil panics (`static_tier_ast_node`) before any facts cache read, in a
    /// debug build as in a release build.
    #[test]
    #[should_panic(expected = "nil node dereference")]
    fn subtree_facts_of_nil_is_a_nil_dereference() {
        let _ = Node::NIL.subtree_facts();
    }

    /// Go caches `SubtreeFacts()` only in nodes of a `CompositeBase` type
    /// (`ast.go:1602`). Tokens, identifiers, literals and type syntax compute
    /// their facts on each call (`ast.go:1234`), with the same values.
    #[test]
    fn subtree_facts_are_cached_only_for_composite_kinds() {
        use crate::frontend::parser::{SourceFileParseOptions, parse_source_file};
        fn collect(n: Node, out: &mut Vec<Node>) {
            out.push(n);
            n.for_each_child(|c| {
                collect(c, out);
                false
            });
        }
        let text = "@dec class C { m(@p a: number = 1) { return a ?? this.b?.c; } }\n\
            let x: string[] = [1, 2].map(y => `${y}`), z = !(-x.length);\n";
        let opts = SourceFileParseOptions {
            file_name: "/facts.ts".to_string(),
            ..Default::default()
        };
        let root = parse_source_file(&opts, text, ScriptKind::TS).root;
        assert!(
            root.subtree_facts()
                .intersects(SubtreeFacts::SUBTREE_CONTAINS_DECORATORS)
        );
        let mut nodes = Vec::new();
        collect(root, &mut nodes);
        let (mut cached, mut computed) = (0, 0);
        for &n in &nodes {
            let facts = n.subtree_facts();
            assert_eq!(n.subtree_facts(), facts, "{:?}", n.kind());
            if with_data!(n, |d| is_composite_data(n, d)) {
                assert_eq!(cached_subtree_facts(n), Some(facts), "{:?}", n.kind());
                cached += 1;
            } else {
                assert_eq!(cached_subtree_facts(n), None, "{:?}", n.kind());
                computed += 1;
            }
        }
        assert!(cached > 10 && computed > 10, "{cached} {computed}");
    }

    /// perfP1 item 1 (perfscan2), in a release build: the child chain of
    /// each published node that has one (R2-5 links, which
    /// `for_each_child_dyn` walks) is the walk of its node data, in Go
    /// `ForEachChild` order. A debug build checks this at each walk; the
    /// protected tests run release builds, so this test checks a sample.
    #[test]
    fn child_chains_are_the_data_walk() {
        const SOURCE: &str = "@dec export abstract class C<T extends object = {}> extends B implements I {\n\
            ;  #p?: T; static { debugger; } constructor(private a: number) { super(); }\n\
            get g(): this { return this; } set g(v) {} [k: string]: any; m?<U>(...r: U[]): void;\n\
            }\n\
            interface I { (x: number): string; new (): I; readonly [P in keyof T]-?: T[P] }\n\
            type M<T> = { [P in keyof T as `k${string & P}`]+?: T[P] extends infer U ? U : never };\n\
            enum E { A = 1 << 2, B }\n\
            namespace N.M { export import Q = N.Q; }\n\
            import d, { a as b, type c } from 'm';\n\
            label: for (const [a, , ...b] of x) { if (a) continue label; else break; }\n\
            switch (x) { case 1: throw new Error(); default: }\n\
            try { using r = f(); } catch ({ e = 1 }) { } finally { }\n\
            async function* g() { yield* await x!; return void delete o[1]?.[2]; }\n\
            const o = { a, b: 1, ...c, ['d']: 2, m() {}, get x() { return 1; } } satisfies O as const;\n\
            let t = tag<string>`a${1}b${2}c` + /re/g.source + 1n + (x, y++) - ~!x + (-x) ** 2;\n\
            /** @param a text */ function h(this: Window, a: unknown): asserts a is string {}\n";
        let dir =
            std::env::temp_dir().join(format!("ts_goport_child_chains_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.ts"), SOURCE).unwrap();
        std::fs::write(
            dir.join("tsconfig.json"),
            r#"{ "compilerOptions": { "noLib": true, "types": [] }, "files": ["a.ts"] }"#,
        )
        .unwrap();
        let config = dir.join("tsconfig.json");
        let program = crate::program::try_load_version(&config.to_string_lossy(), |_| {})
            .unwrap_or_else(|e| panic!("cannot load {}: {e}", config.display()));
        let _ = std::fs::remove_dir_all(&dir);
        let scope = crate::core::enter_program(Some(program));
        let root = program
            .source_files()
            .find(|file| file.info.file_name.ends_with("/a.ts"))
            .expect("a.ts is not in the program")
            .root;
        // (nodes, nodes with a known chain, those with children).
        let mut counts = (0, 0, 0);
        let mut stack = vec![root];
        while let Some(n) = stack.pop() {
            counts.0 += 1;
            let by_data: Vec<Node> = children_by_data(n).collect();
            if let Some(chain) = frozen_store_node(n).and_then(|f| f.children()) {
                assert_eq!(chain.collect::<Vec<_>>(), by_data, "{:?}", n.kind());
                counts.1 += 1;
                counts.2 += usize::from(!by_data.is_empty());
            }
            stack.extend(by_data);
        }
        drop(scope);
        crate::program::release_program(program);
        assert!(counts.1 > 250 && counts.2 > 100, "{counts:?}");
    }

    /// noderef1 W1: `has_no_child_fields` holds for exactly the kinds of the
    /// variants of the last arm of `walk_children`, and that arm visits no
    /// field. In parsed code, a node has a child field exactly when its kind
    /// is not one of them.
    #[test]
    fn no_child_kinds_are_the_kinds_of_the_leaf_arm() {
        use crate::astdata as a;
        use crate::frontend::parser::{SourceFileParseOptions, parse_source_file};
        let (s, f) = (String::new, a::TokenFlags(0));
        let leaves = [
            BoxedData::Token(Box::new(a::TokenData)),
            BoxedData::Identifier(Box::new(a::IdentifierData { text: s() })),
            BoxedData::PrivateIdentifier(Box::new(a::PrivateIdentifierData { text: s() })),
            BoxedData::KeywordExpression(Box::new(a::KeywordExpressionData)),
            BoxedData::KeywordTypeNode(Box::new(a::KeywordTypeNodeData)),
            BoxedData::NumericLiteral(Box::new(a::NumericLiteralData {
                text: s(),
                token_flags: f,
            })),
            BoxedData::BigIntLiteral(Box::new(a::BigIntLiteralData {
                text: s(),
                token_flags: f,
            })),
            BoxedData::StringLiteral(Box::new(a::StringLiteralData {
                text: s(),
                token_flags: f,
            })),
            BoxedData::RegularExpressionLiteral(Box::new(a::RegularExpressionLiteralData {
                text: s(),
                token_flags: f,
            })),
            BoxedData::NoSubstitutionTemplateLiteral(Box::new(
                a::NoSubstitutionTemplateLiteralData {
                    raw_text: s(),
                    template_flags: f,
                    text: s(),
                    token_flags: f,
                },
            )),
            BoxedData::TemplateHead(Box::new(a::TemplateHeadData {
                raw_text: s(),
                template_flags: f,
                text: s(),
                token_flags: f,
            })),
            BoxedData::TemplateMiddle(Box::new(a::TemplateMiddleData {
                raw_text: s(),
                template_flags: f,
                text: s(),
                token_flags: f,
            })),
            BoxedData::TemplateTail(Box::new(a::TemplateTailData {
                raw_text: s(),
                template_flags: f,
                text: s(),
                token_flags: f,
            })),
            BoxedData::JsxText(Box::new(a::JsxTextData {
                contains_only_trivia_white_spaces: false,
                text: s(),
                token_flags: f,
            })),
            BoxedData::ThisTypeNode(Box::new(a::ThisTypeNodeData)),
            BoxedData::OmittedExpression(Box::new(a::OmittedExpressionData)),
            BoxedData::SemicolonClassElement(Box::new(a::SemicolonClassElementData)),
            BoxedData::EmptyStatement(Box::new(a::EmptyStatementData)),
            BoxedData::DebuggerStatement(Box::new(a::DebuggerStatementData)),
            BoxedData::JsxOpeningFragment(Box::new(a::JsxOpeningFragmentData)),
            BoxedData::JsxClosingFragment(Box::new(a::JsxClosingFragmentData)),
            BoxedData::JsDocAllType(Box::new(a::JsDocAllTypeData)),
            BoxedData::JsDocText(Box::new(a::JsDocTextData { text: Vec::new() })),
            BoxedData::NotEmittedStatement(Box::new(a::NotEmittedStatementData)),
            BoxedData::NotEmittedTypeElement(Box::new(a::NotEmittedTypeElementData)),
        ];
        // `n` is the node of `d`: a list field reads the lists of its file.
        let fields = |kind: SyntaxKind, n: Node, d: NodeData<'_>| {
            let mut fields = 0;
            for_each_child_field(kind, n, d, |_| fields += 1);
            fields
        };
        // The leaves are in no file, so a child that a wrong arm visits
        // reads no store: a parallel test may free file 0 or file 1, and a
        // debug build checks that the file of a node is not freed
        // (`file_block`).
        let no_file = Node(((crate::ast::store::FILE_ID_LIMIT as u64) << 32) | 1);
        for d in &leaves {
            assert_eq!(
                fields(SyntaxKind::Unknown, no_file, d.view()),
                0,
                "{}",
                d.view().schema_name()
            );
        }
        for k in 0..SyntaxKind::COUNT as u16 {
            let kind = SyntaxKind::try_from(k).unwrap();
            let leaf = leaves.iter().any(|d| d.view().matches_syntax_kind(kind));
            assert_eq!(has_no_child_fields(kind), leaf, "{kind:?}");
        }

        fn collect(n: Node, out: &mut Vec<Node>) {
            out.push(n);
            n.for_each_child(|c| {
                collect(c, out);
                false
            });
        }
        let text = "@dec export abstract class C<T extends object = {}> extends B implements I {\n\
            ;  #p?: T; static { debugger; } constructor(private a: number) { super(); }\n\
            get g(): this { return this; } set g(v) {} [k: string]: any; m?<U>(...r: U[]): void;\n\
            }\n\
            interface I { (x: number): string; new (): I; readonly [P in keyof T]-?: T[P] }\n\
            type M<T> = { [P in keyof T as `k${string & P}`]+?: T[P] extends infer U ? U : never };\n\
            type F = typeof import('m', { with: { a: 'b' } }).x | (new () => unknown) & [a: 1, b?: 2, ...c: 3[]];\n\
            enum E { A = 1 << 2, B }\n\
            declare module 'm' { export = x; }\n\
            namespace N.M { export import Q = N.Q; }\n\
            import d, { a as b, type c } from 'm' with { type: 'json' };\n\
            import * as ns from 'm';\n\
            export { a as default } from 'm';\n\
            export * as e from 'm';\n\
            label: for (const [a, , ...b] of x) { if (a) continue label; else break; }\n\
            for (let i = 0; i < 1; i++) for (const k in o) while (k) do { ; } while (0);\n\
            switch (x) { case 1: throw new Error(); default: }\n\
            try { using r = f(); } catch ({ e = 1 }) { } finally { }\n\
            async function* g() { yield* await x!; return void delete o[1]?.[2]; }\n\
            const o = { a, b: 1, ...c, ['d']: 2, m() {}, get x() { return 1; } } satisfies O as const;\n\
            let t = tag<string>`a${1}b${2}c` + /re/g.source + 1n + (x, y++) - ~!x + (-x) ** 2;\n\
            let f2 = function () { return <T,>(a: T): a is T => true; };\n\
            x ??= y ||= z &&= import.meta.url ? new.target : null;\n\
            declare function h(this: Window, x: unknown): asserts x is string;\n\
            declare const u: unique symbol;\n";
        let opts = SourceFileParseOptions {
            file_name: "/leaves.ts".to_string(),
            ..Default::default()
        };
        let root = parse_source_file(&opts, text, ScriptKind::TS).root;
        let mut nodes = Vec::new();
        collect(root, &mut nodes);
        let (mut leaves_seen, mut inner_seen) = (0, 0);
        for &n in &nodes {
            let kind = n.kind();
            let leaf = with_ast_data(n, |d| fields(kind, n, d)) == 0;
            assert_eq!(has_no_child_fields(kind), leaf, "{kind:?}");
            if leaf {
                leaves_seen += 1;
            } else {
                inner_seen += 1;
            }
        }
        assert!(
            leaves_seen > 50 && inner_seen > 100,
            "{leaves_seen} {inner_seen}"
        );
    }
}
