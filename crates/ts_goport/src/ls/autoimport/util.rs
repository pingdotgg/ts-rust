use crate::ls::autoimport::prelude::*;

// Port of Go `ls/autoimport/util.go`.

use crate::frontend::compiler;
use crate::frontend::module;
use crate::frontend::module::ResolutionHost as _;
use crate::frontend::packagejson;
use crate::frontend::scanner::scanner_p1::{
    utf8_decode_last_rune_in_string, utf8_decode_rune_in_string,
};
use crate::frontend::tspath;
use crate::frontend::vfs;
use crate::frontend::vfs::Fs as _;
use crate::gostd::Context;
use crate::gostd::unicode;
use crate::modulespecifiers;
use crate::program::ls_program;
use std::cell::Cell;
use std::collections::VecDeque;
use std::time::SystemTime;

// Go: ls/autoimport/util.go:24 tryGetModuleIDAndFileNameOfModuleSymbol
// PORT: Go reads the symbol through its pointer; the port takes the arena
// that owns it (a checker's `symbols`), like the ast helpers.
pub fn try_get_module_id_and_file_name_of_module_symbol(
    symbols: &SymbolArena,
    symbol: SymbolId,
) -> (ModuleID, String, bool) {
    if !symbols.sym(symbol).is_external_module() {
        return (ModuleID::default(), String::new(), false);
    }
    let decl = get_non_augmentation_declaration(symbols, symbol);
    if decl.is_nil() {
        return (ModuleID::default(), String::new(), false);
    }
    if decl.kind() == SyntaxKind::SourceFile {
        return (
            file_module_id(tspath::Path(source_file_info(decl).path.clone())),
            source_file_file_name(decl).to_string(),
            true,
        );
    }
    if is_module_with_string_literal_name(decl) {
        return (ambient_module_id(decl.name().text()), String::new(), true);
    }
    (ModuleID::default(), String::new(), false)
}

// Go: ls/autoimport/util.go:41 getModuleIDAndFileNameOfModuleSymbol
// PORT: the arena parameter as in `try_get_module_id_and_file_name_of_module_symbol`.
pub fn get_module_id_and_file_name_of_module_symbol(
    symbols: &SymbolArena,
    symbol: SymbolId,
) -> (ModuleID, String) {
    if !symbols.sym(symbol).is_external_module() {
        crate::core::go_panic("symbol is not an external module".to_string());
    }
    let decl = get_non_augmentation_declaration(symbols, symbol);
    if decl.is_nil() {
        crate::core::go_panic("module symbol has no non-augmentation declaration".to_string());
    }
    if decl.kind() == SyntaxKind::SourceFile {
        return (
            file_module_id(tspath::Path(source_file_info(decl).path.clone())),
            source_file_file_name(decl).to_string(),
        );
    }
    if is_module_with_string_literal_name(decl) {
        return (ambient_module_id(decl.name().text()), String::new());
    }
    crate::core::go_panic("could not determine module ID of module symbol".to_string());
}

// Go: ls/autoimport/util.go:68 wordIndices
// wordIndices splits an identifier into its constituent words based on camelCase and snake_case conventions
// by returning the starting byte indices of each word. The first index is always 0.
//   - CamelCase
//     ^    ^
//   - snake_case
//     ^     ^
//   - ParseURL
//     ^    ^
//   - __proto__
//     ^
// PORT: Go ranges over the runes of `s`; `char_indices` gives the same byte
// indices. Go decodes `s[byteIndex+1:]` even inside a multi-byte rune; the
// scanner byte decoders do the same without slicing inside a character.
pub fn word_indices(s: &str) -> Vec<i32> {
    let mut indices: Vec<i32> = Vec::new();
    let bytes = s.as_bytes();
    for (byte_index, rune_value) in s.char_indices() {
        if byte_index == 0 {
            indices.push(byte_index as i32);
            continue;
        }
        if rune_value == '_' {
            if byte_index + 1 < s.len() && bytes[byte_index + 1] != b'_' {
                indices.push((byte_index + 1) as i32);
            }
            continue;
        }
        if unicode::is_upper(rune_value)
            && (unicode::is_lower(rune_of(utf8_decode_last_rune_in_string(s, byte_index).0))
                || (byte_index + 1 < s.len()
                    && unicode::is_lower(rune_of(utf8_decode_rune_in_string(s, byte_index + 1).0))))
        {
            indices.push(byte_index as i32);
        }
    }
    indices
}

