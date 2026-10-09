use crate::ls::autoimport::prelude::*;

// Port of Go `ls/autoimport/extract.go`.
//
// PORT: Go keeps `checker *checker.Checker` in the extractors. A Rust
// struct holds it as `&'c mut Checker` for the extractor's life (an
// extractor lives for one file or one symbol). Symbols are read from that
// checker's arena (`checker.symbols`), which holds every bound symbol plus
// the checker's own symbols, as the Go pointers do.
// PORT: Go map iteration over symbol tables (`file.Symbol.Exports`,
// `target.Exports`, ...) has random order; the port uses table insertion
// order. Export order only decides the order of index entries (ties in the
// completion sort).

use crate::frontend::module;
use crate::frontend::tspath;
use crate::ls::lsutil;
use std::cell::Cell;

// Go: ls/autoimport/extract.go:16 symbolExtractor
pub struct SymbolExtractor<'c> {
    pub package_name: String,
    pub stats: Rc<ExtractorStats>,

    pub local_name_resolver: NameResolver,
    pub checker: &'c mut Checker,
    pub to_path: Option<Rc<dyn Fn(&str) -> tspath::Path>>,
    // realpath, if set, is used to resolve symlinks for ModuleID generation.
    // This ensures that symlinked packages use their realpath as ModuleID,
    // deduplicating exports from files that appear via multiple symlink paths.
    pub realpath: Option<Rc<dyn Fn(&str) -> String>>,
}

// Go: ls/autoimport/extract.go:29 exportExtractor
// PORT: Go embeds `*symbolExtractor`; it is the nested `symbol_extractor`
// field, and `Deref` promotes its fields and methods as Go does.
pub struct ExportExtractor<'c> {
    pub symbol_extractor: SymbolExtractor<'c>,
    pub module_resolver: Rc<module::DefaultResolver>,
}

impl<'c> std::ops::Deref for ExportExtractor<'c> {
    type Target = SymbolExtractor<'c>;
    fn deref(&self) -> &SymbolExtractor<'c> {
        &self.symbol_extractor
    }
}

impl<'c> std::ops::DerefMut for ExportExtractor<'c> {
    fn deref_mut(&mut self) -> &mut SymbolExtractor<'c> {
        &mut self.symbol_extractor
    }
}

// Go: ls/autoimport/extract.go:34 extractorStats
// PORT: Go `atomic.Int32` counters are `Cell<i32>` (one thread).
#[derive(Debug, Default)]
pub struct ExtractorStats {
    pub exports: Cell<i32>,
    pub used_checker: Cell<i32>,
}

impl ExportExtractor<'_> {
    // Go: ls/autoimport/extract.go:39 Stats
    pub fn stats(&self) -> Rc<ExtractorStats> {
        self.symbol_extractor.stats.clone()
    }
}

// Go: ls/autoimport/extract.go:43 checkerLease
// PORT: Go keeps a second pointer to the extractor's checker. Rust cannot
// hold a second `&mut Checker`, so the lease keeps only `used`, and its
// methods take the extractor's checker and give it back.
#[derive(Clone, Copy, Debug, Default)]
pub struct CheckerLease {
    pub used: bool,
}

impl CheckerLease {
    // Go: ls/autoimport/extract.go:48 GetChecker
    pub fn get_checker<'a>(&mut self, checker: &'a mut Checker) -> &'a mut Checker {
        self.used = true;
        checker
    }

    // Go: ls/autoimport/extract.go:53 TryChecker
    pub fn try_checker<'a>(&self, checker: &'a mut Checker) -> Option<&'a mut Checker> {
        if self.used {
            return Some(checker);
        }
        None
    }

    /// Go `TryChecker()` as the checker argument of `lsutil` symbol display
    /// functions.
    // PORT: with a nil checker, Go still reads symbol fields through the
    // pointer. The nil case carries the extractor checker's arena, which holds
    // checker symbols (for example the merge of a module augmentation) that
    // the program's binder arena does not hold.
    pub fn try_type_checker<'a>(&self, checker: &'a mut Checker) -> lsutil::TypeChecker<'a> {
        if self.used {
            return lsutil::TypeChecker::Checker(checker);
        }
        lsutil::TypeChecker::Nil(&checker.symbols)
    }
}

