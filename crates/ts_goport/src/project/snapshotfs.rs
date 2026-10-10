//! Go `internal/project/snapshotfs.go`.
//!
//! PORT: one thread (project/dirty/interfaces.rs). Go mutexes are dropped.
//! `collections.SyncMap` / `SyncSet` are `RefCell<FxHashMap>` /
//! `RefCell<FxHashSet>`; `collections.Set` is `FxHashSet`. A shared Go
//! `*collections.SyncSet` is `Rc<RefCell<FxHashSet>>` (nil is `None`). Go
//! `func(fileName string) tspath.Path` is `Rc<dyn Fn(&str) -> tspath::Path>`.
//! Overlay maps are `IndexMap` (see overlayfs.rs). Go `xxh3.Uint128` is
//! `u128`.

use crate::project::prelude::*;

use crate::frontend::compiler::resolve_ahead::AheadLookups;
use std::cell::{Cell, OnceCell};
use std::sync::Arc;
use std::time::SystemTime;
use xxhash_rust::xxh3::xxh3_128;

// Go: project/snapshotfs.go:21 FileHandleSource (ts#64291)
pub trait FileHandleSource {
    fn get_file(&self, file_name: &str) -> Option<Rc<dyn FileHandle>>;
    fn get_file_by_path(&self, file_name: &str, path: &tspath::Path) -> Option<Rc<dyn FileHandle>>;
}

// Go: project/snapshotfs.go:26 FileSource
pub trait FileSource: FileHandleSource {
    fn fs(&self) -> Rc<dyn vfs::Fs>;
    fn file_exists(&self, file_name: &str, path: &tspath::Path) -> bool;
    fn get_accessible_entries(&self, path: &str) -> vfs::Entries;

    /// Go `source.FS().UseCaseSensitiveFileNames()`. A released source
    /// (`SourceFS::release`) answers it with no file system.
    // PORT: not in Go.
    fn use_case_sensitive_file_names(&self) -> bool {
        self.fs().use_case_sensitive_file_names()
    }
}

// Go: project/snapshotfs.go:33 cachedLayeredFileSystem (ts#64291)
// PORT: Go embeds `*cachedvfs.FS`; here it is the field `fs`, and the
// `vfs.FS` methods forward to it.
pub struct CachedLayeredFileSystem {
    pub fs: Rc<vfs::CachedFs>,
    pub layered: Rc<dyn LayeredFileSystem>,
}

// Go: project/snapshotfs.go:38 newCachedLayeredFileSystem
// PORT: the cache asks `layered` through an `AheadLookupLayer`.
pub fn new_cached_layered_file_system(
    file_system: Rc<dyn LayeredFileSystem>,
) -> Rc<dyn LayeredFileSystem> {
    let ahead = Rc::new(AheadLookupLayer {
        layered: file_system.clone(),
        jobs: RefCell::default(),
    });
    Rc::new(CachedLayeredFileSystem {
        fs: vfs::cachedvfs_from(ahead),
        layered: file_system,
    })
}

/// The layer under a snapshot's lookup cache (`CachedLayeredFileSystem`)
/// that answers `file_exists`, `directory_exists` and `realpath` from the
/// lookups of the resolve-ahead jobs of the snapshot's program loads
/// (compiler/resolve_ahead.rs `AheadLookups`) before it asks the layered
/// file system. The workers made these calls during the loads, as Go's
/// parse tasks make theirs on the one cache of the snapshot, so the
/// snapshot has their answers with no copy. The job that was attached first
/// gives the answer.
// PORT: not in Go (perf).
pub struct AheadLookupLayer {
    layered: Rc<dyn LayeredFileSystem>,
    jobs: RefCell<Vec<Arc<dyn AheadLookups>>>,
}

impl AheadLookupLayer {
    /// Adds the lookups of a load's job (`ResolveAheadHost::attach`), and
    /// gives their index.
    pub fn attach(&self, lookups: Arc<dyn AheadLookups>) -> usize {
        let mut jobs = self.jobs.borrow_mut();
        jobs.push(lookups);
        jobs.len() - 1
    }

    /// The first answer of `get` in the jobs attached before job `index`.
    pub fn before<T>(
        &self,
        index: usize,
        get: impl Fn(&dyn AheadLookups) -> Option<T>,
    ) -> Option<T> {
        self.jobs
            .borrow()
            .iter()
            .take(index)
            .find_map(|lookups| get(&**lookups))
    }

    fn first<T>(&self, get: impl Fn(&dyn AheadLookups) -> Option<T>) -> Option<T> {
        self.before(usize::MAX, get)
    }
}

impl Drop for AheadLookupLayer {
    /// The workers allocated the lookups, and they free them.
    fn drop(&mut self) {
        let jobs = std::mem::take(&mut *self.jobs.borrow_mut());
        if !jobs.is_empty() {
            crate::frontend::compiler::resolve_ahead::drop_on_worker(Box::new(jobs));
        }
    }
}

impl vfs::Fs for AheadLookupLayer {
    fn use_case_sensitive_file_names(&self) -> bool {
        self.layered.use_case_sensitive_file_names()
    }
    fn file_exists(&self, path: &str) -> bool {
        self.first(|lookups| lookups.file_exists(path))
            .unwrap_or_else(|| self.layered.file_exists(path))
    }
    fn read_file(&self, path: &str) -> (String, bool) {
        self.layered.read_file(path)
    }
    fn write_file(&self, path: &str, data: &str) -> Result<(), vfs::FsError> {
        self.layered.write_file(path, data)
    }
    fn append_file(&self, path: &str, data: &str) -> Result<(), vfs::FsError> {
        self.layered.append_file(path, data)
    }
    fn remove(&self, path: &str) -> Result<(), vfs::FsError> {
        self.layered.remove(path)
    }
    fn chtimes(
        &self,
        path: &str,
        a_time: Option<SystemTime>,
        m_time: Option<SystemTime>,
    ) -> Result<(), vfs::FsError> {
        self.layered.chtimes(path, a_time, m_time)
    }
    fn directory_exists(&self, path: &str) -> bool {
        self.first(|lookups| lookups.directory_exists(path))
            .unwrap_or_else(|| self.layered.directory_exists(path))
    }
    fn get_accessible_entries(&self, path: &str) -> vfs::Entries {
        self.layered.get_accessible_entries(path)
    }
    fn stat(&self, path: &str) -> Option<vfs::FileInfo> {
        self.layered.stat(path)
    }
    fn realpath(&self, path: &str) -> String {
        self.first(|lookups| lookups.realpath(path))
            .unwrap_or_else(|| self.layered.realpath(path))
    }
    fn as_any(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }
}

/// The `AheadLookupLayer` under the lookup cache `cached` of a snapshot
/// (`new_cached_layered_file_system`).
pub fn ahead_lookup_layer(cached: &vfs::CachedFs) -> Option<&AheadLookupLayer> {
    vfs::Fs::as_any(&**cached.wrapped())?.downcast_ref::<AheadLookupLayer>()
}

impl FileHandleSource for CachedLayeredFileSystem {
    // Go: project/snapshotfs.go:45 cachedLayeredFileSystem.GetFile
    fn get_file(&self, file_name: &str) -> Option<Rc<dyn FileHandle>> {
        self.layered.get_file(file_name)
    }

    // Go: project/snapshotfs.go:49 cachedLayeredFileSystem.GetFileByPath
    fn get_file_by_path(&self, file_name: &str, path: &tspath::Path) -> Option<Rc<dyn FileHandle>> {
        self.layered.get_file_by_path(file_name, path)
    }
}

impl LayeredFileSystem for CachedLayeredFileSystem {
    // Go: project/snapshotfs.go:53 cachedLayeredFileSystem.Overlays
    fn overlays(&self) -> Rc<IndexMap<tspath::Path, Rc<Overlay>>> {
        self.layered.overlays()
    }
}

impl FileChangeExpander for CachedLayeredFileSystem {
    // Go: project/snapshotfs.go:57 cachedLayeredFileSystem.ExpandFileChanges
    fn expand_file_changes(&self, change: FileChangeSummary) -> FileChangeSummary {
        if let Some(expander) = as_file_change_expander(&*self.layered) {
            return expander.expand_file_changes(change);
        }
        change
    }
}

