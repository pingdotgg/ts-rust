//! Go: checker/checker.go:17116-18041 (checkDeclarationInitializer ..
//! getTypeForBindingElementParent), including the cache key builder
//! (`CacheHashKey`, `keyBuilder` and the `get*Key` functions).

use crate::prelude::*;

use std::hash::Hasher as _;

impl Checker {
    // Go: checker/checker.go:17116 checkDeclarationInitializer
    pub fn check_declaration_initializer(
        &mut self,
        declaration: Node,
        check_mode: CheckMode,
        contextual_type: TypeId,
    ) -> TypeId {
        let initializer = declaration.initializer();
        let mut t = self.get_quick_type_of_expression(initializer);
        if t.is_nil() {
            if contextual_type.is_some() {
                t = self.check_expression_with_contextual_type(
                    initializer,
                    contextual_type,
                    InferenceContextId::NIL, /*inferenceContext*/
                    check_mode,
                );
            } else {
                t = self.check_expression_cached_ex(initializer, check_mode);
            }
        }
        if is_parameter_declaration(get_root_declaration(declaration)) {
            let name = declaration.name();
            match name.kind() {
                SyntaxKind::ObjectBindingPattern => {
                    if self.is_object_literal_type(t) {
                        return self.pad_object_literal_type(t, name);
                    }
                }
                SyntaxKind::ArrayBindingPattern => {
                    if self.is_tuple_type(t) {
                        return self.pad_tuple_type(t, name);
                    }
                }
                _ => {}
            }
        }
        t
    }

    // Go: checker/checker.go:17142 padObjectLiteralType
    pub fn pad_object_literal_type(&mut self, t: TypeId, pattern: Node) -> TypeId {
        let mut missing_elements: Vec<Node> = Vec::new();
        for e in pattern.elements().iter() {
            if has_dot_dot_dot_token(e) {
                continue;
            }
            let name = self.get_property_name_from_binding_element(e);
            if name != INTERNAL_SYMBOL_NAME_MISSING && self.get_property_of_type(t, &name).is_nil()
            {
                missing_elements.push(e);
            }
        }
        if missing_elements.is_empty() {
            return t;
        }
        let members = self.symbols.new_table();
        let props = self.get_properties_of_object_type(t).to_vec();
        for prop in props {
            let prop_name = self.sym(prop).name.clone();
            self.symbols.set(members, prop_name, prop);
        }
        for e in missing_elements {
            let name = self.get_property_name_from_binding_element(e);
            let symbol = self.new_symbol(SymbolFlags::PROPERTY | SymbolFlags::OPTIONAL, &name);
            // Go reads the links (and gives the id) before the right side.
            self.value_symbol_links.get_by_id(&self.symbols, symbol);
            let resolved_type = self.get_type_from_binding_element(
                e, false, /*includePatternInType*/
                true,  /*reportErrors*/
            );
            self.value_symbol_links
                .get_by_id(&self.symbols, symbol)
                .resolved_type = resolved_type;
            let symbol_name = self.sym(symbol).name.clone();
            self.symbols.set(members, symbol_name, symbol);
        }
        let t_symbol = self.ty(t).symbol;
        let index_infos = self.get_index_infos_of_type(t).to_vec();
        let result = self.new_anonymous_type(t_symbol, members, &[], &[], &index_infos);
        let object_flags = self.ty(t).object_flags;
        self.ty_mut(result).object_flags = object_flags;
        result
    }

    // Go: checker/checker.go:17170 getPropertyNameFromBindingElement
    pub fn get_property_name_from_binding_element(&mut self, e: Node) -> String {
        let expr_type = self.get_literal_type_from_property_name(e.property_name_or_name());
        if self.is_type_usable_as_property_name(expr_type) {
            return self.get_property_name_from_type(expr_type);
        }
        INTERNAL_SYMBOL_NAME_MISSING.to_string()
    }

    // Go: checker/checker.go:17178 padTupleType
    pub fn pad_tuple_type(&mut self, t: TypeId, pattern: Node) -> TypeId {
        let pattern_elements = pattern.elements();
        if self
            .target_tuple_type(t)
            .combined_flags
            .intersects(ElementFlags::VARIABLE)
            || self.get_type_reference_arity(t) as usize >= pattern_elements.len()
        {
            return t;
        }
        let mut element_types: Vec<TypeId> = self.get_element_types(t).to_vec();
        let mut element_infos: Vec<TupleElementInfo> =
            self.target_tuple_type(t).element_infos.clone();
        let mut i = self.get_type_reference_arity(t) as usize;
        while i < pattern_elements.len() {
            let e = pattern_elements.get(i);
            if i < pattern_elements.len() - 1
                || !(is_binding_element(e) && has_dot_dot_dot_token(e))
            {
                let mut element_type = self.any_type;
                if !is_omitted_expression(e) && self.has_default_value(e) {
                    element_type = self.get_type_from_binding_element(
                        e, false, /*includePatternInType*/
                        false, /*reportErrors*/
                    );
                }
                element_types.push(element_type);
                element_infos.push(TupleElementInfo {
                    flags: ElementFlags::OPTIONAL,
                    labeled_declaration: Node::NIL,
                });
                if !is_omitted_expression(e) && !self.has_default_value(e) {
                    let any_type = self.any_type;
                    self.report_implicit_any(e, any_type, WideningKind::NORMAL);
                }
            }
            i += 1;
        }
        let readonly = self.target_tuple_type(t).readonly;
        self.create_tuple_type_ex(&element_types, &element_infos, readonly)
    }

    // Go: checker/checker.go:17202 widenTypeInferredFromInitializer
    pub fn widen_type_inferred_from_initializer(&mut self, declaration: Node, t: TypeId) -> TypeId {
        let widened = self.get_widened_literal_type_for_initializer(declaration, t);
        if is_in_js_file(declaration) {
            if self.is_empty_literal_type(widened) {
                let any_type = self.any_type;
                self.report_implicit_any(declaration, any_type, WideningKind::NORMAL);
                return self.any_type;
            }
            if self.is_empty_array_literal_type(widened) {
                let any_array_type = self.any_array_type;
                self.report_implicit_any(declaration, any_array_type, WideningKind::NORMAL);
                return self.any_array_type;
            }
        }
        widened
    }

    // Go: checker/checker.go:17217 getWidenedLiteralTypeForInitializer
    pub fn get_widened_literal_type_for_initializer(
        &mut self,
        declaration: Node,
        t: TypeId,
    ) -> TypeId {
        if self
            .get_combined_node_flags_cached(declaration)
            .intersects(NodeFlags::CONSTANT)
            || is_declaration_readonly(declaration)
        {
            return t;
        }
        self.get_widened_literal_type(t)
    }

    // Go: checker/checker.go:17224 getTypeOfFuncClassEnumModule
    pub fn get_type_of_func_class_enum_module(&mut self, symbol: SymbolId) -> TypeId {
        if self
            .value_symbol_links
            .get_by_id(&self.symbols, symbol)
            .resolved_type
            .is_nil()
        {
            let t = self.get_type_of_func_class_enum_module_worker(symbol);
            // PORT: Go assigns through the links pointer taken before the
            // worker call; re-fetch the record here.
            self.value_symbol_links
                .get_by_id(&self.symbols, symbol)
                .resolved_type = t;
        }
        self.value_symbol_links
            .get_by_id(&self.symbols, symbol)
            .resolved_type
    }

    // Go: checker/checker.go:17232 getTypeOfFuncClassEnumModuleWorker
    pub fn get_type_of_func_class_enum_module_worker(&mut self, symbol: SymbolId) -> TypeId {
        let flags = self.sym(symbol).flags;
        let value_declaration = self.sym(symbol).value_declaration;
        if flags.intersects(SymbolFlags::MODULE) && self.is_shorthand_ambient_module_symbol(symbol)
        {
            return self.any_type;
        } else if flags.intersects(SymbolFlags::VALUE_MODULE)
            && value_declaration.is_some()
            && is_source_file(value_declaration)
            && with_source_file_info(value_declaration, |info| info.common_js_module_indicator)
                .is_some()
        {
            let resolved_module =
                self.resolve_external_module_symbol(symbol, false /*dontResolveAlias*/);
            if resolved_module != symbol {
                return self.get_type_of_symbol(resolved_module);
            }
        }
        let t = self.new_object_type(ObjectFlags::ANONYMOUS, symbol);
        if flags.intersects(SymbolFlags::CLASS) {
            let base_type_variable = self.get_base_type_variable_of_class(symbol);
            if base_type_variable.is_some() {
                return self.get_intersection_type(&[t, base_type_variable]);
            }
            return t;
        }
        if self.strict_null_checks && flags.intersects(SymbolFlags::OPTIONAL) {
            return self.get_optional_type(t, true /*isProperty*/);
        }
        t
    }

    // Go: checker/checker.go:17256 getBaseTypeVariableOfClass
    pub fn get_base_type_variable_of_class(&mut self, symbol: SymbolId) -> TypeId {
        let declared = self.get_declared_type_of_class_or_interface(symbol);
        let base_constructor_type = self.get_base_constructor_type_of_class(declared);
        let flags = self.ty(base_constructor_type).flags;
        if flags.intersects(TypeFlags::TYPE_VARIABLE) {
            return base_constructor_type;
        } else if flags.intersects(TypeFlags::INTERSECTION) {
            let types = self.ty(base_constructor_type).types_list();
            for t in types {
                if self.ty(t).flags.intersects(TypeFlags::TYPE_VARIABLE) {
                    return t;
                }
            }
            return TypeId::NIL;
        }
        TypeId::NIL
    }

