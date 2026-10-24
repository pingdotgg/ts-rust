//! Port of typescript-go `checker/relater.go` lines 1827-2795.

use crate::diagnostics::Message;
use crate::prelude::*;
use smallvec::SmallVec;
use std::borrow::Cow;

impl Checker {
    // Go: checker/relater.go:1860 getNameableDeclarationAtPosition
    pub fn get_nameable_declaration_at_position(
        &mut self,
        signature: SignatureId,
        pos: i32,
    ) -> Node {
        let parameters = self.sig(signature).parameters.clone();
        let param_count = parameters.len() as i32
            - if self.signature_has_rest_parameter(signature) {
                1
            } else {
                0
            };
        if pos < param_count {
            let decl = self.sym(parameters[pos as usize]).value_declaration;
            if decl.is_some() && self.is_valid_declaration_for_tuple_label(decl) {
                return decl;
            }
            return Node::NIL;
        }
        if self.signature_has_rest_parameter(signature) {
            let rest_parameter = parameters[param_count as usize];
            let rest_type = self.get_type_of_symbol(rest_parameter);
            if self.is_tuple_type(rest_type) {
                let element_infos = self.target_tuple_type(rest_type).element_infos.clone();
                let index = pos - param_count;
                if (index as usize) < element_infos.len() {
                    return element_infos[index as usize].labeled_declaration;
                }
                return Node::NIL;
            }
            let value_declaration = self.sym(rest_parameter).value_declaration;
            if value_declaration.is_some()
                && self.is_valid_declaration_for_tuple_label(value_declaration)
            {
                return value_declaration;
            }
        }
        Node::NIL
    }

    // Go: checker/relater.go:1887 isValidDeclarationForTupleLabel
    pub fn is_valid_declaration_for_tuple_label(&self, d: Node) -> bool {
        is_named_tuple_member(d)
            || is_parameter_declaration(d) && d.name().is_some() && is_identifier(d.name())
    }

    // Go: checker/relater.go:1891 getNonArrayRestType
    pub fn get_non_array_rest_type(&mut self, signature: SignatureId) -> TypeId {
        let rest_type = self.get_effective_rest_type(signature);
        if rest_type.is_some() && !self.is_array_type(rest_type) && !self.is_type_any(rest_type) {
            return rest_type;
        }
        TypeId::NIL
    }

    // Go: checker/relater.go:1899 getEffectiveRestType
    pub fn get_effective_rest_type(&mut self, signature: SignatureId) -> TypeId {
        if self.signature_has_rest_parameter(signature) {
            let last = *self
                .sig(signature)
                .parameters
                .last()
                .expect("rest parameter");
            let rest_type = self.get_type_of_symbol(last);
            if !self.is_tuple_type(rest_type) {
                if self.is_type_any(rest_type) {
                    return self.any_array_type;
                }
                return rest_type;
            }
            let (combined_flags, fixed_length) = {
                let tuple = self.target_tuple_type(rest_type);
                (tuple.combined_flags, tuple.fixed_length)
            };
            if combined_flags.intersects(ElementFlags::VARIABLE) {
                return self.slice_tuple_type(rest_type, fixed_length, 0);
            }
        }
        TypeId::NIL
    }

    // Go: checker/relater.go:1915 sliceTupleType
    pub fn slice_tuple_type(&mut self, t: TypeId, index: i32, end_skip_count: i32) -> TypeId {
        let fixed_length = self.target_tuple_type(t).fixed_length;
        let end_index = self.get_type_reference_arity(t) - end_skip_count.max(0);
        if index > fixed_length {
            let rest_array_type = self.get_rest_array_type_of_tuple_type(t);
            if rest_array_type.is_some() {
                return rest_array_type;
            }
            return self.create_tuple_type(&[]);
        }
        if index >= end_index {
            return self.create_tuple_type(&[]);
        }
        let type_arguments = self.get_type_arguments(t);
        let element_infos =
            self.target_tuple_type(t).element_infos[index as usize..end_index as usize].to_vec();
        self.create_tuple_type_ex(
            &type_arguments[index as usize..end_index as usize],
            &element_infos,
            false, /*readonly*/
        )
    }

    // Go: checker/relater.go:1930 getKnownKeysOfTupleType
    pub fn get_known_keys_of_tuple_type(&mut self, t: TypeId) -> TypeId {
        let fixed_length = self.target_tuple_type(t).fixed_length;
        let mut keys: Vec<TypeId> = Vec::with_capacity(fixed_length as usize + 1);
        for i in 0..fixed_length {
            keys.push(self.get_string_literal_type(&i.to_string()));
        }
        let array_type = if self.target_tuple_type(t).readonly {
            self.global_readonly_array_type
        } else {
            self.global_array_type
        };
        keys.push(self.get_index_type(array_type));
        self.get_union_type(&keys)
    }

    // Go: checker/relater.go:1940 getRestArrayTypeOfTupleType
    pub fn get_rest_array_type_of_tuple_type(&mut self, t: TypeId) -> TypeId {
        let rest_type = self.get_rest_type_of_tuple_type(t);
        if rest_type.is_some() {
            return self.create_array_type(rest_type);
        }
        TypeId::NIL
    }

    // Go: checker/relater.go:1947 getThisTypeOfSignature
    pub fn get_this_type_of_signature(&mut self, signature: SignatureId) -> TypeId {
        let this_parameter = self.sig(signature).this_parameter;
        if this_parameter.is_some() {
            return self.get_type_of_symbol(this_parameter);
        }
        TypeId::NIL
    }

    // Go: checker/relater.go:1954 isInstantiatedGenericParameter
    pub fn is_instantiated_generic_parameter(&mut self, signature: SignatureId, pos: i32) -> bool {
        let target = self.sig(signature).target;
        if target.is_nil() {
            return false;
        }
        let t = self.try_get_type_at_position(target, pos);
        t.is_some() && self.is_generic_type(t)
    }

    // Go: checker/relater.go:1962 getParameterNameAtPosition
    pub fn get_parameter_name_at_position(&mut self, signature: SignatureId, pos: i32) -> String {
        let parameters = self.sig(signature).parameters.clone();
        let param_count = parameters.len() as i32
            - if self.signature_has_rest_parameter(signature) {
                1
            } else {
                0
            };
        if pos < param_count {
            return self.sym(parameters[pos as usize]).name.to_string();
        }
        let rest_parameter = parameters[param_count as usize];
        let rest_type = self.get_type_of_symbol(rest_parameter);
        if self.is_tuple_type(rest_type) {
            let index = pos - param_count;
            let element_info = self.target_tuple_type(rest_type).element_infos[index as usize];
            return self.get_tuple_element_label(element_info, rest_parameter, index);
        }
        self.sym(rest_parameter).name.to_string()
    }

    // Go: checker/relater.go:1976 getTupleElementLabel
    pub fn get_tuple_element_label(
        &self,
        element_info: TupleElementInfo,
        rest_symbol: SymbolId,
        index: i32,
    ) -> String {
        if element_info.labeled_declaration.is_some() {
            return element_info.labeled_declaration.name().text().to_string();
        }
        if rest_symbol.is_some() {
            let value_declaration = self.sym(rest_symbol).value_declaration;
            if value_declaration.is_some() && is_parameter_declaration(value_declaration) {
                return self.get_tuple_element_label_from_binding_element(
                    value_declaration,
                    index,
                    element_info.flags,
                );
            }
        }
        let root_name = if rest_symbol.is_some() {
            self.sym(rest_symbol).name.to_string()
        } else {
            "arg".to_string()
        };
        root_name + "_" + &index.to_string()
    }

    // Go: checker/relater.go:1992 getTupleElementLabelFromBindingElement
    pub fn get_tuple_element_label_from_binding_element(
        &self,
        node: Node,
        index: i32,
        element_flags: ElementFlags,
    ) -> String {
        if node.name().is_some() {
            match node.name().kind() {
                SyntaxKind::Identifier => {
                    let name = node.name().text().to_string();
                    if has_dot_dot_dot_token(node) {
                        // given
                        //   (...[x, y, ...z]: [number, number, ...number[]]) => ...
                        // this produces
                        //   (x: number, y: number, ...z: number[]) => ...
                        // which preserves rest elements of 'z'

                        // given
                        //   (...[x, y, ...z]: [number, number, ...[...number[], number]]) => ...
                        // this produces
                        //   (x: number, y: number, ...z: number[], z_1: number) => ...
                        // which preserves rest elements of z but gives distinct numbers to fixed elements of 'z'
                        if element_flags.intersects(ElementFlags::VARIABLE) {
                            return name;
                        }
                        return name + "_" + &index.to_string();
                    }
                    // given
                    //   (...[x]: [number]) => ...
                    // this produces
                    //   (x: number) => ...
                    // which preserves fixed elements of 'x'

                    // given
                    //   (...[x]: ...number[]) => ...
                    // this produces
                    //   (x_0: number) => ...
                    // which which numbers fixed elements of 'x' whose tuple element type is variable
                    if element_flags.intersects(ElementFlags::FIXED) {
                        return name;
                    }
                    return name + "_n";
                }
                SyntaxKind::ArrayBindingPattern => {
                    if has_dot_dot_dot_token(node) {
                        let elements = node.name().elements();
                        let last_element = if elements.is_empty() {
                            Node::NIL
                        } else {
                            elements.get(elements.len() - 1)
                        };
                        let last_element_is_binding_element_rest = last_element.is_some()
                            && is_binding_element(last_element)
                            && has_dot_dot_dot_token(last_element);
                        let element_count = elements.len() as i32
                            - if last_element_is_binding_element_rest {
                                1
                            } else {
                                0
                            };
                        if index < element_count {
                            let element = elements.get(index as usize);
                            if is_binding_element(element) {
                                return self.get_tuple_element_label_from_binding_element(
                                    element,
                                    index,
                                    element_flags,
                                );
                            }
                        } else if last_element_is_binding_element_rest {
                            return self.get_tuple_element_label_from_binding_element(
                                last_element,
                                index - element_count,
                                element_flags,
                            );
                        }
                    }
                }
                _ => {}
            }
        }
        "arg_".to_string() + &index.to_string()
    }

