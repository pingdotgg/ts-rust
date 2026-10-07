//! Port of typescript-go `internal/checker/checker.go` lines 26861-27804.

use crate::diagnostics::Message;
use crate::jsnum::Number;
use crate::prelude::*;
use smallvec::SmallVec;

/// The flags of the types whose base constraint Go
/// `getBaseConstraintOfType` resolves (with generic tuple types).
const BASE_CONSTRAINT_FLAGS: TypeFlags = TypeFlags::INSTANTIABLE_NON_PRIMITIVE
    .union(TypeFlags::UNION_OR_INTERSECTION)
    .union(TypeFlags::TEMPLATE_LITERAL)
    .union(TypeFlags::STRING_MAPPING)
    .union(TypeFlags::INDEX);

impl Checker {
    // Go: checker/checker.go:27466 getPropertyTypeForIndexType
    pub fn get_property_type_for_index_type(
        &mut self,
        original_object_type: TypeId,
        object_type: TypeId,
        index_type: TypeId,
        full_index_type: TypeId,
        access_node: Node,
        access_flags: AccessFlags,
    ) -> TypeId {
        let mut access_expression = Node::NIL;
        if access_node.is_some() && is_element_access_expression(access_node) {
            access_expression = access_node;
        }
        // PERF: `prop_name` is an interned `Name`, so the property lookup
        // below compares ids; messages print its text.
        let mut prop_name = Name::default();
        let mut has_prop_name = false;
        if !(access_node.is_some() && is_private_identifier(access_node)) {
            prop_name = self.get_property_name_from_index_as_name(index_type, access_node);
            has_prop_name = prop_name != INTERNAL_SYMBOL_NAME_MISSING;
        }
        if has_prop_name {
            if access_flags.intersects(AccessFlags::CONTEXTUAL) {
                let mut t = self.get_type_of_property_of_contextual_type(object_type, &prop_name);
                if t.is_nil() {
                    t = self.any_type;
                }
                return t;
            }
            let prop = self.get_property_of_type_name(object_type, &prop_name);
            if prop.is_some() {
                if access_flags.intersects(AccessFlags::REPORT_DEPRECATED)
                    && access_node.is_some()
                    && !self.sym(prop).declarations.is_empty()
                    && self.is_deprecated_symbol(prop)
                    && self.is_uncalled_function_reference(access_node, prop)
                {
                    let deprecated_node = if access_expression.is_some() {
                        access_expression.argument_expression()
                    } else if is_indexed_access_type_node(access_node) {
                        access_node.index_type()
                    } else {
                        access_node
                    };
                    let declarations = self.sym(prop).declarations.clone();
                    self.add_deprecated_suggestion(deprecated_node, &declarations, &prop_name);
                }
                if access_expression.is_some() {
                    let object_symbol = self.ty(object_type).symbol;
                    let is_self_type_access =
                        self.is_self_type_access(access_expression.expression(), object_symbol);
                    self.mark_property_as_referenced(prop, access_expression, is_self_type_access);
                    if self.is_assignment_to_readonly_entity(
                        access_expression,
                        prop,
                        get_assignment_target_kind(access_expression),
                    ) {
                        let prop_string = self.symbol_to_string(prop);
                        self.error(
                            access_expression.argument_expression(),
                            diag::Cannot_assign_to_0_because_it_is_a_read_only_property,
                            args![prop_string],
                        );
                        return TypeId::NIL;
                    }
                    if access_flags.intersects(AccessFlags::CACHE_SYMBOL) {
                        self.symbol_node_links.get(access_node).resolved_symbol = prop;
                    }
                    if self.is_this_property_access_in_constructor(access_expression, prop) {
                        return self.auto_type;
                    }
                }
                let prop_type = if access_flags.intersects(AccessFlags::WRITING) {
                    self.get_write_type_of_symbol(prop)
                } else {
                    self.get_type_of_symbol(prop)
                };
                if access_expression.is_some()
                    && get_assignment_target_kind(access_expression) != AssignmentKind::DEFINITE
                {
                    return self.get_flow_type_of_reference(access_expression, prop_type);
                } else if access_node.is_some()
                    && is_indexed_access_type_node(access_node)
                    && self.contains_missing_type(prop_type)
                {
                    let undefined_type = self.undefined_type;
                    return self.get_union_type(&[prop_type, undefined_type]);
                } else {
                    return prop_type;
                }
            }
            if self.every_type(object_type, &mut |c: &mut Checker, t: TypeId| {
                c.is_tuple_type(t)
            }) && is_numeric_literal_name(&prop_name)
            {
                let index = Number::from_string(&prop_name);
                if access_node.is_some()
                    && self.every_type(object_type, &mut |c: &mut Checker, t: TypeId| {
                        !c.target_tuple_type(t)
                            .combined_flags
                            .intersects(ElementFlags::VARIABLE)
                    })
                    && !access_flags.intersects(AccessFlags::ALLOW_MISSING)
                {
                    let index_node = get_index_node_for_access_expression(access_node);
                    if self.is_tuple_type(object_type) {
                        if index < Number(0.0) {
                            self.error(
                                index_node,
                                diag::A_tuple_type_cannot_be_indexed_with_a_negative_value,
                                args![],
                            );
                            return self.undefined_type;
                        }
                        let type_string = self.type_to_string_exported(object_type);
                        let arity = self.get_type_reference_arity(object_type);
                        self.error(
                            index_node,
                            diag::Tuple_type_0_of_length_1_has_no_element_at_index_2,
                            args![type_string, arity, prop_name],
                        );
                    } else {
                        let type_string = self.type_to_string_exported(object_type);
                        self.error(
                            index_node,
                            diag::Property_0_does_not_exist_on_type_1,
                            args![prop_name, type_string],
                        );
                    }
                }
                if index >= Number(0.0) {
                    let number_type = self.number_type;
                    let index_info = self.get_index_info_of_type(object_type, number_type);
                    self.error_if_writing_to_readonly_index(
                        index_info,
                        object_type,
                        access_expression,
                    );
                    let undefined_like_type =
                        if access_flags.intersects(AccessFlags::INCLUDE_UNDEFINED) {
                            self.missing_type
                        } else {
                            TypeId::NIL
                        };
                    return self.get_tuple_element_type_out_of_start_count(
                        object_type,
                        index,
                        undefined_like_type,
                    );
                }
            }
        }
        if !self.ty(index_type).flags.intersects(TypeFlags::NULLABLE)
            && self.is_type_assignable_to_kind(
                index_type,
                TypeFlags::STRING_LIKE | TypeFlags::NUMBER_LIKE | TypeFlags::ES_SYMBOL_LIKE,
            )
        {
            if self
                .ty(object_type)
                .flags
                .intersects(TypeFlags::ANY | TypeFlags::NEVER)
            {
                return object_type;
            }
            // If no index signature is applicable, we default to the string index signature. In effect, this means the string
            // index signature applies even when accessing with a symbol-like type.
            let mut index_info = self.get_applicable_index_info(object_type, index_type);
            if index_info.is_nil() {
                let string_type = self.string_type;
                index_info = self.get_index_info_of_type(object_type, string_type);
            }
            if index_info.is_some() {
                let info_key_type = self.index_info(index_info).key_type;
                let info_value_type = self.index_info(index_info).value_type;
                if access_flags.intersects(AccessFlags::NO_INDEX_SIGNATURES)
                    && info_key_type != self.number_type
                {
                    if access_expression.is_some() {
                        if access_flags.intersects(AccessFlags::WRITING) {
                            let type_string = self.type_to_string_exported(original_object_type);
                            self.error(
                                access_expression,
                                diag::Type_0_is_generic_and_can_only_be_indexed_for_reading,
                                args![type_string],
                            );
                        } else {
                            let index_string = self.type_to_string_exported(index_type);
                            let object_string = self.type_to_string_exported(original_object_type);
                            self.error(
                                access_expression,
                                diag::Type_0_cannot_be_used_to_index_type_1,
                                args![index_string, object_string],
                            );
                        }
                    }
                    return TypeId::NIL;
                }
                if access_node.is_some()
                    && info_key_type == self.string_type
                    && !self.is_type_assignable_to_kind(
                        index_type,
                        TypeFlags::STRING | TypeFlags::NUMBER,
                    )
                {
                    let index_node = get_index_node_for_access_expression(access_node);
                    let index_string = self.type_to_string_exported(index_type);
                    self.error(
                        index_node,
                        diag::Type_0_cannot_be_used_as_an_index_type,
                        args![index_string],
                    );
                    if access_flags.intersects(AccessFlags::INCLUDE_UNDEFINED) {
                        let missing_type = self.missing_type;
                        return self.get_union_type(&[info_value_type, missing_type]);
                    } else {
                        return info_value_type;
                    }
                }
                self.error_if_writing_to_readonly_index(index_info, object_type, access_expression);
                // When accessing an enum object with its own type,
                // e.g. E[E.A] for enum E { A }, undefined shouldn't
                // be included in the result type
                if access_flags.intersects(AccessFlags::INCLUDE_UNDEFINED) {
                    let object_symbol = self.ty(object_type).symbol;
                    let index_symbol = self.ty(index_type).symbol;
                    let is_own_enum_access = object_symbol.is_some()
                        && self
                            .sym(object_symbol)
                            .flags
                            .intersects(SymbolFlags::REGULAR_ENUM | SymbolFlags::CONST_ENUM)
                        && (index_symbol.is_some()
                            && self
                                .ty(index_type)
                                .flags
                                .intersects(TypeFlags::ENUM_LITERAL)
                            && self.get_parent_of_symbol(index_symbol) == object_symbol);
                    if !is_own_enum_access {
                        let missing_type = self.missing_type;
                        return self.get_union_type(&[info_value_type, missing_type]);
                    }
                }
                return info_value_type;
            }
            if self.ty(index_type).flags.intersects(TypeFlags::NEVER) {
                return self.never_type;
            }
            if self.is_js_literal_type(object_type) {
                return self.any_type;
            }
            if access_expression.is_some() && !self.is_const_enum_object_type(object_type) {
                if self.is_object_literal_type(object_type) {
                    let index_flags = self.ty(index_type).flags;
                    if self.no_implicit_any
                        && index_flags
                            .intersects(TypeFlags::STRING_LITERAL | TypeFlags::NUMBER_LITERAL)
                    {
                        let value_string = self.ty(index_type).as_literal_type().value_arg();
                        let type_string = self.type_to_string_exported(object_type);
                        self.add_diagnostic(create_diagnostic_for_node(
                            access_expression,
                            diag::Property_0_does_not_exist_on_type_1,
                            args![value_string, type_string],
                        ));
                        return self.undefined_type;
                    } else if index_flags.intersects(TypeFlags::NUMBER | TypeFlags::STRING) {
                        let properties =
                            self.ty(object_type).as_structured_type().properties.clone();
                        let mut types: Vec<TypeId> = Vec::with_capacity(properties.len() + 1);
                        for prop in properties {
                            types.push(self.get_type_of_symbol(prop));
                        }
                        types.push(self.undefined_type);
                        return self.get_union_type(&types);
                    }
                }
                let global_this_symbol = self.global_this_symbol;
                let is_block_scoped_global_this_property =
                    self.ty(object_type).symbol == global_this_symbol && has_prop_name && {
                        let exports = self.sym(global_this_symbol).exports;
                        let export = self.symbols.get_name(exports, &prop_name);
                        export.is_some()
                            && self.sym(export).flags.intersects(SymbolFlags::BLOCK_SCOPED)
                    };
                if is_block_scoped_global_this_property {
                    let type_string = self.type_to_string_exported(object_type);
                    self.error(
                        access_expression,
                        diag::Property_0_does_not_exist_on_type_1,
                        args![prop_name, type_string],
                    );
                } else if self.no_implicit_any
                    && !access_flags.intersects(AccessFlags::SUPPRESS_NO_IMPLICIT_ANY_ERROR)
                {
                    if has_prop_name && self.type_has_static_property(&prop_name, object_type) {
                        let type_name = self.type_to_string_exported(object_type);
                        let suggestion = format!(
                            "{}[{}]",
                            type_name,
                            get_text_of_node(access_expression.argument_expression())
                        );
                        self.error(
                            access_expression,
                            diag::Property_0_does_not_exist_on_type_1_Did_you_mean_to_access_the_static_member_2_instead,
                            args![prop_name /* as string */, type_name, suggestion],
                        );
                    } else if {
                        let number_type = self.number_type;
                        self.get_index_type_of_type(object_type, number_type)
                            .is_some()
                    } {
                        self.error(
                            access_expression.argument_expression(),
                            diag::Element_implicitly_has_an_any_type_because_index_expression_is_not_of_type_number,
                            args![],
                        );
                    } else {
                        let mut suggestion = String::new();
                        if has_prop_name {
                            suggestion = self
                                .get_suggestion_for_nonexistent_property(&prop_name, object_type);
                        }
                        if !suggestion.is_empty() {
                            let type_string = self.type_to_string_exported(object_type);
                            self.error(
                                access_expression.argument_expression(),
                                diag::Property_0_does_not_exist_on_type_1_Did_you_mean_2,
                                args![prop_name /* as string */, type_string, suggestion],
                            );
                        } else {
                            suggestion = self.get_suggestion_for_nonexistent_index_signature(
                                object_type,
                                access_expression,
                                index_type,
                            );
                            if !suggestion.is_empty() {
                                let type_string = self.type_to_string_exported(object_type);
                                self.error(
                                    access_expression,
                                    diag::Element_implicitly_has_an_any_type_because_type_0_has_no_index_signature_Did_you_mean_to_call_1,
                                    args![type_string, suggestion],
                                );
                            } else {
                                let index_flags = self.ty(index_type).flags;
                                let mut diagnostic: Option<Diagnostic> = None;
                                if index_flags.intersects(TypeFlags::ENUM_LITERAL) {
                                    let index_string = self.type_to_string_exported(index_type);
                                    let type_string = self.type_to_string_exported(object_type);
                                    diagnostic = Some(new_diagnostic_for_node(
                                        access_expression,
                                        diag::Property_0_does_not_exist_on_type_1,
                                        args![format!("[{}]", index_string), type_string],
                                    ));
                                } else if index_flags.intersects(TypeFlags::UNIQUE_ES_SYMBOL) {
                                    let index_symbol = self.ty(index_type).symbol;
                                    let symbol_name = self
                                        .get_fully_qualified_name(index_symbol, access_expression);
                                    let type_string = self.type_to_string_exported(object_type);
                                    diagnostic = Some(new_diagnostic_for_node(
                                        access_expression,
                                        diag::Property_0_does_not_exist_on_type_1,
                                        args![format!("[{}]", symbol_name), type_string],
                                    ));
                                } else if index_flags.intersects(TypeFlags::STRING_LITERAL) {
                                    let value_string =
                                        self.ty(index_type).as_literal_type().value_arg();
                                    let type_string = self.type_to_string_exported(object_type);
                                    diagnostic = Some(new_diagnostic_for_node(
                                        access_expression,
                                        diag::Property_0_does_not_exist_on_type_1,
                                        args![value_string, type_string],
                                    ));
                                } else if index_flags.intersects(TypeFlags::NUMBER_LITERAL) {
                                    let value_string =
                                        self.ty(index_type).as_literal_type().value_arg();
                                    let type_string = self.type_to_string_exported(object_type);
                                    diagnostic = Some(new_diagnostic_for_node(
                                        access_expression,
                                        diag::Property_0_does_not_exist_on_type_1,
                                        args![value_string, type_string],
                                    ));
                                } else if index_flags
                                    .intersects(TypeFlags::NUMBER | TypeFlags::STRING)
                                {
                                    let index_string = self.type_to_string_exported(index_type);
                                    let type_string = self.type_to_string_exported(object_type);
                                    diagnostic = Some(new_diagnostic_for_node(
                                        access_expression,
                                        diag::No_index_signature_with_a_parameter_of_type_0_was_found_on_type_1,
                                        args![index_string, type_string],
                                    ));
                                }
                                let full_index_string =
                                    self.type_to_string_exported(full_index_type);
                                let type_string = self.type_to_string_exported(object_type);
                                self.add_diagnostic(new_diagnostic_chain_for_node(
                                    diagnostic,
                                    access_expression,
                                    diag::Element_implicitly_has_an_any_type_because_expression_of_type_0_can_t_be_used_to_index_type_1,
                                    args![full_index_string, type_string],
                                ));
                            }
                        }
                    }
                }
                return TypeId::NIL;
            }
        }
        if access_flags.intersects(AccessFlags::ALLOW_MISSING)
            && self.is_object_literal_type(object_type)
        {
            return self.undefined_type;
        }
        if self.is_js_literal_type(object_type) {
            return self.any_type;
        }
        if access_node.is_some() {
            let index_node = get_index_node_for_access_expression(access_node);
            let index_flags = self.ty(index_type).flags;
            if index_node.kind() != SyntaxKind::BigIntLiteral
                && index_flags.intersects(TypeFlags::STRING_LITERAL | TypeFlags::NUMBER_LITERAL)
            {
                let value_string = self.ty(index_type).as_literal_type().value_arg();
                let type_string = self.type_to_string_exported(object_type);
                self.error(
                    index_node,
                    diag::Property_0_does_not_exist_on_type_1,
                    args![value_string, type_string],
                );
            } else if index_flags.intersects(TypeFlags::STRING | TypeFlags::NUMBER) {
                let type_string = self.type_to_string_exported(object_type);
                let index_string = self.type_to_string_exported(index_type);
                self.error(
                    index_node,
                    diag::Type_0_has_no_matching_index_signature_for_type_1,
                    args![type_string, index_string],
                );
            } else {
                let type_string = if index_node.kind() == SyntaxKind::BigIntLiteral {
                    "bigint".to_string()
                } else {
                    self.type_to_string_exported(index_type)
                };
                self.error(
                    index_node,
                    diag::Type_0_cannot_be_used_as_an_index_type,
                    args![type_string],
                );
            }
        }
        if self.is_type_any(index_type) {
            return index_type;
        }
        TypeId::NIL
    }

