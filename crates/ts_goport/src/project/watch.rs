//! Go `internal/project/watch.go`.
//!
//! PORT: one thread (project/dirty/interfaces.rs). Go mutexes are dropped;
//! state that Go writes after sharing is in `Cell` / `RefCell`. Go
//! `map[string]struct{}` is `FxHashSet<String>` (a nil map is empty). Go
//! `[]*lsproto.FileSystemWatcher` is `Vec<lsproto::FileSystemWatcher>`.

use crate::project::prelude::*;

use crate::frontend::stringutil_ls;
use std::cell::Cell;
use std::sync::atomic::{AtomicU64, Ordering};

// Go: project/watch.go:19 minWatchLocationDepth
pub const MIN_WATCH_LOCATION_DEPTH: i32 = 2;

// Go: project/watch.go:22 fileSystemWatcherKey
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct FileSystemWatcherKey {
    pub pattern: String,
    pub kind: lsproto::WatchKind,
}

// Go: project/watch.go:27 fileSystemWatcherValue
#[derive(Clone, Debug, Default)]
pub struct FileSystemWatcherValue {
    pub count: i32,
    pub id: WatcherID,
}

// Go: project/watch.go:37 watchRegistry
// watchRegistry tracks the current watch globs and how many individual
// WatchedFiles reference each glob. It provides ref-count helpers so callers
// don't manipulate the map directly.
//
// All methods are safe for concurrent use; locking is handled internally.
// PORT: `mu` is dropped (one thread); the maps are `RefCell`s. Go
// `*fileSystemWatcherValue` map values are plain values (never shared).
#[derive(Debug, Default)]
pub struct WatchRegistry {
    pub entries: RefCell<FxHashMap<FileSystemWatcherKey, FileSystemWatcherValue>>,
    pub pending: RefCell<FxHashSet<WatcherID>>,
}

// Go: project/watch.go:43 newWatchRegistry
pub fn new_watch_registry() -> Rc<WatchRegistry> {
    Rc::new(WatchRegistry {
        entries: RefCell::new(FxHashMap::default()),
        pending: RefCell::new(FxHashSet::default()),
    })
}

impl WatchRegistry {
    // Go: project/watch.go:53 watchRegistry.Acquire
    // Acquire increments the ref count for a watcher. If this is the first
    // reference (count goes from 0 to 1), it returns true so the caller knows
    // to register the watcher with the client.
    pub fn acquire(&self, watcher: &lsproto::FileSystemWatcher, id: WatcherID) -> bool {
        let key = to_file_system_watcher_key(watcher);
        let mut entries = self.entries.borrow_mut();
        let value = entries
            .entry(key)
            .or_insert_with(|| FileSystemWatcherValue { count: 0, id });
        value.count += 1;
        value.count == 1
    }

    // Go: project/watch.go:69 watchRegistry.Release
    // Release decrements the ref count for a watcher. If no references remain,
    // the entry is removed and the function returns the WatcherID and true so
    // the caller knows to unregister the watcher from the client.
    pub fn release(&self, watcher: &lsproto::FileSystemWatcher) -> (WatcherID, bool) {
        let key = to_file_system_watcher_key(watcher);
        let mut entries = self.entries.borrow_mut();
        let Some(value) = entries.get_mut(&key) else {
            return (WatcherID::default(), false);
        };
        if value.count <= 1 {
            let id = value.id.clone();
            entries.remove(&key);
            return (id, true);
        }
        value.count -= 1;
        (WatcherID::default(), false)
    }

    // Go: project/watch.go:86 watchRegistry.MarkPending
    // MarkPending records that a watcher's registration failed and needs retry.
    pub fn mark_pending(&self, id: WatcherID) {
        self.pending.borrow_mut().insert(id);
    }

    // Go: project/watch.go:93 watchRegistry.ClearPending
    // ClearPending removes a watcher from the pending set after successful registration.
    pub fn clear_pending(&self, id: &WatcherID) {
        self.pending.borrow_mut().remove(id);
    }

    // Go: project/watch.go:100 watchRegistry.IsPending
    // IsPending returns true if the watcher needs retry due to a previous failure.
    pub fn is_pending(&self, id: &WatcherID) -> bool {
        self.pending.borrow().contains(id)
    }
}

