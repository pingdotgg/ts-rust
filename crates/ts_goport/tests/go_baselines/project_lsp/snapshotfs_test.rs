//! Port of Go `internal/project/snapshotfs_test.go` (`TestSnapshotFSBuilder`,
//! `TestSnapshotFS`, `TestSourceFS`, `TestAutoImportBuilderFS`,
//! `TestRealpathAliasLifecycle`, `TestExpandAndFilterWatchEvents`). No
//! program is built, so the tests run in the test process.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use indexmap::IndexMap;
use rustc_hash::{FxHashMap, FxHashSet};
use ts_goport::flags::ScriptKind;
use ts_goport::frontend::tspath::Path;
use ts_goport::frontend::vfs::Fs;
use ts_goport::lsp::lsproto;
use ts_goport::project::{
    AutoImportBuilderFS, CachedDirectory, CachedFile, FileBase, FileChangeSummary, FileContent,
    FileHandle, FileHandleSource, FileSource, LayeredFileSystem, Overlay, RealpathAliasSet,
    SnapshotFS, SnapshotFSBuilder, layer_overlay_file_system, new_cached_file,
    new_cached_file_handle, new_overlay_fs, new_snapshot_fs_builder_from_source, new_source_fs,
};

use super::util::uri;
use crate::support::vfstest::{self, MapFile};

type CacheFiles = FxHashMap<Path, Rc<RefCell<CachedFile>>>;
type Dirs = FxHashMap<Path, CachedDirectory>;
type Aliases = FxHashMap<Path, Rc<RefCell<RealpathAliasSet>>>;
type Overlays = IndexMap<Path, Rc<Overlay>>;

fn p(s: &str) -> Path {
    Path(s.to_string())
}

// Go: snapshotfs_test.go:18 toPath
fn to_path() -> Rc<dyn Fn(&str) -> Path> {
    Rc::new(|file_name: &str| Path(file_name.to_string()))
}

fn text_fs(entries: &[(&str, &str)], case_sensitive: bool) -> Rc<dyn Fs> {
    vfstest::from_map(entries.iter().copied(), case_sensitive)
}

fn any_fs(entries: Vec<(&str, MapFile)>, case_sensitive: bool) -> Rc<dyn Fs> {
    vfstest::from_map(entries, case_sensitive)
}

fn text(s: &str) -> MapFile {
    MapFile::from(s)
}

// Go: snapshotfs_test.go:17 newSnapshotFSBuilder (ts#64291: a test helper)
/// Go `newSnapshotFSBuilder(fs, prevOverlays{}, overlays, cacheFiles, cacheDirectories, aliases, UTF16, toPath)`:
/// layers the overlays over `fs`. The maps are new (owned) or shared with a
/// previous snapshot (`Rc`).
fn builder(
    fs: Rc<dyn Fs>,
    overlays: Overlays,
    cache_files: impl Into<Rc<CacheFiles>>,
    dirs: impl Into<Rc<Dirs>>,
    aliases: impl Into<Rc<Aliases>>,
) -> Rc<SnapshotFSBuilder> {
    let layered_fs = layer_overlay_file_system(
        fs,
        overlays,
        lsproto::PositionEncodingKind::UTF16,
        to_path(),
    );
    new_snapshot_fs_builder_from_source(
        layered_fs,
        cache_files.into(),
        dirs.into(),
        aliases.into(),
        to_path(),
    )
}

// Go: snapshotfs_test.go:32 newTestLayeredFileSystem (ts#64291)
fn new_test_layered_file_system(fs: Rc<dyn Fs>) -> Rc<dyn LayeredFileSystem> {
    new_overlay_fs(
        fs,
        IndexMap::default(),
        lsproto::PositionEncodingKind::UTF16,
        to_path(),
    )
}

// Go: snapshotfs_test.go:36 countingHandleFileSystem (ts#64291)
// PORT: Go embeds the `LayeredFileSystem`; the other methods forward to
// `inner`. The counters are `Cell`s.
struct CountingHandleFileSystem {
    inner: Rc<dyn LayeredFileSystem>,
    content: String,
    get_file_by_path_calls: Cell<i32>,
    read_file_calls: Cell<i32>,
}

impl FileHandleSource for CountingHandleFileSystem {
    // Go: snapshotfs_test.go:43 countingHandleFileSystem.GetFile
    fn get_file(&self, file_name: &str) -> Option<Rc<dyn FileHandle>> {
        self.get_file_by_path(file_name, &p(file_name))
    }

    // Go: snapshotfs_test.go:47 countingHandleFileSystem.GetFileByPath
    fn get_file_by_path(&self, file_name: &str, _path: &Path) -> Option<Rc<dyn FileHandle>> {
        self.get_file_by_path_calls
            .set(self.get_file_by_path_calls.get() + 1);
        Some(new_cached_file_handle(file_name, self.content.clone()))
    }
}

impl LayeredFileSystem for CountingHandleFileSystem {
    fn overlays(&self) -> Rc<IndexMap<Path, Rc<Overlay>>> {
        self.inner.overlays()
    }
}

impl Fs for CountingHandleFileSystem {
    fn use_case_sensitive_file_names(&self) -> bool {
        self.inner.use_case_sensitive_file_names()
    }
    fn file_exists(&self, path: &str) -> bool {
        self.inner.file_exists(path)
    }
    // Go: snapshotfs_test.go:52 countingHandleFileSystem.ReadFile
    fn read_file(&self, _path: &str) -> (String, bool) {
        self.read_file_calls.set(self.read_file_calls.get() + 1);
        (self.content.clone(), true)
    }
    fn write_file(&self, path: &str, data: &str) -> Result<(), ts_goport::frontend::vfs::FsError> {
        self.inner.write_file(path, data)
    }
    fn append_file(&self, path: &str, data: &str) -> Result<(), ts_goport::frontend::vfs::FsError> {
        self.inner.append_file(path, data)
    }
    fn remove(&self, path: &str) -> Result<(), ts_goport::frontend::vfs::FsError> {
        self.inner.remove(path)
    }
    fn chtimes(
        &self,
        path: &str,
        a_time: Option<std::time::SystemTime>,
        m_time: Option<std::time::SystemTime>,
    ) -> Result<(), ts_goport::frontend::vfs::FsError> {
        self.inner.chtimes(path, a_time, m_time)
    }
    fn directory_exists(&self, path: &str) -> bool {
        self.inner.directory_exists(path)
    }
    fn get_accessible_entries(&self, path: &str) -> ts_goport::frontend::vfs::Entries {
        self.inner.get_accessible_entries(path)
    }
    fn stat(&self, path: &str) -> Option<ts_goport::frontend::vfs::FileInfo> {
        self.inner.stat(path)
    }
    fn realpath(&self, path: &str) -> String {
        self.inner.realpath(path)
    }
}

