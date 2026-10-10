//! Port of `checker/jsx.go` lines 1-928.

use crate::diagnostics::Message;
use crate::jsnum::Number;
use crate::prelude::*;

// PORT: Go `JsxFlags` and `JsxReferenceKind` (jsx.go:17-32) are generated in
// `crate::flags` (`JsxFlags::INTRINSIC_NAMED_ELEMENT`,
// `JsxReferenceKind::COMPONENT`, ...), so they are not redefined here.

// Go: checker/jsx.go:34 JsxElementLinks
#[derive(Clone, Debug, Default)]
pub struct JsxElementLinks {
    pub jsx_flags: JsxFlags,                          // Flags for the JSX element
    pub resolved_jsx_element_attributes_type: TypeId, // Resolved element attributes type of a JSX opening-like element
    pub jsx_namespace: SymbolId,                      // Resolved JSX namespace symbol for this node
    pub jsx_implicit_import_container: SymbolId, // Resolved module symbol the implicit JSX import of this file should refer to
    pub first_jsx_tag_in_file: Node,             // The first JSX tag in the file
}

// Go: checker/jsx.go:42 JsxNames
// PORT: Go `var JsxNames = struct{...}{...}`. Two spellings are provided so
// both `JsxNames.intrinsic_elements` (the value, like Go) and
// `JsxNames::INTRINSIC_ELEMENTS` (associated consts) work. The braced struct
// `JsxNames {}` lives in the type namespace and the const `JsxNames` lives in
// the value namespace, so they do not clash.
#[derive(Clone, Copy, Debug)]
pub struct JsxNamesValues {
    pub jsx: &'static str,
    pub intrinsic_elements: &'static str,
    pub element_class: &'static str,
    pub element_attributes_property_name_container: &'static str,
    pub element_children_attribute_name_container: &'static str,
    pub element: &'static str,
    pub element_type: &'static str,
    pub intrinsic_attributes: &'static str,
    pub intrinsic_class_attributes: &'static str,
    pub library_managed_attributes: &'static str,
}

#[allow(non_upper_case_globals)]
pub const JsxNames: JsxNamesValues = JsxNamesValues {
    jsx: "JSX",
    intrinsic_elements: "IntrinsicElements",
    element_class: "ElementClass",
    element_attributes_property_name_container: "ElementAttributesProperty",
    element_children_attribute_name_container: "ElementChildrenAttribute",
    element: "Element",
    element_type: "ElementType",
    intrinsic_attributes: "IntrinsicAttributes",
    intrinsic_class_attributes: "IntrinsicClassAttributes",
    library_managed_attributes: "LibraryManagedAttributes",
};

pub struct JsxNames {}

impl JsxNames {
    pub const JSX: &'static str = "JSX";
    pub const INTRINSIC_ELEMENTS: &'static str = "IntrinsicElements";
    pub const ELEMENT_CLASS: &'static str = "ElementClass";
    pub const ELEMENT_ATTRIBUTES_PROPERTY_NAME_CONTAINER: &'static str =
        "ElementAttributesProperty";
    pub const ELEMENT_CHILDREN_ATTRIBUTE_NAME_CONTAINER: &'static str = "ElementChildrenAttribute";
    pub const ELEMENT: &'static str = "Element";
    pub const ELEMENT_TYPE: &'static str = "ElementType";
    pub const INTRINSIC_ATTRIBUTES: &'static str = "IntrinsicAttributes";
    pub const INTRINSIC_CLASS_ATTRIBUTES: &'static str = "IntrinsicClassAttributes";
    pub const LIBRARY_MANAGED_ATTRIBUTES: &'static str = "LibraryManagedAttributes";
}

// Go: checker/jsx.go:66 ReactNames
// PORT: same two spellings as `JsxNames`.
#[derive(Clone, Copy, Debug)]
pub struct ReactNamesValues {
    pub fragment: &'static str,
}

#[allow(non_upper_case_globals)]
pub const ReactNames: ReactNamesValues = ReactNamesValues {
    fragment: "Fragment",
};

pub struct ReactNames {}

impl ReactNames {
    pub const FRAGMENT: &'static str = "Fragment";
}

impl Checker {
    // Go: checker/jsx.go:72 checkJsxElement
    pub fn check_jsx_element(&mut self, node: Node, check_mode: CheckMode) -> TypeId {
        self.check_node_deferred(node);
        self.get_jsx_element_type_at(node)
    }

    // Go: checker/jsx.go:77 checkJsxElementDeferred
    pub fn check_jsx_element_deferred(&mut self, node: Node) {
        let opening_element = node.opening_element();
        let closing_element = node.closing_element();
        self.check_jsx_opening_like_element_or_opening_fragment(opening_element);
        // Perform resolution on the closing tag so that rename/go to definition/etc work
        if is_jsx_intrinsic_tag_name(closing_element.tag_name()) {
            self.get_intrinsic_tag_symbol(closing_element);
        } else {
            self.check_expression(closing_element.tag_name());
        }
        self.check_jsx_children(node, CheckMode::NORMAL);
    }

    // Go: checker/jsx.go:89 checkJsxExpression
    pub fn check_jsx_expression(&mut self, node: Node, check_mode: CheckMode) -> TypeId {
        self.check_grammar_jsx_expression(node);
        if node.expression().is_nil() {
            return self.error_type;
        }
        let t = self.check_expression_ex(node.expression(), check_mode);
        if node.dot_dot_dot_token().is_some() && t != self.any_type && !self.is_array_type(t) {
            self.error(node, diag::JSX_spread_child_must_be_an_array_type, args![]);
        }
        t
    }

    // Go: checker/jsx.go:101 checkJsxSelfClosingElement
    pub fn check_jsx_self_closing_element(&mut self, node: Node, check_mode: CheckMode) -> TypeId {
        self.check_node_deferred(node);
        self.get_jsx_element_type_at(node)
    }

    // Go: checker/jsx.go:106 checkJsxSelfClosingElementDeferred
    pub fn check_jsx_self_closing_element_deferred(&mut self, node: Node) {
        self.check_jsx_opening_like_element_or_opening_fragment(node);
    }

    // Go: checker/jsx.go:110 checkJsxFragment
    pub fn check_jsx_fragment(&mut self, node: Node) -> TypeId {
        self.check_jsx_opening_like_element_or_opening_fragment(node.opening_fragment());
        // by default, jsx:'react' will use jsxFactory = React.createElement and jsxFragmentFactory = React.Fragment
        // if jsxFactory compiler option is provided, ensure jsxFragmentFactory compiler option or @jsxFrag pragma is provided too
        let node_source_file = get_source_file_of_node(node);
        if self.compiler_options.get_jsx_transform_enabled()
            && (!self.compiler_options.jsx_factory.is_empty()
                || get_pragma_from_source_file(node_source_file, "jsx").is_some())
            && self.compiler_options.jsx_fragment_factory.is_empty()
            && get_pragma_from_source_file(node_source_file, "jsxfrag").is_none()
        {
            let message = if !self.compiler_options.jsx_factory.is_empty() {
                diag::The_jsxFragmentFactory_compiler_option_must_be_provided_to_use_JSX_fragments_with_the_jsxFactory_compiler_option
            } else {
                diag::An_jsxFrag_pragma_is_required_when_using_an_jsx_pragma_with_JSX_fragments
            };
            self.error(node, message, args![]);
        }
        self.check_jsx_children(node, CheckMode::NORMAL);
        let t = self.get_jsx_element_type_at(node);
        if self.is_error_type(t) {
            self.any_type
        } else {
            t
        }
    }

    // Go: checker/jsx.go:126 checkJsxAttributes
    pub fn check_jsx_attributes(&mut self, node: Node, check_mode: CheckMode) -> TypeId {
        self.check_node_deferred(node);
        self.create_jsx_attributes_type_from_attributes_property(node.parent(), check_mode)
    }

