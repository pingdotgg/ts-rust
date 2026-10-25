//! Rust port of `internal/tsoptions/tsoptionstest` (vfsparseconfighost.go,
//! parsedcommandline.go) and `internal/tsoptions/export_test.go`, plus the
//! test support that the other tsoptions test files share.
//!
//! PORT: Go `export_test.go` is in package `tsoptions`, so it can build the
//! unexported `commandLineParser`. The Rust `CommandLineParser` fields are
//! `pub`, so this file builds it from the public items.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use ts_goport::execute::build::command_line::BuildOptions;
use ts_goport::execute::incremental::build_info::marshal_any;
use ts_goport::execute::tsc::compile::{Writer, write_str};
use ts_goport::execute::tsc::diagnostics::{
    FormattingOptions, format_diagnostic_with_color_and_context, write_format_diagnostics_to,
};
use ts_goport::frontend::json::{JsonError, MarshalerTo, json_marshal, json_marshal_indent};
use ts_goport::frontend::prelude::*;

use crate::support::baseline;
use crate::support::vfstest::{self, MapFile};

// ---------------------------------------------------------------------------
// tsoptionstest/vfsparseconfighost.go
// ---------------------------------------------------------------------------

// Go: tsoptionstest/vfsparseconfighost.go:10 fixRoot
// PORT: unused in the pinned Go source too; kept so the file is complete.
pub fn fix_root(path: &str) -> String {
    let root_length = get_root_length(path);
    if root_length == 0 {
        return path.to_string();
    }
    if path.len() == root_length {
        return ".".to_string();
    }
    path[root_length..].to_string()
}

// Go: tsoptionstest/vfsparseconfighost.go:21 VfsParseConfigHost
pub struct VfsParseConfigHost {
    pub vfs: Rc<dyn Fs>,
    pub current_directory: String,
}

impl ParseConfigHost for VfsParseConfigHost {
    // Go: tsoptionstest/vfsparseconfighost.go:28 (*VfsParseConfigHost).FS
    fn fs(&self) -> Rc<dyn Fs> {
        Rc::clone(&self.vfs)
    }

    // Go: tsoptionstest/vfsparseconfighost.go:32 (*VfsParseConfigHost).GetCurrentDirectory
    fn get_current_directory(&self) -> String {
        self.current_directory.clone()
    }
}

// Go: tsoptionstest/vfsparseconfighost.go:36 NewVFSParseConfigHost
pub fn new_vfs_parse_config_host(
    files: &BTreeMap<String, String>,
    current_directory: &str,
    use_case_sensitive_file_names: bool,
) -> VfsParseConfigHost {
    VfsParseConfigHost {
        vfs: vfs_from_map(files, use_case_sensitive_file_names),
        current_directory: current_directory.to_string(),
    }
}

// Go: tsoptionstest/vfsparseconfighost.go:45 NewVFSParseConfigHostWithSymlinks (tsgo#4712)
// NewVFSParseConfigHostWithSymlinks builds a parse-config host whose vfs also contains the given symlinks
// (link path -> target path), so config parsing resolves packages through symlinks as it would on disk.
pub fn new_vfs_parse_config_host_with_symlinks(
    files: &BTreeMap<String, String>,
    symlinks: &BTreeMap<String, String>,
    current_directory: &str,
    use_case_sensitive_file_names: bool,
) -> VfsParseConfigHost {
    if symlinks.is_empty() {
        return new_vfs_parse_config_host(files, current_directory, use_case_sensitive_file_names);
    }
    let mut entries: BTreeMap<String, MapFile> = files
        .iter()
        .map(|(name, content)| (name.clone(), MapFile::from(content.as_str())))
        .collect();
    for (link, target) in symlinks {
        entries.insert(
            get_normalized_absolute_path(link, current_directory),
            vfstest::symlink(&get_normalized_absolute_path(target, current_directory)),
        );
    }
    VfsParseConfigHost {
        vfs: vfstest::from_map(entries, use_case_sensitive_file_names),
        current_directory: current_directory.to_string(),
    }
}

// ---------------------------------------------------------------------------
// tsoptionstest/parsedcommandline.go
// ---------------------------------------------------------------------------

// Go: tsoptionstest/parsedcommandline.go:9 GetParsedCommandLine
// PORT: the Go `t assert.TestingT` parameter is not used, so it is dropped.
pub fn get_parsed_command_line(
    json_text: &str,
    files: &BTreeMap<String, String>,
    current_directory: &str,
    use_case_sensitive_file_names: bool,
) -> ParsedCommandLine {
    let host = new_vfs_parse_config_host(files, current_directory, use_case_sensitive_file_names);
    let config_file_name = combine_paths(current_directory, &["tsconfig.json"]);
    let tsconfig_source_file = new_tsconfig_source_file_from_file_path(
        &config_file_name,
        to_path(
            &config_file_name,
            current_directory,
            use_case_sensitive_file_names,
        ),
        json_text,
    );
    parse_json_source_file_config_file_content(
        tsconfig_source_file,
        &host,
        current_directory,
        None,
        None,
        &config_file_name,
        &[],
        None,
    )
}

