//! Port of module/types.go.

use crate::frontend::prelude::*;
use std::sync::Arc;

// Go: module/types.go:14 ResolutionHost
// PORT: Go interface. `FS()` returns a borrowed trait object; the host
// keeps the shared `Rc<dyn Fs>`.
pub trait ResolutionHost {
    fn fs(&self) -> &dyn Fs;
    fn get_current_directory(&self) -> &str;
}

// Go: module/types.go:19 Resolver (ts#64299)
// PORT: Go interface. A nil `*ResolvedModule` is `None`; a nil `error` is
// `None`. `PackageJsonCacheEntries` takes a `dyn FnMut`, because a trait
// object has no generic methods.
pub trait Resolver {
    fn resolve_module_name(
        &self,
        module_name: &str,
        containing_file: &str,
        resolution_mode: ResolutionMode,
        redirected_reference: Option<&dyn ModuleResolvedProjectReference>,
    ) -> (
        Option<Arc<ResolvedModule>>,
        Vec<DiagAndArgs>,
        Option<crate::gostd::GoError>,
    );
    fn resolve_module_name_from_directory(
        &self,
        module_name: &str,
        containing_directory: &str,
        resolution_mode: ResolutionMode,
    ) -> (
        Option<Arc<ResolvedModule>>,
        Vec<DiagAndArgs>,
        Option<crate::gostd::GoError>,
    );
    fn resolve_type_reference_directive(
        &self,
        type_reference_directive_name: &str,
        containing_file: &str,
        resolution_mode: ResolutionMode,
        redirected_reference: Option<&dyn ModuleResolvedProjectReference>,
    ) -> (Rc<ResolvedTypeReferenceDirective>, Vec<DiagAndArgs>);
    // Go: module/types.go:38 Resolver.GetResolutionData (ts#64519)
    fn get_resolution_data(&self) -> Rc<ResolutionData>;

    // Go: module/types.go:38 GetPackageScopeForPath, :39 PackageJsonCacheEntries
    // and :40 ResolvePackageDirectory (at 673a5f17d713; ts#64519 removes them
    // from the interface, module/types.go:38 GetResolutionData)
    // PORT: kept until the program takes Go N' `Program.newResolver`
    // (program.go:191): the program still asks its loader's resolver.
    fn get_package_scope_for_path(&self, directory: &str) -> Option<Rc<InfoCacheEntry>>;
    fn package_json_cache_entries(
        &self,
        f: &mut dyn FnMut(&Path, PackageJsonCacheEntry<'_>) -> bool,
    );
    fn resolve_package_directory(
        &self,
        module_name: &str,
        containing_file: &str,
        resolution_mode: ResolutionMode,
        redirected_reference: Option<&dyn ModuleResolvedProjectReference>,
    ) -> Option<ResolvedModule>;

    /// Empties the resolution caches when no program uses this resolver any
    /// more (`NewProgram::release_resolver_caches`).
    // PORT: not in Go (Go's GC frees the resolver). Only `DefaultResolver`
    // has caches to release.
    fn release_caches(&self) {}

    /// The resolver as a `DefaultResolver`, when it is one (Go
    /// `resolver.(*module.DefaultResolver)`).
    // PORT: Rust has no type assertion on a trait object; tests read the
    // caches through this.
    fn as_default_resolver(&self) -> Option<&DefaultResolver> {
        None
    }
}

// Go: module/resolver.go:335 DefaultResolver as a module.Resolver
// PORT: each method is the `DefaultResolver` method of the same Go name.
impl Resolver for DefaultResolver {
    fn resolve_module_name(
        &self,
        module_name: &str,
        containing_file: &str,
        resolution_mode: ResolutionMode,
        redirected_reference: Option<&dyn ModuleResolvedProjectReference>,
    ) -> (
        Option<Arc<ResolvedModule>>,
        Vec<DiagAndArgs>,
        Option<crate::gostd::GoError>,
    ) {
        let (result, trace, err) = DefaultResolver::resolve_module_name(
            self,
            module_name,
            containing_file,
            resolution_mode,
            redirected_reference,
        );
        (Some(result), trace, err)
    }

    fn resolve_module_name_from_directory(
        &self,
        module_name: &str,
        containing_directory: &str,
        resolution_mode: ResolutionMode,
    ) -> (
        Option<Arc<ResolvedModule>>,
        Vec<DiagAndArgs>,
        Option<crate::gostd::GoError>,
    ) {
        let (result, trace, err) = DefaultResolver::resolve_module_name_from_directory(
            self,
            module_name,
            containing_directory,
            resolution_mode,
        );
        (Some(result), trace, err)
    }

