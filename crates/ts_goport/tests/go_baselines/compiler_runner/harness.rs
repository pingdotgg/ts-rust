//! Go: internal/testutil/harnessutil/harnessutil.go (the parts the compiler
//! runner uses that `support::harnessutil` does not have: `CompileFiles`,
//! the option setup, `CompilationResult`, the file based test
//! configurations, `SkipUnsupportedCompilerOptions`, `EnumerateFiles`) and
//! recorderfs.go.
//!
//! PORT: the program is process state in the port. Each compilation is a
//! program version (`program::new_program_version`) that a caller reads
//! inside `core::enter_program` (`CompilationResult::enter`). The OS
//! override is for the whole process, so every compilation of one test
//! shares one `MapFs` (`GLOBAL_FS`): a compilation imports its files into
//! it, and `CompilationResult::enter` imports the files of that
//! compilation again. A test process runs one test configuration (see
//! child.rs).
//!
//! PORT: Go `t.Fatalf` and `t.Skipf` are panics here (`fatal`, `skip`); the
//! child catches them per subtest.

use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::{Arc, Mutex, OnceLock};

use ts_goport::baseline::type_symbol::TestFile;
use ts_goport::contentmapper::{self, Mapper, ProjectSpec};
use ts_goport::core::{ProgramScope, enter_program};
use ts_goport::emitter::program_emit::{EmitOptions, EmitResult, WriteFile, WriteFileData};
use ts_goport::execute::incremental::build_info::BuildInfo;
use ts_goport::execute::incremental::incremental::{
    BuildInfoReader, create_host, new_build_info_reader,
};
use ts_goport::execute::incremental::program::{
    Program as IncrementalProgram, new_program as new_incremental_program, read_build_info_program,
};
use ts_goport::execute::tsc::compile::Writer;
use ts_goport::frontend::bundled;
use ts_goport::frontend::compiler::{CompilerHost, ProgramOptions, new_compiler_host};
use ts_goport::frontend::outputpaths::get_output_extension;
use ts_goport::frontend::prelude::*;
use ts_goport::frontend::vfs::{OsOverride, install_os_override};
use ts_goport::gostd::context::background;
use ts_goport::program as tsprogram;

use crate::support::baseline::{is_merged_layout, test_data_path};
use crate::support::contentmappertest;
use crate::support::harnessutil::TracerForBaselining;
use crate::support::vfstest::{self, MapFile, MapFs};
use crate::tsoptions::tsoptionstest::type_script_submodule_path;

// Go: harnessutil.go:39 testLibFolder
// Posix-style path to additional test libraries
const TEST_LIB_FOLDER: &str = "/.lib";

// Go: harnessutil.go:55 TestConfiguration
// This maps a compiler setting to its string value, after splitting by commas,
// handling inclusions and exclusions, and deduplicating.
// For example, if a test file contains:
//
//	// @target: esnext, es2015
//
// Then the map will map "target" to "esnext", and another map will map "target" to "es2015".
pub type TestConfiguration = BTreeMap<String, String>;

// Go: harnessutil.go:57 NamedTestConfiguration
#[derive(Clone, Debug)]
pub struct NamedTestConfiguration {
    pub name: String,
    pub config: TestConfiguration,
}

// Go: harnessutil.go:62 HarnessOptions
#[derive(Clone, Debug, Default)]
pub struct HarnessOptions {
    pub use_case_sensitive_file_names: bool,
    pub baseline_file: String,
    pub include_built_file: String,
    pub file_name: String,
    pub lib_files: Vec<String>,
    pub no_implicit_references: bool,
    pub current_directory: String,
    pub symlink: String,
    pub link: String,
    pub no_types_and_symbols: bool,
    pub full_emit_paths: bool,
    pub report_diagnostics: bool,
    pub capture_suggestions: bool,
    pub typescript_version: String,
}

/// Go `t.Fatalf`: fails the running subtest.
pub fn fatal(message: String) -> ! {
    panic!("{message}");
}

/// The panic payload of Go `t.Skipf` (see `skip`).
pub struct SkipPayload(pub String);

/// Go `t.Skipf`: skips the running test.
pub fn skip(message: String) -> ! {
    std::panic::panic_any(SkipPayload(message));
}

/// The part of a Go `*tsoptions.ParsedCommandLine` that `CompileFilesEx`
/// reads: `ConfigFile`, `Errors` and `ParsedConfig.ContentMappers`
/// (tsgo#4712).
#[derive(Clone, Default)]
pub struct TsConfigPart {
    pub config_file: Option<Rc<TsConfigSourceFile>>,
    pub errors: Vec<Diagnostic>,
    pub content_mappers: Vec<Rc<Mapper>>,
}

/// Go `defer f()`: runs `f` when the scope ends, also on a panic.
struct Defer<F: FnOnce()>(Option<F>);

impl<F: FnOnce()> Drop for Defer<F> {
    fn drop(&mut self) {
        if let Some(f) = self.0.take() {
            f();
        }
    }
}

// Go: harnessutil.go:79 CompileFiles
pub fn compile_files(
    input_files: &[TestFile],
    other_files: &[TestFile],
    test_config: Option<&TestConfiguration>,
    tsconfig: Option<&ParsedCommandLine>,
    current_directory: &str,
    symlinks: &BTreeMap<String, String>,
) -> CompilationResult {
    let mut compiler_options = match tsconfig {
        Some(tsconfig) => (*tsconfig.parsed_config.compiler_options).clone(),
        None => CompilerOptions::default(),
    };
    // Set default options for tests
    if compiler_options.new_line == NewLineKind::NONE {
        compiler_options.new_line = NewLineKind::CRLF;
    }
    if compiler_options.skip_default_lib_check == Tristate::Unknown {
        compiler_options.skip_default_lib_check = Tristate::True;
    }
    compiler_options.no_error_truncation = Tristate::True;
    let mut harness_options = HarnessOptions {
        use_case_sensitive_file_names: true,
        current_directory: current_directory.to_string(),
        ..HarnessOptions::default()
    };

    // Parse harness and compiler options from the test configuration
    if let Some(test_config) = test_config {
        set_options_from_test_config(
            test_config,
            &mut compiler_options,
            &mut harness_options,
            current_directory,
            false, /*allowUnknownOptions*/
        );
    }

    let tsconfig_part = tsconfig.map(|tsconfig| TsConfigPart {
        config_file: tsconfig.config_file.clone(),
        errors: tsconfig.errors.clone(),
        content_mappers: tsconfig.parsed_config.content_mappers.clone(),
    });
    compile_files_ex(
        input_files,
        other_files,
        &harness_options,
        &mut compiler_options,
        current_directory,
        symlinks,
        tsconfig_part.as_ref(),
    )
}

/// The inputs of a `CompileFilesEx` call, for `CompilationResult::repeat`.
#[derive(Clone)]
struct CompileInputs {
    input_files: Vec<TestFile>,
    other_files: Vec<TestFile>,
    harness_options: HarnessOptions,
    compiler_options: CompilerOptions,
    current_directory: String,
    symlinks: BTreeMap<String, String>,
    tsconfig: Option<TsConfigPart>,
}

