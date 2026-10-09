//! Go: execute/tsc/help.go (`--version`, `--help`, `--help --all` and
//! `--build --help` output).
//!
//! PORT: Go builds the output as a `[]string` of chunks and writes each one
//! with `fmt.Fprint`; the port does the same with `Vec<String>` and
//! `write_str`. Each Go `Message.Localize(locale, ...)` is
//! `message_localize`.

use crate::prelude::*;

use super::compile::{System, write_str};
use super::diagnostics::{Colors, create_colors, go_pad, go_repeat};
use crate::diagnostics::Message;
use crate::diagnostics_loc::message_localize;
use crate::frontend::tsoptions::{
    CommandLineOption, CommandLineOptionKind, CompilerOptionsValue, OPTIONS_DECLARATIONS,
    OPTIONS_FOR_BUILD, ParsedCommandLine, TSC_BUILD_OPTION,
};
use crate::locale::Locale;

// Go: execute/tsc/help.go:15 PrintVersion
pub fn print_version(sys: &dyn System, locale: &Locale) {
    write_str(
        &sys.writer(),
        &format!(
            "{}\n",
            message_localize(diag::Version_0, locale, &args![version()])
        ),
    );
}

// Go: execute/tsc/help.go:19 PrintHelp
pub fn print_help(sys: &dyn System, locale: &Locale, command_line: &ParsedCommandLine) {
    if command_line.compiler_options().all.is_false_or_unknown() {
        print_easy_help(sys, locale, &get_options_for_help(command_line));
    } else {
        print_all_help(sys, locale, &get_options_for_help(command_line));
    }
}

// Go: execute/tsc/help.go:27 getOptionsForHelp
// PORT: Go `slices.SortFunc` is pdqsort, which is not stable. The input is
// the fixed declaration list, and its only names that are equal in lower
// case are the two `help` options (`-h`, then `-?`). Go keeps them in that
// order (checked against the oracle `--help --all` output).
// `gostd::slices::sort_func` is the same pdqsort.
fn get_options_for_help(command_line: &ParsedCommandLine) -> Vec<&'static CommandLineOption> {
    // Sort our options by their names, (e.g. "--noImplicitAny" comes before "--watch")
    let mut opts: Vec<&'static CommandLineOption> = OPTIONS_DECLARATIONS.to_vec();
    opts.push(&*TSC_BUILD_OPTION);

    if command_line.compiler_options().all.is_true() {
        // Go: execute/tsc/help.go:33 slices.SortFunc(opts, strings.Compare on the lower case names)
        crate::gostd::slices::sort_func(&mut opts, |a, b| {
            a.name.to_lowercase().cmp(&b.name.to_lowercase()) as i32
        });
        opts
    } else {
        opts.into_iter()
            .filter(|opt| opt.show_in_simplified_help_view)
            .collect()
    }
}

// Go: execute/tsc/help.go:44 getHeader
// PORT: Go `getHeader` is unexported; init.rs uses it too.
pub(super) fn get_header(sys: &dyn System, message: &str) -> Vec<String> {
    let colors = create_colors(sys);
    let mut header = Vec::with_capacity(3);
    let terminal_width = sys.get_width_of_terminal();
    const TS_ICON: &str = "     ";
    const TS_ICON_TS: &str = "  TS ";
    const TS_ICON_LENGTH: i32 = TS_ICON.len() as i32;

    let ts_icon_first_line = colors.blue_background(TS_ICON);
    let ts_icon_second_line = colors.blue_background(&colors.bright_white(TS_ICON_TS));
    // If we have enough space, print TS icon.
    if terminal_width >= message.len() as i32 + TS_ICON_LENGTH {
        // right align of the icon is 120 at most.
        let right_align = if terminal_width > 120 {
            120
        } else {
            terminal_width
        };
        let left_align = right_align - TS_ICON_LENGTH;
        header.extend([
            go_pad(message, left_align, true),
            ts_icon_first_line,
            "\n".to_string(),
        ]);
        header.extend([
            go_repeat(" ", left_align),
            ts_icon_second_line,
            "\n".to_string(),
        ]);
    } else {
        header.extend([message.to_string(), "\n".to_string(), "\n".to_string()]);
    }
    header
}

