//! Rust port of `internal/tsoptions/commandlineparser_test.go`.
//!
//! Baselines: `tsoptions/commandLineParsing/{parseCommandLine,parseBuildOptions}`.
//! The TypeScript baselines under
//! `testdata/fixtures/typescript/tests/baselines/reference/config/commandLineParsing`
//! are test input (the submodule copy before microsoft/TypeScript 5f647a841a).
//!
//! PORT: Go `json.Unmarshal` of the options JSON into `core.CompilerOptions`
//! and `core.BuildOptions`, and the `assert.DeepEqual` with the TypeScript
//! baseline options, are not ported: the Rust option structs have no JSON
//! decoder. Each site has a `// PORT:` note. ts#64457 removed the watch
//! options and their rows. The generated baseline, the file names and the
//! projects are compared as in Go.

use std::path::PathBuf;

use ts_goport::execute::build::command_line;
use ts_goport::execute::tsc::diagnostics::FormattingOptions;
use ts_goport::frontend::prelude::*;

use super::tsoptionstest::{
    BuildOptionsJson, CompilerOptionsJson, OptionsMapJson, Subtests, TempDir, VfsParseConfigHost,
    file_map, marshal_or_empty, new_vfs_parse_config_host, parse_command_line_test_worker, strs,
    write_format_diagnostics,
};
use crate::support::baseline;

// Go: commandlineparser_test.go:25 TestCommandLineParseResult
#[test]
fn command_line_parse_result() {
    let parse_command_line_sub_scenarios: Vec<SubScenarioInput> = vec![
        // --lib es6 0.ts
        SubScenarioInput::new(
            "Parse single option of library flag",
            &["--lib", "es6", "0.ts"],
        ),
        SubScenarioInput::new(
            "Handles may only be used with --build flags",
            &["--build", "--clean", "--dry", "--force", "--verbose"],
        ),
        // --declarations --allowTS
        SubScenarioInput::new(
            "Handles did you mean for misspelt flags",
            &["--declarations", "--allowTS"],
        ),
        // --lib es5,es2015.symbol.wellknown 0.ts
        SubScenarioInput::new(
            "Parse multiple options of library flags",
            &["--lib", "es5,es2015.symbol.wellknown", "0.ts"],
        ),
        // --lib es5,invalidOption 0.ts
        SubScenarioInput::new(
            "Parse invalid option of library flags",
            &["--lib", "es5,invalidOption", "0.ts"],
        ),
        // 0.ts --jsx
        SubScenarioInput::new("Parse empty options of --jsx", &["0.ts", "--jsx"]),
        // 0.ts --
        SubScenarioInput::new("Parse empty options of --module", &["0.ts", "--module"]),
        // 0.ts --newLine
        SubScenarioInput::new("Parse empty options of --newLine", &["0.ts", "--newLine"]),
        // 0.ts --target
        SubScenarioInput::new("Parse empty options of --target", &["0.ts", "--target"]),
        // 0.ts --moduleResolution
        SubScenarioInput::new(
            "Parse empty options of --moduleResolution",
            &["0.ts", "--moduleResolution"],
        ),
        // 0.ts --lib
        SubScenarioInput::new("Parse empty options of --lib", &["0.ts", "--lib"]),
        // 0.ts --lib
        // This test is an error because the empty string is falsey
        SubScenarioInput::new("Parse empty string of --lib", &["0.ts", "--lib", ""]),
        // 0.ts --lib
        SubScenarioInput::new(
            "Parse immediately following command line argument of --lib",
            &["0.ts", "--lib", "--sourcemap"],
        ),
        // --lib es5, es7 0.ts
        SubScenarioInput::new(
            "Parse --lib option with extra comma",
            &["--lib", "es5,", "es7", "0.ts"],
        ),
        // --lib es5, es7 0.ts
        SubScenarioInput::new(
            "Parse --lib option with trailing white-space",
            &["--lib", "es5, ", "es7", "0.ts"],
        ),
        // --lib es5,es2015.symbol.wellknown --target es5 0.ts
        SubScenarioInput::new(
            "Parse multiple compiler flags with input files at the end",
            &[
                "--lib",
                "es5,es2015.symbol.wellknown",
                "--target",
                "es5",
                "0.ts",
            ],
        ),
        // --module commonjs --target es5 0.ts --lib es5,es2015.symbol.wellknown
        SubScenarioInput::new(
            "Parse multiple compiler flags with input files in the middle",
            &[
                "--module",
                "commonjs",
                "--target",
                "es5",
                "0.ts",
                "--lib",
                "es5,es2015.symbol.wellknown",
            ],
        ),
        // --module commonjs --target es5 --lib es5 0.ts --library es2015.array,es2015.symbol.wellknown
        SubScenarioInput::new(
            "Parse multiple library compiler flags ",
            &[
                "--module",
                "commonjs",
                "--target",
                "es5",
                "--lib",
                "es5",
                "0.ts",
                "--lib",
                "es2015.core, es2015.symbol.wellknown ",
            ],
        ),
        SubScenarioInput::new(
            "Parse explicit boolean flag value",
            &["--strictNullChecks", "false", "0.ts"],
        ),
        SubScenarioInput::new(
            "Parse non boolean argument after boolean flag",
            &["--noImplicitAny", "t", "0.ts"],
        ),
        SubScenarioInput::new("Parse implicit boolean flag value", &["--strictNullChecks"]),
        SubScenarioInput::new("parse --incremental", &["--incremental", "0.ts"]),
        SubScenarioInput::new(
            "parse --tsBuildInfoFile",
            &["--tsBuildInfoFile", "build.tsbuildinfo", "0.ts"],
        ),
        SubScenarioInput::new(
            "allows tsconfig only option to be set to null",
            &["--composite", "null", "-tsBuildInfoFile", "null", "0.ts"],
        ),
    ];

    let mut t = Subtests::new("TestCommandLineParseResult");
    for test_case in &parse_command_line_sub_scenarios {
        test_case
            .create_sub_scenario("parseCommandLine")
            .assert_parse_result(&mut t);
    }
    t.finish();
}