// Go: snapshotfs_test.go:57 TestSnapshotFSBuilderCachesReturnedSourceHandle (ts#64291)
#[test]
fn snapshot_fs_builder_caches_returned_source_handle() {
    let file_system = Rc::new(CountingHandleFileSystem {
        inner: new_test_layered_file_system(text_fs(&[], true)),
        content: "export const value = 1;".to_string(),
        get_file_by_path_calls: Cell::new(0),
        read_file_calls: Cell::new(0),
    });
    let b = new_snapshot_fs_builder_from_source(
        file_system.clone(),
        Rc::new(CacheFiles::default()),
        Rc::new(Dirs::default()),
        Rc::new(Aliases::default()),
        to_path(),
    );

    let file = b.get_file("/src/index.ts").expect("file");
    assert_eq!(file.content(), file_system.content);
    let again = b.get_file("/src/index.ts").expect("file");
    assert!(std::ptr::eq(
        Rc::as_ptr(&again) as *const u8,
        Rc::as_ptr(&file) as *const u8
    ));
    assert_eq!(file_system.get_file_by_path_calls.get(), 1);
    assert_eq!(file_system.read_file_calls.get(), 0);
}

fn empty_builder(fs: Rc<dyn Fs>) -> Rc<SnapshotFSBuilder> {
    builder(
        fs,
        Overlays::default(),
        CacheFiles::default(),
        Dirs::default(),
        Aliases::default(),
    )
}

/// Go `map[tspath.Path]dirty.CloneableMap[tspath.Path, string]{...}`.
fn dirs(entries: &[(&str, &[(&str, &str)])]) -> Dirs {
    entries
        .iter()
        .map(|(dir, children)| {
            let map: IndexMap<Path, String> = children
                .iter()
                .map(|(k, v)| (p(k), v.to_string()))
                .collect();
            (p(dir), Rc::new(RefCell::new(map)))
        })
        .collect()
}

/// Go `map[tspath.Path]*cachedFile{path: newCachedFile(path, content), ...}`.
fn cache_files(entries: &[(&str, &str)]) -> CacheFiles {
    entries
        .iter()
        .map(|(name, content)| (p(name), new_cached_file(name, content.to_string())))
        .collect()
}

/// Go `&Overlay{fileBase: fileBase{fileName: name, content: content}}`.
fn overlay(name: &str, content: &str) -> Rc<Overlay> {
    Rc::new(Overlay {
        file_base: FileBase {
            file_name: name.to_string(),
            content: content.into(),
            ..FileBase::default()
        },
        version: Cell::new(0),
        kind: ScriptKind::UNKNOWN,
        matches_disk_text: Cell::new(false),
    })
}

fn overlays(entries: &[(&str, &str)]) -> Overlays {
    entries
        .iter()
        .map(|(name, content)| (p(name), overlay(name, content)))
        .collect()
}

/// Go `snapshot.cacheDirectories[dir][child]` presence.
fn dir_has(dirs: &Dirs, dir: &str, child: &str) -> bool {
    dirs.get(&p(dir))
        .is_some_and(|d| d.borrow().contains_key(&p(child)))
}

/// Go `builder.cacheFiles.Load(path)` then `entry.Delete()`.
fn delete_cache_file(b: &SnapshotFSBuilder, path: &str) {
    if let (Some(entry), true) = b.cache_files.load(&p(path)) {
        entry.delete();
    }
}

fn file_content(file: Option<Rc<dyn ts_goport::project::FileHandle>>) -> String {
    file.expect("file should exist").content()
}

fn alias_has(aliases: &Aliases, realpath: &str, symlink: &str) -> bool {
    aliases
        .get(&p(realpath))
        .is_some_and(|set| set.borrow().paths.contains_key(&p(symlink)))
}

// ---------------------------------------------------------------------------
// TestSnapshotFSBuilder
// ---------------------------------------------------------------------------

// Go: snapshotfs_test.go:22 TestSnapshotFSBuilder/builds directory tree on file add
#[test]
fn builder_builds_directory_tree_on_file_add() {
    let b = empty_builder(text_fs(&[("/src/foo.ts", "const foo = 1;")], false));

    // Read the file to add it to the cacheFiles
    assert_eq!(file_content(b.get_file("/src/foo.ts")), "const foo = 1;");

    // Finalize and check directories
    let (snapshot, changed) = b.finalize();
    assert!(changed, "should have changed");

    // /src should contain /src/foo.ts
    assert!(
        snapshot.cache_directories.contains_key(&p("/src")),
        "/src directory should exist"
    );
    assert!(
        dir_has(&snapshot.cache_directories, "/src", "/src/foo.ts"),
        "/src should contain /src/foo.ts"
    );

    // / should contain /src
    assert!(
        snapshot.cache_directories.contains_key(&p("/")),
        "/ directory should exist"
    );
    assert!(
        dir_has(&snapshot.cache_directories, "/", "/src"),
        "/ should contain /src"
    );
}

// Go: snapshotfs_test.go:62 TestSnapshotFSBuilder/builds nested directory tree
#[test]
fn builder_builds_nested_directory_tree() {
    let b = empty_builder(text_fs(
        &[("/src/nested/deep/file.ts", "export const x = 1;")],
        false,
    ));

    assert!(
        b.get_file("/src/nested/deep/file.ts").is_some(),
        "file should exist"
    );

    let (snapshot, changed) = b.finalize();
    assert!(changed, "should have changed");

    // Check the complete directory tree
    let d = &snapshot.cache_directories;
    assert!(dir_has(d, "/src/nested/deep", "/src/nested/deep/file.ts"));
    assert!(dir_has(d, "/src/nested", "/src/nested/deep"));
    assert!(dir_has(d, "/src", "/src/nested"));
    assert!(dir_has(d, "/", "/src"));
}

// Go: snapshotfs_test.go:97 TestSnapshotFSBuilder/removes directory entries on file delete
#[test]
fn builder_removes_directory_entries_on_file_delete() {
    let b = builder(
        text_fs(&[("/src/foo.ts", "const foo = 1;")], false),
        Overlays::default(),
        cache_files(&[("/src/foo.ts", "const foo = 1;")]),
        dirs(&[
            ("/", &[("/src", "src")]),
            ("/src", &[("/src/foo.ts", "foo.ts")]),
        ]),
        Aliases::default(),
    );

    // Mark the file for deletion by loading and deleting
    delete_cache_file(&b, "/src/foo.ts");

    let (snapshot, changed) = b.finalize();
    assert!(changed, "should have changed");

    // File should be deleted
    assert!(
        !snapshot.cache_files.contains_key(&p("/src/foo.ts")),
        "file should be deleted"
    );

    // Directory tree should be cleaned up
    assert!(
        !snapshot.cache_directories.contains_key(&p("/src")),
        "/src directory should be removed"
    );
    assert!(
        !snapshot.cache_directories.contains_key(&p("/")),
        "root directory should be removed"
    );
}

