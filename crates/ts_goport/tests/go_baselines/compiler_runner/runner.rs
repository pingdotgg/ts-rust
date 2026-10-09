//! Go: internal/testrunner/compiler_runner.go, compiler_runner_test.go and
//! runner.go.
//!
//! PORT: Go runs each test configuration as a parallel subtest in one
//! process. Here each configuration runs in a child process of its own
//! (child.rs), because a program and the OS override are process state.
//! `run_single_config_test` is the child side: it runs the Go subtests in
//! Go order and reports each one (`Report`).

use std::collections::{BTreeMap, HashSet};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::OnceLock;

use ts_goport::baseline::type_symbol::TestFile;
use ts_goport::frontend::prelude::*;
use ts_goport::program as tsprogram;

use super::go_regex;
use super::harness::{
    CompilationResult, HarnessOptions, NamedTestConfiguration, SkipPayload, TestConfiguration,
    UnsupportedCompilerOptions, compile_files, enumerate_files, get_file_based_test_configurations,
    set_options_from_test_config, skip_unsupported_compiler_options,
};
use super::test_case_parser::{
    RawCompilerSettings, TestCaseContent, TestUnit, extract_compiler_settings, make_units_from_test,
};
use super::tsbaseline;
use crate::support::baseline::{self, Options};

// Go: compiler_runner.go:33 srcFolder
// Posix-style path to sources under test
pub const SRC_FOLDER: &str = "/.src";

// Go: compiler_runner.go:30 requireStr
const REQUIRE_STR: &str = "require(";

// Go: compiler_runner.go:35 CompilerTestType
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompilerTestType {
    Conformance,
    Regression,
}

impl CompilerTestType {
    // Go: compiler_runner.go:42 String
    pub fn string(self) -> &'static str {
        if self == CompilerTestType::Regression {
            return "compiler";
        }
        "conformance"
    }
}

// Go: compiler_runner.go:49 CompilerBaselineRunner
pub struct CompilerBaselineRunner {
    pub is_submodule: bool,
    test_files: OnceLock<Vec<String>>,
    base_path: String,
    pub test_suit_name: &'static str,
}

// Go: compiler_runner.go:58 NewCompilerBaselineRunner
pub fn new_compiler_baseline_runner(
    test_type: CompilerTestType,
    is_submodule: bool,
) -> CompilerBaselineRunner {
    let test_suit_name = test_type.string();
    let base_path = if is_submodule {
        format!("../_submodules/TypeScript/tests/cases/{test_suit_name}")
    } else {
        format!("tests/cases/{test_suit_name}")
    };
    CompilerBaselineRunner {
        base_path,
        test_suit_name,
        is_submodule,
        test_files: OnceLock::new(),
    }
}

impl CompilerBaselineRunner {
    // Go: compiler_runner.go:73 EnumerateTestFiles
    pub fn enumerate_test_files(&self) -> &[String] {
        self.test_files.get_or_init(|| {
            enumerate_files(
                &self.base_path,
                go_regex::has_ts_or_tsx_suffix,
                true, /*recursive*/
            )
            .unwrap_or_else(|err| panic!("Could not read compiler test files: {err}"))
        })
    }
}

// Go: compiler_runner.go:78 skippedTests
pub const SKIPPED_TESTS: &[&str] = &[
    // Tests that depended on typescript.d.ts in built.
    "APILibCheck.ts",
    "APISample_Watch.ts",
    "APISample_WatchWithDefaults.ts",
    "APISample_WatchWithOwnWatchHost.ts",
    "APISample_compile.ts",
    "APISample_jsdoc.ts",
    "APISample_linter.ts",
    "APISample_parseConfig.ts",
    "APISample_transform.ts",
    "APISample_watcher.ts",
    // These tests contain options that have been completely removed, so fail to parse.
    "preserveUnusedImports.ts",
    "noCrashWithVerbatimModuleSyntaxAndImportsNotUsedAsValues.ts",
    "verbatimModuleSyntaxCompat.ts",
    "verbatimModuleSyntaxCompat2.ts",
    "verbatimModuleSyntaxCompat3.ts",
    "verbatimModuleSyntaxCompat4.ts",
    "preserveValueImports.ts",
    "preserveValueImports_importsNotUsedAsValues.ts",
    "preserveValueImports_errors.ts",
    "preserveValueImports_mixedImports.ts",
    "preserveValueImports_module.ts",
    "importsNotUsedAsValues_error.ts",
    "alwaysStrictNoImplicitUseStrict.ts",
    "nonPrimitiveIndexingWithForInSupressError.ts",
    "parameterInitializerBeforeDestructuringEmit.ts",
    "mappedTypeUnionConstraintInferences.ts",
    "lateBoundConstraintTypeChecksCorrectly.ts",
    "keyofDoesntContainSymbols.ts",
    "noStrictGenericChecks.ts",
    "noImplicitUseStrict_umd.ts",
    "noImplicitUseStrict_system.ts",
    "noImplicitUseStrict_es6.ts",
    "noImplicitUseStrict_commonjs.ts",
    "noImplicitAnyIndexingSuppressed.ts",
    "excessPropertyErrorsSuppressed.ts",
    "moduleNoneDynamicImport.ts",
    "moduleNoneErrors.ts",
    "noErrorUsingImportExportModuleAugmentationInDeclarationFile1.ts",
    "noErrorUsingImportExportModuleAugmentationInDeclarationFile2.ts",
    "noErrorUsingImportExportModuleAugmentationInDeclarationFile3.ts",
    "requireOfJsonFileWithModuleEmitNone.ts",
    "requireOfJsonFileWithModuleNodeResolutionEmitNone.ts",
];