// Go: commandlineparser_test.go:78 TestRemovedWatchOptions (ts#64457)
#[test]
fn removed_watch_options() {
    let host = new_vfs_parse_config_host(&file_map(&[]), "/project", true);
    let mut t = Subtests::new("TestRemovedWatchOptions");
    for option in [
        "watchInterval",
        "watchFile",
        "watchDirectory",
        "fallbackPolling",
        "synchronousWatchDirectory",
        "excludeDirectories",
        "excludeFiles",
    ] {
        t.run(option, || {
            let args = strs(&["--watch", &format!("--{option}")]);
            let parsed = parse_command_line(&args, &host);
            assert!(parsed.compiler_options().watch.is_true());
            assert_eq!(parsed.errors.len(), 1);
            let build = command_line::parse_build_command_line(&args, &host);
            assert!(build.compiler_options.watch.is_true());
            assert_eq!(build.errors.len(), 1);
            let mut errors = String::new();
            let formatting = FormattingOptions {
                new_line: "\n".to_string(),
                ..Default::default()
            };
            write_format_diagnostics(&mut errors, &parsed.errors, &formatting);
            write_format_diagnostics(&mut errors, &build.errors, &formatting);
            baseline::run(
                &format!("{option}.js"),
                &errors,
                &baseline::Options {
                    subfolder: "tsoptions/removedWatchOptions".into(),
                    ..Default::default()
                },
            )
        });
    }
    t.finish();
}

// Go: commandlineparser_test.go:104 TestResponseFileDoesNotPanic
#[test]
fn response_file_does_not_panic() {
    // Passing `@` with an empty or relative filename should not panic.
    // It should produce a diagnostic error instead.
    let cwd = TempDir::new();
    let mut t = Subtests::new("TestResponseFileDoesNotPanic");
    t.run("empty response file", || {
        let parsed = parse_command_line_test_worker(&[], &strs(&["@"]), osvfs_fs(), &cwd.path());
        assert!(
            !parsed.errors.is_empty(),
            "expected an error for empty response file name"
        );
        Ok(())
    });

    t.run("relative response file", || {
        let parsed =
            parse_command_line_test_worker(&[], &strs(&["@blah"]), osvfs_fs(), &cwd.path());
        assert!(
            !parsed.errors.is_empty(),
            "expected an error for non-existent response file"
        );
        Ok(())
    });
    t.finish();
}

