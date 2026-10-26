//! Go: `internal/tspath/{path,ignoredpaths,startsWithDirectory,untitled,extensionrewrite}_test.go`.
//!
//! Blocked: `TestReducePathComponents` (Go `reducePathComponents`; the Rust
//! `reduce_path_components` in `src/frontend/tspath/path.rs` is private).
//! Go benchmarks and fuzz tests are not ported.

use ts_goport::frontend::tspath::{self, ComparePathsOptions};

use super::Failures;

// Go: path_test.go:12 TestNormalizeSlashes
#[test]
fn test_normalize_slashes() {
    assert_eq!(tspath::normalize_slashes("a"), "a");
    assert_eq!(tspath::normalize_slashes("a/b"), "a/b");
    assert_eq!(tspath::normalize_slashes("a\\b"), "a/b");
    assert_eq!(
        tspath::normalize_slashes("\\\\server\\path"),
        "//server/path"
    );
}

// Go: path_test.go:20 TestGetRootLength
#[test]
fn test_get_root_length() {
    assert_eq!(tspath::get_root_length("a"), 0);
    assert_eq!(tspath::get_root_length("/"), 1);
    assert_eq!(tspath::get_root_length("/path"), 1);
    assert_eq!(tspath::get_root_length("c:"), 2);
    assert_eq!(tspath::get_root_length("c:d"), 0);
    assert_eq!(tspath::get_root_length("c:/"), 3);
    assert_eq!(tspath::get_root_length("c:\\"), 3);
    assert_eq!(tspath::get_root_length("//server"), 8);
    assert_eq!(tspath::get_root_length("//server/share"), 9);
    assert_eq!(tspath::get_root_length("\\\\server"), 8);
    assert_eq!(tspath::get_root_length("\\\\server\\share"), 9);
    assert_eq!(tspath::get_root_length("file:///"), 8);
    assert_eq!(tspath::get_root_length("file:///path"), 8);
    assert_eq!(tspath::get_root_length("file:///c:"), 10);
    assert_eq!(tspath::get_root_length("file:///c:d"), 8);
    assert_eq!(tspath::get_root_length("file:///c:/path"), 11);
    assert_eq!(tspath::get_root_length("file:///c%3a"), 12);
    assert_eq!(tspath::get_root_length("file:///c%3ad"), 8);
    assert_eq!(tspath::get_root_length("file:///c%3a/path"), 13);
    assert_eq!(tspath::get_root_length("file:///c%3A"), 12);
    assert_eq!(tspath::get_root_length("file:///c%3Ad"), 8);
    assert_eq!(tspath::get_root_length("file:///c%3A/path"), 13);
    assert_eq!(tspath::get_root_length("file://localhost"), 16);
    assert_eq!(tspath::get_root_length("file://localhost/"), 17);
    assert_eq!(tspath::get_root_length("file://localhost/path"), 17);
    assert_eq!(tspath::get_root_length("file://localhost/c:"), 19);
    assert_eq!(tspath::get_root_length("file://localhost/c:d"), 17);
    assert_eq!(tspath::get_root_length("file://localhost/c:/path"), 20);
    assert_eq!(tspath::get_root_length("file://localhost/c%3a"), 21);
    assert_eq!(tspath::get_root_length("file://localhost/c%3ad"), 17);
    assert_eq!(tspath::get_root_length("file://localhost/c%3a/path"), 22);
    assert_eq!(tspath::get_root_length("file://localhost/c%3A"), 21);
    assert_eq!(tspath::get_root_length("file://localhost/c%3Ad"), 17);
    assert_eq!(tspath::get_root_length("file://localhost/c%3A/path"), 22);
    // ts#64544: the file scheme and localhost compare without case.
    assert_eq!(tspath::get_root_length("FILE:///C:/path"), 11);
    assert_eq!(tspath::get_root_length("file://LOCALHOST/C%3A/path"), 22);
    assert_eq!(tspath::get_root_length("file://server"), 13);
    assert_eq!(tspath::get_root_length("file://server/"), 14);
    assert_eq!(tspath::get_root_length("file://server/path"), 14);
    assert_eq!(tspath::get_root_length("file://server/c:"), 14);
    assert_eq!(tspath::get_root_length("file://server/c:d"), 14);
    assert_eq!(tspath::get_root_length("file://server/c:/d"), 14);
    assert_eq!(tspath::get_root_length("file://server/c%3a"), 14);
    assert_eq!(tspath::get_root_length("file://server/c%3ad"), 14);
    assert_eq!(tspath::get_root_length("file://server/c%3a/d"), 14);
    assert_eq!(tspath::get_root_length("file://server/c%3A"), 14);
    assert_eq!(tspath::get_root_length("file://server/c%3Ad"), 14);
    assert_eq!(tspath::get_root_length("file://server/c%3A/d"), 14);
    assert_eq!(tspath::get_root_length("http://server"), 13);
    assert_eq!(tspath::get_root_length("http://server/path"), 14);
}

// Go: path_test.go:72 TestPathIsAbsolute
#[test]
fn test_path_is_absolute() {
    // POSIX
    assert_eq!(tspath::path_is_absolute("/path/to/file.ext"), true);
    // DOS
    assert_eq!(tspath::path_is_absolute("c:/path/to/file.ext"), true);
    // URL
    assert_eq!(tspath::path_is_absolute("file:///path/to/file.ext"), true);
    // Non-absolute
    assert_eq!(tspath::path_is_absolute("path/to/file.ext"), false);
    assert_eq!(tspath::path_is_absolute("./path/to/file.ext"), false);
}

// Go: path_test.go:85 TestIsUrl
#[test]
fn test_is_url() {
    assert_eq!(tspath::is_url("a"), false);
    assert_eq!(tspath::is_url("/"), false);
    assert_eq!(tspath::is_url("c:"), false);
    assert_eq!(tspath::is_url("c:d"), false);
    assert_eq!(tspath::is_url("c:/"), false);
    assert_eq!(tspath::is_url("c:\\"), false);
    assert_eq!(tspath::is_url("//server"), false);
    assert_eq!(tspath::is_url("//server/share"), false);
    assert_eq!(tspath::is_url("\\\\server"), false);
    assert_eq!(tspath::is_url("\\\\server\\share"), false);

    assert_eq!(tspath::is_url("file:///path"), true);
    assert_eq!(tspath::is_url("file:///c:"), true);
    assert_eq!(tspath::is_url("file:///c:d"), true);
    assert_eq!(tspath::is_url("file:///c:/path"), true);
    assert_eq!(tspath::is_url("file://server"), true);
    assert_eq!(tspath::is_url("file://server/path"), true);
    assert_eq!(tspath::is_url("http://server"), true);
    assert_eq!(tspath::is_url("http://server/path"), true);
}

// Go: path_test.go:108 TestIsRootedDiskPath
#[test]
fn test_is_rooted_disk_path() {
    assert_eq!(tspath::is_rooted_disk_path("a"), false);
    assert_eq!(tspath::is_rooted_disk_path("/"), true);
    assert_eq!(tspath::is_rooted_disk_path("c:"), true);
    assert_eq!(tspath::is_rooted_disk_path("c:d"), false);
    assert_eq!(tspath::is_rooted_disk_path("c:/"), true);
    assert_eq!(tspath::is_rooted_disk_path("c:\\"), true);
    assert_eq!(tspath::is_rooted_disk_path("//server"), true);
    assert_eq!(tspath::is_rooted_disk_path("//server/share"), true);
    assert_eq!(tspath::is_rooted_disk_path("\\\\server"), true);
    assert_eq!(tspath::is_rooted_disk_path("\\\\server\\share"), true);
    assert_eq!(tspath::is_rooted_disk_path("file:///path"), false);
    assert_eq!(tspath::is_rooted_disk_path("file:///c:"), false);
    assert_eq!(tspath::is_rooted_disk_path("file:///c:d"), false);
    assert_eq!(tspath::is_rooted_disk_path("file:///c:/path"), false);
    assert_eq!(tspath::is_rooted_disk_path("file://server"), false);
    assert_eq!(tspath::is_rooted_disk_path("file://server/path"), false);
    assert_eq!(tspath::is_rooted_disk_path("http://server"), false);
    assert_eq!(tspath::is_rooted_disk_path("http://server/path"), false);
}

