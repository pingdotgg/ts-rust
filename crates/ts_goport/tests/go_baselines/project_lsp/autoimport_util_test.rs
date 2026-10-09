//! Port of Go `internal/ls/autoimport/util_test.go`.
//! No program is built, so the tests run in the test process.

use ts_goport::ls::autoimport::util::{get_package_realpath_funcs, word_indices};

use crate::support::vfstest::{self, MapFile};

// Go: util_test.go:11 TestWordIndices (one Go subtest per input)
fn check_word_indices(input: &str, expected_words: &[&str]) {
    let indices = word_indices(input);

    // Convert indices to actual word slices for comparison
    let actual_words: Vec<&str> = indices.iter().map(|&idx| &input[idx as usize..]).collect();

    assert_eq!(
        actual_words, expected_words,
        "wordIndices({input:?}) produced words {actual_words:?}, want {expected_words:?}"
    );
}

// Go: util_test.go:11 TestWordIndices
#[test]
fn word_indices_test() {
    // Basic camelCase
    check_word_indices("camelCase", &["camelCase", "Case"]);
    // snake_case
    check_word_indices("snake_case", &["snake_case", "case"]);
    // ParseURL - uppercase sequence followed by lowercase
    check_word_indices("ParseURL", &["ParseURL", "URL"]);
    // XMLHttpRequest - multiple uppercase sequences
    check_word_indices(
        "XMLHttpRequest",
        &["XMLHttpRequest", "HttpRequest", "Request"],
    );
    // Single word lowercase
    check_word_indices("hello", &["hello"]);
    // Single word uppercase
    check_word_indices("HELLO", &["HELLO"]);
    // Mixed with numbers
    check_word_indices(
        "parseHTML5Parser",
        &["parseHTML5Parser", "HTML5Parser", "Parser"],
    );
    // Underscore variations
    check_word_indices("__proto__", &["__proto__", "proto__"]);
    check_word_indices("_private_member", &["_private_member", "member"]);
    // Single character
    check_word_indices("a", &["a"]);
    check_word_indices("A", &["A"]);
    // Consecutive underscores
    check_word_indices(
        "test__double__underscore",
        &[
            "test__double__underscore",
            "double__underscore",
            "underscore",
        ],
    );
}

fn text(s: &str) -> MapFile {
    MapFile::from(s)
}

// Go: util_test.go:101 TestGetPackageRealpathFuncs_FollowsNodeModulesSymlinks
#[test]
fn get_package_realpath_funcs_follows_node_modules_symlinks() {
    let fs = vfstest::from_map(
        [
            ("/symlink-bin/pkg", vfstest::symlink("/real/bin/pkg")),
            (
                "/real/bin/pkg/index.d.ts",
                text("export declare const a: number;"),
            ),
            // ts#64544: util_test.go:116
            ("/real/bin/pkg/node_modules/.package-lock.json", text("{}")),
            (
                "/real/bin/pkg/node_modules/dep",
                vfstest::symlink("/real/dep"),
            ),
            // ts#64544: util_test.go:118
            (
                "/real/bin/pkg/node_modules/@scope/dep",
                vfstest::symlink("/real/scoped-dep"),
            ),
            (
                "/real/dep/index.d.ts",
                text("export declare const b: number;"),
            ),
            (
                "/real/dep/src/utils/helper.d.ts",
                text("export declare const c: number;"),
            ),
            // ts#64544: util_test.go:121
            (
                "/real/scoped-dep/index.d.ts",
                text("export declare const d: number;"),
            ),
        ],
        true,
    );

    let (to_realpath, _) = get_package_realpath_funcs(fs, "/symlink-bin/pkg");

    // ts#64544: util_test.go:126
    // Files directly within node_modules must not seed a cache entry that
    // prevents a later package-root directory from following its symlink.
    assert_eq!(
        to_realpath("/real/bin/pkg/node_modules/.package-lock.json"),
        "/real/bin/pkg/node_modules/.package-lock.json"
    );

    // Files inside the package should be converted via string replacement (fast path).
    assert_eq!(
        to_realpath("/symlink-bin/pkg/index.d.ts"),
        "/real/bin/pkg/index.d.ts",
        "package files should be converted via prefix replacement"
    );

    // ts#64544: util_test.go:142
    // A sibling whose name starts with the package name is not inside the package.
    assert_eq!(
        to_realpath("/symlink-bin/pkg2/index.d.ts"),
        "/symlink-bin/pkg2/index.d.ts",
        "sibling package paths must not use prefix replacement"
    );

    // Files outside the package (e.g. node_modules symlinks) should be resolved via
    // fs.Realpath so the cache key is the canonical realpath, not the symlink path.
    assert_eq!(
        to_realpath("/real/bin/pkg/node_modules/dep/index.d.ts"),
        "/real/dep/index.d.ts",
        "node_modules symlinks must be followed so the same file gets a consistent cache key"
    );

    // ts#64544: util_test.go:159
    // The module resolver also uses toRealpath while traversing directories.
    assert_eq!(
        to_realpath("/real/bin/pkg/node_modules/dep"),
        "/real/dep",
        "package-root directories should follow their node_modules symlink"
    );

    // ts#64544: util_test.go:167
    // Walking the scope directory first must not seed a cache entry that
    // prevents a nested scoped package from following its symlink.
    assert_eq!(
        to_realpath("/real/bin/pkg/node_modules/@scope"),
        "/real/bin/pkg/node_modules/@scope"
    );
    assert_eq!(
        to_realpath("/real/bin/pkg/node_modules/@scope/dep"),
        "/real/scoped-dep",
        "scoped package-root directories should follow their node_modules symlink"
    );

    // Files in subdirectories of an already-resolved external package should
    // use the cached prefix mapping without additional realpath calls.
    assert_eq!(
        to_realpath("/real/bin/pkg/node_modules/dep/src/utils/helper.d.ts"),
        "/real/dep/src/utils/helper.d.ts",
        "subdirectories of a resolved external package should use cached prefix mapping"
    );
}