// ---------------------------------------------------------------------------
// export_test.go
// ---------------------------------------------------------------------------

// Go: export_test.go:9 getTestParseCommandLineWorkerDiagnostics
// PORT: `ParseCommandLineWorkerDiagnostics` values are `&'static`, as the
// Go package-level pointers are. A custom list gets a leaked value.
pub fn get_test_parse_command_line_worker_diagnostics(
    decls: &'static [&'static CommandLineOption],
) -> &'static ParseCommandLineWorkerDiagnostics {
    if decls.is_empty() {
        return &COMPILER_OPTIONS_DID_YOU_MEAN_DIAGNOSTICS;
    }
    Box::leak(Box::new(get_parse_command_line_worker_diagnostics(decls)))
}

// Go: export_test.go:16 ParseCommandLineTestWorker
// PORT: Go nil `decls` is an empty slice. The Rust parser does not store the
// file system (see `CommandLineParser`); it is passed to `parse_strings`.
pub fn parse_command_line_test_worker(
    decls: &'static [&'static CommandLineOption],
    command_line: &[String],
    fs: Rc<dyn Fs>,
    current_directory: &str,
) -> TestCommandLineParser {
    let mut parser = CommandLineParser {
        current_directory: current_directory.to_string(),
        worker_diagnostics: &COMPILER_OPTIONS_DID_YOU_MEAN_DIAGNOSTICS,
        file_names: Vec::new(),
        options: IndexMap::default(),
        errors: Vec::new(),
        options_map: NameMap::default(),
        response_file_stack: FxHashSet::default(),
    };
    if !decls.is_empty() {
        parser.worker_diagnostics = get_test_parse_command_line_worker_diagnostics(decls);
    }

    parser.options_map = get_name_map_from_list(parser.options_declarations());
    parser.parse_strings(command_line, Some(&*fs));
    TestCommandLineParser {
        fs,
        worker_diagnostics: parser.worker_diagnostics,
        file_names: parser.file_names,
        options: parser.options,
        errors: parser.errors,
    }
}

// Go: export_test.go:45 TestCommandLineParser
pub struct TestCommandLineParser {
    pub fs: Rc<dyn Fs>,
    pub worker_diagnostics: &'static ParseCommandLineWorkerDiagnostics,
    pub file_names: Vec<String>,
    pub options: IndexMap<String, CompilerOptionsValue>,
    pub errors: Vec<Diagnostic>,
}

// ---------------------------------------------------------------------------
// Test support shared by the tsoptions test files. These pieces stand in for
// Go library behavior (map literals, `vfstest.FromMap`, `t.Run`, `t.TempDir`,
// `repo`, reflection-based JSON and `diagnosticwriter` plural writers).
// ---------------------------------------------------------------------------

/// Go `map[string]string{...}` literal of file paths to file text.
pub fn file_map(entries: &[(&str, &str)]) -> BTreeMap<String, String> {
    entries
        .iter()
        .map(|(path, text)| ((*path).to_string(), (*text).to_string()))
        .collect()
}

/// Go `vfstest.FromMap(files, useCaseSensitiveFileNames)` for a
/// `map[string]string`.
pub fn vfs_from_map(
    files: &BTreeMap<String, String>,
    use_case_sensitive_file_names: bool,
) -> Rc<dyn Fs> {
    let files: BTreeMap<String, MapFile> = files
        .iter()
        .map(|(path, text)| (path.clone(), MapFile::from(text.as_str())))
        .collect();
    vfstest::from_map(files, use_case_sensitive_file_names)
}

/// Go `[]string{...}` of command line arguments.
pub fn strs(args: &[&str]) -> Vec<String> {
    args.iter().map(|arg| (*arg).to_string()).collect()
}

/// Go `t.Run` for the ported tests. Each subtest runs to its end; an `Err`
/// (Go `t.Errorf`) or a panic (Go `t.Fatal` and the fatal `assert.*` calls,
/// ported as `assert!`) is recorded, and the next subtest still runs.
/// `finish` panics once with every failure.
pub struct Subtests {
    test: String,
    failures: Vec<String>,
}

impl Subtests {
    pub fn new(test: &str) -> Subtests {
        Subtests {
            test: test.to_string(),
            failures: Vec::new(),
        }
    }