    /**
     * The base constructor of a class can resolve to
     * * undefinedType if the class has no extends clause,
     * * errorType if an error occurred during resolution of the extends expression,
     * * nullType if the extends expression is the null value,
     * * anyType if the extends expression has type any, or
     * * an object type with at least one construct signature.
     */
    // Go: checker/checker.go:17277 getBaseConstructorTypeOfClass
    pub fn get_base_constructor_type_of_class(&mut self, t: TypeId) -> TypeId {
        let resolved = self
            .ty(t)
            .as_interface_type()
            .resolved_base_constructor_type;
        if resolved.is_some() {
            return resolved;
        }
        let base_type_node = self.get_base_type_node_of_class(t);
        if base_type_node.is_nil() {
            let undefined_type = self.undefined_type;
            self.ty_mut(t)
                .as_interface_type_mut()
                .resolved_base_constructor_type = undefined_type;
            return undefined_type;
        }
        if !self.push_type_resolution(
            t.into(),
            TypeSystemPropertyName::RESOLVED_BASE_CONSTRUCTOR_TYPE,
        ) {
            return self.error_type;
        }
        let base_constructor_type = self.check_expression(base_type_node.expression());
        if self
            .ty(base_constructor_type)
            .flags
            .intersects(TypeFlags::OBJECT | TypeFlags::INTERSECTION)
        {
            // Resolving the members of a class requires us to resolve the base class of that class.
            // We force resolution here such that we catch circularities now.
            self.resolve_structured_type_members(base_constructor_type);
        }
        if !self.pop_type_resolution() {
            let t_symbol = self.ty(t).symbol;
            let value_declaration = self.sym(t_symbol).value_declaration;
            let symbol_string = self.symbol_to_string(t_symbol);
            self.error(
                value_declaration,
                diag::X_0_is_referenced_directly_or_indirectly_in_its_own_base_expression,
                args![symbol_string],
            );
            if self
                .ty(t)
                .as_interface_type()
                .resolved_base_constructor_type
                .is_nil()
            {
                let error_type = self.error_type;
                self.ty_mut(t)
                    .as_interface_type_mut()
                    .resolved_base_constructor_type = error_type;
            }
            return self
                .ty(t)
                .as_interface_type()
                .resolved_base_constructor_type;
        }
        if !self
            .ty(base_constructor_type)
            .flags
            .intersects(TypeFlags::ANY)
            && base_constructor_type != self.null_widening_type
            && !self.is_constructor_type(base_constructor_type)
        {
            // PORT: Go calls `c.error` (which adds the diagnostic) and then
            // mutates the returned `*ast.Diagnostic` with `AddRelatedInfo`.
            // Diagnostics are owned values here, so the related info is
            // attached first and the diagnostic is added afterwards. `c.error`
            // is exactly `NewDiagnosticForNode` + `addDiagnostic`, and the
            // diagnostics collection keeps its entries sorted, so the result
            // is the same.
            let type_string = self.type_to_string(base_constructor_type);
            let mut err = new_diagnostic_for_node(
                base_type_node.expression(),
                diag::Type_0_is_not_a_constructor_function_type,
                args![type_string],
            );
            if self
                .ty(base_constructor_type)
                .flags
                .intersects(TypeFlags::TYPE_PARAMETER)
            {
                let constraint = self.get_constraint_from_type_parameter(base_constructor_type);
                let mut ctor_return = self.unknown_type;
                if constraint.is_some() {
                    let ctor_sigs = self
                        .get_signatures_of_type(constraint, SignatureKind::CONSTRUCT)
                        .to_vec();
                    if !ctor_sigs.is_empty() {
                        ctor_return = self.get_return_type_of_signature(ctor_sigs[0]);
                    }
                }
                let base_symbol = self.ty(base_constructor_type).symbol;
                let declarations = self.sym(base_symbol).declarations.clone();
                if !declarations.is_empty() {
                    let symbol_string = self.symbol_to_string(base_symbol);
                    let ctor_return_string = self.type_to_string(ctor_return);
                    err.add_related_info(Some(create_diagnostic_for_node(
                        declarations[0],
                        diag::Did_you_mean_for_0_to_be_constrained_to_type_new_args_Colon_any_1,
                        args![symbol_string, ctor_return_string],
                    )));
                }
            }
            self.add_diagnostic(err);
            if self
                .ty(t)
                .as_interface_type()
                .resolved_base_constructor_type
                .is_nil()
            {
                let error_type = self.error_type;
                self.ty_mut(t)
                    .as_interface_type_mut()
                    .resolved_base_constructor_type = error_type;
            }
            return self
                .ty(t)
                .as_interface_type()
                .resolved_base_constructor_type;
        }
        if self
            .ty(t)
            .as_interface_type()
            .resolved_base_constructor_type
            .is_nil()
        {
            self.ty_mut(t)
                .as_interface_type_mut()
                .resolved_base_constructor_type = base_constructor_type;
        }
        self.ty(t)
            .as_interface_type()
            .resolved_base_constructor_type
    }

    // Go: checker/checker.go:17329 isFunctionType
    pub fn is_function_type(&mut self, t: TypeId) -> bool {
        self.ty(t).flags.intersects(TypeFlags::OBJECT)
            && !self
                .get_signatures_of_type(t, SignatureKind::CALL)
                .is_empty()
    }

    // Go: checker/checker.go:17333 isConstructorType
    pub fn is_constructor_type(&mut self, t: TypeId) -> bool {
        if !self
            .get_signatures_of_type(t, SignatureKind::CONSTRUCT)
            .is_empty()
        {
            return true;
        }
        if self.ty(t).flags.intersects(TypeFlags::TYPE_VARIABLE) {
            let constraint = self.get_base_constraint_of_type(t);
            return constraint.is_some() && self.is_mixin_constructor_type(constraint);
        }
        false
    }

    // A type is a mixin constructor if it has a single construct signature taking no type parameters and a single
    // rest parameter of type any[].
    // Go: checker/checker.go:17346 isMixinConstructorType
    pub fn is_mixin_constructor_type(&mut self, t: TypeId) -> bool {
        let signatures = self
            .get_signatures_of_type(t, SignatureKind::CONSTRUCT)
            .to_vec();
        if signatures.len() == 1 {
            let s = signatures[0];
            if self.sig(s).type_parameters.is_empty()
                && self.sig(s).parameters.len() == 1
                && self.signature_has_rest_parameter(s)
            {
                let param = self.sig(s).parameters[0];
                let param_type = self.get_type_of_parameter(param);
                return self.is_type_any(param_type)
                    || self.get_element_type_of_array_type(param_type) == self.any_type;
            }
        }
        false
    }

    // Go: checker/checker.go:17358 signatureHasRestParameter
    // PORT: Go package function on `*Signature`; it reads signature data, so
    // it is a `Checker` method taking the handle.
    pub fn signature_has_rest_parameter(&self, sig: SignatureId) -> bool {
        self.sig(sig)
            .flags
            .intersects(SignatureFlags::HAS_REST_PARAMETER)
    }

    // Go: checker/checker.go:17362 getTypeOfParameter
    pub fn get_type_of_parameter(&mut self, symbol: SymbolId) -> TypeId {
        let declaration = self.sym(symbol).value_declaration;
        let t = self.get_type_of_symbol(symbol);
        let is_optional = self.parameter_declaration_is_optional(symbol, declaration);
        self.add_optionality_ex(t, false, is_optional)
    }

    /// The declaration test of Go `getTypeOfParameter`.
    ///
    /// PORT: memo, no Go counterpart. The test reads only the AST of the
    /// symbol's `value_declaration`, which does not change once the symbol is
    /// a signature parameter, so it runs once per symbol and the result is in
    /// `ValueSymbolLinks::optional_parameter`. The record is the one
    /// `get_type_of_symbol` has just read. When it has no record (a symbol
    /// with an error type), the test runs with no memo, so no record is added
    /// that `value_symbol_links.has` could see. Go reads no links here, so
    /// the read gives no symbol id (`try_get_without_id`).
    fn parameter_declaration_is_optional(&mut self, symbol: SymbolId, declaration: Node) -> bool {
        let test =
            |d: Node| d.is_some() && (d.initializer().is_some() || is_optional_declaration(d));
        match self
            .value_symbol_links
            .try_get_without_id(symbol)
            .map(|links| links.optional_parameter)
        {
            Some(Tristate::Unknown) => {
                let optional = test(declaration);
                self.value_symbol_links
                    .get_by_id(&self.symbols, symbol)
                    .optional_parameter = bool_to_tristate(optional);
                optional
            }
            Some(memo) => {
                debug_assert_eq!(memo.is_true(), test(declaration));
                memo.is_true()
            }
            None => test(declaration),
        }
    }

    // Go: checker/checker.go:17367 getConstraintOfType
    pub fn get_constraint_of_type(&mut self, t: TypeId) -> TypeId {
        let flags = self.ty(t).flags;
        if flags.intersects(TypeFlags::TYPE_PARAMETER) {
            return self.get_constraint_of_type_parameter(t);
        } else if flags.intersects(TypeFlags::INDEXED_ACCESS) {
            return self.get_constraint_of_indexed_access(t);
        } else if flags.intersects(TypeFlags::CONDITIONAL) {
            return self.get_constraint_of_conditional_type(t);
        }
        self.get_base_constraint_of_type(t)
    }

    // Go: checker/checker.go:17379 getConstraintOfTypeParameter
    pub fn get_constraint_of_type_parameter(&mut self, type_parameter: TypeId) -> TypeId {
        if self.has_non_circular_base_constraint(type_parameter) {
            return self.get_constraint_from_type_parameter(type_parameter);
        }
        TypeId::NIL
    }

    // Go: checker/checker.go:17386 hasNonCircularBaseConstraint
    pub fn has_non_circular_base_constraint(&mut self, t: TypeId) -> bool {
        self.get_resolved_base_constraint(t, &[]) != self.circular_constraint_type
    }

    // This is a worker function. Use getConstraintOfTypeParameter which guards against circular constraints
    // Go: checker/checker.go:17391 getConstraintFromTypeParameter
    pub fn get_constraint_from_type_parameter(&mut self, t: TypeId) -> TypeId {
        if !self.ty(t).flags.intersects(TypeFlags::TYPE_PARAMETER) {
            return TypeId::NIL;
        }

        if self.ty(t).as_type_parameter().constraint.is_nil() {
            let mut constraint: TypeId;
            let tp_target = self.ty(t).as_type_parameter().target;
            let tp_mapper = self.ty(t).as_type_parameter().mapper;
            if tp_target.is_some() {
                let target_constraint = self.get_constraint_of_type_parameter(tp_target);
                constraint = self.instantiate_type(target_constraint, tp_mapper);
            } else {
                let constraint_declaration = self.get_constraint_declaration(t);
                if constraint_declaration.is_some() {
                    constraint = self.get_type_from_type_node(constraint_declaration);
                    if self.ty(constraint).flags.intersects(TypeFlags::ANY)
                        && !self.is_error_type(constraint)
                    {
                        // use stringNumberSymbolType as the base constraint for mapped type key constraints (unknown isn;t assignable to that, but `any` was),
                        // use unknown otherwise
                        if is_mapped_type_node(constraint_declaration.parent().parent()) {
                            constraint = self.string_number_symbol_type;
                        } else {
                            constraint = self.unknown_type;
                        }
                    }
                } else {
                    constraint = self.get_inferred_type_parameter_constraint(t, false);
                }
            }
            if constraint.is_nil() {
                constraint = self.no_constraint_type;
            }
            self.ty_mut(t).as_type_parameter_mut().constraint = constraint;
        }
        let constraint = self.ty(t).as_type_parameter().constraint;
        if constraint != self.no_constraint_type {
            return constraint;
        }
        TypeId::NIL
    }

    // Go: checker/checker.go:17429 getConstraintOrUnknownFromTypeParameter
    pub fn get_constraint_or_unknown_from_type_parameter(&mut self, t: TypeId) -> TypeId {
        let result = self.get_constraint_from_type_parameter(t);
        if result.is_some() {
            result
        } else {
            self.unknown_type
        }
    }

