//! Port of Go `internal/api/requestfilesystem/requestfilesystem.go`
//! (ts#64115, ts#64277, ts#64291, ts#64391).
//!
//! PORT: Go has the exported request type `RequestFileSystem` and the
//! unexported file system `requestFileSystem`. The file system is
//! `RequestFileSystemImpl` here. Its methods have Go value receivers, so it
//! is `Clone` (the fields are shared handles).
//!
//! PORT: Go type assertions on a `vfs.FS` (`fs.(*requestFileSystem)`,
//! `fs.(project.LayeredFileSystem)`, `fs.(project.FileHandleSource)`) use
//! `vfs::Fs::as_any` (a downcast to the concrete type) and the
//! `project::FsLayer` queries (the server lane design).

use crate::prelude::*;

use super::filechanges::add_file_changes;
use super::pathtree::{
    RequestDirectory, RequestEntry, RequestFallback, RequestFile, RequestInfo, RequestPathNode,
    RequestPathNodeRef, RequestSymlinkEntry, compose_request_paths, contains_file_ancestor, ensure,
    first_symlink, lookup, request_path_contains, walk_symlinks,
};
use crate::frontend::json::{JsonDecoder, JsonError, UnmarshalerFrom, json_unmarshal_decode};
use crate::frontend::json_ext::{self, go_type_name, unmarshal_struct_fields};
use crate::frontend::tspath;
use crate::frontend::vfs::{self, Fs as _};
use crate::gostd::{GoError, errors, strconv};
use crate::project::{self, FileHandle as _};
use std::borrow::Cow;
use std::time::SystemTime;

// Go: api/requestfilesystem/requestfilesystem.go Kind
// Kind controls how a request filesystem is used.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Kind(pub Cow<'static, str>);

impl Kind {
    // KindFull makes the supplied filesystem canonical and total.
    pub const FULL: Kind = Kind(Cow::Borrowed("full"));
    // KindLayer checks the supplied filesystem before falling back to the host.
    pub const LAYER: Kind = Kind(Cow::Borrowed("layer"));
}

impl std::fmt::Display for Kind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl UnmarshalerFrom for Kind {
    fn unmarshal_json_from(&mut self, dec: &mut JsonDecoder<'_>) -> Result<(), JsonError> {
        let mut s = String::new();
        json_ext::unmarshal_string_as(dec, &mut s, &go_type_name::<Self>())?;
        self.0 = Cow::Owned(s);
        Ok(())
    }
}

// Go: api/requestfilesystem/requestfilesystem.go RequestDirectoryEntries
// RequestDirectoryEntries is a cached directory listing. Entry names are
// relative to the directory, matching vfs.GetAccessibleEntries.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RequestDirectoryEntries {
    pub files: Vec<String>,
    pub directories: Vec<String>,
}

// Go: api/requestfilesystem/requestfilesystem.go RequestSymlink
// RequestSymlink describes a symbolic link in a request filesystem.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RequestSymlink {
    // Target is resolved relative to the directory containing the link, matching
    // native symbolic-link semantics.
    pub target: String,
    // Host routes the target through the host filesystem. This is the only way a
    // full filesystem can access paths not supplied in the request filesystem.
    pub host: bool,
}

// Go: api/requestfilesystem/requestfilesystem.go RequestFileSystem
// RequestFileSystem supplies file contents and, optionally, directory listings
// for a request that creates a snapshot.
// PORT: Go maps are `FxHashMap`; the code that walks them walks the names in
// sorted order (Go order is random).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RequestFileSystem {
    pub kind: Kind,
    // Files maps file names to their complete contents.
    pub files: FxHashMap<String, String>,
    // Directories maps directory names to complete listing results. Directory
    // structure implied by Files is derived when a listing is omitted.
    pub directories: FxHashMap<String, RequestDirectoryEntries>,
    // Symlinks maps link paths to targets in this filesystem or the host filesystem.
    pub symlinks: FxHashMap<String, RequestSymlink>,
    // RemovedPaths lists files or directory trees that must be treated as missing
    // even when present in an underlying snapshot or host filesystem.
    pub removed_paths: Vec<String>,
}

// PORT: Go v2 default struct unmarshal of the tagged request structs. The api
// only decodes them, so no marshalers are written.
impl UnmarshalerFrom for RequestDirectoryEntries {
    fn unmarshal_json_from(&mut self, dec: &mut JsonDecoder<'_>) -> Result<(), JsonError> {
        let is_object = unmarshal_struct_fields(
            dec,
            "requestfilesystem.RequestDirectoryEntries",
            |name, dec| {
                match name {
                    "files" => json_unmarshal_decode(dec, &mut self.files)?,
                    "directories" => json_unmarshal_decode(dec, &mut self.directories)?,
                    _ => return Ok(false),
                }
                Ok(true)
            },
        )?;
        if !is_object {
            *self = RequestDirectoryEntries::default();
        }
        Ok(())
    }
}

