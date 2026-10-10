//! Port of `checker/checker.go` lines 20351-21263 (reportErrorsFromWidening
//! through appendIndexInfo).

use crate::prelude::*;
use smallvec::SmallVec;

impl Checker {
    // Go: checker/checker.go:20789 reportErrorsFromWidening
    pub fn report_errors_from_widening(
        &mut self,
        declaration: Node,
        t: TypeId,
        widening_kind: WideningKind,
    ) {
        if self.no_implicit_any
            && self
                .ty(t)
                .object_flags
                .intersects(ObjectFlags::CONTAINS_WIDENING_TYPE)
        {
            if widening_kind == WideningKind::NORMAL
                || is_function_like_declaration(declaration)
                    && self.should_report_errors_from_widening_with_contextual_signature(
                        declaration,
                        widening_kind,
                    )
            {
                // Report implicit any error within type if possible, otherwise report error on declaration
                if !self.report_widening_errors_in_type(t) {
                    self.report_implicit_any(declaration, t, widening_kind);
                }
            }
        }
    }

    // Go: checker/checker.go:20800 shouldReportErrorsFromWideningWithContextualSignature
    pub fn should_report_errors_from_widening_with_contextual_signature(
        &mut self,
        declaration: Node,
        widening_kind: WideningKind,
    ) -> bool {
        let signature = self.get_contextual_signature_for_function_like_declaration(declaration);
        if signature.is_nil() {
            return true;
        }
        let mut return_type = self.get_return_type_of_signature(signature);
        let flags = get_function_flags(declaration);
        if widening_kind == WideningKind::FUNCTION_RETURN {
            if flags.intersects(FunctionFlags::GENERATOR) {
                let iteration_type = self.get_iteration_type_of_generator_function_return_type(
                    IterationTypeKind::RETURN,
                    return_type,
                    flags.intersects(FunctionFlags::ASYNC),
                );
                if iteration_type.is_some() {
                    return_type = iteration_type;
                }
            } else if flags.intersects(FunctionFlags::ASYNC) {
                let awaited_type = self.get_awaited_type_no_alias(return_type);
                if awaited_type.is_some() {
                    return_type = awaited_type;
                }
            }
            return self.is_generic_type(return_type);
        } else if widening_kind == WideningKind::GENERATOR_YIELD {
            let yield_type = self.get_iteration_type_of_generator_function_return_type(
                IterationTypeKind::YIELD,
                return_type,
                flags.intersects(FunctionFlags::ASYNC),
            );
            return yield_type.is_some() && self.is_generic_type(yield_type);
        } else if widening_kind == WideningKind::GENERATOR_NEXT {
            let next_type = self.get_iteration_type_of_generator_function_return_type(
                IterationTypeKind::NEXT,
                return_type,
                flags.intersects(FunctionFlags::ASYNC),
            );
            return next_type.is_some() && self.is_generic_type(next_type);
        }
        false
    }

    // Reports implicit any errors that occur as a result of widening 'null' and 'undefined'
    // to 'any'. A call to reportWideningErrorsInType is normally accompanied by a call to
    // getWidenedType. But in some cases getWidenedType is called without reporting errors
    // (type argument inference is an example).
    //
    // The return value indicates whether an error was in fact reported. The particular circumstances
    // are on a best effort basis. Currently, if the null or undefined that causes widening is inside
    // an object literal property (arbitrarily deeply), this function reports an error. If no error is
    // reported, reportImplicitAnyError is a suitable fallback to report a general error.
    // Go: checker/checker.go:20834 reportWideningErrorsInType
    pub fn report_widening_errors_in_type(&mut self, t: TypeId) -> bool {
        let mut error_reported = false;
        if self
            .ty(t)
            .object_flags
            .intersects(ObjectFlags::CONTAINS_WIDENING_TYPE)
        {
            if self.ty(t).flags.intersects(TypeFlags::UNION) {
                let types = self.ty(t).types_list();
                if types.iter().any(|&s| self.is_empty_object_type(s)) {
                    error_reported = true;
                } else {
                    for s in types {
                        error_reported = error_reported || self.report_widening_errors_in_type(s);
                    }
                }
            } else if self.is_array_or_tuple_type(t) {
                for s in self.get_type_arguments(t) {
                    error_reported = error_reported || self.report_widening_errors_in_type(s);
                }
            } else if self.is_object_literal_type(t) {
                for p in self.get_properties_of_object_type(t) {
                    let s = self.get_type_of_symbol(p);
                    if self
                        .ty(s)
                        .object_flags
                        .intersects(ObjectFlags::CONTAINS_WIDENING_TYPE)
                    {
                        error_reported = self.report_widening_errors_in_type(s);
                        if !error_reported {
                            // we need to account for property types coming from object literal type normalization in unions
                            let t_value_declaration = self.sym(self.ty(t).symbol).value_declaration;
                            let value_declaration = self
                                .sym(p)
                                .declarations
                                .iter()
                                .copied()
                                .find(|&d| {
                                    let value_declaration = self.sym(d.symbol()).value_declaration;
                                    value_declaration.is_some()
                                        && value_declaration.parent() == t_value_declaration
                                })
                                .unwrap_or(Node::NIL);
                            if value_declaration.is_some() {
                                let prop_string = self.symbol_to_string(p);
                                let widened_type = self.get_widened_type(s);
                                let type_string = self.type_to_string(widened_type);
                                self.error(
                                    value_declaration,
                                    diag::Object_literal_s_property_0_implicitly_has_an_1_type,
                                    args![prop_string, type_string],
                                );
                                error_reported = true;
                            }
                        }
                    }
                }
            }
        }
        error_reported
    }

    // Go: checker/checker.go:20872 getTypePredicateFromBody
    pub fn get_type_predicate_from_body(&mut self, fn_: Node) -> TypePredicateId {
        match fn_.kind() {
            SyntaxKind::Constructor | SyntaxKind::GetAccessor | SyntaxKind::SetAccessor => {
                return TypePredicateId::NIL;
            }
            _ => {}
        }
        let function_flags = get_function_flags(fn_);
        if function_flags != FunctionFlags::NORMAL {
            return TypePredicateId::NIL;
        }
        // Only attempt to infer a type predicate if there's exactly one return.
        let mut single_return = Node::NIL;
        let body = fn_.body();
        if body.is_some() && !is_block(body) {
            // arrow function
            single_return = body;
        } else {
            let bailed_early = for_each_return_statement(body, |return_statement: Node| {
                if single_return.is_some() || return_statement.expression().is_nil() {
                    return true;
                }
                single_return = return_statement.expression();
                false
            });
            if bailed_early || single_return.is_nil() || self.function_has_implicit_return(fn_) {
                return TypePredicateId::NIL;
            }
        }
        self.check_if_expression_refines_any_parameter(fn_, single_return)
    }

    // Go: checker/checker.go:20902 checkIfExpressionRefinesAnyParameter
    pub fn check_if_expression_refines_any_parameter(
        &mut self,
        fn_: Node,
        expr: Node,
    ) -> TypePredicateId {
        let expr = skip_parentheses(expr);
        let return_type = self.check_expression_cached(expr);
        if !self.ty(return_type).flags.intersects(TypeFlags::BOOLEAN) {
            return TypePredicateId::NIL;
        }
        for (i, param) in fn_.parameters().iter().enumerate() {
            let init_type = self.get_type_of_symbol(param.symbol());
            if init_type.is_nil()
                || self.ty(init_type).flags.intersects(TypeFlags::BOOLEAN)
                || !is_identifier(param.name())
                || self.is_symbol_assigned(param.symbol())
                || is_rest_parameter(param)
            {
                // Refining "x: boolean" to "x is true" or "x is false" isn't useful.
                continue;
            }
            let true_type = self.check_if_expression_refines_parameter(fn_, expr, param, init_type);
            if true_type.is_some() {
                return self.new_type_predicate(
                    TypePredicateKind::IDENTIFIER,
                    param.name().text(),
                    i as i32,
                    true_type,
                );
            }
        }
        TypePredicateId::NIL
    }

