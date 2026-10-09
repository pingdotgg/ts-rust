use crate::ls::autoimport::prelude::*;

// Port of Go `ls/autoimport/registry.go`.
//
// PORT (whole file):
// - Concurrency. `sync.WaitGroup` / `wg.Go` run each task at its start
//   point, serially in Go start order (map-project decision 1); the
//   `ctx.Err()` checks stay. Each task runs under
//   `core::go_wait_group_goroutine`, so a Go panic in it ends the process
//   as Go's goroutine does, and the request's recover does not catch it. A
//   build that the caller drops on cancel has more stop points
//   (`should_stop_build`). Mutexes are dropped. `atomic.Int32` is `Cell`.
// - Go map iteration that feeds the index (`exports` maps, the second-pass
//   root files and lookup sources) uses `IndexMap` / `IndexSet` in insertion
//   order. PORT: Go map order is random. Index order decides ties in the
//   completion sort, and also which path a merged completion export keeps,
//   which can change module specifiers through the specifier cache (see
//   `View::get_completions` in `view.rs`). Other Go maps are `FxHashMap` /
//   `FxHashSet`; their order only reaches logs or sets.
// - Pointers. `*Registry`, `*RegistryBucket`, `*Export`,
//   `*module.ResolvedEntrypoint` are `Rc`. `*directory` is
//   `Rc<RefCell<Directory>>` (mutated through `dirty.Map` changes). A bucket
//   changes after it is shared only in `state` (a `RefCell`) and in its
//   index during the second pass (`Rc<RefCell<Index>>`). Go `Clone()` is
//   `clone_()`.
// - Go `*collections.Set[string]` is `Option<FxHashSet<String>>` (`None` is
//   nil; the nil-safe Go methods are the `set_*` helpers below). Buckets hold
//   shared sets and maps behind `Rc`, as Go shares the pointers.
// - Go `*packagejson.InfoCacheEntry` is `Option<Rc<packagejson::InfoCacheEntry>>`;
//   a Go dereference of nil panics with the Go runtime text.
// - Go `*logging.LogTree` is `Option<Rc<logging::LogTree>>`. Log text uses
//   `{:?}` for Go `%v` durations (log text is not compared).
// - Go `*ast.SourceFile` from `program.GetSourceFiles()` is `&ParsedSourceFile`
//   where only `FileName()` and `Path()` are read, and `file.root` where a
//   node is needed.

use crate::core::go_wait_group_goroutine;
use crate::flags_macros::go_enum;
use crate::frontend::compiler;
use crate::frontend::core_ext::HasFileName;
use crate::frontend::core_ls_ext::{diff_maps_func, unordered_equal};
use crate::frontend::module;
use crate::frontend::module::ResolutionHost as _;
use crate::frontend::packagejson;
use crate::frontend::parser::ParsedSourceFile;
use crate::frontend::tspath;
use crate::frontend::vfs;
use crate::frontend::vfs::Fs as _;
use crate::gostd::context::ContextKey;
use crate::gostd::{Context, GoError};
use crate::ls::{lsconv, lsutil};
use crate::lsp::lsproto;
use crate::modulespecifiers::symlinks::KnownSymlinks;
use crate::program::ls_program;
use crate::project::logging::{LogTreeMethods as _, Logger as _};
use crate::project::{dirty, logging};
use std::sync::LazyLock;
use std::time::Instant;

// Go: ls/autoimport/registry.go:32 ProjectID
// PORT: Go `ProjectID` is an interface (`fmt.Stringer`) that the project
// package fills with its string type `project.ID`; interface map keys compare
// by that value. Every value is a `project.ID`, so the Rust type holds its
// string: the project package passes `ProjectID(id.to_string())`. A Go nil
// `ProjectID` is `None`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ProjectID(pub String);

impl ProjectID {
    /// Go `fmt.Stringer.String`.
    #[must_use]
    pub fn string(&self) -> String {
        self.0.clone()
    }
}

/// Go `%s` / `%v` of a `ProjectID` is its `String()`.
impl std::fmt::Display for ProjectID {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

// Go: ls/autoimport/registry.go:36 knownRecursiveSearchPackages
pub static KNOWN_RECURSIVE_SEARCH_PACKAGES: LazyLock<FxHashSet<&'static str>> =
    LazyLock::new(|| {
        [
            "@material-ui/core",
            "@material-ui/icons",
            "@sap/cds",
            "@testing-library/react-native",
            "ajv",
            "asap",
            "async",
            "aws-sdk",
            "braintree-web",
            "core-js",
            "core-js-pure",
            "crypto-js",
            "cypress-mochawesome-reporter",
            "dd-trace",
            "dumi",
            "dva",
            "egg-mock",
            "electron-log",
            "es-abstract",
            "es6-promise",
            "eslint-config-taro",
            "expo",
            "expo-router",
            "flow-remove-types",
            "gatsby",
            "glamor",
            "gluegun",
            "graphology-indices",
            "graphology-traversal",
            "graphology-utils",
            "jest-expo",
            "lodash",
            "lodash-es",
            "moment",
            "mz",
            "next",
            "pdfjs-dist",
            "protobufjs",
            "react-app-polyfill",
            "react-dev-utils",
            "react-devtools-inline",
            "recast",
            "semver",
            "stylelint-config-html",
            "umi",
            "web3-provider-engine",
            "webpack",
        ]
        .into_iter()
        .collect()
    });

// Go: ls/autoimport/registry.go:86 newProgramStructure
go_enum!(NewProgramStructure, i32 {
    FALSE = 0; // newProgramStructureFalse
    SAME_FILE_NAMES = 1; // newProgramStructureSameFileNames
    DIFFERENT_FILE_NAMES = 2; // newProgramStructureDifferentFileNames
});

// ---------------------------------------------------------------------------
// Go `collections.Set` methods on a possibly nil set.
// ---------------------------------------------------------------------------

// Go: collections/set.go:16 Has
fn set_has(s: Option<&FxHashSet<String>>, key: &str) -> bool {
    match s {
        None => false,
        Some(s) => s.contains(key),
    }
}

// Go: collections/set.go:35 Len
fn set_len(s: Option<&FxHashSet<String>>) -> usize {
    s.map_or(0, |s| s.len())
}

// Go: collections/set.go:104 Equals
// PORT: Go first compares the pointers; equal pointers hold equal sets.
fn set_equals(s: Option<&FxHashSet<String>>, other: Option<&FxHashSet<String>>) -> bool {
    match (s, other) {
        (None, None) => true,
        (Some(s), Some(other)) => s == other,
        _ => false,
    }
}

// Go: collections/set.go:114 IsSubsetOf
fn set_is_subset_of(s: Option<&FxHashSet<String>>, other: Option<&FxHashSet<String>>) -> bool {
    let Some(s) = s else {
        return true;
    };
    for key in s {
        if !set_has(other, key) {
            return false;
        }
    }
    true
}

/// Go copies `module.ResolverOptions` by value.
// PORT: the copy clones each field (the `Rc` values are shared, as Go
// shares the pointers).
fn copy_resolver_options(opts: &module::ResolverOptions) -> module::ResolverOptions {
    module::ResolverOptions {
        host: opts.host.clone(),
        compiler_options: opts.compiler_options.clone(),
        typings_location: opts.typings_location.clone(),
        project_name: opts.project_name.clone(),
        extra_extensions: opts.extra_extensions.clone(),
        package_json_cache: opts.package_json_cache.clone(),
    }
}

// Go: ls/autoimport/registry.go:98 bucketBuildPreferences
// bucketBuildPreferences holds user preferences that affect how a bucket is
// built. When any of these change between builds, the bucket must be rebuilt.
// Adding a new preference here automatically integrates it into the rebuild
// checks via Equal.
#[derive(Clone, Debug, Default)]
pub struct BucketBuildPreferences {
    pub file_exclude_patterns: Vec<String>,
    pub auto_import_entrypoint_directory_search: Tristate,
}

// Go: ls/autoimport/registry.go:103 bucketBuildPreferencesFromUserPreferences
pub fn bucket_build_preferences_from_user_preferences(
    prefs: &lsutil::UserPreferences,
) -> BucketBuildPreferences {
    BucketBuildPreferences {
        file_exclude_patterns: prefs.auto_import_file_exclude_patterns.clone(),
        auto_import_entrypoint_directory_search: prefs.auto_import_entrypoint_directory_search,
    }
}

impl BucketBuildPreferences {
    // Go: ls/autoimport/registry.go:110 Equal
    pub fn equal(&self, other: &BucketBuildPreferences) -> bool {
        unordered_equal(&self.file_exclude_patterns, &other.file_exclude_patterns)
            && self.auto_import_entrypoint_directory_search
                == other.auto_import_entrypoint_directory_search
    }

    // Go: ls/autoimport/registry.go:110 Clone
    pub fn clone_(&self) -> BucketBuildPreferences {
        let mut p = self.clone();
        p.file_exclude_patterns = self.file_exclude_patterns.clone();
        p
    }
}

// Go: ls/autoimport/registry.go:129 BucketState
// BucketState represents the dirty state of a bucket.
// In general, a bucket can be used for an auto-imports request if it is clean
// or if the only edited file is the one that was requested for auto-imports.
// Most edits within a file will not change the imports available to that file.
// However, one exception causes the bucket to be rebuilt after a change to a
// single file: local files are newly added to the project by a manual import.
// This can only happen after a full (non-clone) program update. When this
// happens, the `newProgramStructure` flag is set until the next time the bucket
// is rebuilt, when this condition will be checked.
// PORT: the Go methods `DirtyFile`, `DirtyPackages` and
// `RecursiveSearchPackages` have the snake names of fields of the same type,
// so they end in `_exported` (PORTING "Names").
#[derive(Clone, Debug, Default)]
pub struct BucketState {
    // dirtyFile is the file that was edited last, if any. It does not necessarily
    // indicate that no other files have been edited, so it should be ignored if
    // `multipleFilesDirty` is set. It should not be used for node_modules buckets,
    // which rely on `dirtyPackages` instead.
    pub dirty_file: tspath::Path,
    pub multiple_files_dirty: bool,
    pub new_program_structure: NewProgramStructure,
    // buildPreferences holds the user preferences that were in effect when
    // the bucket was built. If changed, the bucket should be rebuilt.
    pub build_preferences: BucketBuildPreferences,
    // dirtyPackages is the set of package names that need to be re-indexed.
    // This is used for granular updates: when a file in a local workspace package
    // changes, only that package needs to be re-extracted rather than rebuilding
    // the entire node_modules bucket.
    // If nil, no granular updates are pending.
    // If set but multipleFilesDirty is true, the entire bucket needs to be rebuilt.
    pub dirty_packages: Option<FxHashSet<String>>,
    // recursiveSearchPackages tracks which packages were recursively directory-searched
    // when the bucket was built. nil means all non-exports packages were searched
    // (e.g. when the autoImportEntrypointDirectorySearch preference is enabled).
    // A non-nil set lists only the specific packages that were searched.
    // Used for rebuild detection: a rebuild is triggered when target packages are
    // not a subset of the currently searched packages.
    pub recursive_search_packages: Option<FxHashSet<String>>,
}

impl BucketState {
    // Go: ls/autoimport/registry.go:151 Clone
    pub fn clone_(&self) -> BucketState {
        let mut b = self.clone();
        b.build_preferences = self.build_preferences.clone_();
        b.dirty_packages = self.dirty_packages.clone();
        b.recursive_search_packages = self.recursive_search_packages.clone();
        b
    }

    // Go: ls/autoimport/registry.go:163 Dirty
    pub fn dirty(&self) -> bool {
        self.multiple_files_dirty
            || !self.dirty_file.is_empty()
            || self.new_program_structure.0 > 0
            || set_len(self.dirty_packages.as_ref()) > 0
    }

    // Go: ls/autoimport/registry.go:167 DirtyFile
    pub fn dirty_file_exported(&self) -> tspath::Path {
        if self.multiple_files_dirty {
            return tspath::Path::default();
        }
        self.dirty_file.clone()
    }

    // Go: ls/autoimport/registry.go:174 DirtyPackages
    pub fn dirty_packages_exported(&self) -> Option<&FxHashSet<String>> {
        if self.multiple_files_dirty {
            return None;
        }
        self.dirty_packages.as_ref()
    }

    // Go: ls/autoimport/registry.go:181 RecursiveSearchPackages
    pub fn recursive_search_packages_exported(&self) -> Option<&FxHashSet<String>> {
        self.recursive_search_packages.as_ref()
    }

    // Go: ls/autoimport/registry.go:185 possiblyNeedsRebuildForFile
    pub fn possibly_needs_rebuild_for_file(
        &self,
        file: &tspath::Path,
        preferences: &lsutil::UserPreferences,
    ) -> bool {
        self.new_program_structure.0 > 0
            || self.has_dirty_file_besides(file)
            || !self
                .build_preferences
                .equal(&bucket_build_preferences_from_user_preferences(preferences))
            || set_len(self.dirty_packages.as_ref()) > 0
    }

    // Go: ls/autoimport/registry.go:192 hasDirtyFileBesides
    pub fn has_dirty_file_besides(&self, file: &tspath::Path) -> bool {
        self.multiple_files_dirty || !self.dirty_file.is_empty() && self.dirty_file != *file
    }
}

// Go: ls/autoimport/registry.go:200 recursiveSearchSubset
// recursiveSearchSubset reports whether target is a subset of current.
// nil represents "all packages" — a superset of every concrete set.
// Returns true if the current set already covers everything the target needs,
// meaning no rebuild is required for recursive search purposes.
pub fn recursive_search_subset(
    target: Option<&FxHashSet<String>>,
    current: Option<&FxHashSet<String>>,
) -> bool {
    if target.is_none() {
        // Target wants all packages searched — only satisfied if current is also all.
        return current.is_none();
    }
    if current.is_none() {
        // Current searched all packages, so any concrete target is satisfied.
        return true;
    }
    set_is_subset_of(target, current)
}

// Go: ls/autoimport/registry.go:212 RegistryBucket
// PORT: see the file header. Go nil maps that are only read are empty maps
// (`paths`, `ambient_module_names`); `package_files` keeps nil (`None`)
// because Go tests it.
#[derive(Default)]
pub struct RegistryBucket {
    pub state: RefCell<BucketState>,

