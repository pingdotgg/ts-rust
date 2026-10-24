//! Port of `checker/checker.go` lines 19436-20350: base types, declared
//! members, index infos of symbols, signatures from declarations, late-bound
//! names, return types of signatures and bodies, and promise/generator types.

use crate::diagnostics::Message;
use crate::prelude::*;
use smallvec::SmallVec;

// PORT: Go `core.AppendIfUnique` for type slices.
fn append_if_unique_p22(list: &mut Vec<TypeId>, t: TypeId) {
    if !list.contains(&t) {
        list.push(t);
    }
}

// PORT: Go `core.OrElse` for types.
fn or_else_type_p22(t: TypeId, fallback: TypeId) -> TypeId {
    if t.is_some() { t } else { fallback }
}

impl Checker {
    // Go: checker/checker.go:19973 isValidBaseType
    pub fn is_valid_base_type(&mut self, t: TypeId) -> bool {
        if self.ty(t).flags.intersects(TypeFlags::TYPE_PARAMETER) {
            let constraint = self.get_base_constraint_of_type(t);
            if constraint.is_some() {
                return self.is_valid_base_type(constraint);
            }
        }
        // TODO: Given that we allow type parameters here now, is this `!isGenericMappedType(type)` check really needed?
        // There's no reason a `T` should be allowed while a `Readonly<T>` should not.
        let flags = self.ty(t).flags;
        if flags.intersects(TypeFlags::OBJECT | TypeFlags::NON_PRIMITIVE | TypeFlags::ANY)
            && !self.is_generic_mapped_type(t)
        {
            return true;
        }
        if flags.intersects(TypeFlags::INTERSECTION) {
            return (0..self.ty(t).types().len())
                .all(|i| self.is_valid_base_type(self.type_at(t, i)));
        }
        false
    }

    // TODO: GH#18217 If `checkBase` is undefined, we should not call this because this will always return false.
    // Go: checker/checker.go:19987 hasBaseType
    pub fn has_base_type(&mut self, t: TypeId, check_base: TypeId) -> bool {
        // PORT: the Go recursive closure `check` is a nested fn.
        // PERF: Go has no memo here, so a chain of classes that each list
        // all the classes before it makes an exponential walk (corpus case
        // intersectionConstructorReductionCrash.ts). Each step reads the
        // arena once for `t` (once more for a reference to another type),
        // inlines `getTargetType`, and reads resolved base types in place
        // with no list copy.
        fn check(c: &mut Checker, t: TypeId, check_base: TypeId) -> bool {
            let ty = c.ty(t);
            let object_flags = ty.object_flags;
            if object_flags.intersects(ObjectFlags::CLASS_OR_INTERFACE | ObjectFlags::REFERENCE) {
                // Go: checker/checker.go:20002 getTargetType
                let (target, target_ty) = if object_flags.intersects(ObjectFlags::REFERENCE) {
                    // A class or interface with `Reference` is its own
                    // target: a generic one, and also a non-generic class or
                    // an interface that is not thisless (Go
                    // `getDeclaredTypeOfClassOrInterface`, checker.go:17719).
                    // PERF: its target without the `Type::target` dispatch.
                    let target = match &ty.data {
                        TypeData::Interface(data) => data.reference.object.target,
                        _ => ty.target(),
                    };
                    (target, if target == t { ty } else { c.ty(target) })
                } else {
                    (t, ty)
                };
                if target == check_base {
                    return true;
                }
                let mut buf: SharedListBuf<TypeId> = Default::default();
                if let Some(base_types) = target_ty
                    .resolved_base_types()
                    .and_then(|base_types| base_types.detach(&mut buf))
                {
                    return base_types.iter().any(|&b| check(c, b, check_base));
                }
                let base_types = c.get_base_types_shared(target);
                return base_types.iter().any(|&b| check(c, b, check_base));
            }
            if ty.flags.intersects(TypeFlags::INTERSECTION) {
                let types = ty.types_list();
                return types.iter().any(|&t| check(c, t, check_base));
            }
            false
        }
        check(self, t, check_base)
    }

    // Go: checker/checker.go:20002 getTargetType
    pub fn get_target_type(&self, t: TypeId) -> TypeId {
        if self.ty(t).object_flags.intersects(ObjectFlags::REFERENCE) {
            return self.ty(t).target();
        }
        t
    }

    // Go: checker/checker.go:20009 getTypeWithThisArgument
    pub fn get_type_with_this_argument(
        &mut self,
        t: TypeId,
        this_argument: TypeId,
        need_apparent_type: bool,
    ) -> TypeId {
        let mut this_argument = this_argument;
        if self.ty(t).object_flags.intersects(ObjectFlags::REFERENCE) {
            let target = self.ty(t).target();
            let type_parameter_count = self.ty(target).as_interface_type().type_parameters().len();
            let args = {
                let type_arguments = self.type_arguments_of(t);
                (type_parameter_count == type_arguments.len()).then(|| {
                    // PERF: room for the this-argument, and no heap
                    // allocation for up to 8 arguments.
                    let mut args: SmallVec<[TypeId; 8]> =
                        SmallVec::with_capacity(type_arguments.len() + 1);
                    // PERF: a copy loop; `extend_from_slice` calls memmove
                    // and memcpy for a few ids.
                    args.extend(type_arguments.iter().copied());
                    args
                })
            };
            if let Some(mut args) = args {
                if this_argument.is_nil() {
                    this_argument = self.ty(target).as_interface_type().this_type;
                }
                args.push(this_argument);
                return self.create_type_reference(target, &args);
            }
            return t;
        } else if self.ty(t).flags.intersects(TypeFlags::INTERSECTION) {
            // PORT: Go `core.SameMap` + `core.Same` is "no element changed"
            // (`None`) here.
            let count = self.ty(t).types().len();
            let Some(new_types) =
                self.map_stored_types_if_changed(t, count, Checker::type_at, &mut |c, ty| {
                    c.get_type_with_this_argument(ty, this_argument, need_apparent_type)
                })
            else {
                return t;
            };
            return self.get_intersection_type(&new_types);
        }
        if need_apparent_type {
            return self.get_apparent_type(t);
        }
        t
    }

    // Go: checker/checker.go:20034 addInheritedMembers
    pub fn add_inherited_members(
        &mut self,
        symbols: SymbolTable,
        base_symbols: &[SymbolId],
    ) -> SymbolTable {
        let mut symbols = symbols;
        // PERF: room for every base property up front, so the loop does not
        // grow the table or rebuild its index. Entry order does not change.
        self.symbols.reserve(symbols, base_symbols.len());
        for &base in base_symbols {
            if !self.is_static_private_identifier_property(base) {
                let base_name = self.sym(base).name.clone();
                // PORT: one table lookup for the Go read and write. A nil
                // table has no entry, so Go always writes; make it first.
                if symbols.is_nil() {
                    symbols = self.symbols.new_table_with_capacity(base_symbols.len());
                }
                self.symbols
                    .set_if_absent_or(symbols, &base_name, base, |s| {
                        !s.flags.intersects(SymbolFlags::VALUE)
                    });
            }
        }
        symbols
    }