    // Go: checker/checker.go:20922 checkIfExpressionRefinesParameter
    pub fn check_if_expression_refines_parameter(
        &mut self,
        fn_: Node,
        expr: Node,
        param: Node,
        init_type: TypeId,
    ) -> TypeId {
        let mut antecedent = get_flow_node_of_node(expr);
        if antecedent.is_nil() && is_return_statement(expr.parent()) {
            antecedent = get_flow_node_of_node(expr.parent());
        }
        // PORT: Go allocates synthetic `&ast.FlowNode{...}` values here. Binder
        // flow nodes are read-only `&'static` data, so the checker creates
        // them through `new_synthetic_flow_node` (see contract gaps).
        if antecedent.is_nil() {
            antecedent = self.new_synthetic_flow_node(FlowFlags::START, Node::NIL, FlowNodeId::NIL);
        }
        let true_condition =
            self.new_synthetic_flow_node(FlowFlags::TRUE_CONDITION, expr, antecedent);
        let true_type = self.get_flow_type_of_reference_ex(
            param.name(),
            init_type,
            init_type,
            fn_,
            true_condition,
        );
        if true_type == init_type {
            return TypeId::NIL;
        }
        // "x is T" means that x is T if and only if it returns true. If it returns false then x is not T.
        // This means that if the function is called with an argument of type trueType, there can't be anything left in the `else` branch. It must reduce to `never`.
        let false_condition =
            self.new_synthetic_flow_node(FlowFlags::FALSE_CONDITION, expr, antecedent);
        let false_flow_type = self.get_flow_type_of_reference_ex(
            param.name(),
            init_type,
            true_type,
            fn_,
            false_condition,
        );
        let false_subtype = self.get_reduced_type(false_flow_type);
        if self.ty(false_subtype).flags.intersects(TypeFlags::NEVER) {
            return true_type;
        }
        TypeId::NIL
    }

    // Go: checker/checker.go:20945 addOptionalTypeMarker
    pub fn add_optional_type_marker(&mut self, t: TypeId) -> TypeId {
        if self.strict_null_checks {
            let optional_type = self.optional_type;
            return self.get_union_type(&[t, optional_type]);
        }
        t
    }

    // Go: checker/checker.go:20952 instantiateSignature
    pub fn instantiate_signature(&mut self, sig: SignatureId, m: MapperId) -> SignatureId {
        let erase = m == self.permissive_mapper;
        self.instantiate_signature_ex(sig, m, erase /*eraseTypeParameters*/)
    }

    // Go: checker/checker.go:20956 instantiateSignatureEx
    // PORT: the source lists are read by index instead of cloned (no call
    // below changes the lists of `sig`), and the new lists move into the
    // signature through `new_signature_owned`.
    pub fn instantiate_signature_ex(
        &mut self,
        sig: SignatureId,
        m: MapperId,
        erase_type_parameters: bool,
    ) -> SignatureId {
        let mut m = m;
        let mut fresh_type_parameters: Vec<TypeId> = Vec::new();
        let type_parameter_count = self.sig(sig).type_parameters.len();
        if type_parameter_count != 0 && !erase_type_parameters {
            // First create a fresh set of type parameters, then include a mapping from the old to the
            // new type parameters in the mapper function. Finally store this mapper in the new type
            // parameters such that we can use it when instantiating constraints.
            fresh_type_parameters.reserve_exact(type_parameter_count);
            for i in 0..type_parameter_count {
                let tp = self.sig(sig).type_parameters[i];
                let fresh = self.clone_type_parameter(tp);
                fresh_type_parameters.push(fresh);
            }
            // PORT: `new_type_mapper(sig.type_parameters, fresh)` inlined,
            // so the array form copies the source list straight from the
            // signature into the mapper (the copy `new_array_type_mapper`
            // makes) and the single form copies nothing.
            let fresh_mapper = if type_parameter_count == 1 {
                let source = self.sig(sig).type_parameters[0];
                self.new_simple_type_mapper(source, fresh_type_parameters[0])
            } else {
                let sources: SharedList<TypeId> = self.sig(sig).type_parameters.as_slice().into();
                let targets: SharedList<TypeId> = fresh_type_parameters.as_slice().into();
                self.new_array_type_mapper_shared(sources, targets)
            };
            m = self.combine_type_mappers(fresh_mapper, m);
            for &tp in &fresh_type_parameters {
                self.ty_mut(tp).as_type_parameter_mut().mapper = m;
            }
        }
        // Don't compute resolvedReturnType and resolvedTypePredicate now,
        // because using `mapper` now could trigger inferences to become fixed. (See `createInferenceContext`.)
        // See GH#17600.
        let (sig_flags, sig_declaration, sig_this_parameter, sig_min_argument_count) = {
            let s = self.sig(sig);
            (
                s.flags,
                s.declaration,
                s.this_parameter,
                s.min_argument_count,
            )
        };
        let this_parameter = self.instantiate_symbol(sig_this_parameter, m);
        // PORT: `instantiate_symbols(sig.parameters, m)`: every parameter is
        // instantiated once, in order. Go may return the input slice when
        // nothing changes; the Rust signature owns its list either way.
        let parameter_count = self.sig(sig).parameters.len();
        let mut parameters: Vec<SymbolId> = Vec::with_capacity(parameter_count);
        for i in 0..parameter_count {
            let parameter = self.sig(sig).parameters[i];
            let instantiated = self.instantiate_symbol(parameter, m);
            parameters.push(instantiated);
        }
        let result = self.new_signature_owned(
            sig_flags & SignatureFlags::PROPAGATING_FLAGS,
            sig_declaration,
            fresh_type_parameters,
            this_parameter,
            parameters,
            TypeId::NIL,          /*resolvedReturnType*/
            TypePredicateId::NIL, /*resolvedTypePredicate*/
            sig_min_argument_count,
        );
        let r = self.sig_mut(result);
        r.target = sig;
        r.mapper = m;
        result
    }

    // Go: checker/checker.go:20979 instantiateIndexInfo
    pub fn instantiate_index_info(&mut self, info: IndexInfoId, m: MapperId) -> IndexInfoId {
        let value_type = self.index_info(info).value_type;
        let new_value_type = self.instantiate_type(value_type, m);
        if new_value_type == value_type {
            return info;
        }
        let (key_type, is_readonly, declaration, components) = {
            let i = self.index_info(info);
            (
                i.key_type,
                i.is_readonly,
                i.declaration,
                i.components.clone(),
            )
        };
        self.new_index_info(
            key_type,
            new_value_type,
            is_readonly,
            declaration,
            &components,
        )
    }