impl FsLayer for CachedLayeredFileSystem {
    fn as_file_handle_source(&self) -> Option<&dyn FileHandleSource> {
        Some(self)
    }
    fn as_layered_file_system(&self) -> Option<&dyn LayeredFileSystem> {
        Some(self)
    }
    fn as_file_change_expander(&self) -> Option<&dyn FileChangeExpander> {
        Some(self)
    }
}

// PORT: the promoted `*cachedvfs.FS` methods.
impl vfs::Fs for CachedLayeredFileSystem {
    fn use_case_sensitive_file_names(&self) -> bool {
        self.fs.use_case_sensitive_file_names()
    }
    fn file_exists(&self, path: &str) -> bool {
        self.fs.file_exists(path)
    }
    fn read_file(&self, path: &str) -> (String, bool) {
        self.fs.read_file(path)
    }
    fn write_file(&self, path: &str, data: &str) -> Result<(), vfs::FsError> {
        self.fs.write_file(path, data)
    }
    fn append_file(&self, path: &str, data: &str) -> Result<(), vfs::FsError> {
        self.fs.append_file(path, data)
    }
    fn remove(&self, path: &str) -> Result<(), vfs::FsError> {
        self.fs.remove(path)
    }
    fn chtimes(
        &self,
        path: &str,
        a_time: Option<SystemTime>,
        m_time: Option<SystemTime>,
    ) -> Result<(), vfs::FsError> {
        self.fs.chtimes(path, a_time, m_time)
    }
    fn directory_exists(&self, path: &str) -> bool {
        self.fs.directory_exists(path)
    }
    fn get_accessible_entries(&self, path: &str) -> vfs::Entries {
        self.fs.get_accessible_entries(path)
    }
    fn stat(&self, path: &str) -> Option<vfs::FileInfo> {
        self.fs.stat(path)
    }
    fn realpath(&self, path: &str) -> String {
        self.fs.realpath(path)
    }
    // PORT: Go type assertions on a `vfs.FS` (see `project::as_fs_layer`).
    fn as_any(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }
}

// Go: project/snapshotfs.go:82 realpathAliasSet
// realpathAliasSet is a thread-safe set of symlink paths that alias a single realpath.
// It implements dirty.Cloneable so it can be used as a value in dirty.SyncMap.
// PORT: `mu` is dropped. Go `*realpathAliasSet` is `Rc<RefCell<..>>`.
// ts#64159: each alias path keeps the file name it was seen as
// (`aliasPaths`, snapshotfs.go:71).
#[derive(Debug, Default)]
pub struct RealpathAliasSet {
    pub paths: FxHashMap<tspath::Path, String>,
}

impl RealpathAliasSet {
    // Go: project/snapshotfs.go:87 realpathAliasSet.Add
    pub fn add(&mut self, path: tspath::Path, file_name: String) {
        self.paths.insert(path, file_name);
    }

    // Go: project/snapshotfs.go:96 realpathAliasSet.Clone
    // PORT: Go `Clone`; `clone_` keeps it apart from `std::clone::Clone`.
    pub fn clone_(&self) -> Rc<RefCell<RealpathAliasSet>> {
        let mut clone = RealpathAliasSet::default();
        if !self.paths.is_empty() {
            clone.paths = self.paths.clone();
        }
        Rc::new(RefCell::new(clone))
    }
}

impl dirty::Cloneable for Rc<RefCell<RealpathAliasSet>> {
    fn clone_(&self) -> Self {
        self.borrow().clone_()
    }
}

// Go: project/snapshotfs.go:104 SnapshotFS
// PORT: Go shares `cacheFiles`, `cacheDirectories` and
// `nodeModulesRealpathAliases` between snapshots until one of them changes
// (a Go map is a reference). They are `Rc` maps here, so a snapshot clone
// with no cache change copies no map, and dropping an old snapshot frees
// nothing that the next one still uses.
pub struct SnapshotFS {
    pub to_path: Rc<dyn Fn(&str) -> tspath::Path>,
    pub fs: Rc<dyn LayeredFileSystem>,
    pub cache_files: Rc<FxHashMap<tspath::Path, Rc<RefCell<CachedFile>>>>,
    pub cache_directories: Rc<FxHashMap<tspath::Path, CachedDirectory>>,
    pub read_files: RefCell<FxHashMap<tspath::Path, MemoizedCachedFile>>,
    // nodeModulesRealpathAliases maps realpath-based keys to sets of symlink-based keys,
    // for files inside node_modules that are accessed through directory symlinks.
    // This allows watch events (which use realpaths) to invalidate files cached under symlink paths.
    pub node_modules_realpath_aliases: Rc<FxHashMap<tspath::Path, Rc<RefCell<RealpathAliasSet>>>>,
}

// Go: project/snapshotfs.go:116 memoizedCachedFile
// PORT: Go `func() FileHandle` made by `sync.OnceValue`; the closure keeps
// its value in a `OnceCell`.
pub type MemoizedCachedFile = Rc<dyn Fn() -> Option<Rc<dyn FileHandle>>>;

impl SnapshotFS {
    // Go: project/snapshotfs.go:589 SnapshotFS.expandRealpathAliases
    // expandRealpathAliases adds synthetic URIs to the Changed and Deleted sets for
    // files that were accessed through node_modules symlinks. When a watch event arrives
    // using a realpath, this expands it to include the symlink-based path so that
    // downstream consumers (markDirtyFiles, markFilesChanged) can find cached entries.
    pub fn expand_realpath_aliases(&self, mut change: FileChangeSummary) -> FileChangeSummary {
        if self.node_modules_realpath_aliases.is_empty() {
            return change;
        }

        let mut additional_changed: FxHashSet<lsproto::DocumentUri> = FxHashSet::default();
        for uri in &change.changed {
            let path = (self.to_path)(&uri.file_name());
            if let Some(aliases) = self.node_modules_realpath_aliases.get(&path) {
                // ts#64159: the URI of the alias's file name.
                for alias_file_name in aliases.borrow().paths.values() {
                    additional_changed.insert(lsconv::file_name_to_document_uri(alias_file_name));
                }
            }
        }
        for uri in additional_changed {
            change.changed.insert(uri);
        }

        let mut additional_deleted: FxHashSet<lsproto::DocumentUri> = FxHashSet::default();
        for uri in &change.deleted {
            let path = (self.to_path)(&uri.file_name());
            if let Some(aliases) = self.node_modules_realpath_aliases.get(&path) {
                for alias_file_name in aliases.borrow().paths.values() {
                    additional_deleted.insert(lsconv::file_name_to_document_uri(alias_file_name));
                }
            }
        }
        for uri in additional_deleted {
            change.deleted.insert(uri);
        }

        change
    }
}

// Go: project/snapshotfs.go:66 `_ FileSource = (*SnapshotFS)(nil)`
impl FileHandleSource for SnapshotFS {
    // Go: project/snapshotfs.go:122 SnapshotFS.GetFile
    fn get_file(&self, file_name: &str) -> Option<Rc<dyn FileHandle>> {
        self.get_file_by_path(file_name, &(self.to_path)(file_name))
    }

    // Go: project/snapshotfs.go:133 SnapshotFS.GetFileByPath
    fn get_file_by_path(&self, file_name: &str, path: &tspath::Path) -> Option<Rc<dyn FileHandle>> {
        if let Some(file) = self.cache_files.get(path) {
            return Some(file.clone());
        }
        let fs = self.fs.clone();
        let file_name = file_name.to_string();
        let file_path = path.clone();
        let value: OnceCell<Option<Rc<dyn FileHandle>>> = OnceCell::new();
        let new_entry: MemoizedCachedFile = Rc::new(move || {
            value
                .get_or_init(|| fs.get_file_by_path(&file_name, &file_path))
                .clone()
        });
        // Go: s.readFiles.LoadOrStore(path, newEntry)
        let entry = self
            .read_files
            .borrow_mut()
            .entry(path.clone())
            .or_insert(new_entry)
            .clone();
        entry()
    }
}

impl FileSource for SnapshotFS {
    // Go: project/snapshotfs.go:118 SnapshotFS.FS
    fn fs(&self) -> Rc<dyn vfs::Fs> {
        self.fs.clone()
    }

    // Go: project/snapshotfs.go:126 SnapshotFS.FileExists
    fn file_exists(&self, file_name: &str, path: &tspath::Path) -> bool {
        if self.cache_files.contains_key(path) {
            return true;
        }
        self.fs.file_exists(file_name)
    }