// Go: ls/autoimport/extract.go:60 newSymbolExtractor
// PORT: Go nil funcs are `None`. Go `&binder.NameResolver{CompilerOptions:
// core.EmptyCompilerOptions}` leaves every callback nil. ts#64159 (behavior
// only) takes a `caseSensitivity` for `toPath`; Go N' `getModuleID` needs only
// `realpath` (extract.go:82), and every Rust caller with a `realpath` passes
// `to_path` too.
pub fn new_symbol_extractor<'c>(
    package_name: &str,
    checker: &'c mut Checker,
    to_path: Option<Rc<dyn Fn(&str) -> tspath::Path>>,
    realpath: Option<Rc<dyn Fn(&str) -> String>>,
) -> SymbolExtractor<'c> {
    SymbolExtractor {
        package_name: package_name.to_string(),
        checker,
        local_name_resolver: NameResolver {
            compiler_options: empty_compiler_options(),
            get_symbol_of_declaration: None,
            error: None,
            globals: SymbolTable::NIL,
            arguments_symbol: Cell::new(SymbolId::NIL),
            require_symbol: SymbolId::NIL,
            lookup: None,
            symbol_referenced: None,
            set_requires_scope_change_cache: None,
            get_requires_scope_change_cache: None,
            on_property_with_invalid_initializer: None,
            on_failed_to_resolve_symbol: None,
            on_successfully_resolved_symbol: None,
        },
        stats: Rc::new(ExtractorStats::default()),
        to_path,
        realpath,
    }
}

impl RegistryBuilder {
    // Go: ls/autoimport/extract.go:73 newExportExtractor
    pub fn new_export_extractor<'c>(
        &self,
        package_name: &str,
        checker: &'c mut Checker,
        module_resolver: Rc<module::DefaultResolver>,
        realpath: Option<Rc<dyn Fn(&str) -> String>>,
    ) -> ExportExtractor<'c> {
        ExportExtractor {
            symbol_extractor: new_symbol_extractor(
                package_name,
                checker,
                Some(self.base.to_path.clone()),
                realpath,
            ),
            module_resolver,
        }
    }
}

impl SymbolExtractor<'_> {
    // Go: ls/autoimport/extract.go:81 getModuleID
    // getModuleID returns the ModuleID for a file, using realpath if available.
    pub fn get_module_id(&self, file: Node) -> ModuleID {
        if let (Some(realpath), Some(to_path)) = (&self.realpath, &self.to_path) {
            let realpath = realpath(source_file_file_name(file));
            return file_module_id(to_path(&realpath));
        }
        file_module_id(tspath::Path(source_file_info(file).path.clone()))
    }

    // Go: ls/autoimport/extract.go:91 getModuleIDForSymbol
    // getModuleIDForSymbol returns the ModuleID for a module symbol, using realpath
    // normalization when available for source files.
    pub fn get_module_id_for_symbol(&self, symbol: SymbolId) -> (ModuleID, bool) {
        let (module_id, file_name, ok) =
            try_get_module_id_and_file_name_of_module_symbol(&self.checker.symbols, symbol);
        if !ok {
            return (ModuleID::default(), false);
        }
        // If fileName is set, this is a source file that may need realpath normalization
        if !file_name.is_empty() && self.realpath.is_some() {
            let decl = get_non_augmentation_declaration(&self.checker.symbols, symbol);
            if decl.is_some() && decl.kind() == SyntaxKind::SourceFile {
                return (self.get_module_id(decl), true);
            }
        }
        (module_id, true)
    }
}

impl ExportExtractor<'_> {
    // Go: ls/autoimport/extract.go:106 extractFromFile
    // PORT: Go nil results are an empty `Vec`.
    pub fn extract_from_file(&mut self, file: Node) -> Vec<Rc<Export>> {
        if file.symbol().is_some() {
            return self.extract_from_module(file);
        }
        if !source_file_info(file).ambient_module_names.is_empty() {
            let mut export_count: usize = 0;
            for statement in file.statements().iter() {
                if is_module_with_string_literal_name(statement)
                    && is_non_pattern_ambient_module_declaration(file, statement)
                {
                    let decl_exports = self.checker.sym(statement.symbol()).exports;
                    export_count += self.checker.symbols.len(decl_exports);
                }
            }
            let mut exports: Vec<Rc<Export>> = Vec::with_capacity(export_count);
            for statement in file.statements().iter() {
                if is_module_with_string_literal_name(statement)
                    && is_non_pattern_ambient_module_declaration(file, statement)
                {
                    let module_id = ambient_module_id(statement.name().text());
                    self.extract_from_module_declaration(
                        statement,
                        file,
                        &module_id,
                        "",
                        &mut exports,
                    );
                }
            }
            return exports;
        }
        Vec::new()
    }
}