    // Go: checker/checker.go:17434 getInferredTypeParameterConstraint
    pub fn get_inferred_type_parameter_constraint(
        &mut self,
        t: TypeId,
        omit_type_references: bool,
    ) -> TypeId {
        let mut inferences: Vec<TypeId> = Vec::new();
        let t_symbol = self.ty(t).symbol;
        if t_symbol.is_some() && !self.sym(t_symbol).declarations.is_empty() {
            let declarations = self.sym(t_symbol).declarations.clone();
            for &declaration in declarations.iter() {
                if is_infer_type_node(declaration.parent()) {
                    // When an 'infer T' declaration is immediately contained in a type reference node
                    // (such as 'Foo<infer T>'), T's constraint is inferred from the constraint of the
                    // corresponding type parameter in 'Foo'. When multiple 'infer T' declarations are
                    // present, we form an intersection of the inferred constraint types.
                    let mut child = declaration.parent();
                    let mut parent = child.parent();
                    while parent.is_some() && is_parenthesized_type_node(parent) {
                        child = parent;
                        parent = child.parent();
                    }
                    if is_type_reference_node(parent) && !omit_type_references {
                        let type_parameters =
                            self.get_type_parameters_for_type_reference_or_import(parent);
                        if !type_parameters.is_empty() {
                            let index = parent.type_arguments().iter().position(|n| n == child);
                            if let Some(index) = index {
                                if index < type_parameters.len() {
                                    let declared_constraint = self
                                        .get_constraint_of_type_parameter(type_parameters[index]);
                                    if declared_constraint.is_some() {
                                        // Type parameter constraints can reference other type parameters so
                                        // constraints need to be instantiated. If instantiation produces the
                                        // type parameter itself, we discard that inference. For example, in
                                        //   type Foo<T extends string, U extends T> = [T, U];
                                        //   type Bar<T> = T extends Foo<infer X, infer X> ? Foo<X, X> : T;
                                        // the instantiated constraint for U is X, so we discard that inference.
                                        let shared_type_parameters: Rc<Vec<TypeId>> =
                                            Rc::new(type_parameters.clone());
                                        let targets: Vec<DeferredTypeFn> = (0..type_parameters
                                            .len())
                                            .map(|index| {
                                                let type_parameters =
                                                    Rc::clone(&shared_type_parameters);
                                                let f: DeferredTypeFn =
                                                    Rc::new(move |c: &mut Checker| {
                                                        c.get_effective_type_argument_at_index(
                                                            parent,
                                                            &type_parameters,
                                                            index as i32,
                                                        )
                                                    });
                                                f
                                            })
                                            .collect();
                                        let mapper = self
                                            .new_deferred_type_mapper(&type_parameters, targets);
                                        let constraint =
                                            self.instantiate_type(declared_constraint, mapper);
                                        if constraint != t {
                                            inferences.push(constraint);
                                        }
                                    }
                                }
                            }
                        }
                    } else if is_parameter_declaration(parent)
                        && parent.dot_dot_dot_token().is_some()
                        || is_rest_type_node(parent)
                        || is_named_tuple_member(parent) && parent.dot_dot_dot_token().is_some()
                    {
                        let unknown_type = self.unknown_type;
                        let array_type = self.create_array_type(unknown_type);
                        inferences.push(array_type);
                    } else if is_template_literal_type_span(parent) {
                        inferences.push(self.string_type);
                    } else if is_type_parameter_declaration(parent)
                        && is_mapped_type_node(parent.parent())
                    {
                        inferences.push(self.string_number_symbol_type);
                    } else if is_mapped_type_node(parent)
                        && parent.type_().is_some()
                        && skip_parentheses(parent.type_()) == declaration.parent()
                        && is_conditional_type_node(parent.parent())
                        && parent.parent().extends_type() == parent
                        && is_mapped_type_node(parent.parent().check_type())
                        && parent.parent().check_type().type_().is_some()
                    {
                        let check_mapped_type = parent.parent().check_type();
                        let node_type = self.get_type_from_type_node(check_mapped_type.type_());
                        let check_mapped_type_parameter = check_mapped_type.type_parameter();
                        let tp_symbol = self.get_symbol_of_declaration(check_mapped_type_parameter);
                        let source = self.get_declared_type_of_type_parameter(tp_symbol);
                        let target = if check_mapped_type_parameter.constraint().is_some() {
                            self.get_type_from_type_node(check_mapped_type_parameter.constraint())
                        } else {
                            self.string_number_symbol_type
                        };
                        let mapper = self.new_simple_type_mapper(source, target);
                        let instantiated = self.instantiate_type(node_type, mapper);
                        inferences.push(instantiated);
                    }
                }
            }
        }
        if !inferences.is_empty() {
            return self.get_intersection_type(&inferences);
        }
        TypeId::NIL
    }

    // Go: checker/checker.go:17507 getTypeParametersForTypeReferenceOrImport
    pub fn get_type_parameters_for_type_reference_or_import(&mut self, node: Node) -> Vec<TypeId> {
        let t = self.get_type_from_type_node(node);
        if !self.is_error_type(t) {
            let symbol = self.get_resolved_symbol_or_nil(node);
            if symbol.is_some() {
                return self.get_type_parameters_for_type_and_symbol(t, symbol);
            }
        }
        Vec::new()
    }

    // Go: checker/checker.go:17518 getTypeParametersForTypeAndSymbol
    pub fn get_type_parameters_for_type_and_symbol(
        &mut self,
        t: TypeId,
        symbol: SymbolId,
    ) -> Vec<TypeId> {
        if !self.is_error_type(t) {
            if self.sym(symbol).flags.intersects(SymbolFlags::TYPE_ALIAS) {
                let type_parameters = self.type_alias_links.get(symbol).type_parameters.clone();
                if !type_parameters.is_empty() {
                    return type_parameters;
                }
            }
            if self.ty(t).object_flags.intersects(ObjectFlags::REFERENCE) {
                let target = self.ty(t).target();
                return self
                    .ty(target)
                    .as_interface_type()
                    .local_type_parameters()
                    .to_vec();
            }
        }
        Vec::new()
    }

    // Go: checker/checker.go:17532 getEffectiveTypeArgumentAtIndex
    pub fn get_effective_type_argument_at_index(
        &mut self,
        node: Node,
        type_parameters: &[TypeId],
        index: i32,
    ) -> TypeId {
        let type_arguments = node.type_arguments();
        if (index as usize) < type_arguments.len() {
            return self.get_type_from_type_node(type_arguments.get(index as usize));
        }
        self.get_effective_type_arguments(node, type_parameters)[index as usize]
    }

    // Go: checker/checker.go:17540 getConstraintOfIndexedAccess
    pub fn get_constraint_of_indexed_access(&mut self, t: TypeId) -> TypeId {
        if self.has_non_circular_base_constraint(t) {
            return self.get_constraint_from_indexed_access(t);
        }
        TypeId::NIL
    }

    // Go: checker/checker.go:17547 getConstraintFromIndexedAccess
    pub fn get_constraint_from_indexed_access(&mut self, t: TypeId) -> TypeId {
        let (object_type, index_type, access_flags) = {
            let d = self.ty(t).as_indexed_access_type();
            (d.object_type, d.index_type, d.access_flags)
        };
        if self.is_mapped_type_generic_indexed_access(t) {
            // For indexed access types of the form { [P in K]: E }[X], where K is non-generic and X is generic,
            // we substitute an instantiation of E where P is replaced with X.
            return self.substitute_indexed_mapped_type(object_type, index_type);
        }
        let index_constraint = self.get_simplified_type_or_constraint(index_type);
        if index_constraint.is_some() && index_constraint != index_type {
            let indexed_access = self.get_indexed_access_type_or_undefined(
                object_type,
                index_constraint,
                access_flags,
                Node::NIL,
                None,
            );
            if indexed_access.is_some() {
                return indexed_access;
            }
        }
        let object_constraint = self.get_simplified_type_or_constraint(object_type);
        if object_constraint.is_some() && object_constraint != object_type {
            return self.get_indexed_access_type_or_undefined(
                object_constraint,
                index_type,
                access_flags,
                Node::NIL,
                None,
            );
        }
        TypeId::NIL
    }

    // Go: checker/checker.go:17568 getConstraintOfConditionalType
    pub fn get_constraint_of_conditional_type(&mut self, t: TypeId) -> TypeId {
        if self.has_non_circular_base_constraint(t) {
            return self.get_constraint_from_conditional_type(t);
        }
        TypeId::NIL
    }

    // Go: checker/checker.go:17575 getConstraintFromConditionalType
    pub fn get_constraint_from_conditional_type(&mut self, t: TypeId) -> TypeId {
        let constraint = self.get_constraint_of_distributive_conditional_type(t);
        if constraint.is_some() {
            return constraint;
        }
        self.get_default_constraint_of_conditional_type(t)
    }

    // Go: checker/checker.go:17583 getDefaultConstraintOfConditionalType
    pub fn get_default_constraint_of_conditional_type(&mut self, t: TypeId) -> TypeId {
        if self
            .ty(t)
            .as_conditional_type()
            .resolved_default_constraint
            .is_nil()
        {
            // An `any` branch of a conditional type would normally be viral - specifically, without special handling here,
            // a conditional type with a single branch of type `any` would be assignable to anything, since it's constraint would simplify to
            // just `any`. This result is _usually_ unwanted - so instead here we elide an `any` branch from the constraint type,
            // in effect treating `any` like `never` rather than `unknown` in this location.
            let true_constraint = self.get_inferred_true_type_from_conditional_type(t);
            let false_constraint = self.get_false_type_from_conditional_type(t);
            let resolved = if self.is_type_any(true_constraint) {
                false_constraint
            } else if self.is_type_any(false_constraint) {
                true_constraint
            } else {
                self.get_union_type(&[true_constraint, false_constraint])
            };
            self.ty_mut(t)
                .as_conditional_type_mut()
                .resolved_default_constraint = resolved;
        }
        self.ty(t).as_conditional_type().resolved_default_constraint
    }

    // Go: checker/checker.go:17604 getConstraintOfDistributiveConditionalType
    pub fn get_constraint_of_distributive_conditional_type(&mut self, t: TypeId) -> TypeId {
        if self
            .ty(t)
            .as_conditional_type()
            .resolved_constraint_of_distributive
            .is_nil()
        {
            // Check if we have a conditional type of the form 'T extends U ? X : Y', where T is a constrained
            // type parameter. If so, create an instantiation of the conditional type where T is replaced
            // with its constraint. We do this because if the constraint is a union type it will be distributed
            // over the conditional type and possibly reduced. For example, 'T extends undefined ? never : T'
            // removes 'undefined' from T.
            // We skip returning a distributive constraint for a restrictive instantiation of a conditional type
            // as the constraint for all type params (check type included) have been replace with `unknown`, which
            // is going to produce even more false positive/negative results than the distribute constraint already does.
            // Please note: the distributive constraint is a kludge for emulating what a negated type could to do filter
            // a union - once negated types exist and are applied to the conditional false branch, this "constraint"
            // likely doesn't need to exist.
            let (is_distributive, root_check_type, check_type, d_mapper) = {
                let d = self.ty(t).as_conditional_type();
                let root = d.root.borrow();
                (
                    root.is_distributive,
                    root.check_type,
                    d.check_type,
                    d.mapper,
                )
            };
            let t_id = t;
            let cached = self
                .cached_types
                .get(&CachedTypeKey {
                    kind: CachedTypeKind::RESTRICTIVE_INSTANTIATION,
                    type_id: t_id,
                })
                .copied()
                .unwrap_or_default();
            if is_distributive && cached != t {
                let mut constraint = self.get_simplified_type(check_type, false /*writing*/);
                if constraint == check_type {
                    constraint = self.get_constraint_of_type(constraint);
                }
                if constraint.is_some() && constraint != check_type {
                    let mapper = self.prepend_type_mapping(root_check_type, constraint, d_mapper);
                    let instantiated = self.get_conditional_type_instantiation(
                        t, mapper, true, /*forConstraint*/
                        None,
                    );
                    if !self.ty(instantiated).flags.intersects(TypeFlags::NEVER) {
                        self.ty_mut(t)
                            .as_conditional_type_mut()
                            .resolved_constraint_of_distributive = instantiated;
                        return instantiated;
                    }
                }
            }
            let no_constraint_type = self.no_constraint_type;
            self.ty_mut(t)
                .as_conditional_type_mut()
                .resolved_constraint_of_distributive = no_constraint_type;
        }
        let resolved = self
            .ty(t)
            .as_conditional_type()
            .resolved_constraint_of_distributive;
        if resolved != self.no_constraint_type {
            return resolved;
        }
        TypeId::NIL
    }