// Go: path_test.go:130 TestGetDirectoryPath
#[test]
fn test_get_directory_path() {
    assert_eq!(tspath::get_directory_path(""), "");
    assert_eq!(tspath::get_directory_path("a"), "");
    assert_eq!(tspath::get_directory_path("a/b"), "a");
    assert_eq!(tspath::get_directory_path("/"), "/");
    assert_eq!(tspath::get_directory_path("/a"), "/");
    assert_eq!(tspath::get_directory_path("/a/"), "/");
    assert_eq!(tspath::get_directory_path("/a/b"), "/a");
    assert_eq!(tspath::get_directory_path("/a/b/"), "/a");
    assert_eq!(tspath::get_directory_path("c:"), "c:");
    assert_eq!(tspath::get_directory_path("c:d"), "");
    assert_eq!(tspath::get_directory_path("c:/"), "c:/");
    assert_eq!(tspath::get_directory_path("c:/path"), "c:/");
    assert_eq!(tspath::get_directory_path("c:/path/"), "c:/");
    assert_eq!(tspath::get_directory_path("//server"), "//server");
    assert_eq!(tspath::get_directory_path("//server/"), "//server/");
    assert_eq!(tspath::get_directory_path("//server/share"), "//server/");
    assert_eq!(tspath::get_directory_path("//server/share/"), "//server/");
    assert_eq!(tspath::get_directory_path("\\\\server"), "//server");
    assert_eq!(tspath::get_directory_path("\\\\server\\"), "//server/");
    assert_eq!(tspath::get_directory_path("\\\\server\\share"), "//server/");
    assert_eq!(
        tspath::get_directory_path("\\\\server\\share\\"),
        "//server/"
    );
    assert_eq!(tspath::get_directory_path("file:///"), "file:///");
    assert_eq!(tspath::get_directory_path("file:///path"), "file:///");
    assert_eq!(tspath::get_directory_path("file:///path/"), "file:///");
    assert_eq!(tspath::get_directory_path("file:///c:"), "file:///c:");
    assert_eq!(tspath::get_directory_path("file:///c:d"), "file:///");
    assert_eq!(tspath::get_directory_path("file:///c:/"), "file:///c:/");
    assert_eq!(tspath::get_directory_path("file:///c:/path"), "file:///c:/");
    assert_eq!(
        tspath::get_directory_path("file:///c:/path/"),
        "file:///c:/"
    );
    assert_eq!(tspath::get_directory_path("file://server"), "file://server");
    assert_eq!(
        tspath::get_directory_path("file://server/"),
        "file://server/"
    );
    assert_eq!(
        tspath::get_directory_path("file://server/path"),
        "file://server/"
    );
    assert_eq!(
        tspath::get_directory_path("file://server/path/"),
        "file://server/"
    );
    assert_eq!(tspath::get_directory_path("http://server"), "http://server");
    assert_eq!(
        tspath::get_directory_path("http://server/"),
        "http://server/"
    );
    assert_eq!(
        tspath::get_directory_path("http://server/path"),
        "http://server/"
    );
    assert_eq!(
        tspath::get_directory_path("http://server/path/"),
        "http://server/"
    );
}

// Go: path_test.go:171 TestGetLongestExtensionFromPath (tsgo#4712)
#[test]
fn test_get_longest_extension_from_path() {
    let extensions = [".z", ".y.z", ".other"];
    assert_eq!(
        tspath::get_longest_extension_from_path("/src/Component.y.z", &extensions, false),
        ".y.z"
    );
    assert_eq!(
        tspath::get_longest_extension_from_path("/src/Component.z", &extensions, false),
        ".z"
    );
    assert_eq!(
        tspath::get_longest_extension_from_path("/src/Component.y.Z", &extensions, false),
        ""
    );
    assert_eq!(
        tspath::get_longest_extension_from_path("/src/Component.y.Z", &extensions, true),
        ".y.Z"
    );
}

// Go: path_test.go:180 TestRemoveAnyFileExtension (tsgo#4712)
#[test]
fn test_remove_any_file_extension() {
    assert_eq!(
        tspath::remove_any_file_extension("/src/Component.vue"),
        "/src/Component"
    );
    assert_eq!(
        tspath::remove_any_file_extension("/src/Component.d.ts"),
        "/src/Component"
    );
    assert_eq!(
        tspath::remove_any_file_extension("/src/Component"),
        "/src/Component"
    );
}

// Go: path_test.go:175 TestGetPathComponents
#[test]
fn test_get_path_components() {
    assert_eq!(tspath::get_path_components("", ""), vec!["".to_string()]);
    assert_eq!(
        tspath::get_path_components("a", ""),
        vec!["".to_string(), "a".to_string()]
    );
    assert_eq!(
        tspath::get_path_components("./a", ""),
        vec!["".to_string(), ".".to_string(), "a".to_string()]
    );
    assert_eq!(tspath::get_path_components("/", ""), vec!["/".to_string()]);
    assert_eq!(
        tspath::get_path_components("/a", ""),
        vec!["/".to_string(), "a".to_string()]
    );
    assert_eq!(
        tspath::get_path_components("/a/", ""),
        vec!["/".to_string(), "a".to_string()]
    );
    assert_eq!(
        tspath::get_path_components("c:", ""),
        vec!["c:".to_string()]
    );
    assert_eq!(
        tspath::get_path_components("c:d", ""),
        vec!["".to_string(), "c:d".to_string()]
    );
    assert_eq!(
        tspath::get_path_components("c:/", ""),
        vec!["c:/".to_string()]
    );
    assert_eq!(
        tspath::get_path_components("c:/path", ""),
        vec!["c:/".to_string(), "path".to_string()]
    );
    assert_eq!(
        tspath::get_path_components("//server", ""),
        vec!["//server".to_string()]
    );
    assert_eq!(
        tspath::get_path_components("//server/", ""),
        vec!["//server/".to_string()]
    );
    assert_eq!(
        tspath::get_path_components("//server/share", ""),
        vec!["//server/".to_string(), "share".to_string()]
    );
    assert_eq!(
        tspath::get_path_components("file:///", ""),
        vec!["file:///".to_string()]
    );
    assert_eq!(
        tspath::get_path_components("file:///path", ""),
        vec!["file:///".to_string(), "path".to_string()]
    );
    assert_eq!(
        tspath::get_path_components("file:///c:", ""),
        vec!["file:///c:".to_string()]
    );
    assert_eq!(
        tspath::get_path_components("file:///c:d", ""),
        vec!["file:///".to_string(), "c:d".to_string()]
    );
    assert_eq!(
        tspath::get_path_components("file:///c:/", ""),
        vec!["file:///c:/".to_string()]
    );
    assert_eq!(
        tspath::get_path_components("file:///c:/path", ""),
        vec!["file:///c:/".to_string(), "path".to_string()]
    );
    assert_eq!(
        tspath::get_path_components("file://server", ""),
        vec!["file://server".to_string()]
    );
    assert_eq!(
        tspath::get_path_components("file://server/", ""),
        vec!["file://server/".to_string()]
    );
    assert_eq!(
        tspath::get_path_components("file://server/path", ""),
        vec!["file://server/".to_string(), "path".to_string()]
    );
    assert_eq!(
        tspath::get_path_components("http://server", ""),
        vec!["http://server".to_string()]
    );
    assert_eq!(
        tspath::get_path_components("http://server/", ""),
        vec!["http://server/".to_string()]
    );
    assert_eq!(
        tspath::get_path_components("http://server/path", ""),
        vec!["http://server/".to_string(), "path".to_string()]
    );
}