// Go: ls/autoimport/extract.go:128 isNonPatternAmbientModuleDeclaration
// Reports whether `decl` is not a pattern ambient module (`declare module
// "*.css"`). Auto-import skips pattern modules: no import can name them.
fn is_non_pattern_ambient_module_declaration(file: Node, decl: Node) -> bool {
    let decl_symbol = decl.symbol();
    !file_bind_data(file)
        .pattern_ambient_modules
        .iter()
        .any(|module| module.symbol == decl_symbol)
}

impl ExportExtractor<'_> {
    // Go: ls/autoimport/extract.go:137 extractFromModule
    pub fn extract_from_module(&mut self, file: Node) -> Vec<Rc<Export>> {
        let module_augmentations: Vec<Node> = crate::frontend::core_ls_ext::map_non_nil(
            source_file_info(file).module_augmentations.iter().copied(),
            |name: Node| -> Option<Node> {
                let decl = name.parent();
                if is_global_scope_augmentation(decl) {
                    return None;
                }
                Some(decl)
            },
        );
        let mut augmentation_export_count: usize = 0;
        for decl in &module_augmentations {
            let decl_exports = self.checker.sym(decl.symbol()).exports;
            augmentation_export_count += self.checker.symbols.len(decl_exports);
        }
        let module_id = self.get_module_id(file);
        let file_exports_table = self.checker.sym(file.symbol()).exports;
        let file_exports = self.checker.symbols.entries(file_exports_table);
        let mut exports: Vec<Rc<Export>> =
            Vec::with_capacity(file_exports.len() + augmentation_export_count);
        for (name, symbol) in file_exports {
            self.extract_from_symbol(
                &name,
                symbol,
                &module_id,
                source_file_file_name(file),
                file,
                &mut exports,
            );
        }
        for decl in module_augmentations {
            let name = decl.name().text().to_string();
            let mut module_id = ambient_module_id(&name);
            let mut module_file_name = String::new();
            let mut unresolved_module_specifier = String::new();
            if tspath::is_external_module_name_relative(&name) {
                let (resolved, _, _) = self.module_resolver.resolve_module_name(
                    &name,
                    source_file_file_name(file),
                    ModuleKind::COMMON_JS,
                    None,
                );
                let to_path = self
                    .to_path
                    .clone()
                    .unwrap_or_else(|| crate::core::go_nil_dereference());
                if resolved.is_resolved() {
                    module_file_name = resolved.resolved_file_name.clone();
                    module_id = file_module_id(to_path(&module_file_name));
                } else {
                    // PORT: Go N' `file.FileName().Directory().ResolveFile(name)`
                    // (ts#64159) gives the same name for a module name without a
                    // trailing separator.
                    module_file_name = tspath::resolve_path(
                        &tspath::get_directory_path(source_file_file_name(file)),
                        &[name.as_str()],
                    );
                    module_id = file_module_id(to_path(&module_file_name));
                    // ts#64159: extract.go:166
                    unresolved_module_specifier = name.clone();
                }
            }
            let export_start = exports.len();
            self.extract_from_module_declaration(
                decl,
                file,
                &module_id,
                &module_file_name,
                &mut exports,
            );
            // ts#64159: extract.go:169-173
            // PORT: the new exports are not shared yet, so `make_mut` does not copy.
            for export in &mut exports[export_start..] {
                Rc::make_mut(export).unresolved_module_specifier =
                    unresolved_module_specifier.clone();
            }
        }
        exports
    }

    // Go: ls/autoimport/extract.go:173 extractFromModuleDeclaration
    pub fn extract_from_module_declaration(
        &mut self,
        decl: Node,
        file: Node,
        module_id: &ModuleID,
        module_file_name: &str,
        exports: &mut Vec<Rc<Export>>,
    ) {
        let decl_exports_table = self.checker.sym(decl.symbol()).exports;
        for (name, symbol) in self.checker.symbols.entries(decl_exports_table) {
            self.extract_from_symbol(&name, symbol, module_id, module_file_name, file, exports);
        }
    }
}