    fn resolve_type_reference_directive(
        &self,
        type_reference_directive_name: &str,
        containing_file: &str,
        resolution_mode: ResolutionMode,
        redirected_reference: Option<&dyn ModuleResolvedProjectReference>,
    ) -> (Rc<ResolvedTypeReferenceDirective>, Vec<DiagAndArgs>) {
        DefaultResolver::resolve_type_reference_directive(
            self,
            type_reference_directive_name,
            containing_file,
            resolution_mode,
            redirected_reference,
        )
    }

    fn get_resolution_data(&self) -> Rc<ResolutionData> {
        DefaultResolver::get_resolution_data(self)
    }

    fn get_package_scope_for_path(&self, directory: &str) -> Option<Rc<InfoCacheEntry>> {
        DefaultResolver::get_package_scope_for_path(self, directory)
    }

    fn package_json_cache_entries(
        &self,
        f: &mut dyn FnMut(&Path, PackageJsonCacheEntry<'_>) -> bool,
    ) {
        DefaultResolver::package_json_cache_entries(self, f);
    }

    fn resolve_package_directory(
        &self,
        module_name: &str,
        containing_file: &str,
        resolution_mode: ResolutionMode,
        redirected_reference: Option<&dyn ModuleResolvedProjectReference>,
    ) -> Option<ResolvedModule> {
        DefaultResolver::resolve_package_directory(
            self,
            module_name,
            containing_file,
            resolution_mode,
            redirected_reference,
        )
    }

    fn release_caches(&self) {
        self.caches.release();
        // A package.json cache that another resolver or a program shares
        // stays.
        if Rc::strong_count(&self.resolution_data) == 1
            && Rc::strong_count(&self.package_json_info_cache) == 1
        {
            self.package_json_info_cache.clear();
        }
    }

    fn as_default_resolver(&self) -> Option<&DefaultResolver> {
        Some(self)
    }
}

// Go: module/types.go:41 ModeAwareCacheKey
// PORT: the derived `Hash` must stay the one of `dyn ModeAwareKey` (name,
// then mode).
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct ModeAwareCacheKey {
    pub name: String,
    pub mode: ResolutionMode,
}

/// A `ModeAwareCache` key by its parts. `cache.get(&(name, mode) as &dyn
/// ModeAwareKey)` looks up a borrowed name with no owned key. No Go
/// counterpart.
pub trait ModeAwareKey {
    fn parts(&self) -> (&str, ResolutionMode);
}

impl ModeAwareKey for ModeAwareCacheKey {
    fn parts(&self) -> (&str, ResolutionMode) {
        (&self.name, self.mode)
    }
}

impl ModeAwareKey for (&str, ResolutionMode) {
    fn parts(&self) -> (&str, ResolutionMode) {
        *self
    }
}

impl<'a> std::borrow::Borrow<dyn ModeAwareKey + 'a> for ModeAwareCacheKey {
    fn borrow(&self) -> &(dyn ModeAwareKey + 'a) {
        self
    }
}

impl std::hash::Hash for dyn ModeAwareKey + '_ {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        let (name, mode) = self.parts();
        name.hash(state);
        mode.hash(state);
    }
}

impl PartialEq for dyn ModeAwareKey + '_ {
    fn eq(&self, other: &Self) -> bool {
        self.parts() == other.parts()
    }
}

impl Eq for dyn ModeAwareKey + '_ {}

// Go: module/types.go:46 ResolvedProjectReference
// PORT: Go `module.ResolvedProjectReference` is an interface. The name
// `ResolvedProjectReference` is already the program.rs struct, so the
// interface is `ModuleResolvedProjectReference`. Go returns a nil
// `*core.CompilerOptions` as `None`. The options are shared (`Rc`), because
// the resolution state compares them by pointer with the resolver options.
pub trait ModuleResolvedProjectReference {
    fn config_name(&self) -> &str;
    fn compiler_options(&self) -> Option<Rc<CompilerOptions>>;
}

