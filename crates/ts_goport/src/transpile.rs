//! Go package `transpile` (#4849): single-file JavaScript and declaration
//! emit.
//!
//! PORT: Go makes a program of one file over a `transpileFS` file system
//! (fs.go, ts#64009) and emits it. Here that program is a program version of the
//! process (`program::new_program_version`, as the compiler runner makes
//! its programs). It is read inside `core::enter_program`, and its checker
//! and emit pools are freed after the emit (`program::release_program`).
//! Its file versions stay leaked, like those of every program version, so
//! the file nodes of the result diagnostics stay valid.

use crate::prelude::*;

use std::sync::{Arc, Mutex, PoisonError};
use std::time::SystemTime;

use crate::emitter::emitter::EmitOnly;
use crate::emitter::program_emit::{self, EmitOptions, WriteFile, WriteFileData};
use crate::frontend::compiler::{NewProgram, ProgramOptions, new_compiler_host};
use crate::frontend::tsoptions::{get_default_lib_file_name, new_parsed_command_line};
use crate::frontend::tspath::{
    ComparePathsOptions, combine_paths, file_extension_is, get_encoded_root_length,
    get_normalized_absolute_path, get_root_length, has_rooted_url_suffix,
    has_trailing_directory_separator, has_url_root,
};
use crate::frontend::vfs::{Entries, FileInfo, Fs, FsError};
use crate::gostd::Context;
use crate::gostd::strconv::quote;
use crate::program;

// Go: transpile/transpile.go:16 Options
/// Options configures single-file transpilation.
// PORT: Go `CompilerOptions *core.CompilerOptions` is an owned value. The
// Go worker clones it, so the caller's options stay unchanged either way.
#[derive(Clone, Debug, Default)]
pub struct Options {
    /// CompilerOptions are the base compiler options to use for the transpilation.
    /// If nil, a default set of compiler options is used. Regardless of what is
    /// provided, a number of options are unconditionally overridden; see
    /// [`transpile_module`] and [`transpile_declaration`].
    pub compiler_options: Option<CompilerOptions>,

    /// FileName is the name given to the synthesized input file. It only needs to
    /// be provided if the source text relies on characteristics implied by the
    /// file's extension or path, e.g. its extension controls whether the file is
    /// parsed as a script or module, whether JSX syntax is allowed, etc.
    /// Defaults to "module.ts", or "module.tsx" if CompilerOptions.Jsx is set.
    pub file_name: String,

    /// ReportDiagnostics indicates whether syntactic and compiler option
    /// diagnostics should be included in the result. Regardless of this setting,
    /// diagnostics produced while emitting (including declaration emit errors
    /// such as those produced by isolated declarations) are always included.
    pub report_diagnostics: bool,
}

// Go: transpile/transpile.go:38 Output
/// Output contains the emitted text and any requested diagnostics.
// PORT: an empty `diagnostics` is also the Go nil slice.
#[derive(Clone, Debug, Default)]
pub struct Output {
    pub output_text: String,
    pub diagnostics: Vec<Diagnostic>,
    pub source_map_text: String,
}

// Go: transpile/transpile.go:46 inputDirectory
// inputDirectory is the synthetic current directory used to root the
// single input file created for transpilation.
// PORT: Go N' `tspath.RootedDirectoryPathFromNormalized("/")` (ts#64159);
// the port keeps the text.
const INPUT_DIRECTORY: &str = "/";

// Go: transpile/transpile.go:50 libDirectory
// libDirectory is the synthetic directory that the barebones default library
// file is placed in for declaration transpilation. See [barebonesLibContent].
const LIB_DIRECTORY: &str = "/lib";

