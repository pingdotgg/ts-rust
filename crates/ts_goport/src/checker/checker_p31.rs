//! Port of `checker/checker.go` lines 27805-28752: indexed access and
//! conditional type simplification, normalization helpers, regular object
//! literal types, alias reference marking (linked references, JSX, decorator
//! metadata), external emit helpers, and the promised type of a promise.

use crate::prelude::*;

impl Checker {
    // Go: checker/checker.go:28440 distributeObjectOverIndexType
    pub fn distribute_object_over_index_type(
        &mut self,
        object_type: TypeId,
        index_type: TypeId,
        writing: bool,
    ) -> TypeId {
        // T[A | B] -> T[A] | T[B] (reading)
        // T[A | B] -> T[A] & T[B] (writing)
        if self.ty(index_type).flags.intersects(TypeFlags::UNION) {
            let types = self.map_constituents(index_type, &mut |c, t| {
                let indexed = c.get_indexed_access_type(object_type, t);
                c.get_simplified_type(indexed, writing)
            });
            if writing {
                return self.get_intersection_type(&types);
            }
            return self.get_union_type(&types);
        }
        TypeId::NIL
    }

    // Go: checker/checker.go:28455 distributeIndexOverObjectType
    pub fn distribute_index_over_object_type(
        &mut self,
        object_type: TypeId,
        index_type: TypeId,
        writing: bool,
    ) -> TypeId {
        // (T | U)[K] -> T[K] | U[K] (reading)
        // (T | U)[K] -> T[K] & U[K] (writing)
        // (T & U)[K] -> T[K] & U[K]
        let object_flags = self.ty(object_type).flags;
        if object_flags.intersects(TypeFlags::UNION)
            || object_flags.intersects(TypeFlags::INTERSECTION)
                && !self.should_defer_index_type(object_type, IndexFlags::NONE)
        {
            let types = self.map_constituents(object_type, &mut |c, t| {
                let indexed = c.get_indexed_access_type(t, index_type);
                c.get_simplified_type(indexed, writing)
            });
            if self
                .ty(object_type)
                .flags
                .intersects(TypeFlags::INTERSECTION)
                || writing
            {
                return self.get_intersection_type(&types);
            }
            return self.get_union_type(&types);
        }
        TypeId::NIL
    }

    // Go: checker/checker.go:28471 getSimplifiedConditionalType
    pub fn get_simplified_conditional_type(&mut self, t: TypeId, writing: bool) -> TypeId {
        let check_type = self.ty(t).as_conditional_type().check_type;
        let extends_type = self.ty(t).as_conditional_type().extends_type;
        let true_type = self.get_true_type_from_conditional_type(t);
        let false_type = self.get_false_type_from_conditional_type(t);
        // Simplifications for types of the form `T extends U ? T : never` and `T extends U ? never : T`.
        if self.ty(false_type).flags.intersects(TypeFlags::NEVER)
            && self.get_actual_type_variable(true_type) == self.get_actual_type_variable(check_type)
        {
            if self.ty(check_type).flags.intersects(TypeFlags::ANY) || {
                let source = self.get_restrictive_instantiation(check_type);
                let target = self.get_restrictive_instantiation(extends_type);
                self.is_type_assignable_to(source, target)
            } {
                return self.get_simplified_type(true_type, writing);
            } else if self.is_intersection_empty(check_type, extends_type) {
                return self.never_type;
            }
        } else if self.ty(true_type).flags.intersects(TypeFlags::NEVER)
            && self.get_actual_type_variable(false_type)
                == self.get_actual_type_variable(check_type)
        {
            if !self.ty(check_type).flags.intersects(TypeFlags::ANY) && {
                let source = self.get_restrictive_instantiation(check_type);
                let target = self.get_restrictive_instantiation(extends_type);
                self.is_type_assignable_to(source, target)
            } {
                return self.never_type;
            } else if self.ty(check_type).flags.intersects(TypeFlags::ANY)
                || self.is_intersection_empty(check_type, extends_type)
            {
                return self.get_simplified_type(false_type, writing);
            }
        }
        t
    }

    // Invokes union simplification logic to determine if an intersection is considered empty as a union constituent
    // Go: checker/checker.go:28494 isIntersectionEmpty
    pub fn is_intersection_empty(&mut self, type1: TypeId, type2: TypeId) -> bool {
        let intersected = self.intersect_types(type1, type2);
        let never_type = self.never_type;
        let u = self.get_union_type(&[intersected, never_type]);
        self.ty(u).flags.intersects(TypeFlags::NEVER)
    }

    // Go: checker/checker.go:28498 getSimplifiedTypeOrConstraint
    pub fn get_simplified_type_or_constraint(&mut self, t: TypeId) -> TypeId {
        let simplified = self.get_simplified_type(t, false /*writing*/);
        if simplified != t {
            return simplified;
        }
        self.get_constraint_of_type(t)
    }

    // Go: checker/checker.go:28505 getNormalizedUnionOrIntersectionType
    pub fn get_normalized_union_or_intersection_type(
        &mut self,
        t: TypeId,
        writing: bool,
    ) -> TypeId {
        let reduced = self.get_reduced_type(t);
        if reduced != t {
            return reduced;
        }
        if self.ty(t).flags.intersects(TypeFlags::INTERSECTION)
            && self.should_normalize_intersection(t)
        {
            // Normalization handles cases like
            // Partial<T>[K] & ({} | null) ==>
            // Partial<T>[K] & {} | Partial<T>[K} & null ==>
            // (T[K] | undefined) & {} | (T[K] | undefined) & null ==>
            // T[K] & {} | undefined & {} | T[K] & null | undefined & null ==>
            // T[K] & {} | T[K] & null
            // PORT: Go `core.SameMap` + `core.Same`: the result differs only
            // when some element was changed by the mapping.
            let count = self.ty(t).types().len();
            if let Some(normalized_types) =
                self.map_stored_types_if_changed(t, count, Checker::type_at, &mut |c, u| {
                    c.get_normalized_type(u, writing)
                })
            {
                return self.get_intersection_type(&normalized_types);
            }
        }
        t
    }

    // Go: checker/checker.go:28525 shouldNormalizeIntersection
    pub fn should_normalize_intersection(&mut self, t: TypeId) -> bool {
        let mut has_instantiable = false;
        let mut has_nullable_or_empty = false;
        for i in 0..self.ty(t).types().len() {
            let t = self.type_at(t, i);
            has_instantiable =
                has_instantiable || self.ty(t).flags.intersects(TypeFlags::INSTANTIABLE);
            has_nullable_or_empty = has_nullable_or_empty
                || self.ty(t).flags.intersects(TypeFlags::NULLABLE)
                || self.is_empty_anonymous_object_type(t);
            if has_instantiable && has_nullable_or_empty {
                return true;
            }
        }
        false
    }

    // Go: checker/checker.go:28538 getNormalizedTupleType
    pub fn get_normalized_tuple_type(&mut self, t: TypeId, writing: bool) -> TypeId {
        let elements = self.get_element_types(t);
        // PORT: Go `core.SameMap` + `core.Same`.
        let mut normalized_elements: Vec<TypeId> = Vec::with_capacity(elements.len());
        for &e in &elements {
            if self.ty(e).flags.intersects(TypeFlags::SIMPLIFIABLE) {
                normalized_elements.push(self.get_simplified_type(e, writing));
            } else {
                normalized_elements.push(e);
            }
        }
        if elements[..] != normalized_elements[..] {
            let target = self.ty(t).target();
            return self.create_normalized_tuple_type(target, &normalized_elements);
        }
        t
    }

