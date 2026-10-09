//! Ports of internal/module/resolver_test.go,
//! internal/module/staticresolver_test.go (ts#64299) and
//! internal/modulespecifiers/specifiers_test.go.
//!
//! Not ported (blocked, see bugs/S3.md): TestContainsIgnoredPath and
//! TestTryGetModuleNameFromExportsOrImports (`contains_ignored_path` and
//! `try_get_module_name_from_exports_or_imports` are private in
//! `modulespecifiers/specifiers.rs`).

use super::Subtests;
use crate::support::vfstest;
use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::sync::Arc;
use ts_goport::frontend::module::{
    DefaultResolver, ResolutionHost, Resolver, ResolverOptions, StaticResolutionEntry,
    new_resolver, new_static_resolutions, new_static_resolver,
    node_module_package_root_for_directory, node_module_package_root_for_file,
    normalize_path_for_cjs_resolution,
};
use ts_goport::frontend::tspath::{self, Path};
use ts_goport::frontend::vfs::{Fs, Replacements, wrapvfs_wrap};
use ts_goport::modulespecifiers::deps::OutputPathsHost;
use ts_goport::modulespecifiers::packagejson::InfoCacheEntry;
use ts_goport::modulespecifiers::symlinks::{KnownDirectoryLink, KnownSymlinks};
use ts_goport::modulespecifiers::{
    ModuleSpecifierGenerationHost, contains_node_modules, get_each_file_name_of_module,
    try_get_real_file_name_for_non_js_declaration_file_name,
};
use ts_goport::prelude::*;

// ---------------------------------------------------------------------------
// module/resolver_test.go
// ---------------------------------------------------------------------------

// Go: module/resolver_test.go:15 resolutionHostStub
struct ResolutionHostStub {
    fs: Rc<dyn Fs>,
    cwd: String,
}

impl ResolutionHost for ResolutionHostStub {
    fn fs(&self) -> &dyn Fs {
        &*self.fs
    }
    fn get_current_directory(&self) -> &str {
        &self.cwd
    }
}

/// Go `&core.CompilerOptions{ModuleResolution: Bundler, Module: ESNext,
/// Target: ESNext}`.
fn bundler_options() -> Rc<CompilerOptions> {
    Rc::new(CompilerOptions {
        module_resolution: ModuleResolutionKind::BUNDLER,
        module: ModuleKind::ES_NEXT,
        target: ScriptTarget::ES_NEXT,
        ..Default::default()
    })
}

/// Go `module.NewResolver(module.ResolverOptions{Host: host, CompilerOptions:
/// opts})` over `fs` with cwd `/repo` (ts#64299).
fn new_repo_resolver(fs: Rc<dyn Fs>) -> DefaultResolver {
    let host: Rc<dyn ResolutionHost> = Rc::new(ResolutionHostStub {
        fs,
        cwd: "/repo".to_string(),
    });
    new_resolver(ResolverOptions {
        host: Some(host),
        compiler_options: Some(bundler_options()),
        ..Default::default()
    })
}

/// Go `r, _, _ := resolver.ResolveModuleName(name, containingFile,
/// core.ModuleKindESNext, nil); r.IsResolved()`.
fn resolves(resolver: &DefaultResolver, name: &str, containing_file: &str) -> bool {
    let (r, _, _) = resolver.resolve_module_name(name, containing_file, ModuleKind::ES_NEXT, None);
    r.is_resolved()
}

// Go: module/resolver_test.go:27 TestResolveModuleNameTrailingSlash
/// Regression test for https://github.com/microsoft/typescript-go/issues/3526.
///
/// Resolving a node_modules import with a trailing slash (e.g. `pkg/`) must
/// produce the same result as without one.
#[test]
fn test_resolve_module_name_trailing_slash() {
    let fs = vfstest::from_map(
        [
            (
                "/repo/node_modules/pkg/package.json",
                r#"{"name":"pkg","main":"main.js","types":"main.d.ts"}"#,
            ),
            (
                "/repo/node_modules/pkg/main.d.ts",
                "export const x: number;",
            ),
            ("/repo/node_modules/pkg/main.js", "exports.x = 1;"),
            ("/repo/src/file.ts", ""),
        ],
        true,
    );
    let resolver = new_repo_resolver(fs);

    let mut errors = Vec::new();
    for name in ["pkg", "pkg/"] {
        if !resolves(&resolver, name, "/repo/src/file.ts") {
            errors.push(format!("{name:?} failed to resolve"));
        }
    }
    assert!(errors.is_empty(), "{}", errors.join("\n"));
}

/// The cwd of the dynamic resolver tests (ts#64544).
const DYNAMIC_ROOT: &str = "^/~ts-uri~/custom/ts-nul-authority/";

/// Go `NewResolver(ResolverOptions{Host: &resolutionHostStub{fs: fs, cwd:
/// "^/~ts-uri~/custom/ts-nul-authority/"}, CompilerOptions: opts})`.
fn new_dynamic_resolver(files: &[(&str, &str)], options: CompilerOptions) -> DefaultResolver {
    let fs = vfstest::from_map(files.iter().copied(), true);
    let host: Rc<dyn ResolutionHost> = Rc::new(ResolutionHostStub {
        fs,
        cwd: DYNAMIC_ROOT.to_string(),
    });
    new_resolver(ResolverOptions {
        host: Some(host),
        compiler_options: Some(Rc::new(options)),
        ..Default::default()
    })
}

/// `bundler_options` with `rootDirs`.
fn bundler_options_with_root_dirs(root_dirs: &[&str]) -> CompilerOptions {
    CompilerOptions {
        root_dirs: Some(root_dirs.iter().map(|d| d.to_string()).collect()),
        ..(*bundler_options()).clone()
    }
}

/// Go `resolved, _, _ := resolver.ResolveModuleName(name, sourceFile, mode,
/// nil)`; the resolved file name, or `None` when it is not resolved.
fn resolved_file(
    resolver: &DefaultResolver,
    name: &str,
    source_file: &str,
    mode: ModuleKind,
) -> Option<String> {
    let (resolved, _, _) = resolver.resolve_module_name(name, source_file, mode, None);
    resolved
        .is_resolved()
        .then(|| resolved.resolved_file_name.clone())
}

// Go: module/resolver_test.go:128 TestResolveDynamicModuleNameUsingRootDirs (ts#64544)
#[test]
fn test_resolve_dynamic_module_name_using_root_dirs() {
    let mut t = Subtests::new("TestResolveDynamicModuleNameUsingRootDirs");
    for (name, target_root, target_file) in [
        (
            "dynamic roots",
            "^/~ts-uri~/custom/ts-nul-authority/generated",
            "^/~ts-uri~/custom/ts-nul-authority/generated/~ts-uri-escape~7e74732d7572692d6573636170657e66696c65~.ts",
        ),
        (
            "dynamic to disk",
            "c:/generated",
            "c:/generated/~ts-uri-escape~file.ts",
        ),
    ] {
        t.run(name, || {
            const SOURCE_FILE: &str = "^/~ts-uri~/custom/ts-nul-authority/src/main.ts";
            let resolver = new_dynamic_resolver(
                &[(SOURCE_FILE, ""), (target_file, "export const value = 1;")],
                bundler_options_with_root_dirs(&[
                    "^/~ts-uri~/custom/ts-nul-authority/src",
                    target_root,
                ]),
            );
            let resolved = resolved_file(
                &resolver,
                "./~ts-uri-escape~file",
                SOURCE_FILE,
                ModuleKind::ES_NEXT,
            );
            if resolved.as_deref() != Some(target_file) {
                return Err(format!(
                    "resolved file = {resolved:?}, expected {target_file:?}"
                ));
            }
            Ok(())
        });
    }
    t.finish();
}

// Go: module/resolver_test.go:183 TestRootDirsPreservesExceptionalDynamicSegments (ts#64544)
#[test]
fn test_root_dirs_preserves_exceptional_dynamic_segments() {
    const SOURCE_FILE: &str = "^/~ts-uri~/custom/ts-nul-authority/src/~ts-uri-escape~2e2e~/main.ts";
    const TARGET_FILE: &str =
        "^/~ts-uri~/custom/ts-nul-authority/generated/~ts-uri-escape~2e2e~/dep.ts";
    let resolver = new_dynamic_resolver(
        &[
            (SOURCE_FILE, ""),
            (TARGET_FILE, "export const value = 1;"),
            (
                "^/~ts-uri~/custom/ts-nul-authority/dep.ts",
                "export const wrong = 1;",
            ),
        ],
        bundler_options_with_root_dirs(&[
            "^/~ts-uri~/custom/ts-nul-authority/src",
            "^/~ts-uri~/custom/ts-nul-authority/generated",
        ]),
    );
    let resolved = resolved_file(&resolver, "./dep", SOURCE_FILE, ModuleKind::ES_NEXT);
    assert_eq!(resolved.as_deref(), Some(TARGET_FILE));
}

// Go: module/resolver_test.go:218 TestRootDirsRejectsUnrepresentableDiskSegments (ts#64544)
#[test]
fn test_root_dirs_rejects_unrepresentable_disk_segments() {
    let mut t = Subtests::new("TestRootDirsRejectsUnrepresentableDiskSegments");
    for source_file in [
        "^/~ts-uri~/custom/ts-nul-authority/src/~ts-uri-escape~2e2e~/main.ts",
        "^/~ts-uri~/custom/ts-nul-authority/src/c:/main.ts",
        "^/~ts-uri~/custom/ts-nul-authority/src/^/main.ts",
    ] {
        t.run(source_file, || {
            let resolver = new_dynamic_resolver(
                &[(source_file, ""), ("c:/dep.ts", "export const wrong = 1;")],
                bundler_options_with_root_dirs(&[
                    "^/~ts-uri~/custom/ts-nul-authority/src",
                    "c:/generated",
                ]),
            );
            if let Some(resolved) =
                resolved_file(&resolver, "./dep", source_file, ModuleKind::ES_NEXT)
            {
                return Err(format!(
                    "unexpectedly resolved unrepresentable disk path to {resolved:?}"
                ));
            }
            Ok(())
        });
    }
    t.finish();
}

