use crate::frontend::prelude::*;
use std::sync::LazyLock;

// This file ports the option declarations and tsoptions/namemap.go. Since
// ts#64457 Go generates the declarations (tsoptions/declarations_generated.go)
// and the option compare functions (tsoptions/options_generated.go) from
// tools/scripts/tsc/options.ts; declscompiler.go, declsbuild.go, declswatch.go
// and declstypeacquisition.go are gone. The port keeps the hand-written lists:
// at fed0bf24149f every generated declaration list, element, enum map and
// deprecated key set equals the old Go lists field by field, except the 7
// removed watch options (OptionsForWatch) and their 2 elements. The section
// headers below name the old Go files.
// PORT: Go package-level vars are `LazyLock` statics. Declaration lists
// are `Vec<&'static CommandLineOption>` with leaked entries, so the same
// option has the same address in every list, as in Go.

/// Leaks a declaration so it has the Go pointer lifetime.
fn opt(o: CommandLineOption) -> &'static CommandLineOption {
    Box::leak(Box::new(o))
}

/// Go `slices.Concat` for declaration lists.
fn concat_options(
    a: &[&'static CommandLineOption],
    b: &[&'static CommandLineOption],
) -> Vec<&'static CommandLineOption> {
    a.iter().chain(b.iter()).copied().collect()
}

// ---------------------------------------------------------------------------
// tsoptions/declscompiler.go
// ---------------------------------------------------------------------------

// Go: tsoptions/declarations_generated.go:13 OptionsDeclarations
pub static OPTIONS_DECLARATIONS: LazyLock<Vec<&'static CommandLineOption>> =
    LazyLock::new(|| concat_options(&COMMON_OPTIONS_WITH_BUILD, &OPTIONS_FOR_COMPILER));

// Go: tsoptions/declarations_generated.go:17 commonOptionsWithBuild
pub static COMMON_OPTIONS_WITH_BUILD: LazyLock<Vec<&'static CommandLineOption>> = LazyLock::new(
    || {
        vec![
    //******* commonOptionsWithBuild *******
    opt(CommandLineOption {
        name: "help",
        short_name: "h",
        kind: CommandLineOptionKind::BOOLEAN,
        show_in_simplified_help_view: true,
        is_command_line_only: true,
        category: Some(diag::Command_line_Options),
        description: Some(diag::Print_this_message),
        default_value_description: CompilerOptionsValue::Bool(false),
        ..Default::default()
    }),
    opt(CommandLineOption {
        name: "help",
        short_name: "?",
        kind: CommandLineOptionKind::BOOLEAN,
        is_command_line_only: true,
        category: Some(diag::Command_line_Options),
        default_value_description: CompilerOptionsValue::Bool(false),
        ..Default::default()
    }),
    opt(CommandLineOption {
        name: "watch",
        short_name: "w",
        kind: CommandLineOptionKind::BOOLEAN,
        show_in_simplified_help_view: true,
        is_command_line_only: true,
        category: Some(diag::Command_line_Options),
        description: Some(diag::Watch_input_files),
        default_value_description: CompilerOptionsValue::Bool(false),
        ..Default::default()
    }),
    opt(CommandLineOption {
        name: "preserveWatchOutput",
        kind: CommandLineOptionKind::BOOLEAN,
        show_in_simplified_help_view: false,
        category: Some(diag::Output_Formatting),
        description: Some(diag::Disable_wiping_the_console_in_watch_mode),
        default_value_description: CompilerOptionsValue::Bool(false),
        ..Default::default()
    }),
    opt(CommandLineOption {
        name: "listFiles",
        kind: CommandLineOptionKind::BOOLEAN,
        category: Some(diag::Compiler_Diagnostics),
        description: Some(diag::Print_all_of_the_files_read_during_the_compilation),
        default_value_description: CompilerOptionsValue::Bool(false),
        ..Default::default()
    }),
    opt(CommandLineOption {
        name: "explainFiles",
        kind: CommandLineOptionKind::BOOLEAN,
        category: Some(diag::Compiler_Diagnostics),
        description: Some(diag::Print_files_read_during_the_compilation_including_why_it_was_included),
        default_value_description: CompilerOptionsValue::Bool(false),
        ..Default::default()
    }),
    opt(CommandLineOption {
        name: "listEmittedFiles",
        kind: CommandLineOptionKind::BOOLEAN,
        category: Some(diag::Compiler_Diagnostics),
        description: Some(diag::Print_the_names_of_emitted_files_after_a_compilation),
        default_value_description: CompilerOptionsValue::Bool(false),
        ..Default::default()
    }),
    opt(CommandLineOption {
        name: "pretty",
        kind: CommandLineOptionKind::BOOLEAN,
        show_in_simplified_help_view: true,
        category: Some(diag::Output_Formatting),
        description: Some(diag::Enable_color_and_formatting_in_TypeScript_s_output_to_make_compiler_errors_easier_to_read),
        default_value_description: CompilerOptionsValue::Bool(true),
        ..Default::default()
    }),
    opt(CommandLineOption {
        name: "traceResolution",
        kind: CommandLineOptionKind::BOOLEAN,
        category: Some(diag::Compiler_Diagnostics),
        description: Some(diag::Log_paths_used_during_the_moduleResolution_process),
        default_value_description: CompilerOptionsValue::Bool(false),
        ..Default::default()
    }),
    opt(CommandLineOption {
        name: "diagnostics",
        kind: CommandLineOptionKind::BOOLEAN,
        category: Some(diag::Compiler_Diagnostics),
        description: Some(diag::Output_compiler_performance_information_after_building),
        default_value_description: CompilerOptionsValue::Bool(false),
        ..Default::default()
    }),
    opt(CommandLineOption {
        name: "extendedDiagnostics",
        kind: CommandLineOptionKind::BOOLEAN,
        category: Some(diag::Compiler_Diagnostics),
        description: Some(diag::Output_more_detailed_compiler_performance_information_after_building),
        default_value_description: CompilerOptionsValue::Bool(false),
        ..Default::default()
    }),
    opt(CommandLineOption {
        name: "generateCpuProfile",
        kind: CommandLineOptionKind::STRING,
        is_file_path: true,
        category: Some(diag::Compiler_Diagnostics),
        description: Some(diag::Emit_a_v8_CPU_profile_of_the_compiler_run_for_debugging),
        default_value_description: CompilerOptionsValue::String("profile.cpuprofile".to_string()),
        ..Default::default()
    }),

    opt(CommandLineOption {
        name: "generateTrace",
        kind: CommandLineOptionKind::STRING,
        is_file_path: true,
        category: Some(diag::Compiler_Diagnostics),
        description: Some(diag::Generates_an_event_trace_and_a_list_of_types),
        ..Default::default()
    }),
    opt(CommandLineOption {
        name: "incremental",
        short_name: "i",
        kind: CommandLineOptionKind::BOOLEAN,
        category: Some(diag::Projects),
        description: Some(diag::Save_tsbuildinfo_files_to_allow_for_incremental_compilation_of_projects),
        transpile_option_value: Tristate::Unknown,
        default_value_description: CompilerOptionsValue::Message(diag::X_false_unless_composite_is_set),
        ..Default::default()
    }),
    opt(CommandLineOption {
        name: "declaration",
        short_name: "d",
        kind: CommandLineOptionKind::BOOLEAN,
        // Not setting affectsEmit because we calculate this flag might not affect full emit
        affects_build_info: true,
        show_in_simplified_help_view: true,
        category: Some(diag::Emit),
        transpile_option_value: Tristate::Unknown,
        description: Some(diag::Generate_d_ts_files_from_TypeScript_and_JavaScript_files_in_your_project),
        default_value_description: CompilerOptionsValue::Message(diag::X_false_unless_composite_is_set),
        ..Default::default()
    }),
    opt(CommandLineOption {
        name: "declarationMap",
        kind: CommandLineOptionKind::BOOLEAN,
        // Not setting affectsEmit because we calculate this flag might not affect full emit
        affects_build_info: true,
        show_in_simplified_help_view: true,
        category: Some(diag::Emit),
        default_value_description: CompilerOptionsValue::Bool(false),
        description: Some(diag::Create_sourcemaps_for_d_ts_files),
        ..Default::default()
    }),
    opt(CommandLineOption {
        name: "emitDeclarationOnly",
        kind: CommandLineOptionKind::BOOLEAN,
        // Not setting affectsEmit because we calculate this flag might not affect full emit
        affects_build_info: true,
        show_in_simplified_help_view: true,
        category: Some(diag::Emit),
        description: Some(diag::Only_output_d_ts_files_and_not_JavaScript_files),
        transpile_option_value: Tristate::Unknown,
        default_value_description: CompilerOptionsValue::Bool(false),
        ..Default::default()
    }),
    opt(CommandLineOption {
        name: "sourceMap",
        kind: CommandLineOptionKind::BOOLEAN,
        // Not setting affectsEmit because we calculate this flag might not affect full emit
        affects_build_info: true,
        show_in_simplified_help_view: true,
        category: Some(diag::Emit),
        default_value_description: CompilerOptionsValue::Bool(false),
        description: Some(diag::Create_source_map_files_for_emitted_JavaScript_files),
        ..Default::default()
    }),
    opt(CommandLineOption {
        name: "inlineSourceMap",
        kind: CommandLineOptionKind::BOOLEAN,
        // Not setting affectsEmit because we calculate this flag might not affect full emit
        affects_build_info: true,
        category: Some(diag::Emit),
        description: Some(diag::Include_sourcemap_files_inside_the_emitted_JavaScript),
        default_value_description: CompilerOptionsValue::Bool(false),
        ..Default::default()
    }),
    opt(CommandLineOption {
        name: "noCheck",
        kind: CommandLineOptionKind::BOOLEAN,
        show_in_simplified_help_view: false,
        category: Some(diag::Compiler_Diagnostics),
        description: Some(diag::Disable_full_type_checking_only_critical_parse_and_emit_errors_will_be_reported),
        transpile_option_value: Tristate::True,
        default_value_description: CompilerOptionsValue::Bool(false),
        // Not setting affectsSemanticDiagnostics or affectsBuildInfo because we dont want all diagnostics to go away, its handled in builder
        ..Default::default()
    }),
    opt(CommandLineOption {
        name: "deduplicatePackages",
        kind: CommandLineOptionKind::BOOLEAN,
        category: Some(diag::Type_Checking),
        description: Some(diag::Deduplicate_packages_with_the_same_name_and_version),
        default_value_description: CompilerOptionsValue::Bool(true),
        affects_program_structure: true,
        ..Default::default()
    }),
    opt(CommandLineOption {
        name: "noEmit",
        kind: CommandLineOptionKind::BOOLEAN,
        show_in_simplified_help_view: true,
        category: Some(diag::Emit),
        description: Some(diag::Disable_emitting_files_from_a_compilation),
        transpile_option_value: Tristate::Unknown,
        default_value_description: CompilerOptionsValue::Bool(false),
        ..Default::default()
    }),
    opt(CommandLineOption {
        name: "assumeChangesOnlyAffectDirectDependencies",
        kind: CommandLineOptionKind::BOOLEAN,
        affects_semantic_diagnostics: true,
        affects_emit: true,
        affects_build_info: true,
        category: Some(diag::Watch_and_Build_Modes),
        description: Some(diag::Have_recompiles_in_projects_that_use_incremental_and_watch_mode_assume_that_changes_within_a_file_will_only_affect_files_directly_depending_on_it),
        default_value_description: CompilerOptionsValue::Bool(false),
        ..Default::default()
    }),
    opt(CommandLineOption {
        name: "locale",
        kind: CommandLineOptionKind::STRING,
        category: Some(diag::Command_line_Options),
        is_command_line_only: true,
        description: Some(diag::Set_the_language_of_the_messaging_from_TypeScript_This_does_not_affect_emit),
        default_value_description: CompilerOptionsValue::Message(diag::Platform_specific),
        extra_validation: ExtraValidation::LOCALE,
        ..Default::default()
    }),

    opt(CommandLineOption {
        name: "quiet",
        short_name: "q",
        kind: CommandLineOptionKind::BOOLEAN,
        category: Some(diag::Command_line_Options),
        description: Some(diag::Do_not_print_diagnostics),
        ..Default::default()
    }),
    opt(CommandLineOption {
        name: "singleThreaded",
        kind: CommandLineOptionKind::BOOLEAN,
        category: Some(diag::Command_line_Options),
        description: Some(diag::Run_in_single_threaded_mode),
        ..Default::default()
    }),
    opt(CommandLineOption {
        name: "pprofDir",
        kind: CommandLineOptionKind::STRING,
        is_file_path: true,
        category: Some(diag::Command_line_Options),
        description: Some(diag::Generate_pprof_CPU_Slashmemory_profiles_to_the_given_directory),
        ..Default::default()
    }),
    opt(CommandLineOption {
        name: "checkers",
        kind: CommandLineOptionKind::NUMBER,
        category: Some(diag::Command_line_Options),
        description: Some(diag::Set_the_number_of_checkers_per_project),
        default_value_description: CompilerOptionsValue::Message(diag::X_4_unless_singleThreaded_is_passed),
        min_value: 1,
        ..Default::default()
    }),
    // tsgo#4712
    opt(CommandLineOption {
        name: "runExternalCode",
        kind: CommandLineOptionKind::BOOLEAN,
        category: Some(diag::Command_line_Options),
        is_command_line_only: true,
        description: Some(diag::Allow_loading_external_content_mapper_plugins_that_execute_code_during_compilation),
        default_value_description: CompilerOptionsValue::Bool(false),
        ..Default::default()
    }),
    ]
    },
);