    // Go: checker/checker.go:28552 getSingleBaseForNonAugmentingSubtype
    pub fn get_single_base_for_non_augmenting_subtype(&mut self, t: TypeId) -> TypeId {
        if !self.ty(t).object_flags.intersects(ObjectFlags::REFERENCE) {
            return TypeId::NIL;
        }
        let target = self.ty(t).target();
        if !self
            .ty(target)
            .object_flags
            .intersects(ObjectFlags::CLASS_OR_INTERFACE)
        {
            return TypeId::NIL;
        }
        let key = CachedTypeKey {
            kind: CachedTypeKind::EQUIVALENT_BASE_TYPE,
            type_id: t,
        };
        if self
            .ty(t)
            .object_flags
            .intersects(ObjectFlags::IDENTICAL_BASE_TYPE_CALCULATED)
        {
            return self.cached_types.get(&key).copied().unwrap_or_default();
        }
        self.ty_mut(t).object_flags |= ObjectFlags::IDENTICAL_BASE_TYPE_CALCULATED;
        if self.ty(target).object_flags.intersects(ObjectFlags::CLASS) {
            let base_type_node = self.get_base_type_node_of_class(target);
            // A base type expression may circularly reference the class itself (e.g. as an argument to function call), so we only
            // check for base types specified as simple qualified names.
            if base_type_node.is_some()
                && !is_identifier(base_type_node.expression())
                && !is_property_access_expression(base_type_node.expression())
            {
                return TypeId::NIL;
            }
        }
        let bases = self.get_base_types(target);
        if bases.len() != 1 {
            return TypeId::NIL;
        }
        let symbol = self.ty(t).symbol;
        let members = self.get_members_of_symbol(symbol);
        if self.symbols.len(members) != 0 {
            // If the interface has any members, they may subtype members in the base, so we should do a full structural comparison
            return TypeId::NIL;
        }
        let instantiated_base: TypeId;
        let type_parameters = self
            .ty(target)
            .as_interface_type()
            .type_parameters()
            .to_vec();
        if type_parameters.is_empty() {
            instantiated_base = bases[0];
        } else {
            let type_arguments = self.get_type_arguments(t);
            let mapper =
                self.new_type_mapper(&type_parameters, &type_arguments[..type_parameters.len()]);
            instantiated_base = self.instantiate_type(bases[0], mapper);
        }
        let mut instantiated_base = instantiated_base;
        let last = {
            let type_arguments = self.type_arguments_of(t);
            (type_arguments.len() > type_parameters.len())
                .then(|| type_arguments.last().copied().unwrap_or(TypeId::NIL))
        };
        if let Some(last) = last {
            instantiated_base = self.get_type_with_this_argument(instantiated_base, last, false);
        }
        self.cached_types.insert(key, instantiated_base);
        instantiated_base
    }

    // Go: checker/checker.go:28592 getModifiersTypeFromMappedType
    pub fn get_modifiers_type_from_mapped_type(&mut self, t: TypeId) -> TypeId {
        if self.ty(t).as_mapped_type().modifiers_type.is_nil() {
            let modifiers_type: TypeId;
            if self.is_mapped_type_with_keyof_constraint_declaration(t) {
                // If the constraint declaration is a 'keyof T' node, the modifiers type is T. We check
                // AST nodes here because, when T is a non-generic type, the logic below eagerly resolves
                // 'keyof T' to a literal union type and we can't recover T from that type.
                let constraint_declaration = self.get_constraint_declaration_for_mapped_type(t);
                let from_node = self.get_type_from_type_node(constraint_declaration.type_());
                let mapper = self.ty(t).as_object_type().mapper;
                modifiers_type = self.instantiate_type(from_node, mapper);
            } else {
                // Otherwise, get the declared constraint type, and if the constraint type is a type parameter,
                // get the constraint of that type parameter. If the resulting type is an indexed type 'keyof T',
                // the modifiers type is T. Otherwise, the modifiers type is unknown.
                let declaration = self.ty(t).as_mapped_type().declaration;
                let declared_type = self.get_type_from_mapped_type_node(declaration);
                let constraint = self.get_constraint_type_from_mapped_type(declared_type);
                let mut extended_constraint = constraint;
                if constraint.is_some()
                    && self
                        .ty(constraint)
                        .flags
                        .intersects(TypeFlags::TYPE_PARAMETER)
                {
                    let constraint = self.get_non_distributed_type_parameter(constraint);
                    extended_constraint = self.get_constraint_of_type_parameter(constraint);
                }
                if extended_constraint.is_some()
                    && self
                        .ty(extended_constraint)
                        .flags
                        .intersects(TypeFlags::INDEX)
                {
                    let index_target = self.ty(extended_constraint).as_index_type().target;
                    let mapper = self.ty(t).as_object_type().mapper;
                    modifiers_type = self.instantiate_type(index_target, mapper);
                } else {
                    modifiers_type = self.unknown_type;
                }
            }
            self.ty_mut(t).as_mapped_type_mut().modifiers_type = modifiers_type;
        }
        self.ty(t).as_mapped_type().modifiers_type
    }

    // Go: checker/checker.go:28620 extractTypesOfKind
    pub fn extract_types_of_kind(&mut self, t: TypeId, kind: TypeFlags) -> TypeId {
        self.filter_type(t, &mut |c: &mut Checker, t: TypeId| {
            c.ty(t).flags.intersects(kind)
        })
    }

    // Go: checker/checker.go:28624 getRegularTypeOfObjectLiteral
    pub fn get_regular_type_of_object_literal(&mut self, t: TypeId) -> TypeId {
        if !(self.is_object_literal_type(t)
            && self
                .ty(t)
                .object_flags
                .intersects(ObjectFlags::FRESH_LITERAL))
        {
            return t;
        }
        let key = CachedTypeKey {
            kind: CachedTypeKind::REGULAR_OBJECT_LITERAL,
            type_id: t,
        };
        if let Some(&cached) = self.cached_types.get(&key) {
            if cached.is_some() {
                return cached;
            }
        }
        self.resolve_structured_type_members(t);
        let members = self.transform_type_of_members(t, &mut |c: &mut Checker, pt: TypeId| {
            c.get_regular_type_of_object_literal(pt)
        });
        // PORT: Go `resolved` is the StructuredType embedded in `t`, so
        // `resolved.flags`/`resolved.objectFlags` are `t`'s flags.
        let symbol = self.ty(t).symbol;
        let (call_signatures, construct_signatures, index_infos) = {
            let resolved = self.ty(t).as_structured_type();
            (
                resolved.call_signatures().to_vec(),
                resolved.construct_signatures().to_vec(),
                resolved.index_infos_list(),
            )
        };
        let regular = self.new_anonymous_type(
            symbol,
            members,
            &call_signatures,
            &construct_signatures,
            &index_infos,
        );
        let resolved_flags = self.ty(t).flags;
        let resolved_object_flags = self.ty(t).object_flags;
        self.ty_mut(regular).flags = resolved_flags;
        self.ty_mut(regular).object_flags |=
            resolved_object_flags.without(ObjectFlags::FRESH_LITERAL);
        self.cached_types.insert(key, regular);
        regular
    }

    // Go: checker/checker.go:28641 transformTypeOfMembers
    pub fn transform_type_of_members(
        &mut self,
        t: TypeId,
        f: &mut dyn FnMut(&mut Checker, TypeId) -> TypeId,
    ) -> SymbolTable {
        let members = self.symbols.new_table();
        for property in self.get_properties_of_object_type(t) {
            let mut property = property;
            let original = self.get_type_of_symbol(property);
            let updated = f(self, original);
            if updated != original {
                property = self.create_symbol_with_type(property, updated);
            }
            let name = self.sym(property).name.clone();
            self.symbols.set(members, name, property);
        }
        members
    }