// Go: testrunner/options_generated.go:7 compilerVaryBy (ts#64457)
// The compiler options for which a test file can give variations, for
// instance `// @strict: true, false`. Go generates the list
// (tools/scripts/tsc/generate-options.ts:871): every compiler option that is
// not command-line only and is a boolean or an enum, lowercased and sorted.
// N computed a smaller set from the `Affects*` flags (compiler_runner.go:150
// at 673a5f17d713; removed by ts#64457). No test case of N' gives two values
// for an option of the difference, so no configuration name changes.
pub fn compiler_vary_by() -> &'static HashSet<String> {
    const COMPILER_VARY_BY: &[&str] = &[
        "all",
        "allowarbitraryextensions",
        "allowimportingtsextensions",
        "allowjs",
        "allowsyntheticdefaultimports",
        "allowumdglobalaccess",
        "allowunreachablecode",
        "allowunusedlabels",
        "alwaysstrict",
        "assumechangesonlyaffectdirectdependencies",
        "checkjs",
        "composite",
        "declaration",
        "declarationmap",
        "deduplicatepackages",
        "diagnostics",
        "disablereferencedprojectload",
        "disablesizelimit",
        "disablesolutionsearching",
        "disablesourceofprojectreferenceredirect",
        "downleveliteration",
        "emitbom",
        "emitdeclarationonly",
        "emitdecoratormetadata",
        "erasablesyntaxonly",
        "esmoduleinterop",
        "exactoptionalpropertytypes",
        "experimentaldecorators",
        "explainfiles",
        "extendeddiagnostics",
        "forceconsistentcasinginfilenames",
        "importhelpers",
        "incremental",
        "init",
        "inlinesourcemap",
        "inlinesources",
        "isolateddeclarations",
        "isolatedmodules",
        "jsx",
        "libreplacement",
        "listemittedfiles",
        "listfiles",
        "module",
        "moduledetection",
        "moduleresolution",
        "newline",
        "nocheck",
        "noemit",
        "noemithelpers",
        "noemitonerror",
        "noerrortruncation",
        "nofallthroughcasesinswitch",
        "noimplicitany",
        "noimplicitoverride",
        "noimplicitreturns",
        "noimplicitthis",
        "nolib",
        "nopropertyaccessfromindexsignature",
        "noresolve",
        "nouncheckedindexedaccess",
        "nouncheckedsideeffectimports",
        "nounusedlocals",
        "nounusedparameters",
        "preserveconstenums",
        "preservesymlinks",
        "preservewatchoutput",
        "pretty",
        "quiet",
        "removecomments",
        "resolvejsonmodule",
        "resolvepackagejsonexports",
        "resolvepackagejsonimports",
        "rewriterelativeimportextensions",
        "singlethreaded",
        "skipdefaultlibcheck",
        "skiplibcheck",
        "sourcemap",
        "stabletypeordering",
        "strict",
        "strictbindcallapply",
        "strictbuiltiniteratorreturn",
        "strictfunctiontypes",
        "strictnullchecks",
        "strictpropertyinitialization",
        "stripinternal",
        "target",
        "traceresolution",
        "usedefineforclassfields",
        "useunknownincatchvariables",
        "verbatimmodulesyntax",
        "version",
    ];
    static SET: OnceLock<HashSet<String>> = OnceLock::new();
    SET.get_or_init(|| COMPILER_VARY_BY.iter().map(ToString::to_string).collect())
}

// Go: compiler_runner.go:210 compilerFileBasedTest
pub struct CompilerFileBasedTest {
    pub filename: String,
    pub content: String,
    pub configurations: Vec<NamedTestConfiguration>,
}

/// Go `osvfs.FS().ReadFile(filename)`: the port form text, with a UTF-8
/// byte order mark removed and UTF-16 decoded. Call it before the OS
/// override is installed.
pub fn read_test_file(filename: &str) -> String {
    let (content, ok) = ts_goport::frontend::vfs::osvfs_fs().read_file(filename);
    if !ok {
        panic!("Could not read test file: {filename}");
    }
    content
}

// Go: compiler_runner.go:216 getCompilerFileBasedTest
pub fn get_compiler_file_based_test(filename: &str) -> CompilerFileBasedTest {
    let content = read_test_file(filename);
    let settings: RawCompilerSettings = extract_compiler_settings(&content);
    let configurations = get_file_based_test_configurations(&settings, compiler_vary_by());
    CompilerFileBasedTest {
        filename: filename.to_string(),
        content,
        configurations,
    }
}