    // Go: project/snapshotfs.go:144 SnapshotFS.GetAccessibleEntries
    fn get_accessible_entries(&self, directory_name: &str) -> vfs::Entries {
        let lower_entries = self.fs.get_accessible_entries(directory_name);
        let Some(directory) = self.cache_directories.get(&(self.to_path)(directory_name)) else {
            return lower_entries;
        };
        merge_cached_directory_entries(
            lower_entries,
            &directory.borrow(),
            // ts#64159: the file system is asked with the directory's name
            // and the child's name, not the path key.
            &|path: &tspath::Path, child_name: &str| {
                let cached = self.cache_files.contains_key(path);
                cached
                    || self
                        .fs
                        .file_exists(&tspath::combine_paths(directory_name, &[child_name]))
            },
            self.fs.use_case_sensitive_file_names(),
        )
    }
}

/// Go `dirty.CloneableMap[tspath.Path, string]` of `cacheDirectories`: the
/// cached children of one directory, by path, with their base names.
// PORT: Go ranges over it (`mergeCachedDirectoryEntries`). A Go map of at
// most 8 entries is one group of 8 slots: an insert takes the first free
// slot, a delete frees its slot, and a range starts at a random slot and
// wraps. `dirty.CloneableMap.Clone` (`maps.Clone`) keeps the slots. It is an
// `IndexMap` (insertion order). With no delete, the port gives the rotation
// that starts at the first slot, which Go can give. A delete keeps the order
// of the other children (`shift_remove`), as Go keeps their slots. Go can
// differ in two cases:
// - A delete and then an add: Go puts the new child in the first free slot,
//   which can be the slot of the deleted child, but the port puts it last.
// - Above 8 children: Go's order depends on the hash seed of the map.
// The FxHashMap of `dirty::CloneableMap` gave the order of the path hashes,
// which Go can miss (editfuzz2 R1, as for `OverlayDirectories`).
pub type CachedDirectory = Rc<RefCell<IndexMap<tspath::Path, String>>>;

impl dirty::Cloneable for CachedDirectory {
    // Go: project/dirty/cloneablemap.go:7 Clone
    fn clone_(&self) -> Self {
        Rc::new(RefCell::new(self.borrow().clone()))
    }
}

// Go: project/snapshotfs.go:156 mergeCachedDirectoryEntries (ts#64291)
// PORT: Go ranges over the cached entries map (random order); insertion
// order here (`CachedDirectory`).
pub fn merge_cached_directory_entries(
    directory_entries: vfs::Entries,
    cached_entries: &IndexMap<tspath::Path, String>,
    is_cached_file: &dyn Fn(&tspath::Path, &str) -> bool,
    use_case_sensitive_file_names: bool,
) -> vfs::Entries {
    let mut entries = vfs::Entries {
        symlinks: directory_entries.symlinks.clone(),
        ..Default::default()
    };
    let equal_name = |left: &str, right: &str| -> bool {
        tspath::get_canonical_file_name(left, use_case_sensitive_file_names)
            == tspath::get_canonical_file_name(right, use_case_sensitive_file_names)
    };
    let has_name = |names: &[String], name: &str| -> bool {
        names.iter().any(|candidate| equal_name(candidate, name))
    };
    for (child_path, child_name) in cached_entries {
        if let Some(symlinks) = &mut entries.symlinks {
            symlinks.retain(|name| !equal_name(name, child_name));
        }
        if is_cached_file(child_path, child_name) {
            entries.files.push(child_name.clone());
        } else {
            entries.directories.push(child_name.clone());
        }
    }
    for file_name in &directory_entries.files {
        if !has_name(&entries.files, file_name) && !has_name(&entries.directories, file_name) {
            entries.files.push(file_name.clone());
        }
    }
    for directory_name in &directory_entries.directories {
        if !has_name(&entries.files, directory_name)
            && !has_name(&entries.directories, directory_name)
        {
            entries.directories.push(directory_name.clone());
        }
    }
    entries
}

// Go: project/snapshotfs.go:189 snapshotFSBuilder
pub struct SnapshotFSBuilder {
    pub fs: Rc<dyn LayeredFileSystem>,
    pub cache_files: Rc<dirty::SyncMap<tspath::Path, Rc<RefCell<CachedFile>>>>,
    pub cache_directories: Rc<dirty::Map<tspath::Path, CachedDirectory>>,
    pub source_backed_replacements: RefCell<FxHashSet<tspath::Path>>,
    pub node_modules_realpath_aliases:
        Rc<dirty::SyncMap<tspath::Path, Rc<RefCell<RealpathAliasSet>>>>,
    pub to_path: Rc<dyn Fn(&str) -> tspath::Path>,
}

// Go: project/snapshotfs.go:198 newSnapshotFSBuilderFromSource (ts#64291)
// PORT: the base maps are shared `Rc` maps, as Go shares its maps (no
// copy).
pub fn new_snapshot_fs_builder_from_source(
    fs: Rc<dyn LayeredFileSystem>,
    cache_files: Rc<FxHashMap<tspath::Path, Rc<RefCell<CachedFile>>>>,
    cache_directories: Rc<FxHashMap<tspath::Path, CachedDirectory>>,
    node_modules_realpath_aliases: Rc<FxHashMap<tspath::Path, Rc<RefCell<RealpathAliasSet>>>>,
    to_path: Rc<dyn Fn(&str) -> tspath::Path>,
) -> Rc<SnapshotFSBuilder> {
    let fs = new_cached_layered_file_system(fs);

    Rc::new(SnapshotFSBuilder {
        fs,
        cache_files: dirty::new_sync_map_shared(cache_files),
        cache_directories: dirty::new_map_shared(cache_directories),
        source_backed_replacements: RefCell::new(FxHashSet::default()),
        node_modules_realpath_aliases: dirty::new_sync_map_shared(node_modules_realpath_aliases),
        to_path,
    })
}

// Go: project/snapshotfs.go:238 onDeletedFileOrDirectory (a closure in Finalize)
// PORT: the recursive Go closure is a nested function over the map it reads.
fn on_deleted_file_or_directory(
    cache_directories: &dirty::Map<tspath::Path, CachedDirectory>,
    path: &tspath::Path,
) {
    let (dir_entry, ok) = cache_directories.get(&path.get_directory_path());
    if !ok {
        return;
    }
    let dir_entry = dir_entry.expect("dirty.Map.Get: ok implies an entry");
    let entry = dir_entry.clone();
    dir_entry.change(&mut |dir: &CachedDirectory| {
        dir.borrow_mut().shift_remove(path);
        if dir.borrow().is_empty() {
            entry.delete();
            on_deleted_file_or_directory(cache_directories, &entry.key());
        }
    });
}