// Go: transpile/transpile.go:58 barebonesLibContent
// Declaration emit works without a `lib`, but some local inferences you'd
// expect to work won't without at least a minimal `lib` available, since the
// checker will type inferred declarations as `any` without these defined.
// Late bound symbol names, in particular, are impossible to define without
// `Symbol` at least partially defined.
// TODO: This should *probably* just load the full, real `lib` for the target.
const BAREBONES_LIB_CONTENT: &str = r"interface Boolean {}
interface Function {}
interface CallableFunction {}
interface NewableFunction {}
interface IArguments {}
interface Number {}
interface Object {}
interface RegExp {}
interface String {}
interface Array<T> { length: number; [n: number]: T; }
interface SymbolConstructor {
    (desc?: string | number): symbol;
    for(name: string): symbol;
    readonly toStringTag: symbol;
}
declare var Symbol: SymbolConstructor;
interface Symbol {
    readonly [Symbol.toStringTag]: string;
}";

// Go: transpile/transpile.go:92 TranspileModule
/// TranspileModule transpiles a single file of source text to JavaScript
/// using the specified options. If no compiler options are provided, a
/// default set of compiler options is used. It returns nil if the context is
/// canceled before emission completes.
///
/// Extra compiler options that are unconditionally used by this function are:
///   - IsolatedModules = true (unless VerbatimModuleSyntax is set, which makes
///     this option redundant)
///   - NoCheck = true
///   - NoResolve = true
///   - NoLib = true
///   - Declaration = false
///   - DeclarationMap = false
///   - IsolatedDeclarations = false
pub fn transpile_module(ctx: &Context, input: &str, options: Options) -> Option<Output> {
    transpile_worker(ctx, input, options, false /*declaration*/)
}

// Go: transpile/transpile.go:113 TranspileDeclaration
/// TranspileDeclaration creates a declaration (.d.ts) file from a single file
/// of source text using the specified options. If no compiler options are
/// provided, a default set of compiler options is used.
///
/// Note that, because only the single input file is available, the resulting
/// declaration file may differ from the one a full program type-check and
/// emit would produce.
///
/// Extra compiler options that are unconditionally used by this function are:
///   - IsolatedModules = true (unless VerbatimModuleSyntax is set, which makes
///     this option redundant)
///   - NoCheck = true
///   - NoResolve = true
///   - NoLib = false
///   - Declaration = true
///   - EmitDeclarationOnly = true
///   - IsolatedDeclarations = true
pub fn transpile_declaration(ctx: &Context, input: &str, options: Options) -> Option<Output> {
    transpile_worker(ctx, input, options, true /*declaration*/)
}

/// The texts that the emit writes (Go `outputText`, `hasOutputText`,
/// `sourceMapText` and `hasSourceMapText`). `None` is "not written".
#[derive(Default)]
struct Written {
    output_text: Option<String>,
    source_map_text: Option<String>,
}