    // Go: checker/checker.go:20987 resolveAnonymousTypeMembers
    pub fn resolve_anonymous_type_members(&mut self, t: TypeId) {
        let (d_target, d_mapper) = {
            let d = self.ty(t).as_object_type();
            (d.target, d.mapper)
        };
        if d_target.is_some() {
            self.set_structured_type_members(t, SymbolTable::NIL, &[], &[], &[]);
            let properties = self.get_properties_of_object_type(d_target);
            let members = self.create_instantiated_symbol_table(&properties, d_mapper);
            let call_signatures =
                self.instantiate_signatures_of_type(d_target, SignatureKind::CALL, d_mapper);
            let construct_signatures =
                self.instantiate_signatures_of_type(d_target, SignatureKind::CONSTRUCT, d_mapper);
            let target_index_infos = self.get_index_infos_of_type(d_target);
            let index_infos = self.instantiate_index_infos(&target_index_infos, d_mapper);
            self.set_structured_type_members(
                t,
                members,
                &call_signatures,
                &construct_signatures,
                &index_infos,
            );
            return;
        }
        let t_symbol = self.ty(t).symbol;
        let symbol = self.get_merged_symbol(t_symbol);
        if self.sym(symbol).flags.intersects(SymbolFlags::TYPE_LITERAL) {
            self.set_structured_type_members(t, SymbolTable::NIL, &[], &[], &[]);
            let members = self.get_members_of_symbol(symbol);
            let call_symbol = self.symbols.get(members, INTERNAL_SYMBOL_NAME_CALL);
            let call_signatures = self.get_signatures_of_symbol(call_symbol);
            let new_symbol = self.symbols.get(members, INTERNAL_SYMBOL_NAME_NEW);
            let construct_signatures = self.get_signatures_of_symbol(new_symbol);
            let index_infos = self.get_index_infos_of_symbol(symbol);
            self.set_structured_type_members(
                t,
                members,
                &call_signatures,
                &construct_signatures,
                &index_infos,
            );
            return;
        }
        // Combinations of function, class, enum and module
        let mut members = self.get_exports_of_symbol(symbol);
        let mut index_infos: Vec<IndexInfoId> = Vec::new();
        if symbol == self.global_this_symbol {
            let vars_only = self.symbols.new_table();
            for p in self.symbols.values(members) {
                let (p_flags, p_name, p_declarations) = {
                    let ps = self.sym(p);
                    (ps.flags, ps.name.clone(), ps.declarations.clone())
                };
                if !p_flags.intersects(SymbolFlags::BLOCK_SCOPED)
                    && !(p_flags.intersects(SymbolFlags::VALUE_MODULE)
                        && !p_declarations.is_empty()
                        && p_declarations.iter().all(|&d| is_ambient_module(d)))
                {
                    self.symbols.set(vars_only, p_name, p);
                }
            }
            members = vars_only;
        }
        let mut base_constructor_index_info = IndexInfoId::NIL;
        self.set_structured_type_members(t, members, &[], &[], &[]);
        if self.sym(symbol).flags.intersects(SymbolFlags::CLASS) {
            let class_type = self.get_declared_type_of_class_or_interface(symbol);
            let base_constructor_type = self.get_base_constructor_type_of_class(class_type);
            if self
                .ty(base_constructor_type)
                .flags
                .intersects(TypeFlags::OBJECT | TypeFlags::INTERSECTION | TypeFlags::TYPE_VARIABLE)
            {
                // PORT: Go `maps.Clone(members)` (nil stays nil).
                members = self.symbols.clone_table(members);
                let base_properties = self.get_properties_of_type(base_constructor_type);
                self.add_inherited_members(members, &base_properties);
                self.set_structured_type_members(t, members, &[], &[], &[]);
            } else if base_constructor_type == self.any_type {
                base_constructor_index_info = self.any_base_type_index_info;
            }
        }
        let index_symbol = self.symbols.get(members, INTERNAL_SYMBOL_NAME_INDEX);
        if index_symbol.is_some() {
            let sibling_symbols = self.symbols.values(members);
            index_infos = self.get_index_infos_of_index_symbol(index_symbol, &sibling_symbols);
        } else {
            if base_constructor_index_info.is_some() {
                index_infos.push(base_constructor_index_info);
            }
            if self.sym(symbol).flags.intersects(SymbolFlags::ENUM) {
                let declared_type = self.get_declared_type_of_symbol(symbol);
                let is_enum = self.ty(declared_type).flags.intersects(TypeFlags::ENUM) || {
                    let properties = self.ty(t).as_object_type().structured.properties.clone();
                    properties.iter().any(|&prop| {
                        let prop_type = self.get_type_of_symbol(prop);
                        self.ty(prop_type).flags.intersects(TypeFlags::NUMBER_LIKE)
                    })
                };
                if is_enum {
                    index_infos.push(self.enum_number_index_info);
                }
            }
        }
        {
            let d = &mut self.ty_mut(t).as_object_type_mut().structured;
            let (signatures, call_signature_count) =
                (d.signatures_list(), d.call_signature_count());
            d.set_signatures(signatures, call_signature_count, index_infos.into());
        }
        // We resolve the members before computing the signatures because a signature may use
        // typeof with a qualified name expression that circularly references the type we are
        // in the process of resolving (see issue #6072). The temporarily empty signature list
        // will never be observed because a qualified name can't reference signatures.
        if self
            .sym(symbol)
            .flags
            .intersects(SymbolFlags::FUNCTION | SymbolFlags::METHOD)
        {
            let signatures = self.get_signatures_of_symbol(symbol);
            let d = &mut self.ty_mut(t).as_object_type_mut().structured;
            let call_signature_count = signatures.len() as i32;
            let index_infos = d.index_infos_list();
            d.set_signatures(signatures.into(), call_signature_count, index_infos);
        }
        // And likewise for construct signatures for classes
        if self.sym(symbol).flags.intersects(SymbolFlags::CLASS) {
            let class_type = self.get_declared_type_of_class_or_interface(symbol);
            let symbol_members = self.sym(symbol).members;
            let constructor_symbol = self
                .symbols
                .get(symbol_members, INTERNAL_SYMBOL_NAME_CONSTRUCTOR);
            let mut construct_signatures = self.get_signatures_of_symbol(constructor_symbol);
            if construct_signatures.is_empty() {
                construct_signatures = self.get_default_construct_signatures(class_type);
            }
            let d = &mut self.ty_mut(t).as_object_type_mut().structured;
            let mut signatures = d.signatures().to_vec();
            signatures.extend(construct_signatures);
            let (call_signature_count, index_infos) =
                (d.call_signature_count(), d.index_infos_list());
            d.set_signatures(signatures.into(), call_signature_count, index_infos);
        }
    }

    // Go: checker/checker.go:21066 createInstantiatedSymbolTable
    pub fn create_instantiated_symbol_table(
        &mut self,
        symbols: &[SymbolId],
        m: MapperId,
    ) -> SymbolTable {
        if symbols.is_empty() {
            return SymbolTable::NIL;
        }
        let result = self.symbols.new_table_with_capacity(symbols.len());
        for &symbol in symbols {
            let name = self.sym(symbol).name.clone();
            let instantiated = self.instantiate_symbol(symbol, m);
            self.symbols.set(result, name, instantiated);
        }
        result
    }

    // Go: checker/checker.go:21077 instantiateSymbolTable
    pub fn instantiate_symbol_table(&mut self, symbols: SymbolTable, m: MapperId) -> SymbolTable {
        if self.symbols.len(symbols) == 0 {
            return SymbolTable::NIL;
        }
        let result = self
            .symbols
            .new_table_with_capacity(self.symbols.len(symbols));
        // PERF: the snapshot of the table (the loop calls `&mut self`
        // methods) goes on the stack when it is small, in place of the
        // `entries()` Vec. Same entries in the same order.
        let entries: SmallVec<[(Name, SymbolId); 16]> = self.symbols.iter_names(symbols).collect();
        for (id, symbol) in entries {
            if self.is_named_member(symbol, &id) {
                let instantiated = self.instantiate_symbol(symbol, m);
                self.symbols.set(result, id, instantiated);
            }
        }
        result
    }

    // Go: checker/checker.go:21090 instantiateSymbol
    // PORT: #64475 splits Go's body into isSymbolUnaffectedByInstantiation
    // and newInstantiatedSymbol (lazy_members.rs), each with its own links
    // read. Both reads are of `symbol`, so the first one gives the id. Here
    // the links are read once and both halves take them.
    pub fn instantiate_symbol(&mut self, symbol: SymbolId, m: MapperId) -> SymbolId {
        if symbol.is_nil() {
            return SymbolId::NIL;
        }
        let (links_resolved_type, links_write_type, links_target, links_mapper, links_name_type) = {
            let links = self.value_symbol_links.get_by_id(&self.symbols, symbol);
            (
                links.resolved_type,
                links.write_type,
                links.target,
                links.mapper,
                links.name_type,
            )
        };
        if self.is_symbol_unaffected_by_instantiation_with(
            symbol,
            m,
            links_resolved_type,
            links_write_type,
        ) {
            return symbol;
        }
        self.new_instantiated_symbol_with(symbol, m, links_target, links_mapper, links_name_type)
    }

    /// Go `isSymbolUnaffectedByInstantiation` (#64475) after its links read:
    /// `resolved_type` and `write_type` are the links of `symbol`.
    #[inline]
    pub(crate) fn is_symbol_unaffected_by_instantiation_with(
        &mut self,
        symbol: SymbolId,
        m: MapperId,
        links_resolved_type: TypeId,
        links_write_type: TypeId,
    ) -> bool {
        if m.is_some() && self.mapper(m).maps_this_only() && self.is_thisless(symbol) {
            return true;
        }
        // If the type of the symbol is already resolved, and if that type could not possibly
        // be affected by instantiation, simply return the symbol itself.
        if links_resolved_type.is_some() && !self.could_contain_type_variables(links_resolved_type)
        {
            if !self.sym(symbol).flags.intersects(SymbolFlags::SET_ACCESSOR) {
                return true;
            }
            // If we're a setter, check writeType.
            if links_write_type.is_some() && !self.could_contain_type_variables(links_write_type) {
                return true;
            }
        }
        false
    }

    /// Go `newInstantiatedSymbol` (#64475) after its links read: `links_*`
    /// are the links of `symbol`.
    #[inline]
    pub(crate) fn new_instantiated_symbol_with(
        &mut self,
        symbol: SymbolId,
        m: MapperId,
        links_target: SymbolId,
        links_mapper: MapperId,
        links_name_type: TypeId,
    ) -> SymbolId {
        let mut symbol = symbol;
        let mut m = m;
        if self
            .sym(symbol)
            .check_flags
            .intersects(CheckFlags::INSTANTIATED)
        {
            // If symbol being instantiated is itself a instantiation, fetch the original target and combine the
            // type mappers. This ensures that original type identities are properly preserved and that aliases
            // always reference a non-aliases.
            symbol = links_target;
            m = self.combine_type_mappers(links_mapper, m);
        }
        // Keep the flags from the symbol we're instantiating.  Mark that is instantiated, and
        // also transient so that we can just store data on it directly.
        // PORT: Go calls newSymbol and then sets the fields. This builds the
        // full symbol and pushes it once; `symbol_count` and TRANSIENT match
        // `Checker::new_symbol`.
        let full = {
            let s = self.sym(symbol);
            Symbol {
                flags: s.flags | SymbolFlags::TRANSIENT,
                name: s.name.clone(),
                check_flags: CheckFlags::INSTANTIATED
                    | s.check_flags
                        & (CheckFlags::READONLY
                            | CheckFlags::LATE
                            | CheckFlags::OPTIONAL_PARAMETER
                            | CheckFlags::REST_PARAMETER),
                declarations: s.declarations.clone(),
                parent: s.parent,
                value_declaration: s.value_declaration,
                ..Symbol::default()
            }
        };
        self.symbol_count += 1;
        let result = self.symbols.push_symbol(full);
        // PERF: Go `Get` and then sets three fields. `result` is new, so its
        // record is added once with every field set.
        self.value_symbol_links.insert_new_by_id(
            &self.symbols,
            result,
            ValueSymbolLinks {
                target: symbol,
                mapper: m,
                name_type: links_name_type,
                ..ValueSymbolLinks::default()
            },
        );
        result
    }