    // Go: checker/checker.go:17639 getDeclaredTypeOfClassOrInterface
    pub fn get_declared_type_of_class_or_interface(&mut self, symbol: SymbolId) -> TypeId {
        if self.declared_type_links.get(symbol).declared_type.is_nil() {
            let kind = if self.sym(symbol).flags.intersects(SymbolFlags::CLASS) {
                ObjectFlags::CLASS
            } else {
                ObjectFlags::INTERFACE
            };
            let t = self.new_object_type(kind, symbol);
            self.declared_type_links.get(symbol).declared_type = t;
            let outer_type_parameters =
                self.get_outer_type_parameters_of_class_or_interface(symbol);
            let outer_type_parameter_count = outer_type_parameters.len();
            let type_parameters = self
                .append_local_type_parameters_of_class_or_interface_or_type_alias(
                    outer_type_parameters,
                    symbol,
                );
            // A class or interface is generic if it has type parameters or a "this" type. We always give classes a "this" type
            // because it is not feasible to analyze all members to determine if the "this" type escapes the class (in particular,
            // property types inferred from initializers and method return types inferred from return statements are very hard
            // to exhaustively analyze). We give interfaces a "this" type if we can't definitely determine that they are free of
            // "this" references.
            // PORT: Go `typeParameters != nil`. The Go helpers only return a
            // non-nil slice when it has elements, so this checks for elements.
            if !type_parameters.is_empty()
                || kind == ObjectFlags::CLASS
                || !self.is_thisless_interface(symbol)
            {
                self.ty_mut(t).object_flags |= ObjectFlags::REFERENCE;
                let this_type = self.new_type_parameter(symbol);
                self.ty_mut(this_type).as_type_parameter_mut().is_this_type = true;
                self.ty_mut(this_type).as_type_parameter_mut().constraint = t;
                let mut all_type_parameters = type_parameters;
                all_type_parameters.push(this_type);
                let d = self.ty_mut(t).as_interface_type_mut();
                d.this_type = this_type;
                d.all_type_parameters = all_type_parameters;
                d.outer_type_parameter_count = outer_type_parameter_count as i32;
                d.reference.resolved_type_arguments = d.type_parameters().into();
                let key = get_type_list_key(&d.reference.resolved_type_arguments);
                let mut instantiations = InstantiationMap::default();
                instantiations.insert(key, t);
                d.instantiations = Some(instantiations);
                d.reference.object.target = t;
            }
        }
        self.declared_type_links.get(symbol).declared_type
    }

    /**
     * Returns true if the interface given by the symbol is free of "this" references.
     *
     * Specifically, the result is true if the interface itself contains no references
     * to "this" in its body, if all base types are interfaces,
     * and if none of the base interfaces have a "this" type.
     */
    // Go: checker/checker.go:17676 isThislessInterface
    pub fn is_thisless_interface(&mut self, symbol: SymbolId) -> bool {
        let declarations = self.sym(symbol).declarations.clone();
        for &declaration in declarations.iter() {
            if is_interface_declaration(declaration) {
                if declaration.flags().intersects(NodeFlags::CONTAINS_THIS) {
                    return false;
                }
                let base_type_nodes = get_extends_heritage_clause_elements(declaration);
                for node in base_type_nodes {
                    let name = get_heritage_clause_element_name(node);
                    if is_entity_name(name) || is_entity_name_expression(name) {
                        let base_symbol = self.resolve_entity_name(
                            name,
                            SymbolFlags::TYPE,
                            true, /*ignoreErrors*/
                            false,
                            Node::NIL,
                        );
                        if base_symbol.is_nil()
                            || !self
                                .sym(base_symbol)
                                .flags
                                .intersects(SymbolFlags::INTERFACE)
                        {
                            return false;
                        }
                        let declared = self.get_declared_type_of_class_or_interface(base_symbol);
                        if self.ty(declared).as_interface_type().this_type.is_some() {
                            return false;
                        }
                    }
                }
            }
        }
        true
    }
}

// Go: checker/checker.go:17697 CacheHashKey
// PORT: Go `CacheHashKey` is an `xxh3.Uint128` (`{Hi, Lo uint64}`). It is
// 4-byte aligned so cache entries with a 4-byte value take 20 bytes, not 24.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
#[repr(C, packed(4))]
pub struct CacheHashKey {
    pub hi: u64,
    pub lo: u64,
}

// PORT: the key is already an xxh3 hash, so map hashing feeds only its low
// half to the hasher. Equality still compares both halves. No code depends on
// the iteration order of maps with these keys.
impl std::hash::Hash for CacheHashKey {
    #[inline]
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        state.write_u64(self.lo);
    }
}

impl CacheHashKey {
    // Go: checker/checker.go:17699 CacheHashKey.IsZero
    pub fn is_zero(&self) -> bool {
        self.hi == 0 && self.lo == 0
    }
}

/// xxh3 default secret (`zeebo/xxh3` `key`, the XXH3 kSecret).
const XXH3_SECRET: [u8; 192] = [
    0xb8, 0xfe, 0x6c, 0x39, 0x23, 0xa4, 0x4b, 0xbe, 0x7c, 0x01, 0x81, 0x2c, 0xf7, 0x21, 0xad, 0x1c,
    0xde, 0xd4, 0x6d, 0xe9, 0x83, 0x90, 0x97, 0xdb, 0x72, 0x40, 0xa4, 0xa4, 0xb7, 0xb3, 0x67, 0x1f,
    0xcb, 0x79, 0xe6, 0x4e, 0xcc, 0xc0, 0xe5, 0x78, 0x82, 0x5a, 0xd0, 0x7d, 0xcc, 0xff, 0x72, 0x21,
    0xb8, 0x08, 0x46, 0x74, 0xf7, 0x43, 0x24, 0x8e, 0xe0, 0x35, 0x90, 0xe6, 0x81, 0x3a, 0x26, 0x4c,
    0x3c, 0x28, 0x52, 0xbb, 0x91, 0xc3, 0x00, 0xcb, 0x88, 0xd0, 0x65, 0x8b, 0x1b, 0x53, 0x2e, 0xa3,
    0x71, 0x64, 0x48, 0x97, 0xa2, 0x0d, 0xf9, 0x4e, 0x38, 0x19, 0xef, 0x46, 0xa9, 0xde, 0xac, 0xd8,
    0xa8, 0xfa, 0x76, 0x3f, 0xe3, 0x9c, 0x34, 0x3f, 0xf9, 0xdc, 0xbb, 0xc7, 0xc7, 0x0b, 0x4f, 0x1d,
    0x8a, 0x51, 0xe0, 0x4b, 0xcd, 0xb4, 0x59, 0x31, 0xc8, 0x9f, 0x7e, 0xc9, 0xd9, 0x78, 0x73, 0x64,
    0xea, 0xc5, 0xac, 0x83, 0x34, 0xd3, 0xeb, 0xc3, 0xc5, 0x81, 0xa0, 0xff, 0xfa, 0x13, 0x63, 0xeb,
    0x17, 0x0d, 0xdd, 0x51, 0xb7, 0xf0, 0xda, 0x49, 0xd3, 0x16, 0x55, 0x26, 0x29, 0xd4, 0x68, 0x9e,
    0x2b, 0x16, 0xbe, 0x58, 0x7d, 0x47, 0xa1, 0xfc, 0x8f, 0xf8, 0xb8, 0xd1, 0x7a, 0xd0, 0x31, 0xce,
    0x45, 0xcb, 0x3a, 0x8f, 0x95, 0x16, 0x04, 0x28, 0xaf, 0xd7, 0xfb, 0xca, 0xbb, 0x4b, 0x40, 0x7e,
];

const XXH_PRIME32_1: u64 = 2654435761;
const XXH_PRIME32_2: u64 = 2246822519;
const XXH_PRIME32_3: u64 = 3266489917;
const XXH_PRIME64_1: u64 = 11400714785074694791;
const XXH_PRIME64_2: u64 = 14029467366897019727;
const XXH_PRIME64_3: u64 = 1609587929392839161;
const XXH_PRIME64_4: u64 = 9650029242287828579;
const XXH_PRIME64_5: u64 = 2870177450012600261;
const XXH_STRIPE: usize = 64;
const XXH_BLOCK: usize = 1024;

#[inline(always)]
fn xxh_read32(b: &[u8], o: usize) -> u64 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap()) as u64
}

#[inline(always)]
fn xxh_read64(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}

#[inline(always)]
fn xxh_key64(o: usize) -> u64 {
    xxh_read64(&XXH3_SECRET, o)
}

#[inline(always)]
fn xxh_mul_fold64(x: u64, y: u64) -> u64 {
    let p = (x as u128).wrapping_mul(y as u128);
    (p >> 64) as u64 ^ p as u64
}

#[inline(always)]
fn xxh3_avalanche(mut x: u64) -> u64 {
    x ^= x >> 37;
    x = x.wrapping_mul(0x165667919e3779f9);
    x ^ (x >> 32)
}

#[inline(always)]
fn xxh64_avalanche_small(mut x: u64) -> u64 {
    x = x.wrapping_mul(XXH_PRIME64_2);
    x ^= x >> 29;
    x = x.wrapping_mul(XXH_PRIME64_3);
    x ^ (x >> 32)
}

/// One 32-byte round of the 17..240 byte paths: `lo`/`hi` mix `a` and `b`
/// (two 16-byte halves) with the secret at `ka` and `kb`.
#[inline(always)]
fn xxh3_mix32(acc: &mut (u64, u64), p: &[u8], a: usize, b: usize, ka: usize, kb: usize) {
    let (a0, a1) = (xxh_read64(p, a), xxh_read64(p, a + 8));
    let (b0, b1) = (xxh_read64(p, b), xxh_read64(p, b + 8));
    acc.0 = acc
        .0
        .wrapping_add(xxh_mul_fold64(a0 ^ xxh_key64(ka), a1 ^ xxh_key64(ka + 8)));
    acc.0 ^= b0.wrapping_add(b1);
    acc.1 = acc
        .1
        .wrapping_add(xxh_mul_fold64(b0 ^ xxh_key64(kb), b1 ^ xxh_key64(kb + 8)));
    acc.1 ^= a0.wrapping_add(a1);
}

/// XXH3 accumulate over one 64-byte stripe at `p[o..]` with the secret at `k`.
#[inline(always)]
fn xxh3_accumulate_stripe(accs: &mut [u64; 8], p: &[u8], o: usize, k: usize) {
    // One bounds check for the stripe instead of one per word.
    let stripe: &[u8; XXH_STRIPE] = p[o..o + XXH_STRIPE].try_into().unwrap();
    let key: &[u8; XXH_STRIPE] = XXH3_SECRET[k..k + XXH_STRIPE].try_into().unwrap();
    for i in 0..8 {
        let dv = u64::from_le_bytes(stripe[8 * i..8 * i + 8].try_into().unwrap());
        let dk = dv ^ u64::from_le_bytes(key[8 * i..8 * i + 8].try_into().unwrap());
        accs[i ^ 1] = accs[i ^ 1].wrapping_add(dv);
        accs[i] = accs[i].wrapping_add((dk & 0xffff_ffff).wrapping_mul(dk >> 32));
    }
}

