//! Port of Effect-TS/tsgo `internal/typeparser` at `@effect/tsgo@0.46.1`
//! (`f1a7cad0`). It recognizes Effect values, services, layers, schemas and
//! call shapes from checker types. One file per Go file, in Go order.
//!
//! Go `Cached(&tp.links.X, key, compute)` is `cached!(self, x, key, compute)`.
//! The links live on the checker (Go `Checker.EffectLinks`), so every
//! `TypeParser` of one checker shares them.

use crate::prelude::*;

mod cause_type;
mod constant_evaluation;
mod context_tag;
mod context_type;
mod could_be_strict_effect;
mod data_first_signature;
mod data_type;
mod discover;
mod effect_context;
mod effect_fn;
mod effect_fn_opportunity;
mod effect_gen;
mod effect_model_type;
mod effect_type;
mod effect_yieldable_type;
mod execution_flow;
mod expected_and_real_type;
mod expression_stability;
mod extends_context_service;
mod extends_context_tag;
mod extends_data_tagged;
mod extends_effect_model_class;
mod extends_effect_service;
mod extends_effect_tag;
mod extends_schema_class;
mod extends_schema_opaque;
mod extends_schema_tagged;
mod extends_sql_model_class;
mod function_node;
mod get_type_at_location;
mod global_error;
mod helpers;
mod identity_forwarder;
mod layer_type;
mod lazy_expression;
mod module_export_reference;
mod module_identifier;
mod object_literal;
mod option_type;
mod packagejson;
mod pipeable;
mod pipeable_signature_shape;
mod piping_flow;
mod promise_type;
mod reconstruct;
mod result_dispatch;
mod returning_dispatch;
mod schema_type;
mod scope_type;
mod service_type;
mod sql_model_type;
mod stream_type;
mod unique_types_map;
mod unroll_members;
mod vitest_api;
mod yieldable_error;

pub use cause_type::*;
pub use constant_evaluation::*;
pub use context_tag::*;
pub use context_type::*;
pub use could_be_strict_effect::*;
pub use data_first_signature::*;
pub use data_type::*;
pub use discover::*;
pub use effect_context::*;
pub use effect_fn::*;
pub use effect_fn_opportunity::*;
pub use effect_gen::*;
pub use effect_model_type::*;
pub use effect_type::*;
pub use effect_yieldable_type::*;
pub use execution_flow::*;
pub use expected_and_real_type::*;
pub use expression_stability::*;
pub use extends_context_service::*;
pub use extends_context_tag::*;
pub use extends_data_tagged::*;
pub use extends_effect_model_class::*;
pub use extends_effect_service::*;
pub use extends_effect_tag::*;
pub use extends_schema_class::*;
pub use extends_schema_opaque::*;
pub use extends_schema_tagged::*;
pub use extends_sql_model_class::*;
pub use function_node::*;
pub use get_type_at_location::*;
pub use global_error::*;
pub use helpers::*;
pub use identity_forwarder::*;
pub use layer_type::*;
pub use lazy_expression::*;
pub use module_export_reference::*;
pub use module_identifier::*;
pub use object_literal::*;
pub use option_type::*;
pub use packagejson::*;
pub use pipeable::*;
pub use pipeable_signature_shape::*;
pub use piping_flow::*;
pub use promise_type::*;
pub use reconstruct::*;
pub use result_dispatch::*;
pub use returning_dispatch::*;
pub use schema_type::*;
pub use scope_type::*;
pub use service_type::*;
pub use sql_model_type::*;
pub use stream_type::*;
pub use unique_types_map::*;
pub use unroll_members::*;
pub use vitest_api::*;
pub use yieldable_error::*;

/// Go `TypeParser`. It borrows the checker for one rule run.
pub struct TypeParser<'c> {
    pub program: &'static GoProgram,
    pub checker: &'c mut Checker,
}

