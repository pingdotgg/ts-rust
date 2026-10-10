use crate::ls::autoimport::prelude::*;

// Port of Go `ls/autoimport/aliasresolver.go`.
//
// PORT: Go `aliasResolver` implements `checker.Program`, so the registry can
// make a checker (`checker.NewChecker(aliasResolver, nil)`) over node_modules
// entrypoint files that are not in any program. A ts_goport checker reads
// its program version (`prog()`), so `new_checker` makes an alias resolver
// program (`program::new_alias_resolver_program`). The `checker.Program`
// methods are inherent methods with the Go logic. The `program.rs`
// functions call the lazy ones through `program::AliasResolverProgram`, and
// give Go's constant or panic for the others.
// PORT: the registry host parses the files outside any program. Each file
// is published with no program and bound into the binder lineage, as
// sourcedefinition does (`bind_alias_resolver_source_file`).

use crate::frontend::core_ext::HasFileName;
use crate::frontend::module;
use crate::frontend::module::ResolutionHost as _;
use crate::frontend::packagejson;
use crate::frontend::tsoptions;
use crate::frontend::tspath;
use crate::frontend::vfs::Fs as _;
use crate::modulespecifiers::symlinks::KnownSymlinks;
use crate::program::{AliasResolverProgram, AliasResolverProgramScope};
use std::cell::Cell;
use std::sync::Arc;

// Go: ls/autoimport/aliasresolver.go:16 pathAndFileName
#[derive(Clone, Debug, Default)]
pub struct PathAndFileName {
    pub path: tspath::Path,
    pub file_name: String,
}

// Go: ls/autoimport/aliasresolver.go:21 aliasResolver
// PORT: Go `collections.SyncMap` values are `RefCell<FxHashMap>` (one
// thread). A Go nil `symlinks` map is an empty map. The fields after
// `resolved_modules` are the port's (see `new_checker` and
// `new_narrow_checker`).
pub struct AliasResolver {
    pub to_path: Rc<dyn Fn(&str) -> tspath::Path>,
    pub host: Rc<dyn RegistryCloneHost>,
    pub module_resolver: Rc<module::DefaultResolver>,

    pub root_files: Vec<Node>,
    // symlinks maps from realpath to symlinked path and file name
    pub symlinks: FxHashMap<tspath::Path, PathAndFileName>,
    pub on_failed_ambient_module_lookup: Rc<dyn Fn(&dyn HasFileName, &str)>,
    pub resolved_modules: RefCell<
        FxHashMap<
            tspath::Path,
            Rc<RefCell<FxHashMap<module::ModeAwareCacheKey, Arc<ResolvedModule>>>>,
        >,
    >,
    /// The resolutions that `new_checker` made before the checker, by file
    /// path (`prefetch_resolved_module`).
    pub prefetched_modules:
        RefCell<FxHashMap<tspath::Path, FxHashMap<module::ModeAwareCacheKey, Arc<ResolvedModule>>>>,
    /// The program of the checker, once `new_checker` made it.
    pub checker_program: Cell<Option<&'static GoProgram>>,
    /// The ids of the files that the walk of `new_narrow_checker` read. None
    /// for a full walk.
    pub narrow_files: RefCell<Option<FxHashSet<usize>>>,
    /// Set when the checker of a narrow walk asked for a file outside it
    /// (`get_source_file`). The caller then drops the checker and its
    /// results, and makes a checker with the full walk.
    pub missed: Cell<bool>,
}

// Go: ls/autoimport/aliasresolver.go:33 newAliasResolver
pub fn new_alias_resolver(
    root_files: Vec<Node>,
    symlinks: FxHashMap<tspath::Path, PathAndFileName>,
    host: Rc<dyn RegistryCloneHost>,
    module_resolver: Rc<module::DefaultResolver>,
    to_path: Rc<dyn Fn(&str) -> tspath::Path>,
    on_failed_ambient_module_lookup: Rc<dyn Fn(&dyn HasFileName, &str)>,
) -> Rc<AliasResolver> {
    let r = Rc::new(AliasResolver {
        to_path,
        host,
        module_resolver,
        root_files,
        symlinks,
        on_failed_ambient_module_lookup,
        resolved_modules: RefCell::new(FxHashMap::default()),
        prefetched_modules: RefCell::new(FxHashMap::default()),
        checker_program: Cell::new(None),
        narrow_files: RefCell::new(None),
        missed: Cell::new(false),
    });
    r
}

