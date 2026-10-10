//! Port of typescript-go `internal/checker/checker.go` lines 13025-13928.

use crate::diagnostics::Message;
use crate::gostd::Context;
use crate::prelude::*;

// PORT: Go `checkObjectLiteral` builds its result with a closure
// (`createObjectLiteralType`) that reads these locals by reference. The
// locals live in this struct so the closure can become a method.
struct ObjectLiteralBuildState {
    node: Node,
    contextual_type: TypeId,
    in_destructuring_pattern: bool,
    properties_table: SymbolTable,
    properties_array: Vec<SymbolId>,
    object_flags: ObjectFlags,
    pattern_with_computed_properties: bool,
    has_computed_string_property: bool,
    has_computed_number_property: bool,
    has_computed_symbol_property: bool,
    offset: usize,
}

impl Checker {
    // Go: checker/checker.go:13284 checkInExpression
    pub fn check_in_expression(
        &mut self,
        left: Node,
        right: Node,
        left_type: TypeId,
        right_type: TypeId,
    ) -> TypeId {
        if left_type == self.silent_never_type || right_type == self.silent_never_type {
            return self.silent_never_type;
        }
        if is_private_identifier(left) {
            if self.language_version
                < LANGUAGE_FEATURE_MINIMUM_TARGET.private_names_and_class_static_blocks
                || self.language_version
                    < LANGUAGE_FEATURE_MINIMUM_TARGET.class_and_class_element_decorators
                || !self.compiler_options.get_use_define_for_class_fields()
            {
                self.check_external_emit_helpers(left, ExternalEmitHelpers::CLASS_PRIVATE_FIELD_IN);
            }
            // Unlike in 'checkPrivateIdentifierExpression' we now have access to the RHS type
            // which provides us with the opportunity to emit more detailed errors
            if self.symbol_node_links.get(left).resolved_symbol.is_nil()
                && get_containing_class(left).is_some()
            {
                let right_symbol = self.ty(right_type).symbol;
                let is_unchecked_js = self.is_unchecked_js_suggestion(
                    left,
                    right_symbol,
                    true, /*excludeClasses*/
                );
                self.report_nonexistent_property(left, right_type, is_unchecked_js);
            }
        } else {
            // The type of the left operand must be assignable to string, number, or symbol.
            let source = self.check_non_null_type(left_type, left);
            let target = self.string_number_symbol_type;
            self.check_type_assignable_to(source, target, left, None);
        }
        // The type of the right operand must be assignable to 'object'.
        let source = self.check_non_null_type(right_type, right);
        let target = self.non_primitive_type;
        if self.check_type_assignable_to(source, target, right, None) {
            // The {} type is assignable to the object type, yet {} might represent a primitive type. Here we
            // detect and error on {} that results from narrowing the unknown type, as well as intersections
            // that include {} (we know that the other types in such intersections are assignable to object
            // since we already checked for that).
            if self.has_empty_object_intersection(right_type) {
                let type_string = self.type_to_string_exported(right_type);
                self.error(
                    right,
                    diag::Type_0_may_represent_a_primitive_value_which_is_not_permitted_as_the_right_operand_of_the_in_operator,
                    args![type_string],
                );
            }
        }
        // The result is always of the Boolean primitive type.
        self.boolean_type
    }

    // Go: checker/checker.go:13318 hasEmptyObjectIntersection
    pub fn has_empty_object_intersection(&mut self, t: TypeId) -> bool {
        self.some_type(t, &mut |c: &mut Checker, t: TypeId| {
            if t == c.unknown_empty_object_type {
                return true;
            }
            if c.ty(t).flags.intersects(TypeFlags::INTERSECTION) {
                let constraint = c.get_base_constraint_or_type(t);
                return c.is_empty_anonymous_object_type(constraint);
            }
            false
        })
    }

    // Go: checker/checker.go:13324 getExactOptionalUnassignableProperties
    pub fn get_exact_optional_unassignable_properties(
        &mut self,
        source: TypeId,
        target: TypeId,
    ) -> Vec<SymbolId> {
        if self.is_tuple_type(source) && self.is_tuple_type(target) {
            return Vec::new();
        }
        let props = self.get_properties_of_type(target);
        let mut result = Vec::new();
        for target_prop in props {
            let name = self.sym(target_prop).name.clone();
            let source_type = self.get_type_of_property_of_type(source, &name);
            let target_type = self.get_type_of_symbol(target_prop);
            if self.is_exact_optional_property_mismatch(source_type, target_type) {
                result.push(target_prop);
            }
        }
        result
    }

    // Go: checker/checker.go:13333 isExactOptionalPropertyMismatch
    pub fn is_exact_optional_property_mismatch(&mut self, source: TypeId, target: TypeId) -> bool {
        if source.is_nil() || target.is_nil() {
            return false;
        }
        if !self.maybe_type_of_kind(source, TypeFlags::UNDEFINED) {
            return false;
        }
        let contains_missing_type = self.contains_missing_type.clone();
        contains_missing_type(self, target)
    }

    // Go: checker/checker.go:13337 checkReferenceExpression
    pub fn check_reference_expression(
        &mut self,
        expr: Node,
        invalid_reference_message: &'static Message,
        invalid_optional_chain_message: &'static Message,
    ) -> bool {
        // References are combinations of identifiers, parentheses, and property accesses.
        let node = skip_outer_expressions(
            expr,
            OuterExpressionKinds::OEK_ASSERTIONS | OuterExpressionKinds::OEK_PARENTHESES,
        );
        if node.kind() != SyntaxKind::Identifier && !is_access_expression(node) {
            self.error(expr, invalid_reference_message, args![]);
            return false;
        }
        if node.flags().intersects(NodeFlags::OPTIONAL_CHAIN) {
            self.error(expr, invalid_optional_chain_message, args![]);
            return false;
        }
        true
    }