// Go: module/resolver_test.go:465 TestResolveDynamicPackageSubpathFile (ts#64544)
#[test]
fn test_resolve_dynamic_package_subpath_file() {
    const SOURCE_FILE: &str = "^/~ts-uri~/custom/ts-nul-authority/src/main.ts";
    const TARGET_FILE: &str = "^/~ts-uri~/custom/ts-nul-authority/node_modules/pkg/~ts-uri-escape~7e74732d7572692d6573636170657e3636366636667e~.ts";
    let resolver = new_dynamic_resolver(
        &[
            (SOURCE_FILE, ""),
            (
                "^/~ts-uri~/custom/ts-nul-authority/node_modules/pkg/package.json",
                r#"{"name":"pkg"}"#,
            ),
            (TARGET_FILE, "export const value = 1;"),
        ],
        (*bundler_options()).clone(),
    );
    let resolved = resolved_file(
        &resolver,
        "pkg/~ts-uri-escape~666f6f~.ts",
        SOURCE_FILE,
        ModuleKind::ES_NEXT,
    );
    assert_eq!(resolved.as_deref(), Some(TARGET_FILE));
}

// Go: module/resolver_test.go:529 TestResolveDynamicDottedDirectory (ts#64544)
#[test]
fn test_resolve_dynamic_dotted_directory() {
    const SOURCE_FILE: &str = "^/~ts-uri~/custom/ts-nul-authority/src/main.ts";
    const TARGET_FILE: &str = "^/~ts-uri~/custom/ts-nul-authority/src/~ts-uri-escape~7e74732d7572692d6573636170657e6469722e6a73~/index.ts";
    let resolver = new_dynamic_resolver(
        &[(SOURCE_FILE, ""), (TARGET_FILE, "export const value = 1;")],
        CompilerOptions {
            module: ModuleKind::COMMON_JS,
            ..(*bundler_options()).clone()
        },
    );
    let resolved = resolved_file(
        &resolver,
        "./~ts-uri-escape~dir.js",
        SOURCE_FILE,
        ModuleKind::COMMON_JS,
    );
    assert_eq!(resolved.as_deref(), Some(TARGET_FILE));
}

// Go: module/resolver_test.go:427 TestResolveDynamicPackageJSONPath (ts#64544)
#[test]
fn test_resolve_dynamic_package_json_path() {
    const SOURCE_FILE: &str = "^/~ts-uri~/custom/ts-nul-authority/src/main.ts";
    const FALLBACK_FILE: &str = "^/~ts-uri~/custom/ts-nul-authority/node_modules/pkg/~ts-uri-escape~7e74732d7572692d6573636170657e7479706573~.d.ts";
    const TARGET_FILE: &str = "^/~ts-uri~/custom/ts-nul-authority/node_modules/pkg/ts3.1/~ts-uri-escape~7e74732d7572692d6573636170657e7479706573~.d.ts";
    let resolver = new_dynamic_resolver(
        &[
            (SOURCE_FILE, ""),
            (
                "^/~ts-uri~/custom/ts-nul-authority/node_modules/pkg/package.json",
                r#"{"name":"pkg","types":"~ts-uri-escape~types.d.ts","typesVersions":{"*":{"*":["ts3.1/*"]}}}"#,
            ),
            (FALLBACK_FILE, "export const fallback: number;"),
            (TARGET_FILE, "export const value: number;"),
        ],
        (*bundler_options()).clone(),
    );
    let resolved = resolved_file(&resolver, "pkg", SOURCE_FILE, ModuleKind::ES_NEXT);
    assert_eq!(resolved.as_deref(), Some(TARGET_FILE));
}

