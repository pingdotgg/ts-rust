//! Go: internal/testrunner/transpile_runner.go and transpile_runner_test.go
//! (#4849): the `transpileModule` and `transpileDeclaration` baselines of
//! the transpile cases. At the typescript-go layout they are the TypeScript
//! submodule's `tests/cases/transpile` and the baselines are in
//! `submodule/transpile`; at the merged layout (5f647a841a, see
//! `baseline::is_merged_layout`) they are `testdata/tests/cases/transpile`
//! and `transpile`.
//!
//! PORT: Go runs each test configuration as a subtest of `TestTranspile`.
//! Here each configuration runs in a child process, as in the compiler
//! runner (child.rs): a panic or a crash in one configuration fails only
//! that configuration. The child reports these kinds: `options` (Go
//! `SetOptionsFromTestConfig` failed), `js` (the `runKind` of
//! `TranspileModule`) and `dts` (the `runKind` of `TranspileDeclaration`).
//! The parent reads `TRANSPILE_RUNNER_*` (see child.rs).

use std::collections::HashSet;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::OnceLock;

use ts_goport::baseline::type_symbol::TestFile;
use ts_goport::frontend::outputpaths::get_output_extension;
use ts_goport::frontend::prelude::*;
use ts_goport::gostd::context;
use ts_goport::transpile;

use super::child;
use super::go_regex;
use super::harness::{
    HarnessOptions, NamedTestConfiguration, enumerate_files, get_file_based_test_configurations,
    set_options_from_test_config,
};
use super::runner::{ConfigCase, Outcome, Report, SRC_FOLDER, payload_text, run_subtest};
use super::test_case_parser::{TestUnit, extract_compiler_settings, make_units_from_test};
use super::tsbaseline;
use crate::support::baseline::{self, Options};

/// The `ConfigCase::suite` of a transpile case.
pub const TRANSPILE_SUITE: &str = "transpile";
/// The environment prefix of the transpile runner (see child.rs).
const TRANSPILE_ENV_PREFIX: &str = "TRANSPILE_RUNNER";

// Go: transpile_runner.go:25 transpileVaryBy
fn transpile_vary_by() -> &'static HashSet<String> {
    static MAP: OnceLock<HashSet<String>> = OnceLock::new();
    MAP.get_or_init(|| {
        ["declarationmap", "sourcemap", "inlinesourcemap"]
            .into_iter()
            .map(str::to_string)
            .collect()
    })
}

// Go: transpile_runner.go:27 TranspileBaselineRunner
// PORT: `is_submodule` is the layout: true at the typescript-go layout,
// where the cases and baselines are the submodule's.
pub struct TranspileBaselineRunner {
    is_submodule: bool,
    test_files: OnceLock<Vec<String>>,
    base_path: String,
}

// Go: transpile_runner.go:34 NewTranspileBaselineRunner
pub fn new_transpile_baseline_runner() -> TranspileBaselineRunner {
    let is_submodule = !baseline::is_merged_layout();
    TranspileBaselineRunner {
        is_submodule,
        test_files: OnceLock::new(),
        base_path: if is_submodule {
            "../_submodules/TypeScript/tests/cases/transpile"
        } else {
            "../testdata/tests/cases/transpile"
        }
        .to_string(),
    }
}

impl TranspileBaselineRunner {
    // Go: transpile_runner.go:40 EnumerateTestFiles
    pub fn enumerate_test_files(&self) -> &[String] {
        self.test_files.get_or_init(|| {
            enumerate_files(&self.base_path, go_regex::has_transpile_test_suffix, true)
                .unwrap_or_else(|err| panic!("Could not read transpile test files: {err}"))
        })
    }
}

/// Go `osvfs.FS().ReadFile(fileName)` of `runTest`.
fn read_transpile_test_file(file_name: &str) -> String {
    let (content, ok) = ts_goport::frontend::vfs::osvfs_fs().read_file(file_name);
    if !ok {
        panic!("Could not read transpile test file: {file_name}");
    }
    content
}

// Go: transpile_runner.go:58 runTest (the configurations)
fn get_transpile_configurations(content: &str) -> Vec<NamedTestConfiguration> {
    let settings = extract_compiler_settings(content);
    let configurations = get_file_based_test_configurations(&settings, transpile_vary_by());
    if configurations.is_empty() {
        return vec![NamedTestConfiguration {
            name: String::new(),
            config: settings,
        }];
    }
    configurations
}