    // Go: checker/checker.go:13351 checkObjectLiteral
    pub fn check_object_literal(&mut self, node: Node, check_mode: CheckMode) -> TypeId {
        // Expando object literals have empty properties but filled exports
        let node_symbol = node.symbol();
        if node.properties().is_empty()
            && node_symbol.is_some()
            && self.symbols.len(self.sym(node_symbol).exports) != 0
        {
            let exports = self.sym(node_symbol).exports;
            let result = self.new_anonymous_type(node_symbol, exports, &[], &[], &[]);
            if is_in_js_file(node) && !is_in_json_file(node) {
                self.ty_mut(result).object_flags |= ObjectFlags::JS_LITERAL;
            }
            // An expando object literal has no property children (len == 0), so there
            // is nothing to check here.
            return result;
        }
        self.check_node_deferred(node);
        let in_destructuring_pattern = is_assignment_target(node);
        // Grammar checking
        self.check_grammar_object_literal_expression(node, in_destructuring_pattern);
        let mut all_properties_table = SymbolTable::NIL;
        if self.strict_null_checks {
            all_properties_table = self.symbols.new_table();
        }
        let properties_table = self.symbols.new_table();
        let mut spread = self.empty_object_type;
        self.push_cached_contextual_type(node);
        let contextual_type = self.get_apparent_type_of_contextual_type(node, ContextFlags::NONE);
        let mut contextual_type_has_pattern = false;
        if contextual_type.is_some() {
            let pattern = self
                .pattern_for_type
                .get(&contextual_type)
                .copied()
                .unwrap_or_default();
            if pattern.is_some()
                && (is_object_binding_pattern(pattern) || is_object_literal_expression(pattern))
            {
                contextual_type_has_pattern = true;
            }
        }
        let in_const_context = self.is_const_context(node);
        let check_flags = if in_const_context {
            CheckFlags::READONLY
        } else {
            CheckFlags::NONE
        };
        let mut st = ObjectLiteralBuildState {
            node,
            contextual_type,
            in_destructuring_pattern,
            properties_table,
            properties_array: Vec::new(),
            object_flags: ObjectFlags::FRESH_LITERAL,
            pattern_with_computed_properties: false,
            has_computed_string_property: false,
            has_computed_number_property: false,
            has_computed_symbol_property: false,
            offset: 0,
        };
        // Spreads may cause an early bail; ensure computed names are always checked (this is cached)
        // As otherwise they may not be checked until exports for the type at this position are retrieved,
        // which may never occur.
        let properties = node.properties();
        for elem in properties {
            if elem.name().is_some() && is_computed_property_name(elem.name()) {
                self.check_computed_property_name(elem.name());
            }
        }
        for member_decl in properties {
            let mut member = self.get_symbol_of_declaration(member_decl);
            let mut computed_name_type = TypeId::NIL;
            if member_decl.name().is_some()
                && member_decl.name().kind() == SyntaxKind::ComputedPropertyName
            {
                computed_name_type = self.check_computed_property_name(member_decl.name());
            }
            if is_property_assignment(member_decl)
                || is_shorthand_property_assignment(member_decl)
                || is_object_literal_method(member_decl)
            {
                let t = match member_decl.kind() {
                    SyntaxKind::PropertyAssignment => {
                        self.check_property_assignment(member_decl, check_mode)
                    }
                    SyntaxKind::ShorthandPropertyAssignment => self
                        .check_shorthand_property_assignment(
                            member_decl,
                            in_destructuring_pattern,
                            check_mode,
                        ),
                    _ => self.check_object_literal_method(member_decl, check_mode),
                };
                st.object_flags |= self.ty(t).object_flags & ObjectFlags::PROPAGATING_FLAGS;
                let mut name_type = TypeId::NIL;
                if computed_name_type.is_some()
                    && self.is_type_usable_as_property_name(computed_name_type)
                {
                    name_type = computed_name_type;
                }
                let member_flags = self.sym(member).flags;
                let prop = if name_type.is_some() {
                    let name = self.get_property_name_from_type(name_type);
                    self.new_symbol_ex(
                        SymbolFlags::PROPERTY | member_flags,
                        &name,
                        check_flags | CheckFlags::LATE,
                    )
                } else {
                    let name = self.sym(member).name.clone();
                    self.new_symbol_ex(SymbolFlags::PROPERTY | member_flags, &name, check_flags)
                };
                // Go `links := c.valueSymbolLinks.Get(prop)` gives the id here.
                let links = self.value_symbol_links.get_by_id(&self.symbols, prop);
                if name_type.is_some() {
                    links.name_type = name_type;
                }
                if in_destructuring_pattern && self.has_default_value(member_decl) {
                    // If object literal is an assignment pattern and if the assignment pattern specifies a default value
                    // for the property, make the property optional.
                    self.sym_mut(prop).flags |= SymbolFlags::OPTIONAL;
                } else if contextual_type_has_pattern
                    && !self
                        .ty(contextual_type)
                        .object_flags
                        .intersects(ObjectFlags::OBJECT_LITERAL_PATTERN_WITH_COMPUTED_PROPERTIES)
                {
                    // If object literal is contextually typed by the implied type of a binding pattern, and if the
                    // binding pattern specifies a default value for the property, make the property optional.
                    let member_name = self.sym(member).name.clone();
                    let implied_prop = self.get_property_of_type(contextual_type, &member_name);
                    if implied_prop.is_some() {
                        let implied_flags = self.sym(implied_prop).flags;
                        self.sym_mut(prop).flags |= implied_flags & SymbolFlags::OPTIONAL;
                    } else {
                        let string_type = self.string_type;
                        if self
                            .get_index_info_of_type(contextual_type, string_type)
                            .is_nil()
                        {
                            let member_string = self.symbol_to_string(member);
                            let contextual_string = self.type_to_string_exported(contextual_type);
                            self.error(
                                member_decl.name(),
                                diag::Object_literal_may_only_specify_known_properties_and_0_does_not_exist_in_type_1,
                                args![member_string, contextual_string],
                            );
                        }
                    }
                }
                let (declarations, parent, value_declaration) = {
                    let m = self.sym(member);
                    (m.declarations.clone(), m.parent, m.value_declaration)
                };
                {
                    let p = self.sym_mut(prop);
                    p.declarations = declarations;
                    p.parent = parent;
                    p.value_declaration = value_declaration;
                }
                {
                    let links = self.value_symbol_links.get_by_id(&self.symbols, prop);
                    links.resolved_type = t;
                    links.target = member;
                }
                member = prop;
                if all_properties_table.is_some() {
                    let prop_name = self.sym(prop).name.clone();
                    self.symbols.set(all_properties_table, prop_name, prop);
                }
                if contextual_type.is_some()
                    && check_mode.intersects(CheckMode::INFERENTIAL)
                    && !check_mode.intersects(CheckMode::SKIP_CONTEXT_SENSITIVE)
                    && (is_property_assignment(member_decl) || is_method_declaration(member_decl))
                    && self.is_context_sensitive(member_decl)
                {
                    let inference_context = self.get_inference_context(node);
                    // In CheckMode.Inferential we should always have an inference context
                    let mut inference_node = member_decl;
                    if is_property_assignment(member_decl) {
                        inference_node = member_decl.initializer();
                    }
                    self.add_intra_expression_inference_site(inference_context, inference_node, t);
                }
            } else if member_decl.kind() == SyntaxKind::SpreadAssignment {
                if !st.properties_array.is_empty() {
                    let object_literal_type = self.check_object_literal_create_type(&st);
                    spread = self.get_spread_type(
                        spread,
                        object_literal_type,
                        node_symbol,
                        st.object_flags,
                        in_const_context,
                    );
                    st.properties_array = Vec::new();
                    st.properties_table = self.symbols.new_table();
                    st.has_computed_string_property = false;
                    st.has_computed_number_property = false;
                    st.has_computed_symbol_property = false;
                }
                let expr_type = self.check_expression_ex(
                    member_decl.expression(),
                    check_mode & CheckMode::INFERENTIAL,
                );
                let t = self.get_reduced_type(expr_type);
                if self.is_valid_spread_type(t) {
                    let merged_type =
                        self.try_merge_union_of_object_type_and_empty_object(t, in_const_context);
                    if all_properties_table.is_some() {
                        self.check_spread_prop_overrides(
                            merged_type,
                            all_properties_table,
                            member_decl,
                        );
                    }
                    st.offset = st.properties_array.len();
                    if self.is_error_type(spread) {
                        continue;
                    }
                    spread = self.get_spread_type(
                        spread,
                        merged_type,
                        node_symbol,
                        st.object_flags,
                        in_const_context,
                    );
                } else {
                    self.error(
                        member_decl,
                        diag::Spread_types_may_only_be_created_from_object_types,
                        args![],
                    );
                    spread = self.error_type;
                }
                continue;
            } else {
                // TypeScript 1.0 spec (April 2014)
                // A get accessor declaration is processed in the same manner as
                // an ordinary function declaration(section 6.1) with no parameters.
                // A set accessor declaration is processed in the same manner
                // as an ordinary function declaration with a single parameter and a Void return type.
                debug_assert!(
                    member_decl.kind() == SyntaxKind::GetAccessor
                        || member_decl.kind() == SyntaxKind::SetAccessor
                );
                self.check_node_deferred(member_decl);
            }
            if computed_name_type.is_some()
                && !self
                    .ty(computed_name_type)
                    .flags
                    .intersects(TypeFlags::STRING_OR_NUMBER_LITERAL_OR_UNIQUE)
            {
                let string_number_symbol_type = self.string_number_symbol_type;
                if self.is_type_assignable_to(computed_name_type, string_number_symbol_type) {
                    let number_type = self.number_type;
                    let es_symbol_type = self.es_symbol_type;
                    if self.is_type_assignable_to(computed_name_type, number_type) {
                        st.has_computed_number_property = true;
                    } else if self.is_type_assignable_to(computed_name_type, es_symbol_type) {
                        st.has_computed_symbol_property = true;
                    } else {
                        st.has_computed_string_property = true;
                    }
                    if in_destructuring_pattern {
                        st.pattern_with_computed_properties = true;
                    }
                }
            } else {
                let member_name = self.sym(member).name.clone();
                self.symbols.set(st.properties_table, member_name, member);
            }
            st.properties_array.push(member);
        }
        self.pop_contextual_type();
        if self.is_error_type(spread) {
            return self.error_type;
        }
        if spread != self.empty_object_type {
            if !st.properties_array.is_empty() {
                let object_literal_type = self.check_object_literal_create_type(&st);
                spread = self.get_spread_type(
                    spread,
                    object_literal_type,
                    node_symbol,
                    st.object_flags,
                    in_const_context,
                );
                st.properties_array = Vec::new();
                st.properties_table = self.symbols.new_table();
                st.has_computed_string_property = false;
                st.has_computed_number_property = false;
            }
            // remap the raw emptyObjectType fed in at the top into a fresh empty object literal type, unique to this use site
            let st_ref = &st;
            return self.map_type(spread, &mut |c: &mut Checker, t: TypeId| {
                if t == c.empty_object_type {
                    return c.check_object_literal_create_type(st_ref);
                }
                t
            });
        }
        self.check_object_literal_create_type(&st)
    }