impl SnapshotFSBuilder {
    // Go: project/snapshotfs.go:219 snapshotFSBuilder.Finalize
    pub fn finalize(&self) -> (Rc<SnapshotFS>, bool) {
        // Synchronize directory structure based on added and deleted cache entries.
        let mut deleted: Option<FxHashMap<tspath::Path, Option<Rc<RefCell<CachedFile>>>>> = None;

        let on_added_file = |path: &tspath::Path, file_name: &str| {
            let mut child_path = path.clone();
            let mut child = file_name.to_string();
            loop {
                let parent_path = child_path.get_directory_path();
                let parent = tspath::get_directory_path(&child);
                if child_path == parent_path {
                    break; // reached root
                }
                let base_name = tspath::get_base_file_name(&child);
                if let (Some(dir_entry), true) = self.cache_directories.get(&parent_path) {
                    dir_entry.change(&mut |dir: &CachedDirectory| {
                        dir.borrow_mut()
                            .insert(child_path.clone(), base_name.clone());
                    });
                    break;
                } else {
                    let dir = CachedDirectory::default();
                    dir.borrow_mut().insert(child_path.clone(), base_name);
                    self.cache_directories.add(parent_path.clone(), dir);
                }
                child_path = parent_path;
                child = parent;
            }
        };

        let source_backed_replacements = self.source_backed_replacements.borrow().clone();
        let mut on_delete = |key: &tspath::Path, value: Option<&Rc<RefCell<CachedFile>>>| {
            // ts#64291
            if source_backed_replacements.contains(key) {
                return;
            }
            deleted
                .get_or_insert_with(FxHashMap::default)
                .insert(key.clone(), value.cloned());
        };
        let mut on_add = |key: &tspath::Path, value: Option<&Rc<RefCell<CachedFile>>>| {
            let file_name = value
                .unwrap_or_else(|| crate::core::go_nil_dereference())
                .borrow()
                .file_base
                .file_name();
            on_added_file(key, &file_name);
        };
        // Go: s.cacheFiles.FinalizeWith(...) (PORT: the shared form)
        let (cache_files, changed) = self.cache_files.finalize_shared(dirty::FinalizationHooks {
            on_delete: Some(&mut on_delete),
            on_change: None,
            on_add: Some(&mut on_add),
        });

        // PORT: Go ranges over the `deleted` map (random order); the result
        // does not depend on the order.
        for path in deleted.iter().flat_map(|d| d.keys()) {
            on_deleted_file_or_directory(&self.cache_directories, path);
        }

        // Prune deleted symlink paths from realpath alias sets before finalizing,
        // so that empty sets are dropped during finalization.
        for (deleted_path, deleted_file) in deleted.iter().flatten() {
            let realpath_path = deleted_file
                .as_ref()
                .unwrap_or_else(|| crate::core::go_nil_dereference())
                .borrow()
                .realpath_path
                .clone();
            if realpath_path.0.is_empty() {
                continue;
            }
            if let (Some(entry), true) = self.node_modules_realpath_aliases.load(&realpath_path) {
                entry.locked(&mut |e: &dyn dirty::Value<Rc<RefCell<RealpathAliasSet>>>| {
                    e.change(&mut |alias_set: &Rc<RefCell<RealpathAliasSet>>| {
                        alias_set.borrow_mut().paths.remove(deleted_path);
                    });
                    let is_empty = e
                        .value()
                        .unwrap_or_else(|| crate::core::go_nil_dereference())
                        .borrow()
                        .paths
                        .is_empty();
                    if is_empty {
                        e.delete();
                    }
                });
            }
        }

        // Go: s.nodeModulesRealpathAliases.Finalize() (PORT: the shared form)
        let (node_modules_realpath_aliases, aliases_changed) = self
            .node_modules_realpath_aliases
            .finalize_shared(dirty::FinalizationHooks::default());

        (
            Rc::new(SnapshotFS {
                fs: self.fs.clone(),
                cache_files,
                // Go: core.FirstResult(s.cacheDirectories.Finalize())
                cache_directories: self.cache_directories.finalize_shared().0,
                read_files: RefCell::new(FxHashMap::default()),
                node_modules_realpath_aliases,
                to_path: self.to_path.clone(),
            }),
            changed || aliases_changed,
        )
    }

    // Go: project/snapshotfs.go:316 snapshotFSBuilder.deleteCacheEntry (ts#64291)
    pub fn delete_cache_entry(
        &self,
        entry: &Rc<dirty::SyncMapEntry<tspath::Path, Rc<RefCell<CachedFile>>>>,
    ) {
        if let Some(file) = entry.value()
            && self.fs.file_exists(&file.borrow().file_base.file_name)
        {
            self.source_backed_replacements
                .borrow_mut()
                .insert(entry.key());
        }
        entry.delete();
    }

    // Go: project/snapshotfs.go:359 snapshotFSBuilder.cacheSourceFile (ts#64291)
    pub fn cache_source_file(
        &self,
        file_name: &str,
        path: &tspath::Path,
        source: &Rc<dyn FileHandle>,
    ) -> Option<Rc<dyn FileHandle>> {
        let file = new_cached_file(file_name, source.shared_content());
        file.borrow().file_base.hash.set(source.hash());
        let (entry, loaded) = self.cache_files.load_or_store(path.clone(), file);
        let entry = entry?;
        if !loaded && path.0.contains("/node_modules/") {
            self.record_realpath_alias(&entry, file_name, path);
        }
        self.reload_entry_if_needed(&entry)
    }

    // Go: project/snapshotfs.go:372 snapshotFSBuilder.getCachedFile (ts#64291: was getDiskFile)
    pub fn get_cached_file(
        &self,
        file_name: &str,
        path: &tspath::Path,
        force_reload: bool,
    ) -> Option<Rc<dyn FileHandle>> {
        let (entry, loaded) = self.cache_files.load_or_store(
            path.clone(),
            Rc::new(RefCell::new(CachedFile {
                file_base: FileBase {
                    file_name: file_name.to_string(),
                    ..FileBase::default()
                },
                needs_reload: true,
                ..CachedFile::default()
            })),
        );
        if let Some(entry) = entry {
            if !loaded && path.0.contains("/node_modules/") {
                self.record_realpath_alias(&entry, file_name, path);
            }
            if force_reload {
                return self.reload_entry(&entry);
            }
            return self.reload_entry_if_needed(&entry);
        }
        None
    }

    // Go: project/snapshotfs.go:389 snapshotFSBuilder.recordRealpathAlias
    // recordRealpathAlias checks if fileName is accessed through a symlink and, if so,
    // records a mapping from the realpath-based key to the symlink-based key.
    // This is only called for files inside node_modules where symlinks are common.
    pub fn record_realpath_alias(
        &self,
        cached_file_entry: &Rc<dirty::SyncMapEntry<tspath::Path, Rc<RefCell<CachedFile>>>>,
        symlink_file_name: &str,
        symlink_path: &tspath::Path,
    ) {
        let realpath = self.fs.realpath(symlink_file_name);
        let realpath_path = (self.to_path)(&realpath);
        if realpath_path != *symlink_path {
            cached_file_entry.change(&mut |file: &Rc<RefCell<CachedFile>>| {
                file.borrow_mut().realpath_path = realpath_path.clone();
            });
            let (entry, _) = self.node_modules_realpath_aliases.load_or_store(
                realpath_path,
                Rc::new(RefCell::new(RealpathAliasSet::default())),
            );
            entry
                .unwrap_or_else(|| crate::core::go_nil_dereference())
                .change(&mut |alias_set: &Rc<RefCell<RealpathAliasSet>>| {
                    alias_set
                        .borrow_mut()
                        .add(symlink_path.clone(), symlink_file_name.to_string());
                });
        }
    }

    // Go: project/snapshotfs.go:403 snapshotFSBuilder.reloadEntry
    pub fn reload_entry(
        &self,
        entry: &Rc<dirty::SyncMapEntry<tspath::Path, Rc<RefCell<CachedFile>>>>,
    ) -> Option<Rc<dyn FileHandle>> {
        let mut file_name = String::new();
        entry.locked(&mut |e: &dyn dirty::Value<Rc<RefCell<CachedFile>>>| {
            if let Some(value) = e.value() {
                file_name = value.borrow().file_base.file_name.clone();
            }
        });
        if file_name.is_empty() {
            return None;
        }
        // Read file outside the lock to avoid blocking other goroutines.
        let (content, ok) = self.fs.read_file(&file_name);
        entry.locked(&mut |e: &dyn dirty::Value<Rc<RefCell<CachedFile>>>| {
            if e.value().is_none() {
                return;
            }
            if ok {
                e.change(&mut |file: &Rc<RefCell<CachedFile>>| {
                    let mut file = file.borrow_mut();
                    file.file_base.content = content.as_str().into();
                    file.file_base.hash.set(xxh3_128(content.as_bytes()));
                    file.needs_reload = false;
                });
            } else {
                e.delete();
            }
        });
        let value = entry.value()?;
        Some(value)
    }

    // Go: project/snapshotfs.go:435 snapshotFSBuilder.reloadEntryIfNeeded
    pub fn reload_entry_if_needed(
        &self,
        entry: &Rc<dirty::SyncMapEntry<tspath::Path, Rc<RefCell<CachedFile>>>>,
    ) -> Option<Rc<dyn FileHandle>> {
        let mut file_name = String::new();
        entry.locked(&mut |e: &dyn dirty::Value<Rc<RefCell<CachedFile>>>| {
            if let Some(value) = e.value() {
                let value = value.borrow();
                if !value.matches_disk_text() {
                    file_name = value.file_base.file_name.clone();
                }
            }
        });
        if !file_name.is_empty() {
            // Read file outside the lock to avoid blocking other goroutines.
            let (content, ok) = self.fs.read_file(&file_name);
            entry.locked(&mut |e: &dyn dirty::Value<Rc<RefCell<CachedFile>>>| {
                match e.value() {
                    None => return, // another goroutine already reloaded it
                    Some(value) if value.borrow().matches_disk_text() => return,
                    Some(_) => {}
                }
                if ok {
                    e.change(&mut |file: &Rc<RefCell<CachedFile>>| {
                        let mut file = file.borrow_mut();
                        file.file_base.content = content.as_str().into();
                        file.file_base.hash.set(xxh3_128(content.as_bytes()));
                        file.needs_reload = false;
                    });
                } else {
                    e.delete();
                }
            });
        }
        let value = entry.value()?;
        Some(value)
    }