    // Go: checker/jsx.go:131 checkJsxOpeningLikeElementOrOpeningFragment
    pub fn check_jsx_opening_like_element_or_opening_fragment(&mut self, node: Node) {
        let is_node_opening_like_element = is_jsx_opening_like_element(node);
        if is_node_opening_like_element {
            self.check_grammar_jsx_element(node);
        }
        self.check_jsx_preconditions(node);
        self.mark_jsx_alias_referenced(node);
        let sig = self.get_resolved_signature(node, None, CheckMode::NORMAL);
        self.check_deprecated_signature(sig, node);
        if is_node_opening_like_element {
            let element_type_constraint = self.get_jsx_element_type_type_at(node);
            if element_type_constraint.is_some() {
                let tag_name = node.tag_name();
                let tag_type;
                if is_jsx_intrinsic_tag_name(tag_name) {
                    tag_type = self.get_string_literal_type(tag_name.text());
                } else {
                    tag_type = self.check_expression(tag_name);
                }
                let mut diags: Vec<Diagnostic> = Vec::new();
                let relation = self.assignable_relation.clone();
                if !self.check_type_related_to_ex(
                    tag_type,
                    element_type_constraint,
                    &relation,
                    tag_name,
                    Some(diag::Its_type_0_is_not_a_valid_JSX_element_type),
                    Some(&mut diags),
                ) {
                    self.add_diagnostic(new_diagnostic_chain(
                        Some(diags[0].clone()),
                        diag::X_0_cannot_be_used_as_a_JSX_component,
                        args![get_text_of_node(tag_name)],
                    ));
                }
            } else {
                let ref_kind = self.get_jsx_reference_kind(node);
                let return_type = self.get_return_type_of_signature(sig);
                self.check_jsx_return_assignable_to_appropriate_bound(ref_kind, return_type, node);
            }
        }
    }

    // Go: checker/jsx.go:160 checkJsxPreconditions
    pub fn check_jsx_preconditions(&mut self, error_node: Node) {
        // Preconditions for using JSX
        if self.compiler_options.jsx == JsxEmit::NONE {
            self.error(
                error_node,
                diag::Cannot_use_JSX_unless_the_jsx_flag_is_provided,
                args![],
            );
        }
        if self.no_implicit_any && self.get_jsx_element_type_at(error_node).is_nil() {
            self.error(
                error_node,
                diag::JSX_element_implicitly_has_type_any_because_the_global_type_JSX_Element_does_not_exist,
                args![],
            );
        }
    }

    // Go: checker/jsx.go:170 checkJsxReturnAssignableToAppropriateBound
    pub fn check_jsx_return_assignable_to_appropriate_bound(
        &mut self,
        ref_kind: JsxReferenceKind,
        elem_instance_type: TypeId,
        opening_like_element: Node,
    ) {
        let mut diags: Vec<Diagnostic> = Vec::new();
        let relation = self.assignable_relation.clone();
        if ref_kind == JsxReferenceKind::FUNCTION {
            let sfc_return_constraint =
                self.get_jsx_stateless_element_type_at(opening_like_element);
            if sfc_return_constraint.is_some() {
                self.check_type_related_to_ex(
                    elem_instance_type,
                    sfc_return_constraint,
                    &relation,
                    opening_like_element.tag_name(),
                    Some(diag::Its_return_type_0_is_not_a_valid_JSX_element),
                    Some(&mut diags),
                );
            }
        } else if ref_kind == JsxReferenceKind::COMPONENT {
            let class_constraint = self.get_jsx_element_class_type_at(opening_like_element);
            if class_constraint.is_some() {
                // Issue an error if this return type isn't assignable to JSX.ElementClass, failing that
                self.check_type_related_to_ex(
                    elem_instance_type,
                    class_constraint,
                    &relation,
                    opening_like_element.tag_name(),
                    Some(diag::Its_instance_type_0_is_not_a_valid_JSX_element),
                    Some(&mut diags),
                );
            }
        } else {
            let sfc_return_constraint =
                self.get_jsx_stateless_element_type_at(opening_like_element);
            let class_constraint = self.get_jsx_element_class_type_at(opening_like_element);
            if sfc_return_constraint.is_nil() || class_constraint.is_nil() {
                return;
            }
            let combined = self.get_union_type(&[sfc_return_constraint, class_constraint]);
            self.check_type_related_to_ex(
                elem_instance_type,
                combined,
                &relation,
                opening_like_element.tag_name(),
                Some(diag::Its_element_type_0_is_not_a_valid_JSX_element),
                Some(&mut diags),
            );
        }
        if !diags.is_empty() {
            self.add_diagnostic(new_diagnostic_chain(
                Some(diags[0].clone()),
                diag::X_0_cannot_be_used_as_a_JSX_component,
                args![get_text_of_node(opening_like_element.tag_name())],
            ));
        }
    }

    // Go: checker/jsx.go:198 inferJsxTypeArguments
    // PORT: Go passes `context.inferences`; `infer_types` takes the context id
    // (same as other ported callers).
    pub fn infer_jsx_type_arguments(
        &mut self,
        node: Node,
        signature: SignatureId,
        check_mode: CheckMode,
        context: InferenceContextId,
    ) -> Vec<TypeId> {
        let param_type = self.get_effective_first_argument_for_jsx_signature(signature, node);
        let check_attr_type = self.check_expression_with_contextual_type(
            node.attributes(),
            param_type,
            context,
            check_mode,
        );
        self.infer_types(
            context,
            check_attr_type,
            param_type,
            InferencePriority::NONE,
            false,
        );
        self.get_inferred_types(context)
    }

    // Go: checker/jsx.go:205 getContextualTypeForJsxExpression
    pub fn get_contextual_type_for_jsx_expression(
        &mut self,
        node: Node,
        context_flags: ContextFlags,
    ) -> TypeId {
        if is_jsx_attribute_like(node.parent()) {
            return self.get_contextual_type(node, context_flags);
        } else if is_jsx_element(node.parent()) {
            return self.get_contextual_type_for_child_jsx_expression(
                node.parent(),
                node,
                context_flags,
            );
        }
        TypeId::NIL
    }

    // Go: checker/jsx.go:215 getContextualTypeForJsxAttribute
    pub fn get_contextual_type_for_jsx_attribute(
        &mut self,
        attribute: Node,
        context_flags: ContextFlags,
    ) -> TypeId {
        // When we trying to resolve JsxOpeningLikeElement as a stateless function element, we will already give its attributes a contextual type
        // which is a type of the parameter of the signature we are trying out.
        // If there is no contextual type (e.g. we are trying to resolve stateful component), get attributes type from resolving element's tagName
        if is_jsx_attribute(attribute) {
            let attributes_type =
                self.get_apparent_type_of_contextual_type(attribute.parent(), context_flags);
            if attributes_type.is_nil() || self.is_type_any(attributes_type) {
                return TypeId::NIL;
            }
            return self
                .get_type_of_property_of_contextual_type(attributes_type, attribute.name().text());
        }
        self.get_contextual_type(attribute.parent(), context_flags)
    }

    // Go: checker/jsx.go:229 getContextualJsxElementAttributesType
    pub fn get_contextual_jsx_element_attributes_type(
        &mut self,
        node: Node,
        context_flags: ContextFlags,
    ) -> TypeId {
        if is_jsx_opening_element(node) && context_flags != ContextFlags::IGNORE_NODE_INFERENCES {
            let index =
                self.find_contextual_node(node.parent(), context_flags == ContextFlags::NONE);
            if index >= 0 {
                // Contextually applied type is moved from attributes up to the outer jsx attributes so when walking up from the children they get hit
                // _However_ to hit them from the _attributes_ we must look for them here; otherwise we'll used the declared type
                // (as below) instead!
                return self.contextual_infos[index as usize].t;
            }
        }
        self.get_contextual_type_for_argument_at_index(node, 0)
    }