// Go: compiler_runner.go:229 compilerTest
pub struct CompilerTest {
    pub test_name: String,
    pub filename: String,
    pub basename: String,
    pub configured_name: String, // name with configuration description, e.g. `file`
    pub current_directory: String,
    pub options: CompilerOptions,
    pub harness_options: HarnessOptions,
    pub result: CompilationResult,
    pub ts_config_files: Vec<TestFile>,
    pub to_be_compiled: Vec<TestFile>, // equivalent to the files that will be passed on the command line
    pub other_files: Vec<TestFile>, // equivalent to other files on the file system not directly passed to the compiler (ie things that are referenced by other files)
    pub has_non_dts_files: bool,
}

/// The configured name of a test: `name(config).ext` (Go
/// `newCompilerTest`).
pub fn configured_name(filename: &str, configuration_name: Option<&str>) -> String {
    let basename = get_base_file_name(filename);
    match configuration_name {
        Some(name) if !name.is_empty() => {
            let extname = get_any_extension_from_path(&basename, &[], false);
            let extensionless_basename = &basename[..basename.len() - extname.len()];
            format!("{extensionless_basename}({name}){extname}")
        }
        _ => basename,
    }
}

/// The inputs of `newCompilerTest` before `CompileFiles`: the files, the
/// harness configuration and the current directory.
pub struct CompilerTestInputs {
    pub harness_config: Option<TestConfiguration>,
    pub current_directory: String,
    pub ts_config: Option<std::rc::Rc<ParsedCommandLine>>,
    pub ts_config_files: Vec<TestFile>,
    pub to_be_compiled: Vec<TestFile>,
    pub other_files: Vec<TestFile>,
    pub has_non_dts_files: bool,
    pub symlinks: BTreeMap<String, String>,
}

// Go: compiler_runner.go:248 newCompilerTest (the part before CompileFiles)
pub fn new_compiler_test_inputs(
    test_content: TestCaseContent,
    named_configuration: Option<&NamedTestConfiguration>,
) -> CompilerTestInputs {
    let mut harness_config: Option<TestConfiguration> =
        named_configuration.map(|named| named.config.clone());
    let current_directory = get_normalized_absolute_path(
        harness_config
            .as_ref()
            .and_then(|config| config.get("currentdirectory"))
            .map_or("", String::as_str),
        SRC_FOLDER,
    );

    let units = &test_content.test_unit_data;
    let mut to_be_compiled = Vec::new();
    let mut other_files = Vec::new();
    let has_non_dts_files = units
        .iter()
        .any(|unit| !file_extension_is(&unit.name, EXTENSION_DTS));
    let mut ts_config_files = Vec::new();
    if let Some(ts_config) = &test_content.ts_config {
        ts_config_files.push(create_harness_test_file(
            test_content
                .ts_config_file_unit_data
                .as_ref()
                .expect("a tsconfig unit"),
            &current_directory,
        ));
        for unit in units {
            if ts_config
                .parsed_config
                .file_names
                .contains(&get_normalized_absolute_path(
                    &unit.name,
                    &current_directory,
                ))
            {
                to_be_compiled.push(create_harness_test_file(unit, &current_directory));
            } else {
                other_files.push(create_harness_test_file(unit, &current_directory));
            }
        }
    } else {
        if let Some(config) = harness_config.as_mut()
            && let Some(base_url) = config.get("baseurl").cloned()
            && !is_rooted_disk_path(&base_url)
        {
            config.insert(
                "baseurl".to_string(),
                get_normalized_absolute_path(&base_url, &current_directory),
            );
        }

        let last_unit = units.last().expect("a test has a unit");
        // We need to assemble the list of input files for the compiler and other related files on the 'filesystem' (ie in a multi-file test)
        // If the last file in a test uses require or a triple slash reference we'll assume all other files will be brought in via references,
        // otherwise, assume all files are just meant to be in the same compilation session without explicit references to one another.

        if harness_config
            .as_ref()
            .and_then(|config| config.get("noimplicitreferences"))
            .is_some_and(|value| !value.is_empty())
            || last_unit.content.contains(REQUIRE_STR)
            || go_regex::contains_reference_path(&last_unit.content)
        {
            to_be_compiled.push(create_harness_test_file(last_unit, &current_directory));
            for unit in &units[..units.len() - 1] {
                other_files.push(create_harness_test_file(unit, &current_directory));
            }
        } else {
            to_be_compiled = units
                .iter()
                .map(|unit| create_harness_test_file(unit, &current_directory))
                .collect();
        }
    }

    CompilerTestInputs {
        harness_config,
        current_directory,
        ts_config: test_content.ts_config.clone(),
        ts_config_files,
        to_be_compiled,
        other_files,
        has_non_dts_files,
        symlinks: test_content.symlinks,
    }
}

