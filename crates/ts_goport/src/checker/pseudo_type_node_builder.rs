//! Go `checker/pseudotypenodebuilder.go`: maps pseudochecker pseudo types
//! into type nodes, and compares pseudo types with checker types.
//!
//! Methods use the Relater pattern: `impl Checker` methods take `&mut self`
//! and `b: &Rc<RefCell<NodeBuilderImpl>>`. No `RefCell` borrow of `b` is held
//! across a Checker call.

use crate::prelude::*;
use crate::printer::EmitFlags;
use crate::pseudochecker::{
    self, PseudoObjectElementKind, PseudoParameter, PseudoType, PseudoTypeKind,
};

use super::nodebuilder_impl_p3::{nb_ctx, nb_ctx_mut, nb_e, tracker_report_inference_fallback};

// Go: checker/pseudotypenodebuilder.go:632 isStructuralPseudoType
pub fn is_structural_pseudo_type(t: &PseudoType) -> bool {
    match t.kind {
        PseudoTypeKind::OBJECT_LITERAL
        | PseudoTypeKind::TUPLE
        | PseudoTypeKind::SINGLE_CALL_SIGNATURE => true,
        PseudoTypeKind::MAYBE_CONST_LOCATION => {
            let d = t.as_pseudo_type_maybe_const_location();
            is_structural_pseudo_type(&d.const_type) || is_structural_pseudo_type(&d.regular_type)
        }
        _ => false,
    }
}

impl Checker {
    /// Runs `type_to_type_node(checker_type)` with inference fallback reports suppressed.
    // PORT: Go repeats this block inline twice in pseudoTypeToNodeWithCheckerFallback.
    fn pseudo_type_checker_fallback_type_node(
        &mut self,
        b: &Rc<RefCell<NodeBuilderImpl>>,
        checker_type: TypeId,
    ) -> Node {
        let old_suppress = nb_ctx(b, |c| c.suppress_report_inference_fallback);
        nb_ctx_mut(b, |c| c.suppress_report_inference_fallback = true);
        let result = self.type_to_type_node(b, checker_type);
        nb_ctx_mut(b, |c| c.suppress_report_inference_fallback = old_suppress);
        result
    }

    /// pseudoTypeToNodeWithCheckerFallback is like pseudoTypeToNode but when the top-level pseudo type
    /// is PseudoTypeInferred, it reports any error nodes and then serializes from the checker's type.
    /// This avoids incorrect type output when PseudoTypeInferred would derive the type from the
    /// original declaration expression in an instantiated context.
    // Go: checker/pseudotypenodebuilder.go:15 pseudoTypeToNodeWithCheckerFallback
    pub fn pseudo_type_to_node_with_checker_fallback(
        &mut self,
        b: &Rc<RefCell<NodeBuilderImpl>>,
        t: &PseudoType,
        checker_type: TypeId,
    ) -> Node {
        if t.kind == PseudoTypeKind::INFERRED {
            if !nb_ctx(b, |c| c.suppress_report_inference_fallback) {
                let inferred = t.as_pseudo_type_inferred();
                if !inferred.error_nodes.is_empty() {
                    for &n in &inferred.error_nodes {
                        tracker_report_inference_fallback(self, b, n);
                    }
                } else {
                    tracker_report_inference_fallback(self, b, inferred.expression);
                }
            }
            return self.pseudo_type_checker_fallback_type_node(b, checker_type);
        } else if t.kind == PseudoTypeKind::DIRECT {
            let existing = t.as_pseudo_type_direct().type_node;
            if !self.can_reuse_existing_js_type_node(b, existing, checker_type) {
                if !nb_ctx(b, |c| c.suppress_report_inference_fallback) {
                    tracker_report_inference_fallback(self, b, existing);
                }
                return self.pseudo_type_checker_fallback_type_node(b, checker_type);
            }
        }
        self.pseudo_type_to_node(b, t)
    }