    // Go: checker/jsx.go:242 getContextualTypeForChildJsxExpression
    pub fn get_contextual_type_for_child_jsx_expression(
        &mut self,
        node: Node,
        child: Node,
        context_flags: ContextFlags,
    ) -> TypeId {
        let attributes_type = self.get_apparent_type_of_contextual_type(
            node.opening_element().attributes(),
            context_flags,
        );
        // JSX expression is in children of JSX Element, we will look for an "children" attribute (we get the name from JSX.ElementAttributesProperty)
        let jsx_namespace = self.get_jsx_namespace_at(node);
        let jsx_children_property_name = self.get_jsx_element_children_property_name(jsx_namespace);
        if !(attributes_type.is_some()
            && !self.is_type_any(attributes_type)
            && jsx_children_property_name != INTERNAL_SYMBOL_NAME_MISSING
            && !jsx_children_property_name.is_empty())
        {
            return TypeId::NIL;
        }
        let real_children = get_semantic_jsx_children(&node.children().nodes().to_vec());
        let child_index: i32 = real_children
            .iter()
            .position(|&c| c == child)
            .map_or(-1, |i| i as i32);
        let child_field_type = self
            .get_type_of_property_of_contextual_type(attributes_type, &jsx_children_property_name);
        if child_field_type.is_nil() {
            return TypeId::NIL;
        }
        if real_children.len() == 1 {
            return child_field_type;
        }
        self.map_type_ex(
            child_field_type,
            &mut |c: &mut Checker, t: TypeId| {
                if c.is_array_like_type(t) {
                    let index_type = c.get_number_literal_type(Number(child_index as f64));
                    return c.get_indexed_access_type(t, index_type);
                }
                t
            },
            true, /*noReductions*/
        )
    }

    // Go: checker/jsx.go:266 discriminateContextualTypeByJSXAttributes
    pub fn discriminate_contextual_type_by_jsx_attributes(
        &mut self,
        node: Node,
        contextual_type: TypeId,
    ) -> TypeId {
        let key = DiscriminatedContextualTypeKey {
            node_id: node,
            type_id: self.ty(contextual_type).id,
        };
        if let Some(&discriminated) = self.discriminated_contextual_types.get(&key) {
            if discriminated.is_some() {
                return discriminated;
            }
        }
        let jsx_namespace = self.get_jsx_namespace_at(node);
        let jsx_children_property_name = self.get_jsx_element_children_property_name(jsx_namespace);
        let mut discriminant_properties: Vec<Node> = Vec::new();
        for p in node.properties() {
            let symbol = p.symbol();
            let keep = if symbol.is_nil() || !is_jsx_attribute(p) {
                false
            } else {
                let initializer = p.initializer();
                let name = self.sym(symbol).name.clone();
                (initializer.is_nil() || self.is_possibly_discriminant_value(initializer))
                    && self.is_discriminant_property(contextual_type, &name)
            };
            if keep {
                discriminant_properties.push(p);
            }
        }
        let mut discriminant_members: Vec<SymbolId> = Vec::new();
        for s in self.get_properties_of_type(contextual_type) {
            let keep = if !self.sym(s).flags.intersects(SymbolFlags::OPTIONAL)
                || node.symbol().is_nil()
            {
                false
            } else {
                let element = node.parent().parent();
                let name = self.sym(s).name.clone();
                if name == jsx_children_property_name
                    && is_jsx_element(element)
                    && !get_semantic_jsx_children(&element.children().nodes().to_vec()).is_empty()
                {
                    false
                } else {
                    let node_members = self.sym(node.symbol()).members;
                    self.symbols.get(node_members, &name).is_nil()
                        && self.is_discriminant_property(contextual_type, &name)
                }
            };
            if keep {
                discriminant_members.push(s);
            }
        }
        let mut discriminator = ObjectLiteralDiscriminator {
            props: discriminant_properties,
            members: discriminant_members,
        };
        let discriminated =
            self.discriminate_type_by_discriminable_items(contextual_type, &mut discriminator);
        self.discriminated_contextual_types
            .insert(key, discriminated);
        discriminated
    }

