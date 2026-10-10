//! Port of Effect-TS/tsgo `shim/checker/integration.go` at `@effect/tsgo@0.51.1`
//! (`47cb1ed7`). Effect owns this shim file (the other `shim/*` files are
//! generated forwarders). Its functions peek at checker state that the API
//! stability analysis reads without resolving anything.
//!
//! PORT: Go calls these as `checker.Foo(c, ..)` from `typeparser`. Here they
//! are free functions in this module, called as `checker_integration::foo(c, ..)`
//! (the module is not glob re-exported, as Go qualifies them with the package).
//! Go `*MappedType` parameters are the mapped `TypeId`.

use crate::prelude::*;

// Go: shim/checker/integration.go GetNonDistributedTypeParameter
/// GetNonDistributedTypeParameter returns the existing declaration binder for a
/// distributed type parameter. It only reads compiler-owned types and never
/// instantiates or allocates a replacement type.
pub fn get_non_distributed_type_parameter(c: &Checker, t: TypeId) -> TypeId {
    if t.is_nil() {
        return TypeId::NIL;
    }
    c.get_non_distributed_type_parameter(t)
}

// Go: shim/checker/integration.go GetNonPrimitiveType
/// GetNonPrimitiveType returns the checker's canonical non-primitive object type.
pub fn get_non_primitive_type(c: &Checker) -> TypeId {
    c.non_primitive_type
}

// Go: shim/checker/integration.go GetMappedTypeConstraintType
/// ConstraintType returns only the already-resolved constraint of a mapped type.
pub fn get_mapped_type_constraint_type(c: &Checker, t: TypeId) -> TypeId {
    c.ty(t).as_mapped_type().constraint_type
}

// Go: shim/checker/integration.go GetMappedTypeNameType
/// NameType returns only the already-resolved remapped key type of a mapped type.
pub fn get_mapped_type_name_type(c: &Checker, t: TypeId) -> TypeId {
    c.ty(t).as_mapped_type().name_type
}

// Go: shim/checker/integration.go GetMappedTypeTemplateType
/// TemplateType returns only the already-resolved mapped property value type.
pub fn get_mapped_type_template_type(c: &Checker, t: TypeId) -> TypeId {
    c.ty(t).as_mapped_type().template_type
}

// Go: shim/checker/integration.go GetResolvedConditionalTypeBranch
/// GetResolvedConditionalTypeBranch returns an already-resolved true or false
/// branch of a conditional type, or nil when the branch has not been resolved
/// yet. Unlike getTrueTypeFromConditionalType it never instantiates or evaluates
/// the conditional, so callers that only inspect represented components can read
/// a resolved branch without risking recursive instantiation.
pub fn get_resolved_conditional_type_branch(c: &Checker, t: TypeId, true_branch: bool) -> TypeId {
    if t.is_nil() || !c.ty(t).flags().intersects(TypeFlags::CONDITIONAL) {
        return TypeId::NIL;
    }
    let d = c.ty(t).as_conditional_type();
    if true_branch {
        return d.resolved_true_type;
    }
    d.resolved_false_type
}

// Go: shim/checker/integration.go GetConditionalTypeBranchNode
/// GetConditionalTypeBranchNode returns the declared true or false branch node of
/// a conditional type without reading or evaluating the branch type.
pub fn get_conditional_type_branch_node(c: &Checker, t: TypeId, true_branch: bool) -> Node {
    if t.is_nil() || !c.ty(t).flags().intersects(TypeFlags::CONDITIONAL) {
        return Node::NIL;
    }
    let d = c.ty(t).as_conditional_type();
    // PORT: Go `root == nil` cannot happen: the root is not optional here.
    let node = d.root.borrow().node;
    if node.is_nil() {
        return Node::NIL;
    }
    if true_branch {
        return node.true_type();
    }
    node.false_type()
}

// Go: shim/checker/integration.go GetResolvedTypeFromTypeNode
/// GetResolvedTypeFromTypeNode returns the type the checker has already
/// associated with a type node, without resolving it. It peeks the cached node
/// links, so it never instantiates or evaluates anything; an unresolved node
/// returns nil.
pub fn get_resolved_type_from_type_node(c: &Checker, node: Node) -> TypeId {
    if node.is_nil() {
        return TypeId::NIL;
    }
    match c.type_node_links.try_get(node) {
        None => TypeId::NIL,
        Some(links) => links.resolved_type,
    }
}

// Go: shim/checker/integration.go GetConditionalTypeBranchType
/// GetConditionalTypeBranchType returns the branch type of a conditional type
/// only when the checker has already materialized the declared branch node. It
/// is a cached-field peek, not a resolver: an unresolved branch returns nil, so
/// callers that inspect represented components never force instantiation.
pub fn get_conditional_type_branch_type(c: &Checker, t: TypeId, true_branch: bool) -> TypeId {
    get_resolved_type_from_type_node(c, get_conditional_type_branch_node(c, t, true_branch))
}

