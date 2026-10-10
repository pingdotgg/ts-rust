//! Port of typescript-go `internal/checker/checker.go` lines 28753-29671.
//! Read `crates/ts_goport/PORTING.md` before editing.

use crate::jsnum::Number;
use crate::prelude::*;
use smallvec::SmallVec;
use std::borrow::Cow;

// Go: checker/checker.go:29571 maxTemplateLiteralTypeLength, maxTemplateLiteralTypeSpans
// Limits on the size of a template literal type produced by getTemplateLiteralType. Recursive instantiations
// such as `Recur<any, `${S}_${S}`>` double the text (or the number of placeholders) on every iteration and
// exhaust memory long before the tail recursion limit in getConditionalType is reached (see #63271).
const MAX_TEMPLATE_LITERAL_TYPE_LENGTH: usize = 50_000_000;
const MAX_TEMPLATE_LITERAL_TYPE_SPANS: usize = 100_000;

impl Checker {
    // Go: checker/checker.go:29466 getTypeOfFirstParameterOfSignature
    pub fn get_type_of_first_parameter_of_signature(&mut self, signature: SignatureId) -> TypeId {
        let never_type = self.never_type;
        self.get_type_of_first_parameter_of_signature_with_fallback(signature, never_type)
    }

    // Go: checker/checker.go:29470 getTypeOfFirstParameterOfSignatureWithFallback
    pub fn get_type_of_first_parameter_of_signature_with_fallback(
        &mut self,
        signature: SignatureId,
        fallback_type: TypeId,
    ) -> TypeId {
        if !self.sig(signature).parameters.is_empty() {
            return self.get_type_at_position(signature, 0);
        }
        fallback_type
    }

    // Go: checker/checker.go:29477 getMappedTypeModifiers
    // PORT: Go package function; it reads type data, so it is a `Checker` method.
    pub fn get_mapped_type_modifiers(&self, t: TypeId) -> MappedTypeModifiers {
        let declaration = self.ty(t).as_mapped_type().declaration;
        let mut modifiers = MappedTypeModifiers::default();
        let readonly_token = declaration.readonly_token();
        if readonly_token.is_some() {
            modifiers = modifiers
                | if readonly_token.kind() == SyntaxKind::MinusToken {
                    MappedTypeModifiers::EXCLUDE_READONLY
                } else {
                    MappedTypeModifiers::INCLUDE_READONLY
                };
        }
        let question_token = declaration.question_token();
        if question_token.is_some() {
            modifiers = modifiers
                | if question_token.kind() == SyntaxKind::MinusToken {
                    MappedTypeModifiers::EXCLUDE_OPTIONAL
                } else {
                    MappedTypeModifiers::INCLUDE_OPTIONAL
                };
        }
        modifiers
    }

    // Go: checker/checker.go:29491 getMappedTypeOptionality
    // Return -1, 0, or 1, where -1 means optionality is stripped (i.e. -?), 0 means optionality is unchanged, and 1 means
    // optionality is added (i.e. +?).
    pub fn get_mapped_type_optionality(&self, t: TypeId) -> i32 {
        let modifiers = self.get_mapped_type_modifiers(t);
        if modifiers.intersects(MappedTypeModifiers::EXCLUDE_OPTIONAL) {
            return -1;
        }
        if modifiers.intersects(MappedTypeModifiers::INCLUDE_OPTIONAL) {
            return 1;
        }
        0
    }

    // Go: checker/checker.go:29505 getCombinedMappedTypeOptionality
    // Return -1, 0, or 1, for stripped, unchanged, or added optionality respectively. When a homomorphic mapped type doesn't
    // modify optionality, recursively consult the optionality of the type being mapped over to see if it strips or adds optionality.
    // For intersections, return -1 or 1 when all constituents strip or add optionality, otherwise return 0.
    pub fn get_combined_mapped_type_optionality(&mut self, t: TypeId) -> i32 {
        if self.ty(t).object_flags.intersects(ObjectFlags::MAPPED) {
            let optionality = self.get_mapped_type_optionality(t);
            if optionality != 0 {
                return optionality;
            }
            let modifiers_type = self.get_modifiers_type_from_mapped_type(t);
            return self.get_combined_mapped_type_optionality(modifiers_type);
        }
        if self.ty(t).flags.intersects(TypeFlags::INTERSECTION) {
            let types = self.ty(t).types_list();
            let optionality = self.get_combined_mapped_type_optionality(types[0]);
            for &t in &types[1..] {
                if self.get_combined_mapped_type_optionality(t) != optionality {
                    return 0;
                }
            }
            return optionality;
        }
        0
    }

    // Go: checker/checker.go:29525 isPartialMappedType
    pub fn is_partial_mapped_type(&self, t: TypeId) -> bool {
        self.ty(t).object_flags.intersects(ObjectFlags::MAPPED)
            && self
                .get_mapped_type_modifiers(t)
                .intersects(MappedTypeModifiers::INCLUDE_OPTIONAL)
    }

    // Go: checker/checker.go:29529 getOptionalExpressionType
    pub fn get_optional_expression_type(&mut self, expr_type: TypeId, expression: Node) -> TypeId {
        if is_expression_of_optional_chain_root(expression) {
            return self.get_non_nullable_type(expr_type);
        }
        if is_optional_chain(expression) {
            return self.remove_optional_type_marker(expr_type);
        }
        expr_type
    }

    // Go: checker/checker.go:29540 removeOptionalTypeMarker
    pub fn remove_optional_type_marker(&mut self, t: TypeId) -> TypeId {
        if self.strict_null_checks {
            let optional_type = self.optional_type;
            return self.remove_type(t, optional_type);
        }
        t
    }

    // Go: checker/checker.go:29547 propagateOptionalTypeMarker
    pub fn propagate_optional_type_marker(
        &mut self,
        t: TypeId,
        node: Node,
        was_optional: bool,
    ) -> TypeId {
        if was_optional {
            if is_outermost_optional_chain(node) {
                return self.get_optional_type(t, false);
            }
            return self.add_optional_type_marker(t);
        }
        t
    }

    // Go: checker/checker.go:29557 removeMissingType
    pub fn remove_missing_type(&mut self, t: TypeId, is_optional: bool) -> TypeId {
        if self.exact_optional_property_types && is_optional {
            let missing_type = self.missing_type;
            return self.remove_type(t, missing_type);
        }
        t
    }

    // Go: checker/checker.go:29564 removeMissingOrUndefinedType
    pub fn remove_missing_or_undefined_type(&mut self, t: TypeId) -> TypeId {
        if self.exact_optional_property_types {
            let missing_type = self.missing_type;
            return self.remove_type(t, missing_type);
        }
        self.get_type_with_facts(t, TypeFacts::NE_UNDEFINED)
    }

    // Go: checker/checker.go:29571 removeDefinitelyFalsyTypes
    pub fn remove_definitely_falsy_types(&mut self, t: TypeId) -> TypeId {
        self.filter_type(t, &mut |c: &mut Checker, t: TypeId| {
            c.has_type_facts(t, TypeFacts::TRUTHY)
        })
    }

    // Go: checker/checker.go:29575 extractDefinitelyFalsyTypes
    pub fn extract_definitely_falsy_types(&mut self, t: TypeId) -> TypeId {
        self.map_type(t, &mut |c: &mut Checker, t: TypeId| {
            c.get_definitely_falsy_part_of_type(t)
        })
    }

    // Go: checker/checker.go:29579 getDefinitelyFalsyPartOfType
    pub fn get_definitely_falsy_part_of_type(&mut self, t: TypeId) -> TypeId {
        let flags = self.ty(t).flags;
        if flags.intersects(TypeFlags::STRING) {
            return self.empty_string_type;
        }
        if flags.intersects(TypeFlags::NUMBER) {
            return self.zero_type;
        }
        if flags.intersects(TypeFlags::BIG_INT) {
            return self.zero_big_int_type;
        }
        if t == self.regular_false_type
            || t == self.false_type
            || flags.intersects(
                TypeFlags::VOID
                    | TypeFlags::UNDEFINED
                    | TypeFlags::NULL
                    | TypeFlags::ANY_OR_UNKNOWN,
            )
            || flags.intersects(TypeFlags::STRING_LITERAL)
                && self.get_string_literal_value_ref(t).is_empty()
            || flags.intersects(TypeFlags::NUMBER_LITERAL)
                && self.get_number_literal_value(t).0 == 0.0
            || flags.intersects(TypeFlags::BIG_INT_LITERAL) && self.is_zero_big_int(t)
        {
            return t;
        }
        self.never_type
    }