// Go: transpile/transpile.go:117 transpileWorker
fn transpile_worker(
    ctx: &Context,
    input: &str,
    options: Options,
    declaration: bool,
) -> Option<Output> {
    let mut opts = options.compiler_options.unwrap_or_default();

    set_options_for_transpile(&mut opts, declaration);

    // If jsx is specified, then treat the file as .tsx.
    let mut file_name = options.file_name;
    if file_name.is_empty() {
        if opts.jsx != JsxEmit::NONE {
            file_name = "module.tsx".to_string();
        } else {
            file_name = "module.ts".to_string();
        }
    }
    let input_file_name = to_rooted_file_path(&file_name, INPUT_DIRECTORY);

    let mut files = FxHashMap::default();
    files.insert(input_file_name.clone(), input.to_string());

    // Declaration emit needs a default lib to resolve global types (e.g.
    // `Array`, `Symbol`); plain transpilation sets NoLib so none is read.
    // The default lib name depends on the configured target.
    if declaration {
        let lib_file_name = get_default_lib_file_name(&opts);
        // ts#64159: Go `libDirectory.ResolveFile(libFileName)`; a default
        // lib file name appends with no normalization, as here.
        files.insert(
            combine_paths(LIB_DIRECTORY, &[lib_file_name.as_str()]),
            BAREBONES_LIB_CONTENT.to_string(),
        );
    }

    let program_fs = TranspileFs { files };
    let case_sensitive = program_fs.use_case_sensitive_file_names();
    let fs: Rc<dyn Fs> = Rc::new(program_fs);
    // tsgo#4712: the 6th argument is the content mapper project (Go nil).
    // PORT: Go N' `NewCompilerHost` has no current directory (ts#64159): the
    // program reads the base directory of its config. The port's host keeps
    // one, the same "/".
    let host = new_compiler_host(INPUT_DIRECTORY, fs, LIB_DIRECTORY, None, None, None);

    // ts#64159: the config has the base directory "/" and the file system's
    // case sensitivity (`tsoptions.NewParsedCommandLine`). N left both
    // unset.
    let config = Rc::new(new_parsed_command_line(
        Rc::new(opts),
        vec![input_file_name.clone()],
        None,
        ComparePathsOptions {
            use_case_sensitive_file_names: case_sensitive,
            current_directory: INPUT_DIRECTORY.to_string(),
        },
    ));
    // PORT: Go `compiler.NewProgram`. The frontend program parses with no
    // current program and then becomes a program version.
    let np: Rc<NewProgram> = {
        let _scope = enter_program(None);
        Rc::new(crate::frontend::compiler::new_program(ProgramOptions {
            host,
            config,
            use_source_of_project_reference: false,
            single_threaded: Tristate::Unknown,
            typings_location: String::new(),
            project_name: String::new(),
            create_module_resolver: None,
            skip_module_resolution: true,
        }))
    };
    let version = program::new_program_version(&np, None);

    let output = {
        let _scope = enter_program(Some(version));

        let mut all_diagnostics: Vec<Diagnostic> = Vec::new();
        if options.report_diagnostics {
            let source_file = program::get_source_file(&input_file_name);
            all_diagnostics.extend(program::get_syntactic_diagnostics(source_file));
            all_diagnostics.extend(program::get_config_file_parsing_diagnostics());
            all_diagnostics.extend(program::get_program_diagnostics());
        }

        let mut emit_only = EmitOnly::All;
        if declaration {
            emit_only = EmitOnly::Dts;
        }

        let written: Arc<Mutex<Written>> = Arc::default();
        let write_file: WriteFile = {
            let written = written.clone();
            Arc::new(
                move |file_name: &str,
                      text: &str,
                      _data: &mut WriteFileData|
                      -> Result<(), String> {
                    let mut written = written.lock().unwrap_or_else(PoisonError::into_inner);
                    if file_extension_is(file_name, ".map") {
                        go_assert!(
                            written.source_map_text.is_none(),
                            "Unexpected multiple source map outputs, file: {file_name}"
                        );
                        written.source_map_text = Some(text.to_string());
                    } else {
                        go_assert!(
                            written.output_text.is_none(),
                            "Unexpected multiple outputs, file: {file_name}"
                        );
                        written.output_text = Some(text.to_string());
                    }
                    Ok(())
                },
            )
        };
        // PORT: Go `Program.Emit` returns nil when the emit is not forced
        // and ctx is canceled (after `HandleNoEmitOptions`, which returns
        // nil here: noEmit and noEmitOnError are cleared above). The port's
        // `program_emit::emit` has no context, so the check is here.
        if !declaration && ctx.err().is_some() {
            None
        } else {
            let result = program_emit::emit(EmitOptions {
                emit_only,
                force_emit: declaration,
                write_file: Some(write_file),
                ..EmitOptions::default()
            });

            // Diagnostics produced during emit (e.g. isolated declaration errors) are
            // always included, regardless of ReportDiagnostics.
            all_diagnostics.extend(result.diagnostics);

            let written =
                std::mem::take(&mut *written.lock().unwrap_or_else(PoisonError::into_inner));
            go_assert!(written.output_text.is_some(), "Output generation failed");

            Some(Output {
                output_text: written.output_text.unwrap_or_default(),
                diagnostics: all_diagnostics,
                source_map_text: written.source_map_text.unwrap_or_default(),
            })
        }
    };
    program::release_program(version);
    output
}

