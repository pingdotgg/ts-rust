// Go: internal/typeparser/module_export_reference.go

use crate::effect::typeparser::*;
use crate::prelude::*;
use std::sync::atomic::{AtomicU32, Ordering};

/// Go `func(*TypeParser, *checker.Checker, *ast.SourceFile) bool`. The
/// checker is `tp.checker`.
pub type MatchesSourceFileFn = fn(&mut TypeParser<'_>, Node) -> bool;

pub struct PackageSourceFileDescriptor {
    pub package_name: String,
    pub matches_source_file: Option<MatchesSourceFileFn>,
    pub cache_key: Option<PackageSourceFileDescriptorCacheKey>,
}

/// Go `packageSourceFileDescriptorCacheKey`. Go uses the address of a
/// fresh `new(packageSourceFileDescriptorCacheKey)` as the identity of a
/// descriptor; the port gives each descriptor a fresh number.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PackageSourceFileDescriptorCacheKey(pub u32);

/// The next `PackageSourceFileDescriptorCacheKey` (Go `new`).
static NEXT_PACKAGE_SOURCE_FILE_DESCRIPTOR_CACHE_KEY: AtomicU32 = AtomicU32::new(1);

/// Go `moduleExportReferenceCacheKey`.
/// PORT: Go keys by the member name. Here `member` is the descriptor id
/// (low 8 bits) and the id of the member name in
/// `EffectLinks::module_export_members` (high 24 bits), so an entry with
/// its value is 12 bytes and no key holds a string.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ModuleExportReferenceCacheKey {
    pub symbol: SymbolId,
    pub member: u32,
}

// Go: newPackageSourceFileDescriptor
/// Descriptors are package-level values (`LazyLock` statics in the port).
pub fn new_package_source_file_descriptor(
    package_name: &str,
    matches_source_file: Option<MatchesSourceFileFn>,
) -> PackageSourceFileDescriptor {
    PackageSourceFileDescriptor {
        package_name: package_name.to_string(),
        matches_source_file,
        cache_key: Some(PackageSourceFileDescriptorCacheKey(
            NEXT_PACKAGE_SOURCE_FILE_DESCRIPTOR_CACHE_KEY.fetch_add(1, Ordering::Relaxed),
        )),
    }
}