    // Go: checker/checker.go:29597 getConstraintDeclaration
    pub fn get_constraint_declaration(&self, t: TypeId) -> Node {
        let symbol = self.ty(t).symbol;
        if symbol.is_some() {
            for &d in &self.sym(symbol).declarations {
                if is_type_parameter_declaration(d) {
                    let constraint = d.constraint();
                    if constraint.is_some() {
                        return constraint;
                    }
                }
            }
        }
        Node::NIL
    }

    // Go: checker/checker.go:29618 getTemplateLiteralType
    pub fn get_template_literal_type(&mut self, texts: &[String], types: &[TypeId]) -> TypeId {
        let union_index = types
            .iter()
            .position(|&t| {
                self.ty(t)
                    .flags
                    .intersects(TypeFlags::NEVER | TypeFlags::UNION)
            })
            .map_or(-1, |i| i as i32);
        if union_index >= 0 {
            if !self.check_cross_product_union(types) {
                return self.error_type;
            }
            let union_index = union_index as usize;
            let texts_owned = texts.to_vec();
            let types_owned = types.to_vec();
            return self.map_type(types[union_index], &mut |c: &mut Checker, t: TypeId| {
                // Go: core.ReplaceElement(types, unionIndex, t)
                let mut replaced = types_owned.clone();
                replaced[union_index] = t;
                c.get_template_literal_type(&texts_owned, &replaced)
            });
        }
        if types.contains(&self.wildcard_type) {
            return self.wildcard_type;
        }
        // PERF: Go keeps `sb` and a `newTexts` slice of strings. Here all new
        // texts sit back to back in `buf`, and text `k` ends at `ends[k]`.
        // `sb` is the tail of `buf` after the last end. Literal values are
        // written straight into `buf`, and owned texts are made only for a
        // new type. The texts, the key and the created type are the same.
        let mut new_types: SmallVec<[TypeId; 4]> = SmallVec::new();
        let mut ends: SmallVec<[usize; 5]> = SmallVec::new();
        // PERF (tcsplit1): reserve the texts and the direct string literal
        // spans once, so a long joined text is not copied as it grows. The
        // reserve stops at the length limit: past it the type is an error.
        // PERF (perffu1): a span type with one of these flags is not a
        // literal, a template, a generic index type or a placeholder, so
        // `add_spans` returns `string` there. Then only the first text,
        // which is written before the spans, is reserved.
        const STRING_SPAN: TypeFlags = TypeFlags(
            TypeFlags::OBJECT.bits()
                | TypeFlags::UNKNOWN.bits()
                | TypeFlags::ES_SYMBOL_LIKE.bits()
                | TypeFlags::VOID.bits()
                | TypeFlags::NON_PRIMITIVE.bits(),
        );
        let mut capacity: usize = texts.iter().map(String::len).sum();
        for &t in types {
            let ty = self.ty(t);
            if ty.flags.intersects(STRING_SPAN) {
                capacity = texts[0].len();
                break;
            }
            if ty.flags.intersects(TypeFlags::STRING_LITERAL)
                && let Some(LiteralValue::String(s)) = &ty.as_literal_type().value
            {
                capacity += s.len();
            }
        }
        let mut buf = String::with_capacity(capacity.min(MAX_TEMPLATE_LITERAL_TYPE_LENGTH + 1));
        buf.push_str(&texts[0]);
        let mut text_length = 0usize; // combined length of the segments already moved into newTexts
        let mut too_large = false;
        // PORT: Go uses a recursive closure `addSpans` that captures `sb`,
        // `newTypes`, `newTexts`, `textLength` and `tooLarge`. Here the
        // captured state is passed explicitly. `text_length` is in Go bytes.
        #[allow(clippy::too_many_arguments)]
        fn add_spans(
            c: &mut Checker,
            texts: &[String],
            types: &[TypeId],
            buf: &mut String,
            ends: &mut SmallVec<[usize; 5]>,
            new_types: &mut SmallVec<[TypeId; 4]>,
            text_length: &mut usize,
            too_large: &mut bool,
        ) -> bool {
            for (i, &t) in types.iter().enumerate() {
                let flags = c.ty(t).flags;
                if flags.intersects(TypeFlags::LITERAL | TypeFlags::NULL | TypeFlags::UNDEFINED) {
                    c.get_template_string_for_type(t, buf);
                    buf.push_str(&texts[i + 1]);
                } else if flags.intersects(TypeFlags::TEMPLATE_LITERAL) {
                    let (inner_texts, inner_types) = {
                        let tl = c.ty(t).as_template_literal_type();
                        (tl.texts.clone(), tl.types.clone())
                    };
                    buf.push_str(&inner_texts[0]);
                    if !add_spans(
                        c,
                        &inner_texts,
                        &inner_types,
                        buf,
                        ends,
                        new_types,
                        text_length,
                        too_large,
                    ) {
                        return false;
                    }
                    buf.push_str(&texts[i + 1]);
                } else if c.is_generic_index_type(t) || c.is_pattern_literal_placeholder_type(t) {
                    new_types.push(t);
                    // Go: `textLength += sb.Len()`, the Go bytes of the segment
                    // before `CombineSurrogatePairs`.
                    let sb_start = ends.last().copied().unwrap_or(0);
                    *text_length += go_len(&buf[sb_start..]);
                    end_template_text(buf, ends);
                    buf.push_str(&texts[i + 1]);
                } else {
                    return false;
                }
                // PORT: Go `sb.Len()` is the Go bytes of the tail of `buf`. The
                // port form is never shorter than its Go bytes, so the exact
                // count is only needed when the port form is over the limit.
                let sb_start = ends.last().copied().unwrap_or(0);
                if (*text_length + (buf.len() - sb_start) > MAX_TEMPLATE_LITERAL_TYPE_LENGTH
                    && *text_length + go_len(&buf[sb_start..]) > MAX_TEMPLATE_LITERAL_TYPE_LENGTH)
                    || new_types.len() > MAX_TEMPLATE_LITERAL_TYPE_SPANS
                {
                    *too_large = true;
                    return false;
                }
            }
            true
        }
        if !add_spans(
            self,
            texts,
            types,
            &mut buf,
            &mut ends,
            &mut new_types,
            &mut text_length,
            &mut too_large,
        ) {
            if too_large {
                let current_node = self.current_node;
                self.error(
                    current_node,
                    diag::Type_instantiation_is_excessively_deep_and_possibly_infinite,
                    args![],
                );
                return self.error_type;
            }
            return self.string_type;
        }
        // PORT: Go joins the texts by bytes. `go_value` gives the port form
        // of the joined Go bytes (see `scanner_util::GO_STRING_MARKER`).
        if new_types.is_empty() {
            // PERF (tcsplit1): without a marker, `go_value` and
            // `combine_surrogate_pairs_cow` return `buf` as is, so one scan
            // is enough and `buf` moves into the type.
            if !crate::scanner_util::contains_go_string_marker(&buf) {
                return self.get_string_literal_type_owned(buf);
            }
            let value = go_value(&buf);
            let s = combine_surrogate_pairs_cow(&value);
            return self.get_string_literal_type(&s);
        }
        end_template_text(&mut buf, &mut ends);
        // Every new text is empty exactly when `buf` is.
        if buf.is_empty() {
            if new_types
                .iter()
                .all(|&t| self.ty(t).flags.intersects(TypeFlags::STRING))
            {
                return self.string_type;
            }
            // Normalize `${Mapping<xxx>}` into Mapping<xxx>
            if new_types.len() == 1 && self.is_pattern_literal_type(new_types[0]) {
                return new_types[0];
            }
        }
        let key = get_template_type_key_of_buffer(&buf, &ends, &new_types);
        let mut t = self
            .template_literal_types
            .get(&key)
            .copied()
            .unwrap_or_default();
        if t.is_nil() {
            let mut start = 0;
            let new_texts: Vec<String> = ends
                .iter()
                .map(|&end| {
                    let text = buf[start..end].to_string();
                    start = end;
                    text
                })
                .collect();
            debug_assert_eq!(key, get_template_type_key(&new_texts, &new_types));
            t = self.new_template_literal_type(&new_texts, &new_types);
            self.template_literal_types.insert(key, t);
        }
        t
    }