    pub fn run(&mut self, name: &str, f: impl FnOnce() -> Result<(), String>) {
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
            Ok(Ok(())) => {}
            Ok(Err(message)) => self
                .failures
                .push(format!("{}/{name}: {message}", self.test)),
            Err(payload) => {
                let message = if let Some(s) = payload.downcast_ref::<String>() {
                    s.clone()
                } else if let Some(s) = payload.downcast_ref::<&str>() {
                    (*s).to_string()
                } else {
                    "panic".to_string()
                };
                self.failures
                    .push(format!("{}/{name}: panicked: {message}", self.test));
            }
        }
    }

    pub fn finish(self) {
        assert!(
            self.failures.is_empty(),
            "{} subtest(s) of {} failed:\n{}",
            self.failures.len(),
            self.test,
            self.failures.join("\n")
        );
    }
}

/// Go `t.TempDir()`: a new directory under `TSCTEST_TMP` (default
/// `go-baseline-tests/tmp`), removed when the value is dropped.
pub struct TempDir {
    path: PathBuf,
}

#[cfg(not(target_os = "macos"))]
const DEFAULT_TMP: &str =
    "/home/theo/Code/sandbox/ts-rust/target/continuation-r97-goport/go-baseline-tests/tmp";
/// macOS: `/home` is an autofs mount there, so the default is under /tmp.
#[cfg(target_os = "macos")]
const DEFAULT_TMP: &str = "/tmp/ts-rust-go-baseline-tests/tmp";

impl TempDir {
    pub fn new() -> TempDir {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::var_os("TSCTEST_TMP")
            .map_or_else(|| PathBuf::from(DEFAULT_TMP), PathBuf::from);
        let path = root.join(format!(
            "tsoptions-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path)
            .unwrap_or_else(|err| panic!("TempDir: cannot create {}: {err}", path.display()));
        TempDir { path }
    }

    /// The directory as a Go path string.
    pub fn path(&self) -> String {
        normalize_slashes(&self.path.to_string_lossy())
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

// Go: repo/paths.go:50 TypeScriptSubmodulePath
pub fn type_script_submodule_path() -> PathBuf {
    baseline::go_repo().join("_submodules").join("TypeScript")
}

// Go: repo/paths.go:82 SkipIfNoTypeScriptSubmodule
// PORT: Go `t.Skipf`. Returns true when the test should return at once.
pub fn skip_if_no_type_script_submodule(test: &str) -> bool {
    if type_script_submodule_path().join("package.json").exists() {
        return false;
    }
    eprintln!("{test}: skipped: TypeScript submodule does not exist");
    true
}

/// Runs `f` with a `Writer` and returns what it wrote.
pub fn capture_writer(f: impl FnOnce(&Writer)) -> String {
    let buf: Rc<RefCell<Vec<u8>>> = Rc::new(RefCell::new(Vec::new()));
    let writer: Writer = buf.clone();
    f(&writer);
    drop(writer);
    let bytes = buf.borrow().clone();
    String::from_utf8(bytes).expect("writer output is UTF-8")
}

// Go: diagnosticwriter/diagnosticwriter.go:461 WriteFormatDiagnostics
// (with `FromASTDiagnostics`, which adds nothing in the port).
pub fn write_format_diagnostics(
    output: &mut String,
    diagnostics: &[Diagnostic],
    format_opts: &FormattingOptions,
) {
    output.push_str(&capture_writer(|w| {
        write_format_diagnostics_to(w, diagnostics, format_opts);
    }));
}

// Go: diagnosticwriter/diagnosticwriter.go:122 FormatDiagnosticsWithColorAndContext
// PORT: the production port has only the singular writer; this is the Go
// loop over it.
pub fn format_diagnostics_with_color_and_context(
    output: &mut String,
    diags: &[Diagnostic],
    format_opts: &FormattingOptions,
) {
    if diags.is_empty() {
        return;
    }
    output.push_str(&capture_writer(|w| {
        for (i, diagnostic) in diags.iter().enumerate() {
            if i > 0 {
                write_str(w, &format_opts.new_line);
            }
            format_diagnostic_with_color_and_context(w, diagnostic, format_opts);
        }
    }));
}

/// A `FormattingOptions` value like the Go struct literals in the tests.
pub fn formatting_options(
    new_line: &str,
    current_directory: &str,
    use_case_sensitive_file_names: bool,
) -> FormattingOptions {
    FormattingOptions {
        new_line: new_line.to_string(),
        compare_paths_options: ComparePathsOptions {
            current_directory: current_directory.to_string(),
            use_case_sensitive_file_names,
        },
        ..Default::default()
    }
}

// Go: json/json.go:14 Marshal. Go ignores the error in the tests
// (`o, _ := json.Marshal(...)`), which leaves `o` nil.
pub fn marshal_or_empty<T: MarshalerTo + ?Sized>(input: &T) -> String {
    json_marshal(input, &[]).unwrap_or_default()
}

// Go: json/json.go:49 MarshalIndentWrite
pub fn marshal_indent_write<T: MarshalerTo + ?Sized>(
    output: &mut String,
    input: &T,
    prefix: &str,
    indent: &str,
) -> Result<(), JsonError> {
    output.push_str(&json_marshal_indent(input, prefix, indent)?);
    Ok(())
}

/// Go JSON v2 marshaling of an `any` value that tsoptions stores
/// (`*collections.OrderedMap[string, any]` and its members).
pub struct AnyJson<'a>(pub &'a CompilerOptionsValue);

impl MarshalerTo for AnyJson<'_> {
    fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {
        marshal_any(enc, self.0)
    }
}

/// Go JSON v2 marshaling of `*collections.OrderedMap[string, any]`.
pub struct OptionsMapJson<'a>(pub &'a IndexMap<String, CompilerOptionsValue>);

impl MarshalerTo for OptionsMapJson<'_> {
    fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {
        enc.push('{');
        for (i, (key, value)) in self.0.iter().enumerate() {
            if i > 0 {
                enc.push(',');
            }
            key.marshal_json_to(enc)?;
            enc.push(':');
            marshal_any(enc, value)?;
        }
        enc.push('}');
        Ok(())
    }
}