    // Go: checker/checker.go:27680 typeHasStaticProperty
    pub fn type_has_static_property(&mut self, prop_name: &str, containing_type: TypeId) -> bool {
        let containing_symbol = self.ty(containing_type).symbol;
        if containing_symbol.is_some() {
            let symbol_type = self.get_type_of_symbol(containing_symbol);
            let prop = self.get_property_of_type(symbol_type, prop_name);
            return prop.is_some()
                && self.sym(prop).value_declaration.is_some()
                && is_static(self.sym(prop).value_declaration);
        }
        false
    }

    // Go: checker/checker.go:27688 getSuggestionForNonexistentProperty
    pub fn get_suggestion_for_nonexistent_property(
        &mut self,
        name: &str,
        containing_type: TypeId,
    ) -> String {
        let properties = self.get_properties_of_type(containing_type);
        let symbol = self.get_spelling_suggestion_for_name(name, &properties, SymbolFlags::VALUE);
        if symbol.is_some() {
            return self.sym(symbol).name.to_string();
        }
        String::new()
    }

    // Go: checker/checker.go:27696 getSuggestionForNonexistentIndexSignature
    pub fn get_suggestion_for_nonexistent_index_signature(
        &mut self,
        object_type: TypeId,
        expr: Node,
        keyed_type: TypeId,
    ) -> String {
        // check if object type has setter or getter
        let has_prop = |c: &mut Checker, name: &str| -> bool {
            let prop = c.get_property_of_object_type(object_type, name);
            if prop.is_some() {
                let prop_type = c.get_type_of_symbol(prop);
                let s = c.get_single_call_signature(prop_type);
                return s.is_some() && c.get_min_argument_count(s) >= 1 && {
                    let param_type = c.get_type_at_position(s, 0);
                    c.is_type_assignable_to(keyed_type, param_type)
                };
            }
            false
        };
        let suggested_method = if is_assignment_target(expr) {
            "set"
        } else {
            "get"
        };
        if !has_prop(self, suggested_method) {
            return String::new();
        }
        let suggestion = try_get_property_access_or_identifier_to_string(expr.expression());
        if suggestion.is_empty() {
            return suggested_method.to_string();
        }
        suggestion + "." + suggested_method
    }