    // Go: checker/checker.go:29697 getTemplateStringForType
    // PORT: appends the string to `out` instead of returning it, so
    // `get_template_literal_type` writes straight into its buffer. A string
    // literal is copied as is, which is what `any_to_string` returns for it.
    pub fn get_template_string_for_type(&self, t: TypeId, out: &mut String) {
        let flags = self.ty(t).flags;
        if flags.intersects(
            TypeFlags::STRING_LITERAL
                | TypeFlags::NUMBER_LITERAL
                | TypeFlags::BOOLEAN_LITERAL
                | TypeFlags::BIG_INT_LITERAL,
        ) {
            // PORT: Go `evaluator.AnyToString(value)`; literal types of these kinds always carry a value.
            let value = self
                .ty(t)
                .as_literal_type()
                .value
                .as_ref()
                .expect("literal type without value");
            match value {
                LiteralValue::String(s) => out.push_str(s),
                _ => out.push_str(&any_to_string(value)),
            }
            return;
        }
        if flags.intersects(TypeFlags::NULLABLE) {
            out.push_str(&self.ty(t).as_intrinsic_type().intrinsic_name);
        }
    }

    // Go: checker/checker.go:29707 getStringMappingType
    pub fn get_string_mapping_type(&mut self, symbol: SymbolId, t: TypeId) -> TypeId {
        let flags = self.ty(t).flags;
        if flags.intersects(TypeFlags::UNION | TypeFlags::NEVER) {
            return self.map_type(t, &mut |c: &mut Checker, t: TypeId| {
                c.get_string_mapping_type(symbol, t)
            });
        }
        if flags.intersects(TypeFlags::STRING_LITERAL) {
            let value = self.get_string_literal_value(t);
            let mapped = self.apply_string_mapping(symbol, &value);
            return self.get_string_literal_type(&mapped);
        }
        if flags.intersects(TypeFlags::TEMPLATE_LITERAL) {
            let texts = self.ty(t).as_template_literal_type().texts.clone();
            let types = self.ty(t).as_template_literal_type().types.clone();
            let (new_texts, new_types) = self.apply_template_string_mapping(symbol, &texts, &types);
            return self.get_template_literal_type(&new_texts, &new_types);
        }
        if flags.intersects(TypeFlags::STRING_MAPPING) && symbol == self.ty(t).symbol {
            return t;
        }
        if flags.intersects(TypeFlags::ANY | TypeFlags::STRING | TypeFlags::STRING_MAPPING)
            || self.is_generic_index_type(t)
        {
            return self.get_string_mapping_type_for_generic_type(symbol, t);
        }
        if self.is_pattern_literal_placeholder_type(t) {
            let template = self.get_template_literal_type(&[String::new(), String::new()], &[t]);
            return self.get_string_mapping_type_for_generic_type(symbol, template);
        }
        t
    }

    // Go: checker/checker.go:29726 applyStringMapping
    // PORT: Go package function; it reads the symbol name, so it is a `Checker` method.
    pub fn apply_string_mapping(&self, symbol: SymbolId, str: &str) -> String {
        let kind = INTRINSIC_TYPE_KINDS
            .get(self.sym(symbol).name.as_str())
            .copied()
            .unwrap_or(IntrinsicTypeKind::UNKNOWN);
        match kind {
            IntrinsicTypeKind::UPPERCASE => to_upper_js(str),
            IntrinsicTypeKind::LOWERCASE => to_lower_js(str),
            IntrinsicTypeKind::CAPITALIZE => {
                let (_, size) = decode_js_string_rune(str);
                let size = size as usize;
                to_upper_js(&str[..size]) + &str[size..]
            }
            IntrinsicTypeKind::UNCAPITALIZE => {
                let (_, size) = decode_js_string_rune(str);
                let size = size as usize;
                to_lower_js(&str[..size]) + &str[size..]
            }
            _ => str.to_string(),
        }
    }

    // Go: checker/checker.go:29742 applyTemplateStringMapping
    pub fn apply_template_string_mapping(
        &mut self,
        symbol: SymbolId,
        texts: &[String],
        types: &[TypeId],
    ) -> (Vec<String>, Vec<TypeId>) {
        let kind = INTRINSIC_TYPE_KINDS
            .get(self.sym(symbol).name.as_str())
            .copied()
            .unwrap_or(IntrinsicTypeKind::UNKNOWN);
        match kind {
            IntrinsicTypeKind::UPPERCASE | IntrinsicTypeKind::LOWERCASE => {
                let new_texts: Vec<String> = texts
                    .iter()
                    .map(|t| self.apply_string_mapping(symbol, t))
                    .collect();
                let mut new_types = Vec::with_capacity(types.len());
                for &t in types {
                    new_types.push(self.get_string_mapping_type(symbol, t));
                }
                (new_texts, new_types)
            }
            IntrinsicTypeKind::CAPITALIZE | IntrinsicTypeKind::UNCAPITALIZE => {
                if !texts[0].is_empty() {
                    let mut new_texts = texts.to_vec();
                    let first = self.apply_string_mapping(symbol, &new_texts[0]);
                    new_texts[0] = first;
                    return (new_texts, types.to_vec());
                }
                let mut new_types = types.to_vec();
                let first = self.get_string_mapping_type(symbol, new_types[0]);
                new_types[0] = first;
                (texts.to_vec(), new_types)
            }
            _ => (texts.to_vec(), types.to_vec()),
        }
    }

    // Go: checker/checker.go:29760 getStringMappingTypeForGenericType
    pub fn get_string_mapping_type_for_generic_type(
        &mut self,
        symbol: SymbolId,
        t: TypeId,
    ) -> TypeId {
        let key = StringMappingKey { s: symbol, t };
        let mut result = self
            .string_mapping_types
            .get(&key)
            .copied()
            .unwrap_or_default();
        if result.is_nil() {
            result = self.new_string_mapping_type(symbol, t);
            self.string_mapping_types.insert(key, result);
        }
        result
    }

    // Go: checker/checker.go:29775 substituteIndexedMappedType
    // Given an indexed access on a mapped type of the form { [P in K]: E }[X], return an instantiation of E where P is
    // replaced with X. Since this simplification doesn't account for mapped type modifiers, add 'undefined' to the
    // resulting type if the mapped type includes a '?' modifier or if the modifiers type indicates that some properties
    // are optional. If the modifiers type is generic, conservatively estimate optionality by recursively looking for
    // mapped types that include '?' modifiers.
    pub fn substitute_indexed_mapped_type(&mut self, object_type: TypeId, index: TypeId) -> TypeId {
        let type_parameter = self.get_type_parameter_from_mapped_type(object_type);
        let mapper = self.new_simple_type_mapper(type_parameter, index);
        let object_mapper = self.ty(object_type).as_object_type().mapper;
        let template_mapper = self.combine_type_mappers(object_mapper, mapper);
        let mut target = self.ty(object_type).as_object_type().target;
        if target.is_nil() {
            target = object_type;
        }
        let template_type = self.get_template_type_from_mapped_type(target);
        let instantiated_template_type = self.instantiate_type(template_type, template_mapper);
        let mut is_optional = self.get_mapped_type_optionality(object_type) > 0;
        if !is_optional {
            if self.is_generic_type(object_type) {
                let modifiers_type = self.get_modifiers_type_from_mapped_type(object_type);
                is_optional = self.get_combined_mapped_type_optionality(modifiers_type) > 0;
            } else {
                is_optional = self.could_access_optional_property(object_type, index);
            }
        }
        self.add_optionality_ex(
            instantiated_template_type,
            true, /*isProperty*/
            is_optional,
        )
    }

