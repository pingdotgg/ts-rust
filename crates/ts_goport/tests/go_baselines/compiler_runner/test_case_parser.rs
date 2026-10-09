//! Go: internal/testrunner/test_case_parser.go and test_case_parser_test.go.

use std::collections::BTreeMap;
use std::rc::Rc;

use ts_goport::api::to_rooted_path;
use ts_goport::frontend::prelude::*;

use super::go_regex;
use super::harness::get_config_name_from_file_name;
use super::runner::SRC_FOLDER;
use crate::tsoptions::tsoptionstest::new_vfs_parse_config_host_with_symlinks;

// Go: test_case_parser.go:25 rawCompilerSettings
// This maps a compiler setting to its value as written in the test file. For example, if a test file contains:
//
//	// @target: esnext, es2015
//
// Then the map will map "target" to "esnext, es2015"
// PORT: a Go map; only lookups and an unordered walk read it.
pub type RawCompilerSettings = BTreeMap<String, String>;

// Go: test_case_parser.go:28 testUnit
// All the necessary information to turn a multi file test into useful units for later compilation
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TestUnit {
    pub content: String,
    pub name: String,
}

// Go: test_case_parser.go:33 testCaseContent
pub struct TestCaseContent {
    pub test_unit_data: Vec<TestUnit>,
    pub ts_config: Option<Rc<ParsedCommandLine>>,
    pub ts_config_file_unit_data: Option<TestUnit>,
    pub symlinks: BTreeMap<String, String>,
}

// Go: test_case_parser.go:46 fourslashDirectives
// File-specific directives used by fourslash tests
// tsgo#4712 adds "noopen".
const FOURSLASH_DIRECTIVES: &[&str] = &["emitthisfile", "noopen"];

// Go: test_case_parser.go:50 makeUnitsFromTest
// Given a test file containing // @FileName directives,
// return an array of named units of code to be added to an existing compiler instance.
pub fn make_units_from_test(code: &str, file_name: &str) -> TestCaseContent {
    let (mut test_units, symlinks, raw_current_directory, global_options, _) =
        parse_test_files_and_symlinks(code, file_name, |filename, content, _file_options| {
            Ok(TestUnit {
                content: content.to_string(),
                name: filename.to_string(),
            })
        });

    // ts#64159 (test_case_parser.go:60): a raw current directory is rooted
    // against `srcFolder`.
    let current_directory = if raw_current_directory.is_empty() {
        SRC_FOLDER.to_string()
    } else {
        to_rooted_path(&raw_current_directory, SRC_FOLDER)
    };

    // unit tests always list files explicitly
    let mut all_files: BTreeMap<String, String> = BTreeMap::new();
    for data in &test_units {
        all_files.insert(
            to_rooted_path(&data.name, &current_directory),
            data.content.clone(),
        );
    }
    let parse_config_host = new_vfs_parse_config_host_with_symlinks(
        &all_files,
        &symlinks,
        &current_directory,
        true, /*useCaseSensitiveFileNames*/
    );

    // Go: test_case_parser.go:71 (tsgo#4712)
    // Content mappers are gated behind --runExternalCode, a command-line-only option. A test
    // opts in with a top-level `// @runExternalCode: true`, which we surface to the config
    // parse as an existing option so the gate passes and the mappers register.
    let existing_options = (global_options.get("runexternalcode").map(String::as_str)
        == Some("true"))
    .then(|| CompilerOptions {
        run_external_code: Tristate::True,
        ..CompilerOptions::default()
    });

    // check if project has tsconfig.json in the list of files
    let mut ts_config = None;
    let mut ts_config_file_unit_data = None;
    for i in 0..test_units.len() {
        let data = &test_units[i];
        if !get_config_name_from_file_name(&data.name).is_empty() {
            let config_file_name = to_rooted_path(&data.name, &current_directory);
            // Go `configFS.CaseSensitivity().PathKey(configFileName)`
            // (test_case_parser.go:86): `to_path` of the rooted name.
            let path = to_path(
                &config_file_name,
                &parse_config_host.get_current_directory(),
                parse_config_host.fs().use_case_sensitive_file_names(),
            );
            // Go: parser.ParseSourceFile(...ScriptKindJSON) wrapped in a
            // TsConfigSourceFile, which is `NewTsconfigSourceFileFromFilePath`.
            let ts_config_source_file =
                new_tsconfig_source_file_from_file_path(&config_file_name, path, &data.content);
            let config_dir = get_directory_path(&config_file_name);
            ts_config = Some(Rc::new(parse_json_source_file_config_file_content(
                ts_config_source_file,
                &parse_config_host,
                &config_dir,
                existing_options.as_ref(),
                None, /*existingOptionsRaw*/
                &config_file_name,
                &[],  /*resolutionStack*/
                None, /*extendedConfigCache*/
            )));
            ts_config_file_unit_data = Some(data.clone());

            // delete tsconfig file entry from the list
            test_units.remove(i);
            break;
        }
    }

    TestCaseContent {
        test_unit_data: test_units,
        ts_config,
        ts_config_file_unit_data,
        symlinks,
    }
}