    // Go: checker/checker.go:27717 getSuggestedTypeForNonexistentStringLiteralType
    pub fn get_suggested_type_for_nonexistent_string_literal_type(
        &mut self,
        source: TypeId,
        target: TypeId,
    ) -> TypeId {
        // PORT: Go `getStringLiteralValue(t)` is `t.AsLiteralType().value.(string)`; it is read
        // inline here so the name callback only needs `&self`.
        let string_literal_value = |c: &Checker, t: TypeId| -> String {
            match c.ty(t).as_literal_type().value() {
                Some(LiteralValue::String(s)) => s.clone(),
                _ => panic!("interface conversion: value is not a string"),
            }
        };
        let candidates: Vec<TypeId> = self
            .ty(target)
            .types()
            .iter()
            .copied()
            .filter(|&t| self.ty(t).flags.intersects(TypeFlags::STRING_LITERAL))
            .collect();
        let source_value = string_literal_value(self, source);
        let this: &Checker = self;
        get_spelling_suggestion_with_max_candidate_count(
            &source_value,
            candidates,
            |t: &TypeId| string_literal_value(this, *t),
            |a: &TypeId, b: &TypeId| this.compare_types(*a, *b),
            1000,
        )
    }
}

// Go: checker/checker.go:27722 getIndexNodeForAccessExpression
pub fn get_index_node_for_access_expression(access_node: Node) -> Node {
    match access_node.kind() {
        SyntaxKind::ElementAccessExpression => return access_node.argument_expression(),
        SyntaxKind::IndexedAccessType => return access_node.index_type(),
        SyntaxKind::ComputedPropertyName => return access_node.expression(),
        _ => {}
    }
    access_node
}

impl Checker {
    // Go: checker/checker.go:27734 errorIfWritingToReadonlyIndex
    pub fn error_if_writing_to_readonly_index(
        &mut self,
        index_info: IndexInfoId,
        object_type: TypeId,
        access_expression: Node,
    ) {
        if index_info.is_some()
            && self.index_info(index_info).is_readonly
            && access_expression.is_some()
            && (is_assignment_target(access_expression) || is_delete_target(access_expression))
        {
            let type_string = self.type_to_string_exported(object_type);
            self.error(
                access_expression,
                diag::Index_signature_in_type_0_only_permits_reading,
                args![type_string],
            );
        }
    }

    // Go: checker/checker.go:27740 isSelfTypeAccess
    pub fn is_self_type_access(&mut self, name: Node, parent: SymbolId) -> bool {
        name.kind() == SyntaxKind::ThisKeyword
            || parent.is_some()
                && is_entity_name_expression(name)
                && parent == self.get_resolved_symbol(get_first_identifier(name))
    }

    // Go: checker/checker.go:27744 isAssignmentToReadonlyEntity
    pub fn is_assignment_to_readonly_entity(
        &mut self,
        expr: Node,
        symbol: SymbolId,
        assignment_kind: AssignmentKind,
    ) -> bool {
        if assignment_kind == AssignmentKind::NONE {
            // no assignment means it doesn't matter whether the entity is readonly
            return false;
        }
        if is_access_expression(expr) {
            let node = skip_parentheses(expr.expression());
            if is_identifier(node) {
                let expression_symbol = self.get_resolved_symbol(node);
                // CommonJS module.exports is never readonly
                if self
                    .sym(expression_symbol)
                    .flags
                    .intersects(SymbolFlags::MODULE_EXPORTS)
                {
                    return false;
                }
            }
        }
        if self.is_readonly_symbol(symbol) {
            // Allow assignments to readonly properties within constructors of the same class declaration.
            if self.sym(symbol).flags.intersects(SymbolFlags::PROPERTY)
                && is_access_expression(expr)
                && expr.expression().kind() == SyntaxKind::ThisKeyword
            {
                // Look for if this is the constructor for the class that `symbol` is a property of.
                let ctor = self.get_control_flow_container(expr);
                if ctor.is_nil() || !is_constructor_declaration(ctor) {
                    return true;
                }
                let value_declaration = self.sym(symbol).value_declaration;
                if value_declaration.is_some() {
                    let is_assignment_declaration = is_binary_expression(value_declaration);
                    let is_local_property_declaration = ctor.parent() == value_declaration.parent();
                    let is_local_parameter_property = ctor == value_declaration.parent();
                    let is_local_this_property_assignment = is_assignment_declaration && {
                        let parent = self.sym(symbol).parent;
                        self.sym(parent).value_declaration == ctor.parent()
                    };
                    let is_local_this_property_assignment_constructor_function =
                        is_assignment_declaration && {
                            let parent = self.sym(symbol).parent;
                            self.sym(parent).value_declaration == ctor
                        };
                    let is_writeable_symbol = is_local_property_declaration
                        || is_local_parameter_property
                        || is_local_this_property_assignment
                        || is_local_this_property_assignment_constructor_function;
                    return !is_writeable_symbol;
                }
            }
            return true;
        }
        if is_access_expression(expr) {
            // references through namespace import should be readonly
            let node = skip_parentheses(expr.expression());
            if is_identifier(node) {
                let expression_symbol = self.get_resolved_symbol(node);
                if self
                    .sym(expression_symbol)
                    .flags
                    .intersects(SymbolFlags::ALIAS)
                {
                    let declaration = self.get_declaration_of_alias_symbol(expression_symbol);
                    return declaration.is_some() && is_namespace_import(declaration);
                }
            }
        }
        false
    }