    /// Maps a pseudochecker's pseudotypes into ast nodes and reports any inference fallback errors the pseudotype structure implies
    // Go: checker/pseudotypenodebuilder.go:48 pseudoTypeToNode
    pub fn pseudo_type_to_node(
        &mut self,
        b: &Rc<RefCell<NodeBuilderImpl>>,
        t: &PseudoType,
    ) -> Node {
        // PORT: Go asserts `t != nil`; a `&PseudoType` is never nil.
        let e = nb_e(b);
        let f = e.factory();
        match t.kind {
            PseudoTypeKind::DIRECT => self.reuse_type_node(b, t.as_pseudo_type_direct().type_node),
            PseudoTypeKind::INFERRED => {
                let inferred = t.as_pseudo_type_inferred();
                let node = inferred.expression;
                if !inferred.error_nodes.is_empty() {
                    for &n in &inferred.error_nodes {
                        tracker_report_inference_fallback(self, b, n);
                    }
                } else if is_entity_name_expression(node) && is_declaration(node.parent()) {
                    tracker_report_inference_fallback(self, b, node.parent());
                } else {
                    tracker_report_inference_fallback(self, b, node);
                }
                if inferred.is_signature_return {
                    let signature = self.get_signature_from_declaration(node);
                    return self.serialize_return_type_for_signature(b, signature, false);
                }
                // use symbol type from parent declaration to automatically handle expression type widening without duplicating logic
                if is_return_statement(node.parent()) {
                    let enclosing = get_containing_function(node);
                    if is_accessor(enclosing) {
                        return self.serialize_type_for_declaration(
                            b,
                            enclosing,
                            TypeId::NIL,
                            SymbolId::NIL,
                            false,
                        );
                    }
                    let signature = self.get_signature_from_declaration(enclosing);
                    return self.serialize_return_type_for_signature(b, signature, false);
                }
                if is_arrow_function(node.parent()) && node.parent().body() == node {
                    let signature = self.get_signature_from_declaration(node.parent());
                    return self.serialize_return_type_for_signature(b, signature, false);
                }
                if is_declaration(node.parent()) {
                    return self.serialize_type_for_declaration(
                        b,
                        node.parent(),
                        TypeId::NIL,
                        SymbolId::NIL,
                        false,
                    );
                }
                // This might be effectively unreachable. If it's not, it may need more widening rules to mirror checker behavior for whatever expressions are serialized here
                let ty = self.get_type_of_expression(node);
                self.type_to_type_node(b, ty)
            }
            PseudoTypeKind::NO_RESULT => {
                let node = t.as_pseudo_type_no_result().declaration;
                tracker_report_inference_fallback(self, b, node);
                if is_function_like(node) && !is_accessor(node) {
                    let signature = self.get_signature_from_declaration(node);
                    return self.serialize_return_type_for_signature(b, signature, false);
                }
                self.serialize_type_for_declaration(b, node, TypeId::NIL, SymbolId::NIL, false)
            }
            PseudoTypeKind::MAYBE_CONST_LOCATION => {
                let d = t.as_pseudo_type_maybe_const_location();
                // see checkExpressionWithContextualType for general literal widening rules which need to be emulated here, plus
                // checkTemplateLiteralExpression for template literal widening rules if the pseudochecker ever supports literalized templates
                let mut is_in_const_context = self.is_const_context(d.node);
                if !is_in_const_context && pseudochecker::is_in_const_context(d.node) {
                    // Only consult the contextual type if the pseudochecker's syntactic check also puts us in a const context.
                    // getContextualType returns post-inference results at node-printing time which may not have existed
                    // during initial checking (e.g. when the contextual type depends on inference), causing incorrect
                    // literal type preservation.
                    let contextual_type = self.get_contextual_type(d.node, ContextFlags::NONE);
                    let t = self.pseudo_type_to_type(b, &d.const_type);
                    if t.is_some() {
                        let instantiated = self.instantiate_contextual_type(
                            contextual_type,
                            d.node,
                            ContextFlags::NONE,
                        );
                        if self.is_literal_of_contextual_type(t, instantiated) {
                            is_in_const_context = true;
                        }
                    }
                }
                if is_in_const_context {
                    self.pseudo_type_to_node(b, &d.const_type)
                } else {
                    self.pseudo_type_to_node(b, &d.regular_type)
                }
            }
            PseudoTypeKind::UNION => {
                let mut res: Vec<Node> = Vec::new();
                let mut has_elided_type = false;
                let mut has_undefined = false;
                // PORT: Go uses a recursive closure `appendTypeNode`; here it is
                // a nested fn with the captured state passed in.
                fn append_type_node(node: Node, res: &mut Vec<Node>, has_undefined: &mut bool) {
                    if is_union_type_node(node) {
                        for node in node.types().nodes().iter() {
                            append_type_node(node, res, has_undefined);
                        }
                        return;
                    }
                    if node.kind() == SyntaxKind::UndefinedKeyword {
                        if *has_undefined {
                            return;
                        }
                        *has_undefined = true;
                    }
                    res.push(node);
                }
                let members = &t.as_pseudo_type_union().types;
                for m in members {
                    if !self.strict_null_checks
                        && (m.kind == PseudoTypeKind::UNDEFINED || m.kind == PseudoTypeKind::NULL)
                    {
                        has_elided_type = true;
                        continue;
                    }
                    let node = self.pseudo_type_to_node(b, m);
                    append_type_node(node, &mut res, &mut has_undefined);
                }
                if res.len() == 1 {
                    return res[0];
                }
                if res.is_empty() {
                    if has_elided_type {
                        return f.new_keyword_type_node(SyntaxKind::AnyKeyword);
                    }
                    return f.new_keyword_type_node(SyntaxKind::NeverKeyword);
                }
                f.new_union_type_node(f.new_node_list(&res))
            }
            PseudoTypeKind::UNDEFINED => {
                if !self.strict_null_checks {
                    return f.new_keyword_type_node(SyntaxKind::AnyKeyword);
                }
                f.new_keyword_type_node(SyntaxKind::UndefinedKeyword)
            }
            PseudoTypeKind::NULL => {
                if !self.strict_null_checks {
                    return f.new_keyword_type_node(SyntaxKind::AnyKeyword);
                }
                f.new_literal_type_node(f.new_keyword_expression(SyntaxKind::NullKeyword))
            }
            PseudoTypeKind::ANY => f.new_keyword_type_node(SyntaxKind::AnyKeyword),
            PseudoTypeKind::STRING => f.new_keyword_type_node(SyntaxKind::StringKeyword),
            PseudoTypeKind::NUMBER => f.new_keyword_type_node(SyntaxKind::NumberKeyword),
            PseudoTypeKind::BIG_INT => f.new_keyword_type_node(SyntaxKind::BigIntKeyword),
            PseudoTypeKind::BOOLEAN => f.new_keyword_type_node(SyntaxKind::BooleanKeyword),
            PseudoTypeKind::FALSE => {
                f.new_literal_type_node(f.new_keyword_expression(SyntaxKind::FalseKeyword))
            }
            PseudoTypeKind::TRUE => {
                f.new_literal_type_node(f.new_keyword_expression(SyntaxKind::TrueKeyword))
            }
            PseudoTypeKind::SINGLE_CALL_SIGNATURE => {
                let d = t.as_pseudo_type_single_call_signature();
                let signature = self.get_signature_from_declaration(d.signature);
                let expanded_params = self
                    .get_expanded_parameters(signature, true /*skipUnionExpanding*/)
                    .swap_remove(0);
                let (type_parameters, parameters, mapper) = {
                    let sig = self.sig(signature);
                    (
                        sig.type_parameters.clone(),
                        sig.parameters.clone(),
                        sig.mapper,
                    )
                };
                let cleanup = self.enter_new_scope(
                    b,
                    d.signature,
                    &expanded_params,
                    &type_parameters,
                    &parameters,
                    mapper,
                );
                let mut type_params = NodeList::NIL;
                if !d.type_parameters.is_empty() {
                    let mut res = Vec::with_capacity(d.type_parameters.len());
                    for &tp in &d.type_parameters {
                        res.push(self.reuse_node(b, tp));
                    }
                    type_params = f.new_node_list(&res);
                }
                let params = self.pseudo_parameters_to_node_list(b, &d.parameters);
                let return_type = self.pseudo_type_to_node(b, &d.return_type);
                let result = f.new_function_type_node(type_params, params, return_type);
                // PORT: Go `defer cleanup()`; it runs after the result node is built.
                cleanup(self);
                result
            }
            PseudoTypeKind::TUPLE => {
                let mut res: Vec<Node> = Vec::new();
                let elements = &t.as_pseudo_type_tuple().elements;
                for elem in elements {
                    res.push(self.pseudo_type_to_node(b, elem));
                }
                // pseudo-tuples are implicitly `readonly` since they originate from `as const` contexts
                // but strada *sometimes* fails to add the `readonly` modifier to the generated node.
                let result = f.new_tuple_type_node(f.new_node_list(&res));
                e.add_emit_flags(result, EmitFlags::SINGLE_LINE);
                f.new_type_operator_node(SyntaxKind::ReadonlyKeyword, result)
            }
            PseudoTypeKind::OBJECT_LITERAL => {
                let elements = &t.as_pseudo_type_object_literal().elements;
                if elements.is_empty() {
                    let result = f.new_type_literal_node(f.new_node_list(&[]));
                    e.add_emit_flags(result, EmitFlags::SINGLE_LINE);
                    return result;
                }
                // NOTE: using the checker's `isConstContext` instead of the pseudochecker's `isInConstContext`
                // results in different results here. The checker one is more "correct" but means we'll mark
                // objects in parameter positions contextually typed by const type parameters as readonly -
                // something a true syntactic ID emitter couldn't possibly know (since the signature could
                // be from across files). This can't *really* happen in any cases ID doesn't already error on, though.
                // Just something to keep in mind if the ID checker keeps growing.
                let is_const = self.is_const_context(elements[0].name.parent().parent());
                let mut new_elements: Vec<Node> = Vec::with_capacity(elements.len());

                // Member types are serialized within an object type literal, so set the
                // corresponding flag to mirror createTypeNodeFromObjectType. This ensures
                // inaccessible `this` references inside the members are reported (TS2527).
                let restore_object_literal_flags = self.save_restore_flags(b);
                nb_ctx_mut(b, |c| {
                    c.flags = c.flags | NodeBuilderFlags::IN_OBJECT_TYPE_LITERAL
                });

                for elem in elements {
                    let mut modifiers = ModifierList::NIL;
                    if is_const
                        || (elem.kind == PseudoObjectElementKind::PROPERTY_ASSIGNMENT
                            && elem.as_pseudo_property_assignment().readonly)
                    {
                        modifiers =
                            f.new_modifier_list(&[f.new_modifier(SyntaxKind::ReadonlyKeyword)]);
                    }
                    let mut cleanup = None;
                    if elem.kind != PseudoObjectElementKind::PROPERTY_ASSIGNMENT {
                        let signature = self.get_signature_from_declaration(elem.signature());
                        let expanded_params = self
                            .get_expanded_parameters(signature, true /*skipUnionExpanding*/)
                            .swap_remove(0);
                        let (type_parameters, parameters, mapper) = {
                            let sig = self.sig(signature);
                            (
                                sig.type_parameters.clone(),
                                sig.parameters.clone(),
                                sig.mapper,
                            )
                        };
                        cleanup = Some(self.enter_new_scope(
                            b,
                            elem.signature(),
                            &expanded_params,
                            &type_parameters,
                            &parameters,
                            mapper,
                        ));
                    }
                    let mut new_prop = Node::NIL;
                    match elem.kind {
                        PseudoObjectElementKind::METHOD => {
                            let d = elem.as_pseudo_object_method();
                            let mut type_params = NodeList::NIL;
                            if !d.type_parameters.is_empty() {
                                let mut res = Vec::with_capacity(d.type_parameters.len());
                                for &tp in &d.type_parameters {
                                    res.push(self.reuse_node(b, tp));
                                }
                                type_params = f.new_node_list(&res);
                            }
                            if is_const {
                                let name = self.reuse_name(b, elem.name, false /*isMethod*/);
                                let params = self.pseudo_parameters_to_node_list(b, &d.parameters);
                                let return_type = self.pseudo_type_to_node(b, &d.return_type);
                                new_prop = f.new_property_signature_declaration(
                                    modifiers,
                                    name,
                                    Node::NIL,
                                    f.new_function_type_node(type_params, params, return_type),
                                    Node::NIL,
                                );
                            } else {
                                let name = self.reuse_name(b, elem.name, true /*isMethod*/);
                                let params = self.pseudo_parameters_to_node_list(b, &d.parameters);
                                let return_type = self.pseudo_type_to_node(b, &d.return_type);
                                new_prop = f.new_method_signature_declaration(
                                    modifiers,
                                    name,
                                    Node::NIL,
                                    type_params,
                                    params,
                                    return_type,
                                );
                            }
                        }
                        PseudoObjectElementKind::PROPERTY_ASSIGNMENT => {
                            let d = elem.as_pseudo_property_assignment();
                            let name = self.reuse_name(b, elem.name, false /*isMethod*/);
                            let type_node = self.pseudo_type_to_node(b, &d.type_);
                            new_prop = f.new_property_signature_declaration(
                                modifiers,
                                name,
                                Node::NIL,
                                type_node,
                                Node::NIL,
                            );
                        }
                        PseudoObjectElementKind::SET_ACCESSOR => {
                            let d = elem.as_pseudo_set_accessor();
                            let name = self.reuse_name(b, elem.name, false /*isMethod*/);
                            let parameter = self.pseudo_parameter_to_node(b, &d.parameter);
                            new_prop = f.new_set_accessor_declaration(
                                ModifierList::NIL,
                                name,
                                NodeList::NIL,
                                f.new_node_list(&[parameter]),
                                Node::NIL,
                                Node::NIL,
                                Node::NIL,
                            );
                        }
                        PseudoObjectElementKind::GET_ACCESSOR => {
                            let d = elem.as_pseudo_get_accessor();
                            let name = self.reuse_name(b, elem.name, false /*isMethod*/);
                            let type_node = self.pseudo_type_to_node(b, &d.type_);
                            new_prop = f.new_get_accessor_declaration(
                                ModifierList::NIL,
                                name,
                                NodeList::NIL,
                                NodeList::NIL,
                                type_node,
                                Node::NIL,
                                Node::NIL,
                            );
                        }
                        _ => {}
                    }
                    if nb_ctx(b, |c| c.enclosing_file) == get_source_file_of_node(elem.name) {
                        e.set_comment_range(new_prop, elem.name.parent().loc());
                    }
                    new_elements.push(new_prop);
                    if let Some(cleanup) = cleanup {
                        cleanup(self);
                    }
                }
                restore_object_literal_flags();
                let result = f.new_type_literal_node(f.new_node_list(&new_elements));
                if !nb_ctx(b, |c| {
                    c.flags
                        .intersects(NodeBuilderFlags::MULTILINE_OBJECT_LITERALS)
                }) {
                    e.add_emit_flags(result, EmitFlags::SINGLE_LINE);
                }
                result
            }
            PseudoTypeKind::STRING_LITERAL
            | PseudoTypeKind::NUMERIC_LITERAL
            | PseudoTypeKind::BIG_INT_LITERAL => {
                let source = t.as_pseudo_type_literal().node;
                let reused = self.reuse_node(b, source);
                f.new_literal_type_node(reused)
            }
            _ => crate::gostd::debug::assert_never(
                &t.kind.0.to_string(),
                Some("Unhandled pseudotype kind in pseudotype node construction"),
            ),
        }
    }

