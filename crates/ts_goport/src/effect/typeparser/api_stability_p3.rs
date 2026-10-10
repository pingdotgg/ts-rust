//! Port of Effect-TS/tsgo `internal/typeparser/api_stability.go` lines
//! 2683-5023 at `@effect/tsgo@0.51.1` (`47cb1ed7`): declaration surfaces,
//! heritage, class static and anonymous surfaces, mapped and component
//! surfaces, signatures and substitution, provenance, child boundaries,
//! findings and the inspect and visibility filters. See `api_stability_p1.rs`
//! for the split and the signature conventions.
//!
//! PORT: Go `*ast.NodeList` is `NodeList`; Go `map[*T]bool` work sets are
//! `&mut FxHashMap<_, bool>`; Go free functions that read symbols, types or
//! signatures take `c`. Go `defer delete(m, k)` is a labeled block (or an
//! inner call) followed by the removal. Go `t.Target()` panics for a type
//! that has no target, so every read keeps Go's `&&` guard order.

use crate::effect::typeparser::*;
use crate::prelude::*;

impl ApiStabilityAnalysis {
    // Go: typeparser/api_stability.go apiStabilityAnalysis.collectDeclarationSurface
    /// collectDeclarationSurface inspects the public surface declared by a class or
    /// interface declaration: its own members, index signatures and signatures,
    /// its type parameters, and (when expanded) its inherited members. The
    /// declaration type passed here is always uninstantiated; the substitution
    /// context carries the represented type arguments of the reference that reached
    /// it.
    pub fn collect_declaration_surface(
        &mut self,
        tp: &mut TypeParser<'_>,
        surface: &mut ApiStabilitySurface,
        declaration: TypeId,
        inspect: ApiStabilityInspection,
        subst: &ApiStabilitySubstitution,
    ) {
        if declaration.is_nil() {
            return;
        }
        let symbol = tp.checker.ty(declaration).symbol();
        if symbol.is_nil() || !self.should_inspect_symbol(tp, symbol) {
            return;
        }
        self.record_symbol(tp, surface, symbol);
        let key = ApiStabilitySymbolKey {
            symbol,
            carrier: subst.carrier(),
        };
        if self.active_symbols.get(&key).copied().unwrap_or(false) {
            surface.cut_by_cycle();
            return;
        }
        self.active_symbols.insert(key, true);
        self.collect_declaration_surface_body(tp, surface, declaration, symbol, inspect, subst);
        self.active_symbols.remove(&key);
    }

    /// PORT: the body of Go `collectDeclarationSurface` after the active-symbol
    /// insert (Go `defer delete(a.activeSymbols, key)`).
    fn collect_declaration_surface_body(
        &mut self,
        tp: &mut TypeParser<'_>,
        surface: &mut ApiStabilitySurface,
        declaration: TypeId,
        symbol: SymbolId,
        inspect: ApiStabilityInspection,
        subst: &ApiStabilitySubstitution,
    ) {
        if inspect == ApiStabilityInspection::Shallow {
            // A named dependency keeps the shallow boundary: its own public call and
            // construct signatures (and their inherited signatures) are part of the
            // represented surface, but its arbitrary members are not expanded.
            self.collect_declaration_signatures(tp, surface, symbol, subst, false);
            self.collect_heritage(
                tp,
                surface,
                declaration,
                ApiStabilityInspection::Shallow,
                subst,
            );
            return;
        }

        let local_type_parameters: Vec<TypeId> = tp
            .checker
            .ty(declaration)
            .data
            .as_interface_type()
            .map(|iface| iface.local_type_parameters().to_vec())
            .unwrap_or_default();
        for type_parameter in local_type_parameters {
            let child =
                self.type_surface(tp, type_parameter, ApiStabilityInspection::Shallow, subst);
            surface.merge(child);
        }

        let members = tp.checker.sym(symbol).members;
        if self.collect_resolved_member_table_if_materialized(
            tp,
            surface,
            declaration,
            symbol,
            subst,
        ) {
        } else if members.is_some() {
            self.collect_resolved_member_table(tp, surface, members, symbol, subst);
            // A computed member name that the checker must evaluate to resolve the
            // member table is not part of the early table; the surface is incomplete
            // and is never treated as stable.
            if self.symbol_has_late_bound_members(tp, symbol) {
                surface.block();
            }
        } else if self.member_table_resolution_is_safe(tp, declaration, subst) {
            let members = self.materialized_members_of_symbol(tp.checker, symbol);
            if members.is_some() {
                self.collect_resolved_member_table(tp, surface, members, symbol, subst);
            } else {
                surface.block();
            }
        } else {
            surface.block();
        }

        self.collect_declaration_signatures(tp, surface, symbol, subst, false);
        self.collect_index_infos_of_declaration(tp, surface, declaration, symbol, subst);
        self.collect_heritage(tp, surface, declaration, inspect, subst);
    }

    // Go: typeparser/api_stability.go apiStabilityAnalysis.memberSurface
    /// memberSurface inspects one class or interface member. The member's own
    /// declared `@stability` tag is metadata and always contributes; when the tag is
    /// explicit the member is a child boundary for its own represented type and its
    /// internals are not expanded. Otherwise the member's represented type is
    /// preferred from the checker's cache and otherwise materialized through the
    /// ordinary lazy accessor; a member that genuinely has no type still exposes its
    /// raw declaration signature.
    pub fn member_surface(
        &mut self,
        tp: &mut TypeParser<'_>,
        member: SymbolId,
        subst: &ApiStabilitySubstitution,
    ) -> ApiStabilitySurface {
        let mut surface = new_api_stability_surface();
        if member.is_nil() {
            return surface;
        }
        self.record_symbol(tp, &mut surface, member);
        if tp
            .declared_api_stability_of_symbol(member)
            .declaration
            .is_some()
        {
            return surface;
        }
        let member_type = self.type_of_symbol_safely(tp, member);
        if member_type.is_nil() {
            // A method or accessor whose function type is genuinely unavailable
            // still exposes its raw declaration signature. The signature belongs to
            // the class or interface that declares the member, so that declaring
            // symbol is the owner comparison: a tagged declaring class never bounds
            // its own member's raw signature.
            let mut signature_owner = self.member_declaring_symbol(tp, member);
            if signature_owner.is_nil() {
                signature_owner = member;
            }
            let signatures = self.materialized_signatures_of_symbol(tp.checker, member);
            if !signatures.is_empty() {
                for signature in signatures {
                    if self.child_signature_boundary(tp, &mut surface, signature, signature_owner) {
                        continue;
                    }
                    let child = self.signature_surface(tp, signature, subst);
                    surface.merge(child);
                }
                return surface;
            }
            surface.block();
            return surface;
        }
        let child = self.type_surface(tp, member_type, ApiStabilityInspection::Shallow, subst);
        surface.merge(child);
        let declarations: Vec<Node> = tp.checker.sym(member).declarations.to_vec();
        for declaration in declarations {
            if !self.should_inspect_member_declaration(declaration) {
                continue;
            }
            self.collect_declaration_provenance(tp, &mut surface, declaration, member_type, subst);
        }
        surface
    }

    // Go: typeparser/api_stability.go apiStabilityAnalysis.collectDeclarationSignatures
    /// collectDeclarationSignatures collects a declaration's own public call and
    /// construct signatures. Construct signatures declared under the class
    /// constructor member are only included when includeClassConstructors is set;
    /// a class instance surface does not expose them, while the static side does.
    /// The early member table is read directly: call, construct and constructor
    /// symbols are never late-bound, so inspecting them must not resolve a
    /// late-bound computed-name member table.
    pub fn collect_declaration_signatures(
        &mut self,
        tp: &mut TypeParser<'_>,
        surface: &mut ApiStabilitySurface,
        symbol: SymbolId,
        subst: &ApiStabilitySubstitution,
        include_class_constructors: bool,
    ) {
        if symbol.is_nil() {
            return;
        }
        let members = tp.checker.sym(symbol).members;
        if members.is_nil() {
            return;
        }
        let mut internals = vec![INTERNAL_SYMBOL_NAME_CALL, INTERNAL_SYMBOL_NAME_NEW];
        if include_class_constructors {
            internals.push(INTERNAL_SYMBOL_NAME_CONSTRUCTOR);
        }
        for internal in internals {
            let member = tp.checker.symbols.get(members, internal);
            if member.is_nil() {
                continue;
            }
            if !self.signature_member_resolution_is_safe(tp, member, subst) {
                surface.block();
                continue;
            }
            for signature in self.materialized_signatures_of_symbol(tp.checker, member) {
                if self.child_signature_boundary(tp, surface, signature, symbol) {
                    continue;
                }
                let child = self.signature_surface(tp, signature, subst);
                surface.merge(child);
            }
        }
    }

    // Go: typeparser/api_stability.go apiStabilityAnalysis.collectIndexInfosOfDeclaration
    /// collectIndexInfosOfDeclaration reads a declaration's index signatures. The
    /// already resolved index infos are used when the checker materialized them;
    /// otherwise the declared index signatures contribute their own `@stability`
    /// tag and their materialized key/value annotations, and an unmaterialized
    /// annotation leaves the surface incomplete. The resolved infos include index
    /// signatures the compiler flattened in from inherited bases; each info is
    /// attributed to its declaring class or interface so an explicitly tagged base's
    /// index signature stays a child boundary. The declared-only fallback carries
    /// own declarations, so warm and cold runs agree on that boundary.
    pub fn collect_index_infos_of_declaration(
        &mut self,
        tp: &mut TypeParser<'_>,
        surface: &mut ApiStabilitySurface,
        declaration: TypeId,
        symbol: SymbolId,
        subst: &ApiStabilitySubstitution,
    ) {
        let (infos, ok) = checker_integration::get_resolved_index_infos_of_type_if_materialized(
            tp.checker,
            declaration,
        );
        if ok {
            for info in infos {
                if info.is_nil() {
                    continue;
                }
                self.collect_index_info_surface(tp, surface, info, subst, symbol);
            }
            return;
        }
        let infos = checker_integration::get_declared_index_infos_of_symbol(tp.checker, symbol);
        for info in infos {
            if info.is_nil() {
                continue;
            }
            let (index_declaration, key_type, value_type) = {
                let data = tp.checker.index_info(info);
                (data.declaration, data.key_type, data.value_type)
            };
            if index_declaration.is_nil() {
                continue;
            }
            self.collect_index_signature_surface(
                tp,
                surface,
                index_declaration,
                key_type,
                value_type,
                subst,
                symbol,
            );
        }
    }

    // Go: typeparser/api_stability.go apiStabilityAnalysis.collectIndexInfoSurface
    /// collectIndexInfoSurface inspects one already resolved index info. The index
    /// declaration's own `@stability` tag always contributes; when the tag is
    /// explicit the index signature is a child boundary and its key and value
    /// internals are not expanded. A resolved index info the compiler flattened in
    /// from an explicitly tagged base (its declaring class or interface differs from
    /// owner) is bounded the same way. A key or value type the checker has not
    /// resolved is materialized lazily from the declaration when possible.
    pub fn collect_index_info_surface(
        &mut self,
        tp: &mut TypeParser<'_>,
        surface: &mut ApiStabilitySurface,
        info: IndexInfoId,
        subst: &ApiStabilitySubstitution,
        owner: SymbolId,
    ) {
        if info.is_nil() {
            return;
        }
        let (declaration, mut key_type, mut value_type) = {
            let data = tp.checker.index_info(info);
            (data.declaration, data.key_type, data.value_type)
        };
        if self.declaration_child_boundary(tp, surface, declaration, owner) {
            return;
        }
        if key_type.is_nil() {
            key_type = self.represented_type_from_node(
                tp,
                api_stability_index_signature_key_node(declaration),
                subst,
            );
        }
        if value_type.is_nil() {
            value_type = self.represented_type_from_node(
                tp,
                api_stability_index_signature_value_node(declaration),
                subst,
            );
        }
        if key_type.is_some() {
            let child = self.type_surface(tp, key_type, ApiStabilityInspection::Shallow, subst);
            surface.merge(child);
        } else {
            surface.block();
        }
        if value_type.is_some() {
            let child = self.type_surface(tp, value_type, ApiStabilityInspection::Shallow, subst);
            surface.merge(child);
            self.collect_type_node_provenance(
                tp,
                surface,
                api_stability_index_signature_value_node(declaration),
                value_type,
                subst,
            );
        } else {
            surface.block();
        }
    }

    // Go: typeparser/api_stability.go apiStabilityAnalysis.collectIndexSignatureSurface
    /// collectIndexSignatureSurface inspects one declared index signature. The key
    /// and value types are used when the checker has already represented them;
    /// otherwise the declaration's annotations are materialized lazily. An explicit
    /// `@stability` tag makes the signature a child boundary: the tag contributes
    /// and its key and value internals are not expanded, and a declaration whose
    /// declaring class or interface is explicitly tagged (and differs from owner) is
    /// bounded the same way.
    // PORT: Go `declaration.AsIndexSignatureDeclaration()` is nil-checked; a
    // declaration of another kind returns there.
    #[allow(clippy::too_many_arguments)]
    pub fn collect_index_signature_surface(
        &mut self,
        tp: &mut TypeParser<'_>,
        surface: &mut ApiStabilitySurface,
        declaration: Node,
        key_type: TypeId,
        value_type: TypeId,
        subst: &ApiStabilitySubstitution,
        owner: SymbolId,
    ) {
        if declaration.is_nil() {
            return;
        }
        if self.declaration_child_boundary(tp, surface, declaration, owner) {
            return;
        }
        if declaration.kind() != SyntaxKind::IndexSignature {
            return;
        }
        let mut key_type = key_type;
        for parameter in declaration.parameters() {
            if parameter.is_nil() || parameter.kind() != SyntaxKind::Parameter {
                continue;
            }
            let type_node = parameter.type_();
            if key_type.is_nil() {
                key_type = self.represented_type_from_node(tp, type_node, subst);
            }
            if key_type.is_some() {
                let child = self.type_surface(tp, key_type, ApiStabilityInspection::Shallow, subst);
                surface.merge(child);
                self.collect_type_node_provenance(tp, surface, type_node, key_type, subst);
            } else {
                surface.block();
            }
        }
        let mut value_type = value_type;
        let value_node = declaration.type_();
        if value_type.is_nil() {
            value_type = self.represented_type_from_node(tp, value_node, subst);
        }
        if value_type.is_some() {
            let child = self.type_surface(tp, value_type, ApiStabilityInspection::Shallow, subst);
            surface.merge(child);
            self.collect_type_node_provenance(tp, surface, value_node, value_type, subst);
            return;
        }
        surface.block();
    }