#[inline(always)]
fn xxh3_scramble(accs: &mut [u64; 8]) {
    for (i, acc) in accs.iter_mut().enumerate() {
        *acc ^= *acc >> 47;
        *acc ^= xxh_key64(128 + 8 * i);
        *acc = acc.wrapping_mul(XXH_PRIME32_1);
    }
}

/// The 4..=8 byte path of `xxh3_hash128`, from the first and last 4 bytes
/// (little endian, as `u64`) and the length `l`. Returns `(hi, lo)`.
#[inline(always)]
fn xxh3_hash128_4to8(first: u64, last: u64, l: u64) -> (u64, u64) {
    let bitflip = xxh_key64(16) ^ xxh_key64(24);
    let input_64 = first + (last << 32);
    let keyed = input_64 ^ bitflip;
    let r = (keyed as u128).wrapping_mul(XXH_PRIME64_1.wrapping_add(l << 2) as u128);
    let mut r_hi = (r >> 64) as u64;
    let mut r_lo = r as u64;
    r_hi = r_hi.wrapping_add(r_lo << 1);
    r_lo ^= r_hi >> 3;
    r_lo ^= r_lo >> 35;
    r_lo = r_lo.wrapping_mul(0x9fb21c651e98df25);
    r_lo ^= r_lo >> 28;
    (xxh3_avalanche(r_hi), r_lo)
}

/// The 9..=16 byte path of `xxh3_hash128`, from the first and last 8 bytes
/// (little endian) and the length `l`. Returns `(hi, lo)`.
#[inline(always)]
fn xxh3_hash128_9to16(input_lo: u64, input_hi: u64, l: u64) -> (u64, u64) {
    let bitflipl = xxh_key64(32) ^ xxh_key64(40);
    let bitfliph = xxh_key64(48) ^ xxh_key64(56);
    let mut input_hi = input_hi;
    let m = ((input_lo ^ input_hi ^ bitflipl) as u128).wrapping_mul(XXH_PRIME64_1 as u128);
    let mut m_h = (m >> 64) as u64;
    let mut m_l = m as u64;
    m_l = m_l.wrapping_add((l - 1) << 54);
    input_hi ^= bitfliph;
    m_h = m_h.wrapping_add(
        input_hi.wrapping_add((input_hi & 0xffff_ffff).wrapping_mul(XXH_PRIME32_2 - 1)),
    );
    m_l ^= m_h.swap_bytes();
    let r = (m_l as u128).wrapping_mul(XXH_PRIME64_2 as u128);
    let r_hi = ((r >> 64) as u64).wrapping_add(m_h.wrapping_mul(XXH_PRIME64_2));
    (xxh3_avalanche(r_hi), xxh3_avalanche(r as u64))
}

/// `xxh3_hash128` of a short key built in a stack array, with no
/// `KeyBuilder`. `N` is 9..=16, so the length dispatch is gone and the two
/// 8-byte reads fold into register moves.
#[inline(always)]
fn short_key_hash<const N: usize>(p: &[u8; N]) -> CacheHashKey {
    const { assert!(N > 8 && N <= 16) };
    let (hi, lo) = xxh3_hash128_9to16(xxh_read64(p, 0), xxh_read64(p, N - 8), N as u64);
    CacheHashKey { hi, lo }
}

/// Go `keyBuilder{}; writeType(t); writeAlias(nil); hash()`, computed in
/// registers. The key bytes are `t` (4 bytes LE) and then `0`, so the last
/// 4 bytes are `t >> 8`. Same value as the `KeyBuilder` path.
#[inline]
pub fn type_key_no_alias(t: TypeId) -> CacheHashKey {
    let (hi, lo) = xxh3_hash128_4to8(t.0 as u64, (t.0 >> 8) as u64, 5);
    CacheHashKey { hi, lo }
}

/// Go `zeebo/xxh3.Hash128` (seed 0). Returns `(hi, lo)`.
fn xxh3_hash128(p: &[u8]) -> (u64, u64) {
    let l = p.len();
    let lu = l as u64;
    if l <= 16 {
        let (lo, hi);
        if l > 8 {
            return xxh3_hash128_9to16(xxh_read64(p, 0), xxh_read64(p, l - 8), lu);
        } else if l > 3 {
            return xxh3_hash128_4to8(xxh_read32(p, 0), xxh_read32(p, l - 4), lu);
        } else if l == 3 {
            let c12 = p[0] as u64 | (p[1] as u64) << 8;
            lo = (c12 << 16) + p[2] as u64 + (3 << 8);
        } else if l == 2 {
            let c12 = p[0] as u64 | (p[1] as u64) << 8;
            lo = (c12 * ((1 << 24) + 1) >> 8) + (2 << 8);
        } else if l == 1 {
            lo = (p[0] as u64) * ((1 << 24) + (1 << 16) + 1) + (1 << 8);
        } else {
            return (0x99aa06d3014798d8, 0x6001c324468d497f);
        }
        hi = (lo as u32).swap_bytes().rotate_left(13) as u64;
        let lo = lo ^ (xxh_read32(&XXH3_SECRET, 0) ^ xxh_read32(&XXH3_SECRET, 4));
        let hi = hi ^ (xxh_read32(&XXH3_SECRET, 8) ^ xxh_read32(&XXH3_SECRET, 12));
        return (xxh64_avalanche_small(hi), xxh64_avalanche_small(lo));
    }
    if l <= 240 {
        // acc.0 = hi, acc.1 = lo
        let mut acc = (0u64, lu.wrapping_mul(XXH_PRIME64_1));
        if l <= 128 {
            if l > 32 {
                if l > 64 {
                    if l > 96 {
                        xxh3_mix32(&mut acc, p, l - 64, 48, 112, 96);
                    }
                    xxh3_mix32(&mut acc, p, l - 48, 32, 80, 64);
                }
                xxh3_mix32(&mut acc, p, l - 32, 16, 48, 32);
            }
            xxh3_mix32(&mut acc, p, l - 16, 0, 16, 0);
        } else {
            for i in 0..4 {
                xxh3_mix32(&mut acc, p, 32 * i + 16, 32 * i, 32 * i + 16, 32 * i);
            }
            acc.0 = xxh3_avalanche(acc.0);
            acc.1 = xxh3_avalanche(acc.1);
            let top = l & !31;
            let mut i = 128;
            while i < top {
                xxh3_mix32(&mut acc, p, i + 16, i, i - 109, i - 125);
                i += 32;
            }
            // last 32 bytes: hi mixes the first half, lo the second.
            xxh3_mix32(&mut acc, p, l - 32, l - 16, 119, 103);
        }
        let (hi, lo) = acc;
        let new_hi = lo
            .wrapping_mul(XXH_PRIME64_1)
            .wrapping_add(hi.wrapping_mul(XXH_PRIME64_4))
            .wrapping_add(lu.wrapping_mul(XXH_PRIME64_2));
        let new_lo = hi.wrapping_add(lo);
        return (
            xxh3_avalanche(new_hi).wrapping_neg(),
            xxh3_avalanche(new_lo),
        );
    }
    // Long input: stripes of 64 bytes, blocks of 1024 bytes.
    let mut accs: [u64; 8] = [
        XXH_PRIME32_3,
        XXH_PRIME64_1,
        XXH_PRIME64_2,
        XXH_PRIME64_3,
        XXH_PRIME64_4,
        XXH_PRIME32_2,
        XXH_PRIME64_5,
        XXH_PRIME32_1,
    ];
    let mut o = 0;
    let mut rest = l;
    while rest > XXH_BLOCK {
        for s in 0..XXH_BLOCK / XXH_STRIPE {
            xxh3_accumulate_stripe(&mut accs, p, o + s * XXH_STRIPE, 8 * s);
        }
        xxh3_scramble(&mut accs);
        o += XXH_BLOCK;
        rest -= XXH_BLOCK;
    }
    let stripes = (rest - 1) / XXH_STRIPE;
    for s in 0..stripes {
        xxh3_accumulate_stripe(&mut accs, p, o + s * XXH_STRIPE, 8 * s);
    }
    xxh3_accumulate_stripe(&mut accs, p, l - XXH_STRIPE, 121);
    let mut lo = lu.wrapping_mul(XXH_PRIME64_1);
    let mut hi = !lu.wrapping_mul(XXH_PRIME64_2);
    for i in 0..4 {
        let (a, b) = (accs[2 * i], accs[2 * i + 1]);
        lo = lo.wrapping_add(xxh_mul_fold64(
            a ^ xxh_key64(11 + 16 * i),
            b ^ xxh_key64(19 + 16 * i),
        ));
        hi = hi.wrapping_add(xxh_mul_fold64(
            a ^ xxh_key64(117 + 16 * i),
            b ^ xxh_key64(125 + 16 * i),
        ));
    }
    (xxh3_avalanche(hi), xxh3_avalanche(lo))
}

// Go: checker/checker.go:17703 keyBuilder
// PORT: Go hashes with `xxh3.Hash128`. The crate has no xxh3 dependency and
// the contract forbids new ones, so `xxh3_hash128` in this file is a hand
// port of `zeebo/xxh3` `Hash128` (seed 0). A Go nil `overflowBuffer` is an
// empty one.
#[derive(Clone)]
pub struct KeyBuilder {
    pub inline_length: usize,
    pub overflow_buffer: Vec<u8>,
    pub inline_buffer: [u8; KEY_BUILDER_INLINE_BUFFER_LEN],
}

/// Go `len(keyBuilder.inlineBuffer)`.
const KEY_BUILDER_INLINE_BUFFER_LEN: usize = 192;

impl Default for KeyBuilder {
    #[inline]
    fn default() -> Self {
        KeyBuilder {
            inline_length: 0,
            overflow_buffer: Vec::new(),
            inline_buffer: [0; KEY_BUILDER_INLINE_BUFFER_LEN],
        }
    }
}

impl std::fmt::Debug for KeyBuilder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KeyBuilder")
            .field("overflow_buffer", &self.overflow_buffer)
            .field("inline_buffer", &&self.inline_buffer[..self.inline_length])
            .finish()
    }
}

impl KeyBuilder {
    // Go: checker/checker.go:17709 keyBuilder.hash
    pub fn hash(&self) -> CacheHashKey {
        let (hi, lo) = if self.overflow_buffer.is_empty() {
            xxh3_hash128(&self.inline_buffer[..self.inline_length])
        } else {
            let mut bytes = self.overflow_buffer.clone();
            bytes.extend_from_slice(&self.inline_buffer[..self.inline_length]);
            xxh3_hash128(&bytes)
        };
        CacheHashKey { hi, lo }
    }

    // Go: checker/checker.go:17718 keyBuilder.spill
    // spill moves the buffered bytes onto the end of overflowBuffer, so the key's byte
    // stream stays overflowBuffer followed by inlineBuffer.
    #[cold]
    #[inline(never)]
    fn spill(&mut self) {
        self.overflow_buffer
            .extend_from_slice(&self.inline_buffer[..self.inline_length]);
        self.inline_length = 0;
    }