    // Go: checker/relater.go:2049 getTypePredicateOfSignature
    pub fn get_type_predicate_of_signature(&mut self, sig: SignatureId) -> TypePredicateId {
        if self.sig(sig).resolved_type_predicate.is_nil() {
            let target = self.sig(sig).target;
            let composite = self.sig(sig).composite.clone();
            if target.is_some() {
                let target_type_predicate = self.get_type_predicate_of_signature(target);
                if target_type_predicate.is_some() {
                    let mapper = self.sig(sig).mapper;
                    let p = self.instantiate_type_predicate(target_type_predicate, mapper);
                    self.sig_mut(sig).resolved_type_predicate = p;
                }
            } else if let Some(composite) = composite {
                let p = self.get_union_or_intersection_type_predicate(
                    &composite.signatures,
                    composite.is_union,
                );
                self.sig_mut(sig).resolved_type_predicate = p;
            } else {
                let declaration = self.sig(sig).declaration;
                if declaration.is_some() {
                    let type_node = declaration.type_();
                    if type_node.is_some() {
                        if is_type_predicate_node(type_node) {
                            let p =
                                self.create_type_predicate_from_type_predicate_node(type_node, sig);
                            self.sig_mut(sig).resolved_type_predicate = p;
                        }
                    } else if is_function_like_declaration(declaration)
                        && {
                            let resolved_return_type = self.sig(sig).resolved_return_type;
                            resolved_return_type.is_nil()
                                || self
                                    .ty(resolved_return_type)
                                    .flags
                                    .intersects(TypeFlags::BOOLEAN)
                        }
                        && self.get_parameter_count(sig) > 0
                    {
                        let no_type_predicate = self.no_type_predicate;
                        self.sig_mut(sig).resolved_type_predicate = no_type_predicate; // avoid infinite loop
                        let p = self.get_type_predicate_from_body(declaration);
                        self.sig_mut(sig).resolved_type_predicate = p;
                    }
                }
            }
            if self.sig(sig).resolved_type_predicate.is_nil() {
                let no_type_predicate = self.no_type_predicate;
                self.sig_mut(sig).resolved_type_predicate = no_type_predicate;
            }
        }
        let resolved = self.sig(sig).resolved_type_predicate;
        if resolved == self.no_type_predicate {
            return TypePredicateId::NIL;
        }
        resolved
    }

    // Go: checker/relater.go:2084 getUnionOrIntersectionTypePredicate
    pub fn get_union_or_intersection_type_predicate(
        &mut self,
        signatures: &[SignatureId],
        is_union: bool,
    ) -> TypePredicateId {
        let mut last = TypePredicateId::NIL;
        let mut types: Vec<TypeId> = Vec::new();
        for &sig in signatures {
            let pred = self.get_type_predicate_of_signature(sig);
            if pred.is_some() {
                // Constituent type predicates must all have matching kinds. We don't create composite type predicates for assertions.
                let kind = self.pred(pred).kind;
                if kind != TypePredicateKind::THIS && kind != TypePredicateKind::IDENTIFIER
                    || last.is_some() && !self.type_predicate_kinds_match(last, pred)
                {
                    return TypePredicateId::NIL;
                }
                last = pred;
                types.push(self.pred(pred).t);
            } else {
                // In composite union signatures we permit and ignore signatures with a return type `false`.
                let mut return_type = TypeId::NIL;
                if is_union {
                    return_type = self.get_return_type_of_signature(sig);
                }
                if return_type != self.false_type && return_type != self.regular_false_type {
                    return TypePredicateId::NIL;
                }
            }
        }
        if last.is_nil() {
            return TypePredicateId::NIL;
        }
        let composite_type =
            self.get_union_or_intersection_type(&types, is_union, UnionReduction::LITERAL);
        let (kind, parameter_name, parameter_index) = {
            let p = self.pred(last);
            (p.kind, p.parameter_name.clone(), p.parameter_index)
        };
        self.new_type_predicate(kind, &parameter_name, parameter_index, composite_type)
    }

    // Go: checker/relater.go:2114 typePredicateKindsMatch
    pub fn type_predicate_kinds_match(&self, a: TypePredicateId, b: TypePredicateId) -> bool {
        let (a, b) = (self.pred(a), self.pred(b));
        a.kind == b.kind && a.parameter_index == b.parameter_index
    }

    // Go: checker/relater.go:2118 createTypePredicateFromTypePredicateNode
    pub fn create_type_predicate_from_type_predicate_node(
        &mut self,
        node: Node,
        signature: SignatureId,
    ) -> TypePredicateId {
        let predicate_type_node = node.type_();
        let mut t = TypeId::NIL;
        if predicate_type_node.is_some() {
            t = self.get_type_from_type_node(predicate_type_node);
        }
        let parameter_name = node.parameter_name();
        let asserts_modifier = node.asserts_modifier();
        if is_this_type_node(parameter_name) {
            let kind = if asserts_modifier.is_some() {
                TypePredicateKind::ASSERTS_THIS
            } else {
                TypePredicateKind::THIS
            };
            return self.new_type_predicate(
                kind, "", /*parameterName*/
                0,  /*parameterIndex*/
                t,
            );
        }
        let kind = if asserts_modifier.is_some() {
            TypePredicateKind::ASSERTS_IDENTIFIER
        } else {
            TypePredicateKind::IDENTIFIER
        };
        let name = parameter_name.text();
        let index = self
            .sig(signature)
            .parameters
            .iter()
            .position(|&p| self.sym(p).name == name)
            .map_or(-1, |i| i as i32);
        self.new_type_predicate(kind, name, index, t)
    }

    // Go: checker/relater.go:2134 instantiateTypePredicate
    pub fn instantiate_type_predicate(
        &mut self,
        predicate: TypePredicateId,
        mapper: MapperId,
    ) -> TypePredicateId {
        let predicate_type = self.pred(predicate).t;
        let t = self.instantiate_type(predicate_type, mapper);
        if t == predicate_type {
            return predicate;
        }
        let (kind, parameter_name, parameter_index) = {
            let p = self.pred(predicate);
            (p.kind, p.parameter_name.clone(), p.parameter_index)
        };
        self.new_type_predicate(kind, &parameter_name, parameter_index, t)
    }

    // Go: checker/relater.go:2142 newTypePredicate
    pub fn new_type_predicate(
        &mut self,
        kind: TypePredicateKind,
        parameter_name: &str,
        parameter_index: i32,
        t: TypeId,
    ) -> TypePredicateId {
        let id = TypePredicateId(
            u32::try_from(self.type_predicates.len()).expect("type predicate overflow"),
        );
        self.type_predicates.push(TypePredicate {
            kind,
            parameter_index,
            parameter_name: parameter_name.to_string(),
            t,
        });
        id
    }

    // Go: checker/relater.go:2146 isResolvingReturnTypeOfSignature
    pub fn is_resolving_return_type_of_signature(&mut self, signature: SignatureId) -> bool {
        if let Some(composite) = self.sig(signature).composite.clone() {
            for &s in &composite.signatures {
                if self.is_resolving_return_type_of_signature(s) {
                    return true;
                }
            }
        }
        self.sig(signature).resolved_return_type.is_nil()
            && self.find_resolution_cycle_start_index(
                TypeSystemEntity::Signature(signature),
                TypeSystemPropertyName::RESOLVED_RETURN_TYPE,
            ) >= 0
    }

    // Go: checker/relater.go:2153 findMatchingSignatures
    pub fn find_matching_signatures(
        &mut self,
        signature_lists: &[Vec<SignatureId>],
        signature: SignatureId,
        list_index: i32,
    ) -> Vec<SignatureId> {
        if !self.sig(signature).type_parameters.is_empty() {
            // We require an exact match for generic signatures, so we only return signatures from the first
            // signature list and only if they have exact matches in the other signature lists.
            if list_index > 0 {
                return Vec::new();
            }
            for i in 1..signature_lists.len() {
                if self
                    .find_matching_signature(
                        &signature_lists[i],
                        signature,
                        false, /*partialMatch*/
                        false, /*ignoreThisTypes*/
                        false, /*ignoreReturnTypes*/
                    )
                    .is_nil()
                {
                    return Vec::new();
                }
            }
            return vec![signature];
        }
        let mut result: Vec<SignatureId> = Vec::new();
        for i in 0..signature_lists.len() {
            // Allow matching non-generic signatures to have excess parameters (as a fallback if exact parameter match is not found) and different return types.
            // Prefer matching this types if possible.
            let mut match_: SignatureId;
            if i as i32 == list_index {
                match_ = signature;
            } else {
                match_ = self.find_matching_signature(
                    &signature_lists[i],
                    signature,
                    false, /*partialMatch*/
                    false, /*ignoreThisTypes*/
                    true,  /*ignoreReturnTypes*/
                );
                if match_.is_nil() {
                    match_ = self.find_matching_signature(
                        &signature_lists[i],
                        signature,
                        true,  /*partialMatch*/
                        false, /*ignoreThisTypes*/
                        true,  /*ignoreReturnTypes*/
                    );
                }
            }
            if match_.is_nil() {
                return Vec::new();
            }
            if !result.contains(&match_) {
                result.push(match_);
            }
        }
        result
    }

    // Go: checker/relater.go:2188 findMatchingSignature
    pub fn find_matching_signature(
        &mut self,
        signature_list: &[SignatureId],
        signature: SignatureId,
        partial_match: bool,
        ignore_this_types: bool,
        ignore_return_types: bool,
    ) -> SignatureId {
        let mut compare_types = |c: &mut Checker, s: TypeId, t: TypeId| -> Ternary {
            if partial_match {
                c.compare_types_subtype_of(s, t)
            } else {
                c.compare_types_identical(s, t)
            }
        };
        for &s in signature_list {
            if self.compare_signatures_identical(
                s,
                signature,
                partial_match,
                ignore_this_types,
                ignore_return_types,
                &mut compare_types,
            ) != Ternary::FALSE
            {
                return s;
            }
        }
        SignatureId::NIL
    }

