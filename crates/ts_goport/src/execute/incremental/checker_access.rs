//! U4a: the Go `*compiler.Program` methods that the incremental program
//! calls and `program.rs` does not expose yet.
//!
//! - `GetTypeCheckerForFileExclusive` (compiler/program.go:487). Go returns
//!   the checker and a release function. The port lends the file's checker
//!   to a callback on the checker's own thread
//!   (`program::with_type_checker_for_file`). Jobs for one checker run one at
//!   a time, which is what "exclusive" gives Go.
//! - Methods that read the Go frontend program (`GetParseFileRedirect`,
//!   `GetResolvedTypeReferenceDirectives`, `GetDefaultLibFile`,
//!   `CommandLine`, `Host`, `PackageJsonCacheEntries`). The frontend program
//!   is not thread-safe, so these work on the loading thread only, like
//!   `program.rs`.

use crate::frontend::prelude::*;

// Go: compiler/program.go:637 GetTypeCheckerForFileExclusive
// PORT: `f` runs with the checker of `file` on that checker's thread. Copy
// what `f` needs into it, and return plain data (handles, strings).
pub fn get_type_checker_for_file_exclusive<R: Send + 'static>(
    file: Node,
    f: impl FnOnce(&mut Checker) -> R + Send + 'static,
) -> R {
    with_type_checker_for_file(file, f)
}

/// The Go frontend program (Go `*compiler.Program`). Panics off the loading
/// thread.
fn frontend_program() -> Rc<NewProgram> {
    go_frontend_program().expect("the incremental program needs the Go frontend program")
}

// Go: compiler/program.go:236 GetParseFileRedirect
#[must_use]
pub fn get_parse_file_redirect(file_name: &str) -> String {
    frontend_program().get_parse_file_redirect(file_name)
}

// Go: compiler/program.go:2213 GetResolvedTypeReferenceDirectives (the
// resolutions of one file, in the map order)
// PORT: Go returns the program's map. The frontend program is not
// `'static`, so this returns the values of the file's entry. The map key is
// a `Path`; `Borrow<str>` looks it up without a copy of the path.
#[must_use]
pub fn get_resolved_type_reference_directives_in_file(
    path: &str,
) -> Vec<Rc<ResolvedTypeReferenceDirective>> {
    frontend_program()
        .get_resolved_type_reference_directives()
        .get(path)
        .map(|in_file| in_file.values().cloned().collect())
        .unwrap_or_default()
}

// Go: compiler/program.go:1816 GetDefaultLibFile
#[must_use]
pub fn get_default_lib_file(path: &Path) -> Option<Rc<LibFile>> {
    frontend_program().lib_files.get(path).cloned()
}

// Go: compiler/program.go CommandLine
#[must_use]
pub fn command_line() -> Rc<ParsedCommandLine> {
    frontend_program().command_line().clone()
}

// Go: compiler/program.go Host
#[must_use]
pub fn host() -> Rc<dyn CompilerHost> {
    frontend_program().host().clone()
}