// Go: test_case_parser.go:113 ParseTestFilesOptions
#[derive(Clone, Copy, Debug, Default)]
pub struct ParseTestFilesOptions {
    /// If true, allows test content to appear before the first @Filename directive.
    /// In this case, an implicit first file is created using the fileName parameter.
    /// This matches the behavior of the TypeScript fourslash test harness.
    pub allow_implicit_first_file: bool,
}

/// The result of `parse_test_files_and_symlinks`: units, symlinks, the
/// current directory, the global options and the parse error.
pub type ParsedTestFiles<T> = (
    Vec<T>,
    BTreeMap<String, String>,
    String,
    BTreeMap<String, String>,
    Option<String>,
);

// Go: test_case_parser.go:123 ParseTestFilesAndSymlinks
// Given a test file containing // @FileName and // @symlink directives,
// return an array of named units of code to be added to an existing compiler instance,
// along with a map of symlinks and the current directory.
pub fn parse_test_files_and_symlinks<T>(
    code: &str,
    file_name: &str,
    parse_file: impl FnMut(&str, &str, &BTreeMap<String, String>) -> Result<T, String>,
) -> ParsedTestFiles<T> {
    parse_test_files_and_symlinks_with_options(
        code,
        file_name,
        parse_file,
        ParseTestFilesOptions::default(),
    )
}

