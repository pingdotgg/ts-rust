//! Port of Go `internal/symlinks/knownsymlinks.go`.

use crate::prelude::*;

use super::tspath;
use super::tspath::Path;

// Go: symlinks/knownsymlinks.go:13 KnownDirectoryLink
#[derive(Clone, Debug, Default)]
pub struct KnownDirectoryLink {
    /// Matches the spelling used to reach the symlink.
    /// Always has trailing directory separator. (ts#64544)
    pub symlink: String,
    /// Matches the casing returned by `realpath`. Used to compute the `realpath` of children.
    /// Always has trailing directory separator
    pub real: String,
    /// toPath(real). Stored to avoid repeated recomputation.
    /// Always has trailing directory separator
    pub real_path: Path,
}

// Go: symlinks/knownsymlinks.go:23 KnownSymlinks
// PORT: Go uses SyncMap and SyncSet, whose iteration order is random. The
// cache is filled once and then only read, so plain ordered maps are used.
#[derive(Clone, Debug, Default)]
pub struct KnownSymlinks {
    directories: IndexMap<Path, Option<KnownDirectoryLink>>,
    directories_by_realpath: IndexMap<Path, IndexSet<String>>,
    files: IndexMap<Path, String>,
    files_by_realpath: IndexMap<Path, IndexSet<String>>,
    cwd: String,
    use_case_sensitive_file_names: bool,
}

// PORT: `Path` needs `Hash` and `Eq` as a map key. The frontend `Path` type
// derives them.

impl KnownDirectoryLink {
    // Go: symlinks/knownsymlinks.go:68 KnownDirectoryLink.ResolveFilePath (ts#64544 adds it as
    // ResolveFileName; ts#64159 renames it)
    // The real path of `file_name`, a path under the symlink, or `None` when
    // `file_name` is not under the symlink.
    pub fn resolve_file_name(
        &self,
        file_name: &str,
        use_case_sensitive_file_names: bool,
    ) -> Option<String> {
        let relative =
            tspath::trim_file_path_prefix(file_name, &self.symlink, use_case_sensitive_file_names)?;
        Some(format!("{}{relative}", self.real))
    }
}

impl KnownSymlinks {
    // Go: symlinks/knownsymlinks.go:74 NewKnownSymlink (at 673a5f17d713;
    // ts#64159 renames it NewKnownSymlinks, symlinks/knownsymlinks.go:85)
    pub fn new(current_directory: &str, use_case_sensitive_file_names: bool) -> KnownSymlinks {
        KnownSymlinks {
            cwd: current_directory.to_string(),
            use_case_sensitive_file_names,
            ..Default::default()
        }
    }

    // Go: symlinks/knownsymlinks.go:31 HasDirectory
    pub fn has_directory(&self, symlink_path: &Path) -> bool {
        self.directories
            .contains_key(&symlink_path.ensure_trailing_directory_separator())
    }

    // Go: symlinks/knownsymlinks.go:37 Directories
    // Gets a map from symlink to realpath. Keys have trailing directory separators.
    pub fn directories(&self) -> &IndexMap<Path, Option<KnownDirectoryLink>> {
        &self.directories
    }

    // Go: symlinks/knownsymlinks.go:41 DirectoriesByRealpath
    pub fn directories_by_realpath(&self) -> &IndexMap<Path, IndexSet<String>> {
        &self.directories_by_realpath
    }

    // Go: symlinks/knownsymlinks.go:46 Files
    // Gets a map from symlink to realpath
    pub fn files(&self) -> &IndexMap<Path, String> {
        &self.files
    }

    // Go: symlinks/knownsymlinks.go:51 FilesByRealpath
    // Gets a map from realpath to symlinks
    pub fn files_by_realpath(&self) -> &IndexMap<Path, IndexSet<String>> {
        &self.files_by_realpath
    }

