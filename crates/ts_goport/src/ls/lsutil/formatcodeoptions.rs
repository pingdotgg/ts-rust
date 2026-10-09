use crate::ls::lsutil::prelude::*;

use crate::flags_macros::go_enum;
use crate::gostd::unicode;

// Port of Go `ls/lsutil/formatcodeoptions.go`.

// Go: ls/lsutil/formatcodeoptions.go:11 IndentStyle
go_enum!(IndentStyle, i32 {
    NONE = 0;
    BLOCK = 1;
    SMART = 2;
});

// Go: ls/lsutil/formatcodeoptions.go:19 parseIndentStyle (at 673a5f17d713; ts#64554 generates it: ls/lsutil/userpreferences_generated.go:193 parsePreferenceIndentStyle)
// PORT: Go `v any` is an `LspAny`. Go also accepts a Go `int`; an `LspAny`
// number is always the Go `float64` case. Go `int(float64)` keeps 64 bits;
// the Rust field is `i32` (PORTING `int` -> `i32`), so a value outside the
// `i32` range differs from Go.
pub fn parse_indent_style(v: &LspAny) -> IndentStyle {
    match v {
        LspAny::String(s) => match strings_to_lower(s).as_str() {
            "none" => return IndentStyle::NONE,
            "block" => return IndentStyle::BLOCK,
            "smart" => return IndentStyle::SMART,
            _ => {}
        },
        LspAny::Number(s) => return IndentStyle(*s as i64 as i32),
        _ => {}
    }
    IndentStyle::SMART
}

// Go: ls/lsutil/formatcodeoptions.go:17 SemicolonPreference
// PORT: a Go string type. The value is always one of the Go constants or the
// zero value "", so a `&'static str` holds it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct SemicolonPreference(pub &'static str);

impl SemicolonPreference {
    pub const IGNORE: SemicolonPreference = SemicolonPreference("ignore");
    pub const INSERT: SemicolonPreference = SemicolonPreference("insert");
    pub const REMOVE: SemicolonPreference = SemicolonPreference("remove");
}

// Go: ls/lsutil/formatcodeoptions.go:46 parseSemicolonPreference (at 673a5f17d713; ts#64554 generates it: ls/lsutil/userpreferences_generated.go:357 parsePreferenceSemicolonPreference)
pub fn parse_semicolon_preference(v: &LspAny) -> SemicolonPreference {
    if let LspAny::String(s) = v {
        match strings_to_lower(s).as_str() {
            "ignore" => return SemicolonPreference::IGNORE,
            "insert" => return SemicolonPreference::INSERT,
            "remove" => return SemicolonPreference::REMOVE,
            _ => {}
        }
    }
    SemicolonPreference::IGNORE
}

// Go: ls/lsutil/formatcodeoptions.go:60 EditorSettings (at 673a5f17d713; ts#64554 generates it: ls/lsutil/userpreferences_generated.go:43 EditorSettings)
// PORT: the Go `raw` and `config` tags are in the field table in
// userpreferences.rs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EditorSettings {
    pub base_indent_size: i32,
    pub indent_size: i32,
    pub tab_size: i32,
    pub new_line_character: String,
    pub convert_tabs_to_spaces: Tristate,
    pub indent_style: IndentStyle,
    pub trim_trailing_whitespace: Tristate,
}

// Go: ls/lsutil/formatcodeoptions.go:70 FormatCodeSettings (at 673a5f17d713; ts#64554 generates it: ls/lsutil/userpreferences_generated.go:53 FormatCodeSettings)
// PORT: Go embeds `EditorSettings`. It is the nested field `editor_settings`
// (Go `opts.IndentSize` is `opts.editor_settings.indent_size`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FormatCodeSettings {
    pub editor_settings: EditorSettings,
    pub insert_space_after_comma_delimiter: Tristate,
    pub insert_space_after_semicolon_in_for_statements: Tristate,
    pub insert_space_before_and_after_binary_operators: Tristate,
    pub insert_space_after_constructor: Tristate,
    pub insert_space_after_keywords_in_control_flow_statements: Tristate,
    pub insert_space_after_function_keyword_for_anonymous_functions: Tristate,
    pub insert_space_after_opening_and_before_closing_nonempty_parenthesis: Tristate,
    pub insert_space_after_opening_and_before_closing_nonempty_brackets: Tristate,
    pub insert_space_after_opening_and_before_closing_nonempty_braces: Tristate,
    pub insert_space_after_opening_and_before_closing_empty_braces: Tristate,
    pub insert_space_after_opening_and_before_closing_template_string_braces: Tristate,
    pub insert_space_after_opening_and_before_closing_jsx_expression_braces: Tristate,
    pub insert_space_after_type_assertion: Tristate,
    pub insert_space_before_function_parenthesis: Tristate,
    pub place_open_brace_on_new_line_for_functions: Tristate,
    pub place_open_brace_on_new_line_for_control_blocks: Tristate,
    pub insert_space_before_type_annotation: Tristate,
    pub indent_multi_line_object_literal_beginning_on_blank_line: Tristate,
    pub semicolons: SemicolonPreference,
    pub indent_switch_case: Tristate,
}