    // Go: checker/checker.go:20048 resolveDeclaredMembers
    // PORT: returns `t.AsInterfaceType()` borrowed from the type arena.
    pub fn resolve_declared_members(&mut self, t: TypeId) -> &InterfaceType {
        if !self.ty(t).as_interface_type().declared_members_resolved {
            let symbol = self.ty(t).symbol;
            let members = self.get_members_of_symbol(symbol);
            {
                let d = self.ty_mut(t).as_interface_type_mut();
                d.declared_members_resolved = true;
                d.declared_members = members;
            }
            let call_symbol = self.symbols.get(members, INTERNAL_SYMBOL_NAME_CALL);
            let call_signatures = self.get_signatures_of_symbol(call_symbol);
            self.ty_mut(t)
                .as_interface_type_mut()
                .declared_call_signatures = call_signatures.into();
            let new_symbol = self.symbols.get(members, INTERNAL_SYMBOL_NAME_NEW);
            let construct_signatures = self.get_signatures_of_symbol(new_symbol);
            self.ty_mut(t)
                .as_interface_type_mut()
                .declared_construct_signatures = construct_signatures.into();
            let index_infos = self.get_index_infos_of_symbol(symbol);
            self.ty_mut(t).as_interface_type_mut().declared_index_infos = index_infos.into();
        }
        self.ty(t).as_interface_type()
    }

    // Go: checker/checker.go:20061 getIndexInfosOfSymbol
    pub fn get_index_infos_of_symbol(&mut self, symbol: SymbolId) -> Vec<IndexInfoId> {
        let index_symbol = self.get_index_symbol(symbol);
        if index_symbol.is_some() {
            let members = self.get_members_of_symbol(symbol);
            // PORT: Go `maps.Values` order is random; ours is insertion order.
            let sibling_symbols = self.symbols.values(members);
            return self.get_index_infos_of_index_symbol(index_symbol, &sibling_symbols);
        }
        Vec::new()
    }

    // note intentional similarities to index signature building in `checkObjectLiteral` for parity
    // Go: checker/checker.go:20070 getIndexInfosOfIndexSymbol
    pub fn get_index_infos_of_index_symbol(
        &mut self,
        index_symbol: SymbolId,
        sibling_symbols: &[SymbolId],
    ) -> Vec<IndexInfoId> {
        let mut index_infos: Vec<IndexInfoId> = Vec::new();
        let mut has_computed_string_property = false;
        let mut has_computed_number_property = false;
        let mut has_computed_symbol_property = false;
        let mut readonly_computed_string_property = true;
        let mut readonly_computed_number_property = true;
        let mut readonly_computed_symbol_property = true;
        let mut property_symbols: Vec<SymbolId> = Vec::new();
        let declarations = self.sym(index_symbol).declarations.clone();
        for &declaration in declarations.iter() {
            if is_index_signature_declaration(declaration) {
                let parameters = declaration.parameters();
                let return_type_node = declaration.type_();
                if parameters.len() == 1 {
                    let type_node = parameters.get(0).type_();
                    if type_node.is_some() {
                        let mut value_type = self.any_type;
                        if return_type_node.is_some() {
                            value_type = self.get_type_from_type_node(return_type_node);
                        }
                        let key_types = self.get_type_from_type_node(type_node);
                        self.for_each_type(key_types, &mut |c: &mut Checker, key_type: TypeId| {
                            if c.is_valid_index_key_type(key_type)
                                && c.find_index_info(&index_infos, key_type).is_nil()
                            {
                                let index_info = c.new_index_info(
                                    key_type,
                                    value_type,
                                    has_modifier(declaration, ModifierFlags::READONLY),
                                    declaration,
                                    &[],
                                );
                                index_infos.push(index_info);
                            }
                        });
                    }
                }
            } else if self.has_late_bindable_index_signature(declaration) {
                let decl_name = if is_binary_expression(declaration) {
                    declaration.left()
                } else {
                    declaration.name()
                };
                let key_type = if is_element_access_expression(decl_name) {
                    self.check_expression_cached(decl_name.argument_expression())
                } else {
                    self.check_computed_property_name(decl_name)
                };
                if self.find_index_info(&index_infos, key_type).is_some() {
                    continue;
                    // Explicit index for key type takes priority
                }
                let string_number_symbol_type = self.string_number_symbol_type;
                if self.is_type_assignable_to(key_type, string_number_symbol_type) {
                    let number_type = self.number_type;
                    let es_symbol_type = self.es_symbol_type;
                    if self.is_type_assignable_to(key_type, number_type) {
                        has_computed_number_property = true;
                        if !has_readonly_modifier(declaration) {
                            readonly_computed_number_property = false;
                        }
                    } else if self.is_type_assignable_to(key_type, es_symbol_type) {
                        has_computed_symbol_property = true;
                        if !has_readonly_modifier(declaration) {
                            readonly_computed_symbol_property = false;
                        }
                    } else {
                        has_computed_string_property = true;
                        if !has_readonly_modifier(declaration) {
                            readonly_computed_string_property = false;
                        }
                    }
                    property_symbols.push(declaration.symbol());
                }
            }
        }
        if has_computed_string_property
            || has_computed_number_property
            || has_computed_symbol_property
        {
            for &sym in sibling_symbols {
                if sym != index_symbol {
                    property_symbols.push(sym);
                }
            }
            // aggregate similar index infos implied to be the same key to the same combined index info
            let string_type = self.string_type;
            if has_computed_string_property
                && self.find_index_info(&index_infos, string_type).is_nil()
            {
                let info = self.get_object_literal_index_info(
                    readonly_computed_string_property,
                    &property_symbols,
                    string_type,
                );
                index_infos.push(info);
            }
            let number_type = self.number_type;
            if has_computed_number_property
                && self.find_index_info(&index_infos, number_type).is_nil()
            {
                let info = self.get_object_literal_index_info(
                    readonly_computed_number_property,
                    &property_symbols,
                    number_type,
                );
                index_infos.push(info);
            }
            let es_symbol_type = self.es_symbol_type;
            if has_computed_symbol_property
                && self.find_index_info(&index_infos, es_symbol_type).is_nil()
            {
                let info = self.get_object_literal_index_info(
                    readonly_computed_symbol_property,
                    &property_symbols,
                    es_symbol_type,
                );
                index_infos.push(info);
            }
        }
        index_infos
    }

    // NOTE: currently does not make pattern literal indexers, eg `${number}px`
    // Go: checker/checker.go:20157 getObjectLiteralIndexInfo
    pub fn get_object_literal_index_info(
        &mut self,
        is_readonly: bool,
        properties: &[SymbolId],
        key_type: TypeId,
    ) -> IndexInfoId {
        let mut prop_types: Vec<TypeId> = Vec::new();
        let mut components: Vec<Node> = Vec::new();
        for &prop in properties {
            if key_type == self.string_type && !self.is_symbol_with_symbol_name(prop)
                || key_type == self.number_type && self.is_symbol_with_numeric_name(prop)
                || key_type == self.es_symbol_type && self.is_symbol_with_symbol_name(prop)
            {
                let prop_type = self.get_type_of_symbol(prop);
                prop_types.push(prop_type);
                if self.is_symbol_with_computed_name(prop) {
                    components.push(self.sym(prop).declarations[0]);
                }
            }
        }
        let mut union_type = self.undefined_type;
        if !prop_types.is_empty() {
            union_type =
                self.get_union_type_ex(&prop_types, UnionReduction::SUBTYPE, None, TypeId::NIL);
        }
        self.new_index_info(
            key_type,
            union_type,
            is_readonly,
            Node::NIL, /*declaration*/
            &components,
        )
    }