    // Go: checker/checker.go:27793 isThisPropertyAccessInConstructor
    // PERF: chkA. Go compares `GetThisContainer(node)`, which is never nil,
    // with the constructor. With no constructor the answer is false, so the
    // walk (it only reads the tree) is not made.
    pub fn is_this_property_access_in_constructor(&mut self, node: Node, prop: SymbolId) -> bool {
        let mut constructor = Node::NIL;
        let (kind, location) = self.is_constructor_declared_this_property(prop);
        if kind == ThisAssignmentDeclarationKind::THIS_ASSIGNMENT_DECLARATION_CONSTRUCTOR {
            constructor = location;
        } else if is_this_property(node) && self.is_auto_typed_property(prop) {
            constructor = self.get_declaring_constructor(prop);
        }
        constructor.is_some()
            && get_this_container(
                node, true,  /*includeArrowFunctions*/
                false, /*includeClassComputedPropertyName*/
            ) == constructor
    }

    // Go: checker/checker.go:27803 isAutoTypedProperty
    pub fn is_auto_typed_property(&mut self, symbol: SymbolId) -> bool {
        // A property is auto-typed when its declaration has no type annotation or initializer and we're in
        // noImplicitAny mode or a .js file.
        let declaration = self.sym(symbol).value_declaration;
        declaration.is_some()
            && is_property_declaration(declaration)
            && declaration.type_().is_nil()
            && declaration.initializer().is_nil()
            && self.no_implicit_any
    }

    // Go: checker/checker.go:27810 getDeclaringConstructor
    pub fn get_declaring_constructor(&mut self, symbol: SymbolId) -> Node {
        let declarations = self.sym(symbol).declarations.clone();
        for &declaration in declarations.iter() {
            let container = get_this_container(
                declaration,
                false, /*includeArrowFunctions*/
                false, /*includeClassComputedPropertyName*/
            );
            if container.is_some() && is_constructor_declaration(container) {
                return container;
            }
        }
        Node::NIL
    }

    // Go: checker/checker.go:27820 getPropertyNameFromIndex
    pub fn get_property_name_from_index(
        &mut self,
        index_type: TypeId,
        access_node: Node,
    ) -> String {
        if self.is_type_usable_as_property_name(index_type) {
            return self.get_property_name_from_type(index_type);
        }
        if access_node.is_some() && is_property_name(access_node) {
            return get_property_name_for_property_name_node(access_node);
        }
        INTERNAL_SYMBOL_NAME_MISSING.to_string()
    }

    /// `get_property_name_from_index` as an interned `Name`. String literal
    /// and unique symbol names are interned from the type with no `String`.
    pub fn get_property_name_from_index_as_name(
        &mut self,
        index_type: TypeId,
        access_node: Node,
    ) -> Name {
        if self.is_type_usable_as_property_name(index_type) {
            return self.get_property_name_from_type_as_name(index_type);
        }
        if access_node.is_some() && is_property_name(access_node) {
            return Name::from(get_property_name_for_property_name_node(access_node));
        }
        Name::from(INTERNAL_SYMBOL_NAME_MISSING)
    }

    // Go: checker/checker.go:27830 isStringIndexSignatureOnlyTypeWorker
    pub fn is_string_index_signature_only_type_worker(&mut self, t: TypeId) -> bool {
        let flags = self.ty(t).flags;
        // PORT: not in the pinned Go (pingdotgg/ts-rust#20). A type whose
        // members are being resolved and that declares a property itself has
        // properties.
        if flags.intersects(TypeFlags::OBJECT)
            && !self
                .ty(t)
                .object_flags
                .intersects(ObjectFlags::MEMBERS_RESOLVED)
            && self.resolving_type_declares_properties(t)
        {
            return false;
        }
        (flags.intersects(TypeFlags::OBJECT)
            && !self.is_generic_mapped_type(t)
            && self.get_properties_of_type_count(t) == 0
            && self.get_index_infos_of_type(t).len() == 1
            && {
                let string_type = self.string_type;
                self.get_index_info_of_type(t, string_type).is_some()
            })
            || (flags.intersects(TypeFlags::UNION_OR_INTERSECTION)
                && (0..self.ty(t).types().len())
                    .all(|i| self.is_string_index_signature_only_type(self.type_at(t, i))))
    }

    // Go: checker/checker.go:27835 shouldDeferIndexedAccessType
    pub fn should_defer_indexed_access_type(
        &mut self,
        object_type: TypeId,
        index_type: TypeId,
        access_node: Node,
    ) -> bool {
        if self.is_generic_index_type(index_type) {
            return true;
        }
        if access_node.is_some() && !is_indexed_access_type_node(access_node) {
            return self.is_generic_tuple_type(object_type) && {
                let limit = get_total_fixed_element_count(self.target_tuple_type(object_type));
                !self.index_type_less_than(index_type, limit)
            };
        }
        (self.is_generic_object_type(object_type)
            && !(self.is_tuple_type(object_type) && {
                let limit = get_total_fixed_element_count(self.target_tuple_type(object_type));
                self.index_type_less_than(index_type, limit)
            }))
            || self.is_generic_reducible_type(object_type)
    }

    // Go: checker/checker.go:27846 indexTypeLessThan
    pub fn index_type_less_than(&mut self, index_type: TypeId, limit: i32) -> bool {
        self.every_type(index_type, &mut |c: &mut Checker, t: TypeId| {
            if c.ty(t)
                .flags
                .intersects(TypeFlags::STRING_OR_NUMBER_LITERAL)
            {
                let prop_name = c.get_property_name_from_type(t);
                if is_numeric_literal_name(&prop_name) {
                    let index = Number::from_string(&prop_name);
                    return index >= Number(0.0) && index < Number(limit as f64);
                }
            }
            false
        })
    }

    // Go: checker/checker.go:27859 getNoInferType
    pub fn get_no_infer_type(&mut self, t: TypeId) -> TypeId {
        if self.is_no_infer_target_type(t) {
            let unknown_type = self.unknown_type;
            return self.get_or_create_substitution_type(t, unknown_type);
        }
        t
    }

    // Go: checker/checker.go:27866 isNoInferTargetType
    pub fn is_no_infer_target_type(&mut self, t: TypeId) -> bool {
        // This is effectively a more conservative and predictable form of couldContainTypeVariables. We want to
        // preserve NoInfer<T> only for types that could contain type variables, but we don't want to exhaustively
        // examine all object type members.
        let flags = self.ty(t).flags;
        (flags.intersects(TypeFlags::UNION_OR_INTERSECTION) && {
            let types = self.ty(t).as_union_or_intersection_type().types.clone();
            types.into_iter().any(|t| self.is_no_infer_target_type(t))
        }) || (flags.intersects(TypeFlags::SUBSTITUTION) && !self.is_no_infer_type(t) && {
            let base_type = self.ty(t).as_substitution_type().base_type;
            self.is_no_infer_target_type(base_type)
        }) || (flags.intersects(TypeFlags::OBJECT) && !self.is_empty_anonymous_object_type(t))
            || (flags.intersects(TypeFlags::INSTANTIABLE.without(TypeFlags::SUBSTITUTION))
                && !self.is_pattern_literal_type(t))
    }

    // Go: checker/checker.go:27876 getSubstitutionType
    pub fn get_substitution_type(&mut self, base_type: TypeId, constraint: TypeId) -> TypeId {
        if self
            .ty(constraint)
            .flags
            .intersects(TypeFlags::ANY_OR_UNKNOWN)
            || constraint == base_type
            || self.ty(base_type).flags.intersects(TypeFlags::ANY)
        {
            return base_type;
        }
        self.get_or_create_substitution_type(base_type, constraint)
    }

    // Go: checker/checker.go:27883 getOrCreateSubstitutionType
    pub fn get_or_create_substitution_type(
        &mut self,
        base_type: TypeId,
        constraint: TypeId,
    ) -> TypeId {
        let key = SubstitutionTypeKey {
            base_id: base_type,
            constraint_id: constraint,
        };
        if let Some(&cached) = self.substitution_types.get(&key) {
            if cached.is_some() {
                return cached;
            }
        }
        let result = self.new_substitution_type(base_type, constraint);
        self.substitution_types.insert(key, result);
        result
    }

    // Go: checker/checker.go:27893 getBaseConstraintOrType
    pub fn get_base_constraint_or_type(&mut self, t: TypeId) -> TypeId {
        let constraint = self.get_base_constraint_of_type(t);
        if constraint.is_some() {
            return constraint;
        }
        t
    }

