//! Port of Effect-TS/tsgo `internal/rules/leaking_requirements.go`.

use crate::effect::diag;
use crate::effect::etscore::*;
use crate::effect::rule::*;
use crate::effect::typeparser::*;
use crate::prelude::*;

// LeakingRequirements detects when service methods inadvertently expose implementation
// dependencies (Requirements) in their public type signatures. When every method of a
// service requires the same dependency types from callers, this is typically a sign that
// those dependencies should be resolved at Layer creation time instead.
// Supports both V3 and V4, default severity suggestion.
pub static LEAKING_REQUIREMENTS: Rule = Rule {
    name: "leakingRequirements",
    group: "antipattern",
    description: "Detects implementation services leaked in service methods",
    default_severity: Severity::Suggestion,
    supported_effect: &["v3", "v4"],
    codes: &[377041],
    run: run_leaking_requirements,
};

// Collect types to check and the node to report on
struct TypeToCheck {
    t: TypeId,
    report_node: Node,
}

fn run_leaking_requirements(ctx: &mut RuleContext<'_, '_>) -> Vec<Diagnostic> {
    let mut diags = Vec::new();

    // Stack-based traversal
    // PERF: Go walks the tree with a stack of nodes; this stack holds node
    // order indexes and pushes and pops at the same points (effperf4).
    let order = ctx.tp.node_order(ctx.source_file);
    let mut node_to_visit: Vec<u32> = order.children(0).collect();

    while let Some(i) = node_to_visit.pop() {
        let node = order.node(i);
        let mut types_to_check: Vec<TypeToCheck> = Vec::new();

        match node.kind() {
            SyntaxKind::CallExpression => {
                let call = node;
                if call.expression().is_some()
                    && call.expression().kind() == SyntaxKind::PropertyAccessExpression
                {
                    let prop_access = call.expression();
                    if prop_access.name().is_some()
                        && prop_access.name().kind() == SyntaxKind::Identifier
                    {
                        let name = get_text_of_node(prop_access.name());
                        if name == "GenericTag" || name == "Service" {
                            let node_type = ctx.tp.get_type_at_location(node);
                            if node_type.is_some() {
                                types_to_check.push(TypeToCheck {
                                    t: node_type,
                                    report_node: node,
                                });
                            }
                        }
                    }
                }
            }
            SyntaxKind::ClassDeclaration => {
                if node.name().is_some() && !node.heritage_clauses().is_nil() {
                    let class_sym = ctx.tp.get_symbol_at_location(node.name());
                    if class_sym.is_some() {
                        let class_type = ctx
                            .tp
                            .checker
                            .get_type_of_symbol_at_location(class_sym, node);
                        if class_type.is_some() {
                            types_to_check.push(TypeToCheck {
                                t: class_type,
                                report_node: node.name(),
                            });
                        }
                    }
                }
            }
            _ => {}
        }

        if types_to_check.is_empty() {
            // No patterns matched, enqueue children
            node_to_visit.extend(order.children(i));
            continue;
        }

        // Check each collected type
        let mut matched = false;
        for ttc in &types_to_check {
            // Try ContextTag first, fall back to ServiceType
            let mut service = ctx.tp.context_tag(ttc.t);
            if service.is_none() {
                service = ctx.tp.service_type(ttc.t);
            }
            let Some(service) = service else {
                continue;
            };

            let mut leaked = parse_leaked_requirements(ctx.tp, service.shape, node);
            if !leaked.is_empty() {
                leaked =
                    filter_expected_leaking_requirements(ctx.tp.checker, ttc.report_node, leaked);
            }
            if !leaked.is_empty() {
                matched = true;

                // Sort deterministically by type name (alphabetical)
                // PORT: Go's sort.Slice calls TypeToString in the comparator; the
                // names are computed first here. Go's sort is not stable.
                let mut keyed: Vec<(String, TypeId)> = leaked
                    .iter()
                    .map(|&t| (ctx.tp.checker.type_to_string_exported(t), t))
                    .collect();
                keyed.sort_by(|a, b| a.0.cmp(&b.0));
                leaked = keyed.into_iter().map(|(_, t)| t).collect();

                // Format as "TypeA | TypeB | TypeC"
                let mut type_names: Vec<String> = Vec::new();
                for &t in &leaked {
                    type_names.push(ctx.tp.checker.type_to_string_exported(t));
                }
                let formatted = type_names.join(" | ");

                diags.push(ctx.new_diagnostic(
                    ctx.source_file,
                    ctx.get_error_range(ttc.report_node),
                    diag::Methods_of_this_Service_require_0_from_every_caller_The_requirement_becomes_part_of_the_public_service_surface_instead_of_remaining_internal_to_Layer_implementation_Resolve_these_dependencies_at_Layer_creation_and_provide_them_to_each_method_so_the_service_s_type_reflects_its_purpose_not_its_implementation_To_suppress_this_diagnostic_for_specific_dependency_types_that_are_intentionally_passed_through_e_g_HttpServerRequest_add_effect_leakable_service_JSDoc_to_their_interface_declarations_or_to_this_service_by_adding_a_effect_expect_leaking_0_JSDoc_More_info_and_examples_at_https_Colon_Slash_Slasheffect_website_Slashdocs_Slashrequirements_management_Slashlayers_Slash_avoiding_requirement_leakage_effect_leakingRequirements,
                    Vec::new(),
                    vec![formatted],
                ));
            } else {
                matched = true;
            }
        }

        if !matched {
            // Type resolution failed for all candidates, continue visiting children
            node_to_visit.extend(order.children(i));
        }
    }

    diags
}