/// The compiler options that `CompileFiles` gives the program, without
/// compiling. `SkipUnsupportedCompilerOptions` reads only options that the
/// program keeps as given, so a skipped configuration is known before its
/// compilation.
pub fn precompute_compiler_options(inputs: &CompilerTestInputs) -> CompilerOptions {
    let mut compiler_options = match &inputs.ts_config {
        Some(tsconfig) => (*tsconfig.parsed_config.compiler_options).clone(),
        None => CompilerOptions::default(),
    };
    if compiler_options.new_line == NewLineKind::NONE {
        compiler_options.new_line = NewLineKind::CRLF;
    }
    if compiler_options.skip_default_lib_check == Tristate::Unknown {
        compiler_options.skip_default_lib_check = Tristate::True;
    }
    compiler_options.no_error_truncation = Tristate::True;
    let mut harness_options = HarnessOptions::default();
    if let Some(config) = &inputs.harness_config {
        set_options_from_test_config(
            config,
            &mut compiler_options,
            &mut harness_options,
            &inputs.current_directory,
            false,
        );
    }
    compiler_options
}

// Go: compiler_runner.go:248 newCompilerTest (CompileFiles and the result)
pub fn new_compiler_test(
    test_name: &str,
    filename: &str,
    inputs: CompilerTestInputs,
    named_configuration: Option<&NamedTestConfiguration>,
) -> CompilerTest {
    let basename = get_base_file_name(filename);
    let configured_name = configured_name(
        filename,
        named_configuration.map(|named| named.name.as_str()),
    );

    let mut result = compile_files(
        &inputs.to_be_compiled,
        &inputs.other_files,
        inputs.harness_config.as_ref(),
        inputs.ts_config.as_deref(),
        &inputs.current_directory,
        &inputs.symlinks,
    );

    // Go: compiler_runner.go:350 (tsgo#4712)
    // Content-mapped files are transformed during program construction; the transformed text is what the
    // compiler actually parses and reports positions against. Baseline that text (rather than the original
    // foreign source) so the type, symbol, and error baselines line up with the compiler's positions.
    let mut to_be_compiled = inputs.to_be_compiled;
    let mut other_files = inputs.other_files;
    let mut changed = false;
    for file in to_be_compiled.iter_mut().chain(other_files.iter_mut()) {
        if result
            .source_file_content_mapper(&file.unit_name)
            .is_some_and(|content_mapper| !content_mapper.is_empty())
        {
            file.content = result
                .source_file_text(&file.unit_name)
                .expect("the program has the file");
            changed = true;
        }
    }
    // PORT: Go changes the shared `*TestFile` values; `Repeat` sees them.
    if changed {
        result.set_repeat_files(&to_be_compiled, &other_files);
    }

    CompilerTest {
        test_name: test_name.to_string(),
        filename: filename.to_string(),
        basename,
        configured_name,
        current_directory: inputs.current_directory,
        options: result.options.clone(),
        harness_options: result.harness_options.clone(),
        result,
        ts_config_files: inputs.ts_config_files,
        to_be_compiled,
        other_files,
        has_non_dts_files: inputs.has_non_dts_files,
    }
}

// Go: compiler_runner.go:522 createHarnessTestFile
fn create_harness_test_file(unit: &TestUnit, current_directory: &str) -> TestFile {
    TestFile {
        unit_name: get_normalized_absolute_path(&unit.name, current_directory),
        content: unit.content.clone(),
    }
}

// ---------------------------------------------------------------------------
// Subtests (child side)
// ---------------------------------------------------------------------------

/// The outcome of one Go subtest.
pub enum Outcome {
    Pass,
    Fail(String),
    Skip(String),
}

/// Receives each subtest outcome as it ends (child.rs writes it out).
pub type Report<'a> = &'a mut dyn FnMut(&str, Outcome);

/// The text of a panic payload.
pub fn payload_text(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_string()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else if let Some(skip) = payload.downcast_ref::<SkipPayload>() {
        skip.0.clone()
    } else if payload.is::<GoPanic>() {
        "Go panic".to_string()
    } else {
        "panic".to_string()
    }
}

/// Go `t.Run(name, ...)` with `defer testutil.RecoverAndFail(t, message)`:
/// a panic fails the subtest, `skip` skips it.
pub fn run_subtest(
    report: Report<'_>,
    kind: &str,
    message: &str,
    f: impl FnOnce() -> Result<(), String>,
) {
    let outcome = match catch_unwind(AssertUnwindSafe(f)) {
        Ok(Ok(())) => Outcome::Pass,
        Ok(Err(err)) => Outcome::Fail(err),
        Err(payload) => {
            if let Some(skip) = payload.downcast_ref::<SkipPayload>() {
                Outcome::Skip(skip.0.clone())
            } else {
                Outcome::Fail(format!("{message}:\n{}", payload_text(payload.as_ref())))
            }
        }
    };
    report(kind, outcome);
}

impl CompilerTest {
    fn header(&self, is_submodule: bool) -> String {
        let test_data = baseline::test_data_path();
        let mut header_components = get_path_components_relative_to(
            &test_data.to_string_lossy(),
            &self.filename,
            &ComparePathsOptions::default(),
        );
        if is_submodule {
            header_components = header_components[4..].to_vec(); // Strip "./../_submodules/TypeScript" prefix
        }
        get_path_from_path_components(&header_components)
    }

