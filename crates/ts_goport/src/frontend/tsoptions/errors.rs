use crate::diagnostics::Message;
use crate::frontend::prelude::*;
use std::sync::LazyLock;

// ---------------------------------------------------------------------------
// tsoptions/errors.go
// ---------------------------------------------------------------------------

// Go: tsoptions/errors.go:14 createDiagnosticForInvalidEnumType
pub fn create_diagnostic_for_invalid_enum_type(
    opt: &CommandLineOption,
    source_file: Node,
    node: Node,
) -> Diagnostic {
    // PORT: Go `opt.EnumMap()` is non-nil here (the caller found the enum map).
    let names_of_type: Vec<String> = opt
        .enum_map()
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default();
    let string_names = format_enum_type_keys(opt, names_of_type);
    let opt_name = format!("--{}", opt.name);
    create_diagnostic_for_node_in_source_file_or_compiler_diagnostic(
        source_file,
        node,
        diag::Argument_for_0_option_must_be_Colon_1,
        args![opt_name, string_names],
    )
}

// Go: tsoptions/errors.go:21 formatEnumTypeKeys
fn format_enum_type_keys(opt: &CommandLineOption, mut keys: Vec<String>) -> String {
    if let Some(deprecated_keys) = opt.deprecated_keys() {
        keys.retain(|key| !deprecated_keys.contains(key.as_str()));
    }
    format!("'{}'", keys.join("', '"))
}

// Go: tsoptions/errors.go:28 getCompilerOptionValueTypeString
pub fn get_compiler_option_value_type_string(option: &CommandLineOption) -> String {
    match option.kind {
        CommandLineOptionKind::LIST_OR_ELEMENT => {
            // PORT: Go formats a nil `Elements()` with `%v` as `<nil>`. Every
            // listOrElement option has an element declaration.
            let elements = option
                .elements()
                .expect("listOrElement option has elements");
            format!(
                "{} or Array",
                get_compiler_option_value_type_string(elements)
            )
        }
        CommandLineOptionKind::LIST => "Array".to_string(),
        _ => option.kind.0.to_string(),
    }
}

impl CommandLineParser {
    // Go: tsoptions/errors.go:39 (*commandLineParser).createUnknownOptionError
    pub fn create_unknown_option_error(
        &self,
        unknown_option: &str,
        unknown_option_error_text: &str,
        node: Node,
        source_file: Node,
    ) -> Diagnostic {
        create_unknown_option_error(
            unknown_option,
            self.unknown_option_diagnostic(),
            unknown_option_error_text,
            node,
            source_file,
            self.alternate_mode(),
            Some(self.unknown_did_you_mean_diagnostic()),
            Some(&command_line_options_to_map(
                self.worker_diagnostics.did_you_mean.option_declarations,
            )),
        )
    }
}

// createUnknownOptionError creates a diagnostic for an unknown option. If
// unknownDidYouMeanDiagnostic and optionsNameMap are provided, it also checks
// for a spelling suggestion and emits a "did you mean" diagnostic instead.
// Go: tsoptions/errors.go:60 createUnknownOptionError
// PORT: Go nil `unknownDidYouMeanDiagnostic` and `optionsNameMap` are `None`.
#[allow(clippy::too_many_arguments)]
pub fn create_unknown_option_error(
    unknown_option: &str,
    unknown_option_diagnostic: &'static Message,
    unknown_option_error_text: &str,                   // optional
    node: Node,                                        // optional
    source_file: Node,                                 // optional
    alternate_mode: Option<&AlternateModeDiagnostics>, // optional
    unknown_did_you_mean_diagnostic: Option<&'static Message>, // optional; nil skips suggestion
    options_name_map: Option<&CommandLineOptionNameMap>, // optional; nil skips suggestion
) -> Diagnostic {
    if let Some(alternate_mode) = alternate_mode
        && let Some(options_name_map) = alternate_mode.options_name_map
    {
        let other_option = options_name_map.get(&unknown_option.to_lowercase());
        if let Some(other_option) = other_option {
            // tscbuildoption
            let mut diagnostic = alternate_mode.diagnostic;
            if other_option.name == "build" {
                diagnostic = diag::Option_build_must_be_the_first_command_line_argument;
            }
            return create_diagnostic_for_node_in_source_file_or_compiler_diagnostic(
                source_file,
                node,
                diagnostic,
                args![unknown_option],
            );
        }
    }
    let mut unknown_option_error_text = unknown_option_error_text;
    if unknown_option_error_text.is_empty() {
        unknown_option_error_text = unknown_option;
    }
    if let Some(unknown_did_you_mean_diagnostic) = unknown_did_you_mean_diagnostic
        && let Some(options_name_map) = options_name_map
        && let Some(possible_option) = options_name_map.get_spelling_suggestion(unknown_option)
    {
        return create_diagnostic_for_node_in_source_file_or_compiler_diagnostic(
            source_file,
            node,
            unknown_did_you_mean_diagnostic,
            args![unknown_option_error_text, possible_option.name],
        );
    }
    create_diagnostic_for_node_in_source_file_or_compiler_diagnostic(
        source_file,
        node,
        unknown_option_diagnostic,
        args![unknown_option_error_text],
    )
}

// Go: tsoptions/errors.go:92 CreateDiagnosticForNodeInSourceFile
pub fn create_diagnostic_for_node_in_source_file(
    source_file: Node,
    node: Node,
    message: &'static Message,
    args: Vec<String>,
) -> Diagnostic {
    new_diagnostic(
        source_file,
        TextRange::new(
            skip_trivia(&source_file_text(source_file), node.loc().pos()),
            node.end(),
        ),
        message,
        args,
    )
}

