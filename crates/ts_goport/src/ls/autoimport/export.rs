use crate::ls::autoimport::prelude::*;

// Port of Go `ls/autoimport/export.go` and `ls/autoimport/export_stringer_generated.go`.

use crate::flags_macros::go_enum;
use crate::frontend::tspath;
use crate::ls::lsutil;

// Go: ls/autoimport/export.go:13 moduleIDKind (ts#64159)
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum ModuleIDKind {
    #[default]
    Invalid, // moduleIDKindInvalid
    File,    // moduleIDKindFile
    Ambient, // moduleIDKindAmbient
}

// Go: ls/autoimport/export.go:22 ModuleID (ts#64159)
// ModuleID uniquely identifies either a file module or an ambient module.
// PORT: ts#64159 behavior only: Go `tspath.PathKey` is `tspath::Path` and Go
// `tspath.ModuleSpecifier` is `String`. Before ts#64159 a ModuleID was a
// string, and a name was ambient when it was not a relative or rooted name;
// the kind now says it. A module augmentation of a relative name is a file
// module (its resolved or unresolved file), an ambient module declaration is
// ambient whatever its name.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ModuleID {
    path: tspath::Path,
    specifier: String,
    kind: ModuleIDKind,
}

// Go: ls/autoimport/export.go:28 fileModuleID (ts#64159)
pub fn file_module_id(path: tspath::Path) -> ModuleID {
    ModuleID {
        path,
        kind: ModuleIDKind::File,
        ..Default::default()
    }
}

// Go: ls/autoimport/export.go:32 ambientModuleID (ts#64159)
pub fn ambient_module_id(specifier: &str) -> ModuleID {
    ModuleID {
        specifier: specifier.to_string(),
        kind: ModuleIDKind::Ambient,
        ..Default::default()
    }
}

impl ModuleID {
    // Go: ls/autoimport/export.go:36 AsString (ts#64159)
    pub fn as_string(&self) -> &str {
        match self.kind {
            ModuleIDKind::File => &self.path.0,
            ModuleIDKind::Ambient => &self.specifier,
            ModuleIDKind::Invalid => "",
        }
    }

    // Go: ls/autoimport/export.go:47 IsAmbient (ts#64159)
    pub fn is_ambient(&self) -> bool {
        self.kind == ModuleIDKind::Ambient
    }

    // Go: ls/autoimport/export.go:51 AsPathKey (ts#64159)
    // PORT: Go `(PathKey, bool)` is `Option`.
    pub fn as_path_key(&self) -> Option<&tspath::Path> {
        if self.kind != ModuleIDKind::File {
            return None;
        }
        Some(&self.path)
    }

    // Go: ls/autoimport/export.go:58 AsModuleSpecifier (ts#64159)
    // PORT: Go `(ModuleSpecifier, bool)` is `Option`.
    pub fn as_module_specifier(&self) -> Option<&str> {
        if self.kind != ModuleIDKind::Ambient {
            return None;
        }
        Some(&self.specifier)
    }
}

// Go: ls/autoimport/export.go:65 ExportID
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct ExportID {
    pub module_id: ModuleID,
    pub export_name: String,
}

// Go: ls/autoimport/export.go:24 ExportSyntax
go_enum!(ExportSyntax, i32 {
    NONE = 0; // ExportSyntaxNone
    // export const x = {}
    MODIFIER = 1; // ExportSyntaxModifier
    // export { x }
    NAMED = 2; // ExportSyntaxNamed
    // export default function f() {}
    DEFAULT_MODIFIER = 3; // ExportSyntaxDefaultModifier
    // export default f
    DEFAULT_DECLARATION = 4; // ExportSyntaxDefaultDeclaration
    // export = x
    EQUALS = 5; // ExportSyntaxEquals
    // export as namespace x
    UMD = 6; // ExportSyntaxUMD
    // export * from "module"
    STAR = 7; // ExportSyntaxStar
    // module.exports = {}
    COMMON_JS_MODULE_EXPORTS = 8; // ExportSyntaxCommonJSModuleExports
    // exports.x = {}
    COMMON_JS_EXPORTS_PROPERTY = 9; // ExportSyntaxCommonJSExportsProperty
});