    // Go: compiler_runner.go:341 verifyDiagnostics
    pub fn verify_diagnostics(&self, report: Report<'_>, suite_name: &str, is_submodule: bool) {
        run_subtest(
            report,
            "error",
            &format!(
                "Panic on creating error baseline for test {}",
                self.filename
            ),
            || {
                let _scope = self.result.enter();
                let mut files: Vec<TestFile> = self
                    .ts_config_files
                    .iter()
                    .chain(&self.to_be_compiled)
                    .chain(&self.other_files)
                    .cloned()
                    .collect();
                let mut diagnostics = self.result.diagnostics.clone();
                // tsgo#4712
                // Content-mapped files' diagnostics are baselined separately (see verifyContentMapper), where they can
                // be rendered against the correct text; the squiggle renderer here assumes a single coordinate space.
                let content_mapped = self.content_mapped_file_names();
                if !content_mapped.is_empty() {
                    files.retain(|f| {
                        !content_mapped.contains(&get_normalized_absolute_path(
                            &f.unit_name,
                            &self.current_directory,
                        ))
                    });
                    diagnostics.retain(|d| {
                        tsbaseline::diagnostic_file_name(d)
                            .is_none_or(|name| !content_mapped.contains(&name))
                    });
                }
                tsbaseline::do_error_baseline(
                    &self.configured_name,
                    &files,
                    &diagnostics,
                    self.result.options.pretty.is_true(),
                    &Options {
                        subfolder: suite_name.to_string(),
                        is_submodule,
                        diff_fixup_old: Some(std::sync::Arc::new(tsbaseline::error_diff_fixup_old)),
                        ..Options::default()
                    },
                )
            },
        );
    }

    // Go: compiler_runner.go:416 verifyContentMapper
    pub fn verify_content_mapper(&self, report: Report<'_>, suite_name: &str, is_submodule: bool) {
        run_subtest(
            report,
            "contentmapper",
            &format!(
                "Panic on creating content mapper baseline for test {}",
                self.filename
            ),
            || {
                let _scope = self.result.enter();
                tsbaseline::do_content_mapper_baseline(
                    &self.configured_name,
                    &self.result,
                    &self.result.diagnostics,
                    &Options {
                        subfolder: suite_name.to_string(),
                        is_submodule,
                        ..Options::default()
                    },
                )
            },
        );
    }

    // Go: compiler_runner.go:427 contentMappedFileNames
    // contentMappedFileNames returns the set of absolute file names that were produced by a content mapper.
    // PORT: Go returns a nil map when there are none; that is an empty set.
    fn content_mapped_file_names(&self) -> HashSet<String> {
        self.result
            .content_mapped_source_files()
            .into_iter()
            .map(|(file, _)| source_file_file_name(file).to_string())
            .collect()
    }