/// Go `justName` of `runTest`: the base name without its extension, and the
/// extension.
fn split_test_name(file_name: &str) -> (String, String) {
    let extension = get_any_extension_from_path(file_name, &[], false);
    let base_name = get_base_file_name(file_name);
    let just_name = base_name
        .strip_suffix(extension.as_str())
        .unwrap_or(&base_name)
        .to_string();
    (just_name, extension)
}

// Go: transpile_runner.go:79 configuredName
fn transpile_configured_name(just_name: &str, configuration_name: &str) -> String {
    if configuration_name.is_empty() {
        return just_name.to_string();
    }
    format!(
        "{just_name}({})",
        format_transpile_configuration_name(configuration_name)
    )
}

// Go: transpile_runner.go:94 formatTranspileConfigurationName
fn format_transpile_configuration_name(name: &str) -> String {
    let name = name.replace("declarationmap=", "declarationMap=");
    let name = name.replace("inlinesourcemap=", "inlineSourceMap=");
    name.replace("sourcemap=", "sourceMap=")
}

// Go: transpile_runner.go:52 RunTests (the subtest names)
// PORT: returns the cases; child.rs runs them. A test file whose
// configurations cannot be computed is an `Err` with the Go failure.
fn enumerate_config_cases(runner: &TranspileBaselineRunner) -> (Vec<ConfigCase>, Vec<String>) {
    let mut cases = Vec::new();
    let mut errors = Vec::new();
    for (i, file_name) in runner.enumerate_test_files().iter().enumerate() {
        let (just_name, _) = split_test_name(file_name);
        let configurations = match catch_unwind(AssertUnwindSafe(|| {
            get_transpile_configurations(&read_transpile_test_file(file_name))
        })) {
            Ok(configurations) => configurations,
            Err(payload) => {
                errors.push(format!(
                    "{TRANSPILE_SUITE}/{just_name}: {}",
                    payload_text(payload.as_ref())
                ));
                continue;
            }
        };
        for configuration in configurations {
            cases.push(ConfigCase {
                is_submodule: runner.is_submodule,
                suite: TRANSPILE_SUITE,
                filename: file_name.clone(),
                test_name: transpile_configured_name(&just_name, &configuration.name),
                configuration: Some(configuration.name),
                file_index: i,
            });
        }
    }
    (cases, errors)
}

// Go: transpile_runner.go:58 runTest (one configuration: the body of
// `t.Run(configuredName, ...)`)
// PORT: the child side of one case. Each `runKind` reports through
// `report` (kinds `js` and `dts`).
pub fn run_single_config_test(case: &ConfigCase, report: Report<'_>) {
    let content = read_transpile_test_file(&case.filename);
    let configurations = get_transpile_configurations(&content);
    let name = case.configuration.as_deref().unwrap_or("");
    let Some(configuration) = configurations.iter().find(|config| config.name == name) else {
        report(
            "options",
            Outcome::Fail(format!("configuration {name} not found")),
        );
        return;
    };

    let (just_name, extension) = split_test_name(&case.filename);
    let base_name = get_base_file_name(&case.filename);
    let units = make_units_from_test(&content, &base_name).test_unit_data;
    let configured_name = transpile_configured_name(&just_name, &configuration.name);

    let mut options = CompilerOptions::default();
    let mut harness_options = HarnessOptions::default();
    let set_options = catch_unwind(AssertUnwindSafe(|| {
        set_options_from_test_config(
            &configuration.config,
            &mut options,
            &mut harness_options,
            SRC_FOLDER,
            false,
        );
    }));
    if let Err(payload) = set_options {
        report("options", Outcome::Fail(payload_text(payload.as_ref())));
        return;
    }

    let message = format!("Panic on transpiling test {}", case.filename);
    if !options.emit_declaration_only.is_true() {
        run_subtest(report, "js", &message, || {
            run_kind(
                &configured_name,
                &extension,
                &units,
                &options,
                &harness_options,
                false,
                case.is_submodule,
            )
        });
    }
    if options.declaration.is_true() {
        run_subtest(report, "dts", &message, || {
            run_kind(
                &configured_name,
                &extension,
                &units,
                &options,
                &harness_options,
                true,
                case.is_submodule,
            )
        });
    }
}

