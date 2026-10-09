//! Port of internal/outputpaths/outputpaths_test.go (tsgo#4900).

use ts_goport::frontend::outputpaths::{
    OutputPathsHost, get_build_info_file_name, get_output_declaration_file_name_worker,
    get_output_js_file_name_worker, get_source_file_path_in_new_dir,
};
use ts_goport::frontend::tspath::ComparePathsOptions;
use ts_goport::options::{CompilerOptions, Tristate};

// Go: outputpaths/outputpaths_test.go:29 TestGetSourceFileNameInNewDirSourceMatchesCommonDirectory
// (ts#64159 renames TestGetSourceFilePathInNewDirSourceMatchesCommonDirectory)
#[test]
fn test_get_source_file_name_in_new_dir_source_matches_common_directory() {
    let actual = get_source_file_path_in_new_dir(
        "/project/src",
        "/project/out",
        "/project",
        "/project/src",
        true,
    );
    assert_eq!(actual, "/project/src");
}

// Go: outputpaths/outputpaths_test.go:41 TestGetSourceFileNameInNewDirCanonicalizationShrinksCommonDirectory
// (ts#64159 renames TestGetSourceFilePathInNewDirCanonicalizationShrinksCommonDirectory)
#[test]
fn test_get_source_file_name_in_new_dir_canonicalization_shrinks_common_directory() {
    // Each Kelvin sign '\u212A' case-folds to the single-byte 'k', so the raw
    // (non-canonicalized) commonSourceDirectory is longer, in bytes, than the source
    // file path it's a case-insensitive prefix of, even though the file path itself
    // is longer overall once its own (already-lowercase) suffix is included.
    // Slicing sourceFilePath by len(commonSourceDirectory) bytes would still panic
    // here ([14:11]); this must clamp per-rune instead, like the reference
    // implementation's substring does.
    let actual = get_source_file_path_in_new_dir(
        "/kkkk/a.ts",
        "/out",
        "/",
        "/\u{212A}\u{212A}\u{212A}\u{212A}/",
        false,
    );
    assert_eq!(actual, "/out/a.ts");
}

// Go: outputpaths/outputpaths_test.go:59 TestGetBuildInfoFileNameAcrossRoots (ts#64159)
#[test]
fn test_get_build_info_file_name_across_roots() {
    let mut failures = Vec::new();
    #[rustfmt::skip]
    let tests: &[(&str, &str, &str, &str, &str)] = &[
        // (name, rootDir, outDir, configFilePath, expected)
        ("same root", "c:/src", "c:/out", "c:/src/project/tsconfig.json", "c:/out/project/tsconfig.tsbuildinfo"),
        ("empty stem", "c:/src", "c:/out", "c:/src/project/.json", "c:/out/project/.tsbuildinfo"),
        ("dot stem", "c:/src", "c:/out", "c:/src/project/..json", "c:/out/project/..tsbuildinfo"),
        ("empty filename stem without rootDir", "", "c:/out", "c:/src/project/.json", "c:/out/.tsbuildinfo"),
        ("parent-dot filename stem without rootDir", "", "c:/out", "c:/src/project/...json", "c:/out/...tsbuildinfo"),
        ("empty filename stem without outDir", "", "", "c:/src/project/.json", "c:/src/project/.tsbuildinfo"),
        ("dot filename stem without outDir", "", "", "c:/src/project/..json", "c:/src/project/..tsbuildinfo"),
        ("different drive", "c:/src", "c:/out", "d:/project/tsconfig.json", "d:/project/tsconfig.tsbuildinfo"),
        ("different UNC authority", "//server-a/src", "//server-a/out", "//server-b/project/tsconfig.json", "//server-b/project/tsconfig.tsbuildinfo"),
        ("different URL authority", "file://server-a/src", "file://server-a/out", "file://server-b/project/tsconfig.json", "file://server-b/project/tsconfig.tsbuildinfo"),
    ];
    for &(name, root_dir, out_dir, config_file_path, expected) in tests {
        let options = CompilerOptions {
            incremental: Tristate::True,
            root_dir: root_dir.to_string(),
            out_dir: out_dir.to_string(),
            config_file_path: config_file_path.to_string(),
            ..Default::default()
        };
        // Go passes tspath.CaseInsensitive; the paths are rooted, so the
        // current directory is never read.
        let actual = get_build_info_file_name(
            &options,
            &ComparePathsOptions {
                use_case_sensitive_file_names: false,
                current_directory: String::new(),
            },
        );
        if actual != expected {
            failures.push(format!("{name}: got {actual:?}, expected {expected:?}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Go `outputPathsHost` (outputpaths_test.go:12): a common source directory,
/// content mapper extensions, case-insensitive (ts#64159).
struct TestOutputPathsHost {
    common_source_directory: &'static str,
    content_mapper_extensions: Vec<String>,
}

impl OutputPathsHost for TestOutputPathsHost {
    fn common_source_directory(&self) -> String {
        self.common_source_directory.to_string()
    }
    fn content_mapper_extensions(&self) -> Vec<String> {
        self.content_mapper_extensions.clone()
    }
    // PORT: Go's host has no current directory (rooted paths); the common
    // source directory is rooted, so it is never read.
    fn get_current_directory(&self) -> String {
        String::new()
    }
    fn use_case_sensitive_file_names(&self) -> bool {
        false
    }
}

// Go: outputpaths/outputpaths_test.go:152 TestGetOutputFileNameAcrossRoots (ts#64159)
#[test]
fn test_get_output_file_name_across_roots() {
    let options = CompilerOptions {
        out_dir: "c:/dist".to_string(),
        ..Default::default()
    };
    let host = TestOutputPathsHost {
        common_source_directory: "c:/src",
        content_mapper_extensions: Vec::new(),
    };
    assert_eq!(
        get_output_js_file_name_worker("d:/shared.ts", &options, &host),
        "d:/shared.js"
    );
    assert_eq!(
        get_output_declaration_file_name_worker("d:/shared.ts", &options, &host),
        "d:/shared.d.ts"
    );
}

// Go: outputpaths/outputpaths_test.go:167 TestOutputFileNamesPreserveEmptyAndDotStems (ts#64159)
#[test]
fn test_output_file_names_preserve_empty_and_dot_stems() {
    let host = TestOutputPathsHost {
        common_source_directory: "/project/src",
        content_mapper_extensions: vec![".css".to_string()],
    };
    let options = CompilerOptions {
        out_dir: "/project/out".to_string(),
        ..Default::default()
    };
    for (input, js, declaration) in [
        ("/project/src/.ts", "/project/out/.js", "/project/out/.d.ts"),
        (
            "/project/src/..ts",
            "/project/out/..js",
            "/project/out/..d.ts",
        ),
        (
            "/project/src/...ts",
            "/project/out/...js",
            "/project/out/...d.ts",
        ),
        (
            "/project/src/.mts",
            "/project/out/.mjs",
            "/project/out/.d.mts",
        ),
        (
            "/project/src/.d.ts",
            "/project/out/.js",
            "/project/out/.d.ts",
        ),
        (
            "/project/src/.css",
            "/project/out/.css",
            "/project/out/.d.css.ts",
        ),
    ] {
        assert_eq!(
            get_output_js_file_name_worker(input, &options, &host),
            js,
            "{input}"
        );
        assert_eq!(
            get_output_declaration_file_name_worker(input, &options, &host),
            declaration,
            "{input}"
        );
    }
}