// Go: util_test.go:197 TestGetPackageRealpathFuncs_DuplicateCacheKeys
#[test]
fn get_package_realpath_funcs_duplicate_cache_keys() {
    let fs = vfstest::from_map(
        [
            (
                "/workspace/packages/app-a",
                vfstest::symlink("/store/app-a"),
            ),
            (
                "/workspace/packages/app-b",
                vfstest::symlink("/store/app-b"),
            ),
            (
                "/store/app-a/index.d.ts",
                text("export declare const a: number;"),
            ),
            (
                "/store/app-b/index.d.ts",
                text("export declare const b: number;"),
            ),
            (
                "/store/app-a/node_modules/shared-lib",
                vfstest::symlink("/store/shared-lib"),
            ),
            (
                "/store/app-b/node_modules/shared-lib",
                vfstest::symlink("/store/shared-lib"),
            ),
            (
                "/store/shared-lib/index.d.ts",
                text("export declare const shared: string;"),
            ),
        ],
        true,
    );

    let (to_realpath_a, _) = get_package_realpath_funcs(fs.clone(), "/workspace/packages/app-a");
    let (to_realpath_b, _) = get_package_realpath_funcs(fs, "/workspace/packages/app-b");

    let resolved_a = to_realpath_a("/store/app-a/node_modules/shared-lib/index.d.ts");
    let resolved_b = to_realpath_b("/store/app-b/node_modules/shared-lib/index.d.ts");

    // Both should resolve to the same canonical realpath so the module resolver
    // uses a single cache key for the shared dependency, avoiding duplicate loads.
    let expected_realpath = "/store/shared-lib/index.d.ts";
    assert_eq!(
        resolved_a, expected_realpath,
        "app-a's toRealpath should follow the node_modules symlink to the realpath"
    );
    assert_eq!(
        resolved_b, expected_realpath,
        "app-b's toRealpath should follow the node_modules symlink to the realpath"
    );
}

// Go: util_test.go:237 TestGetPackageRealpathFuncs_NonSymlinkedPackageWithSymlinkedDeps
#[test]
fn get_package_realpath_funcs_non_symlinked_package_with_symlinked_deps() {
    let fs = vfstest::from_map(
        [
            (
                "/real/my-pkg/index.d.ts",
                text("export declare const a: number;"),
            ),
            (
                "/real/my-pkg/node_modules/dep",
                vfstest::symlink("/real/dep"),
            ),
            (
                "/real/dep/index.d.ts",
                text("export declare const b: number;"),
            ),
        ],
        true,
    );

    let (to_realpath, _) = get_package_realpath_funcs(fs, "/real/my-pkg");

    // Files inside the (non-symlinked) package should be returned unchanged.
    assert_eq!(
        to_realpath("/real/my-pkg/index.d.ts"),
        "/real/my-pkg/index.d.ts"
    );

    // Files outside the package reached via symlinked node_modules should still be resolved.
    assert_eq!(
        to_realpath("/real/my-pkg/node_modules/dep/index.d.ts"),
        "/real/dep/index.d.ts",
        "symlinked deps must be resolved even when the package dir itself is not a symlink"
    );
}