// Go: module/types.go:29 NodeResolutionFeatures
crate::flags_macros::go_flags!(NodeResolutionFeatures, i32 {
    IMPORTS = 1 << 0; // NodeResolutionFeaturesImports
    SELF_NAME = 1 << 1; // NodeResolutionFeaturesSelfName
    EXPORTS = 1 << 2; // NodeResolutionFeaturesExports
    EXPORTS_PATTERN_TRAILERS = 1 << 3; // NodeResolutionFeaturesExportsPatternTrailers
    // allowing `#/` root imports in package.json imports field
    // not supported until mass adoption - https://github.com/nodejs/node/pull/60864
    IMPORTS_PATTERN_ROOT = 1 << 4; // NodeResolutionFeaturesImportsPatternRoot

    NONE = 0; // NodeResolutionFeaturesNone
    ALL = (1 << 0) | (1 << 1) | (1 << 2) | (1 << 3) | (1 << 4); // NodeResolutionFeaturesAll
    NODE16_DEFAULT = (1 << 0) | (1 << 1) | (1 << 2) | (1 << 3); // NodeResolutionFeaturesNode16Default
    NODE_NEXT_DEFAULT = (1 << 0) | (1 << 1) | (1 << 2) | (1 << 3) | (1 << 4); // NodeResolutionFeaturesNodeNextDefault
    BUNDLER_DEFAULT = (1 << 0) | (1 << 1) | (1 << 2) | (1 << 3) | (1 << 4); // NodeResolutionFeaturesBundlerDefault
});

// Go: module/types.go:69 PackageId
// PORT: the struct is `program::PackageId`. Its Go methods are here.
impl PackageId {
    // Go: module/types.go:76 PackageId.String
    #[must_use]
    pub fn string(&self) -> String {
        format!(
            "{}@{}{}",
            self.package_name(),
            self.version,
            self.peer_dependencies
        )
    }

    // Go: module/types.go:80 PackageId.PackageName
    #[must_use]
    pub fn package_name(&self) -> String {
        if !self.sub_module_name.is_empty() {
            return format!("{}/{}", self.name, self.sub_module_name);
        }
        self.name.clone()
    }
}

// Go: module/types.go:87 ResolvedModule
// PORT: the struct and `IsResolved` are `program::ResolvedModule`.

// Go: module/types.go:105 ResolvedTypeReferenceDirective
#[derive(Clone, Default)]
pub struct ResolvedTypeReferenceDirective {
    pub resolution_diagnostics: Vec<Diagnostic>,
    pub primary: bool,
    pub resolved_file_name: String,
    pub original_path: String,
    pub package_id: PackageId,
    pub is_external_library_import: bool,
}

impl ResolvedTypeReferenceDirective {
    // Go: module/types.go:115 ResolvedTypeReferenceDirective.IsResolved
    #[must_use]
    pub fn is_resolved(&self) -> bool {
        !self.resolved_file_name.is_empty()
    }
}

// Go: module/types.go:93 extensions
// PORT: Go unexported `extensions`. Other files of the package use it, so
// it is `pub`.
crate::flags_macros::go_flags!(Extensions, i32 {
    TYPE_SCRIPT = 1 << 0; // extensionsTypeScript
    JAVA_SCRIPT = 1 << 1; // extensionsJavaScript
    DECLARATION = 1 << 2; // extensionsDeclaration
    JSON = 1 << 3; // extensionsJson

    IMPLEMENTATION_FILES = (1 << 0) | (1 << 1); // extensionsImplementationFiles
});

impl Extensions {
    // Go: module/types.go:130 extensions.String
    #[must_use]
    pub fn string(self) -> String {
        let mut result: Vec<&str> = Vec::with_capacity(self.0.count_ones() as usize);
        if self.intersects(Extensions::TYPE_SCRIPT) {
            result.push("TypeScript");
        }
        if self.intersects(Extensions::JAVA_SCRIPT) {
            result.push("JavaScript");
        }
        if self.intersects(Extensions::DECLARATION) {
            result.push("Declaration");
        }
        if self.intersects(Extensions::JSON) {
            result.push("JSON");
        }
        result.join(", ")
    }

    // Go: module/types.go:147 extensions.Array
    #[must_use]
    pub fn array(self) -> Vec<String> {
        let mut result: Vec<String> = Vec::new();
        if self.intersects(Extensions::TYPE_SCRIPT) {
            result.extend(
                SUPPORTED_TS_IMPLEMENTATION_EXTENSIONS
                    .iter()
                    .map(|e| (*e).to_string()),
            );
        }
        if self.intersects(Extensions::JAVA_SCRIPT) {
            result.extend(
                SUPPORTED_JS_EXTENSIONS_FLAT
                    .iter()
                    .map(|e| (*e).to_string()),
            );
        }
        if self.intersects(Extensions::DECLARATION) {
            result.extend(
                SUPPORTED_DECLARATION_EXTENSIONS
                    .iter()
                    .map(|e| (*e).to_string()),
            );
        }
        if self.intersects(Extensions::JSON) {
            result.push(EXTENSION_JSON.to_string());
        }
        result
    }
}