    // Returns true if the parameter or class/interface member given by the symbol is free of "this" references. The
    // function may return false for symbols that are actually free of "this" references because it is not
    // feasible to perform a complete analysis in all cases. In particular, property members with types
    // inferred from their initializers and function members with inferred return types are conservatively
    // assumed not to be free of "this" references.
    // Go: checker/checker.go:21135 isThisless
    pub fn is_thisless(&self, symbol: SymbolId) -> bool {
        let declarations = &self.sym(symbol).declarations;
        if declarations.len() == 1 {
            let declaration = declarations[0];
            if declaration.is_some() {
                match declaration.kind() {
                    SyntaxKind::Parameter => {
                        return is_thisless_variable_like_declaration(declaration);
                    }
                    SyntaxKind::PropertyDeclaration | SyntaxKind::PropertySignature => {
                        return is_thisless_variable_like_declaration(declaration);
                    }
                    SyntaxKind::MethodDeclaration
                    | SyntaxKind::MethodSignature
                    | SyntaxKind::Constructor
                    | SyntaxKind::GetAccessor
                    | SyntaxKind::SetAccessor => {
                        return is_thisless_function_like_declaration(declaration);
                    }
                    _ => {}
                }
            }
        }
        false
    }
}

// A variable-like declaration is free of this references if it has a type annotation
// that is thisless, or if it has no type annotation and no initializer (and is thus of type any).
// Go: checker/checker.go:21154 isThislessVariableLikeDeclaration
pub fn is_thisless_variable_like_declaration(node: Node) -> bool {
    let type_node = node.type_();
    if type_node.is_some() {
        return is_thisless_type(type_node);
    }
    node.initializer().is_nil()
}

// A type is free of this references if it's the any, string, number, boolean, symbol, or void keyword, a string
// literal type, an array with an element type that is free of this references, or a type reference that is
// free of this references.
// Go: checker/checker.go:21165 isThislessType
pub fn is_thisless_type(node: Node) -> bool {
    match node.kind() {
        SyntaxKind::AnyKeyword
        | SyntaxKind::UnknownKeyword
        | SyntaxKind::StringKeyword
        | SyntaxKind::NumberKeyword
        | SyntaxKind::BigIntKeyword
        | SyntaxKind::BooleanKeyword
        | SyntaxKind::SymbolKeyword
        | SyntaxKind::ObjectKeyword
        | SyntaxKind::VoidKeyword
        | SyntaxKind::UndefinedKeyword
        | SyntaxKind::NeverKeyword
        | SyntaxKind::LiteralType => true,
        SyntaxKind::ArrayType => is_thisless_type(node.element_type()),
        SyntaxKind::TypeReference => node.type_arguments().iter().all(is_thisless_type),
        _ => false,
    }
}

// A function-like declaration is considered free of `this` references if it has a return type
// annotation that is free of this references and if each parameter is thisless and if
// each type parameter (if present) is thisless.
// Go: checker/checker.go:21181 isThislessFunctionLikeDeclaration
pub fn is_thisless_function_like_declaration(node: Node) -> bool {
    let return_type = node.type_();
    (is_constructor_declaration(node) || return_type.is_some() && is_thisless_type(return_type))
        && node
            .parameters()
            .iter()
            .all(is_thisless_variable_like_declaration)
        && node
            .type_parameters()
            .iter()
            .all(is_thisless_type_parameter)
}

// A type parameter is thisless if its constraint is thisless, or if it has no constraint. */
// Go: checker/checker.go:21189 isThislessTypeParameter
pub fn is_thisless_type_parameter(node: Node) -> bool {
    let constraint = node.constraint();
    constraint.is_nil() || is_thisless_type(constraint)
}

impl Checker {
    // Go: checker/checker.go:21194 getDefaultConstructSignatures
    pub fn get_default_construct_signatures(&mut self, class_type: TypeId) -> Vec<SignatureId> {
        let base_constructor_type = self.get_base_constructor_type_of_class(class_type);
        let base_signatures =
            self.get_signatures_of_type(base_constructor_type, SignatureKind::CONSTRUCT);
        let class_symbol = self.ty(class_type).symbol;
        let declaration = get_class_like_declaration_of_symbol(&self.symbols, class_symbol);
        let is_abstract =
            declaration.is_some() && has_syntactic_modifier(declaration, ModifierFlags::ABSTRACT);
        if base_signatures.is_empty() {
            let flags = if is_abstract {
                SignatureFlags::CONSTRUCT | SignatureFlags::ABSTRACT
            } else {
                SignatureFlags::CONSTRUCT
            };
            let local_type_parameters = self
                .ty(class_type)
                .as_interface_type()
                .local_type_parameters()
                .to_vec();
            let sig = self.new_signature(
                flags,
                Node::NIL,
                &local_type_parameters,
                SymbolId::NIL,
                &[],
                class_type,
                TypePredicateId::NIL,
                0,
            );
            let origin = self.class_type_parameters_origin(class_type);
            self.sig_mut(sig).type_parameters_origin = origin;
            return vec![sig];
        }
        let base_type_node = self.get_base_type_node_of_class(class_type);
        let is_java_script = declaration.is_some() && is_in_js_file(declaration);
        let type_arguments = self.get_type_arguments_from_node(base_type_node);
        let type_arg_count = type_arguments.len() as i32;
        let mut result: Vec<SignatureId> = Vec::new();
        for base_sig in base_signatures {
            let base_sig_type_parameters = self.sig(base_sig).type_parameters.clone();
            let min_type_argument_count =
                self.get_min_type_argument_count(&base_sig_type_parameters);
            let type_param_count = base_sig_type_parameters.len() as i32;
            if is_java_script
                || type_arg_count >= min_type_argument_count && type_arg_count <= type_param_count
            {
                let sig = if type_param_count != 0 {
                    let filled = self.fill_missing_type_arguments(
                        &type_arguments,
                        &base_sig_type_parameters,
                        min_type_argument_count,
                        is_java_script,
                    );
                    self.create_signature_instantiation(base_sig, &filled)
                } else {
                    self.clone_signature(base_sig)
                };
                let local_type_parameters = self
                    .ty(class_type)
                    .as_interface_type()
                    .local_type_parameters()
                    .to_vec();
                let origin = self.class_type_parameters_origin(class_type);
                let s = self.sig_mut(sig);
                s.type_parameters = local_type_parameters;
                s.type_parameters_origin = origin;
                s.resolved_return_type = class_type;
                if is_abstract {
                    s.flags |= SignatureFlags::ABSTRACT;
                } else {
                    s.flags = s.flags.without(SignatureFlags::ABSTRACT);
                }
                result.push(sig);
            }
        }
        result
    }

    /// `get_property_name_from_type` as an interned `Name`. String literal
    /// and unique symbol names are interned from the type with no `String`.
    ///
    /// PORT: a string or number literal type keeps its name after the first
    /// call, so each mapped member does not intern the text again or format
    /// the number again. The literal value never changes.
    pub fn get_property_name_from_type_as_name(&self, t: TypeId) -> Name {
        let ty = self.ty(t);
        if ty
            .flags
            .intersects(TypeFlags::STRING_LITERAL | TypeFlags::NUMBER_LITERAL)
        {
            let literal = ty.as_literal_type();
            return literal
                .property_name
                .get_or_init(|| match literal.value.as_ref() {
                    Some(LiteralValue::String(s))
                        if ty.flags.intersects(TypeFlags::STRING_LITERAL) =>
                    {
                        Name::from(s.as_str())
                    }
                    _ => Name::from(self.get_property_name_from_type(t)),
                })
                .clone();
        }
        if ty.flags.intersects(TypeFlags::UNIQUE_ES_SYMBOL) {
            return Name::from(ty.as_unique_es_symbol_type().name.as_str());
        }
        Name::from(self.get_property_name_from_type(t).as_str())
    }