    // Go: project/snapshotfs.go:466 snapshotFSBuilder.watchChangesOverlapCache
    // PORT: Go passes the summary by value; here by reference.
    pub fn watch_changes_overlap_cache(
        &self,
        change: &FileChangeSummary,
        previous_open_files: &IndexMap<tspath::Path, Rc<dyn FileHandle>>,
        open_files: &IndexMap<tspath::Path, Rc<dyn FileHandle>>,
    ) -> bool {
        for uri in &change.changed {
            let path = (self.to_path)(&uri.file_name());
            if previous_open_files.contains_key(&path) || open_files.contains_key(&path) {
                return true;
            }
            if let (_, true) = self.cache_files.load(&path) {
                return true;
            }
            if let (_, true) = self.node_modules_realpath_aliases.load(&path) {
                return true;
            }
        }
        for uri in &change.deleted {
            let path = (self.to_path)(&uri.file_name());
            if previous_open_files.contains_key(&path) || open_files.contains_key(&path) {
                return true;
            }
            if let (_, true) = self.cache_files.load(&path) {
                return true;
            }
            if let (_, true) = self.node_modules_realpath_aliases.load(&path) {
                return true;
            }
        }
        false
    }

    // Go: project/snapshotfs.go:494 snapshotFSBuilder.invalidateCache
    pub fn invalidate_cache(&self) {
        self.cache_files.range(&mut |entry: &Rc<
            dirty::SyncMapEntry<tspath::Path, Rc<RefCell<CachedFile>>>,
        >| {
            entry.change(&mut |file: &Rc<RefCell<CachedFile>>| {
                file.borrow_mut().needs_reload = true;
            });
            true
        });
    }

    // Go: project/snapshotfs.go:503 snapshotFSBuilder.invalidateNodeModulesCache
    pub fn invalidate_node_modules_cache(&self) {
        self.cache_files.range(&mut |entry: &Rc<
            dirty::SyncMapEntry<tspath::Path, Rc<RefCell<CachedFile>>>,
        >| {
            if entry.key().0.contains("/node_modules/") {
                entry.change(&mut |file: &Rc<RefCell<CachedFile>>| {
                    file.borrow_mut().needs_reload = true;
                });
            }
            true
        });
    }

    // Go: project/snapshotfs.go:514 snapshotFSBuilder.markDirtyFiles
    // PORT: Go reloads the changed disk files in a work group; the port
    // reloads them one after another (one thread). The result is a set, so
    // the order does not matter.
    pub fn mark_dirty_files(&self, mut change: FileChangeSummary) -> FileChangeSummary {
        if !change.changed.is_empty() {
            let mut filtered_changed: FxHashSet<lsproto::DocumentUri> = FxHashSet::default();
            for uri in &change.changed {
                let path = (self.to_path)(&uri.file_name());
                if let Some(file) = self.fs.get_file_by_path(&uri.file_name(), &path)
                    && file.is_overlay()
                {
                    filtered_changed.insert(uri.clone());
                    continue;
                }
                let (Some(entry), true) = self.cache_files.load(&path) else {
                    filtered_changed.insert(uri.clone());
                    continue;
                };
                if self.reload_entry_if_content_changed(&entry) {
                    filtered_changed.insert(uri.clone());
                }
            }
            change.changed = filtered_changed;
        }
        for uri in &change.deleted {
            let path = (self.to_path)(&uri.file_name());
            if let (Some(entry), true) = self.cache_files.load(&path) {
                self.delete_cache_entry(&entry);
            }
        }
        change
    }

    // Go: project/snapshotfs.go:551 snapshotFSBuilder.reloadEntryIfContentChanged
    // PORT: Go named result `(changed bool)`.
    pub fn reload_entry_if_content_changed(
        &self,
        entry: &Rc<dirty::SyncMapEntry<tspath::Path, Rc<RefCell<CachedFile>>>>,
    ) -> bool {
        let Some(file) = entry.value() else {
            return true;
        };
        let file_name = file.borrow().file_base.file_name.clone();
        let (content, ok) = self.fs.read_file(&file_name);
        let mut changed = true;
        entry.locked(&mut |e: &dyn dirty::Value<Rc<RefCell<CachedFile>>>| {
            let Some(cur) = e.value() else {
                return;
            };
            if !ok {
                e.delete();
                return;
            }
            if *content == *cur.borrow().file_base.content {
                changed = false;
                if !cur.borrow().matches_disk_text() {
                    e.change(&mut |file: &Rc<RefCell<CachedFile>>| {
                        file.borrow_mut().needs_reload = false;
                    });
                }
                return;
            }
            e.change(&mut |file: &Rc<RefCell<CachedFile>>| {
                let mut file = file.borrow_mut();
                file.file_base.content = content.as_str().into();
                file.file_base.hash.set(xxh3_128(content.as_bytes()));
                file.needs_reload = false;
            });
        });
        changed
    }

    // Go: project/snapshotfs.go:626 snapshotFSBuilder.isRelevantFileName
    // isRelevantFileName returns true if the given URI refers to a file that
    // could affect the project: it has a TypeScript-relevant or configured content-mapper extension,
    // is dynamic (e.g. untitled), or is present in the supplied open-file state.
    // PORT: tsgo#4712 adds the content mapper arguments. Go nil
    // `*collections.Set` is `None`.
    pub fn is_relevant_file_name(
        &self,
        uri: &lsproto::DocumentUri,
        content_mapper_extensions: &[String],
        content_mapper_watched_files: Option<&FxHashSet<tspath::Path>>,
        open_files: &IndexMap<tspath::Path, Rc<dyn FileHandle>>,
    ) -> bool {
        let file_name = uri.file_name();
        if let Some(content_mapper_watched_files) = content_mapper_watched_files
            && content_mapper_watched_files.contains(&(self.to_path)(&file_name))
        {
            return true;
        }
        let content_mapper_extensions: Vec<&str> = content_mapper_extensions
            .iter()
            .map(String::as_str)
            .collect();
        if tspath::file_extension_is_one_of(&file_name, &content_mapper_extensions) {
            return true;
        }
        if tspath::is_dynamic_file_name(&file_name) {
            return true;
        }
        let path = (self.to_path)(&file_name);
        if open_files.contains_key(&path) {
            return true;
        }
        // ts#64159: the extension of the file name's base name, with its
        // case (N took the text after the last "." of the path key).
        is_relevant_extension(&tspath::get_any_extension_from_path(&file_name, &[], false))
    }

    // Go: project/snapshotfs.go:657 snapshotFSBuilder.expandAndFilterWatchEvents
    // expandAndFilterWatchEvents expands directory deletion URIs into individual
    // file deletion URIs using the cached directory structure, and filters out
    // watch events for paths that are neither known directories nor have relevant
    // file extensions.
    pub fn expand_and_filter_watch_events(
        &self,
        mut change: FileChangeSummary,
        content_mapper_extensions: &[String],
        content_mapper_watched_files: Option<&FxHashSet<tspath::Path>>,
        previous_open_files: &IndexMap<tspath::Path, Rc<dyn FileHandle>>,
        open_files: &IndexMap<tspath::Path, Rc<dyn FileHandle>>,
    ) -> FileChangeSummary {
        if !change.deleted.is_empty() {
            let mut filtered_deleted: FxHashSet<lsproto::DocumentUri> = FxHashSet::default();
            for uri in &change.deleted {
                let path = (self.to_path)(&uri.file_name());
                let (_, ok) = self.cache_directories.get(&path);
                if ok || has_open_file_within(&path, previous_open_files, open_files) {
                    self.collect_files_recursive(
                        &path,
                        &mut filtered_deleted,
                        previous_open_files,
                        open_files,
                    );
                } else if self.is_relevant_file_name(
                    uri,
                    content_mapper_extensions,
                    content_mapper_watched_files,
                    open_files,
                ) || is_node_modules_path(&path)
                {
                    // node_modules deletions must always be preserved for auto-import registry change handlers.
                    // They won't be in cacheDirectories since the registry doesn't use the snapshotFSBuilder for
                    // its file system, since we don't want to retain files read there.
                    filtered_deleted.insert(uri.clone());
                }
            }
            change.deleted = filtered_deleted;
        }

        if !change.changed.is_empty() {
            let mut filtered_changed: FxHashSet<lsproto::DocumentUri> = FxHashSet::default();
            for uri in &change.changed {
                if self.is_relevant_file_name(
                    uri,
                    content_mapper_extensions,
                    content_mapper_watched_files,
                    open_files,
                ) {
                    filtered_changed.insert(uri.clone());
                }
            }
            change.changed = filtered_changed;
        }

        // We can't filter created events because any created path could be a directory symlink
        // that includes relevant files. configFileRegistryBuilder will do check if these paths
        // are directories if they fall within a config's wildcard directories.

        change
    }

