use crate::prelude::*;

// Port of checker/jsx.go lines 929-1482.
//
// PORT: Go `JsxNames.X` fields are constant strings. This file uses the
// string values directly ("JSX", "IntrinsicElements", "ElementClass",
// "ElementAttributesProperty", "ElementChildrenAttribute", "Element",
// "ElementType", "IntrinsicAttributes", "IntrinsicClassAttributes",
// "LibraryManagedAttributes"), so it does not depend on how the first JSX
// file names the `JsxNames` struct.
//
// PORT: `JsxElementLinks` (defined with the first part of jsx.go) is assumed
// to have the Go fields in snake case: `jsx_flags: JsxFlags`,
// `resolved_jsx_element_attributes_type: TypeId`, `jsx_namespace: SymbolId`,
// `jsx_implicit_import_container: SymbolId`.

impl Checker {
    // Go: checker/jsx.go:925 getEffectiveFirstArgumentForJsxSignature
    pub fn get_effective_first_argument_for_jsx_signature(
        &mut self,
        signature: SignatureId,
        node: Node,
    ) -> TypeId {
        if is_jsx_opening_fragment(node)
            || self.get_jsx_reference_kind(node) != JsxReferenceKind::COMPONENT
        {
            return self.get_jsx_props_type_from_call_signature(signature, node);
        }
        self.get_jsx_props_type_from_class_type(signature, node)
    }

    // Go: checker/jsx.go:932 getJsxPropsTypeFromCallSignature
    pub fn get_jsx_props_type_from_call_signature(
        &mut self,
        sig: SignatureId,
        context: Node,
    ) -> TypeId {
        let unknown_type = self.unknown_type;
        let mut props_type =
            self.get_type_of_first_parameter_of_signature_with_fallback(sig, unknown_type);
        let ns = self.get_jsx_namespace_at(context);
        props_type =
            self.get_jsx_managed_attributes_from_located_attributes(context, ns, props_type);
        let intrinsic_attribs = self.get_jsx_type("IntrinsicAttributes", context);
        if !self.is_error_type(intrinsic_attribs) {
            props_type = self.intersect_types(intrinsic_attribs, props_type);
        }
        props_type
    }

    // Go: checker/jsx.go:942 getJsxPropsTypeFromClassType
    pub fn get_jsx_props_type_from_class_type(
        &mut self,
        sig: SignatureId,
        context: Node,
    ) -> TypeId {
        let ns = self.get_jsx_namespace_at(context);
        let forced_lookup_location = self.get_jsx_element_properties_name(ns);
        let mut attributes_type;
        match forced_lookup_location.as_str() {
            INTERNAL_SYMBOL_NAME_MISSING => {
                let unknown_type = self.unknown_type;
                attributes_type =
                    self.get_type_of_first_parameter_of_signature_with_fallback(sig, unknown_type);
            }
            "" => {
                attributes_type = self.get_return_type_of_signature(sig);
            }
            _ => {
                attributes_type =
                    self.get_jsx_props_type_for_signature_from_member(sig, &forced_lookup_location);
                if attributes_type.is_nil() && !context.attributes().properties().is_empty() {
                    // There is no property named 'props' on this instance type
                    self.error(
                        context,
                        diag::JSX_element_class_does_not_support_attributes_because_it_does_not_have_a_0_property,
                        args![forced_lookup_location],
                    );
                }
            }
        }
        if attributes_type.is_nil() {
            return self.unknown_type;
        }
        attributes_type =
            self.get_jsx_managed_attributes_from_located_attributes(context, ns, attributes_type);
        if self.is_type_any(attributes_type) {
            // Props is of type 'any' or unknown
            return attributes_type;
        }
        // Normal case -- add in IntrinsicClassAttributes<T> and IntrinsicAttributes
        let mut apparent_attributes_type = attributes_type;
        let intrinsic_class_attribs = self.get_jsx_type("IntrinsicClassAttributes", context);
        if !self.is_error_type(intrinsic_class_attribs) {
            let intrinsic_class_attribs_symbol = self.ty(intrinsic_class_attribs).symbol;
            let type_params = self.get_local_type_parameters_of_class_or_interface_or_type_alias(
                intrinsic_class_attribs_symbol,
            );
            let host_class_type = self.get_return_type_of_signature(sig);
            let library_managed_attribute_type;
            // PORT: Go `typeParams != nil`; the callee returns nil when it finds no type parameters.
            if !type_params.is_empty() {
                // apply JSX.IntrinsicClassAttributes<hostClassType, ...>
                let min_type_argument_count = self.get_min_type_argument_count(&type_params);
                let inferred_args = self.fill_missing_type_arguments(
                    &[host_class_type],
                    &type_params,
                    min_type_argument_count,
                    is_in_js_file(context),
                );
                let mapper = self.new_type_mapper(&type_params, &inferred_args);
                library_managed_attribute_type =
                    self.instantiate_type(intrinsic_class_attribs, mapper);
            } else {
                library_managed_attribute_type = intrinsic_class_attribs;
            }
            apparent_attributes_type =
                self.intersect_types(library_managed_attribute_type, apparent_attributes_type);
        }
        let intrinsic_attribs = self.get_jsx_type("IntrinsicAttributes", context);
        if !self.is_error_type(intrinsic_attribs) {
            apparent_attributes_type =
                self.intersect_types(intrinsic_attribs, apparent_attributes_type);
        }
        apparent_attributes_type
    }