// Go: project/watch.go:107 PatternsAndIgnored
#[derive(Clone, Debug, Default)]
pub struct PatternsAndIgnored {
    pub directories_outside_workspace: Vec<String>,
    pub patterns_inside_workspace: Vec<String>,
    pub ignored: FxHashSet<String>,
}

// Go: project/watch.go:119 toFileSystemWatcherKey
// toFileSystemWatcherKey produces a deduplication key for a file system watcher.
// Note: this key is a simple string concatenation of the base and pattern, so
// structurally different watchers (Pattern vs RelativePattern, URI vs WorkspaceFolder)
// could theoretically collide. In practice, workspace watchers use plain Pattern
// with filesystem paths while outside-workspace watchers use RelativePattern with
// file:// URIs, so collisions don't occur.
pub fn to_file_system_watcher_key(w: &lsproto::FileSystemWatcher) -> FileSystemWatcherKey {
    let kind = match w.kind {
        Some(kind) => kind,
        None => lsproto::WatchKind(
            lsproto::WatchKind::CREATE.0
                | lsproto::WatchKind::CHANGE.0
                | lsproto::WatchKind::DELETE.0,
        ),
    };
    let mut pattern = String::new();
    if let Some(p) = &w.glob_pattern.pattern {
        pattern = p.clone();
    } else if let Some(relative_pattern) = &w.glob_pattern.relative_pattern {
        let mut base = String::new();
        if let Some(uri) = &relative_pattern.base_uri.uri {
            base = uri.0.clone();
        } else if relative_pattern.base_uri.workspace_folder.is_some() {
            crate::core::go_panic(
                "workspace folder-based relative patterns not implemented".to_string(),
            );
        }
        pattern = base + "/" + &relative_pattern.pattern;
    }
    FileSystemWatcherKey { pattern, kind }
}

// Go: project/watch.go:139 fileSystemWatcherGlobString
pub fn file_system_watcher_glob_string(w: &lsproto::FileSystemWatcher) -> String {
    if let Some(pattern) = &w.glob_pattern.pattern {
        return pattern.clone();
    }
    if let Some(relative_pattern) = &w.glob_pattern.relative_pattern {
        let mut base = String::new();
        if let Some(uri) = &relative_pattern.base_uri.uri {
            base = uri.0.clone();
        } else if relative_pattern.base_uri.workspace_folder.is_some() {
            crate::core::go_panic(
                "workspace folder-based relative patterns not implemented".to_string(),
            );
        }
        return base + "/" + &relative_pattern.pattern;
    }
    String::new()
}

// Go: project/watch.go:155 WatcherID
// PORT: Go `type WatcherID string`; the empty id is Go's "".
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct WatcherID(pub String);

// Go `%s` / `%v` of a string type prints the string.
impl std::fmt::Display for WatcherID {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

// Go: project/watch.go:157 watcherID
// PORT: Go `atomic.Uint64`: one process-wide counter that starts at 0. Ids
// reach the client (registration ids), so every thread must share it.
pub static WATCHER_ID: AtomicU64 = AtomicU64::new(0);

/// Go `watcherID.Add(delta)`: adds `delta` and returns the new value. Go
/// atomics are sequentially consistent and wrap on overflow.
fn watcher_id_add(delta: u64) -> u64 {
    WATCHER_ID
        .fetch_add(delta, Ordering::SeqCst)
        .wrapping_add(delta)
}

// Go: project/watch.go:159 WatchedFiles
// PORT: Go `*WatchedFiles[T]` is `Rc<WatchedFiles<T>>` (nil is `None`).
// `mu` is dropped; `computeWatchersOnce` is a `Cell<bool>` guard, and the
// fields that `Watchers` writes are `Cell` / `RefCell`.
pub struct WatchedFiles<T> {
    pub name: String,
    pub watch_kind: lsproto::WatchKind,
    pub has_relative_pattern_capability: bool,
    pub compute_glob_patterns: Rc<dyn Fn(&T) -> PatternsAndIgnored>,