// Go: path_test.go:221 TestCombinePaths
#[test]
fn test_combine_paths() {
    // Non-rooted
    assert_eq!(
        tspath::combine_paths("path", &["to", "file.ext"]),
        "path/to/file.ext"
    );
    assert_eq!(
        tspath::combine_paths("path", &["dir", "..", "to", "file.ext"]),
        "path/dir/../to/file.ext"
    );
    // POSIX
    assert_eq!(
        tspath::combine_paths("/path", &["to", "file.ext"]),
        "/path/to/file.ext"
    );
    assert_eq!(
        tspath::combine_paths("/path", &["/to", "file.ext"]),
        "/to/file.ext"
    );
    // DOS
    assert_eq!(
        tspath::combine_paths("c:/path", &["to", "file.ext"]),
        "c:/path/to/file.ext"
    );
    assert_eq!(
        tspath::combine_paths("c:/path", &["c:/to", "file.ext"]),
        "c:/to/file.ext"
    );
    // URL
    assert_eq!(
        tspath::combine_paths("file:///path", &["to", "file.ext"]),
        "file:///path/to/file.ext"
    );
    assert_eq!(
        tspath::combine_paths("file:///path", &["file:///to", "file.ext"]),
        "file:///to/file.ext"
    );

    assert_eq!(
        tspath::combine_paths("/", &["/node_modules/@types"]),
        "/node_modules/@types"
    );
    assert_eq!(tspath::combine_paths("/a/..", &[""]), "/a/..");
    assert_eq!(tspath::combine_paths("/a/..", &["b"]), "/a/../b");
    assert_eq!(tspath::combine_paths("/a/..", &["b/"]), "/a/../b/");
    assert_eq!(tspath::combine_paths("/a/..", &["/"]), "/");
    assert_eq!(tspath::combine_paths("/a/..", &["/b"]), "/b");
}

// Go: path_test.go:266 TestResolvePath
#[test]
fn test_resolve_path() {
    assert_eq!(tspath::resolve_path("", &[]), "");
    assert_eq!(tspath::resolve_path(".", &[]), "");
    assert_eq!(tspath::resolve_path("./", &[]), "");
    assert_eq!(tspath::resolve_path("..", &[]), "..");
    assert_eq!(tspath::resolve_path("../", &[]), "../");
    assert_eq!(tspath::resolve_path("/", &[]), "/");
    assert_eq!(tspath::resolve_path("/.", &[]), "/");
    assert_eq!(tspath::resolve_path("/./", &[]), "/");
    assert_eq!(tspath::resolve_path("/../", &[]), "/");
    assert_eq!(tspath::resolve_path("/a", &[]), "/a");
    assert_eq!(tspath::resolve_path("/a/", &[]), "/a/");
    assert_eq!(tspath::resolve_path("/a/.", &[]), "/a");
    assert_eq!(tspath::resolve_path("/a/./", &[]), "/a/");
    assert_eq!(tspath::resolve_path("/a/./b", &[]), "/a/b");
    assert_eq!(tspath::resolve_path("/a/./b/", &[]), "/a/b/");
    assert_eq!(tspath::resolve_path("/a/..", &[]), "/");
    assert_eq!(tspath::resolve_path("/a/../", &[]), "/");
    assert_eq!(tspath::resolve_path("/a/../b", &[]), "/b");
    assert_eq!(tspath::resolve_path("/a/../b/", &[]), "/b/");
    assert_eq!(tspath::resolve_path("/a/..", &["b"]), "/b");
    assert_eq!(tspath::resolve_path("/a/..", &["/"]), "/");
    assert_eq!(tspath::resolve_path("/a/..", &["b/"]), "/b/");
    assert_eq!(tspath::resolve_path("/a/..", &["/b"]), "/b");
    assert_eq!(tspath::resolve_path("/a/.", &["b"]), "/a/b");
    assert_eq!(tspath::resolve_path("/a/.", &["."]), "/a");
    assert_eq!(tspath::resolve_path("a", &["b", "c"]), "a/b/c");
    assert_eq!(tspath::resolve_path("a", &["b", "/c"]), "/c");
    assert_eq!(tspath::resolve_path("a", &["b", "../c"]), "a/c");
}