    // Go: typeparser/api_stability.go apiStabilityAnalysis.collectHeritage
    /// collectHeritage inspects a declaration's inherited surface. Base types the
    /// checker has already resolved are preferred; otherwise the declaration's
    /// heritage is materialized through the ordinary lazy accessor. The clauses are
    /// never resolved from syntax.
    pub fn collect_heritage(
        &mut self,
        tp: &mut TypeParser<'_>,
        surface: &mut ApiStabilitySurface,
        declaration: TypeId,
        inspect: ApiStabilityInspection,
        subst: &ApiStabilitySubstitution,
    ) {
        let (mut bases, resolved) =
            checker_integration::get_resolved_base_types_of_type_if_materialized(
                tp.checker,
                declaration,
            );
        if !resolved {
            let symbol = tp.checker.ty(declaration).symbol();
            if !api_stability_symbol_declares_heritage(tp.checker, symbol) {
                return;
            }
            let (lazy_bases, ok) = self.base_types_safely(tp, declaration, subst);
            if !ok {
                surface.block();
                return;
            }
            bases = lazy_bases;
        }
        for base in bases {
            self.collect_inherited_base(tp, surface, base, inspect, subst);
        }
    }

    // Go: typeparser/api_stability.go apiStabilityAnalysis.collectInheritedBase
    /// collectInheritedBase preserves inheritance semantics through the intersection
    /// return types of superclass factories. Direct references still use typeSurface.
    pub fn collect_inherited_base(
        &mut self,
        tp: &mut TypeParser<'_>,
        surface: &mut ApiStabilitySurface,
        base: TypeId,
        inspect: ApiStabilityInspection,
        subst: &ApiStabilitySubstitution,
    ) {
        if base.is_nil() {
            return;
        }
        if tp
            .checker
            .ty(base)
            .flags()
            .intersects(TypeFlags::INTERSECTION)
            && self.heritage_child_boundary_symbol(tp, base, base).is_nil()
        {
            self.collect_child_type_arguments(tp, surface, base, subst);
            let constituents = tp.checker.ty(base).types().to_vec();
            for constituent in constituents {
                self.collect_inherited_base(tp, surface, constituent, inspect, subst);
            }
            return;
        }
        let mut target = base;
        if tp
            .checker
            .ty(base)
            .object_flags()
            .intersects(ObjectFlags::REFERENCE)
            && tp.checker.ty(base).target().is_some()
        {
            target = tp.checker.ty(base).target();
        }
        if target.is_nil()
            || !tp
                .checker
                .ty(target)
                .object_flags()
                .intersects(ObjectFlags::CLASS_OR_INTERFACE)
        {
            let child = self.type_surface(tp, base, ApiStabilityInspection::Shallow, subst);
            surface.merge(child);
            return;
        }
        let target_symbol = tp.checker.ty(target).symbol();
        if self.optional_tagged_base(tp, target_symbol) {
            return;
        }
        let boundary = self.heritage_child_boundary_symbol(tp, base, target);
        if boundary.is_some() {
            self.record_symbol(tp, surface, boundary);
            self.collect_child_type_arguments(tp, surface, base, subst);
            return;
        }
        let parameters = self.reference_type_parameters(tp, target);
        let arguments = self.reference_arguments(tp, base, target);
        let base_subst = self.extend_parameters_substitution(subst, base, &parameters, &arguments);
        self.collect_declaration_surface(tp, surface, target, inspect, &base_subst);
    }

    // Go: typeparser/api_stability.go apiStabilityAnalysis.collectClassStaticSurface
    /// collectClassStaticSurface inspects the class static side. Its members are
    /// declared on the class symbol's exports and cannot depend on class type
    /// parameters. The instance side of a class value is included when the class
    /// value is directly exported, because `typeof C` exposes it through
    /// construction.
    pub fn collect_class_static_surface(
        &mut self,
        tp: &mut TypeParser<'_>,
        surface: &mut ApiStabilitySurface,
        t: TypeId,
        inspect: ApiStabilityInspection,
        subst: &ApiStabilitySubstitution,
    ) {
        let symbol = tp.checker.ty(t).symbol();
        if symbol.is_nil() || !self.should_inspect_symbol(tp, symbol) {
            return;
        }
        self.record_symbol(tp, surface, symbol);
        if inspect == ApiStabilityInspection::Expand {
            let instance = self.declared_type_safely(tp, symbol);
            if instance.is_some() {
                let child = self.type_surface(tp, instance, ApiStabilityInspection::Expand, subst);
                surface.merge(child);
            }
            // The class static members of a directly expanded class value are part
            // of its own surface. A nested `typeof C` keeps the shallow boundary:
            // its declared symbol and directly exposed call and construct
            // signatures are inspected, but its static members are not expanded.
            let (members, ok) =
                checker_integration::get_resolved_members_of_type_if_materialized(tp.checker, t);
            if ok {
                self.collect_class_static_members(tp, surface, members, symbol);
            } else {
                let exports = tp.checker.sym(symbol).exports;
                if exports.is_some() {
                    self.collect_class_static_members(tp, surface, exports, symbol);
                    // The early exports table omits computed members (unique-symbol and
                    // dynamic names); the surface is incomplete until the checker
                    // late-binds them.
                    if self.symbol_has_late_bound_members(tp, symbol) {
                        surface.block();
                    }
                }
            }
        }
        self.collect_declaration_signatures(tp, surface, symbol, subst, true);
        for kind in [SignatureKind::CALL, SignatureKind::CONSTRUCT] {
            let (signatures, ok) =
                checker_integration::get_resolved_signatures_of_type_if_materialized(
                    tp.checker, t, kind,
                );
            if ok {
                for signature in signatures {
                    if self.child_signature_boundary(tp, surface, signature, symbol) {
                        continue;
                    }
                    let child = self.signature_surface(tp, signature, subst);
                    surface.merge(child);
                }
            }
        }
    }

    /// PORT: the two identical member loops of Go `collectClassStaticSurface`
    /// (the resolved member table and the early exports table).
    fn collect_class_static_members(
        &mut self,
        tp: &mut TypeParser<'_>,
        surface: &mut ApiStabilitySurface,
        members: SymbolTable,
        symbol: SymbolId,
    ) {
        for name in sorted_symbol_table_keys(tp.checker, members) {
            if name == "prototype" {
                continue;
            }
            let member = tp.checker.symbols.get(members, &name);
            if member.is_nil() || api_stability_symbol_is_non_public(tp.checker, member) {
                continue;
            }
            if !api_stability_member_table_name_is_visible(tp.checker, &name, member) {
                continue;
            }
            if self.optional_tagged_inherited_member(tp, member, symbol) {
                continue;
            }
            let boundary = self.inherited_child_boundary_symbol(tp, member, symbol);
            if boundary.is_some() {
                self.record_symbol(tp, surface, boundary);
                continue;
            }
            let child = self.member_surface(tp, member, &ApiStabilitySubstitution::default());
            surface.merge(child);
        }
    }

    // Go: typeparser/api_stability.go apiStabilityAnalysis.collectAnonymousSurface
    /// collectAnonymousSurface inspects a finite anonymous object type. The concrete
    /// member surface of an instantiated anonymous type is preferred and read
    /// directly when materialized. An unmaterialized instantiated member surface is
    /// materialized through the ordinary lazy property accessor only when the
    /// recursion guard establishes that member resolution is bounded; otherwise the
    /// raw target members are read under the mapper substitution with their own
    /// gated reads. A raw anonymous type (the target of an instantiation reached
    /// through a heritage or mapper path) is inspected through its declared members
    /// under the active substitution.
    pub fn collect_anonymous_surface(
        &mut self,
        tp: &mut TypeParser<'_>,
        surface: &mut ApiStabilitySurface,
        t: TypeId,
        subst: &ApiStabilitySubstitution,
    ) {
        let instantiated = tp
            .checker
            .ty(t)
            .object_flags()
            .intersects(ObjectFlags::INSTANTIATED)
            && tp.checker.ty(t).target().is_some();
        let mut source = t;
        let mut current = subst.clone();
        if instantiated {
            source = tp.checker.ty(t).target();
            let mapper = tp.checker.ty(t).mapper();
            if mapper.is_some() {
                current = self.extend_mapper_substitution(subst, t, mapper);
            }
        }
        let source_symbol = tp.checker.ty(source).symbol();
        self.record_symbol(tp, surface, source_symbol);
        if instantiated {
            let empty = ApiStabilitySubstitution::default();
            if self.collect_resolved_member_table_if_materialized(
                tp,
                surface,
                t,
                source_symbol,
                &empty,
            ) {
            } else if self.member_table_resolution_is_safe(tp, t, &current) {
                self.note_materializing_read();
                for member in tp.checker.get_properties_of_type_exported(t) {
                    if member.is_nil()
                        || tp
                            .checker
                            .sym(member)
                            .flags
                            .intersects(SymbolFlags::TYPE_PARAMETER)
                        || api_stability_symbol_is_non_public(tp.checker, member)
                    {
                        continue;
                    }
                    let boundary = self.inherited_child_boundary_symbol(tp, member, source_symbol);
                    if boundary.is_some() {
                        self.record_symbol(tp, surface, boundary);
                        continue;
                    }
                    let child = self.member_surface(tp, member, &empty);
                    surface.merge(child);
                }
            } else if source_symbol.is_some() && tp.checker.sym(source_symbol).members.is_some() {
                let members = tp.checker.sym(source_symbol).members;
                self.collect_resolved_member_table(tp, surface, members, source_symbol, &current);
                if self.symbol_has_late_bound_members(tp, source_symbol) {
                    surface.block();
                }
            } else {
                surface.block();
            }
            self.collect_anonymous_index_infos(tp, surface, t, source, &empty, &current);
            self.collect_function_signatures(tp, surface, t, source, &current);
            return;
        }
        if source_symbol.is_some() {
            let members = tp.checker.sym(source_symbol).members;
            if self.collect_resolved_member_table_if_materialized(
                tp,
                surface,
                source,
                source_symbol,
                &current,
            ) {
            } else if members.is_some() {
                self.collect_resolved_member_table(tp, surface, members, source_symbol, &current);
                if self.symbol_has_late_bound_members(tp, source_symbol) {
                    surface.block();
                }
            } else if self.member_table_resolution_is_safe(tp, source, &current) {
                let members = self.materialized_members_of_symbol(tp.checker, source_symbol);
                if members.is_some() {
                    self.collect_resolved_member_table(
                        tp,
                        surface,
                        members,
                        source_symbol,
                        &current,
                    );
                }
                // A function-like symbol has no member table of its own; its call
                // signatures are declared directly on the symbol and are read by
                // collectFunctionSignatures.
            } else {
                surface.block();
            }
        }
        self.collect_anonymous_index_infos(tp, surface, t, source, &current, &current);
        self.collect_function_signatures(tp, surface, t, source, &current);
    }

    // Go: typeparser/api_stability.go apiStabilityAnalysis.collectResolvedMemberTableIfMaterialized
    /// collectResolvedMemberTableIfMaterialized merges the checker's already
    /// resolved member table when one exists.
    pub fn collect_resolved_member_table_if_materialized(
        &mut self,
        tp: &mut TypeParser<'_>,
        surface: &mut ApiStabilitySurface,
        t: TypeId,
        owner: SymbolId,
        subst: &ApiStabilitySubstitution,
    ) -> bool {
        let (members, ok) =
            checker_integration::get_resolved_members_of_type_if_materialized(tp.checker, t);
        if ok {
            self.collect_resolved_member_table(tp, surface, members, owner, subst);
            return true;
        }
        false
    }

    // Go: typeparser/api_stability.go apiStabilityAnalysis.collectAnonymousIndexInfos
    /// collectAnonymousIndexInfos reads the index signatures of an anonymous type.
    /// The resolved infos of the (possibly instantiated) type are preferred; when
    /// the checker has not resolved them, the raw declared infos contribute under
    /// the mapper substitution and their annotations are read through the guard.
    pub fn collect_anonymous_index_infos(
        &mut self,
        tp: &mut TypeParser<'_>,
        surface: &mut ApiStabilitySurface,
        t: TypeId,
        source: TypeId,
        subst: &ApiStabilitySubstitution,
        mapper_subst: &ApiStabilitySubstitution,
    ) {
        let mut owner = SymbolId::NIL;
        if source.is_some() {
            owner = tp.checker.ty(source).symbol();
        }
        let (infos, ok) =
            checker_integration::get_resolved_index_infos_of_type_if_materialized(tp.checker, t);
        if ok {
            let resolved_subst = if source != t {
                ApiStabilitySubstitution::default()
            } else {
                subst.clone()
            };
            for info in infos {
                if info.is_nil() {
                    continue;
                }
                self.collect_index_info_surface(tp, surface, info, &resolved_subst, owner);
            }
            return;
        }
        if source.is_nil() || tp.checker.ty(source).symbol().is_nil() {
            return;
        }
        let source_symbol = tp.checker.ty(source).symbol();
        if source != t {
            self.collect_index_infos_of_symbol(tp, surface, source_symbol, mapper_subst);
            return;
        }
        self.collect_index_infos_of_symbol(tp, surface, source_symbol, subst);
    }

    // Go: typeparser/api_stability.go apiStabilityAnalysis.collectIndexInfosOfSymbol
    /// collectIndexInfosOfSymbol contributes the declared index signatures of a
    /// symbol under a substitution context. Key and value annotations are read
    /// through the recursion guard; an unavailable annotation leaves the surface
    /// incomplete.
    pub fn collect_index_infos_of_symbol(
        &mut self,
        tp: &mut TypeParser<'_>,
        surface: &mut ApiStabilitySurface,
        symbol: SymbolId,
        subst: &ApiStabilitySubstitution,
    ) {
        if symbol.is_nil() {
            return;
        }
        for info in checker_integration::get_declared_index_infos_of_symbol(tp.checker, symbol) {
            if info.is_nil() {
                continue;
            }
            let (declaration, key_type, value_type) = {
                let data = tp.checker.index_info(info);
                (data.declaration, data.key_type, data.value_type)
            };
            if declaration.is_nil() {
                continue;
            }
            self.collect_index_signature_surface(
                tp,
                surface,
                declaration,
                key_type,
                value_type,
                subst,
                symbol,
            );
        }
    }

    // Go: typeparser/api_stability.go apiStabilityAnalysis.collectFunctionSignatures
    /// collectFunctionSignatures reads the call and construct signatures of an
    /// anonymous function-like type. Resolved signatures are preferred. An
    /// unmaterialized surface is read from the raw declaration signatures under the
    /// active substitution: creating a raw signature resolves its parameter
    /// annotations but never instantiates the enclosing application, and the
    /// guarded per-signature reads stay bounded.
    pub fn collect_function_signatures(
        &mut self,
        tp: &mut TypeParser<'_>,
        surface: &mut ApiStabilitySurface,
        t: TypeId,
        source: TypeId,
        subst: &ApiStabilitySubstitution,
    ) {
        let mut owner = SymbolId::NIL;
        if source.is_some() {
            owner = tp.checker.ty(source).symbol();
        }
        for kind in [SignatureKind::CALL, SignatureKind::CONSTRUCT] {
            let (signatures, ok) =
                checker_integration::get_resolved_signatures_of_type_if_materialized(
                    tp.checker, t, kind,
                );
            if ok {
                for signature in signatures {
                    if self.child_signature_boundary(tp, surface, signature, owner) {
                        continue;
                    }
                    let child = self.signature_surface(tp, signature, subst);
                    surface.merge(child);
                }
                continue;
            }
            if source.is_nil() || tp.checker.ty(source).symbol().is_nil() {
                surface.block();
                continue;
            }
            let internal = if kind == SignatureKind::CONSTRUCT {
                INTERNAL_SYMBOL_NAME_NEW
            } else {
                INTERNAL_SYMBOL_NAME_CALL
            };
            let symbol = tp.checker.ty(source).symbol();
            let mut member = tp
                .checker
                .symbols
                .get(tp.checker.sym(symbol).members, internal);
            if member.is_nil()
                && kind == SignatureKind::CALL
                && tp.checker.sym(symbol).flags.intersects(
                    SymbolFlags::FUNCTION
                        | SymbolFlags::METHOD
                        | SymbolFlags::GET_ACCESSOR
                        | SymbolFlags::SET_ACCESSOR
                        | SymbolFlags::ACCESSOR,
                )
            {
                // A function or accessor declaration keeps its call signatures on
                // its own symbol rather than in a member table.
                member = symbol;
            }
            if member.is_nil() {
                continue;
            }
            if !self.signature_member_resolution_is_safe(tp, member, subst) {
                surface.block();
                continue;
            }
            for signature in self.materialized_signatures_of_symbol(tp.checker, member) {
                if self.child_signature_boundary(tp, surface, signature, owner) {
                    continue;
                }
                let child = self.signature_surface(tp, signature, subst);
                surface.merge(child);
            }
        }
    }