    // Go: compiler_runner.go:373 skippedEmitTests
    fn skipped_emit_test(&self) -> Option<&'static str> {
        match self.basename.as_str() {
            "filesEmittingIntoSameOutput.ts" => Some(
                "Output order nondeterministic due to collision on filename during parallel emit.",
            ),
            "jsFileCompilationWithJsEmitPathSameAsInput.ts" => Some(
                "Output order nondeterministic due to collision on filename during parallel emit.",
            ),
            "grammarErrors.ts" => Some(
                "Output order nondeterministic due to collision on filename during parallel emit.",
            ),
            "jsFileCompilationEmitBlockedCorrectly.ts" => Some(
                "Output order nondeterministic due to collision on filename during parallel emit.",
            ),
            "jsDeclarationsReexportAliasesEsModuleInterop.ts" => {
                Some("cls.d.ts is missing statements when run concurrently.")
            }
            "jsFileCompilationWithoutJsExtensions.ts" => Some("No files are emitted."),
            "typeOnlyMerge2.ts" => Some("Nondeterministic contents when run concurrently."),
            "typeOnlyMerge3.ts" => Some("Nondeterministic contents when run concurrently."),
            _ => None,
        }
    }

    // Go: compiler_runner.go:384 verifyJavaScriptOutput
    pub fn verify_java_script_output(
        &self,
        report: Report<'_>,
        suite_name: &str,
        is_submodule: bool,
    ) {
        if !self.has_non_dts_files {
            return;
        }

        if let Some(message) = self.skipped_emit_test() {
            report("output", Outcome::Skip(message.to_string()));
            return;
        }
        run_subtest(
            report,
            "output",
            &format!("Panic on creating js output for test {}", self.filename),
            || {
                let _scope = self.result.enter();
                let header = self.header(is_submodule);
                tsbaseline::do_js_emit_baseline(
                    &self.configured_name,
                    &header,
                    &self.options,
                    &self.result,
                    &self.ts_config_files,
                    &self.to_be_compiled,
                    &self.other_files,
                    &self.harness_options,
                    &Options {
                        subfolder: suite_name.to_string(),
                        is_submodule,
                        ..Options::default()
                    },
                )
            },
        );
    }

    // Go: compiler_runner.go:415 verifySourceMapOutput
    pub fn verify_source_map_output(
        &self,
        report: Report<'_>,
        suite_name: &str,
        is_submodule: bool,
    ) {
        run_subtest(
            report,
            "sourcemap",
            &format!(
                "Panic on creating source map output for test {}",
                self.filename
            ),
            || {
                let _scope = self.result.enter();
                let header = self.header(is_submodule);
                tsbaseline::do_sourcemap_baseline(
                    &self.configured_name,
                    &header,
                    &self.options,
                    &self.result,
                    &self.harness_options,
                    &Options {
                        subfolder: suite_name.to_string(),
                        is_submodule,
                        ..Options::default()
                    },
                )
            },
        );
    }

    // Go: compiler_runner.go:436 verifySourceMapRecord
    pub fn verify_source_map_record(
        &self,
        report: Report<'_>,
        suite_name: &str,
        is_submodule: bool,
    ) {
        run_subtest(
            report,
            "sourcemaprecord",
            &format!(
                "Panic on creating source map record for test {}",
                self.filename
            ),
            || {
                let _scope = self.result.enter();
                let header = self.header(is_submodule);
                tsbaseline::do_sourcemap_record_baseline(
                    &self.configured_name,
                    &header,
                    &self.options,
                    &self.result,
                    &self.harness_options,
                    &Options {
                        subfolder: suite_name.to_string(),
                        is_submodule,
                        ..Options::default()
                    },
                )
            },
        );
    }

    // Go: compiler_runner.go:457 verifyTypesAndSymbols
    pub fn verify_types_and_symbols(
        &self,
        report: Report<'_>,
        suite_name: &str,
        is_submodule: bool,
    ) {
        let no_types_and_symbols = self.harness_options.no_types_and_symbols;
        if no_types_and_symbols {
            return;
        }
        let _scope = self.result.enter();
        let all_files: Vec<TestFile> = self
            .to_be_compiled
            .iter()
            .chain(&self.other_files)
            .filter(|f| tsprogram::get_source_file(&f.unit_name).is_some())
            .cloned()
            .collect();

        let header = self.header(is_submodule);
        let results = tsbaseline::do_type_and_symbol_baseline(
            &self.configured_name,
            &header,
            &all_files,
            &Options {
                subfolder: suite_name.to_string(),
                is_submodule,
                ..Options::default()
            },
            !self.result.diagnostics.is_empty(),
            |message, f| match catch_unwind(AssertUnwindSafe(f)) {
                Ok(result) => result,
                Err(payload) => Err(format!("{message}:\n{}", payload_text(payload.as_ref()))),
            },
        );
        for (kind, result) in [("types", results.types), ("symbols", results.symbols)] {
            report(
                kind,
                match result {
                    Ok(()) => Outcome::Pass,
                    Err(err) => Outcome::Fail(err),
                },
            );
        }
    }

    // Go: compiler_runner.go:494 verifyModuleResolution
    pub fn verify_module_resolution(
        &self,
        report: Report<'_>,
        suite_name: &str,
        is_submodule: bool,
    ) {
        if !self.options.trace_resolution.is_true() {
            return;
        }

        run_subtest(
            report,
            "moduleresolution",
            &format!(
                "Panic on creating module resolution baseline for test {}",
                self.filename
            ),
            || {
                tsbaseline::do_module_resolution_baseline(
                    &self.configured_name,
                    &self.result.trace,
                    &Options {
                        subfolder: suite_name.to_string(),
                        is_submodule,
                        skip_diff_with_old: true,
                        ..Options::default()
                    },
                )
            },
        );
    }

    // Go: compiler_runner.go:529 verifyUnionOrdering
    pub fn verify_union_ordering(&self, report: Report<'_>) {
        run_subtest(report, "unionordering", "Panic on union ordering", || {
            let _scope = self.result.enter();
            let failures: Vec<String> =
                tsprogram::for_each_checker_parallel(|_, checker| check_union_ordering(checker))
                    .into_iter()
                    .flatten()
                    .collect();
            if failures.is_empty() {
                Ok(())
            } else {
                Err(failures.join("\n"))
            }
        });
    }

    // Go: compiler_runner.go:554 verifyParentPointers
    pub fn verify_parent_pointers(&self, report: Report<'_>) {
        run_subtest(
            report,
            "parentpointers",
            "Panic on source file parent pointers",
            || {
                let _scope = self.result.enter();
                for f in tsprogram::source_files() {
                    if tsprogram::is_source_file_default_library(&source_file_info(f).path) {
                        continue;
                    }
                    let mut parent = f;
                    verify_parent_pointers_of(f, &mut parent)?;
                }
                Ok(())
            },
        );
    }
}