    // Go: checker/jsx.go:989 getJsxPropsTypeForSignatureFromMember
    pub fn get_jsx_props_type_for_signature_from_member(
        &mut self,
        sig: SignatureId,
        forced_lookup_location: &str,
    ) -> TypeId {
        if let Some(composite) = self.sig(sig).composite.clone() {
            // JSX Elements using the legacy `props`-field based lookup (eg, react class components) need to treat the `props` member as an input
            // instead of an output position when resolving the signature. We need to go back to the input signatures of the composite signature,
            // get the type of `props` on each return type individually, and then _intersect them_, rather than union them (as would normally occur
            // for a union signature). It's an unfortunate quirk of looking in the output of the signature for the type we want to use for the input.
            // The default behavior of `getTypeOfFirstParameterOfSignatureWithFallback` when no `props` member name is defined is much more sane.
            let mut results: Vec<TypeId> = Vec::new();
            for &signature in &composite.signatures {
                let instance = self.get_return_type_of_signature(signature);
                if self.is_type_any(instance) {
                    return instance;
                }
                let prop_type = self.get_type_of_property_of_type(instance, forced_lookup_location);
                if prop_type.is_nil() {
                    return TypeId::NIL;
                }
                results.push(prop_type);
            }
            return self.get_intersection_type(&results);
            // Same result for both union and intersection signatures
        }
        let instance_type = self.get_return_type_of_signature(sig);
        if self.is_type_any(instance_type) {
            return instance_type;
        }
        self.get_type_of_property_of_type(instance_type, forced_lookup_location)
    }

    // Go: checker/jsx.go:1018 getJsxManagedAttributesFromLocatedAttributes
    pub fn get_jsx_managed_attributes_from_located_attributes(
        &mut self,
        context: Node,
        ns: SymbolId,
        attributes_type: TypeId,
    ) -> TypeId {
        let managed_sym = self.get_jsx_library_managed_attributes(ns);
        if managed_sym.is_some() {
            let ctor_type = self.get_static_type_of_referenced_jsx_constructor(context);
            let result = self.instantiate_alias_or_interface_with_defaults(
                managed_sym,
                &[ctor_type, attributes_type],
                is_in_js_file(context),
            );
            if result.is_some() {
                return result;
            }
        }
        attributes_type
    }

    // Go: checker/jsx.go:1030 instantiateAliasOrInterfaceWithDefaults
    pub fn instantiate_alias_or_interface_with_defaults(
        &mut self,
        managed_sym: SymbolId,
        type_arguments: &[TypeId],
        in_java_script: bool,
    ) -> TypeId {
        let declared_managed_type = self.get_declared_type_of_symbol(managed_sym);
        // fetches interface type, or initializes symbol links type parameters
        if self
            .sym(managed_sym)
            .flags
            .intersects(SymbolFlags::TYPE_ALIAS)
        {
            let params = self
                .type_alias_links
                .get(managed_sym)
                .type_parameters
                .clone();
            if params.len() >= type_arguments.len() {
                let args = self.fill_missing_type_arguments(
                    type_arguments,
                    &params,
                    type_arguments.len() as i32,
                    in_java_script,
                );
                if args.is_empty() {
                    return declared_managed_type;
                }
                return self.get_type_alias_instantiation(managed_sym, &args, None);
            }
        }
        if self
            .ty(declared_managed_type)
            .object_flags
            .intersects(ObjectFlags::CLASS_OR_INTERFACE)
            && self
                .ty(declared_managed_type)
                .as_interface_type()
                .type_parameters()
                .len()
                >= type_arguments.len()
        {
            let type_parameters = self
                .ty(declared_managed_type)
                .as_interface_type()
                .type_parameters()
                .to_vec();
            let args = self.fill_missing_type_arguments(
                type_arguments,
                &type_parameters,
                type_arguments.len() as i32,
                in_java_script,
            );
            return self.create_type_reference(declared_managed_type, &args);
        }
        TypeId::NIL
    }