    // Go: checker/pseudotypenodebuilder.go:327 pseudoParametersToNodeList
    pub fn pseudo_parameters_to_node_list(
        &mut self,
        b: &Rc<RefCell<NodeBuilderImpl>>,
        params: &[Rc<PseudoParameter>],
    ) -> NodeList {
        let mut res = Vec::with_capacity(params.len());
        for p in params {
            res.push(self.pseudo_parameter_to_node(b, p));
        }
        nb_e(b).factory().new_node_list(&res)
    }

    // Go: checker/pseudotypenodebuilder.go:335 pseudoParameterToNode
    pub fn pseudo_parameter_to_node(
        &mut self,
        b: &Rc<RefCell<NodeBuilderImpl>>,
        p: &PseudoParameter,
    ) -> Node {
        let e = nb_e(b);
        let f = e.factory();
        let mut dot_dot_dot = Node::NIL;
        let mut question_mark = Node::NIL;
        if p.rest {
            dot_dot_dot = f.new_token(SyntaxKind::DotDotDotToken);
        }
        if p.optional {
            question_mark = f.new_token(SyntaxKind::QuestionToken);
        }
        // matches strada behavior of always reserializing param names from scratch
        let name = self.parameter_to_parameter_declaration_name(
            b,
            p.name.parent().symbol(),
            p.name.parent(),
        );
        let type_node = self.pseudo_type_to_node(b, &p.type_);
        let parameter = f.new_parameter_declaration(
            ModifierList::NIL,
            dot_dot_dot,
            name,
            question_mark,
            type_node,
            Node::NIL,
        );
        let original = p.name.parent();
        if is_parameter_declaration(original) {
            self.set_comment_range(b, parameter, original);
        }
        parameter
    }