    // Go: checker/checker.go:13400 checkObjectLiteral.createObjectLiteralType
    // PORT: the Go closure `createObjectLiteralType` inside `checkObjectLiteral`.
    // Go slices `propertiesArray[offset:]`, which panics when `offset` is past
    // the end; the Rust slice panics in the same case.
    fn check_object_literal_create_type(&mut self, st: &ObjectLiteralBuildState) -> TypeId {
        let node = st.node;
        let mut index_infos: Vec<IndexInfoId> = Vec::new();
        let is_readonly = self.is_const_context(node);
        if st.has_computed_string_property {
            let string_type = self.string_type;
            let info = self.get_object_literal_index_info(
                is_readonly,
                &st.properties_array[st.offset..],
                string_type,
            );
            index_infos.push(info);
        }
        if st.has_computed_number_property {
            let number_type = self.number_type;
            let info = self.get_object_literal_index_info(
                is_readonly,
                &st.properties_array[st.offset..],
                number_type,
            );
            index_infos.push(info);
        }
        if st.has_computed_symbol_property {
            let es_symbol_type = self.es_symbol_type;
            let info = self.get_object_literal_index_info(
                is_readonly,
                &st.properties_array[st.offset..],
                es_symbol_type,
            );
            index_infos.push(info);
        }
        let result =
            self.new_anonymous_type(node.symbol(), st.properties_table, &[], &[], &index_infos);
        self.ty_mut(result).object_flags |= st.object_flags
            | ObjectFlags::OBJECT_LITERAL
            | ObjectFlags::CONTAINS_OBJECT_OR_ARRAY_LITERAL;
        if st.contextual_type.is_nil() && is_in_js_file(node) && !is_in_json_file(node) {
            self.ty_mut(result).object_flags |= ObjectFlags::JS_LITERAL;
        }
        if st.pattern_with_computed_properties {
            self.ty_mut(result).object_flags |=
                ObjectFlags::OBJECT_LITERAL_PATTERN_WITH_COMPUTED_PROPERTIES;
        }
        if st.in_destructuring_pattern {
            self.pattern_for_type.insert(result, node);
        }
        result
    }

    // Go: checker/checker.go:13563 checkContextualDeprecations
    // Runs as a deferred check of an object literal or JSX attributes node, so
    // each property is checked once and not on every inference pass.
    pub fn check_contextual_deprecations(&mut self, node: Node) {
        let contextual_type = self.get_apparent_type_of_contextual_type(node, ContextFlags::NONE);
        for property in node.properties().to_vec() {
            if self.is_canceled() {
                return;
            }
            let name = property.name();
            if name.is_some() && !is_computed_property_name(name) {
                self.check_deprecated_property(name, contextual_type);
            }
        }
    }

    // Go: checker/checker.go:13575 checkDeprecatedProperty
    pub fn check_deprecated_property(&mut self, name: Node, contextual_type: TypeId) {
        if contextual_type.is_nil() {
            return;
        }
        let prop = self.get_property_of_type(contextual_type, name.text());
        if prop.is_nil() || self.sym(prop).declarations.is_empty() {
            return;
        }
        if self.is_deprecated_symbol(prop) {
            let prop_declarations = self.sym(prop).declarations.clone();
            self.add_deprecated_suggestion(name, &prop_declarations, name.text());
        }
    }

    // Go: checker/checker.go:13588 checkSpreadPropOverrides
    pub fn check_spread_prop_overrides(&mut self, t: TypeId, props: SymbolTable, spread: Node) {
        for right in self.get_properties_of_type(t) {
            let (right_flags, right_check_flags, right_name) = {
                let r = self.sym(right);
                (r.flags, r.check_flags, r.name.clone())
            };
            if !right_flags.intersects(SymbolFlags::OPTIONAL)
                && !right_check_flags.intersects(CheckFlags::PARTIAL)
            {
                let left = self.symbols.get(props, &right_name);
                if left.is_some() {
                    let (left_value_declaration, left_name) = {
                        let l = self.sym(left);
                        (l.value_declaration, l.name.clone())
                    };
                    // PORT: Go calls `c.error` (which adds the diagnostic) and then
                    // mutates the returned `*ast.Diagnostic` with `AddRelatedInfo`.
                    // Diagnostics are owned values here, so the related info is
                    // attached before the diagnostic is added. The result is the same.
                    let mut diagnostic = new_diagnostic_for_node(
                        left_value_declaration,
                        diag::X_0_is_specified_more_than_once_so_this_usage_will_be_overwritten,
                        args![left_name],
                    );
                    diagnostic.add_related_info(Some(new_diagnostic_for_node(
                        spread,
                        diag::This_spread_always_overwrites_this_property,
                        args![],
                    )));
                    self.add_diagnostic(diagnostic);
                }
            }
        }
    }

    /**
     * Since the source of spread types are object literals, which are not binary,
     * this function should be called in a left folding style, with left = previous result of getSpreadType
     * and right = the new element to be spread.
     */
    // Go: checker/checker.go:13604 getSpreadType
    pub fn get_spread_type(
        &mut self,
        left: TypeId,
        right: TypeId,
        symbol: SymbolId,
        object_flags: ObjectFlags,
        readonly: bool,
    ) -> TypeId {
        let mut left = left;
        let mut right = right;
        if self.ty(left).flags.intersects(TypeFlags::ANY)
            || self.ty(right).flags.intersects(TypeFlags::ANY)
        {
            return self.any_type;
        }
        if self.ty(left).flags.intersects(TypeFlags::UNKNOWN)
            || self.ty(right).flags.intersects(TypeFlags::UNKNOWN)
        {
            return self.unknown_type;
        }
        if self.ty(left).flags.intersects(TypeFlags::NEVER) {
            return right;
        }
        if self.ty(right).flags.intersects(TypeFlags::NEVER) {
            return left;
        }
        left = self.try_merge_union_of_object_type_and_empty_object(left, readonly);
        if self.ty(left).flags.intersects(TypeFlags::UNION) {
            if self.check_cross_product_union(&[left, right]) {
                return self.map_type(left, &mut |c: &mut Checker, t: TypeId| {
                    c.get_spread_type(t, right, symbol, object_flags, readonly)
                });
            }
            return self.error_type;
        }
        right = self.try_merge_union_of_object_type_and_empty_object(right, readonly);
        if self.ty(right).flags.intersects(TypeFlags::UNION) {
            if self.check_cross_product_union(&[left, right]) {
                return self.map_type(right, &mut |c: &mut Checker, t: TypeId| {
                    c.get_spread_type(left, t, symbol, object_flags, readonly)
                });
            }
            return self.error_type;
        }
        if self.ty(right).flags.intersects(
            TypeFlags::BOOLEAN_LIKE
                | TypeFlags::NUMBER_LIKE
                | TypeFlags::BIG_INT_LIKE
                | TypeFlags::STRING_LIKE
                | TypeFlags::ENUM_LIKE
                | TypeFlags::NON_PRIMITIVE
                | TypeFlags::INDEX,
        ) {
            return left;
        }
        if self.is_generic_object_type(left) || self.is_generic_object_type(right) {
            if self.is_empty_object_type(left) {
                return right;
            }
            // When the left type is an intersection, we may need to merge the last constituent of the
            // intersection with the right type. For example when the left type is 'T & { a: string }'
            // and the right type is '{ b: string }' we produce 'T & { a: string, b: string }'.
            if self.ty(left).flags.intersects(TypeFlags::INTERSECTION) {
                let types = self.ty(left).types_list();
                let last_left = types[types.len() - 1];
                if self.is_non_generic_object_type(last_left)
                    && self.is_non_generic_object_type(right)
                {
                    let mut new_types = types.to_vec();
                    let last = new_types.len() - 1;
                    new_types[last] =
                        self.get_spread_type(last_left, right, symbol, object_flags, readonly);
                    return self.get_intersection_type(&new_types);
                }
            }
            return self.get_intersection_type(&[left, right]);
        }
        let members = self.symbols.new_table();
        let mut skipped_private_members: FxHashSet<String> = FxHashSet::default();
        let index_infos = if left == self.empty_object_type {
            self.get_index_infos_of_type(right).to_vec()
        } else {
            self.get_union_index_infos(&[left, right])
        };
        for right_prop in self.get_properties_of_type(right) {
            if self
                .get_declaration_modifier_flags_from_symbol(right_prop)
                .intersects(ModifierFlags::PRIVATE | ModifierFlags::PROTECTED)
            {
                let name = self.sym(right_prop).name.to_string();
                skipped_private_members.insert(name);
            } else if self.is_spreadable_property(right_prop) {
                let name = self.sym(right_prop).name.clone();
                let spread_symbol = self.get_spread_symbol(right_prop, readonly);
                self.symbols.set(members, name, spread_symbol);
            }
        }

        for left_prop in self.get_properties_of_type(left) {
            let left_name = self.sym(left_prop).name.to_string();
            if skipped_private_members.contains(&left_name)
                || !self.is_spreadable_property(left_prop)
            {
                continue;
            }
            if self.symbols.get(members, &left_name).is_some() {
                let right_prop = self.symbols.get(members, &left_name);
                let right_type = self.get_type_of_symbol(right_prop);
                if self.sym(right_prop).flags.intersects(SymbolFlags::OPTIONAL) {
                    let mut declarations: Vec<Node> = self.sym(left_prop).declarations.to_vec();
                    declarations.extend(self.sym(right_prop).declarations.iter().copied());
                    let flags =
                        SymbolFlags::PROPERTY | (self.sym(left_prop).flags & SymbolFlags::OPTIONAL);
                    let result = self.new_symbol(flags, &left_name);
                    // Go `links := c.valueSymbolLinks.Get(result)` gives the id here.
                    self.value_symbol_links.get_by_id(&self.symbols, result);
                    // Optimization: avoid calculating the union type if spreading into the exact same type.
                    // This is common, e.g. spreading one options bag into another where the bags have the
                    // same type, or have properties which overlap. If the unions are large, it may turn out
                    // to be expensive to perform subtype reduction.
                    let left_type = self.get_type_of_symbol(left_prop);
                    let left_type_without_undefined =
                        self.remove_missing_or_undefined_type(left_type);
                    let right_type_without_undefined =
                        self.remove_missing_or_undefined_type(right_type);
                    if left_type_without_undefined == right_type_without_undefined {
                        self.value_symbol_links
                            .get_by_id(&self.symbols, result)
                            .resolved_type = left_type;
                    } else {
                        let union = self.get_union_type_ex(
                            &[left_type, right_type_without_undefined],
                            UnionReduction::SUBTYPE,
                            None,
                            TypeId::NIL,
                        );
                        self.value_symbol_links
                            .get_by_id(&self.symbols, result)
                            .resolved_type = union;
                    }
                    self.spread_links.get(result).left_spread = left_prop;
                    self.spread_links.get(result).right_spread = right_prop;
                    self.sym_mut(result).declarations = declarations.into();
                    let name_type = self
                        .value_symbol_links
                        .get_by_id(&self.symbols, left_prop)
                        .name_type;
                    self.value_symbol_links
                        .get_by_id(&self.symbols, result)
                        .name_type = name_type;
                    self.symbols.set(members, left_name, result);
                }
            } else {
                let spread_symbol = self.get_spread_symbol(left_prop, readonly);
                self.symbols.set(members, left_name, spread_symbol);
            }
        }
        let mut spread_index_infos: Vec<IndexInfoId> = Vec::with_capacity(index_infos.len());
        for info in index_infos {
            let info = self.get_index_info_with_readonly(info, readonly);
            spread_index_infos.push(info);
        }
        let spread = self.new_anonymous_type(symbol, members, &[], &[], &spread_index_infos);
        self.ty_mut(spread).object_flags |= ObjectFlags::OBJECT_LITERAL
            | ObjectFlags::CONTAINS_OBJECT_OR_ARRAY_LITERAL
            | ObjectFlags::CONTAINS_SPREAD
            | object_flags;
        spread
    }