// Go: tsoptions/declarations_generated.go:247 optionsForCompiler
pub static OPTIONS_FOR_COMPILER: LazyLock<Vec<&'static CommandLineOption>> = LazyLock::new(|| {
    vec![
    //******* compilerOptions not common with --build *******

    // CommandLine only options
    opt(CommandLineOption {
        name: "all",
        kind: CommandLineOptionKind::BOOLEAN,
        show_in_simplified_help_view: true,
        category: Some(diag::Command_line_Options),
        description: Some(diag::Show_all_compiler_options),
        default_value_description: CompilerOptionsValue::Bool(false),
        ..Default::default()
    }),
    opt(CommandLineOption {
        name: "version",
        short_name: "v",
        kind: CommandLineOptionKind::BOOLEAN,
        show_in_simplified_help_view: true,
        category: Some(diag::Command_line_Options),
        description: Some(diag::Print_the_compiler_s_version),
        default_value_description: CompilerOptionsValue::Bool(false),
        ..Default::default()
    }),
    opt(CommandLineOption {
        name: "init",
        kind: CommandLineOptionKind::BOOLEAN,
        show_in_simplified_help_view: true,
        category: Some(diag::Command_line_Options),
        description: Some(diag::Initializes_a_TypeScript_project_and_creates_a_tsconfig_json_file),
        default_value_description: CompilerOptionsValue::Bool(false),
        ..Default::default()
    }),
    opt(CommandLineOption {
        name: "project",
        short_name: "p",
        kind: CommandLineOptionKind::STRING,
        is_file_path: true,
        show_in_simplified_help_view: true,
        category: Some(diag::Command_line_Options),
        description: Some(diag::Compile_the_project_given_the_path_to_its_configuration_file_or_to_a_folder_with_a_tsconfig_json),
        ..Default::default()
    }),
    opt(CommandLineOption {
        name: "showConfig",
        kind: CommandLineOptionKind::BOOLEAN,
        show_in_simplified_help_view: true,
        category: Some(diag::Command_line_Options),
        is_command_line_only: true,
        description: Some(diag::Print_the_final_configuration_instead_of_building),
        default_value_description: CompilerOptionsValue::Bool(false),
        ..Default::default()
    }),
    opt(CommandLineOption {
        name: "listFilesOnly",
        kind: CommandLineOptionKind::BOOLEAN,
        category: Some(diag::Command_line_Options),
        is_command_line_only: true,
        description: Some(diag::Print_names_of_files_that_are_part_of_the_compilation_and_then_stop_processing),
        default_value_description: CompilerOptionsValue::Bool(false),
        ..Default::default()
    }),
    opt(CommandLineOption {
        name: "ignoreConfig",
        kind: CommandLineOptionKind::BOOLEAN,
        show_in_simplified_help_view: true,
        category: Some(diag::Command_line_Options),
        is_command_line_only: true,
        description: Some(diag::Ignore_the_tsconfig_found_and_build_with_commandline_options_and_files),
        default_value_description: CompilerOptionsValue::Bool(false),
        ..Default::default()
    }),

    // Basic
    // targetOptionDeclaration,
    opt(CommandLineOption {
        name: "target",
        short_name: "t",
        kind: CommandLineOptionKind::ENUM, // targetOptionMap
        affects_source_file: true,
        affects_module_resolution: true,
        affects_emit: true,
        affects_build_info: true,
        show_in_simplified_help_view: true,
        category: Some(diag::Language_and_Environment),
        description: Some(diag::Set_the_JavaScript_language_version_for_emitted_JavaScript_and_include_compatible_library_declarations),
        default_value_description: CompilerOptionsValue::ScriptTarget(ScriptTarget::LATEST_STANDARD),
        ..Default::default()
    }),

    // moduleOptionDeclaration,
    opt(CommandLineOption {
        name: "module",
        short_name: "m",
        kind: CommandLineOptionKind::ENUM, // moduleOptionMap
        affects_module_resolution: true,
        affects_emit: true,
        affects_build_info: true,
        show_in_simplified_help_view: true,
        category: Some(diag::Modules),
        description: Some(diag::Specify_what_module_code_is_generated),
        default_value_description: CompilerOptionsValue::Tristate(Tristate::Unknown),
        ..Default::default()
    }),
    opt(CommandLineOption {
        name: "lib",
        kind: CommandLineOptionKind::LIST,
        // elements: &CommandLineOption{
            // 	name:                    "lib",
            // 	kind:                   CommandLineOptionTypeEnum, // libMap,
            // 	defaultValueDescription: core.TSUnknown,
        // },
        affects_program_structure: true,
        show_in_simplified_help_view: true,
        category: Some(diag::Language_and_Environment),
        description: Some(diag::Specify_a_set_of_bundled_library_declaration_files_that_describe_the_target_runtime_environment),
        transpile_option_value: Tristate::Unknown,
        ..Default::default()
    }),
    opt(CommandLineOption {
        name: "allowJs",
        kind: CommandLineOptionKind::BOOLEAN,
        allow_js_flag: true,
        affects_build_info: true,
        show_in_simplified_help_view: true,
        category: Some(diag::JavaScript_Support),
        description: Some(diag::Allow_JavaScript_files_to_be_a_part_of_your_program_Use_the_checkJs_option_to_get_errors_from_these_files),
        default_value_description: CompilerOptionsValue::Message(diag::X_false_unless_checkJs_is_set),
        ..Default::default()
    }),
    opt(CommandLineOption {
        name: "checkJs",
        kind: CommandLineOptionKind::BOOLEAN,
        affects_module_resolution: true,
        affects_semantic_diagnostics: true,
        affects_build_info: true,
        show_in_simplified_help_view: true,
        category: Some(diag::JavaScript_Support),
        description: Some(diag::Enable_error_reporting_in_type_checked_JavaScript_files),
        default_value_description: CompilerOptionsValue::Bool(false),
        ..Default::default()
    }),
    opt(CommandLineOption {
        name: "jsx",
        kind: CommandLineOptionKind::ENUM, // jsxOptionMap,
        affects_source_file: true,
        affects_emit: true,
        affects_build_info: true,
        affects_module_resolution: true,
        // The checker emits an error when it sees JSX but this option is not set in compilerOptions.
        // This is effectively a semantic error, so mark this option as affecting semantic diagnostics
        // so we know to refresh errors when this option is changed.
        affects_semantic_diagnostics: true,
        show_in_simplified_help_view: true,
        category: Some(diag::Language_and_Environment),
        description: Some(diag::Specify_what_JSX_code_is_generated),
        default_value_description: CompilerOptionsValue::Tristate(Tristate::Unknown),
        ..Default::default()
    }),
    opt(CommandLineOption {
        name: "outFile",
        kind: CommandLineOptionKind::STRING,
        affects_emit: true,
        affects_build_info: true,
        affects_declaration_path: true,
        is_file_path: true,
        show_in_simplified_help_view: true,
        category: Some(diag::Emit),
        description: Some(diag::Specify_a_file_that_bundles_all_outputs_into_one_JavaScript_file_If_declaration_is_true_also_designates_a_file_that_bundles_all_d_ts_output),
        transpile_option_value: Tristate::Unknown,
        ..Default::default()
    }),
    opt(CommandLineOption {
        name: "outDir",
        kind: CommandLineOptionKind::STRING,
        affects_emit: true,
        affects_build_info: true,
        affects_declaration_path: true,
        is_file_path: true,
        show_in_simplified_help_view: true,
        category: Some(diag::Emit),
        description: Some(diag::Specify_an_output_folder_for_all_emitted_files),
        ..Default::default()
    }),
    opt(CommandLineOption {
        name: "rootDir",
        kind: CommandLineOptionKind::STRING,
        affects_emit: true,
        affects_build_info: true,
        affects_declaration_path: true,
        is_file_path: true,
        category: Some(diag::Modules),
        description: Some(diag::Specify_the_root_folder_within_your_source_files),
        default_value_description: CompilerOptionsValue::Message(diag::Computed_from_the_list_of_input_files),
        ..Default::default()
    }),
    opt(CommandLineOption {
        name: "composite",
        kind: CommandLineOptionKind::BOOLEAN,
        // Not setting affectsEmit because we calculate this flag might not affect full emit
        affects_build_info: true,
        is_ts_config_only: true,
        category: Some(diag::Projects),
        transpile_option_value: Tristate::Unknown,
        default_value_description: CompilerOptionsValue::Bool(false),
        description: Some(diag::Enable_constraints_that_allow_a_TypeScript_project_to_be_used_with_project_references),
        ..Default::default()
    }),
    opt(CommandLineOption {
        name: "tsBuildInfoFile",
        kind: CommandLineOptionKind::STRING,
        affects_emit: true,
        affects_build_info: true,
        is_file_path: true,
        category: Some(diag::Projects),
        transpile_option_value: Tristate::Unknown,
        default_value_description: CompilerOptionsValue::String(".tsbuildinfo".to_string()),
        description: Some(diag::Specify_the_path_to_tsbuildinfo_incremental_compilation_file),
        ..Default::default()
    }),
    opt(CommandLineOption {
        name: "removeComments",
        kind: CommandLineOptionKind::BOOLEAN,
        affects_emit: true,
        affects_build_info: true,
        show_in_simplified_help_view: true,
        category: Some(diag::Emit),
        default_value_description: CompilerOptionsValue::Bool(false),
        description: Some(diag::Disable_emitting_comments),
        ..Default::default()
    }),
    opt(CommandLineOption {
        name: "importHelpers",
        kind: CommandLineOptionKind::BOOLEAN,
        affects_emit: true,
        affects_build_info: true,
        affects_source_file: true,
        category: Some(diag::Emit),
        description: Some(diag::Allow_importing_helper_functions_from_tslib_once_per_project_instead_of_including_them_per_file),
        default_value_description: CompilerOptionsValue::Bool(false),
        ..Default::default()
    }),
    opt(CommandLineOption {
        name: "downlevelIteration",
        kind: CommandLineOptionKind::BOOLEAN,
        affects_emit: true,
        affects_build_info: true,
        category: Some(diag::Emit),
        description: Some(diag::Emit_more_compliant_but_verbose_and_less_performant_JavaScript_for_iteration),
        default_value_description: CompilerOptionsValue::Bool(false),
        ..Default::default()
    }),
    opt(CommandLineOption {
        name: "isolatedModules",
        kind: CommandLineOptionKind::BOOLEAN,
        category: Some(diag::Interop_Constraints),
        description: Some(diag::Ensure_that_each_file_can_be_safely_transpiled_without_relying_on_other_imports),
        transpile_option_value: Tristate::True,
        default_value_description: CompilerOptionsValue::Bool(false),
        ..Default::default()
    }),
    opt(CommandLineOption {
        name: "verbatimModuleSyntax",
        kind: CommandLineOptionKind::BOOLEAN,
        affects_emit: true,
        affects_semantic_diagnostics: true,
        affects_build_info: true,
        category: Some(diag::Interop_Constraints),
        description: Some(diag::Do_not_transform_or_elide_any_imports_or_exports_not_marked_as_type_only_ensuring_they_are_written_in_the_output_file_s_format_based_on_the_module_setting),
        default_value_description: CompilerOptionsValue::Bool(false),
        ..Default::default()
    }),
    opt(CommandLineOption {
        name: "isolatedDeclarations",
        kind: CommandLineOptionKind::BOOLEAN,
        category: Some(diag::Interop_Constraints),
        description: Some(diag::Require_sufficient_annotation_on_exports_so_other_tools_can_trivially_generate_declaration_files),
        default_value_description: CompilerOptionsValue::Bool(false),
        affects_build_info: true,
        affects_semantic_diagnostics: true,
        ..Default::default()
    }),
    opt(CommandLineOption {
        name: "erasableSyntaxOnly",
        kind: CommandLineOptionKind::BOOLEAN,
        category: Some(diag::Interop_Constraints),
        description: Some(diag::Do_not_allow_runtime_constructs_that_are_not_part_of_ECMAScript),
        default_value_description: CompilerOptionsValue::Bool(false),
        affects_build_info: true,
        affects_semantic_diagnostics: true,
        ..Default::default()
    }),
    opt(CommandLineOption {
        name: "libReplacement",
        kind: CommandLineOptionKind::BOOLEAN,
        affects_program_structure: true,
        category: Some(diag::Language_and_Environment),
        description: Some(diag::Enable_lib_replacement),
        default_value_description: CompilerOptionsValue::Bool(false),
        ..Default::default()
    }),

    // Strict Type Checks
    opt(CommandLineOption {
        name: "strict",
        kind: CommandLineOptionKind::BOOLEAN,
        // Though this affects semantic diagnostics, affectsSemanticDiagnostics is not set here
        // The value of each strictFlag depends on own strictFlag value or this and never accessed directly.
        // But we need to store `strict` in builf info, even though it won't be examined directly, so that the
        // flags it controls (e.g. `strictNullChecks`) will be retrieved correctly
        affects_build_info: true,
        show_in_simplified_help_view: true,
        category: Some(diag::Type_Checking),
        description: Some(diag::Enable_all_strict_type_checking_options),
        default_value_description: CompilerOptionsValue::Bool(true),
        ..Default::default()
    }),
    opt(CommandLineOption {
        name: "noImplicitAny",
        kind: CommandLineOptionKind::BOOLEAN,
        affects_semantic_diagnostics: true,
        affects_build_info: true,
        strict_flag: true,
        category: Some(diag::Type_Checking),
        description: Some(diag::Enable_error_reporting_for_expressions_and_declarations_with_an_implied_any_type),
        default_value_description: CompilerOptionsValue::Message(diag::X_true_unless_strict_is_false),
        ..Default::default()
    }),
    opt(CommandLineOption {
        name: "strictNullChecks",
        kind: CommandLineOptionKind::BOOLEAN,
        affects_semantic_diagnostics: true,
        affects_build_info: true,
        strict_flag: true,
        category: Some(diag::Type_Checking),
        description: Some(diag::When_type_checking_take_into_account_null_and_undefined),
        default_value_description: CompilerOptionsValue::Message(diag::X_true_unless_strict_is_false),
        ..Default::default()
    }),
    opt(CommandLineOption {
        name: "strictFunctionTypes",
        kind: CommandLineOptionKind::BOOLEAN,
        affects_semantic_diagnostics: true,
        affects_build_info: true,
        strict_flag: true,
        category: Some(diag::Type_Checking),
        description: Some(diag::When_assigning_functions_check_to_ensure_parameters_and_the_return_values_are_subtype_compatible),
        default_value_description: CompilerOptionsValue::Message(diag::X_true_unless_strict_is_false),
        ..Default::default()
    }),
    opt(CommandLineOption {
        name: "strictBindCallApply",
        kind: CommandLineOptionKind::BOOLEAN,
        affects_semantic_diagnostics: true,
        affects_build_info: true,
        strict_flag: true,
        category: Some(diag::Type_Checking),
        description: Some(diag::Check_that_the_arguments_for_bind_call_and_apply_methods_match_the_original_function),
        default_value_description: CompilerOptionsValue::Message(diag::X_true_unless_strict_is_false),
        ..Default::default()
    }),
    opt(CommandLineOption {
        name: "strictPropertyInitialization",
        kind: CommandLineOptionKind::BOOLEAN,
        affects_semantic_diagnostics: true,
        affects_build_info: true,
        strict_flag: true,
        category: Some(diag::Type_Checking),
        description: Some(diag::Check_for_class_properties_that_are_declared_but_not_set_in_the_constructor),
        default_value_description: CompilerOptionsValue::Message(diag::X_true_unless_strict_is_false),
        ..Default::default()
    }),
    opt(CommandLineOption {
        name: "strictBuiltinIteratorReturn",
        kind: CommandLineOptionKind::BOOLEAN,
        affects_semantic_diagnostics: true,
        affects_build_info: true,
        strict_flag: true,
        category: Some(diag::Type_Checking),
        description: Some(diag::Built_in_iterators_are_instantiated_with_a_TReturn_type_of_undefined_instead_of_any),
        default_value_description: CompilerOptionsValue::Message(diag::X_true_unless_strict_is_false),
        ..Default::default()
    }),
    opt(CommandLineOption {
        name: "noImplicitThis",
        kind: CommandLineOptionKind::BOOLEAN,
        affects_semantic_diagnostics: true,
        affects_build_info: true,
        strict_flag: true,
        category: Some(diag::Type_Checking),
        description: Some(diag::Enable_error_reporting_when_this_is_given_the_type_any),
        default_value_description: CompilerOptionsValue::Message(diag::X_true_unless_strict_is_false),
        ..Default::default()
    }),
    opt(CommandLineOption {
        name: "useUnknownInCatchVariables",
        kind: CommandLineOptionKind::BOOLEAN,
        affects_semantic_diagnostics: true,
        affects_build_info: true,
        strict_flag: true,
        category: Some(diag::Type_Checking),
        description: Some(diag::Default_catch_clause_variables_as_unknown_instead_of_any),
        default_value_description: CompilerOptionsValue::Message(diag::X_true_unless_strict_is_false),
        ..Default::default()
    }),
    opt(CommandLineOption {
        name: "alwaysStrict",
        kind: CommandLineOptionKind::BOOLEAN,
        affects_source_file: true,
        affects_emit: true,
        affects_build_info: true,
        category: Some(diag::Type_Checking),
        description: Some(diag::Ensure_use_strict_is_always_emitted),
        default_value_description: CompilerOptionsValue::Bool(true),
        ..Default::default()
    }),
    opt(CommandLineOption {
        name: "stableTypeOrdering",
        kind: CommandLineOptionKind::BOOLEAN,
        affects_semantic_diagnostics: true,
        affects_build_info: true,
        category: Some(diag::Type_Checking),
        description: Some(diag::Ensure_types_are_ordered_stably_and_deterministically_across_compilations),
        default_value_description: CompilerOptionsValue::Bool(true),
        ..Default::default()
    }),

    // Additional Checks
    opt(CommandLineOption {
        name: "noUnusedLocals",
        kind: CommandLineOptionKind::BOOLEAN,
        affects_semantic_diagnostics: true,
        affects_build_info: true,
        category: Some(diag::Type_Checking),
        description: Some(diag::Enable_error_reporting_when_local_variables_aren_t_read),
        default_value_description: CompilerOptionsValue::Bool(false),
        ..Default::default()
    }),
    opt(CommandLineOption {
        name: "noUnusedParameters",
        kind: CommandLineOptionKind::BOOLEAN,
        affects_semantic_diagnostics: true,
        affects_build_info: true,
        category: Some(diag::Type_Checking),
        description: Some(diag::Raise_an_error_when_a_function_parameter_isn_t_read),
        default_value_description: CompilerOptionsValue::Bool(false),
        ..Default::default()
    }),
    opt(CommandLineOption {
        name: "exactOptionalPropertyTypes",
        kind: CommandLineOptionKind::BOOLEAN,
        affects_semantic_diagnostics: true,
        affects_build_info: true,
        category: Some(diag::Type_Checking),
        description: Some(diag::Interpret_optional_property_types_as_written_rather_than_adding_undefined),
        default_value_description: CompilerOptionsValue::Bool(false),
        ..Default::default()
    }),
    opt(CommandLineOption {
        name: "noImplicitReturns",
        kind: CommandLineOptionKind::BOOLEAN,
        affects_semantic_diagnostics: true,
        affects_build_info: true,
        category: Some(diag::Type_Checking),
        description: Some(diag::Enable_error_reporting_for_codepaths_that_do_not_explicitly_return_in_a_function),
        default_value_description: CompilerOptionsValue::Bool(false),
        ..Default::default()
    }),
    opt(CommandLineOption {
        name: "noFallthroughCasesInSwitch",
        kind: CommandLineOptionKind::BOOLEAN,
        affects_bind_diagnostics: true,
        affects_semantic_diagnostics: true,
        affects_build_info: true,
        category: Some(diag::Type_Checking),
        description: Some(diag::Enable_error_reporting_for_fallthrough_cases_in_switch_statements),
        default_value_description: CompilerOptionsValue::Bool(false),
        ..Default::default()
    }),
    opt(CommandLineOption {
        name: "noUncheckedIndexedAccess",
        kind: CommandLineOptionKind::BOOLEAN,
        affects_semantic_diagnostics: true,
        affects_build_info: true,
        category: Some(diag::Type_Checking),
        description: Some(diag::Add_undefined_to_a_type_when_accessed_using_an_index),
        default_value_description: CompilerOptionsValue::Bool(false),
        ..Default::default()
    }),
    opt(CommandLineOption {
        name: "noImplicitOverride",
        kind: CommandLineOptionKind::BOOLEAN,
        affects_semantic_diagnostics: true,
        affects_build_info: true,
        category: Some(diag::Type_Checking),
        description: Some(diag::Ensure_overriding_members_in_derived_classes_are_marked_with_an_override_modifier),
        default_value_description: CompilerOptionsValue::Bool(false),
        ..Default::default()
    }),
    opt(CommandLineOption {
        name: "noPropertyAccessFromIndexSignature",
        kind: CommandLineOptionKind::BOOLEAN,
        affects_semantic_diagnostics: true,
        affects_build_info: true,
        show_in_simplified_help_view: false,
        category: Some(diag::Type_Checking),
        description: Some(diag::Enforces_using_indexed_accessors_for_keys_declared_using_an_indexed_type),
        default_value_description: CompilerOptionsValue::Bool(false),
        ..Default::default()
    }),

    // Module Resolution
    opt(CommandLineOption {
        name: "moduleResolution",
        kind: CommandLineOptionKind::ENUM,
        //    new Map(Object.entries({
        //         // N.B. The first entry specifies the value shown in `tsc --init`
        //         node10: ModuleResolutionKind.Node10,
        //         node: ModuleResolutionKind.Node10,
        //         classic: ModuleResolutionKind.Classic,
        //         node16: ModuleResolutionKind.Node16,
        //         nodenext: ModuleResolutionKind.NodeNext,
        //         bundler: ModuleResolutionKind.Bundler,
        //     })),
        affects_module_resolution: true,
        category: Some(diag::Modules),
        description: Some(diag::Specify_how_TypeScript_looks_up_a_file_from_a_given_module_specifier),
        default_value_description: CompilerOptionsValue::Message(diag::X_nodenext_if_module_is_nodenext_node16_if_module_is_node16_or_node18_otherwise_bundler),
        ..Default::default()
    }),
    opt(CommandLineOption {
        name: "baseUrl",
        kind: CommandLineOptionKind::STRING,
        affects_module_resolution: true,
        is_file_path: true,
        category: Some(diag::Modules),
        description: Some(diag::Specify_the_base_directory_to_resolve_non_relative_module_names),
        ..Default::default()
    }),
    opt(CommandLineOption {
        // this option can only be specified in tsconfig.json
        // use type = object to copy the value as-is
        name: "paths",
        kind: CommandLineOptionKind::OBJECT,
        affects_module_resolution: true,
        allow_config_dir_template_substitution: true,
        is_ts_config_only: true,
        category: Some(diag::Modules),
        description: Some(diag::Specify_a_set_of_entries_that_re_map_imports_to_additional_lookup_locations),
        transpile_option_value: Tristate::Unknown,
        ..Default::default()
    }),
    opt(CommandLineOption {
        // this option can only be specified in tsconfig.json
        // use type = object to copy the value as-is
        name: "rootDirs",
        kind: CommandLineOptionKind::LIST,
        is_ts_config_only: true,
        affects_module_resolution: true,
        allow_config_dir_template_substitution: true,
        category: Some(diag::Modules),
        description: Some(diag::Allow_multiple_folders_to_be_treated_as_one_when_resolving_modules),
        transpile_option_value: Tristate::Unknown,
        default_value_description: CompilerOptionsValue::Message(diag::Computed_from_the_list_of_input_files),
        ..Default::default()
    }),
    opt(CommandLineOption {
        name: "typeRoots",
        kind: CommandLineOptionKind::LIST,
        affects_module_resolution: true,
        allow_config_dir_template_substitution: true,
        category: Some(diag::Modules),
        description: Some(diag::Specify_multiple_folders_that_act_like_Slashnode_modules_Slash_types),
        ..Default::default()
    }),
    opt(CommandLineOption {
        name: "types",
        kind: CommandLineOptionKind::LIST,
        affects_program_structure: true,
        show_in_simplified_help_view: true,
        category: Some(diag::Modules),
        description: Some(diag::Specify_type_package_names_to_be_included_without_being_referenced_in_a_source_file),
        transpile_option_value: Tristate::Unknown,
        ..Default::default()
    }),
    opt(CommandLineOption {
        name: "allowSyntheticDefaultImports",
        kind: CommandLineOptionKind::BOOLEAN,
        affects_semantic_diagnostics: true,
        affects_build_info: true,
        category: Some(diag::Interop_Constraints),
        description: Some(diag::Allow_import_x_from_y_when_a_module_doesn_t_have_a_default_export),
        default_value_description: CompilerOptionsValue::Bool(true),
        ..Default::default()
    }),
    opt(CommandLineOption {
        name: "esModuleInterop",
        kind: CommandLineOptionKind::BOOLEAN,
        affects_semantic_diagnostics: true,
        affects_emit: true,
        affects_build_info: true,
        show_in_simplified_help_view: true,
        category: Some(diag::Interop_Constraints),
        description: Some(diag::Emit_additional_JavaScript_to_ease_support_for_importing_CommonJS_modules_This_enables_allowSyntheticDefaultImports_for_type_compatibility),
        default_value_description: CompilerOptionsValue::Bool(true),
        ..Default::default()
    }),
    opt(CommandLineOption {
        name: "preserveSymlinks",
        kind: CommandLineOptionKind::BOOLEAN,
        category: Some(diag::Interop_Constraints),
        description: Some(diag::Disable_resolving_symlinks_to_their_realpath_This_correlates_to_the_same_flag_in_node),
        default_value_description: CompilerOptionsValue::Bool(false),
        ..Default::default()
    }),
    opt(CommandLineOption {
        name: "allowUmdGlobalAccess",
        kind: CommandLineOptionKind::BOOLEAN,
        affects_semantic_diagnostics: true,
        affects_build_info: true,
        category: Some(diag::Modules),
        description: Some(diag::Allow_accessing_UMD_globals_from_modules),
        default_value_description: CompilerOptionsValue::Bool(false),
        ..Default::default()
    }),
    opt(CommandLineOption {
        name: "moduleSuffixes",
        kind: CommandLineOptionKind::LIST,
        list_preserve_falsy_values: true,
        affects_module_resolution: true,
        category: Some(diag::Modules),
        description: Some(diag::List_of_file_name_suffixes_to_search_when_resolving_a_module),
        ..Default::default()
    }),
    opt(CommandLineOption {
        name: "allowImportingTsExtensions",
        kind: CommandLineOptionKind::BOOLEAN,
        affects_semantic_diagnostics: true,
        affects_build_info: true,
        category: Some(diag::Modules),
        description: Some(diag::Allow_imports_to_include_TypeScript_file_extensions_Requires_moduleResolution_bundler_and_either_noEmit_or_emitDeclarationOnly_to_be_set),
        default_value_description: CompilerOptionsValue::Bool(false),
        transpile_option_value: Tristate::Unknown,
        ..Default::default()
    }),
    opt(CommandLineOption {
        name: "rewriteRelativeImportExtensions",
        kind: CommandLineOptionKind::BOOLEAN,
        affects_semantic_diagnostics: true,
        affects_build_info: true,
        category: Some(diag::Modules),
        description: Some(diag::Rewrite_ts_tsx_mts_and_cts_file_extensions_in_relative_import_paths_to_their_JavaScript_equivalent_in_output_files),
        default_value_description: CompilerOptionsValue::Bool(false),
        ..Default::default()
    }),
    opt(CommandLineOption {
        name: "resolvePackageJsonExports",
        kind: CommandLineOptionKind::BOOLEAN,
        affects_module_resolution: true,
        category: Some(diag::Modules),
        description: Some(diag::Use_the_package_json_exports_field_when_resolving_package_imports),
        default_value_description: CompilerOptionsValue::Message(diag::X_true_when_moduleResolution_is_node16_nodenext_or_bundler_otherwise_false),
        ..Default::default()
    }),
    opt(CommandLineOption {
        name: "resolvePackageJsonImports",
        kind: CommandLineOptionKind::BOOLEAN,
        affects_module_resolution: true,
        category: Some(diag::Modules),
        description: Some(diag::Use_the_package_json_imports_field_when_resolving_imports),
        default_value_description: CompilerOptionsValue::Message(diag::X_true_when_moduleResolution_is_node16_nodenext_or_bundler_otherwise_false),
        ..Default::default()
    }),
    opt(CommandLineOption {
        name: "customConditions",
        kind: CommandLineOptionKind::LIST,
        affects_module_resolution: true,
        category: Some(diag::Modules),
        description: Some(diag::Conditions_to_set_in_addition_to_the_resolver_specific_defaults_when_resolving_imports),
        ..Default::default()
    }),
    opt(CommandLineOption {
        name: "noUncheckedSideEffectImports",
        kind: CommandLineOptionKind::BOOLEAN,
        affects_semantic_diagnostics: true,
        affects_build_info: true,
        category: Some(diag::Modules),
        description: Some(diag::Check_side_effect_imports),
        default_value_description: CompilerOptionsValue::Bool(true),
        ..Default::default()
    }),

    // Source Maps
    opt(CommandLineOption {
        name: "sourceRoot",
        kind: CommandLineOptionKind::STRING,
        affects_emit: true,
        affects_build_info: true,
        category: Some(diag::Emit),
        description: Some(diag::Specify_the_root_path_for_debuggers_to_find_the_reference_source_code),
        ..Default::default()
    }),
    opt(CommandLineOption {
        name: "mapRoot",
        kind: CommandLineOptionKind::STRING,
        affects_emit: true,
        affects_build_info: true,
        category: Some(diag::Emit),
        description: Some(diag::Specify_the_location_where_debugger_should_locate_map_files_instead_of_generated_locations),
        ..Default::default()
    }),
    opt(CommandLineOption {
        name: "inlineSources",
        kind: CommandLineOptionKind::BOOLEAN,
        affects_emit: true,
        affects_build_info: true,
        category: Some(diag::Emit),
        description: Some(diag::Include_source_code_in_the_sourcemaps_inside_the_emitted_JavaScript),
        default_value_description: CompilerOptionsValue::Bool(false),
        ..Default::default()
    }),

    // Experimental
    opt(CommandLineOption {
        name: "experimentalDecorators",
        kind: CommandLineOptionKind::BOOLEAN,
        affects_emit: true,
        affects_semantic_diagnostics: true,
        affects_build_info: true,
        category: Some(diag::Language_and_Environment),
        description: Some(diag::Enable_experimental_support_for_legacy_experimental_decorators),
        default_value_description: CompilerOptionsValue::Bool(false),
        ..Default::default()
    }),
    opt(CommandLineOption {
        name: "emitDecoratorMetadata",
        kind: CommandLineOptionKind::BOOLEAN,
        affects_semantic_diagnostics: true,
        affects_emit: true,
        affects_build_info: true,
        category: Some(diag::Language_and_Environment),
        description: Some(diag::Emit_design_type_metadata_for_decorated_declarations_in_source_files),
        default_value_description: CompilerOptionsValue::Bool(false),
        ..Default::default()
    }),

    // Advanced
    opt(CommandLineOption {
        name: "jsxFactory",
        kind: CommandLineOptionKind::STRING,
        category: Some(diag::Language_and_Environment),
        description: Some(diag::Specify_the_JSX_factory_function_used_when_targeting_React_JSX_emit_e_g_React_createElement_or_h),
        default_value_description: CompilerOptionsValue::String("`React.createElement`".to_string()),
        ..Default::default()
    }),
    opt(CommandLineOption {
        name: "jsxFragmentFactory",
        kind: CommandLineOptionKind::STRING,
        category: Some(diag::Language_and_Environment),
        description: Some(diag::Specify_the_JSX_Fragment_reference_used_for_fragments_when_targeting_React_JSX_emit_e_g_React_Fragment_or_Fragment),
        default_value_description: CompilerOptionsValue::String("React.Fragment".to_string()),
        ..Default::default()
    }),
    opt(CommandLineOption {
        name: "jsxImportSource",
        kind: CommandLineOptionKind::STRING,
        affects_semantic_diagnostics: true,
        affects_emit: true,
        affects_build_info: true,
        affects_module_resolution: true,
        affects_source_file: true,
        category: Some(diag::Language_and_Environment),
        description: Some(diag::Specify_module_specifier_used_to_import_the_JSX_factory_functions_when_using_jsx_Colon_react_jsx_Asterisk),
        default_value_description: CompilerOptionsValue::String("react".to_string()),
        ..Default::default()
    }),
    opt(CommandLineOption {
        name: "resolveJsonModule",
        kind: CommandLineOptionKind::BOOLEAN,
        affects_module_resolution: true,
        category: Some(diag::Modules),
        description: Some(diag::Enable_importing_json_files),
        default_value_description: CompilerOptionsValue::Bool(false),
        ..Default::default()
    }),
    opt(CommandLineOption {
        name: "allowArbitraryExtensions",
        kind: CommandLineOptionKind::BOOLEAN,
        affects_program_structure: true,
        category: Some(diag::Modules),
        description: Some(diag::Enable_importing_files_with_any_extension_provided_a_declaration_file_is_present),
        default_value_description: CompilerOptionsValue::Bool(false),
        ..Default::default()
    }),

    opt(CommandLineOption {
        name: "reactNamespace",
        kind: CommandLineOptionKind::STRING,
        affects_emit: true,
        affects_build_info: true,
        category: Some(diag::Language_and_Environment),
        description: Some(diag::Specify_the_object_invoked_for_createElement_This_only_applies_when_targeting_react_JSX_emit),
        default_value_description: CompilerOptionsValue::String("`React`".to_string()),
        ..Default::default()
    }),
    opt(CommandLineOption {
        name: "skipDefaultLibCheck",
        kind: CommandLineOptionKind::BOOLEAN,
        // We need to store these to determine whether `lib` files need to be rechecked
        affects_build_info: true,
        category: Some(diag::Completeness),
        description: Some(diag::Skip_type_checking_d_ts_files_that_are_included_with_TypeScript),
        default_value_description: CompilerOptionsValue::Bool(false),
        ..Default::default()
    }),
    opt(CommandLineOption {
        name: "emitBOM",
        kind: CommandLineOptionKind::BOOLEAN,
        affects_emit: true,
        affects_build_info: true,
        category: Some(diag::Emit),
        description: Some(diag::Emit_a_UTF_8_Byte_Order_Mark_BOM_in_the_beginning_of_output_files),
        default_value_description: CompilerOptionsValue::Bool(false),
        ..Default::default()
    }),
    opt(CommandLineOption {
        name: "newLine",
        kind: CommandLineOptionKind::ENUM, // newLineOptionMap,
        affects_emit: true,
        affects_build_info: true,
        category: Some(diag::Emit),
        description: Some(diag::Set_the_newline_character_for_emitting_files),
        default_value_description: CompilerOptionsValue::String("lf".to_string()),
        ..Default::default()
    }),
    opt(CommandLineOption {
        name: "noErrorTruncation",
        kind: CommandLineOptionKind::BOOLEAN,
        affects_semantic_diagnostics: true,
        affects_build_info: true,
        category: Some(diag::Output_Formatting),
        description: Some(diag::Disable_truncating_types_in_error_messages),
        default_value_description: CompilerOptionsValue::Bool(false),
        ..Default::default()
    }),
    opt(CommandLineOption {
        name: "noLib",
        kind: CommandLineOptionKind::BOOLEAN,
        category: Some(diag::Language_and_Environment),
        affects_program_structure: true,
        description: Some(diag::Disable_including_any_library_files_including_the_default_lib_d_ts),
        // We are not returning a sourceFile for lib file when asked by the program,
        // so pass --noLib to avoid reporting a file not found error.
        transpile_option_value: Tristate::True,
        default_value_description: CompilerOptionsValue::Bool(false),
        ..Default::default()
    }),
    opt(CommandLineOption {
        name: "noResolve",
        kind: CommandLineOptionKind::BOOLEAN,
        affects_module_resolution: true,
        category: Some(diag::Modules),
        description: Some(diag::Disallow_import_s_require_s_or_reference_s_from_expanding_the_number_of_files_TypeScript_should_add_to_a_project),
        // We are not doing a full typecheck, we are not resolving the whole context,
        // so pass --noResolve to avoid reporting missing file errors.
        transpile_option_value: Tristate::True,
        default_value_description: CompilerOptionsValue::Bool(false),
        ..Default::default()
    }),
    opt(CommandLineOption {
        name: "stripInternal",
        kind: CommandLineOptionKind::BOOLEAN,
        affects_emit: true,
        affects_build_info: true,
        category: Some(diag::Emit),
        description: Some(diag::Disable_emitting_declarations_that_have_internal_in_their_JSDoc_comments),
        default_value_description: CompilerOptionsValue::Bool(false),
        ..Default::default()
    }),
    opt(CommandLineOption {
        name: "disableSizeLimit",
        kind: CommandLineOptionKind::BOOLEAN,
        affects_program_structure: true,
        category: Some(diag::Editor_Support),
        description: Some(diag::Remove_the_20mb_cap_on_total_source_code_size_for_JavaScript_files_in_the_TypeScript_language_server),
        default_value_description: CompilerOptionsValue::Bool(false),
        ..Default::default()
    }),
    opt(CommandLineOption {
        name: "disableSourceOfProjectReferenceRedirect",
        kind: CommandLineOptionKind::BOOLEAN,
        is_ts_config_only: true,
        category: Some(diag::Projects),
        description: Some(diag::Disable_preferring_source_files_instead_of_declaration_files_when_referencing_composite_projects),
        default_value_description: CompilerOptionsValue::Bool(false),
        ..Default::default()
    }),
    opt(CommandLineOption {
        name: "disableSolutionSearching",
        kind: CommandLineOptionKind::BOOLEAN,
        is_ts_config_only: true,
        category: Some(diag::Projects),
        description: Some(diag::Opt_a_project_out_of_multi_project_reference_checking_when_editing),
        default_value_description: CompilerOptionsValue::Bool(false),
        ..Default::default()
    }),
    opt(CommandLineOption {
        name: "disableReferencedProjectLoad",
        kind: CommandLineOptionKind::BOOLEAN,
        is_ts_config_only: true,
        category: Some(diag::Projects),
        description: Some(diag::Reduce_the_number_of_projects_loaded_automatically_by_TypeScript),
        default_value_description: CompilerOptionsValue::Bool(false),
        ..Default::default()
    }),
    opt(CommandLineOption {
        name: "noEmitHelpers",
        kind: CommandLineOptionKind::BOOLEAN,
        affects_emit: true,
        affects_build_info: true,
        category: Some(diag::Emit),
        description: Some(diag::Disable_generating_custom_helper_functions_like_extends_in_compiled_output),
        default_value_description: CompilerOptionsValue::Bool(false),
        ..Default::default()
    }),
    opt(CommandLineOption {
        name: "noEmitOnError",
        kind: CommandLineOptionKind::BOOLEAN,
        affects_emit: true,
        affects_build_info: true,
        category: Some(diag::Emit),
        transpile_option_value: Tristate::Unknown,
        description: Some(diag::Disable_emitting_files_if_any_type_checking_errors_are_reported),
        default_value_description: CompilerOptionsValue::Bool(false),
        ..Default::default()
    }),
    opt(CommandLineOption {
        name: "preserveConstEnums",
        kind: CommandLineOptionKind::BOOLEAN,
        affects_emit: true,
        affects_build_info: true,
        category: Some(diag::Emit),
        description: Some(diag::Disable_erasing_const_enum_declarations_in_generated_code),
        default_value_description: CompilerOptionsValue::Bool(false),
        ..Default::default()
    }),
    opt(CommandLineOption {
        name: "declarationDir",
        kind: CommandLineOptionKind::STRING,
        affects_emit: true,
        affects_build_info: true,
        affects_declaration_path: true,
        is_file_path: true,
        category: Some(diag::Emit),
        transpile_option_value: Tristate::Unknown,
        description: Some(diag::Specify_the_output_directory_for_generated_declaration_files),
        ..Default::default()
    }),
    opt(CommandLineOption {
        name: "skipLibCheck",
        kind: CommandLineOptionKind::BOOLEAN,
        // We need to store these to determine whether `lib` files need to be rechecked
        affects_build_info: true,
        category: Some(diag::Completeness),
        description: Some(diag::Skip_type_checking_all_d_ts_files),
        default_value_description: CompilerOptionsValue::Bool(false),
        ..Default::default()
    }),
    opt(CommandLineOption {
        name: "allowUnusedLabels",
        kind: CommandLineOptionKind::BOOLEAN,
        affects_bind_diagnostics: true,
        affects_semantic_diagnostics: true,
        affects_build_info: true,
        category: Some(diag::Type_Checking),
        description: Some(diag::Disable_error_reporting_for_unused_labels),
        default_value_description: CompilerOptionsValue::Tristate(Tristate::Unknown),
        ..Default::default()
    }),
    opt(CommandLineOption {
        name: "allowUnreachableCode",
        kind: CommandLineOptionKind::BOOLEAN,
        affects_bind_diagnostics: true,
        affects_semantic_diagnostics: true,
        affects_build_info: true,
        category: Some(diag::Type_Checking),
        description: Some(diag::Disable_error_reporting_for_unreachable_code),
        default_value_description: CompilerOptionsValue::Tristate(Tristate::Unknown),
        ..Default::default()
    }),
    opt(CommandLineOption {
        name: "forceConsistentCasingInFileNames",
        kind: CommandLineOptionKind::BOOLEAN,
        affects_module_resolution: true,
        category: Some(diag::Interop_Constraints),
        description: Some(diag::Ensure_that_casing_is_correct_in_imports),
        default_value_description: CompilerOptionsValue::Bool(true),
        ..Default::default()
    }),
    opt(CommandLineOption {
        name: "maxNodeModuleJsDepth",
        kind: CommandLineOptionKind::NUMBER,
        affects_module_resolution: true,
        category: Some(diag::JavaScript_Support),
        description: Some(diag::Specify_the_maximum_folder_depth_used_for_checking_JavaScript_files_from_node_modules_Only_applicable_with_allowJs),
        default_value_description: CompilerOptionsValue::Int(0),
        ..Default::default()
    }),
    opt(CommandLineOption {
        name: "useDefineForClassFields",
        kind: CommandLineOptionKind::BOOLEAN,
        affects_semantic_diagnostics: true,
        affects_emit: true,
        affects_build_info: true,
        category: Some(diag::Language_and_Environment),
        description: Some(diag::Emit_ECMAScript_standard_compliant_class_fields),
        default_value_description: CompilerOptionsValue::Message(diag::X_true_for_ES2022_and_above_including_ESNext),
        ..Default::default()
    }),
    opt(CommandLineOption {
        // A list of plugins to load in the language service
        name: "plugins",
        kind: CommandLineOptionKind::LIST,
        is_ts_config_only: true,
        description: Some(diag::Specify_a_list_of_language_service_plugins_to_include),
        category: Some(diag::Editor_Support),
        ..Default::default()
    }),
    opt(CommandLineOption {
        name: "moduleDetection",
        kind: CommandLineOptionKind::ENUM,
        affects_source_file: true,
        affects_module_resolution: true,
        description: Some(diag::Control_what_method_is_used_to_detect_module_format_JS_files),
        category: Some(diag::Language_and_Environment),
        default_value_description: CompilerOptionsValue::Message(diag::X_auto_Colon_Treat_files_with_imports_exports_import_meta_jsx_with_jsx_Colon_react_jsx_or_esm_format_with_module_Colon_node16_as_modules),
        ..Default::default()
    }),
    opt(CommandLineOption {
        name: "ignoreDeprecations",
        kind: CommandLineOptionKind::STRING,
        default_value_description: CompilerOptionsValue::Tristate(Tristate::Unknown),
        ..Default::default()
    }),
    ]
});