    // Go: checker/checker.go:27901 getBaseConstraintOfType
    // PERF: chkfacts1. Inline: a resolved constraint is read here, as Go
    // `getResolvedBaseConstraint` returns it first, with no call (and for an
    // intersection with no `as_constrained_type` dispatch). The rest is
    // `get_base_constraint_of_type_slow`.
    #[inline]
    pub fn get_base_constraint_of_type(&mut self, t: TypeId) -> TypeId {
        let ty = self.ty(t);
        if ty.flags.intersects(BASE_CONSTRAINT_FLAGS) {
            let resolved = match &ty.data {
                TypeData::Intersection(d) => {
                    d.union_or_intersection
                        .structured
                        .constrained
                        .resolved_base_constraint
                }
                data => data
                    .as_constrained_type()
                    .map_or(TypeId::NIL, |c| c.resolved_base_constraint),
            };
            if resolved.is_some() {
                if resolved != self.no_constraint_type && resolved != self.circular_constraint_type
                {
                    return resolved;
                }
                return TypeId::NIL;
            }
        }
        self.get_base_constraint_of_type_slow(t)
    }

    /// `get_base_constraint_of_type` when no constraint is resolved yet.
    #[inline(never)]
    fn get_base_constraint_of_type_slow(&mut self, t: TypeId) -> TypeId {
        if self.ty(t).flags.intersects(BASE_CONSTRAINT_FLAGS) || self.is_generic_tuple_type(t) {
            let constraint = self.get_resolved_base_constraint(t, &[]);
            if constraint != self.no_constraint_type && constraint != self.circular_constraint_type
            {
                return constraint;
            }
            return TypeId::NIL;
        }
        TypeId::NIL
    }

    // Go: checker/checker.go:27912 getResolvedBaseConstraint
    pub fn get_resolved_base_constraint(&mut self, t: TypeId, stack: &[RecursionId]) -> TypeId {
        let Some(constrained) = self.ty(t).data.as_constrained_type() else {
            return t;
        };
        if constrained.resolved_base_constraint.is_some() {
            return constrained.resolved_base_constraint;
        }
        if !self.push_type_resolution(
            TypeSystemEntity::Type(t),
            TypeSystemPropertyName::RESOLVED_BASE_CONSTRAINT,
        ) {
            return self.circular_constraint_type;
        }
        let mut constraint = TypeId::NIL;
        // We always explore at least 10 levels of nested constraints. Thereafter, we continue to explore
        // up to 50 levels of nested constraints provided there are no "deeply nested" types on the stack
        // (i.e. no types for which five instantiations have been recorded on the stack). If we reach 50
        // levels of nesting, we are presumably exploring a repeating pattern with a long cycle that hasn't
        // yet triggered the deeply nested limiter. We have no test cases that actually get to 50 levels of
        // nesting, so it is effectively just a safety stop.
        let identity = self.get_recursion_identity(t);
        if stack.len() < 10 || stack.len() < 50 && !stack.contains(&identity) {
            let simplified = self.get_simplified_type(t, false /*writing*/);
            // PERF: the stack is at most 50 deep and usually short, so it
            // lives inline on the call stack instead of in a new heap `Vec`.
            let mut new_stack: SmallVec<[RecursionId; 16]> =
                SmallVec::with_capacity(stack.len() + 1);
            new_stack.extend_from_slice(stack);
            new_stack.push(identity);
            constraint = self.compute_base_constraint(simplified, &new_stack);
        }
        if !self.pop_type_resolution() {
            if self.ty(t).flags.intersects(TypeFlags::TYPE_PARAMETER) {
                let error_node = self.get_constraint_declaration(t);
                if error_node.is_some() {
                    let type_string = self.type_to_string_exported(t);
                    let current_node = self.current_node;
                    let add_related = current_node.is_some()
                        && !is_node_descendant_of(error_node, current_node)
                        && !is_node_descendant_of(current_node, error_node);
                    // Inlined `c.error`, so the related info goes on the stored
                    // diagnostic (#4825: it can be an equal one added before).
                    // When it is discarded, Go changes a diagnostic that is not
                    // stored, so nothing is done.
                    let diagnostic = new_diagnostic_for_node(
                        error_node,
                        diag::Type_parameter_0_has_a_circular_constraint,
                        args![type_string],
                    );
                    if let Some(diagnostic) = self.add_diagnostic(diagnostic)
                        && add_related
                    {
                        diagnostic.add_related_info(Some(new_diagnostic_for_node(
                            current_node,
                            diag::Circularity_originates_in_type_at_this_location,
                            args![],
                        )));
                    }
                }
            }
            constraint = self.circular_constraint_type;
        }
        if constraint.is_nil() {
            constraint = self.no_constraint_type;
        }
        let constrained = self.ty_mut(t).as_constrained_type_mut();
        if constrained.resolved_base_constraint.is_nil() {
            constrained.resolved_base_constraint = constraint;
        }
        constraint
    }

    // Go: checker/checker.go:27955 computeBaseConstraint
    pub fn compute_base_constraint(&mut self, t: TypeId, stack: &[RecursionId]) -> TypeId {
        let flags = self.ty(t).flags;
        if flags.intersects(TypeFlags::TYPE_PARAMETER) {
            let constraint = self.get_constraint_from_type_parameter(t);
            if self.ty(t).as_type_parameter().is_this_type {
                return constraint;
            }
            return self.get_next_base_constraint(constraint, stack);
        } else if flags.intersects(TypeFlags::UNION_OR_INTERSECTION) {
            let types = self.ty(t).types_list();
            let mut constraints: Vec<TypeId> = Vec::with_capacity(types.len());
            let mut different = false;
            for &s in &types {
                let constraint = self.get_next_base_constraint(s, stack);
                if constraint.is_some() {
                    if constraint != s {
                        different = true;
                    }
                    constraints.push(constraint);
                } else {
                    different = true;
                }
            }
            if !different {
                return t;
            }
            if flags.intersects(TypeFlags::UNION) && constraints.len() == types.len() {
                return self.get_union_type(&constraints);
            } else if flags.intersects(TypeFlags::INTERSECTION) && !constraints.is_empty() {
                return self.get_intersection_type(&constraints);
            }
            return TypeId::NIL;
        } else if flags.intersects(TypeFlags::INDEX) {
            let target = self.ty(t).as_index_type().target;
            if self.is_generic_mapped_type(target) {
                let mapped_type = target;
                if self.get_name_type_from_mapped_type(mapped_type).is_some()
                    && !self.is_mapped_type_with_keyof_constraint_declaration(mapped_type)
                {
                    let index_type =
                        self.get_index_type_for_mapped_type(mapped_type, IndexFlags::NONE);
                    return self.get_next_base_constraint(index_type, stack);
                }
            }
            return self.string_number_symbol_type;
        } else if flags.intersects(TypeFlags::TEMPLATE_LITERAL) {
            let types = self.ty(t).types_list();
            let mut constraints: Vec<TypeId> = Vec::with_capacity(types.len());
            for &s in &types {
                let constraint = self.get_next_base_constraint(s, stack);
                if constraint.is_some() {
                    constraints.push(constraint);
                }
            }
            if constraints.len() == types.len() {
                let texts = self.ty(t).as_template_literal_type().texts.clone();
                return self.get_template_literal_type(&texts, &constraints);
            }
            return self.string_type;
        } else if flags.intersects(TypeFlags::STRING_MAPPING) {
            let target = self.ty(t).target();
            let constraint = self.get_next_base_constraint(target, stack);
            if constraint.is_some() && constraint != target {
                let symbol = self.ty(t).symbol;
                return self.get_string_mapping_type(symbol, constraint);
            }
            return self.string_type;
        } else if flags.intersects(TypeFlags::INDEXED_ACCESS) {
            let object_type = self.ty(t).as_indexed_access_type().object_type;
            let index_type = self.ty(t).as_indexed_access_type().index_type;
            if self.is_mapped_type_generic_indexed_access(t) {
                // For indexed access types of the form { [P in K]: E }[X], where K is non-generic and X is generic,
                // we substitute an instantiation of E where P is replaced with X.
                let substituted = self.substitute_indexed_mapped_type(object_type, index_type);
                return self.get_next_base_constraint(substituted, stack);
            }
            let base_object_type = self.get_next_base_constraint(object_type, stack);
            let base_index_type = self.get_next_base_constraint(index_type, stack);
            if base_object_type.is_nil() || base_index_type.is_nil() {
                return TypeId::NIL;
            }
            let access_flags = self.ty(t).as_indexed_access_type().access_flags;
            let indexed = self.get_indexed_access_type_or_undefined(
                base_object_type,
                base_index_type,
                access_flags,
                Node::NIL,
                None,
            );
            return self.get_next_base_constraint(indexed, stack);
        } else if flags.intersects(TypeFlags::CONDITIONAL) {
            if self.conditional_constraint_depth >= 100 {
                return TypeId::NIL;
            }
            self.conditional_constraint_depth += 1;
            let constraint = self.get_constraint_from_conditional_type(t);
            self.conditional_constraint_depth -= 1;
            return self.get_next_base_constraint(constraint, stack);
        } else if flags.intersects(TypeFlags::SUBSTITUTION) {
            let intersection = self.get_substitution_intersection(t);
            return self.get_next_base_constraint(intersection, stack);
        } else if self.is_generic_tuple_type(t) {
            // We substitute constraints for variadic elements only when the constraints are array types or
            // non-variadic tuple types as we want to avoid further (possibly unbounded) recursion.
            let element_types = self.get_element_types(t);
            let element_infos = self.target_tuple_type(t).element_infos.clone();
            let readonly = self.target_tuple_type(t).readonly;
            let mut new_elements: Vec<TypeId> = Vec::with_capacity(element_types.len());
            for (i, &v) in element_types.iter().enumerate() {
                let mut new_element = v;
                if self.ty(v).flags.intersects(TypeFlags::TYPE_PARAMETER)
                    && element_infos[i].flags.intersects(ElementFlags::VARIADIC)
                {
                    let constraint = self.get_next_base_constraint(v, stack);
                    if constraint.is_some()
                        && constraint != v
                        && self.every_type(constraint, &mut |c: &mut Checker, n: TypeId| {
                            c.is_array_or_tuple_type(n) && !c.is_generic_tuple_type(n)
                        })
                    {
                        new_element = constraint;
                    }
                }
                new_elements.push(new_element);
            }
            return self.create_tuple_type_ex(&new_elements, &element_infos, readonly);
        }
        t
    }