/// A decoded Go rune as a `char` (`utf8.RuneError` is U+FFFD).
fn rune_of(r: i32) -> char {
    char::from_u32(r as u32).unwrap_or(char::REPLACEMENT_CHARACTER)
}

// Go: ls/autoimport/util.go:88 getPackageNamesInNodeModules
// PORT: Go returns a non-nil `*collections.Set[string]`; the port returns the set.
pub fn get_package_names_in_node_modules(
    node_modules_dir: &str,
    fs: &dyn vfs::Fs,
) -> FxHashSet<String> {
    let mut package_names: FxHashSet<String> = FxHashSet::default();
    if tspath::get_base_file_name(node_modules_dir) != "node_modules" {
        crate::core::go_panic("nodeModulesDir is not a node_modules directory".to_string());
    }
    // A missing node_modules directory yields no entries (GetAccessibleEntries returns
    // empty), so there's no need to check existence first: a deleted node_modules is
    // handled upstream in updateBucketAndDirectoryExistence, which drops the bucket.
    let entries = fs.get_accessible_entries(node_modules_dir);
    for base_name in &entries.directories {
        if base_name.as_bytes()[0] == b'.' {
            continue;
        }
        if base_name.as_bytes()[0] == b'@' {
            let scoped_dir_path = tspath::combine_paths(node_modules_dir, &[base_name.as_str()]);
            for scoped_package_dir_name in &fs.get_accessible_entries(&scoped_dir_path).directories
            {
                let scoped_base_name = tspath::get_base_file_name(scoped_package_dir_name);
                if base_name == "@types" {
                    package_names.insert(module::get_package_name_from_types_package_name(
                        &tspath::combine_paths("@types", &[scoped_base_name.as_str()]),
                    ));
                } else {
                    package_names.insert(tspath::combine_paths(
                        base_name,
                        &[scoped_base_name.as_str()],
                    ));
                }
            }
            continue;
        }
        package_names.insert(base_name.clone());
    }
    package_names
}

// Go: ls/autoimport/util.go:118 getDefaultLikeExportNameFromDeclaration
// PORT: the arena parameter as in `try_get_module_id_and_file_name_of_module_symbol`.
pub fn get_default_like_export_name_from_declaration(
    symbols: &SymbolArena,
    symbol: SymbolId,
) -> String {
    for &d in symbols.sym(symbol).declarations.iter() {
        // "export default" in this case. See `ExportAssignment`for more details.
        if is_export_assignment(d) {
            let inner_expression =
                skip_outer_expressions(d.expression(), OuterExpressionKinds::OEK_ALL);
            if is_identifier(inner_expression) {
                return inner_expression.text().to_string();
            }
            continue;
        }
        // "export { ~ as default }"
        if is_export_specifier(d)
            && symbols.sym(d.symbol()).flags == SymbolFlags::ALIAS
            && d.property_name().is_some()
        {
            if d.property_name().kind() == SyntaxKind::Identifier {
                return d.property_name().text().to_string();
            }
            continue;
        }
        // GH#52694
        let name = get_name_of_declaration(d);
        if name.is_some() && name.kind() == SyntaxKind::Identifier {
            return name.text().to_string();
        }
        let parent = symbols.sym(symbol).parent;
        if parent.is_some() && !is_external_module_symbol_in(symbols, parent) {
            return symbols.sym(parent).name.to_string();
        }
    }
    String::new()
}

/// Go `checker.IsExternalModuleSymbol(moduleSymbol)`.
// PORT: the port has it as a `Checker` method; this is the same test on an
// arena, for callers that have no checker.
fn is_external_module_symbol_in(symbols: &SymbolArena, module_symbol: SymbolId) -> bool {
    let s = symbols.sym(module_symbol);
    s.flags.intersects(SymbolFlags::MODULE) && s.name.as_bytes()[0] == b'"'
}