// Go: tsoptions/declscompiler.go:1211 optionsType
// PORT: ts#64457 removes this Go function (fed0bf24149f compares each field
// in options_generated.go). The port keeps it; the results are the same.
// PORT: Go reads `core.CompilerOptions` fields by reflection. This returns
// the exported fields in Go declaration order, as (Go field name, value).
// The Go field index `i` is the index in this list (the unexported
// `noCopy` field is not in it).
#[must_use]
pub fn compiler_options_field_values(
    o: &CompilerOptions,
) -> Vec<(&'static str, CompilerOptionsValue)> {
    use CompilerOptionsValue as V;
    // PORT: Go `[]string` fields are `Option<Vec<String>>` in Rust and
    // `None` is listed as empty here, so nil and empty compare equal. Go
    // nil and empty slices differ under `reflect.DeepEqual`.
    vec![
        ("AllowJs", V::Tristate(o.allow_js)),
        (
            "AllowArbitraryExtensions",
            V::Tristate(o.allow_arbitrary_extensions),
        ),
        (
            "AllowImportingTsExtensions",
            V::Tristate(o.allow_importing_ts_extensions),
        ),
        (
            "AllowNonTsExtensions",
            V::Tristate(o.allow_non_ts_extensions),
        ),
        (
            "AllowUmdGlobalAccess",
            V::Tristate(o.allow_umd_global_access),
        ),
        (
            "AllowUnreachableCode",
            V::Tristate(o.allow_unreachable_code),
        ),
        ("AllowUnusedLabels", V::Tristate(o.allow_unused_labels)),
        (
            "AssumeChangesOnlyAffectDirectDependencies",
            V::Tristate(o.assume_changes_only_affect_direct_dependencies),
        ),
        ("CheckJs", V::Tristate(o.check_js)),
        (
            "CustomConditions",
            V::StringList(o.custom_conditions.clone().unwrap_or_default()),
        ),
        ("Composite", V::Tristate(o.composite)),
        ("EmitDeclarationOnly", V::Tristate(o.emit_declaration_only)),
        ("EmitBOM", V::Tristate(o.emit_bom)),
        (
            "EmitDecoratorMetadata",
            V::Tristate(o.emit_decorator_metadata),
        ),
        ("Declaration", V::Tristate(o.declaration)),
        ("DeclarationDir", V::String(o.declaration_dir.clone())),
        ("DeclarationMap", V::Tristate(o.declaration_map)),
        ("DeduplicatePackages", V::Tristate(o.deduplicate_packages)),
        ("DisableSizeLimit", V::Tristate(o.disable_size_limit)),
        (
            "DisableSourceOfProjectReferenceRedirect",
            V::Tristate(o.disable_source_of_project_reference_redirect),
        ),
        (
            "DisableSolutionSearching",
            V::Tristate(o.disable_solution_searching),
        ),
        (
            "DisableReferencedProjectLoad",
            V::Tristate(o.disable_referenced_project_load),
        ),
        ("ErasableSyntaxOnly", V::Tristate(o.erasable_syntax_only)),
        (
            "ExactOptionalPropertyTypes",
            V::Tristate(o.exact_optional_property_types),
        ),
        (
            "ExperimentalDecorators",
            V::Tristate(o.experimental_decorators),
        ),
        (
            "ForceConsistentCasingInFileNames",
            V::Tristate(o.force_consistent_casing_in_file_names),
        ),
        ("IsolatedModules", V::Tristate(o.isolated_modules)),
        ("IsolatedDeclarations", V::Tristate(o.isolated_declarations)),
        ("IgnoreConfig", V::Tristate(o.ignore_config)),
        (
            "IgnoreDeprecations",
            V::String(o.ignore_deprecations.clone()),
        ),
        ("ImportHelpers", V::Tristate(o.import_helpers)),
        ("InlineSourceMap", V::Tristate(o.inline_source_map)),
        ("InlineSources", V::Tristate(o.inline_sources)),
        ("Init", V::Tristate(o.init)),
        ("Incremental", V::Tristate(o.incremental)),
        ("Jsx", V::JsxEmit(o.jsx)),
        ("JsxFactory", V::String(o.jsx_factory.clone())),
        (
            "JsxFragmentFactory",
            V::String(o.jsx_fragment_factory.clone()),
        ),
        ("JsxImportSource", V::String(o.jsx_import_source.clone())),
        ("Lib", V::StringList(o.lib.clone().unwrap_or_default())),
        ("LibReplacement", V::Tristate(o.lib_replacement)),
        ("Locale", V::String(o.locale.clone())),
        ("MapRoot", V::String(o.map_root.clone())),
        ("Module", V::ModuleKind(o.module)),
        (
            "ModuleResolution",
            V::ModuleResolutionKind(o.module_resolution),
        ),
        (
            "ModuleSuffixes",
            V::StringList(o.module_suffixes.clone().unwrap_or_default()),
        ),
        (
            "ModuleDetection",
            V::ModuleDetectionKind(o.module_detection),
        ),
        ("NewLine", V::NewLineKind(o.new_line)),
        ("NoEmit", V::Tristate(o.no_emit)),
        ("NoCheck", V::Tristate(o.no_check)),
        ("NoErrorTruncation", V::Tristate(o.no_error_truncation)),
        (
            "NoFallthroughCasesInSwitch",
            V::Tristate(o.no_fallthrough_cases_in_switch),
        ),
        ("NoImplicitAny", V::Tristate(o.no_implicit_any)),
        ("NoImplicitThis", V::Tristate(o.no_implicit_this)),
        ("NoImplicitReturns", V::Tristate(o.no_implicit_returns)),
        ("NoEmitHelpers", V::Tristate(o.no_emit_helpers)),
        ("NoLib", V::Tristate(o.no_lib)),
        (
            "NoPropertyAccessFromIndexSignature",
            V::Tristate(o.no_property_access_from_index_signature),
        ),
        (
            "NoUncheckedIndexedAccess",
            V::Tristate(o.no_unchecked_indexed_access),
        ),
        ("NoEmitOnError", V::Tristate(o.no_emit_on_error)),
        ("NoUnusedLocals", V::Tristate(o.no_unused_locals)),
        ("NoUnusedParameters", V::Tristate(o.no_unused_parameters)),
        ("NoResolve", V::Tristate(o.no_resolve)),
        ("NoImplicitOverride", V::Tristate(o.no_implicit_override)),
        (
            "NoUncheckedSideEffectImports",
            V::Tristate(o.no_unchecked_side_effect_imports),
        ),
        ("OutDir", V::String(o.out_dir.clone())),
        ("Paths", V::Paths(o.paths.clone())),
        // ts#64397: Go `[]PluginImport`. Each element is its JSON form, a
        // map with "name" (Go `reflect.DeepEqual` compares the names). A
        // `None` is listed as empty, like the `[]string` fields.
        (
            "Plugins",
            V::List(
                o.plugins
                    .iter()
                    .flatten()
                    .map(|plugin| {
                        V::Map(IndexMap::from_iter([(
                            "name".to_string(),
                            V::String(plugin.name.clone()),
                        )]))
                    })
                    .collect(),
            ),
        ),
        ("PreserveConstEnums", V::Tristate(o.preserve_const_enums)),
        ("PreserveSymlinks", V::Tristate(o.preserve_symlinks)),
        ("Project", V::String(o.project.clone())),
        ("ResolveJsonModule", V::Tristate(o.resolve_json_module)),
        (
            "ResolvePackageJsonExports",
            V::Tristate(o.resolve_package_json_exports),
        ),
        (
            "ResolvePackageJsonImports",
            V::Tristate(o.resolve_package_json_imports),
        ),
        ("RemoveComments", V::Tristate(o.remove_comments)),
        (
            "RewriteRelativeImportExtensions",
            V::Tristate(o.rewrite_relative_import_extensions),
        ),
        ("ReactNamespace", V::String(o.react_namespace.clone())),
        ("RootDir", V::String(o.root_dir.clone())),
        (
            "RootDirs",
            V::StringList(o.root_dirs.clone().unwrap_or_default()),
        ),
        ("SkipLibCheck", V::Tristate(o.skip_lib_check)),
        ("StableTypeOrdering", V::Tristate(o.stable_type_ordering)),
        ("Strict", V::Tristate(o.strict)),
        ("StrictBindCallApply", V::Tristate(o.strict_bind_call_apply)),
        (
            "StrictBuiltinIteratorReturn",
            V::Tristate(o.strict_builtin_iterator_return),
        ),
        ("StrictFunctionTypes", V::Tristate(o.strict_function_types)),
        ("StrictNullChecks", V::Tristate(o.strict_null_checks)),
        (
            "StrictPropertyInitialization",
            V::Tristate(o.strict_property_initialization),
        ),
        ("StripInternal", V::Tristate(o.strip_internal)),
        ("SkipDefaultLibCheck", V::Tristate(o.skip_default_lib_check)),
        ("SourceMap", V::Tristate(o.source_map)),
        ("SourceRoot", V::String(o.source_root.clone())),
        (
            "SuppressOutputPathCheck",
            V::Tristate(o.suppress_output_path_check),
        ),
        ("Target", V::ScriptTarget(o.target)),
        ("TraceResolution", V::Tristate(o.trace_resolution)),
        ("TsBuildInfoFile", V::String(o.ts_build_info_file.clone())),
        (
            "TypeRoots",
            V::StringList(o.type_roots.clone().unwrap_or_default()),
        ),
        ("Types", V::StringList(o.types.clone().unwrap_or_default())),
        (
            "UseDefineForClassFields",
            V::Tristate(o.use_define_for_class_fields),
        ),
        (
            "UseUnknownInCatchVariables",
            V::Tristate(o.use_unknown_in_catch_variables),
        ),
        (
            "VerbatimModuleSyntax",
            V::Tristate(o.verbatim_module_syntax),
        ),
        (
            "MaxNodeModuleJsDepth",
            V::IntPtr(o.max_node_module_js_depth.clone()),
        ),
        (
            "AllowSyntheticDefaultImports",
            V::Tristate(o.allow_synthetic_default_imports),
        ),
        ("AlwaysStrict", V::Tristate(o.always_strict)),
        ("BaseUrl", V::String(o.base_url.clone())),
        ("DownlevelIteration", V::Tristate(o.downlevel_iteration)),
        ("ESModuleInterop", V::Tristate(o.es_module_interop)),
        ("OutFile", V::String(o.out_file.clone())),
        ("ConfigFilePath", V::String(o.config_file_path.clone())),
        ("NoDtsResolution", V::Tristate(o.no_dts_resolution)),
        ("PathsBasePath", V::String(o.paths_base_path.clone())),
        ("Diagnostics", V::Tristate(o.diagnostics)),
        ("ExtendedDiagnostics", V::Tristate(o.extended_diagnostics)),
        (
            "GenerateCpuProfile",
            V::String(o.generate_cpu_profile.clone()),
        ),
        ("GenerateTrace", V::String(o.generate_trace.clone())),
        ("ListEmittedFiles", V::Tristate(o.list_emitted_files)),
        ("ListFiles", V::Tristate(o.list_files)),
        ("ExplainFiles", V::Tristate(o.explain_files)),
        ("ListFilesOnly", V::Tristate(o.list_files_only)),
        ("NoEmitForJsFiles", V::Tristate(o.no_emit_for_js_files)),
        ("PreserveWatchOutput", V::Tristate(o.preserve_watch_output)),
        ("Pretty", V::Tristate(o.pretty)),
        ("Version", V::Tristate(o.version)),
        ("Watch", V::Tristate(o.watch)),
        ("ShowConfig", V::Tristate(o.show_config)),
        ("Build", V::Tristate(o.build)),
        ("Help", V::Tristate(o.help)),
        ("All", V::Tristate(o.all)),
        ("RunExternalCode", V::Tristate(o.run_external_code)),
        ("PprofDir", V::String(o.pprof_dir.clone())),
        ("SingleThreaded", V::Tristate(o.single_threaded)),
        ("Quiet", V::Tristate(o.quiet)),
        ("Checkers", V::IntPtr(o.checkers.clone())),
    ]
}