    // Go: checker/checker.go:17723 keyBuilder.writeByte
    #[inline]
    pub fn write_byte(&mut self, c: u8) {
        if self.inline_length == self.inline_buffer.len() {
            self.spill();
        }
        self.inline_buffer[self.inline_length] = c;
        self.inline_length += 1;
    }

    // Go: checker/checker.go:17731 keyBuilder.writeString
    pub fn write_string(&mut self, s: &str) {
        if self.inline_length + s.len() > self.inline_buffer.len() {
            self.spill();
            if s.len() > self.inline_buffer.len() {
                self.overflow_buffer.extend_from_slice(s.as_bytes());
                return;
            }
        }
        self.inline_buffer[self.inline_length..self.inline_length + s.len()]
            .copy_from_slice(s.as_bytes());
        self.inline_length += s.len();
    }

    // Go: checker/checker.go:17742 keyBuilder.writeUint32
    #[inline]
    pub fn write_uint32(&mut self, v: u32) {
        if self.inline_length + 4 > self.inline_buffer.len() {
            self.spill();
        }
        self.inline_buffer[self.inline_length..self.inline_length + 4]
            .copy_from_slice(&v.to_le_bytes());
        self.inline_length += 4;
    }

    // Go: checker/checker.go:17750 keyBuilder.writeUint64
    #[inline]
    pub fn write_uint64(&mut self, v: u64) {
        if self.inline_length + 8 > self.inline_buffer.len() {
            self.spill();
        }
        self.inline_buffer[self.inline_length..self.inline_length + 8]
            .copy_from_slice(&v.to_le_bytes());
        self.inline_length += 8;
    }

    // Go: checker/checker.go:17758 keyBuilder.writeInt
    pub fn write_int(&mut self, value: i32) {
        self.write_uint64(value as i64 as u64);
    }

    // Go: checker/checker.go:17762 keyBuilder.writeSymbol
    // PORT: `ast.GetSymbolId` takes the symbol arena per the contract.
    pub fn write_symbol(&mut self, symbols: &SymbolArena, s: SymbolId) {
        self.write_uint64(get_symbol_id(symbols, s));
    }

    // Go: checker/checker.go:17766 keyBuilder.writeType
    // PORT: Go writes `t.id`; the handle value is the type id.
    #[inline]
    pub fn write_type(&mut self, t: TypeId) {
        self.write_uint32(t.0);
    }

    // Go: checker/checker.go:17770 keyBuilder.writeTypes
    // PERF: when the whole list fits in the inline buffer, it is written with
    // one room test. The bytes are the same as `write_int(len)` and then
    // `write_type` for each type.
    #[inline]
    pub fn write_types(&mut self, types: &[TypeId]) {
        let len = self.inline_length;
        let size = 8 + 4 * types.len();
        let room = self
            .inline_buffer
            .get_mut(len..)
            .and_then(|rest| rest.get_mut(..size));
        let Some(dst) = room else {
            self.write_int(types.len() as i32);
            for &t in types {
                self.write_type(t);
            }
            return;
        };
        let (head, tail) = dst.split_at_mut(8);
        head.copy_from_slice(&(types.len() as i32 as i64 as u64).to_le_bytes());
        for (d, &t) in tail.chunks_exact_mut(4).zip(types) {
            d.copy_from_slice(&t.0.to_le_bytes());
        }
        self.inline_length = len + size;
    }

    // Go: checker/checker.go:17777 keyBuilder.writeAlias
    pub fn write_alias(&mut self, symbols: &SymbolArena, alias: Option<&TypeAlias>) {
        if let Some(alias) = alias {
            self.write_byte(1);
            self.write_symbol(symbols, alias.symbol);
            self.write_types(&alias.type_arguments);
        } else {
            self.write_byte(0);
        }
    }

    // Go: checker/checker.go:17787 keyBuilder.writeGenericTypeReferences
    // PORT: Go reaches the checker through `t.checker`; it is passed in.
    pub fn write_generic_type_references(
        &mut self,
        c: &mut Checker,
        source: TypeId,
        target: TypeId,
        ignore_constraints: bool,
    ) -> bool {
        let mut constrained = false;
        let mut type_parameters: Vec<TypeId> = Vec::new();
        self.write_type_reference_for_key(
            c,
            source,
            0,
            ignore_constraints,
            &mut type_parameters,
            &mut constrained,
        );
        self.write_byte(b',');
        self.write_type_reference_for_key(
            c,
            target,
            0,
            ignore_constraints,
            &mut type_parameters,
            &mut constrained,
        );
        constrained
    }

    // PORT: the recursive `writeTypeReference` closure inside Go
    // `keyBuilder.writeGenericTypeReferences`, with its captured variables
    // passed explicitly.
    fn write_type_reference_for_key(
        &mut self,
        c: &mut Checker,
        ref_: TypeId,
        depth: i32,
        ignore_constraints: bool,
        type_parameters: &mut Vec<TypeId>,
        constrained: &mut bool,
    ) {
        let ref_target = c.ty(ref_).target();
        self.write_type(ref_target);
        // PORT: read in place by index; the resolved list does not change.
        let count = c.ty(ref_).as_type_reference().resolved_type_arguments.len();
        for i in 0..count {
            let t = c.ty(ref_).as_type_reference().resolved_type_arguments[i];
            if c.ty(t).flags.intersects(TypeFlags::TYPE_PARAMETER) {
                if ignore_constraints || c.get_constraint_of_type_parameter(t).is_nil() {
                    let index = match type_parameters.iter().position(|&p| p == t) {
                        Some(index) => index,
                        None => {
                            let index = type_parameters.len();
                            type_parameters.push(t);
                            index
                        }
                    };
                    self.write_byte(b'=');
                    self.write_int(index as i32);
                    continue;
                }
                *constrained = true;
            } else if depth < 4 && c.is_type_reference_with_generic_arguments(t) {
                self.write_byte(b'<');
                self.write_type_reference_for_key(
                    c,
                    t,
                    depth + 1,
                    ignore_constraints,
                    type_parameters,
                    constrained,
                );
                self.write_byte(b'>');
                continue;
            }
            self.write_byte(b'-');
            self.write_type(t);
        }
    }

    // Go: checker/checker.go:17822 keyBuilder.writeNodeId
    // PORT: Go `ast.NodeId` is a `uint64` (`get_node_id` returns `u64`).
    pub fn write_node_id(&mut self, id: u64) {
        self.write_uint64(id);
    }

    // Go: checker/checker.go:17826 keyBuilder.writeNode
    pub fn write_node(&mut self, node: Node) {
        if node.is_some() {
            self.write_node_id(get_node_id(node));
        }
    }
}

/// Size of the `ShortKey` buffer. A type list and an alias with 4 types each
/// and a flag byte take 58 bytes.
const SHORT_KEY_CAP: usize = 64;

/// Byte length of Go `keyBuilder.writeTypes` for `n` types: the count as 8
/// bytes and then 4 bytes for each type.
#[inline(always)]
const fn type_list_key_len(n: usize) -> usize {
    8 + 4 * n
}

/// Byte length of Go `keyBuilder.writeAlias`: 1 for a nil alias, else the
/// tag byte, the 8-byte symbol id and the type list.
#[inline(always)]
fn alias_key_len(alias: Option<(SymbolId, &[TypeId])>) -> usize {
    match alias {
        Some((_, types)) => 9 + type_list_key_len(types.len()),
        None => 1,
    }
}

/// PERF: a cache key built in a 64-byte stack array, for the hot
/// `getTypeInstantiationKey`, `getConditionalTypeKey` and
/// `getIntersectionKey`. A `KeyBuilder` zero-fills 192 bytes and owns an
/// overflow vector, and its type list copy loop becomes a `memcpy` call.
/// Here the fill is 64 bytes (safe code must initialize the array; this is
/// a few vector stores and no call), and lists of up to 4 types are
/// fixed-size stores. The caller tests the room for the whole key once and
/// uses the `KeyBuilder` path for a longer key, so the bounds checks of the
/// stores never fail. The bytes and their order are the `KeyBuilder` bytes,
/// so the xxh3 hash is the same.
struct ShortKey {
    len: usize,
    buf: [u8; SHORT_KEY_CAP],
}

impl ShortKey {
    #[inline(always)]
    fn new() -> Self {
        ShortKey {
            len: 0,
            buf: [0; SHORT_KEY_CAP],
        }
    }

    /// Fixed-size store of `bytes` after the bytes written so far.
    #[inline(always)]
    fn put<const N: usize>(&mut self, bytes: [u8; N]) {
        let len = self.len;
        self.buf[len..len + N].copy_from_slice(&bytes);
        self.len = len + N;
    }

    /// Same bytes as `KeyBuilder::write_types`. Up to 4 types are fixed-size
    /// stores (two types are one 8-byte store); a longer list uses the slice
    /// copy.
    #[inline(always)]
    fn write_types(&mut self, types: &[TypeId]) {
        // `a` then `b`, 4 little-endian bytes each.
        let pair = |a: TypeId, b: TypeId| (u64::from(a.0) | (u64::from(b.0) << 32)).to_le_bytes();
        self.put((types.len() as i32 as i64 as u64).to_le_bytes());
        match *types {
            [] => {}
            [a] => self.put(a.0.to_le_bytes()),
            [a, b] => self.put(pair(a, b)),
            [a, b, c] => {
                self.put(pair(a, b));
                self.put(c.0.to_le_bytes());
            }
            [a, b, c, d] => {
                self.put(pair(a, b));
                self.put(pair(c, d));
            }
            _ => {
                let len = self.len;
                let end = len + 4 * types.len();
                for (d, &t) in self.buf[len..end].chunks_exact_mut(4).zip(types) {
                    d.copy_from_slice(&t.0.to_le_bytes());
                }
                self.len = end;
            }
        }
    }

    /// Same bytes as `KeyBuilder::write_alias`, with the alias given as its
    /// symbol and type arguments.
    #[inline(always)]
    fn write_alias(&mut self, symbols: &SymbolArena, alias: Option<(SymbolId, &[TypeId])>) {
        if let Some((symbol, types)) = alias {
            self.put([1]);
            self.put(get_symbol_id(symbols, symbol).to_le_bytes());
            self.write_types(types);
        } else {
            self.put([0]);
        }
    }

    #[inline(always)]
    fn hash(&self) -> CacheHashKey {
        let (hi, lo) = xxh3_hash128(&self.buf[..self.len]);
        CacheHashKey { hi, lo }
    }
}

// PORT: the Go `get*Key` functions below are package functions. The ones
// that only use type ids (and alias symbols) are free functions; functions
// that write aliases take the symbol arena because `ast.GetSymbolId` does.
// Each also has a `Checker` method with the same name and Go parameters
// (supplying `&self.symbols`), so callers can use either form.

