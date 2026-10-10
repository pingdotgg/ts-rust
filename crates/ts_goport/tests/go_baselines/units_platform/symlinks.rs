//! Go: `internal/symlinks/knownsymlinks_test.go`.
//!
//! PORT: Go `symlinks.KnownSymlinks` is `modulespecifiers::symlinks`. The
//! Go test reads the unexported `cwd` and `useCaseSensitiveFileNames`
//! fields; the port reads them through the derived `Debug` text. The Rust
//! cache is `&mut` and single-threaded, so `TestKnownSymlinksThreadSafety`
//! runs its ten writers in order.
//!
//! Blocked: `TestGuessDirectorySymlink` and
//! `TestIsNodeModulesOrScopedPackageDirectory` (the Rust
//! `guess_directory_symlink` and `is_node_modules_or_scoped_package_directory`
//! in `src/modulespecifiers/symlinks.rs` are private).

use std::rc::Rc;
use std::sync::Arc;

use ts_goport::frontend::prelude::{
    ParsedSourceFile, ResolutionCallback, ResolutionMode, ResolvedModule,
    ResolvedTypeReferenceDirective,
};
use ts_goport::frontend::tspath::{self, Path};
use ts_goport::modulespecifiers::symlinks::{KnownDirectoryLink, KnownSymlinks};

