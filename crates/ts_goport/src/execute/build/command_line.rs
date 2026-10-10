use crate::frontend::prelude::*;
use std::cell::OnceCell;

// This file ports the build mode (`tsc -b`) command line pieces:
// core/buildoptions.go, tsoptions/parsedbuildcommandline.go,
// tsoptions/commandlineparser.go ParseBuildCommandLine, and the
// buildOptionsParser / ParseBuildOptions parts of tsoptions/parsinghelpers.go
// that the frontend port left out.

// Go: core/buildoptions.go:3 BuildOptions (at 673a5f17d713; ts#64457 generates it,
// core/options_generated.go:860)
// PORT: Go `*int` (64-bit) is `Option<i64>`. The `noCopy` marker is dropped.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BuildOptions {
    pub dry: Tristate,
    pub force: Tristate,
    pub verbose: Tristate,
    pub builders: Option<i64>,
    pub stop_build_on_errors: Tristate,

    // CompilerOptions are not parsed here and will be available on ParsedBuildCommandLine

    // Internal fields
    pub clean: Tristate,
}

// Go: tsoptions/parsinghelpers.go:299 buildOptionsParser
// PORT: the embedded Go `*core.BuildOptions` is a mutable borrow, as in
// `CompilerOptionsParser`.
pub struct BuildOptionsParser<'a> {
    pub build_options: &'a mut BuildOptions,
}

impl OptionParser for BuildOptionsParser<'_> {
    // Go: tsoptions/parsinghelpers.go:303 (*buildOptionsParser).ParseOption
    fn parse_option(&mut self, key: &str, value: CompilerOptionsValue) -> Vec<Diagnostic> {
        parse_build_options(key, value, self.build_options)
    }

    // Go: tsoptions/parsinghelpers.go:307 (*buildOptionsParser).UnknownOptionDiagnostic
    fn unknown_option_diagnostic(&self) -> &'static Message {
        extra_key_diagnostics("buildOptions").expect("known key")
    }

    // Go: tsoptions/parsinghelpers.go:311 (*buildOptionsParser).UnknownDidYouMeanDiagnostic
    fn unknown_did_you_mean_diagnostic(&self) -> &'static Message {
        extra_key_did_you_mean_diagnostics("buildOptions").expect("known key")
    }
}

// Go: tsoptions/parsinghelpers.go:626 ParseBuildOptions (at 673a5f17d713; ts#64457
// generates it, tsoptions/options_generated.go:342)
// PORT: Go `allOptions` can be nil; the Rust caller always has options, so
// that nil check is dropped (as in `parse_compiler_options`).
pub fn parse_build_options(
    key: &str,
    value: CompilerOptionsValue,
    all_options: &mut BuildOptions,
) -> Vec<Diagnostic> {
    if value.is_nil() {
        return Vec::new();
    }
    let mut key = key;
    let option = BUILD_NAME_MAP.get(key);
    if let Some(option) = option {
        key = option.name;
    }
    match key {
        "clean" => all_options.clean = parse_tristate(&value),
        "dry" => all_options.dry = parse_tristate(&value),
        "force" => all_options.force = parse_tristate(&value),
        "builders" => all_options.builders = parse_number(&value),
        "stopBuildOnErrors" => all_options.stop_build_on_errors = parse_tristate(&value),
        "verbose" => all_options.verbose = parse_tristate(&value),
        _ => {}
    }
    Vec::new()
}

// Go: tsoptions/parsedbuildcommandline.go:12 ParsedBuildCommandLine
// PORT: Go `*core.CompilerOptions` is `Rc<CompilerOptions>`, as in
// `ParsedCommandLine`. Go `Raw any` holds the parser's
// `*collections.OrderedMap`, which is `CompilerOptionsValue::Map`.
// `sync.Once` plus the cached field is a `OnceCell` (for both
// `resolvedProjectPaths` and `locale`).
pub struct ParsedBuildCommandLine {
    pub build_options: BuildOptions,
    pub compiler_options: Rc<CompilerOptions>,
    pub projects: Vec<String>,
    pub errors: Vec<Diagnostic>,
    pub raw: CompilerOptionsValue,

    pub compare_paths_options: ComparePathsOptions,