    // Go: checker/checker.go:21231 resolveMappedTypeMembers
    pub fn resolve_mapped_type_members(&mut self, t: TypeId) {
        let members = self.symbols.new_table();
        let mut index_infos: Vec<IndexInfoId> = Vec::new();
        // Resolve upfront such that recursive references see an empty object type.
        self.set_structured_type_members(t, SymbolTable::NIL, &[], &[], &[]);
        // In { [P in K]: T }, we refer to P as the type parameter type, K as the constraint type,
        // and T as the template type.
        let type_parameter = self.get_type_parameter_from_mapped_type(t);
        let constraint_type = self.get_constraint_type_from_mapped_type(t);
        let mapped_type = {
            let target = self.ty(t).as_mapped_type().object.target;
            if target.is_some() { target } else { t }
        };
        let name_type = self.get_name_type_from_mapped_type(mapped_type);
        let should_link_prop_declarations =
            self.get_mapped_type_name_type_kind(mapped_type) != MappedTypeNameTypeKind::REMAPPING;
        let template_type = self.get_template_type_from_mapped_type(mapped_type);
        let modifiers_type_raw = self.get_modifiers_type_from_mapped_type(t);
        let modifiers_type = self.get_apparent_type(modifiers_type_raw);
        // The 'T' in 'keyof T'
        let template_modifiers = self.get_mapped_type_modifiers(t);
        let include = TypeFlags::STRING_OR_NUMBER_LITERAL_OR_UNIQUE;
        {
            // PORT: Go closure `addMemberForKeyTypeWorker`. It writes `members` (a
            // table handle) and `indexInfos` (captured by `&mut`).
            let mut add_member_for_key_type_worker =
                |c: &mut Checker, key_type: TypeId, prop_name_type: TypeId| {
                    // If the current iteration type constituent is a string literal type, create a property.
                    // Otherwise, for type string create a string index signature.
                    if c.is_type_usable_as_property_name(prop_name_type) {
                        let prop_name = c.get_property_name_from_type_as_name(prop_name_type);
                        // String enum members from separate enums with identical values
                        // are distinct types with the same property name. Make the resulting
                        // property symbol's name type be the union of those enum member types.
                        let existing_prop = c.symbols.get(members, &prop_name);
                        if existing_prop.is_some() {
                            let existing_name_type = c
                                .value_symbol_links
                                .get_by_id(&c.symbols, existing_prop)
                                .name_type;
                            let name_type_union =
                                c.get_union_type(&[existing_name_type, prop_name_type]);
                            c.value_symbol_links
                                .get_by_id(&c.symbols, existing_prop)
                                .name_type = name_type_union;
                            let existing_key_type =
                                c.mapped_symbol_links.get(existing_prop).key_type;
                            let key_type_union = c.get_union_type(&[existing_key_type, key_type]);
                            c.mapped_symbol_links.get(existing_prop).key_type = key_type_union;
                        } else {
                            let mut modifiers_prop = SymbolId::NIL;
                            if c.is_type_usable_as_property_name(key_type) {
                                let key_name = c.get_property_name_from_type_as_name(key_type);
                                modifiers_prop = c.get_property_of_type(modifiers_type, &key_name);
                            }
                            let is_optional = template_modifiers
                                .intersects(MappedTypeModifiers::INCLUDE_OPTIONAL)
                                || !template_modifiers
                                    .intersects(MappedTypeModifiers::EXCLUDE_OPTIONAL)
                                    && modifiers_prop.is_some()
                                    && c.sym(modifiers_prop)
                                        .flags
                                        .intersects(SymbolFlags::OPTIONAL);
                            let is_readonly = template_modifiers
                                .intersects(MappedTypeModifiers::INCLUDE_READONLY)
                                || !template_modifiers
                                    .intersects(MappedTypeModifiers::EXCLUDE_READONLY)
                                    && modifiers_prop.is_some()
                                    && c.is_readonly_symbol(modifiers_prop);
                            let strip_optional = c.strict_null_checks
                                && !is_optional
                                && modifiers_prop.is_some()
                                && c.sym(modifiers_prop)
                                    .flags
                                    .intersects(SymbolFlags::OPTIONAL);
                            let mut late_flag = CheckFlags::NONE;
                            if modifiers_prop.is_some() {
                                late_flag = c.sym(modifiers_prop).check_flags & CheckFlags::LATE;
                            }
                            let prop = c.new_symbol(
                                SymbolFlags::PROPERTY
                                    | if is_optional {
                                        SymbolFlags::OPTIONAL
                                    } else {
                                        SymbolFlags::NONE
                                    },
                                // PERF: the `Name`, not its text, so the name
                                // is not interned a second time.
                                prop_name.clone(),
                            );
                            c.sym_mut(prop).check_flags = late_flag
                                | CheckFlags::MAPPED
                                | if is_readonly {
                                    CheckFlags::READONLY
                                } else {
                                    CheckFlags::NONE
                                }
                                | if strip_optional {
                                    CheckFlags::STRIP_OPTIONAL
                                } else {
                                    CheckFlags::NONE
                                };
                            // PERF: Go `Get` and then sets the fields. `prop`
                            // is new, so each record is added once with its
                            // fields set. A nil `modifiers_prop` is the
                            // default (nil) origin.
                            c.value_symbol_links.insert_new_by_id(
                                &c.symbols,
                                prop,
                                ValueSymbolLinks {
                                    containing_type: t,
                                    name_type: prop_name_type,
                                    ..ValueSymbolLinks::default()
                                },
                            );
                            c.mapped_symbol_links.insert_new(
                                prop,
                                MappedSymbolLinks {
                                    key_type,
                                    synthetic_origin: modifiers_prop,
                                },
                            );
                            if modifiers_prop.is_some() && should_link_prop_declarations {
                                let declarations = c.sym(modifiers_prop).declarations.clone();
                                c.sym_mut(prop).declarations = declarations;
                            }
                            c.symbols.set(members, prop_name, prop);
                        }
                    } else if c.is_valid_index_key_type(prop_name_type)
                        || c.ty(prop_name_type)
                            .flags
                            .intersects(TypeFlags::ANY | TypeFlags::ENUM)
                    {
                        let mut index_key_type = prop_name_type;
                        let prop_name_flags = c.ty(prop_name_type).flags;
                        if prop_name_flags.intersects(TypeFlags::ANY | TypeFlags::STRING) {
                            index_key_type = c.string_type;
                        } else if prop_name_flags.intersects(TypeFlags::NUMBER | TypeFlags::ENUM) {
                            index_key_type = c.number_type;
                        }
                        let t_mapper = c.ty(t).as_mapped_type().object.mapper;
                        let prop_mapper = c.append_type_mapping(t_mapper, type_parameter, key_type);
                        let prop_type = c.instantiate_type(template_type, prop_mapper);
                        let modifiers_index_info =
                            c.get_applicable_index_info(modifiers_type, prop_name_type);
                        let is_readonly = template_modifiers
                            .intersects(MappedTypeModifiers::INCLUDE_READONLY)
                            || !template_modifiers
                                .intersects(MappedTypeModifiers::EXCLUDE_READONLY)
                                && modifiers_index_info.is_some()
                                && c.index_info(modifiers_index_info).is_readonly;
                        let index_info = c.new_index_info(
                            index_key_type,
                            prop_type,
                            is_readonly,
                            Node::NIL,
                            &[],
                        );
                        let current = std::mem::take(&mut index_infos);
                        index_infos = c.append_index_info(current, index_info, true /*union*/);
                    }
                };
            let mut add_member_for_key_type = |c: &mut Checker, key_type: TypeId| {
                let mut prop_name_type = key_type;
                if name_type.is_some() {
                    let t_mapper = c.ty(t).as_mapped_type().object.mapper;
                    let name_mapper = c.append_type_mapping(t_mapper, type_parameter, key_type);
                    prop_name_type = c.instantiate_type(name_type, name_mapper);
                }
                c.for_each_type(prop_name_type, &mut |c: &mut Checker, u: TypeId| {
                    add_member_for_key_type_worker(c, key_type, u);
                });
            };
            if self.is_mapped_type_with_keyof_constraint_declaration(t) {
                // We have a { [P in keyof T]: X }
                self.for_each_mapped_type_property_key_type_and_index_signature_key_type(
                    modifiers_type,
                    include,
                    false, /*stringsOnly*/
                    &mut add_member_for_key_type,
                );
            } else {
                let lower_bound = self.get_lower_bound_of_key_type(constraint_type);
                self.for_each_type(lower_bound, &mut add_member_for_key_type);
            }
        }
        self.set_structured_type_members(t, members, &[], &[], &index_infos);
    }