// Go: tspath/rooted_path.go:120 ToRootedFilePath (ts#64159), through
// ToRootedPath (:29)
// ToRootedFilePath resolves fileName against currentDirectory, normalizes it,
// and gives it file intent.
// PORT: Go `tspath.RootedFilePath` is a `String`. Go N' normalizes with
// `getNormalizedAbsolutePathFromDirectory`; for a rooted, normalized current
// directory it gives the same text as `GetNormalizedAbsolutePath`. Lane-local
// until `tspath` has the rooted path types of ts#64159.
fn to_rooted_file_path(file_name: &str, current_directory: &str) -> String {
    if file_name.is_empty() {
        go_panic("path must not be empty".to_string());
    }
    if has_rooted_url_suffix(file_name) {
        go_panic("path must not contain a URL query or fragment".to_string());
    }
    if get_encoded_root_length(file_name) == 0
        && has_url_root(current_directory)
        && file_name.contains(['?', '#'])
    {
        go_panic("relative URL path must not contain a query or fragment".to_string());
    }
    let mut normalized = get_normalized_absolute_path(file_name, current_directory);
    if get_encoded_root_length(&normalized) == 0 || has_rooted_url_suffix(&normalized) {
        go_panic("path must be rooted".to_string());
    }
    // Go: tspath/rooted_path.go:65 ensureRootedPathRootSeparator
    if get_root_length(&normalized) == normalized.len()
        && !has_trailing_directory_separator(&normalized)
    {
        normalized.push('/');
    }
    normalized
}

// Go: transpile/options_generated.go:7 setOptionsForTranspile (ts#64457)
// PORT: Go generates it from `tools/scripts/tsc/options.ts`; the port keeps
// it by hand. It sets the same values as the N code it replaces.
fn set_options_for_transpile(options: &mut CompilerOptions, declaration: bool) {
    options.allow_importing_ts_extensions = Tristate::Unknown;
    options.allow_non_ts_extensions = Tristate::True;
    options.composite = Tristate::Unknown;
    if declaration {
        options.emit_declaration_only = Tristate::True;
    } else {
        options.emit_declaration_only = Tristate::Unknown;
    }
    if declaration {
        options.declaration = Tristate::True;
    } else {
        options.declaration = Tristate::False;
    }
    options.declaration_dir = String::new();
    if !declaration {
        options.declaration_map = Tristate::False;
    }
    if !options.verbatim_module_syntax.is_true() {
        options.isolated_modules = Tristate::True;
    }
    if declaration {
        options.isolated_declarations = Tristate::True;
    } else {
        options.isolated_declarations = Tristate::False;
    }
    options.incremental = Tristate::Unknown;
    options.lib = None;
    options.no_emit = Tristate::Unknown;
    options.no_check = Tristate::True;
    if declaration {
        options.no_lib = Tristate::False;
    } else {
        options.no_lib = Tristate::True;
    }
    options.no_emit_on_error = Tristate::Unknown;
    options.no_resolve = Tristate::True;
    options.paths = None;
    options.root_dirs = None;
    options.suppress_output_path_check = Tristate::True;
    options.ts_build_info_file = String::new();
    options.types = None;
    options.out_file = String::new();
}

// Go: transpile/fs.go:12 transpileFS
// transpileFS embeds unsupported operations so unexpected filesystem access
// panics.
// PORT: Go embeds a nil `vfs.FS`, so any other method dereferences nil
// (`go_nil_dereference`).
struct TranspileFs {
    files: FxHashMap<String, String>,
}