    // Go: symlinks/knownsymlinks.go:55 SetDirectory
    // ts#64544: the stored link keeps the spelling of `symlink`.
    pub fn set_directory(
        &mut self,
        symlink: &str,
        symlink_path: Path,
        real_directory: Option<KnownDirectoryLink>,
    ) {
        let real_directory = real_directory.map(|mut link| {
            link.symlink = tspath::ensure_trailing_directory_separator(symlink);
            link
        });
        if let Some(real_directory) = &real_directory {
            if !self.directories.contains_key(&symlink_path) {
                self.directories_by_realpath
                    .entry(real_directory.real_path.clone())
                    .or_default()
                    .insert(symlink.to_string());
            }
        }
        self.directories.insert(symlink_path, real_directory);
    }

    // Go: symlinks/knownsymlinks.go:76 SetFile
    pub fn set_file(&mut self, symlink: &str, symlink_path: Path, realpath: &str) {
        if !self.files.contains_key(&symlink_path) {
            let realpath_path =
                tspath::to_path(realpath, &self.cwd, self.use_case_sensitive_file_names);
            self.files_by_realpath
                .entry(realpath_path)
                .or_default()
                .insert(symlink.to_string());
        }
        self.files.insert(symlink_path, realpath.to_string());
    }

    // Go: symlinks/knownsymlinks.go:103 ProcessResolution
    pub fn process_resolution(&mut self, original_path: &str, resolved_file_name: &str) {
        if original_path.is_empty() || resolved_file_name.is_empty() {
            return;
        }
        let cwd = self.cwd.clone();
        let case = self.use_case_sensitive_file_names;
        self.set_file(
            original_path,
            tspath::to_path(original_path, &cwd, case),
            resolved_file_name,
        );
        let (common_resolved, common_original) =
            self.guess_directory_symlink(resolved_file_name, original_path, &cwd);
        if !common_resolved.is_empty() && !common_original.is_empty() {
            let symlink_path = tspath::to_path(&common_original, &cwd, case);
            if !tspath::contains_ignored_path(&symlink_path) {
                self.set_directory(
                    &common_original,
                    symlink_path.ensure_trailing_directory_separator(),
                    Some(KnownDirectoryLink {
                        // `set_directory` sets the symlink spelling.
                        symlink: String::new(),
                        real: tspath::ensure_trailing_directory_separator(&common_resolved),
                        real_path: tspath::to_path(&common_resolved, &cwd, case)
                            .ensure_trailing_directory_separator(),
                    }),
                );
            }
        }
    }

    // Go: symlinks/knownsymlinks.go:114 guessDirectorySymlink (at 673a5f17d713;
    // ts#64159 makes it guessDirectorySymlinkFromFilePaths, symlinks/knownsymlinks.go:124)
    fn guess_directory_symlink(&self, a: &str, b: &str, cwd: &str) -> (String, String) {
        let mut a_parts =
            tspath::get_path_components(&tspath::get_normalized_absolute_path(a, cwd), "");
        let mut b_parts =
            tspath::get_path_components(&tspath::get_normalized_absolute_path(b, cwd), "");
        let mut is_directory = false;
        while a_parts.len() >= 2
            && b_parts.len() >= 2
            && !self.is_node_modules_or_scoped_package_directory(&a_parts[a_parts.len() - 2])
            && !self.is_node_modules_or_scoped_package_directory(&b_parts[b_parts.len() - 2])
            && tspath::get_canonical_file_name(
                &a_parts[a_parts.len() - 1],
                self.use_case_sensitive_file_names,
            ) == tspath::get_canonical_file_name(
                &b_parts[b_parts.len() - 1],
                self.use_case_sensitive_file_names,
            )
        {
            a_parts.pop();
            b_parts.pop();
            is_directory = true;
        }
        if is_directory {
            return (
                tspath::get_path_from_path_components(&a_parts),
                tspath::get_path_from_path_components(&b_parts),
            );
        }
        (String::new(), String::new())
    }

    // Go: symlinks/knownsymlinks.go:145 isNodeModulesOrScopedPackageDirectory
    fn is_node_modules_or_scoped_package_directory(&self, s: &str) -> bool {
        !s.is_empty()
            && (tspath::get_canonical_file_name(s, self.use_case_sensitive_file_names)
                == "node_modules"
                || s.starts_with('@'))
    }
}