impl SymbolExtractor<'_> {
    // Go: ls/autoimport/extract.go:179 extractFromSymbol
    pub fn extract_from_symbol(
        &mut self,
        name: &str,
        symbol: SymbolId,
        module_id: &ModuleID,
        module_file_name: &str,
        file: Node,
        exports: &mut Vec<Rc<Export>>,
    ) {
        if should_ignore_symbol(&self.checker.symbols, symbol) {
            return;
        }

        if name == INTERNAL_SYMBOL_NAME_EXPORT_STAR {
            let mut checker_lease = CheckerLease { used: false };
            let symbol_parent = self.checker.sym(symbol).parent;
            let mut all_exports = self.checker.get_exports_of_module_exported(symbol_parent);
            // allExports includes named exports from the file that will be processed separately;
            // we want to add only the ones that come from the star
            let parent_exports_table = self.checker.sym(symbol_parent).exports;
            for (inner_name, named_export) in self.checker.symbols.entries(parent_exports_table) {
                if &*inner_name != INTERNAL_SYMBOL_NAME_EXPORT_STAR {
                    // Go: slices.Index(allExports, namedExport)
                    let idx: i32 = all_exports
                        .iter()
                        .position(|s| *s == named_export)
                        .map_or(-1, |i| i as i32);
                    if idx >= 0 || should_ignore_symbol(&self.checker.symbols, named_export) {
                        // Go: slices.Delete(allExports, idx, idx+1) panics on idx -1.
                        // Its bound check is the 3-index `s[i:j:len(s)]`, so the
                        // runtime text is `[-1::]`.
                        if idx < 0 {
                            crate::core::go_panic(
                                "runtime error: slice bounds out of range [-1::]".to_string(),
                            );
                        }
                        all_exports.remove(idx as usize);
                    }
                }
            }

            exports.reserve(all_exports.len());
            for reexported_symbol in all_exports {
                let (export, _) = self.create_export(
                    reexported_symbol,
                    module_id,
                    module_file_name,
                    ExportSyntax::STAR,
                    file,
                    &mut checker_lease,
                );
                if let Some(mut export) = export {
                    let parent = {
                        let checker = checker_lease.get_checker(&mut *self.checker);
                        let reexported_parent = checker.sym(reexported_symbol).parent;
                        checker.get_merged_symbol_exported(reexported_parent)
                    };
                    if parent.is_some() && self.checker.sym(parent).is_external_module() {
                        let (target_module_id, ok) = self.get_module_id_for_symbol(parent);
                        if ok {
                            export.target = ExportID {
                                export_name: self.checker.sym(reexported_symbol).name.to_string(),
                                module_id: target_module_id,
                            };
                        }
                    }
                    export.through = INTERNAL_SYMBOL_NAME_EXPORT_STAR.to_string();
                    exports.push(Rc::new(export));
                }
            }
            return;
        }

        let syntax = get_syntax(&self.checker.symbols, symbol);
        let mut checker_lease = CheckerLease { used: false };
        let (export, target) = self.create_export(
            symbol,
            module_id,
            module_file_name,
            syntax,
            file,
            &mut checker_lease,
        );
        let Some(export) = export else {
            return;
        };

        exports.push(Rc::new(export));

        if target.is_some() {
            if syntax == ExportSyntax::EQUALS
                && self
                    .checker
                    .sym(target)
                    .flags
                    .intersects(SymbolFlags::NAMESPACE)
            {
                let target_exports_table = self.checker.sym(target).exports;
                let target_exports = self.checker.symbols.entries(target_exports_table);
                exports.reserve(target_exports.len());
                for (inner_name, named_export) in target_exports {
                    if &*inner_name != INTERNAL_SYMBOL_NAME_EXPORT_STAR {
                        let (export, _) = self.create_export(
                            named_export,
                            module_id,
                            module_file_name,
                            syntax,
                            file,
                            &mut checker_lease,
                        );
                        if let Some(mut export) = export {
                            export.through = name.to_string();
                            exports.push(Rc::new(export));
                        }
                    }
                }
            }
        } else if syntax == ExportSyntax::COMMON_JS_MODULE_EXPORTS {
            let expression = self.checker.sym(symbol).declarations[0].right();
            if expression.kind() == SyntaxKind::ObjectLiteralExpression {
                // what is actually desirable here? I think it would be reasonable to only treat these as exports
                // if *every* property is a shorthand property or identifier: identifier
                // At least, it would be sketchy if there were any methods, computed properties...
                exports.reserve(expression.properties().len());
                for prop in expression.properties().iter() {
                    if is_shorthand_property_assignment(prop)
                        || is_property_assignment(prop)
                            && prop.name().kind() == SyntaxKind::Identifier
                    {
                        let members = self.checker.sym(expression.symbol()).members;
                        let member = self.checker.symbols.get(members, prop.name().text());
                        let (export, _) = self.create_export(
                            member,
                            module_id,
                            module_file_name,
                            syntax,
                            file,
                            &mut checker_lease,
                        );
                        if let Some(mut export) = export {
                            export.through = name.to_string();
                            exports.push(Rc::new(export));
                        }
                    }
                }
            }
        }
    }

    // Go: ls/autoimport/extract.go:261 createExport
    // createExport creates an Export for the given symbol, returning the Export and the target symbol if the export is an alias.
    // PORT: Go returns `*Export`; the port returns the owned value (`None` is
    // nil) so the caller can finish it (`Target`, `through`) before sharing it
    // as `Rc<Export>`.
    pub fn create_export(
        &mut self,
        symbol: SymbolId,
        module_id: &ModuleID,
        module_file_name: &str,
        syntax: ExportSyntax,
        file: Node,
        checker_lease: &mut CheckerLease,
    ) -> (Option<Export>, SymbolId) {
        if should_ignore_symbol(&self.checker.symbols, symbol) {
            return (None, SymbolId::NIL);
        }

        let mut export = Export {
            export_id: ExportID {
                export_name: self.checker.sym(symbol).name.to_string(),
                module_id: module_id.clone(),
            },
            module_file_name: module_file_name.to_string(),
            syntax,
            flags: self
                .checker
                .sym(symbol)
                .combined_local_and_export_symbol_flags(&self.checker.symbols),
            path: tspath::Path(source_file_info(file).path.clone()),
            package_name: self.package_name.clone(),
            ..Default::default()
        };

        if syntax == ExportSyntax::UMD {
            export.export_id.export_name = INTERNAL_SYMBOL_NAME_EXPORT_EQUALS.to_string();
            export.local_name = self.checker.sym(symbol).name.to_string();
        }

        let mut target_symbol = SymbolId::NIL;
        if self
            .checker
            .sym(symbol)
            .flags
            .intersects(SymbolFlags::ALIAS)
        {
            target_symbol = self.try_resolve_symbol(symbol, syntax, checker_lease);
            if target_symbol.is_some() {
                let mut decl = Node::NIL;
                if !self.checker.sym(target_symbol).declarations.is_empty() {
                    decl = self.checker.sym(target_symbol).declarations[0];
                } else if self
                    .checker
                    .sym(target_symbol)
                    .check_flags
                    .intersects(CheckFlags::MAPPED)
                {
                    let mapped_decl = checker_lease
                        .get_checker(&mut *self.checker)
                        .get_mapped_type_symbol_of_property(target_symbol);
                    if mapped_decl.is_some()
                        && !self.checker.sym(mapped_decl).declarations.is_empty()
                    {
                        decl = self.checker.sym(mapped_decl).declarations[0];
                    }
                }
                if decl.is_nil() {
                    // !!! consider GetImmediateAliasedSymbol to go as far as we can
                    decl = self.checker.sym(symbol).declarations[0];
                }
                if decl.is_nil() {
                    crate::core::go_panic("no declaration for aliased symbol".to_string());
                }

                let mut parent = self.checker.sym(target_symbol).parent;
                if let Some(checker) = checker_lease.try_checker(&mut *self.checker) {
                    export.flags = checker.get_symbol_flags_exported(target_symbol);
                    export.is_type_only = checker
                        .get_type_only_alias_declaration_exported(symbol)
                        .is_some();
                    parent = checker.get_merged_symbol_exported(parent);
                } else {
                    export.flags = self.checker.sym(target_symbol).flags;
                    // Go: core.Some(symbol.Declarations, ast.IsPartOfTypeOnlyImportOrExportDeclaration)
                    export.is_type_only = self
                        .checker
                        .sym(symbol)
                        .declarations
                        .iter()
                        .any(|d| is_part_of_type_only_import_or_export_declaration(*d));
                }
                export.script_element_kind = lsutil::get_symbol_kind(
                    checker_lease.try_type_checker(&mut *self.checker),
                    target_symbol,
                    decl,
                );
                export.script_element_kind_modifiers = lsutil::get_symbol_modifiers(
                    checker_lease.try_type_checker(&mut *self.checker),
                    target_symbol,
                );
                let mut target_module_id = file_module_id(tspath::Path(
                    source_file_info(get_source_file_of_node(decl)).path.clone(),
                ));
                if parent.is_some() && self.checker.sym(parent).is_external_module() {
                    let (id, ok) = self.get_module_id_for_symbol(parent);
                    if ok {
                        target_module_id = id;
                    }
                }
                export.target = ExportID {
                    export_name: self.checker.sym(target_symbol).name.to_string(),
                    module_id: target_module_id,
                };
            }
        } else {
            let first_declaration = self.checker.sym(symbol).declarations[0];
            export.script_element_kind = lsutil::get_symbol_kind(
                checker_lease.try_type_checker(&mut *self.checker),
                symbol,
                first_declaration,
            );
            export.script_element_kind_modifiers = lsutil::get_symbol_modifiers(
                checker_lease.try_type_checker(&mut *self.checker),
                symbol,
            );
        }

        let symbol_name = self.checker.sym(symbol).name.to_string();
        if symbol_name == INTERNAL_SYMBOL_NAME_DEFAULT
            || symbol_name == INTERNAL_SYMBOL_NAME_EXPORT_EQUALS
        {
            let mut named_symbol = symbol;
            let s = get_local_symbol_for_export_default(&self.checker.symbols, symbol);
            if s.is_some() {
                named_symbol = s;
            }
            export.local_name =
                get_default_like_export_name_from_declaration(&self.checker.symbols, named_symbol);
            if is_unusable_name(&export.local_name) {
                export.local_name = export.target.export_name.clone();
            }
            if is_unusable_name(&export.local_name) {
                if target_symbol.is_some() {
                    named_symbol = target_symbol;
                    let s =
                        get_local_symbol_for_export_default(&self.checker.symbols, target_symbol);
                    if s.is_some() {
                        named_symbol = s;
                    }
                    export.local_name = get_default_like_export_name_from_declaration(
                        &self.checker.symbols,
                        named_symbol,
                    );
                }
            }
            if is_unusable_name(&export.local_name) {
                // Last resort: derive identifier from the file name. Use FileName() (original
                // casing) rather than ModuleID/Path() which is lowercased on case-insensitive
                // file systems, losing PascalCase.
                export.local_name = lsutil::module_specifier_to_valid_identifier(
                    &file_name_for_default_export_name(
                        &self.checker.symbols,
                        target_symbol,
                        module_file_name,
                        module_id,
                    ),
                    false,
                );
            }
        }

        if is_unusable_name(&export.name()) {
            return (None, SymbolId::NIL);
        }

        self.stats.exports.set(self.stats.exports.get() + 1);
        if checker_lease.try_checker(&mut *self.checker).is_some() {
            self.stats
                .used_checker
                .set(self.stats.used_checker.get() + 1);
        }

        (Some(export), target_symbol)
    }

    // Go: ls/autoimport/extract.go:366 tryResolveSymbol
    pub fn try_resolve_symbol(
        &mut self,
        symbol: SymbolId,
        syntax: ExportSyntax,
        checker_lease: &mut CheckerLease,
    ) -> SymbolId {
        if !is_non_local_alias(&self.checker.symbols, symbol, SymbolFlags::NONE) {
            return symbol;
        }

        let mut loc = Node::NIL;
        let mut name = String::new();
        // PORT: the Go switch with `break` and `fallthrough`; `default_declaration`
        // marks the shared `ExportSyntaxDefaultDeclaration` body.
        let mut default_declaration = false;
        match syntax {
            ExportSyntax::NAMED => {
                let decl = get_declaration_of_kind(
                    &self.checker.symbols,
                    symbol,
                    SyntaxKind::ExportSpecifier,
                );
                if decl.parent().parent().module_specifier().is_nil() {
                    let n = crate::frontend::core_ls_ext::first_non_zero([
                        decl.name(),
                        decl.property_name(),
                    ]);
                    if n.kind() == SyntaxKind::Identifier {
                        loc = n;
                        name = n.text().to_string();
                    }
                }
            }
            // !!! check if module.exports = foo is marked as an alias
            ExportSyntax::EQUALS => {
                if self.checker.sym(symbol).name.as_str() == INTERNAL_SYMBOL_NAME_EXPORT_EQUALS {
                    // Go: fallthrough
                    default_declaration = true;
                }
                // Go: break
            }
            ExportSyntax::DEFAULT_DECLARATION => {
                default_declaration = true;
            }
            _ => {}
        }
        if default_declaration {
            let decl = get_declaration_of_kind(
                &self.checker.symbols,
                symbol,
                SyntaxKind::ExportAssignment,
            );
            if decl.expression().kind() == SyntaxKind::Identifier {
                loc = decl.expression();
                name = loc.text().to_string();
            }
        }

        if loc.is_some() {
            let local = self.local_name_resolver.resolve(
                &mut *self.checker,
                loc,
                &name,
                SymbolFlags::ALL,
                None,
                false,
                false,
            );
            if local.is_some()
                && !is_non_local_alias(&self.checker.symbols, local, SymbolFlags::NONE)
            {
                return local;
            }
        }

        let checker = checker_lease.get_checker(&mut *self.checker);
        let resolved = checker.get_aliased_symbol(symbol);
        if !checker.is_unknown_symbol(resolved) {
            return resolved;
        }
        SymbolId::NIL
    }
}