    // Go: project/snapshotfs.go:715 snapshotFSBuilder.collectFilesRecursive
    // collectFilesRecursive recursively collects all cached file URIs under the
    // given directory path using the cacheDirectories and cacheFiles maps.
    pub fn collect_files_recursive(
        &self,
        dir_path: &tspath::Path,
        files: &mut FxHashSet<lsproto::DocumentUri>,
        previous_open_files: &IndexMap<tspath::Path, Rc<dyn FileHandle>>,
        open_files: &IndexMap<tspath::Path, Rc<dyn FileHandle>>,
    ) {
        for (path, file) in open_files {
            if dir_path.contains_path(path) {
                files.insert(lsconv::file_name_to_document_uri(&file.file_name()));
            }
        }
        for (path, file) in previous_open_files {
            if dir_path.contains_path(path) {
                files.insert(lsconv::file_name_to_document_uri(&file.file_name()));
            }
        }
        let (dir_entry, ok) = self.cache_directories.get(dir_path);
        if !ok {
            return;
        }
        // PORT: the child paths are copied out before the recursion. Go
        // ranges over a nil map when the entry has no value.
        let child_paths: Vec<tspath::Path> = match dir_entry.and_then(|dir_entry| dir_entry.value())
        {
            Some(dir) => dir.borrow().keys().cloned().collect(),
            None => Vec::new(),
        };
        for child_path in &child_paths {
            if let (Some(entry), true) = self.cache_files.load(child_path) {
                if let Some(file) = entry.value() {
                    files.insert(lsconv::file_name_to_document_uri(
                        &file.borrow().file_base.file_name(),
                    ));
                }
            }
            self.collect_files_recursive(child_path, files, previous_open_files, open_files);
        }
    }

    // Go: project/snapshotfs.go:740 snapshotFSBuilder.convertOpenAndCloseToChanges
    pub fn convert_open_and_close_to_changes(
        &self,
        mut change: FileChangeSummary,
        previous_open_files: &IndexMap<tspath::Path, Rc<dyn FileHandle>>,
        open_files: &IndexMap<tspath::Path, Rc<dyn FileHandle>>,
    ) -> FileChangeSummary {
        if !change.opened.0.is_empty() && !tspath::is_dynamic_file_name(&change.opened.file_name())
        {
            let path = (self.to_path)(&change.opened.file_name());
            let (entry, ok) = self.cache_files.load(&path);
            let original = entry.as_ref().and_then(|entry| entry.original());
            if !ok || original.is_none() {
                change.created.insert(change.opened.clone());
            } else if let Some(open_file) = open_files.get(&path) {
                // The file already exists in the program, but the open-file content from
                // didOpen may differ from what was originally read from the source (e.g. the
                // editor normalizes line endings, or the source file changed since the
                // project was loaded). Mark it as Changed so the project rebuilds.
                if let Some(cached_file) = original {
                    if open_file.hash() != cached_file.borrow().file_base.hash() {
                        change.changed.insert(change.opened.clone());
                    }
                }
                self.delete_cache_entry(entry.as_ref().expect("loaded above"));
            }
        }
        for uri in &change.closed {
            let file_name = uri.file_name();
            if tspath::is_dynamic_file_name(&file_name) {
                continue;
            }
            let path = (self.to_path)(&file_name);
            // We may have ignored watcher events while the file was open, so force a reload.
            if let Some(fh) = self.get_cached_file(&file_name, &path, true /*forceReload*/) {
                if let Some(previous_open_file) = previous_open_files.get(&path)
                    && fh.hash() != previous_open_file.hash()
                {
                    change.changed.insert(uri.clone());
                }
                continue;
            }
            change.deleted.insert(uri.clone());
        }
        change
    }
}

// Go: project/snapshotfs.go:65 `_ FileSource = (*snapshotFSBuilder)(nil)`
impl FileHandleSource for SnapshotFSBuilder {
    // Go: project/snapshotfs.go:311 snapshotFSBuilder.GetFile
    fn get_file(&self, file_name: &str) -> Option<Rc<dyn FileHandle>> {
        let path = (self.to_path)(file_name);
        self.get_file_by_path(file_name, &path)
    }

    // Go: project/snapshotfs.go:336 snapshotFSBuilder.GetFileByPath
    fn get_file_by_path(&self, file_name: &str, path: &tspath::Path) -> Option<Rc<dyn FileHandle>> {
        if let (Some(entry), true) = self.cache_files.load(path) {
            return self.reload_entry_if_needed(&entry);
        }
        let file = self.fs.get_file_by_path(file_name, path)?;
        if file.is_overlay() {
            return Some(file);
        }
        self.cache_source_file(file_name, path, &file)
    }
}

impl FileSource for SnapshotFSBuilder {
    // Go: project/snapshotfs.go:215 snapshotFSBuilder.FS
    fn fs(&self) -> Rc<dyn vfs::Fs> {
        self.fs.clone()
    }

    // Go: project/snapshotfs.go:323 snapshotFSBuilder.FileExists
    fn file_exists(&self, file_name: &str, path: &tspath::Path) -> bool {
        if let (Some(entry), true) = self.cache_files.load(path) {
            let val = entry.value();
            if val.is_none() {
                return false;
            }
            // Entry may be dirty - reload to check current state in the source filesystem.
            return self.reload_entry_if_needed(&entry).is_some();
        }
        // Path never loaded into cacheFiles - use cached stat (no file read).
        self.fs.file_exists(file_name)
    }

    // Go: project/snapshotfs.go:347 snapshotFSBuilder.GetAccessibleEntries
    fn get_accessible_entries(&self, path: &str) -> vfs::Entries {
        let lower_entries = self.fs.get_accessible_entries(path);
        let (directory, ok) = self.cache_directories.get(&(self.to_path)(path));
        if !ok {
            return lower_entries;
        }
        let directory = directory
            .and_then(|directory| directory.value())
            .map(|directory| directory.borrow().clone())
            .unwrap_or_default();
        merge_cached_directory_entries(
            lower_entries,
            &directory,
            &|key: &tspath::Path, child_name: &str| {
                let (entry, cached) = self.cache_files.load(key);
                cached && entry.is_some_and(|entry| entry.value().is_some())
                    || self
                        .fs
                        .file_exists(&tspath::combine_paths(path, &[child_name]))
            },
            self.fs.use_case_sensitive_file_names(),
        )
    }
}

/// The state of a `cache_files` entry that `FileSource::file_exists`
/// reads (`SnapshotFSBuilder::cached_file_state`).
// PORT: not in Go (resolve ahead, compiler/resolve_ahead.rs).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CachedFileState {
    /// No entry: `file_exists` asks the layered file system.
    Absent,
    /// A live entry that needs no reload: `file_exists` is true.
    Live,
    /// An entry with no value (a deleted file): `file_exists` is false.
    NoValue,
    /// An entry that `file_exists` would reload first (a read).
    NeedsReload,
}

// PORT: not in Go. What resolve ahead needs to check a worker's answer
// on the snapshot file system (project/compilerhost.rs).
impl SnapshotFSBuilder {
    /// The state of the `cache_files` entry of `path`, as
    /// `FileSource::file_exists` reads it, with no side effect (no reload,
    /// no entry made for the dirty map).
    pub fn cached_file_state(&self, path: &tspath::Path) -> CachedFileState {
        match self.cache_file_value(path) {
            None => CachedFileState::Absent,
            Some(None) => CachedFileState::NoValue,
            Some(Some(file)) if file.borrow().matches_disk_text() => CachedFileState::Live,
            Some(Some(_)) => CachedFileState::NeedsReload,
        }
    }