    /**
     * See signatureRelatedTo, compareSignaturesIdentical
     */
    // Go: checker/relater.go:2201 compareSignaturesIdentical
    pub fn compare_signatures_identical(
        &mut self,
        source: SignatureId,
        target: SignatureId,
        partial_match: bool,
        ignore_this_types: bool,
        ignore_return_types: bool,
        compare_types: &mut dyn FnMut(&mut Checker, TypeId, TypeId) -> Ternary,
    ) -> Ternary {
        let mut source = source;
        if source == target {
            return Ternary::TRUE;
        }
        if !self.is_matching_signature(source, target, partial_match) {
            return Ternary::FALSE;
        }
        // Check that the two signatures have the same number of type parameters.
        let source_type_parameters = self.sig(source).type_parameters.clone();
        let target_type_parameters = self.sig(target).type_parameters.clone();
        if source_type_parameters.len() != target_type_parameters.len() {
            return Ternary::FALSE;
        }
        // Check that type parameter constraints and defaults match. If they do, instantiate the source
        // signature with the type parameters of the target signature and continue the comparison.
        if !target_type_parameters.is_empty() {
            let mapper = self.new_type_mapper(&source_type_parameters, &target_type_parameters);
            for i in 0..target_type_parameters.len() {
                let s = source_type_parameters[i];
                let t = target_type_parameters[i];
                let matches = s == t || {
                    let sc = self.get_constraint_or_unknown_from_type_parameter(s);
                    let sc = self.instantiate_type(sc, mapper);
                    let tc = self.get_constraint_or_unknown_from_type_parameter(t);
                    compare_types(self, sc, tc) != Ternary::FALSE && {
                        let sd = self.get_default_or_unknown_from_type_parameter(s);
                        let sd = self.instantiate_type(sd, mapper);
                        let td = self.get_default_or_unknown_from_type_parameter(t);
                        compare_types(self, sd, td) != Ternary::FALSE
                    }
                };
                if !matches {
                    return Ternary::FALSE;
                }
            }
            source =
                self.instantiate_signature_ex(source, mapper, true /*eraseTypeParameters*/);
        }
        let mut result = Ternary::TRUE;
        if !ignore_this_types {
            let source_this_type = self.get_this_type_of_signature(source);
            if source_this_type.is_some() {
                let target_this_type = self.get_this_type_of_signature(target);
                if target_this_type.is_some() {
                    let related = compare_types(self, source_this_type, target_this_type);
                    if related == Ternary::FALSE {
                        return Ternary::FALSE;
                    }
                    result &= related;
                }
            }
        }
        let target_parameter_count = self.get_parameter_count(target);
        for i in 0..target_parameter_count {
            let s = self.get_type_at_position(source, i);
            let t = self.get_type_at_position(target, i);
            let related = compare_types(self, t, s);
            if related == Ternary::FALSE {
                return Ternary::FALSE;
            }
            result &= related;
        }
        if !ignore_return_types {
            let source_type_predicate = self.get_type_predicate_of_signature(source);
            let target_type_predicate = self.get_type_predicate_of_signature(target);
            if source_type_predicate.is_some() || target_type_predicate.is_some() {
                result &= self.compare_type_predicates_identical(
                    source_type_predicate,
                    target_type_predicate,
                    compare_types,
                );
            } else {
                let source_return_type = self.get_return_type_of_signature(source);
                let target_return_type = self.get_return_type_of_signature(target);
                result &= compare_types(self, source_return_type, target_return_type);
            }
        }
        result
    }

    // Go: checker/relater.go:2261 isMatchingSignature
    pub fn is_matching_signature(
        &mut self,
        source: SignatureId,
        target: SignatureId,
        partial_match: bool,
    ) -> bool {
        let source_parameter_count = self.get_parameter_count(source);
        let target_parameter_count = self.get_parameter_count(target);
        let source_min_argument_count = self.get_min_argument_count(source);
        let target_min_argument_count = self.get_min_argument_count(target);
        let source_has_rest_parameter = self.has_effective_rest_parameter(source);
        let target_has_rest_parameter = self.has_effective_rest_parameter(target);
        // A source signature matches a target signature if the two signatures have the same number of required,
        // optional, and rest parameters.
        if source_parameter_count == target_parameter_count
            && source_min_argument_count == target_min_argument_count
            && source_has_rest_parameter == target_has_rest_parameter
        {
            return true;
        }
        // A source signature partially matches a target signature if the target signature has no fewer required
        // parameters
        if partial_match && source_min_argument_count <= target_min_argument_count {
            return true;
        }
        false
    }

    // Go: checker/relater.go:2281 compareTypeParametersIdentical
    pub fn compare_type_parameters_identical(
        &mut self,
        source_params: &[TypeId],
        target_params: &[TypeId],
    ) -> bool {
        if source_params.len() != target_params.len() {
            return false;
        }
        let mapper = self.new_type_mapper(target_params, source_params);
        for i in 0..source_params.len() {
            let source = source_params[i];
            let target = target_params[i];
            if source == target {
                continue;
            }
            // We instantiate the target type parameter constraints into the source types so we can recognize `<T, U extends T>` as the same as `<A, B extends A>`
            let mut source_constraint = self.get_constraint_from_type_parameter(source);
            if source_constraint.is_nil() {
                source_constraint = self.unknown_type;
            }
            let mut target_constraint = self.get_constraint_from_type_parameter(target);
            if target_constraint.is_nil() {
                target_constraint = self.unknown_type;
            }
            let target_constraint = self.instantiate_type(target_constraint, mapper);
            if !self.is_type_identical_to(source_constraint, target_constraint) {
                return false;
            }
            // We don't compare defaults - we just use the type parameter defaults from the first signature that seems to match.
            // It might make sense to combine these defaults in the future, but doing so intelligently requires knowing
            // if the parameter is used covariantly or contravariantly (so we intersect if it's used like a parameter or union if used like a return type)
            // and, since it's just an inference _default_, just picking one arbitrarily works OK.
        }
        true
    }

    // Go: checker/relater.go:2304 compareTypePredicatesIdentical
    pub fn compare_type_predicates_identical(
        &mut self,
        source: TypePredicateId,
        target: TypePredicateId,
        compare_types: &mut dyn FnMut(&mut Checker, TypeId, TypeId) -> Ternary,
    ) -> Ternary {
        if source.is_nil() || target.is_nil() || !self.type_predicate_kinds_match(source, target) {
            return Ternary::FALSE;
        }
        let (source_type, target_type) = (self.pred(source).t, self.pred(target).t);
        if source_type == target_type {
            return Ternary::TRUE;
        }
        if source_type.is_some() && target_type.is_some() {
            return compare_types(self, source_type, target_type);
        }
        Ternary::FALSE
    }

    // Go: checker/relater.go:2316 getEffectiveConstraintOfIntersection
    pub fn get_effective_constraint_of_intersection(
        &mut self,
        types: &[TypeId],
        target_is_union: bool,
    ) -> TypeId {
        let mut constraints: Vec<TypeId> = Vec::new();
        let mut has_disjoint_domain_type = false;
        for &t in types {
            if self.ty(t).flags.intersects(TypeFlags::INSTANTIABLE) {
                // We keep following constraints as long as we have an instantiable type that is known
                // not to be circular or infinite (hence we stop on index access types).
                let mut constraint = self.get_constraint_of_type(t);
                while constraint.is_some()
                    && self.ty(constraint).flags.intersects(
                        TypeFlags::TYPE_PARAMETER | TypeFlags::INDEX | TypeFlags::CONDITIONAL,
                    )
                {
                    constraint = self.get_constraint_of_type(constraint);
                }
                if constraint.is_some() {
                    constraints.push(constraint);
                    if target_is_union {
                        constraints.push(t);
                    }
                }
            } else if self.ty(t).flags.intersects(TypeFlags::DISJOINT_DOMAINS)
                || self.is_empty_anonymous_object_type(t)
            {
                has_disjoint_domain_type = true;
            }
        }
        // If the target is a union type or if we are intersecting with types belonging to one of the
        // disjoint domains, we may end up producing a constraint that hasn't been examined before.
        // PORT: Go `constraints != nil`; constraints are only ever appended, so non-nil is non-empty.
        if !constraints.is_empty() && (target_is_union || has_disjoint_domain_type) {
            if has_disjoint_domain_type {
                // We add any types belong to one of the disjoint domains because they might cause the final
                // intersection operation to reduce the union constraints.
                for &t in types {
                    if self.ty(t).flags.intersects(TypeFlags::DISJOINT_DOMAINS)
                        || self.is_empty_anonymous_object_type(t)
                    {
                        constraints.push(t);
                    }
                }
            }
            // The source types were normalized; ensure the result is normalized too.
            let intersection = self.get_intersection_type_ex(
                &constraints,
                IntersectionFlags::NO_CONSTRAINT_REDUCTION,
                None,
            );
            return self.get_normalized_type(intersection, false /*writing*/);
        }
        TypeId::NIL
    }

    // Go: checker/relater.go:2355 templateLiteralTypesDefinitelyUnrelated
    pub fn template_literal_types_definitely_unrelated(
        &self,
        source: &TemplateLiteralType,
        target: &TemplateLiteralType,
    ) -> bool {
        // Two template literal types with differences in their starting or ending text spans are definitely unrelated.
        // PORT: Go slices strings by byte, so this compares the Go bytes of
        // the port forms (see `scanner_util::GO_STRING_MARKER`).
        let source_start = go_bytes_of(&source.texts[0], source.go_plain);
        let target_start = go_bytes_of(&target.texts[0], target.go_plain);
        let source_end = go_bytes_of(&source.texts[source.texts.len() - 1], source.go_plain);
        let target_end = go_bytes_of(&target.texts[target.texts.len() - 1], target.go_plain);
        let start_len = source_start.len().min(target_start.len());
        let end_len = source_end.len().min(target_end.len());
        source_start[..start_len] != target_start[..start_len]
            || source_end[source_end.len() - end_len..] != target_end[target_end.len() - end_len..]
    }

    // PORT: perf (chkmid1), no Go function. True when the string literal
    // `source` fails the start and end text test of Go
    // `inferFromLiteralPartsToTemplateLiteral` against the template literal
    // type `target`, so `is_type_matched_by_template_literal_type` is false.
    // Most pairs fail there and Go makes no type before the test, so a
    // caller tests it first and clones the template only when it passes.
    pub fn string_literal_misses_template_literal_ends(
        &self,
        source: TypeId,
        target: TypeId,
    ) -> bool {
        let target = self.ty(target).as_template_literal_type();
        let value = go_bytes_of(
            self.get_string_literal_value_ref(source),
            self.string_literal_go_plain(source),
        );
        !literal_part_ends_match(
            &value,
            &value,
            true,
            &go_bytes_of(&target.texts[0], target.go_plain),
            &go_bytes_of(&target.texts[target.texts.len() - 1], target.go_plain),
        )
    }

    // PORT: perf (perffu1), no Go function. True when Go
    // `inferTypesFromTemplateLiteralType(source, target)` returns nil before
    // it makes a type: `source` is not a string or template literal type, or
    // it fails the start and end text test of
    // `inferFromLiteralPartsToTemplateLiteral`. Then
    // `is_type_matched_by_template_literal_type` is false, so a caller tests
    // this before it clones the template literal type `target`.
    pub fn template_literal_match_fails_early(&self, source: TypeId, target: TypeId) -> bool {
        let flags = self.ty(source).flags;
        if flags.intersects(TypeFlags::STRING_LITERAL) {
            return self.string_literal_misses_template_literal_ends(source, target);
        }
        if !flags.intersects(TypeFlags::TEMPLATE_LITERAL) {
            return true;
        }
        let source = self.ty(source).as_template_literal_type();
        let target = self.ty(target).as_template_literal_type();
        if source.texts == target.texts {
            return false;
        }
        !literal_part_ends_match(
            &go_bytes_of(&source.texts[0], source.go_plain),
            &go_bytes_of(&source.texts[source.texts.len() - 1], source.go_plain),
            source.texts.len() == 1,
            &go_bytes_of(&target.texts[0], target.go_plain),
            &go_bytes_of(&target.texts[target.texts.len() - 1], target.go_plain),
        )
    }