    // Go: typeparser/api_stability.go apiStabilityAnalysis.collectMappedSurface
    /// collectMappedSurface reads the represented mapped type components. An
    /// instantiated mapped type is read through its target so the mapped parameter
    /// stays uninstantiated and the mapper provides the substitution. Cached
    /// components are preferred; a missing component is materialized through the
    /// ordinary mapped accessors unless its annotation would expand a recursive
    /// alias. Whatever remains unavailable leaves the surface incomplete.
    // PORT: Go `source.AsMappedType()` is nil-checked; the port tests the mapped
    // flag, which is what makes the cast succeed.
    pub fn collect_mapped_surface(
        &mut self,
        tp: &mut TypeParser<'_>,
        surface: &mut ApiStabilitySurface,
        t: TypeId,
        subst: &ApiStabilitySubstitution,
    ) {
        let symbol = tp.checker.ty(t).symbol();
        self.record_symbol(tp, surface, symbol);
        let mut source = t;
        let mut current = subst.clone();
        if tp
            .checker
            .ty(t)
            .object_flags()
            .intersects(ObjectFlags::INSTANTIATED)
            && tp.checker.ty(t).target().is_some()
        {
            source = tp.checker.ty(t).target();
            let mapper = tp.checker.ty(t).mapper();
            if mapper.is_some() {
                current = self.extend_mapper_substitution(subst, t, mapper);
            }
        }
        if !tp
            .checker
            .ty(source)
            .object_flags()
            .intersects(ObjectFlags::MAPPED)
        {
            return;
        }
        let mut constraint_node = Node::NIL;
        let mut name_node = Node::NIL;
        let mut template_node = Node::NIL;
        let source_symbol = tp.checker.ty(source).symbol();
        if source_symbol.is_some() {
            for &declaration in &tp.checker.sym(source_symbol).declarations {
                if declaration.is_nil() || declaration.kind() != SyntaxKind::MappedType {
                    continue;
                }
                let type_parameter = declaration.type_parameter();
                if type_parameter.is_some() && type_parameter.kind() == SyntaxKind::TypeParameter {
                    constraint_node = type_parameter.constraint();
                }
                name_node = declaration.name_type();
                template_node = declaration.type_();
                break;
            }
        }
        let mut constraint =
            checker_integration::get_mapped_type_constraint_type(tp.checker, source);
        if constraint.is_nil() {
            constraint = self.type_from_node_safely(tp, constraint_node, &current);
        }
        if constraint.is_some() {
            let child =
                self.type_surface(tp, constraint, ApiStabilityInspection::Shallow, &current);
            surface.merge(child);
        } else {
            surface.block();
        }
        let mut name = checker_integration::get_mapped_type_name_type(tp.checker, source);
        if name.is_nil() {
            name = self.type_from_node_safely(tp, name_node, &current);
        }
        if name.is_some() {
            let child = self.type_surface(tp, name, ApiStabilityInspection::Shallow, &current);
            surface.merge(child);
        } else if name_node.is_some()
            || api_stability_mapped_declaration_has_name(tp.checker, source)
        {
            surface.block();
        }
        let mut template = checker_integration::get_mapped_type_template_type(tp.checker, source);
        if template.is_nil() {
            template = self.type_from_node_safely(tp, template_node, &current);
        }
        if template.is_some() {
            let child = self.type_surface(tp, template, ApiStabilityInspection::Shallow, &current);
            surface.merge(child);
        } else {
            surface.block();
        }
    }

    // Go: typeparser/api_stability.go apiStabilityAnalysis.componentSurface
    /// componentSurface inspects a finite anonymous component exactly once per
    /// recursion path and substitution context.
    pub fn component_surface(
        &mut self,
        tp: &mut TypeParser<'_>,
        t: TypeId,
        inspect: ApiStabilityInspection,
        subst: &ApiStabilitySubstitution,
    ) -> ApiStabilitySurface {
        if t.is_nil() {
            return new_api_stability_surface();
        }
        let key = ApiStabilityTypeKey {
            t,
            inspect,
            carrier: subst.carrier(),
        };
        if let Some(cached) = self.session_components.get(&key) {
            return cached.clone();
        }
        self.type_frames.insert(key, subst.frame.clone());
        if self.active_components.get(&key).copied().unwrap_or(false) {
            return cycle_cut_surface();
        }
        if !self.consume_work() {
            return blocked_surface();
        }
        self.active_components.insert(key, true);
        let start_epoch = self.materialization_epoch;
        let mut surface = self.collect_object_component_surface(tp, t, inspect, subst);
        self.active_components.remove(&key);
        if surface.blocked {
            surface.observed_epoch = start_epoch;
        }
        self.store_component_surface(key, surface.clone());
        surface
    }

    // Go: typeparser/api_stability.go apiStabilityAnalysis.collectObjectComponentSurface
    /// collectObjectComponentSurface is used when a represented object type must be
    /// inspected as a finite anonymous surface (mapped targets, reference targets
    /// that are anonymous). Class and interface declarations never reach this path.
    pub fn collect_object_component_surface(
        &mut self,
        tp: &mut TypeParser<'_>,
        t: TypeId,
        inspect: ApiStabilityInspection,
        subst: &ApiStabilitySubstitution,
    ) -> ApiStabilitySurface {
        let mut surface = new_api_stability_surface();
        if t.is_nil() {
            return surface;
        }
        if tp
            .checker
            .ty(t)
            .object_flags()
            .intersects(ObjectFlags::CLASS_OR_INTERFACE)
        {
            // A class or interface target is inspected through its declaration, not
            // through the resolved member surface.
            self.collect_declaration_surface(tp, &mut surface, t, inspect, subst);
            return surface;
        }
        let symbol = tp.checker.ty(t).symbol();
        if symbol.is_some() && tp.checker.sym(symbol).flags.intersects(SymbolFlags::CLASS) {
            self.collect_class_static_surface(tp, &mut surface, t, inspect, subst);
            return surface;
        }
        self.collect_anonymous_surface(tp, &mut surface, t, subst);
        surface
    }

    // Go: typeparser/api_stability.go apiStabilityAnalysis.signatureSurface
    /// signatureSurface reads a signature's uninstantiated target together with its
    /// represented substitution. Using the target keeps a generic signature's
    /// parameter and return types uninstantiated (so a recursive alias is never
    /// evaluated), while the substitution frame retains the instantiated context
    /// (so an inferred return such as `(x: T) => T` instantiated with `E` still
    /// exposes `E`). The concrete signature pointer and the substitution context are
    /// both part of the cache key, so the same raw signature analyzed under
    /// different enclosing mappers never shares a cached result. A complete,
    /// context-free concrete signature surface is looked up in the per-checker
    /// shared caches and composed from the immutable snapshot plus the signature's
    /// declared tag; a result computed under a substitution carrier stays
    /// analysis-local.
    pub fn signature_surface(
        &mut self,
        tp: &mut TypeParser<'_>,
        signature: SignatureId,
        subst: &ApiStabilitySubstitution,
    ) -> ApiStabilitySurface {
        if signature.is_nil() {
            return new_api_stability_surface();
        }
        let raw = raw_signature(tp.checker, signature);
        if raw.is_nil() || api_stability_signature_is_non_public(tp.checker, raw) {
            return new_api_stability_surface();
        }
        let key = ApiStabilitySignatureKey {
            signature,
            carrier: subst.carrier(),
        };
        if let Some(cached) = self.session_signatures.get(&key) {
            return cached.clone();
        }
        if key.carrier.is_nil()
            && let Some(shared) = self.shared_signature_surface(tp, signature)
        {
            self.store_signature_surface(key, shared.clone());
            return shared;
        }
        self.signature_frames.insert(key, subst.frame.clone());
        if self.active_signatures.get(&key).copied().unwrap_or(false) {
            return cycle_cut_surface();
        }
        if !self.consume_work() {
            return blocked_surface();
        }
        self.active_signatures.insert(key, true);
        let start_epoch = self.materialization_epoch;
        let mut surface = self.collect_signature_surface(tp, raw, signature, subst);
        self.active_signatures.remove(&key);
        if surface.blocked {
            surface.observed_epoch = start_epoch;
        }
        self.store_signature_surface(key, surface.clone());
        surface
    }

    // Go: typeparser/api_stability.go apiStabilityAnalysis.collectSignatureSurface
    pub fn collect_signature_surface(
        &mut self,
        tp: &mut TypeParser<'_>,
        raw: SignatureId,
        concrete: SignatureId,
        subst: &ApiStabilitySubstitution,
    ) -> ApiStabilitySurface {
        let mut surface = new_api_stability_surface();
        if raw.is_nil() {
            return surface;
        }
        self.record_signature_stability(tp, &mut surface, raw);

        let mut signature_subst = subst.clone();
        let mut concrete_parameters: Vec<SymbolId> = Vec::new();
        let mut concrete_this = SymbolId::NIL;
        if concrete.is_some() && concrete != raw {
            signature_subst = self.extend_signature_substitution(subst, concrete);
            // The concrete signature's type arguments are represented in the
            // enclosing context; a compound argument is traversed shallowly.
            for argument in self.signature_arguments(tp, concrete) {
                let child = self.type_surface(tp, argument, ApiStabilityInspection::Shallow, subst);
                surface.merge(child);
            }
            concrete_parameters = tp.checker.sig(concrete).parameters().to_vec();
            concrete_this = tp.checker.sig(concrete).this_parameter();
        }
        let type_parameters = tp.checker.sig(raw).type_parameters().to_vec();
        for type_parameter in type_parameters {
            let (_, _, replaced) = self.substitute(tp, &signature_subst, type_parameter);
            if replaced {
                // A bound argument replaces the declaration; its constraint and default
                // are not part of the instantiated surface.
                continue;
            }
            let child = self.type_surface(
                tp,
                type_parameter,
                ApiStabilityInspection::Shallow,
                &signature_subst,
            );
            surface.merge(child);
        }
        let this_parameter = tp.checker.sig(raw).this_parameter();
        if this_parameter.is_some() {
            self.collect_signature_parameter(
                tp,
                &mut surface,
                this_parameter,
                concrete_this,
                subst,
                &signature_subst,
            );
        }
        let raw_parameters = tp.checker.sig(raw).parameters().to_vec();
        for (index, &parameter) in raw_parameters.iter().enumerate() {
            let concrete_parameter = concrete_parameters
                .get(index)
                .copied()
                .unwrap_or(SymbolId::NIL);
            self.collect_signature_parameter(
                tp,
                &mut surface,
                parameter,
                concrete_parameter,
                subst,
                &signature_subst,
            );
        }
        if concrete.is_some() && concrete != raw {
            // Prefer the concrete signature's represented return: the
            // instantiated return carries the compiler's actual outcome. When the
            // read is cut by a recursive application the raw declaration's
            // uninstantiated return annotation is used with the signature
            // substitution instead.
            let represented = self.return_type_safely(tp, concrete, &signature_subst);
            if represented.is_some() {
                let child =
                    self.type_surface(tp, represented, ApiStabilityInspection::Shallow, subst);
                surface.merge(child);
            } else {
                self.collect_return_type_surface(tp, &mut surface, raw, &signature_subst);
            }
        } else {
            self.collect_return_type_surface(tp, &mut surface, raw, &signature_subst);
        }
        self.collect_signature_provenance(tp, &mut surface, raw, &signature_subst);
        surface
    }

    /// PORT: the Go `collectParameter` closure of `collectSignatureSurface`.
    /// Prefer the concrete signature's represented parameter component: the
    /// instantiated parameter carries the compiler's actual outcome. When the read
    /// is cut by a recursive application the raw parameter's represented component
    /// is used with the signature substitution, so a bare type parameter still maps
    /// to its concrete argument without instantiating a hidden recursive alias.
    fn collect_signature_parameter(
        &mut self,
        tp: &mut TypeParser<'_>,
        surface: &mut ApiStabilitySurface,
        raw_parameter: SymbolId,
        concrete_parameter: SymbolId,
        subst: &ApiStabilitySubstitution,
        signature_subst: &ApiStabilitySubstitution,
    ) {
        if concrete_parameter.is_some() {
            let concrete_type = self.type_of_symbol_safely(tp, concrete_parameter);
            if concrete_type.is_some() {
                let child =
                    self.type_surface(tp, concrete_type, ApiStabilityInspection::Shallow, subst);
                surface.merge(child);
                return;
            }
        }
        let (parameter_type, known) = self.parameter_represented_type(tp, raw_parameter);
        if known {
            if parameter_type.is_some() {
                let child = self.type_surface(
                    tp,
                    parameter_type,
                    ApiStabilityInspection::Shallow,
                    signature_subst,
                );
                surface.merge(child);
            }
        } else {
            surface.block();
        }
    }

    // Go: typeparser/api_stability.go apiStabilityAnalysis.collectReturnTypeSurface
    /// collectReturnTypeSurface inspects the public return type of a signature. The
    /// signature's materialized return type is preferred; otherwise it is
    /// materialized lazily, with the already resolved annotation node, a primitive
    /// keyword, or a bare type parameter binder as fallbacks. A genuinely
    /// unavailable return leaves the surface incomplete.
    pub fn collect_return_type_surface(
        &mut self,
        tp: &mut TypeParser<'_>,
        surface: &mut ApiStabilitySurface,
        signature: SignatureId,
        subst: &ApiStabilitySubstitution,
    ) {
        if signature.is_nil() {
            return;
        }
        let (represented, known) = self.return_represented_type(tp, signature, subst);
        if !known {
            surface.block();
            return;
        }
        if represented.is_some() {
            let child = self.type_surface(tp, represented, ApiStabilityInspection::Shallow, subst);
            surface.merge(child);
        }
    }