// Go: tsoptions/declscompiler.go:1213 optionsHaveChanges
// PORT: ts#64457 removes this Go function (fed0bf24149f compares each field
// in options_generated.go). The port keeps it; the results are the same.
#[must_use]
pub fn options_have_changes(
    old_options: Option<&CompilerOptions>,
    new_options: Option<&CompilerOptions>,
    decl_filter: &dyn Fn(&CommandLineOption) -> bool,
) -> bool {
    // PORT: Go compares the two pointers first. Rust compares addresses.
    match (old_options, new_options) {
        (None, None) => return false,
        (Some(a), Some(b)) if std::ptr::eq(a, b) => return false,
        _ => {}
    }
    let (Some(old_options), Some(new_options)) = (old_options, new_options) else {
        return true;
    };
    let old_options_value = compiler_options_field_values(old_options);
    for_each_compiler_option_value(new_options, decl_filter, &mut |option, new_value, i| {
        let old_value = &old_options_value[i].1;
        if option.strict_flag {
            let (
                CompilerOptionsValue::Tristate(old_value),
                CompilerOptionsValue::Tristate(new_value),
            ) = (old_value, &new_value)
            else {
                panic!("strict option is not a Tristate");
            };
            return old_options.get_strict_option_value(*old_value)
                != new_options.get_strict_option_value(*new_value);
        }
        if option.allow_js_flag {
            return old_options.get_allow_js() != new_options.get_allow_js();
        }
        new_value != *old_value
    })
}