// Go: snapshotfs_test.go:147 TestSnapshotFSBuilder/removes only empty directories on file delete
#[test]
fn builder_removes_only_empty_directories_on_file_delete() {
    let b = builder(
        text_fs(
            &[
                ("/src/foo.ts", "const foo = 1;"),
                ("/src/bar.ts", "const bar = 2;"),
            ],
            false,
        ),
        Overlays::default(),
        cache_files(&[
            ("/src/foo.ts", "const foo = 1;"),
            ("/src/bar.ts", "const bar = 2;"),
        ]),
        dirs(&[
            ("/", &[("/src", "src")]),
            (
                "/src",
                &[("/src/foo.ts", "foo.ts"), ("/src/bar.ts", "bar.ts")],
            ),
        ]),
        Aliases::default(),
    );

    // Delete only foo.ts
    delete_cache_file(&b, "/src/foo.ts");

    let (snapshot, changed) = b.finalize();
    assert!(changed, "should have changed");

    assert!(
        !snapshot.cache_files.contains_key(&p("/src/foo.ts")),
        "foo.ts should be deleted"
    );
    assert!(
        snapshot.cache_files.contains_key(&p("/src/bar.ts")),
        "bar.ts should still exist"
    );

    // /src directory should still exist with bar.ts
    assert!(
        snapshot.cache_directories.contains_key(&p("/src")),
        "/src directory should still exist"
    );
    assert!(
        !dir_has(&snapshot.cache_directories, "/src", "/src/foo.ts"),
        "/src should not contain foo.ts"
    );
    assert!(
        dir_has(&snapshot.cache_directories, "/src", "/src/bar.ts"),
        "/src should contain bar.ts"
    );

    // root should still contain /src
    assert!(
        snapshot.cache_directories.contains_key(&p("/")),
        "root directory should still exist"
    );
    assert!(
        dir_has(&snapshot.cache_directories, "/", "/src"),
        "root should contain /src"
    );
}

// Go: snapshotfs_test.go:211 TestSnapshotFSBuilder/adds file to existing directory
#[test]
fn builder_adds_file_to_existing_directory() {
    let b = builder(
        text_fs(
            &[
                ("/src/foo.ts", "const foo = 1;"),
                ("/src/bar.ts", "const bar = 2;"),
            ],
            false,
        ),
        Overlays::default(),
        cache_files(&[("/src/foo.ts", "const foo = 1;")]),
        dirs(&[
            ("/", &[("/src", "src")]),
            ("/src", &[("/src/foo.ts", "foo.ts")]),
        ]),
        Aliases::default(),
    );

    // Read bar.ts to add it
    assert!(b.get_file("/src/bar.ts").is_some(), "bar.ts should exist");

    let (snapshot, changed) = b.finalize();
    assert!(changed, "should have changed");

    // /src should contain both files
    assert!(
        dir_has(&snapshot.cache_directories, "/src", "/src/foo.ts"),
        "/src should contain foo.ts"
    );
    assert!(
        dir_has(&snapshot.cache_directories, "/src", "/src/bar.ts"),
        "/src should contain bar.ts"
    );
}

// Go: snapshotfs_test.go:257 TestSnapshotFSBuilder/no change when no files added or deleted
#[test]
fn builder_no_change_when_no_files_added_or_deleted() {
    let b = builder(
        text_fs(&[("/src/foo.ts", "const foo = 1;")], false),
        Overlays::default(),
        cache_files(&[("/src/foo.ts", "const foo = 1;")]),
        dirs(&[
            ("/", &[("/src", "src")]),
            ("/src", &[("/src/foo.ts", "foo.ts")]),
        ]),
        Aliases::default(),
    );

    // Don't add or delete any files
    let (snapshot, changed) = b.finalize();
    assert!(!changed, "should not have changed");

    // Directories should remain the same
    assert!(dir_has(&snapshot.cache_directories, "/src", "/src/foo.ts"));
}

// Go: snapshotfs_test.go:355 TestSnapshotFSBuilder/overlay files are returned over disk files
#[test]
fn builder_overlay_files_are_returned_over_disk_files() {
    let b = builder(
        text_fs(&[("/src/foo.ts", "const foo = 1;")], false),
        overlays(&[("/src/foo.ts", "const foo = 999;")]),
        CacheFiles::default(),
        Dirs::default(),
        Aliases::default(),
    );

    // Should return overlay content
    assert_eq!(file_content(b.get_file("/src/foo.ts")), "const foo = 999;");
}

// Go: snapshotfs_test.go:325 TestSnapshotFSBuilder/multiple files added and deleted in single cycle
#[test]
fn builder_multiple_files_added_and_deleted_in_single_cycle() {
    let b = builder(
        text_fs(
            &[
                ("/src/a.ts", "const a = 1;"),
                ("/src/b.ts", "const b = 2;"),
                ("/lib/utils.ts", "export const util = 1;"),
                ("/lib/helpers.ts", "export const helper = 1;"),
                ("/other/single.ts", "const single = 1;"),
            ],
            false,
        ),
        Overlays::default(),
        cache_files(&[
            ("/src/a.ts", "const a = 1;"),
            ("/other/single.ts", "const single = 1;"),
        ]),
        dirs(&[
            ("/", &[("/src", "src"), ("/other", "other")]),
            ("/src", &[("/src/a.ts", "a.ts")]),
            ("/other", &[("/other/single.ts", "single.ts")]),
        ]),
        Aliases::default(),
    );

    // Add new files
    assert!(b.get_file("/src/b.ts").is_some());
    assert!(b.get_file("/lib/utils.ts").is_some());
    assert!(b.get_file("/lib/helpers.ts").is_some());

    // Delete existing files
    delete_cache_file(&b, "/src/a.ts");
    delete_cache_file(&b, "/other/single.ts");

    let (snapshot, changed) = b.finalize();
    assert!(changed, "should have changed");

    // Verify deleted files are gone
    assert!(
        !snapshot.cache_files.contains_key(&p("/src/a.ts")),
        "/src/a.ts should be deleted"
    );
    assert!(
        !snapshot.cache_files.contains_key(&p("/other/single.ts")),
        "/other/single.ts should be deleted"
    );

    // Verify added files exist
    assert!(
        snapshot.cache_files.contains_key(&p("/src/b.ts")),
        "/src/b.ts should exist"
    );
    assert!(
        snapshot.cache_files.contains_key(&p("/lib/utils.ts")),
        "/lib/utils.ts should exist"
    );
    assert!(
        snapshot.cache_files.contains_key(&p("/lib/helpers.ts")),
        "/lib/helpers.ts should exist"
    );

    let d = &snapshot.cache_directories;
    // Verify /other directory is cleaned up (was only entry deleted)
    assert!(
        !d.contains_key(&p("/other")),
        "/other directory should be removed"
    );

    // Verify /src still exists with b.ts (a.ts deleted, b.ts added)
    assert!(d.contains_key(&p("/src")), "/src directory should exist");
    assert!(
        !dir_has(d, "/src", "/src/a.ts"),
        "/src should not contain a.ts"
    );
    assert!(dir_has(d, "/src", "/src/b.ts"), "/src should contain b.ts");

    // Verify /lib was created with both files
    assert!(d.contains_key(&p("/lib")), "/lib directory should exist");
    assert!(
        dir_has(d, "/lib", "/lib/utils.ts"),
        "/lib should contain utils.ts"
    );
    assert!(
        dir_has(d, "/lib", "/lib/helpers.ts"),
        "/lib should contain helpers.ts"
    );

    // Verify root contains /src and /lib but not /other
    assert!(dir_has(d, "/", "/src"), "root should contain /src");
    assert!(dir_has(d, "/", "/lib"), "root should contain /lib");
    assert!(!dir_has(d, "/", "/other"), "root should not contain /other");
}