// Go: harnessutil.go:113 CompileFilesEx
pub fn compile_files_ex(
    input_files: &[TestFile],
    other_files: &[TestFile],
    harness_options: &HarnessOptions,
    compiler_options: &mut CompilerOptions,
    current_directory: &str,
    symlinks: &BTreeMap<String, String>,
    tsconfig: Option<&TsConfigPart>,
) -> CompilationResult {
    let mut program_file_names = Vec::new();
    for file in input_files {
        let file_name = get_normalized_absolute_path(&file.unit_name, current_directory);

        if !file_extension_is(&file_name, EXTENSION_JSON)
            && !file_extension_is(&file_name, EXTENSION_TS_BUILD_INFO)
        {
            program_file_names.push(file_name);
        }
    }

    // !!! Note: lib files are not going to be in `built/local`.
    // In addition, not all files that used to be in `built/local` are going to exist.
    // Files from built\local that are requested by test "@includeBuiltFiles" to be in the context.
    // Treat them as library files, so include them in build, but not in baselines.

    // Performance optimization; avoid copying in the /.lib folder if the test doesn't need it.
    let lib_folder_prefix = format!("{TEST_LIB_FOLDER}/");
    let mut include_lib_dir = input_files
        .iter()
        .any(|file| file.content.contains(&lib_folder_prefix));

    // Files from testdata\lib that are requested by "@libFiles"
    for lib_file in &harness_options.lib_files {
        if lib_file == "lib.d.ts" && compiler_options.no_lib != Tristate::True {
            // We used to override lib with a custom lib.d.ts for some reason. Skip this unless it becomes necessary.
            continue;
        }
        program_file_names.push(combine_paths(TEST_LIB_FOLDER, &[lib_file]));
        include_lib_dir = true;
    }

    // The merged layout has no submodule and no skip here.
    if include_lib_dir
        && !is_merged_layout()
        && !type_script_submodule_path().join("package.json").exists()
    {
        skip("TypeScript submodule does not exist".to_string());
    }

    // !!!
    // ts.assign(options, ts.convertToOptionsWithAbsolutePaths(options, path => ts.getNormalizedAbsolutePath(path, currentDirectory)));
    if !compiler_options.out_dir.is_empty() {
        compiler_options.out_dir =
            get_normalized_absolute_path(&compiler_options.out_dir, current_directory);
    }
    if !compiler_options.project.is_empty() {
        compiler_options.project =
            get_normalized_absolute_path(&compiler_options.project, current_directory);
    }
    if !compiler_options.root_dir.is_empty() {
        compiler_options.root_dir =
            get_normalized_absolute_path(&compiler_options.root_dir, current_directory);
    }
    if !compiler_options.ts_build_info_file.is_empty() {
        compiler_options.ts_build_info_file =
            get_normalized_absolute_path(&compiler_options.ts_build_info_file, current_directory);
    }
    if !compiler_options.base_url.is_empty() {
        compiler_options.base_url =
            get_normalized_absolute_path(&compiler_options.base_url, current_directory);
    }
    if !compiler_options.declaration_dir.is_empty() {
        compiler_options.declaration_dir =
            get_normalized_absolute_path(&compiler_options.declaration_dir, current_directory);
    }
    if let Some(root_dirs) = compiler_options.root_dirs.as_mut() {
        for root_dir in root_dirs.iter_mut() {
            *root_dir = get_normalized_absolute_path(root_dir, current_directory);
        }
    }
    if let Some(type_roots) = compiler_options.type_roots.as_mut() {
        for type_root in type_roots.iter_mut() {
            *type_root = get_normalized_absolute_path(type_root, current_directory);
        }
    }

    let content_mappers: Vec<Rc<Mapper>> = tsconfig
        .map(|tsconfig| tsconfig.content_mappers.clone())
        .unwrap_or_default();

    // Create fake FS for testing
    let mut testfs: BTreeMap<String, MapFile> = BTreeMap::new();
    for file in input_files {
        let file_name = get_normalized_absolute_path(&file.unit_name, current_directory);
        testfs.insert(file_name, MapFile::from(file.content.as_str()));
    }
    for file in other_files {
        let file_name = get_normalized_absolute_path(&file.unit_name, current_directory);
        testfs.insert(file_name, MapFile::from(file.content.as_str()));
    }
    for (src, target) in symlinks {
        let src_file_name = get_normalized_absolute_path(src, current_directory);
        let target_file_name = get_normalized_absolute_path(target, current_directory);
        testfs.insert(src_file_name, vfstest::symlink(&target_file_name));
    }

    if include_lib_dir {
        for (path, file) in test_lib_folder_map() {
            testfs.insert(path.clone(), file.clone());
        }
    }

    let fs = MapFs::from_map(testfs, harness_options.use_case_sensitive_file_names);
    install_global_fs(
        &fs,
        current_directory,
        harness_options.use_case_sensitive_file_names,
    );
    let recorder = OutputRecorder::default();

    // Content mappers, when trusted, are served in-process by the test mapper (see contentmappertest).
    // The host is shared by the pre- and post-emit programs and torn down when this compilation finishes.
    let content_mapper_host: Option<Rc<dyn contentmapper::Host>> =
        (compiler_options.run_external_code.is_true() && !content_mappers.is_empty()).then(|| {
            contentmapper::new_host(
                &background(),
                contentmappertest::new_spawner(),
                ts_goport::locale::DEFAULT,
            )
        });
    let _close_content_mapper_host = Defer(content_mapper_host.clone().map(|host| {
        move || {
            let _ = host.close();
        }
    }));

    let config = Rc::new(ParsedCommandLine {
        parsed_config: ParsedOptions {
            compiler_options: Rc::new(compiler_options.clone()),
            file_names: program_file_names,
            content_mappers,
            ..ParsedOptions::default()
        },
        config_file: tsconfig.and_then(|tsconfig| tsconfig.config_file.clone()),
        errors: tsconfig
            .map(|tsconfig| tsconfig.errors.clone())
            .unwrap_or_default(),
        ..ParsedCommandLine::default()
    });
    // PORT: Go `Host.Project` returns nil only after `Close`, which cannot
    // happen here.
    let content_mapper_project: Option<Rc<dyn contentmapper::Project>> =
        content_mapper_host.as_ref().and_then(|host| {
            host.project(ProjectSpec {
                config_file_name: config.config_name().to_string(),
                mappers: config.content_mappers().to_vec(),
                compiler_options: Some(config.compiler_options().clone()),
            })
        });
    let _close_content_mapper_project = Defer(content_mapper_project.clone().map(|project| {
        move || {
            let _ = project.close();
        }
    }));

    let tracer_bytes: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
    let tracer_writer: Writer = tracer_bytes.clone();
    let tracer = Rc::new(RefCell::new(TracerForBaselining::new(
        ComparePathsOptions {
            use_case_sensitive_file_names: harness_options.use_case_sensitive_file_names,
            current_directory: current_directory.to_string(),
        },
        tracer_writer,
        Some(tracer_bytes),
    )));
    let host = create_compiler_host(current_directory, tracer.clone(), content_mapper_project);
    let mut result = compile_files_with_host(host, config, harness_options, &recorder);
    result.symlinks = symlinks.clone();
    result.trace = tracer.borrow().string();
    result.repeat_inputs = Some(Box::new(CompileInputs {
        input_files: input_files.to_vec(),
        other_files: other_files.to_vec(),
        harness_options: harness_options.clone(),
        compiler_options: compiler_options.clone(),
        current_directory: current_directory.to_string(),
        symlinks: symlinks.clone(),
        tsconfig: tsconfig.cloned(),
    }));
    result
}

// Go: harnessutil.go:241 testLibFolderMap
// The lib dir is `testdata/tests/lib` at the merged layout, the submodule's
// `tests/lib` before.
fn test_lib_folder_map() -> &'static BTreeMap<String, MapFile> {
    static MAP: OnceLock<BTreeMap<String, MapFile>> = OnceLock::new();
    MAP.get_or_init(|| {
        let mut testfs = BTreeMap::new();
        let root = if is_merged_layout() {
            test_data_path()
        } else {
            type_script_submodule_path()
        }
        .join("tests")
        .join("lib");
        fn walk(dir: &std::path::Path, rel: &str, testfs: &mut BTreeMap<String, MapFile>) {
            let mut entries: Vec<_> = std::fs::read_dir(dir)
                .unwrap_or_else(|err| panic!("Failed to read lib dir: {err}"))
                .map(|entry| entry.expect("lib dir entry"))
                .collect();
            entries.sort_by_key(std::fs::DirEntry::file_name);
            for entry in entries {
                let name = entry.file_name().to_string_lossy().into_owned();
                let path = if rel.is_empty() {
                    name.clone()
                } else {
                    format!("{rel}/{name}")
                };
                if entry.file_type().expect("lib dir entry type").is_dir() {
                    walk(&entry.path(), &path, testfs);
                } else {
                    let content = std::fs::read(entry.path())
                        .unwrap_or_else(|err| panic!("Failed to read lib dir: {err}"));
                    testfs.insert(format!("{TEST_LIB_FOLDER}/{path}"), MapFile::from(content));
                }
            }
        }
        walk(&root, "", &mut testfs);
        testfs
    })
}