// Go: shim/checker/integration.go GetResolvedTypeArguments
/// GetResolvedTypeArguments returns the type arguments already recorded on a
/// reference type without resolving or instantiating them. A deferred reference
/// whose arguments have not been materialized returns nil.
pub fn get_resolved_type_arguments(c: &Checker, t: TypeId) -> Vec<TypeId> {
    if t.is_nil()
        || !c.ty(t).flags().intersects(TypeFlags::OBJECT)
        || !c.ty(t).object_flags().intersects(ObjectFlags::REFERENCE)
    {
        return Vec::new();
    }
    c.ty(t).as_type_reference().resolved_type_arguments.to_vec()
}

// Go: shim/checker/integration.go GetResolvedTypeOfSymbolIfMaterialized
/// GetResolvedTypeOfSymbolIfMaterialized returns the type already computed for a
/// value symbol, or nil when the checker has not materialized it. It never
/// resolves the symbol's type, so callers can avoid forcing an inferred type
/// whose instantiation is unbounded.
pub fn get_resolved_type_of_symbol_if_materialized(c: &Checker, symbol: SymbolId) -> TypeId {
    if symbol.is_nil() {
        return TypeId::NIL;
    }
    match materialized_value_symbol_links(c, symbol) {
        None => TypeId::NIL,
        Some(links) => links.resolved_type,
    }
}

// Go: shim/checker/integration.go GetResolvedDeclaredTypeOfSymbolIfMaterialized
/// GetResolvedDeclaredTypeOfSymbolIfMaterialized returns the declared type
/// already recorded for a symbol, or nil when it has not been computed. Only
/// already materialized results are returned, so resolving a type alias whose
/// declared type would instantiate a recursive alias can never be forced.
pub fn get_resolved_declared_type_of_symbol_if_materialized(
    c: &Checker,
    symbol: SymbolId,
) -> TypeId {
    if symbol.is_nil() {
        return TypeId::NIL;
    }
    let flags = c.sym(symbol).flags;
    if flags.intersects(SymbolFlags::CLASS | SymbolFlags::INTERFACE | SymbolFlags::ALIAS) {
        if let Some(links) = c.declared_type_links.try_get(symbol) {
            return links.declared_type;
        }
    } else if flags.intersects(SymbolFlags::TYPE_ALIAS)
        && let Some(links) = c.type_alias_links.try_get(symbol)
    {
        return links.declared_type;
    }
    TypeId::NIL
}

// Go: shim/checker/integration.go GetDeclaredIndexInfosOfSymbol
/// GetDeclaredIndexInfosOfSymbol returns the index infos declared by a symbol
/// without resolving its member table or its key and value annotations. Unlike
/// getIndexSymbol it walks the symbol's own declarations directly, so a
/// late-bound computed property name is never evaluated just to inspect index
/// signatures. A key or value type is returned only when the checker has
/// already materialized the annotation; an unresolved annotation stays nil so
/// callers can inspect the declaration instead of forcing a potentially
/// unbounded instantiation.
pub fn get_declared_index_infos_of_symbol(c: &mut Checker, symbol: SymbolId) -> Vec<IndexInfoId> {
    if symbol.is_nil() {
        return Vec::new();
    }
    let mut infos = Vec::new();
    let symbol_declarations: Vec<Node> = c.sym(symbol).declarations.to_vec();
    for symbol_declaration in symbol_declarations {
        if symbol_declaration.is_nil() {
            continue;
        }
        let mut declarations: Vec<Node> = Vec::new();
        match symbol_declaration.kind() {
            SyntaxKind::ClassDeclaration
            | SyntaxKind::ClassExpression
            | SyntaxKind::InterfaceDeclaration
            | SyntaxKind::EnumDeclaration
            | SyntaxKind::TypeLiteral
            | SyntaxKind::MappedType => {
                declarations = symbol_declaration.members().to_vec();
            }
            _ => {}
        }
        if is_index_signature_declaration(symbol_declaration) {
            declarations.push(symbol_declaration);
        }
        for declaration in declarations {
            if declaration.is_nil() || !is_index_signature_declaration(declaration) {
                continue;
            }
            if has_static_modifier(declaration) {
                // Instance members only, matching getIndexSymbol.
                continue;
            }
            let parameters = declaration.parameters();
            if parameters.len() != 1 {
                continue;
            }
            let key_node = parameters.get(0).type_();
            if key_node.is_nil() {
                continue;
            }
            let key_type = get_resolved_type_from_type_node(c, key_node);
            let value_type = get_resolved_type_from_type_node(c, declaration.type_());
            infos.push(c.new_index_info(
                key_type,
                value_type,
                has_modifier(declaration, ModifierFlags::READONLY),
                declaration,
                &[],
            ));
        }
    }
    infos
}