// Go: knownsymlinks_test.go:12 TestNewKnownSymlink
#[test]
fn test_new_known_symlink() {
    let cache = KnownSymlinks::new("/test/dir", true);
    let debug = format!("{cache:?}");
    assert!(
        debug.contains(r#"cwd: "/test/dir""#),
        "Expected cwd to be '/test/dir', got {debug}"
    );
    assert!(
        debug.contains("use_case_sensitive_file_names: true"),
        "Expected useCaseSensitiveFileNames to be true, got {debug}"
    );
}

fn dir_path(p: &str) -> Path {
    tspath::to_path(p, "/test/dir", true).ensure_trailing_directory_separator()
}

// Go: knownsymlinks_test.go:25 TestSetDirectory
#[test]
fn test_set_directory() {
    let mut cache = KnownSymlinks::new("/test/dir", true);
    let symlink_path = dir_path("/test/symlink");
    let real_directory = KnownDirectoryLink {
        real: "/real/path/".to_string(),
        real_path: dir_path("/real/path"),
        ..Default::default()
    };

    cache.set_directory(
        "/test/symlink",
        symlink_path.clone(),
        Some(real_directory.clone()),
    );

    // Check that directory was stored
    let stored = cache
        .directories()
        .get(&symlink_path)
        .expect("Expected directory to be stored")
        .clone()
        .expect("Expected a directory link");
    assert_eq!(stored.real, real_directory.real);
    assert_eq!(stored.real_path, real_directory.real_path);
    // ts#64544
    assert_eq!(
        stored.symlink, "/test/symlink/",
        "Expected Symlink to preserve '/test/symlink/'"
    );

    // Check that realpath mapping was created
    let set = cache
        .directories_by_realpath()
        .get(&real_directory.real_path)
        .expect("Expected realpath mapping to be created");
    assert!(!set.is_empty(), "Expected realpath mapping to be created");
    assert!(
        set.contains("/test/symlink"),
        "Expected symlink '/test/symlink' to be in set"
    );
}

// Go: knownsymlinks_test.go:59 TestKnownDirectoryLinkPreservesChildSpelling (ts#64544)
#[test]
fn test_known_directory_link_preserves_child_spelling() {
    let mut cache = KnownSymlinks::new("/test/dir", false);
    let symlink = "/Project/Node_Modules/pkg";
    let symlink_path =
        tspath::to_path(symlink, "/test/dir", false).ensure_trailing_directory_separator();
    cache.set_directory(
        symlink,
        symlink_path.clone(),
        Some(KnownDirectoryLink {
            real: "/Real/Package/".to_string(),
            real_path: tspath::to_path("/Real/Package", "/test/dir", false)
                .ensure_trailing_directory_separator(),
            ..Default::default()
        }),
    );

    let link = cache
        .directories()
        .get(&symlink_path)
        .cloned()
        .flatten()
        .expect("Expected directory link");
    let resolved = link
        .resolve_file_name("/PROJECT/node_modules/pkg/Src/File.ts", false)
        .expect("Expected child path to resolve through directory link");
    assert_eq!(
        resolved, "/Real/Package/Src/File.ts",
        "Expected child spelling to be preserved"
    );
}

// Go: knownsymlinks_test.go:56 TestSetFile
#[test]
fn test_set_file() {
    let mut cache = KnownSymlinks::new("/test/dir", true);
    let symlink = "/test/symlink/file.ts";
    let symlink_path = tspath::to_path(symlink, "/test/dir", true);
    let realpath = "/real/path/file.ts";

    cache.set_file(symlink, symlink_path.clone(), realpath);

    let stored = cache
        .files()
        .get(&symlink_path)
        .expect("Expected file to be stored");
    assert_eq!(stored, realpath);
}

// Go: knownsymlinks_test.go:74 TestProcessResolution
#[test]
fn test_process_resolution() {
    let mut cache = KnownSymlinks::new("/test/dir", true);

    // Test with empty paths
    cache.process_resolution("", "");
    cache.process_resolution("original", "");
    cache.process_resolution("", "resolved");

    // Test with valid paths
    let original_path = "/test/original/file.ts";
    let resolved_path = "/test/resolved/file.ts";
    cache.process_resolution(original_path, resolved_path);

    let symlink_path = tspath::to_path(original_path, "/test/dir", true);
    let stored = cache
        .files()
        .get(&symlink_path)
        .expect("Expected file to be stored");
    assert_eq!(stored, resolved_path);
}

// Go: knownsymlinks_test.go:193 TestSetSymlinksFromResolutions
#[test]
fn test_set_symlinks_from_resolutions() {
    let mut cache = KnownSymlinks::new("/test/dir", true);

    // Mock resolution data: (originalPath, resolvedPath, moduleName)
    let resolved_modules = [
        (
            "/test/original/file1.ts",
            "/test/resolved/file1.ts",
            "module1",
        ),
        (
            "/test/original/file2.ts",
            "/test/resolved/file2.ts",
            "module2",
        ),
    ];
    let file_path = tspath::to_path("/test/source.ts", "/test/dir", true);

    let for_each_resolved_module =
        |callback: &mut ResolutionCallback<'_, Arc<ResolvedModule>>,
         _file: Option<&ParsedSourceFile>| {
            for (original_path, resolved_path, module_name) in resolved_modules {
                let resolution = Arc::new(ResolvedModule {
                    original_path: original_path.to_string(),
                    resolved_file_name: resolved_path.to_string(),
                    ..Default::default()
                });
                callback(
                    &resolution,
                    module_name,
                    ResolutionMode::default(),
                    &file_path,
                );
            }
        };
    // No type reference directives for this test
    let for_each_resolved_type_reference_directive =
        |_callback: &mut ResolutionCallback<'_, Rc<ResolvedTypeReferenceDirective>>,
         _file: Option<&ParsedSourceFile>| {};

    cache.set_symlinks_from_resolutions(
        &for_each_resolved_module,
        &for_each_resolved_type_reference_directive,
    );

    for (original_path, resolved_path, _) in resolved_modules {
        let symlink_path = tspath::to_path(original_path, "/test/dir", true);
        match cache.files().get(&symlink_path) {
            Some(stored) => assert_eq!(stored, resolved_path),
            None => panic!("Expected file '{original_path}' to be stored"),
        }
    }
}

// Go: knownsymlinks_test.go:258 TestKnownSymlinksThreadSafety
#[test]
fn test_known_symlinks_thread_safety() {
    let mut cache = KnownSymlinks::new("/test/dir", true);
    for id in 0..10u32 {
        // Go `string(rune(id))`
        let r = char::from_u32(id).unwrap();
        let symlink_path = dir_path(&format!("/test/symlink{r}"));
        let real_directory = KnownDirectoryLink {
            real: format!("/real/path{r}/"),
            real_path: dir_path(&format!("/real/path{r}")),
            ..Default::default()
        };
        cache.set_directory(
            &format!("/test/symlink{r}"),
            symlink_path.clone(),
            Some(real_directory.clone()),
        );
        let stored = cache
            .directories()
            .get(&symlink_path)
            .and_then(Clone::clone)
            .unwrap_or_else(|| panic!("Goroutine {id}: Expected directory to be stored"));
        assert_eq!(stored.real, real_directory.real, "Goroutine {id}");
    }
    assert_eq!(
        cache.directories().len(),
        10,
        "Expected 10 directories to be stored"
    );
}