/// Go `binder.BindSourceFile(file)` on a file that the registry host parsed
/// for the alias resolver. `current_directory` is the host's.
// PORT: the file is in no program. It is published with no program
// (`program::publish_parsed_files`) and bound into the binder lineage
// (`program::bind_file_outside_program`, Go `BindOnce`).
pub fn bind_alias_resolver_source_file(current_directory: &str, file: Node) {
    crate::program::publish_parsed_files(current_directory);
    crate::program::bind_file_outside_program(file);
}

/// The module names that a checker of an alias resolver can resolve from
/// `file` (Go `GetResolvedModule` in checker `resolveExternalModule`): every
/// string module specifier of an import, export, import-equals or JSDoc
/// import at any module declaration depth, the import calls, `require`
/// calls and import types, and the string literal module augmentations.
// PORT: a syntactic walk shaped like Go parser/references.go
// collectExternalModuleReferences. That walk makes `file.imports`, and it
// skips relative names inside ambient modules and the bodies of
// augmentations and namespaces. The checker still resolves those names
// (`declare module "pkg" { export { foo } from "./impl"; }`), so this walk
// keeps them. The import calls, `require` calls and import types come from
// `file.imports`: the parser found them with
// `ForEachDynamicImportOrRequireCall` (references.go:16), which reads the
// whole file text, ambient module bodies too. A name found twice is resolved
// once (`prefetch_resolved_module`).
fn checker_module_references(file: Node) -> Vec<String> {
    let info = source_file_info(file);
    let mut names: Vec<String> = Vec::new();
    for node in file.statements().iter() {
        collect_checker_module_references(node, false, &mut names);
    }
    names.extend(info.imports.iter().map(|name| name.text().to_string()));
    // Go: checker mergeModuleAugmentation resolves each augmentation name.
    names.extend(
        info.module_augmentations
            .iter()
            .filter(|name| is_string_literal(**name))
            .map(|name| name.text().to_string()),
    );
    names
}

/// The module names that the walk of `new_narrow_checker` follows from
/// `file`: every name of `checker_module_references` for a declaration
/// file. For any other file only the re-exports (`export ... from`) and
/// the string literal module augmentations: the imports, `require` calls
/// and import calls of JS and TS sources reach their whole implementation
/// (about 870 files of jsdom, css-tree and cssstyle in Query core), and the
/// export extraction rarely asks for them.
fn narrow_module_references(file: Node) -> Vec<String> {
    let info = source_file_info(file);
    if info.is_declaration_file {
        return checker_module_references(file);
    }
    let mut names: Vec<String> = Vec::new();
    for node in file.statements().iter() {
        collect_checker_module_references(node, true, &mut names);
    }
    names.extend(
        info.module_augmentations
            .iter()
            .filter(|name| is_string_literal(**name))
            .map(|name| name.text().to_string()),
    );
    names
}

// Go: parser/references.go:24 collectModuleReferences
// PORT: without the ambient module conditions (see
// `checker_module_references`). Go checker `resolveExternalModuleNameWorker`
// takes any string literal like specifier.
// PORT: `re_exports_only` keeps only the export declarations
// (`narrow_module_references`).
fn collect_checker_module_references(node: Node, re_exports_only: bool, names: &mut Vec<String>) {
    if re_exports_only && is_any_import_or_re_export(node) && !is_export_declaration(node) {
        return;
    }
    if is_any_import_or_re_export(node) {
        let module_name_expr = crate::ast::get_external_module_name(node);
        if module_name_expr.is_some() && is_string_literal_like(module_name_expr) {
            let module_name = module_name_expr.text();
            if !module_name.is_empty() {
                names.push(module_name.to_string());
            }
        }
        return;
    }
    if is_module_declaration(node) {
        // The body is a module block, or the next module declaration of
        // `namespace A.B {}`.
        let body = node.body();
        if body.is_nil() {
            return;
        }
        if is_module_block(body) {
            for statement in body.statements().iter() {
                collect_checker_module_references(statement, re_exports_only, names);
            }
        } else {
            collect_checker_module_references(body, re_exports_only, names);
        }
    }
}