// Go: commandlineparser_test.go:108 TestResponseFileParsing
#[test]
fn response_file_parsing() {
    let mut t = Subtests::new("TestResponseFileParsing");
    t.run("final token without trailing whitespace", || {
        let host = new_vfs_parse_config_host(
            &file_map(&[("/project/args.txt", "--strict --outDir dist")]),
            "/project",
            true,
        );
        let parsed = parse_command_line(&strs(&["@args.txt"]), &host);
        assert_eq!(parsed.errors.len(), 0);
        assert!(parsed.compiler_options().strict.is_true());
        assert_eq!(parsed.compiler_options().out_dir, "/project/dist");
        Ok(())
    });

    t.run("cyclic response files", || {
        let host = new_vfs_parse_config_host(
            &file_map(&[
                ("/project/a.txt", "@/project/b.txt --strict"),
                ("/project/b.txt", "@/project/a.txt --outDir dist"),
            ]),
            "/project",
            true,
        );
        let parsed = parse_command_line(&strs(&["@a.txt"]), &host);
        assert_eq!(parsed.errors.len(), 0);
        assert!(parsed.compiler_options().strict.is_true());
        assert_eq!(parsed.compiler_options().out_dir, "/project/dist");
        Ok(())
    });
    t.finish();
}

// Go: commandlineparser_test.go:135 TestParseCommandLineTypeRootsRelativePath
#[test]
fn parse_command_line_type_roots_relative_path() {
    let host = new_vfs_parse_config_host(
        &file_map(&[("/home/project/bug.ts", "let x = 1;")]),
        "/home/project",
        true,
    );

    let cmd_line = parse_command_line(&strs(&["--typeRoots", "t", "bug.ts"]), &host);

    let type_roots = cmd_line.compiler_options().type_roots.as_ref();
    assert!(type_roots.is_some(), "typeRoots should not be nil");
    let type_roots = type_roots.unwrap();
    assert_eq!(type_roots.len(), 1);
    assert!(
        is_rooted_disk_path(&type_roots[0]),
        "typeRoots entry should be an absolute path, got: {}",
        type_roots[0]
    );
    assert!(
        type_roots[0].ends_with("/t"),
        "typeRoots entry should end with '/t', got: {}",
        type_roots[0]
    );
}

// Go: commandlineparser_test.go:125 TestCustomConditionsNullOverride
#[test]
fn custom_conditions_null_override() {
    let files = file_map(&[
        (
            "/project/tsconfig.json",
            "{\n  \"compilerOptions\": {\n    \"customConditions\": [\"condition1\", \"condition2\"]\n  }\n}",
        ),
        ("/project/index.ts", "console.log(\"Hello, World!\");"),
    ]);

    let host = new_vfs_parse_config_host(&files, "/project", true);

    // Parse command line with --customConditions null
    let cmd_line = parse_command_line(
        &strs(&["--project", "/project", "--customConditions", "null"]),
        &host,
    );

    // Check that the raw options contain null for customConditions
    let CompilerOptionsValue::Map(raw_map) = &cmd_line.raw else {
        panic!("Raw options should be an OrderedMap");
    };
    let custom_conditions_raw = raw_map.get("customConditions");
    assert!(
        custom_conditions_raw.is_some(),
        "customConditions should exist in raw options"
    );
    assert!(
        custom_conditions_raw.unwrap().is_nil(),
        "customConditions should be nil in raw options, got: {custom_conditions_raw:?}"
    );

    // Now parse the config file with the command line options
    // Wrap command line options in "compilerOptions" key to match tsconfig.json structure
    let mut wrapped_raw: IndexMap<String, CompilerOptionsValue> = IndexMap::default();
    wrapped_raw.insert(
        "compilerOptions".to_string(),
        CompilerOptionsValue::Map(raw_map.clone()),
    );
    let (parsed_config, errors) = get_parsed_command_line_of_config_file(
        "/project/tsconfig.json",
        Some(&**cmd_line.compiler_options()),
        Some(&wrapped_raw),
        &host,
        None,
    );

    assert!(
        errors.is_empty(),
        "Should not have errors: {:?}",
        errors.iter().map(|d| d.code).collect::<Vec<_>>()
    );

    // Check that customConditions is nil (overridden by command line)
    // PORT: Go would panic on a nil `parsedConfig` here; `expect` does too.
    let parsed_config = parsed_config.expect("parsed config");
    let custom_conditions = &parsed_config.compiler_options().custom_conditions;
    assert!(
        custom_conditions.is_none(),
        "customConditions should be nil after override, got: {custom_conditions:?}"
    );
}