impl UnmarshalerFrom for RequestSymlink {
    fn unmarshal_json_from(&mut self, dec: &mut JsonDecoder<'_>) -> Result<(), JsonError> {
        let is_object =
            unmarshal_struct_fields(dec, "requestfilesystem.RequestSymlink", |name, dec| {
                match name {
                    "target" => json_unmarshal_decode(dec, &mut self.target)?,
                    "host" => json_unmarshal_decode(dec, &mut self.host)?,
                    _ => return Ok(false),
                }
                Ok(true)
            })?;
        if !is_object {
            *self = RequestSymlink::default();
        }
        Ok(())
    }
}

impl UnmarshalerFrom for RequestFileSystem {
    fn unmarshal_json_from(&mut self, dec: &mut JsonDecoder<'_>) -> Result<(), JsonError> {
        let is_object =
            unmarshal_struct_fields(dec, "requestfilesystem.RequestFileSystem", |name, dec| {
                match name {
                    "kind" => json_unmarshal_decode(dec, &mut self.kind)?,
                    "files" => json_unmarshal_decode(dec, &mut self.files)?,
                    "directories" => json_unmarshal_decode(dec, &mut self.directories)?,
                    "symlinks" => json_unmarshal_decode(dec, &mut self.symlinks)?,
                    "removedPaths" => json_unmarshal_decode(dec, &mut self.removed_paths)?,
                    _ => return Ok(false),
                }
                Ok(true)
            })?;
        if !is_object {
            *self = RequestFileSystem::default();
        }
        Ok(())
    }
}

/// Go map keys in sorted order (see `RequestFileSystem`).
fn sorted_keys<V>(map: &FxHashMap<String, V>) -> Vec<&String> {
    let mut keys: Vec<&String> = map.keys().collect();
    keys.sort();
    keys
}

// Go: api/requestfilesystem/requestfilesystem.go requestFileSystem
// requestFileSystem is either a full filesystem or a layer over the session
// host filesystem. Its base is always the host, which may be a callback filesystem;
// inherited request entries are compacted into paths.
#[derive(Clone)]
pub struct RequestFileSystemImpl {
    pub kind: Kind,
    pub base: Rc<dyn vfs::Fs>,
    pub current_directory: String,
    pub use_case_sensitive_names: bool,
    pub paths: RequestPathNodeRef,
}

// Go: api/requestfilesystem/requestfilesystem.go resolvedRequestPath
#[derive(Clone, Debug, Default)]
struct ResolvedRequestPath {
    path: String,
    followed_symlink: bool,
    host: bool,
    ok: bool,
}

// Go: api/requestfilesystem/requestfilesystem.go requestPathLookup
#[derive(Clone, Default)]
struct RequestPathLookup {
    path: String,
    info: Option<RequestInfo>,
    file_system: Option<Rc<dyn vfs::Fs>>,
    followed_symlink: bool,
    ok: bool,
}

// Go: api/requestfilesystem/requestfilesystem.go getRequestFileSystem
// PORT: Go returns the `*requestFileSystem` pointer; the port returns a copy
// (the fields are shared handles).
pub fn get_request_file_system(file_system: &dyn vfs::Fs) -> Option<RequestFileSystemImpl> {
    file_system
        .as_any()?
        .downcast_ref::<RequestFileSystemImpl>()
        .cloned()
}

// Go: api/requestfilesystem/requestfilesystem.go NewForUpdate
// NewForUpdate creates a request filesystem for a snapshot update. Layers over
// request filesystems are compacted eagerly so the result does not retain its
// base snapshot's filesystem.
// PORT: Go nil `params` returns `base`; that is `None`.
pub fn new_for_update(
    params: Option<&RequestFileSystem>,
    base: Rc<dyn vfs::Fs>,
    current_directory: &str,
    file_changes: &mut project::FileChangeSummary,
) -> Result<Rc<dyn vfs::Fs>, GoError> {
    let Some(params) = params else {
        return Ok(base);
    };
    let mut base_file_system = base;
    if params.kind == Kind::FULL
        && let Some(request_base) = get_request_file_system(&*base_file_system)
    {
        base_file_system = request_base.base;
    }
    let mut file_system =
        new_request_file_system_worker(params, base_file_system.clone(), current_directory)?;
    if let Some(base_request_file_system) = get_request_file_system(&*base_file_system) {
        file_system = file_system.apply_to(&base_request_file_system);
    }
    if params.kind == Kind::LAYER {
        add_file_changes(
            file_changes,
            params,
            &*base_file_system,
            &file_system,
            current_directory,
        );
    }
    Ok(Rc::new(file_system))
}

// Go: api/requestfilesystem/requestfilesystem.go HasFullFileSystem
// HasFullFileSystem reports whether fileSystem contains a complete request filesystem.
// PORT: Go nil `fileSystem` is `None`.
pub fn has_full_file_system(file_system: Option<&dyn vfs::Fs>) -> bool {
    file_system
        .and_then(get_request_file_system)
        .is_some_and(|request_file_system| request_file_system.kind == Kind::FULL)
}