    // Go: checker/checker.go:28058 getNextBaseConstraint
    pub fn get_next_base_constraint(&mut self, t: TypeId, stack: &[RecursionId]) -> TypeId {
        if t.is_nil() {
            return TypeId::NIL;
        }
        let constraint = self.get_resolved_base_constraint(t, stack);
        if constraint == self.no_constraint_type || constraint == self.circular_constraint_type {
            return TypeId::NIL;
        }
        constraint
    }

    // Return true if type might be of the given kind. A union or intersection type might be of a given
    // kind if at least one constituent type is of the given kind.
    // Go: checker/checker.go:28071 maybeTypeOfKind
    // PERF: unionfn1. It only reads flags and lists, so it takes `&self` and
    // loops over the list in place. For a constituent that is not a union or
    // intersection, Go's recursive call is its flags test, so the loop tests
    // the flags here and only a nested union or intersection recurses.
    pub fn maybe_type_of_kind(&self, t: TypeId, kind: TypeFlags) -> bool {
        let ty = self.ty(t);
        if ty.flags.intersects(kind) {
            return true;
        }
        if ty.flags.intersects(TypeFlags::UNION_OR_INTERSECTION) {
            for &m in ty.types() {
                let flags = self.ty(m).flags;
                if flags.intersects(kind)
                    || (flags.intersects(TypeFlags::UNION_OR_INTERSECTION)
                        && self.maybe_type_of_kind(m, kind))
                {
                    return true;
                }
            }
        }
        false
    }

    // Go: checker/checker.go:28085 maybeTypeOfKindConsideringBaseConstraint
    pub fn maybe_type_of_kind_considering_base_constraint(
        &mut self,
        t: TypeId,
        kind: TypeFlags,
    ) -> bool {
        if self.maybe_type_of_kind(t, kind) {
            return true;
        }
        let base_constraint = self.get_base_constraint_or_type(t);
        base_constraint.is_some() && self.maybe_type_of_kind(base_constraint, kind)
    }

    // Go: checker/checker.go:28093 allTypesAssignableToKind
    pub fn all_types_assignable_to_kind(&mut self, source: TypeId, kind: TypeFlags) -> bool {
        self.all_types_assignable_to_kind_ex(source, kind, false)
    }

    // Go: checker/checker.go:28097 allTypesAssignableToKindEx
    pub fn all_types_assignable_to_kind_ex(
        &mut self,
        source: TypeId,
        kind: TypeFlags,
        strict: bool,
    ) -> bool {
        if self.ty(source).flags.intersects(TypeFlags::UNION) {
            return (0..self.ty(source).types().len()).all(|i| {
                let sub_type = self.type_at(source, i);
                self.all_types_assignable_to_kind_ex(sub_type, kind, strict)
            });
        }
        self.is_type_assignable_to_kind_ex(source, kind, strict)
    }

    // Go: checker/checker.go:28106 isTypeAssignableToKind
    pub fn is_type_assignable_to_kind(&mut self, source: TypeId, kind: TypeFlags) -> bool {
        self.is_type_assignable_to_kind_ex(source, kind, false)
    }

    // Go: checker/checker.go:28110 isTypeAssignableToKindEx
    pub fn is_type_assignable_to_kind_ex(
        &mut self,
        source: TypeId,
        kind: TypeFlags,
        strict: bool,
    ) -> bool {
        let source_flags = self.ty(source).flags;
        if source_flags.intersects(kind) {
            return true;
        }
        if strict
            && source_flags.intersects(
                TypeFlags::ANY_OR_UNKNOWN
                    | TypeFlags::VOID
                    | TypeFlags::UNDEFINED
                    | TypeFlags::NULL,
            )
        {
            return false;
        }
        (kind.intersects(TypeFlags::NUMBER_LIKE) && {
            let target = self.number_type;
            self.is_type_assignable_to(source, target)
        }) || (kind.intersects(TypeFlags::BIG_INT_LIKE) && {
            let target = self.bigint_type;
            self.is_type_assignable_to(source, target)
        }) || (kind.intersects(TypeFlags::STRING_LIKE) && {
            let target = self.string_type;
            self.is_type_assignable_to(source, target)
        }) || (kind.intersects(TypeFlags::BOOLEAN_LIKE) && {
            let target = self.boolean_type;
            self.is_type_assignable_to(source, target)
        }) || (kind.intersects(TypeFlags::VOID) && {
            let target = self.void_type;
            self.is_type_assignable_to(source, target)
        }) || (kind.intersects(TypeFlags::NEVER) && {
            let target = self.never_type;
            self.is_type_assignable_to(source, target)
        }) || (kind.intersects(TypeFlags::NULL) && {
            let target = self.null_type;
            self.is_type_assignable_to(source, target)
        }) || (kind.intersects(TypeFlags::UNDEFINED) && {
            let target = self.undefined_type;
            self.is_type_assignable_to(source, target)
        }) || (kind.intersects(TypeFlags::ES_SYMBOL) && {
            let target = self.es_symbol_type;
            self.is_type_assignable_to(source, target)
        }) || (kind.intersects(TypeFlags::NON_PRIMITIVE) && {
            let target = self.non_primitive_type;
            self.is_type_assignable_to(source, target)
        })
    }

    // Go: checker/checker.go:28129 isConstEnumObjectType
    pub fn is_const_enum_object_type(&self, t: TypeId) -> bool {
        let ty = self.ty(t);
        ty.object_flags.intersects(ObjectFlags::ANONYMOUS)
            && ty.symbol.is_some()
            && self.is_const_enum_symbol(ty.symbol)
    }

    // Go: checker/checker.go:28133 isConstEnumSymbol
    pub fn is_const_enum_symbol(&self, symbol: SymbolId) -> bool {
        self.sym(symbol).flags.intersects(SymbolFlags::CONST_ENUM)
    }

    // Go: checker/checker.go:28137 compareProperties
    pub fn compare_properties(
        &mut self,
        source_prop: SymbolId,
        target_prop: SymbolId,
        compare_types: &mut dyn FnMut(&mut Checker, TypeId, TypeId) -> Ternary,
    ) -> Ternary {
        // Two members are considered identical when
        // - they are public properties with identical names, optionality, and types,
        // - they are private or protected properties originating in the same declaration and having identical types
        if source_prop == target_prop {
            return Ternary::TRUE;
        }
        let source_prop_accessibility = self
            .get_declaration_modifier_flags_from_symbol(source_prop)
            & ModifierFlags::NON_PUBLIC_ACCESSIBILITY_MODIFIER;
        let target_prop_accessibility = self
            .get_declaration_modifier_flags_from_symbol(target_prop)
            & ModifierFlags::NON_PUBLIC_ACCESSIBILITY_MODIFIER;
        if source_prop_accessibility != target_prop_accessibility {
            return Ternary::FALSE;
        }
        if source_prop_accessibility != ModifierFlags::NONE {
            if self.get_target_symbol(source_prop) != self.get_target_symbol(target_prop) {
                return Ternary::FALSE;
            }
        } else if (self.sym(source_prop).flags & SymbolFlags::OPTIONAL)
            != (self.sym(target_prop).flags & SymbolFlags::OPTIONAL)
        {
            return Ternary::FALSE;
        }
        if self.is_readonly_symbol(source_prop) != self.is_readonly_symbol(target_prop) {
            return Ternary::FALSE;
        }
        let source_type = self.get_non_missing_type_of_symbol(source_prop);
        let target_type = self.get_non_missing_type_of_symbol(target_prop);
        compare_types(self, source_type, target_type)
    }
}