    // Go: checker/jsx.go:1050 getJsxLibraryManagedAttributes
    pub fn get_jsx_library_managed_attributes(&mut self, jsx_namespace: SymbolId) -> SymbolId {
        if jsx_namespace.is_some() {
            let exports = self.sym(jsx_namespace).exports;
            return self.get_symbol(exports, "LibraryManagedAttributes", SymbolFlags::TYPE);
        }
        SymbolId::NIL
    }

    // Go: checker/jsx.go:1057 getJsxElementTypeSymbol
    pub fn get_jsx_element_type_symbol(&mut self, jsx_namespace: SymbolId) -> SymbolId {
        // JSX.ElementType [symbol]
        if jsx_namespace.is_some() {
            let exports = self.sym(jsx_namespace).exports;
            return self.get_symbol(exports, "ElementType", SymbolFlags::TYPE);
        }
        SymbolId::NIL
    }

    // e.g. "props" for React.d.ts,
    // or InternalSymbolNameMissing if ElementAttributesProperty doesn't exist (which means all
    //
    //	non-intrinsic elements' attributes type is 'any'),
    //
    // or "" if it has 0 properties (which means every
    //
    //	non-intrinsic elements' attributes type is the element instance type)
    // Go: checker/jsx.go:1073 getJsxElementPropertiesName
    pub fn get_jsx_element_properties_name(&mut self, jsx_namespace: SymbolId) -> String {
        self.get_name_from_jsx_element_attributes_container(
            "ElementAttributesProperty",
            jsx_namespace,
        )
    }

    // Go: checker/jsx.go:1077 getJsxElementChildrenPropertyName
    pub fn get_jsx_element_children_property_name(&mut self, jsx_namespace: SymbolId) -> String {
        if self.compiler_options.jsx == JsxEmit::REACT_JSX
            || self.compiler_options.jsx == JsxEmit::REACT_JSX_DEV
        {
            // In these JsxEmit modes the children property is fixed to 'children'
            return "children".to_string();
        }
        self.get_name_from_jsx_element_attributes_container(
            "ElementChildrenAttribute",
            jsx_namespace,
        )
    }

    // Look into JSX namespace and then look for container with matching name as nameOfAttribPropContainer.
    // Get a single property from that container if existed. Report an error if there are more than one property.
    //
    // @param nameOfAttribPropContainer a string of value JsxNames.ElementAttributesPropertyNameContainer or JsxNames.ElementChildrenAttributeNameContainer
    //
    //	if other string is given or the container doesn't exist, return undefined.
    // Go: checker/jsx.go:1091 getNameFromJsxElementAttributesContainer
    pub fn get_name_from_jsx_element_attributes_container(
        &mut self,
        name_of_attrib_prop_container: &str,
        jsx_namespace: SymbolId,
    ) -> String {
        // JSX.ElementAttributesProperty | JSX.ElementChildrenAttribute [symbol]
        if jsx_namespace.is_some() {
            let exports = self.sym(jsx_namespace).exports;
            let jsx_element_attrib_prop_interface_sym =
                self.get_symbol(exports, name_of_attrib_prop_container, SymbolFlags::TYPE);
            if jsx_element_attrib_prop_interface_sym.is_some() {
                let jsx_element_attrib_prop_interface_type =
                    self.get_declared_type_of_symbol(jsx_element_attrib_prop_interface_sym);
                let properties_of_jsx_element_attrib_prop_interface =
                    self.get_properties_of_type(jsx_element_attrib_prop_interface_type);
                // Element Attributes has zero properties, so the element attributes type will be the class instance type
                if properties_of_jsx_element_attrib_prop_interface.is_empty() {
                    return String::new();
                }
                if properties_of_jsx_element_attrib_prop_interface.len() == 1 {
                    return self
                        .sym(properties_of_jsx_element_attrib_prop_interface[0])
                        .name
                        .to_string();
                }
                if properties_of_jsx_element_attrib_prop_interface.len() > 1
                    && !self
                        .sym(jsx_element_attrib_prop_interface_sym)
                        .declarations
                        .is_empty()
                {
                    // More than one property on ElementAttributesProperty is an error
                    let declaration =
                        self.sym(jsx_element_attrib_prop_interface_sym).declarations[0];
                    self.error(
                        declaration,
                        diag::The_global_type_JSX_0_may_not_have_more_than_one_property,
                        args![name_of_attrib_prop_container],
                    );
                }
            }
        }
        INTERNAL_SYMBOL_NAME_MISSING.to_string()
    }