// Go: api/requestfilesystem/requestfilesystem.go newRequestFileSystemWorker
pub fn new_request_file_system_worker(
    params: &RequestFileSystem,
    base: Rc<dyn vfs::Fs>,
    current_directory: &str,
) -> Result<RequestFileSystemImpl, GoError> {
    if params.kind != Kind::FULL && params.kind != Kind::LAYER {
        return Err(errors::new(format!(
            "unknown request filesystem kind {}",
            strconv::quote(&params.kind.0)
        )));
    }

    let use_case_sensitive_names = base.use_case_sensitive_file_names();
    let mut result = RequestFileSystemImpl {
        kind: params.kind.clone(),
        base,
        current_directory: current_directory.to_string(),
        use_case_sensitive_names,
        paths: Rc::new(RefCell::new(RequestPathNode::default())),
    };
    result.register_directory(current_directory);
    for file_name in sorted_keys(&params.files) {
        let content = &params.files[file_name];
        let absolute_file_name = result.to_absolute_path(file_name);
        let path = result.to_path(&absolute_file_name);
        let node = ensure(&result.paths, &path);
        if let Some(RequestEntry::File(existing)) = &node.borrow().entry {
            return Err(errors::new(format!(
                "duplicate request filesystem file path {} and {}",
                strconv::quote(&existing.file_name),
                strconv::quote(&absolute_file_name)
            )));
        }
        node.borrow_mut().entry = Some(RequestEntry::File(Rc::new(RequestFile {
            file_name: absolute_file_name.clone(),
            content: content.clone(),
        })));
        result.register_directory(&tspath::get_directory_path(&absolute_file_name));
    }
    let mut seen_directories: FxHashSet<tspath::Path> =
        FxHashSet::with_capacity_and_hasher(params.directories.len(), Default::default());
    let mut listed_directories: Vec<String> = Vec::new();
    for directory_name in sorted_keys(&params.directories) {
        let entries = &params.directories[directory_name];
        let absolute_directory_name = result.to_absolute_path(directory_name);
        let path = result.to_path(&absolute_directory_name);
        let node = ensure(&result.paths, &path);
        if seen_directories.contains(&path) {
            return Err(errors::new(format!(
                "duplicate request filesystem directory path {}",
                strconv::quote(&absolute_directory_name)
            )));
        }
        seen_directories.insert(path);
        let is_file = matches!(node.borrow().entry, Some(RequestEntry::File(_)));
        if !is_file {
            node.borrow_mut().entry = Some(RequestEntry::Directory(Rc::new(RequestDirectory {
                directory_name: absolute_directory_name.clone(),
                listing: Some(vfs::Entries {
                    files: entries.files.clone(),
                    directories: entries.directories.clone(),
                    symlinks: None,
                }),
            })));
        }
        result.register_directory(&tspath::get_directory_path(&absolute_directory_name));
        for child in &entries.directories {
            listed_directories.push(tspath::combine_paths(&absolute_directory_name, &[child]));
        }
    }
    for link_name in sorted_keys(&params.symlinks) {
        let directory = tspath::get_directory_path(&result.to_absolute_path(link_name));
        result.register_directory(&directory);
    }
    let mut seen_symlinks: FxHashMap<tspath::Path, String> =
        FxHashMap::with_capacity_and_hasher(params.symlinks.len(), Default::default());
    for link_name in sorted_keys(&params.symlinks) {
        let symlink = &params.symlinks[link_name];
        let absolute_link_name = result.to_absolute_path(link_name);
        let path = result.to_path(&absolute_link_name);
        let node = ensure(&result.paths, &path);
        if let Some(existing) = seen_symlinks.get(&path) {
            return Err(errors::new(format!(
                "duplicate request filesystem symlink path {} and {}",
                strconv::quote(existing),
                strconv::quote(&absolute_link_name)
            )));
        }
        seen_symlinks.insert(path, absolute_link_name.clone());
        let target_directory = tspath::get_directory_path(&absolute_link_name);
        let absolute_target = result.to_absolute_path_from(&symlink.target, &target_directory);
        if node.borrow().entry.is_none() {
            node.borrow_mut().entry = Some(RequestEntry::Symlink(Rc::new(RequestSymlinkEntry {
                link_name: absolute_link_name,
                target: absolute_target,
                host: symlink.host,
            })));
        }
    }
    for directory_name in &listed_directories {
        result.register_directory(directory_name);
    }
    for path in &params.removed_paths {
        let path = result.to_path(&result.to_absolute_path(path));
        ensure(&result.paths, &path).borrow_mut().fallback = RequestFallback::Missing;
    }
    result.paths = compose_request_paths(
        None,
        Some(&result.paths),
        RequestFallback::Allowed,
        result.use_case_sensitive_names,
    )
    .expect("a composed tree is non-nil");
    Ok(result)
}