    // Paths maps file paths to package names. For project buckets, the package name
    // is always empty string. For node_modules buckets, this enables reverse lookup
    // from path to package for granular updates. Only paths for local workspace
    // packages (symlinked and within the workspace root) have entries here, since
    // their realpaths are outside node_modules and need reverse lookup for dirty
    // detection.
    //
    // Paths is considered immutable after the bucket is finalized.
    // It should be fully replaced rather than mutated while changing a bucket.
    pub paths: Rc<FxHashMap<tspath::Path, String>>,
    // PackageFiles maps package names to their file paths and file names.
    // All package directory names in node_modules are keys; indexed packages have
    // non-nil maps with path→fileName entries, unindexed packages have nil maps.
    // This enables efficient removal of a package's files during granular updates
    // without iterating through all entries. Only defined for node_modules buckets.
    //
    // PackageFiles is considered immutable after the bucket is finalized.
    // It should be fully replaced rather than mutated while changing a bucket.
    pub package_files: Option<Rc<FxHashMap<String, Option<FxHashMap<tspath::Path, String>>>>>,
    // ResolvedPackageNames is only defined for project buckets. It is the set of
    // package names that were resolved from imports in the project's program files.
    // This is passed to node_modules buckets so they include packages that are
    // directly imported even if not listed in package.json dependencies.
    //
    // ResolvedPackageNames is considered immutable after the bucket is finalized.
    // It should be fully replaced rather than mutated while changing a bucket.
    pub resolved_package_names: Option<Rc<FxHashSet<String>>>,
    // DependencyNames is only defined for node_modules buckets. It is the set of
    // package names that will be included in the bucket if present in the directory,
    // computed from package.json dependencies plus resolved package names from
    // active programs. If nil, all packages are included because at least one open
    // file has access to this node_modules directory without being filtered by a
    // package.json.
    //
    // DependencyNames is considered immutable after the bucket is finalized.
    // It should be fully replaced rather than mutated while changing a bucket.
    pub dependency_names: Option<Rc<FxHashSet<String>>>,
    // AmbientModuleNames is only defined for node_modules buckets. It is the set of
    // ambient module names found while extracting exports in the bucket.
    //
    // AmbientModuleNames is considered immutable after the bucket is finalized.
    // It should be fully replaced rather than mutated while changing a bucket.
    pub ambient_module_names: Rc<FxHashMap<String, Vec<String>>>,
    // Index is considered immutable after the bucket is finalized.
    // It should be cloned and replaced rather than mutated while changing a bucket.
    pub index: Option<Rc<RefCell<Index<Rc<Export>>>>>,
}

// Go: ls/autoimport/registry.go:263 newRegistryBucket
pub fn new_registry_bucket() -> Rc<RegistryBucket> {
    Rc::new(RegistryBucket {
        state: RefCell::new(BucketState {
            multiple_files_dirty: true,
            new_program_structure: NewProgramStructure::DIFFERENT_FILE_NAMES,
            ..Default::default()
        }),
        ..Default::default()
    })
}

impl RegistryBucket {
    // Go: ls/autoimport/registry.go:267 Clone
    pub fn clone_(&self) -> Rc<RegistryBucket> {
        Rc::new(RegistryBucket {
            state: RefCell::new(self.state.borrow().clone_()),
            paths: self.paths.clone(),
            package_files: self.package_files.clone(),
            resolved_package_names: self.resolved_package_names.clone(),
            dependency_names: self.dependency_names.clone(),
            ambient_module_names: self.ambient_module_names.clone(),
            index: self.index.clone(),
        })
    }

    // Go: ls/autoimport/registry.go:287 markProjectFileDirty
    // markProjectFileDirty should only be called within a Change call on the dirty map.
    // Buckets are considered immutable once in a finalized registry. Should only
    // be used for project buckets.
    pub fn mark_project_file_dirty(&self, file: &tspath::Path) {
        let mut state = self.state.borrow_mut();
        if state.has_dirty_file_besides(file) {
            state.multiple_files_dirty = true;
        } else {
            state.dirty_file = file.clone();
        }
    }

    // Go: ls/autoimport/registry.go:299 markNodeModulesDirty
    // markNodeModulesDirty should only be called within a Change call on the dirty map.
    // Buckets are considered immutable once in a finalized registry. If packageName is
    // non-empty, that package is marked for granular update. Otherwise, the entire bucket
    // is marked dirty.
    pub fn mark_node_modules_dirty(&self, package_name: &str) {
        let mut state = self.state.borrow_mut();
        if state.multiple_files_dirty {
            return;
        }
        if package_name.is_empty() {
            state.multiple_files_dirty = true;
            return;
        }
        // Track the package for granular updates
        if state.dirty_packages.is_none() {
            state.dirty_packages = Some(FxHashSet::default());
        }
        state
            .dirty_packages
            .as_mut()
            .unwrap()
            .insert(package_name.to_string());
    }
}

impl dirty::Cloneable for Rc<RegistryBucket> {
    fn clone_(&self) -> Self {
        RegistryBucket::clone_(self)
    }
}

// Go: ls/autoimport/registry.go:314 directory
#[derive(Clone, Debug, Default)]
pub struct Directory {
    pub name: String,
    pub package_json: Option<Rc<packagejson::InfoCacheEntry>>,
    pub has_node_modules: bool,
}

impl Directory {
    // Go: ls/autoimport/registry.go:315 Clone
    pub fn clone_(&self) -> Rc<RefCell<Directory>> {
        Rc::new(RefCell::new(Directory {
            name: self.name.clone(),
            package_json: self.package_json.clone(),
            has_node_modules: self.has_node_modules,
        }))
    }
}

impl dirty::Cloneable for Rc<RefCell<Directory>> {
    fn clone_(&self) -> Self {
        RefCell::borrow(self).clone_()
    }
}

// Go: ls/autoimport/registry.go:328 Registry
// PORT: Go `func(fileName string) tspath.Path` is `Rc<dyn Fn(&str) -> tspath::Path>`.
// Go `*collections.SyncMap[tspath.Path, string]` specifier caches are
// `Rc<RefCell<FxHashMap<tspath::Path, String>>>` (shared between registries).
pub struct Registry {
    pub to_path: Rc<dyn Fn(&str) -> tspath::Path>,
    pub user_preferences: lsutil::UserPreferences,

    // exports      map[tspath.Path][]*RawExport
    pub directories: FxHashMap<tspath::Path, Rc<RefCell<Directory>>>,

    pub node_modules: FxHashMap<tspath::Path, Rc<RegistryBucket>>,
    pub projects: FxHashMap<ProjectID, Rc<RegistryBucket>>,
    pub unique_package_count: i32,

    // entrypoints maps from file path to the resolved entrypoints for that file, shared across all node_modules buckets.
    pub entrypoints: FxHashMap<tspath::Path, Vec<Rc<module::ResolvedEntrypoint>>>,

    // specifierCache maps from importing file to target file to specifier.
    pub specifier_cache: FxHashMap<tspath::Path, Rc<RefCell<FxHashMap<tspath::Path, String>>>>,
}

// Go: ls/autoimport/registry.go:346 NewRegistry
// PORT: map-ls-completions 2.5 returns the value; callers share it as
// `Rc<Registry>`.
pub fn new_registry(
    to_path: Rc<dyn Fn(&str) -> tspath::Path>,
    preferences: lsutil::UserPreferences,
) -> Registry {
    Registry {
        to_path,
        user_preferences: preferences,
        directories: FxHashMap::default(),
        node_modules: FxHashMap::default(),
        projects: FxHashMap::default(),
        unique_package_count: 0,
        entrypoints: FxHashMap::default(),
        specifier_cache: FxHashMap::default(),
    }
}

impl Registry {
    // Go: ls/autoimport/registry.go:354 IsPreparedForImportingFile
    // PORT: Go allows a nil receiver; `r` is `None` for it.
    pub fn is_prepared_for_importing_file(
        r: Option<&Registry>,
        file_name: &str,
        project_id: &ProjectID,
        preferences: &lsutil::UserPreferences,
    ) -> bool {
        let Some(r) = r else {
            return false;
        };
        let Some(project_bucket) = r.projects.get(project_id) else {
            return false;
        };
        let path = (r.to_path)(file_name);
        if project_bucket
            .state
            .borrow()
            .possibly_needs_rebuild_for_file(&path, preferences)
        {
            return false;
        }

        let mut dir_path = path.get_directory_path();
        loop {
            if let Some(dir_bucket) = r.node_modules.get(&dir_path) {
                if dir_bucket
                    .state
                    .borrow()
                    .possibly_needs_rebuild_for_file(&path, preferences)
                {
                    return false;
                }
            }
            let parent = dir_path.get_directory_path();
            if parent == dir_path {
                break;
            }
            dir_path = parent;
        }
        true
    }

    // Go: ls/autoimport/registry.go:383 NodeModulesDirectories
    pub fn node_modules_directories(&self) -> FxHashMap<tspath::Path, String> {
        let mut dirs: FxHashMap<tspath::Path, String> = FxHashMap::default();
        for (dir_path, dir) in &self.directories {
            let dir = dir.borrow();
            if dir.has_node_modules {
                dirs.insert(
                    tspath::Path(tspath::combine_paths(dir_path, &["node_modules"])),
                    tspath::combine_paths(&dir.name, &["node_modules"]),
                );
            }
        }
        dirs
    }

    // Go: ls/autoimport/registry.go:388 Clone
    // PORT: `clone_` because `Clone::clone` is taken (map-ls-completions 2.5).
    pub fn clone_(
        self: &Rc<Self>,
        ctx: &Context,
        change: RegistryChange,
        host: Rc<dyn RegistryCloneHost>,
        logger: Option<Rc<logging::LogTree>>,
    ) -> Result<Rc<Registry>, GoError> {
        let start = Instant::now();
        let mut logger = logger;
        if logger.is_some() {
            logger = logger.fork("Building autoimport registry");
        }
        let mut builder = new_registry_builder(self.clone(), host);
        if let Some(user_preferences) = &change.user_preferences {
            builder.user_preferences = user_preferences.clone();
            if !unordered_equal(
                &builder
                    .user_preferences
                    .auto_import_specifier_exclude_regexes,
                &self.user_preferences.auto_import_specifier_exclude_regexes,
            ) {
                builder.specifier_cache.clear();
            }
        }
        builder.update_bucket_and_directory_existence(&change, &logger);
        builder.mark_buckets_dirty(&change, &logger);
        if !change.requested_file.is_empty() {
            builder.update_indexes(ctx, &change, &logger);
        }
        if logger.is_some() {
            logger.logf(&format!(
                "Built autoimport registry in {:?}",
                start.elapsed()
            ));
        }
        let registry = builder.build();
        Ok(registry)
    }
}

// Go: ls/autoimport/registry.go:417 BucketStats
#[derive(Clone, Debug, Default)]
pub struct BucketStats {
    pub name: String,
    pub export_count: i32,
    pub file_count: i32,
    pub state: BucketState,
    pub dependency_names: Option<Rc<FxHashSet<String>>>,
    pub package_names: Option<FxHashSet<String>>,
}

// Go: ls/autoimport/registry.go:426 CacheStats
#[derive(Clone, Debug, Default)]
pub struct CacheStats {
    pub project_buckets: Vec<BucketStats>,
    pub node_modules_buckets: Vec<BucketStats>,
    pub unique_package_count: i32,
}

impl Registry {
    // Go: ls/autoimport/registry.go:432 GetCacheStats
    // PORT: Go returns `*CacheStats`; the port returns the value.
    pub fn get_cache_stats(&self) -> CacheStats {
        let mut stats = CacheStats {
            unique_package_count: self.unique_package_count,
            ..Default::default()
        };

        for (project_id, bucket) in &self.projects {
            let mut export_count = 0;
            if let Some(index) = &bucket.index {
                export_count = index.borrow().entries.len() as i32;
            }
            stats.project_buckets.push(BucketStats {
                name: project_id.string(),
                export_count,
                file_count: bucket.paths.len() as i32,
                state: bucket.state.borrow().clone(),
                dependency_names: bucket.dependency_names.clone(),
                package_names: None,
            });
        }

        for (path, bucket) in &self.node_modules {
            let mut export_count = 0;
            if let Some(index) = &bucket.index {
                export_count = index.borrow().entries.len() as i32;
            }
            // Derive PackageNames from PackageFiles keys
            let mut package_names: Option<FxHashSet<String>> = None;
            let mut file_count = 0;
            if let Some(package_files) = &bucket.package_files {
                let mut names: FxHashSet<String> = FxHashSet::default();
                names.reserve(package_files.len());
                for (name, paths) in package_files.iter() {
                    names.insert(name.clone());
                    file_count += paths.as_ref().map_or(0, |p| p.len()) as i32;
                }
                package_names = Some(names);
            }
            stats.node_modules_buckets.push(BucketStats {
                name: path.as_str().to_string(),
                export_count,
                file_count,
                state: bucket.state.borrow().clone(),
                dependency_names: bucket.dependency_names.clone(),
                package_names,
            });
        }

        // Go: cmp.Compare(a.Name, b.Name)
        let compare_paths = |a: &BucketStats, b: &BucketStats| -> i32 {
            match a.name.cmp(&b.name) {
                std::cmp::Ordering::Less => -1,
                std::cmp::Ordering::Equal => 0,
                std::cmp::Ordering::Greater => 1,
            }
        };
        crate::gostd::slices::sort_func(&mut stats.project_buckets, compare_paths);
        crate::gostd::slices::sort_func(&mut stats.node_modules_buckets, compare_paths);

        stats
    }
}

// Go: ls/autoimport/registry.go:486 RegistryChange
// PORT: Go `collections.Set[lsproto.DocumentUri]` values are `FxHashSet`;
// Go `*lsutil.UserPreferences` is `Option` (nil is `None`).
#[derive(Clone, Debug, Default)]
pub struct RegistryChange {
    pub requested_file: tspath::Path,
    // ts#64554: the file name of `requested_file`; its directories are needed
    // (registry.go:488, :590).
    pub requested_file_name: String,
    pub open_files: FxHashMap<tspath::Path, String>,
    pub changed: FxHashSet<lsproto::DocumentUri>,
    pub created: FxHashSet<lsproto::DocumentUri>,
    pub deleted: FxHashSet<lsproto::DocumentUri>,
    // RebuiltPrograms maps from project ID to:
    //   - true: the program was rebuilt with a different set of file names
    //   - false: the program was rebuilt but the set of file names is unchanged
    pub rebuilt_programs: FxHashMap<ProjectID, bool>,
    pub user_preferences: Option<lsutil::UserPreferences>,
}

/// PORT: marks a registry build whose result the caller drops when the
/// context is cancelled: the auto-import warm (`Session::run_pending_warm`,
/// Go session.go:2111). No Go counterpart. Go runs the warm on a goroutine,
/// so no request waits for it. Here it runs on the dispatch thread, so a
/// cancelled warm must stop within about a millisecond. A build without the
/// key (a request) keeps Go's cancel points, because Go adopts its clone
/// even when the request was cancelled.
pub static DISCARD_ON_CANCEL_KEY: ContextKey<()> = ContextKey::new("discardOnCancelKey");

/// PORT: true when a build marked with `DISCARD_ON_CANCEL_KEY` has a
/// cancelled context. The build then returns at once. It leaves only
/// builder state, which the caller drops. `err` is read first, so a build
/// that is not cancelled pays one atomic load, as for Go's checks.
pub fn should_stop_build(ctx: &Context) -> bool {
    ctx.err().is_some() && ctx.value(&DISCARD_ON_CANCEL_KEY).is_some()
}

// Go: ls/autoimport/registry.go:500 RegistryCloneHost
// PORT: Go embeds `module.ResolutionHost` and repeats its `FS()`; both come
// from the supertrait (`fs`, `get_current_directory`). Go
// `*compiler.Program` is `Rc<compiler::NewProgram>` (nil is `None`), Go
// `*packagejson.InfoCacheEntry` is `Option<Rc<..>>`, and Go `*ast.SourceFile`
// is `Node` (`Node::NIL` for nil). The project area implements it
// (`autoImportRegistryCloneHost`).
// Go nil `ProjectID` is `None`.
pub trait RegistryCloneHost: module::ResolutionHost {
    fn get_default_project(
        &self,
        path: &tspath::Path,
    ) -> (Option<ProjectID>, Option<Rc<compiler::NewProgram>>);
    fn get_program_for_project(&self, project_id: &ProjectID) -> Option<Rc<compiler::NewProgram>>;
    fn get_package_json(&self, file_name: &str) -> Option<Rc<packagejson::InfoCacheEntry>>;
    fn get_source_file(&self, file_name: &str, path: &tspath::Path) -> Node;
    fn dispose(&self);
}

// Go: ls/autoimport/registry.go:510 registryBuilder
// PORT: Go `*dirty.Map` / `*dirty.MapBuilder` are the `Rc` handles that
// `dirty::new_map` / `dirty::new_map_builder` return.
pub struct RegistryBuilder {
    pub host: Rc<dyn RegistryCloneHost>,
    pub base: Rc<Registry>,