    // Go: checker/checker.go:13714 getIndexInfoWithReadonly
    pub fn get_index_info_with_readonly(
        &mut self,
        info: IndexInfoId,
        readonly: bool,
    ) -> IndexInfoId {
        if self.index_info(info).is_readonly != readonly {
            let (key_type, value_type, declaration, components) = {
                let i = self.index_info(info);
                (
                    i.key_type,
                    i.value_type,
                    i.declaration,
                    i.components.clone(),
                )
            };
            return self.new_index_info(key_type, value_type, readonly, declaration, &components);
        }
        info
    }

    // Go: checker/checker.go:13721 isValidSpreadType
    pub fn is_valid_spread_type(&mut self, t: TypeId) -> bool {
        let mapped = self.map_type(t, &mut |c: &mut Checker, t: TypeId| {
            c.get_base_constraint_or_type(t)
        });
        let s = self.remove_definitely_falsy_types(mapped);
        let s_flags = self.ty(s).flags;
        if s_flags.intersects(
            TypeFlags::ANY
                | TypeFlags::NON_PRIMITIVE
                | TypeFlags::OBJECT
                | TypeFlags::INSTANTIABLE_NON_PRIMITIVE,
        ) {
            return true;
        }
        if s_flags.intersects(TypeFlags::UNION_OR_INTERSECTION) {
            let types = self.ty(s).types_list();
            for t in types {
                if !self.is_valid_spread_type(t) {
                    return false;
                }
            }
            return true;
        }
        false
    }

    // Go: checker/checker.go:13727 getUnionIndexInfos
    pub fn get_union_index_infos(&mut self, types: &[TypeId]) -> Vec<IndexInfoId> {
        let source_infos = self.get_index_infos_of_type(types[0]);
        let mut result: Vec<IndexInfoId> = Vec::new();
        for info in source_infos {
            let index_type = self.index_info(info).key_type;
            let mut every = true;
            for &t in types {
                if self.get_index_info_of_type(t, index_type).is_nil() {
                    every = false;
                    break;
                }
            }
            if every {
                let mut value_types: Vec<TypeId> = Vec::with_capacity(types.len());
                for &t in types {
                    let value_type = self.get_index_type_of_type(t, index_type);
                    value_types.push(value_type);
                }
                let value_type = self.get_union_type(&value_types);
                let mut is_readonly = false;
                for &t in types {
                    let i = self.get_index_info_of_type(t, index_type);
                    if self.index_info(i).is_readonly {
                        is_readonly = true;
                        break;
                    }
                }
                let new_info =
                    self.new_index_info(index_type, value_type, is_readonly, Node::NIL, &[]);
                result.push(new_info);
            }
        }
        result
    }

    // Go: checker/checker.go:13743 isNonGenericObjectType
    pub fn is_non_generic_object_type(&mut self, t: TypeId) -> bool {
        self.ty(t).flags.intersects(TypeFlags::OBJECT) && !self.is_generic_mapped_type(t)
    }

    // Go: checker/checker.go:13747 tryMergeUnionOfObjectTypeAndEmptyObject
    pub fn try_merge_union_of_object_type_and_empty_object(
        &mut self,
        t: TypeId,
        readonly: bool,
    ) -> TypeId {
        if !self.ty(t).flags.intersects(TypeFlags::UNION) {
            return t;
        }
        let types = self.ty(t).types_list();
        let mut every = true;
        for &u in &types {
            if !self.is_empty_object_type_or_spreads_into_empty_object(u) {
                every = false;
                break;
            }
        }
        if every {
            let mut empty = TypeId::NIL;
            for &u in &types {
                if self.is_empty_object_type(u) {
                    empty = u;
                    break;
                }
            }
            if empty.is_some() {
                return empty;
            }
            return self.empty_object_type;
        }
        let mut first_type = TypeId::NIL;
        for &u in &types {
            if !self.is_empty_object_type_or_spreads_into_empty_object(u) {
                first_type = u;
                break;
            }
        }
        if first_type.is_nil() {
            return t;
        }
        let mut second_type = TypeId::NIL;
        for &u in &types {
            if u != first_type && !self.is_empty_object_type_or_spreads_into_empty_object(u) {
                second_type = u;
                break;
            }
        }
        if second_type.is_some() {
            return t;
        }
        // gets the type as if it had been spread, but where everything in the spread is made optional
        let members = self.symbols.new_table();
        for prop in self.get_properties_of_type(first_type) {
            if self
                .get_declaration_modifier_flags_from_symbol(prop)
                .intersects(ModifierFlags::PRIVATE | ModifierFlags::PROTECTED)
            {
                // do nothing, skip privates
            } else if self.is_spreadable_property(prop) {
                let (prop_flags, prop_check_flags, prop_name) = {
                    let p = self.sym(prop);
                    (p.flags, p.check_flags, p.name.clone())
                };
                let is_setonly_accessor = prop_flags.intersects(SymbolFlags::SET_ACCESSOR)
                    && !prop_flags.intersects(SymbolFlags::GET_ACCESSOR);
                let flags = SymbolFlags::PROPERTY | SymbolFlags::OPTIONAL;
                let check_flags = (prop_check_flags & CheckFlags::LATE)
                    | if readonly {
                        CheckFlags::READONLY
                    } else {
                        CheckFlags::NONE
                    };
                let result = self.new_symbol_ex(flags, &prop_name, check_flags);
                // Go `links := c.valueSymbolLinks.Get(result)` gives the id here.
                self.value_symbol_links.get_by_id(&self.symbols, result);
                if is_setonly_accessor {
                    let undefined_type = self.undefined_type;
                    self.value_symbol_links
                        .get_by_id(&self.symbols, result)
                        .resolved_type = undefined_type;
                } else {
                    let prop_type = self.get_type_of_symbol(prop);
                    let resolved = self.add_optionality_ex(
                        prop_type, true, /*isProperty*/
                        true, /*isOptional*/
                    );
                    self.value_symbol_links
                        .get_by_id(&self.symbols, result)
                        .resolved_type = resolved;
                }
                let declarations = self.sym(prop).declarations.clone();
                self.sym_mut(result).declarations = declarations;
                let name_type = self
                    .value_symbol_links
                    .get_by_id(&self.symbols, prop)
                    .name_type;
                self.value_symbol_links
                    .get_by_id(&self.symbols, result)
                    .name_type = name_type;
                self.mapped_symbol_links.get(result).synthetic_origin = prop;
                self.symbols.set(members, prop_name, result);
            }
        }
        let first_symbol = self.ty(first_type).symbol;
        let index_infos = self.get_index_infos_of_type(first_type);
        let spread = self.new_anonymous_type(first_symbol, members, &[], &[], &index_infos);
        self.ty_mut(spread).object_flags |=
            ObjectFlags::OBJECT_LITERAL | ObjectFlags::CONTAINS_OBJECT_OR_ARRAY_LITERAL;
        spread
    }