    // Go: typeparser/api_stability.go apiStabilityAnalysis.returnRepresentedType
    /// returnRepresentedType returns the return type component of a signature. The
    /// boolean is false only when the component is genuinely unavailable, which
    /// blocks the surface; a primitive keyword is known and exposes nothing.
    pub fn return_represented_type(
        &mut self,
        tp: &mut TypeParser<'_>,
        signature: SignatureId,
        subst: &ApiStabilitySubstitution,
    ) -> (TypeId, bool) {
        if signature.is_nil() {
            return (TypeId::NIL, false);
        }
        let resolved = self.return_type_safely(tp, signature, subst);
        if resolved.is_some() {
            return (resolved, true);
        }
        let node = signature_return_type_node(tp.checker, signature);
        if node.is_nil() {
            return (TypeId::NIL, false);
        }
        let represented = checker_integration::get_resolved_type_from_type_node(tp.checker, node);
        if represented.is_some() {
            return (represented, true);
        }
        if api_stability_primitive_type_node(node) {
            return (TypeId::NIL, true);
        }
        let binder = self.type_parameter_from_annotation(tp, node);
        if binder.is_some() {
            return (binder, true);
        }
        (TypeId::NIL, false)
    }

    // Go: typeparser/api_stability.go apiStabilityAnalysis.parameterTypeOf
    /// parameterTypeOf returns the represented type of a parameter, or nil when the
    /// parameter exposes no type component of its own (a primitive keyword) or is
    /// unavailable.
    pub fn parameter_type_of(&mut self, tp: &mut TypeParser<'_>, parameter: SymbolId) -> TypeId {
        self.parameter_represented_type(tp, parameter).0
    }

    // Go: typeparser/api_stability.go apiStabilityAnalysis.parameterRepresentedType
    /// parameterRepresentedType returns the represented type component of a
    /// parameter. The parameter's materialized type is preferred and otherwise
    /// materialized lazily; the already resolved annotation node type and a bare
    /// type parameter binder are fallbacks, while a primitive keyword is known and
    /// exposes nothing.
    pub fn parameter_represented_type(
        &mut self,
        tp: &mut TypeParser<'_>,
        parameter: SymbolId,
    ) -> (TypeId, bool) {
        if parameter.is_nil() {
            return (TypeId::NIL, false);
        }
        let materialized =
            checker_integration::get_resolved_type_of_symbol_if_materialized(tp.checker, parameter);
        if materialized.is_some() {
            return (materialized, true);
        }
        let resolved = self.type_of_symbol_safely(tp, parameter);
        if resolved.is_some() {
            return (resolved, true);
        }
        let declarations: Vec<Node> = tp.checker.sym(parameter).declarations.to_vec();
        for declaration in declarations {
            if declaration.is_nil() || declaration.kind() != SyntaxKind::Parameter {
                continue;
            }
            let annotation = declaration.type_();
            if annotation.is_nil() {
                continue;
            }
            let represented =
                checker_integration::get_resolved_type_from_type_node(tp.checker, annotation);
            if represented.is_some() {
                return (represented, true);
            }
            if api_stability_primitive_type_node(annotation) {
                return (TypeId::NIL, true);
            }
            let binder = self.type_parameter_from_annotation(tp, annotation);
            if binder.is_some() {
                return (binder, true);
            }
        }
        (TypeId::NIL, false)
    }

    // Go: typeparser/api_stability.go apiStabilityAnalysis.typeParameterFromAnnotation
    /// typeParameterFromAnnotation returns the declared type parameter a type
    /// annotation names, when the annotation is exactly a bare type parameter
    /// reference. It reads the annotation symbol only; no annotation type is
    /// resolved.
    pub fn type_parameter_from_annotation(
        &mut self,
        tp: &mut TypeParser<'_>,
        node: Node,
    ) -> TypeId {
        if node.is_nil() || node.kind() != SyntaxKind::TypeReference {
            return TypeId::NIL;
        }
        if node.type_arguments().len() != 0 {
            return TypeId::NIL;
        }
        let symbol = self.symbol_at_type_name_node(tp, node.type_name());
        if symbol.is_nil()
            || !tp
                .checker
                .sym(symbol)
                .flags
                .intersects(SymbolFlags::TYPE_PARAMETER)
        {
            return TypeId::NIL;
        }
        self.note_materializing_symbol_read(
            ApiStabilityMaterializationReadKind::DeclaredType,
            symbol,
        );
        tp.checker.get_declared_type_of_symbol_exported(symbol)
    }

    // Go: typeparser/api_stability.go apiStabilityAnalysis.signatureArguments
    /// signatureArguments returns the checker-supplied type arguments of an
    /// instantiated signature, or nil when the signature has no represented
    /// substitution. The result is memoized for the analysis.
    pub fn signature_arguments(
        &mut self,
        tp: &mut TypeParser<'_>,
        signature: SignatureId,
    ) -> Vec<TypeId> {
        if signature.is_nil() {
            return Vec::new();
        }
        if let Some(cached) = self.signature_args.get(&signature) {
            return cached.clone();
        }
        let arguments = get_type_arguments_for_resolved_signature(tp.checker, signature);
        self.signature_args.insert(signature, arguments.clone());
        arguments
    }

    // Go: typeparser/api_stability.go apiStabilityAnalysis.substitute
    /// substitute resolves a type parameter through the active substitution chain.
    /// A compiler mapper maps the parameter directly; a concrete signature maps its
    /// own parameters through the concrete type arguments or its instantiated
    /// parameter mappers; a reference maps its target's parameters through the
    /// represented arguments. A parameter replaced by an unrepresented argument
    /// returns a nil mapping with ok set, which blocks the surface.
    pub fn substitute(
        &mut self,
        tp: &mut TypeParser<'_>,
        subst: &ApiStabilitySubstitution,
        t: TypeId,
    ) -> (TypeId, ApiStabilitySubstitution, bool) {
        if t.is_nil()
            || !tp
                .checker
                .ty(t)
                .flags()
                .intersects(TypeFlags::TYPE_PARAMETER)
        {
            return (TypeId::NIL, subst.clone(), false);
        }
        // A distributive conditional check is the distributed clone of its declared
        // type parameter; its constraint points back at the declaration binder. All
        // frames map the declared binder, so match through it.
        let mut t = t;
        let normalized = checker_integration::get_non_distributed_type_parameter(tp.checker, t);
        if normalized.is_some() && normalized != t {
            t = normalized;
        }
        let mut frame = subst.frame.clone();
        while let Some(current) = frame {
            let parent = ApiStabilitySubstitution {
                frame: current.parent.clone(),
            };
            if current.mapper.is_some() {
                let mapped = tp.checker.mapper_map(current.mapper, t);
                if mapped.is_some() && mapped != t {
                    return (mapped, parent, true);
                }
            }
            if current.signature.is_some() {
                let (mapped, ok) = self.signature_type_parameter(tp, current.signature, t);
                if ok && mapped != t {
                    return (mapped, parent, true);
                }
            }
            for (index, &parameter) in current.parameters.iter().enumerate() {
                if parameter != t || index >= current.arguments.len() {
                    continue;
                }
                let mapped = current.arguments[index];
                if mapped.is_nil() {
                    // The argument was not represented. The parameter is replaced,
                    // but its value is unknown, so the component is unknown.
                    return (TypeId::NIL, parent, true);
                }
                if mapped == t {
                    continue;
                }
                return (mapped, parent, true);
            }
            frame = current.parent.clone();
        }
        (TypeId::NIL, subst.clone(), false)
    }

    // Go: typeparser/api_stability.go apiStabilityAnalysis.signatureTypeParameter
    pub fn signature_type_parameter(
        &mut self,
        tp: &mut TypeParser<'_>,
        signature: SignatureId,
        t: TypeId,
    ) -> (TypeId, bool) {
        let raw = raw_signature(tp.checker, signature);
        if raw.is_nil() {
            return (TypeId::NIL, false);
        }
        let arguments = self.signature_arguments(tp, signature);
        if !arguments.is_empty() {
            let parameters = tp.checker.sig(raw).type_parameters().to_vec();
            for (index, &parameter) in parameters.iter().enumerate() {
                if parameter == t && index < arguments.len() {
                    return (arguments[index], true);
                }
            }
        }
        if raw == signature {
            return (TypeId::NIL, false);
        }
        self.signature_bound_type_parameter(tp, signature, t)
    }

    // Go: typeparser/api_stability.go apiStabilityAnalysis.signatureBoundTypeParameter
    /// signatureBoundTypeParameter resolves a bare type parameter through an
    /// instantiated signature's own parameter and return types. It only reads a
    /// concrete type when the corresponding target is itself a bare type parameter,
    /// and it maps the binder forward with the instantiated symbol's own mapper
    /// when the concrete type has not been materialized, so a recursive compound
    /// member is never evaluated. The derived map is memoized per analysis.
    pub fn signature_bound_type_parameter(
        &mut self,
        tp: &mut TypeParser<'_>,
        signature: SignatureId,
        t: TypeId,
    ) -> (TypeId, bool) {
        if signature.is_nil()
            || t.is_nil()
            || !tp
                .checker
                .ty(t)
                .flags()
                .intersects(TypeFlags::TYPE_PARAMETER)
        {
            return (TypeId::NIL, false);
        }
        let raw = raw_signature(tp.checker, signature);
        if raw.is_nil() || raw == signature {
            return (TypeId::NIL, false);
        }
        if !self.signature_parameter_cache.contains_key(&signature) {
            let bound = self.compute_signature_parameter_map(tp, raw, signature);
            self.signature_parameter_cache.insert(signature, bound);
        }
        match self
            .signature_parameter_cache
            .get(&signature)
            .and_then(|bound| bound.get(&t))
        {
            Some(&mapped) => (mapped, true),
            None => (TypeId::NIL, false),
        }
    }

    // Go: typeparser/api_stability.go apiStabilityAnalysis.computeSignatureParameterMap
    pub fn compute_signature_parameter_map(
        &mut self,
        tp: &mut TypeParser<'_>,
        raw: SignatureId,
        concrete: SignatureId,
    ) -> FxHashMap<TypeId, TypeId> {
        let mut bound = FxHashMap::default();
        let raw_parameters = tp.checker.sig(raw).parameters().to_vec();
        let concrete_parameters = tp.checker.sig(concrete).parameters().to_vec();
        if raw_parameters.len() == concrete_parameters.len() {
            for index in 0..raw_parameters.len() {
                self.bind_signature_parameter(
                    tp,
                    &mut bound,
                    raw_parameters[index],
                    concrete_parameters[index],
                );
            }
        }
        let raw_this = tp.checker.sig(raw).this_parameter();
        let concrete_this = tp.checker.sig(concrete).this_parameter();
        self.bind_signature_parameter(tp, &mut bound, raw_this, concrete_this);
        let raw_return = self.parameter_return_type(tp, raw, &ApiStabilitySubstitution::default());
        if raw_return.is_some()
            && tp
                .checker
                .ty(raw_return)
                .flags()
                .intersects(TypeFlags::TYPE_PARAMETER)
        {
            let concrete_return =
                checker_integration::get_resolved_return_type_of_signature_if_materialized(
                    tp.checker, concrete,
                );
            if concrete_return.is_some() && concrete_return != raw_return {
                bound.insert(raw_return, concrete_return);
            }
        }
        bound
    }

    /// PORT: the Go `bind` closure of `computeSignatureParameterMap`.
    fn bind_signature_parameter(
        &mut self,
        tp: &mut TypeParser<'_>,
        bound: &mut FxHashMap<TypeId, TypeId>,
        raw_parameter: SymbolId,
        concrete_parameter: SymbolId,
    ) {
        if raw_parameter.is_nil() || concrete_parameter.is_nil() {
            return;
        }
        let raw_type = self.parameter_type_of(tp, raw_parameter);
        if raw_type.is_nil()
            || !tp
                .checker
                .ty(raw_type)
                .flags()
                .intersects(TypeFlags::TYPE_PARAMETER)
        {
            return;
        }
        let mut concrete_type = checker_integration::get_resolved_type_of_symbol_if_materialized(
            tp.checker,
            concrete_parameter,
        );
        if concrete_type.is_nil() {
            // Prefer the instantiated symbol's own mapper: it maps the binder
            // forward without instantiating a compound annotation.
            let mapper =
                checker_integration::get_instantiated_symbol_mapper(tp.checker, concrete_parameter);
            if mapper.is_some() {
                concrete_type = tp.checker.mapper_map(mapper, raw_type);
            }
        }
        if concrete_type.is_some() && concrete_type != raw_type {
            bound.insert(raw_type, concrete_type);
        }
    }

    // Go: typeparser/api_stability.go apiStabilityAnalysis.parameterReturnType
    /// parameterReturnType returns the represented return type of a signature: the
    /// signature's materialized return type, the already resolved annotation node
    /// type, or the declared binder when the return annotation is exactly a type
    /// parameter reference.
    pub fn parameter_return_type(
        &mut self,
        tp: &mut TypeParser<'_>,
        signature: SignatureId,
        subst: &ApiStabilitySubstitution,
    ) -> TypeId {
        self.return_represented_type(tp, signature, subst).0
    }

    // Go: typeparser/api_stability.go apiStabilityAnalysis.collectDeclarationProvenance
    /// collectDeclarationProvenance records the alias symbols written in a public
    /// declaration's own type annotations. The checker erases an alias such as
    /// `type ExperimentalText = string`, so symbol provenance is the only way to
    /// name it. The traversal is driven by the represented checker type and never
    /// descends into conditional branches, so a symbol that survives only in a
    /// discarded branch is not reported.
    // PORT: Go's `KindIndexSignature` case of the second switch is dead (an
    // index signature has function-like data, so the first branch returns);
    // the port leaves it out.
    pub fn collect_declaration_provenance(
        &mut self,
        tp: &mut TypeParser<'_>,
        surface: &mut ApiStabilitySurface,
        declaration: Node,
        represented: TypeId,
        subst: &ApiStabilitySubstitution,
    ) {
        if declaration.is_nil()
            || represented.is_nil()
            || api_stability_property_declaration_is_internal(declaration)
        {
            return;
        }
        if api_stability_has_function_like_data(declaration) {
            match declaration.kind() {
                SyntaxKind::GetAccessor => {
                    self.collect_type_node_provenance(
                        tp,
                        surface,
                        declaration.type_(),
                        represented,
                        subst,
                    );
                }
                SyntaxKind::SetAccessor => {
                    for parameter in declaration.parameters() {
                        if parameter.is_nil() || parameter.kind() != SyntaxKind::Parameter {
                            continue;
                        }
                        self.collect_type_node_provenance(
                            tp,
                            surface,
                            parameter.type_(),
                            represented,
                            subst,
                        );
                    }
                }
                _ => {}
            }
            return;
        }
        match declaration.kind() {
            SyntaxKind::VariableDeclaration
            | SyntaxKind::PropertyDeclaration
            | SyntaxKind::PropertySignature
            | SyntaxKind::TypeAliasDeclaration
            | SyntaxKind::JsTypeAliasDeclaration => {
                self.collect_type_node_provenance(
                    tp,
                    surface,
                    declaration.type_(),
                    represented,
                    subst,
                );
            }
            _ => {}
        }
    }