fn filter_expected_leaking_requirements(
    c: &mut Checker,
    report_node: Node,
    leaked: Vec<TypeId>,
) -> Vec<TypeId> {
    if report_node.is_nil() || leaked.is_empty() {
        return leaked;
    }

    let mut filtered = Vec::new();
    for leaked_type in leaked {
        if leaked_type.is_nil() {
            continue;
        }
        let name = c.type_to_string_exported(leaked_type);
        if is_expected_leaking_service_suppressed(report_node, &name) {
            continue;
        }
        filtered.push(leaked_type);
    }

    filtered
}

// PORT: Go also takes the checker and returns false when it is nil; the
// port always has one, so the parameter is dropped.
fn is_expected_leaking_service_suppressed(start_node: Node, leaked_service_name: &str) -> bool {
    if start_node.is_nil() || leaked_service_name.is_empty() {
        return false;
    }

    let source_file = get_source_file_of_node(start_node);
    if source_file.is_nil() {
        return false;
    }

    find_ancestor_or_quit(start_node, |current| {
        if current.is_nil() {
            return FindAncestorResult::FIND_ANCESTOR_FALSE;
        }

        if has_expected_leaking_comment(
            &source_file_text(source_file),
            current.pos(),
            leaked_service_name,
        ) {
            return FindAncestorResult::FIND_ANCESTOR_TRUE;
        }

        if is_class_declaration(current)
            || is_variable_statement(current)
            || is_expression_statement(current)
            || is_statement(current)
        {
            return FindAncestorResult::FIND_ANCESTOR_QUIT;
        }

        FindAncestorResult::FIND_ANCESTOR_FALSE
    })
    .is_some()
}

fn has_expected_leaking_comment(source_text: &str, pos: i32, leaked_service_name: &str) -> bool {
    if source_text.is_empty()
        || leaked_service_name.is_empty()
        || pos < 0
        || pos as usize > source_text.len()
    {
        return false;
    }

    for comment_range in crate::frontend::scanner::get_leading_comment_ranges(
        &NodeFactory::default(),
        source_text,
        pos,
    ) {
        let start = comment_range.pos();
        let end = comment_range.end();
        if start < 0 || end < 0 || start >= end || end as usize > source_text.len() {
            continue;
        }

        let comment_text = &source_text[start as usize..end as usize];
        for line in comment_text.split('\n') {
            let Some((_, suffix)) = line.split_once("@effect-expect-leaking") else {
                continue;
            };
            if suffix.contains(leaked_service_name) {
                return true;
            }
        }
    }

    false
}