    // We approximate own properties as non-methods plus methods that are inside the object literal
    // Go: checker/checker.go:13797 isSpreadableProperty
    pub fn is_spreadable_property(&self, prop: SymbolId) -> bool {
        let p = self.sym(prop);
        !p.declarations
            .iter()
            .any(|&d| is_private_identifier_class_element_declaration(d))
            && !p.flags.intersects(
                SymbolFlags::METHOD | SymbolFlags::GET_ACCESSOR | SymbolFlags::SET_ACCESSOR,
            )
            || !p
                .declarations
                .iter()
                .any(|&d| d.parent().is_some() && is_class_like(d.parent()))
    }

    // Go: checker/checker.go:13802 getSpreadSymbol
    pub fn get_spread_symbol(&mut self, prop: SymbolId, readonly: bool) -> SymbolId {
        let (prop_flags, prop_check_flags, prop_name) = {
            let p = self.sym(prop);
            (p.flags, p.check_flags, p.name.clone())
        };
        let is_setonly_accessor = prop_flags.intersects(SymbolFlags::SET_ACCESSOR)
            && !prop_flags.intersects(SymbolFlags::GET_ACCESSOR);
        if !is_setonly_accessor && readonly == self.is_readonly_symbol(prop) {
            return prop;
        }
        let flags = SymbolFlags::PROPERTY | (prop_flags & SymbolFlags::OPTIONAL);
        let check_flags = (prop_check_flags & CheckFlags::LATE)
            | if readonly {
                CheckFlags::READONLY
            } else {
                CheckFlags::NONE
            };
        let result = self.new_symbol_ex(flags, &prop_name, check_flags);
        // Go `links := c.valueSymbolLinks.Get(result)` gives the id here.
        self.value_symbol_links.get_by_id(&self.symbols, result);
        if is_setonly_accessor {
            let undefined_type = self.undefined_type;
            self.value_symbol_links
                .get_by_id(&self.symbols, result)
                .resolved_type = undefined_type;
        } else {
            let prop_type = self.get_type_of_symbol(prop);
            self.value_symbol_links
                .get_by_id(&self.symbols, result)
                .resolved_type = prop_type;
        }
        let declarations = self.sym(prop).declarations.clone();
        self.sym_mut(result).declarations = declarations;
        let name_type = self
            .value_symbol_links
            .get_by_id(&self.symbols, prop)
            .name_type;
        self.value_symbol_links
            .get_by_id(&self.symbols, result)
            .name_type = name_type;
        self.mapped_symbol_links.get(result).synthetic_origin = prop;
        result
    }

    // Go: checker/checker.go:13821 isEmptyObjectTypeOrSpreadsIntoEmptyObject
    pub fn is_empty_object_type_or_spreads_into_empty_object(&mut self, t: TypeId) -> bool {
        self.is_empty_object_type(t)
            || self.ty(t).flags.intersects(
                TypeFlags::NULL
                    | TypeFlags::UNDEFINED
                    | TypeFlags::BOOLEAN_LIKE
                    | TypeFlags::NUMBER_LIKE
                    | TypeFlags::BIG_INT_LIKE
                    | TypeFlags::STRING_LIKE
                    | TypeFlags::ENUM_LIKE
                    | TypeFlags::NON_PRIMITIVE
                    | TypeFlags::INDEX,
            )
    }

    // Go: checker/checker.go:13825 hasDefaultValue
    pub fn has_default_value(&self, node: Node) -> bool {
        is_binding_element(node) && node.initializer().is_some()
            || is_property_assignment(node) && self.has_default_value(node.initializer())
            || is_shorthand_property_assignment(node)
                && node.object_assignment_initializer().is_some()
            || is_binary_expression(node) && node.operator_token().kind() == SyntaxKind::EqualsToken
    }

    // Go: checker/checker.go:13832 isConstContext
    pub fn is_const_context(&mut self, node: Node) -> bool {
        let parent = node.parent();
        if is_const_assertion(parent) {
            return true;
        }
        if self.is_inline_import_attributes(node) {
            return true;
        }
        if self.is_valid_const_assertion_argument(node) {
            let contextual_type = self.get_contextual_type(node, ContextFlags::NONE);
            if self.is_const_type_variable(contextual_type, 0) {
                return true;
            }
        }
        if (is_parenthesized_expression(parent)
            || is_array_literal_expression(parent)
            || is_spread_element(parent))
            && self.is_const_context(parent)
        {
            return true;
        }
        (is_property_assignment(parent)
            || is_shorthand_property_assignment(parent)
            || is_template_span(parent))
            && self.is_const_context(parent.parent())
    }

    // Go: checker/checker.go:13841 isInlineImportAttributes
    pub fn is_inline_import_attributes(&self, node: Node) -> bool {
        if !is_object_literal_expression(node)
            || !is_property_assignment(node.parent())
            || node.parent().initializer() != node
        {
            return false;
        }
        let property = node.parent();
        if (!is_identifier(property.name()) && !is_string_literal_like(property.name()))
            || property.name().text() != "with"
        {
            return false;
        }
        let options = property.parent();
        if !is_object_literal_expression(options) {
            return false;
        }
        let import_call = find_ancestor(options, is_import_call);
        import_call.is_some()
            && import_call.arguments().len() > 1
            && skip_parentheses(import_call.arguments().get(1)) == options
    }

    // Go: checker/checker.go:13857 isValidConstAssertionArgument
    pub fn is_valid_const_assertion_argument(&mut self, node: Node) -> bool {
        match node.kind() {
            SyntaxKind::StringLiteral
            | SyntaxKind::NoSubstitutionTemplateLiteral
            | SyntaxKind::NumericLiteral
            | SyntaxKind::BigIntLiteral
            | SyntaxKind::TrueKeyword
            | SyntaxKind::FalseKeyword
            | SyntaxKind::ArrayLiteralExpression
            | SyntaxKind::ObjectLiteralExpression
            | SyntaxKind::TemplateExpression => true,
            SyntaxKind::ParenthesizedExpression => {
                self.is_valid_const_assertion_argument(node.expression())
            }
            SyntaxKind::PrefixUnaryExpression => {
                let op = node.operator();
                let arg = node.operand();
                op == SyntaxKind::MinusToken
                    && (arg.kind() == SyntaxKind::NumericLiteral
                        || arg.kind() == SyntaxKind::BigIntLiteral)
                    || op == SyntaxKind::PlusToken && arg.kind() == SyntaxKind::NumericLiteral
            }
            SyntaxKind::PropertyAccessExpression | SyntaxKind::ElementAccessExpression => {
                let expr = skip_parentheses(node.expression());
                let mut symbol = SymbolId::NIL;
                if is_entity_name_expression(expr) {
                    symbol = self.resolve_entity_name(
                        expr,
                        SymbolFlags::VALUE,
                        true, /*ignoreErrors*/
                        false,
                        Node::NIL,
                    );
                }
                symbol.is_some() && self.sym(symbol).flags.intersects(SymbolFlags::ENUM)
            }
            _ => false,
        }
    }