// Go: execute/tsc/help.go:67 printEasyHelp
fn print_easy_help(
    sys: &dyn System,
    locale: &Locale,
    simple_options: &[&'static CommandLineOption],
) {
    let colors = create_colors(sys);
    let mut output: Vec<String> = Vec::new();
    // PORT: the Go closure appends to `output`; here it takes `output`.
    let example = |output: &mut Vec<String>, examples: &[&str], desc: &'static Message| {
        for example in examples {
            output.extend(["  ".to_string(), colors.blue(example), "\n".to_string()]);
        }
        output.extend([
            "  ".to_string(),
            message_localize(desc, locale, &[]),
            "\n".to_string(),
            "\n".to_string(),
        ]);
    };

    let msg = format!(
        "{} - {}",
        message_localize(diag::X_tsc_Colon_The_TypeScript_Compiler, locale, &[]),
        message_localize(diag::Version_0, locale, &args![version()])
    );
    output.extend(get_header(sys, &msg));

    output.extend([
        colors.bold(&message_localize(diag::COMMON_COMMANDS, locale, &[])),
        "\n".to_string(),
        "\n".to_string(),
    ]);

    example(
        &mut output,
        &["tsc"],
        diag::Compiles_the_current_project_tsconfig_json_in_the_working_directory,
    );
    example(
        &mut output,
        &["tsc app.ts util.ts"],
        diag::Ignoring_tsconfig_json_compiles_the_specified_files_with_default_compiler_options,
    );
    example(
        &mut output,
        &["tsc -b"],
        diag::Build_a_composite_project_in_the_working_directory,
    );
    example(
        &mut output,
        &["tsc --init"],
        diag::Creates_a_tsconfig_json_with_the_recommended_settings_in_the_working_directory,
    );
    example(
        &mut output,
        &["tsc -p ./path/to/tsconfig.json"],
        diag::Compiles_the_TypeScript_project_located_at_the_specified_path,
    );
    example(
        &mut output,
        &["tsc --help --all"],
        diag::An_expanded_version_of_this_information_showing_all_possible_compiler_options,
    );
    example(
        &mut output,
        &["tsc --noEmit", "tsc --target esnext"],
        diag::Compiles_the_current_project_with_additional_settings,
    );

    let mut cli_commands: Vec<&'static CommandLineOption> = Vec::new();
    let mut config_opts: Vec<&'static CommandLineOption> = Vec::new();
    for &opt in simple_options {
        if opt.is_command_line_only
            || opt
                .category
                .is_some_and(|category| std::ptr::eq(category, diag::Command_line_Options))
        {
            cli_commands.push(opt);
        } else {
            config_opts.push(opt);
        }
    }

    output.extend(generate_section_options_output(
        sys,
        locale,
        &message_localize(diag::COMMAND_LINE_FLAGS, locale, &[]),
        &cli_commands,
        false, /*subCategory*/
        None,  /*beforeOptionsDescription*/
        None,  /*afterOptionsDescription*/
    ));

    let after = message_localize(
        diag::You_can_learn_about_all_of_the_compiler_options_at_0,
        locale,
        &args!["https://aka.ms/tsc"],
    );
    output.extend(generate_section_options_output(
        sys,
        locale,
        &message_localize(diag::COMMON_COMPILER_OPTIONS, locale, &[]),
        &config_opts,
        false, /*subCategory*/
        None,  /*beforeOptionsDescription*/
        Some(&after),
    ));

    for chunk in &output {
        write_str(&sys.writer(), chunk);
    }
}