    /// see `typeNodeIsEquivalentToType` in strada, but applied more broadly here, so is setup to handle more equivalences - strada only used it via
    /// the `canReuseTypeNodeAnnotation` host hook and not the `canReuseTypeNode` hook, which meant locations using the later were reliant on
    /// over-invalidation by the ID inference engine to not emit incorrect types.
    // Go: checker/pseudotypenodebuilder.go:362 pseudoTypeEquivalentToType
    pub fn pseudo_type_equivalent_to_type(
        &mut self,
        b: &Rc<RefCell<NodeBuilderImpl>>,
        t: &PseudoType,
        type_: TypeId,
        is_optional_annotated: bool,
        report_errors: bool,
    ) -> bool {
        // if type_ resolves to an error, we charitably assume equality, since we might be in a single-file checking mode
        if type_.is_some() && self.is_error_type(type_) {
            return true;
        }
        // If we can easily operate on just types, we should
        let type_from_pseudo = self.pseudo_type_to_type(b, t); // note: cannot convert complex types like objects, which must be validated separately
        if type_from_pseudo == type_ {
            return true;
        }
        let mut undefined_stripped = type_;
        if is_optional_annotated {
            undefined_stripped = self.get_type_with_facts(type_, TypeFacts::NE_UNDEFINED);
        }
        if type_from_pseudo.is_some() && type_.is_some() {
            if is_optional_annotated {
                if undefined_stripped == type_from_pseudo {
                    return true;
                }
                if self.ty(type_from_pseudo).flags.intersects(TypeFlags::UNION)
                    && self
                        .ty(undefined_stripped)
                        .flags
                        .intersects(TypeFlags::UNION)
                {
                    // does union comparison in general, since the unions may not be `==` identical due to aliasing and the like
                    if self.compare_types_identical(type_from_pseudo, undefined_stripped)
                        == Ternary::TRUE
                    {
                        return true;
                    }
                }
            }
            // handles freshness mismatches (e.g., fresh true vs regular true in as const)
            if self.get_regular_type_of_literal_type(type_from_pseudo)
                == self.get_regular_type_of_literal_type(type_)
            {
                return true;
            }
            if self.ty(type_from_pseudo).flags.intersects(TypeFlags::UNION)
                && self.ty(type_).flags.intersects(TypeFlags::UNION)
            {
                // handles union comparison in general, since unions may not be `==` identical due to aliasing
                if self.compare_types_identical(type_from_pseudo, type_) == Ternary::TRUE {
                    return true;
                }
            }
        }
        // otherwise, fallback to actual pseudo/type cross-comparisons
        match t.kind {
            PseudoTypeKind::INFERRED => {
                // PseudoTypeInferred with error nodes identifies specific problematic children.
                // Report fine-grained errors on them, then return false so the parent falls back
                // to checker-based serialization (avoiding issues like reusing raw JSON string
                // literal property names from the pseudochecker's AST).
                let inferred = t.as_pseudo_type_inferred();
                if !inferred.error_nodes.is_empty() {
                    if report_errors {
                        for &n in &inferred.error_nodes {
                            tracker_report_inference_fallback(self, b, n);
                        }
                    }
                    return false;
                }
                if report_errors {
                    tracker_report_inference_fallback(self, b, inferred.expression);
                }
                false
            }
            PseudoTypeKind::OBJECT_LITERAL => {
                let pt = t.as_pseudo_type_object_literal();
                if type_.is_nil() {
                    return false;
                }
                let target_props = self.get_properties_of_type(undefined_stripped);
                // Count total declarations across all target prop symbols to handle getter/setter pairs,
                // which are two elements in pt.Elements but only one symbol in targetProps.
                let mut target_decl_count = 0usize;
                for &prop in &target_props {
                    target_decl_count += self.sym(prop).declarations.len();
                }
                if pt.elements.len() != target_decl_count {
                    return false;
                }
                for elem in &pt.elements {
                    let mut target_prop = SymbolId::NIL;
                    let elem_symbol = elem.name.parent().symbol();
                    if elem_symbol.is_some() {
                        let name = self.sym(elem_symbol).name.clone();
                        target_prop = self.get_property_of_type(undefined_stripped, &name);
                    }
                    if target_prop.is_nil() {
                        // Name lookup failed or returned no result; search target properties
                        // for one whose declaration name node matches the one we have
                        for &prop in &target_props {
                            let value_declaration = self.sym(prop).value_declaration;
                            if value_declaration.is_some() && value_declaration.name() == elem.name
                            {
                                target_prop = prop;
                                break;
                            }
                        }
                        if target_prop.is_nil() {
                            if report_errors {
                                tracker_report_inference_fallback(self, b, elem.name.parent());
                            }
                            return false;
                        }
                    }
                    let target_is_optional = self
                        .sym_flags(target_prop)
                        .intersects(SymbolFlags::OPTIONAL);
                    if elem.optional != target_is_optional {
                        if report_errors {
                            tracker_report_inference_fallback(self, b, elem.name.parent());
                        }
                        return false;
                    }
                    let prop_type = self.get_type_of_symbol(target_prop);
                    let prop_type = self.remove_missing_type(prop_type, target_is_optional);
                    match elem.kind {
                        PseudoObjectElementKind::PROPERTY_ASSIGNMENT => {
                            let d = elem.as_pseudo_property_assignment();
                            if !self.pseudo_type_equivalent_to_type(
                                b,
                                &d.type_,
                                prop_type,
                                elem.optional,
                                false,
                            ) {
                                if report_errors {
                                    if d.type_.kind == PseudoTypeKind::INFERRED
                                        && !d.type_.as_pseudo_type_inferred().error_nodes.is_empty()
                                    {
                                        // Re-report the fine-grained error nodes; the recursive call used reportErrors=false
                                        for &n in &d.type_.as_pseudo_type_inferred().error_nodes {
                                            tracker_report_inference_fallback(self, b, n);
                                        }
                                    } else if !is_structural_pseudo_type(&d.type_) {
                                        tracker_report_inference_fallback(
                                            self,
                                            b,
                                            elem.name.parent(),
                                        );
                                    }
                                }
                                return false;
                            }
                        }
                        PseudoObjectElementKind::METHOD => {
                            let d = elem.as_pseudo_object_method();
                            let target_sig = self.get_single_call_signature(prop_type);
                            if target_sig.is_nil() {
                                // Target property type doesn't have a single call signature; can't validate
                                continue;
                            }
                            let param_eq = self.pseudo_parameters_equivalent_to_parameters(
                                b,
                                &d.parameters,
                                target_sig,
                                report_errors,
                                elem.name.parent(),
                            );
                            if !param_eq {
                                return false;
                            }
                            let target_predicate = self.get_type_predicate_of_signature(target_sig);
                            if target_predicate.is_some() {
                                if !self.pseudo_return_type_matches_predicate(
                                    b,
                                    &d.return_type,
                                    target_predicate,
                                ) {
                                    if report_errors {
                                        tracker_report_inference_fallback(
                                            self,
                                            b,
                                            elem.name.parent(),
                                        );
                                    }
                                    return false;
                                }
                            } else {
                                let return_type = self.get_return_type_of_signature(target_sig);
                                if !self.pseudo_type_equivalent_to_type(
                                    b,
                                    &d.return_type,
                                    return_type,
                                    false,
                                    false,
                                ) {
                                    if report_errors {
                                        tracker_report_inference_fallback(
                                            self,
                                            b,
                                            elem.name.parent(),
                                        );
                                    }
                                    return false;
                                }
                            }
                        }
                        PseudoObjectElementKind::GET_ACCESSOR => {
                            let d = elem.as_pseudo_get_accessor();
                            if !self.pseudo_type_equivalent_to_type(
                                b, &d.type_, prop_type, false, false,
                            ) {
                                if report_errors {
                                    tracker_report_inference_fallback(self, b, elem.name.parent());
                                }
                                return false;
                            }
                        }
                        PseudoObjectElementKind::SET_ACCESSOR => {
                            let d = elem.as_pseudo_set_accessor();
                            let write_type = self.get_write_type_of_symbol(target_prop);
                            if !self.pseudo_type_equivalent_to_type(
                                b,
                                &d.parameter.type_,
                                write_type,
                                false,
                                false,
                            ) {
                                if report_errors {
                                    tracker_report_inference_fallback(self, b, elem.name.parent());
                                }
                                return false;
                            }
                        }
                        _ => {}
                    }
                }
                true
            }
            PseudoTypeKind::TUPLE => {
                let pt = t.as_pseudo_type_tuple();
                if undefined_stripped.is_nil() || !self.is_tuple_type(undefined_stripped) {
                    return false;
                }
                // Pseudo-tuples come from `as const` array literals, so they only ever have required elements.
                // If the target tuple has optional, rest, or variadic elements, the structures can't match.
                if self
                    .target_tuple_type(undefined_stripped)
                    .combined_flags
                    .intersects(ElementFlags::NON_REQUIRED)
                {
                    return false;
                }
                let element_types = self.get_type_arguments(undefined_stripped);
                if pt.elements.len() != element_types.len() {
                    return false;
                }
                for (i, elem) in pt.elements.iter().enumerate() {
                    if !self.pseudo_type_equivalent_to_type(
                        b,
                        elem,
                        element_types[i],
                        false,
                        report_errors,
                    ) {
                        return false;
                    }
                }
                true
            }
            PseudoTypeKind::SINGLE_CALL_SIGNATURE => {
                let target_sig = self.get_single_call_signature(undefined_stripped);
                if target_sig.is_nil() {
                    return false;
                }
                let pt = t.as_pseudo_type_single_call_signature();
                if self.sig(target_sig).type_parameters.len() != pt.type_parameters.len() {
                    if report_errors {
                        tracker_report_inference_fallback(self, b, pt.signature);
                    }
                    return false;
                }
                let param_eq = self.pseudo_parameters_equivalent_to_parameters(
                    b,
                    &pt.parameters,
                    target_sig,
                    report_errors,
                    pt.signature,
                );
                if !param_eq {
                    return false;
                }
                let target_predicate = self.get_type_predicate_of_signature(target_sig);
                if target_predicate.is_some() {
                    if !self.pseudo_return_type_matches_predicate(
                        b,
                        &pt.return_type,
                        target_predicate,
                    ) {
                        if report_errors {
                            tracker_report_inference_fallback(self, b, pt.signature);
                        }
                        return false;
                    }
                } else {
                    let return_type = self.get_return_type_of_signature(target_sig);
                    if !self.pseudo_type_equivalent_to_type(
                        b,
                        &pt.return_type,
                        return_type,
                        false,
                        report_errors,
                    ) {
                        // error reported within the return type
                        return false;
                    }
                }
                true
            }
            PseudoTypeKind::NO_RESULT => {
                if report_errors {
                    tracker_report_inference_fallback(
                        self,
                        b,
                        t.as_pseudo_type_no_result().declaration,
                    );
                }
                false
            }
            _ => false,
        }
    }