    // Go: checker/checker.go:13879 isConstTypeVariable
    pub fn is_const_type_variable(&mut self, t: TypeId, depth: i32) -> bool {
        if depth >= 5 || t.is_nil() {
            return false;
        }
        let flags = self.ty(t).flags;
        if flags.intersects(TypeFlags::TYPE_PARAMETER) {
            let symbol = self.ty(t).symbol;
            return symbol.is_some()
                && self
                    .sym(symbol)
                    .declarations
                    .iter()
                    .any(|&d| has_syntactic_modifier(d, ModifierFlags::CONST));
        } else if flags.intersects(TypeFlags::UNION_OR_INTERSECTION) {
            let types = self.ty(t).types_list();
            for s in types {
                if self.is_const_type_variable(s, depth) {
                    return true;
                }
            }
            return false;
        } else if flags.intersects(TypeFlags::INDEXED_ACCESS) {
            let object_type = self.ty(t).as_indexed_access_type().object_type;
            return self.is_const_type_variable(object_type, depth + 1);
        } else if flags.intersects(TypeFlags::CONDITIONAL) {
            let constraint = self.get_constraint_of_conditional_type(t);
            return self.is_const_type_variable(constraint, depth + 1);
        } else if flags.intersects(TypeFlags::SUBSTITUTION) {
            let base_type = self.ty(t).as_substitution_type().base_type;
            return self.is_const_type_variable(base_type, depth);
        } else if self.ty(t).object_flags.intersects(ObjectFlags::MAPPED) {
            let type_variable = self.get_homomorphic_type_variable(t);
            return type_variable.is_some() && self.is_const_type_variable(type_variable, depth);
        } else if self.is_generic_tuple_type(t) {
            let element_types = self.get_element_types(t);
            for (i, s) in element_types.into_iter().enumerate() {
                if self.target_tuple_type(t).element_infos[i]
                    .flags
                    .intersects(ElementFlags::VARIADIC)
                    && self.is_const_type_variable(s, depth)
                {
                    return true;
                }
            }
        }
        false
    }

    // Go: checker/checker.go:13907 checkPropertyAssignment
    pub fn check_property_assignment(&mut self, node: Node, check_mode: CheckMode) -> TypeId {
        // Do not use hasDynamicName here, because that returns false for well known symbols.
        // We want to perform checkComputedPropertyName for all computed properties, including
        // well known symbols.
        if is_computed_property_name(node.name()) {
            self.check_computed_property_name(node.name());
        }
        let initializer_type =
            self.check_expression_for_mutable_location(node.initializer(), check_mode);
        if node.type_().is_some() {
            let t = self.get_type_from_type_node(node.type_());
            self.check_type_assignable_to_and_optionally_elaborate(
                initializer_type,
                t,
                node,
                node.initializer(),
                None, /*headMessage*/
                None,
            );
            return t;
        }
        initializer_type
    }

    // Go: checker/checker.go:13923 checkShorthandPropertyAssignment
    pub fn check_shorthand_property_assignment(
        &mut self,
        node: Node,
        in_destructuring_pattern: bool,
        check_mode: CheckMode,
    ) -> TypeId {
        let mut expr = Node::NIL;
        if !in_destructuring_pattern {
            expr = node.object_assignment_initializer();
        }
        if expr.is_nil() {
            expr = node.name();
        }
        let expression_type = self.check_expression_for_mutable_location(expr, check_mode);
        if node.type_().is_some() {
            let t = self.get_type_from_type_node(node.type_());
            self.check_type_assignable_to_and_optionally_elaborate(
                expression_type,
                t,
                node,
                expr,
                None, /*headMessage*/
                None,
            );
            return t;
        }
        expression_type
    }

    // Go: checker/checker.go:13940 isInPropertyInitializerOrClassStaticBlock
    pub fn is_in_property_initializer_or_class_static_block(
        &self,
        node: Node,
        ignore_arrow_functions: bool,
    ) -> bool {
        find_ancestor_or_quit(node, |node: Node| match node.kind() {
            SyntaxKind::PropertyDeclaration | SyntaxKind::ClassStaticBlockDeclaration => {
                FindAncestorResult::FIND_ANCESTOR_TRUE
            }
            SyntaxKind::TypeQuery | SyntaxKind::JsxClosingElement => {
                FindAncestorResult::FIND_ANCESTOR_QUIT
            }
            SyntaxKind::ArrowFunction => {
                if ignore_arrow_functions {
                    FindAncestorResult::FIND_ANCESTOR_FALSE
                } else {
                    FindAncestorResult::FIND_ANCESTOR_QUIT
                }
            }
            SyntaxKind::Block => {
                if is_function_like_declaration(node.parent())
                    && node.parent().kind() != SyntaxKind::ArrowFunction
                {
                    FindAncestorResult::FIND_ANCESTOR_QUIT
                } else {
                    FindAncestorResult::FIND_ANCESTOR_FALSE
                }
            }
            _ => FindAncestorResult::FIND_ANCESTOR_FALSE,
        })
        .is_some()
    }

    // Go: checker/checker.go:13957 getNarrowedTypeOfSymbol
    pub fn get_narrowed_type_of_symbol(&mut self, symbol: SymbolId, location: Node) -> TypeId {
        let mut t = self.get_type_of_symbol(symbol);
        let declaration = self.sym(symbol).value_declaration;
        if declaration.is_some() {
            // If we have a non-rest binding element with no initializer declared as a const variable or a const-like
            // parameter (a parameter for which there are no assignments in the function body), and if the parent type
            // for the destructuring is a union type, one or more of the binding elements may represent discriminant
            // properties, and we want the effects of conditional checks on such discriminants to affect the types of
            // other binding elements from the same destructuring. Consider:
            //
            //   type Action =
            //       | { kind: 'A', payload: number }
            //       | { kind: 'B', payload: string };
            //
            //   function f({ kind, payload }: Action) {
            //       if (kind === 'A') {
            //           payload.toFixed();
            //       }
            //       if (kind === 'B') {
            //           payload.toUpperCase();
            //       }
            //   }
            //
            // Above, we want the conditional checks on 'kind' to affect the type of 'payload'. To facilitate this, we use
            // the binding pattern AST instance for '{ kind, payload }' as a pseudo-reference and narrow this reference
            // as if it occurred in the specified location. We then recompute the narrowed binding element type by
            // destructuring from the narrowed parent type.
            if is_binding_element(declaration)
                && declaration.initializer().is_nil()
                && !has_dot_dot_dot_token(declaration)
                && declaration.parent().elements().len() >= 2
            {
                let root_declaration = get_root_declaration(declaration);
                let root_initializer = root_declaration.initializer();
                // Avoid declaration circularity without blocking binding defaults or nested callbacks.
                // PORT: Go `break` leaves the switch case; here the rest of the case is in the `if`.
                if !(root_initializer.is_some()
                    && is_node_descendant_of(location, root_initializer)
                    && self.get_control_flow_container(declaration)
                        == self.get_control_flow_container(location))
                {
                    let parent = declaration.parent().parent();
                    if is_variable_declaration(root_declaration)
                        && self
                            .get_combined_node_flags_cached(root_declaration)
                            .intersects(NodeFlags::CONSTANT)
                        || is_parameter_declaration(root_declaration)
                    {
                        if !self
                            .node_links
                            .get(parent)
                            .flags
                            .intersects(NodeCheckFlags::IN_CHECK_IDENTIFIER)
                        {
                            self.node_links.get(parent).flags |=
                                NodeCheckFlags::IN_CHECK_IDENTIFIER;
                            let parent_type =
                                self.get_type_for_binding_element_parent(parent, CheckMode::NORMAL);
                            let mut parent_type_constraint = TypeId::NIL;
                            if parent_type.is_some() {
                                parent_type_constraint = self.map_type(
                                    parent_type,
                                    &mut |c: &mut Checker, t: TypeId| {
                                        c.get_base_constraint_or_type(t)
                                    },
                                );
                            }
                            // Guard parent-type resolution only; flow analysis should allow re-entrant narrowing
                            let links = self.node_links.get(parent);
                            links.flags = links.flags.without(NodeCheckFlags::IN_CHECK_IDENTIFIER);
                            if parent_type_constraint.is_some()
                                && self
                                    .ty(parent_type_constraint)
                                    .flags
                                    .intersects(TypeFlags::UNION)
                                && !(is_parameter_declaration(root_declaration)
                                    && self.is_some_symbol_assigned(root_declaration))
                            {
                                let pattern = declaration.parent();
                                let narrowed_type = self.get_flow_type_of_reference_ex(
                                    pattern,
                                    parent_type_constraint,
                                    parent_type_constraint,
                                    Node::NIL, /*flowContainer*/
                                    get_flow_node_of_node(location),
                                );
                                if self.ty(narrowed_type).flags.intersects(TypeFlags::NEVER) {
                                    t = self.never_type;
                                } else {
                                    // Destructurings are validated against the parent type elsewhere. Here we disable tuple bounds
                                    // checks because the narrowed type may have lower arity than the full parent type. For example,
                                    // for the declaration [x, y]: [1, 2] | [3], we may have narrowed the parent type to just [3].
                                    t = self.get_binding_element_type_from_parent_type(
                                        declaration,
                                        narrowed_type,
                                        true, /*noTupleBoundsCheck*/
                                    );
                                }
                            }
                        }
                    }
                }
            // If we have a const-like parameter with no type annotation or initializer, and if the parameter is contextually
            // typed by a signature with a single rest parameter of a union of tuple types, one or more of the parameters may
            // represent discriminant tuple elements, and we want the effects of conditional checks on such discriminants to
            // affect the types of other parameters in the same parameter list. Consider:
            //
            //   type Action = [kind: 'A', payload: number] | [kind: 'B', payload: string];
            //
            //   const f: (...args: Action) => void = (kind, payload) => {
            //       if (kind === 'A') {
            //           payload.toFixed();
            //       }
            //       if (kind === 'B') {
            //           payload.toUpperCase();
            //       }
            //   }
            //
            // Above, we want the conditional checks on 'kind' to affect the type of 'payload'. To facilitate this, we use
            // the arrow function AST node for '(kind, payload) => ...' as a pseudo-reference and narrow this reference as
            // if it occurred in the specified location. We then recompute the narrowed parameter type by indexing into the
            // narrowed tuple type.
            } else if is_parameter_declaration(declaration)
                && declaration.type_().is_nil()
                && declaration.initializer().is_nil()
                && !has_dot_dot_dot_token(declaration)
            {
                let func = declaration.parent();
                if func.parameters().len() >= 2
                    && self.is_context_sensitive_function_or_object_literal_method(func)
                {
                    let contextual_signature = self.get_contextual_signature(func);
                    if contextual_signature.is_some()
                        && self.sig(contextual_signature).parameters.len() == 1
                        && self.signature_has_rest_parameter(contextual_signature)
                    {
                        let mut mapper = MapperId::NIL;
                        let context = self.get_inference_context(func);
                        if context.is_some() {
                            mapper = self.inference_context(context).non_fixing_mapper;
                        }
                        let rest_param = self.sig(contextual_signature).parameters[0];
                        let rest_param_type = self.get_type_of_symbol(rest_param);
                        let instantiated = self.instantiate_type(rest_param_type, mapper);
                        let rest_type = self.get_reduced_apparent_type(instantiated);
                        if self.ty(rest_type).flags.intersects(TypeFlags::UNION)
                            && self.every_type(rest_type, &mut |c: &mut Checker, t: TypeId| {
                                c.is_tuple_type(t)
                            })
                            && !func
                                .parameters()
                                .iter()
                                .any(|p| self.is_some_symbol_assigned(p))
                        {
                            let narrowed_type = self.get_flow_type_of_reference_ex(
                                func,
                                rest_type,
                                rest_type,
                                Node::NIL, /*flowContainer*/
                                get_flow_node_of_node(location),
                            );
                            let position = match func
                                .parameters()
                                .to_vec()
                                .iter()
                                .position(|&p| p == declaration)
                            {
                                Some(i) => i as i64,
                                None => -1,
                            };
                            let index = position
                                - if get_this_parameter(func).is_some() {
                                    1
                                } else {
                                    0
                                };
                            let index_type = self
                                .get_number_literal_type(crate::jsnum::Number::from(index as f64));
                            t = self.get_indexed_access_type(narrowed_type, index_type);
                        }
                    }
                }
            }
        }
        t
    }