    // Go: checker/checker.go:20177 isSymbolWithSymbolName
    pub fn is_symbol_with_symbol_name(&mut self, symbol: SymbolId) -> bool {
        if self.is_known_symbol(symbol) {
            return true;
        }
        if let Some(&decl) = self.sym(symbol).declarations.first() {
            let name = decl.name();
            if !(name.is_some() && is_computed_property_name(name)) {
                return false;
            }
            let t = self.check_computed_property_name(name);
            return self.is_type_assignable_to_kind(t, TypeFlags::ES_SYMBOL);
        }
        false
    }

    // Go: checker/checker.go:20188 isSymbolWithNumericName
    pub fn is_symbol_with_numeric_name(&mut self, symbol: SymbolId) -> bool {
        if is_numeric_literal_name(&self.sym(symbol).name) {
            return true;
        }
        if let Some(&decl) = self.sym(symbol).declarations.first() {
            let name = decl.name();
            return name.is_some() && self.is_numeric_name(name);
        }
        false
    }

    // Go: checker/checker.go:20199 isSymbolWithComputedName
    pub fn is_symbol_with_computed_name(&self, symbol: SymbolId) -> bool {
        if let Some(&decl) = self.sym(symbol).declarations.first() {
            let name = decl.name();
            return name.is_some() && is_computed_property_name(name);
        }
        false
    }

    // Go: checker/checker.go:20207 isNumericName
    pub fn is_numeric_name(&mut self, name: Node) -> bool {
        match name.kind() {
            SyntaxKind::ComputedPropertyName => return self.is_numeric_computed_name(name),
            SyntaxKind::Identifier | SyntaxKind::NumericLiteral | SyntaxKind::StringLiteral => {
                return is_numeric_literal_name(name.text());
            }
            _ => {}
        }
        false
    }

    // Go: checker/checker.go:20217 isNumericComputedName
    pub fn is_numeric_computed_name(&mut self, name: Node) -> bool {
        // It seems odd to consider an expression of type Any to result in a numeric name,
        // but this behavior is consistent with checkIndexedAccess
        let t = self.check_computed_property_name(name);
        self.is_type_assignable_to_kind(t, TypeFlags::NUMBER_LIKE)
    }

    // Go: checker/checker.go:20223 isValidIndexKeyType
    pub fn is_valid_index_key_type(&mut self, t: TypeId) -> bool {
        if self
            .ty(t)
            .flags
            .intersects(TypeFlags::STRING | TypeFlags::NUMBER | TypeFlags::ES_SYMBOL)
        {
            return true;
        }
        if self.is_pattern_literal_type(t) {
            return true;
        }
        if self.ty(t).flags.intersects(TypeFlags::INTERSECTION) && !self.is_generic_type(t) {
            let types = self.ty(t).types_list();
            return types.iter().any(|&t| self.is_valid_index_key_type(t));
        }
        false
    }

    // Go: checker/checker.go:20229 findIndexInfo
    // PORT: Go has both a package-level `findIndexInfo` (checker.go:19594,
    // ported elsewhere as `Checker::find_index_info`) and this `*Checker`
    // method with the same body. Both would be `find_index_info`, so this
    // method gets a `_method` suffix.
    pub fn find_index_info_method(
        &self,
        index_infos: &[IndexInfoId],
        key_type: TypeId,
    ) -> IndexInfoId {
        for &info in index_infos {
            if self.index_info(info).key_type == key_type {
                return info;
            }
        }
        IndexInfoId::NIL
    }

    // Go: checker/checker.go:20238 getIndexSymbol
    pub fn get_index_symbol(&mut self, symbol: SymbolId) -> SymbolId {
        let members = self.get_members_of_symbol(symbol);
        self.symbols.get(members, INTERNAL_SYMBOL_NAME_INDEX)
    }

    // Go: checker/checker.go:20242 getSignaturesOfSymbol
    pub fn get_signatures_of_symbol(&mut self, symbol: SymbolId) -> Vec<SignatureId> {
        if symbol.is_nil() {
            return Vec::new();
        }
        let mut result: Vec<SignatureId> = Vec::new();
        let declarations = self.sym(symbol).declarations.clone();
        for (i, &decl) in declarations.iter().enumerate() {
            if !is_function_like(decl) {
                continue;
            }
            // Don't include signature if node is the implementation of an overloaded function. A node is considered
            // an implementation node if it has a body and the previous node is of the same kind and immediately
            // precedes the implementation node (i.e. has the same parent and ends where the implementation starts).
            if i > 0 && decl.body().is_some() {
                let previous = declarations[i - 1];
                if decl.parent() == previous.parent()
                    && decl.kind() == previous.kind()
                    && (decl.pos() == previous.end()
                        || previous.flags().intersects(NodeFlags::REPARSED))
                {
                    continue;
                }
            }
            // If this is a function or method declaration, get the signature from the @type tag for the sake of optional parameters.
            // Exclude contextually-typed kinds because we already apply the @type tag to the context, plus applying it here to the initializer would suppress checks that the two are compatible.
            let mut sig = self.get_signature_of_full_signature_type(decl);
            if sig.is_nil() {
                sig = self.get_signature_from_declaration(decl);
            }
            result.push(sig);
        }
        result
    }