    // Go: checker/pseudotypenodebuilder.go:585 pseudoParametersEquivalentToParameters
    pub fn pseudo_parameters_equivalent_to_parameters(
        &mut self,
        b: &Rc<RefCell<NodeBuilderImpl>>,
        mut params: &[Rc<PseudoParameter>],
        target_sig: SignatureId,
        report_errors: bool,
        non_param_error_location: Node,
    ) -> bool {
        let this_parameter = self.sig(target_sig).this_parameter;
        if this_parameter.is_some() && params.is_empty() {
            if report_errors {
                tracker_report_inference_fallback(self, b, non_param_error_location); // missing `this` param
            }
            return false;
        } else if this_parameter.is_some() && is_this_identifier(params[0].name) {
            let target_param = this_parameter;
            let param_type = self.get_type_of_parameter(target_param);
            if !self.pseudo_type_equivalent_to_type(
                b,
                &params[0].type_,
                param_type,
                params[0].optional,
                false,
            ) {
                if report_errors {
                    tracker_report_inference_fallback(self, b, params[0].name.parent());
                }
                return false;
            }
            params = &params[1..];
        } else if this_parameter.is_some() {
            if report_errors {
                tracker_report_inference_fallback(self, b, non_param_error_location);
            }
            return false;
        }
        let target_parameters = self.sig(target_sig).parameters.clone();
        if target_parameters.len() != params.len() {
            if report_errors {
                tracker_report_inference_fallback(self, b, non_param_error_location);
            }
            return false; // TODO: spread tuple params may mess with this check
        }
        for (i, p) in params.iter().enumerate() {
            let target_param = target_parameters[i];
            let value_declaration = self.sym(target_param).value_declaration;
            if p.optional != self.is_optional_parameter(value_declaration) {
                if report_errors {
                    tracker_report_inference_fallback(self, b, p.name.parent());
                }
                return false;
            }
            let param_type = self.get_type_of_parameter(target_param);
            if !self.pseudo_type_equivalent_to_type(b, &p.type_, param_type, p.optional, false) {
                if report_errors {
                    tracker_report_inference_fallback(self, b, p.name.parent());
                }
                return false;
            }
        }
        true
    }