// Go: ls/autoimport/extract.go:410 shouldIgnoreSymbol
// PORT: the arena parameter as in `try_get_module_id_and_file_name_of_module_symbol`.
pub fn should_ignore_symbol(symbols: &SymbolArena, symbol: SymbolId) -> bool {
    if symbols.sym(symbol).flags.intersects(SymbolFlags::PROTOTYPE) {
        return true;
    }
    false
}

// Go: ls/autoimport/extract.go:417 getSyntax
// PORT: the arena parameter as in `try_get_module_id_and_file_name_of_module_symbol`.
pub fn get_syntax(symbols: &SymbolArena, symbol: SymbolId) -> ExportSyntax {
    for &decl in symbols.sym(symbol).declarations.iter() {
        match decl.kind() {
            SyntaxKind::ExportSpecifier => {
                return ExportSyntax::NAMED;
            }
            SyntaxKind::ExportAssignment => {
                return if decl.is_export_equals() {
                    ExportSyntax::EQUALS
                } else {
                    ExportSyntax::DEFAULT_DECLARATION
                };
            }
            SyntaxKind::NamespaceExportDeclaration => {
                return ExportSyntax::UMD;
            }
            SyntaxKind::BinaryExpression => match get_assignment_declaration_kind(decl) {
                JSDeclarationKind::MODULE_EXPORTS => {
                    return ExportSyntax::COMMON_JS_MODULE_EXPORTS;
                }
                JSDeclarationKind::EXPORTS_PROPERTY => {
                    return ExportSyntax::COMMON_JS_EXPORTS_PROPERTY;
                }
                _ => {}
            },
            _ => {
                if get_combined_modifier_flags(decl).intersects(ModifierFlags::DEFAULT) {
                    return ExportSyntax::DEFAULT_MODIFIER;
                } else {
                    return ExportSyntax::MODIFIER;
                }
            }
        }
    }
    ExportSyntax::NONE
}