    /// Pushes onto `out`, in list order, each type of `templates` that the
    /// string literal `source` can match: every string mapping, and every
    /// template literal whose start and end text test it passes (the
    /// opposite of `string_literal_misses_template_literal_ends`). `ends`
    /// holds the end texts of `templates`.
    pub(crate) fn template_candidates_of_string_literal(
        &self,
        source: TypeId,
        templates: &[TypeId],
        ends: &TemplateLiteralEnds,
        out: &mut SmallVec<[TypeId; 4]>,
    ) {
        let value = go_bytes_of(
            self.get_string_literal_value_ref(source),
            self.string_literal_go_plain(source),
        );
        for (&template, span) in templates.iter().zip(&ends.spans) {
            let Some((offset, start_len, end_len)) = *span else {
                out.push(template);
                continue;
            };
            let start = &ends.bytes[offset..offset + start_len];
            let end = &ends.bytes[offset + start_len..offset + start_len + end_len];
            // The first byte of each text rejects most pairs without a
            // `memcmp` call. The length test comes first, so `value` has
            // those bytes when a text is not empty.
            if value.len() >= start_len + end_len
                && start.first().is_none_or(|b| value[0] == *b)
                && end.last().is_none_or(|b| value[value.len() - 1] == *b)
                && literal_part_ends_match(&value, &value, true, start, end)
            {
                out.push(template);
            }
        }
    }

    // Go: checker/relater.go:2366 isTypeMatchedByTemplateLiteralType
    pub fn is_type_matched_by_template_literal_type(
        &mut self,
        source: TypeId,
        target: &TemplateLiteralType,
        compare_types: &mut dyn FnMut(&mut Checker, TypeId, TypeId, bool) -> Ternary,
    ) -> bool {
        let inferences =
            self.infer_types_from_template_literal_type(source, target, &mut *compare_types);
        // PORT: Go checks `inferences != nil`. A non-nil result is never empty, so
        // an empty Vec stands for nil.
        if !inferences.is_empty() {
            for (i, &inference) in inferences.iter().enumerate() {
                if !self.is_valid_type_for_template_literal_placeholder(
                    inference,
                    target.types[i],
                    &mut *compare_types,
                ) {
                    return false;
                }
            }
            return true;
        }
        false
    }

    // Go: checker/relater.go:2379 inferTypesFromTemplateLiteralType
    // PORT: returns an empty list where Go returns nil. Go never returns a
    // non-nil empty slice here (a match always records at least one type).
    pub fn infer_types_from_template_literal_type(
        &mut self,
        source: TypeId,
        target: &TemplateLiteralType,
        compare_types: &mut dyn FnMut(&mut Checker, TypeId, TypeId, bool) -> Ternary,
    ) -> TemplateLiteralInferences {
        let flags = self.ty(source).flags;
        if flags.intersects(TypeFlags::STRING_LITERAL) {
            // PORT: perf. Go passes `[]string{getStringLiteralValue(source)}`.
            // Here the text is matched in place and read again from the type
            // for each match, so the literal is not cloned. The types are
            // made in the same order as Go `addMatch` makes them.
            // The texts are matched on their Go bytes (see
            // `infer_from_literal_parts_to_template_literal`).
            // PERF: when the value and the target texts have no marker
            // (`go_plain`), they are their own Go bytes and are not converted.
            let source_plain = self.string_literal_go_plain(source);
            let mut spans = LiteralPartMatches::new();
            let source_bytes = [go_bytes_of(
                self.get_string_literal_value_ref(source),
                source_plain,
            )];
            let matched = if target.go_plain {
                match_literal_parts_to_template_literal(&source_bytes, &target.texts, &mut spans)
            } else {
                match_literal_parts_to_template_literal(
                    &source_bytes,
                    &go_string_bytes_list(&target.texts),
                    &mut spans,
                )
            };
            let mut result = TemplateLiteralInferences::with_capacity(spans.len());
            for m in &spans {
                // A string literal is one text part, so every match is inside it.
                debug_assert!(m.seg == 0 && m.s == 0);
                let text = combine_surrogate_pairs(&go_value_from_bytes(
                    &go_bytes_of(self.get_string_literal_value_ref(source), source_plain)
                        [m.pos..m.p],
                ));
                result.push(self.get_string_literal_type(&text));
            }
            if !matched {
                return TemplateLiteralInferences::new();
            }
            return result;
        }
        if flags.intersects(TypeFlags::TEMPLATE_LITERAL) {
            let (source_texts, source_types, source_plain) = {
                let tl = self.ty(source).as_template_literal_type();
                (tl.texts.clone(), tl.types.clone(), tl.go_plain)
            };
            if source_texts == target.texts {
                let mut result = TemplateLiteralInferences::with_capacity(source_types.len());
                for (i, &s) in source_types.iter().enumerate() {
                    let source_constraint = self.get_base_constraint_or_type(s);
                    let target_constraint = self.get_base_constraint_or_type(target.types[i]);
                    if compare_types(
                        self,
                        source_constraint,
                        target_constraint,
                        false, /*partialMatch*/
                    ) != Ternary::FALSE
                    {
                        result.push(s);
                    } else {
                        result.push(self.get_string_like_type_for_type(s));
                    }
                }
                return result;
            }
            return self.infer_from_literal_parts_to_template_literal(
                &source_texts,
                &source_types,
                source_plain,
                target,
            );
        }
        TemplateLiteralInferences::new()
    }

    // This function infers from the text parts and type parts of a source literal to a target template literal. The number
    // of text parts is always one more than the number of type parts, and a source string literal is treated as a source
    // with one text part and zero type parts. The function returns an array of inferred string or template literal types
    // corresponding to the placeholders in the target template literal, or undefined if the source doesn't match the target.
    //
    // We first check that the starting source text part matches the starting target text part, and that the ending source
    // text part ends matches the ending target text part. We then iterate through the remaining target text parts, finding
    // a match for each in the source and inferring string or template literal types created from the segments of the source
    // that occur between the matches. During this iteration, seg holds the index of the current text part in the sourceTexts
    // array and pos holds the current character position in the current text part.
    //
    // Consider inference from type `<<${string}>.<${number}-${number}>>` to type `<${string}.${string}>`, i.e.
    //
    //	sourceTexts = ['<<', '>.<', '-', '>>']
    //	sourceTypes = [string, number, number]
    //	target.texts = ['<', '.', '>']
    //
    // We first match '<' in the target to the start of '<<' in the source and '>' in the target to the end of '>>' in
    // the source. The first match for the '.' in target occurs at character 1 in the source text part at index 1, and thus
    // the first inference is the template literal type `<${string}>`. The remainder of the source makes up the second
    // inference, the template literal type `<${number}-${number}>`.
    // Go: checker/relater.go:2419 inferFromLiteralPartsToTemplateLiteral
    // PORT: returns an empty list where Go returns nil (a match is never
    // empty). `match_literal_parts_to_template_literal` does the text
    // matching first, then the types are made in Go `addMatch` order. The
    // matching makes no types, so the order of type creation is the same.
    // Go matches and slices the bytes of the texts. The texts are port forms
    // (see `scanner_util::GO_STRING_MARKER`), so this works on their Go
    // bytes, and each match is the value form of its bytes
    // (`go_value_from_bytes`). A text without a marker is its own bytes.
    //
    // PORT: perf. `source_plain` is the source's `go_plain`: with it and the
    // target's, the texts are their own Go bytes and are not converted.
    pub fn infer_from_literal_parts_to_template_literal(
        &mut self,
        source_texts: &[String],
        source_types: &[TypeId],
        source_plain: bool,
        target: &TemplateLiteralType,
    ) -> TemplateLiteralInferences {
        let source_bytes: SmallVec<[Cow<'_, [u8]>; 4]> = if source_plain {
            source_texts
                .iter()
                .map(|text| Cow::Borrowed(text.as_bytes()))
                .collect()
        } else {
            go_string_bytes_list(source_texts)
        };
        let mut spans = LiteralPartMatches::new();
        let matched = if target.go_plain {
            match_literal_parts_to_template_literal(&source_bytes, &target.texts, &mut spans)
        } else {
            match_literal_parts_to_template_literal(
                &source_bytes,
                &go_string_bytes_list(&target.texts),
                &mut spans,
            )
        };
        let mut result = TemplateLiteralInferences::with_capacity(spans.len());
        for m in &spans {
            // Go reads `getSourceText(s)`. That text is a prefix of
            // `sourceTexts[s]` and `p` is inside it, so the slices are equal.
            let match_type = if m.s == m.seg {
                self.get_string_literal_type(&combine_surrogate_pairs(&go_value_from_bytes(
                    &source_bytes[m.s][m.pos..m.p],
                )))
            } else {
                let mut match_texts: Vec<String> = Vec::with_capacity(m.s - m.seg + 1);
                match_texts.push(go_value_from_bytes(&source_bytes[m.seg][m.pos..]).into_owned());
                match_texts.extend(source_texts[m.seg + 1..m.s].iter().cloned());
                match_texts.push(go_value_from_bytes(&source_bytes[m.s][..m.p]).into_owned());
                self.get_template_literal_type(&match_texts, &source_types[m.seg..m.s])
            };
            result.push(match_type);
        }
        if !matched {
            return TemplateLiteralInferences::new();
        }
        result
    }

    // Go: checker/relater.go:2503 getStringLikeTypeForType
    pub fn get_string_like_type_for_type(&mut self, t: TypeId) -> TypeId {
        if self
            .ty(t)
            .flags
            .intersects(TypeFlags::ANY | TypeFlags::STRING_LIKE)
        {
            return t;
        }
        self.get_template_literal_type(&[String::new(), String::new()], &[t])
    }