    // Go: checker/jsx.go:1114 getStaticTypeOfReferencedJsxConstructor
    pub fn get_static_type_of_referenced_jsx_constructor(&mut self, context: Node) -> TypeId {
        if is_jsx_opening_fragment(context) {
            return self.get_jsx_fragment_type(context);
        }
        if is_jsx_intrinsic_tag_name(context.tag_name()) {
            let result = self.get_intrinsic_attributes_type_from_jsx_opening_like_element(context);
            let fake_signature = self.create_signature_for_jsx_intrinsic(context, result);
            return self.get_or_create_type_from_signature(fake_signature);
        }
        let tag_type = self.check_expression_cached(context.tag_name());
        if self
            .ty(tag_type)
            .flags
            .intersects(TypeFlags::STRING_LITERAL)
        {
            let result =
                self.get_intrinsic_attributes_type_from_string_literal_type(tag_type, context);
            if result.is_nil() {
                return self.error_type;
            }
            let fake_signature = self.create_signature_for_jsx_intrinsic(context, result);
            return self.get_or_create_type_from_signature(fake_signature);
        }
        tag_type
    }

    // Go: checker/jsx.go:1135 getIntrinsicAttributesTypeFromStringLiteralType
    pub fn get_intrinsic_attributes_type_from_string_literal_type(
        &mut self,
        t: TypeId,
        location: Node,
    ) -> TypeId {
        // If the elemType is a stringLiteral type, we can then provide a check to make sure that the string literal type is one of the Jsx intrinsic element type
        // For example:
        //      var CustomTag: "h1" = "h1";
        //      <CustomTag> Hello World </CustomTag>
        let intrinsic_elements_type = self.get_jsx_type("IntrinsicElements", location);
        if !self.is_error_type(intrinsic_elements_type) {
            let string_literal_type_name = self.get_string_literal_value(t);
            let intrinsic_prop =
                self.get_property_of_type(intrinsic_elements_type, &string_literal_type_name);
            if intrinsic_prop.is_some() {
                return self.get_type_of_symbol(intrinsic_prop);
            }
            let string_type = self.string_type;
            let index_signature_type =
                self.get_index_type_of_type(intrinsic_elements_type, string_type);
            if index_signature_type.is_some() {
                return index_signature_type;
            }
            return TypeId::NIL;
        }
        // If we need to report an error, we already done so here. So just return any to prevent any more error downstream
        self.any_type
    }

    // Go: checker/jsx.go:1157 getJsxReferenceKind
    pub fn get_jsx_reference_kind(&mut self, node: Node) -> JsxReferenceKind {
        if is_jsx_intrinsic_tag_name(node.tag_name()) {
            return JsxReferenceKind::MIXED;
        }
        let tag_expr_type = self.check_expression(node.tag_name());
        let tag_type = self.get_apparent_type(tag_expr_type);
        if !self
            .get_signatures_of_type(tag_type, SignatureKind::CONSTRUCT)
            .is_empty()
        {
            return JsxReferenceKind::COMPONENT;
        }
        if !self
            .get_signatures_of_type(tag_type, SignatureKind::CALL)
            .is_empty()
        {
            return JsxReferenceKind::FUNCTION;
        }
        JsxReferenceKind::MIXED
    }

    // Go: checker/jsx.go:1171 createSignatureForJSXIntrinsic
    pub fn create_signature_for_jsx_intrinsic(
        &mut self,
        node: Node,
        result: TypeId,
    ) -> SignatureId {
        let mut element_type = self.error_type;
        let namespace = self.get_jsx_namespace_at(node);
        if namespace.is_some() {
            let exports = self.get_exports_of_symbol(namespace);
            let type_symbol = self.get_symbol(exports, "Element", SymbolFlags::TYPE);
            if type_symbol.is_some() {
                element_type = self.get_declared_type_of_symbol(type_symbol);
            }
        }
        // returnNode := typeSymbol && c.nodeBuilder.symbolToEntityName(typeSymbol, ast.SymbolFlagsType, node)
        // declaration := factory.createFunctionTypeNode(nil, []ParameterDeclaration{factory.createParameterDeclaration(nil, nil /*dotDotDotToken*/, "props", nil /*questionToken*/, c.nodeBuilder.typeToTypeNode(result, node))}, ifElse(returnNode != nil, factory.createTypeReferenceNode(returnNode, nil /*typeArguments*/), factory.createKeywordTypeNode(ast.KindAnyKeyword)))
        let parameter_symbol = self.new_symbol(SymbolFlags::FUNCTION_SCOPED_VARIABLE, "props");
        self.value_symbol_links
            .get_by_id(&self.symbols, parameter_symbol)
            .resolved_type = result;
        self.new_signature(
            SignatureFlags::NONE,
            Node::NIL,
            &[],
            SymbolId::NIL,
            &[parameter_symbol],
            element_type,
            TypePredicateId::NIL,
            1,
        )
    }