/// The children of `node` under the Go `verifier` of `verifyParentPointers`.
fn verify_parent_pointers_of(node: Node, parent: &mut Node) -> Result<(), String> {
    let mut failure: Option<String> = None;
    node.for_each_child(|n| {
        if failure.is_some() || n.is_nil() {
            return false;
        }
        if n.parent().is_nil() {
            failure = Some("parent node does not exist".to_string());
            return true;
        }
        if n.parent() != *parent {
            let elab = if !node_is_synthesized(n) {
                let file = get_source_file_of_node(n);
                source_file_text(file)
                    .get(n.pos() as usize..n.end() as usize)
                    .unwrap_or("")
                    .to_string()
            } else {
                "!synthetic! no text available".to_string()
            };
            failure = Some(format!(
                "parent node does not match traversed parent: {:?}: {elab}",
                n.kind()
            ));
            return true;
        }
        let old_parent = *parent;
        *parent = n;
        if let Err(err) = verify_parent_pointers_of(n, parent) {
            failure = Some(err);
        }
        *parent = old_parent;
        failure.is_some()
    });
    match failure {
        Some(err) => Err(err),
        None => Ok(()),
    }
}

/// The body of the Go `ForEachCheckerParallel` callback of
/// `verifyUnionOrdering`: the failures of one checker.
fn check_union_ordering(checker: &mut Checker) -> Vec<String> {
    let mut failures = Vec::new();
    for union in checker.union_types() {
        let types: Vec<TypeId> = checker.ty(union).types().to_vec();

        let mut reversed = types.clone();
        reversed.reverse();
        reversed.sort_by(|a, b| checker.compare_types(*a, *b).cmp(&0));
        if reversed != types {
            failures.push("compareTypes does not sort union types consistently".to_string());
            return failures;
        }

        let mut shuffled = types.clone();
        let mut rng = GoPcg::new(1234, 5678);

        for _ in 0..10 {
            rng.shuffle(&mut shuffled);
            shuffled.sort_by(|a, b| checker.compare_types(*a, *b).cmp(&0));
            if shuffled != types {
                failures.push("compareTypes does not sort union types consistently".to_string());
                return failures;
            }
        }
    }
    failures
}

/// Go `math/rand/v2` `rand.New(rand.NewPCG(seed1, seed2))` with `Shuffle`.
struct GoPcg {
    hi: u64,
    lo: u64,
}

impl GoPcg {
    fn new(seed1: u64, seed2: u64) -> GoPcg {
        GoPcg {
            hi: seed1,
            lo: seed2,
        }
    }

    // Go: math/rand/v2/pcg.go next
    fn next(&mut self) -> (u64, u64) {
        const MUL_HI: u64 = 2549297995355413924;
        const MUL_LO: u64 = 4865540595714422341;
        const INC_HI: u64 = 6364136223846793005;
        const INC_LO: u64 = 1442695040888963407;
        // state = state * mul + inc
        let product = u128::from(self.lo) * u128::from(MUL_LO);
        let mut hi = (product >> 64) as u64;
        let lo = product as u64;
        hi = hi
            .wrapping_add(self.hi.wrapping_mul(MUL_LO))
            .wrapping_add(self.lo.wrapping_mul(MUL_HI));
        let (lo, carry) = lo.overflowing_add(INC_LO);
        let hi = hi.wrapping_add(INC_HI).wrapping_add(u64::from(carry));
        self.lo = lo;
        self.hi = hi;
        (hi, lo)
    }

    // Go: math/rand/v2/pcg.go Uint64 (DXSM)
    fn uint64(&mut self) -> u64 {
        let (mut hi, lo) = self.next();
        const CHEAP_MUL: u64 = 0xda942042e4dd58b5;
        hi ^= hi >> 32;
        hi = hi.wrapping_mul(CHEAP_MUL);
        hi ^= hi >> (3 * 16);
        hi = hi.wrapping_mul(lo | 1);
        hi
    }

    // Go: math/rand/v2/rand.go uint64n
    fn uint64n(&mut self, n: u64) -> u64 {
        if n & (n - 1) == 0 {
            // n is power of two, can mask
            return self.uint64() & (n - 1);
        }
        let product = u128::from(self.uint64()) * u128::from(n);
        let (mut hi, mut lo) = ((product >> 64) as u64, product as u64);
        if lo < n {
            let thresh = n.wrapping_neg() % n;
            while lo < thresh {
                let product = u128::from(self.uint64()) * u128::from(n);
                hi = (product >> 64) as u64;
                lo = product as u64;
            }
        }
        hi
    }

    // Go: math/rand/v2/rand.go Shuffle
    fn shuffle<T>(&mut self, items: &mut [T]) {
        // Fisher-Yates shuffle
        let mut i = items.len();
        while i > 1 {
            i -= 1;
            let j = self.uint64n((i + 1) as u64) as usize;
            items.swap(i, j);
        }
    }
}

/// One test configuration of a runner, as Go `runTest` makes subtests.
#[derive(Clone, Debug)]
pub struct ConfigCase {
    pub is_submodule: bool,
    pub suite: &'static str,
    pub filename: String,
    /// `None` when the test has no configurations.
    pub configuration: Option<String>,
    /// Go `testName`: the base name, then a space and the configuration
    /// name when it has one.
    pub test_name: String,
    /// The index of the test file in the enumeration (both runners), for
    /// shards.
    pub file_index: usize,
}

