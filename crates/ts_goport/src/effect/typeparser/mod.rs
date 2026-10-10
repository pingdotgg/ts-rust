//! Port of Effect-TS/tsgo `internal/typeparser` at `@effect/tsgo@0.51.1`
//! (`47cb1ed7`). It recognizes Effect values, services, layers, schemas and
//! call shapes from checker types. One file per Go file, in Go order.
//!
//! Go `Cached(&tp.links.X, key, compute)` is `cached!(self, x, key, compute)`.
//! The links live on the checker (Go `Checker.EffectLinks`), so every
//! `TypeParser` of one checker shares them.

use crate::prelude::*;

mod api_stability_inheritance;
mod api_stability_p1;
mod api_stability_p2;
mod api_stability_p3;
mod api_stability_safety;
mod cause_type;
pub mod checker_integration;
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

pub use api_stability_inheritance::*;
pub use api_stability_p1::*;
pub use api_stability_p2::*;
pub use api_stability_p3::*;
pub use api_stability_safety::*;
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
#[derive(Default)]
pub struct EffectLinks {
    pub type_at_location: FxHashMap<Node, TypeId>,
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
    pub reference_symbol: FxHashMap<Node, SymbolId>,
    pub module_export_reference: FxHashMap<ModuleExportReferenceCacheKey, bool>,
    pub pipeable_signature_shape: FxHashMap<PipeableSignatureShapeCacheKey, bool>,
    // API-stability caches. Declared lookups persist per checker: the declared
    // stability of a symbol, of a raw signature overload and of one
    // declaration, shared with the unstableApiUsage/experimentalApiUsage rules.
    // Declared lookups never trigger computed analysis. A computed stability
    // surface lives in an analysis-local ApiStabilitySession, and every
    // complete, settled, context-free concrete type or signature surface is
    // additionally published here as an immutable snapshot so a later export,
    // file or TypeParser over the same checker composes it instead of
    // recomputing. The snapshot excludes the component's own declared tag,
    // which stays in the declared caches and is re-added by each consumer; a
    // substitution-context or incomplete/blocked result is never published and
    // stays analysis-local. Ceilings, locations and diagnostics are never
    // cached.
    pub api_stability_declared_symbol: FxHashMap<SymbolId, ApiStabilityDeclaration>,
    pub api_stability_declared_signature: FxHashMap<SignatureId, ApiStabilityDeclaration>,
    pub api_stability_declared_declaration: FxHashMap<Node, ApiStabilityDeclaration>,
    pub api_stability_surface_type:
        FxHashMap<ApiStabilitySurfaceTypeKey, ApiStabilitySharedSurface>,
    pub api_stability_surface_signature: FxHashMap<SignatureId, ApiStabilitySharedSurface>,

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
    pub parse_effect_fn_opportunity: FxHashMap<Node, Option<Rc<EffectFnOpportunityResult>>>,
    pub parse_pipe_call: FxHashMap<Node, Option<Rc<ParsedPipeCallResult>>>,
    pub execution_flow: FxHashMap<Node, Option<Rc<ExecutionFlow>>>,
    pub effect_context_flags: FxHashMap<Node, EffectContextFlags>,
    pub effect_yield_generator_function: FxHashMap<Node, Node>,

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