    /// The `cache_files` entry of `path` as `dirty::SyncMap::load` reads
    /// it, with no side effect: `None` for no entry, `Some(None)` for an
    /// entry with no value.
    fn cache_file_value(&self, path: &tspath::Path) -> Option<Option<Rc<RefCell<CachedFile>>>> {
        let dirty = self.cache_files.dirty.borrow();
        match dirty.get(path) {
            // `dirty::SyncMap::load` gives no entry for a deleted one.
            Some(entry) if entry.map_entry.borrow().delete => None,
            Some(entry) => Some(entry.value()),
            None => self
                .cache_files
                .base
                .get(path)
                .map(|file| Some(file.clone())),
        }
    }

    /// The hash and the script kind (Go `FileHandle.Kind`) of the file that
    /// `get_file_by_path` gives for `path` now, when they are known with no
    /// read and no side effect: a cached file that needs no reload, else an
    /// open file. `None` when they are not known.
    // PORT: not in Go (parse workers, project/compilerhost.rs
    // `cached_source_file_refs`).
    pub fn known_file(&self, path: &tspath::Path) -> Option<(u128, ScriptKind)> {
        match self.cache_file_value(path) {
            Some(Some(file)) => {
                let file = file.borrow();
                file.matches_disk_text()
                    .then(|| (file.file_base.hash(), file.kind()))
            }
            Some(None) => None,
            None => {
                let layered: &dyn vfs::Fs = &*self.cached_layered()?.layered;
                let overlays = as_overlay_fs(layered)?.overlays.borrow();
                let overlay = overlays.get(path)?;
                Some((overlay.hash(), overlay.kind))
            }
        }
    }

    /// The cache of this snapshot's file system lookups (Go
    /// `cachedLayeredFileSystem`'s `*cachedvfs.FS`), over the layer that
    /// takes the answers of the resolve-ahead jobs (`ahead_lookup_layer`).
    pub fn cached_fs(&self) -> Option<Rc<vfs::CachedFs>> {
        Some(self.cached_layered()?.fs.clone())
    }

    fn cached_layered(&self) -> Option<&CachedLayeredFileSystem> {
        vfs::Fs::as_any(&*self.fs)?.downcast_ref::<CachedLayeredFileSystem>()
    }

    /// When the layered file system is the overlay file system over the
    /// OS file system of this thread (`bundled::is_wrapped_os_fs`), as in
    /// the language server: the paths of its open files and of the
    /// directories that have open files in them. Else `None`.
    pub fn open_files_over_os(&self) -> Option<(FxHashSet<tspath::Path>, FxHashSet<tspath::Path>)> {
        let cached = self.cached_layered()?;
        let layered: &dyn vfs::Fs = &*cached.layered;
        let overlay = as_overlay_fs(layered)?;
        if !crate::frontend::bundled::is_wrapped_os_fs(&overlay.host) {
            return None;
        }
        let files = overlay.overlays.borrow().keys().cloned().collect();
        let directories = overlay
            .overlay_directories
            .borrow()
            .keys()
            .cloned()
            .collect();
        Some((files, directories))
    }
}

// Go: project/snapshotfs.go:645 isRelevantExtension
// isRelevantExtension returns true if the given extension is a known TypeScript
// or JavaScript extension that can affect the project.
pub fn is_relevant_extension(ext: &str) -> bool {
    matches!(
        ext,
        ".js" | ".jsx" | ".mjs" | ".cjs" | ".ts" | ".tsx" | ".mts" | ".cts" | ".json"
    )
}

// Go: project/snapshotfs.go:694 isNodeModulesPath
// isNodeModulesPath reports whether path is a node_modules directory itself or
// lives inside one. Used to preserve node_modules watch deletions, whose package
// files are read transiently and therefore never tracked in cacheDirectories.
pub fn is_node_modules_path(path: &tspath::Path) -> bool {
    let s = path.as_str();
    s.ends_with("/node_modules") || s.contains("/node_modules/")
}

// Go: project/snapshotfs.go:699 hasOpenFileWithin (ts#64291)
pub fn has_open_file_within(
    path: &tspath::Path,
    previous_open_files: &IndexMap<tspath::Path, Rc<dyn FileHandle>>,
    open_files: &IndexMap<tspath::Path, Rc<dyn FileHandle>>,
) -> bool {
    for open_file_path in open_files.keys() {
        if path.contains_path(open_file_path) {
            return true;
        }
    }
    for open_file_path in previous_open_files.keys() {
        if path.contains_path(open_file_path) {
            return true;
        }
    }
    false
}

/// PORT: Go `*collections.SyncMap[tspath.Path, string]` of
/// `sourceFS.seenFiles` (ts#64544): each seen path with the file name that
/// it was last seen as.
pub type SeenFiles = Rc<RefCell<FxHashMap<tspath::Path, String>>>;

// Go: project/snapshotfs.go:775 sourceFS
// sourceFS is a vfs.FS that sources files from a FileSource and tracks seen files.
// PORT: Go `*sourceFS` is shared (`Rc<SourceFS>`, also as `Rc<dyn vfs::Fs>`).
// Go writes `tracking`, `seenFiles` and `source` after sharing, so they are
// `Cell` / `RefCell`.
pub struct SourceFS {
    pub tracking: Cell<bool>,
    pub to_path: Rc<dyn Fn(&str) -> tspath::Path>,
    pub missing_directories: Option<Rc<RefCell<FxHashSet<tspath::Path>>>>,
    pub seen_files: RefCell<Option<SeenFiles>>,
    pub source: RefCell<Rc<dyn FileSource>>,
}

// Go: project/snapshotfs.go:783 newSourceFS
pub fn new_source_fs(
    tracking: bool,
    source: Rc<dyn FileSource>,
    to_path: Rc<dyn Fn(&str) -> tspath::Path>,
) -> Rc<SourceFS> {
    let mut fs = SourceFS {
        tracking: Cell::new(tracking),
        to_path,
        missing_directories: None,
        seen_files: RefCell::new(None),
        source: RefCell::new(source),
    };
    if tracking {
        fs.seen_files = RefCell::new(Some(Rc::new(RefCell::new(FxHashMap::default()))));
        fs.missing_directories = Some(Rc::new(RefCell::new(FxHashSet::default())));
    }
    Rc::new(fs)
}

impl SourceFS {
    /// Go `fs.source` (read). The handle is copied so no borrow is held
    /// while the source runs.
    fn source(&self) -> Rc<dyn FileSource> {
        self.source.borrow().clone()
    }

    // Go: project/snapshotfs.go:798 sourceFS.DisableTracking
    pub fn disable_tracking(&self) {
        self.tracking.set(false);
    }