    // Go: checker/checker.go:20272 getSignatureFromDeclaration
    pub fn get_signature_from_declaration(&mut self, declaration: Node) -> SignatureId {
        let resolved = self.signature_links.get(declaration).resolved_signature;
        if resolved.is_some() {
            return resolved;
        }
        // PORT: Go `declaration.Parameters()` dereferences nil for a node
        // that is not function-like (the API can pass any node). The port's
        // nil list reads as empty, so it would make and register a
        // signature. Nothing before Go's first read has a side effect.
        let parameter_list = declaration.parameter_list();
        if parameter_list.is_nil() {
            // Go loads `FunctionLikeData().Parameters` of the nil data (offset 0x18): addr=0x18
            // (ast.go:200).
            crate::core::go_nil_dereference_at(0x18);
        }
        // PERF: chkport1 item 5. Go reads `declaration.Parameters()`,
        // `declaration.Kind` and `param.Symbol()` as plain fields. Here each
        // one is an AST store read, so they are read once: `params` for the
        // untyped test, the loop and `hasRestParameter`, `kind` for the kind
        // tests (`ast.IsXxx(declaration)`).
        let params = parameter_list.nodes();
        let kind = declaration.kind();
        let mut parameters: Vec<SymbolId> = Vec::new();
        let mut flags = SignatureFlags::NONE;
        let mut this_parameter = SymbolId::NIL;
        let mut min_argument_count: i32 = 0;
        let mut has_this_parameter = false;
        let iife = get_immediately_invoked_function_expression(declaration);
        let is_untyped_signature_in_js_file = iife.is_nil()
            && is_in_js_file(declaration)
            && matches!(
                kind,
                SyntaxKind::FunctionExpression
                    | SyntaxKind::ArrowFunction
                    | SyntaxKind::MethodDeclaration
                    | SyntaxKind::GetAccessor
                    | SyntaxKind::SetAccessor
                    | SyntaxKind::FunctionDeclaration
                    | SyntaxKind::Constructor
            )
            && params.iter().all(|param| param.type_().is_nil())
            && self
                .get_contextual_type(declaration, ContextFlags::SIGNATURE)
                .is_nil();
        if is_untyped_signature_in_js_file {
            flags |= SignatureFlags::IS_UNTYPED_SIGNATURE_IN_JS_FILE;
        }
        for (i, param) in params.iter().enumerate() {
            let param_node_symbol = param.symbol();
            let mut param_symbol = param_node_symbol;
            let type_node = param.type_();
            // Include parameter symbol instead of property symbol in the signature
            if param_symbol.is_some()
                && self
                    .sym(param_symbol)
                    .flags
                    .intersects(SymbolFlags::PROPERTY)
                && !is_binding_pattern(param.name())
            {
                let name = self.sym(param_symbol).name.clone();
                let resolve_name = self.resolve_name.clone();
                let resolved_symbol = resolve_name(
                    self,
                    param,
                    &name,
                    SymbolFlags::VALUE,
                    None,  /*nameNotFoundMessage*/
                    false, /*isUse*/
                    false, /*excludeGlobals*/
                );
                param_symbol = resolved_symbol;
            }
            if i == 0 && self.sym(param_symbol).name == INTERNAL_SYMBOL_NAME_THIS {
                has_this_parameter = true;
                this_parameter = param_node_symbol;
            } else {
                parameters.push(param_symbol);
            }
            if type_node.is_some() && type_node.kind() == SyntaxKind::LiteralType {
                flags |= SignatureFlags::HAS_LITERAL_TYPES;
            }
            // Record a new minimum argument count if this is not an optional parameter
            let is_optional_parameter = is_optional_declaration(param)
                || param.initializer().is_some()
                || is_rest_parameter(param)
                || iife.is_some()
                    && parameters.len() > iife.arguments().len()
                    && type_node.is_nil();
            if !is_optional_parameter {
                min_argument_count = parameters.len() as i32;
            }
        }
        // If only one accessor includes a this-type annotation, the other behaves as if it had the same type annotation
        if (kind == SyntaxKind::GetAccessor || kind == SyntaxKind::SetAccessor)
            && self.has_bindable_name(declaration)
            && (!has_this_parameter || this_parameter.is_nil())
        {
            let other_kind = if kind == SyntaxKind::GetAccessor {
                SyntaxKind::SetAccessor
            } else {
                SyntaxKind::GetAccessor
            };
            let symbol = self.get_symbol_of_declaration(declaration);
            let other = get_declaration_of_kind(&self.symbols, symbol, other_kind);
            if other.is_some() {
                this_parameter = self.get_annotated_accessor_this_parameter(other);
            }
        }
        let mut class_type = TypeId::NIL;
        if kind == SyntaxKind::Constructor {
            let merged = self.get_merged_symbol(declaration.parent().symbol());
            class_type = self.get_declared_type_of_class_or_interface(merged);
        }
        let (type_parameters, type_parameters_origin) = if class_type.is_some() {
            let type_parameters = self
                .ty(class_type)
                .as_interface_type()
                .local_type_parameters()
                .to_vec();
            (
                type_parameters,
                self.class_type_parameters_origin(class_type),
            )
        } else {
            self.get_type_parameters_from_declaration_ex(declaration)
        };
        // Go `hasRestParameter(declaration)` on the list read above.
        if params.last().is_some_and(is_rest_parameter) {
            flags |= SignatureFlags::HAS_REST_PARAMETER;
        }
        if kind == SyntaxKind::ConstructorType
            || kind == SyntaxKind::Constructor
            || kind == SyntaxKind::ConstructSignature
        {
            flags |= SignatureFlags::CONSTRUCT;
        }
        if kind == SyntaxKind::ConstructorType
            && has_syntactic_modifier(declaration, ModifierFlags::ABSTRACT)
            || kind == SyntaxKind::Constructor
                && has_syntactic_modifier(declaration.parent(), ModifierFlags::ABSTRACT)
        {
            flags |= SignatureFlags::ABSTRACT;
        }
        let sig = self.new_signature(
            flags,
            declaration,
            &type_parameters,
            this_parameter,
            &parameters,
            TypeId::NIL,          /*resolvedReturnType*/
            TypePredicateId::NIL, /*resolvedTypePredicate*/
            min_argument_count,
        );
        self.sig_mut(sig).type_parameters_origin = type_parameters_origin;
        self.signature_links.get(declaration).resolved_signature = sig;
        sig
    }

    // Go: checker/checker.go:20348 getTypeParametersFromDeclaration
    pub fn get_type_parameters_from_declaration(&mut self, declaration: Node) -> Vec<TypeId> {
        self.get_type_parameters_from_declaration_ex(declaration).0
    }

    // PORT: `getTypeParametersFromDeclaration` plus the slice identity of
    // the result (`Signature::type_parameters_origin`): Go returns the
    // full signature's own slice, or a new one.
    pub fn get_type_parameters_from_declaration_ex(
        &mut self,
        declaration: Node,
    ) -> (Vec<TypeId>, u32) {
        let sig = self.get_signature_of_full_signature_type(declaration);
        if sig.is_some() {
            let origin = self.share_type_parameters_origin(sig);
            return (self.sig(sig).type_parameters().to_vec(), origin);
        }
        let mut result: Vec<TypeId> = Vec::new();
        for node in declaration.type_parameters().iter() {
            let t = self.get_declared_type_of_type_parameter(node.symbol());
            append_if_unique_p22(&mut result, t);
        }
        (result, 0)
    }

    // Go: checker/checker.go:20359 getAnnotatedAccessorThisParameter
    pub fn get_annotated_accessor_this_parameter(&mut self, accessor: Node) -> SymbolId {
        let parameter = self.get_accessor_this_parameter(accessor);
        if parameter.is_some() {
            return parameter.symbol();
        }
        SymbolId::NIL
    }

    // Go: checker/checker.go:20367 getAccessorThisParameter
    pub fn get_accessor_this_parameter(&self, accessor: Node) -> Node {
        let expected = if is_get_accessor_declaration(accessor) {
            1
        } else {
            2
        };
        if accessor.parameters().len() == expected {
            return get_this_parameter(accessor);
        }
        Node::NIL
    }

    /**
     * Indicates whether a declaration has an early-bound name or a dynamic name that can be late-bound.
     */
    // Go: checker/checker.go:20377 hasBindableName
    pub fn has_bindable_name(&mut self, node: Node) -> bool {
        !has_dynamic_name(node) || self.has_late_bindable_name(node)
    }

    /**
     * Indicates whether a declaration has a late-bindable dynamic name.
     */
    // Go: checker/checker.go:20384 hasLateBindableName
    pub fn has_late_bindable_name(&mut self, node: Node) -> bool {
        let name = get_name_of_declaration(node);
        name.is_some() && self.is_late_bindable_name(name)
    }

