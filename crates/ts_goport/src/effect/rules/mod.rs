//! Port of Effect-TS/tsgo `internal/rules`: every Effect diagnostic rule,
//! one file per Go file. `ALL` keeps Go's `rules.All` order, which is the
//! order the runner runs them in. Files use each other's helpers through
//! `super::<file>::<name>`.

use crate::effect::rule::Rule;

pub mod abort_controller_in_effect;
pub mod acquire_release_disposable;
pub mod all_of_map_to_for_each;
pub mod any_unknown_in_error_context;
pub mod api_stability_leak;
pub mod async_function;
pub mod catch_all_tag_dispatch_to_catch_tag;
pub mod catch_all_to_map_error;
pub mod catch_chain_to_first_success_of;
pub mod catch_conditional_refail_to_catch_if;
pub mod catch_die_to_or_die;
pub mod catch_if_tag_to_catch_tag;
pub mod catch_refail_to_tap_error;
pub mod catch_tag_to_catch_reason;
pub mod catch_to_ignore;
pub mod catch_to_or_else_succeed;
pub mod catch_unfailable_effect;
pub mod class_self_mismatch;
pub mod crypto_random_uuid;
pub mod deterministic_keys;
pub mod duplicate_package;
pub mod effect_do_notation;
pub mod effect_fn_iife;
pub mod effect_fn_implicit_any;
pub mod effect_fn_opportunity;
pub mod effect_gen_uses_adapter;
pub mod effect_in_failure;
pub mod effect_in_void_success;
pub mod effect_map_flatten;
pub mod effect_map_void;
pub mod effect_succeed_with_void;
pub mod extends_native_error;
pub mod flat_map_conditional_to_filter_or_fail;
pub mod flat_map_ignored_param_to_and_then;
pub mod flat_map_to_map;
pub mod floating_effect;
pub mod floating_effect_in_vitest;
pub mod generic_effect_services;
pub mod global_console;
pub mod global_date;
pub mod global_error_in_effect_catch;
pub mod global_error_in_effect_failure;
pub mod global_fetch;
pub mod global_random;
pub mod global_timers;
pub mod instance_of_schema;
pub mod layer_merge_all_with_dependencies;
pub mod lazy_effect;
pub mod lazy_promise_in_effect_sync;
pub mod leaking_requirements;
pub mod map_some_to_as_some;
pub mod match_effect_to_map_both;
pub mod match_effect_to_match;
pub mod missed_pipeable_opportunity;
pub mod missing_effect_context;
pub mod missing_effect_error;
pub mod missing_effect_service_dependency;
pub mod missing_layer_context;
pub mod missing_pipeable_signature;
pub mod missing_return_yield_star;
pub mod missing_star_in_yield_effect_gen;
pub mod multiple_catch_tag;
pub mod multiple_effect_provide;
pub mod nested_effect_gen_yield;
pub mod new_promise;
pub mod new_schema_class;
pub mod node_builtin_import;
pub mod non_object_effect_service_type;
pub mod obsolete_match_import;
pub mod obsolete_schema_import;
pub mod option_match_to_from_option;
pub mod outdated_api;
pub mod outdated_api_db;
pub mod overridden_schema_constructor;
pub mod prefer_schema_over_json;
pub mod prefer_schema_type_property;
pub mod prefer_succeed_some_or_none;
pub mod prefer_typed_schema_decoder;
pub mod prefer_unsafe_constructor;
pub mod process_env;
pub mod promise_in_effect_success;
pub mod provide_layer_succeed_to_provide_service;
pub mod race_first_with_sleep_to_timeout;
pub mod redundant_map_error;
pub mod redundant_or_die;
pub mod redundant_schema_tag_identifier;
pub mod result_dispatch;
pub mod return_effect_in_gen;
pub mod run_effect_inside_effect;
pub mod run_of_exit_to_run_exit;
pub mod schema_literal_non_finite;
pub mod schema_number;
pub mod schema_opaque_instance_member;
pub mod schema_struct_with_tag;
pub mod schema_sync;
pub mod schema_sync_in_effect;
pub mod schema_union_of_literals;
pub mod scope_in_layer_effect;
pub mod service_not_as_class;
pub mod stability_api_usage;
pub mod strict_boolean_expressions;
pub mod strict_effect_provide;
pub mod sync_to_succeed;
pub mod timeout_catch_tag_to_timeout_or_else;
pub mod try_catch_in_effect_gen;
pub mod unknown_in_effect_catch;
pub mod unnecessary_arrow_block;
pub mod unnecessary_effect_gen;
pub mod unnecessary_fail_yieldable_error;
pub mod unnecessary_pipe;
pub mod unnecessary_pipe_chain;
pub mod unnecessary_typeof_type;
pub mod unsafe_effect_type_assertion;

