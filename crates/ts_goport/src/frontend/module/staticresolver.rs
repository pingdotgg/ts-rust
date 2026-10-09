//! Go: module/staticresolver.go (ts#64299).

use crate::frontend::prelude::*;
use crate::gostd::{GoError, errors, strconv};
use std::sync::Arc;

// Go: module/staticresolver.go:11 StaticResolutionEntry
// PORT: Go `*core.ResolutionMode` and `*ResolvedModule` are `Option`.
#[derive(Clone, Default)]
pub struct StaticResolutionEntry {
    pub module_name: String,
    pub containing_directory: String,
    pub resolution_mode: Option<ResolutionMode>,
    pub result: Option<Arc<ResolvedModule>>,
}

// Go: module/staticresolver.go:18 staticResolutionKey
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
struct StaticResolutionKey {
    module_name: String,
    directory: Path,
    mode: ResolutionMode,
    has_directory: bool,
    has_mode: bool,
}

// Go: module/staticresolver.go:26 StaticResolutions
pub struct StaticResolutions {
    fallback_to_resolver: bool,
    entries: FxHashMap<StaticResolutionKey, Option<Arc<ResolvedModule>>>,
    current_directory: String,
    use_case_sensitive_file_names: bool,
}

// Go: module/staticresolver.go:33 NewStaticResolutions
pub fn new_static_resolutions(
    entries: &[StaticResolutionEntry],
    fallback_to_resolver: bool,
    current_directory: &str,
    use_case_sensitive_file_names: bool,
) -> Result<StaticResolutions, GoError> {
    let mut resolutions = StaticResolutions {
        fallback_to_resolver,
        entries: FxHashMap::with_capacity_and_hasher(entries.len(), Default::default()),
        current_directory: current_directory.to_string(),
        use_case_sensitive_file_names,
    };
    for entry in entries {
        if entry.module_name.is_empty() {
            return Err(errors::new("module name is empty"));
        }
        let mut key = StaticResolutionKey {
            module_name: entry.module_name.clone(),
            ..Default::default()
        };
        if !entry.containing_directory.is_empty() {
            key.directory = to_path(
                &entry.containing_directory,
                current_directory,
                use_case_sensitive_file_names,
            );
            key.has_directory = true;
        }
        if let Some(resolution_mode) = entry.resolution_mode {
            key.mode = resolution_mode;
            key.has_mode = true;
        }
        if resolutions.entries.contains_key(&key) {
            return Err(errors::errorf(
                format!(
                    "duplicate static module resolution for {}",
                    strconv::quote(&entry.module_name)
                ),
                Vec::new(),
            ));
        }
        resolutions.entries.insert(key, entry.result.clone());
    }
    Ok(resolutions)
}

impl StaticResolutions {
    // Go: module/staticresolver.go:66 lookup
    // PORT: the outer `Option` is Go's `ok`; the inner one is a nil result.
    fn lookup(
        &self,
        module_name: &str,
        containing_directory: &str,
        resolution_mode: ResolutionMode,
    ) -> Option<Option<Arc<ResolvedModule>>> {
        let directory = to_path(
            containing_directory,
            &self.current_directory,
            self.use_case_sensitive_file_names,
        );
        let keys = [
            StaticResolutionKey {
                module_name: module_name.to_string(),
                directory: directory.clone(),
                mode: resolution_mode,
                has_directory: true,
                has_mode: true,
            },
            StaticResolutionKey {
                module_name: module_name.to_string(),
                directory,
                has_directory: true,
                ..Default::default()
            },
            StaticResolutionKey {
                module_name: module_name.to_string(),
                mode: resolution_mode,
                has_mode: true,
                ..Default::default()
            },
            StaticResolutionKey {
                module_name: module_name.to_string(),
                ..Default::default()
            },
        ];
        for key in &keys {
            if let Some(result) = self.entries.get(key) {
                return Some(result.clone());
            }
        }
        None
    }
}

// Go: module/staticresolver.go:82 StaticResolver
pub struct StaticResolver {
    fallback: Rc<dyn Resolver>,
    resolutions: Rc<StaticResolutions>,
}

// Go: module/staticresolver.go:87 NewStaticResolver
#[must_use]
pub fn new_static_resolver(
    fallback: Rc<dyn Resolver>,
    resolutions: Rc<StaticResolutions>,
) -> StaticResolver {
    StaticResolver {
        fallback,
        resolutions,
    }
}

