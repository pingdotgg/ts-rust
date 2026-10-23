//! Port of Effect-TS/tsgo `internal/typeparser/pipeable_signature_shape.go`.

use crate::effect::typeparser::*;
use crate::prelude::*;

use std::cell::Cell;

pub const PIPEABLE_SHAPE_MAX_DEPTH: i32 = 64;
pub const PIPEABLE_SHAPE_MAX_WORK: i32 = 4096;
pub const PIPEABLE_SHAPE_MAX_UNION_SEARCH: i32 = 128;

/// Go `pipeableSignatureShapeCacheKey`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PipeableSignatureShapeCacheKey {
    pub data_first: SignatureId,
    pub candidate: SignatureId,
    pub subject_index: i32,
}

/// Go `pipeableTypePair`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PipeableTypePair {
    pub left: TypeId,
    pub right: TypeId,
}

// Go: typeparser/pipeable_signature_shape.go rawSignature
// PORT: Go reads `signature.Target()` without a checker; here the signature
// arena is on the checker, so it takes `c`.
pub fn raw_signature(c: &Checker, signature: SignatureId) -> SignatureId {
    let mut signature = signature;
    while signature.is_some() && c.sig(signature).target().is_some() {
        signature = c.sig(signature).target();
    }
    signature
}

// Go: typeparser/pipeable_signature_shape.go pipeableSignatureShapesMatch
pub fn pipeable_signature_shapes_match(
    c: &mut Checker,
    data_first: SignatureId,
    candidate: SignatureId,
    subject_index: i32,
) -> bool {
    let data_first = raw_signature(c, data_first);
    let candidate = raw_signature(c, candidate);
    if data_first.is_nil() || candidate.is_nil() {
        return false;
    }
    let key = PipeableSignatureShapeCacheKey {
        data_first,
        candidate,
        subject_index,
    };
    if c.effect_links.is_some() {
        // Go `Cached(&links.PipeableSignatureShape, key, ..)`.
        if let Some(value) = c
            .effect_links
            .as_ref()
            .and_then(|links| links.pipeable_signature_shape.get(&key))
        {
            return *value;
        }
        let value = compare_pipeable_signature_types(c, data_first, candidate, subject_index);
        if let Some(links) = c.effect_links.as_mut() {
            links.pipeable_signature_shape.insert(key, value);
        }
        return value;
    }
    compare_pipeable_signature_types(c, data_first, candidate, subject_index)
}

/// Go `signatureTypeMatcher`.
/// signatureTypeMatcher zips existing, uninstantiated compiler type graphs. It
/// never asks the checker to relate two signatures and never creates a mapper.
/// Generic binders are paired by structural occurrence rather than declaration
/// order, with maps in both directions to preserve binder identity.
// PORT: Go keeps `checker *checker.Checker` in the struct. A clone of the
// matcher (compareUnionAt) would need a second `&mut Checker`, so every
// method takes `c` instead. Go `work *int` and `unionSearch *int` are shared
// by all clones: `Rc<Cell<i32>>`. Go ranges `leftToRight` in map order (in
// validateBinders); the port uses insertion order.
#[derive(Clone)]
pub struct SignatureTypeMatcher {
    pub left_to_right: IndexMap<TypeId, TypeId>,
    pub right_to_left: FxHashMap<TypeId, TypeId>,
    pub left_binders: FxHashSet<TypeId>,
    pub right_binders: FxHashSet<TypeId>,
    pub active: FxHashSet<PipeableTypePair>,
    pub work: Rc<Cell<i32>>,
    pub union_search: Rc<Cell<i32>>,
}