    // Go: checker/checker.go:14063 isReadonlyAssignmentDeclaration
    pub fn is_readonly_assignment_declaration(&mut self, node: Node) -> bool {
        if !is_call_expression(node) {
            return false;
        }
        let property_descriptor_type = self.check_expression_cached(node.arguments().get(2));
        let value_type = self.get_type_of_property_of_type(property_descriptor_type, "value");
        if value_type.is_some() {
            let writable_prop = self.get_property_of_type(property_descriptor_type, "writable");
            if writable_prop.is_some() {
                let value_declaration = self.sym(writable_prop).value_declaration;
                let writable_type =
                    if value_declaration.is_some() && is_property_assignment(value_declaration) {
                        self.check_expression(value_declaration.initializer())
                    } else {
                        self.get_type_of_symbol(writable_prop)
                    };
                return self
                    .ty(writable_type)
                    .flags
                    .intersects(TypeFlags::BOOLEAN_LITERAL)
                    && !self.get_boolean_literal_value(writable_type);
            }
            return true;
        }
        self.get_type_of_property_of_type(property_descriptor_type, "set")
            .is_nil()
    }

    // Go: checker/checker.go:14083 isReadonlySymbol
    pub fn is_readonly_symbol(&mut self, symbol: SymbolId) -> bool {
        // The following symbols are considered read-only:
        // Properties with a 'readonly' modifier
        // Variables declared with 'const'
        // Get accessors without matching set accessors
        // Enum members
        // Object.defineProperty assignments with writable false or no setter
        // Unions and intersections of the above (unions and intersections eagerly set isReadonly on creation)
        let (flags, check_flags) = {
            let s = self.sym(symbol);
            (s.flags, s.check_flags)
        };
        if check_flags.intersects(CheckFlags::READONLY) {
            return true;
        }
        if flags.intersects(SymbolFlags::PROPERTY)
            && self
                .get_declaration_modifier_flags_from_symbol(symbol)
                .intersects(ModifierFlags::READONLY)
        {
            return true;
        }
        if flags.intersects(SymbolFlags::VARIABLE)
            && self
                .get_declaration_node_flags_from_symbol(symbol)
                .intersects(NodeFlags::CONSTANT)
        {
            return true;
        }
        if flags.intersects(SymbolFlags::ACCESSOR) && !flags.intersects(SymbolFlags::SET_ACCESSOR) {
            return true;
        }
        if flags.intersects(SymbolFlags::ENUM_MEMBER) {
            return true;
        }
        // PORT: isReadonlyAssignmentDeclaration is false for anything but a
        // call expression, so the list is only copied when it has one.
        if !self
            .sym(symbol)
            .declarations
            .iter()
            .any(|&d| is_call_expression(d))
        {
            return false;
        }
        let declarations = self.sym(symbol).declarations.clone();
        declarations
            .iter()
            .any(|&d| self.is_readonly_assignment_declaration(d))
    }

    // Go: checker/checker.go:14099 checkObjectLiteralMethod
    pub fn check_object_literal_method(&mut self, node: Node, check_mode: CheckMode) -> TypeId {
        // Grammar checking
        self.check_grammar_method(node);
        // Do not use hasDynamicName here, because that returns false for well known symbols.
        // We want to perform checkComputedPropertyName for all computed properties, including
        // well known symbols.
        if is_computed_property_name(node.name()) {
            self.check_computed_property_name(node.name());
        }
        let uninstantiated_type =
            self.check_function_expression_or_object_literal_method(node, check_mode);
        self.instantiate_type_with_single_generic_call_signature(
            node,
            uninstantiated_type,
            check_mode,
        )
    }

    // Go: checker/checker.go:14112 checkExpressionForMutableLocation
    pub fn check_expression_for_mutable_location(
        &mut self,
        node: Node,
        check_mode: CheckMode,
    ) -> TypeId {
        let t = self.check_expression_ex(node, check_mode);
        if self.is_const_context(node) {
            self.get_regular_type_of_literal_type(t)
        } else if checker_is_type_assertion(node) {
            t
        } else {
            let contextual_type = self.get_contextual_type(node, ContextFlags::NONE);
            let instantiated =
                self.instantiate_contextual_type(contextual_type, node, ContextFlags::NONE);
            self.get_widened_literal_like_type_for_contextual_type(t, instantiated)
        }
    }

    // Go: checker/checker.go:14124 getResolvedSymbol
    pub fn get_resolved_symbol(&mut self, node: Node) -> SymbolId {
        // One link lookup on the cached hit. The miss returns the value it
        // just stored, as Go returns links.resolvedSymbol.
        let cached = self.symbol_node_links.get(node).resolved_symbol;
        if cached.is_some() {
            return cached;
        }
        let mut symbol = SymbolId::NIL;
        if !node_is_missing(node) {
            // PERF: U1 (a). The name interned at parse (`Node::text_name`)
            // goes to the resolver with its text (`resolver_name_text`), so
            // neither the node data nor an intern is needed.
            let text = resolver_name_text(node.text_name());
            // PERF: the resolver builds the Go
            // `getCannotFindNameDiagnosticForName(node)` message only when
            // the name is not found (see `NameNotFound`).
            let resolve_name = self.resolve_name.clone();
            symbol = resolve_name(
                self,
                node,
                text,
                SymbolFlags::VALUE | SymbolFlags::EXPORT_VALUE,
                Some(NameNotFound::CannotFindName(node)),
                !is_write_only_access(node),
                false, /*excludeGlobals*/
            );
        }
        let resolved = if symbol.is_some() {
            symbol
        } else {
            self.unknown_symbol
        };
        self.symbol_node_links.get(node).resolved_symbol = resolved;
        resolved
    }

    // Go: checker/checker.go:14137 getResolvedSymbolOrNil
    pub fn get_resolved_symbol_or_nil(&mut self, node: Node) -> SymbolId {
        self.symbol_node_links.get(node).resolved_symbol
    }