// Go: snapshotfs_test.go:484 TestSnapshotFSBuilder/overlay directories are computed from overlays
#[test]
fn builder_overlay_directories_are_computed_from_overlays() {
    let b = builder(
        text_fs(&[], false),
        overlays(&[
            ("/src/overlay.ts", "const x = 1;"),
            ("/src/nested/deep.ts", "const y = 2;"),
        ]),
        CacheFiles::default(),
        Dirs::default(),
        Aliases::default(),
    );

    // ts#64291
    let src_entries = b.get_accessible_entries("/src");
    assert!(
        src_entries.files.iter().any(|f| f == "overlay.ts"),
        "/src should contain overlay.ts"
    );
    assert!(
        src_entries.directories.iter().any(|d| d == "nested"),
        "/src should contain nested/"
    );

    let nested_entries = b.get_accessible_entries("/src/nested");
    assert!(
        nested_entries.files.iter().any(|f| f == "deep.ts"),
        "/src/nested should contain deep.ts"
    );

    let root_entries = b.get_accessible_entries("/");
    assert!(
        root_entries.directories.iter().any(|d| d == "src"),
        "/ should contain /src"
    );
}

// Go: snapshotfs_test.go:470 TestSnapshotFSBuilder/GetAccessibleEntries combines disk and overlay
#[test]
fn builder_get_accessible_entries_combines_disk_and_overlay() {
    let b = builder(
        text_fs(&[("/src/disk.ts", "const disk = 1;")], false),
        overlays(&[("/src/overlay.ts", "const overlay = 1;")]),
        CacheFiles::default(),
        Dirs::default(),
        Aliases::default(),
    );

    let entries = b.get_accessible_entries("/src");

    // Should contain both disk file and overlay file (both as basenames)
    assert!(
        entries.files.iter().any(|f| f == "disk.ts"),
        "should contain disk.ts"
    );
    assert!(
        entries.files.iter().any(|f| f == "overlay.ts"),
        "should contain overlay.ts"
    );
}

// ---------------------------------------------------------------------------
// TestSnapshotFS
// ---------------------------------------------------------------------------

/// Go `&SnapshotFS{toPath, fs: newOverlayFS(testFS, overlays, UTF16, toPath), cacheFiles, cacheDirectories}`
/// (ts#64291; with no overlays it is Go `newTestLayeredFileSystem(testFS, toPath)`).
fn snapshot_fs(
    fs: Rc<dyn Fs>,
    overlays: Overlays,
    cache_files: CacheFiles,
    cache_directories: Dirs,
) -> Rc<SnapshotFS> {
    Rc::new(SnapshotFS {
        to_path: to_path(),
        fs: new_overlay_fs(
            fs,
            overlays,
            lsproto::PositionEncodingKind::UTF16,
            to_path(),
        ),
        cache_files: Rc::new(cache_files),
        cache_directories: Rc::new(cache_directories),
        read_files: RefCell::default(),
        node_modules_realpath_aliases: Rc::new(Aliases::default()),
    })
}

// Go: snapshotfs_test.go:508 TestSnapshotFS/GetFile returns overlay file
#[test]
fn snapshot_fs_get_file_returns_overlay_file() {
    let s = snapshot_fs(
        text_fs(&[("/src/foo.ts", "disk content")], false),
        overlays(&[("/src/foo.ts", "overlay content")]),
        CacheFiles::default(),
        Dirs::default(),
    );
    assert_eq!(file_content(s.get_file("/src/foo.ts")), "overlay content");
}

// Go: snapshotfs_test.go:534 TestSnapshotFS/GetFile returns disk file when not in overlay
#[test]
fn snapshot_fs_get_file_returns_disk_file_when_not_in_overlay() {
    let s = snapshot_fs(
        text_fs(&[("/src/foo.ts", "disk content")], false),
        Overlays::default(),
        cache_files(&[("/src/foo.ts", "disk content")]),
        Dirs::default(),
    );
    assert_eq!(file_content(s.get_file("/src/foo.ts")), "disk content");
}

// Go: snapshotfs_test.go:558 TestSnapshotFS/GetFile reads from fs when not cached
#[test]
fn snapshot_fs_get_file_reads_from_fs_when_not_cached() {
    let s = snapshot_fs(
        text_fs(&[("/src/foo.ts", "fs content")], false),
        Overlays::default(),
        CacheFiles::default(),
        Dirs::default(),
    );
    assert_eq!(file_content(s.get_file("/src/foo.ts")), "fs content");
}

// Go: snapshotfs_test.go:578 TestSnapshotFS/GetFile returns nil for non-existent file
#[test]
fn snapshot_fs_get_file_returns_nil_for_non_existent_file() {
    let s = snapshot_fs(
        text_fs(&[], false),
        Overlays::default(),
        CacheFiles::default(),
        Dirs::default(),
    );
    assert!(
        s.get_file("/src/nonexistent.ts").is_none(),
        "should return nil for non-existent file"
    );
}

// Go: snapshotfs_test.go:685 TestSnapshotFS/isOpenFile returns true for overlays
#[test]
fn snapshot_fs_is_open_file_returns_true_for_overlays() {
    // ts#64291: the overlay file system answers it.
    let overlay_fs = new_overlay_fs(
        text_fs(&[], false),
        overlays(&[("/src/foo.ts", "overlay content")]),
        lsproto::PositionEncodingKind::UTF16,
        to_path(),
    );
    assert!(
        overlay_fs
            .get_file("/src/foo.ts")
            .expect("overlay file")
            .is_overlay(),
        "overlay file should be open"
    );
    assert!(
        overlay_fs.get_file("/src/bar.ts").is_none(),
        "non-overlay file should not be open"
    );
}

// Go: snapshotfs_test.go:618 TestSnapshotFS/GetFileByPath uses provided path
#[test]
fn snapshot_fs_get_file_by_path_uses_provided_path() {
    let s = snapshot_fs(
        text_fs(&[("/src/foo.ts", "disk content")], false),
        overlays(&[("/src/foo.ts", "overlay content")]),
        CacheFiles::default(),
        Dirs::default(),
    );
    // GetFileByPath should use the provided path directly
    assert_eq!(
        file_content(s.get_file_by_path("/src/foo.ts", &p("/src/foo.ts"))),
        "overlay content"
    );
}

// Go: snapshotfs_test.go:724 TestSnapshotFS/GetAccessibleEntries combines disk and overlay directories
#[test]
fn snapshot_fs_get_accessible_entries_combines_disk_and_overlay_directories() {
    let s = snapshot_fs(
        text_fs(&[], false),
        overlays(&[("/src/overlay.ts", "overlay content")]),
        cache_files(&[("/src/disk.ts", "disk content")]),
        dirs(&[
            ("/", &[("/src", "src")]),
            ("/src", &[("/src/disk.ts", "disk.ts")]),
        ]),
    );

    let entries = s.get_accessible_entries("/src");

    // Should contain both disk file and overlay file (both as basenames)
    assert!(
        entries.files.iter().any(|f| f == "disk.ts"),
        "should contain disk.ts"
    );
    assert!(
        entries.files.iter().any(|f| f == "overlay.ts"),
        "should contain overlay.ts"
    );
}