// Go: harnessutil.go:266 SetOptionsFromTestConfig
pub fn set_options_from_test_config(
    test_config: &TestConfiguration,
    compiler_options: &mut CompilerOptions,
    harness_options: &mut HarnessOptions,
    current_directory: &str,
    allow_unknown_options: bool,
) {
    for (name, value) in test_config {
        if name == "typescriptversion" {
            continue;
        }

        if let Some(command_line_option) = get_command_line_option(name) {
            let parsed_value = get_option_value(command_line_option, value, current_directory);
            let errors =
                parse_compiler_options(command_line_option.name, parsed_value, compiler_options);
            if !errors.is_empty() {
                fatal(format!(
                    "Error parsing value '{value}' for compiler option '{}'.",
                    command_line_option.name
                ));
            }
            continue;
        }
        if let Some(harness_option) = get_harness_option(name) {
            let parsed_value = get_option_value(harness_option, value, current_directory);
            parse_harness_option(harness_option.name, parsed_value, harness_options);
            continue;
        }
        if !allow_unknown_options {
            fatal(format!("Unknown compiler option '{name}'."));
        }
    }
}

/// Leaks a declaration so it has the Go pointer lifetime.
fn leak_option(name: &'static str, kind: CommandLineOptionKind) -> &'static CommandLineOption {
    Box::leak(Box::new(CommandLineOption {
        name,
        kind,
        ..CommandLineOption::default()
    }))
}

// Go: harnessutil.go:293 compilerOptions
fn compiler_options_declarations() -> &'static [&'static CommandLineOption] {
    static DECLS: OnceLock<Vec<&'static CommandLineOption>> = OnceLock::new();
    DECLS.get_or_init(|| {
        let mut decls: Vec<&'static CommandLineOption> = OPTIONS_DECLARATIONS.clone();
        decls.push(leak_option(
            "allowNonTsExtensions",
            CommandLineOptionKind::BOOLEAN,
        ));
        decls.push(leak_option(
            "noErrorTruncation",
            CommandLineOptionKind::BOOLEAN,
        ));
        decls.push(leak_option(
            "suppressOutputPathCheck",
            CommandLineOptionKind::BOOLEAN,
        ));
        decls.push(leak_option("noCheck", CommandLineOptionKind::BOOLEAN));
        decls
    })
}

// Go: harnessutil.go:315 harnessCommandLineOptions
fn harness_command_line_options() -> &'static [&'static CommandLineOption] {
    static DECLS: OnceLock<Vec<&'static CommandLineOption>> = OnceLock::new();
    DECLS.get_or_init(|| {
        vec![
            leak_option("useCaseSensitiveFileNames", CommandLineOptionKind::BOOLEAN),
            leak_option("baselineFile", CommandLineOptionKind::STRING),
            leak_option("includeBuiltFile", CommandLineOptionKind::STRING),
            leak_option("fileName", CommandLineOptionKind::STRING),
            leak_option("libFiles", CommandLineOptionKind::LIST),
            leak_option("noImplicitReferences", CommandLineOptionKind::BOOLEAN),
            leak_option("currentDirectory", CommandLineOptionKind::STRING),
            leak_option("symlink", CommandLineOptionKind::STRING),
            leak_option("link", CommandLineOptionKind::STRING),
            leak_option("noTypesAndSymbols", CommandLineOptionKind::BOOLEAN),
            // Emitted js baseline will print full paths for every output file
            leak_option("fullEmitPaths", CommandLineOptionKind::BOOLEAN),
            // used to enable error collection in `transpile` baselines
            leak_option("reportDiagnostics", CommandLineOptionKind::BOOLEAN),
            // Adds suggestion diagnostics to error baselines
            leak_option("captureSuggestions", CommandLineOptionKind::BOOLEAN),
        ]
    })
}

// Go: harnessutil.go:373 getHarnessOption
fn get_harness_option(name: &str) -> Option<&'static CommandLineOption> {
    harness_command_line_options()
        .iter()
        .copied()
        .find(|option| option.name.eq_ignore_ascii_case(name))
}

// Go: harnessutil.go:379 parseHarnessOption
fn parse_harness_option(
    key: &str,
    value: CompilerOptionsValue,
    harness_options: &mut HarnessOptions,
) {
    let as_bool = |value: &CompilerOptionsValue| match value {
        CompilerOptionsValue::Bool(b) => *b,
        _ => panic!("interface conversion: interface {{}} is not bool"),
    };
    let as_string = |value: &CompilerOptionsValue| match value {
        CompilerOptionsValue::String(s) => s.clone(),
        _ => panic!("interface conversion: interface {{}} is not string"),
    };
    match key {
        "useCaseSensitiveFileNames" => {
            harness_options.use_case_sensitive_file_names = as_bool(&value);
        }
        "baselineFile" => harness_options.baseline_file = as_string(&value),
        "includeBuiltFile" => harness_options.include_built_file = as_string(&value),
        "fileName" => harness_options.file_name = as_string(&value),
        "libFiles" => {
            let Some(list) = value.as_any_slice() else {
                panic!("interface conversion: interface {{}} is not []interface {{}}");
            };
            harness_options.lib_files = list.iter().map(as_string).collect();
        }
        "noImplicitReferences" => harness_options.no_implicit_references = as_bool(&value),
        "currentDirectory" => harness_options.current_directory = as_string(&value),
        "symlink" => harness_options.symlink = as_string(&value),
        "link" => harness_options.link = as_string(&value),
        "noTypesAndSymbols" => harness_options.no_types_and_symbols = as_bool(&value),
        "fullEmitPaths" => harness_options.full_emit_paths = as_bool(&value),
        "reportDiagnostics" => harness_options.report_diagnostics = as_bool(&value),
        "captureSuggestions" => harness_options.capture_suggestions = as_bool(&value),
        "typescriptVersion" => harness_options.typescript_version = as_string(&value),
        _ => fatal(format!("Unknown harness option '{key}'.")),
    }
}

// Go: harnessutil.go:417 getOptionValue
fn get_option_value(
    option: &'static CommandLineOption,
    value: &str,
    cwd: &str,
) -> CompilerOptionsValue {
    match option.kind {
        CommandLineOptionKind::STRING => {
            if option.is_file_path {
                return CompilerOptionsValue::String(get_normalized_absolute_path(value, cwd));
            }
            CompilerOptionsValue::String(value.to_string())
        }
        CommandLineOptionKind::NUMBER => match go_atoi(value) {
            Some(num) => CompilerOptionsValue::Int(num),
            None => fatal(format!(
                "Value for option '{}' must be a number, got: {value}",
                option.name
            )),
        },
        CommandLineOptionKind::BOOLEAN => match value.to_lowercase().as_str() {
            "true" => CompilerOptionsValue::Bool(true),
            "false" => CompilerOptionsValue::Bool(false),
            _ => fatal(format!(
                "Value for option '{}' must be a boolean, got: {value}",
                option.name
            )),
        },
        CommandLineOptionKind::ENUM => {
            let enum_map = option.enum_map().expect("enum option has a map");
            match enum_map.get(&value.to_lowercase()) {
                Some(enum_val) => enum_val.clone(),
                None => fatal(format!(
                    "Value for option '{}' must be one of {}, got: {value}",
                    option.name,
                    enum_map.keys().cloned().collect::<Vec<_>>().join(",")
                )),
            }
        }
        CommandLineOptionKind::LIST | CommandLineOptionKind::LIST_OR_ELEMENT => {
            let (list_val, errors) = parse_list_type_option(option, value);
            if option
                .elements()
                .is_some_and(|elements| elements.is_file_path)
            {
                // Go `core.Map` keeps a nil list nil.
                let CompilerOptionsValue::List(list) = list_val else {
                    return list_val;
                };
                return CompilerOptionsValue::List(
                    list.into_iter()
                        .map(|item| {
                            let CompilerOptionsValue::String(item) = item else {
                                panic!("interface conversion: interface {{}} is not string");
                            };
                            CompilerOptionsValue::String(get_normalized_absolute_path(&item, cwd))
                        })
                        .collect(),
                );
            }
            if !errors.is_empty() {
                fatal(format!(
                    "Unknown value '{value}' for compiler option '{}'",
                    option.name
                ));
            }
            list_val
        }
        CommandLineOptionKind::OBJECT => fatal(format!(
            "Object type options like '{}' are not supported",
            option.name
        )),
        _ => CompilerOptionsValue::Nil,
    }
}