// Go: tsoptions/declscompiler.go:1234 ForEachCompilerOptionValue
// PORT: ts#64457 removes this Go function (fed0bf24149f compares each field
// in options_generated.go). The port keeps it; the results are the same.
pub fn for_each_compiler_option_value(
    options: &CompilerOptions,
    decl_filter: &dyn Fn(&CommandLineOption) -> bool,
    f: &mut dyn FnMut(&'static CommandLineOption, CompilerOptionsValue, usize) -> bool,
) -> bool {
    for (i, (field_name, value)) in compiler_options_field_values(options)
        .into_iter()
        .enumerate()
    {
        if let Some(option_declaration) = COMMAND_LINE_COMPILER_OPTIONS_MAP.get(field_name) {
            if decl_filter(option_declaration) && f(option_declaration, value, i) {
                return true;
            }
        }
    }
    false
}

// Go: tsoptions/options_generated.go:368 CompilerOptionsAffectSemanticDiagnostics
#[must_use]
pub fn compiler_options_affect_semantic_diagnostics(
    old_options: Option<&CompilerOptions>,
    new_options: Option<&CompilerOptions>,
) -> bool {
    // Effect-TS/tsgo patch 028: Effect plugin options affect Effect diagnostics.
    // Without the rules (a standalone API process) they affect nothing, and
    // its build info records none (`rulerunner::enabled_options`).
    use crate::effect::rulerunner::enabled_options;
    if old_options.and_then(enabled_options) != new_options.and_then(enabled_options) {
        return true;
    }
    options_have_changes(old_options, new_options, &|option| {
        option.affects_semantic_diagnostics
    })
}

// Go: tsoptions/options_generated.go:413 CompilerOptionsAffectDeclarationPath
#[must_use]
pub fn compiler_options_affect_declaration_path(
    old_options: Option<&CompilerOptions>,
    new_options: Option<&CompilerOptions>,
) -> bool {
    options_have_changes(old_options, new_options, &|option| {
        option.affects_declaration_path
    })
}

// Go: tsoptions/options_generated.go:426 CompilerOptionsAffectEmit
#[must_use]
pub fn compiler_options_affect_emit(
    old_options: Option<&CompilerOptions>,
    new_options: Option<&CompilerOptions>,
) -> bool {
    options_have_changes(old_options, new_options, &|option| option.affects_emit)
}

// ---------------------------------------------------------------------------
// tsoptions/namemap.go
// ---------------------------------------------------------------------------

// Go: tsoptions/namemap.go:10 CompilerNameMap
pub static COMPILER_NAME_MAP: LazyLock<NameMap> =
    LazyLock::new(|| get_name_map_from_list(&OPTIONS_DECLARATIONS));
// Go: tsoptions/namemap.go:11 BuildNameMap
pub static BUILD_NAME_MAP: LazyLock<NameMap> =
    LazyLock::new(|| get_name_map_from_list(&BUILD_OPTS));

// Go: tsoptions/namemap.go:14 GetNameMapFromList
#[must_use]
pub fn get_name_map_from_list(opt_decls: &[&'static CommandLineOption]) -> NameMap {
    let mut options_names: IndexMap<String, &'static CommandLineOption> =
        IndexMap::with_capacity(opt_decls.len());
    let mut short_option_names: FxHashMap<String, String> = FxHashMap::default();
    for &option in opt_decls {
        options_names.insert(option.name.to_lowercase(), option);
        if !option.short_name.is_empty() {
            short_option_names.insert(option.short_name.to_string(), option.name.to_string());
        }
    }
    NameMap {
        options_names,
        short_option_names,
    }
}

// Go: tsoptions/namemap.go:29 NameMap
// PORT: Go `OrderedMap.Set` on an existing key keeps the first position and
// replaces the value. `IndexMap::insert` does the same.
#[derive(Clone, Debug, Default)]
pub struct NameMap {
    pub options_names: IndexMap<String, &'static CommandLineOption>,
    pub short_option_names: FxHashMap<String, String>,
}

impl NameMap {
    // Go: tsoptions/namemap.go:34 Get
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&'static CommandLineOption> {
        self.options_names.get(&name.to_lowercase()).copied()
    }

    // Go: tsoptions/namemap.go:38 GetFromShort
    #[must_use]
    pub fn get_from_short(&self, short_name: &str) -> Option<&'static CommandLineOption> {
        // returns option only if shortName is a valid short option
        let Some(name) = self.short_option_names.get(short_name) else {
            return None;
        };
        self.get(name)
    }

    // Go: tsoptions/namemap.go:47 GetOptionDeclarationFromName
    #[must_use]
    pub fn get_option_declaration_from_name(
        &self,
        option_name: &str,
        allow_short: bool,
    ) -> Option<&'static CommandLineOption> {
        let mut option_name = option_name.to_lowercase();
        // Try to translate short option names to their full equivalents.
        if allow_short {
            if let Some(short) = self.short_option_names.get(&option_name) {
                if !short.is_empty() {
                    option_name = short.clone();
                }
            }
        }
        self.get(&option_name)
    }
}