// ---------------------------------------------------------------------------
// TestSourceFS
// ---------------------------------------------------------------------------

fn plain_snapshot(entries: &[(&str, &str)]) -> Rc<SnapshotFS> {
    snapshot_fs(
        text_fs(entries, false),
        Overlays::default(),
        CacheFiles::default(),
        Dirs::default(),
    )
}

// Go: snapshotfs_test.go:698 TestSourceFS/tracks files when tracking enabled
#[test]
fn source_fs_tracks_files_when_tracking_enabled() {
    let snapshot = plain_snapshot(&[("/src/foo.ts", "content")]);
    let source_fs = new_source_fs(true /* tracking */, snapshot, to_path());

    // File should not be seen yet
    assert!(!source_fs.seen_file(&p("/src/foo.ts")));

    // Read the file
    assert!(source_fs.get_file("/src/foo.ts").is_some());

    // Now it should be seen
    assert!(source_fs.seen_file(&p("/src/foo.ts")));
}

// Go: snapshotfs_test.go:726 TestSourceFS/does not track files when tracking disabled
#[test]
fn source_fs_does_not_track_files_when_tracking_disabled() {
    let snapshot = plain_snapshot(&[("/src/foo.ts", "content")]);
    let source_fs = new_source_fs(false /* tracking */, snapshot, to_path());

    // Read the file
    assert!(source_fs.get_file("/src/foo.ts").is_some());

    // Should not be seen since tracking is disabled
    assert!(!source_fs.seen_file(&p("/src/foo.ts")));
}

// Go: snapshotfs_test.go:751 TestSourceFS/DisableTracking stops tracking
#[test]
fn source_fs_disable_tracking_stops_tracking() {
    let snapshot = plain_snapshot(&[("/src/foo.ts", "content"), ("/src/bar.ts", "content")]);
    let source_fs = new_source_fs(true /* tracking */, snapshot, to_path());

    // Read foo while tracking
    source_fs.get_file("/src/foo.ts");
    assert!(source_fs.seen_file(&p("/src/foo.ts")));

    // Disable tracking
    source_fs.disable_tracking();

    // Read bar after tracking disabled
    source_fs.get_file("/src/bar.ts");
    assert!(!source_fs.seen_file(&p("/src/bar.ts")));
}

// PORT: not in Go. `SourceFS::without_tracking` (H3) tracks no read in
// its callback, and the tracking comes back after it, also when the
// callback panics.
#[test]
fn source_fs_without_tracking_restores_tracking_after_a_panic() {
    let snapshot = plain_snapshot(&[("/src/foo.ts", "content"), ("/src/bar.ts", "content")]);
    let source_fs = new_source_fs(true /* tracking */, snapshot, to_path());

    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        source_fs.without_tracking(|| {
            source_fs.get_file("/src/foo.ts");
            panic!("the callback panics");
        })
    }));
    assert!(caught.is_err());
    assert!(!source_fs.seen_file(&p("/src/foo.ts")));

    // The tracking is on again.
    source_fs.get_file("/src/bar.ts");
    assert!(source_fs.seen_file(&p("/src/bar.ts")));
}

// Go: snapshotfs_test.go:781 TestSourceFS/FileExists returns true for files in source
#[test]
fn source_fs_file_exists_returns_true_for_files_in_source() {
    let snapshot = plain_snapshot(&[("/src/foo.ts", "content")]);
    let source_fs = new_source_fs(false /* tracking */, snapshot, to_path());

    assert!(Fs::file_exists(&*source_fs, "/src/foo.ts"));
    assert!(!Fs::file_exists(&*source_fs, "/src/nonexistent.ts"));
}

// Go: snapshotfs_test.go:802 TestSourceFS/ReadFile returns content for files in source
#[test]
fn source_fs_read_file_returns_content_for_files_in_source() {
    let snapshot = plain_snapshot(&[("/src/foo.ts", "file content")]);
    let source_fs = new_source_fs(false /* tracking */, snapshot, to_path());

    let (content, ok) = Fs::read_file(&*source_fs, "/src/foo.ts");
    assert!(ok);
    assert_eq!(content, "file content");

    let (_, ok) = Fs::read_file(&*source_fs, "/src/nonexistent.ts");
    assert!(!ok);
}

// ---------------------------------------------------------------------------
// TestAutoImportBuilderFS
// ---------------------------------------------------------------------------

// Go: snapshotfs_test.go:842 TestAutoImportBuilderFS/symlink cache mismatch: file cached at symlink path, missed at realpath after deletion
#[test]
fn auto_import_builder_fs_symlink_cache_mismatch() {
    // Create a VFS with a real file and a symlinked directory pointing to it.
    let test_fs = any_fs(
        vec![
            (
                "/real/pkg/index.d.ts",
                text("export declare const x: number;"),
            ),
            ("/project/node_modules/pkg", vfstest::symlink("/real/pkg")),
        ],
        true, /* useCaseSensitiveFileNames */
    );

    // Verify symlink works as expected
    let symlink_path = "/project/node_modules/pkg/index.d.ts";
    let realpath_path = test_fs.realpath(symlink_path);
    assert_eq!(
        realpath_path, "/real/pkg/index.d.ts",
        "Realpath should resolve the symlink to the real path"
    );

    let b = empty_builder(test_fs.clone());

    let auto_import_fs = AutoImportBuilderFS {
        snapshot_fs_builder: b,
        untracked_files: RefCell::default(),
    };

    // Step 1: Read the file via its symlink path.
    let fh = auto_import_fs.get_file(symlink_path);
    assert_eq!(file_content(fh), "export declare const x: number;");

    // Step 2: Simulate a file deletion from disk.
    test_fs.remove("/real/pkg/index.d.ts").unwrap();

    // Step 3: Request the file by its realpath.
    let fh2 = auto_import_fs.get_file(&realpath_path);
    assert!(
        fh2.is_none(),
        "File should be nil when accessed by realpath after deletion from disk"
    );
}

// ---------------------------------------------------------------------------
// TestRealpathAliasLifecycle
// ---------------------------------------------------------------------------