impl TypeParser<'_> {
    pub fn reference_symbol_at_node(&mut self, node: Node) -> SymbolId {
        if node.is_nil() {
            return SymbolId::NIL;
        }

        // Go `Cached(&tp.links.ReferenceSymbol, node, ..)`.
        if let Some(cached) = self.links().reference_symbol.try_get(node) {
            return SymbolId(cached.get());
        }
        let mut sym = self.get_symbol_at_location(node);
        if sym.is_nil() && node.kind() == SyntaxKind::PropertyAccessExpression {
            let prop = node;
            if prop.name().is_some() {
                sym = self.get_symbol_at_location(prop.name());
            }
        }

        sym = self.resolve_aliased_symbol(sym);
        if node.kind() == SyntaxKind::Identifier && !is_declaration_name(node) {
            sym = self.resolve_constant_alias_symbol(sym);
        }
        *self.links().reference_symbol.get(node) = CachedId::new(sym.0);
        sym
    }

    pub fn resolve_constant_alias_symbol(&mut self, sym: SymbolId) -> SymbolId {
        let mut sym = sym;
        let mut seen: FxHashSet<SymbolId> = FxHashSet::default();
        while sym.is_some() {
            if seen.contains(&sym) {
                return sym;
            }
            seen.insert(sym);

            let declaration_node = self.checker.sym(sym).value_declaration;
            if declaration_node.is_nil()
                || declaration_node.kind() != SyntaxKind::VariableDeclaration
                || declaration_node.parent().is_nil()
                || declaration_node.parent().kind() != SyntaxKind::VariableDeclarationList
                || !declaration_node
                    .parent()
                    .flags()
                    .intersects(NodeFlags::CONST)
            {
                return sym;
            }
            let declaration = declaration_node;
            if declaration.initializer().is_nil() {
                return sym;
            }

            let next = self.get_symbol_at_location(skip_parentheses(declaration.initializer()));
            if next.is_nil() {
                return sym;
            }
            sym = self.resolve_aliased_symbol(next);
        }
        SymbolId::NIL
    }

    pub fn is_source_file_in_package(&mut self, sf: Node, package_name: &str) -> bool {
        if sf.is_nil() {
            return false;
        }
        let Some(pkg) = self.package_json_for_source_file(sf) else {
            return false;
        };
        let (name, ok) = pkg.fields.name.get_value();
        ok && crate::frontend::vfs::vfsmatch::equal_fold(name.as_bytes(), package_name.as_bytes())
    }

    pub fn is_node_reference_to_module_export(
        &mut self,
        node: Node,
        desc: &PackageSourceFileDescriptor,
        member_name: &str,
    ) -> bool {
        let sym = self.reference_symbol_at_node(node);
        if sym.is_nil() {
            return false;
        }
        // Exported descriptors built as struct literals have no stable matcher identity.
        let Some(cache_key) = desc.cache_key else {
            return self.is_symbol_reference_to_module_export(sym, desc, member_name);
        };

        let Some(member) = self.module_export_member_key(cache_key, member_name) else {
            return self.is_symbol_reference_to_module_export(sym, desc, member_name);
        };
        let key = ModuleExportReferenceCacheKey {
            symbol: sym,
            member,
        };
        cached!(self, module_export_reference, key, {
            self.is_symbol_reference_to_module_export(sym, desc, member_name)
        })
    }

    /// The `member` of a `ModuleExportReferenceCacheKey`, or `None` when the
    /// descriptor id or the member name id does not fit (then the caller
    /// does not cache, as for a descriptor with no cache key).
    fn module_export_member_key(
        &mut self,
        descriptor: PackageSourceFileDescriptorCacheKey,
        member_name: &str,
    ) -> Option<u32> {
        let descriptor = u8::try_from(descriptor.0).ok()?;
        let members = &mut self.links().module_export_members;
        let id = match members.get(member_name) {
            Some(&id) => id,
            None => {
                let id = u32::try_from(members.len()).ok()?;
                members.insert(member_name.into(), id);
                id
            }
        };
        (id < 1 << 24).then_some((id << 8) | u32::from(descriptor))
    }

    pub fn is_symbol_reference_to_module_export(
        &mut self,
        sym: SymbolId,
        desc: &PackageSourceFileDescriptor,
        member_name: &str,
    ) -> bool {
        let declarations = self.checker.sym(sym).declarations.to_vec();
        for decl in declarations {
            if decl.is_nil() {
                continue;
            }
            let sf = get_source_file_of_node(decl);
            if sf.is_nil() || !self.is_source_file_in_package(sf, &desc.package_name) {
                continue;
            }
            if let Some(matches_source_file) = desc.matches_source_file
                && !matches_source_file(self, sf)
            {
                continue;
            }
            let module_sym = self.checker.get_symbol_of_declaration(sf);
            if module_sym.is_nil() {
                continue;
            }
            let mut export_sym = self
                .checker
                .try_get_member_in_module_exports_and_properties(member_name, module_sym);
            export_sym = self.resolve_aliased_symbol(export_sym);
            if self
                .checker
                .get_symbol_if_same_reference(export_sym, sym)
                .is_some()
            {
                return true;
            }
        }

        false
    }

    pub fn is_node_reference_to_module(
        &mut self,
        node: Node,
        desc: &PackageSourceFileDescriptor,
    ) -> bool {
        let sym = self.reference_symbol_at_node(node);
        if sym.is_nil() {
            return false;
        }

        let declarations = self.checker.sym(sym).declarations.to_vec();
        for decl in declarations {
            if decl.is_nil() {
                continue;
            }
            let sf = get_source_file_of_node(decl);
            if sf.is_nil() || !self.is_source_file_in_package(sf, &desc.package_name) {
                continue;
            }
            match desc.matches_source_file {
                None => return true,
                Some(matches_source_file) => {
                    if matches_source_file(self, sf) {
                        return true;
                    }
                }
            }
        }

        false
    }
}