/// Go `strconv.Atoi`: an optional sign and decimal digits that fit a
/// 64-bit int.
fn go_atoi(value: &str) -> Option<i64> {
    let digits = value
        .strip_prefix('+')
        .or_else(|| value.strip_prefix('-'))
        .unwrap_or(value);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    value.parse::<i64>().ok()
}

// Go: harnessutil.go:462 cachedCompilerHost
// PORT: Go keeps a process-wide parse cache (`sourceFileCache`) so the
// programs of all tests share lib files. A test process here runs one
// configuration, so the host parses each file (Go `compiler.NewCompilerHost`).

// Go: harnessutil.go:590 createCompilerHost
// PORT: Go wraps the map file system in `bundled.WrapFS` and the output
// recorder. Here the host reads the shared map through bundled, and emit
// writes go through the recorder's `WriteFile` callback (see
// `OutputRecorder`).
fn create_compiler_host(
    current_directory: &str,
    tracer: Rc<RefCell<TracerForBaselining>>,
    content_mapper_project: Option<Rc<dyn contentmapper::Project>>,
) -> Rc<dyn CompilerHost> {
    let fs = bundled::wrap_fs(global_fs().fs());
    new_compiler_host(
        current_directory,
        fs,
        &bundled::lib_path(),
        None,
        Some(Rc::new(move |msg: &'static Message, args: Vec<String>| {
            tracer.borrow_mut().trace(msg, args);
        })),
        content_mapper_project,
    )
}

/// A Go `compiler.ProgramLike`: the program, or the incremental program
/// when `incremental` is set (Go `createProgram`).
enum ProgramLike {
    Program(&'static GoProgram),
    Incremental(Box<IncrementalProgram>),
}

impl ProgramLike {
    fn program(&self) -> &'static GoProgram {
        match self {
            ProgramLike::Program(program) => program,
            ProgramLike::Incremental(program) => program.get_program(),
        }
    }
}

// Go: harnessutil.go:601 compileFilesWithHost
fn compile_files_with_host(
    host: Rc<dyn CompilerHost>,
    config: Rc<ParsedCommandLine>,
    harness_options: &HarnessOptions,
    recorder: &OutputRecorder,
) -> CompilationResult {
    // !!!
    // if (compilerOptions.project || !rootFiles || rootFiles.length === 0) { ... readProject ... }

    let current_directory = host.get_current_directory();
    let use_case_sensitive_file_names = host.fs().use_case_sensitive_file_names();

    let mut pre_compiler_options = (**config.compiler_options()).clone();
    pre_compiler_options.trace_resolution = Tristate::False;
    let pre_config = Rc::new(ParsedCommandLine {
        parsed_config: ParsedOptions {
            compiler_options: Rc::new(pre_compiler_options),
            file_names: config.file_names().to_vec(),
            content_mappers: config.content_mappers().to_vec(),
            ..ParsedOptions::default()
        },
        config_file: config.config_file.clone(),
        errors: config.errors.clone(),
        ..ParsedCommandLine::default()
    });
    let pre_program = create_program(host.clone(), pre_config.clone());
    let pre_errors = {
        let _scope = enter_program(Some(pre_program.program()));
        tsprogram::sort_and_deduplicate_diagnostics(get_program_like_diagnostics(
            &pre_program,
            &pre_config,
            harness_options,
            true, /*suggestionsFirst*/
        ))
    };

    let post_program = create_program(host, config.clone());
    let _scope = enter_program(Some(post_program.program()));
    let emit_options = EmitOptions {
        write_file: Some(recorder.write_file()),
        ..EmitOptions::default()
    };
    let emit_result = match &post_program {
        ProgramLike::Program(_) => ts_goport::emitter::program_emit::emit(emit_options),
        ProgramLike::Incremental(program) => program.emit(emit_options),
    };
    let post_errors = tsprogram::sort_and_deduplicate_diagnostics(get_program_like_diagnostics(
        &post_program,
        &config,
        harness_options,
        false, /*suggestionsFirst*/
    ));

    let errors = if post_errors.len() != pre_errors.len() {
        let (longer_errors, shorter_errors) = if pre_errors.len() > post_errors.len() {
            (&pre_errors, &post_errors)
        } else {
            (&post_errors, &pre_errors)
        };
        let mut diag = new_ad_hoc_compiler_diagnostic(format!(
            "Pre-emit ({}) and post-emit ({}) diagnostic counts do not match! This can indicate that a semantic _error_ was added by the emit resolver - such an error may not be reflected on the command line or in the editor, but may be captured in a baseline here!",
            pre_errors.len(),
            post_errors.len()
        ));
        diag.add_related_info(Some(new_ad_hoc_compiler_diagnostic(
            "The excess diagnostics are:".to_string(),
        )));
        for d in longer_errors {
            let matched = shorter_errors
                .iter()
                .any(|d2| ts_goport::ast::compare_diagnostics(d, d2) == 0);
            if !matched {
                diag.add_related_info(Some(d.clone()));
            }
        }
        let mut errors = shorter_errors.clone();
        errors.push(diag);
        errors
    } else {
        post_errors
    };

    new_compilation_result(
        current_directory,
        use_case_sensitive_file_names,
        post_program.program(),
        config,
        emit_result,
        errors,
        harness_options,
        recorder.outputs(),
    )
}

/// The diagnostics that Go `compileFilesWithHost` collects from one program,
/// in its order. The program is current. `suggestions_first` is the
/// `preProgram` order (ts#64479, harnessutil.go:643): the suggestions are
/// read before declaration emit can add any, so a suggestion that the emit
/// resolver adds shows as a pre/post count mismatch. `postProgram` reads the
/// declaration diagnostics first (:659).
// PORT: Go writes these lines out twice, for `preProgram` and `postProgram`.
// Go reads `program.Options()`; `config` holds the same options.
fn get_program_like_diagnostics(
    program: &ProgramLike,
    config: &ParsedCommandLine,
    harness_options: &HarnessOptions,
    suggestions_first: bool,
) -> Vec<Diagnostic> {
    let emit_declarations = config.compiler_options().get_emit_declarations();
    let capture_suggestions = harness_options.capture_suggestions;
    let mut diagnostics: Vec<Diagnostic> = Vec::new();
    match program {
        ProgramLike::Program(_) => {
            diagnostics.extend(tsprogram::get_config_file_parsing_diagnostics());
            diagnostics.extend(tsprogram::get_program_diagnostics());
            diagnostics.extend(tsprogram::get_syntactic_diagnostics(Node::NIL));
            diagnostics.extend(tsprogram::get_semantic_diagnostics(Node::NIL));
            diagnostics.extend(tsprogram::get_global_diagnostics());
            extend_declaration_and_suggestion_diagnostics(
                &mut diagnostics,
                suggestions_first,
                emit_declarations.then_some(|| tsprogram::get_declaration_diagnostics(Node::NIL)),
                capture_suggestions.then_some(|| tsprogram::get_suggestion_diagnostics(Node::NIL)),
            );
        }
        ProgramLike::Incremental(program) => {
            diagnostics.extend(program.get_config_file_parsing_diagnostics());
            diagnostics.extend(program.get_program_diagnostics());
            diagnostics.extend(program.get_syntactic_diagnostics(Node::NIL));
            diagnostics.extend(program.get_semantic_diagnostics(Node::NIL));
            diagnostics.extend(program.get_global_diagnostics());
            extend_declaration_and_suggestion_diagnostics(
                &mut diagnostics,
                suggestions_first,
                emit_declarations.then_some(|| program.get_declaration_diagnostics(Node::NIL)),
                capture_suggestions.then_some(|| program.get_suggestion_diagnostics(Node::NIL)),
            );
        }
    }
    diagnostics
}