// Go: commandlineparser_test.go:196 TestParseCommandLineVerifyNull
#[test]
fn parse_command_line_verify_null() {
    let mut t = Subtests::new("TestParseCommandLineVerifyNull");

    // run test for boolean
    SubScenarioInput::new(
        "allows setting option type boolean to false",
        &["--composite", "false", "0.ts"],
    )
    .create_sub_scenario("parseCommandLine")
    .assert_parse_result(&mut t);

    let verify_null_sub_scenarios: Vec<VerifyNull> = vec![
        VerifyNull {
            sub_scenario: "option of type boolean",
            option_name: "composite",
            non_null_value: "true",
            opt_decls: &[],
        },
        VerifyNull {
            sub_scenario: "option of type object",
            option_name: "paths",
            non_null_value: "",
            opt_decls: &[],
        },
        VerifyNull {
            sub_scenario: "option of type list",
            option_name: "rootDirs",
            non_null_value: "abc,xyz",
            opt_decls: &[],
        },
        create_verify_null_for_non_null_included(
            "option of type string",
            CommandLineOptionKind::STRING,
            "hello",
        ),
        create_verify_null_for_non_null_included(
            "option of type number",
            CommandLineOptionKind::NUMBER,
            "10",
        ),
        // todo: make the following work for tests -- currently it is difficult to do extra options of enum type
        // createVerifyNullForNonNullIncluded("option of type custom map", CommandLineOptionTypeEnum, "node"),
    ];

    for verify_null_case in &verify_null_sub_scenarios {
        create_sub_scenario(
            "parseCommandLine",
            &format!(
                "{} allows setting it to null",
                verify_null_case.sub_scenario
            ),
            strs(&[
                &format!("--{}", verify_null_case.option_name),
                "null",
                "0.ts",
            ]),
            Some(verify_null_case.opt_decls),
        )
        .assert_parse_result(&mut t);

        if !verify_null_case.non_null_value.is_empty() {
            create_sub_scenario(
                "parseCommandLine",
                &format!(
                    "{} errors if non null value is passed",
                    verify_null_case.sub_scenario
                ),
                strs(&[
                    &format!("--{}", verify_null_case.option_name),
                    verify_null_case.non_null_value,
                    "0.ts",
                ]),
                Some(verify_null_case.opt_decls),
            )
            .assert_parse_result(&mut t);
        }

        create_sub_scenario(
            "parseCommandLine",
            &format!(
                "{} errors if its followed by another option",
                verify_null_case.sub_scenario
            ),
            strs(&[
                "0.ts",
                "--strictNullChecks",
                &format!("--{}", verify_null_case.option_name),
            ]),
            Some(verify_null_case.opt_decls),
        )
        .assert_parse_result(&mut t);

        create_sub_scenario(
            "parseCommandLine",
            &format!(
                "{} errors if its last option",
                verify_null_case.sub_scenario
            ),
            strs(&["0.ts", &format!("--{}", verify_null_case.option_name)]),
            Some(verify_null_case.opt_decls),
        )
        .assert_parse_result(&mut t);
    }
    t.finish();
}

// Go: commandlineparser_test.go:231 createVerifyNullForNonNullIncluded
// PORT: the Go `slices.Concat` result is leaked so the list is `&'static`,
// as the parser's declaration lists are.
fn create_verify_null_for_non_null_included(
    sub_scenario: &'static str,
    kind: CommandLineOptionKind,
    non_null_value: &'static str,
) -> VerifyNull {
    let mut opt_decls: Vec<&'static CommandLineOption> = OPTIONS_DECLARATIONS.to_vec();
    opt_decls.push(Box::leak(Box::new(CommandLineOption {
        name: "optionName",
        kind,
        is_ts_config_only: true,
        category: Some(diag::Backwards_Compatibility),
        description: Some(diag::Enable_project_compilation),
        default_value_description: CompilerOptionsValue::Nil,
        ..Default::default()
    })));
    VerifyNull {
        sub_scenario,
        option_name: "optionName",
        non_null_value,
        opt_decls: Box::leak(opt_decls.into_boxed_slice()),
    }
}