/// Go `rules.All`.
pub static ALL: &[&Rule] = &[
    &api_stability_leak::API_STABILITY_LEAK,
    &stability_api_usage::EXPERIMENTAL_API_USAGE,
    &stability_api_usage::UNSTABLE_API_USAGE,
    &floating_effect::FLOATING_EFFECT,
    &floating_effect_in_vitest::FLOATING_EFFECT_IN_VITEST,
    &missing_effect_error::MISSING_EFFECT_ERROR,
    &missing_effect_context::MISSING_EFFECT_CONTEXT,
    &missing_return_yield_star::MISSING_RETURN_YIELD_STAR,
    &missing_star_in_yield_effect_gen::MISSING_STAR_IN_YIELD_EFFECT_GEN,
    &catch_unfailable_effect::CATCH_UNFAILABLE_EFFECT,
    &catch_die_to_or_die::CATCH_DIE_TO_OR_DIE,
    &timeout_catch_tag_to_timeout_or_else::TIMEOUT_CATCH_TAG_TO_TIMEOUT_OR_ELSE,
    &catch_all_to_map_error::CATCH_ALL_TO_MAP_ERROR,
    &catch_refail_to_tap_error::CATCH_REFAIL_TO_TAP_ERROR,
    &catch_all_tag_dispatch_to_catch_tag::CATCH_ALL_TAG_DISPATCH_TO_CATCH_TAG,
    &catch_if_tag_to_catch_tag::CATCH_IF_TAG_TO_CATCH_TAG,
    &catch_conditional_refail_to_catch_if::CATCH_CONDITIONAL_REFAIL_TO_CATCH_IF,
    &catch_tag_to_catch_reason::CATCH_TAG_TO_CATCH_REASON,
    &catch_to_or_else_succeed::CATCH_TO_OR_ELSE_SUCCEED,
    &catch_to_ignore::CATCH_TO_IGNORE,
    &catch_chain_to_first_success_of::CATCH_CHAIN_TO_FIRST_SUCCESS_OF,
    &multiple_catch_tag::MULTIPLE_CATCH_TAG,
    &effect_fn_iife::EFFECT_FN_IIFE,
    &try_catch_in_effect_gen::TRY_CATCH_IN_EFFECT_GEN,
    &unnecessary_pipe::UNNECESSARY_PIPE,
    &return_effect_in_gen::RETURN_EFFECT_IN_GEN,
    &unnecessary_pipe_chain::UNNECESSARY_PIPE_CHAIN,
    &effect_succeed_with_void::EFFECT_SUCCEED_WITH_VOID,
    &prefer_succeed_some_or_none::PREFER_SUCCEED_SOME_OR_NONE,
    &unnecessary_effect_gen::UNNECESSARY_EFFECT_GEN,
    &effect_map_void::EFFECT_MAP_VOID,
    &unnecessary_fail_yieldable_error::UNNECESSARY_FAIL_YIELDABLE_ERROR,
    &effect_in_void_success::EFFECT_IN_VOID_SUCCESS,
    &effect_in_failure::EFFECT_IN_FAILURE,
    &unknown_in_effect_catch::UNKNOWN_IN_EFFECT_CATCH,
    &global_error_in_effect_catch::GLOBAL_ERROR_IN_EFFECT_CATCH,
    &global_error_in_effect_failure::GLOBAL_ERROR_IN_EFFECT_FAILURE,
    &abort_controller_in_effect::ABORT_CONTROLLER_IN_EFFECT,
    &crypto_random_uuid::CRYPTO_RANDOM_UUID,
    &crypto_random_uuid::CRYPTO_RANDOM_UUID_IN_EFFECT,
    &global_fetch::GLOBAL_FETCH,
    &global_fetch::GLOBAL_FETCH_IN_EFFECT,
    &process_env::PROCESS_ENV,
    &process_env::PROCESS_ENV_IN_EFFECT,
    &global_console::GLOBAL_CONSOLE,
    &global_console::GLOBAL_CONSOLE_IN_EFFECT,
    &global_date::GLOBAL_DATE,
    &global_date::GLOBAL_DATE_IN_EFFECT,
    &global_random::GLOBAL_RANDOM,
    &global_random::GLOBAL_RANDOM_IN_EFFECT,
    &global_timers::GLOBAL_TIMERS,
    &global_timers::GLOBAL_TIMERS_IN_EFFECT,
    &run_effect_inside_effect::RUN_EFFECT_INSIDE_EFFECT,
    &prefer_schema_over_json::PREFER_SCHEMA_OVER_JSON,
    &effect_gen_uses_adapter::EFFECT_GEN_USES_ADAPTER,
    &effect_fn_implicit_any::EFFECT_FN_IMPLICIT_ANY,
    &strict_boolean_expressions::STRICT_BOOLEAN_EXPRESSIONS,
    &any_unknown_in_error_context::ANY_UNKNOWN_IN_ERROR_CONTEXT,
    &async_function::ASYNC_FUNCTION,
    &scope_in_layer_effect::SCOPE_IN_LAYER_EFFECT,
    &strict_effect_provide::STRICT_EFFECT_PROVIDE,
    &unsafe_effect_type_assertion::UNSAFE_EFFECT_TYPE_ASSERTION,
    &multiple_effect_provide::MULTIPLE_EFFECT_PROVIDE,
    &provide_layer_succeed_to_provide_service::PROVIDE_LAYER_SUCCEED_TO_PROVIDE_SERVICE,
    &missing_layer_context::MISSING_LAYER_CONTEXT,
    &layer_merge_all_with_dependencies::LAYER_MERGE_ALL_WITH_DEPENDENCIES,
    &schema_struct_with_tag::SCHEMA_STRUCT_WITH_TAG,
    &schema_sync::SCHEMA_SYNC,
    &schema_sync_in_effect::SCHEMA_SYNC_IN_EFFECT,
    &schema_number::SCHEMA_NUMBER,
    &schema_literal_non_finite::SCHEMA_LITERAL_NON_FINITE,
    &schema_union_of_literals::SCHEMA_UNION_OF_LITERALS,
    &new_schema_class::NEW_SCHEMA_CLASS,
    &missing_effect_service_dependency::MISSING_EFFECT_SERVICE_DEPENDENCY,
    &leaking_requirements::LEAKING_REQUIREMENTS,
    &lazy_effect::LAZY_EFFECT,
    &lazy_promise_in_effect_sync::LAZY_PROMISE_IN_EFFECT_SYNC,
    &promise_in_effect_success::PROMISE_IN_EFFECT_SUCCESS,
    &nested_effect_gen_yield::NESTED_EFFECT_GEN_YIELD,
    &redundant_map_error::REDUNDANT_MAP_ERROR,
    &redundant_or_die::REDUNDANT_OR_DIE,
    &unnecessary_arrow_block::UNNECESSARY_ARROW_BLOCK,
    &unnecessary_typeof_type::UNNECESSARY_TYPEOF_TYPE,
    &prefer_schema_type_property::PREFER_SCHEMA_TYPE_PROPERTY,
    &instance_of_schema::INSTANCE_OF_SCHEMA,
    &generic_effect_services::GENERIC_EFFECT_SERVICES,
    &overridden_schema_constructor::OVERRIDDEN_SCHEMA_CONSTRUCTOR,
    &schema_opaque_instance_member::SCHEMA_OPAQUE_INSTANCE_MEMBER,
    &redundant_schema_tag_identifier::REDUNDANT_SCHEMA_TAG_IDENTIFIER,
    &class_self_mismatch::CLASS_SELF_MISMATCH,
    &effect_fn_opportunity::EFFECT_FN_OPPORTUNITY,
    &non_object_effect_service_type::NON_OBJECT_EFFECT_SERVICE_TYPE,
    &deterministic_keys::DETERMINISTIC_KEYS,
    &missed_pipeable_opportunity::MISSED_PIPEABLE_OPPORTUNITY,
    &missing_pipeable_signature::MISSING_PIPEABLE_SIGNATURE,
    &duplicate_package::DUPLICATE_PACKAGE,
    &effect_do_notation::EFFECT_DO_NOTATION,
    &effect_map_flatten::EFFECT_MAP_FLATTEN,
    &all_of_map_to_for_each::ALL_OF_MAP_TO_FOR_EACH,
    &map_some_to_as_some::MAP_SOME_TO_AS_SOME,
    &flat_map_to_map::FLAT_MAP_TO_MAP,
    &flat_map_ignored_param_to_and_then::FLAT_MAP_IGNORED_PARAM_TO_AND_THEN,
    &match_effect_to_match::MATCH_EFFECT_TO_MATCH,
    &match_effect_to_map_both::MATCH_EFFECT_TO_MAP_BOTH,
    &flat_map_conditional_to_filter_or_fail::FLAT_MAP_CONDITIONAL_TO_FILTER_OR_FAIL,
    &option_match_to_from_option::OPTION_MATCH_TO_FROM_OPTION,
    &sync_to_succeed::SYNC_TO_SUCCEED,
    &extends_native_error::EXTENDS_NATIVE_ERROR,
    &node_builtin_import::NODE_BUILTIN_IMPORT,
    &obsolete_match_import::OBSOLETE_MATCH_IMPORT,
    &obsolete_schema_import::OBSOLETE_SCHEMA_IMPORT,
    &new_promise::NEW_PROMISE,
    &outdated_api::OUTDATED_API,
    &service_not_as_class::SERVICE_NOT_AS_CLASS,
    &prefer_unsafe_constructor::PREFER_UNSAFE_CONSTRUCTOR,
    &prefer_typed_schema_decoder::PREFER_TYPED_SCHEMA_DECODER,
    &acquire_release_disposable::ACQUIRE_RELEASE_DISPOSABLE,
    &race_first_with_sleep_to_timeout::RACE_FIRST_WITH_SLEEP_TO_TIMEOUT,
    &run_of_exit_to_run_exit::RUN_OF_EXIT_TO_RUN_EXIT,
];