impl RequestFileSystemImpl {
    // Go: api/requestfilesystem/requestfilesystem.go requestFileSystem.baseFileSystem
    fn base_file_system_value(&self) -> Rc<dyn vfs::Fs> {
        self.base.clone()
    }

    // Go: api/requestfilesystem/requestfilesystem.go requestFileSystem.applyTo
    pub fn apply_to(&self, base: &RequestFileSystemImpl) -> RequestFileSystemImpl {
        let mut s = self.clone();
        s.paths = compose_request_paths(
            Some(base.paths.clone()),
            Some(&self.paths),
            RequestFallback::Allowed,
            self.use_case_sensitive_names,
        )
        .expect("a composed tree is non-nil");
        s.kind = base.kind.clone();
        s.base = base.base.clone();
        s
    }

    // Go: api/requestfilesystem/requestfilesystem.go requestFileSystem.blocksFallback
    fn blocks_fallback(&self, path: &str) -> bool {
        let (_, fallback) = lookup(Some(&self.paths), &self.to_path(path));
        fallback == RequestFallback::Missing
    }

    // Go: api/requestfilesystem/requestfilesystem.go requestFileSystem.toAbsolutePath
    fn to_absolute_path(&self, path: &str) -> String {
        self.to_absolute_path_from(path, &self.current_directory)
    }

    // Go: api/requestfilesystem/requestfilesystem.go requestFileSystem.toAbsolutePathFrom
    fn to_absolute_path_from(&self, path: &str, current_directory: &str) -> String {
        let absolute_path = tspath::get_normalized_absolute_path(path, current_directory);
        if tspath::is_disk_path_root(&absolute_path) {
            return absolute_path;
        }
        tspath::remove_trailing_directory_separator(&absolute_path).to_string()
    }

    // Go: api/requestfilesystem/requestfilesystem.go requestFileSystem.toPath
    // (at 673a5f17d713; removed by ts#64159: Go N' keys paths with
    // `CaseSensitivity.PathKey`, which gives this key for rooted names)
    pub fn to_path(&self, path: &str) -> tspath::Path {
        tspath::to_path(path, &self.current_directory, self.use_case_sensitive_names)
    }

    // Go: api/requestfilesystem/requestfilesystem.go requestFileSystem.registerDirectory
    fn register_directory(&self, directory_name: &str) {
        let mut directory_name = self.to_absolute_path(directory_name);
        loop {
            let node = ensure(&self.paths, &self.to_path(&directory_name));
            if node.borrow().entry.is_some() {
                return;
            }
            node.borrow_mut().entry = Some(RequestEntry::Directory(Rc::new(RequestDirectory {
                directory_name: directory_name.clone(),
                listing: None,
            })));
            let parent_name = tspath::get_directory_path(&directory_name);
            if parent_name == directory_name {
                return;
            }
            directory_name = parent_name;
        }
    }

    // Go: api/requestfilesystem/requestfilesystem.go requestFileSystem.resolvePath
    fn resolve_path(&self, path: &str) -> ResolvedRequestPath {
        let path = self.to_absolute_path(path);
        let mut result = ResolvedRequestPath {
            path,
            ok: true,
            ..Default::default()
        };
        let mut seen: FxHashSet<tspath::Path> = FxHashSet::default();
        loop {
            let canonical_path = self.to_path(&result.path);
            if contains_file_ancestor(Some(&self.paths), &canonical_path) {
                result.ok = false;
                return result;
            }
            let Some((match_path, matched)) = first_symlink(Some(&self.paths), &canonical_path)
            else {
                result.host = self.is_host_path(&result.path);
                return result;
            };
            if seen.contains(&match_path) {
                result.ok = false;
                return result;
            }
            seen.insert(match_path);
            result.followed_symlink = true;
            let Some(suffix) = tspath::trim_file_path_prefix(
                &result.path,
                &matched.link_name,
                self.use_case_sensitive_names,
            ) else {
                result.ok = false;
                return result;
            };
            let suffix = suffix.strip_prefix('/').unwrap_or(&suffix).to_string();
            result.path =
                self.to_absolute_path(&tspath::combine_paths(&matched.target, &[&suffix]));
            if matched.host {
                result.host = true;
                return result;
            }
        }
    }

    // Go: api/requestfilesystem/requestfilesystem.go requestFileSystem.isHostPath
    fn is_host_path(&self, path: &str) -> bool {
        let canonical_path = self.to_path(path);
        let mut found = false;
        walk_symlinks(Some(&self.paths), &mut |_, symlink| {
            if symlink.host
                && request_path_contains(&self.to_path(&symlink.target), &canonical_path)
            {
                found = true;
            }
        });
        found
    }