impl CommandLineSubScenario {
    // Go: commandlineparser_test.go:286 (commandLineSubScenario).assertParseResult
    fn assert_parse_result(&self, t: &mut Subtests) {
        t.run(&self.test_name, || {
            let original_baseline = self.baseline.read_file()?;
            let ts_baseline = parse_existing_compiler_baseline(&original_baseline);

            // f.workerDiagnostic is either defined or set to default pointer in `createSubScenario`
            let temp_dir = TempDir::new();
            let parsed = parse_command_line_test_worker(
                self.opt_decls,
                &self.command_line,
                osvfs_fs(),
                &temp_dir.path(),
            );

            let new_baseline_file_names = parsed.file_names.join(",");
            assert_eq!(ts_baseline.file_names, new_baseline_file_names);

            let o = marshal_or_empty(&OptionsMapJson(&parsed.options));
            // PORT: Go unmarshals `o` into a `core.CompilerOptions` and
            // compares it with `tsBaseline.options` (assert.DeepEqual). The
            // Rust `CompilerOptions` has no JSON decoder, so that step is
            // not ported.

            let mut new_baseline_errors = String::new();
            write_format_diagnostics(
                &mut new_baseline_errors,
                &parsed.errors,
                &FormattingOptions {
                    new_line: "\n".to_string(),
                    ..Default::default()
                },
            );

            // !!!
            // useful for debugging--compares the new errors with the old errors. currently will NOT pass because of unimplemented options, not completely identical enum options, etc
            // assert.Equal(t, tsBaseline.errors, newBaselineErrors)

            baseline::run(
                &format!("{}.js", self.test_name),
                &format_new_baseline(
                    &self.command_line,
                    &o,
                    &new_baseline_file_names,
                    &new_baseline_errors,
                ),
                &baseline::Options {
                    subfolder: "tsoptions/commandLineParsing".into(),
                    ..Default::default()
                },
            )
        });
    }
}

// Go: go strings.Cut
fn cut<'a>(s: &'a str, sep: &str) -> (&'a str, &'a str, bool) {
    match s.find(sep) {
        Some(i) => (&s[..i], &s[i + sep.len()..], true),
        None => (s, "", false),
    }
}

// Go: commandlineparser_test.go:317 parseExistingCompilerBaseline
fn parse_existing_compiler_baseline(baseline: &str) -> TestCommandLineParser {
    let (_, rest, _) = cut(baseline, "CompilerOptions::\n");
    let (compiler_options, rest, _) = cut(rest, "\nWatchOptions::\n");
    let (_, rest, _) = cut(rest, "\nFileNames::\n");
    let (file_names, errors, _) = cut(rest, "\nErrors::\n");

    // PORT: Go unmarshals `compilerOptions` into `core.CompilerOptions` and
    // asserts no error. The Rust option types have no JSON decoder; the JSON
    // text is kept instead.
    TestCommandLineParser {
        options: compiler_options.to_string(),
        file_names: file_names.to_string(),
        errors: errors.to_string(),
    }
}

// Go: commandlineparser_test.go:334 formatNewBaseline
fn format_new_baseline(
    command_line: &[String],
    opts: &str,
    file_names: &str,
    errors: &str,
) -> String {
    let mut formatted = String::new();
    formatted.push_str("Args::\n");
    formatted.push('[');
    for (i, arg) in command_line.iter().enumerate() {
        if i > 0 {
            formatted.push_str(", ");
        }
        formatted.push('"');
        formatted.push_str(arg);
        formatted.push('"');
    }
    formatted.push(']');
    formatted.push_str("\n\nCompilerOptions::\n");
    formatted.push_str(opts);
    formatted.push_str("\n\nFileNames::\n");
    formatted.push_str(file_names);
    formatted.push_str("\n\nErrors::\n");
    formatted.push_str(errors);
    formatted
}