    pub user_preferences: lsutil::UserPreferences,
    pub directories: Rc<dirty::Map<tspath::Path, Rc<RefCell<Directory>>>>,
    pub node_modules: Rc<dirty::Map<tspath::Path, Rc<RegistryBucket>>>,
    pub projects: Rc<dirty::Map<ProjectID, Rc<RegistryBucket>>>,
    pub specifier_cache: Rc<
        dirty::MapBuilder<
            tspath::Path,
            Rc<RefCell<FxHashMap<tspath::Path, String>>>,
            Rc<RefCell<FxHashMap<tspath::Path, String>>>,
        >,
    >,
    pub resolver_options: module::ResolverOptions,

    pub unique_package_count: i32,
    pub entrypoints: Rc<
        dirty::MapBuilder<
            tspath::Path,
            Vec<Rc<module::ResolvedEntrypoint>>,
            Vec<Rc<module::ResolvedEntrypoint>>,
        >,
    >,
}

// Go: ls/autoimport/registry.go:525 newRegistryBuilder
// PORT: the dirty maps copy their base maps (see `dirty::new_map`); Go shares them.
pub fn new_registry_builder(
    registry: Rc<Registry>,
    host: Rc<dyn RegistryCloneHost>,
) -> RegistryBuilder {
    RegistryBuilder {
        host,
        user_preferences: registry.user_preferences.clone(),
        directories: dirty::new_map(registry.directories.clone()),
        node_modules: dirty::new_map(registry.node_modules.clone()),
        projects: dirty::new_map(registry.projects.clone()),
        // Go: core.Identity, core.Identity
        specifier_cache: dirty::new_map_builder(
            registry.specifier_cache.clone(),
            |v: Rc<RefCell<FxHashMap<tspath::Path, String>>>| v,
            |v: Rc<RefCell<FxHashMap<tspath::Path, String>>>| v,
        ),
        resolver_options: module::ResolverOptions::default(),
        unique_package_count: registry.unique_package_count,
        entrypoints: dirty::new_map_builder(
            registry.entrypoints.clone(),
            |v: Vec<Rc<module::ResolvedEntrypoint>>| v,
            |v: Vec<Rc<module::ResolvedEntrypoint>>| v,
        ),
        base: registry,
    }
}

impl RegistryBuilder {
    // Go: ls/autoimport/registry.go:540 Build
    pub fn build(&self) -> Rc<Registry> {
        Rc::new(Registry {
            to_path: self.base.to_path.clone(),
            user_preferences: self.user_preferences.clone(),
            directories: self.directories.finalize().0,
            node_modules: self.node_modules.finalize().0,
            projects: self.projects.finalize().0,
            specifier_cache: self.specifier_cache.build(),
            unique_package_count: self.unique_package_count,
            entrypoints: self.entrypoints.build(),
        })
    }

    // Go: ls/autoimport/registry.go:553 updateBucketAndDirectoryExistence
    pub fn update_bucket_and_directory_existence(
        &self,
        change: &RegistryChange,
        logger: &Option<Rc<logging::LogTree>>,
    ) {
        let start = Instant::now();
        let mut needed_projects: FxHashMap<ProjectID, ()> = FxHashMap::default();
        let mut needed_directories: FxHashMap<tspath::Path, String> = FxHashMap::default();
        // Go: ls/autoimport/registry.go:558 addNeededDirectories (ts#64554)
        let add_needed_directories = |needed_directories: &mut FxHashMap<tspath::Path, String>,
                                      path: &tspath::Path,
                                      file_name: &str| {
            if tspath::is_dynamic_file_name(file_name) {
                return;
            }
            let mut dir = file_name.to_string();
            let mut dir_path = path.clone();
            loop {
                dir = tspath::get_directory_path(&dir);
                let last_dir_path = dir_path.clone();
                dir_path = dir_path.get_directory_path();
                if dir_path == last_dir_path {
                    break;
                }
                if needed_directories.contains_key(&dir_path) {
                    break;
                }
                needed_directories.insert(dir_path.clone(), dir.clone());
            }
        };
        for (path, file_name) in &change.open_files {
            if let (Some(project_id), _) = self.host.get_default_project(path) {
                needed_projects.insert(project_id, ());
            }
            if tspath::is_dynamic_file_name(file_name) {
                continue;
            }
            add_needed_directories(&mut needed_directories, path, file_name);

            if !self.specifier_cache.has(path) {
                self.specifier_cache
                    .set(path.clone(), Rc::new(RefCell::new(FxHashMap::default())));
            }
        }

        if !change.requested_file.is_empty() {
            // ts#64554: registry.go:590
            add_needed_directories(
                &mut needed_directories,
                &change.requested_file,
                &change.requested_file_name,
            );
            if let (Some(project_id), _) = self.host.get_default_project(&change.requested_file) {
                needed_projects.insert(project_id, ());
            }
            if !self.specifier_cache.has(&change.requested_file) {
                self.specifier_cache.set(
                    change.requested_file.clone(),
                    Rc::new(RefCell::new(FxHashMap::default())),
                );
            }
        }

        for path in self.base.specifier_cache.keys() {
            if !change.open_files.contains_key(path) && *path != change.requested_file {
                self.specifier_cache.delete(path);
            }
        }

        let mut added_projects: Vec<ProjectID> = Vec::new();
        let mut removed_projects: Vec<ProjectID> = Vec::new();
        {
            let on_added: &mut dyn FnMut(&ProjectID, &()) =
                &mut |project_id: &ProjectID, _: &()| {
                    // Need and don't have
                    self.projects.add(project_id.clone(), new_registry_bucket());
                    added_projects.push(project_id.clone());
                };
            let on_removed: &mut dyn FnMut(&ProjectID, &Rc<RegistryBucket>) =
                &mut |project_id: &ProjectID, _: &Rc<RegistryBucket>| {
                    // Have and don't need
                    self.projects.delete(project_id);
                    removed_projects.push(project_id.clone());
                };
            diff_maps_func(
                &self.base.projects,
                &needed_projects,
                |_: &Rc<RegistryBucket>, _: &()| -> bool {
                    crate::core::go_panic("never called because onChanged is nil".to_string())
                },
                Some(on_added),
                Some(on_removed),
                None,
            );
        }
        if logger.is_some() {
            for project_id in &added_projects {
                logger.logf(&format!("Added project: {project_id}"));
            }
            for project_id in &removed_projects {
                logger.logf(&format!("Removed project: {project_id}"));
            }
        }

        let update_directory =
            |dir_path: &tspath::Path, dir_name: &str, package_json_changed: bool| {
                let package_json_file_name = tspath::combine_paths(dir_name, &["package.json"]);
                let has_node_modules = self
                    .host
                    .fs()
                    .directory_exists(&tspath::combine_paths(dir_name, &["node_modules"]));
                if let (Some(entry), true) = self.directories.get(dir_path) {
                    entry.change_if(
                        &mut |dir: Option<&Rc<RefCell<Directory>>>| {
                            package_json_changed
                                || dir
                                    .unwrap_or_else(|| crate::core::go_nil_dereference())
                                    .borrow()
                                    .has_node_modules
                                    != has_node_modules
                        },
                        &mut |dir: &Rc<RefCell<Directory>>| {
                            let package_json = self.host.get_package_json(&package_json_file_name);
                            let mut dir = dir.borrow_mut();
                            dir.package_json = package_json;
                            dir.has_node_modules = has_node_modules;
                        },
                    );
                } else {
                    self.directories.add(
                        dir_path.clone(),
                        Rc::new(RefCell::new(Directory {
                            name: dir_name.to_string(),
                            package_json: self.host.get_package_json(&package_json_file_name),
                            has_node_modules,
                        })),
                    );
                }

                if has_node_modules {
                    if !self.node_modules.get(dir_path).1 {
                        self.node_modules
                            .add(dir_path.clone(), new_registry_bucket());
                    }
                } else {
                    self.node_modules.try_delete(dir_path);
                }
            };

        let mut added_node_modules_dirs: Vec<tspath::Path> = Vec::new();
        let mut removed_node_modules_dirs: Vec<tspath::Path> = Vec::new();
        let package_json_changed = |dir_name: &str| -> bool {
            let uri = lsconv::file_name_to_document_uri(&tspath::combine_paths(
                dir_name,
                &["package.json"],
            ));
            change.changed.contains(&uri)
                || change.deleted.contains(&uri)
                || change.created.contains(&uri)
        };
        {
            let on_added: &mut dyn FnMut(&tspath::Path, &String) =
                &mut |dir_path: &tspath::Path, dir_name: &String| {
                    // Need and don't have
                    let had_node_modules = self.base.node_modules.get(dir_path).is_some();
                    update_directory(dir_path, dir_name, false);
                    if logger.is_some() {
                        logger.logf(&format!("Added directory: {dir_path}"));
                    }
                    let has_now = self.node_modules.get(dir_path).1;
                    if has_now && !had_node_modules {
                        added_node_modules_dirs.push(dir_path.clone());
                    }
                };
            let on_removed: &mut dyn FnMut(&tspath::Path, &Rc<RefCell<Directory>>) =
                &mut |dir_path: &tspath::Path, _dir: &Rc<RefCell<Directory>>| {
                    // Have and don't need
                    let had_node_modules = self.base.node_modules.get(dir_path).is_some();
                    self.directories.delete(dir_path);
                    self.node_modules.try_delete(dir_path);
                    if logger.is_some() {
                        logger.logf(&format!("Removed directory: {dir_path}"));
                    }
                    if had_node_modules {
                        removed_node_modules_dirs.push(dir_path.clone());
                    }
                };
            let on_changed: &mut dyn FnMut(&tspath::Path, &Rc<RefCell<Directory>>, &String) =
                &mut |dir_path: &tspath::Path, _dir: &Rc<RefCell<Directory>>, dir_name: &String| {
                    update_directory(dir_path, dir_name, package_json_changed(dir_name));
                    if logger.is_some() {
                        logger.logf(&format!("Changed directory: {dir_path}"));
                    }
                };
            diff_maps_func(
                &self.base.directories,
                &needed_directories,
                |dir: &Rc<RefCell<Directory>>, dir_name: &String| -> bool {
                    !package_json_changed(dir_name)
                        && dir.borrow().has_node_modules
                            == self.host.fs().directory_exists(&tspath::combine_paths(
                                dir_name,
                                &["node_modules"],
                            ))
                },
                Some(on_added),
                Some(on_removed),
                Some(on_changed),
            );
        }

        if logger.is_some() {
            for dir_path in &added_node_modules_dirs {
                logger.logf(&format!("Added node_modules bucket: {dir_path}"));
            }
            for dir_path in &removed_node_modules_dirs {
                logger.logf(&format!("Removed node_modules bucket: {dir_path}"));
            }
            logger.logf(&format!(
                "Updated buckets and directories in {:?}",
                start.elapsed()
            ));
        }
    }

