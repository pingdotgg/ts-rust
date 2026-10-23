//! Port of Go `internal/symlinks/knownsymlinks.go`.

use crate::prelude::*;

use super::tspath;
use super::tspath::Path;
use std::borrow::Cow;

// Go: symlinks/knownsymlinks.go:13 KnownDirectoryLink
#[derive(Clone, Debug, Default)]
pub struct KnownDirectoryLink {
    /// Matches the spelling used to reach the symlink. Used to preserve the
    /// spelling of child paths when substituting the real directory.
    // Go N' (ts#64159, symlinks/knownsymlinks.go:16): a
    // `tspath.RootedDirectoryPath`, stored as `SetDirectory` gets it, with no
    // trailing directory separator (ts#64544 added one).
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
    // Go N': `RelativeFilePathFromDirectory(link.Symlink, fileName)` (tspath
    // rooted_path.go:563, `trimContainedPath` :575) takes the rest of
    // `file_name` after a separator below `symlink`, then
    // `link.Real.ResolveRelativeFile` (:746) joins it to `real`.
    // PORT: `real` keeps the trailing separator of the 673a5f17d713 port, so
    // the join is a plain append (Go `appendPathToDirectory`, :853).
    pub fn resolve_file_name(
        &self,
        file_name: &str,
        use_case_sensitive_file_names: bool,
    ) -> Option<String> {
        let relative = tspath::relative_path_within_directory(
            &self.symlink,
            file_name,
            use_case_sensitive_file_names,
        )?;
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
    // ts#64544: the stored link keeps the spelling of `symlink`. Go N'
    // (ts#64159, :58 `link.Symlink = symlink`) stores it unchanged.
    pub fn set_directory(
        &mut self,
        symlink: &str,
        symlink_path: Path,
        real_directory: Option<KnownDirectoryLink>,
    ) {
        let real_directory = real_directory.map(|mut link| {
            link.symlink = symlink.to_string();
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
        let case = self.use_case_sensitive_file_names;
        let original_key = tspath::to_path(original_path, &self.cwd, case);
        self.set_file(original_path, original_key, resolved_file_name);
        // PORT: Go N' gets both names as `RootedFilePath`, rooted and normal
        // by type. The port normalizes them here, as at 673a5f17d713
        // (`guessDirectorySymlink`); a name that is normal already is kept.
        let resolved = tspath::normalized_absolute_path_cow(resolved_file_name, &self.cwd);
        let original = tspath::normalized_absolute_path_cow(original_path, &self.cwd);
        let Some((common_resolved, common_original)) =
            self.guess_directory_symlink_from_file_paths(&resolved, &original)
        else {
            return;
        };
        if common_resolved.is_empty() || common_original.is_empty() {
            return;
        }
        let symlink_path = tspath::to_path(common_original, &self.cwd, case);
        if !tspath::contains_ignored_path(&symlink_path) {
            let real_directory = KnownDirectoryLink {
                // `set_directory` sets the symlink spelling.
                symlink: String::new(),
                real: tspath::ensure_trailing_directory_separator(common_resolved),
                real_path: tspath::to_path(common_resolved, &self.cwd, case)
                    .ensure_trailing_directory_separator(),
            };
            self.set_directory(
                common_original,
                symlink_path.ensure_trailing_directory_separator(),
                Some(real_directory),
            );
        }
    }

    // Go: symlinks/knownsymlinks.go:124 guessDirectorySymlinkFromFilePaths
    // (ts#64159; guessDirectorySymlink at 673a5f17d713)
    /// The longest directories of `a` and `b`, two normalized absolute
    /// paths, whose rest has the same parts below a directory that is not
    /// node_modules or a scope. `None` when not even the last parts match.
    // PERF (modpath1): Go walks both paths up by slicing (`Directory`,
    // `BaseName`). The port built two arrays of part strings, a string per
    // compare and the joined results.
    fn guess_directory_symlink_from_file_paths<'p>(
        &self,
        mut a: &'p str,
        mut b: &'p str,
    ) -> Option<(&'p str, &'p str)> {
        let mut is_directory = false;
        loop {
            let a_parent = tspath::get_directory_path_from_normalized(a);
            let b_parent = tspath::get_directory_path_from_normalized(b);
            if a_parent == a
                || b_parent == b
                || self.is_node_modules_or_scoped_package_directory(
                    tspath::get_base_file_name_from_normalized(a_parent),
                )
                || self.is_node_modules_or_scoped_package_directory(
                    tspath::get_base_file_name_from_normalized(b_parent),
                )
                || self.canonicalize(tspath::get_base_file_name_from_normalized(a))
                    != self.canonicalize(tspath::get_base_file_name_from_normalized(b))
            {
                break;
            }
            a = a_parent;
            b = b_parent;
            is_directory = true;
        }
        is_directory.then_some((a, b))
    }

    // Go: tspath/path.go:1019 CaseSensitivity.Canonicalize: the text itself
    // when case-sensitive.
    fn canonicalize<'s>(&self, s: &'s str) -> Cow<'s, str> {
        if self.use_case_sensitive_file_names {
            Cow::Borrowed(s)
        } else {
            Cow::Owned(tspath::get_canonical_file_name(s, false))
        }
    }

    // Go: symlinks/knownsymlinks.go:145 isNodeModulesOrScopedPackageDirectory
    fn is_node_modules_or_scoped_package_directory(&self, s: &str) -> bool {
        !s.is_empty() && (self.canonicalize(s) == "node_modules" || s.starts_with('@'))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Go: symlinks/knownsymlinks_test.go:129 TestGuessDirectorySymlink
    // PORT: the guess is private, so the test sits here (the other Go tests
    // of this file are in tests/go_baselines/units_platform/symlinks.rs).
    // Go's ("", "") is `None`. The inputs are rooted and normalized, so Go's
    // `ToRootedFilePath(name, cwd)` is the name itself.
    #[test]
    fn guess_directory_symlink() {
        let cache = KnownSymlinks::new("/test/dir", true);
        // (name, a, b, [commonResolved, commonOriginal])
        let tests: &[(&str, &str, &str, Option<(&str, &str)>)] = &[
            (
                "identical paths",
                "/test/path/file.ts",
                "/test/path/file.ts",
                Some(("/", "/")),
            ),
            (
                "different files same directory",
                "/test/path/file1.ts",
                "/test/path/file2.ts",
                None,
            ),
            (
                "different directories",
                "/test/path1/file.ts",
                "/test/path2/file.ts",
                Some(("/test/path1", "/test/path2")),
            ),
            (
                "node_modules paths",
                "/test/node_modules/pkg/file.ts",
                "/test/node_modules/pkg/file.ts",
                Some(("/test/node_modules/pkg", "/test/node_modules/pkg")),
            ),
            (
                "scoped package paths",
                "/test/node_modules/@scope/pkg/file.ts",
                "/test/node_modules/@scope/pkg/file.ts",
                Some((
                    "/test/node_modules/@scope/pkg",
                    "/test/node_modules/@scope/pkg",
                )),
            ),
        ];
        let failures: Vec<String> = tests
            .iter()
            .filter_map(|&(name, a, b, expected)| {
                let got = cache.guess_directory_symlink_from_file_paths(a, b);
                (got != expected).then(|| format!("{name}: got {got:?}, expected {expected:?}"))
            })
            .collect();
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }
}