// parseLeakedRequirements analyzes the service shape to find requirement types that
// are shared across all effect-typed members. This is the "leaking requirements" heuristic.
fn parse_leaked_requirements(
    tp: &mut TypeParser<'_>,
    service_shape: TypeId,
    at_location: Node,
) -> Vec<TypeId> {
    let properties = tp.checker.get_properties_of_type_exported(service_shape);
    if properties.is_empty() {
        return Vec::new();
    }

    let mut memory: FxHashMap<String, TypeId> = FxHashMap::default();
    let mut shared_requirements_keys: Vec<String> = Vec::new();
    let mut shared_initialized = false;
    let mut effect_members = 0;

    let mut should_exclude = |tp: &mut TypeParser<'_>, t: TypeId| -> bool {
        // Exclude never
        if tp.checker.ty(t).flags.intersects(TypeFlags::NEVER) {
            return true;
        }
        // Exclude Scope types
        if tp.is_scope_type(t) {
            return true;
        }
        false
    };

    for property in properties {
        if property.is_nil() {
            continue;
        }

        let service_property_type = tp
            .checker
            .get_type_of_symbol_at_location(property, at_location);
        if service_property_type.is_nil() {
            continue;
        }

        // Try to get the Effect's R type - either directly from the property type
        // or from the return type of a single call signature
        let mut effect_context_type = TypeId::NIL;

        let effect = tp.effect_type(service_property_type);
        if let Some(effect) = effect {
            effect_context_type = effect.r;
        } else {
            // Try call signature: if exactly 1 call signature, parse return type as Effect
            let sigs = tp
                .checker
                .get_signatures_of_type_exported(service_property_type, SignatureKind::CALL);
            if sigs.len() == 1 {
                let ret_type = tp.checker.get_return_type_of_signature_exported(sigs[0]);
                if ret_type.is_some() {
                    let ret_effect = tp.effect_type(ret_type);
                    if let Some(ret_effect) = ret_effect {
                        effect_context_type = ret_effect.r;
                    }
                }
            }
        }

        if effect_context_type.is_nil() {
            continue;
        }

        effect_members += 1;
        let result = tp.append_to_unique_types_map(
            &mut memory,
            effect_context_type,
            Some(&mut should_exclude),
        );

        if !shared_initialized {
            shared_requirements_keys = result.all_indexes;
            shared_initialized = true;
        } else {
            // Intersect with current keys
            shared_requirements_keys =
                intersect_string_slices(&shared_requirements_keys, &result.all_indexes);
            if shared_requirements_keys.is_empty() {
                return Vec::new();
            }
        }
    }

    // Need at least 2 effect members for the heuristic
    if !shared_initialized || shared_requirements_keys.is_empty() || effect_members < 2 {
        return Vec::new();
    }

    // Collect the shared requirement types, filtering out those with @effect-leakable-service
    let mut leaked = Vec::new();
    for key in &shared_requirements_keys {
        let t = memory.get(key).copied().unwrap_or(TypeId::NIL);
        if t.is_nil() {
            continue;
        }
        if has_leakable_service_annotation(tp.checker, t) {
            continue;
        }
        leaked.push(t);
    }

    leaked
}

// hasLeakableServiceAnnotation checks if a type's declarations contain the
// @effect-leakable-service JSDoc annotation.
fn has_leakable_service_annotation(c: &mut Checker, t: TypeId) -> bool {
    let mut sym = c.ty(t).symbol;
    if sym.is_nil() {
        return false;
    }

    // Resolve aliases
    if c.sym(sym).flags.intersects(SymbolFlags::ALIAS) {
        let resolved = c.get_aliased_symbol(sym);
        if resolved.is_some() {
            sym = resolved;
        }
    }

    let declarations: Vec<Node> = c.sym(sym).declarations.iter().copied().collect();
    for decl in declarations {
        if decl.is_nil() {
            continue;
        }
        let sf = get_source_file_of_node(decl);
        if sf.is_nil() {
            continue;
        }
        let text = source_file_text(sf);
        let start = decl.pos();
        let end = decl.end();
        if start < 0 || end < 0 || start >= end || end as usize > text.len() {
            continue;
        }
        let snippet = text[start as usize..end as usize].to_lowercase();
        if snippet.contains("@effect-leakable-service") {
            return true;
        }
    }

    false
}

// intersectStringSlices returns the elements that appear in both slices.
fn intersect_string_slices(a: &[String], b: &[String]) -> Vec<String> {
    let set: FxHashSet<&String> = b.iter().collect();
    let mut result = Vec::new();
    for s in a {
        if set.contains(s) {
            result.push(s.clone());
        }
    }
    result
}