    // Go: checker/checker.go:29791 couldAccessOptionalProperty
    // Return true if an indexed access with the given object and index types could access an optional property.
    pub fn could_access_optional_property(
        &mut self,
        object_type: TypeId,
        index_type: TypeId,
    ) -> bool {
        let index_constraint = self.get_base_constraint_of_type(index_type);
        if index_constraint.is_nil() {
            return false;
        }
        let props = self.get_properties_of_type(object_type);
        for p in props {
            if self.sym(p).flags.intersects(SymbolFlags::OPTIONAL) {
                let literal = self.get_literal_type_from_property(
                    p,
                    TypeFlags::STRING_OR_NUMBER_LITERAL_OR_UNIQUE,
                    false,
                );
                if self.is_type_assignable_to(literal, index_constraint) {
                    return true;
                }
            }
        }
        false
    }

    // Go: checker/checker.go:29798 getTypeOfPropertyOrIndexSignatureOfType
    pub fn get_type_of_property_or_index_signature_of_type(
        &mut self,
        t: TypeId,
        name: &str,
    ) -> TypeId {
        let prop_type = self.get_type_of_property_of_type(t, name);
        if prop_type.is_some() {
            return prop_type;
        }
        let index_info = self.get_applicable_index_info_for_name(t, name);
        if index_info.is_some() {
            let value_type = self.index_info(index_info).value_type;
            return self.add_optionality_ex(
                value_type, true, /*isProperty*/
                true, /*isOptional*/
            );
        }
        TypeId::NIL
    }
}

impl Checker {
    // Go: checker/checker.go:29827 getContextualType
    /**
     * Whoa! Do you really want to use this function?
     *
     * Unless you're trying to get the *non-apparent* type for a
     * value-literal type or you're authoring relevant portions of this algorithm,
     * you probably meant to use 'getApparentTypeOfContextualType'.
     * Otherwise this may not be very useful.
     *
     * In cases where you *are* working on this function, you should understand
     * when it is appropriate to use 'getContextualType' and 'getApparentTypeOfContextualType'.
     *
     *   - Use 'getContextualType' when you are simply going to propagate the result to the expression.
     *   - Use 'getApparentTypeOfContextualType' when you're going to need the members of the type.
     *
     * @param node the expression whose contextual type will be returned.
     * @returns the contextual type of an expression.
     */
    pub fn get_contextual_type(&mut self, node: Node, context_flags: ContextFlags) -> TypeId {
        if node.flags().intersects(NodeFlags::IN_WITH_STATEMENT) {
            // We cannot answer semantic questions within a with block, do not proceed any further
            return TypeId::NIL;
        }
        // Cached contextual types are obtained with no ContextFlags, so we can only consult them for
        // requests with no ContextFlags.
        let index = self.find_contextual_node(
            node,
            context_flags == ContextFlags::NONE, /*includeCaches*/
        );
        if index >= 0 {
            return self.contextual_infos[index as usize].t;
        }
        let parent = node.parent();
        match parent.kind() {
            SyntaxKind::VariableDeclaration
            | SyntaxKind::Parameter
            | SyntaxKind::PropertyDeclaration
            | SyntaxKind::PropertySignature
            | SyntaxKind::BindingElement => {
                return self.get_contextual_type_for_initializer_expression(node, context_flags);
            }
            SyntaxKind::ArrowFunction | SyntaxKind::ReturnStatement => {
                return self.get_contextual_type_for_return_expression(node, context_flags);
            }
            SyntaxKind::YieldExpression => {
                return self.get_contextual_type_for_yield_operand(parent, context_flags);
            }
            SyntaxKind::AwaitExpression => {
                return self.get_contextual_type_for_await_operand(parent, context_flags);
            }
            SyntaxKind::CallExpression | SyntaxKind::NewExpression => {
                return self.get_contextual_type_for_argument(parent, node);
            }
            SyntaxKind::Decorator => {
                return self.get_contextual_type_for_decorator(parent);
            }
            SyntaxKind::TypeAssertionExpression | SyntaxKind::AsExpression => {
                if is_const_assertion(parent) {
                    return self.get_contextual_type(parent, context_flags);
                }
                return self.get_type_from_type_node(parent.type_());
            }
            SyntaxKind::BinaryExpression => {
                return self.get_contextual_type_for_binary_operand(node, context_flags);
            }
            SyntaxKind::PropertyAssignment | SyntaxKind::ShorthandPropertyAssignment => {
                return self.get_contextual_type_for_object_literal_element(parent, context_flags);
            }
            SyntaxKind::SpreadAssignment => {
                return self.get_contextual_type(parent.parent(), context_flags);
            }
            SyntaxKind::ArrayLiteralExpression => {
                let t = self.get_apparent_type_of_contextual_type(parent, context_flags);
                let element_index = index_of_node(parent.elements(), node);
                if element_index < 0 {
                    return TypeId::NIL;
                }
                let (first_spread_index, last_spread_index) = self.get_spread_indices(parent);
                let length = parent.elements().len() as i32;
                return self.get_contextual_type_for_element_expression(
                    t,
                    element_index,
                    length,
                    first_spread_index,
                    last_spread_index,
                );
            }
            SyntaxKind::ConditionalExpression => {
                return self.get_contextual_type_for_conditional_operand(node, context_flags);
            }
            SyntaxKind::TemplateSpan => {
                return self.get_contextual_type_for_substitution_expression(parent.parent(), node);
            }
            SyntaxKind::ParenthesizedExpression => {
                return self.get_contextual_type(parent, context_flags);
            }
            SyntaxKind::NonNullExpression => {
                return self.get_contextual_type(parent, context_flags);
            }
            SyntaxKind::SatisfiesExpression => {
                return self.get_type_from_type_node(parent.type_());
            }
            SyntaxKind::ExportAssignment => {
                return self.try_get_type_from_type_node(parent);
            }
            SyntaxKind::JsxExpression => {
                return self.get_contextual_type_for_jsx_expression(parent, context_flags);
            }
            SyntaxKind::JsxAttribute | SyntaxKind::JsxSpreadAttribute => {
                return self.get_contextual_type_for_jsx_attribute(parent, context_flags);
            }
            SyntaxKind::JsxOpeningElement | SyntaxKind::JsxSelfClosingElement => {
                return self.get_contextual_jsx_element_attributes_type(parent, context_flags);
            }
            SyntaxKind::ImportAttribute => {
                return self.get_contextual_import_attribute_type(parent);
            }
            _ => {}
        }
        TypeId::NIL
    }

    // Go: checker/checker.go:29907 getContextualTypeForInitializerExpression
    // In a variable, parameter or property declaration with a type annotation,
    // the contextual type of an initializer expression is the type of the variable, parameter or property.
    //
    // Otherwise, in a parameter declaration of a contextually typed function expression,
    // the contextual type of an initializer expression is the contextual type of the parameter.
    //
    // Otherwise, in a variable or parameter declaration with a binding pattern name,
    // the contextual type of an initializer expression is the type implied by the binding pattern.
    //
    // Otherwise, in a binding pattern inside a variable or parameter declaration,
    // the contextual type of an initializer expression is the type annotation of the containing declaration, if present.
    pub fn get_contextual_type_for_initializer_expression(
        &mut self,
        node: Node,
        context_flags: ContextFlags,
    ) -> TypeId {
        let declaration = node.parent();
        let initializer = declaration.initializer();
        if node == initializer {
            let result =
                self.get_contextual_type_for_variable_like_declaration(declaration, context_flags);
            if result.is_some() {
                return result;
            }
            if !context_flags.intersects(ContextFlags::SKIP_BINDING_PATTERNS)
                && is_binding_pattern(declaration.name())
                && !declaration.name().elements().is_empty()
            {
                return self.get_type_from_binding_pattern(
                    declaration.name(),
                    true,  /*includePatternInType*/
                    false, /*reportErrors*/
                );
            }
        }
        TypeId::NIL
    }