    // Go: api/requestfilesystem/requestfilesystem.go requestFileSystem.aliasesForPath
    pub fn aliases_for_path(&self, path: &str) -> Vec<String> {
        let mut symlinks: Vec<Rc<RequestSymlinkEntry>> = Vec::new();
        walk_symlinks(Some(&self.paths), &mut |_, symlink| {
            symlinks.push(symlink.clone());
        });

        let mut seen: FxHashSet<tspath::Path> = FxHashSet::default();
        seen.insert(self.to_path(path));
        let mut queue: std::collections::VecDeque<String> =
            std::collections::VecDeque::from([self.to_absolute_path(path)]);
        let mut aliases: Vec<String> = Vec::new();
        while let Some(candidate) = queue.pop_front() {
            for symlink in &symlinks {
                let Some(suffix) = tspath::trim_file_path_prefix(
                    &candidate,
                    &symlink.target,
                    self.use_case_sensitive_names,
                ) else {
                    continue;
                };
                if !suffix.is_empty()
                    && !tspath::has_trailing_directory_separator(&symlink.target)
                    && !suffix.starts_with('/')
                {
                    continue;
                }
                let suffix = suffix.strip_prefix('/').unwrap_or(&suffix).to_string();
                let alias =
                    self.to_absolute_path(&tspath::combine_paths(&symlink.link_name, &[&suffix]));
                let alias_path = self.to_path(&alias);
                if seen.contains(&alias_path) {
                    continue;
                }
                if !self.resolve_path(&alias).ok {
                    continue;
                }
                seen.insert(alias_path);
                aliases.push(alias.clone());
                queue.push_back(alias);
            }
        }
        aliases
    }

    // Go: api/requestfilesystem/requestfilesystem.go requestFileSystem.localPathInfo
    fn local_path_info(&self, path: &str) -> (Option<RequestInfo>, RequestFallback) {
        let (node, fallback) = lookup(Some(&self.paths), &self.to_path(path));
        let Some(node) = node else {
            return (None, fallback);
        };
        let info = match &node.borrow().entry {
            Some(RequestEntry::File(file)) => Some(RequestInfo::File(file.clone())),
            Some(RequestEntry::Directory(directory)) => {
                Some(RequestInfo::Directory(directory.clone()))
            }
            _ => None,
        };
        (info, fallback)
    }

    // Go: api/requestfilesystem/requestfilesystem.go requestFileSystem.lookupPath
    fn lookup_path(&self, path: &str) -> RequestPathLookup {
        let absolute_path = self.to_absolute_path(path);
        let (info, path_fallback) = self.local_path_info(&absolute_path);
        if info.is_some() {
            return RequestPathLookup {
                path: absolute_path,
                info,
                ok: true,
                ..Default::default()
            };
        }
        if path_fallback == RequestFallback::Missing {
            return RequestPathLookup::default();
        }
        let resolved = self.resolve_path(path);
        if !resolved.ok {
            return RequestPathLookup::default();
        }
        let mut result = RequestPathLookup {
            path: resolved.path.clone(),
            followed_symlink: resolved.followed_symlink,
            ok: true,
            ..Default::default()
        };
        let (resolved_info, resolved_fallback) = self.local_path_info(&resolved.path);
        if !resolved.host && resolved_info.is_some() {
            result.info = resolved_info;
        } else if resolved.host || self.kind == Kind::LAYER {
            if resolved_fallback == RequestFallback::Missing {
                return RequestPathLookup::default();
            }
            // PORT: the Rust base is never nil, so `ok` stays true.
            result.file_system = Some(self.base.clone());
            result.ok = result.file_system.is_some();
        }
        result
    }

    // Go: api/requestfilesystem/requestfilesystem.go requestFileSystem.mutationPath
    fn mutation_path(&self, path: &str) -> Option<(Rc<dyn vfs::Fs>, String)> {
        if self.kind != Kind::LAYER {
            return None;
        }
        let resolved = self.resolve_path(path);
        if !resolved.ok {
            return None;
        }
        Some((self.base.clone(), resolved.path))
    }

    // Go: api/requestfilesystem/requestfilesystem.go requestFileSystem.filterLocalEntries
    fn filter_local_entries(&self, directory_name: &str, entries: &vfs::Entries) -> vfs::Entries {
        let mut result = clone_entries(entries);
        let keep = |name: &str| {
            let file_name = tspath::combine_paths(directory_name, &[name]);
            if self.local_path_info(&file_name).0.is_some() {
                return true;
            }
            !self.blocks_fallback(&file_name)
        };
        result.files.retain(|name| keep(name));
        result.directories.retain(|name| keep(name));
        if let Some(symlinks) = &mut result.symlinks {
            symlinks.retain(|name| keep(name));
        }
        result
    }

    // Go: api/requestfilesystem/requestfilesystem.go requestFileSystem.getLocalEntries
    fn get_local_entries(&self, directory_name: &str) -> (vfs::Entries, bool, bool) {
        let (node, _) = lookup(Some(&self.paths), &self.to_path(directory_name));
        let (entries, ok) = super::pathtree::entries(node.as_ref());
        let mut explicit = false;
        if let Some(node) = &node
            && let Some(RequestEntry::Directory(directory)) = &node.borrow().entry
        {
            explicit = directory.listing.is_some();
        }
        (entries, explicit, ok)
    }