    // Get attributes type of the given intrinsic opening-like Jsx element by resolving the tag name.
    // The function is intended to be called from a function which has checked that the opening element is an intrinsic element.
    // @param node an intrinsic JSX opening-like element
    // Go: checker/jsx.go:1188 getIntrinsicAttributesTypeFromJsxOpeningLikeElement
    pub fn get_intrinsic_attributes_type_from_jsx_opening_like_element(
        &mut self,
        node: Node,
    ) -> TypeId {
        debug_assert!(is_jsx_intrinsic_tag_name(node.tag_name()));
        let resolved = self
            .jsx_element_links
            .get(node)
            .resolved_jsx_element_attributes_type;
        if resolved.is_some() {
            return resolved;
        }
        let symbol = self.get_intrinsic_tag_symbol(node);
        // PORT: Go holds the links pointer across calls; re-fetch after each call.
        let jsx_flags = self.jsx_element_links.get(node).jsx_flags;
        if jsx_flags.intersects(JsxFlags::INTRINSIC_NAMED_ELEMENT) {
            let mut t = self.get_type_of_symbol(symbol);
            if t.is_nil() {
                t = self.error_type;
            }
            self.jsx_element_links
                .get(node)
                .resolved_jsx_element_attributes_type = t;
            return t;
        }
        if jsx_flags.intersects(JsxFlags::INTRINSIC_INDEXED_ELEMENT) {
            let intrinsic_elements_type = self.get_jsx_type("IntrinsicElements", node);
            let index_info = self.get_applicable_index_info_for_name(
                intrinsic_elements_type,
                node.tag_name().text(),
            );
            if index_info.is_some() {
                let value_type = self.index_info(index_info).value_type;
                self.jsx_element_links
                    .get(node)
                    .resolved_jsx_element_attributes_type = value_type;
                return value_type;
            }
        }
        let error_type = self.error_type;
        self.jsx_element_links
            .get(node)
            .resolved_jsx_element_attributes_type = error_type;
        error_type
    }

    // Looks up an intrinsic tag name and returns a symbol that either points to an intrinsic
    // property (in which case nodeLinks.jsxFlags will be IntrinsicNamedElement) or an intrinsic
    // string index signature (in which case nodeLinks.jsxFlags will be IntrinsicIndexedElement).
    // May also return unknownSymbol if both of these lookups fail.
    // Go: checker/jsx.go:1214 getIntrinsicTagSymbol
    pub fn get_intrinsic_tag_symbol(&mut self, node: Node) -> SymbolId {
        let resolved_symbol = self.symbol_node_links.get(node).resolved_symbol;
        if resolved_symbol.is_some() {
            return resolved_symbol;
        }
        let intrinsic_elements_type = self.get_jsx_type("IntrinsicElements", node);
        if !self.is_error_type(intrinsic_elements_type) {
            // Property case
            let tag_name = node.tag_name();
            if !is_identifier(tag_name) && !is_jsx_namespaced_name(tag_name) {
                panic!("Invalid tag name");
            }
            let prop_name = tag_name.text();
            let intrinsic_prop = self.get_property_of_type(intrinsic_elements_type, prop_name);
            if intrinsic_prop.is_some() {
                self.jsx_element_links.get(node).jsx_flags |= JsxFlags::INTRINSIC_NAMED_ELEMENT;
                self.symbol_node_links.get(node).resolved_symbol = intrinsic_prop;
                return intrinsic_prop;
            }
            // Intrinsic string indexer case
            let prop_name_type = self.get_string_literal_type(prop_name);
            let index_symbol =
                self.get_applicable_index_symbol(intrinsic_elements_type, prop_name_type);
            if index_symbol.is_some() {
                self.jsx_element_links.get(node).jsx_flags |= JsxFlags::INTRINSIC_INDEXED_ELEMENT;
                self.symbol_node_links.get(node).resolved_symbol = index_symbol;
                return index_symbol;
            }
            if self
                .get_type_of_property_or_index_signature_of_type(intrinsic_elements_type, prop_name)
                .is_some()
            {
                self.jsx_element_links.get(node).jsx_flags |= JsxFlags::INTRINSIC_INDEXED_ELEMENT;
                let s = self.ty(intrinsic_elements_type).symbol;
                self.symbol_node_links.get(node).resolved_symbol = s;
                return s;
            }
            // Wasn't found
            self.error(
                node,
                diag::Property_0_does_not_exist_on_type_1,
                args![tag_name.text(), "JSX.IntrinsicElements"],
            );
            let unknown_symbol = self.unknown_symbol;
            self.symbol_node_links.get(node).resolved_symbol = unknown_symbol;
            return unknown_symbol;
        }
        if self.no_implicit_any {
            self.error(
                node,
                diag::JSX_element_implicitly_has_type_any_because_no_interface_JSX_0_exists,
                args!["IntrinsicElements"],
            );
        }
        let unknown_symbol = self.unknown_symbol;
        self.symbol_node_links.get(node).resolved_symbol = unknown_symbol;
        unknown_symbol
    }