// Go: typeparser/pipeable_signature_shape.go comparePipeableSignatureTypes
pub fn compare_pipeable_signature_types(
    c: &mut Checker,
    data_first: SignatureId,
    candidate: SignatureId,
    subject_index: i32,
) -> bool {
    if data_first.is_nil() || candidate.is_nil() {
        return false;
    }
    let left_parameters = c.sig(data_first).parameters().to_vec();
    if subject_index < 0 || subject_index as usize >= left_parameters.len() {
        return false;
    }

    let candidate_return = c.get_return_type_of_signature_exported(candidate);
    if candidate_return.is_nil() {
        return false;
    }
    let returned_signatures =
        c.get_signatures_of_type_exported(candidate_return, SignatureKind::CALL);
    if returned_signatures.len() != 1
        || returned_signatures[0].is_nil()
        || c.sig(returned_signatures[0]).target().is_some()
    {
        // A target here means that discovering the callable shape instantiated a
        // generic alias. Dropping its mapper would compare the wrong raw graph.
        return false;
    }
    let returned = returned_signatures[0];
    if c.sig(returned).parameters().len() != 1 || c.sig(returned).has_rest_parameter() {
        return false;
    }

    let mut m = SignatureTypeMatcher {
        left_to_right: IndexMap::new(),
        right_to_left: FxHashMap::default(),
        left_binders: FxHashSet::default(),
        right_binders: FxHashSet::default(),
        active: FxHashSet::default(),
        work: Rc::new(Cell::new(0)),
        union_search: Rc::new(Cell::new(0)),
    };
    let data_first_type_parameters = c.sig(data_first).type_parameters().to_vec();
    m.register_binders(c, &data_first_type_parameters, true);
    let candidate_type_parameters = c.sig(candidate).type_parameters().to_vec();
    m.register_binders(c, &candidate_type_parameters, false);
    let returned_type_parameters = c.sig(returned).type_parameters().to_vec();
    m.register_binders(c, &returned_type_parameters, false);
    if m.left_binders.len() != m.right_binders.len() {
        return false;
    }

    let right_parameters = c.sig(candidate).parameters().to_vec();
    if right_parameters.len() != left_parameters.len() - 1 {
        return false;
    }
    let data_first_has_rest = c.sig(data_first).has_rest_parameter();
    let candidate_has_rest = c.sig(candidate).has_rest_parameter();
    let mut right_index = 0usize;
    for (left_index, &left_parameter) in left_parameters.iter().enumerate() {
        if left_index as i32 == subject_index {
            continue;
        }
        if !m.compare_parameter(
            c,
            left_parameter,
            left_index == left_parameters.len() - 1 && data_first_has_rest,
            right_parameters[right_index],
            right_index == right_parameters.len() - 1 && candidate_has_rest,
            0,
        ) {
            return false;
        }
        right_index += 1;
    }
    let returned_parameter = c.sig(returned).parameters()[0];
    if !m.compare_parameter(
        c,
        left_parameters[subject_index as usize],
        false,
        returned_parameter,
        false,
        0,
    ) {
        return false;
    }
    let data_first_return = c.get_return_type_of_signature_exported(data_first);
    let returned_return = c.get_return_type_of_signature_exported(returned);
    if !m.compare_type(c, data_first_return, returned_return, 0) {
        return false;
    }
    m.validate_binders(c)
}

impl SignatureTypeMatcher {
    // Go: typeparser/pipeable_signature_shape.go signatureTypeMatcher.registerBinders
    pub fn register_binders(&mut self, c: &Checker, types: &[TypeId], left: bool) {
        for &t in types {
            if t.is_nil() || !c.ty(t).flags().intersects(TypeFlags::TYPE_PARAMETER) {
                continue;
            }
            if left {
                self.left_binders.insert(t);
            } else {
                self.right_binders.insert(t);
            }
        }
    }

    // Go: typeparser/pipeable_signature_shape.go signatureTypeMatcher.compareParameter
    pub fn compare_parameter(
        &mut self,
        c: &mut Checker,
        left: SymbolId,
        left_rest: bool,
        right: SymbolId,
        right_rest: bool,
        depth: i32,
    ) -> bool {
        if left.is_nil()
            || right.is_nil()
            || left_rest != right_rest
            || symbol_is_optional(c, left) != symbol_is_optional(c, right)
        {
            return false;
        }
        let left_type = c.get_type_of_symbol_exported(left);
        let right_type = c.get_type_of_symbol_exported(right);
        self.compare_type(c, left_type, right_type, depth + 1)
    }
}

// Go: typeparser/pipeable_signature_shape.go symbolIsOptional
// PORT: Go reads `symbol.Flags` without a checker; the symbol arena is on
// the checker here.
pub fn symbol_is_optional(c: &Checker, symbol: SymbolId) -> bool {
    symbol.is_some() && c.sym_flags(symbol).intersects(SymbolFlags::OPTIONAL)
}