    // Go: checker/checker.go:21321 getTypeOfMappedSymbol
    pub fn get_type_of_mapped_symbol(&mut self, symbol: SymbolId) -> TypeId {
        if self
            .value_symbol_links
            .get_by_id(&self.symbols, symbol)
            .resolved_type
            .is_nil()
        {
            let mapped_type = self
                .value_symbol_links
                .get_by_id(&self.symbols, symbol)
                .containing_type;
            let pushed = self.push_type_resolution(
                TypeSystemEntity::Symbol(symbol),
                TypeSystemPropertyName::TYPE,
            );
            if mapped_type.is_nil() {
                // Go reads `mappedType.AsMappedType()` on both branches: a
                // symbol of another checker has no links here.
                go_nil_dereference();
            }
            if !pushed {
                self.ty_mut(mapped_type).as_mapped_type_mut().contains_error = true;
                return self.error_type;
            }
            let mapped_type_target = self.ty(mapped_type).as_mapped_type().object.target;
            let template_type =
                self.get_template_type_from_mapped_type(if mapped_type_target.is_some() {
                    mapped_type_target
                } else {
                    mapped_type
                });
            let mapped_type_mapper = self.ty(mapped_type).as_mapped_type().object.mapper;
            let type_parameter = self.get_type_parameter_from_mapped_type(mapped_type);
            let key_type = self.mapped_symbol_links.get(symbol).key_type;
            let mapper = self.append_type_mapping(mapped_type_mapper, type_parameter, key_type);
            let mut prop_type = self.instantiate_type(template_type, mapper);
            // When creating an optional property in strictNullChecks mode, if 'undefined' isn't assignable to the
            // type, we include 'undefined' in the type. Similarly, when creating a non-optional property in strictNullChecks
            // mode, if the underlying property is optional we remove 'undefined' from the type.
            if self.strict_null_checks
                && self.sym(symbol).flags.intersects(SymbolFlags::OPTIONAL)
                && !self.maybe_type_of_kind(prop_type, TypeFlags::UNDEFINED | TypeFlags::VOID)
            {
                prop_type = self.get_optional_type(prop_type, true /*isProperty*/);
            } else if self
                .sym(symbol)
                .check_flags
                .intersects(CheckFlags::STRIP_OPTIONAL)
            {
                prop_type = self.remove_missing_or_undefined_type(prop_type);
            }
            if self.pop_type_resolution() {
                let links = self.value_symbol_links.get_by_id(&self.symbols, symbol);
                if links.resolved_type.is_nil() {
                    links.resolved_type = prop_type;
                }
            } else {
                let error_type = self.error_type;
                let links = self.value_symbol_links.get_by_id(&self.symbols, symbol);
                if links.resolved_type.is_nil() {
                    links.resolved_type = error_type;
                }
                let current_node = self.current_node;
                let symbol_string = self.symbol_to_string(symbol);
                let type_string = self.type_to_string(mapped_type);
                self.error(
                    current_node,
                    diag::Type_of_property_0_circularly_references_itself_in_mapped_type_1,
                    args![symbol_string, type_string],
                );
            }
        }
        self.value_symbol_links
            .get_by_id(&self.symbols, symbol)
            .resolved_type
    }

    // Return the lower bound of the key type in a mapped type. Intuitively, the lower
    // bound includes those keys that are known to always be present, for example because
    // because of constraints on type parameters (e.g. 'keyof T' for a constrained T).
    // Go: checker/checker.go:21358 getLowerBoundOfKeyType
    pub fn get_lower_bound_of_key_type(&mut self, t: TypeId) -> TypeId {
        let flags = self.ty(t).flags;
        if flags.intersects(TypeFlags::INDEX) {
            let index_target = self.ty(t).as_index_type().target;
            let t = self.get_apparent_type(index_target);
            if self.is_generic_tuple_type(t) {
                return self.get_known_keys_of_tuple_type(t);
            }
            return self.get_index_type(t);
        } else if flags.intersects(TypeFlags::CONDITIONAL) {
            let (is_distributive, root_check_type) = {
                let root = self.ty(t).as_conditional_type().root.borrow();
                (root.is_distributive, root.check_type)
            };
            if is_distributive {
                let check_type = self.ty(t).as_conditional_type().check_type;
                let constraint = self.get_lower_bound_of_key_type(check_type);
                if constraint != check_type {
                    let t_mapper = self.ty(t).as_conditional_type().mapper;
                    let mapper = self.prepend_type_mapping(root_check_type, constraint, t_mapper);
                    return self.get_conditional_type_instantiation(
                        t, mapper, false, /*forConstraint*/
                        None,
                    );
                }
            }
            return t;
        } else if flags.intersects(TypeFlags::UNION) {
            return self.map_type_ex(
                t,
                &mut |c: &mut Checker, t: TypeId| c.get_lower_bound_of_key_type(t),
                true, /*noReductions*/
            );
        } else if flags.intersects(TypeFlags::INTERSECTION) {
            // Similarly to getTypeFromIntersectionTypeNode, we preserve the special string & {}, number & {},
            // and bigint & {} intersections that are used to prevent subtype reduction in union types.
            let types = self.ty(t).types_list();
            if types.len() == 2
                && self
                    .ty(types[0])
                    .flags
                    .intersects(TypeFlags::STRING | TypeFlags::NUMBER | TypeFlags::BIG_INT)
                && types[1] == self.empty_type_literal_type
            {
                return t;
            }
            let mapped: Vec<TypeId> = types
                .iter()
                .map(|&u| self.get_lower_bound_of_key_type(u))
                .collect();
            return self.get_intersection_type(&mapped);
        }
        t
    }

    // Go: checker/checker.go:21389 resolveUnionTypeMembers
    pub fn resolve_union_type_members(&mut self, t: TypeId) {
        // The members and properties collections are empty for union types. To get all properties of a union
        // type use getPropertiesOfType (only the language service uses this).
        let types = self.ty(t).types_list();
        let call_signature_lists: Vec<Vec<SignatureId>> = types
            .iter()
            .map(|&u| {
                if u == self.global_function_type {
                    return vec![self.unknown_signature];
                }
                self.get_signatures_of_type(u, SignatureKind::CALL).to_vec()
            })
            .collect();
        let mut call_signatures = self.get_union_signatures(&call_signature_lists);
        if call_signatures.is_empty() {
            call_signatures = self.get_array_member_call_signatures(t);
        }
        let construct_signature_lists: Vec<Vec<SignatureId>> = types
            .iter()
            .map(|&u| {
                self.get_signatures_of_type(u, SignatureKind::CONSTRUCT)
                    .to_vec()
            })
            .collect();
        let construct_signatures = self.get_union_signatures(&construct_signature_lists);
        let index_infos = self.get_union_index_infos(&types);
        self.set_structured_type_members(
            t,
            SymbolTable::NIL,
            &call_signatures,
            &construct_signatures,
            &index_infos,
        );
    }

    // Go: checker/checker.go:21408 getArrayMemberCallSignatures
    pub fn get_array_member_call_signatures(&mut self, t: TypeId) -> Vec<SignatureId> {
        // Check if union is exclusively instantiations of a member of the global Array or ReadonlyArray type.
        let mut member_name = String::new();
        let types = self.ty(t).types_list();
        for (i, u) in types.into_iter().enumerate() {
            let (u_object_flags, u_symbol) = {
                let ut = self.ty(u);
                (ut.object_flags, ut.symbol)
            };
            if !u_object_flags.intersects(ObjectFlags::INSTANTIATED) || u_symbol.is_nil() || {
                let u_parent = self.sym(u_symbol).parent;
                u_parent.is_nil() || !self.is_array_or_tuple_symbol(u_parent)
            } {
                return Vec::new();
            }
            if i == 0 {
                member_name = self.sym(u_symbol).name.to_string();
            } else if member_name != self.sym(u_symbol).name {
                return Vec::new();
            }
        }
        // Transform the type from `(A[] | B[])["member"]` to `(A | B)[]["member"]` (since we pretend array is covariant anyway).
        let array_arg = self.map_type(t, &mut |c: &mut Checker, u: TypeId| {
            let u_mapper = c.ty(u).mapper();
            let u_parent = c.sym(c.ty(u).symbol).parent;
            let array_type = if c.is_readonly_array_symbol(u_parent) {
                c.global_readonly_array_type
            } else {
                c.global_array_type
            };
            let type_parameter = c.ty(array_type).as_interface_type().type_parameters()[0];
            c.get_mapped_type(type_parameter, u_mapper)
        });
        let readonly = self.some_type(t, &mut |c: &mut Checker, u: TypeId| {
            let u_parent = c.sym(c.ty(u).symbol).parent;
            c.is_readonly_array_symbol(u_parent)
        });
        let array_type = self.create_array_type_ex(array_arg, readonly);
        let prop_type = self.get_type_of_property_of_type(array_type, &member_name);
        self.get_signatures_of_type(prop_type, SignatureKind::CALL)
            .to_vec()
    }

    // Go: checker/checker.go:21431 isArrayOrTupleSymbol
    pub fn is_array_or_tuple_symbol(&mut self, symbol: SymbolId) -> bool {
        let global_array_symbol = self.ty(self.global_array_type).symbol;
        let global_readonly_array_symbol = self.ty(self.global_readonly_array_type).symbol;
        if symbol.is_nil() || global_array_symbol.is_nil() || global_readonly_array_symbol.is_nil()
        {
            return false;
        }
        self.get_symbol_if_same_reference(symbol, global_array_symbol)
            .is_some()
            || self
                .get_symbol_if_same_reference(symbol, global_readonly_array_symbol)
                .is_some()
    }