    pub input: T,
    pub compute_watchers_once: Cell<bool>,
    pub workspace_watchers: RefCell<Vec<lsproto::FileSystemWatcher>>,
    pub outside_workspace_watchers: RefCell<Vec<lsproto::FileSystemWatcher>>,
    pub ignored: RefCell<FxHashSet<String>>,
    pub id: Cell<u64>,
}

// Go: project/watch.go:174 NewWatchedFiles
// PORT: Go leaves `input` at the zero value of T (`T::default()`).
pub fn new_watched_files<T: Default>(
    name: &str,
    watch_kind: lsproto::WatchKind,
    has_relative_pattern_capability: bool,
    compute_glob_patterns: Rc<dyn Fn(&T) -> PatternsAndIgnored>,
) -> Rc<WatchedFiles<T>> {
    Rc::new(WatchedFiles {
        id: Cell::new(watcher_id_add(1)),
        name: name.to_string(),
        watch_kind,
        has_relative_pattern_capability,
        compute_glob_patterns,
        input: T::default(),
        compute_watchers_once: Cell::new(false),
        workspace_watchers: RefCell::new(Vec::new()),
        outside_workspace_watchers: RefCell::new(Vec::new()),
        ignored: RefCell::new(FxHashSet::default()),
    })
}

// Go: project/watch.go:186 NewWatchedFilesForPaths (tsgo#4712)
// NewWatchedFilesForPaths creates a watcher for exact file paths, routing files outside the workspace
// through directory-based external watchers so clients can use URI-based RelativePatterns when supported.
pub fn new_watched_files_for_paths(
    name: &str,
    watch_kind: lsproto::WatchKind,
    has_relative_pattern_capability: bool,
    workspace_directory: &str,
    current_directory: &str,
    use_case_sensitive_file_names: bool,
) -> Rc<WatchedFiles<Vec<String>>> {
    let compare_paths_options = tspath::ComparePathsOptions {
        current_directory: current_directory.to_string(),
        use_case_sensitive_file_names,
    };
    let workspace_directory = workspace_directory.to_string();
    new_watched_files(
        name,
        watch_kind,
        has_relative_pattern_capability,
        Rc::new(move |files: &Vec<String>| {
            let mut result = PatternsAndIgnored::default();
            for file in files {
                if tspath::contains_path(&workspace_directory, file, &compare_paths_options) {
                    result.patterns_inside_workspace.push(file.clone());
                } else {
                    result
                        .directories_outside_workspace
                        .push(tspath::get_directory_path(file));
                }
            }
            result
        }),
    )
}

// Go: project/watch.go:211 Watchers
#[derive(Clone, Debug, Default)]
pub struct Watchers {
    pub watcher_id: WatcherID,
    pub workspace_watchers: Vec<lsproto::FileSystemWatcher>,
    pub outside_workspace_watchers: Vec<lsproto::FileSystemWatcher>,
    pub ignored_paths: FxHashSet<String>,
}

impl<T> WatchedFiles<T> {
    // Go: project/watch.go:218 WatchedFiles.Watchers
    // PORT: Go returns the shared slices and map; the port returns copies.
    pub fn watchers(&self) -> Watchers {
        // Go: w.computeWatchersOnce.Do(...)
        if !self.compute_watchers_once.get() {
            self.compute_watchers_once.set(true);
            let result = (self.compute_glob_patterns)(&self.input);
            let mut globs = result.patterns_inside_workspace.clone();
            globs.sort();
            globs.dedup();

            let ignored = result.ignored.clone();
            // ignored is only used for logging and doesn't affect watcher identity
            *self.ignored.borrow_mut() = ignored;
            let mut changed = false;
            let same_workspace_watchers = {
                let workspace_watchers = self.workspace_watchers.borrow();
                workspace_watchers.len() == globs.len()
                    && workspace_watchers.iter().zip(globs.iter()).all(|(a, b)| {
                        a.glob_pattern
                            .pattern
                            .as_ref()
                            .unwrap_or_else(|| crate::core::go_nil_dereference())
                            == b
                    })
            };
            if !same_workspace_watchers {
                *self.workspace_watchers.borrow_mut() = globs
                    .iter()
                    .map(|glob| lsproto::FileSystemWatcher {
                        glob_pattern: lsproto::PatternOrRelativePattern {
                            pattern: Some(glob.clone()),
                            ..Default::default()
                        },
                        kind: Some(self.watch_kind),
                    })
                    .collect();
                changed = true;
            }
            let mut dirs_outside = result.directories_outside_workspace;
            dirs_outside.sort();
            dirs_outside.dedup();
            let same_outside_watchers = {
                let outside_workspace_watchers = self.outside_workspace_watchers.borrow();
                outside_workspace_watchers.len() == dirs_outside.len()
                    && outside_workspace_watchers
                        .iter()
                        .zip(dirs_outside.iter())
                        .all(|(a, b)| {
                            file_system_watcher_glob_string(a)
                                == recursive_directory_glob_pattern(
                                    b,
                                    self.has_relative_pattern_capability,
                                )
                        })
            };
            if !same_outside_watchers {
                *self.outside_workspace_watchers.borrow_mut() = dirs_outside
                    .iter()
                    .map(|dir| {
                        new_recursive_directory_watcher(
                            dir,
                            self.watch_kind,
                            self.has_relative_pattern_capability,
                        )
                    })
                    .collect();
                changed = true;
            }
            if changed {
                self.id.set(watcher_id_add(1));
            }
        }

        Watchers {
            watcher_id: WatcherID(format!("{} watcher {}", self.name, self.id.get())),
            workspace_watchers: self.workspace_watchers.borrow().clone(),
            outside_workspace_watchers: self.outside_workspace_watchers.borrow().clone(),
            ignored_paths: self.ignored.borrow().clone(),
        }
    }