impl SignatureTypeMatcher {
    // Go: typeparser/pipeable_signature_shape.go signatureTypeMatcher.compareType
    pub fn compare_type(
        &mut self,
        c: &mut Checker,
        left: TypeId,
        right: TypeId,
        depth: i32,
    ) -> bool {
        let left = get_non_distributed_type_parameter(c, left);
        let right = get_non_distributed_type_parameter(c, right);
        if left == right {
            return left.is_some();
        }
        if left.is_nil() || right.is_nil() || depth > PIPEABLE_SHAPE_MAX_DEPTH {
            return false;
        }
        self.work.set(self.work.get() + 1);
        if self.work.get() > PIPEABLE_SHAPE_MAX_WORK {
            return false;
        }

        let base = no_infer_base_type(c, left);
        if base.is_some() {
            return self.compare_type(c, base, right, depth + 1);
        }
        let base = no_infer_base_type(c, right);
        if base.is_some() {
            return self.compare_type(c, left, base, depth + 1);
        }

        let pair = PipeableTypePair { left, right };
        if self.active.contains(&pair) {
            return true;
        }
        self.active.insert(pair);
        // Go `defer delete(m.active, pair)`.
        let result = 'body: {
            let left_alias = c.ty(left).alias();
            let right_alias = c.ty(right).alias();
            if left_alias.is_some() || right_alias.is_some() {
                break 'body match (left_alias, right_alias) {
                    (Some(left_alias), Some(right_alias)) => {
                        same_symbol_reference(c, left_alias.symbol(), right_alias.symbol())
                            && self.compare_type_list(
                                c,
                                left_alias.type_arguments(),
                                right_alias.type_arguments(),
                                depth + 1,
                            )
                    }
                    _ => false,
                };
            }

            let left_flags = c.ty(left).flags();
            let right_flags = c.ty(right).flags();
            let left_is_binder = left_flags.intersects(TypeFlags::TYPE_PARAMETER);
            let right_is_binder = right_flags.intersects(TypeFlags::TYPE_PARAMETER);
            if left_is_binder || right_is_binder {
                break 'body left_is_binder
                    && right_is_binder
                    && self.compare_type_parameter(c, left, right);
            }
            if left_flags != right_flags {
                break 'body false;
            }