    // Go: checker/checker.go:28654 markLinkedReferences
    pub fn mark_linked_references(
        &mut self,
        location: Node,
        hint: ReferenceHint,
        prop_symbol: SymbolId,
        parent_type: TypeId,
    ) {
        if !self.can_collect_symbol_alias_accessibility_data {
            return;
        }
        if location.flags().intersects(NodeFlags::AMBIENT)
            && !is_property_signature_declaration(location)
            && !is_property_declaration(location)
        {
            // References within types and declaration files are never going to contribute to retaining a JS import,
            // except for properties (which can be decorated).
            return;
        }
        match hint {
            ReferenceHint::IDENTIFIER => {
                self.mark_identifier_alias_referenced(location);
            }
            ReferenceHint::PROPERTY => {
                self.mark_property_alias_referenced(location, prop_symbol, parent_type);
            }
            ReferenceHint::EXPORT_ASSIGNMENT => {
                self.mark_export_assignment_alias_referenced(location);
            }
            ReferenceHint::JSX => {
                self.mark_jsx_alias_referenced(location);
            }
            ReferenceHint::EXPORT_IMPORT_EQUALS => {
                self.mark_import_equals_alias_referenced(location);
            }
            ReferenceHint::EXPORT_SPECIFIER => {
                self.mark_export_specifier_alias_referenced(location);
            }
            ReferenceHint::DECORATOR => {
                self.mark_decorator_alias_referenced(location);
            }
            ReferenceHint::UNSPECIFIED => {
                if location.flags().intersects(NodeFlags::IN_WITH_STATEMENT) {
                    // We cannot answer semantic questions within a with block, do not proceed any further
                    return;
                }
                if is_jsx_tag_name(location) && is_jsx_intrinsic_tag_name(location) {
                    return; // builtin JSX tag names aren't real type refs by most metrics, but are expressions, so must be filtered
                }
                if is_identifier(location) {
                    // A shorthand property with an object-assignment-initializer (e.g. `{ s = 5 }`) is only valid inside a
                    // destructuring assignment target. When it appears in an ordinary object literal expression, the checker
                    // checks the initializer and never resolves the property name, so resolving it here would report a spurious
                    // "No value exists in scope for the shorthand property" diagnostic. Skip such names to match checking.
                    let parent = location.parent();
                    if is_shorthand_property_assignment(parent)
                        && parent.name() == location
                        && parent.object_assignment_initializer().is_some()
                        && !is_assignment_target(parent.parent())
                    {
                        return;
                    }
                    let res = find_many_ancestors(
                        location,
                        &[
                            is_meta_property,
                            is_decorator,
                            is_for_in_or_of_statement,
                            is_computed_property_name,
                            is_heritage_clause,
                        ],
                    );
                    let meta_property = res[0];
                    let decorator = res[1];
                    let for_node = res[2];
                    let computed_name = res[3];
                    let heritage_clause = res[4];
                    if meta_property.is_some() {
                        return; // identifiers in meta properties shouldn't be resolved, but are expressions, so must be filtered
                    }
                    if decorator.is_some() {
                        // Decorators on nodes that cannot be decorated (e.g. class expressions, static blocks,
                        // `this` parameters) are never resolved during normal checking, so resolving them here would
                        // report spurious diagnostics. Only bail out for such invalid-position decorators; valid
                        // decorator expressions must still be resolved and marked for emit.
                        let decorated = decorator.parent();
                        if decorated.is_some()
                            && !node_can_be_decorated(
                                self.legacy_decorators,
                                decorated,
                                decorated.parent(),
                                decorated.parent().parent(),
                            )
                        {
                            return;
                        }
                    }
                    // The right-hand side of a 'for-in'/'for-of' statement whose initializer is an empty variable
                    // declaration list (a grammar error, e.g. `for (var of X)`) is never checked, because the RHS
                    // is only checked while inferring the type of a variable declaration and there is none here.
                    // Resolving identifiers in the RHS here would report spurious diagnostics.
                    if for_node.is_some() {
                        let initializer = for_node.initializer();
                        let expression = for_node.expression();
                        if is_variable_declaration_list(initializer)
                            && initializer.declarations().nodes().len() == 0
                            && expression.is_some()
                            && (location == expression
                                || is_node_descendant_of(location, expression))
                        {
                            return;
                        }
                    }
                    // ts#64674, Go N' checker.go:28798: enum member names are checked now (checkEnumMember).
                    if computed_name.is_some() {
                        if is_invalid_computed_property_name(computed_name) {
                            return;
                        }
                    }
                    if heritage_clause.is_some() {
                        // extends heritage clauses on interfaces are not expressions and are unchecked if they are
                        if is_interface_declaration(heritage_clause.parent()) {
                            return;
                        }
                        // On a class, only the first `extends` type is resolved as a value (the base class); any
                        // additional `extends` types are grammar errors (e.g. `class C extends A extends B` or
                        // `class C extends A, B`) and are never resolved during checking.
                        if is_class_like(heritage_clause.parent())
                            && heritage_clause.token() == SyntaxKind::ExtendsKeyword
                        {
                            let first_extends =
                                get_class_extends_heritage_element(heritage_clause.parent());
                            if first_extends.is_some()
                                && location != first_extends
                                && !is_node_descendant_of(location, first_extends)
                            {
                                return;
                            }
                        }
                    }
                    // Identifiers in expression contexts are emitted, so we need to follow their referenced aliases and mark them as used
                    // Some non-expression identifiers are also treated as expression identifiers for this purpose, eg, `a` in `b = {a}` or `q` in `import r = q`
                    // This is the exception, rather than the rule - most non-expression identifiers are declaration names.
                    if (is_expression_node(location)
                        || is_shorthand_property_assignment(location.parent()))
                        && should_mark_identifier_alias_referenced(location)
                    {
                        if is_property_access_or_qualified_name(location.parent()) {
                            let left = if is_property_access_expression(location.parent()) {
                                location.parent().expression()
                            } else {
                                location.parent().left()
                            };
                            if left != location {
                                return; // Only mark the LHS (the RHS is a property lookup)
                            }
                        }
                        self.mark_identifier_alias_referenced(location);
                        return;
                    }
                }
                if is_property_access_or_qualified_name(location) {
                    let mut top_prop = location;
                    while is_property_access_or_qualified_name(top_prop) {
                        // Names in an import type's qualifier (`ns.y` in `typeof import("./b").ns.y`) are exports of the imported module, not references to this file's imports
                        // (ts#64636, Go N' checker.go:28844)
                        if is_part_of_type_node(top_prop)
                            || is_import_type_qualifier_part(top_prop).is_some()
                        {
                            return;
                        }
                        top_prop = top_prop.parent();
                    }
                    self.mark_property_alias_referenced(
                        location,
                        SymbolId::NIL, /*propSymbol*/
                        TypeId::NIL,   /*parentType*/
                    );
                    return;
                }
                if is_export_assignment(location) {
                    self.mark_export_assignment_alias_referenced(location);
                    return;
                }
                if is_jsx_opening_like_element(location) || is_jsx_opening_fragment(location) {
                    self.mark_jsx_alias_referenced(location);
                    return;
                }
                if is_import_equals_declaration(location) {
                    if is_internal_module_import_equals_declaration_p31(location)
                        || self.check_external_import_or_export_declaration(location)
                    {
                        self.mark_import_equals_alias_referenced(location);
                        return;
                    }
                    return;
                }
                if is_export_specifier(location) {
                    self.mark_export_specifier_alias_referenced(location);
                    return;
                }
                if !self.compiler_options.emit_decorator_metadata.is_true() {
                    return;
                }
                if !can_have_decorators(location)
                    || !has_decorators(location)
                    || location.modifiers().is_nil()
                    || !node_can_be_decorated(
                        self.legacy_decorators,
                        location,
                        location.parent(),
                        location.parent().parent(),
                    )
                {
                    return;
                }

                self.mark_decorator_alias_referenced(location);
            }
            _ => panic!("Unhandled reference hint"),
        }
    }
}