    // Go: checker/checker.go:29922 getContextualTypeForVariableLikeDeclaration
    pub fn get_contextual_type_for_variable_like_declaration(
        &mut self,
        declaration: Node,
        context_flags: ContextFlags,
    ) -> TypeId {
        let type_node = declaration.type_();
        if type_node.is_some() {
            return self.get_type_from_type_node(type_node);
        }
        match declaration.kind() {
            SyntaxKind::Parameter => {
                return self.get_contextually_typed_parameter_type(declaration);
            }
            SyntaxKind::BindingElement => {
                return self.get_contextual_type_for_binding_element(declaration, context_flags);
            }
            SyntaxKind::PropertyDeclaration => {
                if is_static(declaration) {
                    return self.get_contextual_type_for_static_property_declaration(
                        declaration,
                        context_flags,
                    );
                }
            }
            _ => {}
        }
        // By default, do nothing and return nil - only the above cases have context implied by a parent
        TypeId::NIL
    }

    // Go: checker/checker.go:29942 getContextuallyTypedParameterType
    // Return contextual type of parameter or undefined if no contextual type is available
    pub fn get_contextually_typed_parameter_type(&mut self, parameter: Node) -> TypeId {
        let fn_ = parameter.parent();
        if !self.is_context_sensitive_function_or_object_literal_method(fn_) {
            return TypeId::NIL;
        }
        let iife = get_immediately_invoked_function_expression(fn_);
        if iife.is_some() {
            let args = self.get_effective_call_arguments(iife);
            let index_of_parameter = fn_
                .parameters()
                .iter()
                .position(|p| p == parameter)
                .map_or(-1, |i| i as i32);
            if has_dot_dot_dot_token(parameter) {
                let any_type = self.any_type;
                let args: Vec<Node> = args.iter().collect();
                return self.get_spread_argument_type(
                    &args,
                    index_of_parameter,
                    args.len() as i32,
                    any_type,
                    InferenceContextId::NIL, /*context*/
                    CheckMode::NORMAL,
                );
            }
            let cached = self.signature_links.get(iife).resolved_signature;
            let any_signature = self.any_signature;
            self.signature_links.get(iife).resolved_signature = any_signature;
            let t;
            if index_of_parameter < args.len() as i32 {
                let arg_type = self.check_expression(args.get(index_of_parameter as usize));
                t = self.get_widened_literal_type(arg_type);
            } else if parameter.initializer().is_some() {
                t = TypeId::NIL;
            } else {
                t = self.undefined_widening_type;
            }
            self.signature_links.get(iife).resolved_signature = cached;
            return t;
        }
        let contextual_signature = self.get_contextual_signature(fn_);
        if contextual_signature.is_some() {
            let parameters = fn_.parameters();
            let index = parameters
                .iter()
                .position(|p| p == parameter)
                .map_or(-1, |i| i as i32)
                - if get_this_parameter(fn_).is_some() {
                    1
                } else {
                    0
                };
            if has_dot_dot_dot_token(parameter)
                && parameters.last().unwrap_or(Node::NIL) == parameter
            {
                return self.get_rest_type_at_position(contextual_signature, index, false);
            }
            return self.try_get_type_at_position(contextual_signature, index);
        }
        TypeId::NIL
    }

    // Go: checker/checker.go:29980 isContextSensitiveFunctionOrObjectLiteralMethod
    pub fn is_context_sensitive_function_or_object_literal_method(&mut self, fn_: Node) -> bool {
        (is_function_expression_or_arrow_function(fn_) || is_object_literal_method(fn_))
            && self.is_context_sensitive_function_like_declaration(fn_)
    }

    // Go: checker/checker.go:29984 getSpreadArgumentType
    pub fn get_spread_argument_type(
        &mut self,
        args: &[Node],
        index: i32,
        arg_count: i32,
        rest_type: TypeId,
        context: InferenceContextId,
        check_mode: CheckMode,
    ) -> TypeId {
        let in_const_context = self.is_const_type_variable(rest_type, 0);
        if arg_count > 0 && index >= arg_count - 1 {
            let mut arg = args[(arg_count - 1) as usize];
            if is_spread_argument(arg) {
                // We are inferring from a spread expression in the last argument position, i.e. both the parameter
                // and the argument are ...x forms.
                let spread_type;
                if is_synthetic_expression(arg) {
                    spread_type = synthetic_expression_type(arg);
                } else {
                    spread_type = self.check_expression_with_contextual_type(
                        arg.expression(),
                        rest_type,
                        context,
                        check_mode,
                    );
                }
                if self.is_array_like_type(spread_type) {
                    return self.get_mutable_array_or_tuple_type(spread_type);
                }
                if is_spread_element(arg) {
                    arg = arg.expression();
                }
                let undefined_type = self.undefined_type;
                let element_type = self.check_iterated_type_or_element_type(
                    IterationUse::SPREAD,
                    spread_type,
                    undefined_type,
                    arg,
                );
                return self.create_array_type_ex(element_type, in_const_context);
            }
        }
        let mut types: Vec<TypeId> = Vec::new();
        let mut infos: Vec<TupleElementInfo> = Vec::new();
        let mut i = index;
        while i < arg_count {
            let arg = args[i as usize];
            let t;
            let mut info = TupleElementInfo::default();
            if is_spread_argument(arg) {
                let spread_type;
                if is_synthetic_expression(arg) {
                    spread_type = synthetic_expression_type(arg);
                } else {
                    spread_type = self.check_expression(arg.expression());
                }
                if self.is_array_like_type(spread_type) {
                    t = spread_type;
                    info.flags = ElementFlags::VARIADIC;
                } else {
                    let undefined_type = self.undefined_type;
                    if is_spread_element(arg) {
                        t = self.check_iterated_type_or_element_type(
                            IterationUse::SPREAD,
                            spread_type,
                            undefined_type,
                            arg.expression(),
                        );
                    } else {
                        t = self.check_iterated_type_or_element_type(
                            IterationUse::SPREAD,
                            spread_type,
                            undefined_type,
                            arg,
                        );
                    }
                    info.flags = ElementFlags::REST;
                }
            } else {
                let contextual_type;
                if self.is_tuple_type(rest_type) {
                    let ct = self.get_contextual_type_for_element_expression(
                        rest_type,
                        i - index,
                        arg_count - index,
                        -1,
                        -1,
                    );
                    contextual_type = if ct.is_some() { ct } else { self.unknown_type };
                } else {
                    let index_type = self.get_number_literal_type(Number((i - index) as f64));
                    contextual_type = self.get_indexed_access_type_ex(
                        rest_type,
                        index_type,
                        AccessFlags::CONTEXTUAL,
                        Node::NIL,
                        None,
                    );
                }
                let arg_type = self.check_expression_with_contextual_type(
                    arg,
                    contextual_type,
                    context,
                    check_mode,
                );
                let has_primitive_contextual_type = in_const_context
                    || self.maybe_type_of_kind(
                        contextual_type,
                        TypeFlags::PRIMITIVE
                            | TypeFlags::INDEX
                            | TypeFlags::TEMPLATE_LITERAL
                            | TypeFlags::STRING_MAPPING,
                    );
                if has_primitive_contextual_type {
                    t = self.get_regular_type_of_literal_type(arg_type);
                } else {
                    t = self.get_widened_literal_type(arg_type);
                }
                info.flags = ElementFlags::REQUIRED;
            }
            if is_synthetic_expression(arg) && arg.tuple_name_source().is_some() {
                info.labeled_declaration = arg.tuple_name_source();
            }
            types.push(t);
            infos.push(info);
            i += 1;
        }
        let readonly = in_const_context
            && !self.some_type(rest_type, &mut |c: &mut Checker, t: TypeId| {
                c.is_mutable_array_like_type(t)
            });
        self.create_tuple_type_ex(&types, &infos, readonly)
    }