/// Writes the members of a Go struct with `omitzero` fields, in field order.
struct OmitZeroWriter<'a> {
    enc: &'a mut String,
    first: bool,
}

impl<'a> OmitZeroWriter<'a> {
    fn new(enc: &'a mut String) -> OmitZeroWriter<'a> {
        enc.push('{');
        OmitZeroWriter { enc, first: true }
    }

    fn name(&mut self, name: &str) -> Result<(), JsonError> {
        if !self.first {
            self.enc.push(',');
        }
        self.first = false;
        name.marshal_json_to(self.enc)?;
        self.enc.push(':');
        Ok(())
    }

    // Go: core/tristate.go:55 MarshalJSON; the zero value is TSUnknown.
    fn tristate(&mut self, name: &str, v: Tristate) -> Result<(), JsonError> {
        if v == Tristate::Unknown {
            return Ok(());
        }
        self.name(name)?;
        self.enc
            .push_str(std::str::from_utf8(v.marshal_json()).expect("ascii"));
        Ok(())
    }

    fn string(&mut self, name: &str, v: &str) -> Result<(), JsonError> {
        if v.is_empty() {
            return Ok(());
        }
        self.name(name)?;
        v.marshal_json_to(self.enc)
    }

    // Go `[]string`: only a nil slice is zero; an empty slice is `[]`.
    fn strings(&mut self, name: &str, v: Option<&[String]>) -> Result<(), JsonError> {
        let Some(v) = v else {
            return Ok(());
        };
        self.name(name)?;
        v.marshal_json_to(self.enc)
    }

    // Go `int32` enum types (`ModuleKind`, `ScriptTarget`, ...).
    fn int(&mut self, name: &str, v: i64) -> Result<(), JsonError> {
        if v == 0 {
            return Ok(());
        }
        self.name(name)?;
        self.enc.push_str(&v.to_string());
        Ok(())
    }

    // Go `*int`: only nil is zero.
    fn int_ptr(&mut self, name: &str, v: Option<i64>) -> Result<(), JsonError> {
        let Some(v) = v else {
            return Ok(());
        };
        self.name(name)?;
        self.enc.push_str(&v.to_string());
        Ok(())
    }

    // Go slice of structs (`[]PluginImport`): only a nil slice is zero.
    fn opt<T: MarshalerTo>(&mut self, name: &str, v: Option<&T>) -> Result<(), JsonError> {
        let Some(v) = v else {
            return Ok(());
        };
        self.name(name)?;
        v.marshal_json_to(self.enc)
    }

    // Go `*collections.OrderedMap[string, []string]`: only nil is zero.
    fn paths(
        &mut self,
        name: &str,
        v: &Option<IndexMap<String, Option<Vec<String>>>>,
    ) -> Result<(), JsonError> {
        if v.is_none() {
            return Ok(());
        }
        self.name(name)?;
        marshal_any(self.enc, &CompilerOptionsValue::Paths(v.clone()))
    }

    fn end(self) -> Result<(), JsonError> {
        self.enc.push('}');
        Ok(())
    }
}

/// Go `core.CompilerOptions` struct tags: (Go field name, JSON name), in
/// field order. Copied from the pinned core/compileroptions.go. Every tag is
/// `"<name>,omitzero"`.
pub const COMPILER_OPTIONS_JSON_FIELDS: &[(&str, &str)] = &[
    ("AllowJs", "allowJs"),
    ("AllowArbitraryExtensions", "allowArbitraryExtensions"),
    ("AllowImportingTsExtensions", "allowImportingTsExtensions"),
    ("AllowNonTsExtensions", "allowNonTsExtensions"),
    ("AllowUmdGlobalAccess", "allowUmdGlobalAccess"),
    ("AllowUnreachableCode", "allowUnreachableCode"),
    ("AllowUnusedLabels", "allowUnusedLabels"),
    (
        "AssumeChangesOnlyAffectDirectDependencies",
        "assumeChangesOnlyAffectDirectDependencies",
    ),
    ("CheckJs", "checkJs"),
    ("CustomConditions", "customConditions"),
    ("Composite", "composite"),
    ("EmitDeclarationOnly", "emitDeclarationOnly"),
    ("EmitBOM", "emitBOM"),
    ("EmitDecoratorMetadata", "emitDecoratorMetadata"),
    ("Declaration", "declaration"),
    ("DeclarationDir", "declarationDir"),
    ("DeclarationMap", "declarationMap"),
    ("DeduplicatePackages", "deduplicatePackages"),
    ("DisableSizeLimit", "disableSizeLimit"),
    (
        "DisableSourceOfProjectReferenceRedirect",
        "disableSourceOfProjectReferenceRedirect",
    ),
    ("DisableSolutionSearching", "disableSolutionSearching"),
    (
        "DisableReferencedProjectLoad",
        "disableReferencedProjectLoad",
    ),
    ("ErasableSyntaxOnly", "erasableSyntaxOnly"),
    ("ExactOptionalPropertyTypes", "exactOptionalPropertyTypes"),
    ("ExperimentalDecorators", "experimentalDecorators"),
    (
        "ForceConsistentCasingInFileNames",
        "forceConsistentCasingInFileNames",
    ),
    ("IsolatedModules", "isolatedModules"),
    ("IsolatedDeclarations", "isolatedDeclarations"),
    ("IgnoreConfig", "ignoreConfig"),
    ("IgnoreDeprecations", "ignoreDeprecations"),
    ("ImportHelpers", "importHelpers"),
    ("InlineSourceMap", "inlineSourceMap"),
    ("InlineSources", "inlineSources"),
    ("Init", "init"),
    ("Incremental", "incremental"),
    ("Jsx", "jsx"),
    ("JsxFactory", "jsxFactory"),
    ("JsxFragmentFactory", "jsxFragmentFactory"),
    ("JsxImportSource", "jsxImportSource"),
    ("Lib", "lib"),
    ("LibReplacement", "libReplacement"),
    ("Locale", "locale"),
    ("MapRoot", "mapRoot"),
    ("Module", "module"),
    ("ModuleResolution", "moduleResolution"),
    ("ModuleSuffixes", "moduleSuffixes"),
    ("ModuleDetection", "moduleDetection"),
    ("NewLine", "newLine"),
    ("NoEmit", "noEmit"),
    ("NoCheck", "noCheck"),
    ("NoErrorTruncation", "noErrorTruncation"),
    ("NoFallthroughCasesInSwitch", "noFallthroughCasesInSwitch"),
    ("NoImplicitAny", "noImplicitAny"),
    ("NoImplicitThis", "noImplicitThis"),
    ("NoImplicitReturns", "noImplicitReturns"),
    ("NoEmitHelpers", "noEmitHelpers"),
    ("NoLib", "noLib"),
    (
        "NoPropertyAccessFromIndexSignature",
        "noPropertyAccessFromIndexSignature",
    ),
    ("NoUncheckedIndexedAccess", "noUncheckedIndexedAccess"),
    ("NoEmitOnError", "noEmitOnError"),
    ("NoUnusedLocals", "noUnusedLocals"),
    ("NoUnusedParameters", "noUnusedParameters"),
    ("NoResolve", "noResolve"),
    ("NoImplicitOverride", "noImplicitOverride"),
    (
        "NoUncheckedSideEffectImports",
        "noUncheckedSideEffectImports",
    ),
    ("OutDir", "outDir"),
    ("Paths", "paths"),
    ("Plugins", "plugins"),
    ("PreserveConstEnums", "preserveConstEnums"),
    ("PreserveSymlinks", "preserveSymlinks"),
    ("Project", "project"),
    ("ResolveJsonModule", "resolveJsonModule"),
    ("ResolvePackageJsonExports", "resolvePackageJsonExports"),
    ("ResolvePackageJsonImports", "resolvePackageJsonImports"),
    ("RemoveComments", "removeComments"),
    (
        "RewriteRelativeImportExtensions",
        "rewriteRelativeImportExtensions",
    ),
    ("ReactNamespace", "reactNamespace"),
    ("RootDir", "rootDir"),
    ("RootDirs", "rootDirs"),
    ("SkipLibCheck", "skipLibCheck"),
    ("StableTypeOrdering", "stableTypeOrdering"),
    ("Strict", "strict"),
    ("StrictBindCallApply", "strictBindCallApply"),
    ("StrictBuiltinIteratorReturn", "strictBuiltinIteratorReturn"),
    ("StrictFunctionTypes", "strictFunctionTypes"),
    ("StrictNullChecks", "strictNullChecks"),
    (
        "StrictPropertyInitialization",
        "strictPropertyInitialization",
    ),
    ("StripInternal", "stripInternal"),
    ("SkipDefaultLibCheck", "skipDefaultLibCheck"),
    ("SourceMap", "sourceMap"),
    ("SourceRoot", "sourceRoot"),
    ("SuppressOutputPathCheck", "suppressOutputPathCheck"),
    ("Target", "target"),
    ("TraceResolution", "traceResolution"),
    ("TsBuildInfoFile", "tsBuildInfoFile"),
    ("TypeRoots", "typeRoots"),
    ("Types", "types"),
    ("UseDefineForClassFields", "useDefineForClassFields"),
    ("UseUnknownInCatchVariables", "useUnknownInCatchVariables"),
    ("VerbatimModuleSyntax", "verbatimModuleSyntax"),
    ("MaxNodeModuleJsDepth", "maxNodeModuleJsDepth"),
    (
        "AllowSyntheticDefaultImports",
        "allowSyntheticDefaultImports",
    ),
    ("AlwaysStrict", "alwaysStrict"),
    ("BaseUrl", "baseUrl"),
    ("DownlevelIteration", "downlevelIteration"),
    ("ESModuleInterop", "esModuleInterop"),
    ("OutFile", "outFile"),
    ("ConfigFilePath", "configFilePath"),
    ("NoDtsResolution", "noDtsResolution"),
    ("PathsBasePath", "pathsBasePath"),
    ("Diagnostics", "diagnostics"),
    ("ExtendedDiagnostics", "extendedDiagnostics"),
    ("GenerateCpuProfile", "generateCpuProfile"),
    ("GenerateTrace", "generateTrace"),
    ("ListEmittedFiles", "listEmittedFiles"),
    ("ListFiles", "listFiles"),
    ("ExplainFiles", "explainFiles"),
    ("ListFilesOnly", "listFilesOnly"),
    ("NoEmitForJsFiles", "noEmitForJsFiles"),
    ("PreserveWatchOutput", "preserveWatchOutput"),
    ("Pretty", "pretty"),
    ("Version", "version"),
    ("Watch", "watch"),
    ("ShowConfig", "showConfig"),
    ("Build", "build"),
    ("Help", "help"),
    ("All", "all"),
    ("RunExternalCode", "runExternalCode"),
    ("PprofDir", "pprofDir"),
    ("SingleThreaded", "singleThreaded"),
    ("Quiet", "quiet"),
    ("Checkers", "checkers"),
];

/// Go JSON v2 marshaling of `core.CompilerOptions` (struct fields in order,
/// all `omitzero`).
// PORT: Go marshals by reflection. This lists the fields in Go order with
// the tags of `COMPILER_OPTIONS_JSON_FIELDS`.
pub struct CompilerOptionsJson<'a>(pub &'a CompilerOptions);