// Go: path_test.go:298 TestGetNormalizedAbsolutePath
#[test]
fn test_get_normalized_absolute_path() {
    assert_eq!(tspath::get_normalized_absolute_path("/", ""), "/");
    assert_eq!(tspath::get_normalized_absolute_path("/.", ""), "/");
    assert_eq!(tspath::get_normalized_absolute_path("/./", ""), "/");
    assert_eq!(tspath::get_normalized_absolute_path("/../", ""), "/");
    assert_eq!(tspath::get_normalized_absolute_path("/a", ""), "/a");
    assert_eq!(tspath::get_normalized_absolute_path("/a/", ""), "/a");
    assert_eq!(tspath::get_normalized_absolute_path("/a/.", ""), "/a");
    assert_eq!(
        tspath::get_normalized_absolute_path("/a/foo.", ""),
        "/a/foo."
    );
    assert_eq!(tspath::get_normalized_absolute_path("/a/./", ""), "/a");
    assert_eq!(tspath::get_normalized_absolute_path("/a/./b", ""), "/a/b");
    assert_eq!(tspath::get_normalized_absolute_path("/a/./b/", ""), "/a/b");
    assert_eq!(tspath::get_normalized_absolute_path("/a/..", ""), "/");
    assert_eq!(tspath::get_normalized_absolute_path("/a/../", ""), "/");
    assert_eq!(tspath::get_normalized_absolute_path("/a/../", ""), "/");
    assert_eq!(tspath::get_normalized_absolute_path("/a/../b", ""), "/b");
    assert_eq!(tspath::get_normalized_absolute_path("/a/../b/", ""), "/b");
    assert_eq!(tspath::get_normalized_absolute_path("/a/..", ""), "/");
    assert_eq!(tspath::get_normalized_absolute_path("/a/..", "/"), "/");
    assert_eq!(tspath::get_normalized_absolute_path("/a/..", "b/"), "/");
    assert_eq!(tspath::get_normalized_absolute_path("/a/..", "/b"), "/");
    assert_eq!(tspath::get_normalized_absolute_path("/a/.", "b"), "/a");
    assert_eq!(tspath::get_normalized_absolute_path("/a/.", "."), "/a");

    // Tests as above, but with backslashes.
    assert_eq!(tspath::get_normalized_absolute_path("\\", ""), "/");
    assert_eq!(tspath::get_normalized_absolute_path("\\.", ""), "/");
    assert_eq!(tspath::get_normalized_absolute_path("\\.\\", ""), "/");
    assert_eq!(tspath::get_normalized_absolute_path("\\..\\", ""), "/");
    assert_eq!(tspath::get_normalized_absolute_path("\\a\\.\\", ""), "/a");
    assert_eq!(
        tspath::get_normalized_absolute_path("\\a\\.\\b", ""),
        "/a/b"
    );
    assert_eq!(
        tspath::get_normalized_absolute_path("\\a\\.\\b\\", ""),
        "/a/b"
    );
    assert_eq!(tspath::get_normalized_absolute_path("\\a\\..", ""), "/");
    assert_eq!(tspath::get_normalized_absolute_path("\\a\\..\\", ""), "/");
    assert_eq!(tspath::get_normalized_absolute_path("\\a\\..\\", ""), "/");
    assert_eq!(tspath::get_normalized_absolute_path("\\a\\..\\b", ""), "/b");
    assert_eq!(
        tspath::get_normalized_absolute_path("\\a\\..\\b\\", ""),
        "/b"
    );
    assert_eq!(tspath::get_normalized_absolute_path("\\a\\..", ""), "/");
    assert_eq!(tspath::get_normalized_absolute_path("\\a\\..", "\\"), "/");
    assert_eq!(tspath::get_normalized_absolute_path("\\a\\..", "b\\"), "/");
    assert_eq!(tspath::get_normalized_absolute_path("\\a\\..", "\\b"), "/");
    assert_eq!(tspath::get_normalized_absolute_path("\\a\\.", "b"), "/a");
    assert_eq!(tspath::get_normalized_absolute_path("\\a\\.", "."), "/a");

    // Relative paths on an empty currentDirectory.
    assert_eq!(tspath::get_normalized_absolute_path("", ""), "");
    assert_eq!(tspath::get_normalized_absolute_path(".", ""), "");
    assert_eq!(tspath::get_normalized_absolute_path("./", ""), "");
    // Strangely, these do not normalize to the empty string.
    assert_eq!(tspath::get_normalized_absolute_path("..", ""), "..");
    assert_eq!(tspath::get_normalized_absolute_path("../", ""), "..");

    // Interaction between relative paths and currentDirectory.
    assert_eq!(tspath::get_normalized_absolute_path("", "/home"), "/home");
    assert_eq!(tspath::get_normalized_absolute_path(".", "/home"), "/home");
    assert_eq!(tspath::get_normalized_absolute_path("./", "/home"), "/home");
    assert_eq!(tspath::get_normalized_absolute_path("..", "/home"), "/");
    assert_eq!(tspath::get_normalized_absolute_path("../", "/home"), "/");
    assert_eq!(tspath::get_normalized_absolute_path("a", "b"), "b/a");
    assert_eq!(tspath::get_normalized_absolute_path("a", "b/c"), "b/c/a");

    // Base names starting or ending with a dot do not affect normalization.
    assert_eq!(tspath::get_normalized_absolute_path(".a", ""), ".a");
    assert_eq!(tspath::get_normalized_absolute_path("..a", ""), "..a");
    assert_eq!(tspath::get_normalized_absolute_path("a.", ""), "a.");
    assert_eq!(tspath::get_normalized_absolute_path("a..", ""), "a..");

    assert_eq!(
        tspath::get_normalized_absolute_path("/base/./.a", ""),
        "/base/.a"
    );
    assert_eq!(
        tspath::get_normalized_absolute_path("/base/../.a", ""),
        "/.a"
    );
    assert_eq!(
        tspath::get_normalized_absolute_path("/base/./..a", ""),
        "/base/..a"
    );
    assert_eq!(
        tspath::get_normalized_absolute_path("/base/../..a", ""),
        "/..a"
    );
    assert_eq!(
        tspath::get_normalized_absolute_path("/base/./..a/b", ""),
        "/base/..a/b"
    );
    assert_eq!(
        tspath::get_normalized_absolute_path("/base/../..a/b", ""),
        "/..a/b"
    );

    assert_eq!(
        tspath::get_normalized_absolute_path("/base/./a.", ""),
        "/base/a."
    );
    assert_eq!(
        tspath::get_normalized_absolute_path("/base/../a.", ""),
        "/a."
    );
    assert_eq!(
        tspath::get_normalized_absolute_path("/base/./a..", ""),
        "/base/a.."
    );
    assert_eq!(
        tspath::get_normalized_absolute_path("/base/../a..", ""),
        "/a.."
    );
    assert_eq!(
        tspath::get_normalized_absolute_path("/base/./a../b", ""),
        "/base/a../b"
    );
    assert_eq!(
        tspath::get_normalized_absolute_path("/base/../a../b", ""),
        "/a../b"
    );

    assert_eq!(tspath::get_normalized_absolute_path("a/..", ""), "");
    assert_eq!(tspath::get_normalized_absolute_path("/a//", ""), "/a");
    assert_eq!(tspath::get_normalized_absolute_path("//a", "a"), "//a/");
    assert_eq!(tspath::get_normalized_absolute_path("/\\", ""), "//");
    assert_eq!(tspath::get_normalized_absolute_path("a///", "a"), "a/a");
    assert_eq!(tspath::get_normalized_absolute_path("/.//", ""), "/");
    assert_eq!(tspath::get_normalized_absolute_path("//\\\\", ""), "///");
    assert_eq!(tspath::get_normalized_absolute_path(".//a", "."), "a");
    assert_eq!(tspath::get_normalized_absolute_path("a/../..", ""), "..");
    assert_eq!(tspath::get_normalized_absolute_path("../..", "\\a"), "/");
    assert_eq!(tspath::get_normalized_absolute_path("a:", "b"), "a:/");
    assert_eq!(
        tspath::get_normalized_absolute_path("a/../..", ".."),
        "../.."
    );
    assert_eq!(tspath::get_normalized_absolute_path("a/../..", "b"), "");
    assert_eq!(
        tspath::get_normalized_absolute_path("a//../..", ".."),
        "../.."
    );

    // Consecutive intermediate slashes are normalized to a single slash.
    assert_eq!(tspath::get_normalized_absolute_path("a//b", ""), "a/b");
    assert_eq!(tspath::get_normalized_absolute_path("a///b", ""), "a/b");
    assert_eq!(tspath::get_normalized_absolute_path("a/b//c", ""), "a/b/c");
    assert_eq!(
        tspath::get_normalized_absolute_path("/a/b//c", ""),
        "/a/b/c"
    );
    assert_eq!(
        tspath::get_normalized_absolute_path("//a/b//c", ""),
        "//a/b/c"
    );

    // Backslashes are converted to slashes,
    // and then consecutive intermediate slashes are normalized to a single slash
    assert_eq!(tspath::get_normalized_absolute_path("a\\\\b", ""), "a/b");
    assert_eq!(tspath::get_normalized_absolute_path("a\\\\\\b", ""), "a/b");
    assert_eq!(
        tspath::get_normalized_absolute_path("a\\b\\\\c", ""),
        "a/b/c"
    );
    assert_eq!(
        tspath::get_normalized_absolute_path("\\a\\b\\\\c", ""),
        "/a/b/c"
    );
    assert_eq!(
        tspath::get_normalized_absolute_path("\\\\a\\b\\\\c", ""),
        "//a/b/c"
    );

    // The same occurs for mixed slashes.
    assert_eq!(tspath::get_normalized_absolute_path("a/\\b", ""), "a/b");
    assert_eq!(tspath::get_normalized_absolute_path("a\\/b", ""), "a/b");
    assert_eq!(tspath::get_normalized_absolute_path("a\\/\\b", ""), "a/b");
    assert_eq!(tspath::get_normalized_absolute_path("a\\b//c", ""), "a/b/c");
    assert_eq!(
        tspath::get_normalized_absolute_path("\\a\\b\\\\c", ""),
        "/a/b/c"
    );
    assert_eq!(
        tspath::get_normalized_absolute_path("\\\\a\\b\\\\c", ""),
        "//a/b/c"
    );
}