// Go: checker/checker.go:17832 getTypeListKey
// PERF: a list of 1 or 2 types is a 12 or 16 byte key (the count as 8 bytes,
// then 4 bytes for each type). It is built in a stack array and hashed with
// no `KeyBuilder`. Same bytes, so the same key.
pub fn get_type_list_key(types: &[TypeId]) -> CacheHashKey {
    let key = match *types {
        [t] => {
            let mut p = [0u8; 12];
            p[..8].copy_from_slice(&1u64.to_le_bytes());
            p[8..].copy_from_slice(&t.0.to_le_bytes());
            short_key_hash(&p)
        }
        [t0, t1] => {
            let mut p = [0u8; 16];
            p[..8].copy_from_slice(&2u64.to_le_bytes());
            p[8..12].copy_from_slice(&t0.0.to_le_bytes());
            p[12..].copy_from_slice(&t1.0.to_le_bytes());
            short_key_hash(&p)
        }
        _ => return type_list_key_with_builder(types),
    };
    debug_assert_eq!(key, type_list_key_with_builder(types));
    key
}

/// Go `getTypeListKey` through `keyBuilder`.
fn type_list_key_with_builder(types: &[TypeId]) -> CacheHashKey {
    let mut b = KeyBuilder::default();
    b.write_types(types);
    b.hash()
}

// Go: checker/checker.go:17838 getAliasKey
pub fn get_alias_key(symbols: &SymbolArena, alias: Option<&TypeAlias>) -> CacheHashKey {
    let mut b = KeyBuilder::default();
    b.write_alias(symbols, alias);
    b.hash()
}

// Go: checker/checker.go:17868 getIntersectionKey
// PERF: built in a `ShortKey` when the whole key fits in it, else through
// `keyBuilder`. Same bytes, so the same key.
pub fn get_intersection_key(
    symbols: &SymbolArena,
    types: &[TypeId],
    flags: IntersectionFlags,
    alias: Option<&TypeAlias>,
) -> CacheHashKey {
    let alias_parts = alias.map(|a| (a.symbol, a.type_arguments.as_slice()));
    let reduce = !flags.intersects(IntersectionFlags::NO_CONSTRAINT_REDUCTION);
    let tail_len = if reduce {
        alias_key_len(alias_parts)
    } else {
        1
    };
    if type_list_key_len(types.len()) + tail_len > SHORT_KEY_CAP {
        return intersection_key_with_builder(symbols, types, flags, alias);
    }
    let mut b = ShortKey::new();
    b.write_types(types);
    if reduce {
        b.write_alias(symbols, alias_parts);
    } else {
        b.put([b'*']);
    }
    let key = b.hash();
    debug_assert_eq!(
        key,
        intersection_key_with_builder(symbols, types, flags, alias)
    );
    key
}

/// Go `getIntersectionKey` through `keyBuilder`.
fn intersection_key_with_builder(
    symbols: &SymbolArena,
    types: &[TypeId],
    flags: IntersectionFlags,
    alias: Option<&TypeAlias>,
) -> CacheHashKey {
    let mut b = KeyBuilder::default();
    b.write_types(types);
    if !flags.intersects(IntersectionFlags::NO_CONSTRAINT_REDUCTION) {
        b.write_alias(symbols, alias);
    } else {
        b.write_byte(b'*');
    }
    b.hash()
}

// Go: checker/checker.go:17879 getTupleKey
pub fn get_tuple_key(element_infos: &[TupleElementInfo], readonly: bool) -> CacheHashKey {
    let mut b = KeyBuilder::default();
    for e in element_infos {
        if e.flags.intersects(ElementFlags::REQUIRED) {
            b.write_byte(b'#');
        } else if e.flags.intersects(ElementFlags::OPTIONAL) {
            b.write_byte(b'?');
        } else if e.flags.intersects(ElementFlags::REST) {
            b.write_byte(b'.');
        } else {
            b.write_byte(b'*');
        }
        if e.labeled_declaration.is_some() {
            b.write_node(e.labeled_declaration);
        }
    }
    if readonly {
        b.write_byte(b'!');
    }
    b.hash()
}

// Go: checker/checker.go:17902 getTypeAliasInstantiationKey
pub fn get_type_alias_instantiation_key(
    symbols: &SymbolArena,
    type_arguments: &[TypeId],
    alias: Option<&TypeAlias>,
) -> CacheHashKey {
    get_type_instantiation_key(symbols, type_arguments, alias, false)
}

// Go: checker/checker.go:17906 getTypeInstantiationKey
pub fn get_type_instantiation_key(
    symbols: &SymbolArena,
    type_arguments: &[TypeId],
    alias: Option<&TypeAlias>,
    single_signature: bool,
) -> CacheHashKey {
    type_instantiation_key_parts(
        symbols,
        type_arguments,
        alias.map(|a| (a.symbol, a.type_arguments.as_slice())),
        single_signature,
    )
}

/// Go `getTypeInstantiationKey` and `getConditionalTypeKey`, which have the
/// same body: `writeTypes(typeArguments)`, `writeAlias(alias)` and then
/// `'!'` when `flag`. The alias is given as its symbol and type arguments,
/// so a caller can hash an alias that it has not built yet.
/// PERF: built in a `ShortKey` when the whole key fits in it, else through
/// `keyBuilder`. Same bytes, so the same key.
pub fn type_instantiation_key_parts(
    symbols: &SymbolArena,
    type_arguments: &[TypeId],
    alias: Option<(SymbolId, &[TypeId])>,
    flag: bool,
) -> CacheHashKey {
    let len = type_list_key_len(type_arguments.len()) + alias_key_len(alias) + usize::from(flag);
    if len > SHORT_KEY_CAP {
        return type_instantiation_key_with_builder(symbols, type_arguments, alias, flag);
    }
    let mut b = ShortKey::new();
    b.write_types(type_arguments);
    b.write_alias(symbols, alias);
    if flag {
        b.put([b'!']);
    }
    let key = b.hash();
    debug_assert_eq!(
        key,
        type_instantiation_key_with_builder(symbols, type_arguments, alias, flag)
    );
    key
}

/// `type_instantiation_key_parts` through `keyBuilder`.
fn type_instantiation_key_with_builder(
    symbols: &SymbolArena,
    type_arguments: &[TypeId],
    alias: Option<(SymbolId, &[TypeId])>,
    flag: bool,
) -> CacheHashKey {
    let mut b = KeyBuilder::default();
    b.write_types(type_arguments);
    // Go `b.writeAlias(alias)`, from the alias parts.
    if let Some((symbol, alias_type_arguments)) = alias {
        b.write_byte(1);
        b.write_symbol(symbols, symbol);
        b.write_types(alias_type_arguments);
    } else {
        b.write_byte(0);
    }
    if flag {
        b.write_byte(b'!');
    }
    b.hash()
}

// Go: checker/checker.go:17916 getIndexedAccessKey
pub fn get_indexed_access_key(
    symbols: &SymbolArena,
    object_type: TypeId,
    index_type: TypeId,
    access_flags: AccessFlags,
    alias: Option<&TypeAlias>,
) -> CacheHashKey {
    let mut b = KeyBuilder::default();
    b.write_type(object_type);
    b.write_type(index_type);
    b.write_uint32(access_flags.0);
    b.write_alias(symbols, alias);
    b.hash()
}

// Go: checker/checker.go:17925 getTemplateTypeKey
pub fn get_template_type_key(texts: &[String], types: &[TypeId]) -> CacheHashKey {
    let mut b = KeyBuilder::default();
    b.write_types(types);
    b.write_byte(b'|');
    for s in texts {
        b.write_int(s.len() as i32);
    }
    b.write_byte(b'|');
    for s in texts {
        b.write_string(s);
    }
    b.hash()
}

// Go: checker/checker.go:17939 getConditionalTypeKey
pub fn get_conditional_type_key(
    symbols: &SymbolArena,
    type_arguments: &[TypeId],
    alias: Option<&TypeAlias>,
    for_constraint: bool,
) -> CacheHashKey {
    type_instantiation_key_parts(
        symbols,
        type_arguments,
        alias.map(|a| (a.symbol, a.type_arguments.as_slice())),
        for_constraint,
    )
}

// Go: checker/checker.go:17967 getNodeListKey
pub fn get_node_list_key(nodes: &[Node]) -> CacheHashKey {
    let mut b = KeyBuilder::default();
    b.write_int(nodes.len() as i32);
    for &n in nodes {
        b.write_node(n);
    }
    b.hash()
}

impl Checker {
    // Go: checker/checker.go:17832 getTypeListKey
    pub fn get_type_list_key(&self, types: &[TypeId]) -> CacheHashKey {
        get_type_list_key(types)
    }

    // Go: checker/checker.go:17838 getAliasKey
    pub fn get_alias_key(&self, alias: Option<&TypeAlias>) -> CacheHashKey {
        get_alias_key(&self.symbols, alias)
    }

    // Go: checker/checker.go:17844 getUnionKey
    pub fn get_union_key(
        &self,
        types: &[TypeId],
        origin: TypeId,
        alias: Option<&TypeAlias>,
    ) -> CacheHashKey {
        let mut b = KeyBuilder::default();
        if origin.is_nil() {
            b.write_types(types);
        } else {
            let origin_flags = self.ty(origin).flags;
            if origin_flags.intersects(TypeFlags::UNION) {
                b.write_byte(b'|');
                b.write_types(self.ty(origin).types());
            } else if origin_flags.intersects(TypeFlags::INTERSECTION) {
                b.write_byte(b'&');
                b.write_types(self.ty(origin).types());
            } else if origin_flags.intersects(TypeFlags::INDEX) {
                // origin type id alone is insufficient, as `keyof x` may resolve to multiple WIP values while `x` is still resolving
                b.write_byte(b'#');
                b.write_type(origin);
                b.write_byte(b'|');
                b.write_types(types);
            } else {
                panic!("Unhandled case in getUnionKey");
            }
        }
        b.write_alias(&self.symbols, alias);
        b.hash()
    }

    // Go: checker/checker.go:17868 getIntersectionKey
    pub fn get_intersection_key(
        &self,
        types: &[TypeId],
        flags: IntersectionFlags,
        alias: Option<&TypeAlias>,
    ) -> CacheHashKey {
        get_intersection_key(&self.symbols, types, flags, alias)
    }

    // Go: checker/checker.go:17879 getTupleKey
    pub fn get_tuple_key(
        &self,
        element_infos: &[TupleElementInfo],
        readonly: bool,
    ) -> CacheHashKey {
        get_tuple_key(element_infos, readonly)
    }

    // Go: checker/checker.go:17902 getTypeAliasInstantiationKey
    pub fn get_type_alias_instantiation_key(
        &self,
        type_arguments: &[TypeId],
        alias: Option<&TypeAlias>,
    ) -> CacheHashKey {
        get_type_alias_instantiation_key(&self.symbols, type_arguments, alias)
    }

    // Go: checker/checker.go:17906 getTypeInstantiationKey
    pub fn get_type_instantiation_key(
        &self,
        type_arguments: &[TypeId],
        alias: Option<&TypeAlias>,
        single_signature: bool,
    ) -> CacheHashKey {
        get_type_instantiation_key(&self.symbols, type_arguments, alias, single_signature)
    }

    // Go: checker/checker.go:17916 getIndexedAccessKey
    pub fn get_indexed_access_key(
        &self,
        object_type: TypeId,
        index_type: TypeId,
        access_flags: AccessFlags,
        alias: Option<&TypeAlias>,
    ) -> CacheHashKey {
        get_indexed_access_key(&self.symbols, object_type, index_type, access_flags, alias)
    }

