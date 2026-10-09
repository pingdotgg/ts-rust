//! Port of modulespecifiers/types.go.

use crate::prelude::*;

use super::packagejson::InfoCacheEntry;
use super::symlinks::KnownSymlinks;
use super::tspath;
use std::sync::Arc;

// Go: modulespecifiers/types.go:13 SourceFileForSpecifierGeneration
pub trait SourceFileForSpecifierGeneration {
    fn path(&self) -> tspath::Path;
    fn file_name(&self) -> String;
    fn imports(&self) -> Vec<Node>;
    fn is_js(&self) -> bool;
    // PORT: Go passes the same value to host methods that take an
    // `ast.HasFileName`. Host methods here take the source file node.
    fn node(&self) -> Node;
}

/// Go `*ast.SourceFile` implements `SourceFileForSpecifierGeneration`.
impl SourceFileForSpecifierGeneration for Node {
    fn path(&self) -> tspath::Path {
        tspath::Path(source_file_info(*self).path.clone())
    }

    fn file_name(&self) -> String {
        source_file_file_name(*self).to_string()
    }

    fn imports(&self) -> Vec<Node> {
        source_file_imports(*self).iter().collect()
    }

    fn is_js(&self) -> bool {
        is_source_file_js(*self)
    }

    fn node(&self) -> Node {
        *self
    }
}

// Go: modulespecifiers/types.go:20 CheckerShape
// PORT: the checker methods take `&mut self`. Go reads
// `moduleSymbol.Declarations` and other symbol fields directly; `symbols`
// gives that access.
pub trait CheckerShape {
    fn get_symbol_at_location(&mut self, node: Node) -> SymbolId;
    fn get_aliased_symbol(&mut self, symbol: SymbolId) -> SymbolId;
    fn symbols(&self) -> &SymbolArena;
}

impl CheckerShape for Checker {
    fn get_symbol_at_location(&mut self, node: Node) -> SymbolId {
        self.get_symbol_at_location_exported(node)
    }

    fn get_aliased_symbol(&mut self, symbol: SymbolId) -> SymbolId {
        Checker::get_aliased_symbol(self, symbol)
    }

    fn symbols(&self) -> &SymbolArena {
        &self.symbols
    }
}

// Go: modulespecifiers/types.go:25 ResultKind
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ResultKind {
    #[default]
    None,
    NodeModules,
    Paths,
    Redirect,
    Relative,
    Ambient,
}

// Go: modulespecifiers/types.go:36 ModuleSpecifiersResult (ts#63931)
#[derive(Clone, Debug, Default)]
pub struct ModuleSpecifiersResult {
    pub specifiers: Vec<String>,
    pub kind: ResultKind,
    pub ambient_module_symbol: SymbolId, // used to construct an import attributes node, if one is needed
}

// Go: modulespecifiers/types.go:42 ModulePath
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ModulePath {
    pub file_name: String,
    pub is_in_node_modules: bool,
    pub is_redirect: bool,
}

// Go: modulespecifiers/types.go:48 ModuleSpecifierGenerationHost
// PORT: methods that take `ast.HasFileName` or `*ast.StringLiteralLike`
// take a `Node`. Package.json entries are shared through `Arc`, as Go
// shares the `*packagejson.InfoCacheEntry` pointer.
pub trait ModuleSpecifierGenerationHost {
    // GetModuleResolutionCache() any // !!! TODO: adapt new resolution cache model
    fn get_symlink_cache(&self) -> Option<Rc<KnownSymlinks>>;
    // GetFileIncludeReasons() any // !!! TODO: adapt new resolution cache model
    fn common_source_directory(&self) -> String;
    // tsgo#4712
    fn content_mapper_extensions(&self) -> Vec<String>;
    fn get_global_typings_cache_location(&self) -> String;
    fn use_case_sensitive_file_names(&self) -> bool;
    fn get_current_directory(&self) -> String;
    // Go: modulespecifiers/types.go:56 BaseDirectory (ts#64159, rule R1): the
    // program's base directory, that is the config file's directory, or the
    // current directory without a config. It is the base of "paths"
    // (specifiers.go:524) and the project directory without a config file
    // (:593).
    // PORT: the default is the current directory. A host whose program knows
    // its base directory returns it.
    fn base_directory(&self) -> String {
        self.get_current_directory()
    }

    fn get_project_reference_from_source(
        &self,
        path: &tspath::Path,
    ) -> Option<Arc<SourceOutputAndProjectReference>>;
    fn get_redirect_targets(&self, path: &tspath::Path) -> Vec<String>;
    fn get_source_of_project_reference_if_output_included(&self, file: Node) -> String;

    fn file_exists(&self, path: &str) -> bool;

    fn get_nearest_ancestor_directory_with_package_json(&self, dirname: &str) -> String;
    fn get_package_json_info(&self, pkg_json_path: &str) -> Option<Arc<InfoCacheEntry>>;
    fn get_default_resolution_mode_for_file(&self, file: Node) -> ResolutionMode;
    fn get_resolved_module_from_module_specifier(
        &self,
        file: Node,
        module_specifier: Node,
    ) -> Option<ResolvedModule>;
    fn get_mode_for_usage_location(&self, file: Node, module_specifier: Node) -> ResolutionMode;

    // PORT: Go passes the same host value as an `outputpaths.OutputPathsHost`.
    // A Rust trait object cannot convert to an unrelated trait, so the host
    // returns itself through this method.
    fn as_output_paths_host(&self) -> &dyn super::deps::OutputPathsHost;
}

// Go: modulespecifiers/types.go:71 ImportModuleSpecifierPreference
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ImportModuleSpecifierPreference {
    #[default]
    None, // "" !!!
    Shortest,        // "shortest"
    ProjectRelative, // "project-relative"
    Relative,        // "relative"
    NonRelative,     // "non-relative"
}

// Go: modulespecifiers/types.go:81 ImportModuleSpecifierEndingPreference
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ImportModuleSpecifierEndingPreference {
    #[default]
    None, // "" !!!
    Auto,    // "auto"
    Minimal, // "minimal"
    Index,   // "index"
    Js,      // "js"
}

// Go: modulespecifiers/types.go:91 UserPreferences
#[derive(Clone, Debug, Default)]
pub struct UserPreferences {
    pub import_module_specifier_preference: ImportModuleSpecifierPreference,
    pub import_module_specifier_ending: ImportModuleSpecifierEndingPreference,
    pub auto_import_specifier_exclude_regexes: Vec<String>,
}

// Go: modulespecifiers/types.go:97 ModuleSpecifierOptions
#[derive(Clone, Copy, Debug, Default)]
pub struct ModuleSpecifierOptions {
    pub override_import_mode: ResolutionMode,
}

// Go: modulespecifiers/types.go:101 RelativePreferenceKind
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RelativePreferenceKind {
    Relative,
    NonRelative,
    Shortest,
    ExternalNonRelative,
}

// Go: modulespecifiers/types.go:110 ModuleSpecifierEnding
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModuleSpecifierEnding {
    Minimal,
    Index,
    JsExtension,
    TsExtension,
}

// Go: modulespecifiers/types.go:119 MatchingMode
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MatchingMode {
    Exact,
    Directory,
    Pattern,
}