/// Reads the declaration and the suggestion diagnostics of
/// `get_program_like_diagnostics` in the order that `suggestions_first`
/// gives. The read order matters: declaration emit can add diagnostics.
fn extend_declaration_and_suggestion_diagnostics(
    diagnostics: &mut Vec<Diagnostic>,
    suggestions_first: bool,
    declarations: Option<impl FnOnce() -> Vec<Diagnostic>>,
    suggestions: Option<impl FnOnce() -> Vec<Diagnostic>>,
) {
    if suggestions_first {
        diagnostics.extend(suggestions.into_iter().flat_map(|read| read()));
        diagnostics.extend(declarations.into_iter().flat_map(|read| read()));
    } else {
        diagnostics.extend(declarations.into_iter().flat_map(|read| read()));
        diagnostics.extend(suggestions.into_iter().flat_map(|read| read()));
    }
}

// Go: diagnostics/diagnostics.go:152 NewAdHocMessage, used as
// `ast.NewCompilerDiagnostic(diagnostics.NewAdHocMessage(message))`.
// PORT: `ts_goport::diagnostics::Message` has a `u32` code, so the Go code -1 is set
// on the diagnostic (`Diagnostic.code`, which the baselines print). A
// diagnostic holds a `&'static Message`, so the message is leaked. Decision
// (bump A queue #78): this test helper stays out of the shared
// `ts_goport::diagnostics` module.
fn new_ad_hoc_compiler_diagnostic(message: String) -> Diagnostic {
    let text: &'static str = Box::leak(message.into_boxed_str());
    let message: &'static Message = Box::leak(Box::new(Message::new(
        0,
        ts_goport::diagnostics::Category::Error,
        "-1",
        text,
        false,
        false,
        false,
    )));
    let mut diag = ts_goport::ast::new_compiler_diagnostic(message, Vec::new());
    diag.code = -1;
    diag
}

// Go: harnessutil.go:679 CompilationResult
// PORT: `Program` is the program version, read inside `enter`. `Host` is
// kept as its current directory and case sensitivity. `Repeat` is
// `repeat`. `inputsAndOutputs` is not kept: the compiler runner does not
// read it.
pub struct CompilationResult {
    pub diagnostics: Vec<Diagnostic>,
    pub result: EmitResult,
    pub program: &'static GoProgram,
    pub options: CompilerOptions,
    pub harness_options: HarnessOptions,
    pub js: IndexMap<String, TestFile>,
    pub dts: IndexMap<String, TestFile>,
    pub maps: IndexMap<String, TestFile>,
    pub symlinks: BTreeMap<String, String>,
    outputs: Vec<TestFile>,
    inputs: Vec<TestFile>,
    pub trace: String,
    /// Go `Host.GetCurrentDirectory()`.
    pub current_directory: String,
    /// Go `Host.FS().UseCaseSensitiveFileNames()`.
    pub use_case_sensitive_file_names: bool,
    /// Go `Program.Program().CommandLine()`.
    pub command_line: Rc<ParsedCommandLine>,
    /// The files of the compilation's file system after emit.
    fs_state: crate::support::vfstest::MapFsState,
    repeat_inputs: Option<Box<CompileInputs>>,
}

// Go: harnessutil.go:704 newCompilationResult
#[allow(clippy::too_many_arguments)]
fn new_compilation_result(
    current_directory: String,
    use_case_sensitive_file_names: bool,
    program: &'static GoProgram,
    command_line: Rc<ParsedCommandLine>,
    result: EmitResult,
    diagnostics: Vec<Diagnostic>,
    harness_options: &HarnessOptions,
    recorded_outputs: Vec<TestFile>,
) -> CompilationResult {
    let options = program.options.clone();

    let mut c = CompilationResult {
        diagnostics,
        result,
        program,
        options,
        harness_options: harness_options.clone(),
        js: IndexMap::new(),
        dts: IndexMap::new(),
        maps: IndexMap::new(),
        symlinks: BTreeMap::new(),
        outputs: Vec::new(),
        inputs: Vec::new(),
        trace: String::new(),
        current_directory,
        use_case_sensitive_file_names,
        command_line,
        fs_state: global_fs().export_state(),
        repeat_inputs: None,
    };

    // Corsa, unlike Strada, can use multiple threads for emit. As a result, the order of outputs is non-deterministic.
    // To make the order deterministic, we sort the outputs by the order of the inputs.
    let mut js: IndexMap<String, TestFile> = IndexMap::new();
    let mut dts: IndexMap<String, TestFile> = IndexMap::new();
    let mut maps: IndexMap<String, TestFile> = IndexMap::new();
    for document in recorded_outputs {
        if has_js_file_extension(&document.unit_name)
            || has_json_file_extension(&document.unit_name)
        {
            js.insert(document.unit_name.clone(), document);
        } else if is_declaration_file_name(&document.unit_name) {
            dts.insert(document.unit_name.clone(), document);
        } else if file_extension_is(&document.unit_name, ".map") {
            maps.insert(document.unit_name.clone(), document);
        }
    }

    // using the order from the inputs, populate the outputs
    for source_file in tsprogram::source_files() {
        let file_name = source_file_file_name(source_file).to_string();
        let input = TestFile {
            unit_name: file_name.clone(),
            content: source_file_text(source_file).to_string(),
        };
        c.inputs.push(input);
        if !is_declaration_file_name(&file_name) {
            let extname = get_output_extension(&file_name, c.options.jsx);
            let js_output = js.get(&c.get_output_path(&file_name, extname)).cloned();
            let dts_output = dts
                .get(&c.get_output_path(
                    &file_name,
                    &get_declaration_emit_extension_for_path(&file_name),
                ))
                .cloned();
            let map_output = maps
                .get(&c.get_output_path(&file_name, &format!("{extname}.map")))
                .cloned();
            if let Some(output) = js_output {
                c.js.insert(output.unit_name.clone(), output.clone());
                js.shift_remove(&output.unit_name);
                c.outputs.push(output);
            }
            if let Some(output) = dts_output {
                c.dts.insert(output.unit_name.clone(), output.clone());
                dts.shift_remove(&output.unit_name);
                c.outputs.push(output);
            }
            if let Some(output) = map_output {
                c.maps.insert(output.unit_name.clone(), output.clone());
                maps.shift_remove(&output.unit_name);
                c.outputs.push(output);
            }
        }
    }

    // add any unhandled outputs, ordered by unit name
    for (target, rest) in [(&mut c.js, js), (&mut c.dts, dts), (&mut c.maps, maps)] {
        let mut rest: Vec<TestFile> = rest.into_values().collect();
        rest.sort_by(|a, b| compare_go_bytes(&a.unit_name, &b.unit_name));
        for document in rest {
            target.insert(document.unit_name.clone(), document);
        }
    }

    c
}

impl CompilationResult {
    // Go: harnessutil.go:794 getOutputPath
    fn get_output_path(&self, path: &str, ext: &str) -> String {
        let mut path = resolve_path(&self.current_directory, &[path]);
        let out_dir = if ext == ".d.ts"
            || ext == ".d.mts"
            || ext == ".d.cts"
            || (ext.ends_with(".ts") && ext.contains(".d."))
        {
            if self.options.declaration_dir.is_empty() {
                &self.options.out_dir
            } else {
                &self.options.declaration_dir
            }
        } else {
            &self.options.out_dir
        };
        if !out_dir.is_empty() {
            let common = {
                let _scope = self.enter_program_only();
                tsprogram::common_source_directory().to_string()
            };
            if !common.is_empty() {
                path = get_relative_path_from_directory(
                    &common,
                    &path,
                    &ComparePathsOptions {
                        use_case_sensitive_file_names: self.use_case_sensitive_file_names,
                        current_directory: self.current_directory.clone(),
                    },
                );
                path = combine_paths(
                    &resolve_path(&self.current_directory, &[&self.options.out_dir]),
                    &[&path],
                );
            }
        }
        if ext == get_declaration_emit_extension_for_path(&path) {
            return self.change_to_declaration_extension(&path);
        }
        change_extension(&path, ext)
    }