// Go: snapshotfs_test.go:901 TestRealpathAliasLifecycle/alias recorded when reading symlinked node_modules file
#[test]
fn alias_recorded_when_reading_symlinked_node_modules_file() {
    let b = empty_builder(any_fs(
        vec![
            (
                "/project/node_modules/mylib",
                vfstest::symlink("/packages/mylib"),
            ),
            (
                "/packages/mylib/package.json",
                text(r#"{"name": "mylib", "main": "index.js"}"#),
            ),
            (
                "/packages/mylib/index.d.ts",
                text("export declare const x: number;"),
            ),
            (
                "/project/node_modules/nolink/package.json",
                text(r#"{"name": "nolink"}"#),
            ),
        ],
        false,
    ));

    // Read a file through the symlink — should record an alias.
    assert_eq!(
        file_content(b.get_file("/project/node_modules/mylib/package.json")),
        r#"{"name": "mylib", "main": "index.js"}"#
    );

    // Read a non-symlinked node_modules file — should NOT record an alias.
    assert!(
        b.get_file("/project/node_modules/nolink/package.json")
            .is_some()
    );

    let (snapshot, _) = b.finalize();

    // Alias exists for the symlinked file.
    assert!(
        snapshot
            .node_modules_realpath_aliases
            .contains_key(&p("/packages/mylib/package.json")),
        "alias should exist for realpath of symlinked file"
    );
    assert!(alias_has(
        &snapshot.node_modules_realpath_aliases,
        "/packages/mylib/package.json",
        "/project/node_modules/mylib/package.json"
    ));

    // No alias for the non-symlinked file.
    assert!(
        !snapshot
            .node_modules_realpath_aliases
            .contains_key(&p("/project/node_modules/nolink/package.json")),
        "no alias should exist for non-symlinked file"
    );
}

// Go: snapshotfs_test.go:942 TestRealpathAliasLifecycle/no alias recorded for files outside node_modules
#[test]
fn no_alias_recorded_for_files_outside_node_modules() {
    let b = empty_builder(any_fs(
        vec![
            ("/project/link", vfstest::symlink("/elsewhere")),
            ("/elsewhere/index.ts", text("export const x = 1;")),
        ],
        false,
    ));

    assert!(b.get_file("/project/link/index.ts").is_some());

    let (snapshot, _) = b.finalize();
    assert_eq!(
        snapshot.node_modules_realpath_aliases.len(),
        0,
        "no aliases for non-node_modules symlinks"
    );
}

/// The file system of the "one symlink to mylib" subtests.
fn mylib_fs(extra: Vec<(&'static str, MapFile)>) -> Rc<dyn Fs> {
    let mut entries = vec![
        (
            "/project/node_modules/mylib",
            vfstest::symlink("/packages/mylib"),
        ),
        ("/packages/mylib/package.json", text(r#"{"name": "mylib"}"#)),
    ];
    entries.extend(extra);
    any_fs(entries, false)
}

/// Go `newSnapshotFSBuilder(fs, {}, {}, prev.cacheFiles, prev.cacheDirectories, prev.nodeModulesRealpathAliases, ...)`.
fn next_builder(fs: Rc<dyn Fs>, prev: &SnapshotFS) -> Rc<SnapshotFSBuilder> {
    builder(
        fs,
        Overlays::default(),
        prev.cache_files.clone(),
        prev.cache_directories.clone(),
        prev.node_modules_realpath_aliases.clone(),
    )
}

// Go: snapshotfs_test.go:967 TestRealpathAliasLifecycle/aliases carried over across snapshots
#[test]
fn aliases_carried_over_across_snapshots() {
    let test_fs = mylib_fs(Vec::new());

    // Build first snapshot.
    let builder1 = empty_builder(test_fs.clone());
    builder1.get_file("/project/node_modules/mylib/package.json");
    let (snapshot1, _) = builder1.finalize();

    // Build second snapshot from the first, without reading the file again.
    let builder2 = next_builder(test_fs, &snapshot1);
    let (snapshot2, _) = builder2.finalize();

    // Alias should still be present.
    assert!(
        snapshot2
            .node_modules_realpath_aliases
            .contains_key(&p("/packages/mylib/package.json")),
        "alias should survive across snapshots"
    );
    assert!(alias_has(
        &snapshot2.node_modules_realpath_aliases,
        "/packages/mylib/package.json",
        "/project/node_modules/mylib/package.json"
    ));
}

// Go: snapshotfs_test.go:1007 TestRealpathAliasLifecycle/alias pruned when symlinked file is deleted
#[test]
fn alias_pruned_when_symlinked_file_is_deleted() {
    let test_fs = mylib_fs(vec![(
        "/packages/mylib/index.d.ts",
        text("export declare const x: number;"),
    )]);

    // Build first snapshot — read both files.
    let builder1 = empty_builder(test_fs.clone());
    builder1.get_file("/project/node_modules/mylib/package.json");
    builder1.get_file("/project/node_modules/mylib/index.d.ts");
    let (snapshot1, _) = builder1.finalize();

    // Both should be aliased under the same realpath directory but separate files.
    assert!(
        snapshot1
            .node_modules_realpath_aliases
            .contains_key(&p("/packages/mylib/package.json"))
    );
    assert!(
        snapshot1
            .node_modules_realpath_aliases
            .contains_key(&p("/packages/mylib/index.d.ts"))
    );

    // Build second snapshot — delete one file via markDirtyFiles.
    let builder2 = next_builder(test_fs, &snapshot1);

    // Simulate deletion of index.d.ts from the disk file cache.
    delete_cache_file(&builder2, "/project/node_modules/mylib/index.d.ts");

    let (snapshot2, _) = builder2.finalize();

    // package.json alias should remain.
    assert!(
        snapshot2
            .node_modules_realpath_aliases
            .contains_key(&p("/packages/mylib/package.json")),
        "package.json alias should survive"
    );
    assert!(alias_has(
        &snapshot2.node_modules_realpath_aliases,
        "/packages/mylib/package.json",
        "/project/node_modules/mylib/package.json"
    ));

    // index.d.ts alias should be fully pruned (empty set → removed from map).
    assert!(
        !snapshot2
            .node_modules_realpath_aliases
            .contains_key(&p("/packages/mylib/index.d.ts")),
        "index.d.ts alias should be pruned after deletion"
    );
}

fn two_symlink_fs() -> Rc<dyn Fs> {
    mylib_fs(vec![(
        "/project/node_modules/alias",
        vfstest::symlink("/packages/mylib"),
    )])
}

// Go: snapshotfs_test.go:1066 TestRealpathAliasLifecycle/multiple symlinks to same realpath
#[test]
fn multiple_symlinks_to_same_realpath() {
    let b = empty_builder(two_symlink_fs());

    // Read via both symlinks.
    assert!(
        b.get_file("/project/node_modules/mylib/package.json")
            .is_some()
    );
    assert!(
        b.get_file("/project/node_modules/alias/package.json")
            .is_some()
    );

    let (snapshot, _) = b.finalize();

    let aliases = &snapshot.node_modules_realpath_aliases;
    assert!(
        aliases.contains_key(&p("/packages/mylib/package.json")),
        "alias should exist"
    );
    assert!(alias_has(
        aliases,
        "/packages/mylib/package.json",
        "/project/node_modules/mylib/package.json"
    ));
    assert!(alias_has(
        aliases,
        "/packages/mylib/package.json",
        "/project/node_modules/alias/package.json"
    ));
}

// Go: snapshotfs_test.go:1099 TestRealpathAliasLifecycle/multiple symlinks pruned individually
#[test]
fn multiple_symlinks_pruned_individually() {
    let test_fs = two_symlink_fs();

    // Build first snapshot – read via both symlinks.
    let builder1 = empty_builder(test_fs.clone());
    builder1.get_file("/project/node_modules/mylib/package.json");
    builder1.get_file("/project/node_modules/alias/package.json");
    let (snapshot1, _) = builder1.finalize();

    // Build second snapshot – delete ONE of the symlink disk entries.
    let builder2 = next_builder(test_fs, &snapshot1);
    delete_cache_file(&builder2, "/project/node_modules/alias/package.json");
    let (snapshot2, _) = builder2.finalize();

    // The realpath alias set should still exist, but only contain the surviving symlink.
    let aliases = &snapshot2.node_modules_realpath_aliases;
    assert!(
        aliases.contains_key(&p("/packages/mylib/package.json")),
        "alias set should still exist"
    );
    assert!(
        alias_has(
            aliases,
            "/packages/mylib/package.json",
            "/project/node_modules/mylib/package.json"
        ),
        "surviving symlink should remain"
    );
    assert!(
        !alias_has(
            aliases,
            "/packages/mylib/package.json",
            "/project/node_modules/alias/package.json"
        ),
        "deleted symlink should be pruned"
    );
}

// Go: snapshotfs_test.go:1145 TestRealpathAliasLifecycle/expandRealpathAliases expands change events
#[test]
fn expand_realpath_aliases_expands_change_events() {
    let b = empty_builder(mylib_fs(Vec::new()));
    b.get_file("/project/node_modules/mylib/package.json");
    let (snapshot, _) = b.finalize();

    // Simulate a watch event on the REALPATH.
    let mut change = FileChangeSummary::default();
    change
        .changed
        .insert(uri("file:///packages/mylib/package.json"));

    let expanded = snapshot.expand_realpath_aliases(change);

    // Should now also contain the symlink path.
    assert!(
        expanded
            .changed
            .contains(&uri("file:///packages/mylib/package.json")),
        "original event should remain"
    );
    assert!(
        expanded
            .changed
            .contains(&uri("file:///project/node_modules/mylib/package.json")),
        "symlink event should be added"
    );
}

// Go: snapshotfs_test.go:1176 TestRealpathAliasLifecycle/expandRealpathAliases expands delete events
#[test]
fn expand_realpath_aliases_expands_delete_events() {
    let b = empty_builder(mylib_fs(Vec::new()));
    b.get_file("/project/node_modules/mylib/package.json");
    let (snapshot, _) = b.finalize();

    // Simulate a delete watch event on the REALPATH.
    let mut change = FileChangeSummary::default();
    change
        .deleted
        .insert(uri("file:///packages/mylib/package.json"));

    let expanded = snapshot.expand_realpath_aliases(change);

    assert!(
        expanded
            .deleted
            .contains(&uri("file:///project/node_modules/mylib/package.json")),
        "symlink deletion should be added"
    );
}

// Go: snapshotfs_test.go:1205 TestRealpathAliasLifecycle/expandRealpathAliases is a no-op with no aliases
// PORT: Go leaves the other SnapshotFS fields nil; they are empty here.
#[test]
fn expand_realpath_aliases_is_a_no_op_with_no_aliases() {
    let snapshot = plain_snapshot(&[]);

    let mut change = FileChangeSummary::default();
    change.changed.insert(uri("file:///some/file.ts"));

    let expanded = snapshot.expand_realpath_aliases(change);
    assert_eq!(expanded.changed.len(), 1);
    assert!(expanded.changed.contains(&uri("file:///some/file.ts")));
}

// Go: snapshotfs_test.go:1267 TestRealpathAliasLifecycle/markDirtyFiles invalidates symlinked file via realpath event
#[test]
fn mark_dirty_files_invalidates_symlinked_file_via_realpath_event() {
    let test_fs = any_fs(
        vec![
            (
                "/project/node_modules/mylib",
                vfstest::symlink("/packages/mylib"),
            ),
            (
                "/packages/mylib/package.json",
                text(r#"{"name": "mylib", "main": "index.js"}"#),
            ),
        ],
        false,
    );

    // Build first snapshot — read the symlinked file.
    let builder1 = empty_builder(test_fs.clone());
    assert_eq!(
        file_content(builder1.get_file("/project/node_modules/mylib/package.json")),
        r#"{"name": "mylib", "main": "index.js"}"#
    );
    let (snapshot1, _) = builder1.finalize();

    // Modify the real file on disk.
    test_fs
        .write_file("/packages/mylib/package.json", r#"{"name": "mylib"}"#)
        .unwrap();

    // Build second snapshot — simulate realpath change event, expanded via aliases.
    let builder2 = next_builder(test_fs, &snapshot1);

    let mut change = FileChangeSummary::default();
    change
        .changed
        .insert(uri("file:///packages/mylib/package.json"));

    // Expand the realpath event to include the symlink path.
    let change = snapshot1.expand_realpath_aliases(change);
    // Now mark dirty — should find the file under the symlink key.
    builder2.mark_dirty_files(change);

    // Trigger reload by reading the file (simulates program construction).
    assert_eq!(
        file_content(builder2.get_file("/project/node_modules/mylib/package.json")),
        r#"{"name": "mylib"}"#,
        "builder should serve updated content after dirty marking"
    );

    let (snapshot2, _) = builder2.finalize();

    // The file should have been reloaded with new content.
    let file = snapshot2
        .cache_files
        .get(&p("/project/node_modules/mylib/package.json"))
        .expect("file should still be in cacheFiles");
    assert_eq!(
        file.content(),
        r#"{"name": "mylib"}"#,
        "content should be updated"
    );
}

// Go: snapshotfs_test.go:1280 TestRealpathAliasLifecycle/alias clone isolation between snapshots
#[test]
fn alias_clone_isolation_between_snapshots() {
    let test_fs = mylib_fs(vec![
        (
            "/project/node_modules/other",
            vfstest::symlink("/packages/other"),
        ),
        ("/packages/other/package.json", text(r#"{"name": "other"}"#)),
    ]);

    // Build first snapshot — read only mylib.
    let builder1 = empty_builder(test_fs.clone());
    builder1.get_file("/project/node_modules/mylib/package.json");
    let (snapshot1, _) = builder1.finalize();

    // Build second snapshot — also read other.
    let builder2 = next_builder(test_fs, &snapshot1);
    builder2.get_file("/project/node_modules/other/package.json");
    let (snapshot2, _) = builder2.finalize();

    // snapshot1 should only have mylib alias.
    assert!(
        snapshot1
            .node_modules_realpath_aliases
            .contains_key(&p("/packages/mylib/package.json")),
        "snapshot1 should have mylib alias"
    );
    assert!(
        !snapshot1
            .node_modules_realpath_aliases
            .contains_key(&p("/packages/other/package.json")),
        "snapshot1 should NOT have other alias — it was added in a later snapshot"
    );

    // snapshot2 should have both.
    assert!(
        snapshot2
            .node_modules_realpath_aliases
            .contains_key(&p("/packages/mylib/package.json")),
        "snapshot2 should have mylib alias"
    );
    assert!(
        snapshot2
            .node_modules_realpath_aliases
            .contains_key(&p("/packages/other/package.json")),
        "snapshot2 should have other alias"
    );
}

// Go: snapshotfs_test.go:1330 TestRealpathAliasLifecycle/adding symlink to inherited realpath key does not mutate previous snapshot
#[test]
fn adding_symlink_to_inherited_realpath_key_does_not_mutate_previous_snapshot() {
    let test_fs = two_symlink_fs();

    // Snapshot 1: read via one symlink only.
    let builder1 = empty_builder(test_fs.clone());
    builder1.get_file("/project/node_modules/mylib/package.json");
    let (snapshot1, _) = builder1.finalize();

    // Verify snapshot1 has exactly one alias for the realpath.
    let aliases1 = snapshot1
        .node_modules_realpath_aliases
        .get(&p("/packages/mylib/package.json"))
        .expect("alias set")
        .clone();
    assert_eq!(aliases1.borrow().paths.len(), 1);
    assert!(
        aliases1
            .borrow()
            .paths
            .contains_key(&p("/project/node_modules/mylib/package.json"))
    );

    // Snapshot 2: read via the SECOND symlink, which maps to the same realpath.
    let builder2 = next_builder(test_fs, &snapshot1);
    builder2.get_file("/project/node_modules/alias/package.json");
    let (snapshot2, _) = builder2.finalize();

    // Snapshot 2 should have both symlinks.
    let aliases2 = snapshot2
        .node_modules_realpath_aliases
        .get(&p("/packages/mylib/package.json"))
        .expect("alias set");
    assert_eq!(aliases2.borrow().paths.len(), 2);
    assert!(
        aliases2
            .borrow()
            .paths
            .contains_key(&p("/project/node_modules/mylib/package.json"))
    );
    assert!(
        aliases2
            .borrow()
            .paths
            .contains_key(&p("/project/node_modules/alias/package.json"))
    );

    // Snapshot 1 must NOT have been mutated — it should still have only one alias.
    assert_eq!(
        aliases1.borrow().paths.len(),
        1,
        "snapshot1 alias set must not be mutated by snapshot2"
    );
    assert!(
        !aliases1
            .borrow()
            .paths
            .contains_key(&p("/project/node_modules/alias/package.json")),
        "snapshot1 must not contain alias added in snapshot2"
    );
}

// ---------------------------------------------------------------------------
// TestExpandAndFilterWatchEvents
// ---------------------------------------------------------------------------

// Go: snapshotfs_test.go:1408 TestExpandAndFilterWatchEvents/preserves node_modules directory deletion even when untracked
#[test]
fn preserves_node_modules_directory_deletion_even_when_untracked() {
    let b = empty_builder(text_fs(
        &[("/project/index.ts", "export const x = 1;")],
        false,
    ));

    let mut change = FileChangeSummary::default();
    change.deleted.insert(uri("file:///project/node_modules"));

    let expanded =
        b.expand_and_filter_watch_events(change, &[], None, &IndexMap::new(), &IndexMap::new());
    assert!(
        expanded
            .deleted
            .contains(&uri("file:///project/node_modules")),
        "bare node_modules directory deletion should be preserved"
    );
}

// Go: snapshotfs_test.go:1425 TestExpandAndFilterWatchEvents/preserves deletion of a package directory inside node_modules
#[test]
fn preserves_deletion_of_a_package_directory_inside_node_modules() {
    let b = empty_builder(text_fs(
        &[("/project/index.ts", "export const x = 1;")],
        false,
    ));

    let mut change = FileChangeSummary::default();
    change
        .deleted
        .insert(uri("file:///project/node_modules/@scope/pkg"));

    let expanded =
        b.expand_and_filter_watch_events(change, &[], None, &IndexMap::new(), &IndexMap::new());
    assert!(
        expanded
            .deleted
            .contains(&uri("file:///project/node_modules/@scope/pkg")),
        "package directory deletion inside node_modules should be preserved"
    );
}

// Go: snapshotfs_test.go:1439 TestExpandAndFilterWatchEvents/drops irrelevant untracked deletion outside node_modules
#[test]
fn drops_irrelevant_untracked_deletion_outside_node_modules() {
    let b = empty_builder(text_fs(
        &[("/project/index.ts", "export const x = 1;")],
        false,
    ));

    let mut change = FileChangeSummary::default();
    change.deleted.insert(uri("file:///project/build"));

    let expanded =
        b.expand_and_filter_watch_events(change, &[], None, &IndexMap::new(), &IndexMap::new());
    assert_eq!(
        expanded.deleted.len(),
        0,
        "untracked non-node_modules directory deletion should be dropped"
    );
}

// Go: snapshotfs_test.go:1500 TestExpandAndFilterWatchEvents/preserves exact content mapper dependencies (tsgo#4712)
#[test]
fn preserves_exact_content_mapper_dependencies() {
    let b = empty_builder(text_fs(
        &[("/project/index.ts", "export const x = 1;")],
        false,
    ));
    let watched: FxHashSet<Path> = [p("/project/mapper.config")].into_iter().collect();
    let mut change = FileChangeSummary::default();
    change.changed.insert(uri("file:///project/mapper.config"));
    change.deleted.insert(uri("file:///project/mapper.config"));

    let expanded = b.expand_and_filter_watch_events(
        change,
        &[],
        Some(&watched),
        &IndexMap::new(),
        &IndexMap::new(),
    );
    assert!(
        expanded
            .changed
            .contains(&uri("file:///project/mapper.config"))
    );
    assert!(
        expanded
            .deleted
            .contains(&uri("file:///project/mapper.config"))
    );
}

// Go: snapshotfs_test.go:1453 TestExpandAndFilterWatchEvents/expands tracked directory deletion into file deletions
#[test]
fn expands_tracked_directory_deletion_into_file_deletions() {
    let b = builder(
        text_fs(&[("/src/foo.ts", "const foo = 1;")], false),
        Overlays::default(),
        cache_files(&[("/src/foo.ts", "const foo = 1;")]),
        dirs(&[
            ("/", &[("/src", "src")]),
            ("/src", &[("/src/foo.ts", "foo.ts")]),
        ]),
        Aliases::default(),
    );

    let mut change = FileChangeSummary::default();
    change.deleted.insert(uri("file:///src"));

    let expanded =
        b.expand_and_filter_watch_events(change, &[], None, &IndexMap::new(), &IndexMap::new());
    assert!(
        expanded.deleted.contains(&uri("file:///src/foo.ts")),
        "tracked directory deletion should expand to contained file deletions"
    );
    assert!(
        !expanded.deleted.contains(&uri("file:///src")),
        "the directory URI itself should be replaced by its files"
    );
}

// PORT: no Go counterpart (editfuzz2 R1). Go ranges over a small directory
// map, which gives a rotation of its insertion order. The port keeps
// insertion order: the open files of a directory come in the order they
// were opened, and its cached files in the order they were read. The order
// of the path hashes gave orders that Go never gives.
#[test]
fn accessible_entries_keep_the_open_order_and_the_read_order() {
    let names: Vec<String> = [5, 2, 7, 1, 8, 3, 6, 4]
        .iter()
        .map(|i| format!("o{i}.ts"))
        .collect();
    let paths: Vec<String> = names.iter().map(|name| format!("/src/{name}")).collect();
    let files: Vec<(&str, &str)> = paths
        .iter()
        .map(|path| (path.as_str(), "export {};"))
        .collect();

    let open = builder(
        text_fs(&[], false),
        overlays(&files),
        CacheFiles::default(),
        Dirs::default(),
        Aliases::default(),
    );
    assert_eq!(open.get_accessible_entries("/src").files, names);

    let read = empty_builder(text_fs(&files, false));
    for (path, _) in &files {
        assert!(read.get_file(path).is_some());
    }
    let (snapshot, _) = read.finalize();
    assert_eq!(snapshot.get_accessible_entries("/src").files, names);
}