// Go: checker/checker.go:28821 isExportOrExportExpression
pub fn is_export_or_export_expression(location: Node) -> bool {
    find_ancestor(location, |n: Node| {
        let parent = n.parent();
        if parent.is_some() {
            if is_any_export_assignment(parent) {
                return parent.expression() == n && is_entity_name_expression(n);
            }
            if is_export_specifier(parent) {
                return parent.name() == n || parent.property_name() == n;
            }
        }
        false
    })
    .is_some()
}

// Go: checker/checker.go:28836 shouldMarkIdentifierAliasReferenced
// PERF: chkA. The parent and the great-grandparent come with their kinds
// (`node_parent_and_kind`, one store lookup each), so each node is looked up
// once. The tests and their order are Go's.
pub fn should_mark_identifier_alias_referenced(node: Node) -> bool {
    let (parent, parent_kind) = node_parent_and_kind(node);
    if parent.is_some() {
        // A property access expression LHS? checkPropertyAccessExpression will handle that.
        if parent_kind == SyntaxKind::PropertyAccessExpression && parent.expression() == node {
            return false;
        }
        // Next two check for an identifier inside a type only export.
        if parent_kind == SyntaxKind::ExportSpecifier && parent.is_type_only() {
            return false;
        }
        let grandparent = parent.parent();
        if grandparent.is_some() {
            let (great_grandparent, great_grandparent_kind) = node_parent_and_kind(grandparent);
            if great_grandparent_kind == SyntaxKind::ExportDeclaration
                && great_grandparent.is_type_only()
            {
                return false;
            }
        }
    }
    true
}

// Go: checker/checker.go:28857 isInternalModuleImportEqualsDeclaration
// PORT: `ast.IsInternalModuleImportEqualsDeclaration` already ports to the
// free fn `is_internal_module_import_equals_declaration` (ast/utilities).
// This checker-package copy is private and suffixed so the prelude globs do
// not see two items with one name.
fn is_internal_module_import_equals_declaration_p31(node: Node) -> bool {
    node.kind() == SyntaxKind::ImportEqualsDeclaration
        && node.module_reference().kind() != SyntaxKind::ExternalModuleReference
}

impl Checker {
    // Go: checker/checker.go:28862 markIdentifierAliasReferenced
    pub fn mark_identifier_alias_referenced(&mut self, location: Node) {
        if is_this_in_type_query(location) {
            return;
        }
        let symbol = self.get_resolved_symbol(location);
        if symbol.is_some() && symbol != self.arguments_symbol && symbol != self.unknown_symbol {
            self.mark_alias_referenced(symbol, location);
        }
    }

    // Go: checker/checker.go:28872 markPropertyAliasReferenced
    pub fn mark_property_alias_referenced(
        &mut self,
        location: Node, /*PropertyAccessExpression | QualifiedName*/
        prop_symbol: SymbolId,
        parent_type: TypeId,
    ) {
        if is_part_of_import_equals_module_reference(location) {
            return;
        }
        let left = if is_property_access_expression(location) {
            location.expression()
        } else {
            location.left()
        };
        if is_this_identifier(left) || !is_identifier(left) {
            return;
        }
        let parent_symbol = self.get_resolved_symbol(left);
        if parent_symbol.is_nil() || parent_symbol == self.unknown_symbol {
            return;
        }
        // In `Foo.Bar.Baz`, 'Foo' is not referenced if 'Bar' is a const enum or a module containing only const enums.
        // `Foo` is also not referenced in `enum FooCopy { Bar = Foo.Bar }`, because the enum member value gets inlined
        // here even if `Foo` is not a const enum.
        //
        // The exceptions are:
        //   1. if 'isolatedModules' is enabled, because the const enum value will not be inlined, and
        //   2. if 'preserveConstEnums' is enabled and the expression is itself an export, e.g. `export = Foo.Bar.Baz`.
        //
        // The property lookup is deferred as much as possible, in as many situations as possible, to avoid alias marking
        // pulling on types/symbols it doesn't strictly need to.
        if self.compiler_options.get_isolated_modules()
            || (self.compiler_options.should_preserve_const_enums()
                && is_export_or_export_expression(location))
        {
            self.mark_alias_referenced(parent_symbol, location);
            return;
        }
        // Hereafter, this relies on type checking - but every check prior to this only used symbol information
        let mut left_type = parent_type;
        if left_type.is_nil() {
            left_type = self.check_expression_cached(left);
        }
        if self.is_type_any(left_type) || left_type == self.silent_never_type {
            self.mark_alias_referenced(parent_symbol, location);
            return;
        }
        let mut prop = prop_symbol;
        if prop.is_nil() && parent_type.is_nil() {
            let right = if is_property_access_expression(location) {
                location.name()
            } else {
                location.right()
            };
            let mut lexically_scoped_symbol = SymbolId::NIL;
            if is_private_identifier(right) {
                lexically_scoped_symbol =
                    self.lookup_symbol_for_private_identifier_declaration(right.text(), right);
            }
            let assignment_kind = get_assignment_target_kind(location);
            let apparent_type = if assignment_kind != AssignmentKind::NONE
                || self.is_method_access_for_call(location)
            {
                let widened = self.get_widened_type(left_type);
                self.get_apparent_type(widened)
            } else {
                self.get_apparent_type(left_type)
            };
            if is_private_identifier(right) {
                if lexically_scoped_symbol.is_some() {
                    prop = self.get_private_identifier_property_of_type(
                        apparent_type,
                        lexically_scoped_symbol,
                    );
                }
            } else {
                prop = self.get_property_of_type(apparent_type, right.text());
            }
        }
        if !(prop.is_some()
            && (self.is_const_enum_or_const_enum_only_module(prop)
                || self.sym(prop).flags.intersects(SymbolFlags::ENUM_MEMBER)
                    && location.parent().kind() == SyntaxKind::EnumMember))
        {
            self.mark_alias_referenced(parent_symbol, location);
        }
    }
}

// Go: checker/checker.go:28944 isPartOfImportEqualsModuleReference
pub fn is_part_of_import_equals_module_reference(location: Node) -> bool {
    let import_equals = find_ancestor_kind(location, SyntaxKind::ImportEqualsDeclaration);
    if import_equals.is_nil() {
        return false;
    }
    let mut node = location;
    while node.is_some() && node != import_equals {
        if node == import_equals.module_reference() {
            return true;
        }
        node = node.parent();
    }
    false
}

impl Checker {
    // Go: checker/checker.go:28957 markExportAssignmentAliasReferenced
    pub fn mark_export_assignment_alias_referenced(
        &mut self,
        location: Node, /*ExportAssignment*/
    ) {
        let id = location.expression();
        if is_identifier(id) {
            let resolved = self.resolve_entity_name(
                id,
                SymbolFlags::ALL,
                true, /*ignoreErrors*/
                true, /*dontResolveAlias*/
                location,
            );
            let sym = self.get_export_symbol_of_value_symbol_if_exported(resolved);
            if sym.is_some() {
                self.mark_alias_referenced(sym, id);
            }
        }
    }