// Go: checker/checker.go:28164 compareTypesEqual
// PORT: a free function because it only compares handles. Pass it as
// `&mut |_: &mut Checker, s, t| compare_types_equal(s, t)`.
pub fn compare_types_equal(s: TypeId, t: TypeId) -> Ternary {
    if s == t {
        return Ternary::TRUE;
    }
    Ternary::FALSE
}

impl Checker {
    // Go: checker/checker.go:28171 markPropertyAsReferenced
    pub fn mark_property_as_referenced(
        &mut self,
        prop: SymbolId,
        node_for_check_write_only: Node,
        is_self_type_access: bool,
    ) {
        let (prop_flags, value_declaration, check_flags) = {
            let s = self.sym(prop);
            (s.flags, s.value_declaration, s.check_flags)
        };
        if !prop_flags.intersects(SymbolFlags::CLASS_MEMBER) || value_declaration.is_nil() {
            return;
        }
        let has_private_modifier = has_modifier(value_declaration, ModifierFlags::PRIVATE);
        let has_private_identifier =
            value_declaration.name().is_some() && is_private_identifier(value_declaration.name());
        if !has_private_modifier && !has_private_identifier {
            return;
        }
        if node_for_check_write_only.is_some()
            && is_write_only_access(node_for_check_write_only)
            && !prop_flags.intersects(SymbolFlags::SET_ACCESSOR)
        {
            return;
        }
        if is_self_type_access {
            // Find any FunctionLikeDeclaration because those create a new 'this' binding. But this should only matter for methods (or getters/setters).
            let containing_method =
                find_ancestor(node_for_check_write_only, is_function_like_declaration);
            if containing_method.is_some() && containing_method.symbol() == prop {
                return;
            }
        }
        let mut target = prop;
        if check_flags.intersects(CheckFlags::INSTANTIATED) {
            target = self.value_symbol_links.get(prop).target;
        }
        self.symbol_reference_links.get(target).reference_kinds |= SymbolFlags::ALL;
    }

    // Go: checker/checker.go:28197 expandSignatureParametersWithTupleMembers
    // PORT: restType is a *TypeReference in Go; it is passed as its TypeId.
    pub fn expand_signature_parameters_with_tuple_members(
        &mut self,
        signature: SignatureId,
        rest_type: TypeId,
        rest_index: i32,
        rest_symbol: SymbolId,
    ) -> Vec<SymbolId> {
        let element_types = self.get_type_arguments(rest_type);
        let element_infos = self.target_tuple_type(rest_type).element_infos.clone();
        let associated_names =
            self.get_uniq_associated_names_from_tuple_type(rest_type, rest_symbol);
        let mut expanded: Vec<SymbolId> =
            Vec::with_capacity(rest_index as usize + element_types.len());
        expanded.extend_from_slice(&self.sig(signature).parameters[..rest_index as usize]);
        for (i, &t) in element_types.iter().enumerate() {
            let flags = element_infos[i].flags;
            let mut check_flags = CheckFlags::NONE;
            if flags.intersects(ElementFlags::VARIABLE) {
                check_flags = CheckFlags::REST_PARAMETER;
            } else if flags.intersects(ElementFlags::OPTIONAL) {
                check_flags = CheckFlags::OPTIONAL_PARAMETER;
            }
            let symbol = self.new_symbol_ex(
                SymbolFlags::FUNCTION_SCOPED_VARIABLE,
                &associated_names[i],
                check_flags,
            );
            let resolved_type = if flags.intersects(ElementFlags::REST) {
                self.create_array_type(t)
            } else {
                t
            };
            self.value_symbol_links.get(symbol).resolved_type = resolved_type;
            expanded.push(symbol);
        }
        expanded
    }

    // Go: checker/checker.go:28223 getUniqAssociatedNamesFromTupleType
    // PORT: t is a *TypeReference in Go; it is passed as its TypeId.
    pub fn get_uniq_associated_names_from_tuple_type(
        &mut self,
        t: TypeId,
        rest_symbol: SymbolId,
    ) -> Vec<String> {
        let element_infos = self.target_tuple_type(t).element_infos.clone();
        let mut names: Vec<String> = vec![String::new(); element_infos.len()];
        let mut counters: FxHashMap<String, i32> = FxHashMap::default();
        for (i, info) in element_infos.iter().enumerate() {
            names[i] = self.get_tuple_element_label(*info, rest_symbol, i as i32);
            // count duplicates using negative values
            *counters.entry(names[i].clone()).or_insert(0) -= 1;
        }
        for i in 0..names.len() {
            let name = names[i].clone();
            if counters.get(&name).copied().unwrap_or(0) == -1 {
                continue;
            }
            loop {
                let counter = counters.entry(name.clone()).or_insert(0);
                if *counter < 0 {
                    // switch to a positive suffix counter
                    *counter = 0;
                }
                *counter += 1;
                let candidate_name = format!("{}_{}", name, *counter);
                if counters.get(&candidate_name).copied().unwrap_or(0) == 0 {
                    names[i] = candidate_name;
                    break;
                }
            }
        }
        names
    }
}

// Go: checker/checker.go:28252 hasRestParameter
pub fn has_rest_parameter(signature: Node) -> bool {
    let last = signature.parameters().last().unwrap_or(Node::NIL);
    last.is_some() && is_rest_parameter(last)
}

// Go: checker/checker.go:28257 isRestParameter
pub fn is_rest_parameter(param: Node) -> bool {
    param.dot_dot_dot_token().is_some()
}

impl Checker {
    // Go: checker/checker.go:28261 getNameFromIndexInfo
    // PORT: a Checker method because it reads the IndexInfo arena.
    pub fn get_name_from_index_info(&self, info: IndexInfoId) -> String {
        let declaration = self.index_info(info).declaration;
        if declaration.is_some() {
            return declaration_name_to_string(declaration.parameters().to_vec()[0].name());
        }
        "x".to_string()
    }

    // Go: checker/checker.go:28268 isUnknownLikeUnionType
    pub fn is_unknown_like_union_type(&mut self, t: TypeId) -> bool {
        if self.strict_null_checks && self.ty(t).flags.intersects(TypeFlags::UNION) {
            if !self
                .ty(t)
                .object_flags
                .intersects(ObjectFlags::IS_UNKNOWN_LIKE_UNION_COMPUTED)
            {
                self.ty_mut(t).object_flags |= ObjectFlags::IS_UNKNOWN_LIKE_UNION_COMPUTED;
                let count = self.ty(t).types().len();
                if count >= 3
                    && self
                        .ty(self.type_at(t, 0))
                        .flags
                        .intersects(TypeFlags::UNDEFINED)
                    && self
                        .ty(self.type_at(t, 1))
                        .flags
                        .intersects(TypeFlags::NULL)
                    && (0..count).any(|i| self.is_empty_anonymous_object_type(self.type_at(t, i)))
                {
                    self.ty_mut(t).object_flags |= ObjectFlags::IS_UNKNOWN_LIKE_UNION;
                }
            }
            return self
                .ty(t)
                .object_flags
                .intersects(ObjectFlags::IS_UNKNOWN_LIKE_UNION);
        }
        false
    }

    // Return true the given type is a primitive union type where no two literal type constituents are
    // comparable. Specifically, that means (a) the union doesn't contain literals from different enum
    // types, and (b) the union doesn't contain both enum literals and string or number literals.
    // Go: checker/checker.go:28285 isUniformUnionType
    pub fn is_uniform_union_type(&mut self, t: TypeId) -> bool {
        if self
            .ty(t)
            .object_flags
            .intersects(ObjectFlags::PRIMITIVE_UNION)
        {
            if !self
                .ty(t)
                .object_flags
                .intersects(ObjectFlags::IS_UNIFORM_ENUM_COMPUTED)
            {
                let types = self.ty(t).types_list();
                let uniform = if self.compute_is_uniform_union_type(&types) {
                    ObjectFlags::IS_UNIFORM_ENUM
                } else {
                    ObjectFlags::NONE
                };
                self.ty_mut(t).object_flags |= ObjectFlags::IS_UNIFORM_ENUM_COMPUTED | uniform;
            }
            return self
                .ty(t)
                .object_flags
                .intersects(ObjectFlags::IS_UNIFORM_ENUM);
        }
        false
    }