// Go: compiler/program.go:187 PackageJsonCacheEntries
// PORT: Go's resolver cache also holds the package.json lookups of module
// specifier generation (program.go:147 GetNearestAncestorDirectoryWithPackageJson
// and :157 GetPackageJsonInfo), which checker threads make for declaration
// diagnostics and declaration emit. The port keeps those in the program's
// thread-safe `HostFsCache` (`modulespecifiers::host`). So they follow the
// resolver's entries here: each one whose key the resolver does not have.
// The order of the entries is not defined, as in Go.
pub fn package_json_cache_entries(mut f: impl FnMut(&Path, PackageJsonCacheEntry<'_>) -> bool) {
    let mut seen: FxHashSet<Path> = FxHashSet::default();
    let mut go_on = true;
    frontend_program().package_json_cache_entries(|key, entry| {
        seen.insert(key.clone());
        go_on = f(key, entry);
        go_on
    });
    if !go_on {
        return;
    }
    crate::program::with_host_fs_cache(|cache| {
        cache.package_json_entries(|key, package_directory, directory_exists, exists| {
            seen.contains(key)
                || f(
                    key,
                    PackageJsonCacheEntry {
                        package_directory,
                        directory_exists,
                        exists,
                    },
                )
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modulespecifiers::{ModuleSpecifierGenerationHost, ProgramHost};

    /// Writes `files` (path, text) to a new dir under the system temp dir
    /// and returns the dir with `/` separators. `name` names the dir.
    fn write_project(name: &str, files: &[(&str, &str)]) -> String {
        let dir = std::env::temp_dir().join(format!("{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for (path, text) in files {
            let path = dir.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        dir.to_string_lossy().replace('\\', "/")
    }

    /// The package.json cache key of `file_name` (Go `tspath.Path`). On a
    /// case-insensitive file system (macOS) the key is in lower case.
    fn cache_key(file_name: &str) -> String {
        to_path(file_name, "", osvfs_fs().use_case_sensitive_file_names()).0
    }

    // specstat1 (realworld3 gap 3): the package.json lookups that module
    // specifier generation makes on a checker thread are entries of the
    // program's package.json cache, as in Go, where they go to the
    // resolver's cache (compiler/program.go:147). So the build info lists
    // them (incremental/program.go:454 ensurePackageJsonsForState). Here
    // no program file is in `pkg/dist`, so only the module specifier lookup
    // asks for `pkg/dist/package.json`.
    #[test]
    fn package_json_entries_have_module_specifier_lookups() {
        let dir = write_project(
            "goport-specifier-package-jsons",
            &[
                (
                    "tsconfig.json",
                    r#"{"compilerOptions":{"types":[]},"files":["index.ts"]}"#,
                ),
                ("index.ts", "export const x = 1;\n"),
                ("node_modules/pkg/package.json", r#"{"name":"pkg"}"#),
                ("node_modules/pkg/dist/index.d.ts", "export {};\n"),
            ],
        );
        let program = crate::program::try_load_version(&format!("{dir}/tsconfig.json"), |_| {})
            .unwrap_or_else(|e| panic!("cannot load {dir}: {e}"));
        let _scope = crate::core::enter_program(Some(program));
        let dist = format!("{dir}/node_modules/pkg/dist");
        let nearest = std::thread::spawn(move || {
            crate::core::set_thread_program(Some(program));
            ProgramHost.get_nearest_ancestor_directory_with_package_json(&dist)
        })
        .join()
        .unwrap();
        let mut entries = Vec::new();
        package_json_cache_entries(|key, entry| {
            entries.push((key.0.clone(), entry.directory_exists, entry.exists));
            true
        });
        drop(_scope);
        crate::program::release_program(program);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(nearest, format!("{dir}/node_modules/pkg"));
        for expected in [
            (
                cache_key(&format!("{dir}/node_modules/pkg/dist/package.json")),
                true,
                false,
            ),
            (
                cache_key(&format!("{dir}/node_modules/pkg/package.json")),
                true,
                true,
            ),
        ] {
            assert!(
                entries.contains(&expected),
                "{expected:?} is not in the package.json entries {entries:?}"
            );
        }
    }

    // followups21 (R168 reviewer): Go ReuseProgram keeps `processedFiles`
    // (compiler/program.go:408), so the new program keeps the resolver, and
    // its cache keeps the package.json lookups of module specifier
    // generation of earlier builds. A --watch --incremental fast-path build
    // then lists them in the build info. Before, the new version started
    // with no lookups. Go gives each build a new host cache, so the worker
    // `file_exists` answers are not kept.
    #[test]
    fn reused_version_keeps_module_specifier_package_json_lookups() {
        let dir = write_project(
            "goport-reused-specifier-package-jsons",
            &[
                (
                    "tsconfig.json",
                    r#"{"compilerOptions":{"types":[]},"files":["index.ts"]}"#,
                ),
                ("index.ts", "export const x = 1;\n"),
                ("node_modules/pkg/package.json", r#"{"name":"pkg"}"#),
                ("node_modules/pkg/dist/index.d.ts", "export {};\n"),
            ],
        );
        let index = format!("{dir}/index.ts");
        let later = format!("{dir}/later.ts");
        let old = crate::program::try_load_version(&format!("{dir}/tsconfig.json"), |_| {})
            .unwrap_or_else(|e| panic!("cannot load {dir}: {e}"));
        let dist = format!("{dir}/node_modules/pkg/dist");
        let later_probe = later.clone();
        let (nearest, later_before) = std::thread::spawn(move || {
            crate::core::set_thread_program(Some(old));
            (
                ProgramHost.get_nearest_ancestor_directory_with_package_json(&dist),
                crate::program::file_exists(&later_probe),
            )
        })
        .join()
        .unwrap();

        std::fs::write(&index, "export const x = 2;\n").unwrap();
        std::fs::write(&later, "").unwrap();
        let (new, reused) = crate::program::update_program_version(old, &index);
        let later_after = std::thread::spawn(move || {
            crate::core::set_thread_program(Some(new));
            crate::program::file_exists(&later)
        })
        .join()
        .unwrap();
        let _scope = crate::core::enter_program(Some(new));
        let mut entries = Vec::new();
        package_json_cache_entries(|key, entry| {
            entries.push((key.0.clone(), entry.directory_exists, entry.exists));
            true
        });
        drop(_scope);
        crate::program::release_program(new);
        crate::program::release_program(old);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(nearest, format!("{dir}/node_modules/pkg"));
        assert!(!later_before);
        assert!(
            reused,
            "the edit keeps the imports, so the program is reused"
        );
        let expected = (
            cache_key(&format!("{dir}/node_modules/pkg/dist/package.json")),
            true,
            false,
        );
        assert!(
            entries.contains(&expected),
            "{expected:?} is not in the package.json entries of the reused version {entries:?}"
        );
        assert!(
            later_after,
            "the reused version kept a file_exists answer of the old version"
        );
    }
}