// Go: ls/autoimport/extract.go:448 isUnusableName
pub fn is_unusable_name(name: &str) -> bool {
    name.is_empty()
        || name == "_default"
        || name == INTERNAL_SYMBOL_NAME_EXPORT_STAR
        || name == INTERNAL_SYMBOL_NAME_DEFAULT
        || name == INTERNAL_SYMBOL_NAME_EXPORT_EQUALS
}

// Go: ls/autoimport/extract.go:461 fileNameForDefaultExportName
// fileNameForDefaultExportName returns the best file name to use when deriving
// a fallback identifier for a default-like export. It prefers the target symbol's
// source file (closest to the export origin), falls back to the module's original
// file name, and uses the lowercased moduleID only for ambient modules where no
// original file name is available.
// PORT: the arena parameter as in `try_get_module_id_and_file_name_of_module_symbol`.
pub fn file_name_for_default_export_name(
    symbols: &SymbolArena,
    target_symbol: SymbolId,
    module_file_name: &str,
    module_id: &ModuleID,
) -> String {
    if target_symbol.is_some() && !symbols.sym(target_symbol).declarations.is_empty() {
        let fn_ = source_file_file_name(get_source_file_of_node(
            symbols.sym(target_symbol).declarations[0],
        ));
        if !fn_.is_empty() {
            return fn_.to_string();
        }
    }
    if !module_file_name.is_empty() {
        return module_file_name.to_string();
    }
    module_id.as_string().to_string()
}