// Go: ls/lsutil/formatcodeoptions.go:25 FromLSFormatOptions
// PORT: Go takes `f` by value; this clones it. Go `int(uint32)` keeps every
// value; the Rust `i32` field wraps above `i32::MAX`.
pub fn from_ls_format_options(
    f: &FormatCodeSettings,
    opt: &crate::lsp::lsproto::FormattingOptions,
) -> FormatCodeSettings {
    let mut updated_settings = f.clone();
    updated_settings.editor_settings.tab_size = opt.tab_size as i32;
    updated_settings.editor_settings.indent_size = opt.tab_size as i32;
    updated_settings.editor_settings.convert_tabs_to_spaces = bool_to_tristate(opt.insert_spaces);
    if let Some(trim_trailing_whitespace) = opt.trim_trailing_whitespace {
        updated_settings.editor_settings.trim_trailing_whitespace =
            bool_to_tristate(trim_trailing_whitespace);
    }
    updated_settings
}

impl FormatCodeSettings {
    // Go: ls/lsutil/formatcodeoptions.go:36 (FormatCodeSettings).ToLSFormatOptions
    // PORT: Go returns a new `*lsproto.FormattingOptions`; this returns the
    // value. Go `uint32(int)` wraps like `as u32`.
    pub fn to_ls_format_options(&self) -> crate::lsp::lsproto::FormattingOptions {
        let trim_trailing_whitespace = self.editor_settings.trim_trailing_whitespace.is_true();
        crate::lsp::lsproto::FormattingOptions {
            tab_size: self.editor_settings.tab_size as u32,
            insert_spaces: self.editor_settings.convert_tabs_to_spaces.is_true(),
            trim_trailing_whitespace: Some(trim_trailing_whitespace),
            ..Default::default()
        }
    }
}

// Go: ls/lsutil/formatcodeoptions.go:45 GetDefaultFormatCodeSettings
pub fn get_default_format_code_settings() -> FormatCodeSettings {
    FormatCodeSettings {
        editor_settings: EditorSettings {
            indent_size: crate::printer::get_default_indent_size(),
            tab_size: crate::printer::get_default_indent_size(),
            new_line_character: "\n".to_string(),
            convert_tabs_to_spaces: Tristate::True,
            indent_style: IndentStyle::SMART,
            trim_trailing_whitespace: Tristate::True,
            ..Default::default()
        },
        insert_space_after_constructor: Tristate::False,
        insert_space_after_comma_delimiter: Tristate::True,
        insert_space_after_semicolon_in_for_statements: Tristate::True,
        insert_space_before_and_after_binary_operators: Tristate::True,
        insert_space_after_keywords_in_control_flow_statements: Tristate::True,
        insert_space_after_function_keyword_for_anonymous_functions: Tristate::False,
        insert_space_after_opening_and_before_closing_nonempty_parenthesis: Tristate::False,
        insert_space_after_opening_and_before_closing_nonempty_brackets: Tristate::False,
        insert_space_after_opening_and_before_closing_nonempty_braces: Tristate::True,
        insert_space_after_opening_and_before_closing_template_string_braces: Tristate::False,
        insert_space_after_opening_and_before_closing_jsx_expression_braces: Tristate::False,
        insert_space_before_function_parenthesis: Tristate::False,
        place_open_brace_on_new_line_for_functions: Tristate::False,
        place_open_brace_on_new_line_for_control_blocks: Tristate::False,
        semicolons: SemicolonPreference::IGNORE,
        indent_switch_case: Tristate::True,
        ..Default::default()
    }
}

// PORT: Go `strings.ToLower` maps each rune with `unicode.ToLower`. Rust
// `str::to_lowercase` uses the full mapping of a newer Unicode and the
// final-sigma rule, which Go does not.
fn strings_to_lower(s: &str) -> String {
    s.chars().map(unicode::to_lower).collect()
}