// Go: execute/tsc/help.go:110 printAllHelp
fn print_all_help(sys: &dyn System, locale: &Locale, options: &[&'static CommandLineOption]) {
    let mut output: Vec<String> = Vec::new();
    let msg = format!(
        "{} - {}",
        message_localize(diag::X_tsc_Colon_The_TypeScript_Compiler, locale, &[]),
        message_localize(diag::Version_0, locale, &args![version()])
    );
    output.extend(get_header(sys, &msg));

    // ALL COMPILER OPTIONS section
    let after_compiler_options = message_localize(
        diag::You_can_learn_about_all_of_the_compiler_options_at_0,
        locale,
        &args!["https://aka.ms/tsc"],
    );
    output.extend(generate_section_options_output(
        sys,
        locale,
        &message_localize(diag::ALL_COMPILER_OPTIONS, locale, &[]),
        options,
        true,
        None,
        Some(&after_compiler_options),
    ));

    // ts#64457: the WATCH OPTIONS section is gone with the watch options.

    // BUILD OPTIONS section
    let before_build_options = message_localize(
        diag::Using_build_b_will_make_tsc_behave_more_like_a_build_orchestrator_than_a_compiler_This_is_used_to_trigger_building_composite_projects_which_you_can_learn_more_about_at_0,
        locale,
        &args!["https://aka.ms/tsc-composite-builds"],
    );
    let build_options: Vec<&'static CommandLineOption> = OPTIONS_FOR_BUILD
        .iter()
        .copied()
        .filter(|option| !std::ptr::eq(*option, &*TSC_BUILD_OPTION))
        .collect();
    output.extend(generate_section_options_output(
        sys,
        locale,
        &message_localize(diag::BUILD_OPTIONS, locale, &[]),
        &build_options,
        false,
        Some(&before_build_options),
        None,
    ));

    for chunk in &output {
        write_str(&sys.writer(), chunk);
    }
}

// Go: execute/tsc/help.go:131 PrintBuildHelp
pub fn print_build_help(
    sys: &dyn System,
    locale: &Locale,
    build_options: &[&'static CommandLineOption],
) {
    let mut output: Vec<String> = Vec::new();
    output.extend(get_header(
        sys,
        &format!(
            "{} - {}",
            message_localize(diag::X_tsc_Colon_The_TypeScript_Compiler, locale, &[]),
            message_localize(diag::Version_0, locale, &args![version()])
        ),
    ));
    let before = message_localize(
        diag::Using_build_b_will_make_tsc_behave_more_like_a_build_orchestrator_than_a_compiler_This_is_used_to_trigger_building_composite_projects_which_you_can_learn_more_about_at_0,
        locale,
        &args!["https://aka.ms/tsc-composite-builds"],
    );
    let options: Vec<&'static CommandLineOption> = build_options
        .iter()
        .copied()
        .filter(|option| !std::ptr::eq(*option, &*TSC_BUILD_OPTION))
        .collect();
    output.extend(generate_section_options_output(
        sys,
        locale,
        &message_localize(diag::BUILD_OPTIONS, locale, &[]),
        &options,
        false,
        Some(&before),
        None,
    ));

    for chunk in &output {
        write_str(&sys.writer(), chunk);
    }
}

// Go: execute/tsc/help.go:145 generateSectionOptionsOutput
fn generate_section_options_output(
    sys: &dyn System,
    locale: &Locale,
    section_name: &str,
    options: &[&'static CommandLineOption],
    sub_category: bool,
    before_options_description: Option<&str>,
    after_options_description: Option<&str>,
) -> Vec<String> {
    let mut output = vec![
        create_colors(sys).bold(section_name),
        "\n".to_string(),
        "\n".to_string(),
    ];

    if let Some(before_options_description) = before_options_description {
        output.extend([
            before_options_description.to_string(),
            "\n".to_string(),
            "\n".to_string(),
        ]);
    }
    if !sub_category {
        output.extend(generate_group_option_output(sys, locale, options));
        if let Some(after_options_description) = after_options_description {
            output.extend([
                after_options_description.to_string(),
                "\n".to_string(),
                "\n".to_string(),
            ]);
        }
        return output;
    }
    // PORT: Go keeps a map and a separate `categoryOrder` slice. An
    // `IndexMap` keeps both.
    let mut category_map: IndexMap<String, Vec<&'static CommandLineOption>> = IndexMap::new();
    for &option in options {
        let Some(category) = option.category else {
            continue;
        };
        let cur_category = message_localize(category, locale, &[]);
        category_map.entry(cur_category).or_default().push(option);
    }
    for (key, value) in &category_map {
        output.extend([
            "### ".to_string(),
            key.clone(),
            "\n".to_string(),
            "\n".to_string(),
        ]);
        output.extend(generate_group_option_output(sys, locale, value));
    }
    if let Some(after_options_description) = after_options_description {
        output.extend([
            after_options_description.to_string(),
            "\n".to_string(),
            "\n".to_string(),
        ]);
    }

    output
}

// Go: execute/tsc/help.go:190 generateGroupOptionOutput
fn generate_group_option_output(
    sys: &dyn System,
    locale: &Locale,
    options_list: &[&'static CommandLineOption],
) -> Vec<String> {
    let mut max_length = 0;
    for option in options_list {
        let cur_lenght = get_display_name_text_of_option(option).len() as i32;
        max_length = max_length.max(cur_lenght);
    }

    // left part should be right align, right part should be left align

    // assume 2 space between left margin and left part.
    let right_align_of_left_part = max_length + 2;
    // assume 2 space between left and right part
    let left_align_of_right_part = right_align_of_left_part + 2;

    let mut lines = Vec::new();
    for option in options_list {
        let tmp = generate_option_output(
            sys,
            locale,
            option,
            right_align_of_left_part,
            left_align_of_right_part,
        );
        lines.extend(tmp);
    }

    // make sure always a blank line in the end.
    if lines.len() < 2 || lines[lines.len() - 2] != "\n" {
        lines.push("\n".to_string());
    }

    lines
}

// Go: execute/tsc/help.go:218 generateOptionOutput
fn generate_option_output(
    sys: &dyn System,
    locale: &Locale,
    option: &CommandLineOption,
    right_align_of_left: i32,
    left_align_of_right: i32,
) -> Vec<String> {
    let mut text: Vec<String> = Vec::new();
    let colors = create_colors(sys);

    // name and description
    let name = get_display_name_text_of_option(option);

    // value type and possible value
    let value_candidates = get_value_candidate(sys, locale, option);

    let default_value_description =
        if let CompilerOptionsValue::Message(msg) = option.default_value_description {
            message_localize(msg, locale, &[])
        } else {
            // Go evaluates both `core.IfElse` arguments.
            let elements = option.elements();
            format_default_value(
                &option.default_value_description,
                if option.kind == CommandLineOptionKind::LIST
                    || option.kind == CommandLineOptionKind::LIST_OR_ELEMENT
                {
                    elements
                } else {
                    Some(option)
                },
            )
        };

    let terminal_width = sys.get_width_of_terminal();

    if terminal_width >= 80 {
        let description = match option.description {
            Some(description) => message_localize(description, locale, &[]),
            None => String::new(),
        };
        text.extend(get_pretty_output(
            &colors,
            &name,
            &description,
            right_align_of_left,
            left_align_of_right,
            terminal_width,
            true, /*colorLeft*/
        ));
        text.push("\n".to_string());
        if show_additional_info_output(value_candidates.as_ref(), option) {
            if let Some(value_candidates) = &value_candidates {
                text.extend(get_pretty_output(
                    &colors,
                    &value_candidates.value_type,
                    &value_candidates.possible_values,
                    right_align_of_left,
                    left_align_of_right,
                    terminal_width,
                    false, /*colorLeft*/
                ));
                text.push("\n".to_string());
            }
            if !default_value_description.is_empty() {
                text.extend(get_pretty_output(
                    &colors,
                    &message_localize(diag::X_default_Colon, locale, &[]),
                    &default_value_description,
                    right_align_of_left,
                    left_align_of_right,
                    terminal_width,
                    false, /*colorLeft*/
                ));
                text.push("\n".to_string());
            }
        }
        text.push("\n".to_string());
    } else {
        text.extend([colors.blue(&name), "\n".to_string()]);
        if let Some(description) = option.description {
            text.push(message_localize(description, locale, &[]));
        }
        text.push("\n".to_string());
        if show_additional_info_output(value_candidates.as_ref(), option) {
            if let Some(value_candidates) = &value_candidates {
                text.extend([
                    value_candidates.value_type.clone(),
                    " ".to_string(),
                    value_candidates.possible_values.clone(),
                ]);
            }
            if !default_value_description.is_empty() {
                if value_candidates.is_some() {
                    text.push("\n".to_string());
                }
                text.extend([
                    message_localize(diag::X_default_Colon, locale, &[]),
                    " ".to_string(),
                    default_value_description,
                ]);
            }

            text.push("\n".to_string());
        }
        text.push("\n".to_string());
    }

    text
}

// Go: execute/tsc/help.go:291 formatDefaultValue
// PORT: Go `option` is a pointer that can be nil (`Elements()` of a list
// without elements); Go dereferences it after the nil value check.
fn format_default_value(
    default_value: &CompilerOptionsValue,
    option: Option<&CommandLineOption>,
) -> String {
    if default_value.is_nil() || *default_value == CompilerOptionsValue::Tristate(Tristate::Unknown)
    {
        return "undefined".to_string();
    }

    let option = option.expect("nil option dereference");
    if option.kind == CommandLineOptionKind::ENUM {
        // e.g. ScriptTarget.ES2015 -> "es6/es2015"
        let mut names: Vec<&str> = Vec::new();
        for (name, value) in option.enum_map().expect("nil enum map dereference") {
            if value == default_value {
                names.push(name.as_str());
            }
        }
        return names.join("/");
    }
    format_value_v(default_value)
}

/// Go `fmt.Sprintf("%v", value)` for the dynamic types that a Go `any`
/// option value holds.
// PORT: helper for the `%v` in formatDefaultValue (and the panic text in
// init.go `formatSingleValue`). Go `%v` uses a type's `String` method when
// it has one (Tristate, ScriptTarget, ModuleKind, ModuleResolutionKind,
// JsxEmit and `*diagnostics.Message`); the other named integer types print
// their number. Go prints the map and `*int` values as a struct dump or a
// pointer address; help output never reaches them, so they call
// `unported!`.
pub(super) fn format_value_v(value: &CompilerOptionsValue) -> String {
    match value {
        CompilerOptionsValue::Nil => "<nil>".to_string(),
        CompilerOptionsValue::Bool(value) => value.to_string(),
        CompilerOptionsValue::Int(value) => value.to_string(),
        CompilerOptionsValue::Number(value) => format_float_v(*value),
        CompilerOptionsValue::String(value) => value.clone(),
        // Go: diagnostics/diagnostics.go:63 (*Message).String
        CompilerOptionsValue::Message(message) => message.text().to_string(),
        CompilerOptionsValue::Tristate(value) => value.string(),
        CompilerOptionsValue::ScriptTarget(value) => value.string(),
        CompilerOptionsValue::ModuleKind(value) => value.string(),
        CompilerOptionsValue::ModuleResolutionKind(value) => value.string(),
        CompilerOptionsValue::ModuleDetectionKind(value) => value.0.to_string(),
        CompilerOptionsValue::JsxEmit(value) => value.string(),
        CompilerOptionsValue::NewLineKind(value) => value.0.to_string(),
        // Go `[]any` and `[]string`: `[a b c]`.
        CompilerOptionsValue::List(list) => format!(
            "[{}]",
            list.iter()
                .map(format_value_v)
                .collect::<Vec<_>>()
                .join(" ")
        ),
        CompilerOptionsValue::StringList(list) => format!("[{}]", list.join(" ")),
        CompilerOptionsValue::NilList => "[]".to_string(),
        CompilerOptionsValue::Map(_) => unported!("fmt %v of *collections.OrderedMap"),
        CompilerOptionsValue::Paths(None) | CompilerOptionsValue::IntPtr(None) => {
            "<nil>".to_string()
        }
        CompilerOptionsValue::Paths(Some(_)) => unported!("fmt %v of *collections.OrderedMap"),
        CompilerOptionsValue::IntPtr(Some(_)) => unported!("fmt %v of *int"),
        CompilerOptionsValue::EmptyStruct => "{}".to_string(),
    }
}

/// Go `fmt.Sprintf("%v", f)` for a `float64`, which is
/// `strconv.FormatFloat(f, 'g', -1, 64)`.
// PORT: Go writes the shortest digits in `%e` form when the decimal
// exponent is below -4 or at least 6 (with a sign and at least two exponent
// digits, `1e+06`), else in `%f` form. Rust `{:e}` and `{}` also write the
// shortest digits, so only the exponent spelling changes.
fn format_float_v(value: f64) -> String {
    if value.is_nan() {
        return "NaN".to_string();
    }
    if value.is_infinite() {
        return if value > 0.0 { "+Inf" } else { "-Inf" }.to_string();
    }
    let shortest = format!("{value:e}");
    let (mantissa, exponent) = shortest
        .split_once('e')
        .expect("Rust {:e} output has an exponent");
    let exponent: i32 = exponent.parse().expect("Rust {:e} exponent is an integer");
    if exponent < -4 || exponent >= 6 {
        let sign = if exponent < 0 { '-' } else { '+' };
        format!("{mantissa}e{sign}{:02}", exponent.abs())
    } else {
        format!("{value}")
    }
}

// Go: execute/tsc/help.go:309 valueCandidate
struct ValueCandidate {
    // "one or more" or "any of"
    value_type: String,
    possible_values: String,
}

// Go: execute/tsc/help.go:315 showAdditionalInfoOutput
fn show_additional_info_output(
    value_candidates: Option<&ValueCandidate>,
    option: &CommandLineOption,
) -> bool {
    if option
        .category
        .is_some_and(|category| std::ptr::eq(category, diag::Command_line_Options))
    {
        return false;
    }
    if let Some(value_candidates) = value_candidates
        && value_candidates.possible_values == "string"
        && (option.default_value_description.is_nil()
            || matches!(
                &option.default_value_description,
                CompilerOptionsValue::String(value) if value == "false" || value == "n/a"
            ))
    {
        return false;
    }
    true
}

// Go: execute/tsc/help.go:328 getValueCandidate
// PORT: Go takes `sys` and does not use it.
fn get_value_candidate(
    _sys: &dyn System,
    locale: &Locale,
    option: &CommandLineOption,
) -> Option<ValueCandidate> {
    // option.type might be "string" | "number" | "boolean" | "object" | "list" | Map<string, number | string>
    // string -- any of: string
    // number -- any of: number
    // boolean -- any of: boolean
    // object -- null
    // list -- one or more: , content depends on `option.element.type`, the same as others
    // Map<string, number | string> -- any of: key1, key2, ....
    if option.kind == CommandLineOptionKind::OBJECT {
        return None;
    }

    if option.kind == CommandLineOptionKind::LIST_OR_ELEMENT {
        // assert(option.type !== "listOrElement")
        panic!("no value candidate for list or element");
    }

    let value_type = if option.kind == CommandLineOptionKind::STRING
        || option.kind == CommandLineOptionKind::NUMBER
        || option.kind == CommandLineOptionKind::BOOLEAN
    {
        message_localize(diag::X_type_Colon, locale, &[])
    } else if option.kind == CommandLineOptionKind::LIST {
        message_localize(diag::X_one_or_more_Colon, locale, &[])
    } else {
        message_localize(diag::X_one_of_Colon, locale, &[])
    };

    Some(ValueCandidate {
        value_type,
        possible_values: get_possible_values(option),
    })
}

// Go: execute/tsc/help.go:362 getPossibleValues
fn get_possible_values(option: &CommandLineOption) -> String {
    if option.kind == CommandLineOptionKind::STRING
        || option.kind == CommandLineOptionKind::NUMBER
        || option.kind == CommandLineOptionKind::BOOLEAN
    {
        return option.kind.0.to_string();
    }
    if option.kind == CommandLineOptionKind::LIST
        || option.kind == CommandLineOptionKind::LIST_OR_ELEMENT
    {
        return get_possible_values(option.elements().expect("nil option dereference"));
    }
    if option.kind == CommandLineOptionKind::OBJECT {
        return String::new();
    }
    // Map<string, number | string>
    // Group synonyms: es6/es2015
    let enum_map = option.enum_map().expect("nil enum map dereference");
    // PORT: Go uses an ordered map keyed by the `any` value. The values are
    // not hashable here, so this is a list in insertion order.
    let mut inverted: Vec<(&CompilerOptionsValue, Vec<&str>)> = Vec::with_capacity(enum_map.len());
    let deprecated_keys = option.deprecated_keys();

    for (name, value) in enum_map {
        if !deprecated_keys.is_some_and(|keys| keys.contains(name)) {
            match inverted.iter_mut().find(|(key, _)| *key == value) {
                Some((_, names)) => names.push(name.as_str()),
                None => inverted.push((value, vec![name.as_str()])),
            }
        }
    }
    let syns: Vec<String> = inverted
        .iter()
        .map(|(_, synonyms)| synonyms.join("/"))
        .collect();
    syns.join(", ")
}

// Go: execute/tsc/help.go:393 getPrettyOutput
// PORT: Go cuts `right` at a byte index, so a cut inside a UTF-8 sequence
// writes invalid UTF-8. The loop cuts the Go bytes of `right` and keeps
// each piece in port form (see `scanner_util::GO_STRING_MARKER`). The
// process output writes the Go bytes back (`compile::GoOutput`).
fn get_pretty_output(
    colors: &Colors,
    left: &str,
    right: &str,
    right_align_of_left: i32,
    left_align_of_right: i32,
    terminal_width: i32,
    color_left: bool,
) -> Vec<String> {
    // !!! How does terminalWidth interact with UTF-8 encoding? Strada just assumed UTF-16.
    let mut res = Vec::with_capacity(4);
    let mut is_first_line = true;
    let right = go_string_bytes(right);
    let mut remain_right: &[u8] = &right;
    let right_character_number = terminal_width - left_align_of_right;
    while !remain_right.is_empty() {
        let cur_left = if is_first_line {
            let cur_left = go_pad(left, right_align_of_left, false);
            let cur_left = go_pad(&cur_left, left_align_of_right, true);
            if color_left {
                colors.blue(&cur_left)
            } else {
                cur_left
            }
        } else {
            go_repeat(" ", left_align_of_right)
        };

        let idx = right_character_number.min(remain_right.len() as i32);
        let Ok(idx) = usize::try_from(idx) else {
            panic!("slice bounds out of range [:{idx}]");
        };
        let (cur_right, rest) = remain_right.split_at(idx);
        remain_right = rest;
        res.extend([
            cur_left,
            go_string_from_bytes(cur_right.to_vec()),
            "\n".to_string(),
        ]);
        is_first_line = false;
    }
    res
}

// Go: execute/tsc/help.go:420 getDisplayNameTextOfOption
fn get_display_name_text_of_option(option: &CommandLineOption) -> String {
    format!(
        "--{}{}",
        option.name,
        if !option.short_name.is_empty() {
            format!(", -{}", option.short_name)
        } else {
            String::new()
        }
    )
}