// ---------------------------------------------------------------------------
// tsoptions/declsbuild.go
// ---------------------------------------------------------------------------

// Go: tsoptions/declarations_generated.go:1052 TscBuildOption
pub static TSC_BUILD_OPTION: LazyLock<CommandLineOption> = LazyLock::new(|| CommandLineOption {
    name: "build",
    kind: CommandLineOptionKind::BOOLEAN,
    short_name: "b",
    show_in_simplified_help_view: true,
    category: Some(diag::Command_line_Options),
    description: Some(diag::Build_one_or_more_projects_and_their_dependencies_if_out_of_date),
    default_value_description: CompilerOptionsValue::Bool(false),
    ..Default::default()
});

// Go: tsoptions/declarations_generated.go:1062 OptionsForBuild
pub static OPTIONS_FOR_BUILD: LazyLock<Vec<&'static CommandLineOption>> = LazyLock::new(|| {
    vec![
        &*TSC_BUILD_OPTION,
        opt(CommandLineOption {
            name: "verbose",
            short_name: "v",
            category: Some(diag::Command_line_Options),
            description: Some(diag::Enable_verbose_logging),
            kind: CommandLineOptionKind::BOOLEAN,
            default_value_description: CompilerOptionsValue::Bool(false),
            ..Default::default()
        }),
        opt(CommandLineOption {
            name: "dry",
            short_name: "d",
            category: Some(diag::Command_line_Options),
            description: Some(diag::Show_what_would_be_built_or_deleted_if_specified_with_clean),
            kind: CommandLineOptionKind::BOOLEAN,
            default_value_description: CompilerOptionsValue::Bool(false),
            ..Default::default()
        }),
        opt(CommandLineOption {
            name: "force",
            short_name: "f",
            category: Some(diag::Command_line_Options),
            description: Some(
                diag::Build_all_projects_including_those_that_appear_to_be_up_to_date,
            ),
            kind: CommandLineOptionKind::BOOLEAN,
            default_value_description: CompilerOptionsValue::Bool(false),
            ..Default::default()
        }),
        opt(CommandLineOption {
            name: "clean",
            category: Some(diag::Command_line_Options),
            description: Some(diag::Delete_the_outputs_of_all_projects),
            kind: CommandLineOptionKind::BOOLEAN,
            default_value_description: CompilerOptionsValue::Bool(false),
            ..Default::default()
        }),
        opt(CommandLineOption {
            name: "builders",
            kind: CommandLineOptionKind::NUMBER,
            category: Some(diag::Command_line_Options),
            description: Some(diag::Set_the_number_of_projects_to_build_concurrently),
            default_value_description: CompilerOptionsValue::Message(
                diag::X_4_unless_singleThreaded_is_passed,
            ),
            min_value: 1,
            ..Default::default()
        }),
        opt(CommandLineOption {
            name: "stopBuildOnErrors",
            category: Some(diag::Command_line_Options),
            description: Some(diag::Skip_building_downstream_projects_on_error_in_upstream_project),
            kind: CommandLineOptionKind::BOOLEAN,
            default_value_description: CompilerOptionsValue::Bool(false),
            ..Default::default()
        }),
    ]
});