// Go: ls/autoimport/util.go:145 getResolvedPackageNames
// PORT: Go returns a non-nil `*collections.Set[string]`; the port returns the
// set. Go ranges over map keys in random order; the port uses map order
// (the results go into a set).
pub fn get_resolved_package_names(
    ctx: &Context,
    program: &compiler::NewProgram,
) -> FxHashSet<String> {
    let raw_names = program.resolved_package_names();
    let unresolved_package_names = program.unresolved_package_names();

    // Normalize @types/ package names to their actual package names
    // (e.g., "@types/react" → "react"). ResolvedPackageNames can contain
    // @types names when the program resolves an import like "react" to
    // "@types/react/index.d.ts" via the PackageId.Name field.
    let mut resolved_package_names: FxHashSet<String> = FxHashSet::default();
    resolved_package_names.reserve(raw_names.len());
    for name in raw_names {
        resolved_package_names.insert(module::get_package_name_from_types_package_name(name));
    }

    for name in program.options().types.iter().flatten() {
        if name != "*" {
            resolved_package_names.insert(module::get_package_name_from_types_package_name(name));
        }
    }

    if !unresolved_package_names.is_empty() {
        let (checker, done) = ls_program::get_type_checker(program, ctx);
        {
            let mut checker_ref = checker.borrow_mut();
            let checker: &mut Checker = &mut checker_ref;
            for name in unresolved_package_names {
                let symbol = checker.try_find_ambient_module_exported(name);
                if symbol.is_some() {
                    let declaring_file = get_source_file_of_module(&checker.symbols, symbol);
                    let package_name = modulespecifiers::get_package_name_from_directory(
                        source_file_file_name(declaring_file),
                    );
                    if !package_name.is_empty() {
                        resolved_package_names.insert(
                            module::get_package_name_from_types_package_name(&package_name),
                        );
                    }
                }
            }
        }
        // Go: defer done()
        done.call();
    }
    resolved_package_names
}

// Go: ls/autoimport/util.go:183 addProjectReferenceOutputMappings
// addProjectReferenceOutputMappings adds output .d.ts to source file mappings
// from a program's project references to the provided map.
// This is used during node_modules bucket building to redirect extraction
// from output files to source files when the output is from a project reference.
pub fn add_project_reference_output_mappings(
    program: &compiler::NewProgram,
    result: &mut FxHashMap<tspath::Path, String>,
) {
    let refs = program.get_resolved_project_references();
    for ref_ in &refs {
        let Some(ref_) = ref_ else {
            continue;
        };
        ref_.parse_input_output_names();
        // Go ranges over a nil map as zero items.
        if let Some(output_dts_to_project_reference) = ref_.output_dts_to_project_reference() {
            for (output_dts_path, mapping) in output_dts_to_project_reference {
                // Only add if not already present (first program wins)
                if !result.contains_key(output_dts_path) {
                    result.insert(output_dts_path.clone(), mapping.source.clone());
                }
            }
        }
    }
}