// Go: transpile_runner.go:100 runKind
// PORT: Go reports through `t`; this returns the messages of a failed
// comparison or check (Go `t.Fatal` and `t.Errorf`). `is_submodule` is the
// runner's (the layout).
fn run_kind(
    configured_name: &str,
    extension: &str,
    units: &[TestUnit],
    options: &CompilerOptions,
    harness_options: &HarnessOptions,
    declaration: bool,
    is_submodule: bool,
) -> Result<(), String> {
    let mut result = String::new();
    for unit in units {
        append_transpile_section(&mut result, &unit.name, &unit.content);
    }

    // Go `t.Context()`: the test does not cancel it.
    let ctx = context::background();
    // Go `assert.Check` failures of `GetErrorBaseline`.
    let mut checks = Vec::new();
    for unit in units {
        let transpile_options = transpile::Options {
            compiler_options: Some(options.clone()),
            file_name: unit.name.clone(),
            report_diagnostics: harness_options.report_diagnostics,
        };
        let output = if declaration {
            transpile::transpile_declaration(&ctx, &unit.content, transpile_options)
        } else {
            transpile::transpile_module(&ctx, &unit.content, transpile_options)
        };
        let Some(output) = output else {
            return Err("transpilation was canceled".to_string());
        };

        let output_extension = if declaration {
            get_declaration_emit_extension_for_path(&unit.name)
        } else {
            get_output_extension(&unit.name, options.jsx).to_string()
        };
        let output_file_name = change_extension(&unit.name, &output_extension);
        append_transpile_section(&mut result, &output_file_name, &output.output_text);
        if !output.source_map_text.is_empty() {
            append_transpile_section(
                &mut result,
                &format!("{output_file_name}.map"),
                &output.source_map_text,
            );
        }
        if !output.diagnostics.is_empty() {
            result.push_str("\r\n\r\n//// [Diagnostics reported]\r\n");
            let diagnostic_file_name = tsbaseline::diagnostic_file_name(&output.diagnostics[0])
                .unwrap_or_else(|| unit.name.clone());
            let error_baseline = tsbaseline::get_error_baseline(
                &mut checks,
                &[TestFile {
                    unit_name: diagnostic_file_name.clone(),
                    content: unit.content.clone(),
                }],
                &output.diagnostics,
                options.pretty.is_true(),
            );
            result.push_str(&error_baseline.replace(&diagnostic_file_name, &unit.name));
            if !result.ends_with('\n') {
                result.push_str("\r\n");
            }
        }
    }

    let baseline_extension = if declaration {
        get_declaration_emit_extension_for_path(&format!("{configured_name}{extension}"))
    } else {
        get_output_extension(&format!("{configured_name}{extension}"), options.jsx).to_string()
    };
    let baseline_name = format!("{configured_name}{baseline_extension}");
    let compared = baseline::run(
        &format!("transpile/{baseline_name}"),
        &result,
        &Options {
            is_submodule,
            ..Options::default()
        },
    );
    tsbaseline::finish_checks(compared, checks)
}

// Go: transpile_runner.go:167 appendTranspileSection
fn append_transpile_section(result: &mut String, file_name: &str, content: &str) {
    result.push_str(&format!("//// [{file_name}] ////\r\n"));
    result.push_str(content);
    if !content.ends_with('\n') {
        result.push_str("\r\n");
    }
}

// Go: transpile_runner.go:175 cleanTranspileBaselines
// PORT: Go always writes local baselines to `testdata/baselines/local`;
// the port writes them only under `baseline::local_root()` (see
// support/baseline.rs), so this cleans there, and nothing when it is off.
// The merged layout has one folder: `transpile`.
fn clean_transpile_baselines() {
    let Some(local_root) = baseline::local_root() else {
        return;
    };
    let dirs: Vec<std::path::PathBuf> = if baseline::is_merged_layout() {
        vec![local_root.join("transpile")]
    } else {
        ["submodule", "submoduleAccepted", "submoduleTriaged"]
            .iter()
            .map(|folder| local_root.join(folder).join("transpile"))
            .collect()
    };
    for dir in dirs {
        match std::fs::remove_dir_all(&dir) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => panic!("Could not clean up transpile baselines: {err}"),
        }
    }
}

// Go: transpile_runner.go:181 RunTranspileTests
// The merged layout has no submodule and no skip.
pub fn run_transpile_tests() {
    if !baseline::is_merged_layout()
        && crate::tsoptions::tsoptionstest::skip_if_no_type_script_submodule("TestTranspile")
    {
        return;
    }
    clean_transpile_baselines();
    let runner = new_transpile_baseline_runner();
    let (cases, errors) = enumerate_config_cases(&runner);
    child::run_cases("transpile runner", TRANSPILE_ENV_PREFIX, cases, errors);
}