// Go: path_test.go:436 TestGetNormalizedAbsolutePathWithoutRoot (at 673a5f17d713; removed by
// ts#64159 with the function. The port keeps it.)
#[test]
fn test_get_normalized_absolute_path_without_root() {
    assert_eq!(
        tspath::get_normalized_absolute_path_without_root("/a/b/c.txt", "/a/b"),
        "a/b/c.txt"
    );
    assert_eq!(
        tspath::get_normalized_absolute_path_without_root("c:/work/hello.txt", "c:/work"),
        "work/hello.txt"
    );
    assert_eq!(
        tspath::get_normalized_absolute_path_without_root("c:/work/hello.txt", "d:/worspaces"),
        "work/hello.txt"
    );
}

// Go: path_test.go:497 TestGetRelativePathToDirectoryOrUrl
#[test]
fn test_get_relative_path_to_directory_or_url() {
    // !!!
    // Based on tests for `getRelativePathFromDirectory`.

    assert_eq!(
        tspath::get_relative_path_to_directory_or_url(
            "/",
            "/",
            false,
            &ComparePathsOptions::default()
        ),
        ""
    );
    assert_eq!(
        tspath::get_relative_path_to_directory_or_url(
            "/a",
            "/a",
            false,
            &ComparePathsOptions::default()
        ),
        ""
    );
    assert_eq!(
        tspath::get_relative_path_to_directory_or_url(
            "/a/",
            "/a",
            false,
            &ComparePathsOptions::default()
        ),
        ""
    );
    assert_eq!(
        tspath::get_relative_path_to_directory_or_url(
            "/a",
            "/",
            false,
            &ComparePathsOptions::default()
        ),
        ".."
    );
    assert_eq!(
        tspath::get_relative_path_to_directory_or_url(
            "/a",
            "/b",
            false,
            &ComparePathsOptions::default()
        ),
        "../b"
    );
    assert_eq!(
        tspath::get_relative_path_to_directory_or_url(
            "/a/b",
            "/b",
            false,
            &ComparePathsOptions::default()
        ),
        "../../b"
    );
    assert_eq!(
        tspath::get_relative_path_to_directory_or_url(
            "/a/b/c",
            "/b",
            false,
            &ComparePathsOptions::default()
        ),
        "../../../b"
    );
    assert_eq!(
        tspath::get_relative_path_to_directory_or_url(
            "/a/b/c",
            "/b/c",
            false,
            &ComparePathsOptions::default()
        ),
        "../../../b/c"
    );
    assert_eq!(
        tspath::get_relative_path_to_directory_or_url(
            "/a/b/c",
            "/a/b",
            false,
            &ComparePathsOptions::default()
        ),
        ".."
    );
    assert_eq!(
        tspath::get_relative_path_to_directory_or_url(
            "c:",
            "d:",
            false,
            &ComparePathsOptions::default()
        ),
        "d:/"
    );
    assert_eq!(
        tspath::get_relative_path_to_directory_or_url(
            "file:///",
            "file:///",
            false,
            &ComparePathsOptions::default()
        ),
        ""
    );
    assert_eq!(
        tspath::get_relative_path_to_directory_or_url(
            "file:///a",
            "file:///a",
            false,
            &ComparePathsOptions::default()
        ),
        ""
    );
    assert_eq!(
        tspath::get_relative_path_to_directory_or_url(
            "file:///a/",
            "file:///a",
            false,
            &ComparePathsOptions::default()
        ),
        ""
    );
    assert_eq!(
        tspath::get_relative_path_to_directory_or_url(
            "file:///a",
            "file:///",
            false,
            &ComparePathsOptions::default()
        ),
        ".."
    );
    assert_eq!(
        tspath::get_relative_path_to_directory_or_url(
            "file:///a",
            "file:///b",
            false,
            &ComparePathsOptions::default()
        ),
        "../b"
    );
    assert_eq!(
        tspath::get_relative_path_to_directory_or_url(
            "file:///a/b",
            "file:///b",
            false,
            &ComparePathsOptions::default()
        ),
        "../../b"
    );
    assert_eq!(
        tspath::get_relative_path_to_directory_or_url(
            "file:///a/b/c",
            "file:///b",
            false,
            &ComparePathsOptions::default()
        ),
        "../../../b"
    );
    assert_eq!(
        tspath::get_relative_path_to_directory_or_url(
            "file:///a/b/c",
            "file:///b/c",
            false,
            &ComparePathsOptions::default()
        ),
        "../../../b/c"
    );
    assert_eq!(
        tspath::get_relative_path_to_directory_or_url(
            "file:///a/b/c",
            "file:///a/b",
            false,
            &ComparePathsOptions::default()
        ),
        ".."
    );
    assert_eq!(
        tspath::get_relative_path_to_directory_or_url(
            "file:///c:",
            "file:///d:",
            false,
            &ComparePathsOptions::default()
        ),
        "file:///d:/"
    );
}

// Go: path_test.go:524 TestToFileNameLowerCase
#[test]
fn test_to_file_name_lower_case() {
    assert_eq!(
        tspath::to_file_name_lower_case("/user/UserName/projects/Project/file.ts"),
        "/user/username/projects/project/file.ts"
    );
    assert_eq!(
        tspath::to_file_name_lower_case("/user/UserName/projects/projectß/file.ts"),
        "/user/username/projects/projectß/file.ts"
    );
    assert_eq!(
        tspath::to_file_name_lower_case("/user/UserName/projects/İproject/file.ts"),
        "/user/username/projects/İproject/file.ts"
    );
    assert_eq!(
        tspath::to_file_name_lower_case("/user/UserName/projects/ı/file.ts"),
        "/user/username/projects/ı/file.ts"
    );
}

// Go: path_test.go:591 TestTrimFilePathPrefix (tsgo#4900; at 673a5f17d713; removed by ts#64159,
// which tests CaseSensitivity.TrimPrefix. The port keeps trim_file_path_prefix.)
// PORT: Go returns `path, false` on a mismatch; the Rust port returns `None`.
#[test]
fn test_trim_file_path_prefix() {
    // case-sensitive exact match
    assert_eq!(
        tspath::trim_file_path_prefix("/project/src/file.ts", "/project/src", true).as_deref(),
        Some("/file.ts")
    );
    // case-sensitive mismatch
    assert_eq!(
        tspath::trim_file_path_prefix("/project/SRC/file.ts", "/project/src", true),
        None
    );
    // case-insensitive match
    assert_eq!(
        tspath::trim_file_path_prefix("/project/SRC/file.ts", "/project/src", false).as_deref(),
        Some("/file.ts")
    );
    // no match
    assert_eq!(
        tspath::trim_file_path_prefix("/other/file.ts", "/project/src", false),
        None
    );
    // case-folding shrinks prefix byte length without changing rune count:
    // each Kelvin sign '\u212A' case-folds to the single-byte 'k'.
    assert_eq!(
        tspath::trim_file_path_prefix("/kkk/a.ts", "/\u{212A}\u{212A}\u{212A}", false).as_deref(),
        Some("/a.ts")
    );
    // path equal to prefix
    assert_eq!(
        tspath::trim_file_path_prefix("/project/src", "/project/src", true).as_deref(),
        Some("")
    );
}