impl ConfigCase {
    /// The key of the case in results and `known_failures.txt`:
    /// `<local|submodule>/<suite>/<configured name>`.
    pub fn key(&self) -> String {
        format!(
            "{}/{}/{}",
            if self.is_submodule {
                "submodule"
            } else {
                "local"
            },
            self.suite,
            configured_name(&self.filename, self.configuration.as_deref())
        )
    }
}

// Go: compiler_runner.go:190 runTest (the subtest names)
// PORT: returns the cases; child.rs runs them. A test file whose
// configurations cannot be computed is an `Err` with the Go failure.
pub fn enumerate_config_cases(
    runner: &CompilerBaselineRunner,
    file_index_start: usize,
) -> (Vec<ConfigCase>, Vec<String>) {
    let mut cases = Vec::new();
    let mut errors = Vec::new();
    for (i, filename) in runner.enumerate_test_files().iter().enumerate() {
        if SKIPPED_TESTS.contains(&get_base_file_name(filename).as_str()) {
            continue;
        }
        let basename = get_base_file_name(filename);
        let test = match catch_unwind(AssertUnwindSafe(|| get_compiler_file_based_test(filename))) {
            Ok(test) => test,
            Err(payload) => {
                errors.push(format!(
                    "{}/{basename}: {}",
                    runner.test_suit_name,
                    payload_text(payload.as_ref())
                ));
                continue;
            }
        };
        let make = |configuration: Option<String>| {
            let mut test_name = basename.clone();
            if let Some(name) = configuration.as_deref()
                && !name.is_empty()
            {
                test_name.push(' ');
                test_name.push_str(name);
            }
            ConfigCase {
                is_submodule: runner.is_submodule,
                suite: runner.test_suit_name,
                filename: filename.clone(),
                configuration,
                test_name,
                file_index: file_index_start + i,
            }
        };
        if !test.configurations.is_empty() {
            let mut names: Vec<String> = test
                .configurations
                .iter()
                .map(|config| config.name.clone())
                .collect();
            names.sort();
            for name in names {
                cases.push(make(Some(name)));
            }
        } else {
            cases.push(make(None));
        }
    }
    (cases, errors)
}

// Go: compiler_runner.go:195 runSingleConfigTest
// PORT: the child side of one case. Each Go subtest reports through
// `report`; `compile` is the Go test itself (its `RecoverAndFail` covers
// `newCompilerTest`), and `config` reports a skipped configuration.
pub fn run_single_config_test(case: &ConfigCase, report: Report<'_>) {
    let test = get_compiler_file_based_test(&case.filename);
    let config = match &case.configuration {
        Some(name) => {
            let Some(config) = test
                .configurations
                .iter()
                .find(|config| config.name == *name)
            else {
                report(
                    "compile",
                    Outcome::Fail(format!("configuration {name} not found")),
                );
                return;
            };
            Some(config.clone())
        }
        None => None,
    };

    let compiled = catch_unwind(AssertUnwindSafe(|| {
        let payload = make_units_from_test(&test.content, &test.filename);
        let inputs = new_compiler_test_inputs(payload, config.as_ref());
        // PORT: Go skips (or fails, ts#64122) after the compilation; the
        // options do not change there, so the check comes first and a skipped
        // or failed configuration is not compiled.
        if let Some(unsupported) =
            skip_unsupported_compiler_options(&precompute_compiler_options(&inputs))
        {
            return Err(unsupported);
        }
        Ok(new_compiler_test(
            &case.test_name,
            &test.filename,
            inputs,
            config.as_ref(),
        ))
    }));
    let compiler_test = match compiled {
        Ok(Ok(compiler_test)) => compiler_test,
        Ok(Err(UnsupportedCompilerOptions::Skip(skip_message))) => {
            report("config", Outcome::Skip(skip_message));
            return;
        }
        Ok(Err(UnsupportedCompilerOptions::Fail(message))) => {
            report("compile", Outcome::Fail(message));
            return;
        }
        Err(payload) => {
            if let Some(skip) = payload.downcast_ref::<SkipPayload>() {
                report("config", Outcome::Skip(skip.0.clone()));
            } else {
                report(
                    "compile",
                    Outcome::Fail(format!(
                        "Panic on compiler test {}:\n{}",
                        test.filename,
                        payload_text(payload.as_ref())
                    )),
                );
            }
            return;
        }
    };
    report("compile", Outcome::Pass);

    let suite = case.suite;
    let is_submodule = case.is_submodule;
    compiler_test.verify_diagnostics(report, suite, is_submodule);
    compiler_test.verify_content_mapper(report, suite, is_submodule);
    compiler_test.verify_java_script_output(report, suite, is_submodule);
    compiler_test.verify_source_map_output(report, suite, is_submodule);
    compiler_test.verify_source_map_record(report, suite, is_submodule);
    compiler_test.verify_types_and_symbols(report, suite, is_submodule);
    compiler_test.verify_module_resolution(report, suite, is_submodule);
    compiler_test.verify_union_ordering(report);
    compiler_test.verify_parent_pointers(report);
}