            if left_flags.intersects(TypeFlags::UNION) {
                let left_types = c.ty(left).types().to_vec();
                let right_types = c.ty(right).types().to_vec();
                self.compare_union(c, &left_types, &right_types, depth + 1)
            } else if left_flags.intersects(TypeFlags::INTERSECTION) {
                // Preserve compiler order for intersections; unlike unions, their order
                // can affect overloaded callable behavior.
                let left_types = c.ty(left).types().to_vec();
                let right_types = c.ty(right).types().to_vec();
                self.compare_type_list(c, &left_types, &right_types, depth + 1)
            } else if left_flags.intersects(TypeFlags::CONDITIONAL) {
                let left_check = c.ty(left).as_conditional_type().check_type();
                let right_check = c.ty(right).as_conditional_type().check_type();
                if !self.compare_type(c, left_check, right_check, depth + 1) {
                    break 'body false;
                }
                let left_extends = c.ty(left).as_conditional_type().extends_type();
                let right_extends = c.ty(right).as_conditional_type().extends_type();
                if !self.compare_type(c, left_extends, right_extends, depth + 1) {
                    break 'body false;
                }
                let left_true = c.get_true_type_of_conditional_type(left);
                let right_true = c.get_true_type_of_conditional_type(right);
                if !self.compare_type(c, left_true, right_true, depth + 1) {
                    break 'body false;
                }
                let left_false = c.get_false_type_of_conditional_type(left);
                let right_false = c.get_false_type_of_conditional_type(right);
                self.compare_type(c, left_false, right_false, depth + 1)
            } else if left_flags.intersects(TypeFlags::INDEXED_ACCESS) {
                let left_object = c.ty(left).as_indexed_access_type().object_type();
                let right_object = c.ty(right).as_indexed_access_type().object_type();
                if !self.compare_type(c, left_object, right_object, depth + 1) {
                    break 'body false;
                }
                let left_index = c.ty(left).as_indexed_access_type().index_type();
                let right_index = c.ty(right).as_indexed_access_type().index_type();
                self.compare_type(c, left_index, right_index, depth + 1)
            } else if left_flags.intersects(TypeFlags::INDEX) {
                let left_target = c.ty(left).as_index_type().target();
                let right_target = c.ty(right).as_index_type().target();
                self.compare_type(c, left_target, right_target, depth + 1)
            } else if left_flags.intersects(TypeFlags::TEMPLATE_LITERAL) {
                if c.ty(left).as_template_literal_type().texts()
                    != c.ty(right).as_template_literal_type().texts()
                {
                    break 'body false;
                }
                let left_types = c.ty(left).as_template_literal_type().types().to_vec();
                let right_types = c.ty(right).as_template_literal_type().types().to_vec();
                self.compare_type_list(c, &left_types, &right_types, depth + 1)
            } else if left_flags.intersects(TypeFlags::STRING_MAPPING) {
                let left_symbol = c.ty(left).symbol();
                let right_symbol = c.ty(right).symbol();
                if !same_symbol_reference(c, left_symbol, right_symbol) {
                    break 'body false;
                }
                let left_target = c.ty(left).as_string_mapping_type().target();
                let right_target = c.ty(right).as_string_mapping_type().target();
                self.compare_type(c, left_target, right_target, depth + 1)
            } else if left_flags.intersects(TypeFlags::SUBSTITUTION) {
                let left_base = c.ty(left).as_substitution_type().base_type();
                let right_base = c.ty(right).as_substitution_type().base_type();
                if !self.compare_type(c, left_base, right_base, depth + 1) {
                    break 'body false;
                }
                let left_constraint = c.ty(left).as_substitution_type().subst_constraint();
                let right_constraint = c.ty(right).as_substitution_type().subst_constraint();
                self.compare_type(c, left_constraint, right_constraint, depth + 1)
            } else if left_flags.intersects(TypeFlags::OBJECT) {
                self.compare_object(c, left, right, depth + 1)
            } else if left_flags.intersects(TypeFlags::LITERAL) {
                // Go `reflect.DeepEqual(left.Value(), right.Value())`.
                let same_value =
                    c.ty(left).as_literal_type().value() == c.ty(right).as_literal_type().value();
                let left_symbol = c.ty(left).symbol();
                let right_symbol = c.ty(right).symbol();
                same_value && same_optional_symbol(c, left_symbol, right_symbol)
            } else if left_flags.intersects(TypeFlags::UNIQUE_ES_SYMBOL)
                || left_flags.intersects(TypeFlags::ENUM)
            {
                let left_symbol = c.ty(left).symbol();
                let right_symbol = c.ty(right).symbol();
                same_symbol_reference(c, left_symbol, right_symbol)
            } else if left_flags.intersects(TypeFlags::SINGLETON) {
                true
            } else {
                false
            }
        };
        self.active.remove(&pair);
        result
    }

    // Go: typeparser/pipeable_signature_shape.go signatureTypeMatcher.compareTypeParameter
    pub fn compare_type_parameter(&mut self, c: &mut Checker, left: TypeId, right: TypeId) -> bool {
        let left_local = self.left_binders.contains(&left);
        let right_local = self.right_binders.contains(&right);
        if left_local || right_local {
            if !left_local || !right_local {
                return false;
            }
            if let Some(&mapped) = self.left_to_right.get(&left) {
                return mapped == right;
            }
            if let Some(&mapped) = self.right_to_left.get(&right) {
                return mapped == left;
            }
            self.left_to_right.insert(left, right);
            self.right_to_left.insert(right, left);
            return true;
        }
        let left_symbol = c.ty(left).symbol();
        let right_symbol = c.ty(right).symbol();
        left == right || same_symbol_reference(c, left_symbol, right_symbol)
    }

    // Go: typeparser/pipeable_signature_shape.go signatureTypeMatcher.compareObject
    pub fn compare_object(
        &mut self,
        c: &mut Checker,
        left: TypeId,
        right: TypeId,
        depth: i32,
    ) -> bool {
        let left_flags = c.ty(left).object_flags();
        let right_flags = c.ty(right).object_flags();
        let left_mapped = left_flags.intersects(ObjectFlags::MAPPED);
        let right_mapped = right_flags.intersects(ObjectFlags::MAPPED);
        if left_mapped || right_mapped {
            return left_mapped
                && right_mapped
                && self.compare_mapped_type(c, left, right, depth + 1);
        }
        if left_flags.intersects(ObjectFlags::REVERSE_MAPPED)
            || right_flags.intersects(ObjectFlags::REVERSE_MAPPED)
            || left_flags.intersects(ObjectFlags::EVOLVING_ARRAY)
            || right_flags.intersects(ObjectFlags::EVOLVING_ARRAY)
            || left_flags.intersects(ObjectFlags::INSTANTIATION_EXPRESSION_TYPE)
            || right_flags.intersects(ObjectFlags::INSTANTIATION_EXPRESSION_TYPE)
        {
            return false;
        }

        let left_reference = left_flags.intersects(ObjectFlags::REFERENCE);
        let right_reference = right_flags.intersects(ObjectFlags::REFERENCE);
        if left_reference || right_reference {
            if !left_reference || !right_reference {
                return false;
            }
            let left_target = c.ty(left).target();
            let right_target = c.ty(right).target();
            let same_target = left_target == right_target || {
                let left_target_symbol = c.ty(left_target).symbol();
                let right_target_symbol = c.ty(right_target).symbol();
                same_symbol_reference(c, left_target_symbol, right_target_symbol)
            };
            if !same_target {
                return false;
            }
            let left_arguments = c.get_type_arguments_exported(left);
            let right_arguments = c.get_type_arguments_exported(right);
            return self.compare_type_list(c, &left_arguments, &right_arguments, depth + 1);
        }

        let left_anonymous = left_flags.intersects(ObjectFlags::ANONYMOUS);
        let right_anonymous = right_flags.intersects(ObjectFlags::ANONYMOUS);
        if !left_anonymous || !right_anonymous {
            if left_anonymous || right_anonymous {
                return false;
            }
            let left_symbol = c.ty(left).symbol();
            let right_symbol = c.ty(right).symbol();
            return same_symbol_reference(c, left_symbol, right_symbol);
        }
        self.compare_anonymous_object(c, left, right, depth + 1)
    }

    // Go: typeparser/pipeable_signature_shape.go signatureTypeMatcher.compareMappedType
    pub fn compare_mapped_type(
        &mut self,
        c: &mut Checker,
        left: TypeId,
        right: TypeId,
        depth: i32,
    ) -> bool {
        let left_binder = c.get_type_parameter_from_mapped_type(left);
        let right_binder = c.get_type_parameter_from_mapped_type(right);
        if left_binder.is_nil()
            || right_binder.is_nil()
            || c.get_mapped_type_modifiers(left) != c.get_mapped_type_modifiers(right)
        {
            return false;
        }
        self.register_binders(c, &[left_binder], true);
        self.register_binders(c, &[right_binder], false);
        if !self.compare_type(c, left_binder, right_binder, depth + 1) {
            return false;
        }
        let left_constraint = c.get_constraint_type_from_mapped_type(left);
        let right_constraint = c.get_constraint_type_from_mapped_type(right);
        if !self.compare_type(c, left_constraint, right_constraint, depth + 1) {
            return false;
        }
        let left_name = c.get_name_type_from_mapped_type(left);
        let right_name = c.get_name_type_from_mapped_type(right);
        if !self.compare_optional_type_at(c, left_name, right_name, depth + 1) {
            return false;
        }
        let left_template = c.get_template_type_from_mapped_type(left);
        let right_template = c.get_template_type_from_mapped_type(right);
        self.compare_type(c, left_template, right_template, depth + 1)
    }

    // Go: typeparser/pipeable_signature_shape.go signatureTypeMatcher.compareAnonymousObject
    pub fn compare_anonymous_object(
        &mut self,
        c: &mut Checker,
        left: TypeId,
        right: TypeId,
        depth: i32,
    ) -> bool {
        let left_properties = c.get_properties_of_type_exported(left);
        let right_properties = c.get_properties_of_type_exported(right);
        if left_properties.len() != right_properties.len() {
            return false;
        }
        let mut right_by_name: FxHashMap<Name, SymbolId> = FxHashMap::default();
        for &property in &right_properties {
            if property.is_nil() {
                return false;
            }
            let name = c.sym(property).name.clone();
            if right_by_name.get(&name).is_some_and(|s| s.is_some()) {
                return false;
            }
            right_by_name.insert(name, property);
        }
        for &left_property in &left_properties {
            if left_property.is_nil() {
                return false;
            }
            let right_property = right_by_name
                .get(&c.sym(left_property).name)
                .copied()
                .unwrap_or(SymbolId::NIL);
            if right_property.is_nil()
                || symbol_is_optional(c, left_property) != symbol_is_optional(c, right_property)
                || c.is_readonly_symbol(left_property) != c.is_readonly_symbol(right_property)
            {
                return false;
            }
            let left_type = c.get_type_of_symbol_exported(left_property);
            let right_type = c.get_type_of_symbol_exported(right_property);
            if !self.compare_type(c, left_type, right_type, depth + 1) {
                return false;
            }
        }

        let left_indexes = c.get_index_infos_of_type_exported(left);
        let right_indexes = c.get_index_infos_of_type_exported(right);
        if left_indexes.len() != right_indexes.len() {
            return false;
        }
        for i in 0..left_indexes.len() {
            if c.index_info(left_indexes[i]).is_readonly()
                != c.index_info(right_indexes[i]).is_readonly()
            {
                return false;
            }
            let left_key = c.index_info(left_indexes[i]).key_type();
            let right_key = c.index_info(right_indexes[i]).key_type();
            if !self.compare_type(c, left_key, right_key, depth + 1) {
                return false;
            }
            let left_value = c.index_info(left_indexes[i]).value_type();
            let right_value = c.index_info(right_indexes[i]).value_type();
            if !self.compare_type(c, left_value, right_value, depth + 1) {
                return false;
            }
        }

        if !c
            .get_signatures_of_type_exported(left, SignatureKind::CONSTRUCT)
            .is_empty()
            || !c
                .get_signatures_of_type_exported(right, SignatureKind::CONSTRUCT)
                .is_empty()
        {
            return false;
        }
        let left_calls = c.get_signatures_of_type_exported(left, SignatureKind::CALL);
        let right_calls = c.get_signatures_of_type_exported(right, SignatureKind::CALL);
        if left_calls.len() != right_calls.len() {
            return false;
        }
        for i in 0..left_calls.len() {
            if c.sig(left_calls[i]).target().is_some()
                || c.sig(right_calls[i]).target().is_some()
                || !self.compare_signature(c, left_calls[i], right_calls[i], depth + 1)
            {
                return false;
            }
        }
        true
    }

    // Go: typeparser/pipeable_signature_shape.go signatureTypeMatcher.compareSignature
    pub fn compare_signature(
        &mut self,
        c: &mut Checker,
        left: SignatureId,
        right: SignatureId,
        depth: i32,
    ) -> bool {
        if left.is_nil()
            || right.is_nil()
            || c.sig(left).has_rest_parameter() != c.sig(right).has_rest_parameter()
            || c.sig(left).min_argument_count() != c.sig(right).min_argument_count()
            || (c.get_type_predicate_of_signature_exported(left).is_some()
                || c.get_type_predicate_of_signature_exported(right).is_some())
        {
            return false;
        }
        let left_type_parameters = c.sig(left).type_parameters().to_vec();
        self.register_binders(c, &left_type_parameters, true);
        let right_type_parameters = c.sig(right).type_parameters().to_vec();
        self.register_binders(c, &right_type_parameters, false);
        let left_parameters = c.sig(left).parameters().to_vec();
        let right_parameters = c.sig(right).parameters().to_vec();
        if left_parameters.len() != right_parameters.len() {
            return false;
        }
        let left_this = c.sig(left).this_parameter();
        let right_this = c.sig(right).this_parameter();
        if left_this.is_nil() != right_this.is_nil() {
            return false;
        }
        if left_this.is_some()
            && !self.compare_parameter(c, left_this, false, right_this, false, depth + 1)
        {
            return false;
        }
        let left_has_rest = c.sig(left).has_rest_parameter();
        let right_has_rest = c.sig(right).has_rest_parameter();
        for i in 0..left_parameters.len() {
            if !self.compare_parameter(
                c,
                left_parameters[i],
                i == left_parameters.len() - 1 && left_has_rest,
                right_parameters[i],
                i == right_parameters.len() - 1 && right_has_rest,
                depth + 1,
            ) {
                return false;
            }
        }
        let left_return = c.get_return_type_of_signature_exported(left);
        let right_return = c.get_return_type_of_signature_exported(right);
        self.compare_type(c, left_return, right_return, depth + 1)
    }

    // Go: typeparser/pipeable_signature_shape.go signatureTypeMatcher.compareTypeList
    pub fn compare_type_list(
        &mut self,
        c: &mut Checker,
        left: &[TypeId],
        right: &[TypeId],
        depth: i32,
    ) -> bool {
        if left.len() != right.len() {
            return false;
        }
        for i in 0..left.len() {
            if !self.compare_type(c, left[i], right[i], depth + 1) {
                return false;
            }
        }
        true
    }

    // Go: typeparser/pipeable_signature_shape.go signatureTypeMatcher.compareUnion
    pub fn compare_union(
        &mut self,
        c: &mut Checker,
        left: &[TypeId],
        right: &[TypeId],
        depth: i32,
    ) -> bool {
        if left.len() != right.len() {
            return false;
        }
        self.compare_union_at(c, left, right, &vec![false; right.len()], 0, depth + 1)
    }

    // Go: typeparser/pipeable_signature_shape.go signatureTypeMatcher.compareUnionAt
    pub fn compare_union_at(
        &mut self,
        c: &mut Checker,
        left: &[TypeId],
        right: &[TypeId],
        used: &[bool],
        index: usize,
        depth: i32,
    ) -> bool {
        if index == left.len() {
            return true;
        }
        for right_index in 0..right.len() {
            if used[right_index] {
                continue;
            }
            self.union_search.set(self.union_search.get() + 1);
            if self.union_search.get() > PIPEABLE_SHAPE_MAX_UNION_SEARCH {
                return false;
            }
            let mut branch = self.clone_matcher();
            if !branch.compare_type(c, left[index], right[right_index], depth + 1) {
                continue;
            }
            let mut branch_used = used.to_vec();
            branch_used[right_index] = true;
            if branch.compare_union_at(c, left, right, &branch_used, index + 1, depth + 1) {
                self.commit(branch);
                return true;
            }
        }
        false
    }

    // Go: typeparser/pipeable_signature_shape.go signatureTypeMatcher.clone
    // PORT: named `clone_matcher` so it does not shadow `Clone::clone`. Go
    // copies the struct (sharing `work` and `unionSearch`) and clones the
    // five maps, which is what the derived `Clone` does.
    pub fn clone_matcher(&self) -> SignatureTypeMatcher {
        self.clone()
    }

    // Go: typeparser/pipeable_signature_shape.go signatureTypeMatcher.commit
    pub fn commit(&mut self, branch: SignatureTypeMatcher) {
        self.left_to_right = branch.left_to_right;
        self.right_to_left = branch.right_to_left;
        self.left_binders = branch.left_binders;
        self.right_binders = branch.right_binders;
    }

    // Go: typeparser/pipeable_signature_shape.go signatureTypeMatcher.validateBinders
    pub fn validate_binders(&mut self, c: &mut Checker) -> bool {
        if self.left_binders.len() != self.right_binders.len()
            || self.left_to_right.len() != self.left_binders.len()
        {
            return false;
        }
        let pairs: Vec<(TypeId, TypeId)> =
            self.left_to_right.iter().map(|(&l, &r)| (l, r)).collect();
        for (left, right) in pairs {
            if c.get_type_parameter_modifiers(left) != c.get_type_parameter_modifiers(right) {
                return false;
            }
            let left_constraint = c.get_constraint_of_type_parameter_exported(left);
            let right_constraint = c.get_constraint_of_type_parameter_exported(right);
            if !self.compare_optional_type(c, left_constraint, right_constraint) {
                return false;
            }
            let left_default = c.get_default_from_type_parameter_exported(left);
            let right_default = c.get_default_from_type_parameter_exported(right);
            if !self.compare_optional_type(c, left_default, right_default) {
                return false;
            }
        }
        true
    }

    // Go: typeparser/pipeable_signature_shape.go signatureTypeMatcher.compareOptionalType
    pub fn compare_optional_type(&mut self, c: &mut Checker, left: TypeId, right: TypeId) -> bool {
        self.compare_optional_type_at(c, left, right, 0)
    }

    // Go: typeparser/pipeable_signature_shape.go signatureTypeMatcher.compareOptionalTypeAt
    pub fn compare_optional_type_at(
        &mut self,
        c: &mut Checker,
        left: TypeId,
        right: TypeId,
        depth: i32,
    ) -> bool {
        if left.is_nil() || right.is_nil() {
            return left == right;
        }
        self.compare_type(c, left, right, depth + 1)
    }
}