    // Go: ls/autoimport/registry.go:707 markBucketsDirty
    // PORT: Go ranges over the clean-bucket maps while deleting the current
    // key; the port ranges over a copy of the keys, which visits the same keys.
    pub fn mark_buckets_dirty(
        &self,
        change: &RegistryChange,
        logger: &Option<Rc<logging::LogTree>>,
    ) {
        // Mark new program structures
        for (project_id, new_file_names) in &change.rebuilt_programs {
            if let (Some(bucket), true) = self.projects.get(project_id) {
                let new_file_names = *new_file_names;
                bucket.change(&mut |bucket: &Rc<RegistryBucket>| {
                    bucket.state.borrow_mut().new_program_structure = if new_file_names {
                        NewProgramStructure::DIFFERENT_FILE_NAMES
                    } else {
                        NewProgramStructure::SAME_FILE_NAMES
                    };
                });
            }
        }

        // Mark files dirty, bailing out if all buckets already have multiple files dirty
        let mut clean_node_modules_buckets: FxHashSet<tspath::Path> = FxHashSet::default();
        let mut clean_project_buckets: FxHashSet<ProjectID> = FxHashSet::default();
        self.node_modules.range(&mut |entry: &Rc<
            dirty::MapEntry<tspath::Path, Rc<RegistryBucket>>,
        >| {
            if !entry
                .value()
                .unwrap_or_else(|| crate::core::go_nil_dereference())
                .state
                .borrow()
                .multiple_files_dirty
            {
                clean_node_modules_buckets.insert(entry.key());
            }
            true
        });
        self.projects.range(
            &mut |entry: &Rc<dirty::MapEntry<ProjectID, Rc<RegistryBucket>>>| {
                if !entry
                    .value()
                    .unwrap_or_else(|| crate::core::go_nil_dereference())
                    .state
                    .borrow()
                    .multiple_files_dirty
                {
                    clean_project_buckets.insert(entry.key());
                }
                true
            },
        );

        let mut mark_files_dirty = |uris: &FxHashSet<lsproto::DocumentUri>| {
            if clean_node_modules_buckets.is_empty() && clean_project_buckets.is_empty() {
                return;
            }
            for uri in uris {
                let path = (self.base.to_path)(&uri.file_name());
                if !clean_node_modules_buckets.is_empty() {
                    // For node_modules, mark the bucket dirty if anything changes in the directory.
                    // The path could be either a symlink path (containing /node_modules/) or a realpath
                    // (for symlinked project references). Both are recorded in Paths for granular updates.
                    if let Some(node_modules_index) = path.find("/node_modules/") {
                        let dir_path =
                            tspath::Path(path.as_str()[..node_modules_index].to_string());
                        if clean_node_modules_buckets.contains(&dir_path) {
                            let entry = self
                                .node_modules
                                .get(&dir_path)
                                .0
                                .unwrap_or_else(|| crate::core::go_nil_dereference());
                            // Look up the package name for granular updates
                            let package_name = entry
                                .value()
                                .unwrap_or_else(|| crate::core::go_nil_dereference())
                                .paths
                                .get(&path)
                                .cloned()
                                .unwrap_or_default();
                            entry.change(&mut |bucket: &Rc<RegistryBucket>| {
                                bucket.mark_node_modules_dirty(&package_name)
                            });
                            if !entry
                                .value()
                                .unwrap_or_else(|| crate::core::go_nil_dereference())
                                .state
                                .borrow()
                                .multiple_files_dirty
                            {
                                clean_node_modules_buckets.remove(&dir_path);
                            }
                        }
                    } else {
                        // Check if this path (possibly a realpath of a workspace package) is in any bucket's Paths.
                        // This handles local workspace packages where the realpath doesn't contain /node_modules/.
                        let bucket_dir_paths: Vec<tspath::Path> =
                            clean_node_modules_buckets.iter().cloned().collect();
                        for bucket_dir_path in bucket_dir_paths {
                            let entry = self
                                .node_modules
                                .get(&bucket_dir_path)
                                .0
                                .unwrap_or_else(|| crate::core::go_nil_dereference());
                            let package_name = entry
                                .value()
                                .unwrap_or_else(|| crate::core::go_nil_dereference())
                                .paths
                                .get(&path)
                                .cloned();
                            if let Some(package_name) = package_name {
                                // Use the package name for granular updates
                                entry.change(&mut |bucket: &Rc<RegistryBucket>| {
                                    bucket.mark_node_modules_dirty(&package_name)
                                });
                                if !entry
                                    .value()
                                    .unwrap_or_else(|| crate::core::go_nil_dereference())
                                    .state
                                    .borrow()
                                    .multiple_files_dirty
                                {
                                    clean_node_modules_buckets.remove(&bucket_dir_path);
                                }
                            }
                        }
                    }
                }

                // For projects, mark the bucket dirty if the bucket contains the file directly.
                // Any other significant change, like a created failed lookup location, is
                // handled by newProgramStructure.
                let project_dir_paths: Vec<ProjectID> =
                    clean_project_buckets.iter().cloned().collect();
                for project_dir_path in project_dir_paths {
                    let (entry, _) = self.projects.get(&project_dir_path);
                    let entry = entry.unwrap_or_else(|| crate::core::go_nil_dereference());
                    if entry
                        .value()
                        .unwrap_or_else(|| crate::core::go_nil_dereference())
                        .paths
                        .contains_key(&path)
                    {
                        // Project buckets don't use package-based granular updates
                        entry.change(&mut |bucket: &Rc<RegistryBucket>| {
                            bucket.mark_project_file_dirty(&path)
                        });
                        if !entry
                            .value()
                            .unwrap_or_else(|| crate::core::go_nil_dereference())
                            .state
                            .borrow()
                            .multiple_files_dirty
                        {
                            clean_project_buckets.remove(&project_dir_path);
                        }
                    }
                }
            }
        };

        mark_files_dirty(&change.created);
        mark_files_dirty(&change.deleted);
        mark_files_dirty(&change.changed);
    }