    /// Go `outputpaths.ChangeToDeclarationExtension(path, c.Program.Program())`
    /// (tsgo#4712).
    pub fn change_to_declaration_extension(&self, path: &str) -> String {
        let _scope = self.enter_program_only();
        let program =
            tsprogram::go_frontend_program().expect("a harness program has a Go frontend");
        ts_goport::frontend::outputpaths::change_to_declaration_extension(path, &*program)
    }

    /// Go `c.Program.GetSourceFile(fileName).ContentMapper()`, or `None`
    /// when the program has no such file (tsgo#4712).
    pub fn source_file_content_mapper(&self, file_name: &str) -> Option<String> {
        let _scope = self.enter_program_only();
        let file = tsprogram::get_source_file(file_name);
        file.is_some()
            .then(|| ts_goport::ast::source_file_content_mapper(file).to_string())
    }

    /// Go `core.Some(c.Program.GetSourceFiles(), func(file) bool { return
    /// file.ContentMapper() != "" })` (tsgo#4712).
    pub fn has_content_mapped_source_files(&self) -> bool {
        let _scope = self.enter_program_only();
        tsprogram::source_files()
            .into_iter()
            .any(|file| !ts_goport::ast::source_file_content_mapper(file).is_empty())
    }

    /// The files of Go `c.Program.GetSourceFiles()` that a content mapper
    /// made, with that mapper: Go `c.Program.Program().GetContentMapper(file)`
    /// is not nil (tsgo#4712). In program order.
    pub fn content_mapped_source_files(&self) -> Vec<(Node, Rc<Mapper>)> {
        let _scope = self.enter_program_only();
        tsprogram::source_files()
            .into_iter()
            .filter_map(|file| self.get_content_mapper(file).map(|mapper| (file, mapper)))
            .collect()
    }

    // Go: compiler/program.go:497 (*Program).GetContentMapper (tsgo#4712)
    // PORT: in Go only the compiler runner and the content mapper baseline
    // call it, so the port keeps it with them. `p.opts.Config` is the
    // command line that the program was made with.
    fn get_content_mapper(&self, file: Node) -> Option<Rc<Mapper>> {
        let content_mapper = ts_goport::ast::source_file_content_mapper(file);
        if content_mapper.is_empty() {
            return None;
        }
        let mapper = self
            .command_line
            .get_content_mapper_for_file_name(source_file_file_name(file))?;
        (mapper.identity() == content_mapper).then_some(mapper)
    }

    /// Go `file.Content = sf.Text()` in `newCompilerTest` (tsgo#4712). Go
    /// changes the `*TestFile` values that `CompileFiles` got, so its
    /// `Repeat` closure compiles the new content. This gives `repeat` the
    /// same files.
    pub fn set_repeat_files(&mut self, input_files: &[TestFile], other_files: &[TestFile]) {
        if let Some(inputs) = self.repeat_inputs.as_mut() {
            inputs.input_files = input_files.to_vec();
            inputs.other_files = other_files.to_vec();
        }
    }

    /// Makes the program current on this thread and its file system the
    /// shared one, until the scope drops.
    pub fn enter(&self) -> ProgramScope {
        global_fs().import_state(self.fs_state.clone());
        enter_program(Some(self.program))
    }

    /// Makes the program current without touching the file system.
    fn enter_program_only(&self) -> ProgramScope {
        enter_program(Some(self.program))
    }

    /// Go `Program.CommonSourceDirectory()`.
    pub fn common_source_directory(&self) -> String {
        let _scope = self.enter_program_only();
        tsprogram::common_source_directory().to_string()
    }

    /// Go `Program.GetSourceFile(fileName)` is not nil.
    pub fn has_source_file(&self, file_name: &str) -> bool {
        let _scope = self.enter_program_only();
        tsprogram::get_source_file(file_name).is_some()
    }

    /// Go `Program.GetSourceFile(fileName).FileName()`, or `None`.
    pub fn source_file_name(&self, file_name: &str) -> Option<String> {
        let _scope = self.enter_program_only();
        let file = tsprogram::get_source_file(file_name);
        file.is_some()
            .then(|| source_file_file_name(file).to_string())
    }

    /// Go `Program.GetSourceFile(fileName).Text()`, or `None`.
    pub fn source_file_text(&self, file_name: &str) -> Option<String> {
        let _scope = self.enter_program_only();
        let file = tsprogram::get_source_file(file_name);
        file.is_some().then(|| source_file_text(file).to_string())
    }

    /// Go `Program.GetSourceFile(fileName).OriginalText()`, or `None`
    /// (ts#63936).
    pub fn source_file_original_text(&self, file_name: &str) -> Option<String> {
        let _scope = self.enter_program_only();
        let file = tsprogram::get_source_file(file_name);
        file.is_some()
            .then(|| ts_goport::ast::source_file_original_text(file).to_string())
    }

    /// Go `result.Repeat(testConfig)`.
    pub fn repeat(&self, test_config: &TestConfiguration) -> CompilationResult {
        let inputs = self
            .repeat_inputs
            .as_ref()
            .expect("a CompileFilesEx result can repeat");
        let mut new_harness_options = inputs.harness_options.clone();
        let mut new_compiler_options = inputs.compiler_options.clone();
        set_options_from_test_config(
            test_config,
            &mut new_compiler_options,
            &mut new_harness_options,
            &inputs.current_directory,
            false, /*allowUnknownOptions*/
        );
        compile_files_ex(
            &inputs.input_files,
            &inputs.other_files,
            &new_harness_options,
            &mut new_compiler_options,
            &inputs.current_directory,
            &inputs.symlinks,
            inputs.tsconfig.as_ref(),
        )
    }

    // Go: harnessutil.go:822 GetNumberOfJSFiles
    pub fn get_number_of_js_files(&self, include_json: bool) -> usize {
        if include_json {
            return self.js.len();
        }
        self.js
            .values()
            .filter(|file| !file_extension_is(&file.unit_name, EXTENSION_JSON))
            .count()
    }

    // Go: harnessutil.go:835 Inputs
    pub fn inputs(&self) -> &[TestFile] {
        &self.inputs
    }

    // Go: harnessutil.go:839 Outputs
    pub fn outputs(&self) -> &[TestFile] {
        &self.outputs
    }
}

// Go: harnessutil.go:907 testBuildInfoReader
struct TestBuildInfoReader {
    inner: Rc<dyn BuildInfoReader>,
}

impl BuildInfoReader for TestBuildInfoReader {
    // Go: harnessutil.go:911 ReadBuildInfo
    fn read_build_info(&self, config: &ParsedCommandLine) -> Option<BuildInfo> {
        let mut r = self.inner.read_build_info(config)?;
        r.version = version().to_string();
        Some(r)
    }
}