    // Go: checker/relater.go:2510 isValidTypeForTemplateLiteralPlaceholder
    pub fn is_valid_type_for_template_literal_placeholder(
        &mut self,
        source: TypeId,
        target: TypeId,
        compare_types: &mut dyn FnMut(&mut Checker, TypeId, TypeId, bool) -> Ternary,
    ) -> bool {
        let target_flags = self.ty(target).flags;
        let source_flags = self.ty(source).flags;
        if target_flags.intersects(TypeFlags::INTERSECTION) {
            let types = self.ty(target).types_list();
            for t in types {
                if !(t == self.empty_type_literal_type
                    || self.is_valid_type_for_template_literal_placeholder(
                        source,
                        t,
                        &mut *compare_types,
                    ))
                {
                    return false;
                }
            }
            return true;
        }
        if target_flags.intersects(TypeFlags::STRING)
            || compare_types(self, source, target, false) != Ternary::FALSE
        {
            return true;
        }
        if source_flags.intersects(TypeFlags::STRING_LITERAL) {
            let value = self.get_string_literal_value(source);
            return target_flags.intersects(TypeFlags::NUMBER)
                && is_valid_number_string(&value, false /*roundTripOnly*/)
                || target_flags.intersects(TypeFlags::BIG_INT)
                    && is_valid_big_int_string(&value, false /*roundTripOnly*/)
                || target_flags.intersects(TypeFlags::BOOLEAN_LITERAL | TypeFlags::NULLABLE)
                    && value == self.ty(target).as_intrinsic_type().intrinsic_name
                || target_flags.intersects(TypeFlags::STRING_MAPPING)
                    && self.is_member_of_string_mapping(source, target)
                || target_flags.intersects(TypeFlags::TEMPLATE_LITERAL)
                    && !self.string_literal_misses_template_literal_ends(source, target)
                    && {
                        let template = self.ty(target).as_template_literal_type().clone();
                        self.is_type_matched_by_template_literal_type(
                            source,
                            &template,
                            &mut *compare_types,
                        )
                    };
        }
        if source_flags.intersects(TypeFlags::TEMPLATE_LITERAL) {
            let (texts_match, first_type) = {
                let tl = self.ty(source).as_template_literal_type();
                (
                    tl.texts.len() == 2 && tl.texts[0].is_empty() && tl.texts[1].is_empty(),
                    tl.types.first().copied(),
                )
            };
            return texts_match
                && compare_types(self, first_type.unwrap_or_default(), target, false)
                    != Ternary::FALSE;
        }
        false
    }

    // Go: checker/relater.go:2532 isMemberOfStringMapping
    pub fn is_member_of_string_mapping(&mut self, source: TypeId, target: TypeId) -> bool {
        let target_flags = self.ty(target).flags;
        if target_flags.intersects(TypeFlags::ANY) {
            return true;
        }
        if target_flags.intersects(TypeFlags::STRING | TypeFlags::TEMPLATE_LITERAL) {
            return self.is_type_assignable_to(source, target);
        }
        if target_flags.intersects(TypeFlags::STRING_MAPPING) {
            // We need to see whether applying the same mappings of the target
            // onto the source would produce an identical type *and* that
            // it's compatible with the inner-most non-string-mapped type.
            //
            // The intuition here is that if same mappings don't affect the source at all,
            // and the source is compatible with the unmapped target, then they must
            // still reside in the same domain.
            let (mapped, inner) = self.apply_target_string_mapping_to_source(source, target);
            return mapped == source && self.is_member_of_string_mapping(source, inner);
        }
        false
    }

    // Go: checker/relater.go:2552 applyTargetStringMappingToSource
    pub fn apply_target_string_mapping_to_source(
        &mut self,
        source: TypeId,
        target: TypeId,
    ) -> (TypeId, TypeId) {
        let mut source = source;
        let mut inner = self.ty(target).as_string_mapping_type().target;
        if self.ty(inner).flags.intersects(TypeFlags::STRING_MAPPING) {
            (source, inner) = self.apply_target_string_mapping_to_source(source, inner);
        }
        let symbol = self.ty(target).symbol;
        (self.get_string_mapping_type(symbol, source), inner)
    }
}

// Go: checker/relater.go:2560 visibilityToString
pub fn visibility_to_string(flags: ModifierFlags) -> String {
    if flags == ModifierFlags::PRIVATE {
        return "private".to_string();
    }
    if flags == ModifierFlags::PROTECTED {
        return "protected".to_string();
    }
    "public".to_string()
}

// Go: checker/relater.go:2570 errorState
#[derive(Clone, Debug, Default)]
pub struct ErrorState {
    pub error_chain: Option<Rc<ErrorChain>>,
    pub related_info: RelatedInfo,
}

/// Go `[]*ast.Diagnostic` of `errorState.relatedInfo` and
/// `Relater.relatedInfo`.
///
/// PERF: shared, so a saved error state (`get_error_state`) and each copy of
/// it copy a pointer, as Go copies a slice header. A push copies the list
/// when a saved state holds it, so a saved state keeps the list it saw, as
/// the port's `Vec` copies did. Empty is `None`, with no allocation.
#[derive(Clone, Debug, Default)]
pub struct RelatedInfo(Option<Rc<Vec<Diagnostic>>>);

impl RelatedInfo {
    /// Go `append(relatedInfo, d)`.
    pub fn push(&mut self, d: Diagnostic) {
        Rc::make_mut(self.0.get_or_insert_with(Default::default)).push(d);
    }

    pub fn as_slice(&self) -> &[Diagnostic] {
        self.0.as_deref().map_or(&[], Vec::as_slice)
    }

    pub fn clear(&mut self) {
        self.0 = None;
    }
}

// Go: checker/relater.go:2575 ErrorChain
// PORT: Go `*ErrorChain` nodes are immutable once built and compared by
// pointer, so they are `Rc<ErrorChain>` (nil is `None`; compare with
// `Rc::ptr_eq`). Go `args []any` are diagnostic args, so `Vec<String>`.
#[derive(Debug)]
pub struct ErrorChain {
    pub next: Option<Rc<ErrorChain>>,
    pub message: &'static Message,
    pub args: Vec<String>,
}

// Go: checker/relater.go:2581 Relater
// PORT: The Go `c *Checker` field is not stored. Every `(r *Relater)` method
// is an `impl Checker` method that takes the handle `r: &Rc<RefCell<Relater>>`
// right after `self` (same convention as relater_p4/p5). Borrows of `r` are
// kept short and never held across a checker call. Go `relation` is `kind`
// (see `RelationKind`); a pooled relater keeps the last kind in place of Go
// nil. `next` links the free list in `Checker::free_relater`.
#[derive(Default)]
pub struct Relater {
    pub kind: RelationKind,
    pub error_node: Node,
    pub error_chain: Option<Rc<ErrorChain>>,
    pub related_info: RelatedInfo,
    pub maybe_keys: Vec<RelationKey>,
    // PORT: perf. Go keeps the set in sync with `maybeKeys` at all times.
    // Here it is empty while `maybe_keys.len() <= MAYBE_KEYS_SCAN_LIMIT` and
    // holds exactly the stack keys above that. Use `maybe_keys_contain`,
    // `push_maybe_key` and `truncate_maybe_keys`, not the fields directly.
    pub maybe_keys_set: RelationKeySet,
    pub source_stack: Vec<TypeId>,
    pub target_stack: Vec<TypeId>,
    // PORT: perf. Not in Go. `stack_recursion_key` of a prefix of
    // `source_stack` and `target_stack`, one word per entry.
    // `is_deeply_nested_relater_type` fills them to the full stack when it
    // reaches its scan; a pop truncates them to the stack length.
    pub source_ids: Vec<RecursionKey>,
    pub target_ids: Vec<RecursionKey>,
    pub maybe_count: i32,
    pub source_depth: i32,
    pub target_depth: i32,
    pub expanding_flags: ExpandingFlags,
    pub overflow: bool,
    pub relation_count: i32,
    pub next: Option<Rc<RefCell<Relater>>>,
}

// PORT: perf. Up to this many maybe keys, membership is a linear scan of
// `maybe_keys` and `maybe_keys_set` stays empty. The stack is almost always
// this small, and the scan is cheaper than the hash probe and the set writes.
const MAYBE_KEYS_SCAN_LIMIT: usize = 16;

// PORT: perf. A set that grew past this capacity is dropped when the stack
// becomes small again, because hashbrown never shrinks and `clear` costs time
// in its capacity.
const MAYBE_KEYS_SET_KEEP_CAPACITY: usize = 256;

// Empties the maybe keys set. Drops a large allocation instead of keeping it,
// so an empty set never holds more than the keep capacity.
fn clear_maybe_keys_set(set: &mut RelationKeySet) {
    // Clearing an empty set still costs time in its capacity.
    if set.is_empty() {
        return;
    }
    if set.capacity() > MAYBE_KEYS_SET_KEEP_CAPACITY {
        *set = RelationKeySet::default();
    } else {
        set.clear();
    }
}

impl Relater {
    // PORT: perf. Replaces Go `r.maybeKeysSet.Has(key)` (relater.go:3121,
    // 3129) with the same result: the stack keys are unique (a key is pushed
    // only when absent), and the set mirrors them above the scan limit.
    #[inline]
    pub fn maybe_keys_contain(&self, key: &RelationKey) -> bool {
        if self.maybe_keys.len() <= MAYBE_KEYS_SCAN_LIMIT {
            self.maybe_keys.contains(key)
        } else {
            self.maybe_keys_set.contains(key)
        }
    }

    // PORT: perf. Replaces the Go `maybeKeys` append and `maybeKeysSet.Add`
    // (relater.go:3140-3141). The set is written only above the scan limit. The
    // push that crosses the limit fills it with every stack key.
    #[inline]
    pub fn push_maybe_key(&mut self, key: RelationKey) {
        self.maybe_keys.push(key);
        let len = self.maybe_keys.len();
        if len == MAYBE_KEYS_SCAN_LIMIT + 1 {
            self.maybe_keys_set.extend(self.maybe_keys.iter().copied());
        } else if len > MAYBE_KEYS_SCAN_LIMIT {
            self.maybe_keys_set.insert(key);
        }
        self.debug_check_maybe_keys();
    }

    // PORT: perf. Replaces the `maybeKeysSet.Delete` calls and the
    // `maybeKeys[:maybeStart]` cut of Go resetMaybeStack (relater.go:3201).
    // A cut to the scan limit or less clears the set in one step.
    pub fn truncate_maybe_keys(&mut self, maybe_start: usize) {
        if maybe_start <= MAYBE_KEYS_SCAN_LIMIT {
            clear_maybe_keys_set(&mut self.maybe_keys_set);
        } else {
            for key in &self.maybe_keys[maybe_start..] {
                self.maybe_keys_set.remove(key);
            }
        }
        self.maybe_keys.truncate(maybe_start);
        self.debug_check_maybe_keys();
    }