    // Go: checker/jsx.go:1257 getJsxStatelessElementTypeAt
    pub fn get_jsx_stateless_element_type_at(&mut self, location: Node) -> TypeId {
        let jsx_element_type = self.get_jsx_element_type_at(location);
        if jsx_element_type.is_nil() {
            return TypeId::NIL;
        }
        let null_type = self.null_type;
        self.get_union_type(&[jsx_element_type, null_type])
    }

    // Go: checker/jsx.go:1265 getJsxElementClassTypeAt
    pub fn get_jsx_element_class_type_at(&mut self, location: Node) -> TypeId {
        let t = self.get_jsx_type("ElementClass", location);
        if self.is_error_type(t) {
            return TypeId::NIL;
        }
        t
    }

    // Go: checker/jsx.go:1273 getJsxElementTypeAt
    pub fn get_jsx_element_type_at(&mut self, location: Node) -> TypeId {
        self.get_jsx_type("Element", location)
    }

    // Go: checker/jsx.go:1277 getJsxElementTypeTypeAt
    pub fn get_jsx_element_type_type_at(&mut self, location: Node) -> TypeId {
        let ns = self.get_jsx_namespace_at(location);
        if ns.is_nil() {
            return TypeId::NIL;
        }
        let sym = self.get_jsx_element_type_symbol(ns);
        if sym.is_nil() {
            return TypeId::NIL;
        }
        let t =
            self.instantiate_alias_or_interface_with_defaults(sym, &[], is_in_js_file(location));
        if t.is_nil() || self.is_error_type(t) {
            return TypeId::NIL;
        }
        t
    }

    // Go: checker/jsx.go:1293 getJsxType
    pub fn get_jsx_type(&mut self, name: &str, location: Node) -> TypeId {
        let namespace = self.get_jsx_namespace_at(location);
        if namespace.is_some() {
            let exports = self.get_exports_of_symbol(namespace);
            // PORT: Go `exports != nil` (a nil map); a nil table is `SymbolTable::NIL`.
            if exports.is_some() {
                let type_symbol = self.get_symbol(exports, name, SymbolFlags::TYPE);
                if type_symbol.is_some() {
                    return self.get_declared_type_of_symbol(type_symbol);
                }
            }
        }
        self.error_type
    }

    // Go: checker/jsx.go:1304 getJsxNamespaceAt
    pub fn get_jsx_namespace_at(&mut self, location: Node) -> SymbolId {
        // PORT: Go `links` is nil only when `location` is nil. Links are
        // re-fetched from the store instead of holding a pointer.
        let has_links = location.is_some();
        let unknown_symbol = self.unknown_symbol;
        let links_jsx_namespace = if has_links {
            self.jsx_element_links.get(location).jsx_namespace
        } else {
            SymbolId::NIL
        };
        if has_links && links_jsx_namespace.is_some() && links_jsx_namespace != unknown_symbol {
            return links_jsx_namespace;
        }
        if !has_links || links_jsx_namespace != unknown_symbol {
            let mut resolved_namespace =
                self.get_jsx_namespace_container_for_implicit_import(location);
            if resolved_namespace.is_nil() || resolved_namespace == unknown_symbol {
                let namespace_name = self.get_jsx_namespace(location);
                let resolve_name = self.resolve_name.clone();
                resolved_namespace = resolve_name(
                    self,
                    location,
                    &namespace_name,
                    SymbolFlags::NAMESPACE,
                    None,  /*nameNotFoundMessage*/
                    false, /*isUse*/
                    false, /*excludeGlobals*/
                );
            }
            if resolved_namespace.is_some() {
                let resolved = self.resolve_symbol(resolved_namespace);
                let exports = self.get_exports_of_symbol(resolved);
                let jsx_symbol = self.get_symbol(exports, "JSX", SymbolFlags::NAMESPACE);
                let candidate = self.resolve_symbol(jsx_symbol);
                if candidate.is_some() && candidate != unknown_symbol {
                    if has_links {
                        self.jsx_element_links.get(location).jsx_namespace = candidate;
                    }
                    return candidate;
                }
            }
            if has_links {
                self.jsx_element_links.get(location).jsx_namespace = unknown_symbol;
            }
        }
        // JSX global fallback
        let global =
            self.get_global_symbol("JSX", SymbolFlags::NAMESPACE, None /*diagnostic*/);
        let s = self.resolve_symbol(global);
        if s == unknown_symbol {
            return SymbolId::NIL;
        }
        s
    }