    /// Runs `f` with no tracking, then sets the tracking back to what it
    /// was, also when `f` panics (a caller that catches the panic keeps the
    /// host's tracking). `CompilerHost::without_fs_tracking` uses it.
    // PORT: not in Go (H3: Go makes no tracked call after `freeze`, and the
    // port builds the checkers' symlink cache copy before it).
    pub fn without_tracking<R>(&self, f: impl FnOnce() -> R) -> R {
        struct Restore<'a>(&'a Cell<bool>, bool);
        impl Drop for Restore<'_> {
            fn drop(&mut self) {
                self.0.set(self.1);
            }
        }
        let _restore = Restore(&self.tracking, self.tracking.replace(false));
        f()
    }

    // Go: project/snapshotfs.go:802 sourceFS.Track
    pub fn track(&self, file_name: &str) {
        if !self.tracking.get() {
            return;
        }
        let path = (self.to_path)(file_name);
        self.track_path(file_name, &path);
    }

    // Go: project/snapshotfs.go:809 sourceFS.SeenFile
    pub fn seen_file(&self, path: &tspath::Path) -> bool {
        let seen_files = self.seen_files.borrow();
        let Some(seen_files) = seen_files.as_ref() else {
            return false;
        };
        let seen = seen_files.borrow().contains_key(path);
        seen
    }

    // Go: project/snapshotfs.go:817 sourceFS.SeenFileOrMissingParentDirectory
    pub fn seen_file_or_missing_parent_directory(&self, path: &tspath::Path) -> bool {
        if let Some(seen_files) = self.seen_files.borrow().as_ref() {
            if seen_files.borrow().contains_key(path) {
                return true;
            }
        }
        if let Some(missing_directories) = &self.missing_directories {
            let missing_directories = missing_directories.borrow();
            if !missing_directories.is_empty() {
                let mut path = path.clone();
                loop {
                    if missing_directories.contains(&path) {
                        return true;
                    }

                    let parent = path.get_directory_path();
                    if parent == path {
                        break;
                    }
                    path = parent;
                }
            }
        }
        false
    }

    // Go: project/snapshotfs.go:839 sourceFS.GetFile
    pub fn get_file(&self, file_name: &str) -> Option<Rc<dyn FileHandle>> {
        self.track(file_name);
        self.source().get_file(file_name)
    }

    // Go: project/snapshotfs.go:844 sourceFS.GetFileByPath
    pub fn get_file_by_path(
        &self,
        file_name: &str,
        path: &tspath::Path,
    ) -> Option<Rc<dyn FileHandle>> {
        self.track(file_name);
        self.source().get_file_by_path(file_name, path)
    }

    /// Drops the file source and the tracked sets when the host of this file
    /// system is released (`compiler::CompilerHost::release`). Only the case
    /// sensitivity stays, so a file name lookup on a released program still
    /// works (Go `Program.GetSourceFile` makes a path first). Any file
    /// access panics after this. `seen_files` can be shared with the host of
    /// a program cloned from this host's program; only this reference goes.
    // PORT: not in Go. Go's GC frees the `sourceFS` with its host.
    pub fn release(&self) {
        self.tracking.set(false);
        let released: Rc<dyn FileSource> = Rc::new(ReleasedFileSource {
            use_case_sensitive_file_names: self.source().use_case_sensitive_file_names(),
        });
        // PORT: the old values drop after the borrows end.
        let source = std::mem::replace(&mut *self.source.borrow_mut(), released);
        let seen_files = self.seen_files.borrow_mut().take();
        let missing_directories = self
            .missing_directories
            .as_ref()
            .map(|missing| std::mem::take(&mut *missing.borrow_mut()));
        drop(source);
        drop(seen_files);
        drop(missing_directories);
    }
}

// PORT: not in Go. The replay of a resolve-ahead answer's calls
// (project/compilerhost.rs): the side effects of `vfs::Fs::file_exists`
// and `vfs::Fs::directory_exists` with the path made already.
impl SourceFS {
    /// `track` of `file_name`, whose path is `path`.
    /// ts#64544: Go `seenFiles.Store(path, fileName)` keeps the last name
    /// that the path was seen as.
    pub fn track_path(&self, file_name: &str, path: &tspath::Path) {
        if !self.tracking.get() {
            return;
        }
        let seen_files = self.seen_files.borrow();
        let mut seen_files = seen_files
            .as_ref()
            .unwrap_or_else(|| crate::core::go_nil_dereference())
            .borrow_mut();
        match seen_files.get_mut(path) {
            Some(seen_name) => {
                if seen_name != file_name {
                    *seen_name = file_name.to_string();
                }
            }
            None => {
                seen_files.insert(path.clone(), file_name.to_string());
            }
        }
    }

    /// What `directory_exists` notes when it gives false for a name whose
    /// path is `path`.
    pub fn note_missing_directory(&self, path: &tspath::Path) {
        if !self.tracking.get() {
            return;
        }
        let mut missing_directories = self
            .missing_directories
            .as_ref()
            .unwrap_or_else(|| crate::core::go_nil_dereference())
            .borrow_mut();
        if !missing_directories.contains(path) {
            missing_directories.insert(path.clone());
        }
    }
}

/// The file source of a released `SourceFS` (`SourceFS::release`). It keeps
/// only the case sensitivity; every file access panics.
// PORT: not in Go.
struct ReleasedFileSource {
    use_case_sensitive_file_names: bool,
}

fn released_file_source_used() -> ! {
    panic!("the file system of a released program's compiler host was used");
}

impl FileHandleSource for ReleasedFileSource {
    fn get_file(&self, _file_name: &str) -> Option<Rc<dyn FileHandle>> {
        released_file_source_used()
    }

    fn get_file_by_path(
        &self,
        _file_name: &str,
        _path: &tspath::Path,
    ) -> Option<Rc<dyn FileHandle>> {
        released_file_source_used()
    }
}

impl FileSource for ReleasedFileSource {
    fn fs(&self) -> Rc<dyn vfs::Fs> {
        released_file_source_used()
    }

    fn file_exists(&self, _file_name: &str, _path: &tspath::Path) -> bool {
        released_file_source_used()
    }

    fn get_accessible_entries(&self, _path: &str) -> vfs::Entries {
        released_file_source_used()
    }

    fn use_case_sensitive_file_names(&self) -> bool {
        self.use_case_sensitive_file_names
    }
}

// Go: project/snapshotfs.go:664 `var _ vfs.FS = (*sourceFS)(nil)`
impl vfs::Fs for SourceFS {
    // Go: project/snapshotfs.go:879 sourceFS.UseCaseSensitiveFileNames (at 673a5f17d713;
    // ts#64159 renames it CaseSensitivity, snapshotfs.go:888)
    // UseCaseSensitiveFileNames implements vfs.FS.
    fn use_case_sensitive_file_names(&self) -> bool {
        // PORT: through the source, so a released source can answer it
        // (`SourceFS::release`).
        self.source().use_case_sensitive_file_names()
    }

    // Go: project/snapshotfs.go:859 sourceFS.FileExists
    // FileExists implements vfs.FS.
    fn file_exists(&self, path: &str) -> bool {
        // PORT: Go makes the path twice (`Track` and the source call); the
        // port makes it once, with the same `to_path`.
        let file_path = (self.to_path)(path);
        self.track_path(path, &file_path);
        self.source().file_exists(path, &file_path)
    }

    // Go: project/snapshotfs.go:870 sourceFS.ReadFile
    // ReadFile implements vfs.FS.
    fn read_file(&self, path: &str) -> (String, bool) {
        if let Some(fh) = self.get_file(path) {
            return (fh.content(), true);
        }
        (String::new(), false)
    }

    // Go: project/snapshotfs.go:893 sourceFS.WriteFile
    // WriteFile implements vfs.FS.
    fn write_file(&self, _path: &str, _data: &str) -> Result<(), vfs::FsError> {
        crate::core::go_panic("unimplemented".to_string());
    }

    // Go: project/snapshotfs.go:898 sourceFS.AppendFile
    // AppendFile implements vfs.FS.
    fn append_file(&self, _path: &str, _data: &str) -> Result<(), vfs::FsError> {
        crate::core::go_panic("unimplemented".to_string());
    }

    // Go: project/snapshotfs.go:903 sourceFS.Remove
    // Remove implements vfs.FS.
    fn remove(&self, _path: &str) -> Result<(), vfs::FsError> {
        crate::core::go_panic("unimplemented".to_string());
    }

    // Go: project/snapshotfs.go:908 sourceFS.Chtimes
    // Chtimes implements vfs.FS.
    fn chtimes(
        &self,
        _path: &str,
        _a_time: Option<SystemTime>,
        _m_time: Option<SystemTime>,
    ) -> Result<(), vfs::FsError> {
        crate::core::go_panic("unimplemented".to_string());
    }

    // Go: project/snapshotfs.go:850 sourceFS.DirectoryExists
    // DirectoryExists implements vfs.FS.
    fn directory_exists(&self, path: &str) -> bool {
        let exists = self.source().fs().directory_exists(path);
        if !exists && self.tracking.get() {
            self.missing_directories
                .as_ref()
                .unwrap_or_else(|| crate::core::go_nil_dereference())
                .borrow_mut()
                .insert((self.to_path)(path));
        }
        exists
    }

    // Go: project/snapshotfs.go:865 sourceFS.GetAccessibleEntries
    // GetAccessibleEntries implements vfs.FS.
    fn get_accessible_entries(&self, path: &str) -> vfs::Entries {
        self.source().get_accessible_entries(path)
    }

    // Go: project/snapshotfs.go:883 sourceFS.Stat
    // Stat implements vfs.FS.
    fn stat(&self, path: &str) -> Option<vfs::FileInfo> {
        self.source().fs().stat(path)
    }

    // Go: project/snapshotfs.go:878 sourceFS.Realpath
    // Realpath implements vfs.FS.
    fn realpath(&self, path: &str) -> String {
        self.source().fs().realpath(path)
    }
}
