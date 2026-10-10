//! Go: internal/vfs/walkdir.go (ts#64277).

use crate::frontend::prelude::*;

// Go: vfs/walkdir.go:18 WalkDir
// WalkDir calls walkFn for root and each accessible descendant in lexical order.
// Symbolic links are reported but not followed. Directory read failures are
// indistinguishable from empty directories because [FS.GetAccessibleEntries]
// does not return errors. The FileInfo returned by DirEntry.Info for a symbolic
// link contains only its name and mode because [FS] does not provide lstat.
// Using DirEntry.Info() requires the FS to implement Stat().
// PORT: Go `walkDirEntry.Info` calls `Stat` when the caller asks. A Rust
// `DirEntry` cannot hold the file system, so the walk calls `Stat` for each
// entry that is not a symbolic link and keeps the result
// (`new_walk_dir_entry`).
pub fn walk_dir(
    file_system: &dyn Fs,
    root: &str,
    walk_fn: &mut WalkDirFunc<'_>,
) -> Result<(), FsError> {
    let Some(root_info) = file_system.stat(root) else {
        return normalize_walk_dir_error(walk_fn(root, None, Some(FsError::NotExist)));
    };

    let use_case_sensitive_file_names = file_system.use_case_sensitive_file_names();
    let root_prefix = root[..get_root_length(root)].to_string();

    let mut walker = Walker {
        file_system,
        use_case_sensitive_file_names,
        root_prefix,
        visited: FxHashSet::default(),
        walk_fn,
    };

    let mut root_entry = file_info_to_dir_entry(root_info);
    let root_realpath = file_system.realpath(root);
    if get_root_length(root) != root.len() {
        let parent = get_directory_path(root);
        let expected_realpath =
            combine_paths(&file_system.realpath(&parent), &[&get_base_file_name(root)]);
        if !walker.equivalent(&root_realpath, &expected_realpath) {
            root_entry = new_walk_dir_entry(
                file_system,
                root,
                &get_base_file_name(root),
                FileMode::SYMLINK,
            );
        }
    }
    normalize_walk_dir_error(walker.visit(root, &root_entry, &root_realpath))
}

// PORT: the Go closures of `WalkDir` (`sameRoot`, `equivalent`,
// `canonicalize`, `visit`) and the variables they share.
struct Walker<'a, 'b> {
    file_system: &'a dyn Fs,
    use_case_sensitive_file_names: bool,
    root_prefix: String,
    visited: FxHashSet<String>,
    walk_fn: &'a mut WalkDirFunc<'b>,
}

impl Walker<'_, '_> {
    fn same_root(&self, path: &str) -> bool {
        let path_root_length = get_root_length(path);
        path_root_length == self.root_prefix.len()
            && compare_paths(
                &path[..path_root_length],
                &self.root_prefix,
                &ComparePathsOptions {
                    use_case_sensitive_file_names: self.use_case_sensitive_file_names,
                    ..Default::default()
                },
            ) == 0
    }

    fn equivalent(&self, left: &str, right: &str) -> bool {
        compare_paths(
            left,
            right,
            &ComparePathsOptions {
                use_case_sensitive_file_names: self.use_case_sensitive_file_names,
                ..Default::default()
            },
        ) == 0
    }

    fn canonicalize(&self, path: &str) -> String {
        get_canonical_file_name(&normalize_path(path), self.use_case_sensitive_file_names)
    }

    fn visit(&mut self, path: &str, entry: &DirEntry, realpath: &str) -> Result<(), FsError> {
        if entry.is_dir() {
            let canonical_realpath = self.canonicalize(realpath);
            if !self.visited.insert(canonical_realpath) {
                return Ok(());
            }
        }

        if let Err(err) = (self.walk_fn)(path, Some(entry), None) {
            if err.is_skip_dir() && entry.is_dir() {
                return Ok(());
            }
            return Err(err);
        }
        if !entry.is_dir() {
            return Ok(());
        }

        let entries = self.file_system.get_accessible_entries(path);
        let directories: FxHashSet<&str> = entries.directories.iter().map(String::as_str).collect();
        let mut names: Vec<&str> = entries.directories.iter().map(String::as_str).collect();
        names.extend(entries.files.iter().map(String::as_str));
        names.sort_unstable();
        for name in names {
            let child_path = combine_paths(path, &[name]);
            if !self.same_root(&child_path) {
                continue;
            }

            let mut mode = FileMode(0);
            if directories.contains(name) {
                mode = FileMode::DIR;
            }
            let mut child_realpath = String::new();
            let is_symlink;
            if let Some(symlinks) = &entries.symlinks {
                is_symlink = symlinks.contains(name);
                if !is_symlink && mode.is_dir() {
                    child_realpath = combine_paths(realpath, &[name]);
                }
            } else {
                child_realpath = self.file_system.realpath(&child_path);
                is_symlink = !self.equivalent(&child_realpath, &combine_paths(realpath, &[name]));
            }
            if is_symlink {
                mode = FileMode::SYMLINK;
            }
            let child_entry = new_walk_dir_entry(self.file_system, &child_path, name, mode);
            if !mode.is_dir() {
                if let Err(err) = self.visit(&child_path, &child_entry, "") {
                    if err.is_skip_dir() {
                        return Ok(());
                    }
                    return Err(err);
                }
                continue;
            }

            if child_realpath.is_empty() {
                child_realpath = self.file_system.realpath(&child_path);
            }
            if let Err(err) = self.visit(&child_path, &child_entry, &child_realpath) {
                if err.is_skip_dir() {
                    return Ok(());
                }
                return Err(err);
            }
        }
        Ok(())
    }
}

// Go: vfs/walkdir.go:147 normalizeWalkDirError
fn normalize_walk_dir_error(err: Result<(), FsError>) -> Result<(), FsError> {
    match err {
        Err(err) if err.is_skip_dir() || err.is_skip_all() => Ok(()),
        err => err,
    }
}

// Go: vfs/walkdir.go:154 walkDirEntry
// PORT: a `DirEntry` value. `Info` (Go walkdir.go:165): a symbolic link has
// the Go `walkDirFileInfo` (its name and mode, size 0, the zero time);
// another entry has the `Stat` of its path, or `ErrNotExist`
// (`DirEntryInfo::NotExist`). See `walk_dir` for when `Stat` runs.
fn new_walk_dir_entry(file_system: &dyn Fs, path: &str, name: &str, mode: FileMode) -> DirEntry {
    let info = if mode.intersects(FileMode::SYMLINK) {
        // Go: vfs/walkdir.go:174 walkDirFileInfo
        DirEntryInfo::Known(FileInfo {
            name: name.to_string(),
            size: 0,
            mode,
            mod_time: None,
        })
    } else {
        match file_system.stat(path) {
            Some(info) => DirEntryInfo::Known(info),
            None => DirEntryInfo::NotExist,
        }
    };
    DirEntry {
        name: name.to_string(),
        typ: mode.type_(),
        info,
    }
}