// Go: ls/autoimport/util.go:199 createCheckerPool
// PORT: Go makes up to GOMAXPROCS new checkers with `checker.NewChecker(program,
// nil)` over any `checker.Program`. Here `program` is a `NewProgram` and each
// checker is `ls_program::new_checker(program)` (contract C3): a new checker
// on the dispatch thread, like Go. The Go channel is
// a FIFO queue; the atomic counter is a `Cell`. Extraction runs serially in
// Go start order, so a released checker is back in the pool before the next
// request and only one checker is made. Go blocks on an empty pool at the
// limit; serial code cannot wait for another goroutine, so that case panics
// with the Go deadlock text. A release after `closePool` panics like a send
// on a closed channel.
pub fn create_checker_pool(
    program: &compiler::NewProgram,
) -> (
    Box<dyn Fn() -> (Rc<RefCell<Checker>>, ls_program::Release) + '_>,
    Box<dyn Fn()>,
    Box<dyn Fn() -> i32>,
) {
    // Go: runtime.GOMAXPROCS(0)
    let max_size = crate::gostd::runtime::gomaxprocs() as i32;
    let pool: Rc<RefCell<VecDeque<Rc<RefCell<Checker>>>>> =
        Rc::new(RefCell::new(VecDeque::with_capacity(max_size as usize)));
    let closed: Rc<Cell<bool>> = Rc::new(Cell::new(false));
    let created: Rc<Cell<i32>> = Rc::new(Cell::new(0));

    // Go: func() { pool <- ch }
    let release_to_pool = {
        let pool = pool.clone();
        let closed = closed.clone();
        move |ch: Rc<RefCell<Checker>>| -> ls_program::Release {
            let pool = pool.clone();
            let closed = closed.clone();
            ls_program::Release::new(move || {
                if closed.get() {
                    crate::core::go_panic("send on closed channel".to_string());
                }
                pool.borrow_mut().push_back(ch);
            })
        }
    };

    let get_checker: Box<dyn Fn() -> (Rc<RefCell<Checker>>, ls_program::Release) + '_> = {
        let pool = pool;
        let created = created.clone();
        Box::new(move || {
            // PORT: the checker runs with its program current until the
            // release (ls_program module comment).
            let program_guard = ls_program::enter(program);
            // Try to get an existing checker
            let existing = pool.borrow_mut().pop_front();
            if let Some(ch) = existing {
                return (ch.clone(), program_guard.with_release(release_to_pool(ch)));
            }
            // Try to create a new one if under limit
            loop {
                let current = created.get();
                if current >= max_size {
                    // At limit, wait for one to become available
                    // PORT: no other goroutine can return a checker (serial port).
                    panic!("fatal error: all goroutines are asleep - deadlock!");
                }
                // Go: created.CompareAndSwap(current, current+1) (always succeeds on one thread)
                created.set(current + 1);
                let ch = Rc::new(RefCell::new(ls_program::new_checker(program)));
                return (ch.clone(), program_guard.with_release(release_to_pool(ch)));
            }
        })
    };

    let close_pool: Box<dyn Fn()> = {
        let closed = closed;
        Box::new(move || {
            // Go: close(pool)
            if closed.get() {
                crate::core::go_panic("close of closed channel".to_string());
            }
            closed.set(true);
        })
    };

    let get_created_count: Box<dyn Fn() -> i32> = {
        let created = created;
        Box::new(move || created.get())
    };

    (get_checker, close_pool, get_created_count)
}

// Go: ls/autoimport/util.go:234 addPackageJsonDependencies
// addPackageJsonDependencies adds all dependencies and peerDependencies from a package.json
// to the given set, canonicalizing @types package names to their base names.
pub fn add_package_json_dependencies(
    contents: &packagejson::PackageJson,
    deps: &mut FxHashSet<String>,
) {
    contents.range_dependencies(|name, _, field| {
        if name.is_empty() || name == "@types/" || name.as_bytes()[0] == b'.' {
            // Edge cases that could make us blow up probably
            return true;
        }
        if field == "dependencies" || field == "peerDependencies" {
            deps.insert(module::get_package_name_from_types_package_name(name));
        }
        true
    });
}