    // Go: ls/autoimport/registry.go:791 updateIndexes
    pub fn update_indexes(
        &mut self,
        ctx: &Context,
        change: &RegistryChange,
        logger: &Option<Rc<logging::LogTree>>,
    ) {
        // Go: ls/autoimport/registry.go:792 nodeModulesBucketTask
        struct NodeModulesBucketTask {
            entry: Rc<dirty::MapEntry<tspath::Path, Rc<RegistryBucket>>>,
            dependency_names: Option<FxHashSet<String>>,
            dir_name: String,
            dir_path: tspath::Path,

            // For granular updates.
            is_update: bool,
            existing_bucket: Option<Rc<RegistryBucket>>,
            dirty_packages: Option<FxHashSet<String>>,

            // Filled by discovery.
            package_names: Option<FxHashSet<String>>,
            directory_package_names: Option<FxHashSet<String>>,
            discovered: Vec<Rc<DiscoveredPackage>>,
        }

        let (project_id, _) = self.host.get_default_project(&change.requested_file);
        let Some(project_id) = project_id else {
            return;
        };

        // Go: var wg sync.WaitGroup (PORT: serial, see the file header)

        // Compute resolved package names and project reference output mappings for all projects upfront.
        // Resolved package names are needed to compute node_modules dependencies so packages that are
        // directly imported by programs are included even if not listed in package.json.
        // Project reference output mappings are needed to redirect extraction from output .d.ts files
        // to source files for packages that are project references.
        // We need all projects because a node_modules directory can be used by multiple projects.
        let mut all_resolved_package_names: FxHashMap<ProjectID, Rc<FxHashSet<String>>> =
            FxHashMap::default();
        let mut project_reference_outputs: FxHashMap<tspath::Path, String> = FxHashMap::default();
        // Compute which packages have implicit deep imports (subpath imports in packages
        // without exports). These packages need recursive directory search to discover
        // all auto-importable files, even when the preference is disabled.
        let mut all_deep_import_packages: FxHashSet<String> = FxHashSet::default();
        self.projects.range(
            &mut |entry: &Rc<dirty::MapEntry<ProjectID, Rc<RegistryBucket>>>| {
                let program = self.host.get_program_for_project(&entry.key());
                if let Some(program) = program.as_deref() {
                    all_resolved_package_names.insert(
                        entry.key(),
                        Rc::new(get_resolved_package_names(ctx, program)),
                    );
                    add_project_reference_output_mappings(program, &mut project_reference_outputs);
                    for name in program.deep_import_package_names() {
                        all_deep_import_packages.insert(name.clone());
                    }
                }
                true
            },
        );

        let file_exclude_patterns = self
            .user_preferences
            .parsed_auto_import_file_exclude_patterns(
                self.host.fs().use_case_sensitive_file_names(),
            );

        // Determine which packages need recursive directory search for this build.
        // nil means all packages (preference is enabled for all).
        let mut target_recursive_packages: Option<FxHashSet<String>> = None;
        if !self
            .user_preferences
            .auto_import_entrypoint_directory_search
            .is_true()
        {
            target_recursive_packages = Some(all_deep_import_packages);
        }

        // --- Collect node_modules tasks ---
        let mut node_modules_tasks: Vec<NodeModulesBucketTask> = Vec::new();
        change
            .requested_file
            .for_each_ancestor_directory(|dir_path: tspath::Path| -> ((), bool) {
                if let (Some(node_modules_bucket), true) = self.node_modules.get(&dir_path) {
                    let dir_name = self
                        .directories
                        .get(&dir_path)
                        .0
                        .unwrap_or_else(|| crate::core::go_nil_dereference())
                        .value()
                        .unwrap_or_else(|| crate::core::go_nil_dereference())
                        .borrow()
                        .name
                        .clone();
                    let dependencies = self.compute_dependencies_for_node_modules_directory(
                        change,
                        &all_resolved_package_names,
                        &dir_name,
                        &dir_path,
                    );
                    let bucket_value = node_modules_bucket
                        .value()
                        .unwrap_or_else(|| crate::core::go_nil_dereference());
                    let bucket_state = bucket_value.state.borrow().clone();
                    // !!! Optimization: handle different dependency set via granular updates
                    let needs_full_rebuild = bucket_state.multiple_files_dirty
                        || !set_equals(
                            bucket_value.dependency_names.as_deref(),
                            dependencies.as_ref(),
                        )
                        || !bucket_state.build_preferences.equal(
                            &bucket_build_preferences_from_user_preferences(&self.user_preferences),
                        )
                        || !recursive_search_subset(
                            target_recursive_packages.as_ref(),
                            bucket_state.recursive_search_packages.as_ref(),
                        );
                    let dirty_packages = bucket_state.dirty_packages_exported().cloned();
                    let can_do_granular_update =
                        !needs_full_rebuild && set_len(dirty_packages.as_ref()) > 0;

                    if needs_full_rebuild {
                        node_modules_tasks.push(NodeModulesBucketTask {
                            entry: node_modules_bucket.clone(),
                            dependency_names: dependencies,
                            dir_name,
                            dir_path,
                            is_update: false,
                            existing_bucket: None,
                            dirty_packages: None,
                            package_names: None,
                            directory_package_names: None,
                            discovered: Vec::new(),
                        });
                    } else if can_do_granular_update {
                        node_modules_tasks.push(NodeModulesBucketTask {
                            entry: node_modules_bucket.clone(),
                            dependency_names: dependencies,
                            dir_name,
                            dir_path,
                            is_update: true,
                            existing_bucket: Some(bucket_value.clone()),
                            dirty_packages,
                            package_names: None,
                            directory_package_names: None,
                            discovered: Vec::new(),
                        });
                    }
                }
                ((), false)
            });

        let mut node_modules_logger: Option<Rc<logging::LogTree>> = None;
        if logger.is_some() && !node_modules_tasks.is_empty() {
            node_modules_logger = logger.fork("Building node_modules indexes");
        }

        // --- Phase 1: Discovery (parallel per bucket) ---
        // Resolve package.json and realpath for each package in each bucket.
        let discovery_start = Instant::now();
        for task in node_modules_tasks.iter_mut() {
            // Go: wg.Go(func() {...})
            go_wait_group_goroutine(|| {
                if task.is_update {
                    task.package_names = task.dirty_packages.clone();
                } else {
                    task.directory_package_names = Some(get_package_names_in_node_modules(
                        &tspath::combine_paths(&task.dir_name, &["node_modules"]),
                        self.host.fs(),
                    ));
                    // Go: core.Coalesce(task.dependencyNames, task.directoryPackageNames)
                    task.package_names = task
                        .dependency_names
                        .clone()
                        .or_else(|| task.directory_package_names.clone());
                }
                task.discovered = self.discover_bucket_packages(
                    ctx,
                    task.package_names.as_ref(),
                    &task.dir_name,
                    &task.dir_path,
                );
            });
        }
        // Go: wg.Wait()
        // PORT: the stop points marked `should_stop_build` are the port's
        // (see `DISCARD_ON_CANCEL_KEY`). A stopped build returns at once and
        // skips Go's later steps. They change only this builder, which the
        // caller drops.
        if should_stop_build(ctx) {
            return;
        }
        if node_modules_logger.is_some() {
            node_modules_logger.logf(&format!(
                "Discovered packages: {:?}",
                discovery_start.elapsed()
            ));
        }

        // --- Phase 2: Extraction (parallel per unique realpath) ---
        // Extract from main packages first. If a main package has no TypeScript entrypoints,
        // we fall back to extracting from @types in a second pass. Packages with no main
        // package extract directly from @types in the primary pass.
        let extraction_start = Instant::now();
        let mut seen: FxHashMap<String, bool> = FxHashMap::default();
        let mut extraction_cache: FxHashMap<String, Rc<PerPackageExtractionResult>> =
            FxHashMap::default();
        // Go: var extractionMu sync.Mutex (PORT: one thread)
        // Collect all packages that have an @types fallback. After the primary pass, we
        // filter to only those whose main extraction failed, then deduplicate by typesRealpath.
        let mut types_fallback_candidates: Vec<Rc<DiscoveredPackage>> = Vec::new();
        for task in &node_modules_tasks {
            for pkg in &task.discovered {
                if !pkg.realpath.is_empty() {
                    if !seen.get(&pkg.realpath).copied().unwrap_or(false) {
                        seen.insert(pkg.realpath.clone(), true);
                        let enable_dir_search = target_recursive_packages.is_none()
                            || set_has(target_recursive_packages.as_ref(), &pkg.package_name)
                            || KNOWN_RECURSIVE_SEARCH_PACKAGES.contains(pkg.package_name.as_str());
                        // Record actual directory-searched packages so the stored set
                        // reflects reality for rebuild detection and stats.
                        if enable_dir_search && target_recursive_packages.is_some() {
                            target_recursive_packages
                                .as_mut()
                                .unwrap()
                                .insert(pkg.package_name.clone());
                        }
                        // Go: wg.Go(func() {...})
                        go_wait_group_goroutine(|| {
                            if ctx.err().is_none() {
                                let result = self.extract_package(
                                    ctx,
                                    &pkg.package_json,
                                    &pkg.package_name,
                                    &project_reference_outputs,
                                    file_exclude_patterns.as_ref(),
                                    enable_dir_search,
                                );
                                if let Some(result) = result {
                                    extraction_cache.insert(pkg.realpath.clone(), result);
                                }
                            }
                        });
                    }
                    if !pkg.types_realpath.is_empty() {
                        types_fallback_candidates.push(pkg.clone());
                    }
                } else if !pkg.types_realpath.is_empty() {
                    if !seen.get(&pkg.types_realpath).copied().unwrap_or(false) {
                        seen.insert(pkg.types_realpath.clone(), true);
                        // @types packages always get directory search
                        if let Some(target_recursive_packages) = target_recursive_packages.as_mut()
                        {
                            target_recursive_packages.insert(pkg.package_name.clone());
                        }
                        // Go: wg.Go(func() {...})
                        go_wait_group_goroutine(|| {
                            if ctx.err().is_none() {
                                let result = self.extract_package(
                                    ctx,
                                    &pkg.types_package_json,
                                    &pkg.package_name,
                                    &project_reference_outputs,
                                    file_exclude_patterns.as_ref(),
                                    true, /*enableDirectorySearch*/
                                );
                                if let Some(result) = result {
                                    extraction_cache.insert(pkg.types_realpath.clone(), result);
                                }
                            }
                        });
                    }
                }
            }
        }
        // Go: wg.Wait()

        // For packages whose main extraction yielded nothing, fall back to @types.
        for pkg in &types_fallback_candidates {
            // Go: extractionMu.Lock() (PORT: one thread, no lock)
            let main_extracted = extraction_cache.get(&pkg.realpath).is_some();
            // Go: extractionMu.Unlock()
            if main_extracted || seen.get(&pkg.types_realpath).copied().unwrap_or(false) {
                continue;
            }
            seen.insert(pkg.types_realpath.clone(), true);
            // @types fallback packages always get directory search
            if let Some(target_recursive_packages) = target_recursive_packages.as_mut() {
                target_recursive_packages.insert(pkg.package_name.clone());
            }
            // Go: wg.Go(func() {...})
            go_wait_group_goroutine(|| {
                if ctx.err().is_none() {
                    let result = self.extract_package(
                        ctx,
                        &pkg.types_package_json,
                        &pkg.package_name,
                        &project_reference_outputs,
                        file_exclude_patterns.as_ref(),
                        true, /*enableDirectorySearch*/
                    );
                    if let Some(result) = result {
                        extraction_cache.insert(pkg.types_realpath.clone(), result);
                    }
                }
            });
        }
        // Go: wg.Wait()
        if node_modules_logger.is_some() {
            node_modules_logger.logf(&format!(
                "Extracted exports: {:?} ({} packages)",
                extraction_start.elapsed(),
                seen.len()
            ));
        }
        if should_stop_build(ctx) {
            return;
        }
        self.unique_package_count = seen.len() as i32;

        // --- Phase 3: Bucket building (parallel per bucket) ---
        // Each bucket installs the shared extraction results and builds its index.
        // PORT: Go appends `br` before its goroutine fills it; the serial port
        // fills it, then appends it (nothing reads the list in between).
        let mut all_results: Vec<BucketBuildResult> = Vec::new();

        for task in &node_modules_tasks {
            let entry = task.entry.clone();
            let mut br = new_bucket_build_result(
                Box::new(move |bucket: Rc<RegistryBucket>| entry.replace(bucket)),
                task.entry.key(),
            );
            // Go: wg.Go(func() {...})
            go_wait_group_goroutine(|| {
                if task.is_update {
                    self.update_node_modules_bucket(
                        ctx,
                        &mut br,
                        task.existing_bucket
                            .as_ref()
                            .unwrap_or_else(|| crate::core::go_nil_dereference()),
                        task.dirty_packages.as_ref(),
                        &task.discovered,
                        &extraction_cache,
                        target_recursive_packages.as_ref(),
                        node_modules_logger.fork(&task.dir_name),
                    );
                } else {
                    self.build_node_modules_bucket(
                        ctx,
                        &mut br,
                        task.dependency_names.clone(),
                        &task.dir_path,
                        &task.discovered,
                        task.directory_package_names.as_ref(),
                        &extraction_cache,
                        target_recursive_packages.as_ref(),
                        node_modules_logger.fork(&task.dir_name),
                    );
                }
            });
            all_results.push(br);
        }

        // Project bucket (not part of the three-phase pipeline — no cross-bucket dedup needed).
        if let (Some(project), true) = self.projects.get(&project_id) {
            let program = self.host.get_program_for_project(&project_id);
            let resolved_package_names = all_resolved_package_names.get(&project_id).cloned();
            let project_value = project
                .value()
                .unwrap_or_else(|| crate::core::go_nil_dereference());
            let mut should_rebuild = project_value
                .state
                .borrow()
                .has_dirty_file_besides(&change.requested_file)
                || !project_value.state.borrow().build_preferences.equal(
                    &bucket_build_preferences_from_user_preferences(&self.user_preferences),
                );
            if !should_rebuild && project_value.state.borrow().new_program_structure.0 > 0 {
                if !set_equals(
                    project_value.resolved_package_names.as_deref(),
                    resolved_package_names.as_deref(),
                ) || has_new_non_node_modules_files(program.as_deref(), &project_value)
                {
                    should_rebuild = true;
                } else {
                    project.change(&mut |b: &Rc<RegistryBucket>| {
                        b.state.borrow_mut().new_program_structure = NewProgramStructure::FALSE;
                    });
                }
            }
            if should_rebuild {
                let entry = project.clone();
                let mut br = new_bucket_build_result(
                    Box::new(move |bucket: Rc<RegistryBucket>| entry.replace(bucket)),
                    (self.base.to_path)(
                        &program
                            .as_deref()
                            .unwrap_or_else(|| crate::core::go_nil_dereference())
                            .get_current_directory(),
                    ),
                );
                // Go: wg.Go(func() {...})
                go_wait_group_goroutine(|| {
                    self.build_project_bucket(
                        ctx,
                        &mut br,
                        &project_id,
                        resolved_package_names,
                        logger.fork(&format!("Building project bucket {}", project_id.string())),
                    );
                });
                all_results.push(br);
            }
        }

        // Go: wg.Wait()
        if should_stop_build(ctx) {
            return;
        }

        for br in &all_results {
            if br.err.is_some() {
                continue;
            }
            for path in &br.removed_entrypoint_paths {
                self.entrypoints.delete(path);
            }
            for (path, entries) in &br.entrypoints {
                self.entrypoints.set(path.clone(), entries.clone());
            }
            (br.replace_bucket)(
                br.bucket
                    .clone()
                    .unwrap_or_else(|| crate::core::go_nil_dereference()),
            );
        }

        // If we failed to resolve any alias exports by ending up at a non-relative module specifier
        // that didn't resolve to another package, it's probably an ambient module declared in another package.
        // We recorded these failures, along with the name of every ambient module declared elsewhere, so we
        // can do a second pass on the failed files, this time including the ambient modules declarations that
        // were missing the first time. Example: node_modules/fs-extra/index.d.ts is simply `export * from "fs"`,
        // but when trying to resolve the `export *`, we don't know where "fs" is declared. The aliasResolver
        // tries to find packages named "fs" on the file system, but after failing, records "fs" as a failure
        // for fs-extra/index.d.ts. Meanwhile, if we also processed node_modules/@types/node/fs.d.ts, we
        // recorded that file as declaring the ambient module "fs". In the second pass, we combine those two
        // files and reprocess fs-extra/index.d.ts, this time finding "fs" declared in @types/node.
        let second_pass_start = Instant::now();
        let mut second_pass_file_count = 0;
        for br in &all_results {
            // PORT: Go has no check in the second pass. A stop here leaves
            // an installed bucket with a partial second pass, so only a
            // build that the caller drops can stop here.
            if should_stop_build(ctx) {
                return;
            }
            if br.err.is_some() {
                continue;
            }
            let Some(targets) = &br.possible_failed_ambient_module_lookup_targets else {
                continue;
            };
            // PORT: Go `map[string]*ast.SourceFile`, then `maps.Values` (random
            // order); insertion order here.
            let mut root_files: IndexMap<String, Node> = IndexMap::new();
            for target in targets {
                for file_name in self.resolve_ambient_module_name(target, &br.resolution_path) {
                    if should_stop_build(ctx) {
                        return;
                    }
                    if root_files.contains_key(&file_name) {
                        continue;
                    }
                    let file = self
                        .host
                        .get_source_file(&file_name, &(self.base.to_path)(&file_name));
                    root_files.insert(file_name, file);
                    second_pass_file_count += 1;
                }
            }
            if !root_files.is_empty() {
                let mut resolver_options = copy_resolver_options(&self.resolver_options);
                resolver_options.host = Some(self.host.clone());
                resolver_options.compiler_options = Some(Rc::new(CompilerOptions::default()));
                let module_resolver = Rc::new(module::new_resolver(resolver_options));
                let alias_resolver = new_alias_resolver(
                    root_files.values().copied().collect(),
                    FxHashMap::default(),
                    self.host.clone(),
                    module_resolver.clone(),
                    self.base.to_path.clone(),
                    Rc::new(|_: &dyn HasFileName, _: &str| {
                        // no-op
                    }),
                );
                let sources = br
                    .possible_failed_ambient_module_lookup_sources
                    .as_ref()
                    .unwrap_or_else(|| crate::core::go_nil_dereference());
                // PORT: Go reads each source file after NewChecker. Here they
                // are read first, so the checker's arena holds them
                // (`AliasResolver::new_checker`).
                let source_files: Vec<Node> = sources
                    .values()
                    .map(|source| alias_resolver.get_source_file(&source.borrow().file_name))
                    .collect();
                let Some((ch, _alias_program)) = alias_resolver.new_checker(ctx, &source_files)
                else {
                    return;
                };
                let mut ch_ref = ch.borrow_mut();
                for (source, &source_file) in sources.values().zip(&source_files) {
                    if should_stop_build(ctx) {
                        return;
                    }
                    let source = source.borrow();
                    let host = self.host.clone();
                    let realpath: Rc<dyn Fn(&str) -> String> =
                        Rc::new(move |f: &str| host.fs().realpath(f));
                    let mut extractor = self.new_export_extractor(
                        &source.package_name,
                        &mut ch_ref,
                        module_resolver.clone(),
                        Some(realpath),
                    );
                    let file_exports = extractor.extract_from_file(source_file);
                    let index = br
                        .bucket
                        .as_ref()
                        .unwrap_or_else(|| crate::core::go_nil_dereference())
                        .index
                        .clone()
                        .unwrap_or_else(|| crate::core::go_nil_dereference());
                    for exp in file_exports {
                        index.borrow_mut().insert_as_words(exp);
                    }
                }
            }
        }

        if node_modules_logger.is_some() {
            if second_pass_file_count > 0 {
                node_modules_logger.logf(&format!(
                    "{} files required second pass, took {:?}",
                    second_pass_file_count,
                    second_pass_start.elapsed()
                ));
            }
            node_modules_logger.logf(&format!("Total: {:?}", discovery_start.elapsed()));
        }
    }
}

// Go: ls/autoimport/registry.go:1137 hasNewNonNodeModulesFiles
// PORT: Go `program` can be nil; it is read only for a
// `newProgramStructureDifferentFileNames` bucket, where nil panics as in Go.
pub fn has_new_non_node_modules_files(
    program: Option<&compiler::NewProgram>,
    bucket: &RegistryBucket,
) -> bool {
    if bucket.state.borrow().new_program_structure != NewProgramStructure::DIFFERENT_FILE_NAMES {
        return false;
    }
    let program = program.unwrap_or_else(|| crate::core::go_nil_dereference());
    for file in program.get_source_files() {
        if file.is_content_mapper_supplemental()
            || file.file_name().contains("/node_modules/")
            || is_ignored_file(program, file)
        {
            continue;
        }
        if !bucket.paths.contains_key(file.path()) {
            return true;
        }
    }
    false
}

// Go: ls/autoimport/registry.go:1152 isIgnoredFile
// PORT: only `FileName()` and `Path()` are read, so the program's
// `ParsedSourceFile` is passed (see the file header).
pub fn is_ignored_file(program: &compiler::NewProgram, file: &ParsedSourceFile) -> bool {
    program.is_source_file_default_library(file.path())
        || ls_program::is_global_typings_file(program, file.file_name())
}

// Go: ls/autoimport/registry.go:1159 hasSymlinkToNodeModules
// hasSymlinkToNodeModules checks if a file's realpath has a symlink that points
// to a node_modules directory. This is used to skip files in the project bucket
// that would be duplicated by the node_modules bucket via their symlink.
// PORT: Go `FilesByRealpath()` and `DirectoriesByRealpath()` can be nil; the
// port's maps always exist, so those nil checks always pass.
pub fn has_symlink_to_node_modules(
    file_path: &tspath::Path,
    project_root_path: &tspath::Path,
    symlink_cache: Option<&KnownSymlinks>,
) -> bool {
    let Some(symlink_cache) = symlink_cache else {
        return false;
    };
    // Keep files inside this project indexed in project buckets even if they are
    // reachable through a node_modules symlink from elsewhere.
    if project_root_path.contains_path(file_path) {
        return false;
    }

    // First check if the file itself has a symlink to node_modules
    let files_by_realpath = symlink_cache.files_by_realpath();
    if let Some(symlink_paths) = files_by_realpath.get(file_path) {
        let mut found = false;
        for symlink_path in symlink_paths {
            if symlink_path.contains("/node_modules/") {
                found = true;
                break; // stop ranging
            }
        }
        if found {
            return true;
        }
    }

    // Fall back to checking ancestor directories
    let directories_by_realpath = symlink_cache.directories_by_realpath();
    let mut found = false;
    file_path.for_each_ancestor_directory(|dir_path: tspath::Path| -> ((), bool) {
        let Some(symlink_paths) =
            directories_by_realpath.get(&dir_path.ensure_trailing_directory_separator())
        else {
            return ((), false);
        };
        // Check if any of the symlinks point to a node_modules directory
        for symlink_path in symlink_paths {
            if symlink_path.contains("/node_modules/") {
                found = true;
                break; // stop ranging
            }
        }
        ((), found) // stop if we found a match
    });
    found
}

// Go: ls/autoimport/registry.go:1210 failedAmbientModuleLookupSource
// PORT: Go `mu sync.Mutex` is dropped (one thread).
#[derive(Clone, Debug, Default)]
pub struct FailedAmbientModuleLookupSource {
    pub file_name: String,
    pub package_name: String,
}

// Go: ls/autoimport/registry.go:1216 bucketBuildResult
// PORT: Go `error` is `Option<GoError>`; Go nil sync maps and sets are
// `None`.
pub struct BucketBuildResult {
    /// Go `replaceBucket func(*RegistryBucket)`.
    pub replace_bucket: Box<dyn Fn(Rc<RegistryBucket>)>,
    pub resolution_path: tspath::Path,
    pub err: Option<GoError>,