    // Go: checker/jsx.go:1339 getJsxNamespace
    pub fn get_jsx_namespace(&mut self, location: Node) -> String {
        if location.is_some() {
            let file = get_source_file_of_node(location);
            if file.is_some() {
                if is_jsx_opening_fragment(location) {
                    let local_jsx_fragment_namespace = self
                        .source_file_links
                        .get(file)
                        .local_jsx_fragment_namespace
                        .clone();
                    if !local_jsx_fragment_namespace.is_empty() {
                        return local_jsx_fragment_namespace;
                    }
                    let jsx_fragment_pragma = get_pragma_from_source_file(file, "jsxfrag");
                    if jsx_fragment_pragma.is_some() {
                        // PORT: Go `pragma.Args["factory"].Value` (zero value when absent).
                        let factory =
                            get_pragma_argument(jsx_fragment_pragma.as_deref(), "factory");
                        let local_jsx_fragment_factory = self.parse_isolated_entity_name(&factory);
                        self.source_file_links.get(file).local_jsx_fragment_factory =
                            local_jsx_fragment_factory;
                        if local_jsx_fragment_factory.is_some() {
                            let namespace = get_first_identifier(local_jsx_fragment_factory)
                                .text()
                                .to_string();
                            self.source_file_links
                                .get(file)
                                .local_jsx_fragment_namespace = namespace.clone();
                            return namespace;
                        }
                    }
                    let entity = self.get_jsx_fragment_factory_entity(location);
                    if entity.is_some() {
                        let namespace = get_first_identifier(entity).text().to_string();
                        let links = self.source_file_links.get(file);
                        links.local_jsx_fragment_factory = entity;
                        links.local_jsx_fragment_namespace = namespace.clone();
                        return namespace;
                    }
                } else {
                    let local_jsx_namespace = self.get_local_jsx_namespace(file);
                    if !local_jsx_namespace.is_empty() {
                        self.source_file_links.get(file).local_jsx_namespace =
                            local_jsx_namespace.clone();
                        return local_jsx_namespace;
                    }
                }
            }
        }
        if self._jsx_namespace.is_empty() {
            self._jsx_namespace = "React".to_string();
            if !self.compiler_options.jsx_factory.is_empty() {
                let jsx_factory = self.compiler_options.jsx_factory.clone();
                self._jsx_factory_entity = self.parse_isolated_entity_name(&jsx_factory);
                if self._jsx_factory_entity.is_some() {
                    self._jsx_namespace = get_first_identifier(self._jsx_factory_entity)
                        .text()
                        .to_string();
                }
            } else if !self.compiler_options.react_namespace.is_empty() {
                self._jsx_namespace = self.compiler_options.react_namespace.clone();
            }
        }
        if self._jsx_factory_entity.is_nil() {
            let left = self.factory.new_identifier(self._jsx_namespace.clone());
            let right = self.factory.new_identifier("createElement");
            self._jsx_factory_entity = self.factory.new_qualified_name(left, right);
        }
        self._jsx_namespace.clone()
    }

    // Go: checker/jsx.go:1388 getLocalJsxNamespace
    pub fn get_local_jsx_namespace(&mut self, file: Node) -> String {
        let local_jsx_namespace = self.source_file_links.get(file).local_jsx_namespace.clone();
        if !local_jsx_namespace.is_empty() {
            return local_jsx_namespace;
        }
        let jsx_pragma = get_pragma_from_source_file(file, "jsx");
        if jsx_pragma.is_some() {
            // PORT: Go `pragma.Args["factory"].Value` (zero value when absent).
            let factory = get_pragma_argument(jsx_pragma.as_deref(), "factory");
            let local_jsx_factory = self.parse_isolated_entity_name(&factory);
            self.source_file_links.get(file).local_jsx_factory = local_jsx_factory;
            if local_jsx_factory.is_some() {
                let namespace = get_first_identifier(local_jsx_factory).text().to_string();
                self.source_file_links.get(file).local_jsx_namespace = namespace.clone();
                return namespace;
            }
        }
        String::new()
    }

    // Go: checker/jsx.go:1404 getJsxFactoryEntity
    pub fn get_jsx_factory_entity(&mut self, location: Node) -> Node {
        if location.is_some() {
            self.get_jsx_namespace(location);
            let local_jsx_factory = self
                .source_file_links
                .get(get_source_file_of_node(location))
                .local_jsx_factory;
            if local_jsx_factory.is_some() {
                return local_jsx_factory;
            }
        }
        self._jsx_factory_entity
    }