    // Go: api/requestfilesystem/requestfilesystem.go requestFileSystem.removeEntries
    fn remove_entries(&self, directory_name: &str, entries: &vfs::Entries) -> vfs::Entries {
        let mut result = clone_entries(entries);
        let blocked =
            |name: &str| self.blocks_fallback(&tspath::combine_paths(directory_name, &[name]));
        result.files.retain(|name| !blocked(name));
        result.directories.retain(|name| !blocked(name));
        if let Some(symlinks) = &mut result.symlinks {
            symlinks.retain(|name| !blocked(name));
        }
        result
    }

    // Go: api/requestfilesystem/requestfilesystem.go requestFileSystem.addSymlinkEntries
    fn add_symlink_entries(&self, directory_name: &str, entries: &vfs::Entries) -> vfs::Entries {
        let mut result = clone_entries(entries);
        if result.symlinks.is_none() {
            result.symlinks = Some(FxHashSet::default());
        }

        let directory_path = self.to_path(directory_name);
        let mut links: Vec<Rc<RequestSymlinkEntry>> = Vec::new();
        if let (Some(node), _) = lookup(Some(&self.paths), &directory_path)
            && let Some(children) = &node.borrow().children
        {
            for child in children.values() {
                if let Some(RequestEntry::Symlink(symlink)) = &child.borrow().entry {
                    links.push(symlink.clone());
                }
            }
        }
        if links.is_empty() {
            return result;
        }
        for symlink in &links {
            let name = tspath::get_base_file_name(&symlink.link_name);
            result.files = self.delete_entry_name(std::mem::take(&mut result.files), &name);
            result.directories =
                self.delete_entry_name(std::mem::take(&mut result.directories), &name);
            let symlinks = result.symlinks.get_or_insert_with(FxHashSet::default);
            symlinks.retain(|existing_name| !self.equal_entry_names(existing_name, &name));
            if self.directory_exists(&symlink.link_name) {
                result.directories.push(name.clone());
                result
                    .symlinks
                    .get_or_insert_with(FxHashSet::default)
                    .insert(name);
            } else if self.file_exists(&symlink.link_name) {
                result.files.push(name.clone());
                result
                    .symlinks
                    .get_or_insert_with(FxHashSet::default)
                    .insert(name);
            }
        }
        result.files.sort();
        result.directories.sort();
        result
    }

    // Go: api/requestfilesystem/requestfilesystem.go requestFileSystem.deleteEntryName
    fn delete_entry_name(&self, mut values: Vec<String>, value: &str) -> Vec<String> {
        values.retain(|candidate| !self.equal_entry_names(candidate, value));
        values
    }

    // Go: api/requestfilesystem/requestfilesystem.go requestFileSystem.equalEntryNames
    fn equal_entry_names(&self, left: &str, right: &str) -> bool {
        tspath::get_canonical_file_name(left, self.use_case_sensitive_names)
            == tspath::get_canonical_file_name(right, self.use_case_sensitive_names)
    }
}

// Go: api/requestfilesystem/requestfilesystem.go cloneEntries
pub fn clone_entries(entries: &vfs::Entries) -> vfs::Entries {
    vfs::Entries {
        files: entries.files.clone(),
        directories: entries.directories.clone(),
        symlinks: entries.symlinks.clone(),
    }
}

// Go: api/requestfilesystem/requestfilesystem.go mergeEntries
pub fn merge_entries(
    base: &vfs::Entries,
    overlay: &vfs::Entries,
    equal: &dyn Fn(&str, &str) -> bool,
) -> vfs::Entries {
    let mut result = clone_entries(base);
    let mut symlinks = result.symlinks.take().unwrap_or_default();
    for name in &overlay.files {
        // addFile
        result.directories.retain(|value| !equal(value, name));
        if !result.files.iter().any(|value| equal(value, name)) {
            result.files.push(name.clone());
        }
        symlinks.retain(|existing_name| !equal(existing_name, name));
    }
    for name in &overlay.directories {
        // addDirectory
        result.files.retain(|value| !equal(value, name));
        if !result.directories.iter().any(|value| equal(value, name)) {
            result.directories.push(name.clone());
        }
        symlinks.retain(|existing_name| !equal(existing_name, name));
    }
    if let Some(overlay_symlinks) = &overlay.symlinks {
        for name in overlay_symlinks {
            symlinks.insert(name.clone());
        }
    }
    result.symlinks = Some(symlinks);
    result.files.sort();
    result.directories.sort();
    result
}