    // Go: checker/checker.go:28295 computeIsUniformUnionType
    pub fn compute_is_uniform_union_type(&mut self, types: &[TypeId]) -> bool {
        let mut enum_symbol = SymbolId::NIL;
        let mut has_string_or_number_literal = false;
        for &t in types {
            let flags = self.ty(t).flags;
            if flags.intersects(TypeFlags::ENUM_LIKE) {
                if has_string_or_number_literal {
                    return false;
                }
                let symbol = self.ty(t).symbol;
                let parent = self.get_parent_of_symbol(symbol);
                if enum_symbol.is_nil() {
                    enum_symbol = parent;
                } else if enum_symbol != parent {
                    return false;
                }
            } else if flags.intersects(TypeFlags::STRING_OR_NUMBER_LITERAL) {
                if enum_symbol.is_some() {
                    return false;
                }
                has_string_or_number_literal = true;
            }
        }
        true
    }

    // Go: checker/checker.go:28319 containsUndefinedType
    pub fn contains_undefined_type(&self, t: TypeId) -> bool {
        let mut t = t;
        if self.ty(t).flags.intersects(TypeFlags::UNION) {
            t = self.ty(t).types()[0];
        }
        self.ty(t).flags.intersects(TypeFlags::UNDEFINED)
    }

    // Go: checker/checker.go:28326 typeHasCallOrConstructSignatures
    pub fn type_has_call_or_construct_signatures(&mut self, t: TypeId) -> bool {
        self.ty(t).flags.intersects(TypeFlags::STRUCTURED_TYPE)
            && !self
                .resolve_structured_type_members(t)
                .signatures()
                .is_empty()
    }

    // Go: checker/checker.go:28330 getNormalizedType
    pub fn get_normalized_type(&mut self, t: TypeId, writing: bool) -> TypeId {
        let mut t = t;
        loop {
            let n: TypeId;
            let flags = self.ty(t).flags;
            if self.is_fresh_literal_type(t) {
                n = self.ty(t).as_literal_type().regular_type;
            } else if self.is_generic_tuple_type(t) {
                n = self.get_normalized_tuple_type(t, writing);
            } else if self.ty(t).object_flags.intersects(ObjectFlags::REFERENCE) {
                if self.ty(t).as_type_reference().node.is_some() {
                    let target = self.ty(t).target();
                    let type_arguments = self.get_type_arguments(t);
                    n = self.create_type_reference(target, &type_arguments);
                } else {
                    let single = self.get_single_base_for_non_augmenting_subtype(t);
                    n = if single.is_nil() { t } else { single };
                }
            } else if flags.intersects(TypeFlags::UNION_OR_INTERSECTION) {
                n = self.get_normalized_union_or_intersection_type(t, writing);
            } else if flags.intersects(TypeFlags::SUBSTITUTION) {
                if writing {
                    n = self.ty(t).as_substitution_type().base_type;
                } else {
                    n = self.get_substitution_intersection(t);
                }
            } else if flags.intersects(TypeFlags::SIMPLIFIABLE) {
                n = self.get_simplified_type(t, writing);
            } else {
                return t;
            }
            if n == t {
                return n;
            }
            t = n;
        }
    }

    // Go: checker/checker.go:28367 getSimplifiedType
    pub fn get_simplified_type(&mut self, t: TypeId, writing: bool) -> TypeId {
        let flags = self.ty(t).flags;
        if flags.intersects(TypeFlags::INDEXED_ACCESS) {
            return self.get_simplified_indexed_access_type(t, writing);
        } else if flags.intersects(TypeFlags::CONDITIONAL) {
            return self.get_simplified_conditional_type(t, writing);
        }
        t
    }

    // Transform an indexed access to a simpler form, if possible. Return the simpler form, or return
    // the type itself if no transformation is possible. The writing flag indicates that the type is
    // the target of an assignment.
    // Go: checker/checker.go:28380 getSimplifiedIndexedAccessType
    pub fn get_simplified_indexed_access_type(&mut self, t: TypeId, writing: bool) -> TypeId {
        let key = CachedTypeKey {
            kind: if writing {
                CachedTypeKind::INDEXED_ACCESS_FOR_WRITING
            } else {
                CachedTypeKind::INDEXED_ACCESS_FOR_READING
            },
            type_id: t,
        };
        if let Some(&cached) = self.cached_types.get(&key) {
            if cached.is_some() {
                return if cached == self.circular_constraint_type {
                    t
                } else {
                    cached
                };
            }
        }
        self.cached_types.insert(key, t);
        let mut result = self.get_simplified_indexed_access_type_worker(t, writing);
        if result != t {
            // If the simplification is a union type that includes t, remove t from the type.
            result = self.remove_type(result, t);
            self.cached_types.insert(key, result);
        }
        result
    }

    // Go: checker/checker.go:28395 getSimplifiedIndexedAccessTypeWorker
    pub fn get_simplified_indexed_access_type_worker(
        &mut self,
        t: TypeId,
        writing: bool,
    ) -> TypeId {
        // We recursively simplify the object type as it may in turn be an indexed access type. For example, with
        // '{ [P in T]: { [Q in U]: number } }[T][U]' we want to first simplify the inner indexed access type.
        let t_object_type = self.ty(t).as_indexed_access_type().object_type;
        let t_index_type = self.ty(t).as_indexed_access_type().index_type;
        let object_type = self.get_simplified_type(t_object_type, writing);
        let index_type = self.get_simplified_type(t_index_type, writing);
        // T[A | B] -> T[A] | T[B] (reading)
        // T[A | B] -> T[A] & T[B] (writing)
        let distributed_over_index =
            self.distribute_object_over_index_type(object_type, index_type, writing);
        if distributed_over_index.is_some() {
            return distributed_over_index;
        }
        // Only do the inner distributions if the index can no longer be instantiated to cause index distribution again
        if !self
            .ty(index_type)
            .flags
            .intersects(TypeFlags::INSTANTIABLE)
        {
            // (T | U)[K] -> T[K] | U[K] (reading)
            // (T | U)[K] -> T[K] & U[K] (writing)
            // (T & U)[K] -> T[K] & U[K]
            let distributed_over_object =
                self.distribute_index_over_object_type(object_type, index_type, writing);
            if distributed_over_object.is_some() {
                return distributed_over_object;
            }
        }
        // So ultimately (reading):
        // ((A & B) | C)[K1 | K2] -> ((A & B) | C)[K1] | ((A & B) | C)[K2] -> (A & B)[K1] | C[K1] | (A & B)[K2] | C[K2] -> (A[K1] & B[K1]) | C[K1] | (A[K2] & B[K2]) | C[K2]
        // A generic tuple type indexed by a number exists only when the index type doesn't select a
        // fixed element. We simplify to either the combined type of all elements (when the index type
        // the actual number type) or to the combined type of all non-fixed elements.
        if self.is_generic_tuple_type(object_type)
            && self.ty(index_type).flags.intersects(TypeFlags::NUMBER_LIKE)
        {
            let index = if self.ty(index_type).flags.intersects(TypeFlags::NUMBER) {
                0
            } else {
                self.target_tuple_type(object_type).fixed_length
            };
            let element_type = self.get_element_type_of_slice_of_tuple_type(
                object_type,
                index,
                0, /*endSkipCount*/
                writing,
                false,
            );
            if element_type.is_some() {
                return element_type;
            }
        }
        // If the object type is a mapped type { [P in K]: E }, where K is generic, or { [P in K as N]: E }, where
        // K is generic and N is assignable to P, instantiate E using a mapper that substitutes the index type for P.
        // For example, for an index access { [P in K]: Box<T[P]> }[X], we construct the type Box<T[X]>.
        if self.is_generic_mapped_type(object_type) {
            if self.get_mapped_type_name_type_kind(object_type) != MappedTypeNameTypeKind::REMAPPING
            {
                let substituted = self.substitute_indexed_mapped_type(object_type, t_index_type);
                return self.map_type(substituted, &mut |c: &mut Checker, t: TypeId| {
                    c.get_simplified_type(t, writing)
                });
            }
        }
        t
    }
}