// Go: harnessutil.go:924 createProgram
fn create_program(host: Rc<dyn CompilerHost>, config: Rc<ParsedCommandLine>) -> ProgramLike {
    // Go: testutil.TestProgramIsSingleThreaded() is true unless
    // TS_TEST_PROGRAM_SINGLE_THREADED says otherwise or the race detector runs.
    let single_threaded = if test_program_is_single_threaded() {
        Tristate::True
    } else {
        Tristate::Unknown
    };

    let program_options = ProgramOptions {
        config: config.clone(),
        host: host.clone(),
        single_threaded,
        use_source_of_project_reference: false,
        typings_location: String::new(),
        project_name: String::new(),
        create_module_resolver: None,
        skip_module_resolution: false,
    };
    // PORT: the frontend parses with no current program; the result is a
    // program version (see the module comment).
    let np = {
        let _scope = enter_program(None);
        Rc::new(new_program(program_options))
    };
    let program = tsprogram::new_program_version(&np, None);
    if config.compiler_options().incremental.is_true() {
        let _scope = enter_program(Some(program));
        let reader = TestBuildInfoReader {
            inner: new_build_info_reader(host.clone()),
        };
        let old_program = read_build_info_program(&config, &reader, &*host);
        let incremental_program =
            new_incremental_program(old_program.as_ref(), create_host(host), None, false);
        return ProgramLike::Incremental(Box::new(incremental_program));
    }
    ProgramLike::Program(program)
}

// Go: testutil.go:37 testProgramIsSingleThreaded
fn test_program_is_single_threaded() -> bool {
    // Leave Program in SingleThreaded mode unless explicitly configured or in race mode.
    if let Ok(v) = std::env::var("TS_TEST_PROGRAM_SINGLE_THREADED")
        && !v.is_empty()
    {
        match v.as_str() {
            "1" | "t" | "T" | "TRUE" | "true" | "True" => return true,
            "0" | "f" | "F" | "FALSE" | "false" | "False" => return false,
            _ => {}
        }
    }
    true
}

// Go: harnessutil.go:944 EnumerateFiles
pub fn enumerate_files(
    folder: &str,
    test_regex: fn(&str) -> bool,
    recursive: bool,
) -> std::io::Result<Vec<String>> {
    let files = list_files_worker(test_regex, recursive, folder)?;
    Ok(files.iter().map(|path| normalize_slashes(path)).collect())
}

// Go: harnessutil.go:956 listFilesWorker
fn list_files_worker(
    spec: fn(&str) -> bool,
    recursive: bool,
    folder: &str,
) -> std::io::Result<Vec<String>> {
    let folder = get_normalized_absolute_path(
        folder,
        &crate::support::baseline::test_data_path().to_string_lossy(),
    );
    let mut entries: Vec<std::fs::DirEntry> =
        std::fs::read_dir(&folder)?.collect::<Result<_, _>>()?;
    // Go `os.ReadDir` sorts by file name.
    entries.sort_by_key(std::fs::DirEntry::file_name);
    let mut paths = Vec::new();
    for entry in entries {
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = normalize_path(&format!("{folder}/{name}"));
        if !entry.file_type()?.is_dir() {
            if spec(&path) {
                paths.push(path);
            }
        } else if recursive {
            paths.extend(list_files_worker(spec, recursive, &path)?);
        }
    }
    Ok(paths)
}

// Go: harnessutil.go:980 getFileBasedTestConfigurationDescription
fn get_file_based_test_configuration_description(config: &TestConfiguration) -> String {
    let mut output = String::new();
    // Go sorts the keys by bytes; `BTreeMap` keys are in that order.
    for (i, (key, value)) in config.iter().enumerate() {
        if i > 0 {
            output.push(',');
        }
        output.push_str(&format!("{key}={}", value.to_lowercase()));
    }
    output
}

// Go: harnessutil.go:992 GetFileBasedTestConfigurations
// PORT: Go walks the settings map in random order; the configurations
// (and their names) do not depend on it. This walks in key order.
pub fn get_file_based_test_configurations(
    settings: &BTreeMap<String, String>,
    vary_by_options: &std::collections::HashSet<String>,
) -> Vec<NamedTestConfiguration> {
    // Each element slice has the option name as the first element, and the values as the rest
    let mut option_entries: Vec<Vec<String>> = Vec::new();
    let mut variation_count = 1usize;
    let mut non_varying_options: TestConfiguration = TestConfiguration::new();
    for (option, value) in settings {
        if vary_by_options.contains(option) {
            let entries = split_option_values(value, option);
            if entries.len() > 1 {
                variation_count *= entries.len();
                if variation_count > 25 {
                    fatal("Provided test options exceeded the maximum number of variations".into());
                }
                let mut entry = vec![option.clone()];
                entry.extend(entries);
                option_entries.push(entry);
            } else if entries.len() == 1 {
                non_varying_options.insert(option.clone(), entries[0].clone());
            }
        } else {
            // Variation is not supported for the option
            non_varying_options.insert(option.clone(), value.clone());
        }
    }

    let mut configurations = Vec::new();
    if !option_entries.is_empty() {
        // Merge varying and non-varying options
        let varying_configurations =
            compute_file_based_test_configuration_variations(variation_count, &option_entries);
        for mut varying_config in varying_configurations {
            let description = get_file_based_test_configuration_description(&varying_config);
            for (key, value) in &non_varying_options {
                varying_config.insert(key.clone(), value.clone());
            }
            configurations.push(NamedTestConfiguration {
                name: description,
                config: varying_config,
            });
        }
    } else if !non_varying_options.is_empty() {
        // Only non-varying options
        configurations.push(NamedTestConfiguration {
            name: String::new(),
            config: non_varying_options,
        });
    }
    configurations
}

// Go: harnessutil.go:1039 splitOptionValues
// Splits a string value into an array of strings, each corresponding to a unique value for the given option.
// Also handles the `*` value, which includes all possible values for the option, and exclusions using `-` or `!`.
// PORT: Go collects the map values in random order; only the set matters.
fn split_option_values(value: &str, option: &str) -> Vec<String> {
    if value.is_empty() {
        return Vec::new();
    }

    let mut star = false;
    let mut includes: Vec<&str> = Vec::new();
    let mut excludes: Vec<&str> = Vec::new();
    for s in value.split(',') {
        let s = s.trim();
        if s.is_empty() {
            continue;
        }
        if s == "*" {
            star = true;
        } else if let Some(rest) = s.strip_prefix('-').or_else(|| s.strip_prefix('!')) {
            excludes.push(rest);
        } else {
            includes.push(s);
        }
    }

    if includes.is_empty() && !star && excludes.is_empty() {
        return Vec::new();
    }

    // Dedupe the variations by their normalized values
    let mut variations: Vec<(CompilerOptionsValue, String)> = Vec::new();
    let mut add = |value: CompilerOptionsValue, include: &str| {
        if !variations.iter().any(|(v, _)| *v == value) {
            variations.push((value, include.to_string()));
        }
    };

    // add (and deduplicate) all included entries
    for include in &includes {
        let value = get_value_of_option_string(option, include);
        add(value, include);
    }

    let all_values = get_all_values_for_option(option);
    if star && !all_values.is_empty() {
        // add all entries
        for include in &all_values {
            let value = get_value_of_option_string(option, include);
            add(value, include);
        }
    }

    // remove all excluded entries
    for exclude in &excludes {
        let Some(value) = try_get_value_of_option_string(option, exclude) else {
            // The excluded value is not recognized (e.g., a removed option like "es3").
            // Just skip it since there's nothing to remove.
            continue;
        };
        variations.retain(|(v, _)| *v != value);
    }

    if variations.is_empty() {
        panic!("Variations in test option '@{option}' resulted in an empty set.");
    }
    variations.into_iter().map(|(_, include)| include).collect()
}

// Go: harnessutil.go:1104 getValueOfOptionString
fn get_value_of_option_string(option: &str, value: &str) -> CompilerOptionsValue {
    match try_get_value_of_option_string(option, value) {
        Some(result) => result,
        None => fatal(format!("Unknown value '{value}' for option '{option}'")),
    }
}

// Go: harnessutil.go:1112 tryGetValueOfOptionString
fn try_get_value_of_option_string(option: &str, value: &str) -> Option<CompilerOptionsValue> {
    let option_decl = get_command_line_option(option)?;
    match option_decl.kind {
        CommandLineOptionKind::ENUM => option_decl
            .enum_map()
            .and_then(|enum_map| enum_map.get(&value.to_lowercase()))
            .cloned(),
        CommandLineOptionKind::BOOLEAN => match value.to_lowercase().as_str() {
            "true" => Some(CompilerOptionsValue::Bool(true)),
            "false" => Some(CompilerOptionsValue::Bool(false)),
            _ => None,
        },
        _ => Some(CompilerOptionsValue::String(value.to_string())),
    }
}