    fn debug_check_maybe_keys(&self) {
        if cfg!(debug_assertions) {
            if self.maybe_keys.len() <= MAYBE_KEYS_SCAN_LIMIT {
                debug_assert!(self.maybe_keys_set.is_empty());
            } else {
                debug_assert_eq!(self.maybe_keys_set.len(), self.maybe_keys.len());
                debug_assert!(
                    self.maybe_keys
                        .iter()
                        .all(|key| self.maybe_keys_set.contains(key))
                );
            }
        }
    }
}

impl Checker {
    // Go: checker/relater.go:2600 getRelater
    // PORT: perf. The pool head and `next` are moved out, not cloned. Go
    // leaves `r.next` set while `r` is in use; nothing reads it then, and
    // `putRelater` sets it again.
    pub fn get_relater(&mut self) -> Rc<RefCell<Relater>> {
        let r = match self.free_relater.take() {
            Some(r) => r,
            None => Rc::new(RefCell::new(Relater::default())),
        };
        self.free_relater = r.borrow_mut().next.take();
        r
    }

    // Go: checker/relater.go:2609 putRelater
    // PORT: the caller must drop any borrow of `r` before calling.
    pub fn put_relater(&mut self, r: Rc<RefCell<Relater>>) {
        {
            let mut rb = r.borrow_mut();
            // PORT: perf. Go rebuilds the struct (`*r = Relater{...}`). This
            // resets each field in place, because `..Relater::default()`
            // allocates and drops a default `Rc<RefCell<Relation>>` on every
            // put. The destructure names every field, so a new field does not
            // compile until it is reset here.
            let Relater {
                // PORT: Go sets `relation` to nil. The pooled relater keeps
                // the last kind. The next `getRelater` user always sets it
                // first.
                kind: _,
                error_node,
                error_chain,
                related_info,
                maybe_keys,
                maybe_keys_set,
                source_stack,
                target_stack,
                source_ids,
                target_ids,
                maybe_count,
                source_depth,
                target_depth,
                expanding_flags,
                overflow,
                relation_count,
                next,
            } = &mut *rb;
            *error_node = Node::NIL;
            *error_chain = None;
            related_info.clear();
            maybe_keys.clear();
            // PORT: resetMaybeStack usually removed every key already.
            clear_maybe_keys_set(maybe_keys_set);
            source_stack.clear();
            target_stack.clear();
            source_ids.clear();
            target_ids.clear();
            *maybe_count = 0;
            *source_depth = 0;
            *target_depth = 0;
            *expanding_flags = ExpandingFlags::NONE;
            *overflow = false;
            *relation_count = 0;
            *next = self.free_relater.take();
        }
        self.free_relater = Some(r);
    }
}

impl Checker {
    // Go: checker/relater.go:2622 isRelatedToSimple
    pub fn is_related_to_simple(
        &mut self,
        r: &Rc<RefCell<Relater>>,
        source: TypeId,
        target: TypeId,
    ) -> Ternary {
        self.is_related_to_ex(
            r,
            source,
            target,
            RecursionFlags::BOTH,
            false, /*reportErrors*/
            None,  /*headMessage*/
            IntersectionState::NONE,
        )
    }

    // Go: checker/relater.go:2626 isRelatedToWorker
    pub fn is_related_to_worker(
        &mut self,
        r: &Rc<RefCell<Relater>>,
        source: TypeId,
        target: TypeId,
        report_errors: bool,
    ) -> Ternary {
        self.is_related_to_ex(
            r,
            source,
            target,
            RecursionFlags::BOTH,
            report_errors,
            None,
            IntersectionState::NONE,
        )
    }

    // Go: checker/relater.go:2630 isRelatedTo
    pub fn is_related_to(
        &mut self,
        r: &Rc<RefCell<Relater>>,
        source: TypeId,
        target: TypeId,
        recursion_flags: RecursionFlags,
        report_errors: bool,
    ) -> Ternary {
        self.is_related_to_ex(
            r,
            source,
            target,
            recursion_flags,
            report_errors,
            None,
            IntersectionState::NONE,
        )
    }