    // Go: checker/checker.go:30055 getMutableArrayOrTupleType
    pub fn get_mutable_array_or_tuple_type(&mut self, t: TypeId) -> TypeId {
        if self.ty(t).flags.intersects(TypeFlags::UNION) {
            return self.map_type(t, &mut |c: &mut Checker, t: TypeId| {
                c.get_mutable_array_or_tuple_type(t)
            });
        }
        if self.ty(t).flags.intersects(TypeFlags::ANY) {
            return t;
        }
        let base = self.get_base_constraint_or_type(t);
        if self.is_mutable_array_or_tuple(base) {
            return t;
        }
        if self.is_tuple_type(t) {
            let element_types = self.get_element_types(t);
            let element_infos = self.target_tuple_type(t).element_infos().to_vec();
            return self.create_tuple_type_ex(
                &element_types,
                &element_infos,
                false, /*readonly*/
            );
        }
        self.create_tuple_type_ex(
            &[t],
            &[TupleElementInfo {
                flags: ElementFlags::VARIADIC,
                labeled_declaration: Node::NIL,
            }],
            false,
        )
    }

    // Go: checker/checker.go:30067 getContextualTypeForBindingElement
    pub fn get_contextual_type_for_binding_element(
        &mut self,
        declaration: Node,
        context_flags: ContextFlags,
    ) -> TypeId {
        let name = declaration.property_name_or_name();
        if is_binding_pattern(name) || is_computed_non_literal_name(name) {
            return TypeId::NIL;
        }
        let parent = declaration.parent().parent();
        let mut parent_type =
            self.get_contextual_type_for_variable_like_declaration(parent, context_flags);
        if parent_type.is_nil() {
            if !is_binding_element(parent) && parent.initializer().is_some() {
                let check_mode = if has_dot_dot_dot_token(declaration) {
                    CheckMode::REST_BINDING_ELEMENT
                } else {
                    CheckMode::NORMAL
                };
                parent_type = self.check_declaration_initializer(parent, check_mode, TypeId::NIL);
            }
        }
        if parent_type.is_nil() {
            return TypeId::NIL;
        }
        if is_array_binding_pattern(parent.name()) {
            let index = declaration
                .parent()
                .elements()
                .to_vec()
                .iter()
                .position(|&e| e == declaration)
                .map_or(-1, |i| i as i32);
            if index < 0 {
                return TypeId::NIL;
            }
            return self.get_contextual_type_for_element_expression(parent_type, index, -1, -1, -1);
        }
        let name_type = self.get_literal_type_from_property_name(name);
        if self.is_type_usable_as_property_name(name_type) {
            let prop_name = self.get_property_name_from_type(name_type);
            return self.get_type_of_property_of_type(parent_type, &prop_name);
        }
        TypeId::NIL
    }

    // Go: checker/checker.go:30096 getContextualTypeForStaticPropertyDeclaration
    pub fn get_contextual_type_for_static_property_declaration(
        &mut self,
        declaration: Node,
        context_flags: ContextFlags,
    ) -> TypeId {
        if is_expression(declaration.parent()) {
            // Don't contextually type a static property by its own class, its type might still be in-progress and that would cause spurious circularities
            // (ts#64525, Go N' checker.go:30163)
            let parent_type = self.get_contextual_type(declaration.parent(), context_flags);
            if parent_type.is_some() && {
                let class_symbol = self.get_symbol_of_declaration(declaration.parent());
                self.ty(parent_type).symbol != class_symbol
            } {
                let symbol = self.get_symbol_of_declaration(declaration);
                let name = self.sym(symbol).name.clone();
                return self.get_type_of_property_of_contextual_type(parent_type, &name);
            }
        }
        TypeId::NIL
    }

    // Go: checker/checker.go:30105 getContextualTypeForReturnExpression
    pub fn get_contextual_type_for_return_expression(
        &mut self,
        node: Node,
        context_flags: ContextFlags,
    ) -> TypeId {
        let fn_ = get_containing_function(node);
        if fn_.is_some() {
            let mut contextual_return_type = self.get_contextual_return_type(fn_, context_flags);
            if contextual_return_type.is_some() {
                let function_flags = get_function_flags(fn_);
                if function_flags.intersects(FunctionFlags::GENERATOR) {
                    let is_async_generator = function_flags.intersects(FunctionFlags::ASYNC);
                    if self
                        .ty(contextual_return_type)
                        .flags
                        .intersects(TypeFlags::UNION)
                    {
                        contextual_return_type = self.filter_type(
                            contextual_return_type,
                            &mut |c: &mut Checker, t: TypeId| {
                                c.get_iteration_type_of_generator_function_return_type(
                                    IterationTypeKind::RETURN,
                                    t,
                                    is_async_generator,
                                )
                                .is_some()
                            },
                        );
                    }
                    let iteration_return_type = self
                        .get_iteration_type_of_generator_function_return_type(
                            IterationTypeKind::RETURN,
                            contextual_return_type,
                            function_flags.intersects(FunctionFlags::ASYNC),
                        );
                    if iteration_return_type.is_nil() {
                        return TypeId::NIL;
                    }
                    contextual_return_type = iteration_return_type;
                    // falls through to unwrap Promise for AsyncGenerators
                }
                if function_flags.intersects(FunctionFlags::ASYNC) {
                    // Get the awaited type without the `Awaited<T>` alias
                    let contextual_awaited_type = self
                        .map_type(contextual_return_type, &mut |c: &mut Checker, t: TypeId| {
                            c.get_awaited_type_no_alias(t)
                        });
                    if contextual_awaited_type.is_nil() {
                        return TypeId::NIL;
                    }
                    let promise_like = self.create_promise_like_type(contextual_awaited_type);
                    return self.get_union_type(&[contextual_awaited_type, promise_like]);
                }
                // Regular function or Generator function
                return contextual_return_type;
            }
        }
        TypeId::NIL
    }

    // Go: checker/checker.go:30140 getContextualIterationType
    pub fn get_contextual_iteration_type(
        &mut self,
        kind: IterationTypeKind,
        function_decl: Node,
    ) -> TypeId {
        let is_async = get_function_flags(function_decl).intersects(FunctionFlags::ASYNC);
        let contextual_return_type =
            self.get_contextual_return_type(function_decl, ContextFlags::NONE);
        if contextual_return_type.is_some() {
            return self.get_iteration_type_of_generator_function_return_type(
                kind,
                contextual_return_type,
                is_async,
            );
        }
        TypeId::NIL
    }

    // Go: checker/checker.go:30149 getContextualReturnType
    pub fn get_contextual_return_type(
        &mut self,
        function_decl: Node,
        context_flags: ContextFlags,
    ) -> TypeId {
        // If the containing function has a return type annotation, is a constructor, or is a get accessor whose
        // corresponding set accessor has a type annotation, return statements in the function are contextually typed
        let return_type = self.get_return_type_from_annotation(function_decl);
        if return_type.is_some() {
            return return_type;
        }
        // Otherwise, if the containing function is contextually typed by a function type with exactly one call signature
        // and that call signature is non-generic, return statements are contextually typed by the return type of the signature
        let signature = self.get_contextual_signature_for_function_like_declaration(function_decl);
        if signature.is_some() && !self.is_resolving_return_type_of_signature(signature) {
            let return_type = self.get_return_type_of_signature(signature);
            let function_flags = get_function_flags(function_decl);
            if function_flags.intersects(FunctionFlags::GENERATOR) {
                return self.filter_type(return_type, &mut |c: &mut Checker, t: TypeId| {
                    c.ty(t).flags.intersects(
                        TypeFlags::ANY_OR_UNKNOWN
                            | TypeFlags::VOID
                            | TypeFlags::INSTANTIABLE_NON_PRIMITIVE,
                    ) || c.check_generator_instantiation_assignability_to_return_type(
                        t,
                        function_flags,
                        Node::NIL, /*errorNode*/
                    )
                });
            }
            if function_flags.intersects(FunctionFlags::ASYNC) {
                return self.filter_type(return_type, &mut |c: &mut Checker, t: TypeId| {
                    c.ty(t).flags.intersects(
                        TypeFlags::ANY_OR_UNKNOWN
                            | TypeFlags::VOID
                            | TypeFlags::INSTANTIABLE_NON_PRIMITIVE,
                    ) || c.get_awaited_type_of_promise(t).is_some()
                });
            }
            return return_type;
        }
        let iife = get_immediately_invoked_function_expression(function_decl);
        if iife.is_some() {
            return self.get_contextual_type(iife, context_flags);
        }
        TypeId::NIL
    }