// Go: harnessutil.go:1136 getCommandLineOption
fn get_command_line_option(option: &str) -> Option<&'static CommandLineOption> {
    compiler_options_declarations()
        .iter()
        .copied()
        .find(|option_decl| option_decl.name.eq_ignore_ascii_case(option))
}

// Go: harnessutil.go:1142 getAllValuesForOption
fn get_all_values_for_option(option: &str) -> Vec<String> {
    let Some(option_decl) = get_command_line_option(option) else {
        return Vec::new();
    };
    match option_decl.kind {
        CommandLineOptionKind::ENUM => option_decl
            .enum_map()
            .map(|enum_map| enum_map.keys().cloned().collect())
            .unwrap_or_default(),
        CommandLineOptionKind::BOOLEAN => vec!["true".to_string(), "false".to_string()],
        _ => Vec::new(),
    }
}

// Go: harnessutil.go:1156 computeFileBasedTestConfigurationVariations
fn compute_file_based_test_configuration_variations(
    variation_count: usize,
    option_entries: &[Vec<String>],
) -> Vec<TestConfiguration> {
    let mut configurations = Vec::with_capacity(variation_count);
    compute_file_based_test_configuration_variations_worker(
        &mut configurations,
        option_entries,
        0,
        &mut TestConfiguration::new(),
    );
    configurations
}

// Go: harnessutil.go:1162 computeFileBasedTestConfigurationVariationsWorker
fn compute_file_based_test_configuration_variations_worker(
    configurations: &mut Vec<TestConfiguration>,
    option_entries: &[Vec<String>],
    index: usize,
    variation_state: &mut TestConfiguration,
) {
    if index >= option_entries.len() {
        configurations.push(variation_state.clone());
        return;
    }

    let option_key = &option_entries[index][0];
    let entries = &option_entries[index][1..];
    for entry in entries {
        // set or overwrite the variation, then compute the next variation
        variation_state.insert(option_key.clone(), entry.clone());
        compute_file_based_test_configuration_variations_worker(
            configurations,
            option_entries,
            index + 1,
            variation_state,
        );
    }
}

// Go: harnessutil.go:1182 GetConfigNameFromFileName
pub fn get_config_name_from_file_name(filename: &str) -> String {
    let basename_lower = get_base_file_name(filename).to_lowercase();
    if basename_lower == "tsconfig.json" || basename_lower == "jsconfig.json" {
        return basename_lower;
    }
    String::new()
}

/// A configuration that Go `SkipUnsupportedCompilerOptions` does not
/// accept, with the Go message.
pub enum UnsupportedCompilerOptions {
    /// Go `t.Fatalf` of `failOnUnsupportedCompilerOptions` (ts#64122).
    Fail(String),
    /// Go `t.Skipf`.
    Skip(String),
}

// Go: harnessutil.go:1236 SkipUnsupportedCompilerOptions
// PORT: returns the Go `t.Fatalf` or `t.Skipf` message instead of failing
// or skipping.
pub fn skip_unsupported_compiler_options(
    options: &CompilerOptions,
) -> Option<UnsupportedCompilerOptions> {
    if let Some(message) = fail_on_unsupported_compiler_options(options) {
        return Some(UnsupportedCompilerOptions::Fail(message));
    }
    let skip = |message: String| Some(UnsupportedCompilerOptions::Skip(message));
    if matches!(options.module, ModuleKind::UMD | ModuleKind::SYSTEM) {
        return skip(format!("unsupported module kind {}", options.module));
    }
    if matches!(
        options.module_resolution,
        ModuleResolutionKind::NODE10 | ModuleResolutionKind::CLASSIC
    ) {
        return skip(format!(
            "unsupported module resolution kind {}",
            options.module_resolution.0
        ));
    }
    if options.es_module_interop.is_false() {
        return skip("esModuleInterop=false is unsupported".to_string());
    }
    if options.allow_synthetic_default_imports.is_false() {
        return skip("allowSyntheticDefaultImports=false is unsupported".to_string());
    }
    if !options.base_url.is_empty() {
        return skip(format!("unsupported baseUrl {}", options.base_url));
    }
    if options.target == ScriptTarget::ES5 {
        return skip(format!("unsupported target {}", options.target.string()));
    }
    if options.always_strict.is_false() {
        return skip("alwaysStrict=false is unsupported".to_string());
    }
    None
}

// Go: harnessutil.go:1265 failOnUnsupportedCompilerOptions (ts#64122)
fn fail_on_unsupported_compiler_options(options: &CompilerOptions) -> Option<String> {
    if options.module == ModuleKind::AMD {
        return Some(format!("unsupported module kind {}", options.module));
    }
    if !options.out_file.is_empty() {
        return Some(format!("unsupported outFile {}", options.out_file));
    }
    None
}

// ---------------------------------------------------------------------------
// The shared file system and the output recorder
// ---------------------------------------------------------------------------

static GLOBAL_FS: OnceLock<MapFs> = OnceLock::new();

/// The map file system that the OS override shows. The first compilation
/// makes it and installs the override.
pub fn global_fs() -> &'static MapFs {
    GLOBAL_FS
        .get()
        .expect("the compiler runner file system is installed by the first compilation")
}

/// Makes `fs` the files of the shared file system. The first call installs
/// the OS override with `current_directory`. Every compilation of a test
/// process has the same current directory and case sensitivity.
fn install_global_fs(fs: &MapFs, current_directory: &str, use_case_sensitive_file_names: bool) {
    static INSTALLED: OnceLock<(String, bool)> = OnceLock::new();
    let first = INSTALLED.get_or_init(|| {
        let global = MapFs::from_map(
            BTreeMap::<String, MapFile>::new(),
            use_case_sensitive_file_names,
        );
        assert!(GLOBAL_FS.set(global).is_ok(), "GLOBAL_FS set twice");
        install_os_override(OsOverride {
            fs: Arc::new(|| -> Rc<dyn Fs> { global_fs().fs() }),
            current_directory: current_directory.to_string(),
        });
        (current_directory.to_string(), use_case_sensitive_file_names)
    });
    assert!(
        first.0 == current_directory && first.1 == use_case_sensitive_file_names,
        "compiler runner: one test process has one current directory and case sensitivity"
    );
    global_fs().import_state(fs.export_state());
}

// Go: recorderfs.go:10 OutputRecorderFS
// PORT: emit writes through the `WriteFile` callback (see
// `create_compiler_host`). The callback runs on the checker threads, so it
// writes through `osvfs_fs()`, which the override makes a view of
// `GLOBAL_FS`.
#[derive(Clone, Default)]
pub struct OutputRecorder {
    inner: Arc<Mutex<RecordedOutputs>>,
}

#[derive(Default)]
struct RecordedOutputs {
    outputs_map: BTreeMap<String, usize>,
    outputs: Vec<TestFile>,
}

impl OutputRecorder {
    // Go: recorderfs.go:21 WriteFile
    fn write_file(&self) -> WriteFile {
        let inner = self.inner.clone();
        Arc::new(
            move |path: &str, data: &str, _data: &mut WriteFileData| -> Result<(), String> {
                let fs = osvfs_fs();
                fs.write_file(path, data)
                    .map_err(|err| crate::support::iovfs::fs_error_text(&err))?;
                let path = fs.realpath(path);
                let mut recorded = inner
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let file = TestFile {
                    unit_name: path.clone(),
                    content: data.to_string(),
                };
                if let Some(&index) = recorded.outputs_map.get(&path) {
                    recorded.outputs[index] = file;
                } else {
                    let index = recorded.outputs.len();
                    recorded.outputs_map.insert(path, index);
                    recorded.outputs.push(file);
                }
                Ok(())
            },
        )
    }

    // Go: recorderfs.go:41 Outputs
    fn outputs(&self) -> Vec<TestFile> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .outputs
            .clone()
    }
}