    // Go: checker/checker.go:21438 isReadonlyArraySymbol
    pub fn is_readonly_array_symbol(&mut self, symbol: SymbolId) -> bool {
        let global_readonly_array_symbol = self.ty(self.global_readonly_array_type).symbol;
        if symbol.is_nil() || global_readonly_array_symbol.is_nil() {
            return false;
        }
        self.get_symbol_if_same_reference(symbol, global_readonly_array_symbol)
            .is_some()
    }

    // The signatures of a union type are those signatures that are present in each of the constituent types.
    // Generic signatures must match exactly, but non-generic signatures are allowed to have extra optional
    // parameters and may differ in return types. When signatures differ in return types, the resulting return
    // type is the union of the constituent return types.
    // Go: checker/checker.go:21449 getUnionSignatures
    pub fn get_union_signatures(
        &mut self,
        signature_lists: &[Vec<SignatureId>],
    ) -> Vec<SignatureId> {
        let mut result: Vec<SignatureId> = Vec::new();
        let mut index_with_length_over_one: usize = 0;
        let mut count_length_over_one: i32 = 0;
        for i in 0..signature_lists.len() {
            if signature_lists[i].is_empty() {
                return Vec::new();
            }
            if signature_lists[i].len() > 1 {
                index_with_length_over_one = i;
                count_length_over_one += 1;
            }
            for &signature in &signature_lists[i] {
                // Only process signatures with parameter lists that aren't already in the result list
                if result.is_empty()
                    || self
                        .find_matching_signature(
                            &result, signature, false, /*partialMatch*/
                            false, /*ignoreThisTypes*/
                            true,  /*ignoreReturnTypes*/
                        )
                        .is_nil()
                {
                    // PORT: Go checks `unionSignatures != nil`. Go findMatchingSignatures
                    // never returns an empty non-nil slice, so `!is_empty()` matches.
                    let union_signatures =
                        self.find_matching_signatures(signature_lists, signature, i as i32);
                    if !union_signatures.is_empty() {
                        let mut s = signature;
                        // Union the result types when more than one signature matches
                        if union_signatures.len() > 1 {
                            let mut this_parameter = self.sig(signature).this_parameter;
                            let first_this_parameter_of_union_signatures = union_signatures
                                .iter()
                                .map(|&sig| self.sig(sig).this_parameter)
                                .find(|p| p.is_some())
                                .unwrap_or(SymbolId::NIL);
                            if first_this_parameter_of_union_signatures.is_some() {
                                let mut this_types: Vec<TypeId> = Vec::new();
                                for &sig in &union_signatures {
                                    let sig_this_parameter = self.sig(sig).this_parameter;
                                    if sig_this_parameter.is_some() {
                                        let this_type = self.get_type_of_symbol(sig_this_parameter);
                                        if this_type.is_some() {
                                            this_types.push(this_type);
                                        }
                                    }
                                }
                                let this_type = self.get_intersection_type(&this_types);
                                this_parameter = self.create_symbol_with_type(
                                    first_this_parameter_of_union_signatures,
                                    this_type,
                                );
                            }
                            s = self.create_union_signature(signature, &union_signatures);
                            self.sig_mut(s).this_parameter = this_parameter;
                        }
                        result.push(s);
                    }
                }
            }
        }
        if result.is_empty() && count_length_over_one <= 1 {
            // No sufficiently similar signature existed to subsume all the other signatures in the union - time to see if we can make a single
            // signature that handles all of them. We only do this when there are overloads in only one constituent. (Overloads are conditional in
            // nature and having overloads in multiple constituents would necessitate making a power set of signatures from the type, whose
            // ordering would be non-obvious)
            let master_list = &signature_lists[index_with_length_over_one];
            let mut results: Vec<SignatureId> = master_list.clone();
            for signatures in signature_lists {
                // PORT: Go `core.Same` compares slice identity (same backing array
                // and length). Rust lists are owned copies, so equal contents
                // stand in for identity.
                if signatures != master_list {
                    let signature = signatures[0];
                    debug_assert!(
                        signature.is_some(),
                        "getUnionSignatures bails early on empty signature lists and should not have empty lists on second pass"
                    );
                    let signature_type_parameters = self.sig(signature).type_parameters.clone();
                    if !signature_type_parameters.is_empty()
                        && results.iter().any(|&s| {
                            let s_type_parameters = self.sig(s).type_parameters.clone();
                            !s_type_parameters.is_empty()
                                && !self.compare_type_parameters_identical(
                                    &signature_type_parameters,
                                    &s_type_parameters,
                                )
                        })
                    {
                        results = Vec::new();
                    } else {
                        results = results
                            .iter()
                            .map(|&sig| {
                                self.combine_union_or_intersection_member_signatures(
                                    sig, signature, true, /*isUnion*/
                                )
                            })
                            .collect();
                    }
                    if results.is_empty() {
                        break;
                    }
                }
            }
            result = results;
        }
        result
    }

    // Go: checker/checker.go:21520 combineUnionOrIntersectionMemberSignatures
    pub fn combine_union_or_intersection_member_signatures(
        &mut self,
        left: SignatureId,
        right: SignatureId,
        is_union: bool,
    ) -> SignatureId {
        let left_type_parameters = self.sig(left).type_parameters.clone();
        let right_type_parameters = self.sig(right).type_parameters.clone();
        let (type_params, type_params_origin) = if left_type_parameters.is_empty() {
            (
                right_type_parameters.clone(),
                self.share_type_parameters_origin(right),
            )
        } else {
            (
                left_type_parameters.clone(),
                self.share_type_parameters_origin(left),
            )
        };
        let mut param_mapper = MapperId::NIL;
        if !left_type_parameters.is_empty() && !right_type_parameters.is_empty() {
            // We just use the type parameter defaults from the first signature
            param_mapper = self.new_type_mapper(&right_type_parameters, &left_type_parameters);
        }
        let mut flags = (self.sig(left).flags | self.sig(right).flags)
            & SignatureFlags::PROPAGATING_FLAGS.without(SignatureFlags::HAS_REST_PARAMETER);
        let declaration = self.sig(left).declaration;
        let params =
            self.combine_union_or_intersection_parameters(left, right, param_mapper, is_union);
        let last_param = params.last().copied().unwrap_or(SymbolId::NIL);
        if last_param.is_some()
            && self
                .sym(last_param)
                .check_flags
                .intersects(CheckFlags::REST_PARAMETER)
        {
            flags |= SignatureFlags::HAS_REST_PARAMETER;
        }
        let (left_this_parameter, right_this_parameter) = (
            self.sig(left).this_parameter,
            self.sig(right).this_parameter,
        );
        let this_param = self.combine_union_or_intersection_this_param(
            left_this_parameter,
            right_this_parameter,
            param_mapper,
            is_union,
        );
        let min_arg_count = std::cmp::max(
            self.sig(left).min_argument_count,
            self.sig(right).min_argument_count,
        );
        let result = self.new_signature(
            flags,
            declaration,
            &type_params,
            this_param,
            &params,
            TypeId::NIL,
            TypePredicateId::NIL,
            min_arg_count,
        );
        self.sig_mut(result).type_parameters_origin = type_params_origin;
        let left_composite = self.sig(left).composite.clone();
        let left_mapper = self.sig(left).mapper;
        let mut left_signatures: Vec<SignatureId> = match &left_composite {
            Some(composite) if composite.is_union => composite.signatures.clone(),
            _ => vec![left],
        };
        left_signatures.push(right);
        self.sig_mut(result).composite = Some(Rc::new(CompositeSignature {
            is_union,
            signatures: left_signatures,
        }));
        if param_mapper.is_some() {
            if left_composite
                .as_ref()
                .is_some_and(|c| c.is_union == is_union)
                && left_mapper.is_some()
            {
                let combined = self.combine_type_mappers(left_mapper, param_mapper);
                self.sig_mut(result).mapper = combined;
            } else {
                self.sig_mut(result).mapper = param_mapper;
            }
        } else if left_composite
            .as_ref()
            .is_some_and(|c| c.is_union == is_union)
        {
            self.sig_mut(result).mapper = left_mapper;
        }
        result
    }