    /// pseudoReturnTypeMatchesPredicate checks if a pseudo return type (which should be a Direct type
    /// wrapping a TypePredicate) matches the given type predicate from the checker.
    // Go: checker/pseudotypenodebuilder.go:645 pseudoReturnTypeMatchesPredicate
    pub fn pseudo_return_type_matches_predicate(
        &mut self,
        _b: &Rc<RefCell<NodeBuilderImpl>>,
        rt: &PseudoType,
        predicate: TypePredicateId,
    ) -> bool {
        if rt.kind != PseudoTypeKind::DIRECT {
            return false;
        }
        let node = rt.as_pseudo_type_direct().type_node;
        if !is_type_predicate_node(node) {
            return false;
        }
        let (predicate_kind, predicate_parameter_name, predicate_type) = {
            let p = self.pred(predicate);
            (p.kind, p.parameter_name.clone(), p.t)
        };
        // Check asserts modifier matches
        let is_asserts = node.asserts_modifier().is_some();
        let predicate_is_asserts = predicate_kind == TypePredicateKind::ASSERTS_THIS
            || predicate_kind == TypePredicateKind::ASSERTS_IDENTIFIER;
        if is_asserts != predicate_is_asserts {
            return false;
        }
        // Check this vs identifier matches
        let is_this = is_this_type_node(node.parameter_name());
        let predicate_is_this = predicate_kind == TypePredicateKind::THIS
            || predicate_kind == TypePredicateKind::ASSERTS_THIS;
        if is_this != predicate_is_this {
            return false;
        }
        // For identifier predicates, check parameter name matches
        if !is_this && node.parameter_name().text() != predicate_parameter_name {
            return false;
        }
        // Check the narrowed type, if any
        let type_node = node.type_();
        if predicate_type.is_some() {
            if type_node.is_nil() {
                return false;
            }
            let predicate_type_from_node = self.get_type_from_type_node(type_node);
            if predicate_type_from_node != predicate_type
                && self.compare_types_identical(predicate_type_from_node, predicate_type)
                    != Ternary::TRUE
            {
                return false;
            }
        } else if type_node.is_some() {
            return false;
        }
        true
    }