// Go: vfs.FS methods of requestFileSystem.
impl vfs::Fs for RequestFileSystemImpl {
    // Go: api/requestfilesystem/requestfilesystem.go requestFileSystem.UseCaseSensitiveFileNames
    // (at 673a5f17d713; ts#64159 renames it CaseSensitivity,
    // requestfilesystem.go:416; the port keeps the bool)
    fn use_case_sensitive_file_names(&self) -> bool {
        self.use_case_sensitive_names
    }

    // Go: api/requestfilesystem/requestfilesystem.go requestFileSystem.FileExists
    fn file_exists(&self, file_name: &str) -> bool {
        let lookup = self.lookup_path(file_name);
        if !lookup.ok || lookup.info.as_ref().is_some_and(RequestInfo::is_dir) {
            return false;
        }
        lookup.info.is_some()
            || lookup
                .file_system
                .as_ref()
                .is_some_and(|fs| fs.file_exists(&lookup.path))
    }

    // Go: api/requestfilesystem/requestfilesystem.go requestFileSystem.ReadFile
    fn read_file(&self, file_name: &str) -> (String, bool) {
        let lookup = self.lookup_path(file_name);
        if !lookup.ok || lookup.info.as_ref().is_some_and(RequestInfo::is_dir) {
            return (String::new(), false);
        }
        if let Some(file_system) = &lookup.file_system {
            return file_system.read_file(&lookup.path);
        }
        if let Some(RequestInfo::File(file)) = &lookup.info {
            return (file.content.clone(), true);
        }
        (String::new(), false)
    }

    // Go: api/requestfilesystem/requestfilesystem.go requestFileSystem.WriteFile
    fn write_file(&self, file_name: &str, data: &str) -> Result<(), vfs::FsError> {
        let Some((host, path)) = self.mutation_path(file_name) else {
            return Err(vfs::FsError::Invalid);
        };
        host.write_file(&path, data)
    }

    // Go: api/requestfilesystem/requestfilesystem.go requestFileSystem.AppendFile
    fn append_file(&self, file_name: &str, data: &str) -> Result<(), vfs::FsError> {
        let Some((host, path)) = self.mutation_path(file_name) else {
            return Err(vfs::FsError::Invalid);
        };
        host.append_file(&path, data)
    }

    // Go: api/requestfilesystem/requestfilesystem.go requestFileSystem.Remove
    fn remove(&self, path: &str) -> Result<(), vfs::FsError> {
        let Some((host, path)) = self.mutation_path(path) else {
            return Err(vfs::FsError::Invalid);
        };
        host.remove(&path)
    }

    // Go: api/requestfilesystem/requestfilesystem.go requestFileSystem.Chtimes
    fn chtimes(
        &self,
        path: &str,
        a_time: Option<SystemTime>,
        m_time: Option<SystemTime>,
    ) -> Result<(), vfs::FsError> {
        let Some((host, path)) = self.mutation_path(path) else {
            return Err(vfs::FsError::Invalid);
        };
        host.chtimes(&path, a_time, m_time)
    }

    // Go: api/requestfilesystem/requestfilesystem.go requestFileSystem.DirectoryExists
    fn directory_exists(&self, directory_name: &str) -> bool {
        let lookup = self.lookup_path(directory_name);
        if !lookup.ok || lookup.info.as_ref().is_some_and(|info| !info.is_dir()) {
            return false;
        }
        lookup.info.is_some()
            || lookup
                .file_system
                .as_ref()
                .is_some_and(|fs| fs.directory_exists(&lookup.path))
    }

    // Go: api/requestfilesystem/requestfilesystem.go requestFileSystem.GetAccessibleEntries
    fn get_accessible_entries(&self, directory_name: &str) -> vfs::Entries {
        let lookup = self.lookup_path(directory_name);
        if !lookup.ok || lookup.info.as_ref().is_some_and(|info| !info.is_dir()) {
            return vfs::Entries {
                symlinks: Some(FxHashSet::default()),
                ..Default::default()
            };
        }
        let mut result;
        if let Some(file_system) = &lookup.file_system {
            result = self.remove_entries(
                &lookup.path,
                &file_system.get_accessible_entries(&lookup.path),
            );
        } else {
            let (local_entries, explicit, _) = self.get_local_entries(&lookup.path);
            result = local_entries.clone();
            if self.kind == Kind::LAYER
                && !explicit
                && !self.blocks_fallback(directory_name)
                && !self.blocks_fallback(&lookup.path)
            {
                result = self.remove_entries(
                    &lookup.path,
                    &self
                        .base_file_system_value()
                        .get_accessible_entries(&lookup.path),
                );
                result = merge_entries(&result, &local_entries, &|left, right| {
                    self.equal_entry_names(left, right)
                });
            }
            result = self.add_symlink_entries(&lookup.path, &result);
        }
        self.filter_local_entries(directory_name, &result)
    }