/// True when the arena of a checker of `program` holds the symbols of
/// `file`: the file is bound, and its symbols are older than the program's
/// copy of the binder lineage.
fn checker_arena_holds(program: &'static GoProgram, file: Node) -> bool {
    let Some(go_file) = crate::ast::try_go_file(file.file_index()) else {
        return false;
    };
    if go_file.file_bind.get().is_none() {
        return false;
    }
    let symbol = file.symbol();
    let count =
        crate::program::bound_symbols_of(program).map_or(0, |symbols| symbols.symbol_count());
    symbol.is_nil() || symbol.index() < count
}

impl AliasResolver {
    /// Go `checker.NewChecker(aliasResolver, nil)`. The checker's program is
    /// current on this thread while the returned scope lives; keep the scope
    /// as long as the checker is used. `also_reads` are files that the caller
    /// reads with `get_source_file` while the checker lives.
    // PORT: a checker copies the binder lineage when it is made
    // (`SymbolArena::for_checker`), so it cannot read a file bound later. Go
    // binds each file when `GetSourceFile` parses it, while the checker
    // runs. Here the files that the checker can reach are read and bound
    // first: the root files, `also_reads`, and from each file the modules
    // that its module specifiers resolve to (`checker_module_references`).
    // This reads more files than Go, never fewer. The resolutions are kept
    // (`prefetch_resolved_module`); `get_resolved_module` fills the Go cache
    // and reports a failed lookup only when the checker asks, as Go does. A
    // file that the checker asks for later and that its arena does not hold
    // is unported (`bind_source_file`).
    // PORT: Go binds a second-pass root file only if an earlier pass bound
    // it (registry.go:1078 reads it from the host). Here every root file is
    // bound.
    // PORT: Go's `NewChecker` has no context. `ctx` is for
    // `should_stop_build`: a build that the caller drops on cancel stops the
    // file walk between two files, or before the program, and gets `None`.
    // For any other context the result is always `Some`. Each walk step
    // finishes its file (it resolves each module name, and reads and binds
    // each target), so a stop leaves only complete cache entries.
    pub fn new_checker(
        self: &Rc<Self>,
        ctx: &Context,
        also_reads: &[Node],
    ) -> Option<(Rc<RefCell<Checker>>, AliasResolverProgramScope)> {
        self.new_checker_walk(ctx, also_reads, false)
    }

    /// `new_checker` with a narrow walk: it follows only
    /// `narrow_module_references`, and the checker can read only the walked
    /// files. For any other file `get_source_file` sets `missed` and gives
    /// nil; the caller must then drop the checker and every result of it, and
    /// use `new_checker` on a new resolver. With no miss, the checker read
    /// only files that it asked for, so its results are those of
    /// `new_checker`.
    // PERF: Go reads only the files that the checker asks for: 450
    // node_modules files in the Query core editor session, against 1,316
    // for the full walk. The first auto-import of that session added 162 MiB
    // of RSS with the full walk, and 16 MiB with this one (Go: 17 to 35).
    pub fn new_narrow_checker(
        self: &Rc<Self>,
        ctx: &Context,
    ) -> Option<(Rc<RefCell<Checker>>, AliasResolverProgramScope)> {
        self.new_checker_walk(ctx, &[], true)
    }