impl MarshalerTo for CompilerOptionsJson<'_> {
    fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {
        let o = self.0;
        let mut w = OmitZeroWriter::new(enc);
        w.tristate("allowJs", o.allow_js)?;
        w.tristate("allowArbitraryExtensions", o.allow_arbitrary_extensions)?;
        w.tristate(
            "allowImportingTsExtensions",
            o.allow_importing_ts_extensions,
        )?;
        w.tristate("allowNonTsExtensions", o.allow_non_ts_extensions)?;
        w.tristate("allowUmdGlobalAccess", o.allow_umd_global_access)?;
        w.tristate("allowUnreachableCode", o.allow_unreachable_code)?;
        w.tristate("allowUnusedLabels", o.allow_unused_labels)?;
        w.tristate(
            "assumeChangesOnlyAffectDirectDependencies",
            o.assume_changes_only_affect_direct_dependencies,
        )?;
        w.tristate("checkJs", o.check_js)?;
        w.strings("customConditions", o.custom_conditions.as_deref())?;
        w.tristate("composite", o.composite)?;
        w.tristate("emitDeclarationOnly", o.emit_declaration_only)?;
        w.tristate("emitBOM", o.emit_bom)?;
        w.tristate("emitDecoratorMetadata", o.emit_decorator_metadata)?;
        w.tristate("declaration", o.declaration)?;
        w.string("declarationDir", &o.declaration_dir)?;
        w.tristate("declarationMap", o.declaration_map)?;
        w.tristate("deduplicatePackages", o.deduplicate_packages)?;
        w.tristate("disableSizeLimit", o.disable_size_limit)?;
        w.tristate(
            "disableSourceOfProjectReferenceRedirect",
            o.disable_source_of_project_reference_redirect,
        )?;
        w.tristate("disableSolutionSearching", o.disable_solution_searching)?;
        w.tristate(
            "disableReferencedProjectLoad",
            o.disable_referenced_project_load,
        )?;
        w.tristate("erasableSyntaxOnly", o.erasable_syntax_only)?;
        w.tristate(
            "exactOptionalPropertyTypes",
            o.exact_optional_property_types,
        )?;
        w.tristate("experimentalDecorators", o.experimental_decorators)?;
        w.tristate(
            "forceConsistentCasingInFileNames",
            o.force_consistent_casing_in_file_names,
        )?;
        w.tristate("isolatedModules", o.isolated_modules)?;
        w.tristate("isolatedDeclarations", o.isolated_declarations)?;
        w.tristate("ignoreConfig", o.ignore_config)?;
        w.string("ignoreDeprecations", &o.ignore_deprecations)?;
        w.tristate("importHelpers", o.import_helpers)?;
        w.tristate("inlineSourceMap", o.inline_source_map)?;
        w.tristate("inlineSources", o.inline_sources)?;
        w.tristate("init", o.init)?;
        w.tristate("incremental", o.incremental)?;
        w.int("jsx", i64::from(o.jsx.0))?;
        w.string("jsxFactory", &o.jsx_factory)?;
        w.string("jsxFragmentFactory", &o.jsx_fragment_factory)?;
        w.string("jsxImportSource", &o.jsx_import_source)?;
        w.strings("lib", o.lib.as_deref())?;
        w.tristate("libReplacement", o.lib_replacement)?;
        w.string("locale", &o.locale)?;
        w.string("mapRoot", &o.map_root)?;
        w.int("module", i64::from(o.module.0))?;
        w.int("moduleResolution", i64::from(o.module_resolution.0))?;
        w.strings("moduleSuffixes", o.module_suffixes.as_deref())?;
        w.int("moduleDetection", i64::from(o.module_detection.0))?;
        w.int("newLine", i64::from(o.new_line.0))?;
        w.tristate("noEmit", o.no_emit)?;
        w.tristate("noCheck", o.no_check)?;
        w.tristate("noErrorTruncation", o.no_error_truncation)?;
        w.tristate(
            "noFallthroughCasesInSwitch",
            o.no_fallthrough_cases_in_switch,
        )?;
        w.tristate("noImplicitAny", o.no_implicit_any)?;
        w.tristate("noImplicitThis", o.no_implicit_this)?;
        w.tristate("noImplicitReturns", o.no_implicit_returns)?;
        w.tristate("noEmitHelpers", o.no_emit_helpers)?;
        w.tristate("noLib", o.no_lib)?;
        w.tristate(
            "noPropertyAccessFromIndexSignature",
            o.no_property_access_from_index_signature,
        )?;
        w.tristate("noUncheckedIndexedAccess", o.no_unchecked_indexed_access)?;
        w.tristate("noEmitOnError", o.no_emit_on_error)?;
        w.tristate("noUnusedLocals", o.no_unused_locals)?;
        w.tristate("noUnusedParameters", o.no_unused_parameters)?;
        w.tristate("noResolve", o.no_resolve)?;
        w.tristate("noImplicitOverride", o.no_implicit_override)?;
        w.tristate(
            "noUncheckedSideEffectImports",
            o.no_unchecked_side_effect_imports,
        )?;
        w.string("outDir", &o.out_dir)?;
        w.paths("paths", &o.paths)?;
        w.opt("plugins", o.plugins.as_ref())?;
        w.tristate("preserveConstEnums", o.preserve_const_enums)?;
        w.tristate("preserveSymlinks", o.preserve_symlinks)?;
        w.string("project", &o.project)?;
        w.tristate("resolveJsonModule", o.resolve_json_module)?;
        w.tristate("resolvePackageJsonExports", o.resolve_package_json_exports)?;
        w.tristate("resolvePackageJsonImports", o.resolve_package_json_imports)?;
        w.tristate("removeComments", o.remove_comments)?;
        w.tristate(
            "rewriteRelativeImportExtensions",
            o.rewrite_relative_import_extensions,
        )?;
        w.string("reactNamespace", &o.react_namespace)?;
        w.string("rootDir", &o.root_dir)?;
        w.strings("rootDirs", o.root_dirs.as_deref())?;
        w.tristate("skipLibCheck", o.skip_lib_check)?;
        w.tristate("stableTypeOrdering", o.stable_type_ordering)?;
        w.tristate("strict", o.strict)?;
        w.tristate("strictBindCallApply", o.strict_bind_call_apply)?;
        w.tristate(
            "strictBuiltinIteratorReturn",
            o.strict_builtin_iterator_return,
        )?;
        w.tristate("strictFunctionTypes", o.strict_function_types)?;
        w.tristate("strictNullChecks", o.strict_null_checks)?;
        w.tristate(
            "strictPropertyInitialization",
            o.strict_property_initialization,
        )?;
        w.tristate("stripInternal", o.strip_internal)?;
        w.tristate("skipDefaultLibCheck", o.skip_default_lib_check)?;
        w.tristate("sourceMap", o.source_map)?;
        w.string("sourceRoot", &o.source_root)?;
        w.tristate("suppressOutputPathCheck", o.suppress_output_path_check)?;
        w.int("target", i64::from(o.target.0))?;
        w.tristate("traceResolution", o.trace_resolution)?;
        w.string("tsBuildInfoFile", &o.ts_build_info_file)?;
        w.strings("typeRoots", o.type_roots.as_deref())?;
        w.strings("types", o.types.as_deref())?;
        w.tristate("useDefineForClassFields", o.use_define_for_class_fields)?;
        w.tristate(
            "useUnknownInCatchVariables",
            o.use_unknown_in_catch_variables,
        )?;
        w.tristate("verbatimModuleSyntax", o.verbatim_module_syntax)?;
        w.int_ptr("maxNodeModuleJsDepth", o.max_node_module_js_depth)?;
        w.tristate(
            "allowSyntheticDefaultImports",
            o.allow_synthetic_default_imports,
        )?;
        w.tristate("alwaysStrict", o.always_strict)?;
        w.string("baseUrl", &o.base_url)?;
        w.tristate("downlevelIteration", o.downlevel_iteration)?;
        w.tristate("esModuleInterop", o.es_module_interop)?;
        w.string("outFile", &o.out_file)?;
        w.string("configFilePath", &o.config_file_path)?;
        w.tristate("noDtsResolution", o.no_dts_resolution)?;
        w.string("pathsBasePath", &o.paths_base_path)?;
        w.tristate("diagnostics", o.diagnostics)?;
        w.tristate("extendedDiagnostics", o.extended_diagnostics)?;
        w.string("generateCpuProfile", &o.generate_cpu_profile)?;
        w.string("generateTrace", &o.generate_trace)?;
        w.tristate("listEmittedFiles", o.list_emitted_files)?;
        w.tristate("listFiles", o.list_files)?;
        w.tristate("explainFiles", o.explain_files)?;
        w.tristate("listFilesOnly", o.list_files_only)?;
        w.tristate("noEmitForJsFiles", o.no_emit_for_js_files)?;
        w.tristate("preserveWatchOutput", o.preserve_watch_output)?;
        w.tristate("pretty", o.pretty)?;
        w.tristate("version", o.version)?;
        w.tristate("watch", o.watch)?;
        w.tristate("showConfig", o.show_config)?;
        w.tristate("build", o.build)?;
        w.tristate("help", o.help)?;
        w.tristate("all", o.all)?;
        w.tristate("runExternalCode", o.run_external_code)?;
        w.string("pprofDir", &o.pprof_dir)?;
        w.tristate("singleThreaded", o.single_threaded)?;
        w.tristate("quiet", o.quiet)?;
        w.int_ptr("checkers", o.checkers)?;
        w.end()
    }
}