    // Go: typeparser/api_stability.go apiStabilityAnalysis.collectSignatureProvenance
    /// collectSignatureProvenance records erased alias symbols in a signature's
    /// public annotations. A type parameter replaced by a represented argument is
    /// skipped together with its constraint and default, matching the represented
    /// instantiated surface.
    pub fn collect_signature_provenance(
        &mut self,
        tp: &mut TypeParser<'_>,
        surface: &mut ApiStabilitySurface,
        raw: SignatureId,
        signature_subst: &ApiStabilitySubstitution,
    ) {
        let declaration = tp.checker.sig(raw).declaration();
        if declaration.is_nil() || !api_stability_has_function_like_data(declaration) {
            return;
        }
        let raw_type_parameters = tp.checker.sig(raw).type_parameters().to_vec();
        for (index, type_parameter) in declaration.type_parameters().iter().enumerate() {
            if type_parameter.is_nil()
                || type_parameter.kind() != SyntaxKind::TypeParameter
                || index >= raw_type_parameters.len()
            {
                continue;
            }
            let (_, _, replaced) = self.substitute(tp, signature_subst, raw_type_parameters[index]);
            if replaced {
                continue;
            }
            let constraint =
                checker_integration::get_resolved_constraint_of_type_parameter_if_materialized(
                    tp.checker,
                    raw_type_parameters[index],
                );
            self.collect_type_node_provenance(
                tp,
                surface,
                type_parameter.constraint(),
                constraint,
                signature_subst,
            );
            let default_type =
                checker_integration::get_resolved_default_from_type_parameter_if_materialized(
                    tp.checker,
                    raw_type_parameters[index],
                );
            self.collect_type_node_provenance(
                tp,
                surface,
                type_parameter.default_type(),
                default_type,
                signature_subst,
            );
        }
        let raw_parameters = tp.checker.sig(raw).parameters().to_vec();
        let mut parameter_index = 0;
        for parameter in declaration.parameters() {
            if parameter.is_nil() || parameter.kind() != SyntaxKind::Parameter {
                continue;
            }
            let mut represented = TypeId::NIL;
            if parameter_index < raw_parameters.len() {
                represented = self.parameter_type_of(tp, raw_parameters[parameter_index]);
            }
            self.collect_type_node_provenance(
                tp,
                surface,
                parameter.type_(),
                represented,
                signature_subst,
            );
            parameter_index += 1;
        }
        let return_type = self.parameter_return_type(tp, raw, signature_subst);
        self.collect_type_node_provenance(
            tp,
            surface,
            declaration.type_(),
            return_type,
            signature_subst,
        );
    }

    // Go: typeparser/api_stability.go apiStabilityAnalysis.collectTypeNodeProvenance
    /// collectTypeNodeProvenance records alias symbols named by type annotation
    /// nodes whose represented checker type is directly reachable. It is
    /// deliberately structural and correlated with the represented types: unions,
    /// intersections, tuples, arrays, reference arguments and literal members are
    /// only descended when the corresponding represented component exists, and
    /// conditional branches are never descended into. A union or intersection
    /// constituent is correlated by concrete component identity, so a retained
    /// constituent whose alias the compiler collapsed or reordered is still named.
    /// An annotation whose represented type is unavailable records nothing, which
    /// keeps the walk from becoming a syntax dependency parser.
    pub fn collect_type_node_provenance(
        &mut self,
        tp: &mut TypeParser<'_>,
        surface: &mut ApiStabilitySurface,
        node: Node,
        represented: TypeId,
        subst: &ApiStabilitySubstitution,
    ) {
        if node.is_nil() || represented.is_nil() {
            return;
        }
        match node.kind() {
            SyntaxKind::ParenthesizedType | SyntaxKind::TypeOperator => {
                self.collect_type_node_provenance(tp, surface, node.type_(), represented, subst);
            }
            SyntaxKind::TypeReference => {
                let symbol = self.symbol_at_type_name_node(tp, node.type_name());
                self.record_symbol(tp, surface, symbol);
                if api_stability_represented_surface_accepts_arguments(tp.checker, represented) {
                    self.collect_type_argument_provenance(
                        tp,
                        surface,
                        node.type_argument_list(),
                        represented,
                        subst,
                    );
                }
            }
            SyntaxKind::ExpressionWithTypeArguments => {
                let symbol = self.symbol_at_type_name_node(tp, node.expression());
                self.record_symbol(tp, surface, symbol);
                if api_stability_represented_surface_accepts_arguments(tp.checker, represented) {
                    self.collect_type_argument_provenance(
                        tp,
                        surface,
                        node.type_argument_list(),
                        represented,
                        subst,
                    );
                }
            }
            SyntaxKind::TypeQuery => {
                let symbol = self.symbol_at_type_name_node(tp, node.expr_name());
                self.record_symbol(tp, surface, symbol);
            }
            SyntaxKind::ImportType => {
                let qualifier = node.qualifier();
                if qualifier.is_some() {
                    let symbol = self.symbol_at_type_name_node(tp, qualifier);
                    self.record_symbol(tp, surface, symbol);
                }
                if api_stability_represented_surface_accepts_arguments(tp.checker, represented) {
                    self.collect_type_argument_provenance(
                        tp,
                        surface,
                        node.type_argument_list(),
                        represented,
                        subst,
                    );
                }
            }
            SyntaxKind::UnionType | SyntaxKind::IntersectionType => {
                let list = node.types();
                if list.is_some() {
                    self.collect_aggregate_type_node_provenance(
                        tp,
                        surface,
                        list,
                        represented,
                        subst,
                    );
                }
            }
            SyntaxKind::ArrayType => {
                let element_type = self.represented_element_type(tp, represented);
                self.collect_type_node_provenance(
                    tp,
                    surface,
                    node.element_type(),
                    element_type,
                    subst,
                );
            }
            SyntaxKind::TupleType => {
                let elements = node.elements();
                if elements.len() == 0 {
                    return;
                }
                let represented_elements = self.represented_tuple_arguments(tp, represented);
                for (index, element) in elements.iter().enumerate() {
                    let element_type = represented_elements
                        .get(index)
                        .copied()
                        .unwrap_or(TypeId::NIL);
                    self.collect_type_node_provenance(tp, surface, element, element_type, subst);
                }
            }
            SyntaxKind::IndexedAccessType => {
                let (object_type, index_type) =
                    api_stability_represented_indexed_access(tp.checker, represented);
                self.collect_type_node_provenance(
                    tp,
                    surface,
                    node.object_type(),
                    object_type,
                    subst,
                );
                self.collect_type_node_provenance(
                    tp,
                    surface,
                    node.index_type(),
                    index_type,
                    subst,
                );
            }
            SyntaxKind::ConditionalType => {
                let (check_type, extends_type) =
                    api_stability_represented_conditional_parts(tp.checker, represented);
                self.collect_type_node_provenance(
                    tp,
                    surface,
                    node.check_type(),
                    check_type,
                    subst,
                );
                self.collect_type_node_provenance(
                    tp,
                    surface,
                    node.extends_type(),
                    extends_type,
                    subst,
                );
            }
            SyntaxKind::FunctionType | SyntaxKind::ConstructorType => {
                self.collect_function_type_provenance(tp, surface, node, represented, subst);
            }
            SyntaxKind::TypeLiteral => {
                self.collect_type_literal_provenance(tp, surface, node, represented, subst);
            }
            SyntaxKind::MappedType => {
                let mut constraint = TypeId::NIL;
                let mut name_type = TypeId::NIL;
                let mut template_type = TypeId::NIL;
                if tp
                    .checker
                    .ty(represented)
                    .object_flags()
                    .intersects(ObjectFlags::MAPPED)
                {
                    constraint = checker_integration::get_mapped_type_constraint_type(
                        tp.checker,
                        represented,
                    );
                    name_type =
                        checker_integration::get_mapped_type_name_type(tp.checker, represented);
                    template_type =
                        checker_integration::get_mapped_type_template_type(tp.checker, represented);
                }
                let type_parameter = node.type_parameter();
                if type_parameter.is_some() && type_parameter.kind() == SyntaxKind::TypeParameter {
                    // The mapped parameter's constraint is the mapped constraint,
                    // already paired above; this pairing only names erased aliases.
                    self.collect_type_node_provenance(
                        tp,
                        surface,
                        type_parameter.constraint(),
                        constraint,
                        subst,
                    );
                }
                self.collect_type_node_provenance(tp, surface, node.name_type(), name_type, subst);
                self.collect_type_node_provenance(tp, surface, node.type_(), template_type, subst);
            }
            _ => {}
        }
    }
}

// Go: typeparser/api_stability.go apiStabilityRepresentedMembers
pub fn api_stability_represented_members(c: &Checker, t: TypeId) -> Vec<TypeId> {
    if t.is_nil() {
        return Vec::new();
    }
    if c.ty(t).flags().intersects(TypeFlags::UNION_OR_INTERSECTION) {
        return c.ty(t).types().to_vec();
    }
    // Compiler normalization collapsed the aggregate to a single constituent;
    // that constituent is the only retained component.
    vec![t]
}

// Go: typeparser/api_stability.go apiStabilityRepresentedContainsType
/// apiStabilityRepresentedContainsType reports whether the normalized aggregate
/// retained a component with the exact compiler identity of t.
pub fn api_stability_represented_contains_type(members: &[TypeId], t: TypeId) -> bool {
    if t.is_nil() {
        return false;
    }
    members.contains(&t)
}

// Go: typeparser/api_stability.go apiStabilityRepresentedContainsAggregate
/// apiStabilityRepresentedContainsAggregate reports whether every constituent of
/// a written union or intersection was retained in the normalized aggregate.
/// Compiler normalization flattens nested aggregates, so a written constituent
/// such as `A = string | number` is retained as its individual members rather
/// than as one aggregate component.
pub fn api_stability_represented_contains_aggregate(
    c: &Checker,
    members: &[TypeId],
    t: TypeId,
) -> bool {
    let constituents = api_stability_represented_members(c, t);
    if constituents.is_empty() {
        return false;
    }
    constituents
        .iter()
        .all(|&constituent| api_stability_represented_contains_type(members, constituent))
}

impl ApiStabilityAnalysis {
    // Go: typeparser/api_stability.go apiStabilityAnalysis.collectAggregateTypeNodeProvenance
    /// collectAggregateTypeNodeProvenance correlates the written constituents of a
    /// union or intersection annotation with the represented aggregate by concrete
    /// component identity. Compiler normalization can remove, collapse, reorder or
    /// combine constituents, so pairing by position would attach the wrong
    /// represented component. A constituent whose resolved type is retained in the
    /// normalized aggregate is descended so its erased alias stays named; a
    /// constituent eliminated by normalization records nothing. The node type is
    /// read through the ordinary lazy accessor, correlated with a retained
    /// component, and only then walked; the node itself never introduces a
    /// dependency the represented type does not contain.
    pub fn collect_aggregate_type_node_provenance(
        &mut self,
        tp: &mut TypeParser<'_>,
        surface: &mut ApiStabilitySurface,
        list: NodeList,
        represented: TypeId,
        subst: &ApiStabilitySubstitution,
    ) {
        let members = api_stability_represented_members(tp.checker, represented);
        for member in list.nodes() {
            if member.is_nil() {
                continue;
            }
            let member_type = self.represented_type_from_node(tp, member, subst);
            if member_type.is_nil()
                || (!api_stability_represented_contains_type(&members, member_type)
                    && !api_stability_represented_contains_aggregate(
                        tp.checker,
                        &members,
                        member_type,
                    ))
            {
                continue;
            }
            self.collect_type_node_provenance(tp, surface, member, member_type, subst);
        }
    }
}

// Go: typeparser/api_stability.go apiStabilityRepresentedIndexedAccess
pub fn api_stability_represented_indexed_access(c: &Checker, t: TypeId) -> (TypeId, TypeId) {
    if t.is_some() && c.ty(t).flags().intersects(TypeFlags::INDEXED_ACCESS) {
        let indexed = c.ty(t).as_indexed_access_type();
        return (indexed.object_type, indexed.index_type);
    }
    (TypeId::NIL, TypeId::NIL)
}

// Go: typeparser/api_stability.go apiStabilityRepresentedConditionalParts
pub fn api_stability_represented_conditional_parts(c: &Checker, t: TypeId) -> (TypeId, TypeId) {
    if t.is_some() && c.ty(t).flags().intersects(TypeFlags::CONDITIONAL) {
        let conditional = c.ty(t).as_conditional_type();
        return (conditional.check_type, conditional.extends_type);
    }
    (TypeId::NIL, TypeId::NIL)
}

// Go: typeparser/api_stability.go apiStabilityRepresentedSurfaceAcceptsArguments
/// apiStabilityRepresentedSurfaceAcceptsArguments reports whether the written
/// type arguments of a named reference are part of the represented surface. An
/// application the checker evaluated away (an eliminated conditional resolving
/// to `never` or to a primitive) exposes none of its arguments, so recording
/// them would report a symbol the surface cannot expose.
pub fn api_stability_represented_surface_accepts_arguments(
    c: &Checker,
    represented: TypeId,
) -> bool {
    if represented.is_nil() {
        return true;
    }
    !c.ty(represented).flags().intersects(
        TypeFlags::NEVER
            | TypeFlags::STRING
            | TypeFlags::NUMBER
            | TypeFlags::BOOLEAN
            | TypeFlags::BIG_INT
            | TypeFlags::ES_SYMBOL
            | TypeFlags::VOID
            | TypeFlags::UNDEFINED
            | TypeFlags::NULL,
    )
}

impl ApiStabilityAnalysis {
    // Go: typeparser/api_stability.go apiStabilityAnalysis.collectTypeArgumentProvenance
    pub fn collect_type_argument_provenance(
        &mut self,
        tp: &mut TypeParser<'_>,
        surface: &mut ApiStabilitySurface,
        arguments: NodeList,
        represented: TypeId,
        subst: &ApiStabilitySubstitution,
    ) {
        if arguments.is_nil() || arguments.nodes().len() == 0 {
            return;
        }
        let represented_arguments = self.represented_type_arguments(tp, represented);
        for (index, argument) in arguments.nodes().iter().enumerate() {
            let argument_type = represented_arguments
                .get(index)
                .copied()
                .unwrap_or(TypeId::NIL);
            self.collect_type_node_provenance(tp, surface, argument, argument_type, subst);
        }
    }