// Go: typeparser/pipeable_signature_shape.go sameOptionalSymbol
pub fn same_optional_symbol(c: &mut Checker, left: SymbolId, right: SymbolId) -> bool {
    if left.is_nil() || right.is_nil() {
        return left == right;
    }
    same_symbol_reference(c, left, right)
}

// Go: typeparser/pipeable_signature_shape.go noInferBaseType
pub fn no_infer_base_type(c: &mut Checker, t: TypeId) -> TypeId {
    if t.is_nil() {
        return TypeId::NIL;
    }
    if c.is_no_infer_type(t) {
        return c.ty(t).as_substitution_type().base_type();
    }
    let Some(alias) = c.ty(t).alias() else {
        return TypeId::NIL;
    };
    if alias.type_arguments().len() != 1 || alias.symbol().is_nil() {
        return TypeId::NIL;
    }
    let declarations = c.sym(alias.symbol()).declarations.to_vec();
    for declaration in declarations {
        if is_no_infer_alias_declaration(c, declaration) {
            return alias.type_arguments()[0];
        }
    }
    TypeId::NIL
}

// Go: typeparser/pipeable_signature_shape.go isNoInferAliasDeclaration
// Effect supported NoInfer before it became a TypeScript intrinsic with the
// standard tuple/indexed-access encoding. Recognize only those two exact
// declarations so an unrelated alias named NoInfer is not made transparent.
pub fn is_no_infer_alias_declaration(c: &mut Checker, declaration: Node) -> bool {
    if declaration.is_nil()
        || declaration.kind() != SyntaxKind::TypeAliasDeclaration
        || declaration.type_parameters().len() != 1
    {
        return false;
    }
    let body = declaration.type_();
    if body.is_nil() {
        return false;
    }
    if body.kind() == SyntaxKind::IntrinsicKeyword {
        return true;
    }
    if body.kind() != SyntaxKind::IndexedAccessType {
        return false;
    }
    let indexed_object_type = body.object_type();
    let indexed_index_type = body.index_type();
    if indexed_object_type.is_nil()
        || indexed_object_type.kind() != SyntaxKind::TupleType
        || indexed_object_type.elements().len() != 1
        || indexed_index_type.is_nil()
        || indexed_index_type.kind() != SyntaxKind::ConditionalType
    {
        return false;
    }
    let type_parameter = declaration.type_parameters().get(0);
    let mut binder = type_parameter.symbol();
    if binder.is_nil() {
        binder = c.get_symbol_at_location_exported(type_parameter.name());
    }
    let conditional = indexed_index_type;
    binder.is_some()
        && is_type_parameter_type_node(c, indexed_object_type.elements().get(0), binder)
        && is_type_parameter_type_node(c, conditional.check_type(), binder)
        && conditional.extends_type().is_some()
        && conditional.extends_type().kind() == SyntaxKind::AnyKeyword
        && is_numeric_literal_type_node(conditional.true_type(), "0")
        && conditional.false_type().is_some()
        && conditional.false_type().kind() == SyntaxKind::NeverKeyword
}