    // Go: checker/jsx.go:296 elaborateJsxComponents
    pub fn elaborate_jsx_components(
        &mut self,
        node: Node,
        source: TypeId,
        target: TypeId,
        relation: &Rc<RefCell<Relation>>,
        mut diagnostic_output: Option<&mut Vec<Diagnostic>>,
    ) -> bool {
        let mut reported_error = false;
        for prop in node.properties() {
            if !is_jsx_spread_attribute(prop) && !is_hyphenated_jsx_name(prop.name().text()) {
                let name_type = self.get_string_literal_type(prop.name().text());
                if name_type.is_some() && !self.ty(name_type).flags.intersects(TypeFlags::NEVER) {
                    reported_error = self.elaborate_element(
                        source,
                        target,
                        relation,
                        prop.name(),
                        prop.initializer(),
                        name_type,
                        None,
                        None,
                        diagnostic_output.as_deref_mut(),
                    ) || reported_error;
                }
            }
        }
        if is_jsx_opening_element(node.parent()) && is_jsx_element(node.parent().parent()) {
            let containing_element = node.parent().parent(); // Containing JSXElement
            let jsx_namespace = self.get_jsx_namespace_at(node);
            let mut children_prop_name = self.get_jsx_element_children_property_name(jsx_namespace);
            if children_prop_name == INTERNAL_SYMBOL_NAME_MISSING {
                children_prop_name = "children".to_string();
            }
            let children_name_type = self.get_string_literal_type(&children_prop_name);
            let children_target_type = self.get_indexed_access_type(target, children_name_type);
            let valid_children =
                get_semantic_jsx_children(&containing_element.children().nodes().to_vec());
            if valid_children.is_empty() {
                return reported_error;
            }
            let more_than_one_real_children = valid_children.len() > 1;
            let array_like_target_parts;
            let non_array_like_target_parts;
            let iterable_type = (self.get_global_iterable_type.clone())(self);
            if iterable_type != self.empty_generic_type {
                let any_type = self.any_type;
                let any_iterable = self.create_iterable_type(any_type);
                array_like_target_parts = self
                    .filter_type(children_target_type, &mut |c: &mut Checker, t: TypeId| {
                        c.is_type_assignable_to(t, any_iterable)
                    });
                non_array_like_target_parts = self
                    .filter_type(children_target_type, &mut |c: &mut Checker, t: TypeId| {
                        !c.is_type_assignable_to(t, any_iterable)
                    });
            } else {
                array_like_target_parts = self
                    .filter_type(children_target_type, &mut |c: &mut Checker, t: TypeId| {
                        c.is_array_or_tuple_like_type(t)
                    });
                non_array_like_target_parts = self
                    .filter_type(children_target_type, &mut |c: &mut Checker, t: TypeId| {
                        !c.is_array_or_tuple_like_type(t)
                    });
            }
            // PORT: Go `getInvalidTextualChildDiagnostic` is a closure that computes
            // the message and args once, on first call. The cache is shared through
            // `Rc<RefCell<..>>` so the returned `Rc<dyn Fn>` can be stored in
            // `JsxElaborationElement::create_diagnostic` closures.
            let invalid_text_diagnostic: Rc<RefCell<Option<(&'static Message, Vec<String>)>>> =
                Rc::new(RefCell::new(None));
            let get_invalid_textual_child_diagnostic: JsxInvalidTextDiagnosticFn = {
                let invalid_text_diagnostic = invalid_text_diagnostic.clone();
                let tag_name = node.parent().tag_name();
                let children_prop_name = children_prop_name.clone();
                Rc::new(move |c: &mut Checker| {
                    if invalid_text_diagnostic.borrow().is_none() {
                        let tag_name_text = get_text_of_node(tag_name);
                        let message = diag::X_0_components_don_t_accept_text_as_child_elements_Text_in_JSX_has_the_type_string_but_the_expected_type_of_1_is_2;
                        let children_target_type_string = c.type_to_string(children_target_type);
                        *invalid_text_diagnostic.borrow_mut() = Some((
                            message,
                            args![
                                tag_name_text,
                                children_prop_name,
                                children_target_type_string
                            ],
                        ));
                    }
                    invalid_text_diagnostic
                        .borrow()
                        .clone()
                        .expect("invalid text diagnostic")
                })
            };
            if more_than_one_real_children {
                if array_like_target_parts != self.never_type {
                    let child_types =
                        self.check_jsx_children(containing_element, CheckMode::NORMAL);
                    let real_source = self.create_tuple_type(&child_types);
                    let mut children = self.generate_jsx_children(
                        containing_element,
                        get_invalid_textual_child_diagnostic.clone(),
                    );
                    reported_error = self.elaborate_iterable_or_array_like_target_elementwise(
                        &mut children,
                        real_source,
                        array_like_target_parts,
                        relation,
                        diagnostic_output.as_deref_mut(),
                    ) || reported_error;
                } else {
                    let source_children_type =
                        self.get_indexed_access_type(source, children_name_type);
                    if !self.is_type_related_to(
                        source_children_type,
                        children_target_type,
                        relation,
                    ) {
                        // arity mismatch
                        let children_target_type_string = self.type_to_string(children_target_type);
                        let diag = self.error(
                            containing_element.opening_element().tag_name(),
                            diag::This_JSX_tag_s_0_prop_expects_a_single_child_of_type_1_but_multiple_children_were_provided,
                            args![children_prop_name, children_target_type_string],
                        );
                        self.report_diagnostic(diag, diagnostic_output.as_deref_mut());
                        reported_error = true;
                    }
                }
            } else {
                if non_array_like_target_parts != self.never_type {
                    let child = valid_children[0];
                    let e = self.get_elaboration_element_for_jsx_child(
                        child,
                        children_name_type,
                        get_invalid_textual_child_diagnostic.clone(),
                    );
                    if e.error_node.is_some() {
                        let create_diagnostic = e.create_diagnostic.clone();
                        let mut factory = |c: &mut Checker, prop: Node| -> Diagnostic {
                            (create_diagnostic.as_ref().expect("create_diagnostic"))(c, prop)
                        };
                        let factory_opt: Option<&mut dyn FnMut(&mut Checker, Node) -> Diagnostic> =
                            if e.create_diagnostic.is_some() {
                                Some(&mut factory)
                            } else {
                                None
                            };
                        reported_error = self.elaborate_element(
                            source,
                            target,
                            relation,
                            e.error_node,
                            e.inner_expression,
                            e.name_type,
                            None,
                            factory_opt,
                            diagnostic_output.as_deref_mut(),
                        ) || reported_error;
                    }
                } else {
                    let source_children_type =
                        self.get_indexed_access_type(source, children_name_type);
                    if !self.is_type_related_to(
                        source_children_type,
                        children_target_type,
                        relation,
                    ) {
                        // arity mismatch
                        let children_target_type_string = self.type_to_string(children_target_type);
                        let diag = self.error(
                            containing_element.opening_element().tag_name(),
                            diag::This_JSX_tag_s_0_prop_expects_type_1_which_requires_multiple_children_but_only_a_single_child_was_provided,
                            args![children_prop_name, children_target_type_string],
                        );
                        self.report_diagnostic(diag, diagnostic_output.as_deref_mut());
                        reported_error = true;
                    }
                }
            }
        }
        reported_error
    }
}

// PORT: Go `func() (*diagnostics.Message, []any)` passed to the JSX child
// elaboration helpers. Stored in closures, so it is an `Rc<dyn Fn>`.
pub type JsxInvalidTextDiagnosticFn = Rc<dyn Fn(&mut Checker) -> (&'static Message, Vec<String>)>;

// Go: checker/jsx.go:369 JsxElaborationElement
// PORT: Go `createDiagnostic func(prop *ast.Node) *ast.Diagnostic` (nil-able)
// is `Option<Rc<dyn Fn(&mut Checker, Node) -> Diagnostic>>`.
#[derive(Clone, Default)]
pub struct JsxElaborationElement {
    pub error_node: Node,
    pub inner_expression: Node,
    pub name_type: TypeId,
    pub create_diagnostic: Option<Rc<dyn Fn(&mut Checker, Node) -> Diagnostic>>, // Optional: creates a custom diagnostic for this element
}

// PORT: Go `generateJsxChildren` returns a lazy `iter.Seq[JsxElaborationElement]`.
// The Rust iterator keeps the same lazy order: each `next` call creates the
// number literal name types and elaborates only up to the next yielded child,
// so type creation interleaves with the consumer exactly as in Go.
pub struct JsxChildrenIterator {
    pub node: Node,
    pub index: usize,
    pub member_offset: i32,
    pub get_invalid_text_diagnostic: JsxInvalidTextDiagnosticFn,
}

impl JsxChildrenIterator {
    // Go: checker/jsx.go:376 generateJsxChildren (loop body)
    pub fn next(&mut self, c: &mut Checker) -> Option<JsxElaborationElement> {
        let children = self.node.children().nodes().to_vec();
        while self.index < children.len() {
            let i = self.index;
            self.index += 1;
            let child = children[i];
            let name_type =
                c.get_number_literal_type(Number((i as i32 - self.member_offset) as f64));
            let e = c.get_elaboration_element_for_jsx_child(
                child,
                name_type,
                self.get_invalid_text_diagnostic.clone(),
            );
            if e.error_node.is_some() {
                return Some(e);
            } else {
                self.member_offset += 1;
            }
        }
        None
    }
}

impl Checker {
    // Go: checker/jsx.go:376 generateJsxChildren
    pub fn generate_jsx_children(
        &mut self,
        node: Node,
        get_invalid_text_diagnostic: JsxInvalidTextDiagnosticFn,
    ) -> JsxChildrenIterator {
        JsxChildrenIterator {
            node,
            index: 0,
            member_offset: 0,
            get_invalid_text_diagnostic,
        }
    }

    // Go: checker/jsx.go:393 getElaborationElementForJsxChild
    pub fn get_elaboration_element_for_jsx_child(
        &mut self,
        child: Node,
        name_type: TypeId,
        get_invalid_text_diagnostic: JsxInvalidTextDiagnosticFn,
    ) -> JsxElaborationElement {
        match child.kind() {
            SyntaxKind::JsxExpression => {
                // child is of the type of the expression
                return JsxElaborationElement {
                    error_node: child,
                    inner_expression: child.expression(),
                    name_type,
                    create_diagnostic: None,
                };
            }
            SyntaxKind::JsxText => {
                if child.contains_only_trivia_white_spaces() {
                    // Whitespace only jsx text isn't real jsx text
                    return JsxElaborationElement::default();
                }
                // child is a string
                return JsxElaborationElement {
                    error_node: child,
                    inner_expression: Node::NIL,
                    name_type,
                    create_diagnostic: Some(Rc::new(
                        move |c: &mut Checker, prop: Node| -> Diagnostic {
                            let (error_message, error_args) = get_invalid_text_diagnostic(c);
                            new_diagnostic_for_node(prop, error_message, error_args)
                        },
                    )),
                };
            }
            SyntaxKind::JsxElement
            | SyntaxKind::JsxSelfClosingElement
            | SyntaxKind::JsxFragment => {
                // child is of type JSX.Element
                return JsxElaborationElement {
                    error_node: child,
                    inner_expression: child,
                    name_type,
                    create_diagnostic: None,
                };
            }
            _ => {}
        }
        panic!("Unhandled case in getElaborationElementForJsxChild")
    }

    // Go: checker/jsx.go:420 elaborateIterableOrArrayLikeTargetElementwise
    // PORT: Go `iter.Seq[JsxElaborationElement]` is the lazy `JsxChildrenIterator`
    // (its only producer is `generateJsxChildren`).
    pub fn elaborate_iterable_or_array_like_target_elementwise(
        &mut self,
        iterator: &mut JsxChildrenIterator,
        source: TypeId,
        target: TypeId,
        relation: &Rc<RefCell<Relation>>,
        mut diagnostic_output: Option<&mut Vec<Diagnostic>>,
    ) -> bool {
        let tuple_or_array_like_target_parts = self
            .filter_type(target, &mut |c: &mut Checker, t: TypeId| {
                c.is_array_or_tuple_like_type(t)
            });
        let non_tuple_or_array_like_target_parts = self
            .filter_type(target, &mut |c: &mut Checker, t: TypeId| {
                !c.is_array_or_tuple_like_type(t)
            });
        // If `nonTupleOrArrayLikeTargetParts` is not `never`, then that should mean `Iterable` is defined.
        let mut iteration_type = TypeId::NIL;
        if non_tuple_or_array_like_target_parts != self.never_type {
            iteration_type = self.get_iteration_type_of_iterable(
                IterationUse::FOR_OF,
                IterationTypeKind::YIELD,
                non_tuple_or_array_like_target_parts,
                Node::NIL, /*errorNode*/
            );
        }
        let mut reported_error = false;
        while let Some(e) = iterator.next(self) {
            let prop = e.error_node;
            let next = e.inner_expression;
            let name_type = e.name_type;
            let mut target_prop_type = iteration_type;
            let mut target_indexed_prop_type = TypeId::NIL;
            if tuple_or_array_like_target_parts != self.never_type {
                target_indexed_prop_type = self.get_best_match_indexed_access_type_or_undefined(
                    source,
                    tuple_or_array_like_target_parts,
                    name_type,
                );
            }
            if target_indexed_prop_type.is_some()
                && !self
                    .ty(target_indexed_prop_type)
                    .flags
                    .intersects(TypeFlags::INDEXED_ACCESS)
            {
                if iteration_type.is_some() {
                    target_prop_type =
                        self.get_union_type(&[iteration_type, target_indexed_prop_type]);
                } else {
                    target_prop_type = target_indexed_prop_type;
                }
            }
            if target_prop_type.is_nil() {
                continue;
            }
            let mut source_prop_type = self.get_indexed_access_type_or_undefined(
                source,
                name_type,
                AccessFlags::NONE,
                Node::NIL,
                None,
            );
            if source_prop_type.is_nil() {
                continue;
            }
            let prop_name =
                self.get_property_name_from_index(name_type, Node::NIL /*accessNode*/);
            if !self.check_type_related_to(
                source_prop_type,
                target_prop_type,
                relation,
                Node::NIL, /*errorNode*/
            ) {
                let elaborated = next.is_some()
                    && self.elaborate_error(
                        next,
                        source_prop_type,
                        target_prop_type,
                        relation,
                        None, /*headMessage*/
                        diagnostic_output.as_deref_mut(),
                    );
                reported_error = true;
                if !elaborated {
                    // Issue error on the prop itself, since the prop couldn't elaborate the error. Use the expression type, if available.
                    let mut specific_source = source_prop_type;
                    if next.is_some() {
                        specific_source = self
                            .check_expression_for_mutable_location_with_contextual_type(
                                next,
                                source_prop_type,
                            );
                    }
                    if let Some(create_diagnostic) = e.create_diagnostic.clone() {
                        // Use the custom diagnostic factory if provided (e.g., for JSX text children with dynamic error messages)
                        let diagnostic = create_diagnostic(self, prop);
                        self.report_diagnostic(diagnostic, diagnostic_output.as_deref_mut());
                    } else if self.exact_optional_property_types
                        && self
                            .is_exact_optional_property_mismatch(specific_source, target_prop_type)
                    {
                        let specific_source_string = self.type_to_string(specific_source);
                        let target_prop_type_string = self.type_to_string(target_prop_type);
                        let diag = create_diagnostic_for_node(
                            prop,
                            diag::Type_0_is_not_assignable_to_type_1_with_exactOptionalPropertyTypes_Colon_true_Consider_adding_undefined_to_the_type_of_the_target,
                            args![specific_source_string, target_prop_type_string],
                        );
                        self.report_diagnostic(diag, diagnostic_output.as_deref_mut());
                    } else {
                        let target_is_optional = prop_name != INTERNAL_SYMBOL_NAME_MISSING && {
                            let mut p = self
                                .get_property_of_type(tuple_or_array_like_target_parts, &prop_name);
                            if p.is_nil() {
                                p = self.unknown_symbol;
                            }
                            self.sym(p).flags.intersects(SymbolFlags::OPTIONAL)
                        };
                        let source_is_optional = prop_name != INTERNAL_SYMBOL_NAME_MISSING && {
                            let mut p = self.get_property_of_type(source, &prop_name);
                            if p.is_nil() {
                                p = self.unknown_symbol;
                            }
                            self.sym(p).flags.intersects(SymbolFlags::OPTIONAL)
                        };
                        target_prop_type =
                            self.remove_missing_type(target_prop_type, target_is_optional);
                        source_prop_type = self.remove_missing_type(
                            source_prop_type,
                            target_is_optional && source_is_optional,
                        );
                        let result = self.check_type_related_to_ex(
                            specific_source,
                            target_prop_type,
                            relation,
                            prop,
                            None,
                            diagnostic_output.as_deref_mut(),
                        );
                        if result && specific_source != source_prop_type {
                            // If for whatever reason the expression type doesn't yield an error, make sure we still issue an error on the sourcePropType
                            self.check_type_related_to_ex(
                                source_prop_type,
                                target_prop_type,
                                relation,
                                prop,
                                None,
                                diagnostic_output.as_deref_mut(),
                            );
                        }
                    }
                }
            }
        }
        reported_error
    }

    // Go: checker/jsx.go:485 getSuggestedSymbolForNonexistentJSXAttribute
    pub fn get_suggested_symbol_for_nonexistent_jsx_attribute(
        &mut self,
        name: &str,
        containing_type: TypeId,
    ) -> SymbolId {
        let properties = self.get_properties_of_type(containing_type);
        let mut jsx_specific = SymbolId::NIL;
        match name {
            "for" => {
                jsx_specific = properties
                    .iter()
                    .copied()
                    .find(|&x| symbol_name(&self.symbols, x) == "htmlFor")
                    .unwrap_or(SymbolId::NIL);
            }
            "class" => {
                jsx_specific = properties
                    .iter()
                    .copied()
                    .find(|&x| symbol_name(&self.symbols, x) == "className")
                    .unwrap_or(SymbolId::NIL);
            }
            _ => {}
        }
        if jsx_specific.is_some() {
            return jsx_specific;
        }
        self.get_spelling_suggestion_for_name(name, &properties, SymbolFlags::VALUE)
    }

    // Go: checker/jsx.go:500 getJSXFragmentType
    pub fn get_jsx_fragment_type(&mut self, node: Node) -> TypeId {
        // An opening fragment is required in order for `getJsxNamespace` to give the fragment factory
        let file = get_source_file_of_node(node);
        let cached = self.source_file_links.get(file).jsx_fragment_type;
        if cached.is_some() {
            return cached;
        }
        let jsx_fragment_factory_name = self.get_jsx_namespace(node);
        // #38720/60122, allow null as jsxFragmentFactory
        let should_resolve_factory_reference = (self.compiler_options.jsx == JsxEmit::REACT
            || !self.compiler_options.jsx_fragment_factory.is_empty())
            && jsx_fragment_factory_name != "null";
        if !should_resolve_factory_reference {
            let any_type = self.any_type;
            self.source_file_links.get(file).jsx_fragment_type = any_type;
            return any_type;
        }
        let mut jsx_factory_symbol = self.get_jsx_namespace_container_for_implicit_import(node);
        if jsx_factory_symbol.is_nil() {
            let should_module_ref_err = self.compiler_options.jsx != JsxEmit::PRESERVE
                && self.compiler_options.jsx != JsxEmit::REACT_NATIVE;
            let mut flags = SymbolFlags::VALUE;
            if !should_module_ref_err {
                flags = flags.without(SymbolFlags::ENUM);
            }
            jsx_factory_symbol = self.resolve_name(
                node,
                &jsx_fragment_factory_name,
                flags,
                Some(diag::Using_JSX_fragments_requires_fragment_factory_0_to_be_in_scope_but_it_could_not_be_found),
                true,  /*isUse*/
                false, /*excludeGlobals*/
            );
        }
        if jsx_factory_symbol.is_nil() {
            let error_type = self.error_type;
            self.source_file_links.get(file).jsx_fragment_type = error_type;
            return error_type;
        }
        if self.sym(jsx_factory_symbol).name == ReactNames.fragment {
            let t = self.get_type_of_symbol(jsx_factory_symbol);
            self.source_file_links.get(file).jsx_fragment_type = t;
            return t;
        }
        let mut resolved_alias = jsx_factory_symbol;
        if self
            .sym(jsx_factory_symbol)
            .flags
            .intersects(SymbolFlags::ALIAS)
        {
            resolved_alias = self.resolve_alias(jsx_factory_symbol);
        }

        let react_exports = self.get_exports_of_symbol(resolved_alias);
        let type_symbol = self.get_symbol(
            react_exports,
            ReactNames.fragment,
            SymbolFlags::BLOCK_SCOPED_VARIABLE,
        );
        let t = if type_symbol.is_some() {
            self.get_type_of_symbol(type_symbol)
        } else {
            self.error_type
        };
        self.source_file_links.get(file).jsx_fragment_type = t;
        t
    }

    // Go: checker/jsx.go:545 resolveJsxOpeningLikeElement
    pub fn resolve_jsx_opening_like_element(
        &mut self,
        node: Node,
        candidates_out_array: Option<&mut Vec<SignatureId>>,
        check_mode: CheckMode,
    ) -> SignatureId {
        let is_jsx_open_fragment = is_jsx_opening_fragment(node);
        let expr_types;
        if !is_jsx_open_fragment {
            if is_jsx_intrinsic_tag_name(node.tag_name()) {
                let result = self.get_intrinsic_attributes_type_from_jsx_opening_like_element(node);
                let fake_signature = self.create_signature_for_jsx_intrinsic(node, result);
                let first_argument =
                    self.get_effective_first_argument_for_jsx_signature(fake_signature, node);
                let attributes_type = self.check_expression_with_contextual_type(
                    node.attributes(),
                    first_argument,
                    InferenceContextId::NIL, /*inferenceContext*/
                    CheckMode::NORMAL,
                );
                self.check_type_assignable_to_and_optionally_elaborate(
                    attributes_type,
                    result,
                    node.tag_name(),
                    node.attributes(),
                    None,
                    None,
                );
                let type_arguments = node.type_arguments().to_vec();
                if !type_arguments.is_empty() {
                    self.check_source_elements(type_arguments.iter().copied());
                    let source_file = get_source_file_of_node(node);
                    let type_argument_list = node.type_argument_list();
                    let loc = TextRange::new(
                        skip_trivia(&source_file_text(source_file), type_argument_list.pos()),
                        type_argument_list.end(),
                    );
                    self.add_diagnostic(new_diagnostic(
                        source_file,
                        loc,
                        diag::Expected_0_type_arguments_but_got_1,
                        args![0, type_arguments.len()],
                    ));
                }
                return fake_signature;
            }
            expr_types = self.check_expression(node.tag_name());
        } else {
            expr_types = self.get_jsx_fragment_type(node);
        }
        let apparent_type = self.get_apparent_type(expr_types);
        if self.is_error_type(apparent_type) {
            return self.resolve_error_call(node);
        }
        let signatures = self.get_uninstantiated_jsx_signatures_of_type(expr_types, node);
        if self.is_untyped_function_call(
            expr_types,
            apparent_type,
            signatures.len() as i32,
            0, /*constructSignatures*/
        ) {
            return self.resolve_untyped_call(node);
        }
        if signatures.is_empty() {
            // We found no signatures at all, which is an error
            if is_jsx_open_fragment {
                self.error(
                    node,
                    diag::JSX_element_type_0_does_not_have_any_construct_or_call_signatures,
                    args![get_text_of_node(node)],
                );
            } else {
                self.error(
                    node.tag_name(),
                    diag::JSX_element_type_0_does_not_have_any_construct_or_call_signatures,
                    args![get_text_of_node(node.tag_name())],
                );
            }
            return self.resolve_error_call(node);
        }
        self.resolve_call(
            node,
            &signatures,
            candidates_out_array,
            check_mode,
            SignatureFlags::NONE,
            None,
        )
    }

    // Check if the given signature can possibly be a signature called by the JSX opening-like element.
    // @param node a JSX opening-like element we are trying to figure its call signature
    // @param signature a candidate signature we are trying whether it is a call signature
    // @param relation a relationship to check parameter and argument type
    // Go: checker/jsx.go:591 checkApplicableSignatureForJsxCallLikeElement
    pub fn check_applicable_signature_for_jsx_call_like_element(
        &mut self,
        node: Node,
        signature: SignatureId,
        relation: &Rc<RefCell<Relation>>,
        check_mode: CheckMode,
        report_errors: bool,
        mut diagnostic_output: Option<&mut Vec<Diagnostic>>,
    ) -> bool {
        // Stateless function components can have maximum of three arguments: "props", "context", and "updater".
        // However "context" and "updater" are implicit and can't be specify by users. Only the first parameter, props,
        // can be specified by users through attributes property.
        let param_type = self.get_effective_first_argument_for_jsx_signature(signature, node);
        let attributes_type;
        if is_jsx_opening_fragment(node) {
            attributes_type =
                self.create_jsx_attributes_type_from_attributes_property(node, CheckMode::NORMAL);
        } else {
            attributes_type = self.check_expression_with_contextual_type(
                node.attributes(),
                param_type,
                InferenceContextId::NIL, /*inferenceContext*/
                check_mode,
            );
        }
        let check_attributes_type;
        // PORT: Go closure `checkTagNameDoesNotExpectTooManyArguments` captures
        // `c` and `diagnosticOutput`; here it takes both as parameters.
        let check_tag_name_does_not_expect_too_many_arguments = |c: &mut Checker,
                                                                 diagnostic_output: Option<
            &mut Vec<Diagnostic>,
        >|
         -> bool {
            if c.get_jsx_namespace_container_for_implicit_import(node)
                .is_some()
            {
                return true; // factory is implicitly jsx/jsxdev - assume it fits the bill, since we don't strongly look for the jsx/jsxs/jsxDEV factory APIs anywhere else (at least not yet)
            }
            // We assume fragments have the correct arity since the node does not have attributes
            let mut tag_type = TypeId::NIL;
            if (is_jsx_opening_element(node) || is_jsx_self_closing_element(node))
                && !(is_jsx_intrinsic_tag_name(node.tag_name())
                    || is_jsx_namespaced_name(node.tag_name()))
            {
                tag_type = c.check_expression(node.tag_name());
            }
            if tag_type.is_nil() {
                return true;
            }
            let tag_call_signatures = c.get_signatures_of_type(tag_type, SignatureKind::CALL);
            if tag_call_signatures.is_empty() {
                return true;
            }
            let factory = c.get_jsx_factory_entity(node);
            if factory.is_nil() {
                return true;
            }
            let factory_symbol = c.resolve_entity_name(
                factory,
                SymbolFlags::VALUE,
                true,  /*ignoreErrors*/
                false, /*dontResolveAlias*/
                node,
            );
            if factory_symbol.is_nil() {
                return true;
            }

            let factory_type = c.get_type_of_symbol(factory_symbol);
            let call_signatures = c.get_signatures_of_type(factory_type, SignatureKind::CALL);
            if call_signatures.is_empty() {
                return true;
            }
            let mut has_first_param_signatures = false;
            let mut max_param_count: i32 = 0;
            // Check that _some_ first parameter expects a FC-like thing, and that some overload of the SFC expects an acceptable number of arguments
            for sig in call_signatures.iter().copied() {
                let firstparam = c.get_type_at_position(sig, 0);
                let signatures_of_param = c.get_signatures_of_type(firstparam, SignatureKind::CALL);
                if signatures_of_param.is_empty() {
                    continue;
                }
                for param_sig in signatures_of_param.iter().copied() {
                    has_first_param_signatures = true;
                    if c.has_effective_rest_parameter(param_sig) {
                        return true; // some signature has a rest param, so function components can have an arbitrary number of arguments
                    }
                    let param_count = c.get_parameter_count(param_sig);
                    if param_count > max_param_count {
                        max_param_count = param_count;
                    }
                }
            }
            if !has_first_param_signatures {
                // Not a single signature had a first parameter which expected a signature - for back compat, and
                // to guard against generic factories which won't have signatures directly, do not error
                return true;
            }
            // PORT: Go `math.MaxInt`; counts are `i32` here.
            let mut absolute_min_arg_count = i32::MAX;
            for tag_sig in tag_call_signatures.iter().copied() {
                let tag_required_arg_count = c.get_min_argument_count(tag_sig);
                if tag_required_arg_count < absolute_min_arg_count {
                    absolute_min_arg_count = tag_required_arg_count;
                }
            }
            if absolute_min_arg_count <= max_param_count {
                return true; // some signature accepts the number of arguments the function component provides
            }
            if report_errors {
                let tag_name = node.tag_name();
                // We will not report errors in this function for fragments, since we do not check them in this function
                // PORT: Go `entityNameToString` (checker wrapper over scanner text);
                // call the ast version with `get_text_of_node` directly.
                let mut diag = new_diagnostic_for_node(
                        tag_name,
                        diag::Tag_0_expects_at_least_1_arguments_but_the_JSX_factory_2_provides_at_most_3,
                        args![
                            crate::ast::entity_name_to_string(tag_name, Some(&get_text_of_node)),
                            absolute_min_arg_count,
                            crate::ast::entity_name_to_string(factory, Some(&get_text_of_node)),
                            max_param_count
                        ],
                    );
                let tag_name_symbol = c.get_symbol_at_location(tag_name, false);
                if tag_name_symbol.is_some() && c.sym(tag_name_symbol).value_declaration.is_some() {
                    let value_declaration = c.sym(tag_name_symbol).value_declaration;
                    diag.add_related_info(Some(new_diagnostic_for_node(
                        value_declaration,
                        diag::X_0_is_declared_here,
                        args![crate::ast::entity_name_to_string(
                            tag_name,
                            Some(&get_text_of_node)
                        )],
                    )));
                }
                c.report_diagnostic(diag, diagnostic_output);
            }
            false
        };
        if check_mode.intersects(CheckMode::SKIP_CONTEXT_SENSITIVE) {
            check_attributes_type = self.get_regular_type_of_object_literal(attributes_type);
        } else {
            check_attributes_type = attributes_type;
        }
        if !check_tag_name_does_not_expect_too_many_arguments(
            self,
            diagnostic_output.as_deref_mut(),
        ) {
            return false;
        }
        let mut error_node = Node::NIL;
        if report_errors {
            if is_jsx_opening_fragment(node) {
                error_node = node;
            } else {
                error_node = node.tag_name();
            }
        }
        let mut attributes = Node::NIL;
        if !is_jsx_opening_fragment(node) {
            attributes = node.attributes();
        }
        self.check_type_related_to_and_optionally_elaborate(
            check_attributes_type,
            param_type,
            relation,
            error_node,
            attributes,
            None,
            diagnostic_output,
        )
    }

    // Get attributes type of the JSX opening-like element. The result is from resolving "attributes" property of the opening-like element.
    //
    // @param openingLikeElement a JSX opening-like element
    // @param filter a function to remove attributes that will not participate in checking whether attributes are assignable
    // @return an anonymous type (similar to the one returned by checkObjectLiteral) in which its properties are attributes property.
    // @remarks Because this function calls getSpreadType, it needs to use the same checks as checkObjectLiteral,
    // which also calls getSpreadType.
    // Go: checker/jsx.go:710 createJsxAttributesTypeFromAttributesProperty
    pub fn create_jsx_attributes_type_from_attributes_property(
        &mut self,
        opening_like_element: Node,
        check_mode: CheckMode,
    ) -> TypeId {
        let mut all_attributes_table = SymbolTable::NIL;
        if self.strict_null_checks {
            all_attributes_table = self.symbols.new_table();
        }
        let mut attributes_table = self.symbols.new_table();
        let mut attributes_symbol = SymbolId::NIL;
        let mut attribute_parent = opening_like_element;
        let mut spread = self.empty_jsx_object_type;
        let mut has_spread_any_type = false;
        let mut type_to_intersect = TypeId::NIL;
        let mut explicitly_specify_children_attribute = false;
        let mut object_flags = ObjectFlags::JSX_ATTRIBUTES;
        // PORT: Go closure `createJsxAttributesType` captures and mutates
        // `objectFlags`, and reads `attributesSymbol` and `attributesTable`.
        // Here those are passed in explicitly.
        let create_jsx_attributes_type = |c: &mut Checker,
                                          object_flags: &mut ObjectFlags,
                                          attributes_symbol: SymbolId,
                                          attributes_table: SymbolTable|
         -> TypeId {
            *object_flags = *object_flags | ObjectFlags::FRESH_LITERAL;
            let result = c.new_anonymous_type(attributes_symbol, attributes_table, &[], &[], &[]);
            let flags = *object_flags
                | ObjectFlags::OBJECT_LITERAL
                | ObjectFlags::CONTAINS_OBJECT_OR_ARRAY_LITERAL;
            let r = c.ty_mut(result);
            r.object_flags = r.object_flags | flags;
            result
        };
        let jsx_namespace = self.get_jsx_namespace_at(opening_like_element);
        let jsx_children_property_name = self.get_jsx_element_children_property_name(jsx_namespace);
        let is_jsx_open_fragment = is_jsx_opening_fragment(opening_like_element);
        if !is_jsx_open_fragment {
            let attributes = opening_like_element.attributes();
            attributes_symbol = attributes.symbol();
            attribute_parent = attributes;
            let contextual_type = self.get_contextual_type(attributes, ContextFlags::NONE);
            // Create anonymous type from given attributes symbol table.
            // @param symbol a symbol of JsxAttributes containing attributes corresponding to attributesTable
            // @param attributesTable a symbol table of attributes property
            for attribute_decl in attributes.properties() {
                let member = attribute_decl.symbol();
                if is_jsx_attribute(attribute_decl) {
                    let expr_type = self.check_jsx_attribute(attribute_decl, check_mode);
                    object_flags = object_flags
                        | (self.ty(expr_type).object_flags & ObjectFlags::PROPAGATING_FLAGS);
                    let member_flags = self.sym(member).flags;
                    let member_name = self.sym(member).name.clone();
                    let attribute_symbol =
                        self.new_symbol(SymbolFlags::PROPERTY | member_flags, &member_name);
                    let member_declarations = self.sym(member).declarations.clone();
                    let member_parent = self.sym(member).parent;
                    let member_value_declaration = self.sym(member).value_declaration;
                    self.sym_mut(attribute_symbol).declarations = member_declarations;
                    self.sym_mut(attribute_symbol).parent = member_parent;
                    if member_value_declaration.is_some() {
                        self.sym_mut(attribute_symbol).value_declaration = member_value_declaration;
                    }
                    let links = self
                        .value_symbol_links
                        .get_by_id(&self.symbols, attribute_symbol);
                    links.resolved_type = expr_type;
                    links.target = member;
                    let attribute_symbol_name = self.sym(attribute_symbol).name.clone();
                    self.symbols.set(
                        attributes_table,
                        attribute_symbol_name.clone(),
                        attribute_symbol,
                    );
                    if all_attributes_table.is_some() {
                        self.symbols.set(
                            all_attributes_table,
                            attribute_symbol_name,
                            attribute_symbol,
                        );
                    }
                    if attribute_decl.name().text() == jsx_children_property_name {
                        explicitly_specify_children_attribute = true;
                    }
                    if contextual_type.is_some()
                        && check_mode.intersects(CheckMode::INFERENTIAL)
                        && !check_mode.intersects(CheckMode::SKIP_CONTEXT_SENSITIVE)
                        && self.is_context_sensitive(attribute_decl)
                    {
                        let inference_context = self.get_inference_context(attributes);
                        debug_assert!(inference_context.is_some());
                        // In CheckMode.Inferential we should always have an inference context
                        let inference_node = attribute_decl.initializer().expression();
                        self.add_intra_expression_inference_site(
                            inference_context,
                            inference_node,
                            expr_type,
                        );
                    }
                } else {
                    debug_assert!(attribute_decl.kind() == SyntaxKind::JsxSpreadAttribute);
                    if self.symbols.len(attributes_table) != 0 {
                        let attrs_type = create_jsx_attributes_type(
                            self,
                            &mut object_flags,
                            attributes_symbol,
                            attributes_table,
                        );
                        spread = self.get_spread_type(
                            spread,
                            attrs_type,
                            attributes_symbol,
                            object_flags,
                            false, /*readonly*/
                        );
                        attributes_table = self.symbols.new_table();
                    }
                    let checked = self.check_expression_ex(
                        attribute_decl.expression(),
                        check_mode & CheckMode::INFERENTIAL,
                    );
                    let expr_type = self.get_reduced_type(checked);
                    if self.is_type_any(expr_type) {
                        has_spread_any_type = true;
                    }
                    if self.is_valid_spread_type(expr_type) {
                        spread = self.get_spread_type(
                            spread,
                            expr_type,
                            attributes_symbol,
                            object_flags,
                            false, /*readonly*/
                        );
                        if all_attributes_table.is_some() {
                            self.check_spread_prop_overrides(
                                expr_type,
                                all_attributes_table,
                                attribute_decl,
                            );
                        }
                    } else {
                        self.error(
                            attribute_decl.expression(),
                            diag::Spread_types_may_only_be_created_from_object_types,
                            args![],
                        );
                        if type_to_intersect.is_some() {
                            type_to_intersect =
                                self.get_intersection_type(&[type_to_intersect, expr_type]);
                        } else {
                            type_to_intersect = expr_type;
                        }
                    }
                }
            }
            if !has_spread_any_type {
                if self.symbols.len(attributes_table) != 0 {
                    let attrs_type = create_jsx_attributes_type(
                        self,
                        &mut object_flags,
                        attributes_symbol,
                        attributes_table,
                    );
                    spread = self.get_spread_type(
                        spread,
                        attrs_type,
                        attributes_symbol,
                        object_flags,
                        false, /*readonly*/
                    );
                }
            }
        }
        let parent_has_semantic_jsx_children = |opening_like_element: Node| -> bool {
            // Handle children attribute
            let parent = opening_like_element.parent();
            if parent.is_nil() {
                return false;
            }
            let mut children: Vec<Node> = Vec::new();

            if is_jsx_element(parent) {
                // We have to check that openingElement of the parent is the one we are visiting as this may not be true for selfClosingElement
                if parent.opening_element() == opening_like_element {
                    children = parent.children().nodes().to_vec();
                }
            } else if is_jsx_fragment(parent) {
                if parent.opening_fragment() == opening_like_element {
                    children = parent.children().nodes().to_vec();
                }
            }
            !get_semantic_jsx_children(&children).is_empty()
        };
        if parent_has_semantic_jsx_children(opening_like_element) {
            let child_types: Vec<TypeId> =
                self.check_jsx_children(opening_like_element.parent(), check_mode);
            if !has_spread_any_type
                && jsx_children_property_name != INTERNAL_SYMBOL_NAME_MISSING
                && !jsx_children_property_name.is_empty()
            {
                // Error if there is a attribute named "children" explicitly specified and children element.
                // This is because children element will overwrite the value from attributes.
                // Note: we will not warn "children" attribute overwritten if "children" attribute is specified in object spread.
                if explicitly_specify_children_attribute {
                    self.error(
                        attribute_parent,
                        diag::X_0_are_specified_twice_The_attribute_named_0_will_be_overwritten,
                        args![jsx_children_property_name],
                    );
                }
                let mut children_contextual_type = TypeId::NIL;
                if is_jsx_opening_element(opening_like_element) {
                    let contextual_type = self.get_apparent_type_of_contextual_type(
                        opening_like_element.attributes(),
                        ContextFlags::NONE,
                    );
                    if contextual_type.is_some() {
                        children_contextual_type = self.get_type_of_property_of_contextual_type(
                            contextual_type,
                            &jsx_children_property_name,
                        );
                    }
                }
                // If there are children in the body of JSX element, create dummy attribute "children" with the union of children types so that it will pass the attribute checking process
                let children_prop_symbol =
                    self.new_symbol(SymbolFlags::PROPERTY, &jsx_children_property_name);
                // Go `links := c.valueSymbolLinks.Get(childrenPropSymbol)` gives the id here.
                self.value_symbol_links
                    .get_by_id(&self.symbols, children_prop_symbol);
                let resolved_type;
                if child_types.len() == 1 {
                    resolved_type = child_types[0];
                } else if children_contextual_type.is_some()
                    && self.some_type(
                        children_contextual_type,
                        &mut |c: &mut Checker, t: TypeId| c.is_tuple_like_type(t),
                    )
                {
                    resolved_type = self.create_tuple_type(&child_types);
                } else {
                    let union = self.get_union_type(&child_types);
                    resolved_type = self.create_array_type(union);
                }
                self.value_symbol_links
                    .get_by_id(&self.symbols, children_prop_symbol)
                    .resolved_type = resolved_type;
                // Fake up a property declaration for the children
                let children_name = self
                    .factory
                    .new_identifier(jsx_children_property_name.clone());
                let value_declaration = self.factory.new_property_signature_declaration(
                    ModifierList::NIL,
                    children_name,
                    Node::NIL, /*postfixToken*/
                    Node::NIL, /*type*/
                    Node::NIL, /*initializer*/
                );
                self.sym_mut(children_prop_symbol).value_declaration = value_declaration;
                set_node_parent(value_declaration, attribute_parent);
                set_node_symbol(value_declaration, children_prop_symbol);
                let child_prop_map = self.symbols.new_table();
                self.symbols.set(
                    child_prop_map,
                    jsx_children_property_name.clone(),
                    children_prop_symbol,
                );
                let children_type =
                    self.new_anonymous_type(attributes_symbol, child_prop_map, &[], &[], &[]);
                let propagating =
                    self.get_propagating_flags_of_types(&child_types, TypeFlags::NONE);
                spread = self.get_spread_type(
                    spread,
                    children_type,
                    attributes_symbol,
                    object_flags | propagating,
                    false, /*readonly*/
                );
            }
        }
        if has_spread_any_type {
            return self.any_type;
        }
        if type_to_intersect.is_some() {
            if spread != self.empty_jsx_object_type {
                return self.get_intersection_type(&[type_to_intersect, spread]);
            }
            return type_to_intersect;
        }
        if spread == self.empty_jsx_object_type {
            return create_jsx_attributes_type(
                self,
                &mut object_flags,
                attributes_symbol,
                attributes_table,
            );
        }
        spread
    }

    // Go: checker/jsx.go:869 checkJsxAttribute
    pub fn check_jsx_attribute(&mut self, node: Node, check_mode: CheckMode) -> TypeId {
        if node.initializer().is_some() {
            return self.check_expression_for_mutable_location(node.initializer(), check_mode);
        }
        // <Elem attr /> is sugar for <Elem attr={true} />
        self.true_type
    }

    // Go: checker/jsx.go:877 checkJsxChildren
    pub fn check_jsx_children(&mut self, node: Node, check_mode: CheckMode) -> Vec<TypeId> {
        let mut child_types: Vec<TypeId> = Vec::new();
        for child in node.children().nodes() {
            // In React, JSX text that contains only whitespaces will be ignored so we don't want to type-check that
            // because then type of children property will have constituent of string type.
            if is_jsx_text(child) {
                if !child.contains_only_trivia_white_spaces() {
                    child_types.push(self.string_type);
                }
            } else if is_jsx_expression(child) && child.expression().is_nil() {
                // empty jsx expressions don't *really* count as present children
                continue;
            } else {
                let t = self.check_expression_for_mutable_location(child, check_mode);
                child_types.push(t);
            }
        }
        child_types
    }

    // Go: checker/jsx.go:896 getUninstantiatedJsxSignaturesOfType
    pub fn get_uninstantiated_jsx_signatures_of_type(
        &mut self,
        element_type: TypeId,
        caller: Node,
    ) -> Vec<SignatureId> {
        if self.ty(element_type).flags.intersects(TypeFlags::STRING) {
            return vec![self.any_signature];
        }
        if self
            .ty(element_type)
            .flags
            .intersects(TypeFlags::STRING_LITERAL)
        {
            let intrinsic_type =
                self.get_intrinsic_attributes_type_from_string_literal_type(element_type, caller);
            if intrinsic_type.is_nil() {
                let value = self.get_string_literal_value(element_type);
                self.error(
                    caller,
                    diag::Property_0_does_not_exist_on_type_1,
                    args![value, "JSX.".to_string() + JsxNames.intrinsic_elements],
                );
                return Vec::new();
            }
            let fake_signature = self.create_signature_for_jsx_intrinsic(caller, intrinsic_type);
            return vec![fake_signature];
        }
        let apparent_elem_type = self.get_apparent_type(element_type);
        // Resolve the signatures, preferring constructor
        let mut signatures =
            self.get_signatures_of_type(apparent_elem_type, SignatureKind::CONSTRUCT);
        if signatures.is_empty() {
            // No construct signatures, try call signatures
            signatures = self.get_signatures_of_type(apparent_elem_type, SignatureKind::CALL);
        }
        if signatures.is_empty()
            && self
                .ty(apparent_elem_type)
                .flags
                .intersects(TypeFlags::UNION)
        {
            // If each member has some combination of new/call signatures; make a union signature list for those
            let types = self.ty(apparent_elem_type).types_list();
            let lists: Vec<Vec<SignatureId>> = types
                .iter()
                .map(|&t| self.get_uninstantiated_jsx_signatures_of_type(t, caller))
                .collect();
            signatures = self.get_union_signatures(&lists).into();
        }
        signatures.to_vec()
    }
}