    // Go: checker/checker.go:28967 markJsxAliasReferenced
    pub fn mark_jsx_alias_referenced(
        &mut self,
        node: Node, /*JsxOpeningLikeElement | JsxOpeningFragment*/
    ) {
        if self
            .get_jsx_namespace_container_for_implicit_import(node)
            .is_some()
        {
            return;
        }
        // The reactNamespace/jsxFactory's root symbol should be marked as 'used' so we don't incorrectly elide its import.
        // And if there is no reactNamespace/jsxFactory's symbol in scope when targeting React emit, we should issue an error.
        let jsx_factory_ref_err: Option<&'static crate::diagnostics::Message> =
            if self.compiler_options.jsx == JsxEmit::REACT {
                Some(diag::This_JSX_tag_requires_0_to_be_in_scope_but_it_could_not_be_found)
            } else {
                None
            };
        let jsx_factory_namespace = self.get_jsx_namespace(node);
        let mut jsx_factory_location = node;
        if is_jsx_opening_like_element(node) {
            jsx_factory_location = node.tag_name();
        }
        let should_factory_ref_err = self.compiler_options.jsx != JsxEmit::PRESERVE
            && self.compiler_options.jsx != JsxEmit::REACT_NATIVE;
        // #38720/60122, allow null as jsxFragmentFactory
        let mut jsx_factory_sym = SymbolId::NIL;
        if !(is_jsx_opening_fragment(node) && jsx_factory_namespace == "null") {
            let mut flags = SymbolFlags::VALUE;
            if !should_factory_ref_err {
                flags = flags.without(SymbolFlags::ENUM);
            }
            jsx_factory_sym = self.resolve_name(
                jsx_factory_location,
                &jsx_factory_namespace,
                flags,
                jsx_factory_ref_err,
                true,  /*isUse*/
                false, /*excludeGlobals*/
            );
        }
        if jsx_factory_sym.is_some() {
            // Mark local symbol as referenced here because it might not have been marked
            // if jsx emit was not jsxFactory as there wont be error being emitted
            self.symbol_referenced(jsx_factory_sym, SymbolFlags::ALL);
            // If react/jsxFactory symbol is alias, mark it as referenced
            if self.can_collect_symbol_alias_accessibility_data
                && self
                    .sym(jsx_factory_sym)
                    .flags
                    .intersects(SymbolFlags::ALIAS)
                && self
                    .get_type_only_alias_declaration(jsx_factory_sym)
                    .is_nil()
            {
                self.mark_alias_symbol_as_referenced(jsx_factory_sym);
            }
        }
        // if JsxFragment, additionally mark jsx pragma as referenced, since `getJsxNamespace` above would have resolved to only the fragment factory if they are distinct
        if is_jsx_opening_fragment(node) {
            let file = get_source_file_of_node(node);
            let entity = self.get_jsx_factory_entity(file);
            if entity.is_some() {
                let local_jsx_namespace = get_first_identifier(entity).text();
                let mut flags = SymbolFlags::VALUE;
                if !should_factory_ref_err {
                    flags = flags.without(SymbolFlags::ENUM);
                }
                self.resolve_name(
                    jsx_factory_location,
                    local_jsx_namespace,
                    flags,
                    jsx_factory_ref_err,
                    true,  /*isUse*/
                    false, /*excludeGlobals*/
                );
            }
        }
    }

    // Go: checker/checker.go:29013 markImportEqualsAliasReferenced
    pub fn mark_import_equals_alias_referenced(
        &mut self,
        location: Node, /*ImportEqualsDeclaration*/
    ) {
        if has_syntactic_modifier(location, ModifierFlags::EXPORT) {
            self.mark_export_as_referenced(location);
        }
    }

    // Go: checker/checker.go:29019 markExportSpecifierAliasReferenced
    pub fn mark_export_specifier_alias_referenced(
        &mut self,
        location: Node, /*ExportSpecifier*/
    ) {
        if location.parent().parent().module_specifier().is_nil()
            && !location.is_type_only()
            && !location.parent().parent().is_type_only()
        {
            let exported_name = location.property_name_or_name();
            if exported_name.kind() == SyntaxKind::StringLiteral {
                return; // Skip for invalid syntax like this: export { "x" }
            }
            let symbol = self.resolve_name(
                exported_name,
                exported_name.text(),
                SymbolFlags::VALUE
                    | SymbolFlags::TYPE
                    | SymbolFlags::NAMESPACE
                    | SymbolFlags::ALIAS,
                None,  /*nameNotFoundMessage*/
                true,  /*isUse*/
                false, /*excludeGlobals*/
            );
            if symbol.is_some()
                && (symbol == self.undefined_symbol
                    || symbol == self.global_this_symbol
                    || !self.sym(symbol).declarations.is_empty()
                        && is_global_source_file(get_declaration_container(
                            self.sym(symbol).declarations[0],
                        )))
            {
                // Do nothing, non-local symbol
            } else {
                let mut target = symbol;
                if target.is_some() && self.sym(target).flags.intersects(SymbolFlags::ALIAS) {
                    target = self.resolve_alias(target);
                }
                if target.is_nil() || self.get_symbol_flags(target).intersects(SymbolFlags::VALUE) {
                    self.mark_export_as_referenced(location); // marks export as used
                    self.mark_identifier_alias_referenced(exported_name); // marks target of export as used
                }
            }
        }
    }

    // Go: checker/checker.go:29041 checkExternalEmitHelpers
    pub fn check_external_emit_helpers(&mut self, location: Node, helpers: ExternalEmitHelpers) {
        if !self.compiler_options.import_helpers.is_true() {
            return;
        }
        let source_file = get_source_file_of_node(location);
        if !is_effective_external_module(source_file, self.compiler_options)
            || location.flags().intersects(NodeFlags::AMBIENT)
        {
            return;
        }
        let helpers_module = self.resolve_helpers_module(source_file, location);
        if helpers_module == self.unknown_symbol {
            return;
        }
        let requested = self
            .source_file_links
            .get(source_file)
            .requested_external_emit_helpers;
        if !requested.contains(helpers) {
            let unchecked_helpers = helpers.without(requested);
            let mut helper = ExternalEmitHelpers::FIRST_EMIT_HELPER;
            while helper <= ExternalEmitHelpers::LAST_EMIT_HELPER {
                if unchecked_helpers.intersects(helper) {
                    for name in self.get_helper_names(helper) {
                        let exports = self.get_exports_of_module(helpers_module);
                        let found = self.get_symbol(exports, &name, SymbolFlags::VALUE);
                        let symbol = self.resolve_symbol(found);
                        if symbol.is_nil() {
                            self.error(
                                location,
                                diag::This_syntax_requires_an_imported_helper_named_1_which_does_not_exist_in_0_Consider_upgrading_your_version_of_0,
                                args![EXTERNAL_HELPERS_MODULE_NAME_TEXT, name],
                            );
                        } else if helper.intersects(ExternalEmitHelpers::CLASS_PRIVATE_FIELD_GET) {
                            if !self.has_signature_with_arity_greater_than(symbol, 3) {
                                self.error(
                                    location,
                                    diag::This_syntax_requires_an_imported_helper_named_1_with_2_parameters_which_is_not_compatible_with_the_one_in_0_Consider_upgrading_your_version_of_0,
                                    args![EXTERNAL_HELPERS_MODULE_NAME_TEXT, name, 4],
                                );
                            }
                        } else if helper.intersects(ExternalEmitHelpers::CLASS_PRIVATE_FIELD_SET) {
                            if !self.has_signature_with_arity_greater_than(symbol, 4) {
                                self.error(
                                    location,
                                    diag::This_syntax_requires_an_imported_helper_named_1_with_2_parameters_which_is_not_compatible_with_the_one_in_0_Consider_upgrading_your_version_of_0,
                                    args![EXTERNAL_HELPERS_MODULE_NAME_TEXT, name, 5],
                                );
                            }
                        }
                    }
                }
                helper = ExternalEmitHelpers(helper.0 << 1);
            }
        }
        self.source_file_links
            .get(source_file)
            .requested_external_emit_helpers |= helpers;
    }