    // Go: checker/relater.go:2634 isRelatedToEx
    #[allow(clippy::too_many_arguments)]
    pub fn is_related_to_ex(
        &mut self,
        r: &Rc<RefCell<Relater>>,
        original_source: TypeId,
        original_target: TypeId,
        recursion_flags: RecursionFlags,
        report_errors: bool,
        head_message: Option<&'static Message>,
        intersection_state: IntersectionState,
    ) -> Ternary {
        if original_source == original_target {
            return Ternary::TRUE;
        }
        let kind = r.borrow().kind;
        let is_comparable = kind == RelationKind::Comparable;
        // Before normalization: if `source` is type an object type, and `target` is primitive,
        // skip all the checks we don't need and just return `isSimpleTypeRelatedTo` result
        if self.ty(original_source).flags.intersects(TypeFlags::OBJECT)
            && self
                .ty(original_target)
                .flags
                .intersects(TypeFlags::PRIMITIVE)
        {
            if is_comparable
                && !self.ty(original_target).flags.intersects(TypeFlags::NEVER)
                && self.is_simple_type_related_to_kind(original_target, original_source, kind, None)
                || if report_errors {
                    self.is_simple_type_related_to_kind(
                        original_source,
                        original_target,
                        kind,
                        Some(&mut |c: &mut Checker,
                                   message: &'static Message,
                                   args: Vec<String>| {
                            c.report_error(r, message, args)
                        }),
                    )
                } else {
                    self.is_simple_type_related_to_kind(
                        original_source,
                        original_target,
                        kind,
                        None,
                    )
                }
            {
                return Ternary::TRUE;
            }
            if report_errors {
                self.report_error_results(
                    r,
                    original_source,
                    original_target,
                    original_source,
                    original_target,
                    head_message,
                );
            }
            return Ternary::FALSE;
        }
        // Normalize the source and target types: Turn fresh literal types into regular literal types,
        // turn deferred type references into regular type references, simplify indexed access and
        // conditional types, and resolve substitution types to either the substitution (on the source
        // side) or the type variable (on the target side).
        let source = self.get_normalized_type(original_source, false /*writing*/);
        let mut target = self.get_normalized_type(original_target, true /*writing*/);
        if source == target {
            return Ternary::TRUE;
        }
        if kind == RelationKind::Identity {
            if self.ty(source).flags != self.ty(target).flags {
                return Ternary::FALSE;
            }
            if self.ty(source).flags.intersects(TypeFlags::SINGLETON) {
                return Ternary::TRUE;
            }
            self.trace_unions_or_intersections_too_large(r, source, target);
            return self.recursive_type_related_to(
                r,
                source,
                target,
                false, /*reportErrors*/
                IntersectionState::NONE,
                recursion_flags,
            );
        }
        // We fastpath comparing a type parameter to exactly its constraint, as this is _super_ common,
        // and otherwise, for type parameters in large unions, causes us to need to compare the union to itself,
        // as we break down the _target_ union first, _then_ get the source constraint - so for every
        // member of the target, we attempt to find a match in the source. This avoids that in cases where
        // the target is exactly the constraint.
        if self.ty(source).flags.intersects(TypeFlags::TYPE_PARAMETER)
            && self.get_constraint_of_type(source) == target
        {
            return Ternary::TRUE;
        }
        // See if we're relating a definitely non-nullable type to a union that includes null and/or undefined
        // plus a single non-nullable type. If so, remove null and/or undefined from the target type.
        if self
            .ty(source)
            .flags
            .intersects(TypeFlags::DEFINITELY_NON_NULLABLE)
            && self.ty(target).flags.intersects(TypeFlags::UNION)
        {
            let types = self.ty(target).types_list();
            let mut candidate = TypeId::NIL;
            if types.len() == 2 && self.ty(types[0]).flags.intersects(TypeFlags::NULLABLE) {
                candidate = types[1];
            } else if types.len() == 3
                && self.ty(types[0]).flags.intersects(TypeFlags::NULLABLE)
                && self.ty(types[1]).flags.intersects(TypeFlags::NULLABLE)
            {
                candidate = types[2];
            }
            if candidate.is_some() && !self.ty(candidate).flags.intersects(TypeFlags::NULLABLE) {
                target = self.get_normalized_type(candidate, true /*writing*/);
                if source == target {
                    return Ternary::TRUE;
                }
            }
        }
        if is_comparable
            && !self.ty(target).flags.intersects(TypeFlags::NEVER)
            && self.is_simple_type_related_to_kind(target, source, kind, None)
            || if report_errors {
                self.is_simple_type_related_to_kind(
                    source,
                    target,
                    kind,
                    Some(
                        &mut |c: &mut Checker, message: &'static Message, args: Vec<String>| {
                            c.report_error(r, message, args)
                        },
                    ),
                )
            } else {
                self.is_simple_type_related_to_kind(source, target, kind, None)
            }
        {
            return Ternary::TRUE;
        }
        if self
            .ty(source)
            .flags
            .intersects(TypeFlags::STRUCTURED_OR_INSTANTIABLE)
            || self
                .ty(target)
                .flags
                .intersects(TypeFlags::STRUCTURED_OR_INSTANTIABLE)
        {
            let is_performing_excess_property_checks = !intersection_state
                .intersects(IntersectionState::TARGET)
                && self.is_object_literal_type(source)
                && self
                    .ty(source)
                    .object_flags
                    .intersects(ObjectFlags::FRESH_LITERAL);
            if is_performing_excess_property_checks
                && self.has_excess_properties(r, source, target, report_errors)
            {
                if report_errors {
                    let error_target = if self.ty(original_target).alias.is_some() {
                        original_target
                    } else {
                        target
                    };
                    self.report_relation_error(r, head_message, source, error_target);
                }
                return Ternary::FALSE;
            }
            let is_performing_common_property_checks =
                (!is_comparable || self.is_unit_type(source))
                    && !intersection_state.intersects(IntersectionState::TARGET)
                    && self.ty(source).flags.intersects(
                        TypeFlags::PRIMITIVE | TypeFlags::OBJECT | TypeFlags::INTERSECTION,
                    )
                    && source != self.global_object_type
                    && self
                        .ty(target)
                        .flags
                        .intersects(TypeFlags::OBJECT | TypeFlags::INTERSECTION)
                    && self.is_weak_type(target)
                    && (self.get_properties_of_type_count(source) != 0
                        || self.type_has_call_or_construct_signatures(source));
            let is_comparing_jsx_attributes = self
                .ty(source)
                .object_flags
                .intersects(ObjectFlags::JSX_ATTRIBUTES);
            if is_performing_common_property_checks
                && !self.has_common_properties(source, target, is_comparing_jsx_attributes)
            {
                if report_errors {
                    let source_string =
                        self.type_to_string_exported(if self.ty(original_source).alias.is_some() {
                            original_source
                        } else {
                            source
                        });
                    let target_string =
                        self.type_to_string_exported(if self.ty(original_target).alias.is_some() {
                            original_target
                        } else {
                            target
                        });
                    let calls = self.get_signatures_of_type(source, SignatureKind::CALL);
                    let constructs = self.get_signatures_of_type(source, SignatureKind::CONSTRUCT);
                    if !calls.is_empty() && {
                        let return_type = self.get_return_type_of_signature(calls[0]);
                        self.is_related_to(
                            r,
                            return_type,
                            target,
                            RecursionFlags::SOURCE,
                            false, /*reportErrors*/
                        ) != Ternary::FALSE
                    } || !constructs.is_empty() && {
                        let return_type = self.get_return_type_of_signature(constructs[0]);
                        self.is_related_to(
                            r,
                            return_type,
                            target,
                            RecursionFlags::SOURCE,
                            false, /*reportErrors*/
                        ) != Ternary::FALSE
                    } {
                        self.report_error(
                            r,
                            diag::Value_of_type_0_has_no_properties_in_common_with_type_1_Did_you_mean_to_call_it,
                            args![source_string, target_string],
                        );
                    } else {
                        self.report_error(
                            r,
                            diag::Type_0_has_no_properties_in_common_with_type_1,
                            args![source_string, target_string],
                        );
                    }
                }
                return Ternary::FALSE;
            }
            self.trace_unions_or_intersections_too_large(r, source, target);
            let skip_caching = self.ty(source).flags.intersects(TypeFlags::UNION)
                && self.ty(source).types().len() < 4
                && !self.ty(target).flags.intersects(TypeFlags::UNION)
                || self.ty(target).flags.intersects(TypeFlags::UNION)
                    && self.ty(target).types().len() < 4
                    && !self
                        .ty(source)
                        .flags
                        .intersects(TypeFlags::STRUCTURED_OR_INSTANTIABLE);
            let result = if skip_caching {
                self.union_or_intersection_related_to(
                    r,
                    source,
                    target,
                    report_errors,
                    intersection_state,
                )
            } else {
                self.recursive_type_related_to(
                    r,
                    source,
                    target,
                    report_errors,
                    intersection_state,
                    recursion_flags,
                )
            };
            if result != Ternary::FALSE {
                return result;
            }
        }
        if report_errors {
            self.report_error_results(
                r,
                original_source,
                original_target,
                source,
                target,
                head_message,
            );
        }
        Ternary::FALSE
    }

    // Go: checker/relater.go:2748 hasExcessProperties
    pub fn has_excess_properties(
        &mut self,
        r: &Rc<RefCell<Relater>>,
        source: TypeId,
        target: TypeId,
        report_errors: bool,
    ) -> bool {
        if !self.is_excess_property_check_target(target)
            || !self.no_implicit_any
                && self
                    .ty(target)
                    .object_flags
                    .intersects(ObjectFlags::JS_LITERAL)
        {
            // Disable excess property checks on JS literals to simulate having an implicit "index signature" - but only outside of noImplicitAny
            return false;
        }
        let is_comparing_jsx_attributes = self
            .ty(source)
            .object_flags
            .intersects(ObjectFlags::JSX_ATTRIBUTES);
        let kind = r.borrow().kind;
        if (kind == RelationKind::Assignable || kind == RelationKind::Comparable)
            && (self.is_type_subset_of(self.global_object_type, target)
                || (!is_comparing_jsx_attributes && self.is_empty_object_type(target)))
        {
            return false;
        }
        let mut reduced_target = target;
        // PORT: Go `checkTypes` is nil unless the target is a union; `None` stands for nil.
        let mut check_types: Option<SharedList<TypeId>> = None;
        if self.ty(target).flags.intersects(TypeFlags::UNION) {
            reduced_target = self.find_matching_discriminant_type(
                source,
                target,
                &mut |c: &mut Checker, s: TypeId, t: TypeId| c.is_related_to_simple(r, s, t),
            );
            if reduced_target.is_nil() {
                reduced_target = self.filter_primitives_if_contains_non_primitive(target);
            }
            check_types = Some(self.ty(reduced_target).distributed());
        }
        let source_symbol = self.ty(source).symbol;
        for prop in self.get_properties_of_type(source) {
            if self.should_check_as_excess_property(prop, source_symbol)
                && !self.is_ignored_jsx_property(source, prop)
            {
                let prop_symbol_name = self.sym(prop).name.clone();
                if !self.is_known_property(
                    reduced_target,
                    &prop_symbol_name,
                    is_comparing_jsx_attributes,
                ) {
                    if report_errors {
                        // Report error in terms of object types in the target as those are the only ones
                        // we check in isKnownProperty.
                        let error_target = self
                            .filter_type(reduced_target, &mut |c: &mut Checker, t: TypeId| {
                                c.is_excess_property_check_target(t)
                            });
                        // We know *exactly* where things went wrong when comparing the types.
                        // Use this property as the error node as this will be more helpful in
                        // reasoning about what went wrong.
                        let error_node = r.borrow().error_node;
                        if error_node.is_nil() {
                            panic!("No errorNode in hasExcessProperties");
                        }
                        if is_jsx_attributes(error_node)
                            || is_jsx_opening_like_element(error_node)
                            || is_jsx_opening_like_element(error_node.parent())
                        {
                            // JsxAttributes has an object-literal flag and undergo same type-assignablity check as normal object-literal.
                            // However, using an object-literal error message will be very confusing to the users so we give different a message.
                            let value_declaration = self.sym(prop).value_declaration;
                            if value_declaration.is_some()
                                && is_jsx_attribute(value_declaration)
                                && get_source_file_of_node(error_node)
                                    == get_source_file_of_node(value_declaration.name())
                            {
                                // Note that extraneous children (as in `<NoChild>extra</NoChild>`) don't pass this check,
                                // since `children` is a Kind.PropertySignature instead of a Kind.JsxAttribute.
                                r.borrow_mut().error_node = value_declaration.name();
                            }
                            let prop_name = self.symbol_to_string(prop);
                            let suggestion_symbol = self
                                .get_suggested_symbol_for_nonexistent_jsx_attribute(
                                    &prop_name,
                                    error_target,
                                );
                            if suggestion_symbol.is_some() {
                                let target_string = self.type_to_string_exported(error_target);
                                let suggestion_string = self.symbol_to_string(suggestion_symbol);
                                self.report_error(
                                    r,
                                    diag::Property_0_does_not_exist_on_type_1_Did_you_mean_2,
                                    args![prop_name, target_string, suggestion_string],
                                );
                            } else {
                                let target_string = self.type_to_string_exported(error_target);
                                self.report_error(
                                    r,
                                    diag::Property_0_does_not_exist_on_type_1,
                                    args![prop_name, target_string],
                                );
                            }
                        } else {
                            // use the property's value declaration if the property is assigned inside the literal itself
                            let mut object_literal_declaration = Node::NIL;
                            if source_symbol.is_some() {
                                object_literal_declaration = self
                                    .sym(source_symbol)
                                    .declarations
                                    .first()
                                    .copied()
                                    .unwrap_or(Node::NIL);
                            }
                            let mut suggestion = String::new();
                            let value_declaration = self.sym(prop).value_declaration;
                            if value_declaration.is_some()
                                && is_object_literal_element(value_declaration)
                                && find_ancestor(value_declaration, |d: Node| {
                                    d == object_literal_declaration
                                })
                                .is_some()
                                && get_source_file_of_node(object_literal_declaration)
                                    == get_source_file_of_node(error_node)
                            {
                                let name = value_declaration.name();
                                r.borrow_mut().error_node = name;
                                if is_identifier(name) {
                                    suggestion = self.get_suggestion_for_nonexistent_property(
                                        name.text(),
                                        error_target,
                                    );
                                }
                            }
                            let prop_string = self.symbol_to_string(prop);
                            let target_string = self.type_to_string_exported(error_target);
                            if !suggestion.is_empty() {
                                self.report_error(
                                    r,
                                    diag::Object_literal_may_only_specify_known_properties_but_0_does_not_exist_in_type_1_Did_you_mean_to_write_2,
                                    args![prop_string, target_string, suggestion],
                                );
                            } else {
                                self.report_error(
                                    r,
                                    diag::Object_literal_may_only_specify_known_properties_and_0_does_not_exist_in_type_1,
                                    args![prop_string, target_string],
                                );
                            }
                        }
                    }
                    return true;
                }
                if let Some(check_types) = &check_types {
                    let prop_type = self.get_type_of_symbol(prop);
                    let target_prop_type =
                        self.get_type_of_property_in_types(check_types, &prop_symbol_name);
                    if self.is_related_to(
                        r,
                        prop_type,
                        target_prop_type,
                        RecursionFlags::BOTH,
                        report_errors,
                    ) == Ternary::FALSE
                    {
                        if report_errors {
                            let prop_string = self.symbol_to_string(prop);
                            self.report_error(
                                r,
                                diag::Types_of_property_0_are_incompatible,
                                args![prop_string],
                            );
                        }
                        return true;
                    }
                }
            }
        }
        false
    }
}

/// One Go `addMatch(s, p)` call of `inferFromLiteralPartsToTemplateLiteral`:
/// the source text from part `seg` at byte `pos` to part `s` at byte `p`.
/// The byte offsets are in the Go bytes of the texts.
#[derive(Clone, Copy)]
struct LiteralPartMatch {
    seg: usize,
    pos: usize,
    s: usize,
    p: usize,
}

type LiteralPartMatches = SmallVec<[LiteralPartMatch; 4]>;

/// The placeholder types that `infer_types_from_template_literal_type` infers.
pub type TemplateLiteralInferences = SmallVec<[TypeId; 4]>;

/// The Go bytes of each port form text (see `scanner_util::GO_STRING_MARKER`).
/// A text without a marker is borrowed.
fn go_string_bytes_list(texts: &[String]) -> SmallVec<[Cow<'_, [u8]>; 4]> {
    texts.iter().map(|text| go_string_bytes(text)).collect()
}

/// PERF: the Go bytes of a port form text whose `go_plain` flag is known.
/// A plain text is its own Go bytes, so it is not scanned.
fn go_bytes_of(text: &str, plain: bool) -> Cow<'_, [u8]> {
    if plain {
        Cow::Borrowed(text.as_bytes())
    } else {
        go_string_bytes(text)
    }
}