// Go: tsoptions/declarations_generated.go:15 BuildOpts
pub static BUILD_OPTS: LazyLock<Vec<&'static CommandLineOption>> =
    LazyLock::new(|| concat_options(&COMMON_OPTIONS_WITH_BUILD, &OPTIONS_FOR_BUILD));

// ---------------------------------------------------------------------------
// tsoptions/declstypeacquisition.go
// ---------------------------------------------------------------------------

// Go: tsoptions/declarations_generated.go:1118 typeAcquisitionDeclaration
pub static TYPE_ACQUISITION_DECLARATION: LazyLock<&'static CommandLineOption> =
    LazyLock::new(|| {
        opt(CommandLineOption {
            name: "typeAcquisition",
            kind: CommandLineOptionKind::OBJECT,
            element_options: command_line_options_to_map(&TYPE_ACQUISITION_DECLS),
            ..Default::default()
        })
    });

// Do not delete this without updating the website's tsconfig generation.
// Go: tsoptions/declarations_generated.go:963 typeAcquisitionDecls
pub static TYPE_ACQUISITION_DECLS: LazyLock<Vec<&'static CommandLineOption>> =
    LazyLock::new(|| {
        vec![
            opt(CommandLineOption {
                name: "enable",
                kind: CommandLineOptionKind::BOOLEAN,
                default_value_description: CompilerOptionsValue::Bool(false),
                ..Default::default()
            }),
            opt(CommandLineOption {
                name: "include",
                kind: CommandLineOptionKind::LIST,
                ..Default::default()
            }),
            opt(CommandLineOption {
                name: "exclude",
                kind: CommandLineOptionKind::LIST,
                ..Default::default()
            }),
            opt(CommandLineOption {
                name: "disableFilenameBasedTypeAcquisition",
                kind: CommandLineOptionKind::BOOLEAN,
                default_value_description: CompilerOptionsValue::Bool(false),
                ..Default::default()
            }),
        ]
    });