    // Go: checker/checker.go:29079 hasSignatureWithArityGreaterThan
    pub fn has_signature_with_arity_greater_than(&mut self, symbol: SymbolId, arity: i32) -> bool {
        for signature in self.get_signatures_of_symbol(symbol) {
            if self.get_parameter_count(signature) > arity {
                return true;
            }
        }
        false
    }

    // Go: checker/checker.go:29088 getHelperNames
    pub fn get_helper_names(&self, helper: ExternalEmitHelpers) -> Vec<String> {
        let names: &[&str] = match helper {
            ExternalEmitHelpers::REST => &["__rest"],
            ExternalEmitHelpers::DECORATE => {
                if self.legacy_decorators {
                    &["__decorate"]
                } else {
                    &["__esDecorate", "__runInitializers"]
                }
            }
            ExternalEmitHelpers::METADATA => &["__metadata"],
            ExternalEmitHelpers::PARAM => &["__param"],
            ExternalEmitHelpers::AWAITER => &["__awaiter"],
            ExternalEmitHelpers::AWAIT => &["__await"],
            ExternalEmitHelpers::ASYNC_GENERATOR => &["__asyncGenerator"],
            ExternalEmitHelpers::ASYNC_DELEGATOR => &["__asyncDelegator"],
            ExternalEmitHelpers::ASYNC_VALUES => &["__asyncValues"],
            ExternalEmitHelpers::EXPORT_STAR => &["__exportStar"],
            ExternalEmitHelpers::IMPORT_STAR => &["__importStar"],
            ExternalEmitHelpers::IMPORT_DEFAULT => &["__importDefault"],
            ExternalEmitHelpers::MAKE_TEMPLATE_OBJECT => &["__makeTemplateObject"],
            ExternalEmitHelpers::CLASS_PRIVATE_FIELD_GET => &["__classPrivateFieldGet"],
            ExternalEmitHelpers::CLASS_PRIVATE_FIELD_SET => &["__classPrivateFieldSet"],
            ExternalEmitHelpers::CLASS_PRIVATE_FIELD_IN => &["__classPrivateFieldIn"],
            ExternalEmitHelpers::SET_FUNCTION_NAME => &["__setFunctionName"],
            ExternalEmitHelpers::PROP_KEY => &["__propKey"],
            ExternalEmitHelpers::ADD_DISPOSABLE_RESOURCE_AND_DISPOSE_RESOURCES => {
                &["__addDisposableResource", "__disposeResources"]
            }
            ExternalEmitHelpers::REWRITE_RELATIVE_IMPORT_EXTENSION => {
                &["__rewriteRelativeImportExtension"]
            }
            _ => panic!("Unrecognized helper"),
        };
        names.iter().map(|s| (*s).to_string()).collect()
    }

    // Go: checker/checker.go:29138 resolveHelpersModule
    pub fn resolve_helpers_module(&mut self, file: Node, error_node: Node) -> SymbolId {
        if self
            .source_file_links
            .get(file)
            .external_helpers_module
            .is_nil()
        {
            let location = get_import_helpers_import_specifier(&source_file_info(file).path);
            let mut helpers_module = self.resolve_external_module(
                location,
                EXTERNAL_HELPERS_MODULE_NAME_TEXT,
                Some(diag::This_syntax_requires_an_imported_helper_but_module_0_cannot_be_found),
                error_node,
                false,       /*isForAugmentation*/
                TypeId::NIL, /*importAttributesType*/
            );
            if helpers_module.is_nil() {
                helpers_module = self.unknown_symbol;
            }
            self.source_file_links.get(file).external_helpers_module = helpers_module;
        }
        self.source_file_links.get(file).external_helpers_module
    }

    // Go: checker/checker.go:29151 markDecoratorAliasReferenced
    pub fn mark_decorator_alias_referenced(&mut self, node: Node /*HasDecorators*/) {
        if self
            .compiler_options
            .emit_decorator_metadata
            .is_false_or_unknown()
        {
            return;
        }
        let decorators = node.decorators();
        let first_decorator = if decorators.is_empty() {
            Node::NIL
        } else {
            decorators.get(0)
        };
        if first_decorator.is_nil() {
            return;
        }

        self.check_external_emit_helpers(first_decorator, ExternalEmitHelpers::METADATA);

        // we only need to perform these checks if we are emitting serialized type metadata for the target of a decorator.
        match node.kind() {
            SyntaxKind::ClassDeclaration => {
                let ctor = get_first_constructor_with_body(node);
                if ctor.is_some() {
                    for p in ctor.parameters().iter() {
                        let type_node = self.get_parameter_type_node_for_decorator_check(p);
                        self.mark_decorator_meda_data_type_node_as_referenced(type_node);
                    }
                }
            }
            SyntaxKind::GetAccessor | SyntaxKind::SetAccessor => {
                let mut other_kind = SyntaxKind::SetAccessor;
                if node.kind() == SyntaxKind::SetAccessor {
                    other_kind = SyntaxKind::GetAccessor;
                }
                let symbol = self.get_symbol_of_declaration(node);
                let other_accessor = get_declaration_of_kind(&self.symbols, symbol, other_kind);
                let mut annotation = self.get_annotated_accessor_type_node(node);
                if annotation.is_nil() && other_accessor.is_some() {
                    annotation = self.get_annotated_accessor_type_node(other_accessor);
                }
                self.mark_decorator_meda_data_type_node_as_referenced(annotation);
            }
            SyntaxKind::MethodDeclaration => {
                for p in node.parameters().iter() {
                    let type_node = self.get_parameter_type_node_for_decorator_check(p);
                    self.mark_decorator_meda_data_type_node_as_referenced(type_node);
                }
                self.mark_decorator_meda_data_type_node_as_referenced(node.type_());
            }
            SyntaxKind::PropertyDeclaration => {
                self.mark_decorator_meda_data_type_node_as_referenced(node.type_());
            }
            SyntaxKind::Parameter => {
                let type_node = self.get_parameter_type_node_for_decorator_check(node);
                self.mark_decorator_meda_data_type_node_as_referenced(type_node);
                let containing_signature = node.parent();
                for p in containing_signature.parameters().iter() {
                    let type_node = self.get_parameter_type_node_for_decorator_check(p);
                    self.mark_decorator_meda_data_type_node_as_referenced(type_node);
                }
                self.mark_decorator_meda_data_type_node_as_referenced(containing_signature.type_());
            }
            _ => {}
        }
    }

    // Go: checker/checker.go:29199 getParameterTypeNodeForDecoratorCheck
    pub fn get_parameter_type_node_for_decorator_check(
        &mut self,
        node: Node, /*ParameterDeclaration*/
    ) -> Node {
        let type_node = node.type_();
        if node.dot_dot_dot_token().is_some() {
            return get_rest_parameter_element_type(type_node);
        }
        type_node
    }