// Go: ls/autoimport/util.go:252 getPackageRealpathFuncs
// getPackageRealpathFuncs returns functions to transform between symlink and realpath for files within a package.
// It calls FS.Realpath once per package directory and uses prefix substitution for files within that directory,
// avoiding expensive realpath syscalls for each file. For files outside the package (e.g. re-exported
// dependencies reached through node_modules symlinks), it resolves the file's directory realpath once,
// finds the symlink boundary (the package root where the symlink lives), and caches that prefix mapping.
// All subsequent files under the same symlinked package directory use prefix substitution with no syscalls.
// PORT: Go `func(string) string` values are `Rc<dyn Fn(&str) -> String>`.
// ts#64159 behavior only: Go `tspath.RootedFilePath` and
// `tspath.RootedDirectoryPath` are `String`.
pub fn get_package_realpath_funcs(
    fs: Rc<dyn vfs::Fs>,
    package_dir: &str,
) -> (Rc<dyn Fn(&str) -> String>, Rc<dyn Fn(&str) -> String>) {
    let real_package_dir = fs.realpath(package_dir);
    let is_symlinked = real_package_dir != package_dir;
    // Go `fileName.RelativeTo(directory)` (rooted_path.go:422, case sensitive
    // below the root) and `RootedDirectoryPath.ResolveRelativeFile`
    // (rooted_path.go:746). ts#64159 replaces ts#64544's replacePrefix
    // (at 59f5b0233 util.go:256) with them.
    fn relative_to(file_name: &str, directory: &str) -> Option<String> {
        tspath::relative_path_within_directory(directory, file_name, true)
            .map(|relative| relative.into_owned())
    }
    fn resolve_relative_file(directory: &str, relative: &str) -> String {
        if relative.is_empty() {
            return directory.to_string();
        }
        tspath::combine_paths(directory, &[relative])
    }
    // Cache of package-directory-level symlink→realpath prefix mappings for
    // external packages encountered via re-exports. Keyed by the node_modules
    // package directory (e.g. "/app/node_modules/dep"), so all files under
    // that package reuse a single realpath lookup.
    let dir_cache: Rc<RefCell<FxHashMap<String, String>>> =
        Rc::new(RefCell::new(FxHashMap::default()));
    let to_realpath: Rc<dyn Fn(&str) -> String> = {
        let fs = fs.clone();
        let package_dir = package_dir.to_string();
        let real_package_dir = real_package_dir.clone();
        let dir_cache = dir_cache;
        Rc::new(move |file_name: &str| -> String {
            // Fast path: files within the package use prefix substitution.
            if is_symlinked {
                // ts#64544: only at a component boundary, so a sibling "pkg2"
                // of "pkg" is not inside the package.
                if let Some(relative) = relative_to(file_name, &package_dir) {
                    return resolve_relative_file(&real_package_dir, &relative);
                }
            }
            // Files outside the package (e.g. re-exports into symlinked deps):
            // find the node_modules package directory, resolve it once, and cache.
            let mut file_package_dir = module::node_module_package_root_for_file(file_name);
            if file_package_dir.is_empty() {
                return file_name.to_string();
            }
            // The wrapped FS also calls Realpath while traversing directories.
            // The two parses differ only when the path may be a package root,
            // so establish its kind before using the package cache.
            let directory_package = module::node_module_package_root_for_directory(file_name);
            if directory_package != file_package_dir && fs.directory_exists(file_name) {
                file_package_dir = directory_package;
            }
            let cached = dir_cache.borrow().get(&file_package_dir).cloned();
            if let Some(real_dir) = cached {
                if real_dir == file_package_dir {
                    return file_name.to_string();
                }
                let relative = relative_to(file_name, &file_package_dir).unwrap_or_default();
                return resolve_relative_file(&real_dir, &relative);
            }
            let real_dir = fs.realpath(&file_package_dir);
            dir_cache
                .borrow_mut()
                .insert(file_package_dir.clone(), real_dir.clone());
            if real_dir == file_package_dir {
                return file_name.to_string();
            }
            let relative = relative_to(file_name, &file_package_dir).unwrap_or_default();
            resolve_relative_file(&real_dir, &relative)
        })
    };
    if !is_symlinked {
        // Go: core.Identity
        return (
            to_realpath,
            Rc::new(|file_name: &str| file_name.to_string()),
        );
    }
    // toSymlink only handles files within the package directory (reversing the
    // packageDir→realPackageDir substitution). It does not handle arbitrary external
    // paths; callers should only use it for files known to be within the package.
    let to_symlink: Rc<dyn Fn(&str) -> String> = {
        let package_dir = package_dir.to_string();
        Rc::new(move |file_name: &str| -> String {
            // ts#64159: only at a component boundary, as in `to_realpath`.
            if let Some(relative) = relative_to(file_name, &real_package_dir) {
                return resolve_relative_file(&package_dir, &relative);
            }
            file_name.to_string()
        })
    };
    (to_realpath, to_symlink)
}