// Go: tsoptions/errors.go:96 CreateDiagnosticForNodeInSourceFileOrCompilerDiagnostic
pub fn create_diagnostic_for_node_in_source_file_or_compiler_diagnostic(
    source_file: Node,
    node: Node,
    message: &'static Message,
    args: Vec<String>,
) -> Diagnostic {
    if source_file.is_some() && node.is_some() {
        return create_diagnostic_for_node_in_source_file(source_file, node, message, args);
    }
    new_compiler_diagnostic(message, args)
}

// Go: tsoptions/errors.go:103 extraKeyDiagnostics
pub fn extra_key_diagnostics(s: &str) -> Option<&'static Message> {
    match s {
        "compilerOptions" => Some(diag::Unknown_compiler_option_0),
        "typeAcquisition" => Some(diag::Unknown_type_acquisition_option_0),
        "buildOptions" => Some(diag::Unknown_build_option_0),
        _ => None,
    }
}

// Go: tsoptions/errors.go:116 extraKeyDidYouMeanDiagnostics
pub fn extra_key_did_you_mean_diagnostics(s: &str) -> Option<&'static Message> {
    match s {
        "compilerOptions" => Some(diag::Unknown_compiler_option_0_Did_you_mean_1),
        "typeAcquisition" => Some(diag::Unknown_type_acquisition_option_0_Did_you_mean_1),
        "buildOptions" => Some(diag::Unknown_build_option_0_Did_you_mean_1),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// tsoptions/diagnostics.go
// ---------------------------------------------------------------------------

// Go: tsoptions/diagnostics.go:9 DidYouMeanOptionsDiagnostics
#[derive(Debug)]
pub struct DidYouMeanOptionsDiagnostics {
    pub alternate_mode: Option<AlternateModeDiagnostics>,
    pub option_declarations: &'static [&'static CommandLineOption],
    pub unknown_option_diagnostic: &'static Message,
    pub unknown_did_you_mean_diagnostic: &'static Message,
}

// Go: tsoptions/diagnostics.go:16 AlternateModeDiagnostics
#[derive(Debug)]
pub struct AlternateModeDiagnostics {
    pub diagnostic: &'static Message,
    pub options_name_map: Option<&'static NameMap>,
}

// Go: tsoptions/diagnostics.go:21 ParseCommandLineWorkerDiagnostics
// PORT: Go `optionsNameMap` and `optionsNameMapOnce` are never read or set
// in the pinned source. `options_name_map` stays `None` and the `sync.Once`
// is dropped.
#[derive(Debug)]
pub struct ParseCommandLineWorkerDiagnostics {
    pub did_you_mean: DidYouMeanOptionsDiagnostics,
    pub options_name_map: Option<&'static NameMap>,
    pub option_type_mismatch_diagnostic: &'static Message,
}

// Go: tsoptions/diagnostics.go:28 CompilerOptionsDidYouMeanDiagnostics
// PORT: Go package-level vars are `LazyLock` statics.
pub static COMPILER_OPTIONS_DID_YOU_MEAN_DIAGNOSTICS: LazyLock<ParseCommandLineWorkerDiagnostics> =
    LazyLock::new(|| get_parse_command_line_worker_diagnostics(OPTIONS_DECLARATIONS.as_slice()));

// Go: tsoptions/diagnostics.go:30 getParseCommandLineWorkerDiagnostics
pub fn get_parse_command_line_worker_diagnostics(
    decls: &'static [&'static CommandLineOption],
) -> ParseCommandLineWorkerDiagnostics {
    // this will only return the correct diagnostics for `compiler` mode, and is factored into a function for testing reasons.
    ParseCommandLineWorkerDiagnostics {
        did_you_mean: DidYouMeanOptionsDiagnostics {
            alternate_mode: Some(AlternateModeDiagnostics {
                diagnostic: diag::Compiler_option_0_may_only_be_used_with_build,
                options_name_map: Some(&*BUILD_NAME_MAP),
            }),
            option_declarations: decls,
            unknown_option_diagnostic: diag::Unknown_compiler_option_0,
            unknown_did_you_mean_diagnostic: diag::Unknown_compiler_option_0_Did_you_mean_1,
        },
        options_name_map: None,
        option_type_mismatch_diagnostic: diag::Compiler_option_0_expects_an_argument,
    }
}

// Go: tsoptions/diagnostics.go:46 buildOptionsDidYouMeanDiagnostics
pub static BUILD_OPTIONS_DID_YOU_MEAN_DIAGNOSTICS: LazyLock<ParseCommandLineWorkerDiagnostics> =
    LazyLock::new(|| ParseCommandLineWorkerDiagnostics {
        did_you_mean: DidYouMeanOptionsDiagnostics {
            alternate_mode: Some(AlternateModeDiagnostics {
                diagnostic: diag::Compiler_option_0_may_not_be_used_with_build,
                options_name_map: Some(&*COMPILER_NAME_MAP),
            }),
            option_declarations: BUILD_OPTS.as_slice(),
            unknown_option_diagnostic: diag::Unknown_build_option_0,
            unknown_did_you_mean_diagnostic: diag::Unknown_build_option_0_Did_you_mean_1,
        },
        options_name_map: None,
        option_type_mismatch_diagnostic: diag::Build_option_0_requires_a_value_of_type_1,
    });