    // Go: checker/checker.go:17925 getTemplateTypeKey
    pub fn get_template_type_key(&self, texts: &[String], types: &[TypeId]) -> CacheHashKey {
        get_template_type_key(texts, types)
    }

    // Go: checker/checker.go:17939 getConditionalTypeKey
    pub fn get_conditional_type_key(
        &self,
        type_arguments: &[TypeId],
        alias: Option<&TypeAlias>,
        for_constraint: bool,
    ) -> CacheHashKey {
        get_conditional_type_key(&self.symbols, type_arguments, alias, for_constraint)
    }

    // Go: checker/checker.go:17949 getRelationKey
    // PORT: perf. A plain key is returned as its key bytes, not hashed (see
    // `RelationKey`).
    #[inline]
    pub fn get_relation_key(
        &mut self,
        source: TypeId,
        target: TypeId,
        intersection_state: IntersectionState,
        is_identity: bool,
        ignore_constraints: bool,
    ) -> (RelationKey, bool) {
        let (mut source, mut target) = (source, target);
        if is_identity && source > target {
            std::mem::swap(&mut source, &mut target);
        }
        if self.is_type_reference_with_generic_arguments(source)
            && self.is_type_reference_with_generic_arguments(target)
        {
            return self.generic_relation_key(
                source,
                target,
                intersection_state,
                ignore_constraints,
            );
        }
        (
            RelationKey::Plain(PlainRelationKey {
                source: source.0,
                target: target.0,
                intersection_state: intersection_state.0,
            }),
            false,
        )
    }

    /// The generic branch of Go `getRelationKey`, out of line.
    #[inline(never)]
    fn generic_relation_key(
        &mut self,
        source: TypeId,
        target: TypeId,
        intersection_state: IntersectionState,
        ignore_constraints: bool,
    ) -> (RelationKey, bool) {
        let mut b = KeyBuilder::default();
        b.write_byte(b'g');
        let constrained = b.write_generic_type_references(self, source, target, ignore_constraints);
        b.write_uint32(intersection_state.0);
        (RelationKey::Generic(b.hash()), constrained)
    }

    // Go: checker/checker.go:17967 getNodeListKey
    pub fn get_node_list_key(&self, nodes: &[Node]) -> CacheHashKey {
        get_node_list_key(nodes)
    }

    // Go: checker/checker.go:17976 isTypeReferenceWithGenericArguments
    pub fn is_type_reference_with_generic_arguments(&mut self, t: TypeId) -> bool {
        if !self.is_non_deferred_type_reference(t) {
            return false;
        }
        // PORT: a resolved list is read in place instead of copied. It does
        // not change once set.
        let (count, memo) = {
            let d = self.ty(t).as_type_reference();
            (d.resolved_type_arguments.len(), d.generic_arguments_memo)
        };
        if count == 0 {
            let type_arguments = self.get_type_arguments(t);
            for t in type_arguments {
                if self.ty(t).flags.intersects(TypeFlags::TYPE_PARAMETER)
                    || self.is_type_reference_with_generic_arguments(t)
                {
                    return true;
                }
            }
            return false;
        }
        // PORT: memo, no Go counterpart. A non-deferred reference keeps its
        // non-empty resolved list for good, and the result depends only on
        // that list, so the walk runs once per type. Relation keys ask this
        // on every comparison.
        if memo != GenericArgumentsMemo::Unknown {
            debug_assert_eq!(
                memo == GenericArgumentsMemo::True,
                self.type_arguments_have_generic_arguments(t, count)
            );
            return memo == GenericArgumentsMemo::True;
        }
        let result = self.type_arguments_have_generic_arguments(t, count);
        let memo = if result {
            GenericArgumentsMemo::True
        } else {
            GenericArgumentsMemo::False
        };
        self.ty_mut(t)
            .as_type_reference_mut()
            .generic_arguments_memo = memo;
        result
    }

    /// The walk of `is_type_reference_with_generic_arguments` over the
    /// first `count` resolved type arguments of `t`.
    fn type_arguments_have_generic_arguments(&mut self, t: TypeId, count: usize) -> bool {
        for i in 0..count {
            let a = self.ty(t).as_type_reference().resolved_type_arguments[i];
            if self.ty(a).flags.intersects(TypeFlags::TYPE_PARAMETER)
                || self.is_type_reference_with_generic_arguments(a)
            {
                return true;
            }
        }
        false
    }

    // Go: checker/checker.go:17982 isNonDeferredTypeReference
    pub fn is_non_deferred_type_reference(&self, t: TypeId) -> bool {
        self.ty(t).object_flags.intersects(ObjectFlags::REFERENCE)
            && self.ty(t).as_type_reference().node.is_nil()
    }

    // Return true if type parameter originates in an unconstrained declaration in a type parameter list
    // Go: checker/checker.go:17987 isUnconstrainedTypeParameter
    pub fn is_unconstrained_type_parameter(&self, tp: TypeId) -> bool {
        let mut target = self.ty(tp).target();
        if target.is_nil() {
            target = tp;
        }
        let target_symbol = self.ty(target).symbol;
        if target_symbol.is_nil() {
            return false;
        }
        for &d in &self.sym(target_symbol).declarations {
            if is_type_parameter_declaration(d)
                && (d.constraint().is_some()
                    || is_mapped_type_node(d.parent())
                    || is_infer_type_node(d.parent()))
            {
                return false;
            }
        }
        true
    }

    // Go: checker/checker.go:18003 isNullOrUndefined
    pub fn is_null_or_undefined(&mut self, node: Node) -> bool {
        let expr = skip_parentheses(node);
        match expr.kind() {
            SyntaxKind::NullKeyword => true,
            SyntaxKind::Identifier => self.get_resolved_symbol(expr) == self.undefined_symbol,
            _ => false,
        }
    }

    // Go: checker/checker.go:18014 checkRightHandSideOfForOf
    pub fn check_right_hand_side_of_for_of(&mut self, statement: Node) -> TypeId {
        let use_ = if statement.await_modifier().is_some() {
            IterationUse::FOR_AWAIT_OF
        } else {
            IterationUse::FOR_OF
        };
        let expression_type = self.check_non_null_expression(statement.expression());
        let undefined_type = self.undefined_type;
        self.check_iterated_type_or_element_type(
            use_,
            expression_type,
            undefined_type,
            statement.expression(),
        )
    }

    // Return the inferred type for a binding element
    // Go: checker/checker.go:18020 getTypeForBindingElement
    pub fn get_type_for_binding_element(&mut self, declaration: Node) -> TypeId {
        let check_mode = if has_dot_dot_dot_token(declaration) {
            CheckMode::REST_BINDING_ELEMENT
        } else {
            CheckMode::NORMAL
        };
        let parent_type =
            self.get_type_for_binding_element_parent(declaration.parent().parent(), check_mode);
        if parent_type.is_some() {
            return self.get_binding_element_type_from_parent_type(
                declaration,
                parent_type,
                false, /*noTupleBoundsCheck*/
            );
        }
        TypeId::NIL
    }

    // Return the type of a binding element parent. We check SymbolLinks first to see if a type has been
    // assigned by contextual typing.
    // Go: checker/checker.go:18031 getTypeForBindingElementParent
    pub fn get_type_for_binding_element_parent(
        &mut self,
        node: Node,
        check_mode: CheckMode,
    ) -> TypeId {
        if check_mode == CheckMode::NORMAL {
            // We can use a cached resolved type if no optionality was included in that type.
            let symbol = self.get_symbol_of_declaration(node);
            if symbol.is_some() {
                let resolved_type = self
                    .value_symbol_links
                    .get_by_id(&self.symbols, symbol)
                    .resolved_type;
                if resolved_type.is_some()
                    && !(self.strict_null_checks && is_optional_declaration(node))
                {
                    return resolved_type;
                }
            }
        }
        self.get_type_for_variable_like_declaration(
            node, false, /*includeOptionality*/
            check_mode,
        )
    }
}

#[cfg(test)]
mod key_hash_tests {
    use super::*;

    /// `xxh3_hash128` matches the xxhash-rust `xxh3_128` for every length
    /// path, and `type_key_no_alias` matches the `KeyBuilder` key.
    #[test]
    fn xxh3_and_type_key_match_reference() {
        let mut x = 0x2545_f491_4f6c_dd1du64;
        let mut next = move || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x
        };
        let bytes: Vec<u8> = (0..4096).map(|_| next() as u8).collect();
        for l in 0..bytes.len() {
            let p = &bytes[..l];
            let want = xxhash_rust::xxh3::xxh3_128(p);
            assert_eq!(
                xxh3_hash128(p),
                ((want >> 64) as u64, want as u64),
                "len {l}"
            );
        }
        for _ in 0..100_000 {
            let t = TypeId(next() as u32);
            let mut b = KeyBuilder::default();
            b.write_type(t);
            // `write_alias(None)` writes this byte.
            b.write_byte(0);
            assert_eq!(type_key_no_alias(t), b.hash());
        }
    }

    /// `KeyBuilder` hashes the bytes it was given, in order, for keys that
    /// stay inline and keys that spill past the inline buffer. The short
    /// `get_type_list_key` paths hash the same bytes as the builder.
    /// The test keeps its accepted name from before tsgo#4784 renamed
    /// `KeyHasher` to `KeyBuilder`.
    #[test]
    fn key_hasher_and_short_keys_match_bytes() {
        let mut x = 0x9e37_79b9_7f4a_7c15u64;
        let mut next = move || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x
        };
        for _ in 0..2_000 {
            let mut b = KeyBuilder::default();
            let mut want = Vec::new();
            let writes = next() % 200;
            for _ in 0..writes {
                let v = next();
                match v % 5 {
                    0 => {
                        b.write_byte(v as u8);
                        want.push(v as u8);
                    }
                    1 => {
                        b.write_uint32(v as u32);
                        want.extend((v as u32).to_le_bytes());
                    }
                    2 => {
                        b.write_uint64(v);
                        want.extend(v.to_le_bytes());
                    }
                    3 => {
                        // Short strings, and now and then one longer than the
                        // inline buffer (Go writes it straight to the overflow).
                        let n = if (v >> 52) % 16 == 0 {
                            200 + (v >> 40) as usize % 100
                        } else {
                            (v >> 60) as usize % 9
                        };
                        let s: String = (0..n)
                            .map(|i| (b'a' + ((v >> (i % 60)) % 26) as u8) as char)
                            .collect();
                        b.write_string(&s);
                        want.extend_from_slice(s.as_bytes());
                    }
                    _ => {
                        let types: Vec<TypeId> =
                            (0..(v >> 56) % 80).map(|_| TypeId(next() as u32)).collect();
                        b.write_types(&types);
                        want.extend((types.len() as u64).to_le_bytes());
                        for t in &types {
                            want.extend(t.0.to_le_bytes());
                        }
                    }
                }
            }
            let (hi, lo) = xxh3_hash128(&want);
            assert_eq!(b.hash(), CacheHashKey { hi, lo }, "len {}", want.len());
        }
        for _ in 0..100_000 {
            let (t0, t1) = (TypeId(next() as u32), TypeId(next() as u32));
            assert_eq!(get_type_list_key(&[t0]), type_list_key_with_builder(&[t0]));
            assert_eq!(
                get_type_list_key(&[t0, t1]),
                type_list_key_with_builder(&[t0, t1])
            );
        }
    }
}