// Go: module/resolver_test.go:496 TestResolveDynamicESMPackageIndexFromReservedDirectory (ts#64544)
#[test]
fn test_resolve_dynamic_esm_package_index_from_reserved_directory() {
    const SOURCE_FILE: &str = "^/~ts-uri~/custom/ts-nul-authority/src/main.ts";
    const PACKAGE_NAME: &str = "~ts-uri-escape~pkg.js";
    const PACKAGE_DIRECTORY: &str = "^/~ts-uri~/custom/ts-nul-authority/node_modules/~ts-uri-escape~7e74732d7572692d6573636170657e706b672e6a73~";
    let package_json_file = format!("{PACKAGE_DIRECTORY}/package.json");
    let package_json = format!(r#"{{"name":"{PACKAGE_NAME}"}}"#);
    let target_file = format!("{PACKAGE_DIRECTORY}/index.js");
    let resolver = new_dynamic_resolver(
        &[
            (SOURCE_FILE, ""),
            (&package_json_file, &package_json),
            (&target_file, "exports.value = 1;"),
        ],
        (*bundler_options()).clone(),
    );
    let resolved = resolved_file(&resolver, PACKAGE_NAME, SOURCE_FILE, ModuleKind::ES_NEXT);
    assert_eq!(resolved.as_deref(), Some(target_file.as_str()));
}

// Go: module/resolver_test.go:742 TestGeneratedDynamicEntrypointSpecifierResolvesEncodedFile (ts#64544)
#[test]
fn test_generated_dynamic_entrypoint_specifier_resolves_encoded_file() {
    const SOURCE_FILE: &str = "^/~ts-uri~/custom/ts-nul-authority/src/main.ts";
    const PACKAGE_FILE: &str = "^/~ts-uri~/custom/ts-nul-authority/node_modules/Pkg/~ts-uri-escape~7e74732d7572692d6573636170657e76616c7565~.d.ts";
    const MODULE_SPECIFIER: &str =
        "Pkg/~ts-uri-spec~7e74732d7572692d6573636170657e76616c7565~.d.ts";
    let resolver = new_dynamic_resolver(
        &[
            (SOURCE_FILE, ""),
            (
                "^/~ts-uri~/custom/ts-nul-authority/node_modules/Pkg/package.json",
                r#"{"name":"Pkg"}"#,
            ),
            (PACKAGE_FILE, "export const value: number;"),
        ],
        (*bundler_options()).clone(),
    );

    let _ = resolver.resolve_module_name("Pkg", SOURCE_FILE, ModuleKind::ES_NEXT, None);
    // PORT: Go takes the one package.json entry that exists from
    // `PackageJsonCacheEntries`. The Rust cache range gives no
    // `InfoCacheEntry`; the package scope of the package directory is that
    // entry.
    let package_json = resolver
        .get_package_scope_for_path("^/~ts-uri~/custom/ts-nul-authority/node_modules/Pkg/")
        .filter(|entry| entry.exists());
    assert!(package_json.is_some(), "expected package JSON cache entry");
    let entrypoints = resolver.get_entrypoints_from_package_json_info(&package_json, "Pkg", true);
    assert!(
        entrypoints.len() == 1 && entrypoints[0].module_specifier == MODULE_SPECIFIER,
        "entrypoints = {:?}, expected {MODULE_SPECIFIER:?}",
        entrypoints
            .iter()
            .map(|e| e.module_specifier.clone())
            .collect::<Vec<_>>()
    );

    let resolved = resolved_file(
        &resolver,
        MODULE_SPECIFIER,
        SOURCE_FILE,
        ModuleKind::ES_NEXT,
    );
    assert_eq!(resolved.as_deref(), Some(PACKAGE_FILE));
}

// Go: module/resolver_internal_test.go:11 TestNormalizePathForCJSResolutionPreservesDirectoryIntent (ts#64159)
// PORT: Go returns a resolution candidate; the Rust result is its text
// (`AsString()`: a directory candidate ends with "/").
#[test]
fn test_normalize_path_for_cjs_resolution_preserves_directory_intent() {
    let mut t = Subtests::new("TestNormalizePathForCJSResolutionPreservesDirectoryIntent");
    #[rustfmt::skip]
    let tests: &[(&str, &str, &str, &str)] = &[
        ("file", "/project", "file", "/project/file"),
        ("trailing separator", "/project", "directory/", "/project/directory/"),
        ("current directory", "/project", ".", "/project/"),
        ("current directory with backslash", "/project", ".\\", "/project/"),
        ("nested parent directory", "/project/src", "../lib/", "/project/lib/"),
        ("nested parent directory with backslashes", "/project/src", "..\\lib\\", "/project/lib/"),
        ("posix root", "/project", "..", "/"),
        ("drive root", "c:/project", "..", "c:/"),
        ("drive root without separator", "/project", "c:", "c:/"),
        ("UNC root without separator", "/project", "//server", "//server/"),
        ("URL root", "file:///project", "..", "file:///"),
        ("URL authority root without separator", "/project", "file://server", "file://server/"),
        ("dynamic reserved prefix", "^/~ts-uri~/custom/ts-nul-authority/folder", "./~ts-uri-escape~file", "^/~ts-uri~/custom/ts-nul-authority/folder/~ts-uri-escape~7e74732d7572692d6573636170657e66696c65~"),
        ("dynamic reserved prefix with extension", "^/~ts-uri~/custom/ts-nul-authority/folder", "./~ts-uri-escape~file.ts", "^/~ts-uri~/custom/ts-nul-authority/folder/~ts-uri-escape~7e74732d7572692d6573636170657e66696c65~.ts"),
        ("dynamic reserved directory prefix", "^/~ts-uri~/custom/ts-nul-authority/folder", "./~ts-uri-escape~dir/file", "^/~ts-uri~/custom/ts-nul-authority/folder/~ts-uri-escape~7e74732d7572692d6573636170657e646972~/file"),
        ("dynamic dotted directory intent", "^/~ts-uri~/custom/ts-nul-authority/folder", "./~ts-uri-escape~dir.js/", "^/~ts-uri~/custom/ts-nul-authority/folder/~ts-uri-escape~7e74732d7572692d6573636170657e6469722e6a73~/"),
    ];
    for &(name, containing_directory, module_name, text) in tests {
        t.run(name, || {
            let got = normalize_path_for_cjs_resolution(containing_directory, module_name);
            if got != text {
                return Err(format!("AsString() = {got:?}, expected {text:?}"));
            }
            Ok(())
        });
    }
    t.finish();
}

// Go: module/resolver_test.go:1224 TestResolveModuleNameExportTargetWithTrailingSlashDoesNotResolveAsFile (ts#64159)
#[test]
fn test_resolve_module_name_export_target_with_trailing_slash_does_not_resolve_as_file() {
    let fs = vfstest::from_map(
        [
            (
                "/repo/node_modules/pkg/package.json",
                r#"{"name":"pkg","exports":{".":"./index.d.ts/"}}"#,
            ),
            (
                "/repo/node_modules/pkg/index.d.ts",
                "export const x: number;",
            ),
            ("/repo/src/file.ts", ""),
        ],
        true,
    );
    let resolver = new_repo_resolver(fs);
    let (resolved, _, _) =
        resolver.resolve_module_name("pkg", "/repo/src/file.ts", ModuleKind::ES_NEXT, None);
    assert!(
        !resolved.is_resolved(),
        "expected directory-only export target to remain unresolved, got {:?}",
        resolved.resolved_file_name
    );
}

// ---------------------------------------------------------------------------
// ts#64159 resolver tests (config part 3)
// ---------------------------------------------------------------------------

/// Go `NewResolver(ResolverOptions{Host: &resolutionTestInputs{fs: fs, cwd:
/// cwd}, CompilerOptions: opts})` over `vfstest.FromMap(files,
/// caseSensitivity)`.
fn new_test_resolver(
    files: &[(&str, &str)],
    use_case_sensitive_file_names: bool,
    cwd: &str,
    options: CompilerOptions,
) -> DefaultResolver {
    let fs = vfstest::from_map(files.iter().copied(), use_case_sensitive_file_names);
    new_resolver_over(fs, cwd, options)
}

/// `new_test_resolver` over a given FS.
fn new_resolver_over(fs: Rc<dyn Fs>, cwd: &str, options: CompilerOptions) -> DefaultResolver {
    let host: Rc<dyn ResolutionHost> = Rc::new(ResolutionHostStub {
        fs,
        cwd: cwd.to_string(),
    });
    new_resolver(ResolverOptions {
        host: Some(host),
        compiler_options: Some(Rc::new(options)),
        ..Default::default()
    })
}

/// Go: the one package.json entry that exists in `PackageJsonCacheEntries`.
/// PORT: the package scope of `package_directory`, as in
/// `test_generated_dynamic_entrypoint_specifier_resolves_encoded_file`.
fn package_json_entry(
    resolver: &DefaultResolver,
    package_directory: &str,
) -> Option<Rc<ts_goport::frontend::packagejson::InfoCacheEntry>> {
    resolver
        .get_package_scope_for_path(&format!("{package_directory}/"))
        .filter(|entry| entry.exists())
}

/// The module specifiers of `GetEntrypointsFromPackageJsonInfo`.
fn entrypoint_specifiers(
    resolver: &DefaultResolver,
    package_directory: &str,
    package_name: &str,
    enable_directory_search: bool,
) -> Result<Vec<String>, String> {
    let package_json = package_json_entry(resolver, package_directory);
    if package_json.is_none() {
        return Err("expected package JSON cache entry".to_string());
    }
    Ok(resolver
        .get_entrypoints_from_package_json_info(
            &package_json,
            package_name,
            enable_directory_search,
        )
        .iter()
        .map(|e| e.module_specifier.clone())
        .collect())
}

// Go: module/resolver_test.go:258 TestRootDirsEscapesRootLikeDynamicSegments (ts#64159)
#[test]
fn test_root_dirs_escapes_root_like_dynamic_segments() {
    let mut t = Subtests::new("TestRootDirsEscapesRootLikeDynamicSegments");
    for (source_file, target_file) in [
        (
            "^/~ts-uri~/custom/ts-nul-authority/src/c:/main.ts",
            "^/~ts-uri~/custom/ts-nul-authority/generated/~ts-uri-escape~633a~/dep.ts",
        ),
        (
            "^/~ts-uri~/custom/ts-nul-authority/src/^/main.ts",
            "^/~ts-uri~/custom/ts-nul-authority/generated/~ts-uri-escape~5e~/dep.ts",
        ),
    ] {
        t.run(source_file, || {
            let resolver = new_test_resolver(
                &[
                    (source_file, ""),
                    (target_file, "export const value = 1;"),
                    ("c:/dep.ts", "export const wrong = 1;"),
                    ("^/dep.ts", "export const wrong = 1;"),
                ],
                true,
                DYNAMIC_ROOT,
                bundler_options_with_root_dirs(&[
                    "^/~ts-uri~/custom/ts-nul-authority/src",
                    "^/~ts-uri~/custom/ts-nul-authority/generated",
                ]),
            );
            let resolved = resolved_file(&resolver, "./dep", source_file, ModuleKind::ES_NEXT);
            if resolved.as_deref() != Some(target_file) {
                return Err(format!(
                    "resolved file = {resolved:?}, expected {target_file:?}"
                ));
            }
            Ok(())
        });
    }
    t.finish();
}

// Go: module/resolver_test.go:303 TestRootDirsPreservesRelativeDiskSuffixes (ts#64159)
#[test]
fn test_root_dirs_preserves_relative_disk_suffixes() {
    let mut t = Subtests::new("TestRootDirsPreservesRelativeDiskSuffixes");
    for (source_file, target_root, target_file) in [
        (
            "c:/src/^/main.ts",
            "^/~ts-uri~/custom/ts-nul-authority/generated",
            "^/~ts-uri~/custom/ts-nul-authority/generated/~ts-uri-escape~5e~/dep.ts",
        ),
        (
            "c:/src/c:/main.ts",
            "d:/generated",
            "d:/generated/c:/dep.ts",
        ),
    ] {
        t.run(source_file, || {
            let resolver = new_test_resolver(
                &[
                    (source_file, ""),
                    (target_file, "export const value = 1;"),
                    ("^/dep.ts", "export const wrong = 1;"),
                    ("c:/dep.ts", "export const wrong = 1;"),
                ],
                true,
                "c:/",
                bundler_options_with_root_dirs(&["c:/src", target_root]),
            );
            let resolved = resolved_file(&resolver, "./dep", source_file, ModuleKind::ES_NEXT);
            if resolved.as_deref() != Some(target_file) {
                return Err(format!(
                    "resolved file = {resolved:?}, expected {target_file:?}"
                ));
            }
            Ok(())
        });
    }
    t.finish();
}

// Go: module/resolver_test.go:351 TestRootDirsPreservesRelativeDirectoryIntent (ts#64159)
#[test]
fn test_root_dirs_preserves_relative_directory_intent() {
    let mut t = Subtests::new("TestRootDirsPreservesRelativeDirectoryIntent");
    #[rustfmt::skip]
    let tests: &[(&str, &str, &str, &str, &str)] = &[
        ("disk to disk", "c:/src", "c:/src/^/main.ts", "d:/generated", "d:/generated/^/pkg/index.ts"),
        ("disk to dynamic", "c:/src", "c:/src/^/main.ts", "^/~ts-uri~/custom/ts-nul-authority/generated", "^/~ts-uri~/custom/ts-nul-authority/generated/~ts-uri-escape~5e~/pkg/index.ts"),
        ("empty suffix to dynamic", "c:/src", "c:/src/main.ts", "^/~ts-uri~/custom/ts-nul-authority/generated", "^/~ts-uri~/custom/ts-nul-authority/generated/index.ts"),
        ("empty suffix to disk", "^/~ts-uri~/custom/ts-nul-authority/src", "^/~ts-uri~/custom/ts-nul-authority/src/main.ts", "d:/generated", "d:/generated/index.ts"),
        ("literal empty segment to dynamic", "^/~ts-uri~/custom/ts-nul-authority/src", "^/~ts-uri~/custom/ts-nul-authority/src/~ts-uri-escape~~/main.ts", "^/~ts-uri~/custom/ts-nul-authority/generated", "^/~ts-uri~/custom/ts-nul-authority/generated/~ts-uri-escape~~/index.ts"),
    ];
    for &(name, source_root, source_file, target_root, target_file) in tests {
        t.run(name, || {
            let resolver = new_test_resolver(
                &[
                    (source_file, ""),
                    (target_file, "export const value = 1;"),
                    ("^/pkg/index.ts", "export const wrong = 1;"),
                    ("d:/generated.ts", "export const wrong = 1;"),
                ],
                true,
                "c:/",
                CompilerOptions {
                    module: ModuleKind::COMMON_JS,
                    ..bundler_options_with_root_dirs(&[source_root, target_root])
                },
            );
            let module_name =
                if name.contains("empty suffix") || name == "literal empty segment to dynamic" {
                    "./"
                } else {
                    "./pkg/"
                };
            let resolved =
                resolved_file(&resolver, module_name, source_file, ModuleKind::COMMON_JS);
            if resolved.as_deref() != Some(target_file) {
                return Err(format!(
                    "resolved file = {resolved:?}, expected {target_file:?}"
                ));
            }
            Ok(())
        });
    }
    t.finish();
}

// Go: module/resolver_test.go:661 TestDynamicExportPatternEntrypointUsesLogicalName (ts#64159)
#[test]
fn test_dynamic_export_pattern_entrypoint_uses_logical_name() {
    const SOURCE_FILE: &str = "^/~ts-uri~/custom/ts-nul-authority/src/main.ts";
    const PACKAGE_DIRECTORY: &str = "^/~ts-uri~/custom/ts-nul-authority/node_modules/Pkg";
    let mut t = Subtests::new("TestDynamicExportPatternEntrypointUsesLogicalName");
    #[rustfmt::skip]
    let tests: &[(&str, &str, &str, &str)] = &[
        ("logical wildcard match", r#"{"name":"Pkg","exports":{"./*":"./*.d.ts"}}"#, "Pkg/~ts-uri-escape~value", "Pkg/~ts-uri-spec~7e74732d7572692d6573636170657e76616c7565~"),
        ("reserved target pattern", r#"{"name":"Pkg","exports":{"./*":"./~ts-uri-escape~*.d.ts"}}"#, "Pkg/value", "Pkg/value"),
        ("reserved wildcard with trailer", r#"{"name":"Pkg","exports":{"./*":"./*-impl.d.ts"}}"#, "Pkg/~ts-uri-escape~value", "Pkg/~ts-uri-spec~7e74732d7572692d6573636170657e76616c7565~"),
    ];
    for &(name, package_json, import_name, module_specifier) in tests {
        t.run(name, || {
            let target_file = if name == "reserved wildcard with trailer" {
                "^/~ts-uri~/custom/ts-nul-authority/node_modules/Pkg/~ts-uri-escape~7e74732d7572692d6573636170657e76616c75652d696d706c~.d.ts"
            } else {
                "^/~ts-uri~/custom/ts-nul-authority/node_modules/Pkg/~ts-uri-escape~7e74732d7572692d6573636170657e76616c7565~.d.ts"
            };
            let package_json_file = format!("{PACKAGE_DIRECTORY}/package.json");
            let resolver = new_dynamic_resolver(
                &[
                    (SOURCE_FILE, ""),
                    (&package_json_file, package_json),
                    (target_file, "export const value: number;"),
                ],
                (*bundler_options()).clone(),
            );
            if resolved_file(&resolver, import_name, SOURCE_FILE, ModuleKind::ES_NEXT).is_none() {
                return Err("expected dynamic package export to resolve".to_string());
            }
            let entrypoints = entrypoint_specifiers(&resolver, PACKAGE_DIRECTORY, "Pkg", false)?;
            if entrypoints.len() != 1 {
                return Err(format!(
                    "entrypoint count = {}, expected 1",
                    entrypoints.len()
                ));
            }
            if entrypoints[0] != module_specifier {
                return Err(format!(
                    "module specifier = {:?}, expected {module_specifier:?}",
                    entrypoints[0]
                ));
            }
            Ok(())
        });
    }
    t.finish();
}

// Go: module/resolver_test.go:796 TestDynamicEntrypointsEncodePhysicalPackageNameAndRootLikePath (ts#64159)
#[test]
fn test_dynamic_entrypoints_encode_physical_package_name_and_root_like_path() {
    const SOURCE_FILE: &str = "^/~ts-uri~/custom/ts-nul-authority/src/main.ts";
    const PHYSICAL_PACKAGE_NAME: &str =
        "~ts-uri-escape~7e74732d7572692d6573636170657e706b672e6a73~";
    const SOURCE_PACKAGE_NAME: &str = "~ts-uri-spec~7e74732d7572692d6573636170657e706b672e6a73~";
    let package_directory =
        format!("^/~ts-uri~/custom/ts-nul-authority/node_modules/{PHYSICAL_PACKAGE_NAME}");
    let target_file = format!("{package_directory}/~ts-uri-escape~633a~/value.d.ts");
    let module_specifier = format!("{SOURCE_PACKAGE_NAME}/~ts-uri-spec~633a~/value.d.ts");
    let package_json_file = format!("{package_directory}/package.json");
    let resolver = new_dynamic_resolver(
        &[
            (SOURCE_FILE, ""),
            (&package_json_file, r#"{"name":"~ts-uri-escape~pkg.js"}"#),
            (&target_file, "export const value: number;"),
        ],
        (*bundler_options()).clone(),
    );
    let _ =
        resolver.resolve_module_name(SOURCE_PACKAGE_NAME, SOURCE_FILE, ModuleKind::ES_NEXT, None);
    let entrypoints =
        entrypoint_specifiers(&resolver, &package_directory, PHYSICAL_PACKAGE_NAME, true).unwrap();
    assert!(
        entrypoints.len() == 1 && entrypoints[0] == module_specifier,
        "entrypoints = {entrypoints:?}, expected {module_specifier:?}"
    );
    let resolved = resolved_file(
        &resolver,
        &module_specifier,
        SOURCE_FILE,
        ModuleKind::ES_NEXT,
    );
    assert_eq!(
        resolved.as_deref(),
        Some(target_file.as_str()),
        "generated module specifier resolved to {resolved:?}"
    );
}

/// The shared body of the tests that resolve `module_specifier` in a
/// dynamic package `Pkg` with `package_json`, then expect one entrypoint
/// whose specifier is `module_specifier` (Go resolver_test.go:852, :900,
/// :1032).
fn assert_single_dynamic_entrypoint(
    source_file: &str,
    package_directory: &str,
    package_json: &str,
    target_file: &str,
    module_specifier: &str,
) {
    let package_json_file = format!("{package_directory}/package.json");
    let resolver = new_dynamic_resolver(
        &[
            (source_file, ""),
            (&package_json_file, package_json),
            (target_file, "export const value: number;"),
        ],
        (*bundler_options()).clone(),
    );
    assert!(
        resolved_file(
            &resolver,
            module_specifier,
            source_file,
            ModuleKind::ES_NEXT
        )
        .is_some(),
        "expected {module_specifier:?} to resolve"
    );
    let entrypoints = entrypoint_specifiers(&resolver, package_directory, "Pkg", false).unwrap();
    assert!(
        entrypoints.len() == 1 && entrypoints[0] == module_specifier,
        "entrypoints = {entrypoints:?}, expected {module_specifier:?}"
    );
}

// Go: module/resolver_test.go:852 TestDynamicExportPatternPreservesStaticSourcePrefix (ts#64159)
#[test]
fn test_dynamic_export_pattern_preserves_static_source_prefix() {
    const PACKAGE_DIRECTORY: &str = "^/~ts-uri~/custom/ts-nul-authority/node_modules/Pkg";
    assert_single_dynamic_entrypoint(
        "^/~ts-uri~/custom/ts-nul-authority/src/main.ts",
        PACKAGE_DIRECTORY,
        r#"{"name":"Pkg","exports":{"./~ts-uri-escape~prefix/*":"./*.d.ts"}}"#,
        &format!("{PACKAGE_DIRECTORY}/foo.d.ts"),
        "Pkg/~ts-uri-escape~prefix/foo",
    );
}

// Go: module/resolver_test.go:900 TestDynamicExportPatternKeepsPackageForRootLikeMatch (ts#64159)
#[test]
fn test_dynamic_export_pattern_keeps_package_for_root_like_match() {
    const PACKAGE_DIRECTORY: &str = "^/~ts-uri~/custom/ts-nul-authority/node_modules/Pkg";
    assert_single_dynamic_entrypoint(
        "^/~ts-uri~/custom/ts-nul-authority/src/main.ts",
        PACKAGE_DIRECTORY,
        r#"{"name":"Pkg","exports":{"./*":"./*.d.ts"}}"#,
        &format!("{PACKAGE_DIRECTORY}/~ts-uri-escape~633a~/value.d.ts"),
        "Pkg/~ts-uri-spec~633a~/value",
    );
}

// Go: module/resolver_test.go:1032 TestDynamicExportPatternUnderExceptionalParent (ts#64159)
#[test]
fn test_dynamic_export_pattern_under_exceptional_parent() {
    const PACKAGE_DIRECTORY: &str =
        "^/~ts-uri~/custom/ts-nul-authority/work/~ts-uri-escape~2e2e~/node_modules/Pkg";
    assert_single_dynamic_entrypoint(
        "^/~ts-uri~/custom/ts-nul-authority/work/~ts-uri-escape~2e2e~/src/main.ts",
        PACKAGE_DIRECTORY,
        r#"{"name":"Pkg","exports":{"./*":"./*.d.ts"}}"#,
        &format!("{PACKAGE_DIRECTORY}/foo.d.ts"),
        "Pkg/foo",
    );
}

// Go: module/resolver_test.go:948 TestDynamicEntrypointsPreserveCaseDistinctFiles (ts#64159)
#[test]
fn test_dynamic_entrypoints_preserve_case_distinct_files() {
    const SOURCE_FILE: &str = "^/~ts-uri~/custom/ts-nul-authority/src/main.ts";
    const PACKAGE_DIRECTORY: &str = "^/~ts-uri~/custom/ts-nul-authority/node_modules/Pkg";
    // Go caseInsensitiveViewFS (resolver_test.go:34): a case-sensitive map
    // that reports a case-insensitive file system.
    let fs = wrapvfs_wrap(
        vfstest::from_map(
            [
                (SOURCE_FILE, ""),
                (
                    "^/~ts-uri~/custom/ts-nul-authority/node_modules/Pkg/package.json",
                    r#"{"name":"Pkg","types":"Foo.d.ts"}"#,
                ),
                (
                    "^/~ts-uri~/custom/ts-nul-authority/node_modules/Pkg/Foo.d.ts",
                    "export const upper: number;",
                ),
                (
                    "^/~ts-uri~/custom/ts-nul-authority/node_modules/Pkg/foo.d.ts",
                    "export const lower: number;",
                ),
            ],
            true,
        ),
        Replacements {
            use_case_sensitive_file_names: Some(Box::new(|| false)),
            ..Default::default()
        },
    );
    let resolver = new_resolver_over(fs, DYNAMIC_ROOT, (*bundler_options()).clone());
    let _ = resolver.resolve_module_name("Pkg", SOURCE_FILE, ModuleKind::ES_NEXT, None);
    let entrypoints = entrypoint_specifiers(&resolver, PACKAGE_DIRECTORY, "Pkg", true).unwrap();
    assert_eq!(entrypoints.len(), 2, "entrypoints = {entrypoints:?}");
}

// Go: module/resolver_test.go:992 TestDiskExportPatternKeepsReservedPrefixLiteral (ts#64159)
#[test]
fn test_disk_export_pattern_keeps_reserved_prefix_literal() {
    let resolver = new_test_resolver(
        &[
            ("/repo/src/main.ts", ""),
            (
                "/repo/node_modules/pkg/package.json",
                r#"{"name":"pkg","exports":{"./*":"./*.d.ts"}}"#,
            ),
            (
                "/repo/node_modules/pkg/~ts-uri-escape~666f6f~.d.ts",
                "export const value: number;",
            ),
        ],
        true,
        "/repo",
        (*bundler_options()).clone(),
    );
    assert!(
        resolved_file(
            &resolver,
            "pkg/~ts-uri-escape~666f6f~",
            "/repo/src/main.ts",
            ModuleKind::ES_NEXT
        )
        .is_some(),
        "expected disk package export to resolve"
    );
    let entrypoints =
        entrypoint_specifiers(&resolver, "/repo/node_modules/pkg", "pkg", false).unwrap();
    assert!(
        entrypoints.len() == 1 && entrypoints[0] == "pkg/~ts-uri-escape~666f6f~",
        "entrypoints = {entrypoints:?}, expected literal reserved-prefix entrypoint"
    );
}

// Go: module/resolver_test.go:1081 TestDynamicExportPatternWithEmptyMatch (ts#64159)
#[test]
fn test_dynamic_export_pattern_with_empty_match() {
    const SOURCE_FILE: &str = "^/~ts-uri~/custom/ts-nul-authority/src/main.ts";
    const PACKAGE_DIRECTORY: &str = "^/~ts-uri~/custom/ts-nul-authority/node_modules/Pkg";
    const TARGET_FILE: &str = "^/~ts-uri~/custom/ts-nul-authority/node_modules/Pkg/index.d.ts";
    let resolver = new_dynamic_resolver(
        &[
            (SOURCE_FILE, ""),
            (
                "^/~ts-uri~/custom/ts-nul-authority/node_modules/Pkg/package.json",
                r#"{"name":"Pkg","exports":{"./foo/*":"./index*.d.ts"}}"#,
            ),
            (TARGET_FILE, "export const value: number;"),
        ],
        (*bundler_options()).clone(),
    );
    assert!(
        resolved_file(&resolver, "Pkg/foo/", SOURCE_FILE, ModuleKind::ES_NEXT).is_some(),
        "expected empty wildcard package export to resolve"
    );
    let entrypoints = entrypoint_specifiers(&resolver, PACKAGE_DIRECTORY, "Pkg", false).unwrap();
    assert_eq!(entrypoints.len(), 1, "entrypoints = {entrypoints:?}");
    let generated = &entrypoints[0];
    let resolved = resolved_file(&resolver, generated, SOURCE_FILE, ModuleKind::ES_NEXT);
    assert_eq!(
        resolved.as_deref(),
        Some(TARGET_FILE),
        "generated module specifier {generated:?} resolved to {resolved:?}"
    );
}

// Go: module/resolver_test.go:1138 TestUnaddressableEmptyExportPatternIsNotAnEntrypoint (ts#64159)
#[test]
fn test_unaddressable_empty_export_pattern_is_not_an_entrypoint() {
    const SOURCE_FILE: &str = "^/~ts-uri~/custom/ts-nul-authority/src/main.ts";
    const PACKAGE_DIRECTORY: &str = "^/~ts-uri~/custom/ts-nul-authority/node_modules/Pkg";
    let resolver = new_dynamic_resolver(
        &[
            (SOURCE_FILE, ""),
            (
                "^/~ts-uri~/custom/ts-nul-authority/node_modules/Pkg/package.json",
                r#"{"name":"Pkg","exports":{"./*":"./index*.d.ts"}}"#,
            ),
            (
                "^/~ts-uri~/custom/ts-nul-authority/node_modules/Pkg/index.d.ts",
                "export const value: number;",
            ),
        ],
        (*bundler_options()).clone(),
    );
    let _ = resolver.resolve_module_name("Pkg", SOURCE_FILE, ModuleKind::ES_NEXT, None);
    let entrypoints = entrypoint_specifiers(&resolver, PACKAGE_DIRECTORY, "Pkg", false).unwrap();
    assert!(
        entrypoints.is_empty(),
        "entrypoints = {entrypoints:?}, expected unaddressable empty wildcard match to be omitted"
    );
}

// Go: module/resolver_test.go:1200 TestResolveModuleNameBlockedByNullExport (ts#64159)
// PORT: the port keeps no `ResolvedPath` (the key is `tspath::to_path` of
// the resolved file name), so only the resolution is checked.
#[test]
fn test_resolve_module_name_blocked_by_null_export() {
    let resolver = new_test_resolver(
        &[
            (
                "/repo/node_modules/pkg/package.json",
                r#"{"name":"pkg","exports":{".":null}}"#,
            ),
            ("/repo/src/file.ts", ""),
        ],
        true,
        "/repo",
        (*bundler_options()).clone(),
    );
    let resolved = resolved_file(&resolver, "pkg", "/repo/src/file.ts", ModuleKind::ES_NEXT);
    assert_eq!(resolved, None, "expected null export to remain unresolved");
}

/// A second resolution that a wrapped FS runs inside a `FileExists` call.
type Nested = Rc<RefCell<Option<Box<dyn FnOnce()>>>>;

// Go: module/resolver_test.go:141 TestResolveModuleNameTrailingSlashRace
/// Regression test for https://github.com/microsoft/typescript-go/issues/3526.
///
/// Two goroutines resolve the same package via specifiers that differ only by
/// a trailing slash (`pkg` and `pkg/`). A blocking FS holds both at the
/// `FileExists` check for `package.json` — *after* each has confirmed a
/// `package.json` info-cache miss but *before* either has called `Set`. When
/// released, both proceed to `LoadOrStore` and one of them loses. See the Go
/// file for the full description.
// PORT: the Rust resolver is single-threaded (`Rc`), so there are no
// goroutines. The Go interleaving is made on one thread instead: the first
// `FileExists` of `package.json` (from `pkg`) runs the whole `pkg/`
// resolution inside it. That one also misses the info cache, reaches
// `FileExists`, and sets its entry first; then the `pkg` resolution sets its
// own entry and gets the other one back, as the Go loser does.
#[test]
fn test_resolve_module_name_trailing_slash_race() {
    const PKG_JSON_PATH: &str = "/repo/node_modules/pkg/package.json";
    let files = [
        // `types` points at a file that is not discoverable through any
        // fallback path: there is no `index.*` and no `main`. The only way
        // to resolve `pkg` (or `pkg/`) is via the package.json `types` field
        // inside `loadNodeModuleFromDirectoryWorker`, which is exactly the
        // step that the bug skips when `candidate` and
        // `packageInfo.PackageDirectory` mismatch.
        (
            PKG_JSON_PATH,
            r#"{"name":"pkg","types":"./typings/index.d.ts"}"#,
        ),
        (
            "/repo/node_modules/pkg/typings/index.d.ts",
            "export const x: number;",
        ),
        // Distinct containing files so each `ResolveModuleName` call has a
        // unique module-resolution-cache key.
        ("/repo/src/a/file.ts", ""),
        ("/repo/src/b/file.ts", ""),
    ];
    let inner = vfstest::from_map(files, true);
    let nested: Nested = Rc::new(RefCell::new(None));
    let arrived = Rc::new(Cell::new(0));
    let fs = {
        let inner = Rc::clone(&inner);
        let nested = Rc::clone(&nested);
        let arrived = Rc::clone(&arrived);
        wrapvfs_wrap(
            Rc::clone(&inner),
            Replacements {
                file_exists: Some(Box::new(move |path: &str| {
                    if path == PKG_JSON_PATH {
                        arrived.set(arrived.get() + 1);
                        let next = nested.borrow_mut().take();
                        if let Some(next) = next {
                            next();
                        }
                    }
                    inner.file_exists(path)
                })),
                ..Default::default()
            },
        )
    };
    let resolver = Rc::new(new_repo_resolver(fs));

    let second: Rc<Cell<Option<bool>>> = Rc::new(Cell::new(None));
    {
        let resolver: Weak<DefaultResolver> = Rc::downgrade(&resolver);
        let second = Rc::clone(&second);
        *nested.borrow_mut() = Some(Box::new(move || {
            let resolver = resolver.upgrade().expect("resolver");
            second.set(Some(resolves(&resolver, "pkg/", "/repo/src/b/file.ts")));
        }));
    }
    let first = resolves(&resolver, "pkg", "/repo/src/a/file.ts");

    // Both resolutions reached the FileExists gate.
    assert_eq!(arrived.get(), 2, "FileExists gate arrivals");
    let mut errors = Vec::new();
    if !first {
        errors.push("\"pkg\" failed to resolve".to_string());
    }
    if second.get() != Some(true) {
        errors.push("\"pkg/\" failed to resolve".to_string());
    }
    assert!(errors.is_empty(), "{}", errors.join("\n"));
}

// Go: module/resolver_test.go:216 TestResolveSubpathNilContentsRace
/// Regression test for https://github.com/microsoft/typescript-go/issues/1290.
///
/// Two goroutines resolve `pkg/sub` concurrently. Both miss the package.json
/// info-cache for the root package directory. A `flipFileExistsFS` forces the
/// first goroutine's `FileExists` to return false (simulating the file not yet
/// being visible), so it stores a nil-Contents cache entry. The second
/// goroutine's `FileExists` returns true, but its `Set` call (`LoadOrStore`)
/// returns the first goroutine's nil-Contents entry. Without the `Exists()`
/// guard on the `typesVersions` lookup, `packageInfo.Contents.GetVersionPaths`
/// dereferences nil and panics. With the guard the nil-Contents entry is safely
/// skipped.
// PORT: single-threaded, as in the test above. The outer resolution (from
// `b`) is the Go second goroutine: its `FileExists` runs the whole inner
// resolution (from `a`, the Go first goroutine) before it returns. The inner
// one's `FileExists` returns false and it sets the nil-Contents entry first;
// then the outer one sees true, reads the file and gets that entry back from
// its `Set`, as in Go.
#[test]
fn test_resolve_subpath_nil_contents_race() {
    const ROOT_PKG_JSON: &str = "/repo/node_modules/pkg/package.json";
    let files = [
        (ROOT_PKG_JSON, r#"{"name":"pkg","version":"1.0.0"}"#),
        (
            "/repo/node_modules/pkg/sub/index.d.ts",
            "export declare const sub: number;",
        ),
        ("/repo/node_modules/pkg/sub/index.js", "exports.sub = 1;"),
        ("/repo/src/a/file.ts", ""),
        ("/repo/src/b/file.ts", ""),
    ];
    let inner = vfstest::from_map(files, true);
    let nested: Nested = Rc::new(RefCell::new(None));
    let call_count = Rc::new(Cell::new(0));
    let fs = {
        let inner = Rc::clone(&inner);
        let nested = Rc::clone(&nested);
        let call_count = Rc::clone(&call_count);
        wrapvfs_wrap(
            Rc::clone(&inner),
            Replacements {
                file_exists: Some(Box::new(move |path: &str| {
                    if path == ROOT_PKG_JSON {
                        call_count.set(call_count.get() + 1);
                        let n = call_count.get();
                        if n == 1 {
                            // The outer (Go second) caller: the inner (Go
                            // first) caller runs and finishes first.
                            let next = nested.borrow_mut().take();
                            if let Some(next) = next {
                                next();
                            }
                            return inner.file_exists(path); // second caller: file is visible
                        }
                        if n == 2 {
                            return false; // first caller: simulate "file not yet visible"
                        }
                    }
                    inner.file_exists(path)
                })),
                ..Default::default()
            },
        )
    };
    let resolver = Rc::new(new_repo_resolver(fs));

    let first: Rc<Cell<Option<bool>>> = Rc::new(Cell::new(None));
    {
        let resolver: Weak<DefaultResolver> = Rc::downgrade(&resolver);
        let first = Rc::clone(&first);
        *nested.borrow_mut() = Some(Box::new(move || {
            let resolver = resolver.upgrade().expect("resolver");
            first.set(Some(resolves(&resolver, "pkg/sub", "/repo/src/a/file.ts")));
        }));
    }
    let panicked;
    let second = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        resolves(&resolver, "pkg/sub", "/repo/src/b/file.ts")
    })) {
        Ok(resolved) => {
            panicked = false;
            resolved
        }
        Err(_) => {
            panicked = true;
            false
        }
    };

    assert!(
        !panicked,
        "resolver panicked due to nil Contents dereference in loadModuleFromSpecificNodeModulesDirectory"
    );
    assert!(call_count.get() >= 2, "both callers reached FileExists");
    assert_eq!(
        first.get(),
        Some(true),
        "\"/repo/src/a/file.ts\" failed to resolve pkg/sub"
    );
    assert!(second, "\"/repo/src/b/file.ts\" failed to resolve pkg/sub");
}

// Go: module/resolver_test.go:1495 TestNodeModulePackageRoot (ts#64544 renames
// TestParseNodeModuleFromPath)
#[test]
fn test_node_module_package_root() {
    let tests: &[(&str, &str, bool, &str)] = &[
        (
            "file in package",
            "/a/node_modules/b/lib/index.d.ts",
            false,
            "/a/node_modules/b",
        ),
        (
            "file in scoped package",
            "/a/node_modules/@scope/b/lib/index.d.ts",
            false,
            "/a/node_modules/@scope/b",
        ),
        (
            "folder subpath",
            "/a/node_modules/b/lib/File",
            true,
            "/a/node_modules/b",
        ),
        (
            "folder subpath scoped",
            "/a/node_modules/@scope/b/lib/File",
            true,
            "/a/node_modules/@scope/b",
        ),
        (
            "package root folder",
            "/a/node_modules/b",
            true,
            "/a/node_modules/b",
        ),
        (
            "scoped package root folder",
            "/a/node_modules/@scope/b",
            true,
            "/a/node_modules/@scope/b",
        ),
        (
            "package root interpreted as file",
            "/a/node_modules/b",
            false,
            "/a/node_modules/",
        ),
        (
            "scoped package root interpreted as file",
            "/a/node_modules/@scope/b",
            false,
            "/a/node_modules/@scope",
        ),
        // A bare scope directory has no package name; must not panic (https://github.com/microsoft/typescript-go/issues/4373).
        (
            "scope-only folder",
            "/a/node_modules/@scope",
            true,
            "/a/node_modules/@scope",
        ),
        (
            "types scope-only folder",
            "/a/node_modules/@types",
            true,
            "/a/node_modules/@types",
        ),
        ("not in node_modules", "/a/src/index.ts", false, ""),
    ];

    let mut t = Subtests::new("TestNodeModulePackageRoot");
    for &(name, path, is_folder, want) in tests {
        t.run(name, || {
            let got = if is_folder {
                node_module_package_root_for_directory(path)
            } else {
                node_module_package_root_for_file(path)
            };
            if got != want {
                return Err(format!(
                    "nodeModulesPackageRoot({path:?}, {is_folder}) = {got:?}, want {want:?}"
                ));
            }
            Ok(())
        });
    }
    t.finish();
}

// Go: module/resolver_test.go:339 TestResolvePeerDependencyNilContentsRace
/// Regression test for https://github.com/microsoft/typescript-go/issues/4478.
///
/// While resolving a package with peerDependencies, two goroutines look up the
/// peer package's package.json concurrently. A `flipFileExistsFS` forces the
/// first lookup to cache a nil-Contents entry and the second lookup to receive
/// that stale entry from `Set`. The resolver must not dereference the peer
/// package.json contents unless the entry actually exists.
// PORT: single-threaded, as in the tests above. The outer resolution (from
// `b`) is the Go second goroutine: its `FileExists` of the peer package.json
// runs the whole inner resolution (from `a`, the Go first goroutine) before
// it returns. The inner one's `FileExists` returns false and it sets the
// nil-Contents entry first; then the outer one sees true, reads the file and
// gets that entry back from its `Set`, as in Go.
#[test]
fn test_resolve_peer_dependency_nil_contents_race() {
    const PEER_PKG_JSON: &str = "/repo/node_modules/peer/package.json";
    let files = [
        (
            "/repo/node_modules/pkg/package.json",
            r#"{"name":"pkg","version":"1.0.0","types":"index.d.ts","peerDependencies":{"peer":"*"}}"#,
        ),
        (
            "/repo/node_modules/pkg/index.d.ts",
            "export declare const x: number;",
        ),
        (PEER_PKG_JSON, r#"{"name":"peer","version":"2.0.0"}"#),
        ("/repo/src/a/file.ts", ""),
        ("/repo/src/b/file.ts", ""),
    ];
    let inner = vfstest::from_map(files, true);
    let nested: Nested = Rc::new(RefCell::new(None));
    let call_count = Rc::new(Cell::new(0));
    let fs = {
        let inner = Rc::clone(&inner);
        let nested = Rc::clone(&nested);
        let call_count = Rc::clone(&call_count);
        wrapvfs_wrap(
            Rc::clone(&inner),
            Replacements {
                file_exists: Some(Box::new(move |path: &str| {
                    if path == PEER_PKG_JSON {
                        call_count.set(call_count.get() + 1);
                        let n = call_count.get();
                        if n == 1 {
                            // The outer (Go second) caller: the inner (Go
                            // first) caller runs and finishes first.
                            let next = nested.borrow_mut().take();
                            if let Some(next) = next {
                                next();
                            }
                            return inner.file_exists(path); // second caller: file is visible
                        }
                        if n == 2 {
                            return false; // first caller: simulate "file not yet visible"
                        }
                    }
                    inner.file_exists(path)
                })),
                ..Default::default()
            },
        )
    };
    let resolver = Rc::new(new_repo_resolver(fs));

    let first: Rc<Cell<Option<bool>>> = Rc::new(Cell::new(None));
    {
        let resolver: Weak<DefaultResolver> = Rc::downgrade(&resolver);
        let first = Rc::clone(&first);
        *nested.borrow_mut() = Some(Box::new(move || {
            let resolver = resolver.upgrade().expect("resolver");
            first.set(Some(resolves(&resolver, "pkg", "/repo/src/a/file.ts")));
        }));
    }
    let panicked;
    let second = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        resolves(&resolver, "pkg", "/repo/src/b/file.ts")
    })) {
        Ok(resolved) => {
            panicked = false;
            resolved
        }
        Err(_) => {
            panicked = true;
            false
        }
    };

    assert!(
        !panicked,
        "resolver panicked due to nil Contents dereference in readPackageJsonPeerDependencies"
    );
    assert!(call_count.get() >= 2, "both callers reached FileExists");
    assert_eq!(
        first.get(),
        Some(true),
        "\"/repo/src/a/file.ts\" failed to resolve pkg"
    );
    assert!(second, "\"/repo/src/b/file.ts\" failed to resolve pkg");
}

// ---------------------------------------------------------------------------
// module/staticresolver_test.go
// ---------------------------------------------------------------------------

// Go: module/staticresolver_test.go:12 TestStaticResolver
#[test]
fn test_static_resolver() {
    let fs = vfstest::from_map(
        [
            (
                "/repo/node_modules/fallback/package.json",
                r#"{"name":"fallback","types":"index.d.ts"}"#,
            ),
            ("/repo/node_modules/fallback/index.d.ts", "export {};"),
        ],
        true,
    );
    let host: Rc<dyn ResolutionHost> = Rc::new(ResolutionHostStub {
        fs,
        cwd: "/repo".to_string(),
    });
    let fallback: Rc<dyn Resolver> = Rc::new(new_resolver(ResolverOptions {
        host: Some(host),
        compiler_options: Some(Rc::new(CompilerOptions {
            module: ModuleKind::ES_NEXT,
            module_resolution: ModuleResolutionKind::BUNDLER,
            ..Default::default()
        })),
        ..Default::default()
    }));
    let esm = ResolutionMode::ESM;
    let resolved = |resolved_file_name: &str| {
        Some(Arc::new(ts_goport::program::ResolvedModule {
            resolved_file_name: resolved_file_name.to_string(),
            ..Default::default()
        }))
    };
    let resolutions = new_static_resolutions(
        &[
            StaticResolutionEntry {
                module_name: "provided".to_string(),
                result: resolved("/global.d.ts"),
                ..Default::default()
            },
            StaticResolutionEntry {
                module_name: "provided".to_string(),
                containing_directory: "/repo/src".to_string(),
                result: resolved("/directory.d.ts"),
                ..Default::default()
            },
            StaticResolutionEntry {
                module_name: "provided".to_string(),
                resolution_mode: Some(esm),
                result: resolved("/esm.d.ts"),
                ..Default::default()
            },
            StaticResolutionEntry {
                module_name: "provided".to_string(),
                containing_directory: "/repo/src".to_string(),
                resolution_mode: Some(esm),
                result: resolved("/directory-esm.d.ts"),
            },
            StaticResolutionEntry {
                module_name: "unresolved".to_string(),
                ..Default::default()
            },
        ],
        true,
        "/repo",
        true,
    )
    .unwrap_or_else(|err| panic!("unexpected error: {}", err.error()));
    let resolver = new_static_resolver(fallback, Rc::new(resolutions));

    struct Test {
        name: &'static str,
        containing_file: &'static str,
        mode: ResolutionMode,
        resolved_file_name: &'static str,
    }
    let tests = [
        Test {
            name: "provided",
            containing_file: "/repo/src/index.ts",
            mode: ResolutionMode::ESM,
            resolved_file_name: "/directory-esm.d.ts",
        },
        Test {
            name: "provided",
            containing_file: "/repo/src/index.ts",
            mode: ResolutionMode::COMMON_JS,
            resolved_file_name: "/directory.d.ts",
        },
        Test {
            name: "provided",
            containing_file: "/repo/other/index.ts",
            mode: ResolutionMode::ESM,
            resolved_file_name: "/esm.d.ts",
        },
        Test {
            name: "provided",
            containing_file: "/repo/other/index.ts",
            mode: ResolutionMode::COMMON_JS,
            resolved_file_name: "/global.d.ts",
        },
        Test {
            name: "fallback",
            containing_file: "/repo/src/index.ts",
            mode: ResolutionMode::ESM,
            resolved_file_name: "/repo/node_modules/fallback/index.d.ts",
        },
        Test {
            name: "unresolved",
            containing_file: "/repo/src/index.ts",
            mode: ResolutionMode::ESM,
            resolved_file_name: "",
        },
    ];
    for test in &tests {
        let (result, _, err) =
            resolver.resolve_module_name(test.name, test.containing_file, test.mode, None);
        if let Some(err) = err {
            panic!("unexpected error: {}", err.error());
        }
        if test.resolved_file_name.is_empty() {
            assert!(
                result.is_none(),
                "{} from {}: expected no result",
                test.name,
                test.containing_file
            );
        } else {
            assert_eq!(
                result.map(|result| result.resolved_file_name.clone()),
                Some(test.resolved_file_name.to_string()),
                "{} from {}",
                test.name,
                test.containing_file
            );
        }
    }
}

// ---------------------------------------------------------------------------
// modulespecifiers/specifiers_test.go
// ---------------------------------------------------------------------------

// Go: modulespecifiers/specifiers_test.go:16 mockModuleSpecifierGenerationHost
struct MockModuleSpecifierGenerationHost {
    current_dir: String,
    // tsgo#4712
    content_mapper_extensions: Vec<String>,
    use_case_sensitive_file_names: bool,
    symlink_cache: Option<Rc<KnownSymlinks>>,
    // ts#64159 (specifiers_test.go:16): `existing_files` None means every
    // file exists.
    existing_files: Option<Vec<String>>,
    file_exists_calls: RefCell<Vec<String>>,
}

impl OutputPathsHost for MockModuleSpecifierGenerationHost {
    fn common_source_directory(&self) -> String {
        self.current_dir.clone()
    }
    fn get_current_directory(&self) -> String {
        self.current_dir.clone()
    }
    fn use_case_sensitive_file_names(&self) -> bool {
        self.use_case_sensitive_file_names
    }
    // tsgo#4712
    fn content_mapper_extensions(&self) -> Vec<String> {
        self.content_mapper_extensions.clone()
    }
}

impl ModuleSpecifierGenerationHost for MockModuleSpecifierGenerationHost {
    fn get_symlink_cache(&self) -> Option<Rc<KnownSymlinks>> {
        self.symlink_cache.clone()
    }
    fn common_source_directory(&self) -> String {
        self.current_dir.clone()
    }
    // Go: specifiers_test.go:47 ContentMapperExtensions (tsgo#4712)
    fn content_mapper_extensions(&self) -> Vec<String> {
        self.content_mapper_extensions.clone()
    }
    fn get_global_typings_cache_location(&self) -> String {
        String::new()
    }
    fn use_case_sensitive_file_names(&self) -> bool {
        self.use_case_sensitive_file_names
    }
    fn get_current_directory(&self) -> String {
        self.current_dir.clone()
    }
    fn get_project_reference_from_source(
        &self,
        _path: &Path,
    ) -> Option<Arc<SourceOutputAndProjectReference>> {
        None
    }
    fn get_redirect_targets(&self, _path: &Path) -> Vec<String> {
        Vec::new()
    }
    fn get_source_of_project_reference_if_output_included(&self, file: Node) -> String {
        ts_goport::ast::source_file_file_name(file).to_string()
    }
    // Go: modulespecifiers/specifiers_test.go:147 FileExists
    fn file_exists(&self, path: &str) -> bool {
        self.file_exists_calls.borrow_mut().push(path.to_string());
        if let Some(existing_files) = &self.existing_files {
            return existing_files.iter().any(|f| f == path);
        }
        true // Mock implementation
    }
    fn get_nearest_ancestor_directory_with_package_json(&self, _dirname: &str) -> String {
        String::new()
    }
    fn get_package_json_info(&self, _pkg_json_path: &str) -> Option<Arc<InfoCacheEntry>> {
        None
    }
    fn get_default_resolution_mode_for_file(&self, _file: Node) -> ResolutionMode {
        ResolutionMode::NONE
    }
    fn get_resolved_module_from_module_specifier(
        &self,
        _file: Node,
        _module_specifier: Node,
    ) -> Option<ResolvedModule> {
        None
    }
    fn get_mode_for_usage_location(&self, _file: Node, _module_specifier: Node) -> ResolutionMode {
        ResolutionMode::NONE
    }
    fn as_output_paths_host(&self) -> &dyn OutputPathsHost {
        self
    }
}

fn mock_host(symlink_cache: KnownSymlinks) -> MockModuleSpecifierGenerationHost {
    MockModuleSpecifierGenerationHost {
        current_dir: "/project".to_string(),
        content_mapper_extensions: Vec::new(),
        use_case_sensitive_file_names: true,
        symlink_cache: Some(Rc::new(symlink_cache)),
        existing_files: None,
        file_exists_calls: RefCell::new(Vec::new()),
    }
}

// Go: modulespecifiers/specifiers_test.go:82 TestGetEachFileNameOfModule
#[test]
fn test_get_each_file_name_of_module() {
    // (name, importingFile, importedFile, preferSymlinks, expectedCount, expectedPaths)
    #[rustfmt::skip]
    let tests: &[(&str, &str, &str, bool, usize, Option<&[&str]>)] = &[
        ("basic file path", "/project/src/main.ts", "/project/lib/utils.ts", false, 1, Some(&["/project/lib/utils.ts"])),
        ("symlink preference false", "/project/src/main.ts", "/project/lib/utils.ts", false, 1, None),
        ("symlink preference true", "/project/src/main.ts", "/project/lib/utils.ts", true, 1, None),
        // Should return 1 because there's no better option (all paths are ignored)
        ("ignored path with no alternatives", "/project/src/main.ts", "/project/node_modules/.pnpm/file.ts", false, 1, None),
    ];
    let mut t = Subtests::new("TestGetEachFileNameOfModule");
    for &(name, importing_file, imported_file, prefer_symlinks, expected_count, expected_paths) in
        tests
    {
        t.run(name, || {
            let host = mock_host(KnownSymlinks::new("/project", true));

            let result =
                get_each_file_name_of_module(importing_file, imported_file, &host, prefer_symlinks);

            let mut errors = Vec::new();
            if result.len() != expected_count {
                errors.push(format!(
                    "Expected {expected_count} paths, got {}",
                    result.len()
                ));
            }

            if let Some(expected_paths) = expected_paths {
                for (i, expected_path) in expected_paths.iter().enumerate() {
                    if i >= result.len() {
                        errors.push(format!(
                            "Expected path {i}: {expected_path}, but result has only {} paths",
                            result.len()
                        ));
                        continue;
                    }
                    if result[i].file_name != *expected_path {
                        errors.push(format!(
                            "Expected path {i} to be {expected_path}, got {}",
                            result[i].file_name
                        ));
                    }
                }
            }

            for (i, path) in result.iter().enumerate() {
                if path.file_name.is_empty() {
                    errors.push(format!("Path {i} has empty FileName"));
                }
            }
            if errors.is_empty() {
                Ok(())
            } else {
                Err(errors.join("\n"))
            }
        });
    }
    t.finish();
}

// Go: modulespecifiers/specifiers_test.go:159 TestGetEachFileNameOfModuleWithSymlinks
#[test]
fn test_get_each_file_name_of_module_with_symlinks() {
    let mut symlink_cache = KnownSymlinks::new("/project", true);

    let symlink_path =
        tspath::to_path("/project/symlink", "/project", true).ensure_trailing_directory_separator();
    let real_directory = KnownDirectoryLink {
        real: "/real/path/".to_string(),
        real_path: tspath::to_path("/real/path", "/project", true)
            .ensure_trailing_directory_separator(),
        ..Default::default()
    };
    symlink_cache.set_directory("/project/symlink", symlink_path, Some(real_directory));
    // PORT: Go sets the directory on the host's cache after making the
    // host; the Rust host holds the cache in an `Rc`, so it is set first.
    let host = mock_host(symlink_cache);

    let result =
        get_each_file_name_of_module("/project/src/main.ts", "/real/path/file.ts", &host, true);

    // Should find the symlink path
    let found = result
        .iter()
        .any(|path| path.file_name == "/project/symlink/file.ts");

    assert!(
        found,
        "Expected to find symlink path /project/symlink/file.ts"
    );
}

// Go: modulespecifiers/specifiers_test.go:288 TestModuleSpecifierContainsNodeModules (ts#64159
// renames TestContainsNodeModules; ContainsNodeModules is moduleSpecifierContainsNodeModules)
#[test]
fn test_module_specifier_contains_node_modules() {
    #[rustfmt::skip]
    let tests: &[(&str, &str, bool)] = &[
        ("contains node_modules", "/project/node_modules/lodash/index.js", true),
        ("does not contain node_modules", "/project/src/utils.ts", false),
        ("node_modules in middle", "/project/packages/node_modules/pkg/file.js", true),
        ("empty path", "", false),
    ];
    let mut t = Subtests::new("TestModuleSpecifierContainsNodeModules");
    for &(name, path, expected) in tests {
        t.run(name, || {
            let result = contains_node_modules(path);
            if result != expected {
                return Err(format!(
                    "moduleSpecifierContainsNodeModules({path:?}) = {result}, expected {expected}"
                ));
            }
            Ok(())
        });
    }
    t.finish();
}

// Not a Go test: a guard for getLocalModuleSpecifier at specifiers.go:633.
// ts#64159 keeps `strings.HasPrefix(maybeNonRelative, "..")` there, so a
// `paths` result like "..lib/thing" loses to the relative path. Go N'
// (tsgo-oracle-fed0bf24149f) gives "../../lib/thing" for the auto-import fix
// and completion of this layout (config skeptic, LSP battery skp-dotdot).
// PORT: it runs in a child process, because the importing file must be
// published (see `parse_type_script_published`).
#[test]
fn test_get_module_specifier_prefers_relative_over_dot_dot_paths_result() {
    super::childprog::in_child(
        module_path!(),
        "test_get_module_specifier_prefers_relative_over_dot_dot_paths_result",
        get_module_specifier_prefers_relative_over_dot_dot_paths_result,
    );
}

fn get_module_specifier_prefers_relative_over_dot_dot_paths_result() {
    use indexmap::IndexMap;
    use ts_goport::modulespecifiers::{ModuleSpecifierOptions, get_module_specifier};

    let host = mock_host(KnownSymlinks::new("/project", true));
    let file =
        super::parsetestutil::parse_type_script_published("thingValue;\n", false /*jsx*/);
    // (paths key, expected specifier)
    let tests: &[(&str, &str)] = &[("..lib/*", "../../lib/thing"), ("@lib/*", "@lib/thing")];
    let mut t = Subtests::new("GetModuleSpecifierPathsKeyStartingWithDotDot");
    for &(key, expected) in tests {
        t.run(key, || {
            let mut paths = IndexMap::new();
            paths.insert(key.to_string(), Some(vec!["./src/lib/*".to_string()]));
            let options = CompilerOptions {
                module: ModuleKind::COMMON_JS,
                target: ScriptTarget::ES2020,
                paths: Some(paths),
                paths_base_path: "/project".to_string(),
                ..Default::default()
            };
            let got = get_module_specifier(
                &options,
                &host,
                file,
                "/project/src/a/b/index.ts",
                "",
                "/project/src/lib/thing.ts",
                ModuleSpecifierOptions::default(),
            );
            if got != expected {
                return Err(format!("got {got:?}, expected {expected:?}"));
            }
            Ok(())
        });
    }
    t.finish();
}

// Go: modulespecifiers/specifiers_test.go:407 TestProcessEndingChecksRootedFilePath (ts#64159)
// PORT: `processEnding` is private, so the test asks `get_module_specifier`
// for the same specifier ("./lib/index.ts" from /project/src, minimal
// ending). It runs in a child process, because the importing file must be
// published (see `parse_type_script_published`).
#[test]
fn test_process_ending_checks_rooted_file_path() {
    super::childprog::in_child(
        module_path!(),
        "test_process_ending_checks_rooted_file_path",
        process_ending_checks_rooted_file_path,
    );
}

fn process_ending_checks_rooted_file_path() {
    use ts_goport::modulespecifiers::{ModuleSpecifierOptions, get_module_specifier};

    let mut host = mock_host(KnownSymlinks::new("/wrong", true));
    host.current_dir = "/wrong".to_string();
    host.existing_files = Some(vec!["/project/src/lib.ts".to_string()]);
    let file = super::parsetestutil::parse_type_script_published("", false /*jsx*/);
    let result = get_module_specifier(
        &CompilerOptions::default(),
        &host,
        file,
        "/project/src/main.ts",
        "",
        "/project/src/lib/index.ts",
        ModuleSpecifierOptions::default(),
    );
    assert_eq!(result, "./lib/index", "processEnding()");
    let calls = host.file_exists_calls.borrow();
    assert!(
        calls.iter().any(|c| c == "/project/src/lib.ts"),
        "FileExists calls = {calls:?}, expected a lookup for /project/src/lib.ts"
    );
}

// Not a Go test: a guard for tryGetModuleNameAsNodeModule at specifiers.go:825.
// ts#64159 asks whether the directory of the top-level node_modules contains
// the importing directory; N tested a string prefix, so /ab/src could import
// /a/node_modules/pkg by its package name. It runs in a child process, because
// the importing file must be published.
#[test]
fn test_node_modules_search_root_contains_the_importing_directory() {
    super::childprog::in_child(
        module_path!(),
        "test_node_modules_search_root_contains_the_importing_directory",
        node_modules_search_root_contains_the_importing_directory,
    );
}

fn node_modules_search_root_contains_the_importing_directory() {
    use ts_goport::modulespecifiers::{ModuleSpecifierOptions, get_module_specifier};

    let file = super::parsetestutil::parse_type_script_published("", false /*jsx*/);
    // (importing file, expected specifier)
    let tests: &[(&str, &str)] = &[
        ("/a/src/main.ts", "pkg"),
        ("/ab/src/main.ts", "../../a/node_modules/pkg"),
    ];
    let mut t = Subtests::new("NodeModulesSearchRoot");
    for &(importing, expected) in tests {
        t.run(importing, || {
            let mut host = mock_host(KnownSymlinks::new("/", true));
            host.current_dir = "/".to_string();
            host.existing_files = Some(Vec::new());
            let got = get_module_specifier(
                &CompilerOptions::default(),
                &host,
                file,
                importing,
                "",
                "/a/node_modules/pkg/index.d.ts",
                ModuleSpecifierOptions::default(),
            );
            if got != expected {
                return Err(format!("got {got:?}, expected {expected:?}"));
            }
            Ok(())
        });
    }
    t.finish();
}

// Go: modulespecifiers/specifiers_test.go:260 TestTryGetRealFileNameForNonJSDeclarationFileName
#[test]
fn test_try_get_real_file_name_for_non_js_declaration_file_name() {
    #[rustfmt::skip]
    let tests: &[(&str, &str, &str)] = &[
        ("json declaration file", "/project/foo.d.json.ts", "/project/foo.json"),
        ("multi-dot source extension declaration file", "/project/foo.module.d.css.ts", "/project/foo.module.css"),
        ("plain dts file ignored", "/project/foo.d.ts", ""),
    ];
    let mut t = Subtests::new("TestTryGetRealFileNameForNonJSDeclarationFileName");
    for &(name, file_name, expected) in tests {
        t.run(name, || {
            let got = try_get_real_file_name_for_non_js_declaration_file_name(file_name);
            if got != expected {
                return Err(format!(
                    "TryGetRealFileNameForNonJSDeclarationFileName({file_name:?}) = {got:?}, expected {expected:?}"
                ));
            }
            Ok(())
        });
    }
    t.finish();
}