    /// `new_checker`, with the narrow walk of `new_narrow_checker` when
    /// `narrow` is set.
    fn new_checker_walk(
        self: &Rc<Self>,
        ctx: &Context,
        also_reads: &[Node],
        narrow: bool,
    ) -> Option<(Rc<RefCell<Checker>>, AliasResolverProgramScope)> {
        // Go: NewChecker reads each root file; a nil file is a nil dereference.
        if self.root_files.iter().any(|file| file.is_nil()) {
            crate::core::go_nil_dereference();
        }
        let current_directory = self.get_current_directory();
        // The registry reads the second-pass root files from its host.
        crate::program::publish_parsed_files(&current_directory);
        let mut files: Vec<Node> = Vec::new();
        let mut seen: FxHashSet<usize> = FxHashSet::default();
        for &file in self.root_files.iter().chain(also_reads) {
            if file.is_some() && seen.insert(file.file_index()) {
                crate::program::bind_file_outside_program(file);
                files.push(file);
            }
        }
        // The host gives the same file for a name each time.
        let mut read_names: FxHashSet<String> = FxHashSet::default();
        let mut next = 0;
        while next < files.len() {
            if should_stop_build(ctx) {
                return None;
            }
            let file = files[next];
            next += 1;
            let module_references = if narrow {
                narrow_module_references(file)
            } else {
                checker_module_references(file)
            };
            for module_reference in module_references {
                let resolved = self.prefetch_resolved_module(file, &module_reference);
                if !resolved.is_resolved()
                    || !read_names.insert(resolved.resolved_file_name.clone())
                {
                    continue;
                }
                // Go: GetSourceFileForResolvedModule
                let target = self.get_source_file(&resolved.resolved_file_name);
                if target.is_some() && seen.insert(target.file_index()) {
                    files.push(target);
                }
            }
        }
        // PORT: a stop here also makes no program shell.
        if should_stop_build(ctx) {
            return None;
        }
        let scope = crate::program::new_alias_resolver_program(
            self.options(),
            &self.root_files,
            &files,
            &current_directory,
            self.use_case_sensitive_file_names(),
            self.clone(),
        );
        if narrow {
            *self.narrow_files.borrow_mut() =
                Some(files.iter().map(|file| file.file_index()).collect());
        }
        self.checker_program.set(Some(scope.program()));
        let checker = ls_program::new_checker_for_version(scope.program());
        Some((Rc::new(RefCell::new(checker)), scope))
    }

    /// The resolution that Go `GetResolvedModule` gives the checker for
    /// `module_reference` in `file` (the mode is Go `GetModeForUsageLocation`,
    /// always ESNext), made before the checker. It does not fill the Go
    /// cache or report a failed lookup.
    fn prefetch_resolved_module(&self, file: Node, module_reference: &str) -> Arc<ResolvedModule> {
        let info = source_file_info(file);
        let path = tspath::Path(info.path.clone());
        let key = module::ModeAwareCacheKey {
            name: module_reference.to_string(),
            mode: ModuleKind::ES_NEXT,
        };
        let cached = self
            .prefetched_modules
            .borrow()
            .get(&path)
            .and_then(|modules| modules.get(&key).cloned());
        if let Some(resolved) = cached {
            return resolved;
        }
        let (resolved, _, _) = self.module_resolver.resolve_module_name(
            module_reference,
            &info.file_name,
            ModuleKind::ES_NEXT,
            None,
        );
        self.prefetched_modules
            .borrow_mut()
            .entry(path)
            .or_default()
            .insert(key, resolved.clone());
        resolved
    }

    /// Go `binder.BindSourceFile(file)` in `GetSourceFile`.
    // PORT: before the checker exists, the file is published and bound
    // (`bind_alias_resolver_source_file`). After, the checker's arena must
    // already hold it (see `new_checker`). The unported guard is for a
    // module name that `checker_module_references` does not find.
    fn bind_source_file(&self, file: Node) {
        let Some(program) = self.checker_program.get() else {
            bind_alias_resolver_source_file(self.host.get_current_directory(), file);
            return;
        };
        if !checker_arena_holds(program, file) {
            unported!("aliasResolver.GetSourceFile after NewChecker");
        }
    }

    // Go: ls/autoimport/aliasresolver.go:53 BindSourceFiles
    // BindSourceFiles implements checker.Program.
    pub fn bind_source_files(&self) {
        // We will bind as we parse
    }

    // Go: ls/autoimport/aliasresolver.go:58 SourceFiles
    // SourceFiles implements checker.Program.
    pub fn source_files(&self) -> Vec<Node> {
        self.root_files.clone()
    }

    // Go: ls/autoimport/aliasresolver.go:63 Options
    // Options implements checker.Program.
    pub fn options(&self) -> CompilerOptions {
        CompilerOptions {
            no_check: Tristate::True,
            ..Default::default()
        }
    }

    // Go: ls/autoimport/aliasresolver.go:70 GetCurrentDirectory (at 673a5f17d713;
    // ts#64159 renames it BaseDirectory, aliasresolver.go:69)
    // GetCurrentDirectory implements checker.Program.
    // PORT: Go N' returns `moduleResolver.BaseDirectory()`, the current
    // directory of the registry builder's resolution host
    // (`store.options.CurrentDirectory`, project/snapshot.go:669). The host
    // here gives the same directory (project/autoimport.rs).
    pub fn get_current_directory(&self) -> String {
        self.host.get_current_directory().to_string()
    }