    /**
     * Indicates whether a declaration name is definitely late-bindable.
     * A declaration name is only late-bindable if:
     * - It is a `ComputedPropertyName`.
     * - Its expression is an `Identifier` or either a `PropertyAccessExpression` an
     * `ElementAccessExpression` consisting only of these same three types of nodes.
     * - The type of its expression is a string or numeric literal type, or is a `unique symbol` type.
     */
    // Go: checker/checker.go:20397 isLateBindableName
    pub fn is_late_bindable_name(&mut self, node: Node) -> bool {
        if !is_late_bindable_ast(node) {
            return false;
        }
        if is_computed_property_name(node) {
            let t = self.check_computed_property_name(node);
            return self.is_type_usable_as_property_name(t);
        }
        let t = self.check_expression_cached(node.argument_expression());
        self.is_type_usable_as_property_name(t)
    }

    // Go: checker/checker.go:20407 hasLateBindableIndexSignature
    pub fn has_late_bindable_index_signature(&mut self, node: Node) -> bool {
        let name = get_name_of_declaration(node);
        name.is_some() && self.is_late_bindable_index_signature(name)
    }

    // Go: checker/checker.go:20412 isLateBindableIndexSignature
    pub fn is_late_bindable_index_signature(&mut self, node: Node) -> bool {
        if !is_late_bindable_ast(node) {
            return false;
        }
        if is_computed_property_name(node) {
            let t = self.check_computed_property_name(node);
            return self.is_type_usable_as_index_signature_declaration(t);
        }
        let t = self.check_expression_cached(node.argument_expression());
        self.is_type_usable_as_index_signature_declaration(t)
    }

    // Go: checker/checker.go:20422 isTypeUsableAsIndexSignatureDeclaration
    pub fn is_type_usable_as_index_signature_declaration(&mut self, t: TypeId) -> bool {
        let string_number_symbol_type = self.string_number_symbol_type;
        self.is_type_assignable_to(t, string_number_symbol_type)
    }
}

// Go: checker/checker.go:20426 isLateBindableAST
pub fn is_late_bindable_ast(node: Node) -> bool {
    let mut expr = Node::NIL;
    if is_computed_property_name(node) {
        expr = node.expression();
    } else if is_element_access_expression(node) {
        expr = node.argument_expression();
    }
    expr.is_some() && is_entity_name_expression(expr)
}

impl Checker {
    // Go: checker/checker.go:20437 getReturnTypeOfSignature
    pub fn get_return_type_of_signature(&mut self, sig: SignatureId) -> TypeId {
        if self.sig(sig).resolved_return_type.is_some() {
            return self.sig(sig).resolved_return_type;
        }
        if !self.push_type_resolution(
            TypeSystemEntity::Signature(sig),
            TypeSystemPropertyName::RESOLVED_RETURN_TYPE,
        ) {
            return self.error_type;
        }
        let target = self.sig(sig).target;
        let mapper = self.sig(sig).mapper;
        let composite = self.sig(sig).composite.clone();
        let declaration = self.sig(sig).declaration;
        let mut t: TypeId;
        if target.is_some() {
            let target_return_type = self.get_return_type_of_signature(target);
            t = self.instantiate_type(target_return_type, mapper);
        } else if let Some(composite) = composite {
            let mut return_types = Vec::with_capacity(composite.signatures.len());
            for &s in &composite.signatures {
                return_types.push(self.get_return_type_of_signature(s));
            }
            let combined = self.get_union_or_intersection_type(
                &return_types,
                composite.is_union,
                UnionReduction::SUBTYPE,
            );
            t = self.instantiate_type(combined, mapper);
        } else {
            t = self.get_return_type_from_annotation(declaration);
            if t.is_nil() {
                if !node_is_missing(declaration.body()) {
                    t = self.get_return_type_from_body(declaration, CheckMode::NORMAL);
                } else {
                    t = self.any_type;
                }
            }
        }
        let flags = self.sig(sig).flags;
        if flags.intersects(SignatureFlags::IS_INNER_CALL_CHAIN) {
            t = self.add_optional_type_marker(t);
        } else if flags.intersects(SignatureFlags::IS_OUTER_CALL_CHAIN) {
            t = self.get_optional_type(t, false /*isProperty*/);
        }
        if !self.pop_type_resolution() {
            if declaration.is_some() {
                let type_node = declaration.type_();
                if type_node.is_some() {
                    self.error(
                        type_node,
                        diag::Return_type_annotation_circularly_references_itself,
                        args![],
                    );
                } else if self.no_implicit_any {
                    let name = get_name_of_declaration(declaration);
                    if name.is_some() {
                        self.error(
                            name,
                            diag::X_0_implicitly_has_return_type_any_because_it_does_not_have_a_return_type_annotation_and_is_referenced_directly_or_indirectly_in_one_of_its_return_expressions,
                            args![declaration_name_to_string(name)],
                        );
                    } else {
                        self.error(
                            declaration,
                            diag::Function_implicitly_has_return_type_any_because_it_does_not_have_a_return_type_annotation_and_is_referenced_directly_or_indirectly_in_one_of_its_return_expressions,
                            args![],
                        );
                    }
                }
            }
            t = self.any_type;
        }
        if self.sig(sig).resolved_return_type.is_nil() {
            self.sig_mut(sig).resolved_return_type = t;
        }
        self.sig(sig).resolved_return_type
    }

    // Go: checker/checker.go:20487 getNonCircularReturnTypeOfSignature
    pub fn get_non_circular_return_type_of_signature(&mut self, sig: SignatureId) -> TypeId {
        if self.is_resolving_return_type_of_signature(sig) {
            return self.any_type;
        }
        self.get_return_type_of_signature(sig)
    }

    // Go: checker/checker.go:20494 getReturnTypeFromAnnotation
    pub fn get_return_type_from_annotation(&mut self, declaration: Node) -> TypeId {
        if is_constructor_declaration(declaration) {
            let merged = self.get_merged_symbol(declaration.parent().symbol());
            return self.get_declared_type_of_class_or_interface(merged);
        }
        let return_type = declaration.type_();
        if return_type.is_some() {
            return self.get_type_from_type_node(return_type);
        }
        if is_get_accessor_declaration(declaration) && self.has_bindable_name(declaration) {
            let symbol = self.get_symbol_of_declaration(declaration);
            let set_accessor =
                get_declaration_of_kind(&self.symbols, symbol, SyntaxKind::SetAccessor);
            return self.get_annotated_accessor_type(set_accessor);
        }
        self.get_return_type_of_full_signature(declaration)
    }

    // Go: checker/checker.go:20508 getSignatureOfFullSignatureType
    pub fn get_signature_of_full_signature_type(&mut self, node: Node) -> SignatureId {
        if is_in_js_file(node)
            && (is_function_declaration(node)
                || is_method_declaration(node)
                || is_function_expression_or_arrow_function(node))
            && node.full_signature().is_some()
        {
            let t = self.get_type_from_type_node(node.full_signature());
            return self.get_single_call_signature(t);
        }
        SignatureId::NIL
    }

    // Go: checker/checker.go:20515 getParameterTypeOfFullSignature
    pub fn get_parameter_type_of_full_signature(&mut self, node: Node, parameter: Node) -> TypeId {
        let signature = self.get_signature_of_full_signature_type(node);
        if signature.is_some() {
            let pos = node
                .parameters()
                .iter()
                .position(|p| p == parameter)
                .map_or(-1, |p| p as i32);
            if parameter.dot_dot_dot_token().is_some() {
                return self.get_rest_type_at_position(signature, pos, false /*readonly*/);
            } else {
                return self.get_type_at_position(signature, pos);
            }
        }
        TypeId::NIL
    }

