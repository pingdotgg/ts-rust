// Go: internal/typeparser/helpers.go

use crate::effect::typeparser::*;
use crate::prelude::*;

impl TypeParser<'_> {
    /// extractCovariantType gets the type argument from a covariant property.
    /// Covariant<A> is encoded as () => A, so we get the return type.
    pub fn extract_covariant_type(&mut self, t: TypeId, prop_name: &str) -> TypeId {
        let prop_type = self.get_type_of_property_by_name(t, prop_name);
        if prop_type.is_nil() {
            return TypeId::NIL;
        }
        let c = &mut *self.checker;
        let signatures = c.get_signatures_of_type_exported(prop_type, SignatureKind::CALL);

        if signatures.len() != 1 {
            return TypeId::NIL;
        }

        if !c.sig(signatures[0]).type_parameters().is_empty() {
            return TypeId::NIL;
        }

        c.get_return_type_of_signature_exported(signatures[0])
    }

    /// extractContravariantType gets the type argument from a contravariant property.
    /// Contravariant<A> is encoded as (_: A) => void, so we get the first parameter type.
    pub fn extract_contravariant_type(&mut self, t: TypeId, prop_name: &str) -> TypeId {
        let prop_type = self.get_type_of_property_by_name(t, prop_name);
        if prop_type.is_nil() {
            return TypeId::NIL;
        }
        let c = &mut *self.checker;
        let signatures = c.get_signatures_of_type_exported(prop_type, SignatureKind::CALL);

        if signatures.len() != 1 {
            return TypeId::NIL;
        }

        if !c.sig(signatures[0]).type_parameters().is_empty() {
            return TypeId::NIL;
        }

        let params = c.sig(signatures[0]).parameters().to_vec();
        if params.is_empty() {
            return TypeId::NIL;
        }

        c.get_type_of_symbol_exported(params[0])
    }

    /// extractInvariantType gets the type argument from an invariant property.
    /// Invariant<A> is encoded as (_: A) => A, so we extract the return type (same as covariant).
    pub fn extract_invariant_type(&mut self, t: TypeId, prop_name: &str) -> TypeId {
        self.extract_covariant_type(t, prop_name)
    }

    /// GetTypeOfPropertyByName returns the type of a property by name.
    /// Prefer this when only the property type is needed.
    pub fn get_type_of_property_by_name(&mut self, t: TypeId, name: &str) -> TypeId {
        if t.is_nil() {
            return TypeId::NIL;
        }
        self.checker.get_type_of_property_of_type_exported(t, name)
    }

    /// GetSymbolAtLocation wraps checker.GetSymbolAtLocation with a meta-property
    /// guard. Meta properties (import.meta, import.defer, new.target) never
    /// reference a symbol rules care about, and the checker debug-asserts (panics)
    /// when asked for `import.defer` used as an import-call callee. Always use
    /// this instead of the raw checker call.
    pub fn get_symbol_at_location(&mut self, node: Node) -> SymbolId {
        if node.is_nil() {
            return SymbolId::NIL;
        }
        if node.kind() == SyntaxKind::MetaProperty {
            return SymbolId::NIL;
        }
        self.checker.get_symbol_at_location_exported(node)
    }

    pub fn resolve_aliased_symbol(&mut self, sym: SymbolId) -> SymbolId {
        let c = &mut *self.checker;
        let mut sym = sym;
        while sym.is_some() && c.sym_flags(sym).intersects(SymbolFlags::ALIAS) {
            sym = c.get_aliased_symbol(sym);
        }
        sym
    }

    /// ResolveToGlobalSymbol follows aliases and up to two simple variable indirections
    /// so rules can recognize references to the original global symbol.
    pub fn resolve_to_global_symbol(&mut self, sym: SymbolId) -> SymbolId {
        if sym.is_nil() {
            return SymbolId::NIL;
        }

        let mut sym = self.resolve_aliased_symbol(sym);
        let mut depth = 0;
        while depth < 2 && sym.is_some() {
            let value_declaration = self.checker.sym(sym).value_declaration;
            if value_declaration.is_nil()
                || value_declaration.kind() != SyntaxKind::VariableDeclaration
            {
                break;
            }
            let decl = value_declaration;
            if decl.initializer().is_nil() {
                break;
            }

            let mut next = self.get_symbol_at_location(decl.initializer());
            if next.is_nil() {
                break;
            }
            next = self.resolve_aliased_symbol(next);
            if next == sym {
                break;
            }

            sym = next;
            depth += 1;
        }

        sym
    }

    /// IsNodeReferenceToGlobalMember reports whether node resolves to a property of
    /// a global value. ReferenceSymbolAtNode follows import and const aliases, so
    /// callers do not need to constrain the source expression to property-access
    /// syntax merely to recognize the global API.
    pub fn is_node_reference_to_global_member(
        &mut self,
        node: Node,
        global_name: &str,
        member_name: &str,
    ) -> bool {
        if node.is_nil() {
            return false;
        }
        let global =
            self.checker
                .resolve_name_exported(global_name, Node::NIL, SymbolFlags::VALUE, false);
        if global.is_nil() {
            return false;
        }
        let global_type = self.checker.get_type_of_symbol_at_location(global, node);
        if global_type.is_nil() {
            return false;
        }
        let member = self
            .checker
            .get_property_of_type_exported(global_type, member_name);
        let actual = self.reference_symbol_at_node(node);
        member.is_some()
            && actual.is_some()
            && self
                .checker
                .get_symbol_if_same_reference(member, actual)
                .is_some()
    }
}