    // Go: ls/autoimport/aliasresolver.go:75 UseCaseSensitiveFileNames (at
    // 673a5f17d713; ts#64159 renames it CaseSensitivity, aliasresolver.go:74)
    // UseCaseSensitiveFileNames implements checker.Program.
    pub fn use_case_sensitive_file_names(&self) -> bool {
        self.host.fs().use_case_sensitive_file_names()
    }

    // Go: ls/autoimport/aliasresolver.go:80 GetSourceFile
    // GetSourceFile implements checker.Program.
    pub fn get_source_file(&self, file_name: &str) -> Node {
        let file = self
            .host
            .get_source_file(file_name, &(self.to_path)(file_name));
        // file may be nil due to symlink/realpath mismatch; see TestAutoImportBuilderFS
        if file.is_nil() {
            return Node::NIL;
        }
        // PORT: a file outside a narrow walk (`new_narrow_checker`).
        if self.checker_program.get().is_some()
            && self
                .narrow_files
                .borrow()
                .as_ref()
                .is_some_and(|files| !files.contains(&file.file_index()))
        {
            self.missed.set(true);
            return Node::NIL;
        }
        self.bind_source_file(file);
        file
    }

    // Go: ls/autoimport/aliasresolver.go:91 GetDefaultResolutionModeForFile
    // GetDefaultResolutionModeForFile implements checker.Program.
    pub fn get_default_resolution_mode_for_file(&self, file: &dyn HasFileName) -> ResolutionMode {
        ModuleKind::ES_NEXT
    }

    // Go: ls/autoimport/aliasresolver.go:96 GetEmitModuleFormatOfFile
    // GetEmitModuleFormatOfFile implements checker.Program.
    pub fn get_emit_module_format_of_file(&self, source_file: &dyn HasFileName) -> ModuleKind {
        ModuleKind::ES_NEXT
    }

    // Go: ls/autoimport/aliasresolver.go:101 GetEmitSyntaxForUsageLocation
    // GetEmitSyntaxForUsageLocation implements checker.Program.
    pub fn get_emit_syntax_for_usage_location(
        &self,
        source_file: &dyn HasFileName,
        usage_location: Node,
    ) -> ResolutionMode {
        ModuleKind::ES_NEXT
    }

    // Go: ls/autoimport/aliasresolver.go:106 GetImpliedNodeFormatForEmit
    // GetImpliedNodeFormatForEmit implements checker.Program.
    pub fn get_implied_node_format_for_emit(&self, source_file: &dyn HasFileName) -> ModuleKind {
        ModuleKind::ES_NEXT
    }

    // Go: ls/autoimport/aliasresolver.go:111 GetModeForUsageLocation
    // GetModeForUsageLocation implements checker.Program.
    pub fn get_mode_for_usage_location(
        &self,
        file: &dyn HasFileName,
        module_specifier: Node,
    ) -> ResolutionMode {
        ModuleKind::ES_NEXT
    }

    // Go: ls/autoimport/aliasresolver.go:116 GetResolvedModule
    // GetResolvedModule implements checker.Program.
    pub fn get_resolved_module(
        &self,
        current_source_file: &dyn HasFileName,
        module_reference: &str,
        mode: ResolutionMode,
    ) -> Arc<ResolvedModule> {
        // Go: r.resolvedModules.LoadOrStore(currentSourceFile.Path(), &collections.SyncMap{})
        let cache = self
            .resolved_modules
            .borrow_mut()
            .entry(current_source_file.path())
            .or_insert_with(|| Rc::new(RefCell::new(FxHashMap::default())))
            .clone();
        let key = module::ModeAwareCacheKey {
            name: module_reference.to_string(),
            mode,
        };
        let cached = cache.borrow().get(&key).cloned();
        if let Some(resolved) = cached {
            return resolved;
        }
        // PORT: `new_checker` made most resolutions before the checker
        // (`prefetch_resolved_module`); they are the same.
        let prefetched = self
            .prefetched_modules
            .borrow()
            .get(&current_source_file.path())
            .and_then(|modules| modules.get(&key).cloned());
        let resolved = match prefetched {
            Some(resolved) => resolved,
            None => {
                self.module_resolver
                    .resolve_module_name(
                        module_reference,
                        &current_source_file.file_name(),
                        mode,
                        None,
                    )
                    .0
            }
        };
        // Go: cache.LoadOrStore(key, resolved)
        let resolved = cache.borrow_mut().entry(key).or_insert(resolved).clone();
        if !resolved.is_resolved() && !tspath::path_is_relative(module_reference) {
            (self.on_failed_ambient_module_lookup)(current_source_file, module_reference);
        }
        resolved
    }