    // Go: checker/checker.go:20527 getReturnTypeOfFullSignature
    pub fn get_return_type_of_full_signature(&mut self, node: Node) -> TypeId {
        let signature = self.get_signature_of_full_signature_type(node);
        if signature.is_some() {
            return self.get_return_type_of_signature(signature);
        }
        TypeId::NIL
    }

    // Go: checker/checker.go:20534 getAnnotatedAccessorType
    pub fn get_annotated_accessor_type(&mut self, accessor: Node) -> TypeId {
        let node = self.get_annotated_accessor_type_node(accessor);
        if node.is_some() {
            return self.get_type_from_type_node(node);
        }
        TypeId::NIL
    }

    // Go: checker/checker.go:20542 getAnnotatedAccessorTypeNode
    pub fn get_annotated_accessor_type_node(&self, accessor: Node) -> Node {
        if accessor.is_some() {
            match accessor.kind() {
                SyntaxKind::GetAccessor | SyntaxKind::PropertyDeclaration => {
                    return accessor.type_();
                }
                SyntaxKind::SetAccessor => {
                    return get_effective_set_accessor_type_annotation_node(accessor);
                }
                _ => {}
            }
        }
        Node::NIL
    }
}

// Go: checker/checker.go:20554 getEffectiveSetAccessorTypeAnnotationNode
pub fn get_effective_set_accessor_type_annotation_node(node: Node) -> Node {
    let param = get_set_accessor_value_parameter(node);
    if param.is_some() {
        return param.type_();
    }
    Node::NIL
}

impl Checker {
    // Go: checker/checker.go:20562 getReturnTypeFromBody
    pub fn get_return_type_from_body(&mut self, fn_: Node, check_mode: CheckMode) -> TypeId {
        let body = fn_.body();
        if body.is_nil() {
            return self.error_type;
        }
        let function_flags = get_function_flags(fn_);
        let is_async = function_flags.intersects(FunctionFlags::ASYNC);
        let is_generator = function_flags.intersects(FunctionFlags::GENERATOR);
        let mut return_type = TypeId::NIL;
        let mut yield_type = TypeId::NIL;
        let mut next_type = TypeId::NIL;
        let mut fallback_return_type = self.void_type;
        if !is_block(body) {
            return_type = self.check_expression_cached_ex(
                body,
                check_mode.without(CheckMode::SKIP_GENERIC_FUNCTIONS),
            );
            if self.is_const_context(body) {
                return_type = self.get_regular_type_of_literal_type(return_type);
            }
            if is_async {
                // From within an async function you can return either a non-promise value or a promise. Any
                // Promise/A+ compatible implementation will always assimilate any foreign promise, so the
                // return type of the body should be unwrapped to its awaited type, which we will wrap in
                // the native Promise<T> type later in this function.
                let awaited = self.check_awaited_type(
                    return_type,
                    false, /*withAlias*/
                    fn_,   /*errorNode*/
                    diag::The_return_type_of_an_async_function_must_either_be_a_valid_promise_or_must_not_contain_a_callable_then_member,
                );
                return_type = self.unwrap_awaited_type(awaited);
            }
        } else if is_generator {
            let (return_types, is_never_returning) =
                self.check_and_aggregate_return_expression_types(fn_, check_mode);
            if is_never_returning {
                fallback_return_type = self.never_type;
            } else if !return_types.is_empty() {
                return_type = self.get_union_type_ex(
                    &return_types,
                    UnionReduction::SUBTYPE,
                    None,
                    TypeId::NIL,
                );
            }
            let (yield_types, next_types) =
                self.check_and_aggregate_yield_operand_types(fn_, check_mode);
            if !yield_types.is_empty() {
                yield_type = self.get_union_type_ex(
                    &yield_types,
                    UnionReduction::SUBTYPE,
                    None,
                    TypeId::NIL,
                );
            }
            if !next_types.is_empty() {
                next_type = self.get_intersection_type(&next_types);
            }
        } else {
            let (types, is_never_returning) =
                self.check_and_aggregate_return_expression_types(fn_, check_mode);
            if is_never_returning {
                // For an async function, the return type will not be never, but rather a Promise for never.
                if function_flags.intersects(FunctionFlags::ASYNC) {
                    let never_type = self.never_type;
                    return self.create_promise_return_type(fn_, never_type);
                }
                // Normal function
                return self.never_type;
            }
            if types.is_empty() {
                // For an async function, the return type will not be void/undefined, but rather a Promise for void/undefined.
                let contextual_return_type =
                    self.get_contextual_return_type(fn_, ContextFlags::NONE);
                let return_type: TypeId;
                let mut has_undefined = false;
                if contextual_return_type.is_some() {
                    let unwrapped = self.unwrap_return_type(contextual_return_type, function_flags);
                    let void_type = self.void_type;
                    let t = or_else_type_p22(unwrapped, void_type);
                    has_undefined = self.some_type(t, &mut |c: &mut Checker, t: TypeId| {
                        c.ty(t).flags.intersects(TypeFlags::UNDEFINED)
                    });
                }
                if has_undefined {
                    return_type = self.undefined_type;
                } else {
                    return_type = self.void_type;
                }
                if function_flags.intersects(FunctionFlags::ASYNC) {
                    return self.create_promise_return_type(fn_, return_type);
                }
                // Normal function
                return return_type;
            }
            // Return a union of the return expression types.
            return_type =
                self.get_union_type_ex(&types, UnionReduction::SUBTYPE, None, TypeId::NIL);
        }
        if return_type.is_some() || yield_type.is_some() || next_type.is_some() {
            if yield_type.is_some() {
                self.report_errors_from_widening(fn_, yield_type, WideningKind::GENERATOR_YIELD);
            }
            if return_type.is_some() {
                self.report_errors_from_widening(fn_, return_type, WideningKind::FUNCTION_RETURN);
            }
            if next_type.is_some() {
                self.report_errors_from_widening(fn_, next_type, WideningKind::GENERATOR_NEXT);
            }
            if return_type.is_some() && self.is_unit_type(return_type)
                || yield_type.is_some() && self.is_unit_type(yield_type)
                || next_type.is_some() && self.is_unit_type(next_type)
            {
                let contextual_signature =
                    self.get_contextual_signature_for_function_like_declaration(fn_);
                let mut contextual_type = TypeId::NIL;
                if contextual_signature.is_nil() {
                    // No contextual type
                } else if contextual_signature == self.get_signature_from_declaration(fn_) {
                    if !is_generator {
                        contextual_type = return_type;
                    }
                } else {
                    let sig_return_type = self.get_return_type_of_signature(contextual_signature);
                    contextual_type =
                        self.instantiate_contextual_type(sig_return_type, fn_, ContextFlags::NONE);
                }
                if is_generator {
                    yield_type = self
                        .get_widened_literal_like_type_for_contextual_iteration_type_if_needed(
                            yield_type,
                            contextual_type,
                            IterationTypeKind::YIELD,
                            is_async,
                        );
                    return_type = self
                        .get_widened_literal_like_type_for_contextual_iteration_type_if_needed(
                            return_type,
                            contextual_type,
                            IterationTypeKind::RETURN,
                            is_async,
                        );
                    next_type = self
                        .get_widened_literal_like_type_for_contextual_iteration_type_if_needed(
                            next_type,
                            contextual_type,
                            IterationTypeKind::NEXT,
                            is_async,
                        );
                } else {
                    return_type = self
                        .get_widened_literal_like_type_for_contextual_return_type_if_needed(
                            return_type,
                            contextual_type,
                            is_async,
                        );
                }
            }
            if yield_type.is_some() {
                yield_type = self.get_widened_type(yield_type);
            }
            if return_type.is_some() {
                return_type = self.get_widened_type(return_type);
            }
            if next_type.is_some() {
                next_type = self.get_widened_type(next_type);
            }
        }
        if return_type.is_nil() {
            return_type = fallback_return_type;
        }
        if is_generator {
            if yield_type.is_nil() {
                yield_type = self.never_type;
            }
            if next_type.is_nil() {
                next_type = self.get_contextual_iteration_type(IterationTypeKind::NEXT, fn_);
                if next_type.is_nil() {
                    next_type = self.unknown_type;
                }
            }
            return self.create_generator_type(yield_type, return_type, next_type, is_async);
        }
        // From within an async function you can return either a non-promise value or a promise. Any
        // Promise/A+ compatible implementation will always assimilate any foreign promise, so the
        // return type of the body is awaited type of the body, wrapped in a native Promise<T> type.
        if is_async {
            return self.create_promise_type(return_type);
        }
        return_type
    }