    pub resolved_project_paths: OnceCell<Vec<String>>,

    pub locale: OnceCell<crate::locale::Locale>,
}

impl ParsedBuildCommandLine {
    // Go: tsoptions/parsedbuildcommandline.go:28 (*ParsedBuildCommandLine).ResolvedProjectPaths
    pub fn resolved_project_paths(&self) -> &[String] {
        self.resolved_project_paths.get_or_init(|| {
            self.projects
                .iter()
                .map(|project| {
                    resolve_config_file_name_of_project_reference(&resolve_path(
                        &self.compare_paths_options.current_directory,
                        &[project.as_str()],
                    ))
                })
                .collect()
        })
    }

    // Go: tsoptions/parsedbuildcommandline.go:39 (*ParsedBuildCommandLine).Locale
    // PORT: Go returns the `Locale` value; this returns a clone of the
    // cached value.
    pub fn locale(&self) -> crate::locale::Locale {
        self.locale
            .get_or_init(|| {
                let (locale, _) = crate::locale::parse(&self.compiler_options.locale);
                locale
            })
            .clone()
    }
}

// Go: tsoptions/commandlineparser.go:64 ParseBuildCommandLine
// PORT: Go nil `commandLine` is an empty slice here already.
pub fn parse_build_command_line(
    command_line: &[String],
    host: &dyn ParseConfigHost,
) -> ParsedBuildCommandLine {
    let fs = host.fs();
    let parser = parse_command_line_worker(
        &BUILD_OPTIONS_DID_YOU_MEAN_DIAGNOSTICS,
        command_line,
        Some(&*fs),
        host.get_current_directory(),
    );
    let mut compiler_options = CompilerOptions::default();
    for (key, value) in &parser.options {
        let build_option = BUILD_NAME_MAP.get(key);
        // PORT: Go compares option pointers; two nil pointers are equal.
        if same_option(build_option, Some(&*TSC_BUILD_OPTION))
            || same_option(build_option, COMPILER_NAME_MAP.get(key))
        {
            parse_compiler_options(key, value.clone(), &mut compiler_options);
        }
    }
    let mut build_options = BuildOptions::default();
    convert_map_to_options(
        &parser.options,
        BuildOptionsParser {
            build_options: &mut build_options,
        },
    );
    let mut result = ParsedBuildCommandLine {
        build_options,
        compiler_options: Rc::new(compiler_options),
        projects: parser.file_names,
        errors: parser.errors,
        raw: CompilerOptionsValue::Map(parser.options),

        compare_paths_options: ComparePathsOptions {
            use_case_sensitive_file_names: fs.use_case_sensitive_file_names(),
            current_directory: host.get_current_directory(),
        },
        resolved_project_paths: OnceCell::new(),
        locale: OnceCell::new(),
    };

    if result.projects.is_empty() {
        // tsc -b invoked with no extra arguments; act as if invoked with "tsc -b ."
        result.projects.push(".".to_string());
    }

    // Nonsensical combinations
    if result.build_options.clean.is_true() && result.build_options.force.is_true() {
        result.errors.push(new_compiler_diagnostic(
            diag::Options_0_and_1_cannot_be_combined,
            args!["clean", "force"],
        ));
    }
    if result.build_options.clean.is_true() && result.build_options.verbose.is_true() {
        result.errors.push(new_compiler_diagnostic(
            diag::Options_0_and_1_cannot_be_combined,
            args!["clean", "verbose"],
        ));
    }
    if result.build_options.clean.is_true() && result.compiler_options.watch.is_true() {
        result.errors.push(new_compiler_diagnostic(
            diag::Options_0_and_1_cannot_be_combined,
            args!["clean", "watch"],
        ));
    }
    if result.compiler_options.watch.is_true() && result.build_options.dry.is_true() {
        result.errors.push(new_compiler_diagnostic(
            diag::Options_0_and_1_cannot_be_combined,
            args!["watch", "dry"],
        ));
    }

    result
}

// Go pointer equality on `*CommandLineOption`, where nil equals nil.
fn same_option(
    a: Option<&'static CommandLineOption>,
    b: Option<&'static CommandLineOption>,
) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => std::ptr::eq(a, b),
        _ => false,
    }
}