    // Go: ls/autoimport/aliasresolver.go:130 GetSourceFileForResolvedModule
    // GetSourceFileForResolvedModule implements checker.Program.
    pub fn get_source_file_for_resolved_module(&self, file_name: &str) -> Node {
        self.get_source_file(file_name)
    }

    // Go: ls/autoimport/aliasresolver.go:135 GetResolvedModules
    // GetResolvedModules implements checker.Program.
    // PORT: the Go nil map is an empty map.
    pub fn get_resolved_modules(
        &self,
    ) -> FxHashMap<tspath::Path, module::ModeAwareCache<Arc<ResolvedModule>>> {
        // only used when producing diagnostics, which hopefully the checker won't do
        FxHashMap::default()
    }

    // ---

    // Go: ls/autoimport/aliasresolver.go:143 GetSymlinkCache
    // GetSymlinkCache implements checker.Program.
    pub fn get_symlink_cache(&self) -> Rc<KnownSymlinks> {
        go_panic("unimplemented".to_string())
    }

    // Go: ls/autoimport/aliasresolver.go:148 GetSourceFileMetaData
    // GetSourceFileMetaData implements checker.Program.
    pub fn get_source_file_meta_data(&self, path: &tspath::Path) -> SourceFileMetaData {
        go_panic("unimplemented".to_string())
    }

    // Go: ls/autoimport/aliasresolver.go:153 CommonSourceDirectory
    // CommonSourceDirectory implements checker.Program.
    pub fn common_source_directory(&self) -> String {
        go_panic("unimplemented".to_string())
    }

    // Go: ls/autoimport/aliasresolver.go:158 ContentMapperExtensions
    // ContentMapperExtensions implements checker.Program.
    // PORT: the Go nil slice is an empty `Vec`.
    pub fn content_mapper_extensions(&self) -> Vec<String> {
        Vec::new()
    }

    // Go: ls/autoimport/aliasresolver.go:163 FileExists
    // FileExists implements checker.Program.
    pub fn file_exists(&self, file_name: &str) -> bool {
        go_panic("unimplemented".to_string())
    }

    // Go: ls/autoimport/aliasresolver.go:168 GetGlobalTypingsCacheLocation
    // GetGlobalTypingsCacheLocation implements checker.Program.
    pub fn get_global_typings_cache_location(&self) -> String {
        go_panic("unimplemented".to_string())
    }

    // Go: ls/autoimport/aliasresolver.go:173 GetImportHelpersImportSpecifier
    // GetImportHelpersImportSpecifier implements checker.Program.
    pub fn get_import_helpers_import_specifier(&self, path: &tspath::Path) -> Node {
        go_panic("unimplemented".to_string())
    }

    // Go: ls/autoimport/aliasresolver.go:178 GetJSXRuntimeImportSpecifier
    // GetJSXRuntimeImportSpecifier implements checker.Program.
    pub fn get_jsx_runtime_import_specifier(&self, path: &tspath::Path) -> (String, Node) {
        (String::new(), Node::NIL)
    }

    // Go: ls/autoimport/aliasresolver.go:183 GetNearestAncestorDirectoryWithPackageJson
    // GetNearestAncestorDirectoryWithPackageJson implements checker.Program.
    pub fn get_nearest_ancestor_directory_with_package_json(&self, dirname: &str) -> String {
        go_panic("unimplemented".to_string())
    }

    // Go: ls/autoimport/aliasresolver.go:188 GetPackageJsonInfo
    // GetPackageJsonInfo implements checker.Program.
    pub fn get_package_json_info(
        &self,
        pkg_json_path: &str,
    ) -> Option<Rc<packagejson::InfoCacheEntry>> {
        go_panic("unimplemented".to_string())
    }