// Go: ls/autoimport/util.go:302 resolutionHost
// PORT: private like Go; the trait is `module::ResolutionHost`.
struct ResolutionHost {
    fs: Rc<dyn vfs::Fs>,
    current_directory: String,
}

// Go: ls/autoimport/util.go:307 var _ module.ResolutionHost = (*resolutionHost)(nil)
impl module::ResolutionHost for ResolutionHost {
    // Go: ls/autoimport/util.go:309 GetCurrentDirectory
    fn get_current_directory(&self) -> &str {
        &self.current_directory
    }

    // Go: ls/autoimport/util.go:313 FS
    fn fs(&self) -> &dyn vfs::Fs {
        &*self.fs
    }
}

// Go: ls/autoimport/util.go:327 getModuleResolver
// PORT: Go `core.EmptyCompilerOptions` is shared; the resolver takes an `Rc`,
// so it gets a new default `CompilerOptions`. Go `*module.DefaultResolver` is
// `Rc<module::DefaultResolver>`. ts#64159 passes the builder's current
// directory; the host's `get_current_directory` is that directory (see
// `RegistryCloneHost`).
pub fn get_module_resolver(
    host: &Rc<dyn RegistryCloneHost>,
    realpath: Rc<dyn Fn(&str) -> String>,
    mut opts: module::ResolverOptions,
) -> Rc<module::DefaultResolver> {
    let rh: Rc<dyn module::ResolutionHost> = Rc::new(ResolutionHost {
        fs: vfs::wrapvfs_wrap(
            Rc::new(RegistryCloneHostFs { host: host.clone() }),
            vfs::Replacements {
                realpath: Some(Box::new(move |path: &str| realpath(path))),
                ..Default::default()
            },
        ),
        current_directory: host.get_current_directory().to_string(),
    });
    opts.host = Some(rh);
    opts.compiler_options = Some(Rc::new(CompilerOptions::default()));
    Rc::new(module::new_resolver(opts))
}

/// The `vfs.FS` value of a registry host (Go `host.FS()` passed on).
// PORT: `module::ResolutionHost::fs` returns a borrow, and Go keeps the FS
// value in closures and wrappers (`wrapvfs.Wrap`, `getPackageRealpathFuncs`).
// This adapter holds the host and forwards every call to `host.fs()`.
pub struct RegistryCloneHostFs {
    pub host: Rc<dyn RegistryCloneHost>,
}

impl vfs::Fs for RegistryCloneHostFs {
    fn use_case_sensitive_file_names(&self) -> bool {
        self.host.fs().use_case_sensitive_file_names()
    }

    fn file_exists(&self, path: &str) -> bool {
        self.host.fs().file_exists(path)
    }

    fn read_file(&self, path: &str) -> (String, bool) {
        self.host.fs().read_file(path)
    }

    fn write_file(&self, path: &str, data: &str) -> Result<(), vfs::FsError> {
        self.host.fs().write_file(path, data)
    }

    fn append_file(&self, path: &str, data: &str) -> Result<(), vfs::FsError> {
        self.host.fs().append_file(path, data)
    }

    fn remove(&self, path: &str) -> Result<(), vfs::FsError> {
        self.host.fs().remove(path)
    }

    fn chtimes(
        &self,
        path: &str,
        a_time: Option<SystemTime>,
        m_time: Option<SystemTime>,
    ) -> Result<(), vfs::FsError> {
        self.host.fs().chtimes(path, a_time, m_time)
    }

    fn directory_exists(&self, path: &str) -> bool {
        self.host.fs().directory_exists(path)
    }

    fn get_accessible_entries(&self, path: &str) -> vfs::Entries {
        self.host.fs().get_accessible_entries(path)
    }

    fn stat(&self, path: &str) -> Option<vfs::FileInfo> {
        self.host.fs().stat(path)
    }

    fn realpath(&self, path: &str) -> String {
        self.host.fs().realpath(path)
    }
}