    // Go: checker/checker.go:30181 checkGeneratorInstantiationAssignabilityToReturnType
    pub fn check_generator_instantiation_assignability_to_return_type(
        &mut self,
        return_type: TypeId,
        function_flags: FunctionFlags,
        error_node: Node,
    ) -> bool {
        // Naively, one could check that Generator<any, any, any> is assignable to the return type annotation.
        // However, that would not catch the error in the following case.
        //
        //    interface BadGenerator extends Iterable<number>, Iterator<string> { }
        //    function* g(): BadGenerator { } // Iterable and Iterator have different types!
        //
        let is_async = function_flags.intersects(FunctionFlags::ASYNC);
        let mut generator_yield_type = self.get_iteration_type_of_generator_function_return_type(
            IterationTypeKind::YIELD,
            return_type,
            is_async,
        );
        if generator_yield_type.is_nil() {
            generator_yield_type = self.any_type;
        }
        let mut generator_return_type = self.get_iteration_type_of_generator_function_return_type(
            IterationTypeKind::RETURN,
            return_type,
            is_async,
        );
        if generator_return_type.is_nil() {
            generator_return_type = generator_yield_type;
        }
        let mut generator_next_type = self.get_iteration_type_of_generator_function_return_type(
            IterationTypeKind::NEXT,
            return_type,
            is_async,
        );
        if generator_next_type.is_nil() {
            generator_next_type = self.unknown_type;
        }
        let generator_instantiation = self.create_generator_type(
            generator_yield_type,
            generator_return_type,
            generator_next_type,
            is_async,
        );
        self.check_type_assignable_to(generator_instantiation, return_type, error_node, None)
    }

    // Go: checker/checker.go:30195 getContextualSignatureForFunctionLikeDeclaration
    pub fn get_contextual_signature_for_function_like_declaration(
        &mut self,
        node: Node,
    ) -> SignatureId {
        // Only function expressions, arrow functions, and object literal methods are contextually typed.
        if is_function_expression_or_arrow_function(node) || is_object_literal_method(node) {
            return self.get_contextual_signature(node);
        }
        SignatureId::NIL
    }

    // Go: checker/checker.go:30203 getContextualTypeForYieldOperand
    pub fn get_contextual_type_for_yield_operand(
        &mut self,
        node: Node,
        context_flags: ContextFlags,
    ) -> TypeId {
        let fn_ = get_containing_function(node);
        if fn_.is_some() {
            let function_flags = get_function_flags(fn_);
            let mut contextual_return_type = self.get_contextual_return_type(fn_, context_flags);
            if contextual_return_type.is_some() {
                let is_async_generator = function_flags.intersects(FunctionFlags::ASYNC);
                let is_yield_star = node.asterisk_token().is_some();
                if !is_yield_star
                    && self
                        .ty(contextual_return_type)
                        .flags
                        .intersects(TypeFlags::UNION)
                {
                    contextual_return_type = self.filter_type(
                        contextual_return_type,
                        &mut |c: &mut Checker, t: TypeId| {
                            c.get_iteration_type_of_generator_function_return_type(
                                IterationTypeKind::RETURN,
                                t,
                                is_async_generator,
                            )
                            .is_some()
                        },
                    );
                }
                if is_yield_star {
                    let iteration_types = self
                        .get_iteration_types_of_generator_function_return_type(
                            contextual_return_type,
                            is_async_generator,
                        );
                    let yield_type = if iteration_types.yield_type.is_some() {
                        iteration_types.yield_type
                    } else {
                        self.silent_never_type
                    };
                    let mut return_type = self.get_contextual_type(node, context_flags);
                    if return_type.is_nil() {
                        return_type = self.silent_never_type;
                    }
                    let next_type = if iteration_types.next_type.is_some() {
                        iteration_types.next_type
                    } else {
                        self.unknown_type
                    };
                    let generator_type = self.create_generator_type(
                        yield_type,
                        return_type,
                        next_type,
                        false, /*isAsyncGenerator*/
                    );
                    if is_async_generator {
                        let async_generator_type = self.create_generator_type(
                            yield_type,
                            return_type,
                            next_type,
                            true, /*isAsyncGenerator*/
                        );
                        return self.get_union_type(&[generator_type, async_generator_type]);
                    }
                    return generator_type;
                }
                return self.get_iteration_type_of_generator_function_return_type(
                    IterationTypeKind::YIELD,
                    contextual_return_type,
                    is_async_generator,
                );
            }
        }
        TypeId::NIL
    }

    // Go: checker/checker.go:30234 getContextualTypeForAwaitOperand
    pub fn get_contextual_type_for_await_operand(
        &mut self,
        node: Node,
        context_flags: ContextFlags,
    ) -> TypeId {
        let contextual_type = self.get_contextual_type(node, context_flags);
        if contextual_type.is_some() {
            let contextual_awaited_type = self.get_awaited_type_no_alias(contextual_type);
            if contextual_awaited_type.is_some() {
                let promise_like = self.create_promise_like_type(contextual_awaited_type);
                return self.get_union_type(&[contextual_awaited_type, promise_like]);
            }
        }
        TypeId::NIL
    }

    // Go: checker/checker.go:30246 getContextualTypeForArgument
    // In a typed function call, an argument or substitution expression is contextually typed by the type of the corresponding parameter.
    pub fn get_contextual_type_for_argument(&mut self, call_target: Node, arg: Node) -> TypeId {
        let args = self.get_effective_call_arguments(call_target);
        let arg_index = args.iter().position(|a| a == arg).map_or(-1, |i| i as i32);
        // -1 for e.g. the expression of a CallExpression, or the tag of a TaggedTemplateExpression
        if arg_index == -1 {
            return TypeId::NIL;
        }
        self.get_contextual_type_for_argument_at_index(call_target, arg_index)
    }

    // Go: checker/checker.go:30256 getContextualTypeForArgumentAtIndex
    pub fn get_contextual_type_for_argument_at_index(
        &mut self,
        call_target: Node,
        arg_index: i32,
    ) -> TypeId {
        if is_import_call(call_target) {
            if arg_index == 0 {
                return self.string_type;
            }
            if arg_index == 1 {
                return (self.get_global_import_call_options_type.clone())(self);
            }
            return self.any_type;
        }
        // If we're already in the process of resolving the given signature, don't resolve again as
        // that could cause infinite recursion. Instead, return anySignature.
        let signature;
        if self.signature_links.get(call_target).resolved_signature == self.resolving_signature {
            signature = self.resolving_signature;
        } else {
            signature = self.get_resolved_signature(call_target, None, CheckMode::NORMAL);
        }
        if is_jsx_opening_like_element(call_target) && arg_index == 0 {
            return self.get_effective_first_argument_for_jsx_signature(signature, call_target);
        }
        let rest_index = self.sig(signature).parameters.len() as i32 - 1;
        if self.signature_has_rest_parameter(signature) && arg_index >= rest_index {
            let rest_param = self.sig(signature).parameters[rest_index as usize];
            let rest_type = self.get_type_of_symbol(rest_param);
            let index_type = self.get_number_literal_type(Number((arg_index - rest_index) as f64));
            return self.get_indexed_access_type_ex(
                rest_type,
                index_type,
                AccessFlags::CONTEXTUAL,
                Node::NIL,
                None,
            );
        }
        self.get_type_at_position(signature, arg_index)
    }

    // Go: checker/checker.go:30285 getContextualTypeForDecorator
    pub fn get_contextual_type_for_decorator(&mut self, decorator: Node) -> TypeId {
        let signature = self.get_decorator_call_signature(decorator);
        if signature.is_some() {
            return self.get_or_create_type_from_signature(signature);
        }
        TypeId::NIL
    }