impl Fs for TranspileFs {
    // Go: transpile/fs.go:18 transpileFS.UseCaseSensitiveFileNames (at 673a5f17d713;
    // ts#64159 makes it CaseSensitivity, transpile/fs.go:19, CaseSensitive)
    fn use_case_sensitive_file_names(&self) -> bool {
        true
    }

    // Go: transpile/fs.go:23 transpileFS.FileExists
    fn file_exists(&self, path: &str) -> bool {
        let ok = self.files.contains_key(path);
        if !ok {
            go_panic(format!(
                "unexpected file existence check for {}",
                quote(path)
            ));
        }
        ok
    }

    // Go: transpile/fs.go:31 transpileFS.ReadFile
    fn read_file(&self, path: &str) -> (String, bool) {
        match self.files.get(path) {
            Some(content) => (content.clone(), true),
            None => go_panic(format!("unexpected file read for {}", quote(path))),
        }
    }

    fn write_file(&self, _path: &str, _data: &str) -> Result<(), FsError> {
        go_nil_dereference()
    }

    fn append_file(&self, _path: &str, _data: &str) -> Result<(), FsError> {
        go_nil_dereference()
    }

    fn remove(&self, _path: &str) -> Result<(), FsError> {
        go_nil_dereference()
    }

    fn chtimes(
        &self,
        _path: &str,
        _a_time: Option<SystemTime>,
        _m_time: Option<SystemTime>,
    ) -> Result<(), FsError> {
        go_nil_dereference()
    }

    // Go: transpile/fs.go:39 transpileFS.DirectoryExists
    fn directory_exists(&self, path: &str) -> bool {
        go_panic(format!(
            "unexpected directory existence check for {}",
            quote(path)
        ))
    }

    fn get_accessible_entries(&self, _path: &str) -> Entries {
        go_nil_dereference()
    }

    fn stat(&self, _path: &str) -> Option<FileInfo> {
        go_nil_dereference()
    }

    // Go: transpile/fs.go:43 transpileFS.Realpath
    fn realpath(&self, path: &str) -> String {
        go_panic(format!("unexpected realpath request for {}", quote(path)))
    }
}

#[cfg(test)]
mod tests {
    // Go: transpile/fs_test.go
    use super::*;

    use std::panic::{AssertUnwindSafe, catch_unwind};

    // Go: internal/testutil/testutil.go:14 AssertPanics
    fn assert_panics(f: impl FnOnce(), expected: &str) {
        let payload = catch_unwind(AssertUnwindSafe(f)).expect_err("expected a panic");
        let got = payload
            .downcast_ref::<GoPanic>()
            .map(|p| p.message.clone())
            .unwrap_or_else(|| panic!("expected a Go panic with {expected:?}"));
        assert_eq!(got, expected);
    }

    // Go: fs_test.go:10 TestTranspileFSRejectsDirectoryAccess
    #[test]
    fn test_transpile_fs_rejects_directory_access() {
        let mut files = FxHashMap::default();
        files.insert("/src/module.ts".to_string(), String::new());
        let fs = TranspileFs { files };
        assert_panics(
            || {
                fs.directory_exists("/src");
            },
            r#"unexpected directory existence check for "/src""#,
        );
        assert_panics(
            || {
                fs.realpath("/src/module.ts");
            },
            r#"unexpected realpath request for "/src/module.ts""#,
        );
    }

    /// ts#64159 (transpile.go:136 `ToRootedFilePath`): the input name is a
    /// rooted file path, and a URL with a query or fragment is not one.
    // PORT: not in Go; the panic text is Go N' output (cmap lane probe).
    #[test]
    fn test_transpile_rejects_url_file_name_with_query_or_fragment() {
        let transpile = |file_name: &str| {
            let options = Options {
                file_name: file_name.to_string(),
                ..Options::default()
            };
            transpile_module(&crate::gostd::context::background(), "export {};", options)
        };
        for file_name in [
            "https://example.com/a.ts?x=1",
            "https://example.com/a.ts#frag",
        ] {
            assert_panics(
                || {
                    transpile(file_name);
                },
                "path must not contain a URL query or fragment",
            );
        }
    }