    // Go: checker/checker.go:21559 combineUnionOrIntersectionParameters
    pub fn combine_union_or_intersection_parameters(
        &mut self,
        left: SignatureId,
        right: SignatureId,
        mapper: MapperId,
        is_union: bool,
    ) -> Vec<SymbolId> {
        let left_count = self.get_parameter_count(left);
        let right_count = self.get_parameter_count(right);
        let (longest_count, longest, shorter) = if left_count >= right_count {
            (left_count, left, right)
        } else {
            (right_count, right, left)
        };
        let either_has_effective_rest =
            self.has_effective_rest_parameter(left) || self.has_effective_rest_parameter(right);
        let needs_extra_rest_element =
            either_has_effective_rest && !self.has_effective_rest_parameter(longest);
        let mut params: Vec<SymbolId> = vec![
            SymbolId::NIL;
            (longest_count + if needs_extra_rest_element { 1 } else { 0 })
                as usize
        ];
        for i in 0..longest_count {
            let mut longest_param_type = self.try_get_type_at_position(longest, i);
            if longest == right {
                longest_param_type = self.instantiate_type(longest_param_type, mapper);
            }
            let mut shorter_param_type = {
                let pt = self.try_get_type_at_position(shorter, i);
                if pt.is_some() { pt } else { self.unknown_type }
            };
            if shorter == right {
                shorter_param_type = self.instantiate_type(shorter_param_type, mapper);
            }
            let combined_param_type = self.get_union_or_intersection_type(
                &[longest_param_type, shorter_param_type],
                !is_union,
                UnionReduction::LITERAL,
            );
            let is_rest_param =
                either_has_effective_rest && !needs_extra_rest_element && i == (longest_count - 1);
            let is_optional = i >= self.get_min_argument_count(longest)
                && i >= self.get_min_argument_count(shorter);
            let mut left_name = String::new();
            let mut right_name = String::new();
            if i < left_count {
                left_name = self.get_parameter_name_at_position(left, i);
            }
            if i < right_count {
                right_name = self.get_parameter_name_at_position(right, i);
            }
            let mut param_name = String::new();
            if left_name == right_name {
                param_name = left_name;
            } else if left_name.is_empty() {
                param_name = right_name;
            } else if right_name.is_empty() {
                param_name = left_name;
            }
            if param_name.is_empty() {
                param_name = format!("arg{i}");
            }
            let param_symbol = self.new_symbol_ex(
                SymbolFlags::FUNCTION_SCOPED_VARIABLE
                    | if is_optional && !is_rest_param {
                        SymbolFlags::OPTIONAL
                    } else {
                        SymbolFlags::NONE
                    },
                param_name.as_str(),
                if is_rest_param {
                    CheckFlags::REST_PARAMETER
                } else if is_optional {
                    CheckFlags::OPTIONAL_PARAMETER
                } else {
                    CheckFlags::NONE
                },
            );
            let resolved_type = if is_rest_param {
                self.create_array_type(combined_param_type)
            } else {
                combined_param_type
            };
            self.value_symbol_links
                .get_by_id(&self.symbols, param_symbol)
                .resolved_type = resolved_type;
            params[i as usize] = param_symbol;
        }
        if needs_extra_rest_element {
            let rest_param_symbol = self.new_symbol_ex(
                SymbolFlags::FUNCTION_SCOPED_VARIABLE,
                "args",
                CheckFlags::REST_PARAMETER,
            );
            // Go `links := c.valueSymbolLinks.Get(restParamSymbol)` gives the id here.
            self.value_symbol_links
                .get_by_id(&self.symbols, rest_param_symbol);
            let type_at_position = self.get_type_at_position(shorter, longest_count);
            let mut resolved_type = self.create_array_type(type_at_position);
            self.value_symbol_links
                .get_by_id(&self.symbols, rest_param_symbol)
                .resolved_type = resolved_type;
            if shorter == right {
                resolved_type = self.instantiate_type(resolved_type, mapper);
                self.value_symbol_links
                    .get_by_id(&self.symbols, rest_param_symbol)
                    .resolved_type = resolved_type;
            }
            params[longest_count as usize] = rest_param_symbol;
        }
        params
    }

    // Go: checker/checker.go:21625 combineUnionOrIntersectionThisParam
    pub fn combine_union_or_intersection_this_param(
        &mut self,
        left: SymbolId,
        right: SymbolId,
        mapper: MapperId,
        is_union: bool,
    ) -> SymbolId {
        if left.is_nil() {
            return right;
        }
        if right.is_nil() {
            return left;
        }
        // A signature `this` type might be a read or a write position... It's very possible that it should be invariant
        // and we should refuse to merge signatures if there are `this` types and they do not match. However, so as to be
        // permissive when calling, for now, we'll intersect the `this` types just like we do for param types in union signatures.
        let left_type = self.get_type_of_symbol(left);
        let right_type = self.get_type_of_symbol(right);
        let right_instantiated = self.instantiate_type(right_type, mapper);
        let this_type = self.get_union_or_intersection_type(
            &[left_type, right_instantiated],
            !is_union,
            UnionReduction::LITERAL,
        );
        self.create_symbol_with_type(left, this_type)
    }

    // Go: checker/checker.go:21639 resolveIntersectionTypeMembers
    pub fn resolve_intersection_type_members(&mut self, t: TypeId) {
        // The members and properties collections are empty for intersection types. To get all properties of an
        // intersection type use getPropertiesOfType (only the language service uses this).
        let mut call_signatures: Vec<SignatureId> = Vec::new();
        let mut construct_signatures: Vec<SignatureId> = Vec::new();
        let mut index_infos: Vec<IndexInfoId> = Vec::new();
        let types = self.ty(t).types_list();
        let (mixin_flags, mixin_count) = self.find_mixins(&types);
        for (i, &u) in types.iter().enumerate() {
            // When an intersection type contains mixin constructor types, the construct signatures from
            // those types are discarded and their return types are mixed into the return types of all
            // other construct signatures in the intersection type. For example, the intersection type
            // '{ new(...args: any[]) => A } & { new(s: string) => B }' has a single construct signature
            // 'new(s: string) => A & B'.
            if !mixin_flags[i] {
                let mut signatures = self
                    .get_signatures_of_type(u, SignatureKind::CONSTRUCT)
                    .to_vec();
                if !signatures.is_empty() && mixin_count > 0 {
                    signatures = signatures
                        .iter()
                        .map(|&s| {
                            let clone = self.clone_signature(s);
                            let return_type = self.get_return_type_of_signature(s);
                            let mixed = self.include_mixin_type(
                                return_type,
                                &types,
                                &mixin_flags,
                                i as i32,
                            );
                            self.sig_mut(clone).resolved_return_type = mixed;
                            clone
                        })
                        .collect();
                }
                construct_signatures = self.append_signatures(construct_signatures, &signatures);
            }
            let call = self.get_signatures_of_type(u, SignatureKind::CALL);
            call_signatures = self.append_signatures(call_signatures, &call);
            for info in self.get_index_infos_of_type(u) {
                index_infos = self.append_index_info(index_infos, info, false /*union*/);
            }
        }
        self.set_structured_type_members(
            t,
            SymbolTable::NIL,
            &call_signatures,
            &construct_signatures,
            &index_infos,
        );
    }

    // Go: checker/checker.go:21672 appendSignatures
    pub fn append_signatures(
        &mut self,
        signatures: Vec<SignatureId>,
        new_signatures: &[SignatureId],
    ) -> Vec<SignatureId> {
        let mut signatures = signatures;
        for &sig in new_signatures {
            let should_append = signatures.is_empty()
                || signatures.clone().into_iter().all(|s| {
                    self.compare_signatures_identical(
                        s,
                        sig,
                        false, /*partialMatch*/
                        false, /*ignoreThisTypes*/
                        false, /*ignoreReturnTypes*/
                        &mut |c: &mut Checker, s: TypeId, t: TypeId| {
                            c.compare_types_identical(s, t)
                        },
                    ) == Ternary::FALSE
                });
            if should_append {
                signatures.push(sig);
            }
        }
        signatures
    }

    // Go: checker/checker.go:21683 appendIndexInfo
    pub fn append_index_info(
        &mut self,
        index_infos: Vec<IndexInfoId>,
        new_info: IndexInfoId,
        union: bool,
    ) -> Vec<IndexInfoId> {
        let mut index_infos = index_infos;
        let (new_key_type, new_value_type, new_is_readonly) = {
            let n = self.index_info(new_info);
            (n.key_type, n.value_type, n.is_readonly)
        };
        for i in 0..index_infos.len() {
            let info = index_infos[i];
            let (key_type, value_type, is_readonly) = {
                let n = self.index_info(info);
                (n.key_type, n.value_type, n.is_readonly)
            };
            if key_type == new_key_type {
                let result_value_type;
                let result_is_readonly;
                if union {
                    result_value_type = self.get_union_type(&[value_type, new_value_type]);
                    result_is_readonly = is_readonly || new_is_readonly;
                } else {
                    result_value_type = self.get_intersection_type(&[value_type, new_value_type]);
                    result_is_readonly = is_readonly && new_is_readonly;
                }
                index_infos[i] = self.new_index_info(
                    key_type,
                    result_value_type,
                    result_is_readonly,
                    Node::NIL,
                    &[],
                );
                return index_infos;
            }
        }
        index_infos.push(new_info);
        index_infos
    }
}