/// Go `EffectLinks`: the per-checker caches. A present key with a `None`
/// value is a cached Go nil, as `LinkStore.TryGet` gives.
///
/// PORT: the caches keep Go's keys and entries, but the large node caches
/// use node link pages (`LinkStore`, Go `core.PagedLinkStore`) with small
/// slots: rules ask about most nodes of a file. On the effect project these
/// caches held about 110 MB in hash maps (effcache1).
#[derive(Default)]
pub struct EffectLinks {
    pub type_at_location: LinkStore<Node, CachedId>,
    pub effect_type: FxHashMap<TypeId, Option<Rc<Effect>>>,
    pub stream_type: FxHashMap<TypeId, Option<Rc<Effect>>>,
    pub strict_effect_type: FxHashMap<TypeId, Option<Rc<Effect>>>,
    pub effect_subtype: FxHashMap<TypeId, Option<Rc<Effect>>>,
    pub fiber_type: FxHashMap<TypeId, Option<Rc<Effect>>>,
    pub effect_yieldable_type: FxHashMap<TypeId, Option<Rc<Effect>>>,
    pub has_effect_type_id: FxHashMap<TypeId, bool>,
    pub layer_type: FxHashMap<TypeId, Option<Rc<Layer>>>,
    pub service_type: FxHashMap<TypeId, Option<Rc<Service>>>,
    pub context_tag: FxHashMap<TypeId, Option<Rc<Service>>>,
    pub effect_schema_types: FxHashMap<TypeId, Option<Rc<SchemaTypes>>>,
    pub is_scope_type: FxHashMap<TypeId, bool>,
    pub is_pipeable_type: FxHashMap<TypeId, bool>,
    pub promise_type: FxHashMap<TypeId, TypeId>,
    pub is_global_error_type: FxHashMap<TypeId, bool>,
    pub is_yieldable_error_type: FxHashMap<TypeId, bool>,
    pub reference_symbol: LinkStore<Node, CachedId>,
    pub module_export_reference: FxHashMap<ModuleExportReferenceCacheKey, bool>,
    /// The member names of `module_export_reference` keys, by id.
    pub module_export_members: FxHashMap<Box<str>, u32>,
    pub pipeable_signature_shape: FxHashMap<PipeableSignatureShapeCacheKey, bool>,

    pub extends_context_tag: FxHashMap<Node, Option<Rc<ContextTagResult>>>,
    pub extends_data_tagged_error: FxHashMap<Node, Option<Rc<DataTaggedErrorResult>>>,
    pub extends_effect_model_class: FxHashMap<Node, Option<Rc<EffectModelClassResult>>>,
    pub extends_effect_service: FxHashMap<Node, Option<Rc<EffectServiceResult>>>,
    pub extends_effect_tag: FxHashMap<Node, Option<Rc<EffectTagResult>>>,
    pub extends_schema_class: FxHashMap<Node, Option<Rc<SchemaClassResult>>>,
    pub extends_schema_opaque: FxHashMap<Node, Option<Rc<SchemaOpaqueResult>>>,
    pub extends_schema_request_class: FxHashMap<Node, Option<Rc<SchemaClassResult>>>,
    pub extends_schema_tagged_class: FxHashMap<Node, Option<Rc<SchemaTaggedResult>>>,
    pub extends_schema_tagged_error: FxHashMap<Node, Option<Rc<SchemaTaggedResult>>>,
    pub extends_schema_tagged_request: FxHashMap<Node, Option<Rc<SchemaTaggedResult>>>,
    pub extends_service_map_service: FxHashMap<Node, Option<Rc<ServiceMapServiceResult>>>,
    pub extends_effect_sql_model_class: FxHashMap<Node, Option<Rc<SqlModelClassResult>>>,

    pub effect_gen_call: FxHashMap<Node, Option<Rc<EffectGenCallResult>>>,
    pub effect_fn_call: FxHashMap<Node, Option<Rc<EffectFnCallResult>>>,
    pub parse_effect_fn_opportunity: NodeOptCache<EffectFnOpportunityResult>,
    pub parse_pipe_call: FxHashMap<Node, Option<Rc<ParsedPipeCallResult>>>,
    pub execution_flow: FxHashMap<Node, Option<Rc<ExecutionFlow>>>,
    pub effect_context_flags: LinkStore<Node, EffectContextFlags>,
    pub effect_yield_generator_function: LinkStore<Node, CachedNode>,

    pub discover_packages_computed: bool,
    pub discover_packages_value: Vec<DiscoveredPackage>,
    pub detect_effect_version_computed: bool,
    pub detect_effect_version_value: EffectMajorVersion,
    pub package_json_for_source_file: FxHashMap<Node, Option<PackageJsonRef>>,
    pub effect_context_analyzed: FxHashMap<Node, bool>,
    pub expected_and_real_types: FxHashMap<Node, Rc<Vec<ExpectedAndRealType>>>,
    pub piping_flows_with_effect_fn: FxHashMap<Node, Rc<Vec<Rc<PipingFlow>>>>,
    pub piping_flows_without_effect_fn: FxHashMap<Node, Rc<Vec<Rc<PipingFlow>>>>,
}

/// A cached `TypeId` or `SymbolId` in a node link page. Go caches nil
/// too, so the slot keeps the handle plus one: `Option<CachedId>` is 4
/// bytes, a page of 64 nodes 256 bytes. The default is nil.
#[derive(Clone, Copy)]
pub struct CachedId(std::num::NonZeroU32);