    // Go: checker/checker.go:30293 getContextualTypeForBinaryOperand
    pub fn get_contextual_type_for_binary_operand(
        &mut self,
        node: Node,
        context_flags: ContextFlags,
    ) -> TypeId {
        let binary = node.parent();
        // PORT: Go reads the `Type` field of `*ast.BinaryExpression` (a JSDoc cast type); `type_()` returns it.
        let t = binary.type_();
        if t.is_some() {
            return self.get_type_from_type_node(t);
        }
        match binary.operator_token().kind() {
            SyntaxKind::EqualsToken
            | SyntaxKind::AmpersandAmpersandEqualsToken
            | SyntaxKind::BarBarEqualsToken
            | SyntaxKind::QuestionQuestionEqualsToken => {
                // In an assignment expression, the right operand is contextually typed by the type of the left operand
                // unless it's an assignment declaration.
                if node == binary.right() {
                    let target = get_leftmost_expression(binary.left(), false);
                    if !(is_identifier(target) && {
                        let s = self.get_resolved_symbol(target);
                        self.sym(s).flags.intersects(SymbolFlags::MODULE_EXPORTS)
                    }) {
                        return self.get_contextual_type_for_assignment_expression(binary);
                    }
                }
            }
            SyntaxKind::BarBarToken | SyntaxKind::QuestionQuestionToken => {
                // When an || expression has a contextual type, the operands are contextually typed by that type, except
                // when that type originates in a binding pattern, the right operand is contextually typed by the type of
                // the left operand. When an || expression has no contextual type, the right operand is contextually typed
                // by the type of the left operand, except for the special case of Javascript declarations of the form
                // `namespace.prop = namespace.prop || {}`.
                let t = self.get_contextual_type(binary, context_flags);
                if node == binary.right()
                    && (t.is_nil()
                        || self
                            .pattern_for_type
                            .get(&t)
                            .copied()
                            .unwrap_or_default()
                            .is_some())
                {
                    return self.get_type_of_expression(binary.left());
                }
                return t;
            }
            SyntaxKind::AmpersandAmpersandToken | SyntaxKind::CommaToken => {
                if node == binary.right() {
                    return self.get_contextual_type(binary, context_flags);
                }
            }
            _ => {}
        }
        TypeId::NIL
    }

    // Go: checker/checker.go:30327 getContextualTypeForAssignmentExpression
    // PORT: Go takes `*ast.BinaryExpression`; here it is the binary expression `Node`.
    pub fn get_contextual_type_for_assignment_expression(&mut self, binary: Node) -> TypeId {
        let left = binary.left();
        if is_access_expression(left) {
            let expr = left.expression();
            match expr.kind() {
                SyntaxKind::Identifier => {
                    let resolved = self.get_resolved_symbol(expr);
                    let symbol = self.get_export_symbol_of_value_symbol_if_exported(resolved);
                    if self
                        .sym(symbol)
                        .flags
                        .intersects(SymbolFlags::MODULE_EXPORTS)
                    {
                        // No contextual type for an expression of the form 'module.exports = expr'.
                        return TypeId::NIL;
                    }
                    if binary.symbol().is_some() {
                        // We have an assignment declaration (a binary expression with a symbol assigned by the binder) of the form
                        // 'F.id = expr' or 'F[xxx] = expr'. If 'F' is declared as a variable with a type annotation, we can obtain a
                        // contextual type from the annotated type without triggering a circularity. Otherwise, the assignment
                        // declaration has no contextual type.
                        let value_declaration = self.sym(symbol).value_declaration;
                        if value_declaration.is_some() && is_variable_declaration(value_declaration)
                        {
                            let type_node = value_declaration.type_();
                            if type_node.is_some() {
                                if is_property_access_expression(left) {
                                    let t = self.get_type_from_type_node(type_node);
                                    return self.get_type_of_property_of_contextual_type(
                                        t,
                                        left.name().text(),
                                    );
                                }
                                let name_type =
                                    self.check_expression_cached(left.argument_expression());
                                if self.is_type_usable_as_property_name(name_type) {
                                    let t = self.get_type_from_type_node(type_node);
                                    let prop_name = self.get_property_name_from_type(name_type);
                                    return self.get_type_of_property_of_contextual_type_ex(
                                        t, &prop_name, name_type,
                                    );
                                }
                                return self.get_type_of_expression(left);
                            }
                        }
                        return TypeId::NIL;
                    }
                }
                SyntaxKind::PropertyAccessExpression | SyntaxKind::ElementAccessExpression => {
                    if binary.symbol().is_some() {
                        return TypeId::NIL;
                    }
                }
                SyntaxKind::ThisKeyword => {
                    let mut symbol = SymbolId::NIL;
                    let this_type = self.get_type_of_expression(expr);
                    if is_property_access_expression(left) {
                        let name = left.name();
                        if is_private_identifier(name) {
                            let this_symbol = self.ty(this_type).symbol;
                            if this_symbol.is_some() {
                                let private_name =
                                    self.private_identifier_symbol_name(this_symbol, name.text());
                                symbol = self.get_property_of_type(this_type, &private_name);
                            }
                        } else {
                            symbol = self.get_property_of_type(this_type, name.text());
                        }
                    } else {
                        let prop_type = self.check_expression_cached(left.argument_expression());
                        if self.is_type_usable_as_property_name(prop_type) {
                            let prop_name = self.get_property_name_from_type(prop_type);
                            symbol = self.get_property_of_type(this_type, &prop_name);
                        }
                    }
                    if symbol.is_some() {
                        let d = self.sym(symbol).value_declaration;
                        if d.is_some()
                            && (is_property_declaration(d) || is_property_signature_declaration(d))
                            && d.type_().is_nil()
                            && d.initializer().is_nil()
                        {
                            // No contextual type for 'this.xxx = expr', where xxx is declared as a property with no type annotation or initializer.
                            return TypeId::NIL;
                        }
                    }
                    let binary_symbol = binary.symbol();
                    if binary_symbol.is_some() {
                        let value_declaration = self.sym(binary_symbol).value_declaration;
                        if value_declaration.is_some() && value_declaration.type_().is_nil() {
                            // We have an assignment declaration 'this.xxx = expr' with no (synthetic) type annotation
                            let this_container = self.get_this_container(expr, false, false);
                            if !is_object_literal_method(this_container) {
                                return TypeId::NIL;
                            }
                            // and now for one single case of object literal methods
                            let name = get_element_or_property_access_name(left);
                            if name.is_nil() {
                                return TypeId::NIL;
                            } else {
                                // !!! contextual typing for `this` in object literals
                                return TypeId::NIL;
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        self.get_type_of_expression(left)
    }
}

/// Ends the current text of `get_template_literal_type`: the tail of `buf`
/// after the last end in `ends`. Applies `go_value` and then
/// `combine_surrogate_pairs` to it in place (a copy only when it has a
/// sentinel) and records its end.
fn end_template_text(buf: &mut String, ends: &mut SmallVec<[usize; 5]>) {
    let start = ends.last().copied().unwrap_or(0);
    let value = go_value(&buf[start..]);
    let changed = match combine_surrogate_pairs_cow(&value) {
        Cow::Owned(combined) => Some(combined),
        Cow::Borrowed(_) => None,
    };
    let changed = changed.or(match value {
        Cow::Owned(value) => Some(value),
        Cow::Borrowed(_) => None,
    });
    if let Some(text) = changed {
        buf.truncate(start);
        buf.push_str(&text);
    }
    ends.push(buf.len());
}

/// `get_template_type_key` for texts kept back to back in `buf`, where text
/// `k` ends at `ends[k]`. The key hasher reads one byte stream, so one write
/// of `buf` equals one write per text, and the key is the same.
fn get_template_type_key_of_buffer(buf: &str, ends: &[usize], types: &[TypeId]) -> CacheHashKey {
    let mut b = KeyBuilder::default();
    b.write_types(types);
    b.write_byte(b'|');
    let mut start = 0;
    for &end in ends {
        b.write_int((end - start) as i32);
        start = end;
    }
    b.write_byte(b'|');
    b.write_string(buf);
    b.hash()
}