// Go: test_case_parser.go:131 ParseTestFilesAndSymlinksWithOptions
pub fn parse_test_files_and_symlinks_with_options<T>(
    code: &str,
    file_name: &str,
    mut parse_file: impl FnMut(&str, &str, &BTreeMap<String, String>) -> Result<T, String>,
    options: ParseTestFilesOptions,
) -> ParsedTestFiles<T> {
    // List of all the subfiles we've parsed out
    let mut test_units: Vec<T> = Vec::new();

    let lines = go_regex::split_line_delimiter(code);

    // Stuff related to the subfile we're parsing
    let mut current_file_content = String::new();
    let mut current_file_name = String::new();
    let mut seen_content_line = false;
    let mut has_seen_file = false;
    if options.allow_implicit_first_file {
        // For fourslash tests, initialize currentFileName to the fileName parameter
        // so content before the first @Filename directive goes into an implicit first file
        current_file_name = file_name.to_string();
    }
    let mut current_directory = String::new();
    let mut parse_error: Option<String> = None;
    let mut current_file_options: BTreeMap<String, String> = BTreeMap::new();
    let mut symlinks: BTreeMap<String, String> = BTreeMap::new();
    let mut global_options: BTreeMap<String, String> = BTreeMap::new();

    for line in lines {
        if parse_symlink_from_test(line, &mut symlinks) {
            continue;
        }
        if let Some((name, value)) = go_regex::match_option_line(line) {
            // Comment line, check for global/file @options and record them
            let meta_data_name = name.to_ascii_lowercase();
            let meta_data_value = value.trim().to_string();
            if meta_data_name == "currentdirectory" {
                current_directory = meta_data_value.clone();
            }
            if meta_data_name != "filename" {
                if meta_data_name == "symlink" && !current_file_name.is_empty() {
                    for link in meta_data_value.split(',') {
                        let link = link.trim();
                        if !link.is_empty() {
                            symlinks.insert(link.to_string(), current_file_name.clone());
                        }
                    }
                } else if FOURSLASH_DIRECTIVES.contains(&meta_data_name.as_str()) {
                    // File-specific option
                    current_file_options.insert(meta_data_name, meta_data_value);
                } else {
                    // Global option
                    // PORT: Go ignores a duplicate global option with another
                    // value (a panic there would break submodule tests).
                    global_options.insert(meta_data_name, meta_data_value);
                }
                continue;
            }

            // New metadata statement after having collected some code to go with the previous metadata
            if !current_file_name.is_empty() {
                // Store result file - always save for regular tests, but skip empty implicit first file for fourslash
                let should_save_file = !options.allow_implicit_first_file
                    || !current_file_content.is_empty()
                    || has_seen_file;
                if should_save_file {
                    has_seen_file = true;
                    match parse_file(
                        &current_file_name,
                        &current_file_content,
                        &current_file_options,
                    ) {
                        Ok(new_test_file) => test_units.push(new_test_file),
                        Err(e) => {
                            parse_error = Some(e);
                            break;
                        }
                    }
                }

                // Reset local data
                current_file_content.clear();
                seen_content_line = false;
                current_file_name = meta_data_value;
                current_file_options = BTreeMap::new();
            } else {
                // First metadata marker in the file
                let has_content_before_first_filename = !current_file_content.is_empty()
                    && skip_trivia(&current_file_content, 0) as usize != current_file_content.len();
                if has_content_before_first_filename && !options.allow_implicit_first_file {
                    panic!(
                        "Non-comment test content appears before the first '// @Filename' directive"
                    );
                }

                // If we have content before the first @Filename and AllowImplicitFirstFile is true,
                // we need to save it as an implicit first file before starting the new file
                if has_content_before_first_filename
                    && options.allow_implicit_first_file
                    && !current_file_name.is_empty()
                {
                    // Store the implicit first file
                    has_seen_file = true;
                    match parse_file(
                        &current_file_name,
                        &current_file_content,
                        &current_file_options,
                    ) {
                        Ok(new_test_file) => test_units.push(new_test_file),
                        Err(e) => {
                            parse_error = Some(e);
                            break;
                        }
                    }
                }

                // Reset for the new file
                current_file_content.clear();
                seen_content_line = false;
                current_file_name = value.trim().to_string();
                current_file_options = BTreeMap::new();
            }
        } else {
            // Subfile content line
            // Append to the current subfile content, inserting a newline if needed
            // For fourslash tests, use seenContentLine to preserve leading blank lines
            // (matching TS fourslash's //// content markers). For compiler tests, use
            // Len() != 0 which drops leading blanks (matching TS's harness behavior).
            if options.allow_implicit_first_file {
                if seen_content_line {
                    current_file_content.push('\n');
                }
                seen_content_line = true;
            } else if !current_file_content.is_empty() {
                current_file_content.push('\n');
            }
            current_file_content.push_str(line);
        }
    }

    // normalize the fileName for the single file case
    if test_units.is_empty() && current_file_name.is_empty() {
        current_file_name = get_base_file_name(file_name);
    }

    // if there are no parse errors so far, parse the rest of the file
    if parse_error.is_none() {
        // EOF, push whatever remains
        match parse_file(
            &current_file_name,
            &current_file_content,
            &current_file_options,
        ) {
            Ok(new_test_file2) => test_units.push(new_test_file2),
            Err(e) => parse_error = Some(e),
        }
    }

    (
        test_units,
        symlinks,
        current_directory,
        global_options,
        parse_error,
    )
}

// Go: test_case_parser.go:264 extractCompilerSettings
pub fn extract_compiler_settings(content: &str) -> RawCompilerSettings {
    let mut opts = RawCompilerSettings::new();

    for (name, value) in go_regex::find_all_options(content) {
        let value = value.trim();
        opts.insert(
            name.to_ascii_lowercase(),
            value.strip_suffix(';').unwrap_or(value).to_string(),
        );
    }

    opts
}

// Go: test_case_parser.go:274 parseSymlinkFromTest
fn parse_symlink_from_test(line: &str, symlinks: &mut BTreeMap<String, String>) -> bool {
    let Some((target, link)) = go_regex::match_link_line(line) else {
        return false;
    };

    symlinks.insert(link.trim().to_string(), target.trim().to_string());
    true
}

// Go: test_case_parser_test.go:10 TestMakeUnitsFromTest
#[test]
fn test_make_units_from_test() {
    let code = r#"// @strict: true
// @noEmit: true
// @filename: firstFile.ts
function foo() { return "a"; }
// normal comment
// @filename: secondFile.ts
// some other comment
function bar() { return "b"; }"#;
    let test_unit1 = TestUnit {
        content: "function foo() { return \"a\"; }\n// normal comment".to_string(),
        name: "firstFile.ts".to_string(),
    };
    let test_unit2 = TestUnit {
        content: "// some other comment\nfunction bar() { return \"b\"; }".to_string(),
        name: "secondFile.ts".to_string(),
    };
    let actual = make_units_from_test(code, "simpleTest.ts");
    assert_eq!(actual.test_unit_data, vec![test_unit1, test_unit2]);
    assert!(actual.ts_config.is_none());
    assert!(actual.ts_config_file_unit_data.is_none());
    assert_eq!(actual.symlinks, BTreeMap::new());
}