    // Go: checker/checker.go:29207 markDecoratorMedataDataTypeNodeAsReferenced
    pub fn mark_decorator_meda_data_type_node_as_referenced(
        &mut self,
        node: Node, /*TypeNode*/
    ) {
        let entity_name = self.get_entity_name_for_decorator_metadata(node);
        if entity_name.is_some() && is_entity_name(entity_name) {
            self.mark_entity_name_or_entity_expression_as_reference(entity_name, true);
        }
    }

    // Go: checker/checker.go:29214 getEntityNameForDecoratorMetadata
    pub fn get_entity_name_for_decorator_metadata(&mut self, node: Node) -> Node {
        if node.is_nil() {
            return node;
        }
        match node.kind() {
            SyntaxKind::IntersectionType => {
                let types = node.types().nodes().to_vec();
                self.get_entity_name_for_decorator_metadata_from_type_list(&types)
            }
            SyntaxKind::UnionType => {
                let types = node.types().nodes().to_vec();
                self.get_entity_name_for_decorator_metadata_from_type_list(&types)
            }
            SyntaxKind::ConditionalType => {
                let types = [node.true_type(), node.false_type()];
                self.get_entity_name_for_decorator_metadata_from_type_list(&types)
            }
            SyntaxKind::ParenthesizedType => {
                self.get_entity_name_for_decorator_metadata(node.type_())
            }
            SyntaxKind::NamedTupleMember => {
                self.get_entity_name_for_decorator_metadata(node.type_())
            }
            SyntaxKind::TypeReference => node.type_name(),
            _ => Node::NIL,
        }
    }

    // Go: checker/checker.go:29235 getEntityNameForDecoratorMetadataFromTypeList
    pub fn get_entity_name_for_decorator_metadata_from_type_list(
        &mut self,
        type_nodes: &[Node],
    ) -> Node {
        let mut common_entity_name = Node::NIL;
        for &type_node in type_nodes {
            if type_node.kind() == SyntaxKind::NeverKeyword {
                continue; // Always elide `never` from the union/intersection if possible
            }
            if !self.strict_null_checks
                && (type_node.kind() == SyntaxKind::LiteralType
                    && type_node.literal().kind() == SyntaxKind::NullKeyword
                    || type_node.kind() == SyntaxKind::UndefinedKeyword)
            {
                continue; // Elide null and undefined from unions for metadata, just like what we did prior to the implementation of strict null checks
            }
            let individual_entity_name = self.get_entity_name_for_decorator_metadata(type_node);
            if individual_entity_name.is_nil() {
                // Individual is something like string number
                // So it would be serialized to either that type or object
                // Safe to return here
                return Node::NIL;
            }

            if common_entity_name.is_nil() {
                common_entity_name = individual_entity_name;
            } else {
                // Note this is in sync with the transformation that happens for type node.
                // Keep this in sync with serializeUnionOrIntersectionType
                // Verify if they refer to same entity and is identifier
                // return undefined if they dont match because we would emit object
                if !is_identifier(common_entity_name)
                    || !is_identifier(individual_entity_name)
                    || common_entity_name.text() != individual_entity_name.text()
                {
                    return Node::NIL;
                }
            }
        }
        common_entity_name
    }

    // Go: checker/checker.go:29267 markAliasReferenced
    pub fn mark_alias_referenced(&mut self, symbol: SymbolId, location: Node) {
        if !self.can_collect_symbol_alias_accessibility_data {
            return;
        }
        if is_non_local_alias(&self.symbols, symbol, SymbolFlags::VALUE /*excludes*/)
            && !is_in_type_query(location)
        {
            let target = self.resolve_alias(symbol);
            if self
                .get_symbol_flags_ex(
                    symbol, true,  /*excludeTypeOnlyMeanings*/
                    false, /*excludeLocalMeanings*/
                )
                .intersects(SymbolFlags::VALUE | SymbolFlags::EXPORT_VALUE)
            {
                // An alias resolving to a const enum cannot be elided if (1) 'isolatedModules' is enabled
                // (because the const enum value will not be inlined), or if (2) the alias is an export
                // of a const enum declaration that will be preserved.
                if self.compiler_options.get_isolated_modules()
                    || self.compiler_options.should_preserve_const_enums()
                        && is_export_or_export_expression(location)
                    || {
                        let export_symbol =
                            self.get_export_symbol_of_value_symbol_if_exported(target);
                        !self.is_const_enum_or_const_enum_only_module(export_symbol)
                    }
                {
                    self.mark_alias_symbol_as_referenced(symbol);
                }
            }
        }
    }

    // When an alias symbol is referenced, we need to mark the entity it references as referenced and in turn repeat that until
    // we reach a non-alias or an exported entity (which is always considered referenced). We do this by checking the target of
    // the alias as an expression (which recursively takes us back here if the target references another alias).
    // Go: checker/checker.go:29289 markAliasSymbolAsReferenced
    pub fn mark_alias_symbol_as_referenced(&mut self, symbol: SymbolId) {
        if !self.alias_symbol_links.get(symbol).referenced {
            self.alias_symbol_links.get(symbol).referenced = true;
            let node = self.get_declaration_of_alias_symbol(symbol);
            if node.is_nil() {
                panic!("Unexpected nil in markAliasSymbolAsReferenced");
            }
            // We defer checking of the reference of an `import =` until the import itself is referenced,
            // This way a chain of imports can be elided if ultimately the final input is only used in a type
            // position.
            if is_import_equals_declaration(node)
                && node.module_reference().kind() != SyntaxKind::ExternalModuleReference
            {
                let resolved = self.resolve_symbol(symbol);
                if self
                    .get_symbol_flags(resolved)
                    .intersects(SymbolFlags::VALUE)
                {
                    // import foo = <symbol>
                    let left = get_first_identifier(node.module_reference());
                    self.mark_identifier_alias_referenced(left);
                }
            }
        }
    }

    // Go: checker/checker.go:29310 markExportAsReferenced
    pub fn mark_export_as_referenced(
        &mut self,
        node: Node, /*ImportEqualsDeclaration | ExportSpecifier*/
    ) {
        let symbol = self.get_symbol_of_declaration(node);
        let target = self.resolve_alias(symbol);
        if target.is_some() {
            let mark_alias = target == self.unknown_symbol
                || (self
                    .get_symbol_flags_ex(
                        symbol, true,  /*excludeTypeOnlyMeanings*/
                        false, /*excludeLocalMeanings*/
                    )
                    .intersects(SymbolFlags::VALUE)
                    && !self.is_const_enum_or_const_enum_only_module(target));
            if mark_alias {
                self.mark_alias_symbol_as_referenced(symbol);
            }
        }
    }