    // Go: typeparser/api_stability.go apiStabilityAnalysis.collectFunctionTypeProvenance
    /// collectFunctionTypeProvenance pairs a function-like annotation's parameter
    /// and return nodes with the represented signature when one is available. When
    /// it is not, the annotation records nothing: nodes are never resolved here.
    pub fn collect_function_type_provenance(
        &mut self,
        tp: &mut TypeParser<'_>,
        surface: &mut ApiStabilitySurface,
        node: Node,
        represented: TypeId,
        subst: &ApiStabilitySubstitution,
    ) {
        let kind = if node.kind() == SyntaxKind::ConstructorType {
            SignatureKind::CONSTRUCT
        } else {
            SignatureKind::CALL
        };
        let mut raw = SignatureId::NIL;
        if represented.is_some() {
            let (signatures, ok) =
                checker_integration::get_resolved_signatures_of_type_if_materialized(
                    tp.checker,
                    represented,
                    kind,
                );
            if ok && !signatures.is_empty() {
                raw = raw_signature(tp.checker, signatures[0]);
            }
        }
        if raw.is_nil() {
            return;
        }
        if api_stability_has_function_like_data(node) && node.type_parameter_list().is_some() {
            let raw_type_parameters = tp.checker.sig(raw).type_parameters().to_vec();
            for (index, type_parameter) in node.type_parameters().iter().enumerate() {
                if type_parameter.is_nil()
                    || type_parameter.kind() != SyntaxKind::TypeParameter
                    || index >= raw_type_parameters.len()
                {
                    continue;
                }
                let constraint =
                    checker_integration::get_resolved_constraint_of_type_parameter_if_materialized(
                        tp.checker,
                        raw_type_parameters[index],
                    );
                self.collect_type_node_provenance(
                    tp,
                    surface,
                    type_parameter.constraint(),
                    constraint,
                    subst,
                );
                let default_type =
                    checker_integration::get_resolved_default_from_type_parameter_if_materialized(
                        tp.checker,
                        raw_type_parameters[index],
                    );
                self.collect_type_node_provenance(
                    tp,
                    surface,
                    type_parameter.default_type(),
                    default_type,
                    subst,
                );
            }
        }
        let raw_parameters = tp.checker.sig(raw).parameters().to_vec();
        for (index, parameter) in node.parameters().iter().enumerate() {
            if parameter.is_nil() || parameter.kind() != SyntaxKind::Parameter {
                continue;
            }
            let mut parameter_type = TypeId::NIL;
            if index < raw_parameters.len() {
                parameter_type = self.parameter_type_of(tp, raw_parameters[index]);
            }
            self.collect_type_node_provenance(
                tp,
                surface,
                parameter.type_(),
                parameter_type,
                subst,
            );
        }
        let return_type = self.parameter_return_type(tp, raw, subst);
        self.collect_type_node_provenance(tp, surface, node.type_(), return_type, subst);
    }
}

// Go: typeparser/api_stability.go apiStabilityMemberSymbolForDeclaration
/// apiStabilityMemberSymbolForDeclaration returns the member symbol a
/// declaration contributes to an already resolved member table. A simply named
/// declaration is looked up by its member-table key. A computed declaration is
/// never asked for its name text: the table is scanned for the symbol whose
/// declarations include the member, which keeps the pairing with the concrete
/// instantiated member (and its signature) the table belongs to. The scan runs
/// only for a computed declaration, so simply named members keep the direct
/// lookup.
pub fn api_stability_member_symbol_for_declaration(
    c: &Checker,
    members: SymbolTable,
    member: Node,
) -> SymbolId {
    if member.is_nil() || members.is_nil() {
        return SymbolId::NIL;
    }
    let name = get_name_of_declaration(member);
    if name.is_some()
        && let Some(simple) = api_stability_simple_name_text(name)
    {
        return c.symbols.get(members, &simple);
    }
    for key in sorted_symbol_table_keys(c, members) {
        let candidate = c.symbols.get(members, &key);
        if candidate.is_nil() {
            continue;
        }
        if c.sym(candidate).declarations.contains(&member) {
            return candidate;
        }
    }
    SymbolId::NIL
}