    pub bucket: Option<Rc<RegistryBucket>>,
    // entrypoints are the resolved entrypoints from this bucket's packages,
    // to be merged into the registry-level entrypoints map.
    pub entrypoints: FxHashMap<tspath::Path, Vec<Rc<module::ResolvedEntrypoint>>>,
    // removedEntrypointPaths lists paths whose entrypoints should be removed from
    // the registry-level map before merging new entrypoints. Used for granular updates.
    pub removed_entrypoint_paths: Vec<tspath::Path>,
    // File path to filename and package name
    pub possible_failed_ambient_module_lookup_sources:
        Option<IndexMap<tspath::Path, Rc<RefCell<FailedAmbientModuleLookupSource>>>>,
    // Likely ambient module name
    pub possible_failed_ambient_module_lookup_targets: Option<IndexSet<String>>,
}

/// Go `&bucketBuildResult{replaceBucket: .., resolutionPath: ..}`.
fn new_bucket_build_result(
    replace_bucket: Box<dyn Fn(Rc<RegistryBucket>)>,
    resolution_path: tspath::Path,
) -> BucketBuildResult {
    BucketBuildResult {
        replace_bucket,
        resolution_path,
        err: None,
        bucket: None,
        entrypoints: FxHashMap::default(),
        removed_entrypoint_paths: Vec::new(),
        possible_failed_ambient_module_lookup_sources: None,
        possible_failed_ambient_module_lookup_targets: None,
    }
}

impl RegistryBuilder {
    // Go: ls/autoimport/registry.go:1234 buildProjectBucket
    // PORT: Go `result.bucket = &RegistryBucket{}` comes first and its fields
    // are set at the end; the port makes the bucket at the end (nothing reads
    // it in between). `getChecker` is `create_checker_pool` (serial).
    pub fn build_project_bucket(
        &self,
        ctx: &Context,
        result: &mut BucketBuildResult,
        project_id: &ProjectID,
        resolved_package_names: Option<Rc<FxHashSet<String>>>,
        logger: Option<Rc<logging::LogTree>>,
    ) {
        if let Some(err) = ctx.err() {
            result.err = Some(err);
            return;
        }

        let start = Instant::now();
        // Go: var mu sync.Mutex (PORT: one thread)
        let file_exclude_patterns = self
            .user_preferences
            .parsed_auto_import_file_exclude_patterns(
                self.host.fs().use_case_sensitive_file_names(),
            );
        let mut resolver_options = copy_resolver_options(&self.resolver_options);
        resolver_options.host = Some(self.host.clone());
        resolver_options.compiler_options = Some(Rc::new(CompilerOptions::default()));
        let module_resolver = Rc::new(module::new_resolver(resolver_options));
        let program = self
            .host
            .get_program_for_project(project_id)
            .unwrap_or_else(|| crate::core::go_nil_dereference());
        let program = &*program;
        let project_root_path = (self.base.to_path)(&program.get_current_directory());
        let symlink_cache = program.get_symlink_cache();
        let (get_checker, close_pool, checker_count) = create_checker_pool(program);
        // PORT: Go map order is random; insertion (program file) order here.
        let mut exports: IndexMap<tspath::Path, Vec<Rc<Export>>> = IndexMap::new();
        let mut skipped_file_count = 0;
        let combined_stats = ExtractorStats::default();

        for file in program.get_source_files() {
            if file.is_content_mapper_supplemental() || is_ignored_file(program, file) {
                continue;
            }
            if let Some(file_exclude_patterns) = &file_exclude_patterns {
                if file_exclude_patterns.match_string(file.file_name()) {
                    skipped_file_count += 1;
                    continue;
                }
            }
            // Ordinary node_modules files are owned by node_modules buckets. Content-mapped files are not
            // discovered by those buckets, but files already transformed in the Program can be indexed here.
            if file.content_mapper().is_empty()
                && (file.file_name().contains("/node_modules/")
                    || has_symlink_to_node_modules(
                        file.path(),
                        &project_root_path,
                        Some(&*symlink_cache),
                    ))
            {
                continue;
            }
            // Go: wg.Go(func() {...})
            go_wait_group_goroutine(|| {
                if ctx.err().is_none() {
                    let (checker, done) = get_checker();
                    {
                        let mut checker_ref = checker.borrow_mut();
                        let mut extractor = self.new_export_extractor(
                            "",
                            &mut checker_ref,
                            module_resolver.clone(),
                            None,
                        );
                        let file_exports = extractor.extract_from_file(file.root);
                        exports.insert(file.path().clone(), file_exports);
                        let stats = extractor.stats();
                        combined_stats
                            .exports
                            .set(combined_stats.exports.get() + stats.exports.get());
                        combined_stats
                            .used_checker
                            .set(combined_stats.used_checker.get() + stats.used_checker.get());
                    }
                    // Go: defer done()
                    done.call();
                }
            });
        }

        // Go: wg.Wait()

        let index_start = Instant::now();
        let mut idx: Index<Rc<Export>> = Index::default();
        let mut paths: FxHashMap<tspath::Path, String> = FxHashMap::default();
        paths.reserve(exports.len());
        for (path, file_exports) in &exports {
            paths.insert(path.clone(), String::new()); // Empty string for project buckets
            for exp in file_exports {
                idx.insert_as_words(exp.clone());
            }
        }

        result.bucket = Some(Rc::new(RegistryBucket {
            paths: Rc::new(paths),
            index: Some(Rc::new(RefCell::new(idx))),
            resolved_package_names,
            state: RefCell::new(BucketState {
                build_preferences: bucket_build_preferences_from_user_preferences(
                    &self.user_preferences,
                ),
                ..Default::default()
            }),
            ..Default::default()
        }));

        if logger.is_some() {
            logger.logf(&format!(
                "Extracted exports: {:?} ({} exports, {} used checker, {} created checkers)",
                index_start.duration_since(start),
                combined_stats.exports.get(),
                combined_stats.used_checker.get(),
                checker_count()
            ));
            if skipped_file_count > 0 {
                logger.logf(&format!(
                    "Skipped {skipped_file_count} files due to exclude patterns"
                ));
            }
            logger.logf(&format!("Built index: {:?}", index_start.elapsed()));
            logger.logf(&format!("Bucket total: {:?}", start.elapsed()));
        }

        // Go: defer closePool()
        close_pool();
    }

    // Go: ls/autoimport/registry.go:1321 computeDependenciesForNodeModulesDirectory
    // PORT: Go returns a `*collections.Set[string]`; nil is `None`.
    pub fn compute_dependencies_for_node_modules_directory(
        &self,
        change: &RegistryChange,
        all_resolved_package_names: &FxHashMap<ProjectID, Rc<FxHashSet<String>>>,
        dir_name: &str,
        dir_path: &tspath::Path,
    ) -> Option<FxHashSet<String>> {
        // If any open files are in scope of this directory but not in scope of any package.json,
        // we need to add all packages in this node_modules directory.
        for path in change.open_files.keys() {
            if dir_path.contains_path(path)
                && self
                    .get_nearest_ancestor_directory_with_package_json(path)
                    .is_none()
            {
                return None;
            }
        }

        // Get all package.jsons that have this node_modules directory in their spine
        let mut dependencies: FxHashSet<String> = FxHashSet::default();
        self.directories.range(&mut |entry: &Rc<
            dirty::MapEntry<tspath::Path, Rc<RefCell<Directory>>>,
        >| {
            let value = entry
                .value()
                .unwrap_or_else(|| crate::core::go_nil_dereference());
            let value = value.borrow();
            if value.package_json.as_ref().is_some_and(|p| p.exists())
                && dir_path.contains_path(&entry.key())
            {
                let contents = value
                    .package_json
                    .as_ref()
                    .unwrap()
                    .contents
                    .as_ref()
                    .unwrap_or_else(|| crate::core::go_nil_dereference());
                add_package_json_dependencies(contents, &mut dependencies);
            }
            true
        });

        // Add packages that are directly imported by programs but not listed in package.json.
        // This ensures node_modules files are always in node_modules buckets.
        // Include packages from all projects that have this node_modules directory in their spine.
        for resolved_package_names in all_resolved_package_names.values() {
            for name in resolved_package_names.iter() {
                dependencies.insert(name.clone());
            }
        }

        Some(dependencies)
    }
}

// Go: ls/autoimport/registry.go:1356 discoveredPackage
// discoveredPackage represents a package found during the discovery phase.
// It holds the resolved package.json and realpath for deduplication.
// When both a real package and a corresponding @types package exist (e.g., react + @types/react),
// both are stored so extraction can fall back to the @types package if the real package has no
// TypeScript entrypoints.
#[derive(Clone, Debug, Default)]
pub struct DiscoveredPackage {
    pub package_name: String,
    pub package_json: Option<Rc<packagejson::InfoCacheEntry>>,
    pub realpath: String,
    pub types_package_json: Option<Rc<packagejson::InfoCacheEntry>>,
    pub types_realpath: String,
    pub dir_path: tspath::Path, // bucket directory path (used as extraction context)
    pub is_local: bool,         // true if realpath is within the workspace root
}

// Go: ls/autoimport/registry.go:1369 perPackageExtractionResult
// perPackageExtractionResult holds the extraction output for one physical package.
// Produced once per unique realpath during the extraction phase, then installed
// into every bucket that needs it during the bucket-building phase.
// PORT: the alias resolver callback adds to the two failed-lookup fields
// while the result is being built, so they are shared `Rc<RefCell<..>>`.
pub struct PerPackageExtractionResult {
    pub package_files: FxHashMap<tspath::Path, String>,
    pub entrypoints: Vec<Rc<module::ResolvedEntrypoint>>,
    pub exports: IndexMap<tspath::Path, Vec<Rc<Export>>>,
    pub ambient_modules: FxHashMap<String, Vec<String>>,
    pub stats_exports: i32,
    pub stats_used_checker: i32,
    pub skipped_entrypoints: i32,
    pub is_symlinked: bool,
    pub failed_ambient_module_lookup_sources:
        Rc<RefCell<IndexMap<tspath::Path, Rc<RefCell<FailedAmbientModuleLookupSource>>>>>,
    pub failed_ambient_module_lookup_targets: Rc<RefCell<IndexSet<String>>>,
}

// Go: ls/autoimport/registry.go:1383 packageExtractionResult
// packageExtractionResult holds the results of extracting exports from a set of packages.
pub struct PackageExtractionResult {
    pub exports: IndexMap<tspath::Path, Vec<Rc<Export>>>,
    pub package_files: FxHashMap<String, FxHashMap<tspath::Path, String>>,
    pub ambient_module_names: FxHashMap<String, Vec<String>>,
    pub entrypoints: Vec<Vec<Rc<module::ResolvedEntrypoint>>>,
    pub workspace_packages: FxHashSet<String>,
    pub possible_failed_ambient_module_lookup_sources:
        IndexMap<tspath::Path, Rc<RefCell<FailedAmbientModuleLookupSource>>>,
    pub possible_failed_ambient_module_lookup_targets: IndexSet<String>,
    pub stats: ExtractorStats,
    pub skipped_entrypoints_count: i32,
}

impl RegistryBuilder {
    // Go: ls/autoimport/registry.go:1397 discoverBucketPackages
    // discoverBucketPackages resolves the package.json and realpath for each package name
    // in a node_modules directory. This is the discovery phase of the three-phase extraction pipeline.
    // PORT: `ctx` is the port's, for `should_stop_build` (Go has no context
    // here). A stopped build gets a partial list, and `update_indexes`
    // returns after discovery.
    pub fn discover_bucket_packages(
        &self,
        ctx: &Context,
        package_names: Option<&FxHashSet<String>>,
        dir_name: &str,
        dir_path: &tspath::Path,
    ) -> Vec<Rc<DiscoveredPackage>> {
        let mut result: Vec<Rc<DiscoveredPackage>> = Vec::with_capacity(set_len(package_names));
        for package_name in package_names.into_iter().flatten() {
            if should_stop_build(ctx) {
                break;
            }
            let types_package_name = module::get_types_package_name(package_name);
            let package_json = self.host.get_package_json(&tspath::combine_paths(
                dir_name,
                &["node_modules", package_name.as_str(), "package.json"],
            ));
            let mut types_package_json: Option<Rc<packagejson::InfoCacheEntry>> = None;
            if *package_name != types_package_name {
                let types_json = self.host.get_package_json(&tspath::combine_paths(
                    dir_name,
                    &["node_modules", types_package_name.as_str(), "package.json"],
                ));
                if types_json
                    .as_ref()
                    .unwrap_or_else(|| crate::core::go_nil_dereference())
                    .directory_exists
                {
                    types_package_json = types_json;
                }
            }
            let mut realpath = String::new();
            let package_json_entry = package_json
                .as_ref()
                .unwrap_or_else(|| crate::core::go_nil_dereference());
            if package_json_entry.directory_exists {
                realpath = self
                    .host
                    .fs()
                    .realpath(&package_json_entry.package_directory);
            }
            let mut types_realpath = String::new();
            if let Some(types_package_json) = &types_package_json {
                types_realpath = self
                    .host
                    .fs()
                    .realpath(&types_package_json.package_directory);
            }
            let is_local = !realpath.is_empty()
                && !realpath.contains("/node_modules/")
                && tspath::contains_path(
                    self.host.get_current_directory(),
                    &realpath,
                    &tspath::ComparePathsOptions {
                        use_case_sensitive_file_names: self
                            .host
                            .fs()
                            .use_case_sensitive_file_names(),
                        ..Default::default()
                    },
                );
            result.push(Rc::new(DiscoveredPackage {
                package_name: package_name.clone(),
                package_json,
                realpath,
                types_package_json,
                types_realpath,
                dir_path: dir_path.clone(),
                is_local,
            }));
        }
        result
    }