    // Go: checker/checker.go:29322 markEntityNameOrEntityExpressionAsReference
    pub fn mark_entity_name_or_entity_expression_as_reference(
        &mut self,
        type_name: Node, /*EntityNameOrEntityNameExpression | nil*/
        for_decorator_metadata: bool,
    ) {
        if type_name.is_nil() {
            return;
        }

        let root_name = get_first_identifier(type_name);
        let meaning = (if type_name.kind() == SyntaxKind::Identifier {
            SymbolFlags::TYPE
        } else {
            SymbolFlags::NAMESPACE
        }) | SymbolFlags::ALIAS;
        let root_symbol = self.resolve_name(
            root_name,
            root_name.text(),
            meaning,
            None,  /*nameNotFoundMessage*/
            true,  /*isUse*/
            false, /*excludeGlobals*/
        );

        if root_symbol.is_some() && self.sym(root_symbol).flags.intersects(SymbolFlags::ALIAS) {
            if self.can_collect_symbol_alias_accessibility_data
                && self.symbol_is_value(root_symbol)
                && {
                    let resolved = self.resolve_alias(root_symbol);
                    !self.is_const_enum_or_const_enum_only_module(resolved)
                }
                && self.get_type_only_alias_declaration(root_symbol).is_nil()
            {
                self.mark_alias_symbol_as_referenced(root_symbol);
            } else if for_decorator_metadata
                && self.compiler_options.get_isolated_modules()
                && self.compiler_options.get_emit_module_kind() >= ModuleKind::ES2015
                && !self.symbol_is_value(root_symbol)
                && !self
                    .sym(root_symbol)
                    .declarations
                    .iter()
                    .any(|&d| is_type_only_import_or_export_declaration(d))
            {
                // PORT: Go calls `c.error` (which adds the diagnostic) and then
                // mutates it with SetRelatedInfo. `error` returns an owned copy,
                // so build the diagnostic, set related info, then add it.
                let mut d = new_diagnostic_for_node(
                    type_name,
                    diag::A_type_referenced_in_a_decorated_signature_must_be_imported_with_import_type_or_a_namespace_import_when_isolatedModules_and_emitDecoratorMetadata_are_enabled,
                    args![],
                );
                let mut alias_declaration = Node::NIL;
                for &decl in &self.sym(root_symbol).declarations {
                    if is_alias_symbol_declaration(decl) {
                        alias_declaration = decl;
                        break;
                    }
                }
                if alias_declaration.is_some() {
                    d.set_related_info(vec![create_diagnostic_for_node(
                        alias_declaration,
                        diag::X_0_was_imported_here,
                        args![root_name.text()],
                    )]);
                }
                self.add_diagnostic(d);
            }
        }
    }
}

// Go: checker/checker.go:29357 getEntityNameFromTypeNode
pub fn get_entity_name_from_type_node(node: Node /*TypeNode*/) -> Node {
    match node.kind() {
        SyntaxKind::TypeReference => node.type_name(),

        SyntaxKind::ExpressionWithTypeArguments => {
            if is_entity_name_expression(node.expression()) {
                return node.expression();
            }
            Node::NIL
        }

        // These aren't valid TypeNodes, but we treat them as such because of `isPartOfTypeNode`, which returns `true` for things that aren't `TypeNode`s.
        SyntaxKind::Identifier | SyntaxKind::QualifiedName => node,

        _ => Node::NIL,
    }
}

impl Checker {
    // If a TypeNode can be resolved to a value symbol imported from an external module, it is
    // marked as referenced to prevent import elision.
    // Go: checker/checker.go:29378 markTypeNodeAsReferenced
    pub fn mark_type_node_as_referenced(&mut self, node: Node /*TypeNode*/) {
        if node.is_some() {
            self.mark_entity_name_or_entity_expression_as_reference(
                get_entity_name_from_type_node(node),
                false, /*forDecoratorMetadata*/
            );
        }
    }

    // Go: checker/checker.go:29384 GetPromisedTypeOfPromise
    pub fn get_promised_type_of_promise(&mut self, t: TypeId) -> TypeId {
        self.get_promised_type_of_promise_ex(t, Node::NIL, None)
    }

    // Gets the "promised type" of a promise.
    // @param type The type of the promise.
    // @remarks The "promised type" of a type is the type of the "value" parameter of the "onfulfilled" callback.
    // Go: checker/checker.go:29391 getPromisedTypeOfPromiseEx
    // PORT: Go `thisTypeForErrorOut **Type` (nil-able out pointer) becomes
    // `Option<&mut TypeId>`.
    pub fn get_promised_type_of_promise_ex(
        &mut self,
        t: TypeId,
        error_node: Node,
        this_type_for_error_out: Option<&mut TypeId>,
    ) -> TypeId {
        //  { // type
        //      then( // thenFunction
        //          onfulfilled: ( // onfulfilledParameterType
        //              value: T // valueParameterType
        //          ) => any
        //      ): any;
        //  }
        if self.is_type_any(t) {
            return TypeId::NIL;
        }
        let key = CachedTypeKey {
            kind: CachedTypeKind::PROMISED_TYPE_OF_PROMISE,
            type_id: t,
        };
        if let Some(&cached) = self.cached_types.get(&key) {
            if cached.is_some() {
                return cached;
            }
        }
        let global_promise_type = self.get_global_promise_type();
        if self.is_reference_to_type(t, global_promise_type) {
            let result = self.type_arguments_of(t)[0];
            self.cached_types.insert(key, result);
            return result;
        }
        // primitives with a `{ then() }` won't be unwrapped/adopted.
        let base = self.get_base_constraint_or_type(t);
        if self.all_types_assignable_to_kind(base, TypeFlags::PRIMITIVE | TypeFlags::NEVER) {
            return TypeId::NIL;
        }
        let then_function = self.get_type_of_property_of_type(t, "then");
        // TODO: GH#18217
        if self.is_type_any(then_function) {
            return TypeId::NIL;
        }
        let mut then_signatures = SharedList::default();
        if then_function.is_some() {
            then_signatures = self.get_signatures_of_type(then_function, SignatureKind::CALL);
        }
        if then_signatures.is_empty() {
            if error_node.is_some() {
                self.error(error_node, diag::A_promise_must_have_a_then_method, args![]);
            }
            return TypeId::NIL;
        }
        let mut this_type_for_error = TypeId::NIL;
        let mut candidates: Vec<SignatureId> = Vec::new();
        for then_signature in then_signatures {
            let this_type = self.get_this_type_of_signature(then_signature);
            if this_type.is_some() && this_type != self.void_type && {
                let relation = self.subtype_relation.clone();
                !self.is_type_related_to(t, this_type, &relation)
            } {
                this_type_for_error = this_type;
            } else {
                candidates.push(then_signature);
            }
        }
        if candidates.is_empty() {
            debug_assert!(this_type_for_error.is_some());
            if let Some(out) = this_type_for_error_out {
                *out = this_type_for_error;
            }
            if error_node.is_some() {
                let type_string = self.type_to_string(t);
                let this_type_string = self.type_to_string(this_type_for_error);
                self.error(
                    error_node,
                    diag::The_this_context_of_type_0_is_not_assignable_to_method_s_this_of_type_1,
                    args![type_string, this_type_string],
                );
            }
            return TypeId::NIL;
        }
        let mut first_parameter_types: Vec<TypeId> = Vec::with_capacity(candidates.len());
        for &candidate in &candidates {
            first_parameter_types.push(self.get_type_of_first_parameter_of_signature(candidate));
        }
        let union = self.get_union_type(&first_parameter_types);
        let onfulfilled_parameter_type =
            self.get_type_with_facts(union, TypeFacts::NE_UNDEFINED_OR_NULL);
        if self.is_type_any(onfulfilled_parameter_type) {
            return TypeId::NIL;
        }
        let onfulfilled_parameter_signatures =
            self.get_signatures_of_type(onfulfilled_parameter_type, SignatureKind::CALL);
        if onfulfilled_parameter_signatures.is_empty() {
            if error_node.is_some() {
                self.error(
                    error_node,
                    diag::The_first_parameter_of_the_then_method_of_a_promise_must_be_a_callback,
                    args![],
                );
            }
            return TypeId::NIL;
        }
        let mut value_types: Vec<TypeId> =
            Vec::with_capacity(onfulfilled_parameter_signatures.len());
        for &signature in &onfulfilled_parameter_signatures {
            value_types.push(self.get_type_of_first_parameter_of_signature(signature));
        }
        let result =
            self.get_union_type_ex(&value_types, UnionReduction::SUBTYPE, None, TypeId::NIL);
        self.cached_types.insert(key, result);
        result
    }
}