/// Go `func() *TestCommandLineParserBuild`, which reads the TypeScript
/// baseline.
type GetTsBaseline<'a> = &'a dyn Fn() -> Result<TestCommandLineParserBuild, String>;

impl CommandLineSubScenario {
    // Go: commandlineparser_test.go:361 (commandLineSubScenario).assertBuildParseResult
    fn assert_build_parse_result(&self, t: &mut Subtests) {
        let get_ts_baseline = || -> Result<TestCommandLineParserBuild, String> {
            let original_baseline = self.baseline.read_file()?;
            Ok(parse_existing_compiler_baseline_build(&original_baseline))
        };
        self.assert_build_parse_result_with_ts_baseline(t, Some(&get_ts_baseline));
    }

    // Go: commandlineparser_test.go:369 (commandLineSubScenario).assertBuildParseResultWithTsBaseline
    fn assert_build_parse_result_with_ts_baseline(
        &self,
        t: &mut Subtests,
        get_ts_baseline: Option<GetTsBaseline<'_>>,
    ) {
        t.run(&self.test_name, || {
            let ts_baseline = match get_ts_baseline {
                Some(get_ts_baseline) => Some(get_ts_baseline()?),
                None => None,
            };

            // f.workerDiagnostic is either defined or set to default pointer in `createSubScenario`
            let parsed = command_line::parse_build_command_line(
                &self.command_line,
                &VfsParseConfigHost {
                    vfs: osvfs_fs(),
                    current_directory: normalize_slashes(
                        &baseline::test_data_path().to_string_lossy(),
                    ),
                },
            );

            let new_baseline_projects = parsed.projects.join(",");
            if let Some(ts_baseline) = &ts_baseline {
                assert_eq!(ts_baseline.projects, new_baseline_projects);
            }

            let o = marshal_or_empty(&BuildOptionsJson(&parsed.build_options));
            // PORT: Go unmarshals `o` into a `core.BuildOptions` and, with a
            // TypeScript baseline, compares it with `tsBaseline.options`
            // (assert.DeepEqual). The Rust `BuildOptions` has no JSON
            // decoder, so that step is not ported.

            let compiler_opts = marshal_or_empty(&CompilerOptionsJson(&parsed.compiler_options));
            // PORT: Go unmarshals `compilerOpts` into a `core.CompilerOptions`
            // and, with a TypeScript baseline, compares it with
            // `tsBaseline.compilerOptions` (assert.DeepEqual). Not ported,
            // as above.

            let mut new_baseline_errors = String::new();
            write_format_diagnostics(
                &mut new_baseline_errors,
                &parsed.errors,
                &FormattingOptions {
                    new_line: "\n".to_string(),
                    ..Default::default()
                },
            );

            // !!!
            // useful for debugging--compares the new errors with the old errors. currently will NOT pass because of unimplemented options, not completely identical enum options, etc
            // assert.Equal(t, tsBaseline.errors, newBaselineErrors)

            baseline::run(
                &format!("{}.js", self.test_name),
                &format_new_baseline_build(
                    &self.command_line,
                    &o,
                    &compiler_opts,
                    &new_baseline_projects,
                    &new_baseline_errors,
                ),
                &baseline::Options {
                    subfolder: "tsoptions/commandLineParsing".into(),
                    ..Default::default()
                },
            )
        });
    }
}

// Go: commandlineparser_test.go:415 parseExistingCompilerBaselineBuild
fn parse_existing_compiler_baseline_build(baseline: &str) -> TestCommandLineParserBuild {
    let (_, rest, _) = cut(baseline, "buildOptions::\n");
    let (build_options, rest, _) = cut(rest, "\nWatchOptions::\n");
    let (_, rest, _) = cut(rest, "\nProjects::\n");
    let (projects, errors, _) = cut(rest, "\nErrors::\n");

    // PORT: Go unmarshals `buildOptions` into both `core.BuildOptions` and
    // `core.CompilerOptions` and asserts no error. The Rust option types have
    // no JSON decoder; the JSON text is kept instead.
    TestCommandLineParserBuild {
        options: build_options.to_string(),
        compiler_options: build_options.to_string(),
        projects: projects.to_string(),
        errors: errors.to_string(),
    }
}