// Go: path_test.go:314 TestResolvePathWithoutTrailingDirectorySeparator (ts#64159)
#[test]
fn test_resolve_path_without_trailing_directory_separator() {
    assert_eq!(
        tspath::resolve_path_without_trailing_directory_separator("/", &[]),
        "/"
    );
    assert_eq!(
        tspath::resolve_path_without_trailing_directory_separator("c:/", &[]),
        "c:/"
    );
    assert_eq!(
        tspath::resolve_path_without_trailing_directory_separator("/a/", &[]),
        "/a"
    );
    assert_eq!(
        tspath::resolve_path_without_trailing_directory_separator("a", &["b/"]),
        "a/b"
    );
}

// Go: path_test.go:323 TestNormalizePathDriveRoot (ts#64159)
#[test]
fn test_normalize_path_drive_root() {
    assert_eq!(tspath::normalize_path("c:"), "c:/");
}

// Go: path_test.go:644 TestToPath (at 673a5f17d713; removed by ts#64159 with ToPath.
// The port keeps to_path, so the test stays.)
#[test]
fn test_to_path() {
    assert_eq!(
        tspath::to_path("file.ext", "path/to", false).0,
        "path/to/file.ext"
    );
    assert_eq!(
        tspath::to_path("file.ext", "/path/to", true).0,
        "/path/to/file.ext"
    );
    assert_eq!(
        tspath::to_path("/path/to/../file.ext", "path/to", true).0,
        "/path/file.ext"
    );
    // ts#64544: a dynamic file name keeps its case.
    assert_eq!(
        tspath::to_path(
            "^/~ts-uri~/custom/ts-nul-authority/CaseSensitive.ts",
            "/",
            false
        )
        .0,
        "^/~ts-uri~/custom/ts-nul-authority/CaseSensitive.ts"
    );
}

// Go: path_test.go:628 pathIsRelativeTests (with the `init` copies that
// use backslashes)
fn path_is_relative_tests() -> Vec<(String, bool)> {
    let mut tests: Vec<(String, bool)> = vec![
        // relative
        (".".into(), true),
        ("..".into(), true),
        ("./".into(), true),
        ("../".into(), true),
        ("./foo/bar".into(), true),
        ("../foo/bar".into(), true),
        (format!("../{}", "foo/".repeat(100)), true),
        // non-relative
        ("".into(), false),
        ("foo".into(), false),
        ("foo/bar".into(), false),
        ("/foo/bar".into(), false),
        ("c:/foo/bar".into(), false),
    ];
    let old = tests.clone();
    for (p, is_relative) in old {
        tests.push((p.replace('/', "\\"), is_relative));
    }
    tests
}