    // Go: ls/autoimport/aliasresolver.go:193 GetProjectReferenceFromOutputDts
    // GetProjectReferenceFromOutputDts implements checker.Program.
    pub fn get_project_reference_from_output_dts(
        &self,
        path: &tspath::Path,
    ) -> Option<Rc<tsoptions::SourceOutputAndProjectReference>> {
        go_panic("unimplemented".to_string())
    }

    // Go: ls/autoimport/aliasresolver.go:198 GetProjectReferenceFromSource
    // GetProjectReferenceFromSource implements checker.Program.
    pub fn get_project_reference_from_source(
        &self,
        path: &tspath::Path,
    ) -> Option<Rc<tsoptions::SourceOutputAndProjectReference>> {
        go_panic("unimplemented".to_string())
    }

    // Go: ls/autoimport/aliasresolver.go:203 GetRedirectForResolution
    // GetRedirectForResolution implements checker.Program.
    pub fn get_redirect_for_resolution(
        &self,
        file: &dyn HasFileName,
    ) -> Option<Rc<tsoptions::ParsedCommandLine>> {
        go_panic("unimplemented".to_string())
    }

    // Go: ls/autoimport/aliasresolver.go:208 GetRedirectTargets
    // GetRedirectTargets implements checker.Program.
    pub fn get_redirect_targets(&self, path: &tspath::Path) -> Vec<String> {
        go_panic("unimplemented".to_string())
    }

    // Go: ls/autoimport/aliasresolver.go:213 GetResolvedModuleFromModuleSpecifier
    // GetResolvedModuleFromModuleSpecifier implements checker.Program.
    pub fn get_resolved_module_from_module_specifier(
        &self,
        file: &dyn HasFileName,
        module_specifier: Node,
    ) -> Option<Arc<ResolvedModule>> {
        go_panic("unimplemented".to_string())
    }

    // Go: ls/autoimport/aliasresolver.go:218 GetSourceOfProjectReferenceIfOutputIncluded
    // GetSourceOfProjectReferenceIfOutputIncluded implements checker.Program.
    pub fn get_source_of_project_reference_if_output_included(
        &self,
        file: &dyn HasFileName,
    ) -> String {
        go_panic("unimplemented".to_string())
    }

    // Go: ls/autoimport/aliasresolver.go:223 IsSourceFileDefaultLibrary
    // IsSourceFileDefaultLibrary implements checker.Program.
    pub fn is_source_file_default_library(&self, path: &tspath::Path) -> bool {
        false
    }

    // Go: ls/autoimport/aliasresolver.go:228 IsSourceFromProjectReference
    // IsSourceFromProjectReference implements checker.Program.
    pub fn is_source_from_project_reference(&self, path: &tspath::Path) -> bool {
        go_panic("unimplemented".to_string())
    }

    // Go: ls/autoimport/aliasresolver.go:233 SourceFileMayBeEmitted
    // SourceFileMayBeEmitted implements checker.Program.
    pub fn source_file_may_be_emitted(&self, source_file: Node, force_dts_emit: bool) -> bool {
        go_panic("unimplemented".to_string())
    }

    // Go: ls/autoimport/aliasresolver.go:237 GetPackagesMap
    // PORT: the Go nil map is an empty map.
    pub fn get_packages_map(&self) -> FxHashMap<String, bool> {
        FxHashMap::default()
    }
}

// Go: ls/autoimport/aliasresolver.go:241 var _ checker.Program = (*aliasResolver)(nil)
// PORT: the lazy `checker.Program` methods, which the alias resolver
// program calls (see the file header).
impl AliasResolverProgram for AliasResolver {
    fn source_file(&self, file_name: &str) -> Node {
        self.get_source_file(file_name)
    }

    fn source_file_for_resolved_module(&self, file_name: &str) -> Node {
        self.get_source_file_for_resolved_module(file_name)
    }

    fn resolved_module(
        &self,
        file: Node,
        module_reference: &str,
        mode: ResolutionMode,
    ) -> Arc<ResolvedModule> {
        let info = source_file_info(file);
        let current_source_file = new_has_file_name(&info.file_name, &info.path);
        self.get_resolved_module(&current_source_file, module_reference, mode)
    }
}