    // Returns the aggregated list of return types, plus a bool indicating a never-returning function.
    // Go: checker/checker.go:20695 checkAndAggregateReturnExpressionTypes
    pub fn check_and_aggregate_return_expression_types(
        &mut self,
        fn_: Node,
        check_mode: CheckMode,
    ) -> (Vec<TypeId>, bool) {
        let function_flags = get_function_flags(fn_);
        let mut aggregated_types: Vec<TypeId> = Vec::new();
        let mut has_return_with_no_expression = self.function_has_implicit_return(fn_);
        let mut has_return_of_type_never = false;
        for_each_return_statement(fn_.body(), &mut |return_statement: Node| -> bool {
            let mut expr = return_statement.expression();
            if expr.is_nil() {
                has_return_with_no_expression = true;
                return false;
            }
            expr = skip_parentheses(expr);
            // Bare calls to this same function don't contribute to inference
            // and `return await` is also safe to unwrap here
            if function_flags.intersects(FunctionFlags::ASYNC) && is_await_expression(expr) {
                expr = skip_parentheses(expr.expression());
            }
            // PORT: the Go `&&` chain is split so each call runs only when Go would run it.
            let mut is_bare_self_call = false;
            if is_call_expression(expr) && is_identifier(expr.expression()) {
                let callee_type = self.check_expression_cached(expr.expression());
                let merged = self.get_merged_symbol(fn_.symbol());
                if self.ty(callee_type).symbol == merged
                    && (!is_function_expression_or_arrow_function(
                        self.sym(fn_.symbol()).value_declaration,
                    ) || self.is_constant_reference(expr.expression()))
                {
                    is_bare_self_call = true;
                }
            }
            if is_bare_self_call {
                has_return_of_type_never = true;
                return false;
            }
            let mut t = self.check_expression_cached_ex(
                expr,
                check_mode.without(CheckMode::SKIP_GENERIC_FUNCTIONS),
            );
            if function_flags.intersects(FunctionFlags::ASYNC) {
                // From within an async function you can return either a non-promise value or a promise. Any
                // Promise/A+ compatible implementation will always assimilate any foreign promise, so the
                // return type of the body should be unwrapped to its awaited type, which should be wrapped in
                // the native Promise<T> type by the caller.
                let awaited = self.check_awaited_type(
                    t,
                    false, /*withAlias*/
                    fn_,
                    diag::The_return_type_of_an_async_function_must_either_be_a_valid_promise_or_must_not_contain_a_callable_then_member,
                );
                t = self.unwrap_awaited_type(awaited);
            }
            if self.ty(t).flags.intersects(TypeFlags::NEVER) {
                has_return_of_type_never = true;
            }
            if self.is_const_context(expr) {
                t = self.get_regular_type_of_literal_type(t);
            }
            append_if_unique_p22(&mut aggregated_types, t);
            false
        });
        if aggregated_types.is_empty()
            && !has_return_with_no_expression
            && (has_return_of_type_never || may_return_never(fn_))
        {
            return (Vec::new(), true);
        }
        if self.strict_null_checks && !aggregated_types.is_empty() && has_return_with_no_expression
        {
            let undefined_type = self.undefined_type;
            append_if_unique_p22(&mut aggregated_types, undefined_type);
        }
        (aggregated_types, false)
    }

    // Go: checker/checker.go:20743 functionHasImplicitReturn
    pub fn function_has_implicit_return(&mut self, fn_: Node) -> bool {
        let end_flow_node = fn_.end_flow_node();
        end_flow_node.is_some() && self.is_reachable_flow_node(end_flow_node)
    }
}

// Go: checker/checker.go:20748 mayReturnNever
pub fn may_return_never(fn_: Node) -> bool {
    match fn_.kind() {
        SyntaxKind::FunctionExpression | SyntaxKind::ArrowFunction => return true,
        SyntaxKind::MethodDeclaration => return is_object_literal_expression(fn_.parent()),
        _ => {}
    }
    false
}

impl Checker {
    // Go: checker/checker.go:20758 checkAndAggregateYieldOperandTypes
    pub fn check_and_aggregate_yield_operand_types(
        &mut self,
        fn_: Node,
        check_mode: CheckMode,
    ) -> (Vec<TypeId>, Vec<TypeId>) {
        let mut yield_types: Vec<TypeId> = Vec::new();
        let mut next_types: Vec<TypeId> = Vec::new();
        let is_async = get_function_flags(fn_).intersects(FunctionFlags::ASYNC);
        for_each_yield_expression(fn_.body(), &mut |yield_expr: Node| -> bool {
            let mut yield_expr_type = self.undefined_widening_type;
            if yield_expr.expression().is_some() {
                yield_expr_type = self.check_expression_ex(
                    yield_expr.expression(),
                    check_mode.without(CheckMode::SKIP_GENERIC_FUNCTIONS),
                );
            }
            if yield_expr.expression().is_some() && self.is_const_context(yield_expr.expression()) {
                yield_expr_type = self.get_regular_type_of_literal_type(yield_expr_type);
            }
            let any_type = self.any_type;
            let yielded = self.get_yielded_type_of_yield_expression(
                yield_expr,
                yield_expr_type,
                any_type,
                is_async,
            );
            append_if_unique_p22(&mut yield_types, yielded);
            let next_type: TypeId;
            if yield_expr.asterisk_token().is_some() {
                let use_ = if is_async {
                    IterationUse::ASYNC_YIELD_STAR
                } else {
                    IterationUse::YIELD_STAR
                };
                let iteration_types = self.get_iteration_types_of_iterable(
                    yield_expr_type,
                    use_,
                    yield_expr.expression(),
                );
                next_type = iteration_types.next_type;
            } else {
                next_type = self.get_contextual_type(yield_expr, ContextFlags::NONE);
            }
            if next_type.is_some() {
                append_if_unique_p22(&mut next_types, next_type);
            }
            false
        });
        (yield_types, next_types)
    }