/// Go JSON v2 marshaling of `core.TypeAcquisition`.
// PORT: the Rust `include` and `exclude` are `Vec`, so an empty list is
// written as the nil slice (omitted). No test sets an empty list.
pub struct TypeAcquisitionJson<'a>(pub &'a TypeAcquisition);

impl MarshalerTo for TypeAcquisitionJson<'_> {
    fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {
        let o = self.0;
        let mut w = OmitZeroWriter::new(enc);
        w.tristate("enable", o.enable)?;
        w.strings("include", (!o.include.is_empty()).then_some(&o.include[..]))?;
        w.strings("exclude", (!o.exclude.is_empty()).then_some(&o.exclude[..]))?;
        w.tristate(
            "disableFilenameBasedTypeAcquisition",
            o.disable_filename_based_type_acquisition,
        )?;
        w.end()
    }
}

/// Go JSON v2 marshaling of `core.BuildOptions`.
pub struct BuildOptionsJson<'a>(pub &'a BuildOptions);

impl MarshalerTo for BuildOptionsJson<'_> {
    fn marshal_json_to(&self, enc: &mut String) -> Result<(), JsonError> {
        let o = self.0;
        let mut w = OmitZeroWriter::new(enc);
        w.tristate("dry", o.dry)?;
        w.tristate("force", o.force)?;
        w.tristate("verbose", o.verbose)?;
        w.int_ptr("builders", o.builders)?;
        w.tristate("stopBuildOnErrors", o.stop_build_on_errors)?;
        w.tristate("clean", o.clean)?;
        w.end()
    }
}