    // Go: checker/checker.go:14141 getReferencedValueOrAliasSymbol
    pub fn get_referenced_value_or_alias_symbol(&mut self, reference: Node) -> SymbolId {
        let resolved_symbol = self.symbol_node_links.get(reference).resolved_symbol;
        if resolved_symbol.is_some() && resolved_symbol != self.unknown_symbol {
            return resolved_symbol;
        }
        let text = reference.text();
        let resolve_name = self.resolve_name.clone();
        resolve_name(
            self,
            reference,
            &text,
            SymbolFlags::VALUE | SymbolFlags::EXPORT_VALUE | SymbolFlags::ALIAS,
            None,
            false, /*isUse*/
            false, /*excludeGlobals*/
        )
    }

    // Go: checker/checker.go:14149 getCannotFindNameDiagnosticForName
    pub fn get_cannot_find_name_diagnostic_for_name(&self, node: Node) -> &'static Message {
        let text = node.text();
        let uses_wildcard_types = self.compiler_options.uses_wildcard_types();
        match &*text {
            "document" | "console" => {
                diag::Cannot_find_name_0_Do_you_need_to_change_your_target_library_Try_changing_the_lib_compiler_option_to_include_dom
            }
            "$" => {
                if uses_wildcard_types {
                    diag::Cannot_find_name_0_Do_you_need_to_install_type_definitions_for_jQuery_Try_npm_i_save_dev_types_Slashjquery
                } else {
                    diag::Cannot_find_name_0_Do_you_need_to_install_type_definitions_for_jQuery_Try_npm_i_save_dev_types_Slashjquery_and_then_add_jquery_to_the_types_field_in_your_tsconfig
                }
            }
            "beforeEach" | "describe" | "suite" | "it" | "test" => {
                if uses_wildcard_types {
                    diag::Cannot_find_name_0_Do_you_need_to_install_type_definitions_for_a_test_runner_Try_npm_i_save_dev_types_Slashjest_or_npm_i_save_dev_types_Slashmocha
                } else {
                    diag::Cannot_find_name_0_Do_you_need_to_install_type_definitions_for_a_test_runner_Try_npm_i_save_dev_types_Slashjest_or_npm_i_save_dev_types_Slashmocha_and_then_add_jest_or_mocha_to_the_types_field_in_your_tsconfig
                }
            }
            "process" | "require" | "Buffer" | "module" | "NodeJS" => {
                if uses_wildcard_types {
                    diag::Cannot_find_name_0_Do_you_need_to_install_type_definitions_for_node_Try_npm_i_save_dev_types_Slashnode
                } else {
                    diag::Cannot_find_name_0_Do_you_need_to_install_type_definitions_for_node_Try_npm_i_save_dev_types_Slashnode_and_then_add_node_to_the_types_field_in_your_tsconfig
                }
            }
            "Bun" => {
                if uses_wildcard_types {
                    diag::Cannot_find_name_0_Do_you_need_to_install_type_definitions_for_Bun_Try_npm_i_save_dev_types_Slashbun
                } else {
                    diag::Cannot_find_name_0_Do_you_need_to_install_type_definitions_for_Bun_Try_npm_i_save_dev_types_Slashbun_and_then_add_bun_to_the_types_field_in_your_tsconfig
                }
            }
            // PORT: Go lists the literal "ast.Symbol" here (a rename artifact); kept as-is for fidelity.
            "Map" | "Set" | "Promise" | "ast.Symbol" | "WeakMap" | "WeakSet" | "Iterator" | "AsyncIterator"
            | "SharedArrayBuffer" | "Atomics" | "AsyncIterable" | "AsyncIterableIterator" | "AsyncGenerator"
            | "AsyncGeneratorFunction" | "BigInt" | "Reflect" | "BigInt64Array" | "BigUint64Array" => {
                diag::Cannot_find_name_0_Do_you_need_to_change_your_target_library_Try_changing_the_lib_compiler_option_to_1_or_later
            }
            _ => {
                // PORT: Go's "await" case falls through to the default branch when the parent is not a call.
                if &*text == "await" && is_call_expression(node.parent()) {
                    return diag::Cannot_find_name_0_Did_you_mean_to_write_this_in_an_async_function;
                }
                if node.parent().kind() == SyntaxKind::ShorthandPropertyAssignment {
                    return diag::No_value_exists_in_scope_for_the_shorthand_property_0_Either_declare_one_or_provide_an_initializer;
                }
                diag::Cannot_find_name_0
            }
        }
    }

    // Go: checker/checker.go:14185 GetDiagnostics
    pub fn get_diagnostics_exported(
        &mut self,
        ctx: &Context,
        source_file: Node,
    ) -> Vec<Diagnostic> {
        self.get_diagnostics(ctx, source_file, false)
    }

    // Go: checker/checker.go:14189 GetSuggestionDiagnostics
    pub fn get_suggestion_diagnostics(
        &mut self,
        ctx: &Context,
        source_file: Node,
    ) -> Vec<Diagnostic> {
        self.get_diagnostics(ctx, source_file, true)
    }

    // Go: checker/checker.go:14193 getDiagnostics
    // PORT: the Go `collection *ast.DiagnosticsCollection` pointer is `is_suggestion`
    // (true selects `suggestion_diagnostics`, false selects `diagnostics`).
    pub fn get_diagnostics(
        &mut self,
        ctx: &Context,
        source_file: Node,
        is_suggestion: bool,
    ) -> Vec<Diagnostic> {
        self.check_not_canceled();
        let check_unused = self.compiler_options.no_unused_locals.is_true()
            || self.compiler_options.no_unused_parameters.is_true()
            || is_suggestion;
        self.check_source_file(ctx, source_file, check_unused);
        if self.was_canceled {
            return Vec::new();
        }
        // Go (#4825) passes the source file, not its name.
        if is_suggestion {
            self.suggestion_diagnostics
                .get_diagnostics_for_file(source_file)
        } else {
            self.diagnostics.get_diagnostics_for_file(source_file)
        }
    }

    // Go: checker/checker.go:14203 GetGlobalDiagnostics
    pub fn get_global_diagnostics(&mut self) -> Vec<Diagnostic> {
        self.check_not_canceled();
        self.produce_deferred_diagnostics();
        self.diagnostics.get_global_diagnostics()
    }

    // Go: checker/checker.go:14209 addDeferredDiagnostic
    pub fn add_deferred_diagnostic(&mut self, callback: Rc<dyn Fn(&mut Checker)>) {
        self.deferred_diagnostic_callbacks.push(callback);
    }

    // Go: checker/checker.go:14213 produceDeferredDiagnostics
    pub fn produce_deferred_diagnostics(&mut self) {
        let callbacks = self.deferred_diagnostic_callbacks.clone();
        for cb in callbacks {
            cb(self);
        }
        self.deferred_diagnostic_callbacks = Vec::new();
    }

    // Go: checker/checker.go:14220 addDiagnostic
    // PORT: Go (#4825) returns the stored `*ast.Diagnostic`: an equal one that is
    // already in the collection, or this one. A caller that changes the result
    // changes the stored diagnostic. Here `Some` is the stored diagnostic. `None`
    // means it was discarded: Go then returns it unstored, and a change to it is
    // not seen.
    pub fn add_diagnostic(&mut self, diagnostic: Diagnostic) -> Option<&mut Diagnostic> {
        // Discard diagnostics created while at the maximum number of recursive TypeToString invocations.
        if self.serialization_level < MAX_SERIALIZATION_LEVEL {
            return Some(self.diagnostics.add(diagnostic));
        }
        None
    }

    // Go: checker/checker.go:14228 addSuggestionDiagnostic
    // PORT: the stored diagnostic, or `None` when discarded (see `add_diagnostic`).
    pub fn add_suggestion_diagnostic(&mut self, diagnostic: Diagnostic) -> Option<&mut Diagnostic> {
        // Discard diagnostics created while at the maximum number of recursive TypeToString invocations.
        if self.serialization_level < MAX_SERIALIZATION_LEVEL {
            return Some(self.suggestion_diagnostics.add(diagnostic));
        }
        None
    }

    // Go: checker/checker.go:14236 error
    // PORT: Go (#4825) returns `c.addDiagnostic(...)`, the stored diagnostic or the
    // discarded one. Here the result is a clone of it. A caller that changes the
    // stored diagnostic calls `add_diagnostic` and changes the one it returns.
    pub fn error(
        &mut self,
        location: Node,
        message: &'static Message,
        args: Vec<String>,
    ) -> Diagnostic {
        let diagnostic = new_diagnostic_for_node(location, message, args);
        // Inlined `c.addDiagnostic`, so the discarded diagnostic can be returned.
        if self.serialization_level < MAX_SERIALIZATION_LEVEL {
            return self.diagnostics.add(diagnostic).clone();
        }
        diagnostic
    }
}