impl ApiStabilityAnalysis {
    // Go: typeparser/api_stability.go apiStabilityAnalysis.collectTypeLiteralProvenance
    /// collectTypeLiteralProvenance pairs a type literal's member annotations with
    /// the represented member types when the literal's member surface has been
    /// materialized. An unmaterialized member records nothing instead of being
    /// resolved. A computed member is paired by declaration identity, so its
    /// concrete instantiated member and signature are preserved without evaluating
    /// the computed name.
    pub fn collect_type_literal_provenance(
        &mut self,
        tp: &mut TypeParser<'_>,
        surface: &mut ApiStabilitySurface,
        node: Node,
        represented: TypeId,
        subst: &ApiStabilitySubstitution,
    ) {
        let (mut members, mut ok) =
            checker_integration::get_resolved_members_of_type_if_materialized(
                tp.checker,
                represented,
            );
        if !ok && represented.is_some() && tp.checker.ty(represented).symbol().is_some() {
            let symbol = tp.checker.ty(represented).symbol();
            members = self.materialized_members_of_symbol(tp.checker, symbol);
            ok = members.is_some();
        }
        if !ok {
            return;
        }
        for member in node.members() {
            if member.is_nil() || api_stability_property_declaration_is_internal(member) {
                continue;
            }
            match member.kind() {
                SyntaxKind::PropertySignature => {
                    let mut property_type = TypeId::NIL;
                    let property =
                        api_stability_member_symbol_for_declaration(tp.checker, members, member);
                    if property.is_some() {
                        // The concrete instantiated member is read through the ordinary
                        // guarded accessor: a materialized component is used directly,
                        // an unresolved one is materialized only when the guard
                        // establishes that the read is bounded. This is the same lazy
                        // path a simply named member takes through memberSurface.
                        property_type = self.type_of_symbol_safely(tp, property);
                    }
                    self.collect_type_node_provenance(
                        tp,
                        surface,
                        member.type_(),
                        property_type,
                        subst,
                    );
                }
                SyntaxKind::MethodSignature => {
                    let property =
                        api_stability_member_symbol_for_declaration(tp.checker, members, member);
                    if property.is_nil() {
                        continue;
                    }
                    let property_type = self.type_of_symbol_safely(tp, property);
                    if property_type.is_some() {
                        let (signatures, ok) =
                            checker_integration::get_resolved_signatures_of_type_if_materialized(
                                tp.checker,
                                property_type,
                                SignatureKind::CALL,
                            );
                        if ok && !signatures.is_empty() {
                            let raw = raw_signature(tp.checker, signatures[0]);
                            self.collect_function_signature_provenance(
                                tp, surface, member, raw, subst,
                            );
                            continue;
                        }
                    }
                    // A method's function type exists but its structured signatures have
                    // not been materialized. The raw declaration signature is created
                    // through the same guarded signature accessor an ordinary method
                    // member uses.
                    if !self.signature_member_resolution_is_safe(tp, property, subst) {
                        continue;
                    }
                    let signatures = self.materialized_signatures_of_symbol(tp.checker, property);
                    if !signatures.is_empty() {
                        let raw = raw_signature(tp.checker, signatures[0]);
                        self.collect_function_signature_provenance(tp, surface, member, raw, subst);
                    }
                }
                SyntaxKind::CallSignature | SyntaxKind::ConstructSignature => {
                    let kind = if member.kind() == SyntaxKind::ConstructSignature {
                        SignatureKind::CONSTRUCT
                    } else {
                        SignatureKind::CALL
                    };
                    let (signatures, ok) =
                        checker_integration::get_resolved_signatures_of_type_if_materialized(
                            tp.checker,
                            represented,
                            kind,
                        );
                    if ok && !signatures.is_empty() {
                        let raw = raw_signature(tp.checker, signatures[0]);
                        self.collect_function_signature_provenance(tp, surface, member, raw, subst);
                    }
                }
                SyntaxKind::IndexSignature => {
                    let (infos, ok) =
                        checker_integration::get_resolved_index_infos_of_type_if_materialized(
                            tp.checker,
                            represented,
                        );
                    if ok {
                        for info in infos {
                            if info.is_nil() {
                                continue;
                            }
                            let (declaration, value_type) = {
                                let data = tp.checker.index_info(info);
                                (data.declaration, data.value_type)
                            };
                            if declaration == member {
                                self.collect_type_node_provenance(
                                    tp,
                                    surface,
                                    member.type_(),
                                    value_type,
                                    subst,
                                );
                                break;
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }

    // Go: typeparser/api_stability.go apiStabilityAnalysis.collectFunctionSignatureProvenance
    /// collectFunctionSignatureProvenance pairs a function-like declaration's
    /// annotation nodes with the represented signature it declares.
    pub fn collect_function_signature_provenance(
        &mut self,
        tp: &mut TypeParser<'_>,
        surface: &mut ApiStabilitySurface,
        declaration: Node,
        raw: SignatureId,
        subst: &ApiStabilitySubstitution,
    ) {
        if declaration.is_nil() || raw.is_nil() {
            return;
        }
        if !api_stability_has_function_like_data(declaration) {
            return;
        }
        let raw_parameters = tp.checker.sig(raw).parameters().to_vec();
        let mut parameter_index = 0;
        for parameter in declaration.parameters() {
            if parameter.is_nil() || parameter.kind() != SyntaxKind::Parameter {
                continue;
            }
            let mut parameter_type = TypeId::NIL;
            if parameter_index < raw_parameters.len() {
                parameter_type = self.parameter_type_of(tp, raw_parameters[parameter_index]);
            }
            self.collect_type_node_provenance(
                tp,
                surface,
                parameter.type_(),
                parameter_type,
                subst,
            );
            parameter_index += 1;
        }
        let return_type = self.parameter_return_type(tp, raw, subst);
        self.collect_type_node_provenance(tp, surface, declaration.type_(), return_type, subst);
    }

    // Go: typeparser/api_stability.go apiStabilityAnalysis.representedTypeArguments
    /// representedTypeArguments returns the already recorded type arguments of a
    /// reference. A deferred reference whose arguments the checker has not
    /// materialized returns nil; nothing is resolved here.
    pub fn represented_type_arguments(
        &mut self,
        tp: &mut TypeParser<'_>,
        t: TypeId,
    ) -> Vec<TypeId> {
        checker_integration::get_resolved_type_arguments(tp.checker, t)
    }

    // Go: typeparser/api_stability.go apiStabilityAnalysis.representedElementType
    /// representedElementType returns the represented element type of an array or
    /// tuple reference.
    pub fn represented_element_type(&mut self, tp: &mut TypeParser<'_>, t: TypeId) -> TypeId {
        self.represented_type_arguments(tp, t)
            .first()
            .copied()
            .unwrap_or(TypeId::NIL)
    }

    // Go: typeparser/api_stability.go apiStabilityAnalysis.representedTupleArguments
    /// representedTupleArguments returns the represented element types of a tuple
    /// reference without instantiating anything.
    pub fn represented_tuple_arguments(
        &mut self,
        tp: &mut TypeParser<'_>,
        t: TypeId,
    ) -> Vec<TypeId> {
        if t.is_nil()
            || !tp.checker.ty(t).flags().intersects(TypeFlags::OBJECT)
            || !tp
                .checker
                .ty(t)
                .object_flags()
                .intersects(ObjectFlags::REFERENCE)
        {
            return Vec::new();
        }
        self.represented_type_arguments(tp, t)
    }

    // Go: typeparser/api_stability.go apiStabilityAnalysis.recordSymbol
    /// recordSymbol records a symbol whose declared stability is above stable. A
    /// stable symbol can never exceed any owner ceiling, so it is omitted to keep
    /// each surface small.
    pub fn record_symbol(
        &mut self,
        tp: &mut TypeParser<'_>,
        surface: &mut ApiStabilitySurface,
        symbol: SymbolId,
    ) {
        if symbol.is_nil() {
            return;
        }
        let declared = tp.declared_api_stability_of_symbol(symbol);
        if declared.level <= ApiStabilityLevel::Stable {
            return;
        }
        self.add_finding(
            surface,
            ApiStabilityFindingKey {
                symbol,
                ..Default::default()
            },
            declared,
        );
    }

    // Go: typeparser/api_stability.go apiStabilityAnalysis.recordSignatureStability
    /// recordSignatureStability records a public signature whose declaration carries
    /// its own `@stability` tag. The raw (uninstantiated) signature keeps distinct
    /// overload tags distinct.
    pub fn record_signature_stability(
        &mut self,
        tp: &mut TypeParser<'_>,
        surface: &mut ApiStabilitySurface,
        signature: SignatureId,
    ) {
        if signature.is_nil() {
            return;
        }
        let declared = tp.declared_api_stability_of_signature(signature);
        if declared.level <= ApiStabilityLevel::Stable {
            return;
        }
        let raw = raw_signature(tp.checker, signature);
        self.add_finding(
            surface,
            ApiStabilityFindingKey {
                signature: raw,
                ..Default::default()
            },
            declared,
        );
    }

    // Go: typeparser/api_stability.go apiStabilityAnalysis.typeChildBoundarySymbol
    /// typeChildBoundarySymbol returns the identity symbol whose explicit declared
    /// tag bounds a type reached as a child component, or nil when the type carries
    /// no explicit tag. An alias application's alias symbol takes precedence over
    /// the represented target, so an explicit tag on the alias is the component's
    /// own tag; otherwise the type's own symbol, or a reference's target symbol,
    /// carries the tag. An absent tag (Declaration nil) is not a boundary.
    pub fn type_child_boundary_symbol(&mut self, tp: &mut TypeParser<'_>, t: TypeId) -> SymbolId {
        if t.is_nil() {
            return SymbolId::NIL;
        }
        if let Some(alias) = tp.checker.ty(t).alias() {
            let alias_symbol = alias.symbol();
            if alias_symbol.is_some()
                && tp
                    .declared_api_stability_of_symbol(alias_symbol)
                    .declaration
                    .is_some()
            {
                return alias_symbol;
            }
        }
        let mut symbol = tp.checker.ty(t).symbol();
        if symbol.is_nil()
            && tp
                .checker
                .ty(t)
                .object_flags()
                .intersects(ObjectFlags::REFERENCE)
            && tp.checker.ty(t).target().is_some()
        {
            symbol = tp.checker.ty(tp.checker.ty(t).target()).symbol();
        }
        if symbol.is_nil() {
            return SymbolId::NIL;
        }
        if tp
            .declared_api_stability_of_symbol(symbol)
            .declaration
            .is_some()
        {
            return symbol;
        }
        SymbolId::NIL
    }

    // Go: typeparser/api_stability.go apiStabilityAnalysis.heritageChildBoundarySymbol
    /// heritageChildBoundarySymbol returns the identity symbol whose explicit
    /// declared tag bounds a heritage base reached as a child component, or nil.
    /// As with a type edge, an alias application's alias symbol takes precedence
    /// over the represented target.
    pub fn heritage_child_boundary_symbol(
        &mut self,
        tp: &mut TypeParser<'_>,
        base: TypeId,
        target: TypeId,
    ) -> SymbolId {
        if base.is_some()
            && let Some(alias) = tp.checker.ty(base).alias()
        {
            let alias_symbol = alias.symbol();
            if alias_symbol.is_some()
                && tp
                    .declared_api_stability_of_symbol(alias_symbol)
                    .declaration
                    .is_some()
            {
                return alias_symbol;
            }
        }
        let mut symbol = SymbolId::NIL;
        if target.is_some() {
            symbol = tp.checker.ty(target).symbol();
        }
        if symbol.is_nil() && base.is_some() {
            symbol = tp.checker.ty(base).symbol();
        }
        if symbol.is_nil() {
            return SymbolId::NIL;
        }
        if tp
            .declared_api_stability_of_symbol(symbol)
            .declaration
            .is_some()
        {
            return symbol;
        }
        SymbolId::NIL
    }

    // Go: typeparser/api_stability.go apiStabilityAnalysis.collectChildTypeArguments
    /// collectChildTypeArguments merges the independently represented type arguments
    /// a tagged child type still exposes. Alias application arguments and reference
    /// arguments are operands of the application, not internals of the tagged
    /// component; each argument is itself traversed through the child boundary rules.
    pub fn collect_child_type_arguments(
        &mut self,
        tp: &mut TypeParser<'_>,
        surface: &mut ApiStabilitySurface,
        t: TypeId,
        subst: &ApiStabilitySubstitution,
    ) {
        if t.is_nil() {
            return;
        }
        if let Some(alias) = tp.checker.ty(t).alias()
            && !self.alias_arguments_are_self(tp, Some(&alias))
            && api_stability_represented_surface_accepts_arguments(tp.checker, t)
        {
            for &argument in alias.type_arguments() {
                if argument.is_nil() {
                    continue;
                }
                let child = self.type_surface(tp, argument, ApiStabilityInspection::Shallow, subst);
                surface.merge(child);
            }
        }
        if tp
            .checker
            .ty(t)
            .object_flags()
            .intersects(ObjectFlags::REFERENCE)
            && tp.checker.ty(t).target().is_some()
            && tp.checker.ty(t).target() != t
        {
            let target = tp.checker.ty(t).target();
            for argument in self.reference_arguments(tp, t, target) {
                if argument.is_nil() {
                    continue;
                }
                let child = self.type_surface(tp, argument, ApiStabilityInspection::Shallow, subst);
                surface.merge(child);
            }
        }
    }

    // Go: typeparser/api_stability.go apiStabilityAnalysis.collectTaggedHeritageArguments
    /// collectTaggedHeritageArguments records every tagged class or interface
    /// ancestor of a declaration as a child boundary and exposes its independently
    /// represented type arguments. It is used where the compiler has already
    /// flattened inherited members into a resolved member table and no per-base
    /// declaration walk happens (an expanded reference). Untagged ancestors are
    /// followed so a tagged ancestor deeper in the chain is still attributed with
    /// the substitution context that reached it; an ancestor cycle is visited once.
    pub fn collect_tagged_heritage_arguments(
        &mut self,
        tp: &mut TypeParser<'_>,
        surface: &mut ApiStabilitySurface,
        declaration: TypeId,
        subst: &ApiStabilitySubstitution,
        visited: &mut FxHashMap<TypeId, bool>,
    ) {
        if declaration.is_nil() || visited.get(&declaration).copied().unwrap_or(false) {
            return;
        }
        visited.insert(declaration, true);
        let (bases, resolved) =
            checker_integration::get_resolved_base_types_of_type_if_materialized(
                tp.checker,
                declaration,
            );
        if !resolved {
            return;
        }
        for base in bases {
            if base.is_nil() {
                continue;
            }
            let mut target = base;
            if tp
                .checker
                .ty(base)
                .object_flags()
                .intersects(ObjectFlags::REFERENCE)
                && tp.checker.ty(base).target().is_some()
            {
                target = tp.checker.ty(base).target();
            }
            if target.is_nil()
                || !tp
                    .checker
                    .ty(target)
                    .object_flags()
                    .intersects(ObjectFlags::CLASS_OR_INTERFACE)
            {
                continue;
            }
            let target_symbol = tp.checker.ty(target).symbol();
            if self.optional_tagged_base(tp, target_symbol) {
                continue;
            }
            let boundary = self.heritage_child_boundary_symbol(tp, base, target);
            if boundary.is_some() {
                self.record_symbol(tp, surface, boundary);
                self.collect_child_type_arguments(tp, surface, base, subst);
                continue;
            }
            let parameters = self.reference_type_parameters(tp, target);
            let arguments = self.reference_arguments(tp, base, target);
            let base_subst =
                self.extend_parameters_substitution(subst, base, &parameters, &arguments);
            self.collect_tagged_heritage_arguments(tp, surface, target, &base_subst, visited);
        }
    }

    // Go: typeparser/api_stability.go apiStabilityAnalysis.declarationDeclaringSymbol
    /// declarationDeclaringSymbol returns the class or interface symbol whose
    /// declaration directly contains the given declaration, or nil for a
    /// declaration of a type literal or any other anonymous surface. Only a direct
    /// declaration names an owner: a declaration nested inside a type literal stays
    /// anonymous even when that literal sits inside a class or interface.
    pub fn declaration_declaring_symbol(
        &mut self,
        tp: &mut TypeParser<'_>,
        declaration: Node,
    ) -> SymbolId {
        if declaration.is_nil() || declaration.parent().is_nil() {
            return SymbolId::NIL;
        }
        let parent = declaration.parent();
        match parent.kind() {
            SyntaxKind::ClassDeclaration
            | SyntaxKind::ClassExpression
            | SyntaxKind::InterfaceDeclaration => tp.checker.get_symbol_of_declaration(parent),
            _ => SymbolId::NIL,
        }
    }

    // Go: typeparser/api_stability.go apiStabilityAnalysis.memberDeclaringSymbol
    /// memberDeclaringSymbol returns the class or interface symbol whose declaration
    /// directly contains the member, or nil for a member of a type literal or any
    /// other anonymous surface. Only a direct member declaration names an owner: a
    /// member nested inside a type literal stays anonymous even when that literal
    /// sits inside a class or interface.
    pub fn member_declaring_symbol(
        &mut self,
        tp: &mut TypeParser<'_>,
        member: SymbolId,
    ) -> SymbolId {
        if member.is_nil() {
            return SymbolId::NIL;
        }
        let declarations: Vec<Node> = tp.checker.sym(member).declarations.to_vec();
        for declaration in declarations {
            let declaring = self.declaration_declaring_symbol(tp, declaration);
            if declaring.is_some() {
                return declaring;
            }
        }
        SymbolId::NIL
    }

    // Go: typeparser/api_stability.go apiStabilityAnalysis.inheritedChildBoundarySymbol
    /// inheritedChildBoundarySymbol returns the class or interface symbol that
    /// declares a member when the declaring symbol is not the member table's owner
    /// and carries an explicit declared tag. A member declared by the owner is an own
    /// member and is never gated; an untagged declaring symbol returns nil. This is
    /// how a member the compiler flattened into a derived member table from a tagged
    /// base is attributed to that base instead of being expanded as the derived
    /// component's own member. A same-named own override keeps the owner as its
    /// declaring symbol and stays inspected.
    pub fn inherited_child_boundary_symbol(
        &mut self,
        tp: &mut TypeParser<'_>,
        member: SymbolId,
        owner: SymbolId,
    ) -> SymbolId {
        if member.is_nil() || owner.is_nil() {
            return SymbolId::NIL;
        }
        let declaring = self.member_declaring_symbol(tp, member);
        if declaring.is_nil() || declaring == owner {
            return SymbolId::NIL;
        }
        if tp
            .declared_api_stability_of_symbol(declaring)
            .declaration
            .is_some()
        {
            return declaring;
        }
        SymbolId::NIL
    }

    // Go: typeparser/api_stability.go apiStabilityAnalysis.inheritedChildBoundarySymbolForDeclaration
    /// inheritedChildBoundarySymbolForDeclaration returns the class or interface
    /// symbol that directly declares a signature, constructor or index declaration
    /// when that declaring symbol is not the member table's owner and carries an
    /// explicit declared tag. The compiler flattens inherited call/construct
    /// signatures and index infos into a derived structured type while keeping each
    /// declaration's base provenance, so this attributes them to the tagged base
    /// exactly like inheritedChildBoundarySymbol does for flattened members. A
    /// declaration that belongs to the owner (an own call signature, own
    /// constructor, own index signature) is never gated, and a declaration without
    /// a class or interface parent (a type literal or a synthesized signature that
    /// has no declaration) returns nil without resolving anything.
    pub fn inherited_child_boundary_symbol_for_declaration(
        &mut self,
        tp: &mut TypeParser<'_>,
        declaration: Node,
        owner: SymbolId,
    ) -> SymbolId {
        let declaring = self.declaration_declaring_symbol(tp, declaration);
        if declaring.is_nil() || declaring == owner {
            return SymbolId::NIL;
        }
        if tp
            .declared_api_stability_of_symbol(declaring)
            .declaration
            .is_some()
        {
            return declaring;
        }
        SymbolId::NIL
    }

    // Go: typeparser/api_stability.go apiStabilityAnalysis.childSignatureBoundary
    /// childSignatureBoundary records an explicitly tagged signature reached as a
    /// child component of owner and reports whether the signature is a boundary.
    /// A signature whose declaration belongs to the owner symbol is that owner's own
    /// signature (a root function's overload set) and stays walked; every other
    /// tagged signature contributes its declared level and its parameter, return and
    /// type-argument internals are not expanded. A signature the compiler flattened
    /// into a derived structured type keeps its declaration's base provenance, so a
    /// signature declared by a different, explicitly tagged class or interface is
    /// attributed to that tagged base (its declared level contributes and its
    /// internals are bounded). A synthesized default constructor signature without a
    /// declaration has no internals to expand and is never gated.
    pub fn child_signature_boundary(
        &mut self,
        tp: &mut TypeParser<'_>,
        surface: &mut ApiStabilitySurface,
        signature: SignatureId,
        owner: SymbolId,
    ) -> bool {
        let raw = raw_signature(tp.checker, signature);
        if raw.is_nil() {
            return false;
        }
        let declared = tp.declared_api_stability_of_signature(raw);
        if declared.declaration.is_some() {
            if owner.is_some()
                && api_stability_declaration_belongs_to_symbol(
                    tp.checker,
                    declared.declaration,
                    owner,
                )
            {
                return false;
            }
            self.record_signature_stability(tp, surface, raw);
            return true;
        }
        // A signature that is a declaration of the owner symbol itself (a method's
        // raw signature reached through its own function type) is the owner's own
        // exposed content, never a flattened inherited signature.
        let raw_declaration = tp.checker.sig(raw).declaration();
        if raw_declaration.is_some()
            && owner.is_some()
            && api_stability_declaration_belongs_to_symbol(tp.checker, raw_declaration, owner)
        {
            return false;
        }
        let boundary =
            self.inherited_child_boundary_symbol_for_declaration(tp, raw_declaration, owner);
        if boundary.is_some() {
            self.record_symbol(tp, surface, boundary);
            return true;
        }
        false
    }

    // Go: typeparser/api_stability.go apiStabilityAnalysis.declarationChildBoundary
    /// declarationChildBoundary records an explicitly tagged declaration reached as
    /// a child component and reports whether the declaration's internals are bounded.
    /// It is used for index signatures, whose content annotations are the
    /// component's internals. A declaration whose own `@stability` tag is explicit
    /// contributes that tag. A declaration the compiler flattened into a derived
    /// structured type is attributed to its declaring class or interface exactly
    /// like a flattened signature: an explicitly tagged declaring symbol that is not
    /// the owner contributes its declared level and bounds the declaration's
    /// internals, so a tagged base's inherited index signature stays hidden while an
    /// own index signature (declaring symbol == owner) keeps being walked.
    pub fn declaration_child_boundary(
        &mut self,
        tp: &mut TypeParser<'_>,
        surface: &mut ApiStabilitySurface,
        declaration: Node,
        owner: SymbolId,
    ) -> bool {
        if declaration.is_nil() {
            return false;
        }
        if tp
            .declared_api_stability_of_declaration(declaration)
            .declaration
            .is_some()
        {
            self.record_declaration_stability(tp, surface, declaration);
            return true;
        }
        let boundary = self.inherited_child_boundary_symbol_for_declaration(tp, declaration, owner);
        if boundary.is_some() {
            self.record_symbol(tp, surface, boundary);
            return true;
        }
        false
    }
}

// Go: typeparser/api_stability.go apiStabilityDeclarationBelongsToSymbol
/// apiStabilityDeclarationBelongsToSymbol reports whether a declaration is one
/// of a symbol's own declarations. It distinguishes a root function's overload
/// signatures (declarations of the root symbol) from child signatures declared
/// in another component.
pub fn api_stability_declaration_belongs_to_symbol(
    c: &Checker,
    declaration: Node,
    symbol: SymbolId,
) -> bool {
    if declaration.is_nil() || symbol.is_nil() {
        return false;
    }
    c.sym(symbol).declarations.contains(&declaration)
}

impl ApiStabilityAnalysis {
    // Go: typeparser/api_stability.go apiStabilityAnalysis.recordDeclarationStability
    /// recordDeclarationStability records a `@stability` tag carried by a
    /// declaration whose checker type has not been materialized. Declared member and
    /// signature stability contributes to the surface independently of type
    /// availability, so an annotation the checker has not resolved still reports its
    /// tagged members.
    pub fn record_declaration_stability(
        &mut self,
        tp: &mut TypeParser<'_>,
        surface: &mut ApiStabilitySurface,
        declaration: Node,
    ) {
        if declaration.is_nil() {
            return;
        }
        let declared = tp.declared_api_stability_of_declaration(declaration);
        if declared.level <= ApiStabilityLevel::Stable {
            return;
        }
        self.add_finding(
            surface,
            ApiStabilityFindingKey {
                declaration,
                ..Default::default()
            },
            declared,
        );
    }

    // Go: typeparser/api_stability.go apiStabilityAnalysis.addFinding
    pub fn add_finding(
        &mut self,
        surface: &mut ApiStabilitySurface,
        key: ApiStabilityFindingKey,
        declared: ApiStabilityDeclaration,
    ) {
        match surface.findings.get(&key) {
            Some(existing) if declared.level <= existing.level => {}
            _ => {
                surface.findings.insert(
                    key,
                    ApiStabilityFinding {
                        level: declared.level,
                        declaration: declared.declaration,
                    },
                );
            }
        }
    }

    // Go: typeparser/api_stability.go apiStabilityAnalysis.declarationRepresentedType
    /// declarationRepresentedType returns the represented type of a symbol's own
    /// declaration for annotation provenance. Type aliases use their declared type;
    /// values use the symbol's value type when it exists, otherwise the
    /// declaration's own annotation. An already computed component is preferred and
    /// a missing one is materialized lazily.
    pub fn declaration_represented_type(
        &mut self,
        tp: &mut TypeParser<'_>,
        symbol: SymbolId,
        declaration: Node,
    ) -> TypeId {
        let empty = ApiStabilitySubstitution::default();
        if matches!(
            declaration.kind(),
            SyntaxKind::TypeAliasDeclaration | SyntaxKind::JsTypeAliasDeclaration
        ) {
            let declared = self.declared_type_of_symbol(tp, symbol);
            if declared.is_some() {
                return declared;
            }
            return self.represented_type_from_node(tp, declaration.type_(), &empty);
        }
        let materialized =
            checker_integration::get_resolved_type_of_symbol_if_materialized(tp.checker, symbol);
        if materialized.is_some() {
            return materialized;
        }
        match declaration.kind() {
            SyntaxKind::VariableDeclaration
            | SyntaxKind::PropertyDeclaration
            | SyntaxKind::PropertySignature => {
                self.represented_type_from_node(tp, declaration.type_(), &empty)
            }
            _ => TypeId::NIL,
        }
    }

    // Go: typeparser/api_stability.go apiStabilityAnalysis.declarationAnnotationNode
    /// declarationAnnotationNode returns the public type annotation a declaration
    /// exposes for metadata reading, or nil.
    pub fn declaration_annotation_node(&self, declaration: Node) -> Node {
        if declaration.is_nil() {
            return Node::NIL;
        }
        if api_stability_has_function_like_data(declaration) {
            return declaration.type_();
        }
        match declaration.kind() {
            SyntaxKind::VariableDeclaration
            | SyntaxKind::PropertyDeclaration
            | SyntaxKind::PropertySignature
            | SyntaxKind::TypeAliasDeclaration
            | SyntaxKind::JsTypeAliasDeclaration => declaration.type_(),
            _ => Node::NIL,
        }
    }

    // Go: typeparser/api_stability.go apiStabilityAnalysis.conditionalDeclarationIsDeferred
    /// conditionalDeclarationIsDeferred reports whether a conditional written at a
    /// declaration site is symbolic: one of its operands mentions an unbound type
    /// parameter or an infer binder. The check is a narrow binder scan; it resolves
    /// nothing. A fully concrete conditional has a compiler outcome that the
    /// declaration does not represent and is never inspected through its branches.
    // PORT: Go `node.AsConditionalTypeNode()` is nil-checked; a node of another
    // kind returns there.
    pub fn conditional_declaration_is_deferred(
        &mut self,
        tp: &mut TypeParser<'_>,
        node: Node,
    ) -> bool {
        if node.is_nil() || node.kind() != SyntaxKind::ConditionalType {
            return false;
        }
        let mut visiting = FxHashMap::default();
        self.type_node_mentions_type_parameter(tp, node.check_type(), &mut visiting)
            || self.type_node_mentions_type_parameter(tp, node.extends_type(), &mut visiting)
    }

    // Go: typeparser/api_stability.go apiStabilityAnalysis.typeNodeMentionsTypeParameter
    /// typeNodeMentionsTypeParameter reports whether a type node names an unbound
    /// type parameter (including an `infer` binder). It only looks up annotation
    /// symbols; no annotation type is resolved.
    pub fn type_node_mentions_type_parameter(
        &mut self,
        tp: &mut TypeParser<'_>,
        node: Node,
        visiting: &mut FxHashMap<Node, bool>,
    ) -> bool {
        if node.is_nil() || visiting.get(&node).copied().unwrap_or(false) {
            return false;
        }
        visiting.insert(node, true);
        let result = self.type_node_mentions_type_parameter_body(tp, node, visiting);
        visiting.remove(&node);
        result
    }

    /// PORT: the body of Go `typeNodeMentionsTypeParameter` after the visiting
    /// insert (Go `defer delete(visiting, node)`).
    fn type_node_mentions_type_parameter_body(
        &mut self,
        tp: &mut TypeParser<'_>,
        node: Node,
        visiting: &mut FxHashMap<Node, bool>,
    ) -> bool {
        match node.kind() {
            SyntaxKind::InferType => true,
            SyntaxKind::TypeReference => {
                let symbol = self.symbol_at_type_name_node(tp, node.type_name());
                if symbol.is_some()
                    && tp
                        .checker
                        .sym(symbol)
                        .flags
                        .intersects(SymbolFlags::TYPE_PARAMETER)
                {
                    return true;
                }
                for argument in node.type_arguments() {
                    if self.type_node_mentions_type_parameter(tp, argument, visiting) {
                        return true;
                    }
                }
                false
            }
            SyntaxKind::ArrayType => {
                self.type_node_mentions_type_parameter(tp, node.element_type(), visiting)
            }
            SyntaxKind::TupleType => {
                for element in node.elements() {
                    if self.type_node_mentions_type_parameter(tp, element, visiting) {
                        return true;
                    }
                }
                false
            }
            SyntaxKind::NamedTupleMember
            | SyntaxKind::OptionalType
            | SyntaxKind::RestType
            | SyntaxKind::ParenthesizedType
            | SyntaxKind::TypeOperator => {
                self.type_node_mentions_type_parameter(tp, node.type_(), visiting)
            }
            SyntaxKind::IndexedAccessType => {
                self.type_node_mentions_type_parameter(tp, node.object_type(), visiting)
                    || self.type_node_mentions_type_parameter(tp, node.index_type(), visiting)
            }
            SyntaxKind::UnionType | SyntaxKind::IntersectionType => {
                let list = node.types();
                if list.is_some() {
                    for member in list.nodes() {
                        if self.type_node_mentions_type_parameter(tp, member, visiting) {
                            return true;
                        }
                    }
                }
                false
            }
            SyntaxKind::TemplateLiteralType => {
                let spans = node.template_spans();
                if spans.is_some() {
                    for span in spans.nodes() {
                        if span.is_nil() || span.kind() != SyntaxKind::TemplateLiteralTypeSpan {
                            continue;
                        }
                        if self.type_node_mentions_type_parameter(tp, span.type_(), visiting) {
                            return true;
                        }
                    }
                }
                false
            }
            SyntaxKind::ConditionalType => {
                self.type_node_mentions_type_parameter(tp, node.check_type(), visiting)
                    || self.type_node_mentions_type_parameter(tp, node.extends_type(), visiting)
            }
            _ => false,
        }
    }

    // Go: typeparser/api_stability.go apiStabilityAnalysis.shouldInspectSymbol
    /// shouldInspectSymbol reports whether a symbol has at least one declaration
    /// outside the default library. The standard library is never stability-tagged
    /// and expanding it would pull a large, irrelevant surface into every reference.
    pub fn should_inspect_symbol(&mut self, tp: &mut TypeParser<'_>, symbol: SymbolId) -> bool {
        if symbol.is_nil() || tp.checker.sym(symbol).declarations.is_empty() {
            return false;
        }
        tp.checker
            .sym(symbol)
            .declarations
            .iter()
            .any(|&declaration| {
                let source_file = get_source_file_of_node(declaration);
                source_file.is_nil()
                    || !is_source_file_default_library(&source_file_info(source_file).path)
            })
    }

    // Go: typeparser/api_stability.go apiStabilityAnalysis.shouldInspectMemberDeclaration
    /// shouldInspectMemberDeclaration reports whether a member declaration's public
    /// annotation should be inspected for erased alias provenance. Function-like
    /// declarations are covered by their public signatures, except accessors whose
    /// value annotation is the only place an erased alias survives.
    pub fn should_inspect_member_declaration(&self, declaration: Node) -> bool {
        if declaration.is_nil() {
            return false;
        }
        if !api_stability_has_function_like_data(declaration) {
            return true;
        }
        matches!(
            declaration.kind(),
            SyntaxKind::GetAccessor | SyntaxKind::SetAccessor
        )
    }
}

// Go: typeparser/api_stability.go typeParameterAnnotationNode
/// typeParameterAnnotationNode returns the declared constraint or default
/// annotation of a type parameter.
pub fn type_parameter_annotation_node(c: &Checker, t: TypeId, constraint: bool) -> Node {
    if t.is_nil() {
        return Node::NIL;
    }
    let symbol = c.ty(t).symbol();
    if symbol.is_nil() {
        return Node::NIL;
    }
    for &declaration in &c.sym(symbol).declarations {
        if declaration.is_nil() || declaration.kind() != SyntaxKind::TypeParameter {
            continue;
        }
        if constraint {
            return declaration.constraint();
        }
        return declaration.default_type();
    }
    Node::NIL
}

// Go: typeparser/api_stability.go signatureReturnTypeNode
/// signatureReturnTypeNode returns the declared return type annotation of a
/// signature, or nil when the return type is inferred.
pub fn signature_return_type_node(c: &Checker, signature: SignatureId) -> Node {
    if signature.is_nil() {
        return Node::NIL;
    }
    let declaration = c.sig(signature).declaration();
    if declaration.is_nil() || !api_stability_has_function_like_data(declaration) {
        return Node::NIL;
    }
    declaration.type_()
}

// Go: typeparser/api_stability.go apiStabilityIndexSignatureKeyNode
/// apiStabilityIndexSignatureKeyNode returns the key type node of an index
/// signature declaration.
pub fn api_stability_index_signature_key_node(declaration: Node) -> Node {
    if declaration.is_nil() || declaration.kind() != SyntaxKind::IndexSignature {
        return Node::NIL;
    }
    let parameters = declaration.parameters();
    if parameters.len() != 1 {
        return Node::NIL;
    }
    let parameter = parameters.get(0);
    if parameter.is_nil() || parameter.kind() != SyntaxKind::Parameter {
        return Node::NIL;
    }
    parameter.type_()
}

// Go: typeparser/api_stability.go apiStabilityIndexSignatureValueNode
/// apiStabilityIndexSignatureValueNode returns the value type node of an index
/// signature declaration.
pub fn api_stability_index_signature_value_node(declaration: Node) -> Node {
    if declaration.is_nil() || declaration.kind() != SyntaxKind::IndexSignature {
        return Node::NIL;
    }
    declaration.type_()
}

// Go: typeparser/api_stability.go apiStabilityMappedDeclarationHasName
/// apiStabilityMappedDeclarationHasName reports whether a mapped type declared a
/// `as` clause whose name type the checker has not materialized.
pub fn api_stability_mapped_declaration_has_name(c: &Checker, t: TypeId) -> bool {
    if t.is_nil() {
        return false;
    }
    let symbol = c.ty(t).symbol();
    if symbol.is_nil() {
        return false;
    }
    c.sym(symbol).declarations.iter().any(|&declaration| {
        declaration.is_some()
            && declaration.kind() == SyntaxKind::MappedType
            && declaration.name_type().is_some()
    })
}

// Go: typeparser/api_stability.go apiStabilityTypeAliasDeclaration
/// apiStabilityTypeAliasDeclaration returns the type alias declaration of a
/// symbol.
pub fn api_stability_type_alias_declaration(c: &Checker, symbol: SymbolId) -> Node {
    if symbol.is_nil() {
        return Node::NIL;
    }
    c.sym(symbol)
        .declarations
        .iter()
        .copied()
        .find(|&declaration| {
            declaration.is_some()
                && matches!(
                    declaration.kind(),
                    SyntaxKind::TypeAliasDeclaration | SyntaxKind::JsTypeAliasDeclaration
                )
        })
        .unwrap_or(Node::NIL)
}

// Go: typeparser/api_stability.go apiStabilitySymbolDeclaresHeritage
/// apiStabilitySymbolDeclaresHeritage reports whether a class or interface
/// symbol declares heritage clauses. It reads declaration structure only and is
/// used to distinguish an unmaterialized heritage surface from one with nothing
/// to resolve.
pub fn api_stability_symbol_declares_heritage(c: &Checker, symbol: SymbolId) -> bool {
    if symbol.is_nil() {
        return false;
    }
    for &declaration in &c.sym(symbol).declarations {
        if declaration.is_nil() {
            continue;
        }
        let clauses = match declaration.kind() {
            SyntaxKind::ClassDeclaration
            | SyntaxKind::ClassExpression
            | SyntaxKind::InterfaceDeclaration => declaration.heritage_clauses(),
            _ => continue,
        };
        if clauses.is_nil() {
            continue;
        }
        for clause in clauses.nodes() {
            if clause.is_nil() {
                continue;
            }
            let types = clause.types();
            if types.is_some() && types.nodes().len() != 0 {
                return true;
            }
        }
    }
    false
}

// Go: typeparser/api_stability.go sortedSymbolTableKeys
// PORT: Go `sort.Strings` compares Go bytes (`scanner_util::compare_go_strings`).
pub fn sorted_symbol_table_keys(c: &Checker, table: SymbolTable) -> Vec<String> {
    let mut keys: Vec<String> = c
        .symbols
        .iter(table)
        .map(|(key, _)| key.to_string())
        .collect();
    keys.sort_by(|left, right| crate::scanner_util::compare_go_strings(left, right));
    keys
}

// Go: typeparser/api_stability.go apiStabilityMemberTableNameIsVisible
/// apiStabilityMemberTableNameIsVisible reports whether a member-table entry is
/// a candidate public member. Entries whose names use the checker's documented
/// internal symbol-name prefix are reserved implementation spellings (`__call`,
/// `__new`, `__index`, `__export`, the binder's `__computed` placeholder, ambient
/// module patterns, ...) and are never public members. A late-bound computed
/// member is the exception: the checker names a computed declaration by the
/// property name its literal or unique-symbol value resolves to, and
/// `checker.IsKnownSymbol` identifies exactly those spellings. Its accessibility
/// is still decided by its declaration provenance (`apiStabilitySymbolIsNonPublic`)
/// at the call site, and an unresolved dynamic name keeps the `__computed`
/// placeholder, which stays invisible.
pub fn api_stability_member_table_name_is_visible(
    c: &Checker,
    name: &str,
    member: SymbolId,
) -> bool {
    if member.is_nil() {
        return false;
    }
    if !name.starts_with(INTERNAL_SYMBOL_NAME_PREFIX) {
        return true;
    }
    c.is_known_symbol(member)
}

// Go: typeparser/api_stability.go apiStabilitySymbolIsNonPublic
/// apiStabilitySymbolIsNonPublic classifies a member symbol by its declaration
/// accessibility. Private identifiers, private/protected members and members
/// whose declarations are all internal are excluded; a leading `__` name is
/// not treated as private.
pub fn api_stability_symbol_is_non_public(c: &mut Checker, symbol: SymbolId) -> bool {
    if symbol.is_nil() {
        return true;
    }
    if c.is_private_identifier_symbol(symbol)
        || api_stability_symbol_has_only_internal_declarations(c, symbol)
    {
        return true;
    }
    c.get_declaration_modifier_flags_from_symbol(symbol)
        .intersects(ModifierFlags::PRIVATE | ModifierFlags::PROTECTED)
}

// Go: typeparser/api_stability.go apiStabilitySymbolHasOnlyInternalDeclarations
/// A public declaration keeps a merged property visible even when another
/// declaration marks that property internal. Declaration-less synthetic members
/// also stay visible rather than inheriting the visibility of a referenced type.
pub fn api_stability_symbol_has_only_internal_declarations(c: &Checker, symbol: SymbolId) -> bool {
    if symbol.is_nil() || c.sym(symbol).declarations.is_empty() {
        return false;
    }
    c.sym(symbol)
        .declarations
        .iter()
        .all(|&declaration| api_stability_property_declaration_is_internal(declaration))
}

// Go: typeparser/api_stability.go apiStabilityPropertyDeclarationIsInternal
pub fn api_stability_property_declaration_is_internal(declaration: Node) -> bool {
    if declaration.is_nil() {
        return false;
    }
    match declaration.kind() {
        SyntaxKind::PropertySignature
        | SyntaxKind::PropertyDeclaration
        | SyntaxKind::MethodSignature
        | SyntaxKind::MethodDeclaration
        | SyntaxKind::GetAccessor
        | SyntaxKind::SetAccessor
        | SyntaxKind::Parameter => internal_tag_of_declaration(declaration),
        _ => false,
    }
}

// Go: typeparser/api_stability.go apiStabilitySignatureIsNonPublic
/// apiStabilitySignatureIsNonPublic filters construct and method signatures whose
/// declaration is private or protected.
pub fn api_stability_signature_is_non_public(c: &Checker, signature: SignatureId) -> bool {
    let declaration = c.sig(signature).declaration();
    if declaration.is_nil() {
        return false;
    }
    let name = get_name_of_declaration(declaration);
    if name.is_some() && is_private_identifier(name) {
        return true;
    }
    get_combined_modifier_flags(declaration)
        .intersects(ModifierFlags::PRIVATE | ModifierFlags::PROTECTED)
}