    // Go: ls/autoimport/registry.go:1444 extractPackage
    // extractPackage extracts exports from a single package.json.
    // This runs once per unique realpath during the extraction phase.
    // Returns nil if the package has no extractable entrypoints.
    pub fn extract_package(
        &self,
        ctx: &Context,
        package_json: &Option<Rc<packagejson::InfoCacheEntry>>,
        package_name: &str,
        project_reference_outputs: &FxHashMap<tspath::Path, String>,
        file_exclude_patterns: Option<&vfs::SpecMatcher>,
        enable_directory_search: bool,
    ) -> Option<Rc<PerPackageExtractionResult>> {
        let Some(package_json_entry) = package_json.as_ref() else {
            return None;
        };
        if !package_json_entry.directory_exists {
            return None;
        }
        let (to_realpath, to_symlink) = get_package_realpath_funcs(
            Rc::new(RegistryCloneHostFs {
                host: self.host.clone(),
            }),
            &package_json_entry.package_directory,
        );
        let resolver = get_module_resolver(
            &self.host,
            to_realpath.clone(),
            copy_resolver_options(&self.resolver_options),
        );
        let mut package_entrypoints = resolver.get_entrypoints_from_package_json_info(
            package_json,
            package_name,
            enable_directory_search,
        );
        // Go: packageEntrypoints == nil (PORT: a Go nil slice is an empty `Vec`)
        if package_entrypoints.is_empty() {
            return None;
        }

        let mut skipped_entrypoints = 0;
        if let Some(file_exclude_patterns) = file_exclude_patterns {
            let count = package_entrypoints.len();
            // Go: slices.DeleteFunc
            package_entrypoints.retain(|entrypoint| {
                !file_exclude_patterns.match_string(&entrypoint.resolved_file_name)
            });
            skipped_entrypoints = (count - package_entrypoints.len()) as i32;
        }
        if package_entrypoints.is_empty() {
            return None;
        }

        // PORT: `result` is made for each checker below.
        let mut is_symlinked = false;

        // Resolve entrypoint source files and build the alias resolver.
        let mut seen_files: FxHashSet<tspath::Path> = FxHashSet::default();
        seen_files.reserve(package_entrypoints.len());
        let mut root_files: Vec<Node> = vec![Node::NIL; package_entrypoints.len()];
        let mut symlinks: FxHashMap<tspath::Path, PathAndFileName> = FxHashMap::default();
        for (i, entrypoint) in package_entrypoints.iter().enumerate() {
            let mut file_name = entrypoint.symlink_or_realpath();
            let mut realpath_file_name = entrypoint.resolved_file_name.clone();
            let mut realpath_path = (self.base.to_path)(&realpath_file_name);

            if let Some(input_file_name) = project_reference_outputs.get(&realpath_path) {
                file_name = to_symlink(input_file_name);
                realpath_file_name = input_file_name.clone();
                realpath_path = (self.base.to_path)(&realpath_file_name);
            }

            // Go: seenFiles.AddIfAbsent(realpathPath)
            if !seen_files.insert(realpath_path.clone()) {
                continue;
            }
            if file_name != realpath_file_name {
                let symlink_path = (self.base.to_path)(&file_name);
                symlinks.insert(
                    realpath_path.clone(),
                    PathAndFileName {
                        path: symlink_path,
                        file_name: file_name.clone(),
                    },
                );
                is_symlinked = true;
            }
            // PORT: Go parses these files on goroutines, so a request never
            // waits for them. Here they run on the dispatch thread, and the
            // next request waits for this loop. So a canceled context stops it
            // before each file. A canceled context discards the whole build
            // (every bucket build then sets its error, and a canceled warm
            // drops its clone), so returning early changes no result.
            if ctx.err().is_some() {
                return None;
            }
            // Go: wg.Go(func() {...})
            go_wait_group_goroutine(|| {
                let file = self
                    .host
                    .get_source_file(&realpath_file_name, &realpath_path);
                if file.is_some() {
                    bind_alias_resolver_source_file(self.host.get_current_directory(), file);
                }
                root_files[i] = file;
            });
        }
        // Go: wg.Wait()
        root_files.retain(|f| f.is_some());
        // PORT: see the check in the loop above. `new_checker` also reads the
        // files that the root files import.
        if ctx.err().is_some() {
            return None;
        }

        // PERF: a checker with the narrow walk first
        // (`AliasResolver::new_narrow_checker`). When it asks for a file
        // outside the walk, that checker and every result of it are dropped,
        // and the package is extracted again with the full walk. A narrow
        // checker with no miss gives the results of the full walk.
        for narrow in [true, false] {
            let mut result = PerPackageExtractionResult {
                package_files: FxHashMap::default(),
                entrypoints: package_entrypoints.clone(),
                exports: IndexMap::new(),
                ambient_modules: FxHashMap::default(),
                stats_exports: 0,
                stats_used_checker: 0,
                skipped_entrypoints,
                is_symlinked,
                failed_ambient_module_lookup_sources: Rc::new(RefCell::new(IndexMap::new())),
                failed_ambient_module_lookup_targets: Rc::new(RefCell::new(IndexSet::new())),
            };
            let failed_targets = result.failed_ambient_module_lookup_targets.clone();
            let failed_sources = result.failed_ambient_module_lookup_sources.clone();
            let alias_resolver = new_alias_resolver(
                root_files.clone(),
                symlinks.clone(),
                self.host.clone(),
                resolver.clone(),
                self.base.to_path.clone(),
                Rc::new(move |source: &dyn HasFileName, module_name: &str| {
                    failed_targets.borrow_mut().insert(module_name.to_string());
                    let source_path = source.path();
                    let exists = failed_sources.borrow().contains_key(&source_path);
                    if !exists {
                        failed_sources.borrow_mut().insert(
                            source_path,
                            Rc::new(RefCell::new(FailedAmbientModuleLookupSource {
                                file_name: source.file_name(),
                                package_name: String::new(),
                            })),
                        );
                    }
                }),
            );

            // PORT: `None` only for a stopped build (`should_stop_build`).
            let (ch, _alias_program) = if narrow {
                alias_resolver.new_narrow_checker(ctx)?
            } else {
                alias_resolver.new_checker(ctx, &[])?
            };
            let mut ch_ref = ch.borrow_mut();
            let mut extractor = self.new_export_extractor(
                package_name,
                &mut ch_ref,
                resolver.clone(),
                Some(to_realpath.clone()),
            );

            let mut non_module_files: FxHashSet<tspath::Path> = FxHashSet::default();
            for entrypoint in alias_resolver.root_files.clone() {
                if ctx.err().is_some() {
                    return None;
                }
                let file_exports = extractor.extract_from_file(entrypoint);
                if alias_resolver.missed.get() {
                    break;
                }
                let entrypoint_path = tspath::Path(source_file_info(entrypoint).path.clone());
                let entrypoint_file_name = source_file_file_name(entrypoint).to_string();
                for name in &source_file_info(entrypoint).ambient_module_names {
                    result
                        .ambient_modules
                        .entry(name.clone())
                        .or_default()
                        .push(entrypoint_file_name.clone());
                }
                result
                    .package_files
                    .insert(entrypoint_path.clone(), entrypoint_file_name.clone());
                let symlink = alias_resolver.symlinks.get(&entrypoint_path).cloned();
                if let Some(symlink) = &symlink {
                    result
                        .package_files
                        .insert(symlink.path.clone(), symlink.file_name.clone());
                }

                let external_module_indicator =
                    source_file_info(entrypoint).external_module_indicator;
                let mut has_exports =
                    !file_exports.is_empty() && external_module_indicator.is_some();
                let source = result
                    .failed_ambient_module_lookup_sources
                    .borrow()
                    .get(&entrypoint_path)
                    .cloned();
                if let Some(source) = source {
                    source.borrow_mut().package_name = package_name.to_string();
                    has_exports = external_module_indicator.is_some();
                } else {
                    result.exports.insert(entrypoint_path.clone(), file_exports);
                }

                if !has_exports {
                    non_module_files.insert(entrypoint_path.clone());
                    if let Some(symlink) = &symlink {
                        non_module_files.insert(symlink.path.clone());
                    }
                }
            }
            if alias_resolver.missed.get() {
                continue;
            }

            // Discard entrypoints for non-module files and empty modules.
            // Go: slices.DeleteFunc
            result.entrypoints.retain(|ep| {
                !non_module_files.contains(&(self.base.to_path)(&ep.resolved_file_name))
            });

            let stats = extractor.stats();
            result.stats_exports = stats.exports.get();
            result.stats_used_checker = stats.used_checker.get();
            return Some(Rc::new(result));
        }
        unreachable!("a checker with the full walk sets no miss")
    }
}

// Go: ls/autoimport/registry.go:1575 installExtractions
// installExtractions aggregates pre-extracted per-package results into a single
// packageExtractionResult for one bucket. This is the install phase of the three-phase pipeline.
pub fn install_extractions(
    discovered: &[Rc<DiscoveredPackage>],
    extraction_cache: &FxHashMap<String, Rc<PerPackageExtractionResult>>,
) -> PackageExtractionResult {
    let mut result = PackageExtractionResult {
        exports: IndexMap::new(),
        package_files: FxHashMap::default(),
        ambient_module_names: FxHashMap::default(),
        entrypoints: Vec::new(),
        workspace_packages: FxHashSet::default(),
        possible_failed_ambient_module_lookup_sources: IndexMap::new(),
        possible_failed_ambient_module_lookup_targets: IndexSet::new(),
        stats: ExtractorStats::default(),
        skipped_entrypoints_count: 0,
    };

    for pkg in discovered {
        let mut extraction = extraction_cache.get(&pkg.realpath);
        if extraction.is_none() {
            extraction = extraction_cache.get(&pkg.types_realpath);
        }
        let Some(extraction) = extraction else {
            continue;
        };
        // Go: maps.Copy(result.exports, extraction.exports)
        for (path, exports) in &extraction.exports {
            result.exports.insert(path.clone(), exports.clone());
        }
        let package_files = result
            .package_files
            .entry(pkg.package_name.clone())
            .or_insert_with(|| {
                let mut m = FxHashMap::default();
                m.reserve(extraction.package_files.len());
                m
            });
        // Go: maps.Copy(result.packageFiles[pkg.packageName], extraction.packageFiles)
        for (path, file_name) in &extraction.package_files {
            package_files.insert(path.clone(), file_name.clone());
        }
        for (name, file_names) in &extraction.ambient_modules {
            result
                .ambient_module_names
                .entry(name.clone())
                .or_default()
                .extend(file_names.iter().cloned());
        }
        // Go: `if extraction.entrypoints != nil`. PORT: the Go slice is never
        // nil here (it comes from a non-empty slice).
        result.entrypoints.push(extraction.entrypoints.clone());
        for (path, source) in extraction
            .failed_ambient_module_lookup_sources
            .borrow()
            .iter()
        {
            // Go: LoadOrStore
            result
                .possible_failed_ambient_module_lookup_sources
                .entry(path.clone())
                .or_insert_with(|| source.clone());
        }
        for target in extraction
            .failed_ambient_module_lookup_targets
            .borrow()
            .iter()
        {
            result
                .possible_failed_ambient_module_lookup_targets
                .insert(target.clone());
        }
        if extraction.is_symlinked && pkg.is_local {
            result.workspace_packages.insert(pkg.package_name.clone());
        }
        result
            .stats
            .exports
            .set(result.stats.exports.get() + extraction.stats_exports);
        result
            .stats
            .used_checker
            .set(result.stats.used_checker.get() + extraction.stats_used_checker);
        result.skipped_entrypoints_count += extraction.skipped_entrypoints;
    }

    result
}

impl RegistryBuilder {
    // Go: ls/autoimport/registry.go:1624 buildNodeModulesBucket
    pub fn build_node_modules_bucket(
        &self,
        ctx: &Context,
        result: &mut BucketBuildResult,
        dependencies: Option<FxHashSet<String>>,
        dir_path: &tspath::Path,
        discovered: &[Rc<DiscoveredPackage>],
        directory_package_names: Option<&FxHashSet<String>>,
        extraction_cache: &FxHashMap<String, Rc<PerPackageExtractionResult>>,
        recursive_search_packages: Option<&FxHashSet<String>>,
        logger: Option<Rc<logging::LogTree>>,
    ) {
        if let Some(err) = ctx.err() {
            result.err = Some(err);
            return;
        }

        let extraction = install_extractions(discovered, extraction_cache);

        let index_start = Instant::now();
        // Build PackageFiles with all directory package names; indexed packages have
        // non-nil maps, unindexed packages have nil maps.
        let mut all_package_files: FxHashMap<String, Option<FxHashMap<tspath::Path, String>>> =
            FxHashMap::default();
        all_package_files.reserve(set_len(directory_package_names));
        for pkg_name in directory_package_names.into_iter().flatten() {
            all_package_files.insert(
                pkg_name.clone(),
                extraction.package_files.get(pkg_name).cloned(),
            );
        }

        // Build Paths as reverse mapping from path to package name.
        // Only include paths for local workspace packages (eligible for granular updates).
        let mut paths: FxHashMap<tspath::Path, String> = FxHashMap::default();
        for pkg_name in &extraction.workspace_packages {
            if let Some(files) = extraction.package_files.get(pkg_name) {
                for path in files.keys() {
                    paths.insert(path.clone(), pkg_name.clone());
                }
            }
        }

        let bucket = Rc::new(RegistryBucket {
            index: Some(Rc::new(RefCell::new(Index::default()))),
            dependency_names: dependencies.map(Rc::new),
            package_files: Some(Rc::new(all_package_files)),
            ambient_module_names: Rc::new(extraction.ambient_module_names.clone()),
            paths: Rc::new(paths),
            state: RefCell::new(BucketState {
                build_preferences: bucket_build_preferences_from_user_preferences(
                    &self.user_preferences,
                ),
                recursive_search_packages: recursive_search_packages.cloned(),
                ..Default::default()
            }),
            resolved_package_names: None,
        });
        result.bucket = Some(bucket.clone());
        result.entrypoints = FxHashMap::default();
        result.entrypoints.reserve(extraction.exports.len());
        result.possible_failed_ambient_module_lookup_sources = Some(
            extraction
                .possible_failed_ambient_module_lookup_sources
                .clone(),
        );
        result.possible_failed_ambient_module_lookup_targets = Some(
            extraction
                .possible_failed_ambient_module_lookup_targets
                .clone(),
        );
        {
            let index = bucket
                .index
                .as_ref()
                .unwrap_or_else(|| crate::core::go_nil_dereference());
            let mut index = index.borrow_mut();
            for file_exports in extraction.exports.values() {
                // PORT: see `should_stop_build`. Go sets `err` at the end of
                // the function; this is the same end state.
                if should_stop_build(ctx) {
                    result.err = ctx.err();
                    return;
                }
                for exp in file_exports {
                    index.insert_as_words(exp.clone());
                }
            }
        }
        for entrypoint_set in &extraction.entrypoints {
            for entrypoint in entrypoint_set {
                let path = (self.base.to_path)(&entrypoint.resolved_file_name);
                result
                    .entrypoints
                    .entry(path)
                    .or_default()
                    .push(entrypoint.clone());
            }
        }

        // Compute old entrypoint paths to remove from the registry-level map.
        // For a full rebuild, all entrypoints belonging to the old bucket's packages must be removed.
        if let (Some(old_entry), true) = self.node_modules.get(dir_path) {
            let old_bucket = old_entry
                .value()
                .unwrap_or_else(|| crate::core::go_nil_dereference());
            if let Some(package_files) = &old_bucket.package_files {
                for files in package_files.values() {
                    for path in files.iter().flat_map(|f| f.keys()) {
                        if self.base.entrypoints.contains_key(path) {
                            result.removed_entrypoint_paths.push(path.clone());
                        }
                    }
                }
            }
        }

        if logger.is_some() {
            logger.logf(&format!(
                "Installed {} exports ({} used checker)",
                extraction.stats.exports.get(),
                extraction.stats.used_checker.get()
            ));
            if extraction.skipped_entrypoints_count > 0 {
                logger.logf(&format!(
                    "Skipped {} entrypoints due to exclude patterns",
                    extraction.skipped_entrypoints_count
                ));
            }
            logger.logf(&format!("Built index: {:?}", index_start.elapsed()));
        }

        result.err = ctx.err();
    }