    // Go: project/watch.go:266 WatchedFiles.ID
    // PORT: Go allows a nil receiver; call as `WatchedFiles::id(w.as_deref())`.
    pub fn id(w: Option<&WatchedFiles<T>>) -> WatcherID {
        let Some(w) = w else {
            return WatcherID::default();
        };
        w.watchers().watcher_id
    }

    // Go: project/watch.go:273 WatchedFiles.Name
    pub fn name(&self) -> String {
        self.name.clone()
    }

    // Go: project/watch.go:277 WatchedFiles.WatchKind
    pub fn watch_kind(&self) -> lsproto::WatchKind {
        self.watch_kind
    }

    // Go: project/watch.go:281 WatchedFiles.Clone
    // PORT: Go allows a nil receiver; call as
    // `WatchedFiles::clone_(w.as_deref(), input)`. `clone_` keeps it apart
    // from `std::clone::Clone`. Go does not copy `id` (it stays 0) or
    // `ignored`.
    pub fn clone_(w: Option<&WatchedFiles<T>>, input: T) -> Option<Rc<WatchedFiles<T>>> {
        let w = w?;
        Some(Rc::new(WatchedFiles {
            name: w.name.clone(),
            watch_kind: w.watch_kind,
            has_relative_pattern_capability: w.has_relative_pattern_capability,
            compute_glob_patterns: w.compute_glob_patterns.clone(),
            workspace_watchers: RefCell::new(w.workspace_watchers.borrow().clone()),
            outside_workspace_watchers: RefCell::new(w.outside_workspace_watchers.borrow().clone()),
            input,
            compute_watchers_once: Cell::new(false),
            ignored: RefCell::new(FxHashSet::default()),
            id: Cell::new(0),
        }))
    }
}

// Go: project/watch.go:298 createResolutionLookupGlobMapper
// PORT: the Go input `*collections.SyncSet[tspath.Path]` is
// `Option<Rc<RefCell<FxHashSet<tspath::Path>>>>` (the sourceFS seen files).
// Go ranges over the set (random order); the result does not depend on the
// order (one file per directory, sorted outputs).
#[allow(clippy::type_complexity)]
pub fn create_resolution_lookup_glob_mapper(
    workspace_directory: &str,
    lib_directory: &str,
    current_directory: &str,
    use_case_sensitive_file_names: bool,
) -> Rc<dyn Fn(&Option<Rc<RefCell<FxHashSet<tspath::Path>>>>) -> PatternsAndIgnored> {
    let workspace_directory_path = tspath::to_path(
        workspace_directory,
        current_directory,
        use_case_sensitive_file_names,
    );
    let current_directory_path = tspath::to_path(
        current_directory,
        current_directory,
        use_case_sensitive_file_names,
    );
    let lib_directory_path = tspath::to_path(
        lib_directory,
        current_directory,
        use_case_sensitive_file_names,
    );

    Rc::new(
        move |data: &Option<Rc<RefCell<FxHashSet<tspath::Path>>>>| -> PatternsAndIgnored {
            let mut ignored: FxHashSet<String> = FxHashSet::default();
            let mut seen_dirs: FxHashSet<tspath::Path> = FxHashSet::default();
            let mut include_workspace = false;
            let mut include_root = false;
            let mut include_lib = false;
            let mut node_modules_directories: FxHashSet<tspath::Path> = FxHashSet::default();
            let mut external_directories: FxHashSet<tspath::Path> = FxHashSet::default();

            if let Some(data) = data {
                for path in data.borrow().iter() {
                    if tspath::is_dynamic_file_name(path) {
                        continue;
                    }
                    // Assuming all of the input paths are file paths, we can avoid
                    // duplicate work by only taking one file per dir, since their outputs
                    // will always be the same.
                    if !seen_dirs.insert(path.get_directory_path()) {
                        continue;
                    }

                    if workspace_directory_path.contains_path(path) {
                        include_workspace = true;
                    } else if current_directory_path.contains_path(path) {
                        include_root = true;
                    } else if lib_directory_path.contains_path(path) {
                        include_lib = true;
                    } else if let Some(idx) = path.0.find("/node_modules/") {
                        node_modules_directories.insert(tspath::Path(
                            path.0[..idx + "/node_modules".len()].to_string(),
                        ));
                    } else {
                        external_directories.insert(path.get_directory_path());
                    }
                }
            }

            let mut globs: Vec<String> = Vec::new();
            if include_workspace {
                globs.push(get_recursive_glob_pattern(&workspace_directory_path));
            }
            if include_root {
                globs.push(get_recursive_glob_pattern(&current_directory_path));
            }
            if include_lib {
                globs.push(get_recursive_glob_pattern(&lib_directory_path));
            }
            if !node_modules_directories.is_empty() {
                let mut node_modules_globs: Vec<String> =
                    Vec::with_capacity(node_modules_directories.len());
                for dir in &node_modules_directories {
                    node_modules_globs.push(get_recursive_glob_pattern(dir));
                }
                node_modules_globs.sort();
                globs.extend(node_modules_globs);
            }
            let mut outside_dirs: Vec<String> = Vec::new();
            if !external_directories.is_empty() {
                let mut external_dir_strings: Vec<String> =
                    Vec::with_capacity(external_directories.len());
                for dir in &external_directories {
                    external_dir_strings.push(dir.0.clone());
                }
                let (mut external_directory_parents, ignored_external_dirs) =
                    tspath::get_common_parents(
                        &external_dir_strings,
                        MIN_WATCH_LOCATION_DEPTH,
                        &get_path_components_for_watching,
                        &tspath::ComparePathsOptions {
                            use_case_sensitive_file_names: true, // Already using tspath.Path
                            ..Default::default()
                        },
                    );
                external_directory_parents.sort();
                ignored = ignored_external_dirs;
                outside_dirs = external_directory_parents;
            }

            PatternsAndIgnored {
                directories_outside_workspace: outside_dirs,
                patterns_inside_workspace: globs,
                ignored,
            }
        },
    )
}

// Go: project/watch.go:380 getTypingsLocationsGlobs
// PORT: Go maps are `IndexMap`s (insertion order; Go map order is random).
// The globs are sorted later (`WatchedFiles.Watchers`) and the external
// directories go through `GetCommonParents` and a sort.
pub fn get_typings_locations_globs(
    typings_files: &[String],
    typings_location: &str,
    workspace_directory: &str,
    current_directory: &str,
    use_case_sensitive_file_names: bool,
) -> PatternsAndIgnored {
    let mut include_typings_location = false;
    let mut include_workspace = false;
    let mut external_directories: IndexMap<tspath::Path, String> = IndexMap::new();
    let mut globs: IndexMap<tspath::Path, String> = IndexMap::new();
    let compare_paths_options = tspath::ComparePathsOptions {
        current_directory: current_directory.to_string(),
        use_case_sensitive_file_names,
    };
    for file in typings_files {
        if tspath::contains_path(typings_location, file, &compare_paths_options) {
            include_typings_location = true;
        } else if !tspath::contains_path(workspace_directory, file, &compare_paths_options) {
            let directory = tspath::get_directory_path(file);
            external_directories.insert(
                tspath::to_path(&directory, current_directory, use_case_sensitive_file_names),
                directory,
            );
        } else {
            include_workspace = true;
        }
    }
    let external_directory_values: Vec<String> = external_directories.values().cloned().collect();
    let (mut external_directory_parents, ignored) = tspath::get_common_parents(
        &external_directory_values,
        MIN_WATCH_LOCATION_DEPTH,
        &get_path_components_for_watching,
        &compare_paths_options,
    );
    external_directory_parents.sort();
    if include_workspace {
        globs.insert(
            tspath::to_path(
                workspace_directory,
                current_directory,
                use_case_sensitive_file_names,
            ),
            get_recursive_glob_pattern(workspace_directory),
        );
    }
    if include_typings_location {
        globs.insert(
            tspath::to_path(
                typings_location,
                current_directory,
                use_case_sensitive_file_names,
            ),
            get_recursive_glob_pattern(typings_location),
        );
    }
    PatternsAndIgnored {
        directories_outside_workspace: external_directory_parents,
        patterns_inside_workspace: globs.values().cloned().collect(),
        ignored,
    }
}

// Go: project/watch.go:424 getPathComponentsForWatching
pub fn get_path_components_for_watching(path: &str, current_directory: &str) -> Vec<String> {
    let components = tspath::get_path_components(path, current_directory);
    let root_length = perceived_os_root_length_for_watching(&components);
    if root_length <= 1 {
        return components;
    }
    let root_length = root_length as usize;
    let rest: Vec<&str> = components[1..root_length]
        .iter()
        .map(String::as_str)
        .collect();
    let new_root = tspath::combine_paths(&components[0], &rest);
    let mut result = vec![new_root];
    result.extend_from_slice(&components[root_length..]);
    result
}

// Go: project/watch.go:434 perceivedOsRootLengthForWatching
pub fn perceived_os_root_length_for_watching(path_components: &[String]) -> i32 {
    let length = path_components.len() as i32;
    if length <= 1 {
        return length;
    }
    if path_components[0].starts_with("//") {
        // Group UNC roots (//server/share) into a single component
        return 2;
    }
    let root = path_components[0].as_bytes();
    if root.len() == 3 && tspath::is_volume_character(root[0]) && root[1] == b':' && root[2] == b'/'
    {
        // Windows-style volume
        // PORT: Go `strings.EqualFold` is `stringutil_ls::equate_string_case_insensitive`.
        if stringutil_ls::equate_string_case_insensitive(&path_components[1], "users") {
            // Group C:/Users/username into a single component
            return std::cmp::min(3, length);
        }
        return 1;
    }
    if path_components[1] == "home" {
        // Group /home/username into a single component
        return std::cmp::min(3, length);
    }
    1
}

// Go: project/watch.go:458 getRecursiveGlobPattern
pub fn get_recursive_glob_pattern(directory: &str) -> String {
    format!(
        "{}/{}",
        tspath::remove_trailing_directory_separator(directory),
        "**/*"
    )
}

// Go: project/watch.go:464 recursiveDirectoryGlobPattern
// recursiveDirectoryGlobPattern returns the string form of a recursive watcher
// for the given directory that would be produced by newRecursiveDirectoryWatcher.
pub fn recursive_directory_glob_pattern(directory: &str, use_relative_pattern: bool) -> String {
    if use_relative_pattern {
        return lsconv::file_name_to_document_uri(directory).0 + "/**/*";
    }
    get_recursive_glob_pattern(directory)
}

// Go: project/watch.go:474 newRecursiveDirectoryWatcher
// newRecursiveDirectoryWatcher creates a FileSystemWatcher for recursively
// watching a directory. When useRelativePattern is true, a RelativePattern with
// a file:// base URI is used; otherwise a plain glob Pattern is used.
// PORT: Go returns a pointer; the watcher lists hold values.
pub fn new_recursive_directory_watcher(
    directory: &str,
    kind: lsproto::WatchKind,
    use_relative_pattern: bool,
) -> lsproto::FileSystemWatcher {
    if use_relative_pattern {
        let base_uri = lsproto::URI(lsconv::file_name_to_document_uri(directory).0);
        return lsproto::FileSystemWatcher {
            glob_pattern: lsproto::PatternOrRelativePattern {
                relative_pattern: Some(lsproto::RelativePattern {
                    base_uri: lsproto::WorkspaceFolderOrURI {
                        uri: Some(base_uri),
                        ..Default::default()
                    },
                    pattern: "**/*".to_string(),
                }),
                ..Default::default()
            },
            kind: Some(kind),
        };
    }
    let glob = get_recursive_glob_pattern(directory);
    lsproto::FileSystemWatcher {
        glob_pattern: lsproto::PatternOrRelativePattern {
            pattern: Some(glob),
            ..Default::default()
        },
        kind: Some(kind),
    }
}