// Go: ls/autoimport/export.go:94 Export
// PORT: Go embeds `ExportID`; it is the nested `export_id` field, and `Deref`
// promotes its fields (`export.module_id`, `export.export_name`) as Go does.
// Go `*Export` values are shared (`Rc<Export>`) once they are complete.
#[derive(Clone, Debug, Default)]
pub struct Export {
    pub export_id: ExportID,
    pub module_file_name: String,
    // ts#64159: the specifier of a relative module augmentation whose module
    // did not resolve (export.go:97, extract.go:166).
    pub unresolved_module_specifier: String,
    pub syntax: ExportSyntax,
    pub flags: SymbolFlags,
    pub local_name: String,
    // through is the name of the module symbol's export that this export was found on,
    // either 'export=', InternalSymbolNameExportStar, or empty string.
    pub through: String,

    // Checker-set fields
    pub target: ExportID,
    pub is_type_only: bool,
    pub script_element_kind: lsutil::ScriptElementKind,
    pub script_element_kind_modifiers: lsutil::ScriptElementKindModifier,

    // The file where the export was found.
    pub path: tspath::Path,

    pub package_name: String,
}

impl std::ops::Deref for Export {
    type Target = ExportID;
    fn deref(&self) -> &ExportID {
        &self.export_id
    }
}

impl std::ops::DerefMut for Export {
    fn deref_mut(&mut self) -> &mut ExportID {
        &mut self.export_id
    }
}

impl Export {
    // Go: ls/autoimport/export.go:71 Name
    pub fn name(&self) -> String {
        if !self.local_name.is_empty() {
            return self.local_name.clone();
        }
        if self.export_id.export_name == INTERNAL_SYMBOL_NAME_EXPORT_EQUALS {
            return self.target.export_name.clone();
        }
        self.export_id.export_name.clone()
    }

    // Go: ls/autoimport/export.go:81 IsRenameable
    pub fn is_renameable(&self) -> bool {
        self.export_id.export_name == INTERNAL_SYMBOL_NAME_EXPORT_EQUALS
            || self.export_id.export_name == INTERNAL_SYMBOL_NAME_DEFAULT
    }

    // Go: ls/autoimport/export.go:132 AmbientModuleName
    pub fn ambient_module_name(&self) -> String {
        // ts#64159: the module ID kind, not the shape of the name.
        if self.export_id.module_id.is_ambient() {
            return self.export_id.module_id.as_string().to_string();
        }
        String::new()
    }

    // Go: ls/autoimport/export.go:92 IsUnresolvedAlias
    pub fn is_unresolved_alias(&self) -> bool {
        self.flags == SymbolFlags::ALIAS
    }
}

// Go: ls/autoimport/export.go:96 SymbolToExport
// PORT: Go `*Export` result; nil is `None`.
pub fn symbol_to_export(symbol: SymbolId, ch: &mut Checker) -> Option<Rc<Export>> {
    let parent = ch.sym(symbol).parent;
    if parent.is_some() && ch.is_external_module_symbol(parent) {
        let (module_id, module_file_name, ok) =
            try_get_module_id_and_file_name_of_module_symbol(&ch.symbols, parent);
        if ok {
            let file = get_source_file_of_module(&ch.symbols, parent);
            return extract_first_export(symbol, ch, &module_id, &module_file_name, file);
        }
        return None;
    }

    // Go: core.FirstOrNil(symbol.Declarations)
    let declaration = ch
        .sym(symbol)
        .declarations
        .first()
        .copied()
        .unwrap_or(Node::NIL);
    if declaration.is_nil() {
        return None;
    }

    let file = get_source_file_of_node(declaration);
    if file.symbol().is_nil() {
        return None;
    }

    let module_symbol = ch.get_merged_symbol_exported(file.symbol());
    let module_id = file_module_id(tspath::Path(source_file_info(file).path.clone()));
    let module_file_name = source_file_file_name(file).to_string();
    let skipped = ch.skip_alias_exported(symbol);
    let target = ch.get_merged_symbol_exported(skipped);

    if let Some(export) = try_get_module_export(
        INTERNAL_SYMBOL_NAME_DEFAULT,
        target,
        module_symbol,
        ch,
        &module_id,
        &module_file_name,
        file,
    ) {
        return Some(export);
    }
    if let Some(export) = try_get_module_export(
        INTERNAL_SYMBOL_NAME_EXPORT_EQUALS,
        target,
        module_symbol,
        ch,
        &module_id,
        &module_file_name,
        file,
    ) {
        return Some(export);
    }
    let name = ch.sym(symbol).name.to_string();
    try_get_module_export(
        &name,
        target,
        module_symbol,
        ch,
        &module_id,
        &module_file_name,
        file,
    )
}