/// The start and end texts (Go bytes) of the template literals in a list of
/// template and string mapping types, read once, so that each string
/// literal of a union is tested against them without reading each template
/// again (`template_candidates_of_string_literal`).
// PERF (sortmisc1): no Go type. Go `removeStringLiteralsMatchedByTemplateLiterals`
// tests every (string literal, template) pair, and almost every pair fails
// this test (immich: 22.3 M pairs, none passed).
pub(crate) struct TemplateLiteralEnds {
    /// The start text and then the end text of each template literal.
    bytes: Vec<u8>,
    /// For each type of the list: the offset of its start text in `bytes`
    /// and the lengths of its start and end texts, or `None` for a string
    /// mapping.
    spans: Vec<Option<(usize, usize, usize)>>,
}

impl TemplateLiteralEnds {
    pub(crate) fn new(c: &Checker, templates: &[TypeId]) -> Self {
        let mut bytes = Vec::new();
        let spans = templates
            .iter()
            .map(|&template| {
                if !c.ty(template).flags.intersects(TypeFlags::TEMPLATE_LITERAL) {
                    return None;
                }
                let target = c.ty(template).as_template_literal_type();
                let start = go_bytes_of(&target.texts[0], target.go_plain);
                let end = go_bytes_of(&target.texts[target.texts.len() - 1], target.go_plain);
                let offset = bytes.len();
                bytes.extend_from_slice(&start);
                bytes.extend_from_slice(&end);
                Some((offset, start.len(), end.len()))
            })
            .collect();
        TemplateLiteralEnds { bytes, spans }
    }
}

/// The first test of Go `inferFromLiteralPartsToTemplateLiteral`: the source
/// starts with the target start text and ends with the target end text, and
/// a single source text is long enough for both. False is Go's early `nil`.
fn literal_part_ends_match(
    source_start_text: &[u8],
    source_end_text: &[u8],
    single_source_text: bool,
    target_start_text: &[u8],
    target_end_text: &[u8],
) -> bool {
    !(single_source_text
        && source_start_text.len() < target_start_text.len() + target_end_text.len())
        && source_start_text.starts_with(target_start_text)
        && source_end_text.ends_with(target_end_text)
}

// PORT: perf. The text matching of Go `inferFromLiteralPartsToTemplateLiteral`
// without the type creation. It records each `addMatch` call in `matches`, so
// the caller can keep the source text borrowed from the checker while it
// matches and make the types after. It returns false where Go returns nil.
// The matches recorded before a failure stay in `matches`, because Go has
// already made their types at that point. Go matches the bytes of the
// strings, so the texts are the Go bytes of the port forms.
fn match_literal_parts_to_template_literal<S: AsRef<[u8]>, T: AsRef<[u8]>>(
    source_texts: &[S],
    target_texts: &[T],
    matches: &mut LiteralPartMatches,
) -> bool {
    let last_source_index = source_texts.len() - 1;
    let source_start_text = source_texts[0].as_ref();
    let source_end_text = source_texts[last_source_index].as_ref();
    let last_target_index = target_texts.len() - 1;
    let target_start_text = target_texts[0].as_ref();
    let target_end_text = target_texts[last_target_index].as_ref();
    if !literal_part_ends_match(
        source_start_text,
        source_end_text,
        last_source_index == 0,
        target_start_text,
        target_end_text,
    ) {
        return false;
    }
    let remaining_end_text = &source_end_text[..source_end_text.len() - target_end_text.len()];
    let mut seg: usize = 0;
    let mut pos: usize = target_start_text.len();
    // Go closure `getSourceText`.
    fn source_text_at<'a, S: AsRef<[u8]>>(
        source_texts: &'a [S],
        remaining_end_text: &'a [u8],
        last_source_index: usize,
        index: usize,
    ) -> &'a [u8] {
        if index < last_source_index {
            return source_texts[index].as_ref();
        }
        remaining_end_text
    }
    let get_source_text =
        |index: usize| source_text_at(source_texts, remaining_end_text, last_source_index, index);
    // Go closure `addMatch`, without the type creation. `seg` and `pos` are
    // passed explicitly so the loop below can read them.
    let mut add_match = |seg: &mut usize, pos: &mut usize, s: usize, p: usize| {
        matches.push(LiteralPartMatch {
            seg: *seg,
            pos: *pos,
            s,
            p,
        });
        *seg = s;
        *pos = p;
    };
    for i in 1..last_target_index {
        let delim = target_texts[i].as_ref();
        if !delim.is_empty() {
            let mut s = seg;
            let mut p = pos;
            loop {
                if let Some(d) = memchr::memmem::find(&get_source_text(s)[p..], delim) {
                    p += d;
                    break;
                }
                s += 1;
                if s == source_texts.len() {
                    return false;
                }
                p = 0;
            }
            add_match(&mut seg, &mut pos, s, p);
            pos += delim.len();
        } else if pos < get_source_text(seg).len() {
            let source_text = get_source_text(seg);
            // Consume one code point at a time, matching the string iterator
            // (`[x, ..._] = s`) rather than UTF-16 code-unit indexing (`s[0]`).
            // DecodeJSStringRune is required rather than utf8.DecodeRuneInString
            // because a lone surrogate is stored as an invalid-UTF-8 sentinel;
            // utf8 would treat that as an error and advance a single byte,
            // breaking the sentinel into stray bytes, whereas DecodeJSStringRune
            // pulls the whole sentinel off as one code point.
            //
            // This intentionally diverges from Strada, which advances one UTF-16
            // code unit at a time (`s[0]` semantics) and therefore splits a
            // supplementary code point such as an emoji into its surrogate
            // halves. If we ever need to match that, expand sourceTexts and
            // targetTexts into code-unit space up front with a SplitSurrogatePairs
            // helper (the inverse of CombineSurrogatePairs) and decode by code
            // unit here; the CombineSurrogatePairs call in addMatch already
            // recombines captured halves back into canonical form.
            let (_, size) = go_decode_js_string_rune_bytes(&source_text[pos..]);
            let (s0, p0) = (seg, pos + size);
            add_match(&mut seg, &mut pos, s0, p0);
        } else if seg < last_source_index {
            let s0 = seg + 1;
            add_match(&mut seg, &mut pos, s0, 0);
        } else {
            return false;
        }
    }
    let end = get_source_text(last_source_index).len();
    add_match(&mut seg, &mut pos, last_source_index, end);
    true
}

#[cfg(test)]
mod template_ends_tests {
    use super::*;
    use crate::checker::utilities_p1::union_sort_tests::with_alias_types;

    /// String literals and template literal types whose start and end texts
    /// share bytes, are empty, are longer than the string, or are not ASCII,
    /// and a string mapping type.
    const SOURCE: &str = r#"
type S1 = "a.b";
type S2 = "x";
type S3 = "abc";
type S4 = "";
type S5 = "é.z";
type S6 = "aXz";
type S7 = "123.b";
type S8 = "ab";
type T1 = `${number}.b`;
type T2 = `a.${string}`;
type T3 = `${string}z`;
type T4 = `a${string}b${string}c`;
type T5 = `${string}`;
type T6 = `é${string}`;
type T7 = `ab${string}ab`;
type T8 = `${string}.${string}`;
type T9 = Uppercase<string>;
"#;

    /// `template_candidates_of_string_literal` gives, in list order, every
    /// string mapping and every template literal that
    /// `string_literal_misses_template_literal_ends` does not reject, and so
    /// every template that matches the string.
    #[test]
    fn template_candidates_are_the_templates_that_pass_the_end_test() {
        with_alias_types(SOURCE, |c, types| {
            let strings: Vec<TypeId> = types
                .iter()
                .copied()
                .filter(|&t| c.ty(t).flags.intersects(TypeFlags::STRING_LITERAL))
                .collect();
            let templates: Vec<TypeId> = types
                .iter()
                .copied()
                .filter(|&t| {
                    c.is_pattern_literal_type(t)
                        || c.ty(t).flags.intersects(TypeFlags::STRING_MAPPING)
                })
                .collect();
            // `${string}` is `string`, not a template.
            assert_eq!((strings.len(), templates.len()), (8, 8));
            let ends = TemplateLiteralEnds::new(c, &templates);
            let (mut kept, mut matched) = (0, 0);
            for &s in &strings {
                let mut candidates = SmallVec::new();
                c.template_candidates_of_string_literal(s, &templates, &ends, &mut candidates);
                let want: Vec<TypeId> = templates
                    .iter()
                    .copied()
                    .filter(|&t| {
                        !c.ty(t).flags.intersects(TypeFlags::TEMPLATE_LITERAL)
                            || !c.string_literal_misses_template_literal_ends(s, t)
                    })
                    .collect();
                assert_eq!(
                    &candidates[..],
                    &want[..],
                    "{:?}",
                    c.get_string_literal_value_ref(s)
                );
                kept += candidates.len();
                for &t in &templates {
                    if c.is_type_matched_by_template_literal_or_string_mapping(s, t) {
                        matched += 1;
                        assert!(
                            candidates.contains(&t),
                            "{:?}",
                            c.get_string_literal_value_ref(s)
                        );
                    }
                }
            }
            assert!(
                kept > matched && matched > 5 && kept < strings.len() * templates.len(),
                "{kept} {matched}"
            );
        });
    }
}

#[cfg(test)]
mod related_info_tests {
    use super::*;

    /// A saved `RelatedInfo` (an error state) keeps the list it saw when the
    /// relater pushes after the save, and after a restore and a new push, as
    /// Go's `errorState` keeps its slice header (relater.go:3212, :3219).
    #[test]
    fn a_saved_state_keeps_its_related_info() {
        let message = diag::X_0_is_declared_here;
        let d = |name: &str| {
            new_diagnostic(
                Node::NIL,
                TextRange::new(0, 0),
                message,
                vec![name.to_string()],
            )
        };
        let names = |info: &RelatedInfo| -> Vec<String> {
            info.as_slice()
                .iter()
                .map(|d| d.message_args[0].clone())
                .collect()
        };
        let mut info = RelatedInfo::default();
        let empty = info.clone();
        info.push(d("a"));
        let saved = info.clone();
        info.push(d("b"));
        assert_eq!(
            (names(&empty), names(&saved), names(&info)),
            (vec![], vec!["a".to_string()], vec!["a".into(), "b".into()])
        );
        // Restore, then push again: the saved state and the dropped list do
        // not change.
        let dropped = info;
        info = saved.clone();
        info.push(d("c"));
        assert_eq!(
            (names(&saved), names(&dropped), names(&info)),
            (
                vec!["a".to_string()],
                vec!["a".into(), "b".into()],
                vec!["a".into(), "c".into()]
            )
        );
        info.clear();
        assert!(info.as_slice().is_empty());
    }
}