// Go: commandlineparser_test.go:437 formatNewBaselineBuild
fn format_new_baseline_build(
    command_line: &[String],
    opts: &str,
    compiler_opts: &str,
    projects: &str,
    errors: &str,
) -> String {
    let mut formatted = String::new();
    formatted.push_str("Args::\n");
    formatted.push('[');
    for (i, arg) in command_line.iter().enumerate() {
        if i > 0 {
            formatted.push_str(", ");
        }
        formatted.push('"');
        formatted.push_str(arg);
        formatted.push('"');
    }
    formatted.push(']');
    formatted.push_str("\n\nbuildOptions::\n");
    formatted.push_str(opts);
    formatted.push_str("\n\ncompilerOptions::\n");
    formatted.push_str(compiler_opts);
    formatted.push_str("\n\nProjects::\n");
    formatted.push_str(projects);
    formatted.push_str("\n\nErrors::\n");
    formatted.push_str(errors);
    formatted
}

// Go: commandlineparser_test.go:488 createSubScenario
// PORT: the Go variadic `opts ...[]*CommandLineOption` is an `Option`; nil
// and an empty list are the empty slice.
fn create_sub_scenario(
    scenario_kind: &str,
    sub_scenario_name: &str,
    commandline: Vec<String>,
    opts: Option<&'static [&'static CommandLineOption]>,
) -> CommandLineSubScenario {
    let sub_scenario_name = format!("{scenario_kind}/{sub_scenario_name}");
    let baseline_file_name =
        format!("tests/baselines/reference/config/commandLineParsing/{sub_scenario_name}.js");

    CommandLineSubScenario {
        baseline: FileFixture::from_file(
            &sub_scenario_name,
            baseline::test_data_path()
                .join("fixtures")
                .join("typescript")
                .join(baseline_file_name),
        ),
        test_name: sub_scenario_name,
        command_line: commandline,
        opt_decls: opts.unwrap_or(&[]),
    }
}

// Go: commandlineparser_test.go:479 subScenarioInput
struct SubScenarioInput {
    name: &'static str,
    command_line_args: Vec<String>,
}

impl SubScenarioInput {
    fn new(name: &'static str, command_line_args: &[&str]) -> SubScenarioInput {
        SubScenarioInput {
            name,
            command_line_args: strs(command_line_args),
        }
    }

    // Go: commandlineparser_test.go:484 (subScenarioInput).createSubScenario
    fn create_sub_scenario(&self, scenario_kind: &str) -> CommandLineSubScenario {
        create_sub_scenario(
            scenario_kind,
            self.name,
            self.command_line_args.clone(),
            None,
        )
    }
}

// Go: testutil/filefixture/filefixture.go:16 fromFile
// PORT: only `FromFile` and `ReadFile` are used here. Go caches the read;
// each subtest reads its fixture once, so there is no cache.
struct FileFixture {
    name: String,
    path: PathBuf,
}

impl FileFixture {
    // Go: testutil/filefixture/filefixture.go:22 FromFile
    fn from_file(name: &str, path: PathBuf) -> FileFixture {
        FileFixture {
            name: name.to_string(),
            path,
        }
    }

    // Go: testutil/filefixture/filefixture.go:45 (*fromFile).ReadFile
    // PORT: Go `tb.Fatalf` is an `Err`.
    fn read_file(&self) -> Result<String, String> {
        std::fs::read_to_string(&self.path).map_err(|err| {
            format!(
                "Failed to read test fixture {:?}: {err}",
                self.path.display().to_string()
            )
        })
    }
}

// Go: commandlineparser_test.go:488 commandLineSubScenario
struct CommandLineSubScenario {
    baseline: FileFixture,
    test_name: String,
    command_line: Vec<String>,
    opt_decls: &'static [&'static CommandLineOption],
}

// Go: commandlineparser_test.go:495 verifyNull
struct VerifyNull {
    sub_scenario: &'static str,
    option_name: &'static str,
    non_null_value: &'static str,
    opt_decls: &'static [&'static CommandLineOption],
}