impl StaticResolver {
    // Go: module/staticresolver.go:118 resolveModuleName
    // PORT: named `resolve_module_name_worker`; the Go names
    // `ResolveModuleName` and `resolveModuleName` share a snake name.
    fn resolve_module_name_worker(
        &self,
        module_name: &str,
        containing_file: &str,
        containing_directory: &str,
        resolution_mode: ResolutionMode,
        redirected_reference: Option<&dyn ModuleResolvedProjectReference>,
    ) -> (
        Option<Arc<ResolvedModule>>,
        Vec<DiagAndArgs>,
        Option<GoError>,
    ) {
        if let Some(result) =
            self.resolutions
                .lookup(module_name, containing_directory, resolution_mode)
        {
            return (result, Vec::new(), None);
        }
        if !self.resolutions.fallback_to_resolver {
            return (None, Vec::new(), None);
        }
        self.fallback.resolve_module_name(
            module_name,
            containing_file,
            resolution_mode,
            redirected_reference,
        )
    }
}

impl Resolver for StaticResolver {
    // Go: module/staticresolver.go:95 ResolveModuleName
    fn resolve_module_name(
        &self,
        module_name: &str,
        containing_file: &str,
        resolution_mode: ResolutionMode,
        redirected_reference: Option<&dyn ModuleResolvedProjectReference>,
    ) -> (
        Option<Arc<ResolvedModule>>,
        Vec<DiagAndArgs>,
        Option<GoError>,
    ) {
        self.resolve_module_name_worker(
            module_name,
            containing_file,
            &get_directory_path(containing_file),
            resolution_mode,
            redirected_reference,
        )
    }

    // Go: module/staticresolver.go:104 ResolveModuleNameFromDirectory
    fn resolve_module_name_from_directory(
        &self,
        module_name: &str,
        containing_directory: &str,
        resolution_mode: ResolutionMode,
    ) -> (
        Option<Arc<ResolvedModule>>,
        Vec<DiagAndArgs>,
        Option<GoError>,
    ) {
        if let Some(result) =
            self.resolutions
                .lookup(module_name, containing_directory, resolution_mode)
        {
            return (result, Vec::new(), None);
        }
        if !self.resolutions.fallback_to_resolver {
            return (None, Vec::new(), None);
        }
        self.fallback.resolve_module_name_from_directory(
            module_name,
            containing_directory,
            resolution_mode,
        )
    }

    // Go: module/staticresolver.go:134 ResolveTypeReferenceDirective
    fn resolve_type_reference_directive(
        &self,
        type_reference_directive_name: &str,
        containing_file: &str,
        resolution_mode: ResolutionMode,
        redirected_reference: Option<&dyn ModuleResolvedProjectReference>,
    ) -> (Rc<ResolvedTypeReferenceDirective>, Vec<DiagAndArgs>) {
        self.fallback.resolve_type_reference_directive(
            type_reference_directive_name,
            containing_file,
            resolution_mode,
            redirected_reference,
        )
    }

    // Go: module/staticresolver.go:143 GetResolutionData (ts#64519)
    fn get_resolution_data(&self) -> Rc<ResolutionData> {
        self.fallback.get_resolution_data()
    }

    // Go: module/staticresolver.go:140 GetPackageScopeForPath (at 673a5f17d713; removed by ts#64519)
    // PORT: kept with `Resolver::get_package_scope_for_path`.
    fn get_package_scope_for_path(&self, directory: &str) -> Option<Rc<InfoCacheEntry>> {
        self.fallback.get_package_scope_for_path(directory)
    }

    // Go: module/staticresolver.go:144 PackageJsonCacheEntries (at 673a5f17d713; removed by ts#64519)
    // PORT: kept with the `Resolver` method.
    fn package_json_cache_entries(
        &self,
        f: &mut dyn FnMut(&Path, PackageJsonCacheEntry<'_>) -> bool,
    ) {
        self.fallback.package_json_cache_entries(f);
    }

    // Go: module/staticresolver.go:148 ResolvePackageDirectory (at 673a5f17d713; removed by ts#64519)
    // PORT: kept with the `Resolver` method.
    fn resolve_package_directory(
        &self,
        module_name: &str,
        containing_file: &str,
        resolution_mode: ResolutionMode,
        redirected_reference: Option<&dyn ModuleResolvedProjectReference>,
    ) -> Option<ResolvedModule> {
        self.fallback.resolve_package_directory(
            module_name,
            containing_file,
            resolution_mode,
            redirected_reference,
        )
    }

    // PORT: not in Go (see `Resolver::release_caches`).
    fn release_caches(&self) {
        self.fallback.release_caches();
    }
}