// Go: shim/checker/integration.go GetResolvedReturnTypeOfSignatureIfMaterialized
/// GetResolvedReturnTypeOfSignatureIfMaterialized returns the return type the
/// checker has already computed for a signature, or nil when it has not been
/// materialized. It is a cached-field peek, not a resolver, so an annotated but
/// unresolved return type is never forced.
pub fn get_resolved_return_type_of_signature_if_materialized(
    c: &Checker,
    signature: SignatureId,
) -> TypeId {
    if signature.is_nil() {
        return TypeId::NIL;
    }
    c.sig(signature).resolved_return_type
}

// Go: shim/checker/integration.go GetResolvedConstraintOfTypeParameterIfMaterialized
/// GetResolvedConstraintOfTypeParameterIfMaterialized returns the constraint the
/// checker has already computed for a type parameter, or nil when it has not
/// been materialized or when the checker recorded that there is no constraint.
/// A nil result means the declared constraint annotation is still unresolved, so
/// callers can inspect the declaration without forcing it.
pub fn get_resolved_constraint_of_type_parameter_if_materialized(c: &Checker, t: TypeId) -> TypeId {
    if t.is_nil() || !c.ty(t).flags().intersects(TypeFlags::TYPE_PARAMETER) {
        return TypeId::NIL;
    }
    let constraint = c.ty(t).as_type_parameter().constraint;
    if constraint.is_nil()
        || constraint == c.no_constraint_type
        || constraint == c.circular_constraint_type
    {
        return TypeId::NIL;
    }
    constraint
}

// Go: shim/checker/integration.go GetResolvedDefaultFromTypeParameterIfMaterialized
/// GetResolvedDefaultFromTypeParameterIfMaterialized returns the default type
/// the checker has already computed for a type parameter, or nil when it has
/// not been materialized or when the checker recorded that there is no default.
pub fn get_resolved_default_from_type_parameter_if_materialized(c: &Checker, t: TypeId) -> TypeId {
    if t.is_nil() || !c.ty(t).flags().intersects(TypeFlags::TYPE_PARAMETER) {
        return TypeId::NIL;
    }
    let default_type = c.ty(t).as_type_parameter().resolved_default_type;
    if default_type.is_nil()
        || default_type == c.no_constraint_type
        || default_type == c.circular_constraint_type
        || default_type == c.resolving_default_type
    {
        return TypeId::NIL;
    }
    default_type
}

// Go: shim/checker/integration.go GetResolvedBaseTypesOfTypeIfMaterialized
/// GetResolvedBaseTypesOfTypeIfMaterialized returns the base types the checker
/// has already resolved for a class or interface. The boolean reports whether
/// inheritance has been materialized at all, so a cold declaration is never
/// mistaken for one without heritage and callers can avoid forcing it.
pub fn get_resolved_base_types_of_type_if_materialized(
    c: &Checker,
    t: TypeId,
) -> (Vec<TypeId>, bool) {
    if t.is_nil()
        || !c
            .ty(t)
            .object_flags()
            .intersects(ObjectFlags::CLASS_OR_INTERFACE | ObjectFlags::TUPLE)
    {
        return (Vec::new(), false);
    }
    let data = c.ty(t).as_interface_type();
    if !data.base_types_resolved {
        return (Vec::new(), false);
    }
    (data.resolved_base_types.to_vec(), true)
}

// Go: shim/checker/integration.go GetResolvedMembersOfTypeIfMaterialized
/// GetResolvedMembersOfTypeIfMaterialized returns the member table the checker
/// has already resolved for a structured type. The boolean reports whether the
/// member surface was materialized at all: a deferred reference keeps false so
/// callers inspect the declaration instead of forcing member resolution, which
/// would instantiate index annotations and inherited members.
pub fn get_resolved_members_of_type_if_materialized(c: &Checker, t: TypeId) -> (SymbolTable, bool) {
    if t.is_nil()
        || !c.ty(t).flags().intersects(TypeFlags::OBJECT)
        || !c
            .ty(t)
            .object_flags()
            .intersects(ObjectFlags::MEMBERS_RESOLVED)
    {
        return (SymbolTable::NIL, false);
    }
    (c.ty(t).as_structured_type().members, true)
}