// Go: commandlineparser_test.go:506 TestCommandLineParser
// PORT: the Go option field is the decoded `*core.CompilerOptions`. Without
// a JSON decoder it holds the JSON text.
struct TestCommandLineParser {
    options: String,
    file_names: String,
    errors: String,
}

// Go: commandlineparser_test.go:511 TestCommandLineParserBuild
// PORT: as `TestCommandLineParser`, the option fields hold the JSON text.
struct TestCommandLineParserBuild {
    options: String,
    compiler_options: String,
    projects: String,
    errors: String,
}

// Go: commandlineparser_test.go:517 TestParseBuildCommandLine
#[test]
fn parse_build_command_line() {
    let parse_command_line_sub_scenarios: Vec<SubScenarioInput> = vec![
        SubScenarioInput::new("parse build without any options ", &[]),
        SubScenarioInput::new("Parse multiple options", &["--verbose", "--force", "tests"]),
        SubScenarioInput::new(
            "Parse option with invalid option",
            &["--verbose", "--invalidOption"],
        ),
        SubScenarioInput::new(
            "Parse multiple flags with input projects at the end",
            &["--force", "--verbose", "src", "tests"],
        ),
        SubScenarioInput::new(
            "Parse multiple flags with input projects in the middle",
            &["--force", "src", "tests", "--verbose"],
        ),
        SubScenarioInput::new(
            "Parse multiple flags with input projects in the beginning",
            &["src", "tests", "--force", "--verbose"],
        ),
        SubScenarioInput::new(
            "parse build with --incremental",
            &["--incremental", "tests"],
        ),
        SubScenarioInput::new(
            "parse build with --locale en-us",
            &["--locale", "en-us", "src"],
        ),
        SubScenarioInput::new(
            "parse build with --tsBuildInfoFile",
            &["--tsBuildInfoFile", "build.tsbuildinfo", "tests"],
        ),
        SubScenarioInput::new(
            "reports other common may not be used with --build flags",
            &["--strict"],
        ),
        SubScenarioInput::new(
            "--clean and --force together is invalid",
            &["--clean", "--force"],
        ),
        SubScenarioInput::new(
            "--clean and --verbose together is invalid",
            &["--clean", "--verbose"],
        ),
        SubScenarioInput::new(
            "--clean and --watch together is invalid",
            &["--clean", "--watch"],
        ),
        SubScenarioInput::new(
            "--watch and --dry together is invalid",
            &["--watch", "--dry"],
        ),
    ];

    let mut t = Subtests::new("TestParseBuildCommandLine");
    for test_case in &parse_command_line_sub_scenarios {
        test_case
            .create_sub_scenario("parseBuildOptions")
            .assert_build_parse_result(&mut t);
    }

    let extra_scenarios: Vec<SubScenarioInput> = vec![
        SubScenarioInput::new("parse --builders", &["--builders", "2"]),
        SubScenarioInput::new(
            "--singleThreaded and --builders together",
            &["--singleThreaded", "--builders", "2"],
        ),
        SubScenarioInput::new("reports error when --builders is 0", &["--builders", "0"]),
        SubScenarioInput::new(
            "reports error when --builders is negative",
            &["--builders", "-1"],
        ),
        SubScenarioInput::new(
            "reports error when --builders is invalid type",
            &["--builders", "invalid"],
        ),
    ];

    for test_case in &extra_scenarios {
        test_case
            .create_sub_scenario("parseBuildOptions")
            .assert_build_parse_result_with_ts_baseline(&mut t, None);
    }
    t.finish();
}

// Go: commandlineparser_test.go:585 TestAffectsBuildInfo (at 673a5f17d713; removed by
// ts#64457, whose option generator keeps this rule)
// PORT: kept. The port has no option generator (bump D gen decision), so its
// hand-written declarations still need this check.
#[test]
fn affects_build_info() {
    let mut t = Subtests::new("TestAffectsBuildInfo");
    t.run(
        "should have affectsBuildInfo true for every option with affectsSemanticDiagnostics",
        || {
            for option in OPTIONS_DECLARATIONS.iter() {
                if option.affects_semantic_diagnostics {
                    // semantic diagnostics affect the build info, so ensure they're included
                    assert!(option.affects_build_info, "{}", option.name);
                }
            }
            Ok(())
        },
    );
    t.finish();
}