impl CachedId {
    #[must_use]
    pub fn new(handle: u32) -> Self {
        Self(std::num::NonZeroU32::new(handle.wrapping_add(1)).expect("handle below u32::MAX"))
    }

    #[must_use]
    pub fn get(self) -> u32 {
        self.0.get() - 1
    }
}

impl Default for CachedId {
    fn default() -> Self {
        Self::new(0)
    }
}

/// A cached `Node` in a node link page, kept like `CachedId`: 8 bytes in
/// an `Option`. The default is nil.
#[derive(Clone, Copy)]
pub struct CachedNode(std::num::NonZeroU64);

impl CachedNode {
    #[must_use]
    pub fn new(node: Node) -> Self {
        Self(std::num::NonZeroU64::new(node.0.wrapping_add(1)).expect("node handle below u64::MAX"))
    }

    #[must_use]
    pub fn get(self) -> Node {
        Node(self.0.get() - 1)
    }
}

impl Default for CachedNode {
    fn default() -> Self {
        Self::new(Node::NIL)
    }
}

/// Go `core.LinkStore[*ast.Node, *T]` for a cache that rules fill for
/// nearly every node of a file and that is nil for most of them
/// (`ParseEffectFnOpportunity`). Node link pages mark the cached nodes (one
/// byte each), and a map holds the results that are not nil.
pub struct NodeOptCache<T> {
    cached: LinkStore<Node, ()>,
    values: FxHashMap<Node, Rc<T>>,
}

impl<T> Default for NodeOptCache<T> {
    fn default() -> Self {
        Self {
            cached: LinkStore::default(),
            values: FxHashMap::default(),
        }
    }
}

impl<T> NodeOptCache<T> {
    /// Go `store.TryGet(node)`: `None` when `node` is not cached.
    #[must_use]
    pub fn get(&self, node: Node) -> Option<Option<Rc<T>>> {
        self.cached
            .has(node)
            .then(|| self.values.get(&node).cloned())
    }

    /// Go `*store.Get(node) = value`.
    pub fn insert(&mut self, node: Node, value: Option<Rc<T>>) {
        self.cached.get(node);
        match value {
            Some(value) => self.values.insert(node, value),
            None => self.values.remove(&node),
        };
    }
}

/// Go `Cached(&tp.links.<field>, key, compute)`: the cached value, or
/// `compute` stored and returned. `compute` may use the type parser.
#[macro_export]
#[doc(hidden)]
macro_rules! effect_cached {
    ($tp:expr, $field:ident, $key:expr, $compute:expr) => {{
        let key = $key;
        if let Some(value) = $tp.links().$field.get(&key) {
            value.clone()
        } else {
            let value = $compute;
            $tp.links().$field.insert(key, value.clone());
            value
        }
    }};
}
pub use crate::effect_cached as cached;

impl<'c> TypeParser<'c> {
    // Go: typeparser/type_parser.go NewTypeParser
    pub fn new(program: &'static GoProgram, checker: &'c mut Checker) -> Self {
        if checker.effect_links.is_none() {
            checker.effect_links = Some(Box::default());
        }
        TypeParser { program, checker }
    }

    /// Go `tp.links`.
    pub fn links(&mut self) -> &mut EffectLinks {
        self.checker.effect_links.get_or_insert_with(Box::default)
    }
}

// PORT: no Go counterpart. The small cache slots of `EffectLinks` keep Go's
// `Cached` meaning: a nil result is cached, and an absent key is not.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_cache_slots_keep_cached_nil() {
        let file_node = Node((3 << 32) | 70);
        let synthetic_node = Node((u64::from(u32::MAX) << 32) | 5);

        assert_eq!(CachedId::default().get(), 0);
        assert_eq!(CachedId::new(7).get(), 7);
        assert_eq!(CachedNode::default().get(), Node::NIL);
        assert_eq!(CachedNode::new(file_node).get(), file_node);

        let mut ids: LinkStore<Node, CachedId> = LinkStore::default();
        for node in [file_node, synthetic_node] {
            assert!(ids.try_get(node).is_none());
            *ids.get(node) = CachedId::new(TypeId::NIL.0);
            assert_eq!(ids.try_get(node).map(|id| id.get()), Some(0));
        }

        let mut opt: NodeOptCache<u32> = NodeOptCache::default();
        for node in [file_node, synthetic_node] {
            assert_eq!(opt.get(node), None);
            opt.insert(node, Some(Rc::new(1)));
            assert_eq!(opt.get(node), Some(Some(Rc::new(1))));
            opt.insert(node, None);
            assert_eq!(opt.get(node), Some(None));
        }
    }
}