// Go: shim/checker/integration.go GetResolvedSignaturesOfTypeIfMaterialized
/// GetResolvedSignaturesOfTypeIfMaterialized returns the call or construct
/// signatures the checker has already resolved for a structured type. The
/// boolean reports whether the member surface was materialized at all, so a
/// deferred type never forces signature resolution.
pub fn get_resolved_signatures_of_type_if_materialized(
    c: &Checker,
    t: TypeId,
    kind: SignatureKind,
) -> (Vec<SignatureId>, bool) {
    if t.is_nil()
        || !c.ty(t).flags().intersects(TypeFlags::OBJECT)
        || !c
            .ty(t)
            .object_flags()
            .intersects(ObjectFlags::MEMBERS_RESOLVED)
    {
        return (Vec::new(), false);
    }
    let data = c.ty(t).as_structured_type();
    if kind == SignatureKind::CALL {
        return (data.call_signatures().to_vec(), true);
    }
    (data.construct_signatures().to_vec(), true)
}

// Go: shim/checker/integration.go GetResolvedIndexInfosOfTypeIfMaterialized
/// GetResolvedIndexInfosOfTypeIfMaterialized returns the index infos the
/// checker has already computed for a structured type. The boolean reports
/// whether the member surface was materialized at all, so a deferred type never
/// forces an index annotation through this accessor.
pub fn get_resolved_index_infos_of_type_if_materialized(
    c: &Checker,
    t: TypeId,
) -> (Vec<IndexInfoId>, bool) {
    if t.is_nil()
        || !c.ty(t).flags().intersects(TypeFlags::OBJECT)
        || !c
            .ty(t)
            .object_flags()
            .intersects(ObjectFlags::MEMBERS_RESOLVED)
    {
        return (Vec::new(), false);
    }
    (c.ty(t).as_structured_type().index_infos().to_vec(), true)
}

// Go: shim/checker/integration.go GetInstantiatedSymbolMapper
/// GetInstantiatedSymbolMapper returns the mapper already recorded on an
/// instantiated symbol, or nil. It is a cached-field peek: it never resolves or
/// instantiates the symbol's type, so a caller can map an uninstantiated binder
/// forward without evaluating a recursive alias member.
pub fn get_instantiated_symbol_mapper(c: &Checker, symbol: SymbolId) -> MapperId {
    if symbol.is_nil()
        || !c
            .sym(symbol)
            .check_flags
            .intersects(CheckFlags::INSTANTIATED)
    {
        return MapperId::NIL;
    }
    match materialized_value_symbol_links(c, symbol) {
        None => MapperId::NIL,
        Some(links) => links.mapper,
    }
}

// Go: shim/checker/integration.go IsSourceFileTypeChecked
/// IsSourceFileTypeChecked reports whether the checker has finished type
/// checking a source file. It is a state peek that never checks the file: the
/// stability analysis uses it to decide whether an inferred type may be resolved
/// through the ordinary accessors, because forcing inference over a file that
/// has not been checked could run body checks and report their diagnostics.
pub fn is_source_file_type_checked(c: &Checker, source_file: Node) -> bool {
    if source_file.is_nil() {
        return false;
    }
    c.source_file_links
        .try_get(source_file)
        .is_some_and(|links| links.type_checked)
}

// Go: shim/checker/integration.go IsCheckingSourceFile
/// IsCheckingSourceFile reports whether the checker is currently inside its
/// ordinary source-file check lifecycle. It is a state peek that never checks a
/// file. The stability analysis uses it to allow native lazy inference only
/// while the checker is actively checking, which is the lifecycle in which that
/// inference's diagnostics are attributed to their declaring expressions; a
/// speculative read outside any check keeps refusing inferred components.
pub fn is_checking_source_file(c: &Checker) -> bool {
    c.ctx.is_some()
}

// Go: shim/checker/integration.go IsAliasResolutionFailed
/// IsAliasResolutionFailed reports only an already-cached alias resolution
/// failure. It never resolves the alias or emits diagnostics.
pub fn is_alias_resolution_failed(c: &Checker, symbol: SymbolId) -> bool {
    if symbol.is_nil() {
        return false;
    }
    c.alias_symbol_links
        .try_get(symbol)
        .is_some_and(|links| links.alias_target == c.unknown_symbol)
}

// Go: shim/checker/integration.go materializedValueSymbolLinks
/// materializedValueSymbolLinks reads the modern provider's paged symbol store
/// through the generated checker layout, without allocating links.
///
/// PORT: this is the one place where the Effect port reads
/// `value_symbol_links` (2 callers). Go `symbolArenaLinkStore.TryGet` keys the
/// store by `ast.GetSymbolId(symbol)`, which gives the symbol its id on the
/// first read. The port store is keyed by the symbol handle, so the id is
/// given here first, as Go does.
fn materialized_value_symbol_links(c: &Checker, symbol: SymbolId) -> Option<&ValueSymbolLinks> {
    get_symbol_id(&c.symbols, symbol);
    c.value_symbol_links.try_get(symbol)
}