// Go: typeparser/pipeable_signature_shape.go isTypeParameterTypeNode
pub fn is_type_parameter_type_node(c: &mut Checker, node: Node, binder: SymbolId) -> bool {
    if node.is_nil() || node.kind() != SyntaxKind::TypeReference {
        return false;
    }
    if !node.type_argument_list().is_nil() {
        return false;
    }
    let symbol = c.get_symbol_at_location_exported(node.type_name());
    same_symbol_reference(c, symbol, binder)
}

// Go: typeparser/pipeable_signature_shape.go isNumericLiteralTypeNode
pub fn is_numeric_literal_type_node(node: Node, value: &str) -> bool {
    node.is_some()
        && node.kind() == SyntaxKind::LiteralType
        && node.literal().is_some()
        && node.literal().kind() == SyntaxKind::NumericLiteral
        && node.literal().text() == value
}

/// Effect shim `checker.GetNonDistributedTypeParameter`
/// (`shim/checker/compatibility.go`): the existing declaration binder for a
/// distributed type parameter. It only reads compiler-owned types and never
/// instantiates or allocates a replacement type.
fn get_non_distributed_type_parameter(c: &Checker, t: TypeId) -> TypeId {
    if t.is_nil() {
        return TypeId::NIL;
    }
    c.get_non_distributed_type_parameter(t)
}