    // Go: checker/checker.go:20784 createPromiseType
    pub fn create_promise_type(&mut self, promised_type: TypeId) -> TypeId {
        // creates a `Promise<T>` type where `T` is the promisedType argument
        let get_global_promise_type_checked = self.get_global_promise_type_checked.clone();
        let global_promise_type = get_global_promise_type_checked(self);
        if global_promise_type != self.empty_generic_type {
            // if the promised type is itself a promise, get the underlying type; otherwise, fallback to the promised type
            // Unwrap an `Awaited<T>` to `T` to improve inference.
            let unwrapped = self.unwrap_awaited_type(promised_type);
            let awaited = self.get_awaited_type_no_alias(unwrapped);
            let promised_type = or_else_type_p22(awaited, self.unknown_type);
            return self.create_type_reference(global_promise_type, &[promised_type]);
        }
        self.unknown_type
    }

    // Go: checker/checker.go:20796 createPromiseLikeType
    pub fn create_promise_like_type(&mut self, promised_type: TypeId) -> TypeId {
        // creates a `PromiseLike<T>` type where `T` is the promisedType argument
        let get_global_promise_like_type = self.get_global_promise_like_type.clone();
        let global_promise_like_type = get_global_promise_like_type(self);
        if global_promise_like_type != self.empty_generic_type {
            // if the promised type is itself a promise, get the underlying type; otherwise, fallback to the promised type
            // Unwrap an `Awaited<T>` to `T` to improve inference.
            let unwrapped = self.unwrap_awaited_type(promised_type);
            let awaited = self.get_awaited_type_no_alias(unwrapped);
            let promised_type = or_else_type_p22(awaited, self.unknown_type);
            return self.create_type_reference(global_promise_like_type, &[promised_type]);
        }
        self.unknown_type
    }

    // Go: checker/checker.go:20808 createPromiseReturnType
    pub fn create_promise_return_type(&mut self, fn_: Node, promised_type: TypeId) -> TypeId {
        let promise_type = self.create_promise_type(promised_type);
        if promise_type == self.unknown_type {
            let message: &'static Message = if is_import_call(fn_) {
                diag::A_dynamic_import_call_returns_a_Promise_Make_sure_you_have_a_declaration_for_Promise_or_include_ES2015_in_your_lib_option
            } else {
                diag::An_async_function_or_method_must_return_a_Promise_Make_sure_you_have_a_declaration_for_Promise_or_include_ES2015_in_your_lib_option
            };
            self.error(fn_, message, args![]);
            return self.error_type;
        }
        let get_global_promise_constructor_symbol =
            self.get_global_promise_constructor_symbol.clone();
        if get_global_promise_constructor_symbol(self).is_nil() {
            let message: &'static Message = if is_import_call(fn_) {
                diag::A_dynamic_import_call_in_ES5_requires_the_Promise_constructor_Make_sure_you_have_a_declaration_for_the_Promise_constructor_or_include_ES2015_in_your_lib_option
            } else {
                diag::An_async_function_or_method_in_ES5_requires_the_Promise_constructor_Make_sure_you_have_a_declaration_for_the_Promise_constructor_or_include_ES2015_in_your_lib_option
            };
            self.error(fn_, message, args![]);
        }
        promise_type
    }

    // Go: checker/checker.go:20824 unwrapReturnType
    pub fn unwrap_return_type(
        &mut self,
        return_type: TypeId,
        function_flags: FunctionFlags,
    ) -> TypeId {
        let is_generator = function_flags.intersects(FunctionFlags::GENERATOR);
        let is_async = function_flags.intersects(FunctionFlags::ASYNC);
        if is_generator {
            let return_iteration_type = self.get_iteration_type_of_generator_function_return_type(
                IterationTypeKind::RETURN,
                return_type,
                is_async,
            );
            if return_iteration_type.is_nil() {
                return self.error_type;
            }
            if is_async {
                let unwrapped = self.unwrap_awaited_type(return_iteration_type);
                return self.get_awaited_type_no_alias(unwrapped);
            }
            return return_iteration_type;
        }
        if is_async {
            let awaited = self.get_awaited_type_no_alias(return_type);
            return or_else_type_p22(awaited, self.error_type);
        }
        return_type
    }

    // Go: checker/checker.go:20843 getWidenedLiteralLikeTypeForContextualReturnTypeIfNeeded
    pub fn get_widened_literal_like_type_for_contextual_return_type_if_needed(
        &mut self,
        t: TypeId,
        contextual_signature_return_type: TypeId,
        is_async: bool,
    ) -> TypeId {
        let mut t = t;
        if t.is_some() && self.is_unit_type(t) {
            let mut contextual_type = TypeId::NIL;
            if contextual_signature_return_type.is_nil() {
                // No contextual type
            } else if is_async {
                contextual_type =
                    self.get_promised_type_of_promise(contextual_signature_return_type);
            } else {
                contextual_type = contextual_signature_return_type;
            }
            t = self.get_widened_literal_like_type_for_contextual_type(t, contextual_type);
        }
        t
    }

    // Go: checker/checker.go:20859 getWidenedLiteralLikeTypeForContextualIterationTypeIfNeeded
    pub fn get_widened_literal_like_type_for_contextual_iteration_type_if_needed(
        &mut self,
        t: TypeId,
        contextual_signature_return_type: TypeId,
        kind: IterationTypeKind,
        is_async_generator: bool,
    ) -> TypeId {
        let mut t = t;
        if t.is_some() && self.is_unit_type(t) {
            let mut contextual_type = TypeId::NIL;
            if contextual_signature_return_type.is_some() {
                contextual_type = self.get_iteration_type_of_generator_function_return_type(
                    kind,
                    contextual_signature_return_type,
                    is_async_generator,
                );
            }
            t = self.get_widened_literal_like_type_for_contextual_type(t, contextual_type);
        }
        t
    }

    // Go: checker/checker.go:20870 createGeneratorType
    pub fn create_generator_type(
        &mut self,
        yield_type: TypeId,
        return_type: TypeId,
        next_type: TypeId,
        is_async_generator: bool,
    ) -> TypeId {
        let resolver = if is_async_generator {
            self.async_iteration_types_resolver.clone()
        } else {
            self.sync_iteration_types_resolver.clone()
        };
        let global_generator_type = (resolver.get_global_generator_type)(self);
        let resolved_yield =
            (resolver.resolve_iteration_type)(self, yield_type, Node::NIL /*errorNode*/);
        let yield_type = or_else_type_p22(resolved_yield, self.unknown_type);
        let resolved_return =
            (resolver.resolve_iteration_type)(self, return_type, Node::NIL /*errorNode*/);
        let return_type = or_else_type_p22(resolved_return, self.unknown_type);
        if global_generator_type == self.empty_generic_type {
            // Fall back to the global IterableIterator type.
            let global_iterable_iterator_type = (resolver.get_global_iterable_iterator_type)(self);
            if global_iterable_iterator_type != self.empty_generic_type {
                return self.create_type_from_generic_global_type(
                    global_iterable_iterator_type,
                    &[yield_type, return_type, next_type],
                );
            }
            // The global Generator type doesn't exist, so report an error
            (resolver.get_global_iterable_iterator_type_checked)(self);
            return self.empty_object_type;
        }
        self.create_type_from_generic_global_type(
            global_generator_type,
            &[yield_type, return_type, next_type],
        )
    }
}