    // Go: checker/pseudotypenodebuilder.go:689 pseudoTypeToType
    pub fn pseudo_type_to_type(
        &mut self,
        b: &Rc<RefCell<NodeBuilderImpl>>,
        t: &PseudoType,
    ) -> TypeId {
        // !!! TODO: only literal types currently mapped because this is only used to determine if literal contextual typing need apply to the pseudotype
        // If this is used more broadly, the implementation needs to be filled out more to handle the structural pseudotypes - signatures, objects, tuples, etc
        // PORT: Go asserts `t != nil`; a `&PseudoType` is never nil.
        match t.kind {
            PseudoTypeKind::DIRECT => {
                self.get_type_from_type_node(t.as_pseudo_type_direct().type_node)
            }
            PseudoTypeKind::INFERRED => {
                let node = t.as_pseudo_type_inferred().expression;
                if t.as_pseudo_type_inferred().is_signature_return {
                    let signature = self.get_signature_from_declaration(node);
                    return self.get_return_type_of_signature(signature);
                }
                let regular = self.get_regular_type_of_expression(node);
                self.get_widened_type(regular)
            }
            PseudoTypeKind::NO_RESULT => TypeId::NIL, // TODO: extract type selection logic from `serializeTypeForDeclaration`, not needed for current usecases but needed if completeness becomes required
            PseudoTypeKind::MAYBE_CONST_LOCATION => {
                let d = t.as_pseudo_type_maybe_const_location();
                if self.is_const_context(d.node) {
                    return self.pseudo_type_to_type(b, &d.const_type);
                }
                self.pseudo_type_to_type(b, &d.regular_type)
            }
            PseudoTypeKind::UNION => {
                let mut res: Vec<TypeId> = Vec::new();
                let mut has_elided_type = false;
                let members = &t.as_pseudo_type_union().types;
                for m in members {
                    if !self.strict_null_checks
                        && (m.kind == PseudoTypeKind::UNDEFINED || m.kind == PseudoTypeKind::NULL)
                    {
                        has_elided_type = true;
                        continue;
                    }
                    let t = self.pseudo_type_to_type(b, m);
                    if t.is_nil() {
                        return TypeId::NIL; // propagate failure
                    }
                    res.push(t);
                }
                if res.len() == 1 {
                    return res[0];
                }
                if res.is_empty() {
                    if has_elided_type {
                        return self.any_type;
                    }
                    return self.never_type;
                }
                self.get_union_type(&res)
            }
            PseudoTypeKind::UNDEFINED => self.undefined_widening_type,
            PseudoTypeKind::NULL => self.null_widening_type,
            PseudoTypeKind::ANY => self.any_type,
            PseudoTypeKind::STRING => self.string_type,
            PseudoTypeKind::NUMBER => self.number_type,
            PseudoTypeKind::BIG_INT => self.bigint_type,
            PseudoTypeKind::BOOLEAN => self.boolean_type,
            PseudoTypeKind::FALSE => self.false_type,
            PseudoTypeKind::TRUE => self.true_type,
            PseudoTypeKind::STRING_LITERAL
            | PseudoTypeKind::NUMERIC_LITERAL
            | PseudoTypeKind::BIG_INT_LITERAL => {
                let source = t.as_pseudo_type_literal().node;
                self.get_regular_type_of_expression(source) // big shortcut, uses cached expression types where possible
            }
            PseudoTypeKind::OBJECT_LITERAL
            | PseudoTypeKind::SINGLE_CALL_SIGNATURE
            | PseudoTypeKind::TUPLE => TypeId::NIL, // no simple mapping to a type, since these are structural types
            _ => crate::gostd::debug::fail(
                "Unhandled pseudochecker.PseudoTypeKind in pseudoTypeToType",
            ),
        }
    }
}