    // Go: api/requestfilesystem/requestfilesystem.go requestFileSystem.Stat
    fn stat(&self, path: &str) -> Option<vfs::FileInfo> {
        let lookup = self.lookup_path(path);
        if !lookup.ok {
            return None;
        }
        if let Some(file_system) = &lookup.file_system {
            return stat_file_system(Some(&**file_system), &lookup.path);
        }
        lookup.info.as_ref().map(RequestInfo::file_info)
    }

    // Go: api/requestfilesystem/requestfilesystem.go requestFileSystem.Realpath
    fn realpath(&self, path: &str) -> String {
        let lookup = self.lookup_path(path);
        if !lookup.ok {
            return path.to_string();
        }
        if let Some(file_system) = &lookup.file_system {
            return file_system.realpath(&lookup.path);
        }
        if lookup.info.is_some() || !lookup.followed_symlink {
            return lookup.path;
        }
        path.to_string()
    }

    // PORT: Go type assertions (see the file header).
    fn as_any(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }
}

// PORT: the Go interfaces and the concrete type that `*requestFileSystem`
// satisfies (`project::FsLayer`, server lane).
impl project::FsLayer for RequestFileSystemImpl {
    fn as_file_handle_source(&self) -> Option<&dyn project::FileHandleSource> {
        Some(self)
    }

    fn as_layered_file_system(&self) -> Option<&dyn project::LayeredFileSystem> {
        Some(self)
    }

    fn as_rebasable_file_system(&self) -> Option<&dyn project::RebasableFileSystem> {
        Some(self)
    }

    fn as_file_change_expander(&self) -> Option<&dyn project::FileChangeExpander> {
        Some(self)
    }

    fn as_any(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }
}

// Go: api/requestfilesystem/requestfilesystem.go requestFileSystem.GetFile,
// GetFileByPath (project.FileHandleSource)
impl project::FileHandleSource for RequestFileSystemImpl {
    fn get_file(&self, file_name: &str) -> Option<Rc<dyn project::FileHandle>> {
        self.get_file_by_path(file_name, &self.to_path(file_name))
    }

    fn get_file_by_path(
        &self,
        file_name: &str,
        _path: &tspath::Path,
    ) -> Option<Rc<dyn project::FileHandle>> {
        let lookup = self.lookup_path(file_name);
        if !lookup.ok || lookup.info.as_ref().is_some_and(RequestInfo::is_dir) {
            return None;
        }
        if let Some(file_system) = &lookup.file_system {
            if let Some(source) = project::as_file_handle_source(&**file_system) {
                return source.get_file(&lookup.path);
            }
            let (content, ok) = file_system.read_file(&lookup.path);
            if ok {
                return Some(project::new_cached_file_handle(file_name, content));
            }
            return None;
        }
        if let Some(RequestInfo::File(file)) = &lookup.info {
            return Some(project::new_cached_file_handle(
                file_name,
                file.content.clone(),
            ));
        }
        None
    }
}

// Go: api/requestfilesystem/requestfilesystem.go requestFileSystem.Overlays
// (project.LayeredFileSystem)
impl project::LayeredFileSystem for RequestFileSystemImpl {
    // PORT: Go returns nil when no overlay is kept; that is an empty map.
    fn overlays(&self) -> Rc<IndexMap<tspath::Path, Rc<project::Overlay>>> {
        let Some(base) = project::as_layered_file_system(&*self.base) else {
            return Rc::new(IndexMap::default());
        };
        let mut result: IndexMap<tspath::Path, Rc<project::Overlay>> = IndexMap::default();
        for (path, overlay) in base.overlays().iter() {
            let lookup = self.lookup_path(&overlay.file_name());
            if lookup.file_system.is_none() || self.to_path(&lookup.path) != *path {
                continue;
            }
            result.insert(path.clone(), overlay.clone());
        }
        Rc::new(result)
    }
}

// Go: api/requestfilesystem/requestfilesystem.go requestFileSystem.BaseFileSystem,
// WithBaseFileSystem (project.RebasableFileSystem)
impl project::RebasableFileSystem for RequestFileSystemImpl {
    fn base_file_system(&self) -> Rc<dyn vfs::Fs> {
        self.base.clone()
    }

    fn with_base_file_system(&self, base: Rc<dyn vfs::Fs>) -> Rc<dyn project::LayeredFileSystem> {
        let mut clone = self.clone();
        clone.base = base;
        Rc::new(clone)
    }
}

// Go: api/requestfilesystem/requestfilesystem.go statFileSystem
fn stat_file_system(file_system: Option<&dyn vfs::Fs>, path: &str) -> Option<vfs::FileInfo> {
    let file_system = file_system?;
    if let Some(info) = file_system.stat(path) {
        return Some(info);
    }
    if file_system.directory_exists(path) {
        return Some(
            RequestDirectory {
                directory_name: path.to_string(),
                listing: None,
            }
            .file_info(),
        );
    }
    if file_system.file_exists(path) {
        return Some(
            RequestFile {
                file_name: path.to_string(),
                content: String::new(),
            }
            .file_info(),
        );
    }
    None
}