    // Go: ls/autoimport/registry.go:1713 updateNodeModulesBucket
    // updateNodeModulesBucket performs a granular update of the node_modules bucket,
    // re-extracting only the dirty packages and merging with the existing bucket.
    pub fn update_node_modules_bucket(
        &self,
        ctx: &Context,
        result: &mut BucketBuildResult,
        existing_bucket: &Rc<RegistryBucket>,
        dirty_packages: Option<&FxHashSet<String>>,
        discovered: &[Rc<DiscoveredPackage>],
        extraction_cache: &FxHashMap<String, Rc<PerPackageExtractionResult>>,
        recursive_search_packages: Option<&FxHashSet<String>>,
        logger: Option<Rc<logging::LogTree>>,
    ) {
        if let Some(err) = ctx.err() {
            result.err = Some(err);
            return;
        }

        let start = Instant::now();
        let extraction = install_extractions(discovered, extraction_cache);

        let index_start = Instant::now();

        // Clone the existing index, excluding exports from dirty packages
        let mut new_index = {
            let existing_index = existing_bucket.index.as_ref().map(|index| index.borrow());
            Index::clone_(existing_index.as_deref(), &mut |exp: &Rc<Export>| {
                !set_has(dirty_packages, &exp.package_name)
            })
        };

        // Clone PackageFiles, removing dirty packages
        // Go: maps.Clone (a nil map stays nil)
        let mut new_package_files: Option<
            FxHashMap<String, Option<FxHashMap<tspath::Path, String>>>,
        > = existing_bucket
            .package_files
            .as_ref()
            .map(|m| (**m).clone());
        for pkg_name in dirty_packages.into_iter().flatten() {
            if let Some(new_package_files) = new_package_files.as_mut() {
                new_package_files.remove(pkg_name);
            }
        }
        // Add newly extracted package files
        // Go: maps.Copy(newPackageFiles, extraction.packageFiles) (assignment to a nil map panics)
        for (pkg_name, files) in &extraction.package_files {
            new_package_files
                .as_mut()
                .unwrap_or_else(|| {
                    crate::core::go_panic("assignment to entry in nil map".to_string())
                })
                .insert(pkg_name.clone(), Some(files.clone()));
        }

        // Clone Paths, removing dirty package paths
        let mut new_paths: FxHashMap<tspath::Path, String> = FxHashMap::default();
        new_paths.reserve(existing_bucket.paths.len());
        for (path, pkg_name) in existing_bucket.paths.iter() {
            if set_has(dirty_packages, pkg_name) {
                continue;
            }
            new_paths.insert(path.clone(), pkg_name.clone());
        }
        // Add paths for newly extracted workspace packages
        for pkg_name in &extraction.workspace_packages {
            if let Some(files) = extraction.package_files.get(pkg_name) {
                for path in files.keys() {
                    new_paths.insert(path.clone(), pkg_name.clone());
                }
            }
        }

        // Clone AmbientModuleNames, removing dirty package entries
        let mut new_ambient_module_names: FxHashMap<String, Vec<String>> = FxHashMap::default();
        new_ambient_module_names.reserve(existing_bucket.ambient_module_names.len());
        for (module_name, file_names) in existing_bucket.ambient_module_names.iter() {
            // Filter out files from dirty packages
            let mut filtered: Vec<String> = Vec::new();
            for file_name in file_names {
                let path = (self.base.to_path)(file_name);
                if let Some(pkg_name) = existing_bucket.paths.get(&path) {
                    if set_has(dirty_packages, pkg_name) {
                        continue;
                    }
                }
                filtered.push(file_name.clone());
            }
            if !filtered.is_empty() {
                new_ambient_module_names.insert(module_name.clone(), filtered);
            }
        }
        // Add newly extracted ambient module names
        for (module_name, file_names) in &extraction.ambient_module_names {
            new_ambient_module_names
                .entry(module_name.clone())
                .or_default()
                .extend(file_names.iter().cloned());
        }

        // Collect entrypoint paths that need to be removed from the registry-level map
        // (paths belonging to dirty packages)
        let mut removed_entrypoint_paths: Vec<tspath::Path> = Vec::new();
        for path in self.base.entrypoints.keys() {
            if let Some(pkg_name) = existing_bucket.paths.get(path) {
                if set_has(dirty_packages, pkg_name) {
                    removed_entrypoint_paths.push(path.clone());
                }
            }
        }
        // Build new entrypoints from extraction
        let mut new_entrypoints: FxHashMap<tspath::Path, Vec<Rc<module::ResolvedEntrypoint>>> =
            FxHashMap::default();
        for entrypoint_set in &extraction.entrypoints {
            for entrypoint in entrypoint_set {
                let path = (self.base.to_path)(&entrypoint.resolved_file_name);
                new_entrypoints
                    .entry(path)
                    .or_default()
                    .push(entrypoint.clone());
            }
        }

        // Insert newly extracted exports into the index
        for file_exports in extraction.exports.values() {
            // PORT: see `should_stop_build`. Go sets `err` at the end of the
            // function; this is the same end state.
            if should_stop_build(ctx) {
                result.err = ctx.err();
                return;
            }
            for exp in file_exports {
                new_index
                    .as_mut()
                    .unwrap_or_else(|| crate::core::go_nil_dereference())
                    .insert_as_words(exp.clone());
            }
        }

        result.bucket = Some(Rc::new(RegistryBucket {
            index: new_index.map(|index| Rc::new(RefCell::new(index))),
            dependency_names: existing_bucket.dependency_names.clone(),
            package_files: new_package_files.map(Rc::new),
            ambient_module_names: Rc::new(new_ambient_module_names),
            paths: Rc::new(new_paths),
            state: RefCell::new(BucketState {
                build_preferences: bucket_build_preferences_from_user_preferences(
                    &self.user_preferences,
                ),
                recursive_search_packages: recursive_search_packages.cloned(),
                ..Default::default()
            }),
            resolved_package_names: None,
        }));
        result.entrypoints = new_entrypoints;
        result.removed_entrypoint_paths = removed_entrypoint_paths;
        result.possible_failed_ambient_module_lookup_sources = Some(
            extraction
                .possible_failed_ambient_module_lookup_sources
                .clone(),
        );
        result.possible_failed_ambient_module_lookup_targets = Some(
            extraction
                .possible_failed_ambient_module_lookup_targets
                .clone(),
        );

        if logger.is_some() {
            logger.logf(&format!(
                "Granular update of {} packages: {:?} ({} exports)",
                set_len(dirty_packages),
                index_start.duration_since(start),
                extraction.stats.exports.get()
            ));
            logger.logf(&format!("Built index: {:?}", index_start.elapsed()));
        }

        result.err = ctx.err();
    }

    // Go: ls/autoimport/registry.go:1832 getNearestAncestorDirectoryWithPackageJson
    pub fn get_nearest_ancestor_directory_with_package_json(
        &self,
        file_path: &tspath::Path,
    ) -> Option<Rc<RefCell<Directory>>> {
        file_path
            .get_directory_path()
            .for_each_ancestor_directory(
                |dir_path: tspath::Path| -> (Option<Rc<RefCell<Directory>>>, bool) {
                    if let (Some(dir_entry), true) = self.directories.get(&dir_path) {
                        let value = dir_entry
                            .value()
                            .unwrap_or_else(|| crate::core::go_nil_dereference());
                        if value
                            .borrow()
                            .package_json
                            .as_ref()
                            .is_some_and(|p| p.exists())
                        {
                            return (Some(value), true);
                        }
                    }
                    (None, false)
                },
            )
            .0
    }

    // Go: ls/autoimport/registry.go:1841 resolveAmbientModuleName
    pub fn resolve_ambient_module_name(
        &self,
        module_name: &str,
        from_path: &tspath::Path,
    ) -> Vec<String> {
        from_path
            .for_each_ancestor_directory(|dir_path: tspath::Path| -> (Vec<String>, bool) {
                if let (Some(bucket), true) = self.node_modules.get(&dir_path) {
                    if let Some(file_names) = bucket
                        .value()
                        .unwrap_or_else(|| crate::core::go_nil_dereference())
                        .ambient_module_names
                        .get(module_name)
                    {
                        return (file_names.clone(), true);
                    }
                }
                (Vec::new(), false)
            })
            .0
    }
}