// Go: path_test.go:658 TestPathIsRelative
#[test]
fn test_path_is_relative() {
    let mut failures = Failures::new("TestPathIsRelative");
    for (p, is_relative) in path_is_relative_tests() {
        failures.check_eq(&p, tspath::path_is_relative(&p), is_relative);
    }
    failures.finish();
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

fn common_parents(paths: &[&str], min_components: i32) -> (Vec<String>, Vec<String>) {
    let (got, ignored) = tspath::get_common_parents(
        &strings(paths),
        min_components,
        &tspath::get_path_components,
        &ComparePathsOptions::default(),
    );
    let mut ignored: Vec<String> = ignored.into_iter().collect();
    ignored.sort();
    (got, ignored)
}

// Go: path_test.go:716 TestGetCommonParents
#[test]
fn test_get_common_parents() {
    let none: Vec<String> = Vec::new();
    // empty input
    assert_eq!(common_parents(&[], 1), (none.clone(), none.clone()));
    // single path returns itself
    assert_eq!(
        common_parents(&["/a/b/c/d"], 1),
        (strings(&["/a/b/c/d"]), none.clone())
    );
    // paths shorter than minComponents are ignored
    assert_eq!(
        common_parents(&["/a/b/c/d", "/a/b/c/e", "/a/b/f/g", "/x/y"], 4),
        (strings(&["/a/b/c", "/a/b/f/g"]), strings(&["/x/y"]))
    );
    // three paths share /a/b
    assert_eq!(
        common_parents(&["/a/b/c/d", "/a/b/c/e", "/a/b/f/g"], 1),
        (strings(&["/a/b"]), none.clone())
    );
    // mixed with short path collapses to root when minComponents=1
    assert_eq!(
        common_parents(&["/a/b/c/d", "/a/b/c/e", "/a/b/f/g", "/x/y/z"], 1),
        (strings(&["/"]), none.clone())
    );
    // mixed with short path preserves both when minComponents=3
    assert_eq!(
        common_parents(&["/a/b/c/d", "/a/b/c/e", "/a/b/f/g", "/x/y/z"], 3),
        (strings(&["/a/b", "/x/y/z"]), none.clone())
    );
    // different volumes are returned individually
    assert_eq!(
        common_parents(&["c:/a/b/c/d", "d:/a/b/c/d"], 1),
        (strings(&["c:/a/b/c/d", "d:/a/b/c/d"]), none.clone())
    );
    // duplicate paths deduplicate result
    assert_eq!(
        common_parents(&["/a/b/c/d", "/a/b/c/d"], 1),
        (strings(&["/a/b/c/d"]), none.clone())
    );
    // paths with few components are returned as-is when minComponents met
    assert_eq!(
        common_parents(&["/a/b/c/d", "/x/y"], 2),
        (strings(&["/a/b/c/d", "/x/y"]), none.clone())
    );
    // minComponents=2
    assert_eq!(
        common_parents(&["/a/b/c/d", "/a/z/c/e", "/a/aaa/f/g", "/x/y/z"], 2),
        (strings(&["/a", "/x/y/z"]), none.clone())
    );
    // trailing separators are handled
    assert_eq!(
        common_parents(&["/a/b/", "/a/b/c"], 1),
        (strings(&["/a/b"]), none)
    );
}

// Go: ignoredpaths_test.go:7 TestContainsIgnoredPath
#[test]
fn test_contains_ignored_path() {
    let tests: &[(&str, &str, bool)] = &[
        (
            "node_modules dot path",
            "/project/node_modules/.pnpm/file.ts",
            true,
        ),
        ("git directory", "/project/.git/hooks/pre-commit", true),
        ("emacs lock file", "/project/src/file.ts.#", true),
        ("regular file path", "/project/src/file.ts", false),
        (
            "node_modules without dot",
            "/project/node_modules/lodash/index.js",
            false,
        ),
        ("empty path", "", false),
        (
            "path with multiple ignored patterns",
            "/project/node_modules/.pnpm/.git/.#file.ts",
            true,
        ),
        (
            "case sensitive test",
            "/project/NODE_MODULES/.PNPM/file.ts",
            false,
        ),
        (
            "path with ignored pattern in middle",
            "/project/src/node_modules/.pnpm/dist/file.js",
            true,
        ),
        (
            "path with ignored pattern at end",
            "/project/src/file.ts.#",
            true,
        ),
    ];
    let mut failures = Failures::new("TestContainsIgnoredPath");
    for (name, path, expected) in tests {
        failures.check_eq(name, tspath::contains_ignored_path(path), *expected);
    }
    failures.finish();
}

// Go: ignoredpaths_test.go:77 TestIgnoredPathsPatterns
#[test]
fn test_ignored_paths_patterns() {
    for pattern in ["/node_modules/.", "/.git", ".#"] {
        let test_path = format!("/test{pattern}/file.ts");
        assert!(
            tspath::contains_ignored_path(&test_path),
            "Expected pattern '{pattern}' to be detected in path '{test_path}'"
        );
    }
}

// Go: ignoredpaths_test.go:90 TestIgnoredPathsEdgeCases
#[test]
fn test_ignored_paths_edge_cases() {
    let tests: &[(&str, &str, bool)] = &[
        ("pattern at start", "/node_modules./file.ts", false),
        ("pattern at end", "/project/file.ts.#", true),
        (
            "multiple occurrences",
            "/project/.git/node_modules./.git/file.ts",
            true,
        ),
        ("no slashes", "node_modules.file.ts", false),
        ("single slash", "/file.ts", false),
    ];
    let mut failures = Failures::new("TestIgnoredPathsEdgeCases");
    for (name, path, expected) in tests {
        failures.check_eq(name, tspath::contains_ignored_path(path), *expected);
    }
    failures.finish();
}

fn check_starts_with_directory(test: &str, tests: &[(&str, &str, &str, bool, bool)]) {
    let mut failures = Failures::new(test);
    for (name, file_name, directory_name, case_sensitive, expected) in tests {
        failures.check_eq(
            name,
            tspath::starts_with_directory(file_name, directory_name, *case_sensitive),
            *expected,
        );
    }
    failures.finish();
}

// Go: startsWithDirectory_test.go:7 TestStartsWithDirectory
#[test]
fn test_starts_with_directory() {
    check_starts_with_directory(
        "TestStartsWithDirectory",
        &[
            (
                "exact match case sensitive",
                "/project/src/file.ts",
                "/project/src",
                true,
                true,
            ),
            (
                "exact match case insensitive",
                "/project/src/file.ts",
                "/PROJECT/SRC",
                false,
                true,
            ),
            (
                "case sensitive mismatch",
                "/project/src/file.ts",
                "/PROJECT/SRC",
                true,
                false,
            ),
            (
                "file not in directory",
                "/project/lib/file.ts",
                "/project/src",
                true,
                false,
            ),
            (
                "file in subdirectory",
                "/project/src/components/Button.tsx",
                "/project/src",
                true,
                true,
            ),
            (
                "file in parent directory",
                "/project/file.ts",
                "/project/src",
                true,
                false,
            ),
            (
                "windows style separators",
                "C:\\project\\src\\file.ts",
                "C:\\project\\src",
                true,
                true,
            ),
            (
                "mixed separators",
                "/project/src/file.ts",
                "\\project\\src",
                true,
                false,
            ),
            (
                "empty directory name",
                "/project/src/file.ts",
                "",
                true,
                false,
            ),
            ("empty file name", "", "/project/src", true, false),
            (
                "identical paths",
                "/project/src",
                "/project/src",
                true,
                false,
            ),
            (
                "directory with trailing separator",
                "/project/src/file.ts",
                "/project/src/",
                true,
                true,
            ),
            (
                "unicode characters",
                "/project/测试/file.ts",
                "/project/测试",
                true,
                true,
            ),
            (
                "unicode case insensitive",
                "/project/测试/file.ts",
                "/PROJECT/测试",
                false,
                true,
            ),
        ],
    );
}

// Go: startsWithDirectory_test.go:128 TestStartsWithDirectoryEdgeCases
#[test]
fn test_starts_with_directory_edge_cases() {
    check_starts_with_directory(
        "TestStartsWithDirectoryEdgeCases",
        &[
            (
                "file name shorter than directory",
                "/proj",
                "/project",
                true,
                false,
            ),
            (
                "file name starts with directory but no separator",
                "/projectsrc/file.ts",
                "/project",
                true,
                false,
            ),
            ("relative paths", "src/file.ts", "src", true, true),
            (
                "absolute vs relative",
                "/project/src/file.ts",
                "project/src",
                true,
                false,
            ),
        ],
    );
}

// Go: untitled_test.go:10 TestUntitledPathHandling
#[test]
fn test_untitled_path_handling() {
    let untitled_path = "^/untitled/ts-nul-authority/Untitled-2";
    assert_eq!(
        tspath::get_encoded_root_length(untitled_path),
        2,
        "GetEncodedRootLength should return 2 for untitled paths"
    );
    assert!(
        tspath::is_rooted_disk_path(untitled_path),
        "IsRootedDiskPath should return true for untitled paths"
    );
    let current_dir = "/home/user/project";
    assert_eq!(
        tspath::to_path(untitled_path, current_dir, true).0,
        "^/untitled/ts-nul-authority/Untitled-2",
        "ToPath should not resolve untitled paths against current directory"
    );
    assert_eq!(
        tspath::get_normalized_absolute_path(untitled_path, current_dir),
        "^/untitled/ts-nul-authority/Untitled-2",
        "GetNormalizedAbsolutePath should not resolve untitled paths"
    );
}

// Go: untitled_test.go:34 TestUntitledPathEdgeCases
#[test]
fn test_untitled_path_edge_cases() {
    let cases: &[(&str, i32, bool)] = &[
        ("^/", 2, true),
        ("^/untitled/ts-nul-authority/test", 2, true),
        ("^", 0, false),
        ("^x", 0, false),
        ("^^/", 0, false),
        ("x^/", 0, false),
        (
            "^/untitled/ts-nul-authority/path/with/deeper/structure",
            2,
            true,
        ),
    ];
    let mut failures = Failures::new("TestUntitledPathEdgeCases");
    for (path, expected, is_rooted) in cases {
        failures.check_eq(
            &format!("GetEncodedRootLength {path}"),
            tspath::get_encoded_root_length(path),
            *expected,
        );
        failures.check_eq(
            &format!("IsRootedDiskPath {path}"),
            tspath::is_rooted_disk_path(path),
            *is_rooted,
        );
    }
    failures.finish();
}

/// Go `[]tspath.ExtensionRewrite{{Source: ..., Target: ...}, ...}`.
fn extension_rewrites(entries: &[(&str, &str)]) -> Vec<tspath::ExtensionRewrite> {
    entries
        .iter()
        .map(|(source, target)| tspath::ExtensionRewrite {
            source: (*source).to_string(),
            target: (*target).to_string(),
        })
        .collect()
}

// Go: extensionrewrite_test.go:10 TestRewriteExtension (ts#64581)
// PORT: Go `CaseSensitive` is `true` (`use_case_sensitive_file_names`), Go
// `CaseInsensitive` is `false`. A Go `(value, false)` result is `None`, and
// its unchanged value is the input.
#[test]
fn test_rewrite_extension() {
    let mut rewrites = extension_rewrites(&[(".z", ".js"), (".y.z", ".mjs")]);

    assert_eq!(
        tspath::rewrite_module_specifier_extension("./Widget.y.z", &rewrites, true).as_deref(),
        Some("./Widget.mjs")
    );
    assert_eq!(
        tspath::rewrite_module_specifier_extension("./Widget.Y.Z", &rewrites, false).as_deref(),
        Some("./Widget.mjs")
    );
    assert_eq!(
        tspath::rewrite_module_specifier_extension(".y", &rewrites, true),
        None
    );

    rewrites.extend(extension_rewrites(&[(".identity.y.z", ".identity.y.z")]));
    assert_eq!(
        tspath::rewrite_module_specifier_extension("./Widget.IDENTITY.Y.Z", &rewrites, false),
        None
    );
    assert!(!tspath::should_rewrite_module_specifier_extension(
        "./Widget.identity.y.z",
        &rewrites,
        true
    ));
}

// Go: extensionrewrite_test.go:36 TestRootedFilePathRewriteExtension (ts#64581)
#[test]
fn test_rooted_file_path_rewrite_extension() {
    let rewrites = extension_rewrites(&[
        (".z", ".js"),
        (".y.z", ".mjs"),
        (".identity.y.z", ".identity.y.z"),
    ]);
    let cases: &[(&str, &str, bool, bool)] = &[
        ("/src/Widget.y.z", "/src/Widget.mjs", true, true),
        ("/src/Widget.Y.Z", "/src/Widget.mjs", false, true),
        ("/src/Widget.Y.Z", "/src/Widget.Y.Z", true, false),
        (
            "/src/Widget.IDENTITY.Y.Z",
            "/src/Widget.IDENTITY.Y.Z",
            false,
            false,
        ),
        ("/src/Widget.txt", "/src/Widget.txt", true, false),
    ];
    let mut failures = Failures::new("TestRootedFilePathRewriteExtension");
    for (path, expected, use_case_sensitive_file_names, changed) in cases {
        let rewritten =
            tspath::rewrite_file_extension(path, &rewrites, *use_case_sensitive_file_names);
        failures.check_eq(&format!("changed {path}"), rewritten.is_some(), *changed);
        failures.check_eq(
            &format!("RewriteExtension {path}"),
            rewritten.unwrap_or_else(|| (*path).to_string()),
            (*expected).to_string(),
        );
    }
    failures.finish();
}

// Go: extensionrewrite_test.go:61 TestModuleSpecifierShouldRewriteExtension (ts#64581)
#[test]
fn test_module_specifier_should_rewrite_extension() {
    let rewrites = extension_rewrites(&[(".astro", ".js")]);
    let cases: &[(&str, bool)] = &[
        ("./Card.astro", true),
        ("../Card.astro", true),
        ("./file.ts", true),
        ("./file.d.ts", false),
        ("./file.d.astro.ts", false),
        ("./file.js", false),
        ("package.astro", false),
        ("/Card.astro", false),
    ];
    let mut failures = Failures::new("TestModuleSpecifierShouldRewriteExtension");
    for (specifier, expected) in cases {
        failures.check_eq(
            &format!("ShouldRewriteExtension {specifier}"),
            tspath::should_rewrite_module_specifier_extension(specifier, &rewrites, true),
            *expected,
        );
    }
    failures.finish();
}

// Go: typed_paths_test.go:551 TestContainsFilePath (the ContainsFilePath
// asserts) and typed_paths_test.go:83 (TestEncodedDynamicPathsPreserveOpaqueIdentity:
// an encoded dynamic name compares with case). `contains_rooted_path` is
// Go `CaseSensitivity.ContainsFilePath`; `true` is Go `CaseSensitive`.
#[test]
fn test_contains_file_path() {
    let cases: &[(&str, bool, &str, &str, bool)] = &[
        ("same path", true, "/project/src", "/project/src", true),
        ("child", true, "/project/src", "/project/src/a.ts", true),
        (
            "prefix sibling",
            true,
            "/project/src",
            "/project/source/a.ts",
            false,
        ),
        ("root", true, "/", "/project/src/a.ts", true),
        (
            "case sensitive mismatch",
            true,
            "/PROJECT",
            "/project/a.ts",
            false,
        ),
        ("case insensitive", false, "/PROJECT", "/project/a.ts", true),
        (
            "case folded rune",
            false,
            "/repo/\u{212A}",
            "/repo/k/a.ts",
            true,
        ),
        (
            "drive root casing",
            true,
            "C:/project",
            "c:/project/a.ts",
            true,
        ),
        (
            "file URL scheme and drive casing",
            true,
            "FILE:///C:/project",
            "file:///c:/project/a.ts",
            true,
        ),
        (
            "file URL localhost and drive casing",
            true,
            "file://LOCALHOST/C:/project",
            "file://localhost/c:/project/a.ts",
            true,
        ),
        (
            "different root",
            false,
            "c:/project",
            "d:/project/a.ts",
            false,
        ),
        (
            "dynamic name with case",
            false,
            "^/~ts-uri~/custom/authority/Folder",
            "^/~ts-uri~/custom/authority/folder/file.ts",
            false,
        ),
    ];
    for &(name, case_sensitive, directory, file_name, contained) in cases {
        assert_eq!(
            tspath::contains_rooted_path(directory, file_name, case_sensitive),
            contained,
            "{name}"
        );
    }
}

// Go: typed_paths_test.go:595-597 (TestContainsFilePath: the PathKey
// asserts) and typed_paths_test.go:94 (a dynamic key compares with case),
// with the answers of a followups76r2 probe of Go `PathKey.ContainsPath` at
// 3e9236361fa9 (keys from `CaseSensitivity.PathKey`). `to_path` is Go's
// `PathKey`; `true` is Go `CaseSensitive`. On a case-insensitive file
// system Go's key folds case with ToLower, which keeps 'ſ' and 'ς' (EqualFold
// would match them to 's' and 'σ') and makes the Kelvin sign 'k'.
#[test]
fn test_path_key_contains_path() {
    let cases: &[(bool, &str, &str, bool)] = &[
        (true, "C:/project", "c:/project/a.ts", true),
        (false, "/ws/\u{17f}", "/ws/s/a", false),
        (false, "/ws/S", "/ws/s/a", true),
        (false, "/ws/\u{3c2}", "/ws/\u{3c3}/a", false),
        (false, "/ws/\u{212a}", "/ws/k/a", true),
        (true, "C:/ws", "c:/ws/a", true),
        (true, "c:/", "C:/a", true),
        (true, "/ws", "/WS/a", false),
        (true, "/", "/a", true),
        (true, "/ws", "/ws", true),
        (true, "/ws", "/wsx", false),
        (true, "/ws/", "/ws/a", true),
        (true, "//Server/share/ws", "//server/share/ws/a", true),
        (
            false,
            "^/~ts-uri~/custom/Authority/src",
            "^/~ts-uri~/custom/Authority/src/a.ts",
            true,
        ),
        (
            false,
            "^/~ts-uri~/custom/Authority/src",
            "^/~ts-uri~/custom/authority/src/a.ts",
            false,
        ),
    ];
    for &(case_sensitive, parent, child, contained) in cases {
        let parent_key = tspath::to_path(parent, "", case_sensitive);
        let child_key = tspath::to_path(child, "", case_sensitive);
        assert_eq!(
            tspath::contains_path_key(&parent_key, &child_key),
            contained,
            "{parent} {child} {case_sensitive}"
        );
    }
    // Go typed_paths_test.go:94 builds the keys directly.
    assert!(!tspath::contains_path_key(
        "^/~ts-uri~/custom/Authority/src",
        "^/~ts-uri~/custom/authority/src/file.ts"
    ));
}

// `Path::contains_path` is Go `PathKey.ContainsPath` (pathkey.go:155). The
// answers are those of a followups79 probe of Go at 3e9236361fa9: roots
// compare without case, dynamic roots with case.
#[test]
fn test_path_contains_path_is_path_key() {
    let cases: &[(&str, &str, bool)] = &[
        ("C:/p", "c:/p/a.ts", true),
        ("c:/", "C:/a", true),
        ("/ws", "/WS/a", false),
        ("//Server/share/ws", "//server/share/ws/a", true),
        ("/ws/", "/ws/a", true),
        ("/ws", "/wsx", false),
        ("/ws", "/ws", true),
        ("/", "/a", true),
        (
            "^/~ts-uri~/custom/Authority/src",
            "^/~ts-uri~/custom/authority/src/a.ts",
            false,
        ),
        ("", "/a", false),
    ];
    for &(parent, child, contained) in cases {
        let parent = tspath::Path(parent.to_string());
        let child = tspath::Path(child.to_string());
        assert_eq!(
            parent.contains_path(&child),
            contained,
            "{parent:?} {child:?}"
        );
    }
}