    // Go: checker/jsx.go:1414 getJsxFragmentFactoryEntity
    pub fn get_jsx_fragment_factory_entity(&mut self, location: Node) -> Node {
        if location.is_some() {
            let file = get_source_file_of_node(location);
            if file.is_some() {
                let local_jsx_fragment_factory =
                    self.source_file_links.get(file).local_jsx_fragment_factory;
                if local_jsx_fragment_factory.is_some() {
                    return local_jsx_fragment_factory;
                }
                let jsx_frag_pragma = get_pragma_from_source_file(file, "jsxfrag");
                if jsx_frag_pragma.is_some() {
                    // PORT: Go `pragma.Args["factory"].Value` (zero value when absent).
                    let factory = get_pragma_argument(jsx_frag_pragma.as_deref(), "factory");
                    let local_jsx_fragment_factory = self.parse_isolated_entity_name(&factory);
                    self.source_file_links.get(file).local_jsx_fragment_factory =
                        local_jsx_fragment_factory;
                    return local_jsx_fragment_factory;
                }
            }
        }
        if !self.compiler_options.jsx_fragment_factory.is_empty() {
            let jsx_fragment_factory = self.compiler_options.jsx_fragment_factory.clone();
            return self.parse_isolated_entity_name(&jsx_fragment_factory);
        }
        Node::NIL
    }

    // Go: checker/jsx.go:1435 parseIsolatedEntityName
    pub fn parse_isolated_entity_name(&mut self, name: &str) -> Node {
        let result = crate::frontend::parser::parse_isolated_entity_name(name);
        if result.is_some() {
            mark_as_synthetic(result);
        }
        result
    }

    // Go: checker/jsx.go:1449 getJsxNamespaceContainerForImplicitImport
    pub fn get_jsx_namespace_container_for_implicit_import(&mut self, location: Node) -> SymbolId {
        let file = get_source_file_of_node(location);
        let container = self
            .jsx_element_links
            .get(file)
            .jsx_implicit_import_container;
        if container.is_some() {
            return if container == self.unknown_symbol {
                SymbolId::NIL
            } else {
                container
            };
        }
        let mut canonical_error_tag = self.jsx_element_links.get(file).first_jsx_tag_in_file;
        if canonical_error_tag.is_nil() {
            fn visit(node: Node, first_jsx_tag_in_file: &mut Node) -> bool {
                if is_jsx_element(node) || is_jsx_self_closing_element(node) {
                    *first_jsx_tag_in_file = node;
                    return true;
                }
                if is_jsx_fragment(node) {
                    *first_jsx_tag_in_file = node.opening_fragment(); // to match strada, fragments issue errors on the opening fragment instead of the whole tag
                    return true;
                }
                node.for_each_child(|child| visit(child, first_jsx_tag_in_file))
            }
            let mut first_jsx_tag_in_file = Node::NIL;
            file.for_each_child(|child| visit(child, &mut first_jsx_tag_in_file));
            self.jsx_element_links.get(file).first_jsx_tag_in_file = first_jsx_tag_in_file;
            canonical_error_tag = first_jsx_tag_in_file;
        }
        let (module_reference, specifier) = self.get_jsx_runtime_import_specifier(file);
        if module_reference.is_empty() {
            return SymbolId::NIL;
        }
        let error_message = diag::This_JSX_tag_requires_the_module_path_0_to_exist_but_none_could_be_found_Make_sure_you_have_types_for_the_appropriate_package_installed;
        let module_location = if specifier.is_some() {
            specifier
        } else {
            canonical_error_tag
        };
        let mod_ = self.resolve_external_module(
            module_location,
            &module_reference,
            Some(error_message),
            canonical_error_tag,
            false,
            TypeId::NIL, /*importAttributesType*/
        );
        let mut result = SymbolId::NIL;
        if mod_.is_some() && mod_ != self.unknown_symbol {
            let resolved = self.resolve_symbol(mod_);
            result = self.get_merged_symbol(resolved);
        }
        let container = if result.is_some() {
            result
        } else {
            self.unknown_symbol
        };
        self.jsx_element_links
            .get(file)
            .jsx_implicit_import_container = container;
        result
    }

    // Go: checker/jsx.go:1486 getJSXRuntimeImportSpecifier
    pub fn get_jsx_runtime_import_specifier(&mut self, file: Node) -> (String, Node) {
        // PORT: Go `c.program.GetJSXRuntimeImportSpecifier(file.Path())` is the
        // program.rs free function of the same snake name.
        crate::program::get_jsx_runtime_import_specifier(&source_file_info(file).path)
    }
}

// Go: checker/jsx.go:1443 markAsSynthetic
// PORT: the isolated parse has no file store, so its nodes are factory
// nodes and `set_node_loc` can write them.
pub fn mark_as_synthetic(node: Node) -> bool {
    set_node_loc(node, TextRange::new(-1, -1));
    node.for_each_child(mark_as_synthetic);
    false
}