// Go: ls/autoimport/export.go:128 tryGetModuleExport
pub fn try_get_module_export(
    export_name: &str,
    target: SymbolId,
    module_symbol: SymbolId,
    ch: &mut Checker,
    module_id: &ModuleID,
    module_file_name: &str,
    file: Node,
) -> Option<Rc<Export>> {
    let exported = ch.try_get_member_in_module_exports_and_properties(export_name, module_symbol);
    if exported.is_some() {
        let skipped = ch.skip_alias_exported(exported);
        if ch.get_merged_symbol_exported(skipped) == target {
            return extract_first_export(exported, ch, module_id, module_file_name, file);
        }
    }
    None
}

// Go: ls/autoimport/export.go:136 extractFirstExport
pub fn extract_first_export(
    symbol: SymbolId,
    ch: &mut Checker,
    module_id: &ModuleID,
    module_file_name: &str,
    file: Node,
) -> Option<Rc<Export>> {
    let mut exports: Vec<Rc<Export>> = Vec::new();
    let name = ch.sym(symbol).name.to_string();
    let mut extractor = new_symbol_extractor("", ch, None, None);
    extractor.extract_from_symbol(
        &name,
        symbol,
        module_id,
        module_file_name,
        file,
        &mut exports,
    );
    // Go: core.FirstOrNil(exports)
    exports.first().cloned()
}

// ---------------------------------------------------------------------------
// export_stringer_generated.go
// ---------------------------------------------------------------------------

// Go: ls/autoimport/export_stringer_generated.go:23 _ExportSyntax_name
const EXPORT_SYNTAX_NAME: &str = "ExportSyntaxNoneExportSyntaxModifierExportSyntaxNamedExportSyntaxDefaultModifierExportSyntaxDefaultDeclarationExportSyntaxEqualsExportSyntaxUMDExportSyntaxStarExportSyntaxCommonJSModuleExportsExportSyntaxCommonJSExportsProperty";

// Go: ls/autoimport/export_stringer_generated.go:25 _ExportSyntax_index
const EXPORT_SYNTAX_INDEX: [u8; 11] = [0, 16, 36, 53, 80, 110, 128, 143, 159, 192, 227];

impl ExportSyntax {
    // Go: ls/autoimport/export_stringer_generated.go:27 String
    pub fn string(self) -> String {
        let idx = self.0 - 0;
        if self.0 < 0 || idx as usize >= EXPORT_SYNTAX_INDEX.len() - 1 {
            return format!("ExportSyntax({})", self.0);
        }
        let idx = idx as usize;
        EXPORT_SYNTAX_NAME[EXPORT_SYNTAX_INDEX[idx] as usize..EXPORT_SYNTAX_INDEX[idx + 1] as usize]
            .to_string()
    }
}

impl std::fmt::Display for ExportSyntax {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.string())
    }
}

// Go: ls/autoimport/export_test.go (ts#64159). A lib test: the Go test reads
// the package's unexported constructors.
#[cfg(test)]
mod export_tests {
    use super::*;

    // Go: export_test.go:10 TestModuleIDVariants
    #[test]
    fn module_id_variants() {
        let zero = ModuleID::default();
        assert!(zero.as_path_key().is_none());
        assert_eq!(zero.as_string(), "");
        assert!(!zero.is_ambient());

        let path = tspath::Path("/project/src/a.ts".to_string());
        let file = file_module_id(path.clone());
        assert_eq!(file.as_path_key(), Some(&path));
        assert_eq!(file.as_string(), path.0);
        assert!(!file.is_ambient());

        let ambient = ambient_module_id("node:fs");
        assert!(ambient.as_path_key().is_none());
        assert_eq!(ambient.as_string(), "node:fs");
        assert!(ambient.is_ambient());
    }
}