    // Go: options_test.go:12 TestTranspileConditionalOptions (ts#64457)
    #[test]
    fn test_transpile_conditional_options() {
        for (declaration, want_map) in [(false, Tristate::False), (true, Tristate::True)] {
            for (verbatim, isolated, want) in [
                (Tristate::Unknown, Tristate::Unknown, Tristate::True),
                (Tristate::False, Tristate::False, Tristate::True),
                (Tristate::True, Tristate::Unknown, Tristate::Unknown),
                (Tristate::True, Tristate::False, Tristate::False),
                (Tristate::True, Tristate::True, Tristate::True),
            ] {
                let mut options = CompilerOptions {
                    verbatim_module_syntax: verbatim,
                    isolated_modules: isolated,
                    declaration_map: Tristate::True,
                    ..CompilerOptions::default()
                };
                set_options_for_transpile(&mut options, declaration);
                assert!(
                    options.isolated_modules == want && options.declaration_map == want_map,
                    "declaration={declaration} verbatim={verbatim:?} isolated={isolated:?}: got isolated={:?} declarationMap={:?}, want {want:?} {want_map:?}",
                    options.isolated_modules,
                    options.declaration_map,
                );
            }
        }
    }

    // Go: options_test.go:46 TestTranspileClearsInapplicableOptions (ts#64457)
    // PORT: the port's `Options` owns its compiler options, so the worker
    // cannot change the caller's copy; the last check still compares them.
    #[test]
    fn test_transpile_clears_inapplicable_options() {
        for declaration in [false, true] {
            let transpile = if declaration {
                transpile_declaration
            } else {
                transpile_module
            };
            let mut paths = IndexMap::default();
            paths.insert("*".to_string(), Some(vec!["/missing/*".to_string()]));
            let options = CompilerOptions {
                incremental: Tristate::True,
                declaration: Tristate::True,
                emit_declaration_only: Tristate::True,
                no_emit: Tristate::True,
                lib: Some(vec!["missing.d.ts".to_string()]),
                out_file: "/other/output.js".to_string(),
                composite: Tristate::True,
                ts_build_info_file: "/other/buildinfo".to_string(),
                paths: Some(paths),
                root_dirs: Some(vec!["/missing".to_string()]),
                types: Some(vec!["missing".to_string()]),
                allow_importing_ts_extensions: Tristate::True,
                no_emit_on_error: Tristate::True,
                declaration_dir: "/other/declarations".to_string(),
                ..CompilerOptions::default()
            };
            let before = options.clone();
            const SOURCE: &str = "export const value: number = 1;";
            let ctx = crate::gostd::context::background();
            let expected = transpile(
                &ctx,
                SOURCE,
                Options {
                    report_diagnostics: true,
                    ..Options::default()
                },
            );
            let actual = transpile(
                &ctx,
                SOURCE,
                Options {
                    compiler_options: Some(options.clone()),
                    report_diagnostics: true,
                    ..Options::default()
                },
            );
            let (Some(expected), Some(actual)) = (expected, actual) else {
                panic!("Transpilation was unexpectedly canceled");
            };
            assert!(
                !expected.output_text.is_empty()
                    && actual.output_text == expected.output_text
                    && actual.source_map_text == expected.source_map_text,
                "Inapplicable options changed the output: got {actual:?}, want {expected:?}"
            );
            assert!(
                expected.diagnostics.is_empty() && actual.diagnostics.is_empty(),
                "Unexpected diagnostics: got {:?}, want {:?}",
                actual.diagnostics,
                expected.diagnostics
            );
            assert!(
                options.deep_equal(&before),
                "Transpilation modified the caller's options"
            );
        }
    }
}
